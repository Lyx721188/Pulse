fn main() {
    // Ship the Windows App SDK runtime next to pulse.exe, so the WinUI 3
    // settings window works without a framework-package install.
    windows_reactor_setup::as_self_contained();
}
