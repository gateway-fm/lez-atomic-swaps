use lez_bridge_protocol::{
    AccountIds, AggregateBip340Signature, ChainClock, ChainPosition, ChainTip,
    CompleteWitnessedClaimRequest, CompleteWitnessedClaimResult, DescribeRuntimeRequest,
    DescribeRuntimeResult, DiscoveryWindow, ErrorCode, ErrorMessage, EscrowMetadataFacts,
    EscrowObservationTarget, EscrowState, ExactMessageBytes, ExactTransactionBytes,
    FundingFoundFacts, FundingObservation, Hex32, InitializationFoundFacts,
    InitializationObservation, MAX_DISCOVERY_BLOCKS, MessageContext, NativeAmount,
    NativeClaimInstructionFacts, NativeCustodyFacts, NativeEscrowAccountFacts,
    NativeEscrowAccountObservation, NativeEscrowTerms, NativeEscrowTermsInput,
    NativeFundInstructionFacts, NativeInitializeInstructionFacts, NativeRefundFoundFacts,
    NativeRefundInstructionFacts, NativeRefundObservation, NativeRefundObservationTarget,
    ObserveEscrowRequest, ObserveEscrowResult, ObserveNativeRefundRequest,
    ObserveNativeRefundResult, ObserveRevealingClaimRequest, ObserveRevealingClaimResult,
    ObservedTransactionFacts, Participant, PrepareNativeEscrowRequest, PrepareNativeEscrowResult,
    PrepareNativeRefundRequest, PrepareNativeRefundResult, PrepareRevealingClaimRequest,
    PrepareRevealingClaimResult, PrepareWitnessedClaimRequest, PrepareWitnessedClaimResult,
    PrepareWitnessedEscrowRequest, PrepareWitnessedEscrowResult, PreparedTransaction,
    PreparedWitnessedClaim, ProtocolErrorReply, RequestId, RevealingClaimFoundFacts,
    RevealingClaimObservation, RevealingClaimObservationTarget, RevealingPreimage, RunId,
    RuntimeCompatibility, RuntimeDescriptor, SchemaVersion, SubmissionOutcome,
    SubmitTransactionRequest, SubmitTransactionResult, TransactionId, WitnessedNativeEscrowTerms,
    WitnessedNativeEscrowTermsInput,
};

#[test]
fn witnessed_escrow_wire_preserves_exact_terms_and_pair() {
    let terms = WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: h(40),
        terms_hash: h(41),
        depositor: Participant::Taker,
        depositor_account_id: h(42),
        claimant: Participant::Maker,
        claimant_account_id: h(43),
        aggregate_authority_account_id: h(44),
        aggregate_x_only_public_key: h(45),
        amount: 75,
        refund_at_ms: 1_850_000_000_123,
        authenticated_transfer_program_id: h(46),
    })
    .unwrap();
    let context = MessageContext::new(
        RunId::new("witnessed-escrow-run-0001").unwrap(),
        RequestId::new("witnessed-escrow-prepare-0001").unwrap(),
        Participant::Taker,
    );
    let request = PrepareWitnessedEscrowRequest::new(context.clone(), runtime(), terms);
    let result = PrepareWitnessedEscrowResult::new(context, tx(47), tx(48));

    assert_eq!(
        serde_json::from_str::<PrepareWitnessedEscrowRequest>(
            &serde_json::to_string(&request).unwrap()
        )
        .unwrap(),
        request
    );
    assert_eq!(
        serde_json::from_str::<PrepareWitnessedEscrowResult>(
            &serde_json::to_string(&result).unwrap()
        )
        .unwrap(),
        result
    );
}

#[test]
fn witnessed_claim_wire_keeps_destination_authority_and_preparation_identity_distinct() {
    let terms = WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: h(60),
        terms_hash: h(61),
        depositor: Participant::Taker,
        depositor_account_id: h(62),
        claimant: Participant::Maker,
        claimant_account_id: h(63),
        aggregate_authority_account_id: h(64),
        aggregate_x_only_public_key: h(65),
        amount: 75,
        refund_at_ms: 1_850_000_000_123,
        authenticated_transfer_program_id: h(66),
    })
    .unwrap();
    let prepare_context = MessageContext::new(
        RunId::new("witnessed-run-0001").unwrap(),
        RequestId::new("witnessed-prepare-0001").unwrap(),
        Participant::Maker,
    );
    let request = PrepareWitnessedClaimRequest::new(
        prepare_context.clone(),
        runtime(),
        terms,
        TransactionId::from_bytes([67; 32]),
    );
    let prepared = PreparedWitnessedClaim::new(
        prepare_context.request_id.clone(),
        h(68),
        ExactMessageBytes::new(vec![69; 128]).unwrap(),
    );
    let prepare_result =
        PrepareWitnessedClaimResult::new(prepare_context.clone(), prepared.clone());
    assert_eq!(
        serde_json::from_str::<PrepareWitnessedClaimRequest>(
            &serde_json::to_string(&request).unwrap()
        )
        .unwrap(),
        request
    );
    assert_eq!(
        serde_json::from_str::<PrepareWitnessedClaimResult>(
            &serde_json::to_string(&prepare_result).unwrap()
        )
        .unwrap(),
        prepare_result
    );

    let complete_context = MessageContext::new(
        prepare_context.run_id.clone(),
        RequestId::new("witnessed-complete-0001").unwrap(),
        Participant::Maker,
    );
    let signature = AggregateBip340Signature::from_bytes([70; 64]);
    let complete = CompleteWitnessedClaimRequest::new(
        complete_context.clone(),
        runtime(),
        prepared,
        signature,
    );
    let completed = CompleteWitnessedClaimResult::new(complete_context, tx(71));
    assert_eq!(
        serde_json::from_str::<CompleteWitnessedClaimRequest>(
            &serde_json::to_string(&complete).unwrap()
        )
        .unwrap(),
        complete
    );
    assert_eq!(
        serde_json::from_str::<CompleteWitnessedClaimResult>(
            &serde_json::to_string(&completed).unwrap()
        )
        .unwrap(),
        completed
    );
    assert_eq!(
        serde_json::to_value(signature)
            .unwrap()
            .as_str()
            .unwrap()
            .len(),
        128
    );
}

#[test]
fn witnessed_claim_wire_rejects_aliases_noncanonical_signature_and_unknown_fields() {
    assert!(
        WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
            swap_id: h(60),
            terms_hash: h(61),
            depositor: Participant::Taker,
            depositor_account_id: h(62),
            claimant: Participant::Maker,
            claimant_account_id: h(63),
            aggregate_authority_account_id: h(63),
            aggregate_x_only_public_key: h(65),
            amount: 75,
            refund_at_ms: 99,
            authenticated_transfer_program_id: h(66),
        })
        .is_err()
    );
    for signature in ["00".repeat(63), "AA".repeat(64), "gg".repeat(64)] {
        assert!(
            serde_json::from_value::<AggregateBip340Signature>(serde_json::json!(signature))
                .is_err()
        );
    }
    let mut prepared = serde_json::to_value(PreparedWitnessedClaim::new(
        RequestId::new("witnessed-prepare-0001").unwrap(),
        h(68),
        ExactMessageBytes::new(vec![69; 128]).unwrap(),
    ))
    .unwrap();
    prepared["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<PreparedWitnessedClaim>(prepared).is_err());
}

#[test]
fn native_refund_state_exact_and_discovery_are_typed_and_timestamped() {
    let prepare = PrepareNativeRefundRequest::new(context(), runtime(), terms());
    let prepared = PrepareNativeRefundResult::new(context(), tx(40));
    assert_eq!(
        serde_json::from_str::<PrepareNativeRefundRequest>(
            &serde_json::to_string(&prepare).unwrap()
        )
        .unwrap(),
        prepare
    );
    assert_eq!(
        prepared.refund.transaction_id,
        TransactionId::from_bytes([40; 32])
    );

    for target in [
        NativeRefundObservationTarget::StateOnly,
        NativeRefundObservationTarget::Exact {
            refund_transaction_id: TransactionId::from_bytes([40; 32]),
            window: DiscoveryWindow::new(10, 20).unwrap(),
        },
        NativeRefundObservationTarget::DiscoverByTerms {
            window: DiscoveryWindow::new(10, 20).unwrap(),
        },
    ] {
        let request = ObserveNativeRefundRequest::new(context(), runtime(), terms(), target);
        let decoded: ObserveNativeRefundRequest =
            serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
        assert_eq!(decoded, request);
    }

    let clock = ChainClock::new(h(41), 42, 1_800_000_000_123);
    let result = ObserveNativeRefundResult::new(
        context(),
        clock,
        NativeEscrowAccountObservation::Absent,
        NativeRefundObservation::NotRequested,
        clock,
    );
    let decoded: ObserveNativeRefundResult =
        serde_json::from_str(&serde_json::to_string(&result).unwrap()).unwrap();
    assert_eq!(decoded, result);
    assert_eq!(decoded.clock_after.timestamp_ms, 1_800_000_000_123);

    let found = ObserveNativeRefundResult::new(
        context(),
        clock,
        NativeEscrowAccountObservation::found(NativeEscrowAccountFacts::new(
            EscrowMetadataFacts::from_native_terms(
                h(10),
                h(4),
                h(12),
                &terms(),
                EscrowState::Refunded,
            ),
            NativeCustodyFacts::new(h(12), h(22), 0),
        )),
        NativeRefundObservation::found(NativeRefundFoundFacts::new(
            ObservedTransactionFacts::new(
                TransactionId::from_bytes([40; 32]),
                ExactTransactionBytes::new(vec![40; 128]).unwrap(),
                ChainPosition::new(h(41), 42, 3),
                AccountIds::new(Vec::new()).unwrap(),
                true,
            ),
            NativeRefundInstructionFacts::new(
                h(4),
                AccountIds::new(vec![h(10), h(12), h(20)]).unwrap(),
                terms().swap_id(),
            ),
        )),
        clock,
    );
    let decoded: ObserveNativeRefundResult =
        serde_json::from_str(&serde_json::to_string(&found).unwrap()).unwrap();
    assert_eq!(decoded, found);
}

#[test]
fn native_refund_wire_rejects_implicit_windows_ambiguity_and_unknown_fields() {
    let transaction_id = "28".repeat(32);
    for malformed_target in [
        serde_json::json!({"mode": "state_only", "window": {"start_height": 10, "max_blocks": 2}}),
        serde_json::json!({"mode": "exact", "refund_transaction_id": transaction_id}),
        serde_json::json!({"mode": "exact", "refund_transaction_id": transaction_id, "window": {"start_height": 10, "max_blocks": 2}, "surprise": true}),
        serde_json::json!({"mode": "discover_by_terms", "refund_transaction_id": transaction_id, "window": {"start_height": 10, "max_blocks": 2}}),
        serde_json::json!({"mode": "discover_by_terms", "window": {"start_height": 10, "max_blocks": 0}}),
        serde_json::json!({"window": {"start_height": 10, "max_blocks": 2}}),
    ] {
        assert!(serde_json::from_value::<NativeRefundObservationTarget>(malformed_target).is_err());
    }

    let mut request = serde_json::to_value(PrepareNativeRefundRequest::new(
        context(),
        runtime(),
        terms(),
    ))
    .unwrap();
    request["surprise"] = serde_json::json!(true);
    assert!(serde_json::from_value::<PrepareNativeRefundRequest>(request).is_err());

    let mut clock = serde_json::to_value(ChainClock::new(h(41), 42, 1_800_000_000_123)).unwrap();
    clock["timestamp_seconds"] = serde_json::json!(1_800_000_000);
    assert!(serde_json::from_value::<ChainClock>(clock).is_err());

    assert!(
        serde_json::from_value::<NativeRefundObservation>(
            serde_json::json!({"status": "not_requested", "facts": {}}),
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<NativeEscrowAccountObservation>(
            serde_json::json!({"status": "absent", "facts": {}}),
        )
        .is_err()
    );
}

fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn context() -> MessageContext {
    MessageContext::new(
        RunId::new("weekend.m2-run_01").unwrap(),
        RequestId::new("request-0001").unwrap(),
        Participant::Maker,
    )
}

fn runtime() -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        Participant::Maker,
        RuntimeCompatibility::NssaV0_1_2,
        h(1),
        h(2),
        h(3),
        h(4),
        h(5),
    )
}

#[test]
fn runtime_compatibility_wire_is_additive_and_version_exact() {
    for (compatibility, expected_wire) in [
        (RuntimeCompatibility::NssaV0_1_2, "nssa_v0_1_2"),
        (RuntimeCompatibility::LeeV0_2_0, "lee_v0_2_0"),
    ] {
        assert_eq!(
            serde_json::to_string(&compatibility).unwrap(),
            format!("\"{expected_wire}\"")
        );
        assert_eq!(
            serde_json::from_str::<RuntimeCompatibility>(&format!("\"{expected_wire}\"")).unwrap(),
            compatibility
        );
    }

    for unsupported in ["lee_v0_2", "lee_v0_2_1", "nssa_v0_2_0"] {
        assert!(
            serde_json::from_str::<RuntimeCompatibility>(&format!("\"{unsupported}\"")).is_err()
        );
    }
}

fn terms_input() -> NativeEscrowTermsInput {
    NativeEscrowTermsInput {
        swap_id: h(6),
        terms_hash: h(17),
        secret_digest: h(7),
        depositor: Participant::Maker,
        depositor_account_id: h(20),
        claimant: Participant::Taker,
        claimant_account_id: h(21),
        amount: 50,
        refund_at_ms: 99,
        authenticated_transfer_program_id: h(22),
    }
}

fn terms() -> NativeEscrowTerms {
    NativeEscrowTerms::new(terms_input()).unwrap()
}

fn tx(byte: u8) -> PreparedTransaction {
    PreparedTransaction::new(
        TransactionId::from_bytes([byte; 32]),
        ExactTransactionBytes::new(vec![byte; 128]).unwrap(),
    )
}

fn discovery_window() -> DiscoveryWindow {
    DiscoveryWindow::new(1, 128).unwrap()
}

#[test]
fn native_happy_path_messages_roundtrip_without_untyped_json() {
    let describe = DescribeRuntimeRequest::new(context());
    let decoded: DescribeRuntimeRequest =
        serde_json::from_str(&serde_json::to_string(&describe).unwrap()).unwrap();
    assert_eq!(decoded, describe);
    let description = DescribeRuntimeResult::new(context(), runtime());
    let decoded: DescribeRuntimeResult =
        serde_json::from_str(&serde_json::to_string(&description).unwrap()).unwrap();
    assert_eq!(decoded, description);

    let prepare = PrepareNativeEscrowRequest::new(context(), runtime(), terms());
    let prepare_json = serde_json::to_string(&prepare).unwrap();
    let decoded: PrepareNativeEscrowRequest = serde_json::from_str(&prepare_json).unwrap();
    assert_eq!(decoded, prepare);

    let prepared = PrepareNativeEscrowResult::new(context(), tx(8), tx(9));
    let result_json = serde_json::to_string(&prepared).unwrap();
    let decoded: PrepareNativeEscrowResult = serde_json::from_str(&result_json).unwrap();
    assert_eq!(decoded, prepared);

    let observe = ObserveEscrowRequest::new(
        context(),
        runtime(),
        terms(),
        EscrowObservationTarget::Exact {
            initialization_transaction_id: TransactionId::from_bytes([8; 32]),
            funding_transaction_id: TransactionId::from_bytes([9; 32]),
        },
    );
    let observe_json = serde_json::to_string(&observe).unwrap();
    let decoded: ObserveEscrowRequest = serde_json::from_str(&observe_json).unwrap();
    assert_eq!(decoded, observe);

    let initialization = ObservedTransactionFacts::new(
        TransactionId::from_bytes([8; 32]),
        ExactTransactionBytes::new(vec![8; 128]).unwrap(),
        ChainPosition::new(h(11), 41, 0),
        AccountIds::new(vec![h(5)]).unwrap(),
        true,
    );
    let funding = ObservedTransactionFacts::new(
        TransactionId::from_bytes([9; 32]),
        ExactTransactionBytes::new(vec![9; 128]).unwrap(),
        ChainPosition::new(h(11), 41, 1),
        AccountIds::new(vec![h(5)]).unwrap(),
        true,
    );
    let observation = ObserveEscrowResult::new(
        context(),
        ChainTip::new(h(11), 41),
        InitializationObservation::found(InitializationFoundFacts::new(
            initialization,
            NativeInitializeInstructionFacts::new(
                h(4),
                AccountIds::new(vec![h(10), h(12), h(20), h(21)]).unwrap(),
                terms(),
            ),
            EscrowMetadataFacts::from_native_terms(
                h(10),
                h(4),
                h(12),
                &terms(),
                EscrowState::Empty,
            ),
        )),
        FundingObservation::found(FundingFoundFacts::new(
            funding,
            NativeFundInstructionFacts::new(
                h(4),
                AccountIds::new(vec![h(10), h(12), h(20)]).unwrap(),
                terms().swap_id(),
            ),
            EscrowMetadataFacts::from_native_terms(
                h(10),
                h(4),
                h(12),
                &terms(),
                EscrowState::Funded,
            ),
            NativeCustodyFacts::new(h(12), h(22), 50),
        )),
        ChainTip::new(h(11), 41),
    );
    let observation_json = serde_json::to_string(&observation).unwrap();
    let decoded: ObserveEscrowResult = serde_json::from_str(&observation_json).unwrap();
    assert_eq!(decoded, observation);

    let discovery = ObserveEscrowRequest::new(
        context(),
        runtime(),
        terms(),
        EscrowObservationTarget::DiscoverByTerms {
            window: discovery_window(),
        },
    );
    let decoded: ObserveEscrowRequest =
        serde_json::from_str(&serde_json::to_string(&discovery).unwrap()).unwrap();
    assert_eq!(decoded, discovery);
}

#[test]
fn upstream_block_hash_fields_never_masquerade_as_numeric_block_ids() {
    let runtime_json = serde_json::to_value(runtime()).unwrap();
    assert_eq!(
        runtime_json["genesis_block_hash"],
        serde_json::to_value(h(3)).unwrap()
    );
    assert!(runtime_json.get("genesis_block_id").is_none());

    let tip_json = serde_json::to_value(ChainTip::new(h(11), 41)).unwrap();
    assert_eq!(tip_json["block_hash"], serde_json::to_value(h(11)).unwrap());
    assert!(tip_json.get("block_id").is_none());

    let position_json = serde_json::to_value(ChainPosition::new(h(12), 42, 3)).unwrap();
    assert_eq!(
        position_json["block_hash"],
        serde_json::to_value(h(12)).unwrap()
    );
    assert!(position_json.get("block_id").is_none());

    assert!(
        serde_json::from_value::<ChainTip>(
            serde_json::json!({"block_id": "0b".repeat(32), "height": 41}),
        )
        .is_err()
    );
}

#[test]
fn actual_guest_native_terms_are_field_by_field_bound() {
    let expected_terms = terms();
    let expected_terms_json = serde_json::to_value(&expected_terms).unwrap();
    for (field, changed) in [
        ("swap_id", serde_json::to_value(h(30)).unwrap()),
        ("terms_hash", serde_json::to_value(h(31)).unwrap()),
        ("secret_digest", serde_json::to_value(h(32)).unwrap()),
        ("depositor", serde_json::json!("taker")),
        ("depositor_account_id", serde_json::to_value(h(33)).unwrap()),
        ("claimant", serde_json::json!("maker")),
        ("claimant_account_id", serde_json::to_value(h(34)).unwrap()),
        ("amount", serde_json::json!("51")),
        ("refund_at_ms", serde_json::json!(100)),
        (
            "authenticated_transfer_program_id",
            serde_json::to_value(h(35)).unwrap(),
        ),
    ] {
        let mut changed_json = expected_terms_json.as_object().unwrap().clone();
        changed_json.insert(field.into(), changed);
        if let Ok(changed_terms) = serde_json::from_value::<NativeEscrowTerms>(changed_json.into())
        {
            assert_ne!(
                changed_terms, expected_terms,
                "term field {field} was unbound"
            );
        }
    }
    let mut unknown_terms = expected_terms_json.as_object().unwrap().clone();
    unknown_terms.insert("surprise".into(), serde_json::json!(true));
    assert!(serde_json::from_value::<NativeEscrowTerms>(unknown_terms.into()).is_err());
}

#[test]
fn actual_guest_native_metadata_is_field_by_field_bound() {
    let expected_metadata =
        EscrowMetadataFacts::from_native_terms(h(10), h(4), h(12), &terms(), EscrowState::Empty);
    let expected_metadata_json = serde_json::to_value(&expected_metadata).unwrap();
    assert_eq!(expected_metadata_json["version"], serde_json::json!(2));
    assert_eq!(expected_metadata_json["status"], serde_json::json!("empty"));
    assert_eq!(
        expected_metadata_json["owner_program_id"],
        serde_json::to_value(h(4)).unwrap()
    );
    assert_eq!(
        expected_metadata_json["asset_program_id"],
        serde_json::to_value(h(22)).unwrap()
    );
    assert_eq!(
        expected_metadata_json["custody_program_id"],
        serde_json::to_value(h(22)).unwrap()
    );
    assert_eq!(
        expected_metadata_json["depositor_asset_account_id"],
        serde_json::to_value(h(20)).unwrap()
    );
    assert_eq!(
        expected_metadata_json["claimant_asset_account_id"],
        serde_json::to_value(h(21)).unwrap()
    );
    assert_eq!(
        expected_metadata_json["asset_definition"],
        serde_json::to_value(Hex32::from_bytes([0; 32])).unwrap()
    );
    for (field, changed) in [
        ("account_id", serde_json::to_value(h(30)).unwrap()),
        ("owner_program_id", serde_json::to_value(h(31)).unwrap()),
        ("version", serde_json::json!(3)),
        ("swap_id", serde_json::to_value(h(32)).unwrap()),
        ("terms_hash", serde_json::to_value(h(33)).unwrap()),
        ("secret_digest", serde_json::to_value(h(34)).unwrap()),
        ("depositor_account_id", serde_json::to_value(h(35)).unwrap()),
        (
            "depositor_asset_account_id",
            serde_json::to_value(h(36)).unwrap(),
        ),
        ("claimant_account_id", serde_json::to_value(h(37)).unwrap()),
        (
            "claimant_asset_account_id",
            serde_json::to_value(h(38)).unwrap(),
        ),
        ("custody_account_id", serde_json::to_value(h(39)).unwrap()),
        ("asset_program_id", serde_json::to_value(h(40)).unwrap()),
        ("custody_program_id", serde_json::to_value(h(41)).unwrap()),
        ("asset_definition", serde_json::to_value(h(42)).unwrap()),
        ("amount", serde_json::json!("52")),
        ("refund_at_ms", serde_json::json!(101)),
        ("status", serde_json::json!("funded")),
    ] {
        let mut changed_json = expected_metadata_json.as_object().unwrap().clone();
        changed_json.insert(field.into(), changed);
        let changed_metadata: EscrowMetadataFacts =
            serde_json::from_value(changed_json.into()).unwrap();
        assert_ne!(
            changed_metadata, expected_metadata,
            "metadata field {field} was unbound"
        );
    }
    let mut unknown_metadata = expected_metadata_json.as_object().unwrap().clone();
    unknown_metadata.insert("surprise".into(), serde_json::json!(true));
    assert!(serde_json::from_value::<EscrowMetadataFacts>(unknown_metadata.into()).is_err());
}

#[test]
fn actual_guest_native_instruction_fields_and_account_order_are_bound() {
    let initialize = NativeInitializeInstructionFacts::new(
        h(4),
        AccountIds::new(vec![h(10), h(12), h(20), h(21)]).unwrap(),
        terms(),
    );
    assert_eq!(
        initialize.ordered_account_ids.as_slice(),
        &[h(10), h(12), h(20), h(21)]
    );
    let initialize_json = serde_json::to_value(&initialize).unwrap();
    for (field, changed) in [
        ("program_id", serde_json::to_value(h(30)).unwrap()),
        (
            "ordered_account_ids",
            serde_json::to_value(vec![h(12), h(10), h(20), h(21)]).unwrap(),
        ),
        (
            "terms",
            serde_json::to_value(
                NativeEscrowTerms::new(NativeEscrowTermsInput {
                    terms_hash: h(30),
                    ..terms_input()
                })
                .unwrap(),
            )
            .unwrap(),
        ),
    ] {
        let mut changed_json = initialize_json.as_object().unwrap().clone();
        changed_json.insert(field.into(), changed);
        let changed: NativeInitializeInstructionFacts =
            serde_json::from_value(changed_json.into()).unwrap();
        assert_ne!(changed, initialize, "initialize field {field} was unbound");
    }
    let mut unknown_initialize = initialize_json.as_object().unwrap().clone();
    unknown_initialize.insert("surprise".into(), serde_json::json!(true));
    assert!(
        serde_json::from_value::<NativeInitializeInstructionFacts>(unknown_initialize.into())
            .is_err()
    );

    let fund = NativeFundInstructionFacts::new(
        h(4),
        AccountIds::new(vec![h(10), h(12), h(20)]).unwrap(),
        terms().swap_id(),
    );
    assert_eq!(fund.ordered_account_ids.as_slice(), &[h(10), h(12), h(20)]);
    let fund_json = serde_json::to_value(&fund).unwrap();
    for (field, changed) in [
        ("program_id", serde_json::to_value(h(30)).unwrap()),
        (
            "ordered_account_ids",
            serde_json::to_value(vec![h(10), h(12), h(21)]).unwrap(),
        ),
        ("swap_id", serde_json::to_value(h(30)).unwrap()),
    ] {
        let mut changed_json = fund_json.as_object().unwrap().clone();
        changed_json.insert(field.into(), changed);
        let changed: NativeFundInstructionFacts =
            serde_json::from_value(changed_json.into()).unwrap();
        assert_ne!(changed, fund, "fund field {field} was unbound");
    }
    let mut unknown_fund = fund_json.as_object().unwrap().clone();
    unknown_fund.insert("surprise".into(), serde_json::json!(true));
    assert!(serde_json::from_value::<NativeFundInstructionFacts>(unknown_fund.into()).is_err());

    let claim = NativeClaimInstructionFacts::new(
        h(4),
        AccountIds::new(vec![h(10), h(12), h(21)]).unwrap(),
        terms().swap_id(),
        RevealingPreimage::new([0x5a; 32]),
    );
    assert_eq!(claim.ordered_account_ids.as_slice(), &[h(10), h(12), h(21)]);
    let claim_json = serde_json::to_value(&claim).unwrap();
    for (field, changed) in [
        ("program_id", serde_json::to_value(h(30)).unwrap()),
        (
            "ordered_account_ids",
            serde_json::to_value(vec![h(10), h(12), h(20)]).unwrap(),
        ),
        ("swap_id", serde_json::to_value(h(30)).unwrap()),
        ("preimage", serde_json::json!("5b".repeat(32))),
    ] {
        let mut changed_json = claim_json.as_object().unwrap().clone();
        changed_json.insert(field.into(), changed);
        let changed: NativeClaimInstructionFacts =
            serde_json::from_value(changed_json.into()).unwrap();
        assert_ne!(changed, claim, "claim field {field} was unbound");
    }
    let mut unknown_claim = claim_json.as_object().unwrap().clone();
    unknown_claim.insert("surprise".into(), serde_json::json!(true));
    assert!(serde_json::from_value::<NativeClaimInstructionFacts>(unknown_claim.into()).is_err());
}

#[test]
fn observation_states_preserve_absence_and_upstream_uncertainty() {
    let escrow = ObserveEscrowResult::new(
        context(),
        ChainTip::new(h(11), 41),
        InitializationObservation::Absent,
        FundingObservation::UnknownOrPending,
        ChainTip::new(h(11), 41),
    );
    let encoded = serde_json::to_string(&escrow).unwrap();
    assert!(encoded.contains("\"status\":\"absent\""));
    assert!(encoded.contains("\"status\":\"unknown_or_pending\""));
    let decoded: ObserveEscrowResult = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, escrow);

    let inverse = ObserveEscrowResult::new(
        context(),
        ChainTip::new(h(11), 41),
        InitializationObservation::UnknownOrPending,
        FundingObservation::Absent,
        ChainTip::new(h(11), 41),
    );
    let decoded: ObserveEscrowResult =
        serde_json::from_str(&serde_json::to_string(&inverse).unwrap()).unwrap();
    assert_eq!(decoded, inverse);

    let claim = ObserveRevealingClaimResult::new(
        context(),
        ChainTip::new(h(11), 41),
        RevealingClaimObservation::UnknownOrPending,
        ChainTip::new(h(11), 41),
    );
    let encoded = serde_json::to_string(&claim).unwrap();
    let decoded: ObserveRevealingClaimResult = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, claim);

    let absent_claim = ObserveRevealingClaimResult::new(
        context(),
        ChainTip::new(h(11), 41),
        RevealingClaimObservation::Absent,
        ChainTip::new(h(11), 41),
    );
    let decoded: ObserveRevealingClaimResult =
        serde_json::from_str(&serde_json::to_string(&absent_claim).unwrap()).unwrap();
    assert_eq!(decoded, absent_claim);

    let discovery = ObserveRevealingClaimRequest::new(
        context(),
        runtime(),
        terms(),
        RevealingClaimObservationTarget::DiscoverByTerms {
            window: discovery_window(),
        },
    );
    let decoded: ObserveRevealingClaimRequest =
        serde_json::from_str(&serde_json::to_string(&discovery).unwrap()).unwrap();
    assert_eq!(decoded, discovery);

    let ambiguity = ProtocolErrorReply::new(
        context(),
        ErrorCode::AmbiguousDiscovery,
        ErrorMessage::new("multiple transactions match the signed terms").unwrap(),
    );
    let decoded: ProtocolErrorReply =
        serde_json::from_str(&serde_json::to_string(&ambiguity).unwrap()).unwrap();
    assert_eq!(decoded, ambiguity);
}

#[test]
fn native_amounts_roundtrip_above_u64_as_canonical_decimal_strings() {
    let amount = u128::from(u64::MAX) + 7;
    let large_terms = NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: h(6),
        terms_hash: h(17),
        secret_digest: h(7),
        depositor: Participant::Maker,
        depositor_account_id: h(20),
        claimant: Participant::Taker,
        claimant_account_id: h(21),
        amount,
        refund_at_ms: 1_750_000_000_123,
        authenticated_transfer_program_id: h(22),
    })
    .unwrap();
    let encoded = serde_json::to_value(&large_terms).unwrap();
    assert_eq!(encoded["amount"], serde_json::json!(amount.to_string()));
    assert_eq!(
        encoded["refund_at_ms"],
        serde_json::json!(1_750_000_000_123_u64)
    );
    assert!(encoded.get("refund_height").is_none());
    let decoded: NativeEscrowTerms = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.amount().as_u128(), amount);
    assert_eq!(decoded.refund_at_ms(), 1_750_000_000_123);

    let custody = NativeCustodyFacts::new(h(12), h(22), u128::MAX);
    let encoded = serde_json::to_value(&custody).unwrap();
    assert_eq!(encoded["balance"], serde_json::json!(u128::MAX.to_string()));
    let decoded: NativeCustodyFacts = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.balance.as_u128(), u128::MAX);

    assert!(serde_json::from_str::<NativeAmount>("1").is_err());
    assert!(serde_json::from_str::<NativeAmount>("\"01\"").is_err());
    assert!(
        serde_json::from_str::<NativeAmount>("\"340282366920938463463374607431768211456\"")
            .is_err()
    );
}

#[test]
fn discovery_windows_are_explicit_nonzero_and_hard_capped() {
    assert!(DiscoveryWindow::new(1, 0).is_err());
    assert!(DiscoveryWindow::new(1, MAX_DISCOVERY_BLOCKS + 1).is_err());

    let window = DiscoveryWindow::new(42, MAX_DISCOVERY_BLOCKS).unwrap();
    let decoded: DiscoveryWindow =
        serde_json::from_str(&serde_json::to_string(&window).unwrap()).unwrap();
    assert_eq!(decoded, window);
    assert_eq!(decoded.start_height(), 42);
    assert_eq!(decoded.max_blocks(), MAX_DISCOVERY_BLOCKS);

    assert!(
        serde_json::from_str::<DiscoveryWindow>(r#"{"start_height":1,"max_blocks":0}"#).is_err()
    );
    assert!(
        serde_json::from_str::<DiscoveryWindow>(r#"{"start_height":1,"max_blocks":4097}"#,)
            .is_err()
    );
}

#[test]
fn revealing_claim_secret_is_redacted_but_roundtrips() {
    let preimage = RevealingPreimage::new([0x5a; 32]);
    assert_eq!(format!("{preimage:?}"), "RevealingPreimage([REDACTED])");

    let request = PrepareRevealingClaimRequest::new(
        context(),
        runtime(),
        terms(),
        TransactionId::from_bytes([9; 32]),
        preimage,
    );
    let encoded = serde_json::to_string(&request).unwrap();
    assert!(!encoded.contains("REDACTED"));
    let decoded: PrepareRevealingClaimRequest = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.preimage().expose_secret(), &[0x5a; 32]);

    let result = PrepareRevealingClaimResult::new(context(), tx(13));
    let decoded: PrepareRevealingClaimResult =
        serde_json::from_str(&serde_json::to_string(&result).unwrap()).unwrap();
    assert_eq!(decoded, result);

    let observe = ObserveRevealingClaimRequest::new(
        context(),
        runtime(),
        terms(),
        RevealingClaimObservationTarget::Exact {
            claim_transaction_id: TransactionId::from_bytes([13; 32]),
        },
    );
    let decoded: ObserveRevealingClaimRequest =
        serde_json::from_str(&serde_json::to_string(&observe).unwrap()).unwrap();
    assert_eq!(decoded, observe);

    let observed = ObserveRevealingClaimResult::new(
        context(),
        ChainTip::new(h(16), 42),
        RevealingClaimObservation::found(RevealingClaimFoundFacts::new(
            ObservedTransactionFacts::new(
                TransactionId::from_bytes([13; 32]),
                ExactTransactionBytes::new(vec![13; 128]).unwrap(),
                ChainPosition::new(h(16), 42, 0),
                AccountIds::new(vec![h(5)]).unwrap(),
                true,
            ),
            NativeClaimInstructionFacts::new(
                h(4),
                AccountIds::new(vec![h(10), h(12), h(21)]).unwrap(),
                terms().swap_id(),
                RevealingPreimage::new([0x5a; 32]),
            ),
            EscrowMetadataFacts::from_native_terms(
                h(10),
                h(4),
                h(12),
                &terms(),
                EscrowState::Claimed,
            ),
            NativeCustodyFacts::new(h(12), h(22), 0),
        )),
        ChainTip::new(h(16), 42),
    );
    let encoded = serde_json::to_string(&observed).unwrap();
    let decoded: ObserveRevealingClaimResult = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, observed);
}

#[test]
fn exact_submit_and_typed_error_replies_roundtrip() {
    let request = SubmitTransactionRequest::new(context(), runtime(), tx(14));
    let decoded: SubmitTransactionRequest =
        serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
    assert_eq!(decoded, request);

    let result = SubmitTransactionResult::new(
        context(),
        TransactionId::from_bytes([14; 32]),
        SubmissionOutcome::Accepted,
    );
    let decoded: SubmitTransactionResult =
        serde_json::from_str(&serde_json::to_string(&result).unwrap()).unwrap();
    assert_eq!(decoded, result);

    let error = ProtocolErrorReply::new(
        context(),
        ErrorCode::InvalidRequest,
        ErrorMessage::new("wrong sidecar role").unwrap(),
    );
    let decoded: ProtocolErrorReply =
        serde_json::from_str(&serde_json::to_string(&error).unwrap()).unwrap();
    assert_eq!(decoded, error);
}

#[test]
fn rejects_invalid_versions_identifiers_hex_unknown_fields_and_semantics() {
    for bad in [
        "short",
        "contains space",
        "non/ascii",
        "1234567",
        &"x".repeat(65),
    ] {
        assert!(RunId::new(bad).is_err(), "accepted invalid run id {bad:?}");
    }
    assert!(RequestId::new("bad/request").is_err());
    assert!(Hex32::from_hex(&"AA".repeat(32)).is_err());
    assert!(Hex32::from_hex("00").is_err());
    assert!(
        NativeEscrowTerms::new(NativeEscrowTermsInput {
            claimant: Participant::Maker,
            ..terms_input()
        })
        .is_err()
    );
    assert!(
        NativeEscrowTerms::new(NativeEscrowTermsInput {
            amount: 0,
            ..terms_input()
        })
        .is_err()
    );

    let json = serde_json::to_value(PrepareNativeEscrowRequest::new(
        context(),
        runtime(),
        terms(),
    ))
    .unwrap();
    let mut object = json.as_object().unwrap().clone();
    object.insert("surprise".into(), serde_json::json!(true));
    assert!(serde_json::from_value::<PrepareNativeEscrowRequest>(object.into()).is_err());

    assert!(
        serde_json::from_str::<EscrowObservationTarget>(
            r#"{"mode":"discover_by_terms","window":{"start_height":1,"max_blocks":8},"surprise":true}"#,
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<InitializationObservation>(r#"{"status":"absent","facts":{}}"#,)
            .is_err()
    );

    let mut object = json.as_object().unwrap().clone();
    object.get_mut("context").unwrap()["schema_version"] = serde_json::json!(2);
    assert!(serde_json::from_value::<PrepareNativeEscrowRequest>(object.into()).is_err());
    assert_eq!(
        serde_json::to_string(&SchemaVersion::current()).unwrap(),
        "1"
    );
}

#[test]
fn rejects_oversize_bytes_accounts_signers_and_error_messages_during_decode() {
    assert!(ExactTransactionBytes::new(vec![0; 2_000_001]).is_err());
    assert!(ExactTransactionBytes::new(Vec::new()).is_err());
    assert!(AccountIds::new(vec![h(1); 17]).is_err());
    assert!(ErrorMessage::new("x".repeat(257)).is_err());

    let tx_json = serde_json::to_value(tx(15)).unwrap();
    let mut oversized = tx_json.as_object().unwrap().clone();
    oversized.insert(
        "exact_bytes".into(),
        serde_json::to_value(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            vec![1; 2_000_001],
        ))
        .unwrap(),
    );
    assert!(serde_json::from_value::<PreparedTransaction>(oversized.into()).is_err());

    let signers = serde_json::to_value(vec!["01".repeat(32); 17]).unwrap();
    assert!(serde_json::from_value::<AccountIds>(signers).is_err());
    assert!(serde_json::from_value::<ErrorMessage>(serde_json::json!("x".repeat(257))).is_err());
}
