//! The data model, ported from `UsageProvider.swift` and `ProviderUsage.swift`.
//!
//! The one rule this module exists to keep: **Pulse does not invent usage
//! percentages.** Everything a `UsageWindow` carries came from the provider,
//! and the two labelled exceptions are the only places a denominator was
//! inferred — each carries its `Estimate` so the UI can say so.

use serde::{Deserialize, Serialize};

/// The coding agents Pulse tracks.
///
/// Fourteen of these are ported to Windows; `antigravity`, `cursor`,
/// `ollamaCloud`, `grokBot` and `volcengine` keep their place in the model but
/// are not fetchable here yet — see `is_ported_to_windows`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    ClaudeCode,
    Codex,
    Antigravity,
    Cursor,
    OpenCodeGo,
    KimiCode,
    OllamaCloud,
    Zai,
    #[serde(rename = "glmCoding")]
    GlmCoding,
    Minimax,
    #[serde(rename = "minimaxCN")]
    MinimaxCn,
    Copilot,
    Grok,
    GrokBot,
    Volcengine,
    CommandCode,
    DeepSeek,
}

pub const ALL_PROVIDERS: [Provider; 17] = [
    Provider::ClaudeCode,
    Provider::Codex,
    Provider::Antigravity,
    Provider::Cursor,
    Provider::OpenCodeGo,
    Provider::KimiCode,
    Provider::OllamaCloud,
    Provider::Zai,
    Provider::GlmCoding,
    Provider::Minimax,
    Provider::MinimaxCn,
    Provider::Copilot,
    Provider::Grok,
    Provider::GrokBot,
    Provider::Volcengine,
    Provider::CommandCode,
    Provider::DeepSeek,
];

impl Provider {
    pub fn raw(&self) -> &'static str {
        crate::localization::provider_raw(*self)
    }

    pub fn from_raw(raw: &str) -> Option<Provider> {
        ALL_PROVIDERS.into_iter().find(|p| p.raw() == raw)
    }

    /// Product names, left untranslated.
    pub fn display_name(&self) -> &'static str {
        match self {
            Provider::ClaudeCode => "Claude Code",
            Provider::Codex => "Codex",
            Provider::Antigravity => "Antigravity",
            Provider::Cursor => "Cursor",
            Provider::OpenCodeGo => "OpenCode Go",
            Provider::KimiCode => "Kimi Code",
            Provider::OllamaCloud => "Ollama Cloud",
            // Two entries rather than one with a region switch: two accounts
            // on two services. Named for the two shops, not for the product.
            Provider::Zai => "z.ai",
            Provider::GlmCoding => "Zhipu",
            Provider::Minimax => "MiniMax",
            Provider::MinimaxCn => "MiniMax CN",
            Provider::Copilot => "GitHub Copilot",
            Provider::Grok => "Grok",
            Provider::GrokBot => "Grok Bot",
            Provider::Volcengine => "Volcengine",
            Provider::CommandCode => "Command Code",
            Provider::DeepSeek => "DeepSeek",
        }
    }

    /// A short monogram for the ring's centre disc. The macOS app draws the
    /// product's own mark; the Windows port ships monograms until each mark
    /// is redrawn as vector art. Two letters where one would collide.
    pub fn monogram(&self) -> &'static str {
        match self {
            Provider::ClaudeCode => "Cl",
            Provider::Codex => "Cd",
            Provider::Antigravity => "A",
            Provider::Cursor => "Cu",
            Provider::OpenCodeGo => "Oc",
            Provider::KimiCode => "K",
            Provider::OllamaCloud => "Ol",
            Provider::Zai => "z",
            Provider::GlmCoding => "Gl",
            Provider::Minimax => "M",
            Provider::MinimaxCn => "M",
            Provider::Copilot => "Cp",
            Provider::Grok => "Gk",
            Provider::GrokBot => "X",
            Provider::Volcengine => "V",
            Provider::CommandCode => "Co",
            Provider::DeepSeek => "D",
        }
    }

    /// Whether this provider's route has actually been ported to Windows.
    /// The others stay in the model (settings, JSON, docs) but never fetch,
    /// and Settings says so in as many words.
    pub fn is_ported_to_windows(&self) -> bool {
        matches!(
            self,
            Provider::ClaudeCode
                | Provider::Codex
                | Provider::Copilot
                | Provider::Grok
                | Provider::OpenCodeGo
                | Provider::KimiCode
                | Provider::Zai
                | Provider::GlmCoding
                | Provider::Minimax
                | Provider::MinimaxCn
                | Provider::CommandCode
                | Provider::DeepSeek
        )
    }

    /// Why the provider is not available on Windows, when it is not.
    pub fn windows_gap(&self) -> Option<&'static str> {
        match self {
            Provider::Antigravity => {
                Some("Reads the language server Antigravity runs while it is open — the editor route has not been ported yet.")
            }
            Provider::Cursor => {
                Some("Reads the login Cursor stored in its own database — that store has not been ported yet.")
            }
            Provider::OllamaCloud => {
                Some("Reads a browser session cookie — browser access has not been ported yet.")
            }
            Provider::GrokBot => {
                Some("Reads the login Cursor saved. Enable Cursor's Grok Bot through Cursor until this route is ported.")
            }
            Provider::Volcengine => {
                Some("Signs Volcengine's usage API with access keys — the signer has not been ported yet.")
            }
            _ => None,
        }
    }

    /// Whether a spending history can be shown for this provider at all.
    pub fn provides_history(&self) -> bool {
        false // Transcript ledger is not ported yet; Z.ai/Zhipu statistics pending.
    }

    /// Providers whose spending is invisible to this machine sit on the
    /// adaptive ceiling for ever; prepaid credit is capped shorter instead.
    pub fn reports_spendable_balance(&self) -> bool {
        matches!(self, Provider::DeepSeek | Provider::CommandCode)
    }

    pub fn spending_is_watched_locally(&self) -> bool {
        !self.reports_spendable_balance()
    }

    /// Whether this provider keeps a credential of its own in `keys.dat`.
    pub fn keeps_own_credential(&self) -> bool {
        self.uses_api_key() || *self == Provider::Copilot
    }

    /// Providers Settings offers a paste field for. Ollama's would be a
    /// session cookie, and it is not ported, so it is not listed.
    pub fn uses_api_key(&self) -> bool {
        matches!(
            self,
            Provider::OpenCodeGo
                | Provider::KimiCode
                | Provider::Zai
                | Provider::GlmCoding
                | Provider::Minimax
                | Provider::MinimaxCn
                | Provider::Volcengine
                | Provider::CommandCode
                | Provider::DeepSeek
        )
    }

    /// Whether the provider can report anything at all without being set up
    /// on this machine — evidence that another tool already ran here.
    pub fn can_report_without_setup(&self) -> bool {
        match self {
            Provider::ClaudeCode => home_path(".claude").exists(),
            Provider::Codex => home_path(".codex").exists(),
            Provider::Grok => home_path(".grok").exists(),
            Provider::OpenCodeGo => opencode_stored_key().is_some(),
            Provider::GlmCoding => glm_stored_key().is_some(),
            Provider::CommandCode => commandcode_stored_key().is_some(),
            _ => false,
        }
    }
}

pub fn home_path(rel: &str) -> std::path::PathBuf {
    crate::home_dir().join(rel)
}

pub fn opencode_stored_key() -> Option<String> {
    let text = std::fs::read_to_string(home_path(".local/share/opencode/auth.json")).ok()?;
    let root: serde_json::Value = serde_json::from_str(&text).ok()?;
    let key = root.get("opencode-go")?.get("key")?.as_str()?;
    Some(key.to_string()).filter(|k| !k.is_empty())
}

pub fn glm_stored_key() -> Option<String> {
    for path in [
        home_path(".coding-relay/glm-api-key"),
        home_path(".config/bigmodel/api_key"),
        home_path(".config/zhipu/api_key"),
    ] {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let key = text
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if !key.is_empty() {
                return Some(key);
            }
        }
    }
    None
}

pub fn commandcode_stored_key() -> Option<String> {
    let text = std::fs::read_to_string(home_path(".commandcode/auth.json")).ok()?;
    let root: serde_json::Value = serde_json::from_str(&text).ok()?;
    let key = root.get("apiKey")?.as_str()?;
    Some(key.to_string()).filter(|k| !k.is_empty())
}

/// One account of one provider. A provider's first account id is the
/// provider's own raw value; added accounts carry a `#slot`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountKey {
    pub provider: Provider,
    #[serde(default)]
    pub slot: String,
}

impl AccountKey {
    pub fn primary(provider: Provider) -> Self {
        AccountKey {
            provider,
            slot: String::new(),
        }
    }

    pub fn id(&self) -> String {
        if self.slot.is_empty() {
            self.provider.raw().to_string()
        } else {
            format!("{}#{}", self.provider.raw(), self.slot)
        }
    }

    pub fn is_primary(&self) -> bool {
        self.slot.is_empty()
    }

    pub fn from_id(id: &str) -> Option<AccountKey> {
        let (head, slot) = match id.split_once('#') {
            Some((h, s)) => (h, s),
            None => (id, ""),
        };
        Some(AccountKey {
            provider: Provider::from_raw(head)?,
            slot: slot.to_string(),
        })
    }
}

/// What kind of window a limit is, kept as meaning rather than as text.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Kind {
    FiveHour,
    Weekly,
    Spend,
    /// Prepaid credit, which is **not a limit**: no ceiling, no window.
    Balance,
    Monthly,
    #[serde(rename = "other")]
    Other(#[serde(default)] i64),
}

impl Kind {
    /// The flat token `--json` promises: a script matches on this, so it is
    /// the same in every language.
    pub fn token(&self) -> String {
        match self {
            Kind::FiveHour => "fiveHour".into(),
            Kind::Weekly => "weekly".into(),
            Kind::Spend => "spend".into(),
            Kind::Balance => "balance".into(),
            Kind::Monthly => "monthly".into(),
            Kind::Other(s) => format!("other:{s}"),
        }
    }

    pub fn localized_name(&self, seconds: i64) -> String {
        let key = match self {
            Kind::FiveHour => "5-hour limit",
            Kind::Weekly => "Weekly limit",
            Kind::Spend => "Spend limit",
            Kind::Balance => "Balance",
            Kind::Monthly => "Monthly limit",
            Kind::Other(_) => {
                return if seconds >= 86_400 {
                    crate::localization::t_fmt(
                        "{n}-day limit",
                        &[(&(seconds / 86_400).to_string())],
                    )
                } else {
                    crate::localization::t_fmt(
                        "{n}-hour limit",
                        &[(&((seconds + 3599) / 3600).to_string())],
                    )
                };
            }
        };
        crate::localization::t(key).to_string()
    }
}

/// Where a window's denominator came from, when it did not come from the
/// provider. The one labelled inference in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Estimate {
    /// Command Code: the monthly grant, from its published plan price.
    PlanPrice,
    /// DeepSeek: the highest balance Pulse has watched.
    SinceTopUp,
    /// DeepSeek: a figure the reader typed.
    YourBudget,
}

impl Estimate {
    pub fn token(&self) -> &'static str {
        match self {
            Estimate::PlanPrice => "planPrice",
            Estimate::SinceTopUp => "sinceTopUp",
            Estimate::YourBudget => "yourBudget",
        }
    }
}

/// One rate-limit window exactly as a provider reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageWindow {
    pub id: String,
    pub kind: Kind,
    /// The model this limit is scoped to, when the provider scopes it to one.
    /// A product name, so it is never translated.
    #[serde(default)]
    pub scope: Option<String>,
    /// 0...1 under normal conditions, but a provider may report over 100%
    /// once a limit is exceeded.
    pub used_fraction: f64,
    #[serde(default)]
    pub window_seconds: i64,
    /// Epoch milliseconds.
    #[serde(default)]
    pub resets_at: Option<i64>,
    /// Whether `window_seconds` is a length the provider actually **stated**,
    /// or one chosen so the row sorts. They are not the same thing, and only
    /// one of them can be divided by.
    #[serde(default = "yes")]
    pub reports_length: bool,
    #[serde(default)]
    pub estimate: Option<Estimate>,
    /// Whether the provider says this limit is spent — its judgement, not
    /// `used_fraction >= 1`. A spend limit can run past 100%.
    #[serde(default)]
    pub is_exhausted: bool,
}

fn yes() -> bool {
    true
}

impl UsageWindow {
    pub fn new(
        id: &str,
        kind: Kind,
        scope: Option<String>,
        used_fraction: f64,
        window_seconds: i64,
        resets_at: Option<i64>,
    ) -> Self {
        UsageWindow {
            id: id.to_string(),
            kind,
            scope,
            used_fraction,
            window_seconds,
            resets_at,
            reports_length: true,
            estimate: None,
            is_exhausted: false,
        }
    }

    pub fn is_estimated(&self) -> bool {
        self.estimate.is_some()
    }

    /// How much of this window has gone by, 0...1 — nil unless the provider
    /// gave a reset time **and** stated a length. A sort key is not a length,
    /// and dividing by one draws a fraction the provider never gave.
    pub fn elapsed_fraction(&self, now_ms: i64) -> Option<f64> {
        if !self.reports_length {
            return None;
        }
        let resets = self.resets_at?;
        if self.window_seconds <= 0 {
            return None;
        }
        let remaining_ms = resets - now_ms;
        let window_ms = self.window_seconds as f64 * 1000.0;
        Some((1.0 - remaining_ms as f64 / window_ms).clamp(0.0, 1.0))
    }

    /// The row's full name: the kind, the scope, and which inference applies.
    pub fn display_name(&self) -> String {
        let base = self.kind.localized_name(self.window_seconds);
        let scoped = match &self.scope {
            Some(s) => format!("{base} · {s}"),
            None => base,
        };
        match self.estimate {
            Some(e) => {
                let title = match e {
                    Estimate::PlanPrice => crate::localization::t("estimated"),
                    Estimate::SinceTopUp => crate::localization::t("since top-up"),
                    Estimate::YourBudget => crate::localization::t("of your budget"),
                };
                format!("{scoped} · {title}")
            }
            None => scoped,
        }
    }

    /// A fraction as a whole percentage that never rounds away the fact that
    /// there is *some*, or that there is *not all*. NaN first, because clamp
    /// propagates it rather than catching it.
    fn figure(fraction: f64) -> i64 {
        if !fraction.is_finite() {
            return 0;
        }
        let percent = fraction.clamp(0.0, 1.0) * 100.0;
        if percent <= 0.0 {
            return 0;
        }
        if percent >= 100.0 {
            return 100;
        }
        (percent.round() as i64).clamp(1, 99)
    }

    pub fn percent_value(&self, remaining: bool) -> i64 {
        if remaining {
            Self::figure(self.remaining_fraction())
        } else {
            Self::figure(self.used_fraction)
        }
    }

    pub fn percent_text(&self, remaining: bool) -> String {
        format!("{}%", self.percent_value(remaining))
    }

    pub fn remaining_fraction(&self) -> f64 {
        (1.0 - self.used_fraction).clamp(0.0, 1.0)
    }

    /// "5h" / "7d" style description of the window's length.
    pub fn length_text(&self) -> String {
        let hours = self.window_seconds as f64 / 3600.0;
        if hours >= 24.0 {
            crate::localization::t_fmt(
                "{n} days",
                &[(&((hours / 24.0).round() as i64).to_string())],
            )
        } else {
            crate::localization::t_fmt("{n} hours", &[(&(hours.round() as i64).to_string())])
        }
    }
}

/// Money the provider says is left, and what it is denominated in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreditAmount {
    pub amount: f64,
    /// An ISO code, as the provider gave it.
    pub currency: String,
}

impl CreditAmount {
    /// Short enough to read inside a ring, **truncated, never rounded** — a
    /// balance shown as more than it is is the wrong way to be wrong.
    pub fn rail_text(&self) -> String {
        let symbol = currency_symbol(&self.currency);
        let magnitude = self.amount.abs();
        let (value, suffix) = if magnitude >= 1_000_000.0 {
            (self.amount / 1_000_000.0, "M")
        } else if magnitude >= 1_000.0 {
            (self.amount / 1_000.0, "k")
        } else {
            (self.amount, "")
        };
        let places: i32 = if value.abs() >= 100.0 {
            0
        } else if suffix.is_empty() {
            2
        } else {
            1
        };
        let scale = 10f64.powi(places);
        // Truncation toward zero, not rounding.
        let shown = (value * scale).trunc() / scale;
        format!("{symbol}{shown:.places$}{suffix}", places = places as usize)
    }
}

fn currency_symbol(code: &str) -> String {
    match code {
        "CNY" | "RMB" => "¥".into(),
        "USD" => "$".into(),
        "EUR" => "€".into(),
        "GBP" => "£".into(),
        "JPY" => "¥".into(),
        other => format!("{other} "),
    }
}

/// Why there is nothing to show. Kept as a case rather than a finished
/// sentence: the text is produced when it is displayed, so it follows the
/// language setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unavailability {
    Loading,
    NotConnected,
    AwaitingResponse,
    NoLimitsReported,
    SignInRequired,
    ClaudeSignInRequired,
    ClaudeLoginExpired,
    CodexNotInstalled,
    CodexServerFailed,
    GrokSignInRequired,
    GrokLoginExpired,
    SignedOut,
    NotSignedIn,
    ApiKeyMissing,
    ApiKeyRefused,
    Unreachable,
    UnreadableReply,
    RateLimited,
    ServerError,
    /// Not ported to Windows yet — named once, in Settings only.
    NotOnWindows,
    ZaiNoCodingPlan,
}

impl Unavailability {
    pub fn message(&self) -> &'static str {
        crate::localization::t(match self {
            Unavailability::Loading => "Loading…",
            Unavailability::NotConnected => "notConnected",
            Unavailability::AwaitingResponse => "awaitingResponse",
            Unavailability::NoLimitsReported => "No limits reported.",
            Unavailability::SignInRequired => "signInRequired",
            Unavailability::ClaudeSignInRequired => "claudeSignInRequired",
            Unavailability::ClaudeLoginExpired => "claudeLoginExpired",
            Unavailability::CodexNotInstalled => "Codex isn't installed.",
            Unavailability::CodexServerFailed => "codexServerFailed",
            Unavailability::GrokSignInRequired => "grokSignInRequired",
            Unavailability::GrokLoginExpired => "grokLoginExpired",
            Unavailability::SignedOut => "signedOut",
            Unavailability::NotSignedIn => "notSignedIn",
            Unavailability::ApiKeyMissing => "Add an API key in Settings.",
            Unavailability::ApiKeyRefused => "That key was refused. Check it in Settings.",
            Unavailability::Unreachable => "The service didn't respond.",
            Unavailability::UnreadableReply => "Couldn't read the reply.",
            Unavailability::RateLimited => "Checking too often — easing off.",
            Unavailability::ServerError => "The service returned an error.",
            Unavailability::NotOnWindows => "notOnWindows",
            Unavailability::ZaiNoCodingPlan => "zaiNoCodingPlan",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "camelCase")]
pub enum State {
    Live,
    Stale,
    Unavailable(Unavailability),
}

/// Everything Pulse currently knows about one provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub account: AccountKey,
    /// Ordered as the provider reports them; the first one drives the ring.
    pub windows: Vec<UsageWindow>,
    /// Epoch milliseconds.
    #[serde(default)]
    pub observed_at: Option<i64>,
    pub state: State,
    #[serde(default)]
    pub plan: Option<String>,
    /// Remaining credit, when the provider reports it, formatted for display.
    #[serde(default)]
    pub credit_balance: Option<String>,
    /// The same figure as a number, where there is one to compare. Separate
    /// from `credit_balance` on purpose: that is a display string and is
    /// sometimes prose — Codex says "Unlimited".
    #[serde(default)]
    pub credit_remaining: Option<CreditAmount>,
    /// Where this reading came from — a stable token, the `--json` `source`.
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub is_cached: bool,
}

impl ProviderUsage {
    pub fn unavailable(account: AccountKey, reason: Unavailability) -> ProviderUsage {
        ProviderUsage {
            account,
            windows: Vec::new(),
            observed_at: None,
            state: State::Unavailable(reason),
            plan: None,
            credit_balance: None,
            credit_remaining: None,
            origin: None,
            is_cached: false,
        }
    }

    pub fn live_now(account: AccountKey, windows: Vec<UsageWindow>) -> ProviderUsage {
        ProviderUsage {
            account,
            windows,
            observed_at: Some(crate::timeutil::now_ms()),
            state: State::Live,
            plan: None,
            credit_balance: None,
            credit_remaining: None,
            origin: None,
            is_cached: false,
        }
    }

    pub fn reports_something(&self) -> bool {
        !self.windows.is_empty() || self.credit_balance.is_some()
    }

    /// The window the rail's ring shows: the pinned one when it still matches,
    /// else the one closest to being used up.
    pub fn headline_window(&self, pinned: Option<&str>) -> Option<&UsageWindow> {
        if let Some(id) = pinned {
            if let Some(w) = self.windows.iter().find(|w| w.id == id) {
                return Some(w);
            }
        }
        self.windows
            .iter()
            .max_by(|a, b| a.used_fraction.partial_cmp(&b.used_fraction).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// The limit the second ring shows: **the fullest of the ring's own model
    /// group**, or of everything else where the group has nothing more.
    pub fn second_window(&self, pinned: Option<&str>) -> Option<&UsageWindow> {
        let headline = self.headline_window(pinned)?;
        if self.windows.len() < 2 {
            return None;
        }
        let rest: Vec<&UsageWindow> =
            self.windows.iter().filter(|w| w.id != headline.id).collect();
        let same_group: Vec<&UsageWindow> = rest
            .iter()
            .copied()
            .filter(|w| w.scope == headline.scope)
            .collect();
        let pool = if same_group.is_empty() { rest } else { same_group };
        pool.into_iter()
            .max_by(|a, b| {
                a.used_fraction
                    .partial_cmp(&b.used_fraction)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    pub fn provider(&self) -> Provider {
        self.account.provider
    }
}

/// Colour means **usage**, not identity. These are the panel's four hues,
/// picked to read at ring size on the dark surface.
pub mod usage_tint {
    use crate::model::UsageWindow;
    pub const CAUTION_THRESHOLD: f64 = 0.5;
    pub const WARNING_THRESHOLD: f64 = 0.75;

    /// (r, g, b) 0..1
    pub const GOOD: [f32; 3] = [0.00, 0.90, 0.55];
    pub const CAUTION: [f32; 3] = [1.00, 0.76, 0.15];
    pub const WARNING: [f32; 3] = [1.00, 0.31, 0.26];
    /// Deeper and flatter than the warning red, so a spent limit doesn't just
    /// look like a slightly redder nearly-spent one.
    pub const EXHAUSTED: [f32; 3] = [0.85, 0.09, 0.13];
    /// The colour the sliver takes past the alert threshold.
    pub const ALERT_GLOW: [f32; 3] = WARNING;

    pub fn color(used_fraction: f64, is_exhausted: bool) -> [f32; 3] {
        if is_exhausted || used_fraction >= 1.0 {
            return EXHAUSTED;
        }
        if used_fraction < CAUTION_THRESHOLD {
            GOOD
        } else if used_fraction < WARNING_THRESHOLD {
            CAUTION
        } else {
            WARNING
        }
    }

    pub fn is_spent(window: Option<&UsageWindow>) -> bool {
        match window {
            None => false,
            Some(w) => w.is_exhausted || w.used_fraction >= 1.0,
        }
    }
}

/// The burn-rate forecast. Off by default; a **projection**, and the copy
/// says so.
pub mod burn_rate {
    use super::UsageWindow;

    /// Nothing is said about a window barely open: early on, dividing by the
    /// elapsed share turns a single burst into a rate that would empty the
    /// account before lunch.
    pub const MINIMUM_ELAPSED: f64 = 0.03;
    /// A prediction further out than this stops being actionable long before
    /// it stops being computable.
    pub const HORIZON_MS: i64 = 2 * 3600 * 1000;

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Reading {
        pub exhausts_before_reset: bool,
        /// Only when that is before the reset **and** inside the horizon.
        pub time_to_exhaustion_ms: Option<i64>,
    }

    pub fn reading(window: &UsageWindow, now_ms: i64) -> Option<Reading> {
        let elapsed = window.elapsed_fraction(now_ms)?;
        if elapsed < MINIMUM_ELAPSED {
            return None;
        }
        let resets = window.resets_at?;
        let until_reset = resets - now_ms;
        if until_reset <= 0 {
            return None;
        }

        let used = window.used_fraction.clamp(0.0, 1.0);
        // The window's own length, from the two figures that describe it.
        let elapsed_seconds = until_reset as f64 / (1.0 - elapsed) * elapsed;
        if elapsed_seconds <= 0.0 {
            return None;
        }

        let per_hour = used / elapsed_seconds * 3600.0;
        if !(used < 1.0) || per_hour <= 0.0 {
            return Some(Reading {
                exhausts_before_reset: used >= 1.0,
                time_to_exhaustion_ms: if used >= 1.0 { Some(0) } else { None },
            });
        }

        let until_empty_ms = ((1.0 - used) / per_hour * 3600.0 * 1000.0) as i64;
        let first = until_empty_ms < until_reset;

        Some(Reading {
            exhausts_before_reset: first,
            time_to_exhaustion_ms: if first && until_empty_ms <= HORIZON_MS {
                Some(until_empty_ms)
            } else {
                None
            },
        })
    }

    /// "about an hour", "about 40 minutes" — **rounded, because the precision
    /// is not real.**
    pub fn approximate(ms: i64) -> String {
        let minutes = (ms / 60_000) as f64;
        let minutes = minutes.round() as i64;
        if minutes < 15 {
            return crate::localization::t("under 15 minutes").to_string();
        }
        if minutes < 68 {
            let quarters = ((minutes as f64 / 15.0).round() as i64).max(1);
            return if quarters == 4 {
                crate::localization::t("about an hour").to_string()
            } else {
                crate::localization::t_fmt(
                    "about {n} minutes",
                    &[(&(quarters * 15).to_string())],
                )
            };
        }
        let hours = minutes as f64 / 60.0;
        let halves = ((hours * 2.0).round() as i64).max(2);
        if halves == 2 {
            crate::localization::t("about an hour").to_string()
        } else {
            crate::localization::t_fmt(
                "about {n} hours",
                &[(&(halves as f64 / 2.0).to_string())],
            )
        }
    }
}
