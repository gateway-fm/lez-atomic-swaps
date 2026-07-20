use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use indexer_service_protocol::{
    Account as IndexedAccount, BedrockStatus, Block, BlockBody, BlockHeader, Data as IndexedData,
    HashType, ProgramId as IndexedProgramId, Signature,
};
use lez_bridge_protocol::Hex32;
use lez_v0_2_sidecar::{
    BridgeRuntimeError, CHECKED_M4_ESCROW_PROGRAM_ID, FinalizedIndexerApi, HistoricalAccount,
    M4FinalizedAccountIds, M4FinalizedAccountPresence, M4StageAFinalizedNonces,
    read_stable_m4_finalized_nonce_snapshot, validate_checked_m4_escrow_program_id,
};
use nssa::AccountId;

const GENESIS: u64 = nssa::GENESIS_BLOCK_ID;
const TIP: u64 = GENESIS + 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewBehavior {
    Stable,
    MovePinnedTip,
}

#[derive(Debug)]
struct FixtureIndexer {
    tips: Mutex<VecDeque<u64>>,
    genesis: Block,
    tip: Block,
    changed_tip: Block,
    behavior: ViewBehavior,
    tip_id_reads: AtomicUsize,
    accounts: BTreeMap<[u8; 32], HistoricalAccount>,
}

#[async_trait]
impl FinalizedIndexerApi for FixtureIndexer {
    async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
        let mut tips = self.tips.lock().expect("tips lock");
        Ok(Some(if tips.len() > 1 {
            tips.pop_front().expect("nonempty tips")
        } else {
            *tips.front().expect("nonempty tips")
        }))
    }

    async fn block_by_id(&self, block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
        if block_id == GENESIS {
            return Ok(Some(self.genesis.clone()));
        }
        if block_id != TIP {
            return Ok(None);
        }
        let read = self.tip_id_reads.fetch_add(1, Ordering::SeqCst);
        if self.behavior == ViewBehavior::MovePinnedTip && read > 0 {
            Ok(Some(self.changed_tip.clone()))
        } else {
            Ok(Some(self.tip.clone()))
        }
    }

    async fn block_by_hash(
        &self,
        block_hash: [u8; 32],
    ) -> Result<Option<Block>, BridgeRuntimeError> {
        Ok([&self.genesis, &self.tip, &self.changed_tip]
            .into_iter()
            .find(|block| block.header.hash.0 == block_hash)
            .cloned())
    }

    async fn account_at_block(
        &self,
        account_id: [u8; 32],
        block_id: u64,
    ) -> Result<HistoricalAccount, BridgeRuntimeError> {
        if block_id != TIP {
            return Err(BridgeRuntimeError::InvalidObservation);
        }
        Ok(self
            .accounts
            .get(&account_id)
            .cloned()
            .unwrap_or(HistoricalAccount::Absent))
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

fn account(nonce: u128) -> HistoricalAccount {
    HistoricalAccount::Present(IndexedAccount {
        program_owner: IndexedProgramId([0x1020_3040; 8]),
        balance: 0,
        data: IndexedData(Vec::new()),
        nonce,
    })
}

fn account_id(byte: u8) -> AccountId {
    AccountId::new([byte; 32])
}

fn fixture(tips: impl IntoIterator<Item = u64>, behavior: ViewBehavior) -> FixtureIndexer {
    FixtureIndexer {
        tips: Mutex::new(tips.into_iter().collect()),
        genesis: block(GENESIS, 41, 1_000),
        tip: block(TIP, 43, 3_000),
        changed_tip: block(TIP, 44, 3_001),
        behavior,
        tip_id_reads: AtomicUsize::new(0),
        accounts: BTreeMap::from([([1; 32], account(7)), ([2; 32], account(11))]),
    }
}

fn accounts() -> M4FinalizedAccountIds {
    M4FinalizedAccountIds::new(account_id(1), account_id(2), account_id(3), account_id(4))
}

#[tokio::test]
async fn stable_snapshot_preserves_exact_finalized_nonce_provenance() {
    let snapshot = read_stable_m4_finalized_nonce_snapshot(
        &fixture([TIP, TIP + 1], ViewBehavior::Stable),
        Hex32::from_bytes([41; 32]),
        accounts(),
    )
    .await
    .expect("stable finalized snapshot");

    assert_eq!(snapshot.finalized_clock().height, TIP);
    assert_eq!(snapshot.finalized_clock().timestamp_ms, 3_000);
    assert_eq!(snapshot.genesis_block_hash(), Hex32::from_bytes([41; 32]));
    assert_eq!(snapshot.maker_owner().nonce(), 7);
    assert_eq!(snapshot.taker_owner().nonce(), 11);
    assert_eq!(snapshot.claim_authority().nonce(), 0);
    assert_eq!(
        snapshot.claim_authority().presence(),
        M4FinalizedAccountPresence::AbsentDefaultNonce
    );
    assert_eq!(snapshot.refund_authority().nonce(), 0);
    assert_eq!(
        snapshot.planned_nonces(),
        M4StageAFinalizedNonces::new(7, 11, 0, 0)
    );
}

#[tokio::test]
async fn moving_pinned_finalized_view_fails_closed() {
    let result = read_stable_m4_finalized_nonce_snapshot(
        &fixture([TIP, TIP], ViewBehavior::MovePinnedTip),
        Hex32::from_bytes([41; 32]),
        accounts(),
    )
    .await;

    assert_eq!(result, Err(BridgeRuntimeError::MovingTip));
}

#[tokio::test]
async fn regressing_finalized_height_fails_closed() {
    let result = read_stable_m4_finalized_nonce_snapshot(
        &fixture([TIP, TIP - 1], ViewBehavior::Stable),
        Hex32::from_bytes([41; 32]),
        accounts(),
    )
    .await;

    assert_eq!(result, Err(BridgeRuntimeError::MovingTip));
}

#[tokio::test]
async fn wrong_genesis_fails_closed_before_snapshot_authority() {
    let result = read_stable_m4_finalized_nonce_snapshot(
        &fixture([TIP, TIP], ViewBehavior::Stable),
        Hex32::from_bytes([99; 32]),
        accounts(),
    )
    .await;

    assert_eq!(result, Err(BridgeRuntimeError::InvalidObservation));
}

#[test]
fn checked_m4_program_id_rejects_one_bit_mutation() {
    validate_checked_m4_escrow_program_id(CHECKED_M4_ESCROW_PROGRAM_ID)
        .expect("checked program ID");
    let mut mutated = CHECKED_M4_ESCROW_PROGRAM_ID;
    mutated[0] ^= 1;
    assert_eq!(
        validate_checked_m4_escrow_program_id(mutated),
        Err(BridgeRuntimeError::InvalidObservation)
    );
}
