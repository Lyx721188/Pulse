//! Kimi Code's limits, from its documented usage endpoint, reached with a
//! key the user pastes into Settings.
//!
//! The reply has **two kinds of limit in it and they are not the same
//! figure**: `limits[]` are windows the service actually times, each stating
//! a duration and a unit; `usage` is the weekly allowance, which carries a
//! reset time and no length — the window rolls, so the length is not
//! inferable and is named from what the plan actually is.
//!
//! Every count arrives as a *string*, and `detail` reports what is left
//! rather than what is spent, so both are converted here.

use super::{pasted_or_none, KeyRing, ProviderService};
use crate::http::{number, HttpClient};
use crate::model::{AccountKey, Kind, Provider, ProviderUsage, State, Unavailability, UsageWindow};
use std::sync::Arc;

const ENDPOINT: &str = "https://api.kimi.com/coding/v1/usages";

pub struct KimiService {
    http: Arc<HttpClient>,
}

impl KimiService {
    pub fn new(http: Arc<HttpClient>) -> Self {
        KimiService { http }
    }
}

impl ProviderService for KimiService {
    fn provider(&self) -> Provider {
        Provider::KimiCode
    }

    fn origin_token(&self) -> &'static str {
        "endpoint"
    }

    fn fetch(&self, keys: &KeyRing) -> ProviderUsage {
        let account = AccountKey::primary(Provider::KimiCode);
        let Some(key) = pasted_or_none(keys.api_key(Provider::KimiCode)) else {
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
        usage.plan = root
            .pointer("/user/membership/level")
            .and_then(|v| v.as_str())
            .and_then(plan_name);
        // `totalQuota` comes back empty and `parallel.limit` is how many
        // requests may run at once, which is not a balance.
        usage
    }
}

pub fn parse_windows(root: &serde_json::Value) -> Vec<UsageWindow> {
    let mut found: Vec<UsageWindow> = Vec::new();

    // The timed windows first, named by the length the service states.
    for (index, limit) in crate::http::array_field(root, "limits").iter().enumerate() {
        let Some(seconds) = duration_of(limit.get("window")) else {
            continue;
        };
        if let Some(window) = window_from_detail(
            limit.get("detail"),
            &format!("limit.{index}.{seconds}"),
            kind_for_seconds(seconds),
            seconds,
            true,
        ) {
            found.push(window);
        }
    }

    // Then the weekly allowance, which the reply carries separately and does
    // not put a length on. Only its reset time is ever displayed; the seconds
    // are what sort it after the shorter windows.
    if let Some(weekly) =
        window_from_detail(root.get("usage"), "weekly", Kind::Weekly, 7 * 86_400, false)
    {
        found.push(weekly);
    }

    found.sort_by_key(|w| w.window_seconds);
    found
}

fn window_from_detail(
    detail: Option<&serde_json::Value>,
    id: &str,
    kind: Kind,
    seconds: i64,
    reports_length: bool,
) -> Option<UsageWindow> {
    let detail = detail?;
    let limit = detail.get("limit").and_then(number)?;
    if limit <= 0.0 {
        return None;
    }

    // `used` when it is given, otherwise what the limit and the remainder
    // imply. `limits[].detail` carries no `used` at all.
    let used = detail
        .get("used")
        .and_then(number)
        .or_else(|| detail.get("remaining").and_then(number).map(|r| limit - r))?;

    let mut window = UsageWindow::new(
        id,
        kind,
        None,
        (used / limit).clamp(0.0, 1.0),
        seconds,
        detail
            .get("resetTime")
            .and_then(|v| v.as_str())
            .and_then(crate::timeutil::parse_iso8601_ms),
    );
    window.reports_length = reports_length;
    window.is_exhausted = used >= limit;
    Some(window)
}

/// A window's length in seconds, or nil for a unit that isn't recognised —
/// a window with no length can't be named or sorted, and inventing one
/// would put a figure under a heading that isn't true.
fn duration_of(window: Option<&serde_json::Value>) -> Option<i64> {
    let window = window?;
    let duration = window.get("duration").and_then(number)?;
    if duration <= 0.0 {
        return None;
    }
    let seconds = match window.get("timeUnit").and_then(|v| v.as_str())? {
        "TIME_UNIT_SECOND" => duration,
        "TIME_UNIT_MINUTE" => duration * 60.0,
        "TIME_UNIT_HOUR" => duration * 3_600.0,
        "TIME_UNIT_DAY" => duration * 86_400.0,
        _ => return None,
    };
    Some(seconds as i64)
}

fn kind_for_seconds(seconds: i64) -> Kind {
    match seconds {
        18_000 => Kind::FiveHour,
        604_800 => Kind::Weekly,
        2_592_000 => Kind::Monthly,
        _ => Kind::Other(seconds),
    }
}

/// "LEVEL_INTERMEDIATE" → "Intermediate". An unfamiliar tier is passed
/// through tidied rather than blanked.
fn plan_name(level: &str) -> Option<String> {
    if level.is_empty() {
        return None;
    }
    let bare = level.strip_prefix("LEVEL_").unwrap_or(level);
    let tidied = bare
        .split('_')
        .filter(|s| !s.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>()
                        + chars.as_str().to_lowercase().as_str()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    Some(tidied).filter(|t| !t.is_empty())
}
