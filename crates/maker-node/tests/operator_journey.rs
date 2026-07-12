//! Black-box acceptance tests at the maker operator process boundary.

use std::{
    fs,
    path::Path,
    process::{Child, Command, Output},
    thread,
    time::{Duration, Instant},
};

use lez_maker_node::apply_zcash_funding_event;
use lez_swap_core::{
    Chain, ChainPosition, ChainProof, ConfirmationPolicy, Pair, Participant, RecoverySchedule,
    SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::SqliteSwapStore;
use lez_zec_swap_sdk::{
    Bip199Contract, CanonicalZcashOutputObservation, CanonicalZcashOutputRemoval,
    ExpectedBip199Output, TransparentFundingRequest, TransparentUtxo, ZcashNodeRemovalSnapshot,
    ZcashNodeSnapshot, ZcashObservationEvent, ZcashStableTip, ZecProfileId, ZecSwapBinding,
    build_funding_transaction,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::Value;
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

const TOKEN: &str = "e2e-maker-owner-capability";

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn maker_cli_controls_authenticated_daemon_and_survives_restart() {
    let run = tempdir().expect("isolated test directory");
    let database = run.path().join("maker.sqlite3");

    let (first_daemon, first_endpoint) = start_daemon(run.path(), &database, "first.ready");
    let created = create_swap(&first_endpoint, "operator-swap-1", "bitcoin", None);
    assert_success(&created);
    assert_swap_view(&created.stdout, "operator-swap-1", "Bitcoin", "Offered");

    let reverse = create_swap(
        &first_endpoint,
        "operator-swap-reverse",
        "zcash",
        Some("taker-sells-lez"),
    );
    assert_success(&reverse);
    let reverse_view: Value = serde_json::from_slice(&reverse.stdout).expect("CLI emits JSON");
    assert_eq!(reverse_view["direction"], "TakerSellsLez");

    let xmr = create_swap(
        &first_endpoint,
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
        &first_endpoint,
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
        &first_endpoint,
        "wrong-capability",
        &["status", "--id", "operator-swap-1"],
    );
    assert!(
        !denied.status.success(),
        "unauthorized CLI unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&denied.stderr).contains("401"),
        "unexpected denial: {}",
        String::from_utf8_lossy(&denied.stderr)
    );

    drop(first_daemon);

    let (_second_daemon, second_endpoint) = start_daemon(run.path(), &database, "second.ready");
    let recovered = maker_cli(
        &second_endpoint,
        TOKEN,
        &["status", "--id", "operator-swap-1"],
    );
    assert_success(&recovered);
    assert_swap_view(&recovered.stdout, "operator-swap-1", "Bitcoin", "Offered");

    let reverse_recovered = maker_cli(
        &second_endpoint,
        TOKEN,
        &["status", "--id", "operator-swap-reverse"],
    );
    assert_success(&reverse_recovered);
    let reverse_view: Value =
        serde_json::from_slice(&reverse_recovered.stdout).expect("CLI emits JSON");
    assert_eq!(reverse_view["direction"], "TakerSellsLez");
}

#[test]
fn owner_lists_and_acknowledges_durable_alert_across_daemon_restart() {
    let run = tempdir().expect("isolated test directory");
    let database = run.path().join("alerts.sqlite3");
    let alert_sequence = seed_replacement_conflict(&database);

    let (first_daemon, first_endpoint) = start_daemon(run.path(), &database, "alert-first.ready");
    let status = maker_cli(
        &first_endpoint,
        TOKEN,
        &["status", "--id", "operator-alert-swap"],
    );
    assert_success(&status);
    assert_attention(&status.stdout, true, 1, "TakerLockReorged");
    let denied = maker_cli(
        &first_endpoint,
        "wrong-capability",
        &["alerts", "--id", "operator-alert-swap"],
    );
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("401"));
    let alerts = maker_cli(
        &first_endpoint,
        TOKEN,
        &["alerts", "--id", "operator-alert-swap"],
    );
    assert_alert_list(&alerts, alert_sequence, false);
    drop(first_daemon);

    let (_second_daemon, second_endpoint) =
        start_daemon(run.path(), &database, "alert-second.ready");
    let restarted = maker_cli(
        &second_endpoint,
        TOKEN,
        &["status", "--id", "operator-alert-swap"],
    );
    assert_success(&restarted);
    assert_attention(&restarted.stdout, true, 1, "TakerLockReorged");
    let acknowledged = maker_cli(
        &second_endpoint,
        TOKEN,
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
    let pending = maker_cli(
        &second_endpoint,
        TOKEN,
        &["alerts", "--id", "operator-alert-swap"],
    );
    assert_success(&pending);
    assert_eq!(
        serde_json::from_slice::<Value>(&pending.stdout).unwrap(),
        serde_json::json!([])
    );
    let all = maker_cli(
        &second_endpoint,
        TOKEN,
        &["alerts", "--id", "operator-alert-swap", "--all"],
    );
    assert_alert_list(&all, alert_sequence, true);
}

fn create_swap(endpoint: &str, id: &str, pair: &str, direction: Option<&str>) -> Output {
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
    maker_cli(endpoint, TOKEN, &arguments)
}

fn start_daemon(run: &Path, database: &Path, ready_name: &str) -> (Daemon, String) {
    let ready = run.join(ready_name);
    let child = Command::new(env!("CARGO_BIN_EXE_lez-maker-daemon"))
        .args(["--listen", "127.0.0.1:0", "--database"])
        .arg(database)
        .arg("--ready-file")
        .arg(&ready)
        .env("LEZ_MAKER_RPC_TOKEN", TOKEN)
        .spawn()
        .expect("start maker daemon");
    let mut daemon = Daemon(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(endpoint) = fs::read_to_string(&ready) {
            return (daemon, endpoint);
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

fn maker_cli(endpoint: &str, token: &str, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lez-maker"))
        .arg("--rpc-url")
        .arg(endpoint)
        .args(arguments)
        .env("LEZ_MAKER_RPC_TOKEN", token)
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
