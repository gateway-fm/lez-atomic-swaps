//! Actual C shared-library boundary for the isolated price worker.

use std::{path::Path, process::Command};

use lez_logos_price_c_api::{AbiDirectionV1, AbiPairV1, WorkerPriceStatusV1, WorkerQuoteV1};
use tempfile::tempdir;

#[test]
fn actual_c_plugin_returns_exact_versioned_integer_quote() {
    let run = tempdir().expect("isolated C-API fixture root");
    let plugin = compile_fixture(run.path(), "good", None);
    let output = run_worker(&plugin, AbiPairV1::Zcash, AbiDirectionV1::TakerSellsLez);
    assert!(output.status.success(), "worker stderr is private");
    let quote: WorkerQuoteV1 = serde_json::from_slice(&output.stdout).expect("bounded worker JSON");
    assert_eq!(quote.schema_version(), 1);
    assert_eq!(quote.status(), WorkerPriceStatusV1::Ok);
    assert_eq!(quote.pair(), AbiPairV1::Zcash);
    assert_eq!(quote.direction(), AbiDirectionV1::TakerSellsLez);
    assert_eq!(quote.lez_units_per_lot(), 5);
    assert_eq!(quote.foreign_units_per_lot(), 2);
    assert_eq!(quote.source_revision(), 7);
    assert_eq!(quote.as_of_unix_seconds(), 1_699_999_995);
}

#[test]
fn missing_and_unavailable_are_typed_without_quote_substitution() {
    let run = tempdir().expect("isolated C-API fixture root");
    for (name, definition, expected) in [
        ("missing", "FIXTURE_MISSING", WorkerPriceStatusV1::Missing),
        (
            "unavailable",
            "FIXTURE_UNAVAILABLE",
            WorkerPriceStatusV1::Unavailable,
        ),
    ] {
        let plugin = compile_fixture(run.path(), name, Some(definition));
        let output = run_worker(&plugin, AbiPairV1::Zcash, AbiDirectionV1::TakerSellsLez);
        assert!(output.status.success());
        let quote: WorkerQuoteV1 = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(quote.status(), expected);
        assert!(!quote.has_quote(), "failure status must not expose a quote");
    }
}

#[test]
fn wrong_abi_missing_symbol_and_native_abort_fail_closed() {
    let run = tempdir().expect("isolated C-API fixture root");
    for (name, definition) in [
        ("wrong-abi", "FIXTURE_WRONG_ABI"),
        ("wrong-version-symbol", "FIXTURE_WRONG_VERSION_SYMBOL"),
        ("missing-symbol", "FIXTURE_MISSING_SYMBOL"),
        ("abort", "FIXTURE_ABORT"),
    ] {
        let plugin = compile_fixture(run.path(), name, Some(definition));
        let output = run_worker(&plugin, AbiPairV1::Zcash, AbiDirectionV1::TakerSellsLez);
        assert!(
            !output.status.success(),
            "{name} must not become a successful quote"
        );
        assert!(output.stdout.len() <= 4096, "diagnostics stay bounded");
    }
}

#[test]
fn malformed_route_ratio_revision_time_and_reserved_fields_fail_closed() {
    let run = tempdir().expect("isolated C-API fixture root");
    for (name, definition) in [
        ("wrong-route", "FIXTURE_WRONG_ROUTE"),
        ("zero-price", "FIXTURE_ZERO_PRICE"),
        ("zero-revision", "FIXTURE_ZERO_REVISION"),
        ("stale", "FIXTURE_STALE"),
        ("future", "FIXTURE_FUTURE"),
        ("reserved", "FIXTURE_RESERVED"),
    ] {
        let plugin = compile_fixture(run.path(), name, Some(definition));
        let output = run_worker(&plugin, AbiPairV1::Zcash, AbiDirectionV1::TakerSellsLez);
        assert!(
            !output.status.success(),
            "{name} must not become a successful quote"
        );
        assert!(output.stdout.is_empty(), "{name} must not emit quote JSON");
    }
}

fn compile_fixture(root: &Path, name: &str, definition: Option<&str>) -> std::path::PathBuf {
    let output = root.join(format!("lib{name}.so"));
    let mut command = Command::new("cc");
    command
        .arg("-shared")
        .arg("-fPIC")
        .arg("-std=c11")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg("-I")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/include"));
    if let Some(definition) = definition {
        command.arg(format!("-D{definition}"));
    }
    let status = command
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/logos_price_fixture.c"
        ))
        .arg("-o")
        .arg(&output)
        .status()
        .expect("invoke C compiler");
    assert!(status.success(), "compile actual C fixture {name}");
    output
}

fn run_worker(plugin: &Path, pair: AbiPairV1, direction: AbiDirectionV1) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lez-logos-price-worker"))
        .arg("--library")
        .arg(plugin)
        .arg("--pair")
        .arg(u32::from(pair).to_string())
        .arg("--direction")
        .arg(u32::from(direction).to_string())
        .arg("--now-unix-seconds")
        .arg("1700000000")
        .output()
        .expect("run isolated C-API worker")
}
