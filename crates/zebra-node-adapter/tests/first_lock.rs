use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use lez_swap_core::{Participant, SwapDirection, UnixSeconds};
use lez_zebra_node_adapter::{
    ZebraCanonicalBlock, ZebraChainIdentity, ZebraChainInfo, ZebraFirstLockError, ZebraRpc,
    ZebraRpcChain, ZebraRpcSwapPort, ZebraTransactionState, ZebraUnspentOutput,
};
use lez_zec_swap_sdk::{
    Bip199Contract, CanonicalZcashOutputObservation, FirstLockConfirmedEvidenceV1,
    FirstLockObservation, FirstLockStepV1, LezAssetV1, LezChainIdentityV1, LezEnvironmentV1,
    MakerLockObservationV1, NegotiationTranscriptV1, PreparedFirstLockSubmissionV1,
    TakerFirstLockObservationV1, TransparentFundingRequest, TransparentUtxo,
    ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashFirstLockPort, ZcashMakerLockObservationPort,
    ZcashNodeSnapshot, ZcashStableTip, ZcashTakerFirstLockObservationPort,
    ZcashTransparentDestinationV1, ZecAgreementBodyV1, ZecAgreementRecordV1, ZecAgreementV1,
    ZecLezTermsV1, ZecParticipantIdentityV1, ZecParticipantsV1, ZecProfileId, ZecProfileRecordV1,
    ZecRefundPlanV1, ZecRefundProfile, ZecSwapBinding, ZecSwapBindingRecordV1,
    ZecTransactionPolicyV1, build_funding_transaction, derive_lez_metadata_account_v1,
    derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use zcash_primitives::{
    block::BlockHash,
    transaction::{Authorized, Transaction, TransactionData, TxVersion},
};
use zcash_protocol::{
    TxId,
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{OutPoint, TxOut},
};

const INCLUSION_HEIGHT: u32 = 100;
const TIP_HEIGHT: u32 = 104;

#[derive(Clone, Debug, thiserror::Error)]
#[error("fake Zebra RPC failure")]
struct FakeRpcError;

#[derive(Clone, Debug)]
struct FakeRpc {
    state: Arc<Mutex<FakeState>>,
}

#[derive(Debug)]
struct FakeState {
    chain_infos: VecDeque<ZebraChainInfo>,
    genesis_hashes: VecDeque<BlockHash>,
    canonical_inclusion_hash: BlockHash,
    canonical_blocks: Vec<ZebraCanonicalBlock>,
    canonical_block_override: Option<ZebraCanonicalBlock>,
    block_transactions: Vec<(TxId, BlockHash, Vec<u8>)>,
    mempool_transactions: Vec<(TxId, Vec<u8>)>,
    raw_transaction: Option<Vec<u8>>,
    transaction_state: Option<ZebraTransactionState>,
    submitted_transaction_id: TxId,
    submitted: Vec<Vec<u8>>,
    calls: Vec<String>,
}

impl FakeRpc {
    fn confirmed(
        agreement: &ZecAgreementV1,
        prepared: &PreparedFirstLockSubmissionV1,
    ) -> (Self, ZebraChainIdentity) {
        let identity = identity_for(agreement);
        let tip_hash = BlockHash([0x33; 32]);
        let inclusion_hash = BlockHash([0x22; 32]);
        let info = ZebraChainInfo::new(
            identity.rpc_chain(),
            BlockHeight::from_u32(TIP_HEIGHT),
            tip_hash,
            identity.consensus_branch_id(),
        );
        let raw = prepared.exact_submission().to_vec();
        (
            Self {
                state: Arc::new(Mutex::new(FakeState {
                    chain_infos: VecDeque::from([info, info]),
                    genesis_hashes: VecDeque::from([
                        identity.genesis_hash(),
                        identity.genesis_hash(),
                    ]),
                    canonical_inclusion_hash: inclusion_hash,
                    canonical_blocks: vec![],
                    canonical_block_override: None,
                    block_transactions: vec![],
                    mempool_transactions: vec![],
                    raw_transaction: Some(raw.clone()),
                    transaction_state: Some(ZebraTransactionState::Confirmed {
                        raw_transaction: raw,
                        block_hash: inclusion_hash,
                        block_height: BlockHeight::from_u32(INCLUSION_HEIGHT),
                        confirmations: TIP_HEIGHT - INCLUSION_HEIGHT + 1,
                        in_active_chain: true,
                    }),
                    submitted_transaction_id: TxId::from_bytes(*prepared.expected_submission_id()),
                    submitted: vec![],
                    calls: vec![],
                })),
            },
            identity,
        )
    }

    fn discoverable(
        agreement: &ZecAgreementV1,
        prepared: &PreparedFirstLockSubmissionV1,
        inclusion_height: u32,
        tip_height: u32,
    ) -> (Self, ZebraChainIdentity) {
        let identity = identity_for(agreement);
        let tip_hash = block_hash_for(tip_height);
        let info = ZebraChainInfo::new(
            identity.rpc_chain(),
            BlockHeight::from_u32(tip_height),
            tip_hash,
            identity.consensus_branch_id(),
        );
        let transaction_id = TxId::from_bytes(*prepared.expected_submission_id());
        let anchor = funding_anchor(agreement);
        let canonical_blocks = (anchor..=tip_height)
            .map(|height| {
                ZebraCanonicalBlock::new(
                    block_hash_for(height),
                    BlockHeight::from_u32(height),
                    if height == inclusion_height {
                        vec![transaction_id]
                    } else {
                        vec![]
                    },
                )
            })
            .collect();
        (
            Self {
                state: Arc::new(Mutex::new(FakeState {
                    chain_infos: VecDeque::from([info, info]),
                    genesis_hashes: VecDeque::from([identity.genesis_hash()]),
                    canonical_inclusion_hash: block_hash_for(inclusion_height),
                    canonical_blocks,
                    canonical_block_override: None,
                    block_transactions: vec![(
                        transaction_id,
                        block_hash_for(inclusion_height),
                        prepared.exact_submission().to_vec(),
                    )],
                    mempool_transactions: vec![],
                    raw_transaction: None,
                    transaction_state: None,
                    submitted_transaction_id: transaction_id,
                    submitted: vec![],
                    calls: vec![],
                })),
            },
            identity,
        )
    }

    fn edit(&self, edit: impl FnOnce(&mut FakeState)) {
        edit(&mut self.state.lock().expect("fake state lock"));
    }

    fn calls(&self) -> Vec<String> {
        self.state.lock().expect("fake state lock").calls.clone()
    }

    fn submitted(&self) -> Vec<Vec<u8>> {
        self.state
            .lock()
            .expect("fake state lock")
            .submitted
            .clone()
    }
}

#[async_trait]
impl ZebraRpc for FakeRpc {
    type Error = FakeRpcError;

    async fn chain_info(&self) -> Result<ZebraChainInfo, Self::Error> {
        let mut state = self.state.lock().expect("fake state lock");
        state.calls.push("chain_info".to_owned());
        Ok(state
            .chain_infos
            .pop_front()
            .expect("each test supplies every chain-info sample"))
    }

    async fn block_hash(&self, height: BlockHeight) -> Result<BlockHash, Self::Error> {
        let mut state = self.state.lock().expect("fake state lock");
        state
            .calls
            .push(format!("block_hash:{}", u32::from(height)));
        if u32::from(height) == 0 {
            Ok(state
                .genesis_hashes
                .pop_front()
                .expect("each test supplies every genesis sample"))
        } else if let Some(block) = state
            .canonical_blocks
            .iter()
            .find(|block| block.block_height() == height)
        {
            Ok(block.block_hash())
        } else {
            Ok(state.canonical_inclusion_hash)
        }
    }

    async fn canonical_block(
        &self,
        block_hash: BlockHash,
    ) -> Result<ZebraCanonicalBlock, Self::Error> {
        let mut state = self.state.lock().expect("fake state lock");
        state.calls.push("canonical_block".to_owned());
        if let Some(block) = &state.canonical_block_override {
            return Ok(block.clone());
        }
        Ok(state
            .canonical_blocks
            .iter()
            .find(|block| block.block_hash() == block_hash)
            .expect("each scan supplies every canonical block")
            .clone())
    }

    async fn block_transaction(
        &self,
        transaction_id: TxId,
        block_hash: BlockHash,
    ) -> Result<Option<Vec<u8>>, Self::Error> {
        let mut state = self.state.lock().expect("fake state lock");
        state.calls.push("block_transaction".to_owned());
        Ok(state
            .block_transactions
            .iter()
            .find(|(candidate_id, candidate_hash, _)| {
                *candidate_id == transaction_id && *candidate_hash == block_hash
            })
            .map(|(_, _, raw)| raw.clone()))
    }

    async fn mempool_transaction_ids(&self) -> Result<Vec<TxId>, Self::Error> {
        let mut state = self.state.lock().expect("fake state lock");
        state.calls.push("mempool_transaction_ids".to_owned());
        Ok(state
            .mempool_transactions
            .iter()
            .map(|(transaction_id, _)| *transaction_id)
            .collect())
    }

    async fn raw_transaction(&self, transaction_id: TxId) -> Result<Option<Vec<u8>>, Self::Error> {
        let mut state = self.state.lock().expect("fake state lock");
        state.calls.push("raw_transaction".to_owned());
        Ok(state
            .mempool_transactions
            .iter()
            .find(|(candidate_id, _)| *candidate_id == transaction_id)
            .map(|(_, raw)| raw.clone())
            .or_else(|| state.raw_transaction.clone()))
    }

    async fn transaction_state(
        &self,
        _transaction_id: TxId,
    ) -> Result<Option<ZebraTransactionState>, Self::Error> {
        let mut state = self.state.lock().expect("fake state lock");
        state.calls.push("transaction_state".to_owned());
        Ok(state.transaction_state.clone())
    }

    async fn unspent_output(
        &self,
        _outpoint: &OutPoint,
    ) -> Result<Option<ZebraUnspentOutput>, Self::Error> {
        panic!("first-lock tests never query the UTXO set")
    }

    async fn send_raw_transaction(&self, transaction: &[u8]) -> Result<TxId, Self::Error> {
        let mut state = self.state.lock().expect("fake state lock");
        state.calls.push("send_raw_transaction".to_owned());
        state.submitted.push(transaction.to_vec());
        Ok(state.submitted_transaction_id)
    }
}

#[tokio::test]
async fn stable_confirmed_absent_mempool_and_moving_tip_are_distinct() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    let prepared = prepared(&agreement);

    let (rpc, identity) = FakeRpc::confirmed(&agreement, &prepared);
    let port = ZebraRpcSwapPort::new(rpc.clone(), identity, Participant::Taker);
    let expected = FirstLockConfirmedEvidenceV1::new(
        FirstLockStepV1::ZcashFund,
        *prepared.expected_submission_id(),
        TxId::from_bytes(*prepared.expected_submission_id()).to_string(),
        TIP_HEIGHT - INCLUSION_HEIGHT + 1,
    )
    .expect("canonical evidence");
    assert_eq!(
        port.observe_first_lock(&agreement, &prepared)
            .await
            .expect("confirmed"),
        FirstLockObservation::Confirmed(expected)
    );
    assert_eq!(
        rpc.calls(),
        [
            "chain_info",
            "block_hash:0",
            "raw_transaction",
            "transaction_state",
            "block_hash:100",
            "chain_info",
        ]
    );

    let (absent, identity) = FakeRpc::confirmed(&agreement, &prepared);
    absent.edit(|state| {
        state.raw_transaction = None;
        state.transaction_state = None;
    });
    assert_eq!(
        ZebraRpcSwapPort::new(absent.clone(), identity, Participant::Taker)
            .observe_first_lock(&agreement, &prepared)
            .await
            .expect("stable absence"),
        FirstLockObservation::Absent
    );
    assert_eq!(
        absent.calls(),
        [
            "chain_info",
            "block_hash:0",
            "raw_transaction",
            "chain_info"
        ]
    );

    let (moving, identity) = FakeRpc::confirmed(&agreement, &prepared);
    moving.edit(|state| {
        state.raw_transaction = None;
        state.transaction_state = None;
        let after = state.chain_infos[1];
        state.chain_infos[1] = ZebraChainInfo::new(
            after.rpc_chain(),
            BlockHeight::from_u32(TIP_HEIGHT + 1),
            BlockHash([0x44; 32]),
            after.consensus_branch_id(),
        );
    });
    assert_eq!(
        ZebraRpcSwapPort::new(moving, identity, Participant::Taker)
            .observe_first_lock(&agreement, &prepared)
            .await
            .expect("moving tip is not absence"),
        FirstLockObservation::Unstable
    );

    let (mempool, identity) = FakeRpc::confirmed(&agreement, &prepared);
    mempool.edit(|state| {
        state.transaction_state = Some(ZebraTransactionState::Mempool {
            raw_transaction: prepared.exact_submission().to_vec(),
        });
    });
    assert_eq!(
        ZebraRpcSwapPort::new(mempool, identity, Participant::Taker)
            .observe_first_lock(&agreement, &prepared)
            .await
            .expect("mempool is visible but unconfirmed"),
        FirstLockObservation::Unstable
    );
}

#[tokio::test]
async fn reverse_maker_observes_and_submits_exact_zcash_funding() {
    let agreement = agreement(SwapDirection::TakerSellsLez);
    let prepared = prepared(&agreement);
    let expected = FirstLockConfirmedEvidenceV1::new(
        FirstLockStepV1::ZcashFund,
        *prepared.expected_submission_id(),
        TxId::from_bytes(*prepared.expected_submission_id()).to_string(),
        TIP_HEIGHT - INCLUSION_HEIGHT + 1,
    )
    .expect("canonical reverse-maker evidence");

    let (observe_rpc, identity) = FakeRpc::confirmed(&agreement, &prepared);
    let maker_port = ZebraRpcSwapPort::new(observe_rpc, identity, Participant::Maker);
    assert_eq!(maker_port.local_participant(), Participant::Maker);
    assert_eq!(
        maker_port
            .observe_first_lock(&agreement, &prepared)
            .await
            .expect("reverse maker observes its exact Zcash funding"),
        FirstLockObservation::Confirmed(expected)
    );

    let (submitted, identity) = FakeRpc::confirmed(&agreement, &prepared);
    ZebraRpcSwapPort::new(submitted.clone(), identity, Participant::Maker)
        .submit_first_lock(&agreement, &prepared)
        .await
        .expect("reverse maker submits its exact Zcash funding once");
    assert_eq!(
        submitted.submitted(),
        vec![prepared.exact_submission().to_vec()]
    );

    let (unknown, identity) = FakeRpc::confirmed(&agreement, &prepared);
    unknown.edit(move_after_tip);
    assert!(matches!(
        ZebraRpcSwapPort::new(unknown.clone(), identity, Participant::Maker)
            .submit_first_lock(&agreement, &prepared)
            .await,
        Err(ZebraFirstLockError::UnstableTipDuringSubmission)
    ));
    assert_eq!(
        unknown.submitted(),
        vec![prepared.exact_submission().to_vec()],
        "an unknown reverse submission outcome is attempted exactly once"
    );
    assert_eq!(
        unknown
            .calls()
            .iter()
            .filter(|call| call.as_str() == "send_raw_transaction")
            .count(),
        1,
        "the adapter never retries an ambiguous reverse submission"
    );
}

#[tokio::test]
async fn role_fixed_observers_discover_unknown_id_funding_in_both_directions() {
    let forward = agreement(SwapDirection::TakerSellsForeign);
    let forward_prepared = prepared(&forward);
    let (forward_rpc, identity) = FakeRpc::discoverable(&forward, &forward_prepared, 116, 120);
    let forward_observation = ZebraRpcSwapPort::new(forward_rpc, identity, Participant::Maker)
        .observe_taker_first_lock(&forward, None)
        .await
        .expect("maker discovers the unknown-ID taker funding");
    let TakerFirstLockObservationV1::CanonicalZcash(forward_canonical) = forward_observation else {
        panic!("forward observer must return complete canonical Zcash evidence");
    };
    assert_eq!(
        forward_canonical.transaction_id(),
        TxId::from_bytes(*forward_prepared.expected_submission_id())
    );
    assert_eq!(forward_canonical.block_height(), BlockHeight::from_u32(116));
    assert_eq!(forward_canonical.tip_height(), BlockHeight::from_u32(120));

    let reverse = agreement(SwapDirection::TakerSellsLez);
    let reverse_prepared = prepared(&reverse);
    let (reverse_rpc, identity) = FakeRpc::discoverable(&reverse, &reverse_prepared, 116, 120);
    let reverse_observation = ZebraRpcSwapPort::new(reverse_rpc, identity, Participant::Taker)
        .observe_maker_lock(&reverse)
        .await
        .expect("taker discovers the unknown-ID maker funding");
    assert_eq!(
        reverse_observation,
        MakerLockObservationV1::Confirmed(
            FirstLockConfirmedEvidenceV1::new(
                FirstLockStepV1::ZcashFund,
                *reverse_prepared.expected_submission_id(),
                TxId::from_bytes(*reverse_prepared.expected_submission_id()).to_string(),
                5,
            )
            .expect("canonical reverse maker evidence"),
        )
    );
}

#[tokio::test]
async fn observer_absence_and_instability_require_complete_stable_coverage() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    let prepared = prepared(&agreement);

    let (absent_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    absent_rpc.edit(clear_canonical_transactions);
    assert_eq!(
        ZebraRpcSwapPort::new(absent_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, None)
            .await
            .expect("complete stable scan proves absence"),
        TakerFirstLockObservationV1::Absent
    );

    let (mempool_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    let mempool_id = TxId::from_bytes(*prepared.expected_submission_id());
    let mempool_raw = prepared.exact_submission().to_vec();
    mempool_rpc.edit(|state| {
        clear_canonical_transactions(state);
        state.mempool_transactions.push((mempool_id, mempool_raw));
    });
    assert_eq!(
        ZebraRpcSwapPort::new(mempool_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, None)
            .await
            .expect("mempool funding is not canonical"),
        TakerFirstLockObservationV1::Unstable
    );

    let (before_anchor_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 115);
    assert_eq!(
        ZebraRpcSwapPort::new(before_anchor_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, None)
            .await
            .expect("a node tip behind the signed anchor is inconclusive"),
        TakerFirstLockObservationV1::Unstable
    );

    let (short_bound_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    assert_eq!(
        ZebraRpcSwapPort::new(short_bound_rpc, identity, Participant::Maker)
            .with_counterparty_scan_blocks(4)
            .observe_taker_first_lock(&agreement, None)
            .await
            .expect("a bound shorter than the inclusive profile horizon is inconclusive"),
        TakerFirstLockObservationV1::Unstable
    );

    let (exhausted_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 121);
    assert_eq!(
        ZebraRpcSwapPort::new(exhausted_rpc, identity, Participant::Maker)
            .with_counterparty_scan_blocks(5)
            .observe_taker_first_lock(&agreement, None)
            .await
            .expect("an exhausted full-window scan is inconclusive"),
        TakerFirstLockObservationV1::Unstable
    );

    let (moving_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    moving_rpc.edit(move_after_tip);
    assert_eq!(
        ZebraRpcSwapPort::new(moving_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, None)
            .await
            .expect("moving tip prevents canonical classification"),
        TakerFirstLockObservationV1::Unstable
    );
}

#[tokio::test]
async fn observer_roles_directions_and_chain_identity_fail_closed() {
    let forward = agreement(SwapDirection::TakerSellsForeign);
    let forward_prepared = prepared(&forward);

    let (wrong_role_rpc, identity) = FakeRpc::discoverable(&forward, &forward_prepared, 116, 120);
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_role_rpc.clone(), identity, Participant::Taker)
            .observe_taker_first_lock(&forward, None)
            .await,
        Err(ZebraFirstLockError::WrongRole {
            expected: Participant::Maker,
            actual: Participant::Taker,
        })
    ));
    assert!(wrong_role_rpc.calls().is_empty());

    let (wrong_direction_rpc, identity) =
        FakeRpc::discoverable(&forward, &forward_prepared, 116, 120);
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_direction_rpc.clone(), identity, Participant::Taker)
            .observe_maker_lock(&forward)
            .await,
        Err(ZebraFirstLockError::WrongObservedFunder {
            expected: Participant::Maker,
            actual: Participant::Taker,
        })
    ));
    assert!(wrong_direction_rpc.calls().is_empty());

    let reverse = agreement(SwapDirection::TakerSellsLez);
    let reverse_prepared = prepared(&reverse);
    let (reverse_role_rpc, identity) = FakeRpc::discoverable(&reverse, &reverse_prepared, 116, 120);
    assert!(matches!(
        ZebraRpcSwapPort::new(reverse_role_rpc.clone(), identity, Participant::Maker)
            .observe_maker_lock(&reverse)
            .await,
        Err(ZebraFirstLockError::WrongRole {
            expected: Participant::Taker,
            actual: Participant::Maker,
        })
    ));
    assert!(reverse_role_rpc.calls().is_empty());

    let (reverse_direction_rpc, identity) =
        FakeRpc::discoverable(&reverse, &reverse_prepared, 116, 120);
    assert!(matches!(
        ZebraRpcSwapPort::new(reverse_direction_rpc.clone(), identity, Participant::Maker,)
            .observe_taker_first_lock(&reverse, None)
            .await,
        Err(ZebraFirstLockError::WrongObservedFunder {
            expected: Participant::Taker,
            actual: Participant::Maker,
        })
    ));
    assert!(reverse_direction_rpc.calls().is_empty());

    let (wrong_network_rpc, _) = FakeRpc::discoverable(&forward, &forward_prepared, 116, 120);
    let wrong_network = ZebraChainIdentity::new(
        NetworkType::Test,
        ZebraRpcChain::Test,
        BranchId::Nu6_2,
        BlockHash([0x55; 32]),
    )
    .expect("valid wrong-network identity");
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_network_rpc.clone(), wrong_network, Participant::Maker,)
            .observe_taker_first_lock(&forward, None)
            .await,
        Err(ZebraFirstLockError::ConfiguredNetworkMismatch)
    ));
    assert!(wrong_network_rpc.calls().is_empty());

    let (wrong_branch_rpc, identity) = FakeRpc::discoverable(&forward, &forward_prepared, 116, 120);
    wrong_branch_rpc.edit(|state| {
        let before = state.chain_infos[0];
        state.chain_infos[0] = ZebraChainInfo::new(
            before.rpc_chain(),
            before.tip_height(),
            before.tip_hash(),
            BranchId::Nu6_1,
        );
    });
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_branch_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&forward, None)
            .await,
        Err(ZebraFirstLockError::RpcConsensusBranchMismatch)
    ));

    let (wrong_genesis_rpc, identity) =
        FakeRpc::discoverable(&forward, &forward_prepared, 116, 120);
    wrong_genesis_rpc.edit(|state| {
        state.genesis_hashes = VecDeque::from([BlockHash([0xee; 32])]);
    });
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_genesis_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&forward, None)
            .await,
        Err(ZebraFirstLockError::GenesisMismatch)
    ));
}

#[tokio::test]
async fn observer_inventory_matrix_fails_closed() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    let prepared = prepared(&agreement);
    let original_id = TxId::from_bytes(*prepared.expected_submission_id());
    let alternate_transaction = funding_transaction_with_input(
        &agreement,
        agreement.binding().expected_output().contract(),
        agreement.binding().expected_output().value(),
        [0x88; 32],
    );
    let alternate = prepared_from(&alternate_transaction);
    let alternate_id = TxId::from_bytes(*alternate.expected_submission_id());

    let (ambiguous_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    let alternate_raw = alternate.exact_submission().to_vec();
    ambiguous_rpc.edit(|state| {
        let block = state
            .canonical_blocks
            .iter_mut()
            .find(|block| u32::from(block.block_height()) == 116)
            .expect("funding block");
        let block_hash = block.block_hash();
        *block = ZebraCanonicalBlock::new(
            block_hash,
            BlockHeight::from_u32(116),
            vec![original_id, alternate_id],
        );
        state
            .block_transactions
            .push((alternate_id, block_hash, alternate_raw));
    });
    assert!(matches!(
        ZebraRpcSwapPort::new(ambiguous_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, None)
            .await,
        Err(ZebraFirstLockError::AmbiguousFundingCandidates)
    ));

    let (wrong_hash_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    wrong_hash_rpc.edit(|state| {
        state.canonical_block_override = Some(ZebraCanonicalBlock::new(
            BlockHash([0xee; 32]),
            BlockHeight::from_u32(116),
            vec![original_id],
        ));
    });
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_hash_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, None)
            .await,
        Err(ZebraFirstLockError::BlockInventoryMismatch)
    ));

    let (wrong_height_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    wrong_height_rpc.edit(|state| {
        state.canonical_block_override = Some(ZebraCanonicalBlock::new(
            block_hash_for(116),
            BlockHeight::from_u32(117),
            vec![original_id],
        ));
    });
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_height_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, None)
            .await,
        Err(ZebraFirstLockError::BlockInventoryMismatch)
    ));

    let (missing_transaction_rpc, identity) =
        FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    missing_transaction_rpc.edit(|state| state.block_transactions.clear());
    assert!(matches!(
        ZebraRpcSwapPort::new(missing_transaction_rpc, identity, Participant::Maker,)
            .observe_taker_first_lock(&agreement, None)
            .await,
        Err(ZebraFirstLockError::BlockInventoryMismatch)
    ));
}

#[tokio::test]
async fn observer_transaction_and_exact_output_matrix_fails_closed() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    let prepared = prepared(&agreement);
    let alternate_transaction = funding_transaction_with_input(
        &agreement,
        agreement.binding().expected_output().contract(),
        agreement.binding().expected_output().value(),
        [0x88; 32],
    );
    let alternate = prepared_from(&alternate_transaction);

    let (wrong_id_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    let alternate_raw = alternate.exact_submission().to_vec();
    wrong_id_rpc.edit(|state| state.block_transactions[0].2 = alternate_raw);
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_id_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, None)
            .await,
        Err(ZebraFirstLockError::DiscoveredTransactionIdMismatch)
    ));

    let (malformed_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    malformed_rpc.edit(|state| state.block_transactions[0].2 = vec![0xff]);
    assert!(matches!(
        ZebraRpcSwapPort::new(malformed_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, None)
            .await,
        Err(ZebraFirstLockError::MalformedDiscoveredTransaction(_))
    ));

    let (trailing_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    trailing_rpc.edit(|state| state.block_transactions[0].2.push(0));
    assert!(matches!(
        ZebraRpcSwapPort::new(trailing_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, None)
            .await,
        Err(ZebraFirstLockError::TrailingDiscoveredTransactionBytes)
    ));

    let wrong_value = prepared_from(&funding_transaction(
        &agreement,
        agreement.binding().expected_output().contract(),
        Zatoshis::from_u64(99_999_999).expect("wrong value"),
    ));
    let (wrong_value_rpc, identity) = FakeRpc::discoverable(&agreement, &wrong_value, 116, 120);
    assert_eq!(
        ZebraRpcSwapPort::new(wrong_value_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, None)
            .await
            .expect("wrong value is a nonmatching canonical transaction"),
        TakerFirstLockObservationV1::Absent
    );

    let wrong_contract = Bip199Contract::new(120, [0xaa; 20], [0xbb; 32], [0xcc; 20]);
    let wrong_script = prepared_from(&funding_transaction(
        &agreement,
        &wrong_contract,
        agreement.binding().expected_output().value(),
    ));
    let (wrong_script_rpc, identity) = FakeRpc::discoverable(&agreement, &wrong_script, 116, 120);
    assert_eq!(
        ZebraRpcSwapPort::new(wrong_script_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, None)
            .await
            .expect("wrong script is a nonmatching canonical transaction"),
        TakerFirstLockObservationV1::Absent
    );

    let later_vout = prepared_from(&funding_transaction_with_expected_output_at_vout_one(
        &agreement,
    ));
    let (later_vout_rpc, identity) = FakeRpc::discoverable(&agreement, &later_vout, 116, 120);
    assert_eq!(
        ZebraRpcSwapPort::new(later_vout_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, None)
            .await
            .expect("a matching later vout never substitutes for protocol vout zero"),
        TakerFirstLockObservationV1::Absent
    );
}

#[tokio::test]
async fn taker_funding_observer_reconciles_same_removed_and_replaced_heads() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    let prepared = prepared(&agreement);
    let (initial_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    let initial = match ZebraRpcSwapPort::new(initial_rpc, identity, Participant::Maker)
        .observe_taker_first_lock(&agreement, None)
        .await
        .expect("initial canonical funding")
    {
        TakerFirstLockObservationV1::CanonicalZcash(canonical) => *canonical,
        other => panic!("expected canonical initial funding, got {other:?}"),
    };

    let (same_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    assert_eq!(
        ZebraRpcSwapPort::new(same_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, Some(&initial))
            .await
            .expect("same canonical inclusion remains observable"),
        TakerFirstLockObservationV1::CanonicalZcash(Box::new(initial.clone()))
    );

    let (removed_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    removed_rpc.edit(|state| {
        clear_canonical_transactions(state);
        let block = state
            .canonical_blocks
            .iter_mut()
            .find(|block| u32::from(block.block_height()) == 116)
            .expect("previous inclusion height");
        *block =
            ZebraCanonicalBlock::new(BlockHash([0xa1; 32]), BlockHeight::from_u32(116), vec![]);
    });
    let removed = ZebraRpcSwapPort::new(removed_rpc, identity, Participant::Maker)
        .observe_taker_first_lock(&agreement, Some(&initial))
        .await
        .expect("affirmative canonical removal");
    let TakerFirstLockObservationV1::ZcashRemoved(removed) = removed else {
        panic!("detached predecessor must emit removal evidence");
    };
    assert_eq!(removed.previous(), &initial);

    let alternate_transaction = funding_transaction_with_input(
        &agreement,
        agreement.binding().expected_output().contract(),
        agreement.binding().expected_output().value(),
        [0x88; 32],
    );
    let alternate = prepared_from(&alternate_transaction);
    let alternate_id = TxId::from_bytes(*alternate.expected_submission_id());
    let alternate_raw = alternate.exact_submission().to_vec();
    let (replaced_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    replaced_rpc.edit(|state| {
        clear_canonical_transactions(state);
        let removed_block = state
            .canonical_blocks
            .iter_mut()
            .find(|block| u32::from(block.block_height()) == 116)
            .expect("previous inclusion height");
        *removed_block =
            ZebraCanonicalBlock::new(BlockHash([0xa2; 32]), BlockHeight::from_u32(116), vec![]);
        let replacement_block = state
            .canonical_blocks
            .iter_mut()
            .find(|block| u32::from(block.block_height()) == 117)
            .expect("replacement inclusion height");
        let replacement_hash = replacement_block.block_hash();
        *replacement_block = ZebraCanonicalBlock::new(
            replacement_hash,
            BlockHeight::from_u32(117),
            vec![alternate_id],
        );
        state
            .block_transactions
            .push((alternate_id, replacement_hash, alternate_raw));
    });
    let replaced = ZebraRpcSwapPort::new(replaced_rpc, identity, Participant::Maker)
        .observe_taker_first_lock(&agreement, Some(&initial))
        .await
        .expect("same-poll canonical replacement");
    let TakerFirstLockObservationV1::ZcashReplaced { removed, canonical } = replaced else {
        panic!("detached predecessor plus new funding must be one replacement");
    };
    assert_eq!(removed.previous(), &initial);
    assert_eq!(canonical.transaction_id(), alternate_id);
    assert_eq!(removed.tip_block_hash(), canonical.tip_block_hash());
    assert_eq!(removed.tip_height(), canonical.tip_height());
}

#[tokio::test]
async fn taker_funding_observer_treats_tip_retreat_as_unstable() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    let prepared = prepared(&agreement);
    let (initial_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 120);
    let initial = match ZebraRpcSwapPort::new(initial_rpc, identity, Participant::Maker)
        .observe_taker_first_lock(&agreement, None)
        .await
        .expect("initial canonical funding")
    {
        TakerFirstLockObservationV1::CanonicalZcash(canonical) => *canonical,
        other => panic!("expected canonical initial funding, got {other:?}"),
    };

    let (retreated_rpc, identity) = FakeRpc::discoverable(&agreement, &prepared, 116, 115);
    assert_eq!(
        ZebraRpcSwapPort::new(retreated_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, Some(&initial))
            .await
            .expect("a tip below the prior inclusion is transient instability"),
        TakerFirstLockObservationV1::Unstable
    );

    let anchor = funding_anchor(&agreement);
    let below_anchor = anchor.checked_sub(1).expect("nonzero funding anchor");
    let tip_height = 120;
    let below_anchor_observation = CanonicalZcashOutputObservation::validate(
        agreement.binding().expected_output(),
        &ZcashNodeSnapshot::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            true,
            block_hash_for(below_anchor),
            block_hash_for(below_anchor),
            BlockHeight::from_u32(below_anchor),
            ZcashStableTip::new(
                block_hash_for(tip_height),
                BlockHeight::from_u32(tip_height),
                block_hash_for(tip_height),
                BlockHeight::from_u32(tip_height),
            ),
            TxId::from_bytes(*prepared.expected_submission_id()),
            prepared.exact_submission().to_vec(),
            0,
            tip_height - below_anchor + 1,
        ),
    )
    .expect("valid canonical observation below this agreement's funding window");
    let (below_anchor_rpc, identity) =
        FakeRpc::discoverable(&agreement, &prepared, 116, tip_height);
    assert!(matches!(
        ZebraRpcSwapPort::new(below_anchor_rpc, identity, Participant::Maker)
            .observe_taker_first_lock(&agreement, Some(&below_anchor_observation))
            .await,
        Err(ZebraFirstLockError::PreviousObservationOutsideFundingWindow)
    ));
}

#[tokio::test]
async fn moving_tip_prevents_found_or_mempool_classification() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    let prepared = prepared(&agreement);

    let (found, identity) = FakeRpc::confirmed(&agreement, &prepared);
    found.edit(|state| {
        state.raw_transaction = Some(mutate_authorization(prepared.exact_submission()));
        move_after_tip(state);
    });
    assert_eq!(
        ZebraRpcSwapPort::new(found, identity, Participant::Taker)
            .observe_first_lock(&agreement, &prepared)
            .await
            .expect("moving found snapshot is not classified"),
        FirstLockObservation::Unstable
    );

    let (mempool, identity) = FakeRpc::confirmed(&agreement, &prepared);
    mempool.edit(|state| {
        state.transaction_state = Some(ZebraTransactionState::Mempool {
            raw_transaction: prepared.exact_submission().to_vec(),
        });
        move_after_tip(state);
    });
    assert_eq!(
        ZebraRpcSwapPort::new(mempool, identity, Participant::Taker)
            .observe_first_lock(&agreement, &prepared)
            .await
            .expect("moving mempool snapshot is not classified"),
        FirstLockObservation::Unstable
    );
}

fn move_after_tip(state: &mut FakeState) {
    let after = state.chain_infos[1];
    state.chain_infos[1] = ZebraChainInfo::new(
        after.rpc_chain(),
        BlockHeight::from_u32(TIP_HEIGHT + 1),
        BlockHash([0x44; 32]),
        after.consensus_branch_id(),
    );
}

#[tokio::test]
async fn local_transaction_and_agreement_negative_matrix_precedes_every_rpc() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    let valid = prepared(&agreement);
    let (rpc, identity) = FakeRpc::confirmed(&agreement, &valid);

    let wrong_step = PreparedFirstLockSubmissionV1::new(
        FirstLockStepV1::LezInitialize,
        *valid.expected_submission_id(),
        valid.exact_submission().to_vec(),
    )
    .expect("bounded wrong-step fixture");
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), identity, Participant::Taker)
            .observe_first_lock(&agreement, &wrong_step)
            .await,
        Err(ZebraFirstLockError::WrongStep(
            FirstLockStepV1::LezInitialize
        ))
    ));

    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), identity, Participant::Maker)
            .observe_first_lock(&agreement, &valid)
            .await,
        Err(ZebraFirstLockError::WrongRole {
            expected: Participant::Taker,
            actual: Participant::Maker,
        })
    ));

    let reverse = self::agreement(SwapDirection::TakerSellsLez);
    let reverse_prepared = prepared(&reverse);
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), identity, Participant::Taker)
            .observe_first_lock(&reverse, &reverse_prepared)
            .await,
        Err(ZebraFirstLockError::WrongRole {
            expected: Participant::Maker,
            actual: Participant::Taker,
        })
    ));

    let wrong_id = PreparedFirstLockSubmissionV1::new(
        FirstLockStepV1::ZcashFund,
        [0x99; 32],
        valid.exact_submission().to_vec(),
    )
    .expect("nonzero wrong identity");
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), identity, Participant::Taker)
            .observe_first_lock(&agreement, &wrong_id)
            .await,
        Err(ZebraFirstLockError::ExpectedTransactionIdMismatch)
    ));

    let malformed =
        PreparedFirstLockSubmissionV1::new(FirstLockStepV1::ZcashFund, [1; 32], vec![0xff])
            .expect("bounded malformed fixture");
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), identity, Participant::Taker)
            .observe_first_lock(&agreement, &malformed)
            .await,
        Err(ZebraFirstLockError::MalformedSubmission(_))
    ));

    let mut trailing_bytes = valid.exact_submission().to_vec();
    trailing_bytes.push(0);
    let trailing = PreparedFirstLockSubmissionV1::new(
        FirstLockStepV1::ZcashFund,
        *valid.expected_submission_id(),
        trailing_bytes,
    )
    .expect("bounded trailing fixture");
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), identity, Participant::Taker)
            .observe_first_lock(&agreement, &trailing)
            .await,
        Err(ZebraFirstLockError::TrailingSubmissionBytes)
    ));

    let missing_output = prepared_from(&empty_transaction(TxVersion::V5));
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), identity, Participant::Taker)
            .observe_first_lock(&agreement, &missing_output)
            .await,
        Err(ZebraFirstLockError::MissingExpectedOutput)
    ));

    let wrong_version = prepared_from(&empty_transaction(TxVersion::V4));
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), identity, Participant::Taker)
            .observe_first_lock(&agreement, &wrong_version)
            .await,
        Err(ZebraFirstLockError::WrongTransactionVersion)
    ));
    assert!(rpc.calls().is_empty(), "all local failures precede RPC");
}

#[tokio::test]
async fn local_output_and_config_negative_matrix_precedes_every_rpc() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    let valid = prepared(&agreement);
    let (rpc, identity) = FakeRpc::confirmed(&agreement, &valid);

    let wrong_value_tx = funding_transaction(
        &agreement,
        agreement.binding().expected_output().contract(),
        Zatoshis::from_u64(99_999_999).expect("alternate amount"),
    );
    let wrong_value = prepared_from(&wrong_value_tx);
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), identity, Participant::Taker)
            .observe_first_lock(&agreement, &wrong_value)
            .await,
        Err(ZebraFirstLockError::OutputValueMismatch)
    ));

    let other_contract = Bip199Contract::new(120, [0xaa; 20], [0xbb; 32], [0xcc; 20]);
    let wrong_script_tx = funding_transaction(
        &agreement,
        &other_contract,
        agreement.binding().expected_output().value(),
    );
    let wrong_script = prepared_from(&wrong_script_tx);
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), identity, Participant::Taker)
            .observe_first_lock(&agreement, &wrong_script)
            .await,
        Err(ZebraFirstLockError::OutputScriptMismatch)
    ));

    let wrong_network = ZebraChainIdentity::new(
        NetworkType::Test,
        ZebraRpcChain::Test,
        BranchId::Nu6_2,
        BlockHash([0x55; 32]),
    )
    .expect("valid but agreement-incompatible identity");
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), wrong_network, Participant::Taker)
            .observe_first_lock(&agreement, &valid)
            .await,
        Err(ZebraFirstLockError::ConfiguredNetworkMismatch)
    ));
    let wrong_branch = ZebraChainIdentity::new(
        NetworkType::Regtest,
        ZebraRpcChain::Test,
        BranchId::Nu6_1,
        identity.genesis_hash(),
    )
    .expect("valid but agreement-incompatible branch identity");
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), wrong_branch, Participant::Taker)
            .observe_first_lock(&agreement, &valid)
            .await,
        Err(ZebraFirstLockError::ConfiguredConsensusBranchMismatch)
    ));
    assert!(rpc.calls().is_empty(), "all local failures precede RPC");
}

#[tokio::test]
async fn chain_identity_and_canonical_snapshot_negative_matrix_fails_closed() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    let prepared = prepared(&agreement);

    let (wrong_chain, identity) = FakeRpc::confirmed(&agreement, &prepared);
    wrong_chain.edit(|state| {
        let before = state.chain_infos[0];
        state.chain_infos[0] = ZebraChainInfo::new(
            ZebraRpcChain::Main,
            before.tip_height(),
            before.tip_hash(),
            before.consensus_branch_id(),
        );
    });
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_chain, identity, Participant::Taker)
            .observe_first_lock(&agreement, &prepared)
            .await,
        Err(ZebraFirstLockError::RpcChainMismatch)
    ));

    let (wrong_branch, identity) = FakeRpc::confirmed(&agreement, &prepared);
    wrong_branch.edit(|state| {
        let before = state.chain_infos[0];
        state.chain_infos[0] = ZebraChainInfo::new(
            before.rpc_chain(),
            before.tip_height(),
            before.tip_hash(),
            BranchId::Nu6_1,
        );
    });
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_branch, identity, Participant::Taker)
            .observe_first_lock(&agreement, &prepared)
            .await,
        Err(ZebraFirstLockError::RpcConsensusBranchMismatch)
    ));

    let (wrong_genesis, identity) = FakeRpc::confirmed(&agreement, &prepared);
    wrong_genesis.edit(|state| {
        state.genesis_hashes = VecDeque::from([BlockHash([0xee; 32])]);
    });
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_genesis, identity, Participant::Taker)
            .observe_first_lock(&agreement, &prepared)
            .await,
        Err(ZebraFirstLockError::GenesisMismatch)
    ));

    let (wrong_block, identity) = FakeRpc::confirmed(&agreement, &prepared);
    wrong_block.edit(|state| state.canonical_inclusion_hash = BlockHash([0xdd; 32]));
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_block, identity, Participant::Taker)
            .observe_first_lock(&agreement, &prepared)
            .await,
        Err(ZebraFirstLockError::Observation(
            lez_zec_swap_sdk::ObservationError::BlockHashMismatch
        ))
    ));

    let (wrong_depth, identity) = FakeRpc::confirmed(&agreement, &prepared);
    wrong_depth.edit(|state| {
        if let Some(ZebraTransactionState::Confirmed { confirmations, .. }) =
            state.transaction_state.as_mut()
        {
            *confirmations = 4;
        }
    });
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_depth, identity, Participant::Taker)
            .observe_first_lock(&agreement, &prepared)
            .await,
        Err(ZebraFirstLockError::Observation(
            lez_zec_swap_sdk::ObservationError::ConfirmationMismatch
        ))
    ));

    let (inactive, identity) = FakeRpc::confirmed(&agreement, &prepared);
    inactive.edit(|state| {
        if let Some(ZebraTransactionState::Confirmed {
            in_active_chain, ..
        }) = state.transaction_state.as_mut()
        {
            *in_active_chain = false;
        }
    });
    assert!(matches!(
        ZebraRpcSwapPort::new(inactive, identity, Participant::Taker)
            .observe_first_lock(&agreement, &prepared)
            .await,
        Err(ZebraFirstLockError::Observation(
            lez_zec_swap_sdk::ObservationError::InactiveChain
        ))
    ));
}

#[tokio::test]
async fn stable_v5_authorization_byte_mismatch_fails_closed() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    let prepared = prepared(&agreement);
    let (wrong_raw, identity) = FakeRpc::confirmed(&agreement, &prepared);
    let mutated = mutate_authorization(prepared.exact_submission());
    wrong_raw.edit(|state| {
        state.raw_transaction = Some(mutated.clone());
        state.transaction_state = Some(ZebraTransactionState::Mempool {
            raw_transaction: mutated,
        });
    });
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_raw, identity, Participant::Taker)
            .observe_first_lock(&agreement, &prepared)
            .await,
        Err(ZebraFirstLockError::ObservedRawTransactionMismatch)
    ));
}

#[tokio::test]
async fn submission_is_byte_exact_and_fails_closed_on_id_tip_or_genesis_drift() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    let prepared = prepared(&agreement);
    let (rpc, identity) = FakeRpc::confirmed(&agreement, &prepared);
    ZebraRpcSwapPort::new(rpc.clone(), identity, Participant::Taker)
        .submit_first_lock(&agreement, &prepared)
        .await
        .expect("byte-exact submission");
    assert_eq!(rpc.submitted(), vec![prepared.exact_submission().to_vec()]);
    assert_eq!(
        rpc.calls(),
        [
            "chain_info",
            "block_hash:0",
            "send_raw_transaction",
            "chain_info",
            "block_hash:0",
        ]
    );

    let (wrong_id, identity) = FakeRpc::confirmed(&agreement, &prepared);
    wrong_id.edit(|state| state.submitted_transaction_id = TxId::from_bytes([0x99; 32]));
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_id, identity, Participant::Taker)
            .submit_first_lock(&agreement, &prepared)
            .await,
        Err(ZebraFirstLockError::SubmittedTransactionIdMismatch)
    ));

    let (moving, identity) = FakeRpc::confirmed(&agreement, &prepared);
    moving.edit(|state| {
        let after = state.chain_infos[1];
        state.chain_infos[1] = ZebraChainInfo::new(
            after.rpc_chain(),
            BlockHeight::from_u32(TIP_HEIGHT + 1),
            BlockHash([0x66; 32]),
            after.consensus_branch_id(),
        );
    });
    assert!(matches!(
        ZebraRpcSwapPort::new(moving.clone(), identity, Participant::Taker)
            .submit_first_lock(&agreement, &prepared)
            .await,
        Err(ZebraFirstLockError::UnstableTipDuringSubmission)
    ));
    assert_eq!(
        moving.submitted().len(),
        1,
        "unknown outcome remains observable"
    );

    let (changed_genesis, identity) = FakeRpc::confirmed(&agreement, &prepared);
    changed_genesis.edit(|state| {
        state.genesis_hashes = VecDeque::from([identity.genesis_hash(), BlockHash([0xee; 32])]);
    });
    assert!(matches!(
        ZebraRpcSwapPort::new(changed_genesis.clone(), identity, Participant::Taker)
            .submit_first_lock(&agreement, &prepared)
            .await,
        Err(ZebraFirstLockError::GenesisMismatch)
    ));
    assert_eq!(
        changed_genesis.submitted().len(),
        1,
        "post-send genesis drift is an explicit unknown outcome"
    );
}

fn identity_for(agreement: &ZecAgreementV1) -> ZebraChainIdentity {
    match agreement.binding().expected_output().network() {
        NetworkType::Regtest => ZebraChainIdentity::deterministic_regtest_nu6_2(),
        NetworkType::Test => ZebraChainIdentity::new(
            NetworkType::Test,
            ZebraRpcChain::Test,
            agreement.binding().expected_output().consensus_branch_id(),
            BlockHash([0x55; 32]),
        )
        .expect("valid testnet fixture identity"),
        NetworkType::Main => panic!("first-lock fixtures never use mainnet"),
    }
}

fn funding_anchor(agreement: &ZecAgreementV1) -> u32 {
    agreement
        .zcash_refund_at_height()
        .checked_sub(
            ZecRefundProfile::for_id(agreement.binding().profile_id()).zcash_refund_blocks(),
        )
        .expect("validated agreement has a funding anchor")
}

fn block_hash_for(height: u32) -> BlockHash {
    let mut hash = [0; 32];
    hash[..4].copy_from_slice(&height.to_le_bytes());
    BlockHash(hash)
}

fn clear_canonical_transactions(state: &mut FakeState) {
    state.canonical_blocks = state
        .canonical_blocks
        .iter()
        .map(|block| ZebraCanonicalBlock::new(block.block_hash(), block.block_height(), vec![]))
        .collect();
    state.block_transactions.clear();
}

fn prepared(agreement: &ZecAgreementV1) -> PreparedFirstLockSubmissionV1 {
    let expected = agreement.binding().expected_output();
    prepared_from(&funding_transaction(
        agreement,
        expected.contract(),
        expected.value(),
    ))
}

fn prepared_from(transaction: &Transaction) -> PreparedFirstLockSubmissionV1 {
    let mut bytes = vec![];
    transaction.write(&mut bytes).expect("canonical V5 bytes");
    PreparedFirstLockSubmissionV1::new(
        FirstLockStepV1::ZcashFund,
        *transaction.txid().as_ref(),
        bytes,
    )
    .expect("bounded prepared transaction")
}

fn empty_transaction(version: TxVersion) -> Transaction {
    TransactionData::<Authorized>::from_parts(
        version,
        BranchId::Nu6_2,
        0,
        BlockHeight::from_u32(200),
        None,
        None,
        None,
        None,
    )
    .freeze()
    .expect("empty transaction fixture freezes")
}

fn funding_transaction(
    agreement: &ZecAgreementV1,
    contract: &Bip199Contract,
    value: Zatoshis,
) -> Transaction {
    funding_transaction_with_input(agreement, contract, value, [0x77; 32])
}

fn funding_transaction_with_input(
    agreement: &ZecAgreementV1,
    contract: &Bip199Contract,
    value: Zatoshis,
    input_transaction_id: [u8; 32],
) -> Transaction {
    let key = SecretKey::from_slice(&[7; 32]).expect("fixed funding key");
    let public_key = PublicKey::from_secret_key(&Secp256k1::new(), &key);
    let owner_script: Script = TransparentAddress::from_pubkey(&public_key).script().into();
    let input_value = Zatoshis::from_u64(u64::from(value) + 20_000).expect("input value");
    let request = TransparentFundingRequest::new(
        vec![TransparentUtxo::new(
            OutPoint::new(input_transaction_id, 0),
            TxOut::new(input_value, owner_script),
        )],
        public_key,
        value,
        Zatoshis::from_u64(1_000).expect("fee"),
        Zatoshis::from_u64(1_000).expect("change floor"),
        BlockHeight::from_u32(200),
        agreement.binding().expected_output().consensus_branch_id(),
    )
    .expect("funding request");
    build_funding_transaction(contract, &request, &key).expect("signed V5 funding transaction")
}

fn funding_transaction_with_expected_output_at_vout_one(agreement: &ZecAgreementV1) -> Transaction {
    let expected = agreement.binding().expected_output();
    let transaction = funding_transaction(agreement, expected.contract(), expected.value());
    let data = transaction.into_data();
    let mut transparent = data
        .transparent_bundle()
        .cloned()
        .expect("funding fixture has a transparent bundle");
    assert_eq!(transparent.vout.len(), 2, "funding fixture has change");
    transparent.vout.swap(0, 1);
    TransactionData::<Authorized>::from_parts(
        data.version(),
        data.consensus_branch_id(),
        data.lock_time(),
        data.expiry_height(),
        Some(transparent),
        None,
        None,
        None,
    )
    .freeze()
    .expect("reordered transparent fixture freezes")
}

fn mutate_authorization(raw: &[u8]) -> Vec<u8> {
    let transaction = Transaction::read(raw, BranchId::Nu6_2).expect("canonical transaction");
    let script_sig = &transaction
        .transparent_bundle()
        .expect("transparent transaction")
        .vin[0]
        .script_sig()
        .0
        .0;
    let mut mutated = raw.to_vec();
    let offset = mutated
        .windows(script_sig.len())
        .position(|window| window == script_sig)
        .expect("serialized transaction contains scriptSig");
    mutated[offset + 10] ^= 1;
    let changed = Transaction::read(mutated.as_slice(), BranchId::Nu6_2)
        .expect("authorization mutation remains decodable");
    assert_eq!(
        changed.txid(),
        transaction.txid(),
        "V5 txid excludes authorization"
    );
    mutated
}

fn agreement(direction: SwapDirection) -> ZecAgreementV1 {
    agreement_for_profile(direction, ZecProfileId::DeterministicLocalV1)
}

fn agreement_for_profile(direction: SwapDirection, profile: ZecProfileId) -> ZecAgreementV1 {
    let maker_secret = SecretKey::from_slice(&[1; 32]).expect("maker key");
    let taker_secret = SecretKey::from_slice(&[2; 32]).expect("taker key");
    let secp = Secp256k1::new();
    let maker_key = PublicKey::from_secret_key(&secp, &maker_secret).serialize();
    let taker_key = PublicKey::from_secret_key(&secp, &taker_secret).serialize();
    let (refund_key, claimant_key) = match direction {
        SwapDirection::TakerSellsForeign => (taker_key, maker_key),
        SwapDirection::TakerSellsLez => (maker_key, taker_key),
    };
    let refund_hash = pubkey_hash(&refund_key);
    let claimant_hash = pubkey_hash(&claimant_key);
    let (network, environment, zcash_anchor, refund_lock, earlier_latest_ms, later_earliest) =
        match profile {
            ZecProfileId::DeterministicLocalV1 => (
                NetworkType::Regtest,
                LezEnvironmentV1::DeterministicLocalV0_2,
                116,
                120,
                160_000,
                200,
            ),
            ZecProfileId::PublicTestnetV1 => (
                NetworkType::Test,
                LezEnvironmentV1::PublicTestnetV0_2,
                100,
                292,
                7_300_000,
                14_500,
            ),
        };
    let contract = Bip199Contract::new(refund_lock, refund_hash, [9; 32], claimant_hash);
    let binding = ZecSwapBinding::new(
        profile,
        lez_zec_swap_sdk::ExpectedBip199Output::new(
            network,
            BranchId::Nu6_2,
            Zatoshis::from_u64(100_000_000).expect("principal"),
            contract,
        ),
    )
    .expect("profile binding");
    let id = format!("zebra-adapter-{direction:?}-{profile:?}");
    let escrow_program = [1; 8];
    let onchain_id = derive_lez_swap_id_v1(id.as_bytes());
    let body = ZecAgreementBodyV1::new(
        id,
        direction,
        ZecProfileRecordV1::from(profile),
        ZecParticipantsV1::new(
            ZecParticipantIdentityV1::new([3; 32], maker_key),
            ZecParticipantIdentityV1::new([4; 32], taker_key),
        ),
        [9; 32],
        ZecLezTermsV1::new(
            LezChainIdentityV1::new(environment, [8; 32], [7; 32]),
            escrow_program,
            LezAssetV1::Native {
                authenticated_transfer_program_id: [2; 8],
            },
            42,
            derive_lez_metadata_account_v1(&escrow_program, &onchain_id),
            derive_lez_native_custody_account_v1(&escrow_program, &onchain_id),
        ),
        ZecSwapBindingRecordV1::from_binding(&binding),
        ZecTransactionPolicyV1::new(
            [12; 32],
            ZcashTransparentDestinationV1::p2pkh(refund_hash),
            10_000,
            1_000,
            ZcashTransparentDestinationV1::p2pkh(claimant_hash),
            10_000,
            ZcashTransparentDestinationV1::p2pkh(refund_hash),
            10_000,
            40,
        ),
        ZecRefundPlanV1::new(100, zcash_anchor, earlier_latest_ms, later_earliest),
        NegotiationTranscriptV1::new([5; 32], [6; 32], 1_000),
    );
    let commitment = body.commitment();
    let record = ZecAgreementRecordV1::from_parts(
        ZEC_CONCRETE_AGREEMENT_SCHEMA_V2,
        body,
        commitment,
        secp.sign_ecdsa(&Message::from_digest(commitment), &maker_secret)
            .serialize_compact(),
        secp.sign_ecdsa(&Message::from_digest(commitment), &taker_secret)
            .serialize_compact(),
    );
    ZecAgreementV1::from_wire_at(
        &record.encode_wire().expect("bounded agreement"),
        UnixSeconds::new(10),
    )
    .expect("valid fixture agreement")
}

fn pubkey_hash(bytes: &[u8; 33]) -> [u8; 20] {
    match TransparentAddress::from_pubkey(&PublicKey::from_slice(bytes).expect("public key")) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public keys produce P2PKH"),
    }
}
