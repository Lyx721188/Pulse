//! The MiniMax Coding Plan's limits, from its token-plan endpoint.
//!
//! **Two providers, one service**, for the same reason as the GLM plan:
//! `api.minimax.io` and `api.minimaxi.com` are one company's international
//! and mainland storefronts, and a key for one is refused by the other.
//!
//! Three things about the reply are worth knowing before changing anything.
//! It reports what is **left**, not what is gone. Numbers arrive as strings
//! or as numbers, interchangeably. And lanes exist that are not part of the
//! subscription — they come back with status 3, zero counts and 100%
//! remaining, and read literally would be a ring pinned at 0% for a thing
//! the account cannot use, so they are left out.

use super::{pasted_or_none, KeyRing, ProviderService};
use crate::http::{number, HttpClient};
use crate::model::{AccountKey, Kind, Provider, ProviderUsage, Unavailability, UsageWindow};
use std::sync::Arc;

pub struct MinimaxService {
    http: Arc<HttpClient>,
    store_provider: Provider,
}

impl ProviderService for MinimaxService {
    fn provider(&self) -> Provider {
        self.store_provider
    }

    fn origin_token(&self) -> &'static str {
        "endpoint"
    }

    fn fetch(&self, keys: &KeyRing) -> ProviderUsage {
        let provider = self.store_provider;
        let account = AccountKey::primary(provider);
        let Some(key) = pasted_or_none(keys.api_key(provider)) else {
            return ProviderUsage::unavailable(account, Unavailability::ApiKeyMissing);
        };

        let api_host = if provider == Provider::MinimaxCn {
            "https://api.minimaxi.com"
        } else {
            "https://api.minimax.io"
        };

        // **Both paths are tried on any failure**: an account on the older
        // plan answers 401 on the current path, not 404, so stopping at the
        // first refusal would strand it. The **first** reason is kept, not
        // the last: a refused key from the current path is what the user can
        // act on, and it should not be masked by an older path's noise.
        let endpoints = [
            format!("{api_host}/v1/token_plan/remains"),
            format!("{api_host}/v1/api/openplatform/coding_plan/remains"),
        ];
        let mut first_problem: Option<Unavailability> = None;
        for endpoint in endpoints {
            match self.attempt(&endpoint, &key, provider) {
                Ok(usage) => return usage,
                Err(Attempt::NotFound) => continue,
                Err(Attempt::Failed(reason)) => {
                    first_problem = first_problem.or(Some(reason));
                }
            }
        }
        ProviderUsage::unavailable(
            account,
            first_problem.unwrap_or(Unavailability::Unreachable),
        )
    }
}

enum Attempt {
    NotFound,
    Failed(Unavailability),
}

impl MinimaxService {
    pub fn new(http: Arc<HttpClient>) -> Self {
        MinimaxService {
            http,
            store_provider: Provider::Minimax,
        }
    }

    pub fn for_mainland(http: Arc<HttpClient>) -> MinimaxService {
        MinimaxService {
            http,
            store_provider: Provider::MinimaxCn,
        }
    }

    fn attempt(
        &self,
        endpoint: &str,
        key: &str,
        provider: Provider,
    ) -> Result<ProviderUsage, Attempt> {
        let account = AccountKey::primary(provider);
        let headers = [
            ("Authorization", format!("Bearer {key}")),
            ("Accept", "application/json".to_string()),
        ];
        let header_refs: Vec<(&str, &str)> =
            headers.iter().map(|(k, v)| (*k, v.as_str())).collect();

        let root = self
            .http
            .fetch_json_detailed(crate::http::Method::Get, endpoint, &header_refs, None)
            .map_err(|failure| match failure {
                crate::http::HttpFailure::NotFound => Attempt::NotFound,
                crate::http::HttpFailure::Unavailable(reason) => Attempt::Failed(reason),
            })?;

        // The service's own verdict, which is not the HTTP status: a refused
        // key comes back as a perfectly good 200 with a non-zero status here.
        // **Not every non-zero status is a bad key**: 1004 is the credential
        // one; the rest are the service having a bad day.
        let base = root.get("base_resp");
        let status = base
            .and_then(|b| b.get("status_code"))
            .and_then(number)
            .unwrap_or(0.0);
        if status != 0.0 {
            let said = base
                .and_then(|b| b.get("status_msg"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            let credential = status == 1004.0
                || ["token", "auth", "login", "cookie", "credential"]
                    .iter()
                    .any(|w| said.contains(w));
            return Err(Attempt::Failed(if credential {
                Unavailability::ApiKeyRefused
            } else {
                Unavailability::ServerError
            }));
        }

        // **The payload is not always wrapped.** Some replies put `data` at
        // the root instead; reading only `data` reported a perfectly good
        // account as "no limits reported".
        let payload = root.get("data").unwrap_or(&root);
        let windows = parse_windows(payload, provider);
        if windows.is_empty() {
            return Err(Attempt::Failed(Unavailability::NoLimitsReported));
        }

        let plan = [
            "current_subscribe_title",
            "plan_name",
            "combo_title",
            "current_plan_title",
        ]
        .iter()
        .filter_map(|key| crate::http::string_field(payload, key))
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty());

        let balance = [
            "points_balance",
            "point_balance",
            "credits_balance",
            "credit_balance",
            "balance",
        ]
        .iter()
        .filter_map(|key| crate::http::number_field(payload, key))
        .find(|p| *p > 0.0)
        .map(|points| crate::localization::t_fmt("{n} points", &[(&(points as i64).to_string())]));

        let mut usage = ProviderUsage::live_now(account, windows);
        usage.origin = Some(self.origin_token().to_string());
        usage.plan = plan;
        usage.credit_balance = balance;
        Ok(usage)
    }
}

pub fn parse_windows(payload: &serde_json::Value, provider: Provider) -> Vec<UsageWindow> {
    let models = crate::http::array_field(payload, "model_remains");
    let mut windows: Vec<UsageWindow> = Vec::new();

    for (index, model) in models.iter().enumerate() {
        let name = crate::http::string_field(model, "model_name")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        // "general" is the plan itself rather than a model, so it is left
        // unscoped — a row reading "5-hour limit · general" says nothing.
        let scope = name
            .as_deref()
            .filter(|n| !n.eq_ignore_ascii_case("general"))
            .map(|n| n.to_string());
        // The id has to be unique within one reading; the position settles
        // it when the name cannot.
        let key = name.clone().unwrap_or_else(|| format!("lane{index}"));

        if let Some(window) = interval(model, scope.clone(), &key, provider) {
            windows.push(window);
        }
        if let Some(window) = weekly(model, scope, &key, provider) {
            windows.push(window);
        }
    }

    windows.sort_by_key(|w| w.window_seconds);
    windows
}

/// The short window. Its length is measured from the timestamps rather than
/// assumed: they are the only statement of it the reply makes, and a window
/// with no length can be neither named nor sorted — so it is dropped rather
/// than given an invented one.
fn interval(
    model: &serde_json::Value,
    scope: Option<String>,
    key: &str,
    provider: Provider,
) -> Option<UsageWindow> {
    if is_unavailable_lane(model, "current_interval") {
        return None;
    }
    let used = spent(
        crate::http::number_field(model, "current_interval_remaining_percent"),
        crate::http::number_field(model, "current_interval_total_count"),
        crate::http::number_field(model, "current_interval_usage_count"),
    )?;
    let start = crate::http::number_field(model, "start_time")?;
    let end = crate::http::number_field(model, "end_time")?;
    if end <= start {
        return None;
    }

    // Sub-second intervals would floor to zero and read as "0-hour limit".
    let seconds = ((end - start) / 1000.0) as i64;
    if seconds <= 0 {
        return None;
    }

    Some(UsageWindow::new(
        &format!("{}.{}.interval", provider.raw(), key),
        kind_for_seconds(seconds),
        scope,
        used / 100.0,
        seconds,
        Some(end as i64),
    ))
}

/// The weekly window. Unlike the one above this one names its own length, so
/// it survives a reply that omits the timestamps.
fn weekly(
    model: &serde_json::Value,
    scope: Option<String>,
    key: &str,
    provider: Provider,
) -> Option<UsageWindow> {
    if is_unavailable_lane(model, "current_weekly") {
        return None;
    }
    let used = spent(
        crate::http::number_field(model, "current_weekly_remaining_percent"),
        crate::http::number_field(model, "current_weekly_total_count"),
        crate::http::number_field(model, "current_weekly_usage_count"),
    )?;

    Some(UsageWindow::new(
        &format!("{}.{}.weekly", provider.raw(), key),
        Kind::Weekly,
        scope,
        used / 100.0,
        7 * 86_400,
        crate::http::number_field(model, "weekly_end_time").map(|ms| ms as i64),
    ))
}

/// A lane the schema has but this subscription does not. Status 3 with
/// nothing issued and nothing spent is how the service says "not part of
/// your plan" — taken at face value it draws a ring pinned at 0% for
/// something the account cannot use at all.
fn is_unavailable_lane(model: &serde_json::Value, prefix: &str) -> bool {
    let status = crate::http::number_field(model, &format!("{prefix}_status"));
    let total = crate::http::number_field(model, &format!("{prefix}_total_count"));
    let remaining = crate::http::number_field(model, &format!("{prefix}_remaining_percent"));
    status == Some(3.0) && total.unwrap_or(0.0) == 0.0 && remaining.unwrap_or(0.0) >= 100.0
}

/// How much of the lane is gone, 0...100, or nil when the reply says nothing
/// usable about it. **Everything here is stated as what is *left***: a
/// percentage of 96 is 4 spent — and `current_interval_usage_count` is the
/// **remaining** quota, not the used one, despite its name.
fn spent(percent_remaining: Option<f64>, total: Option<f64>, left: Option<f64>) -> Option<f64> {
    if let Some(percent_remaining) = percent_remaining {
        return Some((100.0 - percent_remaining).clamp(0.0, 100.0));
    }
    let total = total.filter(|t| *t > 0.0)?;
    let left = left?;
    Some(((total - left) / total * 100.0).clamp(0.0, 100.0))
}

fn kind_for_seconds(seconds: i64) -> Kind {
    match seconds {
        18_000 => Kind::FiveHour,
        604_800 => Kind::Weekly,
        2_592_000 => Kind::Monthly,
        _ => Kind::Other(seconds),
    }
}
