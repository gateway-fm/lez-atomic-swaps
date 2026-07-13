//! Transparent Zcash protocol adapter for LEZ atomic swaps.

mod agreement_v1;
mod claim;
mod claim_material;
mod claim_record;
mod first_lock;
mod first_lock_record;
mod funding;
mod lez_claim_observation;
mod lez_derivation;
mod lez_observation;
mod lifecycle;
mod maker_lock;
mod maker_lock_record;
mod observation;
mod observation_record;
mod observed_maker_lock;
mod observed_maker_lock_record;
mod observed_taker_lock;
mod ports;
mod profile;
mod sdk;
mod spend_observation;
mod transaction;
mod zec_binding_record;

pub use agreement_v1::{
    AcceptedZecAgreementEnvelopeV1, AcceptedZecAgreementV1, FundingInputSetError, LezAssetV1,
    LezChainIdentityV1, LezEnvironmentV1, MAX_ZEC_AGREEMENT_RECORD_BYTES,
    MAX_ZEC_APPLICATION_SWAP_ID_BYTES, MAX_ZEC_FUNDING_INPUTS, MAX_ZEC_FUNDING_SCRIPT_BYTES,
    NegotiationTranscriptV1, SwapDirectionRecordV1, ZEC_AGREEMENT_V1_DOMAIN,
    ZEC_CONCRETE_AGREEMENT_SCHEMA_V1, ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashFundingInputSetV1,
    ZcashFundingInputV1, ZcashTransparentDestinationV1, ZecAgreementBodyV1,
    ZecAgreementExecutionError, ZecAgreementRecordV1, ZecAgreementV1, ZecAgreementV1Error,
    ZecLezTermsV1, ZecParticipantIdentityV1, ZecParticipantsV1, ZecRefundPlanV1, ZecRolePayoutV1,
    ZecTransactionPolicyV1,
};
pub use claim::{
    ClaimDriveOutcome, ClaimError, ClaimIntentV1, ClaimStepV1, FollowupClaimEvidenceV1,
    FollowupClaimObservationV1, FollowupClaimTransitionV1, MAX_CLAIM_SUBMISSION_BYTES,
    ObservedFollowupClaimTransitionV1, ObservedRevealingClaimTransitionV1,
    PreparedClaimSubmissionV1, RevealingClaimEvidenceV1, RevealingClaimObservationV1,
    RevealingClaimTransitionV1,
};
pub use claim_material::{
    ClaimMaterialContext, ClaimMaterialPurpose, ClaimSubmissionContext,
    MAX_PROTECTED_CLAIM_PAYLOAD_BYTES, PROTECTED_CLAIM_SCHEMA_V1, ProtectedClaimEnvelope,
    ProtectedClaimError, ProtectedClaimKey, ProtectedClaimPayloadEnvelope,
};
pub use claim_record::{
    CLAIM_RECORD_SCHEMA_V1, CLAIM_RECORD_SCHEMA_V2, ClaimIntentRecordV1, ClaimRecordError,
    FollowupClaimTransitionRecordV1, ObservedFollowupClaimTransitionRecordV1,
    ObservedRevealingClaimTransitionRecordV1, RevealingClaimTransitionRecordV1,
};

pub use first_lock::{
    CreateFirstLockOutcome, FirstLockConfirmedEvidenceV1, FirstLockDriveOutcome,
    FirstLockIntentError, FirstLockIntentV1, FirstLockObservation, FirstLockPlanV1,
    FirstLockProjectionCommit, FirstLockStepV1, FirstLockTransitionError, FirstLockTransitionV1,
    MAX_FIRST_LOCK_SUBMISSION_BYTES, PreparedFirstLockSubmissionV1,
};
pub use first_lock_record::{
    FIRST_LOCK_RECORD_SCHEMA_V1, FirstLockIntentRecordV1, FirstLockRecordError,
    FirstLockTransitionRecordV1,
};
pub use funding::{
    FundingBuildError, FundingSelection, TransparentFundingRequest, TransparentUtxo,
    build_funding_transaction, select_funding_utxos,
};
pub use lez_claim_observation::{
    CANONICAL_LEZ_CLAIM_SNAPSHOT_SCHEMA_V1, CanonicalLezClaimSnapshotRecordV1,
    LezClaimInstructionKindV1, LezClaimInstructionV1, LezClaimNodeSnapshotV1,
    LezClaimObservationError, LezClaimTransactionSnapshotV1,
};
pub use lez_derivation::{
    derive_lez_metadata_account_v1, derive_lez_native_custody_account_v1, derive_lez_public_pda_v1,
    derive_lez_swap_id_v1, derive_lez_token_account_v1,
};
pub use lez_observation::{
    CanonicalLezEscrowObservationV1, CanonicalLezEscrowRemovalV1, LezCustodySnapshotV1,
    LezEscrowMetadataSnapshotV1, LezEscrowStatusV1, LezFundInstructionV1,
    LezFundTransactionSnapshotV1, LezInclusionStatusV1, LezNodeRemovalSnapshotV1,
    LezNodeSnapshotV1, LezObservationError, LezObservationEventV1, LezObservationReconciliationV1,
    LezObservationTrackerError, LezObservationTrackerV1, LezStableTipV1,
};
pub use lifecycle::{BoxPortError, ClaimPreimage, ZecLifecycleAction, ZecSdkError};
pub use maker_lock::{
    MakerLockDriveOutcome, MakerLockError, MakerLockIntentV1, MakerLockTransitionV1,
};
pub use maker_lock_record::{
    MAKER_LOCK_RECORD_SCHEMA_V1, MakerLockIntentRecordV1, MakerLockRecordError,
    MakerLockTransitionRecordV1,
};

pub use observation::{
    CanonicalZcashOutputObservation, CanonicalZcashOutputRemoval, ExpectedBip199Output,
    ObservationError, ObservationTrackerError, ZcashNodeRemovalSnapshot, ZcashNodeSnapshot,
    ZcashObservationEvent, ZcashObservationReconciliation, ZcashObservationTracker, ZcashStableTip,
};
pub use observation_record::{
    HistoricalReplayError, ObservationRecordError, ZcashNetworkRecordV1,
    ZcashObservationEventRecordV1, ZcashOutputObservationRecordV1, ZcashOutputRemovalRecordV1,
    replay_zcash_observation_history, revalidate_historical_event,
};
pub use observed_maker_lock::{
    MakerLockObservationV1, OBSERVED_MAKER_LOCK_SCHEMA_V1, ObserveMakerLockOutcome,
    ObservedMakerLockError, ObservedMakerLockTransitionV1,
};
pub use observed_maker_lock_record::ObservedMakerLockTransitionRecordV1;
pub use observed_taker_lock::{
    MakerFundingEligibilityOutcome, ObserveTakerFirstLockOutcome, ObservedTakerFirstLockEvidenceV1,
    ObservedTakerFirstLockTransitionError, ObservedTakerFirstLockTransitionRecordV1,
    ObservedTakerFirstLockTransitionV1, TakerFirstLockObservationV1,
};

pub use ports::{
    ClaimRecoveryStore, CreateAgreementOutcome, LezClaimPort, LezFirstLockPort,
    LezMakerLockObservationPort, LezTakerFirstLockObservationPort, NegotiationChannel,
    OfferDiscovery, RecoveryStore, ZcashClaimPort, ZcashFirstLockPort,
    ZcashMakerLockObservationPort, ZcashTakerFirstLockObservationPort,
};
pub use profile::{ProfileError, ZecProfileId, ZecRefundProfile};
pub use sdk::{ActiveZecSwap, ZecPairSdk};
pub use spend_observation::{
    Bip199SpendKind, CanonicalZcashSpendObservation, ExpectedBip199Spend,
    SdkCanonicalSpendDeviation, SdkCanonicalSpendPolicy, SpendObservationError,
    ZCASH_MAX_SCRIPT_BYTES, ZEBRA_MAX_BLOCK_BYTES, ZcashSpendNodeSnapshot,
};
pub use zec_binding_record::{
    ZecBindingRecordError, ZecProfileRecordV1, ZecSwapBinding, ZecSwapBindingRecordV1,
};

pub use transaction::{
    TransactionBuildError, TransparentSpendRequest, build_claim_transaction,
    build_refund_transaction,
};

use zcash_script::{
    Opcode, op,
    opcode::PushValue,
    pattern, pv,
    script::{Component, Evaluable},
};

/// Failures while encoding a BIP-199 spend stack.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ScriptBuildError {
    /// A stack item exceeds Zcash Script's consensus push-size limit.
    #[error("{field} is too large for a script stack item: {length} bytes")]
    StackItemTooLarge {
        /// The input being encoded.
        field: &'static str,
        /// Its rejected byte length.
        length: usize,
    },
}

/// Non-final sequence used by BIP-199 refund inputs so CLTV is effective.
pub const REFUND_INPUT_SEQUENCE: u32 = u32::MAX - 1;

/// The exact SHA-256/CLTV P2PKH redeem script specified by BIP-199.
///
/// The contract stores script-level public-key hashes. Key derivation and
/// ownership belong to the transparent-wallet adapter, which uses canonical
/// librustzcash types.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bip199Contract {
    refund_lock_time: u32,
    refund_pubkey_hash: [u8; 20],
    secret_digest: [u8; 32],
    claimant_pubkey_hash: [u8; 20],
    redeem_script: Vec<u8>,
    p2sh_script_pubkey: Vec<u8>,
}

impl Bip199Contract {
    /// Constructs the exact BIP-199 common-tail layout.
    ///
    /// `refund_lock_time` is the absolute Zcash `nLockTime` threshold. The
    /// spending transaction must also use a non-final input sequence for CLTV
    /// to take effect. Public-key hashes must commit to compressed secp256k1
    /// public-key encodings; the transaction adapter deliberately emits only
    /// canonical compressed public keys.
    #[must_use]
    pub fn new(
        refund_lock_time: u32,
        refund_pubkey_hash: [u8; 20],
        secret_digest: [u8; 32],
        claimant_pubkey_hash: [u8; 20],
    ) -> Self {
        let claim_branch = [
            &[op::SHA256][..],
            &pattern::equals(pattern::push_256b_hash(&secret_digest), true),
            &[
                op::DUP,
                op::HASH160,
                Opcode::PushValue(pattern::push_160b_hash(&claimant_pubkey_hash)),
            ],
        ]
        .concat();
        let refund_branch = [
            &pattern::check_lock_time_verify(refund_lock_time)[..],
            &[
                op::DUP,
                op::HASH160,
                Opcode::PushValue(pattern::push_160b_hash(&refund_pubkey_hash)),
            ],
        ]
        .concat();
        let mut opcodes = pattern::branch(&claim_branch, &refund_branch);
        opcodes.extend([op::EQUALVERIFY, op::CHECKSIG]);

        let redeem_script = Component(opcodes);
        let p2sh_script_pubkey = Component(pattern::pay_to_script_hash(&redeem_script)).to_bytes();

        Self {
            refund_lock_time,
            refund_pubkey_hash,
            secret_digest,
            claimant_pubkey_hash,
            redeem_script: redeem_script.to_bytes(),
            p2sh_script_pubkey,
        }
    }

    /// Returns the consensus-encoded redeem script bytes.
    #[must_use]
    pub fn redeem_script(&self) -> &[u8] {
        &self.redeem_script
    }

    /// Returns the consensus-encoded P2SH script pubkey that funds this contract.
    #[must_use]
    pub fn p2sh_script_pubkey(&self) -> &[u8] {
        &self.p2sh_script_pubkey
    }

    /// Returns the exact absolute lock time required by the refund branch.
    #[must_use]
    pub const fn refund_lock_time(&self) -> u32 {
        self.refund_lock_time
    }

    /// Returns the non-final sequence every refund transaction input must use.
    #[must_use]
    pub const fn refund_input_sequence(&self) -> u32 {
        REFUND_INPUT_SEQUENCE
    }

    pub(crate) const fn refund_pubkey_hash(&self) -> [u8; 20] {
        self.refund_pubkey_hash
    }

    pub(crate) const fn secret_digest(&self) -> [u8; 32] {
        self.secret_digest
    }

    pub(crate) const fn claimant_pubkey_hash(&self) -> [u8; 20] {
        self.claimant_pubkey_hash
    }

    /// Encodes `[signature, claimant_pubkey, preimage, true, redeem_script]`.
    ///
    /// The signature must include its Zcash sighash type byte. Signature and
    /// public-key validity are checked by the consensus interpreter, not this
    /// push-only serializer.
    ///
    /// # Errors
    ///
    /// Returns [`ScriptBuildError::StackItemTooLarge`] if any supplied item
    /// exceeds the consensus push-size limit.
    pub fn claim_script_sig(
        &self,
        signature: &[u8],
        claimant_pubkey: &[u8],
        preimage: &[u8],
    ) -> Result<Vec<u8>, ScriptBuildError> {
        self.spend_script_sig(
            &[
                stack_item("signature", signature)?,
                stack_item("claimant public key", claimant_pubkey)?,
                stack_item("preimage", preimage)?,
                pv::_1,
            ],
            "claim redeem script",
        )
    }

    /// Encodes `[signature, refund_pubkey, false, redeem_script]`.
    ///
    /// The spending transaction must use this contract's absolute lock time and
    /// a non-final input sequence; Zebra remains the consensus authority.
    ///
    /// # Errors
    ///
    /// Returns [`ScriptBuildError::StackItemTooLarge`] if any supplied item
    /// exceeds the consensus push-size limit.
    pub fn refund_script_sig(
        &self,
        signature: &[u8],
        refund_pubkey: &[u8],
    ) -> Result<Vec<u8>, ScriptBuildError> {
        self.spend_script_sig(
            &[
                stack_item("signature", signature)?,
                stack_item("refund public key", refund_pubkey)?,
                pv::_0,
            ],
            "refund redeem script",
        )
    }

    fn spend_script_sig<const N: usize>(
        &self,
        stack: &[PushValue; N],
        redeem_field: &'static str,
    ) -> Result<Vec<u8>, ScriptBuildError> {
        let mut stack = stack.to_vec();
        stack.push(stack_item(redeem_field, &self.redeem_script)?);
        Ok(Component(stack).to_bytes())
    }
}

fn stack_item(field: &'static str, bytes: &[u8]) -> Result<PushValue, ScriptBuildError> {
    pv::push_value(bytes).ok_or(ScriptBuildError::StackItemTooLarge {
        field,
        length: bytes.len(),
    })
}
