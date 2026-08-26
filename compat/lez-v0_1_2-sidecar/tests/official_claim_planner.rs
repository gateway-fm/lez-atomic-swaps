use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use lez_bridge_protocol::{
    Hex32, MessageContext, NativeEscrowTerms, NativeEscrowTermsInput, Participant,
    PrepareRevealingClaimRequest, RequestId, RevealingPreimage, RunId, RuntimeCompatibility,
    RuntimeDescriptor, TransactionId,
};
use lez_v0_1_2_sidecar::{
    NativeEscrowPlanner, NonceSource, SidecarError, decode_prepared_for_role,
};
use nssa::{AccountId, PrivateKey, PublicKey, program::Program};
use sha2::{Digest as _, Sha256};

#[derive(Debug)]
struct CountingNonceSource {
    calls: AtomicUsize,
    nonce: u128,
}

#[async_trait]
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

fn context(role: Participant, request_id: &str) -> MessageContext {
    MessageContext::new(
        RunId::new("claim-planner-run-0001").unwrap(),
        RequestId::new(request_id).unwrap(),
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

fn claim_request(
    role: Participant,
    signer: AccountId,
    depositor: AccountId,
    escrow_program: [u32; 8],
    preimage: [u8; 32],
    funding_id: [u8; 32],
    request_id: &str,
) -> PrepareRevealingClaimRequest {
    let depositor_role = match role {
        Participant::Maker => Participant::Taker,
        Participant::Taker => Participant::Maker,
    };
    let terms = NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: h(4),
        terms_hash: h(5),
        secret_digest: Hex32::from_bytes(Sha256::digest(preimage).into()),
        depositor: depositor_role,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: role,
        claimant_account_id: Hex32::from_bytes(signer.into_value()),
        amount: 91,
        refund_at_ms: 1_750_000_000_123,
        authenticated_transfer_program_id: program_hex(
            Program::authenticated_transfer_program().id(),
        ),
    })
    .unwrap();
    PrepareRevealingClaimRequest::new(
        context(role, request_id),
        runtime(role, signer, escrow_program),
        terms,
        TransactionId::from_bytes(funding_id),
        RevealingPreimage::new(preimage),
    )
}

fn planner(
    key_byte: u8,
    escrow_program: [u32; 8],
    nonce_source: Arc<CountingNonceSource>,
) -> (AccountId, NativeEscrowPlanner) {
    let (signer, key) = keyed_account(key_byte);
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        key,
        escrow_program,
        runtime(Participant::Maker, signer, escrow_program),
        nonce_source,
    )
    .unwrap();
    (signer, planner)
}

#[tokio::test]
async fn plans_official_claim_once_with_exact_guest_order_nonce_preimage_and_bytes() {
    let escrow_program = [0x1234_5678; 8];
    let nonce_source = Arc::new(CountingNonceSource {
        calls: AtomicUsize::new(0),
        nonce: 79,
    });
    let (claimant, planner) = planner(111, escrow_program, Arc::clone(&nonce_source));
    let (depositor, _) = keyed_account(112);
    let preimage = [0x42; 32];
    let request = claim_request(
        Participant::Maker,
        claimant,
        depositor,
        escrow_program,
        preimage,
        [0x77; 32],
        "claim-prepare-0001",
    );

    let prepared = planner.prepare_revealing_claim(&request).await.unwrap();
    let decoded = decode_prepared_for_role(
        &prepared.claim,
        Participant::Maker,
        Participant::Maker,
        claimant,
    )
    .unwrap();
    let swap_id = *request.terms.swap_id().as_bytes();
    let metadata = spel_framework_core::pda::compute_pda(&escrow_program, &[&swap_id]);
    let custody_label = spel_framework_core::pda::seed_from_str("custody");
    let custody =
        spel_framework_core::pda::compute_pda(&escrow_program, &[&custody_label, &swap_id]);

    assert_eq!(prepared.context, request.context);
    assert_eq!(decoded.to_bytes(), prepared.claim.exact_bytes.as_slice());
    assert_eq!(decoded.hash(), *prepared.claim.transaction_id.as_bytes());
    assert_eq!(decoded.message.program_id, escrow_program);
    assert_eq!(
        decoded.message.account_ids,
        vec![metadata, custody, claimant]
    );
    assert_eq!(decoded.message.nonces, vec![79_u128.into()]);
    assert_eq!(
        decoded.message.instruction_data,
        Program::serialize_instruction(lez_zec_escrow_compat::Instruction::ClaimNative {
            swap_id,
            preimage,
        })
        .unwrap()
    );
    assert_eq!(nonce_source.calls.load(Ordering::SeqCst), 1);

    let replay = planner.prepare_revealing_claim(&request).await.unwrap();
    assert_eq!(replay, prepared);
    assert_eq!(nonce_source.calls.load(Ordering::SeqCst), 1);
    planner
        .decode_exact_for_submission(&prepared.claim, prepared.context.sidecar_role)
        .await
        .unwrap();
}

#[tokio::test]
async fn rejects_role_runtime_preimage_and_zero_funding_mutations() {
    let escrow_program = [0x8765_4321; 8];
    let (claimant, planner) = planner(
        121,
        escrow_program,
        Arc::new(CountingNonceSource {
            calls: AtomicUsize::new(0),
            nonce: 9,
        }),
    );
    let (depositor, _) = keyed_account(122);
    let wrong_role = claim_request(
        Participant::Taker,
        claimant,
        depositor,
        escrow_program,
        [1; 32],
        [2; 32],
        "claim-wrong-role-0001",
    );
    assert_eq!(
        planner
            .prepare_revealing_claim(&wrong_role)
            .await
            .unwrap_err(),
        SidecarError::WrongSidecarRole
    );

    let mut wrong_runtime = claim_request(
        Participant::Maker,
        claimant,
        depositor,
        escrow_program,
        [3; 32],
        [4; 32],
        "claim-wrong-runtime-0001",
    );
    wrong_runtime.runtime.chain_id = h(0xaa);
    assert_eq!(
        planner
            .prepare_revealing_claim(&wrong_runtime)
            .await
            .unwrap_err(),
        SidecarError::WrongRuntimeIdentity
    );

    let mut wrong_preimage = claim_request(
        Participant::Maker,
        claimant,
        depositor,
        escrow_program,
        [7; 32],
        [8; 32],
        "claim-wrong-preimage-0001",
    );
    let value = serde_json::to_value(&wrong_preimage).unwrap();
    let mut object = value.as_object().unwrap().clone();
    object.insert("preimage".to_owned(), serde_json::json!("09".repeat(32)));
    wrong_preimage = serde_json::from_value(serde_json::Value::Object(object)).unwrap();
    assert_eq!(
        planner
            .prepare_revealing_claim(&wrong_preimage)
            .await
            .unwrap_err(),
        SidecarError::WrongClaimPreimage
    );

    let zero_funding = claim_request(
        Participant::Maker,
        claimant,
        depositor,
        escrow_program,
        [10; 32],
        [0; 32],
        "claim-zero-funding-0001",
    );
    assert_eq!(
        planner
            .prepare_revealing_claim(&zero_funding)
            .await
            .unwrap_err(),
        SidecarError::InvalidFundingTransaction
    );
}

#[tokio::test]
async fn rejects_terms_claimant_account_that_does_not_match_isolated_signer() {
    let escrow_program = [0x8765_4321; 8];
    let (claimant, planner) = planner(
        121,
        escrow_program,
        Arc::new(CountingNonceSource {
            calls: AtomicUsize::new(0),
            nonce: 9,
        }),
    );
    let (depositor, _) = keyed_account(122);
    let (other, _) = keyed_account(123);
    let request = claim_request(
        Participant::Maker,
        claimant,
        depositor,
        escrow_program,
        [5; 32],
        [6; 32],
        "claim-wrong-signer-0001",
    );
    let mut request = serde_json::to_value(request).unwrap();
    request["terms"]["claimant_account_id"] =
        serde_json::json!(Hex32::from_bytes(other.into_value()));
    let request: PrepareRevealingClaimRequest = serde_json::from_value(request).unwrap();

    assert_eq!(
        planner.prepare_revealing_claim(&request).await.unwrap_err(),
        SidecarError::WrongSigner
    );
}

#[tokio::test]
async fn active_claim_binds_terms_funding_identity_and_redacts_secret() {
    let escrow_program = [0x1122_3344; 8];
    let (claimant, planner) = planner(
        131,
        escrow_program,
        Arc::new(CountingNonceSource {
            calls: AtomicUsize::new(0),
            nonce: 17,
        }),
    );
    let (depositor, _) = keyed_account(132);
    let preimage = [0xcd; 32];
    let request = claim_request(
        Participant::Maker,
        claimant,
        depositor,
        escrow_program,
        preimage,
        [0xef; 32],
        "claim-active-0001",
    );
    let prepared = planner.prepare_revealing_claim(&request).await.unwrap();
    assert!(!format!("{request:?}").contains(&"cd".repeat(32)));
    assert!(!format!("{planner:?}").contains(&"cd".repeat(32)));

    let changed_funding = claim_request(
        Participant::Maker,
        claimant,
        depositor,
        escrow_program,
        preimage,
        [0xee; 32],
        "claim-active-0001",
    );
    assert_eq!(
        planner
            .prepare_revealing_claim(&changed_funding)
            .await
            .unwrap_err(),
        SidecarError::ActiveClaimPrepare
    );
    let mut changed_terms = serde_json::to_value(&request).unwrap();
    changed_terms["terms"]["amount"] = serde_json::json!("92");
    let changed_terms: PrepareRevealingClaimRequest =
        serde_json::from_value(changed_terms).unwrap();
    assert_eq!(
        planner
            .prepare_revealing_claim(&changed_terms)
            .await
            .unwrap_err(),
        SidecarError::ActiveClaimPrepare
    );

    let (_, unrelated_key) = keyed_account(133);
    let unrelated = NativeEscrowPlanner::new(
        Participant::Maker,
        unrelated_key,
        escrow_program,
        runtime(Participant::Maker, keyed_account(133).0, escrow_program),
        Arc::new(CountingNonceSource {
            calls: AtomicUsize::new(0),
            nonce: 17,
        }),
    )
    .unwrap();
    assert_eq!(
        unrelated
            .decode_exact_for_submission(&prepared.claim, Participant::Maker)
            .await
            .unwrap_err(),
        SidecarError::TransactionNotPrepared
    );
}
