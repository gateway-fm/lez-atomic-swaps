//! How many times one LEZ first-lock step went out without being included.
//!
//! The official LEZ v0.2.4 sequencer admits any transaction to its mempool and
//! drops one that fails execution at block building without telling the
//! submitter. A drive that finds its step absent submits the same bytes again,
//! so a step that can never execute would be resubmitted on every drive, in
//! silence, until the swap's deadlines pass (lanehang, 2026-11-11: two swaps
//! with one swap id; the second's escrow initialization was dropped 88 times
//! in ten minutes while the row said `awaiting_first_lock`).
//!
//! The actor keeps one record per swap beside its state
//! (`<swap>/actor/<role>/lez-first-lock-attempts.json`): the step, its
//! transaction id, what the last attempts returned, how many there were and
//! since when. It is replaced when the step or its bytes change and removed as
//! soon as a drive sees the step on the chain, so a healthy swap's record
//! never grows past a few attempts. Two rules read it:
//!
//! - the bridge refused the initialization because its escrow account already
//!   exists without any of this swap's initializations included
//!   (`ErrorCode::EscrowAlreadyInitialized`, a state the sidecar read on the
//!   chain): after [`ESCROW_REFUSAL_STOP_ATTEMPTS`] such attempts spanning at
//!   least [`ESCROW_REFUSAL_STOP_SECONDS`] the swap stops by name. The span
//!   guards a read that met the swap's own initialization half-applied;
//! - the step was submitted [`UNINCLUDED_NOTE_ATTEMPTS`] times over at least
//!   [`UNINCLUDED_NOTE_SECONDS`] and is still not included, for no reason the
//!   chain shows: nothing is decided, the drive keeps submitting, and the
//!   swap's status says so ([`LezFirstLockAttemptsV1::status_detail`]).

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// Refused attempts after which a swap whose escrow account exists without it stops.
pub const ESCROW_REFUSAL_STOP_ATTEMPTS: u32 = 5;
/// The least time those refused attempts span, seconds.
pub const ESCROW_REFUSAL_STOP_SECONDS: u64 = 60;
/// Submissions of one step without inclusion after which the status says so.
pub const UNINCLUDED_NOTE_ATTEMPTS: u32 = 10;
/// The least time those submissions span, seconds.
pub const UNINCLUDED_NOTE_SECONDS: u64 = 120;

/// What the last attempts at one step returned.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LezFirstLockAttemptOutcomeV1 {
    /// The bridge sent the bytes to the sequencer (or found them known).
    Submitted,
    /// The bridge did not send the initialization: its escrow account
    /// already exists, initialized by a transaction that is not this swap's.
    EscrowAlreadyInitialized,
}

/// The record of one step's attempts that have not been seen on the chain.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LezFirstLockAttemptsV1 {
    /// `lez_initialize` or `lez_fund`.
    pub step: String,
    /// The step's transaction id, hex.
    pub transaction_id: String,
    /// What every counted attempt returned.
    pub outcome: LezFirstLockAttemptOutcomeV1,
    /// Attempts counted, at least one.
    pub attempts: u32,
    /// Unix milliseconds of the first counted attempt.
    pub first_attempt_unix_ms: u64,
    /// Unix milliseconds of the last counted attempt.
    pub last_attempt_unix_ms: u64,
}

impl LezFirstLockAttemptsV1 {
    fn span_seconds(&self) -> u64 {
        self.last_attempt_unix_ms
            .saturating_sub(self.first_attempt_unix_ms)
            / 1000
    }

    /// Whether the refusal rule stops the swap.
    #[must_use]
    pub fn stops(&self) -> bool {
        self.outcome == LezFirstLockAttemptOutcomeV1::EscrowAlreadyInitialized
            && self.attempts >= ESCROW_REFUSAL_STOP_ATTEMPTS
            && self.span_seconds() >= ESCROW_REFUSAL_STOP_SECONDS
    }

    /// Whether the submissions are many and old enough to be named.
    #[must_use]
    pub fn unincluded_named(&self) -> bool {
        self.outcome == LezFirstLockAttemptOutcomeV1::Submitted
            && self.attempts >= UNINCLUDED_NOTE_ATTEMPTS
            && self.span_seconds() >= UNINCLUDED_NOTE_SECONDS
    }

    fn short_transaction_id(&self) -> &str {
        self.transaction_id
            .get(..16)
            .unwrap_or(&self.transaction_id)
    }

    /// One human-readable sentence for the swap's status once a rule holds;
    /// `None` while neither does.
    #[must_use]
    pub fn status_detail(&self) -> Option<String> {
        if self.stops() {
            Some(format!(
                "LEZ escrow account of this swap already exists, initialized by a transaction that is not this swap's (another swap with the same swap id?); {} {}… can never execute; refused {} times over {} s, submitting stopped",
                self.step,
                self.short_transaction_id(),
                self.attempts,
                self.span_seconds()
            ))
        } else if self.unincluded_named() {
            Some(format!(
                "{} {}… submitted {} times over {} s, not included yet; the chain shows no reason, submitting continues",
                self.step,
                self.short_transaction_id(),
                self.attempts,
                self.span_seconds()
            ))
        } else {
            None
        }
    }
}

/// The record's path beside the actor's state.
#[must_use]
pub fn lez_first_lock_attempts_path(role_state_db: &Path) -> Option<PathBuf> {
    Some(
        role_state_db
            .parent()?
            .parent()?
            .join("lez-first-lock-attempts.json"),
    )
}

/// The record, when there is a well-formed one.
#[must_use]
pub fn lez_first_lock_attempts(role_state_db: &Path) -> Option<LezFirstLockAttemptsV1> {
    let bytes = fs::read(lez_first_lock_attempts_path(role_state_db)?).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Counts one attempt: the same step, bytes and outcome add to the record,
/// anything else starts a new one. Returns the record as written (also when
/// it could not be written: the rule then sees one attempt at a time, and a
/// swap only stops later or not at all).
pub(crate) fn record_lez_first_lock_attempt(
    role_state_db: &Path,
    step: &str,
    transaction_id: &str,
    outcome: LezFirstLockAttemptOutcomeV1,
    now_unix_ms: u64,
) -> LezFirstLockAttemptsV1 {
    let record = match lez_first_lock_attempts(role_state_db) {
        Some(previous)
            if previous.step == step
                && previous.transaction_id == transaction_id
                && previous.outcome == outcome =>
        {
            LezFirstLockAttemptsV1 {
                attempts: previous.attempts.saturating_add(1),
                last_attempt_unix_ms: now_unix_ms.max(previous.last_attempt_unix_ms),
                ..previous
            }
        }
        _ => LezFirstLockAttemptsV1 {
            step: step.to_owned(),
            transaction_id: transaction_id.to_owned(),
            outcome,
            attempts: 1,
            first_attempt_unix_ms: now_unix_ms,
            last_attempt_unix_ms: now_unix_ms,
        },
    };
    if let (Some(path), Ok(json)) = (
        lez_first_lock_attempts_path(role_state_db),
        serde_json::to_vec(&record),
    ) {
        let partial = path.with_extension("json.partial");
        if fs::write(&partial, json).is_ok() {
            let _ = fs::rename(&partial, &path);
        }
    }
    record
}

/// Removes the record: the step was seen on the chain, or the drive moved on.
pub(crate) fn clear_lez_first_lock_attempts(role_state_db: &Path) {
    if let Some(path) = lez_first_lock_attempts_path(role_state_db) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ESCROW_REFUSAL_STOP_ATTEMPTS, LezFirstLockAttemptOutcomeV1, UNINCLUDED_NOTE_ATTEMPTS,
        clear_lez_first_lock_attempts, lez_first_lock_attempts, record_lez_first_lock_attempt,
    };

    const REFUSED: LezFirstLockAttemptOutcomeV1 =
        LezFirstLockAttemptOutcomeV1::EscrowAlreadyInitialized;
    const SUBMITTED: LezFirstLockAttemptOutcomeV1 = LezFirstLockAttemptOutcomeV1::Submitted;

    fn state_db(root: &std::path::Path) -> std::path::PathBuf {
        let state = root.join("actor/taker/state");
        std::fs::create_dir_all(&state).expect("state directory");
        state.join("zec.sqlite3")
    }

    #[test]
    fn a_refusal_stops_only_after_enough_attempts_over_enough_time() {
        let root = tempfile::tempdir().expect("tempdir");
        let db = state_db(root.path());
        // Five refusals within a few seconds: a read that met the swap's own
        // initialization half-applied would look like this; no stop yet.
        for second in 0..u64::from(ESCROW_REFUSAL_STOP_ATTEMPTS) {
            let record =
                record_lez_first_lock_attempt(&db, "lez_initialize", "ab", REFUSED, second * 1000);
            assert!(!record.stops());
        }
        // Still refused a minute after the first: the swap stops by name.
        let record = record_lez_first_lock_attempt(&db, "lez_initialize", "ab", REFUSED, 60_000);
        assert_eq!(record.attempts, ESCROW_REFUSAL_STOP_ATTEMPTS + 1);
        assert!(record.stops());
        let detail = record.status_detail().expect("named");
        assert!(detail.contains("already exists"), "{detail}");
        assert!(detail.contains("submitting stopped"), "{detail}");
        assert_eq!(lez_first_lock_attempts(&db), Some(record));
    }

    #[test]
    fn a_refusal_over_a_long_time_but_few_attempts_does_not_stop() {
        let root = tempfile::tempdir().expect("tempdir");
        let db = state_db(root.path());
        record_lez_first_lock_attempt(&db, "lez_initialize", "ab", REFUSED, 0);
        let record = record_lez_first_lock_attempt(&db, "lez_initialize", "ab", REFUSED, 600_000);
        assert!(!record.stops());
        assert_eq!(record.status_detail(), None);
    }

    #[test]
    fn a_slow_inclusion_is_named_after_the_threshold_and_never_stops() {
        let root = tempfile::tempdir().expect("tempdir");
        let db = state_db(root.path());
        let mut record = None;
        for attempt in 0..u64::from(UNINCLUDED_NOTE_ATTEMPTS) {
            record = Some(record_lez_first_lock_attempt(
                &db,
                "lez_initialize",
                "0123456789abcdef0123",
                SUBMITTED,
                attempt * 6_000,
            ));
        }
        // Ten submissions over 54 s: below the time threshold, nothing said.
        let record = record.expect("recorded");
        assert_eq!(record.status_detail(), None);
        let record = record_lez_first_lock_attempt(
            &db,
            "lez_initialize",
            "0123456789abcdef0123",
            SUBMITTED,
            120_000,
        );
        assert!(!record.stops());
        let detail = record.status_detail().expect("named");
        assert_eq!(
            detail,
            "lez_initialize 0123456789abcdef… submitted 11 times over 120 s, not included yet; the chain shows no reason, submitting continues"
        );
    }

    #[test]
    fn another_step_bytes_or_outcome_starts_over_and_inclusion_clears() {
        let root = tempfile::tempdir().expect("tempdir");
        let db = state_db(root.path());
        record_lez_first_lock_attempt(&db, "lez_initialize", "ab", SUBMITTED, 0);
        record_lez_first_lock_attempt(&db, "lez_initialize", "ab", SUBMITTED, 5_000);
        let refused = record_lez_first_lock_attempt(&db, "lez_initialize", "ab", REFUSED, 10_000);
        assert_eq!(
            (refused.attempts, refused.first_attempt_unix_ms),
            (1, 10_000)
        );
        let replanned = record_lez_first_lock_attempt(&db, "lez_initialize", "cd", REFUSED, 15_000);
        assert_eq!(replanned.attempts, 1);
        let fund = record_lez_first_lock_attempt(&db, "lez_fund", "ef", SUBMITTED, 20_000);
        assert_eq!(fund.attempts, 1);
        clear_lez_first_lock_attempts(&db);
        assert_eq!(lez_first_lock_attempts(&db), None);
    }
}
