//! Time helpers. Internally everything is epoch milliseconds; parsing accepts
//! the shapes the providers actually send.

use chrono::{DateTime, Local, TimeZone, Utc};

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// ISO 8601 with or without fractional seconds, `Z` or `+00:00`, and the bare
/// `2026-10-01` some replies give.
pub fn parse_iso8601_ms(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    // Bare date: read it as UTC midnight, which is what "the first of the
    // month" means when the reply never said a zone.
    if text.len() == 10 && text.as_bytes()[4] == b'-' {
        let date = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?;
        return Some(
            Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?)
                .timestamp_millis(),
        );
    }

    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|d| d.timestamp_millis())
}

/// `yyyy-MM-dd HH:mm:ss` in **local** time, no zone — the shape the Zhipu
/// statistics endpoint wants.
pub fn format_local_naive(ms: i64) -> String {
    let local: DateTime<Local> = Local.timestamp_millis_opt(ms).single().unwrap_or_default();
    local.format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn epoch_from_parts_ms(local: DateTime<Local>) -> i64 {
    local.timestamp_millis()
}

/// A reset time as the card prints it: the time when it is today, the date
/// too when it is not.
pub fn reset_text(resets_at_ms: i64) -> String {
    let local: DateTime<Local> = match Local.timestamp_millis_opt(resets_at_ms).single() {
        Some(t) => t,
        None => return String::new(),
    };
    let now = Local::now();
    let same_day = local.date_naive() == now.date_naive();
    if same_day {
        local.format("%H:%M").to_string()
    } else {
        local.format("%-m/%-d %H:%M").to_string()
    }
}

/// A relative "3 minutes ago" line, coarse — the card only ever says how much
/// to trust a reading, never sells a precision it does not have.
pub fn relative_text(observed_ms: i64) -> String {
    let delta_ms = now_ms() - observed_ms;
    let minutes = (delta_ms / 60_000).max(0);
    if minutes < 1 {
        return crate::localization::t("just now").to_string();
    }
    if minutes < 60 {
        return crate::localization::t_fmt("{n}m ago", &[(&minutes.to_string())]);
    }
    let hours = minutes / 60;
    if hours < 24 {
        crate::localization::t_fmt("{n}h ago", &[(&hours.to_string())])
    } else {
        crate::localization::t_fmt("{n}d ago", &[(&(hours / 24).to_string())])
    }
}

pub fn iso8601_utc(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_default()
}

/// Epoch seconds vs milliseconds, told apart by magnitude at 1e11: 1e11
/// seconds is the year 5138 and 1e11 milliseconds is 1973, so nothing real is
/// anywhere near the line.
pub fn epoch_to_ms(value: f64) -> i64 {
    if value.abs() > 100_000_000_000.0 {
        value as i64
    } else {
        (value * 1000.0) as i64
    }
}
