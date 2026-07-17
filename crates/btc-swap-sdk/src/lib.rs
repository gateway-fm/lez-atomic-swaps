//! Bitcoin Taproot protocol adapter for LEZ atomic swaps.
//!
//! This first M3 slice owns canonical BIP-341 output and transaction
//! construction. The current boundary accepts an externally derived aggregate
//! x-only key and a completed Schnorr signature as canonical bytes. Future
//! `MuSig2` and adaptor types will remain behind that byte boundary rather than
//! sharing incompatible curve-library Rust types.

mod adaptor;
mod agreement_v1;
mod p2tr;
mod sdk;
mod transaction;

pub use adaptor::{
    AdaptorSessionContext, AdaptorSessionError, AdaptorSigner, FreshAdaptorNonce,
    PersistedAdaptorSigningMaterial, SigningRole, adapt_presignature,
    aggregate_adaptor_presignature, extract_adaptor_secret, sign_persisted_adaptor_partial,
    verify_adaptor_partial_signature, verify_adaptor_presignature, verify_adaptor_secret,
    verify_final_signature, verify_nonce_commitment,
};
pub use agreement_v1::{
    BTC_AGREEMENT_SCHEMA_V1, BtcAdaptorSessionDomain, BtcAgreementBodyV1, BtcAgreementRecordV1,
    BtcAgreementV1, BtcAgreementV1Error, BtcChainPolicyV1, BtcClaimTermsV1, BtcFundingTermsV1,
    BtcLezTermsV1, BtcOutputKeyParityV1, BtcP2trTermsV1, BtcParticipantIdentityV1,
    BtcParticipantsV1, BtcRecoveryPlanV1, MAX_BITCOIN_REQUIRED_CONFIRMATIONS,
    MAX_BTC_AGREEMENT_RECORD_BYTES,
};
pub use p2tr::{
    CsvBlockDelay, CsvBlockDelayError, InvalidXOnlyKey, OutputKeyParity, P2trSwapOutput,
    P2trSwapOutputError, RefundXOnlyKey, TwoPartyAggregateKey, XOnlyKeyPurpose,
};
pub use sdk::{
    AcceptedBtcAgreementV1, ActiveBtcSwap, BitcoinFirstLockEvidenceV1,
    BitcoinRevealingClaimEvidenceV1, BtcActiveSwapEnvelopeV1, BtcFirstLockEvidenceV1,
    BtcLifecycleActionV1, BtcPairSdk, BtcPreparedClaimEffectsV1, BtcPreparedLockEffectsV1,
    BtcPreparedProtocolV1, BtcProtocolCapabilityGapV1, BtcProtocolTermsV1,
    BtcRecoveredClaimMaterialV1, BtcRevealingClaimEvidenceV1, BtcSdkError, BtcSwapStatusV1,
    BtcUnsupportedCanonicalStateV1, BtcUnsupportedRecoveryActionV1, ConfirmedBtcFirstLockV1,
    LezFirstLockEvidenceV1, LezRevealingClaimEvidenceV1, PreparedBitcoinFundingV1,
    PreparedLezClaimTemplateV1, PreparedLezFundingV1, ValidatedBtcProtocolTermsV1,
};
pub use transaction::{
    CooperativeKeyPathSpend, CooperativeKeyPathSpendError, RefundScriptPathSpend,
    RefundScriptPathSpendError,
};
