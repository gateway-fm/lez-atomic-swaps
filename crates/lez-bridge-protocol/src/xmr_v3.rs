//! Strict additive protocol for the LEZ-native leg of an XMR atomic swap.
//!
//! These types mirror only primitive guest and finalized-node facts. They do
//! not import the XMR SDK or an official LEZ runtime graph.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{
    AccountIds, AggregateBip340Signature, ChainClock, DiscoveryWindow, FinalizedBlockIdentity,
    Hex32, MessageContext, NativeAmount, NativeCustodyFacts, ObservedTransactionFacts, Participant,
    PreparedTransaction, PreparedWitnessedClaim, ProtocolValueError, RuntimeDescriptor,
    SubmissionOutcome, TransactionId,
};

/// Exact public-account facts bracketing one harmless local-profile clock driver.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct CurrentProfileClockAccountSnapshot {
    /// Official account identity.
    pub account_id: Hex32,
    /// Native balance.
    pub balance: u128,
    /// Public transaction nonce.
    pub nonce: u128,
    /// Owning program identity.
    pub program_owner: Hex32,
    /// SHA-256 of the canonical official account encoding.
    pub account_sha256: Hex32,
}

impl CurrentProfileClockAccountSnapshot {
    /// Creates one exact account snapshot.
    pub const fn new(
        account_id: Hex32,
        balance: u128,
        nonce: u128,
        program_owner: Hex32,
        account_sha256: Hex32,
    ) -> Self {
        Self {
            account_id,
            balance,
            nonce,
            program_owner,
            account_sha256,
        }
    }
}

/// Requests preparation of one bounded local-profile clock transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareCurrentProfileClockRequest {
    /// Version, run isolation, correlation, and Taker role.
    pub context: MessageContext,
    /// Exact run-owned local sidecar runtime.
    pub runtime: RuntimeDescriptor,
    /// Activated XMR-native escrow terms proved from live metadata and custody.
    pub terms: XmrNativeEscrowTermsV3,
    /// Exact Maker owner receiving one native unit.
    pub recipient_account_id: Hex32,
    /// Exclusive consensus-clock upper bound for the refund path.
    pub exclusive_punish_at_ms: u64,
}

impl PrepareCurrentProfileClockRequest {
    /// Creates one narrow clock preparation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: XmrNativeEscrowTermsV3,
        recipient_account_id: Hex32,
        exclusive_punish_at_ms: u64,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            recipient_account_id,
            exclusive_punish_at_ms,
        }
    }
}

/// Exact durable clock transaction reservation returned before submission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareCurrentProfileClockResult {
    /// Echoed preparation context.
    pub context: MessageContext,
    /// Exact run-owned local sidecar runtime.
    pub runtime: RuntimeDescriptor,
    /// Activated terms proved from live funded escrow state.
    pub terms: XmrNativeEscrowTermsV3,
    /// Exact Maker recipient.
    pub recipient_account_id: Hex32,
    /// Exclusive consensus-clock upper bound.
    pub exclusive_punish_at_ms: u64,
    /// Exact signed official transaction reserved by the Taker sidecar.
    pub transaction: PreparedTransaction,
    /// Stable sequencer clock before preparation.
    pub clock_before: ChainClock,
    /// Exact Taker owner snapshot before preparation.
    pub sender_before: CurrentProfileClockAccountSnapshot,
    /// Exact Maker owner snapshot before preparation.
    pub recipient_before: CurrentProfileClockAccountSnapshot,
    /// Metadata account hash before preparation.
    pub metadata_account_sha256_before: Hex32,
    /// Custody account hash before preparation.
    pub custody_account_sha256_before: Hex32,
}

/// Requests read-only verification of one exact submitted clock transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct VerifyCurrentProfileClockRequest {
    /// Read-only verification context.
    pub context: MessageContext,
    /// Exact run-owned local sidecar runtime.
    pub runtime: RuntimeDescriptor,
    /// Complete exact durable preparation.
    pub preparation: PrepareCurrentProfileClockResult,
    /// Exact canonical submission acknowledgement.
    pub submission: crate::SubmitTransactionResult,
}

/// Auditable result of one local-profile clock-driving transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct VerifyCurrentProfileClockResult {
    pub context: MessageContext,
    pub runtime: RuntimeDescriptor,
    pub terms: XmrNativeEscrowTermsV3,
    pub recipient_account_id: Hex32,
    pub exclusive_punish_at_ms: u64,
    pub transaction_id: TransactionId,
    pub submission_request_id: crate::RequestId,
    pub submission_outcome: SubmissionOutcome,
    pub node_submission_attempts: u8,
    pub transfer_amount: u128,
    pub clock_before: ChainClock,
    pub clock_after: ChainClock,
    pub sender_before: CurrentProfileClockAccountSnapshot,
    pub sender_after: CurrentProfileClockAccountSnapshot,
    pub recipient_before: CurrentProfileClockAccountSnapshot,
    pub recipient_after: CurrentProfileClockAccountSnapshot,
    pub metadata_account_sha256_before: Hex32,
    pub metadata_account_sha256_after: Hex32,
    pub custody_account_sha256_before: Hex32,
    pub custody_account_sha256_after: Hex32,
    pub escrow_accounts_byte_identical: bool,
    pub accounting_verified: bool,
    pub local_only: bool,
    pub retry_policy: String,
}

/// Exact standalone wire version for XMR-native escrow terms.
pub const XMR_NATIVE_ESCROW_TERMS_VERSION: u16 = 3;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct XmrNativeEscrowTermsV3Wire {
    version: u16,
    swap_id: Hex32,
    activation_commitment: Hex32,
    escrow_program_id: Hex32,
    authenticated_transfer_program_id: Hex32,
    metadata_account_id: Hex32,
    custody_account_id: Hex32,
    depositor: Participant,
    depositor_account_id: Hex32,
    claimant: Participant,
    claimant_account_id: Hex32,
    claim_aggregate_x_only_public_key: Hex32,
    claim_authority_account_id: Hex32,
    refund_aggregate_x_only_public_key: Hex32,
    refund_authority_account_id: Hex32,
    maker_dleq_transcript_commitment: Hex32,
    taker_dleq_transcript_commitment: Hex32,
    claim_partial_context_binding: Hex32,
    claim_partial_commitment: Hex32,
    amount: NativeAmount,
    refund_at_ms: u64,
    punish_at_ms: u64,
    claim_message_hash: Hex32,
    refund_message_hash: Hex32,
    punish_message_hash: Hex32,
}

/// Complete primitive input for one standalone XMR-native LEZ escrow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrNativeEscrowTermsV3Input {
    /// Swap identifier used for metadata and custody PDA derivation.
    pub swap_id: Hex32,
    /// Activated agreement commitment stored as the guest terms hash.
    pub activation_commitment: Hex32,
    /// Exact XMR-capable escrow program identity.
    pub escrow_program_id: Hex32,
    /// Exact authenticated-transfer program identity.
    pub authenticated_transfer_program_id: Hex32,
    /// Precomputed metadata PDA account identity.
    pub metadata_account_id: Hex32,
    /// Precomputed native custody PDA account identity.
    pub custody_account_id: Hex32,
    /// Fixed LEZ depositor role; XMR v1 requires Taker.
    pub depositor: Participant,
    /// Taker account that signs initialization, funding, and authorization.
    pub depositor_account_id: Hex32,
    /// Fixed LEZ claimant role; XMR v1 requires Maker.
    pub claimant: Participant,
    /// Maker account receiving a claim or punish transfer.
    pub claimant_account_id: Hex32,
    /// Aggregate x-only key authorizing the claim transaction.
    pub claim_aggregate_x_only_public_key: Hex32,
    /// Official LEZ account derived from the claim aggregate key.
    pub claim_authority_account_id: Hex32,
    /// Aggregate x-only key authorizing the refund transaction.
    pub refund_aggregate_x_only_public_key: Hex32,
    /// Official LEZ account derived from the refund aggregate key.
    pub refund_authority_account_id: Hex32,
    /// Commitment to the Maker cross-curve DLEQ transcript.
    pub maker_dleq_transcript_commitment: Hex32,
    /// Commitment to the Taker cross-curve DLEQ transcript.
    pub taker_dleq_transcript_commitment: Hex32,
    /// Durable context binding for the Taker claim partial.
    pub claim_partial_context_binding: Hex32,
    /// Hash commitment to the exact Taker claim partial.
    pub claim_partial_commitment: Hex32,
    /// Native LEZ amount locked in custody.
    pub amount: u128,
    /// Exclusive claim boundary and inclusive refund boundary.
    pub refund_at_ms: u64,
    /// Exclusive refund boundary and inclusive punish boundary.
    pub punish_at_ms: u64,
    /// Exact official unsigned claim message hash.
    pub claim_message_hash: Hex32,
    /// Exact official unsigned refund message hash.
    pub refund_message_hash: Hex32,
    /// Exact official unsigned punish message hash.
    pub punish_message_hash: Hex32,
}

/// Strict standalone v3 agreement terms for an XMR-native LEZ escrow.
///
/// This is deliberately separate from legacy untagged terms envelopes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    try_from = "XmrNativeEscrowTermsV3Wire",
    into = "XmrNativeEscrowTermsV3Wire"
)]
#[must_use]
pub struct XmrNativeEscrowTermsV3(XmrNativeEscrowTermsV3Wire);

impl XmrNativeEscrowTermsV3 {
    /// Validates the fixed roles, nonzero bindings, distinct identities, and windows.
    ///
    /// # Errors
    ///
    /// Rejects unsupported roles, zero values, unsafe aliases, an empty amount,
    /// or timestamps that do not form claim, refund, and punish intervals.
    pub fn new(input: XmrNativeEscrowTermsV3Input) -> Result<Self, ProtocolValueError> {
        validate_terms_input(&input)?;
        Ok(Self(terms_wire_from_input(&input)))
    }

    /// Returns a complete immutable copy of every validated primitive field.
    ///
    /// This is the sidecar and adapter boundary; callers never need to recover
    /// terms by serializing and reparsing JSON.
    pub const fn to_input(self) -> XmrNativeEscrowTermsV3Input {
        XmrNativeEscrowTermsV3Input {
            swap_id: self.0.swap_id,
            activation_commitment: self.0.activation_commitment,
            escrow_program_id: self.0.escrow_program_id,
            authenticated_transfer_program_id: self.0.authenticated_transfer_program_id,
            metadata_account_id: self.0.metadata_account_id,
            custody_account_id: self.0.custody_account_id,
            depositor: self.0.depositor,
            depositor_account_id: self.0.depositor_account_id,
            claimant: self.0.claimant,
            claimant_account_id: self.0.claimant_account_id,
            claim_aggregate_x_only_public_key: self.0.claim_aggregate_x_only_public_key,
            claim_authority_account_id: self.0.claim_authority_account_id,
            refund_aggregate_x_only_public_key: self.0.refund_aggregate_x_only_public_key,
            refund_authority_account_id: self.0.refund_authority_account_id,
            maker_dleq_transcript_commitment: self.0.maker_dleq_transcript_commitment,
            taker_dleq_transcript_commitment: self.0.taker_dleq_transcript_commitment,
            claim_partial_context_binding: self.0.claim_partial_context_binding,
            claim_partial_commitment: self.0.claim_partial_commitment,
            amount: self.0.amount.as_u128(),
            refund_at_ms: self.0.refund_at_ms,
            punish_at_ms: self.0.punish_at_ms,
            claim_message_hash: self.0.claim_message_hash,
            refund_message_hash: self.0.refund_message_hash,
            punish_message_hash: self.0.punish_message_hash,
        }
    }

    /// Verifies that a request context and discovered runtime belong to these terms.
    ///
    /// # Errors
    ///
    /// Rejects a non-v0.2 runtime, role mismatch, wrong escrow program, or a
    /// sidecar signer that is not the exact agreement account for its role.
    pub fn validate_runtime_binding(
        self,
        context: &MessageContext,
        runtime: &RuntimeDescriptor,
    ) -> Result<(), ProtocolValueError> {
        ensure_fact(
            runtime.compatibility == crate::RuntimeCompatibility::LeeV0_2_0,
            "runtime compatibility",
        )?;
        ensure_fact(
            context.sidecar_role == runtime.sidecar_role,
            "runtime sidecar role",
        )?;
        ensure_fact(
            runtime.escrow_program_id == self.0.escrow_program_id,
            "runtime escrow program",
        )?;
        let expected_signer = match runtime.sidecar_role {
            Participant::Maker => self.0.claimant_account_id,
            Participant::Taker => self.0.depositor_account_id,
        };
        ensure_fact(
            runtime.signer_account_id == expected_signer,
            "runtime signer account",
        )
    }
}

impl TryFrom<XmrNativeEscrowTermsV3Wire> for XmrNativeEscrowTermsV3 {
    type Error = ProtocolValueError;

    fn try_from(wire: XmrNativeEscrowTermsV3Wire) -> Result<Self, Self::Error> {
        if wire.version != XMR_NATIVE_ESCROW_TERMS_VERSION {
            return Err(ProtocolValueError::UnsupportedXmrNativeTermsVersion(
                wire.version,
            ));
        }
        Self::new(XmrNativeEscrowTermsV3Input {
            swap_id: wire.swap_id,
            activation_commitment: wire.activation_commitment,
            escrow_program_id: wire.escrow_program_id,
            authenticated_transfer_program_id: wire.authenticated_transfer_program_id,
            metadata_account_id: wire.metadata_account_id,
            custody_account_id: wire.custody_account_id,
            depositor: wire.depositor,
            depositor_account_id: wire.depositor_account_id,
            claimant: wire.claimant,
            claimant_account_id: wire.claimant_account_id,
            claim_aggregate_x_only_public_key: wire.claim_aggregate_x_only_public_key,
            claim_authority_account_id: wire.claim_authority_account_id,
            refund_aggregate_x_only_public_key: wire.refund_aggregate_x_only_public_key,
            refund_authority_account_id: wire.refund_authority_account_id,
            maker_dleq_transcript_commitment: wire.maker_dleq_transcript_commitment,
            taker_dleq_transcript_commitment: wire.taker_dleq_transcript_commitment,
            claim_partial_context_binding: wire.claim_partial_context_binding,
            claim_partial_commitment: wire.claim_partial_commitment,
            amount: wire.amount.as_u128(),
            refund_at_ms: wire.refund_at_ms,
            punish_at_ms: wire.punish_at_ms,
            claim_message_hash: wire.claim_message_hash,
            refund_message_hash: wire.refund_message_hash,
            punish_message_hash: wire.punish_message_hash,
        })
    }
}

impl From<XmrNativeEscrowTermsV3> for XmrNativeEscrowTermsV3Wire {
    fn from(terms: XmrNativeEscrowTermsV3) -> Self {
        terms.0
    }
}

/// Sensitive Taker claim partial published only by the authorization transaction.
#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
#[must_use]
pub struct XmrClaimPartialV3([u8; 32]);

impl XmrClaimPartialV3 {
    /// Wraps one nonzero claim partial.
    ///
    /// # Errors
    ///
    /// Rejects the invalid all-zero sentinel.
    pub fn new(bytes: [u8; 32]) -> Result<Self, ProtocolValueError> {
        if bytes == [0; 32] {
            Err(ProtocolValueError::ZeroXmrValue("claim partial"))
        } else {
            Ok(Self(bytes))
        }
    }

    /// Explicitly exposes the exact partial to the official transaction builder.
    #[must_use]
    pub const fn expose_secret(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for XmrClaimPartialV3 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("XmrClaimPartialV3([REDACTED])")
    }
}

impl Serialize for XmrClaimPartialV3 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Hex32::from_bytes(self.0).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for XmrClaimPartialV3 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Hex32::deserialize(deserializer)?;
        Self::new(*value.as_bytes()).map_err(D::Error::custom)
    }
}

macro_rules! preparation_request {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
        #[serde(deny_unknown_fields)]
        #[must_use]
        pub struct $name {
            /// Version, run, request, and dedicated sidecar-role binding.
            pub context: MessageContext,
            /// Exact expected runtime identity.
            pub runtime: RuntimeDescriptor,
            /// Complete standalone agreement and guest binding.
            pub terms: XmrNativeEscrowTermsV3,
        }

        impl $name {
            /// Creates the strictly bound preparation request.
            pub const fn new(
                context: MessageContext,
                runtime: RuntimeDescriptor,
                terms: XmrNativeEscrowTermsV3,
            ) -> Self {
                Self {
                    context,
                    runtime,
                    terms,
                }
            }
        }
    };
}

preparation_request!(
    PrepareNativeXmrClaimV3Request,
    "Requests the exact unsigned XMR-native claim message."
);
preparation_request!(
    PrepareNativeXmrRefundV3Request,
    "Requests the exact unsigned XMR-native refund message."
);
preparation_request!(
    PrepareNativeXmrPunishV3Request,
    "Requests the unilateral post-punish-boundary transaction."
);
preparation_request!(
    PrepareNativeXmrEscrowV3Request,
    "Requests XMR-native initialization and funding transactions."
);

/// Exact unsigned XMR claim transcript reserved by the sidecar.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[must_use]
pub struct PrepareNativeXmrClaimV3Result {
    /// Echoed request context.
    pub context: MessageContext,
    /// Echoed immutable standalone terms.
    pub terms: XmrNativeEscrowTermsV3,
    /// Canonical unsigned message and exact hash.
    pub claim: PreparedWitnessedClaim,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareNativeXmrClaimV3ResultWire {
    context: MessageContext,
    terms: XmrNativeEscrowTermsV3,
    claim: PreparedWitnessedClaim,
}

impl PrepareNativeXmrClaimV3Result {
    /// Creates one exact claim reservation.
    ///
    /// # Errors
    ///
    /// Rejects a message hash that differs from the activated agreement.
    pub fn new(
        context: MessageContext,
        terms: XmrNativeEscrowTermsV3,
        claim: PreparedWitnessedClaim,
    ) -> Result<Self, ProtocolValueError> {
        ensure_fact(
            claim.message_hash == terms.0.claim_message_hash,
            "prepared claim message hash",
        )?;
        Ok(Self {
            context,
            terms,
            claim,
        })
    }
}

impl<'de> Deserialize<'de> for PrepareNativeXmrClaimV3Result {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PrepareNativeXmrClaimV3ResultWire::deserialize(deserializer)?;
        Self::new(wire.context, wire.terms, wire.claim).map_err(D::Error::custom)
    }
}

/// Exact unsigned XMR refund transcript reserved by the sidecar.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[must_use]
pub struct PrepareNativeXmrRefundV3Result {
    /// Echoed request context.
    pub context: MessageContext,
    /// Echoed immutable standalone terms.
    pub terms: XmrNativeEscrowTermsV3,
    /// Canonical unsigned message and exact hash.
    pub refund: PreparedWitnessedClaim,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareNativeXmrRefundV3ResultWire {
    context: MessageContext,
    terms: XmrNativeEscrowTermsV3,
    refund: PreparedWitnessedClaim,
}

impl PrepareNativeXmrRefundV3Result {
    /// Creates one exact refund reservation.
    ///
    /// # Errors
    ///
    /// Rejects a message hash that differs from the activated agreement.
    pub fn new(
        context: MessageContext,
        terms: XmrNativeEscrowTermsV3,
        refund: PreparedWitnessedClaim,
    ) -> Result<Self, ProtocolValueError> {
        ensure_fact(
            refund.message_hash == terms.0.refund_message_hash,
            "prepared refund message hash",
        )?;
        Ok(Self {
            context,
            terms,
            refund,
        })
    }
}

impl<'de> Deserialize<'de> for PrepareNativeXmrRefundV3Result {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PrepareNativeXmrRefundV3ResultWire::deserialize(deserializer)?;
        Self::new(wire.context, wire.terms, wire.refund).map_err(D::Error::custom)
    }
}

/// Completes the reserved XMR claim with the exact aggregate signature.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[must_use]
pub struct CompleteNativeXmrClaimV3Request {
    /// Version, run, request, and dedicated sidecar-role binding.
    pub context: MessageContext,
    /// Exact expected runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Complete standalone agreement and guest binding.
    pub terms: XmrNativeEscrowTermsV3,
    /// Previously reserved canonical claim transcript.
    pub claim: PreparedWitnessedClaim,
    /// Aggregate BIP340 signature for the exact claim message.
    pub aggregate_signature: AggregateBip340Signature,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompleteNativeXmrClaimV3RequestWire {
    context: MessageContext,
    runtime: RuntimeDescriptor,
    terms: XmrNativeEscrowTermsV3,
    claim: PreparedWitnessedClaim,
    aggregate_signature: AggregateBip340Signature,
}

impl CompleteNativeXmrClaimV3Request {
    /// Validates and creates one claim completion request.
    ///
    /// # Errors
    ///
    /// Rejects a reservation for any other message hash.
    pub fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: XmrNativeEscrowTermsV3,
        claim: PreparedWitnessedClaim,
        aggregate_signature: AggregateBip340Signature,
    ) -> Result<Self, ProtocolValueError> {
        ensure_fact(
            claim.message_hash == terms.0.claim_message_hash,
            "completed claim message hash",
        )?;
        Ok(Self {
            context,
            runtime,
            terms,
            claim,
            aggregate_signature,
        })
    }
}

impl<'de> Deserialize<'de> for CompleteNativeXmrClaimV3Request {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CompleteNativeXmrClaimV3RequestWire::deserialize(deserializer)?;
        Self::new(
            wire.context,
            wire.runtime,
            wire.terms,
            wire.claim,
            wire.aggregate_signature,
        )
        .map_err(D::Error::custom)
    }
}

/// Completes the reserved XMR refund with the exact aggregate signature.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[must_use]
pub struct CompleteNativeXmrRefundV3Request {
    /// Version, run, request, and dedicated sidecar-role binding.
    pub context: MessageContext,
    /// Exact expected runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Complete standalone agreement and guest binding.
    pub terms: XmrNativeEscrowTermsV3,
    /// Previously reserved canonical refund transcript.
    pub refund: PreparedWitnessedClaim,
    /// Aggregate BIP340 signature for the exact refund message.
    pub aggregate_signature: AggregateBip340Signature,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompleteNativeXmrRefundV3RequestWire {
    context: MessageContext,
    runtime: RuntimeDescriptor,
    terms: XmrNativeEscrowTermsV3,
    refund: PreparedWitnessedClaim,
    aggregate_signature: AggregateBip340Signature,
}

impl CompleteNativeXmrRefundV3Request {
    /// Validates and creates one refund completion request.
    ///
    /// # Errors
    ///
    /// Rejects a reservation for any other message hash.
    pub fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: XmrNativeEscrowTermsV3,
        refund: PreparedWitnessedClaim,
        aggregate_signature: AggregateBip340Signature,
    ) -> Result<Self, ProtocolValueError> {
        ensure_fact(
            refund.message_hash == terms.0.refund_message_hash,
            "completed refund message hash",
        )?;
        Ok(Self {
            context,
            runtime,
            terms,
            refund,
            aggregate_signature,
        })
    }
}

impl<'de> Deserialize<'de> for CompleteNativeXmrRefundV3Request {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CompleteNativeXmrRefundV3RequestWire::deserialize(deserializer)?;
        Self::new(
            wire.context,
            wire.runtime,
            wire.terms,
            wire.refund,
            wire.aggregate_signature,
        )
        .map_err(D::Error::custom)
    }
}

macro_rules! transaction_result {
    ($name:ident, $field:ident, $doc:literal, $field_doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
        #[serde(deny_unknown_fields)]
        #[must_use]
        pub struct $name {
            /// Echoed request context.
            pub context: MessageContext,
            /// Echoed immutable standalone terms.
            pub terms: XmrNativeEscrowTermsV3,
            #[doc = $field_doc]
            pub $field: PreparedTransaction,
        }

        impl $name {
            /// Creates one exact prepared transaction result.
            pub const fn new(
                context: MessageContext,
                terms: XmrNativeEscrowTermsV3,
                $field: PreparedTransaction,
            ) -> Self {
                Self {
                    context,
                    terms,
                    $field,
                }
            }
        }
    };
}

transaction_result!(
    CompleteNativeXmrClaimV3Result,
    claim,
    "Exact completed XMR claim transaction.",
    "Official completed claim transaction."
);
transaction_result!(
    CompleteNativeXmrRefundV3Result,
    refund,
    "Exact completed XMR refund transaction.",
    "Official completed refund transaction."
);
transaction_result!(
    PrepareNativeXmrPunishV3Result,
    punish,
    "Exact unilateral XMR punish transaction.",
    "Official punish transaction."
);

/// Exact XMR-native initialization and funding prepared under consecutive nonces.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareNativeXmrEscrowV3Result {
    /// Echoed request context.
    pub context: MessageContext,
    /// Echoed immutable standalone terms.
    pub terms: XmrNativeEscrowTermsV3,
    /// Official XMR-native initialization transaction.
    pub initialization: PreparedTransaction,
    /// Official native funding transaction.
    pub funding: PreparedTransaction,
}

impl PrepareNativeXmrEscrowV3Result {
    /// Creates one exact consecutive initialization and funding result.
    pub const fn new(
        context: MessageContext,
        terms: XmrNativeEscrowTermsV3,
        initialization: PreparedTransaction,
        funding: PreparedTransaction,
    ) -> Self {
        Self {
            context,
            terms,
            initialization,
            funding,
        }
    }
}

/// Requests publication of the committed Taker claim partial.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareNativeXmrClaimAuthorizationV3Request {
    /// Version, run, request, and dedicated sidecar-role binding.
    pub context: MessageContext,
    /// Exact expected runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Complete standalone agreement and guest binding.
    pub terms: XmrNativeEscrowTermsV3,
    /// Sensitive claim partial checked by the guest commitment.
    pub claim_partial: XmrClaimPartialV3,
}

impl PrepareNativeXmrClaimAuthorizationV3Request {
    /// Creates one exact claim-partial publication request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: XmrNativeEscrowTermsV3,
        claim_partial: XmrClaimPartialV3,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            claim_partial,
        }
    }
}

transaction_result!(
    PrepareNativeXmrClaimAuthorizationV3Result,
    authorization,
    "Exact transaction publishing the committed Taker claim partial.",
    "Official claim authorization transaction."
);
/// Submits one exact, durably owned XMR claim authorization transaction.
///
/// This dedicated request deliberately does not widen generic transaction
/// submission to tag 14. The release service must present the exact prepared
/// authorization together with its complete runtime and agreement binding.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct SubmitNativeXmrClaimAuthorizationV3Request {
    /// Version, run, request, and dedicated Taker sidecar-role binding.
    pub context: MessageContext,
    /// Exact expected runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Complete standalone agreement and guest binding.
    pub terms: XmrNativeEscrowTermsV3,
    /// Exact authorization bytes previously reserved by the durable planner.
    pub authorization: PreparedTransaction,
}

impl SubmitNativeXmrClaimAuthorizationV3Request {
    /// Creates one exact release-authority-gated authorization submission.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: XmrNativeEscrowTermsV3,
        authorization: PreparedTransaction,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            authorization,
        }
    }
}

/// Reports the one-attempt admission result for an exact XMR authorization.
///
/// Admission is not finality. Callers must separately prove the exact
/// authorization in stable finalized history before enabling the claim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct SubmitNativeXmrClaimAuthorizationV3Result {
    /// Echoed request context.
    pub context: MessageContext,
    /// Echoed immutable standalone terms.
    pub terms: XmrNativeEscrowTermsV3,
    /// Exact canonical transaction ID derived from the submitted bytes.
    pub authorization_transaction_id: TransactionId,
    /// Whether the exact bytes were accepted or already canonical.
    pub outcome: SubmissionOutcome,
}

impl SubmitNativeXmrClaimAuthorizationV3Result {
    /// Creates one exact authorization admission result.
    pub const fn new(
        context: MessageContext,
        terms: XmrNativeEscrowTermsV3,
        authorization_transaction_id: TransactionId,
        outcome: SubmissionOutcome,
    ) -> Self {
        Self {
            context,
            terms,
            authorization_transaction_id,
            outcome,
        }
    }
}

/// One guest-visible XMR-native escrow effect.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[must_use]
pub enum XmrNativeEffectV3 {
    /// Initialize version-3 metadata and native custody.
    Initialize,
    /// Fund the initialized native custody account.
    Fund,
    /// Publish the committed Taker claim partial.
    AuthorizeClaim,
    /// Spend custody to the Maker with the claim aggregate witness.
    Claim,
    /// Spend custody to the Taker with the refund aggregate witness.
    Refund,
    /// Spend custody to the Maker after the punish boundary.
    Punish,
}

/// Version-3 guest metadata state at an exact finalized block.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[must_use]
pub enum XmrNativeEscrowStateV3 {
    /// Initialized with zero custody balance.
    Empty,
    /// Funded before claim-partial publication.
    Funded,
    /// Funded after the committed claim partial was published.
    ClaimAuthorized,
    /// Custody was paid to the Maker by claim or punish.
    Claimed,
    /// Custody was paid back to the Taker.
    Refunded,
}

/// Exact decoded XMR instruction fields and official message hash.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct XmrNativeInstructionFactsV3 {
    /// Semantic guest effect decoded from the instruction tag.
    pub effect: XmrNativeEffectV3,
    /// Exact escrow program invoked by the transaction.
    pub program_id: Hex32,
    /// Exact guest account order for this effect.
    pub ordered_account_ids: AccountIds,
    /// Exact swap identifier encoded by the instruction.
    pub swap_id: Hex32,
    /// Official hash of the exact unsigned message.
    pub message_hash: Hex32,
    /// Published Taker partial, present only for claim authorization.
    pub published_claim_partial: Option<Hex32>,
}

impl XmrNativeInstructionFactsV3 {
    /// Validates and creates exact decoded instruction facts.
    ///
    /// # Errors
    ///
    /// Rejects zero identities, empty account order, or a claim partial on any
    /// effect other than authorization.
    pub fn new(
        effect: XmrNativeEffectV3,
        program_id: Hex32,
        ordered_account_ids: AccountIds,
        swap_id: Hex32,
        message_hash: Hex32,
        published_claim_partial: Option<Hex32>,
    ) -> Result<Self, ProtocolValueError> {
        validate_nonzero(&[
            ("instruction program id", program_id),
            ("instruction swap id", swap_id),
            ("instruction message hash", message_hash),
        ])?;
        ensure_fact(
            !ordered_account_ids.as_slice().is_empty(),
            "empty instruction accounts",
        )?;
        match (effect, published_claim_partial) {
            (XmrNativeEffectV3::AuthorizeClaim, Some(partial)) => {
                validate_nonzero(&[("published claim partial", partial)])?;
            }
            (XmrNativeEffectV3::AuthorizeClaim, None) => {
                return Err(ProtocolValueError::XmrFactsMismatch(
                    "missing published claim partial",
                ));
            }
            (_, Some(_)) => {
                return Err(ProtocolValueError::XmrFactsMismatch(
                    "unexpected published claim partial",
                ));
            }
            (_, None) => {}
        }
        Ok(Self {
            effect,
            program_id,
            ordered_account_ids,
            swap_id,
            message_hash,
            published_claim_partial,
        })
    }
}

/// Full version-3 native metadata decoded at an exact finalized block.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct XmrNativeEscrowMetadataFactsV3 {
    /// Metadata account identity.
    pub account_id: Hex32,
    /// Program owning the metadata account.
    pub owner_program_id: Hex32,
    /// Exact guest metadata version.
    pub version: u16,
    /// Swap identifier.
    pub swap_id: Hex32,
    /// Activated agreement commitment stored by the guest.
    pub activation_commitment: Hex32,
    /// Claim aggregate x-only public key.
    pub claim_aggregate_x_only_public_key: Hex32,
    /// Claim aggregate authority account.
    pub claim_authority_account_id: Hex32,
    /// Refund aggregate x-only public key.
    pub refund_aggregate_x_only_public_key: Hex32,
    /// Refund aggregate authority account.
    pub refund_authority_account_id: Hex32,
    /// Maker DLEQ transcript commitment.
    pub maker_dleq_transcript_commitment: Hex32,
    /// Taker DLEQ transcript commitment.
    pub taker_dleq_transcript_commitment: Hex32,
    /// Durable claim-partial context binding.
    pub claim_partial_context_binding: Hex32,
    /// Committed Taker claim-partial digest.
    pub claim_partial_commitment: Hex32,
    /// Taker depositor account.
    pub depositor_account_id: Hex32,
    /// Native depositor asset account, equal to the depositor.
    pub depositor_asset_account_id: Hex32,
    /// Maker claimant account.
    pub claimant_account_id: Hex32,
    /// Native claimant asset account, equal to the claimant.
    pub claimant_asset_account_id: Hex32,
    /// Native custody account.
    pub custody_account_id: Hex32,
    /// Native asset program.
    pub asset_program_id: Hex32,
    /// Native custody program.
    pub custody_program_id: Hex32,
    /// Native asset definition sentinel, which must be all zeroes.
    pub asset_definition: Hex32,
    /// Full-width native amount.
    pub amount: NativeAmount,
    /// Claim/refund boundary.
    pub refund_at_ms: u64,
    /// Refund/punish boundary stored in the dual-adaptor authority.
    pub punish_at_ms: u64,
    /// Exact metadata state.
    pub state: XmrNativeEscrowStateV3,
}

impl XmrNativeEscrowMetadataFactsV3 {
    /// Creates the exact metadata expected from standalone terms in one state.
    pub const fn from_terms(terms: XmrNativeEscrowTermsV3, state: XmrNativeEscrowStateV3) -> Self {
        Self {
            account_id: terms.0.metadata_account_id,
            owner_program_id: terms.0.escrow_program_id,
            version: XMR_NATIVE_ESCROW_TERMS_VERSION,
            swap_id: terms.0.swap_id,
            activation_commitment: terms.0.activation_commitment,
            claim_aggregate_x_only_public_key: terms.0.claim_aggregate_x_only_public_key,
            claim_authority_account_id: terms.0.claim_authority_account_id,
            refund_aggregate_x_only_public_key: terms.0.refund_aggregate_x_only_public_key,
            refund_authority_account_id: terms.0.refund_authority_account_id,
            maker_dleq_transcript_commitment: terms.0.maker_dleq_transcript_commitment,
            taker_dleq_transcript_commitment: terms.0.taker_dleq_transcript_commitment,
            claim_partial_context_binding: terms.0.claim_partial_context_binding,
            claim_partial_commitment: terms.0.claim_partial_commitment,
            depositor_account_id: terms.0.depositor_account_id,
            depositor_asset_account_id: terms.0.depositor_account_id,
            claimant_account_id: terms.0.claimant_account_id,
            claimant_asset_account_id: terms.0.claimant_account_id,
            custody_account_id: terms.0.custody_account_id,
            asset_program_id: terms.0.authenticated_transfer_program_id,
            custody_program_id: terms.0.authenticated_transfer_program_id,
            asset_definition: Hex32::from_bytes([0; 32]),
            amount: terms.0.amount,
            refund_at_ms: terms.0.refund_at_ms,
            punish_at_ms: terms.0.punish_at_ms,
            state,
        }
    }
}

/// Complete exact facts for one finalized XMR-native effect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct FinalizedNativeXmrEffectFactsV3 {
    /// Canonical exact transaction, signer order, and chain position.
    pub transaction: ObservedTransactionFacts,
    /// Exact decoded instruction and account order.
    pub instruction: XmrNativeInstructionFactsV3,
    /// Aggregate witness for claim or refund, absent for other effects.
    pub aggregate_signature: Option<AggregateBip340Signature>,
    /// Explicit identity of the containing finalized block.
    pub containing_block: FinalizedBlockIdentity,
    /// Full metadata decoded at that exact block.
    pub metadata: XmrNativeEscrowMetadataFactsV3,
    /// Native custody state read at that exact block.
    pub custody: NativeCustodyFacts,
}

impl FinalizedNativeXmrEffectFactsV3 {
    /// Creates one primitive finalized evidence bundle.
    pub const fn new(
        transaction: ObservedTransactionFacts,
        instruction: XmrNativeInstructionFactsV3,
        aggregate_signature: Option<AggregateBip340Signature>,
        containing_block: FinalizedBlockIdentity,
        metadata: XmrNativeEscrowMetadataFactsV3,
        custody: NativeCustodyFacts,
    ) -> Self {
        Self {
            transaction,
            instruction,
            aggregate_signature,
            containing_block,
            metadata,
            custody,
        }
    }
}

/// Selects exact persisted bytes or bounded terms-based discovery.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
#[must_use]
pub enum FinalizedNativeXmrTransactionTargetV3 {
    /// Require one exact persisted transaction.
    Exact {
        /// Exact official transaction persisted before any send attempt.
        transaction: PreparedTransaction,
    },
    /// Discover the unique canonical transaction from standalone terms.
    DiscoverByTerms {},
}

impl FinalizedNativeXmrTransactionTargetV3 {
    /// Creates one exact persisted-transaction target.
    pub const fn exact(transaction: PreparedTransaction) -> Self {
        Self::Exact { transaction }
    }
}

/// Why no complete stable finalized XMR scan was available.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[must_use]
pub enum FinalizedNativeXmrUnavailableReasonV3 {
    /// The canonical tip changed across the bracketed scan.
    MovingTip,
    /// The node could not expose the required finalized boundary.
    FinalityUnavailable,
    /// Required historical blocks or account state were unavailable.
    HistoryUnavailable,
    /// More than one canonical candidate matched the terms.
    ConflictingMatches,
}

/// Conservative finalized classification of one XMR-native effect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[must_use]
pub enum FinalizedNativeXmrScanOutcomeV3 {
    /// One exact validated effect was found.
    Found {
        /// Stable finalized clock covering the complete scan.
        finalized_clock: ChainClock,
        /// Exact bounded range completely scanned.
        scanned_window: DiscoveryWindow,
        /// Complete exact finalized facts.
        facts: Box<FinalizedNativeXmrEffectFactsV3>,
    },
    /// Complete stable scan and predecessor state proved absence.
    Absent {
        /// Stable finalized clock covering the complete scan.
        finalized_clock: ChainClock,
        /// Exact bounded range completely scanned.
        scanned_window: DiscoveryWindow,
    },
    /// Stable finalized absence cannot exclude pending or unknown presence.
    Uncertain {
        /// Stable finalized clock covering the complete scan.
        finalized_clock: ChainClock,
        /// Exact bounded range completely scanned.
        scanned_window: DiscoveryWindow,
    },
    /// No complete stable finalized scan was available.
    Unavailable {
        /// Typed reason affirmative classification was impossible.
        reason: FinalizedNativeXmrUnavailableReasonV3,
    },
}

impl FinalizedNativeXmrScanOutcomeV3 {
    /// Wraps one found finalized evidence bundle.
    pub fn found(
        finalized_clock: ChainClock,
        scanned_window: DiscoveryWindow,
        facts: FinalizedNativeXmrEffectFactsV3,
    ) -> Self {
        Self::Found {
            finalized_clock,
            scanned_window,
            facts: Box::new(facts),
        }
    }

    /// Records affirmative absence after a complete stable scan.
    pub const fn absent(finalized_clock: ChainClock, scanned_window: DiscoveryWindow) -> Self {
        Self::Absent {
            finalized_clock,
            scanned_window,
        }
    }

    /// Records a stable scan that cannot prove current absence.
    pub const fn uncertain(finalized_clock: ChainClock, scanned_window: DiscoveryWindow) -> Self {
        Self::Uncertain {
            finalized_clock,
            scanned_window,
        }
    }

    /// Records that complete stable coverage was unavailable.
    pub const fn unavailable(reason: FinalizedNativeXmrUnavailableReasonV3) -> Self {
        Self::Unavailable { reason }
    }
}

/// Requests conservative finalized classification of one XMR-native effect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ClassifyFinalizedNativeXmrEffectV3Request {
    /// Version, run, request, and dedicated sidecar-role binding.
    pub context: MessageContext,
    /// Exact expected runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Complete standalone agreement and guest binding.
    pub terms: XmrNativeEscrowTermsV3,
    /// Exact semantic effect to classify.
    pub effect: XmrNativeEffectV3,
    /// Exact bytes or terms-based discovery target.
    pub target: FinalizedNativeXmrTransactionTargetV3,
    /// Bounded canonical scan range.
    pub window: DiscoveryWindow,
}

impl ClassifyFinalizedNativeXmrEffectV3Request {
    /// Creates one bounded finalized classification request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: XmrNativeEscrowTermsV3,
        effect: XmrNativeEffectV3,
        target: FinalizedNativeXmrTransactionTargetV3,
        window: DiscoveryWindow,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            effect,
            target,
            window,
        }
    }
}

/// Returns one conservative finalized XMR-native effect classification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[must_use]
pub struct ClassifyFinalizedNativeXmrEffectV3Result {
    /// Echoed request context.
    pub context: MessageContext,
    /// Echoed standalone agreement and guest binding.
    pub terms: XmrNativeEscrowTermsV3,
    /// Echoed semantic effect.
    pub effect: XmrNativeEffectV3,
    /// Echoed exact or discovery target.
    pub target: FinalizedNativeXmrTransactionTargetV3,
    /// Conservative finalized outcome.
    pub outcome: FinalizedNativeXmrScanOutcomeV3,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClassifyFinalizedNativeXmrEffectV3ResultWire {
    context: MessageContext,
    terms: XmrNativeEscrowTermsV3,
    effect: XmrNativeEffectV3,
    target: FinalizedNativeXmrTransactionTargetV3,
    outcome: FinalizedNativeXmrScanOutcomeV3,
}

impl ClassifyFinalizedNativeXmrEffectV3Result {
    /// Validates and creates one conservative finalized result.
    ///
    /// # Errors
    ///
    /// Rejects found evidence that contradicts the target, exact guest fields,
    /// signer/account order, containing block, metadata state, or custody.
    pub fn new(
        context: MessageContext,
        terms: XmrNativeEscrowTermsV3,
        effect: XmrNativeEffectV3,
        target: FinalizedNativeXmrTransactionTargetV3,
        outcome: FinalizedNativeXmrScanOutcomeV3,
    ) -> Result<Self, ProtocolValueError> {
        if let FinalizedNativeXmrScanOutcomeV3::Found {
            finalized_clock,
            scanned_window,
            facts,
        } = &outcome
        {
            validate_finalized_facts(
                &terms,
                effect,
                &target,
                *finalized_clock,
                *scanned_window,
                facts,
            )?;
        } else if let FinalizedNativeXmrScanOutcomeV3::Absent {
            finalized_clock,
            scanned_window,
        }
        | FinalizedNativeXmrScanOutcomeV3::Uncertain {
            finalized_clock,
            scanned_window,
        } = &outcome
        {
            validate_scan_coverage(*finalized_clock, *scanned_window)?;
        }
        Ok(Self {
            context,
            terms,
            effect,
            target,
            outcome,
        })
    }
}

impl<'de> Deserialize<'de> for ClassifyFinalizedNativeXmrEffectV3Result {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ClassifyFinalizedNativeXmrEffectV3ResultWire::deserialize(deserializer)?;
        Self::new(
            wire.context,
            wire.terms,
            wire.effect,
            wire.target,
            wire.outcome,
        )
        .map_err(D::Error::custom)
    }
}

fn validate_finalized_placement(
    target: &FinalizedNativeXmrTransactionTargetV3,
    finalized_clock: ChainClock,
    scanned_window: DiscoveryWindow,
    facts: &FinalizedNativeXmrEffectFactsV3,
) -> Result<(), ProtocolValueError> {
    validate_scan_coverage(finalized_clock, scanned_window)?;
    let end_height =
        scanned_window.start_height() + u64::from(scanned_window.max_blocks().saturating_sub(1));
    ensure_fact(facts.transaction.is_public, "public transaction")?;
    ensure_fact(
        (scanned_window.start_height()..=end_height).contains(&facts.transaction.position.height),
        "transaction scan window",
    )?;
    ensure_fact(
        facts.transaction.position.block_hash == facts.containing_block.block_hash,
        "containing block hash",
    )?;
    ensure_fact(
        facts.transaction.position.height == facts.containing_block.block_id,
        "containing block id",
    )?;
    ensure_fact(
        facts.transaction.position.height <= finalized_clock.height
            && facts.containing_block.timestamp_ms <= finalized_clock.timestamp_ms,
        "finalized transaction placement",
    )?;
    if let FinalizedNativeXmrTransactionTargetV3::Exact {
        transaction: expected,
    } = target
    {
        ensure_fact(
            facts.transaction.transaction_id == expected.transaction_id,
            "exact transaction id",
        )?;
        ensure_fact(
            facts.transaction.exact_bytes == expected.exact_bytes,
            "exact transaction bytes",
        )?;
    }
    Ok(())
}

fn validate_effect_accounts(
    terms: &XmrNativeEscrowTermsV3,
    effect: XmrNativeEffectV3,
    facts: &FinalizedNativeXmrEffectFactsV3,
) -> Result<(), ProtocolValueError> {
    let expected_accounts: Vec<Hex32> = match effect {
        XmrNativeEffectV3::Initialize => vec![
            terms.0.metadata_account_id,
            terms.0.custody_account_id,
            terms.0.depositor_account_id,
            terms.0.claimant_account_id,
            terms.0.claim_authority_account_id,
            terms.0.refund_authority_account_id,
        ],
        XmrNativeEffectV3::Fund => vec![
            terms.0.metadata_account_id,
            terms.0.custody_account_id,
            terms.0.depositor_account_id,
        ],
        XmrNativeEffectV3::AuthorizeClaim => {
            vec![terms.0.metadata_account_id, terms.0.depositor_account_id]
        }
        XmrNativeEffectV3::Claim => vec![
            terms.0.metadata_account_id,
            terms.0.custody_account_id,
            terms.0.claimant_account_id,
            terms.0.claim_authority_account_id,
        ],
        XmrNativeEffectV3::Refund => vec![
            terms.0.metadata_account_id,
            terms.0.custody_account_id,
            terms.0.depositor_account_id,
            terms.0.refund_authority_account_id,
        ],
        XmrNativeEffectV3::Punish => vec![
            terms.0.metadata_account_id,
            terms.0.custody_account_id,
            terms.0.claimant_account_id,
        ],
    };
    ensure_fact(
        facts.instruction.ordered_account_ids.as_slice() == expected_accounts,
        "instruction account order",
    )?;
    let expected_signer = match effect {
        XmrNativeEffectV3::Initialize
        | XmrNativeEffectV3::Fund
        | XmrNativeEffectV3::AuthorizeClaim => terms.0.depositor_account_id,
        XmrNativeEffectV3::Claim => terms.0.claim_authority_account_id,
        XmrNativeEffectV3::Refund => terms.0.refund_authority_account_id,
        XmrNativeEffectV3::Punish => terms.0.claimant_account_id,
    };
    ensure_fact(
        facts.transaction.signer_account_ids.as_slice() == [expected_signer],
        "transaction signer",
    )
}

fn validate_finalized_facts(
    terms: &XmrNativeEscrowTermsV3,
    effect: XmrNativeEffectV3,
    target: &FinalizedNativeXmrTransactionTargetV3,
    finalized_clock: ChainClock,
    scanned_window: DiscoveryWindow,
    facts: &FinalizedNativeXmrEffectFactsV3,
) -> Result<(), ProtocolValueError> {
    validate_finalized_placement(target, finalized_clock, scanned_window, facts)?;
    if effect == XmrNativeEffectV3::Refund {
        ensure_fact(
            (terms.0.refund_at_ms..terms.0.punish_at_ms)
                .contains(&facts.containing_block.timestamp_ms),
            "refund timestamp",
        )?;
    }
    if effect == XmrNativeEffectV3::Punish {
        ensure_fact(
            facts.containing_block.timestamp_ms >= terms.0.punish_at_ms,
            "punish timestamp",
        )?;
    }

    ensure_fact(facts.instruction.effect == effect, "instruction effect")?;
    validate_nonzero(&[
        ("instruction program id", facts.instruction.program_id),
        ("instruction swap id", facts.instruction.swap_id),
        ("instruction message hash", facts.instruction.message_hash),
    ])?;
    ensure_fact(
        facts.instruction.program_id == terms.0.escrow_program_id,
        "instruction program",
    )?;
    ensure_fact(
        facts.instruction.swap_id == terms.0.swap_id,
        "instruction swap id",
    )?;

    validate_effect_accounts(terms, effect, facts)?;

    match effect {
        XmrNativeEffectV3::AuthorizeClaim => {
            let partial = facts.instruction.published_claim_partial.ok_or(
                ProtocolValueError::XmrFactsMismatch("missing published claim partial"),
            )?;
            validate_nonzero(&[("published claim partial", partial)])?;
        }
        _ => ensure_fact(
            facts.instruction.published_claim_partial.is_none(),
            "unexpected published claim partial",
        )?,
    }

    let expected_message_hash = match effect {
        XmrNativeEffectV3::Claim => Some(terms.0.claim_message_hash),
        XmrNativeEffectV3::Refund => Some(terms.0.refund_message_hash),
        XmrNativeEffectV3::Punish => Some(terms.0.punish_message_hash),
        _ => None,
    };
    if let Some(expected) = expected_message_hash {
        ensure_fact(
            facts.instruction.message_hash == expected,
            "agreement message hash",
        )?;
    }

    match effect {
        XmrNativeEffectV3::Claim | XmrNativeEffectV3::Refund => {
            ensure_fact(
                facts.aggregate_signature.is_some(),
                "missing aggregate signature",
            )?;
        }
        _ => ensure_fact(
            facts.aggregate_signature.is_none(),
            "unexpected aggregate signature",
        )?,
    }

    let (expected_state, expected_balance) = match effect {
        XmrNativeEffectV3::Initialize => (XmrNativeEscrowStateV3::Empty, 0),
        XmrNativeEffectV3::Fund => (XmrNativeEscrowStateV3::Funded, terms.0.amount.as_u128()),
        XmrNativeEffectV3::AuthorizeClaim => (
            XmrNativeEscrowStateV3::ClaimAuthorized,
            terms.0.amount.as_u128(),
        ),
        XmrNativeEffectV3::Claim | XmrNativeEffectV3::Punish => {
            (XmrNativeEscrowStateV3::Claimed, 0)
        }
        XmrNativeEffectV3::Refund => (XmrNativeEscrowStateV3::Refunded, 0),
    };
    ensure_fact(
        facts.metadata == XmrNativeEscrowMetadataFactsV3::from_terms(*terms, expected_state),
        "version-3 metadata",
    )?;
    ensure_fact(
        facts.custody
            == NativeCustodyFacts::new(
                terms.0.custody_account_id,
                terms.0.authenticated_transfer_program_id,
                expected_balance,
            ),
        "native custody",
    )
}

fn validate_scan_coverage(
    finalized_clock: ChainClock,
    scanned_window: DiscoveryWindow,
) -> Result<(), ProtocolValueError> {
    let end_height =
        scanned_window.start_height() + u64::from(scanned_window.max_blocks().saturating_sub(1));
    ensure_fact(
        finalized_clock.height >= end_height,
        "finalized window coverage",
    )
}

fn validate_terms_input(input: &XmrNativeEscrowTermsV3Input) -> Result<(), ProtocolValueError> {
    if input.depositor != Participant::Taker || input.claimant != Participant::Maker {
        return Err(ProtocolValueError::InvalidXmrRoleMapping);
    }
    if input.amount == 0 {
        return Err(ProtocolValueError::ZeroEscrowAmount);
    }
    if input.refund_at_ms == 0 || input.punish_at_ms <= input.refund_at_ms {
        return Err(ProtocolValueError::InvalidXmrWindows);
    }
    validate_distinct_nonzero(&[
        ("escrow program id", input.escrow_program_id),
        (
            "authenticated-transfer program id",
            input.authenticated_transfer_program_id,
        ),
        ("metadata account id", input.metadata_account_id),
        ("custody account id", input.custody_account_id),
        ("depositor account id", input.depositor_account_id),
        ("claimant account id", input.claimant_account_id),
        (
            "claim authority account id",
            input.claim_authority_account_id,
        ),
        (
            "refund authority account id",
            input.refund_authority_account_id,
        ),
    ])?;
    validate_nonzero(&[
        ("swap id", input.swap_id),
        ("activation commitment", input.activation_commitment),
        (
            "claim aggregate x-only public key",
            input.claim_aggregate_x_only_public_key,
        ),
        (
            "refund aggregate x-only public key",
            input.refund_aggregate_x_only_public_key,
        ),
        (
            "maker DLEQ transcript commitment",
            input.maker_dleq_transcript_commitment,
        ),
        (
            "taker DLEQ transcript commitment",
            input.taker_dleq_transcript_commitment,
        ),
        (
            "claim partial context binding",
            input.claim_partial_context_binding,
        ),
        ("claim partial commitment", input.claim_partial_commitment),
        ("claim message hash", input.claim_message_hash),
        ("refund message hash", input.refund_message_hash),
        ("punish message hash", input.punish_message_hash),
    ])?;
    ensure_distinct(
        (
            "claim aggregate x-only public key",
            input.claim_aggregate_x_only_public_key,
        ),
        (
            "refund aggregate x-only public key",
            input.refund_aggregate_x_only_public_key,
        ),
    )?;
    ensure_distinct(
        (
            "maker DLEQ transcript commitment",
            input.maker_dleq_transcript_commitment,
        ),
        (
            "taker DLEQ transcript commitment",
            input.taker_dleq_transcript_commitment,
        ),
    )?;
    validate_distinct_nonzero(&[
        ("claim message hash", input.claim_message_hash),
        ("refund message hash", input.refund_message_hash),
        ("punish message hash", input.punish_message_hash),
    ])
}

fn terms_wire_from_input(input: &XmrNativeEscrowTermsV3Input) -> XmrNativeEscrowTermsV3Wire {
    XmrNativeEscrowTermsV3Wire {
        version: XMR_NATIVE_ESCROW_TERMS_VERSION,
        swap_id: input.swap_id,
        activation_commitment: input.activation_commitment,
        escrow_program_id: input.escrow_program_id,
        authenticated_transfer_program_id: input.authenticated_transfer_program_id,
        metadata_account_id: input.metadata_account_id,
        custody_account_id: input.custody_account_id,
        depositor: input.depositor,
        depositor_account_id: input.depositor_account_id,
        claimant: input.claimant,
        claimant_account_id: input.claimant_account_id,
        claim_aggregate_x_only_public_key: input.claim_aggregate_x_only_public_key,
        claim_authority_account_id: input.claim_authority_account_id,
        refund_aggregate_x_only_public_key: input.refund_aggregate_x_only_public_key,
        refund_authority_account_id: input.refund_authority_account_id,
        maker_dleq_transcript_commitment: input.maker_dleq_transcript_commitment,
        taker_dleq_transcript_commitment: input.taker_dleq_transcript_commitment,
        claim_partial_context_binding: input.claim_partial_context_binding,
        claim_partial_commitment: input.claim_partial_commitment,
        amount: NativeAmount::new(input.amount),
        refund_at_ms: input.refund_at_ms,
        punish_at_ms: input.punish_at_ms,
        claim_message_hash: input.claim_message_hash,
        refund_message_hash: input.refund_message_hash,
        punish_message_hash: input.punish_message_hash,
    }
}

fn validate_nonzero(values: &[(&'static str, Hex32)]) -> Result<(), ProtocolValueError> {
    for (name, value) in values {
        if value.as_bytes() == &[0; 32] {
            return Err(ProtocolValueError::ZeroXmrValue(name));
        }
    }
    Ok(())
}

fn ensure_fact(condition: bool, field: &'static str) -> Result<(), ProtocolValueError> {
    if condition {
        Ok(())
    } else {
        Err(ProtocolValueError::XmrFactsMismatch(field))
    }
}

fn validate_distinct_nonzero(values: &[(&'static str, Hex32)]) -> Result<(), ProtocolValueError> {
    validate_nonzero(values)?;
    for (index, left) in values.iter().enumerate() {
        for right in values.iter().skip(index + 1) {
            ensure_distinct(*left, *right)?;
        }
    }
    Ok(())
}

fn ensure_distinct(
    left: (&'static str, Hex32),
    right: (&'static str, Hex32),
) -> Result<(), ProtocolValueError> {
    if left.1 == right.1 {
        Err(ProtocolValueError::AliasedXmrValues(left.0, right.0))
    } else {
        Ok(())
    }
}
