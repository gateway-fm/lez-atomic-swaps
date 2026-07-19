// The typed issuer/publisher boundary is deliberately sealed; compile its core in
// production while tests exercise it until concrete capabilities replace ReleasePlan.
#![cfg_attr(not(test), allow(dead_code))]

mod publisher;

use super::{
    ProtectedPublicationIntent, ProtectionError, PublicationProtectionKey, ReleasePlan,
    derive_activation_id, exact_binding_bytes, hash, immutable_release_context_bytes,
    observation_authenticator, release_state_authenticator, semantic_intent_authenticator,
    validate_ciphertext_length, validate_key_id, validate_plaintext_length,
    verify_observation_authenticator, verify_release_state_authenticator,
    verify_semantic_intent_authenticator,
};
use rusqlite::{
    Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use rustix::{
    fs::{CWD, Mode, OFlags, ResolveFlags, openat, openat2},
    io::Errno,
    process::geteuid,
};
use std::{
    ffi::{OsStr, OsString},
    fmt,
    fs::{self, File},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};
use thiserror::Error;

const DATABASE_SCHEMA_VERSION: i64 = 3;
const DATABASE_APPLICATION_ID: i64 = 1_280_855_378;
const MAX_OBSERVATION_BYTES: usize = 65_536;
const MAX_TARGET_BYTES: usize = 65_536;
const PREPARED: &str = "prepared";
const STARTED: &str = "publication_started";
const ADMITTED: &str = "admitted";
const AMBIGUOUS: &str = "ambiguous";
const SUPPRESSED: &str = "suppressed";

const CREATE_TABLE_SQL: &str = "CREATE TABLE release (
    activation BLOB PRIMARY KEY NOT NULL
        CHECK(length(activation) = 32 AND activation != zeroblob(32)),
    swap_id BLOB NOT NULL CHECK(length(swap_id) = 32),
    run_id BLOB NOT NULL CHECK(length(run_id) = 32),
    lez_commitment BLOB NOT NULL
        CHECK(length(lez_commitment) = 32 AND lez_commitment != zeroblob(32)),
    topology_commitment BLOB NOT NULL
        CHECK(length(topology_commitment) = 32 AND topology_commitment != zeroblob(32)),
    resource_id BLOB NOT NULL UNIQUE
        CHECK(length(resource_id) = 32 AND resource_id != zeroblob(32)),
    observation BLOB NOT NULL CHECK(length(observation) BETWEEN 1 AND 65536),
    observation_authenticator BLOB NOT NULL CHECK(length(observation_authenticator) = 32),
    claim_partial_commitment BLOB NOT NULL
        CHECK(length(claim_partial_commitment) = 32
              AND claim_partial_commitment != zeroblob(32)),
    target BLOB NOT NULL CHECK(length(target) BETWEEN 1 AND 65536),
    publication_id BLOB NOT NULL
        CHECK(length(publication_id) = 32 AND publication_id != zeroblob(32)),
    window_start INTEGER NOT NULL CHECK(window_start BETWEEN 0 AND 9223372036854775807),
    window_end INTEGER NOT NULL
        CHECK(window_end > window_start AND window_end <= 9223372036854775807),
    binding BLOB NOT NULL UNIQUE CHECK(length(binding) = 32),
    semantic_authenticator BLOB NOT NULL CHECK(length(semantic_authenticator) = 32),
    key_id TEXT NOT NULL CHECK(length(key_id) BETWEEN 1 AND 128),
    nonce BLOB NOT NULL CHECK(length(nonce) = 24),
    ciphertext BLOB NOT NULL CHECK(length(ciphertext) BETWEEN 17 AND 2000016),
    fingerprint BLOB NOT NULL CHECK(length(fingerprint) = 32),
    state_authenticator BLOB NOT NULL CHECK(length(state_authenticator) = 32),
    state TEXT NOT NULL
        CHECK(state IN ('prepared', 'publication_started', 'admitted', 'ambiguous', 'suppressed')),
    revision INTEGER NOT NULL CHECK(
        (state = 'prepared' AND revision = 0) OR
        (state = 'publication_started' AND revision = 1) OR
        (state = 'admitted' AND revision = 2) OR
        (state = 'ambiguous' AND revision = 2) OR
        (state = 'suppressed' AND revision = 2)
    ),
    UNIQUE(swap_id, run_id)
) STRICT";

/// Half-open finalized LEZ consensus-time window `[start, end)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReleaseWindow {
    pub(super) start: u64,
    pub(super) end: u64,
}

impl ReleaseWindow {
    /// Creates a non-empty half-open window representable by `SQLite`.
    pub fn new(start: u64, end: u64) -> Result<Self, ReleaseError> {
        if start >= end || end > i64::MAX as u64 {
            return Err(ReleaseError::InvalidBinding);
        }
        Ok(Self { start, end })
    }

    /// Returns the inclusive lower bound.
    pub const fn start(self) -> u64 {
        self.start
    }

    /// Returns the exclusive upper bound.
    pub const fn end(self) -> u64 {
        self.end
    }
}

/// Validated durable journal state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleaseState {
    /// Exact release evidence and encrypted intent are durable and unsent.
    Prepared,
    /// One process won the send CAS; every restart must observe only.
    PublicationStarted,
    /// The exact node accepted or already knew the transaction; not chain finality.
    Admitted,
    /// Publication outcome is uncertain and must remain observe-only.
    Ambiguous,
    /// The post-CAS clock gate proved that no node call was made.
    Suppressed,
}

impl ReleaseState {
    const fn record(self) -> (&'static str, u8) {
        match self {
            Self::Prepared => (PREPARED, 0),
            Self::PublicationStarted => (STARTED, 1),
            Self::Admitted => (ADMITTED, 2),
            Self::Ambiguous => (AMBIGUOUS, 2),
            Self::Suppressed => (SUPPRESSED, 2),
        }
    }

    fn from_record(state: &str, revision: i64) -> Result<Self, ReleaseError> {
        match (state, revision) {
            (PREPARED, 0) => Ok(Self::Prepared),
            (STARTED, 1) => Ok(Self::PublicationStarted),
            (ADMITTED, 2) => Ok(Self::Admitted),
            (AMBIGUOUS, 2) => Ok(Self::Ambiguous),
            (SUPPRESSED, 2) => Ok(Self::Suppressed),
            _ => Err(ReleaseError::CorruptRecord),
        }
    }
}

/// Authenticated restart snapshot of one exact release record.
#[derive(Eq, PartialEq)]
pub struct ReleaseSnapshot {
    activation: [u8; 32],
    run_id: [u8; 32],
    resource_id: [u8; 32],
    observation: Vec<u8>,
    observation_authenticator: [u8; 32],
    binding: [u8; 32],
    semantic_authenticator: [u8; 32],
    target: Vec<u8>,
    window: ReleaseWindow,
    publication_id: [u8; 32],
    immutable_context: Vec<u8>,
    intent: ProtectedPublicationIntent,
    state_authenticator: [u8; 32],
    state: ReleaseState,
}

impl fmt::Debug for ReleaseSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReleaseSnapshot")
            .field("identity", &"[REDACTED]")
            .field("window", &self.window)
            .field("intent", &self.intent)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl ReleaseSnapshot {
    /// Returns the deterministic activation identifier.
    pub const fn activation(&self) -> [u8; 32] {
        self.activation
    }

    /// Returns the domain-separated digest of the validated Stage-B run ID.
    pub const fn run_id(&self) -> [u8; 32] {
        self.run_id
    }

    /// Returns the stable immutable Monero resource identifier.
    pub const fn resource_id(&self) -> [u8; 32] {
        self.resource_id
    }

    /// Returns the authenticated publication target bytes.
    pub fn target(&self) -> &[u8] {
        &self.target
    }

    /// Returns the authenticated release window.
    pub const fn window(&self) -> ReleaseWindow {
        self.window
    }

    /// Returns the validated durable state.
    pub const fn state(&self) -> ReleaseState {
        self.state
    }

    /// Returns the protected publication envelope retained across restart.
    pub const fn protected_intent(&self) -> &ProtectedPublicationIntent {
        &self.intent
    }

    /// Returns the official-decoder identity of the exact authorization.
    pub const fn publication_id(&self) -> [u8; 32] {
        self.publication_id
    }
}

/// Unique crash-safe permission to submit one exact publication.
#[derive(Eq, PartialEq)]
struct PublicationAttempt {
    activation: [u8; 32],
    run_id: [u8; 32],
    binding: [u8; 32],
    target: Vec<u8>,
    publication_id: [u8; 32],
    window: ReleaseWindow,
    immutable_context: Vec<u8>,
    intent: ProtectedPublicationIntent,
}

impl fmt::Debug for PublicationAttempt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublicationAttempt")
            .field("identity", &"[REDACTED]")
            .field("window", &self.window)
            .field("intent", &self.intent)
            .finish_non_exhaustive()
    }
}

impl PublicationAttempt {
    /// Authenticates and opens the exact intent while retaining the outcome token.
    fn opened_intent(
        &self,
        key: &PublicationProtectionKey,
    ) -> Result<zeroize::Zeroizing<Vec<u8>>, ProtectionError> {
        self.intent.decrypt(key, &self.immutable_context)
    }
}

/// Whether this process owns the unique send attempt.
#[derive(Debug, Eq, PartialEq)]
enum PublicationDecision {
    /// This process won the prepared-to-started compare-and-swap.
    Send(Box<PublicationAttempt>),
    /// A send was already started or became ambiguous; observe only.
    ObserveOnly,
}

/// Durable authority failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum ReleaseError {
    /// The requested activation exists with another exact run or binding.
    #[error("activation binding mismatch")]
    BindingMismatch,
    /// One finalized Monero observation is already bound elsewhere.
    #[error("observation replay")]
    ObservationReplay,
    /// No exact activation/run record exists.
    #[error("release record absent")]
    Missing,
    /// A caller supplied an empty, overlong, unordered, or unrepresentable binding.
    #[error("release binding is invalid")]
    InvalidBinding,
    /// The database path is not a regular durable file path.
    #[error("release database path is invalid")]
    InvalidPath,
    /// A database parent is symlinked, replaced, or not owner-private.
    #[error("release database directory is insecure")]
    InsecureDirectory,
    /// The database is not one owner-only, regular, non-aliased inode.
    #[error("release database file is unsafe")]
    UnsafeDatabaseFile,
    /// A newer schema must not be reinterpreted.
    #[error("release database uses a future schema")]
    FutureSchema,
    /// Another application or unexpected schema owns this database.
    #[error("release database schema is foreign")]
    ForeignSchema,
    /// A row, schema, or `SQLite` integrity invariant failed closed.
    #[error("durable release record is corrupt")]
    CorruptRecord,
    /// The supplied protection key cannot authenticate this record.
    #[error("durable release record authentication failed")]
    Authentication,
    /// A redacted durable-store operation failed.
    #[error("durable release store failure")]
    Store,
}

/// SQLite-backed at-most-once release journal.
pub struct ReleaseStore {
    directory: SecureDirectory,
    database_name: OsString,
    database_identity: DatabaseIdentity,
    connection: Mutex<Connection>,
}

impl fmt::Debug for ReleaseStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReleaseStore")
            .field("path", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl ReleaseStore {
    /// Opens or initializes one owner-private, durable release journal.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ReleaseError> {
        let (path, parent, database_name) = validate_database_path(path.as_ref())?;
        let directory = SecureDirectory::open(&parent)?;
        let (database_identity, creation_guard) =
            prepare_database_file(&directory, &database_name)?;
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let mut connection =
            Connection::open_with_flags(&path, flags).map_err(|_| ReleaseError::Store)?;
        verify_database_file(&directory, &database_name, database_identity)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|_| ReleaseError::Store)?;
        migrate(&mut connection)?;
        configure_connection(&connection)?;
        validate_connection(&connection)?;
        drop(creation_guard);
        verify_database_file(&directory, &database_name, database_identity)?;
        Ok(Self {
            directory,
            database_name,
            database_identity,
            connection: Mutex::new(connection),
        })
    }

    /// Durably prepares one semantic release plan.
    ///
    /// The concrete XMR issuer is the sole production-compiled caller. Keeping
    /// this raw plan crate-private prevents public construction from bypassing
    /// Stage-B, finalized-Fund, topology, output, and deadline checks.
    /// The first randomized encryption occurs only after an immediate transaction
    /// proves that no semantic record already exists.
    #[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
    pub(crate) fn prepare(
        &self,
        plan: ReleasePlan,
        key: &PublicationProtectionKey,
    ) -> Result<ReleaseSnapshot, ReleaseError> {
        validate_plan(&plan)?;
        self.revalidate_storage()?;
        let immutable_context = plan.immutable_context();
        let semantic_authenticator =
            semantic_intent_authenticator(key, &immutable_context, &plan.publication)
                .map_err(|_| ReleaseError::Authentication)?;
        let new_observation_authenticator =
            observation_authenticator(key, &immutable_context, &plan.observation)
                .map_err(|_| ReleaseError::Authentication)?;
        let mut connection = self.lock_connection()?;
        validate_connection(&connection)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| ReleaseError::Store)?;
        validate_connection(&transaction)?;

        if let Some(existing) = select_record(&transaction, &plan.activation)? {
            let mut snapshot = existing.validate()?;
            authenticate_snapshot(&snapshot, key)?;
            if !record_matches_plan(&snapshot, &plan, &immutable_context)
                || !verify_semantic_intent_authenticator(
                    key,
                    &immutable_context,
                    &plan.publication,
                    &snapshot.semantic_authenticator,
                )
                .map_err(|_| ReleaseError::Authentication)?
            {
                return Err(ReleaseError::BindingMismatch);
            }
            if snapshot.observation != plan.observation {
                let changed = transaction
                    .execute(
                        "UPDATE release
                         SET observation=?1, observation_authenticator=?2
                         WHERE activation=?3 AND binding=?4",
                        params![
                            plan.observation.as_slice(),
                            new_observation_authenticator.as_slice(),
                            plan.activation.as_slice(),
                            snapshot.binding.as_slice(),
                        ],
                    )
                    .map_err(|_| ReleaseError::Store)?;
                if changed != 1 {
                    return Err(ReleaseError::CorruptRecord);
                }
                snapshot.observation = plan.observation;
                snapshot.observation_authenticator = new_observation_authenticator;
            }
            transaction.commit().map_err(|_| ReleaseError::Store)?;
            validate_connection(&connection)?;
            drop(connection);
            self.revalidate_storage()?;
            return Ok(snapshot);
        }
        if resource_exists(&transaction, &plan.resource_id)? {
            return Err(ReleaseError::ObservationReplay);
        }
        if swap_run_exists(&transaction, &plan.swap_id, &plan.run_id)? {
            return Err(ReleaseError::BindingMismatch);
        }

        let intent =
            ProtectedPublicationIntent::encrypt(&plan.publication, key, &immutable_context)
                .map_err(|_| ReleaseError::Authentication)?;
        let binding = hash(&exact_binding_bytes(&immutable_context, &intent));
        let state_authenticator =
            release_state_authenticator(key, &immutable_context, &binding, PREPARED, 0)
                .map_err(|_| ReleaseError::Authentication)?;
        let inserted = transaction
            .execute(
                "INSERT INTO release(
                     activation,swap_id,run_id,lez_commitment,topology_commitment,
                     resource_id,observation,observation_authenticator,
                     claim_partial_commitment,target,publication_id,window_start,window_end,
                     binding,semantic_authenticator,key_id,nonce,ciphertext,
                     fingerprint,state_authenticator,state,revision
                 ) VALUES(
                     ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,
                     ?16,?17,?18,?19,?20,?21,0
                 )",
                params![
                    plan.activation.as_slice(),
                    plan.swap_id.as_slice(),
                    plan.run_id.as_slice(),
                    plan.lez_commitment.as_slice(),
                    plan.topology_commitment.as_slice(),
                    plan.resource_id.as_slice(),
                    plan.observation.as_slice(),
                    new_observation_authenticator.as_slice(),
                    plan.claim_partial_commitment.as_slice(),
                    plan.target.as_slice(),
                    plan.publication_id.as_slice(),
                    i64::try_from(plan.window_start).map_err(|_| ReleaseError::InvalidBinding)?,
                    i64::try_from(plan.window_end).map_err(|_| ReleaseError::InvalidBinding)?,
                    binding.as_slice(),
                    semantic_authenticator.as_slice(),
                    intent.key_id.as_ref(),
                    intent.nonce.as_slice(),
                    intent.ciphertext.as_slice(),
                    intent.fingerprint.as_slice(),
                    state_authenticator.as_slice(),
                    PREPARED,
                ],
            )
            .map_err(|error| {
                if error.sqlite_error_code() == Some(ErrorCode::ConstraintViolation) {
                    ReleaseError::BindingMismatch
                } else {
                    ReleaseError::Store
                }
            })?;
        if inserted != 1 {
            return Err(ReleaseError::CorruptRecord);
        }
        let snapshot = snapshot_from_plan(
            plan,
            immutable_context,
            intent,
            binding,
            semantic_authenticator,
            new_observation_authenticator,
            state_authenticator,
        );
        transaction.commit().map_err(|_| ReleaseError::Store)?;
        validate_connection(&connection)?;
        drop(connection);
        self.revalidate_storage()?;
        Ok(snapshot)
    }

    /// Reopens one activation/run record and authenticates its immutable intent.
    ///
    /// The returned snapshot contains its target, release window, protected
    /// publication, and current observe/send state; no in-memory prepare token
    /// from a previous process is required.
    pub fn load_by_activation_run(
        &self,
        activation: [u8; 32],
        run_id: [u8; 32],
        key: &PublicationProtectionKey,
    ) -> Result<ReleaseSnapshot, ReleaseError> {
        self.revalidate_storage()?;
        let connection = self.lock_connection()?;
        validate_connection(&connection)?;
        let record = select_record(&connection, &activation)?.ok_or(ReleaseError::Missing)?;
        let snapshot = record.validate()?;
        if snapshot.run_id != run_id {
            return Err(ReleaseError::BindingMismatch);
        }
        authenticate_snapshot(&snapshot, key)?;
        drop(connection);
        self.revalidate_storage()?;
        Ok(snapshot)
    }

    /// Atomically grants one send attempt; every later caller observes only.
    fn begin_publication(
        &self,
        snapshot: ReleaseSnapshot,
        key: &PublicationProtectionKey,
    ) -> Result<PublicationDecision, ReleaseError> {
        self.revalidate_storage()?;
        let mut connection = self.lock_connection()?;
        validate_connection(&connection)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| ReleaseError::Store)?;
        validate_connection(&transaction)?;
        let current = select_record(&transaction, &snapshot.activation)?
            .ok_or(ReleaseError::Missing)?
            .validate()?;
        authenticate_snapshot(&current, key)?;
        if !same_snapshot_binding(&current, &snapshot) {
            return Err(ReleaseError::BindingMismatch);
        }
        if current.state != ReleaseState::Prepared {
            transaction.commit().map_err(|_| ReleaseError::Store)?;
            return Ok(PublicationDecision::ObserveOnly);
        }
        let started_authenticator = release_state_authenticator(
            key,
            &current.immutable_context,
            &current.binding,
            STARTED,
            1,
        )
        .map_err(|_| ReleaseError::Authentication)?;
        let changed = transaction
            .execute(
                "UPDATE release SET state=?1, revision=1, state_authenticator=?2
                 WHERE activation=?3 AND run_id=?4 AND binding=?5
                   AND state=?6 AND revision=0",
                params![
                    STARTED,
                    started_authenticator.as_slice(),
                    snapshot.activation.as_slice(),
                    snapshot.run_id.as_slice(),
                    snapshot.binding.as_slice(),
                    PREPARED,
                ],
            )
            .map_err(|_| ReleaseError::Store)?;
        if changed != 1 {
            return Err(ReleaseError::CorruptRecord);
        }
        transaction.commit().map_err(|_| ReleaseError::Store)?;
        validate_connection(&connection)?;
        drop(connection);
        self.revalidate_storage()?;
        Ok(PublicationDecision::Send(Box::new(PublicationAttempt {
            activation: snapshot.activation,
            run_id: snapshot.run_id,
            binding: snapshot.binding,
            target: snapshot.target,
            publication_id: snapshot.publication_id,
            window: snapshot.window,
            immutable_context: snapshot.immutable_context,
            intent: snapshot.intent,
        })))
    }

    /// Persists exact node admission as terminal but not finalized-chain evidence.
    #[allow(clippy::needless_pass_by_value)]
    fn mark_admitted(
        &self,
        attempt: PublicationAttempt,
        key: &PublicationProtectionKey,
    ) -> Result<(), ReleaseError> {
        self.finish_publication(attempt, key, ReleaseState::Admitted)
    }

    /// Persists an uncertain publication result as permanently observe-only.
    ///
    /// There is deliberately no caller-enum finalization API. A future method
    /// must consume exact finalized-chain evidence; definitive absence/retry
    /// authority is likewise intentionally absent.
    #[allow(clippy::needless_pass_by_value)]
    fn mark_ambiguous(
        &self,
        attempt: PublicationAttempt,
        key: &PublicationProtectionKey,
    ) -> Result<(), ReleaseError> {
        self.finish_publication(attempt, key, ReleaseState::Ambiguous)
    }

    /// Persists a post-CAS known-no-send decision as terminal observe-only.
    #[allow(clippy::needless_pass_by_value)]
    fn mark_suppressed(
        &self,
        attempt: PublicationAttempt,
        key: &PublicationProtectionKey,
    ) -> Result<(), ReleaseError> {
        self.finish_publication(attempt, key, ReleaseState::Suppressed)
    }

    #[allow(clippy::needless_pass_by_value)]
    fn finish_publication(
        &self,
        attempt: PublicationAttempt,
        key: &PublicationProtectionKey,
        terminal: ReleaseState,
    ) -> Result<(), ReleaseError> {
        if !matches!(
            terminal,
            ReleaseState::Admitted | ReleaseState::Ambiguous | ReleaseState::Suppressed
        ) {
            return Err(ReleaseError::InvalidBinding);
        }
        let (terminal_state, terminal_revision) = terminal.record();
        self.revalidate_storage()?;
        let mut connection = self.lock_connection()?;
        validate_connection(&connection)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| ReleaseError::Store)?;
        let current = select_record(&transaction, &attempt.activation)?
            .ok_or(ReleaseError::Missing)?
            .validate()?;
        authenticate_snapshot(&current, key)?;
        if current.run_id != attempt.run_id
            || current.binding != attempt.binding
            || current.immutable_context != attempt.immutable_context
            || current.intent != attempt.intent
            || current.state != ReleaseState::PublicationStarted
        {
            return Err(ReleaseError::BindingMismatch);
        }
        let terminal_authenticator = release_state_authenticator(
            key,
            &current.immutable_context,
            &current.binding,
            terminal_state,
            terminal_revision,
        )
        .map_err(|_| ReleaseError::Authentication)?;
        let changed = transaction
            .execute(
                "UPDATE release SET state=?1, revision=?2, state_authenticator=?3
                 WHERE activation=?4 AND run_id=?5 AND binding=?6
                   AND state=?7 AND revision=1",
                params![
                    terminal_state,
                    i64::from(terminal_revision),
                    terminal_authenticator.as_slice(),
                    attempt.activation.as_slice(),
                    attempt.run_id.as_slice(),
                    attempt.binding.as_slice(),
                    STARTED,
                ],
            )
            .map_err(|_| ReleaseError::Store)?;
        if changed != 1 {
            return Err(ReleaseError::CorruptRecord);
        }
        transaction.commit().map_err(|_| ReleaseError::Store)?;
        validate_connection(&connection)?;
        drop(connection);
        self.revalidate_storage()
    }

    fn validate_for_publication(
        &self,
        snapshot: &ReleaseSnapshot,
        key: &PublicationProtectionKey,
    ) -> Result<(), ReleaseError> {
        self.revalidate_storage()?;
        authenticate_snapshot(snapshot, key)?;
        self.revalidate_storage()?;
        Ok(())
    }

    fn lock_connection(&self) -> Result<MutexGuard<'_, Connection>, ReleaseError> {
        self.connection.lock().map_err(|_| ReleaseError::Store)
    }

    fn revalidate_storage(&self) -> Result<(), ReleaseError> {
        verify_database_file(&self.directory, &self.database_name, self.database_identity)
    }
}

struct StoredReleaseRecord {
    activation: Vec<u8>,
    swap_id: Vec<u8>,
    run_id: Vec<u8>,
    lez_commitment: Vec<u8>,
    topology_commitment: Vec<u8>,
    resource_id: Vec<u8>,
    observation: Vec<u8>,
    observation_authenticator: Vec<u8>,
    claim_partial_commitment: Vec<u8>,
    target: Vec<u8>,
    publication_id: Vec<u8>,
    window_start: i64,
    window_end: i64,
    binding: Vec<u8>,
    semantic_authenticator: Vec<u8>,
    key_id: String,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
    fingerprint: Vec<u8>,
    state_authenticator: Vec<u8>,
    state: String,
    revision: i64,
}

impl StoredReleaseRecord {
    fn validate(self) -> Result<ReleaseSnapshot, ReleaseError> {
        if self.observation.is_empty()
            || self.observation.len() > MAX_OBSERVATION_BYTES
            || self.target.is_empty()
            || self.target.len() > MAX_TARGET_BYTES
            || self.window_start < 0
            || self.window_end <= self.window_start
        {
            return Err(ReleaseError::CorruptRecord);
        }
        validate_key_id(&self.key_id).map_err(|_| ReleaseError::CorruptRecord)?;
        validate_ciphertext_length(self.ciphertext.len())
            .map_err(|_| ReleaseError::CorruptRecord)?;
        let activation = array(self.activation)?;
        let swap_id = array(self.swap_id)?;
        let run_id = array(self.run_id)?;
        let lez_commitment = array(self.lez_commitment)?;
        let topology_commitment = array(self.topology_commitment)?;
        let resource_id = array(self.resource_id)?;
        let observation_authenticator = array(self.observation_authenticator)?;
        let claim_partial_commitment = array(self.claim_partial_commitment)?;
        let publication_id = array(self.publication_id)?;
        let binding = array(self.binding)?;
        let semantic_authenticator = array(self.semantic_authenticator)?;
        let nonce = array(self.nonce)?;
        let fingerprint = array(self.fingerprint)?;
        let state_authenticator = array(self.state_authenticator)?;
        if activation == [0; 32]
            || swap_id == [0; 32]
            || run_id == [0; 32]
            || activation != derive_activation_id(&swap_id, &run_id)
            || lez_commitment == [0; 32]
            || topology_commitment == [0; 32]
            || resource_id == [0; 32]
            || claim_partial_commitment == [0; 32]
            || publication_id == [0; 32]
        {
            return Err(ReleaseError::CorruptRecord);
        }
        let window_start =
            u64::try_from(self.window_start).map_err(|_| ReleaseError::CorruptRecord)?;
        let window_end = u64::try_from(self.window_end).map_err(|_| ReleaseError::CorruptRecord)?;
        let immutable_context = immutable_release_context_bytes(
            &activation,
            &swap_id,
            &run_id,
            &lez_commitment,
            &topology_commitment,
            &resource_id,
            &claim_partial_commitment,
            &self.target,
            &publication_id,
            window_start,
            window_end,
        );
        let intent = ProtectedPublicationIntent::from_record_fields(
            self.key_id,
            nonce,
            self.ciphertext,
            fingerprint,
            &immutable_context,
        )
        .map_err(|_| ReleaseError::CorruptRecord)?;
        if hash(&exact_binding_bytes(&immutable_context, &intent)) != binding {
            return Err(ReleaseError::CorruptRecord);
        }
        let state = ReleaseState::from_record(&self.state, self.revision)?;
        Ok(ReleaseSnapshot {
            activation,
            run_id,
            resource_id,
            observation: self.observation,
            observation_authenticator,
            binding,
            semantic_authenticator,
            target: self.target,
            publication_id,
            window: ReleaseWindow {
                start: window_start,
                end: window_end,
            },
            immutable_context,
            intent,
            state_authenticator,
            state,
        })
    }
}

fn array<const N: usize>(bytes: Vec<u8>) -> Result<[u8; N], ReleaseError> {
    bytes.try_into().map_err(|_| ReleaseError::CorruptRecord)
}

fn select_record(
    connection: &Connection,
    activation: &[u8; 32],
) -> Result<Option<StoredReleaseRecord>, ReleaseError> {
    connection
        .query_row(
            "SELECT activation,swap_id,run_id,lez_commitment,topology_commitment,
                    resource_id,observation,observation_authenticator,
                    claim_partial_commitment,target,publication_id,window_start,window_end,
                    binding,semantic_authenticator,key_id,nonce,ciphertext,
                    fingerprint,state_authenticator,state,revision
             FROM release WHERE activation=?1",
            [activation.as_slice()],
            |row| {
                Ok(StoredReleaseRecord {
                    activation: row.get(0)?,
                    swap_id: row.get(1)?,
                    run_id: row.get(2)?,
                    lez_commitment: row.get(3)?,
                    topology_commitment: row.get(4)?,
                    resource_id: row.get(5)?,
                    observation: row.get(6)?,
                    observation_authenticator: row.get(7)?,
                    claim_partial_commitment: row.get(8)?,
                    target: row.get(9)?,
                    publication_id: row.get(10)?,
                    window_start: row.get(11)?,
                    window_end: row.get(12)?,
                    binding: row.get(13)?,
                    semantic_authenticator: row.get(14)?,
                    key_id: row.get(15)?,
                    nonce: row.get(16)?,
                    ciphertext: row.get(17)?,
                    fingerprint: row.get(18)?,
                    state_authenticator: row.get(19)?,
                    state: row.get(20)?,
                    revision: row.get(21)?,
                })
            },
        )
        .optional()
        .map_err(|_| ReleaseError::CorruptRecord)
}

fn resource_exists(connection: &Connection, resource_id: &[u8; 32]) -> Result<bool, ReleaseError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM release WHERE resource_id=?1)",
            [resource_id.as_slice()],
            |row| row.get(0),
        )
        .map_err(|_| ReleaseError::Store)
}

fn swap_run_exists(
    connection: &Connection,
    swap_id: &[u8; 32],
    run_id: &[u8; 32],
) -> Result<bool, ReleaseError> {
    connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM release WHERE swap_id=?1 AND run_id=?2
             )",
            params![swap_id.as_slice(), run_id.as_slice()],
            |row| row.get(0),
        )
        .map_err(|_| ReleaseError::Store)
}

#[allow(clippy::too_many_arguments)]
fn snapshot_from_plan(
    plan: ReleasePlan,
    immutable_context: Vec<u8>,
    intent: ProtectedPublicationIntent,
    binding: [u8; 32],
    semantic_authenticator: [u8; 32],
    observation_authenticator: [u8; 32],
    state_authenticator: [u8; 32],
) -> ReleaseSnapshot {
    ReleaseSnapshot {
        activation: plan.activation,
        run_id: plan.run_id,
        resource_id: plan.resource_id,
        observation: plan.observation,
        observation_authenticator,
        binding,
        semantic_authenticator,
        target: plan.target,
        publication_id: plan.publication_id,
        window: ReleaseWindow {
            start: plan.window_start,
            end: plan.window_end,
        },
        immutable_context,
        intent,
        state_authenticator,
        state: ReleaseState::Prepared,
    }
}

fn record_matches_plan(
    snapshot: &ReleaseSnapshot,
    plan: &ReleasePlan,
    immutable_context: &[u8],
) -> bool {
    snapshot.activation == plan.activation
        && snapshot.run_id == plan.run_id
        && snapshot.resource_id == plan.resource_id
        && snapshot.target == plan.target
        && snapshot.publication_id == plan.publication_id
        && snapshot.window.start == plan.window_start
        && snapshot.window.end == plan.window_end
        && snapshot.immutable_context == immutable_context
}

fn same_snapshot_binding(left: &ReleaseSnapshot, right: &ReleaseSnapshot) -> bool {
    left.activation == right.activation
        && left.run_id == right.run_id
        && left.resource_id == right.resource_id
        && left.binding == right.binding
        && left.semantic_authenticator == right.semantic_authenticator
        && left.target == right.target
        && left.publication_id == right.publication_id
        && left.window == right.window
        && left.immutable_context == right.immutable_context
        && left.intent == right.intent
}

fn authenticate_snapshot(
    snapshot: &ReleaseSnapshot,
    key: &PublicationProtectionKey,
) -> Result<zeroize::Zeroizing<Vec<u8>>, ReleaseError> {
    let plaintext = snapshot
        .intent
        .decrypt(key, &snapshot.immutable_context)
        .map_err(|_| ReleaseError::Authentication)?;
    if !verify_semantic_intent_authenticator(
        key,
        &snapshot.immutable_context,
        &plaintext,
        &snapshot.semantic_authenticator,
    )
    .map_err(|_| ReleaseError::Authentication)?
        || !verify_observation_authenticator(
            key,
            &snapshot.immutable_context,
            &snapshot.observation,
            &snapshot.observation_authenticator,
        )
        .map_err(|_| ReleaseError::Authentication)?
    {
        return Err(ReleaseError::Authentication);
    }
    let (state, revision) = snapshot.state.record();
    if !verify_release_state_authenticator(
        key,
        &snapshot.immutable_context,
        &snapshot.binding,
        state,
        revision,
        &snapshot.state_authenticator,
    )
    .map_err(|_| ReleaseError::Authentication)?
    {
        return Err(ReleaseError::Authentication);
    }
    Ok(plaintext)
}

fn validate_plan(plan: &ReleasePlan) -> Result<(), ReleaseError> {
    if plan.activation == [0; 32]
        || plan.swap_id == [0; 32]
        || plan.run_id == [0; 32]
        || plan.activation != derive_activation_id(&plan.swap_id, &plan.run_id)
        || plan.lez_commitment == [0; 32]
        || plan.topology_commitment == [0; 32]
        || plan.resource_id == [0; 32]
        || plan.observation.is_empty()
        || plan.observation.len() > MAX_OBSERVATION_BYTES
        || plan.claim_partial_commitment == [0; 32]
        || plan.target.is_empty()
        || plan.target.len() > MAX_TARGET_BYTES
        || plan.publication_id == [0; 32]
        || plan.window_start >= plan.window_end
        || plan.window_end > i64::MAX as u64
        || validate_plaintext_length(plan.publication.len()).is_err()
    {
        return Err(ReleaseError::InvalidBinding);
    }
    Ok(())
}

fn validate_database_path(path: &Path) -> Result<(PathBuf, PathBuf, OsString), ReleaseError> {
    if path.as_os_str().is_empty()
        || path == Path::new(":memory:")
        || path.as_os_str().to_string_lossy().starts_with("file:")
    {
        return Err(ReleaseError::InvalidPath);
    }
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        return Err(ReleaseError::InvalidPath);
    }
    let path = std::path::absolute(path).map_err(|_| ReleaseError::InvalidPath)?;
    if path
        .components()
        .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(ReleaseError::InvalidPath);
    }
    let Some(Component::Normal(database_name)) = path.components().next_back() else {
        return Err(ReleaseError::InvalidPath);
    };
    if database_name.is_empty() {
        return Err(ReleaseError::InvalidPath);
    }
    let database_name = database_name.to_os_string();
    let parent = path
        .parent()
        .ok_or(ReleaseError::InvalidPath)?
        .to_path_buf();
    Ok((path, parent, database_name))
}

struct SecureDirectory {
    descriptor: File,
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl SecureDirectory {
    fn open(path: &Path) -> Result<Self, ReleaseError> {
        validate_trusted_parent_chain(path)?;
        let descriptor = open_secure_directory(path)?;
        let metadata = validate_private_directory(&descriptor)?;
        let directory = Self {
            descriptor,
            path: path.to_path_buf(),
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        directory.revalidate()?;
        Ok(directory)
    }

    fn revalidate(&self) -> Result<(), ReleaseError> {
        validate_trusted_parent_chain(&self.path)?;
        let held = validate_private_directory(&self.descriptor)?;
        if held.dev() != self.device || held.ino() != self.inode {
            return Err(ReleaseError::InsecureDirectory);
        }
        let reopened = open_secure_directory(&self.path)?;
        let current = validate_private_directory(&reopened)?;
        if current.dev() != self.device || current.ino() != self.inode {
            return Err(ReleaseError::InsecureDirectory);
        }
        Ok(())
    }
}

fn open_secure_directory(path: &Path) -> Result<File, ReleaseError> {
    openat2(
        CWD,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .map(File::from)
    .map_err(|_| ReleaseError::InsecureDirectory)
}

fn validate_trusted_parent_chain(path: &Path) -> Result<(), ReleaseError> {
    let effective_uid = geteuid().as_raw();
    for ancestor in path.ancestors().skip(1) {
        let descriptor = open_secure_directory(ancestor)?;
        let metadata = descriptor
            .metadata()
            .map_err(|_| ReleaseError::InsecureDirectory)?;
        let trusted_owner = metadata.uid() == 0 || metadata.uid() == effective_uid;
        let group_or_other_writable = metadata.mode() & 0o022 != 0;
        let sticky = metadata.mode() & 0o1000 != 0;
        if !metadata.file_type().is_dir() || !trusted_owner || (group_or_other_writable && !sticky)
        {
            return Err(ReleaseError::InsecureDirectory);
        }
    }
    Ok(())
}

fn validate_private_directory(file: &File) -> Result<std::fs::Metadata, ReleaseError> {
    let metadata = file
        .metadata()
        .map_err(|_| ReleaseError::InsecureDirectory)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(ReleaseError::InsecureDirectory);
    }
    Ok(metadata)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DatabaseIdentity {
    device: u64,
    inode: u64,
}

fn prepare_database_file(
    directory: &SecureDirectory,
    name: &OsStr,
) -> Result<(DatabaseIdentity, File), ReleaseError> {
    directory.revalidate()?;
    let descriptor = match openat(
        &directory.descriptor,
        name,
        OFlags::RDWR | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(descriptor) => descriptor,
        Err(Errno::NOENT) => openat(
            &directory.descriptor,
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| ReleaseError::Store)?,
        Err(Errno::LOOP) => return Err(ReleaseError::UnsafeDatabaseFile),
        Err(_) => return Err(ReleaseError::Store),
    };
    let file = File::from(descriptor);
    let identity = validate_database_file(&file)?;
    file.sync_all().map_err(|_| ReleaseError::Store)?;
    directory
        .descriptor
        .sync_all()
        .map_err(|_| ReleaseError::Store)?;
    directory.revalidate()?;
    Ok((identity, file))
}

fn verify_database_file(
    directory: &SecureDirectory,
    name: &OsStr,
    expected: DatabaseIdentity,
) -> Result<(), ReleaseError> {
    directory.revalidate()?;
    let descriptor = openat(
        &directory.descriptor,
        name,
        OFlags::RDWR | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| {
        if error == Errno::LOOP || error == Errno::NOENT {
            ReleaseError::UnsafeDatabaseFile
        } else {
            ReleaseError::Store
        }
    })?;
    let current = validate_database_file(&File::from(descriptor))?;
    if current != expected {
        return Err(ReleaseError::UnsafeDatabaseFile);
    }
    directory.revalidate()
}

fn validate_database_file(file: &File) -> Result<DatabaseIdentity, ReleaseError> {
    let metadata = file
        .metadata()
        .map_err(|_| ReleaseError::UnsafeDatabaseFile)?;
    if !metadata.file_type().is_file()
        || metadata.uid() != geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(ReleaseError::UnsafeDatabaseFile);
    }
    Ok(DatabaseIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn migrate(connection: &mut Connection) -> Result<(), ReleaseError> {
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|_| ReleaseError::Store)?;
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| ReleaseError::Store)?;
    let object_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| ReleaseError::Store)?;
    match (application_id, version, object_count) {
        (DATABASE_APPLICATION_ID, DATABASE_SCHEMA_VERSION, _) => {
            return validate_schema(connection);
        }
        (_, version, _) if version > DATABASE_SCHEMA_VERSION => {
            return Err(ReleaseError::FutureSchema);
        }
        (0, 0, 0) => {}
        _ => return Err(ReleaseError::ForeignSchema),
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| ReleaseError::Store)?;
    transaction
        .execute_batch(CREATE_TABLE_SQL)
        .map_err(|_| ReleaseError::Store)?;
    transaction
        .pragma_update(None, "application_id", DATABASE_APPLICATION_ID)
        .map_err(|_| ReleaseError::Store)?;
    transaction
        .pragma_update(None, "user_version", DATABASE_SCHEMA_VERSION)
        .map_err(|_| ReleaseError::Store)?;
    transaction.commit().map_err(|_| ReleaseError::Store)?;
    validate_schema(connection)
}

fn configure_connection(connection: &Connection) -> Result<(), ReleaseError> {
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(|_| ReleaseError::Store)?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(|_| ReleaseError::Store)?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|_| ReleaseError::Store)?;
    connection
        .pragma_update(None, "secure_delete", "ON")
        .map_err(|_| ReleaseError::Store)
}

fn validate_connection(connection: &Connection) -> Result<(), ReleaseError> {
    validate_schema(connection)?;
    let journal_mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(|_| ReleaseError::Store)?;
    let synchronous: i64 = connection
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .map_err(|_| ReleaseError::Store)?;
    let foreign_keys: i64 = connection
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .map_err(|_| ReleaseError::Store)?;
    let secure_delete: i64 = connection
        .pragma_query_value(None, "secure_delete", |row| row.get(0))
        .map_err(|_| ReleaseError::Store)?;
    let integrity: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|_| ReleaseError::CorruptRecord)?;
    if !journal_mode.eq_ignore_ascii_case("wal")
        || synchronous != 2
        || foreign_keys != 1
        || secure_delete != 1
        || integrity != "ok"
    {
        return Err(ReleaseError::CorruptRecord);
    }
    Ok(())
}

fn validate_schema(connection: &Connection) -> Result<(), ReleaseError> {
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|_| ReleaseError::Store)?;
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| ReleaseError::Store)?;
    if version > DATABASE_SCHEMA_VERSION {
        return Err(ReleaseError::FutureSchema);
    }
    if application_id != DATABASE_APPLICATION_ID || version != DATABASE_SCHEMA_VERSION {
        return Err(ReleaseError::ForeignSchema);
    }
    let objects: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| ReleaseError::Store)?;
    let actual_sql: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='release'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| ReleaseError::Store)?;
    if objects != 1
        || actual_sql
            .as_deref()
            .is_none_or(|sql| normalize_sql(sql) != normalize_sql(CREATE_TABLE_SQL))
    {
        return Err(ReleaseError::ForeignSchema);
    }
    Ok(())
}

fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn _transaction_type_proof(_: &Transaction<'_>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{envelope_fingerprint, exact_binding_bytes};
    use rusqlite::Connection;
    use std::{
        fs::{self, OpenOptions},
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
        sync::{Arc, Barrier},
        thread,
    };
    use tempfile::{TempDir, tempdir};

    fn directory() -> TempDir {
        let directory = tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        directory
    }

    fn database_path(directory: &TempDir) -> PathBuf {
        directory.path().join("release.sqlite")
    }

    fn protection_key(seed: u8) -> PublicationProtectionKey {
        PublicationProtectionKey::new("release-v1", [seed; 32]).unwrap()
    }

    fn binding(seed: u8, _key: &PublicationProtectionKey, publication: &[u8]) -> ReleasePlan {
        let swap_id = [seed; 32];
        let run_id = [seed.wrapping_add(1); 32];
        let observation = vec![seed.wrapping_add(4); 48];
        ReleasePlan {
            activation: derive_activation_id(&swap_id, &run_id),
            swap_id,
            run_id,
            lez_commitment: [seed.wrapping_add(2); 32],
            topology_commitment: [seed.wrapping_add(3); 32],
            resource_id: [seed.wrapping_add(5); 32],
            observation,
            claim_partial_commitment: [seed.wrapping_add(6); 32],
            target: format!("lez-target-{seed}").into_bytes(),
            publication_id: [seed.wrapping_add(7); 32],
            window_start: 100,
            window_end: 200,
            publication: zeroize::Zeroizing::new(publication.to_vec()),
        }
    }

    fn duplicate(bindings: &ReleasePlan) -> ReleasePlan {
        ReleasePlan {
            activation: bindings.activation,
            swap_id: bindings.swap_id,
            run_id: bindings.run_id,
            lez_commitment: bindings.lez_commitment,
            topology_commitment: bindings.topology_commitment,
            resource_id: bindings.resource_id,
            observation: bindings.observation.clone(),
            claim_partial_commitment: bindings.claim_partial_commitment,
            target: bindings.target.clone(),
            publication_id: bindings.publication_id,
            window_start: bindings.window_start,
            window_end: bindings.window_end,
            publication: zeroize::Zeroizing::new(bindings.publication.as_slice().to_vec()),
        }
    }

    static_assertions::assert_not_impl_any!(ReleaseSnapshot: Clone);
    static_assertions::assert_not_impl_any!(PublicationAttempt: Clone);

    fn raw_secure_database(path: &Path) -> Connection {
        let _file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        Connection::open(path).unwrap()
    }

    #[test]
    fn creates_owner_private_database_and_verified_pragmas() {
        let directory = directory();
        let path = database_path(&directory);
        let store = ReleaseStore::open(&path).unwrap();
        let metadata = fs::symlink_metadata(&path).unwrap();
        assert!(metadata.file_type().is_file());
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
        assert_eq!(metadata.nlink(), 1);

        let connection = store.connection.lock().unwrap();
        let journal: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        let synchronous: i64 = connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .unwrap();
        let foreign_keys: i64 = connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        let secure_delete: i64 = connection
            .pragma_query_value(None, "secure_delete", |row| row.get(0))
            .unwrap();
        let application_id: i64 = connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(journal.to_ascii_lowercase(), "wal");
        assert_eq!(synchronous, 2);
        assert_eq!(foreign_keys, 1);
        assert_eq!(secure_delete, 1);
        assert_eq!(application_id, DATABASE_APPLICATION_ID);
        assert_eq!(version, DATABASE_SCHEMA_VERSION);

        let columns: Vec<String> = connection
            .prepare("SELECT name FROM pragma_table_info('release') ORDER BY cid")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(!columns.iter().any(|column| column == "exact"));
        assert!(!columns.iter().any(|column| column == "immutable_context"));
        assert!(
            columns
                .iter()
                .any(|column| column == "claim_partial_commitment")
        );
        assert!(!columns.iter().any(|column| column == "hidden_commitment"));
        let sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name='release'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(sql.ends_with("STRICT"));
        assert!(sql.contains("length(ciphertext) BETWEEN 17 AND 2000016"));
        assert!(sql.contains("resource_id BLOB NOT NULL UNIQUE"));
        assert!(sql.contains("UNIQUE(swap_id, run_id)"));
        assert!(sql.contains("activation != zeroblob(32)"));
        assert!(sql.contains("swap_id BLOB NOT NULL"));
        assert!(sql.contains("run_id BLOB NOT NULL"));
        assert!(sql.contains("'admitted'"));
        assert!(sql.contains("'suppressed'"));
        assert!(sql.contains("publication_id != zeroblob(32)"));
        assert!(sql.contains("window_end > window_start"));
        assert_eq!(DATABASE_SCHEMA_VERSION, 3);
    }

    #[test]
    fn release_window_is_non_empty_and_half_open() {
        assert_eq!(
            ReleaseWindow::new(100, 100).unwrap_err(),
            ReleaseError::InvalidBinding
        );
        assert_eq!(ReleaseWindow::new(100, 101).unwrap().end(), 101);
        assert_eq!(
            ReleaseWindow::new(i64::MAX as u64, i64::MAX as u64 + 1).unwrap_err(),
            ReleaseError::InvalidBinding
        );
    }

    #[test]
    fn rejects_memory_uri_empty_and_non_file_paths() {
        assert_eq!(
            ReleaseStore::open("").unwrap_err(),
            ReleaseError::InvalidPath
        );
        assert_eq!(
            ReleaseStore::open(":memory:").unwrap_err(),
            ReleaseError::InvalidPath
        );
        assert_eq!(
            ReleaseStore::open("file:release.sqlite?mode=memory").unwrap_err(),
            ReleaseError::InvalidPath
        );
        let directory = directory();
        assert_eq!(
            ReleaseStore::open(directory.path()).unwrap_err(),
            ReleaseError::InvalidPath
        );
    }

    #[test]
    fn rejects_insecure_mode_symlink_hardlink_and_parent_components() {
        let mode_directory = directory();
        let mode_path = database_path(&mode_directory);
        drop(ReleaseStore::open(&mode_path).unwrap());
        fs::set_permissions(&mode_path, fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(
            ReleaseStore::open(&mode_path).unwrap_err(),
            ReleaseError::UnsafeDatabaseFile
        );

        let hardlink_directory = directory();
        let hardlink_path = database_path(&hardlink_directory);
        drop(ReleaseStore::open(&hardlink_path).unwrap());
        fs::hard_link(
            &hardlink_path,
            hardlink_directory.path().join("alias.sqlite"),
        )
        .unwrap();
        assert_eq!(
            ReleaseStore::open(&hardlink_path).unwrap_err(),
            ReleaseError::UnsafeDatabaseFile
        );

        let symlink_directory = directory();
        let target_path = database_path(&symlink_directory);
        drop(ReleaseStore::open(&target_path).unwrap());
        let link_path = symlink_directory.path().join("link.sqlite");
        std::os::unix::fs::symlink(&target_path, &link_path).unwrap();
        assert_eq!(
            ReleaseStore::open(&link_path).unwrap_err(),
            ReleaseError::UnsafeDatabaseFile
        );

        let component_root = directory();
        let real = component_root.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
        let linked = component_root.path().join("linked");
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        assert_eq!(
            ReleaseStore::open(linked.join("release.sqlite")).unwrap_err(),
            ReleaseError::InsecureDirectory
        );

        let public_parent = directory();
        fs::set_permissions(public_parent.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            ReleaseStore::open(database_path(&public_parent)).unwrap_err(),
            ReleaseError::InsecureDirectory
        );
    }

    #[test]
    fn live_inode_replacement_is_detected_before_use() {
        let key = protection_key(0x30);
        let directory = directory();
        let path = database_path(&directory);
        let store = ReleaseStore::open(&path).unwrap();
        let displaced = directory.path().join("displaced.sqlite");
        fs::rename(&path, &displaced).unwrap();
        let _replacement = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        assert_eq!(
            store
                .prepare(binding(10, &key, b"never written"), &key)
                .unwrap_err(),
            ReleaseError::UnsafeDatabaseFile
        );
    }

    #[test]
    fn rejects_future_foreign_and_substituted_schema() {
        let future = directory();
        let future_path = database_path(&future);
        drop(ReleaseStore::open(&future_path).unwrap());
        Connection::open(&future_path)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        assert_eq!(
            ReleaseStore::open(&future_path).unwrap_err(),
            ReleaseError::FutureSchema
        );

        let foreign = directory();
        let foreign_path = database_path(&foreign);
        let connection = raw_secure_database(&foreign_path);
        connection
            .execute_batch("CREATE TABLE foreign_data(value TEXT) STRICT;")
            .unwrap();
        connection
            .pragma_update(None, "application_id", DATABASE_APPLICATION_ID)
            .unwrap();
        connection
            .pragma_update(None, "user_version", DATABASE_SCHEMA_VERSION)
            .unwrap();
        drop(connection);
        assert_eq!(
            ReleaseStore::open(&foreign_path).unwrap_err(),
            ReleaseError::ForeignSchema
        );

        let legacy = directory();
        let legacy_path = database_path(&legacy);
        drop(ReleaseStore::open(&legacy_path).unwrap());
        Connection::open(&legacy_path)
            .unwrap()
            .pragma_update(None, "user_version", 1)
            .unwrap();
        assert_eq!(
            ReleaseStore::open(&legacy_path).unwrap_err(),
            ReleaseError::ForeignSchema
        );

        let wrong_application = directory();
        let wrong_path = database_path(&wrong_application);
        drop(ReleaseStore::open(&wrong_path).unwrap());
        Connection::open(&wrong_path)
            .unwrap()
            .pragma_update(None, "application_id", 7)
            .unwrap();
        assert_eq!(
            ReleaseStore::open(&wrong_path).unwrap_err(),
            ReleaseError::ForeignSchema
        );
    }

    #[test]
    fn semantic_prepare_reuses_ciphertext_across_restart_but_drift_and_replay_fail() {
        let key = protection_key(0x31);
        let directory = directory();
        let path = database_path(&directory);
        let store = ReleaseStore::open(&path).unwrap();
        let first = binding(11, &key, b"publish-once");
        let exact_replay = duplicate(&first);
        let resource_id = first.resource_id;
        let initial = store.prepare(first, &key).unwrap();
        let initial_nonce = initial.intent.nonce;
        let initial_ciphertext = initial.intent.ciphertext.clone();
        drop(store);
        let store = ReleaseStore::open(&path).unwrap();
        let replayed = store.prepare(exact_replay, &key).unwrap();
        assert_eq!(initial, replayed);
        assert_eq!(replayed.intent.nonce, initial_nonce);
        assert_eq!(replayed.intent.ciphertext, initial_ciphertext);

        let drift = binding(11, &key, b"changed-publication");
        assert_eq!(
            store.prepare(drift, &key).unwrap_err(),
            ReleaseError::BindingMismatch
        );

        let mut reused = binding(12, &key, b"other");
        reused.resource_id = resource_id;
        assert_eq!(
            store.prepare(reused, &key).unwrap_err(),
            ReleaseError::ObservationReplay
        );
    }

    #[test]
    fn later_tip_rescan_updates_only_authenticated_observation() {
        let key = protection_key(0x40);
        let directory = directory();
        let path = database_path(&directory);
        let store = ReleaseStore::open(&path).unwrap();
        let first = binding(41, &key, b"same semantic publication");
        let activation = first.activation;
        let run_id = first.run_id;
        let mut later = duplicate(&first);
        later.observation = vec![0xa5; 96];
        let first_snapshot = store.prepare(first, &key).unwrap();
        let first_nonce = first_snapshot.intent.nonce;
        let first_ciphertext = first_snapshot.intent.ciphertext.clone();
        drop(store);

        let reopened = ReleaseStore::open(&path).unwrap();
        let updated = reopened.prepare(later, &key).unwrap();
        assert_eq!(updated.resource_id(), first_snapshot.resource_id());
        assert_eq!(updated.observation, vec![0xa5; 96]);
        assert_eq!(updated.intent.nonce, first_nonce);
        assert_eq!(updated.intent.ciphertext, first_ciphertext);
        assert_ne!(
            updated.observation_authenticator,
            first_snapshot.observation_authenticator
        );
        drop(reopened);

        let reopened_again = ReleaseStore::open(path).unwrap();
        let loaded = reopened_again
            .load_by_activation_run(activation, run_id, &key)
            .unwrap();
        assert_eq!(loaded.observation, vec![0xa5; 96]);
    }

    #[test]
    fn separate_first_inserts_use_random_nonces_even_for_equal_plaintext() {
        let key = protection_key(0x41);
        let directory = directory();
        let store = ReleaseStore::open(database_path(&directory)).unwrap();
        let first = store
            .prepare(binding(42, &key, b"equal plaintext"), &key)
            .unwrap();
        let second = store
            .prepare(binding(43, &key, b"equal plaintext"), &key)
            .unwrap();
        assert_ne!(first.intent.nonce, second.intent.nonce);
        assert_ne!(first.intent.ciphertext, second.intent.ciphertext);
    }

    #[test]
    fn activation_is_deterministic_nonzero_and_swap_run_are_exact_binary() {
        let key = protection_key(0x42);
        let directory = directory();
        let store = ReleaseStore::open(database_path(&directory)).unwrap();
        let plan = binding(44, &key, b"intent");
        assert_eq!(
            plan.activation,
            derive_activation_id(&plan.swap_id, &plan.run_id)
        );
        assert_ne!(plan.activation, [0; 32]);
        let activation = plan.activation;
        let swap_id = plan.swap_id;
        let run_id = plan.run_id;
        store.prepare(plan, &key).unwrap();
        let connection = store.connection.lock().unwrap();
        let (swap_type, run_type): (String, String) = connection
            .query_row(
                "SELECT typeof(swap_id),typeof(run_id) FROM release WHERE activation=?1",
                [activation.as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let (stored_swap, stored_run): (Vec<u8>, Vec<u8>) = connection
            .query_row(
                "SELECT swap_id,run_id FROM release WHERE activation=?1",
                [activation.as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((swap_type.as_str(), run_type.as_str()), ("blob", "blob"));
        assert_eq!(stored_swap, swap_id);
        assert_eq!(stored_run, run_id);
        drop(connection);

        let mut zero_activation = binding(45, &key, b"intent");
        zero_activation.activation = [0; 32];
        assert_eq!(
            store.prepare(zero_activation, &key).unwrap_err(),
            ReleaseError::InvalidBinding
        );
    }

    #[test]
    fn real_reopen_loads_target_window_intent_and_state_without_prepare_token() {
        let key = protection_key(0x32);
        let directory = directory();
        let path = database_path(&directory);
        let bindings = binding(21, &key, b"exact authorize claim transaction");
        let activation = bindings.activation;
        let run = bindings.run_id;
        drop(
            ReleaseStore::open(&path)
                .unwrap()
                .prepare(bindings, &key)
                .unwrap(),
        );

        let store = ReleaseStore::open(&path).unwrap();
        let loaded = store.load_by_activation_run(activation, run, &key).unwrap();
        assert_eq!(loaded.state(), ReleaseState::Prepared);
        assert_eq!(loaded.target(), b"lez-target-21");
        assert_eq!(loaded.window(), ReleaseWindow::new(100, 200).unwrap());
        assert!(!loaded.protected_intent().ciphertext().is_empty());
        let PublicationDecision::Send(attempt) = store.begin_publication(loaded, &key).unwrap()
        else {
            panic!("reopened prepared record must own the one send");
        };
        assert_eq!(
            attempt.opened_intent(&key).unwrap().as_slice(),
            b"exact authorize claim transaction"
        );
        drop(store);

        let reopened = ReleaseStore::open(&path).unwrap();
        let loaded = reopened
            .load_by_activation_run(activation, run, &key)
            .unwrap();
        assert_eq!(loaded.state(), ReleaseState::PublicationStarted);
        assert_eq!(
            reopened.begin_publication(loaded, &key).unwrap(),
            PublicationDecision::ObserveOnly
        );
    }

    #[test]
    fn wrong_run_or_key_cannot_load_authority() {
        let key = protection_key(0x33);
        let wrong_key = protection_key(0x34);
        let directory = directory();
        let store = ReleaseStore::open(database_path(&directory)).unwrap();
        let bindings = binding(22, &key, b"intent");
        let activation = bindings.activation;
        let run = bindings.run_id;
        store.prepare(bindings, &key).unwrap();
        assert_eq!(
            store
                .load_by_activation_run(activation, [0xee; 32], &key)
                .unwrap_err(),
            ReleaseError::BindingMismatch
        );
        assert_eq!(
            store
                .load_by_activation_run(activation, run, &wrong_key)
                .unwrap_err(),
            ReleaseError::Authentication
        );
    }

    #[test]
    fn tamper_and_substituted_ciphertext_fail_before_authority_load() {
        let key = protection_key(0x35);
        let directory = directory();
        let store = ReleaseStore::open(database_path(&directory)).unwrap();
        let bindings = binding(23, &key, b"intent");
        let activation = bindings.activation;
        let run = bindings.run_id;
        let snapshot = store.prepare(bindings, &key).unwrap();

        let mut substituted = snapshot.intent.clone();
        substituted.ciphertext[0] ^= 1;
        substituted.fingerprint = envelope_fingerprint(
            &substituted.key_id,
            &substituted.nonce,
            &substituted.ciphertext,
            &snapshot.immutable_context,
        );
        let substituted_binding = hash(&exact_binding_bytes(
            &snapshot.immutable_context,
            &substituted,
        ));
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE release SET ciphertext=?1,fingerprint=?2,binding=?3 WHERE activation=?4",
                params![
                    substituted.ciphertext,
                    substituted.fingerprint.as_slice(),
                    substituted_binding.as_slice(),
                    activation.as_slice(),
                ],
            )
            .unwrap();
        assert_eq!(
            store
                .load_by_activation_run(activation, run, &key)
                .unwrap_err(),
            ReleaseError::Authentication
        );

        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE release SET target=?1 WHERE activation=?2",
                params![b"substituted-target".as_slice(), activation.as_slice()],
            )
            .unwrap();
        assert_eq!(
            store
                .load_by_activation_run(activation, run, &key)
                .unwrap_err(),
            ReleaseError::CorruptRecord
        );
    }

    #[test]
    fn mutable_observation_substitution_fails_authentication() {
        let key = protection_key(0x46);
        let directory = directory();
        let store = ReleaseStore::open(database_path(&directory)).unwrap();
        let plan = binding(46, &key, b"intent");
        let activation = plan.activation;
        let run_id = plan.run_id;
        store.prepare(plan, &key).unwrap();
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE release SET observation=?1 WHERE activation=?2",
                params![
                    b"unauthenticated later tip".as_slice(),
                    activation.as_slice()
                ],
            )
            .unwrap();
        assert_eq!(
            store
                .load_by_activation_run(activation, run_id, &key)
                .unwrap_err(),
            ReleaseError::Authentication
        );
    }

    #[test]
    fn corrupt_state_pair_fails_closed_on_load() {
        let key = protection_key(0x36);
        let directory = directory();
        let store = ReleaseStore::open(database_path(&directory)).unwrap();
        let bindings = binding(24, &key, b"intent");
        let activation = bindings.activation;
        let run = bindings.run_id;
        store.prepare(bindings, &key).unwrap();
        let connection = store.connection.lock().unwrap();
        connection
            .pragma_update(None, "ignore_check_constraints", "ON")
            .unwrap();
        connection
            .execute(
                "UPDATE release SET state=?1,revision=0 WHERE activation=?2",
                params![STARTED, activation.as_slice()],
            )
            .unwrap();
        connection
            .pragma_update(None, "ignore_check_constraints", "OFF")
            .unwrap();
        drop(connection);
        assert_eq!(
            store
                .load_by_activation_run(activation, run, &key)
                .unwrap_err(),
            ReleaseError::CorruptRecord
        );
    }

    #[test]
    fn valid_state_pair_without_matching_hmac_fails_authentication() {
        let key = protection_key(0x3a);
        let directory = directory();
        let store = ReleaseStore::open(database_path(&directory)).unwrap();
        let bindings = binding(27, &key, b"intent");
        let activation = bindings.activation;
        let run = bindings.run_id;
        store.prepare(bindings, &key).unwrap();
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE release SET state=?1,revision=1 WHERE activation=?2",
                params![STARTED, activation.as_slice()],
            )
            .unwrap();
        assert_eq!(
            store
                .load_by_activation_run(activation, run, &key)
                .unwrap_err(),
            ReleaseError::Authentication
        );
    }

    #[test]
    fn multiple_connections_have_exactly_one_send_winner() {
        let key = protection_key(0x37);
        let directory = directory();
        let path = database_path(&directory);
        let bindings = binding(25, &key, b"publish once");
        let activation = bindings.activation;
        let run = bindings.run_id;
        ReleaseStore::open(&path)
            .unwrap()
            .prepare(bindings, &key)
            .unwrap();

        let barrier = Arc::new(Barrier::new(3));
        let mut joins = Vec::new();
        for _ in 0..2 {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            joins.push(thread::spawn(move || {
                let key = protection_key(0x37);
                let store = ReleaseStore::open(path).unwrap();
                let snapshot = store.load_by_activation_run(activation, run, &key).unwrap();
                barrier.wait();
                store.begin_publication(snapshot, &key).unwrap()
            }));
        }
        barrier.wait();
        let decisions: Vec<_> = joins.into_iter().map(|join| join.join().unwrap()).collect();
        assert_eq!(
            decisions
                .iter()
                .filter(|decision| matches!(decision, PublicationDecision::Send(_)))
                .count(),
            1
        );
        assert_eq!(
            decisions
                .iter()
                .filter(|decision| matches!(decision, PublicationDecision::ObserveOnly))
                .count(),
            1
        );
    }

    #[test]
    fn snapshot_and_attempt_debug_redact_durable_identifiers() {
        let key = protection_key(0x39);
        let directory = directory();
        let store = ReleaseStore::open(database_path(&directory)).unwrap();
        let bindings = binding(28, &key, b"secret-publication-payload");
        let snapshot = store.prepare(bindings, &key).unwrap();
        let snapshot_debug = format!("{snapshot:?}");
        for forbidden in [
            "run-28",
            "lez-target-28",
            "secret-publication-payload",
            "28, 28",
        ] {
            assert!(
                !snapshot_debug.contains(forbidden),
                "snapshot Debug leaked {forbidden}"
            );
        }
        let PublicationDecision::Send(attempt) = store.begin_publication(snapshot, &key).unwrap()
        else {
            panic!("fresh snapshot must win publication");
        };
        let attempt_debug = format!("{attempt:?}");
        for forbidden in [
            "run-28",
            "lez-target-28",
            "secret-publication-payload",
            "28, 28",
        ] {
            assert!(
                !attempt_debug.contains(forbidden),
                "attempt Debug leaked {forbidden}"
            );
        }
    }

    #[test]
    fn admitted_restart_is_terminal_observe_only_after_opening_intent() {
        let key = protection_key(0x3a);
        let directory = directory();
        let path = database_path(&directory);
        let bindings = binding(29, &key, b"exact admitted publication");
        let activation = bindings.activation;
        let run = bindings.run_id;
        let store = ReleaseStore::open(&path).unwrap();
        let prepared = store.prepare(bindings, &key).unwrap();
        let PublicationDecision::Send(attempt) = store.begin_publication(prepared, &key).unwrap()
        else {
            panic!("first process must win send");
        };
        assert_eq!(
            attempt.opened_intent(&key).unwrap().as_slice(),
            b"exact admitted publication"
        );
        store.mark_admitted(*attempt, &key).unwrap();
        drop(store);

        let reopened = ReleaseStore::open(path).unwrap();
        let snapshot = reopened
            .load_by_activation_run(activation, run, &key)
            .unwrap();
        assert_eq!(snapshot.state(), ReleaseState::Admitted);
        assert_eq!(
            reopened.begin_publication(snapshot, &key).unwrap(),
            PublicationDecision::ObserveOnly
        );
    }

    #[test]
    fn ambiguous_restart_is_permanently_observe_only() {
        let key = protection_key(0x38);
        let directory = directory();
        let path = database_path(&directory);
        let bindings = binding(26, &key, b"publish once");
        let activation = bindings.activation;
        let run = bindings.run_id;
        let store = ReleaseStore::open(&path).unwrap();
        let prepared = store.prepare(bindings, &key).unwrap();
        let PublicationDecision::Send(attempt) = store.begin_publication(prepared, &key).unwrap()
        else {
            panic!("first process must win send");
        };
        store.mark_ambiguous(*attempt, &key).unwrap();
        drop(store);

        let reopened = ReleaseStore::open(path).unwrap();
        let snapshot = reopened
            .load_by_activation_run(activation, run, &key)
            .unwrap();
        assert_eq!(snapshot.state(), ReleaseState::Ambiguous);
        assert_eq!(
            reopened.begin_publication(snapshot, &key).unwrap(),
            PublicationDecision::ObserveOnly
        );
    }
}
