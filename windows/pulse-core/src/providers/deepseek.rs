//! DeepSeek's prepaid balance, from the documented
//! `GET https://api.deepseek.com/user/balance`.
//!
//! **There is no allowance, no window, no reset and no spend history** — not
//! in this reply and not anywhere else in the API. Every other provider
//! reports at least one percentage; this one reports money and stops. So the
//! denominator behind the ring comes from somewhere else, and that somewhere
//! is the user's choice: something Pulse watched, a figure they typed, or
//! nothing at all.
//!
//! Every figure arrives as a **string**, including the money, and a field
//! that is absent or unparseable is **absent**, not zero — a balance read as
//! zero is a full red ring and a notification saying the account is spent.

use super::{pasted_or_none, DeepSeekBasis, KeyRing, ProviderService};
use crate::http::HttpClient;
use crate::model::{
    AccountKey, CreditAmount, Estimate, Kind, Provider, ProviderUsage, UsageWindow,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

const ENDPOINT: &str = "https://api.deepseek.com/user/balance";

pub struct DeepSeekService {
    http: Arc<HttpClient>,
}

pub struct Purse {
    pub currency: String,
    pub total: f64,
}

impl ProviderService for DeepSeekService {
    fn provider(&self) -> Provider {
        Provider::DeepSeek
    }

    fn origin_token(&self) -> &'static str {
        "endpoint"
    }

    fn fetch(&self, keys: &KeyRing) -> ProviderUsage {
        self.fetch_with_basis(keys, &DeepSeekBasis::default())
    }
}

impl DeepSeekService {
    pub fn new(http: Arc<HttpClient>) -> Self {
        DeepSeekService { http }
    }

    pub fn fetch_with_basis(&self, keys: &KeyRing, basis: &DeepSeekBasis) -> ProviderUsage {
        let account = AccountKey::primary(Provider::DeepSeek);
        let Some(key) = pasted_or_none(keys.api_key(Provider::DeepSeek)) else {
            return ProviderUsage::unavailable(account, crate::model::Unavailability::ApiKeyMissing);
        };

        let headers = [
            ("Authorization", format!("Bearer {key}")),
            ("Accept", "application/json".to_string()),
        ];
        let header_refs: Vec<(&str, &str)> =
            headers.iter().map(|(k, v)| (*k, v.as_str())).collect();

        let reply = match self
            .http
            .fetch_json(crate::http::Method::Get, ENDPOINT, &header_refs, None)
        {
            Ok(v) => v,
            Err(reason) => return ProviderUsage::unavailable(account, reason),
        };

        let is_available = reply.get("is_available").and_then(|v| v.as_bool());
        let Some(purse) = purse(&reply, basis.currency.as_deref()) else {
            return ProviderUsage::unavailable(
                account,
                crate::model::Unavailability::NoLimitsReported,
            );
        };

        // The mark is advanced on every reading, whichever basis is in
        // force: switching to "since top-up" later should find a peak
        // already there rather than start over from whatever the balance
        // happens to be that afternoon.
        let mut marks = Baseline::load();
        let mark = marks.advance(&purse.currency, purse.total, crate::timeutil::now_ms());
        marks.save();

        let windows = windows_for(
            &purse,
            &basis.basis,
            basis.budget,
            mark.peak,
            mark.set_at,
            is_available,
        );

        let mut usage = ProviderUsage::live_now(account, windows);
        usage.origin = Some(self.origin_token().to_string());
        usage.credit_balance = Some(format!("{:.2}{}", purse.total, currency_suffix(&purse.currency)));
        usage.credit_remaining = Some(CreditAmount {
            amount: purse.total,
            currency: purse.currency.clone(),
        });
        usage
    }
}

/// One currency's money, with the strings turned into numbers. **Which
/// currency the ring follows**: the user's choice if they made one, else the
/// first entry with money in it, else the first entry at all — an account
/// can hold both CNY and USD and they cannot be added together.
fn purse(reply: &serde_json::Value, preferring: Option<&str>) -> Option<Purse> {
    let infos = crate::http::array_field(reply, "balance_infos");
    let purses: Vec<Purse> = infos
        .iter()
        .filter_map(|info| {
            let currency = crate::http::string_field(info, "currency")?;
            let total = money(crate::http::string_field(info, "total_balance"))?;
            Some(Purse {
                currency: currency.to_string(),
                total,
            })
        })
        .collect();
    if purses.is_empty() {
        return None;
    }

    if let Some(preferring) = preferring {
        if let Some(chosen) = purses.iter().find(|p| p.currency == preferring) {
            return Some(Purse {
                currency: chosen.currency.clone(),
                total: chosen.total,
            });
        }
    }
    let chosen = purses
        .iter()
        .find(|p| p.total > 0.0)
        .unwrap_or(&purses[0]);
    Some(Purse {
        currency: chosen.currency.clone(),
        total: chosen.total,
    })
}

/// Money arrives as a string. A field that is absent or unparseable is
/// **absent**, not zero.
fn money(text: Option<&str>) -> Option<f64> {
    let text = text?.trim();
    if text.is_empty() {
        return None;
    }
    text.parse::<f64>().ok()
}

fn currency_suffix(code: &str) -> String {
    match code {
        "CNY" | "RMB" => " ¥".into(),
        "USD" => " $".into(),
        other => format!(" {other}"),
    }
}

/// At most one window, because there is at most one denominator.
/// `balanceOnly` produces none at all and the rail shows the money in place
/// of a percentage. The other two produce a single row whose estimate says
/// where its denominator came from. **No length and no reset, ever.**
pub fn windows_for(
    purse: &Purse,
    basis: &str,
    budget: Option<f64>,
    peak: f64,
    since: i64,
    is_available: Option<bool>,
) -> Vec<UsageWindow> {
    let measured: Option<(f64, Estimate)> = match basis {
        "balanceOnly" => None,
        "budget" => budget
            .filter(|b| b.is_finite() && *b > 0.0)
            .map(|budget| (((budget - purse.total) / budget).clamp(0.0, 1.0), Estimate::YourBudget)),
        // "sinceTopUp" and anything unrecognised: the measured default.
        _ => (peak > 0.0).then(|| {
            ((peak - purse.total) / peak).clamp(0.0, 1.0)
        }).map(|f| (f, Estimate::SinceTopUp)),
    };

    let Some((fraction, estimate)) = measured else {
        return Vec::new();
    };

    let mut window = UsageWindow::new(
        "balance",
        Kind::Balance,
        None,
        fraction,
        30 * 86_400,
        None,
    );
    window.reports_length = false;
    window.estimate = Some(estimate);
    // **DeepSeek's own word**, not the arithmetic: `is_available` is the
    // flag it sets when the balance can no longer pay for a call. A budget
    // the reader set low can reach 100% with money still in the account, and
    // that is not the account being spent.
    window.is_exhausted = is_available == Some(false);
    let _ = since;
    vec![window]
}

fn baseline_path() -> PathBuf {
    crate::data_dir().join("deepseek-baseline.json")
}

/// The highest balance Pulse has watched, per currency. The one measurement
/// the "since top-up" denominator is allowed to rest on.
#[derive(Default, Clone)]
struct Baseline {
    marks: HashMap<String, Mark>,
}

#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
struct Mark {
    peak: f64,
    set_at: i64,
}

impl Baseline {
    fn load() -> Baseline {
        std::fs::read_to_string(baseline_path())
            .ok()
            .and_then(|text| serde_json::from_str::<HashMap<String, Mark>>(&text).ok())
            .map(|marks| Baseline { marks })
            .unwrap_or_default()
    }

    fn advance(&mut self, currency: &str, total: f64, now: i64) -> Mark {
        let mark = self.marks.entry(currency.to_string()).or_default();
        if total > mark.peak {
            mark.peak = total;
            mark.set_at = now;
        } else if mark.set_at == 0 {
            mark.set_at = now;
        }
        *mark
    }

    fn save(&self) {
        let dir = crate::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(text) = serde_json::to_string(&self.marks) {
            let _ = std::fs::write(baseline_path(), text);
        }
    }
}
