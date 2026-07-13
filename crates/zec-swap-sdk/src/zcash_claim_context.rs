//! Agreement- and coordinator-bound input context for the Zcash follow-up claim.

use lez_swap_core::{Chain, Participant, Phase, SwapCoordinator, SwapId};
use thiserror::Error;
use zcash_encoding::ReverseHex;
use zcash_transparent::bundle::OutPoint;

use crate::ZecAgreementV1;

/// Exact durable Zcash funding input an adapter is authorized to spend.
///
/// This context is derived by [`crate::ActiveZecSwap`] after replaying durable
/// lock transitions. An adapter must use this outpoint directly; it must not
/// discover or substitute a different output from node or wallet state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZcashClaimContextV1 {
    agreement_commitment: [u8; 32],
    swap_id: SwapId,
    zcash_funder: Participant,
    funding_transaction_id: Box<str>,
    funding_transaction_id_bytes: [u8; 32],
    funding_outpoint: OutPoint,
}

impl ZcashClaimContextV1 {
    pub(crate) fn from_active(
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
    ) -> Result<Self, ZcashClaimContextError> {
        if coordinator.phase() != Phase::ClaimEvidenceAvailable {
            return Err(ZcashClaimContextError::ClaimNotReady {
                actual: coordinator.phase(),
            });
        }
        let agreed = agreement.coordinator();
        if coordinator.id() != agreed.id()
            || coordinator.pair() != agreed.pair()
            || coordinator.direction() != agreed.direction()
        {
            return Err(ZcashClaimContextError::CoordinatorContextMismatch);
        }

        let zcash_funder = agreement.lez_claimant();
        let actual_chain = coordinator.funded_chain(zcash_funder);
        if actual_chain != Chain::Zcash {
            return Err(ZcashClaimContextError::ZcashFunderMismatch {
                participant: zcash_funder,
                actual_chain,
            });
        }
        let funding_transaction_id = coordinator.funding_transaction_id(zcash_funder).ok_or(
            ZcashClaimContextError::MissingFundingTransactionId {
                participant: zcash_funder,
            },
        )?;
        let funding_transaction_id_bytes = parse_canonical_txid(funding_transaction_id)?;

        Ok(Self {
            agreement_commitment: *agreement.agreement_commitment(),
            swap_id: coordinator.id().clone(),
            zcash_funder,
            funding_transaction_id: funding_transaction_id.into(),
            funding_transaction_id_bytes,
            funding_outpoint: OutPoint::new(funding_transaction_id_bytes, 0),
        })
    }

    /// Commitment of the exact dual-signed agreement authorizing this spend.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    /// Stable application swap identity associated with the funding transition.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Agreement-derived participant that funded the Zcash leg.
    #[must_use]
    pub const fn zcash_funder(&self) -> Participant {
        self.zcash_funder
    }

    /// Canonical lowercase RPC display form of the coordinator-pinned transaction ID.
    #[must_use]
    pub fn funding_transaction_id(&self) -> &str {
        &self.funding_transaction_id
    }

    /// Canonical internal transaction ID bytes decoded from the RPC display form.
    #[must_use]
    pub const fn funding_transaction_id_bytes(&self) -> &[u8; 32] {
        &self.funding_transaction_id_bytes
    }

    /// Protocol-fixed output index of the agreement's BIP-199 funding output.
    #[must_use]
    pub const fn funding_output_index(&self) -> u32 {
        0
    }

    /// Exact Zcash funding output at protocol-fixed index zero.
    #[must_use]
    pub const fn funding_outpoint(&self) -> &OutPoint {
        &self.funding_outpoint
    }
}

fn parse_canonical_txid(value: &str) -> Result<[u8; 32], ZcashClaimContextError> {
    if value.len() != 64 {
        return Err(ZcashClaimContextError::MalformedFundingTransactionId);
    }
    let decoded =
        ReverseHex::decode(value).ok_or(ZcashClaimContextError::MalformedFundingTransactionId)?;
    if ReverseHex::encode(&decoded) != value {
        return Err(ZcashClaimContextError::NonCanonicalFundingTransactionId);
    }
    if decoded == [0; 32] {
        return Err(ZcashClaimContextError::NullFundingTransactionId);
    }
    Ok(decoded)
}

/// Failure to derive the only durable Zcash outpoint safe for a follow-up claim.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ZcashClaimContextError {
    /// A Zcash claim context exists only after the revealing LEZ claim is durable.
    #[error(
        "Zcash follow-up claim context requires ClaimEvidenceAvailable; active phase is {actual:?}"
    )]
    ClaimNotReady {
        /// Actual durable coordinator phase.
        actual: Phase,
    },
    /// The replayed coordinator does not describe the accepted agreement.
    #[error("replayed coordinator does not match the accepted Zcash agreement")]
    CoordinatorContextMismatch,
    /// The agreement-derived funder does not fund Zcash in the replayed coordinator.
    #[error("agreement-derived Zcash funder {participant:?} funds {actual_chain:?}")]
    ZcashFunderMismatch {
        /// Participant derived from the accepted agreement.
        participant: Participant,
        /// Chain reported by the replayed coordinator.
        actual_chain: Chain,
    },
    /// No durable lock transition pinned a transaction for the Zcash funder.
    #[error("no durable funding transaction is pinned for Zcash funder {participant:?}")]
    MissingFundingTransactionId {
        /// Agreement-derived Zcash funder.
        participant: Participant,
    },
    /// The pinned value is not exactly a 32-byte hexadecimal transaction ID.
    #[error("coordinator-pinned Zcash funding transaction ID is malformed")]
    MalformedFundingTransactionId,
    /// The display form is not exact lowercase reverse hexadecimal.
    #[error(
        "coordinator-pinned Zcash funding transaction ID is not canonical lowercase reverse hex"
    )]
    NonCanonicalFundingTransactionId,
    /// The all-zero transaction ID is never a spendable funding transaction.
    #[error("coordinator-pinned Zcash funding transaction ID is null")]
    NullFundingTransactionId,
}

#[cfg(test)]
mod tests {
    use zcash_encoding::ReverseHex;

    use super::{ZcashClaimContextError, parse_canonical_txid};

    const CANONICAL: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    #[test]
    fn canonical_reverse_hex_roundtrips_to_internal_bytes() {
        let decoded = parse_canonical_txid(CANONICAL).expect("canonical transaction ID");
        assert_eq!(
            decoded,
            ReverseHex::decode(CANONICAL).expect("known reverse hex")
        );
        assert_eq!(ReverseHex::encode(&decoded), CANONICAL);
    }

    #[test]
    fn malformed_noncanonical_and_null_ids_fail_closed() {
        for malformed in ["", "00", &"g".repeat(64)] {
            assert_eq!(
                parse_canonical_txid(malformed),
                Err(ZcashClaimContextError::MalformedFundingTransactionId)
            );
        }
        assert_eq!(
            parse_canonical_txid(&CANONICAL.to_ascii_uppercase()),
            Err(ZcashClaimContextError::NonCanonicalFundingTransactionId)
        );
        assert_eq!(
            parse_canonical_txid(&"0".repeat(64)),
            Err(ZcashClaimContextError::NullFundingTransactionId)
        );
    }
}
