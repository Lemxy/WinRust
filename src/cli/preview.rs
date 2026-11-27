//! Read-only install preview arguments and JSON output.
use std::fmt::Write;

/// None means this is an ordinary command. Either preview flag claims the command,
/// so an invalid preview cannot fall through to an install or a settings write.
pub(crate) fn install_args(args: &[String]) -> Option<Result<Vec<&str>, ()>> {
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    let dry_run = has("--dry-run");
    let json = has("--json");
    if !dry_run && !json {
        return None;
    }

    let [command, rest @ ..] = args else { return Some(Err(())) };
    let names: Vec<_> = rest.iter().map(String::as_str)
        .filter(|arg| !matches!(*arg, "--dry-run" | "--json"))
        .collect();
    Some(if command == "install" && dry_run && json && !names.is_empty()
        && names.iter().all(|name| !name.starts_with('-'))
    {
        Ok(names)
    } else {
        Err(())
    })
}

pub(crate) fn json<'a>(components: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut out = String::from("{\"components\":[");
    for (index, (name, warning)) in components.into_iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write!(out, "{{\"order\":{},\"name\":", index + 1).unwrap();
        json_string(&mut out, name);
        out.push_str(",\"warning\":");
        json_string(&mut out, warning);
        out.push('}');
    }
    out.push_str("]}");
    out
}

fn json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0}'..='\u{1f}' => write!(out, "\\u{:04x}", u32::from(ch)).unwrap(),
            _ => out.push(ch),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_flags_never_fall_through_to_normal_commands() {
        let args = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
        assert!(install_args(&args(&["install", "all", "--yes"])).is_none());
        assert_eq!(
            install_args(&args(&["install", "rust-explorer", "--dry-run", "topbar", "--json"])),
            Some(Ok(vec!["rust-explorer", "topbar"])),
        );
        for words in [
            &["install", "all", "--json", "--yes"][..],
            &["install", "all", "--dry-run", "--yes"],
            &["install", "all", "--dry-run", "--json", "--yes"],
            &["install", "all", "--dry-run", "--json", "--lang", "ja"],
            &["install", "--dry-run", "--json"],
            &["rollback", "--dry-run", "--json", "--yes"],
        ] {
            assert_eq!(install_args(&args(words)), Some(Err(())));
        }
    }

    #[test]
    fn json_keeps_component_order_and_escapes_warnings() {
        assert_eq!(
            json([("topbar", "Hide the taskbar."), ("rust-explorer", "\"変更\"\tC:\\Temp\n\u{000b}")]),
            r#"{"components":[{"order":1,"name":"topbar","warning":"Hide the taskbar."},{"order":2,"name":"rust-explorer","warning":"\"変更\"\u0009C:\\Temp\u000a\u000b"}]}"#,
        );
    }
}
