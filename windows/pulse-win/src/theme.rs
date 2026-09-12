//! Fluent Design tokens, the WinUI visual language this port follows.
//!
//! The panel keeps the macOS app's obsidian surface (its identity, and the
//! right call for something that sits over your work all day), but every
//! text face, corner radius, hover state and control fill comes from
//! WinUI's dark-theme palette. The settings window follows the system theme
//! the way a WinUI app does, with a Mica backdrop.

#[derive(Debug, Clone, Copy)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Rgba { r, g, b, a }
    }

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Rgba { r, g, b, a: 1.0 }
    }

    pub fn with_alpha(self, a: f32) -> Rgba {
        Rgba { a, ..self }
    }

    pub fn from_hex(hex: u32) -> Self {
        Rgba::rgb(
            ((hex >> 16) & 0xFF) as f32 / 255.0,
            ((hex >> 8) & 0xFF) as f32 / 255.0,
            (hex & 0xFF) as f32 / 255.0,
        )
    }

    pub fn is_light(&self) -> bool {
        self.a > 0.5
    }
}

/// The dock's palette. There is no painted surface any more — the window is
/// real Mica, drawn by DWM — so the palette is only the ink that sits on it,
/// in one dark and one light voice that match whatever the backdrop resolved
/// to.
pub mod panel {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    use super::{system_accent, system_prefers_light, Rgba};

    /// The ink for one system appearance. Track and text follow the theme;
    /// the accent is the system's own and is shared by both.
    pub struct Palette {
        /// The ring track: a hair of the theme's own ink.
        pub track: Rgba,
        pub text_primary: Rgba,
        pub text_secondary: Rgba,
        pub text_tertiary: Rgba,
        pub text_disabled: Rgba,
        /// The progress-bar track on the detail card.
        pub bar_track: Rgba,
    }

    pub const DARK: Palette = Palette {
        track: Rgba::new(1.0, 1.0, 1.0, 0.14),
        text_primary: Rgba::rgb(1.0, 1.0, 1.0),
        text_secondary: Rgba::new(1.0, 1.0, 1.0, 0.786),
        text_tertiary: Rgba::new(1.0, 1.0, 1.0, 0.544),
        text_disabled: Rgba::new(1.0, 1.0, 1.0, 0.36),
        bar_track: Rgba::new(1.0, 1.0, 1.0, 0.17),
    };

    pub const LIGHT: Palette = Palette {
        track: Rgba::new(0.0, 0.0, 0.0, 0.14),
        text_primary: Rgba::rgb(0.1, 0.1, 0.1),
        text_secondary: Rgba::new(0.0, 0.0, 0.0, 0.61),
        text_tertiary: Rgba::new(0.0, 0.0, 0.0, 0.45),
        text_disabled: Rgba::new(0.0, 0.0, 0.0, 0.36),
        bar_track: Rgba::new(0.0, 0.0, 0.0, 0.15),
    };

    static DARK_MODE: AtomicBool = AtomicBool::new(true);
    static ACCENT: Mutex<Option<Rgba>> = Mutex::new(None);

    /// Re-reads the system appearance and accent. Called once before the
    /// first frame and again whenever Windows announces an immersive color
    /// set change.
    pub fn sync_theme() {
        DARK_MODE.store(!system_prefers_light(), Ordering::SeqCst);
        ACCENT.lock().unwrap().replace(system_accent());
    }

    pub fn is_dark() -> bool {
        DARK_MODE.load(Ordering::SeqCst)
    }

    /// The ink for whatever the system is currently showing.
    pub fn palette() -> &'static Palette {
        if DARK_MODE.load(Ordering::SeqCst) {
            &DARK
        } else {
            &LIGHT
        }
    }

    /// The system accent colour — what the Win11 clock draws its ring in.
    pub fn accent() -> Rgba {
        let cached = ACCENT.lock().unwrap();
        if let Some(c) = *cached {
            c
        } else {
            drop(cached);
            let c = system_accent();
            *ACCENT.lock().unwrap() = Some(c);
            c
        }
    }
}

pub fn system_accent() -> Rgba {
    use windows::UI::ViewManagement::{UIColorType, UISettings};
    let fallback = Rgba::from_hex(0x0078D4);
    let result: Result<Rgba, ()> = (|| {
        let settings = UISettings::new().map_err(|_| ())?;
        let color = settings.GetColorValue(UIColorType::Accent).map_err(|_| ())?;
        Ok(Rgba::new(
            color.R as f32 / 255.0,
            color.G as f32 / 255.0,
            color.B as f32 / 255.0,
            1.0,
        ))
    })();
    result.unwrap_or(fallback)
}

/// Whether apps should use light appearance, from the same registry value
/// Windows Settings writes.
pub fn system_prefers_light() -> bool {
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut key = HKEY::default();
        let open = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            windows::core::w!(
                r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"
            ),
            None,
            KEY_READ,
            &mut key,
        );
        if open.is_err() {
            return false;
        }
        let mut value_type = REG_VALUE_TYPE(0);
        let mut size: u32 = 4;
        let mut data: u32 = 0;
        let result = RegQueryValueExW(
            key,
            windows::core::w!("AppsUseLightTheme"),
            None,
            Some(&mut value_type),
            Some(&mut data as *mut u32 as *mut u8),
            Some(&mut size),
        );
        let _ = RegCloseKey(key);
        result.is_ok() && data == 1
    }
}

/// WinUI corner radii, in design units.
pub mod radius {
    /// Controls: text boxes, buttons, toggles.
    pub const CONTROL: f64 = 4.0;
    /// Cards and flyouts.
    pub const CARD: f64 = 8.0;
    /// Large surfaces (the settings window itself).
    pub const WINDOW: f64 = 8.0;
}
