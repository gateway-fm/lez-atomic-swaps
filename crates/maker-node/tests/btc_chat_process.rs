//! Separate-process RED contract for the smallest local BTC Chat handoff.
//!
//! This deliberately stops at application handoff: no Bitcoin Core or LEZ
//! sidecar is contacted. The daemon and the real Taker CLI must negotiate one
//! exact agreement, publish role-fixed actor authority, and make the resulting
//! receipt usable by the offline monitor before chain lifecycle work begins.

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{
        DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _,
    },
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[path = "support/btc_fixture.rs"]
mod btc_fixture;
use btc_fixture::BtcAuthorityFixture;

use btc_reference_actor::{ActorConfig, ActorRole};
use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    AuthenticatedOfferRefV1, BtcChatProposalV1, BtcChatProposeRequestV1, DeliveryOfferQueryV1,
    LocalPriceSetRequest, PairConfigureRequest, RunLocalDelivery, call_local_rpc,
};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{
    LocalPriceV1, MakerActorKindV1, MakerActorScheduleState, MakerBtcNegotiationStatus,
    MakerOfferId, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1, SqliteSwapStore,
    maker_btc_chat_swap_id,
};
use rustix::process::{Pid, Signal, kill_process};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;

const OFFER_ID: &str = "m5-btc-chat-offer-001";
const RESERVATION_ID: &str = "m5-btc-chat-reservation-001";
const FOREIGN_UNITS_SAT: u64 = 100_000;
const MAKER_AGREEMENT_KEY: u8 = 1;
const TAKER_AGREEMENT_KEY: u8 = 2;

#[tokio::test]
async fn real_taker_and_daemon_handoff_exact_btc_agreement_to_role_fixed_actors() {
    let run = tempdir().expect("isolated BTC Chat process root");
    make_private_directory(run.path());
    let runtime = run.path().join("runtime");
    make_private_directory(&runtime);
    let delivery = run.path().join("delivery");
    let socket = runtime.join("maker.sock");
    let chat_socket = runtime.join("chat.sock");
    let ready = runtime.join("ready");
    let database = run.path().join("maker.sqlite3");
    let delivery_key = run.path().join("delivery.key");
    let maker_agreement_key = run.path().join("maker-agreement.key");
    write_raw_key(&delivery_key, 8);
    write_raw_key(&maker_agreement_key, MAKER_AGREEMENT_KEY);
    let daemon_base = DaemonBase {
        socket: &socket,
        chat_socket: &chat_socket,
        ready: &ready,
        database: &database,
        delivery: &delivery,
        delivery_key: &delivery_key,
        maker_agreement_key: &maker_agreement_key,
    };

    let route =
        MakerRouteV1::new(Pair::Bitcoin, SwapDirection::TakerSellsForeign).expect("BTC route");
    let offer_id = MakerOfferId::new(OFFER_ID).unwrap();
    let reservation_id = request(RESERVATION_ID);

    // Bootstrap Delivery without Chat or pair/actor authority. Its only job is
    // to publish the exact signed offer through the real control RPC and Maker
    // CLI; agreement authority is introduced only after the public swap ID can
    // be derived from that envelope.
    let mut bootstrap_daemon = start_delivery_only_daemon(&daemon_base);
    wait_delivery_only_ready(&mut bootstrap_daemon, &daemon_base);
    configure_live_route(&socket, route).await;
    publish_offer(&socket, &offer_id);
    let delivery_maker = public_key(&key(8));
    let authenticated =
        plan_and_discover(&delivery, &offer_id, &reservation_id, delivery_maker, route).await;
    stop_delivery_only_daemon(&mut bootstrap_daemon, &daemon_base);

    // These are explicit per-role authority inputs, not authority copied out of
    // the peer or derived by Chat. Only the public application swap ID is bound
    // to the already authenticated offer and owner-chosen reservation.
    let authority = BtcAuthorityFixture::new(
        run.path(),
        "live",
        maker_btc_chat_swap_id(&authenticated.commitment(), &reservation_id),
    );
    let paths = daemon_base.with_authority(&authority);
    let mut daemon = start_daemon(&paths);
    wait_ready(&mut daemon, &paths);

    let taker = TakerFiles::new(run.path(), &authority);
    stage_proposal(
        &chat_socket,
        &offer_id,
        &reservation_id,
        &authenticated,
        &taker,
    )
    .await;

    let accepted_at = now();
    let accepted = run_taker(
        &taker,
        &delivery,
        &chat_socket,
        &offer_id,
        &reservation_id,
        &delivery_maker,
        accepted_at,
    );
    assert_initial_acceptance(&accepted);

    let final_wire = fs::read(&taker.final_agreement).expect("exact countersigned BTC agreement");
    assert_completed_handoff(
        &database,
        &offer_id,
        &reservation_id,
        &authenticated,
        &final_wire,
        &paths,
        &taker,
    );

    // Delivery is irrelevant after acceptance. Exact replay must come only
    // from the durable Chat state and must preserve every published inode.
    let snapshot = ArtifactSnapshot::capture(&paths, &taker);
    fs::rename(&delivery, run.path().join("delivery.offline")).unwrap();
    let replay = run_taker(
        &taker,
        &delivery,
        &chat_socket,
        &offer_id,
        &reservation_id,
        &delivery_maker,
        accepted_at,
    );
    assert_exact_replay(&replay);
    snapshot.assert_unchanged(&paths, &taker);
    assert_offline_receipt_monitor(&taker.receipt);

    stop_daemon(&mut daemon, &paths);
    assert_completed_durable(
        &database,
        &offer_id,
        &reservation_id,
        &authenticated,
        &final_wire,
    );
}

async fn stage_proposal(
    chat_socket: &Path,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    authenticated: &AuthenticatedOfferRefV1,
    taker: &TakerFiles,
) {
    let draft_wire = fs::read(&taker.unsigned_draft).expect("explicit unsigned BTC draft");
    let staged: BtcChatProposalV1 = call_local_rpc(
        chat_socket,
        "btc_chat_propose_v1",
        &BtcChatProposeRequestV1 {
            schema_version: 1,
            request_id: chat_request(reservation_id, b"propose"),
            offer_id: offer_id.clone(),
            expected_offer_revision: 1,
            reservation_id: reservation_id.clone(),
            foreign_units: FOREIGN_UNITS_SAT,
            signed_offer_envelope: authenticated.signed_envelope().to_vec(),
            unsigned_draft_wire: draft_wire,
        },
    )
    .await
    .expect("Maker validates, signs, and durably stages the BTC proposal");
    assert_eq!(staged.schema_version, 1);
    assert_eq!(staged.offer_revision, 2);
    assert!(!staged.was_replay);
    assert_eq!(staged.reservation_id, *reservation_id);
    assert_eq!(staged.lez_units, 5_000);
    assert_eq!(
        staged.maker_identity,
        public_key(&key(MAKER_AGREEMENT_KEY)).serialize()
    );
    assert_eq!(
        staged.taker_identity,
        public_key(&key(TAKER_AGREEMENT_KEY)).serialize()
    );
    assert!(!staged.proposal_wire.is_empty());
}

fn assert_initial_acceptance(accepted: &Value) {
    assert_eq!(accepted["schema_version"], 1);
    assert_eq!(accepted["offer_revision"], 3);
    assert_eq!(accepted["replay"]["proposal"], true);
    assert_eq!(accepted["replay"]["completion"], false);
    assert_eq!(accepted["replay"]["agreement_file"], false);
    assert_eq!(accepted["private_material_disclosed"], false);
    assert_eq!(accepted["actor"]["role"], "taker");
    assert_eq!(accepted["actor"]["provisioning_replay"], false);
    assert_eq!(accepted["actor"]["receipt_replay"], false);
}

fn assert_exact_replay(replay: &Value) {
    assert_eq!(replay["replay"]["proposal"], true);
    assert_eq!(replay["replay"]["completion"], true);
    assert_eq!(replay["replay"]["agreement_file"], true);
    assert_eq!(replay["actor"]["provisioning_replay"], true);
    assert_eq!(replay["actor"]["receipt_replay"], true);
}

async fn discover_exact_offer(
    delivery: &Path,
    maker_key: PublicKey,
    route: MakerRouteV1,
) -> AuthenticatedOfferRefV1 {
    RunLocalDelivery::subscriber(delivery, maker_key)
        .unwrap()
        .discover(&DeliveryOfferQueryV1::for_route(route, now()))
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("published BTC offer is discoverable")
}

async fn plan_and_discover(
    delivery: &Path,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    maker_key: PublicKey,
    route: MakerRouteV1,
) -> AuthenticatedOfferRefV1 {
    let planned = plan_btc_offer(delivery, offer_id, reservation_id, &maker_key, now());
    let authenticated = discover_exact_offer(delivery, maker_key, route).await;
    assert_btc_plan(&planned, offer_id, reservation_id, &authenticated);
    authenticated
}

async fn configure_live_route(socket: &Path, route: MakerRouteV1) {
    let disabled = MakerPairConfigurationV1::new(
        route,
        false,
        MakerPriceSourceKind::Local,
        FOREIGN_UNITS_SAT,
        FOREIGN_UNITS_SAT,
        300,
    )
    .unwrap();
    let _: Value = call_local_rpc(
        socket,
        "maker_pair_configure",
        &PairConfigureRequest {
            request_id: request("m5-btc-route-create-001"),
            expected_revision: None,
            configuration: disabled,
        },
    )
    .await
    .unwrap();
    let _: Value = call_local_rpc(
        socket,
        "maker_local_price_set",
        &LocalPriceSetRequest {
            request_id: request("m5-btc-price-create-001"),
            expected_revision: None,
            price: LocalPriceV1::new(route, 1, 20).unwrap(),
        },
    )
    .await
    .unwrap();
    let enabled = MakerPairConfigurationV1::new(
        route,
        true,
        MakerPriceSourceKind::Local,
        FOREIGN_UNITS_SAT,
        FOREIGN_UNITS_SAT,
        300,
    )
    .unwrap();
    let _: Value = call_local_rpc(
        socket,
        "maker_pair_configure",
        &PairConfigureRequest {
            request_id: request("m5-btc-route-enable-001"),
            expected_revision: Some(1),
            configuration: enabled,
        },
    )
    .await
    .unwrap();
}

fn publish_offer(socket: &Path, offer_id: &MakerOfferId) {
    let output = Command::new(env!("CARGO_BIN_EXE_lez-maker"))
        .arg("--socket")
        .arg(socket)
        .arg("publish-offer")
        .arg("--request-id")
        .arg("m5-btc-publish-001")
        .arg("--offer-id")
        .arg(offer_id.as_str())
        .arg("--pair")
        .arg("bitcoin")
        .arg("--direction")
        .arg("taker-sells-foreign")
        .output()
        .expect("run real Maker CLI");
    assert!(
        output.status.success(),
        "BTC offer publication failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn plan_btc_offer(
    delivery: &Path,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    maker_key: &PublicKey,
    planned_at: u64,
) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_lez-taker"))
        .arg("--delivery-directory")
        .arg(delivery)
        .arg("--maker-public-key")
        .arg(hex::encode(maker_key.serialize()))
        .arg("--now-unix-seconds")
        .arg(planned_at.to_string())
        .arg("--pair")
        .arg("bitcoin")
        .arg("--direction")
        .arg("taker-sells-foreign")
        .arg("--plan-btc-offer")
        .arg(offer_id.as_str())
        .arg("--reservation-id")
        .arg(reservation_id.as_str())
        .arg("--foreign-units")
        .arg(FOREIGN_UNITS_SAT.to_string())
        .output()
        .expect("run real BTC Taker planning process");
    assert!(
        output.status.success(),
        "real BTC Taker planning failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("real Taker returns a bounded BTC plan")
}

fn assert_btc_plan(
    planned: &Value,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    authenticated: &AuthenticatedOfferRefV1,
) {
    assert_eq!(planned["schema_version"], 1);
    assert_eq!(planned["offer_id"], offer_id.as_str());
    assert_eq!(planned["reservation_id"], reservation_id.as_str());
    assert_eq!(
        planned["signed_envelope_sha256"],
        hex::encode(authenticated.commitment())
    );
    assert_eq!(
        planned["swap_id"],
        hex::encode(maker_btc_chat_swap_id(
            &authenticated.commitment(),
            reservation_id
        ))
    );
    assert_eq!(planned["foreign_units"], FOREIGN_UNITS_SAT);
    assert_eq!(planned["lez_units"], 5_000);
    assert_eq!(planned["private_material_disclosed"], false);
}

struct TakerFiles {
    unsigned_draft: PathBuf,
    signing_key: PathBuf,
    final_agreement: PathBuf,
    source_config: PathBuf,
    actor_root: PathBuf,
    receipt: PathBuf,
}

impl TakerFiles {
    fn new(run: &Path, authority: &BtcAuthorityFixture) -> Self {
        let root = run.join("taker");
        make_private_directory(&root);
        let files = Self {
            unsigned_draft: root.join("unsigned-draft-v1.borsh"),
            signing_key: root.join("agreement.key"),
            final_agreement: root.join("agreement-v1.borsh"),
            source_config: authority.taker_source_config.clone(),
            actor_root: root.join("accepted-actor"),
            receipt: root.join("acceptance-receipt.json"),
        };
        write_private(
            &files.unsigned_draft,
            &fs::read(&authority.unsigned_draft).expect("fixture unsigned BTC draft"),
        );
        write_raw_key(&files.signing_key, TAKER_AGREEMENT_KEY);
        files
    }
}

fn run_taker(
    taker: &TakerFiles,
    delivery: &Path,
    chat_socket: &Path,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    maker_key: &PublicKey,
    accepted_at: u64,
) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_lez-taker"))
        .arg("--delivery-directory")
        .arg(delivery)
        .arg("--maker-public-key")
        .arg(hex::encode(maker_key.serialize()))
        .arg("--now-unix-seconds")
        .arg(accepted_at.to_string())
        .arg("--pair")
        .arg("bitcoin")
        .arg("--direction")
        .arg("taker-sells-foreign")
        .arg("--accept-btc-offer")
        .arg(offer_id.as_str())
        .arg("--chat-socket")
        .arg(chat_socket)
        .arg("--reservation-id")
        .arg(reservation_id.as_str())
        .arg("--foreign-units")
        .arg(FOREIGN_UNITS_SAT.to_string())
        .arg("--unsigned-draft-file")
        .arg(&taker.unsigned_draft)
        .arg("--taker-signing-key-file")
        .arg(&taker.signing_key)
        .arg("--agreement-output-file")
        .arg(&taker.final_agreement)
        .arg("--btc-source-taker-config")
        .arg(&taker.source_config)
        .arg("--btc-taker-actor-root")
        .arg(&taker.actor_root)
        .arg("--btc-acceptance-receipt")
        .arg(&taker.receipt)
        .output()
        .expect("run real BTC Taker process");
    assert!(
        output.status.success(),
        "real BTC Taker failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("real Taker returns bounded JSON")
}

fn assert_completed_handoff(
    database: &Path,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    authenticated: &AuthenticatedOfferRefV1,
    final_wire: &[u8],
    daemon: &DaemonPaths<'_>,
    taker: &TakerFiles,
) {
    let store = SqliteSwapStore::open(database).unwrap();
    let negotiation = store
        .load_btc_maker_negotiation(offer_id)
        .unwrap()
        .expect("durable completed BTC negotiation");
    assert_eq!(negotiation.status(), MakerBtcNegotiationStatus::Completed);
    assert_eq!(negotiation.reservation_id(), reservation_id);
    assert_eq!(negotiation.offer_commitment(), &authenticated.commitment());
    assert_eq!(negotiation.final_agreement_wire(), Some(final_wire));

    let actors = store.list_maker_actor_processes().unwrap();
    assert_eq!(actors.len(), 1);
    assert_eq!(actors[0].manifest().kind(), MakerActorKindV1::Bitcoin);
    assert_eq!(actors[0].schedule_state(), MakerActorScheduleState::Queued);
    let maker_config_path = actors[0].manifest().config_path();
    assert!(maker_config_path.starts_with(daemon.maker_actor_root));
    let maker_config = ActorConfig::load_private(maker_config_path).unwrap();
    assert_eq!(maker_config.role(), ActorRole::Maker);
    let maker_bundle_root = maker_config_path
        .parent()
        .and_then(Path::parent)
        .expect("Maker config is inside one role-fixed bundle");
    assert_eq!(maker_bundle_root.parent(), Some(daemon.maker_actor_root));
    assert!(!maker_bundle_root.join("taker").exists());

    let taker_config_path = taker.actor_root.join("taker/actor-config.json");
    let taker_config = ActorConfig::load_private(&taker_config_path).unwrap();
    assert_eq!(taker_config.role(), ActorRole::Taker);
    assert!(!taker.actor_root.join("maker").exists());
    assert_eq!(
        maker_config.supervised_swap_id().unwrap(),
        taker_config.supervised_swap_id().unwrap()
    );
    assert_eq!(
        fs::read(maker_bundle_root.join("shared/agreement-v1.borsh")).unwrap(),
        final_wire
    );
    assert_eq!(
        fs::read(taker.actor_root.join("shared/agreement-v1.borsh")).unwrap(),
        final_wire
    );

    let receipt_metadata = fs::symlink_metadata(&taker.receipt).unwrap();
    assert_eq!(receipt_metadata.permissions().mode() & 0o7777, 0o600);
    assert_eq!(receipt_metadata.nlink(), 1);
    let receipt: Value = serde_json::from_slice(&fs::read(&taker.receipt).unwrap()).unwrap();
    assert_eq!(receipt["schema_version"], 1);
    assert_eq!(receipt["pair"], "bitcoin");
    assert_eq!(receipt["role"], "taker");
    assert_eq!(receipt["actor_config_file"], json!(taker_config_path));
    assert_eq!(
        receipt["agreement_sha256"],
        hex::encode(Sha256::digest(final_wire))
    );
}

fn assert_completed_durable(
    database: &Path,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    authenticated: &AuthenticatedOfferRefV1,
    final_wire: &[u8],
) {
    let store = SqliteSwapStore::open(database).unwrap();
    let negotiation = store
        .load_btc_maker_negotiation(offer_id)
        .unwrap()
        .expect("BTC completion survives daemon shutdown");
    assert_eq!(negotiation.status(), MakerBtcNegotiationStatus::Completed);
    assert_eq!(negotiation.reservation_id(), reservation_id);
    assert_eq!(negotiation.offer_commitment(), &authenticated.commitment());
    assert_eq!(negotiation.final_agreement_wire(), Some(final_wire));
    assert_eq!(store.list_maker_actor_processes().unwrap().len(), 1);
}

struct ArtifactSnapshot {
    maker_config_inode: u64,
    maker_config: Vec<u8>,
    taker_config_inode: u64,
    taker_config: Vec<u8>,
    maker_agreement_inode: u64,
    taker_agreement_inode: u64,
    receipt_inode: u64,
    receipt: Vec<u8>,
}

impl ArtifactSnapshot {
    fn capture(daemon: &DaemonPaths<'_>, taker: &TakerFiles) -> Self {
        let maker_bundle = single_maker_bundle(daemon.maker_actor_root);
        let maker_config = maker_bundle.join("maker/actor-config.json");
        let taker_config = taker.actor_root.join("taker/actor-config.json");
        let maker_agreement = maker_bundle.join("shared/agreement-v1.borsh");
        let taker_agreement = taker.actor_root.join("shared/agreement-v1.borsh");
        Self {
            maker_config_inode: inode(&maker_config),
            maker_config: fs::read(maker_config).unwrap(),
            taker_config_inode: inode(&taker_config),
            taker_config: fs::read(taker_config).unwrap(),
            maker_agreement_inode: inode(&maker_agreement),
            taker_agreement_inode: inode(&taker_agreement),
            receipt_inode: inode(&taker.receipt),
            receipt: fs::read(&taker.receipt).unwrap(),
        }
    }

    fn assert_unchanged(&self, daemon: &DaemonPaths<'_>, taker: &TakerFiles) {
        let maker_bundle = single_maker_bundle(daemon.maker_actor_root);
        let maker_config = maker_bundle.join("maker/actor-config.json");
        let taker_config = taker.actor_root.join("taker/actor-config.json");
        assert_eq!(inode(&maker_config), self.maker_config_inode);
        assert_eq!(fs::read(maker_config).unwrap(), self.maker_config);
        assert_eq!(inode(&taker_config), self.taker_config_inode);
        assert_eq!(fs::read(taker_config).unwrap(), self.taker_config);
        assert_eq!(
            inode(&maker_bundle.join("shared/agreement-v1.borsh")),
            self.maker_agreement_inode
        );
        assert_eq!(
            inode(&taker.actor_root.join("shared/agreement-v1.borsh")),
            self.taker_agreement_inode
        );
        assert_eq!(inode(&taker.receipt), self.receipt_inode);
        assert_eq!(fs::read(&taker.receipt).unwrap(), self.receipt);
    }
}

fn assert_offline_receipt_monitor(receipt: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_lez-taker"))
        .arg("monitor")
        .arg("--receipt")
        .arg(receipt)
        .output()
        .expect("run receipt-bound offline Taker monitor");
    assert!(
        output.status.success(),
        "offline BTC monitor failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({
            "schema_version": 1,
            "pair": "bitcoin",
            "role": "taker",
            "state": "not_activated"
        })
    );
}

fn single_maker_bundle(actor_root: &Path) -> PathBuf {
    let mut bundles = fs::read_dir(actor_root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    bundles.sort();
    assert_eq!(
        bundles.len(),
        1,
        "exactly one digest-scoped Maker bundle is published"
    );
    bundles.pop().unwrap()
}

struct DaemonPaths<'a> {
    socket: &'a Path,
    chat_socket: &'a Path,
    ready: &'a Path,
    database: &'a Path,
    delivery: &'a Path,
    delivery_key: &'a Path,
    maker_agreement_key: &'a Path,
    source_maker_config: &'a Path,
    maker_actor_root: &'a Path,
    actor_program: &'a Path,
    actor_program_sha256: &'a str,
}

struct DaemonBase<'a> {
    socket: &'a Path,
    chat_socket: &'a Path,
    ready: &'a Path,
    database: &'a Path,
    delivery: &'a Path,
    delivery_key: &'a Path,
    maker_agreement_key: &'a Path,
}

impl DaemonBase<'_> {
    fn with_authority<'a>(&'a self, authority: &'a BtcAuthorityFixture) -> DaemonPaths<'a> {
        DaemonPaths {
            socket: self.socket,
            chat_socket: self.chat_socket,
            ready: self.ready,
            database: self.database,
            delivery: self.delivery,
            delivery_key: self.delivery_key,
            maker_agreement_key: self.maker_agreement_key,
            source_maker_config: &authority.maker_source_config,
            maker_actor_root: &authority.maker_actor_root,
            actor_program: &authority.actor_program,
            actor_program_sha256: &authority.actor_program_sha256,
        }
    }
}

fn delivery_only_daemon_command(base: &DaemonBase<'_>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lez-maker-daemon"));
    command
        .arg("--socket")
        .arg(base.socket)
        .arg("--database")
        .arg(base.database)
        .arg("--ready-file")
        .arg(base.ready)
        .arg("--delivery-directory")
        .arg(base.delivery)
        .arg("--delivery-signing-key-file")
        .arg(base.delivery_key);
    command
}

fn start_delivery_only_daemon(base: &DaemonBase<'_>) -> Child {
    delivery_only_daemon_command(base)
        .spawn()
        .expect("start isolated Delivery-only Maker daemon")
}

fn wait_delivery_only_ready(daemon: &mut Child, base: &DaemonBase<'_>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(published) = fs::read_to_string(base.ready) {
            assert_eq!(published.trim(), base.socket.to_str().unwrap());
            return;
        }
        assert!(
            daemon.try_wait().unwrap().is_none(),
            "Delivery-only daemon exited before readiness"
        );
        assert!(
            Instant::now() < deadline,
            "Delivery-only daemon readiness timed out"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn stop_delivery_only_daemon(daemon: &mut Child, base: &DaemonBase<'_>) {
    kill_process(Pid::from_child(daemon), Signal::INT).expect("signal Delivery-only daemon");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = daemon.try_wait().expect("poll Delivery-only daemon") {
            assert!(
                status.success(),
                "Delivery-only daemon shutdown failed: {status}"
            );
            break;
        }
        if Instant::now() >= deadline {
            daemon.kill().expect("kill wedged Delivery-only daemon");
            daemon.wait().expect("reap wedged Delivery-only daemon");
            panic!("Delivery-only daemon did not stop");
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!base.socket.exists());
    assert!(!base.chat_socket.exists());
    assert!(!base.ready.exists());
}

fn daemon_command(paths: &DaemonPaths<'_>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lez-maker-daemon"));
    command
        .arg("--socket")
        .arg(paths.socket)
        .arg("--chat-socket")
        .arg(paths.chat_socket)
        .arg("--database")
        .arg(paths.database)
        .arg("--ready-file")
        .arg(paths.ready)
        .arg("--delivery-directory")
        .arg(paths.delivery)
        .arg("--delivery-signing-key-file")
        .arg(paths.delivery_key)
        .arg("--btc-maker-signing-key-file")
        .arg(paths.maker_agreement_key)
        .arg("--btc-source-maker-config")
        .arg(paths.source_maker_config)
        .arg("--btc-maker-actor-root")
        .arg(paths.maker_actor_root)
        .arg("--btc-actor-program")
        .arg(paths.actor_program)
        .arg("--btc-actor-program-sha256")
        .arg(paths.actor_program_sha256);
    command
}

fn start_daemon(paths: &DaemonPaths<'_>) -> Child {
    daemon_command(paths)
        .spawn()
        .expect("start isolated BTC Maker daemon")
}

fn wait_ready(daemon: &mut Child, paths: &DaemonPaths<'_>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(published) = fs::read_to_string(paths.ready) {
            assert_eq!(published.trim(), paths.socket.to_str().unwrap());
            return;
        }
        assert!(daemon.try_wait().unwrap().is_none(), "daemon exited early");
        assert!(Instant::now() < deadline, "daemon readiness timed out");
        thread::sleep(Duration::from_millis(20));
    }
}

fn stop_daemon(daemon: &mut Child, paths: &DaemonPaths<'_>) {
    kill_process(Pid::from_child(daemon), Signal::INT).expect("signal Maker daemon");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = daemon.try_wait().expect("poll Maker daemon") {
            assert!(status.success(), "Maker daemon shutdown failed: {status}");
            break;
        }
        if Instant::now() >= deadline {
            daemon.kill().expect("kill wedged Maker daemon");
            daemon.wait().expect("reap wedged Maker daemon");
            panic!("Maker daemon did not stop");
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!paths.socket.exists());
    assert!(!paths.chat_socket.exists());
    assert!(!paths.ready.exists());
}

fn make_private_directory(path: &Path) {
    if path.exists() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    } else {
        fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    }
}

fn write_raw_key(path: &Path, byte: u8) {
    write_private(path, &[byte; 32]);
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn request(value: &str) -> RequestId {
    RequestId::new(value).unwrap()
}

fn chat_request(reservation_id: &RequestId, label: &[u8]) -> RequestId {
    let mut digest = Sha256::new();
    digest.update(b"lez-atomic-swaps/btc-taker-chat-request/v1\0");
    digest.update(reservation_id.as_str().as_bytes());
    digest.update([0]);
    digest.update(label);
    RequestId::new(hex::encode(digest.finalize())).unwrap()
}

fn key(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).unwrap()
}

fn public_key(secret: &SecretKey) -> PublicKey {
    PublicKey::from_secret_key(&Secp256k1::signing_only(), secret)
}

fn inode(path: &Path) -> u64 {
    fs::symlink_metadata(path).unwrap().ino()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
