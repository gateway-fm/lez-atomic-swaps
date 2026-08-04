//! Role-fixed process boundary for independently composing M4 XMR material.
//!
//! Each invocation accepts exactly one private role root. Public packets may be
//! exchanged between roles; private signing keys and Monero scalars never
//! cross this crate's output boundary.

#![cfg_attr(not(unix), allow(unused_imports))]

#[cfg(not(unix))]
compile_error!("xmr-reference-actor requires Unix file-permission semantics");

#[cfg(feature = "sessions")]
mod application_provision;
#[cfg(feature = "sessions")]
mod effect_authority;
#[cfg(feature = "sessions")]
mod effect_input_custody;
#[cfg(feature = "sessions")]
mod effect_route;

#[cfg(feature = "sessions")]
pub use application_provision::{
    ValidatedXmrEffectExecutionV3, ValidatedXmrMakerAuthorityV2, ValidatedXmrTakerAuthorityV2,
    XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES, XMR_MAKER_ACTOR_ABI_V1, XMR_MAKER_ACTOR_NEXT_ACTION,
    XMR_MAKER_ACTOR_PROGRAM_ID, XmrActorProvisionV1, XmrEffectProvisionV3,
    load_validated_xmr_effect_execution_v3_bytes, load_validated_xmr_effect_manifest_v3_bytes,
    load_validated_xmr_maker_authority_fd, load_validated_xmr_taker_authority_bytes,
    provision_xmr_effect_manifest_v3, provision_xmr_maker_actor_from_material,
    provision_xmr_taker_actor_from_material, publish_xmr_effect_manifest_v3,
    validate_maker_manifest_config_bytes, validate_taker_manifest_config_bytes,
    validate_xmr_effect_manifest_v3_projection_bytes,
};
#[cfg(feature = "sessions")]
pub use effect_authority::{
    ValidatedXmrEffectAuthorityV1, XMR_EFFECT_AUTHORITY_MAX_BYTES, XmrEffectAuthenticatedRpcV1,
    XmrEffectLezRpcV1, XmrEffectMoneroRpcV1, XmrEffectToolV1, XmrMakerEffectToolsV1,
    XmrTakerEffectToolsV1, load_validated_xmr_effect_authority_bytes,
};
#[cfg(feature = "sessions")]
pub use effect_input_custody::{
    PinnedXmrEffectInputsV1, PinnedXmrEffectMoneroCredentialsV1, PinnedXmrEffectRpcCredentialsV1,
    PinnedXmrEffectSecretV1, XMR_EFFECT_OWN_PUBLIC_PACKET_FD, XMR_EFFECT_PEER_PUBLIC_PACKET_FD,
    XMR_EFFECT_PRIVATE_MANIFEST_FD, XMR_EFFECT_PRIVATE_VIEW_KEY_FD, XMR_EFFECT_STAGE_A_FD,
    XMR_EFFECT_STAGE_B_FD,
};
#[cfg(feature = "sessions")]
pub use effect_route::{
    XMR_EFFECT_OBSERVER_RESULT_MAX_BYTES, XmrEffectObserverResultV1, XmrEffectObserverStateV1,
    XmrPreparedEffectInvocationV1, XmrPreparedEffectObservationV1,
    parse_xmr_effect_observer_result_v1,
};

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
use lez_adaptor_role_runner::{
    Role as RunnerRole, ValidatedSession, accept_published_peer_partial_and_adapt,
    read_final_signature_packet, verify_extracted_adaptor_secret,
    write_observed_final_signature_packet,
};
#[cfg(feature = "sessions")]
use lez_bridge_adapter::XmrLezBridgeBindingV3;
#[cfg(feature = "sessions")]
use lez_bridge_protocol::{
    ClassifyFinalizedNativeXmrEffectV3Result, FinalizedNativeXmrEffectFactsV3,
    FinalizedNativeXmrScanOutcomeV3, FinalizedNativeXmrTransactionTargetV3,
    Participant as BridgeParticipant, RunId, XmrNativeEffectV3, XmrNativeEscrowTermsV3,
};
#[cfg(feature = "sessions")]
use lez_swap_store::{AdaptorSessionPhase, SqliteAdaptorSessionJournal};
use lez_xmr_swap_sdk::{
    CrossCurveDleqProofV1, CrossCurveScalar, MAX_XMR_AGREEMENT_WIRE_BYTES,
    MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES, MoneroPrivateViewKey, ValidatedXmrAgreementBodyV1,
    XmrAgreementBodyV1, XmrAgreementV1, XmrParticipantIdentityV1, XmrRoleV1,
};
#[cfg(feature = "sessions")]
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES, MoneroAddressNetworkV1,
    ReconstructedMoneroSpendKey, ValidatedXmrActivationBodyV1, XmrActivatedAgreementV1,
    XmrActivationBodyV1, XmrSessionTranscriptV1,
};
#[cfg(feature = "sessions")]
use monero::Address as MoneroAddress;
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
#[cfg(feature = "sessions")]
use serde::de::DeserializeOwned;
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
const FINALIZED_XMR_EFFECT_MAX_BYTES: u64 = 5 * 1024 * 1024;
#[cfg(feature = "sessions")]
const M4_MONERO_EVIDENCE_MAX_BYTES: u64 = 16 * 1024;
#[cfg(feature = "sessions")]
const M4_BINDING_EVIDENCE_MAX_BYTES: u64 = 16 * 1024;
#[cfg(feature = "sessions")]
const M4_MONERO_CONFIRMATIONS: u64 = 10;
#[cfg(feature = "sessions")]
const M4_MONERO_LEGACY_SWEEP_SCHEMA: &str = "lez_v02_m4_actual_local_monero_claim_sweep_v1";
#[cfg(feature = "sessions")]
const M4_MONERO_CURRENT_SWEEP_SCHEMA: &str = "lez_v02_m4_actual_local_monero_claim_sweep_v2";
#[cfg(feature = "sessions")]
const M4_MONERO_RECEIPT_SCHEMA: &str = "lez_v02_m4_actual_local_monero_verification_v2";
#[cfg(feature = "sessions")]
const M4_MONERO_NETWORK_SCOPE: &str = "isolated_official_monero_regtest";
#[cfg(feature = "sessions")]
const M4_MONERO_DAEMON_VERSION: &str = "0.18.5.1-release";
#[cfg(feature = "sessions")]
const M4_MONERO_WALLET_VERSION: u32 = 65_567;
#[cfg(feature = "sessions")]
const M4_CLAIM_SWEEP_BINDING_SCHEMA: &str = "lez_v02_m4_claim_cross_chain_binding_v1";
#[cfg(feature = "sessions")]
const M5_MONERO_REFUND_SWEEP_SCHEMA: &str = "lez_v02_m5_actual_local_monero_refund_sweep_v3";
#[cfg(feature = "sessions")]
const M5_REFUND_SWEEP_BINDING_SCHEMA: &str = "lez_v02_m5_refund_cross_chain_binding_v1";
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
    /// Publish a role-fixed application bundle from canonical Stage A/B and private authority.
    #[cfg(feature = "sessions")]
    ProvisionApplication {
        /// Fixed role bound by the source private root and journal.
        #[arg(value_enum)]
        role: ActorRole,
        /// Existing owner-only role material root.
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
        /// Canonical countersigned Stage-B wire.
        #[arg(long, value_name = "STAGE_B")]
        activation_stage_b: PathBuf,
        /// Existing owner-private role adaptor journal.
        #[arg(long, value_name = "PRIVATE_SQLITE")]
        role_journal: PathBuf,
        /// New owner-private no-clobber application root.
        #[arg(long, value_name = "NEW_ACTOR_ROOT")]
        output_root: PathBuf,
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
    /// Consume role-local finalized tag-14 discovery, complete the Maker claim presignature,
    /// and adapt it with the retained Maker Monero share.
    #[cfg(feature = "sessions")]
    CompleteClaimFromFinalizedAuthorization {
        /// Existing owner-only Maker role root.
        #[arg(long, value_name = "PRIVATE_ROOT")]
        private_root: PathBuf,
        /// Maker canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        own_public_packet: PathBuf,
        /// Taker canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        peer_public_packet: PathBuf,
        /// Canonical countersigned Stage-A wire.
        #[arg(long, value_name = "STAGE_A")]
        agreement_stage_a: PathBuf,
        /// Canonical countersigned Stage-B activation wire.
        #[arg(long, value_name = "STAGE_B")]
        activation_stage_b: PathBuf,
        /// Existing owner-private Maker journal containing the claim session.
        #[arg(long, value_name = "PRIVATE_SQLITE")]
        journal: PathBuf,
        /// Run identity echoed by the role-local finalized classifier.
        #[arg(long)]
        run_id: String,
        /// Canonical Maker-sidecar `DiscoverByTerms` result for tag 14.
        #[arg(long, value_name = "FINALIZED_JSON")]
        finalized_authorization: PathBuf,
        /// New canonical aggregate final-signature packet for tag 15 completion.
        #[arg(long, value_name = "NEW_PUBLIC_JSON")]
        output_final_signature: PathBuf,
    },
    /// Convert role-local finalized tag-15 discovery into the Taker's extraction packet after
    /// proving it opens the existing durable claim presignature.
    #[cfg(feature = "sessions")]
    IngestFinalizedClaimSignature {
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
        /// Canonical countersigned Stage-B activation wire.
        #[arg(long, value_name = "STAGE_B")]
        activation_stage_b: PathBuf,
        /// Existing owner-private Taker journal containing the claim presignature.
        #[arg(long, value_name = "PRIVATE_SQLITE")]
        journal: PathBuf,
        /// Run identity echoed by the role-local finalized classifier.
        #[arg(long)]
        run_id: String,
        /// Canonical Taker-sidecar `DiscoverByTerms` result for tag 15.
        #[arg(long, value_name = "FINALIZED_JSON")]
        finalized_claim: PathBuf,
        /// New canonical final-signature packet for extraction/reconstruction.
        #[arg(long, value_name = "NEW_PUBLIC_JSON")]
        output_final_signature: PathBuf,
    },
    /// Convert role-local finalized tag-16 discovery into the Maker's extraction packet after
    /// proving it opens the existing durable refund presignature.
    #[cfg(feature = "sessions")]
    IngestFinalizedRefundSignature {
        /// Existing owner-only Maker role root.
        #[arg(long, value_name = "PRIVATE_ROOT")]
        private_root: PathBuf,
        /// Maker canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        own_public_packet: PathBuf,
        /// Taker canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        peer_public_packet: PathBuf,
        /// Canonical countersigned Stage-A wire.
        #[arg(long, value_name = "STAGE_A")]
        agreement_stage_a: PathBuf,
        /// Canonical countersigned Stage-B activation wire.
        #[arg(long, value_name = "STAGE_B")]
        activation_stage_b: PathBuf,
        /// Existing owner-private Maker journal containing the refund presignature.
        #[arg(long, value_name = "PRIVATE_SQLITE")]
        journal: PathBuf,
        /// Run identity echoed by the role-local finalized classifier.
        #[arg(long)]
        run_id: String,
        /// Canonical Maker-sidecar `DiscoverByTerms` result for tag 16.
        #[arg(long, value_name = "FINALIZED_JSON")]
        finalized_refund: PathBuf,
        /// New owner-private canonical final-signature packet for extraction/reconstruction.
        #[arg(long, value_name = "NEW_PRIVATE_JSON")]
        output_final_signature: PathBuf,
    },
    /// Bind finalized LEZ Claim evidence and its verified adaptor extraction to one
    /// independently verified actual-local Monero sweep.
    #[cfg(feature = "sessions")]
    BindFinalizedClaimSweep {
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
        /// Canonical countersigned Stage-B activation wire.
        #[arg(long, value_name = "STAGE_B")]
        activation_stage_b: PathBuf,
        /// Existing owner-private Taker journal containing the claim presignature.
        #[arg(long, value_name = "PRIVATE_SQLITE")]
        journal: PathBuf,
        /// Monero child-run identity echoed by the sweep and receipt evidence.
        #[arg(long)]
        run_id: String,
        /// Parent-run identity echoed by the finalized LEZ claim classifier.
        #[arg(long)]
        claim_run_id: String,
        /// Canonical Taker-sidecar `DiscoverByTerms` result for finalized tag 15.
        #[arg(long, value_name = "FINALIZED_JSON")]
        finalized_claim: PathBuf,
        /// Canonical observed final-signature packet previously ingested from tag 15.
        #[arg(long, value_name = "PUBLIC_JSON")]
        observed_final_signature: PathBuf,
        /// Owner-private scalar extracted from that observed signature.
        #[arg(long, value_name = "PRIVATE_SCALAR")]
        extracted_maker_adaptor_scalar: PathBuf,
        /// Original actual-local Monero sweep-effect evidence v1.
        #[arg(long, value_name = "PRIVATE_JSON")]
        monero_sweep_evidence: PathBuf,
        /// Independent canonical Monero receipt/topology verification evidence v2.
        #[arg(long, value_name = "PRIVATE_JSON")]
        monero_receipt_evidence: PathBuf,
        /// New owner-private canonical cross-chain binding evidence.
        #[arg(long, value_name = "NEW_PRIVATE_JSON")]
        output_binding_evidence: PathBuf,
    },
    /// Bind finalized LEZ Refund evidence and its verified adaptor extraction to one
    /// independently verified actual-local Monero refund sweep.
    #[cfg(feature = "sessions")]
    BindFinalizedRefundSweep {
        /// Existing owner-only Maker role root.
        #[arg(long, value_name = "PRIVATE_ROOT")]
        private_root: PathBuf,
        /// Maker canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        own_public_packet: PathBuf,
        /// Taker canonical public packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        peer_public_packet: PathBuf,
        /// Canonical countersigned Stage-A wire.
        #[arg(long, value_name = "STAGE_A")]
        agreement_stage_a: PathBuf,
        /// Canonical countersigned Stage-B activation wire.
        #[arg(long, value_name = "STAGE_B")]
        activation_stage_b: PathBuf,
        /// Existing owner-private Maker journal containing the refund presignature.
        #[arg(long, value_name = "PRIVATE_SQLITE")]
        journal: PathBuf,
        /// Monero child-run identity echoed by the refund sweep and receipt evidence.
        #[arg(long)]
        run_id: String,
        /// Parent-run identity echoed by the finalized LEZ refund classifier.
        #[arg(long)]
        refund_run_id: String,
        /// Canonical Maker-sidecar `DiscoverByTerms` result for finalized tag 16.
        #[arg(long, value_name = "FINALIZED_JSON")]
        finalized_refund: PathBuf,
        /// Owner-private observed final-signature packet previously ingested from tag 16.
        #[arg(long, value_name = "PRIVATE_JSON")]
        observed_final_signature: PathBuf,
        /// Owner-private Taker scalar extracted from that observed signature.
        #[arg(long, value_name = "PRIVATE_SCALAR")]
        extracted_taker_adaptor_scalar: PathBuf,
        /// Canonical actual-local Monero refund sweep-effect evidence v3.
        #[arg(long, value_name = "PRIVATE_JSON")]
        monero_sweep_evidence: PathBuf,
        /// Independent canonical Monero receipt/topology verification evidence v2.
        #[arg(long, value_name = "PRIVATE_JSON")]
        monero_receipt_evidence: PathBuf,
        /// New owner-private conditional cross-chain refund binding evidence.
        #[arg(long, value_name = "NEW_PRIVATE_JSON")]
        output_binding_evidence: PathBuf,
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

#[cfg(feature = "sessions")]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct MoneroSweepEvidenceV1 {
    schema: String,
    agreement_commitment: String,
    shared_address: String,
    reconstructed_public_spend_key: String,
    destination_address: String,
    funded_amount_piconero: u64,
    transaction_id: String,
    confirmation_tip_height: u64,
    required_confirmations: u64,
    restore_height: u64,
    revealed_role: String,
    sweeping_role: String,
    network_scope: String,
    public_rpc_used: bool,
    faucet_used: bool,
    automatic_submission_retry: bool,
}

#[cfg(feature = "sessions")]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct MoneroReceiptEvidenceV2 {
    schema: String,
    run_id: String,
    agreement_commitment: String,
    monero_genesis_hash: String,
    transaction_id: String,
    destination_address: String,
    amount_piconero: u64,
    containing_block_hash: String,
    containing_block_height: u64,
    confirmations: u64,
    stable_tip_hash: String,
    stable_tip_height: u64,
    peer_count: u64,
    daemon_version: String,
    target_wallet_version: u32,
    foreign_wallet_version: u32,
    network_scope: String,
    public_rpc_used: bool,
    faucet_used: bool,
}

#[cfg(feature = "sessions")]
// These booleans are intentional independent wire claims/resource facts, not a state machine;
// collapsing them would make unsafe downstream claim upgrades harder to detect.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct ClaimSweepBindingEvidenceV1 {
    schema: &'static str,
    run_id: String,
    agreement_commitment: String,
    activation_commitment: String,
    claim_context_binding: String,
    atomicity_scope: &'static str,
    distributed_cross_chain_transaction_claimed: bool,
    future_reorg_immunity_claimed: bool,
    lez_effect: &'static str,
    lez_sidecar_role: &'static str,
    classifier_target: &'static str,
    classifier_outcome: &'static str,
    classifier_request_id: String,
    classifier_result_sha256: String,
    classifier_scan_start_height: u64,
    classifier_scan_max_blocks: u32,
    lez_claim_transaction_id: String,
    lez_claim_block_hash: String,
    lez_claim_block_height: u64,
    lez_claim_transaction_index: u32,
    lez_claim_block_timestamp_ms: u64,
    lez_finalized_tip_hash: String,
    lez_finalized_tip_height: u64,
    lez_finalized_tip_timestamp_ms: u64,
    aggregate_signature_sha256: String,
    observed_final_signature_packet_sha256: String,
    extraction_binding: &'static str,
    reconstructed_public_spend_key: String,
    monero_sweep_evidence_provenance: &'static str,
    monero_sweep_evidence_schema: &'static str,
    monero_sweep_evidence_sha256: String,
    monero_receipt_evidence_schema: &'static str,
    monero_receipt_evidence_sha256: String,
    monero_genesis_hash: String,
    monero_sweep_transaction_id: String,
    monero_evidenced_destination_address: String,
    destination_ownership_binding: &'static str,
    monero_daemon_version: String,
    monero_target_wallet_version: u32,
    monero_foreign_wallet_version: u32,
    monero_sweep_block_hash: String,
    monero_sweep_block_height: u64,
    monero_sweep_confirmations: u64,
    monero_stable_tip_hash: String,
    monero_stable_tip_height: u64,
    funded_amount_piconero: u64,
    received_amount_piconero: u64,
    fee_piconero: Option<u64>,
    unreceived_remainder_piconero: u64,
    peer_count: u64,
    network_scope: &'static str,
    public_rpc_used: bool,
    faucet_used: bool,
    automatic_submission_retry: bool,
}

#[cfg(feature = "sessions")]
// These booleans are independent evidence/resource claims. Keeping them explicit makes the
// deliberately conditional scope impossible to confuse with distributed transaction finality.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct RefundSweepBindingEvidenceV1 {
    schema: &'static str,
    run_id: String,
    agreement_commitment: String,
    activation_commitment: String,
    refund_context_binding: String,
    atomicity_scope: &'static str,
    distributed_cross_chain_transaction_claimed: bool,
    future_reorg_immunity_claimed: bool,
    lez_effect: &'static str,
    lez_sidecar_role: &'static str,
    classifier_target: &'static str,
    classifier_outcome: &'static str,
    classifier_request_id: String,
    classifier_result_sha256: String,
    classifier_scan_start_height: u64,
    classifier_scan_max_blocks: u32,
    lez_refund_transaction_id: String,
    lez_refund_block_hash: String,
    lez_refund_block_height: u64,
    lez_refund_transaction_index: u32,
    lez_refund_block_timestamp_ms: u64,
    lez_finalized_tip_hash: String,
    lez_finalized_tip_height: u64,
    lez_finalized_tip_timestamp_ms: u64,
    aggregate_signature_sha256: String,
    observed_final_signature_packet_sha256: String,
    extraction_binding: &'static str,
    reconstructed_public_spend_key: String,
    monero_sweep_evidence_provenance: &'static str,
    monero_sweep_evidence_schema: &'static str,
    monero_sweep_evidence_sha256: String,
    monero_receipt_evidence_schema: &'static str,
    monero_receipt_evidence_sha256: String,
    monero_genesis_hash: String,
    monero_sweep_transaction_id: String,
    monero_evidenced_destination_address: String,
    destination_ownership_binding: &'static str,
    monero_daemon_version: String,
    monero_target_wallet_version: u32,
    monero_foreign_wallet_version: u32,
    monero_sweep_block_hash: String,
    monero_sweep_block_height: u64,
    monero_sweep_confirmations: u64,
    monero_stable_tip_hash: String,
    monero_stable_tip_height: u64,
    funded_amount_piconero: u64,
    received_amount_piconero: u64,
    fee_piconero: u64,
    peer_count: u64,
    network_scope: &'static str,
    public_rpc_used: bool,
    faucet_used: bool,
    automatic_submission_retry: bool,
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
        #[cfg(feature = "sessions")]
        Action::ProvisionApplication {
            role,
            private_root,
            own_public_packet,
            peer_public_packet,
            agreement_stage_a,
            activation_stage_b,
            role_journal,
            output_root,
        } => provision_application_actor(
            role,
            &private_root,
            &own_public_packet,
            &peer_public_packet,
            &agreement_stage_a,
            &activation_stage_b,
            &role_journal,
            &output_root,
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
        #[cfg(feature = "sessions")]
        Action::CompleteClaimFromFinalizedAuthorization {
            private_root,
            own_public_packet,
            peer_public_packet,
            agreement_stage_a,
            activation_stage_b,
            journal,
            run_id,
            finalized_authorization,
            output_final_signature,
        } => complete_claim_from_finalized_authorization(
            &private_root,
            &own_public_packet,
            &peer_public_packet,
            &agreement_stage_a,
            &activation_stage_b,
            &journal,
            &run_id,
            &finalized_authorization,
            &output_final_signature,
        ),
        #[cfg(feature = "sessions")]
        Action::IngestFinalizedClaimSignature {
            private_root,
            own_public_packet,
            peer_public_packet,
            agreement_stage_a,
            activation_stage_b,
            journal,
            run_id,
            finalized_claim,
            output_final_signature,
        } => ingest_finalized_claim_signature(
            &private_root,
            &own_public_packet,
            &peer_public_packet,
            &agreement_stage_a,
            &activation_stage_b,
            &journal,
            &run_id,
            &finalized_claim,
            &output_final_signature,
        ),
        #[cfg(feature = "sessions")]
        Action::IngestFinalizedRefundSignature {
            private_root,
            own_public_packet,
            peer_public_packet,
            agreement_stage_a,
            activation_stage_b,
            journal,
            run_id,
            finalized_refund,
            output_final_signature,
        } => ingest_finalized_refund_signature(
            &private_root,
            &own_public_packet,
            &peer_public_packet,
            &agreement_stage_a,
            &activation_stage_b,
            &journal,
            &run_id,
            &finalized_refund,
            &output_final_signature,
        ),
        #[cfg(feature = "sessions")]
        Action::BindFinalizedClaimSweep {
            private_root,
            own_public_packet,
            peer_public_packet,
            agreement_stage_a,
            activation_stage_b,
            journal,
            run_id,
            claim_run_id,
            finalized_claim,
            observed_final_signature,
            extracted_maker_adaptor_scalar,
            monero_sweep_evidence,
            monero_receipt_evidence,
            output_binding_evidence,
        } => bind_finalized_claim_sweep(
            &private_root,
            &own_public_packet,
            &peer_public_packet,
            &agreement_stage_a,
            &activation_stage_b,
            &journal,
            &run_id,
            &claim_run_id,
            &finalized_claim,
            &observed_final_signature,
            &extracted_maker_adaptor_scalar,
            &monero_sweep_evidence,
            &monero_receipt_evidence,
            &output_binding_evidence,
        ),
        #[cfg(feature = "sessions")]
        Action::BindFinalizedRefundSweep {
            private_root,
            own_public_packet,
            peer_public_packet,
            agreement_stage_a,
            activation_stage_b,
            journal,
            run_id,
            refund_run_id,
            finalized_refund,
            observed_final_signature,
            extracted_taker_adaptor_scalar,
            monero_sweep_evidence,
            monero_receipt_evidence,
            output_binding_evidence,
        } => bind_finalized_refund_sweep(
            &private_root,
            &own_public_packet,
            &peer_public_packet,
            &agreement_stage_a,
            &activation_stage_b,
            &journal,
            &run_id,
            &refund_run_id,
            &finalized_refund,
            &observed_final_signature,
            &extracted_taker_adaptor_scalar,
            &monero_sweep_evidence,
            &monero_receipt_evidence,
            &output_binding_evidence,
        ),
    }
}

#[cfg(feature = "sessions")]
struct ValidatedClaimLifecycle {
    agreement: XmrAgreementV1,
    activation: XmrActivatedAgreementV1,
    binding: XmrLezBridgeBindingV3,
    session: ValidatedSession,
    material: ValidatedPrivateRoleMaterial,
}

#[cfg(feature = "sessions")]
#[allow(clippy::too_many_arguments)]
fn complete_claim_from_finalized_authorization(
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    agreement_stage_a: &Path,
    activation_stage_b: &Path,
    journal: &Path,
    run_id: &str,
    finalized_authorization: &Path,
    output_final_signature: &Path,
) -> Result<()> {
    let destination =
        SecureDestination::new(output_final_signature, "claim final-signature packet")?;
    destination.ensure_absent("claim final-signature packet")?;
    let lifecycle = load_claim_lifecycle(
        ActorRole::Maker,
        private_root,
        own_public_packet,
        peer_public_packet,
        agreement_stage_a,
        activation_stage_b,
    )?;
    let result = read_finalized_xmr_effect(finalized_authorization)?;
    let facts = discovered_finalized_xmr_facts(
        &result,
        run_id,
        &lifecycle.binding.terms(),
        XmrNativeEffectV3::AuthorizeClaim,
        BridgeParticipant::Maker,
    )?;
    let partial = facts
        .instruction
        .published_claim_partial
        .ok_or_else(|| anyhow!("finalized tag-14 facts omit the published partial"))?;
    lifecycle
        .activation
        .verify_published_taker_claim_partial(&lifecycle.agreement, *partial.as_bytes())
        .context("finalized tag-14 partial differs from Stage B")?;
    let adaptor_secret = lifecycle.material.share.adaptor_scalar_big_endian();
    accept_published_peer_partial_and_adapt(
        journal,
        &lifecycle.session,
        RunnerRole::Maker,
        *partial.as_bytes(),
        adaptor_secret,
        output_final_signature,
    )
    .context("complete Maker claim from finalized tag 14")?;
    destination.revalidate()
}

#[cfg(feature = "sessions")]
#[allow(clippy::too_many_arguments)]
fn ingest_finalized_claim_signature(
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    agreement_stage_a: &Path,
    activation_stage_b: &Path,
    journal: &Path,
    run_id: &str,
    finalized_claim: &Path,
    output_final_signature: &Path,
) -> Result<()> {
    let destination = SecureDestination::new(
        output_final_signature,
        "observed claim final-signature packet",
    )?;
    destination.ensure_absent("observed claim final-signature packet")?;
    let lifecycle = load_claim_lifecycle(
        ActorRole::Taker,
        private_root,
        own_public_packet,
        peer_public_packet,
        agreement_stage_a,
        activation_stage_b,
    )?;
    let result = read_finalized_xmr_effect(finalized_claim)?;
    let facts = discovered_finalized_xmr_facts(
        &result,
        run_id,
        &lifecycle.binding.terms(),
        XmrNativeEffectV3::Claim,
        BridgeParticipant::Taker,
    )?;
    let signature = facts
        .aggregate_signature
        .ok_or_else(|| anyhow!("finalized tag-15 facts omit the aggregate signature"))?;
    write_observed_final_signature_packet(
        journal,
        &lifecycle.session,
        RunnerRole::Taker,
        *signature.as_bytes(),
        output_final_signature,
    )
    .context("ingest finalized tag-15 signature into the Taker claim session")?;
    destination.revalidate()
}

#[cfg(feature = "sessions")]
#[allow(clippy::too_many_arguments)]
fn ingest_finalized_refund_signature(
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    agreement_stage_a: &Path,
    activation_stage_b: &Path,
    journal: &Path,
    run_id: &str,
    finalized_refund: &Path,
    output_final_signature: &Path,
) -> Result<()> {
    let destination = SecureDestination::new(
        output_final_signature,
        "observed refund final-signature packet",
    )?;
    destination.ensure_absent("observed refund final-signature packet")?;
    let lifecycle = load_refund_lifecycle(
        ActorRole::Maker,
        private_root,
        own_public_packet,
        peer_public_packet,
        agreement_stage_a,
        activation_stage_b,
    )?;
    let result = read_finalized_xmr_effect(finalized_refund)?;
    let facts = discovered_finalized_xmr_facts(
        &result,
        run_id,
        &lifecycle.binding.terms(),
        XmrNativeEffectV3::Refund,
        BridgeParticipant::Maker,
    )?;
    let signature = facts
        .aggregate_signature
        .ok_or_else(|| anyhow!("finalized tag-16 facts omit the aggregate signature"))?;
    write_observed_final_signature_packet(
        journal,
        &lifecycle.session,
        RunnerRole::Maker,
        *signature.as_bytes(),
        output_final_signature,
    )
    .context("ingest finalized tag-16 signature into the Maker refund session")?;
    destination.revalidate()
}

#[cfg(feature = "sessions")]
fn load_refund_lifecycle(
    role: ActorRole,
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    agreement_stage_a: &Path,
    activation_stage_b: &Path,
) -> Result<ValidatedClaimLifecycle> {
    let packets = StageRolePackets::read(role, own_public_packet, peer_public_packet)?;
    let material = validate_private_role(private_root, role, &packets)?;
    let agreement = read_validated_stage_a(agreement_stage_a, &packets)?;
    let wire = read_public_input(
        activation_stage_b,
        u64::try_from(MAX_XMR_ACTIVATION_WIRE_BYTES).unwrap_or(u64::MAX),
        "signed Stage-B activation wire",
    )?;
    let activation = XmrActivatedAgreementV1::from_wire(&agreement, &wire, &material.view)
        .context("signed Stage-B activation wire is invalid")?;
    let binding = XmrLezBridgeBindingV3::new(&agreement, &activation)
        .context("Stage-B LEZ binding is invalid")?;
    let session = ValidatedSession::from_untweaked_context(
        agreement
            .refund_session_descriptor()
            .context()
            .context("refund session descriptor is invalid")?,
    )
    .context("refund runner session is invalid")?;
    Ok(ValidatedClaimLifecycle {
        agreement,
        activation,
        binding,
        session,
        material,
    })
}

#[cfg(feature = "sessions")]
fn load_claim_lifecycle(
    role: ActorRole,
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    agreement_stage_a: &Path,
    activation_stage_b: &Path,
) -> Result<ValidatedClaimLifecycle> {
    let packets = StageRolePackets::read(role, own_public_packet, peer_public_packet)?;
    let material = validate_private_role(private_root, role, &packets)?;
    let agreement = read_validated_stage_a(agreement_stage_a, &packets)?;
    let wire = read_public_input(
        activation_stage_b,
        u64::try_from(MAX_XMR_ACTIVATION_WIRE_BYTES).unwrap_or(u64::MAX),
        "signed Stage-B activation wire",
    )?;
    let activation = XmrActivatedAgreementV1::from_wire(&agreement, &wire, &material.view)
        .context("signed Stage-B activation wire is invalid")?;
    let binding = XmrLezBridgeBindingV3::new(&agreement, &activation)
        .context("Stage-B LEZ binding is invalid")?;
    let session = ValidatedSession::from_untweaked_context(
        agreement
            .claim_session_descriptor()
            .context()
            .context("claim session descriptor is invalid")?,
    )
    .context("claim runner session is invalid")?;
    Ok(ValidatedClaimLifecycle {
        agreement,
        activation,
        binding,
        session,
        material,
    })
}

#[cfg(feature = "sessions")]
fn read_finalized_xmr_effect(path: &Path) -> Result<ClassifyFinalizedNativeXmrEffectV3Result> {
    let bytes = read_public_input(path, FINALIZED_XMR_EFFECT_MAX_BYTES, "finalized XMR effect")?;
    let result: ClassifyFinalizedNativeXmrEffectV3Result =
        serde_json::from_slice(&bytes).context("finalized XMR effect is malformed")?;
    ensure!(
        canonical_json_bytes(&result, "encode finalized XMR effect")? == bytes,
        "finalized XMR effect is noncanonical"
    );
    Ok(result)
}

#[cfg(feature = "sessions")]
fn discovered_finalized_xmr_facts<'result>(
    result: &'result ClassifyFinalizedNativeXmrEffectV3Result,
    expected_run_id: &str,
    expected_terms: &XmrNativeEscrowTermsV3,
    expected_effect: XmrNativeEffectV3,
    expected_sidecar_role: BridgeParticipant,
) -> Result<&'result FinalizedNativeXmrEffectFactsV3> {
    let expected_run_id =
        RunId::new(expected_run_id.to_owned()).context("invalid expected run ID")?;
    ensure!(
        result.context.run_id == expected_run_id,
        "finalized XMR effect belongs to another run"
    );
    ensure!(
        result.context.sidecar_role == expected_sidecar_role,
        "finalized XMR effect came from the wrong role sidecar"
    );
    ensure!(
        &result.terms == expected_terms,
        "finalized XMR effect differs from Stage B"
    );
    ensure!(
        result.effect == expected_effect,
        "finalized XMR effect has the wrong instruction"
    );
    ensure!(
        matches!(
            result.target,
            FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {}
        ),
        "finalized XMR effect is not a role-local discovery result"
    );
    let FinalizedNativeXmrScanOutcomeV3::Found { facts, .. } = &result.outcome else {
        return Err(anyhow!("finalized XMR effect is not affirmative Found"));
    };
    Ok(facts)
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

#[cfg(feature = "sessions")]
#[derive(Serialize)]
struct XmrActorProvisionCliSummaryV1<'a> {
    schema_version: u16,
    was_replay: bool,
    role: ActorRole,
    swap_id: String,
    agreement_commitment: String,
    activation_commitment: String,
    config_path: &'a Path,
    config_sha256: String,
    state_database_path: &'a Path,
    stage_a_path: &'a Path,
    stage_a_sha256: String,
    stage_b_path: &'a Path,
    stage_b_sha256: String,
    private_material_disclosed: bool,
}

#[cfg(feature = "sessions")]
#[allow(clippy::too_many_arguments)]
fn provision_application_actor(
    role: ActorRole,
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    agreement_stage_a: &Path,
    activation_stage_b: &Path,
    role_journal: &Path,
    output_root: &Path,
) -> Result<()> {
    let provision = match role {
        ActorRole::Maker => provision_xmr_maker_actor_from_material(
            private_root,
            own_public_packet,
            peer_public_packet,
            agreement_stage_a,
            activation_stage_b,
            role_journal,
            output_root,
        )?,
        ActorRole::Taker => provision_xmr_taker_actor_from_material(
            private_root,
            own_public_packet,
            peer_public_packet,
            agreement_stage_a,
            activation_stage_b,
            role_journal,
            output_root,
        )?,
    };
    let summary = XmrActorProvisionCliSummaryV1 {
        schema_version: 1,
        was_replay: provision.was_replay(),
        role: provision.role(),
        swap_id: hex::encode(provision.swap_id()),
        agreement_commitment: hex::encode(provision.agreement_commitment()),
        activation_commitment: hex::encode(provision.activation_commitment()),
        config_path: provision.manifest_file(),
        config_sha256: hex::encode(provision.manifest_sha256()),
        state_database_path: provision.state_database(),
        stage_a_path: provision.stage_a_file(),
        stage_a_sha256: hex::encode(provision.stage_a_sha256()),
        stage_b_path: provision.stage_b_file(),
        stage_b_sha256: hex::encode(provision.stage_b_sha256()),
        private_material_disclosed: false,
    };
    let bytes = canonical_json_bytes(&summary, "encode XMR actor provision summary")?;
    std::io::stdout()
        .write_all(&bytes)
        .context("write XMR actor provision summary")
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
    #[cfg(feature = "sessions")]
    share: CrossCurveScalar,
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
    #[cfg(not(feature = "sessions"))]
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
        #[cfg(feature = "sessions")]
        share,
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

#[cfg(all(test, feature = "sessions"))]
mod finalized_effect_gate_tests {
    use super::*;
    use lez_bridge_protocol::{
        AccountIds, AggregateBip340Signature, ChainClock, ChainPosition, DiscoveryWindow,
        ExactTransactionBytes, FinalizedBlockIdentity, FinalizedNativeXmrUnavailableReasonV3,
        Hex32, MessageContext, NativeCustodyFacts, ObservedTransactionFacts, PreparedTransaction,
        RequestId, TransactionId, XmrNativeEscrowMetadataFactsV3, XmrNativeEscrowStateV3,
        XmrNativeEscrowTermsV3Input, XmrNativeInstructionFactsV3,
    };

    fn h(byte: u8) -> Hex32 {
        Hex32::from_bytes([byte; 32])
    }

    fn terms() -> XmrNativeEscrowTermsV3 {
        XmrNativeEscrowTermsV3::new(XmrNativeEscrowTermsV3Input {
            swap_id: h(1),
            activation_commitment: h(2),
            escrow_program_id: h(3),
            authenticated_transfer_program_id: h(4),
            metadata_account_id: h(5),
            custody_account_id: h(6),
            depositor: BridgeParticipant::Taker,
            depositor_account_id: h(7),
            claimant: BridgeParticipant::Maker,
            claimant_account_id: h(8),
            claim_aggregate_x_only_public_key: h(9),
            claim_authority_account_id: h(10),
            refund_aggregate_x_only_public_key: h(11),
            refund_authority_account_id: h(12),
            maker_dleq_transcript_commitment: h(13),
            taker_dleq_transcript_commitment: h(14),
            claim_partial_context_binding: h(15),
            claim_partial_commitment: h(16),
            amount: 42,
            refund_at_ms: 10_000,
            punish_at_ms: 20_000,
            claim_message_hash: h(17),
            refund_message_hash: h(18),
            punish_message_hash: h(19),
        })
        .expect("valid XMR terms")
    }

    fn result(
        sidecar_role: BridgeParticipant,
        target: FinalizedNativeXmrTransactionTargetV3,
    ) -> ClassifyFinalizedNativeXmrEffectV3Result {
        ClassifyFinalizedNativeXmrEffectV3Result::new(
            MessageContext::new(
                RunId::new("actor-finalized-gate").expect("run ID"),
                RequestId::new("actor-finalized-gate-request").expect("request ID"),
                sidecar_role,
            ),
            terms(),
            XmrNativeEffectV3::AuthorizeClaim,
            target,
            FinalizedNativeXmrScanOutcomeV3::unavailable(
                FinalizedNativeXmrUnavailableReasonV3::HistoryUnavailable,
            ),
        )
        .expect("well-formed unavailable result")
    }

    #[test]
    fn finalized_actor_bridge_requires_role_local_discovery() {
        for (expected_role, wrong_role) in [
            (BridgeParticipant::Maker, BridgeParticipant::Taker),
            (BridgeParticipant::Taker, BridgeParticipant::Maker),
        ] {
            let wrong_sidecar = result(
                wrong_role,
                FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {},
            );
            let error = discovered_finalized_xmr_facts(
                &wrong_sidecar,
                "actor-finalized-gate",
                &terms(),
                XmrNativeEffectV3::AuthorizeClaim,
                expected_role,
            )
            .expect_err("cross-role sidecar evidence must be rejected");
            assert!(format!("{error:#}").contains("wrong role sidecar"));
        }

        let owner_exact = result(
            BridgeParticipant::Maker,
            FinalizedNativeXmrTransactionTargetV3::exact(PreparedTransaction::new(
                TransactionId::from_bytes([0x51; 32]),
                ExactTransactionBytes::new(vec![0x52]).expect("exact transaction bytes"),
            )),
        );
        let error = discovered_finalized_xmr_facts(
            &owner_exact,
            "actor-finalized-gate",
            &terms(),
            XmrNativeEffectV3::AuthorizeClaim,
            BridgeParticipant::Maker,
        )
        .expect_err("owner-side exact evidence must not replace local discovery");
        assert!(format!("{error:#}").contains("role-local discovery"));

        let local_discovery = result(
            BridgeParticipant::Maker,
            FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {},
        );
        let error = discovered_finalized_xmr_facts(
            &local_discovery,
            "actor-finalized-gate",
            &terms(),
            XmrNativeEffectV3::AuthorizeClaim,
            BridgeParticipant::Maker,
        )
        .expect_err("non-affirmative discovery must remain pending");
        assert!(format!("{error:#}").contains("not affirmative Found"));
    }

    #[test]
    fn maker_accepts_only_canonical_discovered_finalized_refund_signature() {
        let terms = terms();
        let signature = AggregateBip340Signature::from_bytes([0x65; 64]);
        let facts = FinalizedNativeXmrEffectFactsV3::new(
            ObservedTransactionFacts::new(
                TransactionId::from_bytes([0x66; 32]),
                ExactTransactionBytes::new(vec![0x10, 0x65])
                    .expect("exact refund transaction bytes"),
                ChainPosition::new(h(70), 100, 2),
                AccountIds::new(vec![h(12)]).expect("refund signer"),
                true,
            ),
            XmrNativeInstructionFactsV3::new(
                XmrNativeEffectV3::Refund,
                h(3),
                AccountIds::new(vec![h(5), h(6), h(7), h(12)]).expect("refund account order"),
                h(1),
                h(18),
                None,
            )
            .expect("canonical refund instruction"),
            Some(signature),
            FinalizedBlockIdentity::new(100, h(70), 15_000),
            XmrNativeEscrowMetadataFactsV3::from_terms(terms, XmrNativeEscrowStateV3::Refunded),
            NativeCustodyFacts::new(h(6), h(4), 0),
        );
        let result = ClassifyFinalizedNativeXmrEffectV3Result::new(
            MessageContext::new(
                RunId::new("actor-finalized-refund").expect("run ID"),
                RequestId::new("actor-finalized-refund-request").expect("request ID"),
                BridgeParticipant::Maker,
            ),
            terms,
            XmrNativeEffectV3::Refund,
            FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {},
            FinalizedNativeXmrScanOutcomeV3::found(
                ChainClock::new(h(71), 110, 30_000),
                DiscoveryWindow::new(90, 21).expect("discovery window"),
                facts,
            ),
        )
        .expect("canonical finalized refund result");

        let accepted = discovered_finalized_xmr_facts(
            &result,
            "actor-finalized-refund",
            &terms,
            XmrNativeEffectV3::Refund,
            BridgeParticipant::Maker,
        )
        .expect("Maker accepts role-local finalized refund discovery");
        assert_eq!(accepted.aggregate_signature, Some(signature));

        for (effect, role) in [
            (XmrNativeEffectV3::Claim, BridgeParticipant::Maker),
            (XmrNativeEffectV3::Refund, BridgeParticipant::Taker),
        ] {
            assert!(
                discovered_finalized_xmr_facts(
                    &result,
                    "actor-finalized-refund",
                    &terms,
                    effect,
                    role,
                )
                .is_err(),
                "crossed effect or role must fail closed"
            );
        }
    }
}

#[cfg(feature = "sessions")]
struct ExpectedMoneroSweep<'a> {
    run_id: &'a str,
    agreement_commitment: &'a str,
    shared_address: &'a str,
    reconstructed_public_spend_key: [u8; 32],
    monero_genesis_hash: [u8; 32],
    funded_amount_piconero: u64,
    required_confirmations: u64,
}

#[cfg(feature = "sessions")]
// This process boundary deliberately keeps the ordered cross-chain verification steps linear.
// The evidence assembly itself is split out below; reordering the remaining checks would make the
// secret-to-finality dependency harder to audit.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn bind_finalized_claim_sweep(
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    agreement_stage_a: &Path,
    activation_stage_b: &Path,
    journal: &Path,
    run_id: &str,
    claim_run_id: &str,
    finalized_claim: &Path,
    observed_final_signature: &Path,
    extracted_maker_adaptor_scalar: &Path,
    monero_sweep_evidence: &Path,
    monero_receipt_evidence: &Path,
    output_binding_evidence: &Path,
) -> Result<()> {
    let destination =
        SecureDestination::new(output_binding_evidence, "claim sweep binding evidence")?;
    destination.ensure_absent("claim sweep binding evidence")?;
    let lifecycle = load_claim_lifecycle(
        ActorRole::Taker,
        private_root,
        own_public_packet,
        peer_public_packet,
        agreement_stage_a,
        activation_stage_b,
    )?;
    ensure!(
        lifecycle.agreement.body().monero().network() == MoneroAddressNetworkV1::Regtest,
        "Stage A is not bound to Monero Regtest"
    );

    let result = read_finalized_xmr_effect(finalized_claim)?;
    let facts = discovered_finalized_xmr_facts(
        &result,
        claim_run_id,
        &lifecycle.binding.terms(),
        XmrNativeEffectV3::Claim,
        BridgeParticipant::Taker,
    )?;
    let aggregate_signature = facts
        .aggregate_signature
        .ok_or_else(|| anyhow!("finalized tag-15 facts omit the aggregate signature"))?;
    let observed_bytes_before = read_private_input(
        observed_final_signature,
        M4_MONERO_EVIDENCE_MAX_BYTES,
        "observed final-signature packet",
    )?;
    let observed_signature =
        read_final_signature_packet(observed_final_signature, &lifecycle.session)
            .context("read observed Taker claim signature packet")?;
    let observed_bytes_after = read_private_input(
        observed_final_signature,
        M4_MONERO_EVIDENCE_MAX_BYTES,
        "observed final-signature packet",
    )?;
    ensure!(
        observed_bytes_before == observed_bytes_after,
        "observed final-signature packet changed while it was verified"
    );
    ensure!(
        aggregate_signature.as_bytes() == &observed_signature,
        "classifier aggregate signature differs from the observed packet"
    );

    let verified_secret = verify_extracted_adaptor_secret(
        journal,
        &lifecycle.session,
        RunnerRole::Taker,
        observed_signature,
        extracted_maker_adaptor_scalar,
    )
    .context("verify extracted Maker adaptor secret against the durable Taker claim session")?;
    let reconstructed = ReconstructedMoneroSpendKey::reconstruct(
        lifecycle.agreement.shared_address(),
        lifecycle.agreement.maker_proof(),
        lifecycle.material.share,
        verified_secret.into_big_endian_bytes(),
    )
    .context("reconstruct exact Stage-A Monero spend authority")?;
    let reconstructed_public_spend_key = reconstructed.public_key();

    let (sweep, sweep_bytes) = read_canonical_private_json::<MoneroSweepEvidence>(
        monero_sweep_evidence,
        "Monero sweep evidence",
    )?;
    let (receipt, receipt_bytes) = read_canonical_private_json::<MoneroReceiptEvidenceV2>(
        monero_receipt_evidence,
        "Monero receipt evidence v2",
    )?;
    let agreement_commitment = hex::encode(lifecycle.agreement.agreement_commitment());
    let shared_address = lifecycle.agreement.shared_address().address_string();
    let expected = ExpectedMoneroSweep {
        run_id,
        agreement_commitment: &agreement_commitment,
        shared_address: &shared_address,
        reconstructed_public_spend_key,
        monero_genesis_hash: lifecycle.agreement.body().monero().genesis_hash(),
        funded_amount_piconero: lifecycle.agreement.body().monero().amount_piconero(),
        required_confirmations: u64::from(
            lifecycle.agreement.body().monero().required_confirmations(),
        ),
    };
    let accounting = validate_monero_evidence_pair(&sweep, &receipt, &expected)?;

    write_claim_sweep_binding_evidence(
        &destination,
        run_id,
        &lifecycle.agreement,
        &lifecycle.activation,
        &result,
        facts,
        aggregate_signature.as_bytes(),
        &observed_bytes_after,
        reconstructed_public_spend_key,
        &sweep_bytes,
        &receipt_bytes,
        &receipt,
        &accounting,
    )
}

#[cfg(feature = "sessions")]
#[allow(clippy::too_many_arguments)]
fn write_claim_sweep_binding_evidence(
    destination: &SecureDestination,
    run_id: &str,
    agreement: &XmrAgreementV1,
    activation: &XmrActivatedAgreementV1,
    result: &ClassifyFinalizedNativeXmrEffectV3Result,
    facts: &FinalizedNativeXmrEffectFactsV3,
    aggregate_signature: &[u8; 64],
    observed_packet: &[u8],
    reconstructed_public_spend_key: [u8; 32],
    sweep_bytes: &[u8],
    receipt_bytes: &[u8],
    receipt: &MoneroReceiptEvidenceV2,
    accounting: &ValidatedMoneroAccounting,
) -> Result<()> {
    let (finalized_clock, scanned_window) = match &result.outcome {
        FinalizedNativeXmrScanOutcomeV3::Found {
            finalized_clock,
            scanned_window,
            ..
        } => (*finalized_clock, *scanned_window),
        _ => return Err(anyhow!("finalized XMR effect is not affirmative Found")),
    };
    let classifier_bytes = canonical_json_bytes(result, "encode finalized XMR effect")?;
    let evidence = ClaimSweepBindingEvidenceV1 {
        schema: M4_CLAIM_SWEEP_BINDING_SCHEMA,
        run_id: run_id.to_owned(),
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        activation_commitment: hex::encode(activation.activation_commitment()),
        claim_context_binding: hex::encode(agreement.claim_context_binding()),
        atomicity_scope: "successful_claim_path_conditional_atomicity",
        distributed_cross_chain_transaction_claimed: false,
        future_reorg_immunity_claimed: false,
        lez_effect: "claim",
        lez_sidecar_role: "taker",
        classifier_target: "discover_by_terms",
        classifier_outcome: "found",
        classifier_request_id: result.context.request_id.as_str().to_owned(),
        classifier_result_sha256: sha256_hex(&classifier_bytes),
        classifier_scan_start_height: scanned_window.start_height(),
        classifier_scan_max_blocks: scanned_window.max_blocks(),
        lez_claim_transaction_id: hex::encode(facts.transaction.transaction_id.as_bytes()),
        lez_claim_block_hash: hex::encode(facts.containing_block.block_hash.as_bytes()),
        lez_claim_block_height: facts.containing_block.block_id,
        lez_claim_transaction_index: facts.transaction.position.transaction_index,
        lez_claim_block_timestamp_ms: facts.containing_block.timestamp_ms,
        lez_finalized_tip_hash: hex::encode(finalized_clock.block_hash.as_bytes()),
        lez_finalized_tip_height: finalized_clock.height,
        lez_finalized_tip_timestamp_ms: finalized_clock.timestamp_ms,
        aggregate_signature_sha256: sha256_hex(aggregate_signature),
        observed_final_signature_packet_sha256: sha256_hex(observed_packet),
        extraction_binding: "durable_taker_claim_presignature_v1",
        reconstructed_public_spend_key: hex::encode(reconstructed_public_spend_key),
        monero_sweep_evidence_provenance: accounting.provenance,
        monero_sweep_evidence_schema: accounting.sweep_schema,
        monero_sweep_evidence_sha256: sha256_hex(sweep_bytes),
        monero_receipt_evidence_schema: M4_MONERO_RECEIPT_SCHEMA,
        monero_receipt_evidence_sha256: sha256_hex(receipt_bytes),
        monero_genesis_hash: receipt.monero_genesis_hash.clone(),
        monero_sweep_transaction_id: receipt.transaction_id.clone(),
        monero_evidenced_destination_address: receipt.destination_address.clone(),
        destination_ownership_binding: "owner_private_taker_wallet_boundary_not_stage_a_committed",
        monero_daemon_version: receipt.daemon_version.clone(),
        monero_target_wallet_version: receipt.target_wallet_version,
        monero_foreign_wallet_version: receipt.foreign_wallet_version,
        monero_sweep_block_hash: receipt.containing_block_hash.clone(),
        monero_sweep_block_height: receipt.containing_block_height,
        monero_sweep_confirmations: receipt.confirmations,
        monero_stable_tip_hash: receipt.stable_tip_hash.clone(),
        monero_stable_tip_height: receipt.stable_tip_height,
        funded_amount_piconero: accounting.funded_amount_piconero,
        received_amount_piconero: accounting.received_amount_piconero,
        fee_piconero: accounting.fee_piconero,
        unreceived_remainder_piconero: accounting.unreceived_remainder_piconero,
        peer_count: receipt.peer_count,
        network_scope: M4_MONERO_NETWORK_SCOPE,
        public_rpc_used: false,
        faucet_used: false,
        automatic_submission_retry: false,
    };
    let bytes = canonical_json_bytes(&evidence, "encode claim sweep binding evidence")?;
    write_bounded_public_new(
        destination,
        &bytes,
        M4_BINDING_EVIDENCE_MAX_BYTES,
        "claim sweep binding evidence",
    )
}

#[cfg(feature = "sessions")]
// Keep the extraction-to-finality-to-receipt chain linear and auditable. No input in this
// sequence grants authority until all role, session, signature, and accounting checks pass.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn bind_finalized_refund_sweep(
    private_root: &Path,
    own_public_packet: &Path,
    peer_public_packet: &Path,
    agreement_stage_a: &Path,
    activation_stage_b: &Path,
    journal: &Path,
    run_id: &str,
    refund_run_id: &str,
    finalized_refund: &Path,
    observed_final_signature: &Path,
    extracted_taker_adaptor_scalar: &Path,
    monero_sweep_evidence: &Path,
    monero_receipt_evidence: &Path,
    output_binding_evidence: &Path,
) -> Result<()> {
    let destination =
        SecureDestination::new(output_binding_evidence, "refund sweep binding evidence")?;
    destination.ensure_absent("refund sweep binding evidence")?;
    let lifecycle = load_refund_lifecycle(
        ActorRole::Maker,
        private_root,
        own_public_packet,
        peer_public_packet,
        agreement_stage_a,
        activation_stage_b,
    )?;
    ensure!(
        lifecycle.agreement.body().monero().network() == MoneroAddressNetworkV1::Regtest,
        "Stage A is not bound to Monero Regtest"
    );

    let result = read_finalized_xmr_effect(finalized_refund)?;
    let facts = discovered_finalized_xmr_facts(
        &result,
        refund_run_id,
        &lifecycle.binding.terms(),
        XmrNativeEffectV3::Refund,
        BridgeParticipant::Maker,
    )?;
    let aggregate_signature = facts
        .aggregate_signature
        .ok_or_else(|| anyhow!("finalized tag-16 facts omit the aggregate signature"))?;
    let observed_bytes_before = read_private_input(
        observed_final_signature,
        M4_MONERO_EVIDENCE_MAX_BYTES,
        "observed refund final-signature packet",
    )?;
    let observed_signature =
        read_final_signature_packet(observed_final_signature, &lifecycle.session)
            .context("read observed Maker refund signature packet")?;
    let observed_bytes_after = read_private_input(
        observed_final_signature,
        M4_MONERO_EVIDENCE_MAX_BYTES,
        "observed refund final-signature packet",
    )?;
    ensure!(
        observed_bytes_before == observed_bytes_after,
        "observed refund final-signature packet changed while it was verified"
    );
    ensure!(
        aggregate_signature.as_bytes() == &observed_signature,
        "refund classifier aggregate signature differs from the observed packet"
    );

    let verified_secret = verify_extracted_adaptor_secret(
        journal,
        &lifecycle.session,
        RunnerRole::Maker,
        observed_signature,
        extracted_taker_adaptor_scalar,
    )
    .context("verify extracted Taker adaptor secret against the durable Maker refund session")?;
    let reconstructed = ReconstructedMoneroSpendKey::reconstruct(
        lifecycle.agreement.shared_address(),
        lifecycle.agreement.taker_proof(),
        lifecycle.material.share,
        verified_secret.into_big_endian_bytes(),
    )
    .context("reconstruct exact Stage-A Monero refund spend authority")?;
    let reconstructed_public_spend_key = reconstructed.public_key();

    let (sweep, sweep_bytes) = read_canonical_private_json::<MoneroSweepEvidence>(
        monero_sweep_evidence,
        "Monero refund sweep evidence v3",
    )?;
    let (receipt, receipt_bytes) = read_canonical_private_json::<MoneroReceiptEvidenceV2>(
        monero_receipt_evidence,
        "Monero refund receipt evidence v2",
    )?;
    let agreement_commitment = hex::encode(lifecycle.agreement.agreement_commitment());
    let shared_address = lifecycle.agreement.shared_address().address_string();
    let expected = ExpectedMoneroSweep {
        run_id,
        agreement_commitment: &agreement_commitment,
        shared_address: &shared_address,
        reconstructed_public_spend_key,
        monero_genesis_hash: lifecycle.agreement.body().monero().genesis_hash(),
        funded_amount_piconero: lifecycle.agreement.body().monero().amount_piconero(),
        required_confirmations: u64::from(
            lifecycle.agreement.body().monero().required_confirmations(),
        ),
    };
    let accounting = validate_monero_refund_evidence_pair(&sweep, &receipt, &expected)?;

    write_refund_sweep_binding_evidence(
        &destination,
        run_id,
        &lifecycle.agreement,
        &lifecycle.activation,
        &result,
        facts,
        aggregate_signature.as_bytes(),
        &observed_bytes_after,
        reconstructed_public_spend_key,
        &sweep_bytes,
        &receipt_bytes,
        &receipt,
        &accounting,
    )
}

#[cfg(feature = "sessions")]
#[allow(clippy::too_many_arguments)]
fn write_refund_sweep_binding_evidence(
    destination: &SecureDestination,
    run_id: &str,
    agreement: &XmrAgreementV1,
    activation: &XmrActivatedAgreementV1,
    result: &ClassifyFinalizedNativeXmrEffectV3Result,
    facts: &FinalizedNativeXmrEffectFactsV3,
    aggregate_signature: &[u8; 64],
    observed_packet: &[u8],
    reconstructed_public_spend_key: [u8; 32],
    sweep_bytes: &[u8],
    receipt_bytes: &[u8],
    receipt: &MoneroReceiptEvidenceV2,
    accounting: &ValidatedMoneroAccounting,
) -> Result<()> {
    let (finalized_clock, scanned_window) = match &result.outcome {
        FinalizedNativeXmrScanOutcomeV3::Found {
            finalized_clock,
            scanned_window,
            ..
        } => (*finalized_clock, *scanned_window),
        _ => return Err(anyhow!("finalized XMR refund is not affirmative Found")),
    };
    let fee = accounting
        .fee_piconero
        .ok_or_else(|| anyhow!("refund sweep evidence does not prove an exact fee"))?;
    ensure!(
        accounting.unreceived_remainder_piconero == fee,
        "refund sweep remainder differs from its exact fee"
    );
    let classifier_bytes = canonical_json_bytes(result, "encode finalized XMR refund")?;
    let evidence = RefundSweepBindingEvidenceV1 {
        schema: M5_REFUND_SWEEP_BINDING_SCHEMA,
        run_id: run_id.to_owned(),
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        activation_commitment: hex::encode(activation.activation_commitment()),
        refund_context_binding: hex::encode(agreement.refund_context_binding()),
        atomicity_scope: "successful_refund_path_conditional_atomicity",
        distributed_cross_chain_transaction_claimed: false,
        future_reorg_immunity_claimed: false,
        lez_effect: "refund",
        lez_sidecar_role: "maker",
        classifier_target: "discover_by_terms",
        classifier_outcome: "found",
        classifier_request_id: result.context.request_id.as_str().to_owned(),
        classifier_result_sha256: sha256_hex(&classifier_bytes),
        classifier_scan_start_height: scanned_window.start_height(),
        classifier_scan_max_blocks: scanned_window.max_blocks(),
        lez_refund_transaction_id: hex::encode(facts.transaction.transaction_id.as_bytes()),
        lez_refund_block_hash: hex::encode(facts.containing_block.block_hash.as_bytes()),
        lez_refund_block_height: facts.containing_block.block_id,
        lez_refund_transaction_index: facts.transaction.position.transaction_index,
        lez_refund_block_timestamp_ms: facts.containing_block.timestamp_ms,
        lez_finalized_tip_hash: hex::encode(finalized_clock.block_hash.as_bytes()),
        lez_finalized_tip_height: finalized_clock.height,
        lez_finalized_tip_timestamp_ms: finalized_clock.timestamp_ms,
        aggregate_signature_sha256: sha256_hex(aggregate_signature),
        observed_final_signature_packet_sha256: sha256_hex(observed_packet),
        extraction_binding: "durable_maker_refund_presignature_v1",
        reconstructed_public_spend_key: hex::encode(reconstructed_public_spend_key),
        monero_sweep_evidence_provenance: accounting.provenance,
        monero_sweep_evidence_schema: accounting.sweep_schema,
        monero_sweep_evidence_sha256: sha256_hex(sweep_bytes),
        monero_receipt_evidence_schema: M4_MONERO_RECEIPT_SCHEMA,
        monero_receipt_evidence_sha256: sha256_hex(receipt_bytes),
        monero_genesis_hash: receipt.monero_genesis_hash.clone(),
        monero_sweep_transaction_id: receipt.transaction_id.clone(),
        monero_evidenced_destination_address: receipt.destination_address.clone(),
        destination_ownership_binding: "owner_private_maker_wallet_boundary_not_stage_a_committed",
        monero_daemon_version: receipt.daemon_version.clone(),
        monero_target_wallet_version: receipt.target_wallet_version,
        monero_foreign_wallet_version: receipt.foreign_wallet_version,
        monero_sweep_block_hash: receipt.containing_block_hash.clone(),
        monero_sweep_block_height: receipt.containing_block_height,
        monero_sweep_confirmations: receipt.confirmations,
        monero_stable_tip_hash: receipt.stable_tip_hash.clone(),
        monero_stable_tip_height: receipt.stable_tip_height,
        funded_amount_piconero: accounting.funded_amount_piconero,
        received_amount_piconero: accounting.received_amount_piconero,
        fee_piconero: fee,
        peer_count: receipt.peer_count,
        network_scope: M4_MONERO_NETWORK_SCOPE,
        public_rpc_used: false,
        faucet_used: false,
        automatic_submission_retry: false,
    };
    write_refund_binding_output(destination, &evidence)
}

#[cfg(feature = "sessions")]
fn write_refund_binding_output(
    destination: &SecureDestination,
    evidence: &RefundSweepBindingEvidenceV1,
) -> Result<()> {
    ensure!(
        evidence.schema == M5_REFUND_SWEEP_BINDING_SCHEMA
            && evidence.atomicity_scope == "successful_refund_path_conditional_atomicity"
            && !evidence.distributed_cross_chain_transaction_claimed
            && !evidence.future_reorg_immunity_claimed,
        "refund binding overstates its conditional atomicity scope"
    );
    ensure!(
        evidence.lez_effect == "refund"
            && evidence.lez_sidecar_role == "maker"
            && evidence.classifier_target == "discover_by_terms"
            && evidence.classifier_outcome == "found",
        "refund binding has crossed LEZ evidence"
    );
    ensure!(
        evidence.extraction_binding == "durable_maker_refund_presignature_v1"
            && evidence.monero_sweep_evidence_provenance == "refund_v3"
            && evidence.monero_sweep_evidence_schema == M5_MONERO_REFUND_SWEEP_SCHEMA
            && evidence.monero_receipt_evidence_schema == M4_MONERO_RECEIPT_SCHEMA,
        "refund binding has crossed extraction or Monero evidence"
    );
    ensure!(
        evidence.destination_ownership_binding
            == "owner_private_maker_wallet_boundary_not_stage_a_committed"
            && evidence.network_scope == M4_MONERO_NETWORK_SCOPE
            && evidence.peer_count == 0
            && !evidence.public_rpc_used
            && !evidence.faucet_used
            && !evidence.automatic_submission_retry,
        "refund binding used an invalid destination or external resource"
    );
    let accounted = evidence
        .received_amount_piconero
        .checked_add(evidence.fee_piconero)
        .ok_or_else(|| anyhow!("refund binding accounting overflows"))?;
    ensure!(
        evidence.received_amount_piconero > 0
            && evidence.fee_piconero > 0
            && evidence.funded_amount_piconero == accounted,
        "refund binding accounting is not exact"
    );
    let bytes = canonical_json_bytes(&evidence, "encode refund sweep binding evidence")?;
    write_bounded_public_new(
        destination,
        &bytes,
        M4_BINDING_EVIDENCE_MAX_BYTES,
        "refund sweep binding evidence",
    )
}

#[cfg(feature = "sessions")]
// Keep the legacy compatibility validator as one exhaustive field checklist. Its result is
// explicitly only an unreceived remainder, never an exact fee claim.
#[allow(clippy::too_many_lines)]
fn validate_legacy_monero_evidence_pair(
    sweep: &MoneroSweepEvidenceV1,
    receipt: &MoneroReceiptEvidenceV2,
    expected: &ExpectedMoneroSweep<'_>,
    expected_revealed_role: &str,
    expected_sweeping_role: &str,
) -> Result<u64> {
    ensure!(
        sweep.schema == M4_MONERO_LEGACY_SWEEP_SCHEMA,
        "unsupported Monero sweep evidence schema"
    );
    ensure!(
        receipt.schema == M4_MONERO_RECEIPT_SCHEMA,
        "unsupported Monero receipt evidence schema"
    );
    ensure!(
        receipt.run_id == expected.run_id,
        "Monero receipt belongs to another run"
    );
    ensure!(
        sweep.agreement_commitment == expected.agreement_commitment
            && receipt.agreement_commitment == expected.agreement_commitment,
        "Monero evidence differs from Stage A"
    );
    ensure!(
        sweep.shared_address == expected.shared_address,
        "Monero sweep shared address differs from Stage A"
    );
    ensure!(
        decode_nonzero_hex32(
            &sweep.reconstructed_public_spend_key,
            "reconstructed public spend key"
        )? == expected.reconstructed_public_spend_key,
        "Monero sweep reconstructed public spend key differs from verified extraction"
    );
    ensure!(
        decode_nonzero_hex32(&receipt.monero_genesis_hash, "Monero genesis hash")?
            == expected.monero_genesis_hash,
        "Monero receipt genesis differs from Stage A"
    );
    ensure!(
        sweep.transaction_id == receipt.transaction_id,
        "Monero evidence transaction IDs differ"
    );
    let _ = decode_nonzero_hex32(&sweep.transaction_id, "Monero transaction ID")?;
    let containing_block = decode_nonzero_hex32(
        &receipt.containing_block_hash,
        "Monero containing block hash",
    )?;
    let stable_tip = decode_nonzero_hex32(&receipt.stable_tip_hash, "Monero stable tip hash")?;
    ensure!(
        sweep.destination_address == receipt.destination_address,
        "Monero evidence destinations differ"
    );
    let shared = parse_canonical_monero_address(&sweep.shared_address, "Monero shared address")?;
    let destination =
        parse_canonical_monero_address(&sweep.destination_address, "Monero sweep destination")?;
    ensure!(
        shared.network == destination.network,
        "Monero addresses use different networks"
    );
    ensure!(
        shared != destination,
        "Monero sweep destination is the funded shared address"
    );
    ensure!(
        sweep.funded_amount_piconero == expected.funded_amount_piconero
            && sweep.funded_amount_piconero > 0,
        "Monero funded amount differs from Stage A"
    );
    let remainder = sweep
        .funded_amount_piconero
        .checked_sub(receipt.amount_piconero)
        .filter(|remainder| *remainder > 0 && receipt.amount_piconero > 0)
        .ok_or_else(|| anyhow!("Monero received amount does not leave a positive remainder"))?;
    ensure!(
        expected.required_confirmations == M4_MONERO_CONFIRMATIONS
            && sweep.required_confirmations == expected.required_confirmations,
        "Monero sweep confirmation policy differs from Stage A"
    );
    ensure!(
        receipt.confirmations >= expected.required_confirmations,
        "Monero sweep has insufficient confirmations"
    );
    ensure!(
        receipt.stable_tip_height >= receipt.containing_block_height,
        "Monero stable tip precedes the sweep block"
    );
    let exact_confirmations = receipt
        .stable_tip_height
        .checked_sub(receipt.containing_block_height)
        .and_then(|distance| distance.checked_add(1))
        .ok_or_else(|| anyhow!("Monero confirmation heights overflow"))?;
    ensure!(
        receipt.confirmations == exact_confirmations,
        "Monero receipt confirmation count differs from its chain positions"
    );
    ensure!(
        sweep.confirmation_tip_height == receipt.stable_tip_height,
        "Monero sweep and receipt tip heights differ"
    );
    if receipt.stable_tip_height == receipt.containing_block_height {
        ensure!(
            stable_tip == containing_block,
            "same-height Monero block hashes differ"
        );
    }
    ensure!(
        sweep.restore_height <= receipt.containing_block_height,
        "Monero restore height is after the sweep"
    );
    ensure!(
        sweep.revealed_role == expected_revealed_role
            && sweep.sweeping_role == expected_sweeping_role,
        "Monero sweep roles are invalid"
    );
    ensure!(
        sweep.network_scope == M4_MONERO_NETWORK_SCOPE
            && receipt.network_scope == M4_MONERO_NETWORK_SCOPE
            && sweep.network_scope == receipt.network_scope,
        "Monero evidence network scopes differ"
    );
    ensure!(
        !sweep.public_rpc_used
            && !receipt.public_rpc_used
            && !sweep.faucet_used
            && !receipt.faucet_used
            && !sweep.automatic_submission_retry,
        "Monero evidence used a prohibited public resource or automatic retry"
    );
    ensure!(
        receipt.peer_count == 0,
        "Monero verifier was not peer-isolated"
    );
    ensure!(
        receipt.target_wallet_version == M4_MONERO_WALLET_VERSION
            && receipt.foreign_wallet_version == M4_MONERO_WALLET_VERSION,
        "Monero wallet versions differ from the pinned M4 profile"
    );
    ensure!(
        receipt.daemon_version == M4_MONERO_DAEMON_VERSION,
        "Monero daemon version differs from the pinned M4 profile"
    );
    Ok(remainder)
}

#[cfg(feature = "sessions")]
fn read_private_input(path: &Path, max_bytes: u64, label: &'static str) -> Result<Vec<u8>> {
    let file = open_path_no_symlinks(path, label)?;
    read_bounded_file(file, max_bytes, FilePolicy::Private, label)
}

#[cfg(feature = "sessions")]
fn read_canonical_private_json<T>(path: &Path, label: &'static str) -> Result<(T, Vec<u8>)>
where
    T: DeserializeOwned + Serialize,
{
    let bytes = read_private_input(path, M4_MONERO_EVIDENCE_MAX_BYTES, label)?;
    let value = serde_json::from_slice(&bytes).with_context(|| format!("{label} is malformed"))?;
    ensure!(
        canonical_json_bytes(&value, "encode private Monero evidence")? == bytes,
        "{label} is noncanonical"
    );
    Ok((value, bytes))
}

#[cfg(feature = "sessions")]
fn decode_nonzero_hex32(value: &str, label: &'static str) -> Result<[u8; 32]> {
    let decoded = decode_exact(value).with_context(|| format!("{label} is invalid"))?;
    ensure!(decoded != [0; 32], "{label} is zero");
    Ok(decoded)
}

#[cfg(feature = "sessions")]
fn parse_canonical_monero_address(value: &str, label: &'static str) -> Result<MoneroAddress> {
    let address = value
        .parse::<MoneroAddress>()
        .with_context(|| format!("{label} is invalid"))?;
    ensure!(address.to_string() == value, "{label} is noncanonical");
    Ok(address)
}

#[cfg(feature = "sessions")]
fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
#[cfg(feature = "sessions")]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct MoneroSweepEvidenceV2 {
    schema: String,
    run_id: String,
    agreement_commitment: String,
    monero_genesis_hash: String,
    shared_address: String,
    reconstructed_public_spend_key: String,
    destination_address: String,
    funded_amount_piconero: u64,
    received_amount_piconero: u64,
    fee_piconero: u64,
    transaction_id: String,
    containing_block_hash: String,
    containing_block_height: u64,
    confirmations: u64,
    stable_tip_hash: String,
    stable_tip_height: u64,
    generated_confirmation_tip_height: u64,
    required_confirmations: u64,
    peer_count: u64,
    restore_height: u64,
    revealed_role: String,
    sweeping_role: String,
    network_scope: String,
    public_rpc_used: bool,
    faucet_used: bool,
    automatic_submission_retry: bool,
}

#[cfg(feature = "sessions")]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct MoneroRefundSweepEvidenceV3 {
    schema: String,
    journey: String,
    run_id: String,
    agreement_commitment: String,
    monero_genesis_hash: String,
    shared_address: String,
    reconstructed_public_spend_key: String,
    destination_address: String,
    funded_amount_piconero: u64,
    received_amount_piconero: u64,
    fee_piconero: u64,
    transaction_id: String,
    containing_block_hash: String,
    containing_block_height: u64,
    confirmations: u64,
    stable_tip_hash: String,
    stable_tip_height: u64,
    generated_confirmation_tip_height: u64,
    required_confirmations: u64,
    peer_count: u64,
    restore_height: u64,
    revealed_role: String,
    sweeping_role: String,
    network_scope: String,
    public_rpc_used: bool,
    faucet_used: bool,
    automatic_submission_retry: bool,
}

#[cfg(feature = "sessions")]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
enum MoneroSweepEvidence {
    RefundV3(MoneroRefundSweepEvidenceV3),
    CurrentV2(MoneroSweepEvidenceV2),
    LegacyV1(MoneroSweepEvidenceV1),
}

#[cfg(feature = "sessions")]
struct ValidatedMoneroAccounting {
    provenance: &'static str,
    sweep_schema: &'static str,
    funded_amount_piconero: u64,
    received_amount_piconero: u64,
    fee_piconero: Option<u64>,
    unreceived_remainder_piconero: u64,
}

#[cfg(feature = "sessions")]
fn validate_monero_evidence_pair(
    sweep: &MoneroSweepEvidence,
    receipt: &MoneroReceiptEvidenceV2,
    expected: &ExpectedMoneroSweep<'_>,
) -> Result<ValidatedMoneroAccounting> {
    match sweep {
        MoneroSweepEvidence::RefundV3(_) => {
            Err(anyhow!("refund sweep evidence cannot certify a claim"))
        }
        MoneroSweepEvidence::LegacyV1(sweep) => {
            let remainder = validate_legacy_monero_evidence_pair(
                sweep,
                receipt,
                expected,
                "maker_claim_signature",
                "taker",
            )?;
            Ok(ValidatedMoneroAccounting {
                provenance: "legacy_v1_plus_receipt_v2",
                sweep_schema: M4_MONERO_LEGACY_SWEEP_SCHEMA,
                funded_amount_piconero: sweep.funded_amount_piconero,
                received_amount_piconero: receipt.amount_piconero,
                fee_piconero: None,
                unreceived_remainder_piconero: remainder,
            })
        }
        MoneroSweepEvidence::CurrentV2(sweep) => validate_exact_monero_evidence_pair(
            sweep,
            receipt,
            expected,
            M4_MONERO_CURRENT_SWEEP_SCHEMA,
            "maker_claim_signature",
            "taker",
            "current_v2",
        ),
    }
}

#[cfg(feature = "sessions")]
#[allow(clippy::too_many_arguments)]
fn validate_exact_monero_evidence_pair(
    sweep: &MoneroSweepEvidenceV2,
    receipt: &MoneroReceiptEvidenceV2,
    expected: &ExpectedMoneroSweep<'_>,
    expected_schema: &'static str,
    expected_revealed_role: &'static str,
    expected_sweeping_role: &'static str,
    provenance: &'static str,
) -> Result<ValidatedMoneroAccounting> {
    ensure!(
        sweep.schema == expected_schema,
        "unsupported exact Monero sweep evidence schema"
    );
    ensure!(
        sweep.run_id == expected.run_id && sweep.run_id == receipt.run_id,
        "exact Monero sweep run differs from its receipt"
    );
    ensure!(
        sweep.monero_genesis_hash == receipt.monero_genesis_hash,
        "exact Monero sweep genesis differs from its receipt"
    );
    ensure!(
        sweep.received_amount_piconero == receipt.amount_piconero,
        "exact Monero sweep amount differs from its receipt"
    );
    ensure!(
        sweep.containing_block_hash == receipt.containing_block_hash
            && sweep.containing_block_height == receipt.containing_block_height,
        "exact Monero sweep block differs from its receipt"
    );
    ensure!(
        sweep.confirmations == receipt.confirmations,
        "exact Monero sweep confirmations differ from its receipt"
    );
    ensure!(
        sweep.stable_tip_hash == receipt.stable_tip_hash
            && sweep.stable_tip_height == receipt.stable_tip_height,
        "exact Monero sweep stable tip differs from its receipt"
    );
    ensure!(
        sweep.peer_count == receipt.peer_count,
        "exact Monero sweep peer count differs from its receipt"
    );
    ensure!(
        sweep.network_scope == receipt.network_scope,
        "exact Monero sweep scope differs from its receipt"
    );
    ensure!(
        sweep.public_rpc_used == receipt.public_rpc_used
            && sweep.faucet_used == receipt.faucet_used,
        "exact Monero sweep resource flags differ from its receipt"
    );
    let accounted_total = sweep
        .received_amount_piconero
        .checked_add(sweep.fee_piconero)
        .ok_or_else(|| anyhow!("exact Monero sweep accounting overflows"))?;
    ensure!(
        sweep.funded_amount_piconero == accounted_total,
        "exact Monero sweep funded amount is not received plus fee"
    );

    let common = MoneroSweepEvidenceV1 {
        schema: M4_MONERO_LEGACY_SWEEP_SCHEMA.to_owned(),
        agreement_commitment: sweep.agreement_commitment.clone(),
        shared_address: sweep.shared_address.clone(),
        reconstructed_public_spend_key: sweep.reconstructed_public_spend_key.clone(),
        destination_address: sweep.destination_address.clone(),
        funded_amount_piconero: sweep.funded_amount_piconero,
        transaction_id: sweep.transaction_id.clone(),
        confirmation_tip_height: sweep.generated_confirmation_tip_height,
        required_confirmations: sweep.required_confirmations,
        restore_height: sweep.restore_height,
        revealed_role: sweep.revealed_role.clone(),
        sweeping_role: sweep.sweeping_role.clone(),
        network_scope: sweep.network_scope.clone(),
        public_rpc_used: sweep.public_rpc_used,
        faucet_used: sweep.faucet_used,
        automatic_submission_retry: sweep.automatic_submission_retry,
    };
    let remainder = validate_legacy_monero_evidence_pair(
        &common,
        receipt,
        expected,
        expected_revealed_role,
        expected_sweeping_role,
    )?;
    ensure!(
        remainder == sweep.fee_piconero,
        "exact Monero sweep fee differs from the independently derived remainder"
    );
    Ok(ValidatedMoneroAccounting {
        provenance,
        sweep_schema: expected_schema,
        funded_amount_piconero: sweep.funded_amount_piconero,
        received_amount_piconero: sweep.received_amount_piconero,
        fee_piconero: Some(sweep.fee_piconero),
        unreceived_remainder_piconero: remainder,
    })
}

#[cfg(feature = "sessions")]
fn validate_monero_refund_evidence_pair(
    sweep: &MoneroSweepEvidence,
    receipt: &MoneroReceiptEvidenceV2,
    expected: &ExpectedMoneroSweep<'_>,
) -> Result<ValidatedMoneroAccounting> {
    let MoneroSweepEvidence::RefundV3(sweep) = sweep else {
        return Err(anyhow!(
            "refund binding requires Monero refund sweep v3 evidence"
        ));
    };
    ensure!(
        sweep.schema == M5_MONERO_REFUND_SWEEP_SCHEMA,
        "unsupported Monero refund sweep evidence schema"
    );
    ensure!(
        sweep.journey == "refund",
        "Monero refund sweep evidence has the wrong journey"
    );
    ensure!(
        sweep.revealed_role == "taker_refund_signature" && sweep.sweeping_role == "maker",
        "Monero refund sweep roles are invalid"
    );

    let exact_accounting_fields = MoneroSweepEvidenceV2 {
        schema: sweep.schema.clone(),
        run_id: sweep.run_id.clone(),
        agreement_commitment: sweep.agreement_commitment.clone(),
        monero_genesis_hash: sweep.monero_genesis_hash.clone(),
        shared_address: sweep.shared_address.clone(),
        reconstructed_public_spend_key: sweep.reconstructed_public_spend_key.clone(),
        destination_address: sweep.destination_address.clone(),
        funded_amount_piconero: sweep.funded_amount_piconero,
        received_amount_piconero: sweep.received_amount_piconero,
        fee_piconero: sweep.fee_piconero,
        transaction_id: sweep.transaction_id.clone(),
        containing_block_hash: sweep.containing_block_hash.clone(),
        containing_block_height: sweep.containing_block_height,
        confirmations: sweep.confirmations,
        stable_tip_hash: sweep.stable_tip_hash.clone(),
        stable_tip_height: sweep.stable_tip_height,
        generated_confirmation_tip_height: sweep.generated_confirmation_tip_height,
        required_confirmations: sweep.required_confirmations,
        peer_count: sweep.peer_count,
        restore_height: sweep.restore_height,
        revealed_role: sweep.revealed_role.clone(),
        sweeping_role: sweep.sweeping_role.clone(),
        network_scope: sweep.network_scope.clone(),
        public_rpc_used: sweep.public_rpc_used,
        faucet_used: sweep.faucet_used,
        automatic_submission_retry: sweep.automatic_submission_retry,
    };
    validate_exact_monero_evidence_pair(
        &exact_accounting_fields,
        receipt,
        expected,
        M5_MONERO_REFUND_SWEEP_SCHEMA,
        "taker_refund_signature",
        "maker",
        "refund_v3",
    )
}

#[cfg(all(test, feature = "sessions"))]
mod claim_sweep_binding_tests {
    use super::*;

    const RUN_ID: &str = "m4happy-40cbac3-20260721a";
    const AGREEMENT: &str = "4e9250289583b54bf5c6708e7e95f2994cd0d573552a2bddbf76c45820ee8cff";
    const GENESIS: &str = "418015bb9ae982a1975da7d79277c2705727a56894ba0fb246adaabb1f4632e3";
    const PUBLIC_SPEND: &str = "9a02c56a882319b4f7bd1306d4e42c3af27c57c970ce47f90d24e410040eb955";
    const SHARED: &str = "47Tcdbnuze4XGcbMBEhygfArrz1kb2enzif7P5okHShvFH8ogApTDtaDzvrPyLaGnmSMC6GbDASV45QyHhnfGURQ9ZLKYnd";
    const DESTINATION: &str = "47xM3mbitAg7jMWeFb7E6r4xUdbJ4BUNuMQaVZ3vwNzyGGUtdKmc6J6PaaZMpv3kSHCKasHLcpnwMdyM33P4rrFbT5c9mCn";
    const TRANSACTION: &str = "6c8c7bca4ea51fbeafd22b5396efc2c631948ca893385d4cebb436d070e8e21a";
    const BLOCK: &str = "2ff2876056f94e6459f652148f044f457a0af86e875ad5449b1ece78780b7cc2";
    const TIP: &str = "38d3a49edc4e5ae669c1b0b02f6649cc105af04059e4d8a8c5fb9eea12884b48";
    const FUNDED: u64 = 1_000_000_000_000;
    const RECEIVED: u64 = 998_191_600_000;
    const REMAINDER: u64 = FUNDED - RECEIVED;

    fn expected() -> ExpectedMoneroSweep<'static> {
        ExpectedMoneroSweep {
            run_id: RUN_ID,
            agreement_commitment: AGREEMENT,
            shared_address: SHARED,
            reconstructed_public_spend_key: decode_exact(PUBLIC_SPEND).expect("public spend"),
            monero_genesis_hash: decode_exact(GENESIS).expect("genesis"),
            funded_amount_piconero: FUNDED,
            required_confirmations: M4_MONERO_CONFIRMATIONS,
        }
    }

    fn receipt() -> MoneroReceiptEvidenceV2 {
        MoneroReceiptEvidenceV2 {
            schema: M4_MONERO_RECEIPT_SCHEMA.to_owned(),
            run_id: RUN_ID.to_owned(),
            agreement_commitment: AGREEMENT.to_owned(),
            monero_genesis_hash: GENESIS.to_owned(),
            transaction_id: TRANSACTION.to_owned(),
            destination_address: DESTINATION.to_owned(),
            amount_piconero: RECEIVED,
            containing_block_hash: BLOCK.to_owned(),
            containing_block_height: 121,
            confirmations: 10,
            stable_tip_hash: TIP.to_owned(),
            stable_tip_height: 130,
            peer_count: 0,
            daemon_version: "0.18.5.1-release".to_owned(),
            target_wallet_version: 65_567,
            foreign_wallet_version: 65_567,
            network_scope: M4_MONERO_NETWORK_SCOPE.to_owned(),
            public_rpc_used: false,
            faucet_used: false,
        }
    }

    fn legacy() -> MoneroSweepEvidenceV1 {
        MoneroSweepEvidenceV1 {
            schema: M4_MONERO_LEGACY_SWEEP_SCHEMA.to_owned(),
            agreement_commitment: AGREEMENT.to_owned(),
            shared_address: SHARED.to_owned(),
            reconstructed_public_spend_key: PUBLIC_SPEND.to_owned(),
            destination_address: DESTINATION.to_owned(),
            funded_amount_piconero: FUNDED,
            transaction_id: TRANSACTION.to_owned(),
            confirmation_tip_height: 130,
            required_confirmations: M4_MONERO_CONFIRMATIONS,
            restore_height: 0,
            revealed_role: "maker_claim_signature".to_owned(),
            sweeping_role: "taker".to_owned(),
            network_scope: M4_MONERO_NETWORK_SCOPE.to_owned(),
            public_rpc_used: false,
            faucet_used: false,
            automatic_submission_retry: false,
        }
    }

    fn current() -> MoneroSweepEvidenceV2 {
        MoneroSweepEvidenceV2 {
            schema: M4_MONERO_CURRENT_SWEEP_SCHEMA.to_owned(),
            run_id: RUN_ID.to_owned(),
            agreement_commitment: AGREEMENT.to_owned(),
            monero_genesis_hash: GENESIS.to_owned(),
            shared_address: SHARED.to_owned(),
            reconstructed_public_spend_key: PUBLIC_SPEND.to_owned(),
            destination_address: DESTINATION.to_owned(),
            funded_amount_piconero: FUNDED,
            received_amount_piconero: RECEIVED,
            fee_piconero: REMAINDER,
            transaction_id: TRANSACTION.to_owned(),
            containing_block_hash: BLOCK.to_owned(),
            containing_block_height: 121,
            confirmations: 10,
            stable_tip_hash: TIP.to_owned(),
            stable_tip_height: 130,
            generated_confirmation_tip_height: 130,
            required_confirmations: M4_MONERO_CONFIRMATIONS,
            peer_count: 0,
            restore_height: 0,
            revealed_role: "maker_claim_signature".to_owned(),
            sweeping_role: "taker".to_owned(),
            network_scope: M4_MONERO_NETWORK_SCOPE.to_owned(),
            public_rpc_used: false,
            faucet_used: false,
            automatic_submission_retry: false,
        }
    }

    fn refund_current() -> MoneroRefundSweepEvidenceV3 {
        let current = current();
        MoneroRefundSweepEvidenceV3 {
            schema: M5_MONERO_REFUND_SWEEP_SCHEMA.to_owned(),
            journey: "refund".to_owned(),
            run_id: current.run_id,
            agreement_commitment: current.agreement_commitment,
            monero_genesis_hash: current.monero_genesis_hash,
            shared_address: current.shared_address,
            reconstructed_public_spend_key: current.reconstructed_public_spend_key,
            destination_address: current.destination_address,
            funded_amount_piconero: current.funded_amount_piconero,
            received_amount_piconero: current.received_amount_piconero,
            fee_piconero: current.fee_piconero,
            transaction_id: current.transaction_id,
            containing_block_hash: current.containing_block_hash,
            containing_block_height: current.containing_block_height,
            confirmations: current.confirmations,
            stable_tip_hash: current.stable_tip_hash,
            stable_tip_height: current.stable_tip_height,
            generated_confirmation_tip_height: current.generated_confirmation_tip_height,
            required_confirmations: current.required_confirmations,
            peer_count: current.peer_count,
            restore_height: current.restore_height,
            revealed_role: "taker_refund_signature".to_owned(),
            sweeping_role: "maker".to_owned(),
            network_scope: current.network_scope,
            public_rpc_used: current.public_rpc_used,
            faucet_used: current.faucet_used,
            automatic_submission_retry: current.automatic_submission_retry,
        }
    }

    fn refund_binding() -> RefundSweepBindingEvidenceV1 {
        RefundSweepBindingEvidenceV1 {
            schema: M5_REFUND_SWEEP_BINDING_SCHEMA,
            run_id: RUN_ID.to_owned(),
            agreement_commitment: AGREEMENT.to_owned(),
            activation_commitment: "10".repeat(32),
            refund_context_binding: "11".repeat(32),
            atomicity_scope: "successful_refund_path_conditional_atomicity",
            distributed_cross_chain_transaction_claimed: false,
            future_reorg_immunity_claimed: false,
            lez_effect: "refund",
            lez_sidecar_role: "maker",
            classifier_target: "discover_by_terms",
            classifier_outcome: "found",
            classifier_request_id: "refund-classifier-request".to_owned(),
            classifier_result_sha256: "12".repeat(32),
            classifier_scan_start_height: 90,
            classifier_scan_max_blocks: 21,
            lez_refund_transaction_id: "13".repeat(32),
            lez_refund_block_hash: "14".repeat(32),
            lez_refund_block_height: 100,
            lez_refund_transaction_index: 2,
            lez_refund_block_timestamp_ms: 15_000,
            lez_finalized_tip_hash: "15".repeat(32),
            lez_finalized_tip_height: 110,
            lez_finalized_tip_timestamp_ms: 30_000,
            aggregate_signature_sha256: "16".repeat(32),
            observed_final_signature_packet_sha256: "17".repeat(32),
            extraction_binding: "durable_maker_refund_presignature_v1",
            reconstructed_public_spend_key: PUBLIC_SPEND.to_owned(),
            monero_sweep_evidence_provenance: "refund_v3",
            monero_sweep_evidence_schema: M5_MONERO_REFUND_SWEEP_SCHEMA,
            monero_sweep_evidence_sha256: "18".repeat(32),
            monero_receipt_evidence_schema: M4_MONERO_RECEIPT_SCHEMA,
            monero_receipt_evidence_sha256: "19".repeat(32),
            monero_genesis_hash: GENESIS.to_owned(),
            monero_sweep_transaction_id: TRANSACTION.to_owned(),
            monero_evidenced_destination_address: DESTINATION.to_owned(),
            destination_ownership_binding: "owner_private_maker_wallet_boundary_not_stage_a_committed",
            monero_daemon_version: M4_MONERO_DAEMON_VERSION.to_owned(),
            monero_target_wallet_version: M4_MONERO_WALLET_VERSION,
            monero_foreign_wallet_version: M4_MONERO_WALLET_VERSION,
            monero_sweep_block_hash: BLOCK.to_owned(),
            monero_sweep_block_height: 121,
            monero_sweep_confirmations: 10,
            monero_stable_tip_hash: TIP.to_owned(),
            monero_stable_tip_height: 130,
            funded_amount_piconero: FUNDED,
            received_amount_piconero: RECEIVED,
            fee_piconero: REMAINDER,
            peer_count: 0,
            network_scope: M4_MONERO_NETWORK_SCOPE,
            public_rpc_used: false,
            faucet_used: false,
            automatic_submission_retry: false,
        }
    }

    #[test]
    fn legacy_v1_plus_receipt_exposes_remainder_but_never_claims_an_exact_fee() {
        let accounting = validate_monero_evidence_pair(
            &MoneroSweepEvidence::LegacyV1(legacy()),
            &receipt(),
            &expected(),
        )
        .expect("valid retained legacy evidence");
        assert_eq!(accounting.provenance, "legacy_v1_plus_receipt_v2");
        assert_eq!(accounting.fee_piconero, None);
        assert_eq!(accounting.unreceived_remainder_piconero, REMAINDER);
    }

    #[test]
    fn current_v2_proves_exact_fee_and_cross_checks_every_receipt_duplicate() {
        let accounting = validate_monero_evidence_pair(
            &MoneroSweepEvidence::CurrentV2(current()),
            &receipt(),
            &expected(),
        )
        .expect("valid current sweep and independent receipt");
        assert_eq!(accounting.provenance, "current_v2");
        assert_eq!(accounting.fee_piconero, Some(REMAINDER));

        macro_rules! reject_mismatch {
            ($mutation:expr) => {{
                let mut sweep = current();
                $mutation(&mut sweep);
                assert!(
                    validate_monero_evidence_pair(
                        &MoneroSweepEvidence::CurrentV2(sweep),
                        &receipt(),
                        &expected(),
                    )
                    .is_err()
                );
            }};
        }

        reject_mismatch!(
            |sweep: &mut MoneroSweepEvidenceV2| sweep.run_id = "another-valid-run".to_owned()
        );
        reject_mismatch!(
            |sweep: &mut MoneroSweepEvidenceV2| sweep.agreement_commitment = "aa".repeat(32)
        );
        reject_mismatch!(
            |sweep: &mut MoneroSweepEvidenceV2| sweep.monero_genesis_hash = "bb".repeat(32)
        );
        reject_mismatch!(|sweep: &mut MoneroSweepEvidenceV2| sweep.transaction_id = "cc".repeat(32));
        reject_mismatch!(
            |sweep: &mut MoneroSweepEvidenceV2| sweep.destination_address = SHARED.to_owned()
        );
        reject_mismatch!(|sweep: &mut MoneroSweepEvidenceV2| sweep.received_amount_piconero -= 1);
        reject_mismatch!(
            |sweep: &mut MoneroSweepEvidenceV2| sweep.containing_block_hash = "dd".repeat(32)
        );
        reject_mismatch!(|sweep: &mut MoneroSweepEvidenceV2| sweep.containing_block_height -= 1);
        reject_mismatch!(|sweep: &mut MoneroSweepEvidenceV2| sweep.confirmations += 1);
        reject_mismatch!(
            |sweep: &mut MoneroSweepEvidenceV2| sweep.stable_tip_hash = "ee".repeat(32)
        );
        reject_mismatch!(|sweep: &mut MoneroSweepEvidenceV2| sweep.stable_tip_height += 1);
        reject_mismatch!(|sweep: &mut MoneroSweepEvidenceV2| sweep.peer_count = 1);
        reject_mismatch!(
            |sweep: &mut MoneroSweepEvidenceV2| sweep.network_scope = "public".to_owned()
        );
        reject_mismatch!(|sweep: &mut MoneroSweepEvidenceV2| sweep.public_rpc_used = true);
        reject_mismatch!(|sweep: &mut MoneroSweepEvidenceV2| sweep.faucet_used = true);
    }

    #[test]
    fn refund_v3_preserves_honest_roles_and_proves_exact_receipt_accounting() {
        let sweep = MoneroSweepEvidence::RefundV3(refund_current());
        let accounting = validate_monero_refund_evidence_pair(&sweep, &receipt(), &expected())
            .expect("valid honest refund sweep and independent receipt");
        assert_eq!(accounting.provenance, "refund_v3");
        assert_eq!(accounting.sweep_schema, M5_MONERO_REFUND_SWEEP_SCHEMA);
        assert_eq!(accounting.funded_amount_piconero, FUNDED);
        assert_eq!(accounting.received_amount_piconero, RECEIVED);
        assert_eq!(accounting.fee_piconero, Some(REMAINDER));
        assert_eq!(accounting.unreceived_remainder_piconero, REMAINDER);
        assert!(
            validate_monero_evidence_pair(&sweep, &receipt(), &expected()).is_err(),
            "refund evidence must not certify the retained claim path"
        );
        assert!(
            validate_monero_refund_evidence_pair(
                &MoneroSweepEvidence::CurrentV2(current()),
                &receipt(),
                &expected(),
            )
            .is_err(),
            "claim-v2 evidence must not certify the refund path"
        );
    }

    #[test]
    fn refund_v3_rejects_crossed_journey_roles_schema_accounting_and_receipt() {
        macro_rules! reject_refund {
            ($mutation:expr) => {{
                let mut sweep = refund_current();
                $mutation(&mut sweep);
                assert!(
                    validate_monero_refund_evidence_pair(
                        &MoneroSweepEvidence::RefundV3(sweep),
                        &receipt(),
                        &expected(),
                    )
                    .is_err()
                );
            }};
        }
        reject_refund!(|sweep: &mut MoneroRefundSweepEvidenceV3| sweep.journey = "claim".to_owned());
        reject_refund!(
            |sweep: &mut MoneroRefundSweepEvidenceV3| sweep.revealed_role =
                "maker_claim_signature".to_owned()
        );
        reject_refund!(
            |sweep: &mut MoneroRefundSweepEvidenceV3| sweep.sweeping_role = "taker".to_owned()
        );
        reject_refund!(|sweep: &mut MoneroRefundSweepEvidenceV3| sweep.schema =
            M4_MONERO_CURRENT_SWEEP_SCHEMA.to_owned());
        reject_refund!(|sweep: &mut MoneroRefundSweepEvidenceV3| sweep.fee_piconero -= 1);

        let sweep = MoneroSweepEvidence::RefundV3(refund_current());
        let mut crossed_receipt = receipt();
        crossed_receipt.transaction_id = "99".repeat(32);
        assert!(
            validate_monero_refund_evidence_pair(&sweep, &crossed_receipt, &expected()).is_err(),
            "crossed independent receipt must fail closed"
        );
    }

    #[test]
    fn refund_binding_output_is_owner_private_canonical_and_conditionally_scoped() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::TempDir::new().expect("temporary binding root");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .expect("owner-private binding root");
        let output = directory.path().join("refund-binding.json");
        let destination =
            SecureDestination::new(&output, "refund binding test output").expect("destination");
        let evidence = refund_binding();
        let expected_bytes = canonical_json_bytes(&evidence, "encode expected refund binding")
            .expect("canonical expected binding");
        write_refund_binding_output(&destination, &evidence)
            .expect("write conditional refund binding");

        let metadata = std::fs::metadata(&output).expect("binding metadata");
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        let bytes = std::fs::read(&output).expect("binding bytes");
        assert_eq!(bytes, expected_bytes);
        assert_eq!(bytes.last(), Some(&b'\n'));
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).expect("canonical binding JSON");
        assert_eq!(value["schema"], M5_REFUND_SWEEP_BINDING_SCHEMA);
        assert_eq!(
            value["atomicity_scope"],
            "successful_refund_path_conditional_atomicity"
        );
        assert_eq!(value["distributed_cross_chain_transaction_claimed"], false);
        assert_eq!(value["future_reorg_immunity_claimed"], false);
        assert_eq!(value["lez_effect"], "refund");
        assert_eq!(value["lez_sidecar_role"], "maker");
        assert_eq!(
            value["extraction_binding"],
            "durable_maker_refund_presignature_v1"
        );
        assert_eq!(value["monero_sweep_evidence_provenance"], "refund_v3");
        assert_eq!(value["funded_amount_piconero"], FUNDED);
        assert_eq!(value["received_amount_piconero"], RECEIVED);
        assert_eq!(value["fee_piconero"], REMAINDER);

        let mut overstated = refund_binding();
        overstated.distributed_cross_chain_transaction_claimed = true;
        let rejected_path = directory.path().join("overstated-refund-binding.json");
        let rejected = SecureDestination::new(&rejected_path, "rejected refund binding")
            .expect("rejected destination");
        assert!(write_refund_binding_output(&rejected, &overstated).is_err());
        assert!(!rejected_path.exists());
    }

    #[test]
    fn receipt_rejects_unpinned_daemon_or_wallet_versions() {
        let sweep = MoneroSweepEvidence::CurrentV2(current());
        let mut wrong_daemon = receipt();
        wrong_daemon.daemon_version = "0.18.5.0-release".to_owned();
        assert!(
            validate_monero_evidence_pair(&sweep, &wrong_daemon, &expected()).is_err(),
            "wrong daemon version must not certify M4"
        );

        let mut wrong_target_wallet = receipt();
        wrong_target_wallet.target_wallet_version -= 1;
        assert!(
            validate_monero_evidence_pair(&sweep, &wrong_target_wallet, &expected()).is_err(),
            "wrong target wallet version must not certify M4"
        );

        let mut wrong_foreign_wallet = receipt();
        wrong_foreign_wallet.foreign_wallet_version -= 1;
        assert!(
            validate_monero_evidence_pair(&sweep, &wrong_foreign_wallet, &expected()).is_err(),
            "wrong foreign wallet version must not certify M4"
        );

        let valid = receipt();
        assert_eq!(valid.daemon_version, M4_MONERO_DAEMON_VERSION);
        assert_eq!(valid.target_wallet_version, M4_MONERO_WALLET_VERSION);
        assert_eq!(valid.foreign_wallet_version, M4_MONERO_WALLET_VERSION);
    }

    #[test]
    fn current_v2_rejects_unbalanced_or_overflowing_accounting() {
        let mut unbalanced = current();
        unbalanced.fee_piconero -= 1;
        assert!(
            validate_monero_evidence_pair(
                &MoneroSweepEvidence::CurrentV2(unbalanced),
                &receipt(),
                &expected(),
            )
            .is_err()
        );

        let mut overflow = current();
        overflow.received_amount_piconero = u64::MAX;
        overflow.fee_piconero = 1;
        assert!(
            validate_monero_evidence_pair(
                &MoneroSweepEvidence::CurrentV2(overflow),
                &receipt(),
                &expected(),
            )
            .is_err()
        );
    }
}
