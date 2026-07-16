use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[cfg(target_os = "linux")]
use std::{fs, os::unix::fs::PermissionsExt as _};

use async_trait::async_trait;
use lez_bridge_protocol::{
    Hex32, MessageContext, NativeEscrowTerms, NativeEscrowTermsInput, Participant,
    PrepareNativeRefundRequest, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
    WitnessedNativeEscrowTerms, WitnessedNativeEscrowTermsInput,
};
use lez_v0_2_sidecar::{
    NativeEscrowPlanner, NativePrepareError, NonceSource, ZecEscrowInstruction,
    compute_custody_pda, compute_metadata_pda, prepared_from_transaction,
};
use nssa::{
    AccountId, PrivateKey, PublicKey, PublicTransaction, program::Program,
    public_transaction::WitnessSet,
};

const ESCROW_PROGRAM: [u32; 8] = [0x1020_3040; 8];
const TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];

#[derive(Debug)]
struct CountingNonceSource {
    calls: AtomicUsize,
}

#[async_trait]
impl NonceSource for CountingNonceSource {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(151)
    }
}

fn account(byte: u8) -> (AccountId, PrivateKey, PublicKey) {
    let key = PrivateKey::try_new([byte; 32]).unwrap();
    let public_key = PublicKey::new_from_private_key(&key);
    let account = AccountId::from(&public_key);
    (account, key, public_key)
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

fn runtime(signer: AccountId) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        Participant::Maker,
        RuntimeCompatibility::LeeV0_2_0,
        h(1),
        h(2),
        h(3),
        program_hex(ESCROW_PROGRAM),
        Hex32::from_bytes(signer.into_value()),
    )
}

fn make_refund_request(
    depositor: AccountId,
    claimant: AccountId,
    authority: AccountId,
    authority_key: &PublicKey,
    request_id: &str,
) -> PrepareNativeRefundRequest {
    let terms = WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: h(4),
        terms_hash: h(5),
        depositor: Participant::Maker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Taker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        aggregate_authority_account_id: Hex32::from_bytes(authority.into_value()),
        aggregate_x_only_public_key: Hex32::from_bytes(*authority_key.value()),
        amount: 991,
        refund_at_ms: 1_850_000_000_123,
        authenticated_transfer_program_id: program_hex(TRANSFER_PROGRAM),
    })
    .unwrap();
    PrepareNativeRefundRequest::new_witnessed(
        MessageContext::new(
            RunId::new("v02-native-refund-run-0001").unwrap(),
            RequestId::new(request_id).unwrap(),
            Participant::Maker,
        ),
        runtime(depositor),
        terms,
    )
}

#[tokio::test]
async fn prepares_exact_permissionless_witnessed_refund_without_nonce_or_witness() {
    let (depositor, key, _) = account(131);
    let (claimant, _, _) = account(132);
    let (authority, _, authority_key) = account(133);
    let nonces = Arc::new(CountingNonceSource {
        calls: AtomicUsize::new(0),
    });
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        runtime(depositor),
        Arc::clone(&nonces),
    )
    .unwrap();
    let refund_request = make_refund_request(
        depositor,
        claimant,
        authority,
        &authority_key,
        "v02-native-refund-prepare-0001",
    );

    let prepared = planner
        .prepare_native_refund(&refund_request)
        .await
        .unwrap();
    planner
        .validate_prepared_refund(&refund_request, &prepared)
        .unwrap();
    planner
        .validate_owned_submission(&prepared.refund)
        .await
        .unwrap();
    let decoded = PublicTransaction::from_bytes(prepared.refund.exact_bytes.as_slice()).unwrap();
    let swap_id = *refund_request.terms.swap_id().as_bytes();
    let metadata = compute_metadata_pda(&ESCROW_PROGRAM, &swap_id);
    let custody = compute_custody_pda(&ESCROW_PROGRAM, &swap_id);

    assert_eq!(prepared.context, refund_request.context);
    assert_eq!(decoded.to_bytes(), prepared.refund.exact_bytes.as_slice());
    assert_eq!(decoded.hash(), *prepared.refund.transaction_id.as_bytes());
    assert_eq!(decoded.message().program_id, ESCROW_PROGRAM);
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
        Program::serialize_instruction(ZecEscrowInstruction::RefundNative { swap_id }).unwrap()
    );
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        planner
            .prepare_native_refund(&refund_request)
            .await
            .unwrap(),
        prepared
    );
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 0);

    let distinct = make_refund_request(
        depositor,
        claimant,
        authority,
        &authority_key,
        "v02-native-refund-prepare-0002",
    );
    assert_eq!(
        planner.prepare_native_refund(&distinct).await.unwrap_err(),
        NativePrepareError::ActiveRefundPrepare
    );
}

#[tokio::test]
async fn preserves_hashlock_refund_wire_compatibility_with_the_same_official_abi() {
    let (depositor, key, _) = account(161);
    let (claimant, _, _) = account(162);
    let nonces = Arc::new(CountingNonceSource {
        calls: AtomicUsize::new(0),
    });
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        runtime(depositor),
        Arc::clone(&nonces),
    )
    .unwrap();
    let terms = NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: h(14),
        terms_hash: h(15),
        secret_digest: h(16),
        depositor: Participant::Maker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Taker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        amount: 992,
        refund_at_ms: 1_850_000_000_124,
        authenticated_transfer_program_id: program_hex(TRANSFER_PROGRAM),
    })
    .unwrap();
    let request = PrepareNativeRefundRequest::new(
        MessageContext::new(
            RunId::new("v02-hashlock-refund-run-0001").unwrap(),
            RequestId::new("v02-hashlock-refund-prepare-0001").unwrap(),
            Participant::Maker,
        ),
        runtime(depositor),
        terms,
    );

    let prepared = planner.prepare_native_refund(&request).await.unwrap();
    planner
        .validate_prepared_refund(&request, &prepared)
        .unwrap();
    let decoded = PublicTransaction::from_bytes(prepared.refund.exact_bytes.as_slice()).unwrap();
    assert!(decoded.message().nonces.is_empty());
    assert!(
        decoded
            .witness_set()
            .signatures_and_public_keys()
            .is_empty()
    );
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rejects_refund_account_instruction_nonce_and_witness_mutations() {
    let (depositor, key, _) = account(151);
    let (claimant, _, _) = account(152);
    let (authority, _, authority_key) = account(153);
    let nonces = Arc::new(CountingNonceSource {
        calls: AtomicUsize::new(0),
    });
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        runtime(depositor),
        Arc::clone(&nonces),
    )
    .unwrap();
    let request = make_refund_request(
        depositor,
        claimant,
        authority,
        &authority_key,
        "v02-native-refund-prepare-0021",
    );
    let prepared = planner.prepare_native_refund(&request).await.unwrap();
    let decoded = PublicTransaction::from_bytes(prepared.refund.exact_bytes.as_slice()).unwrap();

    let mut wrong_accounts = prepared.clone();
    let mut message = decoded.message().clone();
    message.account_ids.swap(0, 1);
    wrong_accounts.refund = prepared_from_transaction(&PublicTransaction::new(
        message,
        WitnessSet::from_raw_parts(Vec::new()),
    ))
    .unwrap();
    assert_eq!(
        planner
            .validate_prepared_refund(&request, &wrong_accounts)
            .unwrap_err(),
        NativePrepareError::InvalidTransactionBytes
    );

    let mut wrong_instruction = prepared.clone();
    let mut message = decoded.message().clone();
    message.instruction_data.push(0xff);
    wrong_instruction.refund = prepared_from_transaction(&PublicTransaction::new(
        message,
        WitnessSet::from_raw_parts(Vec::new()),
    ))
    .unwrap();
    assert_eq!(
        planner
            .validate_prepared_refund(&request, &wrong_instruction)
            .unwrap_err(),
        NativePrepareError::InvalidTransactionBytes
    );

    let mut injected_nonce = prepared.clone();
    let mut message = decoded.message().clone();
    message.nonces.push(7_u128.into());
    injected_nonce.refund = prepared_from_transaction(&PublicTransaction::new(
        message,
        WitnessSet::from_raw_parts(Vec::new()),
    ))
    .unwrap();
    assert_eq!(
        planner
            .validate_prepared_refund(&request, &injected_nonce)
            .unwrap_err(),
        NativePrepareError::InvalidTransactionBytes
    );

    let mut injected_witness = prepared.clone();
    let message = decoded.message().clone();
    let witness_key = PrivateKey::try_new([154; 32]).unwrap();
    injected_witness.refund = prepared_from_transaction(&PublicTransaction::new(
        message.clone(),
        WitnessSet::for_message(&message, &[&witness_key]),
    ))
    .unwrap();
    assert_eq!(
        planner
            .validate_prepared_refund(&request, &injected_witness)
            .unwrap_err(),
        NativePrepareError::InvalidTransactionBytes
    );

    assert_eq!(nonces.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn rejects_refund_signer_program_and_authority_mutations_before_nonce_rpc() {
    let (depositor, key, _) = account(171);
    let (claimant, _, _) = account(172);
    let (authority, _, authority_key) = account(173);
    let nonces = Arc::new(CountingNonceSource {
        calls: AtomicUsize::new(0),
    });
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        runtime(depositor),
        Arc::clone(&nonces),
    )
    .unwrap();
    let request = make_refund_request(
        depositor,
        claimant,
        authority,
        &authority_key,
        "v02-native-refund-prepare-0031",
    );

    let mut wrong_signer = serde_json::to_value(&request).unwrap();
    wrong_signer["terms"]["depositor_account_id"] = serde_json::json!("aa".repeat(32));
    let wrong_signer = serde_json::from_value(wrong_signer).unwrap();
    assert_eq!(
        planner
            .prepare_native_refund(&wrong_signer)
            .await
            .unwrap_err(),
        NativePrepareError::WrongSigner
    );

    let mut wrong_program = serde_json::to_value(&request).unwrap();
    wrong_program["terms"]["authenticated_transfer_program_id"] =
        serde_json::json!("ab".repeat(32));
    let wrong_program = serde_json::from_value(wrong_program).unwrap();
    assert_eq!(
        planner
            .prepare_native_refund(&wrong_program)
            .await
            .unwrap_err(),
        NativePrepareError::WrongAuthenticatedTransferProgram
    );

    let mut wrong_authority = serde_json::to_value(&request).unwrap();
    wrong_authority["terms"]["aggregate_authority_account_id"] = serde_json::json!("ac".repeat(32));
    let wrong_authority = serde_json::from_value(wrong_authority).unwrap();
    assert_eq!(
        planner
            .prepare_native_refund(&wrong_authority)
            .await
            .unwrap_err(),
        NativePrepareError::WrongAggregateAuthority
    );
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 0);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn rejects_identity_drift_and_restores_exact_refund_without_nonce_rpc() {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let (depositor, key, _) = account(141);
    let (claimant, _, _) = account(142);
    let (authority, _, authority_key) = account(143);
    let request = make_refund_request(
        depositor,
        claimant,
        authority,
        &authority_key,
        "v02-native-refund-prepare-0011",
    );
    let first_nonces = Arc::new(CountingNonceSource {
        calls: AtomicUsize::new(0),
    });
    let first = NativeEscrowPlanner::new_durable(
        Participant::Maker,
        key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        runtime(depositor),
        Arc::clone(&first_nonces),
        directory.path(),
    )
    .unwrap();

    let mut wrong_role = request.clone();
    wrong_role.context.sidecar_role = Participant::Taker;
    assert_eq!(
        first.prepare_native_refund(&wrong_role).await.unwrap_err(),
        NativePrepareError::WrongRole
    );
    let mut wrong_runtime = request.clone();
    wrong_runtime.runtime.chain_id = h(99);
    assert_eq!(
        first
            .prepare_native_refund(&wrong_runtime)
            .await
            .unwrap_err(),
        NativePrepareError::WrongRuntime
    );

    let prepared = first.prepare_native_refund(&request).await.unwrap();
    assert_eq!(first_nonces.calls.load(Ordering::SeqCst), 0);
    drop(first);

    let (_, restarted_key, _) = account(141);
    let restarted_nonces = Arc::new(CountingNonceSource {
        calls: AtomicUsize::new(0),
    });
    let restarted = NativeEscrowPlanner::new_durable(
        Participant::Maker,
        restarted_key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        runtime(depositor),
        Arc::clone(&restarted_nonces),
        directory.path(),
    )
    .unwrap();
    assert_eq!(
        restarted.prepare_native_refund(&request).await.unwrap(),
        prepared
    );
    restarted
        .validate_owned_submission(&prepared.refund)
        .await
        .unwrap();
    assert_eq!(restarted_nonces.calls.load(Ordering::SeqCst), 0);
    let distinct = make_refund_request(
        depositor,
        claimant,
        authority,
        &authority_key,
        "v02-native-refund-prepare-0012",
    );
    assert_eq!(
        restarted
            .prepare_native_refund(&distinct)
            .await
            .unwrap_err(),
        NativePrepareError::ActiveRefundPrepare
    );
    assert_eq!(restarted_nonces.calls.load(Ordering::SeqCst), 0);

    let mut tampered = prepared;
    tampered.refund.transaction_id = lez_bridge_protocol::TransactionId::from_bytes([0xee; 32]);
    assert_eq!(
        restarted
            .validate_prepared_refund(&request, &tampered)
            .unwrap_err(),
        NativePrepareError::WrongTransactionId
    );
}
