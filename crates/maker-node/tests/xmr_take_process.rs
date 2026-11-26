//! Exercises a Monero take between the two Nodes; compiled only with `pair-xmr`.
#![cfg(feature = "pair-xmr")]
#![cfg(any(target_os = "linux", target_os = "macos"))]

//! A real Maker daemon and the real Taker service agree one Monero swap at
//! take time: `taker_swap_initiate_v1` drives `xmr_reserve_v1` →
//! `xmr_chat_stage_a_v1` → `xmr_ceremony_reserve_v1` → `xmr_ceremony_nonce_v1`
//! → `xmr_ceremony_partial_v1` → `xmr_chat_activate_v1` over the Maker's
//! local Chat socket, and both Nodes provision their actors. monerod, each
//! swap's LEZ sidecar and the Stage-A planner are the reservation fixture's
//! fakes (the real planner is built in the separate sidecar workspace, which
//! this crate cannot reach): the take reads no chain beyond monerod's genesis
//! and tip.

#[path = "support/xmr_reserve_fixture.rs"]
mod xmr_reserve_fixture;

use std::{
    fs::{self, OpenOptions},
    io::{self, Read as _, Write as _},
    net::Shutdown,
    os::unix::{
        fs::OpenOptionsExt as _,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use lez_bridge_protocol::RequestId;
use lez_btc_role_lifecycle::swap_run_id;
use lez_maker_node::{
    DeliveryOfferQueryV1, LocalPriceSetRequest, PairConfigureRequest, RunLocalDelivery,
    TakerMakerIdentityV1, XmrChatActivateRequestV1, XmrChatActivateResponseV1,
    XmrChatStageARequestV1, XmrChatStageAResponseV1, call_local_chat_rpc, call_local_rpc,
};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{
    LocalPriceV1, MakerActorKindV1, MakerOfferId, MakerPairConfigurationV1, MakerPriceSourceKind,
    MakerRouteV1, SqliteSwapStore, SqliteTakerFacadeStore,
};
use lez_taker_node::{
    TakerInitiationCommitV1, TakerSwapInitiateRequestV1, TakerSwapStateV1,
    acceptance_files::derived_request_id, load_taker_service_context, taker_service_rpc_module,
};
use rustix::process::{Pid, Signal, kill_process};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use tempfile::tempdir;
use xmr_reference_actor::{
    ActorRole, XMR_STAGE_A_FILE, XMR_STAGE_B_FILE, load_validated_xmr_effect_manifest_v3_bytes,
};
use xmr_reserve_fixture::{
    XmrMakerDeployment, btc_role_config, fake_planner_program, fake_sidecar_program, private_dir,
    start_fake_monerod, xmr_role_config,
};

const FOREIGN_UNITS: u64 = 1_000_000_000_000;
/// One LEZ unit per 1,000,000,000 piconero.
const LEZ_UNITS: u128 = 1_000;
const DELIVERY_KEY: u8 = 8;
const TAKER_SIGNER: u8 = 0x78;

/// The Maker daemon, stopped when the test ends however it ends.
struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One take, its replays and both roles' state are one audit surface.
async fn the_taker_node_takes_a_monero_offer_from_the_maker_node_in_six_rounds() {
    let run = tempdir().unwrap();
    let root = fs::canonicalize(run.path()).unwrap();
    let monerod = start_fake_monerod();
    let alive = root.join("sidecars-alive");
    fs::write(&alive, b"").unwrap();
    let port_base = 20_000 + u16::try_from(std::process::id() % 400).unwrap() * 50 + 30;

    // The Maker, with its Monero flags and one live Monero offer.
    let maker_home = private_dir(&root.join("maker"));
    let runtime = private_dir(&maker_home.join("run"));
    let socket = runtime.join("maker.sock");
    let chat_socket = runtime.join("chat.sock");
    let ready = runtime.join("ready");
    let delivery = maker_home.join("delivery");
    write_private(
        &maker_home.join("delivery-signing.key"),
        &[DELIVERY_KEY; 32],
    );
    let deployment = XmrMakerDeployment::new(
        &maker_home,
        &alive,
        &monerod,
        &fs::canonicalize("/usr/bin/true").unwrap(),
        port_base,
    );
    let mut daemon = spawn(&maker_home, &socket, &chat_socket, &ready, &deployment);
    let route = MakerRouteV1::new(Pair::Monero, SwapDirection::TakerSellsLez).unwrap();
    configure_live_route(&socket, route).await;
    let offer_id = MakerOfferId::new("xmr-take-offer-001").unwrap();
    let published = Command::new(env!("CARGO_BIN_EXE_lez-maker-cli"))
        .args(["--socket", path(&socket), "publish-offer"])
        .args(["--request-id", "xmr-take-publish-001"])
        .args(["--offer-id", offer_id.as_str(), "--pair", "monero"])
        .args(["--direction", "taker-sells-lez"])
        .output()
        .unwrap();
    assert!(
        published.status.success(),
        "publish-offer: {}",
        String::from_utf8_lossy(&published.stderr)
    );

    // The Taker: its own LEZ role and Monero role, the same monerod and the
    // same fixture planner program, no prepared catalog.
    let taker_home = private_dir(&root.join("taker"));
    let taker_tools = private_dir(&taker_home.join("xmr-tools"));
    let taker_btc = btc_role_config(
        &taker_home,
        &fake_sidecar_program(&taker_tools, &alive),
        port_base + 10,
        TAKER_SIGNER,
    );
    let taker_xmr = xmr_role_config(
        &taker_home,
        &monerod,
        &fake_planner_program(&taker_tools),
        "taker",
    );
    // The Taker reaches the Maker's Chat socket through a proxy that records
    // which methods it sends.
    let proxy = ChatProxy::start(&taker_home.join("chat-proxy.sock"), &chat_socket);
    let registry = taker_home.join("registry.sqlite3");
    drop(SqliteTakerFacadeStore::create_new(&registry).unwrap());
    let maker_key = maker_key();
    let service_config = taker_home.join("role.json");
    write_private(
        &service_config,
        &serde_json::to_vec(&json!({
            "schema_version": 1,
            "delivery_sources": [{
                "source_id": "local-maker",
                "directory": delivery,
                "maker_public_key": hex::encode(maker_key.serialize()),
            }],
            "chat_socket": proxy.socket,
            "maximum_offers": 16,
            "initiation": {
                "execute_prepared": true,
                "registry_database": registry,
                "btc_role_config": taker_btc,
                "xmr_role_config": taker_xmr,
            }
        }))
        .unwrap(),
    );
    let offer = RunLocalDelivery::subscriber(&delivery, maker_key)
        .unwrap()
        .discover(&DeliveryOfferQueryV1::for_route(route, now()))
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the published offer is discoverable");
    let request = TakerSwapInitiateRequestV1 {
        schema_version: 1,
        request_id: RequestId::new("xmr-take-initiation-001").unwrap(),
        offer_id: offer_id.clone(),
        route,
        maker_identity: TakerMakerIdentityV1::new(maker_key.serialize()).unwrap(),
        signed_envelope_sha256: offer.commitment(),
        foreign_units: FOREIGN_UNITS,
        expected_lez_units: LEZ_UNITS,
        logos_offer_announcement_base64: None,
        funding_transaction_hex: None,
    };

    // The take: six rounds, then the Taker provisions its actor.
    let module =
        taker_service_rpc_module(load_taker_service_context(&service_config).unwrap()).unwrap();
    let started = Instant::now();
    let commit: TakerInitiationCommitV1 = module
        .call("taker_swap_initiate_v1", [request.clone()])
        .await
        .expect("the take reserves, runs the ceremony and activates over Chat");
    eprintln!("taker_swap_initiate_v1 answered in {:?}", started.elapsed());
    assert!(!commit.was_replay);
    assert_eq!(commit.swap.state, TakerSwapStateV1::NotActivated);
    assert_eq!(commit.swap.route, route);
    assert_eq!(commit.swap.foreign_units, FOREIGN_UNITS);
    assert_eq!(commit.swap.lez_units, LEZ_UNITS);
    assert_eq!(
        proxy.take(),
        [
            "xmr_reserve_v1",
            "xmr_chat_stage_a_v1",
            "xmr_ceremony_reserve_v1",
            "xmr_ceremony_nonce_v1",
            "xmr_ceremony_partial_v1",
            "xmr_chat_activate_v1",
        ],
        "the take is the six rounds, each sent once"
    );
    let swap_id = commit.swap.swap_id.clone();

    // The Taker's swap: its receipt names the actor it provisioned.
    let reservation_id =
        derived_request_id(&request.request_id, "xmr", b"node-reservation").unwrap();
    let taker_swap = taker_home.join("swaps").join(reservation_id.as_str());
    let receipt: Value =
        serde_json::from_slice(&fs::read(taker_swap.join("xmr-receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["pair"], "monero");
    assert_eq!(receipt["swap_id"], swap_id.as_str());
    assert_eq!(receipt["reservation_id"], reservation_id.as_str());
    assert!(
        Path::new(receipt["actor_manifest_file"].as_str().unwrap()).is_file(),
        "the receipt's actor manifest exists"
    );

    // The Maker's swap: the same reservation, its actor registered with a
    // manifest that validates as a schema-3 effect authority.
    let maker_swap = deployment.swap(&reservation_id);
    let manifest_file = maker_swap.join("xmr-effect/actor-manifest.json");
    let _ = load_validated_xmr_effect_manifest_v3_bytes(
        &fs::read(&manifest_file).unwrap(),
        &fs::read(maker_swap.join("xmr-effect/effect-authority.json")).unwrap(),
        ActorRole::Maker,
        swap_run_id(&reservation_id).unwrap().as_str(),
    )
    .expect("the Maker's registered manifest is a complete schema-3 effect authority");
    let actors = SqliteSwapStore::open(maker_home.join("maker.sqlite3"))
        .unwrap()
        .list_maker_actor_processes()
        .unwrap();
    assert_eq!(actors.len(), 1);
    assert_eq!(actors[0].manifest().kind(), MakerActorKindV1::Monero);
    assert_eq!(actors[0].manifest().swap_id().as_str(), swap_id.as_str());
    assert_eq!(actors[0].manifest().config_path(), manifest_file);

    // Both roles hold byte-identical Stage A and Stage B.
    let stage_a = fs::read(taker_swap.join("xmr").join(XMR_STAGE_A_FILE)).unwrap();
    let stage_b = fs::read(taker_swap.join("xmr").join(XMR_STAGE_B_FILE)).unwrap();
    assert_eq!(
        fs::read(maker_swap.join("xmr").join(XMR_STAGE_A_FILE)).unwrap(),
        stage_a
    );
    assert_eq!(
        fs::read(maker_swap.join("xmr").join(XMR_STAGE_B_FILE)).unwrap(),
        stage_b
    );

    // The Maker answers the Taker's R2 and R6, resent as the Taker sends
    // them, as replays.
    let staged: XmrChatStageAResponseV1 = call_local_chat_rpc(
        &chat_socket,
        "xmr_chat_stage_a_v1",
        &XmrChatStageARequestV1 {
            schema_version: 1,
            request_id: derived_request_id(&reservation_id, "xmr", b"stage-a").unwrap(),
            offer_id: offer_id.clone(),
            expected_offer_revision: 1,
            reservation_id: reservation_id.clone(),
            foreign_units: FOREIGN_UNITS,
            signed_offer_envelope: offer.signed_envelope().to_vec(),
            stage_a_wire: stage_a.clone(),
        },
    )
    .await
    .expect("the Maker answers a resent R2");
    assert!(staged.was_replay);
    assert_eq!(staged.swap_id.as_ref(), swap_id.as_str());
    let activated: XmrChatActivateResponseV1 = call_local_chat_rpc(
        &chat_socket,
        "xmr_chat_activate_v1",
        &XmrChatActivateRequestV1 {
            schema_version: 1,
            request_id: derived_request_id(&reservation_id, "xmr", b"activate").unwrap(),
            offer_id: offer_id.clone(),
            expected_offer_revision: 2,
            reservation_id: reservation_id.clone(),
            activation_wire: stage_b.clone(),
        },
    )
    .await
    .expect("the Maker answers a resent R6");
    assert!(activated.was_replay);
    assert_eq!(activated.swap_id.as_ref(), swap_id.as_str());

    // A restarted Taker replays the take from its durable state: it resends
    // none of R1–R5, only the activation, which the Maker answers as a
    // replay (the prepared take's last step, as for Bitcoin and Zcash).
    drop(module);
    let module =
        taker_service_rpc_module(load_taker_service_context(&service_config).unwrap()).unwrap();
    let replay: TakerInitiationCommitV1 = module
        .call("taker_swap_initiate_v1", [request.clone()])
        .await
        .expect("a restarted Taker replays the take");
    assert!(replay.was_replay);
    assert_eq!(replay.swap.swap_id, swap_id);
    assert_eq!(proxy.take(), ["xmr_chat_activate_v1"]);
    assert_eq!(
        fs::read(taker_swap.join("xmr").join(XMR_STAGE_B_FILE)).unwrap(),
        stage_b
    );
    assert_eq!(
        SqliteSwapStore::open(maker_home.join("maker.sqlite3"))
            .unwrap()
            .list_maker_actor_processes()
            .unwrap()
            .len(),
        1
    );
    drop(module);
    stop(&mut daemon);
    fs::remove_file(&alive).unwrap();
}

/// A Unix-socket proxy in front of the Maker's Chat socket that records the
/// method of every request it forwards, in order.
struct ChatProxy {
    socket: PathBuf,
    methods: Arc<Mutex<Vec<String>>>,
}

impl ChatProxy {
    fn start(socket: &Path, upstream: &Path) -> Self {
        let listener = UnixListener::bind(socket).unwrap();
        let methods = Arc::new(Mutex::new(Vec::new()));
        let (recorded, upstream) = (Arc::clone(&methods), upstream.to_path_buf());
        thread::spawn(move || {
            for client in listener.incoming().flatten() {
                let (recorded, upstream) = (Arc::clone(&recorded), upstream.clone());
                thread::spawn(move || forward(client, &upstream, &recorded));
            }
        });
        Self {
            socket: socket.to_path_buf(),
            methods,
        }
    }

    /// The methods forwarded since the last call.
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.methods.lock().unwrap())
    }
}

fn forward(mut client: UnixStream, upstream: &Path, recorded: &Mutex<Vec<String>>) {
    let Ok(mut server) = UnixStream::connect(upstream) else {
        return;
    };
    let (mut to_client, mut from_server) =
        (client.try_clone().unwrap(), server.try_clone().unwrap());
    let answers = thread::spawn(move || {
        let _ = io::copy(&mut from_server, &mut to_client);
        let _ = to_client.shutdown(Shutdown::Write);
    });
    let (mut request, mut buffer, mut method) = (Vec::new(), [0_u8; 16_384], None);
    while let Ok(count) = client.read(&mut buffer) {
        if count == 0 || server.write_all(&buffer[..count]).is_err() {
            break;
        }
        if method.is_none() {
            request.extend_from_slice(&buffer[..count]);
            method = String::from_utf8_lossy(&request)
                .split_once("\"method\":\"")
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(name, _)| name.to_owned());
            if let Some(name) = &method {
                recorded.lock().unwrap().push(name.clone());
            }
        }
    }
    let _ = server.shutdown(Shutdown::Write);
    let _ = answers.join();
}

fn spawn(
    home: &Path,
    socket: &Path,
    chat_socket: &Path,
    ready: &Path,
    deployment: &XmrMakerDeployment,
) -> Daemon {
    let _ = fs::remove_file(ready);
    let mut daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_lez-maker-node"))
            .args(["--socket", path(socket), "--chat-socket", path(chat_socket)])
            .args(["--database", path(&home.join("maker.sqlite3"))])
            .args(["--ready-file", path(ready)])
            .args(["--delivery-directory", path(&home.join("delivery"))])
            .args([
                "--delivery-signing-key-file",
                path(&home.join("delivery-signing.key")),
            ])
            .args(deployment.arguments())
            .spawn()
            .expect("start the Maker daemon"),
    );
    let deadline = Instant::now() + Duration::from_mins(1);
    while !ready.exists() {
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "the Maker exited early"
        );
        assert!(Instant::now() < deadline, "the Maker did not become ready");
        thread::sleep(Duration::from_millis(20));
    }
    daemon
}

fn stop(daemon: &mut Daemon) {
    kill_process(Pid::from_child(&daemon.0), Signal::INT).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while daemon.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "the Maker did not stop");
        thread::sleep(Duration::from_millis(20));
    }
}

async fn configure_live_route(socket: &Path, route: MakerRouteV1) {
    for (request_id, enabled, revision) in [
        ("xmr-take-route-create-001", false, None),
        ("xmr-take-route-enable-001", true, Some(1)),
    ] {
        if enabled {
            let _: Value = call_local_rpc(
                socket,
                "maker_local_price_set",
                &LocalPriceSetRequest {
                    request_id: RequestId::new("xmr-take-price-create-001").unwrap(),
                    expected_revision: None,
                    price: LocalPriceV1::new(route, 1, 1_000_000_000).unwrap(),
                },
            )
            .await
            .unwrap();
        }
        let _: Value = call_local_rpc(
            socket,
            "maker_pair_configure",
            &PairConfigureRequest {
                request_id: RequestId::new(request_id).unwrap(),
                expected_revision: revision,
                configuration: MakerPairConfigurationV1::new(
                    route,
                    enabled,
                    MakerPriceSourceKind::Local,
                    FOREIGN_UNITS,
                    FOREIGN_UNITS,
                    300,
                )
                .unwrap(),
            },
        )
        .await
        .unwrap();
    }
}

fn maker_key() -> PublicKey {
    PublicKey::from_secret_key(
        &Secp256k1::signing_only(),
        &SecretKey::from_slice(&[DELIVERY_KEY; 32]).unwrap(),
    )
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

fn path(value: &Path) -> &str {
    value.to_str().unwrap()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
