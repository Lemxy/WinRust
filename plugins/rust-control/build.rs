fn main() {
    // The icon and version resource are the ones used by the other WinRust programs.
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app.ico");
    embed_resource::compile("app.rc", embed_resource::NONE).manifest_optional().unwrap();
}
