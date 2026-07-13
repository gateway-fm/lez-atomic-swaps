//! Agreement- and coordinator-bound input context for the Zcash follow-up claim.

use lez_swap_core::{Chain, Participant, Phase, SwapCoordinator, SwapId};
use thiserror::Error;
use zcash_encoding::ReverseHex;
use zcash_transparent::bundle::{OutPoint, TxOut};

use crate::{CanonicalZcashOutputObservation, ObservationError, ZcashStableTip, ZecAgreementV1};

// Revealing a reusable preimage is more consequential than ordinary local-dev projection.
// Keep one block of reorg distance even when the deterministic profile permits one confirmation;
// stricter signed production confirmation policies continue to take precedence.
const MIN_PRE_REVEAL_ZCASH_CONFIRMATIONS: u32 = 2;

/// Exact durable Zcash funding output that protects the preimage-revealing claim.
///
/// This context is derived by [`crate::ActiveZecSwap`] after replaying durable
/// lock transitions. An adapter must use this outpoint directly; it must not
/// discover or substitute a different output from node or wallet state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZcashFundingContextV1 {
    agreement_commitment: [u8; 32],
    swap_id: SwapId,
    zcash_funder: Participant,
    funding_transaction_id: Box<str>,
    funding_transaction_id_bytes: [u8; 32],
    funding_outpoint: OutPoint,
}

impl ZcashFundingContextV1 {
    pub(crate) fn from_active(
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
    ) -> Result<Self, ZcashClaimContextError> {
        if !matches!(
            coordinator.phase(),
            Phase::BothLegsLocked
                | Phase::ClaimEvidenceAvailable
                | Phase::MakerLegRefunded
                | Phase::TakerLegRefunded
        ) {
            return Err(ZcashClaimContextError::FundingNotReady {
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

/// Exact durable Zcash funding input an adapter is authorized to spend.
///
/// This follow-up context remains claim-phase-only while sharing the exact funding
/// identity used by the earlier pre-reveal safety check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZcashClaimContextV1 {
    funding: ZcashFundingContextV1,
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
        ZcashFundingContextV1::from_active(agreement, coordinator).map(|funding| Self { funding })
    }

    /// Shared agreement- and outpoint-bound funding identity.
    #[must_use]
    pub const fn funding_context(&self) -> &ZcashFundingContextV1 {
        &self.funding
    }

    /// Commitment of the exact dual-signed agreement authorizing this spend.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        self.funding.agreement_commitment()
    }

    /// Stable application swap identity associated with the funding transition.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        self.funding.swap_id()
    }

    /// Agreement-derived participant that funded the Zcash leg.
    #[must_use]
    pub const fn zcash_funder(&self) -> Participant {
        self.funding.zcash_funder()
    }

    /// Canonical lowercase RPC display form of the coordinator-pinned transaction ID.
    #[must_use]
    pub fn funding_transaction_id(&self) -> &str {
        self.funding.funding_transaction_id()
    }

    /// Canonical internal transaction ID bytes decoded from the RPC display form.
    #[must_use]
    pub const fn funding_transaction_id_bytes(&self) -> &[u8; 32] {
        self.funding.funding_transaction_id_bytes()
    }

    /// Protocol-fixed output index of the agreement BIP-199 funding output.
    #[must_use]
    pub const fn funding_output_index(&self) -> u32 {
        self.funding.funding_output_index()
    }

    /// Exact Zcash funding output at protocol-fixed index zero.
    #[must_use]
    pub const fn funding_outpoint(&self) -> &OutPoint {
        self.funding.funding_outpoint()
    }
}

/// Untrusted result of a stable-tip-bracketed Zcash unspent-output lookup.
///
/// An adapter constructs this from the exact outpoint lookup and the best-chain tip
/// sampled immediately before and after it. The SDK validates every field before reveal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZcashUnspentOutputSnapshotV1 {
    outpoint: OutPoint,
    output: TxOut,
    tip: ZcashStableTip,
}

impl ZcashUnspentOutputSnapshotV1 {
    /// Creates an untrusted unspent-output snapshot for SDK validation.
    #[must_use]
    pub const fn new(outpoint: OutPoint, output: TxOut, tip: ZcashStableTip) -> Self {
        Self {
            outpoint,
            output,
            tip,
        }
    }

    /// Outpoint returned by the unspent-output query.
    #[must_use]
    pub const fn outpoint(&self) -> &OutPoint {
        &self.outpoint
    }

    /// Value and script returned by the unspent-output query.
    #[must_use]
    pub const fn output(&self) -> &TxOut {
        &self.output
    }

    /// Tip samples bracketing the unspent-output query.
    #[must_use]
    pub const fn tip(&self) -> ZcashStableTip {
        self.tip
    }
}

/// Fresh Zcash funding state observed immediately before a preimage-revealing LEZ submit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ZcashFundingObservationV1 {
    /// The exact funding transaction is stably absent.
    Absent,
    /// The exact funding output is no longer in the UTXO set.
    Spent,
    /// The node view changed while the adapter assembled the observation.
    Unstable,
    /// Canonical transaction evidence and an exact same-tip unspent-output lookup.
    Confirmed {
        /// Independently decoded canonical funding transaction.
        canonical: Box<CanonicalZcashOutputObservation>,
        /// Exact UTXO result bracketed by stable tip samples.
        unspent: Box<ZcashUnspentOutputSnapshotV1>,
    },
}

impl ZcashFundingObservationV1 {
    /// Wraps canonical funding evidence and its same-tip unspent-output result.
    #[must_use]
    pub fn confirmed(
        canonical: CanonicalZcashOutputObservation,
        unspent: ZcashUnspentOutputSnapshotV1,
    ) -> Self {
        Self::Confirmed {
            canonical: Box::new(canonical),
            unspent: Box::new(unspent),
        }
    }

    pub(crate) fn validate_confirmed_for_reveal(
        canonical: &CanonicalZcashOutputObservation,
        unspent: &ZcashUnspentOutputSnapshotV1,
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        context: &ZcashFundingContextV1,
    ) -> Result<(), ZcashFundingObservationError> {
        let expected = agreement.binding().expected_output();
        if canonical.network() != expected.network() {
            return Err(ZcashFundingObservationError::NetworkMismatch);
        }
        if canonical.consensus_branch_id() != expected.consensus_branch_id() {
            return Err(ZcashFundingObservationError::ConsensusBranchMismatch);
        }
        if canonical.transaction_id().as_ref() != context.funding_transaction_id_bytes() {
            return Err(ZcashFundingObservationError::TransactionIdMismatch);
        }
        if canonical.outpoint() != context.funding_outpoint() {
            return Err(ZcashFundingObservationError::CanonicalOutpointMismatch);
        }
        if canonical.output().value() != expected.value() {
            return Err(ZcashFundingObservationError::CanonicalValueMismatch);
        }
        if canonical.output().script_pubkey().0.0.as_slice()
            != expected.contract().p2sh_script_pubkey()
            || canonical.p2sh_script_pubkey() != expected.contract().p2sh_script_pubkey()
            || canonical.redeem_script() != expected.contract().redeem_script()
        {
            return Err(ZcashFundingObservationError::CanonicalScriptMismatch);
        }
        let required = coordinator
            .required_confirmations(context.zcash_funder())
            .max(MIN_PRE_REVEAL_ZCASH_CONFIRMATIONS);
        let actual = canonical.confirmations().get();
        if actual < required {
            return Err(ZcashFundingObservationError::InsufficientConfirmations {
                required,
                actual,
            });
        }

        let (unspent_tip_hash, unspent_tip_height) = unspent
            .tip()
            .validated()
            .map_err(ZcashFundingObservationError::InvalidUnspentTip)?;
        if unspent.outpoint() != context.funding_outpoint() {
            return Err(ZcashFundingObservationError::UnspentOutpointMismatch);
        }
        if unspent.output().value() != canonical.output().value() {
            return Err(ZcashFundingObservationError::UnspentValueMismatch);
        }
        if unspent.output().script_pubkey() != canonical.output().script_pubkey() {
            return Err(ZcashFundingObservationError::UnspentScriptMismatch);
        }
        if unspent_tip_hash != canonical.tip_block_hash()
            || unspent_tip_height != canonical.tip_height()
        {
            return Err(ZcashFundingObservationError::UnspentTipMismatch);
        }

        let tip_height = u32::from(canonical.tip_height());
        let refund_height = agreement.zcash_refund_at_height();
        if tip_height >= refund_height {
            return Err(ZcashFundingObservationError::RefundHeightReached {
                tip_height,
                refund_height,
            });
        }
        Ok(())
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
    /// Funding safety is meaningful from both-locked through the follow-up phase.
    #[error(
        "Zcash funding context requires BothLegsLocked or ClaimEvidenceAvailable; active phase is {actual:?}"
    )]
    FundingNotReady {
        /// Actual durable coordinator phase.
        actual: Phase,
    },
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

/// A claimed confirmed-and-unspent funding view that is unsafe for preimage reveal.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ZcashFundingObservationError {
    /// Canonical evidence was built for a different network.
    #[error("pre-reveal Zcash funding network differs from signed terms")]
    NetworkMismatch,
    /// Canonical evidence was decoded under a different consensus branch.
    #[error("pre-reveal Zcash funding consensus branch differs from signed terms")]
    ConsensusBranchMismatch,
    /// The canonical transaction differs from the exact durable funding transaction.
    #[error("pre-reveal Zcash funding transaction ID differs from the durable lock")]
    TransactionIdMismatch,
    /// The canonical output differs from protocol-fixed output zero.
    #[error("pre-reveal canonical Zcash funding outpoint differs from the durable lock")]
    CanonicalOutpointMismatch,
    /// The canonical funding value differs from signed terms.
    #[error("pre-reveal canonical Zcash funding value differs from signed terms")]
    CanonicalValueMismatch,
    /// The canonical funding script differs from the signed BIP-199 contract.
    #[error("pre-reveal canonical Zcash funding script differs from signed terms")]
    CanonicalScriptMismatch,
    /// Fresh canonical depth is below the stricter agreement-or-reveal threshold.
    #[error("pre-reveal Zcash funding has {actual} confirmations; safety gate requires {required}")]
    InsufficientConfirmations {
        /// Agreement-derived or conservative pre-reveal minimum.
        required: u32,
        /// Fresh canonical depth.
        actual: u32,
    },
    /// Tip samples bracketing the UTXO lookup were not stable.
    #[error("pre-reveal Zcash UTXO lookup did not use a stable tip")]
    InvalidUnspentTip(#[source] ObservationError),
    /// The UTXO query returned a different transaction or output index.
    #[error("pre-reveal Zcash UTXO lookup returned a different outpoint")]
    UnspentOutpointMismatch,
    /// The UTXO query returned a different value.
    #[error("pre-reveal Zcash UTXO lookup returned a different value")]
    UnspentValueMismatch,
    /// The UTXO query returned a different script.
    #[error("pre-reveal Zcash UTXO lookup returned a different script")]
    UnspentScriptMismatch,
    /// Canonical and UTXO evidence were not assembled at the same stable tip.
    #[error("pre-reveal canonical and unspent Zcash evidence use different tips")]
    UnspentTipMismatch,
    /// The refund branch is already eligible at the fresh best-chain tip.
    #[error("pre-reveal Zcash tip {tip_height} reached refund CLTV height {refund_height}")]
    RefundHeightReached {
        /// Fresh canonical tip height.
        tip_height: u32,
        /// Signed BIP-199 refund height.
        refund_height: u32,
    },
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
