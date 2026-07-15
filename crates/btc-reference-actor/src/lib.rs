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
use clap::{Parser, Subcommand};
use lez_bridge_adapter::{CapabilityFileBridgeClientFactory, FreshLezBridgeTransportFactory};
use lez_bridge_client::BridgeClient;
use lez_bridge_protocol::{
    ChainTip, DiscoveryWindow, EscrowState, Hex32, MessageContext,
    ObserveFinalizedWitnessedFundingRequest, Participant as BridgeParticipant, RequestId, RunId,
    RuntimeCompatibility, RuntimeDescriptor, WitnessedEscrowMetadataFacts,
    WitnessedNativeEscrowTerms, WitnessedNativeEscrowTermsInput,
};
use lez_btc_core_adapter::{
    BitcoinCoreAdapter, BitcoinCoreEvidenceV1, BitcoinCoreRpc, CoreConnectivityPolicy,
    FundingObservation, HttpBitcoinCoreConfig, HttpBitcoinCoreRpc,
};
use lez_btc_swap_sdk::{BtcAgreementV1, MAX_BTC_AGREEMENT_RECORD_BYTES};
use lez_swap_core::{Chain, Participant, Phase};
use lez_swap_store::{
    BtcAgreementAcceptance, BtcLifecycleEvidenceV1, BtcOfflineStatus, BtcRecoveryError,
    SqliteBtcRecoveryStore,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

const CONFIG_SCHEMA_VERSION: u16 = 1;
const OUTPUT_SCHEMA_VERSION: u16 = 1;
const MAX_CONFIG_BYTES: usize = 64 * 1024;
const MAX_REQUEST_TIMEOUT_MILLIS: u64 = 60_000;
const FINALIZED_LEZ_CONFIRMATION_UNITS: u32 = 1;

/// Exactly one lifecycle action performed by a fresh actor process.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Subcommand)]
pub enum ActorCommand {
    /// Validate and durably accept one countersigned agreement.
    Activate,
    /// Observe and project at most one eligible lifecycle transition.
    Drive,
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
        let paths = [
            &self.agreement_file,
            &self.state_db,
            &self.bitcoin_core.cookie_file,
            &self.lez_bridge.capability_file,
        ];
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
    /// A chain observation was unavailable, malformed, or contradicted signed terms.
    #[error("actor first-lock observation is unavailable")]
    ObservationUnavailable,
    /// Durable evidence projection failed or lost its predecessor CAS.
    #[error("actor evidence projection is unavailable")]
    ProjectionUnavailable,
}

/// Affirmative or pending result returned by one agreement-aware first-lock observer.
#[derive(Clone, Debug, Eq, PartialEq)]
enum FirstLockObservation {
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

/// Agreement-aware observation seam used by deterministic actor tests and live adapters.
#[async_trait]
trait FirstLockObservationPort: Send + Sync {
    /// Performs one bounded read-only observation.
    async fn observe(
        &self,
        agreement: &BtcAgreementV1,
    ) -> Result<FirstLockObservation, ActorCommandError>;
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
        ActorCommand::Drive => drive_live(config).await.map(ActorCommandOutputV1::Effect),
    }
}

fn activate(config: &ActorConfig) -> Result<ActorEffectOutputV1, ActorCommandError> {
    let (agreement, wire) = load_agreement(config)?;
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
    if durable.revision() != 0 {
        return Ok(effect_output(
            config,
            ActorEffectCommandV1::Drive,
            ActorEffectOutcomeV1::NotYetComposed {
                durable_revision: durable.revision(),
            },
            &durable,
        ));
    }
    let expected_chain = agreement.coordinator().funded_chain(Participant::Taker);
    match expected_chain {
        Chain::Bitcoin => {
            let core_config = HttpBitcoinCoreConfig::new(config.bitcoin_core.endpoint.clone())
                .and_then(|value| value.with_cookie_file(&config.bitcoin_core.cookie_file))
                .map_err(|_| ActorCommandError::ConfigurationUnavailable)?;
            let rpc = HttpBitcoinCoreRpc::connect(&core_config)
                .map_err(|_| ActorCommandError::ConfigurationUnavailable)?;
            let observer = BitcoinFirstLockObserver {
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
            let observer = LezFirstLockObserver { config, client };
            drive_with_observer(config, agreement, wire, &observer).await
        }
        Chain::Zcash | Chain::Monero => Err(ActorCommandError::AgreementBindingInvalid),
    }
}

/// Drives revision zero with an injected agreement-aware observer.
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
    observer: &dyn FirstLockObservationPort,
) -> Result<ActorEffectOutputV1, ActorCommandError> {
    if !state_file_exists(&config.state_db)? {
        return Err(ActorCommandError::NotActivated);
    }
    validate_actor_binding(config, &agreement)?;
    let mut store = match open_existing_store(config, &agreement, agreement_wire) {
        Ok(store) => store,
        Err(BtcRecoveryError::MissingAgreementAcceptance) => {
            return Err(ActorCommandError::NotActivated);
        }
        Err(_) => return Err(ActorCommandError::StateUnavailable),
    };
    let before = store
        .status()
        .map_err(|_| ActorCommandError::StateUnavailable)?;
    if before.revision() != 0 {
        return Ok(effect_output(
            config,
            ActorEffectCommandV1::Drive,
            ActorEffectOutcomeV1::NotYetComposed {
                durable_revision: before.revision(),
            },
            &before,
        ));
    }
    let expected_chain = agreement.coordinator().funded_chain(Participant::Taker);
    let observation = observer.observe(&agreement).await?;
    let (chain, transaction_id, confirmations, chain_evidence) = match observation {
        FirstLockObservation::Pending { chain } => {
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
        FirstLockObservation::Ready {
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
    let evidence =
        BtcLifecycleEvidenceV1::taker_lock(chain, transaction_id, confirmations, chain_evidence)
            .map_err(|_| ActorCommandError::ObservationUnavailable)?;
    let (outcome, after) = match store.project(0, &evidence) {
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
        Err(BtcRecoveryError::EvidenceConflict { revision: 1 }) => {
            let winner = store
                .status()
                .map_err(|_| ActorCommandError::StateUnavailable)?;
            if winner.revision() != 1 || winner.phase() != Phase::TakerLockConfirmed {
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

struct BitcoinFirstLockObserver<R> {
    adapter: BitcoinCoreAdapter<R>,
}

#[async_trait]
impl<R> FirstLockObservationPort for BitcoinFirstLockObserver<R>
where
    R: BitcoinCoreRpc + Send + Sync,
{
    async fn observe(
        &self,
        agreement: &BtcAgreementV1,
    ) -> Result<FirstLockObservation, ActorCommandError> {
        let observed = self
            .adapter
            .observe_funding(agreement)
            .await
            .map_err(|_| ActorCommandError::ObservationUnavailable)?;
        let FundingObservation::Ready(observed) = observed else {
            return Ok(FirstLockObservation::Pending {
                chain: Chain::Bitcoin,
            });
        };
        let evidence = BitcoinCoreEvidenceV1::funding_ready(agreement, &observed)
            .and_then(|value| value.encode())
            .map_err(|_| ActorCommandError::ObservationUnavailable)?;
        Ok(FirstLockObservation::Ready {
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

struct LezFirstLockObserver<'a> {
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
impl FirstLockObservationPort for LezFirstLockObserver<'_> {
    async fn observe(
        &self,
        agreement: &BtcAgreementV1,
    ) -> Result<FirstLockObservation, ActorCommandError> {
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
        Ok(FirstLockObservation::Ready {
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
    let signed = agreement.lez_terms();
    let terms = WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
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
    .map_err(|_| ActorCommandError::AgreementBindingInvalid)?;
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
    let identity_bytes =
        serde_json::to_vec(&identity).map_err(|_| ActorCommandError::ConfigurationUnavailable)?;
    let request_id = RequestId::new(hex::encode(Sha256::digest(identity_bytes)))
        .map_err(|_| ActorCommandError::ConfigurationUnavailable)?;
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
