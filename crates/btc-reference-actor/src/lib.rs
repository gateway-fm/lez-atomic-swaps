//! One-shot role-fixed reference actor for the LEZ/Bitcoin M3 corridor.

#![forbid(unsafe_code)]

#[cfg(not(unix))]
compile_error!("btc-reference-actor requires Unix file permissions and inode identity");

use std::{
    fmt,
    fs::{self, File},
    io::Read as _,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use async_trait::async_trait;
use bitcoin::{
    Txid,
    consensus::{deserialize, serialize},
    secp256k1::{Keypair, Message, Secp256k1, SecretKey},
};
use clap::{Parser, Subcommand};
use lez_bridge_adapter::{CapabilityFileBridgeClientFactory, FreshLezBridgeTransportFactory};
use lez_bridge_client::{
    BridgeClient, FinalizedWitnessedClaimPresence, FinalizedWitnessedFundingPresence,
    validate_prepared_witnessed_claim,
};
use lez_bridge_protocol::{
    AggregateBip340Signature, ChainClock, ChainTip, CompleteWitnessedClaimRequest,
    CompleteWitnessedClaimResult, DiscoveryWindow, EscrowState, FinalizedWitnessedClaimFacts,
    FinalizedWitnessedClaimObservationTarget, Hex32, MessageContext,
    NativeEscrowAccountObservation, NativeRefundObservation, NativeRefundObservationTarget,
    ObserveFinalizedWitnessedClaimRequest, ObserveFinalizedWitnessedFundingRequest,
    ObserveNativeRefundRequest, ObserveNativeRefundResult, Participant as BridgeParticipant,
    PrepareNativeRefundRequest, PrepareNativeRefundResult, PrepareWitnessedClaimResult,
    PreparedTransaction, PreparedWitnessedClaim, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor, SubmissionOutcome, SubmitTransactionRequest, SubmitTransactionResult,
    WitnessedEscrowMetadataFacts, WitnessedNativeEscrowTerms, WitnessedNativeEscrowTermsInput,
};
use lez_btc_core_adapter::{
    AuthorizedClaimSubmission, AuthorizedRefundSubmission, BitcoinCoreAdapter,
    BitcoinCoreEvidenceV1, BitcoinCoreRpc, ClaimObservation as BitcoinClaimObservation,
    CoreConnectivityPolicy, FundingObservation, HttpBitcoinCoreConfig, HttpBitcoinCoreRpc,
    RefundObservation as BitcoinRefundObservation,
};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, BtcAdaptorSessionDomain, BtcAgreementV1, MAX_BTC_AGREEMENT_RECORD_BYTES,
    adapt_presignature, extract_adaptor_secret, verify_adaptor_presignature, verify_adaptor_secret,
    verify_final_signature,
};
use lez_swap_core::{
    Chain, ChainPosition as SwapChainPosition, ClaimEvidence, LezUnixMilliseconds, Participant,
    Phase, SwapId,
};
use lez_swap_store::{
    AdaptorSessionIdentity, AdaptorSessionPhase, AdaptorSessionRole, BtcAgreementAcceptance,
    BtcLifecycleEvidenceV1, BtcOfflineStatus, BtcRecoveryError, PreparedPublicEffect,
    PublicEffectChain, PublicEffectDecision, PublicEffectKey, PublicEffectObservation,
    PublicEffectOperation, PublicEffectSubmissionResult, SqliteAdaptorSessionJournal,
    SqliteBtcRecoveryStore, SqlitePublicEffectJournal,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use zeroize::Zeroizing;

const CONFIG_SCHEMA_VERSION: u16 = 3;
const OUTPUT_SCHEMA_VERSION: u16 = 1;
const MAX_CONFIG_BYTES: usize = 64 * 1024;
const MAX_PREPARED_CLAIM_RESULT_BYTES: usize = 4 * 1024 * 1024;
const MAX_ADAPTOR_SECRET_FILE_BYTES: usize = 65;
const MAX_REQUEST_TIMEOUT_MILLIS: u64 = 60_000;
const FINALIZED_LEZ_CONFIRMATION_UNITS: u32 = 1;

/// Exactly one lifecycle action performed by a fresh actor process.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Subcommand)]
pub enum ActorCommand {
    /// Validate and durably accept one countersigned agreement.
    Activate,
    /// Observe and project at most one eligible lifecycle transition.
    Drive,
    /// Observe and execute at most one ordered timeout recovery transition.
    Recover,
    /// Replay only owner-local durable evidence and print secret-free status.
    Status,
}

/// Process arguments for the one-shot actor.
#[derive(Clone, Parser)]
#[command(about = "One-shot role-fixed LEZ/Bitcoin reference actor")]
pub struct ActorCli {
    /// Owner-private bounded JSON configuration.
    #[arg(long, value_name = "PRIVATE_JSON")]
    pub config: PathBuf,
    /// Single lifecycle command; the process exits after completion.
    #[command(subcommand)]
    pub command: ActorCommand,
}

impl fmt::Debug for ActorCli {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActorCli")
            .field("config", &"[REDACTED]")
            .field("command", &self.command)
            .finish()
    }
}

/// Protocol role permanently bound to one actor configuration and database.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorRole {
    /// Liquidity-providing participant.
    Maker,
    /// Offer-taking participant.
    Taker,
}

impl ActorRole {
    const fn sdk(self) -> Participant {
        match self {
            Self::Maker => Participant::Maker,
            Self::Taker => Participant::Taker,
        }
    }

    const fn bridge(self) -> BridgeParticipant {
        match self {
            Self::Maker => BridgeParticipant::Maker,
            Self::Taker => BridgeParticipant::Taker,
        }
    }

    const fn signer(self) -> AdaptorSessionRole {
        match self {
            Self::Maker => AdaptorSessionRole::Maker,
            Self::Taker => AdaptorSessionRole::Taker,
        }
    }
}

/// Explicit Bitcoin Core connectivity posture.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BitcoinConnectivity {
    /// Literal-loopback Regtest with P2P networking disabled.
    IsolatedLocal,
    /// Literal-loopback Regtest whose node has networking enabled.
    Networked,
}

impl From<BitcoinConnectivity> for CoreConnectivityPolicy {
    fn from(value: BitcoinConnectivity) -> Self {
        match value {
            BitcoinConnectivity::IsolatedLocal => Self::IsolatedLocal,
            BitcoinConnectivity::Networked => Self::Networked,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct BitcoinCoreConfig {
    endpoint: Box<str>,
    cookie_file: PathBuf,
    connectivity: BitcoinConnectivity,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LezBridgeConfig {
    endpoint: Box<str>,
    capability_file: PathBuf,
    run_id: RunId,
    runtime: RuntimeDescriptor,
    request_timeout_millis: u64,
    discovery_start_height: u64,
    discovery_max_blocks: u32,
}

/// One agreement-derived durable adaptor-signing session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SigningSessionConfig {
    session_id: Hex32,
    journal_db: PathBuf,
}

/// Claim-recovery authority required before activation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ClaimRecoveryConfig {
    bitcoin: SigningSessionConfig,
    lez: SigningSessionConfig,
    prepared_witnessed_claim_result_file: PathBuf,
    #[serde(
        default,
        deserialize_with = "deserialize_present_path",
        skip_serializing_if = "Option::is_none"
    )]
    adaptor_secret_file: Option<PathBuf>,
}

/// Role-shaped Bitcoin timeout authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RefundAuthorityConfig {
    #[serde(
        default,
        deserialize_with = "deserialize_present_path",
        skip_serializing_if = "Option::is_none"
    )]
    bitcoin_refund_key_file: Option<PathBuf>,
}

fn deserialize_present_path<'de, D>(deserializer: D) -> Result<Option<PathBuf>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    PathBuf::deserialize(deserializer).map(Some)
}

/// Owner-private, role-fixed disk configuration for one actor.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActorConfig {
    schema_version: u16,
    role: ActorRole,
    agreement_file: PathBuf,
    state_db: PathBuf,
    accepted_at_unix_seconds: u64,
    bitcoin_core: BitcoinCoreConfig,
    lez_bridge: LezBridgeConfig,
    signing: ClaimRecoveryConfig,
    refund: RefundAuthorityConfig,
}

impl fmt::Debug for ActorConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActorConfig")
            .field("schema_version", &self.schema_version)
            .field("role", &self.role)
            .field("agreement_file", &"[REDACTED]")
            .field("state_db", &"[REDACTED]")
            .field("accepted_at_unix_seconds", &self.accepted_at_unix_seconds)
            .field("bitcoin_core", &"[REDACTED]")
            .field("lez_bridge", &"[REDACTED]")
            .field("signing", &"[REDACTED]")
            .field("refund", &"[REDACTED]")
            .finish()
    }
}

impl ActorConfig {
    /// Loads one stable owner-private configuration without following a symlink.
    ///
    /// # Errors
    ///
    /// Rejects unsafe file metadata, malformed strict JSON, unsupported schema,
    /// relative or aliased paths, invalid bounds, and runtime-role drift.
    pub fn load_private(path: impl AsRef<Path>) -> Result<Self, ActorConfigError> {
        let bytes = read_stable_file(path.as_ref(), MAX_CONFIG_BYTES, true)
            .map_err(|()| ActorConfigError::Unavailable)?;
        let config: Self = decode_strict_json(&bytes)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ActorConfigError> {
        if self.schema_version != CONFIG_SCHEMA_VERSION
            || self.accepted_at_unix_seconds == 0
            || self.lez_bridge.request_timeout_millis == 0
            || self.lez_bridge.request_timeout_millis > MAX_REQUEST_TIMEOUT_MILLIS
            || self.lez_bridge.runtime.sidecar_role != self.role.bridge()
            || self.lez_bridge.runtime.compatibility != RuntimeCompatibility::LeeV0_2_0
            || DiscoveryWindow::new(
                self.lez_bridge.discovery_start_height,
                self.lez_bridge.discovery_max_blocks,
            )
            .is_err()
        {
            return Err(ActorConfigError::Invalid);
        }
        let mut paths = vec![
            &self.agreement_file,
            &self.state_db,
            &self.bitcoin_core.cookie_file,
            &self.lez_bridge.capability_file,
        ];
        if self
            .signing
            .bitcoin
            .session_id
            .as_bytes()
            .iter()
            .all(|byte| *byte == 0)
            || self
                .signing
                .lez
                .session_id
                .as_bytes()
                .iter()
                .all(|byte| *byte == 0)
            || self.signing.bitcoin.session_id == self.signing.lez.session_id
        {
            return Err(ActorConfigError::Invalid);
        }
        match (self.role, &self.signing.adaptor_secret_file) {
            (ActorRole::Taker, Some(_)) | (ActorRole::Maker, None) => {}
            (ActorRole::Taker, None) | (ActorRole::Maker, Some(_)) => {
                return Err(ActorConfigError::Invalid);
            }
        }
        paths.extend([
            &self.signing.bitcoin.journal_db,
            &self.signing.lez.journal_db,
            &self.signing.prepared_witnessed_claim_result_file,
        ]);
        if let Some(path) = &self.signing.adaptor_secret_file {
            paths.push(path);
        }
        if let Some(path) = &self.refund.bitcoin_refund_key_file {
            paths.push(path);
        }
        if paths.iter().any(|path| !is_normalized_absolute(path)) {
            return Err(ActorConfigError::Invalid);
        }
        for (index, path) in paths.iter().enumerate() {
            if paths[index + 1..].contains(path) {
                return Err(ActorConfigError::Invalid);
            }
        }
        Ok(())
    }

    fn discovery_window(&self) -> Result<DiscoveryWindow, ActorCommandError> {
        DiscoveryWindow::new(
            self.lez_bridge.discovery_start_height,
            self.lez_bridge.discovery_max_blocks,
        )
        .map_err(|_| ActorCommandError::ConfigurationUnavailable)
    }
}

/// Failure to load or validate the owner-private configuration.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ActorConfigError {
    /// The file could not be opened and read safely.
    #[error("actor configuration is unavailable")]
    Unavailable,
    /// Strict JSON, schema, bounds, role, or paths are invalid.
    #[error("actor configuration is invalid")]
    Invalid,
}

/// Secret-free response from one command invocation.
#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ActorCommandOutputV1 {
    /// Durable lifecycle status reconstructed without chain access.
    Status(ActorStatusV1),
    /// Result of activation or one bounded drive attempt.
    Effect(ActorEffectOutputV1),
}

/// Versioned result of one effect-capable actor invocation.
#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct ActorEffectOutputV1 {
    schema_version: u16,
    role: ActorRole,
    command: ActorEffectCommandV1,
    #[serde(flatten)]
    outcome: ActorEffectOutcomeV1,
    phase: ActorPhaseV1,
    revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ActorEffectCommandV1 {
    Activate,
    Drive,
    Recover,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum ActorEffectOutcomeV1 {
    Activated {
        was_replay: bool,
    },
    AwaitingObservation {
        chain: ActorChainV1,
    },
    ObservedThenProjected {
        chain: ActorChainV1,
        was_replay: bool,
    },
    ConvergedOnExistingProjection {
        chain: ActorChainV1,
        durable_revision: u64,
    },
    NotYetComposed {
        durable_revision: u64,
    },
}

/// Offline status of one role-local actor.
#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct ActorStatusV1 {
    schema_version: u16,
    role: ActorRole,
    #[serde(flatten)]
    state: ActorStateV1,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ActorStateV1 {
    NotActivated,
    Active {
        phase: ActorPhaseV1,
        revision: u64,
        next_action: ActorNextActionV1,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ActorNextActionV1 {
    ObserveTakerFirstLock,
    ObserveMakerSecondLockOrRecoverTakerLeg,
    ObserveRevealingClaim,
    ObserveFollowupClaim,
    RecoverTakerLeg,
    LaterRevisionNotYetComposed,
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ActorChainV1 {
    Bitcoin,
    Lez,
}

impl From<Chain> for ActorChainV1 {
    fn from(value: Chain) -> Self {
        match value {
            Chain::Bitcoin => Self::Bitcoin,
            Chain::Lez => Self::Lez,
            Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ActorPhaseV1 {
    Offered,
    AwaitingTakerConfirmations,
    TakerLockConfirmed,
    AwaitingMakerConfirmations,
    BothLegsLocked,
    TakerLockReorged,
    MakerLockReorged,
    ClaimEvidenceAvailable,
    Completed,
    MakerLegRefunded,
    TakerLegRefunded,
    Refunded,
    MakerRecoveryAvailable,
}

impl From<Phase> for ActorPhaseV1 {
    fn from(value: Phase) -> Self {
        match value {
            Phase::Offered => Self::Offered,
            Phase::AwaitingTakerConfirmations => Self::AwaitingTakerConfirmations,
            Phase::TakerLockConfirmed => Self::TakerLockConfirmed,
            Phase::AwaitingMakerConfirmations => Self::AwaitingMakerConfirmations,
            Phase::BothLegsLocked => Self::BothLegsLocked,
            Phase::TakerLockReorged => Self::TakerLockReorged,
            Phase::MakerLockReorged => Self::MakerLockReorged,
            Phase::ClaimEvidenceAvailable => Self::ClaimEvidenceAvailable,
            Phase::Completed => Self::Completed,
            Phase::MakerLegRefunded => Self::MakerLegRefunded,
            Phase::TakerLegRefunded => Self::TakerLegRefunded,
            Phase::Refunded => Self::Refunded,
            Phase::MakerRecoveryAvailable => Self::MakerRecoveryAvailable,
        }
    }
}

/// Stable payload-free command failure categories.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ActorCommandError {
    /// Local configuration could not be mapped into a bounded adapter.
    #[error("actor adapter configuration is unavailable")]
    ConfigurationUnavailable,
    /// The countersigned agreement file was unavailable or invalid.
    #[error("actor agreement is unavailable")]
    AgreementUnavailable,
    /// Agreement, role, runtime, or signed chain identities were inconsistent.
    #[error("actor agreement binding is invalid")]
    AgreementBindingInvalid,
    /// Role-local recovery state could not be opened or replayed.
    #[error("actor durable state is unavailable")]
    StateUnavailable,
    /// No activated role-local state exists.
    #[error("actor is not activated")]
    NotActivated,
    /// Required pre-lock signer journals or prepared claim material are unavailable.
    #[error("actor activation material is unavailable")]
    ActivationMaterialUnavailable,
    /// A chain observation was unavailable, malformed, or contradicted signed terms.
    #[error("actor chain observation is unavailable")]
    ObservationUnavailable,
    /// Durable evidence projection failed or lost its predecessor CAS.
    #[error("actor evidence projection is unavailable")]
    ProjectionUnavailable,
}

/// Funding transition selected only from the durable predecessor revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FundingTransition {
    TakerLock,
    MakerLock,
}

impl FundingTransition {
    const fn from_predecessor(revision: u64) -> Option<Self> {
        match revision {
            0 => Some(Self::TakerLock),
            1 => Some(Self::MakerLock),
            _ => None,
        }
    }

    const fn funder(self) -> Participant {
        match self {
            Self::TakerLock => Participant::Taker,
            Self::MakerLock => Participant::Maker,
        }
    }

    const fn revision(self) -> u64 {
        match self {
            Self::TakerLock => 1,
            Self::MakerLock => 2,
        }
    }

    const fn phase(self) -> Phase {
        match self {
            Self::TakerLock => Phase::TakerLockConfirmed,
            Self::MakerLock => Phase::BothLegsLocked,
        }
    }

    fn evidence(
        self,
        chain: Chain,
        transaction_id: Box<str>,
        confirmations: u32,
        chain_evidence: Vec<u8>,
    ) -> Result<BtcLifecycleEvidenceV1, BtcRecoveryError> {
        match self {
            Self::TakerLock => BtcLifecycleEvidenceV1::taker_lock(
                chain,
                transaction_id,
                confirmations,
                chain_evidence,
            ),
            Self::MakerLock => BtcLifecycleEvidenceV1::maker_lock(
                chain,
                transaction_id,
                confirmations,
                chain_evidence,
            ),
        }
    }
}

/// Affirmative or pending result returned by one agreement-aware funding observer.
#[derive(Clone, Debug, Eq, PartialEq)]
enum ActorFundingObservation {
    /// The exact agreement lock is not yet ready.
    Pending { chain: Chain },
    /// Public exact evidence is ready for one local durable projection.
    Ready {
        chain: Chain,
        transaction_id: Box<str>,
        confirmations: u32,
        chain_evidence: Vec<u8>,
    },
}

/// Revision-aware observation seam used by deterministic actor tests and live adapters.
#[async_trait]
trait FundingObservationPort: Send + Sync {
    /// Performs one bounded read-only observation.
    async fn observe(
        &self,
        agreement: &BtcAgreementV1,
        transition: FundingTransition,
    ) -> Result<ActorFundingObservation, ActorCommandError>;
}

/// Claim transition selected only from the durable predecessor revision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ClaimTransition {
    RevealingClaim,
    FollowupClaim,
}

impl ClaimTransition {
    const fn from_predecessor(revision: u64) -> Option<Self> {
        match revision {
            2 => Some(Self::RevealingClaim),
            3 => Some(Self::FollowupClaim),
            _ => None,
        }
    }

    const fn funded_participant(self) -> Participant {
        match self {
            Self::RevealingClaim => Participant::Maker,
            Self::FollowupClaim => Participant::Taker,
        }
    }

    const fn revision(self) -> u64 {
        match self {
            Self::RevealingClaim => 3,
            Self::FollowupClaim => 4,
        }
    }

    const fn phase(self) -> Phase {
        match self {
            Self::RevealingClaim => Phase::ClaimEvidenceAvailable,
            Self::FollowupClaim => Phase::Completed,
        }
    }

    const fn predecessor_revision(self) -> u64 {
        self.revision() - 1
    }

    const fn submitter(self) -> Participant {
        match self {
            Self::RevealingClaim => Participant::Taker,
            Self::FollowupClaim => Participant::Maker,
        }
    }
}

/// Affirmative or pending result returned by one agreement-aware claim observer.
#[derive(Clone, Debug, Eq, PartialEq)]
enum ActorClaimObservation {
    /// The exact claim is not yet canonical at the signed policy.
    Pending { chain: Chain },
    /// Public exact evidence is ready for one local durable projection.
    Ready {
        chain: Chain,
        transaction_id: Box<str>,
        confirmations: u32,
        chain_evidence: Vec<u8>,
        /// Required only for the revealing claim.
        revealing_public_signature: Option<[u8; 64]>,
    },
}

/// Revision-aware claim seam used by deterministic actor tests and live adapters.
#[async_trait]
trait ClaimObservationPort: Send + Sync {
    /// Performs one bounded read-only exact-claim observation.
    async fn observe(
        &self,
        agreement: &BtcAgreementV1,
        transition: ClaimTransition,
    ) -> Result<ActorClaimObservation, ActorCommandError>;
}

/// Timeout transition selected only from exact durable revision and phase.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RefundTransition {
    FirstLockRecovery,
    MakerLeg,
    TakerLeg,
}

impl RefundTransition {
    const fn from_status(status: &BtcOfflineStatus) -> Option<Self> {
        match (status.revision(), status.phase()) {
            (1, Phase::TakerLockConfirmed) => Some(Self::FirstLockRecovery),
            (2, Phase::BothLegsLocked) => Some(Self::MakerLeg),
            (3, Phase::MakerLegRefunded) => Some(Self::TakerLeg),
            _ => None,
        }
    }

    const fn funded_participant(self) -> Participant {
        match self {
            Self::MakerLeg => Participant::Maker,
            Self::FirstLockRecovery | Self::TakerLeg => Participant::Taker,
        }
    }

    const fn revision(self) -> u64 {
        match self {
            Self::FirstLockRecovery => 2,
            Self::MakerLeg => 3,
            Self::TakerLeg => 4,
        }
    }

    const fn phase(self) -> Phase {
        match self {
            Self::FirstLockRecovery | Self::TakerLeg => Phase::Refunded,
            Self::MakerLeg => Phase::MakerLegRefunded,
        }
    }

    const fn predecessor_revision(self) -> u64 {
        self.revision() - 1
    }

    fn evidence(
        self,
        chain: Chain,
        transaction_id: Box<str>,
        confirmations: u32,
        chain_evidence: Vec<u8>,
        position: SwapChainPosition,
    ) -> Result<BtcLifecycleEvidenceV1, BtcRecoveryError> {
        match self {
            Self::MakerLeg => BtcLifecycleEvidenceV1::maker_leg_refund(
                chain,
                transaction_id,
                confirmations,
                chain_evidence,
                position,
            ),
            Self::FirstLockRecovery | Self::TakerLeg => BtcLifecycleEvidenceV1::taker_leg_refund(
                chain,
                transaction_id,
                confirmations,
                chain_evidence,
                position,
            ),
        }
    }
}

/// Affirmative or pending result returned by one agreement-aware refund observer.
#[derive(Clone, Debug, Eq, PartialEq)]
enum ActorRefundObservation {
    /// The expected timeout effect is not yet canonical.
    Pending { chain: Chain },
    /// Public exact evidence is ready for one local durable projection.
    Ready {
        chain: Chain,
        transaction_id: Box<str>,
        confirmations: u32,
        chain_evidence: Vec<u8>,
        position: SwapChainPosition,
    },
}

/// Revision-aware refund seam used by deterministic actor tests and live adapters.
#[async_trait]
trait RefundObservationPort: Send + Sync {
    /// Performs one bounded read-only exact-refund observation.
    async fn observe(
        &self,
        agreement: &BtcAgreementV1,
        transition: RefundTransition,
    ) -> Result<ActorRefundObservation, ActorCommandError>;
}

/// Cross-chain admission result for the revision-one absent-maker branch.
#[derive(Clone, Debug, Eq, PartialEq)]
enum FirstLockRecoverySafetyObservation {
    /// A stable view could not prove either a canonical maker lock or safe absence.
    Uncertain { maker_chain: Chain },
    /// The exact maker second lock is canonical and must win over direct recovery.
    MakerLockReady {
        chain: Chain,
        transaction_id: Box<str>,
        confirmations: u32,
        chain_evidence: Vec<u8>,
    },
    /// The signed cutoff passed and the exact maker second lock is affirmatively absent.
    ReadyToRefund {
        maker_chain: Chain,
        cutoff_unix_seconds: u64,
        observed_unix_seconds: u64,
        absence_evidence: Vec<u8>,
    },
}

/// Fresh two-chain safety check used only before a first-lock refund can gain send authority.
#[async_trait]
trait FirstLockRecoverySafetyPort: Send + Sync {
    /// Rechecks the signed cutoff and exact maker-lock state at stable canonical tips.
    async fn observe(
        &self,
        agreement: &BtcAgreementV1,
        read_ordinal: u8,
    ) -> Result<FirstLockRecoverySafetyObservation, ActorCommandError>;
}

/// Live LEZ maker-lock classifier; every call creates a fresh authenticated transport.
struct LiveLezMakerLockSafety {
    config: ActorConfig,
}

/// Live Bitcoin maker-lock classifier; every call creates a fresh Core RPC client.
struct LiveBitcoinMakerLockSafety {
    config: ActorConfig,
}

/// One stable Bitcoin claim scan reduced to facts needed by actor policy.
#[derive(Clone, Debug, Eq, PartialEq)]
enum BitcoinClaimScan {
    /// A stable-tip spender-index scan proved the signed outpoint unspent.
    Unspent,
    /// The exact agreement claim was found and independently validated.
    Exact(BitcoinExactClaim),
    /// Node, tip, index, or exact-byte ambiguity proved neither state.
    Uncertain,
}

/// Exact public Bitcoin claim material returned by the typed adapter boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
struct BitcoinExactClaim {
    transaction_bytes: Vec<u8>,
    transaction_id: Box<str>,
    confirmations: u32,
    chain_evidence: Vec<u8>,
    public_signature: [u8; 64],
    finalized: bool,
}

/// Adapter seam keeping durable send authority in the actor, not the RPC layer.
#[async_trait]
trait BitcoinClaimChainPort: Send + Sync {
    /// Performs one stable bounded scan. All ambiguous failures become `Uncertain`.
    async fn observe_claim(&self, agreement: &BtcAgreementV1) -> BitcoinClaimScan;

    /// Consumes authority already durably committed by the caller.
    async fn submit_authorized_claim(
        &self,
        agreement: &BtcAgreementV1,
        transaction_bytes: &[u8],
        expected_transaction_id: Txid,
    ) -> Result<AuthorizedClaimSubmission, ActorCommandError>;
}

/// Stable Core refund state reduced to actor-owned journal semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
enum BitcoinRefundScan {
    Immature,
    Eligible,
    Exact(BitcoinExactRefund),
    Conflicting,
    Uncertain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BitcoinExactRefund {
    transaction_bytes: Vec<u8>,
    transaction_id: Box<str>,
    witness_transaction_id: Box<str>,
    confirmations: u32,
    block_height: Option<u32>,
    chain_evidence: Vec<u8>,
    finalized: bool,
}

#[async_trait]
trait BitcoinRefundChainPort: Send + Sync {
    async fn observe_refund(&self, agreement: &BtcAgreementV1) -> BitcoinRefundScan;

    async fn submit_authorized_refund(
        &self,
        agreement: &BtcAgreementV1,
        transaction_bytes: &[u8],
        expected_transaction_id: Txid,
    ) -> Result<AuthorizedRefundSubmission, ActorCommandError>;
}

/// Fully signed exact public refund bytes, persisted before send eligibility.
#[derive(Clone, Debug, Eq, PartialEq)]
struct PreparedBitcoinRefundEffect {
    effect: PreparedPublicEffect,
    expected_transaction_id: Txid,
    expected_witness_transaction_id: bitcoin::Wtxid,
}

/// Fully signed exact public claim bytes, prepared before any send authorization.
#[derive(Clone, Debug, Eq, PartialEq)]
struct PreparedBitcoinClaimEffect {
    effect: PreparedPublicEffect,
    expected_transaction_id: Txid,
}

/// Exact public LEZ refund material persisted before any presence scan or send authority.
#[derive(Clone, Debug, Eq, PartialEq)]
struct PreparedLezRefundEffect {
    effect: PreparedPublicEffect,
    transaction: PreparedTransaction,
}

/// Adapter seam keeping LEZ refund preparation, observation, and submission explicit.
#[async_trait]
trait LezRefundChainPort: Send + Sync {
    async fn prepare_native_refund(
        &self,
        request: PrepareNativeRefundRequest,
    ) -> Result<PrepareNativeRefundResult, ActorCommandError>;

    async fn observe_native_refund(
        &self,
        request: ObserveNativeRefundRequest,
    ) -> Result<ObserveNativeRefundResult, ActorCommandError>;

    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, ActorCommandError>;
}

#[async_trait]
impl LezRefundChainPort for BridgeClient {
    async fn prepare_native_refund(
        &self,
        request: PrepareNativeRefundRequest,
    ) -> Result<PrepareNativeRefundResult, ActorCommandError> {
        BridgeClient::prepare_native_refund(self, request)
            .await
            .map_err(|_| ActorCommandError::ObservationUnavailable)
    }

    async fn observe_native_refund(
        &self,
        request: ObserveNativeRefundRequest,
    ) -> Result<ObserveNativeRefundResult, ActorCommandError> {
        BridgeClient::observe_native_refund(self, request)
            .await
            .map_err(|_| ActorCommandError::ObservationUnavailable)
    }

    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, ActorCommandError> {
        BridgeClient::submit_transaction(self, request)
            .await
            .map_err(|_| ActorCommandError::ObservationUnavailable)
    }
}

/// Exact completed LEZ public claim persisted before any presence scan or send authority.
#[derive(Clone, Debug, Eq, PartialEq)]
struct PreparedLezClaimEffect {
    effect: PreparedPublicEffect,
    transaction: PreparedTransaction,
    aggregate_signature: [u8; 64],
}

/// Adapter seam keeping LEZ completion, presence, and submission authority explicit.
#[async_trait]
trait LezClaimChainPort: Send + Sync {
    /// Completes an exact signed transcript without submitting it.
    async fn complete_witnessed_claim(
        &self,
        request: CompleteWitnessedClaimRequest,
    ) -> Result<CompleteWitnessedClaimResult, ActorCommandError>;

    /// Classifies exact presence in one deterministic caller-owned finalized window.
    async fn classify_finalized_witnessed_claim(
        &self,
        request: ObserveFinalizedWitnessedClaimRequest,
    ) -> Result<FinalizedWitnessedClaimPresence, ActorCommandError>;

    /// Consumes authority already durably committed by the actor.
    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, ActorCommandError>;
}

#[async_trait]
impl LezClaimChainPort for BridgeClient {
    async fn complete_witnessed_claim(
        &self,
        request: CompleteWitnessedClaimRequest,
    ) -> Result<CompleteWitnessedClaimResult, ActorCommandError> {
        BridgeClient::complete_witnessed_claim(self, request)
            .await
            .map_err(|_| ActorCommandError::ObservationUnavailable)
    }

    async fn classify_finalized_witnessed_claim(
        &self,
        request: ObserveFinalizedWitnessedClaimRequest,
    ) -> Result<FinalizedWitnessedClaimPresence, ActorCommandError> {
        BridgeClient::classify_finalized_witnessed_claim(self, request)
            .await
            .map_err(|_| ActorCommandError::ObservationUnavailable)
    }

    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, ActorCommandError> {
        BridgeClient::submit_transaction(self, request)
            .await
            .map_err(|_| ActorCommandError::ObservationUnavailable)
    }
}

/// Executes one disk-configured role-fixed command.
///
/// `Status` never constructs a chain client. `Drive` performs one observation,
/// returns from the chain call, and only then starts the `SQLite` projection. The
/// command therefore makes no false cross-system atomicity claim.
///
/// # Errors
///
/// Returns a stable failure category for invalid local material, chain facts,
/// role drift, or durable replay/CAS failure.
pub async fn execute_actor_command(
    config: &ActorConfig,
    command: ActorCommand,
) -> Result<ActorCommandOutputV1, ActorCommandError> {
    match command {
        ActorCommand::Activate => activate(config).map(ActorCommandOutputV1::Effect),
        ActorCommand::Status => status(config).map(ActorCommandOutputV1::Status),
        ActorCommand::Recover => Box::pin(recover_live(config))
            .await
            .map(ActorCommandOutputV1::Effect),
        ActorCommand::Drive => drive_live(config).await.map(ActorCommandOutputV1::Effect),
    }
}

fn activate(config: &ActorConfig) -> Result<ActorEffectOutputV1, ActorCommandError> {
    let (agreement, wire) = load_agreement(config)?;
    validate_activation_material(config, &agreement)?;
    let store = open_store(config, &agreement, wire)?;
    let was_replay = store.acceptance_was_replay();
    let status = store
        .status()
        .map_err(|_| ActorCommandError::StateUnavailable)?;
    Ok(effect_output(
        config,
        ActorEffectCommandV1::Activate,
        ActorEffectOutcomeV1::Activated { was_replay },
        &status,
    ))
}

fn validate_activation_material(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
) -> Result<(), ActorCommandError> {
    let _ = load_prepared_witnessed_claim(config, agreement)?;
    validate_signer_journal(
        config,
        agreement,
        BtcAdaptorSessionDomain::Bitcoin,
        &config.signing.bitcoin,
    )?;
    validate_signer_journal(
        config,
        agreement,
        BtcAdaptorSessionDomain::Lez,
        &config.signing.lez,
    )?;
    validate_taker_adaptor_secret(config, agreement)?;
    validate_bitcoin_refund_authority(config, agreement)
}

fn load_prepared_witnessed_claim(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
) -> Result<PrepareWitnessedClaimResult, ActorCommandError> {
    let bytes = read_stable_file(
        &config.signing.prepared_witnessed_claim_result_file,
        MAX_PREPARED_CLAIM_RESULT_BYTES,
        false,
    )
    .map_err(|()| ActorCommandError::ActivationMaterialUnavailable)?;
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
    let prepared = PrepareWitnessedClaimResult::deserialize(&mut deserializer)
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?;
    deserializer
        .end()
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?;
    if prepared.context.run_id != config.lez_bridge.run_id
        || prepared.context.sidecar_role != bridge_participant(agreement.lez_claimant())
        || prepared.context.request_id != prepared.claim.preparation_request_id
        || validate_prepared_witnessed_claim(&prepared.claim).is_err()
        || prepared.claim.message_hash.as_bytes() != agreement.lez_terms().claim_message_hash()
    {
        return Err(ActorCommandError::ActivationMaterialUnavailable);
    }
    Ok(prepared)
}

fn validate_taker_adaptor_secret(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
) -> Result<(), ActorCommandError> {
    if config.role == ActorRole::Maker {
        return Ok(());
    }
    let path = config
        .signing
        .adaptor_secret_file
        .as_ref()
        .ok_or(ActorCommandError::ActivationMaterialUnavailable)?;
    let secret = read_private_adaptor_secret(path)
        .map_err(|()| ActorCommandError::ActivationMaterialUnavailable)?;
    let context = agreement
        .adaptor_session_context(
            BtcAdaptorSessionDomain::Bitcoin,
            *config.signing.bitcoin.session_id.as_bytes(),
        )
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?;
    verify_adaptor_secret(&context, secret)
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)
}

fn validate_bitcoin_refund_authority(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
) -> Result<(), ActorCommandError> {
    let path = config.refund.bitcoin_refund_key_file.as_ref();
    if config.role.sdk() != agreement.bitcoin_funder() {
        return if path.is_none() {
            Ok(())
        } else {
            Err(ActorCommandError::ActivationMaterialUnavailable)
        };
    }
    let secret =
        read_private_adaptor_secret(path.ok_or(ActorCommandError::ActivationMaterialUnavailable)?)
            .map_err(|()| ActorCommandError::ActivationMaterialUnavailable)?;
    let secret = SecretKey::from_slice(secret.as_ref())
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?;
    let actual = Keypair::from_secret_key(&Secp256k1::new(), &secret)
        .x_only_public_key()
        .0
        .serialize();
    if &actual
        != agreement
            .participant(config.role.sdk())
            .bitcoin_refund_key()
    {
        return Err(ActorCommandError::ActivationMaterialUnavailable);
    }
    Ok(())
}

fn validate_signer_journal(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    domain: BtcAdaptorSessionDomain,
    signing: &SigningSessionConfig,
) -> Result<(), ActorCommandError> {
    let session_id = *signing.session_id.as_bytes();
    let context = agreement
        .adaptor_session_context(domain, session_id)
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?;
    let expected_identity = AdaptorSessionIdentity::new(
        session_id,
        config.role.signer(),
        context.durable_context_binding(),
        context.message(),
        context.adaptor_point(),
        context.ordered_public_keys(),
    );
    let journal = SqliteAdaptorSessionJournal::open_existing(&signing.journal_db)
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?;
    let snapshot = journal
        .load(&session_id)
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?
        .ok_or(ActorCommandError::ActivationMaterialUnavailable)?;
    let presignature = snapshot
        .presignature()
        .ok_or(ActorCommandError::ActivationMaterialUnavailable)?;
    if snapshot.identity() != &expected_identity
        || snapshot.phase() != AdaptorSessionPhase::PresignatureVerified
        || verify_adaptor_presignature(&context, *presignature.bytes()).is_err()
    {
        return Err(ActorCommandError::ActivationMaterialUnavailable);
    }
    Ok(())
}

fn status(config: &ActorConfig) -> Result<ActorStatusV1, ActorCommandError> {
    if !state_file_exists(&config.state_db)? {
        return Ok(not_activated_status(config));
    }
    let (agreement, wire) = load_agreement(config)?;
    let store = match open_existing_store(config, &agreement, wire) {
        Ok(store) => store,
        Err(BtcRecoveryError::MissingAgreementAcceptance) => {
            return Ok(not_activated_status(config));
        }
        Err(_) => return Err(ActorCommandError::StateUnavailable),
    };
    let status = store
        .status()
        .map_err(|_| ActorCommandError::StateUnavailable)?;
    Ok(status_output(config, &status))
}

async fn drive_live(config: &ActorConfig) -> Result<ActorEffectOutputV1, ActorCommandError> {
    if !state_file_exists(&config.state_db)? {
        return Err(ActorCommandError::NotActivated);
    }
    let (agreement, wire) = load_agreement(config)?;
    let store = match open_existing_store(config, &agreement, wire.clone()) {
        Ok(store) => store,
        Err(BtcRecoveryError::MissingAgreementAcceptance) => {
            return Err(ActorCommandError::NotActivated);
        }
        Err(_) => return Err(ActorCommandError::StateUnavailable),
    };
    let durable = store
        .status()
        .map_err(|_| ActorCommandError::StateUnavailable)?;
    drop(store);
    if let Some(transition) = FundingTransition::from_predecessor(durable.revision()) {
        let expected_chain = agreement.coordinator().funded_chain(transition.funder());
        return match expected_chain {
            Chain::Bitcoin => {
                let core_config = HttpBitcoinCoreConfig::new(config.bitcoin_core.endpoint.clone())
                    .and_then(|value| value.with_cookie_file(&config.bitcoin_core.cookie_file))
                    .map_err(|_| ActorCommandError::ConfigurationUnavailable)?;
                let rpc = HttpBitcoinCoreRpc::connect(&core_config)
                    .map_err(|_| ActorCommandError::ConfigurationUnavailable)?;
                let observer = BitcoinFundingObserver {
                    adapter: BitcoinCoreAdapter::new(rpc, config.bitcoin_core.connectivity.into()),
                };
                drive_with_observer(config, agreement, wire, &observer).await
            }
            Chain::Lez => {
                let factory = CapabilityFileBridgeClientFactory::new(
                    config.lez_bridge.endpoint.to_string(),
                    config.lez_bridge.capability_file.clone(),
                    config.lez_bridge.run_id.clone(),
                    config.lez_bridge.runtime.clone(),
                    Duration::from_millis(config.lez_bridge.request_timeout_millis),
                );
                let client = factory
                    .fresh_transport()
                    .map_err(|_| ActorCommandError::ConfigurationUnavailable)?;
                let observer = LezFundingObserver { config, client };
                drive_with_observer(config, agreement, wire, &observer).await
            }
            Chain::Zcash | Chain::Monero => Err(ActorCommandError::AgreementBindingInvalid),
        };
    }
    let Some(transition) = ClaimTransition::from_predecessor(durable.revision()) else {
        return Ok(effect_output(
            config,
            ActorEffectCommandV1::Drive,
            ActorEffectOutcomeV1::NotYetComposed {
                durable_revision: durable.revision(),
            },
            &durable,
        ));
    };
    let expected_chain = agreement
        .coordinator()
        .funded_chain(transition.funded_participant());
    match expected_chain {
        Chain::Bitcoin => {
            validate_activation_material(config, &agreement)?;
            let effect = prepare_bitcoin_claim_effect(config, &agreement, transition, &durable)?;
            let core_config = HttpBitcoinCoreConfig::new(config.bitcoin_core.endpoint.clone())
                .and_then(|value| value.with_cookie_file(&config.bitcoin_core.cookie_file))
                .map_err(|_| ActorCommandError::ConfigurationUnavailable)?;
            let rpc = HttpBitcoinCoreRpc::connect(&core_config)
                .map_err(|_| ActorCommandError::ConfigurationUnavailable)?;
            let observer = BitcoinClaimObserver {
                chain: BitcoinCoreAdapter::new(rpc, config.bitcoin_core.connectivity.into()),
                effect,
                state_db: config.state_db.clone(),
            };
            drive_claim_with_observer(config, agreement, wire, &observer).await
        }
        Chain::Lez => drive_live_lez_claim(config, agreement, wire, transition, &durable).await,
        Chain::Zcash | Chain::Monero => Err(ActorCommandError::AgreementBindingInvalid),
    }
}

async fn recover_live(config: &ActorConfig) -> Result<ActorEffectOutputV1, ActorCommandError> {
    if !state_file_exists(&config.state_db)? {
        return Err(ActorCommandError::NotActivated);
    }
    let (agreement, wire) = load_agreement(config)?;
    let store = match open_existing_store(config, &agreement, wire.clone()) {
        Ok(store) => store,
        Err(BtcRecoveryError::MissingAgreementAcceptance) => {
            return Err(ActorCommandError::NotActivated);
        }
        Err(_) => return Err(ActorCommandError::StateUnavailable),
    };
    let durable = store
        .status()
        .map_err(|_| ActorCommandError::StateUnavailable)?;
    drop(store);
    let Some(transition) = RefundTransition::from_status(&durable) else {
        return Ok(effect_output(
            config,
            ActorEffectCommandV1::Recover,
            ActorEffectOutcomeV1::NotYetComposed {
                durable_revision: durable.revision(),
            },
            &durable,
        ));
    };
    let chain = agreement
        .coordinator()
        .funded_chain(transition.funded_participant());
    match chain {
        Chain::Bitcoin => {
            let effect = prepare_bitcoin_refund_effect(config, &agreement, transition, &durable)?;
            let core_config = HttpBitcoinCoreConfig::new(config.bitcoin_core.endpoint.clone())
                .and_then(|value| value.with_cookie_file(&config.bitcoin_core.cookie_file))
                .map_err(|_| ActorCommandError::ConfigurationUnavailable)?;
            let rpc = HttpBitcoinCoreRpc::connect(&core_config)
                .map_err(|_| ActorCommandError::ConfigurationUnavailable)?;
            let observer = BitcoinRefundObserver {
                chain: BitcoinCoreAdapter::new(rpc, config.bitcoin_core.connectivity.into()),
                effect,
                state_db: config.state_db.clone(),
            };
            if transition == RefundTransition::FirstLockRecovery {
                let safety = LiveLezMakerLockSafety {
                    config: config.clone(),
                };
                drive_first_lock_refund_with_observer(config, agreement, wire, &safety, &observer)
                    .await
            } else {
                drive_refund_with_observer(config, agreement, wire, &observer).await
            }
        }
        Chain::Lez => Box::pin(drive_live_lez_refund(config, agreement, wire, transition)).await,
        Chain::Zcash | Chain::Monero => Err(ActorCommandError::AgreementBindingInvalid),
    }
}

async fn drive_live_lez_refund(
    config: &ActorConfig,
    agreement: BtcAgreementV1,
    wire: Vec<u8>,
    transition: RefundTransition,
) -> Result<ActorEffectOutputV1, ActorCommandError> {
    validate_actor_binding(config, &agreement)?;
    let factory = CapabilityFileBridgeClientFactory::new(
        config.lez_bridge.endpoint.to_string(),
        config.lez_bridge.capability_file.clone(),
        config.lez_bridge.run_id.clone(),
        config.lez_bridge.runtime.clone(),
        Duration::from_millis(config.lez_bridge.request_timeout_millis),
    );
    let client = factory
        .fresh_transport()
        .map_err(|_| ActorCommandError::ConfigurationUnavailable)?;
    let observer = LezRefundObserver {
        config: config.clone(),
        chain: client,
        state_db: config.state_db.clone(),
    };
    if transition == RefundTransition::FirstLockRecovery {
        let safety = LiveBitcoinMakerLockSafety {
            config: config.clone(),
        };
        drive_first_lock_refund_with_observer(config, agreement, wire, &safety, &observer).await
    } else {
        drive_refund_with_observer(config, agreement, wire, &observer).await
    }
}

async fn drive_live_lez_claim(
    config: &ActorConfig,
    agreement: BtcAgreementV1,
    wire: Vec<u8>,
    transition: ClaimTransition,
    durable: &BtcOfflineStatus,
) -> Result<ActorEffectOutputV1, ActorCommandError> {
    validate_activation_material(config, &agreement)?;
    let factory = CapabilityFileBridgeClientFactory::new(
        config.lez_bridge.endpoint.to_string(),
        config.lez_bridge.capability_file.clone(),
        config.lez_bridge.run_id.clone(),
        config.lez_bridge.runtime.clone(),
        Duration::from_millis(config.lez_bridge.request_timeout_millis),
    );
    let client = factory
        .fresh_transport()
        .map_err(|_| ActorCommandError::ConfigurationUnavailable)?;
    let prepared = load_prepared_witnessed_claim(config, &agreement)?;
    let effect = prepare_lez_claim_effect(config, &agreement, transition, durable, &client).await?;
    let observer = LezClaimObserver {
        config,
        chain: client,
        effect,
        prepared_claim: prepared.claim,
        state_db: config.state_db.clone(),
    };
    drive_claim_with_observer(config, agreement, wire, &observer).await
}

/// Drives one composed funding revision with an injected agreement-aware observer.
///
/// This is the deterministic TDD seam for the exact same durable projection used
/// by the disk-configured command.
///
/// # Errors
///
/// Fails closed on missing activation, observer contradiction, non-next revision,
/// changed replay evidence, or predecessor-CAS loss.
async fn drive_with_observer(
    config: &ActorConfig,
    agreement: BtcAgreementV1,
    agreement_wire: Vec<u8>,
    observer: &dyn FundingObservationPort,
) -> Result<ActorEffectOutputV1, ActorCommandError> {
    if !state_file_exists(&config.state_db)? {
        return Err(ActorCommandError::NotActivated);
    }
    validate_actor_binding(config, &agreement)?;
    let (mut store, before) = open_projection(config, &agreement, agreement_wire)?;
    let Some(transition) = FundingTransition::from_predecessor(before.revision()) else {
        return Ok(effect_output(
            config,
            ActorEffectCommandV1::Drive,
            ActorEffectOutcomeV1::NotYetComposed {
                durable_revision: before.revision(),
            },
            &before,
        ));
    };
    let expected_chain = agreement.coordinator().funded_chain(transition.funder());
    let observation = observer.observe(&agreement, transition).await?;
    let (chain, transaction_id, confirmations, chain_evidence) = match observation {
        ActorFundingObservation::Pending { chain } => {
            if chain != expected_chain {
                return Err(ActorCommandError::AgreementBindingInvalid);
            }
            return Ok(effect_output(
                config,
                ActorEffectCommandV1::Drive,
                ActorEffectOutcomeV1::AwaitingObservation {
                    chain: chain.into(),
                },
                &before,
            ));
        }
        ActorFundingObservation::Ready {
            chain,
            transaction_id,
            confirmations,
            chain_evidence,
        } => {
            if chain != expected_chain {
                return Err(ActorCommandError::AgreementBindingInvalid);
            }
            (chain, transaction_id, confirmations, chain_evidence)
        }
    };
    let evidence = transition
        .evidence(chain, transaction_id, confirmations, chain_evidence)
        .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    let (outcome, after) = match store.project(before.revision(), &evidence) {
        Ok(commit) => {
            let after = store
                .status()
                .map_err(|_| ActorCommandError::StateUnavailable)?;
            (
                ActorEffectOutcomeV1::ObservedThenProjected {
                    chain: chain.into(),
                    was_replay: commit.was_replay(),
                },
                after,
            )
        }
        Err(BtcRecoveryError::EvidenceConflict { revision })
            if revision == transition.revision() =>
        {
            let winner = store
                .status()
                .map_err(|_| ActorCommandError::StateUnavailable)?;
            if winner.revision() != transition.revision() || winner.phase() != transition.phase() {
                return Err(ActorCommandError::ProjectionUnavailable);
            }
            (
                ActorEffectOutcomeV1::ConvergedOnExistingProjection {
                    chain: expected_chain.into(),
                    durable_revision: winner.revision(),
                },
                winner,
            )
        }
        Err(_) => return Err(ActorCommandError::ProjectionUnavailable),
    };
    Ok(effect_output(
        config,
        ActorEffectCommandV1::Drive,
        outcome,
        &after,
    ))
}

/// Drives one claim revision from exact canonical public evidence.
///
/// The complete activation-material gate is rerun immediately before the
/// observation. Revision three accepts only a final signature related to the
/// agreement-derived presignature and adaptor point. Takers reproduce it from
/// their private scalar; makers recover and point-check that scalar from the
/// public signature. Only its one-way commitment enters durable lifecycle
/// evidence.
async fn drive_claim_with_observer(
    config: &ActorConfig,
    agreement: BtcAgreementV1,
    agreement_wire: Vec<u8>,
    observer: &dyn ClaimObservationPort,
) -> Result<ActorEffectOutputV1, ActorCommandError> {
    if !state_file_exists(&config.state_db)? {
        return Err(ActorCommandError::NotActivated);
    }
    validate_actor_binding(config, &agreement)?;
    validate_activation_material(config, &agreement)?;
    let (mut store, before) = open_projection(config, &agreement, agreement_wire)?;
    let Some(transition) = ClaimTransition::from_predecessor(before.revision()) else {
        return Ok(effect_output(
            config,
            ActorEffectCommandV1::Drive,
            ActorEffectOutcomeV1::NotYetComposed {
                durable_revision: before.revision(),
            },
            &before,
        ));
    };
    let expected_chain = agreement
        .coordinator()
        .funded_chain(transition.funded_participant());
    let observation = observer.observe(&agreement, transition).await?;
    let (chain, transaction_id, confirmations, chain_evidence, public_signature) = match observation
    {
        ActorClaimObservation::Pending { chain } => {
            if chain != expected_chain {
                return Err(ActorCommandError::AgreementBindingInvalid);
            }
            return Ok(effect_output(
                config,
                ActorEffectCommandV1::Drive,
                ActorEffectOutcomeV1::AwaitingObservation {
                    chain: chain.into(),
                },
                &before,
            ));
        }
        ActorClaimObservation::Ready {
            chain,
            transaction_id,
            confirmations,
            chain_evidence,
            revealing_public_signature,
        } => {
            if chain != expected_chain
                || !claim_confirmation_is_ready(&agreement, chain, confirmations)
            {
                return Err(ActorCommandError::AgreementBindingInvalid);
            }
            (
                chain,
                transaction_id,
                confirmations,
                chain_evidence,
                revealing_public_signature,
            )
        }
    };
    let evidence = claim_lifecycle_evidence(
        config,
        &agreement,
        transition,
        chain,
        transaction_id,
        confirmations,
        chain_evidence,
        public_signature,
    )?;
    project_claim_transition(
        config,
        &mut store,
        &before,
        transition,
        chain,
        expected_chain,
        &evidence,
    )
}

/// Drives a timeout revision without granting revision-one send authority.
async fn drive_refund_with_observer(
    config: &ActorConfig,
    agreement: BtcAgreementV1,
    agreement_wire: Vec<u8>,
    observer: &dyn RefundObservationPort,
) -> Result<ActorEffectOutputV1, ActorCommandError> {
    drive_refund_after_first_lock_safety(config, agreement, agreement_wire, None, observer).await
}

const MAX_FIRST_LOCK_SAFETY_READ_BYTES: usize = 16 * 1024;
const MAX_FIRST_LOCK_REFUND_EVIDENCE_BYTES: usize = 32 * 1024;
const MAX_FIRST_LOCK_ENVELOPE_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug)]
struct FirstLockRecoveryAdmission {
    maker_chain: Chain,
    cutoff_unix_seconds: u64,
    first_observed_unix_seconds: u64,
    first_absence_evidence: Vec<u8>,
    second_observed_unix_seconds: u64,
    second_absence_evidence: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct FirstLockRecoveryReadEvidenceV1 {
    read_ordinal: u8,
    observed_unix_seconds: u64,
    absence_evidence_hex: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct FirstLockRecoveryChainEvidenceV1 {
    schema_version: u16,
    agreement_commitment: String,
    maker_chain: Chain,
    cutoff_unix_seconds: u64,
    first_read: FirstLockRecoveryReadEvidenceV1,
    second_read: FirstLockRecoveryReadEvidenceV1,
    refund_chain_evidence_hex: String,
}

fn encode_first_lock_recovery_chain_evidence(
    agreement: &BtcAgreementV1,
    admission: &FirstLockRecoveryAdmission,
    refund_chain_evidence: &[u8],
) -> Result<Vec<u8>, ActorCommandError> {
    let maker_chain = agreement.coordinator().funded_chain(Participant::Maker);
    let cutoff = agreement
        .body()
        .recovery_plan()
        .maker_second_lock_cutoff_unix_seconds();
    if admission.maker_chain != maker_chain
        || admission.cutoff_unix_seconds != cutoff
        || admission.first_observed_unix_seconds < cutoff
        || admission.second_observed_unix_seconds < admission.first_observed_unix_seconds
        || admission.first_absence_evidence.is_empty()
        || admission.second_absence_evidence.is_empty()
        || admission.first_absence_evidence == admission.second_absence_evidence
        || admission.first_absence_evidence.len() > MAX_FIRST_LOCK_SAFETY_READ_BYTES
        || admission.second_absence_evidence.len() > MAX_FIRST_LOCK_SAFETY_READ_BYTES
        || refund_chain_evidence.is_empty()
        || refund_chain_evidence.len() > MAX_FIRST_LOCK_REFUND_EVIDENCE_BYTES
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    let evidence = FirstLockRecoveryChainEvidenceV1 {
        schema_version: 1,
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        maker_chain,
        cutoff_unix_seconds: cutoff,
        first_read: FirstLockRecoveryReadEvidenceV1 {
            read_ordinal: 1,
            observed_unix_seconds: admission.first_observed_unix_seconds,
            absence_evidence_hex: hex::encode(&admission.first_absence_evidence),
        },
        second_read: FirstLockRecoveryReadEvidenceV1 {
            read_ordinal: 2,
            observed_unix_seconds: admission.second_observed_unix_seconds,
            absence_evidence_hex: hex::encode(&admission.second_absence_evidence),
        },
        refund_chain_evidence_hex: hex::encode(refund_chain_evidence),
    };
    let encoded =
        serde_json::to_vec(&evidence).map_err(|_| ActorCommandError::ObservationUnavailable)?;
    if encoded.len() > MAX_FIRST_LOCK_ENVELOPE_BYTES {
        return Err(ActorCommandError::ObservationUnavailable);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(&encoded);
    let decoded = FirstLockRecoveryChainEvidenceV1::deserialize(&mut deserializer)
        .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    deserializer
        .end()
        .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    if decoded != evidence
        || serde_json::to_vec(&decoded).map_err(|_| ActorCommandError::ObservationUnavailable)?
            != encoded
    {
        return Err(ActorCommandError::ObservationUnavailable);
    }
    Ok(encoded)
}

/// Drives revision-one recovery only after two fresh, matching cross-chain safety reads.
async fn drive_first_lock_refund_with_observer(
    config: &ActorConfig,
    agreement: BtcAgreementV1,
    agreement_wire: Vec<u8>,
    safety: &dyn FirstLockRecoverySafetyPort,
    observer: &dyn RefundObservationPort,
) -> Result<ActorEffectOutputV1, ActorCommandError> {
    let maker_chain = agreement.coordinator().funded_chain(Participant::Maker);
    let cutoff = agreement
        .body()
        .recovery_plan()
        .maker_second_lock_cutoff_unix_seconds();
    let mut admitted_reads = Vec::with_capacity(2);
    for read_ordinal in 1_u8..=2 {
        match safety.observe(&agreement, read_ordinal).await? {
            FirstLockRecoverySafetyObservation::Uncertain {
                maker_chain: observed_chain,
            } if observed_chain == maker_chain => {
                return drive_refund_after_first_lock_safety(
                    config,
                    agreement,
                    agreement_wire,
                    None,
                    observer,
                )
                .await;
            }
            FirstLockRecoverySafetyObservation::MakerLockReady {
                chain,
                transaction_id,
                confirmations,
                chain_evidence,
            } if chain == maker_chain
                && !transaction_id.is_empty()
                && confirmations > 0
                && !chain_evidence.is_empty() =>
            {
                return drive_refund_after_first_lock_safety(
                    config,
                    agreement,
                    agreement_wire,
                    None,
                    observer,
                )
                .await;
            }
            FirstLockRecoverySafetyObservation::ReadyToRefund {
                maker_chain: observed_chain,
                cutoff_unix_seconds,
                observed_unix_seconds,
                absence_evidence,
            } if observed_chain == maker_chain
                && cutoff_unix_seconds == cutoff
                && observed_unix_seconds >= cutoff
                && !absence_evidence.is_empty()
                && absence_evidence.len() <= MAX_FIRST_LOCK_SAFETY_READ_BYTES =>
            {
                if admitted_reads
                    .last()
                    .is_some_and(|(prior, _)| observed_unix_seconds < *prior)
                {
                    return drive_refund_after_first_lock_safety(
                        config,
                        agreement,
                        agreement_wire,
                        None,
                        observer,
                    )
                    .await;
                }
                admitted_reads.push((observed_unix_seconds, absence_evidence));
            }
            _ => return Err(ActorCommandError::AgreementBindingInvalid),
        }
    }
    let [
        (first_observed_unix_seconds, first_absence_evidence),
        (second_observed_unix_seconds, second_absence_evidence),
    ] = admitted_reads
        .try_into()
        .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    if first_absence_evidence == second_absence_evidence {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    let admission = FirstLockRecoveryAdmission {
        maker_chain,
        cutoff_unix_seconds: cutoff,
        first_observed_unix_seconds,
        first_absence_evidence,
        second_observed_unix_seconds,
        second_absence_evidence,
    };
    drive_refund_after_first_lock_safety(
        config,
        agreement,
        agreement_wire,
        Some(admission),
        observer,
    )
    .await
}

fn admitted_refund_chain_evidence(
    agreement: &BtcAgreementV1,
    transition: RefundTransition,
    admission: Option<&FirstLockRecoveryAdmission>,
    chain_evidence: Vec<u8>,
) -> Result<Vec<u8>, ActorCommandError> {
    match (transition, admission) {
        (RefundTransition::FirstLockRecovery, Some(admission)) => {
            encode_first_lock_recovery_chain_evidence(agreement, admission, &chain_evidence)
        }
        (_, None) => Ok(chain_evidence),
        _ => Err(ActorCommandError::AgreementBindingInvalid),
    }
}

fn open_projection(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    agreement_wire: Vec<u8>,
) -> Result<(SqliteBtcRecoveryStore, BtcOfflineStatus), ActorCommandError> {
    let store = match open_existing_store(config, agreement, agreement_wire) {
        Ok(store) => store,
        Err(BtcRecoveryError::MissingAgreementAcceptance) => {
            return Err(ActorCommandError::NotActivated);
        }
        Err(_) => return Err(ActorCommandError::StateUnavailable),
    };
    let status = store
        .status()
        .map_err(|_| ActorCommandError::StateUnavailable)?;
    Ok((store, status))
}

/// Drives one ordered timeout revision from exact canonical refund evidence.
async fn drive_refund_after_first_lock_safety(
    config: &ActorConfig,
    agreement: BtcAgreementV1,
    agreement_wire: Vec<u8>,
    first_lock_admission: Option<FirstLockRecoveryAdmission>,
    observer: &dyn RefundObservationPort,
) -> Result<ActorEffectOutputV1, ActorCommandError> {
    if !state_file_exists(&config.state_db)? {
        return Err(ActorCommandError::NotActivated);
    }
    validate_actor_binding(config, &agreement)?;
    let (mut store, before) = open_projection(config, &agreement, agreement_wire)?;
    let Some(transition) = RefundTransition::from_status(&before) else {
        return Ok(effect_output(
            config,
            ActorEffectCommandV1::Recover,
            ActorEffectOutcomeV1::NotYetComposed {
                durable_revision: before.revision(),
            },
            &before,
        ));
    };
    let expected_chain = agreement
        .coordinator()
        .funded_chain(transition.funded_participant());
    if transition != RefundTransition::FirstLockRecovery && first_lock_admission.is_some() {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    if transition == RefundTransition::FirstLockRecovery && first_lock_admission.is_none() {
        return Ok(effect_output(
            config,
            ActorEffectCommandV1::Recover,
            ActorEffectOutcomeV1::AwaitingObservation {
                chain: expected_chain.into(),
            },
            &before,
        ));
    }
    let observation = observer.observe(&agreement, transition).await?;
    let (chain, transaction_id, confirmations, chain_evidence, position) = match observation {
        ActorRefundObservation::Pending { chain } => {
            if chain != expected_chain {
                return Err(ActorCommandError::AgreementBindingInvalid);
            }
            return Ok(effect_output(
                config,
                ActorEffectCommandV1::Recover,
                ActorEffectOutcomeV1::AwaitingObservation {
                    chain: chain.into(),
                },
                &before,
            ));
        }
        ActorRefundObservation::Ready {
            chain,
            transaction_id,
            confirmations,
            chain_evidence,
            position,
        } => {
            if chain != expected_chain
                || position.chain() != chain
                || !refund_confirmation_is_ready(&agreement, chain, confirmations)
            {
                return Err(ActorCommandError::AgreementBindingInvalid);
            }
            (
                chain,
                transaction_id,
                confirmations,
                chain_evidence,
                position,
            )
        }
    };
    let evidence = transition
        .evidence(
            chain,
            transaction_id,
            confirmations,
            admitted_refund_chain_evidence(
                &agreement,
                transition,
                first_lock_admission.as_ref(),
                chain_evidence,
            )?,
            position,
        )
        .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    project_refund_transition(
        config,
        &mut store,
        &before,
        transition,
        chain,
        expected_chain,
        &evidence,
    )
}

fn project_refund_transition(
    config: &ActorConfig,
    store: &mut SqliteBtcRecoveryStore,
    before: &BtcOfflineStatus,
    transition: RefundTransition,
    chain: Chain,
    expected_chain: Chain,
    evidence: &BtcLifecycleEvidenceV1,
) -> Result<ActorEffectOutputV1, ActorCommandError> {
    let (outcome, after) = match store.project(before.revision(), evidence) {
        Ok(commit) => {
            let after = store
                .status()
                .map_err(|_| ActorCommandError::StateUnavailable)?;
            (
                ActorEffectOutcomeV1::ObservedThenProjected {
                    chain: chain.into(),
                    was_replay: commit.was_replay(),
                },
                after,
            )
        }
        Err(BtcRecoveryError::EvidenceConflict { revision })
            if revision == transition.revision() =>
        {
            let winner = store
                .status()
                .map_err(|_| ActorCommandError::StateUnavailable)?;
            if winner.revision() != transition.revision() || winner.phase() != transition.phase() {
                return Err(ActorCommandError::ProjectionUnavailable);
            }
            (
                ActorEffectOutcomeV1::ConvergedOnExistingProjection {
                    chain: expected_chain.into(),
                    durable_revision: winner.revision(),
                },
                winner,
            )
        }
        Err(_) => return Err(ActorCommandError::ProjectionUnavailable),
    };
    Ok(effect_output(
        config,
        ActorEffectCommandV1::Recover,
        outcome,
        &after,
    ))
}

fn refund_confirmation_is_ready(
    agreement: &BtcAgreementV1,
    chain: Chain,
    confirmations: u32,
) -> bool {
    match chain {
        Chain::Bitcoin => confirmations >= agreement.required_bitcoin_confirmations(),
        Chain::Lez => confirmations == FINALIZED_LEZ_CONFIRMATION_UNITS,
        Chain::Zcash | Chain::Monero => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn claim_lifecycle_evidence(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: ClaimTransition,
    chain: Chain,
    transaction_id: Box<str>,
    confirmations: u32,
    chain_evidence: Vec<u8>,
    public_signature: Option<[u8; 64]>,
) -> Result<BtcLifecycleEvidenceV1, ActorCommandError> {
    match transition {
        ClaimTransition::RevealingClaim => {
            let signature = public_signature.ok_or(ActorCommandError::ObservationUnavailable)?;
            let claim_evidence =
                claim_evidence_from_signature(config, agreement, chain, signature)?;
            BtcLifecycleEvidenceV1::revealing_claim(
                chain,
                transaction_id,
                confirmations,
                chain_evidence,
                signature,
                claim_evidence,
            )
        }
        ClaimTransition::FollowupClaim => {
            if public_signature.is_some() {
                return Err(ActorCommandError::ObservationUnavailable);
            }
            BtcLifecycleEvidenceV1::followup_claim(
                chain,
                transaction_id,
                confirmations,
                chain_evidence,
            )
        }
    }
    .map_err(|_| ActorCommandError::ObservationUnavailable)
}

fn project_claim_transition(
    config: &ActorConfig,
    store: &mut SqliteBtcRecoveryStore,
    before: &BtcOfflineStatus,
    transition: ClaimTransition,
    chain: Chain,
    expected_chain: Chain,
    evidence: &BtcLifecycleEvidenceV1,
) -> Result<ActorEffectOutputV1, ActorCommandError> {
    let (outcome, after) = match store.project(before.revision(), evidence) {
        Ok(commit) => {
            let after = store
                .status()
                .map_err(|_| ActorCommandError::StateUnavailable)?;
            (
                ActorEffectOutcomeV1::ObservedThenProjected {
                    chain: chain.into(),
                    was_replay: commit.was_replay(),
                },
                after,
            )
        }
        Err(BtcRecoveryError::EvidenceConflict { revision })
            if revision == transition.revision() =>
        {
            let winner = store
                .status()
                .map_err(|_| ActorCommandError::StateUnavailable)?;
            if winner.revision() != transition.revision() || winner.phase() != transition.phase() {
                return Err(ActorCommandError::ProjectionUnavailable);
            }
            (
                ActorEffectOutcomeV1::ConvergedOnExistingProjection {
                    chain: expected_chain.into(),
                    durable_revision: winner.revision(),
                },
                winner,
            )
        }
        Err(_) => return Err(ActorCommandError::ProjectionUnavailable),
    };
    Ok(effect_output(
        config,
        ActorEffectCommandV1::Drive,
        outcome,
        &after,
    ))
}

fn claim_confirmation_is_ready(
    agreement: &BtcAgreementV1,
    chain: Chain,
    confirmations: u32,
) -> bool {
    match chain {
        Chain::Bitcoin => confirmations >= agreement.required_bitcoin_confirmations(),
        Chain::Lez => confirmations == FINALIZED_LEZ_CONFIRMATION_UNITS,
        Chain::Zcash | Chain::Monero => false,
    }
}

fn claim_evidence_from_signature(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    chain: Chain,
    public_signature: [u8; 64],
) -> Result<ClaimEvidence, ActorCommandError> {
    let (domain, signing) = match chain {
        Chain::Bitcoin => (BtcAdaptorSessionDomain::Bitcoin, &config.signing.bitcoin),
        Chain::Lez => (BtcAdaptorSessionDomain::Lez, &config.signing.lez),
        Chain::Zcash | Chain::Monero => {
            return Err(ActorCommandError::AgreementBindingInvalid);
        }
    };
    let context = agreement
        .adaptor_session_context(domain, *signing.session_id.as_bytes())
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?;
    let journal = SqliteAdaptorSessionJournal::open_existing(&signing.journal_db)
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?;
    let snapshot = journal
        .load(signing.session_id.as_bytes())
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?
        .ok_or(ActorCommandError::ActivationMaterialUnavailable)?;
    let presignature = snapshot
        .presignature()
        .ok_or(ActorCommandError::ActivationMaterialUnavailable)?;
    match config.role {
        ActorRole::Taker => {
            let secret_path = config
                .signing
                .adaptor_secret_file
                .as_ref()
                .ok_or(ActorCommandError::ActivationMaterialUnavailable)?;
            let secret = read_private_adaptor_secret(secret_path)
                .map_err(|()| ActorCommandError::ActivationMaterialUnavailable)?;
            let claim_evidence = ClaimEvidence::new(*secret);
            let expected = adapt_presignature(&context, *presignature.bytes(), secret)
                .map_err(|_| ActorCommandError::ObservationUnavailable)?;
            if expected != public_signature {
                return Err(ActorCommandError::ObservationUnavailable);
            }
            Ok(claim_evidence)
        }
        ActorRole::Maker => {
            let secret = extract_adaptor_secret(&context, *presignature.bytes(), public_signature)
                .map_err(|_| ActorCommandError::ObservationUnavailable)?;
            Ok(ClaimEvidence::new(*secret))
        }
    }
}

fn prepare_bitcoin_refund_effect(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: RefundTransition,
    before: &BtcOfflineStatus,
) -> Result<Option<PreparedBitcoinRefundEffect>, ActorCommandError> {
    if before.revision() != transition.predecessor_revision()
        || agreement
            .coordinator()
            .funded_chain(transition.funded_participant())
            != Chain::Bitcoin
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    if config.role.sdk() != transition.funded_participant() {
        return Ok(None);
    }
    validate_bitcoin_refund_authority(config, agreement)?;
    let path = config
        .refund
        .bitcoin_refund_key_file
        .as_ref()
        .ok_or(ActorCommandError::ActivationMaterialUnavailable)?;
    let secret = read_private_adaptor_secret(path)
        .map_err(|()| ActorCommandError::ActivationMaterialUnavailable)?;
    let secret = SecretKey::from_slice(secret.as_ref())
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?;
    let secp = Secp256k1::new();
    let signature = secp
        .sign_schnorr_no_aux_rand(
            &Message::from_digest(agreement.bitcoin_refund().sighash_bytes()),
            &Keypair::from_secret_key(&secp, &secret),
        )
        .serialize();
    let transaction = agreement
        .bitcoin_refund()
        .clone()
        .finalize(signature)
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?;
    let expected_transaction_id = transaction.compute_txid();
    let expected_witness_transaction_id = transaction.compute_wtxid();
    let swap_id = SwapId::new(hex::encode(agreement.body().swap_id()))
        .map_err(|_| ActorCommandError::AgreementBindingInvalid)?;
    let key = PublicEffectKey::new(
        swap_id,
        config.role.sdk(),
        PublicEffectChain::Bitcoin,
        PublicEffectOperation::Refund,
        transition.predecessor_revision(),
    );
    let effect = PreparedPublicEffect::new(
        key,
        *agreement.agreement_commitment(),
        expected_transaction_id.to_string(),
        serialize(&transaction),
    )
    .map_err(|_| ActorCommandError::AgreementBindingInvalid)?;
    Ok(Some(PreparedBitcoinRefundEffect {
        effect,
        expected_transaction_id,
        expected_witness_transaction_id,
    }))
}

fn prepare_bitcoin_claim_effect(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: ClaimTransition,
    before: &BtcOfflineStatus,
) -> Result<Option<PreparedBitcoinClaimEffect>, ActorCommandError> {
    if before.revision() != transition.predecessor_revision()
        || agreement
            .coordinator()
            .funded_chain(transition.funded_participant())
            != Chain::Bitcoin
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    if config.role.sdk() != transition.submitter() {
        return Ok(None);
    }

    let secret = match transition {
        ClaimTransition::RevealingClaim => {
            let path = config
                .signing
                .adaptor_secret_file
                .as_ref()
                .ok_or(ActorCommandError::ActivationMaterialUnavailable)?;
            read_private_adaptor_secret(path)
                .map_err(|()| ActorCommandError::ActivationMaterialUnavailable)?
        }
        ClaimTransition::FollowupClaim => {
            let public_signature: [u8; 64] = before
                .revealing_public_witness()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or(ActorCommandError::ActivationMaterialUnavailable)?;
            let revealing_chain = agreement
                .coordinator()
                .funded_chain(ClaimTransition::RevealingClaim.funded_participant());
            if revealing_chain == Chain::Bitcoin {
                return Err(ActorCommandError::AgreementBindingInvalid);
            }
            let (context, presignature) =
                verified_chain_presignature(config, agreement, revealing_chain)?;
            extract_adaptor_secret(&context, presignature, public_signature)
                .map_err(|_| ActorCommandError::ObservationUnavailable)?
        }
    };
    let (context, presignature) = verified_chain_presignature(config, agreement, Chain::Bitcoin)?;
    let public_signature = adapt_presignature(&context, presignature, secret)
        .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    let transaction = agreement
        .cooperative_claim()
        .clone()
        .finalize(public_signature)
        .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    let expected_transaction_id = transaction.compute_txid();
    let exact_transaction_bytes = serialize(&transaction);
    let swap_id = SwapId::new(hex::encode(agreement.body().swap_id()))
        .map_err(|_| ActorCommandError::AgreementBindingInvalid)?;
    let key = PublicEffectKey::new(
        swap_id,
        config.role.sdk(),
        PublicEffectChain::Bitcoin,
        PublicEffectOperation::Claim,
        transition.predecessor_revision(),
    );
    let effect = PreparedPublicEffect::new(
        key,
        *agreement.agreement_commitment(),
        expected_transaction_id.to_string(),
        exact_transaction_bytes,
    )
    .map_err(|_| ActorCommandError::AgreementBindingInvalid)?;
    Ok(Some(PreparedBitcoinClaimEffect {
        effect,
        expected_transaction_id,
    }))
}

async fn prepare_lez_claim_effect<P>(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: ClaimTransition,
    before: &BtcOfflineStatus,
    chain: &P,
) -> Result<Option<PreparedLezClaimEffect>, ActorCommandError>
where
    P: LezClaimChainPort,
{
    if before.revision() != transition.predecessor_revision()
        || agreement
            .coordinator()
            .funded_chain(transition.funded_participant())
            != Chain::Lez
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    if config.role.sdk() != transition.submitter() {
        return Ok(None);
    }
    let prepared = load_prepared_witnessed_claim(config, agreement)?;
    if prepared.context.sidecar_role != config.role.bridge() {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }

    let adaptor_secret = match transition {
        ClaimTransition::RevealingClaim => {
            let path = config
                .signing
                .adaptor_secret_file
                .as_ref()
                .ok_or(ActorCommandError::ActivationMaterialUnavailable)?;
            read_private_adaptor_secret(path)
                .map_err(|()| ActorCommandError::ActivationMaterialUnavailable)?
        }
        ClaimTransition::FollowupClaim => {
            let public_signature: [u8; 64] = before
                .revealing_public_witness()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or(ActorCommandError::ActivationMaterialUnavailable)?;
            let revealing_chain = agreement
                .coordinator()
                .funded_chain(ClaimTransition::RevealingClaim.funded_participant());
            if revealing_chain != Chain::Bitcoin {
                return Err(ActorCommandError::AgreementBindingInvalid);
            }
            let (context, presignature) =
                verified_chain_presignature(config, agreement, Chain::Bitcoin)?;
            // Extraction verifies the final Bitcoin signature and point-checks
            // the recovered scalar against the signed agreement adaptor point.
            extract_adaptor_secret(&context, presignature, public_signature)
                .map_err(|_| ActorCommandError::ObservationUnavailable)?
        }
    };
    let (context, presignature) = verified_chain_presignature(config, agreement, Chain::Lez)?;
    let aggregate_signature = adapt_presignature(&context, presignature, adaptor_secret)
        .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    verify_final_signature(&context, aggregate_signature)
        .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    let request = complete_lez_claim_request(
        config,
        agreement,
        transition,
        prepared.claim,
        aggregate_signature,
    )?;
    let expected_context = request.context.clone();
    let completed = chain.complete_witnessed_claim(request).await?;
    if completed.context != expected_context
        || completed.claim.transaction_id.as_bytes() == &[0; 32]
        || completed.claim.exact_bytes.as_slice().is_empty()
    {
        return Err(ActorCommandError::ObservationUnavailable);
    }
    let expected_effect_id = hex::encode(completed.claim.transaction_id.as_bytes());
    let swap_id = SwapId::new(hex::encode(agreement.body().swap_id()))
        .map_err(|_| ActorCommandError::AgreementBindingInvalid)?;
    let key = PublicEffectKey::new(
        swap_id,
        config.role.sdk(),
        PublicEffectChain::Lez,
        PublicEffectOperation::Claim,
        transition.predecessor_revision(),
    );
    let effect = PreparedPublicEffect::new(
        key,
        *agreement.agreement_commitment(),
        expected_effect_id,
        completed.claim.exact_bytes.as_slice().to_vec(),
    )
    .map_err(|_| ActorCommandError::AgreementBindingInvalid)?;
    Ok(Some(PreparedLezClaimEffect {
        effect,
        transaction: completed.claim,
        aggregate_signature,
    }))
}

fn complete_lez_claim_request(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: ClaimTransition,
    claim: PreparedWitnessedClaim,
    aggregate_signature: [u8; 64],
) -> Result<CompleteWitnessedClaimRequest, ActorCommandError> {
    let aggregate_signature = AggregateBip340Signature::from_bytes(aggregate_signature);
    let identity = CompleteLezClaimRequestIdentityV1 {
        schema_version: 1,
        operation: "complete_witnessed_claim",
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        transition,
        run_id: &config.lez_bridge.run_id,
        sidecar_role: config.role.bridge(),
        runtime: &config.lez_bridge.runtime,
        claim: &claim,
        aggregate_signature: &aggregate_signature,
    };
    let request_id = deterministic_request_id(&identity)?;
    Ok(CompleteWitnessedClaimRequest::new(
        MessageContext::new(
            config.lez_bridge.run_id.clone(),
            request_id,
            config.role.bridge(),
        ),
        config.lez_bridge.runtime.clone(),
        claim,
        aggregate_signature,
    ))
}

#[derive(Serialize)]
struct CompleteLezClaimRequestIdentityV1<'a> {
    schema_version: u16,
    operation: &'static str,
    agreement_commitment: String,
    transition: ClaimTransition,
    run_id: &'a RunId,
    sidecar_role: BridgeParticipant,
    runtime: &'a RuntimeDescriptor,
    claim: &'a PreparedWitnessedClaim,
    aggregate_signature: &'a AggregateBip340Signature,
}

fn deterministic_request_id(value: &impl Serialize) -> Result<RequestId, ActorCommandError> {
    let encoded =
        serde_json::to_vec(value).map_err(|_| ActorCommandError::ConfigurationUnavailable)?;
    RequestId::new(hex::encode(Sha256::digest(encoded)))
        .map_err(|_| ActorCommandError::ConfigurationUnavailable)
}

fn verified_chain_presignature(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    chain: Chain,
) -> Result<(AdaptorSessionContext, [u8; 65]), ActorCommandError> {
    let (domain, signing) = match chain {
        Chain::Bitcoin => (BtcAdaptorSessionDomain::Bitcoin, &config.signing.bitcoin),
        Chain::Lez => (BtcAdaptorSessionDomain::Lez, &config.signing.lez),
        Chain::Zcash | Chain::Monero => {
            return Err(ActorCommandError::AgreementBindingInvalid);
        }
    };
    validate_signer_journal(config, agreement, domain, signing)?;
    let context = agreement
        .adaptor_session_context(domain, *signing.session_id.as_bytes())
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?;
    let journal = SqliteAdaptorSessionJournal::open_existing(&signing.journal_db)
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?;
    let snapshot = journal
        .load(signing.session_id.as_bytes())
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?
        .ok_or(ActorCommandError::ActivationMaterialUnavailable)?;
    let presignature = snapshot
        .presignature()
        .ok_or(ActorCommandError::ActivationMaterialUnavailable)?;
    Ok((context, *presignature.bytes()))
}

struct BitcoinFundingObserver<R> {
    adapter: BitcoinCoreAdapter<R>,
}

#[async_trait]
impl<R> FundingObservationPort for BitcoinFundingObserver<R>
where
    R: BitcoinCoreRpc + Send + Sync,
{
    async fn observe(
        &self,
        agreement: &BtcAgreementV1,
        _transition: FundingTransition,
    ) -> Result<ActorFundingObservation, ActorCommandError> {
        let observed = self
            .adapter
            .observe_funding(agreement)
            .await
            .map_err(|_| ActorCommandError::ObservationUnavailable)?;
        let FundingObservation::Ready(observed) = observed else {
            return Ok(ActorFundingObservation::Pending {
                chain: Chain::Bitcoin,
            });
        };
        let evidence = BitcoinCoreEvidenceV1::funding_ready(agreement, &observed)
            .and_then(|value| value.encode())
            .map_err(|_| ActorCommandError::ObservationUnavailable)?;
        Ok(ActorFundingObservation::Ready {
            chain: Chain::Bitcoin,
            transaction_id: observed
                .transaction()
                .compute_txid()
                .to_string()
                .into_boxed_str(),
            confirmations: observed.confirmations(),
            chain_evidence: evidence,
        })
    }
}

#[async_trait]
impl<R> BitcoinClaimChainPort for BitcoinCoreAdapter<R>
where
    R: BitcoinCoreRpc + Send + Sync,
{
    async fn observe_claim(&self, agreement: &BtcAgreementV1) -> BitcoinClaimScan {
        let Ok(observation) = BitcoinCoreAdapter::observe_claim(self, agreement).await else {
            return BitcoinClaimScan::Uncertain;
        };
        let (claim, finalized) = match &observation {
            BitcoinClaimObservation::Unspent => return BitcoinClaimScan::Unspent,
            BitcoinClaimObservation::Revealed(claim)
            | BitcoinClaimObservation::Confirming(claim) => (claim, false),
            BitcoinClaimObservation::Finalized(claim) => (claim, true),
        };
        let Ok(evidence) = BitcoinCoreEvidenceV1::claim(agreement, &observation) else {
            return BitcoinClaimScan::Uncertain;
        };
        let Some(public_signature) = evidence.claim_public_witness().copied() else {
            return BitcoinClaimScan::Uncertain;
        };
        let Ok(chain_evidence) = evidence.encode() else {
            return BitcoinClaimScan::Uncertain;
        };
        BitcoinClaimScan::Exact(BitcoinExactClaim {
            transaction_bytes: serialize(claim.transaction()),
            transaction_id: claim.transaction_id().to_string().into_boxed_str(),
            confirmations: claim.confirmations(),
            chain_evidence,
            public_signature,
            finalized,
        })
    }

    async fn submit_authorized_claim(
        &self,
        agreement: &BtcAgreementV1,
        transaction_bytes: &[u8],
        expected_transaction_id: Txid,
    ) -> Result<AuthorizedClaimSubmission, ActorCommandError> {
        BitcoinCoreAdapter::submit_authorized_claim(
            self,
            agreement,
            transaction_bytes,
            expected_transaction_id,
        )
        .await
        .map_err(|_| ActorCommandError::ObservationUnavailable)
    }
}

#[async_trait]
impl<R> BitcoinRefundChainPort for BitcoinCoreAdapter<R>
where
    R: BitcoinCoreRpc + Send + Sync,
{
    async fn observe_refund(&self, agreement: &BtcAgreementV1) -> BitcoinRefundScan {
        let Ok(observation) = BitcoinCoreAdapter::observe_refund(self, agreement).await else {
            return BitcoinRefundScan::Uncertain;
        };
        let (observed, finalized) = match &observation {
            BitcoinRefundObservation::Immature(_) => return BitcoinRefundScan::Immature,
            BitcoinRefundObservation::Eligible(_) => return BitcoinRefundScan::Eligible,
            BitcoinRefundObservation::ConflictingSpend => {
                return BitcoinRefundScan::Conflicting;
            }
            BitcoinRefundObservation::Revealed(observed)
            | BitcoinRefundObservation::Confirming(observed) => (observed, false),
            BitcoinRefundObservation::Finalized(observed) => (observed, true),
        };
        let chain_evidence = if finalized {
            let Ok(evidence) = BitcoinCoreEvidenceV1::refund_finalized(agreement, &observation)
                .and_then(|value| value.encode())
            else {
                return BitcoinRefundScan::Uncertain;
            };
            evidence
        } else {
            Vec::new()
        };
        BitcoinRefundScan::Exact(BitcoinExactRefund {
            transaction_bytes: serialize(observed.transaction()),
            transaction_id: observed.transaction_id().to_string().into_boxed_str(),
            witness_transaction_id: observed
                .transaction()
                .compute_wtxid()
                .to_string()
                .into_boxed_str(),
            confirmations: observed.confirmations(),
            block_height: observed.block_height(),
            chain_evidence,
            finalized,
        })
    }

    async fn submit_authorized_refund(
        &self,
        agreement: &BtcAgreementV1,
        transaction_bytes: &[u8],
        expected_transaction_id: Txid,
    ) -> Result<AuthorizedRefundSubmission, ActorCommandError> {
        BitcoinCoreAdapter::submit_authorized_refund(
            self,
            agreement,
            transaction_bytes,
            expected_transaction_id,
        )
        .await
        .map_err(|_| ActorCommandError::ObservationUnavailable)
    }
}

struct BitcoinRefundObserver<P> {
    chain: P,
    effect: Option<PreparedBitcoinRefundEffect>,
    state_db: PathBuf,
}

#[async_trait]
impl<P> RefundObservationPort for BitcoinRefundObserver<P>
where
    P: BitcoinRefundChainPort,
{
    async fn observe(
        &self,
        agreement: &BtcAgreementV1,
        transition: RefundTransition,
    ) -> Result<ActorRefundObservation, ActorCommandError> {
        if let Some(effect) = &self.effect {
            validate_prepared_bitcoin_refund_effect(effect, transition)?;
            let mut journal = SqlitePublicEffectJournal::open(&self.state_db)
                .map_err(|_| ActorCommandError::StateUnavailable)?;
            let _ = journal
                .record_prepared(&effect.effect)
                .map_err(|_| ActorCommandError::StateUnavailable)?;
        }
        let scan = self.chain.observe_refund(agreement).await;
        if let Some(effect) = &self.effect {
            self.reconcile_and_maybe_submit(agreement, effect, &scan)
                .await?;
        }
        let BitcoinRefundScan::Exact(exact) = scan else {
            return Ok(ActorRefundObservation::Pending {
                chain: Chain::Bitcoin,
            });
        };
        if !exact.finalized
            || !bitcoin_exact_refund_matches(agreement, &exact, self.effect.as_ref())
        {
            return Ok(ActorRefundObservation::Pending {
                chain: Chain::Bitcoin,
            });
        }
        let block_height = exact
            .block_height
            .ok_or(ActorCommandError::ObservationUnavailable)?;
        let position = SwapChainPosition::block_height(Chain::Bitcoin, u64::from(block_height));
        Ok(ActorRefundObservation::Ready {
            chain: Chain::Bitcoin,
            transaction_id: exact.transaction_id,
            confirmations: exact.confirmations,
            chain_evidence: exact.chain_evidence,
            position,
        })
    }
}

impl<P> BitcoinRefundObserver<P>
where
    P: BitcoinRefundChainPort,
{
    async fn reconcile_and_maybe_submit(
        &self,
        agreement: &BtcAgreementV1,
        effect: &PreparedBitcoinRefundEffect,
        scan: &BitcoinRefundScan,
    ) -> Result<(), ActorCommandError> {
        let observation = match scan {
            BitcoinRefundScan::Eligible => PublicEffectObservation::EligibleToAttempt,
            BitcoinRefundScan::Exact(exact)
                if bitcoin_exact_refund_matches(agreement, exact, Some(effect)) =>
            {
                PublicEffectObservation::PresentExact(exact.transaction_bytes.clone())
            }
            BitcoinRefundScan::Exact(_) | BitcoinRefundScan::Conflicting => {
                PublicEffectObservation::ConflictingPresence
            }
            BitcoinRefundScan::Immature | BitcoinRefundScan::Uncertain => {
                PublicEffectObservation::Uncertain
            }
        };
        let mut journal = SqlitePublicEffectJournal::open(&self.state_db)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        let decision = journal
            .reconcile(effect.effect.key(), observation)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        let PublicEffectDecision::SubmitOnce(_) = decision else {
            return Ok(());
        };
        drop(journal);

        let submission = self
            .chain
            .submit_authorized_refund(
                agreement,
                effect.effect.exact_public_bytes(),
                effect.expected_transaction_id,
            )
            .await;
        let (result, deferred_error) = match submission {
            Ok(AuthorizedRefundSubmission::Accepted {
                transaction_id,
                witness_transaction_id,
            }) if transaction_id == effect.expected_transaction_id
                && witness_transaction_id == effect.expected_witness_transaction_id =>
            {
                (
                    PublicEffectSubmissionResult::Accepted(
                        transaction_id.to_string().into_boxed_str(),
                    ),
                    None,
                )
            }
            Ok(AuthorizedRefundSubmission::Accepted { .. }) => (
                PublicEffectSubmissionResult::Unknown,
                Some(ActorCommandError::AgreementBindingInvalid),
            ),
            Ok(AuthorizedRefundSubmission::Rejected) => {
                (PublicEffectSubmissionResult::Rejected, None)
            }
            Ok(AuthorizedRefundSubmission::Unknown) => {
                (PublicEffectSubmissionResult::Unknown, None)
            }
            Err(error) => (PublicEffectSubmissionResult::Unknown, Some(error)),
        };
        let mut journal = SqlitePublicEffectJournal::open(&self.state_db)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        let _ = journal
            .record_submission_result(effect.effect.key(), &result)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        if let Some(error) = deferred_error {
            return Err(error);
        }
        Ok(())
    }
}

fn bitcoin_exact_refund_matches(
    agreement: &BtcAgreementV1,
    exact: &BitcoinExactRefund,
    effect: Option<&PreparedBitcoinRefundEffect>,
) -> bool {
    let Ok(transaction) = deserialize::<bitcoin::Transaction>(&exact.transaction_bytes) else {
        return false;
    };
    let transaction_id = transaction.compute_txid();
    let witness_transaction_id = transaction.compute_wtxid();
    serialize(&transaction) == exact.transaction_bytes
        && transaction_id
            == agreement
                .bitcoin_refund()
                .unsigned_transaction()
                .compute_txid()
        && exact.transaction_id.as_ref() == transaction_id.to_string().as_str()
        && exact.witness_transaction_id.as_ref() == witness_transaction_id.to_string().as_str()
        && effect.is_none_or(|expected| {
            transaction_id == expected.expected_transaction_id
                && witness_transaction_id == expected.expected_witness_transaction_id
                && exact.transaction_bytes.as_slice() == expected.effect.exact_public_bytes()
        })
}

fn validate_prepared_bitcoin_refund_effect(
    effect: &PreparedBitcoinRefundEffect,
    transition: RefundTransition,
) -> Result<(), ActorCommandError> {
    if effect.effect.key().local_role() != transition.funded_participant()
        || effect.effect.key().chain() != PublicEffectChain::Bitcoin
        || effect.effect.key().operation() != PublicEffectOperation::Refund
        || effect.effect.key().predecessor_revision() != transition.predecessor_revision()
        || effect.effect.expected_effect_id() != effect.expected_transaction_id.to_string()
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    Ok(())
}

#[derive(Serialize)]
struct LezRefundRequestIdentityV1 {
    schema_version: u16,
    operation: String,
    agreement_commitment: String,
    transition: RefundTransition,
    run_id: RunId,
    sidecar_role: BridgeParticipant,
    runtime: RuntimeDescriptor,
    terms: WitnessedNativeEscrowTerms,
    target: Option<NativeRefundObservationTarget>,
    transaction: Option<PreparedTransaction>,
}

fn prepare_lez_refund_request(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: RefundTransition,
) -> Result<PrepareNativeRefundRequest, ActorCommandError> {
    validate_lez_refund_transition(config, agreement, transition)?;
    if config.role.sdk() != transition.funded_participant() {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    let terms = witnessed_lez_terms(agreement)?;
    let identity = LezRefundRequestIdentityV1 {
        schema_version: 1,
        operation: "prepare_native_refund".into(),
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        transition,
        run_id: config.lez_bridge.run_id.clone(),
        sidecar_role: config.role.bridge(),
        runtime: config.lez_bridge.runtime.clone(),
        terms: terms.clone(),
        target: None,
        transaction: None,
    };
    let request_id = deterministic_request_id(&identity)?;
    Ok(PrepareNativeRefundRequest::new_witnessed(
        MessageContext::new(
            config.lez_bridge.run_id.clone(),
            request_id,
            config.role.bridge(),
        ),
        config.lez_bridge.runtime.clone(),
        terms,
    ))
}

fn lez_refund_observation_request(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: RefundTransition,
    target: NativeRefundObservationTarget,
) -> Result<ObserveNativeRefundRequest, ActorCommandError> {
    validate_lez_refund_transition(config, agreement, transition)?;
    let owner = config.role.sdk() == transition.funded_participant();
    let target_is_valid = matches!(
        (owner, target),
        (
            true,
            NativeRefundObservationTarget::StateOnly | NativeRefundObservationTarget::Exact { .. }
        ) | (false, NativeRefundObservationTarget::DiscoverByTerms { .. })
    );
    if !target_is_valid {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    let terms = witnessed_lez_terms(agreement)?;
    let identity = LezRefundRequestIdentityV1 {
        schema_version: 1,
        operation: "observe_native_refund".into(),
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        transition,
        run_id: config.lez_bridge.run_id.clone(),
        sidecar_role: config.role.bridge(),
        runtime: config.lez_bridge.runtime.clone(),
        terms: terms.clone(),
        target: Some(target),
        transaction: None,
    };
    let request_id = deterministic_request_id(&identity)?;
    Ok(ObserveNativeRefundRequest::new_witnessed(
        MessageContext::new(
            config.lez_bridge.run_id.clone(),
            request_id,
            config.role.bridge(),
        ),
        config.lez_bridge.runtime.clone(),
        terms,
        target,
    ))
}

fn submit_lez_refund_request(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: RefundTransition,
    effect: &PreparedLezRefundEffect,
) -> Result<SubmitTransactionRequest, ActorCommandError> {
    validate_lez_refund_transition(config, agreement, transition)?;
    validate_prepared_lez_refund_effect(effect, transition)?;
    if config.role.sdk() != transition.funded_participant() {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    let terms = witnessed_lez_terms(agreement)?;
    let identity = LezRefundRequestIdentityV1 {
        schema_version: 1,
        operation: "submit_transaction".into(),
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        transition,
        run_id: config.lez_bridge.run_id.clone(),
        sidecar_role: config.role.bridge(),
        runtime: config.lez_bridge.runtime.clone(),
        terms,
        target: None,
        transaction: Some(effect.transaction.clone()),
    };
    let request_id = deterministic_request_id(&identity)?;
    Ok(SubmitTransactionRequest::new(
        MessageContext::new(
            config.lez_bridge.run_id.clone(),
            request_id,
            config.role.bridge(),
        ),
        config.lez_bridge.runtime.clone(),
        effect.transaction.clone(),
    ))
}

struct LezRefundObserver<P> {
    config: ActorConfig,
    chain: P,
    state_db: PathBuf,
}

#[async_trait]
impl<P> RefundObservationPort for LezRefundObserver<P>
where
    P: LezRefundChainPort,
{
    async fn observe(
        &self,
        agreement: &BtcAgreementV1,
        transition: RefundTransition,
    ) -> Result<ActorRefundObservation, ActorCommandError> {
        validate_lez_refund_transition(&self.config, agreement, transition)?;
        let owner = self.config.role.sdk() == transition.funded_participant();
        if !owner {
            let request = lez_refund_observation_request(
                &self.config,
                agreement,
                transition,
                NativeRefundObservationTarget::DiscoverByTerms {
                    window: self.config.discovery_window()?,
                },
            )?;
            let response = self.chain.observe_native_refund(request.clone()).await?;
            validate_lez_refund_response(&self.config, agreement, transition, &request, &response)?;
            return finalized_lez_refund_observation(
                &self.config,
                agreement,
                transition,
                None,
                &request,
                &response,
            );
        }

        let state_request = lez_refund_observation_request(
            &self.config,
            agreement,
            transition,
            NativeRefundObservationTarget::StateOnly,
        )?;
        let state_response = self
            .chain
            .observe_native_refund(state_request.clone())
            .await?;
        let state = validate_lez_refund_response(
            &self.config,
            agreement,
            transition,
            &state_request,
            &state_response,
        )?;
        if state_response.refund != NativeRefundObservation::NotRequested {
            return Err(ActorCommandError::AgreementBindingInvalid);
        }
        if state != Some(EscrowState::Funded) && state != Some(EscrowState::Refunded) {
            return Ok(ActorRefundObservation::Pending { chain: Chain::Lez });
        }
        if state == Some(EscrowState::Funded)
            && state_response.clock_after.timestamp_ms < agreement.lez_terms().refund_at_ms()
        {
            return Ok(ActorRefundObservation::Pending { chain: Chain::Lez });
        }

        let effect =
            prepare_lez_refund_effect(&self.config, agreement, transition, &self.chain).await?;
        validate_prepared_lez_refund_effect(&effect, transition)?;
        let mut journal = SqlitePublicEffectJournal::open(&self.state_db)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        let _ = journal
            .record_prepared(&effect.effect)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        drop(journal);

        let request = lez_refund_observation_request(
            &self.config,
            agreement,
            transition,
            NativeRefundObservationTarget::Exact {
                refund_transaction_id: effect.transaction.transaction_id,
                window: self.config.discovery_window()?,
            },
        )?;
        let response = self.chain.observe_native_refund(request.clone()).await?;
        validate_monotonic_lez_clocks(state_response.clock_after, response.clock_after)?;
        let account_state =
            validate_lez_refund_response(&self.config, agreement, transition, &request, &response)?;
        self.reconcile_and_maybe_submit(agreement, transition, &effect, account_state, &response)
            .await?;
        finalized_lez_refund_observation(
            &self.config,
            agreement,
            transition,
            Some(&effect),
            &request,
            &response,
        )
    }
}

impl<P> LezRefundObserver<P>
where
    P: LezRefundChainPort,
{
    async fn reconcile_and_maybe_submit(
        &self,
        agreement: &BtcAgreementV1,
        transition: RefundTransition,
        effect: &PreparedLezRefundEffect,
        account_state: Option<EscrowState>,
        response: &ObserveNativeRefundResult,
    ) -> Result<(), ActorCommandError> {
        let observation = match &response.refund {
            NativeRefundObservation::Found(found)
                if found.transaction.transaction_id == effect.transaction.transaction_id
                    && found.transaction.exact_bytes.as_slice()
                        == effect.effect.exact_public_bytes() =>
            {
                PublicEffectObservation::PresentExact(
                    found.transaction.exact_bytes.as_slice().to_vec(),
                )
            }
            NativeRefundObservation::Found(_) => PublicEffectObservation::ConflictingPresence,
            NativeRefundObservation::Absent | NativeRefundObservation::UnknownOrPending
                if account_state == Some(EscrowState::Funded)
                    && response.clock_after.timestamp_ms
                        >= agreement.lez_terms().refund_at_ms() =>
            {
                PublicEffectObservation::EligibleToAttempt
            }
            NativeRefundObservation::Absent | NativeRefundObservation::UnknownOrPending => {
                PublicEffectObservation::Uncertain
            }
            NativeRefundObservation::NotRequested => {
                return Err(ActorCommandError::AgreementBindingInvalid);
            }
        };
        let mut journal = SqlitePublicEffectJournal::open(&self.state_db)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        let decision = journal
            .reconcile(effect.effect.key(), observation)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        let PublicEffectDecision::SubmitOnce(_) = decision else {
            return Ok(());
        };
        drop(journal);

        let request = submit_lez_refund_request(&self.config, agreement, transition, effect)?;
        let expected_context = request.context.clone();
        let submission = self.chain.submit_transaction(request).await;
        let (result, deferred_error) = match submission {
            Ok(response)
                if response.context == expected_context
                    && response.transaction_id == effect.transaction.transaction_id
                    && matches!(
                        response.outcome,
                        SubmissionOutcome::Accepted | SubmissionOutcome::AlreadyKnown
                    ) =>
            {
                (
                    PublicEffectSubmissionResult::Accepted(
                        hex::encode(response.transaction_id.as_bytes()).into_boxed_str(),
                    ),
                    None,
                )
            }
            Ok(_) => (
                PublicEffectSubmissionResult::Unknown,
                Some(ActorCommandError::AgreementBindingInvalid),
            ),
            Err(error) => (PublicEffectSubmissionResult::Unknown, Some(error)),
        };
        let mut journal = SqlitePublicEffectJournal::open(&self.state_db)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        let _ = journal
            .record_submission_result(effect.effect.key(), &result)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        if let Some(error) = deferred_error {
            return Err(error);
        }
        Ok(())
    }
}

async fn prepare_lez_refund_effect<P>(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: RefundTransition,
    chain: &P,
) -> Result<PreparedLezRefundEffect, ActorCommandError>
where
    P: LezRefundChainPort,
{
    validate_lez_refund_transition(config, agreement, transition)?;
    if config.role.sdk() != transition.funded_participant() {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    let request = prepare_lez_refund_request(config, agreement, transition)?;
    let expected_context = request.context.clone();
    let response = chain.prepare_native_refund(request).await?;
    if response.context != expected_context {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    let swap_id = SwapId::new(hex::encode(agreement.body().swap_id()))
        .map_err(|_| ActorCommandError::AgreementBindingInvalid)?;
    let key = PublicEffectKey::new(
        swap_id,
        config.role.sdk(),
        PublicEffectChain::Lez,
        PublicEffectOperation::Refund,
        transition.predecessor_revision(),
    );
    let effect = PreparedPublicEffect::new(
        key,
        *agreement.agreement_commitment(),
        hex::encode(response.refund.transaction_id.as_bytes()),
        response.refund.exact_bytes.as_slice().to_vec(),
    )
    .map_err(|_| ActorCommandError::AgreementBindingInvalid)?;
    let prepared = PreparedLezRefundEffect {
        effect,
        transaction: response.refund,
    };
    validate_prepared_lez_refund_effect(&prepared, transition)?;
    Ok(prepared)
}

fn validate_prepared_lez_refund_effect(
    effect: &PreparedLezRefundEffect,
    transition: RefundTransition,
) -> Result<(), ActorCommandError> {
    if effect.effect.key().local_role() != transition.funded_participant()
        || effect.effect.key().chain() != PublicEffectChain::Lez
        || effect.effect.key().operation() != PublicEffectOperation::Refund
        || effect.effect.key().predecessor_revision() != transition.predecessor_revision()
        || effect.effect.expected_effect_id()
            != hex::encode(effect.transaction.transaction_id.as_bytes())
        || effect.effect.exact_public_bytes() != effect.transaction.exact_bytes.as_slice()
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    Ok(())
}

fn validate_lez_refund_transition(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: RefundTransition,
) -> Result<(), ActorCommandError> {
    validate_actor_binding(config, agreement)?;
    if agreement
        .coordinator()
        .funded_chain(transition.funded_participant())
        != Chain::Lez
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    Ok(())
}

fn validate_monotonic_lez_clocks(
    before: ChainClock,
    after: ChainClock,
) -> Result<(), ActorCommandError> {
    if after.height < before.height
        || after.timestamp_ms < before.timestamp_ms
        || (after.height == before.height && after.block_hash != before.block_hash)
    {
        return Err(ActorCommandError::ObservationUnavailable);
    }
    Ok(())
}

fn validate_lez_refund_response(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: RefundTransition,
    request: &ObserveNativeRefundRequest,
    response: &ObserveNativeRefundResult,
) -> Result<Option<EscrowState>, ActorCommandError> {
    validate_lez_refund_transition(config, agreement, transition)?;
    if response.context != request.context || response.clock_before != response.clock_after {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    let expected_request =
        lez_refund_observation_request(config, agreement, transition, request.target)?;
    if request != &expected_request {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    let terms = request
        .terms
        .witnessed()
        .ok_or(ActorCommandError::AgreementBindingInvalid)?;
    let state = validate_lez_refund_accounts(config, agreement, terms, &response.accounts)?
        .ok_or(ActorCommandError::AgreementBindingInvalid)?;
    match &response.refund {
        NativeRefundObservation::NotRequested => {
            if request.target != NativeRefundObservationTarget::StateOnly {
                return Err(ActorCommandError::AgreementBindingInvalid);
            }
        }
        NativeRefundObservation::Absent => {
            if !matches!(
                request.target,
                NativeRefundObservationTarget::DiscoverByTerms { .. }
            ) {
                return Err(ActorCommandError::AgreementBindingInvalid);
            }
        }
        NativeRefundObservation::UnknownOrPending => {
            if request.target == NativeRefundObservationTarget::StateOnly {
                return Err(ActorCommandError::AgreementBindingInvalid);
            }
        }
        NativeRefundObservation::Found(found) => {
            validate_lez_refund_found(request, response, Some(state), found)?;
        }
    }
    Ok(Some(state))
}

fn validate_lez_refund_accounts(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    terms: &WitnessedNativeEscrowTerms,
    observation: &NativeEscrowAccountObservation,
) -> Result<Option<EscrowState>, ActorCommandError> {
    let NativeEscrowAccountObservation::Found(facts) = observation else {
        return Ok(None);
    };
    let metadata = facts
        .metadata
        .witnessed()
        .ok_or(ActorCommandError::AgreementBindingInvalid)?;
    let state = metadata.status;
    let signed = agreement.lez_terms();
    let expected_metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        metadata.account_id,
        config.lez_bridge.runtime.escrow_program_id,
        facts.custody.account_id,
        terms,
        state,
    );
    let expected_balance = if state == EscrowState::Funded {
        terms.amount().as_u128()
    } else {
        0
    };
    if metadata.account_id.as_bytes() != signed.metadata_account()
        || facts.custody.account_id.as_bytes() != signed.custody_account()
        || metadata != &expected_metadata
        || facts.custody.owner_program_id != terms.authenticated_transfer_program_id()
        || facts.custody.balance.as_u128() != expected_balance
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    Ok(Some(state))
}

fn validate_lez_refund_found(
    request: &ObserveNativeRefundRequest,
    response: &ObserveNativeRefundResult,
    account_state: Option<EscrowState>,
    found: &lez_bridge_protocol::NativeRefundFoundFacts,
) -> Result<(), ActorCommandError> {
    let window = match request.target {
        NativeRefundObservationTarget::Exact {
            refund_transaction_id,
            window,
        } => {
            if found.transaction.transaction_id != refund_transaction_id {
                return Err(ActorCommandError::AgreementBindingInvalid);
            }
            window
        }
        NativeRefundObservationTarget::DiscoverByTerms { window } => window,
        NativeRefundObservationTarget::StateOnly => {
            return Err(ActorCommandError::AgreementBindingInvalid);
        }
    };
    let end = window
        .start_height()
        .checked_add(u64::from(window.max_blocks() - 1))
        .ok_or(ActorCommandError::ObservationUnavailable)?;
    let terms = request
        .terms
        .witnessed()
        .ok_or(ActorCommandError::AgreementBindingInvalid)?;
    let transaction = &found.transaction;
    let NativeEscrowAccountObservation::Found(accounts) = &response.accounts else {
        return Err(ActorCommandError::AgreementBindingInvalid);
    };
    let metadata = accounts
        .metadata
        .witnessed()
        .ok_or(ActorCommandError::AgreementBindingInvalid)?;
    let expected_accounts = [
        metadata.account_id,
        accounts.custody.account_id,
        terms.depositor_account_id(),
    ];
    let same_height_wrong_hash = transaction.position.height == response.clock_after.height
        && transaction.position.block_hash != response.clock_after.block_hash;
    if account_state != Some(EscrowState::Refunded)
        || transaction.position.height < window.start_height()
        || transaction.position.height > end
        || end > response.clock_after.height
        || transaction.position.height > response.clock_after.height
        || same_height_wrong_hash
        || !transaction.is_public
        || !transaction.signer_account_ids.as_slice().is_empty()
        || found.instruction.program_id != request.runtime.escrow_program_id
        || found.instruction.swap_id != terms.swap_id()
        || found.instruction.ordered_account_ids.as_slice() != expected_accounts
        || response.clock_after.timestamp_ms < terms.refund_at_ms()
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    Ok(())
}

fn finalized_lez_refund_observation(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: RefundTransition,
    effect: Option<&PreparedLezRefundEffect>,
    request: &ObserveNativeRefundRequest,
    response: &ObserveNativeRefundResult,
) -> Result<ActorRefundObservation, ActorCommandError> {
    let NativeRefundObservation::Found(found) = &response.refund else {
        return Ok(ActorRefundObservation::Pending { chain: Chain::Lez });
    };
    if effect.is_some_and(|expected| {
        found.transaction.transaction_id != expected.transaction.transaction_id
            || found.transaction.exact_bytes.as_slice() != expected.effect.exact_public_bytes()
    }) {
        return Ok(ActorRefundObservation::Pending { chain: Chain::Lez });
    }
    let chain_evidence = encode_finalized_lez_refund_evidence(
        config, agreement, transition, effect, request, response,
    )?;
    let confirmations = response
        .clock_after
        .height
        .checked_sub(found.transaction.position.height)
        .and_then(|distance| distance.checked_add(1))
        .and_then(|depth| u32::try_from(depth).ok())
        .ok_or(ActorCommandError::ObservationUnavailable)?;
    if confirmations == 0 {
        return Err(ActorCommandError::ObservationUnavailable);
    }
    Ok(ActorRefundObservation::Ready {
        chain: Chain::Lez,
        transaction_id: hex::encode(found.transaction.transaction_id.as_bytes()).into_boxed_str(),
        confirmations: FINALIZED_LEZ_CONFIRMATION_UNITS,
        chain_evidence,
        position: SwapChainPosition::lez_timestamp_from_milliseconds_floor(
            LezUnixMilliseconds::new(response.clock_after.timestamp_ms),
        ),
    })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct FinalizedLezRefundEvidenceV1 {
    schema_version: u16,
    agreement_commitment: String,
    transition: RefundTransition,
    request: ObserveNativeRefundRequest,
    response: ObserveNativeRefundResult,
}

fn encode_finalized_lez_refund_evidence(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: RefundTransition,
    effect: Option<&PreparedLezRefundEffect>,
    request: &ObserveNativeRefundRequest,
    response: &ObserveNativeRefundResult,
) -> Result<Vec<u8>, ActorCommandError> {
    let encoded = serde_json::to_vec(&FinalizedLezRefundEvidenceV1 {
        schema_version: 1,
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        transition,
        request: request.clone(),
        response: response.clone(),
    })
    .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    decode_finalized_lez_refund_evidence(config, agreement, transition, effect, &encoded)?;
    Ok(encoded)
}

fn decode_finalized_lez_refund_evidence(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: RefundTransition,
    effect: Option<&PreparedLezRefundEffect>,
    bytes: &[u8],
) -> Result<FinalizedLezRefundEvidenceV1, ActorCommandError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let evidence = FinalizedLezRefundEvidenceV1::deserialize(&mut deserializer)
        .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    deserializer
        .end()
        .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    let canonical =
        serde_json::to_vec(&evidence).map_err(|_| ActorCommandError::ObservationUnavailable)?;
    if canonical != bytes
        || evidence.schema_version != 1
        || evidence.agreement_commitment != hex::encode(agreement.agreement_commitment())
        || evidence.transition != transition
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    validate_lez_refund_response(
        config,
        agreement,
        transition,
        &evidence.request,
        &evidence.response,
    )?;
    let NativeRefundObservation::Found(found) = &evidence.response.refund else {
        return Err(ActorCommandError::AgreementBindingInvalid);
    };
    if effect.is_some_and(|expected| {
        found.transaction.transaction_id != expected.transaction.transaction_id
            || found.transaction.exact_bytes.as_slice() != expected.effect.exact_public_bytes()
    }) {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    Ok(evidence)
}

struct BitcoinClaimObserver<P> {
    chain: P,
    effect: Option<PreparedBitcoinClaimEffect>,
    state_db: PathBuf,
}

#[async_trait]
impl<P> ClaimObservationPort for BitcoinClaimObserver<P>
where
    P: BitcoinClaimChainPort,
{
    async fn observe(
        &self,
        agreement: &BtcAgreementV1,
        transition: ClaimTransition,
    ) -> Result<ActorClaimObservation, ActorCommandError> {
        if let Some(effect) = &self.effect
            && (effect.effect.key().local_role() != transition.submitter()
                || effect.effect.key().chain() != PublicEffectChain::Bitcoin
                || effect.effect.key().operation() != PublicEffectOperation::Claim
                || effect.effect.key().predecessor_revision() != transition.predecessor_revision()
                || effect.effect.expected_effect_id() != effect.expected_transaction_id.to_string())
        {
            return Err(ActorCommandError::AgreementBindingInvalid);
        }

        let scan = self.chain.observe_claim(agreement).await;
        if let Some(effect) = &self.effect {
            self.reconcile_and_maybe_submit(agreement, effect, &scan)
                .await?;
        }

        let BitcoinClaimScan::Exact(exact) = scan else {
            return Ok(ActorClaimObservation::Pending {
                chain: Chain::Bitcoin,
            });
        };
        if !exact.finalized
            || self.effect.as_ref().is_some_and(|effect| {
                exact.transaction_bytes.as_slice() != effect.effect.exact_public_bytes()
            })
        {
            return Ok(ActorClaimObservation::Pending {
                chain: Chain::Bitcoin,
            });
        }
        Ok(ActorClaimObservation::Ready {
            chain: Chain::Bitcoin,
            transaction_id: exact.transaction_id,
            confirmations: exact.confirmations,
            chain_evidence: exact.chain_evidence,
            revealing_public_signature: (transition == ClaimTransition::RevealingClaim)
                .then_some(exact.public_signature),
        })
    }
}

impl<P> BitcoinClaimObserver<P>
where
    P: BitcoinClaimChainPort,
{
    async fn reconcile_and_maybe_submit(
        &self,
        agreement: &BtcAgreementV1,
        effect: &PreparedBitcoinClaimEffect,
        scan: &BitcoinClaimScan,
    ) -> Result<(), ActorCommandError> {
        let mut journal = SqlitePublicEffectJournal::open(&self.state_db)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        let _ = journal
            .record_prepared(&effect.effect)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        let observation = match scan {
            BitcoinClaimScan::Unspent => PublicEffectObservation::Absent,
            BitcoinClaimScan::Exact(exact)
                if exact.transaction_bytes.as_slice() == effect.effect.exact_public_bytes() =>
            {
                PublicEffectObservation::PresentExact(exact.transaction_bytes.clone())
            }
            BitcoinClaimScan::Exact(_) => PublicEffectObservation::ConflictingPresence,
            BitcoinClaimScan::Uncertain => PublicEffectObservation::Uncertain,
        };
        let decision = journal
            .reconcile(effect.effect.key(), observation)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        let PublicEffectDecision::SubmitOnce(_) = decision else {
            return Ok(());
        };

        let submission = self
            .chain
            .submit_authorized_claim(
                agreement,
                effect.effect.exact_public_bytes(),
                effect.expected_transaction_id,
            )
            .await;
        let (result, deferred_error) = match submission {
            Ok(AuthorizedClaimSubmission::Accepted { transaction_id })
                if transaction_id == effect.expected_transaction_id =>
            {
                (
                    PublicEffectSubmissionResult::Accepted(
                        transaction_id.to_string().into_boxed_str(),
                    ),
                    None,
                )
            }
            Ok(AuthorizedClaimSubmission::Accepted { .. }) => (
                PublicEffectSubmissionResult::Unknown,
                Some(ActorCommandError::AgreementBindingInvalid),
            ),
            Ok(AuthorizedClaimSubmission::Rejected) => {
                (PublicEffectSubmissionResult::Rejected, None)
            }
            Ok(AuthorizedClaimSubmission::Unknown) => (PublicEffectSubmissionResult::Unknown, None),
            Err(error) => (PublicEffectSubmissionResult::Unknown, Some(error)),
        };
        let _ = journal
            .record_submission_result(effect.effect.key(), &result)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        if let Some(error) = deferred_error {
            return Err(error);
        }
        Ok(())
    }
}

struct LezClaimObserver<'a, P> {
    config: &'a ActorConfig,
    chain: P,
    effect: Option<PreparedLezClaimEffect>,
    prepared_claim: PreparedWitnessedClaim,
    state_db: PathBuf,
}

#[async_trait]
impl<P> ClaimObservationPort for LezClaimObserver<'_, P>
where
    P: LezClaimChainPort,
{
    async fn observe(
        &self,
        agreement: &BtcAgreementV1,
        transition: ClaimTransition,
    ) -> Result<ActorClaimObservation, ActorCommandError> {
        if let Some(effect) = &self.effect {
            validate_prepared_lez_effect(effect, transition)?;
            let mut journal = SqlitePublicEffectJournal::open(&self.state_db)
                .map_err(|_| ActorCommandError::StateUnavailable)?;
            // Exact public bytes and ID reach durable storage before the first
            // presence call or any CAS that could confer send authority.
            let _ = journal
                .record_prepared(&effect.effect)
                .map_err(|_| ActorCommandError::StateUnavailable)?;
        }

        let request = finalized_lez_claim_request(
            self.config,
            agreement,
            transition,
            &self.prepared_claim,
            self.effect.as_ref(),
        )?;
        let durable_request = request.clone();
        let presence = self
            .chain
            .classify_finalized_witnessed_claim(request)
            .await?;
        match &presence {
            FinalizedWitnessedClaimPresence::PresentExact {
                context,
                finalized_tip,
                scanned_window,
                ..
            }
            | FinalizedWitnessedClaimPresence::NotFound {
                context,
                finalized_tip,
                scanned_window,
            } => validate_finalized_lez_presence_envelope(
                &durable_request,
                context,
                *finalized_tip,
                *scanned_window,
            )?,
            FinalizedWitnessedClaimPresence::Unavailable(_)
            | FinalizedWitnessedClaimPresence::Uncertain(_) => {}
        }
        if let Some(effect) = &self.effect {
            // Reconcile the raw exact public identity before decoding evidence.
            // A conflicting PresentExact durably consumes send authority without
            // a transport call, so no later absence can rearm this effect.
            self.reconcile_and_maybe_submit(agreement, transition, effect, &presence)
                .await?;
        }
        let canonical = match &presence {
            FinalizedWitnessedClaimPresence::PresentExact {
                context,
                finalized_tip,
                scanned_window,
                claim,
            } => Some(encode_finalized_lez_claim_evidence(
                self.config,
                agreement,
                transition,
                self.effect.as_ref(),
                &durable_request,
                context,
                *finalized_tip,
                *scanned_window,
                claim,
            )?),
            FinalizedWitnessedClaimPresence::NotFound { .. }
            | FinalizedWitnessedClaimPresence::Unavailable(_)
            | FinalizedWitnessedClaimPresence::Uncertain(_) => None,
        };

        let FinalizedWitnessedClaimPresence::PresentExact { claim, .. } = presence else {
            return Ok(ActorClaimObservation::Pending { chain: Chain::Lez });
        };
        let Some(chain_evidence) = canonical else {
            return Ok(ActorClaimObservation::Pending { chain: Chain::Lez });
        };
        Ok(ActorClaimObservation::Ready {
            chain: Chain::Lez,
            transaction_id: hex::encode(claim.transaction.transaction_id.as_bytes())
                .into_boxed_str(),
            confirmations: FINALIZED_LEZ_CONFIRMATION_UNITS,
            chain_evidence,
            revealing_public_signature: (transition == ClaimTransition::RevealingClaim)
                .then_some(*claim.aggregate_signature.as_bytes()),
        })
    }
}

impl<P> LezClaimObserver<'_, P>
where
    P: LezClaimChainPort,
{
    async fn reconcile_and_maybe_submit(
        &self,
        agreement: &BtcAgreementV1,
        transition: ClaimTransition,
        effect: &PreparedLezClaimEffect,
        presence: &FinalizedWitnessedClaimPresence,
    ) -> Result<(), ActorCommandError> {
        let observation = match presence {
            FinalizedWitnessedClaimPresence::PresentExact { claim, .. }
                if claim.transaction.transaction_id == effect.transaction.transaction_id
                    && claim.transaction.exact_bytes.as_slice()
                        == effect.effect.exact_public_bytes()
                    && claim.aggregate_signature.as_bytes() == &effect.aggregate_signature =>
            {
                PublicEffectObservation::PresentExact(
                    claim.transaction.exact_bytes.as_slice().to_vec(),
                )
            }
            FinalizedWitnessedClaimPresence::NotFound { .. } => PublicEffectObservation::Absent,
            FinalizedWitnessedClaimPresence::PresentExact { .. } => {
                PublicEffectObservation::ConflictingPresence
            }
            FinalizedWitnessedClaimPresence::Unavailable(_)
            | FinalizedWitnessedClaimPresence::Uncertain(_) => PublicEffectObservation::Uncertain,
        };
        let mut journal = SqlitePublicEffectJournal::open(&self.state_db)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        let decision = journal
            .reconcile(effect.effect.key(), observation)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        let PublicEffectDecision::SubmitOnce(_) = decision else {
            return Ok(());
        };
        drop(journal);

        let request = submit_lez_claim_request(self.config, agreement, transition, effect)?;
        let expected_context = request.context.clone();
        let submission = self.chain.submit_transaction(request).await;
        let (result, deferred_error) = match submission {
            Ok(result)
                if result.context == expected_context
                    && result.transaction_id == effect.transaction.transaction_id
                    && matches!(
                        result.outcome,
                        SubmissionOutcome::Accepted | SubmissionOutcome::AlreadyKnown
                    ) =>
            {
                (
                    PublicEffectSubmissionResult::Accepted(
                        hex::encode(result.transaction_id.as_bytes()).into_boxed_str(),
                    ),
                    None,
                )
            }
            Ok(_) => (
                PublicEffectSubmissionResult::Unknown,
                Some(ActorCommandError::AgreementBindingInvalid),
            ),
            Err(error) => (PublicEffectSubmissionResult::Unknown, Some(error)),
        };
        let mut journal = SqlitePublicEffectJournal::open(&self.state_db)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        let _ = journal
            .record_submission_result(effect.effect.key(), &result)
            .map_err(|_| ActorCommandError::StateUnavailable)?;
        if let Some(error) = deferred_error {
            return Err(error);
        }
        Ok(())
    }
}

fn validate_prepared_lez_effect(
    effect: &PreparedLezClaimEffect,
    transition: ClaimTransition,
) -> Result<(), ActorCommandError> {
    if effect.effect.key().local_role() != transition.submitter()
        || effect.effect.key().chain() != PublicEffectChain::Lez
        || effect.effect.key().operation() != PublicEffectOperation::Claim
        || effect.effect.key().predecessor_revision() != transition.predecessor_revision()
        || effect.effect.expected_effect_id()
            != hex::encode(effect.transaction.transaction_id.as_bytes())
        || effect.effect.exact_public_bytes() != effect.transaction.exact_bytes.as_slice()
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    Ok(())
}

fn finalized_lez_claim_request(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: ClaimTransition,
    claim: &PreparedWitnessedClaim,
    effect: Option<&PreparedLezClaimEffect>,
) -> Result<ObserveFinalizedWitnessedClaimRequest, ActorCommandError> {
    validate_actor_binding(config, agreement)?;
    if agreement
        .coordinator()
        .funded_chain(transition.funded_participant())
        != Chain::Lez
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    if let Some(effect) = effect {
        validate_prepared_lez_effect(effect, transition)?;
    }
    let terms = witnessed_lez_terms(agreement)?;
    let window = config.discovery_window()?;
    let target = effect.map_or(
        FinalizedWitnessedClaimObservationTarget::DiscoverByTerms,
        |value| FinalizedWitnessedClaimObservationTarget::Exact {
            claim_transaction_id: value.transaction.transaction_id,
        },
    );
    let identity = FinalizedLezClaimRequestIdentityV1 {
        schema_version: 1,
        operation: "classify_finalized_witnessed_claim",
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        transition,
        run_id: &config.lez_bridge.run_id,
        sidecar_role: config.role.bridge(),
        runtime: &config.lez_bridge.runtime,
        terms: &terms,
        claim,
        target,
        window,
    };
    let request_id = deterministic_request_id(&identity)?;
    let context = MessageContext::new(
        config.lez_bridge.run_id.clone(),
        request_id,
        config.role.bridge(),
    );
    Ok(match target {
        FinalizedWitnessedClaimObservationTarget::Exact {
            claim_transaction_id,
        } => ObserveFinalizedWitnessedClaimRequest::new(
            context,
            config.lez_bridge.runtime.clone(),
            terms,
            claim.clone(),
            claim_transaction_id,
            window,
        ),
        FinalizedWitnessedClaimObservationTarget::DiscoverByTerms => {
            ObserveFinalizedWitnessedClaimRequest::discover_by_terms(
                context,
                config.lez_bridge.runtime.clone(),
                terms,
                claim.clone(),
                window,
            )
        }
    })
}

#[derive(Serialize)]
struct FinalizedLezClaimRequestIdentityV1<'a> {
    schema_version: u16,
    operation: &'static str,
    agreement_commitment: String,
    transition: ClaimTransition,
    run_id: &'a RunId,
    sidecar_role: BridgeParticipant,
    runtime: &'a RuntimeDescriptor,
    terms: &'a WitnessedNativeEscrowTerms,
    claim: &'a PreparedWitnessedClaim,
    target: FinalizedWitnessedClaimObservationTarget,
    window: DiscoveryWindow,
}

fn submit_lez_claim_request(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: ClaimTransition,
    effect: &PreparedLezClaimEffect,
) -> Result<SubmitTransactionRequest, ActorCommandError> {
    validate_prepared_lez_effect(effect, transition)?;
    let identity = SubmitLezClaimRequestIdentityV1 {
        schema_version: 1,
        operation: "submit_transaction",
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        transition,
        run_id: &config.lez_bridge.run_id,
        sidecar_role: config.role.bridge(),
        runtime: &config.lez_bridge.runtime,
        transaction: &effect.transaction,
    };
    let request_id = deterministic_request_id(&identity)?;
    Ok(SubmitTransactionRequest::new(
        MessageContext::new(
            config.lez_bridge.run_id.clone(),
            request_id,
            config.role.bridge(),
        ),
        config.lez_bridge.runtime.clone(),
        effect.transaction.clone(),
    ))
}

#[derive(Serialize)]
struct SubmitLezClaimRequestIdentityV1<'a> {
    schema_version: u16,
    operation: &'static str,
    agreement_commitment: String,
    transition: ClaimTransition,
    run_id: &'a RunId,
    sidecar_role: BridgeParticipant,
    runtime: &'a RuntimeDescriptor,
    transaction: &'a PreparedTransaction,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct FinalizedLezClaimEvidenceV1 {
    schema_version: u16,
    agreement_commitment: String,
    transition: ClaimTransition,
    request: ObserveFinalizedWitnessedClaimRequest,
    finalized_tip: ChainTip,
    scanned_window: DiscoveryWindow,
    claim: FinalizedWitnessedClaimFacts,
}

#[allow(clippy::too_many_arguments)]
fn encode_finalized_lez_claim_evidence(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: ClaimTransition,
    effect: Option<&PreparedLezClaimEffect>,
    request: &ObserveFinalizedWitnessedClaimRequest,
    context: &MessageContext,
    finalized_tip: ChainTip,
    scanned_window: DiscoveryWindow,
    claim: &FinalizedWitnessedClaimFacts,
) -> Result<Vec<u8>, ActorCommandError> {
    if context != &request.context {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    let encoded = serde_json::to_vec(&FinalizedLezClaimEvidenceV1 {
        schema_version: 1,
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        transition,
        request: request.clone(),
        finalized_tip,
        scanned_window,
        claim: claim.clone(),
    })
    .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    decode_finalized_lez_claim_evidence(config, agreement, transition, effect, &encoded)?;
    Ok(encoded)
}

fn decode_finalized_lez_claim_evidence(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: ClaimTransition,
    effect: Option<&PreparedLezClaimEffect>,
    bytes: &[u8],
) -> Result<FinalizedLezClaimEvidenceV1, ActorCommandError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let evidence = FinalizedLezClaimEvidenceV1::deserialize(&mut deserializer)
        .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    deserializer
        .end()
        .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    let canonical =
        serde_json::to_vec(&evidence).map_err(|_| ActorCommandError::ObservationUnavailable)?;
    if canonical != bytes
        || evidence.schema_version != 1
        || evidence.agreement_commitment != hex::encode(agreement.agreement_commitment())
        || evidence.transition != transition
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    validate_finalized_lez_claim_binding(
        config,
        agreement,
        transition,
        effect,
        &evidence.request,
        evidence.finalized_tip,
        evidence.scanned_window,
        &evidence.claim,
    )?;
    Ok(evidence)
}

#[allow(clippy::too_many_arguments)]
fn validate_finalized_lez_claim_binding(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: ClaimTransition,
    effect: Option<&PreparedLezClaimEffect>,
    request: &ObserveFinalizedWitnessedClaimRequest,
    finalized_tip: ChainTip,
    scanned_window: DiscoveryWindow,
    claim: &FinalizedWitnessedClaimFacts,
) -> Result<(), ActorCommandError> {
    let prepared = load_prepared_witnessed_claim(config, agreement)?;
    if request
        != &finalized_lez_claim_request(config, agreement, transition, &prepared.claim, effect)?
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    validate_finalized_lez_presence_envelope(
        request,
        &request.context,
        finalized_tip,
        scanned_window,
    )?;
    let transaction = &claim.transaction;
    let instruction = &claim.instruction;
    let block = claim.containing_block;
    let metadata = &claim.metadata;
    let custody = &claim.custody;
    let terms = &request.terms;
    let expected_metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        metadata.account_id,
        request.runtime.escrow_program_id,
        custody.account_id,
        terms,
        EscrowState::Claimed,
    );
    let expected_accounts = [
        metadata.account_id,
        custody.account_id,
        terms.claimant_account_id(),
        terms.aggregate_authority_account_id(),
    ];
    let (context, _) = verified_chain_presignature(config, agreement, Chain::Lez)?;
    let aggregate_signature = *claim.aggregate_signature.as_bytes();
    let window_start = request.window.start_height();
    let window_end = window_start
        .checked_add(u64::from(request.window.max_blocks() - 1))
        .ok_or(ActorCommandError::ObservationUnavailable)?;
    let wrong_target = matches!(
        request.target,
        FinalizedWitnessedClaimObservationTarget::Exact {
            claim_transaction_id
        } if claim_transaction_id != transaction.transaction_id
    );
    if wrong_target
        || block.block_id < window_start
        || block.block_id > window_end
        || block.block_id > finalized_tip.height
        || (block.block_id == finalized_tip.height && block.block_hash != finalized_tip.block_hash)
        || transaction.position.height != block.block_id
        || transaction.position.block_hash != block.block_hash
        || !transaction.is_public
        || transaction.signer_account_ids.as_slice() != [terms.aggregate_authority_account_id()]
        || instruction.program_id != request.runtime.escrow_program_id
        || instruction.swap_id != terms.swap_id()
        || instruction.claimant_account_id != terms.claimant_account_id()
        || instruction.aggregate_authority_account_id != terms.aggregate_authority_account_id()
        || instruction.claim != prepared.claim
        || instruction.ordered_account_ids.as_slice() != expected_accounts
        || metadata != &expected_metadata
        || custody.account_id != metadata.custody_account_id
        || custody.owner_program_id != terms.authenticated_transfer_program_id()
        || custody.balance.as_u128() != 0
        || verify_final_signature(&context, aggregate_signature).is_err()
        || effect.is_some_and(|expected| {
            transaction.transaction_id != expected.transaction.transaction_id
                || transaction.exact_bytes.as_slice() != expected.effect.exact_public_bytes()
                || aggregate_signature != expected.aggregate_signature
        })
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    Ok(())
}

fn validate_finalized_lez_presence_envelope(
    request: &ObserveFinalizedWitnessedClaimRequest,
    context: &MessageContext,
    finalized_tip: ChainTip,
    scanned_window: DiscoveryWindow,
) -> Result<(), ActorCommandError> {
    let window_start = request.window.start_height();
    let window_end = window_start
        .checked_add(u64::from(request.window.max_blocks() - 1))
        .ok_or(ActorCommandError::ObservationUnavailable)?;
    if context != &request.context
        || scanned_window != request.window
        || window_end > finalized_tip.height
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct LezMakerLockFoundEvidenceV1 {
    schema_version: u16,
    read_ordinal: u8,
    agreement_commitment: String,
    maker_chain: Chain,
    request: ObserveFinalizedWitnessedFundingRequest,
    finalized_clock: ChainClock,
    scanned_window: DiscoveryWindow,
    funding: lez_bridge_protocol::FinalizedWitnessedFundingFacts,
}

#[allow(clippy::too_many_arguments)]
fn encode_lez_maker_lock_found_evidence(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    read_ordinal: u8,
    request: &ObserveFinalizedWitnessedFundingRequest,
    context: &MessageContext,
    finalized_clock: ChainClock,
    scanned_window: DiscoveryWindow,
    funding: &lez_bridge_protocol::FinalizedWitnessedFundingFacts,
) -> Result<Vec<u8>, ActorCommandError> {
    let maker_chain = agreement.coordinator().funded_chain(Participant::Maker);
    let window_end = request
        .window
        .start_height()
        .checked_add(u64::from(request.window.max_blocks() - 1))
        .ok_or(ActorCommandError::ObservationUnavailable)?;
    let signed = agreement.lez_terms();
    if !matches!(read_ordinal, 1 | 2)
        || maker_chain != Chain::Lez
        || request != &first_lock_lez_funding_request(config, agreement, read_ordinal)?
        || context != &request.context
        || scanned_window != request.window
        || window_end > finalized_clock.height
        || funding.metadata.account_id.as_bytes() != signed.metadata_account()
        || funding.custody.account_id.as_bytes() != signed.custody_account()
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    serde_json::to_vec(&LezMakerLockFoundEvidenceV1 {
        schema_version: 1,
        read_ordinal,
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        maker_chain,
        request: request.clone(),
        finalized_clock,
        scanned_window,
        funding: funding.clone(),
    })
    .map_err(|_| ActorCommandError::ObservationUnavailable)
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct LezMakerLockAbsenceEvidenceV1 {
    schema_version: u16,
    read_ordinal: u8,
    agreement_commitment: String,
    maker_chain: Chain,
    cutoff_unix_seconds: u64,
    request: ObserveFinalizedWitnessedFundingRequest,
    finalized_clock: ChainClock,
    scanned_window: DiscoveryWindow,
}

fn encode_lez_maker_lock_absence_evidence(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    read_ordinal: u8,
    request: &ObserveFinalizedWitnessedFundingRequest,
    context: &MessageContext,
    finalized_clock: ChainClock,
    scanned_window: DiscoveryWindow,
) -> Result<Vec<u8>, ActorCommandError> {
    let maker_chain = agreement.coordinator().funded_chain(Participant::Maker);
    let cutoff_unix_seconds = agreement
        .body()
        .recovery_plan()
        .maker_second_lock_cutoff_unix_seconds();
    let cutoff_ms = cutoff_unix_seconds
        .checked_mul(1_000)
        .ok_or(ActorCommandError::AgreementBindingInvalid)?;
    let window_end = request
        .window
        .start_height()
        .checked_add(u64::from(request.window.max_blocks() - 1))
        .ok_or(ActorCommandError::ObservationUnavailable)?;
    if !matches!(read_ordinal, 1 | 2)
        || maker_chain != Chain::Lez
        || request != &first_lock_lez_funding_request(config, agreement, read_ordinal)?
        || context != &request.context
        || scanned_window != request.window
        || window_end != finalized_clock.height
        || finalized_clock.timestamp_ms < cutoff_ms
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    serde_json::to_vec(&LezMakerLockAbsenceEvidenceV1 {
        schema_version: 1,
        read_ordinal,
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        maker_chain,
        cutoff_unix_seconds,
        request: request.clone(),
        finalized_clock,
        scanned_window,
    })
    .map_err(|_| ActorCommandError::ObservationUnavailable)
}

#[async_trait]
impl FirstLockRecoverySafetyPort for LiveLezMakerLockSafety {
    async fn observe(
        &self,
        agreement: &BtcAgreementV1,
        read_ordinal: u8,
    ) -> Result<FirstLockRecoverySafetyObservation, ActorCommandError> {
        let maker_chain = agreement.coordinator().funded_chain(Participant::Maker);
        if maker_chain != Chain::Lez {
            return Err(ActorCommandError::AgreementBindingInvalid);
        }
        let request = first_lock_lez_funding_request(&self.config, agreement, read_ordinal)?;
        let durable_request = request.clone();
        let factory = CapabilityFileBridgeClientFactory::new(
            self.config.lez_bridge.endpoint.to_string(),
            self.config.lez_bridge.capability_file.clone(),
            self.config.lez_bridge.run_id.clone(),
            self.config.lez_bridge.runtime.clone(),
            Duration::from_millis(self.config.lez_bridge.request_timeout_millis),
        );
        let Ok(client) = factory.fresh_transport() else {
            return Ok(FirstLockRecoverySafetyObservation::Uncertain { maker_chain });
        };
        let Ok(presence) = client.classify_finalized_witnessed_funding(request).await else {
            return Ok(FirstLockRecoverySafetyObservation::Uncertain { maker_chain });
        };
        match presence {
            FinalizedWitnessedFundingPresence::Found {
                context,
                finalized_clock,
                scanned_window,
                funding,
            } => {
                let chain_evidence = encode_lez_maker_lock_found_evidence(
                    &self.config,
                    agreement,
                    read_ordinal,
                    &durable_request,
                    &context,
                    finalized_clock,
                    scanned_window,
                    funding.as_ref(),
                )?;
                Ok(FirstLockRecoverySafetyObservation::MakerLockReady {
                    chain: maker_chain,
                    transaction_id: hex::encode(funding.transaction.transaction_id.as_bytes())
                        .into_boxed_str(),
                    confirmations: FINALIZED_LEZ_CONFIRMATION_UNITS,
                    chain_evidence,
                })
            }
            FinalizedWitnessedFundingPresence::Absent {
                context,
                finalized_clock,
                scanned_window,
            } => {
                let cutoff_unix_seconds = agreement
                    .body()
                    .recovery_plan()
                    .maker_second_lock_cutoff_unix_seconds();
                let observed_unix_seconds = finalized_clock.timestamp_ms / 1_000;
                if observed_unix_seconds < cutoff_unix_seconds {
                    return Ok(FirstLockRecoverySafetyObservation::Uncertain { maker_chain });
                }
                let absence_evidence = encode_lez_maker_lock_absence_evidence(
                    &self.config,
                    agreement,
                    read_ordinal,
                    &durable_request,
                    &context,
                    finalized_clock,
                    scanned_window,
                )?;
                Ok(FirstLockRecoverySafetyObservation::ReadyToRefund {
                    maker_chain,
                    cutoff_unix_seconds,
                    observed_unix_seconds,
                    absence_evidence,
                })
            }
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct BitcoinMakerLockFoundEvidenceV1 {
    schema_version: u16,
    read_ordinal: u8,
    agreement_commitment: String,
    maker_chain: Chain,
    core_evidence_hex: String,
}

fn encode_bitcoin_maker_lock_found_evidence(
    agreement: &BtcAgreementV1,
    read_ordinal: u8,
    core_evidence: &[u8],
) -> Result<Vec<u8>, ActorCommandError> {
    let maker_chain = agreement.coordinator().funded_chain(Participant::Maker);
    if !matches!(read_ordinal, 1 | 2) || maker_chain != Chain::Bitcoin || core_evidence.is_empty() {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    serde_json::to_vec(&BitcoinMakerLockFoundEvidenceV1 {
        schema_version: 1,
        read_ordinal,
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        maker_chain,
        core_evidence_hex: hex::encode(core_evidence),
    })
    .map_err(|_| ActorCommandError::ObservationUnavailable)
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct BitcoinMakerLockAbsenceEvidenceV1 {
    schema_version: u16,
    read_ordinal: u8,
    agreement_commitment: String,
    maker_chain: Chain,
    cutoff_unix_seconds: u64,
    expected_funding_transaction_id: String,
    stable_tip_block_hash: String,
    stable_tip_height: u32,
    stable_tip_median_time_unix_seconds: u64,
}

fn encode_bitcoin_maker_lock_absence_evidence(
    agreement: &BtcAgreementV1,
    read_ordinal: u8,
    stable_tip_block_hash: String,
    stable_tip_height: u32,
    stable_tip_median_time_unix_seconds: u64,
) -> Result<Vec<u8>, ActorCommandError> {
    let maker_chain = agreement.coordinator().funded_chain(Participant::Maker);
    let cutoff = agreement
        .body()
        .recovery_plan()
        .maker_second_lock_cutoff_unix_seconds();
    if !matches!(read_ordinal, 1 | 2)
        || maker_chain != Chain::Bitcoin
        || stable_tip_median_time_unix_seconds < cutoff
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    serde_json::to_vec(&BitcoinMakerLockAbsenceEvidenceV1 {
        schema_version: 1,
        read_ordinal,
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        maker_chain,
        cutoff_unix_seconds: cutoff,
        expected_funding_transaction_id: hex::encode(agreement.funding_terms().transaction_id()),
        stable_tip_block_hash,
        stable_tip_height,
        stable_tip_median_time_unix_seconds,
    })
    .map_err(|_| ActorCommandError::ObservationUnavailable)
}

#[async_trait]
impl FirstLockRecoverySafetyPort for LiveBitcoinMakerLockSafety {
    async fn observe(
        &self,
        agreement: &BtcAgreementV1,
        read_ordinal: u8,
    ) -> Result<FirstLockRecoverySafetyObservation, ActorCommandError> {
        let maker_chain = agreement.coordinator().funded_chain(Participant::Maker);
        if maker_chain != Chain::Bitcoin {
            return Err(ActorCommandError::AgreementBindingInvalid);
        }
        let Ok(core_config) = HttpBitcoinCoreConfig::new(self.config.bitcoin_core.endpoint.clone())
            .and_then(|value| value.with_cookie_file(&self.config.bitcoin_core.cookie_file))
        else {
            return Ok(FirstLockRecoverySafetyObservation::Uncertain { maker_chain });
        };
        let Ok(rpc) = HttpBitcoinCoreRpc::connect(&core_config) else {
            return Ok(FirstLockRecoverySafetyObservation::Uncertain { maker_chain });
        };
        let adapter = BitcoinCoreAdapter::new(rpc, self.config.bitcoin_core.connectivity.into());
        let Ok(observation) = adapter.observe_funding(agreement).await else {
            return Ok(FirstLockRecoverySafetyObservation::Uncertain { maker_chain });
        };
        match observation {
            FundingObservation::Ready(observed) => {
                let chain_evidence = BitcoinCoreEvidenceV1::funding_ready(agreement, &observed)
                    .and_then(|evidence| evidence.encode())
                    .map_err(|_| ActorCommandError::ObservationUnavailable)?;
                let chain_evidence = encode_bitcoin_maker_lock_found_evidence(
                    agreement,
                    read_ordinal,
                    &chain_evidence,
                )?;
                Ok(FirstLockRecoverySafetyObservation::MakerLockReady {
                    chain: maker_chain,
                    transaction_id: observed
                        .transaction()
                        .compute_txid()
                        .to_string()
                        .into_boxed_str(),
                    confirmations: observed.confirmations(),
                    chain_evidence,
                })
            }
            FundingObservation::Pending { .. } => {
                Ok(FirstLockRecoverySafetyObservation::Uncertain { maker_chain })
            }
            FundingObservation::Absent { stable_tip } => {
                let observed_unix_seconds = stable_tip.median_time_unix_seconds();
                let cutoff_unix_seconds = agreement
                    .body()
                    .recovery_plan()
                    .maker_second_lock_cutoff_unix_seconds();
                if observed_unix_seconds < cutoff_unix_seconds {
                    return Ok(FirstLockRecoverySafetyObservation::Uncertain { maker_chain });
                }
                let absence_evidence = encode_bitcoin_maker_lock_absence_evidence(
                    agreement,
                    read_ordinal,
                    stable_tip.block_hash().to_string(),
                    stable_tip.height(),
                    observed_unix_seconds,
                )?;
                Ok(FirstLockRecoverySafetyObservation::ReadyToRefund {
                    maker_chain,
                    cutoff_unix_seconds,
                    observed_unix_seconds,
                    absence_evidence,
                })
            }
        }
    }
}

struct LezFundingObserver<'a> {
    config: &'a ActorConfig,
    client: BridgeClient,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct FinalizedLezFundingEvidenceV1 {
    schema_version: u16,
    agreement_commitment: String,
    request: ObserveFinalizedWitnessedFundingRequest,
    finalized_tip: ChainTip,
    funding: lez_bridge_protocol::FinalizedWitnessedFundingFacts,
}

#[async_trait]
impl FundingObservationPort for LezFundingObserver<'_> {
    async fn observe(
        &self,
        agreement: &BtcAgreementV1,
        _transition: FundingTransition,
    ) -> Result<ActorFundingObservation, ActorCommandError> {
        let request = finalized_lez_funding_request(self.config, agreement)?;
        let durable_request = request.clone();
        let result = self
            .client
            .observe_finalized_witnessed_funding(request)
            .await
            .map_err(|_| ActorCommandError::ObservationUnavailable)?;
        let signed = agreement.lez_terms();
        if result.funding.metadata.account_id.as_bytes() != signed.metadata_account()
            || result.funding.custody.account_id.as_bytes() != signed.custody_account()
        {
            return Err(ActorCommandError::AgreementBindingInvalid);
        }
        let chain_evidence = encode_finalized_lez_funding_evidence(
            self.config,
            agreement,
            &durable_request,
            result.finalized_tip,
            &result.funding,
        )?;
        Ok(ActorFundingObservation::Ready {
            chain: Chain::Lez,
            transaction_id: hex::encode(result.funding.transaction.transaction_id.as_bytes())
                .into_boxed_str(),
            confirmations: FINALIZED_LEZ_CONFIRMATION_UNITS,
            chain_evidence,
        })
    }
}

fn encode_finalized_lez_funding_evidence(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    request: &ObserveFinalizedWitnessedFundingRequest,
    finalized_tip: ChainTip,
    funding: &lez_bridge_protocol::FinalizedWitnessedFundingFacts,
) -> Result<Vec<u8>, ActorCommandError> {
    let encoded = serde_json::to_vec(&FinalizedLezFundingEvidenceV1 {
        schema_version: 1,
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        request: request.clone(),
        finalized_tip,
        funding: funding.clone(),
    })
    .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    decode_finalized_lez_funding_evidence(config, agreement, &encoded)?;
    Ok(encoded)
}

fn decode_finalized_lez_funding_evidence(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    bytes: &[u8],
) -> Result<FinalizedLezFundingEvidenceV1, ActorCommandError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let evidence = FinalizedLezFundingEvidenceV1::deserialize(&mut deserializer)
        .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    deserializer
        .end()
        .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    let canonical =
        serde_json::to_vec(&evidence).map_err(|_| ActorCommandError::ObservationUnavailable)?;
    if canonical != bytes
        || evidence.schema_version != 1
        || evidence.agreement_commitment != hex::encode(agreement.agreement_commitment())
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    validate_finalized_lez_funding_binding(
        config,
        agreement,
        &evidence.request,
        evidence.finalized_tip,
        &evidence.funding,
    )?;
    Ok(evidence)
}

fn validate_finalized_lez_funding_binding(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    request: &ObserveFinalizedWitnessedFundingRequest,
    finalized_tip: ChainTip,
    funding: &lez_bridge_protocol::FinalizedWitnessedFundingFacts,
) -> Result<(), ActorCommandError> {
    if request != &finalized_lez_funding_request(config, agreement)? {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    let transaction = &funding.transaction;
    let instruction = &funding.instruction;
    let block = funding.containing_block;
    let metadata = &funding.metadata;
    let custody = &funding.custody;
    let terms = &request.terms;
    let window_start = request.window.start_height();
    let window_end = window_start
        .checked_add(u64::from(request.window.max_blocks() - 1))
        .ok_or(ActorCommandError::ObservationUnavailable)?;
    let expected_metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        metadata.account_id,
        request.runtime.escrow_program_id,
        custody.account_id,
        terms,
        EscrowState::Funded,
    );
    let expected_accounts = [
        metadata.account_id,
        custody.account_id,
        terms.depositor_account_id(),
    ];
    let signed = agreement.lez_terms();
    if metadata.account_id.as_bytes() != signed.metadata_account()
        || custody.account_id.as_bytes() != signed.custody_account()
        || !transaction.is_public
        || transaction.position.height != block.block_id
        || transaction.position.block_hash != block.block_hash
        || transaction.signer_account_ids.as_slice() != [terms.depositor_account_id()]
        || instruction.program_id != request.runtime.escrow_program_id
        || instruction.swap_id != terms.swap_id()
        || instruction.ordered_account_ids.as_slice() != expected_accounts
        || metadata != &expected_metadata
        || custody.owner_program_id != terms.authenticated_transfer_program_id()
        || custody.balance != terms.amount()
        || block.block_id < window_start
        || block.block_id > window_end
        || block.block_id > finalized_tip.height
        || (block.block_id == finalized_tip.height && block.block_hash != finalized_tip.block_hash)
        || window_end > finalized_tip.height
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    Ok(())
}

fn finalized_lez_funding_request(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
) -> Result<ObserveFinalizedWitnessedFundingRequest, ActorCommandError> {
    validate_actor_binding(config, agreement)?;
    let terms = witnessed_lez_terms(agreement)?;
    let window = config.discovery_window()?;
    // The v0.2 server records non-submit observations as `Repeatable`: an exact
    // repeated ID/request digest is re-executed against the current finalized
    // tip, never served from a cached response. Bind the deterministic ID to
    // every non-ID request field so changing a discovery window or runtime
    // creates a distinct reservation while an exact retry retains its ID.
    let identity = FinalizedLezFundingRequestIdentityV1 {
        schema_version: 1,
        run_id: &config.lez_bridge.run_id,
        sidecar_role: config.role.bridge(),
        runtime: &config.lez_bridge.runtime,
        terms: &terms,
        target: "discover_by_terms",
        window,
    };
    let request_id = deterministic_request_id(&identity)?;
    Ok(ObserveFinalizedWitnessedFundingRequest::discover_by_terms(
        MessageContext::new(
            config.lez_bridge.run_id.clone(),
            request_id,
            config.role.bridge(),
        ),
        config.lez_bridge.runtime.clone(),
        terms,
        window,
    ))
}

fn first_lock_lez_funding_request(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    read_ordinal: u8,
) -> Result<ObserveFinalizedWitnessedFundingRequest, ActorCommandError> {
    validate_actor_binding(config, agreement)?;
    if !matches!(read_ordinal, 1 | 2) {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    let terms = witnessed_lez_terms(agreement)?;
    let window = config.discovery_window()?;
    let identity = FirstLockLezFundingRequestIdentityV1 {
        schema_version: 1,
        operation: "classify_first_lock_maker_funding",
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        read_ordinal,
        run_id: config.lez_bridge.run_id.clone(),
        sidecar_role: config.role.bridge(),
        runtime: config.lez_bridge.runtime.clone(),
        terms: terms.clone(),
        target: "discover_by_terms",
        window,
    };
    let request_id = deterministic_request_id(&identity)?;
    Ok(ObserveFinalizedWitnessedFundingRequest::discover_by_terms(
        MessageContext::new(
            config.lez_bridge.run_id.clone(),
            request_id,
            config.role.bridge(),
        ),
        config.lez_bridge.runtime.clone(),
        terms,
        window,
    ))
}

#[derive(Serialize)]
struct FirstLockLezFundingRequestIdentityV1 {
    schema_version: u16,
    operation: &'static str,
    agreement_commitment: String,
    read_ordinal: u8,
    run_id: RunId,
    sidecar_role: BridgeParticipant,
    runtime: RuntimeDescriptor,
    terms: WitnessedNativeEscrowTerms,
    target: &'static str,
    window: DiscoveryWindow,
}

fn witnessed_lez_terms(
    agreement: &BtcAgreementV1,
) -> Result<WitnessedNativeEscrowTerms, ActorCommandError> {
    let signed = agreement.lez_terms();
    WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: Hex32::from_bytes(*agreement.body().swap_id()),
        terms_hash: Hex32::from_bytes(*agreement.agreement_commitment()),
        depositor: bridge_participant(agreement.lez_depositor()),
        depositor_account_id: Hex32::from_bytes(*signed.depositor_account()),
        claimant: bridge_participant(agreement.lez_claimant()),
        claimant_account_id: Hex32::from_bytes(*signed.claimant_account()),
        aggregate_authority_account_id: Hex32::from_bytes(*signed.aggregate_authority_account()),
        aggregate_x_only_public_key: Hex32::from_bytes(
            agreement.p2tr_contract().aggregate_internal_key_bytes(),
        ),
        amount: signed.amount(),
        refund_at_ms: signed.refund_at_ms(),
        authenticated_transfer_program_id: Hex32::from_bytes(
            *signed.authenticated_transfer_program_id(),
        ),
    })
    .map_err(|_| ActorCommandError::AgreementBindingInvalid)
}

#[derive(Serialize)]
struct FinalizedLezFundingRequestIdentityV1<'a> {
    schema_version: u16,
    run_id: &'a RunId,
    sidecar_role: BridgeParticipant,
    runtime: &'a RuntimeDescriptor,
    terms: &'a WitnessedNativeEscrowTerms,
    target: &'static str,
    window: DiscoveryWindow,
}

fn bridge_participant(participant: Participant) -> BridgeParticipant {
    match participant {
        Participant::Maker => BridgeParticipant::Maker,
        Participant::Taker => BridgeParticipant::Taker,
    }
}

fn load_agreement(config: &ActorConfig) -> Result<(BtcAgreementV1, Vec<u8>), ActorCommandError> {
    let wire = read_stable_file(
        &config.agreement_file,
        MAX_BTC_AGREEMENT_RECORD_BYTES,
        false,
    )
    .map_err(|()| ActorCommandError::AgreementUnavailable)?;
    let agreement =
        BtcAgreementV1::from_wire(&wire).map_err(|_| ActorCommandError::AgreementUnavailable)?;
    validate_actor_binding(config, &agreement)?;
    Ok((agreement, wire))
}

fn validate_actor_binding(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
) -> Result<(), ActorCommandError> {
    let runtime = &config.lez_bridge.runtime;
    let signed = agreement.lez_terms();
    let expected_signer = agreement.participant(config.role.sdk()).lez_owner_account();
    if runtime.sidecar_role != config.role.bridge()
        || runtime.compatibility != RuntimeCompatibility::LeeV0_2_0
        || runtime.channel_id.as_bytes() != signed.channel_id()
        || runtime.genesis_block_hash.as_bytes() != signed.genesis_block_hash()
        || runtime.escrow_program_id.as_bytes() != signed.escrow_program_id()
        || runtime.signer_account_id.as_bytes() != expected_signer
    {
        return Err(ActorCommandError::AgreementBindingInvalid);
    }
    Ok(())
}

fn open_store(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    agreement_wire: Vec<u8>,
) -> Result<SqliteBtcRecoveryStore, ActorCommandError> {
    let acceptance = actor_acceptance(config, agreement, agreement_wire)
        .map_err(|_| ActorCommandError::AgreementBindingInvalid)?;
    SqliteBtcRecoveryStore::open(&config.state_db, &acceptance, agreement.coordinator())
        .map_err(|_| ActorCommandError::StateUnavailable)
}

fn open_existing_store(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    agreement_wire: Vec<u8>,
) -> Result<SqliteBtcRecoveryStore, BtcRecoveryError> {
    let acceptance = actor_acceptance(config, agreement, agreement_wire)?;
    SqliteBtcRecoveryStore::open_existing(&config.state_db, &acceptance, agreement.coordinator())
}

fn actor_acceptance(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    agreement_wire: Vec<u8>,
) -> Result<BtcAgreementAcceptance, BtcRecoveryError> {
    let acceptance = BtcAgreementAcceptance::new(
        agreement.coordinator(),
        config.role.sdk(),
        agreement_wire,
        *agreement.agreement_commitment(),
        config.accepted_at_unix_seconds,
    )?;
    Ok(acceptance)
}

fn effect_output(
    config: &ActorConfig,
    command: ActorEffectCommandV1,
    outcome: ActorEffectOutcomeV1,
    status: &BtcOfflineStatus,
) -> ActorEffectOutputV1 {
    ActorEffectOutputV1 {
        schema_version: OUTPUT_SCHEMA_VERSION,
        role: config.role,
        command,
        outcome,
        phase: status.phase().into(),
        revision: status.revision(),
    }
}

fn status_output(config: &ActorConfig, status: &BtcOfflineStatus) -> ActorStatusV1 {
    let next_action = if status.terminal().is_some() {
        ActorNextActionV1::Complete
    } else if status.revision() == 0 {
        ActorNextActionV1::ObserveTakerFirstLock
    } else if status.revision() == 1 {
        ActorNextActionV1::ObserveMakerSecondLockOrRecoverTakerLeg
    } else if status.revision() == 2 {
        ActorNextActionV1::ObserveRevealingClaim
    } else if status.revision() == 3 && status.phase() == Phase::ClaimEvidenceAvailable {
        ActorNextActionV1::ObserveFollowupClaim
    } else if status.revision() == 3 && status.phase() == Phase::MakerLegRefunded {
        ActorNextActionV1::RecoverTakerLeg
    } else {
        ActorNextActionV1::LaterRevisionNotYetComposed
    };
    ActorStatusV1 {
        schema_version: OUTPUT_SCHEMA_VERSION,
        role: config.role,
        state: ActorStateV1::Active {
            phase: status.phase().into(),
            revision: status.revision(),
            next_action,
        },
    }
}

fn not_activated_status(config: &ActorConfig) -> ActorStatusV1 {
    ActorStatusV1 {
        schema_version: OUTPUT_SCHEMA_VERSION,
        role: config.role,
        state: ActorStateV1::NotActivated,
    }
}

fn state_file_exists(path: &Path) -> Result<bool, ActorCommandError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Ok(_) | Err(_) => Err(ActorCommandError::StateUnavailable),
    }
}

fn decode_strict_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ActorConfigError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = T::deserialize(&mut deserializer).map_err(|_| ActorConfigError::Invalid)?;
    deserializer.end().map_err(|_| ActorConfigError::Invalid)?;
    Ok(value)
}

fn read_stable_file(path: &Path, maximum: usize, owner_private: bool) -> Result<Vec<u8>, ()> {
    let before = fs::symlink_metadata(path).map_err(|_| ())?;
    validate_file_metadata(&before, maximum, owner_private)?;
    let file = File::open(path).map_err(|_| ())?;
    let opened = file.metadata().map_err(|_| ())?;
    validate_file_metadata(&opened, maximum, owner_private)?;
    if !same_file(&before, &opened) {
        return Err(());
    }
    let mut bytes = Vec::with_capacity(maximum.saturating_add(1));
    file.take(u64::try_from(maximum).map_err(|_| ())?.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    let after = fs::symlink_metadata(path).map_err(|_| ())?;
    validate_file_metadata(&after, maximum, owner_private)?;
    if !same_file(&opened, &after) || bytes.is_empty() || bytes.len() > maximum {
        return Err(());
    }
    Ok(bytes)
}

fn read_private_adaptor_secret(path: &Path) -> Result<Zeroizing<[u8; 32]>, ()> {
    let bytes = read_stable_private_file(path, MAX_ADAPTOR_SECRET_FILE_BYTES)?;
    let encoded = bytes.strip_suffix(b"\n").unwrap_or(bytes.as_slice());
    if encoded.len() != 64
        || !encoded.iter().all(u8::is_ascii_hexdigit)
        || encoded.iter().any(u8::is_ascii_uppercase)
    {
        return Err(());
    }
    let mut secret = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(encoded, &mut secret[..]).map_err(|_| ())?;
    Ok(secret)
}

fn read_stable_private_file(path: &Path, maximum: usize) -> Result<Zeroizing<Vec<u8>>, ()> {
    let before = fs::symlink_metadata(path).map_err(|_| ())?;
    validate_file_metadata(&before, maximum, true)?;
    let file = File::open(path).map_err(|_| ())?;
    let opened = file.metadata().map_err(|_| ())?;
    validate_file_metadata(&opened, maximum, true)?;
    if !same_file(&before, &opened) {
        return Err(());
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(maximum.saturating_add(1)));
    file.take(u64::try_from(maximum).map_err(|_| ())?.saturating_add(1))
        .read_to_end(bytes.as_mut())
        .map_err(|_| ())?;
    let after = fs::symlink_metadata(path).map_err(|_| ())?;
    validate_file_metadata(&after, maximum, true)?;
    if !same_file(&opened, &after) || bytes.is_empty() || bytes.len() > maximum {
        return Err(());
    }
    Ok(bytes)
}

fn validate_file_metadata(
    metadata: &fs::Metadata,
    maximum: usize,
    owner_private: bool,
) -> Result<(), ()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    if !metadata.file_type().is_file()
        || metadata.len() == 0
        || metadata.len() > u64::try_from(maximum).map_err(|_| ())?
        || metadata.nlink() != 1
        || (owner_private && metadata.permissions().mode() & 0o7777 != 0o600)
    {
        return Err(());
    }
    Ok(())
}

fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

fn is_normalized_absolute(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => normalized.push(Path::new("/")),
            Component::Normal(part) => normalized.push(part),
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => return false,
        }
    }
    normalized == path
}

#[cfg(test)]
mod tests;
