use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use indexer_service_protocol::{BedrockStatus, Block, BlockBody, BlockHeader, HashType, Signature};
use lez_bridge_protocol::Hex32;
use lez_v0_2_sidecar::{
    BridgeRuntimeError, FinalizedIndexerApi, HistoricalAccount, read_genesis_bound_finalized_clock,
};

const GENESIS: u64 = nssa::GENESIS_BLOCK_ID;
const TIP: u64 = GENESIS + 2;

#[derive(Debug)]
struct FixtureIndexer {
    blocks: BTreeMap<u64, Block>,
    tip_reads: AtomicUsize,
    /// The number of leading samples whose closing tip read sees one more block.
    moving_samples: usize,
}

#[async_trait]
impl FinalizedIndexerApi for FixtureIndexer {
    async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
        // Each complete sample reads the finalized tip twice: first and last.
        let read = self.tip_reads.fetch_add(1, Ordering::SeqCst);
        Ok(Some(if read % 2 == 1 && read / 2 < self.moving_samples {
            TIP + 1
        } else {
            TIP
        }))
    }

    async fn block_by_id(&self, block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
        Ok(self.blocks.get(&block_id).cloned())
    }

    async fn block_by_hash(
        &self,
        block_hash: [u8; 32],
    ) -> Result<Option<Block>, BridgeRuntimeError> {
        Ok(self
            .blocks
            .values()
            .find(|block| block.header.hash.0 == block_hash)
            .cloned())
    }

    async fn account_at_block(
        &self,
        _account_id: [u8; 32],
        _block_id: u64,
    ) -> Result<HistoricalAccount, BridgeRuntimeError> {
        Err(BridgeRuntimeError::Unavailable)
    }
}

fn block(block_id: u64, hash_byte: u8, timestamp: u64) -> Block {
    Block {
        header: BlockHeader {
            block_id,
            prev_block_hash: HashType([hash_byte.saturating_sub(1); 32]),
            hash: HashType([hash_byte; 32]),
            timestamp,
            signature: Signature([hash_byte; 64]),
        },
        body: BlockBody {
            transactions: Vec::new(),
        },
        bedrock_status: BedrockStatus::Finalized,
    }
}

fn indexer(moving_samples: usize) -> FixtureIndexer {
    FixtureIndexer {
        blocks: [
            (GENESIS, block(GENESIS, 41, 1_000)),
            (TIP, block(TIP, 43, 3_000)),
        ]
        .into_iter()
        .collect(),
        tip_reads: AtomicUsize::new(0),
        moving_samples,
    }
}

#[tokio::test]
async fn returns_exact_clock_only_for_stable_tip_on_expected_genesis() {
    let fixture = indexer(0);

    let clock = read_genesis_bound_finalized_clock(&fixture, Hex32::from_bytes([41; 32]))
        .await
        .expect("stable finalized clock");

    assert_eq!(clock.block_hash, Hex32::from_bytes([43; 32]));
    assert_eq!(clock.height, TIP);
    assert_eq!(clock.timestamp_ms, 3_000);
    assert_eq!(fixture.tip_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn rejects_wrong_genesis_without_resampling() {
    let fixture = indexer(0);

    let wrong_genesis =
        read_genesis_bound_finalized_clock(&fixture, Hex32::from_bytes([99; 32])).await;

    assert_eq!(wrong_genesis, Err(BridgeRuntimeError::InvalidObservation));
    assert_eq!(fixture.tip_reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn resamples_whole_clock_after_finalized_tip_moved() {
    let fixture = indexer(2);

    let clock = read_genesis_bound_finalized_clock(&fixture, Hex32::from_bytes([41; 32]))
        .await
        .expect("a later sample under a still tip");

    assert_eq!(clock.block_hash, Hex32::from_bytes([43; 32]));
    assert_eq!(clock.height, TIP);
    assert_eq!(fixture.tip_reads.load(Ordering::SeqCst), 6);
}

#[tokio::test]
async fn rejects_tip_that_moves_during_every_sample() {
    let fixture = indexer(usize::MAX);

    let moving = read_genesis_bound_finalized_clock(&fixture, Hex32::from_bytes([41; 32])).await;

    assert_eq!(moving, Err(BridgeRuntimeError::MovingTip));
    assert_eq!(fixture.tip_reads.load(Ordering::SeqCst), 10);
}
