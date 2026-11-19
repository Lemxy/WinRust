fn main() {
    // Common Controls 6 + per-monitor DPI awareness, as the rest of WinRust's windows
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", dir.join("app.manifest").display());
}
