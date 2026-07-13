use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use lez_bridge_protocol::{
    ExactTransactionBytes, Hex32, MessageContext, NativeEscrowTerms, NativeEscrowTermsInput,
    Participant, PrepareNativeEscrowRequest, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor, TransactionId,
};
use lez_v0_2_sidecar::{
    NativeEscrowPlanner, NativePrepareError, NonceSource, ZecEscrowInstruction,
    compute_custody_pda, compute_metadata_pda, decode_prepared_for_signer,
    prepared_from_transaction,
};
use nssa::{
    AccountId, PrivateKey, PublicKey, PublicTransaction, program::Program,
    public_transaction::WitnessSet,
};

#[derive(Debug)]
struct FixedNonce {
    value: u128,
    calls: AtomicUsize,
}

#[async_trait]
impl NonceSource for FixedNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.value)
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

fn runtime(role: Participant, signer: AccountId, escrow_program: [u32; 8]) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::LeeV0_2_0,
        h(1),
        h(2),
        h(3),
        program_hex(escrow_program),
        Hex32::from_bytes(signer.into_value()),
    )
}

fn request(
    role: Participant,
    signer: AccountId,
    claimant: AccountId,
    escrow_program: [u32; 8],
    authenticated_transfer_program: [u32; 8],
) -> PrepareNativeEscrowRequest {
    let terms = NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: h(4),
        terms_hash: h(5),
        secret_digest: h(6),
        depositor: role,
        depositor_account_id: Hex32::from_bytes(signer.into_value()),
        claimant: match role {
            Participant::Maker => Participant::Taker,
            Participant::Taker => Participant::Maker,
        },
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        amount: u128::from(u64::MAX) + 7,
        refund_at_ms: 1_850_000_000_123,
        authenticated_transfer_program_id: program_hex(authenticated_transfer_program),
    })
    .unwrap();
    PrepareNativeEscrowRequest::new(
        MessageContext::new(
            RunId::new("v02-native-run-0001").unwrap(),
            RequestId::new("v02-native-prepare-0001").unwrap(),
            role,
        ),
        runtime(role, signer, escrow_program),
        terms,
    )
}

#[tokio::test]
async fn prepares_one_exact_official_v02_pair_with_owned_consecutive_nonces() {
    let (depositor, key) = keyed_account(21);
    let (claimant, _) = keyed_account(22);
    let escrow_program = [0x1020_3040; 8];
    let authenticated_transfer_program = [0x5060_7080; 8];
    let nonces = Arc::new(FixedNonce {
        value: 41,
        calls: AtomicUsize::new(0),
    });
    let expected_runtime = runtime(Participant::Maker, depositor, escrow_program);
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        escrow_program,
        authenticated_transfer_program,
        expected_runtime,
        Arc::clone(&nonces),
    )
    .unwrap();
    let request = request(
        Participant::Maker,
        depositor,
        claimant,
        escrow_program,
        authenticated_transfer_program,
    );

    let prepared = planner.prepare(request.clone()).await.unwrap();
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 1);
    assert_eq!(prepared.context, request.context);
    planner.validate_prepared(&request, &prepared).unwrap();
    let initialization = decode_prepared_for_signer(&prepared.initialization, depositor).unwrap();
    let funding = decode_prepared_for_signer(&prepared.funding, depositor).unwrap();
    assert_eq!(initialization.message.program_id, escrow_program);
    assert_eq!(funding.message.program_id, escrow_program);
    assert_eq!(initialization.message.nonces, vec![41_u128.into()]);
    assert_eq!(funding.message.nonces, vec![42_u128.into()]);
    let swap_id = *request.terms.swap_id().as_bytes();
    let metadata = compute_metadata_pda(&escrow_program, &swap_id);
    let custody = compute_custody_pda(&escrow_program, &swap_id);
    assert_eq!(
        initialization.message.account_ids,
        vec![metadata, custody, depositor, claimant]
    );
    assert_eq!(
        funding.message.account_ids,
        vec![metadata, custody, depositor]
    );
    assert_eq!(
        initialization.message.instruction_data,
        Program::serialize_instruction(ZecEscrowInstruction::InitializeNative {
            swap_id,
            terms_hash: *request.terms.terms_hash().as_bytes(),
            secret_digest: *request.terms.secret_digest().as_bytes(),
            amount: request.terms.amount().as_u128(),
            refund_at: request.terms.refund_at_ms(),
            authenticated_transfer_program,
        })
        .unwrap()
    );
    assert_eq!(
        funding.message.instruction_data,
        Program::serialize_instruction(ZecEscrowInstruction::FundNative { swap_id }).unwrap()
    );
    assert_eq!(
        initialization.hash(),
        *prepared.initialization.transaction_id.as_bytes()
    );
    assert_eq!(funding.hash(), *prepared.funding.transaction_id.as_bytes());
    assert_eq!(
        initialization.to_bytes(),
        prepared.initialization.exact_bytes.as_slice()
    );
    assert_eq!(funding.to_bytes(), prepared.funding.exact_bytes.as_slice());

    assert_eq!(planner.prepare(request).await.unwrap(), prepared);
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejects_role_runtime_signer_program_account_and_nonce_cross_wiring_before_signing() {
    let (depositor, key) = keyed_account(31);
    let (claimant, _) = keyed_account(32);
    let escrow_program = [17; 8];
    let authenticated_transfer_program = [18; 8];
    let nonces = Arc::new(FixedNonce {
        value: 9,
        calls: AtomicUsize::new(0),
    });
    let expected_runtime = runtime(Participant::Maker, depositor, escrow_program);
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        escrow_program,
        authenticated_transfer_program,
        expected_runtime.clone(),
        Arc::clone(&nonces),
    )
    .unwrap();
    let valid = request(
        Participant::Maker,
        depositor,
        claimant,
        escrow_program,
        authenticated_transfer_program,
    );

    let mut wrong_context_role = valid.clone();
    wrong_context_role.context.sidecar_role = Participant::Taker;
    assert_eq!(
        planner.prepare(wrong_context_role).await.unwrap_err(),
        NativePrepareError::WrongRole
    );
    let mut wrong_runtime = valid.clone();
    wrong_runtime.runtime.chain_id = h(99);
    assert_eq!(
        planner.prepare(wrong_runtime).await.unwrap_err(),
        NativePrepareError::WrongRuntime
    );
    let (other_signer, _) = keyed_account(33);
    let wrong_signer = request(
        Participant::Maker,
        other_signer,
        claimant,
        escrow_program,
        authenticated_transfer_program,
    );
    assert_eq!(
        planner.prepare(wrong_signer).await.unwrap_err(),
        NativePrepareError::WrongRuntime
    );
    let mut wrong_account = valid.clone();
    wrong_account.terms = request(
        Participant::Maker,
        other_signer,
        claimant,
        escrow_program,
        authenticated_transfer_program,
    )
    .terms;
    wrong_account.runtime = expected_runtime;
    assert_eq!(
        planner.prepare(wrong_account).await.unwrap_err(),
        NativePrepareError::WrongSigner
    );
    let wrong_program = request(
        Participant::Maker,
        depositor,
        claimant,
        escrow_program,
        [19; 8],
    );
    assert_eq!(
        planner.prepare(wrong_program).await.unwrap_err(),
        NativePrepareError::WrongAuthenticatedTransferProgram
    );
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 0);

    let overflow_planner = NativeEscrowPlanner::new(
        Participant::Maker,
        PrivateKey::try_new([31; 32]).unwrap(),
        escrow_program,
        authenticated_transfer_program,
        runtime(Participant::Maker, depositor, escrow_program),
        Arc::new(FixedNonce {
            value: u128::MAX,
            calls: AtomicUsize::new(0),
        }),
    )
    .unwrap();
    assert_eq!(
        overflow_planner.prepare(valid).await.unwrap_err(),
        NativePrepareError::NonceOverflow
    );
}

#[tokio::test]
async fn rejects_distinct_active_nonce_reservation() {
    let (depositor, key) = keyed_account(41);
    let (claimant, _) = keyed_account(42);
    let (other_claimant, _) = keyed_account(43);
    let escrow_program = [21; 8];
    let authenticated_transfer_program = [22; 8];
    let planner = NativeEscrowPlanner::new(
        Participant::Taker,
        key,
        escrow_program,
        authenticated_transfer_program,
        runtime(Participant::Taker, depositor, escrow_program),
        Arc::new(FixedNonce {
            value: 7,
            calls: AtomicUsize::new(0),
        }),
    )
    .unwrap();
    let _prepared = planner
        .prepare(request(
            Participant::Taker,
            depositor,
            claimant,
            escrow_program,
            authenticated_transfer_program,
        ))
        .await
        .unwrap();
    assert_eq!(
        planner
            .prepare(request(
                Participant::Taker,
                depositor,
                other_claimant,
                escrow_program,
                authenticated_transfer_program,
            ))
            .await
            .unwrap_err(),
        NativePrepareError::ActivePrepare
    );
}

#[tokio::test]
async fn exact_decoder_rejects_signer_id_signature_and_byte_substitution() {
    let (depositor, key) = keyed_account(51);
    let (claimant, _) = keyed_account(52);
    let escrow_program = [31; 8];
    let authenticated_transfer_program = [32; 8];
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        escrow_program,
        authenticated_transfer_program,
        runtime(Participant::Maker, depositor, escrow_program),
        Arc::new(FixedNonce {
            value: 3,
            calls: AtomicUsize::new(0),
        }),
    )
    .unwrap();
    let prepared = planner
        .prepare(request(
            Participant::Maker,
            depositor,
            claimant,
            escrow_program,
            authenticated_transfer_program,
        ))
        .await
        .unwrap();

    let (wrong_signer, _) = keyed_account(53);
    assert_eq!(
        decode_prepared_for_signer(&prepared.initialization, wrong_signer).unwrap_err(),
        NativePrepareError::WrongSigner
    );
    let mut wrong_id = prepared.initialization.clone();
    wrong_id.transaction_id = TransactionId::from_bytes([99; 32]);
    assert_eq!(
        decode_prepared_for_signer(&wrong_id, depositor).unwrap_err(),
        NativePrepareError::WrongTransactionId
    );

    let decoded =
        PublicTransaction::from_bytes(prepared.initialization.exact_bytes.as_slice()).unwrap();
    let invalid_signature = PublicTransaction::new(
        decoded.message.clone(),
        WitnessSet::from_raw_parts(Vec::new()),
    );
    let invalid_signature = lez_bridge_protocol::PreparedTransaction::new(
        TransactionId::from_bytes(invalid_signature.hash()),
        ExactTransactionBytes::new(invalid_signature.to_bytes()).unwrap(),
    );
    assert_eq!(
        decode_prepared_for_signer(&invalid_signature, depositor).unwrap_err(),
        NativePrepareError::InvalidSignature
    );

    let mut substituted = prepared.initialization;
    let mut bytes = substituted.exact_bytes.as_slice().to_vec();
    bytes.push(0);
    substituted.exact_bytes = ExactTransactionBytes::new(bytes).unwrap();
    assert_eq!(
        decode_prepared_for_signer(&substituted, depositor).unwrap_err(),
        NativePrepareError::InvalidTransactionBytes
    );
}

#[tokio::test]
async fn recovered_pair_rejects_validly_signed_account_instruction_and_nonce_substitution() {
    let (depositor, key) = keyed_account(71);
    let (claimant, _) = keyed_account(72);
    let escrow_program = [51; 8];
    let authenticated_transfer_program = [52; 8];
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key.clone(),
        escrow_program,
        authenticated_transfer_program,
        runtime(Participant::Maker, depositor, escrow_program),
        Arc::new(FixedNonce {
            value: 80,
            calls: AtomicUsize::new(0),
        }),
    )
    .unwrap();
    let request = request(
        Participant::Maker,
        depositor,
        claimant,
        escrow_program,
        authenticated_transfer_program,
    );
    let prepared = planner.prepare(request.clone()).await.unwrap();

    let initialization = decode_prepared_for_signer(&prepared.initialization, depositor).unwrap();
    let mut account_substitution = initialization.clone();
    account_substitution.message.account_ids.swap(0, 1);
    account_substitution.witness_set =
        WitnessSet::for_message(&account_substitution.message, &[&key]);
    let mut substituted_pair = prepared.clone();
    substituted_pair.initialization = prepared_from_transaction(&account_substitution).unwrap();
    assert_eq!(
        planner
            .validate_prepared(&request, &substituted_pair)
            .unwrap_err(),
        NativePrepareError::InvalidTransactionBytes
    );

    let mut instruction_substitution = initialization;
    instruction_substitution.message.instruction_data =
        Program::serialize_instruction(ZecEscrowInstruction::FundNative {
            swap_id: *request.terms.swap_id().as_bytes(),
        })
        .unwrap();
    instruction_substitution.witness_set =
        WitnessSet::for_message(&instruction_substitution.message, &[&key]);
    let mut substituted_pair = prepared.clone();
    substituted_pair.initialization = prepared_from_transaction(&instruction_substitution).unwrap();
    assert_eq!(
        planner
            .validate_prepared(&request, &substituted_pair)
            .unwrap_err(),
        NativePrepareError::InvalidTransactionBytes
    );

    let mut nonce_substitution = decode_prepared_for_signer(&prepared.funding, depositor).unwrap();
    nonce_substitution.message.nonces = vec![99_u128.into()];
    nonce_substitution.witness_set = WitnessSet::for_message(&nonce_substitution.message, &[&key]);
    let mut substituted_pair = prepared;
    substituted_pair.funding = prepared_from_transaction(&nonce_substitution).unwrap();
    assert_eq!(
        planner
            .validate_prepared(&request, &substituted_pair)
            .unwrap_err(),
        NativePrepareError::InvalidTransactionBytes
    );
}

#[test]
fn planner_debug_redacts_signing_key() {
    let (signer, key) = keyed_account(61);
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        [41; 8],
        [42; 8],
        runtime(Participant::Maker, signer, [41; 8]),
        Arc::new(FixedNonce {
            value: 0,
            calls: AtomicUsize::new(0),
        }),
    )
    .unwrap();
    let rendered = format!("{planner:?}");
    assert!(rendered.contains("[REDACTED]"));
    assert!(!rendered.contains(&"3d".repeat(32)));
}
