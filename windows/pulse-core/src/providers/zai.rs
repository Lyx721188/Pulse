//! The GLM Coding Plan's limits, from Zhipu's quota endpoint.
//!
//! **Two providers, one service.** Z.ai and BigModel are the same company's
//! international and mainland storefronts, answering the same JSON at the
//! same path on different hosts — but they are separate accounts with
//! separate keys, and a key for one is refused by the other. So each gets
//! its own provider and its own ring.
//!
//! The reply wraps its payload in a status of its own — `success` and
//! `code`, both of which have to say 200 even when HTTP did. A refused key
//! comes back as HTTP 200 with `success: false`, so reading only the status
//! line would report an empty plan rather than a bad key.

use super::{pasted_or_none, KeyRing, ProviderService};
use crate::http::{number, HttpClient};
use crate::model::{AccountKey, Kind, Provider, ProviderUsage, State, Unavailability, UsageWindow};
use std::sync::Arc;

pub struct ZaiService {
    http: Arc<HttpClient>,
    /// Which storefront this instance is for. It decides the host and the
    /// account the answer belongs to, and nothing else.
    store_provider: Provider,
}

impl ProviderService for ZaiService {
    fn provider(&self) -> Provider {
        self.store_provider
    }

    fn origin_token(&self) -> &'static str {
        "endpoint"
    }

    fn fetch(&self, keys: &KeyRing) -> ProviderUsage {
        self.fetch_for(self.store_provider, keys)
    }
}

impl ZaiService {
    pub fn new(http: Arc<HttpClient>) -> Self {
        ZaiService {
            http,
            store_provider: Provider::Zai,
        }
    }

    /// A second instance for the mainland storefront. Same code, different
    /// host, different account, different key.
    pub fn for_mainland(http: Arc<HttpClient>) -> ZaiService {
        ZaiService {
            http,
            store_provider: Provider::GlmCoding,
        }
    }

    pub fn fetch_for(&self, provider: Provider, keys: &KeyRing) -> ProviderUsage {
        let account = AccountKey::primary(provider);
        let host = host_for(provider);
        // What the user pasted wins, so a stale file cannot quietly override
        // a deliberate choice — the same order OpenCode Go's two sources take.
        let key = pasted_or_none(keys.api_key(provider)).or_else(|| {
            (provider == Provider::GlmCoding)
                .then(crate::model::glm_stored_key)
                .flatten()
        });
        let Some(key) = key else {
            return ProviderUsage::unavailable(account, Unavailability::ApiKeyMissing);
        };

        let endpoint = format!("{host}/api/monitor/usage/quota/limit");
        let headers = [
            ("Authorization", format!("Bearer {key}")),
            ("Accept", "application/json".to_string()),
        ];
        let header_refs: Vec<(&str, &str)> =
            headers.iter().map(|(k, v)| (*k, v.as_str())).collect();

        let reply =
            match self
                .http
                .fetch_json(crate::http::Method::Get, &endpoint, &header_refs, None)
            {
                Ok(v) => v,
                Err(reason) => return ProviderUsage::unavailable(account, reason),
            };

        // The envelope's own verdict. A key the service refuses arrives here
        // as a perfectly good HTTP 200, so this is the only place it can be
        // seen — and not every refusal is about the key: the envelope's own
        // code says which.
        if reply.get("success").and_then(|v| v.as_bool()) != Some(true)
            || reply.get("code").and_then(|v| v.as_i64()) != Some(200)
        {
            return ProviderUsage::unavailable(account, envelope_problem(&reply));
        }

        let data = reply.get("data");
        let windows = parse_limits(
            data.map(|d| crate::http::array_field(d, "limits"))
                .unwrap_or(&[]),
            provider,
        );
        let mut usage = ProviderUsage::live_now(account, windows);
        usage.origin = Some(self.origin_token().to_string());
        if usage.windows.is_empty() {
            usage.state = State::Unavailable(Unavailability::NoLimitsReported);
        }
        usage.plan = data.and_then(plan_label);
        usage
    }
}

/// `api.z.ai` is the international one; mainland accounts live on BigModel
/// and are not reachable there.
pub fn host_for(provider: Provider) -> &'static str {
    if provider == Provider::GlmCoding {
        "https://open.bigmodel.cn"
    } else {
        "https://api.z.ai"
    }
}

/// What the envelope's refusal actually was. **Checked in this order,
/// because the code is useless here**: a working key on an account with no
/// running subscription answers `500` — the vendor's generic number — with
/// the coding-plan sentence, and 500 alone would say the service broke.
pub fn envelope_problem(reply: &serde_json::Value) -> Unavailability {
    let said = reply
        .get("msg")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();

    // The phrase is embedded in English on both hosts.
    if said.contains("coding plan") {
        return Unavailability::ZaiNoCodingPlan;
    }

    // Matched as text because the code list below cannot be complete: this
    // is one vendor's private numbering, and the mainland host answers in
    // Chinese.
    let auth_words = [
        "token",
        "auth",
        "key",
        "unauthor",
        "forbidden",
        "credential",
        "身份验证",
        "鉴权",
        "认证",
        "令牌",
        "未授权",
        "无权限",
        "密钥",
    ];
    if auth_words.iter().any(|w| said.contains(w)) {
        return Unavailability::ApiKeyRefused;
    }

    let code = reply.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
    match code {
        401 | 403 => Unavailability::ApiKeyRefused,
        429 => Unavailability::RateLimited,
        // Zhipu's own 1000-series is authentication. 1000 and 1001 are
        // measured; the rest of the band is documented as the same family,
        // and sending somebody to check their key is the better mistake.
        1000..=1099 => Unavailability::ApiKeyRefused,
        _ => Unavailability::ServerError,
    }
}

pub fn parse_limits(limits: &[serde_json::Value], provider: Provider) -> Vec<UsageWindow> {
    let mut windows: Vec<UsageWindow> = limits
        .iter()
        .enumerate()
        .filter_map(|(index, limit)| window(limit, index, provider))
        .collect();
    // Shortest first, so a five-hour limit is read before a weekly one.
    windows.sort_by_key(|w| w.window_seconds);
    windows
}

fn window(limit: &serde_json::Value, index: usize, provider: Provider) -> Option<UsageWindow> {
    // Only these three carry a quota. Anything else the service starts
    // reporting is left out rather than shown under a heading guessed at.
    let type_ = crate::http::string_field(limit, "type")?;
    if !matches!(type_, "TOKENS_LIMIT" | "CREDIT_LIMIT" | "TIME_LIMIT") {
        return None;
    }
    let unit = limit.get("unit").and_then(number)? as i64;
    let number_ = limit.get("number").and_then(number)? as i64;

    let minutes = minutes_for(unit, number_, type_)?;
    let used = used_fraction(limit)?;
    let kind = kind_for_minutes(minutes);

    let window = UsageWindow::new(
        &format!(
            "{}.{}.{}-{}.{}",
            provider.raw(),
            type_,
            unit,
            number_,
            index
        ),
        kind,
        // The MCP lane is a different allowance from the coding quota, and
        // saying so is the only way two rows of the same length tell apart.
        (type_ == "TIME_LIMIT").then(|| "MCP".to_string()),
        used / 100.0,
        minutes * 60,
        limit
            .get("nextResetTime")
            .and_then(number)
            .map(|ms| ms as i64),
    );
    Some(window)
}

/// How long the window runs. The reply states a `unit` code and a `number`
/// of them. An unrecognised unit means the length cannot be read, and a
/// window with no length can be neither named nor sorted — so it is dropped
/// rather than given an invented one.
fn minutes_for(unit: i64, number: i64, type_: &str) -> Option<i64> {
    // A monthly MCP allowance is reported as "1 minute", which is a marker
    // rather than a duration — taken literally it would sort above a
    // five-hour limit and claim to reset every minute.
    if type_ == "TIME_LIMIT" && unit == 5 && number == 1 {
        return Some(30 * 24 * 60);
    }
    let per_unit: i64 = match unit {
        1 => 1440,
        3 => 60,
        5 => 1,
        6 => 10080,
        _ => return None,
    };
    (number > 0).then(|| number * per_unit)
}

fn kind_for_minutes(minutes: i64) -> Kind {
    match minutes {
        300 => Kind::FiveHour,
        10080 => Kind::Weekly,
        43200 => Kind::Monthly,
        _ => Kind::Other(minutes * 60),
    }
}

/// How much of the limit is gone, 0...100. `percentage` is what the service
/// intends to be read, but it is a whole number — so a plan whose counts are
/// also given is worked out from those instead, which is finer. `remaining`
/// is what is *left*, so the spend is the difference; `currentValue` is the
/// spend directly and wins when both are present. **Nil rather than zero**:
/// a limit that arrives with no figure at all is not a limit at 0%.
pub fn used_fraction(limit: &serde_json::Value) -> Option<f64> {
    let usage = limit.get("usage").and_then(number);
    if let Some(usage) = usage.filter(|u| *u > 0.0) {
        let remaining = limit.get("remaining").and_then(number);
        let current = limit.get("currentValue").and_then(number);
        let used = if let Some(remaining) = remaining {
            Some((usage - remaining).max(current.unwrap_or(usage - remaining)))
        } else {
            current
        };
        if let Some(used) = used {
            return Some(((used.min(usage) / usage) * 100.0).clamp(0.0, 100.0));
        }
    }

    let percentage = limit.get("percentage").and_then(number)?;
    Some(percentage.clamp(0.0, 100.0))
}

/// The plan's name, under whichever of five keys this account's tier happens
/// to use. Passed through verbatim when unfamiliar.
fn plan_label(data: &serde_json::Value) -> Option<String> {
    ["planName", "plan", "planType", "packageName", "level"]
        .iter()
        .filter_map(|key| crate::http::string_field(data, key))
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty())
}
