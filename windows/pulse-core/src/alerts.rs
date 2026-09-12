//! The alert rules, ported from `UsageAlerts` (the part the Windows shell
//! delivers as tray-balloon notifications).
//!
//! **Notifications say nothing Pulse did not witness.** A `spent` is said
//! only when the provider says so, a reset only on unambiguous evidence, an
//! unavailable reading is not automatically a failure. A limit already past
//! the line when the setting goes on **is** announced, once — silence then a
//! wall is the feature failing. Each thing is said once: never again until
//! it resets or gets worse. All off by default.

use crate::model::{AccountKey, ProviderUsage, State, Unavailability, UsageWindow};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub enum AlertEvent {
    /// A limit crossed the warned threshold.
    Threshold {
        account: AccountKey,
        window_id: String,
        window_name: String,
        percent: i64,
    },
    /// The provider says this limit is spent.
    Spent {
        account: AccountKey,
        window_name: String,
    },
    /// A window Pulse warned about has come back.
    Reset {
        account: AccountKey,
        window_name: String,
    },
    /// Several checks in a row failed, so the panel may be showing older
    /// figures.
    Failure {
        account: AccountKey,
        provider_name: String,
    },
    /// A prepaid balance fell under the figure the reader set.
    LowBalance {
        account: AccountKey,
        provider_name: String,
        text: String,
    },
}

impl AlertEvent {
    /// The localized headline and body for the notification surface.
    pub fn notification_text(&self) -> (String, String) {
        match self {
            AlertEvent::Threshold { percent, window_name, .. } => (
                crate::localization::t("A limit is close").to_string(),
                crate::localization::t_fmt("{w} passed {p}", &[window_name, &format!("{percent}%")]),
            ),
            AlertEvent::Spent { window_name, .. } => (
                crate::localization::t("A limit is spent").to_string(),
                window_name.clone(),
            ),
            AlertEvent::Reset { window_name, .. } => (
                crate::localization::t("A warned window came back").to_string(),
                window_name.clone(),
            ),
            AlertEvent::Failure { provider_name, .. } => (
                crate::localization::t("Checks keep failing").to_string(),
                provider_name.clone(),
            ),
            AlertEvent::LowBalance { provider_name, text, .. } => (
                crate::localization::t("Low balance").to_string(),
                format!("{provider_name}: {text}"),
            ),
        }
    }
}

/// What has already been said, so nothing is said twice.
pub struct AlertMemory {
    /// (account, window id) -> the last percent that was announced.
    warned: HashMap<(String, String), i64>,
    spent_announced: HashSet<(String, String)>,
    /// Windows that were warned and have since come back, awaiting the
    /// reset announcement.
    warned_windows: HashSet<(String, String)>,
    failure_streaks: HashMap<String, u32>,
    failure_announced: HashSet<String>,
    low_balance_announced: HashSet<String>,
    readings: HashMap<String, ProviderUsage>,
}

const FAILURE_STREAK_LIMIT: u32 = 3;

impl AlertMemory {
    pub fn new() -> AlertMemory {
        AlertMemory {
            warned: HashMap::new(),
            spent_announced: HashSet::new(),
            warned_windows: HashSet::new(),
            failure_streaks: HashMap::new(),
            failure_announced: HashSet::new(),
            low_balance_announced: HashSet::new(),
            readings: HashMap::new(),
        }
    }

    pub fn last_reading(&self, account: &AccountKey) -> Option<ProviderUsage> {
        self.readings.get(&account.id()).cloned()
    }

    pub fn remember_reading(&mut self, account: &AccountKey, reading: &ProviderUsage) {
        self.readings.insert(account.id(), reading.clone());
    }
}

pub fn evaluate(
    memory: &mut AlertMemory,
    old: Option<&ProviderUsage>,
    new: Option<&ProviderUsage>,
) -> Vec<AlertEvent> {
    let Some(new) = new else { return Vec::new() };
    let account = new.account.clone();

    let settings_on = crate::settings::with(|s| {
        (
            s.wants_alerts,
            s.alert_threshold,
            s.alerts_on_reset,
            s.alerts_on_failure,
            s.low_balance_alerts
                .get(&account.id())
                .copied()
                .unwrap_or(f64::NEG_INFINITY),
        )
    });
    let (wants_alerts, threshold, alerts_on_reset, alerts_on_failure, balance_floor) = settings_on;

    let mut events = Vec::new();

    // Cache-restored readings are history, not news: only something this
    // pass actually fetched may speak.
    if wants_alerts && !new.is_cached && matches!(new.state, State::Live) {
        for window in &new.windows {
            events.extend(evaluate_window(memory, &account, window, threshold));
        }

        // A warned window that has come back: the previous reading held a
        // warning for a window id that now reads comfortably under the line.
        if alerts_on_reset {
            events.extend(reset_events(memory, old, new));
        }

        // The balance floor, for the services that sell prepaid credit.
        if balance_floor.is_finite() {
            if let Some(credit) = &new.credit_remaining {
                let key = account.id();
                if credit.amount < balance_floor && !memory.low_balance_announced.contains(&key) {
                    memory.low_balance_announced.insert(key);
                    events.push(AlertEvent::LowBalance {
                        account: account.clone(),
                        provider_name: new.provider().display_name().to_string(),
                        text: credit.rail_text(),
                    });
                } else if credit.amount >= balance_floor {
                    // Back above the line: the next fall below may be said
                    // again.
                    memory.low_balance_announced.remove(&key);
                }
            }
        }
    }

    // A failed check is a *result*, so the streak counts regardless of the
    // alerts switch for readings — but is only **announced** when the
    // setting is on, and config states are not failures.
    if matches!(new.state, State::Unavailable(_)) && !new.is_cached {
        let is_config_state = matches!(
            new.state,
            State::Unavailable(
                Unavailability::ApiKeyMissing
                    | Unavailability::SignedOut
                    | Unavailability::NotSignedIn
                    | Unavailability::NotOnWindows
            )
        );
        let streak = memory.failure_streaks.entry(account.id()).or_insert(0);
        if is_config_state {
            *streak = 0;
        } else {
            *streak += 1;
            if alerts_on_failure
                && *streak == FAILURE_STREAK_LIMIT
                && memory.failure_announced.insert(account.id())
            {
                events.push(AlertEvent::Failure {
                    account: account.clone(),
                    provider_name: new.provider().display_name().to_string(),
                });
            }
        }
    } else if !new.is_cached {
        // An answer, any answer, ends the streak — and re-arms the
        // announcement for the next one.
        memory.failure_streaks.insert(account.id(), 0);
        memory.failure_announced.remove(&account.id());
    }

    events
}

fn evaluate_window(
    memory: &mut AlertMemory,
    account: &AccountKey,
    window: &UsageWindow,
    threshold: i64,
) -> Vec<AlertEvent> {
    let key = (account.id(), window.id.clone());
    let mut events = Vec::new();
    let percent = window.percent_value(false);

    // Spent is the provider's own word, well ahead of 100% sometimes.
    if window.is_exhausted {
        if memory.spent_announced.insert(key.clone()) {
            memory.warned.remove(&key);
            events.push(AlertEvent::Spent {
                account: account.clone(),
                window_name: window.display_name(),
            });
        }
        return events;
    }
    memory.spent_announced.remove(&key);

    // The threshold. A limit already past the line when this first runs is
    // announced (no entry yet counts as never warned), then never again
    // until it resets or gets worse past the line.
    if percent >= threshold {
        let last = memory.warned.get(&key).copied().unwrap_or(0);
        if percent > last {
            memory.warned.insert(key.clone(), percent);
            memory.warned_windows.insert(key.clone());
            events.push(AlertEvent::Threshold {
                account: account.clone(),
                window_id: window.id.clone(),
                window_name: window.display_name(),
                percent,
            });
        }
    } else if memory.warned.remove(&key).is_some() {
        // It came back under the line on its own; the reset rule below may
        // want to speak, and the next crossing can warn again.
        memory.warned_windows.remove(&key);
    }

    events
}

/// A reset is announced only on unambiguous evidence: the same window id,
/// previously warned or spent, now reading well under where it was — and the
/// reset time, if both readings carry one, having moved forward.
fn reset_events(
    memory: &mut AlertMemory,
    old: Option<&ProviderUsage>,
    new: &ProviderUsage,
) -> Vec<AlertEvent> {
    let Some(old) = old else { return Vec::new() };
    let mut events = Vec::new();

    for window in &new.windows {
        let key = (new.account.id(), window.id.clone());
        let Some(old_window) = old.windows.iter().find(|w| w.id == window.id) else {
            continue;
        };

        let was_warned = memory.warned_windows.contains(&key)
            || memory.spent_announced.contains(&key)
            || old_window.percent_value(false) >= 75
            || old_window.is_exhausted;
        let now_comfortable =
            window.percent_value(false) < old_window.percent_value(false) && !window.is_exhausted;
        let reset_advanced = match (old_window.resets_at, window.resets_at) {
            (Some(old_at), Some(new_at)) => new_at > old_at,
            _ => true,
        };

        if was_warned && now_comfortable && reset_advanced && memory.warned_windows.remove(&key) {
            memory.spent_announced.remove(&key);
            memory.warned.remove(&key);
            events.push(AlertEvent::Reset {
                account: new.account.clone(),
                window_name: window.display_name(),
            });
        }
    }

    events
}
