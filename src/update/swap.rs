//! Replaces installed exes with a downloaded one, all targets or none. A running exe cannot be overwritten but can be
//! renamed: the old one becomes `<name>.old` (deleted by the next run). The new file is staged as `<name>.new` first and
//! moved in with one rename, so a power cut never leaves a half-written exe. Also a test target (std only).
use std::{fs, path::{Path, PathBuf}};

fn with_ext(p: &Path, ext: &str) -> PathBuf {
    PathBuf::from(format!("{}.{ext}", p.display()))
}

/// A free name for the old file: a previous `.old` may still belong to a running program.
fn aside(dst: &Path) -> PathBuf {
    let old = with_ext(dst, "old");
    let _ = fs::remove_file(&old);
    if !old.exists() {
        return old;
    }
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    with_ext(dst, &format!("old{t}"))
}

pub(crate) fn swap_all(new: &Path, targets: &[PathBuf]) -> Result<(), String> {
    let err = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    for t in targets {
        let stage = with_ext(t, "new");
        if let Err(e) = fs::copy(new, &stage) {
            targets.iter().for_each(|t| drop(fs::remove_file(with_ext(t, "new"))));
            return Err(err(t, e));
        }
    }
    let mut done: Vec<(&PathBuf, PathBuf)> = vec![];
    for t in targets {
        let old = aside(t);
        let moved = fs::rename(t, &old).map_err(|e| err(t, e)).and_then(|_| {
            fs::rename(with_ext(t, "new"), t).map_err(|e| {
                let _ = fs::rename(&old, t);
                err(t, e)
            })
        });
        match moved {
            Ok(()) => done.push((t, old)),
            Err(e) => {
                for (d, o) in done.iter().rev() {
                    let _ = fs::rename(o, d);
                }
                targets.iter().for_each(|t| drop(fs::remove_file(with_ext(t, "new"))));
                return Err(e);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("winrust-swap-{n}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn all_targets_get_the_new_file_and_keep_the_old_one_aside() {
        let d = dir("ok");
        let (a, b, n) = (d.join("a.exe"), d.join("b.exe"), d.join("n"));
        fs::write(&a, "old").unwrap();
        fs::write(&b, "old").unwrap();
        fs::write(&n, "new").unwrap();
        swap_all(&n, &[a.clone(), b.clone()]).unwrap();
        assert_eq!((fs::read_to_string(&a).unwrap(), fs::read_to_string(&b).unwrap()), ("new".into(), "new".into()));
        assert_eq!(fs::read_to_string(d.join("a.exe.old")).unwrap(), "old");
        assert!(!d.join("a.exe.new").exists());
    }

    #[test]
    fn a_failure_on_the_second_target_restores_the_first() {
        let d = dir("fail");
        let (a, b, n) = (d.join("a.exe"), d.join("b.exe"), d.join("n")); // b.exe does not exist: its rename fails
        fs::write(&a, "old").unwrap();
        fs::write(&n, "new").unwrap();
        assert!(swap_all(&n, &[a.clone(), b]).is_err());
        assert_eq!(fs::read_to_string(&a).unwrap(), "old");
        assert!(!d.join("a.exe.new").exists() && !d.join("b.exe.new").exists());
    }

    #[test]
    fn a_failed_copy_touches_nothing() {
        let d = dir("copy");
        let (a, n) = (d.join("a.exe"), d.join("n"));
        fs::write(&a, "old").unwrap();
        fs::write(&n, "new").unwrap();
        assert!(swap_all(&n, &[a.clone(), d.join("no-such-dir").join("b.exe")]).is_err());
        assert_eq!(fs::read_to_string(&a).unwrap(), "old");
        assert!(!d.join("a.exe.new").exists());
    }
}
