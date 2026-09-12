//! Pulse for Windows — entry point.
//!
//! `pulse --json` prints the cached rail and exits; it never fetches and
//! never writes. Everything else starts the app.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod autostart;
mod berth;
mod card;
mod clipboard;
mod d2d;
mod geometry;
mod panel;
mod rings;
mod settings_ui;
mod theme;
mod tray;
mod winutil;

use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--json") {
        attach_console();
        pulse_core::settings::initialize();
        pulse_core::report::print();
        return;
    }

    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Pulse — a screen-edge monitor for your AI coding allowances.");
        println!("  --json   print the last readings for status lines and scripts");
        return;
    }

    if !winutil::acquire_single_instance() {
        return;
    }

    winutil::set_dpi_awareness();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    pulse_core::localization::detect_from_system();
    pulse_core::settings::initialize();
    pulse_core::secrets::install_dpapi(dpapi_protect, dpapi_unprotect);

    // The renderer is built before any window exists.
    let _ = d2d::global_engine();

    let mut app = app::App::start();
    app.run();
}

/// `--json` runs under the `windows` subsystem, so stdout starts detached.
/// Attaching to the parent's console is what makes a status line's `exec`
/// see the report.
fn attach_console() {
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn dpapi_protect(data: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let mut input = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(data.len()).ok()?,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        CryptProtectData(&mut input, None, None, None, None, 0, &mut output).ok()?;
        let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let out = slice.to_vec();
        drop_local(output.pbData);
        Some(out)
    }
}

fn dpapi_unprotect(data: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let mut input = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(data.len()).ok()?,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(&mut input, None, None, None, None, 0, &mut output).ok()?;
        let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let out = slice.to_vec();
        drop_local(output.pbData);
        Some(out)
    }
}

unsafe fn drop_local(ptr: *mut u8) {
    let _ = windows::Win32::Foundation::LocalFree(Some(
        windows::Win32::Foundation::HLOCAL(ptr as *mut core::ffi::c_void),
    ));
}
