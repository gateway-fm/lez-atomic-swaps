use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[cfg(target_os = "linux")]
use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::atomic::AtomicU64,
};

use async_trait::async_trait;
use borsh::BorshDeserialize as _;
use lez_bridge_protocol::{
    AggregateBip340Signature, CompleteWitnessedClaimRequest, Hex32, MessageContext, Participant,
    PrepareWitnessedClaimRequest, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
    TransactionId, WitnessedNativeEscrowTerms, WitnessedNativeEscrowTermsInput,
};
use lez_v0_2_sidecar::{
    NativeEscrowPlanner, NativePrepareError, NonceSource, ZecEscrowInstruction,
    compute_custody_pda, compute_metadata_pda, decode_prepared_for_signer,
};
use nssa::{
    AccountId, PrivateKey, PublicKey, Signature, program::Program, public_transaction::Message,
};

const ESCROW_PROGRAM: [u32; 8] = [0x1020_3040; 8];
const TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];
const SWAP_ID: [u8; 32] = [51; 32];

#[derive(Debug)]
struct AuthorityNonce {
    authority: AccountId,
    value: u128,
    calls: AtomicUsize,
}

#[async_trait]
impl NonceSource for AuthorityNonce {
    async fn account_nonce(&self, account_id: AccountId) -> Result<u128, NativePrepareError> {
        if account_id != self.authority {
            return Err(NativePrepareError::WrongAggregateAuthority);
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.value)
    }
}

fn account(byte: u8) -> (AccountId, PrivateKey, PublicKey) {
    let key = PrivateKey::try_new([byte; 32]).unwrap();
    let public_key = PublicKey::new_from_private_key(&key);
    (AccountId::from(&public_key), key, public_key)
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn program_hex(program: [u32; 8]) -> Hex32 {
    let mut bytes = [0_u8; 32];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(program) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    Hex32::from_bytes(bytes)
}

fn runtime(destination: AccountId) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        Participant::Maker,
        RuntimeCompatibility::LeeV0_2_0,
        h(1),
        h(2),
        h(3),
        program_hex(ESCROW_PROGRAM),
        Hex32::from_bytes(destination.into_value()),
    )
}

fn prepare_request(
    destination: AccountId,
    authority: AccountId,
    authority_key: &PublicKey,
) -> PrepareWitnessedClaimRequest {
    let terms = WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: Hex32::from_bytes(SWAP_ID),
        terms_hash: h(52),
        depositor: Participant::Taker,
        depositor_account_id: h(53),
        claimant: Participant::Maker,
        claimant_account_id: Hex32::from_bytes(destination.into_value()),
        aggregate_authority_account_id: Hex32::from_bytes(authority.into_value()),
        aggregate_x_only_public_key: Hex32::from_bytes(*authority_key.value()),
        amount: 75,
        refund_at_ms: 1_850_000_000_123,
        authenticated_transfer_program_id: program_hex(TRANSFER_PROGRAM),
    })
    .unwrap();
    PrepareWitnessedClaimRequest::new(
        MessageContext::new(
            RunId::new("witnessed-run-0001").unwrap(),
            RequestId::new("witnessed-prepare-0001").unwrap(),
            Participant::Maker,
        ),
        runtime(destination),
        terms,
        TransactionId::from_bytes([54; 32]),
    )
}

fn completion(
    prepared: &lez_bridge_protocol::PrepareWitnessedClaimResult,
    authority_key: &PrivateKey,
    runtime: RuntimeDescriptor,
    request_id: &str,
) -> CompleteWitnessedClaimRequest {
    let message = Message::try_from_slice(prepared.claim.exact_message_bytes.as_slice()).unwrap();
    assert_eq!(
        borsh::to_vec(&message).unwrap(),
        prepared.claim.exact_message_bytes.as_slice()
    );
    assert_eq!(message.hash(), *prepared.claim.message_hash.as_bytes());
    let signature = Signature::new(authority_key, &message.hash());
    CompleteWitnessedClaimRequest::new(
        MessageContext::new(
            prepared.context.run_id.clone(),
            RequestId::new(request_id).unwrap(),
            prepared.context.sidecar_role,
        ),
        runtime,
        prepared.claim.clone(),
        AggregateBip340Signature::from_bytes(signature.value),
    )
}

#[tokio::test]
async fn exact_unsigned_message_completes_to_one_authority_signed_public_transaction() {
    let (destination, destination_key, _) = account(21);
    let (authority, authority_key, authority_public) = account(22);
    assert_ne!(destination, authority);
    let nonces = Arc::new(AuthorityNonce {
        authority,
        value: 41,
        calls: AtomicUsize::new(0),
    });
    let expected_runtime = runtime(destination);
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        destination_key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        expected_runtime.clone(),
        Arc::clone(&nonces),
    )
    .unwrap();
    let request = prepare_request(destination, authority, &authority_public);

    let prepared = planner.prepare_witnessed_claim(&request).await.unwrap();
    planner
        .validate_prepared_witnessed_claim(&request, &prepared)
        .unwrap();
    assert_eq!(
        prepared.claim.preparation_request_id,
        request.context.request_id
    );
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 1);
    let message = Message::try_from_slice(prepared.claim.exact_message_bytes.as_slice()).unwrap();
    assert_eq!(message.program_id, ESCROW_PROGRAM);
    assert_eq!(message.nonces, vec![41_u128.into()]);
    assert_eq!(
        message.account_ids,
        vec![
            compute_metadata_pda(&ESCROW_PROGRAM, &SWAP_ID),
            compute_custody_pda(&ESCROW_PROGRAM, &SWAP_ID),
            destination,
            authority,
        ]
    );
    assert_eq!(
        message.instruction_data,
        Program::serialize_instruction(ZecEscrowInstruction::ClaimNativeWitnessed {
            swap_id: SWAP_ID,
        })
        .unwrap()
    );

    let complete = completion(
        &prepared,
        &authority_key,
        expected_runtime,
        "witnessed-complete-0001",
    );
    let completed = planner.complete_witnessed_claim(&complete).await.unwrap();
    let transaction = decode_prepared_for_signer(&completed.claim, authority).unwrap();
    assert_eq!(transaction.message(), &message);
    assert_eq!(
        transaction.witness_set().signatures_and_public_keys()[0].1,
        authority_public
    );
    planner
        .validate_owned_submission(&completed.claim)
        .await
        .unwrap();
    assert_eq!(
        planner.prepare_witnessed_claim(&request).await.unwrap(),
        prepared
    );
    assert_eq!(
        planner.complete_witnessed_claim(&complete).await.unwrap(),
        completed
    );
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 1);

    let mut conflicting = complete;
    conflicting.context.request_id = RequestId::new("witnessed-complete-0002").unwrap();
    conflicting.aggregate_signature = AggregateBip340Signature::from_bytes([7; 64]);
    assert_eq!(
        planner
            .complete_witnessed_claim(&conflicting)
            .await
            .unwrap_err(),
        NativePrepareError::ActiveWitnessedClaimCompletion
    );
}

#[tokio::test]
async fn rejects_role_authority_and_transcript_drift_before_reserving_completion() {
    let (destination, destination_key, _) = account(24);
    let (authority, authority_key, authority_public) = account(25);
    let nonces = Arc::new(AuthorityNonce {
        authority,
        value: 47,
        calls: AtomicUsize::new(0),
    });
    let expected_runtime = runtime(destination);
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        destination_key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        expected_runtime.clone(),
        Arc::clone(&nonces),
    )
    .unwrap();

    let mut wrong_role = prepare_request(destination, authority, &authority_public);
    wrong_role.context.sidecar_role = Participant::Taker;
    assert_eq!(
        planner
            .prepare_witnessed_claim(&wrong_role)
            .await
            .unwrap_err(),
        NativePrepareError::WrongRole
    );

    let (wrong_authority, _, _) = account(26);
    let wrong_authority = prepare_request(destination, wrong_authority, &authority_public);
    assert_eq!(
        planner
            .prepare_witnessed_claim(&wrong_authority)
            .await
            .unwrap_err(),
        NativePrepareError::WrongAggregateAuthority
    );
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 0);

    let request = prepare_request(destination, authority, &authority_public);
    let prepared = planner.prepare_witnessed_claim(&request).await.unwrap();
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 1);

    let mut drifted = completion(
        &prepared,
        &authority_key,
        expected_runtime.clone(),
        "witnessed-complete-drifted",
    );
    drifted.claim.message_hash = h(99);
    assert_eq!(
        planner
            .complete_witnessed_claim(&drifted)
            .await
            .unwrap_err(),
        NativePrepareError::InvalidTransactionBytes
    );

    let mut invalid_signature = completion(
        &prepared,
        &authority_key,
        expected_runtime.clone(),
        "witnessed-complete-invalid-signature",
    );
    let mut signature_bytes = *invalid_signature.aggregate_signature.as_bytes();
    signature_bytes[0] ^= 1;
    invalid_signature.aggregate_signature = AggregateBip340Signature::from_bytes(signature_bytes);
    assert_eq!(
        planner
            .complete_witnessed_claim(&invalid_signature)
            .await
            .unwrap_err(),
        NativePrepareError::InvalidSignature
    );

    let valid = completion(
        &prepared,
        &authority_key,
        expected_runtime,
        "witnessed-complete-valid",
    );
    let _ = planner.complete_witnessed_claim(&valid).await.unwrap();
}

#[cfg(target_os = "linux")]
static DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct SecureDirectory(PathBuf);

#[cfg(target_os = "linux")]
impl SecureDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "lez-v02-witnessed-{}-{}",
            std::process::id(),
            DIRECTORY_SEQUENCE.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

#[cfg(target_os = "linux")]
impl Drop for SecureDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn fresh_process_loads_exact_preparation_before_completion_and_never_rereads_nonce() {
    let directory = SecureDirectory::new();
    let (destination, destination_key, _) = account(31);
    let (authority, authority_key, authority_public) = account(32);
    let request = prepare_request(destination, authority, &authority_public);
    let first_nonces = Arc::new(AuthorityNonce {
        authority,
        value: 73,
        calls: AtomicUsize::new(0),
    });
    let first = NativeEscrowPlanner::new_durable(
        Participant::Maker,
        destination_key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        runtime(destination),
        Arc::clone(&first_nonces),
        directory.path(),
    )
    .unwrap();
    let prepared = first.prepare_witnessed_claim(&request).await.unwrap();
    assert_eq!(first_nonces.calls.load(Ordering::SeqCst), 1);
    let complete = completion(
        &prepared,
        &authority_key,
        runtime(destination),
        "witnessed-complete-0001",
    );
    drop(first);

    let (_, restarted_destination_key, _) = account(31);
    let restarted_nonces = Arc::new(AuthorityNonce {
        authority,
        value: 999,
        calls: AtomicUsize::new(0),
    });
    let restarted = NativeEscrowPlanner::new_durable(
        Participant::Maker,
        restarted_destination_key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        runtime(destination),
        Arc::clone(&restarted_nonces),
        directory.path(),
    )
    .unwrap();
    let completed = restarted.complete_witnessed_claim(&complete).await.unwrap();
    assert_eq!(restarted_nonces.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        decode_prepared_for_signer(&completed.claim, authority)
            .unwrap()
            .message()
            .hash(),
        *prepared.claim.message_hash.as_bytes()
    );
    drop(restarted);

    let (_, second_restart_key, _) = account(31);
    let second_restart_nonces = Arc::new(AuthorityNonce {
        authority,
        value: 1_000,
        calls: AtomicUsize::new(0),
    });
    let second_restart = NativeEscrowPlanner::new_durable(
        Participant::Maker,
        second_restart_key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        runtime(destination),
        Arc::clone(&second_restart_nonces),
        directory.path(),
    )
    .unwrap();
    assert_eq!(
        second_restart
            .complete_witnessed_claim(&complete)
            .await
            .unwrap(),
        completed
    );
    assert_eq!(second_restart_nonces.calls.load(Ordering::SeqCst), 0);
}
