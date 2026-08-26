//! Additive F7 asset-bound lock authorization for LEZ/Bitcoin swaps.
//!
//! This module owns no chain, persistence, or submission capability. It binds
//! exact prepared effects and adapter-produced first-lock facts to one
//! countersigned asset extension before releasing the maker's second-lock plan.

use std::collections::HashSet;

use bitcoin::{Transaction, Txid, consensus::deserialize, hashes::Hash as _};
use lez_swap_core::{Chain, Participant, SwapDirection};
use lez_swap_sdk_core::{ExactPublicEffectPlanV1, PublicEffectStepV1};
use thiserror::Error;

use crate::{
    BtcAgreementV1, BtcLezAssetExtensionV1, BtcLezAssetV1,
    sdk::{
        AcceptedBtcAgreementV1, BitcoinFirstLockEvidenceV1, BtcPairSdk, PreparedBitcoinFundingV1,
    },
};

const BITCOIN_FUNDING_STEP: &str = "bitcoin.funding";
const LEZ_INITIALIZE_STEP: &str = "lez.initialize";
const LEZ_CREATE_CUSTODY_ATA_STEP: &str = "lez.create_custody_ata";
const LEZ_FUND_STEP: &str = "lez.fund";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LezAssetPlanKindV1 {
    Native,
    CustomToken,
}

/// Exact ordered LEZ funding effects for one explicit F7 asset kind.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct PreparedLezAssetFundingV1 {
    kind: LezAssetPlanKindV1,
    plan: ExactPublicEffectPlanV1,
}

impl PreparedLezAssetFundingV1 {
    /// Validates an exact native [initialize, fund] plan.
    ///
    /// # Errors
    ///
    /// Rejects a missing, extra, reordered, duplicate-ID, or duplicate-bytes effect.
    pub fn native(plan: ExactPublicEffectPlanV1) -> Result<Self, BtcLezAssetSdkError> {
        Self::new(LezAssetPlanKindV1::Native, plan)
    }

    /// Validates an exact custom-token [initialize, create-custody-ATA, fund] plan.
    ///
    /// # Errors
    ///
    /// Rejects a missing, extra, reordered, duplicate-ID, or duplicate-bytes effect.
    pub fn custom_token(plan: ExactPublicEffectPlanV1) -> Result<Self, BtcLezAssetSdkError> {
        Self::new(LezAssetPlanKindV1::CustomToken, plan)
    }

    fn new(
        kind: LezAssetPlanKindV1,
        plan: ExactPublicEffectPlanV1,
    ) -> Result<Self, BtcLezAssetSdkError> {
        validate_lez_asset_plan(&plan, kind)?;
        Ok(Self { kind, plan })
    }

    /// Immutable exact ordered LEZ public-effect plan.
    pub const fn plan(&self) -> &ExactPublicEffectPlanV1 {
        &self.plan
    }

    fn validate(&self) -> Result<(), BtcLezAssetSdkError> {
        validate_lez_asset_plan(&self.plan, self.kind)
    }
}

/// Exact Bitcoin funding plus the explicit native or token F7 LEZ plan.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcLezAssetPreparedLockEffectsV1 {
    extension: BtcLezAssetExtensionV1,
    bitcoin: PreparedBitcoinFundingV1,
    lez: PreparedLezAssetFundingV1,
}

impl BtcLezAssetPreparedLockEffectsV1 {
    /// Binds complete exact lock effects to one accepted base agreement and extension.
    ///
    /// # Errors
    ///
    /// Rejects extension/agreement substitution, asset-kind/plan mismatch,
    /// malformed LEZ plan shape, or Bitcoin transaction/output substitution.
    pub fn new(
        agreement: &BtcAgreementV1,
        extension: BtcLezAssetExtensionV1,
        bitcoin: PreparedBitcoinFundingV1,
        lez: PreparedLezAssetFundingV1,
    ) -> Result<Self, BtcLezAssetSdkError> {
        let effects = Self {
            extension,
            bitcoin,
            lez,
        };
        effects.validate(agreement)?;
        Ok(effects)
    }

    /// Exact validated countersigned asset extension.
    #[must_use]
    pub const fn asset_extension(&self) -> &BtcLezAssetExtensionV1 {
        &self.extension
    }

    /// Exact signed Bitcoin funding effect.
    pub const fn bitcoin(&self) -> &PreparedBitcoinFundingV1 {
        &self.bitcoin
    }

    /// Exact ordered native or custom-token LEZ funding effects.
    pub const fn lez(&self) -> &PreparedLezAssetFundingV1 {
        &self.lez
    }

    fn validate(&self, agreement: &BtcAgreementV1) -> Result<(), BtcLezAssetSdkError> {
        if self.extension.base_agreement_commitment() != agreement.agreement_commitment() {
            return Err(BtcLezAssetSdkError::AssetExtensionAgreementMismatch);
        }
        let expected_kind = match self.extension.asset() {
            BtcLezAssetV1::Native => LezAssetPlanKindV1::Native,
            BtcLezAssetV1::CustomToken(_) => LezAssetPlanKindV1::CustomToken,
        };
        if self.lez.kind != expected_kind {
            return Err(BtcLezAssetSdkError::AssetPlanKindMismatch);
        }
        self.lez.validate()?;
        validate_bitcoin_funding(agreement, &self.bitcoin)
    }

    fn plan_for_participant(
        &self,
        agreement: &BtcAgreementV1,
        participant: Participant,
    ) -> &ExactPublicEffectPlanV1 {
        match agreement.coordinator().funded_chain(participant) {
            Chain::Bitcoin => self.bitcoin.plan(),
            Chain::Lez => self.lez.plan(),
            Chain::Monero | Chain::Zcash => {
                unreachable!("a validated BTC agreement has only Bitcoin and LEZ legs")
            }
        }
    }
}

/// Asset-specific custody facts decoded by the LEZ adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum LezAssetCustodyEvidenceV1 {
    /// Native custody must equal the base agreement's authenticated-transfer account.
    Native {
        /// Observed native custody account.
        custody_account: [u8; 32],
    },
    /// Token custody must identify both the exact ATA and token definition.
    CustomToken {
        /// Observed escrow custody associated-token account.
        custody_ata_account: [u8; 32],
        /// Observed fungible-token definition account.
        token_definition_account: [u8; 32],
    },
}

/// Adapter-produced finalized LEZ asset first-lock facts.
///
/// The observed plan is built from the adapter's decoded transactions. Validation
/// compares its semantic steps, ordered public IDs, and exact bytes with the
/// prepared plan; it is not treated as trusted preparation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct LezAssetFirstLockEvidenceV1 {
    genesis_block_hash: [u8; 32],
    observed_plan: ExactPublicEffectPlanV1,
    metadata_account: [u8; 32],
    custody: LezAssetCustodyEvidenceV1,
    amount: u128,
    finalized: bool,
}

impl LezAssetFirstLockEvidenceV1 {
    /// Captures complete adapter-produced asset escrow facts.
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        genesis_block_hash: [u8; 32],
        observed_plan: ExactPublicEffectPlanV1,
        metadata_account: [u8; 32],
        custody: LezAssetCustodyEvidenceV1,
        amount: u128,
        finalized: bool,
    ) -> Self {
        Self {
            genesis_block_hash,
            observed_plan,
            metadata_account,
            custody,
            amount,
            finalized,
        }
    }

    /// Adapter-produced exact ordered semantic steps, IDs, and bytes.
    pub const fn observed_plan(&self) -> &ExactPublicEffectPlanV1 {
        &self.observed_plan
    }

    /// Finalized LEZ network identity observed by the adapter.
    #[must_use]
    pub const fn genesis_block_hash(&self) -> &[u8; 32] {
        &self.genesis_block_hash
    }

    /// Observed witnessed-escrow metadata account.
    #[must_use]
    pub const fn metadata_account(&self) -> &[u8; 32] {
        &self.metadata_account
    }

    /// Observed native or custom-token custody identity.
    pub const fn custody(&self) -> &LezAssetCustodyEvidenceV1 {
        &self.custody
    }

    /// Observed full-width native or token amount.
    #[must_use]
    pub const fn amount(&self) -> u128 {
        self.amount
    }

    /// Whether the adapter proved the observation against a finalized chain view.
    #[must_use]
    pub const fn finalized(&self) -> bool {
        self.finalized
    }
}

/// Direction-specific adapter-produced taker first-lock evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcLezAssetFirstLockEvidenceV1 {
    /// Canonical Bitcoin transaction and confirmation facts.
    Bitcoin(BitcoinFirstLockEvidenceV1),
    /// Finalized asset-specific LEZ escrow facts.
    Lez(LezAssetFirstLockEvidenceV1),
}

/// Opaque authorization proving the exact taker first lock was confirmed.
///
/// The value cannot be directly constructed. It explicitly binds the base
/// agreement, countersigned asset, first-lock plan, and direction.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct ConfirmedBtcLezAssetFirstLockV1 {
    base_agreement_commitment: [u8; 32],
    asset_commitment: [u8; 32],
    first_plan_commitment: [u8; 32],
    direction: SwapDirection,
}

impl ConfirmedBtcLezAssetFirstLockV1 {
    /// Exact accepted base agreement commitment.
    #[must_use]
    pub const fn base_agreement_commitment(&self) -> &[u8; 32] {
        &self.base_agreement_commitment
    }

    /// Exact countersigned asset-extension commitment.
    #[must_use]
    pub const fn asset_commitment(&self) -> &[u8; 32] {
        &self.asset_commitment
    }

    /// Exact validated taker first-lock plan commitment.
    #[must_use]
    pub const fn first_plan_commitment(&self) -> &[u8; 32] {
        &self.first_plan_commitment
    }

    /// Signed swap direction selecting the taker first-lock chain.
    #[must_use]
    pub const fn direction(&self) -> SwapDirection {
        self.direction
    }
}

/// Role-fixed asset-bound activation with no chain or submission capability.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct ActiveBtcLezAssetSwapV1 {
    accepted: AcceptedBtcAgreementV1,
    lock_effects: BtcLezAssetPreparedLockEffectsV1,
}

impl ActiveBtcLezAssetSwapV1 {
    /// Immutable validated base agreement.
    #[must_use]
    pub const fn agreement(&self) -> &BtcAgreementV1 {
        self.accepted.agreement()
    }

    /// Role fixed by the SDK facade at activation.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.accepted.local_participant()
    }

    /// Exact validated countersigned asset extension.
    #[must_use]
    pub const fn asset_extension(&self) -> &BtcLezAssetExtensionV1 {
        self.lock_effects.asset_extension()
    }

    /// Complete exact agreement-bound lock preparation.
    pub const fn prepared_lock_effects(&self) -> &BtcLezAssetPreparedLockEffectsV1 {
        &self.lock_effects
    }

    /// Exact taker-funded first-lock plan; this grants no submission authority.
    pub fn first_lock_plan(&self) -> &ExactPublicEffectPlanV1 {
        self.lock_effects
            .plan_for_participant(self.agreement(), Participant::Taker)
    }

    /// Validates adapter-produced taker first-lock evidence.
    ///
    /// # Errors
    ///
    /// Rejects chain, network, finality, plan, output, custody, metadata, amount,
    /// or extension substitution.
    pub fn validate_first_lock(
        &self,
        evidence: &BtcLezAssetFirstLockEvidenceV1,
    ) -> Result<ConfirmedBtcLezAssetFirstLockV1, BtcLezAssetSdkError> {
        match (self.agreement().direction(), evidence) {
            (
                SwapDirection::TakerSellsForeign,
                BtcLezAssetFirstLockEvidenceV1::Bitcoin(observed),
            ) => validate_bitcoin_first_lock(self.agreement(), self.first_lock_plan(), observed)?,
            (SwapDirection::TakerSellsLez, BtcLezAssetFirstLockEvidenceV1::Lez(observed)) => {
                validate_lez_asset_first_lock(
                    self.agreement(),
                    self.asset_extension(),
                    self.first_lock_plan(),
                    observed,
                )?;
            }
            _ => return Err(BtcLezAssetSdkError::WrongAssetFirstLockChain),
        }
        Ok(ConfirmedBtcLezAssetFirstLockV1 {
            base_agreement_commitment: *self.agreement().agreement_commitment(),
            asset_commitment: *self.asset_extension().asset_commitment(),
            first_plan_commitment: self.first_lock_plan().commitment(),
            direction: self.agreement().direction(),
        })
    }

    /// Releases the maker-funded second-lock plan only for this exact confirmation.
    ///
    /// # Errors
    ///
    /// Rejects a token from another base agreement, asset, first plan, or direction.
    pub fn second_lock_plan(
        &self,
        first: &ConfirmedBtcLezAssetFirstLockV1,
    ) -> Result<&ExactPublicEffectPlanV1, BtcLezAssetSdkError> {
        if first.base_agreement_commitment != *self.agreement().agreement_commitment()
            || first.asset_commitment != *self.asset_extension().asset_commitment()
            || first.first_plan_commitment != self.first_lock_plan().commitment()
            || first.direction != self.agreement().direction()
        {
            return Err(BtcLezAssetSdkError::AssetFirstLockConfirmationMismatch);
        }
        Ok(self
            .lock_effects
            .plan_for_participant(self.agreement(), Participant::Maker))
    }
}

impl BtcPairSdk {
    /// Activates exact asset-bound lock preparation under this facade's fixed role.
    ///
    /// # Errors
    ///
    /// Rejects local-role, base agreement, extension, asset-plan, or Bitcoin
    /// funding substitution.
    pub fn activate_asset(
        &self,
        accepted: AcceptedBtcAgreementV1,
        lock_effects: BtcLezAssetPreparedLockEffectsV1,
    ) -> Result<ActiveBtcLezAssetSwapV1, BtcLezAssetSdkError> {
        if accepted.local_participant() != self.local_participant() {
            return Err(BtcLezAssetSdkError::LocalRoleMismatch {
                expected: self.local_participant(),
                actual: accepted.local_participant(),
            });
        }
        lock_effects.validate(accepted.agreement())?;
        Ok(ActiveBtcLezAssetSwapV1 {
            accepted,
            lock_effects,
        })
    }
}

/// Additive F7 lock preparation and authorization failures.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum BtcLezAssetSdkError {
    /// LEZ plan does not have the asset-kind-specific semantic step order.
    #[error("LEZ asset plan has the wrong semantic step order")]
    LezAssetPlanShape,
    /// Two LEZ steps claim the same public transaction identity.
    #[error("LEZ asset plan contains a duplicate transaction identity")]
    DuplicateLezAssetEffectIdentity,
    /// Two LEZ steps contain identical exact transaction bytes.
    #[error("LEZ asset plan contains duplicate exact transaction bytes")]
    DuplicateLezAssetEffectBytes,
    /// Validated extension belongs to another base agreement.
    #[error("LEZ asset extension belongs to another base agreement")]
    AssetExtensionAgreementMismatch,
    /// Native/token plan kind differs from the countersigned extension.
    #[error("LEZ asset plan kind differs from the countersigned extension")]
    AssetPlanKindMismatch,
    /// Exact Bitcoin transaction identity differs from the accepted agreement.
    #[error("Bitcoin funding identity differs from the accepted agreement")]
    BitcoinFundingAgreementMismatch,
    /// Exact Bitcoin output differs from the accepted amount or P2TR destination.
    #[error("Bitcoin funding output differs from the accepted agreement")]
    BitcoinFundingOutputMismatch,
    /// Accepted role differs from the facade's fixed role.
    #[error("accepted local role is {actual:?}; SDK role is {expected:?}")]
    LocalRoleMismatch {
        /// Role fixed on the SDK.
        expected: Participant,
        /// Role carried by the accepted agreement.
        actual: Participant,
    },
    /// Evidence uses the wrong direction-derived taker first-lock chain.
    #[error("asset first-lock evidence uses the wrong chain")]
    WrongAssetFirstLockChain,
    /// Evidence identifies another Bitcoin or LEZ network.
    #[error("asset first-lock evidence identifies another network")]
    AssetFirstLockNetworkMismatch,
    /// Adapter-produced semantic steps, IDs, or bytes differ from preparation.
    #[error("asset first-lock evidence differs from the exact prepared plan")]
    AssetFirstLockPlanMismatch,
    /// Bitcoin first-lock evidence has not reached the signed policy.
    #[error("Bitcoin asset first lock has {actual} confirmations; requires {required}")]
    AssetFirstLockConfirmationLag {
        /// Signed required Bitcoin confirmation count.
        required: u32,
        /// Adapter-observed canonical confirmation count.
        actual: u32,
    },
    /// LEZ asset evidence is not tied to a finalized chain view.
    #[error("LEZ asset first lock is not finalized")]
    AssetFirstLockNotFinalized,
    /// Metadata, custody identity, token definition, amount, or Bitcoin output differs.
    #[error("asset first-lock terms differ from the accepted extension")]
    AssetFirstLockTermsMismatch,
    /// Confirmation belongs to another agreement, asset, first plan, or direction.
    #[error("confirmed asset first lock is not bound to this prepared swap")]
    AssetFirstLockConfirmationMismatch,
}

fn validate_lez_asset_plan(
    plan: &ExactPublicEffectPlanV1,
    kind: LezAssetPlanKindV1,
) -> Result<(), BtcLezAssetSdkError> {
    let expected_steps: &[&str] = match kind {
        LezAssetPlanKindV1::Native => &[LEZ_INITIALIZE_STEP, LEZ_FUND_STEP],
        LezAssetPlanKindV1::CustomToken => &[
            LEZ_INITIALIZE_STEP,
            LEZ_CREATE_CUSTODY_ATA_STEP,
            LEZ_FUND_STEP,
        ],
    };
    if plan.steps().len() != expected_steps.len()
        || plan
            .steps()
            .iter()
            .zip(expected_steps)
            .any(|(effect, expected)| effect.step().as_str() != *expected)
    {
        return Err(BtcLezAssetSdkError::LezAssetPlanShape);
    }
    validate_unique_lez_effect_material(plan.steps())
}

fn validate_unique_lez_effect_material(
    effects: &[PublicEffectStepV1],
) -> Result<(), BtcLezAssetSdkError> {
    let mut transaction_ids = HashSet::with_capacity(effects.len());
    let mut exact_bytes = HashSet::with_capacity(effects.len());
    for effect in effects {
        if !transaction_ids.insert(effect.expected_public_id().as_str()) {
            return Err(BtcLezAssetSdkError::DuplicateLezAssetEffectIdentity);
        }
        if !exact_bytes.insert(effect.exact_bytes().as_slice()) {
            return Err(BtcLezAssetSdkError::DuplicateLezAssetEffectBytes);
        }
    }
    Ok(())
}

fn validate_bitcoin_funding(
    agreement: &BtcAgreementV1,
    prepared: &PreparedBitcoinFundingV1,
) -> Result<(), BtcLezAssetSdkError> {
    let expected_txid = Txid::from_byte_array(*agreement.funding_terms().transaction_id());
    let [step] = prepared.plan().steps() else {
        return Err(BtcLezAssetSdkError::BitcoinFundingAgreementMismatch);
    };
    if step.step().as_str() != BITCOIN_FUNDING_STEP
        || prepared.transaction_id() != expected_txid
        || step.expected_public_id().as_str() != expected_txid.to_string()
    {
        return Err(BtcLezAssetSdkError::BitcoinFundingAgreementMismatch);
    }
    let transaction: Transaction = deserialize(step.exact_bytes().as_slice())
        .map_err(|_| BtcLezAssetSdkError::BitcoinFundingAgreementMismatch)?;
    validate_bitcoin_output(agreement, &transaction)
}

fn validate_bitcoin_first_lock(
    agreement: &BtcAgreementV1,
    prepared_plan: &ExactPublicEffectPlanV1,
    observed: &BitcoinFirstLockEvidenceV1,
) -> Result<(), BtcLezAssetSdkError> {
    if observed.genesis_block_hash() != agreement.bitcoin_genesis_hash() {
        return Err(BtcLezAssetSdkError::AssetFirstLockNetworkMismatch);
    }
    let required = agreement.required_bitcoin_confirmations();
    if observed.confirmations() < required {
        return Err(BtcLezAssetSdkError::AssetFirstLockConfirmationLag {
            required,
            actual: observed.confirmations(),
        });
    }
    let [prepared] = prepared_plan.steps() else {
        return Err(BtcLezAssetSdkError::AssetFirstLockPlanMismatch);
    };
    if observed.exact_transaction() != prepared.exact_bytes() {
        return Err(BtcLezAssetSdkError::AssetFirstLockPlanMismatch);
    }
    let transaction: Transaction = deserialize(observed.exact_transaction().as_slice())
        .map_err(|_| BtcLezAssetSdkError::AssetFirstLockPlanMismatch)?;
    if transaction.compute_txid().to_string() != prepared.expected_public_id().as_str() {
        return Err(BtcLezAssetSdkError::AssetFirstLockPlanMismatch);
    }
    validate_bitcoin_output(agreement, &transaction)
        .map_err(|_| BtcLezAssetSdkError::AssetFirstLockTermsMismatch)
}

fn validate_bitcoin_output(
    agreement: &BtcAgreementV1,
    transaction: &Transaction,
) -> Result<(), BtcLezAssetSdkError> {
    let output = transaction
        .output
        .get(agreement.funding_terms().output_index() as usize)
        .ok_or(BtcLezAssetSdkError::BitcoinFundingOutputMismatch)?;
    if output.value.to_sat() != agreement.funding_terms().value_sat()
        || output.script_pubkey.as_bytes() != agreement.p2tr_contract().script_pubkey_bytes()
    {
        return Err(BtcLezAssetSdkError::BitcoinFundingOutputMismatch);
    }
    Ok(())
}

fn validate_lez_asset_first_lock(
    agreement: &BtcAgreementV1,
    extension: &BtcLezAssetExtensionV1,
    prepared_plan: &ExactPublicEffectPlanV1,
    observed: &LezAssetFirstLockEvidenceV1,
) -> Result<(), BtcLezAssetSdkError> {
    if !observed.finalized() {
        return Err(BtcLezAssetSdkError::AssetFirstLockNotFinalized);
    }
    if observed.genesis_block_hash() != agreement.lez_terms().genesis_block_hash() {
        return Err(BtcLezAssetSdkError::AssetFirstLockNetworkMismatch);
    }
    if observed.observed_plan() != prepared_plan {
        return Err(BtcLezAssetSdkError::AssetFirstLockPlanMismatch);
    }
    if observed.metadata_account() != agreement.lez_terms().metadata_account() {
        return Err(BtcLezAssetSdkError::AssetFirstLockTermsMismatch);
    }
    match (extension.asset(), observed.custody()) {
        (BtcLezAssetV1::Native, LezAssetCustodyEvidenceV1::Native { custody_account })
            if custody_account == agreement.lez_terms().custody_account()
                && observed.amount() == agreement.lez_terms().amount() => {}
        (
            BtcLezAssetV1::CustomToken(token),
            LezAssetCustodyEvidenceV1::CustomToken {
                custody_ata_account,
                token_definition_account,
            },
        ) if custody_ata_account == token.custody_ata_account()
            && token_definition_account == token.token_definition_account()
            && observed.amount() == token.amount() => {}
        _ => return Err(BtcLezAssetSdkError::AssetFirstLockTermsMismatch),
    }
    Ok(())
}
