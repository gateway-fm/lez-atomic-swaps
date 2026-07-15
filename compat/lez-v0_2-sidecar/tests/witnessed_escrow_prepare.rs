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
use lez_bridge_protocol::{
    Hex32, MessageContext, Participant, PrepareWitnessedEscrowRequest, RequestId, RunId,
    RuntimeCompatibility, RuntimeDescriptor, WitnessedNativeEscrowTerms,
    WitnessedNativeEscrowTermsInput,
};
use lez_v0_2_sidecar::{
    NativeEscrowPlanner, NativePrepareError, NonceSource, ZecEscrowInstruction,
    compute_custody_pda, compute_metadata_pda, decode_prepared_for_signer,
};
use nssa::{AccountId, PrivateKey, PublicKey, program::Program};

const ESCROW_PROGRAM: [u32; 8] = [0x1020_3040; 8];
const TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];
const SWAP_ID: [u8; 32] = [51; 32];

#[derive(Debug)]
struct DepositorNonce {
    depositor: AccountId,
    value: u128,
    calls: AtomicUsize,
}

#[async_trait]
impl NonceSource for DepositorNonce {
    async fn account_nonce(&self, account_id: AccountId) -> Result<u128, NativePrepareError> {
        if account_id != self.depositor {
            return Err(NativePrepareError::WrongSigner);
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

fn runtime(depositor: AccountId) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        Participant::Maker,
        RuntimeCompatibility::LeeV0_2_0,
        h(1),
        h(2),
        h(3),
        program_hex(ESCROW_PROGRAM),
        Hex32::from_bytes(depositor.into_value()),
    )
}

fn request(
    depositor: AccountId,
    claimant: AccountId,
    authority: AccountId,
    authority_key: &PublicKey,
) -> PrepareWitnessedEscrowRequest {
    let terms = WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: Hex32::from_bytes(SWAP_ID),
        terms_hash: h(52),
        depositor: Participant::Maker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Taker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        aggregate_authority_account_id: Hex32::from_bytes(authority.into_value()),
        aggregate_x_only_public_key: Hex32::from_bytes(*authority_key.value()),
        amount: 75,
        refund_at_ms: 1_850_000_000_123,
        authenticated_transfer_program_id: program_hex(TRANSFER_PROGRAM),
    })
    .unwrap();
    PrepareWitnessedEscrowRequest::new(
        MessageContext::new(
            RunId::new("witnessed-escrow-run-0001").unwrap(),
            RequestId::new("witnessed-escrow-prepare-0001").unwrap(),
            Participant::Maker,
        ),
        runtime(depositor),
        terms,
    )
}

#[tokio::test]
async fn prepares_exact_generated_witnessed_initialization_and_funding_pair() {
    let (depositor, depositor_key, _) = account(21);
    let (claimant, _, _) = account(22);
    let (authority, _, authority_key) = account(23);
    let nonces = Arc::new(DepositorNonce {
        depositor,
        value: 41,
        calls: AtomicUsize::new(0),
    });
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        depositor_key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        runtime(depositor),
        Arc::clone(&nonces),
    )
    .unwrap();
    let request = request(depositor, claimant, authority, &authority_key);

    let prepared = planner.prepare_witnessed_escrow(&request).await.unwrap();
    planner
        .validate_prepared_witnessed_escrow(&request, &prepared)
        .unwrap();
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 1);
    let initialization = decode_prepared_for_signer(&prepared.initialization, depositor).unwrap();
    let funding = decode_prepared_for_signer(&prepared.funding, depositor).unwrap();
    let metadata = compute_metadata_pda(&ESCROW_PROGRAM, &SWAP_ID);
    let custody = compute_custody_pda(&ESCROW_PROGRAM, &SWAP_ID);
    assert_eq!(initialization.message().nonces, vec![41_u128.into()]);
    assert_eq!(funding.message().nonces, vec![42_u128.into()]);
    assert_eq!(
        initialization.message().account_ids,
        vec![metadata, custody, depositor, claimant, authority]
    );
    assert_eq!(
        funding.message().account_ids,
        vec![metadata, custody, depositor]
    );
    assert_eq!(
        initialization.message().instruction_data,
        Program::serialize_instruction(ZecEscrowInstruction::InitializeNativeWitnessed {
            swap_id: SWAP_ID,
            terms_hash: *request.terms.terms_hash().as_bytes(),
            aggregate_x_only_public_key: *authority_key.value(),
            amount: request.terms.amount().as_u128(),
            refund_at: request.terms.refund_at_ms(),
            authenticated_transfer_program: TRANSFER_PROGRAM,
        })
        .unwrap()
    );
    assert_eq!(
        funding.message().instruction_data,
        Program::serialize_instruction(ZecEscrowInstruction::FundNative { swap_id: SWAP_ID })
            .unwrap()
    );
    assert_eq!(
        planner.prepare_witnessed_escrow(&request).await.unwrap(),
        prepared
    );
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 1);
    planner
        .validate_owned_submission(&prepared.initialization)
        .await
        .unwrap();
    planner
        .validate_owned_submission(&prepared.funding)
        .await
        .unwrap();
}

#[tokio::test]
async fn rejects_cross_wired_role_and_aggregate_authority_before_nonce_read() {
    let (depositor, depositor_key, _) = account(24);
    let (claimant, _, _) = account(25);
    let (authority, _, authority_key) = account(26);
    let nonces = Arc::new(DepositorNonce {
        depositor,
        value: 47,
        calls: AtomicUsize::new(0),
    });
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        depositor_key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        runtime(depositor),
        Arc::clone(&nonces),
    )
    .unwrap();

    let mut wrong_role = request(depositor, claimant, authority, &authority_key);
    wrong_role.context.sidecar_role = Participant::Taker;
    assert_eq!(
        planner
            .prepare_witnessed_escrow(&wrong_role)
            .await
            .unwrap_err(),
        NativePrepareError::WrongRole
    );

    let (wrong_authority, _, _) = account(27);
    let wrong_authority = request(depositor, claimant, wrong_authority, &authority_key);
    assert_eq!(
        planner
            .prepare_witnessed_escrow(&wrong_authority)
            .await
            .unwrap_err(),
        NativePrepareError::WrongAggregateAuthority
    );
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 0);
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
            "lez-v02-witnessed-escrow-{}-{}",
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
async fn fresh_process_recovers_exact_pair_without_rereading_nonce() {
    let directory = SecureDirectory::new();
    let (depositor, depositor_key, _) = account(31);
    let (claimant, _, _) = account(32);
    let (authority, _, authority_key) = account(33);
    let request = request(depositor, claimant, authority, &authority_key);
    let first_nonces = Arc::new(DepositorNonce {
        depositor,
        value: 73,
        calls: AtomicUsize::new(0),
    });
    let first = NativeEscrowPlanner::new_durable(
        Participant::Maker,
        depositor_key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        runtime(depositor),
        Arc::clone(&first_nonces),
        directory.path(),
    )
    .unwrap();
    let prepared = first.prepare_witnessed_escrow(&request).await.unwrap();
    assert_eq!(first_nonces.calls.load(Ordering::SeqCst), 1);
    drop(first);

    let (_, restarted_key, _) = account(31);
    let restarted_nonces = Arc::new(DepositorNonce {
        depositor,
        value: 999,
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
        restarted.prepare_witnessed_escrow(&request).await.unwrap(),
        prepared
    );
    assert_eq!(restarted_nonces.calls.load(Ordering::SeqCst), 0);
    restarted
        .validate_owned_submission(&prepared.funding)
        .await
        .unwrap();
}
