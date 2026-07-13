use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use lez_swap_core::{SwapDirection, UnixSeconds};
use lez_zebra_node_adapter::{
    ZebraCanonicalBlock, ZebraChainIdentity, ZebraChainInfo, ZebraFirstLockError, ZebraRpc,
    ZebraRpcChain, ZebraRpcSwapPort, ZebraTransactionState, ZebraUnspentOutput,
};
use lez_zec_swap_sdk::{
    Bip199Contract, FirstLockConfirmedEvidenceV1, FirstLockObservation, FirstLockStepV1,
    LezAssetV1, LezChainIdentityV1, LezEnvironmentV1, NegotiationTranscriptV1,
    PreparedFirstLockSubmissionV1, TransparentFundingRequest, TransparentUtxo,
    ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashFirstLockPort, ZcashTransparentDestinationV1,
    ZecAgreementBodyV1, ZecAgreementRecordV1, ZecAgreementV1, ZecLezTermsV1,
    ZecParticipantIdentityV1, ZecParticipantsV1, ZecProfileId, ZecProfileRecordV1, ZecRefundPlanV1,
    ZecSwapBinding, ZecSwapBindingRecordV1, ZecTransactionPolicyV1, build_funding_transaction,
    derive_lez_metadata_account_v1, derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
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
        } else {
            Ok(state.canonical_inclusion_hash)
        }
    }

    async fn canonical_block(
        &self,
        _block_hash: BlockHash,
    ) -> Result<ZebraCanonicalBlock, Self::Error> {
        panic!("first-lock tests never scan blocks")
    }

    async fn block_transaction(
        &self,
        _transaction_id: TxId,
        _block_hash: BlockHash,
    ) -> Result<Option<Vec<u8>>, Self::Error> {
        panic!("first-lock tests never scan block transactions")
    }

    async fn mempool_transaction_ids(&self) -> Result<Vec<TxId>, Self::Error> {
        panic!("first-lock tests never enumerate the mempool")
    }

    async fn raw_transaction(&self, _transaction_id: TxId) -> Result<Option<Vec<u8>>, Self::Error> {
        let mut state = self.state.lock().expect("fake state lock");
        state.calls.push("raw_transaction".to_owned());
        Ok(state.raw_transaction.clone())
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
    let port = ZebraRpcSwapPort::new(rpc.clone(), identity);
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
        ZebraRpcSwapPort::new(absent.clone(), identity)
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
        ZebraRpcSwapPort::new(moving, identity)
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
        ZebraRpcSwapPort::new(mempool, identity)
            .observe_first_lock(&agreement, &prepared)
            .await
            .expect("mempool is visible but unconfirmed"),
        FirstLockObservation::Unstable
    );
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
        ZebraRpcSwapPort::new(found, identity)
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
        ZebraRpcSwapPort::new(mempool, identity)
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
        ZebraRpcSwapPort::new(rpc.clone(), identity)
            .observe_first_lock(&agreement, &wrong_step)
            .await,
        Err(ZebraFirstLockError::WrongStep(
            FirstLockStepV1::LezInitialize
        ))
    ));

    let reverse = self::agreement(SwapDirection::TakerSellsLez);
    let reverse_prepared = prepared(&reverse);
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), identity)
            .observe_first_lock(&reverse, &reverse_prepared)
            .await,
        Err(ZebraFirstLockError::WrongDirection(
            SwapDirection::TakerSellsLez
        ))
    ));

    let wrong_id = PreparedFirstLockSubmissionV1::new(
        FirstLockStepV1::ZcashFund,
        [0x99; 32],
        valid.exact_submission().to_vec(),
    )
    .expect("nonzero wrong identity");
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), identity)
            .observe_first_lock(&agreement, &wrong_id)
            .await,
        Err(ZebraFirstLockError::ExpectedTransactionIdMismatch)
    ));

    let malformed =
        PreparedFirstLockSubmissionV1::new(FirstLockStepV1::ZcashFund, [1; 32], vec![0xff])
            .expect("bounded malformed fixture");
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), identity)
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
        ZebraRpcSwapPort::new(rpc.clone(), identity)
            .observe_first_lock(&agreement, &trailing)
            .await,
        Err(ZebraFirstLockError::TrailingSubmissionBytes)
    ));

    let missing_output = prepared_from(&empty_transaction(TxVersion::V5));
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), identity)
            .observe_first_lock(&agreement, &missing_output)
            .await,
        Err(ZebraFirstLockError::MissingExpectedOutput)
    ));

    let wrong_version = prepared_from(&empty_transaction(TxVersion::V4));
    assert!(matches!(
        ZebraRpcSwapPort::new(rpc.clone(), identity)
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
        ZebraRpcSwapPort::new(rpc.clone(), identity)
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
        ZebraRpcSwapPort::new(rpc.clone(), identity)
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
        ZebraRpcSwapPort::new(rpc.clone(), wrong_network)
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
        ZebraRpcSwapPort::new(rpc.clone(), wrong_branch)
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
        ZebraRpcSwapPort::new(wrong_chain, identity)
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
        ZebraRpcSwapPort::new(wrong_branch, identity)
            .observe_first_lock(&agreement, &prepared)
            .await,
        Err(ZebraFirstLockError::RpcConsensusBranchMismatch)
    ));

    let (wrong_genesis, identity) = FakeRpc::confirmed(&agreement, &prepared);
    wrong_genesis.edit(|state| {
        state.genesis_hashes = VecDeque::from([BlockHash([0xee; 32])]);
    });
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_genesis, identity)
            .observe_first_lock(&agreement, &prepared)
            .await,
        Err(ZebraFirstLockError::GenesisMismatch)
    ));

    let (wrong_block, identity) = FakeRpc::confirmed(&agreement, &prepared);
    wrong_block.edit(|state| state.canonical_inclusion_hash = BlockHash([0xdd; 32]));
    assert!(matches!(
        ZebraRpcSwapPort::new(wrong_block, identity)
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
        ZebraRpcSwapPort::new(wrong_depth, identity)
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
        ZebraRpcSwapPort::new(inactive, identity)
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
        ZebraRpcSwapPort::new(wrong_raw, identity)
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
    ZebraRpcSwapPort::new(rpc.clone(), identity)
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
        ZebraRpcSwapPort::new(wrong_id, identity)
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
        ZebraRpcSwapPort::new(moving.clone(), identity)
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
        ZebraRpcSwapPort::new(changed_genesis.clone(), identity)
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
    assert_eq!(
        agreement.binding().expected_output().network(),
        NetworkType::Regtest
    );
    ZebraChainIdentity::deterministic_regtest_nu6_2()
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
    let key = SecretKey::from_slice(&[7; 32]).expect("fixed funding key");
    let public_key = PublicKey::from_secret_key(&Secp256k1::new(), &key);
    let owner_script: Script = TransparentAddress::from_pubkey(&public_key).script().into();
    let input_value = Zatoshis::from_u64(u64::from(value) + 20_000).expect("input value");
    let request = TransparentFundingRequest::new(
        vec![TransparentUtxo::new(
            OutPoint::new([0x77; 32], 0),
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
    let contract = Bip199Contract::new(120, refund_hash, [9; 32], claimant_hash);
    let binding = ZecSwapBinding::new(
        ZecProfileId::DeterministicLocalV1,
        lez_zec_swap_sdk::ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            Zatoshis::from_u64(100_000_000).expect("principal"),
            contract,
        ),
    )
    .expect("profile binding");
    let id = format!("zebra-adapter-{direction:?}");
    let escrow_program = [1; 8];
    let onchain_id = derive_lez_swap_id_v1(id.as_bytes());
    let body = ZecAgreementBodyV1::new(
        id,
        direction,
        ZecProfileRecordV1::from(ZecProfileId::DeterministicLocalV1),
        ZecParticipantsV1::new(
            ZecParticipantIdentityV1::new([3; 32], maker_key),
            ZecParticipantIdentityV1::new([4; 32], taker_key),
        ),
        [9; 32],
        ZecLezTermsV1::new(
            LezChainIdentityV1::new(LezEnvironmentV1::DeterministicLocalV0_2, [8; 32], [7; 32]),
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
        ZecRefundPlanV1::new(100, 116, 160_000, 200),
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
