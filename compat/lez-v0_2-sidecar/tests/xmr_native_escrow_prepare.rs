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
    ChainClock, CurrentProfileClockAccountSnapshot, Hex32, MessageContext, Participant,
    PrepareCurrentProfileClockRequest, PrepareNativeXmrClaimAuthorizationV3Request,
    PrepareNativeXmrEscrowV3Request, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
    XmrClaimPartialV3, XmrNativeEscrowTermsV3, XmrNativeEscrowTermsV3Input,
};
use lez_v0_2_sidecar::{
    NativeEscrowPlanner, NativePrepareError, NonceSource, ZecEscrowInstruction,
    compute_custody_pda, compute_metadata_pda, decode_prepared_for_signer,
    prepared_from_transaction, program_id_to_hex,
};
use nssa::{AccountId, PrivateKey, PublicKey, program::Program, public_transaction::WitnessSet};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

const ESCROW_PROGRAM: [u32; 8] = [0x1020_3040; 8];
const TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];
const SWAP_ID: [u8; 32] = [51; 32];
const CLAIM_PARTIAL_COMMITMENT_DOMAIN: &[u8] =
    b"logos.gateway.lez-xmr.claim-partial-commitment.v1\0";

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

fn claim_partial_commitment(context_binding: Hex32, claim_partial: [u8; 32]) -> Hex32 {
    let mut hasher = Sha256::new();
    hasher.update(CLAIM_PARTIAL_COMMITMENT_DOMAIN);
    hasher.update(context_binding.as_bytes());
    hasher.update(claim_partial);
    Hex32::from_bytes(hasher.finalize().into())
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
        claim_partial_commitment: claim_partial_commitment(h(15), [77; 32]),
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

fn authorization_request(
    runtime: RuntimeDescriptor,
    terms: &XmrNativeEscrowTermsV3,
    claim_partial: [u8; 32],
    request_id: &str,
) -> PrepareNativeXmrClaimAuthorizationV3Request {
    PrepareNativeXmrClaimAuthorizationV3Request::new(
        MessageContext::new(
            RunId::new("xmr-native-escrow-run").expect("run id"),
            RequestId::new(request_id).expect("request id"),
            Participant::Taker,
        ),
        runtime,
        *terms,
        XmrClaimPartialV3::new(claim_partial).expect("claim partial"),
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
            claim_partial_commitment: *xmr_terms.to_input().claim_partial_commitment.as_bytes(),
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
    let authorization_path = directory
        .path()
        .join("xmr-native-claim-authorization-reservation.v3.json");
    let wrong_partial = authorization_request(
        descriptor.clone(),
        &xmr_terms,
        [78; 32],
        "xmr-native-authorization-wrong-partial",
    );
    assert_eq!(
        planner
            .prepare_native_xmr_claim_authorization_v3(&wrong_partial)
            .await
            .expect_err("uncommitted partial"),
        NativePrepareError::WrongXmrClaimPartial
    );
    assert!(!authorization_path.exists());
    assert_eq!(
        hex::encode(xmr_terms.to_input().claim_partial_commitment.as_bytes()),
        "694ebef5494cb2e3f1732e1d56cc73c469c32a2257a572df9e7c0e69060b8c4a"
    );
    let claim_authorization_request = authorization_request(
        descriptor.clone(),
        &xmr_terms,
        [77; 32],
        "xmr-native-authorization",
    );
    let prepared_authorization = planner
        .prepare_native_xmr_claim_authorization_v3(&claim_authorization_request)
        .await
        .expect("exact durable authorization");
    assert_eq!(first_nonce.calls.load(Ordering::SeqCst), 1);
    assert!(authorization_path.is_file());
    planner
        .validate_prepared_native_xmr_claim_authorization_v3(
            &claim_authorization_request,
            &prepared_authorization,
        )
        .expect("self-validating authorization");
    let authorization =
        decode_prepared_for_signer(&prepared_authorization.authorization, depositor)
            .expect("authorization");
    assert_eq!(authorization.message().program_id, ESCROW_PROGRAM);
    assert_eq!(
        authorization.message().account_ids,
        vec![compute_metadata_pda(&ESCROW_PROGRAM, &SWAP_ID), depositor,]
    );
    assert_eq!(authorization.message().nonces, vec![43_u128.into()]);
    assert_eq!(
        authorization.message().instruction_data,
        Program::serialize_instruction(ZecEscrowInstruction::AuthorizeNativeXmrClaim {
            swap_id: SWAP_ID,
            claim_partial: [77; 32],
        })
        .expect("authorization instruction encoding")
    );
    assert_eq!(
        planner
            .prepare_native_xmr_claim_authorization_v3(&claim_authorization_request)
            .await
            .expect("same-process exact replay"),
        prepared_authorization
    );
    assert_eq!(first_nonce.calls.load(Ordering::SeqCst), 1);

    for index in 0..3 {
        let mut changed = authorization.clone();
        match index {
            0 => changed.message.account_ids.swap(0, 1),
            1 => changed.message.instruction_data.push(0xff),
            _ => changed.message.nonces = vec![99_u128.into()],
        }
        changed.witness_set = WitnessSet::for_message(&changed.message, &[&depositor_key]);
        let mut drifted = prepared_authorization.clone();
        drifted.authorization = prepared_from_transaction(&changed).expect("authorization");
        assert_eq!(
            planner
                .validate_prepared_native_xmr_claim_authorization_v3(
                    &claim_authorization_request,
                    &drifted,
                )
                .expect_err("authorization ABI drift"),
            NativePrepareError::InvalidTransactionBytes
        );
    }
    let conflicting_authorization = authorization_request(
        descriptor.clone(),
        &xmr_terms,
        [77; 32],
        "xmr-native-authorization-conflict",
    );
    assert_eq!(
        planner
            .prepare_native_xmr_claim_authorization_v3(&conflicting_authorization)
            .await
            .expect_err("distinct request cannot reuse reserved nonce"),
        NativePrepareError::ActiveXmrClaimAuthorizationPrepare
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
    assert_eq!(
        restarted
            .prepare_native_xmr_claim_authorization_v3(&claim_authorization_request)
            .await
            .expect("byte-identical authorization recovery"),
        prepared_authorization
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
    fs::remove_file(&authorization_path).expect("delete authorization reservation");
    assert!(matches!(
        restarted
            .prepare_native_xmr_claim_authorization_v3(&claim_authorization_request)
            .await
            .expect_err("active replay requires its durable authorization"),
        NativePrepareError::DurableReservation(_)
    ));
}

#[tokio::test]
async fn authorization_requires_durable_escrow_and_reserves_nothing_on_nonce_overflow() {
    let (depositor, depositor_key, _) = account(31);
    let (claimant, _, _) = account(32);
    let (claim_authority, _, claim_key) = account(33);
    let (refund_authority, _, refund_key) = account(34);
    let descriptor = runtime(depositor);
    let xmr_terms = terms(
        depositor,
        claimant,
        claim_authority,
        &claim_key,
        refund_authority,
        &refund_key,
        85,
    );
    let claim_authorization_request = authorization_request(
        descriptor.clone(),
        &xmr_terms,
        [77; 32],
        "xmr-native-authorization-prerequisite",
    );

    let missing_directory = private_directory();
    let missing_nonce = Arc::new(CountingNonce {
        value: 41,
        calls: AtomicUsize::new(0),
    });
    let missing_planner = NativeEscrowPlanner::new_durable(
        Participant::Taker,
        depositor_key.clone(),
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        descriptor.clone(),
        Arc::clone(&missing_nonce),
        missing_directory.path(),
    )
    .expect("missing-prerequisite planner");
    assert_eq!(
        missing_planner
            .prepare_native_xmr_claim_authorization_v3(&claim_authorization_request)
            .await
            .expect_err("authorization requires exact durable Fund reservation"),
        NativePrepareError::InvalidTransactionBytes
    );
    assert_eq!(missing_nonce.calls.load(Ordering::SeqCst), 0);
    assert!(
        !missing_directory
            .path()
            .join("xmr-native-claim-authorization-reservation.v3.json")
            .exists()
    );

    let overflow_directory = private_directory();
    let overflow_nonce = Arc::new(CountingNonce {
        value: u128::MAX - 1,
        calls: AtomicUsize::new(0),
    });
    let overflow_planner = NativeEscrowPlanner::new_durable(
        Participant::Taker,
        depositor_key,
        ESCROW_PROGRAM,
        TRANSFER_PROGRAM,
        descriptor.clone(),
        Arc::clone(&overflow_nonce),
        overflow_directory.path(),
    )
    .expect("overflow planner");
    let _ = overflow_planner
        .prepare_native_xmr_escrow_v3(&request(descriptor, &xmr_terms))
        .await
        .expect("MAX-1 initialization and MAX funding remain valid");
    assert_eq!(
        overflow_planner
            .prepare_native_xmr_claim_authorization_v3(&claim_authorization_request)
            .await
            .expect_err("authorization cannot increment MAX Fund nonce"),
        NativePrepareError::NonceOverflow
    );
    assert_eq!(overflow_nonce.calls.load(Ordering::SeqCst), 1);
    assert!(
        !overflow_directory
            .path()
            .join("xmr-native-claim-authorization-reservation.v3.json")
            .exists()
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one concurrent create/replay/reject journey shares exact durable fixtures"
)]
fn current_profile_clock_reservation_is_create_once_under_concurrency() {
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
    let input = xmr_terms.to_input();
    let request = PrepareCurrentProfileClockRequest::new(
        MessageContext::new(
            RunId::new("xmr-native-escrow-run").expect("run id"),
            RequestId::new("xmr-current-clock-prepare").expect("request id"),
            Participant::Taker,
        ),
        descriptor.clone(),
        xmr_terms,
        input.claimant_account_id,
        input.punish_at_ms,
    );
    let directory = private_directory();
    let planner = Arc::new(
        NativeEscrowPlanner::new_durable(
            Participant::Taker,
            depositor_key,
            ESCROW_PROGRAM,
            TRANSFER_PROGRAM,
            descriptor,
            Arc::new(CountingNonce {
                value: 17,
                calls: AtomicUsize::new(0),
            }),
            directory.path(),
        )
        .expect("planner"),
    );
    let clock = ChainClock::new(h(30), 5, 1_000);
    let sender = CurrentProfileClockAccountSnapshot::new(
        input.depositor_account_id,
        100,
        17,
        input.authenticated_transfer_program_id,
        h(31),
    );
    let recipient = CurrentProfileClockAccountSnapshot::new(
        input.claimant_account_id,
        25,
        3,
        input.authenticated_transfer_program_id,
        h(32),
    );
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let planner = Arc::clone(&planner);
                let request = request.clone();
                scope.spawn(move || {
                    planner.prepare_current_profile_clock(
                        &request,
                        sender.nonce,
                        clock,
                        sender,
                        recipient,
                        h(33),
                        h(34),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("thread").expect("same reservation"))
            .collect::<Vec<_>>()
    });
    assert!(results.windows(2).all(|pair| pair[0] == pair[1]));
    assert!(
        directory
            .path()
            .join("xmr-current-profile-clock.v1.json")
            .is_file()
    );

    let mut distinct = request;
    distinct.context.request_id = RequestId::new("xmr-current-clock-second").expect("request id");
    assert_eq!(
        planner
            .prepare_current_profile_clock(
                &distinct,
                sender.nonce,
                clock,
                sender,
                recipient,
                h(33),
                h(34),
            )
            .expect_err("a second reservation must fail closed"),
        NativePrepareError::ActivePrepare
    );
}
