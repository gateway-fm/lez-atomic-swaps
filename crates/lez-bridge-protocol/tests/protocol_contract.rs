use lez_bridge_protocol::{
    AccountIds, AggregateBip340Signature, ChainClock, ChainPosition, ChainTip,
    ClassifyFinalizedWitnessedClaimResult, ClassifyFinalizedWitnessedFundingResult,
    ClassifyFinalizedWitnessedInitializationRequest,
    ClassifyFinalizedWitnessedInitializationResult, CompleteWitnessedAssetClaimV2Request,
    CompleteWitnessedAssetClaimV2Result, CompleteWitnessedClaimRequest,
    CompleteWitnessedClaimResult, DescribeRuntimeRequest, DescribeRuntimeResult, DiscoveryWindow,
    ErrorCode, ErrorMessage, EscrowMetadataFacts, EscrowObservationTarget, EscrowState,
    ExactMessageBytes, ExactTransactionBytes, FinalizedBlockIdentity,
    FinalizedWitnessedAssetClaimFactsV2, FinalizedWitnessedClaimFacts,
    FinalizedWitnessedClaimObservationTarget, FinalizedWitnessedClaimScanOutcome,
    FinalizedWitnessedFundingFacts, FinalizedWitnessedFundingObservationTarget,
    FinalizedWitnessedFundingScanOutcome, FinalizedWitnessedInitializationFacts,
    FinalizedWitnessedInitializationScanOutcome, FundingFoundFacts, FundingObservation, Hex32,
    InitializationFoundFacts, InitializationObservation, MAX_DISCOVERY_BLOCKS,
    METHOD_COMPLETE_WITNESSED_ASSET_CLAIM_V2, METHOD_COMPLETE_WITNESSED_CLAIM,
    METHOD_OBSERVE_FINALIZED_WITNESSED_ASSET_CLAIM_V2, METHOD_OBSERVE_WITNESSED_ASSET_ESCROW_V2,
    METHOD_OBSERVE_WITNESSED_ASSET_REFUND_V2, METHOD_OBSERVE_WITNESSED_ESCROW,
    METHOD_PREPARE_WITNESSED_ASSET_CLAIM_V2, METHOD_PREPARE_WITNESSED_ASSET_ESCROW_V2,
    METHOD_PREPARE_WITNESSED_ASSET_REFUND_V2, METHOD_PREPARE_WITNESSED_CLAIM,
    METHOD_PREPARE_WITNESSED_ESCROW, MessageContext, NativeAmount, NativeClaimInstructionFacts,
    NativeCustodyFacts, NativeEscrowAccountFacts, NativeEscrowAccountObservation,
    NativeEscrowTerms, NativeEscrowTermsInput, NativeFundInstructionFacts,
    NativeInitializeInstructionFacts, NativeRefundFoundFacts, NativeRefundInstructionFacts,
    NativeRefundObservation, NativeRefundObservationTarget, ObserveCurrentClockRequest,
    ObserveCurrentClockResult, ObserveEscrowRequest, ObserveEscrowResult,
    ObserveFinalizedClockRequest, ObserveFinalizedClockResult,
    ObserveFinalizedWitnessedAssetClaimV2Request, ObserveFinalizedWitnessedAssetClaimV2Result,
    ObserveFinalizedWitnessedClaimRequest, ObserveFinalizedWitnessedClaimResult,
    ObserveFinalizedWitnessedFundingRequest, ObserveFinalizedWitnessedFundingResult,
    ObserveNativeRefundRequest, ObserveNativeRefundResult, ObserveRevealingClaimRequest,
    ObserveRevealingClaimResult, ObserveWitnessedAssetEscrowV2Request,
    ObserveWitnessedAssetEscrowV2Result, ObserveWitnessedAssetRefundV2Request,
    ObserveWitnessedAssetRefundV2Result, ObserveWitnessedEscrowRequest,
    ObserveWitnessedEscrowResult, ObservedTransactionFacts, Participant,
    PrepareNativeEscrowRequest, PrepareNativeEscrowResult, PrepareNativeRefundRequest,
    PrepareNativeRefundResult, PrepareRevealingClaimRequest, PrepareRevealingClaimResult,
    PrepareWitnessedAssetClaimV2Request, PrepareWitnessedAssetClaimV2Result,
    PrepareWitnessedAssetEscrowV2Request, PrepareWitnessedAssetEscrowV2Result,
    PrepareWitnessedAssetRefundV2Request, PrepareWitnessedAssetRefundV2Result,
    PrepareWitnessedClaimRequest, PrepareWitnessedClaimResult, PrepareWitnessedEscrowRequest,
    PrepareWitnessedEscrowResult, PreparedTransaction, PreparedWitnessedClaim, ProtocolErrorReply,
    RequestId, RevealingClaimFoundFacts, RevealingClaimObservation,
    RevealingClaimObservationTarget, RevealingPreimage, RunId, RuntimeCompatibility,
    RuntimeDescriptor, SchemaVersion, SubmissionOutcome, SubmitTransactionRequest,
    SubmitTransactionResult, TokenHoldingFactsV2, TransactionId, WITNESSED_LEZ_ASSET_TERMS_VERSION,
    WitnessedAssetClaimInstructionFactsV2, WitnessedAssetCustodyFactsV2,
    WitnessedAssetObservedPrepareEffectV2, WitnessedAssetPrepareStepV2,
    WitnessedAssetPreparedEffectV2, WitnessedAssetRefundFoundFactsV2,
    WitnessedAssetRefundInstructionFactsV2, WitnessedAssetRefundObservationV2,
    WitnessedClaimInstructionFacts, WitnessedEscrowMetadataFacts, WitnessedFundingFoundFacts,
    WitnessedFundingObservation, WitnessedInitializationFoundFacts,
    WitnessedInitializationObservation, WitnessedLezAssetTermsV2, WitnessedLezAssetV2,
    WitnessedNativeEscrowTerms, WitnessedNativeEscrowTermsInput,
    WitnessedNativeInitializeInstructionFacts, WitnessedTokenEscrowTermsV2,
    WitnessedTokenEscrowTermsV2Input,
};
use lez_bridge_protocol::{
    ClassifyFinalizedWitnessedAssetClaimV2Request, ClassifyFinalizedWitnessedAssetClaimV2Result,
    ClassifyFinalizedWitnessedAssetCustodyCreationV2Request,
    ClassifyFinalizedWitnessedAssetCustodyCreationV2Result,
    ClassifyFinalizedWitnessedAssetFundingV2Request,
    ClassifyFinalizedWitnessedAssetFundingV2Result,
    ClassifyFinalizedWitnessedAssetInitializationV2Request,
    ClassifyFinalizedWitnessedAssetInitializationV2Result,
    FinalizedWitnessedAssetCustodyCreationFactsV2, FinalizedWitnessedAssetFundingFactsV2,
    FinalizedWitnessedAssetInitializationFactsV2, FinalizedWitnessedAssetTransactionTargetV2,
    FinalizedWitnessedAssetUnavailableReasonV2, METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CLAIM_V2,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CUSTODY_CREATION_V2,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_FUNDING_V2,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_INITIALIZATION_V2,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_CLAIM, METHOD_CLASSIFY_FINALIZED_WITNESSED_FUNDING,
    METHOD_CLASSIFY_FINALIZED_WITNESSED_INITIALIZATION, METHOD_OBSERVE_FINALIZED_CLOCK,
    WitnessedAssetEffectInstructionFactsV2, WitnessedAssetInitializationCustodyFactsV2,
};

#[test]
fn current_clock_wire_is_strict_and_runtime_bound() {
    let request = ObserveCurrentClockRequest::new(context(), runtime());
    let result = ObserveCurrentClockResult::new(
        request.context.clone(),
        request.runtime.clone(),
        ChainClock::new(h(94), 95, 1_850_000_001_500),
    );

    assert_eq!(
        serde_json::from_value::<ObserveCurrentClockRequest>(
            serde_json::to_value(&request).unwrap()
        )
        .unwrap(),
        request
    );
    assert_eq!(
        serde_json::from_value::<ObserveCurrentClockResult>(serde_json::to_value(&result).unwrap())
            .unwrap(),
        result
    );

    let mut invalid_request = serde_json::to_value(&request).unwrap();
    invalid_request["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ObserveCurrentClockRequest>(invalid_request).is_err());
    let mut invalid_result = serde_json::to_value(result).unwrap();
    invalid_result["clock"]["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ObserveCurrentClockResult>(invalid_result).is_err());
}

#[test]
fn finalized_clock_wire_is_strict_and_runtime_bound() {
    assert_eq!(
        METHOD_OBSERVE_FINALIZED_CLOCK,
        "lez_bridge.v1.observe_finalized_clock"
    );
    let request = ObserveFinalizedClockRequest::new(context(), runtime());
    let result = ObserveFinalizedClockResult::new(
        request.context.clone(),
        request.runtime.clone(),
        ChainClock::new(h(96), 97, 1_850_000_001_600),
    );
    assert_eq!(
        serde_json::from_value::<ObserveFinalizedClockResult>(
            serde_json::to_value(&result).unwrap()
        )
        .unwrap(),
        result
    );
    let mut invalid_request = serde_json::to_value(request).unwrap();
    invalid_request["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ObserveFinalizedClockRequest>(invalid_request).is_err());
    let mut invalid_result = serde_json::to_value(result).unwrap();
    invalid_result["clock"]["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ObserveFinalizedClockResult>(invalid_result).is_err());
}

#[test]
fn finalized_witnessed_initialization_classifier_is_exact_strict_and_three_way() {
    let terms = WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: h(80),
        terms_hash: h(81),
        depositor: Participant::Maker,
        depositor_account_id: h(82),
        claimant: Participant::Taker,
        claimant_account_id: h(83),
        aggregate_authority_account_id: h(84),
        aggregate_x_only_public_key: h(85),
        amount: 125,
        refund_at_ms: 1_850_000_001_123,
        authenticated_transfer_program_id: h(86),
    })
    .unwrap();
    let initialization = PreparedTransaction::new(
        TransactionId::from_bytes([87; 32]),
        ExactTransactionBytes::new(vec![88; 128]).unwrap(),
    );
    let window = DiscoveryWindow::new(89, 3).unwrap();
    let request = ClassifyFinalizedWitnessedInitializationRequest::new(
        context(),
        runtime(),
        terms.clone(),
        initialization.clone(),
        TransactionId::from_bytes([89; 32]),
        window,
    );
    let facts = FinalizedWitnessedInitializationFacts::new(
        ObservedTransactionFacts::new(
            initialization.transaction_id,
            initialization.exact_bytes.clone(),
            ChainPosition::new(h(90), 89, 2),
            AccountIds::new(vec![h(82)]).unwrap(),
            true,
        ),
        WitnessedNativeInitializeInstructionFacts::new(
            h(4),
            AccountIds::new(vec![h(91), h(92), h(82), h(83), h(84)]).unwrap(),
            terms.clone(),
        ),
        FinalizedBlockIdentity::new(89, h(90), 1_850_000_001_456),
        WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
            h(91),
            h(4),
            h(92),
            &terms,
            EscrowState::Empty,
        ),
        NativeCustodyFacts::new(h(92), h(86), 0),
    );
    let found = ClassifyFinalizedWitnessedInitializationResult::found(
        request.context.clone(),
        ChainClock::new(h(93), 91, 1_850_000_001_470),
        window,
        facts,
    );
    let absent = ClassifyFinalizedWitnessedInitializationResult::absent(
        request.context.clone(),
        ChainClock::new(h(93), 91, 1_850_000_001_470),
        window,
    );
    let uncertain = ClassifyFinalizedWitnessedInitializationResult::uncertain(
        request.context.clone(),
        ChainClock::new(h(93), 91, 1_850_000_001_470),
        window,
    );

    assert_eq!(
        serde_json::from_value::<ClassifyFinalizedWitnessedInitializationRequest>(
            serde_json::to_value(&request).unwrap()
        )
        .unwrap(),
        request
    );
    for result in [found, absent, uncertain] {
        assert_eq!(
            serde_json::from_value::<ClassifyFinalizedWitnessedInitializationResult>(
                serde_json::to_value(&result).unwrap()
            )
            .unwrap(),
            result
        );
    }
    assert!(matches!(
        ClassifyFinalizedWitnessedInitializationResult::uncertain(
            context(),
            ChainClock::new(h(93), 91, 1_850_000_001_470),
            window,
        )
        .outcome,
        FinalizedWitnessedInitializationScanOutcome::Uncertain {}
    ));
    let mut invalid = serde_json::to_value(request).unwrap();
    invalid["initialization"]["unexpected"] = serde_json::json!(true);
    assert!(
        serde_json::from_value::<ClassifyFinalizedWitnessedInitializationRequest>(invalid).is_err()
    );
}

#[test]
fn finalized_witnessed_funding_wire_is_strict_bounded_and_complete() {
    let terms = WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: h(60),
        terms_hash: h(61),
        depositor: Participant::Maker,
        depositor_account_id: h(62),
        claimant: Participant::Taker,
        claimant_account_id: h(63),
        aggregate_authority_account_id: h(64),
        aggregate_x_only_public_key: h(65),
        amount: 125,
        refund_at_ms: 1_850_000_001_123,
        authenticated_transfer_program_id: h(66),
    })
    .unwrap();
    let request = ObserveFinalizedWitnessedFundingRequest::new(
        context(),
        runtime(),
        terms.clone(),
        TransactionId::from_bytes([67; 32]),
        DiscoveryWindow::new(68, 3).unwrap(),
    );
    assert_eq!(
        request.target,
        FinalizedWitnessedFundingObservationTarget::Exact {
            funding_transaction_id: TransactionId::from_bytes([67; 32])
        }
    );
    let funding = finalized_witnessed_funding_facts(&terms);
    let result =
        ObserveFinalizedWitnessedFundingResult::new(context(), ChainTip::new(h(73), 70), funding);

    let request_json = serde_json::to_value(&request).unwrap();
    assert_eq!(
        serde_json::from_value::<ObserveFinalizedWitnessedFundingRequest>(request_json.clone())
            .unwrap(),
        request
    );
    assert_eq!(
        serde_json::from_value::<ObserveFinalizedWitnessedFundingResult>(
            serde_json::to_value(&result).unwrap()
        )
        .unwrap(),
        result
    );

    let discovery = ObserveFinalizedWitnessedFundingRequest::discover_by_terms(
        context(),
        runtime(),
        terms,
        DiscoveryWindow::new(68, MAX_DISCOVERY_BLOCKS).unwrap(),
    );
    assert_eq!(
        discovery.target,
        FinalizedWitnessedFundingObservationTarget::DiscoverByTerms
    );
    assert_eq!(
        serde_json::to_value(&discovery).unwrap()["target"],
        serde_json::json!({"mode": "discover_by_terms"})
    );

    for invalid_target in [
        serde_json::json!({
            "mode": "discover_by_terms",
            "funding_transaction_id": "43".repeat(32)
        }),
        serde_json::json!({"mode": "exact"}),
        serde_json::json!({
            "mode": "exact",
            "funding_transaction_id": "43".repeat(32),
            "unexpected": true
        }),
    ] {
        let mut invalid = request_json.clone();
        invalid["target"] = invalid_target;
        assert!(
            serde_json::from_value::<ObserveFinalizedWitnessedFundingRequest>(invalid).is_err()
        );
    }

    for max_blocks in [0, MAX_DISCOVERY_BLOCKS + 1] {
        let mut invalid = request_json.clone();
        invalid["window"]["max_blocks"] = serde_json::json!(max_blocks);
        assert!(
            serde_json::from_value::<ObserveFinalizedWitnessedFundingRequest>(invalid).is_err()
        );
    }

    let mut unknown_result = serde_json::to_value(result).unwrap();
    unknown_result["funding"]["unexpected"] = serde_json::json!(true);
    assert!(
        serde_json::from_value::<ObserveFinalizedWitnessedFundingResult>(unknown_result).is_err()
    );
}

#[test]
fn finalized_witnessed_funding_classifier_wire_preserves_three_way_semantics() {
    let terms = WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: h(60),
        terms_hash: h(61),
        depositor: Participant::Maker,
        depositor_account_id: h(62),
        claimant: Participant::Taker,
        claimant_account_id: h(63),
        aggregate_authority_account_id: h(64),
        aggregate_x_only_public_key: h(65),
        amount: 125,
        refund_at_ms: 1_850_000_001_123,
        authenticated_transfer_program_id: h(66),
    })
    .unwrap();
    let window = DiscoveryWindow::new(68, 3).unwrap();
    let found = ClassifyFinalizedWitnessedFundingResult::found(
        context(),
        ChainClock::new(h(73), 70, 1_850_000_001_470),
        window,
        finalized_witnessed_funding_facts(&terms),
    );
    let absent = ClassifyFinalizedWitnessedFundingResult::absent(
        context(),
        ChainClock::new(h(73), 70, 1_850_000_001_470),
        window,
    );
    let uncertain = ClassifyFinalizedWitnessedFundingResult::uncertain(
        context(),
        ChainClock::new(h(73), 70, 1_850_000_001_470),
        window,
    );

    assert!(matches!(
        found.outcome,
        FinalizedWitnessedFundingScanOutcome::Found { .. }
    ));
    assert_eq!(
        serde_json::to_value(&absent).unwrap()["outcome"],
        serde_json::json!({"status": "absent"})
    );
    assert_eq!(
        serde_json::to_value(&uncertain).unwrap()["outcome"],
        serde_json::json!({"status": "uncertain"})
    );
    assert_eq!(
        serde_json::from_value::<ClassifyFinalizedWitnessedFundingResult>(
            serde_json::to_value(found).unwrap()
        )
        .unwrap()
        .finalized_clock
        .timestamp_ms,
        1_850_000_001_470
    );
    assert_eq!(
        serde_json::from_value::<ClassifyFinalizedWitnessedFundingResult>(
            serde_json::to_value(ClassifyFinalizedWitnessedFundingResult::absent(
                context(),
                ChainClock::new(h(73), 70, 1_850_000_001_470),
                window,
            ))
            .unwrap()
        )
        .unwrap()
        .scanned_window,
        window
    );

    let mut unknown = serde_json::to_value(absent).unwrap();
    unknown["outcome"]["reason"] = serde_json::json!("rpc_failed");
    assert!(
        serde_json::from_value::<ClassifyFinalizedWitnessedFundingResult>(unknown).is_err(),
        "failures must not fit the affirmative-absence wire shape"
    );
}

fn finalized_witnessed_funding_facts(
    terms: &WitnessedNativeEscrowTerms,
) -> FinalizedWitnessedFundingFacts {
    FinalizedWitnessedFundingFacts::new(
        ObservedTransactionFacts::new(
            TransactionId::from_bytes([67; 32]),
            ExactTransactionBytes::new(vec![69; 128]).unwrap(),
            ChainPosition::new(h(70), 69, 2),
            AccountIds::new(vec![h(62)]).unwrap(),
            true,
        ),
        NativeFundInstructionFacts::new(
            h(4),
            AccountIds::new(vec![h(71), h(72), h(62)]).unwrap(),
            h(60),
        ),
        FinalizedBlockIdentity::new(69, h(70), 1_850_000_001_456),
        WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
            h(71),
            h(4),
            h(72),
            terms,
            EscrowState::Funded,
        ),
        NativeCustodyFacts::new(h(72), h(66), 125),
    )
}

#[test]
fn finalized_witnessed_claim_wire_binds_exact_transcript_witness_and_block() {
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
    let claim = PreparedWitnessedClaim::new(
        RequestId::new("witnessed-claim-prepare-0001").unwrap(),
        h(47),
        ExactMessageBytes::new(vec![48; 128]).unwrap(),
    );
    let request = ObserveFinalizedWitnessedClaimRequest::new(
        context(),
        runtime(),
        terms.clone(),
        claim.clone(),
        TransactionId::from_bytes([49; 32]),
        DiscoveryWindow::new(50, 3).unwrap(),
    );
    let transaction = ObservedTransactionFacts::new(
        TransactionId::from_bytes([49; 32]),
        ExactTransactionBytes::new(vec![51; 128]).unwrap(),
        ChainPosition::new(h(52), 51, 2),
        AccountIds::new(vec![h(44)]).unwrap(),
        true,
    );
    let result = ObserveFinalizedWitnessedClaimResult::new(
        context(),
        ChainTip::new(h(57), 52),
        FinalizedWitnessedClaimFacts::new(
            transaction,
            WitnessedClaimInstructionFacts::new(
                h(4),
                AccountIds::new(vec![h(53), h(54), h(43), h(44)]).unwrap(),
                h(40),
                h(43),
                h(44),
                claim.clone(),
            ),
            AggregateBip340Signature::from_bytes([55; 64]),
            FinalizedBlockIdentity::new(51, h(52), 1_850_000_000_456),
            WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
                h(53),
                h(4),
                h(54),
                &terms,
                EscrowState::Claimed,
            ),
            NativeCustodyFacts::new(h(54), h(46), 0),
        ),
    );

    assert_eq!(
        serde_json::from_str::<ObserveFinalizedWitnessedClaimRequest>(
            &serde_json::to_string(&request).unwrap()
        )
        .unwrap(),
        request
    );
    assert_eq!(
        serde_json::from_str::<ObserveFinalizedWitnessedClaimResult>(
            &serde_json::to_string(&result).unwrap()
        )
        .unwrap(),
        result
    );
}

#[test]
fn finalized_witnessed_claim_presence_wire_is_strict_and_carries_three_way_coverage() {
    let window = DiscoveryWindow::new(50, 3).unwrap();
    let not_found = ClassifyFinalizedWitnessedClaimResult::not_found(
        context(),
        ChainTip::new(h(57), 52),
        window,
    );
    assert_eq!(
        not_found.outcome,
        FinalizedWitnessedClaimScanOutcome::NotFound
    );
    assert_eq!(not_found.scanned_window, window);
    assert_eq!(
        serde_json::to_value(&not_found).unwrap()["outcome"],
        serde_json::json!({"status": "not_found"})
    );
    assert_eq!(
        serde_json::from_value::<ClassifyFinalizedWitnessedClaimResult>(
            serde_json::to_value(&not_found).unwrap()
        )
        .unwrap(),
        not_found
    );

    let prefix = DiscoveryWindow::new(50, 2).unwrap();
    let uncertain = ClassifyFinalizedWitnessedClaimResult::uncertain(
        context(),
        ChainTip::new(h(58), 51),
        prefix,
    );
    assert_eq!(
        uncertain.outcome,
        FinalizedWitnessedClaimScanOutcome::Uncertain {}
    );
    assert_eq!(uncertain.scanned_window, prefix);
    assert_eq!(
        serde_json::to_value(&uncertain).unwrap()["outcome"],
        serde_json::json!({"status": "uncertain"})
    );
    assert_eq!(
        serde_json::from_value::<ClassifyFinalizedWitnessedClaimResult>(
            serde_json::to_value(&uncertain).unwrap()
        )
        .unwrap(),
        uncertain
    );

    let mut unknown = serde_json::to_value(&not_found).unwrap();
    unknown["outcome"]["claim"] = serde_json::json!(null);
    assert!(
        serde_json::from_value::<ClassifyFinalizedWitnessedClaimResult>(unknown).is_err(),
        "not-found evidence must reject fields from the present variant"
    );
}

#[test]
fn peerless_finalized_claim_target_is_strict_and_has_explicit_conflict_error() {
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
    let discovery = ObserveFinalizedWitnessedClaimRequest::discover_by_terms(
        context(),
        runtime(),
        terms,
        PreparedWitnessedClaim::new(
            RequestId::new("witnessed-claim-prepare-0001").unwrap(),
            h(47),
            ExactMessageBytes::new(vec![48; 128]).unwrap(),
        ),
        DiscoveryWindow::new(50, 3).unwrap(),
    );
    assert_eq!(
        discovery.target,
        FinalizedWitnessedClaimObservationTarget::DiscoverByTerms
    );
    let encoded = serde_json::to_value(&discovery).unwrap();
    assert_eq!(
        encoded["target"],
        serde_json::json!({"mode": "discover_by_terms"})
    );
    assert!(encoded.get("claim_transaction_id").is_none());
    assert_eq!(
        serde_json::from_value::<ObserveFinalizedWitnessedClaimRequest>(encoded.clone()).unwrap(),
        discovery
    );
    for invalid_target in [
        serde_json::json!({"mode": "discover_by_terms", "claim_transaction_id": "31".repeat(32)}),
        serde_json::json!({"mode": "exact"}),
        serde_json::json!({"mode": "unknown"}),
    ] {
        let mut invalid = encoded.clone();
        invalid["target"] = invalid_target;
        assert!(serde_json::from_value::<ObserveFinalizedWitnessedClaimRequest>(invalid).is_err());
    }
    let conflict = ProtocolErrorReply::new(
        context(),
        ErrorCode::ConflictingDiscovery,
        ErrorMessage::new("canonical terms match conflicts with signed transcript").unwrap(),
    );
    assert_eq!(
        serde_json::from_str::<ProtocolErrorReply>(&serde_json::to_string(&conflict).unwrap())
            .unwrap(),
        conflict
    );
}

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
fn witnessed_observation_binds_authority_transactions_accounts_effects_and_tip() {
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
    let request = ObserveWitnessedEscrowRequest::new(
        context(),
        runtime(),
        terms.clone(),
        EscrowObservationTarget::Exact {
            initialization_transaction_id: TransactionId::from_bytes([47; 32]),
            funding_transaction_id: TransactionId::from_bytes([48; 32]),
        },
    );
    let initialization_transaction = ObservedTransactionFacts::new(
        TransactionId::from_bytes([47; 32]),
        ExactTransactionBytes::new(vec![47; 128]).unwrap(),
        ChainPosition::new(h(49), 51, 0),
        AccountIds::new(vec![h(42)]).unwrap(),
        true,
    );
    let funding_transaction = ObservedTransactionFacts::new(
        TransactionId::from_bytes([48; 32]),
        ExactTransactionBytes::new(vec![48; 128]).unwrap(),
        ChainPosition::new(h(49), 51, 1),
        AccountIds::new(vec![h(42)]).unwrap(),
        true,
    );
    let metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        h(50),
        h(4),
        h(52),
        &terms,
        EscrowState::Funded,
    );
    let result = ObserveWitnessedEscrowResult::new(
        context(),
        ChainTip::new(h(49), 51),
        WitnessedInitializationObservation::found(WitnessedInitializationFoundFacts::new(
            initialization_transaction,
            WitnessedNativeInitializeInstructionFacts::new(
                h(4),
                AccountIds::new(vec![h(50), h(52), h(42), h(43), h(44)]).unwrap(),
                terms.clone(),
            ),
            metadata.clone(),
        )),
        WitnessedFundingObservation::found(WitnessedFundingFoundFacts::new(
            funding_transaction,
            NativeFundInstructionFacts::new(
                h(4),
                AccountIds::new(vec![h(50), h(52), h(42)]).unwrap(),
                terms.swap_id(),
            ),
            metadata,
            NativeCustodyFacts::new(h(52), h(46), 75),
        )),
        ChainTip::new(h(49), 51),
    );

    assert_eq!(
        serde_json::from_str::<ObserveWitnessedEscrowRequest>(
            &serde_json::to_string(&request).unwrap()
        )
        .unwrap(),
        request
    );
    let encoded = serde_json::to_string(&result).unwrap();
    let decoded: ObserveWitnessedEscrowResult = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, result);
    assert!(encoded.contains("aggregate_authority_account_id"));
    assert!(encoded.contains("aggregate_x_only_public_key"));
    assert!(encoded.contains("exact_bytes"));
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
    for aliased_authority in [h(62), h(63)] {
        assert!(
            WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
                swap_id: h(60),
                terms_hash: h(61),
                depositor: Participant::Taker,
                depositor_account_id: h(62),
                claimant: Participant::Maker,
                claimant_account_id: h(63),
                aggregate_authority_account_id: aliased_authority,
                aggregate_x_only_public_key: h(65),
                amount: 75,
                refund_at_ms: 99,
                authenticated_transfer_program_id: h(66),
            })
            .is_err()
        );
    }
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
            EscrowMetadataFacts::from_lee_v0_2_native_terms(
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
#[allow(clippy::too_many_lines)] // One contract keeps legacy/witnessed JSON and mixed-field negatives together.
fn native_refund_wire_preserves_hashlock_shape_and_adds_strict_witnessed_shape() {
    let hashlock_prepare = PrepareNativeRefundRequest::new(context(), runtime(), terms());
    let hashlock_json = serde_json::to_value(&hashlock_prepare).unwrap();
    assert!(hashlock_json["terms"].get("secret_digest").is_some());
    assert!(
        hashlock_json["terms"]
            .get("aggregate_authority_account_id")
            .is_none()
    );
    assert!(hashlock_json["terms"].get("kind").is_none());
    assert_eq!(
        hashlock_json["terms"],
        serde_json::to_value(terms()).unwrap(),
        "the legacy terms object must not acquire an envelope or discriminator"
    );
    assert_eq!(
        serde_json::to_vec(&hashlock_prepare.terms).unwrap(),
        serde_json::to_vec(&terms()).unwrap(),
        "the legacy terms bytes must remain identical"
    );
    assert_eq!(
        hashlock_prepare.terms.hashlock(),
        Some(&terms()),
        "the existing constructor remains hashlock-only"
    );

    let witnessed = WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: h(30),
        terms_hash: h(31),
        depositor: Participant::Maker,
        depositor_account_id: h(32),
        claimant: Participant::Taker,
        claimant_account_id: h(33),
        aggregate_authority_account_id: h(34),
        aggregate_x_only_public_key: h(35),
        amount: 9_000,
        refund_at_ms: 1_850_000_000_123,
        authenticated_transfer_program_id: h(36),
    })
    .unwrap();
    let witnessed_prepare =
        PrepareNativeRefundRequest::new_witnessed(context(), runtime(), witnessed.clone());
    let witnessed_json = serde_json::to_value(&witnessed_prepare).unwrap();
    assert!(
        witnessed_json["terms"]
            .get("aggregate_authority_account_id")
            .is_some()
    );
    assert!(witnessed_json["terms"].get("secret_digest").is_none());
    assert!(witnessed_json["terms"].get("kind").is_none());
    assert_eq!(
        witnessed_prepare.terms.witnessed(),
        Some(&witnessed),
        "the witnessed constructor cannot silently downgrade authority"
    );
    assert_eq!(
        serde_json::from_value::<PrepareNativeRefundRequest>(witnessed_json.clone()).unwrap(),
        witnessed_prepare
    );

    let target = NativeRefundObservationTarget::DiscoverByTerms {
        window: DiscoveryWindow::new(40, 3).unwrap(),
    };
    let observe =
        ObserveNativeRefundRequest::new_witnessed(context(), runtime(), witnessed.clone(), target);
    assert_eq!(observe.terms.swap_id(), witnessed.swap_id());
    assert_eq!(observe.terms.terms_hash(), witnessed.terms_hash());
    assert_eq!(observe.terms.depositor(), witnessed.depositor());
    assert_eq!(
        observe.terms.depositor_account_id(),
        witnessed.depositor_account_id()
    );
    assert_eq!(observe.terms.claimant(), witnessed.claimant());
    assert_eq!(
        observe.terms.claimant_account_id(),
        witnessed.claimant_account_id()
    );
    assert_eq!(observe.terms.amount(), witnessed.amount());
    assert_eq!(observe.terms.refund_at_ms(), witnessed.refund_at_ms());
    assert_eq!(
        observe.terms.authenticated_transfer_program_id(),
        witnessed.authenticated_transfer_program_id()
    );
    assert_eq!(
        serde_json::from_value::<ObserveNativeRefundRequest>(
            serde_json::to_value(&observe).unwrap()
        )
        .unwrap(),
        observe
    );

    let facts = NativeEscrowAccountFacts::new_witnessed(
        WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
            h(37),
            h(4),
            h(38),
            &witnessed,
            EscrowState::Refunded,
        ),
        NativeCustodyFacts::new(h(38), h(36), 0),
    );
    let witnessed_facts_json = serde_json::to_value(&facts).unwrap();
    assert_eq!(
        facts.metadata.witnessed().unwrap().status,
        EscrowState::Refunded
    );
    let result = ObserveNativeRefundResult::new(
        context(),
        ChainClock::new(h(39), 40, witnessed.refund_at_ms()),
        NativeEscrowAccountObservation::found(facts),
        NativeRefundObservation::UnknownOrPending,
        ChainClock::new(h(39), 40, witnessed.refund_at_ms()),
    );
    assert_eq!(
        serde_json::from_value::<ObserveNativeRefundResult>(serde_json::to_value(&result).unwrap())
            .unwrap(),
        result
    );

    let mut mixed_witnessed = witnessed_json;
    mixed_witnessed["terms"]["secret_digest"] = serde_json::json!("2a".repeat(32));
    assert!(
        serde_json::from_value::<PrepareNativeRefundRequest>(mixed_witnessed).is_err(),
        "mixed witnessed/hashlock authority must fail closed"
    );
    let mut mixed_hashlock = hashlock_json;
    mixed_hashlock["terms"]["aggregate_authority_account_id"] = serde_json::json!("2b".repeat(32));
    mixed_hashlock["terms"]["aggregate_x_only_public_key"] = serde_json::json!("2c".repeat(32));
    assert!(
        serde_json::from_value::<PrepareNativeRefundRequest>(mixed_hashlock).is_err(),
        "mixed hashlock/witnessed authority must fail closed"
    );

    let mut mixed_witnessed_metadata = witnessed_facts_json;
    mixed_witnessed_metadata["metadata"]["secret_digest"] = serde_json::json!("2d".repeat(32));
    assert!(
        serde_json::from_value::<NativeEscrowAccountFacts>(mixed_witnessed_metadata).is_err(),
        "mixed witnessed/hashlock metadata must fail closed"
    );
    let hashlock_facts = NativeEscrowAccountFacts::new(
        EscrowMetadataFacts::from_lee_v0_2_native_terms(
            h(10),
            h(4),
            h(12),
            &terms(),
            EscrowState::Refunded,
        ),
        NativeCustodyFacts::new(h(12), h(22), 0),
    );
    let mut mixed_hashlock_metadata = serde_json::to_value(hashlock_facts).unwrap();
    mixed_hashlock_metadata["metadata"]["aggregate_authority_account_id"] =
        serde_json::json!("2e".repeat(32));
    mixed_hashlock_metadata["metadata"]["aggregate_x_only_public_key"] =
        serde_json::json!("2f".repeat(32));
    assert!(
        serde_json::from_value::<NativeEscrowAccountFacts>(mixed_hashlock_metadata).is_err(),
        "mixed hashlock/witnessed metadata must fail closed"
    );
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
            EscrowMetadataFacts::from_lee_v0_2_native_terms(
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
            EscrowMetadataFacts::from_lee_v0_2_native_terms(
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
    let compatibility_metadata = EscrowMetadataFacts::from_nssa_v0_1_2_native_terms(
        h(10),
        h(4),
        h(12),
        &terms(),
        EscrowState::Empty,
    );
    let expected_metadata = EscrowMetadataFacts::from_lee_v0_2_native_terms(
        h(10),
        h(4),
        h(12),
        &terms(),
        EscrowState::Empty,
    );
    assert_eq!(compatibility_metadata.version, 1);
    assert_eq!(expected_metadata.version, 2);
    assert_eq!(compatibility_metadata.swap_id, expected_metadata.swap_id);
    assert_eq!(
        compatibility_metadata.terms_hash,
        expected_metadata.terms_hash
    );
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
            EscrowMetadataFacts::from_lee_v0_2_native_terms(
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
fn transaction_id_derives_exact_stable_submission_request_id() {
    let transaction_id = TransactionId::from_bytes([0xab; 32]);

    let first = transaction_id.submission_request_id();
    let second = transaction_id.submission_request_id();

    assert_eq!(first, second);
    assert_eq!(
        first.as_str(),
        "abababababababababababababababababababababababababababababababab"
    );
    assert_eq!(first.as_str().len(), 64);
    assert!(
        first
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
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

#[test]
fn witnessed_asset_v2_adds_native_without_changing_v1_json_or_methods() {
    let native = witnessed_native_terms();
    let legacy_json = serde_json::to_value(&native).unwrap();
    assert_eq!(
        legacy_json,
        serde_json::json!({
            "swap_id": "28".repeat(32),
            "terms_hash": "29".repeat(32),
            "depositor": "taker",
            "depositor_account_id": "2a".repeat(32),
            "claimant": "maker",
            "claimant_account_id": "2b".repeat(32),
            "aggregate_authority_account_id": "2c".repeat(32),
            "aggregate_x_only_public_key": "2d".repeat(32),
            "amount": "75",
            "refund_at_ms": 1_850_000_000_123_u64,
            "authenticated_transfer_program_id": "2e".repeat(32),
        })
    );
    assert_eq!(
        serde_json::from_value::<WitnessedNativeEscrowTerms>(legacy_json.clone()).unwrap(),
        native
    );
    assert_eq!(
        METHOD_PREPARE_WITNESSED_ESCROW,
        "lez_bridge.v1.prepare_witnessed_escrow"
    );
    assert_eq!(
        METHOD_OBSERVE_WITNESSED_ESCROW,
        "lez_bridge.v1.observe_witnessed_escrow"
    );
    assert_eq!(
        METHOD_PREPARE_WITNESSED_CLAIM,
        "lez_bridge.v1.prepare_witnessed_claim"
    );
    assert_eq!(
        METHOD_COMPLETE_WITNESSED_CLAIM,
        "lez_bridge.v1.complete_witnessed_claim"
    );

    let versioned = WitnessedLezAssetTermsV2::native(native.clone());
    assert_eq!(
        versioned.asset_terms_version(),
        WITNESSED_LEZ_ASSET_TERMS_VERSION
    );
    assert_eq!(
        versioned.asset(),
        &WitnessedLezAssetV2::Native(native.clone())
    );
    let versioned_json = serde_json::to_value(&versioned).unwrap();
    assert_eq!(
        versioned_json,
        serde_json::json!({
            "asset_terms_version": 2,
            "asset": {
                "kind": "native",
                "terms": legacy_json,
            },
        })
    );
    assert_eq!(
        serde_json::from_value::<WitnessedLezAssetTermsV2>(versioned_json.clone()).unwrap(),
        versioned
    );
    assert!(
        serde_json::from_value::<WitnessedNativeEscrowTerms>(versioned_json).is_err(),
        "the established unversioned v1 terms must not silently accept v2"
    );
}

#[test]
fn witnessed_asset_v2_token_terms_bind_two_definitions_and_every_substitution() {
    let first = WitnessedTokenEscrowTermsV2::new(witnessed_token_input(h(82))).unwrap();
    let second = WitnessedTokenEscrowTermsV2::new(witnessed_token_input(h(83))).unwrap();
    assert_ne!(first, second);
    assert_eq!(first.token_definition_account_id(), h(82));
    assert_eq!(second.token_definition_account_id(), h(83));
    assert_eq!(first.token_program_id(), h(70));
    assert_eq!(first.ata_program_id(), h(71));
    assert_eq!(first.depositor_owner_account_id(), h(73));
    assert_eq!(first.depositor_ata_account_id(), h(74));
    assert_eq!(first.claimant_owner_account_id(), h(75));
    assert_eq!(first.claimant_ata_account_id(), h(76));
    assert_eq!(first.custody_ata_account_id(), h(77));
    assert_eq!(first.aggregate_authority_account_id(), h(78));
    assert_eq!(first.aggregate_x_only_public_key(), h(79));
    assert_eq!(first.swap_id(), h(80));
    assert_eq!(first.terms_hash(), h(81));
    assert_eq!(first.amount(), NativeAmount::new(125));
    assert_eq!(first.refund_at_ms(), 1_850_000_000_456);

    for terms in [first.clone(), second] {
        let versioned = WitnessedLezAssetTermsV2::custom_token(terms.clone());
        assert_eq!(versioned.asset(), &WitnessedLezAssetV2::CustomToken(terms));
        assert_eq!(
            serde_json::from_value::<WitnessedLezAssetTermsV2>(
                serde_json::to_value(&versioned).unwrap()
            )
            .unwrap(),
            versioned
        );
    }

    let expected = first.clone();
    let expected_json = serde_json::to_value(&expected).unwrap();
    for (field, replacement) in [
        ("swap_id", h(90)),
        ("terms_hash", h(91)),
        ("token_program_id", h(92)),
        ("ata_program_id", h(93)),
        ("token_definition_account_id", h(94)),
        ("depositor_owner_account_id", h(95)),
        ("depositor_ata_account_id", h(96)),
        ("claimant_owner_account_id", h(97)),
        ("claimant_ata_account_id", h(98)),
        ("custody_ata_account_id", h(99)),
        ("aggregate_authority_account_id", h(100)),
        ("aggregate_x_only_public_key", h(101)),
    ] {
        let mut changed = expected_json.clone();
        changed[field] = serde_json::to_value(replacement).unwrap();
        let changed = serde_json::from_value::<WitnessedTokenEscrowTermsV2>(changed).unwrap();
        assert_ne!(changed, expected, "token field {field} was not bound");
    }
    for (field, replacement) in [
        ("amount", serde_json::json!("126")),
        ("refund_at_ms", serde_json::json!(1_850_000_000_457_u64)),
    ] {
        let mut changed = expected_json.clone();
        changed[field] = replacement;
        let changed = serde_json::from_value::<WitnessedTokenEscrowTermsV2>(changed).unwrap();
        assert_ne!(changed, expected, "token field {field} was not bound");
    }
    let mut changed_roles = expected_json;
    changed_roles["depositor"] = serde_json::json!("taker");
    changed_roles["claimant"] = serde_json::json!("maker");
    let changed_roles =
        serde_json::from_value::<WitnessedTokenEscrowTermsV2>(changed_roles).unwrap();
    assert_ne!(changed_roles, expected, "token roles were not bound");
}

#[test]
fn witnessed_asset_v2_rejects_zero_alias_malformed_and_unknown_token_json() {
    let valid = WitnessedTokenEscrowTermsV2::new(witnessed_token_input(h(82))).unwrap();
    let valid_json = serde_json::to_value(&valid).unwrap();
    for field in [
        "terms_hash",
        "token_program_id",
        "ata_program_id",
        "token_definition_account_id",
        "depositor_owner_account_id",
        "depositor_ata_account_id",
        "claimant_owner_account_id",
        "claimant_ata_account_id",
        "custody_ata_account_id",
        "aggregate_authority_account_id",
        "aggregate_x_only_public_key",
    ] {
        let mut zero = valid_json.clone();
        zero[field] = serde_json::json!("00".repeat(32));
        assert!(
            serde_json::from_value::<WitnessedTokenEscrowTermsV2>(zero).is_err(),
            "zero {field} was accepted"
        );
    }
    for (target, source) in [
        ("claimant_owner_account_id", "depositor_owner_account_id"),
        ("claimant_ata_account_id", "depositor_ata_account_id"),
        ("custody_ata_account_id", "claimant_ata_account_id"),
        ("depositor_ata_account_id", "depositor_owner_account_id"),
        (
            "aggregate_authority_account_id",
            "claimant_owner_account_id",
        ),
        ("aggregate_authority_account_id", "claimant_ata_account_id"),
        ("ata_program_id", "token_program_id"),
        ("token_definition_account_id", "token_program_id"),
    ] {
        let mut aliased = valid_json.clone();
        aliased[target] = aliased[source].clone();
        assert!(
            serde_json::from_value::<WitnessedTokenEscrowTermsV2>(aliased).is_err(),
            "alias {target}={source} was accepted"
        );
    }

    for (field, invalid) in [
        ("amount", serde_json::json!("0")),
        ("amount", serde_json::json!(125)),
        ("refund_at_ms", serde_json::json!(0)),
        ("depositor", serde_json::json!("taker")),
        ("claimant", serde_json::json!("maker")),
    ] {
        let mut malformed = valid_json.clone();
        malformed[field] = invalid;
        assert!(
            serde_json::from_value::<WitnessedTokenEscrowTermsV2>(malformed).is_err(),
            "invalid {field} was accepted"
        );
    }
    let mut unknown_terms = valid_json;
    unknown_terms["surprise"] = serde_json::json!(true);
    assert!(serde_json::from_value::<WitnessedTokenEscrowTermsV2>(unknown_terms).is_err());

    let versioned = WitnessedLezAssetTermsV2::custom_token(valid);
    let versioned_json = serde_json::to_value(versioned).unwrap();
    for invalid in [
        {
            let mut value = versioned_json.clone();
            value["asset_terms_version"] = serde_json::json!(1);
            value
        },
        {
            let mut value = versioned_json.clone();
            value["surprise"] = serde_json::json!(true);
            value
        },
        {
            let mut value = versioned_json.clone();
            value["asset"]["surprise"] = serde_json::json!(true);
            value
        },
        {
            let mut value = versioned_json.clone();
            value["asset"]["kind"] = serde_json::json!("fungible");
            value
        },
    ] {
        assert!(serde_json::from_value::<WitnessedLezAssetTermsV2>(invalid).is_err());
    }
}

fn witnessed_native_terms() -> WitnessedNativeEscrowTerms {
    WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
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
    .unwrap()
}

fn witnessed_token_input(definition: Hex32) -> WitnessedTokenEscrowTermsV2Input {
    WitnessedTokenEscrowTermsV2Input {
        swap_id: h(80),
        terms_hash: h(81),
        depositor: Participant::Maker,
        depositor_owner_account_id: h(73),
        depositor_ata_account_id: h(74),
        claimant: Participant::Taker,
        claimant_owner_account_id: h(75),
        claimant_ata_account_id: h(76),
        custody_ata_account_id: h(77),
        token_program_id: h(70),
        ata_program_id: h(71),
        token_definition_account_id: definition,
        aggregate_authority_account_id: h(78),
        aggregate_x_only_public_key: h(79),
        amount: 125,
        refund_at_ms: 1_850_000_000_456,
    }
}

#[test]
fn witnessed_asset_v2_methods_and_prepare_effect_order_are_additive_and_strict() {
    for (actual, expected) in [
        (
            METHOD_PREPARE_WITNESSED_ASSET_ESCROW_V2,
            "lez_bridge.v2.prepare_witnessed_asset_escrow",
        ),
        (
            METHOD_OBSERVE_WITNESSED_ASSET_ESCROW_V2,
            "lez_bridge.v2.observe_witnessed_asset_escrow",
        ),
        (
            METHOD_PREPARE_WITNESSED_ASSET_CLAIM_V2,
            "lez_bridge.v2.prepare_witnessed_asset_claim",
        ),
        (
            METHOD_COMPLETE_WITNESSED_ASSET_CLAIM_V2,
            "lez_bridge.v2.complete_witnessed_asset_claim",
        ),
        (
            METHOD_OBSERVE_FINALIZED_WITNESSED_ASSET_CLAIM_V2,
            "lez_bridge.v2.observe_finalized_witnessed_asset_claim",
        ),
        (
            METHOD_PREPARE_WITNESSED_ASSET_REFUND_V2,
            "lez_bridge.v2.prepare_witnessed_asset_refund",
        ),
        (
            METHOD_OBSERVE_WITNESSED_ASSET_REFUND_V2,
            "lez_bridge.v2.observe_witnessed_asset_refund",
        ),
    ] {
        assert_eq!(actual, expected);
        assert!(!actual.starts_with("lez_bridge.v1."));
    }

    let token = token_asset(h(82));
    let request = PrepareWitnessedAssetEscrowV2Request::new(context(), runtime(), token.clone());
    assert_eq!(
        serde_json::from_value::<PrepareWitnessedAssetEscrowV2Request>(
            serde_json::to_value(&request).unwrap()
        )
        .unwrap(),
        request
    );
    let effects = token_prepared_effects();
    let result =
        PrepareWitnessedAssetEscrowV2Result::new(context(), token.clone(), effects.clone())
            .unwrap();
    assert_eq!(result.effects, effects);
    assert_eq!(
        serde_json::from_value::<PrepareWitnessedAssetEscrowV2Result>(
            serde_json::to_value(&result).unwrap()
        )
        .unwrap(),
        result
    );

    let mut wrong_order = effects.clone();
    wrong_order.swap(1, 2);
    assert!(
        PrepareWitnessedAssetEscrowV2Result::new(context(), token.clone(), wrong_order).is_err()
    );
    assert!(
        PrepareWitnessedAssetEscrowV2Result::new(context(), token, effects[..2].to_vec()).is_err()
    );

    let native = WitnessedLezAssetTermsV2::native(witnessed_native_terms());
    let native_effects = vec![
        WitnessedAssetPreparedEffectV2::new(
            WitnessedAssetPrepareStepV2::InitializeWitnessed,
            tx(8),
        ),
        WitnessedAssetPreparedEffectV2::new(WitnessedAssetPrepareStepV2::Fund, tx(9)),
    ];
    assert!(
        PrepareWitnessedAssetEscrowV2Result::new(
            context(),
            native.clone(),
            native_effects.clone(),
        )
        .is_ok()
    );
    let mut native_with_ata = native_effects;
    native_with_ata.insert(
        1,
        WitnessedAssetPreparedEffectV2::new(WitnessedAssetPrepareStepV2::CreateCustodyAta, tx(10)),
    );
    assert!(PrepareWitnessedAssetEscrowV2Result::new(context(), native, native_with_ata).is_err());

    let mut unknown = serde_json::to_value(result).unwrap();
    unknown["effects"][0]["surprise"] = serde_json::json!(true);
    assert!(serde_json::from_value::<PrepareWitnessedAssetEscrowV2Result>(unknown).is_err());
}

#[test]
fn witnessed_asset_v2_token_prepare_observation_rejects_definition_ata_authority_and_order_drift() {
    for definition in [h(82), h(83)] {
        let terms = token_asset(definition);
        let token = terms.asset().custom_token().unwrap();
        let metadata = WitnessedEscrowMetadataFacts::from_witnessed_token_terms(
            h(102),
            h(103),
            token,
            EscrowState::Funded,
        );
        let custody = WitnessedAssetCustodyFactsV2::CustomToken(TokenHoldingFactsV2::new(
            token.custody_ata_account_id(),
            token.token_program_id(),
            token.token_definition_account_id(),
            token.amount().as_u128(),
        ));
        let observed_effects = token_observed_prepare_effects(token, metadata.account_id);
        let request = ObserveWitnessedAssetEscrowV2Request::new(
            context(),
            runtime(),
            terms.clone(),
            token_prepared_effects(),
            discovery_window(),
        )
        .unwrap();
        let result = ObserveWitnessedAssetEscrowV2Result::new(
            context(),
            terms.clone(),
            ChainTip::new(h(104), 90),
            observed_effects,
            metadata,
            custody,
            ChainTip::new(h(104), 90),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_value::<ObserveWitnessedAssetEscrowV2Request>(
                serde_json::to_value(&request).unwrap()
            )
            .unwrap(),
            request
        );
        let encoded = serde_json::to_value(&result).unwrap();
        assert_eq!(
            serde_json::from_value::<ObserveWitnessedAssetEscrowV2Result>(encoded.clone()).unwrap(),
            result
        );

        for (path, replacement) in [
            (
                vec!["terms", "asset", "terms", "token_definition_account_id"],
                serde_json::to_value(h(110)).unwrap(),
            ),
            (
                vec!["terms", "asset", "terms", "claimant_ata_account_id"],
                serde_json::to_value(h(111)).unwrap(),
            ),
            (
                vec!["terms", "asset", "terms", "aggregate_authority_account_id"],
                serde_json::to_value(h(112)).unwrap(),
            ),
        ] {
            let mut changed = encoded.clone();
            json_set(&mut changed, &path, replacement);
            assert!(
                serde_json::from_value::<ObserveWitnessedAssetEscrowV2Result>(changed).is_err(),
                "drift at {path:?} was accepted"
            );
        }
        let mut wrong_order = encoded.clone();
        wrong_order["effects"][0]["ordered_account_ids"]
            .as_array_mut()
            .unwrap()
            .swap(1, 2);
        assert!(
            serde_json::from_value::<ObserveWitnessedAssetEscrowV2Result>(wrong_order).is_err()
        );
        let mut unknown = encoded;
        unknown["custody"]["facts"]["surprise"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ObserveWitnessedAssetEscrowV2Result>(unknown).is_err());
    }
}

#[test]
fn witnessed_asset_v2_claim_prepare_and_complete_models_are_strict() {
    let terms = token_asset(h(82));
    let claim = PreparedWitnessedClaim::new(
        RequestId::new("asset-v2-claim-prepare").unwrap(),
        h(120),
        ExactMessageBytes::new(vec![121; 128]).unwrap(),
    );
    let prepare_claim = PrepareWitnessedAssetClaimV2Request::new(
        context(),
        runtime(),
        terms.clone(),
        TransactionId::from_bytes([122; 32]),
    );
    let prepare_claim_result =
        PrepareWitnessedAssetClaimV2Result::new(context(), terms.clone(), claim.clone());
    let complete_claim = CompleteWitnessedAssetClaimV2Request::new(
        context(),
        runtime(),
        terms.clone(),
        claim.clone(),
        AggregateBip340Signature::from_bytes([123; 64]),
    );
    let complete_claim_result =
        CompleteWitnessedAssetClaimV2Result::new(context(), terms.clone(), tx(124));
    for roundtrip in [
        serde_json::from_value::<PrepareWitnessedAssetClaimV2Request>(
            serde_json::to_value(&prepare_claim).unwrap(),
        )
        .is_ok(),
        serde_json::from_value::<PrepareWitnessedAssetClaimV2Result>(
            serde_json::to_value(&prepare_claim_result).unwrap(),
        )
        .is_ok(),
        serde_json::from_value::<CompleteWitnessedAssetClaimV2Request>(
            serde_json::to_value(&complete_claim).unwrap(),
        )
        .is_ok(),
        serde_json::from_value::<CompleteWitnessedAssetClaimV2Result>(
            serde_json::to_value(&complete_claim_result).unwrap(),
        )
        .is_ok(),
    ] {
        assert!(roundtrip);
    }
}

#[test]
fn witnessed_asset_v2_finalized_claim_is_exact_and_token_bound() {
    let terms = token_asset(h(82));
    let token = terms.asset().custom_token().unwrap();
    let claim = PreparedWitnessedClaim::new(
        RequestId::new("asset-v2-claim-prepare").unwrap(),
        h(120),
        ExactMessageBytes::new(vec![121; 128]).unwrap(),
    );
    let claimed_metadata = WitnessedEscrowMetadataFacts::from_witnessed_token_terms(
        h(125),
        h(103),
        token,
        EscrowState::Claimed,
    );
    let empty_custody = WitnessedAssetCustodyFactsV2::CustomToken(TokenHoldingFactsV2::new(
        token.custody_ata_account_id(),
        token.token_program_id(),
        token.token_definition_account_id(),
        0,
    ));
    let claim_facts = FinalizedWitnessedAssetClaimFactsV2::new(
        observed_tx(124),
        WitnessedAssetClaimInstructionFactsV2::new(
            h(103),
            AccountIds::new(vec![
                claimed_metadata.account_id,
                token.custody_ata_account_id(),
                token.claimant_owner_account_id(),
                token.claimant_ata_account_id(),
                token.aggregate_authority_account_id(),
            ])
            .unwrap(),
            token.swap_id(),
            claim.clone(),
        ),
        AggregateBip340Signature::from_bytes([123; 64]),
        FinalizedBlockIdentity::new(91, h(126), 1_850_000_000_600),
        claimed_metadata,
        empty_custody,
    );
    let observe_claim = ObserveFinalizedWitnessedAssetClaimV2Request::new(
        context(),
        runtime(),
        terms.clone(),
        claim,
        TransactionId::from_bytes([124; 32]),
        discovery_window(),
    );
    let observe_claim_result = ObserveFinalizedWitnessedAssetClaimV2Result::new(
        context(),
        terms.clone(),
        ChainTip::new(h(127), 92),
        claim_facts,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_value::<ObserveFinalizedWitnessedAssetClaimV2Request>(
            serde_json::to_value(&observe_claim).unwrap()
        )
        .unwrap(),
        observe_claim
    );
    let claim_json = serde_json::to_value(&observe_claim_result).unwrap();
    assert_eq!(
        serde_json::from_value::<ObserveFinalizedWitnessedAssetClaimV2Result>(claim_json.clone())
            .unwrap(),
        observe_claim_result
    );
    let mut wrong_authority = claim_json;
    wrong_authority["claim"]["instruction"]["ordered_account_ids"][4] =
        serde_json::to_value(h(128)).unwrap();
    assert!(
        serde_json::from_value::<ObserveFinalizedWitnessedAssetClaimV2Result>(wrong_authority)
            .is_err()
    );
}

#[test]
fn witnessed_asset_v2_refund_models_are_exact_and_token_bound() {
    let terms = token_asset(h(82));
    let token = terms.asset().custom_token().unwrap();
    let prepare_refund =
        PrepareWitnessedAssetRefundV2Request::new(context(), runtime(), terms.clone());
    let prepare_refund_result =
        PrepareWitnessedAssetRefundV2Result::new(context(), terms.clone(), tx(129));
    assert_eq!(
        serde_json::from_value::<PrepareWitnessedAssetRefundV2Request>(
            serde_json::to_value(&prepare_refund).unwrap()
        )
        .unwrap(),
        prepare_refund
    );
    assert_eq!(
        serde_json::from_value::<PrepareWitnessedAssetRefundV2Result>(
            serde_json::to_value(&prepare_refund_result).unwrap()
        )
        .unwrap(),
        prepare_refund_result
    );
    let refunded_metadata = WitnessedEscrowMetadataFacts::from_witnessed_token_terms(
        h(125),
        h(103),
        token,
        EscrowState::Refunded,
    );
    let refund = WitnessedAssetRefundObservationV2::found(WitnessedAssetRefundFoundFactsV2::new(
        observed_tx(129),
        WitnessedAssetRefundInstructionFactsV2::new(
            h(103),
            AccountIds::new(vec![
                refunded_metadata.account_id,
                token.custody_ata_account_id(),
                token.depositor_ata_account_id(),
            ])
            .unwrap(),
            token.swap_id(),
        ),
    ));
    let observe_refund = ObserveWitnessedAssetRefundV2Request::new(
        context(),
        runtime(),
        terms.clone(),
        NativeRefundObservationTarget::Exact {
            refund_transaction_id: TransactionId::from_bytes([129; 32]),
            window: discovery_window(),
        },
    );
    let observe_refund_result = ObserveWitnessedAssetRefundV2Result::new(
        context(),
        terms.clone(),
        ChainClock::new(h(130), 93, 1_850_000_000_700),
        refunded_metadata,
        WitnessedAssetCustodyFactsV2::CustomToken(TokenHoldingFactsV2::new(
            token.custody_ata_account_id(),
            token.token_program_id(),
            token.token_definition_account_id(),
            0,
        )),
        refund,
        ChainClock::new(h(130), 93, 1_850_000_000_700),
    )
    .unwrap();
    assert_eq!(
        serde_json::from_value::<ObserveWitnessedAssetRefundV2Request>(
            serde_json::to_value(&observe_refund).unwrap()
        )
        .unwrap(),
        observe_refund
    );
    let refund_json = serde_json::to_value(&observe_refund_result).unwrap();
    assert_eq!(
        serde_json::from_value::<ObserveWitnessedAssetRefundV2Result>(refund_json.clone()).unwrap(),
        observe_refund_result
    );
    let mut wrong_refund_ata = refund_json;
    wrong_refund_ata["refund"]["facts"]["instruction"]["ordered_account_ids"][2] =
        serde_json::to_value(h(131)).unwrap();
    assert!(
        serde_json::from_value::<ObserveWitnessedAssetRefundV2Result>(wrong_refund_ata).is_err()
    );
}

#[test]
fn finalized_witnessed_asset_classifier_methods_are_additive_and_v1_stays_exact() {
    for (actual, expected) in [
        (
            METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_INITIALIZATION_V2,
            "lez_bridge.v2.classify_finalized_witnessed_asset_initialization",
        ),
        (
            METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_FUNDING_V2,
            "lez_bridge.v2.classify_finalized_witnessed_asset_funding",
        ),
        (
            METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CUSTODY_CREATION_V2,
            "lez_bridge.v2.classify_finalized_witnessed_asset_custody_creation",
        ),
        (
            METHOD_CLASSIFY_FINALIZED_WITNESSED_ASSET_CLAIM_V2,
            "lez_bridge.v2.classify_finalized_witnessed_asset_claim",
        ),
    ] {
        assert_eq!(actual, expected);
        assert!(!actual.starts_with("lez_bridge.v1."));
    }
    assert_eq!(
        METHOD_CLASSIFY_FINALIZED_WITNESSED_INITIALIZATION,
        "lez_bridge.v1.classify_finalized_witnessed_initialization"
    );
    assert_eq!(
        METHOD_CLASSIFY_FINALIZED_WITNESSED_FUNDING,
        "lez_bridge.v1.classify_finalized_witnessed_funding"
    );
    assert_eq!(
        METHOD_CLASSIFY_FINALIZED_WITNESSED_CLAIM,
        "lez_bridge.v1.classify_finalized_witnessed_claim"
    );
}

#[test]
fn finalized_asset_initialization_binds_exact_bytes_accounts_state_and_coverage() {
    for definition in [h(82), h(83)] {
        let terms = token_asset(definition);
        let window = finalized_asset_window();
        let exact = ClassifyFinalizedWitnessedAssetInitializationV2Request::new(
            context(),
            runtime(),
            terms.clone(),
            tx(105),
            window,
        );
        let discovery = ClassifyFinalizedWitnessedAssetInitializationV2Request::discover_by_terms(
            context(),
            runtime(),
            terms.clone(),
            window,
        );
        assert_asset_classifier_requests_roundtrip(&exact, &discovery);
        let target = if definition == h(82) {
            exact.target.clone()
        } else {
            discovery.target.clone()
        };
        let found = ClassifyFinalizedWitnessedAssetInitializationV2Result::found(
            context(),
            terms,
            target,
            finalized_asset_clock(),
            window,
            finalized_token_initialization_facts(definition),
        )
        .unwrap();
        let encoded = serde_json::to_value(&found).unwrap();
        assert_eq!(
            serde_json::from_value::<ClassifyFinalizedWitnessedAssetInitializationV2Result>(
                encoded.clone()
            )
            .unwrap(),
            found
        );
        assert_finalized_initialization_substitutions_fail(&encoded);
    }
}

#[test]
fn finalized_asset_initialization_statuses_separate_absence_uncertainty_and_unavailability() {
    let terms = token_asset(h(82));
    let target = FinalizedWitnessedAssetTransactionTargetV2::exact(tx(105));
    let window = finalized_asset_window();
    let absent = ClassifyFinalizedWitnessedAssetInitializationV2Result::absent(
        context(),
        terms.clone(),
        target.clone(),
        finalized_asset_clock(),
        window,
    )
    .unwrap();
    let uncertain = ClassifyFinalizedWitnessedAssetInitializationV2Result::uncertain(
        context(),
        terms.clone(),
        target.clone(),
        finalized_asset_clock(),
        window,
    )
    .unwrap();
    let unavailable = ClassifyFinalizedWitnessedAssetInitializationV2Result::unavailable(
        context(),
        terms,
        target,
        FinalizedWitnessedAssetUnavailableReasonV2::MovingTip,
    );
    assert_asset_scan_statuses_roundtrip(absent, uncertain, unavailable);
}

#[test]
fn finalized_asset_funding_binds_exact_or_discovered_token_effects() {
    let terms = token_asset(h(83));
    let window = finalized_asset_window();
    let exact = ClassifyFinalizedWitnessedAssetFundingV2Request::new(
        context(),
        runtime(),
        terms.clone(),
        tx(107),
        window,
    );
    let discovery = ClassifyFinalizedWitnessedAssetFundingV2Request::discover_by_terms(
        context(),
        runtime(),
        terms.clone(),
        window,
    );
    assert_asset_classifier_requests_roundtrip(&exact, &discovery);
    let result = ClassifyFinalizedWitnessedAssetFundingV2Result::found(
        context(),
        terms,
        exact.target.clone(),
        finalized_asset_clock(),
        window,
        finalized_token_funding_facts(h(83)),
    )
    .unwrap();
    let encoded = serde_json::to_value(result).unwrap();
    assert!(
        serde_json::from_value::<ClassifyFinalizedWitnessedAssetFundingV2Result>(encoded.clone())
            .is_ok()
    );
    assert_finalized_funding_substitutions_fail(&encoded);
}

#[test]
fn finalized_token_custody_creation_is_restart_safe_exact_and_token_only() {
    let terms = token_asset(h(82));
    let window = finalized_asset_window();
    let exact = ClassifyFinalizedWitnessedAssetCustodyCreationV2Request::new(
        context(),
        runtime(),
        terms.clone(),
        tx(106),
        window,
    )
    .unwrap();
    let discovery = ClassifyFinalizedWitnessedAssetCustodyCreationV2Request::discover_by_terms(
        context(),
        runtime(),
        terms.clone(),
        window,
    )
    .unwrap();
    assert_asset_classifier_requests_roundtrip(&exact, &discovery);
    assert!(
        ClassifyFinalizedWitnessedAssetCustodyCreationV2Request::new(
            context(),
            runtime(),
            WitnessedLezAssetTermsV2::native(witnessed_native_terms()),
            tx(106),
            window,
        )
        .is_err()
    );
    let found = ClassifyFinalizedWitnessedAssetCustodyCreationV2Result::found(
        context(),
        terms,
        exact.target.clone(),
        finalized_asset_clock(),
        window,
        finalized_token_custody_creation_facts(h(82)),
    )
    .unwrap();
    let encoded = serde_json::to_value(found).unwrap();
    assert!(
        serde_json::from_value::<ClassifyFinalizedWitnessedAssetCustodyCreationV2Result>(
            encoded.clone()
        )
        .is_ok()
    );
    assert_finalized_custody_creation_substitutions_fail(&encoded);
}

#[test]
fn finalized_token_custody_creation_has_four_nonoverlapping_statuses() {
    let terms = token_asset(h(82));
    let target = FinalizedWitnessedAssetTransactionTargetV2::exact(tx(106));
    let window = finalized_asset_window();
    assert_wire_roundtrip(
        &ClassifyFinalizedWitnessedAssetCustodyCreationV2Result::absent(
            context(),
            terms.clone(),
            target.clone(),
            finalized_asset_clock(),
            window,
        )
        .unwrap(),
    );
    assert_wire_roundtrip(
        &ClassifyFinalizedWitnessedAssetCustodyCreationV2Result::uncertain(
            context(),
            terms.clone(),
            target.clone(),
            finalized_asset_clock(),
            window,
        )
        .unwrap(),
    );
    let unavailable = ClassifyFinalizedWitnessedAssetCustodyCreationV2Result::unavailable(
        context(),
        terms,
        target,
        FinalizedWitnessedAssetUnavailableReasonV2::HistoryUnavailable,
    )
    .unwrap();
    let encoded = serde_json::to_value(&unavailable).unwrap();
    assert!(encoded["outcome"].get("finalized_clock").is_none());
    assert!(encoded["outcome"].get("scanned_window").is_none());
    assert_wire_roundtrip(&unavailable);
}

#[test]
fn finalized_asset_claim_presence_is_conservative_exact_and_conflict_typed() {
    let terms = token_asset(h(82));
    let claim = asset_claim_transcript();
    let window = finalized_asset_window();
    let request = ClassifyFinalizedWitnessedAssetClaimV2Request::new(
        context(),
        runtime(),
        terms.clone(),
        claim.clone(),
        tx(124),
        window,
    );
    let discovery = ClassifyFinalizedWitnessedAssetClaimV2Request::discover_by_terms(
        context(),
        runtime(),
        terms.clone(),
        claim.clone(),
        window,
    );
    assert_asset_classifier_requests_roundtrip(&request, &discovery);
    let found = ClassifyFinalizedWitnessedAssetClaimV2Result::found(
        context(),
        terms.clone(),
        claim.clone(),
        request.target.clone(),
        finalized_asset_clock(),
        window,
        finalized_token_claim_facts(h(82), claim),
    )
    .unwrap();
    let encoded = serde_json::to_value(&found).unwrap();
    assert_eq!(
        serde_json::from_value::<ClassifyFinalizedWitnessedAssetClaimV2Result>(encoded.clone())
            .unwrap(),
        found
    );
    assert_finalized_claim_substitutions_fail(&encoded);

    let unavailable = ClassifyFinalizedWitnessedAssetClaimV2Result::unavailable(
        context(),
        terms,
        asset_claim_transcript(),
        discovery.target,
        FinalizedWitnessedAssetUnavailableReasonV2::ConflictingMatches,
    );
    assert_eq!(
        serde_json::to_value(unavailable).unwrap()["outcome"],
        serde_json::json!({"status":"unavailable","reason":"conflicting_matches"})
    );
}

#[test]
fn finalized_asset_classifiers_preserve_native_found_path_parity() {
    let terms = WitnessedLezAssetTermsV2::native(witnessed_native_terms());
    let window = finalized_asset_window();
    let initialization = ClassifyFinalizedWitnessedAssetInitializationV2Result::found(
        context(),
        terms.clone(),
        FinalizedWitnessedAssetTransactionTargetV2::exact(tx(132)),
        finalized_asset_clock(),
        window,
        finalized_native_initialization_facts(),
    )
    .unwrap();
    let funding = ClassifyFinalizedWitnessedAssetFundingV2Result::found(
        context(),
        terms.clone(),
        FinalizedWitnessedAssetTransactionTargetV2::exact(tx(133)),
        finalized_asset_clock(),
        window,
        finalized_native_funding_facts(),
    )
    .unwrap();
    let claim = asset_claim_transcript();
    let claimed = ClassifyFinalizedWitnessedAssetClaimV2Result::found(
        context(),
        terms,
        claim.clone(),
        FinalizedWitnessedAssetTransactionTargetV2::exact(tx(134)),
        finalized_asset_clock(),
        window,
        finalized_native_claim_facts(claim),
    )
    .unwrap();
    assert_eq!(
        serde_json::from_value::<ClassifyFinalizedWitnessedAssetInitializationV2Result>(
            serde_json::to_value(&initialization).unwrap()
        )
        .unwrap(),
        initialization
    );
    assert_eq!(
        serde_json::from_value::<ClassifyFinalizedWitnessedAssetFundingV2Result>(
            serde_json::to_value(&funding).unwrap()
        )
        .unwrap(),
        funding
    );
    assert_eq!(
        serde_json::from_value::<ClassifyFinalizedWitnessedAssetClaimV2Result>(
            serde_json::to_value(&claimed).unwrap()
        )
        .unwrap(),
        claimed
    );
}

#[test]
fn finalized_asset_funding_and_claim_keep_absent_uncertain_unavailable_distinct() {
    let terms = token_asset(h(82));
    let target = FinalizedWitnessedAssetTransactionTargetV2::exact(tx(107));
    let window = finalized_asset_window();
    assert_wire_roundtrip(
        &ClassifyFinalizedWitnessedAssetFundingV2Result::absent(
            context(),
            terms.clone(),
            target.clone(),
            finalized_asset_clock(),
            window,
        )
        .unwrap(),
    );
    assert_wire_roundtrip(
        &ClassifyFinalizedWitnessedAssetFundingV2Result::uncertain(
            context(),
            terms.clone(),
            target.clone(),
            finalized_asset_clock(),
            window,
        )
        .unwrap(),
    );
    assert_wire_roundtrip(
        &ClassifyFinalizedWitnessedAssetFundingV2Result::unavailable(
            context(),
            terms.clone(),
            target.clone(),
            FinalizedWitnessedAssetUnavailableReasonV2::FinalityUnavailable,
        ),
    );
    let claim = asset_claim_transcript();
    assert_wire_roundtrip(
        &ClassifyFinalizedWitnessedAssetClaimV2Result::absent(
            context(),
            terms.clone(),
            claim.clone(),
            target.clone(),
            finalized_asset_clock(),
            window,
        )
        .unwrap(),
    );
    assert_wire_roundtrip(
        &ClassifyFinalizedWitnessedAssetClaimV2Result::uncertain(
            context(),
            terms.clone(),
            claim.clone(),
            target.clone(),
            finalized_asset_clock(),
            window,
        )
        .unwrap(),
    );
    assert_wire_roundtrip(&ClassifyFinalizedWitnessedAssetClaimV2Result::unavailable(
        context(),
        terms,
        claim,
        target,
        FinalizedWitnessedAssetUnavailableReasonV2::MovingTip,
    ));
}

fn assert_asset_classifier_requests_roundtrip<T>(exact: &T, discovery: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    for request in [exact, discovery] {
        assert_eq!(
            serde_json::from_value::<T>(serde_json::to_value(request).unwrap()).unwrap(),
            *request
        );
    }
    let mut unknown = serde_json::to_value(exact).unwrap();
    unknown["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<T>(unknown).is_err());
}

fn assert_wire_roundtrip<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_eq!(
        &serde_json::from_value::<T>(serde_json::to_value(value).unwrap()).unwrap(),
        value
    );
}

fn assert_asset_scan_statuses_roundtrip(
    absent: ClassifyFinalizedWitnessedAssetInitializationV2Result,
    uncertain: ClassifyFinalizedWitnessedAssetInitializationV2Result,
    unavailable: ClassifyFinalizedWitnessedAssetInitializationV2Result,
) {
    for result in [absent, uncertain, unavailable] {
        assert_eq!(
            serde_json::from_value::<ClassifyFinalizedWitnessedAssetInitializationV2Result>(
                serde_json::to_value(&result).unwrap()
            )
            .unwrap(),
            result
        );
    }
}

fn finalized_asset_window() -> DiscoveryWindow {
    DiscoveryWindow::new(90, 3).unwrap()
}

fn finalized_asset_clock() -> ChainClock {
    ChainClock::new(h(150), 92, 1_850_000_001_900)
}

fn finalized_token_initialization_facts(
    definition: Hex32,
) -> FinalizedWitnessedAssetInitializationFactsV2 {
    let terms = token_asset(definition);
    let token = terms.asset().custom_token().unwrap();
    let metadata = WitnessedEscrowMetadataFacts::from_witnessed_token_terms(
        h(141),
        h(103),
        token,
        EscrowState::Empty,
    );
    FinalizedWitnessedAssetInitializationFactsV2::new(
        finalized_observed_tx(105, h(140), 90, h(73)),
        WitnessedAssetEffectInstructionFactsV2::new(
            WitnessedAssetPrepareStepV2::InitializeWitnessed,
            h(103),
            AccountIds::new(vec![
                metadata.account_id,
                token.depositor_owner_account_id(),
                token.claimant_owner_account_id(),
                token.token_definition_account_id(),
                token.aggregate_authority_account_id(),
            ])
            .unwrap(),
            token.swap_id(),
        ),
        FinalizedBlockIdentity::new(900, h(140), 1_850_000_001_700),
        metadata,
        WitnessedAssetInitializationCustodyFactsV2::custom_token_ata_absent(
            token.custody_ata_account_id(),
        ),
    )
}

fn finalized_token_funding_facts(definition: Hex32) -> FinalizedWitnessedAssetFundingFactsV2 {
    let terms = token_asset(definition);
    let token = terms.asset().custom_token().unwrap();
    let metadata = WitnessedEscrowMetadataFacts::from_witnessed_token_terms(
        h(141),
        h(103),
        token,
        EscrowState::Funded,
    );
    FinalizedWitnessedAssetFundingFactsV2::new(
        finalized_observed_tx(107, h(142), 92, h(73)),
        WitnessedAssetEffectInstructionFactsV2::new(
            WitnessedAssetPrepareStepV2::Fund,
            h(103),
            AccountIds::new(vec![
                metadata.account_id,
                token.depositor_owner_account_id(),
                token.depositor_ata_account_id(),
                token.custody_ata_account_id(),
            ])
            .unwrap(),
            token.swap_id(),
        ),
        FinalizedBlockIdentity::new(902, h(142), 1_850_000_001_800),
        metadata,
        WitnessedAssetCustodyFactsV2::CustomToken(TokenHoldingFactsV2::new(
            token.custody_ata_account_id(),
            token.token_program_id(),
            token.token_definition_account_id(),
            token.amount().as_u128(),
        )),
    )
}

fn finalized_token_custody_creation_facts(
    definition: Hex32,
) -> FinalizedWitnessedAssetCustodyCreationFactsV2 {
    let terms = token_asset(definition);
    let token = terms.asset().custom_token().unwrap();
    let metadata = WitnessedEscrowMetadataFacts::from_witnessed_token_terms(
        h(141),
        h(103),
        token,
        EscrowState::Empty,
    );
    FinalizedWitnessedAssetCustodyCreationFactsV2::new(
        finalized_permissionless_observed_tx(106, h(144), 91),
        WitnessedAssetEffectInstructionFactsV2::new(
            WitnessedAssetPrepareStepV2::CreateCustodyAta,
            h(103),
            AccountIds::new(vec![
                metadata.account_id,
                token.token_definition_account_id(),
                token.custody_ata_account_id(),
            ])
            .unwrap(),
            token.swap_id(),
        ),
        FinalizedBlockIdentity::new(901, h(144), 1_850_000_001_750),
        metadata,
        TokenHoldingFactsV2::new(
            token.custody_ata_account_id(),
            token.token_program_id(),
            token.token_definition_account_id(),
            0,
        ),
    )
}

fn finalized_token_claim_facts(
    definition: Hex32,
    claim: PreparedWitnessedClaim,
) -> FinalizedWitnessedAssetClaimFactsV2 {
    let terms = token_asset(definition);
    let token = terms.asset().custom_token().unwrap();
    let metadata = WitnessedEscrowMetadataFacts::from_witnessed_token_terms(
        h(141),
        h(103),
        token,
        EscrowState::Claimed,
    );
    FinalizedWitnessedAssetClaimFactsV2::new(
        finalized_observed_tx(124, h(143), 91, h(78)),
        WitnessedAssetClaimInstructionFactsV2::new(
            h(103),
            AccountIds::new(vec![
                metadata.account_id,
                token.custody_ata_account_id(),
                token.claimant_owner_account_id(),
                token.claimant_ata_account_id(),
                token.aggregate_authority_account_id(),
            ])
            .unwrap(),
            token.swap_id(),
            claim,
        ),
        AggregateBip340Signature::from_bytes([123; 64]),
        FinalizedBlockIdentity::new(901, h(143), 1_850_000_001_750),
        metadata,
        WitnessedAssetCustodyFactsV2::CustomToken(TokenHoldingFactsV2::new(
            token.custody_ata_account_id(),
            token.token_program_id(),
            token.token_definition_account_id(),
            0,
        )),
    )
}

fn finalized_native_initialization_facts() -> FinalizedWitnessedAssetInitializationFactsV2 {
    let terms = witnessed_native_terms();
    let metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        h(145),
        h(103),
        h(146),
        &terms,
        EscrowState::Empty,
    );
    FinalizedWitnessedAssetInitializationFactsV2::new(
        finalized_observed_tx(132, h(147), 90, terms.depositor_account_id()),
        WitnessedAssetEffectInstructionFactsV2::new(
            WitnessedAssetPrepareStepV2::InitializeWitnessed,
            h(103),
            AccountIds::new(vec![
                metadata.account_id,
                metadata.custody_account_id,
                terms.depositor_account_id(),
                terms.claimant_account_id(),
                terms.aggregate_authority_account_id(),
            ])
            .unwrap(),
            terms.swap_id(),
        ),
        FinalizedBlockIdentity::new(900, h(147), 1_850_000_001_700),
        metadata,
        WitnessedAssetInitializationCustodyFactsV2::native(NativeCustodyFacts::new(
            h(146),
            terms.authenticated_transfer_program_id(),
            0,
        )),
    )
}

fn finalized_native_funding_facts() -> FinalizedWitnessedAssetFundingFactsV2 {
    let terms = witnessed_native_terms();
    let metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        h(145),
        h(103),
        h(146),
        &terms,
        EscrowState::Funded,
    );
    FinalizedWitnessedAssetFundingFactsV2::new(
        finalized_observed_tx(133, h(148), 91, terms.depositor_account_id()),
        WitnessedAssetEffectInstructionFactsV2::new(
            WitnessedAssetPrepareStepV2::Fund,
            h(103),
            AccountIds::new(vec![
                metadata.account_id,
                metadata.custody_account_id,
                terms.depositor_account_id(),
            ])
            .unwrap(),
            terms.swap_id(),
        ),
        FinalizedBlockIdentity::new(901, h(148), 1_850_000_001_750),
        metadata,
        WitnessedAssetCustodyFactsV2::Native(NativeCustodyFacts::new(
            h(146),
            terms.authenticated_transfer_program_id(),
            terms.amount().as_u128(),
        )),
    )
}

fn finalized_native_claim_facts(
    claim: PreparedWitnessedClaim,
) -> FinalizedWitnessedAssetClaimFactsV2 {
    let terms = witnessed_native_terms();
    let metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        h(145),
        h(103),
        h(146),
        &terms,
        EscrowState::Claimed,
    );
    FinalizedWitnessedAssetClaimFactsV2::new(
        finalized_observed_tx(134, h(149), 92, terms.aggregate_authority_account_id()),
        WitnessedAssetClaimInstructionFactsV2::new(
            h(103),
            AccountIds::new(vec![
                metadata.account_id,
                metadata.custody_account_id,
                terms.claimant_account_id(),
                terms.aggregate_authority_account_id(),
            ])
            .unwrap(),
            terms.swap_id(),
            claim,
        ),
        AggregateBip340Signature::from_bytes([123; 64]),
        FinalizedBlockIdentity::new(902, h(149), 1_850_000_001_800),
        metadata,
        WitnessedAssetCustodyFactsV2::Native(NativeCustodyFacts::new(
            h(146),
            terms.authenticated_transfer_program_id(),
            0,
        )),
    )
}

fn finalized_permissionless_observed_tx(
    byte: u8,
    block_hash: Hex32,
    height: u64,
) -> ObservedTransactionFacts {
    ObservedTransactionFacts::new(
        TransactionId::from_bytes([byte; 32]),
        ExactTransactionBytes::new(vec![byte; 128]).unwrap(),
        ChainPosition::new(block_hash, height, 0),
        AccountIds::new(Vec::new()).unwrap(),
        true,
    )
}

fn finalized_observed_tx(
    byte: u8,
    block_hash: Hex32,
    height: u64,
    signer: Hex32,
) -> ObservedTransactionFacts {
    ObservedTransactionFacts::new(
        TransactionId::from_bytes([byte; 32]),
        ExactTransactionBytes::new(vec![byte; 128]).unwrap(),
        ChainPosition::new(block_hash, height, 0),
        AccountIds::new(vec![signer]).unwrap(),
        true,
    )
}

fn asset_claim_transcript() -> PreparedWitnessedClaim {
    PreparedWitnessedClaim::new(
        RequestId::new("asset-v2-claim-prepare").unwrap(),
        h(120),
        ExactMessageBytes::new(vec![121; 128]).unwrap(),
    )
}

fn assert_finalized_initialization_substitutions_fail(encoded: &serde_json::Value) {
    assert_json_mutations_fail::<ClassifyFinalizedWitnessedAssetInitializationV2Result>(
        encoded,
        vec![
            (
                vec!["target", "transaction"],
                serde_json::to_value(tx(220)).unwrap(),
            ),
            (
                vec![
                    "outcome",
                    "facts",
                    "instruction",
                    "ordered_account_ids",
                    "1",
                ],
                serde_json::to_value(h(221)).unwrap(),
            ),
            (
                vec!["outcome", "facts", "metadata", "asset_definition"],
                serde_json::to_value(h(222)).unwrap(),
            ),
            (
                vec!["outcome", "facts", "containing_block", "block_hash"],
                serde_json::to_value(h(223)).unwrap(),
            ),
            (
                vec!["outcome", "finalized_clock", "height"],
                serde_json::json!(91),
            ),
            (
                vec!["outcome", "facts", "transaction", "position", "height"],
                serde_json::json!(93),
            ),
            (
                vec!["outcome", "facts", "unexpected"],
                serde_json::json!(true),
            ),
        ],
    );
}

fn assert_finalized_funding_substitutions_fail(encoded: &serde_json::Value) {
    assert_json_mutations_fail::<ClassifyFinalizedWitnessedAssetFundingV2Result>(
        encoded,
        vec![
            (
                vec!["target", "transaction"],
                serde_json::to_value(tx(221)).unwrap(),
            ),
            (
                vec![
                    "outcome",
                    "facts",
                    "custody",
                    "facts",
                    "token_definition_account_id",
                ],
                serde_json::to_value(h(222)).unwrap(),
            ),
            (
                vec![
                    "outcome",
                    "facts",
                    "instruction",
                    "ordered_account_ids",
                    "2",
                ],
                serde_json::to_value(h(223)).unwrap(),
            ),
            (
                vec![
                    "outcome",
                    "facts",
                    "metadata",
                    "aggregate_authority_account_id",
                ],
                serde_json::to_value(h(224)).unwrap(),
            ),
            (
                vec!["outcome", "facts", "transaction", "exact_bytes"],
                serde_json::to_value(ExactTransactionBytes::new(vec![225; 128]).unwrap()).unwrap(),
            ),
        ],
    );
}

fn assert_finalized_custody_creation_substitutions_fail(encoded: &serde_json::Value) {
    assert_json_mutations_fail::<ClassifyFinalizedWitnessedAssetCustodyCreationV2Result>(
        encoded,
        vec![
            (
                vec!["target", "transaction"],
                serde_json::to_value(tx(221)).unwrap(),
            ),
            (
                vec![
                    "outcome",
                    "facts",
                    "instruction",
                    "ordered_account_ids",
                    "1",
                ],
                serde_json::to_value(h(222)).unwrap(),
            ),
            (
                vec!["outcome", "facts", "metadata", "custody_program_id"],
                serde_json::to_value(h(223)).unwrap(),
            ),
            (
                vec!["outcome", "facts", "custody", "token_definition_account_id"],
                serde_json::to_value(h(224)).unwrap(),
            ),
            (
                vec!["outcome", "facts", "containing_block", "block_hash"],
                serde_json::to_value(h(225)).unwrap(),
            ),
        ],
    );
}

fn assert_finalized_claim_substitutions_fail(encoded: &serde_json::Value) {
    assert_json_mutations_fail::<ClassifyFinalizedWitnessedAssetClaimV2Result>(
        encoded,
        vec![
            (
                vec!["target", "transaction"],
                serde_json::to_value(tx(221)).unwrap(),
            ),
            (
                vec![
                    "outcome",
                    "facts",
                    "instruction",
                    "ordered_account_ids",
                    "4",
                ],
                serde_json::to_value(h(222)).unwrap(),
            ),
            (
                vec!["outcome", "facts", "metadata", "claimant_asset_account_id"],
                serde_json::to_value(h(223)).unwrap(),
            ),
            (vec!["outcome", "facts"], serde_json::json!([])),
            (vec!["outcome", "unexpected"], serde_json::json!(true)),
        ],
    );
}

fn assert_json_mutations_fail<T>(
    encoded: &serde_json::Value,
    mutations: Vec<(Vec<&str>, serde_json::Value)>,
) where
    T: serde::de::DeserializeOwned,
{
    for (path, replacement) in mutations {
        let mut changed = encoded.clone();
        json_set(&mut changed, &path, replacement);
        assert!(
            serde_json::from_value::<T>(changed).is_err(),
            "mutation at {path:?} was accepted"
        );
    }
}

fn token_asset(definition: Hex32) -> WitnessedLezAssetTermsV2 {
    WitnessedLezAssetTermsV2::custom_token(
        WitnessedTokenEscrowTermsV2::new(witnessed_token_input(definition)).unwrap(),
    )
}

fn token_prepared_effects() -> Vec<WitnessedAssetPreparedEffectV2> {
    vec![
        WitnessedAssetPreparedEffectV2::new(
            WitnessedAssetPrepareStepV2::InitializeWitnessed,
            tx(105),
        ),
        WitnessedAssetPreparedEffectV2::new(WitnessedAssetPrepareStepV2::CreateCustodyAta, tx(106)),
        WitnessedAssetPreparedEffectV2::new(WitnessedAssetPrepareStepV2::Fund, tx(107)),
    ]
}

fn token_observed_prepare_effects(
    token: &WitnessedTokenEscrowTermsV2,
    metadata: Hex32,
) -> Vec<WitnessedAssetObservedPrepareEffectV2> {
    vec![
        WitnessedAssetObservedPrepareEffectV2::new(
            WitnessedAssetPrepareStepV2::InitializeWitnessed,
            observed_tx(105),
            h(103),
            AccountIds::new(vec![
                metadata,
                token.depositor_owner_account_id(),
                token.claimant_owner_account_id(),
                token.token_definition_account_id(),
                token.aggregate_authority_account_id(),
            ])
            .unwrap(),
        ),
        WitnessedAssetObservedPrepareEffectV2::new(
            WitnessedAssetPrepareStepV2::CreateCustodyAta,
            observed_tx(106),
            h(103),
            AccountIds::new(vec![
                metadata,
                token.token_definition_account_id(),
                token.custody_ata_account_id(),
            ])
            .unwrap(),
        ),
        WitnessedAssetObservedPrepareEffectV2::new(
            WitnessedAssetPrepareStepV2::Fund,
            observed_tx(107),
            h(103),
            AccountIds::new(vec![
                metadata,
                token.depositor_owner_account_id(),
                token.depositor_ata_account_id(),
                token.custody_ata_account_id(),
            ])
            .unwrap(),
        ),
    ]
}

fn observed_tx(byte: u8) -> ObservedTransactionFacts {
    ObservedTransactionFacts::new(
        TransactionId::from_bytes([byte; 32]),
        ExactTransactionBytes::new(vec![byte; 128]).unwrap(),
        ChainPosition::new(h(byte.wrapping_add(1)), u64::from(byte), 0),
        AccountIds::new(vec![h(73)]).unwrap(),
        true,
    )
}

fn json_set(value: &mut serde_json::Value, path: &[&str], replacement: serde_json::Value) {
    let (last, parents) = path.split_last().unwrap();
    let mut current = value;
    for key in parents {
        current = if current.is_array() {
            &mut current[key.parse::<usize>().unwrap()]
        } else {
            &mut current[*key]
        };
    }
    if current.is_array() {
        current[last.parse::<usize>().unwrap()] = replacement;
    } else {
        current[*last] = replacement;
    }
}
