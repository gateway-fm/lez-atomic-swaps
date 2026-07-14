#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::{
    collections::BTreeMap,
    fs,
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

const TEST_EVIDENCE_AUTHENTICATION_KEY: [u8; 32] = [0xa5; 32];

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
        let genesis = common::test_utils::produce_dummy_block(nssa::GENESIS_BLOCK_ID, None, vec![]);
        Self {
            state: Arc::new(Mutex::new(MockState {
                rpc_calls: 0,
                submissions: Vec::new(),
                blocks: vec![genesis],
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
                let previous = state.blocks.last().expect("mock genesis").header.clone();
                state.blocks.push(common::test_utils::produce_dummy_block(
                    previous.block_id + 1,
                    Some(previous.hash),
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

fn mock_trusted_expected_target() -> TrustedExpectedTarget {
    let program_id_words = [
        0x0102_0304,
        0x1112_1314,
        0x2122_2324,
        0x3132_3334,
        0x4142_4344,
        0x5152_5354,
        0x6162_6364,
        0x7172_7374,
    ];
    let mock_bytecode = b"independent native-safe mock deployment bytecode";
    let transaction = ProgramDeploymentTransaction::new(
        nssa::program_deployment_transaction::Message::new(mock_bytecode.to_vec()),
    );
    TrustedExpectedTarget {
        rpc_url: "https://native-safe.mock.invalid".to_owned(),
        channel_id: "22".repeat(32),
        elf_sha256: hex::encode(Sha256::digest(mock_bytecode)),
        image_id: program_id_hex(program_id_words),
        program_id_words,
        authenticated_transfer_program_id: [0x31; 8],
        token_program_id: [0x32; 8],
        associated_token_account_program_id: [0x33; 8],
        associated_token_account_identity_source: "independent-native-safe-fixture".to_owned(),
        deployment_transaction_hash: HashType(transaction.hash()).to_string(),
    }
}

fn mock_deployment_evidence(expected: &TrustedExpectedTarget) -> DeploymentEvidence {
    DeploymentEvidence {
        schema_version: DEPLOYMENT_EVIDENCE_SCHEMA_VERSION,
        preflight: PreflightEvidence {
            rpc_url: expected.rpc_url.clone(),
            channel_id: expected.channel_id.clone(),
            genesis_block_id: nssa::GENESIS_BLOCK_ID,
            genesis_block_hash: "33".repeat(32),
            elf_sha256: expected.elf_sha256.clone(),
            image_id: expected.image_id.clone(),
            program_id_words: expected.program_id_words,
            authenticated_transfer_program_id: expected.authenticated_transfer_program_id,
            token_program_id: expected.token_program_id,
            associated_token_account_program_id: expected.associated_token_account_program_id,
            associated_token_account_identity_source: expected
                .associated_token_account_identity_source
                .clone(),
            rpc_program_names: vec!["authenticated_transfer".to_owned(), "token".to_owned()],
            last_block_id: nssa::GENESIS_BLOCK_ID,
        },
        transaction_hash: expected.deployment_transaction_hash.clone(),
        inclusion_block_id: nssa::GENESIS_BLOCK_ID + 1,
        inclusion_block_hash: "44".repeat(32),
    }
}

fn write_authenticated_evidence(path: &Path, evidence: &DeploymentEvidence) {
    let authenticated =
        authenticate_deployment_evidence(evidence, &TEST_EVIDENCE_AUTHENTICATION_KEY)
            .expect("authenticate mock deployment evidence");
    fs::write(
        path,
        serde_json::to_vec_pretty(&authenticated).expect("serialize authenticated evidence"),
    )
    .unwrap();
}

fn private_tempdir() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    directory
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
    assert_eq!(evidence.preflight.channel_id, OFFICIAL_CHANNEL_ID);
    assert_eq!(evidence.preflight.genesis_block_id, nssa::GENESIS_BLOCK_ID);
    assert_eq!(
        evidence.preflight.genesis_block_hash,
        mock.blocks()
            .first()
            .expect("mock genesis block")
            .header
            .hash
            .to_string()
    );
    handle.stop().unwrap();
}

#[test]
fn verified_evidence_provisions_exact_runtime_identity_offline() {
    let expected = mock_trusted_expected_target();
    let evidence = mock_deployment_evidence(&expected);
    let directory = private_tempdir();
    let evidence_path = directory.path().join("deployment-evidence.json");
    let identity_path = directory.path().join("runtime-identity.json");
    write_authenticated_evidence(&evidence_path, &evidence);

    provision_runtime_identity_for_target(
        &expected,
        &evidence_path,
        &TEST_EVIDENCE_AUTHENTICATION_KEY,
        &identity_path,
    )
    .expect("verified identity output");

    let identity: ProvisionedRuntimeIdentity =
        serde_json::from_slice(&fs::read(&identity_path).unwrap()).unwrap();
    assert_eq!(identity.schema_version, PROVISIONED_IDENTITY_SCHEMA_VERSION);
    assert_eq!(identity.status, "deployed_and_observed");
    assert_eq!(identity.environment, "public_testnet_v0_2");
    assert_eq!(identity.compatibility, "lee_v0_2");
    assert_eq!(identity.rpc_url, expected.rpc_url);
    assert_eq!(identity.chain_id, expected.channel_id);
    assert_eq!(identity.channel_id, evidence.preflight.channel_id);
    assert_eq!(
        identity.genesis_block_hash,
        evidence.preflight.genesis_block_hash
    );
    assert_eq!(identity.escrow_program_id_words, expected.program_id_words);
    assert_eq!(identity.escrow_program_id_hex, expected.image_id);
    assert_eq!(
        identity.deployment_evidence_sha256,
        hex::encode(Sha256::digest(fs::read(&evidence_path).unwrap()))
    );
    assert_eq!(
        identity.deployment_transaction_hash,
        evidence.transaction_hash
    );
    assert_eq!(identity.inclusion_block_id, evidence.inclusion_block_id);
    assert_eq!(identity.inclusion_block_hash, evidence.inclusion_block_hash);
}

#[test]
fn offline_provisioning_never_clobbers_existing_identity() {
    let expected = mock_trusted_expected_target();
    let evidence = mock_deployment_evidence(&expected);
    let directory = private_tempdir();
    let evidence_path = directory.path().join("deployment-evidence.json");
    let identity_path = directory.path().join("runtime-identity.json");
    write_authenticated_evidence(&evidence_path, &evidence);
    fs::write(&identity_path, b"existing trusted runtime identity\n").unwrap();
    let before = fs::read(&identity_path).unwrap();

    let error = provision_runtime_identity_for_target(
        &expected,
        &evidence_path,
        &TEST_EVIDENCE_AUTHENTICATION_KEY,
        &identity_path,
    )
    .expect_err("existing identity output must never be clobbered");

    assert!(error.to_string().contains("already exists"));
    assert_eq!(fs::read(&identity_path).unwrap(), before);
}

#[test]
fn offline_provisioning_rejects_mutated_identity_without_creating_output() {
    let expected = mock_trusted_expected_target();
    let baseline = serde_json::to_value(mock_deployment_evidence(&expected)).unwrap();
    let mutations = [
        ("/schema_version", serde_json::json!(2)),
        ("/preflight/rpc_url", serde_json::json!("https://evil.test")),
        ("/preflight/channel_id", serde_json::json!("00".repeat(32))),
        (
            "/preflight/genesis_block_hash",
            serde_json::json!("00".repeat(32)),
        ),
        ("/preflight/program_id_words/0", serde_json::json!(0)),
        ("/transaction_hash", serde_json::json!("11".repeat(32))),
        ("/inclusion_block_id", serde_json::json!(1)),
        ("/inclusion_block_hash", serde_json::json!("00".repeat(32))),
    ];

    for (index, (pointer, replacement)) in mutations.into_iter().enumerate() {
        let directory = private_tempdir();
        let evidence_path = directory.path().join(format!("evidence-{index}.json"));
        let identity_path = directory.path().join(format!("identity-{index}.json"));
        let mut mutated = baseline.clone();
        *mutated.pointer_mut(pointer).expect("mutation pointer") = replacement;
        let mutated: DeploymentEvidence = serde_json::from_value(mutated).unwrap();
        write_authenticated_evidence(&evidence_path, &mutated);

        provision_runtime_identity_for_target(
            &expected,
            &evidence_path,
            &TEST_EVIDENCE_AUTHENTICATION_KEY,
            &identity_path,
        )
        .expect_err("mutated deployment identity must fail closed");
        assert!(!identity_path.exists());
    }
}

#[test]
fn offline_provisioning_rejects_chain_fact_tampering_without_authentication_key() {
    let expected = mock_trusted_expected_target();
    let evidence = mock_deployment_evidence(&expected);
    let directory = private_tempdir();
    let evidence_path = directory.path().join("deployment-evidence.json");
    let identity_path = directory.path().join("runtime-identity.json");
    write_authenticated_evidence(&evidence_path, &evidence);

    let wrong_key_error = provision_runtime_identity_for_target(
        &expected,
        &evidence_path,
        &[0x5a; EVIDENCE_AUTHENTICATION_KEY_BYTES],
        &identity_path,
    )
    .expect_err("a different owner key must not authenticate retained evidence");
    assert!(wrong_key_error.to_string().contains("authentication"));
    assert!(!identity_path.exists());

    let mut tampered: serde_json::Value =
        serde_json::from_slice(&fs::read(&evidence_path).unwrap()).unwrap();
    *tampered
        .pointer_mut("/evidence/inclusion_block_hash")
        .expect("authenticated evidence inclusion hash") = serde_json::json!("55".repeat(32));
    fs::write(&evidence_path, serde_json::to_vec(&tampered).unwrap()).unwrap();

    let error = provision_runtime_identity_for_target(
        &expected,
        &evidence_path,
        &TEST_EVIDENCE_AUTHENTICATION_KEY,
        &identity_path,
    )
    .expect_err("unauthenticated dynamic chain-fact mutation must fail closed");
    assert!(error.to_string().contains("authentication"));
    assert!(!identity_path.exists());

    for (pointer, replacement) in [
        ("/schema_version", serde_json::json!(2)),
        ("/algorithm", serde_json::json!("hmac-sha512-v1")),
        ("/authentication_tag", serde_json::json!("00".repeat(32))),
    ] {
        write_authenticated_evidence(&evidence_path, &evidence);
        let mut tampered: serde_json::Value =
            serde_json::from_slice(&fs::read(&evidence_path).unwrap()).unwrap();
        *tampered.pointer_mut(pointer).expect("envelope field") = replacement;
        fs::write(&evidence_path, serde_json::to_vec(&tampered).unwrap()).unwrap();
        assert!(
            provision_runtime_identity_for_target(
                &expected,
                &evidence_path,
                &TEST_EVIDENCE_AUTHENTICATION_KEY,
                &identity_path,
            )
            .is_err()
        );
        assert!(!identity_path.exists());
    }
}

#[test]
fn offline_provisioning_rejects_empty_oversized_and_nonregular_evidence() {
    let expected = mock_trusted_expected_target();
    let directory = private_tempdir();
    let identity_path = directory.path().join("runtime-identity.json");
    let empty = directory.path().join("empty.json");
    fs::write(&empty, []).unwrap();
    assert!(
        provision_runtime_identity_for_target(
            &expected,
            &empty,
            &TEST_EVIDENCE_AUTHENTICATION_KEY,
            &identity_path,
        )
        .is_err()
    );

    let oversized = directory.path().join("oversized.json");
    fs::write(&oversized, vec![b' '; MAX_DEPLOYMENT_EVIDENCE_BYTES + 1]).unwrap();
    assert!(
        provision_runtime_identity_for_target(
            &expected,
            &oversized,
            &TEST_EVIDENCE_AUTHENTICATION_KEY,
            &identity_path,
        )
        .is_err()
    );
    assert!(
        provision_runtime_identity_for_target(
            &expected,
            directory.path(),
            &TEST_EVIDENCE_AUTHENTICATION_KEY,
            &identity_path,
        )
        .is_err()
    );
    assert!(!identity_path.exists());
}

#[test]
fn evidence_authentication_key_must_be_exact_owner_only_regular_file() {
    let directory = private_tempdir();
    let key_path = directory.path().join("evidence-authentication.key");
    fs::write(&key_path, TEST_EVIDENCE_AUTHENTICATION_KEY).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        read_evidence_authentication_key(&key_path)
            .unwrap()
            .as_slice(),
        TEST_EVIDENCE_AUTHENTICATION_KEY
    );

    fs::write(&key_path, [0xa5; EVIDENCE_AUTHENTICATION_KEY_BYTES - 1]).unwrap();
    assert!(read_evidence_authentication_key(&key_path).is_err());
    fs::write(&key_path, TEST_EVIDENCE_AUTHENTICATION_KEY).unwrap();
    #[cfg(unix)]
    {
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(read_evidence_authentication_key(&key_path).is_err());
    }
    assert!(read_evidence_authentication_key(directory.path()).is_err());
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
