//! The settings store, ported from `AppSettings.swift` (the fields the
//! Windows shell uses) and persisted as JSON in `%APPDATA%\Pulse`.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    /// Account ids currently enabled. The first account of a provider is
    /// enabled as a whole; added accounts are not ported yet, so today an id
    /// is a provider raw value.
    pub enabled_accounts: HashSet<String>,
    /// Provider raw values, in the order the rail draws them.
    pub provider_order: Vec<String>,
    /// Account id -> pinned window id.
    pub pinned_windows: HashMap<String, String>,
    /// Account id -> "#RRGGBB", or empty/absent to colour by usage.
    pub ring_tints: HashMap<String, String>,
    /// Seconds; 0 is the adaptive default.
    pub refresh_interval: i64,
    /// small | standard | large
    pub panel_size: String,
    /// compact | standard | roomy
    pub rail_spacing: String,
    pub side_rail_shows_percentages: bool,
    pub top_rail_shows_percentages: bool,
    pub label_above_ring: bool,
    pub shows_window_clock: bool,
    pub shows_remaining: bool,
    pub shows_forecast: bool,
    pub shows_second_ring: bool,
    /// Collapse to the 6pt sliver while docked and unhovered.
    pub auto_collapse: bool,
    pub follows_active_display: bool,
    /// left | right | top — or none of these with `floating`.
    pub dock_side: String,
    pub floating: bool,
    /// The rail's position as fractions of the display it sits on. The rail's
    /// position is stored, never the window's — the window is wider than the
    /// rail, and which side the rail sits on flips at screen mid.
    pub float_x: f64,
    pub float_y: f64,
    /// Display the panel lives on, as its device path; empty is the primary.
    pub display: String,
    /// auto | en | zh
    pub language: String,
    // --- Alerts. All off by default, all silent. ---
    pub wants_alerts: bool,
    /// 75 | 80 | 90 | 95
    pub alert_threshold: i64,
    pub alerts_on_reset: bool,
    pub alerts_on_failure: bool,
    /// Account id -> balance floor for the low-balance warning.
    pub low_balance_alerts: HashMap<String, f64>,
    // --- DeepSeek's basis: the ring's denominator is a choice. ---
    /// sinceTopUp | budget | balanceOnly
    pub deepseek_basis: String,
    pub deepseek_budget: Option<f64>,
    pub deepseek_currency: Option<String>,
    /// First-run bookkeeping.
    pub has_run: bool,
    pub offered_providers: Vec<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            enabled_accounts: HashSet::new(),
            provider_order: Vec::new(),
            pinned_windows: HashMap::new(),
            ring_tints: HashMap::new(),
            refresh_interval: 0,
            panel_size: "standard".into(),
            rail_spacing: "standard".into(),
            side_rail_shows_percentages: true,
            top_rail_shows_percentages: false,
            label_above_ring: false,
            shows_window_clock: false,
            shows_remaining: false,
            shows_forecast: false,
            shows_second_ring: false,
            auto_collapse: true,
            follows_active_display: false,
            dock_side: "right".into(),
            floating: false,
            float_x: 0.0,
            float_y: 0.5,
            display: String::new(),
            language: "auto".into(),
            wants_alerts: false,
            alert_threshold: 75,
            alerts_on_reset: false,
            alerts_on_failure: false,
            low_balance_alerts: HashMap::new(),
            deepseek_basis: "sinceTopUp".into(),
            deepseek_budget: None,
            deepseek_currency: None,
            has_run: false,
            offered_providers: Vec::new(),
        }
    }
}

impl AppSettings {
    pub fn scale(&self) -> f64 {
        match self.panel_size.as_str() {
            "small" => 0.82,
            "large" => 1.22,
            _ => 1.0,
        }
    }

    pub fn spacing(&self) -> f64 {
        match self.rail_spacing.as_str() {
            "compact" => 0.6,
            "roomy" => 1.4,
            _ => 1.0,
        }
    }

    /// The enabled accounts, in the order the rail draws them: stored order
    /// first, then any provider the order has not heard of, appended in name
    /// order.
    pub fn ordered_enabled(&self) -> Vec<crate::model::AccountKey> {
        use crate::model::Provider;
        let all = crate::model::ALL_PROVIDERS;
        let mut order: Vec<Provider> = Vec::new();

        let known: HashSet<String> = all.iter().map(|p| p.raw().to_string()).collect();
        for raw in &self.provider_order {
            if let Some(p) = Provider::from_raw(raw) {
                if !order.contains(&p) {
                    order.push(p);
                }
            }
        }
        let mut others: Vec<Provider> = all
            .iter()
            .copied()
            .filter(|p| !order.contains(p))
            .collect();
        others.sort_by_key(|p| p.display_name().to_string());
        order.extend(others);
        let _ = known;

        order
            .into_iter()
            .filter(|p| p.is_ported_to_windows())
            .filter(|p| self.enabled_accounts.contains(p.raw()))
            .map(crate::model::AccountKey::primary)
            .collect()
    }

    /// The enabled set a first launch resolves: only providers that have
    /// something to say on this machine, so nobody gets a rail of rings
    /// asking to be configured.
    pub fn resolve_first_run(&mut self) {
        if self.has_run {
            return;
        }
        self.has_run = true;
        for provider in crate::model::ALL_PROVIDERS {
            if provider.is_ported_to_windows() && provider.can_report_without_setup() {
                self.enabled_accounts.insert(provider.raw().to_string());
            }
            self.offered_providers.push(provider.raw().to_string());
        }
    }

    pub fn parse_hex_color(hex: &str) -> Option<[f32; 4]> {
        let text = hex.trim().trim_start_matches('#');
        if text.len() != 6 {
            return None;
        }
        let value = u32::from_str_radix(text, 16).ok()?;
        Some([
            ((value >> 16) & 0xFF) as f32 / 255.0,
            ((value >> 8) & 0xFF) as f32 / 255.0,
            (value & 0xFF) as f32 / 255.0,
            1.0,
        ])
    }

    /// The colour an account's ring draws in: the chosen one, or usage.
    pub fn tint_for(&self, account_id: &str) -> Option<[f32; 4]> {
        self.ring_tints
            .get(account_id)
            .and_then(|h| Self::parse_hex_color(h))
    }
}

#[allow(dead_code)]
fn primary_of(provider: crate::model::Provider) -> crate::model::AccountKey {
    crate::model::AccountKey::primary(provider)
}

// --- Global store -----------------------------------------------------------

static SETTINGS: OnceLock<Mutex<AppSettings>> = OnceLock::new();
static DIRTY: AtomicBool = AtomicBool::new(false);

fn settings_path() -> std::path::PathBuf {
    crate::data_dir().join("settings.json")
}

fn cell() -> &'static Mutex<AppSettings> {
    SETTINGS.get_or_init(|| Mutex::new(load()))
}

/// Loads once at startup: language detection, first-run resolution, then the
/// localized strings follow the stored language.
pub fn initialize() {
    cell();
    let lang = with(|s| s.language.clone());
    match lang.as_str() {
        "en" => crate::localization::set_language(crate::localization::Language::English),
        "zh" => crate::localization::set_language(crate::localization::Language::Chinese),
        _ => {}
    }
}

/// `--json` and friends read the stored rail without stamping first-run
/// state on the way through.
fn load() -> AppSettings {
    match std::fs::read_to_string(settings_path()) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => AppSettings::default(),
    }
}

pub fn save(settings: &AppSettings) {
    let dir = crate::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(text) = serde_json::to_string_pretty(settings) {
        let tmp = settings_path().with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, settings_path());
        }
    }
}

pub fn with<R>(f: impl FnOnce(&AppSettings) -> R) -> R {
    let guard = cell().lock().unwrap_or_else(|e| e.into_inner());
    f(&guard)
}

/// Mutates and persists. Returns whatever the closure does.
pub fn mutate<R>(f: impl FnOnce(&mut AppSettings) -> R) -> R {
    let mut guard = cell().lock().unwrap_or_else(|e| e.into_inner());
    let result = f(&mut guard);
    save(&guard);
    DIRTY.store(true, Ordering::Relaxed);
    result
}

/// For tests only: swap in a fresh store.
#[doc(hidden)]
pub fn reset_for_tests() {
    *cell().lock().unwrap_or_else(|e| e.into_inner()) = AppSettings::default();
}
