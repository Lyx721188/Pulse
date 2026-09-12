//! The last good reading from each account, ported from `UsageCache.swift`.
//!
//! Two rules keep it honest:
//! - Only `.live` readings are ever stored, and they come back marked
//!   `.stale`. A cached reading is never passed off as current.
//! - A window whose reset time has **passed** is dropped rather than shown —
//!   it has reset, not aged.

use crate::model::{AccountKey, ProviderUsage, State, Unavailability};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// A backstop for windows that never say when they reset.
const MAXIMUM_AGE_MS: i64 = 24 * 3600 * 1000;

#[derive(Serialize, Deserialize, Default)]
struct StoredMap {
    #[serde(flatten)]
    entries: HashMap<String, Stored>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    windows: Vec<crate::model::UsageWindow>,
    observed_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    credit_balance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    credit_remaining: Option<crate::model::CreditAmount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
}

pub struct UsageCache {
    file: PathBuf,
    readings: Option<HashMap<String, Stored>>,
}

impl UsageCache {
    pub fn new() -> Self {
        UsageCache {
            file: crate::data_dir().join("last-readings.json"),
            readings: None,
        }
    }

    /// Returns the reading worth showing: the fetched one when it carries
    /// anything, otherwise whatever was last banked — and, when both have
    /// something, whichever was actually taken later.
    pub fn reconciled(&mut self, fetched: ProviderUsage) -> ProviderUsage {
        if matches!(fetched.state, State::Live) && fetched.reports_something() {
            // `.live` is not the same as "newest": a route can call its own
            // capture live while an earlier reading banked later.
            if let Some(banked) = self.reading(&fetched.account) {
                if let (Some(banked_at), Some(taken_at)) = (banked.observed_at, fetched.observed_at)
                {
                    if taken_at < banked_at {
                        return banked;
                    }
                }
            }
            self.store(&fetched);
            return fetched;
        }

        // A missing credential is not a stumble to be papered over: showing
        // yesterday's percentages while Settings holds an empty field hides
        // the one thing the user needs told.
        if let State::Unavailable(reason) = &fetched.state {
            if matches!(
                reason,
                Unavailability::ApiKeyMissing
                    | Unavailability::SignedOut
                    | Unavailability::NotSignedIn
            ) {
                return fetched;
            }
        }

        let Some(cached) = self.reading(&fetched.account) else {
            return fetched;
        };

        // A fetch that came back with something no older than the cache wins;
        // this only fills gaps, it never overrules a real answer.
        if let (Some(fetched_at), Some(cached_at)) = (fetched.observed_at, cached.observed_at) {
            if fetched_at >= cached_at && fetched.reports_something() {
                return fetched;
            }
        }

        cached
    }

    fn store(&mut self, usage: &ProviderUsage) {
        let mut all = self.load().clone();
        all.insert(
            usage.account.id(),
            Stored {
                windows: usage.windows.clone(),
                observed_at: usage.observed_at.unwrap_or_else(crate::timeutil::now_ms),
                plan: usage.plan.clone(),
                credit_balance: usage.credit_balance.clone(),
                credit_remaining: usage.credit_remaining.clone(),
                origin: usage.origin.clone(),
            },
        );
        self.write(all);
    }

    /// The last good reading for an account, if there is one worth showing.
    /// It comes back `.stale`, so the card says when it was taken.
    pub fn reading(&mut self, account: &AccountKey) -> Option<ProviderUsage> {
        let stored = self.load().get(&account.id()).cloned()?;

        let now = crate::timeutil::now_ms();
        if now - stored.observed_at > MAXIMUM_AGE_MS {
            return None;
        }

        // Drop what has since reset — it is a number about a window that no
        // longer exists.
        let windows: Vec<_> = stored
            .windows
            .into_iter()
            .filter(|w| w.resets_at.unwrap_or(i64::MAX) > now)
            .collect();

        // A balance with no limits is a complete answer (DeepSeek, balance
        // only); an account with neither is not.
        if windows.is_empty() && stored.credit_balance.is_none() {
            return None;
        }

        Some(ProviderUsage {
            account: account.clone(),
            windows,
            observed_at: Some(stored.observed_at),
            state: State::Stale,
            plan: stored.plan,
            credit_balance: stored.credit_balance,
            credit_remaining: stored.credit_remaining,
            origin: stored.origin,
            is_cached: true,
        })
    }

    fn load(&mut self) -> &mut HashMap<String, Stored> {
        if self.readings.is_none() {
            self.readings = Some(
                std::fs::read_to_string(&self.file)
                    .ok()
                    .and_then(|text| serde_json::from_str::<StoredMap>(&text).ok())
                    .map(|m| m.entries)
                    .unwrap_or_default(),
            );
        }
        self.readings.as_mut().unwrap()
    }

    fn write(&mut self, all: HashMap<String, Stored>) {
        let dir = crate::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(text) = serde_json::to_string(&StoredMap {
            entries: all.clone(),
        }) {
            let tmp = self.file.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &self.file);
            }
        }
        self.readings = Some(all);
    }
}
