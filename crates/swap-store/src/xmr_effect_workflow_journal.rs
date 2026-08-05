//! Durable orchestration authority for role-fixed XMR effect workflows.
//!
//! This journal coordinates role-local steps. The tag-15/tag-16 sidecar
//! journals remain the sole one-attempt authorities for actual LEZ sends.

use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use lez_swap_core::{Participant, SwapId};
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, TransactionBehavior, params};

use crate::{StoreError, participant_name};

const APPLICATION_ID: i64 = 0x4c58_5752;
const SCHEMA_VERSION: i64 = 3;
const MAX_RUN_ID_BYTES: usize = 128;

const CREATE_SCHEMA_V2: &str = "
CREATE TABLE xmr_workflow_identity (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    swap_id TEXT NOT NULL CHECK (length(swap_id) = 64),
    local_role TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
    run_id TEXT NOT NULL CHECK (length(run_id) BETWEEN 1 AND 128),
    agreement_commitment BLOB NOT NULL CHECK (length(agreement_commitment) = 32),
    activation_commitment BLOB NOT NULL CHECK (length(activation_commitment) = 32),
    authority_sha256 BLOB NOT NULL CHECK (length(authority_sha256) = 32),
    selected_branch TEXT CHECK (selected_branch IN ('claim', 'refund')),
    revision INTEGER NOT NULL CHECK (
        (selected_branch IS NULL AND revision = 0)
        OR (selected_branch IS NOT NULL AND revision = 1)
    )
) STRICT;
CREATE TABLE xmr_workflow_steps (
    step TEXT PRIMARY KEY CHECK (step IN (
        'initialize_lez_tag13', 'fund_lez_tag13', 'fund_monero',
        'authorize_lez_tag14', 'claim_lez_tag15', 'sweep_monero_claim',
        'refund_lez_tag16', 'sweep_monero_refund'
    )),
    singleton_id INTEGER NOT NULL CHECK (singleton_id = 1),
    local_role TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
    scope TEXT NOT NULL CHECK (scope IN ('common', 'claim', 'refund')),
    state TEXT NOT NULL CHECK (
        state IN ('prepared', 'started', 'succeeded', 'unknown')
    ),
    attempt_count INTEGER NOT NULL CHECK (attempt_count IN (0, 1)),
    revision INTEGER NOT NULL CHECK (
        (state = 'prepared' AND attempt_count = 0 AND revision = 0)
        OR (state = 'started' AND attempt_count = 1 AND revision = 1)
        OR (state = 'unknown' AND attempt_count = 1 AND revision = 2)
        OR (state = 'succeeded' AND attempt_count = 1 AND revision IN (2, 3))
    ),
    effect_evidence_sha256 BLOB,
    tool_plan_identity_sha256 BLOB,
    reconciliation_source TEXT CHECK (reconciliation_source IN (
        'lez_finalized_event', 'monero_wallet_transaction'
    )),
    CHECK (
        (state != 'succeeded'
         AND effect_evidence_sha256 IS NULL
         AND tool_plan_identity_sha256 IS NULL
         AND reconciliation_source IS NULL)
        OR
        (state = 'succeeded'
         AND length(effect_evidence_sha256) = 32
         AND length(tool_plan_identity_sha256) = 32
         AND reconciliation_source IS NOT NULL)
    ),
    FOREIGN KEY (singleton_id) REFERENCES xmr_workflow_identity(singleton_id)
) STRICT, WITHOUT ROWID;
";

fn create_schema_v3() -> String {
    CREATE_SCHEMA_V2
        .replacen(
            "selected_branch IN ('claim', 'refund')",
            "selected_branch IN ('claim', 'refund', 'punish')",
            1,
        )
        .replacen(
            "'refund_lez_tag16', 'sweep_monero_refund'",
            "'refund_lez_tag16', 'sweep_monero_refund', 'punish_lez_tag17'",
            1,
        )
        .replacen(
            "scope IN ('common', 'claim', 'refund')",
            "scope IN ('common', 'claim', 'refund', 'punish')",
            1,
        )
}

/// Immutable identity of one role-local XMR workflow.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrWorkflowIdentityV1 {
    swap_id: SwapId,
    local_role: Participant,
    run_id: Box<str>,
    agreement_commitment: [u8; 32],
    activation_commitment: [u8; 32],
    authority_sha256: [u8; 32],
}

impl XmrWorkflowIdentityV1 {
    /// Constructs a complete canonical workflow identity.
    ///
    /// # Errors
    ///
    /// Rejects malformed XMR swap/run identities or zero commitments.
    pub fn new(
        swap_id: SwapId,
        local_role: Participant,
        run_id: Box<str>,
        agreement_commitment: [u8; 32],
        activation_commitment: [u8; 32],
        authority_sha256: [u8; 32],
    ) -> Result<Self, StoreError> {
        let value = Self {
            swap_id,
            local_role,
            run_id,
            agreement_commitment,
            activation_commitment,
            authority_sha256,
        };
        validate_identity(&value)?;
        Ok(value)
    }

    /// Exact swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Role owning this workflow.
    #[must_use]
    pub const fn local_role(&self) -> Participant {
        self.local_role
    }
}

/// Irreversible successful or recovery branch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum XmrWorkflowBranch {
    /// Successful dual-reveal path.
    Claim,
    /// Deadline recovery path.
    Refund,
    /// Later Maker punishment after Taker abandonment.
    Punish,
}

impl XmrWorkflowBranch {
    const fn name(self) -> &'static str {
        match self {
            Self::Claim => "claim",
            Self::Refund => "refund",
            Self::Punish => "punish",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "claim" => Ok(Self::Claim),
            "refund" => Ok(Self::Refund),
            "punish" => Ok(Self::Punish),
            _ => Err(StoreError::CorruptXmrWorkflowState),
        }
    }
}

/// Branch-neutral or terminal-branch scope of one effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum XmrWorkflowStepScope {
    /// Effect required before either terminal branch.
    Common,
    /// Successful dual-reveal path.
    Claim,
    /// Deadline recovery path.
    Refund,
    /// Later Maker punishment after Taker abandonment.
    Punish,
}

impl XmrWorkflowStepScope {
    const fn name(self) -> &'static str {
        match self {
            Self::Common => "common",
            Self::Claim => "claim",
            Self::Refund => "refund",
            Self::Punish => "punish",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "common" => Ok(Self::Common),
            "claim" => Ok(Self::Claim),
            "refund" => Ok(Self::Refund),
            "punish" => Ok(Self::Punish),
            _ => Err(StoreError::CorruptXmrWorkflowState),
        }
    }

    const fn branch(self) -> Option<XmrWorkflowBranch> {
        match self {
            Self::Common => None,
            Self::Claim => Some(XmrWorkflowBranch::Claim),
            Self::Refund => Some(XmrWorkflowBranch::Refund),
            Self::Punish => Some(XmrWorkflowBranch::Punish),
        }
    }
}

/// Durable source that proved an external XMR workflow effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum XmrWorkflowReconciliationSource {
    /// Finalized LEZ event with the exact expected transaction/effect identity.
    LezFinalizedEvent,
    /// Confirmed Monero wallet transaction recovered from exact wallet history.
    MoneroWalletTransaction,
}

impl XmrWorkflowReconciliationSource {
    const fn name(self) -> &'static str {
        match self {
            Self::LezFinalizedEvent => "lez_finalized_event",
            Self::MoneroWalletTransaction => "monero_wallet_transaction",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "lez_finalized_event" => Ok(Self::LezFinalizedEvent),
            "monero_wallet_transaction" => Ok(Self::MoneroWalletTransaction),
            _ => Err(StoreError::CorruptXmrWorkflowState),
        }
    }
}

/// Exact persisted proof that one externally visible effect succeeded.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrWorkflowReconciliationV2 {
    effect_evidence_sha256: [u8; 32],
    tool_plan_identity_sha256: [u8; 32],
    source: XmrWorkflowReconciliationSource,
}

impl XmrWorkflowReconciliationV2 {
    /// Constructs nonzero effect and tool-plan evidence.
    ///
    /// # Errors
    ///
    /// Rejects an all-zero evidence or tool-plan identity digest.
    pub fn new(
        effect_evidence_sha256: [u8; 32],
        tool_plan_identity_sha256: [u8; 32],
        source: XmrWorkflowReconciliationSource,
    ) -> Result<Self, StoreError> {
        if effect_evidence_sha256.iter().all(|byte| *byte == 0)
            || tool_plan_identity_sha256.iter().all(|byte| *byte == 0)
        {
            return Err(StoreError::InvalidXmrWorkflowReconciliation);
        }
        Ok(Self {
            effect_evidence_sha256,
            tool_plan_identity_sha256,
            source,
        })
    }

    /// Digest of canonical external-effect evidence.
    #[must_use]
    pub const fn effect_evidence_sha256(&self) -> [u8; 32] {
        self.effect_evidence_sha256
    }

    /// Digest of the exact role-fixed tool plan used for the effect.
    #[must_use]
    pub const fn tool_plan_identity_sha256(&self) -> [u8; 32] {
        self.tool_plan_identity_sha256
    }

    /// External authority used to reconcile the effect.
    pub const fn source(&self) -> XmrWorkflowReconciliationSource {
        self.source
    }
}

/// Fixed role-specific application step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum XmrWorkflowStep {
    /// Taker initializes the LEZ tag-13 escrow.
    InitializeLezTag13,
    /// Taker funds the initialized LEZ tag-13 escrow.
    FundLezTag13,
    /// Maker funds the exact shared Monero output.
    FundMonero,
    /// Taker publishes the LEZ tag-14 claim authorization.
    AuthorizeLezTag14,
    /// Maker publishes the LEZ tag-15 claim.
    ClaimLezTag15,
    /// Taker sweeps the claim-path shared Monero output.
    SweepMoneroClaim,
    /// Taker publishes the LEZ tag-16 refund.
    RefundLezTag16,
    /// Maker sweeps the refund-path shared Monero output.
    SweepMoneroRefund,
    /// Maker publishes the later LEZ tag-17 punishment.
    PunishLezTag17,
}

impl XmrWorkflowStep {
    /// Complete stable effect-step catalog in protocol order.
    pub const ALL: [Self; 9] = [
        Self::InitializeLezTag13,
        Self::FundLezTag13,
        Self::FundMonero,
        Self::AuthorizeLezTag14,
        Self::ClaimLezTag15,
        Self::SweepMoneroClaim,
        Self::RefundLezTag16,
        Self::SweepMoneroRefund,
        Self::PunishLezTag17,
    ];

    /// Stable lowercase name used by durable state and effect-worker ABIs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::InitializeLezTag13 => "initialize_lez_tag13",
            Self::FundLezTag13 => "fund_lez_tag13",
            Self::FundMonero => "fund_monero",
            Self::AuthorizeLezTag14 => "authorize_lez_tag14",
            Self::ClaimLezTag15 => "claim_lez_tag15",
            Self::SweepMoneroClaim => "sweep_monero_claim",
            Self::RefundLezTag16 => "refund_lez_tag16",
            Self::SweepMoneroRefund => "sweep_monero_refund",
            Self::PunishLezTag17 => "punish_lez_tag17",
        }
    }

    /// Exact role allowed to invoke this effect.
    #[must_use]
    pub const fn role(self) -> Participant {
        match self {
            Self::FundMonero
            | Self::ClaimLezTag15
            | Self::SweepMoneroRefund
            | Self::PunishLezTag17 => Participant::Maker,
            Self::InitializeLezTag13
            | Self::FundLezTag13
            | Self::AuthorizeLezTag14
            | Self::SweepMoneroClaim
            | Self::RefundLezTag16 => Participant::Taker,
        }
    }

    const fn reconciliation_source(self) -> XmrWorkflowReconciliationSource {
        match self {
            Self::FundMonero | Self::SweepMoneroClaim | Self::SweepMoneroRefund => {
                XmrWorkflowReconciliationSource::MoneroWalletTransaction
            }
            Self::InitializeLezTag13
            | Self::FundLezTag13
            | Self::AuthorizeLezTag14
            | Self::ClaimLezTag15
            | Self::RefundLezTag16
            | Self::PunishLezTag17 => XmrWorkflowReconciliationSource::LezFinalizedEvent,
        }
    }

    /// Branch-neutral or terminal-branch scope.
    pub const fn scope(self) -> XmrWorkflowStepScope {
        match self {
            Self::InitializeLezTag13 | Self::FundLezTag13 | Self::FundMonero => {
                XmrWorkflowStepScope::Common
            }
            Self::AuthorizeLezTag14 | Self::ClaimLezTag15 | Self::SweepMoneroClaim => {
                XmrWorkflowStepScope::Claim
            }
            Self::RefundLezTag16 | Self::SweepMoneroRefund => XmrWorkflowStepScope::Refund,
            Self::PunishLezTag17 => XmrWorkflowStepScope::Punish,
        }
    }

    const fn predecessor(self) -> Option<Self> {
        match self {
            Self::InitializeLezTag13 | Self::FundMonero => None,
            Self::FundLezTag13 => Some(Self::InitializeLezTag13),
            Self::AuthorizeLezTag14 | Self::RefundLezTag16 => Some(Self::FundLezTag13),
            Self::ClaimLezTag15 | Self::SweepMoneroRefund | Self::PunishLezTag17 => {
                Some(Self::FundMonero)
            }
            Self::SweepMoneroClaim => Some(Self::AuthorizeLezTag14),
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "initialize_lez_tag13" => Ok(Self::InitializeLezTag13),
            "fund_lez_tag13" => Ok(Self::FundLezTag13),
            "fund_monero" => Ok(Self::FundMonero),
            "authorize_lez_tag14" => Ok(Self::AuthorizeLezTag14),
            "claim_lez_tag15" => Ok(Self::ClaimLezTag15),
            "sweep_monero_claim" => Ok(Self::SweepMoneroClaim),
            "refund_lez_tag16" => Ok(Self::RefundLezTag16),
            "sweep_monero_refund" => Ok(Self::SweepMoneroRefund),
            "punish_lez_tag17" => Ok(Self::PunishLezTag17),
            _ => Err(StoreError::CorruptXmrWorkflowState),
        }
    }
}

/// Result after reconciling a step with durable authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum XmrWorkflowDecision {
    /// This caller consumed the only invocation authority.
    InvokeOnce,
    /// Invocation may have happened; exact observation only.
    ObserveOnly,
    /// Exact reconciliation already proved completion.
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StepState {
    Prepared,
    Started,
    Succeeded,
    Unknown,
}

impl StepState {
    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "started" => Ok(Self::Started),
            "succeeded" => Ok(Self::Succeeded),
            "unknown" => Ok(Self::Unknown),
            _ => Err(StoreError::CorruptXmrWorkflowState),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Started => "started",
            Self::Succeeded => "succeeded",
            Self::Unknown => "unknown",
        }
    }
}

struct StepSnapshot {
    step: XmrWorkflowStep,
    role: Participant,
    scope: XmrWorkflowStepScope,
    state: StepState,
    attempts: u32,
    revision: u64,
    reconciliation: Option<XmrWorkflowReconciliationV2>,
}

/// SQLite-backed role-local XMR workflow journal.
#[derive(Debug)]
pub struct SqliteXmrWorkflowJournal {
    connection: Connection,
    path: PathBuf,
    identity: FileIdentity,
}

impl SqliteXmrWorkflowJournal {
    /// Exclusively creates a new 0600 workflow journal.
    ///
    /// # Errors
    ///
    /// Existing paths never reopen through this constructor.
    pub fn create_new(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        validate_path(path)?;
        let guard = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                classify_existing(path)?;
                return Err(StoreError::XmrWorkflowDatabaseAlreadyExists);
            }
            Err(_) => return Err(StoreError::DatabaseFileUnavailable),
        };
        let identity = validate_file(&guard)?;
        guard
            .sync_all()
            .map_err(|_| StoreError::DatabaseFileUnavailable)?;
        sync_parent(path)?;
        let mut journal = Self::open_connection(path, identity)?;
        initialize_schema(&mut journal.connection)?;
        validate_connection(&journal.connection)?;
        drop(guard);
        journal.revalidate_storage()?;
        Ok(journal)
    }

    /// Opens an existing exact supported journal without creating or migrating it.
    ///
    /// # Errors
    ///
    /// Missing, foreign, future, aliased, or corrupt journals fail closed.
    pub fn open_existing(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        validate_path(path)?;
        let file = open_checked(path)?;
        let identity = validate_file(&file)?;
        let journal = Self::open_connection(path, identity)?;
        validate_connection(&journal.connection)?;
        drop(file);
        journal.revalidate_storage()?;
        Ok(journal)
    }

    fn open_connection(path: &Path, identity: FileIdentity) -> Result<Self, StoreError> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let connection = Connection::open_with_flags(path, flags)?;
        configure(&connection)?;
        Ok(Self {
            connection,
            path: path.to_owned(),
            identity,
        })
    }

    /// Initializes or exactly replays the singleton immutable identity.
    ///
    /// # Errors
    ///
    /// Any identity drift or corrupt durable row fails closed.
    pub fn initialize(&mut self, identity: &XmrWorkflowIdentityV1) -> Result<(), StoreError> {
        validate_identity(identity)?;
        self.revalidate_storage()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        match load_identity(&transaction)? {
            None => {
                transaction.execute(
                    "INSERT INTO xmr_workflow_identity (
                         singleton_id, swap_id, local_role, run_id,
                         agreement_commitment, activation_commitment,
                         authority_sha256, selected_branch, revision
                     ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, NULL, 0)",
                    params![
                        identity.swap_id.as_str(),
                        participant_name(identity.local_role),
                        identity.run_id.as_ref(),
                        identity.agreement_commitment.as_slice(),
                        identity.activation_commitment.as_slice(),
                        identity.authority_sha256.as_slice(),
                    ],
                )?;
            }
            Some((durable, _, _)) if durable == *identity => {}
            Some(_) => return Err(StoreError::XmrWorkflowConflict),
        }
        transaction.commit()?;
        self.revalidate_storage()
    }

    /// Validates the exact initialized identity without creating or changing it.
    ///
    /// # Errors
    ///
    /// Missing, malformed, crossed, or changed durable identity fails closed.
    pub fn validate_initialized(&self, identity: &XmrWorkflowIdentityV1) -> Result<(), StoreError> {
        validate_identity(identity)?;
        self.revalidate_storage()?;
        let (durable, _, _) =
            load_identity(&self.connection)?.ok_or(StoreError::MissingXmrWorkflowIdentity)?;
        ensure_identity(identity, &durable)?;
        self.revalidate_storage()
    }

    /// Selects one branch with a durable compare-and-set.
    ///
    /// # Errors
    ///
    /// The losing branch, identity drift, or an incomplete common-step plan
    /// fails closed.
    pub fn select_branch(
        &mut self,
        identity: &XmrWorkflowIdentityV1,
        branch: XmrWorkflowBranch,
    ) -> Result<(), StoreError> {
        self.revalidate_storage()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (durable, selected, revision) =
            load_identity(&transaction)?.ok_or(StoreError::MissingXmrWorkflowIdentity)?;
        ensure_identity(identity, &durable)?;
        match (selected, revision) {
            (None, 0) => {
                ensure_common_prepared(&transaction, identity.local_role)?;
                let changed = transaction.execute(
                    "UPDATE xmr_workflow_identity
                     SET selected_branch = ?1, revision = 1
                     WHERE singleton_id = 1
                       AND selected_branch IS NULL AND revision = 0",
                    [branch.name()],
                )?;
                if changed != 1 {
                    return Err(StoreError::XmrWorkflowConflict);
                }
            }
            (Some(current), 1) if current == branch => {}
            (Some(_), 1) => return Err(StoreError::XmrWorkflowConflict),
            _ => return Err(StoreError::CorruptXmrWorkflowState),
        }
        transaction.commit()?;
        self.revalidate_storage()
    }

    /// Prepares a fixed role/scope step without invoking it.
    ///
    /// # Errors
    ///
    /// Wrong role, scope, predecessor, identity, or durable state fails closed.
    pub fn prepare_step(
        &mut self,
        identity: &XmrWorkflowIdentityV1,
        step: XmrWorkflowStep,
    ) -> Result<(), StoreError> {
        ensure_step_role(identity, step)?;
        self.revalidate_storage()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(snapshot) = load_step(&transaction, step)? {
            validate_step(identity, step, &snapshot)?;
        } else {
            ensure_scope(&transaction, identity, step.scope())?;
            ensure_predecessor(&transaction, step)?;
            transaction.execute(
                "INSERT INTO xmr_workflow_steps (
                     step, singleton_id, local_role, scope,
                     state, attempt_count, revision,
                     effect_evidence_sha256, tool_plan_identity_sha256,
                     reconciliation_source
                 ) VALUES (?1, 1, ?2, ?3, 'prepared', 0, 0, NULL, NULL, NULL)",
                params![
                    step.name(),
                    participant_name(identity.local_role),
                    step.scope().name(),
                ],
            )?;
        }
        transaction.commit()?;
        self.revalidate_storage()
    }

    /// Atomically consumes the only invocation authority.
    ///
    /// # Errors
    ///
    /// Missing, crossed, or corrupt state fails closed.
    pub fn authorize_once(
        &mut self,
        identity: &XmrWorkflowIdentityV1,
        step: XmrWorkflowStep,
    ) -> Result<XmrWorkflowDecision, StoreError> {
        ensure_step_role(identity, step)?;
        self.revalidate_storage()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_scope(&transaction, identity, step.scope())?;
        let snapshot = load_step(&transaction, step)?.ok_or(StoreError::MissingXmrWorkflowStep)?;
        validate_step(identity, step, &snapshot)?;
        let decision = match snapshot.state {
            StepState::Prepared => {
                let changed = transaction.execute(
                    "UPDATE xmr_workflow_steps
                     SET state = 'started', attempt_count = 1, revision = 1
                     WHERE step = ?1 AND state = 'prepared'
                       AND attempt_count = 0 AND revision = 0",
                    [step.name()],
                )?;
                if changed != 1 {
                    return Err(StoreError::XmrWorkflowConflict);
                }
                XmrWorkflowDecision::InvokeOnce
            }
            StepState::Started | StepState::Unknown => XmrWorkflowDecision::ObserveOnly,
            StepState::Succeeded => XmrWorkflowDecision::Complete,
        };
        transaction.commit()?;
        self.revalidate_storage()?;
        Ok(decision)
    }

    /// Marks an invoked step ambiguous without rearming it.
    ///
    /// # Errors
    ///
    /// Only Started may advance; exact Unknown replay is idempotent.
    pub fn mark_unknown(
        &mut self,
        identity: &XmrWorkflowIdentityV1,
        step: XmrWorkflowStep,
    ) -> Result<(), StoreError> {
        ensure_step_role(identity, step)?;
        self.revalidate_storage()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_scope(&transaction, identity, step.scope())?;
        let snapshot = load_step(&transaction, step)?.ok_or(StoreError::MissingXmrWorkflowStep)?;
        validate_step(identity, step, &snapshot)?;
        match snapshot.state {
            StepState::Started => {
                let changed = transaction.execute(
                    "UPDATE xmr_workflow_steps
                     SET state = 'unknown', revision = 2
                     WHERE step = ?1 AND state = 'started'
                       AND attempt_count = 1 AND revision = 1",
                    [step.name()],
                )?;
                if changed != 1 {
                    return Err(StoreError::XmrWorkflowConflict);
                }
            }
            StepState::Unknown => {}
            StepState::Prepared | StepState::Succeeded => {
                return Err(StoreError::XmrWorkflowConflict);
            }
        }
        transaction.commit()?;
        self.revalidate_storage()
    }

    /// Rejects the legacy evidence-free success transition.
    ///
    /// # Errors
    ///
    /// Supported schemas require `reconcile_succeeded` with exact evidence.
    pub fn mark_succeeded(
        &mut self,
        identity: &XmrWorkflowIdentityV1,
        step: XmrWorkflowStep,
    ) -> Result<(), StoreError> {
        ensure_step_role(identity, step)?;
        Err(StoreError::XmrWorkflowConflict)
    }

    /// Validates that one exact effect may be observed without resubmission.
    ///
    /// This read-only boundary accepts only `Started` or `Unknown`. A
    /// `Prepared` step has not consumed invocation authority, while a
    /// `Succeeded` step is already reconciled; neither may start an observer.
    ///
    /// # Errors
    ///
    /// Wrong role, branch scope, identity, step state, or corrupt storage
    /// fails closed. This method never changes durable workflow state.
    pub fn validate_observation_eligible(
        &self,
        identity: &XmrWorkflowIdentityV1,
        step: XmrWorkflowStep,
    ) -> Result<(), StoreError> {
        ensure_step_role(identity, step)?;
        self.revalidate_storage()?;
        ensure_scope(&self.connection, identity, step.scope())?;
        let snapshot =
            load_step(&self.connection, step)?.ok_or(StoreError::MissingXmrWorkflowStep)?;
        validate_step(identity, step, &snapshot)?;
        if !matches!(snapshot.state, StepState::Started | StepState::Unknown) {
            return Err(StoreError::XmrWorkflowConflict);
        }
        self.revalidate_storage()
    }

    /// Reports whether the exact step is still Prepared and therefore needs a
    /// non-sending preflight before its one invocation authority is consumed.
    ///
    /// # Errors
    ///
    /// Wrong role, branch scope, identity, missing step, or corrupt storage
    /// fails closed. This method never changes durable workflow state.
    pub fn requires_invocation_preflight(
        &self,
        identity: &XmrWorkflowIdentityV1,
        step: XmrWorkflowStep,
    ) -> Result<bool, StoreError> {
        ensure_step_role(identity, step)?;
        self.revalidate_storage()?;
        ensure_scope(&self.connection, identity, step.scope())?;
        let snapshot =
            load_step(&self.connection, step)?.ok_or(StoreError::MissingXmrWorkflowStep)?;
        validate_step(identity, step, &snapshot)?;
        self.revalidate_storage()?;
        Ok(snapshot.state == StepState::Prepared)
    }

    /// Reconciles Started or Unknown to Succeeded with exact durable evidence.
    ///
    /// # Errors
    ///
    /// Missing, invalid, or drifted evidence and any crossed state fail closed.
    pub fn reconcile_succeeded(
        &mut self,
        identity: &XmrWorkflowIdentityV1,
        step: XmrWorkflowStep,
        reconciliation: &XmrWorkflowReconciliationV2,
    ) -> Result<(), StoreError> {
        ensure_step_role(identity, step)?;
        validate_reconciliation(step, reconciliation)?;
        self.revalidate_storage()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_scope(&transaction, identity, step.scope())?;
        let snapshot = load_step(&transaction, step)?.ok_or(StoreError::MissingXmrWorkflowStep)?;
        validate_step(identity, step, &snapshot)?;
        match snapshot.state {
            StepState::Started | StepState::Unknown => {
                let expected_revision = i64::try_from(snapshot.revision)
                    .map_err(|_| StoreError::CorruptXmrWorkflowState)?;
                let next_revision: i64 = if snapshot.state == StepState::Started {
                    2
                } else {
                    3
                };
                let changed = transaction.execute(
                    "UPDATE xmr_workflow_steps
                     SET state = 'succeeded', revision = ?1,
                         effect_evidence_sha256 = ?2,
                         tool_plan_identity_sha256 = ?3,
                         reconciliation_source = ?4
                     WHERE step = ?5 AND state = ?6
                       AND attempt_count = 1 AND revision = ?7
                       AND effect_evidence_sha256 IS NULL
                       AND tool_plan_identity_sha256 IS NULL
                       AND reconciliation_source IS NULL",
                    params![
                        next_revision,
                        reconciliation.effect_evidence_sha256.as_slice(),
                        reconciliation.tool_plan_identity_sha256.as_slice(),
                        reconciliation.source.name(),
                        step.name(),
                        snapshot.state.name(),
                        expected_revision,
                    ],
                )?;
                if changed != 1 {
                    return Err(StoreError::XmrWorkflowConflict);
                }
            }
            StepState::Succeeded if snapshot.reconciliation.as_ref() == Some(reconciliation) => {}
            StepState::Prepared | StepState::Succeeded => {
                return Err(StoreError::XmrWorkflowConflict);
            }
        }
        transaction.commit()?;
        self.revalidate_storage()
    }

    /// Loads exact persisted reconciliation evidence for one step.
    ///
    /// # Errors
    ///
    /// Missing, crossed, or corrupt durable state fails closed.
    pub fn load_reconciliation(
        &self,
        identity: &XmrWorkflowIdentityV1,
        step: XmrWorkflowStep,
    ) -> Result<Option<XmrWorkflowReconciliationV2>, StoreError> {
        ensure_step_role(identity, step)?;
        self.revalidate_storage()?;
        ensure_scope(&self.connection, identity, step.scope())?;
        let snapshot =
            load_step(&self.connection, step)?.ok_or(StoreError::MissingXmrWorkflowStep)?;
        validate_step(identity, step, &snapshot)?;
        self.revalidate_storage()?;
        Ok(snapshot.reconciliation)
    }

    fn revalidate_storage(&self) -> Result<(), StoreError> {
        let file = open_checked(&self.path)?;
        if validate_file(&file)? != self.identity {
            return Err(StoreError::UnsafeDatabaseFile);
        }
        Ok(())
    }
}

fn initialize_schema(connection: &mut Connection) -> Result<(), StoreError> {
    let app: i64 = connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let objects: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    if app != 0 || version != 0 || objects != 0 {
        return Err(StoreError::ForeignXmrWorkflowSchema);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let schema = create_schema_v3();
    transaction.execute_batch(&schema)?;
    transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

fn validate_connection(connection: &Connection) -> Result<(), StoreError> {
    let app: i64 = connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(StoreError::FutureXmrWorkflowSchema);
    }
    if app != APPLICATION_ID || !(2..=SCHEMA_VERSION).contains(&version) {
        return Err(StoreError::ForeignXmrWorkflowSchema);
    }
    let names: String = connection.query_row(
        "SELECT group_concat(name, ',') FROM (
             SELECT name FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name
         )",
        [],
        |row| row.get(0),
    )?;
    let identity_sql: String = connection.query_row(
        "SELECT sql FROM sqlite_schema
         WHERE type = 'table' AND name = 'xmr_workflow_identity'",
        [],
        |row| row.get(0),
    )?;
    let steps_sql: String = connection.query_row(
        "SELECT sql FROM sqlite_schema
         WHERE type = 'table' AND name = 'xmr_workflow_steps'",
        [],
        |row| row.get(0),
    )?;
    let expected_schema = if version == 2 {
        CREATE_SCHEMA_V2.to_owned()
    } else {
        create_schema_v3()
    };
    let (expected_identity_sql, expected_steps_tail) = expected_schema
        .split_once("CREATE TABLE xmr_workflow_steps")
        .ok_or(StoreError::CorruptXmrWorkflowState)?;
    let expected_steps_sql = format!("CREATE TABLE xmr_workflow_steps{expected_steps_tail}");

    let unexpected: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema
         WHERE name NOT LIKE 'sqlite_%' AND type != 'table'",
        [],
        |row| row.get(0),
    )?;
    let integrity: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    let foreign_key_errors: i64 =
        connection.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if names != "xmr_workflow_identity,xmr_workflow_steps"
        || normalized_schema_sql(&identity_sql) != normalized_schema_sql(expected_identity_sql)
        || normalized_schema_sql(&steps_sql) != normalized_schema_sql(&expected_steps_sql)
        || unexpected != 0
        || integrity != "ok"
        || foreign_key_errors != 0
    {
        return Err(StoreError::CorruptXmrWorkflowState);
    }
    validate_identity_count(connection)?;
    validate_all_steps(connection)
}

fn normalized_schema_sql(value: &str) -> String {
    value
        .trim()
        .trim_end_matches(';')
        .split_ascii_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn configure(connection: &Connection) -> Result<(), StoreError> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "secure_delete", "ON")?;
    Ok(())
}

type LoadedIdentity = (XmrWorkflowIdentityV1, Option<XmrWorkflowBranch>, u64);

fn load_identity(connection: &Connection) -> Result<Option<LoadedIdentity>, StoreError> {
    validate_identity_count(connection)?;
    let raw = connection
        .query_row(
            "SELECT swap_id, local_role, run_id, agreement_commitment,
                    activation_commitment, authority_sha256,
                    selected_branch, revision
             FROM xmr_workflow_identity WHERE singleton_id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, Vec<u8>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()?;
    let Some((swap, role, run, agreement, activation, authority, branch, revision)) = raw else {
        return Ok(None);
    };
    let identity = XmrWorkflowIdentityV1::new(
        SwapId::new(swap).map_err(|_| StoreError::CorruptXmrWorkflowState)?,
        parse_role(&role)?,
        run.into_boxed_str(),
        digest(agreement)?,
        digest(activation)?,
        digest(authority)?,
    )
    .map_err(|_| StoreError::CorruptXmrWorkflowState)?;
    let branch = branch
        .as_deref()
        .map(XmrWorkflowBranch::parse)
        .transpose()?;
    let revision = u64::try_from(revision).map_err(|_| StoreError::CorruptXmrWorkflowState)?;
    if !matches!((branch, revision), (None, 0) | (Some(_), 1)) {
        return Err(StoreError::CorruptXmrWorkflowState);
    }
    Ok(Some((identity, branch, revision)))
}

fn load_step(
    connection: &Connection,
    expected: XmrWorkflowStep,
) -> Result<Option<StepSnapshot>, StoreError> {
    let raw = connection
        .query_row(
            "SELECT step, local_role, scope, state, attempt_count, revision,
                    effect_evidence_sha256, tool_plan_identity_sha256,
                    reconciliation_source
             FROM xmr_workflow_steps WHERE step = ?1",
            [expected.name()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<Vec<u8>>>(6)?,
                    row.get::<_, Option<Vec<u8>>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            },
        )
        .optional()?;
    let Some((step, role, scope, state, attempts, revision, effect, tool, source)) = raw else {
        return Ok(None);
    };
    let reconciliation = match (effect, tool, source) {
        (None, None, None) => None,
        (Some(effect), Some(tool), Some(source)) => Some(
            XmrWorkflowReconciliationV2::new(
                digest(effect)?,
                digest(tool)?,
                XmrWorkflowReconciliationSource::parse(&source)?,
            )
            .map_err(|_| StoreError::CorruptXmrWorkflowState)?,
        ),
        _ => return Err(StoreError::CorruptXmrWorkflowState),
    };
    let snapshot = StepSnapshot {
        step: XmrWorkflowStep::parse(&step)?,
        role: parse_role(&role)?,
        scope: XmrWorkflowStepScope::parse(&scope)?,
        state: StepState::parse(&state)?,
        attempts: u32::try_from(attempts).map_err(|_| StoreError::CorruptXmrWorkflowState)?,
        revision: u64::try_from(revision).map_err(|_| StoreError::CorruptXmrWorkflowState)?,
        reconciliation,
    };
    let shape = match snapshot.state {
        StepState::Prepared => {
            snapshot.attempts == 0 && snapshot.revision == 0 && snapshot.reconciliation.is_none()
        }
        StepState::Started => {
            snapshot.attempts == 1 && snapshot.revision == 1 && snapshot.reconciliation.is_none()
        }
        StepState::Unknown => {
            snapshot.attempts == 1 && snapshot.revision == 2 && snapshot.reconciliation.is_none()
        }
        StepState::Succeeded => {
            snapshot.attempts == 1
                && matches!(snapshot.revision, 2 | 3)
                && snapshot.reconciliation.is_some()
        }
    };
    if !shape {
        return Err(StoreError::CorruptXmrWorkflowState);
    }
    Ok(Some(snapshot))
}

fn ensure_scope(
    connection: &Connection,
    identity: &XmrWorkflowIdentityV1,
    expected: XmrWorkflowStepScope,
) -> Result<(), StoreError> {
    let (durable, branch, revision) =
        load_identity(connection)?.ok_or(StoreError::MissingXmrWorkflowIdentity)?;
    ensure_identity(identity, &durable)?;
    let valid = match expected.branch() {
        None => matches!((branch, revision), (None, 0) | (Some(_), 1)),
        Some(expected_branch) => branch == Some(expected_branch) && revision == 1,
    };
    if valid {
        Ok(())
    } else {
        Err(StoreError::XmrWorkflowConflict)
    }
}

fn ensure_common_prepared(connection: &Connection, role: Participant) -> Result<(), StoreError> {
    let expected: &[XmrWorkflowStep] = match role {
        Participant::Maker => &[XmrWorkflowStep::FundMonero],
        Participant::Taker => &[
            XmrWorkflowStep::InitializeLezTag13,
            XmrWorkflowStep::FundLezTag13,
        ],
    };
    for step in expected {
        let snapshot = load_step(connection, *step)?.ok_or(StoreError::MissingXmrWorkflowStep)?;
        if snapshot.role != role || snapshot.scope != XmrWorkflowStepScope::Common {
            return Err(StoreError::CorruptXmrWorkflowState);
        }
    }
    Ok(())
}

fn ensure_predecessor(connection: &Connection, step: XmrWorkflowStep) -> Result<(), StoreError> {
    let Some(predecessor) = step.predecessor() else {
        return Ok(());
    };
    let snapshot = load_step(connection, predecessor)?.ok_or(StoreError::MissingXmrWorkflowStep)?;
    if snapshot.state == StepState::Succeeded {
        Ok(())
    } else {
        Err(StoreError::XmrWorkflowConflict)
    }
}

fn validate_reconciliation(
    step: XmrWorkflowStep,
    reconciliation: &XmrWorkflowReconciliationV2,
) -> Result<(), StoreError> {
    if reconciliation.source != step.reconciliation_source()
        || reconciliation
            .effect_evidence_sha256
            .iter()
            .all(|byte| *byte == 0)
        || reconciliation
            .tool_plan_identity_sha256
            .iter()
            .all(|byte| *byte == 0)
    {
        Err(StoreError::InvalidXmrWorkflowReconciliation)
    } else {
        Ok(())
    }
}

fn ensure_identity(
    requested: &XmrWorkflowIdentityV1,
    durable: &XmrWorkflowIdentityV1,
) -> Result<(), StoreError> {
    validate_identity(requested)?;
    if requested == durable {
        Ok(())
    } else {
        Err(StoreError::XmrWorkflowConflict)
    }
}

fn ensure_step_role(
    identity: &XmrWorkflowIdentityV1,
    step: XmrWorkflowStep,
) -> Result<(), StoreError> {
    validate_identity(identity)?;
    if identity.local_role == step.role() {
        Ok(())
    } else {
        Err(StoreError::XmrWorkflowConflict)
    }
}

fn validate_step(
    identity: &XmrWorkflowIdentityV1,
    expected: XmrWorkflowStep,
    snapshot: &StepSnapshot,
) -> Result<(), StoreError> {
    if snapshot.step == expected
        && snapshot.role == identity.local_role
        && snapshot.role == expected.role()
        && snapshot.scope == expected.scope()
        && snapshot
            .reconciliation
            .as_ref()
            .is_none_or(|value| value.source == expected.reconciliation_source())
    {
        Ok(())
    } else {
        Err(StoreError::CorruptXmrWorkflowState)
    }
}

fn validate_identity(identity: &XmrWorkflowIdentityV1) -> Result<(), StoreError> {
    let swap = identity.swap_id.as_str();
    if swap.len() != 64
        || !swap
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || identity.run_id.is_empty()
        || identity.run_id.len() > MAX_RUN_ID_BYTES
        || !identity.run_id.bytes().all(|byte| byte.is_ascii_graphic())
        || identity.agreement_commitment.iter().all(|byte| *byte == 0)
        || identity.activation_commitment.iter().all(|byte| *byte == 0)
        || identity.authority_sha256.iter().all(|byte| *byte == 0)
    {
        return Err(StoreError::InvalidXmrWorkflowIdentity);
    }
    Ok(())
}

fn validate_identity_count(connection: &Connection) -> Result<(), StoreError> {
    let count: i64 =
        connection.query_row("SELECT COUNT(*) FROM xmr_workflow_identity", [], |row| {
            row.get(0)
        })?;
    if (0..=1).contains(&count) {
        Ok(())
    } else {
        Err(StoreError::CorruptXmrWorkflowState)
    }
}

fn validate_all_steps(connection: &Connection) -> Result<(), StoreError> {
    let durable = load_identity(connection)?.map(|(identity, _, _)| identity);
    let mut statement = connection.prepare("SELECT step FROM xmr_workflow_steps ORDER BY step")?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if !names.is_empty() && durable.is_none() {
        return Err(StoreError::CorruptXmrWorkflowState);
    }
    for name in names {
        let step = XmrWorkflowStep::parse(&name)?;
        let snapshot = load_step(connection, step)?.ok_or(StoreError::CorruptXmrWorkflowState)?;
        validate_step(
            durable
                .as_ref()
                .ok_or(StoreError::CorruptXmrWorkflowState)?,
            step,
            &snapshot,
        )?;
    }
    Ok(())
}

fn parse_role(value: &str) -> Result<Participant, StoreError> {
    match value {
        "maker" => Ok(Participant::Maker),
        "taker" => Ok(Participant::Taker),
        _ => Err(StoreError::CorruptXmrWorkflowState),
    }
}

fn digest(value: Vec<u8>) -> Result<[u8; 32], StoreError> {
    value
        .try_into()
        .map_err(|_| StoreError::CorruptXmrWorkflowState)
}

fn validate_path(path: &Path) -> Result<(), StoreError> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(StoreError::DatabaseFileUnavailable);
    }
    let parent = path.parent().ok_or(StoreError::DatabaseFileUnavailable)?;
    let metadata = fs::symlink_metadata(parent).map_err(|_| StoreError::DatabaseFileUnavailable)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(StoreError::UnsafeDatabaseFile);
    }
    Ok(())
}

fn open_checked(path: &Path) -> Result<File, StoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| StoreError::DatabaseFileUnavailable)?;
    if !metadata.file_type().is_file() {
        return Err(StoreError::UnsafeDatabaseFile);
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| StoreError::DatabaseFileUnavailable)
}

fn classify_existing(path: &Path) -> Result<(), StoreError> {
    let file = open_checked(path).map_err(|_| StoreError::UnsafeDatabaseFile)?;
    validate_file(&file)?;
    Ok(())
}

fn validate_file(file: &File) -> Result<FileIdentity, StoreError> {
    let metadata = file
        .metadata()
        .map_err(|_| StoreError::UnsafeDatabaseFile)?;
    if !metadata.file_type().is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(StoreError::UnsafeDatabaseFile);
    }
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn sync_parent(path: &Path) -> Result<(), StoreError> {
    File::open(path.parent().ok_or(StoreError::DatabaseFileUnavailable)?)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| StoreError::DatabaseFileUnavailable)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn exact_schema_v2_remains_readable_and_unmigrated() {
        let root = tempdir().expect("isolated schema-v2 compatibility root");
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.path().join("workflow-v2.sqlite3");
        drop(
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .unwrap(),
        );
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(CREATE_SCHEMA_V2).unwrap();
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        connection.pragma_update(None, "user_version", 2).unwrap();
        drop(connection);

        let identity = XmrWorkflowIdentityV1::new(
            SwapId::new("a7".repeat(32)).unwrap(),
            Participant::Maker,
            "schema-v2-compatibility".into(),
            [0xa8; 32],
            [0xa9; 32],
            [0xaa; 32],
        )
        .unwrap();
        let mut journal = SqliteXmrWorkflowJournal::open_existing(&path).unwrap();
        journal.initialize(&identity).unwrap();
        journal
            .prepare_step(&identity, XmrWorkflowStep::FundMonero)
            .unwrap();
        journal
            .select_branch(&identity, XmrWorkflowBranch::Refund)
            .unwrap();
        assert!(
            journal
                .select_branch(&identity, XmrWorkflowBranch::Punish)
                .is_err(),
            "schema-v2 cannot persist the additive schema-v3 branch"
        );
        drop(journal);

        let connection = Connection::open(&path).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2, "opening schema-v2 must not migrate it");
    }
}
