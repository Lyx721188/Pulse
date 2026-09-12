//! GitHub Copilot's quotas, from the endpoint its own editor plugins use.
//!
//! The reply reports what is **left** — `percent_remaining` at 90 means 10%
//! spent — so the inversion happens here and everything downstream stays in
//! terms of what is gone. Three quotas come back — completions, chat and
//! premium interactions — and which of them an account actually has depends
//! on the plan.

use super::{KeyRing, ProviderService};
use crate::http::{bool_field, number, number_field, string_field, HttpClient};
use crate::model::{AccountKey, Kind, Provider, ProviderUsage, State, Unavailability, UsageWindow};
use std::sync::Arc;

const ENDPOINT: &str = "https://api.github.com/copilot_internal/user";

/// The three quotas, in the order they are worth reading. `completions` is
/// included: on a free plan it is the largest allowance of the three, so
/// dropping it hides most of what the account has.
const LANES: [(&str, &str); 3] = [
    ("premium_interactions", "Premium requests"),
    ("chat", "Chat"),
    ("completions", "Completions"),
];

pub struct CopilotService {
    http: Arc<HttpClient>,
}

impl CopilotService {
    pub fn new(http: Arc<HttpClient>) -> Self {
        CopilotService { http }
    }
}

impl ProviderService for CopilotService {
    fn provider(&self) -> Provider {
        Provider::Copilot
    }

    fn origin_token(&self) -> &'static str {
        "sign-in"
    }

    fn fetch(&self, keys: &KeyRing) -> ProviderUsage {
        let account = AccountKey::primary(Provider::Copilot);
        let Some(token) = keys.copilot_token.clone().filter(|t| !t.is_empty()) else {
            return ProviderUsage::unavailable(account, Unavailability::SignedOut);
        };

        let headers = [
            ("Authorization", format!("token {token}")),
            // "token", not "Bearer": this endpoint takes the OAuth token in
            // GitHub's older scheme, and refuses the other one.
            ("Accept", "application/json".to_string()),
            ("X-Github-Api-Version", "2025-04-01".to_string()),
            // It answers a plugin, so it is asked as one.
            ("Editor-Version", "vscode/1.96.2".to_string()),
            ("Editor-Plugin-Version", "copilot-chat/0.26.7".to_string()),
        ];
        let header_refs: Vec<(&str, &str)> =
            headers.iter().map(|(k, v)| (*k, v.as_str())).collect();

        let root =
            match self
                .http
                .fetch_json(crate::http::Method::Get, ENDPOINT, &header_refs, None)
            {
                Ok(v) => v,
                Err(Unavailability::ApiKeyRefused) => {
                    return ProviderUsage::unavailable(account, Unavailability::SignedOut)
                }
                Err(reason) => return ProviderUsage::unavailable(account, reason),
            };

        let windows = parse_windows(&root);
        let mut usage = ProviderUsage::live_now(account, windows);
        usage.origin = Some(self.origin_token().to_string());
        if usage.windows.is_empty() {
            usage.state = State::Unavailable(Unavailability::NoLimitsReported);
        }
        usage.plan = string_field(&root, "copilot_plan").map(plan_name);
        usage
    }
}

pub fn parse_windows(root: &serde_json::Value) -> Vec<UsageWindow> {
    let Some(snapshots) = root.get("quota_snapshots") else {
        return Vec::new();
    };

    // One reset for the account, not one per quota: the per-snapshot
    // `quota_reset_at` comes back as 0.
    let resets = string_field(root, "quota_reset_date_utc")
        .or_else(|| string_field(root, "quota_reset_date"))
        .and_then(crate::timeutil::parse_iso8601_ms);

    LANES
        .into_iter()
        .filter_map(|(key, scope)| {
            let snapshot = snapshots.get(key)?;
            window(snapshot, scope, key, resets)
        })
        .collect()
}

fn window(
    snapshot: &serde_json::Value,
    scope: &str,
    key: &str,
    resets: Option<i64>,
) -> Option<UsageWindow> {
    let entitlement = snapshot.get("entitlement").and_then(number).unwrap_or(0.0);
    let unlimited = bool_field(snapshot, "unlimited").unwrap_or(false);
    let has_quota = bool_field(snapshot, "has_quota");

    // **A quota the plan does not include is left out, not drawn at 100%.**
    // It comes back with `has_quota: false`, nothing issued, and
    // `percent_remaining: 0` — which read literally is a full red ring for
    // something the account never had.
    if has_quota == Some(false) {
        return None;
    }

    // Without that flag, an older reply is told apart by the *percentage*:
    // a lane the plan excludes reads 100% remaining with nothing issued,
    // while a lane you have **run out of** also has nothing left — and
    // dropping that one hides the alarm at the moment it matters.
    if has_quota.is_none()
        && !unlimited
        && entitlement <= 0.0
        && snapshot.get("remaining").and_then(number).unwrap_or(0.0) <= 0.0
        && snapshot
            .get("percent_remaining")
            .and_then(number)
            .unwrap_or(0.0)
            >= 100.0
    {
        return None;
    }

    // An unlimited lane has no share to show, so there is no ring to draw.
    if unlimited {
        return None;
    }
    let remaining = number_field(snapshot, "percent_remaining")?;

    let mut window = UsageWindow::new(
        &format!("copilot.{key}"),
        Kind::Monthly,
        Some(scope.to_string()),
        ((100.0 - remaining) / 100.0).clamp(0.0, 1.0),
        // Enough to sort by, and **not** a length the service stated: a
        // calendar month is 28 to 31 days, so the clock arc must not divide
        // by it.
        30 * 86_400,
        resets,
    );
    window.reports_length = false;
    // **Spent is not the same as over the allowance.** A lane with overage
    // permitted keeps working past its included share and is billed for it —
    // reading `overage_count` as spent painted a red ring for someone who
    // had deliberately paid to carry on.
    window.is_exhausted =
        remaining <= 0.0 && !bool_field(snapshot, "overage_permitted").unwrap_or(false);
    Some(window)
}

pub fn plan_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    match trimmed.to_lowercase().as_str() {
        "individual" => "Individual".into(),
        "free" => "Free".into(),
        "business" => "Business".into(),
        "enterprise" => "Enterprise".into(),
        other => other.to_string(),
    }
}
