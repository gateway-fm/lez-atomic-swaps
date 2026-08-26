use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use lez_bridge_protocol::{
    Hex32, MessageContext, NativeEscrowTerms, NativeEscrowTermsInput, Participant,
    PrepareNativeRefundRequest, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
};
use lez_v0_1_2_sidecar::{NativeEscrowPlanner, NonceSource, SidecarError};
use lez_zec_escrow_compat::Instruction as EscrowInstruction;
use nssa::{AccountId, PrivateKey, PublicKey, PublicTransaction, program::Program};
use sha2::{Digest as _, Sha256};

#[derive(Debug)]
struct CountingNonceSource {
    calls: AtomicUsize,
}

#[async_trait]
impl NonceSource for CountingNonceSource {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, SidecarError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(151)
    }
}

fn keyed_account(byte: u8) -> (AccountId, PrivateKey) {
    let key = PrivateKey::try_new([byte; 32]).unwrap();
    let account = AccountId::from(&PublicKey::new_from_private_key(&key));
    (account, key)
}

fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn program_hex(program_id: [u32; 8]) -> Hex32 {
    let mut bytes = [0_u8; 32];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(program_id) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    Hex32::from_bytes(bytes)
}

fn runtime(signer: AccountId, escrow_program: [u32; 8]) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        Participant::Maker,
        RuntimeCompatibility::NssaV0_1_2,
        h(1),
        h(2),
        h(3),
        program_hex(escrow_program),
        Hex32::from_bytes(signer.into_value()),
    )
}

fn refund_request(
    depositor: AccountId,
    claimant: AccountId,
    escrow_program: [u32; 8],
) -> PrepareNativeRefundRequest {
    let context = MessageContext::new(
        RunId::new("refund-planner-run-0001").unwrap(),
        RequestId::new("refund-prepare-0001").unwrap(),
        Participant::Maker,
    );
    let terms = NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: h(4),
        terms_hash: h(5),
        secret_digest: Hex32::from_bytes(Sha256::digest([42_u8; 32]).into()),
        depositor: Participant::Maker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Taker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        amount: 991,
        refund_at_ms: 1_800_000_000_123,
        authenticated_transfer_program_id: program_hex(
            Program::authenticated_transfer_program().id(),
        ),
    })
    .unwrap();
    PrepareNativeRefundRequest::new(context, runtime(depositor, escrow_program), terms)
}

#[tokio::test]
async fn prepares_exact_permissionless_refund_without_nonce_or_witness() {
    let (depositor, key) = keyed_account(131);
    let (claimant, _) = keyed_account(132);
    let escrow_program = [0x1020_3040; 8];
    let nonces = Arc::new(CountingNonceSource {
        calls: AtomicUsize::new(0),
    });
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        escrow_program,
        runtime(depositor, escrow_program),
        Arc::clone(&nonces),
    )
    .unwrap();
    let request = refund_request(depositor, claimant, escrow_program);

    let prepared = planner
        .prepare_native_refund(request.clone())
        .await
        .unwrap();
    let decoded = PublicTransaction::from_bytes(prepared.refund.exact_bytes.as_slice()).unwrap();
    let swap_id = *request.terms.swap_id().as_bytes();
    let metadata = spel_framework_core::pda::compute_pda(&escrow_program, &[&swap_id]);
    let custody_label = spel_framework_core::pda::seed_from_str("custody");
    let custody =
        spel_framework_core::pda::compute_pda(&escrow_program, &[&custody_label, &swap_id]);

    assert_eq!(prepared.context, request.context);
    assert_eq!(decoded.to_bytes(), prepared.refund.exact_bytes.as_slice());
    assert_eq!(decoded.hash(), *prepared.refund.transaction_id.as_bytes());
    assert_eq!(decoded.message().program_id, escrow_program);
    assert_eq!(
        decoded.message().account_ids,
        [metadata, custody, depositor]
    );
    assert!(decoded.message().nonces.is_empty());
    assert!(
        decoded
            .witness_set()
            .signatures_and_public_keys()
            .is_empty()
    );
    assert_eq!(
        decoded.message().instruction_data,
        Program::serialize_instruction(EscrowInstruction::RefundNative { swap_id }).unwrap()
    );
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        planner.prepare_native_refund(request).await.unwrap(),
        prepared
    );
}

#[tokio::test]
async fn rejects_refund_identity_drift_and_restores_exact_cache_without_nonce() {
    let (depositor, key) = keyed_account(141);
    let restore_key = PrivateKey::try_new([141; 32]).unwrap();
    let (claimant, _) = keyed_account(142);
    let escrow_program = [0x5060_7080; 8];
    let nonces = Arc::new(CountingNonceSource {
        calls: AtomicUsize::new(0),
    });
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        escrow_program,
        runtime(depositor, escrow_program),
        Arc::clone(&nonces),
    )
    .unwrap();
    let request = refund_request(depositor, claimant, escrow_program);
    let mut wrong_role = request.clone();
    wrong_role.context.sidecar_role = Participant::Taker;
    assert_eq!(
        planner.prepare_native_refund(wrong_role).await.unwrap_err(),
        SidecarError::WrongSidecarRole
    );
    let mut wrong_runtime = request.clone();
    wrong_runtime.runtime.channel_id = h(0xaa);
    assert_eq!(
        planner
            .prepare_native_refund(wrong_runtime)
            .await
            .unwrap_err(),
        SidecarError::WrongRuntimeIdentity
    );
    let mut wrong_signer = serde_json::to_value(request.clone()).unwrap();
    wrong_signer["terms"]["depositor_account_id"] = serde_json::json!("aa".repeat(32));
    let wrong_signer: PrepareNativeRefundRequest = serde_json::from_value(wrong_signer).unwrap();
    assert_eq!(
        planner
            .prepare_native_refund(wrong_signer)
            .await
            .unwrap_err(),
        SidecarError::WrongSigner
    );

    let prepared = planner
        .prepare_native_refund(request.clone())
        .await
        .unwrap();
    let mut distinct = request.clone();
    distinct.context.request_id = RequestId::new("refund-prepare-0002").unwrap();
    assert_eq!(
        planner.prepare_native_refund(distinct).await.unwrap_err(),
        SidecarError::ActiveRefundPrepare
    );

    let restored_nonces = Arc::new(CountingNonceSource {
        calls: AtomicUsize::new(0),
    });
    let restored = NativeEscrowPlanner::new(
        Participant::Maker,
        restore_key,
        escrow_program,
        runtime(depositor, escrow_program),
        Arc::clone(&restored_nonces),
    )
    .unwrap();
    let mut tampered = prepared.clone();
    tampered.refund.transaction_id = lez_bridge_protocol::TransactionId::from_bytes([0xee; 32]);
    assert_eq!(
        restored
            .restore_native_refund(request.clone(), tampered)
            .await
            .unwrap_err(),
        SidecarError::InvalidTransactionBytes
    );
    restored
        .restore_native_refund(request.clone(), prepared.clone())
        .await
        .unwrap();
    assert_eq!(
        restored.prepare_native_refund(request).await.unwrap(),
        prepared
    );
    restored
        .decode_exact_for_submission(&prepared.refund, Participant::Maker)
        .await
        .unwrap();
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 0);
    assert_eq!(restored_nonces.calls.load(Ordering::SeqCst), 0);
}
