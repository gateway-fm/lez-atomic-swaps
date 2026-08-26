//! Crash-safe caller-owned bridge request context journal.

use std::path::Path;

use lez_bridge_protocol::{DiscoveryWindow, RequestId, RunId};
use lez_swap_core::{Participant, SwapId};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{StoreError, open_configured_connection, participant_name};

/// One lifecycle-specific bridge operation.
///
/// Submit operations are purpose-specific because the bridge client's wire-level
/// `submit_transaction` method is shared by escrow, claim, and refund effects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum BridgeOperationKind {
    /// Prepare the native escrow initialize/fund transaction pair.
    NativeEscrowPrepare,
    /// Observe native escrow initialization and funding by exact prepared identity.
    NativeEscrowExactObserve,
    /// Discover native escrow initialization and funding in a bounded window.
    NativeEscrowDiscoveryObserve,
    /// Submit the prepared native escrow initialization transaction.
    NativeEscrowInitializeSubmit,
    /// Submit the prepared native escrow funding transaction.
    NativeEscrowFundSubmit,
    /// Prepare the revealing LEZ claim.
    RevealingClaimPrepare,
    /// Observe the revealing LEZ claim by exact prepared identity.
    RevealingClaimExactObserve,
    /// Discover a counterparty revealing LEZ claim in a bounded window.
    RevealingClaimDiscoveryObserve,
    /// Submit the revealing LEZ claim.
    RevealingClaimSubmit,
    /// Prepare the fixed-destination native refund.
    NativeRefundPrepare,
    /// Observe owner-side native refund eligibility before preparation.
    NativeRefundEligibilityObserve,
    /// Observe a native refund by exact prepared identity.
    NativeRefundExactObserve,
    /// Discover a counterparty native refund in a bounded window.
    NativeRefundDiscoveryObserve,
    /// Submit the fixed-destination native refund.
    NativeRefundSubmit,
}

impl BridgeOperationKind {
    const fn name(self) -> &'static str {
        match self {
            Self::NativeEscrowPrepare => "native_escrow_prepare",
            Self::NativeEscrowExactObserve => "native_escrow_exact_observe",
            Self::NativeEscrowDiscoveryObserve => "native_escrow_discovery_observe",
            Self::NativeEscrowInitializeSubmit => "native_escrow_initialize_submit",
            Self::NativeEscrowFundSubmit => "native_escrow_fund_submit",
            Self::RevealingClaimPrepare => "revealing_claim_prepare",
            Self::RevealingClaimExactObserve => "revealing_claim_exact_observe",
            Self::RevealingClaimDiscoveryObserve => "revealing_claim_discovery_observe",
            Self::RevealingClaimSubmit => "revealing_claim_submit",
            Self::NativeRefundPrepare => "native_refund_prepare",
            Self::NativeRefundEligibilityObserve => "native_refund_eligibility_observe",
            Self::NativeRefundExactObserve => "native_refund_exact_observe",
            Self::NativeRefundDiscoveryObserve => "native_refund_discovery_observe",
            Self::NativeRefundSubmit => "native_refund_submit",
        }
    }

    const fn is_observation(self) -> bool {
        matches!(
            self,
            Self::NativeEscrowExactObserve
                | Self::NativeEscrowDiscoveryObserve
                | Self::RevealingClaimExactObserve
                | Self::RevealingClaimDiscoveryObserve
                | Self::NativeRefundEligibilityObserve
                | Self::NativeRefundExactObserve
                | Self::NativeRefundDiscoveryObserve
        )
    }

    const fn requires_window(self) -> bool {
        matches!(
            self,
            Self::NativeEscrowDiscoveryObserve
                | Self::RevealingClaimDiscoveryObserve
                | Self::NativeRefundExactObserve
                | Self::NativeRefundDiscoveryObserve
        )
    }
}

/// Composite isolation key for one bridge lifecycle operation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BridgeOperationKey {
    run_id: RunId,
    swap_id: SwapId,
    local_role: Participant,
    operation: BridgeOperationKind,
}

impl BridgeOperationKey {
    /// Constructs a key from already validated run and swap identities.
    pub const fn new(
        run_id: RunId,
        swap_id: SwapId,
        local_role: Participant,
        operation: BridgeOperationKind,
    ) -> Self {
        Self {
            run_id,
            swap_id,
            local_role,
            operation,
        }
    }

    /// Composed-run isolation identity.
    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Stable swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Role fixed to the local SDK actor and sidecar.
    #[must_use]
    pub const fn local_role(&self) -> Participant {
        self.local_role
    }

    /// Lifecycle-specific operation identity.
    pub const fn operation(&self) -> BridgeOperationKind {
        self.operation
    }
}

/// Caller-selected request identity and optional bounded discovery window.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BridgeRequestSpec {
    request_id: RequestId,
    discovery_window: Option<DiscoveryWindow>,
}

impl BridgeRequestSpec {
    /// Constructs a request specification without deriving either field.
    pub const fn new(request_id: RequestId, discovery_window: Option<DiscoveryWindow>) -> Self {
        Self {
            request_id,
            discovery_window,
        }
    }

    /// Caller-owned one-use bridge request ID.
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Caller-owned bounded scan window, when this is a discovery poll.
    #[must_use]
    pub const fn discovery_window(&self) -> Option<DiscoveryWindow> {
        self.discovery_window
    }
}

/// One active caller request restored from durable storage.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct DurableBridgeRequestContext {
    poll_sequence: u64,
    request_id: RequestId,
    discovery_window: Option<DiscoveryWindow>,
}

impl DurableBridgeRequestContext {
    /// Monotonic poll sequence within the complete composite operation key.
    #[must_use]
    pub const fn poll_sequence(&self) -> u64 {
        self.poll_sequence
    }

    /// Exact caller-owned request ID to resume after ambiguity.
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Exact caller-owned discovery window to resume after ambiguity.
    #[must_use]
    pub const fn discovery_window(&self) -> Option<DiscoveryWindow> {
        self.discovery_window
    }

    fn matches_spec(&self, spec: &BridgeRequestSpec) -> bool {
        self.request_id == spec.request_id && self.discovery_window == spec.discovery_window
    }
}

/// Definitive observation result that consumes one request context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum BridgeObservationOutcome {
    /// Observation returned a successful typed result.
    Succeeded,
    /// Observation returned a definitive typed protocol error.
    TypedError,
}

impl BridgeObservationOutcome {
    const fn name(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::TypedError => "typed_error",
        }
    }
}

/// Result of inserting or idempotently replaying one durable request context.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BridgeContextCommit {
    context: DurableBridgeRequestContext,
    was_replay: bool,
}

impl BridgeContextCommit {
    /// Active durable request context after the operation.
    pub const fn context(&self) -> &DurableBridgeRequestContext {
        &self.context
    }

    /// Whether the exact requested transition was already durable.
    #[must_use]
    pub const fn was_replay(&self) -> bool {
        self.was_replay
    }
}

/// SQLite-backed bridge operation-context journal.
#[derive(Debug)]
pub struct SqliteBridgeOperationJournal {
    connection: Connection,
}

impl SqliteBridgeOperationJournal {
    /// Opens or creates the additive bridge operation journal.
    ///
    /// The connection reuses the swap store's WAL, `FULL` synchronous, foreign-key,
    /// secure-delete, busy-timeout, and migration policy.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` cannot open, configure, or migrate the journal.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let mut connection = open_configured_connection(path)?;
        migrate_bridge_operation_journal(&mut connection)?;
        Ok(Self { connection })
    }

    /// Inserts the initial caller request or resumes an identical active request.
    ///
    /// A different request cannot replace an active ambiguous request. Request IDs
    /// are never synthesized and are one-use within one run/role sidecar client.
    ///
    /// # Errors
    ///
    /// Returns a typed store error for invalid operation/window shape, active
    /// context conflict, request-ID reuse, sequence overflow, or `SQLite` failure.
    pub fn begin_or_resume(
        &mut self,
        key: &BridgeOperationKey,
        requested: &BridgeRequestSpec,
    ) -> Result<BridgeContextCommit, StoreError> {
        validate_request_shape(key, requested)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(active) = load_active_context(&transaction, key)? {
            if active.matches_spec(requested) {
                transaction.commit()?;
                return Ok(BridgeContextCommit {
                    context: active,
                    was_replay: true,
                });
            }
            return Err(StoreError::BridgeOperationContextConflict);
        }
        ensure_request_id_unused(&transaction, key, requested.request_id())?;
        let poll_sequence = next_poll_sequence(&transaction, key)?;
        let context = DurableBridgeRequestContext {
            poll_sequence,
            request_id: requested.request_id.clone(),
            discovery_window: requested.discovery_window,
        };
        insert_active_context(&transaction, key, &context)?;
        transaction.commit()?;
        Ok(BridgeContextCommit {
            context,
            was_replay: false,
        })
    }

    /// Reloads the exact active request after an ambiguous remote outcome.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::MissingBridgeOperationContext`] when no active row
    /// exists, or [`StoreError::BridgeOperationContextConflict`] when the caller
    /// attempts to resume a different sequence, request ID, or discovery window.
    pub fn resume_after_ambiguous(
        &self,
        key: &BridgeOperationKey,
        expected: &DurableBridgeRequestContext,
    ) -> Result<DurableBridgeRequestContext, StoreError> {
        let active = load_active_context(&self.connection, key)?
            .ok_or(StoreError::MissingBridgeOperationContext)?;
        if active == *expected {
            Ok(active)
        } else {
            Err(StoreError::BridgeOperationContextConflict)
        }
    }

    /// Atomically completes one definitive observation and inserts its next poll.
    ///
    /// Retrying after an unknown successful `SQLite` commit returns the already
    /// active next context. The next request ID is caller-supplied and must differ
    /// from the consumed request ID.
    ///
    /// # Errors
    ///
    /// Returns a typed store error for a non-observation operation, stale context,
    /// reused request ID, sequence overflow, or `SQLite` failure.
    pub fn advance_observation(
        &mut self,
        key: &BridgeOperationKey,
        expected: &DurableBridgeRequestContext,
        outcome: BridgeObservationOutcome,
        next: &BridgeRequestSpec,
    ) -> Result<BridgeContextCommit, StoreError> {
        if !key.operation.is_observation() {
            return Err(StoreError::InvalidBridgeOperationContext);
        }
        validate_request_shape(key, next)?;
        if expected.request_id == next.request_id {
            return Err(StoreError::InvalidBridgeOperationContext);
        }
        let next_sequence = expected
            .poll_sequence
            .checked_add(1)
            .ok_or(StoreError::BridgePollSequenceOverflow)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active = load_active_context(&transaction, key)?
            .ok_or(StoreError::MissingBridgeOperationContext)?;
        if active != *expected {
            if active.poll_sequence == next_sequence
                && active.matches_spec(next)
                && completed_context_exists(&transaction, key, expected, outcome)?
            {
                transaction.commit()?;
                return Ok(BridgeContextCommit {
                    context: active,
                    was_replay: true,
                });
            }
            return Err(StoreError::BridgeOperationContextConflict);
        }
        ensure_request_id_unused(&transaction, key, next.request_id())?;
        let updated = transaction.execute(
            "
            UPDATE bridge_operation_contexts
            SET state = 'completed', completion_outcome = ?1
            WHERE run_id = ?2 AND swap_id = ?3 AND local_role = ?4
              AND operation = ?5 AND poll_sequence = ?6 AND state = 'active'
            ",
            params![
                outcome.name(),
                key.run_id.as_str(),
                key.swap_id.as_str(),
                participant_name(key.local_role),
                key.operation.name(),
                sequence_to_sql(expected.poll_sequence)?,
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::BridgeOperationContextConflict);
        }
        let context = DurableBridgeRequestContext {
            poll_sequence: next_sequence,
            request_id: next.request_id.clone(),
            discovery_window: next.discovery_window,
        };
        insert_active_context(&transaction, key, &context)?;
        transaction.commit()?;
        Ok(BridgeContextCommit {
            context,
            was_replay: false,
        })
    }

    /// Loads the active context for exactly one run/role/swap/operation key.
    ///
    /// # Errors
    ///
    /// Returns a store error for `SQLite` failure or invalid persisted values.
    pub fn current(
        &self,
        key: &BridgeOperationKey,
    ) -> Result<Option<DurableBridgeRequestContext>, StoreError> {
        load_active_context(&self.connection, key)
    }
}

fn migrate_bridge_operation_journal(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS bridge_operation_contexts (
            run_id              TEXT NOT NULL,
            swap_id             TEXT NOT NULL,
            local_role          TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
            operation           TEXT NOT NULL CHECK (operation IN (
                'native_escrow_prepare', 'native_escrow_exact_observe',
                'native_escrow_discovery_observe',
                'native_escrow_initialize_submit', 'native_escrow_fund_submit',
                'revealing_claim_prepare', 'revealing_claim_exact_observe',
                'revealing_claim_discovery_observe', 'revealing_claim_submit',
                'native_refund_prepare', 'native_refund_eligibility_observe',
                'native_refund_exact_observe', 'native_refund_discovery_observe',
                'native_refund_submit'
            )),
            poll_sequence       INTEGER NOT NULL CHECK (poll_sequence >= 0),
            request_id          TEXT NOT NULL,
            window_start_height TEXT,
            window_max_blocks   INTEGER CHECK (window_max_blocks > 0),
            state               TEXT NOT NULL CHECK (state IN ('active', 'completed')),
            completion_outcome  TEXT CHECK (
                completion_outcome IN ('succeeded', 'typed_error')
            ),
            PRIMARY KEY (run_id, swap_id, local_role, operation, poll_sequence),
            CHECK (
                (window_start_height IS NULL AND window_max_blocks IS NULL)
                OR (window_start_height IS NOT NULL AND window_max_blocks IS NOT NULL)
            ),
            CHECK (
                (state = 'active' AND completion_outcome IS NULL)
                OR (state = 'completed' AND completion_outcome IS NOT NULL)
            )
        ) STRICT;
        CREATE UNIQUE INDEX IF NOT EXISTS bridge_operation_one_active
            ON bridge_operation_contexts (run_id, swap_id, local_role, operation)
            WHERE state = 'active';
        CREATE UNIQUE INDEX IF NOT EXISTS bridge_operation_request_once
            ON bridge_operation_contexts (run_id, local_role, request_id);
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn validate_request_shape(
    key: &BridgeOperationKey,
    request: &BridgeRequestSpec,
) -> Result<(), StoreError> {
    if request.discovery_window.is_some() == key.operation.requires_window() {
        Ok(())
    } else {
        Err(StoreError::InvalidBridgeOperationContext)
    }
}

fn load_active_context(
    connection: &Connection,
    key: &BridgeOperationKey,
) -> Result<Option<DurableBridgeRequestContext>, StoreError> {
    let encoded = connection
        .query_row(
            "
            SELECT poll_sequence, request_id, window_start_height, window_max_blocks
            FROM bridge_operation_contexts
            WHERE run_id = ?1 AND swap_id = ?2 AND local_role = ?3
              AND operation = ?4 AND state = 'active'
            ",
            params![
                key.run_id.as_str(),
                key.swap_id.as_str(),
                participant_name(key.local_role),
                key.operation.name(),
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()?;
    encoded.map(decode_context).transpose()
}

fn decode_context(
    (sequence, request_id, start_height, max_blocks): (i64, String, Option<String>, Option<i64>),
) -> Result<DurableBridgeRequestContext, StoreError> {
    let poll_sequence =
        u64::try_from(sequence).map_err(|_| StoreError::BridgePollSequenceOverflow)?;
    let request_id = RequestId::new(request_id)?;
    let discovery_window = match (start_height, max_blocks) {
        (None, None) => None,
        (Some(start), Some(maximum)) => {
            let start = start
                .parse::<u64>()
                .map_err(|_| StoreError::InvalidBridgeOperationContext)?;
            let maximum =
                u32::try_from(maximum).map_err(|_| StoreError::InvalidBridgeOperationContext)?;
            Some(DiscoveryWindow::new(start, maximum)?)
        }
        (Some(_), None) | (None, Some(_)) => {
            return Err(StoreError::InvalidBridgeOperationContext);
        }
    };
    Ok(DurableBridgeRequestContext {
        poll_sequence,
        request_id,
        discovery_window,
    })
}

fn insert_active_context(
    connection: &Connection,
    key: &BridgeOperationKey,
    context: &DurableBridgeRequestContext,
) -> Result<(), StoreError> {
    let (start_height, max_blocks) = encode_window(context.discovery_window);
    connection.execute(
        "
        INSERT INTO bridge_operation_contexts (
            run_id, swap_id, local_role, operation, poll_sequence, request_id,
            window_start_height, window_max_blocks, state, completion_outcome
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'active', NULL)
        ",
        params![
            key.run_id.as_str(),
            key.swap_id.as_str(),
            participant_name(key.local_role),
            key.operation.name(),
            sequence_to_sql(context.poll_sequence)?,
            context.request_id.as_str(),
            start_height,
            max_blocks,
        ],
    )?;
    Ok(())
}

fn next_poll_sequence(
    connection: &Connection,
    key: &BridgeOperationKey,
) -> Result<u64, StoreError> {
    let latest = connection.query_row(
        "
        SELECT MAX(poll_sequence)
        FROM bridge_operation_contexts
        WHERE run_id = ?1 AND swap_id = ?2 AND local_role = ?3 AND operation = ?4
        ",
        params![
            key.run_id.as_str(),
            key.swap_id.as_str(),
            participant_name(key.local_role),
            key.operation.name(),
        ],
        |row| row.get::<_, Option<i64>>(0),
    )?;
    latest.map_or(Ok(0), |value| {
        u64::try_from(value)
            .map_err(|_| StoreError::BridgePollSequenceOverflow)?
            .checked_add(1)
            .ok_or(StoreError::BridgePollSequenceOverflow)
    })
}

fn ensure_request_id_unused(
    connection: &Connection,
    key: &BridgeOperationKey,
    request_id: &RequestId,
) -> Result<(), StoreError> {
    let exists = connection.query_row(
        "
        SELECT EXISTS(
            SELECT 1 FROM bridge_operation_contexts
            WHERE run_id = ?1 AND local_role = ?2 AND request_id = ?3
        )
        ",
        params![
            key.run_id.as_str(),
            participant_name(key.local_role),
            request_id.as_str(),
        ],
        |row| row.get::<_, bool>(0),
    )?;
    if exists {
        Err(StoreError::BridgeRequestIdReused)
    } else {
        Ok(())
    }
}

fn completed_context_exists(
    connection: &Connection,
    key: &BridgeOperationKey,
    expected: &DurableBridgeRequestContext,
    outcome: BridgeObservationOutcome,
) -> Result<bool, StoreError> {
    let (start_height, max_blocks) = encode_window(expected.discovery_window);
    connection
        .query_row(
            "
            SELECT EXISTS(
                SELECT 1 FROM bridge_operation_contexts
                WHERE run_id = ?1 AND swap_id = ?2 AND local_role = ?3
                  AND operation = ?4 AND poll_sequence = ?5 AND request_id = ?6
                  AND window_start_height IS ?7 AND window_max_blocks IS ?8
                  AND state = 'completed' AND completion_outcome = ?9
            )
            ",
            params![
                key.run_id.as_str(),
                key.swap_id.as_str(),
                participant_name(key.local_role),
                key.operation.name(),
                sequence_to_sql(expected.poll_sequence)?,
                expected.request_id.as_str(),
                start_height,
                max_blocks,
                outcome.name(),
            ],
            |row| row.get(0),
        )
        .map_err(StoreError::from)
}

fn encode_window(window: Option<DiscoveryWindow>) -> (Option<String>, Option<i64>) {
    window.map_or((None, None), |window| {
        (
            Some(window.start_height().to_string()),
            Some(i64::from(window.max_blocks())),
        )
    })
}

fn sequence_to_sql(sequence: u64) -> Result<i64, StoreError> {
    i64::try_from(sequence).map_err(|_| StoreError::BridgePollSequenceOverflow)
}
