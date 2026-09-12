//! The orchestrator: three windows (panel, tray, settings), the store
//! worker, and the message loop that keeps them in step. Everything the
//! windows decide travels through one channel; everything the store
//! produces comes back through one poll.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};

use pulse_core::model::{Provider, ProviderUsage, State};
use pulse_core::store::{Command, StoreHandle, Update};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::panel::{PanelEvent, PanelWindow, RailEntry};
use crate::settings_ui::{SettingsAction, SettingsWindow};
use crate::tray::{TrayCommand, TrayIcon};

pub enum AppMsg {
    Tray(TrayCommand),
    Panel(PanelEvent),
    Settings(SettingsAction),
    StorePoll,
    DeviceFlowDone(Result<String, String>),
}

pub struct App {
    panel: Box<PanelWindow>,
    tray: Box<TrayIcon>,
    settings: Box<SettingsWindow>,
    store: StoreHandle,
    tx: Sender<AppMsg>,
    rx: Receiver<AppMsg>,
    readings: HashMap<String, ProviderUsage>,
    refreshing: std::collections::HashSet<String>,
}

impl App {
    pub fn start() -> App {
        let (tx, rx) = channel::<AppMsg>();

        // The windows speak their own vocabularies; forwarder threads fold
        // them into the one channel the loop listens to.
        let (panel_tx, panel_rx) = channel::<PanelEvent>();
        let (settings_tx, settings_rx) = channel::<SettingsAction>();
        let (tray_tx, tray_rx) = channel::<TrayCommand>();
        let (poll_tx, poll_rx) = channel::<()>();

        // Forwarders into the main channel.
        std::thread::spawn({
            let tx = tx.clone();
            move || {
            for event in panel_rx {
                if tx.send(AppMsg::Panel(event)).is_err() {
                    return;
                }
            }
        }
        });
        std::thread::spawn({
            let tx = tx.clone();
            move || {
            for action in settings_rx {
                if tx.send(AppMsg::Settings(action)).is_err() {
                    return;
                }
            }
        }
        });
        std::thread::spawn({
            let tx = tx.clone();
            move || {
            for command in tray_rx {
                if tx.send(AppMsg::Tray(command)).is_err() {
                    return;
                }
            }
        }
        });
        std::thread::spawn({
            let tx = tx.clone();
            move || {
            for tick in poll_rx {
                let _ = tick;
                if tx.send(AppMsg::StorePoll).is_err() {
                    return;
                }
            }
        }
        });

        // First-run resolution happens exactly once, at launch, on the UI
        // path — `--json` never stamps anything.
        pulse_core::settings::mutate(|s| s.resolve_first_run());

        let panel = PanelWindow::new(panel_tx);
        let mut tray = TrayIcon::new(tray_tx);
        tray.set_poll(poll_tx);
        let settings = SettingsWindow::new(settings_tx);
        let store = StoreHandle::start();

        let mut app = App {
            panel,
            tray,
            settings,
            store,
            tx: tx.clone(),
            rx,
            readings: HashMap::new(),
            refreshing: std::collections::HashSet::new(),
        };

        // Paint the rail from the cache before the first round trip, so it
        // is not blank on a cold start.
        let initial = pulse_core::store::initial_readings_from_cache();
        for reading in &initial {
            app.readings.insert(reading.account.id(), reading.clone());
        }
        app.rebuild_entries();
        app.panel.show();

        // Timers: the panel's frame clock, and the store's poll.
        unsafe {
            let _ = SetTimer(Some(app.panel.hwnd), 1, 30, None);
            let _ = SetTimer(Some(app.tray.hwnd), 2, 250, None);
        }

        // Something happening is a reason to look now.
        app.store.send(Command::RefreshAll);
        app
    }

    /// The message loop. Timers wake it; everything else arrives through
    /// the channel.
    pub fn run(&mut self) {
        unsafe {
            let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                self.drain();
            }
        }
        self.store.send(Command::Shutdown);
    }

    fn drain(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                AppMsg::Tray(command) => self.handle_tray(command),
                AppMsg::Panel(event) => self.handle_panel(event),
                AppMsg::Settings(action) => self.handle_settings(action),
                AppMsg::StorePoll => self.poll_store(),
                AppMsg::DeviceFlowDone(result) => match result {
                    Ok(token) => {
                        pulse_core::secrets::set_key("copilot", &token);
                        self.tray.show_balloon(
                            "Pulse",
                            &pulse_core::localization::t("Signed in to GitHub.").to_string(),
                            false,
                        );
                        self.store.send(Command::SettingsChanged);
                        self.settings.rebuild_widgets();
                        self.settings.invalidate();
                    }
                    Err(text) => {
                        self.tray.show_balloon("Pulse", &text, true);
                    }
                },
            }
        }
    }

    fn handle_tray(&mut self, command: TrayCommand) {
        match command {
            TrayCommand::TogglePanel => {
                if self.panel.is_visible() {
                    unsafe {
                        let _ = ShowWindow(self.panel.hwnd, SW_HIDE);
                    }
                } else {
                    self.panel.show();
                }
            }
            TrayCommand::OpenSettings => {
                self.settings.rebuild_widgets();
                unsafe {
                    let _ = ShowWindow(self.settings.hwnd, SW_SHOW);
                    let _ = SetForegroundWindow(self.settings.hwnd);
                }
            }
            TrayCommand::RefreshAll => {
                let accounts = pulse_core::settings::with(|s| {
                    s.ordered_enabled().into_iter().map(|a| a.id()).collect::<Vec<_>>()
                });
                self.refreshing.extend(accounts);
                self.store.send(Command::RefreshAll);
                self.mark_refreshing();
            }
            TrayCommand::Exit => {
                unsafe {
                    PostQuitMessage(0);
                }
            }
        }
    }

    fn handle_panel(&mut self, event: PanelEvent) {
        match event {
            PanelEvent::RefreshAccount(account) => {
                self.refreshing.insert(account.id());
                self.store.send(Command::RefreshAccount(account));
                self.mark_refreshing();
            }
            PanelEvent::OpenSettings => {
                unsafe {
                    let _ = ShowWindow(self.settings.hwnd, SW_SHOW);
                }
            }
            PanelEvent::PositionChanged => {
                // Nothing to do: the panel persisted its own position.
            }
        }
    }

    fn handle_settings(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::Changed => {
                self.panel.reload_settings();
                self.store.send(Command::SettingsChanged);
                self.settings.rebuild_widgets();
                self.settings.invalidate();
            }
            SettingsAction::RefreshProvider(raw) => {
                if let Some(provider) = Provider::from_raw(&raw) {
                    self.refreshing.insert(pulse_core::model::AccountKey::primary(provider).id());
                    self.store
                        .send(Command::RefreshAccount(pulse_core::model::AccountKey::primary(provider)));
                    self.mark_refreshing();
                }
            }
            SettingsAction::SaveKey(_, _) => {
                self.store.send(Command::SettingsChanged);
            }
            SettingsAction::SignInCopilot => self.start_device_flow(),
            SettingsAction::SignOutCopilot => {
                self.store.send(Command::SettingsChanged);
                self.settings.rebuild_widgets();
                self.settings.invalidate();
            }
            SettingsAction::OpenUrl(url) => open_in_browser(&url),
            SettingsAction::Close => self.settings.close(),
        }
    }

    /// The GitHub device flow, on its own thread: ask for a code, put it
    /// in the clipboard and the browser, poll until done. The result comes
    /// back through the channel like everything else.
    fn start_device_flow(&mut self) {
        let tx = self.tx.clone();
        let http = pulse_core::http::HttpClient::new();
        std::thread::spawn(move || {
            let run = || -> Result<String, String> {
                let prompt = pulse_core::providers::device_login::start(&http)
                    .map_err(|e| e.to_string())?;
                crate::clipboard::set_text(&prompt.user_code);
                open_in_browser(&prompt.verification_url);
                // The clipboard is the convenience, not the message: the
                // user still types the code, which is the point.
                pulse_core::providers::device_login::await_token(&http, &prompt)
                    .map_err(|e| e.to_string())
            };
            let _ = tx.send(AppMsg::DeviceFlowDone(run()));
        });
        self.tray.show_balloon(
            "Pulse",
            &pulse_core::localization::t("Device code copied. Finish signing in at github.com/login/device — the code is on your clipboard.").to_string(),
            false,
        );
        self.settings.rebuild_widgets();
        self.settings.invalidate();
    }

    fn mark_refreshing(&mut self) {
        for entry in self.panel.entries_mut() {
            if self.refreshing.contains(&entry.account.id()) {
                entry.ring.is_refreshing = true;
            }
        }
        self.panel.redraw();
    }

    fn poll_store(&mut self) {
        self.panel.follow_pointer_if_enabled();

        while let Some(update) = self.store.poll() {
            match update {
                Update::Readings(readings) => {
                    for reading in readings {
                        self.refreshing.remove(&reading.account.id());
                        self.readings.insert(reading.account.id(), reading);
                    }
                    self.rebuild_entries();
                }
                Update::Alert(event) => {
                    let (title, text) = event.notification_text();
                    let warning = !matches!(
                        event,
                        pulse_core::alerts::AlertEvent::Reset { .. }
                    );
                    self.tray.show_balloon(&title, &text, warning);
                }
            }
        }
        self.update_settings_status();
    }

    fn rebuild_entries(&mut self) {
        let settings = pulse_core::settings::with(|s| s.clone());
        let entries: Vec<RailEntry> = settings
            .ordered_enabled()
            .into_iter()
            .map(|account| {
                let mut entry = match self.readings.get(&account.id()) {
                    Some(reading) => RailEntry::from_reading(reading, &settings, settings.shows_remaining),
                    None => RailEntry::placeholder(account.provider),
                };
                if self.refreshing.contains(&account.id()) {
                    entry.ring.is_refreshing = true;
                }
                entry
            })
            .collect();
        self.panel.set_entries(entries);
    }

    fn update_settings_status(&mut self) {
        if !unsafe { IsWindowVisible(self.settings.hwnd).as_bool() } {
            return;
        }
        for (id, reading) in &self.readings {
            let provider_raw = reading.provider().raw().to_string();
            let _ = id;
            let text = match &reading.state {
                State::Live => {
                    let plan = reading
                        .plan
                        .as_ref()
                        .map(|p| format!(" · {p}"))
                        .unwrap_or_default();
                    let headline = reading.headline_window(None);
                    match headline {
                        Some(window) => format!(
                            "{}{} · {}",
                            window.percent_text(false),
                            plan,
                            pulse_core::timeutil::relative_text(
                                reading.observed_at.unwrap_or(0)
                            )
                        ),
                        None => reading
                            .credit_balance
                            .clone()
                            .unwrap_or_else(|| pulse_core::localization::t("No reading").to_string()),
                    }
                }
                State::Stale => {
                    pulse_core::localization::t("Reading may be out of date").to_string()
                }
                State::Unavailable(reason) => reason.message().to_string(),
            };
            self.settings.set_status(&provider_raw, text);
        }
    }
}

pub fn open_in_browser(url: &str) {
    unsafe {
        let url_wide = crate::winutil::TempWide::new(url);
        let verb = crate::winutil::TempWide::new("open");
        ShellExecuteW(
            None,
            verb.as_ptr(),
            url_wide.as_ptr(),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}
