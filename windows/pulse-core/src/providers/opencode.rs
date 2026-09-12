//! The OpenCode Go plan's limits, from `GET /zen/go/v1/usage` — undocumented,
//! so it can change without notice, exactly like the two routes the CLIs use.
//!
//! Two places the key can come from, in this order: a key pasted into
//! Settings, which wins, because someone who typed a key meant that one; and
//! what OpenCode saved for itself in `~/.local/share/opencode/auth.json`.

use super::{pasted_or_none, KeyRing, ProviderService};
use crate::http::{number, string_field, HttpClient};
use crate::model::{AccountKey, Kind, Provider, ProviderUsage, State, Unavailability, UsageWindow};
use std::sync::Arc;

const ENDPOINT: &str = "https://opencode.ai/zen/go/v1/usage";

pub struct OpenCodeService {
    http: Arc<HttpClient>,
}

impl OpenCodeService {
    pub fn new(http: Arc<HttpClient>) -> Self {
        OpenCodeService { http }
    }
}

impl ProviderService for OpenCodeService {
    fn provider(&self) -> Provider {
        Provider::OpenCodeGo
    }

    fn origin_token(&self) -> &'static str {
        "endpoint"
    }

    fn fetch(&self, keys: &KeyRing) -> ProviderUsage {
        let account = AccountKey::primary(Provider::OpenCodeGo);
        let key = pasted_or_none(keys.api_key(Provider::OpenCodeGo))
            .or_else(crate::model::opencode_stored_key);
        let Some(key) = key else {
            return ProviderUsage::unavailable(account, Unavailability::ApiKeyMissing);
        };

        let headers = [("Authorization", format!("Bearer {key}"))];
        let header_refs: Vec<(&str, &str)> =
            headers.iter().map(|(k, v)| (*k, v.as_str())).collect();

        let root =
            match self
                .http
                .fetch_json(crate::http::Method::Get, ENDPOINT, &header_refs, None)
            {
                Ok(v) => v,
                Err(reason) => return ProviderUsage::unavailable(account, reason),
            };

        let windows = parse_windows(&root);
        let mut usage = ProviderUsage::live_now(account, windows);
        usage.origin = Some(self.origin_token().to_string());
        if usage.windows.is_empty() {
            usage.state = State::Unavailable(Unavailability::NoLimitsReported);
        }
        // The reply carries limits and nothing else — no plan name, no
        // balance — so neither is invented here.
        usage
    }
}

/// Shortest window first, which is the order the other providers' limits
/// arrive in and the order they matter in — the one about to bite leads.
pub fn parse_windows(root: &serde_json::Value) -> Vec<UsageWindow> {
    let Some(usage) = root.get("usage") else {
        return Vec::new();
    };
    [
        (usage.get("rolling"), "rolling", Kind::FiveHour, 5 * 3_600),
        (usage.get("weekly"), "weekly", Kind::Weekly, 7 * 86_400),
        (usage.get("monthly"), "monthly", Kind::Monthly, 30 * 86_400),
    ]
    .into_iter()
    .filter_map(|(reported, id, kind, seconds)| {
        let reported = reported?;
        let percent = reported.get("percent").and_then(number)?;
        let mut window = UsageWindow::new(
            id,
            kind,
            None,
            (percent / 100.0).clamp(0.0, 1.0),
            seconds,
            string_field(reported, "resetsAt").and_then(crate::timeutil::parse_iso8601_ms),
        );
        // The provider's own verdict, not one inferred from the percentage.
        // Anything other than "ok" is treated as spent — erring towards
        // "you're blocked" is the safer way to be wrong.
        window.is_exhausted = string_field(reported, "status")
            .unwrap_or("ok")
            .to_lowercase()
            != "ok";
        Some(window)
    })
    .collect()
}
