//! The tray icon and its menu — the Windows counterpart of the macOS
//! status item. Left click shows or hides the panel; right click opens the
//! menu. Balloon notifications ride the same icon.

use std::sync::mpsc::Sender;

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    DefSubclassProc, SetWindowSubclass, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE,
    NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::winutil;

pub enum TrayCommand {
    TogglePanel,
    OpenSettings,
    RefreshAll,
    Exit,
}

const ID_TOGGLE: u32 = 1001;
const ID_SETTINGS: u32 = 1002;
const ID_REFRESH: u32 = 1003;
const ID_EXIT: u32 = 1004;

pub struct TrayIcon {
    pub hwnd: HWND,
    data: NOTIFYICONDATAW,
    commands: Sender<TrayCommand>,
    /// The app's poll tick: the tray window carries the slow timer, and its
    /// firings become store polls.
    poll: Option<Sender<()>>,
}

/// A tiny runtime-drawn icon: the pulse ring, in the app's good green.
fn build_icon() -> HICON {
    // A 32×32 ring drawn with GDI into a colour bitmap. Direct2D would be
    // nicer, but the tray wants an HICON, and four GDI calls make one.
    unsafe {
        let size = 32i32;
        let hdc = windows::Win32::Graphics::Gdi::CreateCompatibleDC(None);
        let mut bmi = windows::Win32::Graphics::Gdi::BITMAPINFO {
            bmiHeader: windows::Win32::Graphics::Gdi::BITMAPINFOHEADER {
                biSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>()
                    as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: windows::Win32::Graphics::Gdi::BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(bitmap) = windows::Win32::Graphics::Gdi::CreateDIBSection(
            Some(hdc),
            &bmi,
            windows::Win32::Graphics::Gdi::DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        ) else {
            return HICON::default();
        };
        let old = windows::Win32::Graphics::Gdi::SelectObject(hdc, bitmap.into());

        // Transparent background, then the ring: green stroke, square caps
        // drawn as a thick circle with a hole.
        let pixels = bits as *mut [u8; 4];
        let cx = 15.5f32;
        let cy = 15.5f32;
        let outer = 12.5f32;
        let inner = 7.5f32;
        for y in 0..size as usize {
            for x in 0..size as usize {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let d = (dx * dx + dy * dy).sqrt();
                let alpha = if d <= outer && d >= inner { 255 } else { 0 };
                let fade = if d > outer - 1.5 {
                    ((outer - d) * 170.0).clamp(0.0, 255.0) as u32
                } else if d < inner + 1.5 {
                    ((d - inner) * 170.0).clamp(0.0, 255.0) as u32
                } else {
                    255
                };
                *pixels.add(y * size as usize + x) = [
                    0,   // blue
                    230, // green (0.90)
                    140, // red (0.55)
                    ((alpha as f32 * fade as f32) / 255.0) as u8,
                ];
            }
        }
        let _ = windows::Win32::Graphics::Gdi::SelectObject(hdc, old);
        let _ = windows::Win32::Graphics::Gdi::DeleteDC(hdc);
        bmi.bmiHeader.biSizeImage = 0;

        let mut icon_info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: bitmap,
            hbmColor: bitmap,
        };
        // A mask bitmap is required: a plain all-zero one suffices for an
        // alpha icon.
        let mask = windows::Win32::Graphics::Gdi::CreateBitmap(size, size, 1, 1, None);
        icon_info.hbmMask = mask;
        let icon = CreateIconIndirect(&icon_info).unwrap_or_default();
        let _ = windows::Win32::Graphics::Gdi::DeleteObject(bitmap.into());
        let _ = windows::Win32::Graphics::Gdi::DeleteObject(mask.into());
        icon
    }
}

impl TrayIcon {
    pub fn new(commands: Sender<TrayCommand>) -> Box<TrayIcon> {
        let class_name = w!("PulseTrayWindow");
        unsafe {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(tray_wndproc),
                lpszClassName: class_name,
                hInstance: winutil::hinstance(),
                ..Default::default()
            };
            RegisterClassW(&wc);
        }

        let mut tray = Box::new(TrayIcon {
            hwnd: HWND::default(),
            data: NOTIFYICONDATAW::default(),
            commands,
            poll: None,
        });

        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                w!("Pulse"),
                WS_OVERLAPPEDWINDOW & !WS_VISIBLE,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(winutil::hinstance()),
                None,
            )
            .expect("tray window");
            tray.hwnd = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &*tray as *const TrayIcon as isize);

            let mut data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                uCallbackMessage: winutil::WM_APP_TRAY,
                hIcon: build_icon(),
                ..Default::default()
            };
            let tip: Vec<u16> = "Pulse".encode_utf16().collect();
            data.szTip[..tip.len()].copy_from_slice(&tip);
            if Shell_NotifyIconW(NIM_ADD, &mut data).as_bool() {
                tray.data = data;
            }
        }
        tray
    }

    pub fn set_poll(&mut self, poll: Sender<()>) {
        self.poll = Some(poll);
    }

    pub fn show_balloon(&mut self, title: &str, text: &str, warning: bool) {
        unsafe {
            let mut data = self.data;
            data.uFlags = NIF_INFO;
            let title_w: Vec<u16> = title.encode_utf16().take(63).collect();
            let text_w: Vec<u16> = text.encode_utf16().take(255).collect();
            data.szInfoTitle[..title_w.len()].copy_from_slice(&title_w);
            data.szInfo[..text_w.len()].copy_from_slice(&text_w);
            data.dwInfoFlags = if warning {
                windows::Win32::UI::Shell::NIIF_WARNING
            } else {
                windows::Win32::UI::Shell::NIIF_INFO
            };
            let _ = Shell_NotifyIconW(NIM_MODIFY, &mut data);
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &mut self.data);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn tray_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if state == 0 {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let tray = &mut *(state as *mut TrayIcon);

    match msg {
        m if m == winutil::WM_APP_TRAY => {
            let event = (lparam.0 & 0xFFFF) as u32;
            match event {
                WM_LBUTTONUP => {
                    let _ = tray.commands.send(TrayCommand::TogglePanel);
                    LRESULT(0)
                }
                WM_RBUTTONUP | WM_CONTEXTMENU => {
                    show_menu(hwnd, &tray.commands);
                    LRESULT(0)
                }
                _ => LRESULT(0),
            }
        }
        WM_TIMER => {
            if let Some(poll) = tray.poll.as_ref() {
                let _ = poll.send(());
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = wparam.0 as u32;
            let command = match id {
                ID_TOGGLE => Some(TrayCommand::TogglePanel),
                ID_SETTINGS => Some(TrayCommand::OpenSettings),
                ID_REFRESH => Some(TrayCommand::RefreshAll),
                ID_EXIT => Some(TrayCommand::Exit),
                _ => None,
            };
            if let Some(command) = command {
                let _ = tray.commands.send(command);
            }
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn show_menu(hwnd: HWND, commands: &Sender<TrayCommand>) {
    let _ = commands;
    let menu = match CreatePopupMenu() {
        Ok(menu) => menu,
        Err(_) => return,
    };
    let _ = AppendMenuW(menu, MF_STRING, ID_TOGGLE as usize, w!("Show panel"));
    let _ = AppendMenuW(menu, MF_STRING, ID_REFRESH as usize, w!("Refresh all"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_SETTINGS as usize, w!("Settings"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_EXIT as usize, w!("Exit"));

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // The menu's window must be foreground for TrackPopupMenu to dismiss
    // on an outside click; the classic tray-menu dance.
    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenuEx(
        menu,
        (TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_RIGHTALIGN).0 as u32,
        pt.x,
        pt.y,
        hwnd,
        Some(std::ptr::null::<TPMPARAMS>()),
    );
    let _ = DestroyMenu(menu);
}

/// Installed once so the subclass keeps the tray window's messages flowing
/// even if a future window takes the class over.
pub fn install_subclass(hwnd: HWND) {
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(subclass_proc), 1, 0);
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    DefSubclassProc(hwnd, msg, wparam, lparam)
}
