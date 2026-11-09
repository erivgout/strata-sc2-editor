//! Windows: embed the app icon and version details in strata.exe, so
//! Explorer, the Start menu, the taskbar and "Open with" show Strata's icon
//! and name. Other targets need nothing.

fn main() {
    println!("cargo:rerun-if-changed=assets/icons/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icons/icon.ico")
        .set("ProductName", "Strata")
        .set("FileDescription", "Strata")
        .set("LegalCopyright", "MIT OR Apache-2.0")
        .set("OriginalFilename", "strata.exe");
    // A build without the Windows SDK's resource compiler still works; it
    // just has the default icon.
    if let Err(e) = res.compile() {
        println!("cargo:warning=strata.exe has no icon: the Windows resource compiler failed ({e})");
    }
}
