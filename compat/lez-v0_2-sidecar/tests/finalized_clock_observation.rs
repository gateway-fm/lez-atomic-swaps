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
    advance_on_final_tip_read: bool,
}

#[async_trait]
impl FinalizedIndexerApi for FixtureIndexer {
    async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
        let read = self.tip_reads.fetch_add(1, Ordering::SeqCst);
        Ok(Some(if self.advance_on_final_tip_read && read == 1 {
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

fn indexer(advance_on_final_tip_read: bool) -> FixtureIndexer {
    FixtureIndexer {
        blocks: [
            (GENESIS, block(GENESIS, 41, 1_000)),
            (TIP, block(TIP, 43, 3_000)),
        ]
        .into_iter()
        .collect(),
        tip_reads: AtomicUsize::new(0),
        advance_on_final_tip_read,
    }
}

#[tokio::test]
async fn returns_exact_clock_only_for_stable_tip_on_expected_genesis() {
    let fixture = indexer(false);

    let clock = read_genesis_bound_finalized_clock(&fixture, Hex32::from_bytes([41; 32]))
        .await
        .expect("stable finalized clock");

    assert_eq!(clock.block_hash, Hex32::from_bytes([43; 32]));
    assert_eq!(clock.height, TIP);
    assert_eq!(clock.timestamp_ms, 3_000);
    assert_eq!(fixture.tip_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn rejects_wrong_genesis_and_tip_movement() {
    let wrong_genesis =
        read_genesis_bound_finalized_clock(&indexer(false), Hex32::from_bytes([99; 32])).await;
    assert_eq!(wrong_genesis, Err(BridgeRuntimeError::InvalidObservation));

    let moving =
        read_genesis_bound_finalized_clock(&indexer(true), Hex32::from_bytes([41; 32])).await;
    assert_eq!(moving, Err(BridgeRuntimeError::MovingTip));
}
