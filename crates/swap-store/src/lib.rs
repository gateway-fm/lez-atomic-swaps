//! Persistent swap repository.

use std::{path::Path, time::Duration};

use lez_swap_core::{Participant, SwapCoordinator, SwapId};
use lez_zec_swap_sdk::{
    ObservationRecordError, ZcashObservationEventRecordV1, ZecBindingRecordError, ZecSwapBinding,
    ZecSwapBindingRecordV1, revalidate_historical_event,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use thiserror::Error;

const DATABASE_SCHEMA_VERSION: i64 = 3;
const SWAP_PAYLOAD_VERSION: i64 = 1;
const ZCASH_EVENT_PAYLOAD_VERSION: i64 = 1;
const ZCASH_BINDING_PAYLOAD_VERSION: i64 = 1;

/// Persistent-store failure.
#[derive(Debug, Error)]
pub enum StoreError {
    /// `SQLite` operation failed.
    #[error("SQLite swap-store operation failed")]
    Sqlite(#[from] rusqlite::Error),
    /// Durable state could not be encoded or decoded.
    #[error("swap state serialization failed")]
    Serialization(#[from] serde_json::Error),
    /// Persisted Zcash evidence is internally inconsistent.
    #[error("persisted Zcash observation record is invalid")]
    ObservationRecord(#[from] ObservationRecordError),
    /// Persisted immutable ZEC binding is internally inconsistent.
    #[error("persisted ZEC swap binding is invalid")]
    ZcashBindingRecord(#[from] ZecBindingRecordError),
    /// The database was created by a newer unsupported application version.
    #[error("unsupported SQLite schema version {0}")]
    UnsupportedDatabaseVersion(i64),
    /// A row uses a payload version this binary cannot decode.
    #[error("unsupported {kind} payload version {version}")]
    UnsupportedPayloadVersion {
        /// Payload family.
        kind: &'static str,
        /// Unsupported version.
        version: i64,
    },
    /// The requested swap does not exist.
    #[error("swap does not exist")]
    MissingSwap,
    /// A ZEC event cannot be accepted without immutable negotiated terms.
    #[error("Zcash swap has no immutable profile/output binding")]
    MissingZcashBinding,
    /// An existing immutable ZEC binding differs from newly supplied terms.
    #[error("immutable ZEC swap binding does not match durable terms")]
    ImmutableZcashBindingMismatch,
    /// Optimistic aggregate revision did not match durable state.
    #[error("stale aggregate revision: expected {expected}, actual {actual}")]
    StaleRevision {
        /// Revision supplied by the caller.
        expected: u64,
        /// Current durable revision.
        actual: u64,
    },
    /// A durable revision cannot be represented safely.
    #[error("aggregate revision overflowed")]
    RevisionOverflow,
}

/// Result of one atomic Zcash event and aggregate commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventCommit {
    revision: u64,
    was_replay: bool,
}

impl EventCommit {
    /// Durable aggregate revision after the operation.
    #[must_use]
    pub const fn revision(self) -> u64 {
        self.revision
    }

    /// Whether an identical event was already durable.
    #[must_use]
    pub const fn was_replay(self) -> bool {
        self.was_replay
    }
}

/// Single-process `SQLite` repository for durable swap aggregates.
#[derive(Debug)]
pub struct SqliteSwapStore {
    connection: Connection,
}

impl SqliteSwapStore {
    /// Opens or creates a store and applies the current schema.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` cannot open, configure, or migrate the database.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut connection)?;
        Ok(Self { connection })
    }

    /// Atomically inserts or replaces one complete swap aggregate.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when encoding or writing the aggregate fails.
    pub fn save(&self, swap: &SwapCoordinator) -> Result<(), StoreError> {
        let state_json = serde_json::to_string(swap)?;
        self.connection.execute(
            "
            INSERT INTO swaps (id, schema_version, state_json)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(id) DO UPDATE SET
                schema_version = excluded.schema_version,
                state_json = excluded.state_json
            ",
            params![swap.id().as_str(), SWAP_PAYLOAD_VERSION, state_json],
        )?;
        Ok(())
    }

    /// Atomically saves a swap and its insert-once immutable ZEC binding.
    ///
    /// Repeating the exact binding is idempotent. A changed profile or expected
    /// output fails without overwriting either durable row.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for serialization, invalid binding, immutable
    /// mismatch, or an `SQLite` transaction failure.
    pub fn save_with_zcash_binding(
        &mut self,
        swap: &SwapCoordinator,
        binding: &ZecSwapBinding,
    ) -> Result<(), StoreError> {
        let state_json = serde_json::to_string(swap)?;
        let binding_record = ZecSwapBindingRecordV1::from_binding(binding);
        binding_record.validate()?;
        let binding_json = serde_json::to_string(&binding_record)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "
            INSERT INTO swaps (id, schema_version, state_json)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(id) DO UPDATE SET
                schema_version = excluded.schema_version,
                state_json = excluded.state_json
            ",
            params![swap.id().as_str(), SWAP_PAYLOAD_VERSION, state_json],
        )?;
        let existing = transaction
            .query_row(
                "SELECT payload_version, payload_json FROM zcash_swap_bindings WHERE swap_id = ?1",
                params![swap.id().as_str()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        match existing {
            None => {
                transaction.execute(
                    "
                    INSERT INTO zcash_swap_bindings (swap_id, payload_version, payload_json)
                    VALUES (?1, ?2, ?3)
                    ",
                    params![
                        swap.id().as_str(),
                        ZCASH_BINDING_PAYLOAD_VERSION,
                        binding_json
                    ],
                )?;
            }
            Some((version, json))
                if version == ZCASH_BINDING_PAYLOAD_VERSION && json == binding_json => {}
            Some(_) => return Err(StoreError::ImmutableZcashBindingMismatch),
        }
        transaction.commit()?;
        Ok(())
    }

    /// Loads and fully revalidates one immutable ZEC binding.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for `SQLite`, unsupported payload version, malformed
    /// JSON, or inconsistent profile/output terms.
    pub fn load_zcash_binding(&self, id: &SwapId) -> Result<Option<ZecSwapBinding>, StoreError> {
        load_zcash_binding_from(&self.connection, id)
    }

    /// Loads a swap by stable ID.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when reading or decoding the stored aggregate fails.
    pub fn load(&self, id: &SwapId) -> Result<Option<SwapCoordinator>, StoreError> {
        let encoded = self
            .connection
            .query_row(
                "SELECT schema_version, state_json FROM swaps WHERE id = ?1",
                params![id.as_str()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        encoded
            .map(|(version, json)| {
                if version != SWAP_PAYLOAD_VERSION {
                    return Err(StoreError::UnsupportedPayloadVersion {
                        kind: "swap",
                        version,
                    });
                }
                serde_json::from_str(&json).map_err(StoreError::from)
            })
            .transpose()
    }

    /// Returns the durable optimistic revision for a swap.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` cannot read the aggregate.
    pub fn revision(&self, id: &SwapId) -> Result<Option<u64>, StoreError> {
        self.connection
            .query_row(
                "SELECT revision FROM swaps WHERE id = ?1",
                params![id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(revision_from_sql)
            .transpose()
    }

    /// Atomically appends one validated Zcash event and updates the swap aggregate.
    ///
    /// Exact event replay is idempotent. The event record and aggregate revision
    /// either both commit or both roll back.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for invalid evidence, a missing swap, stale revision,
    /// serialization failure, overflow, or any `SQLite` transaction failure.
    pub fn commit_zcash_event(
        &mut self,
        expected_revision: u64,
        swap: &SwapCoordinator,
        funded_by: Participant,
        event: &ZcashObservationEventRecordV1,
    ) -> Result<EventCommit, StoreError> {
        event.validate()?;
        let event_json = serde_json::to_string(event)?;
        let state_json = serde_json::to_string(swap)?;
        let role = participant_name(funded_by);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_bound_zcash_event(&transaction, swap.id(), event)?;
        let actual = transaction
            .query_row(
                "SELECT revision FROM swaps WHERE id = ?1",
                params![swap.id().as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .ok_or(StoreError::MissingSwap)
            .and_then(revision_from_sql)?;
        let proposed_revision = expected_revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let proposed_sql_revision =
            i64::try_from(proposed_revision).map_err(|_| StoreError::RevisionOverflow)?;
        let replay = transaction.query_row(
            "
            SELECT EXISTS(
                SELECT 1 FROM chain_events
                WHERE swap_id = ?1 AND funded_by = ?2
                  AND aggregate_revision = ?3
                  AND payload_version = ?4 AND payload_json = ?5
            )
            ",
            params![
                swap.id().as_str(),
                role,
                proposed_sql_revision,
                ZCASH_EVENT_PAYLOAD_VERSION,
                event_json
            ],
            |row| row.get::<_, bool>(0),
        )?;
        if replay {
            transaction.commit()?;
            return Ok(EventCommit {
                revision: actual,
                was_replay: true,
            });
        }
        if actual != expected_revision {
            return Err(StoreError::StaleRevision {
                expected: expected_revision,
                actual,
            });
        }

        let revision = proposed_revision;
        let sql_revision = proposed_sql_revision;
        transaction.execute(
            "
            INSERT INTO chain_events (
                swap_id, aggregate_revision, chain, funded_by,
                event_kind, payload_version, payload_json
            ) VALUES (?1, ?2, 'zcash', ?3, 'observation', ?4, ?5)
            ",
            params![
                swap.id().as_str(),
                sql_revision,
                role,
                ZCASH_EVENT_PAYLOAD_VERSION,
                event_json
            ],
        )?;
        let updated = transaction.execute(
            "
            UPDATE swaps
            SET schema_version = ?1, state_json = ?2, revision = ?3
            WHERE id = ?4 AND revision = ?5
            ",
            params![
                SWAP_PAYLOAD_VERSION,
                state_json,
                sql_revision,
                swap.id().as_str(),
                i64::try_from(actual).map_err(|_| StoreError::RevisionOverflow)?
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::StaleRevision {
                expected: expected_revision,
                actual,
            });
        }
        transaction.commit()?;
        Ok(EventCommit {
            revision,
            was_replay: false,
        })
    }

    /// Finds the exact event committed for one predecessor revision and role.
    ///
    /// This probe lets a runtime detect an unknown successful commit outcome before
    /// reapplying a potentially non-idempotent removal to the aggregate.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for invalid evidence, revision overflow, or `SQLite`
    /// query failure.
    pub fn committed_zcash_event(
        &self,
        predecessor_revision: u64,
        id: &SwapId,
        funded_by: Participant,
        event: &ZcashObservationEventRecordV1,
    ) -> Result<Option<EventCommit>, StoreError> {
        event.validate()?;
        validate_bound_zcash_event(&self.connection, id, event)?;
        let event_json = serde_json::to_string(event)?;
        let revision = predecessor_revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let sql_revision = i64::try_from(revision).map_err(|_| StoreError::RevisionOverflow)?;
        let committed = self.connection.query_row(
            "
            SELECT EXISTS(
                SELECT 1 FROM chain_events
                WHERE swap_id = ?1 AND funded_by = ?2
                  AND aggregate_revision = ?3
                  AND payload_version = ?4 AND payload_json = ?5
            )
            ",
            params![
                id.as_str(),
                participant_name(funded_by),
                sql_revision,
                ZCASH_EVENT_PAYLOAD_VERSION,
                event_json
            ],
            |row| row.get::<_, bool>(0),
        )?;
        Ok(committed.then_some(EventCommit {
            revision,
            was_replay: true,
        }))
    }

    /// Loads ordered, internally revalidated historical Zcash events for one role.
    ///
    /// Loaded records are not fresh canonical evidence and must be reconciled with
    /// the selected node before causing effects.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for `SQLite`, payload-version, JSON, or record failures.
    pub fn load_zcash_events(
        &self,
        id: &SwapId,
        funded_by: Participant,
    ) -> Result<Vec<ZcashObservationEventRecordV1>, StoreError> {
        let mut statement = self.connection.prepare(
            "
            SELECT payload_version, payload_json
            FROM chain_events
            WHERE swap_id = ?1 AND funded_by = ?2 AND chain = 'zcash'
            ORDER BY sequence
            ",
        )?;
        let rows = statement
            .query_map(params![id.as_str(), participant_name(funded_by)], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
        let mut events = Vec::new();
        for row in rows {
            let (version, json) = row?;
            if version != ZCASH_EVENT_PAYLOAD_VERSION {
                return Err(StoreError::UnsupportedPayloadVersion {
                    kind: "Zcash event",
                    version,
                });
            }
            let event: ZcashObservationEventRecordV1 = serde_json::from_str(&json)?;
            event.validate()?;
            events.push(event);
        }
        Ok(events)
    }
}

fn load_zcash_binding_from(
    connection: &Connection,
    id: &SwapId,
) -> Result<Option<ZecSwapBinding>, StoreError> {
    let encoded = connection
        .query_row(
            "SELECT payload_version, payload_json FROM zcash_swap_bindings WHERE swap_id = ?1",
            params![id.as_str()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    encoded
        .map(|(version, json)| {
            if version != ZCASH_BINDING_PAYLOAD_VERSION {
                return Err(StoreError::UnsupportedPayloadVersion {
                    kind: "ZEC swap binding",
                    version,
                });
            }
            let record: ZecSwapBindingRecordV1 = serde_json::from_str(&json)?;
            record.validate().map_err(StoreError::from)
        })
        .transpose()
}

fn validate_bound_zcash_event(
    connection: &Connection,
    id: &SwapId,
    event: &ZcashObservationEventRecordV1,
) -> Result<(), StoreError> {
    let binding =
        load_zcash_binding_from(connection, id)?.ok_or(StoreError::MissingZcashBinding)?;
    let event = revalidate_historical_event(event)?;
    binding.validate_event(&event)?;
    Ok(())
}

fn participant_name(participant: Participant) -> &'static str {
    match participant {
        Participant::Maker => "maker",
        Participant::Taker => "taker",
    }
}

fn revision_from_sql(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::RevisionOverflow)
}

fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > DATABASE_SCHEMA_VERSION {
        return Err(StoreError::UnsupportedDatabaseVersion(version));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS swaps (
            id             TEXT PRIMARY KEY NOT NULL,
            schema_version INTEGER NOT NULL,
            state_json     TEXT NOT NULL,
            revision       INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0)
        ) STRICT;
        ",
    )?;
    let has_revision: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('swaps') WHERE name = 'revision')",
        [],
        |row| row.get(0),
    )?;
    if !has_revision {
        transaction.execute(
            "ALTER TABLE swaps ADD COLUMN revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0)",
            [],
        )?;
    }
    transaction.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS chain_events (
            sequence           INTEGER PRIMARY KEY AUTOINCREMENT,
            swap_id            TEXT NOT NULL REFERENCES swaps(id) ON DELETE CASCADE,
            aggregate_revision INTEGER NOT NULL CHECK (aggregate_revision > 0),
            chain              TEXT NOT NULL CHECK (chain = 'zcash'),
            funded_by          TEXT NOT NULL CHECK (funded_by IN ('maker', 'taker')),
            event_kind         TEXT NOT NULL CHECK (event_kind = 'observation'),
            payload_version    INTEGER NOT NULL,
            payload_json       TEXT NOT NULL,
            UNIQUE (swap_id, aggregate_revision)
        ) STRICT;
        CREATE INDEX IF NOT EXISTS chain_events_swap_role_sequence
            ON chain_events (swap_id, funded_by, sequence);
        CREATE TABLE IF NOT EXISTS zcash_swap_bindings (
            swap_id         TEXT PRIMARY KEY NOT NULL REFERENCES swaps(id) ON DELETE CASCADE,
            payload_version INTEGER NOT NULL,
            payload_json    TEXT NOT NULL
        ) STRICT;
        PRAGMA user_version = 3;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}
