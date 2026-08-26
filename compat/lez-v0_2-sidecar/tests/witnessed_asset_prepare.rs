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
    AggregateBip340Signature, CompleteWitnessedAssetClaimV2Request, Hex32, MessageContext,
    Participant, PrepareWitnessedAssetClaimV2Request, PrepareWitnessedAssetEscrowV2Request,
    PrepareWitnessedAssetRefundV2Request, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor, TransactionId, WitnessedAssetPrepareStepV2, WitnessedLezAssetTermsV2,
    WitnessedTokenEscrowTermsV2, WitnessedTokenEscrowTermsV2Input,
};
use lez_v0_2_sidecar::{
    NativeEscrowPlanner, NativePrepareError, NonceSource, ZecEscrowInstruction,
    compute_metadata_pda, decode_prepared_for_signer, prepared_from_transaction, program_id_to_hex,
};
use nssa::{
    AccountId, PrivateKey, PublicKey, PublicTransaction, Signature,
    program::Program,
    public_transaction::{Message, WitnessSet},
};

const ESCROW_PROGRAM: [u32; 8] = [0x1020_3040; 8];
const LEGACY_TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];

#[derive(Debug)]
struct ExactNonce {
    expected: AccountId,
    value: u128,
    calls: AtomicUsize,
}

#[async_trait]
impl NonceSource for ExactNonce {
    async fn account_nonce(&self, account_id: AccountId) -> Result<u128, NativePrepareError> {
        if account_id != self.expected {
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

fn ata(owner: AccountId, definition: AccountId) -> AccountId {
    let ata_program = programs::ata().id();
    let seed = ata_core::compute_ata_seed(owner, definition);
    ata_core::get_associated_token_account_id(&ata_program, &seed)
}

fn runtime(role: Participant, signer: AccountId) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::LeeV0_2_0,
        h(1),
        h(2),
        h(3),
        program_id_to_hex(ESCROW_PROGRAM),
        Hex32::from_bytes(signer.into_value()),
    )
}

struct Fixture {
    role: Participant,
    depositor: AccountId,
    depositor_key: PrivateKey,
    claimant: AccountId,
    claimant_key: PrivateKey,
    authority: AccountId,
    authority_key: PrivateKey,
    authority_public: PublicKey,
    definition: AccountId,
    request: PrepareWitnessedAssetEscrowV2Request,
}

fn fixture(role: Participant, definition_byte: u8, request_suffix: &str) -> Fixture {
    let (depositor, depositor_key, _) = account(definition_byte.wrapping_add(1));
    let (claimant, claimant_key, _) = account(definition_byte.wrapping_add(2));
    let (authority, authority_key, authority_public) = account(definition_byte.wrapping_add(3));
    let (definition, _, _) = account(definition_byte);
    let claimant_role = match role {
        Participant::Maker => Participant::Taker,
        Participant::Taker => Participant::Maker,
    };
    let swap_id = Hex32::from_bytes([definition_byte.wrapping_add(4); 32]);
    let metadata = compute_metadata_pda(&ESCROW_PROGRAM, swap_id.as_bytes());
    let terms = WitnessedTokenEscrowTermsV2::new(WitnessedTokenEscrowTermsV2Input {
        swap_id,
        terms_hash: h(definition_byte.wrapping_add(5)),
        depositor: role,
        depositor_owner_account_id: Hex32::from_bytes(depositor.into_value()),
        depositor_ata_account_id: Hex32::from_bytes(ata(depositor, definition).into_value()),
        claimant: claimant_role,
        claimant_owner_account_id: Hex32::from_bytes(claimant.into_value()),
        claimant_ata_account_id: Hex32::from_bytes(ata(claimant, definition).into_value()),
        custody_ata_account_id: Hex32::from_bytes(ata(metadata, definition).into_value()),
        token_program_id: program_id_to_hex(programs::token().id()),
        ata_program_id: program_id_to_hex(programs::ata().id()),
        token_definition_account_id: Hex32::from_bytes(definition.into_value()),
        aggregate_authority_account_id: Hex32::from_bytes(authority.into_value()),
        aggregate_x_only_public_key: Hex32::from_bytes(*authority_public.value()),
        amount: 75,
        refund_at_ms: 1_850_000_000_123,
    })
    .unwrap();
    let request = PrepareWitnessedAssetEscrowV2Request::new(
        MessageContext::new(
            RunId::new("asset-token-run-0001").unwrap(),
            RequestId::new(format!("asset-token-prepare-{request_suffix}")).unwrap(),
            role,
        ),
        runtime(role, depositor),
        WitnessedLezAssetTermsV2::custom_token(terms),
    );
    Fixture {
        role,
        depositor,
        depositor_key,
        claimant,
        claimant_key,
        authority,
        authority_key,
        authority_public,
        definition,
        request,
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one corridor keeps exact ordered account, signer, nonce, tag, and replay assertions together"
)]
#[tokio::test]
async fn exact_token_plan_uses_official_programs_atas_order_signers_and_nonce_continuity() {
    for (role, definition_byte, suffix) in [
        (Participant::Maker, 31, "maker"),
        (Participant::Taker, 41, "taker"),
    ] {
        let fixture = fixture(role, definition_byte, suffix);
        let nonce = Arc::new(ExactNonce {
            expected: fixture.depositor,
            value: 91,
            calls: AtomicUsize::new(0),
        });
        let planner = NativeEscrowPlanner::new(
            fixture.role,
            fixture.depositor_key,
            ESCROW_PROGRAM,
            LEGACY_TRANSFER_PROGRAM,
            fixture.request.runtime.clone(),
            Arc::clone(&nonce),
        )
        .unwrap();

        let prepared = planner
            .prepare_witnessed_asset_escrow_v2(&fixture.request)
            .await
            .unwrap();
        planner
            .validate_prepared_witnessed_asset_escrow_v2(&fixture.request, &prepared)
            .unwrap();
        assert_eq!(nonce.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            prepared
                .effects
                .iter()
                .map(|effect| effect.step)
                .collect::<Vec<_>>(),
            [
                WitnessedAssetPrepareStepV2::InitializeWitnessed,
                WitnessedAssetPrepareStepV2::CreateCustodyAta,
                WitnessedAssetPrepareStepV2::Fund,
            ]
        );

        let initialization =
            decode_prepared_for_signer(&prepared.effects[0].transaction, fixture.depositor)
                .unwrap();
        let custody =
            PublicTransaction::from_bytes(prepared.effects[1].transaction.exact_bytes.as_slice())
                .unwrap();
        let funding =
            decode_prepared_for_signer(&prepared.effects[2].transaction, fixture.depositor)
                .unwrap();
        let token_terms = fixture.request.terms.asset().custom_token().unwrap();
        let metadata = compute_metadata_pda(&ESCROW_PROGRAM, token_terms.swap_id().as_bytes());
        let custody_ata = ata(metadata, fixture.definition);
        let depositor_ata = ata(fixture.depositor, fixture.definition);

        assert_eq!(initialization.message().nonces, vec![91_u128.into()]);
        assert!(custody.message().nonces.is_empty());
        assert_eq!(funding.message().nonces, vec![92_u128.into()]);
        assert_eq!(
            initialization.message().account_ids,
            [
                metadata,
                fixture.depositor,
                fixture.claimant,
                fixture.definition,
                fixture.authority,
            ]
        );
        assert_eq!(
            custody.message().account_ids,
            [metadata, fixture.definition, custody_ata]
        );
        assert!(
            custody
                .witness_set()
                .signatures_and_public_keys()
                .is_empty()
        );
        assert_eq!(
            funding.message().account_ids,
            [metadata, fixture.depositor, depositor_ata, custody_ata]
        );
        assert_eq!(
            initialization.message().instruction_data,
            Program::serialize_instruction(ZecEscrowInstruction::InitializeTokenWitnessed {
                swap_id: *token_terms.swap_id().as_bytes(),
                terms_hash: *token_terms.terms_hash().as_bytes(),
                aggregate_x_only_public_key: *fixture.authority_public.value(),
                amount: token_terms.amount().as_u128(),
                refund_at: token_terms.refund_at_ms(),
                ata_program: programs::ata().id(),
            })
            .unwrap()
        );
        assert_eq!(initialization.message().instruction_data[0], 11);
        assert_eq!(custody.message().instruction_data[0], 7);
        assert_eq!(funding.message().instruction_data[0], 8);

        for effect in &prepared.effects {
            planner
                .validate_owned_submission(&effect.transaction)
                .await
                .unwrap();
        }
        assert_eq!(
            planner
                .prepare_witnessed_asset_escrow_v2(&fixture.request)
                .await
                .unwrap(),
            prepared
        );
        assert_eq!(nonce.calls.load(Ordering::SeqCst), 1);
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one corridor keeps transcript, aggregate completion, replay, and conflict assertions together"
)]
#[tokio::test]
async fn exact_token_claim_transcript_completes_with_only_the_aggregate_authority() {
    let fixture = fixture(Participant::Maker, 61, "claim");
    let terms = fixture.request.terms.clone();
    let token_terms = terms.asset().custom_token().unwrap();
    let claimant_role = token_terms.claimant();
    let claimant_runtime = runtime(claimant_role, fixture.claimant);
    let request = PrepareWitnessedAssetClaimV2Request::new(
        MessageContext::new(
            RunId::new("asset-token-run-0001").unwrap(),
            RequestId::new("asset-token-claim-prepare").unwrap(),
            claimant_role,
        ),
        claimant_runtime.clone(),
        terms.clone(),
        TransactionId::from_bytes([77; 32]),
    );
    let nonce = Arc::new(ExactNonce {
        expected: fixture.authority,
        value: 111,
        calls: AtomicUsize::new(0),
    });
    let planner = NativeEscrowPlanner::new(
        claimant_role,
        fixture.claimant_key,
        ESCROW_PROGRAM,
        LEGACY_TRANSFER_PROGRAM,
        claimant_runtime.clone(),
        Arc::clone(&nonce),
    )
    .unwrap();

    let prepared = planner
        .prepare_witnessed_asset_claim_v2(&request)
        .await
        .unwrap();
    planner
        .validate_prepared_witnessed_asset_claim_v2(&request, &prepared)
        .unwrap();
    assert_eq!(nonce.calls.load(Ordering::SeqCst), 1);
    let message = Message::try_from_slice(prepared.claim.exact_message_bytes.as_slice()).unwrap();
    let metadata = compute_metadata_pda(&ESCROW_PROGRAM, token_terms.swap_id().as_bytes());
    assert_eq!(message.nonces, [111_u128.into()]);
    assert_eq!(
        message.account_ids,
        [
            metadata,
            ata(metadata, fixture.definition),
            fixture.claimant,
            ata(fixture.claimant, fixture.definition),
            fixture.authority,
        ]
    );
    assert_eq!(message.instruction_data[0], 12);
    assert_eq!(
        message.instruction_data,
        Program::serialize_instruction(ZecEscrowInstruction::ClaimTokenWitnessed {
            swap_id: *token_terms.swap_id().as_bytes(),
        })
        .unwrap()
    );

    let signature = Signature::new(&fixture.authority_key, &message.hash());
    let completion = CompleteWitnessedAssetClaimV2Request::new(
        MessageContext::new(
            request.context.run_id.clone(),
            RequestId::new("asset-token-claim-complete").unwrap(),
            claimant_role,
        ),
        claimant_runtime,
        terms,
        prepared.claim.clone(),
        AggregateBip340Signature::from_bytes(signature.value),
    );
    let completed = planner
        .complete_witnessed_asset_claim_v2(&completion)
        .await
        .unwrap();
    let transaction = decode_prepared_for_signer(&completed.claim, fixture.authority).unwrap();
    assert_eq!(transaction.message(), &message);
    assert_eq!(
        transaction.witness_set().signatures_and_public_keys()[0].1,
        fixture.authority_public
    );
    planner
        .validate_owned_submission(&completed.claim)
        .await
        .unwrap();
    assert_eq!(
        planner
            .prepare_witnessed_asset_claim_v2(&request)
            .await
            .unwrap(),
        prepared
    );
    assert_eq!(
        planner
            .complete_witnessed_asset_claim_v2(&completion)
            .await
            .unwrap(),
        completed
    );
    let mut conflicting_prepare = request.clone();
    conflicting_prepare.context.request_id =
        RequestId::new("asset-token-claim-prepare-conflict").unwrap();
    assert_eq!(
        planner
            .prepare_witnessed_asset_claim_v2(&conflicting_prepare)
            .await
            .unwrap_err(),
        NativePrepareError::ActiveWitnessedAssetClaimPrepare
    );
    let mut conflicting_completion = completion.clone();
    conflicting_completion.context.request_id =
        RequestId::new("asset-token-claim-complete-conflict").unwrap();
    conflicting_completion.aggregate_signature = AggregateBip340Signature::from_bytes([7; 64]);
    assert_eq!(
        planner
            .complete_witnessed_asset_claim_v2(&conflicting_completion)
            .await
            .unwrap_err(),
        NativePrepareError::ActiveWitnessedAssetClaimCompletion
    );
    assert_eq!(nonce.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn exact_token_refund_is_permissionless_fixed_destination_and_never_reads_nonce() {
    let fixture = fixture(Participant::Taker, 71, "refund");
    let terms = fixture.request.terms.clone();
    let swap_id = terms.asset().custom_token().unwrap().swap_id();
    let request = PrepareWitnessedAssetRefundV2Request::new(
        MessageContext::new(
            RunId::new("asset-token-run-0001").unwrap(),
            RequestId::new("asset-token-refund-prepare").unwrap(),
            fixture.role,
        ),
        fixture.request.runtime.clone(),
        terms,
    );
    let nonce = Arc::new(ExactNonce {
        expected: fixture.depositor,
        value: 121,
        calls: AtomicUsize::new(0),
    });
    let planner = NativeEscrowPlanner::new(
        fixture.role,
        fixture.depositor_key,
        ESCROW_PROGRAM,
        LEGACY_TRANSFER_PROGRAM,
        fixture.request.runtime.clone(),
        Arc::clone(&nonce),
    )
    .unwrap();

    let prepared = planner
        .prepare_witnessed_asset_refund_v2(&request)
        .await
        .unwrap();
    planner
        .validate_prepared_witnessed_asset_refund_v2(&request, &prepared)
        .unwrap();
    let transaction =
        PublicTransaction::from_bytes(prepared.refund.exact_bytes.as_slice()).unwrap();
    let metadata = compute_metadata_pda(&ESCROW_PROGRAM, swap_id.as_bytes());
    assert!(transaction.message().nonces.is_empty());
    assert!(
        transaction
            .witness_set()
            .signatures_and_public_keys()
            .is_empty()
    );
    assert_eq!(
        transaction.message().account_ids,
        [
            metadata,
            ata(metadata, fixture.definition),
            ata(fixture.depositor, fixture.definition),
        ]
    );
    assert_eq!(transaction.message().instruction_data[0], 10);
    assert_eq!(nonce.calls.load(Ordering::SeqCst), 0);
    planner
        .validate_owned_submission(&prepared.refund)
        .await
        .unwrap();
    assert_eq!(
        planner
            .prepare_witnessed_asset_refund_v2(&request)
            .await
            .unwrap(),
        prepared
    );
    let mut conflicting = request.clone();
    conflicting.context.request_id = RequestId::new("asset-token-refund-prepare-conflict").unwrap();
    assert_eq!(
        planner
            .prepare_witnessed_asset_refund_v2(&conflicting)
            .await
            .unwrap_err(),
        NativePrepareError::ActiveWitnessedAssetRefundPrepare
    );
    assert_eq!(nonce.calls.load(Ordering::SeqCst), 0);
}

fn mutate_token_term(
    request: &PrepareWitnessedAssetEscrowV2Request,
    field: &str,
    value: Hex32,
) -> PrepareWitnessedAssetEscrowV2Request {
    let mut json = serde_json::to_value(request).unwrap();
    json["terms"]["asset"]["terms"][field] = serde_json::to_value(value).unwrap();
    serde_json::from_value(json).unwrap()
}

#[tokio::test]
async fn rejects_program_definition_ata_authority_order_and_conflict_drift_fail_closed() {
    let fixture = fixture(Participant::Maker, 101, "negative");
    let nonce = Arc::new(ExactNonce {
        expected: fixture.depositor,
        value: 151,
        calls: AtomicUsize::new(0),
    });
    let planner = NativeEscrowPlanner::new(
        fixture.role,
        fixture.depositor_key,
        ESCROW_PROGRAM,
        LEGACY_TRANSFER_PROGRAM,
        fixture.request.runtime.clone(),
        Arc::clone(&nonce),
    )
    .unwrap();

    for (field, value, expected) in [
        (
            "token_program_id",
            h(201),
            NativePrepareError::WrongTokenProgram,
        ),
        (
            "ata_program_id",
            h(202),
            NativePrepareError::WrongAtaProgram,
        ),
        (
            "token_definition_account_id",
            h(203),
            NativePrepareError::WrongTokenAccount,
        ),
        (
            "depositor_ata_account_id",
            h(204),
            NativePrepareError::WrongTokenAccount,
        ),
        (
            "aggregate_authority_account_id",
            h(205),
            NativePrepareError::WrongAggregateAuthority,
        ),
    ] {
        let drifted = mutate_token_term(&fixture.request, field, value);
        assert_eq!(
            planner
                .prepare_witnessed_asset_escrow_v2(&drifted)
                .await
                .unwrap_err(),
            expected
        );
    }
    assert_eq!(nonce.calls.load(Ordering::SeqCst), 0);

    let prepared = planner
        .prepare_witnessed_asset_escrow_v2(&fixture.request)
        .await
        .unwrap();
    assert_eq!(nonce.calls.load(Ordering::SeqCst), 1);
    let mut reordered = prepared.clone();
    let initialization =
        decode_prepared_for_signer(&reordered.effects[0].transaction, fixture.depositor).unwrap();
    let mut message = initialization.message().clone();
    message.account_ids.swap(1, 2);
    let signer_key = PrivateKey::try_new([102; 32]).unwrap();
    let witnesses = WitnessSet::for_message(&message, &[&signer_key]);
    reordered.effects[0].transaction =
        prepared_from_transaction(&PublicTransaction::new(message, witnesses)).unwrap();
    assert_eq!(
        planner
            .validate_prepared_witnessed_asset_escrow_v2(&fixture.request, &reordered)
            .unwrap_err(),
        NativePrepareError::InvalidTransactionBytes
    );

    let mut conflicting = fixture.request.clone();
    conflicting.context.request_id = RequestId::new("asset-token-prepare-conflict").unwrap();
    assert_eq!(
        planner
            .prepare_witnessed_asset_escrow_v2(&conflicting)
            .await
            .unwrap_err(),
        NativePrepareError::ActiveWitnessedAssetEscrowPrepare
    );
    assert_eq!(nonce.calls.load(Ordering::SeqCst), 1);
    let debug = format!("{planner:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains(&hex::encode([102; 32])));
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
            "lez-v02-asset-token-{}-{}",
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
async fn exact_token_plan_is_durable_before_exposure_and_restart_never_regenerates() {
    let directory = SecureDirectory::new();
    let fixture = fixture(Participant::Maker, 51, "restart");
    let first_nonce = Arc::new(ExactNonce {
        expected: fixture.depositor,
        value: 101,
        calls: AtomicUsize::new(0),
    });
    let first = NativeEscrowPlanner::new_durable(
        fixture.role,
        fixture.depositor_key,
        ESCROW_PROGRAM,
        LEGACY_TRANSFER_PROGRAM,
        fixture.request.runtime.clone(),
        Arc::clone(&first_nonce),
        directory.path(),
    )
    .unwrap();
    let prepared = first
        .prepare_witnessed_asset_escrow_v2(&fixture.request)
        .await
        .unwrap();
    assert_eq!(first_nonce.calls.load(Ordering::SeqCst), 1);
    assert!(
        directory
            .path()
            .join("witnessed-asset-escrow-reservation.v2.json")
            .is_file()
    );
    drop(first);

    let (_, restarted_key, _) = account(52);
    let restarted_nonce = Arc::new(ExactNonce {
        expected: fixture.depositor,
        value: 999,
        calls: AtomicUsize::new(0),
    });
    let restarted = NativeEscrowPlanner::new_durable(
        fixture.role,
        restarted_key,
        ESCROW_PROGRAM,
        LEGACY_TRANSFER_PROGRAM,
        fixture.request.runtime.clone(),
        Arc::clone(&restarted_nonce),
        directory.path(),
    )
    .unwrap();
    assert_eq!(
        restarted
            .prepare_witnessed_asset_escrow_v2(&fixture.request)
            .await
            .unwrap(),
        prepared
    );
    assert_eq!(restarted_nonce.calls.load(Ordering::SeqCst), 0);
}

#[cfg(target_os = "linux")]
#[allow(
    clippy::too_many_lines,
    reason = "one restart corridor proves all distinct v2 files without sharing process state"
)]
#[tokio::test]
async fn token_claim_completion_and_refund_restart_from_distinct_v2_files_without_regeneration() {
    let claim_directory = SecureDirectory::new();
    let claim_fixture = fixture(Participant::Maker, 81, "durable-claim");
    let claim_terms = claim_fixture.request.terms.clone();
    let claimant_role = claim_terms.asset().custom_token().unwrap().claimant();
    let claimant_runtime = runtime(claimant_role, claim_fixture.claimant);
    let claim_request = PrepareWitnessedAssetClaimV2Request::new(
        MessageContext::new(
            RunId::new("asset-token-durable-run").unwrap(),
            RequestId::new("asset-token-durable-claim-prepare").unwrap(),
            claimant_role,
        ),
        claimant_runtime.clone(),
        claim_terms.clone(),
        TransactionId::from_bytes([82; 32]),
    );
    let first_nonce = Arc::new(ExactNonce {
        expected: claim_fixture.authority,
        value: 131,
        calls: AtomicUsize::new(0),
    });
    let first = NativeEscrowPlanner::new_durable(
        claimant_role,
        claim_fixture.claimant_key,
        ESCROW_PROGRAM,
        LEGACY_TRANSFER_PROGRAM,
        claimant_runtime.clone(),
        Arc::clone(&first_nonce),
        claim_directory.path(),
    )
    .unwrap();
    let prepared = first
        .prepare_witnessed_asset_claim_v2(&claim_request)
        .await
        .unwrap();
    let message = Message::try_from_slice(prepared.claim.exact_message_bytes.as_slice()).unwrap();
    let signature = Signature::new(&claim_fixture.authority_key, &message.hash());
    let completion = CompleteWitnessedAssetClaimV2Request::new(
        MessageContext::new(
            claim_request.context.run_id.clone(),
            RequestId::new("asset-token-durable-claim-complete").unwrap(),
            claimant_role,
        ),
        claimant_runtime.clone(),
        claim_terms,
        prepared.claim.clone(),
        AggregateBip340Signature::from_bytes(signature.value),
    );
    assert_eq!(first_nonce.calls.load(Ordering::SeqCst), 1);
    assert!(
        claim_directory
            .path()
            .join("witnessed-asset-claim-reservation.v2.json")
            .is_file()
    );
    drop(first);

    let (_, restarted_claimant_key, _) = account(83);
    let restarted_nonce = Arc::new(ExactNonce {
        expected: claim_fixture.authority,
        value: 999,
        calls: AtomicUsize::new(0),
    });
    let restarted = NativeEscrowPlanner::new_durable(
        claimant_role,
        restarted_claimant_key,
        ESCROW_PROGRAM,
        LEGACY_TRANSFER_PROGRAM,
        claimant_runtime.clone(),
        Arc::clone(&restarted_nonce),
        claim_directory.path(),
    )
    .unwrap();
    let completed = restarted
        .complete_witnessed_asset_claim_v2(&completion)
        .await
        .unwrap();
    assert_eq!(restarted_nonce.calls.load(Ordering::SeqCst), 0);
    assert!(
        claim_directory
            .path()
            .join("witnessed-asset-claim-completion.v2.json")
            .is_file()
    );
    restarted
        .validate_owned_submission(&completed.claim)
        .await
        .unwrap();
    drop(restarted);

    let (_, second_restart_key, _) = account(83);
    let second_restart_nonce = Arc::new(ExactNonce {
        expected: claim_fixture.authority,
        value: 1_000,
        calls: AtomicUsize::new(0),
    });
    let second_restart = NativeEscrowPlanner::new_durable(
        claimant_role,
        second_restart_key,
        ESCROW_PROGRAM,
        LEGACY_TRANSFER_PROGRAM,
        claimant_runtime,
        Arc::clone(&second_restart_nonce),
        claim_directory.path(),
    )
    .unwrap();
    assert_eq!(
        second_restart
            .complete_witnessed_asset_claim_v2(&completion)
            .await
            .unwrap(),
        completed
    );
    assert_eq!(second_restart_nonce.calls.load(Ordering::SeqCst), 0);

    let refund_directory = SecureDirectory::new();
    let refund_fixture = fixture(Participant::Taker, 91, "durable-refund");
    let refund_request = PrepareWitnessedAssetRefundV2Request::new(
        MessageContext::new(
            RunId::new("asset-token-durable-run").unwrap(),
            RequestId::new("asset-token-durable-refund").unwrap(),
            refund_fixture.role,
        ),
        refund_fixture.request.runtime.clone(),
        refund_fixture.request.terms.clone(),
    );
    let refund_nonce = Arc::new(ExactNonce {
        expected: refund_fixture.depositor,
        value: 141,
        calls: AtomicUsize::new(0),
    });
    let refund_first = NativeEscrowPlanner::new_durable(
        refund_fixture.role,
        refund_fixture.depositor_key,
        ESCROW_PROGRAM,
        LEGACY_TRANSFER_PROGRAM,
        refund_fixture.request.runtime.clone(),
        Arc::clone(&refund_nonce),
        refund_directory.path(),
    )
    .unwrap();
    let refund = refund_first
        .prepare_witnessed_asset_refund_v2(&refund_request)
        .await
        .unwrap();
    assert_eq!(refund_nonce.calls.load(Ordering::SeqCst), 0);
    drop(refund_first);

    let (_, refund_restart_key, _) = account(92);
    let refund_restart_nonce = Arc::new(ExactNonce {
        expected: refund_fixture.depositor,
        value: 1_000,
        calls: AtomicUsize::new(0),
    });
    let refund_restart = NativeEscrowPlanner::new_durable(
        refund_fixture.role,
        refund_restart_key,
        ESCROW_PROGRAM,
        LEGACY_TRANSFER_PROGRAM,
        refund_fixture.request.runtime.clone(),
        Arc::clone(&refund_restart_nonce),
        refund_directory.path(),
    )
    .unwrap();
    assert_eq!(
        refund_restart
            .prepare_witnessed_asset_refund_v2(&refund_request)
            .await
            .unwrap(),
        refund
    );
    assert_eq!(refund_restart_nonce.calls.load(Ordering::SeqCst), 0);
    assert!(
        refund_directory
            .path()
            .join("witnessed-asset-refund-reservation.v2.json")
            .is_file()
    );
}
