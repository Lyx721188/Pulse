//! Small Win32 conveniences shared by every window in the app.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HINSTANCE, HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITORINFO, MONITORINFOEXW,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_APP_TRAY: u32 = WM_APP + 1;
/// The detail flyout -> panel: "pointer came in" / "pointer left".
pub const WM_APP_CARD: u32 = WM_APP + 3;

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
    unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }
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
            Ok(_) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    return false;
                }
                // HANDLE is Copy, so "leaking" is automatic: never
                // closing it is what keeps the mutex alive for the
                // process lifetime.
                true
            }
            Err(_) => true,
        }
    }
}

/// A NUL-terminated UTF-16 buffer; the vec outlives the calls it feeds.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
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

/// The default arrow cursor, loaded lazily.
pub fn arrow_cursor() -> HCURSOR {
    unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() }
}

pub use windows::Win32::Foundation::RECT;

/// The real Win11 window materials, in three DWM attributes: Mica behind
/// the whole client area, the frame's dark variant matching the system
/// appearance, and Windows' own corner rounding. Set **once**, at window
/// creation.
pub fn apply_system_backdrop(hwnd: HWND, dark: bool) {
    unsafe {
        let margins = windows::Win32::UI::Controls::MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        let _ = windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea(hwnd, &margins);
        // DWMSBT_MAINWINDOW is Mica.
        let backdrop: i32 = 2;
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
        set_backdrop_dark(hwnd, dark);
        // DWMWCP_ROUND: the corner radius Windows itself applies to
        // surfaces — not a radius this app draws.
        let corner: i32 = 2;
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

/// Updates only the dark-mode flag of the backdrop. Re-applying the whole
/// material set on a theme change provokes DWM into rebuilding its
/// composition for the window — which the DirectComposition visual tree
/// does not survive — so theme switches touch just this flag, which Mica
/// honours live.
pub fn set_backdrop_dark(hwnd: HWND, dark: bool) {
    unsafe {
        let dark_flag: i32 = dark as i32;
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark_flag as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}
