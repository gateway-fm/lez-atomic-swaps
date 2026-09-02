//! Standalone owner-private persistence for the Taker facade.
//!
//! This first schema stores only initiation admission and public projections.
//! It deliberately contains no worker, actor, Chat, chain, or action authority.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, SwapId};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension as _, Transaction, TransactionBehavior, params,
};
use rustix::fs::{Mode, OFlags};
use secp256k1::PublicKey;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{MakerOfferId, MakerRouteV1, is_owner_private_regular_file, open_no_symlinks};

const APPLICATION_ID: i64 = 0x4c54_4652;
const SCHEMA_VERSION: i64 = 1;
const PAYLOAD_VERSION: i64 = 1;
const MAX_SOURCE_ID_BYTES: usize = 128;
const MAX_PATH_BYTES: usize = 4_096;

const CREATE_SCHEMA: &str = "
CREATE TABLE taker_facade_swaps (
    swap_id         TEXT PRIMARY KEY NOT NULL,
    payload_version INTEGER NOT NULL CHECK (payload_version = 1),
    public_json     TEXT NOT NULL,
    created_at      INTEGER NOT NULL CHECK (created_at >= 0)
) STRICT;
CREATE TABLE taker_facade_authorities (
    swap_id         TEXT PRIMARY KEY NOT NULL
                        REFERENCES taker_facade_swaps(swap_id) ON DELETE CASCADE,
    payload_version INTEGER NOT NULL CHECK (payload_version = 1),
    private_json    TEXT NOT NULL
) STRICT;
CREATE TABLE taker_facade_requests (
    request_id             TEXT PRIMARY KEY NOT NULL,
    operation              TEXT NOT NULL CHECK (operation IN ('initiate', 'claim', 'refund')),
    swap_id                TEXT NOT NULL REFERENCES taker_facade_swaps(swap_id) ON DELETE CASCADE,
    request_payload_version INTEGER NOT NULL CHECK (request_payload_version = 1),
    request_json           TEXT NOT NULL,
    result_payload_version INTEGER NOT NULL CHECK (result_payload_version = 1),
    result_json            TEXT NOT NULL,
    state                  TEXT NOT NULL CHECK (state = 'admitted'),
    created_at             INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at             INTEGER NOT NULL CHECK (updated_at >= created_at)
) STRICT;
";

/// Fixed, path-free failures from the standalone Taker facade registry.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum TakerFacadeStoreError {
    /// The registry could not be created or opened.
    #[error("Taker registry storage is unavailable")]
    DatabaseUnavailable,
    /// The registry inode, ownership, mode, or link count changed.
    #[error("Taker registry storage identity is unsafe")]
    UnsafeDatabaseFile,
    /// Exclusive creation found an existing valid registry.
    #[error("Taker registry already exists")]
    DatabaseAlreadyExists,
    /// The database belongs to another application or schema.
    #[error("Taker registry schema is foreign")]
    ForeignSchema,
    /// The database was written by a newer implementation.
    #[error("Taker registry schema is newer than supported")]
    FutureSchema,
    /// Durable rows or constraints do not revalidate.
    #[error("Taker registry state is corrupt")]
    CorruptState,
    /// Typed initiation facts or private authority are invalid.
    #[error("Taker registry input is invalid")]
    InvalidInput,
    /// A request identity is already bound to another operation or payload.
    #[error("Taker registry request conflicts with durable state")]
    RequestConflict,
    /// A swap identity is already bound to another request.
    #[error("Taker registry swap conflicts with durable state")]
    SwapConflict,
    /// The requested action has no durable parent swap.
    #[error("Taker registry swap is unavailable")]
    SwapUnavailable,
    /// Another irreversible terminal action is already bound to this swap.
    #[error("Taker registry terminal action conflicts with durable state")]
    ActionGenerationConflict,
    /// A bounded `SQLite` or serialization operation failed.
    #[error("Taker registry operation is unavailable")]
    StorageUnavailable,
}

/// Public immutable facts admitted for one Taker initiation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerInitiationFactsV1 {
    schema_version: u16,
    swap_id: SwapId,
    offer_id: MakerOfferId,
    route: MakerRouteV1,
    #[serde(with = "maker_identity_serde")]
    maker_identity: [u8; 33],
    signed_envelope_sha256: [u8; 32],
    foreign_units: u64,
    lez_units: u128,
}

impl TakerInitiationFactsV1 {
    /// Constructs exact public facts for either role-correct ZEC Taker direction.
    ///
    /// # Errors
    ///
    /// Rejects another pair, a non-compressed identity, or zero amounts.
    pub fn new(
        swap_id: SwapId,
        offer_id: MakerOfferId,
        route: MakerRouteV1,
        maker_identity: [u8; 33],
        signed_envelope_sha256: [u8; 32],
        foreign_units: u64,
        lez_units: u128,
    ) -> Result<Self, TakerFacadeStoreError> {
        let value = Self {
            schema_version: 1,
            swap_id,
            offer_id,
            route,
            maker_identity,
            signed_envelope_sha256,
            foreign_units,
            lez_units,
        };
        value.validate()?;
        Ok(value)
    }

    /// Stable application swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Immutable authenticated offer identity.
    #[must_use]
    pub const fn offer_id(&self) -> &MakerOfferId {
        &self.offer_id
    }

    /// Exact pair and role-fixed direction.
    #[must_use]
    pub const fn route(&self) -> MakerRouteV1 {
        self.route
    }

    /// Pinned compressed Maker identity.
    #[must_use]
    pub const fn maker_identity(&self) -> &[u8; 33] {
        &self.maker_identity
    }

    /// Exact signed Delivery envelope commitment.
    #[must_use]
    pub const fn signed_envelope_sha256(&self) -> &[u8; 32] {
        &self.signed_envelope_sha256
    }

    /// Selected foreign-chain atomic units.
    #[must_use]
    pub const fn foreign_units(&self) -> u64 {
        self.foreign_units
    }

    /// Exact quoted LEZ atomic units.
    #[must_use]
    pub const fn lez_units(&self) -> u128 {
        self.lez_units
    }

    fn validate(&self) -> Result<(), TakerFacadeStoreError> {
        if self.schema_version != 1
            || self.route.pair() != Pair::Zcash
            || PublicKey::from_slice(&self.maker_identity).is_err()
            || self.foreign_units == 0
            || self.lez_units == 0
        {
            return Err(TakerFacadeStoreError::InvalidInput);
        }
        Ok(())
    }
}

/// Identified owner-private input selected by trusted service configuration.
#[derive(Clone, Eq, PartialEq)]
pub struct TakerPrivateFileBindingV1 {
    path: PathBuf,
    sha256: Option<[u8; 32]>,
    device: u64,
    inode: u64,
}

impl std::fmt::Debug for TakerPrivateFileBindingV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TakerPrivateFileBindingV1")
            .finish_non_exhaustive()
    }
}

impl TakerPrivateFileBindingV1 {
    /// Binds one immutable private input by normalized path, digest, and inode.
    ///
    /// # Errors
    ///
    /// Rejects an unsafe path or zero inode.
    pub fn immutable(
        path: PathBuf,
        sha256: [u8; 32],
        device: u64,
        inode: u64,
    ) -> Result<Self, TakerFacadeStoreError> {
        Self::new(path, Some(sha256), device, inode)
    }

    /// Binds one secret input by normalized path and inode without persisting a verifier.
    ///
    /// # Errors
    ///
    /// Rejects an unsafe path or zero inode.
    pub fn secret(path: PathBuf, device: u64, inode: u64) -> Result<Self, TakerFacadeStoreError> {
        Self::new(path, None, device, inode)
    }

    fn new(
        path: PathBuf,
        sha256: Option<[u8; 32]>,
        device: u64,
        inode: u64,
    ) -> Result<Self, TakerFacadeStoreError> {
        validate_authority_path(&path)?;
        if inode == 0 {
            return Err(TakerFacadeStoreError::InvalidInput);
        }
        Ok(Self {
            path,
            sha256,
            device,
            inode,
        })
    }
}

/// Complete private, service-derived authority for a future initiation worker.
#[derive(Clone, Eq, PartialEq)]
pub struct TakerInitiationAuthorityV1 {
    maker_source_id: Box<str>,
    reservation_id: RequestId,
    signed_envelope: TakerPrivateFileBindingV1,
    unsigned_draft: TakerPrivateFileBindingV1,
    signing_key: TakerPrivateFileBindingV1,
    source_config: TakerPrivateFileBindingV1,
    agreement_output: PathBuf,
    actor_root: PathBuf,
    receipt_output: PathBuf,
}

impl std::fmt::Debug for TakerInitiationAuthorityV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TakerInitiationAuthorityV1")
            .finish_non_exhaustive()
    }
}

impl TakerInitiationAuthorityV1 {
    /// Constructs one exact private authority without exposing it through getters.
    ///
    /// # Errors
    ///
    /// Rejects an unsafe source identity, aliased paths, or a secret binding
    /// that unexpectedly carries a persisted digest.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        maker_source_id: impl Into<Box<str>>,
        reservation_id: RequestId,
        signed_envelope: TakerPrivateFileBindingV1,
        unsigned_draft: TakerPrivateFileBindingV1,
        signing_key: TakerPrivateFileBindingV1,
        source_config: TakerPrivateFileBindingV1,
        agreement_output: PathBuf,
        actor_root: PathBuf,
        receipt_output: PathBuf,
    ) -> Result<Self, TakerFacadeStoreError> {
        let value = Self {
            maker_source_id: maker_source_id.into(),
            reservation_id,
            signed_envelope,
            unsigned_draft,
            signing_key,
            source_config,
            agreement_output,
            actor_root,
            receipt_output,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), TakerFacadeStoreError> {
        if self.maker_source_id.is_empty()
            || self.maker_source_id.len() > MAX_SOURCE_ID_BYTES
            || !self
                .maker_source_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            || self.signed_envelope.sha256.is_none()
            || self.unsigned_draft.sha256.is_none()
            || self.signing_key.sha256.is_some()
            || self.source_config.sha256.is_none()
        {
            return Err(TakerFacadeStoreError::InvalidInput);
        }
        for path in [
            &self.agreement_output,
            &self.actor_root,
            &self.receipt_output,
        ] {
            validate_authority_path(path)?;
        }
        let paths = [
            self.signed_envelope.path.as_path(),
            self.unsigned_draft.path.as_path(),
            self.signing_key.path.as_path(),
            self.source_config.path.as_path(),
            self.agreement_output.as_path(),
            self.actor_root.as_path(),
            self.receipt_output.as_path(),
        ];
        let mut unique = BTreeSet::new();
        if paths.iter().any(|path| !unique.insert(*path)) {
            return Err(TakerFacadeStoreError::InvalidInput);
        }
        Ok(())
    }

    fn stored(&self) -> StoredTakerInitiationAuthorityV1 {
        StoredTakerInitiationAuthorityV1 {
            schema_version: 1,
            maker_source_id: self.maker_source_id.clone(),
            reservation_id: self.reservation_id.clone(),
            signed_envelope: self.signed_envelope.stored(),
            unsigned_draft: self.unsigned_draft.stored(),
            signing_key: self.signing_key.stored(),
            source_config: self.source_config.stored(),
            agreement_output: self.agreement_output.clone(),
            actor_root: self.actor_root.clone(),
            receipt_output: self.receipt_output.clone(),
        }
    }
}

impl TakerPrivateFileBindingV1 {
    fn stored(&self) -> StoredTakerPrivateFileBindingV1 {
        StoredTakerPrivateFileBindingV1 {
            path: self.path.clone(),
            sha256: self.sha256,
            device: self.device,
            inode: self.inode,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredTakerPrivateFileBindingV1 {
    path: PathBuf,
    sha256: Option<[u8; 32]>,
    device: u64,
    inode: u64,
}

impl StoredTakerPrivateFileBindingV1 {
    fn validate(&self) -> Result<(), TakerFacadeStoreError> {
        validate_authority_path(&self.path)?;
        if self.inode == 0 {
            return Err(TakerFacadeStoreError::CorruptState);
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredTakerInitiationAuthorityV1 {
    schema_version: u16,
    maker_source_id: Box<str>,
    reservation_id: RequestId,
    signed_envelope: StoredTakerPrivateFileBindingV1,
    unsigned_draft: StoredTakerPrivateFileBindingV1,
    signing_key: StoredTakerPrivateFileBindingV1,
    source_config: StoredTakerPrivateFileBindingV1,
    agreement_output: PathBuf,
    actor_root: PathBuf,
    receipt_output: PathBuf,
}

impl StoredTakerInitiationAuthorityV1 {
    fn validate(&self) -> Result<(), TakerFacadeStoreError> {
        self.signed_envelope.validate()?;
        self.unsigned_draft.validate()?;
        self.signing_key.validate()?;
        self.source_config.validate()?;
        let value = TakerInitiationAuthorityV1 {
            maker_source_id: self.maker_source_id.clone(),
            reservation_id: self.reservation_id.clone(),
            signed_envelope: self.signed_envelope.as_public(),
            unsigned_draft: self.unsigned_draft.as_public(),
            signing_key: self.signing_key.as_public(),
            source_config: self.source_config.as_public(),
            agreement_output: self.agreement_output.clone(),
            actor_root: self.actor_root.clone(),
            receipt_output: self.receipt_output.clone(),
        };
        if self.schema_version != 1 {
            return Err(TakerFacadeStoreError::CorruptState);
        }
        value
            .validate()
            .map_err(|_| TakerFacadeStoreError::CorruptState)
    }
}

impl StoredTakerPrivateFileBindingV1 {
    fn as_public(&self) -> TakerPrivateFileBindingV1 {
        TakerPrivateFileBindingV1 {
            path: self.path.clone(),
            sha256: self.sha256,
            device: self.device,
            inode: self.inode,
        }
    }
}

/// Method-fixed action retained by the owner-private registry.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TakerFacadeActionV1 {
    /// Agreement-ordered claim progression.
    Claim,
    /// Agreement-ordered timeout recovery.
    Refund,
}

impl TakerFacadeActionV1 {
    const fn name(self) -> &'static str {
        match self {
            Self::Claim => "claim",
            Self::Refund => "refund",
        }
    }

    fn parse(value: &str) -> Result<Self, TakerFacadeStoreError> {
        match value {
            "claim" => Ok(Self::Claim),
            "refund" => Ok(Self::Refund),
            _ => Err(TakerFacadeStoreError::CorruptState),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredTakerActionRequestV1 {
    schema_version: u16,
    swap_id: SwapId,
    expected_generation: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredTakerActionResultV1 {
    schema_version: u16,
    swap_id: SwapId,
    action: TakerFacadeActionV1,
    requested_after_generation: u64,
}

/// Durable result of one generation-fenced action admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TakerActionAdmissionV1 {
    swap_id: SwapId,
    action: TakerFacadeActionV1,
    requested_after_generation: u64,
    was_replay: bool,
}

impl TakerActionAdmissionV1 {
    /// Stable application swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Exact method-fixed terminal action.
    #[must_use]
    pub const fn action(&self) -> TakerFacadeActionV1 {
        self.action
    }

    /// Actor progress generation observed at original admission.
    #[must_use]
    pub const fn requested_after_generation(&self) -> u64 {
        self.requested_after_generation
    }

    /// Whether the exact immutable request was already durable.
    #[must_use]
    pub const fn was_replay(&self) -> bool {
        self.was_replay
    }
}

/// Durable public result of one initiation admission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakerInitiationAdmissionV1 {
    schema_version: u16,
    facts: TakerInitiationFactsV1,
    was_replay: bool,
}

impl TakerInitiationAdmissionV1 {
    /// Exact admitted public facts.
    #[must_use]
    pub const fn facts(&self) -> &TakerInitiationFactsV1 {
        &self.facts
    }

    /// Whether the exact request and private authority were already durable.
    #[must_use]
    pub const fn was_replay(&self) -> bool {
        self.was_replay
    }
}

/// Standalone `SQLite` schema-v1 Taker facade registry.
pub struct SqliteTakerFacadeStore {
    connection: Connection,
    path: PathBuf,
    identity: FileIdentity,
}

impl std::fmt::Debug for SqliteTakerFacadeStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteTakerFacadeStore")
            .finish_non_exhaustive()
    }
}

impl SqliteTakerFacadeStore {
    /// Exclusively creates a new owner-private empty registry.
    ///
    /// # Errors
    ///
    /// Rejects existing, unsafe, foreign, or unavailable storage.
    pub fn create_new(path: impl AsRef<Path>) -> Result<Self, TakerFacadeStoreError> {
        let path = path.as_ref();
        validate_database_path(path)?;
        let guard = match open_no_symlinks(
            path,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(file) => file,
            Err(error) if error == rustix::io::Errno::EXIST => {
                classify_existing(path)?;
                return Err(TakerFacadeStoreError::DatabaseAlreadyExists);
            }
            Err(error) if error == rustix::io::Errno::LOOP => {
                return Err(TakerFacadeStoreError::UnsafeDatabaseFile);
            }
            Err(_) => return Err(TakerFacadeStoreError::DatabaseUnavailable),
        };
        let identity = validate_database_file(&guard)?;
        guard
            .sync_all()
            .map_err(|_| TakerFacadeStoreError::DatabaseUnavailable)?;
        sync_parent(path)?;
        let mut store = Self::open_connection(path, identity)?;
        initialize_schema(&mut store.connection)?;
        validate_connection(&store.connection)?;
        drop(guard);
        store.revalidate_storage()?;
        Ok(store)
    }

    /// Opens an existing exact schema-v1 registry without migrating it.
    ///
    /// # Errors
    ///
    /// Missing, unsafe, foreign, future, or corrupt registries fail closed.
    pub fn open_existing(path: impl AsRef<Path>) -> Result<Self, TakerFacadeStoreError> {
        let path = path.as_ref();
        validate_database_path(path)?;
        let file = open_checked(path)?;
        let identity = validate_database_file(&file)?;
        let store = Self::open_connection(path, identity)?;
        validate_connection(&store.connection)?;
        validate_all_records(&store.connection)?;
        drop(file);
        store.revalidate_storage()?;
        Ok(store)
    }

    /// Finds the exact public facts durably bound to one initiation request.
    ///
    /// This lookup is intended to run before live offer or trusted-time checks,
    /// so an exact retry remains replayable after the original Delivery offer
    /// expires or disappears. It never returns the stored private authority.
    ///
    /// # Errors
    ///
    /// Rejects changed storage identity or any malformed, incomplete, or
    /// inconsistently bound request, public projection, or private authority.
    pub fn lookup_initiation(
        &self,
        request_id: &RequestId,
    ) -> Result<Option<TakerInitiationFactsV1>, TakerFacadeStoreError> {
        self.revalidate_storage()?;
        let row = self
            .connection
            .query_row(
                "SELECT operation, swap_id, request_payload_version, request_json,
                        result_payload_version, result_json, state
                 FROM taker_facade_requests WHERE request_id = ?1",
                [request_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        let Some((
            operation,
            swap_id,
            request_version,
            request_json,
            result_version,
            result_json,
            state,
        )) = row
        else {
            self.revalidate_storage()?;
            return Ok(None);
        };
        if operation != "initiate" {
            let raw = query_action_by_request(&self.connection, request_id)?
                .ok_or(TakerFacadeStoreError::CorruptState)?;
            let action = validate_action_row(raw)?;
            validate_action_parent(&self.connection, &action.swap_id)?;
            let rows = load_valid_action_rows(&self.connection, &action.swap_id)?;
            if !rows.iter().any(|row| row.request_id == *request_id) {
                return Err(TakerFacadeStoreError::CorruptState);
            }
            self.revalidate_storage()?;
            return Ok(None);
        }
        let (public_version, public_json): (i64, String) = self
            .connection
            .query_row(
                "SELECT payload_version, public_json FROM taker_facade_swaps WHERE swap_id = ?1",
                [&swap_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| TakerFacadeStoreError::CorruptState)?;
        let (authority_version, authority_json): (i64, String) = self
            .connection
            .query_row(
                "SELECT payload_version, private_json FROM taker_facade_authorities
                 WHERE swap_id = ?1",
                [&swap_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| TakerFacadeStoreError::CorruptState)?;
        let facts: TakerInitiationFactsV1 = decode(&public_json)?;
        let authority: StoredTakerInitiationAuthorityV1 = decode(&authority_json)?;
        if operation != "initiate"
            || request_version != PAYLOAD_VERSION
            || result_version != PAYLOAD_VERSION
            || public_version != PAYLOAD_VERSION
            || authority_version != PAYLOAD_VERSION
            || state != "admitted"
            || request_json != public_json
            || result_json != public_json
            || facts.swap_id.as_str() != swap_id
        {
            return Err(TakerFacadeStoreError::CorruptState);
        }
        facts
            .validate()
            .map_err(|_| TakerFacadeStoreError::CorruptState)?;
        authority.validate()?;
        self.revalidate_storage()?;
        Ok(Some(facts))
    }

    /// Resolves one admitted swap only when it is still bound to the exact
    /// owner-private authority supplied by trusted service configuration.
    ///
    /// The private authority is used solely as a comparison key and is never
    /// returned. Only an unknown swap returns `None`; authority drift fails
    /// closed as a durable swap conflict.
    ///
    /// # Errors
    ///
    /// Rejects an invalid supplied authority, changed storage identity, or any
    /// malformed, incomplete, duplicated, or inconsistently joined durable row.
    pub fn lookup_initiation_for_monitor(
        &self,
        swap_id: &SwapId,
        authority: &TakerInitiationAuthorityV1,
    ) -> Result<Option<TakerInitiationFactsV1>, TakerFacadeStoreError> {
        authority.validate()?;
        let expected_authority = encode(&authority.stored())?;
        self.revalidate_storage()?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        let mut rows = query_monitor_initiation(&transaction, swap_id)?;
        let row = match rows.len() {
            0 => {
                let related_rows = related_monitor_row_count(&transaction, swap_id)?;
                if related_rows == 0 {
                    transaction
                        .commit()
                        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
                    self.revalidate_storage()?;
                    return Ok(None);
                }
                return Err(TakerFacadeStoreError::CorruptState);
            }
            1 => rows.pop().ok_or(TakerFacadeStoreError::CorruptState)?,
            _ => return Err(TakerFacadeStoreError::CorruptState),
        };
        let (facts, stored_authority) = validate_monitor_initiation(row, swap_id)?;
        transaction
            .commit()
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        self.revalidate_storage()?;
        if stored_authority != expected_authority {
            return Err(TakerFacadeStoreError::SwapConflict);
        }
        Ok(Some(facts))
    }

    /// Returns the original trusted admission timestamp for one initiation request.
    ///
    /// This timestamp is immutable and lets a restart revalidate already-accepted
    /// agreement bytes at their original acceptance time rather than at the
    /// current wall clock. Call this only after `lookup_initiation` returned facts.
    ///
    /// # Errors
    ///
    /// Rejects changed storage identity, malformed time, or unavailable storage.
    pub fn lookup_initiation_admitted_at(
        &self,
        request_id: &RequestId,
    ) -> Result<Option<u64>, TakerFacadeStoreError> {
        self.revalidate_storage()?;
        let admitted_at = self
            .connection
            .query_row(
                "SELECT created_at FROM taker_facade_requests
                 WHERE request_id = ?1 AND operation = 'initiate' AND state = 'admitted'",
                [request_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?
            .map(|value| u64::try_from(value).map_err(|_| TakerFacadeStoreError::CorruptState))
            .transpose()?;
        self.revalidate_storage()?;
        Ok(admitted_at)
    }

    /// Finds an exact durable action request without consulting current actor progress.
    ///
    /// Call this while holding the actor lock before applying a freshness check,
    /// so a retry remains replayable after its original effect advanced progress.
    ///
    /// # Errors
    ///
    /// Rejects request-ID reuse, malformed durable state, or unsafe storage.
    pub fn lookup_exact_action(
        &self,
        request_id: &RequestId,
        swap_id: &SwapId,
        action: TakerFacadeActionV1,
        expected_generation: u64,
    ) -> Result<Option<TakerActionAdmissionV1>, TakerFacadeStoreError> {
        self.revalidate_storage()?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        let result = lookup_exact_action_request(
            &transaction,
            request_id,
            swap_id,
            action,
            expected_generation,
        )?;
        transaction
            .commit()
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        self.revalidate_storage()?;
        Ok(result)
    }

    /// Returns the sole durable terminal authorization for one swap.
    ///
    /// This read is suitable for overlaying an in-progress authorization onto a
    /// receipt-bound monitor projection. It returns no row for an unknown swap or
    /// for an admitted swap that has no terminal authorization.
    ///
    /// # Errors
    ///
    /// Rejects malformed parent state, multiple terminal authorizations, changed
    /// storage identity, or unavailable storage.
    pub fn lookup_action_for_swap(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<TakerActionAdmissionV1>, TakerFacadeStoreError> {
        self.revalidate_storage()?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        match validate_action_parent(&transaction, swap_id) {
            Err(TakerFacadeStoreError::SwapUnavailable) => {
                transaction
                    .commit()
                    .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
                self.revalidate_storage()?;
                return Ok(None);
            }
            result => result?,
        }
        let result = load_valid_action_rows(&transaction, swap_id)?
            .into_iter()
            .next()
            .map(|stored| TakerActionAdmissionV1 {
                swap_id: stored.swap_id,
                action: stored.action,
                requested_after_generation: stored.expected_generation,
                was_replay: true,
            });
        transaction
            .commit()
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        self.revalidate_storage()?;
        Ok(result)
    }

    /// Atomically admits one generation-fenced action or exactly replays it.
    ///
    /// The transaction commits before any caller performs an effect. A caller
    /// must validate the current actor action and generation under its actor lock
    /// before admitting a new request, and retain that lock through the effect.
    ///
    /// # Errors
    ///
    /// Rejects request reuse, an unknown parent swap, an existing authorization,
    /// corrupt durable state, an invalid timestamp, or unavailable storage.
    pub fn admit_action(
        &mut self,
        request_id: &RequestId,
        swap_id: &SwapId,
        action: TakerFacadeActionV1,
        expected_generation: u64,
        now: u64,
    ) -> Result<TakerActionAdmissionV1, TakerFacadeStoreError> {
        let request_json = encode(&StoredTakerActionRequestV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
            expected_generation,
        })?;
        let result_json = encode(&StoredTakerActionResultV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
            action,
            requested_after_generation: expected_generation,
        })?;
        self.revalidate_storage()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        if let Some(replay) = lookup_exact_action_request(
            &transaction,
            request_id,
            swap_id,
            action,
            expected_generation,
        )? {
            transaction
                .commit()
                .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
            self.revalidate_storage()?;
            return Ok(replay);
        }
        validate_action_parent(&transaction, swap_id)?;
        let actions = load_valid_action_rows(&transaction, swap_id)?;
        if !actions.is_empty() {
            return Err(TakerFacadeStoreError::ActionGenerationConflict);
        }
        let now = i64::try_from(now).map_err(|_| TakerFacadeStoreError::InvalidInput)?;
        transaction
            .execute(
                "INSERT INTO taker_facade_requests (
                     request_id, operation, swap_id, request_payload_version, request_json,
                     result_payload_version, result_json, state, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, 1, ?4, 1, ?5, 'admitted', ?6, ?6)",
                params![
                    request_id.as_str(),
                    action.name(),
                    swap_id.as_str(),
                    request_json,
                    result_json,
                    now
                ],
            )
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        transaction
            .commit()
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        self.revalidate_storage()?;
        Ok(TakerActionAdmissionV1 {
            swap_id: swap_id.clone(),
            action,
            requested_after_generation: expected_generation,
            was_replay: false,
        })
    }

    fn open_connection(path: &Path, identity: FileIdentity) -> Result<Self, TakerFacadeStoreError> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let connection = Connection::open_with_flags(path, flags)
            .map_err(|_| TakerFacadeStoreError::DatabaseUnavailable)?;
        configure(&connection)?;
        Ok(Self {
            connection,
            path: path.to_owned(),
            identity,
        })
    }

    /// Atomically admits one initiation or exactly replays its original result.
    ///
    /// # Errors
    ///
    /// Rejects invalid input, request reuse, an existing swap, corrupt state,
    /// or an unavailable transaction. No partial request is retained on error.
    pub fn admit_initiation(
        &mut self,
        request_id: &RequestId,
        facts: &TakerInitiationFactsV1,
        authority: &TakerInitiationAuthorityV1,
        now: u64,
    ) -> Result<TakerInitiationAdmissionV1, TakerFacadeStoreError> {
        facts.validate()?;
        authority.validate()?;
        let now = i64::try_from(now).map_err(|_| TakerFacadeStoreError::InvalidInput)?;
        let request_json = encode(facts)?;
        let authority_json = encode(&authority.stored())?;
        let result_json = request_json.clone();
        self.revalidate_storage()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        if validate_exact_replay(
            &transaction,
            request_id,
            facts,
            &request_json,
            &result_json,
            &authority_json,
        )? {
            transaction
                .commit()
                .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
            self.revalidate_storage()?;
            return Ok(TakerInitiationAdmissionV1 {
                schema_version: 1,
                facts: facts.clone(),
                was_replay: true,
            });
        }
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM taker_facade_swaps WHERE swap_id = ?1)",
                [facts.swap_id.as_str()],
                |row| row.get(0),
            )
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        if exists {
            return Err(TakerFacadeStoreError::SwapConflict);
        }
        transaction
            .execute(
                "INSERT INTO taker_facade_swaps
                 (swap_id, payload_version, public_json, created_at) VALUES (?1, 1, ?2, ?3)",
                params![facts.swap_id.as_str(), request_json, now],
            )
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        transaction
            .execute(
                "INSERT INTO taker_facade_authorities
                 (swap_id, payload_version, private_json) VALUES (?1, 1, ?2)",
                params![facts.swap_id.as_str(), authority_json],
            )
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        transaction
            .execute(
                "INSERT INTO taker_facade_requests (
                     request_id, operation, swap_id, request_payload_version, request_json,
                     result_payload_version, result_json, state, created_at, updated_at
                 ) VALUES (?1, 'initiate', ?2, 1, ?3, 1, ?3, 'admitted', ?4, ?4)",
                params![
                    request_id.as_str(),
                    facts.swap_id.as_str(),
                    request_json,
                    now
                ],
            )
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        transaction
            .commit()
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        self.revalidate_storage()?;
        Ok(TakerInitiationAdmissionV1 {
            schema_version: 1,
            facts: facts.clone(),
            was_replay: false,
        })
    }

    /// Lists every public initiation projection in stable swap-ID order.
    ///
    /// # Errors
    ///
    /// Rejects changed storage identity or malformed durable public facts.
    pub fn list_initiations(&self) -> Result<Vec<TakerInitiationFactsV1>, TakerFacadeStoreError> {
        self.revalidate_storage()?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT swap_id, payload_version, public_json
                 FROM taker_facade_swaps ORDER BY swap_id",
            )
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
        drop(statement);
        let mut results = Vec::with_capacity(rows.len());
        for (swap_id, version, json) in rows {
            if version != PAYLOAD_VERSION {
                return Err(TakerFacadeStoreError::CorruptState);
            }
            let facts: TakerInitiationFactsV1 = decode(&json)?;
            facts
                .validate()
                .map_err(|_| TakerFacadeStoreError::CorruptState)?;
            if facts.swap_id.as_str() != swap_id {
                return Err(TakerFacadeStoreError::CorruptState);
            }
            results.push(facts);
        }
        self.revalidate_storage()?;
        Ok(results)
    }

    fn revalidate_storage(&self) -> Result<(), TakerFacadeStoreError> {
        validate_database_path(&self.path)?;
        let file = open_checked(&self.path)?;
        if validate_database_file(&file)? != self.identity {
            return Err(TakerFacadeStoreError::UnsafeDatabaseFile);
        }
        Ok(())
    }
}

struct TakerActionRow {
    request_id: String,
    operation: String,
    swap_id: String,
    request_version: i64,
    request_json: String,
    result_version: i64,
    result_json: String,
    state: String,
    created_at: i64,
    updated_at: i64,
}

struct ValidatedTakerActionRow {
    request_id: RequestId,
    swap_id: SwapId,
    action: TakerFacadeActionV1,
    expected_generation: u64,
}

fn query_action_by_request(
    connection: &Connection,
    request_id: &RequestId,
) -> Result<Option<TakerActionRow>, TakerFacadeStoreError> {
    connection
        .query_row(
            "SELECT request_id, operation, swap_id, request_payload_version, request_json,
                    result_payload_version, result_json, state, created_at, updated_at
             FROM taker_facade_requests WHERE request_id = ?1",
            [request_id.as_str()],
            |row| {
                Ok(TakerActionRow {
                    request_id: row.get(0)?,
                    operation: row.get(1)?,
                    swap_id: row.get(2)?,
                    request_version: row.get(3)?,
                    request_json: row.get(4)?,
                    result_version: row.get(5)?,
                    result_json: row.get(6)?,
                    state: row.get(7)?,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)
}

fn query_action_rows(
    connection: &Connection,
    swap_id: &SwapId,
) -> Result<Vec<TakerActionRow>, TakerFacadeStoreError> {
    let mut statement = connection
        .prepare(
            "SELECT request_id, operation, swap_id, request_payload_version, request_json,
                    result_payload_version, result_json, state, created_at, updated_at
             FROM taker_facade_requests
             WHERE swap_id = ?1 AND operation IN ('claim', 'refund')
             ORDER BY request_id",
        )
        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
    let rows = statement
        .query_map([swap_id.as_str()], |row| {
            Ok(TakerActionRow {
                request_id: row.get(0)?,
                operation: row.get(1)?,
                swap_id: row.get(2)?,
                request_version: row.get(3)?,
                request_json: row.get(4)?,
                result_version: row.get(5)?,
                result_json: row.get(6)?,
                state: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })
        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| TakerFacadeStoreError::CorruptState)?;
    drop(statement);
    Ok(rows)
}

fn validate_action_row(
    row: TakerActionRow,
) -> Result<ValidatedTakerActionRow, TakerFacadeStoreError> {
    let action = TakerFacadeActionV1::parse(&row.operation)?;
    let request_id =
        RequestId::new(row.request_id).map_err(|_| TakerFacadeStoreError::CorruptState)?;
    let swap_id =
        SwapId::new(row.swap_id.clone()).map_err(|_| TakerFacadeStoreError::CorruptState)?;
    let request: StoredTakerActionRequestV1 = decode(&row.request_json)?;
    let result: StoredTakerActionResultV1 = decode(&row.result_json)?;
    if row.request_version != PAYLOAD_VERSION
        || row.result_version != PAYLOAD_VERSION
        || row.state != "admitted"
        || row.created_at < 0
        || row.updated_at != row.created_at
        || request.schema_version != 1
        || result.schema_version != 1
        || request.swap_id != swap_id
        || result.swap_id != swap_id
        || result.action != action
        || result.requested_after_generation != request.expected_generation
        || encode(&request)? != row.request_json
        || encode(&result)? != row.result_json
    {
        return Err(TakerFacadeStoreError::CorruptState);
    }
    Ok(ValidatedTakerActionRow {
        request_id,
        swap_id,
        action,
        expected_generation: request.expected_generation,
    })
}

fn load_valid_action_rows(
    connection: &Connection,
    swap_id: &SwapId,
) -> Result<Vec<ValidatedTakerActionRow>, TakerFacadeStoreError> {
    let rows = query_action_rows(connection, swap_id)?
        .into_iter()
        .map(validate_action_row)
        .collect::<Result<Vec<_>, _>>()?;
    if rows.len() > 1 || rows.iter().any(|row| row.swap_id != *swap_id) {
        return Err(TakerFacadeStoreError::CorruptState);
    }
    Ok(rows)
}

fn validate_action_parent(
    connection: &Connection,
    swap_id: &SwapId,
) -> Result<(), TakerFacadeStoreError> {
    let mut rows = query_monitor_initiation(connection, swap_id)?;
    match rows.len() {
        0 if related_monitor_row_count(connection, swap_id)? == 0 => {
            Err(TakerFacadeStoreError::SwapUnavailable)
        }
        1 => {
            let row = rows.pop().ok_or(TakerFacadeStoreError::CorruptState)?;
            validate_monitor_initiation(row, swap_id).map(|_| ())
        }
        _ => Err(TakerFacadeStoreError::CorruptState),
    }
}

fn lookup_exact_action_request(
    connection: &Connection,
    request_id: &RequestId,
    swap_id: &SwapId,
    action: TakerFacadeActionV1,
    expected_generation: u64,
) -> Result<Option<TakerActionAdmissionV1>, TakerFacadeStoreError> {
    let Some(raw) = query_action_by_request(connection, request_id)? else {
        return Ok(None);
    };
    if raw.operation == "initiate" {
        return Err(TakerFacadeStoreError::RequestConflict);
    }
    let durable_swap =
        SwapId::new(raw.swap_id.clone()).map_err(|_| TakerFacadeStoreError::CorruptState)?;
    validate_action_parent(connection, &durable_swap)?;
    let rows = load_valid_action_rows(connection, &durable_swap)?;
    let stored = rows
        .into_iter()
        .find(|row| row.request_id == *request_id)
        .ok_or(TakerFacadeStoreError::CorruptState)?;
    if stored.swap_id != *swap_id
        || stored.action != action
        || stored.expected_generation != expected_generation
    {
        return Err(TakerFacadeStoreError::RequestConflict);
    }
    Ok(Some(TakerActionAdmissionV1 {
        swap_id: stored.swap_id,
        action: stored.action,
        requested_after_generation: stored.expected_generation,
        was_replay: true,
    }))
}

struct MonitorInitiationRow {
    public_version: i64,
    public_json: String,
    authority_version: i64,
    authority_json: String,
    request_id: String,
    operation: String,
    request_swap_id: String,
    request_version: i64,
    request_json: String,
    result_version: i64,
    result_json: String,
    state: String,
    created_at: i64,
    updated_at: i64,
}

fn query_monitor_initiation(
    connection: &Connection,
    swap_id: &SwapId,
) -> Result<Vec<MonitorInitiationRow>, TakerFacadeStoreError> {
    let mut statement = connection
        .prepare(
            "SELECT s.payload_version, s.public_json,
                    a.payload_version, a.private_json,
                    r.request_id, r.operation, r.swap_id,
                    r.request_payload_version, r.request_json,
                    r.result_payload_version, r.result_json, r.state,
                    r.created_at, r.updated_at
             FROM taker_facade_swaps AS s
             JOIN taker_facade_authorities AS a ON a.swap_id = s.swap_id
             JOIN taker_facade_requests AS r ON r.swap_id = s.swap_id
                                                AND r.operation = 'initiate'
             WHERE s.swap_id = ?1",
        )
        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
    let rows = statement
        .query_map([swap_id.as_str()], |row| {
            Ok(MonitorInitiationRow {
                public_version: row.get(0)?,
                public_json: row.get(1)?,
                authority_version: row.get(2)?,
                authority_json: row.get(3)?,
                request_id: row.get(4)?,
                operation: row.get(5)?,
                request_swap_id: row.get(6)?,
                request_version: row.get(7)?,
                request_json: row.get(8)?,
                result_version: row.get(9)?,
                result_json: row.get(10)?,
                state: row.get(11)?,
                created_at: row.get(12)?,
                updated_at: row.get(13)?,
            })
        })
        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| TakerFacadeStoreError::CorruptState)?;
    drop(statement);
    Ok(rows)
}

fn related_monitor_row_count(
    connection: &Connection,
    swap_id: &SwapId,
) -> Result<i64, TakerFacadeStoreError> {
    connection
        .query_row(
            "SELECT
                 EXISTS(SELECT 1 FROM taker_facade_swaps WHERE swap_id = ?1) +
                 EXISTS(SELECT 1 FROM taker_facade_authorities WHERE swap_id = ?1) +
                 EXISTS(SELECT 1 FROM taker_facade_requests
                        WHERE swap_id = ?1 AND operation = 'initiate')",
            [swap_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)
}

fn validate_monitor_initiation(
    row: MonitorInitiationRow,
    swap_id: &SwapId,
) -> Result<(TakerInitiationFactsV1, String), TakerFacadeStoreError> {
    let facts: TakerInitiationFactsV1 = decode(&row.public_json)?;
    let stored_authority: StoredTakerInitiationAuthorityV1 = decode(&row.authority_json)?;
    facts
        .validate()
        .map_err(|_| TakerFacadeStoreError::CorruptState)?;
    stored_authority
        .validate()
        .map_err(|_| TakerFacadeStoreError::CorruptState)?;
    if row.public_version != PAYLOAD_VERSION
        || row.authority_version != PAYLOAD_VERSION
        || row.request_version != PAYLOAD_VERSION
        || row.result_version != PAYLOAD_VERSION
        || RequestId::new(row.request_id).is_err()
        || row.operation != "initiate"
        || row.state != "admitted"
        || row.request_swap_id != swap_id.as_str()
        || facts.swap_id.as_str() != swap_id.as_str()
        || row.request_json != row.public_json
        || row.result_json != row.public_json
        || encode(&facts)? != row.public_json
        || encode(&stored_authority)? != row.authority_json
        || row.created_at < 0
        || row.updated_at < row.created_at
    {
        return Err(TakerFacadeStoreError::CorruptState);
    }
    Ok((facts, row.authority_json))
}

fn validate_exact_replay(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
    facts: &TakerInitiationFactsV1,
    request_json: &str,
    result_json: &str,
    authority_json: &str,
) -> Result<bool, TakerFacadeStoreError> {
    let Some((
        operation,
        swap_id,
        request_version,
        stored_request,
        result_version,
        stored_result,
        state,
    )) = transaction
        .query_row(
            "SELECT operation, swap_id, request_payload_version, request_json,
                        result_payload_version, result_json, state
                 FROM taker_facade_requests WHERE request_id = ?1",
            [request_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?
    else {
        return Ok(false);
    };
    if operation != "initiate" {
        let raw = query_action_by_request(transaction, request_id)?
            .ok_or(TakerFacadeStoreError::CorruptState)?;
        let action = validate_action_row(raw)?;
        validate_action_parent(transaction, &action.swap_id)?;
        let rows = load_valid_action_rows(transaction, &action.swap_id)?;
        if !rows.iter().any(|row| row.request_id == *request_id) {
            return Err(TakerFacadeStoreError::CorruptState);
        }
        return Err(TakerFacadeStoreError::RequestConflict);
    }
    let (authority_version, stored_authority): (i64, String) = transaction
        .query_row(
            "SELECT payload_version, private_json FROM taker_facade_authorities WHERE swap_id = ?1",
            [&swap_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| TakerFacadeStoreError::CorruptState)?;
    let (swap_version, stored_public): (i64, String) = transaction
        .query_row(
            "SELECT payload_version, public_json FROM taker_facade_swaps WHERE swap_id = ?1",
            [&swap_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| TakerFacadeStoreError::CorruptState)?;
    if request_version != PAYLOAD_VERSION
        || result_version != PAYLOAD_VERSION
        || authority_version != PAYLOAD_VERSION
        || swap_version != PAYLOAD_VERSION
        || state != "admitted"
        || stored_request != stored_public
        || stored_result != stored_public
    {
        return Err(TakerFacadeStoreError::CorruptState);
    }
    if operation != "initiate"
        || swap_id != facts.swap_id.as_str()
        || stored_request != request_json
        || stored_result != result_json
        || stored_authority != authority_json
    {
        return Err(TakerFacadeStoreError::RequestConflict);
    }
    Ok(true)
}

fn initialize_schema(connection: &mut Connection) -> Result<(), TakerFacadeStoreError> {
    let application: i64 = pragma(connection, "application_id")?;
    let version: i64 = pragma(connection, "user_version")?;
    let objects: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
    if application != 0 || version != 0 || objects != 0 {
        return Err(TakerFacadeStoreError::ForeignSchema);
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
    transaction
        .execute_batch(CREATE_SCHEMA)
        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
    transaction
        .pragma_update(None, "application_id", APPLICATION_ID)
        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)?;
    transaction
        .commit()
        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)
}

fn validate_connection(connection: &Connection) -> Result<(), TakerFacadeStoreError> {
    let application: i64 = pragma(connection, "application_id")?;
    let version: i64 = pragma(connection, "user_version")?;
    if application == APPLICATION_ID && version > SCHEMA_VERSION {
        return Err(TakerFacadeStoreError::FutureSchema);
    }
    if application != APPLICATION_ID || version != SCHEMA_VERSION {
        return Err(TakerFacadeStoreError::ForeignSchema);
    }
    let names: String = connection
        .query_row(
            "SELECT group_concat(name, ',') FROM (
                 SELECT name FROM sqlite_schema
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|_| TakerFacadeStoreError::CorruptState)?;
    let unexpected: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%' AND type != 'table'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| TakerFacadeStoreError::CorruptState)?;
    let integrity: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|_| TakerFacadeStoreError::CorruptState)?;
    let foreign_keys: i64 = connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .map_err(|_| TakerFacadeStoreError::CorruptState)?;
    if names != "taker_facade_authorities,taker_facade_requests,taker_facade_swaps"
        || unexpected != 0
        || integrity != "ok"
        || foreign_keys != 0
    {
        return Err(TakerFacadeStoreError::CorruptState);
    }
    for name in [
        "taker_facade_swaps",
        "taker_facade_authorities",
        "taker_facade_requests",
    ] {
        let actual: String = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                [name],
                |row| row.get(0),
            )
            .map_err(|_| TakerFacadeStoreError::CorruptState)?;
        let expected = expected_table_schema(name)?;
        if normalized_schema_sql(&actual) != normalized_schema_sql(&expected) {
            return Err(TakerFacadeStoreError::CorruptState);
        }
    }
    Ok(())
}

fn expected_table_schema(name: &str) -> Result<String, TakerFacadeStoreError> {
    let marker = format!("CREATE TABLE {name}");
    let start = CREATE_SCHEMA
        .find(&marker)
        .ok_or(TakerFacadeStoreError::CorruptState)?;
    let tail = &CREATE_SCHEMA[start..];
    let end = tail.find(";\n").unwrap_or(tail.len());
    Ok(tail[..end].to_owned())
}

fn validate_all_records(connection: &Connection) -> Result<(), TakerFacadeStoreError> {
    let public_by_swap = load_and_validate_swap_records(connection)?;
    validate_request_records(connection, &public_by_swap)
}

fn load_and_validate_swap_records(
    connection: &Connection,
) -> Result<BTreeMap<String, String>, TakerFacadeStoreError> {
    let mut statement = connection
        .prepare("SELECT swap_id, payload_version, public_json FROM taker_facade_swaps")
        .map_err(|_| TakerFacadeStoreError::CorruptState)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|_| TakerFacadeStoreError::CorruptState)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| TakerFacadeStoreError::CorruptState)?;
    drop(statement);
    let mut public_by_swap = BTreeMap::new();
    for (swap_id, version, json) in rows {
        let facts: TakerInitiationFactsV1 = decode(&json)?;
        if version != PAYLOAD_VERSION || facts.swap_id.as_str() != swap_id {
            return Err(TakerFacadeStoreError::CorruptState);
        }
        facts
            .validate()
            .map_err(|_| TakerFacadeStoreError::CorruptState)?;
        let (authority_version, authority_json): (i64, String) = connection
            .query_row(
                "SELECT payload_version, private_json FROM taker_facade_authorities
                 WHERE swap_id = ?1",
                [&swap_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| TakerFacadeStoreError::CorruptState)?;
        let authority: StoredTakerInitiationAuthorityV1 = decode(&authority_json)?;
        if authority_version != PAYLOAD_VERSION {
            return Err(TakerFacadeStoreError::CorruptState);
        }
        authority.validate()?;
        if public_by_swap.insert(swap_id, json).is_some() {
            return Err(TakerFacadeStoreError::CorruptState);
        }
    }
    Ok(public_by_swap)
}

fn validate_request_records(
    connection: &Connection,
    public_by_swap: &BTreeMap<String, String>,
) -> Result<(), TakerFacadeStoreError> {
    let mut statement = connection
        .prepare(
            "SELECT request_id, operation, swap_id, request_payload_version, request_json,
                    result_payload_version, result_json, state, created_at, updated_at
             FROM taker_facade_requests ORDER BY request_id",
        )
        .map_err(|_| TakerFacadeStoreError::CorruptState)?;
    let requests = statement
        .query_map([], |row| {
            Ok(TakerActionRow {
                request_id: row.get(0)?,
                operation: row.get(1)?,
                swap_id: row.get(2)?,
                request_version: row.get(3)?,
                request_json: row.get(4)?,
                result_version: row.get(5)?,
                result_json: row.get(6)?,
                state: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })
        .map_err(|_| TakerFacadeStoreError::CorruptState)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| TakerFacadeStoreError::CorruptState)?;
    drop(statement);

    let mut initiated_swaps = BTreeSet::new();
    let mut action_swaps = BTreeSet::new();
    for row in requests {
        let public_json = public_by_swap
            .get(&row.swap_id)
            .ok_or(TakerFacadeStoreError::CorruptState)?;
        if row.operation == "initiate" {
            if RequestId::new(row.request_id).is_err()
                || SwapId::new(row.swap_id.clone()).is_err()
                || row.request_version != PAYLOAD_VERSION
                || row.result_version != PAYLOAD_VERSION
                || row.state != "admitted"
                || row.created_at < 0
                || row.updated_at < row.created_at
                || row.request_json != *public_json
                || row.result_json != *public_json
                || !initiated_swaps.insert(row.swap_id)
            {
                return Err(TakerFacadeStoreError::CorruptState);
            }
        } else {
            let action = validate_action_row(row)?;
            if !action_swaps.insert(action.swap_id.as_str().to_owned()) {
                return Err(TakerFacadeStoreError::CorruptState);
            }
        }
    }
    if initiated_swaps.len() != public_by_swap.len()
        || initiated_swaps
            .iter()
            .any(|swap_id| !public_by_swap.contains_key(swap_id))
    {
        return Err(TakerFacadeStoreError::CorruptState);
    }
    Ok(())
}

fn configure(connection: &Connection) -> Result<(), TakerFacadeStoreError> {
    connection
        .busy_timeout(Duration::from_secs(5))
        .and_then(|()| connection.pragma_update(None, "journal_mode", "WAL"))
        .and_then(|()| connection.pragma_update(None, "synchronous", "FULL"))
        .and_then(|()| connection.pragma_update(None, "foreign_keys", "ON"))
        .and_then(|()| connection.pragma_update(None, "secure_delete", "ON"))
        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)
}

fn pragma(connection: &Connection, name: &str) -> Result<i64, TakerFacadeStoreError> {
    connection
        .pragma_query_value(None, name, |row| row.get(0))
        .map_err(|_| TakerFacadeStoreError::StorageUnavailable)
}

fn normalized_schema_sql(value: &str) -> String {
    value
        .trim()
        .trim_end_matches(';')
        .split_ascii_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn encode<T: Serialize>(value: &T) -> Result<String, TakerFacadeStoreError> {
    serde_json::to_string(value).map_err(|_| TakerFacadeStoreError::StorageUnavailable)
}

fn decode<T: for<'de> Deserialize<'de>>(value: &str) -> Result<T, TakerFacadeStoreError> {
    serde_json::from_str(value).map_err(|_| TakerFacadeStoreError::CorruptState)
}

fn validate_database_path(path: &Path) -> Result<(), TakerFacadeStoreError> {
    if !normalized_absolute_file_path(path) {
        return Err(TakerFacadeStoreError::DatabaseUnavailable);
    }
    let parent = path
        .parent()
        .ok_or(TakerFacadeStoreError::DatabaseUnavailable)?;
    let metadata =
        fs::symlink_metadata(parent).map_err(|_| TakerFacadeStoreError::DatabaseUnavailable)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(TakerFacadeStoreError::UnsafeDatabaseFile);
    }
    Ok(())
}

fn validate_authority_path(path: &Path) -> Result<(), TakerFacadeStoreError> {
    if !normalized_absolute_file_path(path) || path.as_os_str().len() > MAX_PATH_BYTES {
        return Err(TakerFacadeStoreError::InvalidInput);
    }
    Ok(())
}

fn normalized_absolute_file_path(path: &Path) -> bool {
    path.is_absolute()
        && path.file_name().is_some()
        && path
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
}

fn open_checked(path: &Path) -> Result<File, TakerFacadeStoreError> {
    open_no_symlinks(path, OFlags::RDWR, Mode::empty()).map_err(|error| {
        if error == rustix::io::Errno::LOOP {
            TakerFacadeStoreError::UnsafeDatabaseFile
        } else {
            TakerFacadeStoreError::DatabaseUnavailable
        }
    })
}

fn classify_existing(path: &Path) -> Result<(), TakerFacadeStoreError> {
    let file = open_checked(path).map_err(|_| TakerFacadeStoreError::UnsafeDatabaseFile)?;
    validate_database_file(&file).map(|_| ())
}

fn validate_database_file(file: &File) -> Result<FileIdentity, TakerFacadeStoreError> {
    let metadata = file
        .metadata()
        .map_err(|_| TakerFacadeStoreError::UnsafeDatabaseFile)?;
    if !is_owner_private_regular_file(&metadata, 0o600) {
        return Err(TakerFacadeStoreError::UnsafeDatabaseFile);
    }
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn sync_parent(path: &Path) -> Result<(), TakerFacadeStoreError> {
    open_no_symlinks(
        path.parent()
            .ok_or(TakerFacadeStoreError::DatabaseUnavailable)?,
        OFlags::RDONLY | OFlags::DIRECTORY,
        Mode::empty(),
    )
    .map_err(|_| TakerFacadeStoreError::DatabaseUnavailable)?
    .sync_all()
    .map_err(|_| TakerFacadeStoreError::DatabaseUnavailable)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

mod maker_identity_serde {
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

    pub fn serialize<S>(value: &[u8; 33], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut encoded = String::with_capacity(66);
        for byte in value {
            encoded.push(char::from(LOWER_HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(LOWER_HEX[usize::from(byte & 0x0f)]));
        }
        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 33], D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.len() != 66
            || value
                .bytes()
                .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
        {
            return Err(D::Error::custom(
                "Maker identity is not canonical lowercase hex",
            ));
        }
        let mut decoded = [0_u8; 33];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            decoded[index] = (decode_nibble(pair[0]) << 4) | decode_nibble(pair[1]);
        }
        super::PublicKey::from_slice(&decoded)
            .map_err(|_| D::Error::custom("Maker identity is not a compressed secp256k1 point"))?;
        Ok(decoded)
    }

    const fn decode_nibble(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => unreachable!(),
        }
    }
}
