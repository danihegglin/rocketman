// Embeds assets/icon/rocketman.ico in the Windows .exe files, as icon resource 1:
// Explorer, the taskbar and GPUI's window class all take the icon from there.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon/rocketman.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon/rocketman.ico");
        res.compile().expect("failed to embed the Windows icon");
    }
}
