//! `pulse --json` — the contract other status lines are built on.
//!
//! It prints the cache and **never fetches**: a status line polls every
//! couple of seconds, and seventeen providers cannot be asked at that rate.
//! Every account carries `observedAt` and `ageSeconds`; the consumer decides
//! what counts as too old. It also **reads and never writes** — no first-run
//! stamping on the way through.
//!
//! Nothing in it is translated: `kind` is a flat token, `scope` and `name`
//! are product names, and `label` is the user's own. Window *names* are
//! localized in the app and would change under a script's feet, so they are
//! not a field.

use crate::model::{AccountKey, ProviderUsage, UsageWindow};
use crate::settings::AppSettings;
use serde::Serialize;

#[derive(Serialize)]
pub struct Report {
    /// ISO 8601.
    #[serde(rename = "generatedAt")]
    generated_at: String,
    accounts: Vec<AccountReport>,
}

#[derive(Serialize)]
pub struct AccountReport {
    id: String,
    provider: String,
    /// The product's name.
    name: String,
    /// The user's name for it; the product's name for a first account.
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "creditBalance")]
    credit_balance: Option<String>,
    /// When this reading was taken, absent when there is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "observedAt")]
    observed_at: Option<String>,
    /// `generatedAt − observedAt`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ageSeconds")]
    age_seconds: Option<i64>,
    /// The actual origin of the saved reading; absent for older caches.
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    /// Even without a reading.
    #[serde(rename = "settingsURL")]
    settings_url: String,
    /// The window the ring shows — so a status line never has to re-derive
    /// "which limit matters" and disagree with the panel.
    #[serde(skip_serializing_if = "Option::is_none")]
    headline: Option<HeadlineReport>,
    windows: Vec<WindowReport>,
}

#[derive(Serialize)]
pub struct HeadlineReport {
    #[serde(rename = "windowId")]
    window_id: String,
    #[serde(rename = "usedPercent")]
    used_percent: i64,
    exhausted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "resetsAt")]
    resets_at: Option<String>,
}

#[derive(Serialize)]
pub struct WindowReport {
    id: String,
    /// A flat token: `fiveHour`, `weekly`, `spend`, `monthly`, `balance`,
    /// or `other:<seconds>`.
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    /// The figure the ring shows — the display rule, so a status line
    /// agrees with the panel.
    #[serde(rename = "usedPercent")]
    used_percent: i64,
    /// The reading itself, unrounded.
    #[serde(rename = "usedFraction")]
    used_fraction: f64,
    /// The provider's word, not `usedPercent >= 100`.
    exhausted: bool,
    #[serde(rename = "windowSeconds")]
    window_seconds: i64,
    /// False when windowSeconds is only a sort key. Do not divide by it.
    #[serde(rename = "reportsLength")]
    reports_length: bool,
    estimated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "estimatedFrom")]
    estimated_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "resetsAt")]
    resets_at: Option<String>,
}

/// Reads the stored rail and prints the report. Never fetches, never
/// writes: `--json` does not stamp first-run state on its way through.
pub fn build() -> Report {
    let now = crate::timeutil::now_ms();
    let settings = read_only_settings();
    let mut cache = crate::cache::UsageCache::new();

    let accounts = settings
        .ordered_enabled()
        .into_iter()
        .map(|account| {
            let reading = cache.reading(&account);
            account_report(&account, reading, now)
        })
        .collect();

    Report {
        generated_at: crate::timeutil::iso8601_utc(now),
        accounts,
    }
}

fn account_report(account: &AccountKey, reading: Option<ProviderUsage>, now: i64) -> AccountReport {
    let settings = read_only_settings();
    let label = settings_label(&settings, account);
    let provider = account.provider;

    let windows: Vec<WindowReport> = reading
        .as_ref()
        .map(|r| r.windows.iter().map(window_report).collect())
        .unwrap_or_default();

    let headline = reading.as_ref().and_then(|r| {
        let pinned = settings
            .pinned_windows
            .get(&account.id())
            .map(|s| s.as_str());
        r.headline_window(pinned).map(|w| HeadlineReport {
            window_id: w.id.clone(),
            used_percent: w.percent_value(false),
            exhausted: w.is_exhausted,
            resets_at: w.resets_at.map(crate::timeutil::iso8601_utc),
        })
    });

    AccountReport {
        id: account.id(),
        provider: provider.raw().to_string(),
        name: provider.display_name().to_string(),
        label,
        plan: reading.as_ref().and_then(|r| r.plan.clone()),
        credit_balance: reading.as_ref().and_then(|r| r.credit_balance.clone()),
        observed_at: reading
            .as_ref()
            .and_then(|r| r.observed_at)
            .map(crate::timeutil::iso8601_utc),
        age_seconds: reading
            .as_ref()
            .and_then(|r| r.observed_at)
            .map(|at| (now - at) / 1000),
        source: reading.as_ref().and_then(|r| r.origin.clone()),
        settings_url: format!("pulse://account/{}", urlencode(&account.id())),
        headline,
        windows,
    }
}

fn window_report(window: &UsageWindow) -> WindowReport {
    WindowReport {
        id: window.id.clone(),
        kind: window.kind.token(),
        scope: window.scope.clone(),
        used_percent: window.percent_value(false),
        used_fraction: window.used_fraction,
        exhausted: window.is_exhausted,
        window_seconds: window.window_seconds,
        reports_length: window.reports_length,
        estimated: window.is_estimated(),
        estimated_from: window.estimate.map(|e| e.token().to_string()),
        resets_at: window.resets_at.map(crate::timeutil::iso8601_utc),
    }
}

fn settings_label(_settings: &AppSettings, account: &AccountKey) -> String {
    // Added accounts carry user labels; the primary account is the product's
    // own name.
    account.provider.display_name().to_string()
}

fn urlencode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            // Added-account `#` separators are encoded as %23, not URL
            // fragments.
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The stored settings, without resolving anything.
fn read_only_settings() -> AppSettings {
    // `settings::with` reads the in-memory store, which `initialize()` loaded
    // from disk without mutating; `--json` runs before the UI has done
    // anything, so the store is exactly what the file said.
    let mut out = None;
    crate::settings::with(|s| out = Some(s.clone()));
    out.unwrap_or_default()
}

/// Prints the JSON report to stdout. The one entry point `main` calls.
pub fn print() {
    match serde_json::to_string_pretty(&build()) {
        Ok(text) => println!("{text}"),
        Err(_) => println!(
            "{{\"generatedAt\":\"{}\",\"accounts\":[]}}",
            crate::timeutil::iso8601_utc(crate::timeutil::now_ms())
        ),
    }
}

#[cfg(test)]
mod tests {
    //! The `--json` shape is a contract other people's status lines are
    //! built on, so it is pinned here rather than left to whatever the
    //! encoder happened to do last. Ported from `UsageReportTests`.

    use super::{account_report, window_report};
    use crate::model::{AccountKey, Estimate, Kind, Provider, ProviderUsage, UsageWindow};

    fn reading(account: &AccountKey, windows: Vec<UsageWindow>, observed_at: i64) -> ProviderUsage {
        let mut usage = ProviderUsage::live_now(account.clone(), windows);
        usage.observed_at = Some(observed_at);
        usage
    }

    fn account_json(account: &AccountKey, usage: Option<ProviderUsage>) -> serde_json::Value {
        let report = account_report(account, usage, 1_800_000_000_000);
        serde_json::to_value(&report).expect("account json")
    }

    fn window(
        id: &str,
        kind: Kind,
        used: f64,
        reports_length: bool,
        estimate: Option<Estimate>,
    ) -> UsageWindow {
        let mut w = UsageWindow::new(id, kind, None, used, 7 * 86_400, None);
        w.reports_length = reports_length;
        w.estimate = estimate;
        w
    }

    #[test]
    fn an_account_with_nothing_banked_is_present_and_plainly_empty() {
        // A script should be able to see that Pulse knows about the account
        // and has no figures, rather than finding it missing.
        let account = AccountKey::primary(Provider::Codex);
        let entry = account_json(&account, None);
        assert_eq!(entry["id"], "codex");
        assert_eq!(entry["provider"], "codex");
        assert!(entry.get("observedAt").is_none());
        assert_eq!(entry["windows"].as_array().unwrap().len(), 0);
        assert!(entry.get("headline").is_none());
    }

    #[test]
    fn the_reported_figure_matches_the_one_the_ring_shows() {
        let account = AccountKey::primary(Provider::Cursor);
        let reading = reading(
            &account,
            vec![
                window("tiny", Kind::Weekly, 0.0003, true, None),
                window("nearly", Kind::Weekly, 0.996, true, None),
            ],
            1_800_000_000_000,
        );
        let windows = account_json(&account, Some(reading))["windows"]
            .as_array()
            .unwrap()
            .clone();

        // Anything used never reads 0%, and not quite full never reads 100%.
        assert_eq!(windows[0]["usedPercent"], 1);
        assert_eq!(windows[1]["usedPercent"], 99);
        // And the raw reading is there too, for anything doing its own sums.
        assert_eq!(windows[0]["usedFraction"], 0.0003);
    }

    #[test]
    fn window_kinds_are_flat_tokens_a_script_can_switch_on() {
        let account = AccountKey::primary(Provider::OpenCodeGo);
        let reading = reading(
            &account,
            vec![
                window("a", Kind::FiveHour, 0.1, true, None),
                window("b", Kind::Weekly, 0.1, true, None),
                window("c", Kind::Spend, 0.1, true, None),
                window("d", Kind::Monthly, 0.1, true, None),
                window("e", Kind::Other(10_800), 0.1, true, None),
            ],
            1_800_000_000_000,
        );
        let entry = account_json(&account, Some(reading));
        let tokens: Vec<String> = entry["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["kind"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            tokens,
            ["fiveHour", "weekly", "spend", "monthly", "other:10800"]
        );
    }

    #[test]
    fn nothing_translated_is_printed() {
        // `UsageWindow.name` is localized and would change under a script's
        // feet, so it is deliberately not a field. `scope` is a product name.
        let account = AccountKey::primary(Provider::ClaudeCode);
        let mut scoped = window("w", Kind::Weekly, 0.5, true, None);
        scoped.scope = Some("Opus".into());
        let entry = account_json(
            &account,
            Some(reading(&account, vec![scoped], 1_800_000_000_000)),
        );
        let first = &entry["windows"].as_array().unwrap()[0];
        assert!(first.get("name").is_none());
        assert_eq!(first["scope"], "Opus");
    }

    #[test]
    fn a_sort_key_is_flagged_rather_than_passed_off_as_a_length() {
        let account = AccountKey::primary(Provider::KimiCode);
        let reading = reading(
            &account,
            vec![window("rolling", Kind::Weekly, 0.3, false, None)],
            1_800_000_000_000,
        );
        let entry = account_json(&account, Some(reading));
        let first = &entry["windows"].as_array().unwrap()[0];
        assert_eq!(first["reportsLength"], false);
    }

    #[test]
    fn an_inferred_denominator_is_flagged_and_the_flag_is_on_every_window() {
        // The flag has to be on **every** window, not only the one that sets
        // it, or a script cannot filter on its absence.
        let account = AccountKey::primary(Provider::CommandCode);
        let mut hourly = UsageWindow::new("five-hour", Kind::FiveHour, None, 0.25, 5 * 3_600, None);
        let mut monthly = UsageWindow::new("monthly", Kind::Monthly, None, 0.58, 30 * 86_400, None);
        monthly.estimate = Some(Estimate::PlanPrice);

        let rows = account_json(
            &account,
            Some(reading(&account, vec![hourly, monthly], 1_800_000_000_000)),
        )["windows"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|w| w.get("estimated").is_some()));
        let five_hour = rows.iter().find(|w| w["id"] == "five-hour").unwrap();
        let monthly_row = rows.iter().find(|w| w["id"] == "monthly").unwrap();
        assert_eq!(five_hour["estimated"], false);
        assert_eq!(monthly_row["estimated"], true);
        // Which inference, as a token a script can switch on rather than a
        // translated word.
        assert_eq!(monthly_row["estimatedFrom"], "planPrice");
        assert!(five_hour.get("estimatedFrom").is_none());
        // And it is a flag, not a translated word hidden in a product field.
        assert!(rows.iter().all(|w| w.get("scope").is_none()));
    }

    #[test]
    fn an_added_account_is_named_by_the_users_own_label() {
        let extra = AccountKey::from_id("claudeCode#work").expect("added account id");
        let entry = account_json(&extra, None);
        assert_eq!(entry["id"], "claudeCode#work");
        // The product is still named, so a script can group by it.
        assert_eq!(entry["name"], "Claude Code");
        assert_eq!(entry["provider"], "claudeCode");
        // Added-account `#` separators are encoded, not left as fragments.
        assert_eq!(entry["settingsURL"], "pulse://account/claudeCode%23work");
        assert!(entry.get("source").is_none());
    }

    #[test]
    fn window_report_carries_the_raw_window_contract() {
        let w = window("w", Kind::FiveHour, 0.25, true, None);
        let json = serde_json::to_value(window_report(&w)).expect("window json");
        assert_eq!(json["id"], "w");
        assert_eq!(json["kind"], "fiveHour");
        assert_eq!(json["usedPercent"], 25);
        assert_eq!(json["windowSeconds"], 7 * 86_400);
        assert_eq!(json["reportsLength"], true);
        assert_eq!(json["exhausted"], false);
        // No reset stated, no field invented.
        assert!(json.get("resetsAt").is_none());
    }
}
