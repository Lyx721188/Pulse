//! Small Win32 conveniences shared by every window in the app.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITORINFO, MONITORINFOEXW,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};

pub const WM_APP_TRAY: u32 = WM_APP + 1;
pub const WM_APP_STORE: u32 = WM_APP + 2;

/// The instance handle, resolved once.
pub fn hinstance() -> windows::Win32::Foundation::HINSTANCE {
    unsafe { HINSTANCE(GetModuleHandleW(None).expect("GetModuleHandleW").0) }
}

/// Opts the whole process into per-monitor DPI awareness. A screen-edge
/// panel that renders itself must do its own scaling.
pub fn set_dpi_awareness() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn dpi_for(hwnd: HWND) -> f64 {
    unsafe { GetDpiForWindow(hwnd) as f64 / 96.0 }
}

/// The work area of the monitor a point sits on, in physical pixels.
pub fn monitor_work_at(x: i32, y: i32) -> (RECT, RECT) {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        monitor_rects(monitor)
    }
}

/// (full rect, work rect) of a monitor.
pub fn monitor_rects(monitor: HMONITOR) -> (RECT, RECT) {
    unsafe {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        (info.rcMonitor, info.rcWork)
    }
}

/// The monitor's device name, which is how a display is remembered across
/// launches — the counterpart of the macOS app remembering by UUID.
pub fn monitor_name(monitor: HMONITOR) -> String {
    unsafe {
        let mut info = MONITORINFOEXW {
            monitorInfo: MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        let _ = GetMonitorInfoW(
            monitor,
            &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
        );
        String::from_utf16_lossy(
            &info.szDevice[..info.szDevice.iter().position(|c| *c == 0).unwrap_or(0)],
        )
    }
}

pub fn monitor_from_hwnd(hwnd: HWND) -> HMONITOR {
    unsafe {
        MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)
    }
}

/// One-shot single-instance check. A second launch reports and exits.
pub fn acquire_single_instance() -> bool {
    unsafe {
        let handle = windows::Win32::System::Threading::CreateMutexW(
            None,
            false,
            w!("Pulse.Windows.SingleInstance"),
        );
        match handle {
            Ok(handle) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    return false;
                }
                // Intentionally leaked: the mutex lives as long as the
                // process does, and dropping the HANDLE closes it.
                std::mem::forget(handle);
                true
            }
            Err(_) => true,
        }
    }
}

pub fn center_cursor() -> POINT {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    pt
}

/// A `.wide()`-shaped convenience: NUL-terminated UTF-16.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn pcwstr(text: &str) -> PCWSTR {
    PCWSTR::from_raw(wide(text).as_ptr())
}

/// Keeps a wide buffer alive for the duration of a call that borrows it.
pub struct TempWide {
    buffer: Vec<u16>,
}

impl TempWide {
    pub fn new(text: &str) -> Self {
        TempWide { buffer: wide(text) }
    }

    pub fn as_ptr(&self) -> PCWSTR {
        PCWSTR::from_raw(self.buffer.as_ptr())
    }
}

pub fn post_wm_quit() {
    unsafe {
        let _ = PostQuitMessage(0);
    }
}

pub type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// The default arrow cursor, loaded lazily.
pub fn arrow_cursor() -> HCURSOR {
    unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() }
}

pub use windows::Win32::Foundation::RECT;

/// `HANDLE` that must not be closed for the process lifetime.
pub fn leak_handle(handle: HANDLE) {
    std::mem::forget(handle);
}
