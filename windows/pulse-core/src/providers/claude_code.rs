//! Claude Code's usage, from the account's own endpoint.
//!
//! The credentials Claude Code stores on Windows live in
//! `~/.claude/.credentials.json` (there is no keychain here), and the
//! endpoint is the one the CLI itself calls: `GET /api/oauth/usage`. It
//! answers with every limit at once and it answers whenever asked.
//!
//! The status-line and desktop-session fallbacks are macOS routes and are
//! not ported; when the token is missing or refused, that is what gets
//! reported.

use super::{KeyRing, ProviderService};
use crate::http::{number, object_field, string_field, HttpClient};
use crate::model::{
    AccountKey, Kind, Provider, ProviderUsage, State, Unavailability, UsageWindow,
};
use std::sync::{Arc, Mutex};

const USAGE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/profile";

pub struct ClaudeCodeService {
    http: std::sync::Arc<HttpClient>,
    /// The plan's name, asked rarely: a subscription changes about as often
    /// as a person changes their mind about paying for one. The failure is
    /// remembered too, so it never costs the usage reading a retry.
    plan_cache: std::sync::Arc<Mutex<Option<(String, i64)>>>,
}

impl ProviderService for ClaudeCodeService {
    fn provider(&self) -> Provider {
        Provider::ClaudeCode
    }

    fn origin_token(&self) -> &'static str {
        "endpoint"
    }

    fn fetch(&self, _keys: &KeyRing) -> ProviderUsage {
        let account = AccountKey::primary(Provider::ClaudeCode);
        match self.load_access_token() {
            Login::None => ProviderUsage::unavailable(account, Unavailability::ClaudeSignInRequired),
            Login::Expired => {
                ProviderUsage::unavailable(account, Unavailability::ClaudeLoginExpired)
            }
            Login::Usable(token) => self.endpoint_usage(account, token),
        }
    }
}

enum Login {
    None,
    Expired,
    Usable(String),
}

impl ClaudeCodeService {
    pub fn new(http: std::sync::Arc<HttpClient>) -> Self {
        ClaudeCodeService {
            http,
            plan_cache: Arc::new(Mutex::new(None)),
        }
    }

    /// The blob Claude Code stores, as stored, expired or not — so "no login"
    /// and "an expired login" can be told apart, which is the whole message.
    fn load_access_token(&self) -> Login {
        let path = crate::model::home_path(".claude/.credentials.json");
        let Ok(text) = std::fs::read_to_string(path) else {
            return Login::None;
        };
        let Ok(root) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Login::None;
        };
        let Some(oauth) = object_field(&root, "claudeAiOauth") else {
            return Login::None;
        };
        let Some(token) = string_field(oauth, "accessToken") else {
            return Login::None;
        };
        if !token.is_empty() {
            if let Some(expires_ms) = oauth
                .get("expiresAt")
                .and_then(number)
                .filter(|ms| *ms > 1e12)
            {
                if expires_ms as i64 <= crate::timeutil::now_ms() {
                    return Login::Expired;
                }
            }
            return Login::Usable(token.to_string());
        }
        Login::None
    }

    fn endpoint_usage(&self, account: AccountKey, token: String) -> ProviderUsage {
        let headers = [
            ("Authorization", format!("Bearer {token}")),
            ("anthropic-beta", "oauth-2025-04-20".to_string()),
            ("User-Agent", "claude-cli (external, cli)".to_string()),
            ("Accept", "application/json".to_string()),
        ];
        let header_refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect();

        let root = match self.http.fetch_json(crate::http::Method::Get, USAGE_ENDPOINT, &header_refs, None)
        {
            Ok(v) => v,
            Err(Unavailability::ApiKeyRefused) => {
                return ProviderUsage::unavailable(account, Unavailability::ClaudeLoginExpired)
            }
            Err(reason) => return ProviderUsage::unavailable(account, reason),
        };

        // The plan is an enrichment on a second endpoint, asked rarely and
        // never waited for: whatever is already known goes with the figures,
        // and a miss is filled in for the next pass rather than paid for on
        // this one.
        let plan = self.cached_plan();

        let windows = parse_windows(&root);
        let mut usage = ProviderUsage::live_now(account, windows);
        usage.origin = Some(self.origin_token().to_string());
        if usage.windows.is_empty() {
            usage.state = State::Unavailable(Unavailability::NoLimitsReported);
        }
        usage.plan = plan;
        self.refresh_plan_if_stale(&token);
        usage
    }

    fn cached_plan(&self) -> Option<String> {
        let guard = self.plan_cache.lock().unwrap_or_else(|e| e.into_inner());
        match guard.as_ref() {
            // An empty name is a remembered failure, not a plan worth showing.
            Some((name, at)) if !name.is_empty() => {
                Some(name.clone()).filter(|_| crate::timeutil::now_ms() - at < 6 * 3600 * 1000)
            }
            _ => None,
        }
    }

    /// Refreshes the plan name when the cache has aged out, on its own
    /// thread — the usage figures are already in hand by the time this runs,
    /// and a stalled profile call must not hold them back.
    fn refresh_plan_if_stale(&self, token: &str) {
        {
            let guard = self.plan_cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((_, at)) = guard.as_ref() {
                if crate::timeutil::now_ms() - at < 6 * 3600 * 1000 {
                    return;
                }
            }
        }
        let http = self.http.clone();
        let cache = self.plan_cache.clone();
        let token = token.to_string();
        std::thread::spawn(move || {
            let headers = [
                ("Authorization", format!("Bearer {token}")),
                ("anthropic-beta", "oauth-2025-04-20".to_string()),
                ("User-Agent", "claude-cli (external, cli)".to_string()),
                ("Accept", "application/json".to_string()),
            ];
            let header_refs: Vec<(&str, &str)> =
                headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let result = http
                .fetch_json(crate::http::Method::Get, PROFILE_ENDPOINT, &header_refs, None)
                .ok()
                .and_then(|root| plan_name(&root));
            let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some((result.unwrap_or_default(), crate::timeutil::now_ms()));
        });
    }
}

/// The name on the plan, from the tier the account is rate limited at.
pub fn plan_name(root: &serde_json::Value) -> Option<String> {
    let organization = root.get("organization");
    let tier = organization
        .and_then(|o| o.get("rate_limit_tier"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_lowercase());
    let multiplier = tier.as_deref().and_then(|t| {
        ["20x", "5x"]
            .into_iter()
            .find(|m| t.ends_with(&format!("_{m}")))
    });

    let stated = organization
        .and_then(|o| o.get("subscription_type"))
        .or_else(|| root.get("subscription_type"))
        .and_then(|v| v.as_str());
    if let Some(stated) = stated {
        let name = tidy(stated);
        return Some(match multiplier {
            Some(m) => format!("{name} {m}"),
            None => name,
        });
    }

    let Some(tier) = tier else {
        return organization
            .and_then(|o| o.get("organization_type"))
            .and_then(|v| v.as_str())
            .map(tidy);
    };
    let base = if tier.contains("max") {
        "Max"
    } else if tier.contains("team") {
        "Team"
    } else if tier.contains("enterprise") {
        "Enterprise"
    } else if tier.contains("pro") {
        "Pro"
    } else if tier.contains("free") {
        "Free"
    } else {
        return Some(tidy(&tier));
    };
    Some(match multiplier {
        Some(m) => format!("{base} {m}"),
        None => base.to_string(),
    })
}

fn tidy(raw: &str) -> String {
    raw.split(['_', '-'])
        .filter(|s| !s.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `limits[]` is the fuller answer — per-model windows included — and the
/// older top-level pair is the fallback.
pub fn parse_windows(root: &serde_json::Value) -> Vec<UsageWindow> {
    let mut windows: Vec<UsageWindow> = crate::http::array_field(root, "limits")
        .iter()
        .filter_map(window_from_limit)
        .collect();

    if windows.is_empty() {
        for (key, kind, seconds) in [
            ("five_hour", Kind::FiveHour, 5 * 3600),
            ("seven_day", Kind::Weekly, 7 * 86_400),
        ] {
            let Some(node) = object_field(root, key) else {
                continue;
            };
            let Some(percent) = node.get("utilization").and_then(number) else {
                continue;
            };
            let mut window = UsageWindow::new(
                &format!("claudeCode.{key}"),
                kind,
                None,
                percent / 100.0,
                seconds,
                string_field(node, "resets_at").and_then(crate::timeutil::parse_iso8601_ms),
            );
            window.is_exhausted = is_spent(node);
            windows.push(window);
        }
    }
    windows
}

fn window_from_limit(limit: &serde_json::Value) -> Option<UsageWindow> {
    let kind_name = string_field(limit, "kind")?;
    let percent = limit.get("percent").and_then(number)?;

    let (kind, seconds) = match kind_name {
        "session" => (Kind::FiveHour, 5 * 3600),
        "weekly_all" | "weekly_scoped" => (Kind::Weekly, 7 * 86_400),
        _ => return None,
    };

    let scope = limit
        .get("scope")
        .and_then(|s| s.get("model"))
        .and_then(|m| m.get("display_name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut window = UsageWindow::new(
        &format!("claudeCode.{kind_name}.{}", scope.as_deref().unwrap_or("all")),
        kind,
        scope,
        percent / 100.0,
        seconds,
        string_field(limit, "resets_at").and_then(crate::timeutil::parse_iso8601_ms),
    );
    window.is_exhausted = is_spent(limit);
    Some(window)
}

/// `severity` is the provider's own judgement with only loose documentation,
/// so anything that isn't plainly fine is treated as spent — erring towards
/// "you are blocked" is the safer way to be wrong. `locked_reason` is
/// unambiguous.
pub fn is_spent(limit: &serde_json::Value) -> bool {
    if let Some(locked) = limit.get("locked_reason") {
        if !locked.is_null() {
            return true;
        }
    }
    let Some(severity) = string_field(limit, "severity").map(|s| s.to_lowercase()) else {
        return false;
    };
    !matches!(severity.as_str(), "normal" | "ok" | "none" | "healthy")
}
