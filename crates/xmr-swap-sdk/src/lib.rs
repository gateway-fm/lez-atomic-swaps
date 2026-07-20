//! Monero spend-key-share protocol adapter for LEZ atomic swaps.
//!
//! Progressive M4 boundaries cover maintained cross-curve DLEQ proofs, shared
//! Monero spend-key reconstruction, a bounded dual-signed agreement, separate
//! claim/refund adaptor sessions, countersigned Stage-B activation, and a
//! structural finalized-LEZ-lock candidate check. Trusted LEZ evidence and the
//! exact Monero-output release gate remain pending; caller-supplied candidates
//! are not lifecycle authority. The crate does not claim production
//! cryptographic acceptance.

mod agreement_v1;
mod cross_curve;
mod shared_spend;

pub use agreement_v1::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES,
    MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES, MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES,
    ValidatedXmrActivationBodyV1, ValidatedXmrAgreementBodyV1, XMR_ACTIVATION_SCHEMA_V1,
    XMR_AGREEMENT_SCHEMA_V1, XmrActivatedAgreementV1, XmrActivationBodyV1, XmrActivationRecordV1,
    XmrAdaptorSessionDescriptorV1, XmrAdaptorSessionPurposeV1, XmrAgreementBodyV1,
    XmrAgreementRecordV1, XmrAgreementV1, XmrAgreementV1Error, XmrLezInitializePlanV1,
    XmrLezLockCandidateV1, XmrLezLockStatusV1, XmrLezTermsV1, XmrMessagesV1, XmrMoneroTermsV1,
    XmrNamedProfileV1, XmrParticipantIdentityV1, XmrParticipantsV1, XmrRoleV1,
    XmrSessionTranscriptV1, XmrSwapDirectionV1, XmrValidatedLezLockCandidateV1, XmrWindowsV1,
};

pub use cross_curve::{
    CROSS_CURVE_DLEQ_SCHEMA_V1, CrossCurveDleqError, CrossCurveDleqProofV1, CrossCurveScalar,
};
pub use shared_spend::{
    MoneroAddressNetworkV1, MoneroPrivateViewKey, MoneroSharedAddressV1, MoneroSharedSpendError,
    ReconstructedMoneroSpendKey,
};
