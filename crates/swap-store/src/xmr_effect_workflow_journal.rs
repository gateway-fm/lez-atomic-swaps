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
const SCHEMA_VERSION: i64 = 1;
const MAX_RUN_ID_BYTES: usize = 128;

const CREATE_SCHEMA: &str = "
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
    step TEXT PRIMARY KEY,
    singleton_id INTEGER NOT NULL CHECK (singleton_id = 1),
    local_role TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
    branch TEXT NOT NULL CHECK (branch IN ('claim', 'refund')),
    state TEXT NOT NULL CHECK (
        state IN ('prepared', 'started', 'succeeded', 'unknown')
    ),
    attempt_count INTEGER NOT NULL CHECK (attempt_count IN (0, 1)),
    revision INTEGER NOT NULL CHECK (
        (state = 'prepared' AND attempt_count = 0 AND revision = 0)
        OR (state = 'started' AND attempt_count = 1 AND revision = 1)
        OR (state IN ('succeeded', 'unknown') AND attempt_count = 1 AND revision = 2)
    ),
    FOREIGN KEY (singleton_id) REFERENCES xmr_workflow_identity(singleton_id)
) STRICT, WITHOUT ROWID;
";

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
}

impl XmrWorkflowBranch {
    const fn name(self) -> &'static str {
        match self {
            Self::Claim => "claim",
            Self::Refund => "refund",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "claim" => Ok(Self::Claim),
            "refund" => Ok(Self::Refund),
            _ => Err(StoreError::CorruptXmrWorkflowState),
        }
    }
}

/// Fixed role-specific application step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum XmrWorkflowStep {
    /// Maker invokes sealed tag-15 LEZ claim authority.
    SubmitLezClaimTag15,
    /// Taker invokes sealed tag-16 LEZ refund authority.
    SubmitLezRefundTag16,
}

impl XmrWorkflowStep {
    const fn name(self) -> &'static str {
        match self {
            Self::SubmitLezClaimTag15 => "submit_lez_claim_tag15",
            Self::SubmitLezRefundTag16 => "submit_lez_refund_tag16",
        }
    }

    const fn role(self) -> Participant {
        match self {
            Self::SubmitLezClaimTag15 => Participant::Maker,
            Self::SubmitLezRefundTag16 => Participant::Taker,
        }
    }

    const fn branch(self) -> XmrWorkflowBranch {
        match self {
            Self::SubmitLezClaimTag15 => XmrWorkflowBranch::Claim,
            Self::SubmitLezRefundTag16 => XmrWorkflowBranch::Refund,
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "submit_lez_claim_tag15" => Ok(Self::SubmitLezClaimTag15),
            "submit_lez_refund_tag16" => Ok(Self::SubmitLezRefundTag16),
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

    const fn terminal_name(self) -> Option<&'static str> {
        match self {
            Self::Succeeded => Some("succeeded"),
            Self::Unknown => Some("unknown"),
            Self::Prepared | Self::Started => None,
        }
    }
}

struct StepSnapshot {
    step: XmrWorkflowStep,
    role: Participant,
    branch: XmrWorkflowBranch,
    state: StepState,
    attempts: u32,
    revision: u64,
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

    /// Opens an existing exact schema-v1 journal without creating or migrating it.
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

    /// Selects one branch with a durable compare-and-set.
    ///
    /// # Errors
    ///
    /// The losing branch or identity drift fails closed.
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

    /// Prepares a fixed role/branch step without invoking it.
    ///
    /// # Errors
    ///
    /// Wrong role, branch, identity, or durable state fails closed.
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
        ensure_branch(&transaction, identity, step.branch())?;
        if let Some(snapshot) = load_step(&transaction, step)? {
            validate_step(identity, step, &snapshot)?;
        } else {
            transaction.execute(
                "INSERT INTO xmr_workflow_steps (
                     step, singleton_id, local_role, branch,
                     state, attempt_count, revision
                 ) VALUES (?1, 1, ?2, ?3, 'prepared', 0, 0)",
                params![
                    step.name(),
                    participant_name(identity.local_role),
                    step.branch().name(),
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
        ensure_branch(&transaction, identity, step.branch())?;
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
        self.finish(identity, step, StepState::Unknown)
    }

    /// Marks exact external reconciliation complete.
    ///
    /// # Errors
    ///
    /// Only Started may advance; exact Succeeded replay is idempotent.
    pub fn mark_succeeded(
        &mut self,
        identity: &XmrWorkflowIdentityV1,
        step: XmrWorkflowStep,
    ) -> Result<(), StoreError> {
        self.finish(identity, step, StepState::Succeeded)
    }

    fn finish(
        &mut self,
        identity: &XmrWorkflowIdentityV1,
        step: XmrWorkflowStep,
        target: StepState,
    ) -> Result<(), StoreError> {
        ensure_step_role(identity, step)?;
        self.revalidate_storage()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_branch(&transaction, identity, step.branch())?;
        let snapshot = load_step(&transaction, step)?.ok_or(StoreError::MissingXmrWorkflowStep)?;
        validate_step(identity, step, &snapshot)?;
        if snapshot.state == StepState::Started {
            let target_name = target
                .terminal_name()
                .ok_or(StoreError::XmrWorkflowConflict)?;
            let changed = transaction.execute(
                "UPDATE xmr_workflow_steps
                 SET state = ?1, revision = 2
                 WHERE step = ?2 AND state = 'started'
                   AND attempt_count = 1 AND revision = 1",
                params![target_name, step.name()],
            )?;
            if changed != 1 {
                return Err(StoreError::XmrWorkflowConflict);
            }
        } else if snapshot.state != target {
            return Err(StoreError::XmrWorkflowConflict);
        }
        transaction.commit()?;
        self.revalidate_storage()
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
    transaction.execute_batch(CREATE_SCHEMA)?;
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
    if app != APPLICATION_ID || version != SCHEMA_VERSION {
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
    let (expected_identity_sql, expected_steps_tail) = CREATE_SCHEMA
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
    validate_identity_count(connection)
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
            "SELECT step, local_role, branch, state, attempt_count, revision
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
                ))
            },
        )
        .optional()?;
    let Some((step, role, branch, state, attempts, revision)) = raw else {
        return Ok(None);
    };
    let snapshot = StepSnapshot {
        step: XmrWorkflowStep::parse(&step)?,
        role: parse_role(&role)?,
        branch: XmrWorkflowBranch::parse(&branch)?,
        state: StepState::parse(&state)?,
        attempts: u32::try_from(attempts).map_err(|_| StoreError::CorruptXmrWorkflowState)?,
        revision: u64::try_from(revision).map_err(|_| StoreError::CorruptXmrWorkflowState)?,
    };
    let shape = match snapshot.state {
        StepState::Prepared => snapshot.attempts == 0 && snapshot.revision == 0,
        StepState::Started => snapshot.attempts == 1 && snapshot.revision == 1,
        StepState::Succeeded | StepState::Unknown => {
            snapshot.attempts == 1 && snapshot.revision == 2
        }
    };
    if !shape {
        return Err(StoreError::CorruptXmrWorkflowState);
    }
    Ok(Some(snapshot))
}

fn ensure_branch(
    connection: &Connection,
    identity: &XmrWorkflowIdentityV1,
    expected: XmrWorkflowBranch,
) -> Result<(), StoreError> {
    let (durable, branch, revision) =
        load_identity(connection)?.ok_or(StoreError::MissingXmrWorkflowIdentity)?;
    ensure_identity(identity, &durable)?;
    if branch != Some(expected) || revision != 1 {
        return Err(StoreError::XmrWorkflowConflict);
    }
    Ok(())
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
        && snapshot.branch == expected.branch()
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
