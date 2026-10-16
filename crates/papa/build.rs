//! Build script for the desktop app.
//!
//! On Windows it embeds the app icon in `papa.exe` as resource 1, which
//! Explorer and the taskbar show and GPUI loads for its windows, along with
//! the name Windows shows in Task Manager and the file's Properties.

fn main() {
    println!("cargo::rerun-if-changed=icons/icon.ico");
    // The script runs on the machine doing the build, so ask Cargo which OS
    // the app is for rather than using `cfg(windows)`.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("icons/icon.ico")
            .set("ProductName", "Papa")
            .set("FileDescription", "Papa");
        resource
            .compile()
            .expect("couldn't embed the icon and version info in papa.exe");
    }
}
