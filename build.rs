// Embeds the app icon (and version info) into the Windows .exe.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Rainbow");
        res.set("FileDescription", "Rainbow - Chorus Encore chart downloader");
        res.compile().expect("failed to embed Windows resources");
    }
}
