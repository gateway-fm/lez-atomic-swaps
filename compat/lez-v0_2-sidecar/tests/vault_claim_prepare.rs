use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use common::transaction::LeeTransaction;
use lez_bridge_protocol::{
    ExactTransactionBytes, Hex32, MessageContext, Participant, RequestId, RunId,
    RuntimeCompatibility, RuntimeDescriptor, TransactionId,
};
use lez_v0_2_sidecar::{
    PrepareVaultClaimRequest, PrepareVaultClaimResult, VaultClaimAllocation, VaultClaimNonceSource,
    VaultClaimPlanner, VaultClaimPrepareError, decode_prepared_for_signer,
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
impl VaultClaimNonceSource for FixedNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, VaultClaimPrepareError> {
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

fn runtime(role: Participant, signer: AccountId) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::LeeV0_2_0,
        h(11),
        h(12),
        h(13),
        program_hex([0x1020_3040; 8]),
        Hex32::from_bytes(signer.into_value()),
    )
}

fn allocation(role: Participant, owner: AccountId, amount: u128) -> VaultClaimAllocation {
    VaultClaimAllocation::new(role, Hex32::from_bytes(owner.into_value()), amount).unwrap()
}

fn request(
    role: Participant,
    owner: AccountId,
    amount: u128,
    request_id: &str,
) -> PrepareVaultClaimRequest {
    PrepareVaultClaimRequest::new(
        MessageContext::new(
            RunId::new("v02-vault-onboarding-0001").unwrap(),
            RequestId::new(request_id).unwrap(),
            role,
        ),
        runtime(role, owner),
        allocation(role, owner, amount),
        0,
    )
}

const VAULT_PROGRAM_ID_SNAPSHOT: [u32; 8] = [
    1_168_813_120,
    241_877_831,
    3_407_559_972,
    2_131_462_206,
    1_965_161_891,
    2_000_235_008,
    2_574_408_698,
    1_333_126_597,
];

#[tokio::test]
async fn prepares_exact_maker_and_taker_claim_snapshots_with_distinct_allocations() {
    let actors = [
        (
            Participant::Maker,
            1,
            100_000,
            "1b84c5567b126440995d3ed5aaba0565d71e1834604819ff9c17f5e9d5dd078f",
            "B1UN3hPgxacgHKBRoThcAmsPajGcUf6YXUhgB36x4DAd",
            "7Mzr43PK9VxpcvwdjgL8PeE4nb2aG9FqBKLfkoH8RBmQ",
            "v02-maker-vault-claim-0001",
        ),
        (
            Participant::Taker,
            2,
            200_000,
            "4d4b6cd1361032ca9bd2aeb9d900aa4d45d9ead80ac9423374c451a7254d0766",
            "34Kqgek6R7N1zU5FSJz8ziXwSPEPCuWGcn1T7GCVrfib",
            "AXLjVw4tKTgieQoGRgXMVLVVaB4c5YnL1YTogZdX1cpH",
            "v02-taker-vault-claim-0001",
        ),
    ];
    let mut prepared_claims = Vec::new();

    for (role, key_byte, amount, public_key_snapshot, owner_snapshot, vault_snapshot, request_id) in
        actors
    {
        let (owner, key) = keyed_account(key_byte);
        let public_key = PublicKey::new_from_private_key(&key);
        let nonce_source = Arc::new(FixedNonce {
            value: 0,
            calls: AtomicUsize::new(0),
        });
        let expected_allocation = allocation(role, owner, amount);
        let planner = VaultClaimPlanner::new(
            role,
            key,
            runtime(role, owner),
            expected_allocation,
            Arc::clone(&nonce_source),
        )
        .unwrap();
        let request = request(role, owner, amount, request_id);

        let prepared = planner.prepare(request.clone()).await.unwrap();
        planner.validate_prepared(&request, &prepared).unwrap();
        assert_eq!(prepared.context, request.context);
        assert_eq!(nonce_source.calls.load(Ordering::SeqCst), 1);

        let transaction = decode_prepared_for_signer(&prepared.claim, owner).unwrap();
        let vault_program = programs::vault().id();
        assert_eq!(vault_program, VAULT_PROGRAM_ID_SNAPSHOT);
        let owner_vault = vault_core::compute_vault_account_id(vault_program, owner);
        assert_eq!(hex::encode(public_key.value()), public_key_snapshot);
        assert_eq!(owner.to_string(), owner_snapshot);
        assert_eq!(owner_vault.to_string(), vault_snapshot);
        assert_eq!(transaction.message().program_id, vault_program);
        assert_eq!(
            transaction.message().account_ids,
            vec![owner, owner_vault],
            "the owner signs and its Vault PDA never does"
        );
        assert_eq!(transaction.message().nonces, vec![0_u128.into()]);
        assert_eq!(
            transaction.message().instruction_data,
            Program::serialize_instruction(vault_core::Instruction::Claim { amount }).unwrap()
        );
        assert_eq!(
            transaction.witness_set().signatures_and_public_keys().len(),
            1
        );
        assert_eq!(
            AccountId::from(&transaction.witness_set().signatures_and_public_keys()[0].1),
            owner
        );
        assert!(
            LeeTransaction::Public(transaction.clone())
                .transaction_stateless_check()
                .is_ok()
        );
        assert_eq!(
            transaction.hash(),
            *prepared.claim.transaction_id.as_bytes()
        );
        assert_eq!(
            transaction.to_bytes(),
            prepared.claim.exact_bytes.as_slice()
        );

        assert_eq!(planner.prepare(request).await.unwrap(), prepared);
        assert_eq!(nonce_source.calls.load(Ordering::SeqCst), 1);
        prepared_claims.push((amount, owner, owner_vault, prepared.claim));
    }

    assert_ne!(prepared_claims[0].0, prepared_claims[1].0);
    assert_ne!(prepared_claims[0].1, prepared_claims[1].1);
    assert_ne!(prepared_claims[0].2, prepared_claims[1].2);
    assert_ne!(
        prepared_claims[0].3.transaction_id,
        prepared_claims[1].3.transaction_id
    );
}

#[tokio::test]
async fn rejects_role_key_runtime_owner_and_allocation_drift_before_nonce_lookup() {
    let (owner, key) = keyed_account(1);
    let nonce_source = Arc::new(FixedNonce {
        value: 0,
        calls: AtomicUsize::new(0),
    });
    let expected_runtime = runtime(Participant::Maker, owner);
    let expected_allocation = allocation(Participant::Maker, owner, 100_000);
    let planner = VaultClaimPlanner::new(
        Participant::Maker,
        key,
        expected_runtime.clone(),
        expected_allocation.clone(),
        Arc::clone(&nonce_source),
    )
    .unwrap();
    let valid = request(
        Participant::Maker,
        owner,
        100_000,
        "v02-maker-vault-claim-0002",
    );

    let mut wrong_context_role = valid.clone();
    wrong_context_role.context.sidecar_role = Participant::Taker;
    assert_eq!(
        planner.prepare(wrong_context_role).await.unwrap_err(),
        VaultClaimPrepareError::WrongRole
    );

    let mut wrong_runtime_role = valid.clone();
    wrong_runtime_role.runtime.sidecar_role = Participant::Taker;
    assert_eq!(
        planner.prepare(wrong_runtime_role).await.unwrap_err(),
        VaultClaimPrepareError::WrongRole
    );

    let mut wrong_runtime = valid.clone();
    wrong_runtime.runtime.channel_id = h(99);
    assert_eq!(
        planner.prepare(wrong_runtime).await.unwrap_err(),
        VaultClaimPrepareError::WrongRuntime
    );

    let (other_owner, _) = keyed_account(3);
    let mut wrong_owner = valid.clone();
    wrong_owner.allocation = allocation(Participant::Maker, other_owner, 100_000);
    assert_eq!(
        planner.prepare(wrong_owner).await.unwrap_err(),
        VaultClaimPrepareError::WrongSigner
    );

    let mut wrong_allocation_role = valid.clone();
    wrong_allocation_role.allocation = allocation(Participant::Taker, owner, 100_000);
    assert_eq!(
        planner.prepare(wrong_allocation_role).await.unwrap_err(),
        VaultClaimPrepareError::WrongAllocation
    );

    let mut wrong_amount = valid;
    wrong_amount.allocation = allocation(Participant::Maker, owner, 200_000);
    assert_eq!(
        planner.prepare(wrong_amount).await.unwrap_err(),
        VaultClaimPrepareError::WrongAllocation
    );
    assert_eq!(nonce_source.calls.load(Ordering::SeqCst), 0);

    let mut wrong_nonce = request(
        Participant::Maker,
        owner,
        100_000,
        "v02-maker-vault-claim-0006",
    );
    wrong_nonce.owner_nonce = 1;
    assert_eq!(
        planner.prepare(wrong_nonce).await.unwrap_err(),
        VaultClaimPrepareError::WrongNonce
    );
    assert_eq!(nonce_source.calls.load(Ordering::SeqCst), 1);

    assert_eq!(
        VaultClaimAllocation::new(Participant::Maker, Hex32::from_bytes(owner.into_value()), 0,)
            .unwrap_err(),
        VaultClaimPrepareError::ZeroAllocation
    );

    let (wrong_key_owner, wrong_key) = keyed_account(4);
    assert_eq!(
        VaultClaimPlanner::new(
            Participant::Maker,
            wrong_key,
            expected_runtime,
            expected_allocation,
            Arc::new(FixedNonce {
                value: 0,
                calls: AtomicUsize::new(0),
            }),
        )
        .unwrap_err(),
        VaultClaimPrepareError::WrongSigner
    );
    assert_ne!(wrong_key_owner, owner);
}

#[tokio::test]
async fn reserves_one_exact_claim_and_rejects_a_distinct_active_request() {
    let (owner, key) = keyed_account(2);
    let planner = VaultClaimPlanner::new(
        Participant::Taker,
        key,
        runtime(Participant::Taker, owner),
        allocation(Participant::Taker, owner, 200_000),
        Arc::new(FixedNonce {
            value: 7,
            calls: AtomicUsize::new(0),
        }),
    )
    .unwrap();
    let mut first = request(
        Participant::Taker,
        owner,
        200_000,
        "v02-taker-vault-claim-0002",
    );
    first.owner_nonce = 7;
    let mut distinct = first.clone();
    distinct.context.request_id = RequestId::new("v02-taker-vault-claim-0003").unwrap();

    let _prepared = planner.prepare(first).await.unwrap();
    assert_eq!(
        planner.prepare(distinct).await.unwrap_err(),
        VaultClaimPrepareError::ActivePrepare
    );
}

fn resign(mut transaction: PublicTransaction, key: &PrivateKey) -> PrepareVaultClaimResult {
    transaction.witness_set = WitnessSet::for_message(&transaction.message, &[key]);
    PrepareVaultClaimResult::new(
        MessageContext::new(
            RunId::new("v02-vault-onboarding-0001").unwrap(),
            RequestId::new("v02-maker-vault-claim-0004").unwrap(),
            Participant::Maker,
        ),
        prepared_from_transaction(&transaction).unwrap(),
    )
}

#[tokio::test]
async fn recovered_claim_rejects_validly_signed_program_account_order_amount_nonce_and_key_changes()
{
    let (owner, key) = keyed_account(1);
    let planner = VaultClaimPlanner::new(
        Participant::Maker,
        key.clone(),
        runtime(Participant::Maker, owner),
        allocation(Participant::Maker, owner, 100_000),
        Arc::new(FixedNonce {
            value: 5,
            calls: AtomicUsize::new(0),
        }),
    )
    .unwrap();
    let mut request = request(
        Participant::Maker,
        owner,
        100_000,
        "v02-maker-vault-claim-0004",
    );
    request.owner_nonce = 5;
    let prepared = planner.prepare(request.clone()).await.unwrap();
    let transaction = decode_prepared_for_signer(&prepared.claim, owner).unwrap();

    let mut wrong_program = transaction.clone();
    wrong_program.message.program_id = [88; 8];
    assert_eq!(
        planner
            .validate_prepared(&request, &resign(wrong_program, &key))
            .unwrap_err(),
        VaultClaimPrepareError::InvalidTransactionBytes
    );

    let (other_account, _) = keyed_account(9);
    let mut wrong_account = transaction.clone();
    wrong_account.message.account_ids[1] = other_account;
    assert_eq!(
        planner
            .validate_prepared(&request, &resign(wrong_account, &key))
            .unwrap_err(),
        VaultClaimPrepareError::InvalidTransactionBytes
    );

    let mut wrong_order = transaction.clone();
    wrong_order.message.account_ids.swap(0, 1);
    assert_eq!(
        planner
            .validate_prepared(&request, &resign(wrong_order, &key))
            .unwrap_err(),
        VaultClaimPrepareError::InvalidTransactionBytes
    );

    let mut wrong_amount = transaction.clone();
    wrong_amount.message.instruction_data =
        Program::serialize_instruction(vault_core::Instruction::Claim { amount: 99_999 }).unwrap();
    assert_eq!(
        planner
            .validate_prepared(&request, &resign(wrong_amount, &key))
            .unwrap_err(),
        VaultClaimPrepareError::InvalidTransactionBytes
    );

    let mut wrong_nonce = transaction.clone();
    wrong_nonce.message.nonces = vec![6_u128.into()];
    assert_eq!(
        planner
            .validate_prepared(&request, &resign(wrong_nonce, &key))
            .unwrap_err(),
        VaultClaimPrepareError::InvalidTransactionBytes
    );

    let (_, wrong_key) = keyed_account(10);
    assert_eq!(
        planner
            .validate_prepared(&request, &resign(transaction, &wrong_key))
            .unwrap_err(),
        VaultClaimPrepareError::WrongSigner
    );

    let mut wrong_context = prepared;
    wrong_context.context.sidecar_role = Participant::Taker;
    assert_eq!(
        planner
            .validate_prepared(&request, &wrong_context)
            .unwrap_err(),
        VaultClaimPrepareError::InvalidTransactionBytes
    );
}

#[tokio::test]
async fn recovered_claim_rejects_hash_signature_and_noncanonical_byte_substitution() {
    let (owner, key) = keyed_account(1);
    let planner = VaultClaimPlanner::new(
        Participant::Maker,
        key,
        runtime(Participant::Maker, owner),
        allocation(Participant::Maker, owner, 100_000),
        Arc::new(FixedNonce {
            value: 0,
            calls: AtomicUsize::new(0),
        }),
    )
    .unwrap();
    let request = request(
        Participant::Maker,
        owner,
        100_000,
        "v02-maker-vault-claim-0005",
    );
    let prepared = planner.prepare(request.clone()).await.unwrap();

    let mut wrong_id = prepared.clone();
    wrong_id.claim.transaction_id = TransactionId::from_bytes([99; 32]);
    assert_eq!(
        planner.validate_prepared(&request, &wrong_id).unwrap_err(),
        VaultClaimPrepareError::WrongTransactionId
    );

    let decoded = PublicTransaction::from_bytes(prepared.claim.exact_bytes.as_slice()).unwrap();
    let invalid_signature = PublicTransaction::new(
        decoded.message.clone(),
        WitnessSet::from_raw_parts(Vec::new()),
    );
    let invalid_signature = PrepareVaultClaimResult::new(
        request.context.clone(),
        lez_bridge_protocol::PreparedTransaction::new(
            TransactionId::from_bytes(invalid_signature.hash()),
            ExactTransactionBytes::new(invalid_signature.to_bytes()).unwrap(),
        ),
    );
    assert_eq!(
        planner
            .validate_prepared(&request, &invalid_signature)
            .unwrap_err(),
        VaultClaimPrepareError::InvalidSignature
    );

    let mut noncanonical = prepared;
    let mut bytes = noncanonical.claim.exact_bytes.as_slice().to_vec();
    bytes.push(0);
    noncanonical.claim.exact_bytes = ExactTransactionBytes::new(bytes).unwrap();
    assert_eq!(
        planner
            .validate_prepared(&request, &noncanonical)
            .unwrap_err(),
        VaultClaimPrepareError::InvalidTransactionBytes
    );
}

#[test]
fn allocation_and_planner_diagnostics_redact_key_material() {
    let (owner, key) = keyed_account(1);
    let allocation = allocation(Participant::Maker, owner, 100_000);
    let planner = VaultClaimPlanner::new(
        Participant::Maker,
        key,
        runtime(Participant::Maker, owner),
        allocation.clone(),
        Arc::new(FixedNonce {
            value: 0,
            calls: AtomicUsize::new(0),
        }),
    )
    .unwrap();

    let rendered = format!("{planner:?} {allocation:?}");
    assert!(rendered.contains("[REDACTED]"));
    assert!(!rendered.contains(&"01".repeat(32)));
}
