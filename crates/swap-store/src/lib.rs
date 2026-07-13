//! Persistent swap repository.

use std::{path::Path, time::Duration};

mod zec_recovery;

pub use zec_recovery::SqliteZecRecoveryStore;

use lez_swap_core::{Participant, Phase, SwapCoordinator, SwapId};
use lez_zec_swap_sdk::{
    ClaimError, ClaimRecordError, FirstLockRecordError, MakerLockError, MakerLockRecordError,
    ObservationRecordError, ObservedMakerLockError, ObservedTakerFirstLockTransitionError,
    ProtectedClaimError, ZcashObservationEventRecordV1, ZecAgreementV1Error, ZecBindingRecordError,
    ZecSwapBinding, ZecSwapBindingRecordV1, revalidate_historical_event,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

const DATABASE_SCHEMA_VERSION: i64 = 10;
const SWAP_PAYLOAD_VERSION: i64 = 1;
const ZCASH_EVENT_PAYLOAD_VERSION: i64 = 1;
const ZCASH_BINDING_PAYLOAD_VERSION: i64 = 1;
const OPERATOR_ALERT_PAYLOAD_VERSION: i64 = 1;

/// Stable operator/security alert kind.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorAlertKind {
    /// Canonical chain truth replaced a protocol-committed ZEC transaction ID.
    ZcashReplacementConflict,
    /// ZEC chain truth changed after an absorbing lifecycle outcome.
    ZcashTerminalReorg,
}

/// Stable operator alert severity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorAlertSeverity {
    /// Funds remain recoverable but automatic dependent effects are suspended.
    Warning,
    /// An absorbing protocol outcome conflicts with later canonical chain truth.
    Critical,
}

/// Chain event shape that caused an operator alert.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertObservedEvent {
    /// Previously canonical funding evidence was removed.
    Removed,
    /// Removed evidence and a new canonical output arrived atomically.
    Replaced,
}

/// Version-1 durable semantic snapshot for an operator/security alert.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperatorAlertRecordV1 {
    kind: OperatorAlertKind,
    severity: OperatorAlertSeverity,
    funded_by: Participant,
    observed_event: AlertObservedEvent,
    previous_transaction_id: Box<str>,
    canonical_transaction_id: Option<Box<str>>,
    terminal_phase: Option<Phase>,
}

impl OperatorAlertRecordV1 {
    /// Constructs a warning for a different-ID post-dependent replacement.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] unless `event` is an atomic replacement.
    pub fn replacement_conflict(
        funded_by: Participant,
        event: &lez_zec_swap_sdk::ZcashObservationEvent,
    ) -> Result<Self, StoreError> {
        let ids = alert_event_ids(event)?;
        if ids.observed_event != AlertObservedEvent::Replaced {
            return Err(StoreError::InvalidOperatorAlert);
        }
        Ok(Self {
            kind: OperatorAlertKind::ZcashReplacementConflict,
            severity: OperatorAlertSeverity::Warning,
            funded_by,
            observed_event: ids.observed_event,
            previous_transaction_id: ids.previous,
            canonical_transaction_id: ids.canonical,
            terminal_phase: None,
        })
    }

    /// Constructs a critical alert for removal/replacement after a terminal phase.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a non-terminal phase or unsupported event.
    pub fn terminal_reorg(
        terminal_phase: Phase,
        funded_by: Participant,
        event: &lez_zec_swap_sdk::ZcashObservationEvent,
    ) -> Result<Self, StoreError> {
        if !matches!(terminal_phase, Phase::Completed | Phase::Refunded) {
            return Err(StoreError::InvalidOperatorAlert);
        }
        let ids = alert_event_ids(event)?;
        Ok(Self {
            kind: OperatorAlertKind::ZcashTerminalReorg,
            severity: OperatorAlertSeverity::Critical,
            funded_by,
            observed_event: ids.observed_event,
            previous_transaction_id: ids.previous,
            canonical_transaction_id: ids.canonical,
            terminal_phase: Some(terminal_phase),
        })
    }

    /// Stable alert kind.
    #[must_use]
    pub const fn kind(&self) -> OperatorAlertKind {
        self.kind
    }

    /// Stable alert severity.
    #[must_use]
    pub const fn severity(&self) -> OperatorAlertSeverity {
        self.severity
    }

    /// Participant whose ZEC funding evidence changed.
    #[must_use]
    pub const fn funded_by(&self) -> Participant {
        self.funded_by
    }

    /// Removal or replacement event shape.
    #[must_use]
    pub const fn observed_event(&self) -> AlertObservedEvent {
        self.observed_event
    }

    /// Exact detached funding transaction ID.
    #[must_use]
    pub const fn previous_transaction_id(&self) -> &str {
        &self.previous_transaction_id
    }

    /// Newly canonical transaction ID for replacement alerts.
    #[must_use]
    pub fn canonical_transaction_id(&self) -> Option<&str> {
        self.canonical_transaction_id.as_deref()
    }

    /// Absorbing lifecycle phase retained for terminal alerts.
    #[must_use]
    pub const fn terminal_phase(&self) -> Option<Phase> {
        self.terminal_phase
    }

    fn validate(&self) -> Result<(), StoreError> {
        let valid = match self.kind {
            OperatorAlertKind::ZcashReplacementConflict => {
                self.severity == OperatorAlertSeverity::Warning
                    && self.observed_event == AlertObservedEvent::Replaced
                    && self.canonical_transaction_id.is_some()
                    && self.terminal_phase.is_none()
            }
            OperatorAlertKind::ZcashTerminalReorg => {
                self.severity == OperatorAlertSeverity::Critical
                    && matches!(
                        self.terminal_phase,
                        Some(Phase::Completed | Phase::Refunded)
                    )
                    && (self.observed_event == AlertObservedEvent::Removed
                        || self.canonical_transaction_id.is_some())
            }
        };
        if valid {
            Ok(())
        } else {
            Err(StoreError::InvalidOperatorAlert)
        }
    }

    fn validate_against(
        &self,
        funded_by: Participant,
        event: &lez_zec_swap_sdk::ZcashObservationEvent,
    ) -> Result<(), StoreError> {
        self.validate()?;
        let ids = alert_event_ids(event)?;
        if self.funded_by != funded_by
            || self.observed_event != ids.observed_event
            || self.previous_transaction_id != ids.previous
            || self.canonical_transaction_id != ids.canonical
        {
            return Err(StoreError::InvalidOperatorAlert);
        }
        Ok(())
    }

    const fn kind_name(&self) -> &'static str {
        match self.kind {
            OperatorAlertKind::ZcashReplacementConflict => "zcash_replacement_conflict",
            OperatorAlertKind::ZcashTerminalReorg => "zcash_terminal_reorg",
        }
    }

    const fn severity_name(&self) -> &'static str {
        match self.severity {
            OperatorAlertSeverity::Warning => "warning",
            OperatorAlertSeverity::Critical => "critical",
        }
    }
}

/// One durable operator alert row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorAlert {
    sequence: u64,
    aggregate_revision: u64,
    acknowledged: bool,
    record: OperatorAlertRecordV1,
}

impl OperatorAlert {
    /// Stable local alert cursor.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
    /// Aggregate/event revision that created this alert.
    #[must_use]
    pub const fn aggregate_revision(&self) -> u64 {
        self.aggregate_revision
    }
    /// Whether the owner acknowledged seeing this alert.
    #[must_use]
    pub const fn acknowledged(&self) -> bool {
        self.acknowledged
    }
    /// Validated versioned semantic record.
    #[must_use]
    pub const fn record(&self) -> &OperatorAlertRecordV1 {
        &self.record
    }
}

/// Persistent-store failure.
#[derive(Debug, Error)]
pub enum StoreError {
    /// `SQLite` operation failed.
    #[error("SQLite swap-store operation failed")]
    Sqlite(#[from] rusqlite::Error),
    /// Durable state could not be encoded or decoded.
    #[error("swap state serialization failed")]
    Serialization(#[from] serde_json::Error),
    /// A pre-v9 aggregate cannot be migrated without guessing about durable state.
    #[error("legacy swap aggregate is malformed or internally inconsistent")]
    InvalidLegacySwapState,
    /// A reader prevented secure truncation of historical migration WAL frames.
    #[error("legacy swap migration WAL could not be securely truncated")]
    LegacyMigrationCheckpointBusy,
    /// Persisted Zcash evidence is internally inconsistent.
    #[error("persisted Zcash observation record is invalid")]
    ObservationRecord(#[from] ObservationRecordError),
    /// Persisted immutable ZEC binding is internally inconsistent.
    #[error("persisted ZEC swap binding is invalid")]
    ZcashBindingRecord(#[from] ZecBindingRecordError),
    /// A durable concrete LEZ/ZEC agreement failed full revalidation.
    #[error("persisted concrete ZEC agreement is invalid")]
    ZecAgreement(#[from] ZecAgreementV1Error),
    /// A primitive SDK first-lock record failed full context revalidation.
    #[error("persisted SDK first-lock record is invalid")]
    FirstLockRecord(#[from] FirstLockRecordError),
    /// A primitive SDK maker-lock record failed full context revalidation.
    #[error("persisted SDK maker-lock record is invalid")]
    MakerLockRecord(#[from] MakerLockRecordError),
    /// A durable maker-lock transition cannot apply to the reconstructed aggregate.
    #[error("persisted SDK maker-lock transition is invalid")]
    MakerLock(#[from] MakerLockError),
    /// A maker-local taker-lock observation record failed revalidation.
    #[error("persisted maker taker-lock observation is invalid")]
    ObservedTakerFirstLock(#[from] ObservedTakerFirstLockTransitionError),
    /// A taker-local maker-lock observation record failed revalidation.
    #[error("persisted taker maker-lock observation is invalid")]
    ObservedMakerLock(#[from] ObservedMakerLockError),
    /// A secret-free durable claim record failed full context revalidation.
    #[error("persisted SDK claim record is invalid")]
    ClaimRecord(#[from] ClaimRecordError),
    /// A durable claim transition cannot apply to the reconstructed aggregate.
    #[error("persisted SDK claim transition is invalid")]
    Claim(#[from] ClaimError),
    /// A primitive SDK refund record failed full context revalidation.
    #[error("persisted SDK refund record is invalid")]
    RefundRecord(#[from] lez_zec_swap_sdk::RefundRecordError),
    /// A durable refund transition cannot apply to the reconstructed aggregate.
    #[error("persisted SDK refund transition is invalid")]
    Refund(#[from] lez_zec_swap_sdk::RefundError),
    /// A protected claim envelope could not be authenticated or decoded.
    #[error("protected SDK claim material is invalid")]
    ProtectedClaim(#[from] ProtectedClaimError),
    /// The operating system could not provide a fresh claim-envelope nonce.
    #[error("claim nonce generation failed")]
    ClaimEntropy,
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
    /// Operator alert fields disagree with their event or semantic kind.
    #[error("operator alert is inconsistent with its Zcash event")]
    InvalidOperatorAlert,
    /// Alert cursor does not belong to the requested swap.
    #[error("operator alert does not exist for this swap")]
    MissingOperatorAlert,
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
    /// A role-fixed SDK recovery adapter received a different local role.
    #[error("SDK recovery record local role does not match the store")]
    ZecRecoveryRoleMismatch,
    /// An SDK recovery operation requires an agreement row that does not exist.
    #[error("SDK recovery agreement does not exist")]
    MissingZecRecoveryAgreement,
    /// A first-lock transition has no matching retained intent.
    #[error("SDK first-lock intent does not exist")]
    MissingZecFirstLockIntent,
    /// A claim-capable operation was attempted on a store opened without a key.
    #[error("SDK claim recovery key is unavailable")]
    MissingZecClaimKey,
    /// A claim transition or exact payload has no matching retained material.
    #[error("SDK claim recovery material does not exist")]
    MissingZecClaimMaterial,
    /// A claim transition has no matching retained intent.
    #[error("SDK claim intent does not exist")]
    MissingZecClaimIntent,
    /// An owner refund transition has no matching pending exact intent.
    #[error("SDK refund intent does not exist")]
    MissingZecRefundIntent,
    /// An exact claim predecessor slot contains different evidence.
    #[error("SDK claim transition conflicts with durable evidence")]
    ConflictingZecClaimTransition,
    /// An exact refund predecessor slot contains different durable evidence.
    #[error("SDK refund transition conflicts with durable evidence")]
    ConflictingZecRefundTransition,
    /// An exact predecessor transition slot contains different evidence.
    #[error("SDK first-lock transition conflicts with durable evidence")]
    ConflictingZecFirstLockTransition,
    /// SDK recovery rows disagree about revisions or intent closure.
    #[error("SDK recovery rows are internally inconsistent")]
    InvalidZecRecoveryState,
    /// The cloneable SDK recovery connection mutex was poisoned.
    #[error("SDK recovery store lock was poisoned")]
    ZecRecoveryLockPoisoned,
}

/// Result of one atomic Zcash event and aggregate commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventCommit {
    revision: u64,
    was_replay: bool,
}

/// Result of one atomic Zcash event, aggregate, and optional alert commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZcashTransitionCommit {
    event: EventCommit,
    alert_sequence: Option<u64>,
}

impl ZcashTransitionCommit {
    /// Event/aggregate commit metadata.
    #[must_use]
    pub const fn event(self) -> EventCommit {
        self.event
    }

    /// Durable alert cursor, when the transition required operator attention.
    #[must_use]
    pub const fn alert_sequence(self) -> Option<u64> {
        self.alert_sequence
    }
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
        let connection = open_configured_connection(path)?;
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
        self.commit_zcash_transition(expected_revision, swap, funded_by, event, None)
            .map(ZcashTransitionCommit::event)
    }

    /// Atomically commits a Zcash event, aggregate revision, and optional alert.
    ///
    /// Exact replay returns the original alert cursor and never resets its
    /// acknowledgment state.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for invalid evidence/alert consistency, missing or
    /// stale state, serialization, overflow, or any `SQLite` transaction failure.
    pub fn commit_zcash_transition(
        &mut self,
        expected_revision: u64,
        swap: &SwapCoordinator,
        funded_by: Participant,
        event: &ZcashObservationEventRecordV1,
        alert: Option<&OperatorAlertRecordV1>,
    ) -> Result<ZcashTransitionCommit, StoreError> {
        event.validate()?;
        let trusted_event = revalidate_historical_event(event)?;
        if let Some(alert) = alert {
            alert.validate_against(funded_by, &trusted_event)?;
        }
        let event_json = serde_json::to_string(event)?;
        let state_json = serde_json::to_string(swap)?;
        let role = participant_name(funded_by);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_bound_zcash_event(&transaction, swap.id(), event)?;
        if let Some(alert) = alert {
            alert.validate_against(funded_by, &trusted_event)?;
        }
        let actual = current_swap_revision(&transaction, swap.id())?;
        let proposed_revision = expected_revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let proposed_sql_revision =
            i64::try_from(proposed_revision).map_err(|_| StoreError::RevisionOverflow)?;
        let replay = exact_zcash_event_exists(
            &transaction,
            swap.id(),
            role,
            proposed_sql_revision,
            &event_json,
        )?;
        if replay {
            let alert_sequence =
                persist_operator_alert(&transaction, swap.id(), proposed_sql_revision, alert)?;
            transaction.commit()?;
            return Ok(ZcashTransitionCommit {
                event: EventCommit {
                    revision: proposed_revision,
                    was_replay: true,
                },
                alert_sequence,
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
        let alert_sequence = persist_operator_alert(&transaction, swap.id(), sql_revision, alert)?;
        transaction.commit()?;
        Ok(ZcashTransitionCommit {
            event: EventCommit {
                revision,
                was_replay: false,
            },
            alert_sequence,
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

    /// Lists validated operator alerts for one swap after a stable cursor.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for `SQLite`, payload-version, JSON, or alert
    /// validation failures.
    pub fn list_operator_alerts(
        &self,
        id: &SwapId,
        after_sequence: u64,
        include_acknowledged: bool,
    ) -> Result<Vec<OperatorAlert>, StoreError> {
        let after = i64::try_from(after_sequence).map_err(|_| StoreError::RevisionOverflow)?;
        let mut statement = self.connection.prepare(
            "
            SELECT alert_sequence, aggregate_revision, payload_version,
                   payload_json, acknowledged
            FROM operator_alert_outbox
            WHERE swap_id = ?1 AND alert_sequence > ?2
              AND (?3 OR acknowledged = 0)
            ORDER BY alert_sequence
            ",
        )?;
        let rows =
            statement.query_map(params![id.as_str(), after, include_acknowledged], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                ))
            })?;
        let mut alerts = Vec::new();
        for row in rows {
            let (sequence, revision, version, json, acknowledged) = row?;
            if version != OPERATOR_ALERT_PAYLOAD_VERSION {
                return Err(StoreError::UnsupportedPayloadVersion {
                    kind: "operator alert",
                    version,
                });
            }
            let record: OperatorAlertRecordV1 = serde_json::from_str(&json)?;
            record.validate()?;
            alerts.push(OperatorAlert {
                sequence: revision_from_sql(sequence)?,
                aggregate_revision: revision_from_sql(revision)?,
                acknowledged,
                record,
            });
        }
        Ok(alerts)
    }

    /// Marks one owner-visible alert as acknowledged without changing protocol state.
    ///
    /// Acknowledgment is idempotent and never deletes or rewrites alert evidence.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::MissingOperatorAlert`] when the cursor is absent or
    /// belongs to another swap, or [`StoreError::Sqlite`] on write failure.
    pub fn acknowledge_operator_alert(
        &self,
        id: &SwapId,
        alert_sequence: u64,
    ) -> Result<(), StoreError> {
        let sequence = i64::try_from(alert_sequence).map_err(|_| StoreError::RevisionOverflow)?;
        let updated = self.connection.execute(
            "
            UPDATE operator_alert_outbox SET acknowledged = 1
            WHERE swap_id = ?1 AND alert_sequence = ?2
            ",
            params![id.as_str(), sequence],
        )?;
        if updated == 0 {
            return Err(StoreError::MissingOperatorAlert);
        }
        Ok(())
    }
}

fn open_configured_connection(path: impl AsRef<Path>) -> Result<Connection, StoreError> {
    let mut connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "secure_delete", "ON")?;
    migrate(&mut connection)?;
    let checkpoint_busy: i64 =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
    if checkpoint_busy != 0 {
        return Err(StoreError::LegacyMigrationCheckpointBusy);
    }
    Ok(connection)
}

struct AlertEventIds {
    observed_event: AlertObservedEvent,
    previous: Box<str>,
    canonical: Option<Box<str>>,
}

fn alert_event_ids(
    event: &lez_zec_swap_sdk::ZcashObservationEvent,
) -> Result<AlertEventIds, StoreError> {
    match event {
        lez_zec_swap_sdk::ZcashObservationEvent::Removed(removed) => Ok(AlertEventIds {
            observed_event: AlertObservedEvent::Removed,
            previous: removed
                .previous()
                .transaction_id()
                .to_string()
                .into_boxed_str(),
            canonical: None,
        }),
        lez_zec_swap_sdk::ZcashObservationEvent::Replaced { removed, canonical } => {
            Ok(AlertEventIds {
                observed_event: AlertObservedEvent::Replaced,
                previous: removed
                    .previous()
                    .transaction_id()
                    .to_string()
                    .into_boxed_str(),
                canonical: Some(canonical.transaction_id().to_string().into_boxed_str()),
            })
        }
        lez_zec_swap_sdk::ZcashObservationEvent::Canonical(_) => {
            Err(StoreError::InvalidOperatorAlert)
        }
    }
}

fn persist_operator_alert(
    transaction: &rusqlite::Transaction<'_>,
    id: &SwapId,
    aggregate_revision: i64,
    alert: Option<&OperatorAlertRecordV1>,
) -> Result<Option<u64>, StoreError> {
    let Some(alert) = alert else {
        return transaction
            .query_row(
                "
                SELECT alert_sequence FROM operator_alert_outbox
                WHERE swap_id = ?1 AND aggregate_revision = ?2
                ORDER BY alert_sequence LIMIT 1
                ",
                params![id.as_str(), aggregate_revision],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(revision_from_sql)
            .transpose();
    };
    alert.validate()?;
    let json = serde_json::to_string(alert)?;
    let existing = transaction
        .query_row(
            "
            SELECT alert_sequence, payload_version, payload_json
            FROM operator_alert_outbox
            WHERE swap_id = ?1 AND aggregate_revision = ?2 AND alert_kind = ?3
            ",
            params![id.as_str(), aggregate_revision, alert.kind_name()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    if let Some((sequence, version, existing_json)) = existing {
        if version != OPERATOR_ALERT_PAYLOAD_VERSION || existing_json != json {
            return Err(StoreError::InvalidOperatorAlert);
        }
        return revision_from_sql(sequence).map(Some);
    }
    transaction.execute(
        "
        INSERT INTO operator_alert_outbox (
            swap_id, aggregate_revision, alert_kind, severity,
            payload_version, payload_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ",
        params![
            id.as_str(),
            aggregate_revision,
            alert.kind_name(),
            alert.severity_name(),
            OPERATOR_ALERT_PAYLOAD_VERSION,
            json
        ],
    )?;
    revision_from_sql(transaction.last_insert_rowid()).map(Some)
}

fn current_swap_revision(
    transaction: &rusqlite::Transaction<'_>,
    id: &SwapId,
) -> Result<u64, StoreError> {
    transaction
        .query_row(
            "SELECT revision FROM swaps WHERE id = ?1",
            params![id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .ok_or(StoreError::MissingSwap)
        .and_then(revision_from_sql)
}

fn exact_zcash_event_exists(
    transaction: &rusqlite::Transaction<'_>,
    id: &SwapId,
    role: &str,
    aggregate_revision: i64,
    event_json: &str,
) -> Result<bool, StoreError> {
    transaction
        .query_row(
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
                role,
                aggregate_revision,
                ZCASH_EVENT_PAYLOAD_VERSION,
                event_json
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(StoreError::from)
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
    if version < DATABASE_SCHEMA_VERSION {
        migrate_legacy_claim_evidence(&transaction)?;
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
        CREATE TABLE IF NOT EXISTS operator_alert_outbox (
            alert_sequence     INTEGER PRIMARY KEY AUTOINCREMENT,
            swap_id            TEXT NOT NULL,
            aggregate_revision INTEGER NOT NULL CHECK (aggregate_revision > 0),
            alert_kind         TEXT NOT NULL CHECK (
                alert_kind IN ('zcash_replacement_conflict', 'zcash_terminal_reorg')
            ),
            severity           TEXT NOT NULL CHECK (severity IN ('warning', 'critical')),
            payload_version    INTEGER NOT NULL,
            payload_json       TEXT NOT NULL,
            acknowledged       INTEGER NOT NULL DEFAULT 0 CHECK (acknowledged IN (0, 1)),
            UNIQUE (swap_id, aggregate_revision, alert_kind),
            FOREIGN KEY (swap_id, aggregate_revision)
                REFERENCES chain_events(swap_id, aggregate_revision) ON DELETE CASCADE
        ) STRICT;
        CREATE INDEX IF NOT EXISTS operator_alert_swap_pending_sequence
            ON operator_alert_outbox (swap_id, acknowledged, alert_sequence);
        CREATE INDEX IF NOT EXISTS operator_alert_pending_severity_sequence
            ON operator_alert_outbox (acknowledged, severity, alert_sequence);
        ",
    )?;
    migrate_zec_sdk_recovery(&transaction)?;
    transaction.pragma_update(None, "user_version", DATABASE_SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn migrate_legacy_claim_evidence(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<(), StoreError> {
    let rows = {
        let mut statement =
            transaction.prepare("SELECT id, schema_version, state_json FROM swaps ORDER BY id")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };

    for (id, payload_version, encoded) in rows {
        if payload_version != SWAP_PAYLOAD_VERSION {
            return Err(StoreError::UnsupportedPayloadVersion {
                kind: "swap",
                version: payload_version,
            });
        }
        let mut value: serde_json::Value = serde_json::from_str(&encoded)?;
        let object = value
            .as_object_mut()
            .ok_or(StoreError::InvalidLegacySwapState)?;
        let was_plaintext = match object.get_mut("claim_evidence") {
            None | Some(serde_json::Value::Null | serde_json::Value::Object(_)) => false,
            Some(serde_json::Value::Array(bytes)) => {
                let preimage = legacy_claim_preimage(bytes)?;
                object.insert(
                    "claim_evidence".to_owned(),
                    serde_json::json!({
                        "commitment": <[u8; 32]>::from(Sha256::digest(preimage))
                    }),
                );
                true
            }
            Some(_) => return Err(StoreError::InvalidLegacySwapState),
        };

        let trusted: SwapCoordinator = serde_json::from_value(value)?;
        if trusted.id().as_str() != id {
            return Err(StoreError::InvalidLegacySwapState);
        }
        if was_plaintext {
            let canonical = serde_json::to_string(&trusted)?;
            let updated = transaction.execute(
                "UPDATE swaps SET state_json = ?1 WHERE id = ?2 AND state_json = ?3",
                params![canonical, id, encoded],
            )?;
            if updated != 1 {
                return Err(StoreError::InvalidLegacySwapState);
            }
        }
    }
    Ok(())
}

fn legacy_claim_preimage(bytes: &[serde_json::Value]) -> Result<[u8; 32], StoreError> {
    if bytes.len() != 32 {
        return Err(StoreError::InvalidLegacySwapState);
    }
    let mut preimage = [0_u8; 32];
    for (target, value) in preimage.iter_mut().zip(bytes) {
        *target = value
            .as_u64()
            .and_then(|byte| u8::try_from(byte).ok())
            .ok_or(StoreError::InvalidLegacySwapState)?;
    }
    Ok(preimage)
}

// The schema is intentionally kept in one atomic declarative batch so every
// supported legacy database receives the same referential graph or none of it.
#[allow(clippy::too_many_lines)]
fn migrate_zec_sdk_recovery(transaction: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    transaction.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS zec_sdk_agreements (
            local_role          TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
            swap_id             TEXT NOT NULL,
            payload_version     INTEGER NOT NULL CHECK (payload_version > 0),
            agreement_wire      BLOB NOT NULL,
            accepted_at         INTEGER NOT NULL CHECK (accepted_at >= 0),
            accepted_revision   INTEGER NOT NULL CHECK (accepted_revision >= 0),
            active_revision     INTEGER NOT NULL DEFAULT 0 CHECK (active_revision >= 0),
            PRIMARY KEY (local_role, swap_id),
            CHECK (active_revision >= accepted_revision)
        ) STRICT;
        CREATE TABLE IF NOT EXISTS zec_sdk_first_lock_intents (
            local_role          TEXT NOT NULL,
            swap_id             TEXT NOT NULL,
            predecessor_revision INTEGER NOT NULL CHECK (predecessor_revision >= 0),
            payload_version     INTEGER NOT NULL CHECK (payload_version > 0),
            payload_json        TEXT NOT NULL,
            closed_revision     INTEGER CHECK (
                closed_revision IS NULL OR closed_revision = predecessor_revision + 1
            ),
            PRIMARY KEY (local_role, swap_id),
            FOREIGN KEY (local_role, swap_id)
                REFERENCES zec_sdk_agreements(local_role, swap_id) ON DELETE CASCADE
        ) STRICT;
        CREATE INDEX IF NOT EXISTS zec_sdk_open_first_lock_intents
            ON zec_sdk_first_lock_intents (local_role, swap_id)
            WHERE closed_revision IS NULL;
        CREATE TABLE IF NOT EXISTS zec_sdk_first_lock_transitions (
            local_role          TEXT NOT NULL,
            swap_id             TEXT NOT NULL,
            predecessor_revision INTEGER NOT NULL CHECK (predecessor_revision >= 0),
            committed_revision  INTEGER NOT NULL CHECK (
                committed_revision = predecessor_revision + 1
            ),
            payload_version     INTEGER NOT NULL CHECK (payload_version > 0),
            payload_json        TEXT NOT NULL,
            PRIMARY KEY (local_role, swap_id, predecessor_revision),
            UNIQUE (local_role, swap_id, committed_revision),
            FOREIGN KEY (local_role, swap_id)
                REFERENCES zec_sdk_agreements(local_role, swap_id) ON DELETE CASCADE
        ) STRICT;
        CREATE TABLE IF NOT EXISTS zec_sdk_maker_lock_intents (
            local_role          TEXT NOT NULL CHECK (local_role = 'maker'),
            swap_id             TEXT NOT NULL,
            staged_revision     INTEGER NOT NULL CHECK (staged_revision >= 0),
            payload_version     INTEGER NOT NULL CHECK (payload_version > 0),
            payload_json        TEXT NOT NULL,
            closed_revision     INTEGER CHECK (
                closed_revision IS NULL OR closed_revision > staged_revision
            ),
            PRIMARY KEY (local_role, swap_id),
            UNIQUE (local_role, swap_id, staged_revision),
            FOREIGN KEY (local_role, swap_id)
                REFERENCES zec_sdk_agreements(local_role, swap_id) ON DELETE CASCADE
        ) STRICT;
        CREATE INDEX IF NOT EXISTS zec_sdk_open_maker_lock_intents
            ON zec_sdk_maker_lock_intents (local_role, swap_id)
            WHERE closed_revision IS NULL;
        CREATE TABLE IF NOT EXISTS zec_sdk_maker_lock_transitions (
            local_role          TEXT NOT NULL CHECK (local_role = 'maker'),
            swap_id             TEXT NOT NULL,
            predecessor_revision INTEGER NOT NULL CHECK (predecessor_revision >= 0),
            committed_revision  INTEGER NOT NULL CHECK (
                committed_revision = predecessor_revision + 1
            ),
            intent_staged_revision INTEGER NOT NULL CHECK (
                intent_staged_revision >= 0
                AND intent_staged_revision <= predecessor_revision
            ),
            payload_version     INTEGER NOT NULL CHECK (payload_version > 0),
            payload_json        TEXT NOT NULL,
            PRIMARY KEY (local_role, swap_id, predecessor_revision),
            UNIQUE (local_role, swap_id, intent_staged_revision),
            FOREIGN KEY (local_role, swap_id)
                REFERENCES zec_sdk_agreements(local_role, swap_id) ON DELETE CASCADE,
            FOREIGN KEY (local_role, swap_id, intent_staged_revision)
                REFERENCES zec_sdk_maker_lock_intents(
                    local_role, swap_id, staged_revision
                ) ON DELETE CASCADE
        ) STRICT;
        CREATE TABLE IF NOT EXISTS zec_sdk_observed_maker_lock_transitions (
            local_role          TEXT NOT NULL CHECK (local_role = 'taker'),
            swap_id             TEXT NOT NULL,
            predecessor_revision INTEGER NOT NULL CHECK (predecessor_revision >= 0),
            committed_revision  INTEGER NOT NULL CHECK (
                committed_revision = predecessor_revision + 1
            ),
            payload_version     INTEGER NOT NULL CHECK (payload_version > 0),
            payload_json        TEXT NOT NULL,
            PRIMARY KEY (local_role, swap_id, predecessor_revision),
            UNIQUE (local_role, swap_id, committed_revision),
            FOREIGN KEY (local_role, swap_id)
                REFERENCES zec_sdk_agreements(local_role, swap_id) ON DELETE CASCADE
        ) STRICT;
        CREATE TABLE IF NOT EXISTS zec_sdk_claim_materials (
            local_role          TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
            swap_id             TEXT NOT NULL,
            purpose             TEXT NOT NULL CHECK (
                purpose IN ('local_first_claim', 'observed_followup_claim')
            ),
            created_revision    INTEGER NOT NULL CHECK (created_revision >= 0),
            envelope_version    INTEGER NOT NULL CHECK (envelope_version > 0),
            ciphertext          BLOB NOT NULL CHECK (length(ciphertext) = 48),
            nonce               BLOB NOT NULL CHECK (length(nonce) = 24),
            key_id              TEXT NOT NULL CHECK (
                length(CAST(key_id AS BLOB)) BETWEEN 1 AND 128
            ),
            fingerprint         BLOB NOT NULL CHECK (length(fingerprint) = 32),
            PRIMARY KEY (local_role, swap_id),
            UNIQUE (local_role, swap_id, created_revision),
            UNIQUE (key_id, nonce),
            FOREIGN KEY (local_role, swap_id)
                REFERENCES zec_sdk_agreements(local_role, swap_id) ON DELETE CASCADE
        ) STRICT;
        CREATE TABLE IF NOT EXISTS zec_sdk_claim_intents (
            local_role          TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
            swap_id             TEXT NOT NULL,
            staged_revision     INTEGER NOT NULL CHECK (staged_revision >= 0),
            material_created_revision INTEGER NOT NULL CHECK (
                material_created_revision >= 0
                AND material_created_revision <= staged_revision
            ),
            payload_version     INTEGER NOT NULL CHECK (payload_version > 0),
            payload_json        TEXT NOT NULL,
            protected_version   INTEGER NOT NULL CHECK (protected_version > 0),
            protected_ciphertext BLOB NOT NULL CHECK (
                length(protected_ciphertext) BETWEEN 17 AND 2000016
            ),
            protected_nonce     BLOB NOT NULL CHECK (length(protected_nonce) = 24),
            protected_key_id    TEXT NOT NULL CHECK (
                length(CAST(protected_key_id AS BLOB)) BETWEEN 1 AND 128
            ),
            protected_fingerprint BLOB NOT NULL CHECK (
                length(protected_fingerprint) = 32
            ),
            closed_revision     INTEGER CHECK (
                closed_revision IS NULL OR closed_revision > staged_revision
            ),
            PRIMARY KEY (local_role, swap_id),
            UNIQUE (local_role, swap_id, staged_revision),
            UNIQUE (protected_key_id, protected_nonce),
            FOREIGN KEY (local_role, swap_id)
                REFERENCES zec_sdk_agreements(local_role, swap_id) ON DELETE CASCADE,
            FOREIGN KEY (local_role, swap_id, material_created_revision)
                REFERENCES zec_sdk_claim_materials(
                    local_role, swap_id, created_revision
                ) ON DELETE CASCADE
        ) STRICT;
        CREATE INDEX IF NOT EXISTS zec_sdk_open_claim_intents
            ON zec_sdk_claim_intents (local_role, swap_id)
            WHERE closed_revision IS NULL;
        CREATE TABLE IF NOT EXISTS zec_sdk_owned_claim_transitions (
            local_role          TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
            swap_id             TEXT NOT NULL,
            transition_kind     TEXT NOT NULL CHECK (
                transition_kind IN ('revealing_lez', 'followup_zcash')
            ),
            predecessor_revision INTEGER NOT NULL CHECK (predecessor_revision >= 0),
            committed_revision  INTEGER NOT NULL CHECK (
                committed_revision = predecessor_revision + 1
            ),
            intent_staged_revision INTEGER NOT NULL CHECK (
                intent_staged_revision >= 0
                AND intent_staged_revision <= predecessor_revision
            ),
            payload_version     INTEGER NOT NULL CHECK (payload_version > 0),
            payload_json        TEXT NOT NULL,
            PRIMARY KEY (local_role, swap_id, predecessor_revision),
            UNIQUE (local_role, swap_id, committed_revision),
            UNIQUE (local_role, swap_id, intent_staged_revision),
            FOREIGN KEY (local_role, swap_id)
                REFERENCES zec_sdk_agreements(local_role, swap_id) ON DELETE CASCADE,
            FOREIGN KEY (local_role, swap_id, intent_staged_revision)
                REFERENCES zec_sdk_claim_intents(
                    local_role, swap_id, staged_revision
                ) ON DELETE CASCADE
        ) STRICT;
        CREATE TABLE IF NOT EXISTS zec_sdk_observed_claim_transitions (
            local_role          TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
            swap_id             TEXT NOT NULL,
            transition_kind     TEXT NOT NULL CHECK (
                transition_kind IN ('observed_revealing_lez', 'observed_followup_zcash')
            ),
            predecessor_revision INTEGER NOT NULL CHECK (predecessor_revision >= 0),
            committed_revision  INTEGER NOT NULL CHECK (
                committed_revision = predecessor_revision + 1
            ),
            material_created_revision INTEGER,
            payload_version     INTEGER NOT NULL CHECK (payload_version > 0),
            payload_json        TEXT NOT NULL,
            PRIMARY KEY (local_role, swap_id, predecessor_revision),
            UNIQUE (local_role, swap_id, committed_revision),
            CHECK (
                (transition_kind = 'observed_revealing_lez'
                    AND material_created_revision = committed_revision)
                OR
                (transition_kind = 'observed_followup_zcash'
                    AND material_created_revision IS NULL)
            ),
            FOREIGN KEY (local_role, swap_id)
                REFERENCES zec_sdk_agreements(local_role, swap_id) ON DELETE CASCADE,
            FOREIGN KEY (local_role, swap_id, material_created_revision)
                REFERENCES zec_sdk_claim_materials(
                    local_role, swap_id, created_revision
                ) ON DELETE CASCADE
        ) STRICT;
        CREATE TABLE IF NOT EXISTS zec_sdk_refund_intents (
            local_role TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
            swap_id TEXT NOT NULL,
            staged_revision INTEGER NOT NULL CHECK (staged_revision >= 0),
            payload_version INTEGER NOT NULL CHECK (payload_version > 0),
            payload_json TEXT NOT NULL,
            PRIMARY KEY (local_role, swap_id),
            UNIQUE (local_role, swap_id, staged_revision),
            FOREIGN KEY (local_role, swap_id)
                REFERENCES zec_sdk_agreements(local_role, swap_id) ON DELETE CASCADE
        ) STRICT;
        CREATE TABLE IF NOT EXISTS zec_sdk_refund_transitions (
            local_role TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
            swap_id TEXT NOT NULL,
            predecessor_revision INTEGER NOT NULL CHECK (predecessor_revision >= 0),
            committed_revision INTEGER NOT NULL CHECK (
                committed_revision = predecessor_revision + 1
            ),
            transition_kind TEXT NOT NULL CHECK (
                transition_kind IN ('owned', 'observed')
            ),
            payload_version INTEGER NOT NULL CHECK (payload_version > 0),
            payload_json TEXT NOT NULL,
            retained_intent_version INTEGER,
            retained_intent_json TEXT,
            intent_staged_revision INTEGER,
            PRIMARY KEY (local_role, swap_id, predecessor_revision),
            UNIQUE (local_role, swap_id, committed_revision),
            CHECK (
                (transition_kind = 'owned'
                    AND retained_intent_version IS NOT NULL
                    AND retained_intent_json IS NOT NULL
                    AND intent_staged_revision IS NOT NULL
                    AND intent_staged_revision >= 0
                    AND intent_staged_revision <= predecessor_revision)
                OR
                (transition_kind = 'observed'
                    AND retained_intent_version IS NULL
                    AND retained_intent_json IS NULL
                    AND intent_staged_revision IS NULL)
            ),
            FOREIGN KEY (local_role, swap_id)
                REFERENCES zec_sdk_agreements(local_role, swap_id) ON DELETE CASCADE
        ) STRICT;
        ",
    )?;
    Ok(())
}
