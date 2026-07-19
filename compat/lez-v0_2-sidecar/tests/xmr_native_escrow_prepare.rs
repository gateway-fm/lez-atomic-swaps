#![cfg(target_os = "linux")]

use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use lez_bridge_protocol::{
    Hex32, MessageContext, Participant, PrepareNativeXmrEscrowV3Request, RequestId, RunId,
    RuntimeCompatibility, RuntimeDescriptor, XmrNativeEscrowTermsV3, XmrNativeEscrowTermsV3Input,
};
use lez_v0_2_sidecar::{
    NativeEscrowPlanner, NativePrepareError, NonceSource, ZecEscrowInstruction,
    compute_custody_pda, compute_metadata_pda, decode_prepared_for_signer,
    prepared_from_transaction, program_id_to_hex,
};
use nssa::{AccountId, PrivateKey, PublicKey, program::Program, public_transaction::WitnessSet};
use tempfile::TempDir;

const ESCROW_PROGRAM: [u32; 8] = [0x1020_3040; 8];
const TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];
const SWAP_ID: [u8; 32] = [51; 32];

#[derive(Debug)]
struct CountingNonce {
    value: u128,
    calls: AtomicUsize,
}

#[async_trait]
impl NonceSource for CountingNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.value)
    }
}

fn account(byte: u8) -> (AccountId, PrivateKey, PublicKey) {
    let key = PrivateKey::try_new([byte; 32]).expect("valid private key");
    let public = PublicKey::new_from_private_key(&key);
    (AccountId::from(&public), key, public)
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn runtime(signer: AccountId) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        Participant::Taker,
        RuntimeCompatibility::LeeV0_2_0,
        h(40),
        h(41),
        h(42),
        program_id_to_hex(ESCROW_PROGRAM),
        Hex32::from_bytes(signer.into_value()),
    )
}

fn terms(
    depositor: AccountId,
    claimant: AccountId,
    claim_authority: AccountId,
    claim_key: &PublicKey,
    refund_authority: AccountId,
    refund_key: &PublicKey,
    amount: u128,
) -> XmrNativeEscrowTermsV3 {
    XmrNativeEscrowTermsV3::new(XmrNativeEscrowTermsV3Input {
        swap_id: Hex32::from_bytes(SWAP_ID),
        activation_commitment: h(2),
        escrow_program_id: program_id_to_hex(ESCROW_PROGRAM),
        authenticated_transfer_program_id: program_id_to_hex(TRANSFER_PROGRAM),
        metadata_account_id: Hex32::from_bytes(
            compute_metadata_pda(&ESCROW_PROGRAM, &SWAP_ID).into_value(),
        ),
        custody_account_id: Hex32::from_bytes(
            compute_custody_pda(&ESCROW_PROGRAM, &SWAP_ID).into_value(),
        ),
        depositor: Participant::Taker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Maker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        claim_aggregate_x_only_public_key: Hex32::from_bytes(*claim_key.value()),
        claim_authority_account_id: Hex32::from_bytes(claim_authority.into_value()),
        refund_aggregate_x_only_public_key: Hex32::from_bytes(*refund_key.value()),
        refund_authority_account_id: Hex32::from_bytes(refund_authority.into_value()),
        maker_dleq_transcript_commitment: h(13),
        taker_dleq_transcript_commitment: h(14),
        claim_partial_context_binding: h(15),
        claim_partial_commitment: h(16),
        amount,
        refund_at_ms: 10_000,
        punish_at_ms: 20_000,
        claim_message_hash: h(17),
        refund_message_hash: h(18),
        punish_message_hash: h(19),
    })
    .expect("valid XMR terms")
}

fn request(
    runtime: RuntimeDescriptor,
    terms: &XmrNativeEscrowTermsV3,
) -> PrepareNativeXmrEscrowV3Request {
    PrepareNativeXmrEscrowV3Request::new(
        MessageContext::new(
            RunId::new("xmr-native-escrow-run").expect("run id"),
            RequestId::new("xmr-native-escrow-prepare").expect("request id"),
            Participant::Taker,
        ),
        runtime,
        *terms,
    )
}

fn private_directory() -> TempDir {
    let directory = TempDir::new().expect("temporary directory");
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
        .expect("owner-only directory");
    directory
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one exact prepare/restart journey keeps every drift assertion bound to the same durable bytes"
)]
async fn xmr_native_pair_is_exact_durable_and_recovered_without_nonce_or_submission() {
    let (depositor, depositor_key, _) = account(21);
    let (claimant, _, _) = account(22);
    let (claim_authority, _, claim_key) = account(23);
    let (refund_authority, _, refund_key) = account(24);
    let descriptor = runtime(depositor);
    let xmr_terms = terms(
        depositor,
        claimant,
        claim_authority,
        &claim_key,
        refund_authority,
        &refund_key,
        75,
    );
    let prepare_request = request(descriptor.clone(), &xmr_terms);
    let directory = private_directory();
    let first_nonce = Arc::new(CountingNonce {
        value: 41,
        calls: AtomicUsize::new(0),
    });
    let planner = NativeEscrowPlanner::new_durable(
        Participant::Taker,
        depositor_key.clone(),
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        descriptor.clone(),
        Arc::clone(&first_nonce),
        directory.path(),
    )
    .expect("planner");

    let prepared = planner
        .prepare_native_xmr_escrow_v3(&prepare_request)
        .await
        .expect("exact pair");
    assert_eq!(first_nonce.calls.load(Ordering::SeqCst), 1);
    planner
        .validate_prepared_native_xmr_escrow_v3(&prepare_request, &prepared)
        .expect("self-validating pair");

    let initialization =
        decode_prepared_for_signer(&prepared.initialization, depositor).expect("initialization");
    let funding = decode_prepared_for_signer(&prepared.funding, depositor).expect("funding");
    assert_eq!(initialization.message().nonces, vec![41_u128.into()]);
    assert_eq!(funding.message().nonces, vec![42_u128.into()]);
    assert_eq!(
        initialization.message().account_ids,
        vec![
            compute_metadata_pda(&ESCROW_PROGRAM, &SWAP_ID),
            compute_custody_pda(&ESCROW_PROGRAM, &SWAP_ID),
            depositor,
            claimant,
            claim_authority,
            refund_authority,
        ]
    );
    assert_eq!(
        funding.message().account_ids,
        vec![
            compute_metadata_pda(&ESCROW_PROGRAM, &SWAP_ID),
            compute_custody_pda(&ESCROW_PROGRAM, &SWAP_ID),
            depositor,
        ]
    );
    assert_eq!(
        initialization.message().instruction_data,
        Program::serialize_instruction(ZecEscrowInstruction::InitializeNativeXmr {
            swap_id: SWAP_ID,
            terms_hash: *xmr_terms.to_input().activation_commitment.as_bytes(),
            claim_aggregate_x_only_public_key: *claim_key.value(),
            refund_aggregate_x_only_public_key: *refund_key.value(),
            maker_dleq_transcript_commitment: [13; 32],
            taker_dleq_transcript_commitment: [14; 32],
            claim_partial_context_binding: [15; 32],
            claim_partial_commitment: [16; 32],
            amount: 75,
            refund_at: 10_000,
            punish_at: 20_000,
            authenticated_transfer_program: TRANSFER_PROGRAM,
        })
        .expect("instruction encoding")
    );
    assert_eq!(
        funding.message().instruction_data,
        Program::serialize_instruction(ZecEscrowInstruction::FundNative { swap_id: SWAP_ID })
            .expect("instruction encoding")
    );
    let mut wrong_runtime = prepare_request.clone();
    wrong_runtime.runtime.signer_account_id = h(99);
    assert_eq!(
        planner
            .prepare_native_xmr_escrow_v3(&wrong_runtime)
            .await
            .expect_err("runtime drift"),
        NativePrepareError::WrongRuntime
    );
    let mut wrong_account_terms = xmr_terms.to_input();
    wrong_account_terms.metadata_account_id = h(99);
    let wrong_account_request = request(
        descriptor.clone(),
        &XmrNativeEscrowTermsV3::new(wrong_account_terms).expect("structurally valid drift"),
    );
    assert_eq!(
        planner
            .prepare_native_xmr_escrow_v3(&wrong_account_request)
            .await
            .expect_err("PDA drift"),
        NativePrepareError::InvalidTransactionBytes
    );
    assert_eq!(first_nonce.calls.load(Ordering::SeqCst), 1);

    for (index, mut changed) in [initialization.clone(), initialization.clone()]
        .into_iter()
        .enumerate()
    {
        if index == 0 {
            changed.message.account_ids.swap(0, 1);
        } else {
            changed.message.instruction_data.push(0xff);
        }
        changed.witness_set = WitnessSet::for_message(&changed.message, &[&depositor_key]);
        let mut drifted_pair = prepared.clone();
        drifted_pair.initialization = prepared_from_transaction(&changed).expect("pair");
        assert_eq!(
            planner
                .validate_prepared_native_xmr_escrow_v3(&prepare_request, &drifted_pair)
                .expect_err("account or instruction drift"),
            NativePrepareError::InvalidTransactionBytes
        );
    }

    let mut nonce_substitution = funding.clone();
    nonce_substitution.message.nonces = vec![99_u128.into()];
    nonce_substitution.witness_set =
        WitnessSet::for_message(&nonce_substitution.message, &[&depositor_key]);
    let mut drifted_pair = prepared.clone();
    drifted_pair.funding = prepared_from_transaction(&nonce_substitution).expect("pair");
    assert_eq!(
        planner
            .validate_prepared_native_xmr_escrow_v3(&prepare_request, &drifted_pair)
            .expect_err("nonce drift"),
        NativePrepareError::InvalidTransactionBytes
    );

    let (_, wrong_key, _) = account(25);
    let mut signer_substitution = initialization.clone();
    signer_substitution.witness_set =
        WitnessSet::for_message(&signer_substitution.message, &[&wrong_key]);
    let mut drifted_pair = prepared.clone();
    drifted_pair.initialization = prepared_from_transaction(&signer_substitution).expect("pair");
    assert_eq!(
        planner
            .validate_prepared_native_xmr_escrow_v3(&prepare_request, &drifted_pair)
            .expect_err("signer-witness drift"),
        NativePrepareError::WrongSigner
    );
    drop(planner);

    let (_, restart_key, _) = account(21);
    let restart_nonce = Arc::new(CountingNonce {
        value: 999,
        calls: AtomicUsize::new(0),
    });
    let restarted = NativeEscrowPlanner::new_durable(
        Participant::Taker,
        restart_key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        descriptor.clone(),
        Arc::clone(&restart_nonce),
        directory.path(),
    )
    .expect("restarted planner");
    assert_eq!(
        restarted
            .prepare_native_xmr_escrow_v3(&prepare_request)
            .await
            .expect("byte-identical recovery"),
        prepared
    );
    assert_eq!(restart_nonce.calls.load(Ordering::SeqCst), 0);

    let drifted = request(
        descriptor,
        &terms(
            depositor,
            claimant,
            claim_authority,
            &claim_key,
            refund_authority,
            &refund_key,
            76,
        ),
    );
    assert_eq!(
        restarted
            .prepare_native_xmr_escrow_v3(&drifted)
            .await
            .expect_err("terms drift fails closed"),
        NativePrepareError::ActivePrepare
    );
}
