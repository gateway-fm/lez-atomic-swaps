//! Role-fixed, no-copy XMR application actor provisioning.

use std::{
    fs::{self, DirBuilder, File},
    os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context as _, Result, anyhow, ensure};
use lez_adaptor_role_runner::{Role as RunnerRole, ValidatedSession};
use lez_swap_store::{AdaptorSessionPhase, AdaptorSessionSnapshot, SqliteAdaptorSessionJournal};
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES, XmrActivatedAgreementV1,
    XmrAgreementV1, XmrSessionTranscriptV1,
};
use rustix::{
    fs::{RenameFlags, renameat_with},
    io::Errno,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{
    ActorRole, FilePolicy, PRIVATE_KEY_MAX_BYTES, PRIVATE_MANIFEST_FILE, SecureDestination,
    StageRolePackets, VIEW_KEY_FILE, cleanup_staged_file, create_staged_file,
    create_staging_directory, open_path_no_symlinks, read_bounded_file, read_validated_stage_a,
    validate_private_role, write_new_at,
};

const APPLICATION_PROVISION_SCHEMA_V1: u16 = 1;
const APPLICATION_MANIFEST_FILE: &str = "actor-provision.json";
const STAGE_A_FILE: &str = "stage-a-v1.borsh";
const STAGE_B_FILE: &str = "stage-b-v1.borsh";
const APPLICATION_STATE_DIRECTORY: &str = "state";
const APPLICATION_JOURNAL_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Maximum accepted canonical role-authority manifest size.
pub const XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES: u64 = 16 * 1024;

/// Secret-free result for one immutable role-fixed XMR application bundle.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[must_use]
pub struct XmrActorProvisionV1 {
    was_replay: bool,
    schema_version: u16,
    role: ActorRole,
    swap_id: [u8; 32],
    agreement_commitment: [u8; 32],
    activation_commitment: [u8; 32],
    stage_a_file: PathBuf,
    stage_a_sha256: [u8; 32],
    stage_b_file: PathBuf,
    stage_b_sha256: [u8; 32],
    manifest_file: PathBuf,
    manifest_sha256: [u8; 32],
    state_database: PathBuf,
    state_directory: PathBuf,
}

impl XmrActorProvisionV1 {
    /// Whether an already published byte-identical bundle was reused.
    #[must_use]
    pub const fn was_replay(&self) -> bool {
        self.was_replay
    }

    /// Role permanently bound to the bundle.
    #[must_use]
    pub const fn role(&self) -> ActorRole {
        self.role
    }

    /// Exact signed Stage-A swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> [u8; 32] {
        self.swap_id
    }

    /// Exact countersigned Stage-A agreement commitment.
    #[must_use]
    pub const fn agreement_commitment(&self) -> [u8; 32] {
        self.agreement_commitment
    }

    /// Exact countersigned Stage-B activation commitment.
    #[must_use]
    pub const fn activation_commitment(&self) -> [u8; 32] {
        self.activation_commitment
    }

    /// Private canonical Stage-A path inside the application bundle.
    #[must_use]
    pub fn stage_a_file(&self) -> &Path {
        &self.stage_a_file
    }

    /// SHA-256 of the exact accepted Stage-A wire.
    #[must_use]
    pub const fn stage_a_sha256(&self) -> [u8; 32] {
        self.stage_a_sha256
    }

    /// Private canonical Stage-B path inside the application bundle.
    #[must_use]
    pub fn stage_b_file(&self) -> &Path {
        &self.stage_b_file
    }

    /// SHA-256 of the exact accepted Stage-B wire.
    #[must_use]
    pub const fn stage_b_sha256(&self) -> [u8; 32] {
        self.stage_b_sha256
    }

    /// Owner-private role-authority manifest.
    #[must_use]
    pub fn manifest_file(&self) -> &Path {
        &self.manifest_file
    }

    /// SHA-256 of the exact role-authority manifest bytes.
    #[must_use]
    pub const fn manifest_sha256(&self) -> [u8; 32] {
        self.manifest_sha256
    }

    /// Existing role-local adaptor journal validated as actor state authority.
    #[must_use]
    pub fn state_database(&self) -> &Path {
        &self.state_database
    }

    /// Empty role-local state directory reserved for the semantic actor.
    #[must_use]
    pub fn state_directory(&self) -> &Path {
        &self.state_directory
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct XmrActorProvisionManifestV1 {
    schema_version: u16,
    role: ActorRole,
    swap_id: String,
    stage_a_sha256: String,
    stage_b_sha256: String,
    source_private_root: PathBuf,
    source_private_manifest_sha256: String,
    source_view_key_sha256: String,
    own_public_packet: PathBuf,
    own_public_packet_sha256: String,
    peer_public_packet: PathBuf,
    peer_public_packet_sha256: String,
    role_journal: PathBuf,
    role_journal_sha256: String,
}

/// Publishes one Maker-only application bundle without copying private authority.
///
/// # Errors
///
/// Rejects unsafe or crossed paths, role-private authority drift, noncanonical
/// Stage A/B, an incomplete Maker journal, output collision, and replay drift.
#[allow(clippy::too_many_arguments)]
pub fn provision_xmr_maker_actor_from_material(
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    agreement_stage_a: &Path,
    activation_stage_b: &Path,
    role_journal: &Path,
    output_root: &Path,
) -> Result<XmrActorProvisionV1> {
    provision_xmr_actor_from_material(
        ActorRole::Maker,
        private_root,
        own_public_packet,
        peer_public_packet,
        agreement_stage_a,
        activation_stage_b,
        role_journal,
        output_root,
    )
}

/// Publishes one Taker-only application bundle without copying private authority.
///
/// # Errors
///
/// Rejects unsafe or crossed paths, role-private authority drift, noncanonical
/// Stage A/B, an incomplete Taker journal, output collision, and replay drift.
#[allow(clippy::too_many_arguments)]
pub fn provision_xmr_taker_actor_from_material(
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    agreement_stage_a: &Path,
    activation_stage_b: &Path,
    role_journal: &Path,
    output_root: &Path,
) -> Result<XmrActorProvisionV1> {
    provision_xmr_actor_from_material(
        ActorRole::Taker,
        private_root,
        own_public_packet,
        peer_public_packet,
        agreement_stage_a,
        activation_stage_b,
        role_journal,
        output_root,
    )
}

#[allow(
    clippy::similar_names,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
fn provision_xmr_actor_from_material(
    role: ActorRole,
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    agreement_stage_a: &Path,
    activation_stage_b: &Path,
    role_journal: &Path,
    output_root: &Path,
) -> Result<XmrActorProvisionV1> {
    for path in [
        private_root,
        own_public_packet,
        peer_public_packet,
        agreement_stage_a,
        activation_stage_b,
        role_journal,
        output_root,
    ] {
        ensure!(
            normalized_absolute(path),
            "XMR actor provisioning path is invalid"
        );
    }
    ensure!(
        private_root != output_root && !output_root.starts_with(private_root),
        "XMR actor output aliases private source authority"
    );

    let packets = StageRolePackets::read(role, own_public_packet, peer_public_packet)?;
    let material = validate_private_role(private_root, role, &packets)?;
    let stage_a_wire = read_private_source(
        agreement_stage_a,
        u64::try_from(MAX_XMR_AGREEMENT_WIRE_BYTES).unwrap_or(u64::MAX),
        "signed Stage-A wire",
    )?;
    let agreement =
        read_validated_stage_a(agreement_stage_a, &packets).context("validate signed Stage A")?;
    ensure!(
        agreement.encode_wire().context("encode signed Stage A")? == stage_a_wire,
        "signed Stage-A wire changed"
    );
    let stage_b_wire = read_private_source(
        activation_stage_b,
        u64::try_from(MAX_XMR_ACTIVATION_WIRE_BYTES).unwrap_or(u64::MAX),
        "signed Stage-B wire",
    )?;
    let activation = XmrActivatedAgreementV1::from_wire(&agreement, &stage_b_wire, &material.view)
        .context("signed Stage-B wire is invalid")?;
    let _coordinator = activation
        .initial_coordinator(&agreement)
        .context("derive XMR actor swap identity")?;
    let role_journal_bytes =
        validate_role_journal_snapshot(role, role_journal, &agreement, &activation)?;

    let own_packet_bytes = read_private_source(
        own_public_packet,
        super::ROLE_PACKET_MAX_BYTES,
        "own role packet",
    )?;
    let peer_packet_bytes = read_private_source(
        peer_public_packet,
        super::ROLE_PACKET_MAX_BYTES,
        "peer role packet",
    )?;
    let source_manifest_bytes = read_private_source(
        &private_root.join(PRIVATE_MANIFEST_FILE),
        super::PRIVATE_MANIFEST_MAX_BYTES,
        "private role manifest",
    )?;
    let source_view_key_bytes = read_private_source(
        &private_root.join(VIEW_KEY_FILE),
        PRIVATE_KEY_MAX_BYTES,
        "private Monero view key",
    )?;

    let stage_a_sha256 = sha256(&stage_a_wire);
    let stage_b_sha256 = sha256(&stage_b_wire);
    let manifest = XmrActorProvisionManifestV1 {
        schema_version: APPLICATION_PROVISION_SCHEMA_V1,
        role,
        swap_id: hex::encode(agreement.body().swap_id()),
        stage_a_sha256: hex::encode(stage_a_sha256),
        stage_b_sha256: hex::encode(stage_b_sha256),
        source_private_root: private_root.to_path_buf(),
        source_private_manifest_sha256: hex::encode(sha256(&source_manifest_bytes)),
        source_view_key_sha256: hex::encode(sha256(&source_view_key_bytes)),
        own_public_packet: own_public_packet.to_path_buf(),
        own_public_packet_sha256: hex::encode(sha256(&own_packet_bytes)),
        peer_public_packet: peer_public_packet.to_path_buf(),
        peer_public_packet_sha256: hex::encode(sha256(&peer_packet_bytes)),
        role_journal: role_journal.to_path_buf(),
        role_journal_sha256: hex::encode(sha256(&role_journal_bytes)),
    };
    let manifest_bytes = canonical_manifest_bytes(&manifest)?;
    let manifest_sha256 = sha256(&manifest_bytes);
    let paths = ProvisionPaths::new(output_root, role);
    let prepared = PreparedProvision {
        role,
        swap_id: agreement.body().swap_id(),
        agreement_commitment: agreement.agreement_commitment(),
        activation_commitment: activation.activation_commitment(),
        stage_a_wire,
        stage_a_sha256,
        stage_b_wire,
        stage_b_sha256,
        manifest,
        manifest_bytes,
        manifest_sha256,
    };

    if output_root.exists() {
        validate_exact_replay(&paths, &prepared)?;
        return Ok(prepared.summary(paths, true));
    }

    let destination = SecureDestination::new(output_root, "XMR actor application root")?;
    destination.ensure_absent("XMR actor application root")?;
    let (stage_name, stage) = create_staging_directory(&destination.parent)?;
    let stage_path = destination.parent_path.join(&stage_name);
    let stage_paths = ProvisionPaths::new(&stage_path, role);
    let mut published = false;
    let result = (|| {
        publish_stage(&stage_paths, &stage, &prepared)?;
        destination.revalidate()?;
        renameat_with(
            &destination.parent,
            stage_name.as_str(),
            &destination.parent,
            &destination.name,
            RenameFlags::NOREPLACE,
        )
        .map_err(|error| {
            if error == Errno::EXIST {
                anyhow!("XMR actor application root already exists")
            } else {
                anyhow!("publish XMR actor application root failed")
            }
        })?;
        published = true;
        destination
            .parent
            .sync_all()
            .context("sync XMR actor application parent")?;
        validate_exact_replay(&paths, &prepared)
    })();
    if result.is_err() && !published {
        let _ = fs::remove_dir_all(&stage_path);
        let _ = destination.parent.sync_all();
    }
    result?;
    Ok(prepared.summary(paths, false))
}

struct PreparedProvision {
    role: ActorRole,
    swap_id: [u8; 32],
    stage_a_wire: Vec<u8>,
    agreement_commitment: [u8; 32],
    activation_commitment: [u8; 32],
    stage_a_sha256: [u8; 32],
    stage_b_wire: Vec<u8>,
    stage_b_sha256: [u8; 32],
    manifest: XmrActorProvisionManifestV1,
    manifest_bytes: Vec<u8>,
    manifest_sha256: [u8; 32],
}

impl PreparedProvision {
    fn summary(&self, paths: ProvisionPaths, was_replay: bool) -> XmrActorProvisionV1 {
        XmrActorProvisionV1 {
            schema_version: APPLICATION_PROVISION_SCHEMA_V1,
            was_replay,
            role: self.role,
            swap_id: self.swap_id,
            stage_a_file: paths.stage_a,
            agreement_commitment: self.agreement_commitment,
            activation_commitment: self.activation_commitment,
            stage_a_sha256: self.stage_a_sha256,
            stage_b_file: paths.stage_b,
            stage_b_sha256: self.stage_b_sha256,
            manifest_file: paths.manifest,
            manifest_sha256: self.manifest_sha256,
            state_directory: paths.state,
            state_database: self.manifest.role_journal.clone(),
        }
    }
}

struct ProvisionPaths {
    root: PathBuf,
    shared: PathBuf,
    role_root: PathBuf,
    state: PathBuf,
    stage_a: PathBuf,
    stage_b: PathBuf,
    manifest: PathBuf,
}

impl ProvisionPaths {
    fn new(root: &Path, role: ActorRole) -> Self {
        let shared = root.join("shared");
        let role_root = root.join(role_name(role));
        Self {
            root: root.to_path_buf(),
            stage_a: shared.join(STAGE_A_FILE),
            stage_b: shared.join(STAGE_B_FILE),
            manifest: role_root.join(APPLICATION_MANIFEST_FILE),
            state: role_root.join(APPLICATION_STATE_DIRECTORY),
            shared,
            role_root,
        }
    }
}

fn publish_stage(paths: &ProvisionPaths, stage: &File, prepared: &PreparedProvision) -> Result<()> {
    for path in [&paths.shared, &paths.role_root, &paths.state] {
        DirBuilder::new()
            .mode(0o700)
            .create(path)
            .context("create XMR actor bundle directory")?;
    }
    let shared = File::open(&paths.shared).context("open XMR shared directory")?;
    write_new_at(
        &shared,
        STAGE_A_FILE,
        &prepared.stage_a_wire,
        "signed Stage-A wire",
    )?;
    write_new_at(
        &shared,
        STAGE_B_FILE,
        &prepared.stage_b_wire,
        "signed Stage-B wire",
    )?;
    let role_root = File::open(&paths.role_root).context("open XMR role directory")?;
    write_new_at(
        &role_root,
        APPLICATION_MANIFEST_FILE,
        &prepared.manifest_bytes,
        "XMR actor provision manifest",
    )?;
    for path in [&paths.state, &paths.shared, &paths.role_root] {
        File::open(path)
            .and_then(|directory| directory.sync_all())
            .context("sync XMR actor bundle directory")?;
    }
    stage.sync_all().context("sync XMR actor staging root")
}

fn validate_exact_replay(paths: &ProvisionPaths, prepared: &PreparedProvision) -> Result<()> {
    for directory in [&paths.root, &paths.shared, &paths.role_root, &paths.state] {
        validate_private_directory(directory)?;
    }
    ensure!(
        fs::symlink_metadata(paths.root.join(role_name(prepared.role.opposite()))).is_err(),
        "XMR actor bundle exposes opposite-role authority"
    );
    let stage_a = read_private_source(
        &paths.stage_a,
        u64::try_from(MAX_XMR_AGREEMENT_WIRE_BYTES).unwrap_or(u64::MAX),
        "published Stage-A wire",
    )?;
    let stage_b = read_private_source(
        &paths.stage_b,
        u64::try_from(MAX_XMR_ACTIVATION_WIRE_BYTES).unwrap_or(u64::MAX),
        "published Stage-B wire",
    )?;
    let manifest_bytes = read_private_source(
        &paths.manifest,
        XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "XMR actor provision manifest",
    )?;
    let manifest: XmrActorProvisionManifestV1 =
        serde_json::from_slice(&manifest_bytes).context("XMR actor manifest is malformed")?;
    ensure!(
        canonical_manifest_bytes(&manifest)? == manifest_bytes
            && manifest == prepared.manifest
            && stage_a == prepared.stage_a_wire
            && stage_b == prepared.stage_b_wire,
        "XMR actor provision replay differs from published authority"
    );
    for file in [&paths.stage_a, &paths.stage_b, &paths.manifest] {
        File::open(file)
            .and_then(|opened| opened.sync_all())
            .context("sync published XMR actor file")?;
    }
    for directory in [&paths.state, &paths.shared, &paths.role_root, &paths.root] {
        File::open(directory)
            .and_then(|opened| opened.sync_all())
            .context("sync published XMR actor directory")?;
    }
    File::open(
        paths
            .root
            .parent()
            .ok_or_else(|| anyhow!("XMR actor root has no parent"))?,
    )
    .and_then(|opened| opened.sync_all())
    .context("sync XMR actor parent")
}

fn validate_role_journal_snapshot(
    role: ActorRole,
    path: &Path,
    agreement: &XmrAgreementV1,
    activation: &XmrActivatedAgreementV1,
) -> Result<Vec<u8>> {
    validate_no_journal_sidecars(path)?;
    let source_bytes = read_private_source(path, APPLICATION_JOURNAL_MAX_BYTES, "role journal")?;
    let destination = SecureDestination::new(path, "role journal")?;
    destination.revalidate()?;
    let (stage_name, stage_file) = create_staged_file(
        &destination.parent,
        "journal-check",
        &source_bytes,
        APPLICATION_JOURNAL_MAX_BYTES,
        "role journal validation snapshot",
    )?;
    drop(stage_file);
    let stage_path = destination.parent_path.join(&stage_name);
    let validation = validate_role_journal(role, &stage_path, agreement, activation);
    for suffix in ["-wal", "-shm", "-journal"] {
        let _ = fs::remove_file(journal_sidecar_path(&stage_path, suffix));
    }
    cleanup_staged_file(&destination.parent, &stage_name);
    validation?;

    destination.revalidate()?;
    validate_no_journal_sidecars(path)?;
    let after = read_private_source(path, APPLICATION_JOURNAL_MAX_BYTES, "role journal")?;
    ensure!(
        source_bytes == after,
        "role journal changed while its private snapshot was validated"
    );
    validate_no_journal_sidecars(path)?;
    Ok(source_bytes)
}

fn validate_role_journal(
    role: ActorRole,
    path: &Path,
    agreement: &XmrAgreementV1,
    activation: &XmrActivatedAgreementV1,
) -> Result<()> {
    let claim = ValidatedSession::from_untweaked_context(
        agreement
            .claim_session_descriptor()
            .context()
            .context("claim descriptor is invalid")?,
    )
    .context("claim session is invalid")?;
    let refund = ValidatedSession::from_untweaked_context(
        agreement
            .refund_session_descriptor()
            .context()
            .context("refund descriptor is invalid")?,
    )
    .context("refund session is invalid")?;
    let runner_role = match role {
        ActorRole::Maker => RunnerRole::Maker,
        ActorRole::Taker => RunnerRole::Taker,
    };
    let journal = SqliteAdaptorSessionJournal::open_existing(path)
        .context("role journal is unavailable or unsafe")?;
    let claim_identity = claim.identity(runner_role);
    let refund_identity = refund.identity(runner_role);
    let claim_snapshot = journal
        .load(claim_identity.session_id())
        .context("load role claim journal")?
        .ok_or_else(|| anyhow!("role claim journal is incomplete"))?;
    let refund_snapshot = journal
        .load(refund_identity.session_id())
        .context("load role refund journal")?
        .ok_or_else(|| anyhow!("role refund journal is incomplete"))?;
    ensure!(
        claim_snapshot.identity() == &claim_identity,
        "role claim journal identity mismatch"
    );
    ensure!(
        refund_snapshot.identity() == &refund_identity,
        "role refund journal identity mismatch"
    );
    validate_snapshot_transcript(
        role,
        &claim_snapshot,
        activation.body().claim_transcript(),
        "claim",
    )?;
    validate_snapshot_transcript(
        role,
        &refund_snapshot,
        activation.body().refund_transcript(),
        "refund",
    )?;
    match role {
        ActorRole::Maker => ensure!(
            claim_snapshot.phase() == AdaptorSessionPhase::PartialPersisted
                && claim_snapshot.own_partial().is_some_and(|partial| {
                    *partial.bytes() == activation.body().maker_claim_partial()
                }),
            "Maker claim journal differs from Stage B"
        ),
        ActorRole::Taker => {
            let partial = claim_snapshot
                .own_partial()
                .ok_or_else(|| anyhow!("Taker claim journal is incomplete"))?;
            ensure!(
                claim_snapshot.phase() == AdaptorSessionPhase::PresignatureVerified,
                "Taker claim journal is incomplete"
            );
            activation
                .verify_published_taker_claim_partial(agreement, *partial.bytes())
                .context("Taker claim journal differs from Stage B")?;
        }
    }
    ensure!(
        refund_snapshot.phase() == AdaptorSessionPhase::PresignatureVerified
            && refund_snapshot
                .presignature()
                .is_some_and(|value| *value.bytes() == activation.body().refund_presignature()),
        "role refund journal differs from Stage B"
    );
    let (expected_own, expected_peer) = match role {
        ActorRole::Maker => (
            activation.body().maker_refund_partial(),
            activation.body().taker_refund_partial(),
        ),
        ActorRole::Taker => (
            activation.body().taker_refund_partial(),
            activation.body().maker_refund_partial(),
        ),
    };
    ensure!(
        refund_snapshot
            .own_partial()
            .is_some_and(|value| *value.bytes() == expected_own)
            && refund_snapshot
                .peer_partial()
                .is_some_and(|value| *value.bytes() == expected_peer),
        "role refund journal partials differ from Stage B"
    );
    Ok(())
}

fn validate_snapshot_transcript(
    role: ActorRole,
    snapshot: &AdaptorSessionSnapshot,
    transcript: &XmrSessionTranscriptV1,
    label: &'static str,
) -> Result<()> {
    let own_commitment = snapshot.own_commitment();
    let peer_commitment = snapshot
        .peer_commitment()
        .ok_or_else(|| anyhow!("role {label} journal is incomplete"))?;
    let own_nonce = snapshot
        .own_public_nonce()
        .ok_or_else(|| anyhow!("role {label} journal is incomplete"))?;
    let peer_nonce = snapshot
        .peer_public_nonce()
        .ok_or_else(|| anyhow!("role {label} journal is incomplete"))?;
    let (
        expected_own_commitment,
        expected_peer_commitment,
        expected_own_nonce,
        expected_peer_nonce,
    ) = match role {
        ActorRole::Maker => (
            transcript.maker_nonce_commitment(),
            transcript.taker_nonce_commitment(),
            transcript.maker_public_nonce(),
            transcript.taker_public_nonce(),
        ),
        ActorRole::Taker => (
            transcript.taker_nonce_commitment(),
            transcript.maker_nonce_commitment(),
            transcript.taker_public_nonce(),
            transcript.maker_public_nonce(),
        ),
    };
    ensure!(
        *own_commitment.bytes() == expected_own_commitment
            && *peer_commitment.bytes() == expected_peer_commitment
            && *own_nonce.bytes() == expected_own_nonce
            && *peer_nonce.bytes() == expected_peer_nonce,
        "role {label} journal transcript differs from Stage B"
    );
    Ok(())
}

/// Validates one canonical Maker provision manifest against scheduler authority.
///
/// This byte-only boundary lets a daemon validate already securely read and
/// digest-pinned config without reopening the path. It exposes no private
/// material and does not grant chain-effect authority.
///
/// # Errors
///
/// Rejects an oversized, malformed, noncanonical, unsupported, non-Maker,
/// path-unsafe, wrong-swap, wrong-state-database, or malformed-digest manifest.
pub fn validate_maker_manifest_config_bytes(
    bytes: &[u8],
    expected_swap_id: [u8; 32],
    expected_state_database: &Path,
) -> Result<()> {
    ensure!(
        u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "XMR actor provision manifest is oversized"
    );
    ensure!(
        normalized_absolute(expected_state_database),
        "expected XMR actor state database path is invalid"
    );
    let manifest: XmrActorProvisionManifestV1 =
        serde_json::from_slice(bytes).context("XMR actor manifest is malformed")?;
    ensure!(
        canonical_manifest_bytes(&manifest)? == bytes,
        "XMR actor manifest is noncanonical"
    );
    ensure!(
        manifest.schema_version == APPLICATION_PROVISION_SCHEMA_V1
            && manifest.role == ActorRole::Maker
            && super::decode_exact::<32>(&manifest.swap_id)? == expected_swap_id
            && manifest.role_journal == expected_state_database,
        "XMR Maker actor manifest differs from scheduler authority"
    );
    for path in [
        &manifest.source_private_root,
        &manifest.own_public_packet,
        &manifest.peer_public_packet,
        &manifest.role_journal,
    ] {
        ensure!(
            normalized_absolute(path),
            "XMR Maker actor manifest path is invalid"
        );
    }
    for digest in [
        &manifest.stage_a_sha256,
        &manifest.stage_b_sha256,
        &manifest.source_private_manifest_sha256,
        &manifest.source_view_key_sha256,
        &manifest.own_public_packet_sha256,
        &manifest.peer_public_packet_sha256,
        &manifest.role_journal_sha256,
    ] {
        let _ = super::decode_exact::<32>(digest)?;
    }
    Ok(())
}

fn journal_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut candidate = path.as_os_str().to_os_string();
    candidate.push(suffix);
    PathBuf::from(candidate)
}

fn validate_no_journal_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut candidate = path.as_os_str().to_os_string();
        candidate.push(suffix);
        match fs::symlink_metadata(PathBuf::from(candidate)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(anyhow!(
                    "role journal has uncheckpointed or concurrently owned SQLite state"
                ));
            }
            Err(_) => {
                return Err(anyhow!(
                    "role journal auxiliary state cannot be inspected safely"
                ));
            }
        }
    }
    Ok(())
}

fn read_private_source(path: &Path, max_bytes: u64, label: &'static str) -> Result<Vec<u8>> {
    let file = open_path_no_symlinks(path, label)?;
    read_bounded_file(file, max_bytes, FilePolicy::Private, label)
}

fn validate_private_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("inspect XMR actor directory")?;
    let canonical = fs::canonicalize(path).context("canonicalize XMR actor directory")?;
    ensure!(
        metadata.file_type().is_dir()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.permissions().mode() & 0o7777 == 0o700
            && canonical == path,
        "XMR actor directory is unavailable or unsafe"
    );
    Ok(())
}

fn normalized_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn canonical_manifest_bytes(manifest: &XmrActorProvisionManifestV1) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(manifest).context("encode XMR actor provision manifest")?;
    bytes.push(b'\n');
    ensure!(
        u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "XMR actor provision manifest is oversized"
    );
    Ok(bytes)
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

const fn role_name(role: ActorRole) -> &'static str {
    match role {
        ActorRole::Maker => "maker",
        ActorRole::Taker => "taker",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> XmrActorProvisionManifestV1 {
        XmrActorProvisionManifestV1 {
            schema_version: APPLICATION_PROVISION_SCHEMA_V1,
            role: ActorRole::Maker,
            swap_id: "11".repeat(32),
            stage_a_sha256: "21".repeat(32),
            stage_b_sha256: "22".repeat(32),
            source_private_root: PathBuf::from("/private/maker"),
            source_private_manifest_sha256: "23".repeat(32),
            source_view_key_sha256: "24".repeat(32),
            own_public_packet: PathBuf::from("/exchange/maker.json"),
            own_public_packet_sha256: "25".repeat(32),
            peer_public_packet: PathBuf::from("/exchange/taker.json"),
            peer_public_packet_sha256: "26".repeat(32),
            role_journal: PathBuf::from("/private/journals/maker.sqlite"),
            role_journal_sha256: "27".repeat(32),
        }
    }

    #[test]
    fn maker_manifest_validator_binds_role_swap_state_paths_and_canonical_bytes() {
        let expected_swap = [0x11; 32];
        let state = Path::new("/private/journals/maker.sqlite");
        let valid = canonical_manifest_bytes(&manifest()).expect("canonical manifest");
        validate_maker_manifest_config_bytes(&valid, expected_swap, state)
            .expect("valid Maker manifest");

        let mut invalid = Vec::new();
        let mut value = manifest();
        value.role = ActorRole::Taker;
        invalid.push(value);
        let mut value = manifest();
        value.schema_version += 1;
        invalid.push(value);
        let mut value = manifest();
        value.swap_id = "12".repeat(32);
        invalid.push(value);
        let mut value = manifest();
        value.role_journal = PathBuf::from("/private/journals/other.sqlite");
        invalid.push(value);
        let mut value = manifest();
        value.source_private_root = PathBuf::from("relative/private");
        invalid.push(value);
        let mut value = manifest();
        value.stage_b_sha256 = "AA".repeat(32);
        invalid.push(value);
        for value in invalid {
            let bytes = canonical_manifest_bytes(&value).expect("canonical invalid manifest");
            assert!(validate_maker_manifest_config_bytes(&bytes, expected_swap, state).is_err());
        }

        let mut noncanonical = valid;
        noncanonical.push(b' ');
        assert!(validate_maker_manifest_config_bytes(&noncanonical, expected_swap, state).is_err());
        assert!(
            validate_maker_manifest_config_bytes(
                &canonical_manifest_bytes(&manifest()).unwrap(),
                expected_swap,
                Path::new("relative.sqlite"),
            )
            .is_err()
        );
    }
}
