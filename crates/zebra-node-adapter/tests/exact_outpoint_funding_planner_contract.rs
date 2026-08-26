//! End-to-end contract for agreement-committed exact-outpoint funding planning.

#![forbid(unsafe_code)]

use std::{
    collections::VecDeque,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use lez_swap_core::{Participant, SwapDirection, UnixSeconds};
use lez_zebra_node_adapter::{
    ExactOutpointZcashFundingPlanner, ExactOutpointZcashFundingPlannerError, RoleKeyedZcashSigner,
    ZebraCanonicalBlock, ZebraChainIdentity, ZebraChainInfo, ZebraFundingSigner, ZebraRpc,
    ZebraRpcChain, ZebraTransactionState, ZebraUnspentOutput,
};
use lez_zec_swap_sdk::{
    Bip199Contract, FirstLockPlanV1, FirstLockStepV1, LezAssetV1, LezChainIdentityV1,
    LezEnvironmentV1, NegotiationTranscriptV1, TransparentFundingRequest, TransparentUtxo,
    ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashFundingInputSetV1, ZcashFundingInputV1,
    ZcashTransparentDestinationV1, ZecAgreementBodyV1, ZecAgreementRecordV1, ZecAgreementV1,
    ZecLezTermsV1, ZecParticipantIdentityV1, ZecParticipantsV1, ZecProfileId, ZecProfileRecordV1,
    ZecRefundPlanV1, ZecSwapBinding, ZecSwapBindingRecordV1, ZecTransactionPolicyV1,
    build_funding_transaction, derive_lez_metadata_account_v1,
    derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use zcash_primitives::{
    block::BlockHash,
    transaction::{Authorized, Transaction, TransactionData},
};
use zcash_protocol::{
    TxId,
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{OutPoint, TxIn, TxOut},
};

const TIP_HEIGHT: u32 = 200;
const TIP_HASH: BlockHash = BlockHash([0x44; 32]);
const PRINCIPAL: u64 = 100_000_000;
const INPUT_VALUES: [u64; 2] = [60_000_000, 50_020_000];

#[tokio::test]
async fn both_directions_and_reordered_candidates_build_identical_canonical_plans() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = fixture(direction, INPUT_VALUES);
        let expected = direct_plan(&fixture);
        let (planner, calls, rpc) = configured_planner(&fixture);
        let actual = planner
            .plan(&fixture.agreement, fixture.outpoints.clone())
            .await
            .expect("stable exact candidates build funding plan");
        assert_eq!(actual, expected);
        assert_eq!(calls.load(Ordering::Relaxed), 1, "sign exactly once");
        assert_eq!(
            rpc.calls(),
            [
                "chain_info",
                "block_hash",
                "unspent",
                "unspent",
                "chain_info"
            ]
        );

        let (reordered, reordered_calls, _) = configured_planner(&fixture);
        let mut reversed = fixture.outpoints.clone();
        reversed.reverse();
        assert_eq!(
            reordered
                .plan(&fixture.agreement, reversed)
                .await
                .expect("canonical input set is order independent"),
            expected
        );
        assert_eq!(reordered_calls.load(Ordering::Relaxed), 1);
    }
}

#[tokio::test]
async fn finite_count_role_key_and_config_identity_fail_before_rpc_or_signing() {
    let fixture = fixture(SwapDirection::TakerSellsForeign, INPUT_VALUES);

    let (valid, calls, rpc) = configured_planner(&fixture);
    assert!(matches!(
        valid.plan(&fixture.agreement, vec![]).await,
        Err(ExactOutpointZcashFundingPlannerError::InvalidCandidateCount)
    ));
    assert!(matches!(
        valid
            .plan(&fixture.agreement, vec![fixture.outpoints[0].clone(); 65])
            .await,
        Err(ExactOutpointZcashFundingPlannerError::InvalidCandidateCount)
    ));
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(rpc.calls().is_empty());

    let (rpc, identity) = FakeRpc::stable(&fixture);
    let wrong_role = ExactOutpointZcashFundingPlanner::new(
        rpc.clone(),
        identity,
        fixture.role.other(),
        RoleKeyedZcashSigner::new(fixture.role, fixture.secret()),
    );
    assert!(matches!(
        wrong_role
            .plan(&fixture.agreement, fixture.outpoints.clone())
            .await,
        Err(ExactOutpointZcashFundingPlannerError::WrongRole)
    ));
    assert!(rpc.calls().is_empty());

    let role_mismatch = ExactOutpointZcashFundingPlanner::new(
        rpc.clone(),
        identity,
        fixture.role,
        RoleKeyedZcashSigner::new(fixture.role.other(), fixture.secret()),
    );
    assert!(matches!(
        role_mismatch
            .plan(&fixture.agreement, fixture.outpoints.clone())
            .await,
        Err(ExactOutpointZcashFundingPlannerError::SignerRoleMismatch)
    ));
    let foreign_key = ExactOutpointZcashFundingPlanner::new(
        rpc.clone(),
        identity,
        fixture.role,
        RoleKeyedZcashSigner::new(fixture.role, key(9)),
    );
    assert!(matches!(
        foreign_key
            .plan(&fixture.agreement, fixture.outpoints.clone())
            .await,
        Err(ExactOutpointZcashFundingPlannerError::SignerKeyMismatch)
    ));

    let wrong_network = ZebraChainIdentity::new(
        NetworkType::Test,
        ZebraRpcChain::Test,
        BranchId::Nu6_2,
        BlockHash([0x55; 32]),
    )
    .expect("valid test identity");
    let planner = ExactOutpointZcashFundingPlanner::new(
        rpc.clone(),
        wrong_network,
        fixture.role,
        RoleKeyedZcashSigner::new(fixture.role, fixture.secret()),
    );
    assert!(matches!(
        planner
            .plan(&fixture.agreement, fixture.outpoints.clone())
            .await,
        Err(ExactOutpointZcashFundingPlannerError::ConfiguredNetworkMismatch)
    ));
    let wrong_branch = ZebraChainIdentity::new(
        NetworkType::Regtest,
        ZebraRpcChain::Test,
        BranchId::Nu6,
        identity.genesis_hash(),
    )
    .expect("valid alternate branch identity");
    let planner = ExactOutpointZcashFundingPlanner::new(
        rpc.clone(),
        wrong_branch,
        fixture.role,
        RoleKeyedZcashSigner::new(fixture.role, fixture.secret()),
    );
    assert!(matches!(
        planner
            .plan(&fixture.agreement, fixture.outpoints.clone())
            .await,
        Err(ExactOutpointZcashFundingPlannerError::ConfiguredConsensusBranchMismatch)
    ));
    assert!(rpc.calls().is_empty());
}

#[tokio::test]
async fn genesis_missing_unconfirmed_utxo_drift_and_moving_tip_never_sign() {
    let fixture = fixture(SwapDirection::TakerSellsLez, INPUT_VALUES);

    for mutation in [
        Mutation::Genesis,
        Mutation::Missing,
        Mutation::Unconfirmed,
        Mutation::UtxoTip,
        Mutation::MovingTip,
        Mutation::RpcChain,
        Mutation::RpcBranch,
    ] {
        let (rpc, identity) = FakeRpc::stable(&fixture);
        rpc.mutate(mutation, &fixture);
        let calls = Arc::new(AtomicUsize::new(0));
        let signer = CountingSigner::new(fixture.role, fixture.secret(), calls.clone());
        let planner = ExactOutpointZcashFundingPlanner::new(rpc, identity, fixture.role, signer);
        let error = planner
            .plan(&fixture.agreement, fixture.outpoints.clone())
            .await
            .expect_err("unstable or noncanonical RPC facts fail closed");
        assert_eq!(calls.load(Ordering::Relaxed), 0, "{mutation:?}");
        assert!(
            matches!(
                (mutation, &error),
                (
                    Mutation::Genesis,
                    ExactOutpointZcashFundingPlannerError::GenesisMismatch
                ) | (
                    Mutation::Missing,
                    ExactOutpointZcashFundingPlannerError::CandidateUnavailable
                ) | (
                    Mutation::Unconfirmed,
                    ExactOutpointZcashFundingPlannerError::CandidateUnconfirmed
                ) | (
                    Mutation::UtxoTip,
                    ExactOutpointZcashFundingPlannerError::UtxoTipMismatch
                ) | (
                    Mutation::MovingTip,
                    ExactOutpointZcashFundingPlannerError::UnstableTip
                ) | (
                    Mutation::RpcChain,
                    ExactOutpointZcashFundingPlannerError::RpcChainMismatch
                ) | (
                    Mutation::RpcBranch,
                    ExactOutpointZcashFundingPlannerError::RpcConsensusBranchMismatch
                )
            ),
            "unexpected error for {mutation:?}: {error:?}"
        );
    }
}

#[tokio::test]
async fn duplicate_substituted_and_mutated_candidates_fail_signed_commitment() {
    let fixture = fixture(SwapDirection::TakerSellsForeign, INPUT_VALUES);

    let (rpc, identity) = FakeRpc::stable(&fixture);
    let calls = Arc::new(AtomicUsize::new(0));
    let planner = ExactOutpointZcashFundingPlanner::new(
        rpc,
        identity,
        fixture.role,
        CountingSigner::new(fixture.role, fixture.secret(), calls.clone()),
    );
    assert!(matches!(
        planner
            .plan(
                &fixture.agreement,
                vec![fixture.outpoints[0].clone(), fixture.outpoints[0].clone()],
            )
            .await,
        Err(ExactOutpointZcashFundingPlannerError::Agreement(_))
    ));
    assert_eq!(calls.load(Ordering::Relaxed), 0);

    let alternative = OutPoint::new([0x99; 32], 7);
    let (rpc, identity) = FakeRpc::stable(&fixture);
    rpc.edit(|state| {
        state.outputs.push((
            alternative.clone(),
            ZebraUnspentOutput::new(TIP_HASH, 3, fixture.outputs[1].clone()),
        ));
    });
    let planner = ExactOutpointZcashFundingPlanner::new(
        rpc,
        identity,
        fixture.role,
        RoleKeyedZcashSigner::new(fixture.role, fixture.secret()),
    );
    assert!(matches!(
        planner
            .plan(
                &fixture.agreement,
                vec![fixture.outpoints[0].clone(), alternative],
            )
            .await,
        Err(ExactOutpointZcashFundingPlannerError::Agreement(_))
    ));

    let (rpc, identity) = FakeRpc::stable(&fixture);
    rpc.edit(|state| {
        state.outputs[0].1 = ZebraUnspentOutput::new(
            TIP_HASH,
            3,
            TxOut::new(
                Zatoshis::from_u64(INPUT_VALUES[0] + 1).expect("mutated value"),
                fixture.outputs[0].script_pubkey().clone(),
            ),
        );
    });
    let planner = ExactOutpointZcashFundingPlanner::new(
        rpc,
        identity,
        fixture.role,
        RoleKeyedZcashSigner::new(fixture.role, fixture.secret()),
    );
    assert!(matches!(
        planner
            .plan(&fixture.agreement, fixture.outpoints.clone())
            .await,
        Err(ExactOutpointZcashFundingPlannerError::Agreement(_))
    ));
}

#[tokio::test]
async fn signer_build_failure_and_noncanonical_bytes_are_typed_and_redacted() {
    let insufficient = fixture(SwapDirection::TakerSellsForeign, [20_000, 20_000]);
    let (rpc, identity) = FakeRpc::stable(&insufficient);
    let planner = ExactOutpointZcashFundingPlanner::new(
        rpc,
        identity,
        insufficient.role,
        RoleKeyedZcashSigner::new(insufficient.role, insufficient.secret()),
    );
    assert!(matches!(
        planner
            .plan(&insufficient.agreement, insufficient.outpoints.clone())
            .await,
        Err(ExactOutpointZcashFundingPlannerError::Signer(_))
    ));

    let fixture = fixture(SwapDirection::TakerSellsLez, INPUT_VALUES);
    let (rpc, identity) = FakeRpc::stable(&fixture);
    let marker = "private-signer-marker-001122";
    let signer = FailingSigner {
        role: fixture.role,
        public_key: public_key(&fixture.secret()),
        marker,
    };
    let planner = ExactOutpointZcashFundingPlanner::new(rpc, identity, fixture.role, signer);
    let error = planner
        .plan(&fixture.agreement, fixture.outpoints.clone())
        .await
        .expect_err("signer failure is typed");
    assert!(matches!(
        &error,
        ExactOutpointZcashFundingPlannerError::Signer(_)
    ));
    for diagnostic in [
        format!("{planner:?}"),
        format!("{error:?}"),
        error.to_string(),
    ] {
        assert!(!diagnostic.contains(marker));
        assert!(!diagnostic.contains(&hex::encode(fixture.secret_bytes)));
    }

    for exact in [vec![0xff], {
        let mut trailing = direct_bytes(&fixture);
        trailing.push(0);
        trailing
    }] {
        let (rpc, identity) = FakeRpc::stable(&fixture);
        let planner = ExactOutpointZcashFundingPlanner::new(
            rpc,
            identity,
            fixture.role,
            StaticSigner::new(fixture.role, public_key(&fixture.secret()), exact),
        );
        assert!(matches!(
            planner
                .plan(&fixture.agreement, fixture.outpoints.clone())
                .await,
            Err(
                ExactOutpointZcashFundingPlannerError::MalformedSignedTransaction(_)
                    | ExactOutpointZcashFundingPlannerError::TrailingSignedTransaction
            )
        ));
    }
}

#[tokio::test]
async fn canonical_signer_mutations_of_input_change_and_expiry_are_rejected() {
    let fixture = fixture(SwapDirection::TakerSellsForeign, INPUT_VALUES);
    let canonical = direct_bytes(&fixture);
    for mutated in [
        mutate_foreign_input(&canonical),
        mutate_change(&canonical),
        mutate_expiry(&canonical),
    ] {
        let (rpc, identity) = FakeRpc::stable(&fixture);
        let planner = ExactOutpointZcashFundingPlanner::new(
            rpc,
            identity,
            fixture.role,
            StaticSigner::new(fixture.role, public_key(&fixture.secret()), mutated),
        );
        assert!(matches!(
            planner
                .plan(&fixture.agreement, fixture.outpoints.clone())
                .await,
            Err(ExactOutpointZcashFundingPlannerError::SignedTransactionPolicyMismatch)
        ));
    }
}

#[derive(Clone, Copy, Debug)]
enum Mutation {
    Genesis,
    Missing,
    Unconfirmed,
    UtxoTip,
    MovingTip,
    RpcChain,
    RpcBranch,
}

#[derive(Clone)]
struct FakeRpc {
    state: Arc<Mutex<FakeState>>,
}

struct FakeState {
    infos: VecDeque<ZebraChainInfo>,
    genesis: BlockHash,
    outputs: Vec<(OutPoint, ZebraUnspentOutput)>,
    calls: Vec<&'static str>,
}

#[derive(Debug, thiserror::Error)]
#[error("private-rpc-marker")]
struct FakeRpcError;

impl FakeRpc {
    fn stable(fixture: &Fixture) -> (Self, ZebraChainIdentity) {
        let identity = ZebraChainIdentity::deterministic_regtest_nu6_2();
        let info = chain_info(identity, TIP_HASH, TIP_HEIGHT);
        let outputs = fixture
            .outpoints
            .iter()
            .cloned()
            .zip(fixture.outputs.iter().cloned())
            .map(|(outpoint, output)| (outpoint, ZebraUnspentOutput::new(TIP_HASH, 3, output)))
            .collect();
        (
            Self {
                state: Arc::new(Mutex::new(FakeState {
                    infos: VecDeque::from([info, info]),
                    genesis: identity.genesis_hash(),
                    outputs,
                    calls: Vec::new(),
                })),
            },
            identity,
        )
    }

    fn edit(&self, operation: impl FnOnce(&mut FakeState)) {
        operation(&mut self.state.lock().expect("fake RPC lock"));
    }

    fn calls(&self) -> Vec<&'static str> {
        self.state.lock().expect("fake RPC lock").calls.clone()
    }

    fn mutate(&self, mutation: Mutation, fixture: &Fixture) {
        self.edit(|state| match mutation {
            Mutation::Genesis => state.genesis = BlockHash([0xee; 32]),
            Mutation::Missing => {
                state.outputs.pop();
            }
            Mutation::Unconfirmed => {
                state.outputs[0].1 =
                    ZebraUnspentOutput::new(TIP_HASH, 0, fixture.outputs[0].clone());
            }
            Mutation::UtxoTip => {
                state.outputs[0].1 =
                    ZebraUnspentOutput::new(BlockHash([0xdd; 32]), 3, fixture.outputs[0].clone());
            }
            Mutation::MovingTip => {
                let identity = ZebraChainIdentity::deterministic_regtest_nu6_2();
                state.infos = VecDeque::from([
                    chain_info(identity, TIP_HASH, TIP_HEIGHT),
                    chain_info(identity, BlockHash([0xaa; 32]), TIP_HEIGHT + 1),
                ]);
            }
            Mutation::RpcChain => {
                let wrong = ZebraChainInfo::new(
                    ZebraRpcChain::Main,
                    BlockHeight::from_u32(TIP_HEIGHT),
                    TIP_HASH,
                    BranchId::Nu6_2,
                );
                state.infos = VecDeque::from([wrong, wrong]);
            }
            Mutation::RpcBranch => {
                let wrong = ZebraChainInfo::new(
                    ZebraRpcChain::Test,
                    BlockHeight::from_u32(TIP_HEIGHT),
                    TIP_HASH,
                    BranchId::Nu6,
                );
                state.infos = VecDeque::from([wrong, wrong]);
            }
        });
    }
}

#[async_trait]
impl ZebraRpc for FakeRpc {
    type Error = FakeRpcError;

    async fn chain_info(&self) -> Result<ZebraChainInfo, Self::Error> {
        let mut state = self.state.lock().expect("fake RPC lock");
        state.calls.push("chain_info");
        state.infos.pop_front().ok_or(FakeRpcError)
    }

    async fn block_hash(&self, height: BlockHeight) -> Result<BlockHash, Self::Error> {
        assert_eq!(height, BlockHeight::from_u32(0));
        let mut state = self.state.lock().expect("fake RPC lock");
        state.calls.push("block_hash");
        Ok(state.genesis)
    }

    async fn canonical_block(
        &self,
        _block_hash: BlockHash,
    ) -> Result<ZebraCanonicalBlock, Self::Error> {
        Err(FakeRpcError)
    }

    async fn block_transaction(
        &self,
        _transaction_id: TxId,
        _block_hash: BlockHash,
    ) -> Result<Option<Vec<u8>>, Self::Error> {
        Err(FakeRpcError)
    }

    async fn mempool_transaction_ids(&self) -> Result<Vec<TxId>, Self::Error> {
        Err(FakeRpcError)
    }

    async fn raw_transaction(&self, _transaction_id: TxId) -> Result<Option<Vec<u8>>, Self::Error> {
        Err(FakeRpcError)
    }

    async fn transaction_state(
        &self,
        _transaction_id: TxId,
    ) -> Result<Option<ZebraTransactionState>, Self::Error> {
        Err(FakeRpcError)
    }

    async fn unspent_output(
        &self,
        outpoint: &OutPoint,
    ) -> Result<Option<ZebraUnspentOutput>, Self::Error> {
        let mut state = self.state.lock().expect("fake RPC lock");
        state.calls.push("unspent");
        Ok(state
            .outputs
            .iter()
            .find(|(candidate, _)| candidate == outpoint)
            .map(|(_, output)| output.clone()))
    }

    async fn send_raw_transaction(&self, _transaction: &[u8]) -> Result<TxId, Self::Error> {
        Err(FakeRpcError)
    }
}

#[derive(Clone)]
struct CountingSigner {
    inner: RoleKeyedZcashSigner,
    calls: Arc<AtomicUsize>,
}

impl CountingSigner {
    fn new(role: Participant, secret: SecretKey, calls: Arc<AtomicUsize>) -> Self {
        Self {
            inner: RoleKeyedZcashSigner::new(role, secret),
            calls,
        }
    }
}

#[async_trait]
impl ZebraFundingSigner for CountingSigner {
    type Error = lez_zebra_node_adapter::RoleKeyedZcashSignerError;

    fn participant(&self) -> Participant {
        self.inner.participant()
    }

    fn public_key(&self) -> PublicKey {
        self.inner.public_key()
    }

    async fn sign_funding(
        &self,
        contract: &Bip199Contract,
        request: &TransparentFundingRequest,
    ) -> Result<Vec<u8>, Self::Error> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner.sign_funding(contract, request).await
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct PrivateSignerError(&'static str);

struct FailingSigner {
    role: Participant,
    public_key: PublicKey,
    marker: &'static str,
}

#[async_trait]
impl ZebraFundingSigner for FailingSigner {
    type Error = PrivateSignerError;

    fn participant(&self) -> Participant {
        self.role
    }

    fn public_key(&self) -> PublicKey {
        self.public_key
    }

    async fn sign_funding(
        &self,
        _contract: &Bip199Contract,
        _request: &TransparentFundingRequest,
    ) -> Result<Vec<u8>, Self::Error> {
        Err(PrivateSignerError(self.marker))
    }
}

struct StaticSigner {
    role: Participant,
    public_key: PublicKey,
    exact: Vec<u8>,
}

impl StaticSigner {
    fn new(role: Participant, public_key: PublicKey, exact: Vec<u8>) -> Self {
        Self {
            role,
            public_key,
            exact,
        }
    }
}

#[async_trait]
impl ZebraFundingSigner for StaticSigner {
    type Error = PrivateSignerError;

    fn participant(&self) -> Participant {
        self.role
    }

    fn public_key(&self) -> PublicKey {
        self.public_key
    }

    async fn sign_funding(
        &self,
        _contract: &Bip199Contract,
        _request: &TransparentFundingRequest,
    ) -> Result<Vec<u8>, Self::Error> {
        Ok(self.exact.clone())
    }
}

struct Fixture {
    agreement: ZecAgreementV1,
    role: Participant,
    secret_bytes: [u8; 32],
    outpoints: Vec<OutPoint>,
    outputs: Vec<TxOut>,
}

impl Fixture {
    fn secret(&self) -> SecretKey {
        SecretKey::from_slice(&self.secret_bytes).expect("fixture funding key")
    }

    fn utxos(&self) -> Vec<TransparentUtxo> {
        self.outpoints
            .iter()
            .cloned()
            .zip(self.outputs.iter().cloned())
            .map(|(outpoint, output)| TransparentUtxo::new(outpoint, output))
            .collect()
    }
}

fn configured_planner(
    fixture: &Fixture,
) -> (
    ExactOutpointZcashFundingPlanner<FakeRpc, CountingSigner>,
    Arc<AtomicUsize>,
    FakeRpc,
) {
    let (rpc, identity) = FakeRpc::stable(fixture);
    let calls = Arc::new(AtomicUsize::new(0));
    let signer = CountingSigner::new(fixture.role, fixture.secret(), calls.clone());
    (
        ExactOutpointZcashFundingPlanner::new(rpc.clone(), identity, fixture.role, signer),
        calls,
        rpc,
    )
}

fn direct_plan(fixture: &Fixture) -> FirstLockPlanV1 {
    let bytes = direct_bytes(fixture);
    let transaction = Transaction::read(
        bytes.as_slice(),
        fixture
            .agreement
            .binding()
            .expected_output()
            .consensus_branch_id(),
    )
    .expect("direct canonical transaction");
    let prepared = lez_zec_swap_sdk::PreparedFirstLockSubmissionV1::new(
        FirstLockStepV1::ZcashFund,
        *transaction.txid().as_ref(),
        bytes,
    )
    .expect("direct prepared funding");
    FirstLockPlanV1::zcash(prepared).expect("direct Zcash plan")
}

fn direct_bytes(fixture: &Fixture) -> Vec<u8> {
    let request = fixture
        .agreement
        .funding_request(fixture.utxos(), BlockHeight::from_u32(TIP_HEIGHT))
        .expect("agreement funding request");
    let transaction = build_funding_transaction(
        fixture.agreement.binding().expected_output().contract(),
        &request,
        &fixture.secret(),
    )
    .expect("direct canonical funding build");
    serialize(&transaction)
}

fn mutate_foreign_input(bytes: &[u8]) -> Vec<u8> {
    mutate_transaction(
        bytes,
        |transparent| {
            let script_sig = transparent.vin[0].script_sig().clone();
            let sequence = transparent.vin[0].sequence();
            transparent.vin[0] =
                TxIn::from_parts(OutPoint::new([0xee; 32], 9), script_sig, sequence);
        },
        None,
    )
}

fn mutate_change(bytes: &[u8]) -> Vec<u8> {
    mutate_transaction(
        bytes,
        |transparent| {
            let script_pubkey = transparent.vout[1].script_pubkey().clone();
            transparent.vout[1] = TxOut::new(
                Zatoshis::from_u64(1).expect("mutated change"),
                script_pubkey,
            );
        },
        None,
    )
}

fn mutate_expiry(bytes: &[u8]) -> Vec<u8> {
    mutate_transaction(bytes, |_| {}, Some(BlockHeight::from_u32(TIP_HEIGHT + 99)))
}

fn mutate_transaction(
    bytes: &[u8],
    mutation: impl FnOnce(&mut zcash_transparent::bundle::Bundle<zcash_transparent::bundle::Authorized>),
    expiry: Option<BlockHeight>,
) -> Vec<u8> {
    let transaction = Transaction::read(bytes, BranchId::Nu6_2).expect("canonical transaction");
    let data = transaction.into_data();
    let mut transparent = data
        .transparent_bundle()
        .cloned()
        .expect("transparent funding bundle");
    mutation(&mut transparent);
    let transaction = TransactionData::<Authorized>::from_parts(
        data.version(),
        data.consensus_branch_id(),
        data.lock_time(),
        expiry.unwrap_or(data.expiry_height()),
        Some(transparent),
        None,
        None,
        None,
    )
    .freeze()
    .expect("mutated transaction freezes");
    serialize(&transaction)
}

fn serialize(transaction: &Transaction) -> Vec<u8> {
    let mut bytes = Vec::new();
    transaction
        .write(&mut bytes)
        .expect("serialize canonical transaction");
    bytes
}

fn chain_info(identity: ZebraChainIdentity, tip_hash: BlockHash, height: u32) -> ZebraChainInfo {
    ZebraChainInfo::new(
        identity.rpc_chain(),
        BlockHeight::from_u32(height),
        tip_hash,
        identity.consensus_branch_id(),
    )
}

#[allow(clippy::too_many_lines)]
fn fixture(direction: SwapDirection, values: [u64; 2]) -> Fixture {
    let maker_secret = key(1);
    let taker_secret = key(2);
    let secp = Secp256k1::new();
    let maker_key = PublicKey::from_secret_key(&secp, &maker_secret);
    let taker_key = PublicKey::from_secret_key(&secp, &taker_secret);
    let role = match direction {
        SwapDirection::TakerSellsForeign => Participant::Taker,
        SwapDirection::TakerSellsLez => Participant::Maker,
    };
    let secret_bytes = match role {
        Participant::Maker => maker_secret.secret_bytes(),
        Participant::Taker => taker_secret.secret_bytes(),
    };
    let funding_key = match role {
        Participant::Maker => maker_key,
        Participant::Taker => taker_key,
    };
    let owner_script: Script = TransparentAddress::from_pubkey(&funding_key)
        .script()
        .into();
    let outpoints = vec![OutPoint::new([0x11; 32], 0), OutPoint::new([0x22; 32], 1)];
    let outputs: Vec<_> = values
        .into_iter()
        .map(|value| {
            TxOut::new(
                Zatoshis::from_u64(value).expect("fixture value"),
                owner_script.clone(),
            )
        })
        .collect();
    let input_set = ZcashFundingInputSetV1::new(
        outpoints
            .iter()
            .zip(&outputs)
            .map(|(outpoint, output)| {
                ZcashFundingInputV1::new(
                    *outpoint.hash(),
                    outpoint.n(),
                    u64::from(output.value()),
                    output.script_pubkey().0.0.clone(),
                )
            })
            .collect(),
    )
    .expect("canonical input set");
    let (refund_key, claimant_key) = match direction {
        SwapDirection::TakerSellsForeign => (taker_key, maker_key),
        SwapDirection::TakerSellsLez => (maker_key, taker_key),
    };
    let refund_hash = public_key_hash(&refund_key);
    let claimant_hash = public_key_hash(&claimant_key);
    let binding = ZecSwapBinding::new(
        ZecProfileId::DeterministicLocalV1,
        lez_zec_swap_sdk::ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            Zatoshis::from_u64(PRINCIPAL).expect("principal"),
            Bip199Contract::new(120, refund_hash, [9; 32], claimant_hash),
        ),
    )
    .expect("binding");
    let id = format!("exact-outpoint-{direction:?}-{}", values[0]);
    let escrow_program = [1; 8];
    let onchain_id = derive_lez_swap_id_v1(id.as_bytes());
    let body = ZecAgreementBodyV1::new(
        id,
        direction,
        ZecProfileRecordV1::from(ZecProfileId::DeterministicLocalV1),
        ZecParticipantsV1::new(
            ZecParticipantIdentityV1::new([3; 32], maker_key.serialize()),
            ZecParticipantIdentityV1::new([4; 32], taker_key.serialize()),
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
            input_set.commitment(),
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
    let agreement = ZecAgreementV1::from_wire_at(
        &record.encode_wire().expect("bounded agreement"),
        UnixSeconds::new(10),
    )
    .expect("valid signed agreement");
    assert_eq!(agreement.lez_claimant(), role);
    Fixture {
        agreement,
        role,
        secret_bytes,
        outpoints,
        outputs,
    }
}

fn key(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).expect("valid fixture key")
}

fn public_key(secret: &SecretKey) -> PublicKey {
    PublicKey::from_secret_key(&Secp256k1::new(), secret)
}

fn public_key_hash(key: &PublicKey) -> [u8; 20] {
    match TransparentAddress::from_pubkey(key) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public key produces P2PKH"),
    }
}

impl fmt::Debug for FailingSigner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FailingSigner([REDACTED])")
    }
}
