//! A Maker tag-17 punishment signed by the shared LEZ owner at its live nonce
//! loses that nonce when another transaction of the owner takes it first; it
//! can then never be included, and after the punish time nobody else moves
//! the escrow. Asked for again, the punishment the node has not included
//! while the owner's sequenced nonce is above its own is re-planned at that
//! nonce into a durable lineage slot (the original stays reserved), the
//! superseded bytes are never admitted again, and the lineage survives a
//! restart, also one that comes between a re-plan and its journal entry. An
//! included punishment, or one whose nonce is still live, is never re-planned.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::PermissionsExt as _,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use common::{HashType, transaction::LeeTransaction};
use indexer_service_protocol::Block;
use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, BridgeClientError, SidecarCapability};
use lez_bridge_protocol::{
    Hex32, MessageContext, Participant, PrepareNativeXmrPunishV3Request, PreparedTransaction,
    RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor, SubmissionOutcome,
    SubmitTransactionRequest, XmrNativeEscrowTermsV3, XmrNativeEscrowTermsV3Input,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeRuntimeError, BridgeServerCapability, BridgeServerConfig,
    FinalizedIndexerApi, HistoricalAccount, NativeEscrowPlanner, NativePrepareError, NonceSource,
    OfficialNodeRpc, ZecEscrowInstruction, compute_custody_pda, compute_metadata_pda,
    decode_official_public_transaction, decode_prepared_for_signer, program_id_to_hex,
    start_bridge_server,
};
use nssa::{Account, AccountId, PrivateKey, PublicKey, public_transaction::Message};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

const CAPABILITY: &str = "xmr-punish-replan-capability-0001";
const RUN_ID: &str = "xmr-punish-replan-run";
const ESCROW_PROGRAM: [u32; 8] = [0x1020_3040; 8];
const TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];
const SWAP_ID: [u8; 32] = [61; 32];
const FIRST_NONCE: u128 = 41;
const PREPARE_ID: &str = "m4-tag17-prepare-001-replan";

/// The chain as the sidecar sees it: the Maker owner's live nonce (served to
/// the planner's nonce source and `getAccount`), the included transactions,
/// and the sends.
#[derive(Clone)]
struct Chain {
    live_nonce: Arc<Mutex<u128>>,
    sends: Arc<AtomicUsize>,
    included: Arc<Mutex<Vec<LeeTransaction>>>,
}

impl Chain {
    fn new() -> Self {
        Self {
            live_nonce: Arc::new(Mutex::new(FIRST_NONCE)),
            sends: Arc::new(AtomicUsize::new(0)),
            included: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn set_live_nonce(&self, nonce: u128) {
        *self.live_nonce.lock().expect("nonce lock") = nonce;
    }

    fn include_exact(&self, prepared: &PreparedTransaction) {
        let transaction = decode_official_public_transaction(prepared.exact_bytes.as_slice())
            .expect("official prepared transaction");
        self.included
            .lock()
            .expect("included lock")
            .push(LeeTransaction::Public(transaction));
    }

    fn send_count(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl NonceSource for Chain {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
        Ok(*self.live_nonce.lock().expect("nonce lock"))
    }
}

#[derive(Debug)]
struct UnavailableIndexer;

#[async_trait]
impl FinalizedIndexerApi for UnavailableIndexer {
    async fn last_finalized_block_id(&self) -> Result<Option<u64>, BridgeRuntimeError> {
        Err(BridgeRuntimeError::Unavailable)
    }

    async fn block_by_id(&self, _block_id: u64) -> Result<Option<Block>, BridgeRuntimeError> {
        Err(BridgeRuntimeError::Unavailable)
    }

    async fn block_by_hash(
        &self,
        _block_hash: [u8; 32],
    ) -> Result<Option<Block>, BridgeRuntimeError> {
        Err(BridgeRuntimeError::Unavailable)
    }

    async fn account_at_block(
        &self,
        _account_id: [u8; 32],
        _block_id: u64,
    ) -> Result<HistoricalAccount, BridgeRuntimeError> {
        Err(BridgeRuntimeError::Unavailable)
    }
}

async fn start_sequencer(chain: Chain) -> (String, jsonrpsee::server::ServerHandle) {
    let server = ServerBuilder::default()
        .build("127.0.0.1:0")
        .await
        .expect("sequencer binds");
    let address = server.local_addr().expect("sequencer address");
    let mut rpc = RpcModule::new(chain);
    rpc.register_method("checkHealth", |_, _, _| Ok::<_, ErrorObjectOwned>(()))
        .expect("health method");
    rpc.register_method("getChannelId", |_, _, _| {
        Ok::<_, ErrorObjectOwned>(hex::encode([41_u8; 32]))
    })
    .expect("channel method");
    rpc.register_method("getAccount", |_params, chain, _| {
        let account = Account {
            balance: u128::MAX / 2,
            nonce: (*chain.live_nonce.lock().expect("nonce lock")).into(),
            ..Default::default()
        };
        Ok::<_, ErrorObjectOwned>(account)
    })
    .expect("account method");
    rpc.register_method("getTransaction", |params, chain, _| {
        let requested: HashType = params.one()?;
        let observed = chain
            .included
            .lock()
            .expect("included lock")
            .iter()
            .find(|transaction| transaction.hash() == requested)
            .cloned()
            .map(|transaction| (transaction, 1_u64));
        Ok::<_, ErrorObjectOwned>(observed)
    })
    .expect("transaction lookup method");
    rpc.register_method("sendTransaction", |params, chain, _| {
        let transaction: LeeTransaction = params.one()?;
        chain.sends.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ErrorObjectOwned>(transaction.hash())
    })
    .expect("send method");
    (format!("http://{address}"), server.start(rpc))
}

fn account(byte: u8) -> (AccountId, PrivateKey, PublicKey) {
    let key = PrivateKey::try_new([byte; 32]).expect("private key");
    let public = PublicKey::new_from_private_key(&key);
    (AccountId::from(&public), key, public)
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn runtime(signer: AccountId) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        Participant::Maker,
        RuntimeCompatibility::LeeV0_2_0,
        h(40),
        h(41),
        h(42),
        program_id_to_hex(ESCROW_PROGRAM),
        Hex32::from_bytes(signer.into_value()),
    )
}

fn context(request_id: &str) -> MessageContext {
    MessageContext::new(
        RunId::new(RUN_ID).expect("run id"),
        RequestId::new(request_id).expect("request id"),
        Participant::Maker,
    )
}

/// XMR escrow terms whose committed messages are planned at nonce 0, as the
/// Maker Node's Stage A plans them.
fn terms(depositor: AccountId, claimant: AccountId) -> XmrNativeEscrowTermsV3 {
    let (claim_authority, _, claim_public) = account(63);
    let (refund_authority, _, refund_public) = account(64);
    let metadata = compute_metadata_pda(&ESCROW_PROGRAM, &SWAP_ID);
    let custody = compute_custody_pda(&ESCROW_PROGRAM, &SWAP_ID);
    let message = |accounts, instruction| {
        Message::try_new(ESCROW_PROGRAM, accounts, vec![0_u128.into()], instruction)
            .expect("canonical message")
            .hash()
    };
    let claim_partial_context_binding = h(15);
    let mut commitment = Sha256::new();
    commitment.update(b"logos.gateway.lez-xmr.claim-partial-commitment.v1\0");
    commitment.update(claim_partial_context_binding.as_bytes());
    commitment.update([77; 32]);
    XmrNativeEscrowTermsV3::new(XmrNativeEscrowTermsV3Input {
        swap_id: Hex32::from_bytes(SWAP_ID),
        activation_commitment: h(2),
        escrow_program_id: program_id_to_hex(ESCROW_PROGRAM),
        authenticated_transfer_program_id: program_id_to_hex(TRANSFER_PROGRAM),
        metadata_account_id: Hex32::from_bytes(metadata.into_value()),
        custody_account_id: Hex32::from_bytes(custody.into_value()),
        depositor: Participant::Taker,
        depositor_account_id: Hex32::from_bytes(depositor.into_value()),
        claimant: Participant::Maker,
        claimant_account_id: Hex32::from_bytes(claimant.into_value()),
        claim_aggregate_x_only_public_key: Hex32::from_bytes(*claim_public.value()),
        claim_authority_account_id: Hex32::from_bytes(claim_authority.into_value()),
        refund_aggregate_x_only_public_key: Hex32::from_bytes(*refund_public.value()),
        refund_authority_account_id: Hex32::from_bytes(refund_authority.into_value()),
        maker_dleq_transcript_commitment: h(13),
        taker_dleq_transcript_commitment: h(14),
        claim_partial_context_binding,
        claim_partial_commitment: Hex32::from_bytes(commitment.finalize().into()),
        amount: 75,
        refund_at_ms: 10_000,
        punish_at_ms: 20_000,
        claim_message_hash: Hex32::from_bytes(message(
            vec![metadata, custody, claimant, claim_authority],
            ZecEscrowInstruction::ClaimNativeXmr { swap_id: SWAP_ID },
        )),
        refund_message_hash: Hex32::from_bytes(message(
            vec![metadata, custody, depositor, refund_authority],
            ZecEscrowInstruction::RefundNativeXmr { swap_id: SWAP_ID },
        )),
        punish_message_hash: Hex32::from_bytes(message(
            vec![metadata, custody, claimant],
            ZecEscrowInstruction::PunishNativeXmr { swap_id: SWAP_ID },
        )),
    })
    .expect("valid XMR terms")
}

fn nonce_of(prepared: &PreparedTransaction, signer: AccountId) -> u128 {
    let transaction = decode_prepared_for_signer(prepared, signer).expect("signer transaction");
    let [nonce] = transaction.message().nonces.as_slice() else {
        panic!("one signer nonce");
    };
    u128::from(*nonce)
}

/// One Maker sidecar on a durable state directory, as a swap's sidecar runs.
struct Sidecar {
    server: lez_v0_2_sidecar::BridgeServerHandle,
    planner: Arc<NativeEscrowPlanner>,
    signer: AccountId,
    terms: XmrNativeEscrowTermsV3,
}

impl Sidecar {
    fn client(&self) -> BridgeClient {
        BridgeClient::connect(BridgeClientConfig::new(
            self.server.endpoint(),
            SidecarCapability::new(CAPABILITY).expect("client capability"),
            RunId::new(RUN_ID).expect("run id"),
            runtime(self.signer),
            Duration::from_secs(2),
        ))
        .expect("bridge client")
    }

    fn request(&self) -> PrepareNativeXmrPunishV3Request {
        PrepareNativeXmrPunishV3Request::new(context(PREPARE_ID), runtime(self.signer), self.terms)
    }

    /// The tool's preparation: the same request every time.
    async fn prepare(&self) -> Result<PreparedTransaction, BridgeClientError> {
        let prepared = self
            .client()
            .prepare_native_xmr_punish_v3(self.request())
            .await?;
        self.planner
            .validate_prepared_native_xmr_punish_v3(&self.request(), &prepared)
            .expect("a prepared punishment validates");
        Ok(prepared.punish)
    }

    /// The tool's submission: request id from the transaction id.
    async fn submit(&self, punish: &PreparedTransaction) -> SubmissionOutcome {
        self.client()
            .submit_transaction(SubmitTransactionRequest::new(
                MessageContext::new(
                    RunId::new(RUN_ID).expect("run id"),
                    punish.transaction_id.submission_request_id(),
                    Participant::Maker,
                ),
                runtime(self.signer),
                punish.clone(),
            ))
            .await
            .expect("submission answered")
            .outcome
    }
}

fn secure_directory() -> TempDir {
    let directory = TempDir::new().expect("sidecar directory");
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
        .expect("private directory");
    let planner_directory = directory.path().join("planner");
    fs::create_dir(&planner_directory).expect("planner directory");
    fs::set_permissions(&planner_directory, fs::Permissions::from_mode(0o700))
        .expect("private planner directory");
    directory
}

async fn start_sidecar(sequencer_endpoint: &str, directory: &Path, chain: &Chain) -> Sidecar {
    let (depositor, _, _) = account(62);
    let (signer, key, _) = account(65);
    let planner = Arc::new(
        NativeEscrowPlanner::new_durable(
            Participant::Maker,
            key,
            ESCROW_PROGRAM,
            TRANSFER_PROGRAM,
            runtime(signer),
            Arc::new(chain.clone()),
            directory.join("planner"),
        )
        .expect("planner"),
    );
    let bridge_runtime = Arc::new(BridgeRuntime::new(
        runtime(signer),
        Arc::clone(&planner),
        Arc::new(OfficialNodeRpc::connect(sequencer_endpoint).expect("official node")),
        Arc::new(UnavailableIndexer),
    ));
    let server = start_bridge_server(
        BridgeServerConfig::new(
            RunId::new(RUN_ID).expect("run id"),
            BridgeServerCapability::new(CAPABILITY).expect("server capability"),
            directory.join("bridge-idempotency.json"),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        ),
        bridge_runtime,
    )
    .await
    .expect("sidecar starts");
    Sidecar {
        server,
        planner,
        signer,
        terms: terms(depositor, signer),
    }
}

fn supersession_files(directory: &Path) -> Vec<String> {
    let mut names = fs::read_dir(directory.join("planner"))
        .expect("planner directory listing")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.starts_with("xmr-native-punish-supersession."))
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one punishment lineage is followed end to end"
)]
async fn a_punishment_whose_nonce_was_taken_is_replanned_at_the_live_nonce() {
    let chain = Chain::new();
    let (endpoint, sequencer) = start_sequencer(chain.clone()).await;
    let directory = secure_directory();
    let sidecar = start_sidecar(&endpoint, directory.path(), &chain).await;
    let signer = sidecar.signer;

    let original = sidecar.prepare().await.expect("punishment prepared");
    assert_eq!(nonce_of(&original, signer), FIRST_NONCE);
    assert_eq!(sidecar.submit(&original).await, SubmissionOutcome::Accepted);
    assert_eq!(chain.send_count(), 1);

    // Another transaction of the Maker's owner took nonce 41 first.
    chain.set_live_nonce(FIRST_NONCE + 1);
    let replanned = sidecar.prepare().await.expect("punishment re-planned");
    assert_eq!(nonce_of(&replanned, signer), FIRST_NONCE + 1);
    assert_ne!(replanned.transaction_id, original.transaction_id);
    assert_eq!(
        decode_prepared_for_signer(&original, signer)
            .expect("original")
            .message()
            .instruction_data,
        decode_prepared_for_signer(&replanned, signer)
            .expect("re-planned")
            .message()
            .instruction_data,
        "the same punishment of the same escrow"
    );
    assert_eq!(
        supersession_files(directory.path()),
        ["xmr-native-punish-supersession.1.v3.json"]
    );
    assert!(
        directory
            .path()
            .join("planner/xmr-native-punish-reservation.v3.json")
            .exists(),
        "the original reservation stays"
    );

    // The superseded bytes are no longer owned: never sent again. Their
    // journaled submission replays without reaching the node.
    assert!(
        sidecar
            .planner
            .validate_owned_submission(&original)
            .await
            .is_err()
    );
    assert_eq!(sidecar.submit(&original).await, SubmissionOutcome::Accepted);
    assert_eq!(chain.send_count(), 1);
    assert_eq!(
        sidecar.submit(&replanned).await,
        SubmissionOutcome::Accepted
    );
    assert_eq!(chain.send_count(), 2);

    // Asked again with the nonce still at the re-plan's: the same bytes.
    assert_eq!(sidecar.prepare().await.expect("replay"), replanned);
    assert_eq!(supersession_files(directory.path()).len(), 1);

    // A restart recovers the lineage; the current punishment is the re-plan.
    sidecar.server.stop().await.expect("stop");
    let restarted = start_sidecar(&endpoint, directory.path(), &chain).await;
    assert_eq!(restarted.prepare().await.expect("recovered"), replanned);
    assert!(
        restarted
            .planner
            .validate_owned_submission(&original)
            .await
            .is_err()
    );

    restarted.server.stop().await.expect("stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
}

#[tokio::test]
async fn an_included_punishment_or_a_live_nonce_is_never_replanned() {
    let chain = Chain::new();
    let (endpoint, sequencer) = start_sequencer(chain.clone()).await;
    let directory = secure_directory();
    let sidecar = start_sidecar(&endpoint, directory.path(), &chain).await;

    let original = sidecar.prepare().await.expect("punishment prepared");
    // The nonce has not moved: nothing to re-plan.
    assert_eq!(sidecar.prepare().await.expect("same"), original);
    assert!(matches!(
        sidecar
            .planner
            .supersede_native_xmr_punish_v3(
                &sidecar.request(),
                original.transaction_id,
                FIRST_NONCE
            )
            .await,
        Err(NativePrepareError::SupersededNonceStillLive)
    ));
    // It landed, which moved the nonce: kept, never re-planned.
    chain.include_exact(&original);
    chain.set_live_nonce(FIRST_NONCE + 1);
    assert_eq!(sidecar.prepare().await.expect("included"), original);
    assert_eq!(
        sidecar.submit(&original).await,
        SubmissionOutcome::AlreadyKnown
    );
    assert!(supersession_files(directory.path()).is_empty());
    assert_eq!(chain.send_count(), 0);

    sidecar.server.stop().await.expect("stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
}

#[tokio::test]
async fn a_restart_after_a_replan_and_before_its_journal_entry_starts() {
    let chain = Chain::new();
    let (endpoint, sequencer) = start_sequencer(chain.clone()).await;
    let directory = secure_directory();
    let sidecar = start_sidecar(&endpoint, directory.path(), &chain).await;
    let original = sidecar.prepare().await.expect("punishment prepared");

    // The re-plan is reserved, the journal still answers the original.
    let replanned = sidecar
        .planner
        .supersede_native_xmr_punish_v3(
            &sidecar.request(),
            original.transaction_id,
            FIRST_NONCE + 3,
        )
        .await
        .expect("re-planned in the planner only")
        .punish;
    assert_eq!(nonce_of(&replanned, sidecar.signer), FIRST_NONCE + 3);
    sidecar.server.stop().await.expect("stop");

    chain.set_live_nonce(FIRST_NONCE + 3);
    let restarted = start_sidecar(&endpoint, directory.path(), &chain).await;
    assert_eq!(restarted.prepare().await.expect("recovered"), replanned);

    restarted.server.stop().await.expect("stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
}

/// Tag 17 has no upper window and after P nothing else moves the escrow, so
/// a punishment can always still land: it is re-planned for as long as its
/// nonce keeps being taken, past five re-plans, never given up for the count
/// (the rule of issue #172), and its current bytes are sent again until they
/// land. A restart recovers the whole lineage.
#[tokio::test]
async fn a_punishment_replanned_more_than_five_times_still_lands() {
    let chain = Chain::new();
    let (endpoint, sequencer) = start_sequencer(chain.clone()).await;
    let directory = secure_directory();
    let sidecar = start_sidecar(&endpoint, directory.path(), &chain).await;
    let mut current = sidecar.prepare().await.expect("punishment prepared");
    for step in 1..=8_u128 {
        chain.set_live_nonce(FIRST_NONCE + step);
        let replanned = sidecar
            .prepare()
            .await
            .expect("re-planned while it can land");
        assert_eq!(nonce_of(&replanned, sidecar.signer), FIRST_NONCE + step);
        assert_ne!(replanned, current);
        current = replanned;
    }
    assert_eq!(supersession_files(directory.path()).len(), 8);

    // Accepted is not included: the sequencer may drop it, so the current
    // punishment is sent again.
    assert_eq!(sidecar.submit(&current).await, SubmissionOutcome::Accepted);
    assert_eq!(sidecar.submit(&current).await, SubmissionOutcome::Accepted);
    assert_eq!(chain.send_count(), 2);

    sidecar.server.stop().await.expect("stop");
    let restarted = start_sidecar(&endpoint, directory.path(), &chain).await;
    assert_eq!(
        restarted.prepare().await.expect("lineage recovered"),
        current
    );
    // It lands, which moves the nonce: kept, and already known.
    chain.include_exact(&current);
    chain.set_live_nonce(FIRST_NONCE + 9);
    assert_eq!(restarted.prepare().await.expect("included"), current);
    assert_eq!(
        restarted.submit(&current).await,
        SubmissionOutcome::AlreadyKnown
    );
    assert_eq!(chain.send_count(), 2);

    restarted.server.stop().await.expect("stop");
    sequencer.stop().expect("sequencer stop");
    sequencer.stopped().await;
}
