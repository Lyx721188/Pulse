//! Clipboard write, used by the diagnostic report and the device-flow code.

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;

pub fn set_text(text: &str) -> bool {
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = wide.len() * 2;
        let Ok(handle) = GlobalAlloc(GMEM_MOVEABLE, bytes) else {
            let _ = CloseClipboard();
            return false;
        };
        let lock = GlobalLock(handle);
        if lock.is_null() {
            let _ = GlobalUnlock(handle);
            let _ = CloseClipboard();
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), lock as *mut u16, wide.len());
        let _ = GlobalUnlock(handle);
        let ok = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(handle.0))).is_ok();
        let _ = CloseClipboard();
        ok
    }
}
