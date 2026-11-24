//! The cleaner rs-du (console) and WinRust Disk (window) share: what can go, how risky that is and how it goes.
//! Safe and Rebuild items come back on their own and are picked by default; Ask items (old or duplicate files, recycle bins,
//! Windows.old) only go when chosen, and the files among them wait in a per-drive quarantine for 30 days first.
//! Needs admin: the MFT read (mft::scan), Windows\Temp, other users' recycle bins.
//! shortcuts: Windows.old/SoftwareDistribution may need ownership rights (reported as partly kept); leftovers of uninstalled
//! programs in AppData are not detected; no restore point is created (the quarantine is the undo).
use crate::mft::{Rec, path_of, scan, totals, ROOT};
use crate::{t, tf};
use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    env, fs,
    io::{Read, Write},
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};

pub const QUAR_DAYS: u64 = 30;
const DUP_MIN: u64 = 64 << 20; // smaller copies are not worth a question
const FRESH: u64 = 2 * 3600; // what was touched this recently stays: an installer may still be running from Temp

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Risk {
    Safe,
    Rebuild,
    Ask,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum How {
    Wipe,       // empty the folder, keep it (a lone file is removed)
    Remove,     // the folder or file goes
    Quarantine, // moved aside for 30 days, can come back
}

#[derive(Clone, Debug)]
pub struct Item {
    pub risk: Risk,
    pub how: How,
    pub label: String,
    pub note: String,
    pub paths: Vec<PathBuf>,
    pub bytes: u64,
}

impl Item {
    /// Picked unless the user has to decide.
    pub fn default_pick(&self) -> bool {
        self.risk != Risk::Ask
    }

    /// What becomes of it, for the ones that do not come back by themselves ("" for the rest).
    pub fn fate(&self) -> &'static str {
        match (self.risk, self.how) {
            (Risk::Ask, How::Quarantine) => t!("Уйдёт в карантин на 30 дней, можно вернуть.", "It goes to quarantine for 30 days and can be restored.", ja: "30 日間は隔離され、復元できます。", pt: "Vai para a quarentena por 30 dias e pode ser restaurado.", zh: "会进入隔离区保留 30 天，可以恢复。", es: "Va a cuarentena durante 30 días y se puede restaurar."),
            (Risk::Ask, _) => t!("Удалится навсегда.", "It is deleted permanently.", ja: "完全に削除されます。", pt: "É excluído permanentemente.", zh: "将永久删除。", es: "Se elimina de forma permanente."),
            _ => "",
        }
    }
}

/// The heading of a risk group.
pub fn risk_title(r: Risk) -> &'static str {
    match r {
        Risk::Safe => t!("БЕЗОПАСНО: Windows и программы создадут это заново сами", "SAFE: Windows and programs recreate this on their own", ja: "安全: Windows とプログラムが自動的に再作成します", pt: "SEGURO: o Windows e os programas recriam isso sozinhos", zh: "安全：Windows 和程序会自行重新创建", es: "SEGURO: Windows y los programas lo vuelven a crear solos"),
        Risk::Rebuild => t!("МОЖНО УДАЛИТЬ: пересоздастся, но первое время может работать чуть медленнее", "CAN BE DELETED: it is recreated, but may run a little slower for a while", ja: "削除可能: 再作成されますが、しばらく動作が少し遅くなることがあります", pt: "PODE EXCLUIR: é recriado, mas pode ficar um pouco mais lento por um tempo", zh: "可以删除：会自动重建，但可能暂时运行稍慢", es: "SE PUEDE ELIMINAR: se vuelve a crear, pero puede ir un poco más lento durante un tiempo"),
        Risk::Ask => t!("ТОЛЬКО ПО ВАШЕМУ ВЫБОРУ: проверьте, что это вам не нужно", "YOUR CHOICE ONLY: make sure you do not need this", ja: "ご自身で判断: 不要かどうか確認してください", pt: "SÓ VOCÊ DECIDE: confirme que você não precisa disso", zh: "仅由你决定：请确认你不需要此项", es: "SOLO SI TÚ LO DECIDES: asegúrate de que no lo necesitas"),
    }
}

/// One drive as mft::scan read it, with mft::totals (allocated bytes) over it.
pub struct Drive {
    pub letter: char,
    pub recs: HashMap<u32, Rec>,
    pub tot: HashMap<u32, u64>,
}

/// What `plan` needs from one drive. A drive's MFT takes a few hundred MB, so each is boiled down to this and let go
/// before the next one is read.
pub struct Digest {
    letter: char,
    projects: Vec<PathBuf>,
    project_bytes: u64,
    shaders: Vec<PathBuf>,
    big: Vec<(u64, u64, PathBuf)>, // user files of DUP_MIN and up: (size, bytes on disk, path)
}

pub fn digest(d: &Drive) -> Digest {
    let (projects, project_bytes) = projects(d);
    Digest { letter: d.letter, projects, project_bytes, shaders: steam_shaders(d), big: user_files(d, DUP_MIN) }
}

/// The fixed drives (no USB sticks, no network shares).
pub fn fixed_drives() -> Vec<char> {
    let mask = unsafe { GetLogicalDrives() };
    ('A'..='Z')
        .filter(|&c| mask & (1 << (c as u8 - b'A')) != 0)
        .filter(|c| unsafe { GetDriveTypeW(format!("{c}:\\").encode_utf16().chain([0]).collect::<Vec<u16>>().as_ptr()) } == 3) // DRIVE_FIXED
        .collect()
}

/// Reads every fixed NTFS drive, one at a time; the others (FAT disks, ReFS) are skipped. `say` hears which one is read.
pub fn scan_all(say: &mut dyn FnMut(String)) -> Vec<Digest> {
    fixed_drives()
        .into_iter()
        .filter_map(|letter| {
            say(tf!("Чтение диска {letter}:", "Reading drive {letter}:", ja: "ドライブ {letter}: を読み込んでいます", pt: "Lendo a unidade {letter}:", zh: "正在读取磁盘 {letter}:", es: "Leyendo la unidad {letter}:"));
            let recs = scan(letter).ok()?;
            let tot = totals(&recs, false);
            Some(digest(&Drive { letter, recs, tot }))
        })
        .collect()
}

/// Bytes under `p` (or the file itself), not following symlinks/junctions.
pub fn dir_size(p: &Path) -> u64 {
    let Ok(m) = fs::symlink_metadata(p) else { return 0 };
    if !m.is_dir() {
        return if m.file_type().is_symlink() { 0 } else { m.len() };
    }
    fs::read_dir(p).into_iter().flatten().flatten().map(|e| dir_size(&e.path())).sum()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// True when nothing touched the file (written, read, created) for `secs`.
fn quiet(m: &fs::Metadata, secs: u64) -> bool {
    [m.modified(), m.accessed(), m.created()].into_iter().flatten().max().is_some_and(|t| t.elapsed().is_ok_and(|e| e >= Duration::from_secs(secs)))
}

const DAY: u64 = 86400;

/// A dir is idle when it and its direct children (release/, debug/, .package-lock.json...) are quiet for `days`.
fn idle(p: &Path, days: u64) -> bool {
    p.metadata().is_ok_and(|m| quiet(&m, days * DAY)) && fs::read_dir(p).into_iter().flatten().flatten().all(|e| e.metadata().is_ok_and(|m| quiet(&m, days * DAY)))
}

/// Empties a folder but keeps it (a lone file is removed). What is in use, or was touched in the last `keep` seconds, stays.
pub fn wipe(p: &Path, keep: u64) {
    let young = |m: &fs::Metadata| keep > 0 && !quiet(m, keep);
    if p.is_file() {
        if !p.metadata().is_ok_and(|m| young(&m)) {
            let _ = fs::remove_file(p);
        }
        return;
    }
    for e in fs::read_dir(p).into_iter().flatten().flatten() {
        if e.metadata().is_ok_and(|m| young(&m)) {
            continue;
        }
        let q = e.path();
        let _ = if e.file_type().is_ok_and(|t| t.is_dir()) { fs::remove_dir_all(&q) } else { fs::remove_file(&q) }; // locked: skipped
    }
}

fn remove(p: &Path) {
    let _ = if p.is_dir() { fs::remove_dir_all(p) } else { fs::remove_file(p) };
}

fn subdirs(base: &Path, pred: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    fs::read_dir(base).into_iter().flatten().flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && pred(&e.file_name().to_string_lossy())).map(|e| e.path()).collect()
}

/// The known places. Paths that do not exist are dropped later, so rules for software you do not have cost nothing.
fn rules() -> Vec<Item> {
    let var = |k: &str| env::var_os(k).map(PathBuf::from).filter(|p| p.is_absolute());
    let (local, roam, home, win, pdata) = (var("LOCALAPPDATA"), var("APPDATA"), var("USERPROFILE"), var("SystemRoot"), var("ProgramData"));
    let at = |b: &Option<PathBuf>, rel: &[&str]| -> Vec<PathBuf> { b.iter().flat_map(|b| rel.iter().map(move |r| b.join(r))).collect() };
    let mut v: Vec<Item> = vec![];
    let mut add = |risk, label: &str, note: &str, paths: Vec<PathBuf>| v.push(Item { risk, how: How::Wipe, label: label.into(), note: note.into(), paths, bytes: 0 });

    add(Risk::Safe, t!("Временные файлы", "Temporary files", ja: "一時ファイル", pt: "Arquivos temporários", zh: "临时文件", es: "Archivos temporales"), t!("Мусор от программ и установщиков. Программы создадут нужное заново.", "Junk left by programs and installers. Programs will recreate what they need.", ja: "プログラムやインストーラーが残した不要ファイルです。必要なものはプログラムが再作成します。", pt: "Lixo deixado por programas e instaladores. Os programas recriam o que precisarem.", zh: "程序和安装程序留下的垃圾文件。程序会重新创建所需内容。", es: "Restos que dejan los programas y los instaladores. Los programas vuelven a crear lo que necesitan."),
        [var("TEMP").into_iter().collect::<Vec<_>>(), at(&win, &["Temp"])].concat());
    add(Risk::Safe, t!("Отчёты об ошибках и дампы падений", "Error reports and crash dumps", ja: "エラーレポートとクラッシュダンプ", pt: "Relatórios de erros e despejos de falhas", zh: "错误报告和崩溃转储", es: "Informes de errores y volcados de bloqueo"), t!("Данные о сбоях программ для разработчиков. Вам они не нужны.", "Program crash data for developers. You do not need it.", ja: "開発者向けのプログラムのクラッシュデータです。通常は不要です。", pt: "Dados de falhas de programas para desenvolvedores. Você não precisa deles.", zh: "供开发者使用的程序崩溃数据。你不需要它。", es: "Datos de bloqueos de programas para sus desarrolladores. No los necesitas."),
        [at(&pdata, &["Microsoft/Windows/WER/ReportArchive", "Microsoft/Windows/WER/ReportQueue"]), at(&local, &["CrashDumps"]), at(&win, &["Minidump", "MEMORY.DMP", "LiveKernelReports"])].concat());
    add(Risk::Safe, t!("Кеш обновлений Windows", "Windows Update cache", ja: "Windows Update のキャッシュ", pt: "Cache do Windows Update", zh: "Windows 更新缓存", es: "Caché de Windows Update"), t!("Уже установленные обновления. Что понадобится, Windows скачает заново.", "Updates that are already installed. Windows re-downloads what it needs.", ja: "インストール済みの更新プログラムです。必要なものは Windows が再ダウンロードします。", pt: "Atualizações já instaladas. O Windows baixa de novo o que precisar.", zh: "已安装的更新。Windows 会重新下载所需内容。", es: "Actualizaciones que ya están instaladas. Windows vuelve a descargar lo que necesite."),
        at(&win, &["SoftwareDistribution/Download", "ServiceProfiles/NetworkService/AppData/Local/Microsoft/Windows/DeliveryOptimization/Cache"]));
    add(Risk::Safe, t!("Журналы установки Windows", "Windows setup logs", ja: "Windows のセットアップ ログ", pt: "Logs de instalação do Windows", zh: "Windows 安装日志", es: "Registros de instalación de Windows"), t!("Записи о том, как ставились обновления. Нужны только при разборе сбоя.", "Notes on how updates were installed. Only needed to investigate a failure.", ja: "更新プログラムのインストール記録です。障害の調査時にのみ必要です。", pt: "Registros de como as atualizações foram instaladas. Só servem para investigar uma falha.", zh: "更新安装过程的记录。仅在排查故障时需要。", es: "Notas de cómo se instalaron las actualizaciones. Solo sirven para investigar un fallo."), at(&win, &["Logs/CBS", "Logs/DISM"]));
    add(Risk::Safe, t!("Кеш интернета Windows", "Windows internet cache", ja: "Windows のインターネット キャッシュ", pt: "Cache de internet do Windows", zh: "Windows 互联网缓存", es: "Caché de internet de Windows"), t!("Временные копии страниц и картинок для системных программ.", "Temporary copies of pages and images for system programs.", ja: "システム プログラム用のページと画像の一時コピーです。", pt: "Cópias temporárias de páginas e imagens para programas do sistema.", zh: "系统程序使用的页面和图片临时副本。", es: "Copias temporales de páginas e imágenes para programas del sistema."), at(&local, &["Microsoft/Windows/INetCache"]));
    add(Risk::Safe, t!("Эскизы и кеш значков", "Thumbnails and icon cache", ja: "縮小表示とアイコン キャッシュ", pt: "Miniaturas e cache de ícones", zh: "缩略图和图标缓存", es: "Miniaturas y caché de íconos"), t!("Картинки-превью в папках. Нарисуются заново, когда откроете папку.", "Preview pictures in folders. They are drawn again when you open a folder.", ja: "フォルダー内のプレビュー画像です。フォルダーを開くと再作成されます。", pt: "Prévias de imagens nas pastas. São desenhadas de novo ao abrir a pasta.", zh: "文件夹中的预览图。打开文件夹时会重新生成。", es: "Vistas previas de las carpetas. Se vuelven a dibujar al abrir la carpeta."), at(&local, &["Microsoft/Windows/Explorer"]));

    let web = t!("Копии страниц и картинок. Сайты первый раз откроются чуть медленнее. Пароли, закладки и история не затрагиваются.", "Copies of pages and images. Sites open a little slower the first time. Passwords, bookmarks and history are not touched.", ja: "ページと画像のコピーです。初回はサイトの表示が少し遅くなります。パスワード、ブックマーク、履歴は削除されません。", pt: "Cópias de páginas e imagens. Os sites abrem um pouco mais devagar na primeira vez. Senhas, favoritos e histórico não são afetados.", zh: "页面和图片的副本。网站首次打开会稍慢。密码、书签和历史记录不受影响。", es: "Copias de páginas e imágenes. Los sitios abren un poco más lento la primera vez. No se tocan las contraseñas, los marcadores ni el historial.");
    let chromium = ["Cache/Cache_Data", "Code Cache", "GPUCache", "Service Worker/CacheStorage"];
    for (name, rel) in [("Google Chrome", "Google/Chrome/User Data"), ("Microsoft Edge", "Microsoft/Edge/User Data"), (t!("Яндекс Браузер", "Yandex Browser", ja: "Yandex ブラウザー", pt: "Yandex Browser", zh: "Yandex 浏览器", es: "Yandex Browser"), "Yandex/YandexBrowser/User Data"), ("Brave", "BraveSoftware/Brave-Browser/User Data"), ("Vivaldi", "Vivaldi/User Data")] {
        let ps = local.as_ref().map_or(vec![], |l| subdirs(&l.join(rel), |n| n == "Default" || n.starts_with("Profile ")).into_iter().flat_map(|p| chromium.map(|c| p.join(c))).collect());
        add(Risk::Rebuild, name, web, ps);
    }
    // Opera keeps the disk cache under Local and the rest of the profile under Roaming
    let opera: Vec<PathBuf> = ["Opera Stable", "Opera GX Stable"]
        .iter()
        .flat_map(|o| [at(&local, &[&format!("Opera Software/{o}/Cache"), &format!("Opera Software/{o}/Default/Cache")]), at(&roam, &[&format!("Opera Software/{o}/Default/Code Cache"), &format!("Opera Software/{o}/Default/GPUCache"), &format!("Opera Software/{o}/GPUCache")])].concat())
        .collect();
    add(Risk::Rebuild, "Opera, Opera GX", web, opera);
    let ff = local.as_ref().map(|l| l.join("Mozilla/Firefox/Profiles"));
    add(Risk::Rebuild, "Firefox", web, ff.map_or(vec![], |ff| subdirs(&ff, |_| true).into_iter().map(|p| p.join("cache2")).collect()));
    add(Risk::Rebuild, t!("Telegram: кеш медиа", "Telegram: media cache", ja: "Telegram: メディア キャッシュ", pt: "Telegram: cache de mídia", zh: "Telegram：媒体缓存", es: "Telegram: caché multimedia"), t!("Картинки и видео из чатов. Скачаются снова, когда откроете чат.", "Pictures and videos from chats. They download again when you open a chat.", ja: "チャットの画像と動画です。チャットを開くと再ダウンロードされます。", pt: "Fotos e vídeos dos chats. Eles são baixados de novo ao abrir um chat.", zh: "聊天中的图片和视频。打开聊天时会重新下载。", es: "Fotos y videos de los chats. Se vuelven a descargar al abrir un chat."), at(&roam, &["Telegram Desktop/tdata/user_data/media_cache", "Telegram Desktop/tdata/user_data/cache"]));
    add(Risk::Rebuild, "Discord", t!("Кеш картинок и интерфейса. Создастся заново.", "Image and UI cache. It will be recreated.", ja: "画像と UI のキャッシュです。再作成されます。", pt: "Cache de imagens e da interface. Será recriado.", zh: "图片和界面缓存。会自动重建。", es: "Caché de imágenes y de la interfaz. Se volverá a crear."), at(&roam, &["discord/Cache", "discord/Code Cache", "discord/GPUCache"]));
    add(Risk::Rebuild, "Microsoft Teams", t!("Кеш интерфейса и вложений. Создастся заново.", "UI and attachment cache. It will be recreated.", ja: "UI と添付ファイルのキャッシュです。再作成されます。", pt: "Cache da interface e de anexos. Será recriado.", zh: "界面和附件缓存。会自动重建。", es: "Caché de la interfaz y de los adjuntos. Se volverá a crear."), at(&roam, &["Microsoft/Teams/Cache", "Microsoft/Teams/blob_storage", "Microsoft/Teams/GPUCache", "Microsoft/Teams/tmp"]));
    add(Risk::Rebuild, "Visual Studio Code", t!("Кеш редактора. Создастся заново.", "Editor cache. It will be recreated.", ja: "エディターのキャッシュです。再作成されます。", pt: "Cache do editor. Será recriado.", zh: "编辑器缓存。会自动重建。", es: "Caché del editor. Se volverá a crear."), at(&roam, &["Code/Cache", "Code/CachedData", "Code/GPUCache", "Code/Code Cache"]));
    add(Risk::Rebuild, t!("Кеш шейдеров видеокарты (NVIDIA, AMD, DirectX)", "GPU shader cache (NVIDIA, AMD, DirectX)", ja: "GPU シェーダー キャッシュ (NVIDIA、AMD、DirectX)", pt: "Cache de shaders da GPU (NVIDIA, AMD, DirectX)", zh: "GPU 着色器缓存（NVIDIA、AMD、DirectX）", es: "Caché de sombreadores de la GPU (NVIDIA, AMD, DirectX)"), t!("Игры пересоберут его сами, первый запуск игры может чуть подтормаживать.", "Games rebuild it themselves, the first launch of a game may stutter a little.", ja: "ゲームが自動的に再構築します。ゲームの初回起動時に少しカクつくことがあります。", pt: "Os jogos o recriam sozinhos; a primeira abertura de um jogo pode engasgar um pouco.", zh: "游戏会自行重建，首次启动游戏时可能略有卡顿。", es: "Los juegos la reconstruyen solos; el primer inicio de un juego puede dar algunos tirones."),
        at(&local, &["NVIDIA/DXCache", "NVIDIA/GLCache", "D3DSCache", "AMD/DxCache", "AMD/DxcCache"]));
    add(Risk::Rebuild, t!("Steam: кеш браузера", "Steam: browser cache", ja: "Steam: ブラウザー キャッシュ", pt: "Steam: cache do navegador", zh: "Steam：浏览器缓存", es: "Steam: caché del navegador"), t!("Кеш встроенного браузера Steam. Игры не затрагиваются.", "Cache of Steam's built-in browser. Games are not touched.", ja: "Steam 内蔵ブラウザーのキャッシュです。ゲームには影響しません。", pt: "Cache do navegador integrado do Steam. Os jogos não são afetados.", zh: "Steam 内置浏览器的缓存。不会影响游戏。", es: "Caché del navegador integrado de Steam. No se tocan los juegos."), at(&local, &["Steam/htmlcache"]));
    add(Risk::Rebuild, t!("Кеши программистов (cargo, npm, pip, NuGet, Gradle, Go)", "Developer caches (cargo, npm, pip, NuGet, Gradle, Go)", ja: "開発者向けキャッシュ (cargo、npm、pip、NuGet、Gradle、Go)", pt: "Caches de desenvolvedor (cargo, npm, pip, NuGet, Gradle, Go)", zh: "开发者缓存（cargo、npm、pip、NuGet、Gradle、Go）", es: "Cachés de desarrollo (cargo, npm, pip, NuGet, Gradle, Go)"), t!("Скачанные пакеты. Загрузятся снова при следующей сборке.", "Downloaded packages. They download again on the next build.", ja: "ダウンロード済みのパッケージです。次回のビルドで再ダウンロードされます。", pt: "Pacotes baixados. Eles são baixados de novo na próxima compilação.", zh: "已下载的软件包。下次构建时会重新下载。", es: "Paquetes descargados. Se vuelven a descargar en la próxima compilación."),
        [at(&home, &[".cargo/registry/cache", ".cargo/registry/src", ".gradle/caches"]), at(&local, &["npm-cache", "pip/cache", "NuGet/v3-cache", "Yarn/Cache", "go-build"]), at(&roam, &["npm-cache"])].concat());
    if let Some(sys) = win.as_ref().and_then(|w| w.parent()) {
        v.push(Item { risk: Risk::Ask, how: How::Remove, label: t!("Windows.old (прежняя версия Windows)", "Windows.old (previous Windows version)", ja: "Windows.old (以前の Windows)", pt: "Windows.old (versão anterior do Windows)", zh: "Windows.old（以前的 Windows 版本）", es: "Windows.old (versión anterior de Windows)").into(),
            note: t!("Нужна только для отката на старую Windows.", "Only needed to roll back to the old Windows.", ja: "以前の Windows に戻すときにのみ必要です。", pt: "Necessário apenas para voltar ao Windows anterior.", zh: "仅在回滚到旧版 Windows 时才需要。", es: "Solo hace falta para volver al Windows anterior.").into(), paths: vec![sys.join("Windows.old")], bytes: 0 });
        v.push(Item { risk: Risk::Ask, how: How::Remove, label: t!("Остатки обновления Windows ($WINDOWS.~BT, $Windows.~WS)", "Windows upgrade leftovers ($WINDOWS.~BT, $Windows.~WS)", ja: "Windows アップグレードの残り ($WINDOWS.~BT、$Windows.~WS)", pt: "Sobras da atualização do Windows ($WINDOWS.~BT, $Windows.~WS)", zh: "Windows 升级残留（$WINDOWS.~BT、$Windows.~WS）", es: "Restos de la actualización de Windows ($WINDOWS.~BT, $Windows.~WS)").into(),
            note: t!("Файлы прошлой установки новой версии. Не удаляйте, пока обновление ещё идёт.", "Files from the last upgrade to a new version. Do not delete them while an upgrade is still running.", ja: "前回の新バージョンへのアップグレードのファイルです。アップグレード中は削除しないでください。", pt: "Arquivos da última atualização para uma nova versão. Não exclua enquanto uma atualização ainda estiver em andamento.", zh: "上次升级到新版本时留下的文件。升级仍在进行时请勿删除。", es: "Archivos de la última actualización a una versión nueva. No los borres mientras haya una actualización en curso.").into(), paths: vec![sys.join("$WINDOWS.~BT"), sys.join("$Windows.~WS")], bytes: 0 });
    }
    v
}

/// Not ours to judge: the system, programs and their data, game libraries, cloud folders (reading those would download them).
fn hands_off(path: &str) -> bool {
    const NO: [&str; 18] = ["\\windows\\", "\\program files", "\\programdata\\", "\\appdata\\", "\\$recycle.bin", "\\system volume information", "\\.rs-du-quarantine", "\\$windows.~", "\\steamapps\\", "\\steamlibrary\\", "\\epic games\\", "\\xboxgames\\", "\\windowsapps\\", "\\riot games\\", "\\onedrive", "\\google drive", "\\dropbox", "\\yandex.disk"];
    let low = path.to_lowercase();
    NO.iter().any(|k| low.contains(k))
}

/// Project build dirs: target next to Cargo.toml, node_modules next to package.json (outermost only, never inside apps).
fn projects(d: &Drive) -> (Vec<PathBuf>, u64) {
    let recs = &d.recs;
    let is_cand = |r: &Rec| r.dir && (r.name.eq_ignore_ascii_case("target") || r.name.eq_ignore_ascii_case("node_modules"));
    let parents: HashSet<u32> = recs.values().filter(|r| is_cand(r)).map(|r| r.parent).collect();
    let marks: HashSet<(u32, String)> = recs.values().filter(|r| !r.dir && parents.contains(&r.parent) && (r.name.eq_ignore_ascii_case("Cargo.toml") || r.name.eq_ignore_ascii_case("package.json"))).map(|r| (r.parent, r.name.to_ascii_lowercase())).collect();
    let ok = |r: &Rec| marks.contains(&(r.parent, if r.name.eq_ignore_ascii_case("target") { "cargo.toml" } else { "package.json" }.into()));
    let cands: HashSet<u32> = recs.iter().filter(|(_, r)| is_cand(r) && ok(r)).map(|(&i, _)| i).collect();
    let (mut out, mut bytes) = (vec![], 0);
    for &id in &cands {
        let (mut p, mut nested) = (recs[&id].parent, false);
        for _ in 0..128 {
            if cands.contains(&p) {
                nested = true;
                break;
            }
            if p == ROOT {
                break;
            }
            p = match recs.get(&p) {
                Some(x) => x.parent,
                None => break,
            };
        }
        let path = path_of(recs, id, d.letter).0;
        // installed apps ship their own node_modules (VS Code, Discord, Node, Unity...): deleting those breaks them
        let low = path.to_lowercase();
        if nested || low.contains("\\appdata\\") || low[3..].starts_with("program files") || low[3..].starts_with("windows") {
            continue;
        }
        let p = PathBuf::from(path);
        if !idle(&p, 30) {
            continue; // still being built or installed into: somebody is working here
        }
        bytes += d.tot.get(&id).copied().unwrap_or(0);
        out.push(p);
    }
    (out, bytes)
}

/// Steam's per-game shader caches, in every library on the drive (steamapps\shadercache).
fn steam_shaders(d: &Drive) -> Vec<PathBuf> {
    d.recs.iter().filter(|(_, r)| r.dir && r.name.eq_ignore_ascii_case("shadercache") && d.recs.get(&r.parent).is_some_and(|p| p.name.eq_ignore_ascii_case("steamapps"))).map(|(&id, _)| PathBuf::from(path_of(&d.recs, id, d.letter).0)).collect()
}

/// Old installers and archives lying in Downloads (top level only).
fn old_downloads() -> Option<Item> {
    const EXT: [&str; 7] = ["exe", "msi", "msix", "iso", "zip", "rar", "7z"];
    let dl = env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads"))?;
    let paths: Vec<PathBuf> = fs::read_dir(&dl)
        .ok()?
        .flatten()
        .filter(|e| e.metadata().is_ok_and(|m| m.is_file() && quiet(&m, 90 * DAY)) && e.path().extension().is_some_and(|x| EXT.contains(&x.to_string_lossy().to_lowercase().as_str())))
        .map(|e| e.path())
        .collect();
    Some(Item { risk: Risk::Ask, how: How::Quarantine, label: tf!("Старые установщики и архивы в «Загрузках» ({} шт., старше 90 дней)", "Old installers and archives in Downloads ({} items, older than 90 days)", ja: "ダウンロード フォルダー内の古いインストーラーとアーカイブ ({} 個、90 日以上前)", pt: "Instaladores e arquivos antigos em Downloads ({} itens, com mais de 90 dias)", zh: "下载文件夹中的旧安装程序和压缩包（{} 项，超过 90 天）", es: "Instaladores y archivos comprimidos viejos en Descargas ({} elementos, de más de 90 días)", paths.len()),
        note: t!("Скорее всего, программы уже установлены.", "Most likely the programs are already installed.", ja: "おそらくそのプログラムはすでにインストール済みです。", pt: "Provavelmente os programas já estão instalados.", zh: "程序很可能已经安装。", es: "Lo más probable es que esos programas ya estén instalados.").into(), paths, bytes: 0 })
}

/// A file whose bytes are elsewhere (a cloud placeholder): opening it would download it.
fn online_only(m: &fs::Metadata) -> bool {
    m.file_attributes() & (0x1000 | 0x40000 | 0x40_0000) != 0 // OFFLINE, RECALL_ON_OPEN, RECALL_ON_DATA_ACCESS
}

/// Files of `min` bytes and up that the user put on the drive (not the system's, not programs').
fn user_files(d: &Drive, min: u64) -> Vec<(u64, u64, PathBuf)> {
    let mut v = vec![];
    let mut seen = HashMap::new();
    for (&id, r) in &d.recs {
        if !r.dir && r.logical >= min {
            let p = path_of(&d.recs, id, d.letter).0;
            if !hands_off(&p) && !program_file(Path::new(&p), &mut seen) {
                v.push((r.logical, d.tot.get(&id).copied().unwrap_or(r.alloc), PathBuf::from(p)));
            }
        }
    }
    v
}

/// Whether `p` belongs to a program: outside the user's own folders, a file with an .exe or .dll in its folder or in any
/// folder above it (the drive root aside) is part of that program, a game in C:\Games say, even when it looks like a stray
/// copy (a game's "Trial.exe" is byte for byte its main exe). What lies in Downloads, Desktop, Documents, Videos, Music or
/// Pictures is the user's, installers included. `seen` remembers the folders already looked into.
fn program_file(p: &Path, seen: &mut HashMap<PathBuf, bool>) -> bool {
    const MINE: [&str; 6] = ["\\downloads\\", "\\desktop\\", "\\documents\\", "\\videos\\", "\\music\\", "\\pictures\\"];
    let low = p.to_string_lossy().to_lowercase();
    if low.contains("\\users\\") && MINE.iter().any(|m| low.contains(m)) {
        return false;
    }
    // an Unreal game keeps its exe down in Binaries\Win64 and its movies in Content: the Binaries/Engine folder gives it away
    let has_code = |dir: &Path| {
        fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if e.file_type().is_ok_and(|t| t.is_dir()) { n == "binaries" || n == "engine" } else { n.ends_with(".exe") || n.ends_with(".dll") }
        })
    };
    p.ancestors().skip(1).filter(|a| a.parent().is_some()).any(|dir| *seen.entry(dir.to_path_buf()).or_insert_with(|| has_code(dir)))
}

/// The biggest forgotten videos, disk images, archives and installers on every drive.
fn big_old(ds: &[Digest], skip: &HashSet<PathBuf>) -> Vec<Item> {
    const EXT: [&str; 12] = ["mp4", "mkv", "avi", "mov", "wmv", "iso", "img", "zip", "rar", "7z", "exe", "msi"];
    let mut c: Vec<(u64, u64, PathBuf)> = ds.iter().flat_map(|d| d.big.iter().cloned()).filter(|(size, _, p)| *size >= 1 << 30 && p.extension().is_some_and(|e| EXT.contains(&e.to_string_lossy().to_lowercase().as_str())) && !skip.contains(p)).collect();
    c.sort_by_key(|c| Reverse(c.1));
    c.into_iter()
        .filter(|(_, _, p)| p.metadata().is_ok_and(|m| !online_only(&m) && quiet(&m, 180 * DAY)))
        .take(20)
        .map(|(_, bytes, p)| Item { risk: Risk::Ask, how: How::Quarantine, label: p.to_string_lossy().into(), note: t!("Большой файл, который давно не открывали.", "A large file that has not been opened for a long time.", ja: "長期間開かれていない大きなファイルです。", pt: "Um arquivo grande que não é aberto há muito tempo.", zh: "很久没有打开过的大文件。", es: "Un archivo grande que no se abre desde hace mucho.").into(), paths: vec![p], bytes })
        .collect()
}

/// Whether two files have the same bytes, read side by side; stops at the first difference.
fn same_bytes(a: &Path, b: &Path) -> bool {
    let (Ok(mut fa), Ok(mut fb)) = (fs::File::open(a), fs::File::open(b)) else { return false };
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let (Ok(na), Ok(nb)) = (fill(&mut fa, &mut ba), fill(&mut fb, &mut bb)) else { return false };
        if na != nb || ba[..na] != bb[..nb] {
            return false;
        }
        if na == 0 {
            return true;
        }
    }
}

/// Reads until the buffer is full or the file ends.
fn fill(f: &mut fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// Sets of files with identical bytes: (the one kept, its copies). Same size first (free, from the MFT), then the bytes.
/// The kept one is the oldest, the original rather than the copy.
fn twins(mut files: Vec<(u64, PathBuf)>, say: &mut dyn FnMut(String)) -> Vec<(PathBuf, Vec<PathBuf>)> {
    files.sort();
    let mut out = vec![];
    for group in files.chunk_by(|a, b| a.0 == b.0).filter(|g| g.len() > 1) {
        let born = |p: &Path| p.metadata().ok().and_then(|m| m.created().ok()).unwrap_or(SystemTime::now());
        let mut left: Vec<PathBuf> = group.iter().map(|f| f.1.clone()).filter(|p| p.metadata().is_ok_and(|m| !online_only(&m))).collect();
        left.sort_by_key(|p| (born(p), p.as_os_str().len()));
        while left.len() > 1 {
            let keep = left.remove(0);
            say(keep.display().to_string());
            let (copies, rest): (Vec<_>, Vec<_>) = left.into_iter().partition(|p| same_bytes(&keep, p));
            if !copies.is_empty() {
                out.push((keep, copies));
            }
            left = rest;
        }
    }
    out
}

/// Copies of the same big file, anywhere on the PC: every copy after the first can go.
fn duplicates(ds: &[Digest], skip: &HashSet<PathBuf>, say: &mut dyn FnMut(String)) -> Vec<Item> {
    let files: Vec<(u64, PathBuf)> = ds.iter().flat_map(|d| d.big.iter()).filter(|f| !skip.contains(&f.2)).map(|(size, _, p)| (*size, p.clone())).collect();
    let mut say_dup = |p: String| say(tf!("Сравнение копий: {}", "Comparing copies: {}", ja: "コピーを比較しています: {}", pt: "Comparando cópias: {}", zh: "正在比较副本：{}", es: "Comparando copias: {}", p));
    twins(files, &mut say_dup)
        .into_iter()
        .flat_map(|(keep, copies)| {
            copies.into_iter().map(move |p| Item { risk: Risk::Ask, how: How::Quarantine, label: p.to_string_lossy().into(), note: tf!("Точная копия файла {}, он остаётся.", "An exact copy of {}, which stays.", ja: "{} とまったく同じコピーです。元のファイルは残ります。", pt: "Uma cópia exata de {}, que continua lá.", zh: "与 {} 完全相同的副本，原文件会保留。", es: "Una copia exacta de {}, que se queda.", keep.display()), bytes: 0, paths: vec![p] })
        })
        .collect()
}

/// Everything that could go, on the drives `ds` were taken from, safest first and biggest first within a risk.
pub fn plan(ds: &[Digest], say: &mut dyn FnMut(String)) -> Vec<Item> {
    say(t!("Поиск мусора и кешей", "Looking for junk and caches", ja: "不要なファイルとキャッシュを探しています", pt: "Procurando lixo e caches", zh: "正在查找垃圾文件和缓存", es: "Buscando basura y cachés").into());
    let mut items = rules();
    let proj: Vec<PathBuf> = ds.iter().flat_map(|d| d.projects.iter().cloned()).collect();
    let pb = ds.iter().map(|d| d.project_bytes).sum();
    let shaders: Vec<PathBuf> = ds.iter().flat_map(|d| d.shaders.iter().cloned()).collect();
    let bins: Vec<PathBuf> = ds.iter().map(|d| PathBuf::from(format!(r"{}:\$Recycle.Bin", d.letter))).collect(); // per-user subfolders need admin
    items.push(Item { risk: Risk::Rebuild, how: How::Wipe, label: t!("Steam: кеш шейдеров игр", "Steam: game shader cache", ja: "Steam: ゲームのシェーダー キャッシュ", pt: "Steam: cache de shaders dos jogos", zh: "Steam：游戏着色器缓存", es: "Steam: caché de sombreadores de los juegos").into(), note: t!("Steam соберёт его заново при запуске игры.", "Steam rebuilds it when a game starts.", ja: "ゲームの起動時に Steam が再構築します。", pt: "O Steam o recria quando um jogo é aberto.", zh: "启动游戏时 Steam 会重新生成。", es: "Steam la vuelve a generar al abrir un juego.").into(), paths: shaders, bytes: 0 });
    items.push(Item { risk: Risk::Rebuild, how: How::Remove, label: t!("Папки сборки проектов (target, node_modules)", "Project build folders (target, node_modules)", ja: "プロジェクトのビルド フォルダー (target、node_modules)", pt: "Pastas de compilação de projetos (target, node_modules)", zh: "项目构建文件夹（target、node_modules）", es: "Carpetas de compilación de proyectos (target, node_modules)").into(), note: t!("Пересоберутся при следующей сборке или npm install.", "They are rebuilt on the next build or npm install.", ja: "次回のビルドまたは npm install で再作成されます。", pt: "São recriadas na próxima compilação ou npm install.", zh: "会在下次构建或 npm install 时重新生成。", es: "Se regeneran en la próxima compilación o con npm install.").into(), paths: proj, bytes: pb });
    items.push(Item { risk: Risk::Ask, how: How::Wipe, label: t!("Корзина", "Recycle Bin", ja: "ごみ箱", pt: "Lixeira", zh: "回收站", es: "Papelera de reciclaje").into(), note: t!("Файлы, которые вы уже удалили.", "Files you have already deleted.", ja: "すでに削除したファイルです。", pt: "Arquivos que você já excluiu.", zh: "你已经删除的文件。", es: "Archivos que ya eliminaste.").into(), paths: bins, bytes: 0 });
    let dl = old_downloads();
    let mut skip: HashSet<PathBuf> = dl.as_ref().map(|d| d.paths.iter().cloned().collect()).unwrap_or_default();
    items.extend(dl);
    say(t!("Поиск больших забытых файлов", "Looking for big forgotten files", ja: "忘れられた大きなファイルを探しています", pt: "Procurando arquivos grandes esquecidos", zh: "正在查找被遗忘的大文件", es: "Buscando archivos grandes olvidados").into());
    let old = big_old(ds, &skip);
    skip.extend(old.iter().flat_map(|i| i.paths.iter().cloned()));
    items.extend(old);
    items.extend(duplicates(ds, &skip, say));
    items.retain_mut(|i| {
        i.paths.retain(|p| p.exists());
        if i.bytes == 0 {
            i.bytes = i.paths.iter().map(|p| dir_size(p)).sum();
        }
        i.bytes >= 1 << 20 // crumbs (an "empty" recycle bin is 987 bytes) are noise
    });
    items.sort_by_key(|i| (i.risk as u8, Reverse(i.bytes)));
    items
}

/// What a clean did: bytes freed now, bytes moved to the quarantine, and the items that partly stayed (in use, no rights).
#[derive(Default, Debug)]
pub struct Done {
    pub freed: u64,
    pub held: u64,
    pub partly: Vec<(String, u64)>,
}

/// Cleans `items`. `say` hears each one as it starts.
pub fn apply(items: &[&Item], say: &mut dyn FnMut(String)) -> Done {
    for d in fixed_drives() {
        purge(d, false); // what waited out its 30 days goes first
    }
    let log = env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("rs-du")).unwrap_or_default();
    if log.is_absolute() && fs::create_dir_all(&log).is_ok() && let Ok(mut f) = fs::File::create(log.join("clean-log.txt")) {
        for i in items {
            for p in &i.paths {
                let _ = writeln!(f, "{}\t{}", i.bytes, p.display());
            }
        }
    }
    let (stamp, mut seq, mut done) = (now(), 0, Done::default());
    for i in items {
        say(i.label.clone());
        for p in &i.paths {
            match i.how {
                How::Wipe => wipe(p, FRESH),
                How::Remove => remove(p),
                How::Quarantine => {
                    seq += 1;
                    quarantine(p, stamp, seq);
                }
            }
        }
        let left: u64 = i.paths.iter().map(|p| dir_size(p)).sum();
        let gone = i.bytes.saturating_sub(left);
        if i.how == How::Quarantine {
            done.held += gone;
        } else {
            done.freed += gone;
        }
        if left >= 1 << 20 && i.how != How::Quarantine {
            done.partly.push((i.label.clone(), left));
        }
    }
    done
}

// ---------- quarantine ----------

fn qroot(drive: char) -> PathBuf {
    PathBuf::from(format!(r"{}:\.rs-du-quarantine", drive.to_ascii_uppercase()))
}

/// The list of quarantined items lives in a WinRust folder beside the one this exe is installed in (under Program Files:
/// admin-only), shared by rs-du and WinRust Disk. Not in the quarantine folder: under a drive root any user can create and
/// edit files, and a forged row would make this elevated process delete or move any path.
fn mpath(drive: char) -> PathBuf {
    let exe = env::current_exe().expect("exe path");
    let dir = exe.parent().and_then(Path::parent).map_or_else(PathBuf::new, |p| p.join("WinRust"));
    dir.join(format!("quarantine-{}.tsv", drive.to_ascii_uppercase()))
}

/// Where rs-du kept the list before (next to its exe): read once and moved into the shared one.
fn legacy_mpath(drive: char) -> PathBuf {
    env::current_exe().expect("exe path").with_file_name(format!("quarantine-{}.tsv", drive.to_ascii_uppercase()))
}

/// Closes the quarantine folder to everyone but administrators and SYSTEM (inherited from a drive root, users could edit it).
// shortcut: a folder a user created before the first clean stays owned by them; the trusted list above keeps that harmless.
fn lock(root: &Path) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| lock_now(root));
}

fn lock_now(root: &Path) {
    let _ = std::process::Command::new("icacls").arg(root).args(["/inheritance:r", "/grant:r", "*S-1-5-32-544:(OI)(CI)F", "*S-1-5-18:(OI)(CI)F", "/Q"]).output();
}

fn quarantine(p: &Path, stamp: u64, n: usize) -> bool {
    let drive = p.to_string_lossy().chars().next().unwrap_or('C');
    let root = qroot(drive);
    let dir = root.join(stamp.to_string());
    let to = dir.join(format!("{n}_{}", p.file_name().unwrap_or_default().to_string_lossy()));
    let made = fs::create_dir_all(&dir).is_ok();
    if made {
        lock(&root);
    }
    let m = mpath(drive);
    let ok = made && m.parent().is_some_and(|d| fs::create_dir_all(d).is_ok()) && fs::rename(p, &to).is_ok();
    if ok && let Ok(mut f) = fs::OpenOptions::new().append(true).create(true).open(m) {
        let _ = writeln!(f, "{stamp}\t{}\t{}", p.display(), to.display());
    }
    ok
}

type Row = (u64, PathBuf, PathBuf); // (when, original path, quarantined path)

/// Rows whose quarantined path is not inside this drive's quarantine folder are dropped: purge deletes that path.
fn parse(text: &str, drive: char) -> Vec<Row> {
    let root = qroot(drive);
    text.lines()
        .filter_map(|l| {
            let mut s = l.splitn(3, '\t');
            Some((s.next()?.parse().ok()?, s.next()?.into(), s.next()?.into()))
        })
        .filter(|r: &Row| r.2.starts_with(&root) && !r.2.components().any(|c| c == std::path::Component::ParentDir))
        .collect()
}

fn manifest(drive: char) -> (PathBuf, Vec<Row>) {
    let m = mpath(drive);
    let mut rows = parse(&fs::read_to_string(&m).unwrap_or_default(), drive);
    let old = legacy_mpath(drive);
    if old != m && let Ok(t) = fs::read_to_string(&old) {
        rows.extend(parse(&t, drive));
        if m.parent().is_some_and(|d| fs::create_dir_all(d).is_ok()) {
            save(&m, &rows);
            let _ = fs::remove_file(old);
        }
    }
    (m, rows)
}

fn save(m: &Path, rows: &[Row]) {
    let _ = fs::write(m, rows.iter().map(|(t, a, b)| format!("{t}\t{}\t{}\n", a.display(), b.display())).collect::<String>());
}

/// How many items wait in the quarantine of `drive`.
pub fn quarantined(drive: char) -> usize {
    manifest(drive).1.iter().filter(|r| r.2.exists()).count()
}

/// Deletes quarantined items older than 30 days (all of them with `all`): this is when that space really comes back.
/// Returns the bytes freed.
pub fn purge(drive: char, all: bool) -> u64 {
    let (m, rows) = manifest(drive);
    if rows.is_empty() {
        return 0;
    }
    let (mut keep, mut freed) = (vec![], 0);
    for r in rows {
        if all || now().saturating_sub(r.0) > QUAR_DAYS * DAY {
            let before = dir_size(&r.2);
            remove(&r.2);
            freed += before.saturating_sub(dir_size(&r.2));
            if !r.2.exists() {
                continue;
            }
        }
        keep.push(r);
    }
    save(&m, &keep);
    for d in subdirs(&qroot(drive), |_| true) {
        let _ = fs::remove_dir(d); // only empty ones go
    }
    freed
}

/// Puts quarantined items back where they were: (how many came back, the ones that could not and why).
pub fn restore(drive: char) -> (usize, Vec<(PathBuf, &'static str)>) {
    let (m, rows) = manifest(drive);
    let (mut keep, mut back, mut failed) = (vec![], 0, vec![]);
    for r in rows {
        if !r.2.exists() {
            continue;
        }
        if r.1.exists() {
            failed.push((r.1.clone(), t!("место занято", "the place is taken", ja: "場所が使われています", pt: "o local está ocupado", zh: "原位置已被占用", es: "el lugar está ocupado")));
            keep.push(r);
            continue;
        }
        if r.1.parent().is_some_and(|d| fs::create_dir_all(d).is_ok()) && fs::rename(&r.2, &r.1).is_ok() {
            back += 1;
        } else {
            failed.push((r.1.clone(), t!("не удалось переместить", "could not be moved", ja: "移動できませんでした", pt: "não foi possível mover", zh: "无法移动", es: "no se pudo mover")));
            keep.push(r);
        }
    }
    save(&m, &keep);
    (back, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = env::temp_dir().join(format!("winrust-clean-{name}"));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rule_paths_are_absolute() {
        // a missing env var must never turn into a relative path that wipes something in the current dir
        for i in rules() {
            for p in &i.paths {
                assert!(p.is_absolute(), "{}", p.display());
            }
        }
    }

    #[test]
    fn wipe_empties_a_dir_but_keeps_it_and_what_is_fresh() {
        let d = tmp("wipe");
        fs::create_dir_all(d.join("a/b")).unwrap();
        fs::write(d.join("a/b/f"), [0u8; 100]).unwrap();
        fs::write(d.join("g"), [0u8; 50]).unwrap();
        assert_eq!(dir_size(&d), 150);
        wipe(&d, FRESH); // all of it was just written: an installer at work, nothing goes
        assert_eq!(dir_size(&d), 150);
        wipe(&d, 0);
        assert!(d.is_dir() && fs::read_dir(&d).unwrap().count() == 0);
        fs::remove_dir(&d).unwrap();
    }

    #[test]
    fn copies_are_found_by_their_bytes_not_their_size() {
        let d = tmp("twins");
        let (a, b, c, e) = (d.join("a.bin"), d.join("b.bin"), d.join("c.bin"), d.join("e.bin"));
        let body: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&a, &body).unwrap();
        fs::write(&b, &body).unwrap();
        let mut other = body.clone();
        other[2_500_000] ^= 1; // same size, one byte off, past the first chunk
        fs::write(&c, &other).unwrap();
        fs::write(&e, &body[..10]).unwrap();
        let size = body.len() as u64;
        let found = twins(vec![(size, b.clone()), (size, a.clone()), (size, c.clone()), (10, e.clone())], &mut |_| {});
        assert_eq!(found.len(), 1);
        let (keep, copies) = &found[0];
        assert_eq!(copies.len(), 1); // c differs in one byte: not a copy
        assert!([&a, &b].contains(&keep) && [&a, &b].contains(&&copies[0]) && keep != &copies[0]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn system_programs_and_cloud_folders_are_left_alone() {
        for p in [r"C:\Windows\Installer\x.msi", r"D:\Program Files (x86)\Game\intro.mp4", r"C:\Users\a\AppData\Local\x.zip", r"D:\SteamLibrary\steamapps\common\g\v.mp4", r"C:\Users\a\OneDrive\video.mp4", r"E:\.rs-du-quarantine\1\1_x.iso"] {
            assert!(hands_off(p), "{p}");
        }
        for p in [r"C:\Users\a\Videos\trip.mp4", r"D:\Backups\disk.iso", r"E:\Movies\film.mkv"] {
            assert!(!hands_off(p), "{p}");
        }
    }

    #[test]
    fn files_next_to_programs_belong_to_them() {
        let d = tmp("programs");
        for dir in ["Games/It/Binaries/Win64", "Games/It/Content/Movies", "Tools/x", "Films"] {
            fs::create_dir_all(d.join(dir)).unwrap();
        }
        for f in ["Games/It/Binaries/Win64/It.exe", "Games/It/Binaries/Win64/It_Trial.exe", "Games/It/Content/Movies/intro.mp4", "Tools/app.dll", "Tools/x/data.bin", "Films/trip.mkv"] {
            fs::write(d.join(f), b"x").unwrap();
        }
        // whatever lies above the temp folder is not part of the test
        let mut seen: HashMap<PathBuf, bool> = d.ancestors().map(|a| (a.to_path_buf(), false)).collect();
        assert!(program_file(&d.join("Games/It/Binaries/Win64/It_Trial.exe"), &mut seen));
        assert!(program_file(&d.join("Games/It/Content/Movies/intro.mp4"), &mut seen)); // the exe is elsewhere in the game
        assert!(program_file(&d.join("Tools/x/data.bin"), &mut seen)); // a dll two folders up
        assert!(!program_file(&d.join("Films/trip.mkv"), &mut seen));
        assert!(!program_file(Path::new(r"C:\Users\a\Downloads\setup.exe"), &mut seen)); // the user's own folder: theirs
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn manifest_round_trip() {
        let d = tmp("manifest");
        let m = d.join("manifest.tsv");
        save(&m, &[(5, r"C:\a".into(), r"C:\.rs-du-quarantine\5\1_a".into())]);
        assert_eq!(parse(&fs::read_to_string(&m).unwrap(), 'c'), vec![(5, PathBuf::from(r"C:\a"), PathBuf::from(r"C:\.rs-du-quarantine\5\1_a"))]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn manifest_rows_outside_the_quarantine_are_dropped() {
        let t = [r"C:\Windows\System32", r"C:\.rs-du-quarantine\..\Windows", r"D:\.rs-du-quarantine\1\a"].map(|q| format!("0\tC:\\x\t{q}")).join("\n");
        assert!(parse(&t, 'C').is_empty());
    }
}
