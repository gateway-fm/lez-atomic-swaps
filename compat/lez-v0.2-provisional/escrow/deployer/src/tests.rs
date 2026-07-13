use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use common::{HashType, block::Block, transaction::LeeTransaction};
use jsonrpsee::{core::async_trait, server::ServerBuilder, types::ErrorObjectOwned};
use nssa::{Account, AccountId};
use sequencer_service_protocol::{
    BlockId, ChannelId, Commitment, MembershipProof, Nonce, ProgramId,
};
use sequencer_service_rpc::{RpcServer, SequencerClientBuilder};

use super::*;

#[derive(Clone, Copy, Debug)]
enum SubmitMode {
    Include,
    Pending,
    WrongHash,
    Ambiguous,
}

#[derive(Debug)]
struct MockState {
    rpc_calls: usize,
    submissions: Vec<LeeTransaction>,
    blocks: Vec<Block>,
    submit_mode: SubmitMode,
}

#[derive(Clone, Debug)]
struct MockNode {
    state: Arc<Mutex<MockState>>,
}

impl MockNode {
    fn new(submit_mode: SubmitMode) -> Self {
        Self {
            state: Arc::new(Mutex::new(MockState {
                rpc_calls: 0,
                submissions: Vec::new(),
                blocks: Vec::new(),
                submit_mode,
            })),
        }
    }

    fn record_call(&self) {
        self.state.lock().unwrap().rpc_calls += 1;
    }

    fn rpc_calls(&self) -> usize {
        self.state.lock().unwrap().rpc_calls
    }

    fn submissions(&self) -> Vec<LeeTransaction> {
        self.state.lock().unwrap().submissions.clone()
    }

    fn blocks(&self) -> Vec<Block> {
        self.state.lock().unwrap().blocks.clone()
    }
}

#[async_trait]
impl RpcServer for MockNode {
    async fn send_transaction(
        &self,
        transaction: LeeTransaction,
    ) -> Result<HashType, ErrorObjectOwned> {
        self.record_call();
        let hash = transaction.hash();
        let mut state = self.state.lock().unwrap();
        state.submissions.push(transaction.clone());
        match state.submit_mode {
            SubmitMode::Include => {
                state.blocks.push(common::test_utils::produce_dummy_block(
                    42,
                    None,
                    vec![transaction],
                ));
                Ok(hash)
            }
            SubmitMode::Pending => Ok(hash),
            SubmitMode::WrongHash => Ok(HashType([0x99; 32])),
            SubmitMode::Ambiguous => Err(ErrorObjectOwned::owned(
                -32603,
                "transport outcome unknown after accepting request bytes",
                None::<()>,
            )),
        }
    }

    async fn check_health(&self) -> Result<(), ErrorObjectOwned> {
        self.record_call();
        Ok(())
    }

    async fn get_block(&self, block_id: BlockId) -> Result<Option<Block>, ErrorObjectOwned> {
        self.record_call();
        Ok(self
            .blocks()
            .into_iter()
            .find(|block| block.header.block_id == block_id))
    }

    async fn get_block_range(
        &self,
        start_block_id: BlockId,
        end_block_id: BlockId,
    ) -> Result<Vec<Block>, ErrorObjectOwned> {
        self.record_call();
        Ok(self
            .blocks()
            .into_iter()
            .filter(|block| (start_block_id..=end_block_id).contains(&block.header.block_id))
            .collect())
    }

    async fn get_last_block_id(&self) -> Result<BlockId, ErrorObjectOwned> {
        self.record_call();
        Ok(self
            .blocks()
            .last()
            .map_or(41, |block| block.header.block_id))
    }

    async fn get_account_balance(&self, _account_id: AccountId) -> Result<u128, ErrorObjectOwned> {
        self.record_call();
        Ok(0)
    }

    async fn get_transaction(
        &self,
        _transaction_hash: HashType,
    ) -> Result<Option<LeeTransaction>, ErrorObjectOwned> {
        self.record_call();
        Ok(None)
    }

    async fn get_accounts_nonces(
        &self,
        _account_ids: Vec<AccountId>,
    ) -> Result<Vec<Nonce>, ErrorObjectOwned> {
        self.record_call();
        Ok(Vec::new())
    }

    async fn get_proof_for_commitment(
        &self,
        _commitment: Commitment,
    ) -> Result<Option<MembershipProof>, ErrorObjectOwned> {
        self.record_call();
        Ok(None)
    }

    async fn get_account(&self, _account_id: AccountId) -> Result<Account, ErrorObjectOwned> {
        self.record_call();
        Ok(Account::default())
    }

    async fn get_program_ids(&self) -> Result<BTreeMap<String, ProgramId>, ErrorObjectOwned> {
        self.record_call();
        Ok(BTreeMap::from([
            (
                "authenticated_transfer".to_owned(),
                OFFICIAL_AUTHENTICATED_TRANSFER_PROGRAM_ID,
            ),
            ("token".to_owned(), OFFICIAL_TOKEN_PROGRAM_ID),
        ]))
    }

    async fn get_channel_id(&self) -> Result<ChannelId, ErrorObjectOwned> {
        self.record_call();
        Ok(ChannelId([1; 32]))
    }
}

async fn start_mock(
    submit_mode: SubmitMode,
) -> (MockNode, SequencerClient, jsonrpsee::server::ServerHandle) {
    let mock = MockNode::new(submit_mode);
    let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let handle = server.start(mock.clone().into_rpc());
    let client = SequencerClientBuilder::default()
        .max_request_size(16 * 1024 * 1024)
        .max_response_size(16 * 1024 * 1024)
        .request_timeout(Duration::from_secs(1))
        .max_concurrent_requests(1)
        .build(format!("http://{address}"))
        .unwrap();
    (mock, client, handle)
}

fn checked_manifest() -> DeploymentManifest {
    toml::from_str(MANIFEST).expect("checked manifest")
}

async fn checked_preflight(
    submit_mode: SubmitMode,
) -> (
    MockNode,
    SequencerClient,
    PreflightEvidence,
    jsonrpsee::server::ServerHandle,
) {
    let (mock, client, handle) = start_mock(submit_mode).await;
    let preflight = preflight_with_client(&checked_manifest(), &client)
        .await
        .expect("checked loopback preflight");
    (mock, client, preflight, handle)
}

#[tokio::test]
async fn any_local_preflight_identity_mutation_causes_zero_rpc_effects() {
    for mutation in 0..10 {
        let (mock, client, handle) = start_mock(SubmitMode::Pending).await;
        let mut manifest = checked_manifest();
        match mutation {
            0 => manifest.target.rpc_url.push_str("/mutated"),
            1 => manifest.target.channel_id.replace_range(0..2, "ff"),
            2 => manifest.target.authenticated_transfer_program_id[0] ^= 1,
            3 => manifest.target.token_program_id[0] ^= 1,
            4 => manifest.target.associated_token_account_program_id[0] ^= 1,
            5 => manifest
                .target
                .associated_token_account_identity_source
                .push_str("-mutated"),
            6 => manifest.artifact_status.push_str("-mutated"),
            7 => manifest.artifact.elf_sha256.replace_range(0..2, "ff"),
            8 => manifest.artifact.image_id.replace_range(0..2, "ff"),
            9 => manifest.artifact.program_id_words[0] ^= 1,
            _ => unreachable!(),
        }

        let error = preflight_with_client(&manifest, &client)
            .await
            .expect_err("mutated immutable target must fail locally");

        assert!(error.to_string().contains("immutable manifest"));
        assert_eq!(mock.rpc_calls(), 0, "local rejection must precede all RPC");
        handle.stop().unwrap();
    }
}

#[tokio::test]
async fn submits_exact_checked_program_deployment_once_and_binds_its_canonical_block() {
    let (mock, client, preflight, handle) = checked_preflight(SubmitMode::Include).await;

    let evidence =
        deploy_once_and_observe_with_timeout(client, preflight, Duration::from_millis(100))
            .await
            .expect("one exact deployment is canonically included");

    let submissions = mock.submissions();
    assert_eq!(submissions.len(), 1);
    let LeeTransaction::ProgramDeployment(deployment) = submissions[0].clone() else {
        panic!("the sole submission must be a ProgramDeployment");
    };
    assert_eq!(
        deployment.into_message().into_bytecode(),
        ZEC_ESCROW_V02_ELF
    );
    let block = mock.blocks().pop().expect("canonical inclusion block");
    assert_eq!(evidence.inclusion_block_id, block.header.block_id);
    assert_eq!(evidence.inclusion_block_hash, block.header.hash.to_string());
    assert_eq!(evidence.transaction_hash, submissions[0].hash().to_string());
    handle.stop().unwrap();
}

#[tokio::test]
async fn returned_hash_mismatch_fails_after_exactly_one_submission() {
    let (mock, client, preflight, handle) = checked_preflight(SubmitMode::WrongHash).await;

    let error = deploy_once_and_observe_with_timeout(client, preflight, Duration::from_millis(100))
        .await
        .expect_err("noncanonical returned hash must fail closed");

    assert!(
        error
            .to_string()
            .contains("different from the locally checked")
    );
    assert_eq!(mock.submissions().len(), 1);
    handle.stop().unwrap();
}

#[tokio::test]
async fn ambiguous_submission_result_never_resubmits() {
    let (mock, client, preflight, handle) = checked_preflight(SubmitMode::Ambiguous).await;

    let error = deploy_once_and_observe_with_timeout(client, preflight, Duration::from_millis(100))
        .await
        .expect_err("ambiguous submission must remain unknown");

    assert!(error.to_string().contains("outcome is unknown"));
    assert_eq!(mock.submissions().len(), 1);
    handle.stop().unwrap();
}

#[tokio::test]
async fn observation_timeout_never_resubmits() {
    let (mock, client, preflight, handle) = checked_preflight(SubmitMode::Pending).await;

    let error = deploy_once_and_observe_with_timeout(client, preflight, Duration::from_millis(25))
        .await
        .expect_err("unknown inclusion must time out without retry");

    assert!(error.to_string().contains("do not resubmit"));
    assert_eq!(mock.submissions().len(), 1);
    handle.stop().unwrap();
}

#[test]
fn checked_elf_image_id_and_manifest_program_words_are_one_identity() {
    let manifest = checked_manifest();
    validate_immutable_target(&manifest).expect("immutable official identities");
    let (elf_sha256, image_id) = checked_artifact().expect("checked artifact identity");
    assert_eq!(elf_sha256.len(), 64);
    assert_eq!(image_id.len(), 64);
    assert_ne!(elf_sha256, image_id);
    assert_ne!(ZEC_ESCROW_V02_ID, [0; 8]);
    assert_eq!(
        manifest.target.associated_token_account_program_id,
        OFFICIAL_ASSOCIATED_TOKEN_ACCOUNT_PROGRAM_ID
    );
    assert_eq!(
        manifest.target.associated_token_account_identity_source,
        OFFICIAL_ASSOCIATED_TOKEN_ACCOUNT_IDENTITY_SOURCE
    );
}
