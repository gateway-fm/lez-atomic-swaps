//! Black-box acceptance tests at the maker operator process boundary.

mod support;

use std::{
    fs,
    fs::OpenOptions,
    io::Write as _,
    os::unix::fs::{
        DirBuilderExt as _, FileTypeExt as _, OpenOptionsExt as _, PermissionsExt as _,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Output},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use support::actor_deployment;

use lez_maker_node::apply_zcash_funding_event;
use lez_swap_core::{
    Chain, ChainPosition, ChainProof, ConfirmationPolicy, Pair, Participant, RecoverySchedule,
    SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{MakerActorKindV1, MakerActorManifestV1, SqliteSwapStore};
use lez_zec_swap_sdk::{
    Bip199Contract, CanonicalZcashOutputObservation, CanonicalZcashOutputRemoval,
    ExpectedBip199Output, TransparentFundingRequest, TransparentUtxo, ZcashNodeRemovalSnapshot,
    ZcashNodeSnapshot, ZcashObservationEvent, ZcashStableTip, ZecProfileId, ZecSwapBinding,
    build_funding_transaction,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;
use zcash_primitives::block::BlockHash;
use zcash_protocol::{
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{OutPoint, TxOut},
};
use zec_reference_actor::ActorConfig;

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn maker_cli_controls_owner_local_daemon_and_survives_restart() {
    let run = tempdir().expect("isolated test directory");
    let database = run.path().join("maker.sqlite3");

    let (first_daemon, first_socket) = start_daemon(run.path(), &database, "first");
    let runtime = fs::metadata(first_socket.parent().unwrap()).unwrap();
    assert_eq!(runtime.permissions().mode() & 0o7777, 0o700);
    let socket_metadata = fs::symlink_metadata(&first_socket).unwrap();
    assert!(socket_metadata.file_type().is_socket());
    assert_eq!(socket_metadata.permissions().mode() & 0o7777, 0o600);

    configure_zec_route(&first_socket);
    publish_zec_offer(&first_socket);
    assert_delivery_offer(run.path(), true);

    let created = create_swap(&first_socket, "operator-swap-1", "bitcoin", None);
    assert_success(&created);
    assert_swap_view(&created.stdout, "operator-swap-1", "Bitcoin", "Offered");

    let reverse = create_swap(
        &first_socket,
        "operator-swap-reverse",
        "zcash",
        Some("taker-sells-lez"),
    );
    assert_success(&reverse);
    let reverse_view: Value = serde_json::from_slice(&reverse.stdout).expect("CLI emits JSON");
    assert_eq!(reverse_view["direction"], "TakerSellsLez");

    let xmr = create_swap(
        &first_socket,
        "operator-xmr-event-recovery",
        "monero",
        Some("taker-sells-lez"),
    );
    assert_success(&xmr);
    assert_swap_view(
        &xmr.stdout,
        "operator-xmr-event-recovery",
        "Monero",
        "Offered",
    );

    let unsupported_xmr_first = create_swap(
        &first_socket,
        "unsafe-xmr-first",
        "monero",
        Some("taker-sells-foreign"),
    );
    assert!(!unsupported_xmr_first.status.success());
    assert!(
        String::from_utf8_lossy(&unsupported_xmr_first.stderr)
            .contains("does not support direction"),
        "unexpected XMR direction error: {}",
        String::from_utf8_lossy(&unsupported_xmr_first.stderr)
    );

    let denied = maker_cli(
        &run.path().join("not-the-owner-socket"),
        &["status", "--id", "operator-swap-1"],
    );
    assert!(
        !denied.status.success(),
        "wrong socket unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&denied.stderr).contains("connect local RPC socket"),
        "unexpected denial: {}",
        String::from_utf8_lossy(&denied.stderr)
    );

    drop(first_daemon);

    let (_second_daemon, second_socket) = start_daemon(run.path(), &database, "second.ready");
    let recovered = maker_cli(&second_socket, &["status", "--id", "operator-swap-1"]);
    assert_success(&recovered);
    assert_swap_view(&recovered.stdout, "operator-swap-1", "Bitcoin", "Offered");

    let reverse_recovered = maker_cli(&second_socket, &["status", "--id", "operator-swap-reverse"]);
    assert_success(&reverse_recovered);
    let reverse_view: Value =
        serde_json::from_slice(&reverse_recovered.stdout).expect("CLI emits JSON");
    assert_eq!(reverse_view["direction"], "TakerSellsLez");
    assert_route_lists(&second_socket);
    assert_route_quote(&second_socket);
    assert_offer_history(&second_socket, "active", 1);
    assert_delivery_offer(run.path(), true);
    withdraw_zec_offer(&second_socket);
    assert_offer_history(&second_socket, "withdrawn", 2);
    assert_delivery_offer(run.path(), false);
    let history = maker_cli(&second_socket, &["history"]);
    assert_success(&history);
    let history: Value = serde_json::from_slice(&history.stdout).expect("CLI emits history JSON");
    let history = history.as_array().expect("history array");
    assert_eq!(history.len(), 3);
    assert!(history.iter().any(|view| view["id"] == "operator-swap-1"));
    assert!(
        history
            .iter()
            .any(|view| view["id"] == "operator-swap-reverse")
    );
    assert!(
        history
            .iter()
            .any(|view| view["id"] == "operator-xmr-event-recovery")
    );
}

#[test]
fn disabled_route_rejects_quote_and_publication_without_disabling_another_pair() {
    let run = tempdir().expect("isolated test directory");
    let database = run.path().join("disabled-route.sqlite3");
    let (first_daemon, first_socket) =
        start_daemon(run.path(), &database, "disabled-route-first.ready");

    configure_disabled_zec_route(&first_socket);
    configure_enabled_btc_route(&first_socket);

    assert_disabled_route(&first_socket);
    assert_enabled_btc_route(&first_socket);
    drop(first_daemon);

    let (_second_daemon, second_socket) =
        start_daemon(run.path(), &database, "disabled-route-second.ready");
    assert_disabled_route(&second_socket);
    assert_enabled_btc_route(&second_socket);

    let zec_enabled = maker_cli(
        &second_socket,
        &[
            "configure-pair",
            "--request-id",
            "disabled-route-zec-enable",
            "--expected-revision",
            "1",
            "--pair",
            "zcash",
            "--direction",
            "taker-sells-lez",
            "--enabled",
            "true",
            "--minimum-foreign-units",
            "10",
            "--maximum-foreign-units",
            "10000",
            "--offer-ttl-seconds",
            "300",
        ],
    );
    assert_configuration_commit(&zec_enabled, 2, false);
    let zec_quote = maker_cli(
        &second_socket,
        &["quote", "--pair", "zcash", "--direction", "taker-sells-lez"],
    );
    assert_success(&zec_quote);
}

fn configure_disabled_zec_route(first_socket: &Path) {
    let zec_disabled = maker_cli(
        first_socket,
        &[
            "configure-pair",
            "--request-id",
            "disabled-route-zec-create",
            "--pair",
            "zcash",
            "--direction",
            "taker-sells-lez",
            "--enabled",
            "false",
            "--minimum-foreign-units",
            "10",
            "--maximum-foreign-units",
            "10000",
            "--offer-ttl-seconds",
            "300",
        ],
    );
    assert_configuration_commit(&zec_disabled, 1, false);
    let zec_price = maker_cli(
        first_socket,
        &[
            "set-local-price",
            "--request-id",
            "disabled-route-zec-price",
            "--pair",
            "zcash",
            "--direction",
            "taker-sells-lez",
            "--lez-units-per-lot",
            "5",
            "--foreign-units-per-lot",
            "2",
        ],
    );
    assert_configuration_commit(&zec_price, 1, false);
}

fn configure_enabled_btc_route(first_socket: &Path) {
    let btc_disabled = maker_cli(
        first_socket,
        &[
            "configure-pair",
            "--request-id",
            "disabled-route-btc-create",
            "--pair",
            "bitcoin",
            "--direction",
            "taker-sells-lez",
            "--enabled",
            "false",
            "--minimum-foreign-units",
            "10",
            "--maximum-foreign-units",
            "10000",
            "--offer-ttl-seconds",
            "300",
        ],
    );
    assert_configuration_commit(&btc_disabled, 1, false);
    let btc_price = maker_cli(
        first_socket,
        &[
            "set-local-price",
            "--request-id",
            "disabled-route-btc-price",
            "--pair",
            "bitcoin",
            "--direction",
            "taker-sells-lez",
            "--lez-units-per-lot",
            "7",
            "--foreign-units-per-lot",
            "3",
        ],
    );
    assert_configuration_commit(&btc_price, 1, false);
    let btc_enabled = maker_cli(
        first_socket,
        &[
            "configure-pair",
            "--request-id",
            "disabled-route-btc-enable",
            "--expected-revision",
            "1",
            "--pair",
            "bitcoin",
            "--direction",
            "taker-sells-lez",
            "--enabled",
            "true",
            "--minimum-foreign-units",
            "10",
            "--maximum-foreign-units",
            "10000",
            "--offer-ttl-seconds",
            "300",
        ],
    );
    assert_configuration_commit(&btc_enabled, 2, false);
}

fn assert_disabled_route(socket: &Path) {
    let quote = maker_cli(
        socket,
        &["quote", "--pair", "zcash", "--direction", "taker-sells-lez"],
    );
    assert!(
        !quote.status.success(),
        "disabled route unexpectedly quoted"
    );
    let quote_error = String::from_utf8_lossy(&quote.stderr);
    assert!(
        quote_error.contains("-32602"),
        "unexpected quote error: {quote_error}"
    );
    assert!(
        quote_error.contains("maker route is disabled"),
        "unexpected quote error: {quote_error}"
    );

    let publish = maker_cli(
        socket,
        &[
            "publish-offer",
            "--request-id",
            "disabled-route-zec-publish",
            "--offer-id",
            "disabled-route-zec-offer",
            "--pair",
            "zcash",
            "--direction",
            "taker-sells-lez",
        ],
    );
    assert!(
        !publish.status.success(),
        "disabled route unexpectedly published"
    );
    let publish_error = String::from_utf8_lossy(&publish.stderr);
    assert!(publish_error.contains("-32602"));
    assert!(publish_error.contains("maker route is disabled"));
}

fn assert_enabled_btc_route(socket: &Path) {
    let quote = maker_cli(
        socket,
        &[
            "quote",
            "--pair",
            "bitcoin",
            "--direction",
            "taker-sells-lez",
        ],
    );
    assert_success(&quote);
}

#[test]
fn owner_lists_and_acknowledges_durable_alert_across_daemon_restart() {
    let run = tempdir().expect("isolated test directory");
    let database = run.path().join("alerts.sqlite3");
    let alert_sequence = seed_replacement_conflict(&database);

    let (first_daemon, first_socket) = start_daemon(run.path(), &database, "alert-first.ready");
    let status = maker_cli(&first_socket, &["status", "--id", "operator-alert-swap"]);
    assert_success(&status);
    assert_attention(&status.stdout, true, 1, "TakerLockReorged");
    let alerts = maker_cli(&first_socket, &["alerts", "--id", "operator-alert-swap"]);
    assert_alert_list(&alerts, alert_sequence, false);
    drop(first_daemon);

    let (_second_daemon, second_socket) = start_daemon(run.path(), &database, "alert-second.ready");
    let restarted = maker_cli(&second_socket, &["status", "--id", "operator-alert-swap"]);
    assert_success(&restarted);
    assert_attention(&restarted.stdout, true, 1, "TakerLockReorged");
    let acknowledged = maker_cli(
        &second_socket,
        &[
            "acknowledge-alert",
            "--id",
            "operator-alert-swap",
            "--alert",
            &alert_sequence.to_string(),
        ],
    );
    assert_success(&acknowledged);
    assert_attention(&acknowledged.stdout, false, 0, "TakerLockReorged");
    let pending = maker_cli(&second_socket, &["alerts", "--id", "operator-alert-swap"]);
    assert_success(&pending);
    assert_eq!(
        serde_json::from_slice::<Value>(&pending.stdout).unwrap(),
        serde_json::json!([])
    );
    let all = maker_cli(
        &second_socket,
        &["alerts", "--id", "operator-alert-swap", "--all"],
    );
    assert_alert_list(&all, alert_sequence, true);
}

#[test]
fn maker_actor_lifecycle_commands_are_read_only_replay_safe_and_restart_durable() {
    let run = tempdir().expect("isolated test directory");
    let database = run.path().join("actor-lifecycle.sqlite3");
    let swap_id = "m5-maker-lifecycle-zec";
    seed_maker_actor(run.path(), &database, swap_id);

    let (first_daemon, first_socket) =
        start_daemon(run.path(), &database, "actor-lifecycle-first.ready");
    assert_initial_actor_monitor(&first_socket, swap_id);
    assert_actor_generation_guards(&first_socket, swap_id);
    let after = queue_claim_and_assert_replay(&first_socket, swap_id);

    drop(first_daemon);
    let (_second_daemon, second_socket) =
        start_daemon(run.path(), &database, "actor-lifecycle-second.ready");
    assert_eq!(maker_actor_monitor(&second_socket, swap_id), after);
    assert_missing_actor_errors(&second_socket);
}

fn maker_actor_monitor(socket: &Path, swap_id: &str) -> Value {
    let output = maker_cli(socket, &["monitor", "--id", swap_id]);
    assert_success(&output);
    serde_json::from_slice(&output.stdout).expect("monitor JSON")
}

fn assert_initial_actor_monitor(socket: &Path, swap_id: &str) {
    let monitor = maker_actor_monitor(socket, swap_id);
    let fields = monitor.as_object().expect("monitor object");
    assert_eq!(fields.len(), 8);
    for field in [
        "schema_version",
        "swap_id",
        "actor_kind",
        "schedule_state",
        "lease_generation",
        "attempt_count",
        "progress",
        "manual_action",
    ] {
        assert!(
            fields.contains_key(field),
            "missing allowlisted field {field}"
        );
    }
    assert_eq!(monitor["schema_version"], 1);
    assert_eq!(monitor["swap_id"], swap_id);
    assert_eq!(monitor["actor_kind"], "zcash");
    assert_eq!(monitor["schedule_state"], "queued");
    assert_eq!(monitor["lease_generation"], 0);
    assert_eq!(monitor["attempt_count"], 0);
    assert!(monitor["progress"].is_null());
    assert!(monitor["manual_action"].is_null());
}

fn assert_actor_generation_guards(socket: &Path, swap_id: &str) {
    let missing_generation = maker_cli(
        socket,
        &[
            "claim",
            "--id",
            swap_id,
            "--request-id",
            "missing-generation",
        ],
    );
    assert!(!missing_generation.status.success());
    assert!(String::from_utf8_lossy(&missing_generation.stderr).contains("--expected-generation"));

    let stale = maker_cli(
        socket,
        &[
            "claim",
            "--id",
            swap_id,
            "--request-id",
            "m5-maker-claim-stale-001",
            "--expected-generation",
            "1",
        ],
    );
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("-32009"));
}

fn queue_claim_and_assert_replay(socket: &Path, swap_id: &str) -> Value {
    let action = [
        "claim",
        "--id",
        swap_id,
        "--request-id",
        "m5-maker-claim-001",
        "--expected-generation",
        "0",
    ];
    let claim = maker_cli(socket, &action);
    assert_success(&claim);
    let claim: Value = serde_json::from_slice(&claim.stdout).expect("claim JSON");
    assert_eq!(claim.as_object().expect("claim object").len(), 5);
    assert_eq!(claim["schema_version"], 1);
    assert_eq!(claim["swap_id"], swap_id);
    assert_eq!(claim["action"], "claim");
    assert_eq!(claim["requested_after_generation"], 0);
    assert_eq!(claim["was_replay"], false);

    let replay = maker_cli(socket, &action);
    assert_success(&replay);
    let replay: Value = serde_json::from_slice(&replay.stdout).expect("claim replay JSON");
    assert_eq!(replay["was_replay"], true);

    let conflicting = maker_cli(
        socket,
        &[
            "refund",
            "--id",
            swap_id,
            "--request-id",
            "m5-maker-claim-001",
            "--expected-generation",
            "0",
        ],
    );
    assert!(!conflicting.status.success());
    assert!(String::from_utf8_lossy(&conflicting.stderr).contains("-32009"));

    let monitor = maker_actor_monitor(socket, swap_id);
    assert_eq!(monitor.as_object().expect("monitor action object").len(), 8);
    assert_eq!(
        monitor["manual_action"]
            .as_object()
            .expect("action object")
            .len(),
        5
    );
    assert_eq!(monitor["lease_generation"], 0);
    assert_eq!(monitor["attempt_count"], 0);
    assert_eq!(monitor["manual_action"]["request_id"], "m5-maker-claim-001");
    assert_eq!(monitor["manual_action"]["action"], "claim");
    assert_eq!(monitor["manual_action"]["state"], "queued");
    assert_eq!(monitor["manual_action"]["requested_after_generation"], 0);
    assert!(monitor["manual_action"]["lease_generation"].is_null());
    monitor
}

fn assert_missing_actor_errors(socket: &Path) {
    let missing = maker_cli(socket, &["monitor", "--id", "missing-maker-actor"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("-32004"));

    let missing_action = maker_cli(
        socket,
        &[
            "refund",
            "--id",
            "missing-maker-actor",
            "--request-id",
            "m5-maker-missing-001",
            "--expected-generation",
            "0",
        ],
    );
    assert!(!missing_action.status.success());
    assert!(String::from_utf8_lossy(&missing_action.stderr).contains("-32004"));
}

fn seed_maker_actor(run: &Path, database: &Path, swap_id: &str) {
    let deployment = actor_deployment(run, swap_id);
    let actor = ActorConfig::load_private(&deployment.source_config).unwrap();
    let direction = SwapDirection::TakerSellsLez;
    let swap = SwapCoordinator::new_with_direction(
        SwapId::new(swap_id).unwrap(),
        Pair::Zcash,
        direction,
        ConfirmationPolicy::new(1).unwrap(),
        RecoverySchedule::new(
            Pair::Zcash,
            direction,
            ChainPosition::block_height(Chain::Zcash, 120),
            ChainPosition::block_height(Chain::Lez, 100),
            TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    );
    let mut store = SqliteSwapStore::open(database).unwrap();
    store.save(&swap).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                SwapId::new(swap_id).unwrap(),
                MakerActorKindV1::Zcash,
                deployment.source_config.clone(),
                Sha256::digest(fs::read(&deployment.source_config).unwrap()).into(),
                deployment.program.clone(),
                hex::decode(&deployment.program_sha256)
                    .unwrap()
                    .try_into()
                    .unwrap(),
                actor.role_state_db().to_path_buf(),
            )
            .unwrap(),
            10,
        )
        .unwrap();
}

fn configure_zec_route(socket: &Path) {
    let disabled = maker_cli(
        socket,
        &[
            "configure-pair",
            "--request-id",
            "operator-pair-zec-create-001",
            "--pair",
            "zcash",
            "--direction",
            "taker-sells-lez",
            "--enabled",
            "false",
            "--minimum-foreign-units",
            "10",
            "--maximum-foreign-units",
            "10000",
            "--offer-ttl-seconds",
            "300",
        ],
    );
    assert_configuration_commit(&disabled, 1, false);
    let price = maker_cli(
        socket,
        &[
            "set-local-price",
            "--request-id",
            "operator-price-zec-create-001",
            "--pair",
            "zcash",
            "--direction",
            "taker-sells-lez",
            "--lez-units-per-lot",
            "5",
            "--foreign-units-per-lot",
            "2",
        ],
    );
    assert_configuration_commit(&price, 1, false);
    let enabled = maker_cli(
        socket,
        &[
            "configure-pair",
            "--request-id",
            "operator-pair-zec-enable-001",
            "--expected-revision",
            "1",
            "--pair",
            "zcash",
            "--direction",
            "taker-sells-lez",
            "--enabled",
            "true",
            "--minimum-foreign-units",
            "10",
            "--maximum-foreign-units",
            "10000",
            "--offer-ttl-seconds",
            "300",
        ],
    );
    assert_configuration_commit(&enabled, 2, false);
    assert_route_lists(socket);
}

fn publish_zec_offer(socket: &Path) {
    let published = maker_cli(
        socket,
        &[
            "publish-offer",
            "--request-id",
            "operator-offer-zec-publish-001",
            "--offer-id",
            "operator-offer-zec-001",
            "--pair",
            "zcash",
            "--direction",
            "taker-sells-lez",
        ],
    );
    assert_configuration_commit(&published, 1, false);
    let replay = maker_cli(
        socket,
        &[
            "publish-offer",
            "--request-id",
            "operator-offer-zec-publish-001",
            "--offer-id",
            "operator-offer-zec-001",
            "--pair",
            "zcash",
            "--direction",
            "taker-sells-lez",
        ],
    );
    assert_configuration_commit(&replay, 1, true);
}

fn withdraw_zec_offer(socket: &Path) {
    let withdrawn = maker_cli(
        socket,
        &[
            "withdraw-offer",
            "--request-id",
            "operator-offer-zec-withdraw-001",
            "--offer-id",
            "operator-offer-zec-001",
            "--expected-revision",
            "1",
        ],
    );
    assert_configuration_commit(&withdrawn, 2, false);
}

fn create_swap(socket: &Path, id: &str, pair: &str, direction: Option<&str>) -> Output {
    let mut arguments = vec![
        "create-swap",
        "--id",
        id,
        "--pair",
        pair,
        "--confirmations",
        "2",
        "--taker-refund-at",
        "120",
    ];
    if pair == "monero" {
        arguments.extend(["--xmr-refund-event-confirmations", "2"]);
    } else {
        arguments.extend([
            "--maker-refund-at",
            "100",
            "--earlier-refund-latest",
            "1000",
            "--later-refund-earliest",
            "1200",
            "--required-margin",
            "100",
        ]);
    }
    if let Some(direction) = direction {
        arguments.extend(["--direction", direction]);
    }
    maker_cli(socket, &arguments)
}

fn start_daemon(run: &Path, database: &Path, name: &str) -> (Daemon, PathBuf) {
    let runtime = run.join(format!("{name}.runtime"));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&runtime)
        .expect("create owner-only maker runtime");
    let socket = runtime.join("maker.sock");
    let ready = runtime.join("ready");
    let delivery_key = run.join("delivery-signing.key");
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&delivery_key)
    {
        Ok(mut file) => {
            writeln!(file, "{}", hex::encode([8_u8; 32])).unwrap();
            file.sync_all().unwrap();
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => panic!("create Delivery signing key: {error}"),
    }
    let actor = actor_deployment(run, "m5-integration-authority-001");
    let child = Command::new(env!("CARGO_BIN_EXE_lez-maker-daemon"))
        .arg("--socket")
        .arg(&socket)
        .arg("--database")
        .arg(database)
        .arg("--ready-file")
        .arg(&ready)
        .arg("--delivery-directory")
        .arg(run.join("delivery"))
        .arg("--chat-socket")
        .arg(runtime.join("chat.sock"))
        .arg("--delivery-signing-key-file")
        .arg(delivery_key)
        .arg("--maker-claim-key-id")
        .arg("m5-operator-claim-key-v1")
        .arg("--maker-claim-key-file")
        .arg(&actor.claim_key)
        .arg("--maker-claim-preimage-file")
        .arg(&actor.claim_preimage)
        .arg("--zec-source-maker-config")
        .arg(&actor.source_config)
        .arg("--zec-maker-actor-root")
        .arg(&actor.root)
        .arg("--zec-actor-program")
        .arg(&actor.program)
        .arg("--zec-actor-program-sha256")
        .arg(&actor.program_sha256)
        .spawn()
        .expect("start maker daemon");
    let mut daemon = Daemon(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(published) = fs::read_to_string(&ready) {
            assert_eq!(published.trim(), socket.to_str().unwrap());
            return (daemon, socket);
        }
        if let Some(status) = daemon.0.try_wait().expect("poll maker daemon") {
            panic!("maker daemon exited before readiness: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "maker daemon readiness timed out"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn assert_delivery_offer(run: &Path, expected: bool) {
    let maker = PublicKey::from_secret_key(
        &Secp256k1::signing_only(),
        &SecretKey::from_slice(&[8_u8; 32]).unwrap(),
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let output = Command::new(env!("CARGO_BIN_EXE_lez-taker"))
        .arg("--delivery-directory")
        .arg(run.join("delivery"))
        .arg("--maker-public-key")
        .arg(hex::encode(maker.serialize()))
        .arg("--now-unix-seconds")
        .arg(now.to_string())
        .arg("--pair")
        .arg("zcash")
        .arg("--direction")
        .arg("taker-sells-lez")
        .output()
        .expect("run separate taker discovery");
    assert_success(&output);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["offers"].as_array().unwrap().len(),
        usize::from(expected)
    );
}

fn maker_cli(socket: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lez-maker"))
        .arg("--socket")
        .arg(socket)
        .args(arguments)
        .output()
        .expect("run maker CLI")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_configuration_commit(output: &Output, revision: u64, replay: bool) {
    assert_success(output);
    let commit: Value = serde_json::from_slice(&output.stdout).expect("CLI emits commit JSON");
    assert_eq!(commit["revision"], revision);
    assert_eq!(commit["was_replay"], replay);
}

fn assert_route_lists(socket: &Path) {
    let pairs = maker_cli(socket, &["pairs"]);
    assert_success(&pairs);
    let pairs: Value = serde_json::from_slice(&pairs.stdout).expect("CLI emits pair JSON");
    assert_eq!(pairs.as_array().unwrap().len(), 1);
    assert_eq!(pairs[0]["revision"], 2);
    assert_eq!(pairs[0]["value"]["enabled"], true);
    assert_eq!(pairs[0]["value"]["price_source"], "local");
    assert_eq!(pairs[0]["value"]["route"]["pair"], "Zcash");
    assert_eq!(pairs[0]["value"]["route"]["direction"], "TakerSellsLez");

    let prices = maker_cli(socket, &["prices"]);
    assert_success(&prices);
    let prices: Value = serde_json::from_slice(&prices.stdout).expect("CLI emits price JSON");
    assert_eq!(prices.as_array().unwrap().len(), 1);
    assert_eq!(prices[0]["revision"], 1);
    assert_eq!(prices[0]["value"]["lez_units_per_lot"], 5);
    assert_eq!(prices[0]["value"]["foreign_units_per_lot"], 2);
}

fn assert_route_quote(socket: &Path) {
    let quote = maker_cli(
        socket,
        &["quote", "--pair", "zcash", "--direction", "taker-sells-lez"],
    );
    assert_success(&quote);
    let quote: Value = serde_json::from_slice(&quote.stdout).expect("CLI emits quote JSON");
    assert_eq!(quote["price"]["route"]["pair"], "Zcash");
    assert_eq!(quote["price"]["route"]["direction"], "TakerSellsLez");
    assert_eq!(quote["price"]["lez_units_per_lot"], 5);
    assert_eq!(quote["price"]["foreign_units_per_lot"], 2);
    assert_eq!(quote["source_revision"], 1);
    assert!(quote["observed_at_unix_seconds"].as_u64().unwrap() > 0);
}

fn assert_offer_history(socket: &Path, expected_status: &str, expected_revision: u64) {
    let offers = maker_cli(socket, &["offers"]);
    assert_success(&offers);
    let offers: Value = serde_json::from_slice(&offers.stdout).expect("CLI emits offer JSON");
    assert_eq!(offers.as_array().unwrap().len(), 1);
    assert_eq!(offers[0]["revision"], expected_revision);
    assert_eq!(offers[0]["status"], expected_status);
    assert_eq!(offers[0]["offer"]["id"], "operator-offer-zec-001");
    assert_eq!(offers[0]["offer"]["pair_configuration_revision"], 2);
    assert_eq!(offers[0]["offer"]["price_source_revision"], 1);
    assert_eq!(offers[0]["offer"]["price"]["lez_units_per_lot"], 5);
    assert_eq!(offers[0]["offer"]["price"]["foreign_units_per_lot"], 2);
}

fn assert_swap_view(bytes: &[u8], id: &str, pair: &str, phase: &str) {
    let view: Value = serde_json::from_slice(bytes).expect("CLI emits JSON");
    assert_eq!(view["id"], id);
    assert_eq!(view["pair"], pair);
    assert_eq!(view["phase"], phase);
}

fn assert_attention(bytes: &[u8], required: bool, pending: u64, phase: &str) {
    let view: Value = serde_json::from_slice(bytes).expect("CLI emits JSON");
    assert_eq!(view["requires_attention"], required);
    assert_eq!(view["pending_alerts"], pending);
    assert_eq!(view["phase"], phase);
}

fn assert_alert_list(output: &Output, sequence: u64, acknowledged: bool) {
    assert_success(output);
    let alerts: Value = serde_json::from_slice(&output.stdout).expect("CLI emits alert JSON");
    assert_eq!(alerts.as_array().unwrap().len(), 1);
    assert_eq!(alerts[0]["sequence"], sequence);
    assert_eq!(alerts[0]["kind"], "zcash_replacement_conflict");
    assert_eq!(alerts[0]["severity"], "warning");
    assert_eq!(alerts[0]["acknowledged"], acknowledged);
}

fn seed_replacement_conflict(path: &Path) -> u64 {
    let mut store = SqliteSwapStore::open(path).unwrap();
    let direction = SwapDirection::TakerSellsForeign;
    let mut swap = SwapCoordinator::new_with_direction(
        SwapId::new("operator-alert-swap").unwrap(),
        Pair::Zcash,
        direction,
        ConfirmationPolicy::new(1).unwrap(),
        RecoverySchedule::new(
            Pair::Zcash,
            direction,
            ChainPosition::block_height(Chain::Lez, 100),
            ChainPosition::block_height(Chain::Zcash, 120),
            TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    );
    store
        .save_with_zcash_binding(&swap, &local_binding())
        .unwrap();
    let original = canonical_observation(7, [0x44; 32], 100, [0xaa; 32], 102);
    apply_zcash_funding_event(
        &mut store,
        0,
        swap.id(),
        &ZcashObservationEvent::Canonical(original.clone()),
    )
    .unwrap();
    swap = store.load(swap.id()).unwrap().unwrap();
    swap.observe_funding(
        Participant::Maker,
        ChainProof::new("lez-maker-lock", 1).unwrap(),
    )
    .unwrap();
    store
        .save_with_zcash_binding(&swap, &local_binding())
        .unwrap();
    let replacement = canonical_observation(8, [0x66; 32], 101, [0xcc; 32], 104);
    let applied = apply_zcash_funding_event(
        &mut store,
        1,
        swap.id(),
        &ZcashObservationEvent::Replaced {
            removed: Box::new(removal(&original)),
            canonical: Box::new(replacement),
        },
    )
    .unwrap();
    applied.alert_sequence().unwrap()
}

fn local_binding() -> ZecSwapBinding {
    ZecSwapBinding::new(
        ZecProfileId::DeterministicLocalV1,
        ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            zatoshis(100_000),
            contract(),
        ),
    )
    .unwrap()
}

fn contract() -> Bip199Contract {
    Bip199Contract::new(500_000, [0x11; 20], [0x22; 32], [0x33; 20])
}

fn zatoshis(value: u64) -> Zatoshis {
    Zatoshis::from_u64(value).unwrap()
}

fn canonical_observation(
    seed: u8,
    inclusion_hash: [u8; 32],
    inclusion_height: u32,
    tip_hash: [u8; 32],
    tip_height: u32,
) -> CanonicalZcashOutputObservation {
    let key = SecretKey::from_slice(&[seed; 32]).unwrap();
    let public_key = PublicKey::from_secret_key(&Secp256k1::new(), &key);
    let owner_script: Script = TransparentAddress::from_pubkey(&public_key).script().into();
    let request = TransparentFundingRequest::new(
        vec![TransparentUtxo::new(
            OutPoint::new([seed.wrapping_add(2); 32], 0),
            TxOut::new(zatoshis(120_000), owner_script),
        )],
        public_key,
        zatoshis(100_000),
        zatoshis(10_000),
        zatoshis(1_000),
        BlockHeight::from_u32(4_100_000),
        BranchId::Nu6_2,
    )
    .unwrap();
    let transaction = build_funding_transaction(&contract(), &request, &key).unwrap();
    let mut raw = vec![];
    transaction.write(&mut raw).unwrap();
    CanonicalZcashOutputObservation::validate(
        local_binding().expected_output(),
        &ZcashNodeSnapshot::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            true,
            BlockHash(inclusion_hash),
            BlockHash(inclusion_hash),
            BlockHeight::from_u32(inclusion_height),
            ZcashStableTip::new(
                BlockHash(tip_hash),
                BlockHeight::from_u32(tip_height),
                BlockHash(tip_hash),
                BlockHeight::from_u32(tip_height),
            ),
            transaction.txid(),
            raw,
            0,
            tip_height - inclusion_height + 1,
        ),
    )
    .unwrap()
}

fn removal(previous: &CanonicalZcashOutputObservation) -> CanonicalZcashOutputRemoval {
    CanonicalZcashOutputRemoval::validate(
        previous,
        &ZcashNodeRemovalSnapshot::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            BlockHash([0x55; 32]),
            ZcashStableTip::new(
                BlockHash([0xbb; 32]),
                BlockHeight::from_u32(104),
                BlockHash([0xbb; 32]),
                BlockHeight::from_u32(104),
            ),
        ),
    )
    .unwrap()
}
