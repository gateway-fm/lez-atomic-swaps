use lez_bridge_protocol::{
    AccountIds, AggregateBip340Signature, ChainClock, ChainPosition,
    ClassifyFinalizedNativeXmrEffectV3Request, ClassifyFinalizedNativeXmrEffectV3Result,
    CompleteNativeXmrClaimV3Request, CompleteNativeXmrClaimV3Result,
    CompleteNativeXmrRefundV3Request, CompleteNativeXmrRefundV3Result, DiscoveryWindow,
    ExactMessageBytes, ExactTransactionBytes, FinalizedBlockIdentity,
    FinalizedNativeXmrEffectFactsV3, FinalizedNativeXmrScanOutcomeV3,
    FinalizedNativeXmrTransactionTargetV3, FinalizedNativeXmrUnavailableReasonV3, Hex32,
    METHOD_CLASSIFY_FINALIZED_NATIVE_XMR_EFFECT_V3, METHOD_COMPLETE_NATIVE_XMR_CLAIM_V3,
    METHOD_COMPLETE_NATIVE_XMR_REFUND_V3, METHOD_PREPARE_NATIVE_XMR_CLAIM_AUTHORIZATION_V3,
    METHOD_PREPARE_NATIVE_XMR_CLAIM_V3, METHOD_PREPARE_NATIVE_XMR_ESCROW_V3,
    METHOD_PREPARE_NATIVE_XMR_PUNISH_V3, METHOD_PREPARE_NATIVE_XMR_REFUND_V3, MessageContext,
    NativeCustodyFacts, ObservedTransactionFacts, Participant,
    PrepareNativeXmrClaimAuthorizationV3Request, PrepareNativeXmrClaimAuthorizationV3Result,
    PrepareNativeXmrClaimV3Request, PrepareNativeXmrClaimV3Result, PrepareNativeXmrEscrowV3Request,
    PrepareNativeXmrEscrowV3Result, PrepareNativeXmrPunishV3Request,
    PrepareNativeXmrPunishV3Result, PrepareNativeXmrRefundV3Request,
    PrepareNativeXmrRefundV3Result, PreparedTransaction, PreparedWitnessedClaim,
    ProtocolValueError, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor, TransactionId,
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

#[test]
fn all_eight_xmr_v3_method_families_roundtrip_strict_json() {
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

    let partial = XmrClaimPartialV3::new([0x77; 32]).expect("claim partial");
    assert_eq!(format!("{partial:?}"), "XmrClaimPartialV3([REDACTED])");
    assert_eq!(
        serde_json::to_value(&partial).expect("serialize partial"),
        Value::String("77".repeat(32))
    );
    roundtrip(&PrepareNativeXmrClaimAuthorizationV3Request::new(
        context.clone(),
        runtime.clone(),
        terms,
        partial,
    ));
    roundtrip(&PrepareNativeXmrClaimAuthorizationV3Result::new(
        context.clone(),
        terms,
        prepared(55, 0xc6),
    ));

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
fn xmr_v3_messages_reject_unknown_fields_and_wrong_agreement_hashes() {
    let request = PrepareNativeXmrClaimV3Request::new(context(), runtime(), terms());
    let mut encoded = serde_json::to_value(request).expect("serialize request");
    encoded
        .as_object_mut()
        .expect("object")
        .insert("legacy_terms".into(), Value::Null);
    assert!(serde_json::from_value::<PrepareNativeXmrClaimV3Request>(encoded).is_err());

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
        FinalizedBlockIdentity::new(100, h(70), 25_000),
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
