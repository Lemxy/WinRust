//! Signs WinRust release manifests for the auto-updater (src/update).
//!   winrust-sign keygen <public key out>   new key pair; the private key goes to %USERPROFILE%\.winrust\update-signing.key
//!   winrust-sign sign <file> <signature out>
//! The private key never goes into the repository. Lose it and installed copies can no longer be updated automatically
//! (they would need a new installer with a new public key): keep a backup somewhere offline.
use ed25519_dalek::{Signer, SigningKey};
use std::{env, fs, path::PathBuf, process::exit};

fn key_path() -> PathBuf {
    env::var_os("WINRUST_SIGNING_KEY").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env::var_os("USERPROFILE").unwrap_or_default()).join(r".winrust\update-signing.key"))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn fail(m: &str) -> ! {
    eprintln!("winrust-sign: {m}");
    exit(1)
}

fn load() -> SigningKey {
    let p = key_path();
    let s = fs::read_to_string(&p).unwrap_or_else(|e| fail(&format!("{}: {e}", p.display())));
    let b: Vec<u8> = (0..64).step_by(2).filter_map(|i| s.trim().get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect();
    SigningKey::from_bytes(&b.try_into().unwrap_or_else(|_| fail("the key file is damaged")))
}

fn main() {
    let a: Vec<String> = env::args().skip(1).collect();
    match a.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["keygen", out] => {
            let p = key_path();
            if p.exists() {
                fail(&format!("{} already exists: refusing to replace a key that installed copies trust", p.display()));
            }
            let mut seed = [0u8; 32];
            getrandom::getrandom(&mut seed).unwrap_or_else(|e| fail(&e.to_string()));
            let k = SigningKey::from_bytes(&seed);
            fs::create_dir_all(p.parent().unwrap()).unwrap_or_else(|e| fail(&e.to_string()));
            fs::write(&p, hex(&seed)).unwrap_or_else(|e| fail(&e.to_string()));
            fs::write(out, hex(k.verifying_key().as_bytes()) + "\n").unwrap_or_else(|e| fail(&e.to_string()));
            println!("private key: {} (back it up offline)\npublic key: {out}", p.display());
        }
        ["sign", file, out] => {
            let data = fs::read(file).unwrap_or_else(|e| fail(&format!("{file}: {e}")));
            fs::write(out, hex(&load().sign(&data).to_bytes()) + "\n").unwrap_or_else(|e| fail(&e.to_string()));
        }
        _ => fail("usage: winrust-sign keygen <public key out> | sign <file> <signature out>"),
    }
}
