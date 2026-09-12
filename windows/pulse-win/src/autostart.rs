//! Startup persistence: the `Run` key, the Windows counterpart of a login
//! item.

use windows::core::w;
use windows::Win32::System::Registry::*;

pub fn is_enabled() -> bool {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Run"),
            None,
            KEY_READ,
            &mut key,
        )
        .is_err()
        {
            return false;
        }
        let mut size: u32 = 0;
        let status = RegQueryValueExW(key, w!("Pulse"), None, None, None, Some(&mut size));
        let _ = RegCloseKey(key);
        status.is_ok()
    }
}

pub fn set_enabled(enabled: bool) {
    unsafe {
        let mut key = HKEY::default();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Run"),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .is_err()
        {
            return;
        }
        if enabled {
            if let Some(path) = current_exe_path() {
                let quoted = format!("\"{path}\"");
                let mut bytes: Vec<u8> = quoted
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .flat_map(|c| c.to_le_bytes())
                    .collect();
                let _ = RegSetValueExW(key, w!("Pulse"), None, REG_SZ, Some(&mut bytes));
            }
        } else {
            let _ = RegDeleteValueW(key, w!("Pulse"));
        }
        let _ = RegCloseKey(key);
    }
}

fn current_exe_path() -> Option<String> {
    unsafe {
        let mut buffer = [0u16; 1024];
        let len = windows::Win32::System::LibraryLoader::GetModuleFileNameW(None, &mut buffer);
        if len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..len as usize]))
    }
}
