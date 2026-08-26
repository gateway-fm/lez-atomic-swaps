//! Role-fixed, no-clobber Bitcoin actor provisioning.

use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::Write as _,
    os::unix::fs::{
        DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _,
    },
    path::{Component, Path, PathBuf},
};

use rustix::{
    fs::{CWD, RenameFlags, renameat_with},
    io::Errno,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::{
    ActorConfig, ActorRole, BtcAgreementV1, Hex32, MAX_BTC_AGREEMENT_RECORD_BYTES,
    MAX_CONFIG_BYTES, SUPERVISED_CONFIG_SCHEMA_VERSION, SwapId, load_agreement, read_stable_file,
    validate_activation_material,
};

/// Secret-free immutable result for one role-fixed Bitcoin actor bundle.
#[derive(Clone, Debug, Serialize)]
pub struct BtcActorProvisionV1 {
    schema_version: u16,
    was_replay: bool,
    role: ActorRole,
    swap_id: SwapId,
    agreement_file: PathBuf,
    agreement_sha256: Hex32,
    config_file: PathBuf,
    config_sha256: Hex32,
    state_database: PathBuf,
}

impl BtcActorProvisionV1 {
    /// Whether a previously published byte-identical bundle was reused.
    #[must_use]
    pub const fn was_replay(&self) -> bool {
        self.was_replay
    }

    /// Role permanently bound to the published bundle.
    #[must_use]
    pub const fn role(&self) -> ActorRole {
        self.role
    }

    /// Exact accepted application swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Owner-private exact final agreement path.
    #[must_use]
    pub fn agreement_file(&self) -> &Path {
        &self.agreement_file
    }

    /// SHA-256 of the exact accepted agreement wire.
    #[must_use]
    pub const fn agreement_sha256(&self) -> [u8; 32] {
        *self.agreement_sha256.as_bytes()
    }

    /// Owner-private role-fixed actor configuration path.
    #[must_use]
    pub fn config_file(&self) -> &Path {
        &self.config_file
    }

    /// SHA-256 of the exact role-fixed actor configuration bytes.
    #[must_use]
    pub const fn config_sha256(&self) -> [u8; 32] {
        *self.config_sha256.as_bytes()
    }

    /// Role-local actor lifecycle database path.
    #[must_use]
    pub fn state_database(&self) -> &Path {
        &self.state_database
    }
}

/// Bounded, secret-free Bitcoin actor provisioning failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum BtcActorProvisionError {
    /// Source authority, agreement, role, path, or replay semantics were invalid.
    #[error("Bitcoin actor provisioning input is invalid")]
    Invalid,
    /// A private filesystem publication or durability operation failed.
    #[error("Bitcoin actor provisioning filesystem operation failed")]
    Filesystem,
}

type Result<T> = std::result::Result<T, BtcActorProvisionError>;

const fn role_name(role: ActorRole) -> &'static str {
    match role {
        ActorRole::Maker => "maker",
        ActorRole::Taker => "taker",
    }
}

const fn other_role_name(role: ActorRole) -> &'static str {
    match role {
        ActorRole::Maker => "taker",
        ActorRole::Taker => "maker",
    }
}

/// Publishes one Maker-only Bitcoin actor bundle from a startup-pinned config.
///
/// # Errors
///
/// Fails closed on a non-Maker or non-supervised source, agreement drift,
/// unsafe paths, output collision, partial publication, or semantic replay drift.
pub fn provision_btc_maker_actor_from_config(
    source: &ActorConfig,
    final_agreement_wire: &[u8],
    accepted_at_unix_seconds: u64,
    output_root: &Path,
) -> Result<BtcActorProvisionV1> {
    provision_btc_actor_from_config(
        source,
        final_agreement_wire,
        accepted_at_unix_seconds,
        output_root,
        ActorRole::Maker,
    )
}

/// Publishes one Taker-only Bitcoin actor bundle from a startup-pinned config.
///
/// # Errors
///
/// Fails closed on a non-Taker or non-supervised source, agreement drift,
/// unsafe paths, output collision, partial publication, or semantic replay drift.
pub fn provision_btc_taker_actor_from_config(
    source: &ActorConfig,
    final_agreement_wire: &[u8],
    accepted_at_unix_seconds: u64,
    output_root: &Path,
) -> Result<BtcActorProvisionV1> {
    provision_btc_actor_from_config(
        source,
        final_agreement_wire,
        accepted_at_unix_seconds,
        output_root,
        ActorRole::Taker,
    )
}

fn provision_btc_actor_from_config(
    source: &ActorConfig,
    final_agreement_wire: &[u8],
    accepted_at_unix_seconds: u64,
    output_root: &Path,
    role: ActorRole,
) -> Result<BtcActorProvisionV1> {
    let prepared = prepare_provision(
        source,
        final_agreement_wire,
        accepted_at_unix_seconds,
        output_root,
        role,
    )?;
    let paths = ProvisionPaths::new(output_root, role);
    if output_root.exists() {
        validate_exact_replay(
            &paths,
            final_agreement_wire,
            &prepared.config_bytes,
            &prepared.agreement,
            accepted_at_unix_seconds,
        )?;
        return Ok(prepared.summary(paths, true));
    }

    let stage_root = create_stage(output_root, role)?;
    let stage_paths = ProvisionPaths::new(&stage_root, role);
    let publication = publish_stage(
        &stage_paths,
        &stage_root,
        output_root,
        final_agreement_wire,
        &prepared.config_bytes,
    );
    match publication {
        Ok(PublishOutcome::Published) => {
            validate_exact_replay(
                &paths,
                final_agreement_wire,
                &prepared.config_bytes,
                &prepared.agreement,
                accepted_at_unix_seconds,
            )?;
            Ok(prepared.summary(paths, false))
        }
        Ok(PublishOutcome::Existing) => {
            fs::remove_dir_all(&stage_root).map_err(|_| BtcActorProvisionError::Filesystem)?;
            validate_exact_replay(
                &paths,
                final_agreement_wire,
                &prepared.config_bytes,
                &prepared.agreement,
                accepted_at_unix_seconds,
            )?;
            Ok(prepared.summary(paths, true))
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&stage_root);
            Err(error)
        }
    }
}

fn prepare_provision(
    source: &ActorConfig,
    final_agreement_wire: &[u8],
    accepted_at_unix_seconds: u64,
    output_root: &Path,
    role: ActorRole,
) -> Result<PreparedProvision> {
    if source.schema_version != SUPERVISED_CONFIG_SCHEMA_VERSION
        || source.role != role
        || accepted_at_unix_seconds == 0
        || i64::try_from(accepted_at_unix_seconds).is_err()
        || final_agreement_wire.is_empty()
        || final_agreement_wire.len() > MAX_BTC_AGREEMENT_RECORD_BYTES
    {
        return Err(BtcActorProvisionError::Invalid);
    }
    let (source_agreement, _) =
        load_agreement(source).map_err(|_| BtcActorProvisionError::Invalid)?;
    validate_activation_material(source, &source_agreement)
        .map_err(|_| BtcActorProvisionError::Invalid)?;
    let agreement = BtcAgreementV1::from_wire(final_agreement_wire)
        .map_err(|_| BtcActorProvisionError::Invalid)?;
    if source_agreement.body() != agreement.body() {
        return Err(BtcActorProvisionError::Invalid);
    }
    let agreement_sha256 = Hex32::from_bytes(Sha256::digest(final_agreement_wire).into());
    validate_private_parent(output_root)?;

    let paths = ProvisionPaths::new(output_root, role);
    let mut rebound = source.clone();
    rebound.agreement_file.clone_from(&paths.agreement);
    rebound.state_db.clone_from(&paths.state_database);
    rebound.accepted_at_unix_seconds = accepted_at_unix_seconds;
    rebound.agreement_sha256 = Some(agreement_sha256);
    rebound
        .validate()
        .map_err(|_| BtcActorProvisionError::Invalid)?;
    let config_bytes =
        serde_json::to_vec_pretty(&rebound).map_err(|_| BtcActorProvisionError::Invalid)?;
    ActorConfig::from_private_bytes(&config_bytes, true)
        .map_err(|_| BtcActorProvisionError::Invalid)?;
    let config_sha256 = Hex32::from_bytes(Sha256::digest(&config_bytes).into());
    let swap_id = agreement.coordinator().id().clone();
    Ok(PreparedProvision {
        agreement,
        config_bytes,
        swap_id,
        agreement_sha256,
        config_sha256,
        role,
    })
}

struct PreparedProvision {
    agreement: BtcAgreementV1,
    config_bytes: Vec<u8>,
    swap_id: SwapId,
    agreement_sha256: Hex32,
    config_sha256: Hex32,
    role: ActorRole,
}

impl PreparedProvision {
    fn summary(self, paths: ProvisionPaths, was_replay: bool) -> BtcActorProvisionV1 {
        BtcActorProvisionV1 {
            schema_version: 1,
            was_replay,
            role: self.role,
            swap_id: self.swap_id,
            agreement_file: paths.agreement,
            agreement_sha256: self.agreement_sha256,
            config_file: paths.config,
            config_sha256: self.config_sha256,
            state_database: paths.state_database,
        }
    }
}

struct ProvisionPaths {
    root: PathBuf,
    shared: PathBuf,
    role: ActorRole,
    role_root: PathBuf,
    state: PathBuf,
    agreement: PathBuf,
    config: PathBuf,
    state_database: PathBuf,
}

impl ProvisionPaths {
    fn new(root: &Path, role: ActorRole) -> Self {
        let shared = root.join("shared");
        let role_root = root.join(role_name(role));
        let state = role_root.join("state");
        Self {
            root: root.to_path_buf(),
            role,
            agreement: shared.join("agreement-v1.borsh"),
            config: role_root.join("actor-config.json"),
            state_database: state.join("actor.sqlite3"),
            shared,
            role_root,
            state,
        }
    }
}

fn validate_private_parent(output_root: &Path) -> Result<()> {
    if !normalized_absolute(output_root) {
        return Err(BtcActorProvisionError::Invalid);
    }
    let parent = output_root
        .parent()
        .ok_or(BtcActorProvisionError::Invalid)?;
    validate_private_directory(parent)
}

fn normalized_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn validate_private_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| BtcActorProvisionError::Invalid)?;
    let canonical = fs::canonicalize(path).map_err(|_| BtcActorProvisionError::Invalid)?;
    if metadata.file_type().is_dir()
        && metadata.uid() == rustix::process::geteuid().as_raw()
        && metadata.permissions().mode() & 0o7777 == 0o700
        && canonical == path
    {
        Ok(())
    } else {
        Err(BtcActorProvisionError::Invalid)
    }
}

fn create_stage(output_root: &Path, role: ActorRole) -> Result<PathBuf> {
    let parent = output_root
        .parent()
        .ok_or(BtcActorProvisionError::Invalid)?;
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| BtcActorProvisionError::Filesystem)?;
    let stage = parent.join(format!(
        ".lez-btc-{}-actor-stage-{}",
        role_name(role),
        hex::encode(nonce),
    ));
    DirBuilder::new()
        .mode(0o700)
        .create(&stage)
        .map_err(|_| BtcActorProvisionError::Filesystem)?;
    Ok(stage)
}

fn create_private_directory(path: &Path) -> Result<()> {
    DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|_| BtcActorProvisionError::Filesystem)
}

fn write_private_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| BtcActorProvisionError::Filesystem)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| BtcActorProvisionError::Filesystem)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublishOutcome {
    Published,
    Existing,
}

fn publish_stage(
    paths: &ProvisionPaths,
    stage_root: &Path,
    output_root: &Path,
    agreement: &[u8],
    config: &[u8],
) -> Result<PublishOutcome> {
    for directory in [&paths.shared, &paths.role_root, &paths.state] {
        create_private_directory(directory)?;
    }
    write_private_new(&paths.agreement, agreement)?;
    write_private_new(&paths.config, config)?;
    for directory in [&paths.state, &paths.shared, &paths.role_root, stage_root] {
        sync_directory(directory)?;
    }
    match renameat_with(CWD, stage_root, CWD, output_root, RenameFlags::NOREPLACE) {
        Ok(()) => {
            sync_directory(
                output_root
                    .parent()
                    .ok_or(BtcActorProvisionError::Invalid)?,
            )?;
            Ok(PublishOutcome::Published)
        }
        Err(Errno::EXIST) => Ok(PublishOutcome::Existing),
        Err(_) => Err(BtcActorProvisionError::Filesystem),
    }
}

fn validate_exact_replay(
    paths: &ProvisionPaths,
    agreement_wire: &[u8],
    config_bytes: &[u8],
    agreement: &BtcAgreementV1,
    accepted_at_unix_seconds: u64,
) -> Result<()> {
    for directory in [&paths.root, &paths.shared, &paths.role_root, &paths.state] {
        validate_private_directory(directory)?;
    }
    validate_absent(&paths.root.join(other_role_name(paths.role)))?;
    validate_optional_private_file(&paths.state_database)?;
    let actual_agreement = read_stable_file(&paths.agreement, MAX_BTC_AGREEMENT_RECORD_BYTES, true)
        .map_err(|()| BtcActorProvisionError::Invalid)?;
    let actual_config = read_stable_file(&paths.config, MAX_CONFIG_BYTES, true)
        .map_err(|()| BtcActorProvisionError::Invalid)?;
    if actual_agreement != agreement_wire || actual_config != config_bytes {
        return Err(BtcActorProvisionError::Invalid);
    }
    let config =
        ActorConfig::load_private(&paths.config).map_err(|_| BtcActorProvisionError::Invalid)?;
    let (published_agreement, published_wire) =
        load_agreement(&config).map_err(|_| BtcActorProvisionError::Invalid)?;
    if config.schema_version != SUPERVISED_CONFIG_SCHEMA_VERSION
        || config.role != paths.role
        || config.agreement_file != paths.agreement
        || config.state_db != paths.state_database
        || config.accepted_at_unix_seconds != accepted_at_unix_seconds
        || published_wire != agreement_wire
        || published_agreement.coordinator().id() != agreement.coordinator().id()
    {
        return Err(BtcActorProvisionError::Invalid);
    }
    for file in [&paths.agreement, &paths.config, &paths.state_database] {
        sync_file_if_present(file)?;
    }
    for directory in [&paths.state, &paths.shared, &paths.role_root, &paths.root] {
        sync_directory(directory)?;
    }
    sync_directory(paths.root.parent().ok_or(BtcActorProvisionError::Invalid)?)
}

fn validate_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) | Err(_) => Err(BtcActorProvisionError::Invalid),
    }
}

fn validate_optional_private_file(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(BtcActorProvisionError::Invalid),
    };
    let canonical = fs::canonicalize(path).map_err(|_| BtcActorProvisionError::Invalid)?;
    if metadata.file_type().is_file()
        && metadata.uid() == rustix::process::geteuid().as_raw()
        && metadata.permissions().mode() & 0o7777 == 0o600
        && metadata.nlink() == 1
        && canonical == path
    {
        Ok(())
    } else {
        Err(BtcActorProvisionError::Invalid)
    }
}

fn sync_file_if_present(path: &Path) -> Result<()> {
    match File::open(path) {
        Ok(file) => file
            .sync_all()
            .map_err(|_| BtcActorProvisionError::Filesystem),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(BtcActorProvisionError::Filesystem),
    }
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| BtcActorProvisionError::Filesystem)
}
