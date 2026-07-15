//! Bitcoin Taproot protocol adapter for LEZ atomic swaps.
//!
//! This first M3 slice owns canonical BIP-341 output and transaction
//! construction. The current boundary accepts an externally derived aggregate
//! x-only key and a completed Schnorr signature as canonical bytes. Future
//! `MuSig2` and adaptor types will remain behind that byte boundary rather than
//! sharing incompatible curve-library Rust types.

mod adaptor;
mod p2tr;
mod transaction;

pub use adaptor::{
    AdaptorSessionContext, AdaptorSessionError, AdaptorSigner, FreshAdaptorNonce,
    PersistedAdaptorSigningMaterial, SigningRole, adapt_presignature,
    aggregate_adaptor_presignature, extract_adaptor_secret, sign_persisted_adaptor_partial,
    verify_adaptor_partial_signature, verify_adaptor_presignature, verify_final_signature,
    verify_nonce_commitment,
};
pub use p2tr::{
    CsvBlockDelay, CsvBlockDelayError, InvalidXOnlyKey, OutputKeyParity, P2trSwapOutput,
    P2trSwapOutputError, RefundXOnlyKey, TwoPartyAggregateKey, XOnlyKeyPurpose,
};
pub use transaction::{CooperativeKeyPathSpend, CooperativeKeyPathSpendError};
