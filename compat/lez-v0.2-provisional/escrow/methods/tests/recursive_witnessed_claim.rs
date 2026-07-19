use borsh::BorshDeserialize as _;
use lez_zec_escrow_v02::{
    ClaimAuthority, EscrowMetadata, EscrowStatus, Instruction as EscrowInstruction,
};
use lez_zec_escrow_v02_methods::{ZEC_ESCROW_V02_ELF, ZEC_ESCROW_V02_ID};
use musig2::secp::{Point, Scalar};
use musig2::{CompactSignature, FirstRound, KeyAggContext, PartialSignature, SecNonceSpices};
use nssa::{
    Account, AccountId, PrivateKey, PublicKey, PublicTransaction, Signature, V03State,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use spel_framework_core::pda::{compute_pda, seed_from_str};

const SWAP_ID: [u8; 32] = [81; 32];
const AMOUNT: u128 = 75;
const REFUND_AT: u64 = 10_000;
const PUNISH_AT: u64 = 12_000;
const MAKER_SECRET: [u8; 32] = [0x31; 32];
const TAKER_SECRET: [u8; 32] = [0x42; 32];
const REFUND_MAKER_SECRET: [u8; 32] = [0x53; 32];
const REFUND_TAKER_SECRET: [u8; 32] = [0x64; 32];
const CLAIM_PARTIAL_CONTEXT_BINDING: [u8; 32] = [0x85; 32];
const CLAIM_PARTIAL: [u8; 32] = [0x96; 32];
const CLAIM_PARTIAL_DOMAIN: &[u8] = b"logos.gateway.lez-xmr.claim-partial-commitment.v1\0";

struct AggregateFixture {
    maker_secret: Scalar,
    taker_secret: Scalar,
    context: KeyAggContext,
    aggregate_point: Point,
    aggregate_public_key: PublicKey,
    authority: AccountId,
}

struct FundedFixture {
    state: V03State,
    metadata_id: AccountId,
    custody: AccountId,
    claimant: AccountId,
    claimant_key: PrivateKey,
    authority: AccountId,
    maker_key: PrivateKey,
    taker_key: PrivateKey,
}

struct XmrFundedFixture {
    state: V03State,
    metadata_id: AccountId,
    custody: AccountId,
    depositor: AccountId,
    depositor_key: PrivateKey,
    claimant: AccountId,
    claimant_key: PrivateKey,
    claim_aggregate: AggregateFixture,
    refund_aggregate: AggregateFixture,
}

fn actor(secret: [u8; 32]) -> (AccountId, PrivateKey) {
    let key = PrivateKey::try_new(secret).expect("deterministic test private key");
    let account_id = AccountId::from(&PublicKey::new_from_private_key(&key));
    (account_id, key)
}

fn aggregate_fixture() -> AggregateFixture {
    aggregate_fixture_from_secrets(MAKER_SECRET, TAKER_SECRET)
}

fn aggregate_fixture_from_secrets(
    maker_secret_bytes: [u8; 32],
    taker_secret_bytes: [u8; 32],
) -> AggregateFixture {
    let maker_secret = Scalar::from_slice(&maker_secret_bytes).expect("maker scalar");
    let taker_secret = Scalar::from_slice(&taker_secret_bytes).expect("taker scalar");
    let context =
        KeyAggContext::new([maker_secret.base_point_mul(), taker_secret.base_point_mul()])
            .expect("BIP-327 two-party key aggregation");
    let aggregate_point: Point = context.aggregated_pubkey();
    let aggregate_public_key = PublicKey::try_new(aggregate_point.serialize_xonly())
        .expect("MuSig2 aggregate is a LEZ BIP-340 key");
    // Official pinned lee API; guest initialization below only succeeds when
    // its lean lee_core-compatible mapping produces this exact AccountId.
    let authority = AccountId::from(&aggregate_public_key);
    AggregateFixture {
        maker_secret,
        taker_secret,
        context,
        aggregate_point,
        aggregate_public_key,
        authority,
    }
}

fn claim_partial_commitment() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(CLAIM_PARTIAL_DOMAIN);
    hasher.update(CLAIM_PARTIAL_CONTEXT_BINDING);
    hasher.update(CLAIM_PARTIAL);
    hasher.finalize().into()
}

fn transaction<T: Serialize>(
    state: &V03State,
    account_ids: Vec<AccountId>,
    signers: &[(AccountId, &PrivateKey)],
    instruction: T,
) -> PublicTransaction {
    let nonces = signers
        .iter()
        .map(|(account_id, _)| state.get_account_by_id(*account_id).nonce)
        .collect();
    let message = Message::try_new(ZEC_ESCROW_V02_ID, account_ids, nonces, instruction)
        .expect("serialize exact escrow instruction");
    let keys = signers.iter().map(|(_, key)| *key).collect::<Vec<_>>();
    PublicTransaction::new(message.clone(), WitnessSet::for_message(&message, &keys))
}

fn escrow_ids() -> (AccountId, AccountId) {
    let metadata = compute_pda(&ZEC_ESCROW_V02_ID, &[&SWAP_ID]);
    let label = seed_from_str("custody");
    let custody = compute_pda(&ZEC_ESCROW_V02_ID, &[&label, &SWAP_ID]);
    (metadata, custody)
}

fn metadata(state: &V03State, account_id: AccountId) -> EscrowMetadata {
    EscrowMetadata::try_from_slice(state.get_account_by_id(account_id).data.as_ref())
        .expect("state stores canonical escrow metadata")
}

fn funded_state(aggregate: &AggregateFixture) -> FundedFixture {
    let escrow = Program::new(ZEC_ESCROW_V02_ELF.into()).expect("checked guest is canonical ELF");
    assert_eq!(escrow.id(), ZEC_ESCROW_V02_ID);
    let authenticated_transfer = programs::authenticated_transfer();
    let authenticated_transfer_id = authenticated_transfer.id();
    let (depositor, depositor_key) = actor([1; 32]);
    let (claimant, claimant_key) = actor([2; 32]);
    let (maker, maker_key) = actor(MAKER_SECRET);
    let (taker, taker_key) = actor(TAKER_SECRET);
    assert_ne!(claimant, aggregate.authority);
    let mut state = V03State::new()
        .with_public_accounts([
            (
                depositor,
                Account {
                    program_owner: authenticated_transfer_id,
                    balance: 200,
                    ..Account::default()
                },
            ),
            (
                claimant,
                Account {
                    program_owner: authenticated_transfer_id,
                    balance: 10,
                    ..Account::default()
                },
            ),
            (aggregate.authority, Account::default()),
            (maker, Account::default()),
            (taker, Account::default()),
        ])
        .with_programs([escrow, authenticated_transfer]);
    let (metadata_id, custody) = escrow_ids();
    let initialize = transaction(
        &state,
        vec![
            metadata_id,
            custody,
            depositor,
            claimant,
            aggregate.authority,
        ],
        &[(depositor, &depositor_key)],
        EscrowInstruction::InitializeNativeWitnessed {
            swap_id: SWAP_ID,
            terms_hash: [31; 32],
            aggregate_x_only_public_key: *aggregate.aggregate_public_key.value(),
            amount: AMOUNT,
            refund_at: REFUND_AT,
            authenticated_transfer_program: authenticated_transfer_id,
        },
    );
    state
        .transition_from_public_transaction(&initialize, 1, 100)
        .expect("aggregate-authority escrow initializes recursively");
    let initialized = metadata(&state, metadata_id);
    assert_eq!(
        initialized.claim_authority,
        ClaimAuthority::AggregateWitness {
            x_only_public_key: *aggregate.aggregate_public_key.value(),
            account_id: aggregate.authority,
        }
    );
    assert_eq!(initialized.claimant, claimant);
    assert_ne!(initialized.claimant, aggregate.authority);

    let fund = transaction(
        &state,
        vec![metadata_id, custody, depositor],
        &[(depositor, &depositor_key)],
        EscrowInstruction::FundNative { swap_id: SWAP_ID },
    );
    state
        .transition_from_public_transaction(&fund, 2, 101)
        .expect("aggregate-authority escrow funds recursively");
    assert_eq!(metadata(&state, metadata_id).status, EscrowStatus::Funded);
    FundedFixture {
        state,
        metadata_id,
        custody,
        claimant,
        claimant_key,
        authority: aggregate.authority,
        maker_key,
        taker_key,
    }
}

fn xmr_funded_state(claimant_balance: u128) -> XmrFundedFixture {
    let escrow = Program::new(ZEC_ESCROW_V02_ELF.into()).expect("checked guest is canonical ELF");
    assert_eq!(escrow.id(), ZEC_ESCROW_V02_ID);
    let authenticated_transfer = programs::authenticated_transfer();
    let authenticated_transfer_id = authenticated_transfer.id();
    let (depositor, depositor_key) = actor([1; 32]);
    let (claimant, claimant_key) = actor([2; 32]);
    let claim_aggregate = aggregate_fixture();
    let refund_aggregate = aggregate_fixture_from_secrets(REFUND_MAKER_SECRET, REFUND_TAKER_SECRET);
    assert_ne!(claim_aggregate.authority, refund_aggregate.authority);
    assert_ne!(depositor, claimant);
    assert_ne!(depositor, claim_aggregate.authority);
    assert_ne!(depositor, refund_aggregate.authority);
    assert_ne!(claimant, claim_aggregate.authority);
    assert_ne!(claimant, refund_aggregate.authority);

    let mut state = V03State::new()
        .with_public_accounts([
            (
                depositor,
                Account {
                    program_owner: authenticated_transfer_id,
                    balance: 200,
                    ..Account::default()
                },
            ),
            (
                claimant,
                Account {
                    program_owner: authenticated_transfer_id,
                    balance: claimant_balance,
                    ..Account::default()
                },
            ),
            (claim_aggregate.authority, Account::default()),
            (refund_aggregate.authority, Account::default()),
        ])
        .with_programs([escrow, authenticated_transfer]);
    let (metadata_id, custody) = escrow_ids();
    let initialize = transaction(
        &state,
        vec![
            metadata_id,
            custody,
            depositor,
            claimant,
            claim_aggregate.authority,
            refund_aggregate.authority,
        ],
        &[(depositor, &depositor_key)],
        EscrowInstruction::InitializeNativeXmr {
            swap_id: SWAP_ID,
            terms_hash: [31; 32],
            claim_aggregate_x_only_public_key: *claim_aggregate.aggregate_public_key.value(),
            refund_aggregate_x_only_public_key: *refund_aggregate.aggregate_public_key.value(),
            maker_dleq_transcript_commitment: [0xa7; 32],
            taker_dleq_transcript_commitment: [0xb8; 32],
            claim_partial_context_binding: CLAIM_PARTIAL_CONTEXT_BINDING,
            claim_partial_commitment: claim_partial_commitment(),
            amount: AMOUNT,
            refund_at: REFUND_AT,
            punish_at: PUNISH_AT,
            authenticated_transfer_program: authenticated_transfer_id,
        },
    );
    state
        .transition_from_public_transaction(&initialize, 1, 100)
        .expect("version-3 XMR escrow initializes recursively");

    let initialized = metadata(&state, metadata_id);
    assert_eq!(initialized.version, 3);
    assert_eq!(initialized.status, EscrowStatus::Empty);
    assert_eq!(initialized.depositor, depositor);
    assert_eq!(initialized.claimant, claimant);
    assert_eq!(initialized.refund_at, REFUND_AT);
    assert_eq!(initialized.amount, AMOUNT);
    assert_eq!(
        initialized.claim_authority,
        ClaimAuthority::XmrDualAdaptor {
            claim_aggregate_x_only_public_key: *claim_aggregate.aggregate_public_key.value(),
            claim_aggregate_account_id: claim_aggregate.authority,
            refund_aggregate_x_only_public_key: *refund_aggregate.aggregate_public_key.value(),
            refund_aggregate_account_id: refund_aggregate.authority,
            maker_dleq_transcript_commitment: [0xa7; 32],
            taker_dleq_transcript_commitment: [0xb8; 32],
            claim_partial_context_binding: CLAIM_PARTIAL_CONTEXT_BINDING,
            claim_partial_commitment: claim_partial_commitment(),
            punish_at: PUNISH_AT,
        }
    );
    let serialized_metadata = borsh::to_vec(&initialized).expect("serialize XMR metadata");
    assert!(
        !serialized_metadata
            .windows(CLAIM_PARTIAL.len())
            .any(|window| window == CLAIM_PARTIAL),
        "initialization commits the context-bound partial without publishing it",
    );

    let fund = transaction(
        &state,
        vec![metadata_id, custody, depositor],
        &[(depositor, &depositor_key)],
        EscrowInstruction::FundNative { swap_id: SWAP_ID },
    );
    state
        .transition_from_public_transaction(&fund, 2, 101)
        .expect("version-3 XMR escrow funds recursively");
    assert_eq!(metadata(&state, metadata_id).status, EscrowStatus::Funded);
    assert_eq!(state.get_account_by_id(custody).balance, AMOUNT);
    assert_eq!(state.get_account_by_id(depositor).balance, 200 - AMOUNT);
    assert_eq!(state.get_account_by_id(claimant).balance, claimant_balance);

    XmrFundedFixture {
        state,
        metadata_id,
        custody,
        depositor,
        depositor_key,
        claimant,
        claimant_key,
        claim_aggregate,
        refund_aggregate,
    }
}

fn claim_message(
    state: &V03State,
    metadata: AccountId,
    custody: AccountId,
    claimant: AccountId,
    authority: AccountId,
) -> Message {
    Message::try_new(
        ZEC_ESCROW_V02_ID,
        vec![metadata, custody, claimant, authority],
        vec![state.get_account_by_id(authority).nonce],
        EscrowInstruction::ClaimNativeWitnessed { swap_id: SWAP_ID },
    )
    .expect("serialize witnessed claim")
}

fn aggregate_witness(aggregate: &AggregateFixture, message: &Message) -> WitnessSet {
    let message_hash = message.hash();
    let maker_spices = SecNonceSpices::new()
        .with_seckey(aggregate.maker_secret)
        .with_message(&message_hash);
    let taker_spices = SecNonceSpices::new()
        .with_seckey(aggregate.taker_secret)
        .with_message(&message_hash);
    let mut maker_first = FirstRound::new(aggregate.context.clone(), [0x71; 32], 0, maker_spices)
        .expect("maker first round");
    let mut taker_first = FirstRound::new(aggregate.context.clone(), [0x82; 32], 1, taker_spices)
        .expect("taker first round");
    let maker_nonce = maker_first.our_public_nonce();
    let taker_nonce = taker_first.our_public_nonce();
    maker_first
        .receive_nonce(1, taker_nonce)
        .expect("maker receives taker nonce");
    taker_first
        .receive_nonce(0, maker_nonce)
        .expect("taker receives maker nonce");

    let mut maker_second = maker_first
        .finalize(aggregate.maker_secret, message_hash)
        .expect("maker partial signature");
    let mut taker_second = taker_first
        .finalize(aggregate.taker_secret, message_hash)
        .expect("taker partial signature");
    let maker_partial: PartialSignature = maker_second.our_signature();
    let taker_partial: PartialSignature = taker_second.our_signature();
    maker_second
        .receive_signature(1, taker_partial)
        .expect("maker verifies taker partial");
    taker_second
        .receive_signature(0, maker_partial)
        .expect("taker verifies maker partial");
    let maker_signature: CompactSignature = maker_second.finalize().expect("maker aggregate");
    let taker_signature: CompactSignature = taker_second.finalize().expect("taker aggregate");
    assert_eq!(maker_signature, taker_signature);
    musig2::verify_single(aggregate.aggregate_point, maker_signature, message_hash)
        .expect("completed aggregate signature verifies under the MuSig2 key");

    let signature = Signature {
        value: maker_signature.serialize(),
    };
    WitnessSet::from_raw_parts(vec![(signature, aggregate.aggregate_public_key.clone())])
}

fn aggregate_transaction<T: Serialize>(
    state: &V03State,
    account_ids: Vec<AccountId>,
    aggregate: &AggregateFixture,
    instruction: T,
) -> PublicTransaction {
    let message = Message::try_new(
        ZEC_ESCROW_V02_ID,
        account_ids,
        vec![state.get_account_by_id(aggregate.authority).nonce],
        instruction,
    )
    .expect("serialize exact XMR aggregate-authority instruction");
    let witness = aggregate_witness(aggregate, &message);
    PublicTransaction::new(message, witness)
}

#[test]
fn recursive_native_claim_requires_exact_two_party_aggregate_witness() {
    let aggregate = aggregate_fixture();
    let mut funded = funded_state(&aggregate);
    let message = claim_message(
        &funded.state,
        funded.metadata_id,
        funded.custody,
        funded.claimant,
        funded.authority,
    );
    let witness = aggregate_witness(&aggregate, &message);
    assert_eq!(witness.signatures_and_public_keys().len(), 1);
    let (signature, public_key) = &witness.signatures_and_public_keys()[0];
    assert_eq!(public_key, &aggregate.aggregate_public_key);
    assert_eq!(AccountId::from(public_key), funded.authority);
    assert_ne!(funded.authority, funded.claimant);
    assert!(signature.is_valid_for(&message.hash(), public_key));
    funded
        .state
        .transition_from_public_transaction(&PublicTransaction::new(message, witness), 3, 102)
        .expect("completed two-party aggregate witness claims recursively");
    assert_eq!(
        metadata(&funded.state, funded.metadata_id).status,
        EscrowStatus::Claimed
    );
    assert_eq!(funded.state.get_account_by_id(funded.custody).balance, 0);
    assert_eq!(
        funded.state.get_account_by_id(funded.claimant).balance,
        10 + AMOUNT
    );
    assert_eq!(
        funded.state.get_account_by_id(funded.authority).balance,
        0,
        "aggregate authority authorizes but never receives the escrowed asset",
    );

    for share in [MAKER_SECRET, TAKER_SECRET] {
        let mut funded = funded_state(&aggregate);
        let message = claim_message(
            &funded.state,
            funded.metadata_id,
            funded.custody,
            funded.claimant,
            funded.authority,
        );
        let share_key = if share == MAKER_SECRET {
            &funded.maker_key
        } else {
            &funded.taker_key
        };
        let one_share = WitnessSet::for_message(&message, &[share_key]);
        assert!(
            funded
                .state
                .transition_from_public_transaction(
                    &PublicTransaction::new(message, one_share),
                    3,
                    102,
                )
                .is_err(),
            "one key share must not authorize the separate aggregate authority account",
        );
        assert_eq!(
            metadata(&funded.state, funded.metadata_id).status,
            EscrowStatus::Funded
        );
    }

    let mut funded = funded_state(&aggregate);
    let signed_message = claim_message(
        &funded.state,
        funded.metadata_id,
        funded.custody,
        funded.claimant,
        funded.authority,
    );
    let witness = aggregate_witness(&aggregate, &signed_message);
    let wrong_message = Message::try_new(
        ZEC_ESCROW_V02_ID,
        vec![
            funded.metadata_id,
            funded.custody,
            funded.claimant,
            funded.authority,
        ],
        vec![funded.state.get_account_by_id(funded.authority).nonce],
        EscrowInstruction::ClaimNativeWitnessed { swap_id: [99; 32] },
    )
    .expect("serialize wrong message");
    assert!(!witness.is_valid_for(&wrong_message));
    assert!(
        funded
            .state
            .transition_from_public_transaction(
                &PublicTransaction::new(wrong_message, witness),
                3,
                102,
            )
            .is_err(),
        "aggregate signature for another exact LEZ message must fail",
    );
    assert_eq!(
        metadata(&funded.state, funded.metadata_id).status,
        EscrowStatus::Funded
    );

    let mut funded = funded_state(&aggregate);
    let bypass_message = Message::try_new(
        ZEC_ESCROW_V02_ID,
        vec![funded.metadata_id, funded.custody, funded.claimant],
        vec![funded.state.get_account_by_id(funded.claimant).nonce],
        EscrowInstruction::ClaimNative {
            swap_id: SWAP_ID,
            preimage: [7; 32],
        },
    )
    .expect("serialize preimage bypass attempt");
    let bypass_witness = WitnessSet::for_message(&bypass_message, &[&funded.claimant_key]);
    assert!(bypass_witness.is_valid_for(&bypass_message));
    assert!(
        funded
            .state
            .transition_from_public_transaction(
                &PublicTransaction::new(bypass_message, bypass_witness),
                3,
                102,
            )
            .is_err(),
        "aggregate-authority escrow must not be claimable through the preimage instruction",
    );
    assert_eq!(
        metadata(&funded.state, funded.metadata_id).status,
        EscrowStatus::Funded
    );
}

fn authorize_xmr_claim(funded: &mut XmrFundedFixture) {
    let authorization = transaction(
        &funded.state,
        vec![funded.metadata_id, funded.depositor],
        &[(funded.depositor, &funded.depositor_key)],
        EscrowInstruction::AuthorizeNativeXmrClaim {
            swap_id: SWAP_ID,
            claim_partial: CLAIM_PARTIAL,
        },
    );
    funded
        .state
        .transition_from_public_transaction(&authorization, 3, 102)
        .expect("Taker's exact context-bound claim partial authorizes the claim branch");
    assert_eq!(
        metadata(&funded.state, funded.metadata_id).status,
        EscrowStatus::XmrClaimAuthorized
    );
    assert_eq!(
        funded.state.get_account_by_id(funded.custody).balance,
        AMOUNT
    );
    assert_eq!(
        funded.state.get_account_by_id(funded.depositor).balance,
        200 - AMOUNT
    );
    assert_eq!(
        funded.state.get_account_by_id(funded.depositor).nonce.0,
        3,
        "initialize, fund, and claim authorization are the Taker's three signed messages",
    );
}

#[test]
fn recursive_xmr_claim_requires_taker_authorization_and_commits_exact_transfer() {
    let mut funded = xmr_funded_state(10);
    let claim = aggregate_transaction(
        &funded.state,
        vec![
            funded.metadata_id,
            funded.custody,
            funded.claimant,
            funded.claim_aggregate.authority,
        ],
        &funded.claim_aggregate,
        EscrowInstruction::ClaimNativeXmr { swap_id: SWAP_ID },
    );
    assert!(
        funded
            .state
            .transition_from_public_transaction(&claim, 3, 102)
            .is_err(),
        "the aggregate claim cannot reveal its share before the Taker authorizes the committed partial",
    );
    assert_eq!(
        metadata(&funded.state, funded.metadata_id).status,
        EscrowStatus::Funded
    );
    assert_eq!(
        funded.state.get_account_by_id(funded.custody).balance,
        AMOUNT
    );
    assert_eq!(funded.state.get_account_by_id(funded.claimant).balance, 10);
    assert_eq!(
        funded
            .state
            .get_account_by_id(funded.claim_aggregate.authority)
            .nonce
            .0,
        0,
        "a rejected aggregate claim does not consume the authority nonce",
    );

    let wrong_authorization = transaction(
        &funded.state,
        vec![funded.metadata_id, funded.depositor],
        &[(funded.depositor, &funded.depositor_key)],
        EscrowInstruction::AuthorizeNativeXmrClaim {
            swap_id: SWAP_ID,
            claim_partial: [0x97; 32],
        },
    );
    assert!(
        funded
            .state
            .transition_from_public_transaction(&wrong_authorization, 3, 102)
            .is_err(),
        "a Taker signature cannot authorize a partial outside the initialization commitment",
    );
    assert_eq!(
        metadata(&funded.state, funded.metadata_id).status,
        EscrowStatus::Funded
    );
    assert_eq!(funded.state.get_account_by_id(funded.depositor).nonce.0, 2);

    authorize_xmr_claim(&mut funded);
    let claim = aggregate_transaction(
        &funded.state,
        vec![
            funded.metadata_id,
            funded.custody,
            funded.claimant,
            funded.claim_aggregate.authority,
        ],
        &funded.claim_aggregate,
        EscrowInstruction::ClaimNativeXmr { swap_id: SWAP_ID },
    );
    funded
        .state
        .transition_from_public_transaction(&claim, 4, REFUND_AT - 1)
        .expect("authorized aggregate claim recursively commits metadata and custody transfer");
    assert_eq!(
        metadata(&funded.state, funded.metadata_id).status,
        EscrowStatus::Claimed
    );
    assert_eq!(funded.state.get_account_by_id(funded.custody).balance, 0);
    assert_eq!(
        funded.state.get_account_by_id(funded.claimant).balance,
        10 + AMOUNT
    );
    assert_eq!(
        funded.state.get_account_by_id(funded.depositor).balance,
        200 - AMOUNT
    );
    assert_eq!(
        funded
            .state
            .get_account_by_id(funded.claim_aggregate.authority)
            .nonce
            .0,
        1,
    );
    assert_eq!(
        funded
            .state
            .get_account_by_id(funded.refund_aggregate.authority)
            .nonce
            .0,
        0,
        "the disjoint refund signing session is untouched by a claim",
    );
}

#[test]
fn recursive_xmr_refund_requires_its_distinct_aggregate_at_the_exact_boundary() {
    let mut funded = xmr_funded_state(10);
    let refund = aggregate_transaction(
        &funded.state,
        vec![
            funded.metadata_id,
            funded.custody,
            funded.depositor,
            funded.refund_aggregate.authority,
        ],
        &funded.refund_aggregate,
        EscrowInstruction::RefundNativeXmr { swap_id: SWAP_ID },
    );
    assert!(
        funded
            .state
            .transition_from_public_transaction(&refund, 3, REFUND_AT - 1)
            .is_err(),
        "the signed refund window is closed before the exact boundary",
    );
    assert_eq!(
        metadata(&funded.state, funded.metadata_id).status,
        EscrowStatus::Funded
    );
    assert_eq!(
        funded.state.get_account_by_id(funded.custody).balance,
        AMOUNT
    );

    funded
        .state
        .transition_from_public_transaction(&refund, 3, REFUND_AT)
        .expect("the distinct signed aggregate refund opens at the exact boundary");
    assert_eq!(
        metadata(&funded.state, funded.metadata_id).status,
        EscrowStatus::Refunded
    );
    assert_eq!(funded.state.get_account_by_id(funded.custody).balance, 0);
    assert_eq!(
        funded.state.get_account_by_id(funded.depositor).balance,
        200
    );
    assert_eq!(funded.state.get_account_by_id(funded.claimant).balance, 10);
    assert_eq!(
        funded
            .state
            .get_account_by_id(funded.refund_aggregate.authority)
            .nonce
            .0,
        1,
    );
    assert_eq!(
        funded
            .state
            .get_account_by_id(funded.claim_aggregate.authority)
            .nonce
            .0,
        0,
        "refund cannot consume or impersonate the claim authority",
    );
}

#[test]
fn recursive_xmr_maker_punishment_opens_at_the_exact_punish_boundary() {
    let mut funded = xmr_funded_state(10);
    let punish = transaction(
        &funded.state,
        vec![funded.metadata_id, funded.custody, funded.claimant],
        &[(funded.claimant, &funded.claimant_key)],
        EscrowInstruction::PunishNativeXmr { swap_id: SWAP_ID },
    );
    assert!(
        funded
            .state
            .transition_from_public_transaction(&punish, 3, PUNISH_AT - 1)
            .is_err(),
        "Maker punishment remains closed while the signed refund branch is live",
    );
    assert_eq!(
        metadata(&funded.state, funded.metadata_id).status,
        EscrowStatus::Funded
    );
    assert_eq!(
        funded.state.get_account_by_id(funded.custody).balance,
        AMOUNT
    );

    funded
        .state
        .transition_from_public_transaction(&punish, 3, PUNISH_AT)
        .expect("Maker punishment opens at the exact punish boundary");
    assert_eq!(
        metadata(&funded.state, funded.metadata_id).status,
        EscrowStatus::Claimed
    );
    assert_eq!(funded.state.get_account_by_id(funded.custody).balance, 0);
    assert_eq!(
        funded.state.get_account_by_id(funded.claimant).balance,
        10 + AMOUNT
    );
    assert_eq!(
        funded.state.get_account_by_id(funded.depositor).balance,
        200 - AMOUNT
    );
    assert_eq!(funded.state.get_account_by_id(funded.claimant).nonce.0, 1);
}

#[test]
fn recursive_xmr_claim_child_overflow_rolls_back_authorization_and_every_account() {
    let mut funded = xmr_funded_state(u128::MAX);
    authorize_xmr_claim(&mut funded);
    let before_metadata = funded.state.get_account_by_id(funded.metadata_id);
    let before_custody = funded.state.get_account_by_id(funded.custody);
    let before_claimant = funded.state.get_account_by_id(funded.claimant);
    let before_authority = funded
        .state
        .get_account_by_id(funded.claim_aggregate.authority);
    assert_eq!(
        metadata(&funded.state, funded.metadata_id).status,
        EscrowStatus::XmrClaimAuthorized
    );

    let claim = aggregate_transaction(
        &funded.state,
        vec![
            funded.metadata_id,
            funded.custody,
            funded.claimant,
            funded.claim_aggregate.authority,
        ],
        &funded.claim_aggregate,
        EscrowInstruction::ClaimNativeXmr { swap_id: SWAP_ID },
    );
    assert!(
        funded
            .state
            .transition_from_public_transaction(&claim, 4, REFUND_AT - 1)
            .is_err(),
        "authenticated-transfer rejects recipient overflow",
    );

    assert_eq!(
        funded.state.get_account_by_id(funded.metadata_id),
        before_metadata
    );
    assert_eq!(
        funded.state.get_account_by_id(funded.custody),
        before_custody
    );
    assert_eq!(
        funded.state.get_account_by_id(funded.claimant),
        before_claimant
    );
    assert_eq!(
        funded
            .state
            .get_account_by_id(funded.claim_aggregate.authority),
        before_authority,
    );
    assert_eq!(
        metadata(&funded.state, funded.metadata_id).status,
        EscrowStatus::XmrClaimAuthorized,
        "terminal metadata rolls back to the authorized branch state",
    );
    assert_eq!(
        funded.state.get_account_by_id(funded.custody).balance,
        AMOUNT
    );
    assert_eq!(
        funded.state.get_account_by_id(funded.claimant).balance,
        u128::MAX
    );
}
