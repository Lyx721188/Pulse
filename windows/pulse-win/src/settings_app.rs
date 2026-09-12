//! The settings window, on **Windows Reactor** — Microsoft's official
//! declarative WinUI 3 library for Rust, shipped in `windows-rs` (May 2026).
//!
//! The first Windows build drew its own controls with Direct2D; this one
//! uses the real WinUI 3 controls — `NavigationView`, `ToggleSwitch`,
//! `ComboBox`, `PasswordBox` — so the look and the accessibility tree are
//! WinUI's own.
//!
//! The window lives on its own thread with its own Reactor host, opened on
//! demand and gone when closed. The app pushes per-provider status lines
//! into a shared snapshot; a background poller wakes the component when it
//! changes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows_reactor::*;
use windows_reactor::SlotsControl;

use pulse_core::model::Provider;

/// Messages the settings window sends the app. The app owns the panel, the
/// store and the tray; the window only describes what the user asked for.
pub enum SettingsAction {
    /// A setting changed; the app re-reads everything it drives.
    Changed,
    RefreshProvider(String),
    SaveKey(String, String),
    SignInCopilot,
    SignOutCopilot,
    OpenUrl(String),
}

/// What the app pushes to the window while it is open. The poller watches
/// the generation counter and wakes the component when it moves.
#[derive(Default)]
struct SettingsSnapshot {
    generation: u64,
    status: HashMap<String, String>,
}

pub(crate) struct Shared {
    snapshot: Mutex<SettingsSnapshot>,
    actions: Sender<SettingsAction>,
    /// One settings window at a time: claimed by `show`, released when the
    /// window closes.
    alive: AtomicBool,
}

impl Shared {
    fn generation(&self) -> u64 {
        self.snapshot.lock().unwrap().generation
    }
    fn take_status(&self) -> HashMap<String, String> {
        std::mem::take(&mut self.snapshot.lock().unwrap().status)
    }
    fn send(&self, action: SettingsAction) {
        let _ = self.actions.send(action);
    }
}

/// The app's handle on the settings window. `show` asks the main thread —
/// where the Reactor host lives — to open it; the window is one at a time.
pub struct SettingsHost {
    shared: Arc<Shared>,
    open_tx: Sender<()>,
}

impl SettingsHost {
    pub fn new(actions: Sender<SettingsAction>, open_tx: Sender<()>) -> Self {
        SettingsHost {
            shared: Arc::new(Shared {
                snapshot: Mutex::new(SettingsSnapshot::default()),
                actions,
                alive: AtomicBool::new(false),
            }),
            open_tx,
        }
    }

    /// The handle the main thread needs to serve the window.
    pub(crate) fn shared(&self) -> Arc<Shared> {
        self.shared.clone()
    }

    /// One provider's status line for the Accounts page.
    pub fn set_status(&self, provider_raw: &str, text: String) {
        let mut snapshot = self.shared.snapshot.lock().unwrap();
        snapshot.status.insert(provider_raw.to_string(), text);
        snapshot.generation += 1;
    }

    /// Something the window shows has changed out from under it.
    pub fn refresh(&self) {
        self.shared.snapshot.lock().unwrap().generation += 1;
    }

    pub fn is_open(&self) -> bool {
        self.shared.alive.load(Ordering::SeqCst)
    }

    /// Opens the window, unless it is already open.
    pub fn show(&mut self) {
        if !self.shared.alive.swap(true, Ordering::SeqCst) {
            let _ = self.open_tx.send(());
        }
    }
}

/// Mounts one settings window on the calling (main) thread and pumps it
/// until the user closes it.
pub(crate) fn serve_once(shared: &Arc<Shared>) {
    SHARED.with(|cell| *cell.borrow_mut() = Some(shared.clone()));
    let _ = App::run_component::<SettingsApp>(());
    shared.alive.store(false, Ordering::SeqCst);
}

thread_local! {
    static SHARED: std::cell::RefCell<Option<Arc<Shared>>> =
        const { std::cell::RefCell::new(None) };
}

// --- The component ----------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    General,
    Accounts,
    Notifications,
    About,
}

impl Page {
    fn tag(self) -> &'static str {
        match self {
            Page::General => "general",
            Page::Accounts => "accounts",
            Page::Notifications => "notifications",
            Page::About => "about",
        }
    }
    fn from_tag(tag: &str) -> Page {
        match tag {
            "accounts" => Page::Accounts,
            "notifications" => Page::Notifications,
            "about" => Page::About,
            _ => Page::General,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Accounts => "Accounts",
            Page::Notifications => "Notifications",
            Page::About => "About",
        }
    }
    fn glyph(self) -> &'static str {
        match self {
            Page::General => "\u{E713}",
            Page::Accounts => "\u{E77B}",
            Page::Notifications => "\u{EA8F}",
            Page::About => "\u{E946}",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToggleKey {
    SidePct,
    Remaining,
    Clock,
    SecondRing,
    Forecast,
    AutoCollapse,
    FollowDisplay,
    Startup,
    Alerts,
    AlertReset,
    AlertFailure,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChoiceKey {
    Dock,
    PanelSize,
    RailSpacing,
    Interval,
    AlertThreshold,
}

#[derive(Clone, PartialEq)]
enum Message {
    Tick,
    Nav(Option<String>),
    Toggle(ToggleKey, bool),
    ToggleEnabled(usize, bool),
    Choice(ChoiceKey, Option<usize>),
    KeyEdit(usize, String),
    Save(usize),
    Refresh(usize),
    CopilotAuth,
    OpenGitHub,
    Scheme(ColorScheme),
}

struct SettingsApp {
    shared: Arc<Shared>,
    page: Page,
    dark: bool,
    /// Draft key text per provider raw, until Saved.
    keys: HashMap<String, String>,
    /// Status lines pushed by the app, per provider raw.
    status: HashMap<String, String>,
    poll: Option<ComponentTask>,
}

impl Component for SettingsApp {
    type Input = ();
    type Message = Message;

    fn create(_input: &(), context: &ComponentContext<Self>) -> Self {
        let shared = SHARED.with(|cell| cell.borrow().clone().expect("settings shared state"));
        SettingsApp {
            shared: shared.clone(),
            page: Page::General,
            dark: false,
            keys: HashMap::new(),
            status: HashMap::new(),
            poll: Some(Self::spawn_watcher(&shared, context)),
        }
    }

    fn update(&mut self, message: Message, context: &ComponentContext<Self>) {
        match message {
            Message::Tick => {
                self.status = self.shared.take_status();
            }
            Message::Scheme(scheme) => self.dark = scheme == ColorScheme::Dark,
            Message::Nav(tag) => {
                self.page = tag.as_deref().map(Page::from_tag).unwrap_or(self.page);
            }
            Message::Toggle(key, value) => {
                Self::apply_toggle(key, value);
                self.shared.send(SettingsAction::Changed);
            }
            Message::ToggleEnabled(index, value) => {
                if let Some(provider) = all_providers().get(index) {
                    let raw = provider.raw();
                    pulse_core::settings::mutate(move |s| {
                        if value {
                            s.enabled_accounts.insert(raw.to_string());
                        } else {
                            s.enabled_accounts.remove(raw);
                        }
                    });
                    self.shared.send(SettingsAction::Changed);
                }
            }
            Message::Choice(key, selected) => {
                Self::apply_choice(key, selected.unwrap_or(0));
                self.shared.send(SettingsAction::Changed);
            }
            Message::KeyEdit(index, text) => {
                if let Some(provider) = all_providers().get(index) {
                    self.keys.insert(provider.raw().to_string(), text);
                }
            }
            Message::Save(index) => {
                if let Some(provider) = all_providers().get(index) {
                    let value = self.keys.get(provider.raw()).cloned().unwrap_or_default();
                    pulse_core::secrets::set_key(provider.raw(), value.trim());
                    self.shared.send(SettingsAction::SaveKey(
                        provider.raw().to_string(),
                        value.trim().to_string(),
                    ));
                }
            }
            Message::Refresh(index) => {
                if let Some(provider) = all_providers().get(index) {
                    self.shared
                        .send(SettingsAction::RefreshProvider(provider.raw().to_string()));
                }
            }
            Message::CopilotAuth => {
                if pulse_core::secrets::key_for("copilot").is_some() {
                    pulse_core::secrets::set_key("copilot", "");
                    self.shared.send(SettingsAction::SignOutCopilot);
                } else {
                    self.shared.send(SettingsAction::SignInCopilot);
                }
            }
            Message::OpenGitHub => {
                self.shared.send(SettingsAction::OpenUrl(
                    "https://github.com/Lyx721188/Pulse".into(),
                ));
            }
        }
        // Re-arm the watcher whichever way this update came, so the next
        // push from the app wakes us again.
        self.poll = Some(Self::spawn_watcher(&self.shared, context));
    }

    fn view(&self, _input: &(), context: &mut ViewContext<Self>) -> View {
        let nav = NavigationView::new()
            .pane_title("Pulse")
            .on_selected_tag_changed(context.callback(Message::Nav))
            .slots([
                SlotView::collection(
                    NavigationViewSlot::MenuItems,
                    [
                        Page::General,
                        Page::Accounts,
                        Page::Notifications,
                        Page::About,
                    ]
                    .map(|page| {
                        (
                            page.tag(),
                            NavigationViewItem::new()
                                .tag(page.tag())
                                .is_selected(self.page == page)
                                .slots([
                                    SlotView::new(
                                        NavigationViewItemSlot::Content,
                                        TextBlock::new()
                                            .text(pulse_core::localization::t(page.label())),
                                    ),
                                    SlotView::new(
                                        NavigationViewItemSlot::Icon,
                                        FontIcon::new().glyph(page.glyph()),
                                    ),
                                ]),
                        )
                    }),
                ),
                SlotView::new(
                    NavigationViewSlot::Content,
                    ScrollViewer::new().content(
                        Border::new()
                            .padding(Thickness::uniform(28.0))
                            .content(match self.page {
                                Page::General => self.general_view(context).into(),
                                Page::Accounts => self.accounts_view(context),
                                Page::Notifications => self.notifications_view(context),
                                Page::About => self.about_view(context),
                            }),
                    ),
                ),
            ]);
        context.window_visuals(WindowVisuals::new().backdrop(WindowBackdrop::Mica));
        context.window_title("Pulse Settings");
        nav.into()
    }
}

fn all_providers() -> &'static [Provider] {
    &pulse_core::model::ALL_PROVIDERS
}

/// A row of siblings: the closest thing to the missing `hstack`.
fn row(children: impl IntoViews) -> View {
    StackPanel::new()
        .orientation(Orientation::Horizontal)
        .spacing(12.0)
        .children(children)
}

fn heading(text: &str) -> TextBlock {
    TextBlock::new()
        .text(pulse_core::localization::t(text))
        .font_size(24.0)
        .font_weight(FontWeight::BOLD)
}

fn section(text: &str) -> TextBlock {
    TextBlock::new()
        .text(pulse_core::localization::t(text))
        .font_size(16.0)
        .font_weight(FontWeight::SEMI_BOLD)
}

impl SettingsApp {
    fn spawn_watcher(shared: &Arc<Shared>, context: &ComponentContext<Self>) -> ComponentTask {
        let watcher = shared.clone();
        context.spawn_background(move |_| {
            // Sleep until the app pushes something new, then wake the
            // component. `seen` starts at the generation observed now, so
            // changes that landed during the previous dispatch are caught.
            let seen = AtomicU64::new(watcher.generation());
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let generation = watcher.generation();
                if generation != seen.load(Ordering::SeqCst) {
                    seen.store(generation, Ordering::SeqCst);
                    return Message::Tick;
                }
            }
        })
    }

    /// Muted text: theme-aware, because WinUI follows the system theme.
    fn muted(&self, text: &str) -> TextBlock {
        let gray = if self.dark { 255 } else { 0 };
        TextBlock::new()
            .text(text)
            .text_wrapping(TextWrapping::Wrap)
            .font_size(13.0)
            .foreground(Brush::Solid(Color::argb(160, gray, gray, gray)))
    }

    fn apply_toggle(key: ToggleKey, value: bool) {
        pulse_core::settings::mutate(|s| match key {
            ToggleKey::SidePct => s.side_rail_shows_percentages = value,
            ToggleKey::Remaining => s.shows_remaining = value,
            ToggleKey::Clock => s.shows_window_clock = value,
            ToggleKey::SecondRing => s.shows_second_ring = value,
            ToggleKey::Forecast => s.shows_forecast = value,
            ToggleKey::AutoCollapse => s.auto_collapse = value,
            ToggleKey::FollowDisplay => s.follows_active_display = value,
            ToggleKey::Alerts => s.wants_alerts = value,
            ToggleKey::AlertReset => s.alerts_on_reset = value,
            ToggleKey::AlertFailure => s.alerts_on_failure = value,
            ToggleKey::Startup => {}
        });
        if key == ToggleKey::Startup {
            crate::autostart::set_enabled(value);
        }
    }

    fn apply_choice(key: ChoiceKey, selected: usize) {
        pulse_core::settings::mutate(|s| match key {
            ChoiceKey::Dock => match selected {
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
            },
            ChoiceKey::PanelSize => {
                s.panel_size = match selected {
                    0 => "small".into(),
                    2 => "large".into(),
                    _ => "standard".into(),
                };
            }
            ChoiceKey::RailSpacing => {
                s.rail_spacing = match selected {
                    0 => "compact".into(),
                    2 => "roomy".into(),
                    _ => "standard".into(),
                };
            }
            ChoiceKey::Interval => {
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
            ChoiceKey::AlertThreshold => {
                s.alert_threshold = match selected {
                    1 => 80,
                    2 => 90,
                    3 => 95,
                    _ => 75,
                };
            }
        });
    }

    fn toggle(
        &self,
        key: ToggleKey,
        label: &str,
        on: bool,
        context: &ViewContext<Self>,
    ) -> View {
        row((
            TextBlock::new().text(pulse_core::localization::t(label)),
            ToggleSwitch::new()
                .is_on(on)
                .on_toggled(context.callback(move |on| Message::Toggle(key, on))),
        ))
    }

    fn choice(
        &self,
        key: ChoiceKey,
        label: &str,
        options: &[&str],
        selected: usize,
        context: &ViewContext<Self>,
    ) -> View {
        row((
            TextBlock::new().text(pulse_core::localization::t(label)),
            ComboBox::new()
                .items_source(options.iter().map(|o| pulse_core::localization::t(o)))
                .selected_index(selected)
                .on_selection_changed(context.callback(move |index| Message::Choice(key, index))),
        ))
    }

    fn general_view(&self, context: &ViewContext<Self>) -> View {
        let s = pulse_core::settings::with(|s| s.clone());
        let dock_selected = if s.floating {
            3
        } else {
            match s.dock_side.as_str() {
                "left" => 1,
                "top" => 2,
                _ => 0,
            }
        };
        let size_selected = match s.panel_size.as_str() {
            "small" => 0,
            "large" => 2,
            _ => 1,
        };
        let spacing_selected = match s.rail_spacing.as_str() {
            "compact" => 0,
            "roomy" => 2,
            _ => 1,
        };
        let interval_selected = match s.refresh_interval {
            30 => 1,
            60 => 2,
            120 => 3,
            300 => 4,
            600 => 5,
            1800 => 6,
            _ => 0,
        };
        StackPanel::new()
            .spacing(10.0)
            .children((
                heading("General"),
                section("Panel"),
                self.choice(
                    ChoiceKey::Dock,
                    "Dock to",
                    &["Right", "Left", "Top", "Floating"],
                    dock_selected,
                    context,
                ),
                self.choice(
                    ChoiceKey::PanelSize,
                    "Panel size",
                    &["Small", "Standard", "Large"],
                    size_selected,
                    context,
                ),
                self.choice(
                    ChoiceKey::RailSpacing,
                    "Ring spacing",
                    &["Tight", "Standard", "Loose"],
                    spacing_selected,
                    context,
                ),
                self.toggle(
                    ToggleKey::SidePct,
                    "Show percent labels",
                    s.side_rail_shows_percentages,
                    context,
                ),
                self.toggle(
                    ToggleKey::Remaining,
                    "Show what's left",
                    s.shows_remaining,
                    context,
                ),
                self.toggle(
                    ToggleKey::Clock,
                    "Show window clock",
                    s.shows_window_clock,
                    context,
                ),
                self.toggle(
                    ToggleKey::SecondRing,
                    "Show second ring",
                    s.shows_second_ring,
                    context,
                ),
                self.toggle(
                    ToggleKey::Forecast,
                    "Show forecast",
                    s.shows_forecast,
                    context,
                ),
                self.toggle(
                    ToggleKey::AutoCollapse,
                    "Auto-collapse when idle",
                    s.auto_collapse,
                    context,
                ),
                self.toggle(
                    ToggleKey::FollowDisplay,
                    "Follow the active display",
                    s.follows_active_display,
                    context,
                ),
                section("Refresh"),
                self.choice(
                    ChoiceKey::Interval,
                    "Refresh interval",
                    &["Automatic", "30s", "1min", "2min", "5min", "10min", "30min"],
                    interval_selected,
                    context,
                ),
                section("Windows"),
                self.toggle(
                    ToggleKey::Startup,
                    "Launch at startup",
                    crate::autostart::is_enabled(),
                    context,
                ),
            ))
            .into()
    }

    fn provider_row(
        &self,
        index: usize,
        provider: Provider,
        context: &ViewContext<Self>,
    ) -> View {
        let raw = provider.raw();
        let enabled = pulse_core::settings::with(|s| s.enabled_accounts.contains(raw));

        // A route this port does not reach is named, not shown broken.
        if !provider.is_ported_to_windows() {
            return StackPanel::new()
                .spacing(4.0)
                .children((
                    TextBlock::new()
                        .text(provider.display_name())
                        .font_size(15.0)
                        .font_weight(FontWeight::SEMI_BOLD),
                    self.muted(provider.windows_gap().unwrap_or("Not available on Windows.")),
                ))
                .into();
        }

        let status_line = self.status.get(raw).cloned();
        let key_draft = self.keys.get(raw).cloned();

        let header = row((
            TextBlock::new()
                .text(provider.display_name())
                .font_size(15.0)
                .font_weight(FontWeight::SEMI_BOLD),
            ToggleSwitch::new()
                .is_on(enabled)
                .on_toggled(context.callback(move |on| Message::ToggleEnabled(index, on))),
        ));

        let body: View = match provider {
            Provider::ClaudeCode | Provider::Codex | Provider::Grok => self
                .muted(&pulse_core::localization::t(
                    "Reads the login this tool already saved on this PC.",
                ))
                .into(),
            Provider::Copilot => {
                let signed_in = pulse_core::secrets::key_for("copilot").is_some();
                StackPanel::new()
                    .spacing(6.0)
                    .children((
                        Button::new()
                            .on_click(context.message(Message::CopilotAuth))
                            .content(pulse_core::localization::t(if signed_in {
                                "Sign out"
                            } else {
                                "Sign in with GitHub"
                            })),
                        self.muted(&pulse_core::localization::t(
                            "Device-code sign-in. Requests read:user only — never your repositories. The consent page names the editor whose client it borrows; this is not an official integration.",
                        )),
                    ))
                    .into()
            }
            _ => {
                let stored = key_draft
                    .or_else(|| pulse_core::secrets::key_for(raw))
                    .unwrap_or_default();
                StackPanel::new()
                    .spacing(6.0)
                    .children((
                        TextBlock::new().text(pulse_core::localization::t("API key")),
                        PasswordBox::new()
                            .password(stored)
                            .placeholder_text(pulse_core::localization::t("Paste the API key"))
                            .on_password_changed(context.callback(move |text| {
                                Message::KeyEdit(index, text)
                            })),
                        row((
                            Button::new()
                                .on_click(context.message(Message::Save(index)))
                                .content(pulse_core::localization::t("Save")),
                            Button::new()
                                .on_click(context.message(Message::Refresh(index)))
                                .content(pulse_core::localization::t("Refresh")),
                        )),
                    ))
                    .into()
            }
        };

        let status: View = match status_line {
            Some(text) => TextBlock::new().text(text).into(),
            None => View::empty(),
        };

        StackPanel::new()
            .spacing(6.0)
            .children((header, body, status))
            .into()
    }

    fn accounts_view(&self, context: &ViewContext<Self>) -> View {
        // Keyed by provider raw, so rows keep their identity as the list
        // reconciles. `keyed_children` finishes the builder, so the header
        // rows ride the same keyed list under reserved keys.
        let note = self.muted(&pulse_core::localization::t(
            "Each provider reports its own figures by the route that product offers — Pulse holds no account of its own and sends nothing anywhere but to the provider you already use.",
        ));
        let rows = std::iter::once(("__heading", heading("Accounts").into()))
            .chain(std::iter::once(("__note", note.into())))
            .chain(
                all_providers()
                    .iter()
                    .enumerate()
                    .map(|(index, provider)| {
                        (provider.raw(), self.provider_row(index, *provider, context))
                    }),
            );
        StackPanel::new().spacing(14.0).keyed_children(rows)
    }

    fn notifications_view(&self, context: &ViewContext<Self>) -> View {
        let s = pulse_core::settings::with(|s| s.clone());
        let threshold_selected = match s.alert_threshold {
            80 => 1,
            90 => 2,
            95 => 3,
            _ => 0,
        };
        let follow_ups: View = if s.wants_alerts {
            StackPanel::new()
                .spacing(10.0)
                .children((
                    self.choice(
                        ChoiceKey::AlertThreshold,
                        "Warn me when a limit passes",
                        &["75%", "80%", "90%", "95%"],
                        threshold_selected,
                        context,
                    ),
                    self.toggle(
                        ToggleKey::AlertReset,
                        "when a warned window comes back",
                        s.alerts_on_reset,
                        context,
                    ),
                    self.toggle(
                        ToggleKey::AlertFailure,
                        "when checks keep failing",
                        s.alerts_on_failure,
                        context,
                    ),
                ))
                .into()
        } else {
            View::empty()
        };
        StackPanel::new()
            .spacing(10.0)
            .children((
                heading("Notifications"),
                self.muted(&pulse_core::localization::t(
                    "All notifications are off until you turn them on.",
                )),
                self.toggle(
                    ToggleKey::Alerts,
                    "Enable notifications",
                    s.wants_alerts,
                    context,
                ),
                follow_ups,
            ))
            .into()
    }

    fn about_view(&self, context: &ViewContext<Self>) -> View {
        StackPanel::new()
            .spacing(10.0)
            .children((
                heading("Pulse"),
                self.muted(&pulse_core::localization::t(
                    "A screen-edge monitor for your AI coding allowances.",
                )),
                TextBlock::new().text(format!(
                    "{} 1.1.0 (Windows)",
                    pulse_core::localization::t("Version")
                )),
                self.muted(
                    "No Pulse servers, no Pulse account, no telemetry. Requests go to the providers you already use and follow the Windows system proxy settings.",
                ),
                Button::new()
                    .on_click(context.message(Message::OpenGitHub))
                    .content("github.com/Lyx721188/Pulse"),
            ))
            .into()
    }
}
