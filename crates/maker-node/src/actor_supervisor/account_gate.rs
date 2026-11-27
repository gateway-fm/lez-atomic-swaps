//! The Maker's LEZ locks in flight, for the one gate every LEZ transaction of
//! its account passes (ADR 0219).
//!
//! A Maker Node signs with one LEZ account: its Zcash follow-up LEZ claims,
//! its Zcash and Bitcoin LEZ locks (the escrow's initialization and funding)
//! and its Monero punishment. Each is planned at the account's live nonce,
//! so two planned before the first is in a block carry the same nonce and
//! the sequencer keeps one.
//!
//! The claims' side of the gate is kept in marker files beside each Zcash
//! swap's state (the actor writes one of them). The locks' side is this
//! registry, in memory: every lock is admitted by this process, and a Node
//! that restarts forgets at most one lock in flight, which costs the next
//! transaction one re-plan.

use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

/// One gate decision at a time in this process: a claim's and a lock's
/// admission never interleave, so neither is planned while the other is being
/// admitted.
pub(super) static LEZ_ACCOUNT_GATE: Mutex<()> = Mutex::new(());

/// How long one lock in flight holds the other locks at most, from its first
/// admission: two minutes. A lock's transactions are in a block within
/// seconds of a healthy sequencer; one still unfinished then is waiting for
/// LEZ finality, which takes no nonce. Past the limit the other locks are
/// planned beside it, and a transaction that loses its nonce is planned again.
pub(super) const LEZ_LOCK_HOLD_LIMIT: Duration = Duration::from_secs(120);

/// How long a Bitcoin LEZ escrow the Node prepared holds the other locks
/// after each pass of its preparer that still finds it in flight (three
/// passes, five seconds apart).
pub(crate) const PREPARED_ESCROW_HOLD: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug)]
struct LockInFlight {
    /// The first admission of this unbroken hold.
    since: Instant,
    /// The other locks wait until then.
    hold_until: Instant,
}

/// The locks this process admitted and has not seen finished, by swap id.
#[derive(Debug, Default)]
pub(super) struct LocksInFlight {
    locks: HashMap<String, LockInFlight>,
}

impl LocksInFlight {
    fn holds(lock: &LockInFlight, now: Instant) -> bool {
        now < lock.hold_until && now.saturating_duration_since(lock.since) < LEZ_LOCK_HOLD_LIMIT
    }

    /// Another swap's lock that holds `swap_id`'s at `now`, if any.
    pub(super) fn holder(&self, swap_id: &str, now: Instant) -> Option<&str> {
        self.locks
            .iter()
            .find(|(holder, lock)| holder.as_str() != swap_id && Self::holds(lock, now))
            .map(|(holder, _)| holder.as_str())
    }

    /// Whether `swap_id`'s own lock is recorded in flight at `now`.
    pub(super) fn in_flight(&self, swap_id: &str, now: Instant) -> bool {
        self.locks
            .get(swap_id)
            .is_some_and(|lock| now < lock.hold_until)
    }

    /// Records `swap_id`'s lock as in flight until `now + hold`. A hold that
    /// was still running goes on from its first admission, so the limit
    /// counts from there.
    pub(super) fn hold(&mut self, swap_id: &str, now: Instant, hold: Duration) {
        self.locks.retain(|_, lock| now < lock.hold_until);
        let since = self.locks.get(swap_id).map_or(now, |lock| lock.since);
        self.locks.insert(
            swap_id.to_owned(),
            LockInFlight {
                since,
                hold_until: now + hold,
            },
        );
    }

    /// Forgets `swap_id`'s lock: its transactions are in a block, or its
    /// attempt sent none.
    pub(super) fn release(&mut self, swap_id: &str) {
        self.locks.remove(swap_id);
    }
}

static LOCKS_IN_FLIGHT: std::sync::LazyLock<Mutex<HashMap<String, LocksInFlight>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Runs `read` on the registry of the Node whose store is `store`: a Node is
/// one store and one LEZ account, and a process holds one Node (tests hold
/// several, each with its own store).
pub(super) fn with_locks_in_flight<T>(
    store: &lez_swap_store::SqliteSwapStore,
    read: impl FnOnce(&mut LocksInFlight) -> T,
) -> T {
    let node = store.database_path().unwrap_or_default();
    read(
        LOCKS_IN_FLIGHT
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(node)
            .or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lock_in_flight_holds_the_other_locks_for_its_hold_and_never_itself() {
        let start = Instant::now();
        let mut locks = LocksInFlight::default();
        assert_eq!(locks.holder("b", start), None);
        locks.hold("a", start, Duration::from_secs(5));
        assert_eq!(locks.holder("b", start), Some("a"));
        assert_eq!(locks.holder("a", start), None, "a lock never holds itself");
        assert!(locks.in_flight("a", start));
        assert!(!locks.in_flight("b", start));
        assert_eq!(
            locks.holder("b", start + Duration::from_secs(5)),
            None,
            "the hold ends by itself"
        );
        locks.hold("a", start, Duration::from_secs(5));
        locks.release("a");
        assert_eq!(locks.holder("b", start), None);
    }

    #[test]
    fn a_renewed_lock_holds_the_others_no_longer_than_the_limit() {
        // A lock waiting out LEZ finality renews its hold at every attempt;
        // the others are planned beside it once the limit has passed.
        let start = Instant::now();
        let mut locks = LocksInFlight::default();
        let step = Duration::from_secs(10);
        let mut now = start;
        while now.saturating_duration_since(start) < LEZ_LOCK_HOLD_LIMIT {
            locks.hold("a", now, step * 2);
            assert_eq!(locks.holder("b", now), Some("a"));
            now += step;
        }
        locks.hold("a", now, step * 2);
        assert_eq!(locks.holder("b", now), None);
        assert!(locks.in_flight("a", now), "it is still this swap's lock");
        // Once its hold lapsed, a later lock of the same swap starts afresh.
        let later = now + step * 3;
        locks.hold("a", later, step);
        assert_eq!(locks.holder("b", later), Some("a"));
    }
}
