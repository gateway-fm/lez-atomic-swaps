//! Black-box acceptance tests at the maker operator process boundary.

use std::{
    fs,
    path::Path,
    process::{Child, Command, Output},
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;
use tempfile::tempdir;

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
            "--maker-refund-latest",
            "1000",
            "--taker-refund-earliest",
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
