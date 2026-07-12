//! Authenticated local JSON-RPC boundary for the headless maker.

use std::sync::Mutex;

use jsonrpsee::{RpcModule, core::RpcResult, types::ErrorObjectOwned};
use lez_swap_core::{
    Chain, ChainPosition, ClockBasis, ConfirmationPolicy, Error, Pair, Participant, Phase,
    RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{EventCommit, SqliteSwapStore, StoreError};
use lez_zec_swap_sdk::{ZcashObservationEvent, ZcashObservationEventRecordV1};
use serde::{Deserialize, Serialize};

const NOT_FOUND: i32 = -32_004;
const CONFLICT: i32 = -32_009;
const INTERNAL_ERROR: i32 = -32_603;

/// Minimum capability length. Deployments should use at least 256 random bits.
pub const MINIMUM_CAPABILITY_LENGTH: usize = 24;

/// RPC context owned by one maker daemon.
pub struct MakerRpc {
    store: Mutex<SqliteSwapStore>,
}

impl std::fmt::Debug for MakerRpc {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MakerRpc")
            .field("store", &self.store)
            .finish()
    }
}

impl MakerRpc {
    /// Creates a maker RPC context. Transport authentication is configured by the daemon.
    #[must_use]
    pub fn new(store: SqliteSwapStore) -> Self {
        Self {
            store: Mutex::new(store),
        }
    }
}

/// Result of one committed Zcash funding reconciliation.
#[derive(Debug)]
pub struct AppliedZcashFundingEvent {
    swap: SwapCoordinator,
    commit: EventCommit,
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
    if swap.pair() != Pair::Zcash {
        return Err(ZcashFundingApplyError::WrongPair);
    }
    let funded_by = if swap.funded_chain(Participant::Taker) == Chain::Zcash {
        Participant::Taker
    } else {
        Participant::Maker
    };
    if let Some(commit) =
        store.committed_zcash_event(predecessor_revision, id, funded_by, &record)?
    {
        swap = store.load(id)?.ok_or(StoreError::MissingSwap)?;
        return Ok(AppliedZcashFundingEvent { swap, commit });
    }

    project_zcash_funding_event(&mut swap, funded_by, event)?;
    let commit = store.commit_zcash_event(predecessor_revision, &swap, funded_by, &record)?;
    Ok(AppliedZcashFundingEvent { swap, commit })
}

fn project_zcash_funding_event(
    swap: &mut SwapCoordinator,
    funded_by: Participant,
    event: &ZcashObservationEvent,
) -> Result<(), Error> {
    match event {
        ZcashObservationEvent::Canonical(canonical) => {
            swap.observe_funding(funded_by, canonical.chain_proof()?)
        }
        ZcashObservationEvent::Removed(removed) => swap
            .observe_funding_removed(funded_by, &removed.previous().transaction_id().to_string()),
        ZcashObservationEvent::Replaced { removed, canonical } => {
            swap.observe_funding_removed(
                funded_by,
                &removed.previous().transaction_id().to_string(),
            )?;
            swap.observe_funding(funded_by, canonical.chain_proof()?)
        }
    }
}

/// Rejects trivially weak owner capabilities before transport setup.
///
/// # Errors
///
/// Returns an error when the capability is too short.
pub fn validate_capability(capability: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        capability.len() >= MINIMUM_CAPABILITY_LENGTH,
        "maker RPC capability must contain at least {MINIMUM_CAPABILITY_LENGTH} bytes"
    );
    Ok(())
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
}

impl From<&SwapCoordinator> for SwapView {
    fn from(swap: &SwapCoordinator) -> Self {
        Self {
            id: swap.id().as_str().into(),
            pair: swap.pair(),
            direction: swap.direction(),
            phase: swap.phase(),
        }
    }
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

/// Parameters for reading one swap.
#[derive(Debug, Deserialize, Serialize)]
pub struct StatusRequest {
    /// Stable swap identifier.
    pub id: Box<str>,
}

/// Builds the RPC module shared by daemon transports and direct contract tests.
///
/// # Errors
///
/// Returns an error if a method cannot be registered.
pub fn rpc_module(context: MakerRpc) -> anyhow::Result<RpcModule<MakerRpc>> {
    let mut module = RpcModule::new(context);
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
                .load(&id)
                .map_err(internal_store_error)?
                .ok_or_else(|| rpc_error(NOT_FOUND, "swap not found"))?;
            Ok(SwapView::from(&swap))
        },
    )?;
    Ok(module)
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

fn internal_store_error(error: impl std::fmt::Display) -> ErrorObjectOwned {
    rpc_error(INTERNAL_ERROR, format!("swap store failure: {error}"))
}

fn rpc_error(code: i32, message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(code, message.into(), None::<()>)
}
