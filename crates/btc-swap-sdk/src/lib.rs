//! Bitcoin Taproot protocol adapter for LEZ atomic swaps.
//!
//! The crate owns canonical BIP-341 output and transaction construction,
//! role-separated `MuSig2` adaptor signing, and the public durable pair
//! lifecycle. Its public signing boundary exchanges canonical byte arrays so
//! curve-library Rust types remain private to the crate.

mod agreement_v1;
mod asset_sdk;
mod p2tr;
mod sdk;
mod transaction;

pub use agreement_v1::{
    BTC_AGREEMENT_SCHEMA_V1, BTC_LEZ_ASSET_EXTENSION_SCHEMA_V1, BTC_LEZ_ASSET_EXTENSION_V1_DOMAIN,
    BtcAdaptorSessionDomain, BtcAgreementBodyV1, BtcAgreementRecordV1, BtcAgreementV1,
    BtcAgreementV1Error, BtcChainPolicyV1, BtcClaimTermsV1, BtcFundingTermsV1,
    BtcLezAssetExtensionBodyV1, BtcLezAssetExtensionRecordV1, BtcLezAssetExtensionV1,
    BtcLezAssetExtensionV1Error, BtcLezAssetV1, BtcLezCustomTokenTermsV1, BtcLezTermsV1,
    BtcOutputKeyParityV1, BtcP2trTermsV1, BtcParticipantIdentityV1, BtcParticipantsV1,
    BtcRecoveryPlanV1, MAX_BITCOIN_REQUIRED_CONFIRMATIONS, MAX_BTC_AGREEMENT_RECORD_BYTES,
    MAX_BTC_LEZ_ASSET_EXTENSION_RECORD_BYTES,
};
pub use asset_sdk::{
    ActiveBtcLezAssetSwapV1, BtcLezAssetFirstLockEvidenceV1, BtcLezAssetPreparedLockEffectsV1,
    BtcLezAssetSdkError, ConfirmedBtcLezAssetFirstLockV1, LezAssetCustodyEvidenceV1,
    LezAssetFirstLockEvidenceV1, PreparedLezAssetFundingV1,
};
pub use lez_adaptor_signature::{
    AdaptorSessionContext, AdaptorSessionError, AdaptorSigner, FreshAdaptorNonce,
    PersistedAdaptorSigningMaterial, SigningRole, adapt_presignature,
    aggregate_adaptor_presignature, extract_adaptor_secret, sign_persisted_adaptor_partial,
    verify_adaptor_partial_signature, verify_adaptor_presignature, verify_adaptor_secret,
    verify_final_signature, verify_nonce_commitment,
};
pub use p2tr::{
    CsvBlockDelay, CsvBlockDelayError, InvalidXOnlyKey, OutputKeyParity, P2trSwapOutput,
    P2trSwapOutputError, RefundXOnlyKey, TwoPartyAggregateKey, XOnlyKeyPurpose,
};
pub use sdk::{
    AcceptedBtcAgreementV1, ActiveBtcSwap, BitcoinBtcLifecyclePort,
    BitcoinCanonicalRecoveryStateV1, BitcoinFirstLockEvidenceV1, BitcoinFollowupClaimEvidenceV1,
    BitcoinRevealingClaimEvidenceV1, BtcActiveSwapEnvelopeV1, BtcBoxPortError,
    BtcCanonicalRecoveryStateV1, BtcFirstLockEvidenceV1, BtcFollowupClaimEvidenceV1,
    BtcLifecycleActionV1, BtcLifecycleChainOutcomeV1, BtcLifecycleCodecError,
    BtcLifecycleDriveOutcomeV1, BtcLifecycleDriveRequestV1, BtcLifecycleRecordV1,
    BtcLifecycleRuntime, BtcLifecycleSdk, BtcLifecycleStore, BtcLifecycleStoreCompareExchangeV1,
    BtcLifecycleStoreCreateV1, BtcLifecycleTransitionOutcomeV1, BtcLifecycleTransitionV1,
    BtcPairSdk, BtcPreparedClaimEffectsV1, BtcPreparedLockEffectsV1, BtcPreparedProtocolV1,
    BtcPreparedRecoveryEffectsV1, BtcProtocolTermsV1, BtcRecoveredClaimMaterialV1,
    BtcRecoveryActionV1, BtcRecoveryWaitReasonV1, BtcRevealingClaimEvidenceV1, BtcSdkError,
    BtcSwapStatusV1, ConfirmedBtcFirstLockV1, InMemoryBtcLifecycleStore,
    InMemoryBtcLifecycleStoreError, LezBtcLifecyclePort, LezCanonicalRecoveryStateV1,
    LezFirstLockEvidenceV1, LezFollowupClaimEvidenceV1, LezRevealingClaimEvidenceV1,
    MAX_BTC_LIFECYCLE_RECORD_BYTES, PreparedBitcoinFundingV1, PreparedBitcoinRefundV1,
    PreparedLezClaimTemplateV1, PreparedLezFundingV1, PreparedLezRefundV1, StoredBtcLifecycleSdk,
    ValidatedBtcProtocolTermsV1,
};
pub use transaction::{
    CooperativeKeyPathSpend, CooperativeKeyPathSpendError, RefundScriptPathSpend,
    RefundScriptPathSpendError,
};
