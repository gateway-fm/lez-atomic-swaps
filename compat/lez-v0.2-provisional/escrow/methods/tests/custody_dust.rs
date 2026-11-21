//! A third party credits a live escrow's custody account (TM-0032).
//!
//! LEZ lets anyone raise the balance of any initialised account: the
//! authenticated-transfer program checks only that the sender authorised the
//! transfer, the token program checks only the sender's authorisation and the
//! definition, and `validate_execution` restricts balance decreases, never
//! increases. So anyone holding one unit of the escrowed asset can send it to
//! the custody account of a funded escrow, at a moment of their choosing.
//!
//! These tests run that transfer as a public transaction against the real
//! guest, then drive every ending of every pair: the ZEC preimage claim, the
//! BTC aggregate claim, the shared native refund, XMR tags 15, 16 and 17, and
//! the SHA-256 and witnessed token claims and the token refund. Each ending
//! must still commit and must leave the custody empty, paying the whole
//! balance, the escrowed amount and the surplus, to that ending's recipient.
//!
//! The mirror cases are here too: a credit between initialisation and funding,
//! a custody ATA a third party creates before `CreateTokenCustody`, and a
//! credit to a native custody that is not initialised yet, which LEZ refuses.
//!
//!   cargo test -p lez-zec-escrow-v02-methods --test custody_dust

use borsh::BorshDeserialize as _;
use lez_zec_escrow_v02::{EscrowMetadata, EscrowStatus, Instruction as EscrowInstruction};
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
use token_core::TokenHolding;

const SWAP_ID: [u8; 32] = [91; 32];
const PREIMAGE: [u8; 32] = [12; 32];
const AMOUNT: u128 = 75;
const DUST: u128 = 1;
const ACTOR_FUNDS: u128 = 500;
const REFUND_AT: u64 = 10_000;
const PUNISH_AT: u64 = 12_000;
const CLAIM_PARTIAL_CONTEXT_BINDING: [u8; 32] = [0x85; 32];
const CLAIM_PARTIAL: [u8; 32] = [0x96; 32];
const CLAIM_PARTIAL_DOMAIN: &[u8] = b"logos.gateway.lez-xmr.claim-partial-commitment.v1\0";

/// The wire shape of LEZ v0.2.4's `authenticated_transfer_core::Instruction`
/// (`lez/programs/authenticated_transfer/core/src/lib.rs`): same variants in
/// the same order, so the same risc0 serde words. Only the variant the
/// attacker sends is used.
#[derive(Serialize)]
#[allow(dead_code)]
enum AuthenticatedTransferInstruction {
    Transfer { amount: u128 },
    Initialize,
}

struct Aggregate {
    maker_secret: Scalar,
    taker_secret: Scalar,
    context: KeyAggContext,
    point: Point,
    public_key: PublicKey,
    authority: AccountId,
}

fn actor(key_byte: u8) -> (AccountId, PrivateKey) {
    let key = PrivateKey::try_new([key_byte; 32]).expect("deterministic test private key");
    (AccountId::from(&PublicKey::new_from_private_key(&key)), key)
}

fn aggregate(maker: u8, taker: u8) -> Aggregate {
    let maker_secret = Scalar::from_slice(&[maker; 32]).expect("maker scalar");
    let taker_secret = Scalar::from_slice(&[taker; 32]).expect("taker scalar");
    let context =
        KeyAggContext::new([maker_secret.base_point_mul(), taker_secret.base_point_mul()])
            .expect("BIP-327 two-party key aggregation");
    let point: Point = context.aggregated_pubkey();
    let public_key =
        PublicKey::try_new(point.serialize_xonly()).expect("MuSig2 aggregate is a LEZ key");
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

fn transaction<T: Serialize>(
    state: &V03State,
    program_id: [u32; 8],
    account_ids: Vec<AccountId>,
    signers: &[(AccountId, &PrivateKey)],
    instruction: T,
) -> PublicTransaction {
    let nonces = signers
        .iter()
        .map(|(account_id, _)| state.get_account_by_id(*account_id).nonce)
        .collect();
    let message = Message::try_new(program_id, account_ids, nonces, instruction)
        .expect("serialize exact instruction");
    let keys = signers.iter().map(|(_, key)| *key).collect::<Vec<_>>();
    PublicTransaction::new(message.clone(), WitnessSet::for_message(&message, &keys))
}

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
    .expect("serialize exact aggregate-authority instruction");
    let witness = aggregate_witness(aggregate, &message);
    PublicTransaction::new(message, witness)
}

fn native_ids() -> (AccountId, AccountId) {
    let metadata = compute_pda(&ZEC_ESCROW_V02_ID, &[&SWAP_ID]);
    let custody = compute_pda(&ZEC_ESCROW_V02_ID, &[&seed_from_str("custody"), &SWAP_ID]);
    (metadata, custody)
}

fn status(state: &V03State, metadata: AccountId) -> EscrowStatus {
    EscrowMetadata::try_from_slice(state.get_account_by_id(metadata).data.as_ref())
        .expect("state stores canonical escrow metadata")
        .status
}

fn balance(state: &V03State, account_id: AccountId) -> u128 {
    state.get_account_by_id(account_id).balance
}

fn claim_partial_commitment() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(CLAIM_PARTIAL_DOMAIN);
    hasher.update(CLAIM_PARTIAL_CONTEXT_BINDING);
    hasher.update(CLAIM_PARTIAL);
    hasher.finalize().into()
}

/// The native pairs' chain: escrow and authenticated transfer, a depositor,
/// a claimant and a third party who holds a little LEZ of its own.
struct Native {
    state: V03State,
    metadata: AccountId,
    custody: AccountId,
    depositor: AccountId,
    depositor_key: PrivateKey,
    claimant: AccountId,
    claimant_key: PrivateKey,
    stranger: AccountId,
    stranger_key: PrivateKey,
    transfer_program: [u32; 8],
    block: u64,
}

impl Native {
    fn new(authorities: &[AccountId]) -> Self {
        let escrow = Program::new(ZEC_ESCROW_V02_ELF.into()).expect("checked guest ELF");
        assert_eq!(escrow.id(), ZEC_ESCROW_V02_ID);
        let authenticated_transfer = programs::authenticated_transfer();
        let transfer_program = authenticated_transfer.id();
        let (depositor, depositor_key) = actor(1);
        let (claimant, claimant_key) = actor(2);
        let (stranger, stranger_key) = actor(3);
        let owned = |balance| Account {
            program_owner: transfer_program,
            balance,
            ..Account::default()
        };
        let mut accounts = vec![
            (depositor, owned(200)),
            (claimant, owned(10)),
            (stranger, owned(5)),
        ];
        accounts.extend(authorities.iter().map(|id| (*id, Account::default())));
        let state = V03State::new()
            .with_public_accounts(accounts)
            .with_programs([escrow, authenticated_transfer]);
        let (metadata, custody) = native_ids();
        Self {
            state,
            metadata,
            custody,
            depositor,
            depositor_key,
            claimant,
            claimant_key,
            stranger,
            stranger_key,
            transfer_program,
            block: 1,
        }
    }

    /// The same chain from this point on, for an alternative ending.
    fn branch(&self) -> Self {
        Self {
            state: self.state.clone(),
            metadata: self.metadata,
            custody: self.custody,
            depositor: self.depositor,
            depositor_key: PrivateKey::try_new([1; 32]).expect("depositor key"),
            claimant: self.claimant,
            claimant_key: PrivateKey::try_new([2; 32]).expect("claimant key"),
            stranger: self.stranger,
            stranger_key: PrivateKey::try_new([3; 32]).expect("stranger key"),
            transfer_program: self.transfer_program,
            block: self.block,
        }
    }

    fn submit(&mut self, transaction: &PublicTransaction, timestamp: u64) -> Result<(), String> {
        let result = self
            .state
            .transition_from_public_transaction(transaction, self.block, timestamp)
            .map(|_| ())
            .map_err(|error| format!("{error:?}"));
        self.block += 1;
        result
    }

    /// What anyone with LEZ can do: a plain transfer it signs alone.
    fn stranger_credits_custody(&mut self) -> Result<(), String> {
        let credit = transaction(
            &self.state,
            self.transfer_program,
            vec![self.stranger, self.custody],
            &[(self.stranger, &self.stranger_key)],
            AuthenticatedTransferInstruction::Transfer { amount: DUST },
        );
        self.submit(&credit, 100)
    }

    fn initialize(&mut self, instruction: EscrowInstruction, extra: &[AccountId]) {
        let mut accounts = vec![self.metadata, self.custody, self.depositor, self.claimant];
        accounts.extend_from_slice(extra);
        let initialize = transaction(
            &self.state,
            ZEC_ESCROW_V02_ID,
            accounts,
            &[(self.depositor, &self.depositor_key)],
            instruction,
        );
        self.submit(&initialize, 100)
            .expect("the escrow initializes");
    }

    fn fund(&mut self) -> Result<(), String> {
        let fund = transaction(
            &self.state,
            ZEC_ESCROW_V02_ID,
            vec![self.metadata, self.custody, self.depositor],
            &[(self.depositor, &self.depositor_key)],
            EscrowInstruction::FundNative { swap_id: SWAP_ID },
        );
        self.submit(&fund, 101)
    }

    fn assert_swept_to(&self, recipient: AccountId, before: u128, terminal: EscrowStatus) {
        assert_eq!(status(&self.state, self.metadata), terminal);
        assert_eq!(balance(&self.state, self.custody), 0, "custody is empty");
        assert_eq!(
            balance(&self.state, recipient),
            before + AMOUNT + DUST,
            "the recipient receives the amount and the surplus"
        );
    }

    fn native_refund(mut self) {
        let before = balance(&self.state, self.depositor);
        let refund = transaction(
            &self.state,
            ZEC_ESCROW_V02_ID,
            vec![self.metadata, self.custody, self.depositor],
            &[],
            EscrowInstruction::RefundNative { swap_id: SWAP_ID },
        );
        self.submit(&refund, REFUND_AT)
            .expect("the refund commits on a credited custody");
        self.assert_swept_to(self.depositor, before, EscrowStatus::Refunded);
    }
}

fn zec_native() -> Native {
    let mut chain = Native::new(&[]);
    chain.initialize(
        EscrowInstruction::InitializeNative {
            swap_id: SWAP_ID,
            terms_hash: [31; 32],
            secret_digest: Sha256::digest(PREIMAGE).into(),
            amount: AMOUNT,
            refund_at: REFUND_AT,
            authenticated_transfer_program: programs::authenticated_transfer().id(),
        },
        &[],
    );
    chain
}

fn btc_native(aggregate: &Aggregate) -> Native {
    let mut chain = Native::new(&[aggregate.authority]);
    chain.initialize(
        EscrowInstruction::InitializeNativeWitnessed {
            swap_id: SWAP_ID,
            terms_hash: [31; 32],
            aggregate_x_only_public_key: *aggregate.public_key.value(),
            amount: AMOUNT,
            refund_at: REFUND_AT,
            authenticated_transfer_program: programs::authenticated_transfer().id(),
        },
        &[aggregate.authority],
    );
    chain
}

fn xmr_native(claim: &Aggregate, refund: &Aggregate) -> Native {
    let mut chain = Native::new(&[claim.authority, refund.authority]);
    chain.initialize(
        EscrowInstruction::InitializeNativeXmr {
            swap_id: SWAP_ID,
            terms_hash: [31; 32],
            claim_aggregate_x_only_public_key: *claim.public_key.value(),
            refund_aggregate_x_only_public_key: *refund.public_key.value(),
            maker_dleq_transcript_commitment: [0xa7; 32],
            taker_dleq_transcript_commitment: [0xb8; 32],
            claim_partial_context_binding: CLAIM_PARTIAL_CONTEXT_BINDING,
            claim_partial_commitment: claim_partial_commitment(),
            amount: AMOUNT,
            refund_at: REFUND_AT,
            punish_at: PUNISH_AT,
            authenticated_transfer_program: programs::authenticated_transfer().id(),
        },
        &[claim.authority, refund.authority],
    );
    chain
}

/// LEZ refuses the credit before `Initialize`: the transfer would have to
/// claim the default custody account for the authenticated-transfer program,
/// and `Claim::Authorized` needs the account's own authorisation, which only
/// the escrow can give its PDA. So the native custody cannot be pre-funded.
#[test]
fn a_stranger_cannot_credit_native_custody_before_it_is_initialized() {
    let mut chain = Native::new(&[]);
    let error = chain
        .stranger_credits_custody()
        .expect_err("an uninitialised PDA cannot be claimed by a stranger's transfer");
    assert!(error.contains("ClaimedUnauthorizedAccount"), "{error}");
    assert_eq!(
        chain.state.get_account_by_id(chain.custody),
        Account::default()
    );
}

#[test]
fn zec_preimage_claim_and_refund_survive_a_credited_custody() {
    let mut chain = zec_native();
    chain.fund().expect("the escrow funds");
    chain
        .stranger_credits_custody()
        .expect("LEZ lets anyone credit an initialised account");
    assert_eq!(balance(&chain.state, chain.custody), AMOUNT + DUST);

    let mut claim_chain = chain.branch();
    let before = balance(&claim_chain.state, claim_chain.claimant);
    let claim = transaction(
        &claim_chain.state,
        ZEC_ESCROW_V02_ID,
        vec![
            claim_chain.metadata,
            claim_chain.custody,
            claim_chain.claimant,
        ],
        &[(claim_chain.claimant, &claim_chain.claimant_key)],
        EscrowInstruction::ClaimNative {
            swap_id: SWAP_ID,
            preimage: PREIMAGE,
        },
    );
    claim_chain
        .submit(&claim, REFUND_AT - 1)
        .expect("the preimage claim commits on a credited custody");
    claim_chain.assert_swept_to(claim_chain.claimant, before, EscrowStatus::Claimed);

    chain.native_refund();
}

#[test]
fn btc_aggregate_claim_and_refund_survive_a_credited_custody() {
    let aggregate = aggregate(0x31, 0x42);
    let mut chain = btc_native(&aggregate);
    chain.fund().expect("the escrow funds");
    chain
        .stranger_credits_custody()
        .expect("LEZ lets anyone credit an initialised account");

    let mut claim_chain = chain.branch();
    let before = balance(&claim_chain.state, claim_chain.claimant);
    let claim = aggregate_transaction(
        &claim_chain.state,
        vec![
            claim_chain.metadata,
            claim_chain.custody,
            claim_chain.claimant,
            aggregate.authority,
        ],
        &aggregate,
        EscrowInstruction::ClaimNativeWitnessed { swap_id: SWAP_ID },
    );
    claim_chain
        .submit(&claim, REFUND_AT - 1)
        .expect("the aggregate claim commits on a credited custody");
    claim_chain.assert_swept_to(claim_chain.claimant, before, EscrowStatus::Claimed);

    chain.native_refund();
}

#[test]
fn xmr_claim_refund_and_punish_survive_a_credited_custody() {
    let claim_aggregate = aggregate(0x31, 0x42);
    let refund_aggregate = aggregate(0x53, 0x64);
    let mut chain = xmr_native(&claim_aggregate, &refund_aggregate);
    chain.fund().expect("the escrow funds");
    chain
        .stranger_credits_custody()
        .expect("LEZ lets anyone credit an initialised account");

    // Tag 15 after tag 14.
    let mut claim_chain = chain.branch();
    let authorize = transaction(
        &claim_chain.state,
        ZEC_ESCROW_V02_ID,
        vec![claim_chain.metadata, claim_chain.depositor],
        &[(claim_chain.depositor, &claim_chain.depositor_key)],
        EscrowInstruction::AuthorizeNativeXmrClaim {
            swap_id: SWAP_ID,
            claim_partial: CLAIM_PARTIAL,
        },
    );
    claim_chain
        .submit(&authorize, 102)
        .expect("tag 14 does not read the custody");
    let before = balance(&claim_chain.state, claim_chain.claimant);
    let claim = aggregate_transaction(
        &claim_chain.state,
        vec![
            claim_chain.metadata,
            claim_chain.custody,
            claim_chain.claimant,
            claim_aggregate.authority,
        ],
        &claim_aggregate,
        EscrowInstruction::ClaimNativeXmr { swap_id: SWAP_ID },
    );
    claim_chain
        .submit(&claim, REFUND_AT - 1)
        .expect("tag 15 commits on a credited custody");
    claim_chain.assert_swept_to(claim_chain.claimant, before, EscrowStatus::Claimed);

    // Tag 16 in [R, P).
    let mut refund_chain = chain.branch();
    let before = balance(&refund_chain.state, refund_chain.depositor);
    let refund = aggregate_transaction(
        &refund_chain.state,
        vec![
            refund_chain.metadata,
            refund_chain.custody,
            refund_chain.depositor,
            refund_aggregate.authority,
        ],
        &refund_aggregate,
        EscrowInstruction::RefundNativeXmr { swap_id: SWAP_ID },
    );
    refund_chain
        .submit(&refund, REFUND_AT)
        .expect("tag 16 commits on a credited custody");
    refund_chain.assert_swept_to(refund_chain.depositor, before, EscrowStatus::Refunded);

    // Tag 17 from P.
    let before = balance(&chain.state, chain.claimant);
    let punish = transaction(
        &chain.state,
        ZEC_ESCROW_V02_ID,
        vec![chain.metadata, chain.custody, chain.claimant],
        &[(chain.claimant, &chain.claimant_key)],
        EscrowInstruction::PunishNativeXmr { swap_id: SWAP_ID },
    );
    chain
        .submit(&punish, PUNISH_AT)
        .expect("tag 17 commits on a credited custody");
    chain.assert_swept_to(chain.claimant, before, EscrowStatus::Claimed);
}

/// The credit lands after `Initialize` and before `FundNative`. Funding must
/// not be blocked by it, and the ending pays the surplus out with the lot.
#[test]
fn a_credit_before_funding_neither_blocks_funding_nor_strands_the_surplus() {
    let mut chain = zec_native();
    chain
        .stranger_credits_custody()
        .expect("LEZ lets anyone credit an initialised account");
    chain
        .fund()
        .expect("funding commits on a custody a stranger credited first");
    assert_eq!(status(&chain.state, chain.metadata), EscrowStatus::Funded);
    assert_eq!(balance(&chain.state, chain.custody), AMOUNT + DUST);
    chain.native_refund();
}

// Token custody: the escrow metadata PDA's associated token account.

fn ata(owner: AccountId, definition: AccountId) -> AccountId {
    ata_core::get_associated_token_account_id(
        &programs::ata().id(),
        &ata_core::compute_ata_seed(owner, definition),
    )
}

fn holding_balance(state: &V03State, account_id: AccountId) -> u128 {
    match TokenHolding::try_from(&state.get_account_by_id(account_id).data)
        .expect("official token program stores a holding")
    {
        TokenHolding::Fungible { balance, .. } => balance,
        _ => panic!("fungible holding"),
    }
}

/// The token pairs' chain. The stranger holds the definition's remaining
/// supply, so it can send the custody one token whenever it likes.
struct Token {
    state: V03State,
    block: u64,
    definition: AccountId,
    metadata: AccountId,
    custody: AccountId,
    depositor: AccountId,
    depositor_key: PrivateKey,
    depositor_ata: AccountId,
    claimant: AccountId,
    claimant_ata: AccountId,
    claimant_key: PrivateKey,
    stranger: AccountId,
    stranger_key: PrivateKey,
}

impl Token {
    fn new() -> Self {
        let escrow = Program::new(ZEC_ESCROW_V02_ELF.into()).expect("checked guest ELF");
        assert_eq!(escrow.id(), ZEC_ESCROW_V02_ID);
        let state = V03State::new().with_programs([escrow, programs::ata(), programs::token()]);
        let (depositor, depositor_key) = actor(1);
        let (claimant, claimant_key) = actor(2);
        let (definition, definition_key) = actor(41);
        let (stranger, stranger_key) = actor(51);
        let metadata = compute_pda(&ZEC_ESCROW_V02_ID, &[&SWAP_ID]);
        let mut chain = Self {
            state,
            block: 1,
            definition,
            metadata,
            custody: ata(metadata, definition),
            depositor,
            depositor_key,
            depositor_ata: ata(depositor, definition),
            claimant,
            claimant_ata: ata(claimant, definition),
            claimant_key,
            stranger,
            stranger_key,
        };
        let define = transaction(
            &chain.state,
            programs::token().id(),
            vec![definition, stranger],
            &[
                (definition, &definition_key),
                (stranger, &chain.stranger_key),
            ],
            token_core::Instruction::NewFungibleDefinition {
                name: "custody dust".into(),
                total_supply: 1_000,
            },
        );
        chain.submit(&define).expect("fungible definition");
        for (owner, key_byte) in [(depositor, 1), (claimant, 2)] {
            let key = PrivateKey::try_new([key_byte; 32]).expect("owner key");
            let create = transaction(
                &chain.state,
                programs::ata().id(),
                vec![owner, definition, ata(owner, definition)],
                &[(owner, &key)],
                ata_core::Instruction::Create {
                    ata_program_id: programs::ata().id(),
                },
            );
            chain.submit(&create).expect("owner ATA");
        }
        let supply = transaction(
            &chain.state,
            programs::token().id(),
            vec![stranger, chain.depositor_ata],
            &[(stranger, &chain.stranger_key)],
            token_core::Instruction::Transfer {
                amount_to_transfer: ACTOR_FUNDS,
            },
        );
        chain.submit(&supply).expect("depositor funds");
        chain
    }

    fn submit(&mut self, transaction: &PublicTransaction) -> Result<(), String> {
        self.submit_at(transaction, 100)
    }

    fn submit_at(&mut self, transaction: &PublicTransaction, timestamp: u64) -> Result<(), String> {
        let result = self
            .state
            .transition_from_public_transaction(transaction, self.block, timestamp)
            .map(|_| ())
            .map_err(|error| format!("{error:?}"));
        self.block += 1;
        result
    }

    fn initialize(&mut self, instruction: EscrowInstruction, extra: &[AccountId]) {
        let mut accounts = vec![
            self.metadata,
            self.depositor,
            self.claimant,
            self.definition,
        ];
        accounts.extend_from_slice(extra);
        let initialize = transaction(
            &self.state,
            ZEC_ESCROW_V02_ID,
            accounts,
            &[(self.depositor, &self.depositor_key)],
            instruction,
        );
        self.submit(&initialize).expect("token escrow initializes");
    }

    fn create_custody(&mut self) -> Result<(), String> {
        let create = transaction(
            &self.state,
            ZEC_ESCROW_V02_ID,
            vec![self.metadata, self.definition, self.custody],
            &[],
            EscrowInstruction::CreateTokenCustody { swap_id: SWAP_ID },
        );
        self.submit(&create)
    }

    fn fund(&mut self) -> Result<(), String> {
        let fund = transaction(
            &self.state,
            ZEC_ESCROW_V02_ID,
            vec![
                self.metadata,
                self.depositor,
                self.depositor_ata,
                self.custody,
            ],
            &[(self.depositor, &self.depositor_key)],
            EscrowInstruction::FundToken { swap_id: SWAP_ID },
        );
        self.submit(&fund)
    }

    /// What any holder of the token can do: a plain token transfer it signs.
    fn stranger_credits_custody(&mut self) -> Result<(), String> {
        let credit = transaction(
            &self.state,
            programs::token().id(),
            vec![self.stranger, self.custody],
            &[(self.stranger, &self.stranger_key)],
            token_core::Instruction::Transfer {
                amount_to_transfer: DUST,
            },
        );
        self.submit(&credit)
    }

    fn assert_swept_to(&self, recipient: AccountId, before: u128, terminal: EscrowStatus) {
        assert_eq!(status(&self.state, self.metadata), terminal);
        assert_eq!(
            holding_balance(&self.state, self.custody),
            0,
            "custody is empty"
        );
        assert_eq!(
            holding_balance(&self.state, recipient),
            before + AMOUNT + DUST,
            "the recipient receives the amount and the surplus"
        );
    }

    fn refund(mut self) {
        let before = holding_balance(&self.state, self.depositor_ata);
        let refund = transaction(
            &self.state,
            ZEC_ESCROW_V02_ID,
            vec![self.metadata, self.custody, self.depositor_ata],
            &[],
            EscrowInstruction::RefundToken { swap_id: SWAP_ID },
        );
        self.submit_at(&refund, REFUND_AT)
            .expect("the token refund commits on a credited custody");
        self.assert_swept_to(self.depositor_ata, before, EscrowStatus::Refunded);
    }

    fn branch(&self) -> Self {
        Self {
            state: self.state.clone(),
            block: self.block,
            definition: self.definition,
            metadata: self.metadata,
            custody: self.custody,
            depositor: self.depositor,
            depositor_key: PrivateKey::try_new([1; 32]).expect("depositor key"),
            depositor_ata: self.depositor_ata,
            claimant: self.claimant,
            claimant_ata: self.claimant_ata,
            claimant_key: PrivateKey::try_new([2; 32]).expect("claimant key"),
            stranger: self.stranger,
            stranger_key: PrivateKey::try_new([51; 32]).expect("stranger key"),
        }
    }
}

fn token_preimage() -> Token {
    let mut chain = Token::new();
    chain.initialize(
        EscrowInstruction::InitializeToken {
            swap_id: SWAP_ID,
            terms_hash: [31; 32],
            secret_digest: Sha256::digest(PREIMAGE).into(),
            amount: AMOUNT,
            refund_at: REFUND_AT,
            ata_program: programs::ata().id(),
        },
        &[],
    );
    chain
}

#[test]
fn token_preimage_claim_and_refund_survive_a_credited_custody() {
    let mut chain = token_preimage();
    chain.create_custody().expect("custody ATA");
    chain.fund().expect("the token escrow funds");
    chain
        .stranger_credits_custody()
        .expect("any holder can credit an initialised holding");
    assert_eq!(holding_balance(&chain.state, chain.custody), AMOUNT + DUST);

    let mut claim_chain = chain.branch();
    let before = holding_balance(&claim_chain.state, claim_chain.claimant_ata);
    let claim = transaction(
        &claim_chain.state,
        ZEC_ESCROW_V02_ID,
        vec![
            claim_chain.metadata,
            claim_chain.custody,
            claim_chain.claimant,
            claim_chain.claimant_ata,
        ],
        &[(claim_chain.claimant, &claim_chain.claimant_key)],
        EscrowInstruction::ClaimToken {
            swap_id: SWAP_ID,
            preimage: PREIMAGE,
        },
    );
    claim_chain
        .submit_at(&claim, REFUND_AT - 1)
        .expect("the token preimage claim commits on a credited custody");
    claim_chain.assert_swept_to(claim_chain.claimant_ata, before, EscrowStatus::Claimed);

    chain.refund();
}

#[test]
fn token_aggregate_claim_survives_a_credited_custody() {
    let aggregate = aggregate(0x31, 0x42);
    let mut chain = Token::new();
    chain.initialize(
        EscrowInstruction::InitializeTokenWitnessed {
            swap_id: SWAP_ID,
            terms_hash: [31; 32],
            aggregate_x_only_public_key: *aggregate.public_key.value(),
            amount: AMOUNT,
            refund_at: REFUND_AT,
            ata_program: programs::ata().id(),
        },
        &[aggregate.authority],
    );
    chain.create_custody().expect("custody ATA");
    chain.fund().expect("the token escrow funds");
    chain
        .stranger_credits_custody()
        .expect("any holder can credit an initialised holding");

    let mut claim_chain = chain.branch();
    let before = holding_balance(&claim_chain.state, claim_chain.claimant_ata);
    let claim = aggregate_transaction(
        &claim_chain.state,
        vec![
            claim_chain.metadata,
            claim_chain.custody,
            claim_chain.claimant,
            claim_chain.claimant_ata,
            aggregate.authority,
        ],
        &aggregate,
        EscrowInstruction::ClaimTokenWitnessed { swap_id: SWAP_ID },
    );
    claim_chain
        .submit_at(&claim, REFUND_AT - 1)
        .expect("the witnessed token claim commits on a credited custody");
    claim_chain.assert_swept_to(claim_chain.claimant_ata, before, EscrowStatus::Claimed);

    chain.refund();
}

/// The associated-token-account program's `Create` is permissionless, so a
/// stranger can create the escrow's custody ATA after `InitializeToken` and
/// credit it before `FundToken`. Neither may block the escrow.
#[test]
fn a_stranger_made_and_credited_custody_ata_neither_blocks_nor_strands() {
    let mut chain = token_preimage();
    let create = transaction(
        &chain.state,
        programs::ata().id(),
        vec![chain.metadata, chain.definition, chain.custody],
        &[],
        ata_core::Instruction::Create {
            ata_program_id: programs::ata().id(),
        },
    );
    chain
        .submit(&create)
        .expect("anyone can create the ATA of an initialised owner");
    chain
        .stranger_credits_custody()
        .expect("any holder can credit an initialised holding");
    chain
        .create_custody()
        .expect("CreateTokenCustody accepts the exact ATA a stranger created");
    chain
        .fund()
        .expect("funding commits on a custody a stranger credited first");
    assert_eq!(holding_balance(&chain.state, chain.custody), AMOUNT + DUST);
    chain.refund();
}

/// Before `InitializeToken` the metadata PDA is a default account, and the
/// ATA program's `Create` would have to claim it for the stranger, which LEZ
/// refuses as it refuses the native case. So no custody ATA exists before the
/// escrow does.
#[test]
fn a_stranger_cannot_create_the_custody_ata_before_the_escrow_exists() {
    let mut chain = Token::new();
    let create = transaction(
        &chain.state,
        programs::ata().id(),
        vec![chain.metadata, chain.definition, chain.custody],
        &[],
        ata_core::Instruction::Create {
            ata_program_id: programs::ata().id(),
        },
    );
    let error = chain
        .submit(&create)
        .expect_err("the metadata PDA cannot be claimed by a stranger's call");
    assert!(error.contains("ClaimedUnauthorizedAccount"), "{error}");
    assert_eq!(
        chain.state.get_account_by_id(chain.custody),
        Account::default()
    );
}
