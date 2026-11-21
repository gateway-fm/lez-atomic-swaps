//! Executor cycle counts for the escrow operations the RFP names (P1).
//!
//! Why here: the fixtures next door already build the exact public transactions
//! the swap submits, and `build.rs` with `LEZ_V02_PREBUILT_GUEST_ELF` proves the
//! embedded ELF is the deployed program before this test runs.
//!
//! The BTC pair uses the **witnessed** path: the escrow's claim authority is a
//! two-party MuSig2 aggregate account, not a SHA-256 preimage (that is the
//! older ZEC path this same program still serves). So the operations measured
//! here are `InitializeNativeWitnessed`, `FundNative`, `ClaimNativeWitnessed`
//! and `RefundNative`, which is what `finalized_asset_observation` matches.
//!
//! The chain executes a program through `lee`'s `Program::execute`, which is
//! crate-private, so this mirrors it exactly: same four inputs in the same
//! order, same executor, no proving. `lee`'s own `cycle_bench` measures its
//! built-ins the same way, so these numbers are comparable to the ones behind
//! the fee model.
//!
//! The XMR path's tags 13–17 are measured the same way in a second test.
//!
//!   cargo test -p lez-zec-escrow-v02-methods --test escrow_cycles -- --nocapture

use borsh::BorshDeserialize as _;
use lez_zec_escrow_v02::{
    ClaimAuthority, EscrowMetadata, EscrowStatus, Instruction as EscrowInstruction,
};
use lez_zec_escrow_v02_methods::{ZEC_ESCROW_V02_ELF, ZEC_ESCROW_V02_ID};
use musig2::secp::{Point, Scalar};
use musig2::{CompactSignature, FirstRound, KeyAggContext, PartialSignature, SecNonceSpices};
use nssa::{
    Account, AccountId, PrivateKey, ProgramId, PublicKey, PublicTransaction, Signature, V03State,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use nssa_core::account::AccountWithMetadata;
use risc0_zkvm::{ExecutorEnv, default_executor};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use spel_framework_core::pda::{compute_pda, seed_from_str};

/// The program id the deployment manifest pins. The escrow deployed on LEZ
/// testnet 0.2 (c22d61fc…, block 10566) is the program before the custody
/// fix of TM-0032; these cycles are measured on the pinned one.
const DEPLOYED_IMAGE_ID: &str = "ca66e0dc4118597e0a9010670fd49aec923bba9c332e9e6a26d9208dbdf6af98";
const DEPLOYED_PROGRAM_ID_WORDS: [u32; 8] = [
    3705693898, 2119768129, 1729138698, 3969569807, 2629450642, 1788751411, 2367740198, 2561668797,
];

/// The chain's ceiling for one public execution.
const MAX_NUM_CYCLES_PUBLIC_EXECUTION: u64 = 1024 * 1024 * 32;

const SWAP_ID: [u8; 32] = [81; 32];
const AMOUNT: u128 = 75;
const REFUND_AT: u64 = 10_000;
const MAKER_SECRET: [u8; 32] = [0x31; 32];
const TAKER_SECRET: [u8; 32] = [0x42; 32];
const PUNISH_AT: u64 = 12_000;
const REFUND_MAKER_SECRET: [u8; 32] = [0x53; 32];
const REFUND_TAKER_SECRET: [u8; 32] = [0x64; 32];
const CLAIM_PARTIAL_CONTEXT_BINDING: [u8; 32] = [0x85; 32];
const CLAIM_PARTIAL: [u8; 32] = [0x96; 32];
const CLAIM_PARTIAL_DOMAIN: &[u8] = b"logos.gateway.lez-xmr.claim-partial-commitment.v1\0";

struct Aggregate {
    maker_secret: Scalar,
    taker_secret: Scalar,
    context: KeyAggContext,
    point: Point,
    public_key: PublicKey,
    authority: AccountId,
}

fn actor(secret: [u8; 32]) -> (AccountId, PrivateKey) {
    let key = PrivateKey::try_new(secret).expect("deterministic test private key");
    (AccountId::from(&PublicKey::new_from_private_key(&key)), key)
}

/// The two-party MuSig2 key whose account authorises the witnessed claim.
fn aggregate() -> Aggregate {
    aggregate_from(MAKER_SECRET, TAKER_SECRET)
}

fn aggregate_from(maker_secret: [u8; 32], taker_secret: [u8; 32]) -> Aggregate {
    let maker_secret = Scalar::from_slice(&maker_secret).expect("maker scalar");
    let taker_secret = Scalar::from_slice(&taker_secret).expect("taker scalar");
    let context =
        KeyAggContext::new([maker_secret.base_point_mul(), taker_secret.base_point_mul()])
            .expect("BIP-327 two-party key aggregation");
    let point: Point = context.aggregated_pubkey();
    let public_key =
        PublicKey::try_new(point.serialize_xonly()).expect("MuSig2 aggregate is a BIP-340 key");
    let authority = AccountId::from(&public_key);
    Aggregate {
        maker_secret,
        taker_secret,
        context,
        point,
        public_key,
        authority,
    }
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

/// Both halves sign, exactly as Maker and Taker do for a real claim.
fn aggregate_witness(aggregate: &Aggregate, message: &Message) -> WitnessSet {
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
    let signature: CompactSignature = maker_second.finalize().expect("maker aggregate");
    musig2::verify_single(aggregate.point, signature, message_hash)
        .expect("completed aggregate signature verifies");
    WitnessSet::from_raw_parts(vec![(
        Signature {
            value: signature.serialize(),
        },
        aggregate.public_key.clone(),
    )])
}

fn escrow_ids() -> (AccountId, AccountId) {
    let metadata = compute_pda(&ZEC_ESCROW_V02_ID, &[&SWAP_ID]);
    let custody = compute_pda(&ZEC_ESCROW_V02_ID, &[&seed_from_str("custody"), &SWAP_ID]);
    (metadata, custody)
}

fn metadata(state: &V03State, account_id: AccountId) -> EscrowMetadata {
    EscrowMetadata::try_from_slice(state.get_account_by_id(account_id).data.as_ref())
        .expect("state stores canonical escrow metadata")
}

/// Executor cycles for one instruction against the given pre-state accounts.
///
/// Mirrors `lee`'s `Program::execute`: program id, caller (none: the swap calls
/// the escrow directly), pre-states, then instruction words.
fn cycles<T: Serialize>(
    state: &V03State,
    account_ids: &[AccountId],
    signers: &[AccountId],
    instruction: &T,
) -> (u64, usize) {
    let program = Program::new(ZEC_ESCROW_V02_ELF.into()).expect("canonical guest ELF");
    // `is_authorized` is "this account signed the transaction", which is how the
    // state machine fills pre-states before it calls the program.
    let pre_states = account_ids
        .iter()
        .map(|account_id| {
            AccountWithMetadata::new(
                state.get_account_by_id(*account_id),
                signers.contains(account_id),
                *account_id,
            )
        })
        .collect::<Vec<_>>();
    let instruction_words =
        Program::serialize_instruction(instruction).expect("serializable instruction");

    let mut builder = ExecutorEnv::builder();
    builder
        .write(&program.id())
        .expect("write program id")
        .write(&None::<ProgramId>)
        .expect("write caller program id")
        .write(&pre_states)
        .expect("write pre-states")
        .write(&instruction_words)
        .expect("write instruction data");
    let env = builder.build().expect("executor environment");

    let info = default_executor()
        .execute(env, program.elf())
        .expect("the escrow guest executes");
    (info.cycles(), info.segments.len())
}

/// The ZEC pair's SHA-256 hash/time lock on the same deployed program, in
/// either direction: the role that sells LEZ locks it with `InitializeNative`
/// and `FundNative`, the LEZ buyer claims with `ClaimNative` by revealing the
/// preimage, and `RefundNative` is the other ending.
/// Inputs and account orders are the ones `recursive_atomicity.rs` and the
/// sidecar's `prepare_native_escrow`, `prepare_revealing_claim` and
/// `prepare_native_refund` build.
#[test]
fn zec_escrow_operations_report_executor_cycles_for_the_deployed_program() {
    const PREIMAGE: [u8; 32] = [12; 32];

    let escrow = Program::new(ZEC_ESCROW_V02_ELF.into()).expect("canonical guest ELF");
    assert_eq!(escrow.id(), ZEC_ESCROW_V02_ID);
    assert_eq!(
        ZEC_ESCROW_V02_ID, DEPLOYED_PROGRAM_ID_WORDS,
        "these cycles must describe the program the deployment manifest pins"
    );

    let authenticated_transfer = programs::authenticated_transfer();
    let authenticated_transfer_id = authenticated_transfer.id();
    let (depositor, depositor_key) = actor([1; 32]);
    let (claimant, claimant_key) = actor([2; 32]);
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
        ])
        .with_programs([escrow, authenticated_transfer]);
    let (metadata_id, custody) = escrow_ids();

    // Initialise (0): four accounts, the depositor signs.
    let initialize = EscrowInstruction::InitializeNative {
        swap_id: SWAP_ID,
        terms_hash: [31; 32],
        secret_digest: Sha256::digest(PREIMAGE).into(),
        amount: AMOUNT,
        refund_at: REFUND_AT,
        authenticated_transfer_program: authenticated_transfer_id,
    };
    let initialize_accounts = vec![metadata_id, custody, depositor, claimant];
    let (initialize_cycles, initialize_segments) =
        cycles(&state, &initialize_accounts, &[depositor], &initialize);
    state
        .transition_from_public_transaction(
            &transaction(
                &state,
                initialize_accounts,
                &[(depositor, &depositor_key)],
                initialize,
            ),
            1,
            100,
        )
        .expect("preimage escrow initializes recursively");
    assert_eq!(
        metadata(&state, metadata_id).claim_authority,
        ClaimAuthority::Sha256Preimage {
            secret_digest: Sha256::digest(PREIMAGE).into(),
        }
    );

    // Fund (2, shared with the other pairs): three accounts, the depositor signs.
    let fund = EscrowInstruction::FundNative { swap_id: SWAP_ID };
    let fund_accounts = vec![metadata_id, custody, depositor];
    let (fund_cycles, fund_segments) = cycles(&state, &fund_accounts, &[depositor], &fund);
    state
        .transition_from_public_transaction(
            &transaction(&state, fund_accounts, &[(depositor, &depositor_key)], fund),
            2,
            101,
        )
        .expect("preimage escrow funds recursively");
    assert_eq!(metadata(&state, metadata_id).status, EscrowStatus::Funded);

    // Claim (3) and refund (5) are alternative endings, each on the funded state.
    let claim = EscrowInstruction::ClaimNative {
        swap_id: SWAP_ID,
        preimage: PREIMAGE,
    };
    let claim_accounts = vec![metadata_id, custody, claimant];
    let (claim_cycles, claim_segments) = cycles(&state, &claim_accounts, &[claimant], &claim);
    let mut claim_state = state.clone();
    claim_state
        .transition_from_public_transaction(
            &transaction(
                &claim_state,
                claim_accounts,
                &[(claimant, &claimant_key)],
                claim,
            ),
            3,
            REFUND_AT - 1,
        )
        .expect("the revealed preimage claims recursively");
    assert_eq!(
        metadata(&claim_state, metadata_id).status,
        EscrowStatus::Claimed
    );

    // Permissionless at the boundary: no signer authorises it.
    let refund = EscrowInstruction::RefundNative { swap_id: SWAP_ID };
    let refund_accounts = vec![metadata_id, custody, depositor];
    let (refund_cycles, refund_segments) = cycles(&state, &refund_accounts, &[], &refund);
    let mut refund_state = state.clone();
    refund_state
        .transition_from_public_transaction(
            &transaction(&refund_state, refund_accounts, &[], refund),
            3,
            REFUND_AT,
        )
        .expect("fixed-destination refund is permissionless at the boundary");
    assert_eq!(
        metadata(&refund_state, metadata_id).status,
        EscrowStatus::Refunded
    );

    report(
        "ZEC escrow",
        &[
            ("InitializeNative", initialize_cycles, initialize_segments),
            ("FundNative", fund_cycles, fund_segments),
            ("ClaimNative", claim_cycles, claim_segments),
            ("RefundNative", refund_cycles, refund_segments),
        ],
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one linear measurement per operation"
)]
fn escrow_operations_report_executor_cycles_for_the_deployed_program() {
    let escrow = Program::new(ZEC_ESCROW_V02_ELF.into()).expect("canonical guest ELF");
    assert_eq!(escrow.id(), ZEC_ESCROW_V02_ID);
    assert_eq!(
        ZEC_ESCROW_V02_ID, DEPLOYED_PROGRAM_ID_WORDS,
        "these cycles must describe the program the deployment manifest pins"
    );

    let aggregate = aggregate();
    let authenticated_transfer = programs::authenticated_transfer();
    let authenticated_transfer_id = authenticated_transfer.id();
    let (depositor, depositor_key) = actor([1; 32]);
    let (claimant, _claimant_key) = actor([2; 32]);
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
        ])
        .with_programs([escrow, authenticated_transfer]);
    let (metadata_id, custody) = escrow_ids();

    // Initialise: five accounts, the depositor signs.
    let initialize = EscrowInstruction::InitializeNativeWitnessed {
        swap_id: SWAP_ID,
        terms_hash: [31; 32],
        aggregate_x_only_public_key: *aggregate.public_key.value(),
        amount: AMOUNT,
        refund_at: REFUND_AT,
        authenticated_transfer_program: authenticated_transfer_id,
    };
    let initialize_accounts = vec![
        metadata_id,
        custody,
        depositor,
        claimant,
        aggregate.authority,
    ];
    let (initialize_cycles, initialize_segments) =
        cycles(&state, &initialize_accounts, &[depositor], &initialize);
    state
        .transition_from_public_transaction(
            &transaction(
                &state,
                initialize_accounts,
                &[(depositor, &depositor_key)],
                initialize,
            ),
            1,
            100,
        )
        .expect("aggregate-authority escrow initializes recursively");
    assert_eq!(
        metadata(&state, metadata_id).claim_authority,
        ClaimAuthority::AggregateWitness {
            x_only_public_key: *aggregate.public_key.value(),
            account_id: aggregate.authority,
        }
    );

    // Fund: three accounts, the depositor signs.
    let fund = EscrowInstruction::FundNative { swap_id: SWAP_ID };
    let fund_accounts = vec![metadata_id, custody, depositor];
    let (fund_cycles, fund_segments) = cycles(&state, &fund_accounts, &[depositor], &fund);
    state
        .transition_from_public_transaction(
            &transaction(&state, fund_accounts, &[(depositor, &depositor_key)], fund),
            2,
            101,
        )
        .expect("aggregate-authority escrow funds recursively");
    assert_eq!(metadata(&state, metadata_id).status, EscrowStatus::Funded);

    // Claim and refund are alternative endings, so each runs on the funded state.
    let claim = EscrowInstruction::ClaimNativeWitnessed { swap_id: SWAP_ID };
    let claim_accounts = vec![metadata_id, custody, claimant, aggregate.authority];
    let (claim_cycles, claim_segments) =
        cycles(&state, &claim_accounts, &[aggregate.authority], &claim);
    let claim_message = Message::try_new(
        ZEC_ESCROW_V02_ID,
        claim_accounts,
        vec![state.get_account_by_id(aggregate.authority).nonce],
        claim,
    )
    .expect("serialize witnessed claim");
    let claim_witness = aggregate_witness(&aggregate, &claim_message);
    let mut claim_state = state.clone();
    claim_state
        .transition_from_public_transaction(
            &PublicTransaction::new(claim_message, claim_witness),
            3,
            102,
        )
        .expect("completed two-party aggregate witness claims recursively");
    assert_eq!(
        metadata(&claim_state, metadata_id).status,
        EscrowStatus::Claimed
    );

    // Permissionless at the boundary: no signer authorises it.
    let refund = EscrowInstruction::RefundNative { swap_id: SWAP_ID };
    let refund_accounts = vec![metadata_id, custody, depositor];
    let (refund_cycles, refund_segments) = cycles(&state, &refund_accounts, &[], &refund);
    let mut refund_state = state.clone();
    refund_state
        .transition_from_public_transaction(
            &transaction(&refund_state, refund_accounts, &[], refund),
            3,
            REFUND_AT,
        )
        .expect("fixed-destination refund is permissionless at the boundary");
    assert_eq!(
        metadata(&refund_state, metadata_id).status,
        EscrowStatus::Refunded
    );

    report(
        "escrow",
        &[
            (
                "InitializeNativeWitnessed",
                initialize_cycles,
                initialize_segments,
            ),
            ("FundNative", fund_cycles, fund_segments),
            ("ClaimNativeWitnessed", claim_cycles, claim_segments),
            ("RefundNative", refund_cycles, refund_segments),
        ],
    );
}

fn report(title: &str, rows: &[(&str, u64, usize)]) {
    println!("\n{title} executor cycles (ImageID {DEPLOYED_IMAGE_ID})");
    println!(
        "{:<26} {:>12} {:>10}",
        "operation", "user cycles", "segments"
    );
    for &(name, count, segments) in rows {
        let share = count as f64 * 100.0 / MAX_NUM_CYCLES_PUBLIC_EXECUTION as f64;
        println!("{name:<26} {count:>12} {segments:>10}   {share:.2}% of the limit");
        assert!(
            count < MAX_NUM_CYCLES_PUBLIC_EXECUTION,
            "{name} exceeds the public execution cycle limit"
        );
    }
}

fn claim_partial_commitment() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(CLAIM_PARTIAL_DOMAIN);
    hasher.update(CLAIM_PARTIAL_CONTEXT_BINDING);
    hasher.update(CLAIM_PARTIAL);
    hasher.finalize().into()
}

/// One aggregate-authority transaction, signed by both halves.
fn aggregate_transaction<T: Serialize>(
    state: &V03State,
    account_ids: Vec<AccountId>,
    aggregate: &Aggregate,
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

/// The XMR path (tags 13–17) of the same deployed program: the Taker locks
/// LEZ with `InitializeNativeXmr` and the shared `FundNative`, publishes its
/// committed claim partial with `AuthorizeNativeXmrClaim`, and the escrow
/// ends in `ClaimNativeXmr` (claim aggregate), `RefundNativeXmr` (the distinct
/// refund aggregate) or `PunishNativeXmr` (the Maker, after the punish time).
/// Inputs and account orders are the ones `recursive_witnessed_claim.rs` and
/// the sidecar's native-XMR planners build.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one linear measurement per operation"
)]
fn xmr_escrow_operations_report_executor_cycles_for_the_deployed_program() {
    let escrow = Program::new(ZEC_ESCROW_V02_ELF.into()).expect("canonical guest ELF");
    assert_eq!(escrow.id(), ZEC_ESCROW_V02_ID);
    assert_eq!(
        ZEC_ESCROW_V02_ID, DEPLOYED_PROGRAM_ID_WORDS,
        "these cycles must describe the program the deployment manifest pins"
    );

    let claim_aggregate = aggregate();
    let refund_aggregate = aggregate_from(REFUND_MAKER_SECRET, REFUND_TAKER_SECRET);
    let authenticated_transfer = programs::authenticated_transfer();
    let authenticated_transfer_id = authenticated_transfer.id();
    // The Taker deposits LEZ; the Maker is the claimant.
    let (depositor, depositor_key) = actor([1; 32]);
    let (claimant, claimant_key) = actor([2; 32]);
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
            (claim_aggregate.authority, Account::default()),
            (refund_aggregate.authority, Account::default()),
        ])
        .with_programs([escrow, authenticated_transfer]);
    let (metadata_id, custody) = escrow_ids();

    // Initialise (13): six accounts, the depositor signs.
    let initialize = EscrowInstruction::InitializeNativeXmr {
        swap_id: SWAP_ID,
        terms_hash: [31; 32],
        claim_aggregate_x_only_public_key: *claim_aggregate.public_key.value(),
        refund_aggregate_x_only_public_key: *refund_aggregate.public_key.value(),
        maker_dleq_transcript_commitment: [0xa7; 32],
        taker_dleq_transcript_commitment: [0xb8; 32],
        claim_partial_context_binding: CLAIM_PARTIAL_CONTEXT_BINDING,
        claim_partial_commitment: claim_partial_commitment(),
        amount: AMOUNT,
        refund_at: REFUND_AT,
        punish_at: PUNISH_AT,
        authenticated_transfer_program: authenticated_transfer_id,
    };
    let initialize_accounts = vec![
        metadata_id,
        custody,
        depositor,
        claimant,
        claim_aggregate.authority,
        refund_aggregate.authority,
    ];
    let (initialize_cycles, initialize_segments) =
        cycles(&state, &initialize_accounts, &[depositor], &initialize);
    state
        .transition_from_public_transaction(
            &transaction(
                &state,
                initialize_accounts,
                &[(depositor, &depositor_key)],
                initialize,
            ),
            1,
            100,
        )
        .expect("version-3 XMR escrow initializes recursively");
    assert_eq!(metadata(&state, metadata_id).version, 3);

    // Fund (2, shared with the other pairs): three accounts, the depositor signs.
    let fund = EscrowInstruction::FundNative { swap_id: SWAP_ID };
    let fund_accounts = vec![metadata_id, custody, depositor];
    let (fund_cycles, fund_segments) = cycles(&state, &fund_accounts, &[depositor], &fund);
    state
        .transition_from_public_transaction(
            &transaction(&state, fund_accounts, &[(depositor, &depositor_key)], fund),
            2,
            101,
        )
        .expect("version-3 XMR escrow funds recursively");
    assert_eq!(metadata(&state, metadata_id).status, EscrowStatus::Funded);

    // Authorise the claim (14): two accounts, the depositor signs.
    let authorize = EscrowInstruction::AuthorizeNativeXmrClaim {
        swap_id: SWAP_ID,
        claim_partial: CLAIM_PARTIAL,
    };
    let authorize_accounts = vec![metadata_id, depositor];
    let (authorize_cycles, authorize_segments) =
        cycles(&state, &authorize_accounts, &[depositor], &authorize);
    let mut authorized_state = state.clone();
    authorized_state
        .transition_from_public_transaction(
            &transaction(
                &authorized_state,
                authorize_accounts,
                &[(depositor, &depositor_key)],
                authorize,
            ),
            3,
            102,
        )
        .expect("the committed claim partial authorizes the claim branch");
    assert_eq!(
        metadata(&authorized_state, metadata_id).status,
        EscrowStatus::XmrClaimAuthorized
    );

    // Claim (15) on the authorised state: four accounts, the claim aggregate signs.
    let claim = EscrowInstruction::ClaimNativeXmr { swap_id: SWAP_ID };
    let claim_accounts = vec![metadata_id, custody, claimant, claim_aggregate.authority];
    let (claim_cycles, claim_segments) = cycles(
        &authorized_state,
        &claim_accounts,
        &[claim_aggregate.authority],
        &claim,
    );
    let mut claim_state = authorized_state.clone();
    claim_state
        .transition_from_public_transaction(
            &aggregate_transaction(&claim_state, claim_accounts, &claim_aggregate, claim),
            4,
            REFUND_AT - 1,
        )
        .expect("authorized aggregate claim commits recursively");
    assert_eq!(
        metadata(&claim_state, metadata_id).status,
        EscrowStatus::Claimed
    );

    // Refund (16) and punish (17) are the other endings, each on the funded state.
    let refund = EscrowInstruction::RefundNativeXmr { swap_id: SWAP_ID };
    let refund_accounts = vec![metadata_id, custody, depositor, refund_aggregate.authority];
    let (refund_cycles, refund_segments) = cycles(
        &state,
        &refund_accounts,
        &[refund_aggregate.authority],
        &refund,
    );
    let mut refund_state = state.clone();
    refund_state
        .transition_from_public_transaction(
            &aggregate_transaction(&refund_state, refund_accounts, &refund_aggregate, refund),
            3,
            REFUND_AT,
        )
        .expect("the distinct refund aggregate refunds at the boundary");
    assert_eq!(
        metadata(&refund_state, metadata_id).status,
        EscrowStatus::Refunded
    );

    let punish = EscrowInstruction::PunishNativeXmr { swap_id: SWAP_ID };
    let punish_accounts = vec![metadata_id, custody, claimant];
    let (punish_cycles, punish_segments) = cycles(&state, &punish_accounts, &[claimant], &punish);
    let mut punish_state = state.clone();
    punish_state
        .transition_from_public_transaction(
            &transaction(
                &punish_state,
                punish_accounts,
                &[(claimant, &claimant_key)],
                punish,
            ),
            3,
            PUNISH_AT,
        )
        .expect("the Maker punishes at the punish boundary");
    assert_eq!(
        metadata(&punish_state, metadata_id).status,
        EscrowStatus::Claimed
    );

    report(
        "XMR escrow",
        &[
            (
                "InitializeNativeXmr",
                initialize_cycles,
                initialize_segments,
            ),
            ("FundNative", fund_cycles, fund_segments),
            (
                "AuthorizeNativeXmrClaim",
                authorize_cycles,
                authorize_segments,
            ),
            ("ClaimNativeXmr", claim_cycles, claim_segments),
            ("RefundNativeXmr", refund_cycles, refund_segments),
            ("PunishNativeXmr", punish_cycles, punish_segments),
        ],
    );
}
