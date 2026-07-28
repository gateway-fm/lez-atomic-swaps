//! Maker-only, no-clobber actor provisioning from one accepted Chat agreement.

use std::{
    fs::{self, File},
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};
use lez_bridge_protocol::Hex32;
use lez_swap_core::{Participant, SwapId, UnixSeconds};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, MAX_ZEC_AGREEMENT_RECORD_BYTES, ZecAgreementDraftV1, ZecAgreementV1,
};
use rustix::{
    fs::{CWD, RenameFlags, renameat_with},
    io::Errno,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::{
    ActorConfig, ActorRole,
    config::encode_rebound_local_v0_2_actor_config,
    local_poc::{create_private_directory, validate_role_authority, write_private_new},
    secure_file::{FilePrivacy, read_bounded_identified},
};

const MAX_CONFIG_BYTES: usize = 64 * 1024;

/// Secret-free immutable result used to construct one durable scheduler manifest.
#[derive(Clone, Debug, Serialize)]
pub struct ZecMakerActorProvisionV1 {
    schema_version: u16,
    was_replay: bool,
    swap_id: SwapId,
    agreement_file: PathBuf,
    agreement_sha256: Hex32,
    config_file: PathBuf,
    config_sha256: Hex32,
    state_database: PathBuf,
}

impl ZecMakerActorProvisionV1 {
    /// Whether a previously published byte-identical bundle was reused.
    #[must_use]
    pub const fn was_replay(&self) -> bool {
        self.was_replay
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

    /// Owner-private maker actor configuration path.
    #[must_use]
    pub fn config_file(&self) -> &Path {
        &self.config_file
    }

    /// SHA-256 of the exact maker actor configuration bytes.
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

/// Publishes one maker-only actor bundle from an accepted Chat agreement.
///
/// The destination is published from a private sibling staging directory with
/// `RENAME_NOREPLACE`. An existing destination is accepted only when its
/// agreement and config bytes, role, swap, state path, and authority all match.
/// No taker configuration or credential is read or emitted.
///
/// # Errors
///
/// Fails closed on unsafe paths, non-maker authority, changed executable terms,
/// invalid signatures, output collision, partial publication, or semantic drift.
pub fn provision_zec_maker_actor_from_chat(
    source_maker_config_file: &Path,
    final_agreement_wire: &[u8],
    accepted_at: UnixSeconds,
    output_root: &Path,
) -> Result<ZecMakerActorProvisionV1> {
    let source = ActorConfig::load_private(source_maker_config_file)
        .context("source maker config is unsafe or invalid")?;
    provision_zec_maker_actor_from_config(&source, final_agreement_wire, accepted_at, output_root)
}

/// Publishes one maker-only actor bundle from a startup-pinned configuration.
///
/// # Errors
///
/// Fails closed if any source file identity changed after the configuration was
/// loaded, or on the same validation and publication errors as the path API.
pub fn provision_zec_maker_actor_from_config(
    source: &ActorConfig,
    final_agreement_wire: &[u8],
    accepted_at: UnixSeconds,
    output_root: &Path,
) -> Result<ZecMakerActorProvisionV1> {
    let prepared = prepare_provision(source, final_agreement_wire, accepted_at, output_root)?;
    let paths = ProvisionPaths::new(output_root);
    if output_root.exists() {
        validate_exact_replay(
            &paths,
            final_agreement_wire,
            &prepared.config_bytes,
            &prepared.agreement,
        )?;
        return Ok(prepared.summary(paths, true));
    }

    let stage_root = create_stage(output_root)?;
    let stage_paths = ProvisionPaths::new(&stage_root);
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
            )?;
            Ok(prepared.summary(paths, false))
        }
        Ok(PublishOutcome::Existing) => {
            fs::remove_dir_all(&stage_root).context("remove collided maker actor stage")?;
            validate_exact_replay(
                &paths,
                final_agreement_wire,
                &prepared.config_bytes,
                &prepared.agreement,
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
    accepted_at: UnixSeconds,
    output_root: &Path,
) -> Result<PreparedProvision> {
    ensure!(
        !final_agreement_wire.is_empty()
            && final_agreement_wire.len() <= MAX_ZEC_AGREEMENT_RECORD_BYTES,
        "final Chat agreement is unavailable or unsafe"
    );
    ensure!(
        source.role() == ActorRole::Maker,
        "source config is not Maker"
    );
    let source_material = source
        .load_activate_material()
        .context("source maker activation material is invalid")?;
    let source_agreement =
        ZecAgreementV1::from_wire_at(source_material.signed_agreement_wire(), accepted_at)
            .context("source agreement is invalid at final acceptance")?;
    let accepted = AcceptedZecAgreementV1::accept_wire_at(
        final_agreement_wire,
        accepted_at,
        Participant::Maker,
        0,
    )
    .context("final Chat agreement is invalid for maker")?;
    let agreement = accepted.agreement();
    ensure!(
        agreement.application_swap_id() == source.swap_id().as_str(),
        "final agreement swap ID differs from source actor"
    );
    let expected = ZecAgreementDraftV1::rebind_validated_transcript(
        &source_agreement,
        *agreement.transcript(),
    );
    let expected_wire = expected
        .encode_wire()
        .context("failed to encode expected Chat body")?;
    let expected = ZecAgreementDraftV1::from_wire_at(&expected_wire, accepted_at)
        .context("expected Chat body did not revalidate")?;
    ensure!(
        expected.body() == agreement.record().body(),
        "final agreement changed executable chain facts outside the Chat transcript"
    );
    validate_role_authority(
        &source_material,
        agreement,
        Participant::Maker,
        source.is_local_zcash_funder(),
    )?;

    validate_private_parent(output_root)?;
    let agreement_sha256 = Hex32::from_bytes(Sha256::digest(final_agreement_wire).into());
    let swap_id = SwapId::new(agreement.application_swap_id().to_owned())
        .context("final application swap ID is invalid")?;
    let paths = ProvisionPaths::new(output_root);
    let config_bytes = encode_rebound_local_v0_2_actor_config(
        source,
        swap_id.clone(),
        paths.agreement.clone(),
        agreement_sha256,
        paths.state_database.clone(),
        paths.bridge_journal.clone(),
    )
    .context("failed to encode rebound maker config")?;
    let config_sha256 = Hex32::from_bytes(Sha256::digest(&config_bytes).into());
    Ok(PreparedProvision {
        agreement: agreement.clone(),
        config_bytes,
        swap_id,
        agreement_sha256,
        config_sha256,
    })
}

struct PreparedProvision {
    agreement: ZecAgreementV1,
    config_bytes: Vec<u8>,
    swap_id: SwapId,
    agreement_sha256: Hex32,
    config_sha256: Hex32,
}

impl PreparedProvision {
    fn summary(self, paths: ProvisionPaths, was_replay: bool) -> ZecMakerActorProvisionV1 {
        summary(
            paths,
            self.swap_id,
            self.agreement_sha256,
            self.config_sha256,
            was_replay,
        )
    }
}

struct ProvisionPaths {
    root: PathBuf,
    shared: PathBuf,
    maker: PathBuf,
    state: PathBuf,
    agreement: PathBuf,
    config: PathBuf,
    state_database: PathBuf,
    bridge_journal: PathBuf,
}

impl ProvisionPaths {
    fn new(root: &Path) -> Self {
        let shared = root.join("shared");
        let maker = root.join("maker");
        let state = maker.join("state");
        Self {
            root: root.to_path_buf(),
            agreement: shared.join("agreement-v2.borsh"),
            config: maker.join("actor-config.json"),
            state_database: state.join("actor.sqlite3"),
            bridge_journal: state.join("bridge.sqlite3"),
            shared,
            maker,
            state,
        }
    }
}

fn summary(
    paths: ProvisionPaths,
    swap_id: SwapId,
    agreement_sha256: Hex32,
    config_sha256: Hex32,
    was_replay: bool,
) -> ZecMakerActorProvisionV1 {
    ZecMakerActorProvisionV1 {
        schema_version: 1,
        was_replay,
        swap_id,
        agreement_file: paths.agreement,
        agreement_sha256,
        config_file: paths.config,
        config_sha256,
        state_database: paths.state_database,
    }
}

fn validate_private_parent(output_root: &Path) -> Result<()> {
    ensure!(output_root.is_absolute(), "output root must be absolute");
    ensure!(
        output_root
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_))),
        "output root must be normalized"
    );
    let parent = output_root.parent().context("output root has no parent")?;
    validate_private_directory(parent, "output parent")
}

fn validate_private_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path).with_context(|| format!("{label} is unavailable"))?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.permissions().mode() & 0o7777 == 0o700
            && fs::canonicalize(path).with_context(|| format!("{label} is unavailable"))? == path,
        "{label} is unsafe"
    );
    Ok(())
}

fn create_stage(output_root: &Path) -> Result<PathBuf> {
    let parent = output_root.parent().context("output root has no parent")?;
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| anyhow::anyhow!("staging randomness unavailable"))?;
    let stage = parent.join(format!(".lez-maker-actor-stage-{}", hex::encode(nonce)));
    create_private_directory(&stage)?;
    Ok(stage)
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
    for directory in [&paths.shared, &paths.maker, &paths.state] {
        create_private_directory(directory)?;
    }
    write_private_new(&paths.agreement, agreement)?;
    write_private_new(&paths.config, config)?;
    for directory in [&paths.state, &paths.shared, &paths.maker, stage_root] {
        sync_directory(directory)?;
    }
    match renameat_with(CWD, stage_root, CWD, output_root, RenameFlags::NOREPLACE) {
        Ok(()) => {
            sync_directory(output_root.parent().context("output root has no parent")?)?;
            Ok(PublishOutcome::Published)
        }
        Err(Errno::EXIST) => Ok(PublishOutcome::Existing),
        Err(_) => Err(anyhow::anyhow!("failed to publish maker actor output")),
    }
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("failed to sync private directory {}", path.display()))
}

fn validate_exact_replay(
    paths: &ProvisionPaths,
    agreement_wire: &[u8],
    config_bytes: &[u8],
    agreement: &ZecAgreementV1,
) -> Result<()> {
    for (path, label) in [
        (&paths.root, "actor root"),
        (&paths.shared, "actor shared root"),
        (&paths.maker, "maker root"),
        (&paths.state, "maker state root"),
    ] {
        validate_private_directory(path, label)?;
    }
    ensure!(
        !paths.root.join("taker").exists(),
        "maker bundle contains taker state"
    );
    validate_optional_private_file(&paths.state_database, "maker state database")?;
    validate_optional_private_file(&paths.bridge_journal, "maker bridge journal")?;
    let (actual_agreement, _) = read_bounded_identified(
        &paths.agreement,
        MAX_ZEC_AGREEMENT_RECORD_BYTES,
        FilePrivacy::OwnerPrivate,
    )
    .map_err(|_| anyhow::anyhow!("published maker agreement is unsafe"))?;
    let (actual_config, _) =
        read_bounded_identified(&paths.config, MAX_CONFIG_BYTES, FilePrivacy::OwnerPrivate)
            .map_err(|_| anyhow::anyhow!("published maker config is unsafe"))?;
    ensure!(
        actual_agreement.as_slice() == agreement_wire,
        "maker agreement collision"
    );
    ensure!(
        actual_config.as_slice() == config_bytes,
        "maker config collision"
    );
    let config = ActorConfig::load_private(&paths.config)
        .context("published maker config did not reload")?;
    ensure!(
        config.role() == ActorRole::Maker
            && config.swap_id().as_str() == agreement.application_swap_id()
            && config.role_state_db() == paths.state_database,
        "published maker config changed semantic binding"
    );
    let material = config
        .load_activate_material()
        .context("published maker activation material is invalid")?;
    ensure!(
        material.signed_agreement_wire() == agreement_wire,
        "published maker agreement drifted"
    );
    validate_role_authority(
        &material,
        agreement,
        Participant::Maker,
        config.is_local_zcash_funder(),
    )?;
    for file in [
        &paths.agreement,
        &paths.config,
        &paths.state_database,
        &paths.bridge_journal,
    ] {
        sync_file_if_present(file)?;
    }
    for directory in [&paths.state, &paths.shared, &paths.maker, &paths.root] {
        sync_directory(directory)?;
    }
    sync_directory(paths.root.parent().context("actor root has no parent")?)?;
    Ok(())
}

fn sync_file_if_present(path: &Path) -> Result<()> {
    match File::open(path) {
        Ok(file) => file
            .sync_all()
            .with_context(|| format!("failed to sync private file {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("failed to open private file {}", path.display()))
        }
    }
}

fn validate_optional_private_file(path: &Path, label: &str) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(anyhow::anyhow!("{label} is unavailable")),
    };
    ensure!(
        metadata.file_type().is_file()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.mode() & 0o7777 == 0o600
            && metadata.nlink() == 1
            && fs::canonicalize(path).with_context(|| format!("{label} is unavailable"))? == path,
        "{label} is unsafe"
    );
    Ok(())
}
