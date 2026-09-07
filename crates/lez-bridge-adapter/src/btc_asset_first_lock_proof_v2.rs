//! Exact F7 asset preparation-to-finalized-observation proof for the BTC actor.

use lez_bridge_protocol::{
    ChainPosition, ChainTip, DiscoveryWindow, ObserveWitnessedAssetEscrowV2Result,
    PrepareWitnessedAssetEscrowV2Result, ProtocolValueError, RequestId,
    WitnessedAssetCustodyFactsV2, WitnessedAssetPrepareStepV2, WitnessedAssetPreparedEffectV2,
};
use lez_btc_swap_sdk::{
    LezAssetCustodyEvidenceV1, LezAssetFirstLockEvidenceV1, PreparedLezAssetFundingV1,
};
use lez_swap_sdk_core::{
    ExactPublicEffectBytes, ExactPublicEffectPlanV1, ExpectedPublicEffectId, PublicEffectStepId,
    PublicEffectStepV1,
};
use thiserror::Error;

use crate::{
    BtcLezAssetBridgeBindingV2, BtcLezAssetBridgeV2Error, LezBridgeAdapter,
    LezBridgeAssetV2Transport, encode_hex32,
};

const LEZ_INITIALIZE_STEP: &str = "lez.initialize";
const LEZ_CREATE_CUSTODY_ATA_STEP: &str = "lez.create_custody_ata";
const LEZ_FUND_STEP: &str = "lez.fund";

/// SDK material cross-bound to one exact preparation and one stable finalized observation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcLezAssetFirstLockProofV2 {
    prepared: PreparedLezAssetFundingV1,
    evidence: LezAssetFirstLockEvidenceV1,
    finalized_tip: ChainTip,
}

impl BtcLezAssetFirstLockProofV2 {
    /// Exact native or token plan retained before any submission attempt.
    pub const fn prepared(&self) -> &PreparedLezAssetFundingV1 {
        &self.prepared
    }

    /// Canonical finalized observation converted to SDK evidence.
    pub const fn evidence(&self) -> &LezAssetFirstLockEvidenceV1 {
        &self.evidence
    }

    /// Stable finalized tip bracketing the exact observation.
    pub const fn finalized_tip(&self) -> ChainTip {
        self.finalized_tip
    }

    /// Splits the proof into the two inputs consumed by the BTC asset SDK.
    pub fn into_sdk_parts(self) -> (PreparedLezAssetFundingV1, LezAssetFirstLockEvidenceV1) {
        (self.prepared, self.evidence)
    }
}

/// Failure to cross-bind an F7 preparation to finalized canonical observations.
#[derive(Debug, Error)]
pub enum BtcLezAssetFirstLockProofV2Error<E: std::error::Error + 'static> {
    /// Policy, role, request, response-echo, or transport validation failed.
    #[error("LEZ asset first-lock bridge read failed")]
    Bridge(#[source] BtcLezAssetBridgeV2Error<E>),
    /// Preparation terms differ from the accepted asset binding.
    #[error("LEZ asset first-lock preparation terms differ from the binding")]
    PreparationTermsMismatch,
    /// The response no longer passes strict protocol state and instruction validation.
    #[error("LEZ asset first-lock observation is invalid")]
    InvalidObservation(#[source] ProtocolValueError),
    /// Canonical facts identify another witnessed-escrow metadata account.
    #[error("LEZ asset first-lock metadata account differs from the binding")]
    MetadataAccountMismatch,
    /// Prepared and observed plans have different effect counts.
    #[error("LEZ asset first-lock prepared and observed plan lengths differ")]
    EffectCountMismatch,
    /// A canonical observation occupies another semantic step.
    #[error("LEZ asset first-lock observed semantic step differs from preparation")]
    EffectStepMismatch,
    /// A canonical observation identifies another transaction.
    #[error("LEZ asset first-lock observed transaction identity differs from preparation")]
    EffectIdentityMismatch,
    /// Canonical bytes differ from the exact pre-submission bytes.
    #[error("LEZ asset first-lock observed transaction bytes differ from preparation")]
    EffectBytesMismatch,
    /// A purported canonical preparation effect is not a public transaction.
    #[error("LEZ asset first-lock observed effect is not a public transaction")]
    NonPublicEffect,
    /// The bracketed finalized tip changed during the read.
    #[error("LEZ asset first-lock finalized tip changed during observation")]
    UnstableFinalizedTip,
    /// The stable finalized tip does not completely cover the caller-owned window.
    #[error("LEZ asset first-lock finalized window is not completely covered")]
    IncompleteFinalizedWindow,
    /// A transaction lies outside the completely covered finalized window.
    #[error("LEZ asset first-lock transaction placement is outside the finalized window")]
    EffectPlacementOutsideWindow,
    /// Required effects do not appear in strict canonical order.
    #[error("LEZ asset first-lock effects are not chronological")]
    EffectOrderMismatch,
    /// Two effects at one height claim different canonical blocks.
    #[error("LEZ asset first-lock transaction placements disagree on the canonical block")]
    InconsistentBlockPlacement,
    /// Strict canonical facts could not form the SDK's exact material.
    #[error("LEZ asset first-lock canonical facts cannot form SDK material")]
    InvalidSdkMaterial,
}

impl<T> LezBridgeAdapter<T>
where
    T: LezBridgeAssetV2Transport,
{
    /// Observes and proves one exact native or token first-lock plan.
    ///
    /// No missing or unavailable read can produce this proof: every bridge error is
    /// preserved. On success, the method revalidates protocol facts, requires a
    /// stable finalized window, checks strict transaction placement order, and
    /// cross-binds every semantic step, public ID, and byte of the retained plan.
    ///
    /// # Errors
    ///
    /// Fails closed on bridge unavailability, binding drift, unstable/incomplete
    /// finality, malformed state, placement drift, or any exact-effect substitution.
    pub async fn prove_btc_asset_first_lock_v2(
        &self,
        binding: &BtcLezAssetBridgeBindingV2,
        request_id: RequestId,
        preparation: &PrepareWitnessedAssetEscrowV2Result,
        window: DiscoveryWindow,
    ) -> Result<BtcLezAssetFirstLockProofV2, BtcLezAssetFirstLockProofV2Error<T::Error>> {
        if preparation.terms != *binding.terms() {
            return Err(BtcLezAssetFirstLockProofV2Error::PreparationTermsMismatch);
        }
        let observation = self
            .observe_btc_asset_escrow_v2(binding, request_id, preparation.effects.clone(), window)
            .await
            .map_err(BtcLezAssetFirstLockProofV2Error::Bridge)?;
        build_proof(binding, &preparation.effects, observation, window)
    }
}

fn build_proof<E: std::error::Error + 'static>(
    binding: &BtcLezAssetBridgeBindingV2,
    prepared_effects: &[WitnessedAssetPreparedEffectV2],
    observation: ObserveWitnessedAssetEscrowV2Result,
    window: DiscoveryWindow,
) -> Result<BtcLezAssetFirstLockProofV2, BtcLezAssetFirstLockProofV2Error<E>> {
    cross_bind_effects(prepared_effects, &observation.effects)?;
    let observation = ObserveWitnessedAssetEscrowV2Result::new(
        observation.context,
        observation.terms,
        observation.tip_before,
        observation.effects,
        observation.metadata,
        observation.custody,
        observation.tip_after,
    )
    .map_err(BtcLezAssetFirstLockProofV2Error::InvalidObservation)?;
    validate_finalized_placement(&observation, window)?;
    if observation.metadata.account_id.as_bytes() != binding.metadata_account_id() {
        return Err(BtcLezAssetFirstLockProofV2Error::MetadataAccountMismatch);
    }

    let prepared_plan = prepared_plan(prepared_effects)?;
    let observed_plan = observed_plan(&observation.effects)?;
    let prepared = match binding.terms().asset() {
        lez_bridge_protocol::WitnessedLezAssetV2::Native(_) => {
            PreparedLezAssetFundingV1::native(prepared_plan)
        }
        lez_bridge_protocol::WitnessedLezAssetV2::CustomToken(_) => {
            PreparedLezAssetFundingV1::custom_token(prepared_plan)
        }
    }
    .map_err(|_| BtcLezAssetFirstLockProofV2Error::InvalidSdkMaterial)?;
    let custody = match &observation.custody {
        WitnessedAssetCustodyFactsV2::Native(facts) => LezAssetCustodyEvidenceV1::Native {
            custody_account: *facts.account_id.as_bytes(),
        },
        WitnessedAssetCustodyFactsV2::CustomToken(facts) => {
            LezAssetCustodyEvidenceV1::CustomToken {
                custody_ata_account: *facts.account_id.as_bytes(),
                token_definition_account: *facts.token_definition_account_id.as_bytes(),
            }
        }
    };
    let evidence = LezAssetFirstLockEvidenceV1::new(
        *binding.genesis_block_hash(),
        observed_plan,
        *binding.metadata_account_id(),
        custody,
        observation.metadata.amount.as_u128(),
        true,
    );
    Ok(BtcLezAssetFirstLockProofV2 {
        prepared,
        evidence,
        finalized_tip: observation.tip_after,
    })
}

fn cross_bind_effects<E: std::error::Error + 'static>(
    prepared: &[WitnessedAssetPreparedEffectV2],
    observed: &[lez_bridge_protocol::WitnessedAssetObservedPrepareEffectV2],
) -> Result<(), BtcLezAssetFirstLockProofV2Error<E>> {
    if prepared.len() != observed.len() {
        return Err(BtcLezAssetFirstLockProofV2Error::EffectCountMismatch);
    }
    for (prepared, observed) in prepared.iter().zip(observed) {
        if prepared.step != observed.step {
            return Err(BtcLezAssetFirstLockProofV2Error::EffectStepMismatch);
        }
        if prepared.transaction.transaction_id != observed.transaction.transaction_id {
            return Err(BtcLezAssetFirstLockProofV2Error::EffectIdentityMismatch);
        }
        if prepared.transaction.exact_bytes != observed.transaction.exact_bytes {
            return Err(BtcLezAssetFirstLockProofV2Error::EffectBytesMismatch);
        }
        if !observed.transaction.is_public {
            return Err(BtcLezAssetFirstLockProofV2Error::NonPublicEffect);
        }
    }
    Ok(())
}

fn validate_finalized_placement<E: std::error::Error + 'static>(
    observation: &ObserveWitnessedAssetEscrowV2Result,
    window: DiscoveryWindow,
) -> Result<(), BtcLezAssetFirstLockProofV2Error<E>> {
    if !crate::tip_extends(&observation.tip_before, &observation.tip_after) {
        return Err(BtcLezAssetFirstLockProofV2Error::UnstableFinalizedTip);
    }
    let end = window
        .start_height()
        .checked_add(u64::from(window.max_blocks() - 1))
        .ok_or(BtcLezAssetFirstLockProofV2Error::IncompleteFinalizedWindow)?;
    if end > observation.tip_after.height {
        return Err(BtcLezAssetFirstLockProofV2Error::IncompleteFinalizedWindow);
    }
    let mut previous: Option<ChainPosition> = None;
    for effect in &observation.effects {
        let position = effect.transaction.position;
        if position.height < window.start_height()
            || position.height > end
            || position.height > observation.tip_after.height
            || (position.height == observation.tip_after.height
                && position.block_hash != observation.tip_after.block_hash)
        {
            return Err(BtcLezAssetFirstLockProofV2Error::EffectPlacementOutsideWindow);
        }
        if let Some(prior) = previous {
            if prior.height == position.height && prior.block_hash != position.block_hash {
                return Err(BtcLezAssetFirstLockProofV2Error::InconsistentBlockPlacement);
            }
            if !strictly_precedes(prior, position) {
                return Err(BtcLezAssetFirstLockProofV2Error::EffectOrderMismatch);
            }
        }
        previous = Some(position);
    }
    Ok(())
}

const fn strictly_precedes(left: ChainPosition, right: ChainPosition) -> bool {
    left.height < right.height
        || (left.height == right.height && left.transaction_index < right.transaction_index)
}

fn prepared_plan<E: std::error::Error + 'static>(
    effects: &[WitnessedAssetPreparedEffectV2],
) -> Result<ExactPublicEffectPlanV1, BtcLezAssetFirstLockProofV2Error<E>> {
    exact_plan(effects.iter().map(|effect| {
        (
            effect.step,
            effect.transaction.transaction_id.as_bytes(),
            effect.transaction.exact_bytes.as_slice(),
        )
    }))
}

fn observed_plan<E: std::error::Error + 'static>(
    effects: &[lez_bridge_protocol::WitnessedAssetObservedPrepareEffectV2],
) -> Result<ExactPublicEffectPlanV1, BtcLezAssetFirstLockProofV2Error<E>> {
    exact_plan(effects.iter().map(|effect| {
        (
            effect.step,
            effect.transaction.transaction_id.as_bytes(),
            effect.transaction.exact_bytes.as_slice(),
        )
    }))
}

fn exact_plan<'a, E: std::error::Error + 'static>(
    effects: impl Iterator<Item = (WitnessedAssetPrepareStepV2, &'a [u8; 32], &'a [u8])>,
) -> Result<ExactPublicEffectPlanV1, BtcLezAssetFirstLockProofV2Error<E>> {
    let steps = effects
        .map(|(step, transaction_id, exact_bytes)| {
            Ok(PublicEffectStepV1::new(
                PublicEffectStepId::new(step_id(step))?,
                ExpectedPublicEffectId::new(encode_hex32(transaction_id))?,
                ExactPublicEffectBytes::new(exact_bytes.to_vec())?,
            ))
        })
        .collect::<Result<Vec<_>, lez_swap_sdk_core::PublicEffectPlanError>>()
        .map_err(|_| BtcLezAssetFirstLockProofV2Error::InvalidSdkMaterial)?;
    ExactPublicEffectPlanV1::new(steps)
        .map_err(|_| BtcLezAssetFirstLockProofV2Error::InvalidSdkMaterial)
}

const fn step_id(step: WitnessedAssetPrepareStepV2) -> &'static str {
    match step {
        WitnessedAssetPrepareStepV2::InitializeWitnessed => LEZ_INITIALIZE_STEP,
        WitnessedAssetPrepareStepV2::CreateCustodyAta => LEZ_CREATE_CUSTODY_ATA_STEP,
        WitnessedAssetPrepareStepV2::Fund => LEZ_FUND_STEP,
    }
}
