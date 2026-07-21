//! Role-fixed process boundary for independently composing M4 XMR material.
//!
//! Each invocation accepts exactly one private role root. Public packets may be
//! exchanged between roles; private signing keys and Monero scalars never
//! cross this crate's output boundary.

#![cfg_attr(not(unix), allow(unused_imports))]

#[cfg(not(unix))]
compile_error!("xmr-reference-actor requires Unix file-permission semantics");

use std::{
    ffi::OsString,
    fs::File,
    io::{Read as _, Write as _},
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, anyhow, ensure};
use clap::{Parser, Subcommand, ValueEnum};
#[cfg(feature = "sessions")]
use lez_adaptor_role_runner::{Role as RunnerRole, ValidatedSession};
#[cfg(feature = "sessions")]
use lez_swap_store::{AdaptorSessionPhase, SqliteAdaptorSessionJournal};
use lez_xmr_swap_sdk::{
    CrossCurveDleqProofV1, CrossCurveScalar, MAX_XMR_AGREEMENT_WIRE_BYTES,
    MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES, MoneroPrivateViewKey, ValidatedXmrAgreementBodyV1,
    XmrAgreementBodyV1, XmrAgreementV1, XmrParticipantIdentityV1, XmrRoleV1,
};
#[cfg(feature = "sessions")]
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES,
    ValidatedXmrActivationBodyV1, XmrActivatedAgreementV1, XmrActivationBodyV1,
    XmrSessionTranscriptV1,
};
#[cfg(feature = "sessions")]
use rustix::fs::Dir;
use rustix::{
    fs::{
        AtFlags, CWD, Mode, OFlags, RenameFlags, ResolveFlags, mkdirat, openat, openat2,
        renameat_with, unlinkat,
    },
    io::Errno,
};
use secp256k1::rand::{CryptoRng, RngCore, SeedableRng, rngs::OsRng, rngs::StdRng};
use secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

const ROLE_PACKET_SCHEMA_V1: u16 = 1;
const PRIVATE_MANIFEST_SCHEMA_V1: u16 = 1;
const ROLE_PACKET_MAX_BYTES: u64 = 270 * 1024;
const ROLE_PACKET_MAX_HEX_CHARS: usize = 270 * 1024 * 2;
const PRIVATE_MANIFEST_MAX_BYTES: u64 = 1024;
const PRIVATE_KEY_MAX_BYTES: u64 = 66;
const STAGE_A_SIGNATURE_BYTES: usize = 64;
#[cfg(feature = "sessions")]
const STAGE_B_SIGNATURE_BYTES: usize = 64;
#[cfg(feature = "sessions")]
const SESSION_FILE_MAX_BYTES: u64 = 8 * 1024;
#[cfg(feature = "sessions")]
const CLAIM_SESSION_FILE: &str = "claim.json";
#[cfg(feature = "sessions")]
const REFUND_SESSION_FILE: &str = "refund.json";
#[cfg(feature = "sessions")]
const SESSION_BUNDLE_FILES: [&str; 2] = [CLAIM_SESSION_FILE, REFUND_SESSION_FILE];
const AGREEMENT_KEY_FILE: &str = "agreement.key";
const CLAIM_KEY_FILE: &str = "claim.key";
const REFUND_KEY_FILE: &str = "refund.key";
const XMR_SHARE_FILE: &str = "xmr-share.key";
const VIEW_KEY_FILE: &str = "monero-view.key";
const PRIVATE_MANIFEST_FILE: &str = "manifest.json";
const PRIVATE_BUNDLE_FILES: [&str; 6] = [
    AGREEMENT_KEY_FILE,
    CLAIM_KEY_FILE,
    REFUND_KEY_FILE,
    XMR_SHARE_FILE,
    VIEW_KEY_FILE,
    PRIVATE_MANIFEST_FILE,
];

/// One private role selected for a process invocation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ActorRole {
    /// LEZ claimant and Monero funder.
    Maker,
    /// LEZ depositor and Monero claimant.
    Taker,
}

/// One bounded role-process action.
#[derive(Clone, Debug, Subcommand)]
pub enum Action {
    /// Generate fresh private role material and one canonical public packet.
    Provision {
        /// Fixed private role for this root.
        #[arg(value_enum)]
        role: ActorRole,
        /// New role directory under an existing exact owner-only parent.
        #[arg(long, value_name = "NEW_PRIVATE_ROOT")]
        private_root: PathBuf,
        /// Exact lowercase-hex LEZ owner account assigned to this role.
        #[arg(long, value_name = "HEX32")]
        lez_owner_account: String,
        /// Existing owner-private shared view-key handoff; required only by Maker.
        #[arg(long, value_name = "PRIVATE_VIEW_KEY")]
        shared_view_key_file: Option<PathBuf>,
        /// New canonical public role packet under an exact owner-only parent.
        #[arg(long, value_name = "NEW_PUBLIC_JSON")]
        public_packet: PathBuf,
    },
    /// Sign one canonical unsigned Stage-A wire with this role's private agreement key.
    SignStageA {
        /// Fixed role bound by the private manifest.
        #[arg(value_enum)]
        role: ActorRole,
        /// Existing owner-only role root.
        #[arg(long, value_name = "PRIVATE_ROOT")]
        private_root: PathBuf,
        /// This role's canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        own_public_packet: PathBuf,
        /// The other role's canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        peer_public_packet: PathBuf,
        /// Canonical validated unsigned Stage-A wire.
        #[arg(long, value_name = "UNSIGNED_STAGE_A")]
        unsigned_stage_a: PathBuf,
        /// New raw fixed-width BIP340 signature.
        #[arg(long, value_name = "NEW_SIGNATURE")]
        output_signature: PathBuf,
    },
    /// Assemble two raw role signatures into the canonical signed Stage-A wire.
    AssembleStageA {
        /// Canonical Maker public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        maker_public_packet: PathBuf,
        /// Canonical Taker public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        taker_public_packet: PathBuf,
        /// Canonical validated unsigned Stage-A wire.
        #[arg(long, value_name = "UNSIGNED_STAGE_A")]
        unsigned_stage_a: PathBuf,
        /// Raw fixed-width Maker BIP340 signature.
        #[arg(long, value_name = "SIGNATURE")]
        maker_signature: PathBuf,
        /// Raw fixed-width Taker BIP340 signature.
        #[arg(long, value_name = "SIGNATURE")]
        taker_signature: PathBuf,
        /// New canonical signed Stage-A wire.
        #[arg(long, value_name = "NEW_STAGE_A")]
        output_stage_a: PathBuf,
    },
    /// Derive exact claim/refund runner sessions from an accepted Stage A.
    ///
    /// The complete `claim.json`/`refund.json` bundle is exposed by one
    /// no-replace directory rename. A path-only runner API is used only inside
    /// the random unpublished staging directory. A hostile same-UID race can
    /// leave an orphan there, but held-directory identity validation prevents
    /// any incomplete canonical session root from being published.
    #[cfg(feature = "sessions")]
    InitializeSessions {
        /// Fixed role bound by the private manifest.
        #[arg(value_enum)]
        role: ActorRole,
        /// Existing owner-only role root.
        #[arg(long, value_name = "PRIVATE_ROOT")]
        private_root: PathBuf,
        /// This role's canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        own_public_packet: PathBuf,
        /// The other role's canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        peer_public_packet: PathBuf,
        /// Canonical countersigned Stage-A wire.
        #[arg(long, value_name = "STAGE_A")]
        agreement_stage_a: PathBuf,
        /// New atomic role-local directory containing claim.json and refund.json.
        #[arg(long, value_name = "NEW_DIRECTORY")]
        session_root: PathBuf,
    },
    /// Compose canonical unsigned Stage B from the Taker's completed journals.
    #[cfg(feature = "sessions")]
    ComposeStageB {
        /// Existing owner-only Taker role root.
        #[arg(long, value_name = "PRIVATE_ROOT")]
        private_root: PathBuf,
        /// Taker canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        own_public_packet: PathBuf,
        /// Maker canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        peer_public_packet: PathBuf,
        /// Canonical countersigned Stage-A wire.
        #[arg(long, value_name = "STAGE_A")]
        agreement_stage_a: PathBuf,
        /// Existing owner-private Taker journal containing claim and refund.
        #[arg(long, value_name = "PRIVATE_SQLITE")]
        journal: PathBuf,
        /// New canonical unsigned Stage-B wire.
        #[arg(long, value_name = "NEW_UNSIGNED_STAGE_B")]
        output_unsigned_stage_b: PathBuf,
    },
    /// Sign one canonical unsigned Stage-B wire with this role's agreement key.
    #[cfg(feature = "sessions")]
    SignStageB {
        /// Fixed role bound by the private manifest.
        #[arg(value_enum)]
        role: ActorRole,
        /// Existing owner-only role root.
        #[arg(long, value_name = "PRIVATE_ROOT")]
        private_root: PathBuf,
        /// This role's canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        own_public_packet: PathBuf,
        /// The other role's canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        peer_public_packet: PathBuf,
        /// Canonical countersigned Stage-A wire.
        #[arg(long, value_name = "STAGE_A")]
        agreement_stage_a: PathBuf,
        /// Canonical validated unsigned Stage-B wire.
        #[arg(long, value_name = "UNSIGNED_STAGE_B")]
        unsigned_stage_b: PathBuf,
        /// New raw fixed-width BIP340 signature.
        #[arg(long, value_name = "NEW_SIGNATURE")]
        output_signature: PathBuf,
    },
    /// Assemble two role signatures into the canonical signed Stage-B wire.
    #[cfg(feature = "sessions")]
    AssembleStageB {
        /// Fixed role supplying the private view key for final validation.
        #[arg(value_enum)]
        role: ActorRole,
        /// Existing owner-only role root.
        #[arg(long, value_name = "PRIVATE_ROOT")]
        private_root: PathBuf,
        /// This role's canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        own_public_packet: PathBuf,
        /// The other role's canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        peer_public_packet: PathBuf,
        /// Canonical countersigned Stage-A wire.
        #[arg(long, value_name = "STAGE_A")]
        agreement_stage_a: PathBuf,
        /// Canonical validated unsigned Stage-B wire.
        #[arg(long, value_name = "UNSIGNED_STAGE_B")]
        unsigned_stage_b: PathBuf,
        /// Raw fixed-width Maker BIP340 signature.
        #[arg(long, value_name = "SIGNATURE")]
        maker_signature: PathBuf,
        /// Raw fixed-width Taker BIP340 signature.
        #[arg(long, value_name = "SIGNATURE")]
        taker_signature: PathBuf,
        /// New canonical signed Stage-B wire.
        #[arg(long, value_name = "NEW_STAGE_B")]
        output_stage_b: PathBuf,
    },
}

/// CLI for one role-fixed material invocation.
#[derive(Clone, Debug, Parser)]
#[command(about = "PoC role-fixed LEZ/XMR stage-material actor")]
pub struct Cli {
    /// Exactly one monotonic material action.
    #[command(subcommand)]
    pub action: Action,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RolePacketV1 {
    schema_version: u16,
    role: ActorRole,
    lez_owner_account: String,
    agreement_public_key: String,
    claim_session_public_key: String,
    refund_session_public_key: String,
    dleq_proof_wire: String,
    public_view_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PrivateManifestV1 {
    schema_version: u16,
    role: ActorRole,
    lez_owner_account: String,
    public_packet_sha256: String,
}

/// Validated public role packet. Private material is never retained here.
#[derive(Clone, Debug)]
#[must_use]
pub struct ValidatedRolePacket {
    role: ActorRole,
    identity: XmrParticipantIdentityV1,
    proof: CrossCurveDleqProofV1,
    public_view_key: [u8; 32],
}

impl ValidatedRolePacket {
    /// Reads a canonical, bounded public role packet and revalidates its proof.
    ///
    /// # Errors
    ///
    /// Rejects unsafe files, noncanonical JSON/hex, invalid or aliased keys, or
    /// a failed DLEQ proof.
    pub fn read(path: &Path) -> Result<Self> {
        read_role_packet_with_digest(path).map(|(packet, _)| packet)
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let raw: RolePacketV1 =
            serde_json::from_slice(bytes).context("public role packet is malformed")?;
        ensure!(
            raw.canonical_bytes()? == bytes,
            "public role packet is noncanonical"
        );
        ensure!(
            raw.schema_version == ROLE_PACKET_SCHEMA_V1,
            "public role packet schema is unsupported"
        );
        let owner = decode_exact(&raw.lez_owner_account)?;
        ensure!(owner != [0; 32], "public role owner account is invalid");
        let proof_wire = decode_vec(&raw.dleq_proof_wire)?;
        let proof = CrossCurveDleqProofV1::from_wire_bytes(&proof_wire)
            .context("public role DLEQ proof is invalid")?;
        let signing_keys = [
            decode_public_key(&raw.agreement_public_key)?,
            decode_public_key(&raw.claim_session_public_key)?,
            decode_public_key(&raw.refund_session_public_key)?,
        ];
        validate_intra_role_keys(signing_keys, &proof)?;
        let identity =
            XmrParticipantIdentityV1::new(owner, signing_keys[0], signing_keys[1], signing_keys[2]);
        let public_view_key = decode_exact(&raw.public_view_key)?;
        Ok(Self {
            role: raw.role,
            identity,
            proof,
            public_view_key,
        })
    }

    /// Role that produced this packet.
    #[must_use]
    pub const fn role(&self) -> ActorRole {
        self.role
    }

    /// Exact public identity committed into Stage A.
    #[must_use]
    pub const fn identity(&self) -> &XmrParticipantIdentityV1 {
        &self.identity
    }

    /// Verified public cross-curve proof owned by this role.
    #[must_use]
    pub const fn proof(&self) -> &CrossCurveDleqProofV1 {
        &self.proof
    }

    /// Public half of the shared Monero view key.
    #[must_use]
    pub const fn public_view_key(&self) -> [u8; 32] {
        self.public_view_key
    }
}

/// Validated role/owner/public-packet binding retained in one private root.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct ValidatedPrivateManifest {
    role: ActorRole,
    lez_owner_account: [u8; 32],
    public_packet_sha256: [u8; 32],
}

impl ValidatedPrivateManifest {
    /// Opens an exact owner-only root and reads its canonical manifest without
    /// following symbolic links.
    ///
    /// # Errors
    ///
    /// Rejects unsafe roots/files and malformed or noncanonical manifests.
    pub fn read(private_root: &Path) -> Result<Self> {
        let root = open_private_directory(private_root, "private role root")?;
        read_private_manifest_at(&root)
    }

    /// Private role durably bound to this root.
    #[must_use]
    pub const fn role(&self) -> ActorRole {
        self.role
    }

    /// Exact LEZ owner durably bound to this root.
    #[must_use]
    pub const fn lez_owner_account(&self) -> [u8; 32] {
        self.lez_owner_account
    }

    /// SHA-256 of the exact canonical public packet bytes.
    #[must_use]
    pub const fn public_packet_sha256(&self) -> [u8; 32] {
        self.public_packet_sha256
    }
}

impl RolePacketV1 {
    fn canonical_bytes(&self) -> Result<Vec<u8>> {
        canonical_json_bytes(self, "encode public role packet")
    }
}

impl PrivateManifestV1 {
    fn canonical_bytes(&self) -> Result<Vec<u8>> {
        canonical_json_bytes(self, "encode private manifest")
    }
}

/// Executes one role-fixed material command.
///
/// # Errors
///
/// Returns a redacted error when private-file, randomness, proof, or
/// public-packet validation fails.
#[allow(clippy::too_many_lines)] // Explicit CLI dispatch keeps every role action auditable.
pub fn execute(cli: Cli) -> Result<()> {
    match cli.action {
        Action::Provision {
            role,
            private_root,
            lez_owner_account,
            shared_view_key_file,
            public_packet,
        } => provision(
            role,
            &private_root,
            &lez_owner_account,
            shared_view_key_file.as_deref(),
            &public_packet,
        ),
        Action::SignStageA {
            role,
            private_root,
            own_public_packet,
            peer_public_packet,
            unsigned_stage_a,
            output_signature,
        } => sign_stage_a(
            role,
            &private_root,
            &own_public_packet,
            &peer_public_packet,
            &unsigned_stage_a,
            &output_signature,
        ),
        Action::AssembleStageA {
            maker_public_packet,
            taker_public_packet,
            unsigned_stage_a,
            maker_signature,
            taker_signature,
            output_stage_a,
        } => assemble_stage_a(
            &maker_public_packet,
            &taker_public_packet,
            &unsigned_stage_a,
            &maker_signature,
            &taker_signature,
            &output_stage_a,
        ),
        #[cfg(feature = "sessions")]
        Action::InitializeSessions {
            role,
            private_root,
            own_public_packet,
            peer_public_packet,
            agreement_stage_a,
            session_root,
        } => initialize_sessions(
            role,
            &private_root,
            &own_public_packet,
            &peer_public_packet,
            &agreement_stage_a,
            &session_root,
        ),
        #[cfg(feature = "sessions")]
        Action::ComposeStageB {
            private_root,
            own_public_packet,
            peer_public_packet,
            agreement_stage_a,
            journal,
            output_unsigned_stage_b,
        } => compose_stage_b(
            &private_root,
            &own_public_packet,
            &peer_public_packet,
            &agreement_stage_a,
            &journal,
            &output_unsigned_stage_b,
        ),
        #[cfg(feature = "sessions")]
        Action::SignStageB {
            role,
            private_root,
            own_public_packet,
            peer_public_packet,
            agreement_stage_a,
            unsigned_stage_b,
            output_signature,
        } => sign_stage_b(
            role,
            &private_root,
            &own_public_packet,
            &peer_public_packet,
            &agreement_stage_a,
            &unsigned_stage_b,
            &output_signature,
        ),
        #[cfg(feature = "sessions")]
        Action::AssembleStageB {
            role,
            private_root,
            own_public_packet,
            peer_public_packet,
            agreement_stage_a,
            unsigned_stage_b,
            maker_signature,
            taker_signature,
            output_stage_b,
        } => assemble_stage_b(
            role,
            &private_root,
            &own_public_packet,
            &peer_public_packet,
            &agreement_stage_a,
            &unsigned_stage_b,
            &maker_signature,
            &taker_signature,
            &output_stage_b,
        ),
    }
}

fn sign_stage_a(
    role: ActorRole,
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    unsigned_stage_a: &Path,
    output_signature: &Path,
) -> Result<()> {
    let destination = SecureDestination::new(output_signature, "Stage-A signature")?;
    destination.ensure_absent("Stage-A signature")?;
    let packets = StageRolePackets::read(role, own_public_packet, peer_public_packet)?;
    let material = validate_private_role(private_root, role, &packets)?;
    let wire = read_public_input(
        unsigned_stage_a,
        u64::try_from(MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES).unwrap_or(u64::MAX),
        "unsigned Stage-A wire",
    )?;
    let validated = ValidatedXmrAgreementBodyV1::from_unsigned_wire(&wire)
        .context("unsigned Stage-A wire is invalid")?;
    validate_stage_a_packets(validated.body(), &packets)?;
    let secp = Secp256k1::new();
    let signature = secp
        .sign_schnorr_no_aux_rand(
            &Message::from_digest(validated.commitment()),
            &Keypair::from_secret_key(&secp, &material.agreement.key),
        )
        .serialize();
    write_bounded_public_new(
        &destination,
        &signature,
        STAGE_A_SIGNATURE_BYTES as u64,
        "Stage-A signature",
    )
}

fn assemble_stage_a(
    maker_public_packet: &Path,
    taker_public_packet: &Path,
    unsigned_stage_a: &Path,
    maker_signature: &Path,
    taker_signature: &Path,
    output_stage_a: &Path,
) -> Result<()> {
    let destination = SecureDestination::new(output_stage_a, "signed Stage-A wire")?;
    destination.ensure_absent("signed Stage-A wire")?;
    let packets = StageRolePackets::read_explicit(maker_public_packet, taker_public_packet)?;
    let wire = read_public_input(
        unsigned_stage_a,
        u64::try_from(MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES).unwrap_or(u64::MAX),
        "unsigned Stage-A wire",
    )?;
    let validated = ValidatedXmrAgreementBodyV1::from_unsigned_wire(&wire)
        .context("unsigned Stage-A wire is invalid")?;
    validate_stage_a_packets(validated.body(), &packets)?;
    let maker =
        read_fixed_public::<STAGE_A_SIGNATURE_BYTES>(maker_signature, "Maker Stage-A signature")?;
    let taker =
        read_fixed_public::<STAGE_A_SIGNATURE_BYTES>(taker_signature, "Taker Stage-A signature")?;
    let agreement = validated
        .attach_signatures(maker, taker)
        .context("Stage-A role signatures are invalid")?;
    let encoded = agreement
        .encode_wire()
        .context("encode signed Stage-A wire")?;
    XmrAgreementV1::from_wire(&encoded).context("written Stage-A wire would be invalid")?;
    write_bounded_public_new(
        &destination,
        &encoded,
        u64::try_from(MAX_XMR_AGREEMENT_WIRE_BYTES).unwrap_or(u64::MAX),
        "signed Stage-A wire",
    )
}

#[cfg(feature = "sessions")]
fn initialize_sessions(
    role: ActorRole,
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    agreement_stage_a: &Path,
    session_root: &Path,
) -> Result<()> {
    let destination = SecureDestination::new(session_root, "session root")?;
    destination.ensure_absent("session root")?;
    let packets = StageRolePackets::read(role, own_public_packet, peer_public_packet)?;
    let _material = validate_private_role(private_root, role, &packets)?;
    let wire = read_public_input(
        agreement_stage_a,
        u64::try_from(MAX_XMR_AGREEMENT_WIRE_BYTES).unwrap_or(u64::MAX),
        "signed Stage-A wire",
    )?;
    let agreement = XmrAgreementV1::from_wire(&wire).context("signed Stage-A wire is invalid")?;
    validate_stage_a_packets(agreement.body(), &packets)?;
    let claim = ValidatedSession::from_untweaked_context(
        agreement
            .claim_session_descriptor()
            .context()
            .context("claim session descriptor is invalid")?,
    )
    .context("claim runner session is invalid")?;
    let refund = ValidatedSession::from_untweaked_context(
        agreement
            .refund_session_descriptor()
            .context()
            .context("refund session descriptor is invalid")?,
    )
    .context("refund runner session is invalid")?;
    publish_session_bundle(&destination, &claim, &refund)
}

#[cfg(feature = "sessions")]
struct CompletedTakerSession {
    transcript: XmrSessionTranscriptV1,
    maker_partial: [u8; 32],
    taker_partial: [u8; 32],
    presignature: [u8; 65],
}

#[cfg(feature = "sessions")]
fn compose_stage_b(
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    agreement_stage_a: &Path,
    journal: &Path,
    output_unsigned_stage_b: &Path,
) -> Result<()> {
    let destination = SecureDestination::new(output_unsigned_stage_b, "unsigned Stage-B wire")?;
    destination.ensure_absent("unsigned Stage-B wire")?;
    let packets = StageRolePackets::read(ActorRole::Taker, own_public_packet, peer_public_packet)?;
    let material = validate_private_role(private_root, ActorRole::Taker, &packets)?;
    let agreement = read_validated_stage_a(agreement_stage_a, &packets)?;

    let claim_session = ValidatedSession::from_untweaked_context(
        agreement
            .claim_session_descriptor()
            .context()
            .context("claim session descriptor is invalid")?,
    )
    .context("claim runner session is invalid")?;
    let refund_session = ValidatedSession::from_untweaked_context(
        agreement
            .refund_session_descriptor()
            .context()
            .context("refund session descriptor is invalid")?,
    )
    .context("refund runner session is invalid")?;
    let claim = load_completed_taker_session(journal, &claim_session, "claim")?;
    let refund = load_completed_taker_session(journal, &refund_session, "refund")?;

    let claim_partial_context = agreement
        .claim_partial_context_binding(&claim.transcript, claim.maker_partial)
        .context("claim partial context is invalid")?;
    let claim_partial_commitment = agreement
        .commit_taker_claim_partial(&claim.transcript, claim.maker_partial, claim.taker_partial)
        .context("Taker claim partial is invalid")?;
    let body = XmrActivationBodyV1::new(
        agreement.agreement_commitment(),
        agreement.claim_context_binding(),
        claim.transcript,
        claim.maker_partial,
        claim_partial_context,
        claim_partial_commitment,
        agreement.refund_context_binding(),
        refund.transcript,
        refund.maker_partial,
        refund.taker_partial,
        refund.presignature,
    );
    let validated = ValidatedXmrActivationBodyV1::validate(&agreement, body, &material.view)
        .context("unsigned Stage-B body is invalid")?;
    let encoded = validated
        .encode_unsigned_wire()
        .context("encode unsigned Stage-B wire")?;
    write_bounded_public_new(
        &destination,
        &encoded,
        u64::try_from(MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES).unwrap_or(u64::MAX),
        "unsigned Stage-B wire",
    )
}

#[cfg(feature = "sessions")]
fn sign_stage_b(
    role: ActorRole,
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    agreement_stage_a: &Path,
    unsigned_stage_b: &Path,
    output_signature: &Path,
) -> Result<()> {
    let destination = SecureDestination::new(output_signature, "Stage-B signature")?;
    destination.ensure_absent("Stage-B signature")?;
    let packets = StageRolePackets::read(role, own_public_packet, peer_public_packet)?;
    let material = validate_private_role(private_root, role, &packets)?;
    let agreement = read_validated_stage_a(agreement_stage_a, &packets)?;
    let wire = read_public_input(
        unsigned_stage_b,
        u64::try_from(MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES).unwrap_or(u64::MAX),
        "unsigned Stage-B wire",
    )?;
    let validated =
        ValidatedXmrActivationBodyV1::from_unsigned_wire(&agreement, &wire, &material.view)
            .context("unsigned Stage-B wire is invalid")?;
    let secp = Secp256k1::new();
    let signature = secp
        .sign_schnorr_no_aux_rand(
            &Message::from_digest(validated.commitment()),
            &Keypair::from_secret_key(&secp, &material.agreement.key),
        )
        .serialize();
    write_bounded_public_new(
        &destination,
        &signature,
        STAGE_B_SIGNATURE_BYTES as u64,
        "Stage-B signature",
    )
}

#[cfg(feature = "sessions")]
#[allow(clippy::too_many_arguments)]
fn assemble_stage_b(
    role: ActorRole,
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    agreement_stage_a: &Path,
    unsigned_stage_b: &Path,
    maker_signature: &Path,
    taker_signature: &Path,
    output_stage_b: &Path,
) -> Result<()> {
    let destination = SecureDestination::new(output_stage_b, "signed Stage-B wire")?;
    destination.ensure_absent("signed Stage-B wire")?;
    let packets = StageRolePackets::read(role, own_public_packet, peer_public_packet)?;
    let material = validate_private_role(private_root, role, &packets)?;
    let agreement = read_validated_stage_a(agreement_stage_a, &packets)?;
    let wire = read_public_input(
        unsigned_stage_b,
        u64::try_from(MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES).unwrap_or(u64::MAX),
        "unsigned Stage-B wire",
    )?;
    let validated =
        ValidatedXmrActivationBodyV1::from_unsigned_wire(&agreement, &wire, &material.view)
            .context("unsigned Stage-B wire is invalid")?;
    let maker =
        read_fixed_public::<STAGE_B_SIGNATURE_BYTES>(maker_signature, "Maker Stage-B signature")?;
    let taker =
        read_fixed_public::<STAGE_B_SIGNATURE_BYTES>(taker_signature, "Taker Stage-B signature")?;
    let activated = validated
        .attach_signatures(maker, taker)
        .context("Stage-B role signatures are invalid")?;
    let encoded = activated
        .encode_wire()
        .context("encode signed Stage-B wire")?;
    XmrActivatedAgreementV1::from_wire(&agreement, &encoded, &material.view)
        .context("written Stage-B wire would be invalid")?;
    write_bounded_public_new(
        &destination,
        &encoded,
        u64::try_from(MAX_XMR_ACTIVATION_WIRE_BYTES).unwrap_or(u64::MAX),
        "signed Stage-B wire",
    )
}

#[cfg(feature = "sessions")]
fn read_validated_stage_a(
    agreement_stage_a: &Path,
    packets: &StageRolePackets,
) -> Result<XmrAgreementV1> {
    let wire = read_public_input(
        agreement_stage_a,
        u64::try_from(MAX_XMR_AGREEMENT_WIRE_BYTES).unwrap_or(u64::MAX),
        "signed Stage-A wire",
    )?;
    let agreement = XmrAgreementV1::from_wire(&wire).context("signed Stage-A wire is invalid")?;
    validate_stage_a_packets(agreement.body(), packets)?;
    Ok(agreement)
}

#[cfg(feature = "sessions")]
fn load_completed_taker_session(
    journal_path: &Path,
    session: &ValidatedSession,
    label: &'static str,
) -> Result<CompletedTakerSession> {
    let journal = SqliteAdaptorSessionJournal::open_existing(journal_path)
        .with_context(|| format!("open existing Taker {label} journal"))?;
    let expected = session.identity(RunnerRole::Taker);
    let snapshot = journal
        .load(expected.session_id())
        .with_context(|| format!("load Taker {label} journal"))?
        .ok_or_else(|| anyhow!("Taker {label} journal has no matching session"))?;
    ensure!(
        snapshot.identity() == &expected,
        "Taker {label} journal identity mismatch"
    );
    ensure!(
        snapshot.phase() == AdaptorSessionPhase::PresignatureVerified,
        "Taker {label} journal is incomplete"
    );
    let own_nonce = snapshot
        .own_public_nonce()
        .ok_or_else(|| anyhow!("Taker {label} public nonce is unavailable"))?;
    let peer_commitment = snapshot
        .peer_commitment()
        .ok_or_else(|| anyhow!("Maker {label} commitment is unavailable"))?;
    let peer_nonce = snapshot
        .peer_public_nonce()
        .ok_or_else(|| anyhow!("Maker {label} public nonce is unavailable"))?;
    let taker_partial = snapshot
        .own_partial()
        .ok_or_else(|| anyhow!("Taker {label} partial is unavailable"))?;
    let maker_partial = snapshot
        .peer_partial()
        .ok_or_else(|| anyhow!("Maker {label} partial is unavailable"))?;
    let presignature = snapshot
        .presignature()
        .ok_or_else(|| anyhow!("Taker {label} presignature is unavailable"))?;
    Ok(CompletedTakerSession {
        transcript: XmrSessionTranscriptV1::new(
            *peer_commitment.bytes(),
            *snapshot.own_commitment().bytes(),
            *peer_nonce.bytes(),
            *own_nonce.bytes(),
        ),
        maker_partial: *maker_partial.bytes(),
        taker_partial: *taker_partial.bytes(),
        presignature: *presignature.bytes(),
    })
}

fn provision(
    role: ActorRole,
    private_root: &Path,
    lez_owner_account: &str,
    shared_view_key_file: Option<&Path>,
    public_packet: &Path,
) -> Result<()> {
    ensure!(
        matches!(
            (role, shared_view_key_file),
            (ActorRole::Maker, Some(_)) | (ActorRole::Taker, None)
        ),
        "Maker must import and Taker must generate the shared view key"
    );
    let lez_owner_account: [u8; 32] = decode_exact(lez_owner_account)?;
    ensure!(lez_owner_account != [0; 32], "LEZ owner account is invalid");

    let private_destination = SecureDestination::new(private_root, "private role root")?;
    private_destination.ensure_absent("private role root")?;
    let public_destination = SecureDestination::new(public_packet, "public packet")?;
    public_destination.ensure_absent("public packet")?;

    let view_key = match shared_view_key_file {
        Some(path) => read_private_view_key(path)?,
        None => MoneroPrivateViewKey::generate().context("generate private Monero view key")?,
    };
    let mut rng = fallible_seeded_rng()?;
    let agreement = GeneratedSecpKey::generate(&mut rng);
    let claim = GeneratedSecpKey::generate(&mut rng);
    let refund = GeneratedSecpKey::generate(&mut rng);
    let share = CrossCurveScalar::generate().context("generate private Monero share")?;
    let proof =
        CrossCurveDleqProofV1::prove(&share, &mut rng).context("create cross-curve proof")?;
    let public_view_key = view_key.public_key();

    let secp = Secp256k1::new();
    let packet = RolePacketV1 {
        schema_version: ROLE_PACKET_SCHEMA_V1,
        role,
        lez_owner_account: hex::encode(lez_owner_account),
        agreement_public_key: hex::encode(agreement.public_key(&secp).serialize()),
        claim_session_public_key: hex::encode(claim.public_key(&secp).serialize()),
        refund_session_public_key: hex::encode(refund.public_key(&secp).serialize()),
        dleq_proof_wire: hex::encode(proof.to_wire_bytes().context("encode public DLEQ proof")?),
        public_view_key: hex::encode(public_view_key),
    };
    let packet_bytes = packet.canonical_bytes()?;
    let validated = ValidatedRolePacket::from_bytes(&packet_bytes)?;
    ensure!(
        validated.role == role
            && validated.identity.lez_owner_account() == lez_owner_account
            && validated.public_view_key == public_view_key,
        "generated public role packet changed identity"
    );
    let packet_digest: [u8; 32] = Sha256::digest(&packet_bytes).into();
    let manifest = PrivateManifestV1 {
        schema_version: PRIVATE_MANIFEST_SCHEMA_V1,
        role,
        lez_owner_account: hex::encode(lez_owner_account),
        public_packet_sha256: hex::encode(packet_digest),
    };
    let manifest_bytes = manifest.canonical_bytes()?;

    let (public_stage_name, public_stage_file) = create_staged_file(
        &public_destination.parent,
        "public",
        &packet_bytes,
        ROLE_PACKET_MAX_BYTES,
        "public packet",
    )?;
    let private_result = publish_private_bundle(
        &private_destination,
        &agreement,
        &claim,
        &refund,
        share,
        view_key,
        &manifest_bytes,
    );
    if let Err(error) = private_result {
        cleanup_staged_file(&public_destination.parent, &public_stage_name);
        return Err(error);
    }

    let publish_result = publish_staged_file(
        &public_destination,
        &public_stage_name,
        &public_stage_file,
        ROLE_PACKET_MAX_BYTES,
        "public packet",
    );
    if publish_result.is_err() {
        cleanup_staged_file(&public_destination.parent, &public_stage_name);
    }
    publish_result
}

struct StageRolePackets {
    maker: ValidatedRolePacket,
    taker: ValidatedRolePacket,
    own_packet_digest: Option<[u8; 32]>,
}

impl StageRolePackets {
    fn read(role: ActorRole, own_public_packet: &Path, peer_public_packet: &Path) -> Result<Self> {
        let (own, own_digest) = read_role_packet_with_digest(own_public_packet)?;
        let (peer, _) = read_role_packet_with_digest(peer_public_packet)?;
        ensure!(own.role == role, "own public packet has the wrong role");
        ensure!(
            peer.role == role.opposite(),
            "peer public packet has the wrong role"
        );
        let (maker, taker) = match role {
            ActorRole::Maker => (own, peer),
            ActorRole::Taker => (peer, own),
        };
        Self::validated(maker, taker, Some(own_digest))
    }

    fn read_explicit(maker_path: &Path, taker_path: &Path) -> Result<Self> {
        let (maker, _) = read_role_packet_with_digest(maker_path)?;
        let (taker, _) = read_role_packet_with_digest(taker_path)?;
        ensure!(
            maker.role == ActorRole::Maker,
            "Maker public packet has the wrong role"
        );
        ensure!(
            taker.role == ActorRole::Taker,
            "Taker public packet has the wrong role"
        );
        Self::validated(maker, taker, None)
    }

    fn validated(
        maker: ValidatedRolePacket,
        taker: ValidatedRolePacket,
        own_packet_digest: Option<[u8; 32]>,
    ) -> Result<Self> {
        ensure!(
            maker.public_view_key == taker.public_view_key,
            "role packets use different Monero view keys"
        );
        ensure!(
            maker.identity.lez_owner_account() != taker.identity.lez_owner_account(),
            "role packets alias the LEZ owner account"
        );
        Ok(Self {
            maker,
            taker,
            own_packet_digest,
        })
    }

    const fn for_role(&self, role: ActorRole) -> &ValidatedRolePacket {
        match role {
            ActorRole::Maker => &self.maker,
            ActorRole::Taker => &self.taker,
        }
    }
}

impl ActorRole {
    const fn opposite(self) -> Self {
        match self {
            Self::Maker => Self::Taker,
            Self::Taker => Self::Maker,
        }
    }
}

struct LoadedSecpKey {
    key: SecretKey,
    _bytes: Zeroizing<[u8; 32]>,
}

impl LoadedSecpKey {
    fn from_bytes(bytes: Zeroizing<[u8; 32]>, label: &'static str) -> Result<Self> {
        let key = SecretKey::from_slice(bytes.as_ref())
            .with_context(|| format!("private {label} key is invalid"))?;
        Ok(Self { key, _bytes: bytes })
    }

    fn public_key(&self, secp: &Secp256k1<secp256k1::All>) -> [u8; 33] {
        PublicKey::from_secret_key(secp, &self.key).serialize()
    }
}

impl Drop for LoadedSecpKey {
    fn drop(&mut self) {
        self.key.non_secure_erase();
    }
}

struct ValidatedPrivateRoleMaterial {
    agreement: LoadedSecpKey,
    #[cfg(feature = "sessions")]
    view: MoneroPrivateViewKey,
}

fn validate_private_role(
    private_root: &Path,
    role: ActorRole,
    packets: &StageRolePackets,
) -> Result<ValidatedPrivateRoleMaterial> {
    let root = open_private_directory(private_root, "private role root")?;
    let manifest = read_private_manifest_at(&root)?;
    let own = packets.for_role(role);
    let own_digest = packets
        .own_packet_digest
        .ok_or_else(|| anyhow!("private role validation requires an own packet digest"))?;
    ensure!(manifest.role == role, "private manifest role mismatch");
    ensure!(
        manifest.lez_owner_account == own.identity.lez_owner_account(),
        "private manifest owner mismatch"
    );
    ensure!(
        manifest.public_packet_sha256 == own_digest,
        "private manifest public-packet digest mismatch"
    );

    let agreement = LoadedSecpKey::from_bytes(
        read_secret_hex_at(&root, AGREEMENT_KEY_FILE, "agreement key")?,
        "agreement",
    )?;
    let claim = LoadedSecpKey::from_bytes(
        read_secret_hex_at(&root, CLAIM_KEY_FILE, "claim key")?,
        "claim",
    )?;
    let refund = LoadedSecpKey::from_bytes(
        read_secret_hex_at(&root, REFUND_KEY_FILE, "refund key")?,
        "refund",
    )?;
    let secp = Secp256k1::new();
    ensure!(
        agreement.public_key(&secp) == own.identity.agreement_public_key()
            && claim.public_key(&secp) == own.identity.claim_session_public_key()
            && refund.public_key(&secp) == own.identity.refund_session_public_key(),
        "private signing keys do not match the bound public packet"
    );

    let share_bytes = read_secret_hex_at(&root, XMR_SHARE_FILE, "Monero share")?;
    let share = CrossCurveScalar::from_monero_little_endian(*share_bytes)
        .context("private Monero share is invalid")?;
    drop(share_bytes);
    own.proof
        .verify_scalar(&share)
        .context("private Monero share does not match the bound DLEQ proof")?;
    drop(share);

    let view_bytes = read_secret_hex_at(&root, VIEW_KEY_FILE, "Monero view key")?;
    let view = MoneroPrivateViewKey::from_monero_little_endian(*view_bytes)
        .context("private Monero view key is invalid")?;
    drop(view_bytes);
    ensure!(
        view.public_key() == own.public_view_key,
        "private Monero view key does not match the bound public packet"
    );
    drop(claim);
    drop(refund);
    Ok(ValidatedPrivateRoleMaterial {
        agreement,
        #[cfg(feature = "sessions")]
        view,
    })
}

fn validate_stage_a_packets(body: &XmrAgreementBodyV1, packets: &StageRolePackets) -> Result<()> {
    ensure!(
        body.participants().for_role(XmrRoleV1::Maker) == packets.maker.identity()
            && body.participants().for_role(XmrRoleV1::Taker) == packets.taker.identity(),
        "Stage-A participants differ from the role packets"
    );
    let maker_wire = packets
        .maker
        .proof
        .to_wire_bytes()
        .context("encode Maker DLEQ proof")?;
    let taker_wire = packets
        .taker
        .proof
        .to_wire_bytes()
        .context("encode Taker DLEQ proof")?;
    ensure!(
        body.monero().maker_dleq_proof_wire() == maker_wire
            && body.monero().taker_dleq_proof_wire() == taker_wire
            && body.monero().public_view_key() == packets.maker.public_view_key,
        "Stage-A Monero identity differs from the role packets"
    );
    Ok(())
}

fn read_role_packet_with_digest(path: &Path) -> Result<(ValidatedRolePacket, [u8; 32])> {
    let file = open_path_no_symlinks(path, "public role packet")?;
    let bytes = read_bounded_file(
        file,
        ROLE_PACKET_MAX_BYTES,
        FilePolicy::Public,
        "public role packet",
    )?;
    let packet = ValidatedRolePacket::from_bytes(&bytes)?;
    Ok((packet, Sha256::digest(bytes).into()))
}

fn read_private_manifest_at(root: &File) -> Result<ValidatedPrivateManifest> {
    let manifest = openat(
        root,
        PRIVATE_MANIFEST_FILE,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| anyhow!("private manifest is unavailable"))?;
    let bytes = read_bounded_file(
        manifest,
        PRIVATE_MANIFEST_MAX_BYTES,
        FilePolicy::Private,
        "private manifest",
    )?;
    let raw: PrivateManifestV1 =
        serde_json::from_slice(&bytes).context("private manifest is malformed")?;
    ensure!(
        raw.canonical_bytes()? == bytes,
        "private manifest is noncanonical"
    );
    ensure!(
        raw.schema_version == PRIVATE_MANIFEST_SCHEMA_V1,
        "private manifest schema is unsupported"
    );
    let owner = decode_exact(&raw.lez_owner_account)?;
    ensure!(owner != [0; 32], "private manifest owner is invalid");
    Ok(ValidatedPrivateManifest {
        role: raw.role,
        lez_owner_account: owner,
        public_packet_sha256: decode_exact(&raw.public_packet_sha256)?,
    })
}

fn read_secret_hex_at(root: &File, name: &str, label: &'static str) -> Result<Zeroizing<[u8; 32]>> {
    let file = openat(
        root,
        name,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| anyhow!("private {label} is unavailable"))?;
    let encoded = Zeroizing::new(read_bounded_file(
        file,
        PRIVATE_KEY_MAX_BYTES,
        FilePolicy::Private,
        label,
    )?);
    let trimmed = encoded
        .strip_suffix(b"\r\n")
        .or_else(|| encoded.strip_suffix(b"\n"))
        .unwrap_or(&encoded);
    decode_secret_exact(trimmed)
}

fn publish_private_bundle(
    destination: &SecureDestination,
    agreement: &GeneratedSecpKey,
    claim: &GeneratedSecpKey,
    refund: &GeneratedSecpKey,
    share: CrossCurveScalar,
    view_key: MoneroPrivateViewKey,
    manifest_bytes: &[u8],
) -> Result<()> {
    destination.revalidate()?;
    let (stage_name, stage) = create_staging_directory(&destination.parent)?;
    let mut published = false;
    let result = (|| {
        write_secret_hex_new_at(&stage, AGREEMENT_KEY_FILE, agreement.secret_bytes())?;
        write_secret_hex_new_at(&stage, CLAIM_KEY_FILE, claim.secret_bytes())?;
        write_secret_hex_new_at(&stage, REFUND_KEY_FILE, refund.secret_bytes())?;
        let share_bytes = share.into_monero_little_endian();
        write_secret_hex_new_at(&stage, XMR_SHARE_FILE, &share_bytes)?;
        drop(share_bytes);
        let view_bytes = view_key.into_monero_little_endian();
        write_secret_hex_new_at(&stage, VIEW_KEY_FILE, &view_bytes)?;
        drop(view_bytes);
        write_new_at(
            &stage,
            PRIVATE_MANIFEST_FILE,
            manifest_bytes,
            "private manifest",
        )?;
        stage.sync_all().context("sync private staging directory")?;
        validate_owner_directory(&stage, "private staging directory")?;
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
                anyhow!("private role root already exists")
            } else {
                anyhow!("publish private role root failed")
            }
        })?;
        published = true;
        destination
            .parent
            .sync_all()
            .context("sync private role parent")?;
        destination.revalidate()?;
        Ok(())
    })();
    if result.is_err() && !published {
        cleanup_staging_directory(&destination.parent, &stage_name, &stage);
    }
    result
}

fn publish_staged_file(
    destination: &SecureDestination,
    stage_name: &str,
    stage_file: &File,
    max_bytes: u64,
    label: &'static str,
) -> Result<()> {
    destination.revalidate()?;
    validate_file(stage_file, FilePolicy::Private, max_bytes, label)?;
    renameat_with(
        &destination.parent,
        stage_name,
        &destination.parent,
        &destination.name,
        RenameFlags::NOREPLACE,
    )
    .map_err(|error| {
        if error == Errno::EXIST {
            anyhow!("{label} already exists")
        } else {
            anyhow!("publish {label} failed")
        }
    })?;
    destination
        .parent
        .sync_all()
        .with_context(|| format!("sync {label} parent"))?;
    destination.revalidate()
}

fn write_bounded_public_new(
    destination: &SecureDestination,
    bytes: &[u8],
    max_bytes: u64,
    label: &'static str,
) -> Result<()> {
    destination.ensure_absent(label)?;
    let (stage_name, stage_file) =
        create_staged_file(&destination.parent, label, bytes, max_bytes, label)?;
    let result = publish_staged_file(destination, &stage_name, &stage_file, max_bytes, label);
    if result.is_err() {
        cleanup_staged_file(&destination.parent, &stage_name);
    }
    result
}

fn read_public_input(path: &Path, max_bytes: u64, label: &'static str) -> Result<Vec<u8>> {
    let file = open_path_no_symlinks(path, label)?;
    read_bounded_file(file, max_bytes, FilePolicy::Public, label)
}

fn read_fixed_public<const N: usize>(path: &Path, label: &'static str) -> Result<[u8; N]> {
    let bytes = read_public_input(path, N as u64, label)?;
    bytes
        .try_into()
        .map_err(|_| anyhow!("{label} has the wrong width"))
}

#[cfg(feature = "sessions")]
fn publish_session_bundle(
    destination: &SecureDestination,
    claim: &ValidatedSession,
    refund: &ValidatedSession,
) -> Result<()> {
    destination.ensure_absent("session root")?;
    destination.revalidate()?;
    let (stage_name, stage) = create_staging_directory(&destination.parent)?;
    let mut published = false;
    let result = (|| {
        write_session_member(
            destination,
            &stage_name,
            &stage,
            CLAIM_SESSION_FILE,
            claim,
            "claim session",
        )?;
        write_session_member(
            destination,
            &stage_name,
            &stage,
            REFUND_SESSION_FILE,
            refund,
            "refund session",
        )?;
        validate_complete_session_bundle(&stage, "session staging root")?;
        stage.sync_all().context("sync session staging root")?;
        validate_named_directory(
            &destination.parent,
            Path::new(&stage_name),
            &stage,
            "session staging root",
        )?;
        validate_complete_session_bundle(&stage, "session staging root")?;
        destination.ensure_absent("session root")?;
        renameat_with(
            &destination.parent,
            stage_name.as_str(),
            &destination.parent,
            &destination.name,
            RenameFlags::NOREPLACE,
        )
        .map_err(|error| {
            if error == Errno::EXIST {
                anyhow!("session root already exists")
            } else {
                anyhow!("publish session root failed")
            }
        })?;
        published = true;
        destination
            .parent
            .sync_all()
            .context("sync session-root parent; complete root may already be published")?;
        destination.revalidate()?;
        validate_named_directory(
            &destination.parent,
            Path::new(&destination.name),
            &stage,
            "published session root",
        )?;
        validate_complete_session_bundle(&stage, "published session root")?;
        Ok(())
    })();
    if result.is_err() && !published {
        cleanup_staging_directory_with_files(
            &destination.parent,
            &stage_name,
            &stage,
            &SESSION_BUNDLE_FILES,
        );
    }
    result
}

#[cfg(feature = "sessions")]
fn write_session_member(
    destination: &SecureDestination,
    stage_name: &str,
    stage: &File,
    member_name: &str,
    session: &ValidatedSession,
    label: &'static str,
) -> Result<()> {
    destination.revalidate()?;
    validate_named_directory(
        &destination.parent,
        Path::new(stage_name),
        stage,
        "session staging root",
    )?;
    let stage_path = destination.parent_path.join(stage_name).join(member_name);
    let write_result = session
        .write_new(&stage_path)
        .with_context(|| format!("write staged {label}"));
    write_result?;
    destination.revalidate()?;
    validate_named_directory(
        &destination.parent,
        Path::new(stage_name),
        stage,
        "session staging root",
    )?;
    validate_session_file(stage, member_name, label)
}

#[cfg(feature = "sessions")]
fn validate_session_file(directory: &File, name: &str, label: &'static str) -> Result<()> {
    let file = openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| anyhow!("open staged {label} failed"))?;
    let bytes = read_bounded_file(file, SESSION_FILE_MAX_BYTES, FilePolicy::Private, label)?;
    ensure!(!bytes.is_empty(), "staged {label} is empty");
    Ok(())
}

#[cfg(feature = "sessions")]
fn validate_complete_session_bundle(directory: &File, label: &'static str) -> Result<()> {
    validate_exact_directory_entries(directory, &SESSION_BUNDLE_FILES, label)?;
    validate_session_file(directory, CLAIM_SESSION_FILE, "claim session")?;
    validate_session_file(directory, REFUND_SESSION_FILE, "refund session")
}

#[cfg(feature = "sessions")]
fn validate_named_directory(
    parent: &File,
    name: &Path,
    held: &File,
    label: &'static str,
) -> Result<()> {
    validate_owner_directory(held, label)?;
    let current = openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| anyhow!("{label} is unavailable or unsafe"))?;
    validate_owner_directory(&current, label)?;
    let held_metadata = held
        .metadata()
        .with_context(|| format!("inspect {label}"))?;
    let current_metadata = current
        .metadata()
        .with_context(|| format!("inspect {label}"))?;
    ensure!(
        held_metadata.dev() == current_metadata.dev()
            && held_metadata.ino() == current_metadata.ino(),
        "{label} changed"
    );
    Ok(())
}

#[cfg(feature = "sessions")]
fn validate_exact_directory_entries(
    directory: &File,
    expected: &[&str],
    label: &'static str,
) -> Result<()> {
    let mut stream = Dir::read_from(directory).map_err(|_| anyhow!("inspect {label} failed"))?;
    let mut actual = Vec::new();
    while let Some(entry) = stream.read() {
        let entry = entry.map_err(|_| anyhow!("inspect {label} failed"))?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
            actual.push(name.to_vec());
        }
    }
    actual.sort_unstable();
    let mut expected = expected
        .iter()
        .map(|name| name.as_bytes().to_vec())
        .collect::<Vec<_>>();
    expected.sort_unstable();
    ensure!(actual == expected, "{label} has unexpected entries");
    Ok(())
}

fn read_private_view_key(path: &Path) -> Result<MoneroPrivateViewKey> {
    let file = open_path_no_symlinks(path, "private key file")?;
    let encoded = Zeroizing::new(read_bounded_file(
        file,
        PRIVATE_KEY_MAX_BYTES,
        FilePolicy::Private,
        "private key file",
    )?);
    let trimmed = encoded
        .strip_suffix(b"\r\n")
        .or_else(|| encoded.strip_suffix(b"\n"))
        .unwrap_or(&encoded);
    let bytes = decode_secret_exact(trimmed)?;
    MoneroPrivateViewKey::from_monero_little_endian(*bytes)
        .context("private Monero view key is invalid")
}

fn validate_intra_role_keys(encoded: [[u8; 33]; 3], proof: &CrossCurveDleqProofV1) -> Result<()> {
    ensure!(
        encoded[0] != encoded[1] && encoded[0] != encoded[2] && encoded[1] != encoded[2],
        "public role signing keys are aliased"
    );
    let parsed = encoded.map(|bytes| {
        PublicKey::from_slice(&bytes).expect("public role keys were parsed before validation")
    });
    let x_only = parsed.map(|key| key.x_only_public_key().0.serialize());
    ensure!(
        x_only[0] != x_only[1] && x_only[0] != x_only[2] && x_only[1] != x_only[2],
        "public role signing keys have aliased x-only identities"
    );
    let proof_x_only = PublicKey::from_slice(&proof.secp256k1_public_key())
        .context("public role DLEQ point is invalid")?
        .x_only_public_key()
        .0
        .serialize();
    ensure!(
        !x_only.contains(&proof_x_only),
        "public role DLEQ point aliases a signing key"
    );
    Ok(())
}

struct GeneratedSecpKey {
    key: SecretKey,
    bytes: Zeroizing<[u8; 32]>,
}

impl GeneratedSecpKey {
    fn generate(rng: &mut (impl CryptoRng + RngCore)) -> Self {
        loop {
            let mut bytes = Zeroizing::new([0; 32]);
            rng.fill_bytes(&mut *bytes);
            if let Ok(key) = SecretKey::from_slice(bytes.as_ref()) {
                return Self { key, bytes };
            }
        }
    }

    fn secret_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    fn public_key(&self, secp: &Secp256k1<secp256k1::All>) -> PublicKey {
        PublicKey::from_secret_key(secp, &self.key)
    }
}

impl Drop for GeneratedSecpKey {
    fn drop(&mut self) {
        self.key.non_secure_erase();
    }
}

fn fallible_seeded_rng() -> Result<StdRng> {
    let mut seed = Zeroizing::new([0; 32]);
    OsRng
        .try_fill_bytes(&mut *seed)
        .context("operating-system entropy is unavailable")?;
    Ok(StdRng::from_seed(*seed))
}

struct SecureDestination {
    parent: File,
    parent_path: PathBuf,
    name: OsString,
}

impl SecureDestination {
    fn new(path: &Path, label: &'static str) -> Result<Self> {
        let name = path
            .file_name()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| anyhow!("{label} path is invalid"))?
            .to_os_string();
        let parent_path = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            Some(_) | None => PathBuf::from("."),
        };
        let parent = open_private_directory(&parent_path, "destination parent")?;
        Ok(Self {
            parent,
            parent_path,
            name,
        })
    }

    fn ensure_absent(&self, label: &'static str) -> Result<()> {
        self.revalidate()?;
        match openat(
            &self.parent,
            &self.name,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Err(Errno::NOENT) => Ok(()),
            Ok(_) | Err(Errno::LOOP) => Err(anyhow!("{label} already exists")),
            Err(_) => Err(anyhow!("{label} destination is unsafe")),
        }
    }

    fn revalidate(&self) -> Result<()> {
        validate_owner_directory(&self.parent, "destination parent")?;
        let reopened = open_private_directory(&self.parent_path, "destination parent")?;
        let held = self
            .parent
            .metadata()
            .context("inspect destination parent")?;
        let current = reopened.metadata().context("inspect destination parent")?;
        ensure!(
            held.dev() == current.dev() && held.ino() == current.ino(),
            "destination parent changed"
        );
        Ok(())
    }
}

fn open_private_directory(path: &Path, label: &'static str) -> Result<File> {
    let file = openat2(
        CWD,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .map(File::from)
    .map_err(|_| anyhow!("{label} is unavailable or unsafe"))?;
    validate_owner_directory(&file, label)?;
    Ok(file)
}

fn validate_owner_directory(file: &File, label: &'static str) -> Result<()> {
    let metadata = file
        .metadata()
        .map_err(|_| anyhow!("{label} is unavailable or unsafe"))?;
    ensure!(
        metadata.file_type().is_dir()
            && metadata.uid() == effective_uid()
            && metadata.mode() & 0o7777 == 0o700,
        "{label} is not an exact owner-only directory"
    );
    Ok(())
}

fn effective_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

fn open_path_no_symlinks(path: &Path, label: &'static str) -> Result<File> {
    openat2(
        CWD,
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .map(File::from)
    .map_err(|_| anyhow!("{label} is unavailable or unsafe"))
}

#[derive(Clone, Copy)]
enum FilePolicy {
    Public,
    Private,
}

fn read_bounded_file(
    mut file: File,
    max_bytes: u64,
    policy: FilePolicy,
    label: &'static str,
) -> Result<Vec<u8>> {
    let before = validate_file(&file, policy, max_bytes, label)?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("read {label} failed"))?;
    ensure!(
        u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= max_bytes,
        "{label} is oversized"
    );
    let after = validate_file(&file, policy, max_bytes, label)?;
    ensure!(
        before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.len() == after.len()
            && after.len() == u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "{label} changed while it was read"
    );
    Ok(bytes)
}

fn validate_file(
    file: &File,
    policy: FilePolicy,
    max_bytes: u64,
    label: &'static str,
) -> Result<std::fs::Metadata> {
    let metadata = file
        .metadata()
        .map_err(|_| anyhow!("inspect {label} failed"))?;
    let private_ok = match policy {
        FilePolicy::Public => true,
        FilePolicy::Private => {
            metadata.uid() == effective_uid() && metadata.mode() & 0o7777 == 0o600
        }
    };
    ensure!(
        metadata.file_type().is_file()
            && metadata.nlink() == 1
            && metadata.len() <= max_bytes
            && private_ok,
        "{label} is unsafe or oversized"
    );
    Ok(metadata)
}

fn create_staging_directory(parent: &File) -> Result<(String, File)> {
    let stage_name = temporary_name("private")?;
    mkdirat(
        parent,
        stage_name.as_str(),
        Mode::RUSR | Mode::WUSR | Mode::XUSR,
    )
    .map_err(|_| anyhow!("create private staging directory failed"))?;
    let open_result = openat(
        parent,
        stage_name.as_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| anyhow!("open private staging directory failed"));
    let stage = match open_result {
        Ok(stage) => stage,
        Err(error) => {
            let _ = unlinkat(parent, stage_name.as_str(), AtFlags::REMOVEDIR);
            let _ = parent.sync_all();
            return Err(error);
        }
    };
    if let Err(error) = validate_owner_directory(&stage, "private staging directory") {
        let _ = unlinkat(parent, stage_name.as_str(), AtFlags::REMOVEDIR);
        let _ = parent.sync_all();
        return Err(error);
    }
    Ok((stage_name, stage))
}

fn create_staged_file(
    parent: &File,
    kind: &str,
    bytes: &[u8],
    max_bytes: u64,
    label: &'static str,
) -> Result<(String, File)> {
    ensure!(
        u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= max_bytes,
        "{label} is oversized"
    );
    let stage_name = temporary_name(kind)?;
    let mut file = openat(
        parent,
        stage_name.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map(File::from)
    .map_err(|_| anyhow!("create staged {label} failed"))?;
    let result = (|| {
        validate_file(&file, FilePolicy::Private, max_bytes, label)?;
        file.write_all(bytes)
            .with_context(|| format!("write staged {label}"))?;
        file.sync_all()
            .with_context(|| format!("sync staged {label}"))?;
        let metadata = validate_file(&file, FilePolicy::Private, max_bytes, label)?;
        ensure!(
            metadata.len() == u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            "staged {label} has the wrong length"
        );
        parent
            .sync_all()
            .with_context(|| format!("sync {label} staging"))
    })();
    if let Err(error) = result {
        cleanup_staged_file(parent, &stage_name);
        return Err(error);
    }
    Ok((stage_name, file))
}

fn write_secret_hex_new_at(directory: &File, name: &str, bytes: &[u8; 32]) -> Result<()> {
    let mut encoded = Zeroizing::new(hex::encode(bytes));
    encoded.push('\n');
    write_new_at(directory, name, encoded.as_bytes(), "private material")
}

fn write_new_at(directory: &File, name: &str, bytes: &[u8], label: &'static str) -> Result<()> {
    let mut file = openat(
        directory,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map(File::from)
    .map_err(|_| anyhow!("create new {label} failed"))?;
    validate_file(
        &file,
        FilePolicy::Private,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        label,
    )?;
    file.write_all(bytes)
        .map_err(|_| anyhow!("write {label} failed"))?;
    file.sync_all()
        .map_err(|_| anyhow!("sync {label} failed"))?;
    let metadata = validate_file(
        &file,
        FilePolicy::Private,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        label,
    )?;
    ensure!(
        metadata.len() == u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "written {label} has the wrong length"
    );
    Ok(())
}

fn cleanup_staged_file(parent: &File, stage_name: &str) {
    let _ = unlinkat(parent, stage_name, AtFlags::empty());
    let _ = parent.sync_all();
}

fn cleanup_staging_directory(parent: &File, stage_name: &str, stage: &File) {
    cleanup_staging_directory_with_files(parent, stage_name, stage, &PRIVATE_BUNDLE_FILES);
}

fn cleanup_staging_directory_with_files(
    parent: &File,
    stage_name: &str,
    stage: &File,
    files: &[&str],
) {
    for name in files {
        let _ = unlinkat(stage, *name, AtFlags::empty());
    }
    let _ = unlinkat(parent, stage_name, AtFlags::REMOVEDIR);
    let _ = parent.sync_all();
}

fn temporary_name(kind: &str) -> Result<String> {
    let mut random = Zeroizing::new([0; 16]);
    OsRng
        .try_fill_bytes(&mut *random)
        .context("operating-system entropy is unavailable")?;
    Ok(format!(
        ".xmr-reference-actor-{kind}-{}-{}",
        std::process::id(),
        hex::encode(*random)
    ))
}

fn canonical_json_bytes(value: &impl Serialize, label: &'static str) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value).with_context(|| label)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn decode_public_key(encoded: &str) -> Result<[u8; 33]> {
    let bytes = decode_exact(encoded)?;
    PublicKey::from_slice(&bytes).context("public role key is invalid")?;
    Ok(bytes)
}

fn decode_exact<const N: usize>(encoded: &str) -> Result<[u8; N]> {
    ensure!(
        encoded.len() == N * 2
            && encoded
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "canonical lowercase hex is invalid"
    );
    hex::decode(encoded)
        .context("canonical lowercase hex is invalid")?
        .try_into()
        .map_err(|_| anyhow!("canonical lowercase hex has the wrong width"))
}

fn decode_secret_exact(encoded: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    ensure!(
        encoded.len() == 64
            && encoded
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)),
        "canonical private hex is invalid"
    );
    let mut result = Zeroizing::new([0; 32]);
    for (index, pair) in encoded.chunks_exact(2).enumerate() {
        result[index] = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
    }
    Ok(result)
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => unreachable!("private hex was validated before decoding"),
    }
}

fn decode_vec(encoded: &str) -> Result<Vec<u8>> {
    ensure!(
        encoded.len().is_multiple_of(2)
            && encoded.len() <= ROLE_PACKET_MAX_HEX_CHARS
            && encoded
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "canonical lowercase hex is invalid"
    );
    hex::decode(encoded).context("canonical lowercase hex is invalid")
}
