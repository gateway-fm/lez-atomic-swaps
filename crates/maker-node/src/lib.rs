//! Authenticated local JSON-RPC boundary for the headless maker.

mod actor_supervisor;
mod btc_chat;
mod btc_lifecycle;
mod daemon_lifecycle;
mod logos_price_source;
mod price_source;
mod route_health;
mod rpc_contracts;
#[cfg(feature = "pair-xmr")]
mod xmr_chat;
#[cfg(feature = "pair-zec")]
mod zec_application;
#[cfg(feature = "pair-zec")]
mod zec_chat;

pub use actor_supervisor::{
    MakerActorSupervisorCancellation, MakerActorSupervisorConfig, MakerActorSupervisorError,
    MakerActorSupervisorOutcome, MakerActorSupervisorResolution, prepare_maker_actor,
    supervise_one_abandoned_maker_actor, supervise_one_abandoned_maker_actor_until,
    supervise_one_due_maker_actor, supervise_one_due_maker_actor_until,
};
use btc_chat::register_btc_chat_methods;
pub use btc_chat::{BtcMakerActorProvisioner, BtcMakerRoleAgreementAuthority};
pub use btc_lifecycle::BtcMakerLifecycle;
pub use daemon_lifecycle::{
    MakerDaemonHealth, MakerDaemonLaunchConfig, MakerDaemonLifecycle, MakerDaemonLifecycleError,
    ProcessMakerDaemon,
};
pub use lez_node_common::*;
pub use logos_price_source::ProcessLogosPriceSource;
pub use price_source::{LocalPriceSource, PriceQuoteV1, PriceSource, PriceSourceError};
pub use route_health::{ProcessRouteHealthProbe, RouteHealthProbeConfigError};
pub use rpc_contracts::{ListRequest, MakerDependencyStateV1, MakerHealthV1, MakerRouteHealthV1};
#[cfg(feature = "pair-xmr")]
pub use xmr_chat::XmrMakerChatAuthority;
#[cfg(feature = "pair-xmr")]
use xmr_chat::register_xmr_chat_methods;
#[cfg(feature = "pair-zec")]
pub use zec_application::{
    AppliedZcashFundingEvent, ZcashFundingApplyError, ZcashFundingProjectionOutcome,
    apply_zcash_funding_event, import_terminal_zec_maker_projection,
    load_zcash_observation_tracker,
};
#[cfg(feature = "pair-zec")]
pub use zec_chat::ZecMakerActorProvisioner;
#[cfg(feature = "pair-zec")]
use zec_chat::register_zec_chat_methods;

use std::{
    fs,
    os::unix::fs::{FileTypeExt as _, MetadataExt as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use btc_reference_actor::{
    ActorConfig as BtcActorConfig, ActorRole as BtcActorRole, provision_btc_maker_actor_from_config,
};
use jsonrpsee::{RpcModule, core::RpcResult, types::ErrorObjectOwned};
use lez_bridge_protocol::RequestId;
use lez_btc_swap_sdk::{
    BtcAgreementDraftV1, BtcAgreementV1, BtcMakerAgreementProposalV1, BtcRoleContributionPairV1,
    BtcRoleContributionV1, MAX_BTC_AGREEMENT_RECORD_BYTES, MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES,
    derive_btc_pre_session_id_v1,
};
use lez_swap_core::{
    Chain, ChainPosition, ClockBasis, ConfirmationPolicy, Error, Pair, Participant, Phase,
    RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{
    AlertObservedEvent, BtcAgreementAcceptance, LocalPriceV1, MakerActorKindV1,
    MakerActorManifestV1, MakerActorManualAction, MakerActorManualActionState,
    MakerActorProcessError, MakerActorProgressObservationV1, MakerActorScheduleState,
    MakerBtcNegotiationV1, MakerConfigurationCommit, MakerLocalRouteCommit, MakerOfferCommit,
    MakerOfferId, MakerOfferPublicationPreflight, MakerOfferRecordV1, MakerOfferStatus,
    MakerOfferV1, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1, OperatorAlert,
    OperatorAlertKind, OperatorAlertSeverity, SqliteSwapStore, StoreError, VersionedMakerRecord,
    maker_btc_chat_swap_id, validate_maker_actor_program,
};
#[cfg(feature = "pair-zec")]
use lez_swap_store::{
    EventCommit, MakerZecNegotiationV1, OperatorAlertRecordV1, OperatorTerminalProjectionCommit,
    SqliteZecRecoveryStore, maker_zec_chat_session_id,
};
#[cfg(feature = "pair-xmr")]
use lez_swap_store::{
    MakerXmrActivationAcceptance, MakerXmrNegotiationStatus, MakerXmrNegotiationV1,
    maker_xmr_chat_swap_id,
};
#[cfg(feature = "pair-xmr")]
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES, MoneroPrivateViewKey,
    XmrActivatedAgreementV1, XmrAgreementV1, XmrRoleV1, XmrSwapDirectionV1,
};
#[cfg(feature = "pair-zec")]
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, ClaimPreimage, HistoricalReplayError, ProtectedClaimKey,
    ValidatedZecAgreementDraftV1, ZcashObservationEvent, ZcashObservationEventRecordV1,
    ZcashObservationTracker, ZecAgreementDraftV1, ZecBindingRecordError, ZecPairSdk,
    ZecRefundProfile, ZecSwapBinding, replay_zcash_observation_history,
};
use secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};

use sha2::{Digest as _, Sha256};
#[cfg(feature = "pair-zec")]
use zec_reference_actor::{ActorConfig, ActorRole, provision_zec_maker_actor_from_config};
const NOT_FOUND: i32 = -32_004;
const CONFLICT: i32 = -32_009;
const RESULT_LIMIT_EXCEEDED: i32 = -32_011;
const OFFER_UNAVAILABLE: i32 = -32_018;
const INTERNAL_ERROR: i32 = -32_603;

const MAXIMUM_LOGOS_OFFER_SNAPSHOT_PAGE_ENTRIES: usize = 128;
const MAXIMUM_LOGOS_OFFER_SNAPSHOT_PAYLOAD_BYTES: usize = 48 * 1024;
const _: () = assert!(
    run_local_delivery::MAXIMUM_LOGOS_OFFER_ANNOUNCEMENT_BASE64_BYTES + 3
        <= MAXIMUM_LOGOS_OFFER_SNAPSHOT_PAYLOAD_BYTES
);
/// RPC context owned by one maker daemon.
#[derive(Clone)]
pub struct MakerRpc {
    store: Arc<Mutex<SqliteSwapStore>>,
    route_health_probe: Option<Arc<dyn MakerRouteHealthProbe>>,
    logos_price_source: Option<Arc<ProcessLogosPriceSource>>,
    delivery: Option<Arc<RunLocalDelivery>>,
    offer_snapshot_clock: Option<Arc<dyn Fn() -> RpcResult<u64> + Send + Sync>>,
    chat_socket: Option<Arc<PathBuf>>,
    chat_signing_key: Option<Arc<SecretKey>>,
    btc_chat_signing_key: Option<Arc<SecretKey>>,
    btc_actor_provisioner: Option<Arc<BtcMakerActorProvisioner>>,
    btc_role_agreement_authority: Option<Arc<BtcMakerRoleAgreementAuthority>>,
    /// Node-owned Bitcoin lifecycle (ADR 0213); supersedes the fixture paths.
    btc_lifecycle: Option<Arc<BtcMakerLifecycle>>,
    #[cfg(feature = "pair-xmr")]
    xmr_chat_authority: Option<Arc<XmrMakerChatAuthority>>,
    #[cfg(feature = "pair-zec")]
    zec_completion_store: Option<Arc<SqliteZecRecoveryStore>>,
    #[cfg(feature = "pair-zec")]
    maker_claim_preimage: Option<Arc<ClaimPreimage>>,
    #[cfg(feature = "pair-zec")]
    zec_actor_provisioner: Option<Arc<ZecMakerActorProvisioner>>,
}

impl std::fmt::Debug for MakerRpc {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("MakerRpc");
        debug
            .field("store", &self.store)
            .field(
                "route_health_probe",
                &self.route_health_probe.as_ref().map(|_| "configured"),
            )
            .field(
                "logos_price_source",
                &self.logos_price_source.as_ref().map(|_| "configured"),
            )
            .field("delivery", &self.delivery)
            .field(
                "offer_snapshot_clock",
                &self.offer_snapshot_clock.as_ref().map(|_| "configured"),
            )
            .field("chat_socket", &self.chat_socket)
            .field(
                "chat_signing_key",
                &self.chat_signing_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "btc_chat_signing_key",
                &self.btc_chat_signing_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "btc_actor_provisioner",
                &self.btc_actor_provisioner.as_ref().map(|_| "configured"),
            )
            .field(
                "btc_role_agreement_authority",
                &self
                    .btc_role_agreement_authority
                    .as_ref()
                    .map(|_| "configured"),
            )
            .field(
                "btc_lifecycle",
                &self.btc_lifecycle.as_ref().map(|_| "configured"),
            );
        #[cfg(feature = "pair-xmr")]
        debug.field(
            "xmr_chat_authority",
            &self.xmr_chat_authority.as_ref().map(|_| "configured"),
        );
        #[cfg(feature = "pair-zec")]
        debug
            .field(
                "zec_completion_store",
                &self.zec_completion_store.as_ref().map(|_| "configured"),
            )
            .field(
                "maker_claim_preimage",
                &self.maker_claim_preimage.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "zec_actor_provisioner",
                &self.zec_actor_provisioner.as_ref().map(|_| "configured"),
            );
        debug.finish()
    }
}

impl MakerRpc {
    /// Creates a maker RPC context. Transport authentication is configured by the daemon.
    #[must_use]
    pub fn new(store: SqliteSwapStore) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            route_health_probe: None,
            logos_price_source: None,
            delivery: None,
            offer_snapshot_clock: None,
            chat_socket: None,
            chat_signing_key: None,
            #[cfg(feature = "pair-zec")]
            zec_completion_store: None,
            btc_chat_signing_key: None,
            btc_actor_provisioner: None,
            btc_role_agreement_authority: None,
            btc_lifecycle: None,
            #[cfg(feature = "pair-xmr")]
            xmr_chat_authority: None,
            #[cfg(feature = "pair-zec")]
            maker_claim_preimage: None,
            #[cfg(feature = "pair-zec")]
            zec_actor_provisioner: None,
        }
    }

    /// Creates a shared Delivery and isolated-Chat transport without pair authority.
    #[must_use]
    pub fn with_delivery_transport(
        store: SqliteSwapStore,
        delivery: RunLocalDelivery,
        chat_signing_key: SecretKey,
    ) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            route_health_probe: None,
            logos_price_source: None,
            delivery: Some(Arc::new(delivery)),
            offer_snapshot_clock: None,
            chat_socket: None,
            chat_signing_key: Some(Arc::new(chat_signing_key)),
            btc_chat_signing_key: None,
            btc_actor_provisioner: None,
            btc_role_agreement_authority: None,
            btc_lifecycle: None,
            #[cfg(feature = "pair-xmr")]
            xmr_chat_authority: None,
            #[cfg(feature = "pair-zec")]
            zec_completion_store: None,
            #[cfg(feature = "pair-zec")]
            maker_claim_preimage: None,
            #[cfg(feature = "pair-zec")]
            zec_actor_provisioner: None,
        }
    }

    /// Injects the trusted time used only for signed offer snapshots.
    /// Production daemons retain the host clock; deterministic local tests use this seam.
    #[doc(hidden)]
    #[must_use]
    pub fn with_offer_snapshot_clock<F>(mut self, clock: F) -> Self
    where
        F: Fn() -> RpcResult<u64> + Send + Sync + 'static,
    {
        self.offer_snapshot_clock = Some(Arc::new(clock));
        self
    }

    #[cfg(feature = "pair-zec")]
    /// Attaches direction-aware ZEC agreement and Maker actor authority.
    ///
    /// The Maker preimage is absent when the accepted direction assigns that
    /// material to the Taker. Agreement completion rejects an absent preimage
    /// if and only if the signed agreement makes the Maker the LEZ claimant.
    #[must_use]
    pub fn with_directional_zec_chat_authority(
        mut self,
        completion_store: SqliteZecRecoveryStore,
        maker_claim_preimage: Option<ClaimPreimage>,
        actor_provisioner: Option<ZecMakerActorProvisioner>,
    ) -> Self {
        self.zec_completion_store = Some(Arc::new(completion_store));
        self.maker_claim_preimage = maker_claim_preimage.map(Arc::new);
        self.zec_actor_provisioner = actor_provisioner.map(Arc::new);
        self
    }

    /// Attaches fixture-independent BTC role-agreement authority, optionally
    /// retaining the legacy pre-finalized actor path for Chat v1.
    #[must_use]
    pub fn with_btc_chat_authorities(
        mut self,
        signing_key: SecretKey,
        actor_provisioner: Option<BtcMakerActorProvisioner>,
        role_agreement_authority: Option<BtcMakerRoleAgreementAuthority>,
    ) -> Self {
        self.btc_chat_signing_key = Some(Arc::new(signing_key));
        self.btc_actor_provisioner = actor_provisioner.map(Arc::new);
        self.btc_role_agreement_authority = role_agreement_authority.map(Arc::new);
        self
    }

    /// Attaches the Node-owned Bitcoin lifecycle (reservation, planning,
    /// ceremony, actor synthesis) for this Maker role.
    #[must_use]
    pub fn with_btc_lifecycle(mut self, lifecycle: BtcMakerLifecycle) -> Self {
        let lifecycle = Arc::new(lifecycle);
        lifecycle.spawn_sidecar_keepalive();
        self.btc_lifecycle = Some(lifecycle);
        self
    }

    #[cfg(feature = "pair-xmr")]
    /// Attaches daemon-owned Monero validation and actor authority.
    #[must_use]
    pub fn with_xmr_chat_authority(mut self, authority: XmrMakerChatAuthority) -> Self {
        self.xmr_chat_authority = Some(Arc::new(authority));
        self
    }

    /// Installs the bounded external-price process used only by Logos-configured routes.
    #[must_use]
    pub fn with_logos_price_source(mut self, source: ProcessLogosPriceSource) -> Self {
        self.logos_price_source = Some(Arc::new(source));
        self
    }

    /// Attaches the taker-facing Chat endpoint for read-only dependency health.
    #[must_use]
    pub fn with_chat_socket(mut self, socket: PathBuf) -> Self {
        self.chat_socket = Some(Arc::new(socket));
        self
    }

    /// Attaches the route-scoped chain dependency probe used for fail-closed publication.
    #[must_use]
    pub fn with_route_health_probe(mut self, probe: Arc<dyn MakerRouteHealthProbe>) -> Self {
        self.route_health_probe = Some(probe);
        self
    }

    /// Returns true when automatic route-scoped chain health is configured.
    #[must_use]
    pub fn route_health_is_configured(&self) -> bool {
        self.route_health_probe.is_some()
    }

    /// Reconciles active advertisements against current route-scoped chain health.
    ///
    /// # Errors
    ///
    /// Fails when trusted time or durable offer reconciliation is unavailable.
    pub fn reconcile_route_health(&self) -> anyhow::Result<()> {
        let now_unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before Unix epoch")?
            .as_secs();
        reconcile_unhealthy_route_offers(self, now_unix_seconds)
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }
}

/// Synchronous, bounded route-health boundary supplied by chain-specific adapters.
///
/// The Maker owns policy and offer withdrawal; an implementation only reports whether the
/// already-configured chain dependency for one exact route can currently serve that route.
pub trait MakerRouteHealthProbe: Send + Sync {
    /// Returns the current availability of the selected route's chain dependency.
    fn state(&self, route: MakerRouteV1) -> MakerDependencyStateV1;
}

/// Serializable operator-facing snapshot. Secret evidence is deliberately omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapView {
    /// Stable swap identifier.
    pub id: Box<str>,
    /// Foreign-chain pair.
    pub pair: Pair,
    /// Which asset is funded first by the taker.
    pub direction: SwapDirection,
    /// Current durable phase.
    pub phase: Phase,
    /// Whether unacknowledged durable alerts require operator attention.
    pub requires_attention: bool,
    /// Number of unacknowledged durable alerts.
    pub pending_alerts: u32,
    /// Highest severity among unacknowledged alerts.
    pub highest_alert_severity: Option<OperatorAlertSeverity>,
}

impl From<&SwapCoordinator> for SwapView {
    fn from(swap: &SwapCoordinator) -> Self {
        Self {
            id: swap.id().as_str().into(),
            pair: swap.pair(),
            direction: swap.direction(),
            phase: swap.phase(),
            requires_attention: false,
            pending_alerts: 0,
            highest_alert_severity: None,
        }
    }
}

impl SwapView {
    fn with_pending_alerts(
        swap: &SwapCoordinator,
        alerts: &[OperatorAlert],
    ) -> Result<Self, StoreError> {
        let pending_alerts =
            u32::try_from(alerts.len()).map_err(|_| StoreError::RevisionOverflow)?;
        let highest_alert_severity = alerts
            .iter()
            .map(|alert| alert.record().severity())
            .max_by_key(|severity| match severity {
                OperatorAlertSeverity::Warning => 0,
                OperatorAlertSeverity::Critical => 1,
            });
        Ok(Self {
            id: swap.id().as_str().into(),
            pair: swap.pair(),
            direction: swap.direction(),
            phase: swap.phase(),
            requires_attention: pending_alerts > 0,
            pending_alerts,
            highest_alert_severity,
        })
    }
}

/// Non-secret operator view of one durable alert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorAlertView {
    /// Stable local alert cursor.
    pub sequence: u64,
    /// Event/aggregate revision that created the alert.
    pub aggregate_revision: u64,
    /// Stable semantic kind.
    pub kind: OperatorAlertKind,
    /// Stable operator severity.
    pub severity: OperatorAlertSeverity,
    /// Participant whose ZEC funding evidence changed.
    pub funded_by: Participant,
    /// Removal or replacement shape.
    pub observed_event: AlertObservedEvent,
    /// Exact detached funding transaction ID.
    pub previous_transaction_id: Box<str>,
    /// Newly canonical transaction ID for replacement alerts.
    pub canonical_transaction_id: Option<Box<str>>,
    /// Retained terminal phase for terminal-reorg alerts.
    pub terminal_phase: Option<Phase>,
    /// Whether the owner acknowledged seeing this alert.
    pub acknowledged: bool,
}

impl From<&OperatorAlert> for OperatorAlertView {
    fn from(alert: &OperatorAlert) -> Self {
        let record = alert.record();
        Self {
            sequence: alert.sequence(),
            aggregate_revision: alert.aggregate_revision(),
            kind: record.kind(),
            severity: record.severity(),
            funded_by: record.funded_by(),
            observed_event: record.observed_event(),
            previous_transaction_id: record.previous_transaction_id().into(),
            canonical_transaction_id: record.canonical_transaction_id().map(Into::into),
            terminal_phase: record.terminal_phase(),
            acknowledged: alert.acknowledged(),
        }
    }
}

/// Parameters for one atomic, idempotent local route and price mutation.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalRouteSaveRequest {
    /// Stable request identity used for exact replay of the complete operation.
    pub request_id: RequestId,
    /// Current pair-policy revision, or `None` for insert-only.
    pub expected_pair_revision: Option<u64>,
    /// Current local-price revision, or `None` for insert-only.
    pub expected_price_revision: Option<u64>,
    /// Fully validated policy using the local price source.
    pub configuration: MakerPairConfigurationV1,
    /// Exact integer price for the same route.
    pub price: LocalPriceV1,
}

/// Parameters for one idempotent maker pair-policy mutation.
#[derive(Debug, Deserialize, Serialize)]
pub struct PairConfigureRequest {
    /// Stable request identity used for exact replay.
    pub request_id: RequestId,
    /// Current route revision, or `None` for insert-only.
    pub expected_revision: Option<u64>,
    /// Fully validated versioned policy.
    pub configuration: MakerPairConfigurationV1,
}

/// Parameters for one idempotent local-price mutation.
#[derive(Debug, Deserialize, Serialize)]
pub struct LocalPriceSetRequest {
    /// Stable request identity used for exact replay.
    pub request_id: RequestId,
    /// Current price revision, or `None` for insert-only.
    pub expected_revision: Option<u64>,
    /// Exact integer lot ratio.
    pub price: LocalPriceV1,
}

/// Parameters for reading one route's currently selected price source.
#[derive(Debug, Deserialize, Serialize)]
pub struct PriceQuoteRequest {
    /// Exact pair and direction; another route is never substituted.
    pub route: MakerRouteV1,
}

/// Parameters for atomically publishing one offer from current local configuration.
#[derive(Debug, Deserialize, Serialize)]
pub struct OfferPublishRequest {
    /// Stable request identity used for exact replay.
    pub request_id: RequestId,
    /// New bounded offer identity.
    pub offer_id: MakerOfferId,
    /// Exact enabled local-price route.
    pub route: MakerRouteV1,
}

/// Parameters for withdrawing one unreserved offer.
#[derive(Debug, Deserialize, Serialize)]
pub struct OfferWithdrawRequest {
    /// Stable request identity used for exact replay.
    pub request_id: RequestId,
    /// Existing bounded offer identity.
    pub offer_id: MakerOfferId,
    /// Current offer revision.
    pub expected_revision: u64,
}

/// Owner-local request for one signed Delivery rebroadcast snapshot.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogosOfferAnnouncementSnapshotRequestV1 {
    /// Must be one for this DTO shape.
    pub schema_version: u16,
    /// Current app-lifetime Chat address returned by `chat_module.get_address`.
    pub maker_chat_address: Box<str>,
    /// Last offer identifier returned by the preceding page, if any.
    #[serde(default)]
    pub after_offer_id: Option<MakerOfferId>,
}

/// Bounded signed announcements ready for exact `delivery_module.send` calls.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogosOfferAnnouncementSnapshotV1 {
    /// This response schema version.
    pub schema_version: u16,
    /// Exact content topic that the Taker index subscribes to.
    pub content_topic: Box<str>,
    /// Maker refresh cadence; each announcement carries a longer signed lease.
    pub rebroadcast_after_seconds: u64,
    /// Canonical signed announcement bytes encoded as standard Base64.
    pub announcements_base64: Vec<Box<str>>,
    /// Cursor for the next transport-bounded page, absent on the final page.
    pub next_after_offer_id: Option<MakerOfferId>,
}

/// Parameters for creating one swap with already negotiated immutable terms.
#[derive(Debug, Deserialize, Serialize)]
pub struct CreateSwapRequest {
    /// Stable swap identifier.
    pub id: Box<str>,
    /// Foreign-chain pair.
    pub pair: Pair,
    /// Which asset the taker contributes.
    pub direction: SwapDirection,
    /// Confirmations required before maker lock.
    pub confirmations: u32,
    /// Pair-appropriate deadline or event-gated recovery terms.
    pub recovery: RecoveryRequest,
}

/// Operator-facing recovery terms. XMR cannot carry a fake Monero deadline.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecoveryRequest {
    /// Deadline-bearing BTC/ZEC recovery terms.
    Deadlines {
        /// Maker-leg refund clock basis.
        maker_refund_basis: ClockBasis,
        /// Maker-leg refund position.
        maker_refund_at: u64,
        /// Taker-leg refund clock basis.
        taker_refund_basis: ClockBasis,
        /// Taker-leg refund position.
        taker_refund_at: u64,
        /// Conservative latest Unix time for the construction's earlier refund.
        earlier_refund_latest: u64,
        /// Conservative earliest Unix time for the construction's later refund.
        later_refund_earliest: u64,
        /// Required user/chain reaction margin in seconds.
        required_margin: u64,
    },
    /// LEZ-first XMR terms whose maker recovery follows a canonical LEZ refund.
    XmrLezFirst {
        /// Taker's LEZ refund clock basis.
        taker_refund_basis: ClockBasis,
        /// Taker's LEZ refund position.
        taker_refund_at: u64,
        /// LEZ refund confirmations before Monero key-share recovery.
        refund_event_confirmations: u32,
    },
}

/// Parameters for reading one secret-free Maker actor lifecycle snapshot.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MakerActorMonitorRequestV1 {
    /// Stable application swap identity.
    pub id: Box<str>,
}

/// Parameters for one explicit, generation-fenced Maker actor action.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MakerActorActionRequestV1 {
    /// Stable idempotency identity for exact request replay.
    pub request_id: RequestId,
    /// Stable application swap identity.
    pub id: Box<str>,
    /// Actor generation observed by the operator before requesting the action.
    pub expected_generation: u64,
}

/// Allowlisted pair-actor kind exposed to the owner CLI.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MakerActorKindViewV1 {
    /// Bitcoin reference actor.
    Bitcoin,
    /// Monero reference actor.
    Monero,
    /// Zcash reference actor.
    Zcash,
}

impl From<MakerActorKindV1> for MakerActorKindViewV1 {
    fn from(kind: MakerActorKindV1) -> Self {
        match kind {
            MakerActorKindV1::Bitcoin => Self::Bitcoin,
            MakerActorKindV1::Monero => Self::Monero,
            MakerActorKindV1::Zcash => Self::Zcash,
        }
    }
}

/// Allowlisted process-scheduler state exposed to the owner CLI.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MakerActorScheduleStateViewV1 {
    /// Ready for a worker attempt.
    Queued,
    /// Owned by one generation-fenced worker.
    Leased,
    /// Waiting for a bounded retry.
    Backoff,
    /// Actor reported an absorbing protocol outcome.
    Terminal,
    /// Operator attention is required.
    Failed,
}

impl From<MakerActorScheduleState> for MakerActorScheduleStateViewV1 {
    fn from(state: MakerActorScheduleState) -> Self {
        match state {
            MakerActorScheduleState::Queued => Self::Queued,
            MakerActorScheduleState::Leased => Self::Leased,
            MakerActorScheduleState::Backoff => Self::Backoff,
            MakerActorScheduleState::Terminal => Self::Terminal,
            MakerActorScheduleState::Failed => Self::Failed,
        }
    }
}

/// Allowlisted validated actor progress exposed to the owner CLI.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerActorProgressViewV1 {
    /// Exact process generation that produced the observation.
    pub source_generation: u64,
    /// Pair-specific, schema-validated lifecycle observation.
    pub observation: MakerActorProgressObservationV1,
    /// Trusted application timestamp of the projection.
    pub observed_at: u64,
}

/// Allowlisted latest owner action exposed to the owner CLI.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerActorManualActionViewV1 {
    /// Stable idempotency identity.
    pub request_id: RequestId,
    /// Explicit claim or refund action.
    pub action: MakerActorManualAction,
    /// Current durable request state.
    pub state: MakerActorManualActionState,
    /// Actor generation against which the request was admitted.
    pub requested_after_generation: u64,
    /// Exact execution generation while leased.
    pub lease_generation: Option<u64>,
}

/// Secret-free, explicitly allowlisted Maker actor lifecycle response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerActorMonitorV1 {
    /// Response schema version; currently one.
    pub schema_version: u16,
    /// Stable application swap identity.
    pub swap_id: Box<str>,
    /// Pair actor implementation.
    pub actor_kind: MakerActorKindViewV1,
    /// Durable process-scheduler state.
    pub schedule_state: MakerActorScheduleStateViewV1,
    /// Current actor generation fence.
    pub lease_generation: u64,
    /// Number of bounded worker attempts.
    pub attempt_count: u64,
    /// Latest validated actor lifecycle projection, when available.
    pub progress: Option<MakerActorProgressViewV1>,
    /// Latest explicit owner action, when available.
    pub manual_action: Option<MakerActorManualActionViewV1>,
}

/// Durable admission result for one explicit Maker actor action.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerActorActionCommitV1 {
    /// Response schema version; currently one.
    pub schema_version: u16,
    /// Stable application swap identity.
    pub swap_id: Box<str>,
    /// Explicit claim or refund action.
    pub action: MakerActorManualAction,
    /// Generation against which the original request was admitted.
    pub requested_after_generation: u64,
    /// Whether this exact request and payload were already durable.
    pub was_replay: bool,
}

/// Parameters for reading one swap.
#[derive(Debug, Deserialize, Serialize)]
pub struct StatusRequest {
    /// Stable swap identifier.
    pub id: Box<str>,
}

/// Parameters for listing durable operator alerts.
#[derive(Debug, Deserialize, Serialize)]
pub struct AlertListRequest {
    /// Stable swap identifier.
    pub id: Box<str>,
    /// Return alerts with a sequence strictly greater than this cursor.
    pub after_sequence: u64,
    /// Include alerts already acknowledged by the owner.
    pub include_acknowledged: bool,
}

/// Parameters for acknowledging one durable operator alert.
#[derive(Debug, Deserialize, Serialize)]
pub struct AlertAcknowledgeRequest {
    /// Stable swap identifier.
    pub id: Box<str>,
    /// Stable alert cursor returned by `swap_alerts`.
    pub alert_sequence: u64,
}

/// Builds the RPC module shared by daemon transports and direct contract tests.
///
/// # Errors
///
/// Returns an error if a method cannot be registered.
pub fn rpc_module(context: MakerRpc) -> anyhow::Result<RpcModule<MakerRpc>> {
    let mut module = RpcModule::new(context);
    register_application_methods(&mut module)?;
    module.register_blocking_method::<RpcResult<SwapView>, _>(
        "swap_create",
        |params, context, _| {
            let request: CreateSwapRequest = params.one()?;

            let schedule = recovery_schedule(&request).map_err(invalid_request)?;
            let id = SwapId::new(request.id).map_err(invalid_request)?;
            let confirmations =
                ConfirmationPolicy::new(request.confirmations).map_err(invalid_request)?;
            let swap = SwapCoordinator::new_with_direction(
                id,
                request.pair,
                request.direction,
                confirmations,
                schedule,
            );
            let store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            if store
                .load(swap.id())
                .map_err(internal_store_error)?
                .is_some()
            {
                return Err(rpc_error(CONFLICT, "swap already exists"));
            }
            store.save(&swap).map_err(internal_store_error)?;
            Ok(SwapView::from(&swap))
        },
    )?;
    module.register_blocking_method::<RpcResult<SwapView>, _>(
        "swap_status",
        |params, context, _| {
            let request: StatusRequest = params.one()?;
            let id = SwapId::new(request.id).map_err(invalid_request)?;
            let store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            let swap = store
                .load_operator_swap(&id)
                .map_err(internal_store_error)?
                .ok_or_else(|| rpc_error(NOT_FOUND, "swap not found"))?;
            let alerts = store
                .list_operator_alerts(&id, 0, false)
                .map_err(internal_store_error)?;
            SwapView::with_pending_alerts(&swap, &alerts).map_err(internal_store_error)
        },
    )?;
    module.register_blocking_method::<RpcResult<Vec<OperatorAlertView>>, _>(
        "swap_alerts",
        |params, context, _| {
            let request: AlertListRequest = params.one()?;
            let id = SwapId::new(request.id).map_err(invalid_request)?;
            let store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            if store.load(&id).map_err(internal_store_error)?.is_none() {
                return Err(rpc_error(NOT_FOUND, "swap not found"));
            }
            store
                .list_operator_alerts(&id, request.after_sequence, request.include_acknowledged)
                .map(|alerts| alerts.iter().map(OperatorAlertView::from).collect())
                .map_err(internal_store_error)
        },
    )?;
    module.register_blocking_method::<RpcResult<SwapView>, _>(
        "swap_alert_acknowledge",
        |params, context, _| {
            let request: AlertAcknowledgeRequest = params.one()?;
            let id = SwapId::new(request.id).map_err(invalid_request)?;
            let store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            let swap = store
                .load(&id)
                .map_err(internal_store_error)?
                .ok_or_else(|| rpc_error(NOT_FOUND, "swap not found"))?;
            store
                .acknowledge_operator_alert(&id, request.alert_sequence)
                .map_err(|error| match error {
                    StoreError::MissingOperatorAlert => {
                        rpc_error(NOT_FOUND, "operator alert not found")
                    }
                    other => internal_store_error(other),
                })?;
            let alerts = store
                .list_operator_alerts(&id, 0, false)
                .map_err(internal_store_error)?;
            SwapView::with_pending_alerts(&swap, &alerts).map_err(internal_store_error)
        },
    )?;
    Ok(module)
}

fn register_application_methods(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
    register_health_method(module)?;
    register_pair_and_price_methods(module)?;
    register_offer_methods(module)?;
    register_maker_actor_methods(module)?;
    module.register_blocking_method::<RpcResult<Vec<SwapView>>, _>(
        "swap_history",
        |params, context, _| {
            let _: ListRequest = params.one()?;
            let store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            store
                .list_operator_swaps()
                .map_err(internal_store_error)?
                .iter()
                .map(|swap| {
                    let alerts = store
                        .list_operator_alerts(swap.id(), 0, false)
                        .map_err(internal_store_error)?;
                    SwapView::with_pending_alerts(swap, &alerts).map_err(internal_store_error)
                })
                .collect()
        },
    )?;
    Ok(())
}

fn register_maker_actor_methods(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
    module.register_blocking_method::<RpcResult<MakerActorMonitorV1>, _>(
        "maker_actor_monitor_v1",
        |params, context, _| {
            let request: MakerActorMonitorRequestV1 = params.one()?;
            let id = SwapId::new(request.id).map_err(invalid_request)?;
            let mut store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            let snapshot = store
                .maker_actor_monitor_snapshot(&id)
                .map_err(maker_actor_process_error)?
                .ok_or_else(|| rpc_error(NOT_FOUND, "maker actor not found"))?;
            let progress = snapshot
                .progress()
                .map(|snapshot| MakerActorProgressViewV1 {
                    source_generation: snapshot.source_generation(),
                    observation: snapshot.observation().clone(),
                    observed_at: snapshot.observed_at(),
                });
            let manual_action =
                snapshot
                    .manual_action()
                    .map(|snapshot| MakerActorManualActionViewV1 {
                        request_id: snapshot.request_id().clone(),
                        action: snapshot.action(),
                        state: snapshot.state(),
                        requested_after_generation: snapshot.requested_after_generation(),
                        lease_generation: snapshot.lease_generation(),
                    });
            let record = snapshot.process();
            Ok(MakerActorMonitorV1 {
                schema_version: 1,
                swap_id: record.swap_id().as_str().into(),
                actor_kind: record.manifest().kind().into(),
                schedule_state: record.schedule_state().into(),
                lease_generation: record.lease_generation(),
                attempt_count: record.attempt_count(),
                progress,
                manual_action,
            })
        },
    )?;
    register_maker_actor_action_method(
        module,
        "maker_actor_claim_v1",
        MakerActorManualAction::Claim,
    )?;
    register_maker_actor_action_method(
        module,
        "maker_actor_refund_v1",
        MakerActorManualAction::Refund,
    )?;
    Ok(())
}

fn register_maker_actor_action_method(
    module: &mut RpcModule<MakerRpc>,
    method: &'static str,
    action: MakerActorManualAction,
) -> anyhow::Result<()> {
    module.register_blocking_method::<RpcResult<MakerActorActionCommitV1>, _>(
        method,
        move |params, context, _| {
            let request: MakerActorActionRequestV1 = params.one()?;
            let id = SwapId::new(request.id).map_err(invalid_request)?;
            let now = trusted_now_unix_seconds()?;
            let mut store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            let _record = store
                .maker_actor_process(&id)
                .map_err(maker_actor_process_error)?
                .ok_or_else(|| rpc_error(NOT_FOUND, "maker actor not found"))?;
            let commit = store
                .queue_maker_actor_manual_action(
                    &request.request_id,
                    &id,
                    action,
                    request.expected_generation,
                    now,
                )
                .map_err(maker_actor_process_error)?;
            Ok(MakerActorActionCommitV1 {
                schema_version: 1,
                swap_id: id.as_str().into(),
                action,
                requested_after_generation: commit.requested_after_generation(),
                was_replay: commit.was_replay(),
            })
        },
    )?;
    Ok(())
}

fn maker_actor_process_error(error: MakerActorProcessError) -> ErrorObjectOwned {
    match error {
        MakerActorProcessError::ManualActionRequestConflict
        | MakerActorProcessError::ManualActionGenerationConflict
        | MakerActorProcessError::ManualActionPending => rpc_error(CONFLICT, error.to_string()),
        MakerActorProcessError::ManualActionUnavailable
        | MakerActorProcessError::InvalidManifest
        | MakerActorProcessError::PairMismatch
        | MakerActorProcessError::InvalidSchedulingInput => invalid_request(error),
        MakerActorProcessError::MissingSwap => rpc_error(NOT_FOUND, error.to_string()),
        other => internal_store_error(other),
    }
}
fn register_health_method(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
    module.register_blocking_method::<RpcResult<MakerHealthV1>, _>(
        "maker_health",
        |params, context, _| {
            let _: ListRequest = params.one()?;
            let now_unix_seconds = trusted_now_unix_seconds()?;
            reconcile_unhealthy_route_offers(&context, now_unix_seconds)?;
            let (active, routes) = {
                let store = context
                    .store
                    .lock()
                    .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
                let routes = store
                    .list_maker_pairs()
                    .map_err(application_store_error)?
                    .into_iter()
                    .map(|record| {
                        MakerRouteHealthV1::new(
                            record.value().route(),
                            route_dependency_state(&context, record.value().route()),
                        )
                    })
                    .collect::<Vec<_>>();
                let active = store
                    .list_retryable_maker_offers(now_unix_seconds)
                    .map_err(application_store_error)?
                    .into_iter()
                    .filter(|record| record.status() != MakerOfferStatus::Consumed)
                    .map(|record| record.offer().clone())
                    .collect::<Vec<_>>();
                (active, routes)
            };
            let delivery =
                context
                    .delivery
                    .as_ref()
                    .map_or(MakerDependencyStateV1::Disabled, |delivery| {
                        if delivery
                            .projection_health(&active, now_unix_seconds)
                            .is_ok()
                        {
                            MakerDependencyStateV1::Available
                        } else {
                            MakerDependencyStateV1::Unavailable
                        }
                    });
            let chat =
                context
                    .chat_socket
                    .as_ref()
                    .map_or(MakerDependencyStateV1::Disabled, |socket| {
                        if owner_socket_is_available(socket) {
                            MakerDependencyStateV1::Available
                        } else {
                            MakerDependencyStateV1::Unavailable
                        }
                    });
            Ok(MakerHealthV1::ready(delivery, chat, routes))
        },
    )?;
    Ok(())
}

fn route_dependency_state(context: &MakerRpc, route: MakerRouteV1) -> MakerDependencyStateV1 {
    let Some(probe) = context.route_health_probe.as_ref() else {
        return MakerDependencyStateV1::Disabled;
    };
    match probe.state(route) {
        MakerDependencyStateV1::Disabled => MakerDependencyStateV1::Unavailable,
        state => state,
    }
}

fn ensure_route_dependency_available(context: &MakerRpc, route: MakerRouteV1) -> RpcResult<()> {
    if route_dependency_state(context, route) == MakerDependencyStateV1::Unavailable {
        return Err(invalid_request("chain dependency is unavailable for route"));
    }
    Ok(())
}

fn route_health_withdrawal_request_id(record: &MakerOfferRecordV1) -> RequestId {
    let mut digest = Sha256::new();
    digest.update(b"lez-maker-route-health-withdraw-v1\0");
    digest.update(record.offer().id().as_str().as_bytes());
    digest.update(record.revision().to_be_bytes());
    RequestId::new(hex::encode(digest.finalize())).expect("SHA-256 is a valid request ID")
}

fn reconcile_unhealthy_route_offers(context: &MakerRpc, now_unix_seconds: u64) -> RpcResult<()> {
    let active = {
        let store = context
            .store
            .lock()
            .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
        store
            .list_discoverable_maker_offers(now_unix_seconds)
            .map_err(application_store_error)?
    };
    let mut route_states = Vec::new();
    for record in &active {
        let route = record.offer().route();
        if !route_states.iter().any(|(observed, _)| *observed == route) {
            route_states.push((route, route_dependency_state(context, route)));
        }
    }
    for record in active {
        if !route_states.iter().any(|(route, state)| {
            *route == record.offer().route() && *state == MakerDependencyStateV1::Unavailable
        }) {
            continue;
        }
        let withdrew = {
            let mut store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            match store.withdraw_maker_offer(
                &route_health_withdrawal_request_id(&record),
                record.offer().id(),
                record.revision(),
            ) {
                Ok(_) => true,
                Err(StoreError::MakerOfferUnavailable | StoreError::StaleMakerOffer { .. }) => {
                    false
                }
                Err(error) => return Err(application_store_error(error)),
            }
        };
        if withdrew && let Some(delivery) = &context.delivery {
            let _ = delivery.withdraw(record.offer().id());
        }
    }
    if let Some(delivery) = &context.delivery {
        let projected = {
            let store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            store
                .list_retryable_maker_offers(now_unix_seconds)
                .map_err(application_store_error)?
                .into_iter()
                .filter(|record| record.status() != MakerOfferStatus::Consumed)
                .map(|record| record.offer().clone())
                .collect::<Vec<_>>()
        };
        // Durable state remains authoritative. Failed cleanup is retried on the next sample;
        // Delivery health stays degraded until its exact projection matches this set. A
        // consumed lot is not projected: it is bound to one swap, and a Taker retrying its
        // acceptance replays from the store rather than rediscovering the offer.
        let _ = delivery.reconcile(&projected, now_unix_seconds);
    }
    Ok(())
}

fn owner_socket_is_available(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.file_type().is_socket()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.mode() & 0o7777 == 0o600
    })
}

fn register_pair_and_price_methods(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
    module.register_blocking_method::<RpcResult<MakerLocalRouteCommit>, _>(
        "maker_local_route_save_v1",
        |params, context, _| {
            let request: LocalRouteSaveRequest = params.one()?;
            let mut store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            store
                .save_local_maker_route(
                    &request.request_id,
                    request.expected_pair_revision,
                    request.expected_price_revision,
                    &request.configuration,
                    &request.price,
                )
                .map_err(application_store_error)
        },
    )?;
    module.register_blocking_method::<RpcResult<MakerConfigurationCommit>, _>(
        "maker_pair_configure",
        |params, context, _| {
            let request: PairConfigureRequest = params.one()?;
            let mut store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            store
                .configure_maker_pair(
                    &request.request_id,
                    request.expected_revision,
                    &request.configuration,
                )
                .map_err(application_store_error)
        },
    )?;
    module.register_blocking_method::<
        RpcResult<Vec<VersionedMakerRecord<MakerPairConfigurationV1>>>,
        _,
    >("maker_pair_list", |params, context, _| {
        let _: ListRequest = params.one()?;
        let store = context
            .store
            .lock()
            .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
        store.list_maker_pairs().map_err(application_store_error)
    })?;
    module.register_blocking_method::<RpcResult<MakerConfigurationCommit>, _>(
        "maker_local_price_set",
        |params, context, _| {
            let request: LocalPriceSetRequest = params.one()?;
            let mut store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            store
                .set_local_price(
                    &request.request_id,
                    request.expected_revision,
                    &request.price,
                )
                .map_err(application_store_error)
        },
    )?;
    module.register_blocking_method::<RpcResult<Vec<VersionedMakerRecord<LocalPriceV1>>>, _>(
        "maker_local_price_list",
        |params, context, _| {
            let _: ListRequest = params.one()?;
            let store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            store.list_local_prices().map_err(application_store_error)
        },
    )?;
    module.register_blocking_method::<RpcResult<PriceQuoteV1>, _>(
        "maker_price_quote",
        |params, context, _| {
            let request: PriceQuoteRequest = params.one()?;
            let observed_at_unix_seconds = trusted_now_unix_seconds()?;
            ensure_route_dependency_available(&context, request.route)?;
            quote_selected_price_source(&context, request.route, observed_at_unix_seconds)
        },
    )?;
    Ok(())
}

fn offer_announcement_snapshot(
    context: &MakerRpc,
    request: &LogosOfferAnnouncementSnapshotRequestV1,
) -> RpcResult<LogosOfferAnnouncementSnapshotV1> {
    if request.schema_version != 1 {
        return Err(invalid_request("unsupported offer announcement snapshot"));
    }
    let delivery = context
        .delivery
        .as_ref()
        .ok_or_else(|| rpc_error(INTERNAL_ERROR, "maker Delivery signer is unavailable"))?;
    let now_unix_seconds = if let Some(clock) = context.offer_snapshot_clock.as_ref() {
        clock()?
    } else {
        trusted_now_unix_seconds()?
    };
    let mut records = context
        .store
        .lock()
        .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?
        .list_retryable_maker_offers(now_unix_seconds)
        .map_err(application_store_error)?;
    records.sort_by(|left, right| left.offer().id().as_str().cmp(right.offer().id().as_str()));
    let records = records
        .iter()
        .filter(|record| {
            (record.status() != MakerOfferStatus::Active
                || record.offer().created_at_unix_seconds() <= now_unix_seconds)
                && request
                    .after_offer_id
                    .as_ref()
                    .is_none_or(|cursor| record.offer().id().as_str() > cursor.as_str())
        })
        .collect::<Vec<_>>();
    let mut announcements_base64 = Vec::new();
    let mut payload_bytes = 0_usize;
    for record in records
        .iter()
        .take(MAXIMUM_LOGOS_OFFER_SNAPSHOT_PAGE_ENTRIES)
    {
        let encoded = delivery
            .sign_logos_offer_announcement(record, &request.maker_chat_address, now_unix_seconds)
            .map_err(|error| delivery_error(&error))?;
        let announcement = BASE64_STANDARD.encode(encoded).into_boxed_str();
        let next_payload_bytes = payload_bytes
            .checked_add(announcement.len())
            .and_then(|bytes| bytes.checked_add(3))
            .ok_or_else(|| rpc_error(RESULT_LIMIT_EXCEEDED, "offer snapshot page exceeds limit"))?;
        if next_payload_bytes > MAXIMUM_LOGOS_OFFER_SNAPSHOT_PAYLOAD_BYTES {
            if announcements_base64.is_empty() {
                return Err(rpc_error(
                    RESULT_LIMIT_EXCEEDED,
                    "one signed offer announcement exceeds the owner RPC page budget",
                ));
            }
            break;
        }
        payload_bytes = next_payload_bytes;
        announcements_base64.push(announcement);
    }
    let next_after_offer_id = if announcements_base64.len() < records.len() {
        let last_index = announcements_base64.len().saturating_sub(1);
        Some(records[last_index].offer().id().clone())
    } else {
        None
    };
    Ok(LogosOfferAnnouncementSnapshotV1 {
        schema_version: 1,
        content_topic: LOGOS_OFFER_CONTENT_TOPIC_V1.into(),
        rebroadcast_after_seconds: LOGOS_OFFER_REBROADCAST_SECONDS_V1,
        announcements_base64,
        next_after_offer_id,
    })
}

fn register_offer_methods(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
    module.register_blocking_method::<RpcResult<MakerOfferCommit>, _>(
        "maker_offer_publish",
        |params, context, _| {
            let request: OfferPublishRequest = params.one()?;
            ensure_route_dependency_available(&context, request.route)?;
            let offer_id = request.offer_id.clone();
            let now_unix_seconds = trusted_now_unix_seconds()?;
            let commit = publish_offer(&context, &request, now_unix_seconds)?;
            if let Some(delivery) = &context.delivery {
                let delivery_now_unix_seconds = trusted_now_unix_seconds()?;
                let active_offer = {
                    let store = context
                        .store
                        .lock()
                        .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
                    store
                        .list_maker_offer_history(delivery_now_unix_seconds)
                        .map_err(application_store_error)?
                        .into_iter()
                        .find(|record| {
                            record.offer().id() == &offer_id
                                && record.status() == MakerOfferStatus::Active
                        })
                        .map(|record| record.offer().clone())
                };
                if let Some(offer) = active_offer {
                    delivery
                        .publish_or_verify(&DeliveryPublicationV1::new(
                            offer,
                            delivery_now_unix_seconds,
                        ))
                        .map_err(|error| delivery_error(&error))?;
                }
            }
            Ok(commit)
        },
    )?;
    module.register_blocking_method::<RpcResult<Vec<MakerOfferRecordV1>>, _>(
        "maker_offer_list",
        |params, context, _| {
            let _: ListRequest = params.one()?;
            let now_unix_seconds = trusted_now_unix_seconds()?;
            let store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            store
                .list_maker_offer_history(now_unix_seconds)
                .map_err(application_store_error)
        },
    )?;
    module.register_blocking_method::<RpcResult<LogosOfferAnnouncementSnapshotV1>, _>(
        "maker_offer_announcement_snapshot_v1",
        |params, context, _| {
            let request: LogosOfferAnnouncementSnapshotRequestV1 = params.one()?;
            offer_announcement_snapshot(context.as_ref(), &request)
        },
    )?;
    module.register_blocking_method::<RpcResult<MakerOfferCommit>, _>(
        "maker_offer_withdraw",
        |params, context, _| {
            let request: OfferWithdrawRequest = params.one()?;
            let mut store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            let commit = store
                .withdraw_maker_offer(
                    &request.request_id,
                    &request.offer_id,
                    request.expected_revision,
                )
                .map_err(application_store_error)?;
            if let Some(delivery) = &context.delivery {
                delivery
                    .withdraw(&request.offer_id)
                    .map_err(|error| delivery_error(&error))?;
            }
            Ok(commit)
        },
    )?;
    Ok(())
}

fn quote_selected_price_source(
    context: &MakerRpc,
    route: MakerRouteV1,
    now_unix_seconds: u64,
) -> RpcResult<PriceQuoteV1> {
    let store = context
        .store
        .lock()
        .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
    let configuration = store
        .list_maker_pairs()
        .map_err(application_store_error)?
        .into_iter()
        .find(|record| record.value().route() == route)
        .ok_or_else(|| application_store_error(StoreError::MissingMakerPair))?;
    if !configuration.value().enabled() {
        return Err(application_store_error(StoreError::MakerRouteDisabled));
    }
    match configuration.value().price_source() {
        MakerPriceSourceKind::Local => LocalPriceSource::new(&store)
            .quote(route, now_unix_seconds)
            .map_err(price_source_error),
        MakerPriceSourceKind::LogosCApi => {
            let source = context
                .logos_price_source
                .clone()
                .ok_or_else(|| price_source_error(PriceSourceError::UnavailableQuote))?;
            drop(store);
            source
                .quote(route, now_unix_seconds)
                .map_err(price_source_error)
        }
    }
}

fn publish_offer(
    context: &MakerRpc,
    request: &OfferPublishRequest,
    now_unix_seconds: u64,
) -> RpcResult<MakerOfferCommit> {
    let preflight = {
        let mut store = context
            .store
            .lock()
            .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
        store
            .prepare_maker_offer_publication(&request.request_id, &request.offer_id, request.route)
            .map_err(application_store_error)?
    };
    match preflight {
        MakerOfferPublicationPreflight::Replayed(commit) => Ok(commit),
        MakerOfferPublicationPreflight::Pending {
            price_source: MakerPriceSourceKind::Local,
            ..
        } => {
            let mut store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            store
                .publish_local_offer(
                    &request.request_id,
                    &request.offer_id,
                    request.route,
                    now_unix_seconds,
                )
                .map_err(application_store_error)
        }
        MakerOfferPublicationPreflight::Pending {
            pair_configuration_revision,
            price_source: MakerPriceSourceKind::LogosCApi,
        } => {
            let source = context
                .logos_price_source
                .clone()
                .ok_or_else(|| price_source_error(PriceSourceError::UnavailableQuote))?;
            let quote = source
                .quote(request.route, now_unix_seconds)
                .map_err(price_source_error)?;
            let commit_time = trusted_now_unix_seconds()?;
            let mut store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            store
                .publish_logos_offer(
                    &request.request_id,
                    &request.offer_id,
                    request.route,
                    pair_configuration_revision,
                    quote.price(),
                    source.source_identity_sha256(),
                    quote.source_revision(),
                    quote.observed_at_unix_seconds(),
                    commit_time,
                    source.max_age_seconds(),
                )
                .map_err(application_store_error)
        }
    }
}

fn delivery_error(error: &RunLocalDeliveryError) -> ErrorObjectOwned {
    rpc_error(INTERNAL_ERROR, error.to_string())
}

/// Builds the taker-facing Chat module without registering owner-control methods.
///
/// # Errors
///
/// Returns an error if the bounded Chat method cannot be registered.
pub fn chat_rpc_module(context: MakerRpc) -> anyhow::Result<RpcModule<MakerRpc>> {
    let mut module = RpcModule::new(context);
    register_chat_methods(&mut module)?;
    Ok(module)
}

fn register_chat_methods(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
    #[cfg(feature = "pair-zec")]
    register_zec_chat_methods(module)?;
    register_btc_chat_methods(module)?;
    #[cfg(feature = "pair-xmr")]
    register_xmr_chat_methods(module)?;
    Ok(())
}

fn recovery_schedule(request: &CreateSwapRequest) -> Result<RecoverySchedule, Error> {
    match (&request.recovery, request.pair, request.direction) {
        (_, Pair::Monero, SwapDirection::TakerSellsForeign) => Err(Error::UnsupportedDirection {
            pair: request.pair,
            direction: request.direction,
        }),
        (
            RecoveryRequest::XmrLezFirst {
                taker_refund_basis,
                taker_refund_at,
                refund_event_confirmations,
            },
            Pair::Monero,
            SwapDirection::TakerSellsLez,
        ) => RecoverySchedule::xmr_lez_first(
            position(Chain::Lez, *taker_refund_basis, *taker_refund_at),
            *refund_event_confirmations,
        ),
        (RecoveryRequest::XmrLezFirst { .. }, _, _) => Err(Error::RecoveryRequiresTakerRefundEvent),
        (
            RecoveryRequest::Deadlines {
                maker_refund_basis,
                maker_refund_at,
                taker_refund_basis,
                taker_refund_at,
                earlier_refund_latest,
                later_refund_earliest,
                required_margin,
            },
            pair,
            direction,
        ) => {
            let foreign = Chain::from(pair);
            let role_chains = match direction {
                SwapDirection::TakerSellsForeign => [Chain::Lez, foreign],
                SwapDirection::TakerSellsLez => [foreign, Chain::Lez],
            };
            let safety_chains = match pair {
                Pair::Bitcoin => role_chains,
                Pair::Zcash => [Chain::Lez, Chain::Zcash],
                Pair::Monero => unreachable!("Monero uses event-gated recovery"),
            };
            RecoverySchedule::new(
                pair,
                direction,
                position(role_chains[0], *maker_refund_basis, *maker_refund_at),
                position(role_chains[1], *taker_refund_basis, *taker_refund_at),
                TimelockSafety::between(
                    safety_chains[0],
                    safety_chains[1],
                    *earlier_refund_latest,
                    *later_refund_earliest,
                    *required_margin,
                )?,
            )
        }
    }
}

fn position(chain: Chain, basis: ClockBasis, value: u64) -> ChainPosition {
    match basis {
        ClockBasis::BlockHeight => ChainPosition::block_height(chain, value),
        ClockBasis::Timestamp => ChainPosition::timestamp(chain, value),
    }
}

fn invalid_request(error: impl std::fmt::Display) -> ErrorObjectOwned {
    rpc_error(-32_602, error.to_string())
}

fn trusted_now_unix_seconds() -> RpcResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| rpc_error(INTERNAL_ERROR, "system clock is before Unix epoch"))
}

fn application_store_error(error: StoreError) -> ErrorObjectOwned {
    match error {
        StoreError::MakerConfigurationRequestConflict
        | StoreError::StaleMakerConfiguration { .. }
        | StoreError::MakerOfferRequestConflict
        | StoreError::MakerOfferAlreadyExists
        | StoreError::MakerPriceRevisionRollback
        | StoreError::MakerPriceRevisionConflict
        | StoreError::StaleMakerOffer { .. } => rpc_error(CONFLICT, error.to_string()),
        StoreError::MissingMakerOffer => rpc_error(NOT_FOUND, error.to_string()),
        StoreError::MakerOfferExpired
        | StoreError::MakerOfferUnavailable
        | StoreError::MakerOfferReservationConflict => {
            rpc_error(OFFER_UNAVAILABLE, error.to_string())
        }
        StoreError::MakerConfiguration(_)
        | StoreError::MakerOffer(_)
        | StoreError::MissingMakerPair
        | StoreError::MissingMakerLocalPrice
        | StoreError::MakerLocalRouteMismatch
        | StoreError::MakerPriceSourceMismatch
        | StoreError::MakerRouteDisabled
        | StoreError::MakerOfferSwapMismatch => invalid_request(error),
        other => internal_store_error(other),
    }
}

fn price_source_error(error: PriceSourceError) -> ErrorObjectOwned {
    match error {
        PriceSourceError::MissingQuote => rpc_error(NOT_FOUND, error.to_string()),
        PriceSourceError::Store(error) => internal_store_error(error),
        PriceSourceError::DuplicateQuote | PriceSourceError::InvalidSource => {
            rpc_error(INTERNAL_ERROR, error.to_string())
        }
        PriceSourceError::UnavailableQuote | PriceSourceError::SourceTimeout => {
            rpc_error(-32_003, error.to_string())
        }
    }
}

fn internal_store_error(error: impl std::fmt::Display) -> ErrorObjectOwned {
    rpc_error(INTERNAL_ERROR, format!("swap store failure: {error}"))
}

fn rpc_error(code: i32, message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(code, message.into(), None::<()>)
}
