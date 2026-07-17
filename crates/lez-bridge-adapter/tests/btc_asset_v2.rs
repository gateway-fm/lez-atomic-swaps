use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bitcoin::{
    Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
    absolute::LockTime,
    hashes::Hash as _,
    secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey},
    transaction::Version,
};
use lez_bridge_adapter::{
    BtcLezAssetBridgeBindingV2, BtcLezAssetBridgeBindingV2Error, BtcLezAssetBridgeV2Error,
    BtcLezAssetFirstLockProofV2Error, LezBridgeAdapter, LezBridgeAssetV2Transport,
};
use lez_bridge_client::BridgeClient;
use lez_bridge_protocol::{
    AccountIds, AggregateBip340Signature, ChainClock, ChainPosition, ChainTip,
    ClassifyFinalizedWitnessedAssetClaimV2Request, ClassifyFinalizedWitnessedAssetClaimV2Result,
    ClassifyFinalizedWitnessedAssetCustodyCreationV2Request,
    ClassifyFinalizedWitnessedAssetCustodyCreationV2Result,
    ClassifyFinalizedWitnessedAssetFundingV2Request,
    ClassifyFinalizedWitnessedAssetFundingV2Result,
    ClassifyFinalizedWitnessedAssetInitializationV2Request,
    ClassifyFinalizedWitnessedAssetInitializationV2Result, CompleteWitnessedAssetClaimV2Request,
    CompleteWitnessedAssetClaimV2Result, DiscoveryWindow, EscrowState, ExactMessageBytes,
    ExactTransactionBytes, FinalizedBlockIdentity, FinalizedWitnessedAssetInitializationFactsV2,
    FinalizedWitnessedAssetScanOutcomeV2, FinalizedWitnessedAssetTransactionTargetV2,
    FinalizedWitnessedAssetUnavailableReasonV2, FinalizedWitnessedClaimObservationTarget, Hex32,
    MessageContext, NativeAmount, NativeCustodyFacts, NativeRefundObservationTarget,
    ObserveFinalizedWitnessedAssetClaimV2Request, ObserveFinalizedWitnessedAssetClaimV2Result,
    ObserveWitnessedAssetEscrowV2Request, ObserveWitnessedAssetEscrowV2Result,
    ObserveWitnessedAssetRefundV2Request, ObserveWitnessedAssetRefundV2Result,
    ObservedTransactionFacts, Participant as BridgeParticipant,
    PrepareWitnessedAssetClaimV2Request, PrepareWitnessedAssetClaimV2Result,
    PrepareWitnessedAssetEscrowV2Request, PrepareWitnessedAssetEscrowV2Result,
    PrepareWitnessedAssetRefundV2Request, PrepareWitnessedAssetRefundV2Result, PreparedTransaction,
    PreparedWitnessedClaim, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
    TokenHoldingFactsV2, TransactionId, WitnessedAssetCustodyFactsV2,
    WitnessedAssetEffectInstructionFactsV2, WitnessedAssetInitializationCustodyFactsV2,
    WitnessedAssetObservedPrepareEffectV2, WitnessedAssetPrepareStepV2,
    WitnessedAssetPreparedEffectV2, WitnessedEscrowMetadataFacts, WitnessedLezAssetV2,
};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, BTC_AGREEMENT_SCHEMA_V1, BTC_LEZ_ASSET_EXTENSION_SCHEMA_V1,
    BtcAgreementBodyV1, BtcAgreementRecordV1, BtcAgreementV1, BtcChainPolicyV1, BtcClaimTermsV1,
    BtcFundingTermsV1, BtcLezAssetExtensionBodyV1, BtcLezAssetExtensionRecordV1,
    BtcLezAssetExtensionV1, BtcLezAssetV1, BtcLezCustomTokenTermsV1, BtcLezTermsV1, BtcP2trTermsV1,
    BtcParticipantIdentityV1, BtcParticipantsV1, BtcRecoveryPlanV1, CooperativeKeyPathSpend,
    CsvBlockDelay, P2trSwapOutput, RefundXOnlyKey, TwoPartyAggregateKey,
};
use lez_swap_core::{Participant, SwapDirection};

const LEZ_CHANNEL: [u8; 32] = [17; 32];
const LEZ_GENESIS: [u8; 32] = [18; 32];
const ESCROW_PROGRAM: [u8; 32] = [15; 32];
const TRANSFER_PROGRAM: [u8; 32] = [16; 32];
const METADATA_ACCOUNT: [u8; 32] = [13; 32];
const CUSTODY_ACCOUNT: [u8; 32] = [14; 32];
const MAKER_ACCOUNT: [u8; 32] = [10; 32];
const TAKER_ACCOUNT: [u8; 32] = [11; 32];
const LEZ_AMOUNT: u128 = 5_000;

#[test]
fn f7_adapter_exposes_an_exactly_once_bridge_client_transport() {
    fn assert_transport<T: LezBridgeAssetV2Transport>() {}
    assert_transport::<BridgeClient>();
}

#[test]
// Keeping every signed field adjacent makes omissions across the three asset policies visible.
#[allow(clippy::too_many_lines)]
fn native_and_two_token_policies_map_every_countersigned_field_in_both_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let agreement = agreement(direction);
        let assets = [
            BtcLezAssetV1::Native,
            token_asset(&agreement, 40),
            token_asset(&agreement, 50),
        ];
        for expected_asset in assets {
            let extension = extension(&agreement, expected_asset.clone());
            let binding = BtcLezAssetBridgeBindingV2::new(&agreement, &extension, &expected_asset)
                .expect("policy-bound bridge terms");
            assert_eq!(binding.depositor(), agreement.lez_depositor());
            assert_eq!(binding.claimant(), agreement.lez_claimant());
            match (binding.terms().asset(), expected_asset) {
                (WitnessedLezAssetV2::Native(actual), BtcLezAssetV1::Native) => {
                    let signed = agreement.lez_terms();
                    assert_eq!(actual.swap_id().as_bytes(), agreement.body().swap_id());
                    assert_eq!(actual.terms_hash().as_bytes(), extension.asset_commitment());
                    assert_eq!(
                        actual.depositor(),
                        bridge_participant(agreement.lez_depositor())
                    );
                    assert_eq!(
                        actual.depositor_account_id().as_bytes(),
                        signed.depositor_account()
                    );
                    assert_eq!(
                        actual.claimant(),
                        bridge_participant(agreement.lez_claimant())
                    );
                    assert_eq!(
                        actual.claimant_account_id().as_bytes(),
                        signed.claimant_account()
                    );
                    assert_eq!(
                        actual.aggregate_authority_account_id().as_bytes(),
                        signed.aggregate_authority_account()
                    );
                    assert_eq!(
                        actual.aggregate_x_only_public_key().as_bytes(),
                        &agreement.p2tr_contract().aggregate_internal_key_bytes()
                    );
                    assert_eq!(actual.amount().as_u128(), signed.amount());
                    assert_eq!(actual.refund_at_ms(), signed.refund_at_ms());
                    assert_eq!(
                        actual.authenticated_transfer_program_id().as_bytes(),
                        signed.authenticated_transfer_program_id()
                    );
                }
                (
                    WitnessedLezAssetV2::CustomToken(actual),
                    BtcLezAssetV1::CustomToken(expected),
                ) => {
                    assert_eq!(actual.swap_id().as_bytes(), agreement.body().swap_id());
                    assert_eq!(actual.terms_hash().as_bytes(), extension.asset_commitment());
                    assert_eq!(
                        actual.depositor(),
                        bridge_participant(agreement.lez_depositor())
                    );
                    assert_eq!(
                        actual.depositor_owner_account_id().as_bytes(),
                        expected.depositor_owner_account()
                    );
                    assert_eq!(
                        actual.depositor_ata_account_id().as_bytes(),
                        expected.depositor_ata_account()
                    );
                    assert_eq!(
                        actual.claimant(),
                        bridge_participant(agreement.lez_claimant())
                    );
                    assert_eq!(
                        actual.claimant_owner_account_id().as_bytes(),
                        expected.claimant_owner_account()
                    );
                    assert_eq!(
                        actual.claimant_ata_account_id().as_bytes(),
                        expected.claimant_ata_account()
                    );
                    assert_eq!(
                        actual.custody_ata_account_id().as_bytes(),
                        expected.custody_ata_account()
                    );
                    assert_eq!(
                        actual.token_program_id().as_bytes(),
                        expected.token_program_id()
                    );
                    assert_eq!(
                        actual.ata_program_id().as_bytes(),
                        expected.ata_program_id()
                    );
                    assert_eq!(
                        actual.token_definition_account_id().as_bytes(),
                        expected.token_definition_account()
                    );
                    assert_eq!(
                        actual.aggregate_authority_account_id().as_bytes(),
                        expected.aggregate_authority_account()
                    );
                    assert_eq!(
                        actual.aggregate_x_only_public_key().as_bytes(),
                        expected.aggregate_x_only_public_key()
                    );
                    assert_eq!(actual.amount().as_u128(), expected.amount());
                    assert_eq!(actual.refund_at_ms(), expected.refund_at_ms());
                }
                _ => panic!("asset kind changed at the adapter boundary"),
            }
        }
    }
}

#[test]
fn cross_agreement_and_local_policy_drift_fail_before_an_adapter_exists() {
    let agreement_a = agreement(SwapDirection::TakerSellsForeign);
    let agreement_b = agreement(SwapDirection::TakerSellsLez);
    let selected = token_asset(&agreement_a, 40);
    let extension = extension(&agreement_a, selected.clone());
    assert!(matches!(
        BtcLezAssetBridgeBindingV2::new(&agreement_b, &extension, &selected),
        Err(BtcLezAssetBridgeBindingV2Error::BaseAgreementMismatch)
    ));
    assert!(matches!(
        BtcLezAssetBridgeBindingV2::new(&agreement_a, &extension, &token_asset(&agreement_a, 50),),
        Err(BtcLezAssetBridgeBindingV2Error::LocalAssetPolicy(_))
    ));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn all_eleven_calls_are_once_role_correct_and_preserve_caller_owned_values() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    let selected = token_asset(&agreement, 40);
    let extension = extension(&agreement, selected.clone());
    let binding =
        BtcLezAssetBridgeBindingV2::new(&agreement, &extension, &selected).expect("binding");
    assert_eq!(binding.depositor(), Participant::Maker);
    assert_eq!(binding.claimant(), Participant::Taker);

    let transport = RecordingTransport::default();
    let maker = adapter(
        transport.clone(),
        Participant::Maker,
        runtime(Participant::Maker),
    );
    let taker = adapter(
        transport.clone(),
        Participant::Taker,
        runtime(Participant::Taker),
    );
    let window = DiscoveryWindow::new(70, 4).expect("window");
    let effects = token_effects();
    let claim = claim();
    let signature = AggregateBip340Signature::from_bytes([90; 64]);
    let exact_target = FinalizedWitnessedAssetTransactionTargetV2::Exact {
        transaction: tx(93),
    };

    assert_transport_error(
        &maker
            .prepare_btc_asset_escrow_v2(&binding, rid("prepare-escrow"))
            .await,
    );
    assert_transport_error(
        &maker
            .observe_btc_asset_escrow_v2(&binding, rid("observe-escrow"), effects.clone(), window)
            .await,
    );
    assert_transport_error(
        &taker
            .prepare_btc_asset_claim_v2(
                &binding,
                rid("prepare-claim"),
                TransactionId::from_bytes([91; 32]),
            )
            .await,
    );
    assert_transport_error(
        &taker
            .complete_btc_asset_claim_v2(&binding, rid("complete-claim"), claim.clone(), signature)
            .await,
    );
    assert_transport_error(
        &maker
            .observe_finalized_btc_asset_claim_v2(
                &binding,
                rid("observe-claim"),
                claim.clone(),
                FinalizedWitnessedClaimObservationTarget::DiscoverByTerms,
                window,
            )
            .await,
    );
    assert_transport_error(
        &taker
            .prepare_btc_asset_refund_v2(&binding, rid("prepare-refund"))
            .await,
    );
    assert_transport_error(
        &maker
            .observe_btc_asset_refund_v2(
                &binding,
                rid("observe-refund"),
                NativeRefundObservationTarget::Exact {
                    refund_transaction_id: TransactionId::from_bytes([92; 32]),
                    window,
                },
            )
            .await,
    );
    assert_transport_error(
        &maker
            .classify_finalized_btc_asset_initialization_v2(
                &binding,
                rid("classify-init"),
                exact_target.clone(),
                window,
            )
            .await,
    );
    assert_transport_error(
        &maker
            .classify_finalized_btc_asset_custody_creation_v2(
                &binding,
                rid("classify-custody"),
                FinalizedWitnessedAssetTransactionTargetV2::DiscoverByTerms {},
                window,
            )
            .await,
    );
    assert_transport_error(
        &maker
            .classify_finalized_btc_asset_funding_v2(
                &binding,
                rid("classify-funding"),
                exact_target.clone(),
                window,
            )
            .await,
    );
    assert_transport_error(
        &maker
            .classify_finalized_btc_asset_claim_v2(
                &binding,
                rid("classify-claim"),
                claim.clone(),
                exact_target,
                window,
            )
            .await,
    );

    let requests = transport.requests.lock().expect("request log");
    assert_eq!(requests.len(), 11);
    assert_eq!(
        requests
            .iter()
            .map(RecordedRequest::operation)
            .collect::<Vec<_>>(),
        Operation::ALL
    );
    for request in requests.iter() {
        assert_eq!(request.terms(), binding.terms());
        assert_eq!(request.context().run_id, run_id());
    }
    let RecordedRequest::ObserveEscrow(request) = &requests[1] else {
        panic!("observe escrow request");
    };
    assert_eq!(request.prepared_effects, effects);
    assert_eq!(request.window, window);
    let RecordedRequest::CompleteClaim(request) = &requests[3] else {
        panic!("complete claim request");
    };
    assert_eq!(request.claim, claim);
    assert_eq!(request.aggregate_signature, signature);
    let RecordedRequest::ObserveRefund(request) = &requests[6] else {
        panic!("observe refund request");
    };
    assert!(matches!(
        request.target,
        NativeRefundObservationTarget::Exact { window: actual, .. } if actual == window
    ));
    let RecordedRequest::ClassifyClaim(request) = &requests[10] else {
        panic!("classify claim request");
    };
    assert_eq!(request.claim, claim);
    assert_eq!(request.window, window);
}

#[tokio::test]
async fn wrong_roles_runtime_signer_and_native_custody_classification_are_zero_wire() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    let selected = token_asset(&agreement, 40);
    let binding = BtcLezAssetBridgeBindingV2::new(
        &agreement,
        &extension(&agreement, selected.clone()),
        &selected,
    )
    .expect("binding");
    let transport = RecordingTransport::default();
    let maker = adapter(
        transport.clone(),
        Participant::Maker,
        runtime(Participant::Maker),
    );
    let taker = adapter(
        transport.clone(),
        Participant::Taker,
        runtime(Participant::Taker),
    );
    assert!(matches!(
        taker
            .prepare_btc_asset_escrow_v2(&binding, rid("wrong-depositor"))
            .await,
        Err(BtcLezAssetBridgeV2Error::WrongDepositor)
    ));
    assert!(matches!(
        maker
            .prepare_btc_asset_claim_v2(
                &binding,
                rid("wrong-claimant"),
                TransactionId::from_bytes([1; 32]),
            )
            .await,
        Err(BtcLezAssetBridgeV2Error::WrongClaimant)
    ));
    let wrong_signer = adapter(
        transport.clone(),
        Participant::Maker,
        runtime_with(
            Participant::Maker,
            RuntimeCompatibility::LeeV0_2_0,
            [99; 32],
        ),
    );
    assert!(matches!(
        wrong_signer
            .prepare_btc_asset_refund_v2(&binding, rid("wrong-signer"))
            .await,
        Err(BtcLezAssetBridgeV2Error::SignerAccountMismatch)
    ));
    let wrong_runtime = adapter(
        transport.clone(),
        Participant::Maker,
        runtime_with(
            Participant::Maker,
            RuntimeCompatibility::NssaV0_1_2,
            MAKER_ACCOUNT,
        ),
    );
    assert!(matches!(
        wrong_runtime
            .prepare_btc_asset_refund_v2(&binding, rid("wrong-runtime"))
            .await,
        Err(BtcLezAssetBridgeV2Error::IncompatibleRuntime)
    ));

    let native = BtcLezAssetV1::Native;
    let native_binding = BtcLezAssetBridgeBindingV2::new(
        &agreement,
        &extension(&agreement, native.clone()),
        &native,
    )
    .expect("native binding");
    assert!(matches!(
        maker
            .classify_finalized_btc_asset_custody_creation_v2(
                &native_binding,
                rid("native-custody"),
                FinalizedWitnessedAssetTransactionTargetV2::DiscoverByTerms {},
                DiscoveryWindow::new(1, 1).expect("window"),
            )
            .await,
        Err(BtcLezAssetBridgeV2Error::CustodyCreationRequiresCustomToken)
    ));
    assert!(transport.requests.lock().expect("request log").is_empty());
}

#[tokio::test]
async fn finalized_classification_preserves_all_four_conservative_outcomes() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    let selected = BtcLezAssetV1::Native;
    let binding = BtcLezAssetBridgeBindingV2::new(
        &agreement,
        &extension(&agreement, selected.clone()),
        &selected,
    )
    .expect("binding");
    let target = FinalizedWitnessedAssetTransactionTargetV2::Exact {
        transaction: tx(94),
    };
    let window = DiscoveryWindow::new(70, 4).expect("window");

    for expected in InitializationOutcome::ALL {
        let transport = RecordingTransport::with_initialization_outcome(expected);
        let adapter = adapter(
            transport.clone(),
            Participant::Maker,
            runtime(Participant::Maker),
        );
        let outcome = adapter
            .classify_finalized_btc_asset_initialization_v2(
                &binding,
                rid(expected.request_suffix()),
                target.clone(),
                window,
            )
            .await
            .expect("successful classification");
        assert!(
            matches!(
                (expected, outcome),
                (
                    InitializationOutcome::Found,
                    FinalizedWitnessedAssetScanOutcomeV2::Found { .. }
                ) | (
                    InitializationOutcome::Absent,
                    FinalizedWitnessedAssetScanOutcomeV2::Absent { .. }
                ) | (
                    InitializationOutcome::Uncertain,
                    FinalizedWitnessedAssetScanOutcomeV2::Uncertain { .. }
                ) | (
                    InitializationOutcome::Unavailable,
                    FinalizedWitnessedAssetScanOutcomeV2::Unavailable { .. }
                )
            ),
            "adapter collapsed {expected:?}"
        );
        assert_eq!(transport.requests.lock().expect("request log").len(), 1);
    }
}

#[tokio::test]
async fn custom_token_first_lock_proof_returns_exact_sdk_material() {
    let agreement = agreement(SwapDirection::TakerSellsLez);
    let selected = token_asset(&agreement, 40);
    let binding = BtcLezAssetBridgeBindingV2::new(
        &agreement,
        &extension(&agreement, selected.clone()),
        &selected,
    )
    .expect("binding");
    let preparation = asset_preparation(&binding, token_effects());
    let transport = RecordingTransport::with_asset_proof(AssetProofMutation::None);
    let proof = adapter(
        transport.clone(),
        Participant::Maker,
        runtime(Participant::Maker),
    )
    .prove_btc_asset_first_lock_v2(
        &binding,
        rid("prove-token-first-lock"),
        &preparation,
        proof_window(),
    )
    .await
    .expect("exact token proof");

    assert_eq!(proof.finalized_tip().height, 73);
    assert_eq!(proof.prepared().plan(), proof.evidence().observed_plan());
    assert_eq!(
        proof
            .prepared()
            .plan()
            .steps()
            .iter()
            .map(|step| step.step().as_str())
            .collect::<Vec<_>>(),
        ["lez.initialize", "lez.create_custody_ata", "lez.fund"]
    );
    assert_eq!(
        proof
            .prepared()
            .plan()
            .steps()
            .iter()
            .map(|step| step.expected_public_id().as_str())
            .collect::<Vec<_>>(),
        vec!["01".repeat(32), "02".repeat(32), "03".repeat(32)]
    );
    assert_eq!(proof.evidence().metadata_account(), &[13; 32]);
    assert_eq!(proof.evidence().amount(), LEZ_AMOUNT);
    let (prepared, evidence) = proof.into_sdk_parts();
    assert_eq!(prepared.plan(), evidence.observed_plan());
    assert_eq!(transport.requests.lock().expect("request log").len(), 1);
}

#[tokio::test]
async fn native_first_lock_proof_preserves_two_step_parity() {
    let agreement = agreement(SwapDirection::TakerSellsLez);
    let selected = BtcLezAssetV1::Native;
    let binding = BtcLezAssetBridgeBindingV2::new(
        &agreement,
        &extension(&agreement, selected.clone()),
        &selected,
    )
    .expect("binding");
    let preparation = asset_preparation(
        &binding,
        vec![
            WitnessedAssetPreparedEffectV2::new(
                WitnessedAssetPrepareStepV2::InitializeWitnessed,
                tx(4),
            ),
            WitnessedAssetPreparedEffectV2::new(WitnessedAssetPrepareStepV2::Fund, tx(5)),
        ],
    );
    let proof = adapter(
        RecordingTransport::with_asset_proof(AssetProofMutation::None),
        Participant::Maker,
        runtime(Participant::Maker),
    )
    .prove_btc_asset_first_lock_v2(
        &binding,
        rid("prove-native-first-lock"),
        &preparation,
        proof_window(),
    )
    .await
    .expect("exact native proof");

    assert_eq!(proof.prepared().plan().steps().len(), 2);
    assert!(matches!(
        proof.evidence().custody(),
        lez_btc_swap_sdk::LezAssetCustodyEvidenceV1::Native { custody_account }
            if *custody_account == CUSTODY_ACCOUNT
    ));
}

#[tokio::test]
async fn first_lock_proof_fails_closed_on_finality_placement_and_exact_effect_drift() {
    for (mutation, expected) in [
        (AssetProofMutation::TipDrift, "tip"),
        (AssetProofMutation::IncompleteWindow, "window"),
        (AssetProofMutation::PlacementOutsideWindow, "placement"),
        (AssetProofMutation::PlacementOrder, "chronological"),
        (AssetProofMutation::StepDrift, "step"),
        (AssetProofMutation::IdentityDrift, "identity"),
        (AssetProofMutation::BytesDrift, "bytes"),
        (AssetProofMutation::NonPublic, "public transaction"),
        (AssetProofMutation::SameHeightFork, "canonical block"),
        (AssetProofMutation::CustodyDrift, "observation"),
        (AssetProofMutation::AmountDrift, "observation"),
        (AssetProofMutation::MetadataDrift, "metadata account"),
    ] {
        let agreement = agreement(SwapDirection::TakerSellsLez);
        let selected = token_asset(&agreement, 40);
        let binding = BtcLezAssetBridgeBindingV2::new(
            &agreement,
            &extension(&agreement, selected.clone()),
            &selected,
        )
        .expect("binding");
        let preparation = asset_preparation(&binding, token_effects());
        let error = adapter(
            RecordingTransport::with_asset_proof(mutation),
            Participant::Maker,
            runtime(Participant::Maker),
        )
        .prove_btc_asset_first_lock_v2(
            &binding,
            rid(&format!("prove-drift-{mutation:?}").to_lowercase()),
            &preparation,
            proof_window(),
        )
        .await
        .expect_err("drift must not produce SDK material");
        assert!(
            error.to_string().contains(expected),
            "{mutation:?}: {error}"
        );
    }
}

#[tokio::test]
async fn first_lock_proof_never_turns_missing_or_unavailable_reads_into_evidence() {
    let agreement = agreement(SwapDirection::TakerSellsLez);
    let selected = token_asset(&agreement, 40);
    let binding = BtcLezAssetBridgeBindingV2::new(
        &agreement,
        &extension(&agreement, selected.clone()),
        &selected,
    )
    .expect("binding");
    let preparation = asset_preparation(&binding, token_effects());

    for outcome in [AssetProofMutation::Missing, AssetProofMutation::Unavailable] {
        let error = adapter(
            RecordingTransport::with_asset_proof(outcome),
            Participant::Maker,
            runtime(Participant::Maker),
        )
        .prove_btc_asset_first_lock_v2(
            &binding,
            rid(&format!("prove-no-result-{outcome:?}").to_lowercase()),
            &preparation,
            proof_window(),
        )
        .await
        .expect_err("no result must not produce SDK material");
        assert!(matches!(
            error,
            BtcLezAssetFirstLockProofV2Error::Bridge(BtcLezAssetBridgeV2Error::Transport(
                TestTransportError
            ))
        ));
    }
}

fn assert_transport_error<T>(result: &Result<T, BtcLezAssetBridgeV2Error<TestTransportError>>) {
    assert!(matches!(
        result,
        Err(BtcLezAssetBridgeV2Error::Transport(TestTransportError))
    ));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    PrepareEscrow,
    ObserveEscrow,
    PrepareClaim,
    CompleteClaim,
    ObserveClaim,
    PrepareRefund,
    ObserveRefund,
    ClassifyInitialization,
    ClassifyCustody,
    ClassifyFunding,
    ClassifyClaim,
}

impl Operation {
    const ALL: [Self; 11] = [
        Self::PrepareEscrow,
        Self::ObserveEscrow,
        Self::PrepareClaim,
        Self::CompleteClaim,
        Self::ObserveClaim,
        Self::PrepareRefund,
        Self::ObserveRefund,
        Self::ClassifyInitialization,
        Self::ClassifyCustody,
        Self::ClassifyFunding,
        Self::ClassifyClaim,
    ];
}

#[derive(Debug)]
enum RecordedRequest {
    PrepareEscrow(Box<PrepareWitnessedAssetEscrowV2Request>),
    ObserveEscrow(Box<ObserveWitnessedAssetEscrowV2Request>),
    PrepareClaim(Box<PrepareWitnessedAssetClaimV2Request>),
    CompleteClaim(Box<CompleteWitnessedAssetClaimV2Request>),
    ObserveClaim(Box<ObserveFinalizedWitnessedAssetClaimV2Request>),
    PrepareRefund(Box<PrepareWitnessedAssetRefundV2Request>),
    ObserveRefund(Box<ObserveWitnessedAssetRefundV2Request>),
    ClassifyInitialization(Box<ClassifyFinalizedWitnessedAssetInitializationV2Request>),
    ClassifyCustody(Box<ClassifyFinalizedWitnessedAssetCustodyCreationV2Request>),
    ClassifyFunding(Box<ClassifyFinalizedWitnessedAssetFundingV2Request>),
    ClassifyClaim(Box<ClassifyFinalizedWitnessedAssetClaimV2Request>),
}

impl RecordedRequest {
    fn operation(&self) -> Operation {
        match self {
            Self::PrepareEscrow(_) => Operation::PrepareEscrow,
            Self::ObserveEscrow(_) => Operation::ObserveEscrow,
            Self::PrepareClaim(_) => Operation::PrepareClaim,
            Self::CompleteClaim(_) => Operation::CompleteClaim,
            Self::ObserveClaim(_) => Operation::ObserveClaim,
            Self::PrepareRefund(_) => Operation::PrepareRefund,
            Self::ObserveRefund(_) => Operation::ObserveRefund,
            Self::ClassifyInitialization(_) => Operation::ClassifyInitialization,
            Self::ClassifyCustody(_) => Operation::ClassifyCustody,
            Self::ClassifyFunding(_) => Operation::ClassifyFunding,
            Self::ClassifyClaim(_) => Operation::ClassifyClaim,
        }
    }

    fn context(&self) -> &lez_bridge_protocol::MessageContext {
        match self {
            Self::PrepareEscrow(request) => &request.context,
            Self::ObserveEscrow(request) => &request.context,
            Self::PrepareClaim(request) => &request.context,
            Self::CompleteClaim(request) => &request.context,
            Self::ObserveClaim(request) => &request.context,
            Self::PrepareRefund(request) => &request.context,
            Self::ObserveRefund(request) => &request.context,
            Self::ClassifyInitialization(request) => &request.context,
            Self::ClassifyCustody(request) => &request.context,
            Self::ClassifyFunding(request) => &request.context,
            Self::ClassifyClaim(request) => &request.context,
        }
    }

    fn terms(&self) -> &lez_bridge_protocol::WitnessedLezAssetTermsV2 {
        match self {
            Self::PrepareEscrow(request) => &request.terms,
            Self::ObserveEscrow(request) => &request.terms,
            Self::PrepareClaim(request) => &request.terms,
            Self::CompleteClaim(request) => &request.terms,
            Self::ObserveClaim(request) => &request.terms,
            Self::PrepareRefund(request) => &request.terms,
            Self::ObserveRefund(request) => &request.terms,
            Self::ClassifyInitialization(request) => &request.terms,
            Self::ClassifyCustody(request) => &request.terms,
            Self::ClassifyFunding(request) => &request.terms,
            Self::ClassifyClaim(request) => &request.terms,
        }
    }
}

#[derive(Clone, Default)]
struct RecordingTransport {
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    initialization_outcome: Option<InitializationOutcome>,
    asset_proof: Option<AssetProofMutation>,
}

#[derive(Clone, Copy, Debug)]
enum AssetProofMutation {
    None,
    TipDrift,
    IncompleteWindow,
    PlacementOutsideWindow,
    PlacementOrder,
    StepDrift,
    IdentityDrift,
    BytesDrift,
    NonPublic,
    SameHeightFork,
    CustodyDrift,
    AmountDrift,
    MetadataDrift,
    Missing,
    Unavailable,
}

#[derive(Clone, Copy, Debug)]
enum InitializationOutcome {
    Found,
    Absent,
    Uncertain,
    Unavailable,
}

impl InitializationOutcome {
    const ALL: [Self; 4] = [
        Self::Found,
        Self::Absent,
        Self::Uncertain,
        Self::Unavailable,
    ];

    const fn request_suffix(self) -> &'static str {
        match self {
            Self::Found => "found",
            Self::Absent => "absent",
            Self::Uncertain => "uncertain",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("test transport failure")]
struct TestTransportError;

#[async_trait]
impl LezBridgeAssetV2Transport for RecordingTransport {
    type Error = TestTransportError;

    async fn prepare_witnessed_asset_escrow_v2(
        &self,
        request: PrepareWitnessedAssetEscrowV2Request,
    ) -> Result<PrepareWitnessedAssetEscrowV2Result, Self::Error> {
        self.record(RecordedRequest::PrepareEscrow(Box::new(request)))
    }

    async fn observe_witnessed_asset_escrow_v2(
        &self,
        request: ObserveWitnessedAssetEscrowV2Request,
    ) -> Result<ObserveWitnessedAssetEscrowV2Result, Self::Error> {
        let Some(mutation) = self.asset_proof else {
            return self.record(RecordedRequest::ObserveEscrow(Box::new(request)));
        };
        self.requests
            .lock()
            .expect("request log")
            .push(RecordedRequest::ObserveEscrow(Box::new(request.clone())));
        asset_observation(&request, mutation)
    }

    async fn prepare_witnessed_asset_claim_v2(
        &self,
        request: PrepareWitnessedAssetClaimV2Request,
    ) -> Result<PrepareWitnessedAssetClaimV2Result, Self::Error> {
        self.record(RecordedRequest::PrepareClaim(Box::new(request)))
    }

    async fn complete_witnessed_asset_claim_v2(
        &self,
        request: CompleteWitnessedAssetClaimV2Request,
    ) -> Result<CompleteWitnessedAssetClaimV2Result, Self::Error> {
        self.record(RecordedRequest::CompleteClaim(Box::new(request)))
    }

    async fn observe_finalized_witnessed_asset_claim_v2(
        &self,
        request: ObserveFinalizedWitnessedAssetClaimV2Request,
    ) -> Result<ObserveFinalizedWitnessedAssetClaimV2Result, Self::Error> {
        self.record(RecordedRequest::ObserveClaim(Box::new(request)))
    }

    async fn prepare_witnessed_asset_refund_v2(
        &self,
        request: PrepareWitnessedAssetRefundV2Request,
    ) -> Result<PrepareWitnessedAssetRefundV2Result, Self::Error> {
        self.record(RecordedRequest::PrepareRefund(Box::new(request)))
    }

    async fn observe_witnessed_asset_refund_v2(
        &self,
        request: ObserveWitnessedAssetRefundV2Request,
    ) -> Result<ObserveWitnessedAssetRefundV2Result, Self::Error> {
        self.record(RecordedRequest::ObserveRefund(Box::new(request)))
    }

    async fn classify_finalized_witnessed_asset_initialization_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetInitializationV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetInitializationV2Result, Self::Error> {
        if let Some(outcome) = self.initialization_outcome {
            let response = initialization_result(&request, outcome);
            self.requests
                .lock()
                .expect("request log")
                .push(RecordedRequest::ClassifyInitialization(Box::new(request)));
            Ok(response)
        } else {
            self.record(RecordedRequest::ClassifyInitialization(Box::new(request)))
        }
    }

    async fn classify_finalized_witnessed_asset_custody_creation_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetCustodyCreationV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetCustodyCreationV2Result, Self::Error> {
        self.record(RecordedRequest::ClassifyCustody(Box::new(request)))
    }

    async fn classify_finalized_witnessed_asset_funding_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetFundingV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetFundingV2Result, Self::Error> {
        self.record(RecordedRequest::ClassifyFunding(Box::new(request)))
    }

    async fn classify_finalized_witnessed_asset_claim_v2(
        &self,
        request: ClassifyFinalizedWitnessedAssetClaimV2Request,
    ) -> Result<ClassifyFinalizedWitnessedAssetClaimV2Result, Self::Error> {
        self.record(RecordedRequest::ClassifyClaim(Box::new(request)))
    }
}

impl RecordingTransport {
    fn with_initialization_outcome(initialization_outcome: InitializationOutcome) -> Self {
        Self {
            initialization_outcome: Some(initialization_outcome),
            ..Self::default()
        }
    }

    fn with_asset_proof(asset_proof: AssetProofMutation) -> Self {
        Self {
            asset_proof: Some(asset_proof),
            ..Self::default()
        }
    }

    fn record<T>(&self, request: RecordedRequest) -> Result<T, TestTransportError> {
        self.requests.lock().expect("request log").push(request);
        Err(TestTransportError)
    }
}

fn initialization_result(
    request: &ClassifyFinalizedWitnessedAssetInitializationV2Request,
    outcome: InitializationOutcome,
) -> ClassifyFinalizedWitnessedAssetInitializationV2Result {
    let end_height =
        request.window.start_height() + u64::from(request.window.max_blocks().saturating_sub(1));
    let finalized_hash = Hex32::from_bytes([120; 32]);
    let finalized_clock = ChainClock::new(finalized_hash, end_height, 1_700_000_000_000);
    match outcome {
        InitializationOutcome::Found => {
            ClassifyFinalizedWitnessedAssetInitializationV2Result::found(
                request.context.clone(),
                request.terms.clone(),
                request.target.clone(),
                finalized_clock,
                request.window,
                initialization_facts(request, finalized_hash, end_height),
            )
            .expect("protocol-valid found classification")
        }
        InitializationOutcome::Absent => {
            ClassifyFinalizedWitnessedAssetInitializationV2Result::absent(
                request.context.clone(),
                request.terms.clone(),
                request.target.clone(),
                finalized_clock,
                request.window,
            )
            .expect("protocol-valid absent classification")
        }
        InitializationOutcome::Uncertain => {
            ClassifyFinalizedWitnessedAssetInitializationV2Result::uncertain(
                request.context.clone(),
                request.terms.clone(),
                request.target.clone(),
                finalized_clock,
                request.window,
            )
            .expect("protocol-valid uncertain classification")
        }
        InitializationOutcome::Unavailable => {
            ClassifyFinalizedWitnessedAssetInitializationV2Result::unavailable(
                request.context.clone(),
                request.terms.clone(),
                request.target.clone(),
                FinalizedWitnessedAssetUnavailableReasonV2::FinalityUnavailable,
            )
        }
    }
}

fn initialization_facts(
    request: &ClassifyFinalizedWitnessedAssetInitializationV2Request,
    finalized_hash: Hex32,
    end_height: u64,
) -> FinalizedWitnessedAssetInitializationFactsV2 {
    let WitnessedLezAssetV2::Native(terms) = request.terms.asset() else {
        panic!("native fixture");
    };
    let FinalizedWitnessedAssetTransactionTargetV2::Exact { transaction } = &request.target else {
        panic!("exact fixture target");
    };
    let metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        Hex32::from_bytes(METADATA_ACCOUNT),
        request.runtime.escrow_program_id,
        Hex32::from_bytes(CUSTODY_ACCOUNT),
        terms,
        EscrowState::Empty,
    );
    let observed = ObservedTransactionFacts::new(
        transaction.transaction_id,
        transaction.exact_bytes.clone(),
        ChainPosition::new(finalized_hash, end_height, 0),
        AccountIds::new(vec![terms.depositor_account_id()]).expect("signer accounts"),
        true,
    );
    let instruction = WitnessedAssetEffectInstructionFactsV2::new(
        WitnessedAssetPrepareStepV2::InitializeWitnessed,
        request.runtime.escrow_program_id,
        AccountIds::new(vec![
            metadata.account_id,
            metadata.custody_account_id,
            terms.depositor_account_id(),
            terms.claimant_account_id(),
            terms.aggregate_authority_account_id(),
        ])
        .expect("instruction accounts"),
        terms.swap_id(),
    );
    FinalizedWitnessedAssetInitializationFactsV2::new(
        observed,
        instruction,
        FinalizedBlockIdentity::new(end_height, finalized_hash, 1_700_000_000_000),
        metadata,
        WitnessedAssetInitializationCustodyFactsV2::native(NativeCustodyFacts::new(
            Hex32::from_bytes(CUSTODY_ACCOUNT),
            terms.authenticated_transfer_program_id(),
            0,
        )),
    )
}

fn token_effects() -> Vec<WitnessedAssetPreparedEffectV2> {
    vec![
        WitnessedAssetPreparedEffectV2::new(
            WitnessedAssetPrepareStepV2::InitializeWitnessed,
            tx(1),
        ),
        WitnessedAssetPreparedEffectV2::new(WitnessedAssetPrepareStepV2::CreateCustodyAta, tx(2)),
        WitnessedAssetPreparedEffectV2::new(WitnessedAssetPrepareStepV2::Fund, tx(3)),
    ]
}

fn proof_window() -> DiscoveryWindow {
    DiscoveryWindow::new(70, 4).expect("proof window")
}

fn asset_preparation(
    binding: &BtcLezAssetBridgeBindingV2,
    effects: Vec<WitnessedAssetPreparedEffectV2>,
) -> PrepareWitnessedAssetEscrowV2Result {
    PrepareWitnessedAssetEscrowV2Result::new(
        MessageContext::new(run_id(), rid("proof-preparation"), BridgeParticipant::Taker),
        binding.terms().clone(),
        effects,
    )
    .expect("valid proof preparation")
}

fn asset_observation(
    request: &ObserveWitnessedAssetEscrowV2Request,
    mutation: AssetProofMutation,
) -> Result<ObserveWitnessedAssetEscrowV2Result, TestTransportError> {
    if matches!(
        mutation,
        AssetProofMutation::Missing | AssetProofMutation::Unavailable
    ) {
        return Err(TestTransportError);
    }
    let mut result = baseline_asset_observation(request);
    mutate_asset_observation(&mut result, mutation);
    Ok(result)
}

fn baseline_asset_observation(
    request: &ObserveWitnessedAssetEscrowV2Request,
) -> ObserveWitnessedAssetEscrowV2Result {
    let (metadata, custody) = match request.terms.asset() {
        WitnessedLezAssetV2::Native(terms) => (
            WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
                Hex32::from_bytes(METADATA_ACCOUNT),
                request.runtime.escrow_program_id,
                Hex32::from_bytes(CUSTODY_ACCOUNT),
                terms,
                EscrowState::Funded,
            ),
            WitnessedAssetCustodyFactsV2::Native(NativeCustodyFacts::new(
                Hex32::from_bytes(CUSTODY_ACCOUNT),
                terms.authenticated_transfer_program_id(),
                terms.amount().as_u128(),
            )),
        ),
        WitnessedLezAssetV2::CustomToken(terms) => (
            WitnessedEscrowMetadataFacts::from_witnessed_token_terms(
                Hex32::from_bytes(METADATA_ACCOUNT),
                request.runtime.escrow_program_id,
                terms,
                EscrowState::Funded,
            ),
            WitnessedAssetCustodyFactsV2::CustomToken(TokenHoldingFactsV2::new(
                terms.custody_ata_account_id(),
                terms.token_program_id(),
                terms.token_definition_account_id(),
                terms.amount().as_u128(),
            )),
        ),
    };
    let effects = request
        .prepared_effects
        .iter()
        .enumerate()
        .map(|(index, effect)| {
            let height = 70 + u64::try_from(index).expect("bounded effect index");
            let block_byte = 100 + u8::try_from(index).expect("bounded effect index");
            WitnessedAssetObservedPrepareEffectV2::new(
                effect.step,
                ObservedTransactionFacts::new(
                    effect.transaction.transaction_id,
                    effect.transaction.exact_bytes.clone(),
                    ChainPosition::new(Hex32::from_bytes([block_byte; 32]), height, 0),
                    AccountIds::new(vec![]).expect("empty signer list is valid"),
                    true,
                ),
                request.runtime.escrow_program_id,
                prepare_accounts(request.terms.asset(), effect.step),
            )
        })
        .collect::<Vec<_>>();
    let tip = ChainTip::new(Hex32::from_bytes([120; 32]), 73);
    ObserveWitnessedAssetEscrowV2Result::new(
        request.context.clone(),
        request.terms.clone(),
        tip,
        effects,
        metadata,
        custody,
        tip,
    )
    .expect("valid baseline observation")
}

fn mutate_asset_observation(
    result: &mut ObserveWitnessedAssetEscrowV2Result,
    mutation: AssetProofMutation,
) {
    match mutation {
        AssetProofMutation::None => {}
        AssetProofMutation::TipDrift => {
            result.tip_after = ChainTip::new(Hex32::from_bytes([121; 32]), 74);
        }
        AssetProofMutation::IncompleteWindow => {
            let incomplete_tip = ChainTip::new(Hex32::from_bytes([119; 32]), 72);
            result.tip_before = incomplete_tip;
            result.tip_after = incomplete_tip;
        }
        AssetProofMutation::PlacementOutsideWindow => {
            result.effects[0].transaction.position =
                ChainPosition::new(Hex32::from_bytes([98; 32]), 69, 0);
        }
        AssetProofMutation::PlacementOrder => {
            result.effects[1].transaction.position =
                ChainPosition::new(Hex32::from_bytes([100; 32]), 70, 0);
        }
        AssetProofMutation::StepDrift => {
            result.effects[0].step = WitnessedAssetPrepareStepV2::Fund;
        }
        AssetProofMutation::IdentityDrift => {
            result.effects[0].transaction.transaction_id = TransactionId::from_bytes([77; 32]);
        }
        AssetProofMutation::BytesDrift => {
            result.effects[0].transaction.exact_bytes =
                ExactTransactionBytes::new(vec![77; 64]).expect("drift bytes");
        }
        AssetProofMutation::NonPublic => {
            result.effects[0].transaction.is_public = false;
        }
        AssetProofMutation::SameHeightFork => {
            result.effects[1].transaction.position =
                ChainPosition::new(Hex32::from_bytes([101; 32]), 70, 1);
        }
        AssetProofMutation::CustodyDrift => {
            let WitnessedAssetCustodyFactsV2::CustomToken(custody) = &mut result.custody else {
                unreachable!("token fixture")
            };
            custody.account_id = Hex32::from_bytes([77; 32]);
        }
        AssetProofMutation::AmountDrift => {
            let WitnessedAssetCustodyFactsV2::CustomToken(custody) = &mut result.custody else {
                unreachable!("token fixture")
            };
            custody.balance = NativeAmount::new(LEZ_AMOUNT + 1);
        }
        AssetProofMutation::MetadataDrift => {
            result.metadata.account_id = Hex32::from_bytes([78; 32]);
            for effect in &mut result.effects {
                let mut accounts: Vec<_> = effect.ordered_account_ids.clone().into();
                accounts[0] = result.metadata.account_id;
                effect.ordered_account_ids =
                    AccountIds::new(accounts).expect("drift account order remains bounded");
            }
        }
        AssetProofMutation::Missing | AssetProofMutation::Unavailable => unreachable!("returned"),
    }
}

fn prepare_accounts(asset: &WitnessedLezAssetV2, step: WitnessedAssetPrepareStepV2) -> AccountIds {
    let accounts = match (asset, step) {
        (WitnessedLezAssetV2::Native(terms), WitnessedAssetPrepareStepV2::InitializeWitnessed) => {
            vec![
                Hex32::from_bytes(METADATA_ACCOUNT),
                Hex32::from_bytes(CUSTODY_ACCOUNT),
                terms.depositor_account_id(),
                terms.claimant_account_id(),
                terms.aggregate_authority_account_id(),
            ]
        }
        (WitnessedLezAssetV2::Native(terms), WitnessedAssetPrepareStepV2::Fund) => vec![
            Hex32::from_bytes(METADATA_ACCOUNT),
            Hex32::from_bytes(CUSTODY_ACCOUNT),
            terms.depositor_account_id(),
        ],
        (
            WitnessedLezAssetV2::CustomToken(terms),
            WitnessedAssetPrepareStepV2::InitializeWitnessed,
        ) => vec![
            Hex32::from_bytes(METADATA_ACCOUNT),
            terms.depositor_owner_account_id(),
            terms.claimant_owner_account_id(),
            terms.token_definition_account_id(),
            terms.aggregate_authority_account_id(),
        ],
        (
            WitnessedLezAssetV2::CustomToken(terms),
            WitnessedAssetPrepareStepV2::CreateCustodyAta,
        ) => vec![
            Hex32::from_bytes(METADATA_ACCOUNT),
            terms.token_definition_account_id(),
            terms.custody_ata_account_id(),
        ],
        (WitnessedLezAssetV2::CustomToken(terms), WitnessedAssetPrepareStepV2::Fund) => vec![
            Hex32::from_bytes(METADATA_ACCOUNT),
            terms.depositor_owner_account_id(),
            terms.depositor_ata_account_id(),
            terms.custody_ata_account_id(),
        ],
        (WitnessedLezAssetV2::Native(_), WitnessedAssetPrepareStepV2::CreateCustodyAta) => {
            unreachable!("native plan has no custody ATA")
        }
    };
    AccountIds::new(accounts).expect("prepare account order")
}

fn tx(byte: u8) -> PreparedTransaction {
    PreparedTransaction::new(
        TransactionId::from_bytes([byte; 32]),
        ExactTransactionBytes::new(vec![byte; 64]).expect("transaction bytes"),
    )
}

fn claim() -> PreparedWitnessedClaim {
    PreparedWitnessedClaim::new(
        rid("claim-transcript"),
        Hex32::from_bytes([88; 32]),
        ExactMessageBytes::new(vec![89; 64]).expect("message bytes"),
    )
}

fn rid(value: &str) -> RequestId {
    RequestId::new(format!("btc-asset-v2-{value}")).expect("request id")
}

fn run_id() -> RunId {
    RunId::new("btc-asset-v2-run").expect("run id")
}

fn adapter(
    transport: RecordingTransport,
    role: Participant,
    runtime: RuntimeDescriptor,
) -> LezBridgeAdapter<RecordingTransport> {
    LezBridgeAdapter::new(transport, run_id(), runtime, role).expect("adapter")
}

fn runtime(role: Participant) -> RuntimeDescriptor {
    runtime_with(
        role,
        RuntimeCompatibility::LeeV0_2_0,
        match role {
            Participant::Maker => MAKER_ACCOUNT,
            Participant::Taker => TAKER_ACCOUNT,
        },
    )
}

fn runtime_with(
    role: Participant,
    compatibility: RuntimeCompatibility,
    signer: [u8; 32],
) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        bridge_participant(role),
        compatibility,
        Hex32::from_bytes([9; 32]),
        Hex32::from_bytes(LEZ_CHANNEL),
        Hex32::from_bytes(LEZ_GENESIS),
        Hex32::from_bytes(ESCROW_PROGRAM),
        Hex32::from_bytes(signer),
    )
}

fn token_asset(agreement: &BtcAgreementV1, base: u8) -> BtcLezAssetV1 {
    BtcLezAssetV1::CustomToken(Box::new(BtcLezCustomTokenTermsV1::new(
        [base; 32],
        [base + 1; 32],
        [base + 2; 32],
        *agreement.lez_terms().depositor_account(),
        [base + 3; 32],
        *agreement.lez_terms().claimant_account(),
        [base + 4; 32],
        [base + 5; 32],
        agreement.lez_terms().amount(),
        agreement.lez_terms().refund_at_ms(),
        *agreement.lez_terms().aggregate_authority_account(),
        agreement.p2tr_contract().aggregate_internal_key_bytes(),
    )))
}

fn extension(agreement: &BtcAgreementV1, asset: BtcLezAssetV1) -> BtcLezAssetExtensionV1 {
    let body = BtcLezAssetExtensionBodyV1::new(*agreement.agreement_commitment(), asset);
    let commitment = body.commitment();
    BtcLezAssetExtensionV1::validate(
        BtcLezAssetExtensionRecordV1::from_parts(
            BTC_LEZ_ASSET_EXTENSION_SCHEMA_V1,
            body,
            commitment,
            sign(&secret(1), commitment),
            sign(&secret(2), commitment),
        ),
        agreement,
    )
    .expect("valid extension")
}

fn secret(value: u8) -> SecretKey {
    SecretKey::from_slice(&[value; 32]).expect("fixed secret")
}

fn public_key(secret: &SecretKey) -> [u8; 33] {
    PublicKey::from_secret_key(&Secp256k1::new(), secret).serialize()
}

fn x_only(secret: &SecretKey) -> [u8; 32] {
    Keypair::from_secret_key(&Secp256k1::new(), secret)
        .x_only_public_key()
        .0
        .serialize()
}

fn destination(secret: &SecretKey) -> Vec<u8> {
    ScriptBuf::new_p2tr(
        &Secp256k1::verification_only(),
        Keypair::from_secret_key(&Secp256k1::new(), secret)
            .x_only_public_key()
            .0,
        None,
    )
    .into_bytes()
}

fn sign(secret: &SecretKey, commitment: [u8; 32]) -> [u8; 64] {
    Secp256k1::new()
        .sign_schnorr_no_aux_rand(
            &Message::from_digest(commitment),
            &Keypair::from_secret_key(&Secp256k1::new(), secret),
        )
        .serialize()
}

#[allow(clippy::too_many_lines)]
fn agreement(direction: SwapDirection) -> BtcAgreementV1 {
    let maker = secret(1);
    let taker = secret(2);
    let participants = BtcParticipantsV1::new(
        BtcParticipantIdentityV1::new(
            MAKER_ACCOUNT,
            public_key(&maker),
            x_only(&secret(3)),
            destination(&secret(5)),
        ),
        BtcParticipantIdentityV1::new(
            TAKER_ACCOUNT,
            public_key(&taker),
            x_only(&secret(4)),
            destination(&secret(6)),
        ),
    );
    let adaptor_point = public_key(&secret(7));
    let aggregate = AdaptorSessionContext::untweaked(
        [public_key(&maker), public_key(&taker)],
        [30; 32],
        adaptor_point,
        [31; 32],
    )
    .expect("aggregate context")
    .output_key();
    let bitcoin_funder = match direction {
        SwapDirection::TakerSellsForeign => Participant::Taker,
        SwapDirection::TakerSellsLez => Participant::Maker,
    };
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(aggregate).expect("aggregate key"),
        RefundXOnlyKey::from_bytes(
            *participants
                .for_participant(bitcoin_funder)
                .bitcoin_refund_key(),
        )
        .expect("refund key"),
        CsvBlockDelay::new(144).expect("CSV"),
    )
    .expect("contract");
    let funding = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::from_byte_array([42; 32]),
                vout: 0,
            },
            script_sig: ScriptBuf::from_bytes(vec![0x51]),
            sequence: Sequence::MAX,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(100_000),
            script_pubkey: ScriptBuf::from_bytes(contract.script_pubkey_bytes().to_vec()),
        }],
    };
    let claim = CooperativeKeyPathSpend::new(
        &contract,
        OutPoint {
            txid: funding.compute_txid(),
            vout: 0,
        },
        Amount::from_sat(100_000),
        vec![TxOut {
            value: Amount::from_sat(99_000),
            script_pubkey: ScriptBuf::from_bytes(
                participants
                    .for_participant(bitcoin_funder.other())
                    .claim_destination_script_pubkey()
                    .to_vec(),
            ),
        }],
    )
    .expect("claim");
    let lez_depositor = match direction {
        SwapDirection::TakerSellsForeign => Participant::Maker,
        SwapDirection::TakerSellsLez => Participant::Taker,
    };
    let refund_at_ms = match direction {
        SwapDirection::TakerSellsForeign => 1_700_000_100_000,
        SwapDirection::TakerSellsLez => 1_700_000_500_000,
    };
    let body = BtcAgreementBodyV1::new(
        [20; 32],
        direction,
        BtcChainPolicyV1::new([8; 32], 6),
        participants.clone(),
        adaptor_point,
        BtcLezTermsV1::new(
            LEZ_CHANNEL,
            LEZ_GENESIS,
            ESCROW_PROGRAM,
            TRANSFER_PROGRAM,
            [12; 32],
            METADATA_ACCOUNT,
            CUSTODY_ACCOUNT,
            *participants
                .for_participant(lez_depositor)
                .lez_owner_account(),
            *participants
                .for_participant(lez_depositor.other())
                .lez_owner_account(),
            LEZ_AMOUNT,
            refund_at_ms,
            [19; 32],
        ),
        BtcP2trTermsV1::from_contract(&contract),
        BtcFundingTermsV1::new(funding.compute_txid().to_byte_array(), 0, 100_000),
        BtcClaimTermsV1::from_spend(&claim).expect("claim terms"),
        BtcRecoveryPlanV1::new(
            1_000,
            1_144,
            1_699_999_800,
            1_700_000_100,
            1_700_000_500,
            300,
        ),
    );
    let commitment = body.commitment();
    BtcAgreementV1::validate(BtcAgreementRecordV1::from_parts(
        BTC_AGREEMENT_SCHEMA_V1,
        body,
        commitment,
        sign(&maker, commitment),
        sign(&taker, commitment),
    ))
    .expect("agreement")
}

const fn bridge_participant(participant: Participant) -> BridgeParticipant {
    match participant {
        Participant::Maker => BridgeParticipant::Maker,
        Participant::Taker => BridgeParticipant::Taker,
    }
}
