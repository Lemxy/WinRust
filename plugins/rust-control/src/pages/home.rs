//! The classic Category view of the Control Panel, and the flat "Icons" alternative.

use crate::model::{Action, Applet, Category, HomeLink, PageId, PageMeta, Target};
use winrust_common::style::rgb;

/// The eight categories, in the order of the classic Windows view: four on the left, four on the right.
pub fn categories() -> Vec<Category> {
    let link = |label: &str, action: Action| HomeLink { label: label.into(), action };
    vec![
        Category {
            title: t!("Система и безопасность", "System and Security", ja: "システムとセキュリティ", pt: "Sistema e segurança", zh: "系统和安全").into(),
            icon: "\u{E72E}",
            tile: rgb(10, 132, 255),
            links: vec![
                link(t!("Проверить состояние компьютера", "Review your computer's status", ja: "コンピューターの状態を確認", pt: "Verificar o status do computador", zh: "查看计算机状态"), Action::Go(PageId::Security)),
                link(t!("Сохранить резервные копии файлов с помощью истории файлов", "Save backup copies of your files with File History", ja: "ファイル履歴でファイルのバックアップ コピーを保存", pt: "Salvar cópias de backup dos arquivos com o Histórico de Arquivos", zh: "使用文件历史记录保存文件备份副本"), Action::Open(Target::Control("Microsoft.FileHistory"))),
                link(t!("Резервное копирование и восстановление (Windows 7)", "Backup and Restore (Windows 7)", ja: "バックアップと復元 (Windows 7)", pt: "Backup e Restauração (Windows 7)", zh: "备份和还原（Windows 7）"), Action::Open(Target::Control("Microsoft.BackupAndRestore"))),
            ],
        },
        Category {
            title: t!("Сети и Интернет", "Network and Internet", ja: "ネットワークとインターネット", pt: "Rede e Internet", zh: "网络和 Internet").into(),
            icon: "\u{E774}",
            tile: rgb(0, 122, 255),
            links: vec![link(
                t!("Просмотреть состояние и задачи сети", "View network status and tasks", ja: "ネットワークの状態とタスクの表示", pt: "Exibir status e tarefas da rede", zh: "查看网络状态和任务"),
                Action::Go(PageId::Network),
            )],
        },
        Category {
            title: t!("Оборудование и звук", "Hardware and Sound", ja: "ハードウェアとサウンド", pt: "Hardware e som", zh: "硬件和声音").into(),
            icon: "\u{E767}",
            tile: rgb(255, 159, 10),
            links: vec![
                link(t!("Просмотреть устройства и принтеры", "View devices and printers", ja: "デバイスとプリンターの表示", pt: "Exibir dispositivos e impressoras", zh: "查看设备和打印机"), Action::Go(PageId::Devices)),
                link(t!("Добавить устройство", "Add a device", ja: "デバイスの追加", pt: "Adicionar um dispositivo", zh: "添加设备"), Action::Open(Target::File("devicepairingwizard.exe"))),
            ],
        },
        Category {
            title: t!("Программы", "Programs", ja: "プログラム", pt: "Programas", zh: "程序").into(),
            icon: "\u{E71D}",
            tile: rgb(255, 159, 10),
            links: vec![link(
                t!("Удалить программу", "Uninstall a program", ja: "プログラムのアンインストール", pt: "Desinstalar um programa", zh: "卸载程序"),
                Action::Go(PageId::Programs),
            )],
        },
        Category {
            title: t!("Учётные записи пользователей", "User Accounts", ja: "ユーザー アカウント", pt: "Contas de usuário", zh: "用户账户").into(),
            icon: "\u{E77B}",
            tile: rgb(52, 199, 89),
            links: vec![link(
                t!("Изменить тип учётной записи", "Change account type", ja: "アカウントの種類の変更", pt: "Alterar o tipo de conta", zh: "更改帐户类型"),
                Action::Go(PageId::Users),
            )],
        },
        Category {
            title: t!("Оформление и персонализация", "Appearance and Personalization", ja: "外観と個人用設定", pt: "Aparência e personalização", zh: "外观和个性化").into(),
            icon: "\u{E7F4}",
            tile: rgb(10, 132, 255),
            links: vec![
                link(t!("Изменить тему", "Change the theme", ja: "テーマの変更", pt: "Alterar o tema", zh: "更改主题"), Action::Open(Target::Uri("ms-settings:themes"))),
                link(t!("Изменить фон", "Change the background", ja: "背景の変更", pt: "Alterar o plano de fundo", zh: "更改背景"), Action::Open(Target::Uri("ms-settings:personalization-background"))),
            ],
        },
        Category {
            title: t!("Часы и регион", "Clock and Region", ja: "時計と地域", pt: "Relógio e região", zh: "时钟和区域").into(),
            icon: "\u{E823}",
            tile: rgb(94, 92, 230),
            links: vec![link(
                t!("Изменить форматы даты, времени или чисел", "Change date, time, or number formats", ja: "日付、時刻、数値の形式の変更", pt: "Alterar formatos de data, hora ou número", zh: "更改日期、时间或数字格式"),
                Action::Go(PageId::DateTime),
            )],
        },
        Category {
            title: t!("Специальные возможности", "Accessibility", ja: "ユーザー補助", pt: "Acessibilidade", zh: "辅助功能").into(),
            icon: "\u{E776}",
            tile: rgb(50, 173, 230),
            links: vec![
                link(t!("Разрешить Windows предлагать параметры", "Let Windows suggest settings", ja: "Windows が設定を提案できるようにする", pt: "Permitir que o Windows sugira configurações", zh: "允许 Windows 建议设置"), Action::Open(Target::Uri("ms-settings:easeofaccess"))),
                link(t!("Оптимизировать отображение", "Optimize visual display", ja: "視覚表示を最適化", pt: "Otimizar a exibição visual", zh: "优化视觉显示"), Action::Go(PageId::Accessibility)),
            ],
        },
    ]
}

/// The flat list of applets shown by "View by: Icons".
pub fn applets(meta: &[PageMeta]) -> Vec<Applet> {
    meta.iter()
        .filter(|m| m.id != PageId::Home)
        .map(|m| Applet { title: m.title.clone(), icon: m.icon, tile: m.tile, action: Action::Go(m.id), keywords: format!("{} {}", m.title, m.keywords) })
        .collect()
}
