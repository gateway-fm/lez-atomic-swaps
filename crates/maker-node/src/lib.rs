//! Authenticated local JSON-RPC boundary for the headless maker.

mod local_rpc;
mod price_source;
mod run_local_delivery;
pub use local_rpc::call_local_rpc;
pub use price_source::{LocalPriceSource, PriceQuoteV1, PriceSource, PriceSourceError};
pub use run_local_delivery::{
    AuthenticatedOfferRefV1, DeliveryOfferQueryV1, DeliveryPublicationV1, RunLocalDelivery,
    RunLocalDeliveryError,
};

use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use jsonrpsee::{RpcModule, core::RpcResult, types::ErrorObjectOwned};
use lez_bridge_protocol::RequestId;
use lez_swap_core::{
    Chain, ChainPosition, ClockBasis, ConfirmationPolicy, Error, Pair, Participant, Phase,
    RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{
    AlertObservedEvent, EventCommit, LocalPriceV1, MakerConfigurationCommit, MakerOfferCommit,
    MakerOfferId, MakerOfferRecordV1, MakerOfferStatus, MakerOfferV1, MakerPairConfigurationV1,
    MakerRouteV1, MakerZecNegotiationV1, OperatorAlert, OperatorAlertKind, OperatorAlertRecordV1,
    OperatorAlertSeverity, OperatorTerminalProjectionCommit, SqliteSwapStore,
    SqliteZecRecoveryStore, StoreError, VersionedMakerRecord, maker_zec_chat_session_id,
};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, ClaimPreimage, HistoricalReplayError, ProtectedClaimKey,
    ValidatedZecAgreementDraftV1, ZcashObservationEvent, ZcashObservationEventRecordV1,
    ZcashObservationTracker, ZecAgreementDraftV1, ZecBindingRecordError, ZecPairSdk,
    ZecRefundProfile, ZecSwapBinding, replay_zcash_observation_history,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};

const NOT_FOUND: i32 = -32_004;
const CONFLICT: i32 = -32_009;
const INTERNAL_ERROR: i32 = -32_603;

/// RPC context owned by one maker daemon.
#[derive(Clone)]
pub struct MakerRpc {
    store: Arc<Mutex<SqliteSwapStore>>,
    delivery: Option<Arc<RunLocalDelivery>>,
    chat_signing_key: Option<Arc<SecretKey>>,
    zec_completion_store: Option<Arc<SqliteZecRecoveryStore>>,
    maker_claim_preimage: Option<Arc<ClaimPreimage>>,
}

impl std::fmt::Debug for MakerRpc {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MakerRpc")
            .field("store", &self.store)
            .field("delivery", &self.delivery)
            .field(
                "chat_signing_key",
                &self.chat_signing_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "zec_completion_store",
                &self.zec_completion_store.as_ref().map(|_| "configured"),
            )
            .field(
                "maker_claim_preimage",
                &self.maker_claim_preimage.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

impl MakerRpc {
    /// Creates a maker RPC context. Transport authentication is configured by the daemon.
    #[must_use]
    pub fn new(store: SqliteSwapStore) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            delivery: None,
            chat_signing_key: None,
            zec_completion_store: None,
            maker_claim_preimage: None,
        }
    }

    /// Creates a shared maker context for Delivery and the isolated Chat RPC module.
    #[must_use]
    pub fn with_delivery(
        store: SqliteSwapStore,
        delivery: RunLocalDelivery,
        chat_signing_key: SecretKey,
        zec_completion_store: SqliteZecRecoveryStore,
        maker_claim_preimage: ClaimPreimage,
    ) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            delivery: Some(Arc::new(delivery)),
            chat_signing_key: Some(Arc::new(chat_signing_key)),
            zec_completion_store: Some(Arc::new(zec_completion_store)),
            maker_claim_preimage: Some(Arc::new(maker_claim_preimage)),
        }
    }
}

/// Replays one stopped Maker actor offline and atomically imports its terminal operator view.
///
/// Unit chain ports make the replay incapable of RPC or chain effects. The application store
/// validates that the actor's exact signed agreement is the one previously completed through
/// Maker Chat; the ordinary application aggregate remains untouched.
///
/// # Errors
///
/// Fails when the source is missing, not a fully replayable terminal Maker actor, aliases the
/// application database, lacks the matching claim key, or disagrees with application history.
pub async fn import_terminal_zec_maker_projection(
    application_database: &Path,
    actor_state_database: &Path,
    swap_id: &SwapId,
    claim_key: ProtectedClaimKey,
) -> anyhow::Result<OperatorTerminalProjectionCommit> {
    anyhow::ensure!(
        application_database != actor_state_database,
        "terminal actor state must be separate from the application database"
    );
    let actor_store = SqliteZecRecoveryStore::open_claim_capable_existing(
        actor_state_database,
        Participant::Maker,
        claim_key,
    )?;
    let sdk: ZecPairSdk<(), (), (), (), SqliteZecRecoveryStore> =
        ZecPairSdk::new(Participant::Maker, (), (), (), (), actor_store);
    let active = sdk
        .resume_all_capable(swap_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("terminal Maker actor is not activated"))?;
    let terminal = active.terminal_coordinator().ok_or_else(|| {
        anyhow::anyhow!("Maker actor has not reached an absorbing terminal phase")
    })?;
    let agreement_wire = active.agreement().encode_wire()?;
    let source_revision = active.revision();
    let mut application = SqliteSwapStore::open(application_database)?;
    application
        .project_zec_terminal_for_operator(terminal, source_revision, &agreement_wire)
        .map_err(Into::into)
}

/// Result of one committed Zcash funding reconciliation.
#[derive(Debug)]
pub struct AppliedZcashFundingEvent {
    swap: SwapCoordinator,
    commit: EventCommit,
    outcome: ZcashFundingProjectionOutcome,
    alert_sequence: Option<u64>,
}

/// Durable runtime classification of one Zcash funding event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZcashFundingProjectionOutcome {
    /// The event changed or refreshed non-terminal protocol state.
    Applied,
    /// Chain truth replaced a transaction ID already committed to the protocol.
    ReplacementConflict {
        /// Direction-derived participant whose transaction remains pinned.
        funded_by: Participant,
    },
    /// A removal/replacement affected an absorbing lifecycle outcome.
    TerminalReorgDetected {
        /// Completed or refunded lifecycle result retained for audit.
        terminal_phase: Phase,
        /// Direction-derived participant whose funding evidence changed.
        funded_by: Participant,
    },
}

impl AppliedZcashFundingEvent {
    /// Durable aggregate after the event or replay reload.
    #[must_use]
    pub const fn swap(&self) -> &SwapCoordinator {
        &self.swap
    }

    /// Durable commit metadata.
    #[must_use]
    pub const fn commit(&self) -> EventCommit {
        self.commit
    }

    /// Durable projection classification used to gate subsequent effects.
    #[must_use]
    pub const fn outcome(&self) -> ZcashFundingProjectionOutcome {
        self.outcome
    }

    /// Durable operator-alert cursor for attention-requiring outcomes.
    #[must_use]
    pub const fn alert_sequence(&self) -> Option<u64> {
        self.alert_sequence
    }
}

/// Failure while projecting and atomically committing a Zcash funding event.
#[derive(Debug, thiserror::Error)]
pub enum ZcashFundingApplyError {
    /// Persistence or optimistic concurrency failed.
    #[error("Zcash funding persistence failed")]
    Store(#[from] StoreError),
    /// Valid chain evidence conflicts with the current protocol aggregate.
    #[error("Zcash funding evidence conflicts with swap state")]
    Core(#[from] Error),
    /// The selected swap is not a Zcash swap.
    #[error("Zcash funding event was routed to a non-Zcash swap")]
    WrongPair,
    /// Legacy or corrupt storage omitted immutable ZEC profile/output terms.
    #[error("Zcash swap has no immutable profile/output binding")]
    MissingZcashBinding,
    /// Chain evidence differs from the swap's immutable profile/output terms.
    #[error("Zcash funding event does not match immutable swap binding")]
    Binding(#[from] ZecBindingRecordError),
    /// Coordinator leg policies disagree with the immutable named profile.
    #[error("Zcash swap confirmation policies do not match immutable profile")]
    BindingPolicyMismatch,
    /// Historical event records are corrupt or out of order.
    #[error("Zcash observation history cannot be replayed")]
    HistoricalReplay(#[from] HistoricalReplayError),
}

/// Restores the historical watcher head for the direction-derived ZEC-funded role.
///
/// The returned tracker is not fresh canonical evidence. Reconcile it with a new
/// stable Zebra snapshot before enabling any external effect.
///
/// # Errors
///
/// Returns [`ZcashFundingApplyError`] for missing storage, a non-Zcash swap,
/// corrupt records, or impossible event order.
pub fn load_zcash_observation_tracker(
    store: &SqliteSwapStore,
    id: &SwapId,
) -> Result<ZcashObservationTracker, ZcashFundingApplyError> {
    let swap = store.load(id)?.ok_or(StoreError::MissingSwap)?;
    let funded_by = zcash_funder(&swap)?;
    let binding = store
        .load_zcash_binding(id)?
        .ok_or(ZcashFundingApplyError::MissingZcashBinding)?;
    validate_binding_policies(&swap, &binding)?;
    let records = store.load_zcash_events(id, funded_by)?;
    replay_zcash_observation_history(&records).map_err(ZcashFundingApplyError::from)
}

/// Projects one validated Zcash watcher event and commits it with the aggregate.
///
/// The ZEC funder is derived from immutable swap direction rather than caller input.
/// An exact predecessor-slot replay reloads durable state before any core mutation,
/// which makes retries safe after an unknown successful commit outcome.
///
/// # Errors
///
/// Returns [`ZcashFundingApplyError`] for missing/stale storage, a non-Zcash swap,
/// invalid core transition, record/proof conversion, or transaction failure.
pub fn apply_zcash_funding_event(
    store: &mut SqliteSwapStore,
    predecessor_revision: u64,
    id: &SwapId,
    event: &ZcashObservationEvent,
) -> Result<AppliedZcashFundingEvent, ZcashFundingApplyError> {
    let record = ZcashObservationEventRecordV1::from_event(event);
    let mut swap = store.load(id)?.ok_or(StoreError::MissingSwap)?;
    let funded_by = zcash_funder(&swap)?;
    let binding = store
        .load_zcash_binding(id)?
        .ok_or(ZcashFundingApplyError::MissingZcashBinding)?;
    validate_binding_policies(&swap, &binding)?;
    binding.validate_event(event)?;
    if store
        .committed_zcash_event(predecessor_revision, id, funded_by, &record)?
        .is_some()
    {
        swap = store.load(id)?.ok_or(StoreError::MissingSwap)?;
        let outcome = terminal_reorg_outcome(&swap, funded_by, event)
            .or_else(|| replacement_conflict_outcome(&swap, funded_by, event))
            .unwrap_or(ZcashFundingProjectionOutcome::Applied);
        let alert = operator_alert(outcome, event)?;
        let transition = store.commit_zcash_transition(
            predecessor_revision,
            &swap,
            funded_by,
            &record,
            alert.as_ref(),
        )?;
        return Ok(AppliedZcashFundingEvent {
            swap,
            commit: transition.event(),
            outcome,
            alert_sequence: transition.alert_sequence(),
        });
    }

    let records = store.load_zcash_events(id, funded_by)?;
    let mut historical_tracker = replay_zcash_observation_history(&records)?;
    historical_tracker
        .apply_committed(event)
        .map_err(HistoricalReplayError::from)?;

    let outcome = if let Some(outcome) = terminal_reorg_outcome(&swap, funded_by, event) {
        outcome
    } else {
        project_zcash_funding_event(&mut swap, funded_by, event)?
    };
    let alert = operator_alert(outcome, event)?;
    let transition = store.commit_zcash_transition(
        predecessor_revision,
        &swap,
        funded_by,
        &record,
        alert.as_ref(),
    )?;
    Ok(AppliedZcashFundingEvent {
        swap,
        commit: transition.event(),
        outcome,
        alert_sequence: transition.alert_sequence(),
    })
}

fn operator_alert(
    outcome: ZcashFundingProjectionOutcome,
    event: &ZcashObservationEvent,
) -> Result<Option<OperatorAlertRecordV1>, ZcashFundingApplyError> {
    match outcome {
        ZcashFundingProjectionOutcome::Applied => Ok(None),
        ZcashFundingProjectionOutcome::ReplacementConflict { funded_by } => {
            OperatorAlertRecordV1::replacement_conflict(funded_by, event)
                .map(Some)
                .map_err(ZcashFundingApplyError::from)
        }
        ZcashFundingProjectionOutcome::TerminalReorgDetected {
            terminal_phase,
            funded_by,
        } => OperatorAlertRecordV1::terminal_reorg(terminal_phase, funded_by, event)
            .map(Some)
            .map_err(ZcashFundingApplyError::from),
    }
}

fn validate_binding_policies(
    swap: &SwapCoordinator,
    binding: &ZecSwapBinding,
) -> Result<(), ZcashFundingApplyError> {
    let profile = ZecRefundProfile::for_id(binding.profile_id());
    for participant in [Participant::Maker, Participant::Taker] {
        let expected = if swap.funded_chain(participant) == Chain::Zcash {
            profile.zcash_confirmations()
        } else {
            profile.lez_confirmations()
        };
        if swap.required_confirmations(participant) != expected {
            return Err(ZcashFundingApplyError::BindingPolicyMismatch);
        }
    }
    Ok(())
}

fn replacement_conflict_outcome(
    swap: &SwapCoordinator,
    funded_by: Participant,
    event: &ZcashObservationEvent,
) -> Option<ZcashFundingProjectionOutcome> {
    let ZcashObservationEvent::Replaced { canonical, .. } = event else {
        return None;
    };
    let role_is_reorged = matches!(
        (funded_by, swap.phase()),
        (Participant::Taker, Phase::TakerLockReorged)
            | (Participant::Maker, Phase::MakerLockReorged)
    );
    let replacement_id = canonical.transaction_id().to_string();
    (role_is_reorged
        && swap
            .funding_transaction_id(funded_by)
            .is_some_and(|pinned| pinned != replacement_id))
    .then_some(ZcashFundingProjectionOutcome::ReplacementConflict { funded_by })
}

fn terminal_reorg_outcome(
    swap: &SwapCoordinator,
    funded_by: Participant,
    event: &ZcashObservationEvent,
) -> Option<ZcashFundingProjectionOutcome> {
    (matches!(swap.phase(), Phase::Completed | Phase::Refunded)
        && matches!(
            event,
            ZcashObservationEvent::Removed(_) | ZcashObservationEvent::Replaced { .. }
        ))
    .then_some(ZcashFundingProjectionOutcome::TerminalReorgDetected {
        terminal_phase: swap.phase(),
        funded_by,
    })
}

fn zcash_funder(swap: &SwapCoordinator) -> Result<Participant, ZcashFundingApplyError> {
    if swap.pair() != Pair::Zcash {
        return Err(ZcashFundingApplyError::WrongPair);
    }
    Ok(if swap.funded_chain(Participant::Taker) == Chain::Zcash {
        Participant::Taker
    } else {
        Participant::Maker
    })
}

fn project_zcash_funding_event(
    swap: &mut SwapCoordinator,
    funded_by: Participant,
    event: &ZcashObservationEvent,
) -> Result<ZcashFundingProjectionOutcome, Error> {
    match event {
        ZcashObservationEvent::Canonical(canonical) => {
            swap.observe_funding(funded_by, canonical.chain_proof()?)?;
            Ok(ZcashFundingProjectionOutcome::Applied)
        }
        ZcashObservationEvent::Removed(removed) => {
            swap.observe_funding_removed(
                funded_by,
                &removed.previous().transaction_id().to_string(),
            )?;
            Ok(ZcashFundingProjectionOutcome::Applied)
        }
        ZcashObservationEvent::Replaced { removed, canonical } => {
            swap.observe_funding_removed(
                funded_by,
                &removed.previous().transaction_id().to_string(),
            )?;
            match swap.observe_funding(funded_by, canonical.chain_proof()?) {
                Ok(()) => Ok(ZcashFundingProjectionOutcome::Applied),
                Err(Error::ConflictingTakerLock) if funded_by == Participant::Taker => {
                    Ok(ZcashFundingProjectionOutcome::ReplacementConflict { funded_by })
                }
                Err(Error::ConflictingMakerLock) if funded_by == Participant::Maker => {
                    Ok(ZcashFundingProjectionOutcome::ReplacementConflict { funded_by })
                }
                Err(error) => Err(error),
            }
        }
    }
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

/// Versioned untrusted taker request for one maker-first ZEC proposal.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZecChatProposeRequestV1 {
    /// Must be one for this DTO shape.
    pub schema_version: u16,
    /// Global exact-replay identity for durable proposal staging.
    pub request_id: RequestId,
    /// Selected immutable offer identity.
    pub offer_id: MakerOfferId,
    /// Current active offer revision, normally one.
    pub expected_offer_revision: u64,
    /// Winning reservation and Chat-session identity.
    pub reservation_id: RequestId,
    /// Exact selected Zcash principal in zatoshis.
    pub foreign_units: u64,
    /// Exact signed Delivery envelope previously authenticated by the taker.
    pub signed_offer_envelope: Vec<u8>,
    /// Canonical bounded unsigned agreement draft produced from public chain facts.
    pub unsigned_draft_wire: Vec<u8>,
}

/// Exact durable maker proposal returned only after staging commits.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZecChatProposalV1 {
    /// This response schema version.
    pub schema_version: u16,
    /// Durable reserved offer revision.
    pub offer_revision: u64,
    /// Whether the exact stage request was already committed.
    pub was_replay: bool,
    /// Winning reservation and Chat-session identity.
    pub reservation_id: RequestId,
    /// Exact no-rounding LEZ principal.
    pub lez_units: u128,
    /// Maker identity authenticated by Delivery and the proposal signature.
    pub maker_identity: Vec<u8>,
    /// Taker identity committed by the validated unsigned draft.
    pub taker_identity: Vec<u8>,
    /// Canonical body commitment signed by the maker.
    pub agreement_commitment: [u8; 32],
    /// Exact bounded maker-proposal wire for taker validation and countersigning.
    pub proposal_wire: Vec<u8>,
}

/// Versioned taker response carrying the exact countersigned ZEC agreement.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZecChatCompleteRequestV1 {
    /// Must be one for this DTO shape.
    pub schema_version: u16,
    /// Global exact-replay identity for atomic final acceptance.
    pub request_id: RequestId,
    /// Reserved immutable offer identity.
    pub offer_id: MakerOfferId,
    /// Current reserved offer revision, normally two.
    pub expected_offer_revision: u64,
    /// Winning reservation and Chat-session identity.
    pub reservation_id: RequestId,
    /// Exact bounded dual-signed agreement wire validated by the taker.
    pub final_agreement_wire: Vec<u8>,
}

/// Durable final-acceptance result returned only after every linked row commits.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZecChatCompleteResponseV1 {
    /// This response schema version.
    pub schema_version: u16,
    /// Durable consumed offer revision.
    pub offer_revision: u64,
    /// Whether the exact completion request was already committed.
    pub was_replay: bool,
    /// Agreement-derived application swap identity.
    pub swap_id: Box<str>,
}

/// Empty parameters for bounded owner-local list methods.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ListRequest {}

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
            let store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            LocalPriceSource::new(&store)
                .quote(request.route, observed_at_unix_seconds)
                .map_err(price_source_error)
        },
    )?;
    register_offer_methods(module)?;
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

fn register_offer_methods(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
    module.register_blocking_method::<RpcResult<MakerOfferCommit>, _>(
        "maker_offer_publish",
        |params, context, _| {
            let request: OfferPublishRequest = params.one()?;
            let offer_id = request.offer_id.clone();
            let now_unix_seconds = trusted_now_unix_seconds()?;
            let mut store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            let commit = store
                .publish_local_offer(
                    &request.request_id,
                    &offer_id,
                    request.route,
                    now_unix_seconds,
                )
                .map_err(application_store_error)?;
            if let Some(delivery) = &context.delivery {
                let active = store
                    .list_maker_offer_history(now_unix_seconds)
                    .map_err(application_store_error)?
                    .into_iter()
                    .find(|record| {
                        record.offer().id() == &offer_id
                            && record.status() == MakerOfferStatus::Active
                    });
                if let Some(record) = active {
                    delivery
                        .publish_or_verify(&DeliveryPublicationV1::new(
                            record.offer().clone(),
                            now_unix_seconds,
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
    module.register_blocking_method::<RpcResult<ZecChatProposalV1>, _>(
        "zec_chat_propose_v1",
        |params, context, _| {
            let request: ZecChatProposeRequestV1 = params.one()?;
            validate_zec_chat_shape(&request)?;
            let now_unix_seconds = trusted_now_unix_seconds()?;
            let delivery = context
                .delivery
                .as_ref()
                .ok_or_else(|| invalid_request("maker Chat Delivery is unavailable"))?;
            let signing_key = context
                .chat_signing_key
                .as_ref()
                .ok_or_else(|| invalid_request("maker Chat signer is unavailable"))?;
            let authenticated = delivery
                .authenticate_envelope(&request.signed_offer_envelope)
                .map_err(|error| invalid_request(error.to_string()))?;
            let offer = authenticated.offer();
            if offer.id() != &request.offer_id
                || offer.route().pair() != Pair::Zcash
                || offer.route().direction() != SwapDirection::TakerSellsLez
                || offer.created_at_unix_seconds() > now_unix_seconds
                || now_unix_seconds >= offer.expires_at_unix_seconds()
            {
                return Err(invalid_request(
                    "Delivery offer does not match live ZEC Chat request",
                ));
            }
            let validated = ZecAgreementDraftV1::from_wire_at(
                &request.unsigned_draft_wire,
                lez_swap_core::UnixSeconds::new(now_unix_seconds),
            )
            .map_err(invalid_request)?;
            let maker_key = PublicKey::from_secret_key(&Secp256k1::signing_only(), signing_key);
            let maker_identity = maker_key.serialize();
            let taker_identity = validated.taker_zcash_key().serialize();
            let lez_units = offer
                .quote_foreign_amount(request.foreign_units)
                .map_err(invalid_request)?;
            let offer_commitment = authenticated.commitment();
            if !zec_draft_matches_offer(
                &validated,
                &authenticated,
                offer,
                &maker_key,
                &request.reservation_id,
                request.foreign_units,
                lez_units,
            ) {
                return Err(invalid_request(
                    "unsigned ZEC draft is not bound to the selected offer",
                ));
            }
            let agreement_commitment = validated.commitment();
            let signature = Secp256k1::signing_only()
                .sign_ecdsa(&Message::from_digest(agreement_commitment), signing_key)
                .serialize_compact();
            let proposal = validated
                .with_maker_signature(signature)
                .map_err(invalid_request)?;
            let proposal_wire = proposal.encode_wire().map_err(invalid_request)?;
            let negotiation = MakerZecNegotiationV1::proposed(
                request.reservation_id.clone(),
                offer_commitment,
                maker_identity,
                taker_identity,
                request.foreign_units,
                lez_units,
                now_unix_seconds,
                agreement_commitment,
                proposal_wire.clone(),
            )
            .map_err(invalid_request)?;
            let mut store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            let commit = store
                .stage_zec_maker_negotiation(
                    &request.request_id,
                    &request.offer_id,
                    request.expected_offer_revision,
                    &negotiation,
                )
                .map_err(application_store_error)?;
            Ok(ZecChatProposalV1 {
                schema_version: 1,
                offer_revision: commit.revision(),
                was_replay: commit.was_replay(),
                reservation_id: request.reservation_id,
                lez_units,
                maker_identity: maker_identity.to_vec(),
                taker_identity: taker_identity.to_vec(),
                agreement_commitment,
                proposal_wire,
            })
        },
    )?;
    register_chat_complete_method(module)?;
    Ok(())
}

fn register_chat_complete_method(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
    module.register_blocking_method::<RpcResult<ZecChatCompleteResponseV1>, _>(
        "zec_chat_complete_v1",
        |params, context, _| {
            let request: ZecChatCompleteRequestV1 = params.one()?;
            complete_zec_chat(&request, &context)
        },
    )?;
    Ok(())
}

fn complete_zec_chat(
    request: &ZecChatCompleteRequestV1,
    context: &MakerRpc,
) -> RpcResult<ZecChatCompleteResponseV1> {
    if request.schema_version != 1 {
        return Err(invalid_request("unsupported ZEC Chat completion"));
    }
    let now_unix_seconds = trusted_now_unix_seconds()?;
    let completion_store = context
        .zec_completion_store
        .as_ref()
        .ok_or_else(|| invalid_request("maker ZEC completion store is unavailable"))?;
    let preimage = context
        .maker_claim_preimage
        .as_ref()
        .ok_or_else(|| invalid_request("maker claim authority is unavailable"))?;
    let accepted = AcceptedZecAgreementV1::accept_wire_at(
        &request.final_agreement_wire,
        lez_swap_core::UnixSeconds::new(now_unix_seconds),
        Participant::Maker,
        0,
    )
    .map_err(invalid_request)?;
    let swap_id: Box<str> = accepted.agreement().coordinator().id().as_str().into();
    let commit = completion_store
        .complete_maker_zec_negotiation(
            &request.request_id,
            &request.offer_id,
            request.expected_offer_revision,
            &request.reservation_id,
            &accepted,
            preimage,
        )
        .map_err(application_store_error)?;
    Ok(ZecChatCompleteResponseV1 {
        schema_version: 1,
        offer_revision: commit.offer_revision(),
        was_replay: commit.was_replay(),
        swap_id,
    })
}

fn validate_zec_chat_shape(request: &ZecChatProposeRequestV1) -> RpcResult<()> {
    if request.schema_version != 1 || request.foreign_units == 0 {
        return Err(invalid_request("unsupported or empty ZEC Chat proposal"));
    }
    Ok(())
}

fn zec_draft_matches_offer(
    validated: &ValidatedZecAgreementDraftV1,
    authenticated: &AuthenticatedOfferRefV1,
    offer: &MakerOfferV1,
    maker_key: &PublicKey,
    reservation_id: &RequestId,
    foreign_units: u64,
    lez_units: u128,
) -> bool {
    let transcript = validated.body().transcript();
    validated.maker_zcash_key() == maker_key
        && authenticated.maker_identity() == &maker_key.serialize()
        && validated.body().direction() == SwapDirection::TakerSellsLez
        && validated.zcash_amount_zatoshis() == foreign_units
        && validated.body().lez_terms().amount() == lez_units
        && transcript.session_id() == &maker_zec_chat_session_id(reservation_id)
        && transcript.offer_commitment() == &authenticated.commitment()
        && transcript.expires_at_unix_seconds() == offer.expires_at_unix_seconds()
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
        | StoreError::StaleMakerOffer { .. } => rpc_error(CONFLICT, error.to_string()),
        StoreError::MissingMakerOffer => rpc_error(NOT_FOUND, error.to_string()),
        StoreError::MakerConfiguration(_)
        | StoreError::MakerOffer(_)
        | StoreError::MissingMakerPair
        | StoreError::MissingMakerLocalPrice
        | StoreError::MakerPriceSourceMismatch
        | StoreError::MakerRouteDisabled
        | StoreError::MakerOfferExpired
        | StoreError::MakerOfferUnavailable
        | StoreError::MakerOfferSwapMismatch
        | StoreError::MakerOfferReservationConflict => invalid_request(error),
        other => internal_store_error(other),
    }
}

fn price_source_error(error: PriceSourceError) -> ErrorObjectOwned {
    match error {
        PriceSourceError::MissingQuote => rpc_error(NOT_FOUND, error.to_string()),
        PriceSourceError::Store(error) => internal_store_error(error),
        PriceSourceError::DuplicateQuote => rpc_error(INTERNAL_ERROR, error.to_string()),
    }
}

fn internal_store_error(error: impl std::fmt::Display) -> ErrorObjectOwned {
    rpc_error(INTERNAL_ERROR, format!("swap store failure: {error}"))
}

fn rpc_error(code: i32, message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(code, message.into(), None::<()>)
}
