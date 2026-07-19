//! Capability-bearing finalized LEZ first-lock evidence for M4 XMR swaps.
//!
//! Primitive v3 protocol values are intentionally constructible for wire
//! decoding and tests. They are not actor authority by themselves. This module
//! mints a private-field evidence value only after the concrete authenticated
//! [`BridgeClient`] has classified the exact retained funding transaction as a
//! canonical finalized `Fund` effect under terms derived from Stage B.

use lez_bridge_client::{BridgeClient, BridgeClientError};
use lez_bridge_protocol::{
    ChainClock, ClassifyFinalizedNativeXmrEffectV3Request,
    ClassifyFinalizedNativeXmrEffectV3Result, DiscoveryWindow, FinalizedNativeXmrEffectFactsV3,
    FinalizedNativeXmrScanOutcomeV3, FinalizedNativeXmrTransactionTargetV3,
    FinalizedNativeXmrUnavailableReasonV3, Hex32, MessageContext, Participant as BridgeParticipant,
    PreparedTransaction, ProtocolValueError, RequestId, RunId, RuntimeDescriptor,
    XmrNativeEffectV3, XmrNativeEscrowTermsV3, XmrNativeEscrowTermsV3Input,
};
use lez_swap_core::Participant;
use lez_xmr_swap_sdk::{XmrActivatedAgreementV1, XmrAgreementV1, XmrAgreementV1Error};
use thiserror::Error;

use crate::LezBridgeAdapter;

/// Exact Stage-B-to-bridge binding for one native LEZ/XMR escrow.
///
/// The terms can only be created here from a fully validated Stage-B
/// activation and its matching Stage-A agreement. A Stage-A record alone
/// cannot produce this binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrLezBridgeBindingV3 {
    terms: XmrNativeEscrowTermsV3,
}

impl XmrLezBridgeBindingV3 {
    /// Derives every guest and bridge field from a validated Stage-B activation.
    ///
    /// # Errors
    ///
    /// Rejects a Stage-B record for another base agreement or any conversion
    /// that violates the standalone v3 protocol invariants.
    pub fn new(
        agreement: &XmrAgreementV1,
        activation: &XmrActivatedAgreementV1,
    ) -> Result<Self, XmrLezBridgeBindingV3Error> {
        let plan = activation
            .lez_initialize_plan(agreement)
            .map_err(XmrLezBridgeBindingV3Error::StageB)?;
        let escrow_program_id = program_id_bytes(plan.escrow_program_id());
        let authenticated_transfer_program_id =
            program_id_bytes(plan.authenticated_transfer_program_id());
        let terms = XmrNativeEscrowTermsV3::new(XmrNativeEscrowTermsV3Input {
            swap_id: Hex32::from_bytes(plan.swap_id()),
            activation_commitment: Hex32::from_bytes(plan.activation_commitment()),
            escrow_program_id: Hex32::from_bytes(escrow_program_id),
            authenticated_transfer_program_id: Hex32::from_bytes(authenticated_transfer_program_id),
            metadata_account_id: Hex32::from_bytes(plan.metadata_account()),
            custody_account_id: Hex32::from_bytes(plan.custody_account()),
            depositor: BridgeParticipant::Taker,
            depositor_account_id: Hex32::from_bytes(plan.depositor_account()),
            claimant: BridgeParticipant::Maker,
            claimant_account_id: Hex32::from_bytes(plan.claimant_account()),
            claim_aggregate_x_only_public_key: Hex32::from_bytes(plan.claim_aggregate_x_only_key()),
            claim_authority_account_id: Hex32::from_bytes(plan.claim_authority_account()),
            refund_aggregate_x_only_public_key: Hex32::from_bytes(
                plan.refund_aggregate_x_only_key(),
            ),
            refund_authority_account_id: Hex32::from_bytes(plan.refund_authority_account()),
            maker_dleq_transcript_commitment: Hex32::from_bytes(
                plan.maker_dleq_transcript_commitment(),
            ),
            taker_dleq_transcript_commitment: Hex32::from_bytes(
                plan.taker_dleq_transcript_commitment(),
            ),
            claim_partial_context_binding: Hex32::from_bytes(plan.claim_partial_context_binding()),
            claim_partial_commitment: Hex32::from_bytes(plan.claim_partial_commitment()),
            amount: plan.amount(),
            refund_at_ms: plan.refund_at_ms(),
            punish_at_ms: plan.punish_at_ms(),
            claim_message_hash: Hex32::from_bytes(plan.claim_message_hash()),
            refund_message_hash: Hex32::from_bytes(plan.refund_message_hash()),
            punish_message_hash: Hex32::from_bytes(plan.punish_message_hash()),
        })
        .map_err(XmrLezBridgeBindingV3Error::Protocol)?;
        Ok(Self { terms })
    }

    /// Complete standalone v3 terms sent to the dedicated sidecar.
    pub const fn terms(&self) -> XmrNativeEscrowTermsV3 {
        self.terms
    }
}

/// Failure deriving exact bridge terms from Stage B.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum XmrLezBridgeBindingV3Error {
    /// Stage B does not belong to the supplied Stage-A agreement.
    #[error("XMR Stage-B activation does not match the base agreement")]
    StageB(#[source] XmrAgreementV1Error),
    /// Derived fields violate the strict v3 bridge contract.
    #[error("XMR Stage-B fields violate the v3 LEZ bridge contract")]
    Protocol(#[source] ProtocolValueError),
}

/// Canonical finalized first-lock evidence minted through the concrete bridge.
///
/// This type is deliberately non-`Clone` and has no public constructor. The
/// eventual release journal consumes it by ownership together with the
/// independent Monero observation and topology-authentication attestation.
/// It does not by itself authorize publication of the hidden claim partial.
/// ```compile_fail
/// use lez_bridge_adapter::FinalizedXmrLezFirstLockEvidenceV3;
///
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<FinalizedXmrLezFirstLockEvidenceV3>();
/// ```
#[derive(Debug, Eq, PartialEq)]
#[must_use]
pub struct FinalizedXmrLezFirstLockEvidenceV3 {
    run_id: RunId,
    observer: Participant,
    runtime: RuntimeDescriptor,
    terms: XmrNativeEscrowTermsV3,
    exact_funding: PreparedTransaction,
    finalized_clock: ChainClock,
    scanned_window: DiscoveryWindow,
    facts: FinalizedNativeXmrEffectFactsV3,
}

impl FinalizedXmrLezFirstLockEvidenceV3 {
    /// Composed run whose authenticated sidecar produced the evidence.
    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Role whose dedicated sidecar performed the finalized read.
    #[must_use]
    pub const fn observer(&self) -> Participant {
        self.observer
    }

    /// Exact pinned runtime identity used for the read.
    pub const fn runtime(&self) -> &RuntimeDescriptor {
        &self.runtime
    }

    /// Exact Stage-B-derived guest terms.
    pub const fn terms(&self) -> XmrNativeEscrowTermsV3 {
        self.terms
    }

    /// Exact funding bytes retained before the first send.
    pub const fn exact_funding(&self) -> &PreparedTransaction {
        &self.exact_funding
    }

    /// Stable finalized clock covering the complete scan.
    pub const fn finalized_clock(&self) -> ChainClock {
        self.finalized_clock
    }

    /// Exact caller-owned finalized discovery window.
    pub const fn scanned_window(&self) -> DiscoveryWindow {
        self.scanned_window
    }

    /// Validated canonical transaction, instruction, metadata, and custody facts.
    pub const fn facts(&self) -> &FinalizedNativeXmrEffectFactsV3 {
        &self.facts
    }

    /// Stage-B activation commitment stored by the finalized guest metadata.
    #[must_use]
    pub fn activation_commitment(&self) -> [u8; 32] {
        *self.terms.to_input().activation_commitment.as_bytes()
    }

    /// Exact swap ID stored by the finalized guest metadata.
    #[must_use]
    pub fn swap_id(&self) -> [u8; 32] {
        *self.terms.to_input().swap_id.as_bytes()
    }
}

/// Failure proving one exact finalized XMR-native LEZ first lock.
#[derive(Debug, Error)]
pub enum FinalizedXmrLezFirstLockError {
    /// Only the Taker may observe the first lock before releasing Monero.
    #[error("only the XMR Taker may prove the release-side LEZ first lock")]
    WrongObserver,
    /// Role/runtime/Stage-B terms do not match the dedicated sidecar.
    #[error("XMR LEZ first-lock request is not bound to the selected runtime")]
    Binding(#[source] ProtocolValueError),
    /// The authenticated bridge call failed or returned invalid evidence.
    #[error("XMR LEZ first-lock bridge read failed")]
    Bridge(#[source] BridgeClientError),
    /// The strict bridge response contradicted the caller-owned request.
    #[error("XMR LEZ first-lock bridge response changed its request binding")]
    ResponseBinding,
    /// A complete stable finalized scan proved the funding effect absent.
    #[error("XMR LEZ first-lock funding effect is finalized absent")]
    Absent,
    /// A stable scan could not exclude pending or unknown funding presence.
    #[error("XMR LEZ first-lock funding effect remains uncertain")]
    Uncertain,
    /// The finalized scan could not complete under the selected node profile.
    #[error("XMR LEZ first-lock finalized scan is unavailable: {0:?}")]
    Unavailable(FinalizedNativeXmrUnavailableReasonV3),
}

impl LezBridgeAdapter<BridgeClient> {
    /// Proves one exact Stage-B-bound funding transaction finalized on LEZ.
    ///
    /// The call uses the concrete authenticated, run-bound bridge client and
    /// makes one read-only attempt. Only a strict `Found` result for the exact
    /// retained funding bytes can mint the private evidence value. Absence,
    /// uncertainty, moving finality, history gaps, and transport failures remain
    /// distinct fail-closed outcomes.
    ///
    /// # Errors
    ///
    /// Rejects runtime/role drift, request/response substitution, every
    /// non-`Found` classification, or a bridge failure.
    pub async fn prove_finalized_xmr_first_lock_v3(
        &self,
        binding: &XmrLezBridgeBindingV3,
        request_id: RequestId,
        exact_funding: PreparedTransaction,
        window: DiscoveryWindow,
    ) -> Result<FinalizedXmrLezFirstLockEvidenceV3, FinalizedXmrLezFirstLockError> {
        let request = build_first_lock_request(
            &self.run_id,
            self.local_participant,
            &self.runtime,
            binding,
            request_id,
            exact_funding,
            window,
        )?;
        let response = self
            .transport
            .classify_finalized_native_xmr_effect_v3(request.clone())
            .await
            .map_err(FinalizedXmrLezFirstLockError::Bridge)?;
        mint_first_lock_evidence(request, response)
    }
}

fn build_first_lock_request(
    run_id: &RunId,
    observer: Participant,
    runtime: &RuntimeDescriptor,
    binding: &XmrLezBridgeBindingV3,
    request_id: RequestId,
    exact_funding: PreparedTransaction,
    window: DiscoveryWindow,
) -> Result<ClassifyFinalizedNativeXmrEffectV3Request, FinalizedXmrLezFirstLockError> {
    if observer != Participant::Taker {
        return Err(FinalizedXmrLezFirstLockError::WrongObserver);
    }
    let context = MessageContext::new(run_id.clone(), request_id, bridge_participant(observer));
    binding
        .terms
        .validate_runtime_binding(&context, runtime)
        .map_err(FinalizedXmrLezFirstLockError::Binding)?;
    Ok(ClassifyFinalizedNativeXmrEffectV3Request::new(
        context,
        runtime.clone(),
        binding.terms,
        XmrNativeEffectV3::Fund,
        FinalizedNativeXmrTransactionTargetV3::exact(exact_funding),
        window,
    ))
}

fn mint_first_lock_evidence(
    request: ClassifyFinalizedNativeXmrEffectV3Request,
    response: ClassifyFinalizedNativeXmrEffectV3Result,
) -> Result<FinalizedXmrLezFirstLockEvidenceV3, FinalizedXmrLezFirstLockError> {
    if request.context.sidecar_role != BridgeParticipant::Taker
        || request.runtime.sidecar_role != BridgeParticipant::Taker
        || request.effect != XmrNativeEffectV3::Fund
    {
        return Err(FinalizedXmrLezFirstLockError::ResponseBinding);
    }
    request
        .terms
        .validate_runtime_binding(&request.context, &request.runtime)
        .map_err(FinalizedXmrLezFirstLockError::Binding)?;
    let FinalizedNativeXmrTransactionTargetV3::Exact {
        transaction: exact_funding,
    } = request.target.clone()
    else {
        return Err(FinalizedXmrLezFirstLockError::ResponseBinding);
    };
    if response.context != request.context
        || response.terms != request.terms
        || response.effect != request.effect
        || response.target != request.target
    {
        return Err(FinalizedXmrLezFirstLockError::ResponseBinding);
    }
    let response = ClassifyFinalizedNativeXmrEffectV3Result::new(
        response.context,
        response.terms,
        response.effect,
        response.target,
        response.outcome,
    )
    .map_err(FinalizedXmrLezFirstLockError::Binding)?;
    match response.outcome {
        FinalizedNativeXmrScanOutcomeV3::Found {
            finalized_clock,
            scanned_window,
            facts,
        } => Ok(FinalizedXmrLezFirstLockEvidenceV3 {
            run_id: request.context.run_id,
            observer: Participant::Taker,
            runtime: request.runtime,
            terms: request.terms,
            exact_funding,
            finalized_clock,
            scanned_window,
            facts: *facts,
        }),
        FinalizedNativeXmrScanOutcomeV3::Absent { .. } => {
            Err(FinalizedXmrLezFirstLockError::Absent)
        }
        FinalizedNativeXmrScanOutcomeV3::Uncertain { .. } => {
            Err(FinalizedXmrLezFirstLockError::Uncertain)
        }
        FinalizedNativeXmrScanOutcomeV3::Unavailable { reason } => {
            Err(FinalizedXmrLezFirstLockError::Unavailable(reason))
        }
    }
}

const fn bridge_participant(participant: Participant) -> BridgeParticipant {
    match participant {
        Participant::Maker => BridgeParticipant::Maker,
        Participant::Taker => BridgeParticipant::Taker,
    }
}

fn program_id_bytes(words: [u32; 8]) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    for (index, word) in words.into_iter().enumerate() {
        bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use lez_bridge_client::{BridgeClientConfig, SidecarCapability};
    use lez_bridge_protocol::{
        AccountIds, ChainPosition, ExactTransactionBytes, FinalizedBlockIdentity,
        NativeCustodyFacts, ObservedTransactionFacts, RuntimeCompatibility, TransactionId,
        XmrNativeEscrowMetadataFactsV3, XmrNativeEscrowStateV3, XmrNativeInstructionFactsV3,
    };

    use super::*;

    fn h(byte: u8) -> Hex32 {
        Hex32::from_bytes([byte; 32])
    }

    fn terms(amount: u128) -> XmrNativeEscrowTermsV3 {
        XmrNativeEscrowTermsV3::new(XmrNativeEscrowTermsV3Input {
            swap_id: h(1),
            activation_commitment: h(2),
            escrow_program_id: h(3),
            authenticated_transfer_program_id: h(4),
            metadata_account_id: h(5),
            custody_account_id: h(6),
            depositor: BridgeParticipant::Taker,
            depositor_account_id: h(7),
            claimant: BridgeParticipant::Maker,
            claimant_account_id: h(8),
            claim_aggregate_x_only_public_key: h(9),
            claim_authority_account_id: h(10),
            refund_aggregate_x_only_public_key: h(11),
            refund_authority_account_id: h(12),
            maker_dleq_transcript_commitment: h(13),
            taker_dleq_transcript_commitment: h(14),
            claim_partial_context_binding: h(15),
            claim_partial_commitment: h(16),
            amount,
            refund_at_ms: 10_000,
            punish_at_ms: 20_000,
            claim_message_hash: h(17),
            refund_message_hash: h(18),
            punish_message_hash: h(19),
        })
        .expect("canonical XMR v3 terms")
    }

    fn binding() -> XmrLezBridgeBindingV3 {
        XmrLezBridgeBindingV3 { terms: terms(42) }
    }

    fn run_id() -> RunId {
        RunId::new("xmr-v3-run").expect("run ID")
    }

    fn runtime(role: BridgeParticipant) -> RuntimeDescriptor {
        RuntimeDescriptor::new(
            role,
            RuntimeCompatibility::LeeV0_2_0,
            h(40),
            h(41),
            h(42),
            h(3),
            match role {
                BridgeParticipant::Maker => h(8),
                BridgeParticipant::Taker => h(7),
            },
        )
    }

    fn exact_funding() -> PreparedTransaction {
        PreparedTransaction::new(
            TransactionId::from_bytes([61; 32]),
            ExactTransactionBytes::new(vec![61]).expect("transaction bytes"),
        )
    }

    fn window() -> DiscoveryWindow {
        DiscoveryWindow::new(90, 21).expect("canonical window")
    }

    fn request() -> ClassifyFinalizedNativeXmrEffectV3Request {
        build_first_lock_request(
            &run_id(),
            Participant::Taker,
            &runtime(BridgeParticipant::Taker),
            &binding(),
            RequestId::new("xmr-v3-request").expect("request ID"),
            exact_funding(),
            window(),
        )
        .expect("valid release-side request")
    }

    fn finalized_clock() -> ChainClock {
        ChainClock::new(h(71), 110, 30_000)
    }

    fn funding_facts() -> FinalizedNativeXmrEffectFactsV3 {
        let prepared = exact_funding();
        FinalizedNativeXmrEffectFactsV3::new(
            ObservedTransactionFacts::new(
                prepared.transaction_id,
                prepared.exact_bytes,
                ChainPosition::new(h(70), 100, 2),
                AccountIds::new(vec![h(7)]).expect("signers"),
                true,
            ),
            XmrNativeInstructionFactsV3::new(
                XmrNativeEffectV3::Fund,
                h(3),
                AccountIds::new(vec![h(5), h(6), h(7)]).expect("accounts"),
                h(1),
                h(61),
                None,
            )
            .expect("fund instruction"),
            None,
            FinalizedBlockIdentity::new(100, h(70), 25_000),
            XmrNativeEscrowMetadataFactsV3::from_terms(terms(42), XmrNativeEscrowStateV3::Funded),
            NativeCustodyFacts::new(h(6), h(4), 42),
        )
    }

    fn response_with(
        request: &ClassifyFinalizedNativeXmrEffectV3Request,
        outcome: FinalizedNativeXmrScanOutcomeV3,
    ) -> ClassifyFinalizedNativeXmrEffectV3Result {
        ClassifyFinalizedNativeXmrEffectV3Result::new(
            request.context.clone(),
            request.terms,
            request.effect,
            request.target.clone(),
            outcome,
        )
        .expect("protocol-valid classifier response")
    }

    fn found_response(
        request: &ClassifyFinalizedNativeXmrEffectV3Request,
    ) -> ClassifyFinalizedNativeXmrEffectV3Result {
        response_with(
            request,
            FinalizedNativeXmrScanOutcomeV3::found(finalized_clock(), window(), funding_facts()),
        )
    }

    #[test]
    fn validated_found_mints_exact_non_cloneable_capability() {
        let request = request();
        let response = found_response(&request);
        let evidence = mint_first_lock_evidence(request.clone(), response)
            .expect("validated Found mints evidence");

        assert_eq!(evidence.run_id(), &request.context.run_id);
        assert_eq!(evidence.observer(), Participant::Taker);
        assert_eq!(evidence.runtime(), &request.runtime);
        assert_eq!(evidence.terms(), request.terms);
        assert_eq!(evidence.exact_funding(), &exact_funding());
        assert_eq!(evidence.finalized_clock(), finalized_clock());
        assert_eq!(evidence.scanned_window(), window());
        assert_eq!(evidence.facts(), &funding_facts());
        assert_eq!(evidence.activation_commitment(), [2; 32]);
        assert_eq!(evidence.swap_id(), [1; 32]);
    }

    #[test]
    fn every_non_found_outcome_fails_closed_without_minting() {
        let cases = [
            (
                FinalizedNativeXmrScanOutcomeV3::absent(finalized_clock(), window()),
                "absent",
            ),
            (
                FinalizedNativeXmrScanOutcomeV3::uncertain(finalized_clock(), window()),
                "uncertain",
            ),
            (
                FinalizedNativeXmrScanOutcomeV3::unavailable(
                    FinalizedNativeXmrUnavailableReasonV3::HistoryUnavailable,
                ),
                "unavailable",
            ),
        ];
        for (outcome, expected) in cases {
            let request = request();
            let error = mint_first_lock_evidence(request.clone(), response_with(&request, outcome))
                .expect_err("non-Found must not mint");
            match (expected, error) {
                ("absent", FinalizedXmrLezFirstLockError::Absent)
                | ("uncertain", FinalizedXmrLezFirstLockError::Uncertain)
                | (
                    "unavailable",
                    FinalizedXmrLezFirstLockError::Unavailable(
                        FinalizedNativeXmrUnavailableReasonV3::HistoryUnavailable,
                    ),
                ) => {}
                (_, other) => panic!("unexpected {expected} error: {other}"),
            }
        }
    }

    #[test]
    fn response_context_terms_effect_and_target_drift_cannot_mint() {
        let assert_rejected = |response| {
            assert!(matches!(
                mint_first_lock_evidence(request(), response),
                Err(FinalizedXmrLezFirstLockError::ResponseBinding)
            ));
        };

        let canonical = request();
        let mut response = found_response(&canonical);
        response.context.request_id = RequestId::new("drifted-request").expect("request ID");
        assert_rejected(response);

        let mut response = found_response(&canonical);
        response.terms = terms(43);
        assert_rejected(response);

        let mut response = found_response(&canonical);
        response.effect = XmrNativeEffectV3::Refund;
        assert_rejected(response);

        let mut response = found_response(&canonical);
        response.target = FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {};
        assert_rejected(response);
    }

    #[test]
    fn malformed_found_facts_cannot_cross_the_mint_boundary() {
        let request = request();
        let mut response = found_response(&request);
        let FinalizedNativeXmrScanOutcomeV3::Found { facts, .. } = &mut response.outcome else {
            panic!("Found fixture")
        };
        facts.transaction.exact_bytes =
            ExactTransactionBytes::new(vec![0xff]).expect("mutated transaction bytes");

        assert!(matches!(
            mint_first_lock_evidence(request, response),
            Err(FinalizedXmrLezFirstLockError::Binding(
                ProtocolValueError::XmrFactsMismatch("exact transaction bytes")
            ))
        ));
    }

    #[test]
    fn wrong_observer_is_a_pure_pre_wire_rejection() {
        let error = build_first_lock_request(
            &run_id(),
            Participant::Maker,
            &runtime(BridgeParticipant::Maker),
            &binding(),
            RequestId::new("wrong-observer").expect("request ID"),
            exact_funding(),
            window(),
        )
        .expect_err("Maker must not perform the release-side observation");
        assert!(matches!(
            error,
            FinalizedXmrLezFirstLockError::WrongObserver
        ));
    }

    #[tokio::test]
    async fn wrong_observer_never_reaches_an_unavailable_transport() {
        let runtime = runtime(BridgeParticipant::Maker);
        let client = BridgeClient::connect(BridgeClientConfig::new(
            "http://127.0.0.1:9",
            SidecarCapability::new("xmr-v3-capability-000000000000000001").expect("capability"),
            run_id(),
            runtime.clone(),
            Duration::from_millis(100),
        ))
        .expect("valid loopback client");
        let adapter = LezBridgeAdapter::new(client, run_id(), runtime, Participant::Maker)
            .expect("role-local adapter");

        assert!(matches!(
            adapter
                .prove_finalized_xmr_first_lock_v3(
                    &binding(),
                    RequestId::new("wrong-observer-wire").expect("request ID"),
                    exact_funding(),
                    window(),
                )
                .await,
            Err(FinalizedXmrLezFirstLockError::WrongObserver)
        ));
    }
}
