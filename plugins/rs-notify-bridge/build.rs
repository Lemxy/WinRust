fn main() {
    // The manifest carries the <msix> identity block the sparse package in tools\notify-bridge matches, so it is required.
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app.ico");
    println!("cargo:rerun-if-changed=app.manifest");
    embed_resource::compile("app.rc", embed_resource::NONE).manifest_required().unwrap();
}
