//! Cursor's limits, read from the account with the login the editor already
//! stored on this PC.
//!
//! Cursor bills a **month** rather than a rolling window, and what the plan
//! includes is **two separate pools**, which is how the account page draws
//! it: one for Cursor's own models (Composer, Cursor Grok) and one for
//! everything else. Spending past the first eats into the second, and past
//! that into whatever extra spend the account has agreed to — so they are
//! three different limits and are reported as three windows, all resetting
//! with the billing cycle.
//!
//! The login is Cursor's own OAuth token, sitting in the SQLite database VS
//! Code keeps global state in — see `cursor_app_login` below. The website,
//! though, authenticates with a **cookie** rather than a bearer header, and
//! that cookie is not stored anywhere to be found. It is *built*: the
//! account id is the tail of the token's own `sub` claim, and the cookie's
//! value is that id and the token joined by `::`. Which is the whole reason
//! this provider needs nothing from the user.
//!
//! `GET cursor.com/api/usage-summary` is not public API, exactly like the
//! CLI routes, and can change without notice.

use super::{KeyRing, ProviderService};
use crate::model::{
    AccountKey, CreditAmount, Kind, Provider, ProviderUsage, Unavailability, UsageWindow,
};

const ENDPOINT: &str = "https://cursor.com/api/usage-summary";

pub struct CursorService {
    /// The session goes out as a `Cookie` header, which a redirect to
    /// another host would happily carry — so redirects are refused outright.
    http: reqwest::blocking::Client,
}

impl Default for CursorService {
    fn default() -> Self {
        Self::new()
    }
}

impl CursorService {
    pub fn new() -> Self {
        let http = reqwest::blocking::ClientBuilder::new()
            // URLSession's per-request figure for this call.
            .timeout(std::time::Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        CursorService { http }
    }
}

impl ProviderService for CursorService {
    fn provider(&self) -> Provider {
        Provider::Cursor
    }

    fn origin_token(&self) -> &'static str {
        "endpoint"
    }

    fn fetch(&self, _keys: &KeyRing) -> ProviderUsage {
        let account = AccountKey::primary(Provider::Cursor);
        let Some(token) = stored_token() else {
            return ProviderUsage::unavailable(account, Unavailability::CursorSignInRequired);
        };
        // A token that is stored but spent is a login gone stale, not an
        // absent one — so say what actually helps.
        let Some(cookie) = session_cookie(&token) else {
            return ProviderUsage::unavailable(account, Unavailability::CursorLoginExpired);
        };

        let response = self
            .http
            .get(ENDPOINT)
            .header("Cookie", cookie)
            .header("Accept", "application/json")
            .send();
        let response = match response {
            Ok(r) => r,
            Err(_) => return ProviderUsage::unavailable(account, Unavailability::Unreachable),
        };

        match response.status().as_u16() {
            200 => {}
            // The token parsed and had months left on it, and the account
            // still refused it — so it is the login that has gone bad, not
            // the absence of one, and the remedy is to open Cursor rather
            // than to sign in.
            401 | 403 => {
                return ProviderUsage::unavailable(account, Unavailability::CursorLoginExpired)
            }
            429 => {
                return ProviderUsage::unavailable(account, Unavailability::RateLimited);
            }
            _ => {
                return ProviderUsage::unavailable(account, Unavailability::ServerError);
            }
        }

        let text = match response.text() {
            Ok(t) => t,
            Err(_) => return ProviderUsage::unavailable(account, Unavailability::UnreadableReply),
        };
        let reply: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => return ProviderUsage::unavailable(account, Unavailability::UnreadableReply),
        };

        let windows = windows_from_reply(&reply);
        if windows.is_empty() {
            return ProviderUsage::unavailable(account, Unavailability::NoLimitsReported);
        }

        let mut usage = ProviderUsage::live_now(account, windows);
        usage.origin = Some(self.origin_token().to_string());
        usage.plan = reply
            .get("membershipType")
            .and_then(|v| v.as_str())
            .and_then(plan_name);
        if let Some(remaining) = remaining_cents(&reply) {
            // Cents, and Cursor prices in dollars whatever the reader's own
            // currency is — hence a fixed code rather than the locale's.
            let dollars = remaining / 100.0;
            usage.credit_balance = Some(format!("{dollars:.2} $"));
            usage.credit_remaining = Some(CreditAmount {
                amount: dollars,
                currency: "USD".into(),
            });
        }
        usage
    }
}

// --- The database -------------------------------------------------------

/// Cursor is an editor built on VS Code, so its login sits in the SQLite
/// database VS Code keeps global state in.
fn database_path() -> Option<std::path::PathBuf> {
    Some(crate::model::cursor_database())
}

/// Opened read-only, in place.
///
/// Cursor is usually running, which means the database is in WAL mode and
/// its `-wal` and `-shm` sidecars belong to that process. Copying the file
/// aside would take the main database without the journal holding the newest
/// writes, and opening it writable would touch a directory that isn't ours.
/// Read-only leaves all three alone.
fn stored_token() -> Option<String> {
    let path = database_path()?;
    if !path.exists() {
        return None;
    }
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    let statement = connection
        .prepare_cached("SELECT value FROM ItemTable WHERE key = ?1 LIMIT 1")
        .ok()?;
    let mut statement = statement;
    let mut rows = statement.query(["cursorAuth/accessToken"]).ok()?;
    let row = rows.next().ok()??;
    match row.get_ref(0).ok()? {
        rusqlite::types::ValueRef::Text(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        rusqlite::types::ValueRef::Blob(bytes) => text_from_blob(bytes),
        _ => None,
    }
}

/// The row is normally text, and has been seen as a blob. When it is, it is
/// UTF-16 with no byte-order mark — read as UTF-8 that comes out as the
/// token with a null byte after every character, which is a string, parses
/// as nothing, and explains itself to nobody. The zero in the second byte of
/// an ASCII-range character is what gives it away.
fn text_from_blob(bytes: &[u8]) -> Option<String> {
    if bytes.len() >= 2 && bytes.len().is_multiple_of(2) && bytes[0] != 0 && bytes[1] == 0 {
        let wide: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        return Some(String::from_utf16_lossy(&wide));
    }
    String::from_utf8(bytes.to_vec()).ok()
}

// --- The cookie ---------------------------------------------------------

/// The cookie the account pages ask for, built out of the editor's token.
///
/// The token is used to build one request header and is never written,
/// logged or shown anywhere by Pulse.
pub fn session_cookie(token: &str) -> Option<String> {
    let claims = claims(token)?;
    let subject = claims.get("sub")?.as_str()?;
    let expiry = claims.get("exp")?.as_f64()?;
    // A minute's headroom. A token that expires while the request is in
    // flight comes back refused, and nothing here can renew it — Cursor does
    // that for itself, the next time it is used.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs_f64();
    if expiry - now <= 60.0 {
        return None;
    }
    // "auth0|user_abc" — the account id is the half after the bar, and a
    // subject with no bar in it is already the id.
    let account = subject.split('|').next_back()?.trim();
    if account.is_empty() {
        return None;
    }
    // `%3A%3A` is `::` encoded, which is how the value appears in the cookie
    // the site sets for itself.
    Some(format!("WorkosCursorSessionToken={account}%3A%3A{token}"))
}

/// When a token stops being accepted, read from the token itself. Cursor
/// issues these for **60 days** — measured against a real one — which is why
/// this login does not need renewing every few hours the way a Codex or
/// Claude Code login does.
#[allow(dead_code)]
pub fn expiry_of(token: &str) -> Option<i64> {
    claims(token)?
        .get("exp")?
        .as_f64()
        .map(|secs| (secs * 1000.0) as i64)
}

/// The token's middle section, which is where `sub` and `exp` live.
///
/// Base64**URL**, and unpadded — neither of which the strict engines take as
/// they stand, so the padding is stripped before decoding.
fn claims(token: &str) -> Option<serde_json::Value> {
    use base64::Engine;
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1].trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

// --- Reading the reply --------------------------------------------------

/// One pot of money. `used`, `limit` and `remaining` are **cents**; the lane
/// percentages are percentages — see `pool` below.
fn allowances<'a>(reply: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    reply
        .get(key)
        .and_then(|v| v.get("plan").or_else(|| v.get("pooled")))
}

pub fn windows_from_reply(reply: &serde_json::Value) -> Vec<UsageWindow> {
    let resets = crate::http::string_field(reply, "billingCycleEnd")
        .and_then(crate::timeutil::parse_iso8601_ms);
    // A team account reports the same shape under another name.
    let plan = allowances(reply, "individualUsage").or_else(|| allowances(reply, "teamUsage"));
    let mut found = Vec::new();

    // The two pools the plan includes, in the order and under the names the
    // account page gives them.
    if let Some(window) = pool(
        plan.and_then(|p| crate::http::number_field(p, "autoPercentUsed")),
        "cursorModels",
        "Cursor Models",
        resets,
    ) {
        found.push(window);
    }
    if let Some(window) = pool(
        plan.and_then(|p| crate::http::number_field(p, "apiPercentUsed")),
        "otherModels",
        "Other Models",
        resets,
    ) {
        found.push(window);
    }

    // An account shape that reports no pools at all still has the money:
    // what the plan is worth and how much of it has gone. Only used as a
    // fallback, since on an account that *does* report pools this is a
    // different denominator and would read as a third, contradictory limit —
    // see `money` below.
    if found.is_empty() {
        if let Some(window) = money(plan, "plan", Kind::Monthly, resets) {
            found.push(window);
        }
    }

    // Spending past the plan. Off unless the account has turned it on, and
    // left out entirely when it is — a row reading "0% of nothing" says less
    // than no row at all.
    let on_demand = reply
        .get("individualUsage")
        .and_then(|v| v.get("onDemand"))
        .or_else(|| reply.get("teamUsage").and_then(|v| v.get("onDemand")));
    if let Some(window) = money(on_demand, "onDemand", Kind::Spend, resets) {
        found.push(window);
    }

    found
}

/// One of the plan's two pools.
///
/// **The figure is a percentage, not a fraction** — 0.0267 means 0.0267%,
/// not 2.67%. That was settled upstream by arithmetic rather than assumed:
/// on an account 12¢ in, the three percentages the reply carries imply pools
/// of $450 for Cursor's own models and $22.50 for the rest, and a combined
/// $472.50 that matches `totalPercentUsed` to the cent. Three numbers that
/// agree are a system, not a coincidence.
///
/// One consequence to expect: Cursor's own page shows **1%** for anything
/// above zero, so a pool Pulse reads as 0.03% — and rounds to 0% like every
/// other figure in the app — reads as 1% there. Both are true; only the
/// rounding differs.
fn pool(percent: Option<f64>, id: &str, scope: &str, resets: Option<i64>) -> Option<UsageWindow> {
    let percent = percent?;

    let mut window = UsageWindow::new(
        id,
        Kind::Monthly,
        Some(scope.to_string()),
        (percent / 100.0).clamp(0.0, 1.0),
        // A billing cycle, which runs 28 to 31 days. Thirty is what orders
        // the rows; it is not a length Cursor reported, so it is not one to
        // divide by either.
        30 * 86_400,
        resets,
    );
    window.reports_length = false;
    window.is_exhausted = percent >= 100.0;
    Some(window)
}

/// A pot measured in money rather than as a share of a pool: how many of its
/// cents have gone.
///
/// This is a **different denominator** from the two pools above — it is the
/// plan's cash value, $20 on Pro, against which the pools are worth $472.50
/// — so the two must never be shown side by side as though they were the
/// same kind of thing. It is what the extra-spend limit is, and what the
/// plan falls back to when an account reports no pools.
fn money(
    allowance: Option<&serde_json::Value>,
    id: &str,
    kind: Kind,
    resets: Option<i64>,
) -> Option<UsageWindow> {
    let allowance = allowance?;
    if crate::http::bool_field(allowance, "enabled") == Some(false) {
        return None;
    }
    let used = crate::http::number_field(allowance, "used")?;
    let limit = crate::http::number_field(allowance, "limit")?;
    if limit <= 0.0 {
        return None;
    }

    // The billing cycle. Only the reset stamp is ever displayed; this is
    // what orders the two rows — and, being chosen rather than reported, is
    // not something to divide by.
    let mut window = UsageWindow::new(
        id,
        kind,
        None,
        (used / limit).clamp(0.0, 1.0),
        30 * 86_400,
        resets,
    );
    window.reports_length = false;
    window.is_exhausted = used >= limit;
    Some(window)
}

/// What is left of the plan's allowance, in cents.
///
/// A real balance rather than an allowance, which is why it is reported here
/// and not for Antigravity: the account says how much of the money is still
/// there, not merely how much it started with.
fn remaining_cents(reply: &serde_json::Value) -> Option<f64> {
    crate::http::number_field(allowances(reply, "individualUsage")?, "remaining")
        .or_else(|| crate::http::number_field(allowances(reply, "teamUsage")?, "remaining"))
}

/// "pro_plus" → "Pro+". An unfamiliar tier is tidied and passed through
/// rather than blanked: an unknown name still beats none, and it is the only
/// clue left when a new one appears.
fn plan_name(membership: &str) -> Option<String> {
    if membership.is_empty() {
        return None;
    }
    let mut name = String::new();
    for part in membership.split('_') {
        let word = if part == "plus" {
            "+".to_string()
        } else {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>()
                        + chars.as_str().to_lowercase().as_str()
                }
                None => String::new(),
            }
        };
        // "+" joins on to the word before it; anything else is a word.
        if name.is_empty() || word == "+" {
            name.push_str(&word);
        } else {
            name.push(' ');
            name.push_str(&word);
        }
    }
    Some(name).filter(|n| !n.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn the_two_pools_come_out_under_the_account_pages_names() {
        let reply: serde_json::Value = serde_json::from_str(
            r#"{
              "billingCycleEnd": "2026-10-01T00:00:00.000Z",
              "membershipType": "pro_plus",
              "individualUsage": {
                "plan": { "enabled": true, "used": 1200, "limit": 20000, "remaining": 18800,
                          "autoPercentUsed": 0.0267, "apiPercentUsed": 0.1 },
                "onDemand": { "enabled": false, "used": 0, "limit": 200 }
              }
            }"#,
        )
        .unwrap();

        let windows = windows_from_reply(&reply);
        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["cursorModels", "otherModels"]);
        assert_eq!(
            windows
                .iter()
                .map(|w| w.scope.as_deref())
                .collect::<Vec<_>>(),
            [Some("Cursor Models"), Some("Other Models")]
        );
        // The figure is a percentage, not a fraction — reading 0.0267 as
        // 2.67% would be a hundredfold overstatement.
        assert!((windows[0].used_fraction - 0.000_267).abs() < 1e-9);
        assert!(!windows[0].reports_length);
        assert!(!windows[0].is_exhausted);
    }

    #[test]
    fn a_hundred_percent_pool_is_spent() {
        let reply: serde_json::Value = serde_json::from_str(
            r#"{"individualUsage":{"plan":{"autoPercentUsed":100,"apiPercentUsed":0}}}"#,
        )
        .unwrap();
        let windows = windows_from_reply(&reply);
        assert!(windows[0].is_exhausted);
        assert_eq!(windows[0].used_fraction, 1.0);
    }

    #[test]
    fn an_account_without_pools_falls_back_to_the_money() {
        let reply: serde_json::Value = serde_json::from_str(
            r#"{"individualUsage":{"plan":{"enabled":true,"used":500,"limit":2000}}}"#,
        )
        .unwrap();
        let windows = windows_from_reply(&reply);
        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["plan"]);
        assert_eq!(windows[0].kind, Kind::Monthly);
        assert!((windows[0].used_fraction - 0.25).abs() < 1e-9);
    }

    #[test]
    fn an_on_demand_limb_off_by_default_says_nothing() {
        let reply: serde_json::Value = serde_json::from_str(
            r#"{"individualUsage":{"plan":{"enabled":true,"used":500,"limit":2000},
               "onDemand":{"enabled":false,"used":0,"limit":200}}}"#,
        )
        .unwrap();
        let windows = windows_from_reply(&reply);
        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["plan"]);
    }

    #[test]
    fn an_on_demand_limb_that_is_on_is_a_spend_row() {
        let reply: serde_json::Value = serde_json::from_str(
            r#"{"individualUsage":{"plan":{"autoPercentUsed":1},
               "onDemand":{"enabled":true,"used":900,"limit":200}}}"#,
        )
        .unwrap();
        let windows = windows_from_reply(&reply);
        let last = windows.last().unwrap();
        assert_eq!(last.id, "onDemand");
        assert_eq!(last.kind, Kind::Spend);
        // A spend limit can run past its fraction — the clamp holds the row
        // at full while `is_exhausted` carries the provider's own judgement.
        assert_eq!(last.used_fraction, 1.0);
        assert!(last.is_exhausted);
    }

    #[test]
    fn a_team_account_reports_the_same_shape_under_another_name() {
        let reply: serde_json::Value = serde_json::from_str(
            r#"{"teamUsage":{"pooled":{"autoPercentUsed":5,"apiPercentUsed":0}}}"#,
        )
        .unwrap();
        let windows = windows_from_reply(&reply);
        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["cursorModels", "otherModels"]);
    }

    #[test]
    fn the_reset_stamp_is_read_as_a_real_date() {
        let reply: serde_json::Value = serde_json::from_str(
            r#"{"billingCycleEnd":"2026-10-01T00:00:00.000Z","individualUsage":{"plan":{"autoPercentUsed":1}}}"#,
        )
        .unwrap();
        let window = &windows_from_reply(&reply)[0];
        assert_eq!(window.resets_at, Some(1_790_812_800_000));
    }

    #[test]
    fn plan_names_are_tidied_and_passed_through() {
        assert_eq!(plan_name("pro_plus").as_deref(), Some("Pro+"));
        assert_eq!(plan_name("pro").as_deref(), Some("Pro"));
        assert_eq!(plan_name("free_trial").as_deref(), Some("Free Trial"));
        assert_eq!(plan_name("ultra").as_deref(), Some("Ultra"));
        assert_eq!(plan_name(""), None);
    }

    #[test]
    fn the_cookie_is_the_account_id_and_the_token_joined() {
        // {"sub":"auth0|user_abc","exp":4102444800} — an expiry far out.
        let token =
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhdXRoMHx1c2VyX2FiYyIsImV4cCI6NDEwMjQ0NDgwMH0.sig";
        assert_eq!(
            session_cookie(token).as_deref(),
            Some("WorkosCursorSessionToken=user_abc%3A%3AeyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhdXRoMHx1c2VyX2FiYyIsImV4cCI6NDEwMjQ0NDgwMH0.sig")
        );
    }

    #[test]
    fn a_token_with_months_left_is_usable_a_spent_one_is_not() {
        let claims = serde_json::json!({"sub": "auth0|u", "exp": 4102444800u64});
        let mid = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        let token = format!("head.{mid}.sig");
        assert!(session_cookie(&token).is_some());

        let expired = serde_json::json!({"sub": "auth0|u", "exp": 1000u64});
        let mid = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(expired.to_string());
        assert!(session_cookie(&format!("head.{mid}.sig")).is_none());
    }

    #[test]
    fn a_subject_without_a_bar_is_already_the_id() {
        let claims = serde_json::json!({"sub": "user_abc", "exp": 4102444800u64});
        let mid = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        let cookie = session_cookie(&format!("head.{mid}.sig")).unwrap();
        assert!(cookie.starts_with("WorkosCursorSessionToken=user_abc%3A%3A"));
    }
}
