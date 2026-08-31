//! Black-box daemon/CLI/Delivery journey for the Logos C-API price source.

#[path = "support/cross_role_binary.rs"]
mod cross_role;
mod support;

use std::{
    fs,
    fs::OpenOptions,
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Child, Command, Output},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use support::actor_deployment;

use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn daemon_signs_exact_logos_quote_and_replays_without_the_failed_module() {
    let run = tempdir().expect("isolated Logos daemon journey");
    fs::set_permissions(run.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database = run.path().join("maker.sqlite3");
    let module_bytes = b"pinned-logos-price-module-v1";
    let module = secure_file(run.path(), "logos-price.so", module_bytes, 0o600);
    let module_sha256: [u8; 32] = Sha256::digest(module_bytes).into();
    let worker = secure_file(
        run.path(),
        "logos-price-worker",
        br#"#!/bin/sh
now=
while [ "$#" -gt 0 ]; do
  case "$1" in
    --now-unix-seconds) now="$2"; shift 2 ;;
    *) shift ;;
  esac
done
[ -n "$now" ] || exit 90
printf '%s\n' "{\"schema_version\":1,\"status\":\"ok\",\"pair\":\"zcash\",\"direction\":\"taker_sells_lez\",\"lez_units_per_lot\":5,\"foreign_units_per_lot\":2,\"source_revision\":7,\"as_of_unix_seconds\":$now}"
"#,
        0o700,
    );

    let (first, socket) = start_daemon(
        run.path(),
        &database,
        "first",
        &worker,
        &module,
        module_sha256,
    );
    let configured = maker_cli(
        &socket,
        &[
            "configure-pair",
            "--request-id",
            "logos-pair-configure-001",
            "--pair",
            "zcash",
            "--direction",
            "taker-sells-lez",
            "--enabled",
            "true",
            "--price-source",
            "logos-c-api",
            "--minimum-foreign-units",
            "10",
            "--maximum-foreign-units",
            "10000",
            "--offer-ttl-seconds",
            "300",
        ],
    );
    assert_success(&configured);

    let quote = maker_cli(
        &socket,
        &["quote", "--pair", "zcash", "--direction", "taker-sells-lez"],
    );
    assert_success(&quote);
    let quote: Value = serde_json::from_slice(&quote.stdout).unwrap();
    assert_eq!(quote["price"]["lez_units_per_lot"], 5);
    assert_eq!(quote["price"]["foreign_units_per_lot"], 2);
    assert_eq!(quote["source_revision"], 7);

    let published = publish(&socket, "logos-offer-publish-001", "logos-offer-zec-001");
    assert_success(&published);
    let commit: Value = serde_json::from_slice(&published.stdout).unwrap();
    assert_eq!(commit["was_replay"], false);
    assert_signed_discovery(run.path(), module_sha256, 1);

    fs::write(&module, b"corrupt-module").unwrap();
    remove_delivery_files(&run.path().join("delivery"));
    let replay = publish(&socket, "logos-offer-publish-001", "logos-offer-zec-001");
    assert_success(&replay);
    let replay: Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(replay["was_replay"], true);
    assert_signed_discovery(run.path(), module_sha256, 1);

    let rejected = publish(&socket, "logos-offer-publish-002", "logos-offer-zec-002");
    assert!(!rejected.status.success());
    assert_signed_discovery(run.path(), module_sha256, 1);

    drop(first);
    fs::write(&module, module_bytes).unwrap();
    let (_second, _socket) = start_daemon(
        run.path(),
        &database,
        "second",
        &worker,
        &module,
        module_sha256,
    );
    assert_signed_discovery(run.path(), module_sha256, 1);
}

fn publish(socket: &Path, request_id: &str, offer_id: &str) -> Output {
    maker_cli(
        socket,
        &[
            "publish-offer",
            "--request-id",
            request_id,
            "--offer-id",
            offer_id,
            "--pair",
            "zcash",
            "--direction",
            "taker-sells-lez",
        ],
    )
}

fn start_daemon(
    run: &Path,
    database: &Path,
    name: &str,
    worker: &Path,
    module: &Path,
    module_sha256: [u8; 32],
) -> (Daemon, PathBuf) {
    let runtime = run.join(format!("{name}.runtime"));
    fs::create_dir(&runtime).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    let socket = runtime.join("maker.sock");
    let ready = runtime.join("ready");
    let delivery_key = secure_file(
        run,
        "delivery-signing.key",
        &hex::encode([8; 32]).into_bytes(),
        0o600,
    );
    let actor = actor_deployment(run, "m5-integration-authority-001");
    let child = Command::new(env!("CARGO_BIN_EXE_lez-maker-node"))
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
        .arg("m5-logos-price-claim-key-v1")
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
        .arg("--logos-price-worker")
        .arg(worker)
        .arg("--logos-price-module")
        .arg(module)
        .arg("--logos-price-module-sha256")
        .arg(hex::encode(module_sha256))
        .arg("--logos-price-timeout-milliseconds")
        .arg("500")
        .arg("--logos-price-max-age-seconds")
        .arg("30")
        .spawn()
        .expect("start maker daemon");
    let mut daemon = Daemon(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(published) = fs::read_to_string(&ready) {
            assert_eq!(published.trim(), socket.to_str().unwrap());
            return (daemon, socket);
        }
        if let Some(status) = daemon.0.try_wait().unwrap() {
            panic!("maker daemon exited before readiness: {status}");
        }
        assert!(Instant::now() < deadline, "maker readiness timed out");
        thread::sleep(Duration::from_millis(20));
    }
}

fn assert_signed_discovery(run: &Path, module_sha256: [u8; 32], expected: usize) {
    let maker = PublicKey::from_secret_key(
        &Secp256k1::signing_only(),
        &SecretKey::from_slice(&[8; 32]).unwrap(),
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let output = Command::new(cross_role::workspace_binary("lez-taker-cli"))
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
        .unwrap();
    assert_success(&output);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let offers = value["offers"].as_array().unwrap();
    assert_eq!(offers.len(), expected);
    if let Some(offer) = offers.first() {
        assert_eq!(
            offer["offer"]["pair_configuration"]["price_source"],
            "logos_c_api"
        );
        assert_eq!(offer["offer"]["price_source_revision"], 7);
        assert_eq!(
            offer["offer"]["price_source_identity_sha256"],
            serde_json::json!(module_sha256)
        );
        assert_eq!(offer["offer"]["price"]["lez_units_per_lot"], 5);
        assert_eq!(offer["offer"]["price"]["foreign_units_per_lot"], 2);
    }
}

fn remove_delivery_files(directory: &Path) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            fs::remove_file(path).unwrap();
        }
    }
}

fn maker_cli(socket: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lez-maker-cli"))
        .arg("--socket")
        .arg(socket)
        .args(args)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn secure_file(root: &Path, name: &str, bytes: &[u8], mode: u32) -> PathBuf {
    let path = root.join(name);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}
