//! Typed Bitcoin and LEZ chain ports composed with durable lifecycle replay.

use async_trait::async_trait;
use lez_swap_core::{Chain, Participant, Phase, SwapId};
use lez_swap_sdk_core::{ClaimLeg, ExactPublicEffectPlanV1};

use super::{
    ActiveBtcSwap, BtcAgreementV1, BtcLifecycleActionV1, BtcLifecycleStore,
    BtcLifecycleTransitionOutcomeV1, BtcLifecycleTransitionV1, BtcSdkError, BtcSwapStatusV1,
    StoredBtcLifecycleSdk,
};

/// Immutable role/action request sent to exactly one chain adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcLifecycleDriveRequestV1 {
    agreement: BtcAgreementV1,
    status: BtcSwapStatusV1,
    action: BtcLifecycleActionV1,
    exact_effect_plan: Option<ExactPublicEffectPlanV1>,
}

impl BtcLifecycleDriveRequestV1 {
    /// Validated immutable agreement.
    #[must_use]
    pub const fn agreement(&self) -> &BtcAgreementV1 {
        &self.agreement
    }

    /// Durable status before this bounded drive.
    pub const fn status(&self) -> &BtcSwapStatusV1 {
        &self.status
    }

    /// Structural action selected by the role-fixed SDK.
    pub const fn action(&self) -> BtcLifecycleActionV1 {
        self.action
    }

    /// Exact SDK-owned public effect when the action can be preconstructed.
    ///
    /// Revealing claims are prepared by a role-local signer adapter and then
    /// validated through the returned exact transition. Observation-only
    /// actions have no plan.
    #[must_use]
    pub const fn exact_effect_plan(&self) -> Option<&ExactPublicEffectPlanV1> {
        self.exact_effect_plan.as_ref()
    }
}

/// One bounded chain-adapter result.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcLifecycleChainOutcomeV1 {
    /// No exact canonical transition is available yet.
    Pending,
    /// Exact canonical evidence to clone-validate and durably compare-exchange.
    Transition(Box<BtcLifecycleTransitionV1>),
}

/// Typed Bitcoin lifecycle boundary.
///
/// Mutating implementations must persist exact public bytes before a possible
/// node call, observe before every send decision, consume fresh authority at
/// most once, and return Unknown outcomes as Pending until exact chain
/// observation resolves them.
#[async_trait]
pub trait BitcoinBtcLifecyclePort: Send + Sync {
    /// Structured adapter error.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Performs one bounded Bitcoin action or observation.
    async fn drive(
        &self,
        request: BtcLifecycleDriveRequestV1,
    ) -> Result<BtcLifecycleChainOutcomeV1, Self::Error>;
}

/// Typed LEZ lifecycle boundary.
///
/// Mutating implementations follow the same persist-before-send, exact
/// observation, one-authority, and Unknown-is-observation-only rules as the
/// Bitcoin port.
#[async_trait]
pub trait LezBtcLifecyclePort: Send + Sync {
    /// Structured adapter error.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Performs one bounded LEZ action or observation.
    async fn drive(
        &self,
        request: BtcLifecycleDriveRequestV1,
    ) -> Result<BtcLifecycleChainOutcomeV1, Self::Error>;
}

/// Result of one durable runtime iteration.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcLifecycleDriveOutcomeV1 {
    /// The selected chain has no canonical transition yet.
    Pending(BtcSwapStatusV1),
    /// One transition was durably applied or converged on an exact replay.
    Transition {
        /// Store-level transition result.
        outcome: BtcLifecycleTransitionOutcomeV1,
        /// Fully resumed status after the durable write.
        status: BtcSwapStatusV1,
    },
    /// The durable lifecycle was already terminal; no port was called.
    Complete(BtcSwapStatusV1),
}

/// Full post-activation runtime over one store and two typed chain ports.
///
/// This type contains no discovery or negotiation capability. A new process
/// needs only the same role policy, durable store, and chain ports.
#[derive(Clone, Debug)]
pub struct BtcLifecycleRuntime<Store, Bitcoin, Lez> {
    stored: StoredBtcLifecycleSdk<Store>,
    bitcoin: Bitcoin,
    lez: Lez,
}

impl<Store, Bitcoin, Lez> BtcLifecycleRuntime<Store, Bitcoin, Lez>
where
    Store: BtcLifecycleStore,
    Bitcoin: BitcoinBtcLifecyclePort,
    Lez: LezBtcLifecyclePort,
{
    /// Composes a durable lifecycle with typed chain adapters.
    pub const fn new(stored: StoredBtcLifecycleSdk<Store>, bitcoin: Bitcoin, lez: Lez) -> Self {
        Self {
            stored,
            bitcoin,
            lez,
        }
    }

    /// Durable role-fixed lifecycle facade.
    pub const fn stored_sdk(&self) -> &StoredBtcLifecycleSdk<Store> {
        &self.stored
    }

    /// Executes at most one typed chain iteration and one durable transition.
    ///
    /// # Errors
    ///
    /// Returns store, codec, protocol, routing, or chain adapter errors.
    pub async fn drive_once(
        &self,
        swap_id: &SwapId,
    ) -> Result<BtcLifecycleDriveOutcomeV1, BtcSdkError> {
        let active = self
            .stored
            .resume(swap_id)
            .await?
            .ok_or(BtcSdkError::LifecycleNotActivated)?;
        let status = active.status();
        if status.phase() == Phase::Completed || status.phase() == Phase::Refunded {
            return Ok(BtcLifecycleDriveOutcomeV1::Complete(status));
        }
        let action = active.next_action();
        let chain = action_chain(&active, action)?;
        let request = BtcLifecycleDriveRequestV1 {
            agreement: active.agreement().clone(),
            status: status.clone(),
            action,
            exact_effect_plan: exact_plan(&active, action)?,
        };
        let chain_outcome = match chain {
            Chain::Bitcoin => self
                .bitcoin
                .drive(request)
                .await
                .map_err(|error| BtcSdkError::BitcoinLifecycle(Box::new(error)))?,
            Chain::Lez => self
                .lez
                .drive(request)
                .await
                .map_err(|error| BtcSdkError::LezLifecycle(Box::new(error)))?,
            Chain::Monero | Chain::Zcash => return Err(BtcSdkError::LifecycleRouting),
        };
        let BtcLifecycleChainOutcomeV1::Transition(transition) = chain_outcome else {
            return Ok(BtcLifecycleDriveOutcomeV1::Pending(status));
        };
        let outcome = self.stored.apply_transition(swap_id, *transition).await?;
        let status = self
            .stored
            .resume(swap_id)
            .await?
            .ok_or(BtcSdkError::LifecycleNotActivated)?
            .status();
        Ok(BtcLifecycleDriveOutcomeV1::Transition { outcome, status })
    }
}

fn exact_plan(
    active: &ActiveBtcSwap,
    action: BtcLifecycleActionV1,
) -> Result<Option<ExactPublicEffectPlanV1>, BtcSdkError> {
    Ok(match action {
        BtcLifecycleActionV1::PublishBitcoinFirstLock
        | BtcLifecycleActionV1::PublishLezFirstLock => Some(active.first_lock_plan().clone()),
        BtcLifecycleActionV1::PublishBitcoinSecondLock
        | BtcLifecycleActionV1::PublishLezSecondLock => Some(
            active
                .lock_effects
                .plan_for_participant(active.agreement(), Participant::Maker)
                .clone(),
        ),
        BtcLifecycleActionV1::PublishBitcoinFollowupClaim
        | BtcLifecycleActionV1::PublishLezFollowupClaim => active.followup_claim_plan()?,
        BtcLifecycleActionV1::AwaitTakerFirstLock
        | BtcLifecycleActionV1::AwaitMakerSecondLock
        | BtcLifecycleActionV1::PublishBitcoinRevealingClaim
        | BtcLifecycleActionV1::PublishLezRevealingClaim
        | BtcLifecycleActionV1::AwaitRevealingClaim
        | BtcLifecycleActionV1::AwaitFollowupClaim
        | BtcLifecycleActionV1::AwaitCounterpartyRefund
        | BtcLifecycleActionV1::EvaluateRecovery
        | BtcLifecycleActionV1::Complete => None,
    })
}

fn action_chain(
    active: &ActiveBtcSwap,
    action: BtcLifecycleActionV1,
) -> Result<Chain, BtcSdkError> {
    let coordinator = active.agreement().coordinator();
    Ok(match action {
        BtcLifecycleActionV1::PublishBitcoinFirstLock
        | BtcLifecycleActionV1::PublishBitcoinSecondLock
        | BtcLifecycleActionV1::PublishBitcoinRevealingClaim
        | BtcLifecycleActionV1::PublishBitcoinFollowupClaim => Chain::Bitcoin,
        BtcLifecycleActionV1::PublishLezFirstLock
        | BtcLifecycleActionV1::PublishLezSecondLock
        | BtcLifecycleActionV1::PublishLezRevealingClaim
        | BtcLifecycleActionV1::PublishLezFollowupClaim => Chain::Lez,
        BtcLifecycleActionV1::AwaitTakerFirstLock => coordinator.funded_chain(Participant::Taker),
        BtcLifecycleActionV1::AwaitMakerSecondLock => coordinator.funded_chain(Participant::Maker),
        BtcLifecycleActionV1::AwaitRevealingClaim => {
            claim_leg_chain(active.claim_order().revealing())
        }
        BtcLifecycleActionV1::AwaitFollowupClaim => {
            claim_leg_chain(active.claim_order().followup())
        }
        BtcLifecycleActionV1::AwaitCounterpartyRefund | BtcLifecycleActionV1::EvaluateRecovery => {
            match active.status().phase() {
                Phase::BothLegsLocked => coordinator.funded_chain(Participant::Maker),
                Phase::MakerLegRefunded => coordinator.funded_chain(Participant::Taker),
                _ => return Err(BtcSdkError::LifecycleRouting),
            }
        }
        BtcLifecycleActionV1::Complete => return Err(BtcSdkError::LifecycleRouting),
    })
}

const fn claim_leg_chain(leg: ClaimLeg) -> Chain {
    match leg {
        ClaimLeg::Lez => Chain::Lez,
        ClaimLeg::Foreign => Chain::Bitcoin,
    }
}
