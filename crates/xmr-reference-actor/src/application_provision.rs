//! Role-fixed, no-copy XMR application actor provisioning.

use std::{
    fmt,
    fs::{self, DirBuilder, File},
    io::{Read as _, Seek as _, SeekFrom},
    os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context as _, Result, anyhow, ensure};
use lez_adaptor_role_runner::{Role as RunnerRole, ValidatedSession};
use lez_swap_core::{Participant, SwapId};
use lez_swap_store::{
    AdaptorSessionPhase, AdaptorSessionSnapshot, MAKER_ACTOR_CONFIG_FD,
    SqliteAdaptorSessionJournal, SqliteXmrWorkflowJournal, StoreError, XmrWorkflowIdentityV1,
};
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES, XmrActivatedAgreementV1,
    XmrAgreementV1, XmrSessionTranscriptV1,
};
use rustix::{
    fs::{RenameFlags, SealFlags, fcntl_get_seals, renameat_with},
    io::Errno,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::{
    ActorRole, FilePolicy, PRIVATE_KEY_MAX_BYTES, PRIVATE_MANIFEST_FILE, SecureDestination,
    StageRolePackets, VIEW_KEY_FILE, cleanup_staged_file, create_staged_file,
    create_staging_directory,
    effect_authority::{
        MAX_AUTHORITY_BYTES, ValidatedXmrEffectAuthorityV1,
        load_validated_xmr_effect_authority_bytes,
    },
    open_path_no_symlinks, read_bounded_file, read_validated_stage_a, validate_private_role,
    write_new_at,
};

const APPLICATION_PROVISION_SCHEMA_V2: u16 = 2;
const APPLICATION_MANIFEST_FILE: &str = "actor-provision.json";
const STAGE_A_FILE: &str = "stage-a-v1.borsh";
const STAGE_B_FILE: &str = "stage-b-v1.borsh";
const APPLICATION_STATE_DIRECTORY: &str = "state";
const APPLICATION_JOURNAL_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Maximum accepted canonical role-authority manifest size.
pub const XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES: u64 = 16 * 1024;

/// Program identifier pinned by the Maker supervisor and one-shot child.
pub const XMR_MAKER_ACTOR_PROGRAM_ID: &str = "xmr-maker-actor";
/// Pre-effect ABI pinned by the Maker supervisor and one-shot child.
pub const XMR_MAKER_ACTOR_ABI_V1: &str = "lez_maker_xmr_pre_effect_v1";
/// Current bounded action: validate authority without publishing a chain effect.
pub const XMR_MAKER_ACTOR_NEXT_ACTION: &str = "xmr_chain_effects_not_yet_composed";
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
/// Secret-free summary of one replay-safe schema-v3 effect provisioning.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[must_use]
pub struct XmrEffectProvisionV3 {
    was_replay: bool,
    role: ActorRole,
    swap_id: [u8; 32],
    run_id: Box<str>,
    agreement_commitment: [u8; 32],
    activation_commitment: [u8; 32],
    manifest_file: PathBuf,
    manifest_sha256: [u8; 32],
    effect_authority_file: PathBuf,
    effect_authority_sha256: [u8; 32],
    workflow_journal: PathBuf,
}

/// Semantically validated schema-v3 authority ready for one workflow route.
#[must_use]
pub struct ValidatedXmrEffectExecutionV3 {
    pub(crate) effect: ValidatedXmrEffectAuthorityV1,
    pub(crate) workflow_identity: XmrWorkflowIdentityV1,
    pub(crate) effect_authority_sha256: [u8; 32],
}

impl fmt::Debug for ValidatedXmrEffectExecutionV3 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedXmrEffectExecutionV3")
            .field("effect", &self.effect)
            .field("workflow_identity", &self.workflow_identity)
            .field(
                "effect_authority_sha256",
                &hex::encode(self.effect_authority_sha256),
            )
            .finish_non_exhaustive()
    }
}

impl ValidatedXmrEffectExecutionV3 {
    /// Fully validated role-fixed effect authority.
    pub const fn effect_authority(&self) -> &ValidatedXmrEffectAuthorityV1 {
        &self.effect
    }

    /// Exact durable workflow identity reconstructed from schema v3.
    pub const fn workflow_identity(&self) -> &XmrWorkflowIdentityV1 {
        &self.workflow_identity
    }

    /// SHA-256 of the exact immutable effect-authority bytes.
    #[must_use]
    pub const fn effect_authority_sha256(&self) -> [u8; 32] {
        self.effect_authority_sha256
    }
}

impl XmrEffectProvisionV3 {
    /// Whether an existing byte-identical schema-v3 manifest was reused.
    #[must_use]
    pub const fn was_replay(&self) -> bool {
        self.was_replay
    }

    /// Role permanently bound to the effect authority.
    #[must_use]
    pub const fn role(&self) -> ActorRole {
        self.role
    }

    /// Exact accepted application swap.
    #[must_use]
    pub const fn swap_id(&self) -> [u8; 32] {
        self.swap_id
    }

    /// Exact application run identity.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Exact countersigned agreement commitment.
    #[must_use]
    pub const fn agreement_commitment(&self) -> [u8; 32] {
        self.agreement_commitment
    }

    /// Exact countersigned activation commitment.
    #[must_use]
    pub const fn activation_commitment(&self) -> [u8; 32] {
        self.activation_commitment
    }

    /// Canonical schema-v3 manifest path.
    #[must_use]
    pub fn manifest_file(&self) -> &Path {
        &self.manifest_file
    }

    /// SHA-256 of the exact schema-v3 manifest bytes.
    #[must_use]
    pub const fn manifest_sha256(&self) -> [u8; 32] {
        self.manifest_sha256
    }

    /// Immutable effect-authority path.
    #[must_use]
    pub fn effect_authority_file(&self) -> &Path {
        &self.effect_authority_file
    }

    /// SHA-256 of the exact effect-authority bytes.
    #[must_use]
    pub const fn effect_authority_sha256(&self) -> [u8; 32] {
        self.effect_authority_sha256
    }

    /// Separate mutable workflow-journal path.
    #[must_use]
    pub fn workflow_journal(&self) -> &Path {
        &self.workflow_journal
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct XmrActorProvisionManifestV2 {
    schema_version: u16,
    role: ActorRole,
    swap_id: String,
    published_stage_a: PathBuf,
    stage_a_sha256: String,
    published_stage_b: PathBuf,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct XmrActorProvisionManifestV3 {
    schema_version: u16,
    role: ActorRole,
    swap_id: String,
    run_id: String,
    published_stage_a: PathBuf,
    stage_a_sha256: String,
    published_stage_b: PathBuf,
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
    effect_authority_file: PathBuf,
    effect_authority_sha256: String,
    workflow_journal: PathBuf,
}

impl XmrActorProvisionManifestV3 {
    fn legacy(&self) -> XmrActorProvisionManifestV2 {
        XmrActorProvisionManifestV2 {
            schema_version: APPLICATION_PROVISION_SCHEMA_V2,
            role: self.role,
            swap_id: self.swap_id.clone(),
            published_stage_a: self.published_stage_a.clone(),
            stage_a_sha256: self.stage_a_sha256.clone(),
            published_stage_b: self.published_stage_b.clone(),
            stage_b_sha256: self.stage_b_sha256.clone(),
            source_private_root: self.source_private_root.clone(),
            source_private_manifest_sha256: self.source_private_manifest_sha256.clone(),
            source_view_key_sha256: self.source_view_key_sha256.clone(),
            own_public_packet: self.own_public_packet.clone(),
            own_public_packet_sha256: self.own_public_packet_sha256.clone(),
            peer_public_packet: self.peer_public_packet.clone(),
            peer_public_packet_sha256: self.peer_public_packet_sha256.clone(),
            role_journal: self.role_journal.clone(),
            role_journal_sha256: self.role_journal_sha256.clone(),
        }
    }
}

/// Execution-time Maker authority derived from a fully sealed schema-v2 manifest.
///
/// The journal bytes are intentionally private: successful construction proves
/// their semantics against the pinned Stage A/B and retains the exact immutable
/// snapshot for this one-shot pre-effect invocation.
#[must_use]
pub struct ValidatedXmrMakerAuthorityV2 {
    swap_id: [u8; 32],
    state_database: PathBuf,
    agreement_commitment: [u8; 32],
    activation_commitment: [u8; 32],
    published_stage_a: PathBuf,
    stage_a_sha256: [u8; 32],
    published_stage_b: PathBuf,
    stage_b_sha256: [u8; 32],
    _role_journal_snapshot: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for ValidatedXmrMakerAuthorityV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedXmrMakerAuthorityV2")
            .field("swap_id", &hex::encode(self.swap_id))
            .field("state_database", &self.state_database)
            .field(
                "agreement_commitment",
                &hex::encode(self.agreement_commitment),
            )
            .field(
                "activation_commitment",
                &hex::encode(self.activation_commitment),
            )
            .field("published_stage_a", &self.published_stage_a)
            .field("stage_a_sha256", &hex::encode(self.stage_a_sha256))
            .field("published_stage_b", &self.published_stage_b)
            .field("stage_b_sha256", &hex::encode(self.stage_b_sha256))
            .field("_role_journal_snapshot", &"[REDACTED]")
            .finish()
    }
}

impl ValidatedXmrMakerAuthorityV2 {
    /// Exact signed Stage-A swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> [u8; 32] {
        self.swap_id
    }

    /// Exact role-local state database named by scheduler authority.
    #[must_use]
    pub fn state_database(&self) -> &Path {
        &self.state_database
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

    /// Canonical published Stage-A path validated from the manifest.
    #[must_use]
    pub fn published_stage_a(&self) -> &Path {
        &self.published_stage_a
    }

    /// SHA-256 of the exact validated Stage-A wire.
    #[must_use]
    pub const fn stage_a_sha256(&self) -> [u8; 32] {
        self.stage_a_sha256
    }

    /// Canonical published Stage-B path validated from the manifest.
    #[must_use]
    pub fn published_stage_b(&self) -> &Path {
        &self.published_stage_b
    }

    /// SHA-256 of the exact validated Stage-B wire.
    #[must_use]
    pub const fn stage_b_sha256(&self) -> [u8; 32] {
        self.stage_b_sha256
    }
}

/// Execution-time Taker authority derived from canonical schema-v2 manifest bytes.
///
/// The caller must securely read and digest-pin the canonical manifest before
/// calling the byte boundary. Construction then fully validates every source
/// named by that manifest and privately retains the exact adaptor-journal
/// snapshot used for the semantic checks.
#[must_use]
pub struct ValidatedXmrTakerAuthorityV2 {
    swap_id: [u8; 32],
    state_database: PathBuf,
    agreement_commitment: [u8; 32],
    activation_commitment: [u8; 32],
    published_stage_a: PathBuf,
    stage_a_sha256: [u8; 32],
    published_stage_b: PathBuf,
    stage_b_sha256: [u8; 32],
    _role_journal_snapshot: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for ValidatedXmrTakerAuthorityV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedXmrTakerAuthorityV2")
            .field("swap_id", &hex::encode(self.swap_id))
            .field("state_database", &self.state_database)
            .field(
                "agreement_commitment",
                &hex::encode(self.agreement_commitment),
            )
            .field(
                "activation_commitment",
                &hex::encode(self.activation_commitment),
            )
            .field("published_stage_a", &self.published_stage_a)
            .field("stage_a_sha256", &hex::encode(self.stage_a_sha256))
            .field("published_stage_b", &self.published_stage_b)
            .field("stage_b_sha256", &hex::encode(self.stage_b_sha256))
            .field("_role_journal_snapshot", &"[REDACTED]")
            .finish()
    }
}

impl ValidatedXmrTakerAuthorityV2 {
    /// Exact signed Stage-A swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> [u8; 32] {
        self.swap_id
    }

    /// Exact role-local state database named by scheduler authority.
    #[must_use]
    pub fn state_database(&self) -> &Path {
        &self.state_database
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

    /// Canonical published Stage-A path validated from the manifest.
    #[must_use]
    pub fn published_stage_a(&self) -> &Path {
        &self.published_stage_a
    }

    /// SHA-256 of the exact validated Stage-A wire.
    #[must_use]
    pub const fn stage_a_sha256(&self) -> [u8; 32] {
        self.stage_a_sha256
    }

    /// Canonical published Stage-B path validated from the manifest.
    #[must_use]
    pub fn published_stage_b(&self) -> &Path {
        &self.published_stage_b
    }

    /// SHA-256 of the exact validated Stage-B wire.
    #[must_use]
    pub const fn stage_b_sha256(&self) -> [u8; 32] {
        self.stage_b_sha256
    }
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
    let source_view_key_bytes = Zeroizing::new(read_private_source(
        &private_root.join(VIEW_KEY_FILE),
        PRIVATE_KEY_MAX_BYTES,
        "private Monero view key",
    )?);
    let paths = ProvisionPaths::new(output_root, role);

    let stage_a_sha256 = sha256(&stage_a_wire);
    let stage_b_sha256 = sha256(&stage_b_wire);
    let manifest = XmrActorProvisionManifestV2 {
        schema_version: APPLICATION_PROVISION_SCHEMA_V2,
        role,
        swap_id: hex::encode(agreement.body().swap_id()),
        published_stage_a: paths.stage_a.clone(),
        stage_a_sha256: hex::encode(stage_a_sha256),
        published_stage_b: paths.stage_b.clone(),
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
    manifest: XmrActorProvisionManifestV2,
    manifest_bytes: Vec<u8>,
    manifest_sha256: [u8; 32],
}

impl PreparedProvision {
    fn summary(&self, paths: ProvisionPaths, was_replay: bool) -> XmrActorProvisionV1 {
        XmrActorProvisionV1 {
            schema_version: APPLICATION_PROVISION_SCHEMA_V2,
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
    let manifest: XmrActorProvisionManifestV2 =
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
        normalized_absolute(expected_state_database),
        "expected XMR actor state database path is invalid"
    );
    let manifest = parse_maker_manifest_config_bytes(bytes)?;
    ensure!(
        super::decode_exact::<32>(&manifest.swap_id)? == expected_swap_id
            && manifest.role_journal == expected_state_database,
        "XMR Maker actor manifest differs from scheduler authority"
    );
    Ok(())
}

/// Validates one canonical Taker provision manifest against receipt authority.
///
/// This bounded byte-only check binds the receipt-selected swap and database
/// before the caller creates or acquires the role-state lock.
///
/// # Errors
///
/// Rejects an oversized, malformed, noncanonical, unsupported, non-Taker,
/// path-unsafe, wrong-swap, wrong-state-database, or malformed-digest manifest.
pub fn validate_taker_manifest_config_bytes(
    bytes: &[u8],
    expected_swap_id: [u8; 32],
    expected_state_database: &Path,
) -> Result<()> {
    ensure!(
        normalized_absolute(expected_state_database),
        "expected XMR actor state database path is invalid"
    );
    let manifest = parse_manifest_config_bytes(bytes, ActorRole::Taker)?;
    ensure!(
        decode_canonical_exact_32(&manifest.swap_id)? == expected_swap_id
            && manifest.role_journal == expected_state_database,
        "XMR Taker actor manifest differs from receipt authority"
    );
    Ok(())
}

/// Loads and fully validates Maker authority from fixed sealed descriptor 196.
///
/// Every authority file is securely reread, digest-pinned, and semantically
/// checked. The result privately owns the exact validated journal snapshot, so
/// later mutation of the named `SQLite` path cannot affect this invocation.
///
/// # Errors
///
/// Rejects any descriptor other than 196, incomplete memfd seals, unsafe or
/// changed authority files, digest drift, role crossing, or Stage A/B/journal
/// semantic mismatch.
#[allow(clippy::similar_names, clippy::too_many_lines)]
// The complete immutable-authority validation is one deliberate audit surface.
pub fn load_validated_xmr_maker_authority_fd(fd: i32) -> Result<ValidatedXmrMakerAuthorityV2> {
    let config = read_sealed_config_fd(fd)?;
    let authority = load_validated_xmr_role_authority_bytes(config.as_slice(), ActorRole::Maker)?;

    Ok(ValidatedXmrMakerAuthorityV2 {
        swap_id: authority.swap_id,
        state_database: authority.state_database,
        agreement_commitment: authority.agreement_commitment,
        activation_commitment: authority.activation_commitment,
        published_stage_a: authority.published_stage_a,
        stage_a_sha256: authority.stage_a_sha256,
        published_stage_b: authority.published_stage_b,
        stage_b_sha256: authority.stage_b_sha256,
        _role_journal_snapshot: authority.role_journal_snapshot,
    })
}

/// Loads and fully validates Taker authority from canonical manifest bytes.
///
/// The caller owns the path boundary: these bytes must already have been read
/// securely and digest-pinned to scheduler authority. This function validates
/// canonical schema-v2 Taker authority, every pinned source, Stage A/B, private
/// role material, and both Taker adaptor sessions, then revalidates all sources.
///
/// # Errors
///
/// Rejects oversized, malformed, noncanonical, non-Taker, unsafe, changed, or
/// digest-drifted authority and any Stage A/B/private-role/journal mismatch.
#[allow(clippy::similar_names, clippy::too_many_lines)]
// The complete immutable-authority validation is one deliberate audit surface.
pub fn load_validated_xmr_taker_authority_bytes(
    bytes: &[u8],
) -> Result<ValidatedXmrTakerAuthorityV2> {
    let authority = load_validated_xmr_role_authority_bytes(bytes, ActorRole::Taker)?;

    Ok(ValidatedXmrTakerAuthorityV2 {
        swap_id: authority.swap_id,
        state_database: authority.state_database,
        agreement_commitment: authority.agreement_commitment,
        activation_commitment: authority.activation_commitment,
        published_stage_a: authority.published_stage_a,
        stage_a_sha256: authority.stage_a_sha256,
        published_stage_b: authority.published_stage_b,
        stage_b_sha256: authority.stage_b_sha256,
        _role_journal_snapshot: authority.role_journal_snapshot,
    })
}

struct ValidatedXmrRoleAuthorityV2 {
    swap_id: [u8; 32],
    state_database: PathBuf,
    agreement_commitment: [u8; 32],
    activation_commitment: [u8; 32],
    published_stage_a: PathBuf,
    stage_a_sha256: [u8; 32],
    published_stage_b: PathBuf,
    stage_b_sha256: [u8; 32],
    role_journal_snapshot: Zeroizing<Vec<u8>>,
}

#[allow(clippy::similar_names, clippy::too_many_lines)]
fn load_validated_xmr_role_authority_bytes(
    bytes: &[u8],
    role: ActorRole,
) -> Result<ValidatedXmrRoleAuthorityV2> {
    let manifest = parse_manifest_config_bytes(bytes, role)?;
    let (own_packet_label, peer_packet_label, private_manifest_label, private_view_key_label) =
        match role {
            ActorRole::Maker => (
                "Maker public role packet",
                "Taker public role packet",
                "Maker private role manifest",
                "Maker private Monero view key",
            ),
            ActorRole::Taker => (
                "Taker public role packet",
                "Maker public role packet",
                "Taker private role manifest",
                "Taker private Monero view key",
            ),
        };

    let own_packet = read_pinned_private_source(
        &manifest.own_public_packet,
        super::ROLE_PACKET_MAX_BYTES,
        own_packet_label,
        &manifest.own_public_packet_sha256,
    )?;
    let peer_packet = read_pinned_private_source(
        &manifest.peer_public_packet,
        super::ROLE_PACKET_MAX_BYTES,
        peer_packet_label,
        &manifest.peer_public_packet_sha256,
    )?;
    let source_manifest_path = manifest.source_private_root.join(PRIVATE_MANIFEST_FILE);
    let source_view_key_path = manifest.source_private_root.join(VIEW_KEY_FILE);
    let source_manifest = read_pinned_private_source(
        &source_manifest_path,
        super::PRIVATE_MANIFEST_MAX_BYTES,
        private_manifest_label,
        &manifest.source_private_manifest_sha256,
    )?;
    let source_view_key = Zeroizing::new(read_pinned_private_source(
        &source_view_key_path,
        PRIVATE_KEY_MAX_BYTES,
        private_view_key_label,
        &manifest.source_view_key_sha256,
    )?);
    let stage_a_wire = read_pinned_private_source(
        &manifest.published_stage_a,
        u64::try_from(MAX_XMR_AGREEMENT_WIRE_BYTES).unwrap_or(u64::MAX),
        "published Stage-A wire",
        &manifest.stage_a_sha256,
    )?;
    let stage_b_wire = read_pinned_private_source(
        &manifest.published_stage_b,
        u64::try_from(MAX_XMR_ACTIVATION_WIRE_BYTES).unwrap_or(u64::MAX),
        "published Stage-B wire",
        &manifest.stage_b_sha256,
    )?;

    let packets = StageRolePackets::read(
        role,
        &manifest.own_public_packet,
        &manifest.peer_public_packet,
    )?;
    let material = validate_private_role(&manifest.source_private_root, role, &packets)?;
    let agreement = read_validated_stage_a(&manifest.published_stage_a, &packets)
        .context("validate published Stage A")?;
    ensure!(
        agreement
            .encode_wire()
            .context("encode published Stage A")?
            == stage_a_wire,
        "published Stage-A wire changed"
    );
    let activation = XmrActivatedAgreementV1::from_wire(&agreement, &stage_b_wire, &material.view)
        .context("published Stage-B wire is invalid")?;
    let _coordinator = activation
        .initial_coordinator(&agreement)
        .context("derive XMR actor swap identity")?;
    ensure!(
        agreement.body().swap_id() == super::decode_exact::<32>(&manifest.swap_id)?,
        "published Stage A differs from XMR actor manifest"
    );

    let journal_snapshot =
        validate_role_journal_snapshot(role, &manifest.role_journal, &agreement, &activation)?;
    ensure!(
        sha256(&journal_snapshot) == super::decode_exact::<32>(&manifest.role_journal_sha256)?,
        "role journal digest differs from provision manifest"
    );

    revalidate_exact_private_source(
        &manifest.own_public_packet,
        super::ROLE_PACKET_MAX_BYTES,
        own_packet_label,
        &own_packet,
    )?;
    revalidate_exact_private_source(
        &manifest.peer_public_packet,
        super::ROLE_PACKET_MAX_BYTES,
        peer_packet_label,
        &peer_packet,
    )?;
    revalidate_exact_private_source(
        &source_manifest_path,
        super::PRIVATE_MANIFEST_MAX_BYTES,
        private_manifest_label,
        &source_manifest,
    )?;
    revalidate_exact_private_source(
        &source_view_key_path,
        PRIVATE_KEY_MAX_BYTES,
        private_view_key_label,
        &source_view_key,
    )?;
    revalidate_exact_private_source(
        &manifest.published_stage_a,
        u64::try_from(MAX_XMR_AGREEMENT_WIRE_BYTES).unwrap_or(u64::MAX),
        "published Stage-A wire",
        &stage_a_wire,
    )?;
    revalidate_exact_private_source(
        &manifest.published_stage_b,
        u64::try_from(MAX_XMR_ACTIVATION_WIRE_BYTES).unwrap_or(u64::MAX),
        "published Stage-B wire",
        &stage_b_wire,
    )?;

    Ok(ValidatedXmrRoleAuthorityV2 {
        swap_id: agreement.body().swap_id(),
        state_database: manifest.role_journal,
        agreement_commitment: agreement.agreement_commitment(),
        activation_commitment: activation.activation_commitment(),
        published_stage_a: manifest.published_stage_a,
        stage_a_sha256: sha256(&stage_a_wire),
        published_stage_b: manifest.published_stage_b,
        stage_b_sha256: sha256(&stage_b_wire),
        role_journal_snapshot: Zeroizing::new(journal_snapshot),
    })
}

fn promote_effect_manifest_v3(
    legacy: XmrActorProvisionManifestV2,
    effect_authority_file: PathBuf,
    effect_authority_sha256: String,
    run_id: String,
    workflow_journal: PathBuf,
) -> Result<XmrActorProvisionManifestV3> {
    ensure!(
        normalized_absolute(&effect_authority_file)
            && normalized_absolute(&workflow_journal)
            && crate::effect_authority::valid_label(&run_id)
            && workflow_journal != legacy.role_journal
            && effect_authority_file != workflow_journal
            && effect_authority_file != legacy.role_journal,
        "XMR effect manifest paths are invalid"
    );
    decode_canonical_exact_32(&effect_authority_sha256)?;
    Ok(XmrActorProvisionManifestV3 {
        schema_version: 3,
        role: legacy.role,
        swap_id: legacy.swap_id,
        run_id,
        published_stage_a: legacy.published_stage_a,
        stage_a_sha256: legacy.stage_a_sha256,
        published_stage_b: legacy.published_stage_b,
        stage_b_sha256: legacy.stage_b_sha256,
        source_private_root: legacy.source_private_root,
        source_private_manifest_sha256: legacy.source_private_manifest_sha256,
        source_view_key_sha256: legacy.source_view_key_sha256,
        own_public_packet: legacy.own_public_packet,
        own_public_packet_sha256: legacy.own_public_packet_sha256,
        peer_public_packet: legacy.peer_public_packet,
        peer_public_packet_sha256: legacy.peer_public_packet_sha256,
        role_journal: legacy.role_journal,
        role_journal_sha256: legacy.role_journal_sha256,
        effect_authority_file,
        effect_authority_sha256,
        workflow_journal,
    })
}

fn parse_effect_manifest_config_bytes(
    bytes: &[u8],
    expected_role: ActorRole,
) -> Result<XmrActorProvisionManifestV3> {
    ensure!(
        !bytes.is_empty()
            && u64::try_from(bytes.len()).unwrap_or(u64::MAX)
                <= XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "XMR effect provision manifest is oversized"
    );
    let manifest: XmrActorProvisionManifestV3 =
        serde_json::from_slice(bytes).context("XMR effect manifest is malformed")?;
    ensure!(
        canonical_effect_manifest_bytes(&manifest)? == bytes
            && manifest.schema_version == 3
            && manifest.role == expected_role,
        "XMR effect manifest is noncanonical or unsupported"
    );
    let legacy = manifest.legacy();
    let legacy_bytes = canonical_manifest_bytes(&legacy)?;
    parse_manifest_config_bytes(&legacy_bytes, expected_role)?;
    let reconstructed = promote_effect_manifest_v3(
        legacy,
        manifest.effect_authority_file.clone(),
        manifest.effect_authority_sha256.clone(),
        manifest.run_id.clone(),
        manifest.workflow_journal.clone(),
    )?;
    ensure!(
        reconstructed == manifest,
        "XMR effect manifest fields are inconsistent"
    );
    Ok(manifest)
}

fn canonical_effect_manifest_bytes(manifest: &XmrActorProvisionManifestV3) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(manifest).context("encode XMR effect manifest")?;
    bytes.push(b'\n');
    ensure!(
        u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "XMR effect provision manifest is oversized"
    );
    Ok(bytes)
}

fn publish_effect_manifest_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(
        !bytes.is_empty()
            && u64::try_from(bytes.len()).unwrap_or(u64::MAX)
                <= XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "XMR effect provision manifest is oversized"
    );
    let destination = SecureDestination::new(path, "XMR effect provision manifest")?;
    super::write_bounded_public_new(
        &destination,
        bytes,
        XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "XMR effect provision manifest",
    )?;
    ensure!(
        read_private_source(
            path,
            XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
            "XMR effect provision manifest"
        )? == bytes,
        "published XMR effect provision manifest changed"
    );
    Ok(())
}

fn workflow_participant(role: ActorRole) -> Participant {
    match role {
        ActorRole::Maker => Participant::Maker,
        ActorRole::Taker => Participant::Taker,
    }
}

fn workflow_identity(
    authority: &ValidatedXmrRoleAuthorityV2,
    role: ActorRole,
    run_id: &str,
    effect_authority_sha256: [u8; 32],
) -> Result<XmrWorkflowIdentityV1> {
    XmrWorkflowIdentityV1::new(
        SwapId::new(hex::encode(authority.swap_id)).context("invalid XMR workflow swap ID")?,
        workflow_participant(role),
        run_id.into(),
        authority.agreement_commitment,
        authority.activation_commitment,
        effect_authority_sha256,
    )
    .context("invalid XMR workflow identity")
}

/// Validates that schema v3 embeds one exact canonical schema-v2 projection.
///
/// This byte-only boundary performs no filesystem, journal, RPC, or chain I/O.
///
/// # Errors
///
/// Rejects noncanonical or crossed schemas, roles, runs, and any legacy byte
/// difference.
pub fn validate_xmr_effect_manifest_v3_projection_bytes(
    effect_manifest_bytes: &[u8],
    legacy_manifest_bytes: &[u8],
    expected_role: ActorRole,
    expected_run_id: &str,
) -> Result<()> {
    let manifest = parse_effect_manifest_config_bytes(effect_manifest_bytes, expected_role)?;
    ensure!(
        manifest.run_id == expected_run_id
            && canonical_manifest_bytes(&manifest.legacy())? == legacy_manifest_bytes,
        "XMR schema-v3 legacy projection differs from receipt authority"
    );
    Ok(())
}

/// Fully validates canonical schema-v3 application and effect authority bytes.
///
/// The schema-v2 projection is semantically revalidated from its pinned files,
/// the effect bytes must match the manifest digest and the exact swap, role,
/// agreement, activation, and run, and the existing workflow journal must
/// contain that same immutable identity.
///
/// # Errors
///
/// Rejects legacy/noncanonical manifests, source or digest drift, crossed
/// identities, unsafe paths, invalid effect profiles, or a missing/foreign
/// workflow journal.
pub fn load_validated_xmr_effect_manifest_v3_bytes(
    manifest_bytes: &[u8],
    effect_authority_bytes: &[u8],
    expected_role: ActorRole,
    expected_run_id: &str,
) -> Result<ValidatedXmrEffectAuthorityV1> {
    Ok(load_validated_xmr_effect_execution_v3_bytes(
        manifest_bytes,
        effect_authority_bytes,
        expected_role,
        expected_run_id,
    )?
    .effect)
}

/// Fully validates schema-v3 authority and retains its workflow identity.
///
/// # Errors
///
/// Rejects every condition rejected by
/// `load_validated_xmr_effect_manifest_v3_bytes`.
pub fn load_validated_xmr_effect_execution_v3_bytes(
    manifest_bytes: &[u8],
    effect_authority_bytes: &[u8],
    expected_role: ActorRole,
    expected_run_id: &str,
) -> Result<ValidatedXmrEffectExecutionV3> {
    let manifest = parse_effect_manifest_config_bytes(manifest_bytes, expected_role)?;
    ensure!(
        manifest.run_id == expected_run_id,
        "XMR effect manifest run differs from scheduler authority"
    );
    let legacy_bytes = canonical_manifest_bytes(&manifest.legacy())?;
    let legacy = load_validated_xmr_role_authority_bytes(&legacy_bytes, expected_role)?;
    let effect_authority_sha256 = sha256(effect_authority_bytes);
    ensure!(
        effect_authority_sha256 == decode_canonical_exact_32(&manifest.effect_authority_sha256)?,
        "XMR effect authority digest differs from schema-v3 manifest"
    );
    let effect = load_validated_xmr_effect_authority_bytes(
        effect_authority_bytes,
        expected_role,
        legacy.swap_id,
        legacy.agreement_commitment,
        legacy.activation_commitment,
        expected_run_id,
    )
    .context("validate schema-v3 XMR effect authority")?;
    ensure!(
        effect.workflow_journal() == manifest.workflow_journal
            && effect.adaptor_journal() == manifest.role_journal,
        "XMR effect authority journal paths differ from schema-v3 manifest"
    );
    let identity = workflow_identity(
        &legacy,
        expected_role,
        expected_run_id,
        effect_authority_sha256,
    )?;
    let workflow = SqliteXmrWorkflowJournal::open_existing(&manifest.workflow_journal)
        .context("open schema-v3 XMR workflow journal")?;
    workflow
        .validate_initialized(&identity)
        .context("bind schema-v3 XMR workflow identity")?;
    Ok(ValidatedXmrEffectExecutionV3 {
        effect,
        workflow_identity: identity,
        effect_authority_sha256,
    })
}

/// Publishes one canonical owner-private schema-v3 manifest without clobbering.
///
/// The legacy and effect files are securely read, the complete semantic
/// authority plus initialized workflow journal is validated before publication,
/// and every source is reread and revalidated after the atomic create-new write.
///
/// # Errors
///
/// Rejects unsafe/overlapping paths, authority drift, identity mismatch,
/// uninitialized workflow state, output collision, or post-publication change.
#[allow(clippy::too_many_arguments)]
pub fn publish_xmr_effect_manifest_v3(
    legacy_manifest_file: &Path,
    expected_role: ActorRole,
    effect_authority_file: &Path,
    workflow_journal: &Path,
    expected_run_id: &str,
    output_manifest_file: &Path,
) -> Result<[u8; 32]> {
    ensure!(
        [
            legacy_manifest_file,
            effect_authority_file,
            workflow_journal,
            output_manifest_file,
        ]
        .into_iter()
        .all(normalized_absolute)
            && legacy_manifest_file != effect_authority_file
            && legacy_manifest_file != workflow_journal
            && legacy_manifest_file != output_manifest_file
            && effect_authority_file != workflow_journal
            && effect_authority_file != output_manifest_file
            && workflow_journal != output_manifest_file,
        "XMR effect publication paths are invalid"
    );
    let legacy_bytes = read_private_source(
        legacy_manifest_file,
        XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "legacy XMR actor provision manifest",
    )?;
    let effect_authority_bytes = read_private_source(
        effect_authority_file,
        u64::try_from(MAX_AUTHORITY_BYTES).unwrap_or(u64::MAX),
        "XMR effect authority",
    )?;
    let legacy = parse_manifest_config_bytes(&legacy_bytes, expected_role)?;
    let promoted = promote_effect_manifest_v3(
        legacy,
        effect_authority_file.to_path_buf(),
        hex::encode(sha256(&effect_authority_bytes)),
        expected_run_id.to_owned(),
        workflow_journal.to_path_buf(),
    )?;
    let manifest_bytes = canonical_effect_manifest_bytes(&promoted)?;
    let _ = load_validated_xmr_effect_manifest_v3_bytes(
        &manifest_bytes,
        &effect_authority_bytes,
        expected_role,
        expected_run_id,
    )?;
    publish_effect_manifest_bytes(output_manifest_file, &manifest_bytes)?;

    let published = read_private_source(
        output_manifest_file,
        XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "published XMR effect provision manifest",
    )?;
    let legacy_after = read_private_source(
        legacy_manifest_file,
        XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "legacy XMR actor provision manifest",
    )?;
    let effect_after = read_private_source(
        effect_authority_file,
        u64::try_from(MAX_AUTHORITY_BYTES).unwrap_or(u64::MAX),
        "XMR effect authority",
    )?;
    ensure!(
        published == manifest_bytes
            && legacy_after == legacy_bytes
            && effect_after == effect_authority_bytes,
        "XMR effect authority changed during schema-v3 publication"
    );
    let _ = load_validated_xmr_effect_manifest_v3_bytes(
        &published,
        &effect_after,
        expected_role,
        expected_run_id,
    )?;
    Ok(sha256(&published))
}

/// Provisions or exactly replays one complete schema-v3 effect authority.
///
/// A missing workflow journal is created and initialized before the immutable
/// manifest is published. Existing journals and manifests are accepted only
/// when their complete identities and bytes match exactly; neither is migrated
/// or overwritten.
///
/// # Errors
///
/// Rejects unsafe or overlapping paths, legacy/effect identity drift, foreign
/// workflow state, non-exact replay, and any source change during publication.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn provision_xmr_effect_manifest_v3(
    legacy_manifest_file: &Path,
    expected_role: ActorRole,
    effect_authority_file: &Path,
    workflow_journal: &Path,
    expected_run_id: &str,
    output_manifest_file: &Path,
) -> Result<XmrEffectProvisionV3> {
    ensure!(
        [
            legacy_manifest_file,
            effect_authority_file,
            workflow_journal,
            output_manifest_file,
        ]
        .into_iter()
        .all(normalized_absolute)
            && legacy_manifest_file != effect_authority_file
            && legacy_manifest_file != workflow_journal
            && legacy_manifest_file != output_manifest_file
            && effect_authority_file != workflow_journal
            && effect_authority_file != output_manifest_file
            && workflow_journal != output_manifest_file,
        "XMR effect provisioning paths are invalid"
    );
    let legacy_bytes = read_private_source(
        legacy_manifest_file,
        XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "legacy XMR actor provision manifest",
    )?;
    let effect_authority_bytes = read_private_source(
        effect_authority_file,
        u64::try_from(MAX_AUTHORITY_BYTES).unwrap_or(u64::MAX),
        "XMR effect authority",
    )?;
    let legacy_manifest = parse_manifest_config_bytes(&legacy_bytes, expected_role)?;
    let legacy = load_validated_xmr_role_authority_bytes(&legacy_bytes, expected_role)?;
    let effect_authority_sha256 = sha256(&effect_authority_bytes);
    let effect = load_validated_xmr_effect_authority_bytes(
        &effect_authority_bytes,
        expected_role,
        legacy.swap_id,
        legacy.agreement_commitment,
        legacy.activation_commitment,
        expected_run_id,
    )
    .context("validate replay-safe XMR effect authority")?;
    ensure!(
        effect.workflow_journal() == workflow_journal
            && effect.adaptor_journal() == legacy.state_database,
        "XMR effect authority journal paths differ from application authority"
    );
    let identity = workflow_identity(
        &legacy,
        expected_role,
        expected_run_id,
        effect_authority_sha256,
    )?;
    match SqliteXmrWorkflowJournal::create_new(workflow_journal) {
        Ok(mut journal) => journal
            .initialize(&identity)
            .context("initialize new XMR effect workflow")?,
        Err(StoreError::XmrWorkflowDatabaseAlreadyExists) => {
            let journal = SqliteXmrWorkflowJournal::open_existing(workflow_journal)
                .context("open existing XMR effect workflow")?;
            journal
                .validate_initialized(&identity)
                .context("validate replayed XMR effect workflow")?;
        }
        Err(error) => return Err(error).context("create XMR effect workflow"),
    }

    let promoted = promote_effect_manifest_v3(
        legacy_manifest,
        effect_authority_file.to_path_buf(),
        hex::encode(effect_authority_sha256),
        expected_run_id.to_owned(),
        workflow_journal.to_path_buf(),
    )?;
    let manifest_bytes = canonical_effect_manifest_bytes(&promoted)?;
    let was_replay = match publish_effect_manifest_bytes(output_manifest_file, &manifest_bytes) {
        Ok(()) => false,
        Err(error) => {
            let existing = read_private_source(
                output_manifest_file,
                XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
                "existing XMR effect provision manifest",
            )
            .with_context(|| format!("publish XMR effect provision manifest: {error}"))?;
            ensure!(
                existing == manifest_bytes,
                "existing XMR effect provision manifest conflicts"
            );
            true
        }
    };

    let published = read_private_source(
        output_manifest_file,
        XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "published XMR effect provision manifest",
    )?;
    let legacy_after = read_private_source(
        legacy_manifest_file,
        XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "legacy XMR actor provision manifest",
    )?;
    let effect_after = read_private_source(
        effect_authority_file,
        u64::try_from(MAX_AUTHORITY_BYTES).unwrap_or(u64::MAX),
        "XMR effect authority",
    )?;
    ensure!(
        published == manifest_bytes
            && legacy_after == legacy_bytes
            && effect_after == effect_authority_bytes,
        "XMR effect authority changed during replay-safe provisioning"
    );
    let validated = load_validated_xmr_effect_manifest_v3_bytes(
        &published,
        &effect_after,
        expected_role,
        expected_run_id,
    )?;
    ensure!(
        validated.role() == expected_role
            && validated.swap_id() == legacy.swap_id
            && validated.run_id() == expected_run_id
            && validated.workflow_journal() == workflow_journal
            && validated.adaptor_journal() == legacy.state_database,
        "validated XMR effect authority changed during provisioning"
    );
    Ok(XmrEffectProvisionV3 {
        was_replay,
        role: expected_role,
        swap_id: legacy.swap_id,
        run_id: expected_run_id.to_owned().into_boxed_str(),
        agreement_commitment: legacy.agreement_commitment,
        activation_commitment: legacy.activation_commitment,
        manifest_file: output_manifest_file.to_path_buf(),
        manifest_sha256: sha256(&published),
        effect_authority_file: effect_authority_file.to_path_buf(),
        effect_authority_sha256,
        workflow_journal: workflow_journal.to_path_buf(),
    })
}

fn parse_maker_manifest_config_bytes(bytes: &[u8]) -> Result<XmrActorProvisionManifestV2> {
    parse_manifest_config_bytes(bytes, ActorRole::Maker)
}

fn parse_manifest_config_bytes(
    bytes: &[u8],
    expected_role: ActorRole,
) -> Result<XmrActorProvisionManifestV2> {
    ensure!(
        !bytes.is_empty()
            && u64::try_from(bytes.len()).unwrap_or(u64::MAX)
                <= XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "XMR actor provision manifest is oversized"
    );
    let manifest: XmrActorProvisionManifestV2 =
        serde_json::from_slice(bytes).context("XMR actor manifest is malformed")?;
    ensure!(
        canonical_manifest_bytes(&manifest)? == bytes,
        "XMR actor manifest is noncanonical"
    );
    ensure!(
        manifest.schema_version == APPLICATION_PROVISION_SCHEMA_V2
            && manifest.role == expected_role,
        "XMR actor manifest role or schema is invalid"
    );
    for path in [
        &manifest.published_stage_a,
        &manifest.published_stage_b,
        &manifest.source_private_root,
        &manifest.own_public_packet,
        &manifest.peer_public_packet,
        &manifest.role_journal,
    ] {
        ensure!(
            normalized_absolute(path),
            "XMR actor manifest path is invalid"
        );
    }
    let shared = manifest
        .published_stage_a
        .parent()
        .ok_or_else(|| anyhow!("published XMR Stage A has no parent"))?;
    let application_root = shared
        .parent()
        .ok_or_else(|| anyhow!("published XMR shared authority has no application root"))?;
    ensure!(
        shared.file_name().is_some_and(|name| name == "shared")
            && manifest
                .published_stage_a
                .file_name()
                .is_some_and(|name| name == STAGE_A_FILE)
            && manifest.published_stage_b.parent() == Some(shared)
            && manifest
                .published_stage_b
                .file_name()
                .is_some_and(|name| name == STAGE_B_FILE)
            && normalized_absolute(application_root)
            && application_root != manifest.source_private_root
            && !application_root.starts_with(&manifest.source_private_root),
        "published XMR Stage authority escapes its application bundle"
    );
    for digest in [
        &manifest.stage_a_sha256,
        &manifest.stage_b_sha256,
        &manifest.source_private_manifest_sha256,
        &manifest.source_view_key_sha256,
        &manifest.own_public_packet_sha256,
        &manifest.peer_public_packet_sha256,
        &manifest.role_journal_sha256,
    ] {
        let _ = decode_canonical_exact_32(digest)?;
    }
    let _ = decode_canonical_exact_32(&manifest.swap_id)?;
    Ok(manifest)
}

fn decode_canonical_exact_32(value: &str) -> Result<[u8; 32]> {
    let decoded = super::decode_exact::<32>(value)?;
    ensure!(
        hex::encode(decoded) == value,
        "XMR actor manifest hex is noncanonical"
    );
    Ok(decoded)
}

fn read_pinned_private_source(
    path: &Path,
    maximum: u64,
    label: &'static str,
    expected_sha256: &str,
) -> Result<Vec<u8>> {
    let bytes = read_private_source(path, maximum, label)?;
    ensure!(
        sha256(&bytes) == super::decode_exact::<32>(expected_sha256)?,
        "{label} digest differs from provision manifest"
    );
    Ok(bytes)
}

fn revalidate_exact_private_source(
    path: &Path,
    maximum: u64,
    label: &'static str,
    expected: &[u8],
) -> Result<()> {
    ensure!(
        read_private_source(path, maximum, label)? == expected,
        "{label} changed during XMR actor authority validation"
    );
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

fn read_sealed_config_fd(fd: i32) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(
        fd == MAKER_ACTOR_CONFIG_FD,
        "XMR Maker actor config descriptor must be {MAKER_ACTOR_CONFIG_FD}"
    );
    let mut file =
        File::open(format!("/proc/self/fd/{fd}")).context("open sealed XMR Maker actor config")?;
    let before = file
        .metadata()
        .context("inspect sealed XMR Maker actor config")?;
    validate_sealed_config_metadata(&before)?;
    validate_config_seals(&file)?;

    file.seek(SeekFrom::Start(0))
        .context("rewind sealed XMR Maker actor config")?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(
        usize::try_from(XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES)
            .unwrap_or(16 * 1024)
            .saturating_add(1),
    ));
    file.by_ref()
        .take(XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES.saturating_add(1))
        .read_to_end(bytes.as_mut())
        .context("read sealed XMR Maker actor config")?;

    let after = file
        .metadata()
        .context("reinspect sealed XMR Maker actor config")?;
    validate_sealed_config_metadata(&after)?;
    validate_config_seals(&file)?;
    ensure!(
        same_file(&before, &after)
            && !bytes.is_empty()
            && u64::try_from(bytes.len()).unwrap_or(u64::MAX) == before.len(),
        "sealed XMR Maker actor config changed or is invalid"
    );
    Ok(bytes)
}

fn validate_sealed_config_metadata(metadata: &fs::Metadata) -> Result<()> {
    ensure!(
        metadata.file_type().is_file()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.permissions().mode() & 0o7777 == 0o600
            && metadata.nlink() == 0
            && metadata.len() > 0
            && metadata.len() <= XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "sealed XMR Maker actor config metadata is invalid"
    );
    Ok(())
}

fn validate_config_seals(file: &File) -> Result<()> {
    let required = SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE;
    let actual = fcntl_get_seals(file).context("inspect XMR Maker actor config seals")?;
    ensure!(
        actual.contains(required),
        "XMR Maker actor config is not fully sealed"
    );
    Ok(())
}

fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
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

fn canonical_manifest_bytes(manifest: &XmrActorProvisionManifestV2) -> Result<Vec<u8>> {
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

    use std::{ffi::CString, io::Write as _};

    use rustix::fs::{MemfdFlags, Mode, fchmod, fcntl_add_seals, memfd_create};

    fn manifest() -> XmrActorProvisionManifestV2 {
        XmrActorProvisionManifestV2 {
            schema_version: APPLICATION_PROVISION_SCHEMA_V2,
            role: ActorRole::Maker,
            swap_id: "11".repeat(32),
            stage_a_sha256: "21".repeat(32),
            published_stage_a: PathBuf::from("/application/shared/stage-a-v1.borsh"),
            stage_b_sha256: "22".repeat(32),
            published_stage_b: PathBuf::from("/application/shared/stage-b-v1.borsh"),
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
        value.published_stage_b = PathBuf::from("relative/stage-b-v1.borsh");
        invalid.push(value);
        let mut value = manifest();
        value.published_stage_b = PathBuf::from("/other/shared/stage-b-v1.borsh");
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

    #[test]
    fn maker_execution_authority_retains_validated_stage_wire_identity() {
        let authority = ValidatedXmrMakerAuthorityV2 {
            swap_id: [0x11; 32],
            state_database: PathBuf::from("/private/journals/maker.sqlite"),
            agreement_commitment: [0x21; 32],
            activation_commitment: [0x22; 32],
            published_stage_a: PathBuf::from("/application/shared/stage-a-v1.borsh"),
            stage_a_sha256: [0x31; 32],
            published_stage_b: PathBuf::from("/application/shared/stage-b-v1.borsh"),
            stage_b_sha256: [0x32; 32],
            _role_journal_snapshot: Zeroizing::new(b"private-journal-snapshot".to_vec()),
        };

        assert_eq!(
            authority.published_stage_a(),
            Path::new("/application/shared/stage-a-v1.borsh")
        );
        assert_eq!(authority.stage_a_sha256(), [0x31; 32]);
        assert_eq!(
            authority.published_stage_b(),
            Path::new("/application/shared/stage-b-v1.borsh")
        );
        assert_eq!(authority.stage_b_sha256(), [0x32; 32]);
        let debug = format!("{authority:?}");
        assert!(debug.contains("/application/shared/stage-a-v1.borsh"));
        assert!(debug.contains("/application/shared/stage-b-v1.borsh"));
        assert!(!debug.contains("private-journal-snapshot"));
    }

    #[test]
    fn taker_manifest_validator_binds_receipt_before_state_lock() {
        let expected_swap = [0x11; 32];
        let state = Path::new("/private/journals/taker.sqlite");
        let mut taker = manifest();
        taker.role = ActorRole::Taker;
        taker.source_private_root = PathBuf::from("/private/taker");
        taker.own_public_packet = PathBuf::from("/exchange/taker.json");
        taker.peer_public_packet = PathBuf::from("/exchange/maker.json");
        taker.role_journal = state.to_path_buf();
        let valid = canonical_manifest_bytes(&taker).unwrap();
        validate_taker_manifest_config_bytes(&valid, expected_swap, state)
            .expect("valid Taker receipt binding");

        assert!(
            validate_taker_manifest_config_bytes(
                &valid,
                expected_swap,
                Path::new("/private/journals/unbound.sqlite"),
            )
            .is_err()
        );
        assert!(validate_taker_manifest_config_bytes(&valid, [0x12; 32], state).is_err());

        let mut wrong_role = taker.clone();
        wrong_role.role = ActorRole::Maker;
        assert!(
            validate_taker_manifest_config_bytes(
                &canonical_manifest_bytes(&wrong_role).unwrap(),
                expected_swap,
                state,
            )
            .is_err()
        );
        taker.stage_a_sha256 = "AB".repeat(32);
        assert!(
            validate_taker_manifest_config_bytes(
                &canonical_manifest_bytes(&taker).unwrap(),
                expected_swap,
                state,
            )
            .is_err()
        );
    }

    fn config_memfd(bytes: &[u8], seals: SealFlags) -> File {
        let name = CString::new("lez-xmr-actor-test-config").expect("memfd name");
        let descriptor = memfd_create(
            name.as_c_str(),
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )
        .expect("create config memfd");
        let mut file = File::from(descriptor);
        fchmod(&file, Mode::RUSR | Mode::WUSR).expect("set config memfd mode");
        file.write_all(bytes).expect("write config memfd");
        fcntl_add_seals(&file, seals).expect("seal config memfd");
        file
    }

    #[test]
    fn sealed_config_pinned_digest_and_sidecar_boundaries_fail_closed() {
        assert!(load_validated_xmr_maker_authority_fd(MAKER_ACTOR_CONFIG_FD - 1).is_err());

        let incomplete = config_memfd(
            b"{}\n",
            SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW,
        );
        assert!(validate_sealed_config_metadata(&incomplete.metadata().unwrap()).is_ok());
        assert!(validate_config_seals(&incomplete).is_err());

        let complete = config_memfd(
            b"{}\n",
            SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE,
        );
        assert!(validate_sealed_config_metadata(&complete.metadata().unwrap()).is_ok());
        assert!(validate_config_seals(&complete).is_ok());

        let directory = tempfile::tempdir().expect("private source root");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("private root mode");
        let source = directory.path().join("authority.bin");
        fs::write(&source, b"authority-v1").expect("write private authority");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600))
            .expect("private authority mode");
        let digest = hex::encode(sha256(b"authority-v1"));
        assert_eq!(
            read_pinned_private_source(&source, 64, "test authority", &digest).unwrap(),
            b"authority-v1"
        );
        fs::write(&source, b"authority-v2").expect("tamper private authority");
        assert!(read_pinned_private_source(&source, 64, "test authority", &digest).is_err());

        let journal = directory.path().join("maker.sqlite");
        assert!(validate_no_journal_sidecars(&journal).is_ok());
        fs::write(journal_sidecar_path(&journal, "-wal"), b"uncheckpointed")
            .expect("write fake WAL");
        assert!(validate_no_journal_sidecars(&journal).is_err());
    }

    #[test]
    fn schema_v3_promotes_v2_without_reinterpreting_or_overwriting_it() {
        let legacy = manifest();
        let legacy_bytes = canonical_manifest_bytes(&legacy).expect("canonical legacy manifest");
        let effect_file = PathBuf::from("/application/maker/xmr-effect-authority-v1.json");
        let workflow = PathBuf::from("/private/journals/maker-workflow.sqlite");
        let promoted = promote_effect_manifest_v3(
            legacy.clone(),
            effect_file.clone(),
            "91".repeat(32),
            "m5-xmr-effect-run-1".to_owned(),
            workflow.clone(),
        )
        .expect("promote exact legacy authority");
        assert_eq!(promoted.schema_version, 3);
        assert_eq!(promoted.legacy(), legacy);
        assert_eq!(promoted.effect_authority_file, effect_file);
        assert_eq!(promoted.workflow_journal, workflow);

        let bytes =
            canonical_effect_manifest_bytes(&promoted).expect("canonical v3 effect manifest");
        let parsed = parse_effect_manifest_config_bytes(&bytes, ActorRole::Maker)
            .expect("schema v3 parses only through the effect loader");
        assert_eq!(parsed, promoted);
        assert!(
            parse_effect_manifest_config_bytes(&legacy_bytes, ActorRole::Maker).is_err(),
            "legacy v2 must remain monitor-only"
        );

        let crossed = promote_effect_manifest_v3(
            legacy.clone(),
            PathBuf::from("/application/maker/xmr-effect-authority-v1.json"),
            "92".repeat(32),
            "m5-xmr-effect-run-1".to_owned(),
            legacy.role_journal.clone(),
        );
        assert!(
            crossed.is_err(),
            "workflow and adaptor journals must remain separate"
        );

        let mut wrong_role = promoted;
        wrong_role.role = ActorRole::Taker;
        assert!(
            parse_effect_manifest_config_bytes(
                &canonical_effect_manifest_bytes(&wrong_role).unwrap(),
                ActorRole::Maker,
            )
            .is_err()
        );

        let root = tempfile::tempdir().expect("private effect-manifest root");
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
            .expect("owner-private effect-manifest root");
        let output = root.path().join("actor-effect-provision-v3.json");
        publish_effect_manifest_bytes(&output, &bytes).expect("publish schema-v3 manifest once");
        let published = fs::read(&output).expect("read published schema-v3 manifest");
        assert_eq!(published, bytes);
        assert!(
            publish_effect_manifest_bytes(&output, b"crossed\n").is_err(),
            "schema-v3 publication must never overwrite an existing authority"
        );
        assert_eq!(fs::read(&output).unwrap(), published);
    }
}
