//! The settings window, in the WinUI visual language: a Mica-backed
//! window with a custom caption, a NavigationView-style sidebar, and the
//! Fluent controls — toggles, combo boxes, text boxes and buttons — drawn
//! to the dark- and light-theme resources.
//!
//! The window owns its widgets as a flat list laid out top to bottom with
//! a scroll offset, the way a settings pane actually reads. Hit testing
//! walks the same layout the painter drew, so the two cannot disagree.

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::theme::{radius, system_accent, system_prefers_light, SettingsPalette, Rgba};
use crate::winutil;

pub enum SettingsAction {
    /// A setting changed; the app re-reads everything it drives.
    Changed,
    RefreshProvider(String),
    SaveKey(String, String),
    SignInCopilot,
    SignOutCopilot,
    OpenUrl(String),
    Close,
}

// Widget ids. Ranges keep the match readable.
mod ids {
    pub const CHOICE_LANGUAGE: u32 = 100;
    pub const CHOICE_PANEL_SIZE: u32 = 101;
    pub const CHOICE_RAIL_SPACING: u32 = 102;
    pub const CHOICE_DOCK: u32 = 103;
    pub const CHOICE_INTERVAL: u32 = 104;
    pub const TOGGLE_STARTUP: u32 = 110;
    pub const TOGGLE_SIDE_PCT: u32 = 111;
    pub const TOGGLE_TOP_PCT: u32 = 112;
    pub const TOGGLE_LABEL_ABOVE: u32 = 113;
    pub const TOGGLE_REMAINING: u32 = 114;
    pub const TOGGLE_CLOCK: u32 = 115;
    pub const TOGGLE_SECOND_RING: u32 = 116;
    pub const TOGGLE_FORECAST: u32 = 117;
    pub const TOGGLE_AUTO_COLLAPSE: u32 = 118;
    pub const TOGGLE_FOLLOW_DISPLAY: u32 = 119;
    pub const TOGGLE_ALERTS: u32 = 130;
    pub const CHOICE_ALERT_THRESHOLD: u32 = 131;
    pub const TOGGLE_ALERT_RESET: u32 = 132;
    pub const TOGGLE_ALERT_FAILURE: u32 = 133;
    pub const TOGGLE_LOW_BALANCE: u32 = 134;
    pub const BUTTON_DIAGNOSTICS: u32 = 150;
    pub const PROVIDER_TOGGLE: u32 = 1000;
    pub const PROVIDER_KEY: u32 = 2000;
    pub const PROVIDER_SAVE: u32 = 3000;
    pub const PROVIDER_REFRESH: u32 = 4000;
    pub const PROVIDER_SIGNIN: u32 = 5000;
    pub const PROVIDER_PIN: u32 = 6000;
}

pub enum Widget {
    Title(String),
    Section(String),
    Toggle {
        id: u32,
        label: String,
        detail: Option<String>,
        value: bool,
    },
    Choice {
        id: u32,
        label: String,
        options: Vec<String>,
        selected: usize,
    },
    Text {
        id: u32,
        label: String,
        value: String,
        masked: bool,
        /// Hint under the field.
        hint: Option<String>,
    },
    Button {
        id: u32,
        label: String,
        primary: bool,
    },
    Note(String),
    Status(String),
    Separator,
    Gap(f64),
}

struct LaidOut {
    widget: Widget,
    /// Full-row rect, in physical pixels, relative to the content origin.
    rect: RECT,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    General,
    Accounts,
    Notifications,
    About,
}

impl Page {
    fn label(&self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Accounts => "Accounts",
            Page::Notifications => "Notifications",
            Page::About => "About",
        }
    }

    fn glyph(&self) -> &str {
        match self {
            Page::General => "\u{E713}",
            Page::Accounts => "\u{E77B}",
            Page::Notifications => "\u{EA8F}",
            Page::About => "\u{E946}",
        }
    }

    fn index(&self) -> usize {
        match self {
            Page::General => 0,
            Page::Accounts => 1,
            Page::Notifications => 2,
            Page::About => 3,
        }
    }
}

const SIDEBAR_WIDTH: i32 = 220;
const CAPTION_HEIGHT: i32 = 44;
const CONTENT_PADDING: i32 = 28;
const ROW_HEIGHT: i32 = 44;
const CONTROL_HEIGHT: i32 = 32;

pub struct SettingsWindow {
    pub hwnd: HWND,
    palette: SettingsPalette,
    page: Page,
    widgets: Vec<Widget>,
    layout: Vec<(usize, RECT)>,
    scroll: i32,
    content_height: i32,
    client: (i32, i32),
    /// Text widget buffers, keyed by id — the field's own state, committed
    /// by the Save button rather than by every keystroke.
    text_values: std::cell::RefCell<HashMap<u32, String>>,
    focused_text: Option<u32>,
    hover: Option<(i32, i32)>,
    /// The provider index the Accounts page is showing, or none for the
    /// list of all of them.
    account_focus: Option<usize>,
    actions: Sender<SettingsAction>,
    canvas: Option<MemCanvas>,
    dpi: f64,
    /// Device flow in progress: (user code, verification url).
    pub device_flow: Option<(String, String)>,
    /// The statuses shown under each account, filled by the app as
    /// readings land.
    pub status_cache: HashMap<String, String>,
}

/// A memory-DC canvas presented through WM_PAINT — the settings window is
/// opaque, so it paints normally instead of going through a layered
/// update.
struct MemCanvas {
    memdc: HDC,
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    rt: ID2D1DCRenderTarget,
    width: i32,
    height: i32,
}

impl MemCanvas {
    fn new(width: i32, height: i32) -> windows::core::Result<MemCanvas> {
        unsafe {
            let engine = crate::d2d::global_engine();
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 0.0,
                dpiY: 0.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = engine.factory.CreateDCRenderTarget(&props)?;
            let (memdc, bitmap, old_bitmap) = crate::d2d::make_dib_pub(width, height)?;
            rt.BindDC(memdc, &RECT { left: 0, top: 0, right: width, bottom: height })?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            Ok(MemCanvas { memdc, bitmap, old_bitmap, rt, width, height })
        }
    }

    fn resize(&mut self, width: i32, height: i32) -> windows::core::Result<()> {
        if self.width == width && self.height == height {
            return Ok(());
        }
        unsafe {
            let (memdc, bitmap, old_bitmap) = crate::d2d::make_dib_pub(width, height)?;
            DeleteObject(self.bitmap.into());
            self.memdc = memdc;
            self.bitmap = bitmap;
            self.old_bitmap = old_bitmap;
            self.rt.BindDC(memdc, &RECT { left: 0, top: 0, right: width, bottom: height })?;
            self.width = width;
            self.height = height;
        }
        Ok(())
    }
}

impl Drop for MemCanvas {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.memdc, self.old_bitmap);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.memdc);
        }
    }
}

impl SettingsWindow {
    pub fn new(actions: Sender<SettingsAction>) -> Box<SettingsWindow> {
        let class_name = w!("PulseSettings");
        unsafe {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(settings_wndproc),
                lpszClassName: class_name,
                hInstance: winutil::hinstance(),
                hCursor: winutil::arrow_cursor(),
                ..Default::default()
            };
            RegisterClassW(&wc);
        }

        let mut window = Box::new(SettingsWindow {
            hwnd: HWND::default(),
            palette: Self::palette(),
            page: Page::General,
            widgets: Vec::new(),
            layout: Vec::new(),
            scroll: 0,
            content_height: 0,
            client: (960, 640),
            text_values: std::cell::RefCell::new(HashMap::new()),
            focused_text: None,
            hover: None,
            account_focus: None,
            actions,
            canvas: None,
            dpi: 1.0,
            device_flow: None,
            status_cache: HashMap::new(),
        });

        window.rebuild_widgets();

        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                w!("Pulse Settings"),
                WS_OVERLAPPEDWINDOW & !WS_VISIBLE,
                100,
                100,
                window.client.0,
                window.client.1,
                None,
                None,
                Some(winutil::hinstance()),
                None,
            )
            .expect("settings window");
            window.hwnd = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &*window as *const SettingsWindow as isize);

            // The WinUI window: dark chrome, a Mica backdrop, and the frame
            // extended under the client so the backdrop can reach it.
            let dark: i32 = window.palette.is_dark as i32;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const i32 as *const core::ffi::c_void,
                4,
            );
            let backdrop = DWMSBT_MAINWINDOW.0 as i32;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &backdrop as *const i32 as *const core::ffi::c_void,
                4,
            );
            let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
            let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);

            ShowWindow(hwnd, SW_SHOW);
        }
        window.on_size();
        window
    }

    fn palette() -> SettingsPalette {
        let accent = system_accent();
        if system_prefers_light() {
            SettingsPalette::light(accent)
        } else {
            SettingsPalette::dark(accent)
        }
    }

    pub fn on_size(&mut self) {
        unsafe {
            let mut rect = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut rect);
            self.client = (rect.right, rect.bottom);
            self.dpi = winutil::dpi_for(self.hwnd);
        }
        if self.canvas.is_none() {
            self.canvas = MemCanvas::new(self.client.0.max(1), self.client.1.max(1)).ok();
        } else if let Some(canvas) = self.canvas.as_mut() {
            let _ = canvas.resize(self.client.0.max(1), self.client.1.max(1));
        }
        self.relayout();
        self.invalidate();
    }

    pub fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// Rebuilds the widget list for the current page and state. Called on
    /// every page switch and after every commit, so the widgets always
    /// describe the settings as they are.
    pub fn rebuild_widgets(&mut self) {
        self.widgets = match self.page {
            Page::General => self.general_widgets(),
            Page::Accounts => self.accounts_widgets(),
            Page::Notifications => self.notifications_widgets(),
            Page::About => self.about_widgets(),
        };
        self.relayout();
    }

    fn general_widgets(&self) -> Vec<Widget> {
        let s = pulse_core::settings::with(|s| s.clone());
        vec![
            Widget::Title(pulse_core::localization::t("General").to_string()),
            Widget::Section(pulse_core::localization::t("Panel").to_string()),
            Widget::Choice {
                id: ids::CHOICE_DOCK,
                label: pulse_core::localization::t("Dock to").to_string(),
                options: vec!["Right".into(), "Left".into(), "Top".into(), "Floating".into()],
                selected: if s.floating {
                    3
                } else {
                    match s.dock_side.as_str() {
                        "left" => 1,
                        "top" => 2,
                        _ => 0,
                    }
                },
            },
            Widget::Choice {
                id: ids::CHOICE_PANEL_SIZE,
                label: pulse_core::localization::t("Panel size").to_string(),
                options: vec![
                    pulse_core::localization::t("Small").to_string(),
                    pulse_core::localization::t("Standard").to_string(),
                    pulse_core::localization::t("Large").to_string(),
                ],
                selected: match s.panel_size.as_str() {
                    "small" => 0,
                    "large" => 2,
                    _ => 1,
                },
            },
            Widget::Choice {
                id: ids::CHOICE_RAIL_SPACING,
                label: pulse_core::localization::t("Ring spacing").to_string(),
                options: vec![
                    pulse_core::localization::t("Tight").to_string(),
                    pulse_core::localization::t("Standard").to_string(),
                    pulse_core::localization::t("Loose").to_string(),
                ],
                selected: match s.rail_spacing.as_str() {
                    "compact" => 0,
                    "roomy" => 2,
                    _ => 1,
                },
            },
            Widget::Toggle {
                id: ids::TOGGLE_SIDE_PCT,
                label: pulse_core::localization::t("Show percent labels").to_string(),
                detail: None,
                value: s.side_rail_shows_percentages,
            },
            Widget::Toggle {
                id: ids::TOGGLE_REMAINING,
                label: pulse_core::localization::t("Show what's left").to_string(),
                detail: None,
                value: s.shows_remaining,
            },
            Widget::Toggle {
                id: ids::TOGGLE_CLOCK,
                label: pulse_core::localization::t("Show window clock").to_string(),
                detail: None,
                value: s.shows_window_clock,
            },
            Widget::Toggle {
                id: ids::TOGGLE_SECOND_RING,
                label: pulse_core::localization::t("Show second ring").to_string(),
                detail: None,
                value: s.shows_second_ring,
            },
            Widget::Toggle {
                id: ids::TOGGLE_FORECAST,
                label: pulse_core::localization::t("Show forecast").to_string(),
                detail: None,
                value: s.shows_forecast,
            },
            Widget::Toggle {
                id: ids::TOGGLE_AUTO_COLLAPSE,
                label: pulse_core::localization::t("Auto-collapse when idle").to_string(),
                detail: None,
                value: s.auto_collapse,
            },
            Widget::Toggle {
                id: ids::TOGGLE_FOLLOW_DISPLAY,
                label: pulse_core::localization::t("Follow the active display").to_string(),
                detail: None,
                value: s.follows_active_display,
            },
            Widget::Gap(8.0),
            Widget::Section(pulse_core::localization::t("Refresh").to_string()),
            Widget::Choice {
                id: ids::CHOICE_INTERVAL,
                label: pulse_core::localization::t("Refresh interval").to_string(),
                options: vec![
                    pulse_core::localization::t("Automatic").to_string(),
                    "30s".into(),
                    "1min".into(),
                    "2min".into(),
                    "5min".into(),
                    "10min".into(),
                    "30min".into(),
                ],
                selected: match s.refresh_interval {
                    30 => 1,
                    60 => 2,
                    120 => 3,
                    300 => 4,
                    600 => 5,
                    1800 => 6,
                    _ => 0,
                },
            },
            Widget::Gap(8.0),
            Widget::Section(pulse_core::localization::t("Windows").to_string()),
            Widget::Toggle {
                id: ids::TOGGLE_STARTUP,
                label: pulse_core::localization::t("Launch at startup").to_string(),
                detail: None,
                value: crate::autostart::is_enabled(),
            },
            Widget::Choice {
                id: ids::CHOICE_LANGUAGE,
                label: pulse_core::localization::t("Language").to_string(),
                options: vec![
                    pulse_core::localization::t("Automatic").to_string(),
                    "English".into(),
                    "简体中文".into(),
                ],
                selected: match s.language.as_str() {
                    "en" => 1,
                    "zh" => 2,
                    _ => 0,
                },
            },
        ]
    }

    fn accounts_widgets(&self) -> Vec<Widget> {
        let mut widgets = vec![
            Widget::Title(pulse_core::localization::t("Accounts").to_string()),
            Widget::Note(pulse_core::localization::t("Each provider reports its own figures by the route that product offers — Pulse holds no account of its own and sends nothing anywhere but to the provider you already use.").to_string()),
            Widget::Gap(4.0),
        ];

        let snapshot = pulse_core::settings::with(|s| s.clone());
        for (index, provider) in pulse_core::model::ALL_PROVIDERS.into_iter().enumerate() {
            let enabled = snapshot.enabled_accounts.contains(provider.raw());
            widgets.push(Widget::Section(provider.display_name().to_string()));

            if !provider.is_ported_to_windows() {
                widgets.push(Widget::Note(
                    provider.windows_gap().unwrap_or("Not available on Windows.").to_string(),
                ));
                widgets.push(Widget::Gap(6.0));
                continue;
            }

            widgets.push(Widget::Toggle {
                id: ids::PROVIDER_TOGGLE + index as u32,
                label: pulse_core::localization::t("Enabled").to_string(),
                detail: None,
                value: enabled,
            });

            // The credential, in the shape the provider's route takes.
            match provider {
                pulse_core::model::Provider::ClaudeCode | pulse_core::model::Provider::Codex | pulse_core::model::Provider::Grok => {
                    widgets.push(Widget::Note(
                        pulse_core::localization::t("Reads the login this tool already saved on this PC.").to_string(),
                    ));
                }
                pulse_core::model::Provider::Copilot => {
                    let signed_in = self.text_values.borrow().get(&(ids::PROVIDER_KEY + index as u32)).is_some()
                        || pulse_core::secrets::key_for("copilot").is_some();
                    if signed_in {
                        widgets.push(Widget::Note(
                            pulse_core::localization::t("Signed in to GitHub with read-only access.").to_string(),
                        ));
                        widgets.push(Widget::Button {
                            id: ids::PROVIDER_SIGNIN + index as u32,
                            label: pulse_core::localization::t("Sign out").to_string(),
                            primary: false,
                        });
                    } else {
                        widgets.push(Widget::Button {
                            id: ids::PROVIDER_SIGNIN + index as u32,
                            label: pulse_core::localization::t("Sign in with GitHub").to_string(),
                            primary: true,
                        });
                        widgets.push(Widget::Note(
                            pulse_core::localization::t("Device-code sign-in. Requests read:user only — never your repositories. The consent page names the editor whose client it borrows; this is not an official integration.").to_string(),
                        ));
                    }
                    // Expose the stored token for the sign-out path.
                    if let Some(token) = pulse_core::secrets::key_for("copilot") {
                        self.text_values
                            .borrow_mut()
                            .entry(ids::PROVIDER_KEY + index as u32)
                            .or_insert_with(|| token);
                    }
                }
                _ => {
                    let current = self
                        .text_values
                        .borrow()
                        .get(&(ids::PROVIDER_KEY + index as u32))
                        .cloned()
                        .or_else(|| pulse_core::secrets::key_for(provider.raw()))
                        .unwrap_or_default();
                    widgets.push(Widget::Text {
                        id: ids::PROVIDER_KEY + index as u32,
                        label: pulse_core::localization::t("API key").to_string(),
                        value: current,
                        masked: true,
                        hint: None,
                    });
                    widgets.push(Widget::Button {
                        id: ids::PROVIDER_SAVE + index as u32,
                        label: pulse_core::localization::t("Save").to_string(),
                        primary: true,
                    });
                }
            }

            widgets.push(Widget::Button {
                id: ids::PROVIDER_REFRESH + index as u32,
                label: pulse_core::localization::t("Refresh now").to_string(),
                primary: false,
            });
            widgets.push(Widget::Status(self.status_for(provider)));
            widgets.push(Widget::Gap(10.0));
        }
        widgets
    }

    fn status_for(&self, provider: pulse_core::model::Provider) -> String {
        match self.status_cache.get(&provider.raw().to_string()) {
            Some(text) => text.clone(),
            None => String::new(),
        }
    }

    fn notifications_widgets(&self) -> Vec<Widget> {
        let s = pulse_core::settings::with(|s| s.clone());
        let mut widgets = vec![
            Widget::Title(pulse_core::localization::t("Notifications").to_string()),
            Widget::Note(pulse_core::localization::t("All notifications are off until you turn them on.").to_string()),
            Widget::Gap(4.0),
            Widget::Toggle {
                id: ids::TOGGLE_ALERTS,
                label: pulse_core::localization::t("Enable notifications").to_string(),
                detail: None,
                value: s.wants_alerts,
            },
        ];
        if s.wants_alerts {
            widgets.push(Widget::Choice {
                id: ids::CHOICE_ALERT_THRESHOLD,
                label: pulse_core::localization::t("Warn me when a limit passes").to_string(),
                options: vec!["75%".into(), "80%".into(), "90%".into(), "95%".into()],
                selected: match s.alert_threshold {
                    80 => 1,
                    90 => 2,
                    95 => 3,
                    _ => 0,
                },
            });
            widgets.push(Widget::Toggle {
                id: ids::TOGGLE_ALERT_RESET,
                label: pulse_core::localization::t("when a warned window comes back").to_string(),
                detail: None,
                value: s.alerts_on_reset,
            });
            widgets.push(Widget::Toggle {
                id: ids::TOGGLE_ALERT_FAILURE,
                label: pulse_core::localization::t("when checks keep failing").to_string(),
                detail: None,
                value: s.alerts_on_failure,
            });
        }
        widgets
    }

    fn about_widgets(&self) -> Vec<Widget> {
        vec![
            Widget::Title("Pulse".to_string()),
            Widget::Note(pulse_core::localization::t("A screen-edge monitor for your AI coding allowances.").to_string()),
            Widget::Status(format!("{} 1.1.0 (Windows)", pulse_core::localization::t("Version"))),
            Widget::Gap(8.0),
            Widget::Note("No Pulse servers, no Pulse account, no telemetry. Requests go to the providers you already use and follow the Windows system proxy settings.".to_string()),
            Widget::Button {
                id: 900,
                label: "github.com/Lyx721188/Pulse".to_string(),
                primary: false,
            },
        ]
    }

    /// Positions every widget. One pass, top to bottom; the same rects the
    /// painter will draw and the hit test will walk.
    fn relayout(&mut self) {
        let scale = self.dpi.max(0.5);
        let content_w = self.client.0 - SIDEBAR_WIDTH - CONTENT_PADDING * 2;
        let mut y = CAPTION_HEIGHT + CONTENT_PADDING + (self.scroll * -1);
        self.layout.clear();

        let content_left = SIDEBAR_WIDTH + CONTENT_PADDING;
        for (index, widget) in self.widgets.iter().enumerate() {
            let height = match widget {
                Widget::Title(_) => (36.0 * scale) as i32,
                Widget::Section(_) => (28.0 * scale) as i32,
                Widget::Toggle { .. } | Widget::Choice { .. } | Widget::Text { .. } => {
                    (ROW_HEIGHT as f64 * scale) as i32
                }
                Widget::Button { .. } => (CONTROL_HEIGHT as f64 * scale) as i32 + (8.0 * scale) as i32,
                Widget::Note(_) => (52.0 * scale) as i32,
                Widget::Status(_) => (24.0 * scale) as i32,
                Widget::Separator => (13.0 * scale) as i32,
                Widget::Gap(units) => (*units * scale) as i32,
            };
            let rect = RECT {
                left: content_left,
                top: y,
                right: content_left + content_w,
                bottom: y + height,
            };
            self.layout.push((index, rect));
            y += height;
        }
        self.content_height = y - CAPTION_HEIGHT + CONTENT_PADDING;
    }

    /// Paints one frame.
    pub fn render(&mut self) {
        let Some(canvas) = self.canvas.as_ref() else {
            return;
        };
        let engine = crate::d2d::global_engine();
        unsafe {
            canvas.rt.BeginDraw();
            canvas.rt.Clear(None);
        }
        // Owned copies: `render_widgets` mutates hover state, so neither
        // the painter nor the blit may hold a borrow of `self` while it
        // runs.
        let rt = canvas.rt.clone();
        let memdc = canvas.memdc;
        let painter = crate::d2d::Painter {
            rt: &rt,
            engine,
        };
        let scale = self.dpi as f32;
        unsafe {
            painter.rt.SetTransform(&crate::d2d::scale_matrix(scale, scale));
        }

        // The backdrop: where Mica shows, alpha stays zero. The DWM
        // backdrop paints the extended frame; anything with alpha here
        // covers it.
        let bg = painter.brush(if self.palette.is_dark {
            Rgba::rgb(0.125, 0.125, 0.125).with_alpha(0.02)
        } else {
            Rgba::rgb(0.973, 0.973, 0.973).with_alpha(0.02)
        }).unwrap();
        unsafe {
            painter.rt.FillRectangle(
                &crate::d2d::rect(0.0, 0.0, self.client.0 as f32 / scale, self.client.1 as f32 / scale),
                &bg,
            );
        }

        self.render_sidebar(&painter);
        self.render_widgets(&painter);
        self.render_caption(&painter);

        unsafe {
            let _ = rt.EndDraw(None, None);
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(self.hwnd, &mut ps);
            let _ = BitBlt(
                hdc,
                0,
                0,
                self.client.0,
                self.client.1,
                Some(memdc),
                0,
                0,
                SRCCOPY,
            );
            let _ = EndPaint(self.hwnd, &ps);
        }
    }

    fn render_sidebar(&self, painter: &crate::d2d::Painter) {
        let scale = self.dpi as f32;
        let sidebar_w = SIDEBAR_WIDTH as f32 / scale;
        let client_h = self.client.1 as f32 / scale;

        // The sidebar is one Fluent layer above the backdrop.
        let layer = painter.brush(self.palette.layer.with_alpha(0.6)).unwrap();
        painter
            .fill_rounded_rect(
                crate::d2d::rect(0.0, 0.0, sidebar_w, client_h),
                0.0,
                &layer,
            );

        let pages = [Page::General, Page::Accounts, Page::Notifications, Page::About];
        let item_h = 38.0;
        let top = 20.0 + 4.0;
        for (index, page) in pages.iter().enumerate() {
            let y = top + index as f32 * (item_h + 2.0);
            let selected = self.page == *page;
            if selected {
                let pill = painter.brush(self.palette.layer).unwrap();
                painter
                    .fill_rounded_rect(
                        crate::d2d::rect(8.0, y, sidebar_w - 16.0, item_h),
                        radius::CONTROL as f32,
                        &pill,
                    );
                let indicator = painter.brush(self.palette.accent).unwrap();
                painter
                    .fill_rounded_rect(
                        crate::d2d::rect(8.0, y + 8.0, 3.0, item_h - 16.0),
                        1.5,
                        &indicator,
                    );
            }
            let text_color = if selected {
                self.palette.text
            } else {
                self.palette.text_secondary
            };
            let glyph_brush = painter.brush(text_color).unwrap();
            painter.text(
                page.glyph(),
                crate::d2d::rect(22.0, y, 20.0, item_h),
                14.0,
                windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL,
                &glyph_brush,
                1,
                1,
            );
            let label_brush = painter.brush(text_color).unwrap();
            painter.text(
                page.label(),
                crate::d2d::rect(50.0, y, sidebar_w - 60.0, item_h),
                13.0,
                windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL,
                &label_brush,
                0,
                1,
            );
        }
    }

    fn render_caption(&self, painter: &crate::d2d::Painter) {
        let scale = self.dpi as f32;
        let caption_h = CAPTION_HEIGHT as f32 / scale;
        let client_w = self.client.0 as f32 / scale;

        // Title, on the caption's left — inset past the sidebar.
        let brush = painter.brush(self.palette.text_secondary).unwrap();
        painter.text(
            "Settings",
            crate::d2d::rect(SIDEBAR_WIDTH as f32 / scale + 12.0, 0.0, 200.0, caption_h),
            12.0,
            windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL,
            &brush,
            0,
            1,
        );

        // Caption buttons: minimize and close, 46px wide, hover states.
        for (index, (glyph, wide)) in [("\u{E921}", 46.0), ("\u{E8BB}", 46.0)].into_iter().enumerate() {
            let x = client_w - 2.0 * wide + index as f32 * wide;
            let hovered = self
                .hover
                .map(|(hx, hy)| {
                    let ux = hx as f32 / scale;
                    let uy = hy as f32 / scale;
                    ux >= x && ux <= x + wide && uy <= caption_h
                })
                .unwrap_or(false);
            let is_close = index == 1;
            if hovered {
                let fill = painter
                    .brush(if is_close {
                        self.palette.caption_close_hover
                    } else {
                        self.palette.caption_hover
                    })
                    .unwrap();
                unsafe {
                    painter.rt.FillRectangle(
                        &crate::d2d::rect(x, 0.0, wide, caption_h),
                        &fill,
                    );
                }
            }
            let glyph_brush = painter.brush(self.palette.text).unwrap();
            painter.text(
                glyph,
                crate::d2d::rect(x, 0.0, wide, caption_h),
                10.0,
                windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL,
                &glyph_brush,
                1,
                1,
            );
        }
    }

    fn render_widgets(&mut self, painter: &crate::d2d::Painter) {
        let scale = self.dpi as f32;
        for (index, rect) in self.layout.clone() {
            let Some(widget) = self.widgets.get(index) else {
                continue;
            };
            // The scroll offset is already folded into the layout rects;
            // widgets outside the client are skipped, not clipped.
            if rect.bottom < CAPTION_HEIGHT || rect.top > self.client.1 {
                continue;
            }
            let x = rect.left as f32 / scale;
            let y = rect.top as f32 / scale;
            let w = (rect.right - rect.left) as f32 / scale;
            let h = (rect.bottom - rect.top) as f32 / scale;

            match widget {
                Widget::Title(text) => {
                    let brush = painter.brush(self.palette.text).unwrap();
                    painter.text(text, crate::d2d::rect(x, y, w, h), 22.0, windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_SEMI_BOLD, &brush, 0, 1);
                }
                Widget::Section(text) => {
                    let brush = painter.brush(self.palette.text_secondary).unwrap();
                    painter.text(text, crate::d2d::rect(x, y, w, h), 14.0, windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_SEMI_BOLD, &brush, 0, 1);
                }
                Widget::Toggle { label, detail, value, .. } => {
                    let label_brush = painter.brush(self.palette.text).unwrap();
                    painter.text(label, crate::d2d::rect(x, y + h * 0.14, w - 80.0, h * 0.5), 13.0, windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL, &label_brush, 0, 1);
                    if let Some(detail) = detail {
                        let detail_brush = painter.brush(self.palette.text_tertiary).unwrap();
                        painter.text(detail, crate::d2d::rect(x, y + h * 0.55, w - 80.0, h * 0.4), 11.5, windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL, &detail_brush, 0, 0);
                    }
                    // The Fluent toggle: a 40×20 pill, thumb 12.
                    let tw = 40.0f32;
                    let th = 20.0f32;
                    let tx = x + w - tw;
                    let ty = y + (h - th) / 2.0;
                    let (fill, stroke) = if *value {
                        (self.palette.accent, self.palette.accent)
                    } else {
                        (self.palette.control, self.palette.control_stroke)
                    };
                    let fill_brush = painter.brush(fill).unwrap();
                    painter.fill_rounded_rect(crate::d2d::rect(tx, ty, tw, th), th / 2.0, &fill_brush);
                    if !*value {
                        let stroke_brush = painter.brush(stroke).unwrap();
                        painter.draw_rounded_rect(crate::d2d::rect(tx, ty, tw, th), th / 2.0, &stroke_brush, 1.0);
                    }
                    let thumb_r = 5.0f32;
                    let thumb_x = if *value { tx + tw - 3.0 - thumb_r } else { tx + 3.0 + thumb_r };
                    let thumb_brush = painter.brush(if *value { self.palette.accent_text } else { self.palette.text_secondary }).unwrap();
                    painter.fill_ellipse(crate::d2d::point(thumb_x, ty + th / 2.0), thumb_r, &thumb_brush);
                }
                Widget::Choice { label, options, selected, .. } => {
                    let label_brush = painter.brush(self.palette.text).unwrap();
                    painter.text(label, crate::d2d::rect(x, y, w - 220.0, h), 13.0, windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL, &label_brush, 0, 1);
                    // The combo: 200 wide, right-aligned.
                    let cw = 200.0f32;
                    let cx = x + w - cw;
                    let cy = y + (h - CONTROL_HEIGHT as f32) / 2.0;
                    let fill = painter.brush(self.palette.control).unwrap();
                    painter.fill_rounded_rect(crate::d2d::rect(cx, cy, cw, CONTROL_HEIGHT as f32), radius::CONTROL as f32, &fill);
                    let stroke = painter.brush(self.palette.control_stroke).unwrap();
                    painter.draw_rounded_rect(crate::d2d::rect(cx, cy, cw, CONTROL_HEIGHT as f32), radius::CONTROL as f32, &stroke, 1.0);
                    let value = options.get(*selected).cloned().unwrap_or_default();
                    let value_brush = painter.brush(self.palette.text).unwrap();
                    painter.text(&value, crate::d2d::rect(cx + 10.0, cy, cw - 34.0, CONTROL_HEIGHT as f32), 12.5, windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL, &value_brush, 0, 1);
                    let chevron_brush = painter.brush(self.palette.text_secondary).unwrap();
                    painter.text("\u{E70D}", crate::d2d::rect(cx + cw - 26.0, cy, 20.0, CONTROL_HEIGHT as f32), 10.0, windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL, &chevron_brush, 1, 1);
                }
                Widget::Text { id, label, value, masked, hint } => {
                    let label_brush = painter.brush(self.palette.text_secondary).unwrap();
                    painter.text(label, crate::d2d::rect(x, y, w, 16.0), 11.5, windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL, &label_brush, 0, 0);
                    let field_h = CONTROL_HEIGHT as f32;
                    let fy = y + 18.0;
                    let focused = self.focused_text == Some(*id);
                    let stored = self.text_values.borrow().get(id).cloned().unwrap_or_else(|| value.clone());
                    let shown = if *masked && !focused {
                        "*".repeat(stored.chars().count().min(40))
                    } else {
                        stored.clone()
                    };
                    let fill = painter.brush(if focused { self.palette.control_hover } else { self.palette.control }).unwrap();
                    painter.fill_rounded_rect(crate::d2d::rect(x, fy, w, field_h), radius::CONTROL as f32, &fill);
                    let stroke_color = if focused { self.palette.accent } else { self.palette.control_stroke };
                    let stroke = painter.brush(stroke_color).unwrap();
                    painter.draw_rounded_rect(crate::d2d::rect(x, fy, w, field_h), radius::CONTROL as f32, &stroke, if focused { 2.0 } else { 1.0 });
                    let value_brush = painter.brush(if stored.is_empty() { self.palette.text_tertiary } else { self.palette.text }).unwrap();
                    let placeholder = if stored.is_empty() { paste_hint(label) } else { shown };
                    painter.text(&placeholder, crate::d2d::rect(x + 10.0, fy, w - 20.0, field_h), 12.5, windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL, &value_brush, 0, 1);
                    if let Some(hint) = hint {
                        let hint_brush = painter.brush(self.palette.text_tertiary).unwrap();
                        painter.text(hint, crate::d2d::rect(x, fy + field_h + 2.0, w, 14.0), 11.0, windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL, &hint_brush, 0, 0);
                    }
                }
                Widget::Button { label, primary, id } => {
                    let bw = 120.0f32;
                    let by = y + (h - CONTROL_HEIGHT as f32) / 2.0;
                    let hovered = self
                        .hover
                        .map(|(hx, hy)| {
                            let ux = hx as f32 / scale;
                            let uy = hy as f32 / scale;
                            ux >= x && ux <= x + bw && uy >= by && uy <= by + CONTROL_HEIGHT as f32
                        })
                        .unwrap_or(false);
                    let (fill, text_color) = if *primary {
                        (if hovered { self.palette.accent.with_alpha(0.9) } else { self.palette.accent }, self.palette.accent_text)
                    } else {
                        (if hovered { self.palette.control_hover } else { self.palette.control }, self.palette.text)
                    };
                    let fill_brush = painter.brush(fill).unwrap();
                    painter.fill_rounded_rect(crate::d2d::rect(x, by, bw, CONTROL_HEIGHT as f32), radius::CONTROL as f32, &fill_brush);
                    if !*primary {
                        let stroke = painter.brush(self.palette.control_stroke).unwrap();
                        painter.draw_rounded_rect(crate::d2d::rect(x, by, bw, CONTROL_HEIGHT as f32), radius::CONTROL as f32, &stroke, 1.0);
                    }
                    let label_brush = painter.brush(text_color).unwrap();
                    let _ = id;
                    painter.text(label, crate::d2d::rect(x, by, bw, CONTROL_HEIGHT as f32), 12.5, windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL, &label_brush, 1, 1);
                }
                Widget::Note(text) => {
                    let brush = painter.brush(self.palette.text_tertiary).unwrap();
                    self.draw_note(painter, text, x, y, w, h);
                    let _ = brush;
                }
                Widget::Status(text) => {
                    if text.is_empty() {
                        continue;
                    }
                    let brush = painter.brush(self.palette.text_secondary).unwrap();
                    painter.text(text, crate::d2d::rect(x, y, w, h), 12.0, windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL, &brush, 0, 1);
                }
                Widget::Separator => {
                    let brush = painter.brush(self.palette.layer_stroke).unwrap();
                    painter.draw_line(
                        crate::d2d::point(x, y + h / 2.0),
                        crate::d2d::point(x + w, y + h / 2.0),
                        &brush,
                        1.0,
                    );
                }
                Widget::Gap(_) => {}
            }
        }
    }

    fn draw_note(&self, painter: &crate::d2d::Painter, text: &str, x: f32, y: f32, w: f32, h: f32) {
        // Notes wrap; the row height budgets three lines.
        let mut line = String::new();
        let mut lines: Vec<String> = Vec::new();
        let chars_per_line = ((w / 6.4).floor() as usize).max(10);
        for word in text.split_whitespace() {
            let candidate = line.chars().count() + word.chars().count() + 1;
            if candidate > chars_per_line && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            lines.push(line);
        }
        let brush = painter.brush(self.palette.text_tertiary).unwrap();
        for (i, line) in lines.iter().take(3).enumerate() {
            painter.text(
                line,
                crate::d2d::rect(x, y + (h / 3.0) * i as f32, w, h / 3.0),
                11.5,
                windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL,
                &brush,
                0,
                0,
            );
        }
    }

    // --- Input -----------------------------------------------------------

    pub fn on_mouse_move(&mut self, x: i32, y: i32) {
        self.hover = Some((x, y));
        self.invalidate();
    }

    pub fn on_mouse_leave(&mut self) {
        self.hover = None;
        self.invalidate();
    }

    pub fn on_wheel(&mut self, delta: i32) {
        let max = (self.content_height - self.client.1 + CAPTION_HEIGHT).max(0);
        if max == 0 {
            return;
        }
        self.scroll = (self.scroll - delta / 4).clamp(-max, 0);
        self.relayout();
        self.invalidate();
    }

    pub fn on_l_button_down(&mut self, x: i32, y: i32) {
        let scale = self.dpi.max(0.5);
        let ux = x as f64 / scale;
        let uy = y as f64 / scale;

        // Caption buttons first: they live above everything.
        let caption_h = CAPTION_HEIGHT as f64 / scale;
        let client_w = self.client.0 as f64 / scale;
        if uy <= caption_h {
            if ux >= client_w - 46.0 {
                let _ = self.actions.send(SettingsAction::Close);
                return;
            }
            if ux >= client_w - 92.0 && ux < client_w - 46.0 {
                unsafe {
                    let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
                }
                return;
            }
            if ux <= SIDEBAR_WIDTH as f64 {
                return; // drag area; HTCAPTION handles it
            }
        }

        // Sidebar navigation.
        if ux <= SIDEBAR_WIDTH as f64 {
            let pages = [Page::General, Page::Accounts, Page::Notifications, Page::About];
            let index = ((uy - 24.0) / 40.0).floor();
            if index >= 0.0 && (index as usize) < pages.len() {
                self.page = pages[index as usize];
                self.account_focus = None;
                self.scroll = 0;
                self.rebuild_widgets();
                self.invalidate();
            }
            return;
        }

        // Widgets, top-down.
        for (index, rect) in self.layout.clone() {
            let Some(widget) = self.widgets.get(index) else {
                continue;
            };
            let hit = ux >= rect.left as f64
                && ux <= rect.right as f64
                && uy >= rect.top as f64
                && uy <= rect.bottom as f64;
            if !hit {
                continue;
            }
            match widget {
                Widget::Toggle { id, .. } => {
                    self.handle_toggle(*id);
                    return;
                }
                Widget::Choice { id, options, .. } => {
                    self.show_choice_popup(*id, rect, options.clone());
                    return;
                }
                Widget::Text { id, .. } => {
                    self.focused_text = Some(*id);
                    self.invalidate();
                    unsafe {
                        let _ = SetFocus(Some(self.hwnd));
                    }
                    return;
                }
                Widget::Button { id, .. } => {
                    self.handle_button(*id);
                    return;
                }
                _ => {}
            }
        }
        self.focused_text = None;
        self.invalidate();
    }

    fn handle_toggle(&mut self, id: u32) {
        pulse_core::settings::mutate(|s| {
            if id == ids::TOGGLE_SIDE_PCT {
                s.side_rail_shows_percentages = !s.side_rail_shows_percentages;
            } else if id == ids::TOGGLE_TOP_PCT {
                s.top_rail_shows_percentages = !s.top_rail_shows_percentages;
            } else if id == ids::TOGGLE_LABEL_ABOVE {
                s.label_above_ring = !s.label_above_ring;
            } else if id == ids::TOGGLE_REMAINING {
                s.shows_remaining = !s.shows_remaining;
            } else if id == ids::TOGGLE_CLOCK {
                s.shows_window_clock = !s.shows_window_clock;
            } else if id == ids::TOGGLE_SECOND_RING {
                s.shows_second_ring = !s.shows_second_ring;
            } else if id == ids::TOGGLE_FORECAST {
                s.shows_forecast = !s.shows_forecast;
            } else if id == ids::TOGGLE_AUTO_COLLAPSE {
                s.auto_collapse = !s.auto_collapse;
            } else if id == ids::TOGGLE_FOLLOW_DISPLAY {
                s.follows_active_display = !s.follows_active_display;
            } else if id == ids::TOGGLE_ALERTS {
                s.wants_alerts = !s.wants_alerts;
            } else if id == ids::TOGGLE_ALERT_RESET {
                s.alerts_on_reset = !s.alerts_on_reset;
            } else if id == ids::TOGGLE_ALERT_FAILURE {
                s.alerts_on_failure = !s.alerts_on_failure;
            } else if id >= ids::PROVIDER_TOGGLE {
                let index = (id - ids::PROVIDER_TOGGLE) as usize;
                if let Some(provider) = pulse_core::model::ALL_PROVIDERS.get(index) {
                    if s.enabled_accounts.contains(provider.raw()) {
                        s.enabled_accounts.remove(provider.raw());
                    } else {
                        s.enabled_accounts.insert(provider.raw().to_string());
                    }
                }
            }
        });
        if id == ids::TOGGLE_STARTUP {
            crate::autostart::set_enabled(!crate::autostart::is_enabled());
        }
        let _ = self.actions.send(SettingsAction::Changed);
        self.rebuild_widgets();
        self.invalidate();
    }

    fn show_choice_popup(&mut self, id: u32, rect: RECT, options: Vec<String>) {
        unsafe {
            let menu = match CreatePopupMenu() {
                Ok(menu) => menu,
                Err(_) => return,
            };
            for (index, option) in options.iter().enumerate() {
                let wide: Vec<u16> = option.encode_utf16().chain(std::iter::once(0)).collect();
                let _ = AppendMenuW(
                    menu,
                    MF_STRING,
                    index as usize,
                    windows::core::PCWSTR::from_raw(wide.as_ptr()),
                );
            }
            let mut pt = POINT { x: rect.left, y: rect.bottom };
            let _ = ClientToScreen(self.hwnd, &mut pt);
            let _ = SetForegroundWindow(self.hwnd);
            let chosen = TrackPopupMenuEx(
                menu,
                (TPM_RIGHTBUTTON | TPM_RETURNCMD).0 as u32,
                pt.x,
                pt.y,
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            let chosen_id = chosen.0.max(0) as usize;
            if chosen_id > 0 {
                self.apply_choice(id, chosen_id, &options);
            }
        }
    }

    fn apply_choice(&mut self, id: u32, selected: usize, options: &[String]) {
        let _ = options;
        pulse_core::settings::mutate(|s| {
            match id {
                ids::CHOICE_DOCK => {
                    match selected {
                        0 => {
                            s.floating = false;
                            s.dock_side = "right".into();
                        }
                        1 => {
                            s.floating = false;
                            s.dock_side = "left".into();
                        }
                        2 => {
                            s.floating = false;
                            s.dock_side = "top".into();
                        }
                        _ => s.floating = true,
                    }
                }
                ids::CHOICE_PANEL_SIZE => {
                    s.panel_size = match selected {
                        0 => "small".into(),
                        2 => "large".into(),
                        _ => "standard".into(),
                    };
                }
                ids::CHOICE_RAIL_SPACING => {
                    s.rail_spacing = match selected {
                        0 => "compact".into(),
                        2 => "roomy".into(),
                        _ => "standard".into(),
                    };
                }
                ids::CHOICE_INTERVAL => {
                    s.refresh_interval = match selected {
                        1 => 30,
                        2 => 60,
                        3 => 120,
                        4 => 300,
                        5 => 600,
                        6 => 1800,
                        _ => 0,
                    };
                }
                ids::CHOICE_ALERT_THRESHOLD => {
                    s.alert_threshold = [75, 80, 90, 95][selected.min(3)];
                }
                ids::CHOICE_LANGUAGE => {
                    s.language = match selected {
                        1 => "en".into(),
                        2 => "zh".into(),
                        _ => "auto".into(),
                    };
                    match s.language.as_str() {
                        "en" => pulse_core::localization::set_language(pulse_core::localization::Language::English),
                        "zh" => pulse_core::localization::set_language(pulse_core::localization::Language::Chinese),
                        _ => pulse_core::localization::detect_from_system(),
                    }
                }
                _ => {}
            }
        });
        let _ = self.actions.send(SettingsAction::Changed);
        self.rebuild_widgets();
        self.invalidate();
    }

    fn handle_button(&mut self, id: u32) {
        if id == ids::BUTTON_DIAGNOSTICS {
            return;
        }
        if id == 900 {
            let _ = self
                .actions
                .send(SettingsAction::OpenUrl("https://github.com/Lyx721188/Pulse".into()));
            return;
        }
        if id >= ids::PROVIDER_SIGNIN {
            let index = (id - ids::PROVIDER_SIGNIN) as usize;
            if let Some(provider) = pulse_core::model::ALL_PROVIDERS.get(index) {
                if pulse_core::secrets::key_for("copilot").is_some() && *provider == pulse_core::model::Provider::Copilot {
                    pulse_core::secrets::set_key("copilot", "");
                    let _ = self.actions.send(SettingsAction::SignOutCopilot);
                } else {
                    let _ = self.actions.send(SettingsAction::SignInCopilot);
                }
            }
            return;
        }
        if id >= ids::PROVIDER_SAVE {
            let index = (id - ids::PROVIDER_SAVE) as usize;
            if let Some(provider) = pulse_core::model::ALL_PROVIDERS.get(index) {
                let value = self
                    .text_values
                    .borrow()
                    .get(&(ids::PROVIDER_KEY + index as u32))
                    .cloned()
                    .unwrap_or_default();
                pulse_core::secrets::set_key(provider.raw(), value.trim());
                let _ = self.actions.send(SettingsAction::SaveKey(provider.raw().to_string(), value.trim().to_string()));
            }
            return;
        }
        if id >= ids::PROVIDER_REFRESH {
            let index = (id - ids::PROVIDER_REFRESH) as usize;
            if let Some(provider) = pulse_core::model::ALL_PROVIDERS.get(index) {
                let _ = self
                    .actions
                    .send(SettingsAction::RefreshProvider(provider.raw().to_string()));
            }
        }
    }

    pub fn on_char(&mut self, ch: u16) {
        let Some(id) = self.focused_text else {
            return;
        };
        let mut map = self.text_values.borrow_mut();
        let entry = map.entry(id).or_default();
        match ch {
            0x08 => {
                entry.pop();
            }
            0x0D | 0x1B => {
                self.focused_text = None;
            }
            c if c >= 0x20 => {
                if let Some(decoded) = char::from_u32(c as u32) {
                    entry.push(decoded);
                }
            }
            _ => {}
        }
        self.invalidate();
    }

    /// External state (store readings) landed; the status lines refresh.
    pub fn set_status(&mut self, provider_raw: &str, text: String) {
        self.status_cache.insert(provider_raw.to_string(), text);
        if self.page == Page::Accounts {
            self.rebuild_widgets();
            self.invalidate();
        }
    }

    pub fn close(&mut self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }
}

fn paste_hint(label: &str) -> String {
    // The empty field hints at what goes in it.
    match label {
        "API key" => "sk-…".to_string(),
        other => other.to_string(),
    }
}

unsafe extern "system" fn settings_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if state == 0 {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let window = &mut *(state as *mut SettingsWindow);

    match msg {
        WM_SIZE => {
            window.on_size();
            LRESULT(0)
        }
        WM_PAINT => {
            window.render();
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as u16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i32;
            window.on_mouse_move(x, y);
            track_leave(hwnd);
            LRESULT(0)
        }
        _WM_MOUSELEAVE => {
            window.on_mouse_leave();
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            window.on_wheel(delta);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as u16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i32;
            window.on_l_button_down(x, y);
            LRESULT(0)
        }
        WM_CHAR => {
            window.on_char(wparam.0 as u16);
            LRESULT(0)
        }
        WM_NCCALCSIZE => {
            if wparam.0 != 0 {
                // No standard frame: the window draws its own caption.
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_NCHITTEST => {
            let x = (lparam.0 & 0xFFFF) as u16 as i32 as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i32 as i16 as i32;
            let mut rect = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rect);
            let local_x = x - rect.left;
            let local_y = y - rect.top;
            // The resizable border, then the caption. The caption row hands
            // its buttons to the system, which does the rest.
            let scale = window.dpi.max(0.5);
            let edge = 8.0 * scale;
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            if local_y >= height - edge as i32 {
                if local_x <= edge as i32 {
                    return LRESULT(HTBOTTOMLEFT as isize);
                }
                if local_x >= width - edge as i32 {
                    return LRESULT(HTBOTTOMRIGHT as isize);
                }
                return LRESULT(HTBOTTOM as isize);
            }
            if local_x <= edge as i32 {
                return LRESULT(HTLEFT as isize);
            }
            if local_x >= width - edge as i32 {
                return LRESULT(HTRIGHT as isize);
            }
            if local_y <= CAPTION_HEIGHT {
                let ux = local_x as f64 / scale;
                let client_w = width as f64 / scale;
                if ux >= client_w - 92.0 {
                    if ux >= client_w - 46.0 {
                        return LRESULT(HTCLOSE as isize);
                    }
                    return LRESULT(HTMINBUTTON as isize);
                }
                return LRESULT(HTCAPTION as isize);
            }
            LRESULT(HTCLIENT as isize)
        }
        WM_COMMAND => LRESULT(0),
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn track_leave(hwnd: HWND) {
    unsafe {
        let mut tme = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        let _ = TrackMouseEvent(&mut tme);
    }
}

/// The caption's colour reference, for the DWM default titlebar fallback.
#[allow(dead_code)]
fn caption_color(palette: &SettingsPalette) -> COLORREF {
    COLORREF(((palette.background.b * 255.0) as u32)
        | (((palette.background.g * 255.0) as u32) << 8)
        | (((palette.background.r * 255.0) as u32) << 16))
}
