//! Role-fixed deterministic LEZ/Bitcoin lifecycle facade.
//!
//! This module performs no node, discovery, negotiation, persistence, claim,
//! or refund I/O. It validates exact lock and revealing-claim evidence, then
//! deterministically constructs the material-consuming follow-up claim and
//! projects exact signed refunds from stable canonical timeout evidence.

use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::Hash as _;
use bitcoin::{Transaction, Txid, Witness};
use std::error::Error;

use lez_swap_core::{
    Chain, ChainPosition, ChainProof, ClaimEvidence, Error as CoreError, Participant, Phase,
    SwapCoordinator, SwapDirection, SwapId,
};
use lez_swap_sdk_core::{
    ClaimLeg, ClaimOrder, ErrorCategory, ExactPublicEffectBytes, ExactPublicEffectPlanV1,
    ExpectedPublicEffectId, NegotiationChannel, OfferDiscovery, ProtocolError,
    PublicEffectPlanError, PublicEffectStepId, PublicEffectStepV1, SwapProtocol,
};

use zeroize::Zeroizing;

use crate::{
    AdaptorSessionError, BtcAdaptorSessionDomain, BtcAgreementRecordV1, BtcAgreementV1,
    BtcAgreementV1Error, BtcChainPolicyV1, CooperativeKeyPathSpendError,
    RefundScriptPathSpendError, adapt_presignature, extract_adaptor_secret,
    verify_adaptor_presignature, verify_final_signature,
};

const BITCOIN_FUNDING_STEP: &str = "bitcoin.funding";
const LEZ_INITIALIZE_STEP: &str = "lez.initialize";
const LEZ_FUND_STEP: &str = "lez.fund";
const BITCOIN_CLAIM_STEP: &str = "bitcoin.claim";
const LEZ_CLAIM_STEP: &str = "lez.claim";
const BITCOIN_REFUND_STEP: &str = "bitcoin.refund";
const LEZ_REFUND_STEP: &str = "lez.refund";
const SCHNORR_SIGNATURE_BYTES: usize = 64;

/// Exact signed Bitcoin funding effect supplied before activation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct PreparedBitcoinFundingV1 {
    transaction_id: Txid,
    plan: ExactPublicEffectPlanV1,
}

impl PreparedBitcoinFundingV1 {
    /// Parses exact signed transaction bytes and binds their computed txid to
    /// the caller-supplied expected public identity.
    ///
    /// # Errors
    ///
    /// Rejects malformed/unsigned transaction bytes, an invalid public ID, or
    /// an expected ID different from the non-witness transaction ID.
    pub fn new(
        expected_transaction_id: impl Into<Box<str>>,
        exact_signed_transaction: impl Into<Box<[u8]>>,
    ) -> Result<Self, BtcSdkError> {
        let expected_transaction_id = ExpectedPublicEffectId::new(expected_transaction_id)?;
        let exact_signed_transaction = ExactPublicEffectBytes::new(exact_signed_transaction)?;
        let transaction = parse_signed_bitcoin_funding(&exact_signed_transaction)?;
        let transaction_id = transaction.compute_txid();
        if expected_transaction_id.as_str() != transaction_id.to_string() {
            return Err(BtcSdkError::BitcoinFundingIdentityMismatch);
        }
        let plan = ExactPublicEffectPlanV1::new(vec![PublicEffectStepV1::new(
            PublicEffectStepId::new(BITCOIN_FUNDING_STEP)?,
            expected_transaction_id,
            exact_signed_transaction,
        )])?;
        Ok(Self {
            transaction_id,
            plan,
        })
    }

    /// Computed Bitcoin transaction ID.
    #[must_use]
    pub const fn transaction_id(&self) -> Txid {
        self.transaction_id
    }

    /// Immutable one-step exact-public-effect plan.
    pub const fn plan(&self) -> &ExactPublicEffectPlanV1 {
        &self.plan
    }

    fn transaction(&self) -> Result<Transaction, BtcSdkError> {
        parse_signed_bitcoin_funding(self.plan.steps()[0].exact_bytes())
    }
}

/// Exact ordered LEZ escrow initialization and funding effects.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct PreparedLezFundingV1 {
    plan: ExactPublicEffectPlanV1,
}

impl PreparedLezFundingV1 {
    /// Builds a two-step initialize-then-fund plan from complete signed bytes
    /// and chain-native expected transaction identities.
    ///
    /// # Errors
    ///
    /// Rejects invalid/duplicate public identities or invalid exact bytes.
    pub fn new(
        initialization_public_id: impl Into<Box<str>>,
        exact_signed_initialization: impl Into<Box<[u8]>>,
        funding_public_id: impl Into<Box<str>>,
        exact_signed_funding: impl Into<Box<[u8]>>,
    ) -> Result<Self, BtcSdkError> {
        let initialization_public_id = ExpectedPublicEffectId::new(initialization_public_id)?;
        let funding_public_id = ExpectedPublicEffectId::new(funding_public_id)?;
        if initialization_public_id == funding_public_id {
            return Err(BtcSdkError::DuplicateLezEffectIdentity);
        }
        let plan = ExactPublicEffectPlanV1::new(vec![
            PublicEffectStepV1::new(
                PublicEffectStepId::new(LEZ_INITIALIZE_STEP)?,
                initialization_public_id,
                ExactPublicEffectBytes::new(exact_signed_initialization)?,
            ),
            PublicEffectStepV1::new(
                PublicEffectStepId::new(LEZ_FUND_STEP)?,
                funding_public_id,
                ExactPublicEffectBytes::new(exact_signed_funding)?,
            ),
        ])?;
        Ok(Self { plan })
    }

    /// Immutable initialize-then-fund exact-public-effect plan.
    pub const fn plan(&self) -> &ExactPublicEffectPlanV1 {
        &self.plan
    }
}

/// Exact effects for both possible direction-derived funding legs.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcPreparedLockEffectsV1 {
    bitcoin: PreparedBitcoinFundingV1,
    lez: PreparedLezFundingV1,
}

impl BtcPreparedLockEffectsV1 {
    /// Combines already prepared Bitcoin and LEZ effects.
    pub const fn new(bitcoin: PreparedBitcoinFundingV1, lez: PreparedLezFundingV1) -> Self {
        Self { bitcoin, lez }
    }

    /// Exact Bitcoin funding effect.
    pub const fn bitcoin(&self) -> &PreparedBitcoinFundingV1 {
        &self.bitcoin
    }

    /// Exact ordered LEZ effects.
    pub const fn lez(&self) -> &PreparedLezFundingV1 {
        &self.lez
    }

    /// Returns the effect funded by one immutable protocol role.
    pub fn plan_for_participant(
        &self,
        agreement: &BtcAgreementV1,
        participant: Participant,
    ) -> &ExactPublicEffectPlanV1 {
        self.plan_for_chain(agreement.coordinator().funded_chain(participant))
    }

    fn plan_for_chain(&self, chain: Chain) -> &ExactPublicEffectPlanV1 {
        match chain {
            Chain::Bitcoin => self.bitcoin.plan(),
            Chain::Lez => self.lez.plan(),
            Chain::Monero | Chain::Zcash => unreachable!("a validated BTC agreement has two legs"),
        }
    }

    fn validate(&self, agreement: &BtcAgreementV1) -> Result<(), BtcSdkError> {
        let transaction = self.bitcoin.transaction()?;
        let expected_txid = Txid::from_byte_array(*agreement.funding_terms().transaction_id());
        if self.bitcoin.transaction_id != expected_txid {
            return Err(BtcSdkError::BitcoinFundingAgreementMismatch);
        }
        let output = transaction
            .output
            .get(agreement.funding_terms().output_index() as usize)
            .ok_or(BtcSdkError::BitcoinFundingOutputMismatch)?;
        if output.value.to_sat() != agreement.funding_terms().value_sat()
            || output.script_pubkey.as_bytes() != agreement.p2tr_contract().script_pubkey_bytes()
        {
            return Err(BtcSdkError::BitcoinFundingOutputMismatch);
        }
        Ok(())
    }
}

/// Exact bounded LEZ claim envelope with a canonical Schnorr signature slot.
///
/// The SDK does not reinterpret LEZ transaction encoding. Instead, the LEZ
/// adapter supplies the exact public envelope it already knows how to submit,
/// with one 64-byte zero placeholder. Claim construction replaces only that
/// slot after verifying/adapting the agreement-bound presignature.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct PreparedLezClaimTemplateV1 {
    expected_public_id: ExpectedPublicEffectId,
    exact_template: ExactPublicEffectBytes,
    signature_offset: usize,
}

impl PreparedLezClaimTemplateV1 {
    /// Validates a bounded exact envelope and its unique 64-byte zero slot.
    ///
    /// # Errors
    ///
    /// Rejects an invalid public ID, empty/oversized bytes, an out-of-bounds
    /// offset, or a nonzero signature placeholder.
    pub fn new(
        expected_public_id: impl Into<Box<str>>,
        exact_template: impl Into<Box<[u8]>>,
        signature_offset: usize,
    ) -> Result<Self, BtcSdkError> {
        let expected_public_id = ExpectedPublicEffectId::new(expected_public_id)?;
        let exact_template = ExactPublicEffectBytes::new(exact_template)?;
        let signature_end = signature_offset
            .checked_add(SCHNORR_SIGNATURE_BYTES)
            .ok_or(BtcSdkError::InvalidLezClaimSignatureSlot)?;
        if exact_template
            .as_slice()
            .get(signature_offset..signature_end)
            != Some([0_u8; SCHNORR_SIGNATURE_BYTES].as_slice())
        {
            return Err(BtcSdkError::InvalidLezClaimSignatureSlot);
        }
        Ok(Self {
            expected_public_id,
            exact_template,
            signature_offset,
        })
    }

    fn materialize(
        &self,
        signature: [u8; SCHNORR_SIGNATURE_BYTES],
    ) -> Result<ExactPublicEffectBytes, BtcSdkError> {
        let mut exact = self.exact_template.as_slice().to_vec();
        let signature_end = self.signature_offset + SCHNORR_SIGNATURE_BYTES;
        exact[self.signature_offset..signature_end].copy_from_slice(&signature);
        ExactPublicEffectBytes::new(exact).map_err(Into::into)
    }
}

/// Complete public claim preparation required before lock construction.
///
/// Presignatures remain secret-free public cryptographic material. They are
/// verified under deterministic contexts derived from the countersigned
/// agreement during term validation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcPreparedClaimEffectsV1 {
    agreement_commitment: [u8; 32],
    bitcoin_presignature: [u8; 65],
    lez_presignature: [u8; 65],
    lez_claim: PreparedLezClaimTemplateV1,
}

impl BtcPreparedClaimEffectsV1 {
    /// Combines exact dual-chain presignatures and the bounded LEZ claim envelope.
    pub fn new(
        agreement: &BtcAgreementV1,
        bitcoin_presignature: [u8; 65],
        lez_presignature: [u8; 65],
        lez_claim: PreparedLezClaimTemplateV1,
    ) -> Self {
        Self {
            agreement_commitment: *agreement.agreement_commitment(),
            bitcoin_presignature,
            lez_presignature,
            lez_claim,
        }
    }

    fn validate(&self, agreement: &BtcAgreementV1) -> Result<(), BtcSdkError> {
        if self.agreement_commitment != *agreement.agreement_commitment() {
            return Err(BtcSdkError::ClaimPreparationAgreementMismatch);
        }
        let bitcoin = agreement
            .claim_adaptor_session_context(BtcAdaptorSessionDomain::Bitcoin)
            .map_err(BtcSdkError::InvalidAdaptorClaim)?;
        verify_adaptor_presignature(&bitcoin, self.bitcoin_presignature)
            .map_err(BtcSdkError::InvalidAdaptorClaim)?;
        let lez = agreement
            .claim_adaptor_session_context(BtcAdaptorSessionDomain::Lez)
            .map_err(BtcSdkError::InvalidAdaptorClaim)?;
        verify_adaptor_presignature(&lez, self.lez_presignature)
            .map_err(BtcSdkError::InvalidAdaptorClaim)
    }
}

/// Agreement-bound exact signed Bitcoin script-path refund.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct PreparedBitcoinRefundV1 {
    agreement_commitment: [u8; 32],
    signature: [u8; SCHNORR_SIGNATURE_BYTES],
    transaction_id: Txid,
    plan: ExactPublicEffectPlanV1,
}

impl PreparedBitcoinRefundV1 {
    /// Finalizes the agreement-reconstructed BIP-342 refund with the funder's
    /// signature and captures the exact canonical transaction.
    ///
    /// # Errors
    ///
    /// Rejects an invalid signature or exact-effect construction failure.
    pub fn new(
        agreement: &BtcAgreementV1,
        signature: [u8; SCHNORR_SIGNATURE_BYTES],
    ) -> Result<Self, BtcSdkError> {
        let transaction = agreement
            .bitcoin_refund()
            .clone()
            .finalize(signature)
            .map_err(BtcSdkError::InvalidBitcoinRefund)?;
        let transaction_id = transaction.compute_txid();
        let plan = ExactPublicEffectPlanV1::new(vec![PublicEffectStepV1::new(
            PublicEffectStepId::new(BITCOIN_REFUND_STEP)?,
            ExpectedPublicEffectId::new(transaction_id.to_string())?,
            ExactPublicEffectBytes::new(serialize(&transaction))?,
        )])?;
        Ok(Self {
            agreement_commitment: *agreement.agreement_commitment(),
            signature,
            transaction_id,
            plan,
        })
    }

    /// Canonical signed refund transaction ID.
    #[must_use]
    pub const fn transaction_id(&self) -> Txid {
        self.transaction_id
    }

    /// Exact immutable signed refund plan.
    pub const fn plan(&self) -> &ExactPublicEffectPlanV1 {
        &self.plan
    }

    fn validate(&self, agreement: &BtcAgreementV1) -> Result<(), BtcSdkError> {
        if self.agreement_commitment != *agreement.agreement_commitment() {
            return Err(BtcSdkError::RecoveryPreparationAgreementMismatch);
        }
        let reconstructed = Self::new(agreement, self.signature)?;
        if reconstructed.transaction_id != self.transaction_id || reconstructed.plan != self.plan {
            return Err(BtcSdkError::BitcoinRefundPlanMismatch);
        }
        Ok(())
    }
}

/// Agreement-bound exact signed LEZ refund effect supplied by the LEZ adapter.
///
/// The BTC SDK does not reinterpret LEZ transaction encoding. It preserves the
/// adapter-produced signed envelope byte-for-byte and binds it to the validated
/// agreement before any first lock can be constructed.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct PreparedLezRefundV1 {
    agreement_commitment: [u8; 32],
    plan: ExactPublicEffectPlanV1,
}

impl PreparedLezRefundV1 {
    /// Captures one bounded exact signed LEZ refund effect.
    ///
    /// # Errors
    ///
    /// Rejects an invalid public ID or empty/oversized exact bytes.
    pub fn new(
        agreement: &BtcAgreementV1,
        expected_public_id: impl Into<Box<str>>,
        exact_signed_refund: impl Into<Box<[u8]>>,
    ) -> Result<Self, BtcSdkError> {
        let plan = ExactPublicEffectPlanV1::new(vec![PublicEffectStepV1::new(
            PublicEffectStepId::new(LEZ_REFUND_STEP)?,
            ExpectedPublicEffectId::new(expected_public_id)?,
            ExactPublicEffectBytes::new(exact_signed_refund)?,
        )])?;
        Ok(Self {
            agreement_commitment: *agreement.agreement_commitment(),
            plan,
        })
    }

    /// Exact immutable signed LEZ refund plan.
    pub const fn plan(&self) -> &ExactPublicEffectPlanV1 {
        &self.plan
    }

    fn validate(&self, agreement: &BtcAgreementV1) -> Result<(), BtcSdkError> {
        if self.agreement_commitment != *agreement.agreement_commitment() {
            return Err(BtcSdkError::RecoveryPreparationAgreementMismatch);
        }
        Ok(())
    }
}

/// Complete agreement-bound signed recovery effects required before locking.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcPreparedRecoveryEffectsV1 {
    bitcoin: PreparedBitcoinRefundV1,
    lez: PreparedLezRefundV1,
}

impl BtcPreparedRecoveryEffectsV1 {
    /// Combines both already signed refund effects.
    pub const fn new(bitcoin: PreparedBitcoinRefundV1, lez: PreparedLezRefundV1) -> Self {
        Self { bitcoin, lez }
    }

    /// Exact signed Bitcoin refund.
    pub const fn bitcoin(&self) -> &PreparedBitcoinRefundV1 {
        &self.bitcoin
    }

    /// Exact signed LEZ refund.
    pub const fn lez(&self) -> &PreparedLezRefundV1 {
        &self.lez
    }

    fn plan_for_chain(&self, chain: Chain) -> &ExactPublicEffectPlanV1 {
        match chain {
            Chain::Bitcoin => self.bitcoin.plan(),
            Chain::Lez => self.lez.plan(),
            Chain::Monero | Chain::Zcash => unreachable!("validated BTC agreement"),
        }
    }

    fn validate(&self, agreement: &BtcAgreementV1) -> Result<(), BtcSdkError> {
        self.bitcoin.validate(agreement)?;
        self.lez.validate(agreement)
    }
}

/// Untrusted terms consumed by the deterministic common lifecycle contract.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcProtocolTermsV1 {
    agreement: BtcAgreementRecordV1,
    lock_effects: BtcPreparedLockEffectsV1,
    claim_effects: Option<BtcPreparedClaimEffectsV1>,
    recovery_effects: Option<BtcPreparedRecoveryEffectsV1>,
}

impl BtcProtocolTermsV1 {
    /// Combines a countersigned agreement record and exact prepared locks.
    pub const fn new(
        agreement: BtcAgreementRecordV1,
        lock_effects: BtcPreparedLockEffectsV1,
    ) -> Self {
        Self {
            agreement,
            lock_effects,
            claim_effects: None,
            recovery_effects: None,
        }
    }

    /// Adds complete public claim preparation for the common lifecycle trait.
    pub fn with_claim_effects(mut self, claim_effects: BtcPreparedClaimEffectsV1) -> Self {
        self.claim_effects = Some(claim_effects);
        self
    }

    /// Adds both agreement-bound signed refund effects required by full prepare.
    pub fn with_recovery_effects(mut self, recovery_effects: BtcPreparedRecoveryEffectsV1) -> Self {
        self.recovery_effects = Some(recovery_effects);
        self
    }
}

/// Fully validated agreement and exact lock effects.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct ValidatedBtcProtocolTermsV1 {
    agreement: BtcAgreementV1,
    lock_effects: BtcPreparedLockEffectsV1,
    claim_effects: Option<BtcPreparedClaimEffectsV1>,
    recovery_effects: Option<BtcPreparedRecoveryEffectsV1>,
}

impl ValidatedBtcProtocolTermsV1 {
    /// Validated immutable agreement.
    #[must_use]
    pub const fn agreement(&self) -> &BtcAgreementV1 {
        &self.agreement
    }
}

/// Deterministic protocol state prepared for claims and, when present, refunds.
///
/// Both agreement-bound adaptor presignatures and the exact LEZ substitution
/// template have been verified. Values returned by [`SwapProtocol::prepare`]
/// also contain both exact signed refunds; [`BtcPairSdk::prepare_claims`]
/// intentionally creates the smaller claim-only form.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcPreparedProtocolV1 {
    terms: ValidatedBtcProtocolTermsV1,
}

impl BtcPreparedProtocolV1 {
    /// Validated agreement used by every direction/role decision.
    #[must_use]
    pub const fn agreement(&self) -> &BtcAgreementV1 {
        self.terms.agreement()
    }

    fn claim_effects(&self) -> &BtcPreparedClaimEffectsV1 {
        self.terms
            .claim_effects
            .as_ref()
            .expect("prepare proves complete claim effects")
    }

    /// Exact signed refund effects proven present by full preparation.
    ///
    /// Returning a plan grants no submission authority; role-fixed recovery
    /// projection determines when one local effect is safe.
    #[must_use]
    pub const fn recovery_effects(&self) -> Option<&BtcPreparedRecoveryEffectsV1> {
        self.terms.recovery_effects.as_ref()
    }

    fn required_recovery_effects(&self) -> Result<&BtcPreparedRecoveryEffectsV1, BtcSdkError> {
        self.recovery_effects()
            .ok_or(BtcSdkError::MissingRecoveryEffects)
    }
}

/// Accepted role-local countersigned agreement at revision zero.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct AcceptedBtcAgreementV1 {
    agreement: BtcAgreementV1,
    wire: Box<[u8]>,
    local_participant: Participant,
}

impl AcceptedBtcAgreementV1 {
    /// Validated immutable agreement.
    #[must_use]
    pub const fn agreement(&self) -> &BtcAgreementV1 {
        &self.agreement
    }

    /// Role fixed when the agreement was accepted.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Initial durable revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        0
    }
}

/// Boxed discovery/negotiation error retaining its concrete source.
pub type BtcBoxPortError = Box<dyn Error + Send + Sync + 'static>;

/// Canonical confirmed Bitcoin follow-up-claim observation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BitcoinFollowupClaimEvidenceV1 {
    genesis_block_hash: [u8; 32],
    exact_transaction: ExactPublicEffectBytes,
    confirmations: u32,
}
impl BitcoinFollowupClaimEvidenceV1 {
    /// Captures canonical Bitcoin claim bytes and their confirmation count.
    ///
    /// # Errors
    ///
    /// Rejects malformed, non-canonical, or non-key-path-spend bytes.
    pub fn new(
        genesis_block_hash: [u8; 32],
        exact_transaction: impl Into<Box<[u8]>>,
        confirmations: u32,
    ) -> Result<Self, BtcSdkError> {
        let exact_transaction = ExactPublicEffectBytes::new(exact_transaction)?;
        let transaction = parse_bitcoin_revealing_claim(&exact_transaction)?;
        if serialize(&transaction) != exact_transaction.as_slice() {
            return Err(BtcSdkError::MalformedBitcoinClaim(
                bitcoin::consensus::encode::Error::ParseFailed(
                    "non-canonical Bitcoin claim transaction bytes",
                ),
            ));
        }
        Ok(Self {
            genesis_block_hash,
            exact_transaction,
            confirmations,
        })
    }
    /// Exact canonical transaction bytes observed by the adapter.
    pub const fn exact_transaction(&self) -> &ExactPublicEffectBytes {
        &self.exact_transaction
    }
}

/// Canonical finalized LEZ follow-up-claim observation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct LezFollowupClaimEvidenceV1 {
    genesis_block_hash: [u8; 32],
    public_id: ExpectedPublicEffectId,
    exact_claim: ExactPublicEffectBytes,
    finalized: bool,
}
impl LezFollowupClaimEvidenceV1 {
    /// Captures an adapter-validated finalized LEZ claim effect.
    ///
    /// # Errors
    ///
    /// Rejects an invalid identity or empty/oversized exact bytes.
    pub fn new(
        genesis_block_hash: [u8; 32],
        public_id: impl Into<Box<str>>,
        exact_claim: impl Into<Box<[u8]>>,
        finalized: bool,
    ) -> Result<Self, BtcSdkError> {
        Ok(Self {
            genesis_block_hash,
            public_id: ExpectedPublicEffectId::new(public_id)?,
            exact_claim: ExactPublicEffectBytes::new(exact_claim)?,
            finalized,
        })
    }
    /// Exact signed public envelope observed by the LEZ adapter.
    pub const fn exact_claim(&self) -> &ExactPublicEffectBytes {
        &self.exact_claim
    }
}

/// Direction-specific canonical follow-up-claim evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcFollowupClaimEvidenceV1 {
    /// Canonical confirmed Bitcoin follow-up claim.
    Bitcoin(BitcoinFollowupClaimEvidenceV1),
    /// Canonical finalized LEZ follow-up claim.
    Lez(LezFollowupClaimEvidenceV1),
}

/// One agreement-bound durable lifecycle transition.
///
/// The log is replayed from revision zero. Every entry carries canonical bytes
/// or recovery identities; a revision number alone is never trusted.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcLifecycleTransitionV1 {
    /// The taker-funded first lock became canonical.
    FirstLockConfirmed(BtcFirstLockEvidenceV1),
    /// The maker-funded second lock became canonical.
    SecondLockConfirmed(BtcFirstLockEvidenceV1),
    /// The agreement-selected revealing claim became canonical.
    RevealingClaimConfirmed(BtcRevealingClaimEvidenceV1),
    /// The material-consuming follow-up claim became canonical.
    FollowupClaimConfirmed(BtcFollowupClaimEvidenceV1),
    /// One agreement-prepared refund became canonical.
    RecoveryObserved(BtcCanonicalRecoveryStateV1),
}

/// Result of applying an exact durable lifecycle transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcLifecycleTransitionOutcomeV1 {
    /// A new transition advanced the durable head.
    Applied {
        /// New aggregate revision.
        revision: u64,
    },
    /// The same effect already exists; nothing was duplicated.
    AlreadyApplied {
        /// Existing aggregate revision.
        revision: u64,
    },
}

/// Pre-lock facade over application-supplied discovery and negotiation ports.
///
/// Activation returns [`ActiveBtcSwap`], which contains neither capability.
/// Delivery/Chat adapters remain application-owned and out of this M3 slice.
#[derive(Debug)]
pub struct BtcLifecycleSdk<Discovery, Negotiation> {
    pair: BtcPairSdk,
    discovery: Discovery,
    negotiation: Negotiation,
}
impl<Discovery, Negotiation> BtcLifecycleSdk<Discovery, Negotiation> {
    /// Composes the deterministic pair SDK with pre-lock ports.
    pub const fn new(pair: BtcPairSdk, discovery: Discovery, negotiation: Negotiation) -> Self {
        Self {
            pair,
            discovery,
            negotiation,
        }
    }
    /// Deterministic role/policy facade.
    pub const fn pair_sdk(&self) -> &BtcPairSdk {
        &self.pair
    }
    /// Publishes one application-defined authenticated offer.
    ///
    /// # Errors
    ///
    /// Preserves the discovery adapter source.
    pub async fn publish_offer(
        &self,
        offer: Discovery::Offer,
    ) -> Result<Discovery::OfferRef, BtcSdkError>
    where
        Discovery: OfferDiscovery,
    {
        self.discovery
            .publish(offer)
            .await
            .map_err(|error| BtcSdkError::Discovery(Box::new(error)))
    }
    /// Discovers authenticated, unexpired offers.
    ///
    /// # Errors
    ///
    /// Preserves the discovery adapter source.
    pub async fn discover(
        &self,
        query: &Discovery::Query,
    ) -> Result<Vec<Discovery::OfferRef>, BtcSdkError>
    where
        Discovery: OfferDiscovery,
    {
        self.discovery
            .discover(query)
            .await
            .map_err(|error| BtcSdkError::Discovery(Box::new(error)))
    }
    /// Negotiates and validates the returned countersigned wire.
    ///
    /// # Errors
    ///
    /// Preserves the negotiation source and agreement validation.
    pub async fn negotiate(
        &self,
        offer: &Discovery::OfferRef,
        proposal: Negotiation::LocalProposal,
    ) -> Result<AcceptedBtcAgreementV1, BtcSdkError>
    where
        Discovery: OfferDiscovery,
        Negotiation: NegotiationChannel<OfferRef = Discovery::OfferRef>,
    {
        let wire = self
            .negotiation
            .negotiate(self.pair.local_participant(), offer, proposal)
            .await
            .map_err(|error| BtcSdkError::Negotiation(Box::new(error)))?;
        self.pair.accept_wire(&wire)
    }
    /// Activates complete material and drops both ports from the result type.
    ///
    /// # Errors
    ///
    /// Rejects role, agreement, policy, or effect substitution.
    pub fn activate(
        &self,
        accepted: AcceptedBtcAgreementV1,
        prepared: BtcPreparedProtocolV1,
    ) -> Result<ActiveBtcSwap, BtcSdkError> {
        self.pair.activate_prepared(accepted, prepared)
    }
    /// Replays a durable lifecycle without either pre-lock port.
    ///
    /// # Errors
    ///
    /// Revalidates the complete transition log.
    pub fn resume(&self, envelope: BtcActiveSwapEnvelopeV1) -> Result<ActiveBtcSwap, BtcSdkError> {
        self.pair.resume(envelope)
    }
}

/// Untrusted durable activation envelope used for deterministic restart.
#[derive(Clone, Eq, PartialEq)]
#[must_use]
pub struct BtcActiveSwapEnvelopeV1 {
    agreement_wire: Box<[u8]>,
    local_participant: Participant,
    revision: u64,
    lock_effects: BtcPreparedLockEffectsV1,
    prepared_protocol: Option<BtcPreparedProtocolV1>,
    transitions: Vec<BtcLifecycleTransitionV1>,
}

impl std::fmt::Debug for BtcActiveSwapEnvelopeV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BtcActiveSwapEnvelopeV1")
            .field("agreement_wire", &"[REDACTED]")
            .field("local_participant", &self.local_participant)
            .field("revision", &self.revision)
            .field("lock_effects", &self.lock_effects)
            .field("prepared_protocol", &self.prepared_protocol.is_some())
            .field("transitions", &self.transitions)
            .finish()
    }
}

impl BtcActiveSwapEnvelopeV1 {
    /// Reconstructs an untrusted store record. [`BtcPairSdk::resume`] fully
    /// revalidates its agreement, role, revision, and exact effects.
    pub fn from_parts(
        agreement_wire: impl Into<Box<[u8]>>,
        local_participant: Participant,
        revision: u64,
        lock_effects: BtcPreparedLockEffectsV1,
    ) -> Self {
        Self {
            agreement_wire: agreement_wire.into(),
            local_participant,
            revision,
            lock_effects,
            prepared_protocol: None,
            transitions: Vec::new(),
        }
    }

    /// Reconstructs an untrusted complete lifecycle store record.
    ///
    /// The revision must equal the transition count. [`BtcPairSdk::resume`]
    /// revalidates every agreement-bound prepared effect and transition.
    pub fn from_lifecycle_parts(
        agreement_wire: impl Into<Box<[u8]>>,
        local_participant: Participant,
        revision: u64,
        lock_effects: BtcPreparedLockEffectsV1,
        prepared_protocol: BtcPreparedProtocolV1,
        transitions: Vec<BtcLifecycleTransitionV1>,
    ) -> Self {
        Self {
            agreement_wire: agreement_wire.into(),
            local_participant,
            revision,
            lock_effects,
            prepared_protocol: Some(prepared_protocol),
            transitions,
        }
    }

    /// Untrusted durable transition entries, in revision order.
    pub fn transitions(&self) -> &[BtcLifecycleTransitionV1] {
        &self.transitions
    }

    /// Stored role-local revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }
}

/// Deterministic action vocabulary for the revision-zero facade.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcLifecycleActionV1 {
    /// The local taker may durably stage and submit the Bitcoin first lock.
    PublishBitcoinFirstLock,
    /// The local taker may durably stage LEZ initialization then funding.
    PublishLezFirstLock,
    /// A maker waits for independently validated canonical first-lock evidence.
    AwaitTakerFirstLock,
    /// Publish the maker-funded Bitcoin second lock.
    PublishBitcoinSecondLock,
    /// Publish the maker-funded LEZ second lock.
    PublishLezSecondLock,
    /// Wait for the maker-funded second lock.
    AwaitMakerSecondLock,
    /// Publish the Bitcoin revealing claim.
    PublishBitcoinRevealingClaim,
    /// Publish the LEZ revealing claim.
    PublishLezRevealingClaim,
    /// Wait for the agreement-selected revealing claim.
    AwaitRevealingClaim,
    /// Publish the material-consuming Bitcoin follow-up claim.
    PublishBitcoinFollowupClaim,
    /// Publish the material-consuming LEZ follow-up claim.
    PublishLezFollowupClaim,
    /// Wait for the material-consuming follow-up claim.
    AwaitFollowupClaim,
    /// Wait for the counterparty-owned ordered refund.
    AwaitCounterpartyRefund,
    /// Obtain a fresh canonical view and call [`ActiveBtcSwap::recovery_action`].
    EvaluateRecovery,
    /// The durable lifecycle is terminal.
    Complete,
}

/// Offline role-local status returned without node or peer access.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcSwapStatusV1 {
    swap_id: SwapId,
    local_participant: Participant,
    direction: SwapDirection,
    phase: Phase,
    revision: u64,
    next_action: BtcLifecycleActionV1,
}

impl BtcSwapStatusV1 {
    /// Agreement-derived stable swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Fixed local role.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Signed swap direction.
    #[must_use]
    pub const fn direction(&self) -> SwapDirection {
        self.direction
    }

    /// Deterministic aggregate phase.
    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }

    /// Durable role-local revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Structural local action at this revision; timeout projection still dominates.
    pub const fn next_action(&self) -> BtcLifecycleActionV1 {
        self.next_action
    }
}

/// Role-fixed public facade with no hidden chain, store, or transport handles.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcPairSdk {
    local_participant: Participant,
    bitcoin_policy: BtcChainPolicyV1,
}

impl BtcPairSdk {
    /// Fixes the local role and exact Bitcoin network/confirmation policy.
    pub const fn new(local_participant: Participant, bitcoin_policy: BtcChainPolicyV1) -> Self {
        Self {
            local_participant,
            bitcoin_policy,
        }
    }

    /// Fixed local participant.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Validates bounded countersigned wire under the fixed local policy.
    ///
    /// # Errors
    ///
    /// Preserves the concrete agreement validation error.
    pub fn accept_wire(&self, wire: &[u8]) -> Result<AcceptedBtcAgreementV1, BtcSdkError> {
        let agreement = BtcAgreementV1::from_wire_for_bitcoin_policy(wire, &self.bitcoin_policy)?;
        Ok(AcceptedBtcAgreementV1 {
            agreement,
            wire: wire.into(),
            local_participant: self.local_participant,
        })
    }

    /// Creates the deterministic claim-only prepared value used by the claim
    /// methods on [`SwapProtocol`].
    ///
    /// This deliberately does not implement [`SwapProtocol::prepare`]: the
    /// common method promises complete pre-lock refund recoverability, while
    /// this first lifecycle slice has only complete claim preparation.
    ///
    /// # Errors
    ///
    /// Rejects terms without both verified adaptor presignatures and the exact
    /// LEZ claim substitution template, or terms validated under another local
    /// Bitcoin policy.
    pub fn prepare_claims(
        &self,
        terms: ValidatedBtcProtocolTermsV1,
    ) -> Result<BtcPreparedProtocolV1, BtcSdkError> {
        terms
            .agreement
            .ensure_bitcoin_policy(&self.bitcoin_policy)?;
        if terms.claim_effects.is_none() {
            return Err(BtcSdkError::MissingClaimEffects);
        }
        Ok(BtcPreparedProtocolV1 { terms })
    }

    /// Validates exact lock effects and returns a role-fixed active value.
    ///
    /// # Errors
    ///
    /// Rejects role substitution or any Bitcoin transaction/output drift from
    /// the accepted agreement.
    pub fn activate(
        &self,
        accepted: AcceptedBtcAgreementV1,
        lock_effects: BtcPreparedLockEffectsV1,
    ) -> Result<ActiveBtcSwap, BtcSdkError> {
        if accepted.local_participant != self.local_participant {
            return Err(BtcSdkError::LocalRoleMismatch {
                expected: self.local_participant,
                actual: accepted.local_participant,
            });
        }
        lock_effects.validate(&accepted.agreement)?;
        let coordinator = accepted.agreement.coordinator().clone();
        Ok(ActiveBtcSwap {
            accepted,
            lock_effects,
            coordinator,
            revision: 0,
            prepared_protocol: None,
            transitions: Vec::new(),
        })
    }

    /// Activates complete agreement-bound claim and signed-refund material.
    ///
    /// This is the revisioned lifecycle path. The legacy [`Self::activate`]
    /// remains available for revision-zero lock-only consumers.
    ///
    /// # Errors
    ///
    /// Rejects role, policy, agreement, lock, claim, or refund substitution.
    pub fn activate_prepared(
        &self,
        accepted: AcceptedBtcAgreementV1,
        prepared: BtcPreparedProtocolV1,
    ) -> Result<ActiveBtcSwap, BtcSdkError> {
        if accepted.local_participant != self.local_participant {
            return Err(BtcSdkError::LocalRoleMismatch {
                expected: self.local_participant,
                actual: accepted.local_participant,
            });
        }
        if prepared.agreement() != accepted.agreement() {
            return Err(BtcSdkError::LifecycleAgreementMismatch);
        }
        prepared
            .agreement()
            .ensure_bitcoin_policy(&self.bitcoin_policy)?;
        prepared.terms.lock_effects.validate(accepted.agreement())?;
        prepared.required_recovery_effects()?;
        let lock_effects = prepared.terms.lock_effects.clone();
        let coordinator = accepted.agreement.coordinator().clone();
        Ok(ActiveBtcSwap {
            accepted,
            lock_effects,
            coordinator,
            revision: 0,
            prepared_protocol: Some(prepared),
            transitions: Vec::new(),
        })
    }

    /// Revalidates an offline durable envelope without discovery, negotiation,
    /// node, or public-effect submission I/O.
    ///
    /// # Errors
    ///
    /// Rejects malformed agreement bytes, role/material substitution, revision
    /// drift, non-canonical transition order, and exact-effect drift.
    pub fn resume(&self, envelope: BtcActiveSwapEnvelopeV1) -> Result<ActiveBtcSwap, BtcSdkError> {
        if envelope.revision > 4 {
            return Err(BtcSdkError::UnsupportedResumeRevision(envelope.revision));
        }
        if envelope.revision != envelope.transitions.len() as u64 {
            return Err(BtcSdkError::LifecycleRevisionMismatch {
                revision: envelope.revision,
                transitions: envelope.transitions.len() as u64,
            });
        }
        if envelope.local_participant != self.local_participant {
            return Err(BtcSdkError::LocalRoleMismatch {
                expected: self.local_participant,
                actual: envelope.local_participant,
            });
        }
        let accepted = self.accept_wire(&envelope.agreement_wire)?;
        let mut active = match envelope.prepared_protocol {
            Some(prepared) => self.activate_prepared(accepted, prepared)?,
            None if envelope.revision == 0 => {
                self.activate(accepted, envelope.lock_effects.clone())?
            }
            None => return Err(BtcSdkError::MissingLifecyclePreparation),
        };
        if active.lock_effects != envelope.lock_effects {
            return Err(BtcSdkError::LifecycleEffectMismatch);
        }
        for transition in envelope.transitions {
            let _ = active.apply_transition(transition)?;
        }
        Ok(active)
    }
}

/// Post-activation deterministic lifecycle facade.
///
/// It contains no discovery or negotiation capability. An application persists
/// [`Self::durable_envelope`] before restart and replays through
/// [`BtcPairSdk::resume`].
///
/// ```
/// use lez_btc_swap_sdk::{
///     ActiveBtcSwap, BtcLifecycleTransitionV1, BtcPairSdk, BtcSdkError,
/// };
///
/// # fn persist_and_resume(
/// #     sdk: &BtcPairSdk,
/// #     mut active: ActiveBtcSwap,
/// #     observed: [BtcLifecycleTransitionV1; 4],
/// # ) -> Result<ActiveBtcSwap, BtcSdkError> {
/// for transition in observed {
///     let _outcome = active.apply_transition(transition)?;
///     active = sdk.resume(active.durable_envelope())?;
/// }
/// Ok(active)
/// # }
/// ```
///
/// Pre-lock transports cannot accidentally become post-lock dependencies:
///
/// ```compile_fail
/// use lez_btc_swap_sdk::ActiveBtcSwap;
///
/// fn cannot_renegotiate(active: &ActiveBtcSwap) {
///     let _ = active.negotiate(&(), ());
/// }
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct ActiveBtcSwap {
    accepted: AcceptedBtcAgreementV1,
    lock_effects: BtcPreparedLockEffectsV1,
    coordinator: SwapCoordinator,
    revision: u64,
    prepared_protocol: Option<BtcPreparedProtocolV1>,
    transitions: Vec<BtcLifecycleTransitionV1>,
}

impl ActiveBtcSwap {
    /// Immutable validated agreement.
    #[must_use]
    pub const fn agreement(&self) -> &BtcAgreementV1 {
        self.accepted.agreement()
    }

    /// Fixed local role.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.accepted.local_participant()
    }

    /// Offline status reconstructed only from durable state.
    pub fn status(&self) -> BtcSwapStatusV1 {
        BtcSwapStatusV1 {
            swap_id: self.coordinator.id().clone(),
            local_participant: self.local_participant(),
            direction: self.agreement().direction(),
            phase: self.coordinator.phase(),
            revision: self.revision,
            next_action: self.next_action(),
        }
    }

    /// Exact taker-funded first-lock plan. Returning it grants no submission authority.
    pub fn first_lock_plan(&self) -> &ExactPublicEffectPlanV1 {
        self.lock_effects
            .plan_for_participant(self.agreement(), Participant::Taker)
    }

    /// Validates adapter-produced first-lock evidence against agreement and plan.
    ///
    /// # Errors
    ///
    /// Rejects wrong chain/network/value/identity/finality or plan commitment.
    pub fn validate_first_lock(
        &self,
        evidence: &BtcFirstLockEvidenceV1,
    ) -> Result<ConfirmedBtcFirstLockV1, BtcSdkError> {
        validate_first_lock(self.agreement(), &self.lock_effects, evidence)
    }

    /// Returns the maker-funded second-lock plan only for evidence validated by
    /// this exact agreement and first-lock plan.
    ///
    /// # Errors
    ///
    /// Rejects a confirmation token from another agreement, direction, or plan.
    pub fn second_lock_plan(
        &self,
        first: &ConfirmedBtcFirstLockV1,
    ) -> Result<&ExactPublicEffectPlanV1, BtcSdkError> {
        validate_confirmation_binding(self.agreement(), self.first_lock_plan(), first)?;
        Ok(self
            .lock_effects
            .plan_for_participant(self.agreement(), Participant::Maker))
    }

    /// Explicit direction-specific revealing/follow-up claim order.
    pub const fn claim_order(&self) -> ClaimOrder {
        claim_order(self.accepted.agreement.direction())
    }

    /// Role-local structural action reconstructed from the durable aggregate head.
    ///
    /// This offline projection has no chain clock. Before submitting a lock or claim,
    /// refresh canonical state; an eligible [`Self::recovery_action`] dominates
    /// this phase-only action.
    pub fn next_action(&self) -> BtcLifecycleActionV1 {
        let local = self.local_participant();
        match self.coordinator.phase() {
            Phase::Offered if local == Participant::Taker => {
                publish_lock_action(self.coordinator.funded_chain(Participant::Taker), true)
            }
            Phase::Offered | Phase::AwaitingTakerConfirmations => {
                BtcLifecycleActionV1::AwaitTakerFirstLock
            }
            Phase::TakerLockConfirmed if local == Participant::Maker => {
                publish_lock_action(self.coordinator.funded_chain(Participant::Maker), false)
            }
            Phase::TakerLockConfirmed | Phase::AwaitingMakerConfirmations => {
                BtcLifecycleActionV1::AwaitMakerSecondLock
            }
            Phase::BothLegsLocked if local == self.coordinator.first_claimant() => {
                publish_revealing_action(self.claim_order().revealing())
            }
            Phase::BothLegsLocked => BtcLifecycleActionV1::AwaitRevealingClaim,
            Phase::ClaimEvidenceAvailable if local == self.coordinator.first_claimant().other() => {
                publish_followup_action(self.claim_order().followup())
            }
            Phase::ClaimEvidenceAvailable => BtcLifecycleActionV1::AwaitFollowupClaim,
            Phase::MakerLegRefunded if local == Participant::Taker => {
                BtcLifecycleActionV1::EvaluateRecovery
            }
            Phase::Completed | Phase::Refunded => BtcLifecycleActionV1::Complete,
            Phase::MakerLegRefunded
            | Phase::TakerLegRefunded
            | Phase::TakerLockReorged
            | Phase::MakerLockReorged
            | Phase::MakerRecoveryAvailable => BtcLifecycleActionV1::AwaitCounterpartyRefund,
        }
    }

    /// Applies one exact canonical transition without performing I/O.
    ///
    /// Exact replay is idempotent and never increments the revision or returns
    /// the same public effect twice. Replaying any accepted historical effect,
    /// including a non-head transition, returns its original revision while the
    /// aggregate head remains unchanged.
    /// New transitions are applied to a cloned candidate and replace the public
    /// state only after every validation and coordinator update succeeds.
    ///
    /// # Errors
    ///
    /// Rejects missing complete preparation, transition reordering, agreement,
    /// role, network, finality, confirmation, identity, or byte substitution.
    pub fn apply_transition(
        &mut self,
        transition: BtcLifecycleTransitionV1,
    ) -> Result<BtcLifecycleTransitionOutcomeV1, BtcSdkError> {
        if let Some(index) = self
            .transitions
            .iter()
            .position(|existing| lifecycle_effect_eq(existing, &transition))
        {
            return Ok(BtcLifecycleTransitionOutcomeV1::AlreadyApplied {
                revision: index as u64 + 1,
            });
        }
        if self.revision >= 4 {
            return Err(BtcSdkError::UnsupportedResumeRevision(self.revision + 1));
        }
        let mut candidate = self.clone();
        candidate.apply_new_transition(&transition)?;
        candidate.transitions.push(transition);
        candidate.revision += 1;
        let revision = candidate.revision;
        *self = candidate;
        Ok(BtcLifecycleTransitionOutcomeV1::Applied { revision })
    }

    fn apply_new_transition(
        &mut self,
        transition: &BtcLifecycleTransitionV1,
    ) -> Result<(), BtcSdkError> {
        match transition {
            BtcLifecycleTransitionV1::FirstLockConfirmed(evidence) => {
                require_phase(self.coordinator.phase(), Phase::Offered)?;
                validate_lock_for_participant(
                    self.agreement(),
                    &self.lock_effects,
                    Participant::Taker,
                    evidence,
                )?;
                let proof = funding_proof(
                    evidence,
                    self.coordinator.required_confirmations(Participant::Taker),
                )?;
                self.coordinator
                    .observe_funding(Participant::Taker, proof)
                    .map_err(BtcSdkError::InvalidCoordinatorTransition)
            }
            BtcLifecycleTransitionV1::SecondLockConfirmed(evidence) => {
                require_phase(self.coordinator.phase(), Phase::TakerLockConfirmed)?;
                validate_lock_for_participant(
                    self.agreement(),
                    &self.lock_effects,
                    Participant::Maker,
                    evidence,
                )?;
                let proof = funding_proof(
                    evidence,
                    self.coordinator.required_confirmations(Participant::Maker),
                )?;
                self.coordinator
                    .observe_funding(Participant::Maker, proof)
                    .map_err(BtcSdkError::InvalidCoordinatorTransition)
            }
            BtcLifecycleTransitionV1::RevealingClaimConfirmed(evidence) => {
                require_phase(self.coordinator.phase(), Phase::BothLegsLocked)?;
                let prepared = self.prepared()?;
                let material = validate_revealing_claim_for_lifecycle(prepared, evidence)?;
                let proof = revealing_claim_proof(evidence)?;
                self.coordinator
                    .observe_revealing_claim(
                        self.coordinator.first_claimant(),
                        proof,
                        ClaimEvidence::new(*material.adaptor_secret),
                    )
                    .map_err(BtcSdkError::InvalidCoordinatorTransition)
            }
            BtcLifecycleTransitionV1::FollowupClaimConfirmed(evidence) => {
                require_phase(self.coordinator.phase(), Phase::ClaimEvidenceAvailable)?;
                let prepared = self.prepared()?;
                let revealing = self
                    .transitions
                    .iter()
                    .find_map(|transition| match transition {
                        BtcLifecycleTransitionV1::RevealingClaimConfirmed(evidence) => {
                            Some(evidence)
                        }
                        _ => None,
                    })
                    .ok_or(BtcSdkError::LifecycleTransitionConflict)?;
                let material = validate_revealing_claim_for_lifecycle(prepared, revealing)?;
                let plan = build_followup_claim_for_lifecycle(prepared, &material)?;
                validate_followup_claim(prepared.agreement(), &plan, evidence)?;
                self.coordinator
                    .observe_followup_claim(
                        self.coordinator.first_claimant().other(),
                        followup_claim_proof(evidence)?,
                    )
                    .map_err(BtcSdkError::InvalidCoordinatorTransition)
            }
            BtcLifecycleTransitionV1::RecoveryObserved(state) => {
                let prepared = self.prepared()?.clone();
                apply_recovery_transition(&mut self.coordinator, &prepared, state)
            }
        }
    }

    /// Reconstructs the exact follow-up claim for its owning local role.
    ///
    /// # Errors
    ///
    /// Returns validation errors if durable revealing evidence was substituted.
    pub fn followup_claim_plan(&self) -> Result<Option<ExactPublicEffectPlanV1>, BtcSdkError> {
        if self.coordinator.phase() != Phase::ClaimEvidenceAvailable
            || self.local_participant() != self.coordinator.first_claimant().other()
        {
            return Ok(None);
        }
        let prepared = self.prepared()?;
        let revealing = self
            .transitions
            .iter()
            .find_map(|transition| match transition {
                BtcLifecycleTransitionV1::RevealingClaimConfirmed(evidence) => Some(evidence),
                _ => None,
            })
            .ok_or(BtcSdkError::LifecycleTransitionConflict)?;
        let material = validate_revealing_claim_for_lifecycle(prepared, revealing)?;
        Ok(Some(build_followup_claim_for_lifecycle(
            prepared, &material,
        )?))
    }

    /// Projects a role-local exact timeout action from a stable canonical view.
    ///
    /// # Errors
    ///
    /// Applies the same complete recovery-state validation as [`SwapProtocol`].
    pub fn recovery_action(
        &self,
        state: &BtcCanonicalRecoveryStateV1,
    ) -> Result<BtcRecoveryActionV1, BtcSdkError> {
        let sdk = BtcPairSdk::new(
            self.local_participant(),
            *self.agreement().bitcoin_chain_policy(),
        );
        recovery_action(&sdk, self.prepared()?, state)
    }

    fn prepared(&self) -> Result<&BtcPreparedProtocolV1, BtcSdkError> {
        self.prepared_protocol
            .as_ref()
            .ok_or(BtcSdkError::MissingLifecyclePreparation)
    }

    /// Creates an exact offline restart envelope for application-owned durable storage.
    pub fn durable_envelope(&self) -> BtcActiveSwapEnvelopeV1 {
        BtcActiveSwapEnvelopeV1 {
            agreement_wire: self.accepted.wire.clone(),
            local_participant: self.local_participant(),
            revision: self.revision,
            lock_effects: self.lock_effects.clone(),
            prepared_protocol: self.prepared_protocol.clone(),
            transitions: self.transitions.clone(),
        }
    }
}

/// Adapter-produced Bitcoin first-lock facts.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BitcoinFirstLockEvidenceV1 {
    genesis_block_hash: [u8; 32],
    exact_transaction: ExactPublicEffectBytes,
    confirmations: u32,
}

impl BitcoinFirstLockEvidenceV1 {
    /// Captures canonical Bitcoin adapter facts for deterministic validation.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, malformed, non-canonical, coinbase, or
    /// structurally unsigned transaction bytes.
    pub fn new(
        genesis_block_hash: [u8; 32],
        exact_transaction: impl Into<Box<[u8]>>,
        confirmations: u32,
    ) -> Result<Self, BtcSdkError> {
        let exact_transaction = ExactPublicEffectBytes::new(exact_transaction)?;
        let transaction = parse_signed_bitcoin_funding(&exact_transaction)?;
        if serialize(&transaction) != exact_transaction.as_slice() {
            return Err(BtcSdkError::MalformedBitcoinFunding(
                bitcoin::consensus::encode::Error::ParseFailed(
                    "non-canonical Bitcoin transaction bytes",
                ),
            ));
        }
        Ok(Self {
            genesis_block_hash,
            exact_transaction,
            confirmations,
        })
    }

    /// Complete canonical consensus transaction bytes observed by the adapter.
    pub const fn exact_transaction(&self) -> &ExactPublicEffectBytes {
        &self.exact_transaction
    }

    /// Bitcoin genesis hash observed by the adapter.
    #[must_use]
    pub const fn genesis_block_hash(&self) -> &[u8; 32] {
        &self.genesis_block_hash
    }

    /// Canonical confirmation count observed by the adapter.
    #[must_use]
    pub const fn confirmations(&self) -> u32 {
        self.confirmations
    }
}

/// Adapter-produced finalized LEZ first-lock facts.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct LezFirstLockEvidenceV1 {
    genesis_block_hash: [u8; 32],
    initialization_public_id: ExpectedPublicEffectId,
    exact_initialization: ExactPublicEffectBytes,
    funding_public_id: ExpectedPublicEffectId,
    exact_funding: ExactPublicEffectBytes,
    metadata_account: [u8; 32],
    custody_account: [u8; 32],
    amount: u128,
    finalized: bool,
}

impl LezFirstLockEvidenceV1 {
    /// Captures finalized LEZ adapter facts for deterministic validation.
    ///
    /// # Errors
    ///
    /// Rejects invalid public identities.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        genesis_block_hash: [u8; 32],
        initialization_public_id: impl Into<Box<str>>,
        exact_initialization: impl Into<Box<[u8]>>,
        funding_public_id: impl Into<Box<str>>,
        exact_funding: impl Into<Box<[u8]>>,
        metadata_account: [u8; 32],
        custody_account: [u8; 32],
        amount: u128,
        finalized: bool,
    ) -> Result<Self, BtcSdkError> {
        Ok(Self {
            genesis_block_hash,
            initialization_public_id: ExpectedPublicEffectId::new(initialization_public_id)?,
            exact_initialization: ExactPublicEffectBytes::new(exact_initialization)?,
            funding_public_id: ExpectedPublicEffectId::new(funding_public_id)?,
            exact_funding: ExactPublicEffectBytes::new(exact_funding)?,
            metadata_account,
            custody_account,
            amount,
            finalized,
        })
    }

    /// Complete initialization bytes observed by the LEZ adapter.
    pub const fn exact_initialization(&self) -> &ExactPublicEffectBytes {
        &self.exact_initialization
    }

    /// Complete funding bytes observed by the LEZ adapter.
    pub const fn exact_funding(&self) -> &ExactPublicEffectBytes {
        &self.exact_funding
    }
}

/// Direction-specific first-lock evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcFirstLockEvidenceV1 {
    /// Bitcoin P2TR funding output and confirmation facts.
    Bitcoin(BitcoinFirstLockEvidenceV1),
    /// Finalized LEZ initialize/fund facts.
    Lez(LezFirstLockEvidenceV1),
}

/// Result of deterministic first-lock validation. It cannot be directly constructed.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct ConfirmedBtcFirstLockV1 {
    agreement_commitment: [u8; 32],
    plan_commitment: [u8; 32],
    direction: SwapDirection,
}

impl ConfirmedBtcFirstLockV1 {
    /// Exact accepted agreement commitment.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    /// Exact validated first-lock plan commitment.
    #[must_use]
    pub const fn plan_commitment(&self) -> &[u8; 32] {
        &self.plan_commitment
    }
}

/// Adapter-produced canonical Bitcoin revealing-claim facts.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BitcoinRevealingClaimEvidenceV1 {
    claimant: Participant,
    genesis_block_hash: [u8; 32],
    exact_transaction: ExactPublicEffectBytes,
    signature: [u8; SCHNORR_SIGNATURE_BYTES],
    confirmations: u32,
}

impl BitcoinRevealingClaimEvidenceV1 {
    /// Captures a canonical one-input, one-item key-path claim observation.
    ///
    /// # Errors
    ///
    /// Rejects empty/oversized, malformed, non-canonical, or structurally
    /// different Bitcoin claim bytes.
    pub fn new(
        claimant: Participant,
        genesis_block_hash: [u8; 32],
        exact_transaction: impl Into<Box<[u8]>>,
        confirmations: u32,
    ) -> Result<Self, BtcSdkError> {
        let exact_transaction = ExactPublicEffectBytes::new(exact_transaction)?;
        let transaction = parse_bitcoin_revealing_claim(&exact_transaction)?;
        if serialize(&transaction) != exact_transaction.as_slice() {
            return Err(BtcSdkError::MalformedBitcoinClaim(
                bitcoin::consensus::encode::Error::ParseFailed(
                    "non-canonical Bitcoin claim transaction bytes",
                ),
            ));
        }
        let signature = bitcoin_claim_signature(&transaction)?;
        Ok(Self {
            claimant,
            genesis_block_hash,
            exact_transaction,
            signature,
            confirmations,
        })
    }

    /// Exact canonical consensus transaction bytes observed by the adapter.
    pub const fn exact_transaction(&self) -> &ExactPublicEffectBytes {
        &self.exact_transaction
    }
}

/// Adapter-produced finalized LEZ revealing-claim facts.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct LezRevealingClaimEvidenceV1 {
    claimant: Participant,
    genesis_block_hash: [u8; 32],
    public_id: ExpectedPublicEffectId,
    exact_claim: ExactPublicEffectBytes,
    signature: [u8; SCHNORR_SIGNATURE_BYTES],
    finalized: bool,
}

impl LezRevealingClaimEvidenceV1 {
    /// Captures finalized LEZ adapter facts and the exact witnessed signature.
    ///
    /// # Errors
    ///
    /// Rejects an invalid public identity or empty/oversized exact bytes.
    pub fn new(
        claimant: Participant,
        genesis_block_hash: [u8; 32],
        public_id: impl Into<Box<str>>,
        exact_claim: impl Into<Box<[u8]>>,
        signature: [u8; SCHNORR_SIGNATURE_BYTES],
        finalized: bool,
    ) -> Result<Self, BtcSdkError> {
        Ok(Self {
            claimant,
            genesis_block_hash,
            public_id: ExpectedPublicEffectId::new(public_id)?,
            exact_claim: ExactPublicEffectBytes::new(exact_claim)?,
            signature,
            finalized,
        })
    }

    /// Complete signed public envelope observed by the LEZ adapter.
    pub const fn exact_claim(&self) -> &ExactPublicEffectBytes {
        &self.exact_claim
    }
}

/// Direction-specific canonical revealing-claim evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcRevealingClaimEvidenceV1 {
    /// Canonical confirmed Bitcoin P2TR key-path claim.
    Bitcoin(BitcoinRevealingClaimEvidenceV1),
    /// Canonical finalized LEZ witnessed claim.
    Lez(LezRevealingClaimEvidenceV1),
}

/// Agreement-bound adaptor scalar extracted from canonical revealing evidence.
///
/// Debug output is redacted and no accessor exposes the private scalar.
/// [`SwapProtocol::build_followup_claim`] can borrow it to deterministically
/// reconstruct the exact replay-safe public effect.
#[must_use]
pub struct BtcRecoveredClaimMaterialV1 {
    agreement_commitment: [u8; 32],
    direction: SwapDirection,
    revealing_claimant: Participant,
    followup_claimant: Participant,
    adaptor_secret: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for BtcRecoveredClaimMaterialV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BtcRecoveredClaimMaterialV1")
            .field("agreement_commitment", &self.agreement_commitment)
            .field("direction", &self.direction)
            .field("revealing_claimant", &self.revealing_claimant)
            .field("followup_claimant", &self.followup_claimant)
            .field("adaptor_secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CanonicalRecoveryStatusV1 {
    Absent,
    Locked,
    Refunded,
}

/// Canonical Bitcoin adapter snapshot used only for deterministic recovery selection.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BitcoinCanonicalRecoveryStateV1 {
    status: CanonicalRecoveryStatusV1,
    genesis_block_hash: Option<[u8; 32]>,
    funding_transaction_id: Option<[u8; 32]>,
    refund_transaction_id: Option<[u8; 32]>,
    confirmations: u32,
    funding_output_unspent: bool,
}

impl BitcoinCanonicalRecoveryStateV1 {
    /// The agreement funding output is canonically absent.
    pub const fn absent() -> Self {
        Self {
            status: CanonicalRecoveryStatusV1::Absent,
            genesis_block_hash: None,
            funding_transaction_id: None,
            refund_transaction_id: None,
            confirmations: 0,
            funding_output_unspent: false,
        }
    }

    /// The exact agreement funding output is canonical and currently unspent.
    pub const fn locked(
        genesis_block_hash: [u8; 32],
        funding_transaction_id: [u8; 32],
        confirmations: u32,
        funding_output_unspent: bool,
    ) -> Self {
        Self {
            status: CanonicalRecoveryStatusV1::Locked,
            genesis_block_hash: Some(genesis_block_hash),
            funding_transaction_id: Some(funding_transaction_id),
            refund_transaction_id: None,
            confirmations,
            funding_output_unspent,
        }
    }

    /// The exact prepared Bitcoin refund is canonical.
    pub const fn refunded(
        genesis_block_hash: [u8; 32],
        funding_transaction_id: [u8; 32],
        refund_transaction_id: [u8; 32],
        confirmations: u32,
    ) -> Self {
        Self {
            status: CanonicalRecoveryStatusV1::Refunded,
            genesis_block_hash: Some(genesis_block_hash),
            funding_transaction_id: Some(funding_transaction_id),
            refund_transaction_id: Some(refund_transaction_id),
            confirmations,
            funding_output_unspent: false,
        }
    }
}

/// Canonical finalized LEZ adapter snapshot used for recovery selection.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct LezCanonicalRecoveryStateV1 {
    status: CanonicalRecoveryStatusV1,
    genesis_block_hash: Option<[u8; 32]>,
    initialization_public_id: Option<ExpectedPublicEffectId>,
    funding_public_id: Option<ExpectedPublicEffectId>,
    refund_public_id: Option<ExpectedPublicEffectId>,
    finalized: bool,
    custody_unspent: bool,
}

impl LezCanonicalRecoveryStateV1 {
    /// The agreement escrow is finalized absent.
    pub const fn absent() -> Self {
        Self {
            status: CanonicalRecoveryStatusV1::Absent,
            genesis_block_hash: None,
            initialization_public_id: None,
            funding_public_id: None,
            refund_public_id: None,
            finalized: true,
            custody_unspent: false,
        }
    }

    /// The exact initialized/funded agreement escrow is observed.
    ///
    /// # Errors
    ///
    /// Rejects malformed public effect identities.
    pub fn locked(
        genesis_block_hash: [u8; 32],
        initialization_public_id: impl Into<Box<str>>,
        funding_public_id: impl Into<Box<str>>,
        finalized: bool,
        custody_unspent: bool,
    ) -> Result<Self, BtcSdkError> {
        Ok(Self {
            status: CanonicalRecoveryStatusV1::Locked,
            genesis_block_hash: Some(genesis_block_hash),
            initialization_public_id: Some(ExpectedPublicEffectId::new(initialization_public_id)?),
            funding_public_id: Some(ExpectedPublicEffectId::new(funding_public_id)?),
            refund_public_id: None,
            finalized,
            custody_unspent,
        })
    }

    /// The exact prepared LEZ refund is finalized.
    ///
    /// # Errors
    ///
    /// Rejects malformed public effect identities.
    pub fn refunded(
        genesis_block_hash: [u8; 32],
        initialization_public_id: impl Into<Box<str>>,
        funding_public_id: impl Into<Box<str>>,
        refund_public_id: impl Into<Box<str>>,
        finalized: bool,
    ) -> Result<Self, BtcSdkError> {
        Ok(Self {
            status: CanonicalRecoveryStatusV1::Refunded,
            genesis_block_hash: Some(genesis_block_hash),
            initialization_public_id: Some(ExpectedPublicEffectId::new(initialization_public_id)?),
            funding_public_id: Some(ExpectedPublicEffectId::new(funding_public_id)?),
            refund_public_id: Some(ExpectedPublicEffectId::new(refund_public_id)?),
            finalized,
            custody_unspent: false,
        })
    }
}

/// Agreement-bound stable two-chain snapshot with each native deadline clock.
///
/// This value contains no node handles and performs no I/O. Adapters must form
/// it from one stable canonical view; recovery selection validates every
/// identity, finality bit, direction, and deadline against the prepared swap.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcCanonicalRecoveryStateV1 {
    agreement_commitment: [u8; 32],
    direction: SwapDirection,
    bitcoin_best_height: u32,
    lez_unix_seconds: u64,
    bitcoin: BitcoinCanonicalRecoveryStateV1,
    lez: LezCanonicalRecoveryStateV1,
}

impl BtcCanonicalRecoveryStateV1 {
    /// Binds canonical adapter snapshots and both native clocks to one agreement.
    pub fn new(
        agreement: &BtcAgreementV1,
        bitcoin_best_height: u32,
        lez_unix_seconds: u64,
        bitcoin: BitcoinCanonicalRecoveryStateV1,
        lez: LezCanonicalRecoveryStateV1,
    ) -> Self {
        Self {
            agreement_commitment: *agreement.agreement_commitment(),
            direction: agreement.direction(),
            bitcoin_best_height,
            lez_unix_seconds,
            bitcoin,
            lez,
        }
    }
}

/// Why a pure recovery projection returns no local public effect yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcRecoveryWaitReasonV1 {
    /// Neither agreement lock exists canonically.
    NoRecoverableLock,
    /// The applicable native-chain refund deadline is not reached.
    AwaitRefundDeadline,
    /// The earlier revealing-leg refund must become canonical first.
    AwaitEarlierRefund,
    /// The eligible refund belongs to the other fixed protocol role.
    CounterpartyRefund {
        /// Role that owns the exact refund effect.
        owner: Participant,
        /// Chain on which that role recovers.
        chain: Chain,
    },
}

/// Deterministic recovery projection with no submission authority or I/O.
///
/// Bitcoin recovery is direction-dependent, unlike the fixed LEZ-first Zcash
/// profile: `TakerSellsForeign` refunds LEZ then Bitcoin, while
/// `TakerSellsLez` refunds Bitcoin then LEZ. In both cases the Maker-funded
/// revealing leg is earlier and must be canonical before the Taker-funded
/// follow-up leg is returned.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcRecoveryActionV1 {
    /// No local exact public effect is safe yet.
    Wait(BtcRecoveryWaitReasonV1),
    /// Persist/submit the exact signed Bitcoin refund through the Bitcoin port.
    SubmitBitcoinRefund(ExactPublicEffectPlanV1),
    /// Persist/submit the exact signed LEZ refund through the LEZ port.
    SubmitLezRefund(ExactPublicEffectPlanV1),
    /// Every canonically present lock has already been refunded.
    Recovered,
}

/// Structured public facade and deterministic protocol errors.
#[derive(Debug, thiserror::Error)]
pub enum BtcSdkError {
    /// Countersigned agreement validation failed.
    #[error(transparent)]
    InvalidAgreement(#[from] BtcAgreementV1Error),
    /// Exact-public-effect construction failed.
    #[error(transparent)]
    InvalidEffectPlan(#[from] PublicEffectPlanError),
    /// Bitcoin transaction bytes could not be decoded exactly.
    #[error("Bitcoin funding transaction bytes are malformed")]
    MalformedBitcoinFunding(#[source] bitcoin::consensus::encode::Error),
    /// Funding bytes do not contain a non-coinbase input with unlocking data.
    #[error("Bitcoin funding transaction is not a signed non-coinbase transaction")]
    UnsignedBitcoinFunding,
    /// Caller-supplied expected txid differs from the exact transaction bytes.
    #[error("Bitcoin funding expected identity differs from exact bytes")]
    BitcoinFundingIdentityMismatch,
    /// Exact funding txid differs from the countersigned agreement.
    #[error("Bitcoin funding transaction differs from the countersigned agreement")]
    BitcoinFundingAgreementMismatch,
    /// Agreement-selected output is absent or has the wrong value/P2TR script.
    #[error("Bitcoin funding output differs from the countersigned P2TR output")]
    BitcoinFundingOutputMismatch,
    /// LEZ initialization and funding must have independent transaction IDs.
    #[error("LEZ initialization and funding identities must be distinct")]
    DuplicateLezEffectIdentity,
    /// Claim-only preparation requires both presignatures and the LEZ template.
    #[error("BTC SDK claim preparation is incomplete")]
    MissingClaimEffects,
    /// Recovery projection requires both exact signed refund effects.
    #[error("BTC SDK recovery preparation is incomplete")]
    MissingRecoveryEffects,
    /// Prepared claim effects were created for another countersigned agreement.
    #[error("prepared claim effects belong to another countersigned agreement")]
    ClaimPreparationAgreementMismatch,
    /// Accepted/durable role differs from this facade's fixed role.
    #[error("stored local role is {actual:?}; SDK role is {expected:?}")]
    LocalRoleMismatch {
        /// Role fixed on the SDK.
        expected: Participant,
        /// Role supplied by untrusted accepted/durable state.
        actual: Participant,
    },
    /// Offer discovery failed in its application adapter.
    #[error("BTC offer discovery failed")]
    Discovery(#[source] BtcBoxPortError),
    /// Pre-lock negotiation failed in its application adapter.
    #[error("BTC pre-lock negotiation failed")]
    Negotiation(#[source] BtcBoxPortError),
    /// Revisioned replay requires complete claim and signed-refund preparation.
    #[error("BTC durable lifecycle preparation is incomplete")]
    MissingLifecyclePreparation,
    /// Prepared lifecycle material belongs to another accepted agreement.
    #[error("BTC durable lifecycle material belongs to another agreement")]
    LifecycleAgreementMismatch,
    /// Stored lock effects differ from complete prepared lifecycle effects.
    #[error("BTC durable lifecycle lock effects were substituted")]
    LifecycleEffectMismatch,
    /// Durable revision differs from the number of transition entries.
    #[error("BTC lifecycle revision {revision} has {transitions} transitions")]
    LifecycleRevisionMismatch {
        /// Stored aggregate revision.
        revision: u64,
        /// Stored transition count.
        transitions: u64,
    },
    /// A transition was supplied outside its only valid predecessor phase.
    #[error("BTC lifecycle transition requires {expected:?}; actual phase is {actual:?}")]
    LifecycleTransitionOrder {
        /// Required predecessor phase.
        expected: Phase,
        /// Reconstructed current phase.
        actual: Phase,
    },
    /// A durable transition conflicts with the reconstructed aggregate head.
    #[error("BTC lifecycle transition conflicts with durable history")]
    LifecycleTransitionConflict,
    /// Chain-independent coordinator rejected reconstructed canonical evidence.
    #[error("BTC lifecycle coordinator rejected a transition")]
    InvalidCoordinatorTransition(#[source] CoreError),
    /// Follow-up evidence names the wrong direction-derived chain.
    #[error("follow-up claim evidence uses the wrong chain")]
    WrongFollowupClaimChain,
    /// Follow-up evidence identifies another chain network.
    #[error("follow-up claim evidence identifies another network")]
    FollowupClaimNetworkMismatch,
    /// Bitcoin follow-up evidence has insufficient confirmations.
    #[error("Bitcoin follow-up claim has {actual} confirmations; requires {required}")]
    FollowupClaimConfirmationLag {
        /// Required canonical confirmations.
        required: u32,
        /// Observed confirmations.
        actual: u32,
    },
    /// LEZ follow-up evidence is not finalized.
    #[error("LEZ follow-up claim is not finalized")]
    FollowupClaimNotFinalized,
    /// Follow-up identity or exact bytes differ from the reconstructed plan.
    #[error("follow-up claim differs from the exact reconstructed effect")]
    FollowupClaimPlanMismatch,
    /// The version-one lifecycle log supports revisions zero through four.
    #[error("BTC SDK lifecycle schema cannot resume revision {0}")]
    UnsupportedResumeRevision(u64),
    /// Evidence names the wrong direction-derived first-lock chain.
    #[error("first-lock evidence uses the wrong chain for the signed direction")]
    WrongFirstLockChain,
    /// Adapter-observed exact transaction bytes differ from the prepared effects.
    #[error("first-lock observed bytes differ from the exact prepared plan")]
    FirstLockPlanMismatch,
    /// Evidence identifies a different network.
    #[error("first-lock evidence identifies a different network")]
    FirstLockNetworkMismatch,
    /// Evidence identifies the wrong public effect.
    #[error("first-lock evidence identifies a different public effect")]
    FirstLockIdentityMismatch,
    /// Evidence value, output, escrow, or account differs from accepted terms.
    #[error("first-lock evidence differs from accepted value or destination terms")]
    FirstLockTermsMismatch,
    /// Bitcoin evidence has not reached the signed confirmation policy.
    #[error("Bitcoin first lock has {actual} confirmations; requires {required}")]
    FirstLockConfirmationLag {
        /// Signed required confirmation count.
        required: u32,
        /// Current canonical confirmation count.
        actual: u32,
    },
    /// LEZ evidence is not finalized.
    #[error("LEZ first lock is not finalized")]
    FirstLockNotFinalized,
    /// Confirmation belongs to a different agreement/direction/first plan.
    #[error("confirmed first lock is not bound to this prepared swap")]
    FirstLockConfirmationMismatch,
    /// LEZ claim template does not contain a complete zeroed Schnorr slot.
    #[error("LEZ claim template has an invalid 64-byte signature slot")]
    InvalidLezClaimSignatureSlot,
    /// Bitcoin revealing-claim bytes could not be decoded exactly.
    #[error("Bitcoin revealing-claim transaction bytes are malformed")]
    MalformedBitcoinClaim(#[source] bitcoin::consensus::encode::Error),
    /// Bitcoin revealing evidence is not a canonical one-item key-path spend.
    #[error("Bitcoin revealing claim is not a canonical one-item key-path spend")]
    InvalidBitcoinClaimWitness,
    /// Exact signed Bitcoin follow-up construction failed canonical verification.
    #[error("Bitcoin follow-up claim signature or transaction is invalid")]
    InvalidBitcoinClaim(#[source] CooperativeKeyPathSpendError),
    /// The Bitcoin funder's BIP-342 refund signature is invalid.
    #[error("Bitcoin pre-lock refund signature is invalid")]
    InvalidBitcoinRefund(#[source] RefundScriptPathSpendError),
    /// Revalidated Bitcoin refund bytes differ from their prepared exact plan.
    #[error("Bitcoin signed refund differs from the prepared exact plan")]
    BitcoinRefundPlanMismatch,
    /// Signed recovery effects belong to another countersigned agreement.
    #[error("prepared recovery effects belong to another countersigned agreement")]
    RecoveryPreparationAgreementMismatch,
    /// A claim presignature, signature, or extracted scalar failed verification.
    #[error("claim adaptor transcript is invalid")]
    InvalidAdaptorClaim(#[source] AdaptorSessionError),
    /// Evidence names the wrong direction-derived revealing chain.
    #[error("revealing-claim evidence uses the wrong chain for the signed direction")]
    WrongRevealingClaimChain,
    /// Adapter evidence attributes the revealing effect to the wrong claimant.
    #[error("revealing claim belongs to {actual:?}; signed claimant is {expected:?}")]
    RevealingClaimRoleMismatch {
        /// Claimant selected by the signed agreement and chain order.
        expected: Participant,
        /// Claimant supplied by untrusted canonical evidence.
        actual: Participant,
    },
    /// The role-fixed facade does not own the material-consuming follow-up leg.
    #[error("follow-up claim belongs to {expected:?}; SDK role is {actual:?}")]
    FollowupClaimRoleMismatch {
        /// Follow-up claimant selected by the signed agreement.
        expected: Participant,
        /// Role fixed on the SDK.
        actual: Participant,
    },
    /// Revealing evidence identifies a different chain network.
    #[error("revealing-claim evidence identifies a different network")]
    RevealingClaimNetworkMismatch,
    /// Bitcoin revealing evidence has not reached the signed policy.
    #[error("Bitcoin revealing claim has {actual} confirmations; requires {required}")]
    RevealingClaimConfirmationLag {
        /// Signed required confirmation count.
        required: u32,
        /// Current canonical confirmation count.
        actual: u32,
    },
    /// LEZ revealing evidence is not finalized.
    #[error("LEZ revealing claim is not finalized")]
    RevealingClaimNotFinalized,
    /// Observed revealing bytes or public identity differ from prepared effects.
    #[error("revealing claim differs from the exact prepared claim effect")]
    RevealingClaimPlanMismatch,
    /// Recovered material belongs to another agreement, direction, or role.
    #[error("recovered claim material is not bound to this prepared swap")]
    RecoveredClaimMaterialMismatch,
    /// Canonical recovery state belongs to another agreement or direction.
    #[error("canonical recovery state is not bound to this prepared swap")]
    RecoveryStateAgreementMismatch,
    /// Canonical recovery evidence identifies another chain network.
    #[error("canonical recovery state identifies a different network")]
    RecoveryNetworkMismatch,
    /// Canonical recovery evidence identifies different prepared effects.
    #[error("canonical recovery state differs from prepared lock/refund effects")]
    RecoveryPlanMismatch,
    /// A canonical recovery observation has insufficient finality.
    #[error("{chain:?} recovery observation has {actual} confirmations; requires {required}")]
    RecoveryObservationLag {
        /// Chain whose evidence is lagging.
        chain: Chain,
        /// Required canonical confirmation units.
        required: u32,
        /// Observed confirmation units.
        actual: u32,
    },
    /// LEZ recovery evidence is not finalized.
    #[error("LEZ recovery observation is not finalized")]
    RecoveryNotFinalized,
    /// Canonical lock/spend combinations violate the protocol lifecycle.
    #[error("canonical recovery state is contradictory")]
    RecoveryStateContradiction,
    /// The later follow-up-leg refund appeared before the revealing-leg refund.
    #[error("canonical refunds violate the signed revealing-before-follow-up order")]
    RecoveryOrderViolation,
}

impl ProtocolError for BtcSdkError {
    fn category(&self) -> ErrorCategory {
        match self {
            Self::InvalidAgreement(_)
            | Self::InvalidEffectPlan(_)
            | Self::MalformedBitcoinFunding(_)
            | Self::UnsignedBitcoinFunding
            | Self::BitcoinFundingIdentityMismatch
            | Self::BitcoinFundingAgreementMismatch
            | Self::DuplicateLezEffectIdentity
            | Self::MissingClaimEffects
            | Self::MissingRecoveryEffects
            | Self::ClaimPreparationAgreementMismatch
            | Self::LocalRoleMismatch { .. }
            | Self::MissingLifecyclePreparation
            | Self::LifecycleAgreementMismatch
            | Self::LifecycleEffectMismatch
            | Self::LifecycleRevisionMismatch { .. }
            | Self::LifecycleTransitionOrder { .. }
            | Self::LifecycleTransitionConflict
            | Self::InvalidCoordinatorTransition(_)
            | Self::WrongFollowupClaimChain
            | Self::FollowupClaimPlanMismatch
            | Self::WrongFirstLockChain
            | Self::FirstLockPlanMismatch
            | Self::FirstLockIdentityMismatch
            | Self::FirstLockConfirmationMismatch
            | Self::InvalidLezClaimSignatureSlot
            | Self::MalformedBitcoinClaim(_)
            | Self::InvalidBitcoinClaimWitness
            | Self::InvalidBitcoinClaim(_)
            | Self::InvalidBitcoinRefund(_)
            | Self::BitcoinRefundPlanMismatch
            | Self::RecoveryPreparationAgreementMismatch
            | Self::InvalidAdaptorClaim(_)
            | Self::WrongRevealingClaimChain
            | Self::RevealingClaimRoleMismatch { .. }
            | Self::FollowupClaimRoleMismatch { .. }
            | Self::RevealingClaimPlanMismatch
            | Self::RecoveredClaimMaterialMismatch
            | Self::RecoveryStateAgreementMismatch
            | Self::RecoveryPlanMismatch
            | Self::RecoveryStateContradiction
            | Self::RecoveryOrderViolation => ErrorCategory::TranscriptMismatch,
            Self::BitcoinFundingOutputMismatch | Self::FirstLockTermsMismatch => {
                ErrorCategory::WrongValue
            }
            Self::UnsupportedResumeRevision(_) => ErrorCategory::UnsupportedCapability,
            Self::Discovery(_) | Self::Negotiation(_) => ErrorCategory::DependencyUnavailable,
            Self::FirstLockNetworkMismatch
            | Self::RevealingClaimNetworkMismatch
            | Self::RecoveryNetworkMismatch
            | Self::FollowupClaimNetworkMismatch => ErrorCategory::WrongNetwork,
            Self::FirstLockConfirmationLag { .. }
            | Self::RevealingClaimConfirmationLag { .. }
            | Self::RecoveryObservationLag { .. }
            | Self::FollowupClaimConfirmationLag { .. } => ErrorCategory::ObservationLag,
            Self::FirstLockNotFinalized
            | Self::RevealingClaimNotFinalized
            | Self::RecoveryNotFinalized
            | Self::FollowupClaimNotFinalized => ErrorCategory::NonCanonicalEvidence,
        }
    }
}

impl SwapProtocol for BtcPairSdk {
    type Terms = BtcProtocolTermsV1;
    type ValidatedTerms = ValidatedBtcProtocolTermsV1;
    type Prepared = BtcPreparedProtocolV1;
    type FirstLockTemplate = ExactPublicEffectPlanV1;
    type FirstLockEvidence = BtcFirstLockEvidenceV1;
    type ConfirmedFirstLock = ConfirmedBtcFirstLockV1;
    type SecondLockTemplate = ExactPublicEffectPlanV1;
    type RevealingClaimEvidence = BtcRevealingClaimEvidenceV1;
    type RecoveredClaimMaterial = BtcRecoveredClaimMaterialV1;
    type FollowupClaimTemplate = ExactPublicEffectPlanV1;
    type CanonicalChainState = BtcCanonicalRecoveryStateV1;
    type RecoveryAction = BtcRecoveryActionV1;
    type Error = BtcSdkError;

    fn validate_terms(&self, terms: &Self::Terms) -> Result<Self::ValidatedTerms, Self::Error> {
        let agreement = BtcAgreementV1::validate_for_bitcoin_policy(
            terms.agreement.clone(),
            &self.bitcoin_policy,
        )?;
        terms.lock_effects.validate(&agreement)?;
        if let Some(claim_effects) = &terms.claim_effects {
            claim_effects.validate(&agreement)?;
        }
        if let Some(recovery_effects) = &terms.recovery_effects {
            recovery_effects.validate(&agreement)?;
        }
        Ok(ValidatedBtcProtocolTermsV1 {
            agreement,
            lock_effects: terms.lock_effects.clone(),
            claim_effects: terms.claim_effects.clone(),
            recovery_effects: terms.recovery_effects.clone(),
        })
    }

    fn prepare(&self, terms: Self::ValidatedTerms) -> Result<Self::Prepared, Self::Error> {
        if terms.claim_effects.is_none() {
            return Err(BtcSdkError::MissingClaimEffects);
        }
        if terms.recovery_effects.is_none() {
            return Err(BtcSdkError::MissingRecoveryEffects);
        }
        Ok(BtcPreparedProtocolV1 { terms })
    }

    fn build_first_lock(
        &self,
        prepared: &Self::Prepared,
    ) -> Result<Self::FirstLockTemplate, Self::Error> {
        Ok(prepared
            .terms
            .lock_effects
            .plan_for_participant(prepared.agreement(), Participant::Taker)
            .clone())
    }

    fn validate_first_lock(
        &self,
        prepared: &Self::Prepared,
        evidence: &Self::FirstLockEvidence,
    ) -> Result<Self::ConfirmedFirstLock, Self::Error> {
        validate_first_lock(prepared.agreement(), &prepared.terms.lock_effects, evidence)
    }

    fn build_second_lock(
        &self,
        prepared: &Self::Prepared,
        first: &Self::ConfirmedFirstLock,
    ) -> Result<Self::SecondLockTemplate, Self::Error> {
        let first_plan = prepared
            .terms
            .lock_effects
            .plan_for_participant(prepared.agreement(), Participant::Taker);
        validate_confirmation_binding(prepared.agreement(), first_plan, first)?;
        Ok(prepared
            .terms
            .lock_effects
            .plan_for_participant(prepared.agreement(), Participant::Maker)
            .clone())
    }

    fn claim_order(&self, prepared: &Self::Prepared) -> ClaimOrder {
        claim_order(prepared.agreement().direction())
    }

    fn validate_revealing_claim(
        &self,
        prepared: &Self::Prepared,
        evidence: &Self::RevealingClaimEvidence,
    ) -> Result<Self::RecoveredClaimMaterial, Self::Error> {
        validate_revealing_claim(self, prepared, evidence)
    }

    fn build_followup_claim(
        &self,
        prepared: &Self::Prepared,
        material: &Self::RecoveredClaimMaterial,
    ) -> Result<Self::FollowupClaimTemplate, Self::Error> {
        build_followup_claim(self, prepared, material)
    }

    fn recovery_action(
        &self,
        prepared: &Self::Prepared,
        state: &Self::CanonicalChainState,
    ) -> Result<Self::RecoveryAction, Self::Error> {
        recovery_action(self, prepared, state)
    }
}

fn recovery_action(
    sdk: &BtcPairSdk,
    prepared: &BtcPreparedProtocolV1,
    state: &BtcCanonicalRecoveryStateV1,
) -> Result<BtcRecoveryActionV1, BtcSdkError> {
    validate_recovery_state(prepared, state)?;
    let agreement = prepared.agreement();
    let earlier_chain = agreement.coordinator().funded_chain(Participant::Maker);
    let later_chain = agreement.coordinator().funded_chain(Participant::Taker);
    let earlier = recovery_status(state, earlier_chain);
    let later = recovery_status(state, later_chain);

    match (earlier, later) {
        (CanonicalRecoveryStatusV1::Absent, CanonicalRecoveryStatusV1::Absent) => Ok(
            BtcRecoveryActionV1::Wait(BtcRecoveryWaitReasonV1::NoRecoverableLock),
        ),
        (
            CanonicalRecoveryStatusV1::Absent | CanonicalRecoveryStatusV1::Refunded,
            CanonicalRecoveryStatusV1::Locked,
        ) => {
            if !refund_deadline_reached(state, agreement, later_chain)? {
                return Ok(BtcRecoveryActionV1::Wait(
                    BtcRecoveryWaitReasonV1::AwaitRefundDeadline,
                ));
            }
            refund_action(sdk, prepared, later_chain)
        }
        (
            CanonicalRecoveryStatusV1::Absent | CanonicalRecoveryStatusV1::Refunded,
            CanonicalRecoveryStatusV1::Refunded,
        ) => Ok(BtcRecoveryActionV1::Recovered),
        (CanonicalRecoveryStatusV1::Locked, CanonicalRecoveryStatusV1::Locked) => {
            if !refund_deadline_reached(state, agreement, earlier_chain)? {
                return Ok(BtcRecoveryActionV1::Wait(
                    BtcRecoveryWaitReasonV1::AwaitRefundDeadline,
                ));
            }
            if sdk.local_participant == owner_for_chain(agreement, later_chain)
                && refund_deadline_reached(state, agreement, later_chain)?
            {
                return Ok(BtcRecoveryActionV1::Wait(
                    BtcRecoveryWaitReasonV1::AwaitEarlierRefund,
                ));
            }
            refund_action(sdk, prepared, earlier_chain)
        }
        (CanonicalRecoveryStatusV1::Locked, CanonicalRecoveryStatusV1::Refunded) => {
            Err(BtcSdkError::RecoveryOrderViolation)
        }
        (
            CanonicalRecoveryStatusV1::Refunded | CanonicalRecoveryStatusV1::Locked,
            CanonicalRecoveryStatusV1::Absent,
        ) => Err(BtcSdkError::RecoveryStateContradiction),
    }
}

fn refund_action(
    sdk: &BtcPairSdk,
    prepared: &BtcPreparedProtocolV1,
    chain: Chain,
) -> Result<BtcRecoveryActionV1, BtcSdkError> {
    let owner = owner_for_chain(prepared.agreement(), chain);
    if sdk.local_participant != owner {
        return Ok(BtcRecoveryActionV1::Wait(
            BtcRecoveryWaitReasonV1::CounterpartyRefund { owner, chain },
        ));
    }
    let plan = prepared
        .required_recovery_effects()?
        .plan_for_chain(chain)
        .clone();
    Ok(match chain {
        Chain::Bitcoin => BtcRecoveryActionV1::SubmitBitcoinRefund(plan),
        Chain::Lez => BtcRecoveryActionV1::SubmitLezRefund(plan),
        Chain::Monero | Chain::Zcash => unreachable!("validated BTC agreement"),
    })
}

fn validate_recovery_state(
    prepared: &BtcPreparedProtocolV1,
    state: &BtcCanonicalRecoveryStateV1,
) -> Result<(), BtcSdkError> {
    let agreement = prepared.agreement();
    if state.agreement_commitment != *agreement.agreement_commitment()
        || state.direction != agreement.direction()
    {
        return Err(BtcSdkError::RecoveryStateAgreementMismatch);
    }
    validate_bitcoin_recovery_state(prepared, &state.bitcoin)?;
    validate_lez_recovery_state(prepared, &state.lez)
}

fn validate_bitcoin_recovery_state(
    prepared: &BtcPreparedProtocolV1,
    state: &BitcoinCanonicalRecoveryStateV1,
) -> Result<(), BtcSdkError> {
    if state.status == CanonicalRecoveryStatusV1::Absent {
        return Ok(());
    }
    let agreement = prepared.agreement();
    if state.genesis_block_hash != Some(*agreement.bitcoin_genesis_hash()) {
        return Err(BtcSdkError::RecoveryNetworkMismatch);
    }
    if state.funding_transaction_id != Some(*agreement.funding_terms().transaction_id()) {
        return Err(BtcSdkError::RecoveryPlanMismatch);
    }
    let required = agreement.required_bitcoin_confirmations();
    if state.confirmations < required {
        return Err(BtcSdkError::RecoveryObservationLag {
            chain: Chain::Bitcoin,
            required,
            actual: state.confirmations,
        });
    }
    match state.status {
        CanonicalRecoveryStatusV1::Absent => unreachable!("returned above"),
        CanonicalRecoveryStatusV1::Locked => {
            if !state.funding_output_unspent || state.refund_transaction_id.is_some() {
                return Err(BtcSdkError::RecoveryStateContradiction);
            }
        }
        CanonicalRecoveryStatusV1::Refunded => {
            let expected = prepared
                .required_recovery_effects()?
                .bitcoin()
                .transaction_id()
                .to_byte_array();
            if state.funding_output_unspent || state.refund_transaction_id != Some(expected) {
                return Err(BtcSdkError::RecoveryPlanMismatch);
            }
        }
    }
    Ok(())
}

fn validate_lez_recovery_state(
    prepared: &BtcPreparedProtocolV1,
    state: &LezCanonicalRecoveryStateV1,
) -> Result<(), BtcSdkError> {
    if state.status == CanonicalRecoveryStatusV1::Absent {
        return Ok(());
    }
    let agreement = prepared.agreement();
    if state.genesis_block_hash != Some(*agreement.lez_terms().genesis_block_hash()) {
        return Err(BtcSdkError::RecoveryNetworkMismatch);
    }
    if !state.finalized {
        return Err(BtcSdkError::RecoveryNotFinalized);
    }
    let [initialization, funding] = prepared.terms.lock_effects.lez().plan().steps() else {
        return Err(BtcSdkError::RecoveryPlanMismatch);
    };
    if state.initialization_public_id.as_ref() != Some(initialization.expected_public_id())
        || state.funding_public_id.as_ref() != Some(funding.expected_public_id())
    {
        return Err(BtcSdkError::RecoveryPlanMismatch);
    }
    match state.status {
        CanonicalRecoveryStatusV1::Absent => unreachable!("returned above"),
        CanonicalRecoveryStatusV1::Locked => {
            if !state.custody_unspent || state.refund_public_id.is_some() {
                return Err(BtcSdkError::RecoveryStateContradiction);
            }
        }
        CanonicalRecoveryStatusV1::Refunded => {
            let [refund] = prepared.required_recovery_effects()?.lez().plan().steps() else {
                return Err(BtcSdkError::RecoveryPlanMismatch);
            };
            if state.custody_unspent
                || state.refund_public_id.as_ref() != Some(refund.expected_public_id())
            {
                return Err(BtcSdkError::RecoveryPlanMismatch);
            }
        }
    }
    Ok(())
}

fn recovery_status(state: &BtcCanonicalRecoveryStateV1, chain: Chain) -> CanonicalRecoveryStatusV1 {
    match chain {
        Chain::Bitcoin => state.bitcoin.status,
        Chain::Lez => state.lez.status,
        Chain::Monero | Chain::Zcash => unreachable!("validated BTC agreement"),
    }
}

fn refund_deadline_reached(
    state: &BtcCanonicalRecoveryStateV1,
    agreement: &BtcAgreementV1,
    chain: Chain,
) -> Result<bool, BtcSdkError> {
    let deadline = agreement
        .recovery_schedule()
        .deadline_for_chain(chain)
        .ok_or(BtcSdkError::RecoveryPlanMismatch)?;
    let observed = match chain {
        Chain::Bitcoin => u64::from(state.bitcoin_best_height),
        Chain::Lez => state.lez_unix_seconds,
        Chain::Monero | Chain::Zcash => unreachable!("validated BTC agreement"),
    };
    Ok(observed >= deadline.value())
}

fn owner_for_chain(agreement: &BtcAgreementV1, chain: Chain) -> Participant {
    if agreement.coordinator().funded_chain(Participant::Maker) == chain {
        Participant::Maker
    } else {
        debug_assert_eq!(
            agreement.coordinator().funded_chain(Participant::Taker),
            chain
        );
        Participant::Taker
    }
}

fn validate_revealing_claim(
    sdk: &BtcPairSdk,
    prepared: &BtcPreparedProtocolV1,
    evidence: &BtcRevealingClaimEvidenceV1,
) -> Result<BtcRecoveredClaimMaterialV1, BtcSdkError> {
    let agreement = prepared.agreement();
    let followup_claimant =
        claimant_for_leg(agreement, claim_order(agreement.direction()).followup());
    if sdk.local_participant != followup_claimant {
        return Err(BtcSdkError::FollowupClaimRoleMismatch {
            expected: followup_claimant,
            actual: sdk.local_participant,
        });
    }
    validate_revealing_claim_for_lifecycle(prepared, evidence)
}

fn validate_revealing_claim_for_lifecycle(
    prepared: &BtcPreparedProtocolV1,
    evidence: &BtcRevealingClaimEvidenceV1,
) -> Result<BtcRecoveredClaimMaterialV1, BtcSdkError> {
    let agreement = prepared.agreement();
    let order = claim_order(agreement.direction());
    let revealing_claimant = claimant_for_leg(agreement, order.revealing());
    let followup_claimant = claimant_for_leg(agreement, order.followup());

    let adaptor_secret = match (order.revealing(), evidence) {
        (ClaimLeg::Foreign, BtcRevealingClaimEvidenceV1::Bitcoin(observed)) => {
            validate_revealing_claimant(revealing_claimant, observed.claimant)?;
            validate_bitcoin_revealing_claim(prepared, observed)?
        }
        (ClaimLeg::Lez, BtcRevealingClaimEvidenceV1::Lez(observed)) => {
            validate_revealing_claimant(revealing_claimant, observed.claimant)?;
            validate_lez_revealing_claim(prepared, observed)?
        }
        _ => return Err(BtcSdkError::WrongRevealingClaimChain),
    };

    Ok(BtcRecoveredClaimMaterialV1 {
        agreement_commitment: *agreement.agreement_commitment(),
        direction: agreement.direction(),
        revealing_claimant,
        followup_claimant,
        adaptor_secret,
    })
}

fn validate_revealing_claimant(
    expected: Participant,
    actual: Participant,
) -> Result<(), BtcSdkError> {
    if expected == actual {
        Ok(())
    } else {
        Err(BtcSdkError::RevealingClaimRoleMismatch { expected, actual })
    }
}

fn validate_bitcoin_revealing_claim(
    prepared: &BtcPreparedProtocolV1,
    observed: &BitcoinRevealingClaimEvidenceV1,
) -> Result<Zeroizing<[u8; 32]>, BtcSdkError> {
    let agreement = prepared.agreement();
    if observed.genesis_block_hash != *agreement.bitcoin_genesis_hash() {
        return Err(BtcSdkError::RevealingClaimNetworkMismatch);
    }
    let required = agreement.required_bitcoin_confirmations();
    if observed.confirmations < required {
        return Err(BtcSdkError::RevealingClaimConfirmationLag {
            required,
            actual: observed.confirmations,
        });
    }
    let transaction = parse_bitcoin_revealing_claim(&observed.exact_transaction)?;
    let mut unsigned = transaction;
    unsigned.input[0].witness = Witness::new();
    if unsigned != *agreement.cooperative_claim().unsigned_transaction() {
        return Err(BtcSdkError::RevealingClaimPlanMismatch);
    }
    let context = agreement
        .claim_adaptor_session_context(BtcAdaptorSessionDomain::Bitcoin)
        .map_err(BtcSdkError::InvalidAdaptorClaim)?;
    verify_final_signature(&context, observed.signature)
        .map_err(BtcSdkError::InvalidAdaptorClaim)?;
    extract_adaptor_secret(
        &context,
        prepared.claim_effects().bitcoin_presignature,
        observed.signature,
    )
    .map_err(BtcSdkError::InvalidAdaptorClaim)
}

fn validate_lez_revealing_claim(
    prepared: &BtcPreparedProtocolV1,
    observed: &LezRevealingClaimEvidenceV1,
) -> Result<Zeroizing<[u8; 32]>, BtcSdkError> {
    let agreement = prepared.agreement();
    if observed.genesis_block_hash != *agreement.lez_terms().genesis_block_hash() {
        return Err(BtcSdkError::RevealingClaimNetworkMismatch);
    }
    if !observed.finalized {
        return Err(BtcSdkError::RevealingClaimNotFinalized);
    }
    let template = &prepared.claim_effects().lez_claim;
    if observed.public_id != template.expected_public_id
        || observed.exact_claim != template.materialize(observed.signature)?
    {
        return Err(BtcSdkError::RevealingClaimPlanMismatch);
    }
    let context = agreement
        .claim_adaptor_session_context(BtcAdaptorSessionDomain::Lez)
        .map_err(BtcSdkError::InvalidAdaptorClaim)?;
    verify_final_signature(&context, observed.signature)
        .map_err(BtcSdkError::InvalidAdaptorClaim)?;
    extract_adaptor_secret(
        &context,
        prepared.claim_effects().lez_presignature,
        observed.signature,
    )
    .map_err(BtcSdkError::InvalidAdaptorClaim)
}

fn build_followup_claim(
    sdk: &BtcPairSdk,
    prepared: &BtcPreparedProtocolV1,
    material: &BtcRecoveredClaimMaterialV1,
) -> Result<ExactPublicEffectPlanV1, BtcSdkError> {
    let agreement = prepared.agreement();
    let followup_claimant =
        claimant_for_leg(agreement, claim_order(agreement.direction()).followup());
    if sdk.local_participant != followup_claimant {
        return Err(BtcSdkError::FollowupClaimRoleMismatch {
            expected: followup_claimant,
            actual: sdk.local_participant,
        });
    }
    build_followup_claim_for_lifecycle(prepared, material)
}

fn build_followup_claim_for_lifecycle(
    prepared: &BtcPreparedProtocolV1,
    material: &BtcRecoveredClaimMaterialV1,
) -> Result<ExactPublicEffectPlanV1, BtcSdkError> {
    let agreement = prepared.agreement();
    let order = claim_order(agreement.direction());
    let revealing_claimant = claimant_for_leg(agreement, order.revealing());
    let followup_claimant = claimant_for_leg(agreement, order.followup());
    if material.agreement_commitment != *agreement.agreement_commitment()
        || material.direction != agreement.direction()
        || material.revealing_claimant != revealing_claimant
        || material.followup_claimant != followup_claimant
    {
        return Err(BtcSdkError::RecoveredClaimMaterialMismatch);
    }

    let step = match order.followup() {
        ClaimLeg::Foreign => {
            let context = agreement
                .claim_adaptor_session_context(BtcAdaptorSessionDomain::Bitcoin)
                .map_err(BtcSdkError::InvalidAdaptorClaim)?;
            let signature = adapt_presignature(
                &context,
                prepared.claim_effects().bitcoin_presignature,
                Zeroizing::new(*material.adaptor_secret),
            )
            .map_err(BtcSdkError::InvalidAdaptorClaim)?;
            let transaction = agreement
                .cooperative_claim()
                .clone()
                .finalize(signature)
                .map_err(BtcSdkError::InvalidBitcoinClaim)?;
            PublicEffectStepV1::new(
                PublicEffectStepId::new(BITCOIN_CLAIM_STEP)?,
                ExpectedPublicEffectId::new(transaction.compute_txid().to_string())?,
                ExactPublicEffectBytes::new(serialize(&transaction))?,
            )
        }
        ClaimLeg::Lez => {
            let context = agreement
                .claim_adaptor_session_context(BtcAdaptorSessionDomain::Lez)
                .map_err(BtcSdkError::InvalidAdaptorClaim)?;
            let signature = adapt_presignature(
                &context,
                prepared.claim_effects().lez_presignature,
                Zeroizing::new(*material.adaptor_secret),
            )
            .map_err(BtcSdkError::InvalidAdaptorClaim)?;
            let lez_claim = &prepared.claim_effects().lez_claim;
            PublicEffectStepV1::new(
                PublicEffectStepId::new(LEZ_CLAIM_STEP)?,
                lez_claim.expected_public_id.clone(),
                lez_claim.materialize(signature)?,
            )
        }
    };
    ExactPublicEffectPlanV1::new(vec![step]).map_err(Into::into)
}

const fn claimant_for_leg(agreement: &BtcAgreementV1, leg: ClaimLeg) -> Participant {
    match leg {
        ClaimLeg::Lez => agreement.lez_claimant(),
        ClaimLeg::Foreign => agreement.bitcoin_claimant(),
    }
}

fn publish_lock_action(chain: Chain, first: bool) -> BtcLifecycleActionV1 {
    match (chain, first) {
        (Chain::Bitcoin, true) => BtcLifecycleActionV1::PublishBitcoinFirstLock,
        (Chain::Lez, true) => BtcLifecycleActionV1::PublishLezFirstLock,
        (Chain::Bitcoin, false) => BtcLifecycleActionV1::PublishBitcoinSecondLock,
        (Chain::Lez, false) => BtcLifecycleActionV1::PublishLezSecondLock,
        (Chain::Monero | Chain::Zcash, _) => BtcLifecycleActionV1::AwaitCounterpartyRefund,
    }
}

const fn publish_revealing_action(leg: ClaimLeg) -> BtcLifecycleActionV1 {
    match leg {
        ClaimLeg::Foreign => BtcLifecycleActionV1::PublishBitcoinRevealingClaim,
        ClaimLeg::Lez => BtcLifecycleActionV1::PublishLezRevealingClaim,
    }
}

const fn publish_followup_action(leg: ClaimLeg) -> BtcLifecycleActionV1 {
    match leg {
        ClaimLeg::Foreign => BtcLifecycleActionV1::PublishBitcoinFollowupClaim,
        ClaimLeg::Lez => BtcLifecycleActionV1::PublishLezFollowupClaim,
    }
}

fn require_phase(actual: Phase, expected: Phase) -> Result<(), BtcSdkError> {
    if actual == expected {
        Ok(())
    } else {
        Err(BtcSdkError::LifecycleTransitionOrder { expected, actual })
    }
}

fn validate_lock_for_participant(
    agreement: &BtcAgreementV1,
    effects: &BtcPreparedLockEffectsV1,
    participant: Participant,
    evidence: &BtcFirstLockEvidenceV1,
) -> Result<(), BtcSdkError> {
    let plan = effects.plan_for_participant(agreement, participant);
    match (agreement.coordinator().funded_chain(participant), evidence) {
        (Chain::Bitcoin, BtcFirstLockEvidenceV1::Bitcoin(observed)) => {
            validate_bitcoin_first_lock(agreement, plan, observed)
        }
        (Chain::Lez, BtcFirstLockEvidenceV1::Lez(observed)) => {
            validate_lez_first_lock(agreement, plan, observed)
        }
        _ => Err(BtcSdkError::WrongFirstLockChain),
    }
}

fn funding_proof(
    evidence: &BtcFirstLockEvidenceV1,
    finalized_confirmations: u32,
) -> Result<ChainProof, BtcSdkError> {
    match evidence {
        BtcFirstLockEvidenceV1::Bitcoin(observed) => {
            let transaction = parse_signed_bitcoin_funding(&observed.exact_transaction)?;
            ChainProof::new(
                transaction.compute_txid().to_string(),
                observed.confirmations,
            )
        }
        BtcFirstLockEvidenceV1::Lez(observed) => {
            ChainProof::new(observed.funding_public_id.as_str(), finalized_confirmations)
        }
    }
    .map_err(BtcSdkError::InvalidCoordinatorTransition)
}

fn revealing_claim_proof(
    evidence: &BtcRevealingClaimEvidenceV1,
) -> Result<ChainProof, BtcSdkError> {
    match evidence {
        BtcRevealingClaimEvidenceV1::Bitcoin(observed) => {
            let transaction = parse_bitcoin_revealing_claim(&observed.exact_transaction)?;
            ChainProof::new(
                transaction.compute_txid().to_string(),
                observed.confirmations,
            )
        }
        BtcRevealingClaimEvidenceV1::Lez(observed) => {
            ChainProof::new(observed.public_id.as_str(), 1)
        }
    }
    .map_err(BtcSdkError::InvalidCoordinatorTransition)
}

fn followup_claim_proof(evidence: &BtcFollowupClaimEvidenceV1) -> Result<ChainProof, BtcSdkError> {
    match evidence {
        BtcFollowupClaimEvidenceV1::Bitcoin(observed) => {
            let transaction = parse_bitcoin_revealing_claim(&observed.exact_transaction)?;
            ChainProof::new(
                transaction.compute_txid().to_string(),
                observed.confirmations,
            )
        }
        BtcFollowupClaimEvidenceV1::Lez(observed) => {
            ChainProof::new(observed.public_id.as_str(), 1)
        }
    }
    .map_err(BtcSdkError::InvalidCoordinatorTransition)
}

fn validate_followup_claim(
    agreement: &BtcAgreementV1,
    plan: &ExactPublicEffectPlanV1,
    evidence: &BtcFollowupClaimEvidenceV1,
) -> Result<(), BtcSdkError> {
    let [step] = plan.steps() else {
        return Err(BtcSdkError::FollowupClaimPlanMismatch);
    };
    match (claim_order(agreement.direction()).followup(), evidence) {
        (ClaimLeg::Foreign, BtcFollowupClaimEvidenceV1::Bitcoin(observed)) => {
            if observed.genesis_block_hash != *agreement.bitcoin_genesis_hash() {
                return Err(BtcSdkError::FollowupClaimNetworkMismatch);
            }
            let required = agreement.required_bitcoin_confirmations();
            if observed.confirmations < required {
                return Err(BtcSdkError::FollowupClaimConfirmationLag {
                    required,
                    actual: observed.confirmations,
                });
            }
            let transaction = parse_bitcoin_revealing_claim(&observed.exact_transaction)?;
            if transaction.compute_txid().to_string() != step.expected_public_id().as_str()
                || observed.exact_transaction != *step.exact_bytes()
            {
                return Err(BtcSdkError::FollowupClaimPlanMismatch);
            }
        }
        (ClaimLeg::Lez, BtcFollowupClaimEvidenceV1::Lez(observed)) => {
            if observed.genesis_block_hash != *agreement.lez_terms().genesis_block_hash() {
                return Err(BtcSdkError::FollowupClaimNetworkMismatch);
            }
            if !observed.finalized {
                return Err(BtcSdkError::FollowupClaimNotFinalized);
            }
            if observed.public_id != *step.expected_public_id()
                || observed.exact_claim != *step.exact_bytes()
            {
                return Err(BtcSdkError::FollowupClaimPlanMismatch);
            }
        }
        _ => return Err(BtcSdkError::WrongFollowupClaimChain),
    }
    Ok(())
}

fn apply_recovery_transition(
    coordinator: &mut SwapCoordinator,
    prepared: &BtcPreparedProtocolV1,
    state: &BtcCanonicalRecoveryStateV1,
) -> Result<(), BtcSdkError> {
    validate_recovery_state(prepared, state)?;
    let maker_chain = coordinator.funded_chain(Participant::Maker);
    let taker_chain = coordinator.funded_chain(Participant::Taker);
    let maker = recovery_status(state, maker_chain);
    let taker = recovery_status(state, taker_chain);
    match (coordinator.phase(), maker, taker) {
        (
            Phase::TakerLockConfirmed,
            CanonicalRecoveryStatusV1::Absent,
            CanonicalRecoveryStatusV1::Refunded,
        )
        | (
            Phase::MakerLegRefunded,
            CanonicalRecoveryStatusV1::Refunded,
            CanonicalRecoveryStatusV1::Refunded,
        ) => coordinator
            .refund_taker_leg(recovery_position(state, taker_chain))
            .map_err(BtcSdkError::InvalidCoordinatorTransition),
        (
            Phase::BothLegsLocked,
            CanonicalRecoveryStatusV1::Refunded,
            CanonicalRecoveryStatusV1::Locked,
        ) => coordinator
            .refund_maker_leg(recovery_position(state, maker_chain))
            .map_err(BtcSdkError::InvalidCoordinatorTransition),
        _ => Err(BtcSdkError::LifecycleTransitionConflict),
    }
}

const fn recovery_position(state: &BtcCanonicalRecoveryStateV1, chain: Chain) -> ChainPosition {
    match chain {
        Chain::Bitcoin => {
            ChainPosition::block_height(Chain::Bitcoin, state.bitcoin_best_height as u64)
        }
        Chain::Lez => ChainPosition::timestamp(Chain::Lez, state.lez_unix_seconds),
        Chain::Monero | Chain::Zcash => ChainPosition::timestamp(chain, 0),
    }
}

fn lifecycle_effect_eq(left: &BtcLifecycleTransitionV1, right: &BtcLifecycleTransitionV1) -> bool {
    left == right
}

fn parse_bitcoin_revealing_claim(
    bytes: &ExactPublicEffectBytes,
) -> Result<Transaction, BtcSdkError> {
    let transaction: Transaction =
        deserialize(bytes.as_slice()).map_err(BtcSdkError::MalformedBitcoinClaim)?;
    if transaction.is_coinbase()
        || transaction.input.len() != 1
        || !transaction.input[0].script_sig.is_empty()
        || transaction.input[0].witness.len() != 1
        || transaction.input[0]
            .witness
            .iter()
            .next()
            .is_none_or(|signature| signature.len() != SCHNORR_SIGNATURE_BYTES)
    {
        return Err(BtcSdkError::InvalidBitcoinClaimWitness);
    }
    Ok(transaction)
}

fn bitcoin_claim_signature(
    transaction: &Transaction,
) -> Result<[u8; SCHNORR_SIGNATURE_BYTES], BtcSdkError> {
    transaction.input[0]
        .witness
        .iter()
        .next()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(BtcSdkError::InvalidBitcoinClaimWitness)
}

fn parse_signed_bitcoin_funding(
    bytes: &ExactPublicEffectBytes,
) -> Result<Transaction, BtcSdkError> {
    let transaction: Transaction =
        deserialize(bytes.as_slice()).map_err(BtcSdkError::MalformedBitcoinFunding)?;
    if transaction.is_coinbase()
        || transaction.input.is_empty()
        || !transaction
            .input
            .iter()
            .any(|input| !input.script_sig.is_empty() || !input.witness.is_empty())
    {
        return Err(BtcSdkError::UnsignedBitcoinFunding);
    }
    Ok(transaction)
}

fn validate_first_lock(
    agreement: &BtcAgreementV1,
    lock_effects: &BtcPreparedLockEffectsV1,
    evidence: &BtcFirstLockEvidenceV1,
) -> Result<ConfirmedBtcFirstLockV1, BtcSdkError> {
    let first_plan = lock_effects.plan_for_participant(agreement, Participant::Taker);
    match (agreement.direction(), evidence) {
        (SwapDirection::TakerSellsForeign, BtcFirstLockEvidenceV1::Bitcoin(observed)) => {
            validate_bitcoin_first_lock(agreement, first_plan, observed)?;
        }
        (SwapDirection::TakerSellsLez, BtcFirstLockEvidenceV1::Lez(observed)) => {
            validate_lez_first_lock(agreement, first_plan, observed)?;
        }
        _ => return Err(BtcSdkError::WrongFirstLockChain),
    }
    Ok(ConfirmedBtcFirstLockV1 {
        agreement_commitment: *agreement.agreement_commitment(),
        plan_commitment: first_plan.commitment(),
        direction: agreement.direction(),
    })
}

fn validate_bitcoin_first_lock(
    agreement: &BtcAgreementV1,
    plan: &ExactPublicEffectPlanV1,
    observed: &BitcoinFirstLockEvidenceV1,
) -> Result<(), BtcSdkError> {
    if observed.genesis_block_hash != *agreement.bitcoin_chain_policy().genesis_block_hash() {
        return Err(BtcSdkError::FirstLockNetworkMismatch);
    }
    let expected = &plan.steps()[0];
    if observed.exact_transaction != *expected.exact_bytes() {
        return Err(BtcSdkError::FirstLockPlanMismatch);
    }
    let transaction = parse_signed_bitcoin_funding(&observed.exact_transaction)?;
    if transaction.compute_txid().to_string() != expected.expected_public_id().as_str() {
        return Err(BtcSdkError::FirstLockIdentityMismatch);
    }
    let output = transaction
        .output
        .get(agreement.funding_terms().output_index() as usize)
        .ok_or(BtcSdkError::FirstLockTermsMismatch)?;
    if output.value.to_sat() != agreement.funding_terms().value_sat()
        || output.script_pubkey.as_bytes() != agreement.p2tr_contract().script_pubkey_bytes()
    {
        return Err(BtcSdkError::FirstLockTermsMismatch);
    }
    let required = agreement.required_bitcoin_confirmations();
    if observed.confirmations < required {
        return Err(BtcSdkError::FirstLockConfirmationLag {
            required,
            actual: observed.confirmations,
        });
    }
    Ok(())
}

fn validate_lez_first_lock(
    agreement: &BtcAgreementV1,
    plan: &ExactPublicEffectPlanV1,
    observed: &LezFirstLockEvidenceV1,
) -> Result<(), BtcSdkError> {
    if observed.genesis_block_hash != *agreement.lez_terms().genesis_block_hash() {
        return Err(BtcSdkError::FirstLockNetworkMismatch);
    }
    let [initialization, funding] = plan.steps() else {
        return Err(BtcSdkError::FirstLockPlanMismatch);
    };
    if &observed.initialization_public_id != initialization.expected_public_id()
        || &observed.funding_public_id != funding.expected_public_id()
    {
        return Err(BtcSdkError::FirstLockIdentityMismatch);
    }
    if observed.exact_initialization != *initialization.exact_bytes()
        || observed.exact_funding != *funding.exact_bytes()
    {
        return Err(BtcSdkError::FirstLockPlanMismatch);
    }
    if observed.metadata_account != *agreement.lez_terms().metadata_account()
        || observed.custody_account != *agreement.lez_terms().custody_account()
        || observed.amount != agreement.lez_terms().amount()
    {
        return Err(BtcSdkError::FirstLockTermsMismatch);
    }
    if !observed.finalized {
        return Err(BtcSdkError::FirstLockNotFinalized);
    }
    Ok(())
}

fn validate_confirmation_binding(
    agreement: &BtcAgreementV1,
    first_plan: &ExactPublicEffectPlanV1,
    first: &ConfirmedBtcFirstLockV1,
) -> Result<(), BtcSdkError> {
    if first.agreement_commitment != *agreement.agreement_commitment()
        || first.plan_commitment != first_plan.commitment()
        || first.direction != agreement.direction()
    {
        return Err(BtcSdkError::FirstLockConfirmationMismatch);
    }
    Ok(())
}

const fn claim_order(direction: SwapDirection) -> ClaimOrder {
    match direction {
        SwapDirection::TakerSellsForeign => ClaimOrder::LEZ_THEN_FOREIGN,
        SwapDirection::TakerSellsLez => ClaimOrder::FOREIGN_THEN_LEZ,
    }
}
