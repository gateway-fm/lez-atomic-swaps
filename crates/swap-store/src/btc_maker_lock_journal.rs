//! Durable ordered maker second-lock intent and one-attempt step authority.
//!
//! Exact public bytes are retained before any node call. Each ordered step has
//! one monotonic send authority: after `Started` or `Unknown`, absence can never
//! rearm a call. Exact observed completion may instead advance the step to
//! `Accepted` without granting submission authority.

use std::path::Path;

use lez_swap_core::{Participant, SwapId};
use lez_swap_sdk_core::{
    EXACT_PUBLIC_EFFECT_PLAN_SCHEMA_V1, ExactPublicEffectBytes, ExactPublicEffectPlanV1,
    ExpectedPublicEffectId, PublicEffectStepId, PublicEffectStepV1,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest as _, Sha256};

use crate::{StoreError, open_configured_connection, participant_name};

const MAKER_LOCK_PREDECESSOR_REVISION: u64 = 1;

/// Immutable maker-only second-lock intent staged at the exact aggregate head.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcMakerLockIntentV1 {
    swap_id: SwapId,
    agreement_commitment: [u8; 32],
    local_role: Participant,
    predecessor_revision: u64,
    plan: ExactPublicEffectPlanV1,
}

impl BtcMakerLockIntentV1 {
    /// Constructs a maker second-lock intent from an exact ordered public plan.
    ///
    /// # Errors
    ///
    /// Rejects a non-maker role, any predecessor other than the Bitcoin
    /// lifecycle's revision one, a zero agreement commitment, or an invalid
    /// exact plan reconstruction.
    pub fn new(
        swap_id: SwapId,
        agreement_commitment: [u8; 32],
        local_role: Participant,
        predecessor_revision: u64,
        plan: ExactPublicEffectPlanV1,
    ) -> Result<Self, StoreError> {
        let candidate = Self {
            swap_id,
            agreement_commitment,
            local_role,
            predecessor_revision,
            plan,
        };
        candidate.validate()?;
        Ok(candidate)
    }

    /// Stable signed swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Commitment to the exact accepted agreement.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    /// Fixed local role, always [`Participant::Maker`].
    #[must_use]
    pub const fn local_role(&self) -> Participant {
        self.local_role
    }

    /// Exact aggregate head at which this plan is staged.
    #[must_use]
    pub const fn predecessor_revision(&self) -> u64 {
        self.predecessor_revision
    }

    /// Immutable ordered exact-public-effect plan.
    pub const fn plan(&self) -> &ExactPublicEffectPlanV1 {
        &self.plan
    }

    fn validate(&self) -> Result<(), StoreError> {
        let reconstructed = ExactPublicEffectPlanV1::new(self.plan.steps().to_vec())
            .map_err(|_| StoreError::InvalidBtcMakerLockIntent)?;
        if self.local_role != Participant::Maker
            || self.predecessor_revision != MAKER_LOCK_PREDECESSOR_REVISION
            || self.agreement_commitment.iter().all(|byte| *byte == 0)
            || i64::try_from(self.predecessor_revision).is_err()
            || reconstructed != self.plan
        {
            return Err(StoreError::InvalidBtcMakerLockIntent);
        }
        Ok(())
    }
}

/// Idempotent outcome of staging one immutable maker intent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcMakerLockIntentCreateOutcome {
    /// The intent and every ordered step were inserted atomically.
    Created,
    /// The byte-identical immutable intent was already durable.
    ExistingSame,
    /// The swap identity already names different immutable material.
    Conflict,
}

/// Monotonic durable state for one maker-lock plan step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcMakerLockStepState {
    /// Exact bytes are durable and no send was authorized.
    Prepared,
    /// The sole send authority was consumed before the node call.
    Started,
    /// A node-accepted, ambiguous, or contradictory result still needs exact
    /// confirmed evidence; only observation is allowed.
    Unknown,
    /// Exact confirmed chain observation proved completion.
    Accepted,
}

impl BtcMakerLockStepState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Started => "started",
            Self::Unknown => "unknown",
            Self::Accepted => "accepted",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "started" => Ok(Self::Started),
            "unknown" => Ok(Self::Unknown),
            "accepted" => Ok(Self::Accepted),
            _ => Err(StoreError::CorruptBtcMakerLockIntent),
        }
    }
}

/// Fully revalidated durable state for one ordered plan step.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcMakerLockStepSnapshot {
    step: PublicEffectStepV1,
    state: BtcMakerLockStepState,
    attempt_count: u32,
    revision: u64,
    submission_result: Option<BtcMakerLockSubmissionResult>,
}

impl BtcMakerLockStepSnapshot {
    /// Immutable exact step material.
    pub const fn step(&self) -> &PublicEffectStepV1 {
        &self.step
    }

    /// Current monotonic step state.
    pub const fn state(&self) -> BtcMakerLockStepState {
        self.state
    }

    /// Consumed fresh-send count, constrained to zero or one.
    #[must_use]
    pub const fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    /// Per-step compare-and-swap revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Exact node-call result retained after the sole authorized submission.
    ///
    /// `None` means no node result was durably recorded. In every case,
    /// canonical `Accepted` still requires a separate exact chain observation.
    #[must_use]
    pub const fn submission_result(&self) -> Option<&BtcMakerLockSubmissionResult> {
        self.submission_result.as_ref()
    }
}

/// Fully reconstructed maker intent and all ordered step states.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcMakerLockIntentSnapshot {
    intent: BtcMakerLockIntentV1,
    steps: Vec<BtcMakerLockStepSnapshot>,
    closed_revision: Option<u64>,
}

impl BtcMakerLockIntentSnapshot {
    /// Immutable exact intent reconstructed from durable step rows.
    pub const fn intent(&self) -> &BtcMakerLockIntentV1 {
        &self.intent
    }

    /// Ordered step state matching the intent plan exactly.
    pub fn steps(&self) -> &[BtcMakerLockStepSnapshot] {
        &self.steps
    }

    /// Aggregate revision atomically committed with maker-lock evidence.
    #[must_use]
    pub const fn closed_revision(&self) -> Option<u64> {
        self.closed_revision
    }
}

/// Fresh chain result used before deciding whether one step may be sent.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcMakerLockStepObservation {
    /// Chain evidence proved this exact public identity and complete bytes.
    PresentExact {
        /// Observed chain-native public identity.
        expected_public_id: Box<str>,
        /// Complete observed public wire bytes.
        exact_public_bytes: Vec<u8>,
    },
    /// A bounded fresh lookup proved this exact effect absent.
    Absent,
    /// The lookup could not prove exact presence or absence.
    Uncertain,
    /// Presence contradicted the retained exact plan and burns send authority.
    ConflictingPresence,
}

/// Action returned after durable step reconciliation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcMakerLockStepDecision {
    /// The Prepared-to-Started CAS committed; this caller may send once.
    SubmitOnce(BtcMakerLockStepSnapshot),
    /// No transport call is authorized; exact observation only.
    ObserveOnly(BtcMakerLockStepSnapshot),
}

/// Outcome recorded immediately after the one authorized node call.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcMakerLockSubmissionResult {
    /// The node admitted the retained expected public identity. This consumes
    /// send authority but does not become `Accepted` before exact observation.
    Accepted(Box<str>),
    /// Transport or response ambiguity requires observation-only recovery.
    Unknown,
}

impl BtcMakerLockSubmissionResult {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Accepted(_) => "accepted",
            Self::Unknown => "unknown",
        }
    }

    fn parse(value: Option<&str>, step: &PublicEffectStepV1) -> Result<Option<Self>, StoreError> {
        match value {
            None => Ok(None),
            Some("accepted") => Ok(Some(Self::Accepted(
                step.expected_public_id().as_str().into(),
            ))),
            Some("unknown") => Ok(Some(Self::Unknown)),
            Some(_) => Err(StoreError::CorruptBtcMakerLockIntent),
        }
    }
}

/// Idempotent result of recording one submission outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcMakerLockStepCommit {
    snapshot: BtcMakerLockStepSnapshot,
    was_replay: bool,
}

impl BtcMakerLockStepCommit {
    /// Validated durable step state after the call.
    pub const fn snapshot(&self) -> &BtcMakerLockStepSnapshot {
        &self.snapshot
    }

    /// Whether the exact requested result was already durable.
    #[must_use]
    pub const fn was_replay(&self) -> bool {
        self.was_replay
    }
}

/// SQLite-backed durable maker second-lock journal.
#[derive(Debug)]
pub struct SqliteBtcMakerLockJournal {
    connection: Connection,
}

impl SqliteBtcMakerLockJournal {
    /// Opens or creates the additive maker-lock schema in the hardened store.
    ///
    /// # Errors
    ///
    /// Returns a store error for unsafe files, malformed schemas, or migration
    /// failures. Existing public-effect tables and rows are never rewritten.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let mut connection = open_configured_connection(path)?;
        migrate_btc_maker_lock_journal(&mut connection)?;
        Ok(Self { connection })
    }

    /// Atomically persists the intent header and every exact ordered plan step.
    ///
    /// # Errors
    ///
    /// Rejects invalid caller material, corrupt durable rows, or storage failure.
    pub fn create_intent(
        &mut self,
        intent: &BtcMakerLockIntentV1,
    ) -> Result<BtcMakerLockIntentCreateOutcome, StoreError> {
        intent.validate()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = load_intent_snapshot(&transaction, intent.swap_id())? {
            let outcome = if existing.intent == *intent {
                BtcMakerLockIntentCreateOutcome::ExistingSame
            } else {
                BtcMakerLockIntentCreateOutcome::Conflict
            };
            transaction.commit()?;
            return Ok(outcome);
        }
        transaction.execute(
            "INSERT INTO btc_maker_lock_intents (
                 swap_id, local_role, predecessor_revision, agreement_commitment,
                 plan_schema_version, plan_commitment, closed_revision
             ) VALUES (?1, 'maker', ?2, ?3, ?4, ?5, NULL)",
            params![
                intent.swap_id.as_str(),
                revision_to_sql(intent.predecessor_revision)?,
                intent.agreement_commitment.as_slice(),
                i64::from(intent.plan.schema_version().get()),
                intent.plan.commitment().as_slice(),
            ],
        )?;
        for (index, step) in intent.plan.steps().iter().enumerate() {
            transaction.execute(
                "INSERT INTO btc_maker_lock_steps (
                     swap_id, local_role, step_index, step_id, expected_public_id,
                     exact_public_bytes, public_bytes_sha256, state, attempt_count, revision
                 ) VALUES (?1, 'maker', ?2, ?3, ?4, ?5, ?6, 'prepared', 0, 0)",
                params![
                    intent.swap_id.as_str(),
                    i64::try_from(index).map_err(|_| StoreError::InvalidBtcMakerLockIntent)?,
                    step.step().as_str(),
                    step.expected_public_id().as_str(),
                    step.exact_bytes().as_slice(),
                    step.exact_bytes().sha256().as_slice(),
                ],
            )?;
        }
        let inserted = load_intent_snapshot(&transaction, intent.swap_id())?
            .ok_or(StoreError::CorruptBtcMakerLockIntent)?;
        if inserted.intent != *intent
            || inserted.closed_revision.is_some()
            || inserted.steps.iter().any(|step| {
                step.state != BtcMakerLockStepState::Prepared
                    || step.attempt_count != 0
                    || step.revision != 0
                    || step.submission_result.is_some()
            })
        {
            return Err(StoreError::CorruptBtcMakerLockIntent);
        }
        transaction.commit()?;
        Ok(BtcMakerLockIntentCreateOutcome::Created)
    }

    /// Loads and fully reconstructs one immutable plan and all step states.
    ///
    /// # Errors
    ///
    /// Returns corruption for any header, order, digest, exact-plan, or state
    /// mismatch. Absence is represented by `None`.
    pub fn load_intent(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<BtcMakerLockIntentSnapshot>, StoreError> {
        load_intent_snapshot(&self.connection, swap_id)
    }

    /// Reconciles one ordered step with a fresh exact chain observation.
    ///
    /// # Errors
    ///
    /// Rejects missing, changed, out-of-order, conflicting, corrupt, or stale
    /// durable state without partially applying a transition.
    pub fn reconcile_step(
        &mut self,
        intent: &BtcMakerLockIntentV1,
        step_id: &PublicEffectStepId,
        observation: BtcMakerLockStepObservation,
    ) -> Result<BtcMakerLockStepDecision, StoreError> {
        intent.validate()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let durable = require_matching_intent(&transaction, intent)?;
        let index = step_index(&durable, step_id)?;
        let mut step = durable.steps[index].clone();
        let predecessors_accepted = durable.steps[..index]
            .iter()
            .all(|candidate| candidate.state == BtcMakerLockStepState::Accepted);
        if step.state == BtcMakerLockStepState::Accepted && !predecessors_accepted {
            return Err(StoreError::CorruptBtcMakerLockIntent);
        }
        let mut submit_once = false;
        match observation {
            BtcMakerLockStepObservation::PresentExact {
                expected_public_id,
                exact_public_bytes,
            } => {
                if expected_public_id.as_ref() != step.step.expected_public_id().as_str()
                    || exact_public_bytes.as_slice() != step.step.exact_bytes().as_slice()
                {
                    return Err(StoreError::BtcMakerLockConflict);
                }
                if !predecessors_accepted {
                    return Err(StoreError::BtcMakerLockConflict);
                }
                if step.state != BtcMakerLockStepState::Accepted {
                    advance_step_to_accepted(&transaction, intent, index, &mut step)?;
                }
            }
            BtcMakerLockStepObservation::Absent => {
                if durable.closed_revision.is_none()
                    && predecessors_accepted
                    && step.state == BtcMakerLockStepState::Prepared
                {
                    begin_step_once(&transaction, intent, index, &mut step)?;
                    submit_once = true;
                }
            }
            BtcMakerLockStepObservation::Uncertain => {}
            BtcMakerLockStepObservation::ConflictingPresence => match step.state {
                BtcMakerLockStepState::Prepared => {
                    burn_step_authority(&transaction, intent, index, &mut step)?;
                }
                BtcMakerLockStepState::Started | BtcMakerLockStepState::Unknown => {}
                BtcMakerLockStepState::Accepted => {
                    return Err(StoreError::BtcMakerLockConflict);
                }
            },
        }
        let post = load_step_snapshot(&transaction, intent, index)?;
        if post != step {
            return Err(StoreError::CorruptBtcMakerLockIntent);
        }
        transaction.commit()?;
        if submit_once {
            Ok(BtcMakerLockStepDecision::SubmitOnce(step))
        } else {
            Ok(BtcMakerLockStepDecision::ObserveOnly(step))
        }
    }

    /// Records the immediate outcome of the sole authorized node call.
    ///
    /// # Errors
    ///
    /// Only Started may transition. Exact terminal replay is idempotent; a
    /// changed identity, pre-start result, or post-observation rewrite conflicts.
    pub fn record_submission_result(
        &mut self,
        intent: &BtcMakerLockIntentV1,
        step_id: &PublicEffectStepId,
        result: &BtcMakerLockSubmissionResult,
    ) -> Result<BtcMakerLockStepCommit, StoreError> {
        intent.validate()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let durable = require_matching_intent(&transaction, intent)?;
        if durable.closed_revision.is_some() {
            return Err(StoreError::BtcMakerLockConflict);
        }
        let index = step_index(&durable, step_id)?;
        if !durable.steps[..index]
            .iter()
            .all(|candidate| candidate.state == BtcMakerLockStepState::Accepted)
        {
            return Err(StoreError::BtcMakerLockConflict);
        }
        let mut step = durable.steps[index].clone();
        match result {
            BtcMakerLockSubmissionResult::Accepted(expected_id) => {
                if expected_id.as_ref() != step.step.expected_public_id().as_str() {
                    return Err(StoreError::BtcMakerLockConflict);
                }
            }
            BtcMakerLockSubmissionResult::Unknown => {}
        }
        let target = BtcMakerLockStepState::Unknown;
        let was_replay = if step.state == BtcMakerLockStepState::Started {
            set_started_result(
                &transaction,
                intent,
                index,
                &mut step,
                target,
                result.clone(),
            )?;
            false
        } else if step.state == target
            && step.attempt_count == 1
            && step.revision == 2
            && step.submission_result.as_ref() == Some(result)
        {
            true
        } else {
            return Err(StoreError::BtcMakerLockConflict);
        };
        let post = load_step_snapshot(&transaction, intent, index)?;
        if post != step {
            return Err(StoreError::CorruptBtcMakerLockIntent);
        }
        transaction.commit()?;
        Ok(BtcMakerLockStepCommit {
            snapshot: step,
            was_replay,
        })
    }
}

pub(crate) fn migrate_btc_maker_lock_journal(
    connection: &mut Connection,
) -> Result<(), StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS btc_maker_lock_intents (
             swap_id                 TEXT NOT NULL,
             local_role              TEXT NOT NULL CHECK (local_role = 'maker'),
             predecessor_revision    INTEGER NOT NULL CHECK (predecessor_revision = 1),
             agreement_commitment    BLOB NOT NULL CHECK (
                 length(agreement_commitment) = 32
                 AND agreement_commitment != zeroblob(32)
             ),
             plan_schema_version     INTEGER NOT NULL CHECK (plan_schema_version = 1),
             plan_commitment         BLOB NOT NULL CHECK (length(plan_commitment) = 32),
             closed_revision         INTEGER CHECK (
                 closed_revision IS NULL
                 OR closed_revision = predecessor_revision + 1
             ),
             PRIMARY KEY (swap_id, local_role)
         ) STRICT;

         CREATE TABLE IF NOT EXISTS btc_maker_lock_steps (
             swap_id                 TEXT NOT NULL,
             local_role              TEXT NOT NULL CHECK (local_role = 'maker'),
             step_index              INTEGER NOT NULL CHECK (step_index BETWEEN 0 AND 31),
             step_id                 TEXT NOT NULL CHECK (length(step_id) BETWEEN 1 AND 96),
             expected_public_id      TEXT NOT NULL CHECK (
                 length(expected_public_id) BETWEEN 1 AND 512
             ),
             exact_public_bytes      BLOB NOT NULL CHECK (
                 length(exact_public_bytes) BETWEEN 1 AND 4194304
             ),
             public_bytes_sha256     BLOB NOT NULL CHECK (length(public_bytes_sha256) = 32),
             submission_result       TEXT CHECK (
                 submission_result IS NULL
                 OR submission_result IN ('accepted', 'unknown')
             ),
             state                   TEXT NOT NULL CHECK (
                 state IN ('prepared', 'started', 'unknown', 'accepted')
             ),
             attempt_count           INTEGER NOT NULL CHECK (attempt_count IN (0, 1)),
             revision                INTEGER NOT NULL CHECK (revision BETWEEN 0 AND 3),
             PRIMARY KEY (swap_id, local_role, step_index),
             UNIQUE (swap_id, local_role, step_id),
             FOREIGN KEY (swap_id, local_role)
                 REFERENCES btc_maker_lock_intents(swap_id, local_role) ON DELETE RESTRICT,
             CHECK (
                 (state = 'prepared' AND attempt_count = 0 AND revision = 0
                     AND submission_result IS NULL)
                 OR (state = 'started' AND attempt_count = 1 AND revision = 1
                     AND submission_result IS NULL)
                 OR (state = 'unknown' AND attempt_count = 1 AND revision = 2)
                 OR (state = 'accepted' AND (
                     (attempt_count = 0 AND revision = 1)
                     OR (attempt_count = 1 AND revision IN (2, 3))
                 ))
             )
         ) STRICT;",
    )?;
    validate_maker_lock_schema(&transaction)?;
    transaction.commit()?;
    Ok(())
}

fn validate_maker_lock_schema(connection: &Connection) -> Result<(), StoreError> {
    const INTENT_COLUMNS: &[(i64, &str, &str, i64, i64)] = &[
        (0, "swap_id", "TEXT", 1, 1),
        (1, "local_role", "TEXT", 1, 2),
        (2, "predecessor_revision", "INTEGER", 1, 0),
        (3, "agreement_commitment", "BLOB", 1, 0),
        (4, "plan_schema_version", "INTEGER", 1, 0),
        (5, "plan_commitment", "BLOB", 1, 0),
        (6, "closed_revision", "INTEGER", 0, 0),
    ];
    const STEP_COLUMNS: &[(i64, &str, &str, i64, i64)] = &[
        (0, "swap_id", "TEXT", 1, 1),
        (1, "local_role", "TEXT", 1, 2),
        (2, "step_index", "INTEGER", 1, 3),
        (3, "step_id", "TEXT", 1, 0),
        (4, "expected_public_id", "TEXT", 1, 0),
        (5, "exact_public_bytes", "BLOB", 1, 0),
        (6, "public_bytes_sha256", "BLOB", 1, 0),
        (7, "submission_result", "TEXT", 0, 0),
        (8, "state", "TEXT", 1, 0),
        (9, "attempt_count", "INTEGER", 1, 0),
        (10, "revision", "INTEGER", 1, 0),
    ];
    if !table_schema_matches(connection, "btc_maker_lock_intents", INTENT_COLUMNS)?
        || !table_schema_matches(connection, "btc_maker_lock_steps", STEP_COLUMNS)?
        || maker_lock_trigger_count(connection)? != 0
        || !maker_lock_constraints_match(connection)?
        || !maker_lock_foreign_key_matches(connection)?
    {
        return Err(StoreError::CorruptBtcMakerLockIntent);
    }
    Ok(())
}

type SchemaColumn = (i64, String, String, i64, Option<String>, i64, i64);

fn table_schema_matches(
    connection: &Connection,
    table: &str,
    expected: &[(i64, &str, &str, i64, i64)],
) -> Result<bool, StoreError> {
    let mut statement = connection.prepare("SELECT * FROM pragma_table_xinfo(?1)")?;
    let actual = statement
        .query_map([table], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })?
        .collect::<Result<Vec<SchemaColumn>, _>>()?;
    let strict: Option<i64> = connection
        .query_row(
            "SELECT strict FROM pragma_table_list
             WHERE schema = 'main' AND name = ?1 AND type = 'table'",
            [table],
            |row| row.get(0),
        )
        .optional()?;
    Ok(strict == Some(1)
        && actual.len() == expected.len()
        && actual.iter().zip(expected).all(
            |((cid, name, kind, not_null, default, primary_key, hidden), expected)| {
                (*cid, name.as_str(), kind.as_str(), *not_null, *primary_key) == *expected
                    && default.is_none()
                    && *hidden == 0
            },
        ))
}

fn maker_lock_trigger_count(connection: &Connection) -> Result<i64, StoreError> {
    Ok(connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema
         WHERE type = 'trigger'
           AND tbl_name IN ('btc_maker_lock_intents', 'btc_maker_lock_steps')",
        [],
        |row| row.get(0),
    )?)
}

fn maker_lock_constraints_match(connection: &Connection) -> Result<bool, StoreError> {
    let intent_sql = normalized_table_sql(connection, "btc_maker_lock_intents")?;
    let step_sql = normalized_table_sql(connection, "btc_maker_lock_steps")?;
    let intent_checks = [
        "CHECK (local_role = 'maker')",
        "CHECK (predecessor_revision = 1)",
        "length(agreement_commitment) = 32",
        "agreement_commitment != zeroblob(32)",
        "CHECK (plan_schema_version = 1)",
        "length(plan_commitment) = 32",
        "closed_revision = predecessor_revision + 1",
        "PRIMARY KEY (swap_id, local_role)",
    ];
    let step_checks = [
        "CHECK (local_role = 'maker')",
        "step_index BETWEEN 0 AND 31",
        "length(step_id) BETWEEN 1 AND 96",
        "length(expected_public_id) BETWEEN 1 AND 512",
        "length(exact_public_bytes) BETWEEN 1 AND 4194304",
        "length(public_bytes_sha256) = 32",
        "submission_result IN ('accepted', 'unknown')",
        "state IN ('prepared', 'started', 'unknown', 'accepted')",
        "attempt_count IN (0, 1)",
        "revision BETWEEN 0 AND 3",
        "PRIMARY KEY (swap_id, local_role, step_index)",
        "UNIQUE (swap_id, local_role, step_id)",
        "FOREIGN KEY (swap_id, local_role) REFERENCES btc_maker_lock_intents(swap_id, local_role) ON DELETE RESTRICT",
        "state = 'prepared' AND attempt_count = 0 AND revision = 0 AND submission_result IS NULL",
        "state = 'started' AND attempt_count = 1 AND revision = 1 AND submission_result IS NULL",
        "state = 'unknown' AND attempt_count = 1 AND revision = 2",
    ];
    Ok(intent_checks
        .iter()
        .all(|required| intent_sql.contains(required))
        && step_checks
            .iter()
            .all(|required| step_sql.contains(required)))
}

fn normalized_table_sql(connection: &Connection, table: &str) -> Result<String, StoreError> {
    let sql: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )
        .optional()?;
    Ok(sql
        .as_deref()
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default())
}

type ForeignKeyRow = (i64, i64, String, String, String, String, String, String);

fn maker_lock_foreign_key_matches(connection: &Connection) -> Result<bool, StoreError> {
    let mut statement = connection.prepare("SELECT * FROM pragma_foreign_key_list(?1)")?;
    let rows = statement
        .query_map(["btc_maker_lock_steps"], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
            ))
        })?
        .collect::<Result<Vec<ForeignKeyRow>, _>>()?;
    Ok(rows
        == [
            (
                0,
                0,
                "btc_maker_lock_intents".to_owned(),
                "swap_id".to_owned(),
                "swap_id".to_owned(),
                "NO ACTION".to_owned(),
                "RESTRICT".to_owned(),
                "NONE".to_owned(),
            ),
            (
                0,
                1,
                "btc_maker_lock_intents".to_owned(),
                "local_role".to_owned(),
                "local_role".to_owned(),
                "NO ACTION".to_owned(),
                "RESTRICT".to_owned(),
                "NONE".to_owned(),
            ),
        ])
}

type StepRow = (
    i64,
    String,
    String,
    Vec<u8>,
    Vec<u8>,
    Option<String>,
    String,
    i64,
    i64,
);

fn load_intent_snapshot(
    connection: &Connection,
    swap_id: &SwapId,
) -> Result<Option<BtcMakerLockIntentSnapshot>, StoreError> {
    let header = connection
        .query_row(
            "SELECT local_role, predecessor_revision, agreement_commitment,
                    plan_schema_version, plan_commitment, closed_revision
             FROM btc_maker_lock_intents WHERE swap_id = ?1",
            [swap_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((role, predecessor, agreement, schema, plan_commitment, closed)) = header else {
        return Ok(None);
    };
    if role != participant_name(Participant::Maker)
        || schema != i64::from(EXACT_PUBLIC_EFFECT_PLAN_SCHEMA_V1.get())
    {
        return Err(StoreError::CorruptBtcMakerLockIntent);
    }

    let (plan, snapshots) = load_exact_step_plan(connection, swap_id)?;
    if plan.commitment().as_slice() != plan_commitment.as_slice() {
        return Err(StoreError::CorruptBtcMakerLockIntent);
    }
    let agreement_commitment = <[u8; 32]>::try_from(agreement.as_slice())
        .map_err(|_| StoreError::CorruptBtcMakerLockIntent)?;
    let predecessor_revision =
        u64::try_from(predecessor).map_err(|_| StoreError::CorruptBtcMakerLockIntent)?;
    let intent = BtcMakerLockIntentV1::new(
        swap_id.clone(),
        agreement_commitment,
        Participant::Maker,
        predecessor_revision,
        plan,
    )
    .map_err(|_| StoreError::CorruptBtcMakerLockIntent)?;
    let closed_revision = closed
        .map(|value| u64::try_from(value).map_err(|_| StoreError::CorruptBtcMakerLockIntent))
        .transpose()?;
    if let Some(closed_revision) = closed_revision
        && (closed_revision != predecessor_revision + 1
            || snapshots
                .iter()
                .any(|step| step.state != BtcMakerLockStepState::Accepted))
    {
        return Err(StoreError::CorruptBtcMakerLockIntent);
    }
    Ok(Some(BtcMakerLockIntentSnapshot {
        intent,
        steps: snapshots,
        closed_revision,
    }))
}

fn load_exact_step_plan(
    connection: &Connection,
    swap_id: &SwapId,
) -> Result<(ExactPublicEffectPlanV1, Vec<BtcMakerLockStepSnapshot>), StoreError> {
    let mut statement = connection.prepare(
        "SELECT step_index, step_id, expected_public_id, exact_public_bytes,
                public_bytes_sha256, submission_result, state, attempt_count, revision
         FROM btc_maker_lock_steps
         WHERE swap_id = ?1 AND local_role = 'maker'
         ORDER BY step_index ASC",
    )?;
    let rows = statement
        .query_map([swap_id.as_str()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
            ))
        })?
        .collect::<Result<Vec<StepRow>, _>>()?;
    if rows.is_empty() {
        return Err(StoreError::CorruptBtcMakerLockIntent);
    }
    let mut exact_steps = Vec::with_capacity(rows.len());
    let mut snapshots = Vec::with_capacity(rows.len());
    for (expected_index, row) in rows.into_iter().enumerate() {
        let (
            index,
            step_id,
            expected_id,
            bytes,
            digest,
            submission_result,
            state,
            attempts,
            revision,
        ) = row;
        if index != i64::try_from(expected_index).unwrap_or(i64::MAX)
            || <[u8; 32]>::try_from(digest.as_slice())
                .map_err(|_| StoreError::CorruptBtcMakerLockIntent)?
                != <[u8; 32]>::from(Sha256::digest(&bytes))
        {
            return Err(StoreError::CorruptBtcMakerLockIntent);
        }
        let step = PublicEffectStepV1::new(
            PublicEffectStepId::new(step_id).map_err(|_| StoreError::CorruptBtcMakerLockIntent)?,
            ExpectedPublicEffectId::new(expected_id)
                .map_err(|_| StoreError::CorruptBtcMakerLockIntent)?,
            ExactPublicEffectBytes::new(bytes)
                .map_err(|_| StoreError::CorruptBtcMakerLockIntent)?,
        );
        let submission_result =
            BtcMakerLockSubmissionResult::parse(submission_result.as_deref(), &step)?;
        let state = BtcMakerLockStepState::parse(&state)?;
        let attempt_count =
            u32::try_from(attempts).map_err(|_| StoreError::CorruptBtcMakerLockIntent)?;
        let revision =
            u64::try_from(revision).map_err(|_| StoreError::CorruptBtcMakerLockIntent)?;
        validate_step_shape(state, attempt_count, revision, submission_result.as_ref())?;
        exact_steps.push(step.clone());
        snapshots.push(BtcMakerLockStepSnapshot {
            step,
            state,
            attempt_count,
            revision,
            submission_result,
        });
    }
    let plan = ExactPublicEffectPlanV1::new(exact_steps)
        .map_err(|_| StoreError::CorruptBtcMakerLockIntent)?;
    Ok((plan, snapshots))
}

fn require_matching_intent(
    connection: &Connection,
    intent: &BtcMakerLockIntentV1,
) -> Result<BtcMakerLockIntentSnapshot, StoreError> {
    let durable = load_intent_snapshot(connection, intent.swap_id())?
        .ok_or(StoreError::MissingBtcMakerLockIntent)?;
    if durable.intent != *intent {
        return Err(StoreError::BtcMakerLockConflict);
    }
    Ok(durable)
}

fn step_index(
    durable: &BtcMakerLockIntentSnapshot,
    step_id: &PublicEffectStepId,
) -> Result<usize, StoreError> {
    durable
        .steps
        .iter()
        .position(|step| step.step.step() == step_id)
        .ok_or(StoreError::BtcMakerLockConflict)
}

fn load_step_snapshot(
    connection: &Connection,
    intent: &BtcMakerLockIntentV1,
    index: usize,
) -> Result<BtcMakerLockStepSnapshot, StoreError> {
    load_intent_snapshot(connection, intent.swap_id())?
        .ok_or(StoreError::MissingBtcMakerLockIntent)?
        .steps
        .get(index)
        .cloned()
        .ok_or(StoreError::CorruptBtcMakerLockIntent)
}

fn begin_step_once(
    transaction: &rusqlite::Transaction<'_>,
    intent: &BtcMakerLockIntentV1,
    index: usize,
    step: &mut BtcMakerLockStepSnapshot,
) -> Result<(), StoreError> {
    update_step(
        transaction,
        intent,
        index,
        step,
        BtcMakerLockStepState::Started,
        1,
        1,
        None,
    )
}

fn burn_step_authority(
    transaction: &rusqlite::Transaction<'_>,
    intent: &BtcMakerLockIntentV1,
    index: usize,
    step: &mut BtcMakerLockStepSnapshot,
) -> Result<(), StoreError> {
    update_step(
        transaction,
        intent,
        index,
        step,
        BtcMakerLockStepState::Unknown,
        1,
        2,
        None,
    )
}

fn advance_step_to_accepted(
    transaction: &rusqlite::Transaction<'_>,
    intent: &BtcMakerLockIntentV1,
    index: usize,
    step: &mut BtcMakerLockStepSnapshot,
) -> Result<(), StoreError> {
    let revision = step
        .revision
        .checked_add(1)
        .ok_or(StoreError::CorruptBtcMakerLockIntent)?;
    let submission_result = step.submission_result.clone();
    update_step(
        transaction,
        intent,
        index,
        step,
        BtcMakerLockStepState::Accepted,
        step.attempt_count,
        revision,
        submission_result,
    )
}

fn set_started_result(
    transaction: &rusqlite::Transaction<'_>,
    intent: &BtcMakerLockIntentV1,
    index: usize,
    step: &mut BtcMakerLockStepSnapshot,
    target: BtcMakerLockStepState,
    submission_result: BtcMakerLockSubmissionResult,
) -> Result<(), StoreError> {
    update_step(
        transaction,
        intent,
        index,
        step,
        target,
        1,
        2,
        Some(submission_result),
    )
}

#[allow(clippy::too_many_arguments)]
fn update_step(
    transaction: &rusqlite::Transaction<'_>,
    intent: &BtcMakerLockIntentV1,
    index: usize,
    step: &mut BtcMakerLockStepSnapshot,
    next_state: BtcMakerLockStepState,
    next_attempts: u32,
    next_revision: u64,
    next_submission_result: Option<BtcMakerLockSubmissionResult>,
) -> Result<(), StoreError> {
    let next_submission_name = next_submission_result
        .as_ref()
        .map(BtcMakerLockSubmissionResult::as_str);
    let current_submission_name = step
        .submission_result
        .as_ref()
        .map(BtcMakerLockSubmissionResult::as_str);
    let updated = transaction.execute(
        "UPDATE btc_maker_lock_steps
         SET state = ?4, attempt_count = ?5, revision = ?6, submission_result = ?7
         WHERE swap_id = ?1 AND local_role = 'maker' AND step_index = ?2
           AND step_id = ?3 AND state = ?8 AND attempt_count = ?9 AND revision = ?10
           AND submission_result IS ?11",
        params![
            intent.swap_id.as_str(),
            i64::try_from(index).map_err(|_| StoreError::BtcMakerLockConflict)?,
            step.step.step().as_str(),
            next_state.as_str(),
            i64::from(next_attempts),
            revision_to_sql(next_revision)?,
            next_submission_name,
            step.state.as_str(),
            i64::from(step.attempt_count),
            revision_to_sql(step.revision)?,
            current_submission_name,
        ],
    )?;
    if updated != 1 {
        return Err(StoreError::BtcMakerLockConflict);
    }
    step.state = next_state;
    step.attempt_count = next_attempts;
    step.revision = next_revision;
    step.submission_result = next_submission_result;
    Ok(())
}

fn validate_step_shape(
    state: BtcMakerLockStepState,
    attempt_count: u32,
    revision: u64,
    submission_result: Option<&BtcMakerLockSubmissionResult>,
) -> Result<(), StoreError> {
    let valid = match state {
        BtcMakerLockStepState::Prepared => {
            attempt_count == 0 && revision == 0 && submission_result.is_none()
        }
        BtcMakerLockStepState::Started => {
            attempt_count == 1 && revision == 1 && submission_result.is_none()
        }
        BtcMakerLockStepState::Unknown => attempt_count == 1 && revision == 2,
        BtcMakerLockStepState::Accepted => {
            (attempt_count == 0 && revision == 1 && submission_result.is_none())
                || (attempt_count == 1 && revision == 2 && submission_result.is_none())
                || (attempt_count == 1 && revision == 3)
        }
    };
    if valid {
        Ok(())
    } else {
        Err(StoreError::CorruptBtcMakerLockIntent)
    }
}

pub(crate) fn close_ready_btc_maker_lock_intent(
    transaction: &rusqlite::Transaction<'_>,
    swap_id: &SwapId,
    agreement_commitment: &[u8; 32],
    predecessor_revision: u64,
    committed_revision: u64,
) -> Result<bool, StoreError> {
    if predecessor_revision != MAKER_LOCK_PREDECESSOR_REVISION
        || committed_revision != predecessor_revision.saturating_add(1)
    {
        return Err(StoreError::BtcMakerLockConflict);
    }
    let durable =
        load_intent_snapshot(transaction, swap_id)?.ok_or(StoreError::MissingBtcMakerLockIntent)?;
    if durable.intent.local_role != Participant::Maker
        || durable.intent.agreement_commitment != *agreement_commitment
        || durable.intent.predecessor_revision != predecessor_revision
        || durable
            .steps
            .iter()
            .any(|step| step.state != BtcMakerLockStepState::Accepted)
    {
        return Err(StoreError::BtcMakerLockConflict);
    }
    if durable.closed_revision == Some(committed_revision) {
        return Ok(true);
    }
    if durable.closed_revision.is_some() {
        return Err(StoreError::BtcMakerLockConflict);
    }
    let updated = transaction.execute(
        "UPDATE btc_maker_lock_intents SET closed_revision = ?3
         WHERE swap_id = ?1 AND local_role = 'maker'
           AND predecessor_revision = ?2 AND closed_revision IS NULL",
        params![
            swap_id.as_str(),
            revision_to_sql(predecessor_revision)?,
            revision_to_sql(committed_revision)?,
        ],
    )?;
    if updated != 1 {
        return Err(StoreError::BtcMakerLockConflict);
    }
    Ok(false)
}

fn revision_to_sql(revision: u64) -> Result<i64, StoreError> {
    i64::try_from(revision).map_err(|_| StoreError::InvalidBtcMakerLockIntent)
}
