//! Local user accounts and groups through the NetUser/NetLocalGroup family.

use std::ptr::null;

use windows_sys::Win32::{
    Foundation::*,
    NetworkManagement::NetManagement::*,
};

use crate::{
    model::{col, Action, Row, Target, Tone},
    sys,
};

fn pwstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe {
        let mut n = 0usize;
        while *p.add(n) != 0 && n < 4096 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }
}

fn user_rows() -> Result<Vec<Row>, u32> {
    unsafe {
        let mut buf: *mut u8 = std::ptr::null_mut();
        let (mut read, mut total, mut resume) = (0u32, 0u32, 0u32);
        let rc = NetUserEnum(null(), 1, FILTER_NORMAL_ACCOUNT, &mut buf, MAX_PREFERRED_LENGTH, &mut read, &mut total, &mut resume);
        if rc != NERR_Success {
            return Err(rc);
        }
        let mut out = vec![];
        if read > 0 && !buf.is_null() {
            let list = std::slice::from_raw_parts(buf as *const USER_INFO_1, read as usize);
            for u in list {
                let name = pwstr(u.usri1_name);
                if name.is_empty() {
                    continue;
                }
                let description = pwstr(u.usri1_comment);
                let mut flags = vec![];
                let mut tone = Tone::Neutral;
                if u.usri1_flags & UF_ACCOUNTDISABLE != 0 {
                    flags.push(t!("Отключена", "Disabled", ja: "無効", pt: "Desativada", zh: "已禁用").to_string());
                    tone = Tone::Bad;
                }
                if u.usri1_flags & UF_LOCKOUT != 0 {
                    flags.push(t!("Заблокирована", "Locked out", ja: "ロックアウト", pt: "Bloqueada", zh: "已锁定").to_string());
                    tone = Tone::Bad;
                }
                if u.usri1_flags & UF_PASSWD_CANT_CHANGE != 0 {
                    flags.push(t!("Пароль нельзя изменить", "Password cannot change", ja: "パスワード変更不可", pt: "A senha não pode ser alterada", zh: "密码不可更改").to_string());
                }
                if u.usri1_flags & UF_DONT_EXPIRE_PASSWD != 0 {
                    flags.push(t!("Пароль не истекает", "Password never expires", ja: "パスワード無期限", pt: "A senha nunca expira", zh: "密码永不过期").to_string());
                }
                if u.usri1_flags & UF_PASSWD_NOTREQD != 0 {
                    flags.push(t!("Пароль не требуется", "No password required", ja: "パスワード不要", pt: "Nenhuma senha necessária", zh: "不需要密码").to_string());
                    tone = Tone::Warn;
                }
                if u.usri1_flags & UF_SMARTCARD_REQUIRED != 0 {
                    flags.push(t!("Требуется смарт-карта", "Smart card required", ja: "スマート カードが必要", pt: "Cartão inteligente necessário", zh: "需要智能卡").to_string());
                }
                let flag_text = if flags.is_empty() { t!("Обычная", "Standard", ja: "標準", pt: "Padrão", zh: "标准").to_string() } else { flags.join(" · ") };
                out.push(Row::table(
                    vec![
                        col(name, 2, Tone::Neutral),
                        col(if description.is_empty() { "—".into() } else { description }, 4, Tone::Dim),
                        col(flag_text, 3, tone),
                    ],
                    Tone::Neutral,
                    None,
                    vec![],
                    0,
                ));
            }
        }
        NetApiBufferFree(buf as *const _);
        Ok(out)
    }
}

fn group_rows() -> Result<Vec<Row>, u32> {
    unsafe {
        let mut buf: *mut u8 = std::ptr::null_mut();
        let (mut read, mut total, mut resume) = (0u32, 0u32, 0usize);
        let rc = NetLocalGroupEnum(null(), 1, &mut buf, MAX_PREFERRED_LENGTH, &mut read, &mut total, &mut resume);
        if rc != NERR_Success {
            return Err(rc);
        }
        let mut out = vec![];
        if read > 0 && !buf.is_null() {
            let list = std::slice::from_raw_parts(buf as *const LOCALGROUP_INFO_1, read as usize);
            for g in list {
                let name = pwstr(g.lgrpi1_name);
                if name.is_empty() {
                    continue;
                }
                let comment = pwstr(g.lgrpi1_comment);
                out.push(Row::table(
                    vec![col(name, 2, Tone::Neutral), col(if comment.is_empty() { "—".into() } else { comment }, 5, Tone::Dim)],
                    Tone::Neutral,
                    None,
                    vec![],
                    0,
                ));
            }
        }
        NetApiBufferFree(buf as *const _);
        Ok(out)
    }
}

pub fn rows() -> Vec<Row> {
    let mut rows = vec![Row::entry(
        t!("Управление учётными записями", "Manage accounts", ja: "アカウントの管理", pt: "Gerenciar contas", zh: "管理账户"),
        "ms-settings:otherusers",
        "",
        t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开"),
        Tone::Accent,
        Some(Action::Open(Target::Uri("ms-settings:otherusers"))),
        vec![],
    )];
    match user_rows() {
        Ok(users) => {
            rows.push(Row::section(t!("Локальные пользователи", "Local users", ja: "ローカル ユーザー", pt: "Usuários locais", zh: "本地用户")));
            if users.is_empty() {
                rows.push(Row::note(t!("Пользователи не найдены.", "No users were found.", ja: "ユーザーが見つかりません。", pt: "Nenhum usuário foi encontrado.", zh: "未找到用户。"), Tone::Warn));
            } else {
                rows.push(Row::head(
                    vec![
                        col(t!("Имя", "Name", ja: "名前", pt: "Nome", zh: "名称"), 2, Tone::Dim),
                        col(t!("Описание", "Description", ja: "説明", pt: "Descrição", zh: "描述"), 4, Tone::Dim),
                        col(t!("Флаги", "Flags", ja: "フラグ", pt: "Sinalizadores", zh: "标志"), 3, Tone::Dim),
                    ],
                    0,
                ));
                rows.extend(users);
            }
        }
        Err(e) => rows.push(Row::note(
            tf!("Не удалось перечислить пользователей: {}", "Could not list the users: {}", ja: "ユーザーを列挙できませんでした: {}", pt: "Não foi possível listar os usuários: {}", zh: "无法列出用户：{}", sys::error_text(e)),
            Tone::Bad,
        )),
    }
    match group_rows() {
        Ok(groups) => {
            rows.push(Row::section(t!("Локальные группы", "Local groups", ja: "ローカル グループ", pt: "Grupos locais", zh: "本地组")));
            if groups.is_empty() {
                rows.push(Row::note(t!("Группы не найдены.", "No groups were found.", ja: "グループが見つかりません。", pt: "Nenhum grupo foi encontrado.", zh: "未找到组。"), Tone::Warn));
            } else {
                rows.push(Row::head(
                    vec![col(t!("Группа", "Group", ja: "グループ", pt: "Grupo", zh: "组"), 2, Tone::Dim), col(t!("Описание", "Description", ja: "説明", pt: "Descrição", zh: "描述"), 5, Tone::Dim)],
                    0,
                ));
                rows.extend(groups);
            }
        }
        Err(e) => rows.push(Row::note(
            tf!("Не удалось перечислить группы: {}", "Could not list the groups: {}", ja: "グループを列挙できませんでした: {}", pt: "Não foi possível listar os grupos: {}", zh: "无法列出组：{}", sys::error_text(e)),
            Tone::Bad,
        )),
    }
    rows
}
