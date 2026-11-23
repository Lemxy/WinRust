//! The signed update manifest. A release carries `manifest.txt` and `manifest.sig` (hex ed25519 signature of the exact bytes
//! of manifest.txt, made with the owner's private key by `winrust-sign`). The installer holds only the public key, so a
//! swapped release asset fails the signature and nothing is installed. Also a test target (no crate:: imports here).
//!
//!   winrust-manifest 1
//!   version v0.5.4
//!   <sha256 hex> <file name>      one line per released exe
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

pub(crate) struct Manifest {
    pub version: String,
    pub files: Vec<(String, String)>, // (sha256 hex lower-case, file name)
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// `v0.5.4` -> [0, 5, 4]; compared lexicographically.
pub(crate) fn ver(s: &str) -> Option<Vec<u64>> {
    s.strip_prefix('v')?.split('.').map(|p| p.parse().ok()).collect()
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// Signature first, then the contents: nothing from an unsigned file is looked at.
pub(crate) fn verify(text: &[u8], sig_hex: &str, key_hex: &str) -> Result<Manifest, String> {
    let key: [u8; 32] = unhex(key_hex.trim()).and_then(|k| k.try_into().ok()).ok_or("bad public key")?;
    let key = VerifyingKey::from_bytes(&key).map_err(|_| "bad public key")?;
    let sig: [u8; 64] = unhex(sig_hex.trim()).and_then(|s| s.try_into().ok()).ok_or("bad signature format")?;
    key.verify(text, &Signature::from_bytes(&sig)).map_err(|_| "signature does not match: the release was not signed by the owner")?;
    parse(std::str::from_utf8(text).map_err(|_| "manifest is not UTF-8")?)
}

/// A name ends up in a path and a URL: plain file names only.
fn safe_name(n: &str) -> bool {
    n.to_ascii_lowercase().ends_with(".exe") && !n.contains("..") && !n.starts_with(' ') && n.chars().all(|c| c.is_ascii_alphanumeric() || " ._-".contains(c))
}

fn parse(text: &str) -> Result<Manifest, String> {
    let mut lines = text.lines().map(str::trim_end).filter(|l| !l.is_empty());
    if lines.next() != Some("winrust-manifest 1") {
        return Err("not a WinRust manifest".into());
    }
    let version = lines.next().and_then(|l| l.strip_prefix("version ")).filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || ".-".contains(c))).ok_or("manifest has no version")?;
    let mut files = vec![];
    for l in lines {
        let (sha, name) = l.split_once(' ').ok_or_else(|| format!("bad manifest line: {l}"))?;
        if sha.len() != 64 || unhex(sha).is_none() || !safe_name(name) {
            return Err(format!("bad manifest line: {l}"));
        }
        files.push((sha.to_ascii_lowercase(), name.to_string()));
    }
    Ok(Manifest { version: version.to_string(), files })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn signed(text: &str) -> (Vec<u8>, String, String) {
        let k = SigningKey::from_bytes(&[7; 32]);
        (text.as_bytes().to_vec(), hex(&k.sign(text.as_bytes()).to_bytes()), hex(k.verifying_key().as_bytes()))
    }

    const GOOD: &str = "winrust-manifest 1\nversion v0.5.4\n\
        e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 topbar.exe\n\
        e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 Task manager.exe\n";

    #[test]
    fn signed_manifest_is_read() {
        let (t, s, k) = signed(GOOD);
        let m = verify(&t, &s, &k).unwrap();
        assert_eq!(m.version, "v0.5.4");
        assert_eq!(m.files.len(), 2);
        assert_eq!(m.files[1].1, "Task manager.exe");
        assert_eq!(sha256_hex(b""), m.files[0].0);
    }

    #[test]
    fn changed_byte_or_other_key_is_refused() {
        let (mut t, s, k) = signed(GOOD);
        t[30] ^= 1;
        assert!(verify(&t, &s, &k).is_err());
        let (t, s, _) = signed(GOOD);
        let other = hex(SigningKey::from_bytes(&[8; 32]).verifying_key().as_bytes());
        assert!(verify(&t, &s, &other).is_err());
    }

    #[test]
    fn unsafe_names_are_refused_even_when_signed() {
        for bad in [r"..\x.exe", "a/b.exe", "evil.dll", r"C:\x.exe"] {
            let (t, s, k) = signed(&format!("winrust-manifest 1\nversion v1\n{} {bad}\n", "0".repeat(64)));
            assert!(verify(&t, &s, &k).is_err(), "{bad}");
        }
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(ver("v0.10.0") > ver("v0.9.9"));
        assert!(ver("v0.5.4") == ver("v0.5.4"));
        assert_eq!(ver("0.5.4"), None);
        assert_eq!(ver("v1.x"), None);
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(unhex(&hex(&[0, 255, 16])), Some(vec![0, 255, 16]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }
}
