//! Actual-process contract for the owner-only read-only Taker service.

use std::{
    env, fs,
    io::Write as _,
    os::unix::{
        fs::{
            DirBuilderExt as _, FileTypeExt as _, MetadataExt as _, OpenOptionsExt as _,
            PermissionsExt as _,
        },
        net::UnixListener,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use lez_maker_node::{
    TakerDependencyStateV1, TakerHealthRequestV1, TakerHealthV1, TakerOfferListRequestV1,
    TakerOfferListV1, TakerSwapListRequestV1, TakerSwapListV1, TakerSwapMonitorRequestV1,
    TakerSwapStateV1, TakerSwapViewV1, call_local_rpc,
};
use rustix::process::{Pid, Signal, kill_process};
use serde_json::{Value, json};

use std::time::{SystemTime, UNIX_EPOCH};

use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    DeliveryPublicationV1, RunLocalDelivery, TakerInitiationCommitV1, TakerMakerIdentityV1,
    TakerSwapInitiateRequestV1,
};
use lez_swap_core::{Pair, SwapDirection, SwapId};
use lez_swap_store::{
    LocalPriceV1, MakerOfferId, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    SqliteSwapStore, SqliteTakerFacadeStore,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use sha2::{Digest as _, Sha256};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_service_serves_only_read_methods_and_cleans_exact_socket_on_sigterm() {
    let run = tempfile::tempdir().unwrap();
    let runtime = private_directory(run.path().join("runtime"));
    let config = private_config(run.path().join("taker-service.json"), &valid_config());
    let socket = runtime.join("taker.sock");
    let mut service = TestService::spawn(&config, &socket);
    wait_until_ready(&mut service, &socket).await;

    let metadata = fs::symlink_metadata(&socket).unwrap();
    assert!(metadata.file_type().is_socket());
    assert_eq!(metadata.mode() & 0o7777, 0o600);

    let health: TakerHealthV1 = call_local_rpc(
        &socket,
        "taker_health",
        &TakerHealthRequestV1 { schema_version: 1 },
    )
    .await
    .unwrap();
    assert!(health.is_ready());
    assert_eq!(health.delivery(), TakerDependencyStateV1::Disabled);
    assert_eq!(health.chat(), TakerDependencyStateV1::Disabled);
    let methods = health.registered_methods();
    assert!(methods.health());
    assert!(methods.offer_list());
    assert!(!methods.swap_list());
    assert!(!methods.initiate());
    assert!(!methods.monitor());
    assert!(!methods.claim());
    assert!(!methods.refund());

    let offers: TakerOfferListV1 = call_local_rpc(
        &socket,
        "taker_offer_list_v1",
        &TakerOfferListRequestV1 {
            schema_version: 1,
            route: None,
        },
    )
    .await
    .unwrap();
    assert!(offers.offers.is_empty());

    for method in [
        "maker_health",
        "taker_swap_list_v1",
        "taker_swap_initiate_v1",
        "taker_swap_monitor_v1",
        "taker_swap_claim_v1",
        "taker_swap_refund_v1",
    ] {
        let error = call_local_rpc::<_, Value>(&socket, method, &json!({"schema_version": 1}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("-32601"), "{error:#}");
    }

    assert!(service.terminate().await.success());
    assert!(!socket.exists(), "SIGTERM left the owned socket path");

    let mut restarted = TestService::spawn(&config, &socket);
    wait_until_ready(&mut restarted, &socket).await;
    let original = socket_identity(&socket);

    fs::remove_file(&socket).unwrap();
    let replacement = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let replacement_identity = socket_identity(&socket);
    assert_ne!(replacement_identity, original);

    assert!(restarted.terminate().await.success());
    assert_eq!(
        socket_identity(&socket),
        replacement_identity,
        "service removed a replacement socket with a different inode"
    );
    drop(replacement);
    fs::remove_file(&socket).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_initiation_survives_process_restart_without_live_delivery() {
    let fixture = ProcessInitiationFixture::new();
    let runtime = private_directory(fixture.root.join("runtime"));
    let socket = runtime.join("taker.sock");
    let mut service = TestService::spawn(&fixture.config, &socket);
    wait_until_ready(&mut service, &socket).await;

    let health: TakerHealthV1 = call_local_rpc(
        &socket,
        "taker_health",
        &TakerHealthRequestV1 { schema_version: 1 },
    )
    .await
    .unwrap();
    let methods = health.registered_methods();
    assert!(methods.health());
    assert!(methods.offer_list());
    assert!(methods.initiate());
    assert!(methods.swap_list());
    assert!(methods.monitor());
    assert!(methods.claim());
    assert!(methods.refund());

    let request = fixture.request();
    let first: TakerInitiationCommitV1 =
        call_local_rpc(&socket, "taker_swap_initiate_v1", &request)
            .await
            .unwrap();
    assert!(!first.was_replay);
    assert_eq!(first.swap.swap_id.as_str(), "m6-process-zec-swap-001");
    assert_eq!(first.swap.offer_id, request.offer_id);
    assert_eq!(first.swap.foreign_units, 42);
    assert_eq!(first.swap.lez_units, 84);
    assert_eq!(first.swap.state, TakerSwapStateV1::Initiating);
    assert_eq!(first.swap.progress_generation, 0);
    assert_eq!(first.swap.available_action, None);
    assert_eq!(first.swap.privacy_guidance, None);
    assert_eq!(
        SqliteTakerFacadeStore::open_existing(&fixture.registry)
            .unwrap()
            .lookup_initiation(&request.request_id)
            .unwrap()
            .unwrap()
            .swap_id()
            .as_str(),
        "m6-process-zec-swap-001"
    );
    assert_initiating_reads_are_public_and_effect_free(&socket, &first.swap, &fixture).await;

    assert!(service.terminate().await.success());
    assert!(!socket.exists());
    fs::remove_file(&fixture.delivery_offer).unwrap();

    let mut restarted = TestService::spawn(&fixture.config, &socket);
    wait_until_ready(&mut restarted, &socket).await;
    let restarted_health: TakerHealthV1 = call_local_rpc(
        &socket,
        "taker_health",
        &TakerHealthRequestV1 { schema_version: 1 },
    )
    .await
    .unwrap();
    let restarted_methods = restarted_health.registered_methods();
    assert!(restarted_methods.health());
    assert!(restarted_methods.offer_list());
    assert!(restarted_methods.initiate());
    assert!(restarted_methods.swap_list());
    assert!(restarted_methods.monitor());
    assert!(restarted_methods.claim());
    assert!(restarted_methods.refund());
    assert_initiating_reads_are_public_and_effect_free(&socket, &first.swap, &fixture).await;
    let replay: TakerInitiationCommitV1 =
        call_local_rpc(&socket, "taker_swap_initiate_v1", &request)
            .await
            .expect("exact replay must precede live Delivery after restart");
    assert!(replay.was_replay);
    assert_eq!(replay.swap, first.swap);
    assert!(restarted.terminate().await.success());
    assert!(!socket.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the external pinned-Basecamp product driver"]
async fn basecamp_prepared_offer_rendezvous_drives_the_production_taker_service() {
    let rendezvous = PathBuf::from(
        env::var_os("M6_BASECAMP_RENDEZVOUS")
            .expect("M6_BASECAMP_RENDEZVOUS must select a new absolute file"),
    );
    assert!(rendezvous.is_absolute());
    let acknowledgement = rendezvous.with_extension("done");
    assert!(!rendezvous.exists());
    assert!(!acknowledgement.exists());

    let fixture = ProcessInitiationFixture::new();
    let runtime = private_directory(fixture.root.join("basecamp-runtime"));
    let socket = runtime.join("taker.sock");
    let mut service = TestService::spawn(&fixture.config, &socket);
    wait_until_ready(&mut service, &socket).await;

    let request = fixture.request();
    let public_fixture = json!({
        "schema_version": 1,
        "socket": socket,
        "offer_id": request.offer_id,
        "maker_identity": hex::encode(fixture.maker),
        "signed_envelope_sha256": hex::encode(fixture.commitment),
        "foreign_units": request.foreign_units,
        "expected_lez_units": request.expected_lez_units,
        "swap_id": "m6-process-zec-swap-001",
    });
    let mut handoff = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&rendezvous)
        .unwrap();
    handoff
        .write_all(&serde_json::to_vec(&public_fixture).unwrap())
        .unwrap();
    handoff.sync_all().unwrap();

    let deadline = Instant::now() + Duration::from_secs(300);
    while !acknowledgement.exists() {
        assert!(
            service.child_mut().try_wait().unwrap().is_none(),
            "Taker service exited while Basecamp held the rendezvous"
        );
        assert!(Instant::now() < deadline, "Basecamp rendezvous timed out");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(fs::read_to_string(&acknowledgement).unwrap(), "ok\n");

    let committed = SqliteTakerFacadeStore::open_existing(&fixture.registry)
        .unwrap()
        .lookup_initiation(&RequestId::new("taker-ui-initiate-001").unwrap())
        .unwrap()
        .expect("Basecamp must commit the prepared initiation");
    assert_eq!(committed.swap_id().as_str(), "m6-process-zec-swap-001");
    assert!(service.terminate().await.success());
    assert!(!socket.exists());
}

async fn assert_initiating_reads_are_public_and_effect_free(
    socket: &Path,
    expected: &TakerSwapViewV1,
    fixture: &ProcessInitiationFixture,
) {
    let listed: TakerSwapListV1 = call_local_rpc(
        socket,
        "taker_swap_list_v1",
        &TakerSwapListRequestV1 { schema_version: 1 },
    )
    .await
    .unwrap();
    assert_eq!(listed.schema_version, 1);
    assert_eq!(listed.swaps.as_slice(), std::slice::from_ref(expected));

    let monitored: TakerSwapViewV1 = call_local_rpc(
        socket,
        "taker_swap_monitor_v1",
        &TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: expected.swap_id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(&monitored, expected);
    assert_eq!(monitored.state, TakerSwapStateV1::Initiating);
    assert_eq!(monitored.progress_generation, 0);
    assert_eq!(monitored.available_action, None);
    assert_eq!(monitored.privacy_guidance, None);

    let mut terminal_errors = Vec::new();
    for (method, request_id) in [
        ("taker_swap_claim_v1", "m6-process-claim-without-receipt"),
        ("taker_swap_refund_v1", "m6-process-refund-without-receipt"),
    ] {
        let error = call_local_rpc::<_, Value>(
            socket,
            method,
            &json!({
                "schema_version": 1,
                "request_id": request_id,
                "swap_id": expected.swap_id.clone(),
                "expected_generation": expected.progress_generation,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "local RPC error -32010: Taker dependency unavailable"
        );
        terminal_errors.push(error);
    }

    let unknown = call_local_rpc::<_, TakerSwapViewV1>(
        socket,
        "taker_swap_monitor_v1",
        &TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: SwapId::new("m6-process-unknown-swap-001").unwrap(),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        unknown.to_string(),
        "local RPC error -32014: Taker swap not found"
    );

    let public_wire = format!(
        "{} {} {} {terminal_errors:?}",
        serde_json::to_string(&listed).unwrap(),
        serde_json::to_string(&monitored).unwrap(),
        unknown
    );
    for private_marker in [
        fixture.root.display().to_string(),
        "m6-process-zec-reservation-001".to_owned(),
        "signed.json".to_owned(),
        "draft.json".to_owned(),
        "key.bin".to_owned(),
        "actor.json".to_owned(),
        "process-draft".to_owned(),
        "process-taker".to_owned(),
        hex::encode([42; 32]),
    ] {
        assert!(
            !public_wire.contains(&private_marker),
            "private initiation material leaked in {public_wire}"
        );
    }

    for absent_effect in [
        fixture.root.join("agreement.json"),
        fixture.root.join("actor-root"),
        fixture.root.join("receipt.json"),
    ] {
        assert!(
            !absent_effect.exists(),
            "read-only Initiating projection created {}",
            absent_effect.display()
        );
    }
}

#[test]
fn startup_requires_private_config_and_rejects_invalid_or_relative_socket_before_bind() {
    let run = tempfile::tempdir().unwrap();
    let runtime = private_directory(run.path().join("runtime"));

    let help = Command::new(env!("CARGO_BIN_EXE_lez-taker-service"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("--config"));
    assert!(help.contains("/run/lez-atomic-swaps/taker.sock"));

    let missing_config_socket = runtime.join("missing-config.sock");
    let missing = Command::new(env!("CARGO_BIN_EXE_lez-taker-service"))
        .arg("--socket")
        .arg(&missing_config_socket)
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(!missing_config_socket.exists());

    let invalid = private_config(
        run.path().join("invalid.json"),
        &json!({
            "schema_version": 2,
            "delivery_sources": [],
            "maximum_offers": 16,
        }),
    );
    let invalid_socket = runtime.join("invalid.sock");
    let failed = Command::new(env!("CARGO_BIN_EXE_lez-taker-service"))
        .arg("--config")
        .arg(&invalid)
        .arg("--socket")
        .arg(&invalid_socket)
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(!invalid_socket.exists());

    let valid = private_config(run.path().join("valid.json"), &valid_config());
    let relative = Command::new(env!("CARGO_BIN_EXE_lez-taker-service"))
        .current_dir(run.path())
        .arg("--config")
        .arg(&valid)
        .arg("--socket")
        .arg("relative.sock")
        .output()
        .unwrap();
    assert!(!relative.status.success());
    assert!(!run.path().join("relative.sock").exists());
}

struct ProcessInitiationFixture {
    _run: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
    registry: PathBuf,
    delivery_offer: PathBuf,
    route: MakerRouteV1,
    maker: [u8; 33],
    commitment: [u8; 32],
}

impl ProcessInitiationFixture {
    fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let run = tempfile::tempdir().unwrap();
        let root = private_directory(run.path().join("initiation-fixture"));
        let delivery = private_directory(root.join("delivery"));
        let registry = root.join("registry.sqlite3");
        drop(SqliteTakerFacadeStore::create_new(&registry).unwrap());

        let maker_secret = SecretKey::from_slice(&[17; 32]).unwrap();
        let maker =
            PublicKey::from_secret_key(&Secp256k1::signing_only(), &maker_secret).serialize();
        let route = MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).unwrap();
        let offer = process_prepared_offer(&root, route, now);
        let publisher = RunLocalDelivery::publisher(delivery.clone(), maker_secret).unwrap();
        let authenticated = publisher
            .publish_or_verify(&DeliveryPublicationV1::new(offer, now))
            .unwrap();
        let commitment = authenticated.commitment();
        let delivery_offer = delivery.join("m6-process-zec-offer-001.offer.json");

        let signed =
            process_private_file(root.join("signed.json"), authenticated.signed_envelope());
        let draft =
            process_private_file(root.join("draft.json"), br#"{"unsigned":"process-draft"}"#);
        let key = process_private_file(root.join("key.bin"), &[42; 32]);
        let actor = process_private_file(root.join("actor.json"), br#"{"role":"process-taker"}"#);
        let config = private_config(
            root.join("service.json"),
            &json!({
                "schema_version": 1,
                "delivery_sources": [{
                    "source_id": "process-maker",
                    "directory": delivery,
                    "maker_public_key": hex::encode(maker),
                }],
                "chat_socket": root.join("chat.sock"),
                "maximum_offers": 16,
                "initiation": {
                    "registry_database": registry,
                    "prepared_zec": [{
                        "source_id": "process-maker",
                        "swap_id": "m6-process-zec-swap-001",
                        "offer_id": "m6-process-zec-offer-001",
                        "reservation_id": "m6-process-zec-reservation-001",
                        "foreign_units": 42,
                        "lez_units": 84,
                        "signed_envelope": process_digest_binding(&signed),
                        "unsigned_draft": process_digest_binding(&draft),
                        "signing_key": {"path": key},
                        "source_config": process_digest_binding(&actor),
                        "agreement_output": root.join("agreement.json"),
                        "actor_root": root.join("actor-root"),
                        "receipt_output": root.join("receipt.json"),
                    }]
                }
            }),
        );

        Self {
            _run: run,
            root,
            config,
            registry,
            delivery_offer,
            route,
            maker,
            commitment,
        }
    }

    fn request(&self) -> TakerSwapInitiateRequestV1 {
        TakerSwapInitiateRequestV1 {
            schema_version: 1,
            request_id: RequestId::new("m6-process-initiation-request-001").unwrap(),
            offer_id: MakerOfferId::new("m6-process-zec-offer-001").unwrap(),
            route: self.route,
            maker_identity: TakerMakerIdentityV1::new(self.maker).unwrap(),
            signed_envelope_sha256: self.commitment,
            foreign_units: 42,
            expected_lez_units: 84,
        }
    }
}

fn process_prepared_offer(
    root: &Path,
    route: MakerRouteV1,
    now: u64,
) -> lez_swap_store::MakerOfferV1 {
    let mut store = SqliteSwapStore::open(root.join("offer.sqlite3")).unwrap();
    let disabled =
        MakerPairConfigurationV1::new(route, false, MakerPriceSourceKind::Local, 1, 10_000, 300)
            .unwrap();
    store
        .configure_maker_pair(
            &RequestId::new("m6-process-pair-create").unwrap(),
            None,
            &disabled,
        )
        .unwrap();
    store
        .set_local_price(
            &RequestId::new("m6-process-price-create").unwrap(),
            None,
            &LocalPriceV1::new(route, 2, 1).unwrap(),
        )
        .unwrap();
    let enabled =
        MakerPairConfigurationV1::new(route, true, MakerPriceSourceKind::Local, 1, 10_000, 300)
            .unwrap();
    store
        .configure_maker_pair(
            &RequestId::new("m6-process-pair-enable").unwrap(),
            Some(1),
            &enabled,
        )
        .unwrap();
    store
        .publish_local_offer(
            &RequestId::new("m6-process-offer-publish").unwrap(),
            &MakerOfferId::new("m6-process-zec-offer-001").unwrap(),
            route,
            now,
        )
        .unwrap();
    store.list_discoverable_maker_offers(now).unwrap()[0]
        .offer()
        .clone()
}

fn process_digest_binding(path: &Path) -> Value {
    json!({
        "path": path,
        "sha256": hex::encode(Sha256::digest(fs::read(path).unwrap())),
    })
}

fn process_private_file(path: PathBuf, bytes: &[u8]) -> PathBuf {
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}

fn valid_config() -> Value {
    json!({
        "schema_version": 1,
        "delivery_sources": [],
        "maximum_offers": 16,
    })
}

fn private_config(path: PathBuf, value: &Value) -> PathBuf {
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}

fn private_directory(path: PathBuf) -> PathBuf {
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    path
}

fn socket_identity(path: &Path) -> (u64, u64) {
    let metadata = fs::symlink_metadata(path).unwrap();
    (metadata.dev(), metadata.ino())
}

struct TestService(Option<Child>);

impl TestService {
    fn spawn(config: &Path, socket: &Path) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_lez-taker-service"))
            .arg("--config")
            .arg(config)
            .arg("--socket")
            .arg(socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self(Some(child))
    }

    fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("Taker service is running")
    }

    async fn terminate(&mut self) -> std::process::ExitStatus {
        let child = self.child_mut();
        kill_process(Pid::from_child(child), Signal::TERM).unwrap();
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                self.0 = None;
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "Taker service exceeded graceful shutdown deadline"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

impl Drop for TestService {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

async fn wait_until_ready(service: &mut TestService, socket: &Path) {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        assert!(
            service.child_mut().try_wait().unwrap().is_none(),
            "Taker service exited before readiness"
        );
        if fs::symlink_metadata(socket).is_ok_and(|metadata| metadata.file_type().is_socket()) {
            let request = TakerHealthRequestV1 { schema_version: 1 };
            if tokio::time::timeout(
                Duration::from_millis(250),
                call_local_rpc::<_, TakerHealthV1>(socket, "taker_health", &request),
            )
            .await
            .is_ok_and(|result| result.is_ok())
            {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "Taker service did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
