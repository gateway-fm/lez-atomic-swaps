use std::{
    fmt,
    fs::File,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use async_trait::async_trait;
use common::{HashType, transaction::LeeTransaction};
use jsonrpsee::core::ClientError;
use lez_bridge_protocol::{
    DiscoveryWindow, ExactTransactionBytes, Hex32, Participant, RequestId, RunId,
    RuntimeDescriptor, TransactionId,
};
use nssa::{Account, AccountId};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use rustix::{
    fs::{Mode, OFlags, openat},
    io::Errno,
    process::geteuid,
};
use serde::{Deserialize, Serialize};

use crate::{
    PrepareVaultClaimRequest, PrepareVaultClaimResult, VaultClaimPlanner, VaultClaimPrepareError,
    decode_official_public_transaction,
    durable_reservation::{DurableReservationError, SecureStateDirectory},
};

const DATABASE_FILENAME: &str = "vault-claim-effects.v1.sqlite";
const DATABASE_SCHEMA_VERSION: i64 = 1;
const DATABASE_APPLICATION_ID: i64 = 0x4c5a_5643;
const MAX_TYPED_EFFECT_JSON_BYTES: usize = 8 * 1024 * 1024;
const MAX_BINDING_JSON_BYTES: usize = 64 * 1024;

/// Immutable process identity stored in one role-local effect journal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct VaultClaimActorBinding {
    run_id: RunId,
    role: Participant,
    runtime: RuntimeDescriptor,
    signer_account_id: Hex32,
}

impl VaultClaimActorBinding {
    /// Returns the run that exclusively owns this journal.
    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Returns the one actor role that owns this journal.
    pub const fn role(&self) -> Participant {
        self.role
    }

    /// Returns the complete pinned runtime identity.
    pub const fn runtime(&self) -> &RuntimeDescriptor {
        &self.runtime
    }

    /// Returns the official public signer account.
    pub const fn signer_account_id(&self) -> Hex32 {
        self.signer_account_id
    }
}

/// Typed isolation scope for the pre-swap Vault onboarding effect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[must_use]
pub enum VaultClaimEffectScope {
    /// One genesis allocation moving from an owner's Vault to that owner.
    VaultOnboarding {
        /// Official allocation owner.
        owner_account_id: Hex32,
        /// Official owner-derived Vault PDA.
        vault_account_id: Hex32,
        /// Exact genesis allocation claimed once.
        allocation: u128,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum VaultClaimEffectOperation {
    ClaimGenesisAllocation,
}

/// Complete durable identity of one exact Vault Claim node effect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct VaultClaimEffectIdentity {
    run_id: RunId,
    request_id: RequestId,
    role: Participant,
    operation: VaultClaimEffectOperation,
    scope: VaultClaimEffectScope,
    runtime: RuntimeDescriptor,
    transaction_id: TransactionId,
}

impl VaultClaimEffectIdentity {
    /// Returns the typed Vault onboarding scope.
    pub const fn scope(&self) -> &VaultClaimEffectScope {
        &self.scope
    }

    /// Returns the complete runtime identity committed by this effect.
    pub const fn runtime(&self) -> &RuntimeDescriptor {
        &self.runtime
    }

    /// Returns the exact official transaction identity.
    pub const fn transaction_id(&self) -> &TransactionId {
        &self.transaction_id
    }
}

/// Exact official account snapshots and query tips captured before preparation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct VaultClaimBeforeState {
    owner_account_id: Hex32,
    owner_account: Account,
    vault_account_id: Hex32,
    vault_account: Account,
    sequencer_tip: u64,
    indexer_tip: Option<u64>,
}

impl VaultClaimBeforeState {
    /// Captures typed official pre-Claim account values and fixed query tips.
    ///
    /// # Errors
    ///
    /// Rejects an owner aliased to its Vault. Complete request-specific checks
    /// are performed by [`PreparedVaultClaimEffect::new`].
    pub fn new(
        owner_account_id: AccountId,
        owner_account: Account,
        vault_account_id: AccountId,
        vault_account: Account,
        sequencer_tip: u64,
        indexer_tip: Option<u64>,
    ) -> Result<Self, VaultClaimEffectPrepareError> {
        if owner_account_id == vault_account_id {
            return Err(VaultClaimEffectPrepareError::InvalidBeforeState);
        }
        Ok(Self {
            owner_account_id: Hex32::from_bytes(owner_account_id.into_value()),
            owner_account,
            vault_account_id: Hex32::from_bytes(vault_account_id.into_value()),
            vault_account,
            sequencer_tip,
            indexer_tip,
        })
    }

    /// Returns the captured owner ID.
    pub const fn owner_account_id(&self) -> &Hex32 {
        &self.owner_account_id
    }

    /// Returns the captured owner Vault ID.
    pub const fn vault_account_id(&self) -> &Hex32 {
        &self.vault_account_id
    }
}

/// Invalid typed or official data while making a prepared Claim eligible.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum VaultClaimEffectPrepareError {
    /// The original official Claim no longer validates under its planner.
    #[error("prepared Vault Claim is not valid for the isolated actor")]
    InvalidPrepared,
    /// The typed account snapshots or query bounds do not match the request.
    #[error("Vault Claim before-state or query bounds are invalid")]
    InvalidBeforeState,
}

impl From<VaultClaimPrepareError> for VaultClaimEffectPrepareError {
    fn from(_: VaultClaimPrepareError) -> Self {
        Self::InvalidPrepared
    }
}

/// One validated exact Vault Claim plus its durable query authority.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct PreparedVaultClaimEffect {
    actor_binding: VaultClaimActorBinding,
    identity: VaultClaimEffectIdentity,
    request: PrepareVaultClaimRequest,
    result: PrepareVaultClaimResult,
    before_state: VaultClaimBeforeState,
    sequencer_window: DiscoveryWindow,
    indexer_window: DiscoveryWindow,
}

impl PreparedVaultClaimEffect {
    /// Constructs a submission candidate from official prepared bytes and
    /// source-correct pre-Claim account snapshots.
    ///
    /// # Errors
    ///
    /// Rejects any request, runtime, signer, transaction, account, allocation,
    /// nonce, Vault-program, or fixed-window substitution.
    pub fn new(
        planner: &VaultClaimPlanner,
        request: PrepareVaultClaimRequest,
        result: PrepareVaultClaimResult,
        before_state: VaultClaimBeforeState,
        sequencer_window: DiscoveryWindow,
        indexer_window: DiscoveryWindow,
    ) -> Result<Self, VaultClaimEffectPrepareError> {
        planner.validate_prepared(&request, &result)?;
        let owner = AccountId::new(*request.allocation.owner_account_id().as_bytes());
        let vault = vault_core::compute_vault_account_id(programs::vault().id(), owner);
        let owner_id = Hex32::from_bytes(owner.into_value());
        let vault_id = Hex32::from_bytes(vault.into_value());
        let owner_nonce = u128::from(before_state.owner_account.nonce);
        let expected_indexer_start = match before_state.indexer_tip {
            Some(tip) => tip
                .checked_add(1)
                .ok_or(VaultClaimEffectPrepareError::InvalidBeforeState)?,
            None => 0,
        };
        if request.context.sidecar_role != request.allocation.role()
            || request.runtime.sidecar_role != request.allocation.role()
            || request.runtime.signer_account_id != owner_id
            || before_state.owner_account_id != owner_id
            || before_state.vault_account_id != vault_id
            || owner_nonce != request.owner_nonce
            || before_state.vault_account.program_owner != programs::vault().id()
            || before_state.vault_account.balance != request.allocation.amount()
            || sequencer_window.start_height()
                != before_state
                    .sequencer_tip
                    .checked_add(1)
                    .ok_or(VaultClaimEffectPrepareError::InvalidBeforeState)?
            || indexer_window.start_height() != expected_indexer_start
        {
            return Err(VaultClaimEffectPrepareError::InvalidBeforeState);
        }
        let actor_binding = VaultClaimActorBinding {
            run_id: request.context.run_id.clone(),
            role: request.context.sidecar_role,
            runtime: request.runtime.clone(),
            signer_account_id: owner_id,
        };
        let identity = VaultClaimEffectIdentity {
            run_id: request.context.run_id.clone(),
            request_id: request.context.request_id.clone(),
            role: request.context.sidecar_role,
            operation: VaultClaimEffectOperation::ClaimGenesisAllocation,
            scope: VaultClaimEffectScope::VaultOnboarding {
                owner_account_id: owner_id,
                vault_account_id: vault_id,
                allocation: request.allocation.amount(),
            },
            runtime: request.runtime.clone(),
            transaction_id: result.claim.transaction_id,
        };
        Ok(Self {
            actor_binding,
            identity,
            request,
            result,
            before_state,
            sequencer_window,
            indexer_window,
        })
    }

    /// Returns the immutable role-local journal binding.
    pub const fn actor_binding(&self) -> &VaultClaimActorBinding {
        &self.actor_binding
    }

    /// Returns the complete exact effect identity.
    pub const fn identity(&self) -> &VaultClaimEffectIdentity {
        &self.identity
    }

    /// Returns the original typed request.
    pub const fn request(&self) -> &PrepareVaultClaimRequest {
        &self.request
    }

    /// Returns the exact prepared official Claim.
    pub const fn result(&self) -> &PrepareVaultClaimResult {
        &self.result
    }

    /// Returns the typed account snapshots captured before the Claim.
    pub const fn before_state(&self) -> &VaultClaimBeforeState {
        &self.before_state
    }

    /// Returns the fixed sequencer scan bound.
    pub const fn sequencer_window(&self) -> DiscoveryWindow {
        self.sequencer_window
    }

    /// Returns the fixed finalized-indexer scan bound.
    pub const fn indexer_window(&self) -> DiscoveryWindow {
        self.indexer_window
    }

    fn validate_with(
        &self,
        planner: &VaultClaimPlanner,
    ) -> Result<(), VaultClaimEffectPrepareError> {
        let reconstructed = Self::new(
            planner,
            self.request.clone(),
            self.result.clone(),
            self.before_state.clone(),
            self.sequencer_window,
            self.indexer_window,
        )?;
        if &reconstructed == self {
            Ok(())
        } else {
            Err(VaultClaimEffectPrepareError::InvalidPrepared)
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredPreparedVaultClaimEffect {
    actor_binding: VaultClaimActorBinding,
    identity: VaultClaimEffectIdentity,
    request: PrepareVaultClaimRequest,
    result: PrepareVaultClaimResult,
    before_state: VaultClaimBeforeState,
    sequencer_window: DiscoveryWindow,
    indexer_window: DiscoveryWindow,
}

impl From<&PreparedVaultClaimEffect> for StoredPreparedVaultClaimEffect {
    fn from(effect: &PreparedVaultClaimEffect) -> Self {
        Self {
            actor_binding: effect.actor_binding.clone(),
            identity: effect.identity.clone(),
            request: effect.request.clone(),
            result: effect.result.clone(),
            before_state: effect.before_state.clone(),
            sequencer_window: effect.sequencer_window,
            indexer_window: effect.indexer_window,
        }
    }
}

impl StoredPreparedVaultClaimEffect {
    fn into_prepared(
        mut self,
        exact_transaction_bytes: Vec<u8>,
    ) -> Result<PreparedVaultClaimEffect, VaultClaimEffectJournalError> {
        if self.result.claim.exact_bytes.as_slice() != exact_transaction_bytes {
            return Err(VaultClaimEffectJournalError::CorruptState);
        }
        self.result.claim.exact_bytes = ExactTransactionBytes::new(exact_transaction_bytes)
            .map_err(|_| VaultClaimEffectJournalError::CorruptState)?;
        Ok(PreparedVaultClaimEffect {
            actor_binding: self.actor_binding,
            identity: self.identity,
            request: self.request,
            result: self.result,
            before_state: self.before_state,
            sequencer_window: self.sequencer_window,
            indexer_window: self.indexer_window,
        })
    }
}

/// Monotonic evidence phase for one exact Vault Claim.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[must_use]
pub enum VaultClaimEffectState {
    /// Complete typed payload exists but no node call is authorized yet.
    Prepared,
    /// Attempt one was committed before the sole possible node call.
    AttemptStarted,
    /// The sequencer returned the exact official transaction hash.
    Admitted,
    /// The pinned handler definitively rejected invalid parameters pre-enqueue.
    Rejected,
}

impl VaultClaimEffectState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::AttemptStarted => "attempt_started",
            Self::Admitted => "admitted",
            Self::Rejected => "rejected",
        }
    }

    fn parse(value: &str) -> Result<Self, VaultClaimEffectJournalError> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "attempt_started" => Ok(Self::AttemptStarted),
            "admitted" => Ok(Self::Admitted),
            "rejected" => Ok(Self::Rejected),
            _ => Err(VaultClaimEffectJournalError::CorruptState),
        }
    }
}

/// Explicit uncertainty retained without demoting durable evidence.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[must_use]
pub enum VaultClaimSubmissionUncertainty {
    /// Transport or non-InvalidParams RPC outcome did not prove admission.
    AmbiguousRpc,
    /// A successful response returned a hash other than the expected Claim.
    ReturnedHashMismatch,
}

impl VaultClaimSubmissionUncertainty {
    const fn as_str(self) -> &'static str {
        match self {
            Self::AmbiguousRpc => "ambiguous_rpc",
            Self::ReturnedHashMismatch => "returned_hash_mismatch",
        }
    }

    fn parse(value: &str) -> Result<Self, VaultClaimEffectJournalError> {
        match value {
            "ambiguous_rpc" => Ok(Self::AmbiguousRpc),
            "returned_hash_mismatch" => Ok(Self::ReturnedHashMismatch),
            _ => Err(VaultClaimEffectJournalError::CorruptState),
        }
    }
}

/// Complete typed effect plus its current durable submission evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct JournaledVaultClaimEffect {
    effect: PreparedVaultClaimEffect,
    state: VaultClaimEffectState,
    uncertainty: Option<VaultClaimSubmissionUncertainty>,
    attempt_count: u32,
    revision: u64,
}

impl JournaledVaultClaimEffect {
    /// Returns the monotonic evidence phase.
    pub const fn state(&self) -> VaultClaimEffectState {
        self.state
    }

    /// Returns the durable network-attempt count, constrained to zero or one.
    #[must_use]
    pub const fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    /// Returns the compare-and-swap revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns any bounded ambiguity annotation.
    #[must_use]
    pub const fn uncertainty(&self) -> Option<VaultClaimSubmissionUncertainty> {
        self.uncertainty
    }
}

/// Fail-closed journal errors with no state-path disclosure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum VaultClaimEffectJournalError {
    /// The state directory is not the held owner-only non-symlink directory.
    #[error("Vault Claim journal directory is not an owner-only real directory")]
    InsecureDirectory,
    /// The fixed database is not the held owner-only regular single-link inode.
    #[error("Vault Claim journal database is not a private non-aliased regular file")]
    UnsafeDatabaseFile,
    /// This database is already bound to another actor identity.
    #[error("Vault Claim journal actor binding does not match")]
    ActorBindingMismatch,
    /// A different effect or payload already owns this onboarding slot.
    #[error("Vault Claim journal payload conflicts with durable state")]
    Conflict,
    /// The requested effect does not exist in this actor journal.
    #[error("Vault Claim effect is unknown")]
    UnknownEffect,
    /// Durable state is malformed or violates its transition invariants.
    #[error("Vault Claim journal state is corrupt")]
    CorruptState,
    /// A newer schema owns this database.
    #[error("Vault Claim journal uses a future schema")]
    FutureSchema,
    /// A redacted `SQLite` or filesystem operation failed.
    #[error("Vault Claim journal storage operation failed")]
    Storage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DatabaseIdentity {
    device: u64,
    inode: u64,
}

type StoredJournalRow = (String, Vec<u8>, String, Option<String>, i64, i64);

/// Role-bound durable Vault Claim effect store.
pub struct VaultClaimEffectJournal {
    directory: SecureStateDirectory,
    database_identity: DatabaseIdentity,
    actor_binding: VaultClaimActorBinding,
    connection: Mutex<Connection>,
}

impl fmt::Debug for VaultClaimEffectJournal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VaultClaimEffectJournal")
            .field("state", &"[REDACTED]")
            .field("actor_binding", &self.actor_binding)
            .finish_non_exhaustive()
    }
}

impl VaultClaimEffectJournal {
    /// Opens or creates one fixed private `SQLite` journal and binds it to an
    /// immutable actor identity.
    ///
    /// # Errors
    ///
    /// Rejects symlinked/replaced directories, unsafe database inodes, future
    /// schemas, cross-role bindings, and storage failures.
    pub fn open(
        state_directory: impl AsRef<Path>,
        actor_binding: VaultClaimActorBinding,
    ) -> Result<Self, VaultClaimEffectJournalError> {
        let directory =
            SecureStateDirectory::open(state_directory.as_ref()).map_err(map_directory_error)?;
        let (database_identity, creation_guard) = prepare_database_file(&directory)?;
        let database_path = sqlite_database_path(&directory);
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let mut connection = Connection::open_with_flags(&database_path, flags)
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        verify_database_file(&directory, database_identity)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        connection
            .pragma_update(None, "secure_delete", "ON")
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        migrate(&mut connection)?;
        drop(creation_guard);
        verify_database_file(&directory, database_identity)?;
        let journal = Self {
            directory,
            database_identity,
            actor_binding,
            connection: Mutex::new(connection),
        };
        journal.ensure_actor_binding()?;
        journal.revalidate_storage()?;
        Ok(journal)
    }

    /// Persists a complete typed effect exactly once; exact replay is
    /// idempotent and any payload or identity drift conflicts.
    ///
    /// # Errors
    ///
    /// Returns a binding, conflict, corruption, or storage error without
    /// partially installing the effect.
    pub fn record_prepared(
        &self,
        effect: &PreparedVaultClaimEffect,
    ) -> Result<(), VaultClaimEffectJournalError> {
        self.revalidate_storage()?;
        if effect.actor_binding != self.actor_binding {
            return Err(VaultClaimEffectJournalError::ActorBindingMismatch);
        }
        let identity_json = encode_bounded(effect.identity(), MAX_BINDING_JSON_BYTES)?;
        let effect_json = encode_bounded(
            &StoredPreparedVaultClaimEffect::from(effect),
            MAX_TYPED_EFFECT_JSON_BYTES,
        )?;
        let exact_bytes = effect.result.claim.exact_bytes.as_slice();
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        validate_journal_authority(&transaction, &self.actor_binding)?;
        let existing: Option<(String, String, Vec<u8>)> = transaction
            .query_row(
                "SELECT identity_json, effect_json, exact_transaction_bytes
                 FROM vault_claim_effects WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        if let Some(existing) = existing {
            if existing.0 != identity_json || existing.1 != effect_json || existing.2 != exact_bytes
            {
                return Err(VaultClaimEffectJournalError::Conflict);
            }
        } else {
            transaction
                .execute(
                    "INSERT INTO vault_claim_effects (
                         singleton, identity_json, effect_json,
                         exact_transaction_bytes, state, uncertainty,
                         attempt_count, revision
                     ) VALUES (1, ?1, ?2, ?3, 'prepared', NULL, 0, 0)",
                    params![identity_json, effect_json, exact_bytes],
                )
                .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        }
        let persisted: (String, String, Vec<u8>) = transaction
            .query_row(
                "SELECT identity_json, effect_json, exact_transaction_bytes
                 FROM vault_claim_effects WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        if persisted.0 != identity_json || persisted.1 != effect_json || persisted.2 != exact_bytes
        {
            return Err(VaultClaimEffectJournalError::CorruptState);
        }
        transaction
            .commit()
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        drop(connection);
        self.revalidate_storage()
    }

    /// Loads one exact typed effect and its durable evidence.
    ///
    /// # Errors
    ///
    /// Rejects corrupt typed payloads, invalid transition counters, and storage
    /// substitution. An absent exact identity returns `None`.
    pub fn load(
        &self,
        identity: &VaultClaimEffectIdentity,
    ) -> Result<Option<JournaledVaultClaimEffect>, VaultClaimEffectJournalError> {
        self.revalidate_storage()?;
        let identity_json = encode_bounded(identity, MAX_BINDING_JSON_BYTES)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        validate_journal_authority(&transaction, &self.actor_binding)?;
        let row: Option<StoredJournalRow> = transaction
            .query_row(
                "SELECT effect_json, exact_transaction_bytes, state,
                        uncertainty, attempt_count, revision
                 FROM vault_claim_effects WHERE identity_json = ?1",
                [identity_json],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        transaction
            .commit()
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        drop(connection);
        let Some((effect_json, exact_bytes, state, uncertainty, attempts, revision)) = row else {
            self.revalidate_storage()?;
            return Ok(None);
        };
        let stored: StoredPreparedVaultClaimEffect =
            decode_bounded(effect_json.as_bytes(), MAX_TYPED_EFFECT_JSON_BYTES)?;
        if encode_bounded(&stored, MAX_TYPED_EFFECT_JSON_BYTES)? != effect_json {
            return Err(VaultClaimEffectJournalError::CorruptState);
        }
        let effect = stored.into_prepared(exact_bytes)?;
        if effect.identity != *identity || effect.actor_binding != self.actor_binding {
            return Err(VaultClaimEffectJournalError::CorruptState);
        }
        let state = VaultClaimEffectState::parse(&state)?;
        let uncertainty = uncertainty
            .as_deref()
            .map(VaultClaimSubmissionUncertainty::parse)
            .transpose()?;
        let attempt_count =
            u32::try_from(attempts).map_err(|_| VaultClaimEffectJournalError::CorruptState)?;
        let revision =
            u64::try_from(revision).map_err(|_| VaultClaimEffectJournalError::CorruptState)?;
        validate_transition_shape(state, uncertainty, attempt_count, revision)?;
        self.revalidate_storage()?;
        Ok(Some(JournaledVaultClaimEffect {
            effect,
            state,
            uncertainty,
            attempt_count,
            revision,
        }))
    }

    /// Atomically grants send permission to the first caller by committing
    /// `AttemptStarted`, attempt one, and revision one.
    ///
    /// A `false` result means another caller or an earlier process already
    /// consumed send permission; it is not an error and must be observed only.
    ///
    /// # Errors
    ///
    /// Rejects unknown or corrupt state and rolls back on every storage error.
    pub fn begin_attempt(
        &self,
        identity: &VaultClaimEffectIdentity,
    ) -> Result<bool, VaultClaimEffectJournalError> {
        self.revalidate_storage()?;
        let identity_json = encode_bounded(identity, MAX_BINDING_JSON_BYTES)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        validate_journal_authority(&transaction, &self.actor_binding)?;
        let current: Option<(String, Option<String>, i64, i64)> = transaction
            .query_row(
                "SELECT state, uncertainty, attempt_count, revision
                 FROM vault_claim_effects WHERE identity_json = ?1",
                [&identity_json],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        let Some((state, uncertainty, attempts, revision)) = current else {
            return Err(VaultClaimEffectJournalError::UnknownEffect);
        };
        let state = VaultClaimEffectState::parse(&state)?;
        let uncertainty = uncertainty
            .as_deref()
            .map(VaultClaimSubmissionUncertainty::parse)
            .transpose()?;
        let attempts =
            u32::try_from(attempts).map_err(|_| VaultClaimEffectJournalError::CorruptState)?;
        let revision =
            u64::try_from(revision).map_err(|_| VaultClaimEffectJournalError::CorruptState)?;
        validate_transition_shape(state, uncertainty, attempts, revision)?;
        let won = if state == VaultClaimEffectState::Prepared {
            let updated = transaction
                .execute(
                    "UPDATE vault_claim_effects
                     SET state = 'attempt_started', attempt_count = 1, revision = 1
                     WHERE identity_json = ?1 AND state = 'prepared'
                       AND attempt_count = 0 AND revision = 0",
                    [&identity_json],
                )
                .map_err(|_| VaultClaimEffectJournalError::Storage)?;
            if updated != 1 {
                return Err(VaultClaimEffectJournalError::Conflict);
            }
            true
        } else {
            false
        };
        if won {
            let postcondition: (String, Option<String>, i64, i64) = transaction
                .query_row(
                    "SELECT state, uncertainty, attempt_count, revision
                     FROM vault_claim_effects WHERE identity_json = ?1",
                    [&identity_json],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(|_| VaultClaimEffectJournalError::Storage)?;
            if postcondition != ("attempt_started".to_owned(), None, 1, 1) {
                return Err(VaultClaimEffectJournalError::CorruptState);
            }
        }
        transaction
            .commit()
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        drop(connection);
        self.revalidate_storage()?;
        Ok(won)
    }

    fn ensure_actor_binding(&self) -> Result<(), VaultClaimEffectJournalError> {
        self.revalidate_storage()?;
        let binding_json = encode_bounded(&self.actor_binding, MAX_BINDING_JSON_BYTES)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        reject_unexpected_schema_objects(&transaction)?;
        let existing: Option<String> = transaction
            .query_row(
                "SELECT actor_binding_json FROM effect_journal_metadata WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        if let Some(existing) = existing {
            let decoded: VaultClaimActorBinding =
                decode_bounded(existing.as_bytes(), MAX_BINDING_JSON_BYTES)?;
            if decoded != self.actor_binding || existing != binding_json {
                return Err(VaultClaimEffectJournalError::ActorBindingMismatch);
            }
        } else {
            transaction
                .execute(
                    "INSERT INTO effect_journal_metadata (singleton, actor_binding_json)
                     VALUES (1, ?1)",
                    [&binding_json],
                )
                .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        }
        let persisted: String = transaction
            .query_row(
                "SELECT actor_binding_json FROM effect_journal_metadata WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        if persisted != binding_json {
            return Err(VaultClaimEffectJournalError::CorruptState);
        }
        transaction
            .commit()
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        drop(connection);
        self.revalidate_storage()
    }

    fn record_after_attempt(
        &self,
        identity: &VaultClaimEffectIdentity,
        state: VaultClaimEffectState,
        uncertainty: Option<VaultClaimSubmissionUncertainty>,
    ) -> Result<(), VaultClaimEffectJournalError> {
        self.revalidate_storage()?;
        let identity_json = encode_bounded(identity, MAX_BINDING_JSON_BYTES)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        validate_journal_authority(&transaction, &self.actor_binding)?;
        let current: Option<(String, Option<String>, i64, i64)> = transaction
            .query_row(
                "SELECT state, uncertainty, attempt_count, revision
                 FROM vault_claim_effects WHERE identity_json = ?1",
                [&identity_json],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        let Some((current_state, current_uncertainty, attempts, revision)) = current else {
            return Err(VaultClaimEffectJournalError::UnknownEffect);
        };
        let parsed_state = VaultClaimEffectState::parse(&current_state)?;
        let parsed_uncertainty = current_uncertainty
            .as_deref()
            .map(VaultClaimSubmissionUncertainty::parse)
            .transpose()?;
        let attempts =
            u32::try_from(attempts).map_err(|_| VaultClaimEffectJournalError::CorruptState)?;
        let revision =
            u64::try_from(revision).map_err(|_| VaultClaimEffectJournalError::CorruptState)?;
        validate_transition_shape(parsed_state, parsed_uncertainty, attempts, revision)?;
        if parsed_state != VaultClaimEffectState::AttemptStarted
            || attempts != 1
            || revision != 1
            || parsed_uncertainty.is_some()
        {
            return Err(VaultClaimEffectJournalError::Conflict);
        }
        let updated = transaction
            .execute(
                "UPDATE vault_claim_effects
                 SET state = ?2, uncertainty = ?3, revision = revision + 1
                 WHERE identity_json = ?1 AND state = 'attempt_started'
                   AND uncertainty IS NULL AND attempt_count = 1 AND revision = 1",
                params![
                    identity_json,
                    state.as_str(),
                    uncertainty.map(VaultClaimSubmissionUncertainty::as_str)
                ],
            )
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        if updated != 1 {
            return Err(VaultClaimEffectJournalError::Conflict);
        }
        let postcondition: (String, Option<String>, i64, i64) = transaction
            .query_row(
                "SELECT state, uncertainty, attempt_count, revision
                 FROM vault_claim_effects WHERE identity_json = ?1",
                [&identity_json],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        let expected_uncertainty = uncertainty
            .map(VaultClaimSubmissionUncertainty::as_str)
            .map(str::to_owned);
        if postcondition != (state.as_str().to_owned(), expected_uncertainty, 1, 2) {
            return Err(VaultClaimEffectJournalError::CorruptState);
        }
        transaction
            .commit()
            .map_err(|_| VaultClaimEffectJournalError::Storage)?;
        drop(connection);
        self.revalidate_storage()
    }

    fn revalidate_storage(&self) -> Result<(), VaultClaimEffectJournalError> {
        self.directory.revalidate().map_err(map_directory_error)?;
        verify_database_file(&self.directory, self.database_identity)
    }

    fn lock_connection(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, Connection>, VaultClaimEffectJournalError> {
        self.connection
            .lock()
            .map_err(|_| VaultClaimEffectJournalError::Storage)
    }
}

/// Classification of the official sequencer call result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SequencerSendFailure {
    /// JSON-RPC call error `-32602`, known to occur before enqueue.
    DefinitiveInvalidParams,
    /// Every other RPC or transport error, which cannot prove non-enqueue.
    Ambiguous,
}

/// Classifies a real jsonrpsee client error without treating transport or
/// unrelated server failures as definitive rejection.
#[must_use]
pub fn classify_sequencer_send_error(error: &ClientError) -> SequencerSendFailure {
    match error {
        ClientError::Call(object) if object.code() == -32602 => {
            SequencerSendFailure::DefinitiveInvalidParams
        }
        _ => SequencerSendFailure::Ambiguous,
    }
}

/// Narrow official-transaction submission boundary used by the coordinator.
#[async_trait]
pub trait SequencerSubmitApi: Send + Sync {
    /// Calls the official `sendTransaction` method exactly as supplied.
    ///
    /// # Errors
    ///
    /// Returns the unclassified official client error. The coordinator owns
    /// the fail-closed definitive/ambiguous classification.
    async fn send_transaction(&self, transaction: LeeTransaction) -> Result<HashType, ClientError>;
}

/// Result of a submit-or-observe decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum VaultClaimSubmissionOutcome {
    /// The exact expected transaction hash was returned.
    Admitted,
    /// Invalid parameters were definitively rejected before enqueue.
    Rejected,
    /// The first call occurred but its outcome remains unknown.
    Unknown(VaultClaimSubmissionUncertainty),
    /// Send permission was already consumed; callers may only query.
    ObserveOnly {
        /// Strongest monotonic evidence retained so far.
        state: VaultClaimEffectState,
        /// Any bounded reason the outcome remains unresolved.
        uncertainty: Option<VaultClaimSubmissionUncertainty>,
    },
}

/// Fail-closed errors from revalidation or durable submit ordering.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum VaultClaimSubmitError {
    /// Stored request or official transaction data failed complete revalidation.
    #[error("durable Vault Claim effect is invalid")]
    InvalidDurableEffect,
    /// The exact requested effect is absent.
    #[error("durable Vault Claim effect is unknown")]
    UnknownEffect,
    /// The durable effect journal failed closed.
    #[error("durable Vault Claim journal failed: {0}")]
    Journal(VaultClaimEffectJournalError),
}

/// Coordinator that grants at most one official sequencer call per Claim.
pub struct VaultClaimSubmitter<'a> {
    journal: VaultClaimEffectJournal,
    planner: &'a VaultClaimPlanner,
}

impl fmt::Debug for VaultClaimSubmitter<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VaultClaimSubmitter")
            .field("journal", &self.journal)
            .field("planner", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl<'a> VaultClaimSubmitter<'a> {
    /// Binds a role-local journal to the planner that validates recovered bytes.
    ///
    /// # Errors
    ///
    /// This constructor is fallible for forward compatibility; current
    /// validation occurs when an exact effect is loaded.
    pub const fn new(
        journal: VaultClaimEffectJournal,
        planner: &'a VaultClaimPlanner,
    ) -> Result<Self, VaultClaimSubmitError> {
        Ok(Self { journal, planner })
    }

    /// Commits attempt one before the only possible node call, or returns an
    /// observe-only result after any reopen.
    ///
    /// # Errors
    ///
    /// Rejects missing/corrupt effects, official transaction drift, or any
    /// durable transition failure. It never retries a consumed attempt.
    pub async fn submit_or_observe<S: SequencerSubmitApi + ?Sized>(
        &self,
        identity: &VaultClaimEffectIdentity,
        sequencer: &S,
    ) -> Result<VaultClaimSubmissionOutcome, VaultClaimSubmitError> {
        let journaled = self.load_validated(identity)?;
        if journaled.state != VaultClaimEffectState::Prepared {
            return Ok(observe_only(&journaled));
        }
        if !self
            .journal
            .begin_attempt(identity)
            .map_err(VaultClaimSubmitError::Journal)?
        {
            return self
                .load_validated(identity)
                .map(|state| observe_only(&state));
        }

        let public = decode_official_public_transaction(
            journaled.effect.result.claim.exact_bytes.as_slice(),
        )
        .map_err(|_| VaultClaimSubmitError::InvalidDurableEffect)?;
        let response = sequencer
            .send_transaction(LeeTransaction::Public(public))
            .await;
        match response {
            Ok(returned) if returned.0 == *journaled.effect.identity.transaction_id.as_bytes() => {
                self.journal
                    .record_after_attempt(identity, VaultClaimEffectState::Admitted, None)
                    .map_err(VaultClaimSubmitError::Journal)?;
                Ok(VaultClaimSubmissionOutcome::Admitted)
            }
            Ok(_) => {
                let uncertainty = VaultClaimSubmissionUncertainty::ReturnedHashMismatch;
                self.journal
                    .record_after_attempt(
                        identity,
                        VaultClaimEffectState::AttemptStarted,
                        Some(uncertainty),
                    )
                    .map_err(VaultClaimSubmitError::Journal)?;
                Ok(VaultClaimSubmissionOutcome::Unknown(uncertainty))
            }
            Err(error) => match classify_sequencer_send_error(&error) {
                SequencerSendFailure::DefinitiveInvalidParams => {
                    self.journal
                        .record_after_attempt(identity, VaultClaimEffectState::Rejected, None)
                        .map_err(VaultClaimSubmitError::Journal)?;
                    Ok(VaultClaimSubmissionOutcome::Rejected)
                }
                SequencerSendFailure::Ambiguous => {
                    let uncertainty = VaultClaimSubmissionUncertainty::AmbiguousRpc;
                    self.journal
                        .record_after_attempt(
                            identity,
                            VaultClaimEffectState::AttemptStarted,
                            Some(uncertainty),
                        )
                        .map_err(VaultClaimSubmitError::Journal)?;
                    Ok(VaultClaimSubmissionOutcome::Unknown(uncertainty))
                }
            },
        }
    }

    fn load_validated(
        &self,
        identity: &VaultClaimEffectIdentity,
    ) -> Result<JournaledVaultClaimEffect, VaultClaimSubmitError> {
        let journaled = self
            .journal
            .load(identity)
            .map_err(|error| {
                if error == VaultClaimEffectJournalError::CorruptState {
                    VaultClaimSubmitError::InvalidDurableEffect
                } else {
                    VaultClaimSubmitError::Journal(error)
                }
            })?
            .ok_or(VaultClaimSubmitError::UnknownEffect)?;
        journaled
            .effect
            .validate_with(self.planner)
            .map_err(|_| VaultClaimSubmitError::InvalidDurableEffect)?;
        Ok(journaled)
    }
}

fn observe_only(journaled: &JournaledVaultClaimEffect) -> VaultClaimSubmissionOutcome {
    VaultClaimSubmissionOutcome::ObserveOnly {
        state: journaled.state,
        uncertainty: journaled.uncertainty,
    }
}

fn map_directory_error(_: DurableReservationError) -> VaultClaimEffectJournalError {
    VaultClaimEffectJournalError::InsecureDirectory
}

fn sqlite_database_path(directory: &SecureStateDirectory) -> PathBuf {
    directory.path().join(DATABASE_FILENAME)
}

fn prepare_database_file(
    directory: &SecureStateDirectory,
) -> Result<(DatabaseIdentity, File), VaultClaimEffectJournalError> {
    directory.revalidate().map_err(map_directory_error)?;
    let descriptor = match openat(
        directory.descriptor(),
        DATABASE_FILENAME,
        OFlags::RDWR | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(descriptor) => descriptor,
        Err(Errno::NOENT) => openat(
            directory.descriptor(),
            DATABASE_FILENAME,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| VaultClaimEffectJournalError::Storage)?,
        Err(Errno::LOOP) => return Err(VaultClaimEffectJournalError::UnsafeDatabaseFile),
        Err(_) => return Err(VaultClaimEffectJournalError::Storage),
    };
    let file = File::from(descriptor);
    let identity = validate_database_file(&file)?;
    file.sync_all()
        .map_err(|_| VaultClaimEffectJournalError::Storage)?;
    directory
        .descriptor()
        .sync_all()
        .map_err(|_| VaultClaimEffectJournalError::Storage)?;
    directory.revalidate().map_err(map_directory_error)?;
    Ok((identity, file))
}

fn verify_database_file(
    directory: &SecureStateDirectory,
    expected: DatabaseIdentity,
) -> Result<(), VaultClaimEffectJournalError> {
    directory.revalidate().map_err(map_directory_error)?;
    let descriptor = openat(
        directory.descriptor(),
        DATABASE_FILENAME,
        OFlags::RDWR | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| {
        if error == Errno::LOOP || error == Errno::NOENT {
            VaultClaimEffectJournalError::UnsafeDatabaseFile
        } else {
            VaultClaimEffectJournalError::Storage
        }
    })?;
    let file = File::from(descriptor);
    let current = validate_database_file(&file)?;
    if current != expected {
        return Err(VaultClaimEffectJournalError::UnsafeDatabaseFile);
    }
    directory.revalidate().map_err(map_directory_error)
}

fn validate_database_file(file: &File) -> Result<DatabaseIdentity, VaultClaimEffectJournalError> {
    let metadata = file
        .metadata()
        .map_err(|_| VaultClaimEffectJournalError::UnsafeDatabaseFile)?;
    if !metadata.file_type().is_file()
        || metadata.uid() != geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(VaultClaimEffectJournalError::UnsafeDatabaseFile);
    }
    Ok(DatabaseIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn migrate(connection: &mut Connection) -> Result<(), VaultClaimEffectJournalError> {
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|_| VaultClaimEffectJournalError::Storage)?;
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| VaultClaimEffectJournalError::Storage)?;
    match version {
        DATABASE_SCHEMA_VERSION if application_id == DATABASE_APPLICATION_ID => return Ok(()),
        version if version > DATABASE_SCHEMA_VERSION => {
            return Err(VaultClaimEffectJournalError::FutureSchema);
        }
        0 if application_id == 0 => {}
        _ => return Err(VaultClaimEffectJournalError::CorruptState),
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| VaultClaimEffectJournalError::Storage)?;
    transaction
        .execute_batch(
            "CREATE TABLE effect_journal_metadata (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 actor_binding_json TEXT NOT NULL
                     CHECK (length(actor_binding_json) BETWEEN 1 AND 65536)
             );
             CREATE TABLE vault_claim_effects (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 identity_json TEXT NOT NULL UNIQUE
                     CHECK (length(identity_json) BETWEEN 1 AND 65536),
                 effect_json TEXT NOT NULL
                     CHECK (length(effect_json) BETWEEN 1 AND 8388608),
                 exact_transaction_bytes BLOB NOT NULL
                     CHECK (length(exact_transaction_bytes) BETWEEN 1 AND 2000000),
                 state TEXT NOT NULL
                     CHECK (state IN ('prepared', 'attempt_started', 'admitted', 'rejected')),
                 uncertainty TEXT
                     CHECK (uncertainty IS NULL OR uncertainty IN
                         ('ambiguous_rpc', 'returned_hash_mismatch')),
                 attempt_count INTEGER NOT NULL
                     CHECK (attempt_count BETWEEN 0 AND 1),
                 revision INTEGER NOT NULL CHECK (revision >= 0)
             );
             PRAGMA application_id = 1280988739;
             PRAGMA user_version = 1;",
        )
        .map_err(|_| VaultClaimEffectJournalError::Storage)?;
    transaction
        .commit()
        .map_err(|_| VaultClaimEffectJournalError::Storage)
}

fn reject_unexpected_schema_objects(
    transaction: &Transaction<'_>,
) -> Result<(), VaultClaimEffectJournalError> {
    let unexpected: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%'
               AND NOT (type = 'table' AND name IN
                   ('effect_journal_metadata', 'vault_claim_effects'))",
            [],
            |row| row.get(0),
        )
        .map_err(|_| VaultClaimEffectJournalError::Storage)?;
    let required_tables: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name IN
                 ('effect_journal_metadata', 'vault_claim_effects')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| VaultClaimEffectJournalError::Storage)?;
    let application_id: i64 = transaction
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|_| VaultClaimEffectJournalError::Storage)?;
    let version: i64 = transaction
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| VaultClaimEffectJournalError::Storage)?;
    if unexpected == 0
        && required_tables == 2
        && application_id == DATABASE_APPLICATION_ID
        && version == DATABASE_SCHEMA_VERSION
    {
        Ok(())
    } else {
        Err(VaultClaimEffectJournalError::CorruptState)
    }
}

fn validate_journal_authority(
    transaction: &Transaction<'_>,
    expected_binding: &VaultClaimActorBinding,
) -> Result<(), VaultClaimEffectJournalError> {
    reject_unexpected_schema_objects(transaction)?;
    let encoded: String = transaction
        .query_row(
            "SELECT actor_binding_json FROM effect_journal_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|_| VaultClaimEffectJournalError::CorruptState)?;
    let decoded: VaultClaimActorBinding =
        decode_bounded(encoded.as_bytes(), MAX_BINDING_JSON_BYTES)?;
    if decoded == *expected_binding && encode_bounded(&decoded, MAX_BINDING_JSON_BYTES)? == encoded
    {
        Ok(())
    } else {
        Err(VaultClaimEffectJournalError::ActorBindingMismatch)
    }
}

fn encode_bounded<T: Serialize>(
    value: &T,
    maximum: usize,
) -> Result<String, VaultClaimEffectJournalError> {
    let encoded =
        serde_json::to_string(value).map_err(|_| VaultClaimEffectJournalError::CorruptState)?;
    if encoded.is_empty() || encoded.len() > maximum {
        return Err(VaultClaimEffectJournalError::CorruptState);
    }
    Ok(encoded)
}

fn decode_bounded<T: for<'de> Deserialize<'de>>(
    encoded: &[u8],
    maximum: usize,
) -> Result<T, VaultClaimEffectJournalError> {
    if encoded.is_empty() || encoded.len() > maximum {
        return Err(VaultClaimEffectJournalError::CorruptState);
    }
    serde_json::from_slice(encoded).map_err(|_| VaultClaimEffectJournalError::CorruptState)
}

fn validate_transition_shape(
    state: VaultClaimEffectState,
    uncertainty: Option<VaultClaimSubmissionUncertainty>,
    attempts: u32,
    revision: u64,
) -> Result<(), VaultClaimEffectJournalError> {
    let valid = match state {
        VaultClaimEffectState::Prepared => attempts == 0 && revision == 0 && uncertainty.is_none(),
        VaultClaimEffectState::AttemptStarted => match uncertainty {
            None => attempts == 1 && revision == 1,
            Some(_) => attempts == 1 && revision == 2,
        },
        VaultClaimEffectState::Admitted | VaultClaimEffectState::Rejected => {
            attempts == 1 && revision == 2 && uncertainty.is_none()
        }
    };
    if valid {
        Ok(())
    } else {
        Err(VaultClaimEffectJournalError::CorruptState)
    }
}
