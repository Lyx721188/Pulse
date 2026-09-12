//! Command Code's plan limits, credit pool and org spend limits.
//!
//! Command Code bills a **credit balance in US dollars** rather than a token
//! allowance, and layers rolling usage windows and per-organisation spend
//! limits on top. Four undocumented account routes on
//! `https://api.commandcode.ai`, each with `Authorization: Bearer <apiKey>`:
//! whoami, billing/credits, billing/subscriptions, usage/summary.
//!
//! **What is not done:** the CLI carries a hard-coded table of monthly
//! credit allowances per plan id, and Pulse uses it for exactly one number —
//! the monthly plan grant's denominator, labelled `estimated` — because the
//! grant's *remainder* is reported while its size is not. A plan this table
//! cannot size draws nothing, not a guess.

use super::{pasted_or_none, KeyRing, ProviderService};
use crate::http::{number, HttpClient};
use crate::model::{
    AccountKey, Estimate, Kind, Provider, ProviderUsage, State, Unavailability, UsageWindow,
};
use std::sync::Arc;

const HOST: &str = "https://api.commandcode.ai";

/// Plan id, exactly as `subscriptions.data.planId` and `credits.planId`
/// report it, to the dollars it grants each month. Last checked against
/// `command-code@1.51.3`, 2026-09-09. **A plan this table cannot size draws
/// no ring at all** — not zero, not a guess.
fn monthly_grant_usd(plan_id: &str) -> Option<f64> {
    let normalized = plan_id.to_lowercase().replace('_', "-");
    let grant = match normalized.as_str() {
        "individual-go" => 10.0,
        "individual-provider" => 15.0,
        "individual-pro" => 30.0,
        // Not a typo: the same displayed name, "Pro", at two very different
        // allowances — which is why matching is on the whole id, never a
        // prefix.
        "individual-pro-v1" => 80.0,
        "teams-pro" => 40.0,
        "individual-goat" => 70.0,
        "individual-max" => 150.0,
        "individual-ultra" => 300.0,
        _ => return None,
    };
    Some(grant)
}

pub struct CommandCodeService {
    http: Arc<HttpClient>,
}

impl ProviderService for CommandCodeService {
    fn provider(&self) -> Provider {
        Provider::CommandCode
    }

    fn origin_token(&self) -> &'static str {
        "endpoint"
    }

    fn fetch(&self, keys: &KeyRing) -> ProviderUsage {
        let account = AccountKey::primary(Provider::CommandCode);
        let key = pasted_or_none(keys.api_key(Provider::CommandCode))
            .or_else(crate::model::commandcode_stored_key);
        let Some(key) = key else {
            return ProviderUsage::unavailable(account, Unavailability::ApiKeyMissing);
        };

        // `whoami` first, and alone: it is the cheapest call, it says whether
        // the key is any good, and the organisation id it returns is what
        // scopes the other three.
        let whoami = match self.get("/alpha/whoami", &[("limits", "1")], &key) {
            Ok(v) => v,
            Err(reason) => return ProviderUsage::unavailable(account, reason),
        };
        let org = whoami
            .pointer("/org/id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // The credit pool is the reading. Losing it is losing the answer, so
        // its failure is reported rather than papered over with the windows
        // that happen to have arrived.
        let credits = match self.get_opt_query("/alpha/billing/credits", org.as_deref(), &key) {
            Ok(v) => v,
            Err(reason) => return ProviderUsage::unavailable(account, reason),
        };

        // A subscription this account has not got is not a failure — a
        // pay-as-you-go balance is a complete answer — so this one is allowed
        // to come back empty and the billing period simply goes unstated.
        let subscription = self
            .get_opt_query("/alpha/billing/subscriptions", org.as_deref(), &key)
            .ok();

        let mut summary_items: Vec<(&str, String)> = Vec::new();
        if let Some(org) = &org {
            summary_items.push(("orgId", org.clone()));
        }
        // The period start is passed through exactly as it arrived: it is a
        // query parameter to the service that produced it, not a date this
        // side has any business reformatting.
        if let Some(since) = subscription
            .as_ref()
            .and_then(|s| s.get("data"))
            .and_then(|d| d.get("currentPeriodStart"))
        {
            let since = stamp_query(since);
            if !since.is_empty() {
                summary_items.push(("since", since));
            }
        }
        let summary = self
            .get_query("/alpha/usage/summary", &summary_items, &key)
            .ok();

        let windows = build_windows(&whoami, &credits, subscription.as_ref(), summary.as_ref());
        let mut usage = ProviderUsage::live_now(account, windows);
        usage.origin = Some(self.origin_token().to_string());
        if usage.windows.is_empty() {
            usage.state = State::Unavailable(Unavailability::NoLimitsReported);
        }

        let plan_id = plan_id_of(&credits, subscription.as_ref());
        usage.plan = plan_id.as_deref().map(plan_name);
        usage.credit_balance = balance_string(&credits);
        usage.credit_remaining = remaining_amount(&credits);
        usage
    }
}

fn plan_id_of(
    credits: &serde_json::Value,
    subscription: Option<&serde_json::Value>,
) -> Option<String> {
    let from_sub = subscription
        .and_then(|s| s.pointer("/data/planId"))
        .and_then(|v| v.as_str());
    let from_credits = credits.pointer("/credits/planId").and_then(|v| v.as_str());
    let id = from_sub.or(from_credits)?;
    (!id.is_empty()).then(|| id.to_string())
}

/// Whether a plan is paying for this account right now. `"active"` is the
/// CLI's own test, and anything else is an account back on what it has
/// bought. **No answer is not the same as an answer of "none"**: the lookup
/// is allowed to fail, so its silence is not evidence of no plan.
fn is_on_a_plan(credits: &serde_json::Value, subscription: Option<&serde_json::Value>) -> bool {
    let Some(subscription) = subscription else {
        return plan_id_of(credits, None).is_some();
    };
    let Some(data) = subscription.get("data") else {
        // It answered: take it at its word.
        return false;
    };
    match data.get("status").and_then(|v| v.as_str()) {
        // A plan with no status is a reply whose shape has moved; treat it
        // as running — the cost of the other way is the pooled balance
        // passed off as a plan.
        None => true,
        Some(status) => status.eq_ignore_ascii_case("active"),
    }
}

fn build_windows(
    whoami: &serde_json::Value,
    credits: &serde_json::Value,
    subscription: Option<&serde_json::Value>,
    summary: Option<&serde_json::Value>,
) -> Vec<UsageWindow> {
    let mut built = rolling_windows(credits);
    built.extend(org_windows(whoami));
    if let Some(window) = credit_window(credits, subscription, summary) {
        built.push(window);
    }

    // Shortest first; ties keep the order they were built in, so a weekly
    // rolling limit beside a weekly org limit does not swap between passes.
    let mut indexed: Vec<(usize, UsageWindow)> = built.into_iter().enumerate().collect();
    indexed.sort_by(|a, b| {
        a.1.window_seconds
            .cmp(&b.1.window_seconds)
            .then(a.0.cmp(&b.0))
    });
    indexed.into_iter().map(|(_, w)| w).collect()
}

/// The two rolling limits, when the account says it is subject to them.
fn rolling_windows(credits: &serde_json::Value) -> Vec<UsageWindow> {
    let limits = credits.get("windowLimits");
    if limits
        .and_then(|l| l.get("limited"))
        .and_then(|v| v.as_bool())
        != Some(true)
    {
        return Vec::new();
    }
    [
        ("fiveHour", "five-hour", Kind::FiveHour, 5 * 3_600),
        ("weekly", "weekly", Kind::Weekly, 7 * 86_400),
    ]
    .into_iter()
    .filter_map(|(key, id, kind, seconds)| {
        let window = limits?.get(key)?;
        let cap = window.get("cap").and_then(number)?;
        let used = window.get("used").and_then(number)?;
        if cap <= 0.0 {
            return None;
        }
        let mut window = UsageWindow::new(
            id,
            kind,
            None,
            (used / cap).clamp(0.0, 1.0),
            seconds,
            window
                .get("resetAt")
                .and_then(number)
                .map(crate::timeutil::epoch_to_ms),
        );
        window.is_exhausted = used >= cap;
        Some(window)
    })
    .collect()
}

/// The organisation's spend limits, which are money rather than tokens and
/// arrive already counted as **spent** — no inversion here. **A ceiling of
/// zero or less is not a denominator, so there is no row**: `-1` is the usual
/// way to say *unlimited*, and reading it as "reached" would paint an
/// untouched organisation solid red.
fn org_windows(whoami: &serde_json::Value) -> Vec<UsageWindow> {
    let org_limits = crate::http::array_field(whoami, "orgLimits");
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    org_limits
        .iter()
        .filter_map(|limit| {
            let ceiling = limit.get("limit").and_then(number)?;
            let spent = limit.get("spent").and_then(number)?;
            if ceiling <= 0.0 {
                return None;
            }

            let named = limit
                .get("modelLabel")
                .or(limit.get("model"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let scope = (crate::http::string_field(limit, "scope") == Some("model"))
                .then_some(named)
                .flatten();

            let (seconds, stated) = interval_for(crate::http::string_field(limit, "resetInterval"));

            // Ids are built from what each limit *is*, so a row keeps its
            // identity when the array comes back in another order.
            let identity = [
                crate::http::string_field(limit, "scope").unwrap_or("org"),
                crate::http::string_field(limit, "model").unwrap_or(""),
                crate::http::string_field(limit, "resetInterval").unwrap_or(""),
            ]
            .join(".");
            let occurrence = seen.entry(identity.clone()).or_insert(0);
            let id = if *occurrence == 0 {
                format!("org.{identity}")
            } else {
                format!("org.{identity}.{}", *occurrence)
            };
            *occurrence += 1;

            let mut window = UsageWindow::new(
                &id,
                Kind::Spend,
                scope,
                (spent / ceiling).clamp(0.0, 1.0),
                seconds,
                crate::http::string_field(limit, "resetAt")
                    .and_then(crate::timeutil::parse_iso8601_ms),
            );
            window.reports_length = stated;
            window.is_exhausted = limit.get("exceeded").and_then(|v| v.as_bool()) == Some(true);
            Some(window)
        })
        .collect()
}

/// A reset interval as a length, and whether that length is one the provider
/// actually **stated**. A month is 28 to 31 days and is stored as a flat 30 —
/// a sort key, and it must not feed the window clock or the forecast.
fn interval_for(name: Option<&str>) -> (i64, bool) {
    match name {
        Some("daily") => (86_400, true),
        Some("weekly") => (7 * 86_400, true),
        Some("monthly") => (30 * 86_400, false),
        _ => (365 * 86_400, false),
    }
}

/// The monthly row: the plan's grant while one is running, the purchased
/// balance when none is. Two different questions, and answering the wrong
/// one is the failure this splits to avoid — adding the pots together draws
/// a Pro subscriber who has burnt $28 of a $30 month at 12%.
fn credit_window(
    credits: &serde_json::Value,
    subscription: Option<&serde_json::Value>,
    summary: Option<&serde_json::Value>,
) -> Option<UsageWindow> {
    let credits_node = credits.get("credits")?;
    if is_on_a_plan(credits, subscription) {
        plan_window(credits_node, subscription, credits)
    } else {
        pool_window(credits_node, summary, subscription, credits)
    }
}

/// How much of this month's plan grant is gone. The grant is not reported by
/// anything; what *is* reported is the remainder, so the subtraction is the
/// account's own number and only the denominator is inferred. **A plan this
/// build cannot size draws nothing.**
fn plan_window(
    credits: &serde_json::Value,
    subscription: Option<&serde_json::Value>,
    credits_reply: &serde_json::Value,
) -> Option<UsageWindow> {
    let grant = plan_id_of(credits_reply, subscription)
        .as_deref()
        .and_then(monthly_grant_usd)?;
    if grant <= 0.0 {
        return None;
    }
    // Absent is not zero. Without the remainder there is no numerator, and a
    // plan drawn as wholly spent is the loudest way to be wrong.
    let reported = credits.get("monthlyCredits").and_then(number)?;

    let remaining = reported.clamp(0.0, grant);
    let period = billing_period(subscription);

    let mut window = UsageWindow::new(
        "monthly",
        Kind::Monthly,
        None,
        (grant - remaining) / grant,
        period.seconds.unwrap_or(30 * 86_400),
        period.end,
    );
    window.reports_length = period.seconds.is_some();
    // The grant is the one denominator on this rail nobody reported.
    window.estimate = Some(Estimate::PlanPrice);
    window.is_exhausted = reported <= 0.0;
    Some(window)
}

/// What an account with no plan has bought, as a spend limit. **Both halves
/// have to have been reported, and absent is not zero** — a missing half
/// leaves no denominator the provider gave, and there is no window.
fn pool_window(
    credits: &serde_json::Value,
    summary: Option<&serde_json::Value>,
    subscription: Option<&serde_json::Value>,
    credits_reply: &serde_json::Value,
) -> Option<UsageWindow> {
    let pots: Vec<Option<f64>> = vec![
        credits.get("monthlyCredits").and_then(number),
        credits.get("purchasedCredits").and_then(number),
        credits.get("freeCredits").and_then(number),
    ];
    if !pots.iter().any(|p| p.is_some()) {
        return None;
    }
    let reported_spend = summary.and_then(|s| s.get("totalCost")).and_then(number)?;

    let remaining: f64 = pots.iter().filter_map(|p| *p).map(|v| v.max(0.0)).sum();
    let spent = reported_spend.max(0.0);
    let pool = remaining + spent;
    // Nothing left and nothing spent is an account that has said nothing
    // about a pool at all, which is not the same as one that is empty.
    if pool <= 0.0 {
        return None;
    }

    let period = billing_period(subscription);
    let mut window = UsageWindow::new(
        "credits",
        Kind::Spend,
        None,
        (spent / pool).clamp(0.0, 1.0),
        period.seconds.unwrap_or(30 * 86_400),
        period.end,
    );
    window.reports_length = period.seconds.is_some();
    window.is_exhausted = remaining <= 0.0;
    let _ = credits_reply;
    Some(window)
}

/// The billing period, and a length only where the reply gave both ends of
/// it.
struct Period {
    end: Option<i64>,
    seconds: Option<i64>,
}

fn billing_period(subscription: Option<&serde_json::Value>) -> Period {
    let data = subscription.and_then(|s| s.get("data"));
    let start = data
        .and_then(|d| d.get("currentPeriodStart"))
        .map(stamp_date)
        .flatten();
    let end = data
        .and_then(|d| d.get("currentPeriodEnd"))
        .map(stamp_date)
        .flatten();
    let seconds = match (start, end) {
        (Some(start), Some(end)) if end > start => Some((end - start) / 1000),
        _ => None,
    };
    Period { end, seconds }
}

/// A billing-period boundary, which the reply may spell either as a date
/// string or as an epoch number. The text that arrived is kept so the period
/// start can be handed back to the service verbatim.
fn stamp_query(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

fn stamp_date(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::String(text) => crate::timeutil::parse_iso8601_ms(text),
        serde_json::Value::Number(number) => number.as_f64().map(crate::timeutil::epoch_to_ms),
        _ => None,
    }
}

fn plan_name(id: &str) -> String {
    id.split(['-', '_'])
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
        .join(" ")
}

/// The same total as a number, for the low-balance warning. Nil where no pot
/// was reported at all — absent is not a balance of zero. Command Code
/// prices in US dollars and says so nowhere in the reply.
fn remaining_amount(credits: &serde_json::Value) -> Option<crate::model::CreditAmount> {
    let node = credits.get("credits")?;
    let pots: Vec<Option<f64>> = vec![
        node.get("monthlyCredits").and_then(number),
        node.get("purchasedCredits").and_then(number),
        node.get("freeCredits").and_then(number),
    ];
    if !pots.iter().any(|p| p.is_some()) {
        return None;
    }
    Some(crate::model::CreditAmount {
        amount: pots.iter().filter_map(|p| *p).map(|v| v.max(0.0)).sum(),
        currency: "USD".into(),
    })
}

fn balance_string(credits: &serde_json::Value) -> Option<String> {
    remaining_amount(credits).map(|amount| format!("${:.2}", amount.amount))
}

impl CommandCodeService {
    pub fn new(http: Arc<HttpClient>) -> Self {
        CommandCodeService { http }
    }

    fn get(
        &self,
        path: &str,
        query: &[(&str, &str)],
        key: &str,
    ) -> Result<serde_json::Value, Unavailability> {
        let items: Vec<(&str, String)> = query.iter().map(|(k, v)| (*k, v.to_string())).collect();
        self.get_query(path, &items, key)
    }

    fn get_query(
        &self,
        path: &str,
        query: &[(&str, String)],
        key: &str,
    ) -> Result<serde_json::Value, Unavailability> {
        let mut url = format!("{HOST}{path}");
        if !query.is_empty() {
            let pairs: Vec<String> = query
                .iter()
                .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
                .collect();
            url.push('?');
            url.push_str(&pairs.join("&"));
        }

        let headers = [
            ("Authorization", format!("Bearer {key}")),
            ("Accept", "application/json".to_string()),
        ];
        let header_refs: Vec<(&str, &str)> =
            headers.iter().map(|(k, v)| (*k, v.as_str())).collect();

        self.http
            .fetch_json(crate::http::Method::Get, &url, &header_refs, None)
    }

    fn get_opt_query(
        &self,
        path: &str,
        org: Option<&str>,
        key: &str,
    ) -> Result<serde_json::Value, Unavailability> {
        let mut items: Vec<(&str, String)> = Vec::new();
        if let Some(org) = org {
            items.push(("orgId", org.to_string()));
        }
        self.get_query(path, &items, key)
    }
}

fn urlencode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
