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
    CurrentLezFirstLockError, LezBridgeAdapter, LezBridgeBtcFirstLockProofTransport,
    LezBridgeCurrentEscrowTransport,
};
use lez_bridge_client::FinalizedWitnessedFundingPresence;
use lez_bridge_protocol::{
    AccountIds, ChainClock, ChainPosition, ChainTip, DiscoveryWindow, EscrowObservationTarget,
    EscrowState, ExactTransactionBytes, FinalizedBlockIdentity, FinalizedWitnessedFundingFacts,
    Hex32, NativeCustodyFacts, NativeEscrowAccountFacts, NativeEscrowAccountObservation,
    NativeFundInstructionFacts, NativeRefundObservation, NativeRefundObservationTarget,
    ObserveFinalizedWitnessedFundingRequest, ObserveNativeRefundRequest, ObserveNativeRefundResult,
    ObserveWitnessedEscrowRequest, ObserveWitnessedEscrowResult, ObservedTransactionFacts,
    Participant as BridgeParticipant, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
    TransactionId, WitnessedEscrowMetadataFacts, WitnessedFundingFoundFacts,
    WitnessedFundingObservation, WitnessedInitializationFoundFacts,
    WitnessedInitializationObservation, WitnessedNativeInitializeInstructionFacts,
};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, BTC_AGREEMENT_SCHEMA_V1, BtcAgreementBodyV1, BtcAgreementRecordV1,
    BtcAgreementV1, BtcChainPolicyV1, BtcClaimTermsV1, BtcFundingTermsV1, BtcLezTermsV1,
    BtcP2trTermsV1, BtcParticipantIdentityV1, BtcParticipantsV1, BtcRecoveryPlanV1,
    CooperativeKeyPathSpend, CsvBlockDelay, P2trSwapOutput, RefundXOnlyKey, TwoPartyAggregateKey,
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

#[derive(Clone, Copy, Debug)]
enum Mutation {
    None,
    ClockDrift,
    Claimed,
    TermsHash,
    MetadataAccount,
    CustodyAccount,
    CustodyOwner,
    CustodyValue,
    AccountsAbsent,
    RefundLookedUp,
}

#[derive(Clone, Debug)]
struct ReadOnlyTransport {
    mutation: Mutation,
    requests: Arc<Mutex<Vec<ObserveNativeRefundRequest>>>,
}

impl ReadOnlyTransport {
    fn new(mutation: Mutation) -> Self {
        Self {
            mutation,
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("test transport error")]
struct TestTransportError;

#[async_trait]
impl LezBridgeCurrentEscrowTransport for ReadOnlyTransport {
    type Error = TestTransportError;

    async fn observe_native_refund(
        &self,
        request: ObserveNativeRefundRequest,
    ) -> Result<ObserveNativeRefundResult, Self::Error> {
        self.requests
            .lock()
            .expect("request log")
            .push(request.clone());
        let terms = request.terms.witnessed().expect("witnessed terms");
        let state = if matches!(self.mutation, Mutation::Claimed) {
            EscrowState::Claimed
        } else {
            EscrowState::Funded
        };
        let metadata_account = if matches!(self.mutation, Mutation::MetadataAccount) {
            Hex32::from_bytes([0xa1; 32])
        } else {
            Hex32::from_bytes(METADATA_ACCOUNT)
        };
        let custody_account = if matches!(self.mutation, Mutation::CustodyAccount) {
            Hex32::from_bytes([0xa2; 32])
        } else {
            Hex32::from_bytes(CUSTODY_ACCOUNT)
        };
        let custody_owner = if matches!(self.mutation, Mutation::CustodyOwner) {
            Hex32::from_bytes([0xa3; 32])
        } else {
            terms.authenticated_transfer_program_id()
        };
        let custody_value = if matches!(self.mutation, Mutation::CustodyValue) {
            LEZ_AMOUNT - 1
        } else if state == EscrowState::Funded {
            LEZ_AMOUNT
        } else {
            0
        };
        let accounts = if matches!(self.mutation, Mutation::AccountsAbsent) {
            NativeEscrowAccountObservation::Absent
        } else {
            let mut metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
                metadata_account,
                request.runtime.escrow_program_id,
                custody_account,
                terms,
                state,
            );
            if matches!(self.mutation, Mutation::TermsHash) {
                metadata.terms_hash = Hex32::from_bytes([0xa4; 32]);
            }
            NativeEscrowAccountObservation::found(NativeEscrowAccountFacts::new_witnessed(
                metadata,
                NativeCustodyFacts::new(custody_account, custody_owner, custody_value),
            ))
        };
        let before = ChainClock::new(Hex32::from_bytes([0x90; 32]), 12, 200_000);
        let after = if matches!(self.mutation, Mutation::ClockDrift) {
            ChainClock::new(Hex32::from_bytes([0x91; 32]), 13, 201_000)
        } else {
            before
        };
        let refund = if matches!(self.mutation, Mutation::RefundLookedUp) {
            NativeRefundObservation::Absent
        } else {
            NativeRefundObservation::NotRequested
        };
        Ok(ObserveNativeRefundResult::new(
            request.context,
            before,
            accounts,
            refund,
            after,
        ))
    }
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
    let lez_refund_at_ms = match direction {
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
            lez_refund_at_ms,
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

fn runtime(role: Participant) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        match role {
            Participant::Maker => BridgeParticipant::Maker,
            Participant::Taker => BridgeParticipant::Taker,
        },
        RuntimeCompatibility::LeeV0_2_0,
        Hex32::from_bytes([9; 32]),
        Hex32::from_bytes(LEZ_CHANNEL),
        Hex32::from_bytes(LEZ_GENESIS),
        Hex32::from_bytes(ESCROW_PROGRAM),
        Hex32::from_bytes(match role {
            Participant::Maker => MAKER_ACCOUNT,
            Participant::Taker => TAKER_ACCOUNT,
        }),
    )
}

fn adapter(transport: ReadOnlyTransport, role: Participant) -> LezBridgeAdapter<ReadOnlyTransport> {
    LezBridgeAdapter::new(
        transport,
        RunId::new("btc-current-first-lock-run").expect("run ID"),
        runtime(role),
        role,
    )
    .expect("adapter")
}

#[tokio::test]
async fn generic_funded_escrow_reads_the_agreement_selected_lez_depositor_in_both_directions() {
    for (direction, expected_depositor, expected_claimant) in [
        (
            SwapDirection::TakerSellsForeign,
            BridgeParticipant::Maker,
            BridgeParticipant::Taker,
        ),
        (
            SwapDirection::TakerSellsLez,
            BridgeParticipant::Taker,
            BridgeParticipant::Maker,
        ),
    ] {
        let agreement = agreement(direction);
        for role in [Participant::Maker, Participant::Taker] {
            let transport = ReadOnlyTransport::new(Mutation::None);
            let adapter = adapter(transport.clone(), role);
            let evidence = adapter
                .observe_current_lez_funded_escrow(
                    &agreement,
                    RequestId::new(format!("current-funded-{direction:?}-{role:?}").to_lowercase())
                        .expect("request ID"),
                )
                .await
                .expect("current agreement-selected funded escrow");

            assert_eq!(evidence.schema_version(), 1);
            assert_eq!(evidence.clock().height, 12);
            assert_eq!(evidence.clock().timestamp_ms, 200_000);
            assert_eq!(evidence.metadata().status, EscrowState::Funded);
            assert_eq!(evidence.metadata().account_id.as_bytes(), &METADATA_ACCOUNT);
            assert_eq!(evidence.custody().account_id.as_bytes(), &CUSTODY_ACCOUNT);
            assert_eq!(evidence.custody().balance.as_u128(), LEZ_AMOUNT);

            let requests = transport.requests.lock().expect("request log");
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].context.sidecar_role, runtime(role).sidecar_role);
            assert_eq!(requests[0].terms.depositor(), expected_depositor);
            assert_eq!(requests[0].terms.claimant(), expected_claimant);
            assert_eq!(
                requests[0].terms.depositor_account_id().as_bytes(),
                agreement.lez_terms().depositor_account()
            );
            assert_eq!(
                requests[0].terms.claimant_account_id().as_bytes(),
                agreement.lez_terms().claimant_account()
            );
            assert_eq!(requests[0].target, NativeRefundObservationTarget::StateOnly);
        }
    }
}

#[tokio::test]
async fn both_roles_derive_the_reverse_first_lock_and_read_state_only_once() {
    let agreement = agreement(SwapDirection::TakerSellsLez);
    for role in [Participant::Maker, Participant::Taker] {
        let transport = ReadOnlyTransport::new(Mutation::None);
        let adapter = adapter(transport.clone(), role);
        let evidence = adapter
            .observe_current_lez_first_lock(
                &agreement,
                RequestId::new(format!("current-first-lock-{role:?}").to_lowercase())
                    .expect("request ID"),
            )
            .await
            .expect("current funded first lock");
        assert_eq!(evidence.clock().height, 12);
        assert_eq!(evidence.clock().timestamp_ms, 200_000);
        assert_eq!(evidence.metadata().account_id.as_bytes(), &METADATA_ACCOUNT);
        assert_eq!(evidence.custody().account_id.as_bytes(), &CUSTODY_ACCOUNT);
        assert_eq!(evidence.custody().balance.as_u128(), LEZ_AMOUNT);

        let requests = transport.requests.lock().expect("request log");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].context.sidecar_role, runtime(role).sidecar_role);
        assert_eq!(requests[0].terms.depositor(), BridgeParticipant::Taker);
        assert_eq!(requests[0].terms.claimant(), BridgeParticipant::Maker);
        assert_eq!(requests[0].target, NativeRefundObservationTarget::StateOnly);
    }
}

#[tokio::test]
async fn forward_direction_is_not_misrepresented_as_a_lez_first_lock() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    for role in [Participant::Maker, Participant::Taker] {
        let transport = ReadOnlyTransport::new(Mutation::None);
        let error = adapter(transport.clone(), role)
            .observe_current_lez_first_lock(
                &agreement,
                RequestId::new(format!("wrong-direction-{role:?}").to_lowercase())
                    .expect("request ID"),
            )
            .await
            .expect_err("Bitcoin is the first lock in this direction");
        assert!(matches!(error, CurrentLezFirstLockError::WrongDirection));
        assert!(transport.requests.lock().expect("request log").is_empty());
    }
}

#[tokio::test]
async fn generic_funded_escrow_rejects_runtime_and_role_account_drift_before_transport() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    for (label, selected_runtime, expected) in [
        (
            "compatibility",
            RuntimeDescriptor {
                compatibility: RuntimeCompatibility::NssaV0_1_2,
                ..runtime(Participant::Maker)
            },
            "incompatible",
        ),
        (
            "channel",
            RuntimeDescriptor {
                channel_id: Hex32::from_bytes([0xb1; 32]),
                ..runtime(Participant::Maker)
            },
            "chain identity",
        ),
        (
            "genesis",
            RuntimeDescriptor {
                genesis_block_hash: Hex32::from_bytes([0xb2; 32]),
                ..runtime(Participant::Maker)
            },
            "chain identity",
        ),
        (
            "program",
            RuntimeDescriptor {
                escrow_program_id: Hex32::from_bytes([0xb3; 32]),
                ..runtime(Participant::Maker)
            },
            "program",
        ),
        (
            "role-account",
            RuntimeDescriptor {
                signer_account_id: Hex32::from_bytes(TAKER_ACCOUNT),
                ..runtime(Participant::Maker)
            },
            "local role",
        ),
    ] {
        let transport = ReadOnlyTransport::new(Mutation::None);
        let adapter = LezBridgeAdapter::new(
            transport.clone(),
            RunId::new("btc-current-runtime-drift").expect("run ID"),
            selected_runtime,
            Participant::Maker,
        )
        .expect("runtime sidecar role remains maker");
        let error = adapter
            .observe_current_lez_funded_escrow(
                &agreement,
                RequestId::new(format!("runtime-{label}")).expect("request ID"),
            )
            .await
            .expect_err("runtime drift must fail before transport");
        assert!(error.to_string().contains(expected), "{label}: {error}");
        assert!(transport.requests.lock().expect("request log").is_empty());
    }
}

#[tokio::test]
async fn drift_spent_wrong_accounts_and_value_fail_closed() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let agreement = agreement(direction);
        for (mutation, expected) in [
            (Mutation::ClockDrift, "clock"),
            (Mutation::Claimed, "funded"),
            (Mutation::TermsHash, "account"),
            (Mutation::MetadataAccount, "account"),
            (Mutation::CustodyAccount, "account"),
            (Mutation::CustodyOwner, "account"),
            (Mutation::CustodyValue, "value"),
            (Mutation::AccountsAbsent, "unavailable"),
            (Mutation::RefundLookedUp, "state-only"),
        ] {
            let transport = ReadOnlyTransport::new(mutation);
            let error = adapter(transport.clone(), Participant::Maker)
                .observe_current_lez_funded_escrow(
                    &agreement,
                    RequestId::new(format!("mutation-{direction:?}-{mutation:?}").to_lowercase())
                        .expect("request ID"),
                )
                .await
                .expect_err("mutation must fail closed");
            assert!(
                error.to_string().contains(expected),
                "{direction:?}/{mutation:?}: {error}"
            );
            assert_eq!(transport.requests.lock().expect("request log").len(), 1);
        }
    }
}

const INITIALIZATION_ID: [u8; 32] = [0x41; 32];
const FUNDING_ID: [u8; 32] = [0x42; 32];
const INITIALIZATION_BYTES: &[u8] = b"exact-witnessed-initialization";
const FUNDING_BYTES: &[u8] = b"exact-witnessed-funding";

#[derive(Clone, Copy, Debug)]
enum ProofMutation {
    None,
    FinalizedAbsent,
    FinalizedInstruction,
    FinalizedPosition,
    CurrentSigner,
    CurrentAccounts,
    CurrentTipDrift,
    MissingInitialization,
    ReversedPair,
    CrossBoundFunding,
    WrongCurrentMetadata,
}

#[derive(Clone, Debug)]
struct BtcFirstLockProofTransport {
    mutation: ProofMutation,
    order: Arc<Mutex<Vec<&'static str>>>,
    finalized_requests: Arc<Mutex<Vec<ObserveFinalizedWitnessedFundingRequest>>>,
    current_requests: Arc<Mutex<Vec<ObserveWitnessedEscrowRequest>>>,
}

impl BtcFirstLockProofTransport {
    fn new(mutation: ProofMutation) -> Self {
        Self {
            mutation,
            order: Arc::new(Mutex::new(Vec::new())),
            finalized_requests: Arc::new(Mutex::new(Vec::new())),
            current_requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

fn proof_transaction(
    transaction_id: [u8; 32],
    exact_bytes: &[u8],
    block_hash: [u8; 32],
    height: u64,
    transaction_index: u32,
) -> ObservedTransactionFacts {
    ObservedTransactionFacts::new(
        TransactionId::from_bytes(transaction_id),
        ExactTransactionBytes::new(exact_bytes.to_vec()).expect("exact transaction bytes"),
        ChainPosition::new(Hex32::from_bytes(block_hash), height, transaction_index),
        AccountIds::new(vec![Hex32::from_bytes(TAKER_ACCOUNT)]).expect("one signer"),
        true,
    )
}

fn proof_metadata(
    request: &ObserveFinalizedWitnessedFundingRequest,
    status: EscrowState,
) -> WitnessedEscrowMetadataFacts {
    WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        Hex32::from_bytes(METADATA_ACCOUNT),
        request.runtime.escrow_program_id,
        Hex32::from_bytes(CUSTODY_ACCOUNT),
        &request.terms,
        status,
    )
}

fn proof_custody(
    request: &ObserveFinalizedWitnessedFundingRequest,
    balance: u128,
) -> NativeCustodyFacts {
    NativeCustodyFacts::new(
        Hex32::from_bytes(CUSTODY_ACCOUNT),
        request.terms.authenticated_transfer_program_id(),
        balance,
    )
}

fn proof_funding_instruction(
    request: &ObserveFinalizedWitnessedFundingRequest,
) -> NativeFundInstructionFacts {
    NativeFundInstructionFacts::new(
        request.runtime.escrow_program_id,
        AccountIds::new(vec![
            Hex32::from_bytes(METADATA_ACCOUNT),
            Hex32::from_bytes(CUSTODY_ACCOUNT),
            request.terms.depositor_account_id(),
        ])
        .expect("fund accounts"),
        request.terms.swap_id(),
    )
}

#[async_trait]
impl LezBridgeBtcFirstLockProofTransport for BtcFirstLockProofTransport {
    type Error = TestTransportError;

    async fn classify_finalized_witnessed_funding(
        &self,
        request: ObserveFinalizedWitnessedFundingRequest,
    ) -> Result<FinalizedWitnessedFundingPresence, Self::Error> {
        self.order.lock().expect("order log").push("finalized");
        self.finalized_requests
            .lock()
            .expect("finalized requests")
            .push(request.clone());
        let finalized_clock = ChainClock::new(Hex32::from_bytes([0x70; 32]), 20, 1_700_000_200_000);
        if matches!(self.mutation, ProofMutation::FinalizedAbsent) {
            return Ok(FinalizedWitnessedFundingPresence::Absent {
                context: request.context,
                finalized_clock,
                scanned_window: request.window,
            });
        }
        let mut instruction = proof_funding_instruction(&request);
        if matches!(self.mutation, ProofMutation::FinalizedInstruction) {
            instruction.program_id = Hex32::from_bytes([0xa5; 32]);
        }
        let block_id = if matches!(self.mutation, ProofMutation::FinalizedPosition) {
            7
        } else {
            6
        };
        let funding = FinalizedWitnessedFundingFacts::new(
            proof_transaction(FUNDING_ID, FUNDING_BYTES, [0x62; 32], 6, 0),
            instruction,
            FinalizedBlockIdentity::new(block_id, Hex32::from_bytes([0x62; 32]), 1_700_000_006_000),
            proof_metadata(&request, EscrowState::Funded),
            proof_custody(&request, LEZ_AMOUNT),
        );
        Ok(FinalizedWitnessedFundingPresence::Found {
            context: request.context,
            finalized_clock,
            scanned_window: request.window,
            funding: Box::new(funding),
        })
    }

    async fn observe_witnessed_escrow(
        &self,
        request: ObserveWitnessedEscrowRequest,
    ) -> Result<ObserveWitnessedEscrowResult, Self::Error> {
        self.order.lock().expect("order log").push("current");
        self.current_requests
            .lock()
            .expect("current requests")
            .push(request.clone());
        let finalized_shape = ObserveFinalizedWitnessedFundingRequest::discover_by_terms(
            request.context.clone(),
            request.runtime.clone(),
            request.terms.clone(),
            match request.target {
                EscrowObservationTarget::DiscoverByTerms { window } => window,
                EscrowObservationTarget::Exact { .. } => panic!("expected discovery"),
            },
        );
        let init_position = if matches!(self.mutation, ProofMutation::ReversedPair) {
            (6, 1)
        } else {
            (5, 1)
        };
        let initialization = if matches!(self.mutation, ProofMutation::MissingInitialization) {
            WitnessedInitializationObservation::Absent
        } else {
            WitnessedInitializationObservation::found(WitnessedInitializationFoundFacts::new(
                proof_transaction(
                    INITIALIZATION_ID,
                    INITIALIZATION_BYTES,
                    [0x61; 32],
                    init_position.0,
                    init_position.1,
                ),
                WitnessedNativeInitializeInstructionFacts::new(
                    request.runtime.escrow_program_id,
                    AccountIds::new(vec![
                        Hex32::from_bytes(METADATA_ACCOUNT),
                        Hex32::from_bytes(CUSTODY_ACCOUNT),
                        request.terms.depositor_account_id(),
                        request.terms.claimant_account_id(),
                        request.terms.aggregate_authority_account_id(),
                    ])
                    .expect("initialize accounts"),
                    request.terms.clone(),
                ),
                proof_metadata(&finalized_shape, EscrowState::Funded),
            ))
        };
        let current_funding_bytes = if matches!(self.mutation, ProofMutation::CrossBoundFunding) {
            b"substituted-current-funding".as_slice()
        } else {
            FUNDING_BYTES
        };
        let current_status = if matches!(self.mutation, ProofMutation::WrongCurrentMetadata) {
            EscrowState::Claimed
        } else {
            EscrowState::Funded
        };
        let funding = WitnessedFundingObservation::found(WitnessedFundingFoundFacts::new(
            proof_transaction(FUNDING_ID, current_funding_bytes, [0x62; 32], 6, 0),
            proof_funding_instruction(&finalized_shape),
            proof_metadata(&finalized_shape, current_status),
            proof_custody(
                &finalized_shape,
                if current_status == EscrowState::Funded {
                    LEZ_AMOUNT
                } else {
                    0
                },
            ),
        ));
        let tip_before = ChainTip::new(Hex32::from_bytes([0x71; 32]), 21);
        let tip_after = if matches!(self.mutation, ProofMutation::CurrentTipDrift) {
            ChainTip::new(Hex32::from_bytes([0x72; 32]), 22)
        } else {
            tip_before
        };
        Ok(ObserveWitnessedEscrowResult::new(
            request.context,
            tip_before,
            initialization,
            funding,
            tip_after,
        ))
    }
}

fn proof_window() -> DiscoveryWindow {
    DiscoveryWindow::new(1, 20).expect("proof window")
}

fn proof_adapter(
    transport: BtcFirstLockProofTransport,
    role: Participant,
) -> LezBridgeAdapter<BtcFirstLockProofTransport> {
    LezBridgeAdapter::new(
        transport,
        RunId::new("btc-finalized-current-first-lock").expect("run ID"),
        runtime(role),
        role,
    )
    .expect("proof adapter")
}

#[tokio::test]
async fn maker_proves_finalized_and_current_lez_first_lock_in_that_order() {
    let agreement = agreement(SwapDirection::TakerSellsLez);
    let transport = BtcFirstLockProofTransport::new(ProofMutation::None);
    let proof = proof_adapter(transport.clone(), Participant::Maker)
        .prove_btc_lez_first_lock(
            &agreement,
            RequestId::new("btc-first-lock-finalized").expect("request ID"),
            RequestId::new("btc-first-lock-current").expect("request ID"),
            proof_window(),
        )
        .await
        .expect("finalized and current first-lock proof");

    assert_eq!(
        transport.order.lock().expect("order log").as_slice(),
        ["finalized", "current"]
    );
    assert_eq!(proof.finalized_clock().height, 20);
    assert_eq!(proof.current_tip().height, 21);
    assert_eq!(
        proof.prepared().plan().steps()[0]
            .expected_public_id()
            .as_str(),
        "41".repeat(32)
    );
    assert_eq!(
        proof.prepared().plan().steps()[1]
            .expected_public_id()
            .as_str(),
        "42".repeat(32)
    );
    assert_eq!(
        proof.evidence().exact_initialization().as_slice(),
        INITIALIZATION_BYTES
    );
    assert_eq!(proof.evidence().exact_funding().as_slice(), FUNDING_BYTES);

    let finalized = transport
        .finalized_requests
        .lock()
        .expect("finalized requests");
    let current = transport.current_requests.lock().expect("current requests");
    assert_eq!(finalized.len(), 1);
    assert_eq!(current.len(), 1);
    assert_eq!(finalized[0].window, proof_window());
    assert_eq!(finalized[0].context.sidecar_role, BridgeParticipant::Maker);
    assert_eq!(current[0].context.sidecar_role, BridgeParticipant::Maker);
    assert!(matches!(
        current[0].target,
        EscrowObservationTarget::DiscoverByTerms { window } if window == proof_window()
    ));
}

#[tokio::test]
async fn proof_rejects_wrong_direction_and_non_claimant_before_transport() {
    for (direction, role, expected) in [
        (
            SwapDirection::TakerSellsForeign,
            Participant::Maker,
            "does not select",
        ),
        (
            SwapDirection::TakerSellsLez,
            Participant::Taker,
            "not the LEZ",
        ),
    ] {
        let transport = BtcFirstLockProofTransport::new(ProofMutation::None);
        let error = proof_adapter(transport.clone(), role)
            .prove_btc_lez_first_lock(
                &agreement(direction),
                RequestId::new(format!("proof-role-finalized-{role:?}").to_lowercase())
                    .expect("request ID"),
                RequestId::new(format!("proof-role-current-{role:?}").to_lowercase())
                    .expect("request ID"),
                proof_window(),
            )
            .await
            .expect_err("direction or role must fail closed");
        assert!(error.to_string().contains(expected), "{error}");
        assert!(transport.order.lock().expect("order log").is_empty());
    }

    let transport = BtcFirstLockProofTransport::new(ProofMutation::None);
    let request_id = RequestId::new("duplicate-proof-read").expect("request ID");
    let error = proof_adapter(transport.clone(), Participant::Maker)
        .prove_btc_lez_first_lock(
            &agreement(SwapDirection::TakerSellsLez),
            request_id.clone(),
            request_id,
            proof_window(),
        )
        .await
        .expect_err("duplicate operation identities must fail before transport");
    assert!(error.to_string().contains("must be distinct"), "{error}");
    assert!(transport.order.lock().expect("order log").is_empty());
}

#[tokio::test]
async fn proof_fails_closed_on_finality_current_state_pair_and_cross_binding_drift() {
    for (mutation, expected) in [
        (
            ProofMutation::FinalizedAbsent,
            "finalized funding is unavailable",
        ),
        (ProofMutation::CurrentTipDrift, "current tip changed"),
        (
            ProofMutation::MissingInitialization,
            "complete current pair",
        ),
        (ProofMutation::ReversedPair, "chronological"),
        (
            ProofMutation::CrossBoundFunding,
            "finalized funding differs",
        ),
        (
            ProofMutation::WrongCurrentMetadata,
            "current funded escrow differs",
        ),
    ] {
        let transport = BtcFirstLockProofTransport::new(mutation);
        let error = proof_adapter(transport.clone(), Participant::Maker)
            .prove_btc_lez_first_lock(
                &agreement(SwapDirection::TakerSellsLez),
                RequestId::new(format!("proof-finalized-{mutation:?}").to_lowercase())
                    .expect("request ID"),
                RequestId::new(format!("proof-current-{mutation:?}").to_lowercase())
                    .expect("request ID"),
                proof_window(),
            )
            .await
            .expect_err("proof drift must fail closed");
        assert!(
            error.to_string().contains(expected),
            "{mutation:?}: {error}"
        );
    }
}
