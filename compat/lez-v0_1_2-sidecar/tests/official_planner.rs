use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use common::transaction::NSSATransaction;
use lez_bridge_protocol::{
    Hex32, MessageContext, NativeEscrowTerms, NativeEscrowTermsInput, Participant,
    PrepareNativeEscrowRequest, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
};
use lez_v0_1_2_sidecar::{
    NativeEscrowPlanner, NonceSource, SidecarError, decode_prepared_for_role,
};
use lez_zec_escrow_compat::Instruction as EscrowInstruction;
use nssa::{AccountId, PrivateKey, PublicKey, PublicTransaction, program::Program};
use sha2::{Digest as _, Sha256};

#[derive(Debug)]
struct CountingNonceSource {
    calls: AtomicUsize,
    nonce: u128,
}

#[async_trait::async_trait]
impl NonceSource for CountingNonceSource {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, SidecarError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.nonce)
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

fn context(role: Participant) -> MessageContext {
    MessageContext::new(
        RunId::new("sidecar-run-0001").unwrap(),
        RequestId::new("prepare-request-0001").unwrap(),
        role,
    )
}

fn runtime(role: Participant, signer: AccountId, escrow_program: [u32; 8]) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::NssaV0_1_2,
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
) -> PrepareNativeEscrowRequest {
    let native_program = Program::authenticated_transfer_program().id();
    let runtime = runtime(role, signer, escrow_program);
    let terms = NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: h(4),
        terms_hash: h(5),
        secret_digest: Hex32::from_bytes(Sha256::digest([42_u8; 32]).into()),
        depositor: role,
        depositor_account_id: Hex32::from_bytes(signer.into_value()),
        claimant: match role {
            Participant::Maker => Participant::Taker,
            Participant::Taker => Participant::Maker,
        },
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        amount: u128::from(u64::MAX) + 91,
        refund_at_ms: 1_750_000_000_123,
        authenticated_transfer_program_id: program_hex(native_program),
    })
    .unwrap();
    PrepareNativeEscrowRequest::new(context(role), runtime, terms)
}

#[tokio::test]
async fn plans_official_native_pair_once_with_consecutive_nonces_and_exact_bytes() {
    let (depositor, key) = keyed_account(11);
    let (claimant, _) = keyed_account(12);
    let escrow_program = [0x1234_5678; 8];
    let nonces = Arc::new(CountingNonceSource {
        calls: AtomicUsize::new(0),
        nonce: 77,
    });
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        escrow_program,
        runtime(Participant::Maker, depositor, escrow_program),
        Arc::clone(&nonces),
    )
    .unwrap();
    let request = request(Participant::Maker, depositor, claimant, escrow_program);

    let prepared_pair = planner.prepare(request.clone()).await.unwrap();
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 1);

    let initialize = decode_prepared_for_role(
        &prepared_pair.initialization,
        Participant::Maker,
        Participant::Maker,
        depositor,
    )
    .unwrap();
    let funding = decode_prepared_for_role(
        &prepared_pair.funding,
        Participant::Maker,
        Participant::Maker,
        depositor,
    )
    .unwrap();

    assert_eq!(
        initialize.to_bytes(),
        prepared_pair.initialization.exact_bytes.as_slice()
    );
    assert_eq!(
        initialize.hash(),
        *prepared_pair.initialization.transaction_id.as_bytes()
    );
    assert_eq!(
        funding.to_bytes(),
        prepared_pair.funding.exact_bytes.as_slice()
    );
    assert_eq!(
        funding.hash(),
        *prepared_pair.funding.transaction_id.as_bytes()
    );
    assert_eq!(initialize.message.program_id, escrow_program);
    assert_eq!(funding.message.program_id, escrow_program);
    assert_eq!(initialize.message.nonces, vec![77_u128.into()]);
    assert_eq!(funding.message.nonces, vec![78_u128.into()]);

    let swap_id = *request.terms.swap_id().as_bytes();
    let metadata = spel_framework_core::pda::compute_pda(&escrow_program, &[&swap_id]);
    let custody_label = spel_framework_core::pda::seed_from_str("custody");
    let custody =
        spel_framework_core::pda::compute_pda(&escrow_program, &[&custody_label, &swap_id]);
    assert_eq!(
        initialize.message.account_ids,
        vec![metadata, custody, depositor, claimant]
    );
    assert_eq!(
        funding.message.account_ids,
        vec![metadata, custody, depositor]
    );
    assert_eq!(
        initialize.message.instruction_data,
        Program::serialize_instruction(EscrowInstruction::InitializeNative {
            swap_id,
            terms_hash: *request.terms.terms_hash().as_bytes(),
            secret_digest: *request.terms.secret_digest().as_bytes(),
            amount: request.terms.amount().as_u128(),
            refund_at: request.terms.refund_at_ms(),
            authenticated_transfer_program: Program::authenticated_transfer_program().id(),
        })
        .unwrap()
    );
    assert_eq!(
        funding.message.instruction_data,
        Program::serialize_instruction(EscrowInstruction::FundNative { swap_id }).unwrap()
    );

    let cached = planner.prepare(request).await.unwrap();
    assert_eq!(cached, prepared_pair);
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn independent_plans_preserve_randomized_bip340_signature_identity() {
    let (depositor, first_key) = keyed_account(51);
    let second_key = PrivateKey::try_new([51; 32]).unwrap();
    let (claimant, _) = keyed_account(52);
    let escrow_program = [23; 8];
    let request = request(Participant::Maker, depositor, claimant, escrow_program);
    let first = NativeEscrowPlanner::new(
        Participant::Maker,
        first_key,
        escrow_program,
        runtime(Participant::Maker, depositor, escrow_program),
        Arc::new(CountingNonceSource {
            calls: AtomicUsize::new(0),
            nonce: 100,
        }),
    )
    .unwrap()
    .prepare(request.clone())
    .await
    .unwrap();
    let second = NativeEscrowPlanner::new(
        Participant::Maker,
        second_key,
        escrow_program,
        runtime(Participant::Maker, depositor, escrow_program),
        Arc::new(CountingNonceSource {
            calls: AtomicUsize::new(0),
            nonce: 100,
        }),
    )
    .unwrap()
    .prepare(request)
    .await
    .unwrap();

    let first_transaction =
        PublicTransaction::from_bytes(first.initialization.exact_bytes.as_slice()).unwrap();
    let second_transaction =
        PublicTransaction::from_bytes(second.initialization.exact_bytes.as_slice()).unwrap();
    assert_eq!(first_transaction.message, second_transaction.message);
    assert_ne!(
        first.initialization.exact_bytes,
        second.initialization.exact_bytes
    );
    assert_ne!(
        first.initialization.transaction_id,
        second.initialization.transaction_id
    );
}

#[tokio::test]
async fn rejects_a_second_active_prepare_and_nonce_overflow() {
    let (depositor, key) = keyed_account(13);
    let (claimant, _) = keyed_account(14);
    let escrow_program = [9; 8];
    let nonces = Arc::new(CountingNonceSource {
        calls: AtomicUsize::new(0),
        nonce: 5,
    });
    let planner = NativeEscrowPlanner::new(
        Participant::Taker,
        key,
        escrow_program,
        runtime(Participant::Taker, depositor, escrow_program),
        Arc::clone(&nonces),
    )
    .unwrap();
    let _ = planner
        .prepare(request(
            Participant::Taker,
            depositor,
            claimant,
            escrow_program,
        ))
        .await
        .unwrap();

    let (other_claimant, _) = keyed_account(15);
    let error = planner
        .prepare(request(
            Participant::Taker,
            depositor,
            other_claimant,
            escrow_program,
        ))
        .await
        .unwrap_err();
    assert_eq!(error, SidecarError::ActivePrepare);

    let (overflow_depositor, overflow_key) = keyed_account(16);
    let overflow_source = Arc::new(CountingNonceSource {
        calls: AtomicUsize::new(0),
        nonce: u128::MAX,
    });
    let overflow_planner = NativeEscrowPlanner::new(
        Participant::Maker,
        overflow_key,
        escrow_program,
        runtime(Participant::Maker, overflow_depositor, escrow_program),
        overflow_source,
    )
    .unwrap();
    let error = overflow_planner
        .prepare(request(
            Participant::Maker,
            overflow_depositor,
            claimant,
            escrow_program,
        ))
        .await
        .unwrap_err();
    assert_eq!(error, SidecarError::NonceOverflow);
}

#[tokio::test]
async fn exact_codec_rejects_wrong_id_signature_signer_role_and_trailing_bytes() {
    let (depositor, key) = keyed_account(21);
    let (claimant, _) = keyed_account(22);
    let escrow_program = [7; 8];
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        escrow_program,
        runtime(Participant::Maker, depositor, escrow_program),
        Arc::new(CountingNonceSource {
            calls: AtomicUsize::new(0),
            nonce: 1,
        }),
    )
    .unwrap();
    let prepared_pair = planner
        .prepare(request(
            Participant::Maker,
            depositor,
            claimant,
            escrow_program,
        ))
        .await
        .unwrap();

    let mut wrong_id = prepared_pair.initialization.clone();
    wrong_id.transaction_id = lez_bridge_protocol::TransactionId::from_bytes([99; 32]);
    assert_eq!(
        decode_prepared_for_role(&wrong_id, Participant::Maker, Participant::Maker, depositor,)
            .unwrap_err(),
        SidecarError::WrongTransactionId
    );

    let mut invalid_signature_tx =
        PublicTransaction::from_bytes(prepared_pair.initialization.exact_bytes.as_slice()).unwrap();
    invalid_signature_tx.witness_set = nssa::public_transaction::WitnessSet::from_raw_parts(vec![]);
    let invalid_signature =
        lez_v0_1_2_sidecar::prepared_from_transaction(&invalid_signature_tx).unwrap();
    assert_eq!(
        decode_prepared_for_role(
            &invalid_signature,
            Participant::Maker,
            Participant::Maker,
            depositor,
        )
        .unwrap_err(),
        SidecarError::InvalidSignature
    );

    let (wrong_signer, _) = keyed_account(23);
    assert_eq!(
        decode_prepared_for_role(
            &prepared_pair.initialization,
            Participant::Maker,
            Participant::Maker,
            wrong_signer,
        )
        .unwrap_err(),
        SidecarError::WrongSigner
    );
    assert_eq!(
        decode_prepared_for_role(
            &prepared_pair.initialization,
            Participant::Taker,
            Participant::Maker,
            depositor,
        )
        .unwrap_err(),
        SidecarError::WrongSidecarRole
    );

    let mut trailing = prepared_pair.initialization.clone();
    let mut bytes = trailing.exact_bytes.as_slice().to_vec();
    bytes.push(0);
    trailing.exact_bytes = lez_bridge_protocol::ExactTransactionBytes::new(bytes).unwrap();
    assert_eq!(
        decode_prepared_for_role(&trailing, Participant::Maker, Participant::Maker, depositor,)
            .unwrap_err(),
        SidecarError::InvalidTransactionBytes
    );
}

#[tokio::test]
async fn submission_wraps_the_persisted_inner_transaction_without_resigning() {
    let (depositor, key) = keyed_account(31);
    let (claimant, _) = keyed_account(32);
    let escrow_program = [17; 8];
    let planner = NativeEscrowPlanner::new(
        Participant::Taker,
        key,
        escrow_program,
        runtime(Participant::Taker, depositor, escrow_program),
        Arc::new(CountingNonceSource {
            calls: AtomicUsize::new(0),
            nonce: 44,
        }),
    )
    .unwrap();
    let prepared_pair = planner
        .prepare(request(
            Participant::Taker,
            depositor,
            claimant,
            escrow_program,
        ))
        .await
        .unwrap();

    let wrapped = planner
        .decode_exact_for_submission(&prepared_pair.funding, Participant::Taker)
        .await
        .unwrap();
    let NSSATransaction::Public(decoded) = wrapped else {
        panic!("submission primitive must wrap a public transaction")
    };
    assert_eq!(
        decoded.to_bytes(),
        prepared_pair.funding.exact_bytes.as_slice()
    );
    assert_eq!(
        decoded.hash(),
        *prepared_pair.funding.transaction_id.as_bytes()
    );
}

#[test]
fn debug_output_redacts_private_key_and_preimage() {
    let (signer, key) = keyed_account(41);
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        [1; 8],
        runtime(Participant::Maker, signer, [1; 8]),
        Arc::new(CountingNonceSource {
            calls: AtomicUsize::new(0),
            nonce: 0,
        }),
    )
    .unwrap();
    let rendered = format!("{planner:?}");
    assert!(rendered.contains("[REDACTED]"));
    assert!(!rendered.contains(&"29".repeat(32)));

    let preimage = lez_bridge_protocol::RevealingPreimage::new([42; 32]);
    let rendered = format!("{preimage:?}");
    assert_eq!(rendered, "RevealingPreimage([REDACTED])");
    assert!(!rendered.contains(&"2a".repeat(32)));
}

#[tokio::test]
async fn rejects_cross_wired_runtime_and_valid_but_unprepared_transaction() {
    let (depositor, key) = keyed_account(61);
    let duplicate_key = PrivateKey::try_new([61; 32]).unwrap();
    let (claimant, _) = keyed_account(62);
    let escrow_program = [29; 8];
    let nonces = Arc::new(CountingNonceSource {
        calls: AtomicUsize::new(0),
        nonce: 200,
    });
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        escrow_program,
        runtime(Participant::Maker, depositor, escrow_program),
        Arc::clone(&nonces),
    )
    .unwrap();
    let valid_request = request(Participant::Maker, depositor, claimant, escrow_program);
    let cross_wired_runtime = RuntimeDescriptor::new(
        Participant::Maker,
        RuntimeCompatibility::NssaV0_1_2,
        h(91),
        h(2),
        h(3),
        program_hex(escrow_program),
        Hex32::from_bytes(depositor.into_value()),
    );
    let cross_wired = PrepareNativeEscrowRequest::new(
        valid_request.context.clone(),
        cross_wired_runtime,
        valid_request.terms.clone(),
    );
    assert_eq!(
        planner.prepare(cross_wired).await.unwrap_err(),
        SidecarError::WrongRuntimeIdentity
    );
    assert_eq!(nonces.calls.load(Ordering::SeqCst), 0);

    let prepared = planner.prepare(valid_request.clone()).await.unwrap();
    let independently_signed = NativeEscrowPlanner::new(
        Participant::Maker,
        duplicate_key,
        escrow_program,
        valid_request.runtime.clone(),
        Arc::new(CountingNonceSource {
            calls: AtomicUsize::new(0),
            nonce: 200,
        }),
    )
    .unwrap()
    .prepare(valid_request)
    .await
    .unwrap();
    assert_ne!(independently_signed.initialization, prepared.initialization);
    assert_eq!(
        planner
            .decode_exact_for_submission(&independently_signed.initialization, Participant::Maker,)
            .await
            .unwrap_err(),
        SidecarError::TransactionNotPrepared
    );
}
