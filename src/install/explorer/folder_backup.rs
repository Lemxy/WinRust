//! Check that a registry export belongs to one folder verb before replaying it.
const HEADER: &str = "Windows Registry Editor Version 5.00";

fn retarget(data: &[u8], user: &str, verb: &str) -> Result<Vec<u8>, String> {
    let bytes = data.strip_prefix(&[0xff, 0xfe]).ok_or("expected a UTF-16 registry export")?;
    let (pairs, extra) = bytes.as_chunks::<2>();
    if !extra.is_empty() {
        return Err("truncated registry export".into());
    }
    let words: Vec<_> = pairs.iter().copied().map(u16::from_le_bytes).collect();
    let text = String::from_utf16(&words).map_err(|e| e.to_string())?;

    let old_root = format!(r"HKEY_CURRENT_USER\{verb}");
    let root = format!(r"HKEY_USERS\{user}\{verb}");
    let mut lines = text.lines();
    if lines.next() != Some(HEADER) {
        return Err("invalid registry export header".into());
    }
    let mut output = format!("{HEADER}\r\n");
    let mut found_root = false;
    for line in lines {
        if let Some(section) = line.trim_start().strip_prefix('[') {
            let section = section.trim_end().strip_suffix(']').ok_or("invalid registry key section")?;
            let suffix = [&root, &old_root].iter().find_map(|prefix| {
                let head = section.get(..prefix.len())?;
                let tail = &section[prefix.len()..];
                (head.eq_ignore_ascii_case(prefix) && (tail.is_empty() || tail.starts_with('\\'))).then_some(tail)
            }).ok_or("registry export targets a different account or folder verb")?;
            found_root |= suffix.is_empty();
            output.push_str(&format!("[{root}{suffix}]"));
        } else {
            output.push_str(line);
        }
        output.push_str("\r\n");
    }
    if !found_root {
        return Err("registry export is missing its root key".into());
    }
    Ok([0xff, 0xfe].into_iter().chain(output.encode_utf16().flat_map(u16::to_le_bytes)).collect())
}

pub(crate) fn uses_executable(command: &str, executable: &str) -> bool {
    let command = command.trim_start_matches([' ', '\t']);
    let path = if let Some(quoted) = command.strip_prefix('"') {
        let Some((path, rest)) = quoted.split_once('"') else { return false };
        if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
            return false;
        }
        path
    } else {
        command.split([' ', '\t']).next().unwrap_or_default()
    };
    path.eq_ignore_ascii_case(executable)
}

pub(crate) fn restore_export<P>(
    data: &[u8],
    user: &str,
    verb: &str,
    stage: impl FnOnce(Vec<u8>) -> Result<P, String>,
    clear: impl FnOnce() -> Result<(), String>,
    import: impl FnOnce(P) -> Result<(), String>,
) -> Result<(), String> {
    // Validate and stage the replacement before removing the current binding.
    let staged = stage(retarget(data, user, verb)?)?;
    clear()?;
    import(staged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const SID: &str = "S-1-5-21-123";
    const VERB: &str = r"Software\Classes\Directory\shell\open";

    fn export(sections: &str) -> Vec<u8> {
        [0xff, 0xfe].into_iter()
            .chain(format!("{HEADER}\r\n\r\n{sections}\r\n").encode_utf16().flat_map(u16::to_le_bytes))
            .collect()
    }

    #[test]
    fn matches_the_executable_even_with_argument_spacing() {
        let exe = r"C:\Program Files\rust-explorer\rust-explorer.exe";
        assert!(uses_executable(&format!(" \t\"{exe}\"  \t\"%1\""), exe));
        assert!(uses_executable(&format!("\"{}\" --open \"%1\"", exe.to_uppercase()), exe));
        assert!(!uses_executable(&format!("\"{exe}.old\" \"%1\""), exe));
        assert!(!uses_executable(&format!("other.exe \"{exe}\""), exe));
        assert!(!uses_executable(&format!("\"{exe}\".old \"%1\""), exe));
        assert!(uses_executable(r"C:\WinRust\rust-explorer.exe  %1", r"C:\WinRust\rust-explorer.exe"));
    }

    #[test]
    fn redirects_a_legacy_hkcu_backup_to_the_signed_in_user() {
        let source = export(&format!("[HKEY_CURRENT_USER\\{VERB}]\r\n\r\n[HKEY_CURRENT_USER\\{VERB}\\command]\r\n@=\"custom\""));
        let expected = export(&format!("[HKEY_USERS\\{SID}\\{VERB}]\r\n\r\n[HKEY_USERS\\{SID}\\{VERB}\\command]\r\n@=\"custom\""));
        assert_eq!(retarget(&source, SID, VERB).unwrap(), expected);
        assert_eq!(retarget(&expected, SID, VERB).unwrap(), expected);
    }

    #[test]
    fn rejects_exports_outside_the_requested_verb() {
        for key in [format!(r"HKEY_USERS\S-1-5-21-456\{VERB}"),
            format!(r"HKEY_CURRENT_USER\{VERB}Extra"),
            format!(r"-HKEY_CURRENT_USER\{VERB}"),
            format!(r"HKEY_CURRENT_USER\{VERB}\command")]
        {
            let source = export(&format!("[{key}]"));
            assert!(retarget(&source, SID, VERB).is_err(), "{key}");
        }
    }

    #[test]
    fn a_corrupt_or_unwritable_backup_leaves_the_current_key_in_place() {
        let result = restore_export::<()>(&[0xff, 0xfe, 1], SID, VERB,
            |_| panic!("staged invalid backup"), || panic!("cleared key"), |_| panic!("imported backup"));
        assert!(result.is_err());
        let source = export(&format!("[HKEY_CURRENT_USER\\{VERB}]"));
        let result = restore_export::<()>(&source, SID, VERB,
            |_| Err("disk full".into()), || panic!("cleared key"), |_| panic!("imported backup"));
        assert_eq!(result, Err("disk full".into()));
    }

    #[test]
    fn failed_delete_skips_import_and_failed_import_can_be_retried() {
        let source = export(&format!("[HKEY_CURRENT_USER\\{VERB}]"));
        let error = restore_export(&source, SID, VERB, Ok,
            || Err("cannot remove current key".into()), |_| panic!("imported backup"));
        assert_eq!(error, Err("cannot remove current key".into()));
        let actions = RefCell::new(Vec::new());
        for result in [Err("cannot import backup".into()), Ok(())] {
            assert_eq!(restore_export(&source, SID, VERB,
                |data| { actions.borrow_mut().push("stage"); Ok(data) },
                || { actions.borrow_mut().push("clear"); Ok(()) },
                |_| { actions.borrow_mut().push("import"); result.clone() }), result);
        }
        assert_eq!(actions.into_inner(), ["stage", "clear", "import", "stage", "clear", "import"]);
    }
}
