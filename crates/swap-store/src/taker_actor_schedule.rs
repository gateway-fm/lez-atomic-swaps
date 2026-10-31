//! The Taker Node's durable schedule for its Zcash actors: one row per taken
//! swap with the schedule state, retry time, lease generation, attempt count,
//! last failure class and the last status projection, in the Maker
//! supervisor's shape (`maker_actor_process`). Its own small `SQLite` file
//! beside the facade registry: the registry validates its exact table set, so
//! the schedule does not live in it.
//!
//! Execution authority stays with the per-swap `ActorHeldLock`; the lease
//! generation here fences the schedule's own writes, as on the Maker.

use std::path::Path;

use lez_actor_supervision::{AttemptResolution, ScheduleState, validate_failure_class};
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const APPLICATION_ID: i64 = 0x4c5a_5453; // "LZTS"
const SCHEMA_VERSION: i64 = 1;
const MAX_SWAP_ID_BYTES: usize = 128;
const MAX_PROGRESS_BYTES: usize = 4_096;
const MAX_DUE_LIMIT: usize = 128;
/// The class a row found leased at startup is requeued with: the lease died
/// with the process (the swap's lock went with it).
pub const COORDINATOR_RESTARTED: &str = "coordinator_restarted";

const CREATE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS taker_actor_schedule (
    swap_id            TEXT PRIMARY KEY NOT NULL,
    schedule_state     TEXT NOT NULL CHECK (schedule_state IN ('queued','leased','backoff','terminal','failed')),
    next_attempt_at    INTEGER NOT NULL CHECK (next_attempt_at >= 0),
    lease_generation   INTEGER NOT NULL CHECK (lease_generation >= 0),
    attempt_count      INTEGER NOT NULL CHECK (attempt_count >= 0),
    last_failure_class TEXT,
    progress_json      TEXT,
    observed_at        INTEGER,
    created_at         INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at         INTEGER NOT NULL CHECK (updated_at >= created_at)
) STRICT;
";

/// Why a schedule operation was refused.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum TakerActorScheduleError {
    /// The database file cannot be opened, created or is not this schema.
    #[error("taker actor schedule storage is unavailable")]
    StorageUnavailable,
    /// A swap id, failure class or progress snapshot is out of bounds.
    #[error("taker actor schedule input is invalid")]
    InvalidInput,
    /// The row is no longer leased at this generation.
    #[error("taker actor schedule lease conflict")]
    LeaseConflict,
}

/// The last status projection a supervisor attempt observed, kept so the
/// Node can answer a monitor read while the swap's lock is busy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TakerActorProgressSnapshotV1 {
    /// The actor's durable phase (its fixed spelling).
    pub phase: String,
    /// The actor's revision at that status.
    pub revision: u64,
    /// The actor's next action (its fixed spelling).
    pub next_action: String,
    /// The Taker view state projected from phase and next action (its fixed spelling).
    pub state: String,
    /// The terminal action the view offered, when one was (its fixed spelling).
    pub available_action: Option<String>,
    /// Unix seconds of the observation.
    pub observed_at: u64,
}

/// One supervised swap's schedule row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TakerActorScheduleRecordV1 {
    swap_id: String,
    schedule_state: ScheduleState,
    next_attempt_at: u64,
    lease_generation: u64,
    attempt_count: u64,
    last_failure_class: Option<String>,
    progress: Option<TakerActorProgressSnapshotV1>,
    updated_at: u64,
}

impl TakerActorScheduleRecordV1 {
    /// The swap.
    #[must_use]
    pub fn swap_id(&self) -> &str {
        &self.swap_id
    }

    /// Durable schedule state.
    #[must_use]
    pub const fn schedule_state(&self) -> ScheduleState {
        self.schedule_state
    }

    /// Unix seconds before which no attempt runs.
    #[must_use]
    pub const fn next_attempt_at(&self) -> u64 {
        self.next_attempt_at
    }

    /// Current lease generation.
    #[must_use]
    pub const fn lease_generation(&self) -> u64 {
        self.lease_generation
    }

    /// Bounded attempts so far.
    #[must_use]
    pub const fn attempt_count(&self) -> u64 {
        self.attempt_count
    }

    /// Why the last attempt backed off or the row failed.
    #[must_use]
    pub fn last_failure_class(&self) -> Option<&str> {
        self.last_failure_class.as_deref()
    }

    /// The last status projection, when an attempt observed one.
    #[must_use]
    pub const fn progress(&self) -> Option<&TakerActorProgressSnapshotV1> {
        self.progress.as_ref()
    }

    /// Unix seconds of the last change.
    #[must_use]
    pub const fn updated_at(&self) -> u64 {
        self.updated_at
    }
}

/// A claimed row: the generation every write of this attempt is fenced by.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TakerActorLeaseV1 {
    swap_id: String,
    generation: u64,
    attempt_count: u64,
}

impl TakerActorLeaseV1 {
    /// The swap.
    #[must_use]
    pub fn swap_id(&self) -> &str {
        &self.swap_id
    }

    /// The generation this attempt runs at.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Attempts including this one.
    #[must_use]
    pub const fn attempt_count(&self) -> u64 {
        self.attempt_count
    }
}

/// The schedule store.
#[derive(Debug)]
pub struct SqliteTakerActorSchedule {
    connection: Connection,
}

impl SqliteTakerActorSchedule {
    /// Opens the schedule, creating the file (owner-private) and its table
    /// when missing. A file with another application id or a newer schema is
    /// refused.
    ///
    /// # Errors
    ///
    /// Returns [`TakerActorScheduleError::StorageUnavailable`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self, TakerActorScheduleError> {
        let path = path.as_ref();
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let mut connection = Connection::open_with_flags(path, flags).map_err(unavailable)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        connection
            .pragma_update(None, "journal_mode", "wal")
            .map_err(unavailable)?;
        connection
            .pragma_update(None, "synchronous", "full")
            .map_err(unavailable)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(unavailable)?;
        let application: i64 = connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .map_err(unavailable)?;
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(unavailable)?;
        match (application, version) {
            (0, 0) => {
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(unavailable)?;
                transaction
                    .execute_batch(CREATE_SCHEMA)
                    .map_err(unavailable)?;
                transaction
                    .pragma_update(None, "application_id", APPLICATION_ID)
                    .map_err(unavailable)?;
                transaction
                    .pragma_update(None, "user_version", SCHEMA_VERSION)
                    .map_err(unavailable)?;
                transaction.commit().map_err(unavailable)?;
            }
            (APPLICATION_ID, SCHEMA_VERSION) => {}
            _ => return Err(TakerActorScheduleError::StorageUnavailable),
        }
        Ok(Self { connection })
    }

    /// Adds a swap as queued at generation zero; an existing row is kept
    /// unchanged (`false`).
    ///
    /// # Errors
    ///
    /// Rejects an out-of-bounds swap id or unavailable storage.
    pub fn register(&mut self, swap_id: &str, now: u64) -> Result<bool, TakerActorScheduleError> {
        validate_swap_id(swap_id)?;
        let inserted = self
            .connection
            .execute(
                "INSERT OR IGNORE INTO taker_actor_schedule
                 (swap_id, schedule_state, next_attempt_at, lease_generation, attempt_count,
                  last_failure_class, progress_json, observed_at, created_at, updated_at)
                 VALUES (?1, 'queued', ?2, 0, 0, NULL, NULL, NULL, ?2, ?2)",
                params![swap_id, to_i64(now)?],
            )
            .map_err(unavailable)?;
        Ok(inserted == 1)
    }

    /// Requeues every row left leased by a process that is gone, with the
    /// class [`COORDINATOR_RESTARTED`] and a new generation. Returns how many.
    ///
    /// # Errors
    ///
    /// Returns [`TakerActorScheduleError::StorageUnavailable`].
    pub fn requeue_leased_on_start(&mut self, now: u64) -> Result<u64, TakerActorScheduleError> {
        let changed = self
            .connection
            .execute(
                "UPDATE taker_actor_schedule
                 SET schedule_state = 'queued', next_attempt_at = ?1,
                     lease_generation = lease_generation + 1,
                     last_failure_class = ?2, updated_at = ?1
                 WHERE schedule_state = 'leased'",
                params![to_i64(now)?, COORDINATOR_RESTARTED],
            )
            .map_err(unavailable)?;
        Ok(changed as u64)
    }

    /// Swaps whose next attempt is due, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`TakerActorScheduleError::StorageUnavailable`].
    pub fn list_due(&self, now: u64, limit: usize) -> Result<Vec<String>, TakerActorScheduleError> {
        let limit = limit.clamp(1, MAX_DUE_LIMIT);
        let mut statement = self
            .connection
            .prepare(
                "SELECT swap_id FROM taker_actor_schedule
                 WHERE schedule_state IN ('queued', 'backoff') AND next_attempt_at <= ?1
                 ORDER BY next_attempt_at ASC, swap_id ASC LIMIT ?2",
            )
            .map_err(unavailable)?;
        let rows = statement
            .query_map(params![to_i64(now)?, to_i64(limit as u64)?], |row| {
                row.get::<_, String>(0)
            })
            .map_err(unavailable)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(unavailable)
    }

    /// Claims a due row for one attempt: leased, generation plus one,
    /// attempts plus one. `None` when the row is not runnable or not due.
    ///
    /// # Errors
    ///
    /// Returns [`TakerActorScheduleError::StorageUnavailable`].
    pub fn claim(
        &mut self,
        swap_id: &str,
        now: u64,
    ) -> Result<Option<TakerActorLeaseV1>, TakerActorScheduleError> {
        validate_swap_id(swap_id)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        let changed = transaction
            .execute(
                "UPDATE taker_actor_schedule
                 SET schedule_state = 'leased', lease_generation = lease_generation + 1,
                     attempt_count = attempt_count + 1, updated_at = ?2
                 WHERE swap_id = ?1 AND schedule_state IN ('queued', 'backoff')
                   AND next_attempt_at <= ?2",
                params![swap_id, to_i64(now)?],
            )
            .map_err(unavailable)?;
        if changed != 1 {
            transaction.rollback().map_err(unavailable)?;
            return Ok(None);
        }
        let (generation, attempt_count): (i64, i64) = transaction
            .query_row(
                "SELECT lease_generation, attempt_count FROM taker_actor_schedule WHERE swap_id = ?1",
                params![swap_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(unavailable)?;
        transaction.commit().map_err(unavailable)?;
        Ok(Some(TakerActorLeaseV1 {
            swap_id: swap_id.to_owned(),
            generation: from_i64(generation)?,
            attempt_count: from_i64(attempt_count)?,
        }))
    }

    /// Records how the leased attempt ended, with the status projection it
    /// observed, fenced by the lease's generation.
    ///
    /// # Errors
    ///
    /// [`TakerActorScheduleError::LeaseConflict`] when the row is not leased
    /// at that generation; [`TakerActorScheduleError::InvalidInput`] for a
    /// failure class outside the alphabet or an oversized snapshot.
    pub fn resolve(
        &mut self,
        lease: &TakerActorLeaseV1,
        resolution: &AttemptResolution,
        progress: Option<&TakerActorProgressSnapshotV1>,
        now: u64,
    ) -> Result<(), TakerActorScheduleError> {
        let (state, next_attempt_at, failure_class): (ScheduleState, u64, Option<&str>) =
            match resolution {
                AttemptResolution::Requeue { not_before } => {
                    (ScheduleState::Queued, *not_before, None)
                }
                AttemptResolution::Backoff {
                    not_before,
                    failure_class,
                } => (
                    ScheduleState::Backoff,
                    *not_before,
                    Some(failure_class.as_str()),
                ),
                AttemptResolution::Terminal | AttemptResolution::ManualActionCompleted => {
                    (ScheduleState::Terminal, 0, None)
                }
                AttemptResolution::Failed { failure_class } => {
                    (ScheduleState::Failed, 0, Some(failure_class.as_str()))
                }
            };
        if let Some(class) = failure_class
            && !validate_failure_class(class)
        {
            return Err(TakerActorScheduleError::InvalidInput);
        }
        let progress_json = progress
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| TakerActorScheduleError::InvalidInput)?;
        if progress_json
            .as_ref()
            .is_some_and(|json| json.len() > MAX_PROGRESS_BYTES)
        {
            return Err(TakerActorScheduleError::InvalidInput);
        }
        let observed_at = progress
            .map(|snapshot| to_i64(snapshot.observed_at))
            .transpose()?;
        let changed = if progress_json.is_some() {
            self.connection
                .execute(
                    "UPDATE taker_actor_schedule
                     SET schedule_state = ?3, next_attempt_at = ?4, last_failure_class = ?5,
                         progress_json = ?6, observed_at = ?7, updated_at = ?8
                     WHERE swap_id = ?1 AND schedule_state = 'leased' AND lease_generation = ?2",
                    params![
                        lease.swap_id,
                        to_i64(lease.generation)?,
                        state.as_str(),
                        to_i64(next_attempt_at)?,
                        failure_class,
                        progress_json,
                        observed_at,
                        to_i64(now)?
                    ],
                )
                .map_err(unavailable)?
        } else {
            self.connection
                .execute(
                    "UPDATE taker_actor_schedule
                     SET schedule_state = ?3, next_attempt_at = ?4, last_failure_class = ?5,
                         updated_at = ?6
                     WHERE swap_id = ?1 AND schedule_state = 'leased' AND lease_generation = ?2",
                    params![
                        lease.swap_id,
                        to_i64(lease.generation)?,
                        state.as_str(),
                        to_i64(next_attempt_at)?,
                        failure_class,
                        to_i64(now)?
                    ],
                )
                .map_err(unavailable)?
        };
        if changed == 1 {
            Ok(())
        } else {
            Err(TakerActorScheduleError::LeaseConflict)
        }
    }

    /// The owner queued a claim or refund: a queued, backed-off or failed row
    /// runs now (a leased row is already running; a terminal one is left).
    /// Returns whether the row changed.
    ///
    /// # Errors
    ///
    /// Returns [`TakerActorScheduleError::StorageUnavailable`].
    pub fn requeue_for_action(
        &mut self,
        swap_id: &str,
        now: u64,
    ) -> Result<bool, TakerActorScheduleError> {
        validate_swap_id(swap_id)?;
        let changed = self
            .connection
            .execute(
                "UPDATE taker_actor_schedule
                 SET schedule_state = 'queued', next_attempt_at = ?2, updated_at = ?2
                 WHERE swap_id = ?1 AND schedule_state IN ('queued', 'backoff', 'failed')",
                params![swap_id, to_i64(now)?],
            )
            .map_err(unavailable)?;
        Ok(changed == 1)
    }

    /// One swap's row.
    ///
    /// # Errors
    ///
    /// Returns [`TakerActorScheduleError::StorageUnavailable`].
    pub fn record(
        &self,
        swap_id: &str,
    ) -> Result<Option<TakerActorScheduleRecordV1>, TakerActorScheduleError> {
        validate_swap_id(swap_id)?;
        self.connection
            .query_row(
                "SELECT swap_id, schedule_state, next_attempt_at, lease_generation, attempt_count,
                        last_failure_class, progress_json, updated_at
                 FROM taker_actor_schedule WHERE swap_id = ?1",
                params![swap_id],
                record_from_row,
            )
            .optional()
            .map_err(unavailable)?
            .transpose()
    }
}

type RowFields = (
    String,
    String,
    i64,
    i64,
    i64,
    Option<String>,
    Option<String>,
    i64,
);

fn record_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<TakerActorScheduleRecordV1, TakerActorScheduleError>> {
    let fields: RowFields = (
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    );
    Ok(record_from_fields(fields))
}

fn record_from_fields(
    (swap_id, state, next_attempt_at, generation, attempts, class, progress, updated_at): RowFields,
) -> Result<TakerActorScheduleRecordV1, TakerActorScheduleError> {
    let schedule_state =
        ScheduleState::parse(&state).ok_or(TakerActorScheduleError::StorageUnavailable)?;
    let progress = progress
        .map(|json| serde_json::from_str::<TakerActorProgressSnapshotV1>(&json))
        .transpose()
        .map_err(|_| TakerActorScheduleError::StorageUnavailable)?;
    Ok(TakerActorScheduleRecordV1 {
        swap_id,
        schedule_state,
        next_attempt_at: from_i64(next_attempt_at)?,
        lease_generation: from_i64(generation)?,
        attempt_count: from_i64(attempts)?,
        last_failure_class: class,
        progress,
        updated_at: from_i64(updated_at)?,
    })
}

fn validate_swap_id(swap_id: &str) -> Result<(), TakerActorScheduleError> {
    if swap_id.is_empty() || swap_id.len() > MAX_SWAP_ID_BYTES {
        return Err(TakerActorScheduleError::InvalidInput);
    }
    Ok(())
}

fn to_i64(value: u64) -> Result<i64, TakerActorScheduleError> {
    i64::try_from(value).map_err(|_| TakerActorScheduleError::InvalidInput)
}

fn from_i64(value: i64) -> Result<u64, TakerActorScheduleError> {
    u64::try_from(value).map_err(|_| TakerActorScheduleError::StorageUnavailable)
}

fn unavailable<E>(_: E) -> TakerActorScheduleError {
    TakerActorScheduleError::StorageUnavailable
}

#[cfg(test)]
mod tests {
    use lez_actor_supervision::{AttemptResolution, ScheduleState};

    use super::{
        COORDINATOR_RESTARTED, SqliteTakerActorSchedule, TakerActorProgressSnapshotV1,
        TakerActorScheduleError,
    };

    fn open() -> (tempfile::TempDir, SqliteTakerActorSchedule) {
        let dir = tempfile::tempdir().expect("tempdir");
        let schedule =
            SqliteTakerActorSchedule::open(dir.path().join("taker-actor-schedule.sqlite3"))
                .expect("open");
        (dir, schedule)
    }

    #[test]
    fn a_registered_swap_is_due_once_claimed_once_and_requeued_by_its_resolution() {
        let (_dir, mut schedule) = open();
        assert!(schedule.register("swap-a", 100).expect("register"));
        assert!(!schedule.register("swap-a", 101).expect("replay"));
        assert_eq!(
            schedule.list_due(100, 10).expect("due"),
            vec!["swap-a".to_owned()]
        );
        let lease = schedule
            .claim("swap-a", 100)
            .expect("claim")
            .expect("leased");
        assert_eq!((lease.generation(), lease.attempt_count()), (1, 1));
        assert!(
            schedule
                .claim("swap-a", 100)
                .expect("second claim")
                .is_none()
        );
        assert!(schedule.list_due(100, 10).expect("due").is_empty());
        let snapshot = TakerActorProgressSnapshotV1 {
            phase: "offered".into(),
            revision: 2,
            next_action: "wait".into(),
            state: "awaiting_first_lock".into(),
            available_action: None,
            observed_at: 105,
        };
        schedule
            .resolve(
                &lease,
                &AttemptResolution::Requeue { not_before: 110 },
                Some(&snapshot),
                105,
            )
            .expect("resolve");
        assert_eq!(
            schedule.resolve(&lease, &AttemptResolution::Terminal, None, 106),
            Err(TakerActorScheduleError::LeaseConflict)
        );
        let record = schedule.record("swap-a").expect("record").expect("row");
        assert_eq!(record.schedule_state(), ScheduleState::Queued);
        assert_eq!(record.next_attempt_at(), 110);
        assert_eq!(record.progress(), Some(&snapshot));
        assert!(schedule.list_due(109, 10).expect("not yet").is_empty());
        assert_eq!(schedule.list_due(110, 10).expect("due again").len(), 1);
    }

    #[test]
    fn backoff_waits_failed_stops_until_an_action_and_terminal_is_final() {
        let (_dir, mut schedule) = open();
        schedule.register("swap-b", 0).expect("register");
        let lease = schedule.claim("swap-b", 0).expect("claim").expect("leased");
        schedule
            .resolve(
                &lease,
                &AttemptResolution::Backoff {
                    not_before: 30,
                    failure_class: "lez_depositor_balance_insufficient".into(),
                },
                None,
                1,
            )
            .expect("backoff");
        let record = schedule.record("swap-b").expect("record").expect("row");
        assert_eq!(record.schedule_state(), ScheduleState::Backoff);
        assert_eq!(
            record.last_failure_class(),
            Some("lez_depositor_balance_insufficient")
        );
        assert!(schedule.list_due(29, 10).expect("waiting").is_empty());
        let lease = schedule
            .claim("swap-b", 30)
            .expect("claim")
            .expect("leased after backoff");
        assert_eq!(lease.generation(), 2);
        schedule
            .resolve(
                &lease,
                &AttemptResolution::Failed {
                    failure_class: "actor_deployment_invalid".into(),
                },
                None,
                31,
            )
            .expect("failed");
        assert!(
            schedule
                .list_due(10_000, 10)
                .expect("failed is not due")
                .is_empty()
        );
        assert!(schedule.requeue_for_action("swap-b", 40).expect("requeue"));
        let lease = schedule
            .claim("swap-b", 40)
            .expect("claim")
            .expect("leased after action");
        schedule
            .resolve(&lease, &AttemptResolution::ManualActionCompleted, None, 41)
            .expect("terminal");
        assert!(
            !schedule
                .requeue_for_action("swap-b", 50)
                .expect("terminal stays")
        );
        assert_eq!(
            schedule
                .record("swap-b")
                .expect("record")
                .expect("row")
                .schedule_state(),
            ScheduleState::Terminal
        );
        assert_eq!(
            schedule.resolve(
                &lease,
                &AttemptResolution::Backoff {
                    not_before: 1,
                    failure_class: "Not A Class".into()
                },
                None,
                1
            ),
            Err(TakerActorScheduleError::InvalidInput)
        );
    }

    #[test]
    fn a_lease_left_by_a_dead_process_is_requeued_at_start_with_its_class() {
        let (dir, mut schedule) = open();
        schedule.register("swap-c", 0).expect("register");
        let _lease = schedule.claim("swap-c", 0).expect("claim").expect("leased");
        drop(schedule);
        let mut reopened =
            SqliteTakerActorSchedule::open(dir.path().join("taker-actor-schedule.sqlite3"))
                .expect("reopen");
        assert_eq!(reopened.requeue_leased_on_start(5).expect("requeue"), 1);
        let record = reopened.record("swap-c").expect("record").expect("row");
        assert_eq!(record.schedule_state(), ScheduleState::Queued);
        assert_eq!(record.lease_generation(), 2);
        assert_eq!(record.last_failure_class(), Some(COORDINATOR_RESTARTED));
        assert_eq!(
            reopened.requeue_leased_on_start(6).expect("nothing left"),
            0
        );
    }
}
