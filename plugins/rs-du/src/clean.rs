// rs-du --clean: the shared cleaner (winrust_common::clean) in the console. It lists what can go by risk, then asks or
// applies; WinRust Disk shows the same list in a window.
use super::*;
use winrust_common::clean::{self as engine, Digest, Item, Risk};
use windows_sys::Win32::System::Console::{ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, STD_OUTPUT_HANDLE, SetConsoleMode};

fn vt() -> bool {
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut m = 0;
        GetConsoleMode(h, &mut m) != 0 && SetConsoleMode(h, m | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

/// Enter = defaults, "a" = everything, "n" = nothing, numbers (1-based, any separator) = defaults plus those.
fn select(s: &str, sel: &mut [bool]) {
    match s.trim().to_lowercase().as_str() {
        "n" | "q" => sel.fill(false),
        "a" => sel.fill(true),
        t => for n in t.split(|c: char| !c.is_ascii_digit()).filter_map(|x| x.parse::<usize>().ok()) {
            if let Some(x) = sel.get_mut(n.wrapping_sub(1)) { *x = true; }
        },
    }
}

/// Puts quarantined items back where they were.
pub fn restore(drives: &[char]) {
    for &d in drives {
        let (back, failed) = engine::restore(d);
        if back > 0 { println!("{}", tf!("{d}: восстановлено {}", "{d}: {} restored", ja: "{d}: {} 個を復元しました", pt: "{d}: {} restaurados", zh: "{d}: 已恢复 {} 项", es: "{d}: {} restaurados", back)); }
        for (p, why) in failed { println!("{}", tf!("не восстановлено ({}): {}", "not restored ({}): {}", ja: "復元できません ({}): {}", pt: "não restaurado ({}): {}", zh: "未恢复（{}）：{}", es: "no restaurado ({}): {}", why, p.display())); }
    }
}

/// Empties the quarantine now (it also empties itself after 30 days).
pub fn purge(drives: &[char]) {
    let freed: u64 = drives.iter().map(|&d| engine::purge(d, true)).sum();
    if freed > 0 { println!("{}", tf!("карантин очищен: освобождено {}", "quarantine emptied: {} freed", ja: "隔離を空にしました: {} を解放", pt: "quarentena esvaziada: {} liberados", zh: "隔离区已清空：释放了 {}", es: "cuarentena vaciada: {} liberados", human(freed))); }
}

pub fn clean(drives: &[Digest], apply: bool, ask: bool, all: bool, safe_only: bool) {
    let items = engine::plan(drives, &mut |s| eprintln!("{s}"));
    let c = vt();
    let (rst, col) = (if c { "\x1b[0m" } else { "" }, |r: Risk| if !c { "" } else { match r { Risk::Safe => "\x1b[32m", Risk::Rebuild => "\x1b[33m", Risk::Ask => "\x1b[31m" } });
    let mut last = None;
    for (n, i) in items.iter().enumerate() {
        if last != Some(i.risk) {
            last = Some(i.risk);
            println!("\n{}{}{rst}", col(i.risk), engine::risk_title(i.risk));
        }
        let fate = if i.fate().is_empty() { String::new() } else { format!(" {}{}", i.fate(), if i.how == engine::How::Quarantine { " (rs-du --restore)" } else { "" }) };
        println!("{:>3}. {:>9}  {}\n{:>16}{}{fate}", n + 1, human(i.bytes), i.label, "", i.note);
    }
    let mut sel: Vec<bool> = items.iter().map(|i| all || if safe_only { i.risk == Risk::Safe } else { i.default_pick() }).collect();
    let sum = |sel: &[bool]| -> u64 { items.iter().zip(sel).filter(|(_, s)| **s).map(|(i, _)| i.bytes).sum() };
    println!("{}", tf!("\nМожно освободить: {} безопасно, {} всего ({} мест)", "\nYou can free: {} safely, {} in total ({} places)", ja: "\n解放できる容量: 安全なもの {}、合計 {} ({} 件)", pt: "\nVocê pode liberar: {} com segurança, {} no total ({} locais)", zh: "\n可以安全释放：{}，总计 {}（{} 处）", es: "\nPuedes liberar: {} de forma segura, {} en total ({} lugares)", human(sum(&sel)), human(items.iter().map(|i| i.bytes).sum()), items.len()));
    let line = |p: &str| { print!("{p}"); let _ = std::io::stdout().flush(); let mut l = String::new(); let _ = std::io::stdin().read_line(&mut l); l };
    if !apply && !ask { println!("{}", t!("(только просмотр; --apply удалит безопасное, --apply --all всё, --apply --safe только зелёное)", "(preview only; --apply deletes the safe items, --apply --all everything, --apply --safe only the green ones)", ja: "(プレビューのみ。--apply で安全な項目を削除、--apply --all ですべて、--apply --safe で緑の項目のみ)", pt: "(apenas visualização; --apply exclui os itens seguros, --apply --all tudo, --apply --safe só os verdes)", zh: "（仅预览；--apply 删除安全项，--apply --all 删除全部，--apply --safe 只删除绿色项）", es: "(solo vista previa; --apply elimina lo seguro, --apply --all todo, --apply --safe solo lo verde)")); return; }
    if !apply {
        select(&line(t!("Enter = очистить безопасное (зелёное и жёлтое), a = всё, 12,13 = безопасное + эти номера, n = отмена: ", "Enter = clean the safe items (green and yellow), a = everything, 12,13 = safe + these numbers, n = cancel:", ja: "Enter = 安全な項目 (緑と黄) を削除、a = すべて、12,13 = 安全な項目 + 指定番号、n = キャンセル:", pt: "Enter = limpar os itens seguros (verdes e amarelos), a = tudo, 12,13 = seguros + esses números, n = cancelar:", zh: "Enter = 清理安全项（绿色和黄色），a = 全部，12,13 = 安全项 + 这些编号，n = 取消：", es: "Enter = limpiar lo seguro (verde y amarillo), a = todo, 12,13 = lo seguro + esos números, n = cancelar:")), &mut sel);
    }
    if !sel.contains(&true) { if ask { line(t!("Ничего не удалено. Нажмите Enter, чтобы закрыть.", "Nothing was deleted. Press Enter to close.", ja: "何も削除されませんでした。Enter キーを押すと閉じます。", pt: "Nada foi excluído. Pressione Enter para fechar.", zh: "未删除任何内容。按 Enter 关闭。", es: "No se eliminó nada. Pulsa Enter para cerrar.")); } return; }

    let picked: Vec<&Item> = items.iter().zip(&sel).filter(|(_, s)| **s).map(|(i, _)| i).collect();
    let done = engine::apply(&picked, &mut |_| {});
    for (label, left) in &done.partly { println!("{}", tf!("частично осталось {} (занято программой или нет прав): {}", "partially left {} (in use by a program or no rights): {}", ja: "{} が残りました (プログラムが使用中、または権限がありません): {}", pt: "{} restaram parcialmente (em uso por um programa ou sem permissão): {}", zh: "部分残留 {}（被程序占用或没有权限）：{}", es: "quedó en parte {} (en uso por un programa o sin permisos): {}", human(*left), label)); }
    const QUAR_DAYS: u64 = engine::QUAR_DAYS;
    println!("{}", tf!("\nОсвобождено: {}. В карантине: {} (место вернётся через {QUAR_DAYS} дней или после rs-du --purge).", "\nFreed: {}. In quarantine: {} (the space returns in {QUAR_DAYS} days or after rs-du --purge).", ja: "\n解放: {}。隔離中: {} ({QUAR_DAYS} 日後、または rs-du --purge の後に容量が戻ります)。", pt: "\nLiberado: {}. Em quarentena: {} (o espaço volta em {QUAR_DAYS} dias ou após rs-du --purge).", zh: "已释放：{}。隔离区中：{}（这部分空间将在 {QUAR_DAYS} 天后或执行 rs-du --purge 后收回）。", es: "\nLiberado: {}. En cuarentena: {} (el espacio vuelve en {QUAR_DAYS} días o tras rs-du --purge).", human(done.freed), human(done.held)));
    if let Some(l) = env::var_os("LOCALAPPDATA") { println!("{}", tf!("Список: {}", "List: {}", ja: "一覧: {}", pt: "Lista: {}", zh: "列表：{}", es: "Lista: {}", PathBuf::from(l).join(r"rs-du\clean-log.txt").display())); }
    if ask { line(t!("Нажмите Enter, чтобы закрыть.", "Press Enter to close.", ja: "Enter キーを押すと閉じます。", pt: "Pressione Enter para fechar.", zh: "按 Enter 关闭。", es: "Pulsa Enter para cerrar.")); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_defaults_all_none_and_numbers() {
        let mut s = [true, true, false, false];
        select("", &mut s); assert_eq!(s, [true, true, false, false]);
        select("4", &mut s); assert_eq!(s, [true, true, false, true]);
        select("3, 99,0", &mut s); assert_eq!(s, [true, true, true, true]);
        select("n", &mut s); assert_eq!(s, [false; 4]);
        select("a", &mut s); assert_eq!(s, [true; 4]);
    }
}
