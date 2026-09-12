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

/// The panel's palette — always dark, always solid.
pub mod panel {
    use super::Rgba;

    /// The obsidian surface. Solid, not translucent: translucency reads as
    /// "no reading" over a busy desktop, the same lesson the macOS app
    /// learned with Liquid Glass.
    pub const SURFACE: Rgba = Rgba::rgb(0.118, 0.118, 0.118); // #1E1E1E
    /// A hairline where the surface meets the desktop.
    pub const SURFACE_STROKE: Rgba = Rgba::new(1.0, 1.0, 1.0, 0.07);
    /// The ring track: `Color.primary.opacity(0.18)` on the macOS side.
    pub const TRACK: Rgba = Rgba::new(1.0, 1.0, 1.0, 0.18);
    pub const TEXT_PRIMARY: Rgba = Rgba::rgb(1.0, 1.0, 1.0);
    pub const TEXT_SECONDARY: Rgba = Rgba::new(1.0, 1.0, 1.0, 0.786);
    pub const TEXT_TERTIARY: Rgba = Rgba::new(1.0, 1.0, 1.0, 0.544);
    pub const TEXT_DISABLED: Rgba = Rgba::new(1.0, 1.0, 1.0, 0.36);
    /// The detail card, one Fluent layer above the surface.
    pub const CARD: Rgba = Rgba::rgb(0.165, 0.165, 0.165); // #2A2A2A
    pub const CARD_STROKE: Rgba = Rgba::new(1.0, 1.0, 1.0, 0.08);
    /// The progress-bar track on the card.
    pub const BAR_TRACK: Rgba = Rgba::new(1.0, 1.0, 1.0, 0.17);
}

/// The settings window's palette. Two sets, because a WinUI app follows the
/// system's light or dark choice.
#[derive(Debug, Clone, Copy)]
pub struct SettingsPalette {
    /// Window background. With Mica on, the window paints transparent and
    /// the backdrop shows through; this is the fallback.
    pub background: Rgba,
    /// Card / layer fill (sidebar, list rows).
    pub layer: Rgba,
    pub layer_stroke: Rgba,
    /// Control fill: text boxes, comboboxes.
    pub control: Rgba,
    pub control_hover: Rgba,
    pub control_stroke: Rgba,
    pub text: Rgba,
    pub text_secondary: Rgba,
    pub text_tertiary: Rgba,
    /// Accent fill for primary buttons and toggles.
    pub accent: Rgba,
    pub accent_text: Rgba,
    /// Window caption area.
    pub caption_hover: Rgba,
    pub caption_close_hover: Rgba,
    pub is_dark: bool,
}

impl SettingsPalette {
    /// WinUI dark theme resources.
    pub fn dark(accent: Rgba) -> SettingsPalette {
        SettingsPalette {
            background: Rgba::rgb(0.125, 0.125, 0.125), // #202020
            layer: Rgba::rgb(0.164, 0.164, 0.164),      // #2A2A2A (layer)
            layer_stroke: Rgba::new(1.0, 1.0, 1.0, 0.08),
            control: Rgba::new(1.0, 1.0, 1.0, 0.0605),  // #FFFFFF0F
            control_hover: Rgba::new(1.0, 1.0, 1.0, 0.0837),
            control_stroke: Rgba::new(1.0, 1.0, 1.0, 0.0698),
            text: Rgba::rgb(1.0, 1.0, 1.0),
            text_secondary: Rgba::new(1.0, 1.0, 1.0, 0.786),
            text_tertiary: Rgba::new(1.0, 1.0, 1.0, 0.544),
            accent,
            accent_text: Rgba::rgb(0.0, 0.0, 0.0),
            caption_hover: Rgba::new(1.0, 1.0, 1.0, 0.0605),
            caption_close_hover: Rgba::rgb(0.792, 0.157, 0.157), // #C42B1C
            is_dark: true,
        }
    }

    /// WinUI light theme resources.
    pub fn light(accent: Rgba) -> SettingsPalette {
        SettingsPalette {
            background: Rgba::rgb(0.973, 0.973, 0.973), // #F8F8F8-ish
            layer: Rgba::rgb(1.0, 1.0, 1.0),
            layer_stroke: Rgba::new(0.0, 0.0, 0.0, 0.058),
            control: Rgba::new(1.0, 1.0, 1.0, 0.7),
            control_hover: Rgba::new(0.976, 0.976, 0.976, 0.5),
            control_stroke: Rgba::new(0.0, 0.0, 0.0, 0.096),
            text: Rgba::rgb(0.0, 0.0, 0.0),
            text_secondary: Rgba::new(0.0, 0.0, 0.0, 0.606),
            text_tertiary: Rgba::new(0.0, 0.0, 0.0, 0.446),
            accent,
            accent_text: Rgba::rgb(1.0, 1.0, 1.0),
            caption_hover: Rgba::new(0.0, 0.0, 0.0, 0.0373),
            caption_close_hover: Rgba::rgb(0.792, 0.157, 0.157),
            is_dark: false,
        }
    }
}

/// The system accent color, the way WinUI reads it. Falls back to the
/// Fluent default blue when the WinRT call is unavailable.
pub fn system_accent() -> Rgba {
    use windows::UI::ViewManagement::{UIColorType, UISettings};
    let fallback = Rgba::from_hex(0x0078D4);
    let result: Result<Rgba, ()> = (|| {
        unsafe {
            let settings = UISettings::new().map_err(|_| ())?;
            let color = settings.GetColorValue(UIColorType::Accent).map_err(|_| ())?;
            Ok(Rgba::new(
                color.R as f32 / 255.0,
                color.G as f32 / 255.0,
                color.B as f32 / 255.0,
                1.0,
            ))
        }
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
