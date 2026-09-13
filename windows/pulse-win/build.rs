fn main() {
    // Ship the Windows App SDK runtime next to pulse.exe, so the WinUI 3
    // settings window works without a framework-package install.
    windows_reactor_setup::as_self_contained();

    // The application icon — the macOS AppIcon rendered into a Windows
    // .ico — plus the version strings Explorer and the taskbar read.
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/app.ico");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    res.set("FileDescription", "Pulse");
    res.set("ProductName", "Pulse");
    res.set("FileVersion", &version);
    res.set("ProductVersion", &version);
    if let Err(e) = res.compile() {
        // A missing icon resource should never fail the build; the exe
        // just falls back to the generic one.
        eprintln!("winresource: {e}");
    }
}
