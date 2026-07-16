//! Role-fixed deterministic LEZ/Bitcoin funding facade.
//!
//! This module deliberately stops at exact lock construction and canonical
//! first-lock validation. It performs no node, discovery, negotiation,
//! persistence, claim, or refund I/O. The unsupported claim and recovery
//! methods on its [`SwapProtocol`] implementation return typed capability gaps
//! instead of pretending the current M3 slice is a full lifecycle SDK.

use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::Hash as _;
use bitcoin::{Transaction, Txid};
use lez_swap_core::{Chain, Participant, Phase, SwapCoordinator, SwapDirection, SwapId};
use lez_swap_sdk_core::{
    ClaimOrder, ErrorCategory, ExactPublicEffectBytes, ExactPublicEffectPlanV1,
    ExpectedPublicEffectId, ProtocolError, PublicEffectPlanError, PublicEffectStepId,
    PublicEffectStepV1, SwapProtocol,
};

use crate::{BtcAgreementRecordV1, BtcAgreementV1, BtcAgreementV1Error, BtcChainPolicyV1};

const BITCOIN_FUNDING_STEP: &str = "bitcoin.funding";
const LEZ_INITIALIZE_STEP: &str = "lez.initialize";
const LEZ_FUND_STEP: &str = "lez.fund";

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

/// Untrusted terms consumed by the deterministic common lifecycle contract.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcProtocolTermsV1 {
    agreement: BtcAgreementRecordV1,
    lock_effects: BtcPreparedLockEffectsV1,
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
        }
    }
}

/// Fully validated agreement and exact lock effects.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct ValidatedBtcProtocolTermsV1 {
    agreement: BtcAgreementV1,
    lock_effects: BtcPreparedLockEffectsV1,
}

impl ValidatedBtcProtocolTermsV1 {
    /// Validated immutable agreement.
    #[must_use]
    pub const fn agreement(&self) -> &BtcAgreementV1 {
        &self.agreement
    }
}

/// Funding-ready deterministic protocol state.
///
/// Claim and refund recovery material is not represented by this slice; the
/// corresponding trait calls return [`BtcSdkError::UnsupportedCapability`].
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

/// Untrusted durable activation envelope used for deterministic restart.
#[derive(Clone, Eq, PartialEq)]
#[must_use]
pub struct BtcActiveSwapEnvelopeV1 {
    agreement_wire: Box<[u8]>,
    local_participant: Participant,
    revision: u64,
    lock_effects: BtcPreparedLockEffectsV1,
}

impl std::fmt::Debug for BtcActiveSwapEnvelopeV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BtcActiveSwapEnvelopeV1")
            .field("agreement_wire", &"[REDACTED]")
            .field("local_participant", &self.local_participant)
            .field("revision", &self.revision)
            .field("lock_effects", &self.lock_effects)
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
        }
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

    /// Safe local action at this revision.
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
        })
    }

    /// Revalidates an offline durable envelope without discovery, negotiation,
    /// node, or public-effect submission I/O.
    ///
    /// # Errors
    ///
    /// Rejects malformed agreement bytes, role substitution, effect drift, or
    /// revisions whose transition replay is not implemented by this slice.
    pub fn resume(&self, envelope: BtcActiveSwapEnvelopeV1) -> Result<ActiveBtcSwap, BtcSdkError> {
        if envelope.revision != 0 {
            return Err(BtcSdkError::UnsupportedResumeRevision(envelope.revision));
        }
        if envelope.local_participant != self.local_participant {
            return Err(BtcSdkError::LocalRoleMismatch {
                expected: self.local_participant,
                actual: envelope.local_participant,
            });
        }
        let accepted = self.accept_wire(&envelope.agreement_wire)?;
        self.activate(accepted, envelope.lock_effects)
    }
}

/// Post-activation deterministic funding facade.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct ActiveBtcSwap {
    accepted: AcceptedBtcAgreementV1,
    lock_effects: BtcPreparedLockEffectsV1,
    coordinator: SwapCoordinator,
    revision: u64,
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

    /// Explicit direction-specific claim order, even though claim construction
    /// remains an unsupported capability in this slice.
    pub const fn claim_order(&self) -> ClaimOrder {
        claim_order(self.accepted.agreement.direction())
    }

    /// Safe next revision-zero role action.
    pub fn next_action(&self) -> BtcLifecycleActionV1 {
        if self.local_participant() == Participant::Maker {
            return BtcLifecycleActionV1::AwaitTakerFirstLock;
        }
        match self.coordinator.funded_chain(Participant::Taker) {
            Chain::Bitcoin => BtcLifecycleActionV1::PublishBitcoinFirstLock,
            Chain::Lez => BtcLifecycleActionV1::PublishLezFirstLock,
            Chain::Monero | Chain::Zcash => unreachable!("validated BTC agreement"),
        }
    }

    /// Creates an exact offline restart envelope for application-owned durable storage.
    pub fn durable_envelope(&self) -> BtcActiveSwapEnvelopeV1 {
        BtcActiveSwapEnvelopeV1 {
            agreement_wire: self.accepted.wire.clone(),
            local_participant: self.local_participant(),
            revision: self.revision,
            lock_effects: self.lock_effects.clone(),
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

/// Full-lifecycle capabilities intentionally absent from this funding slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcProtocolCapabilityGapV1 {
    /// Complete dual-chain adaptor presignatures and signed recovery effects
    /// cannot yet be proven durable before the first public lock.
    PreLockRecovery,
    /// Canonical revealing-claim extraction is not exposed yet.
    RevealingClaim,
    /// Material-consuming follow-up claim construction is not exposed yet.
    FollowupClaim,
    /// Construction-ordered recovery selection is not exposed yet.
    Recovery,
}

/// Placeholder evidence accepted only to return a typed claim capability gap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BtcUnsupportedClaimEvidenceV1;

/// Placeholder material accepted only to return a typed claim capability gap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BtcUnsupportedClaimMaterialV1;

/// Placeholder state accepted only to return a typed recovery capability gap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BtcUnsupportedCanonicalStateV1;

/// Placeholder recovery result that is never successfully constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BtcUnsupportedRecoveryActionV1;

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
    /// Accepted/durable role differs from this facade's fixed role.
    #[error("stored local role is {actual:?}; SDK role is {expected:?}")]
    LocalRoleMismatch {
        /// Role fixed on the SDK.
        expected: Participant,
        /// Role supplied by untrusted accepted/durable state.
        actual: Participant,
    },
    /// This funding slice cannot replay post-activation transitions yet.
    #[error("BTC SDK funding slice cannot resume revision {0}")]
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
    /// Full-lifecycle behavior is deliberately unavailable in this slice.
    #[error("BTC SDK capability is not implemented in this funding slice: {0:?}")]
    UnsupportedCapability(BtcProtocolCapabilityGapV1),
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
            | Self::LocalRoleMismatch { .. }
            | Self::WrongFirstLockChain
            | Self::FirstLockPlanMismatch
            | Self::FirstLockIdentityMismatch
            | Self::FirstLockConfirmationMismatch => ErrorCategory::TranscriptMismatch,
            Self::BitcoinFundingOutputMismatch | Self::FirstLockTermsMismatch => {
                ErrorCategory::WrongValue
            }
            Self::UnsupportedResumeRevision(_) | Self::UnsupportedCapability(_) => {
                ErrorCategory::UnsupportedCapability
            }
            Self::FirstLockNetworkMismatch => ErrorCategory::WrongNetwork,
            Self::FirstLockConfirmationLag { .. } => ErrorCategory::ObservationLag,
            Self::FirstLockNotFinalized => ErrorCategory::NonCanonicalEvidence,
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
    type RevealingClaimEvidence = BtcUnsupportedClaimEvidenceV1;
    type RecoveredClaimMaterial = BtcUnsupportedClaimMaterialV1;
    type FollowupClaimTemplate = ExactPublicEffectPlanV1;
    type CanonicalChainState = BtcUnsupportedCanonicalStateV1;
    type RecoveryAction = BtcUnsupportedRecoveryActionV1;
    type Error = BtcSdkError;

    fn validate_terms(&self, terms: &Self::Terms) -> Result<Self::ValidatedTerms, Self::Error> {
        let agreement = BtcAgreementV1::validate_for_bitcoin_policy(
            terms.agreement.clone(),
            &self.bitcoin_policy,
        )?;
        terms.lock_effects.validate(&agreement)?;
        Ok(ValidatedBtcProtocolTermsV1 {
            agreement,
            lock_effects: terms.lock_effects.clone(),
        })
    }

    fn prepare(&self, _terms: Self::ValidatedTerms) -> Result<Self::Prepared, Self::Error> {
        Err(BtcSdkError::UnsupportedCapability(
            BtcProtocolCapabilityGapV1::PreLockRecovery,
        ))
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
        _prepared: &Self::Prepared,
        _evidence: &Self::RevealingClaimEvidence,
    ) -> Result<Self::RecoveredClaimMaterial, Self::Error> {
        Err(BtcSdkError::UnsupportedCapability(
            BtcProtocolCapabilityGapV1::RevealingClaim,
        ))
    }

    fn build_followup_claim(
        &self,
        _prepared: &Self::Prepared,
        _material: &Self::RecoveredClaimMaterial,
    ) -> Result<Self::FollowupClaimTemplate, Self::Error> {
        Err(BtcSdkError::UnsupportedCapability(
            BtcProtocolCapabilityGapV1::FollowupClaim,
        ))
    }

    fn recovery_action(
        &self,
        _prepared: &Self::Prepared,
        _state: &Self::CanonicalChainState,
    ) -> Result<Self::RecoveryAction, Self::Error> {
        Err(BtcSdkError::UnsupportedCapability(
            BtcProtocolCapabilityGapV1::Recovery,
        ))
    }
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
