//! The Grok account's pool, read with the login Grok Build's CLI stored in
//! `~/.grok/auth.json` — the CLI's own proxy, `cli-chat-proxy.grok.com`.
//!
//! What comes back is the **account's** pool, not the CLI's: since June 2026
//! a paid Grok plan spends one weekly pool across every Grok product. There
//! is no per-product limit to show instead.
//!
//! The stored token lasts about six hours: the CLI renews it while you use
//! Grok, and nothing renews it for Pulse, so an aged-out login is reported
//! rather than worked around.

use super::{KeyRing, ProviderService};
use crate::http::{number, string_field, HttpClient};
use crate::model::{
    AccountKey, Kind, Provider, ProviderUsage, Unavailability, UsageWindow,
};
use std::sync::Arc;

const BILLING_ENDPOINT: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
const SETTINGS_ENDPOINT: &str = "https://cli-chat-proxy.grok.com/v1/settings";

pub struct GrokService {
    http: Arc<HttpClient>,
}

impl ProviderService for GrokService {
    fn provider(&self) -> Provider {
        Provider::Grok
    }

    fn origin_token(&self) -> &'static str {
        "endpoint"
    }

    fn fetch(&self, _keys: &KeyRing) -> ProviderUsage {
        let account = AccountKey::primary(Provider::Grok);
        match stored_login() {
            Login::None => ProviderUsage::unavailable(account, Unavailability::GrokSignInRequired),
            Login::Expired => {
                ProviderUsage::unavailable(account, Unavailability::GrokLoginExpired)
            }
            Login::Usable(token) => self.fetch_with_token(&account, &token),
        }
    }
}

enum Login {
    None,
    Expired,
    Usable(String),
}

impl GrokService {
    pub fn new(http: Arc<HttpClient>) -> Self {
        GrokService { http }
    }

    fn fetch_with_token(&self, account: &AccountKey, token: &str) -> ProviderUsage {
        let headers = [
            ("Authorization", format!("Bearer {token}")),
            // What the CLI sends. Without it the proxy answers the enterprise
            // credit shape instead, whose `monthlyLimit` is zero on a
            // personal plan — a denominator of nothing, and no percentage.
            ("x-xai-token-auth", "xai-grok-cli".to_string()),
            ("Accept", "application/json".to_string()),
        ];
        let header_refs: Vec<(&str, &str)> =
            headers.iter().map(|(k, v)| (*k, v.as_str())).collect();

        let root = match self
            .http
            .fetch_json(crate::http::Method::Get, BILLING_ENDPOINT, &header_refs, None)
        {
            Ok(v) => v,
            Err(Unavailability::ApiKeyRefused) => {
                return ProviderUsage::unavailable(
                    account.clone(),
                    if account.is_primary() {
                        Unavailability::GrokLoginExpired
                    } else {
                        Unavailability::SignedOut
                    },
                )
            }
            Err(reason) => return ProviderUsage::unavailable(account.clone(), reason),
        };

        let Some(config) = root.get("config") else {
            return ProviderUsage::unavailable(account.clone(), Unavailability::UnreadableReply);
        };

        let Some(window) = pool_window(config) else {
            return ProviderUsage::unavailable(account.clone(), Unavailability::NoLimitsReported);
        };

        let plan = self.plan(token);
        let mut usage = ProviderUsage::live_now(account.clone(), vec![window]);
        usage.origin = Some(self.origin_token().to_string());
        usage.plan = plan;
        usage
    }

    /// The plan's name comes from here, not from the billing reply. Optional
    /// enrichment on a short budget — the usage figures are already in hand
    /// by the time this runs.
    fn plan(&self, token: &str) -> Option<String> {
        let request = self
            .http
            .client_for_login()
            .get(SETTINGS_ENDPOINT)
            .header("Authorization", format!("Bearer {token}"))
            .header("x-xai-token-auth", "xai-grok-cli")
            .header("Accept", "application/json")
            .timeout(std::time::Duration::from_secs(4));
        let response = request.send().ok()?;
        if response.status().as_u16() != 200 {
            return None;
        }
        let settings: serde_json::Value = response.json().ok()?;
        let tier = string_field(&settings, "subscription_tier_display")?
            .trim()
            .to_string();
        Some(tier).filter(|t| !t.is_empty())
    }
}

/// What the CLI wrote at its last `grok login`. The file is keyed by issuer
/// and client id rather than by account, and may hold more than one entry,
/// so the freshest unexpired one is taken. An entry that has aged out is
/// kept as evidence: "signed in, and the login has gone stale" is a
/// different instruction from "never signed in".
fn stored_login() -> Login {
    let Ok(text) = std::fs::read_to_string(crate::model::home_path(".grok/auth.json")) else {
        return Login::None;
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Login::None;
    };
    let Some(map) = root.as_object() else {
        return Login::None;
    };

    let now = crate::timeutil::now_ms();
    let mut saw_entry = false;
    // An undated entry is not the same as expired — the token is simply
    // undated, and the service decides. It is kept as a last resort and
    // **cannot outrank a dated one**: parked far in the future it would win
    // every comparison, so it loses instead, which is what a fallback does.
    let mut best: Option<(String, i64)> = None;

    for (_key, entry) in map {
        let Some(token) = entry.get("key").and_then(|v| v.as_str()).filter(|t| !t.is_empty()) else {
            continue;
        };
        saw_entry = true;

        let Some(expires_text) = entry.get("expires_at").and_then(|v| v.as_str()) else {
            if best.is_none() {
                best = Some((token.to_string(), i64::MIN));
            }
            continue;
        };
        let Some(expires) = crate::timeutil::parse_iso8601_ms(expires_text) else {
            if best.is_none() {
                best = Some((token.to_string(), i64::MIN));
            }
            continue;
        };
        if expires <= now {
            continue;
        }
        if best.as_ref().map(|(_, e)| expires > *e).unwrap_or(true) {
            best = Some((token.to_string(), expires));
        }
    }

    match best {
        Some((token, _)) => Login::Usable(token),
        None if saw_entry => Login::Expired,
        None => Login::None,
    }
}

/// The one window the pool amounts to. Both ends of the period are stated,
/// so the length is measured from them rather than assumed — `reportsLength`
/// is true and the window clock and the forecast both have a real number to
/// divide by.
fn pool_window(config: &serde_json::Value) -> Option<UsageWindow> {
    let current = config.get("currentPeriod");
    let start_text = current
        .and_then(|c| string_field(c, "start"))
        .or_else(|| string_field(config, "billingPeriodStart"))?;
    let end_text = current
        .and_then(|c| string_field(c, "end"))
        .or_else(|| string_field(config, "billingPeriodEnd"))?;
    let start = crate::timeutil::parse_iso8601_ms(start_text)?;
    let end = crate::timeutil::parse_iso8601_ms(end_text)?;
    if end <= start {
        return None;
    }

    let seconds = ((end - start) / 1000) as i64;

    // An omitted percentage is a zero the proto3 serialiser dropped — but
    // only inside a period that is actually running. A reply describing a
    // period that has ended says nothing about what has been spent in the
    // one that followed it.
    let now = crate::timeutil::now_ms();
    let percent = match config.get("creditUsagePercent").and_then(number) {
        Some(p) => p,
        None if now >= start && now <= end => 0.0,
        None => return None,
    };

    Some(UsageWindow::new(
        "grok-pool",
        kind_for_seconds(seconds),
        None,
        (percent / 100.0).clamp(0.0, 1.0),
        seconds,
        Some(end),
    ))
}

/// Named from the length the reply gave, not from its `type` string: the
/// enum is theirs to rename, the two timestamps are arithmetic.
fn kind_for_seconds(seconds: i64) -> Kind {
    let days = seconds as f64 / 86_400.0;
    if (6.0..=8.0).contains(&days) {
        Kind::Weekly
    } else if (27.0..=32.0).contains(&days) {
        Kind::Monthly
    } else {
        Kind::Other(seconds)
    }
}
