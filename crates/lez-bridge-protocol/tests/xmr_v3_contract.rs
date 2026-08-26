use lez_bridge_protocol::{
    AccountIds, AggregateBip340Signature, ChainClock, ChainPosition,
    ClassifyFinalizedNativeXmrEffectV3Request, ClassifyFinalizedNativeXmrEffectV3Result,
    CompleteNativeXmrClaimV3Request, CompleteNativeXmrClaimV3Result,
    CompleteNativeXmrRefundV3Request, CompleteNativeXmrRefundV3Result,
    CurrentProfileClockAccountSnapshot, DiscoveryWindow, ExactMessageBytes, ExactTransactionBytes,
    FinalizedBlockIdentity, FinalizedNativeXmrEffectFactsV3, FinalizedNativeXmrScanOutcomeV3,
    FinalizedNativeXmrTransactionTargetV3, FinalizedNativeXmrUnavailableReasonV3, Hex32,
    METHOD_CLASSIFY_FINALIZED_NATIVE_XMR_EFFECT_V3, METHOD_COMPLETE_NATIVE_XMR_CLAIM_V3,
    METHOD_COMPLETE_NATIVE_XMR_REFUND_V3, METHOD_PREPARE_NATIVE_XMR_CLAIM_AUTHORIZATION_V3,
    METHOD_PREPARE_NATIVE_XMR_CLAIM_V3, METHOD_PREPARE_NATIVE_XMR_ESCROW_V3,
    METHOD_PREPARE_NATIVE_XMR_PUNISH_V3, METHOD_PREPARE_NATIVE_XMR_REFUND_V3,
    METHOD_SUBMIT_NATIVE_XMR_CLAIM_AUTHORIZATION_V3, MessageContext, NativeCustodyFacts,
    ObservedTransactionFacts, Participant, PrepareCurrentProfileClockRequest,
    PrepareCurrentProfileClockResult, PrepareNativeXmrClaimAuthorizationV3Request,
    PrepareNativeXmrClaimAuthorizationV3Result, PrepareNativeXmrClaimV3Request,
    PrepareNativeXmrClaimV3Result, PrepareNativeXmrEscrowV3Request, PrepareNativeXmrEscrowV3Result,
    PrepareNativeXmrPunishV3Request, PrepareNativeXmrPunishV3Result,
    PrepareNativeXmrRefundV3Request, PrepareNativeXmrRefundV3Result, PreparedTransaction,
    PreparedWitnessedClaim, ProtocolValueError, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor, SubmissionOutcome, SubmitNativeXmrClaimAuthorizationV3Request,
    SubmitNativeXmrClaimAuthorizationV3Result, SubmitTransactionResult, TransactionId,
    VerifyCurrentProfileClockRequest, VerifyCurrentProfileClockResult,
    XMR_NATIVE_ESCROW_TERMS_VERSION, XmrClaimPartialV3, XmrNativeEffectV3,
    XmrNativeEscrowMetadataFactsV3, XmrNativeEscrowStateV3, XmrNativeEscrowTermsV3,
    XmrNativeEscrowTermsV3Input, XmrNativeInstructionFactsV3,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn terms_input() -> XmrNativeEscrowTermsV3Input {
    XmrNativeEscrowTermsV3Input {
        swap_id: h(1),
        activation_commitment: h(2),
        escrow_program_id: h(3),
        authenticated_transfer_program_id: h(4),
        metadata_account_id: h(5),
        custody_account_id: h(6),
        depositor: Participant::Taker,
        depositor_account_id: h(7),
        claimant: Participant::Maker,
        claimant_account_id: h(8),
        claim_aggregate_x_only_public_key: h(9),
        claim_authority_account_id: h(10),
        refund_aggregate_x_only_public_key: h(11),
        refund_authority_account_id: h(12),
        maker_dleq_transcript_commitment: h(13),
        taker_dleq_transcript_commitment: h(14),
        claim_partial_context_binding: h(15),
        claim_partial_commitment: h(16),
        amount: 42,
        refund_at_ms: 10_000,
        punish_at_ms: 20_000,
        claim_message_hash: h(17),
        refund_message_hash: h(18),
        punish_message_hash: h(19),
    }
}

fn terms() -> XmrNativeEscrowTermsV3 {
    XmrNativeEscrowTermsV3::new(terms_input()).expect("valid XMR v3 terms")
}

fn context() -> MessageContext {
    MessageContext::new(
        RunId::new("xmr-v3-run").expect("run id"),
        RequestId::new("xmr-v3-request").expect("request id"),
        Participant::Taker,
    )
}

fn runtime() -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        Participant::Taker,
        RuntimeCompatibility::LeeV0_2_0,
        h(40),
        h(41),
        h(42),
        h(3),
        h(7),
    )
}

fn prepared(id: u8, bytes: u8) -> PreparedTransaction {
    PreparedTransaction::new(
        TransactionId::from_bytes([id; 32]),
        ExactTransactionBytes::new(vec![bytes]).expect("transaction bytes"),
    )
}

fn reserved(hash: Hex32, suffix: &str) -> PreparedWitnessedClaim {
    PreparedWitnessedClaim::new(
        RequestId::new(format!("xmr-v3-{suffix}")).expect("request id"),
        hash,
        ExactMessageBytes::new(vec![
            0xa0,
            u8::try_from(suffix.len()).expect("bounded fixture suffix"),
        ])
        .expect("message bytes"),
    )
}

fn roundtrip<T>(value: &T)
where
    T: Serialize + DeserializeOwned + std::fmt::Debug + Eq,
{
    let encoded = serde_json::to_vec(value).expect("serialize");
    let decoded: T = serde_json::from_slice(&encoded).expect("deserialize");
    assert_eq!(&decoded, value);
}

fn roundtrip_and_reject_unknown<T>(value: &T)
where
    T: Serialize + DeserializeOwned + std::fmt::Debug + Eq,
{
    roundtrip(value);
    let mut encoded = serde_json::to_value(value).expect("serialize");
    encoded
        .as_object_mut()
        .expect("object")
        .insert("unexpected".to_owned(), Value::Bool(true));
    assert!(serde_json::from_value::<T>(encoded).is_err());
}

#[test]
fn xmr_v3_method_names_are_strictly_additive() {
    assert_eq!(
        METHOD_PREPARE_NATIVE_XMR_CLAIM_V3,
        "lez_bridge.v3.prepare_native_xmr_claim"
    );
    assert_eq!(
        METHOD_COMPLETE_NATIVE_XMR_CLAIM_V3,
        "lez_bridge.v3.complete_native_xmr_claim"
    );
    assert_eq!(
        METHOD_PREPARE_NATIVE_XMR_REFUND_V3,
        "lez_bridge.v3.prepare_native_xmr_refund"
    );
    assert_eq!(
        METHOD_COMPLETE_NATIVE_XMR_REFUND_V3,
        "lez_bridge.v3.complete_native_xmr_refund"
    );
    assert_eq!(
        METHOD_PREPARE_NATIVE_XMR_PUNISH_V3,
        "lez_bridge.v3.prepare_native_xmr_punish"
    );
    assert_eq!(
        METHOD_PREPARE_NATIVE_XMR_ESCROW_V3,
        "lez_bridge.v3.prepare_native_xmr_escrow"
    );
    assert_eq!(
        METHOD_PREPARE_NATIVE_XMR_CLAIM_AUTHORIZATION_V3,
        "lez_bridge.v3.prepare_native_xmr_claim_authorization"
    );
    assert_eq!(
        METHOD_SUBMIT_NATIVE_XMR_CLAIM_AUTHORIZATION_V3,
        "lez_bridge.v3.submit_native_xmr_claim_authorization"
    );
    assert_eq!(
        METHOD_CLASSIFY_FINALIZED_NATIVE_XMR_EFFECT_V3,
        "lez_bridge.v3.classify_finalized_native_xmr_effect"
    );
}

#[test]
fn xmr_v3_terms_have_one_canonical_standalone_wire_shape() {
    let terms = terms();
    assert_eq!(terms.to_input(), terms_input());
    terms
        .validate_runtime_binding(&context(), &runtime())
        .expect("bound Taker runtime");
    let encoded = serde_json::to_value(terms).expect("serialize terms");
    assert_eq!(
        encoded,
        json!({
            "version": XMR_NATIVE_ESCROW_TERMS_VERSION,
            "swap_id": "01".repeat(32),
            "activation_commitment": "02".repeat(32),
            "escrow_program_id": "03".repeat(32),
            "authenticated_transfer_program_id": "04".repeat(32),
            "metadata_account_id": "05".repeat(32),
            "custody_account_id": "06".repeat(32),
            "depositor": "taker",
            "depositor_account_id": "07".repeat(32),
            "claimant": "maker",
            "claimant_account_id": "08".repeat(32),
            "claim_aggregate_x_only_public_key": "09".repeat(32),
            "claim_authority_account_id": "0a".repeat(32),
            "refund_aggregate_x_only_public_key": "0b".repeat(32),
            "refund_authority_account_id": "0c".repeat(32),
            "maker_dleq_transcript_commitment": "0d".repeat(32),
            "taker_dleq_transcript_commitment": "0e".repeat(32),
            "claim_partial_context_binding": "0f".repeat(32),
            "claim_partial_commitment": "10".repeat(32),
            "amount": "42",
            "refund_at_ms": 10_000,
            "punish_at_ms": 20_000,
            "claim_message_hash": "11".repeat(32),
            "refund_message_hash": "12".repeat(32),
            "punish_message_hash": "13".repeat(32),
        })
    );
    assert_eq!(
        serde_json::from_value::<XmrNativeEscrowTermsV3>(encoded).expect("deserialize terms"),
        terms
    );
}

#[test]
fn xmr_v3_terms_reject_mixed_unknown_zero_and_aliased_values() {
    let mut canonical = serde_json::to_value(terms()).expect("serialize terms");
    canonical.as_object_mut().expect("object").insert(
        "aggregate_authority_account_id".into(),
        Value::String("20".repeat(32)),
    );
    assert!(serde_json::from_value::<XmrNativeEscrowTermsV3>(canonical).is_err());

    let mut invalid = terms_input();
    invalid.depositor = Participant::Maker;
    assert_eq!(
        XmrNativeEscrowTermsV3::new(invalid),
        Err(ProtocolValueError::InvalidXmrRoleMapping)
    );

    let mut invalid = terms_input();
    invalid.claim_aggregate_x_only_public_key = h(0);
    assert!(matches!(
        XmrNativeEscrowTermsV3::new(invalid),
        Err(ProtocolValueError::ZeroXmrValue(_))
    ));

    let mut invalid = terms_input();
    invalid.refund_authority_account_id = invalid.claim_authority_account_id;
    assert!(matches!(
        XmrNativeEscrowTermsV3::new(invalid),
        Err(ProtocolValueError::AliasedXmrValues(_, _))
    ));

    let mut invalid = terms_input();
    invalid.punish_at_ms = invalid.refund_at_ms;
    assert_eq!(
        XmrNativeEscrowTermsV3::new(invalid),
        Err(ProtocolValueError::InvalidXmrWindows)
    );

    let mut wrong_runtime = runtime();
    wrong_runtime.escrow_program_id = h(99);
    assert_eq!(
        terms().validate_runtime_binding(&context(), &wrong_runtime),
        Err(ProtocolValueError::XmrFactsMismatch(
            "runtime escrow program"
        ))
    );

    let mut wrong_runtime = runtime();
    wrong_runtime.signer_account_id = h(99);
    assert_eq!(
        terms().validate_runtime_binding(&context(), &wrong_runtime),
        Err(ProtocolValueError::XmrFactsMismatch(
            "runtime signer account"
        ))
    );
}
fn roundtrip_authorization_submission(
    context: &MessageContext,
    runtime: &RuntimeDescriptor,
    terms: &XmrNativeEscrowTermsV3,
) {
    let authorization = prepared(57, 0xc8);
    roundtrip(&SubmitNativeXmrClaimAuthorizationV3Request::new(
        context.clone(),
        runtime.clone(),
        *terms,
        authorization.clone(),
    ));
    roundtrip(&SubmitNativeXmrClaimAuthorizationV3Result::new(
        context.clone(),
        *terms,
        authorization.transaction_id,
        SubmissionOutcome::AlreadyKnown,
    ));
}

fn roundtrip_claim_authorization_preparation(
    context: &MessageContext,
    runtime: &RuntimeDescriptor,
    terms: &XmrNativeEscrowTermsV3,
) {
    let partial = XmrClaimPartialV3::new([0x77; 32]).expect("claim partial");
    assert_eq!(format!("{partial:?}"), "XmrClaimPartialV3([REDACTED])");
    assert_eq!(
        serde_json::to_value(&partial).expect("serialize partial"),
        Value::String("77".repeat(32))
    );
    roundtrip(&PrepareNativeXmrClaimAuthorizationV3Request::new(
        context.clone(),
        runtime.clone(),
        *terms,
        partial,
    ));
    roundtrip(&PrepareNativeXmrClaimAuthorizationV3Result::new(
        context.clone(),
        *terms,
        prepared(55, 0xc6),
    ));
}

#[test]
fn all_nine_xmr_v3_method_families_roundtrip_strict_json() {
    let context = context();
    let runtime = runtime();
    let terms = terms();
    let claim = reserved(h(17), "claim");
    let refund = reserved(h(18), "refund");
    let signature = AggregateBip340Signature::from_bytes([0x55; 64]);

    roundtrip(&PrepareNativeXmrClaimV3Request::new(
        context.clone(),
        runtime.clone(),
        terms,
    ));
    roundtrip(
        &PrepareNativeXmrClaimV3Result::new(context.clone(), terms, claim.clone())
            .expect("claim result"),
    );
    roundtrip(
        &CompleteNativeXmrClaimV3Request::new(
            context.clone(),
            runtime.clone(),
            terms,
            claim,
            signature,
        )
        .expect("claim completion"),
    );
    roundtrip(&CompleteNativeXmrClaimV3Result::new(
        context.clone(),
        terms,
        prepared(50, 0xc1),
    ));

    roundtrip(&PrepareNativeXmrRefundV3Request::new(
        context.clone(),
        runtime.clone(),
        terms,
    ));
    roundtrip(
        &PrepareNativeXmrRefundV3Result::new(context.clone(), terms, refund.clone())
            .expect("refund result"),
    );
    roundtrip(
        &CompleteNativeXmrRefundV3Request::new(
            context.clone(),
            runtime.clone(),
            terms,
            refund,
            signature,
        )
        .expect("refund completion"),
    );
    roundtrip(&CompleteNativeXmrRefundV3Result::new(
        context.clone(),
        terms,
        prepared(51, 0xc2),
    ));

    roundtrip(&PrepareNativeXmrPunishV3Request::new(
        context.clone(),
        runtime.clone(),
        terms,
    ));
    roundtrip(&PrepareNativeXmrPunishV3Result::new(
        context.clone(),
        terms,
        prepared(52, 0xc3),
    ));
    roundtrip(&PrepareNativeXmrEscrowV3Request::new(
        context.clone(),
        runtime.clone(),
        terms,
    ));
    roundtrip(&PrepareNativeXmrEscrowV3Result::new(
        context.clone(),
        terms,
        prepared(53, 0xc4),
        prepared(54, 0xc5),
    ));

    roundtrip_claim_authorization_preparation(&context, &runtime, &terms);
    roundtrip_authorization_submission(&context, &runtime, &terms);

    roundtrip(&ClassifyFinalizedNativeXmrEffectV3Request::new(
        context,
        runtime,
        terms,
        XmrNativeEffectV3::Claim,
        FinalizedNativeXmrTransactionTargetV3::exact(prepared(56, 0xc7)),
        DiscoveryWindow::new(90, 21).expect("window"),
    ));
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "all four nested clock wire envelopes share one exact fixture"
)]
fn current_profile_clock_wires_roundtrip_and_reject_unknown_fields() {
    let activated_terms = terms();
    let input = activated_terms.to_input();
    let preparation_context = context();
    let preparation_request = PrepareCurrentProfileClockRequest::new(
        preparation_context.clone(),
        runtime(),
        activated_terms,
        input.claimant_account_id,
        input.punish_at_ms,
    );
    roundtrip_and_reject_unknown(&preparation_request);

    let transaction = prepared(71, 0xd1);
    let sender_before = CurrentProfileClockAccountSnapshot::new(
        input.depositor_account_id,
        50,
        4,
        input.authenticated_transfer_program_id,
        h(72),
    );
    let recipient_before = CurrentProfileClockAccountSnapshot::new(
        input.claimant_account_id,
        10,
        2,
        input.authenticated_transfer_program_id,
        h(73),
    );
    let preparation = PrepareCurrentProfileClockResult {
        context: preparation_context,
        runtime: runtime(),
        terms: activated_terms,
        recipient_account_id: input.claimant_account_id,
        exclusive_punish_at_ms: input.punish_at_ms,
        transaction: transaction.clone(),
        clock_before: ChainClock::new(h(74), 12, 1_000),
        sender_before,
        recipient_before,
        metadata_account_sha256_before: h(75),
        custody_account_sha256_before: h(76),
    };
    roundtrip_and_reject_unknown(&preparation);

    let submission_context = MessageContext::new(
        RunId::new("xmr-v3-run").expect("run id"),
        transaction.transaction_id.submission_request_id(),
        Participant::Taker,
    );
    let submission = SubmitTransactionResult::new(
        submission_context.clone(),
        transaction.transaction_id,
        SubmissionOutcome::Accepted,
    );
    let verification_context = MessageContext::new(
        RunId::new("xmr-v3-run").expect("run id"),
        RequestId::new("xmr-v3-clock-verify").expect("request id"),
        Participant::Taker,
    );
    let verification_request = VerifyCurrentProfileClockRequest {
        context: verification_context.clone(),
        runtime: runtime(),
        preparation: preparation.clone(),
        submission: submission.clone(),
    };
    roundtrip_and_reject_unknown(&verification_request);

    let verification = VerifyCurrentProfileClockResult {
        context: verification_context,
        runtime: runtime(),
        terms: activated_terms,
        recipient_account_id: input.claimant_account_id,
        exclusive_punish_at_ms: input.punish_at_ms,
        transaction_id: transaction.transaction_id,
        submission_request_id: submission_context.request_id,
        submission_outcome: submission.outcome,
        node_submission_attempts: 1,
        transfer_amount: 1,
        clock_before: preparation.clock_before,
        clock_after: ChainClock::new(h(77), 13, 2_000),
        sender_before,
        sender_after: CurrentProfileClockAccountSnapshot::new(
            sender_before.account_id,
            sender_before.balance - 1,
            sender_before.nonce + 1,
            sender_before.program_owner,
            h(78),
        ),
        recipient_before,
        recipient_after: CurrentProfileClockAccountSnapshot::new(
            recipient_before.account_id,
            recipient_before.balance + 1,
            recipient_before.nonce,
            recipient_before.program_owner,
            h(79),
        ),
        metadata_account_sha256_before: preparation.metadata_account_sha256_before,
        metadata_account_sha256_after: preparation.metadata_account_sha256_before,
        custody_account_sha256_before: preparation.custody_account_sha256_before,
        custody_account_sha256_after: preparation.custody_account_sha256_before,
        escrow_accounts_byte_identical: true,
        accounting_verified: true,
        local_only: true,
        retry_policy: "one_node_submission_attempt_no_retry_poll_only".to_owned(),
    };
    roundtrip_and_reject_unknown(&verification);
}

#[test]
fn xmr_v3_messages_reject_unknown_fields_and_wrong_agreement_hashes() {
    let request = PrepareNativeXmrClaimV3Request::new(context(), runtime(), terms());
    let mut encoded = serde_json::to_value(request).expect("serialize request");
    encoded
        .as_object_mut()
        .expect("object")
        .insert("legacy_terms".into(), Value::Null);
    assert!(serde_json::from_value::<PrepareNativeXmrClaimV3Request>(encoded).is_err());

    let mut encoded = serde_json::to_value(SubmitNativeXmrClaimAuthorizationV3Request::new(
        context(),
        runtime(),
        terms(),
        prepared(57, 0xc8),
    ))
    .expect("serialize authorization submission");
    encoded
        .as_object_mut()
        .expect("object")
        .insert("transaction".into(), Value::Null);
    assert!(serde_json::from_value::<SubmitNativeXmrClaimAuthorizationV3Request>(encoded).is_err());

    let mut encoded = serde_json::to_value(SubmitNativeXmrClaimAuthorizationV3Result::new(
        context(),
        terms(),
        TransactionId::from_bytes([57; 32]),
        SubmissionOutcome::Accepted,
    ))
    .expect("serialize authorization submission result");
    encoded
        .as_object_mut()
        .expect("object")
        .insert("accepted".into(), Value::Bool(true));
    assert!(serde_json::from_value::<SubmitNativeXmrClaimAuthorizationV3Result>(encoded).is_err());

    let invalid = PrepareNativeXmrClaimV3Result {
        context: context(),
        terms: terms(),
        claim: reserved(h(99), "wrong-claim"),
    };
    assert!(
        serde_json::from_value::<PrepareNativeXmrClaimV3Result>(
            serde_json::to_value(invalid).expect("serialize invalid")
        )
        .is_err(),
        "semantic bindings must survive JSON deserialization"
    );

    let invalid = CompleteNativeXmrRefundV3Request {
        context: context(),
        runtime: runtime(),
        terms: terms(),
        refund: reserved(h(99), "wrong-refund"),
        aggregate_signature: AggregateBip340Signature::from_bytes([0x55; 64]),
    };
    assert!(
        serde_json::from_value::<CompleteNativeXmrRefundV3Request>(
            serde_json::to_value(invalid).expect("serialize invalid")
        )
        .is_err(),
        "completion must reject a reservation for another agreement"
    );
}

fn effect_fixture(
    effect: XmrNativeEffectV3,
) -> (
    FinalizedNativeXmrTransactionTargetV3,
    FinalizedNativeXmrEffectFactsV3,
) {
    let terms = terms();
    let (tag, accounts, signer, message_hash, partial, signature, state, balance) = match effect {
        XmrNativeEffectV3::Initialize => (
            60,
            vec![h(5), h(6), h(7), h(8), h(10), h(12)],
            h(7),
            h(60),
            None,
            None,
            XmrNativeEscrowStateV3::Empty,
            0,
        ),
        XmrNativeEffectV3::Fund => (
            61,
            vec![h(5), h(6), h(7)],
            h(7),
            h(61),
            None,
            None,
            XmrNativeEscrowStateV3::Funded,
            42,
        ),
        XmrNativeEffectV3::AuthorizeClaim => (
            62,
            vec![h(5), h(7)],
            h(7),
            h(62),
            Some(h(63)),
            None,
            XmrNativeEscrowStateV3::ClaimAuthorized,
            42,
        ),
        XmrNativeEffectV3::Claim => (
            64,
            vec![h(5), h(6), h(8), h(10)],
            h(10),
            h(17),
            None,
            Some(AggregateBip340Signature::from_bytes([0x64; 64])),
            XmrNativeEscrowStateV3::Claimed,
            0,
        ),
        XmrNativeEffectV3::Refund => (
            65,
            vec![h(5), h(6), h(7), h(12)],
            h(12),
            h(18),
            None,
            Some(AggregateBip340Signature::from_bytes([0x65; 64])),
            XmrNativeEscrowStateV3::Refunded,
            0,
        ),
        XmrNativeEffectV3::Punish => (
            66,
            vec![h(5), h(6), h(8)],
            h(8),
            h(19),
            None,
            None,
            XmrNativeEscrowStateV3::Claimed,
            0,
        ),
    };
    let prepared = prepared(tag, tag);
    let transaction = ObservedTransactionFacts::new(
        prepared.transaction_id,
        prepared.exact_bytes.clone(),
        ChainPosition::new(h(70), 100, 2),
        AccountIds::new(vec![signer]).expect("signers"),
        true,
    );
    let instruction = XmrNativeInstructionFactsV3::new(
        effect,
        h(3),
        AccountIds::new(accounts).expect("accounts"),
        h(1),
        message_hash,
        partial,
    )
    .expect("instruction");
    let facts = FinalizedNativeXmrEffectFactsV3::new(
        transaction,
        instruction,
        signature,
        FinalizedBlockIdentity::new(
            100,
            h(70),
            if effect == XmrNativeEffectV3::Refund {
                15_000
            } else {
                25_000
            },
        ),
        XmrNativeEscrowMetadataFactsV3::from_terms(terms, state),
        NativeCustodyFacts::new(h(6), h(4), balance),
    );
    (
        FinalizedNativeXmrTransactionTargetV3::exact(prepared),
        facts,
    )
}

#[test]
fn all_six_xmr_effects_validate_exact_finalized_facts() {
    let clock = ChainClock::new(h(71), 110, 30_000);
    let window = DiscoveryWindow::new(90, 21).expect("window");
    for effect in [
        XmrNativeEffectV3::Initialize,
        XmrNativeEffectV3::Fund,
        XmrNativeEffectV3::AuthorizeClaim,
        XmrNativeEffectV3::Claim,
        XmrNativeEffectV3::Refund,
        XmrNativeEffectV3::Punish,
    ] {
        let (target, facts) = effect_fixture(effect);
        let result = ClassifyFinalizedNativeXmrEffectV3Result::new(
            context(),
            terms(),
            effect,
            target,
            FinalizedNativeXmrScanOutcomeV3::found(clock, window, facts),
        )
        .expect("valid finalized effect");
        roundtrip(&result);
    }
}

#[test]
fn refund_finalized_facts_enforce_half_open_refund_window() {
    let clock = ChainClock::new(h(71), 110, 30_000);
    let window = DiscoveryWindow::new(90, 21).expect("window");
    for (timestamp_ms, accepted) in [
        (9_999, false),
        (10_000, true),
        (19_999, true),
        (20_000, false),
    ] {
        let (target, mut facts) = effect_fixture(XmrNativeEffectV3::Refund);
        facts.containing_block.timestamp_ms = timestamp_ms;
        let result = ClassifyFinalizedNativeXmrEffectV3Result::new(
            context(),
            terms(),
            XmrNativeEffectV3::Refund,
            target,
            FinalizedNativeXmrScanOutcomeV3::found(clock, window, facts),
        );
        if accepted {
            let _ = result.expect("refund timestamp inside [refund_at, punish_at)");
        } else {
            assert_eq!(
                result,
                Err(ProtocolValueError::XmrFactsMismatch("refund timestamp")),
                "timestamp {timestamp_ms} must be rejected",
            );
        }
    }
}

#[test]
fn punish_finalized_facts_enforce_inclusive_punish_boundary() {
    let clock = ChainClock::new(h(71), 110, 30_000);
    let window = DiscoveryWindow::new(90, 21).expect("window");
    for (timestamp_ms, accepted) in [(19_999, false), (20_000, true), (20_001, true)] {
        let (target, mut facts) = effect_fixture(XmrNativeEffectV3::Punish);
        facts.containing_block.timestamp_ms = timestamp_ms;
        let result = ClassifyFinalizedNativeXmrEffectV3Result::new(
            context(),
            terms(),
            XmrNativeEffectV3::Punish,
            target,
            FinalizedNativeXmrScanOutcomeV3::found(clock, window, facts),
        );
        if accepted {
            let _ = result.expect("punishment timestamp at or after punish_at");
        } else {
            assert_eq!(
                result,
                Err(ProtocolValueError::XmrFactsMismatch("punish timestamp")),
                "timestamp {timestamp_ms} must be rejected",
            );
        }
    }
}

#[test]
fn finalized_outcomes_are_distinct_and_reject_mixed_or_incomplete_evidence() {
    let clock = ChainClock::new(h(71), 110, 30_000);
    let window = DiscoveryWindow::new(90, 21).expect("window");
    let target = FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {};
    for outcome in [
        FinalizedNativeXmrScanOutcomeV3::absent(clock, window),
        FinalizedNativeXmrScanOutcomeV3::uncertain(clock, window),
        FinalizedNativeXmrScanOutcomeV3::unavailable(
            FinalizedNativeXmrUnavailableReasonV3::HistoryUnavailable,
        ),
    ] {
        let result = ClassifyFinalizedNativeXmrEffectV3Result::new(
            context(),
            terms(),
            XmrNativeEffectV3::Claim,
            target.clone(),
            outcome,
        )
        .expect("valid outcome");
        roundtrip(&result);
    }

    let absent = ClassifyFinalizedNativeXmrEffectV3Result::new(
        context(),
        terms(),
        XmrNativeEffectV3::Claim,
        target.clone(),
        FinalizedNativeXmrScanOutcomeV3::absent(clock, window),
    )
    .expect("absent");
    let mut mixed = serde_json::to_value(absent).expect("serialize absent");
    mixed["outcome"]
        .as_object_mut()
        .expect("outcome object")
        .insert("facts".into(), Value::Null);
    assert!(serde_json::from_value::<ClassifyFinalizedNativeXmrEffectV3Result>(mixed).is_err());

    let incomplete_clock = ChainClock::new(h(71), 109, 30_000);
    assert_eq!(
        ClassifyFinalizedNativeXmrEffectV3Result::new(
            context(),
            terms(),
            XmrNativeEffectV3::Claim,
            target,
            FinalizedNativeXmrScanOutcomeV3::absent(incomplete_clock, window),
        ),
        Err(ProtocolValueError::XmrFactsMismatch(
            "finalized window coverage"
        ))
    );
}

#[test]
fn finalized_xmr_facts_reject_wrong_account_order_and_exact_bytes() {
    let clock = ChainClock::new(h(71), 110, 30_000);
    let window = DiscoveryWindow::new(90, 21).expect("window");
    let (target, mut facts) = effect_fixture(XmrNativeEffectV3::Claim);
    facts.instruction.ordered_account_ids =
        AccountIds::new(vec![h(5), h(6), h(10), h(8)]).expect("accounts");
    assert_eq!(
        ClassifyFinalizedNativeXmrEffectV3Result::new(
            context(),
            terms(),
            XmrNativeEffectV3::Claim,
            target,
            FinalizedNativeXmrScanOutcomeV3::found(clock, window, facts),
        ),
        Err(ProtocolValueError::XmrFactsMismatch(
            "instruction account order"
        ))
    );

    let (_, facts) = effect_fixture(XmrNativeEffectV3::Claim);
    assert_eq!(
        ClassifyFinalizedNativeXmrEffectV3Result::new(
            context(),
            terms(),
            XmrNativeEffectV3::Claim,
            FinalizedNativeXmrTransactionTargetV3::exact(prepared(99, 0xff)),
            FinalizedNativeXmrScanOutcomeV3::found(clock, window, facts),
        ),
        Err(ProtocolValueError::XmrFactsMismatch("exact transaction id"))
    );
}
