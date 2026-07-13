use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use lez_bridge_adapter::{LezBridgeAdapter, LezBridgeTransport, PrepareNativeFirstLockError};
use lez_bridge_protocol::{
    ExactTransactionBytes, Hex32, Participant as BridgeParticipant, PrepareNativeEscrowRequest,
    PrepareNativeEscrowResult, PreparedTransaction, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor, TransactionId,
};
use lez_swap_core::{Participant, SwapDirection, UnixSeconds};
use lez_zec_swap_sdk::{
    Bip199Contract, ExpectedBip199Output, FirstLockPlanV1, FirstLockStepV1, LezAssetV1,
    LezChainIdentityV1, LezEnvironmentV1, NegotiationTranscriptV1,
    ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashTransparentDestinationV1, ZecAgreementBodyV1,
    ZecAgreementRecordV1, ZecAgreementV1, ZecLezTermsV1, ZecParticipantIdentityV1,
    ZecParticipantsV1, ZecProfileId, ZecProfileRecordV1, ZecRefundPlanV1, ZecSwapBinding,
    ZecSwapBindingRecordV1, ZecTransactionPolicyV1, derive_lez_metadata_account_v1,
    derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
    derive_nssa_v0_1_2_metadata_account_v1, derive_nssa_v0_1_2_native_custody_account_v1,
    derive_nssa_v0_1_2_token_account_v1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::address::TransparentAddress;

#[derive(Clone, Debug, Default)]
struct FakeTransport {
    requests: Arc<Mutex<Vec<PrepareNativeEscrowRequest>>>,
}

#[derive(Clone, Copy, Debug, Error)]
#[error("fake transport failure")]
struct FakeError;

#[async_trait]
impl LezBridgeTransport for FakeTransport {
    type Error = FakeError;

    async fn prepare_native_escrow(
        &self,
        request: PrepareNativeEscrowRequest,
    ) -> Result<PrepareNativeEscrowResult, Self::Error> {
        self.requests
            .lock()
            .expect("request log")
            .push(request.clone());
        Ok(prepared_response(request.context))
    }
}

#[tokio::test]
async fn signed_native_terms_prepare_an_exact_lez_first_lock_plan() {
    let agreement = agreement();
    let transport = FakeTransport::default();
    let adapter = adapter(transport.clone(), &agreement);

    let plan = adapter
        .prepare_native_first_lock(
            &agreement,
            RequestId::new("prepare-0001").expect("request id"),
        )
        .await
        .expect("signed terms prepare");

    let requests = transport.requests.lock().expect("request log");
    assert_eq!(
        requests.len(),
        1,
        "randomized preparation is attempted once"
    );
    let request = &requests[0];
    assert_eq!(request.context.run_id.as_str(), "native-run-0001");
    assert_eq!(request.context.request_id.as_str(), "prepare-0001");
    assert_eq!(request.context.sidecar_role, BridgeParticipant::Taker);
    assert_eq!(request.runtime, runtime(&agreement));
    assert_eq!(
        request.terms.swap_id().as_bytes(),
        agreement.onchain_swap_id()
    );
    assert_eq!(
        request.terms.terms_hash().as_bytes(),
        agreement.agreement_commitment()
    );
    assert_eq!(
        request.terms.secret_digest().as_bytes(),
        agreement.secret_digest()
    );
    assert_eq!(request.terms.depositor(), BridgeParticipant::Taker);
    assert_eq!(
        request.terms.depositor_account_id().as_bytes(),
        agreement.lez_account(Participant::Taker)
    );
    assert_eq!(request.terms.claimant(), BridgeParticipant::Maker);
    assert_eq!(
        request.terms.claimant_account_id().as_bytes(),
        agreement.lez_account(Participant::Maker)
    );
    assert_eq!(request.terms.amount().as_u128(), 42);
    assert_eq!(request.terms.refund_at_ms(), agreement.lez_refund_at_ms());
    assert_eq!(
        request.terms.authenticated_transfer_program_id().as_bytes(),
        &program_bytes(&[2; 8])
    );

    let FirstLockPlanV1::Lez { initialize, fund } = plan else {
        panic!("LEZ depositor must receive a LEZ first-lock plan");
    };
    assert_eq!(initialize.step(), FirstLockStepV1::LezInitialize);
    assert_eq!(initialize.expected_submission_id(), &[0x11; 32]);
    assert_eq!(initialize.exact_submission(), [0xaa, 0xbb]);
    assert_eq!(fund.step(), FirstLockStepV1::LezFund);
    assert_eq!(fund.expected_submission_id(), &[0x22; 32]);
    assert_eq!(fund.exact_submission(), [0xcc, 0xdd]);
}

#[tokio::test]
async fn non_depositor_is_rejected_before_randomized_preparation() {
    let agreement = agreement();
    let transport = FakeTransport::default();
    let adapter = LezBridgeAdapter::new(
        transport.clone(),
        RunId::new("native-run-0001").expect("run id"),
        RuntimeDescriptor::new(
            BridgeParticipant::Maker,
            RuntimeCompatibility::NssaV0_1_2,
            Hex32::from_bytes([6; 32]),
            Hex32::from_bytes(*agreement.lez_terms().chain().channel_id()),
            Hex32::from_bytes(*agreement.lez_terms().chain().genesis_block_hash()),
            Hex32::from_bytes(program_bytes(agreement.lez_terms().escrow_program_id())),
            Hex32::from_bytes(*agreement.lez_account(Participant::Maker)),
        ),
        Participant::Maker,
    )
    .expect("matching actor sidecar");

    let error = adapter
        .prepare_native_first_lock(
            &agreement,
            RequestId::new("prepare-0002").expect("request id"),
        )
        .await
        .expect_err("claimant cannot prepare depositor first lock");
    assert!(matches!(error, PrepareNativeFirstLockError::WrongDepositor));
    assert!(transport.requests.lock().expect("request log").is_empty());
}

#[tokio::test]
async fn runtime_identity_mismatches_are_rejected_before_preparation() {
    let agreement = agreement();

    let mut wrong_chain = runtime(&agreement);
    wrong_chain.channel_id = Hex32::from_bytes([0x91; 32]);
    assert_preparation_rejected(
        &agreement,
        wrong_chain,
        PrepareNativeFirstLockError::ChainIdentityMismatch,
        "wrong-chain",
    )
    .await;

    let mut wrong_program = runtime(&agreement);
    wrong_program.escrow_program_id = Hex32::from_bytes([0x92; 32]);
    assert_preparation_rejected(
        &agreement,
        wrong_program,
        PrepareNativeFirstLockError::EscrowProgramMismatch,
        "wrong-program",
    )
    .await;

    let mut wrong_signer = runtime(&agreement);
    wrong_signer.signer_account_id = Hex32::from_bytes([0x93; 32]);
    assert_preparation_rejected(
        &agreement,
        wrong_signer,
        PrepareNativeFirstLockError::SignerAccountMismatch,
        "wrong-signer",
    )
    .await;
}

#[tokio::test]
async fn incompatible_environment_and_token_are_rejected_without_transport() {
    for (agreement, expected, request_id) in [
        (
            agreement_for(LezEnvironmentV1::DeterministicLocalV0_2, false),
            "environment",
            "bad-environment",
        ),
        (
            agreement_for(
                LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
                true,
            ),
            "asset",
            "bad-token-asset",
        ),
    ] {
        let transport = FakeTransport::default();
        let adapter = adapter(transport.clone(), &agreement);
        let error = adapter
            .prepare_native_first_lock(&agreement, RequestId::new(request_id).expect("request id"))
            .await
            .expect_err("unsupported signed terms fail closed");
        match expected {
            "environment" => assert!(matches!(
                error,
                PrepareNativeFirstLockError::IncompatibleEnvironment
            )),
            "asset" => assert!(matches!(
                error,
                PrepareNativeFirstLockError::UnsupportedAsset
            )),
            _ => unreachable!("fixed case"),
        }
        assert!(transport.requests.lock().expect("request log").is_empty());
    }
}

#[derive(Clone, Debug, Default)]
struct FailingTransport {
    attempts: Arc<AtomicUsize>,
}

#[async_trait]
impl LezBridgeTransport for FailingTransport {
    type Error = FakeError;

    async fn prepare_native_escrow(
        &self,
        _request: PrepareNativeEscrowRequest,
    ) -> Result<PrepareNativeEscrowResult, Self::Error> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Err(FakeError)
    }
}

#[tokio::test]
async fn unknown_transport_outcome_is_not_retried() {
    let agreement = agreement();
    let transport = FailingTransport::default();
    let adapter = LezBridgeAdapter::new(
        transport.clone(),
        RunId::new("native-run-0001").expect("run id"),
        runtime(&agreement),
        Participant::Taker,
    )
    .expect("matching actor sidecar");

    let error = adapter
        .prepare_native_first_lock(
            &agreement,
            RequestId::new("unknown-outcome").expect("request id"),
        )
        .await
        .expect_err("transport delivery is unknown");
    assert!(matches!(
        error,
        PrepareNativeFirstLockError::Transport(FakeError)
    ));
    assert_eq!(transport.attempts.load(Ordering::SeqCst), 1);
}

#[derive(Clone, Copy, Debug)]
struct WrongContextTransport;

#[async_trait]
impl LezBridgeTransport for WrongContextTransport {
    type Error = FakeError;

    async fn prepare_native_escrow(
        &self,
        request: PrepareNativeEscrowRequest,
    ) -> Result<PrepareNativeEscrowResult, Self::Error> {
        let mut context = request.context;
        context.request_id = RequestId::new("wrong-response").expect("request id");
        Ok(prepared_response(context))
    }
}

#[tokio::test]
async fn prepared_bytes_with_a_different_context_are_rejected() {
    let agreement = agreement();
    let adapter = LezBridgeAdapter::new(
        WrongContextTransport,
        RunId::new("native-run-0001").expect("run id"),
        runtime(&agreement),
        Participant::Taker,
    )
    .expect("matching actor sidecar");

    let error = adapter
        .prepare_native_first_lock(
            &agreement,
            RequestId::new("expected-response").expect("request id"),
        )
        .await
        .expect_err("response context is exact");
    assert!(matches!(
        error,
        PrepareNativeFirstLockError::ResponseContextMismatch
    ));
}

async fn assert_preparation_rejected(
    agreement: &ZecAgreementV1,
    runtime: RuntimeDescriptor,
    expected: PrepareNativeFirstLockError<FakeError>,
    request_id: &str,
) {
    let transport = FakeTransport::default();
    let adapter = LezBridgeAdapter::new(
        transport.clone(),
        RunId::new("native-run-0001").expect("run id"),
        runtime,
        Participant::Taker,
    )
    .expect("matching actor sidecar");
    let actual = adapter
        .prepare_native_first_lock(agreement, RequestId::new(request_id).expect("request id"))
        .await
        .expect_err("runtime mismatch fails closed");
    assert_eq!(
        std::mem::discriminant(&actual),
        std::mem::discriminant(&expected)
    );
    assert!(transport.requests.lock().expect("request log").is_empty());
}

fn adapter(
    transport: FakeTransport,
    agreement: &ZecAgreementV1,
) -> LezBridgeAdapter<FakeTransport> {
    LezBridgeAdapter::new(
        transport,
        RunId::new("native-run-0001").expect("run id"),
        runtime(agreement),
        Participant::Taker,
    )
    .expect("matching actor sidecar")
}

fn runtime(agreement: &ZecAgreementV1) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        BridgeParticipant::Taker,
        RuntimeCompatibility::NssaV0_1_2,
        Hex32::from_bytes([6; 32]),
        Hex32::from_bytes(*agreement.lez_terms().chain().channel_id()),
        Hex32::from_bytes(*agreement.lez_terms().chain().genesis_block_hash()),
        Hex32::from_bytes(program_bytes(agreement.lez_terms().escrow_program_id())),
        Hex32::from_bytes(*agreement.lez_account(Participant::Taker)),
    )
}

fn agreement() -> ZecAgreementV1 {
    agreement_for(
        LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility,
        false,
    )
}

#[allow(clippy::too_many_lines)]
fn agreement_for(environment: LezEnvironmentV1, token: bool) -> ZecAgreementV1 {
    let maker_secret = SecretKey::from_slice(&[1; 32]).expect("maker key");
    let taker_secret = SecretKey::from_slice(&[2; 32]).expect("taker key");
    let secp = Secp256k1::new();
    let maker_key = PublicKey::from_secret_key(&secp, &maker_secret).serialize();
    let taker_key = PublicKey::from_secret_key(&secp, &taker_secret).serialize();
    let refund_hash = pubkey_hash(&maker_key);
    let claimant_hash = pubkey_hash(&taker_key);
    let secret_digest: [u8; 32] = Sha256::digest([0x91; 32]).into();
    let contract = Bip199Contract::new(120, refund_hash, secret_digest, claimant_hash);
    let binding = ZecSwapBinding::new(
        ZecProfileId::DeterministicLocalV1,
        ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            Zatoshis::from_u64(100_000_000).expect("principal"),
            contract,
        ),
    )
    .expect("profile binding");
    let id = match (environment, token) {
        (LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility, false) => {
            "lez-bridge-native-test"
        }
        (LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility, true) => "lez-bridge-token-test",
        (LezEnvironmentV1::DeterministicLocalV0_2, false) => "lez-bridge-v02-test",
        (LezEnvironmentV1::PublicTestnetV0_2, _)
        | (LezEnvironmentV1::DeterministicLocalV0_2, true) => {
            unreachable!("test fixtures cover supported deterministic combinations")
        }
    };
    let escrow_program = [1; 8];
    let onchain_id = derive_lez_swap_id_v1(id.as_bytes());
    let metadata = if environment == LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility {
        derive_nssa_v0_1_2_metadata_account_v1(&escrow_program, &onchain_id)
    } else {
        derive_lez_metadata_account_v1(&escrow_program, &onchain_id)
    };
    let (asset, custody) = if token {
        let definition_account = [9; 32];
        let token_program_id = [3; 8];
        let ata_program_id = [4; 8];
        (
            LezAssetV1::FungibleToken {
                definition_account,
                token_program_id,
                ata_program_id,
                depositor_ata: derive_nssa_v0_1_2_token_account_v1(
                    &ata_program_id,
                    &[4; 32],
                    &definition_account,
                ),
                claimant_ata: derive_nssa_v0_1_2_token_account_v1(
                    &ata_program_id,
                    &[3; 32],
                    &definition_account,
                ),
            },
            derive_nssa_v0_1_2_token_account_v1(&ata_program_id, &metadata, &definition_account),
        )
    } else {
        (
            LezAssetV1::Native {
                authenticated_transfer_program_id: [2; 8],
            },
            if environment == LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility {
                derive_nssa_v0_1_2_native_custody_account_v1(&escrow_program, &onchain_id)
            } else {
                derive_lez_native_custody_account_v1(&escrow_program, &onchain_id)
            },
        )
    };
    let body = ZecAgreementBodyV1::new(
        id,
        SwapDirection::TakerSellsLez,
        ZecProfileRecordV1::from(ZecProfileId::DeterministicLocalV1),
        ZecParticipantsV1::new(
            ZecParticipantIdentityV1::new([3; 32], maker_key),
            ZecParticipantIdentityV1::new([4; 32], taker_key),
        ),
        secret_digest,
        ZecLezTermsV1::new(
            LezChainIdentityV1::new(environment, [8; 32], [7; 32]),
            escrow_program,
            asset,
            42,
            metadata,
            custody,
        ),
        ZecSwapBindingRecordV1::from_binding(&binding),
        ZecTransactionPolicyV1::new(
            [12; 32],
            ZcashTransparentDestinationV1::p2pkh(refund_hash),
            10_000,
            1_000,
            ZcashTransparentDestinationV1::p2pkh(claimant_hash),
            10_000,
            ZcashTransparentDestinationV1::p2pkh(refund_hash),
            10_000,
            40,
        ),
        ZecRefundPlanV1::new(100, 116, 160_000, 200),
        NegotiationTranscriptV1::new([5; 32], [6; 32], 1_000),
    );
    let commitment = body.commitment();
    let record = ZecAgreementRecordV1::from_parts(
        ZEC_CONCRETE_AGREEMENT_SCHEMA_V2,
        body,
        commitment,
        secp.sign_ecdsa(&Message::from_digest(commitment), &maker_secret)
            .serialize_compact(),
        secp.sign_ecdsa(&Message::from_digest(commitment), &taker_secret)
            .serialize_compact(),
    );
    ZecAgreementV1::from_wire_at(
        &record.encode_wire().expect("bounded agreement"),
        UnixSeconds::new(10),
    )
    .expect("valid agreement")
}

fn prepared_response(context: lez_bridge_protocol::MessageContext) -> PrepareNativeEscrowResult {
    PrepareNativeEscrowResult::new(
        context,
        PreparedTransaction::new(
            TransactionId::from_bytes([0x11; 32]),
            ExactTransactionBytes::new(vec![0xaa, 0xbb]).expect("initialize bytes"),
        ),
        PreparedTransaction::new(
            TransactionId::from_bytes([0x22; 32]),
            ExactTransactionBytes::new(vec![0xcc, 0xdd]).expect("fund bytes"),
        ),
    )
}

fn pubkey_hash(bytes: &[u8; 33]) -> [u8; 20] {
    match TransparentAddress::from_pubkey(&PublicKey::from_slice(bytes).expect("public key")) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public keys produce P2PKH"),
    }
}

fn program_bytes(words: &[u32; 8]) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(words) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    bytes
}
