//! Reproducers against the exact LEZ v0.2.0 mempool and block builder.
//!
//! Every test starts a real `sequencer_core` with the mock block publisher,
//! submits transactions through the real mempool handle, and produces blocks
//! with the real builder. Nothing here is mocked below the publisher.
//!
//! What they pin down:
//! - the mempool admits any stateless-valid transaction; state validation,
//!   including balances and validity windows, happens at block construction;
//! - validity windows are evaluated against the height and timestamp of the
//!   block being built, lower-inclusive and upper-exclusive;
//! - a transaction that was valid when queued but has fallen out of its window
//!   by the time the builder reaches it is dropped, leaves no state effect,
//!   and is not re-queued.
//!
//! Block heights are exact because the tests control block production.
//! Timestamps come from the builder's wall clock, so the timestamp cases use
//! margins rather than exact millisecond bounds.

#![forbid(unsafe_code)]

use std::time::Duration;

use bytesize::ByteSize;
use common::{
    block::Block,
    test_utils::{
        create_transaction_native_token_transfer, produce_dummy_empty_transaction,
        sequencer_sign_key_for_testing,
    },
    transaction::{LeeTransaction, clock_invocation},
};
use lee::{
    AccountId, PrivateKey, PublicTransaction, program_deployment_transaction, public_transaction,
};
use lee_core::{
    account::Nonce,
    program::{BlockValidityWindow, TimestampValidityWindow},
};
use lez_v0_2_sequencer_reproducers::windowed_transfer;
use logos_blockchain_core::mantle::ops::channel::ChannelId;
use mempool::MemPoolHandle;
use sequencer_core::{
    SequencerCoreWithMockClients, TransactionOrigin,
    config::{BedrockConfig, SequencerConfig},
};
use testnet_initial_state::{initial_pub_accounts_private_keys, initial_public_user_accounts};

const AMOUNT: u128 = 250;

/// One in-process sequencer with its mempool handle. Mirrors upstream's own
/// `common_setup`: after start, one empty transaction has been mined so the
/// chain is past genesis.
struct Node {
    sequencer: SequencerCoreWithMockClients,
    mempool: MemPoolHandle<(TransactionOrigin, LeeTransaction)>,
    _home: tempfile::TempDir,
}

impl Node {
    async fn start(max_num_tx_in_block: usize) -> Self {
        let home = tempfile::tempdir().expect("temporary sequencer home");
        let config = SequencerConfig {
            home: home.path().to_path_buf(),
            max_num_tx_in_block,
            max_block_size: ByteSize::mib(1),
            mempool_max_size: 10_000,
            block_create_timeout: Duration::from_secs(1),
            signing_key: *sequencer_sign_key_for_testing().value(),
            bedrock_config: BedrockConfig {
                channel_id: ChannelId::from([0; 32]),
                node_url: "http://not-used-in-unit-tests".parse().expect("static URL"),
                auth: None,
            },
            retry_pending_blocks_timeout: Duration::from_mins(4),
            genesis: vec![],
        };
        let (mut sequencer, mempool) =
            SequencerCoreWithMockClients::start_from_config(config).await;

        mempool
            .push((TransactionOrigin::User, produce_dummy_empty_transaction()))
            .await
            .expect("mempool admits the setup transaction");
        sequencer
            .produce_new_block()
            .await
            .expect("setup block is produced");

        Self {
            sequencer,
            mempool,
            _home: home,
        }
    }

    async fn submit(&self, tx: LeeTransaction) {
        self.mempool
            .push((TransactionOrigin::User, tx))
            .await
            .expect("mempool admits the transaction");
    }

    async fn produce_block(&mut self) -> u64 {
        self.sequencer
            .produce_new_block()
            .await
            .expect("block is produced")
    }

    fn next_height(&self) -> u64 {
        self.sequencer.chain_height() + 1
    }

    fn latest_block(&self) -> Block {
        self.sequencer
            .block_store()
            .get_block_at_id(self.sequencer.chain_height())
            .expect("block store is readable")
            .expect("latest block exists")
    }

    fn latest_transactions(&self) -> Vec<LeeTransaction> {
        self.latest_block().body.transactions
    }

    /// The mandatory clock transaction the builder appends to every block.
    fn clock(&self) -> LeeTransaction {
        LeeTransaction::Public(clock_invocation(self.latest_block().header.timestamp))
    }

    fn balance(&self, account: AccountId) -> u128 {
        self.sequencer.state().get_account_by_id(account).balance
    }

    fn nonce(&self, account: AccountId) -> Nonce {
        self.sequencer.state().get_account_by_id(account).nonce
    }

    /// Whether the builder would accept `tx` for a block at `height` and
    /// `timestamp`, using the same check the builder runs.
    fn validates_at(&self, tx: &LeeTransaction, height: u64, timestamp: u64) -> bool {
        tx.validate_on_state(self.sequencer.state(), height, timestamp)
            .is_ok()
    }

    /// Deploys the windowed-transfer guest through the mempool.
    async fn deploy_windowed_transfer(&mut self) {
        let program = windowed_transfer();
        let deploy = LeeTransaction::ProgramDeployment(lee::ProgramDeploymentTransaction::new(
            program_deployment_transaction::Message::new(program.elf().to_owned()),
        ));
        self.submit(deploy.clone()).await;
        self.produce_block().await;
        assert_eq!(self.latest_transactions(), vec![deploy, self.clock()]);
    }
}

/// The two genesis-funded public accounts. `a` signs the windowed transfers;
/// `b` receives them and signs the unrelated filler transfer.
struct Accounts {
    a: AccountId,
    a_key: PrivateKey,
    b: AccountId,
    b_key: PrivateKey,
}

fn accounts() -> Accounts {
    let keys = initial_pub_accounts_private_keys();
    let ids = initial_public_user_accounts();
    Accounts {
        a: ids[0].account_id,
        a_key: keys[0].pub_sign_key.clone(),
        b: ids[1].account_id,
        b_key: keys[1].pub_sign_key.clone(),
    }
}

fn now_millis() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).expect("timestamp is positive")
}

fn block_window(range: std::ops::Range<u64>) -> BlockValidityWindow {
    range.try_into().expect("non-empty block window")
}

fn timestamp_window(range: std::ops::Range<u64>) -> TimestampValidityWindow {
    range.try_into().expect("non-empty timestamp window")
}

fn windowed_transfer_transaction(
    sender: AccountId,
    sender_key: &PrivateKey,
    sender_nonce: u128,
    receiver: AccountId,
    block: BlockValidityWindow,
    timestamp: TimestampValidityWindow,
) -> LeeTransaction {
    let message = public_transaction::Message::try_new(
        windowed_transfer().id(),
        vec![sender, receiver],
        vec![Nonce(sender_nonce)],
        (
            AMOUNT,
            programs::authenticated_transfer().id(),
            block,
            timestamp,
        ),
    )
    .expect("instruction serializes");
    let witness_set = public_transaction::WitnessSet::for_message(&message, &[sender_key]);
    LeeTransaction::Public(PublicTransaction::new(message, witness_set))
}

/// A stateless-valid transfer for more than the sender holds is admitted by the
/// mempool and dropped by the builder: state validation happens at block
/// construction, not at admission.
#[tokio::test]
async fn mempool_admits_then_block_rejects_insufficient_balance() {
    let mut node = Node::start(10).await;
    let acc = accounts();
    let a_before = node.balance(acc.a);

    let overspend =
        create_transaction_native_token_transfer(acc.a, 0, acc.b, a_before + 1, &acc.a_key);
    assert!(overspend.clone().transaction_stateless_check().is_ok());
    node.submit(overspend).await;

    node.produce_block().await;
    assert_eq!(node.latest_transactions(), vec![node.clock()]);
    assert_eq!(node.balance(acc.a), a_before);
    assert_eq!(node.nonce(acc.a), Nonce(0));
}

/// Two identical submissions yield exactly one inclusion, and the included
/// transaction is byte-for-byte the submitted one, signature included.
#[tokio::test]
async fn transaction_bytes_are_preserved_from_mempool_to_block() {
    let mut node = Node::start(10).await;
    let acc = accounts();
    let a_before = node.balance(acc.a);

    let tx = create_transaction_native_token_transfer(acc.a, 0, acc.b, 100, &acc.a_key);
    node.submit(tx.clone()).await;
    node.submit(tx.clone()).await;

    node.produce_block().await;
    assert_eq!(node.latest_transactions(), vec![tx, node.clock()]);
    assert_eq!(node.balance(acc.a), a_before - 100);
    assert_eq!(node.nonce(acc.a), Nonce(1));
}

/// The reviewer's scenario: a transfer that is valid for the next block when it
/// enters the mempool, is held back by a full block, and has expired by the
/// time the builder reaches it. It is not included, touches no state, and is
/// not re-queued for a later block.
#[tokio::test]
async fn valid_when_queued_expires_before_inclusion() {
    let mut node = Node::start(1).await;
    node.deploy_windowed_transfer().await;
    let acc = accounts();
    let (a_before, b_before) = (node.balance(acc.a), node.balance(acc.b));

    let next = node.next_height();
    // Valid for exactly one block: the next one.
    let windowed = windowed_transfer_transaction(
        acc.a,
        &acc.a_key,
        0,
        acc.b,
        block_window(next..next + 1),
        TimestampValidityWindow::new_unbounded(),
    );
    // An unrelated valid transfer that takes the single slot of the next block.
    let filler = create_transaction_native_token_transfer(acc.b, 0, acc.a, 100, &acc.b_key);

    node.submit(filler.clone()).await;
    node.submit(windowed.clone()).await;
    // Valid for the block it is queued for, and only for that block.
    assert!(node.validates_at(&windowed, next, now_millis()));
    assert!(!node.validates_at(&windowed, next + 1, now_millis()));

    // Block `next` fills with the filler; the windowed transfer stays queued.
    assert_eq!(node.produce_block().await, next);
    assert_eq!(node.latest_transactions(), vec![filler, node.clock()]);
    assert_eq!(node.balance(acc.a), a_before + 100);
    assert_eq!(node.nonce(acc.a), Nonce(0));
    assert_eq!(node.balance(acc.b), b_before - 100);

    // Block `next + 1` is the first the builder can offer it, and that is past
    // its window: dropped, with no state effect.
    assert_eq!(node.produce_block().await, next + 1);
    assert_eq!(node.latest_transactions(), vec![node.clock()]);
    assert_eq!(node.balance(acc.a), a_before + 100);
    assert_eq!(node.nonce(acc.a), Nonce(0));
    assert_eq!(node.balance(acc.b), b_before - 100);

    // Dropped, not deferred: nothing surfaces in a later block either.
    node.produce_block().await;
    assert_eq!(node.latest_transactions(), vec![node.clock()]);
    assert_eq!(node.nonce(acc.a), Nonce(0));
}

/// Block windows are lower-inclusive: dropped one block before the start,
/// included at exactly the start.
#[tokio::test]
async fn block_window_start_is_inclusive() {
    let mut node = Node::start(10).await;
    node.deploy_windowed_transfer().await;
    let acc = accounts();
    let (a_before, b_before) = (node.balance(acc.a), node.balance(acc.b));

    let next = node.next_height();
    let tx = windowed_transfer_transaction(
        acc.a,
        &acc.a_key,
        0,
        acc.b,
        block_window(next + 1..u64::MAX),
        TimestampValidityWindow::new_unbounded(),
    );
    assert!(!node.validates_at(&tx, next, now_millis()));
    assert!(node.validates_at(&tx, next + 1, now_millis()));

    // One block before the start: admitted, then dropped by the builder.
    node.submit(tx.clone()).await;
    assert_eq!(node.produce_block().await, next);
    assert_eq!(node.latest_transactions(), vec![node.clock()]);
    assert_eq!(node.balance(acc.a), a_before);
    assert_eq!(node.nonce(acc.a), Nonce(0));

    // Exactly at the start: the same bytes are included and applied.
    node.submit(tx.clone()).await;
    assert_eq!(node.produce_block().await, next + 1);
    assert_eq!(node.latest_transactions(), vec![tx, node.clock()]);
    assert_eq!(node.balance(acc.a), a_before - AMOUNT);
    assert_eq!(node.balance(acc.b), b_before + AMOUNT);
    assert_eq!(node.nonce(acc.a), Nonce(1));
}

/// Block windows are upper-exclusive: included at the last block before the
/// end, dropped at exactly the end.
#[tokio::test]
async fn block_window_end_is_exclusive() {
    let mut node = Node::start(10).await;
    node.deploy_windowed_transfer().await;
    let acc = accounts();
    let (a_before, b_before) = (node.balance(acc.a), node.balance(acc.b));

    let next = node.next_height();
    let end = next + 1;

    // Last block before the end: included.
    let first = windowed_transfer_transaction(
        acc.a,
        &acc.a_key,
        0,
        acc.b,
        block_window(0..end),
        TimestampValidityWindow::new_unbounded(),
    );
    node.submit(first.clone()).await;
    assert_eq!(node.produce_block().await, end - 1);
    assert_eq!(node.latest_transactions(), vec![first, node.clock()]);
    assert_eq!(node.balance(acc.a), a_before - AMOUNT);
    assert_eq!(node.balance(acc.b), b_before + AMOUNT);
    assert_eq!(node.nonce(acc.a), Nonce(1));

    // Exactly at the end: the same window on the next nonce is otherwise valid,
    // but the builder evaluates it against block `end` and drops it.
    let second = windowed_transfer_transaction(
        acc.a,
        &acc.a_key,
        1,
        acc.b,
        block_window(0..end),
        TimestampValidityWindow::new_unbounded(),
    );
    assert!(node.validates_at(&second, end - 1, now_millis()));
    assert!(!node.validates_at(&second, end, now_millis()));
    node.submit(second).await;
    assert_eq!(node.produce_block().await, end);
    assert_eq!(node.latest_transactions(), vec![node.clock()]);
    assert_eq!(node.balance(acc.a), a_before - AMOUNT);
    assert_eq!(node.balance(acc.b), b_before + AMOUNT);
    assert_eq!(node.nonce(acc.a), Nonce(1));
}

/// Timestamp windows are evaluated against the wall-clock timestamp the builder
/// stamps on the block: a window that closes while the transfer waits in the
/// mempool expires it.
#[tokio::test]
async fn timestamp_window_expires_while_queued() {
    let mut node = Node::start(10).await;
    node.deploy_windowed_transfer().await;
    let acc = accounts();
    let (a_before, b_before) = (node.balance(acc.a), node.balance(acc.b));

    let next = node.next_height();
    let queued_at = now_millis();
    let expires_at = queued_at + 1_500;
    let tx = windowed_transfer_transaction(
        acc.a,
        &acc.a_key,
        0,
        acc.b,
        BlockValidityWindow::new_unbounded(),
        timestamp_window(queued_at - 60_000..expires_at),
    );
    assert!(node.validates_at(&tx, next, queued_at));
    assert!(!node.validates_at(&tx, next, expires_at));
    node.submit(tx).await;

    // Let the window close before the builder runs.
    tokio::time::sleep(Duration::from_secs(2)).await;
    node.produce_block().await;

    assert!(node.latest_block().header.timestamp >= expires_at);
    assert_eq!(node.latest_transactions(), vec![node.clock()]);
    assert_eq!(node.balance(acc.a), a_before);
    assert_eq!(node.balance(acc.b), b_before);
    assert_eq!(node.nonce(acc.a), Nonce(0));

    node.produce_block().await;
    assert_eq!(node.latest_transactions(), vec![node.clock()]);
    assert_eq!(node.nonce(acc.a), Nonce(0));
}

/// Timestamp bounds in one block: not-yet-open and already-closed windows are
/// dropped from the same block that includes an open one.
#[tokio::test]
async fn timestamp_window_bounds_apply_at_block_time() {
    let mut node = Node::start(10).await;
    node.deploy_windowed_transfer().await;
    let acc = accounts();
    let (a_before, b_before) = (node.balance(acc.a), node.balance(acc.b));

    let now = now_millis();
    let hour = 3_600_000;
    let not_yet_open = windowed_transfer_transaction(
        acc.a,
        &acc.a_key,
        0,
        acc.b,
        BlockValidityWindow::new_unbounded(),
        timestamp_window(now + hour..now + 2 * hour),
    );
    let already_closed = windowed_transfer_transaction(
        acc.a,
        &acc.a_key,
        0,
        acc.b,
        BlockValidityWindow::new_unbounded(),
        timestamp_window(now - 2 * hour..now - hour),
    );
    let open = windowed_transfer_transaction(
        acc.a,
        &acc.a_key,
        0,
        acc.b,
        BlockValidityWindow::new_unbounded(),
        timestamp_window(now - hour..now + hour),
    );
    for tx in [&not_yet_open, &already_closed, &open] {
        node.submit(tx.clone()).await;
    }

    node.produce_block().await;
    assert_eq!(node.latest_transactions(), vec![open, node.clock()]);
    assert_eq!(node.balance(acc.a), a_before - AMOUNT);
    assert_eq!(node.balance(acc.b), b_before + AMOUNT);
    assert_eq!(node.nonce(acc.a), Nonce(1));
}
