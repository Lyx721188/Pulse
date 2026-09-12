//! The refresh engine, ported from `UsageStore` and `AdaptiveRefresh`.
//!
//! The interval is **adaptive by default** — 2–30 minutes — and it is a
//! one-shot schedule, not a repeating timer. Each account has its own
//! cadence and a pass asks only the accounts that are actually **due**
//! (asked, not answered: a provider that refuses every time must not read as
//! permanently due and spin the loop). A fixed interval chosen in Settings
//! applies to everything equally.
//!
//! The one asymmetry: providers this machine cannot watch (prepaid credit
//! draining on somebody else's servers) are blind to every activity signal
//! and would land on the ceiling for ever — so their wait is capped at
//! `UNWATCHED_CEILING`.

use crate::cache::UsageCache;
use crate::model::{AccountKey, Provider, ProviderUsage, State, UsageWindow};
use crate::providers::{DeepSeekBasis, KeyRing, Services};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

pub const FLOOR: i64 = 120;
pub const CEILING: i64 = 1800;
pub const UNWATCHED_CEILING: i64 = 300;

#[derive(Debug, Clone)]
pub enum Command {
    /// Everything enabled, whatever the cadence says.
    RefreshAll,
    /// One account, by name — a Settings pane may refresh a provider while
    /// it is switched off.
    RefreshAccount(AccountKey),
    SettingsChanged,
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum Update {
    /// Fresh or cache-reconciled readings.
    Readings(Vec<ProviderUsage>),
    Alert(crate::alerts::AlertEvent),
}

struct AccountState {
    /// When this account was last **asked**, not answered — a provider that
    /// refuses every time must not read as permanently due and spin.
    asked_at: i64,
    /// Consecutive passes whose reported windows did not move.
    stable_passes: u32,
    last_windows: Option<Vec<UsageWindow>>,
}

pub struct StoreHandle {
    tx: Sender<Command>,
    rx: Receiver<Update>,
}

impl StoreHandle {
    pub fn start() -> StoreHandle {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (upd_tx, upd_rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("pulse-store".into())
            .spawn(move || worker(cmd_rx, upd_tx))
            .expect("spawn store worker");
        StoreHandle {
            tx: cmd_tx,
            rx: upd_rx,
        }
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }

    /// Drains one pending update, if any. The UI polls this on its timer.
    pub fn poll(&self) -> Option<Update> {
        self.rx.try_recv().ok()
    }
}

fn worker(cmd_rx: Receiver<Command>, upd_tx: Sender<Update>) {
    let services = Arc::new(Services::new());
    let mut cache = UsageCache::new();
    let mut alerts = crate::alerts::AlertMemory::new();
    let mut state: HashMap<String, AccountState> = HashMap::new();
    let mut keys = KeyRing::load();
    let mut next_pass_at = crate::timeutil::now_ms();

    loop {
        let now = crate::timeutil::now_ms();
        let wait = (next_pass_at - now).clamp(1, 1000) as u64;

        match cmd_rx.recv_timeout(std::time::Duration::from_millis(wait)) {
            Ok(Command::Shutdown) => return,
            Ok(Command::RefreshAll) => {
                run_pass(&services, &mut cache, &mut alerts, &mut state, &keys, &upd_tx, None);
                next_pass_at = schedule_next(&state);
            }
            Ok(Command::RefreshAccount(account)) => {
                run_pass(
                    &services,
                    &mut cache,
                    &mut alerts,
                    &mut state,
                    &keys,
                    &upd_tx,
                    Some(vec![account]),
                );
                next_pass_at = schedule_next(&state);
            }
            Ok(Command::SettingsChanged) => {
                keys = KeyRing::load();
                // Something happening is a reason to look now, whatever the
                // cadence says — switching DeepSeek's basis changes what the
                // reading means, and a pass that skipped the provider left
                // the old ring on screen.
                next_pass_at = crate::timeutil::now_ms();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }

        // The schedule is one-shot: `schedule_next` runs only when a pass
        // finishes, so a stalled pass cannot take the cadence with it.
        if crate::timeutil::now_ms() >= next_pass_at {
            run_pass(&services, &mut cache, &mut alerts, &mut state, &keys, &upd_tx, None);
            next_pass_at = schedule_next(&state);
        }
    }
}

/// Which accounts the timed pass asks: everything enabled that is due.
fn due_accounts(state: &HashMap<String, AccountState>) -> Vec<AccountKey> {
    let now = crate::timeutil::now_ms();
    crate::settings::with(|settings| {
        settings
            .ordered_enabled()
            .into_iter()
            .filter(|account| match state.get(&account.id()) {
                Some(entry) => {
                    let interval = interval_for(settings, &account.provider, entry.stable_passes);
                    now - entry.asked_at >= interval * 1000
                }
                None => true,
            })
            .collect()
    })
}

fn interval_for(
    settings: &crate::settings::AppSettings,
    provider: &Provider,
    stable_passes: u32,
) -> i64 {
    // A fixed interval chosen in Settings applies to everything equally:
    // somebody who picked five minutes meant five minutes.
    if settings.refresh_interval > 0 {
        return settings.refresh_interval;
    }
    let ceiling = if provider.spending_is_watched_locally() {
        CEILING
    } else {
        UNWATCHED_CEILING
    };
    ladder_interval(stable_passes, ceiling)
}

/// The adaptive ladder. Every signal here is a reason to wait **longer**,
/// never shorter: an unchanged reading climbs, a changed one drops back to
/// the floor. The cap only ever lowers a wait the ladder already decided.
fn ladder_interval(stable_passes: u32, ceiling: i64) -> i64 {
    const LADDER: [i64; 7] = [120, 180, 300, 480, 720, 1200, 1800];
    let index = (stable_passes as usize).min(LADDER.len() - 1);
    LADDER[index].min(ceiling).max(FLOOR)
}

/// When the next pass is due: whichever account's own cadence ends first.
fn schedule_next(state: &HashMap<String, AccountState>) -> i64 {
    let now = crate::timeutil::now_ms();
    crate::settings::with(|settings| {
        let mut earliest = now + FLOOR * 1000;
        for account in settings.ordered_enabled() {
            let interval = match state.get(&account.id()) {
                Some(entry) => {
                    interval_for(settings, &account.provider, entry.stable_passes)
                }
                None => FLOOR,
            };
            let due_at = state
                .get(&account.id())
                .map(|s| s.asked_at + interval * 1000)
                .unwrap_or(now);
            earliest = earliest.min(due_at);
        }
        earliest
    })
}

/// One pass. `only` scopes it to named accounts; otherwise it is every
/// enabled account that is due. Each account fetches on its own thread, and
/// everything not asked keeps the reading it has, because the commit loop is
/// gated on the same set.
fn run_pass(
    services: &Arc<Services>,
    cache: &mut UsageCache,
    alerts: &mut crate::alerts::AlertMemory,
    state: &mut HashMap<String, AccountState>,
    keys: &KeyRing,
    upd_tx: &Sender<Update>,
    only: Option<Vec<AccountKey>>,
) {
    let accounts: Vec<AccountKey> = match only {
        Some(list) => list
            .into_iter()
            .filter(|a| a.provider.is_ported_to_windows())
            .collect(),
        None => due_accounts(state),
    };
    if accounts.is_empty() {
        return;
    }

    let now = crate::timeutil::now_ms();
    for account in &accounts {
        state
            .entry(account.id())
            .or_insert_with(|| AccountState {
                asked_at: 0,
                stable_passes: 0,
                last_windows: None,
            })
            .asked_at = now;
    }

    let deepseek_basis = crate::settings::with(|s| DeepSeekBasis {
        basis: s.deepseek_basis.clone(),
        budget: s.deepseek_budget,
        currency: s.deepseek_currency.clone(),
    });

    let mut handles = Vec::new();
    for account in &accounts {
        let ring = KeyRing {
            api_keys: keys.api_keys.clone(),
            copilot_token: keys.copilot_token.clone(),
        };
        let account = account.clone();
        let basis = deepseek_basis.clone();
        let services = services.clone();
        handles.push((
            account.clone(),
            std::thread::spawn(move || {
                if account.provider == Provider::DeepSeek {
                    return services.deepseek.fetch_with_basis(&ring, &basis);
                }
                match services.for_provider(account.provider) {
                    Some(service) => service.fetch(&ring),
                    None => ProviderUsage::unavailable(
                        account,
                        crate::model::Unavailability::NotOnWindows,
                    ),
                }
            }),
        ));
    }

    let mut results: Vec<ProviderUsage> = Vec::new();
    for (account, handle) in handles {
        let fetched = handle.join().unwrap_or_else(|_| {
            ProviderUsage::unavailable(account.clone(), crate::model::Unavailability::Unreachable)
        });

        // Every fetched reading goes through the cache: a refusal shows the
        // last good figures with a date, and a live reading banks.
        let reconciled = cache.reconciled(fetched);

        let old = state.get(&account.id()).and_then(|entry| entry.last_windows.clone());
        let old_reading = readings_snapshot(alerts, &account);
        let events = crate::alerts::evaluate(alerts, old_reading.as_ref(), Some(&reconciled));
        let _ = old;
        for event in events {
            let _ = upd_tx.send(Update::Alert(event));
        }

        record_pass(state, &account, &reconciled);
        readings_insert(alerts, &account, &reconciled);
        results.push(reconciled);
    }

    if !results.is_empty() {
        let _ = upd_tx.send(Update::Readings(results));
    }
}

/// Whether this pass's windows actually moved. `observed_at` is ignored for
/// the comparison — every fetch would otherwise look like a change. Only a
/// live reading moves the ladder; a refusal leaves the cadence alone.
fn record_pass(
    state: &mut HashMap<String, AccountState>,
    account: &AccountKey,
    reading: &ProviderUsage,
) {
    let entry = state
        .entry(account.id())
        .or_insert_with(|| AccountState {
            asked_at: crate::timeutil::now_ms(),
            stable_passes: 0,
            last_windows: None,
        });
    if !matches!(reading.state, State::Live) {
        return;
    }
    let moved = match &entry.last_windows {
        None => true,
        Some(previous) => previous != &reading.windows,
    };
    entry.stable_passes = if moved { 0 } else { entry.stable_passes + 1 };
    entry.last_windows = Some(reading.windows.clone());
}

// The alert rules need the *previous reading*, not just its windows; the
// state map above carries windows for pacing, and this map carries the
// reading for the rules. Kept on the alert memory itself so one structure
// owns one concern.
fn readings_snapshot(
    alerts: &mut crate::alerts::AlertMemory,
    account: &AccountKey,
) -> Option<ProviderUsage> {
    alerts.last_reading(account)
}

fn readings_insert(alerts: &mut crate::alerts::AlertMemory, account: &AccountKey, reading: &ProviderUsage) {
    alerts.remember_reading(account, reading);
}

/// What the panel paints the moment it opens, before the first round trip.
pub fn initial_readings_from_cache() -> Vec<ProviderUsage> {
    let mut cache = UsageCache::new();
    crate::settings::with(|settings| {
        settings
            .ordered_enabled()
            .into_iter()
            .filter_map(|account| cache.reading(&account))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    //! Refresh pacing, ported from `RefreshPacingTests`. Every signal the
    //! ladder reads is local; a provider billed entirely on its own servers
    //! is invisible to all of them and lands on the cap every time.

    use super::{ladder_interval, CEILING, FLOOR, UNWATCHED_CEILING};

    #[test]
    fn an_unchanged_reading_climbs_the_ladder_to_the_ceiling() {
        assert_eq!(ladder_interval(0, CEILING), 120);
        assert_eq!(ladder_interval(1, CEILING), 180);
        assert_eq!(ladder_interval(3, CEILING), 480);
        assert_eq!(ladder_interval(5, CEILING), 1200);
        assert_eq!(ladder_interval(6, CEILING), CEILING);
        assert_eq!(ladder_interval(50, CEILING), CEILING);
    }

    #[test]
    fn a_provider_this_machine_cannot_watch_is_capped_well_short() {
        // Six stable passes would sit at the top of the ladder, but a
        // provider billed on its own servers never earns it: nothing
        // appears to change because nothing local can see it move.
        assert_eq!(ladder_interval(50, UNWATCHED_CEILING), UNWATCHED_CEILING);
        assert!(UNWATCHED_CEILING < CEILING);
    }

    #[test]
    fn the_cap_only_ever_lowers_a_wait() {
        for passes in [0u32, 1, 2, 3, 5, 7, 50] {
            let watched = ladder_interval(passes, CEILING);
            let unwatched = ladder_interval(passes, UNWATCHED_CEILING);
            assert!(unwatched <= watched, "cap raised the wait at {passes}");
        }
    }

    #[test]
    fn no_interval_ever_drops_below_the_floor() {
        for passes in [0u32, 1, 4, 9] {
            assert!(ladder_interval(passes, FLOOR) >= FLOOR);
        }
    }
}
