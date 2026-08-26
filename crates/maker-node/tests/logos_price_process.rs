//! Exact parent-process contract for the isolated Logos price worker.

use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    time::Duration,
};

use lez_maker_node::{PriceSource as _, PriceSourceError, ProcessLogosPriceSource};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_store::{LocalPriceV1, MakerRouteV1};
use sha2::{Digest as _, Sha256};
use tempfile::{TempDir, tempdir};

const NOW: u64 = 1_700_000_000;

#[test]
fn exact_secure_worker_quote_is_copied_into_the_domain() {
    let run = secure_run();
    let module = secure_file(run.path(), "price.so", b"module-v1", 0o600);
    let worker = worker_script(
        run.path(),
        "good-worker",
        r#"printf '%s\n' '{"schema_version":1,"status":"ok","pair":"zcash","direction":"taker_sells_lez","lez_units_per_lot":5,"foreign_units_per_lot":2,"source_revision":7,"as_of_unix_seconds":1699999995}'"#,
    );
    let source = source(&worker, &module, Duration::from_millis(500)).unwrap();
    let route = route();

    let quote = source.quote(route, NOW).unwrap();

    assert_eq!(quote.price(), &LocalPriceV1::new(route, 5, 2).unwrap());
    assert_eq!(quote.source_revision(), 7);
    assert_eq!(quote.observed_at_unix_seconds(), NOW - 5);
}

#[test]
fn typed_unavailability_is_not_substituted_with_a_local_or_zero_quote() {
    let run = secure_run();
    let module = secure_file(run.path(), "price.so", b"module-v1", 0o600);
    let worker = worker_script(
        run.path(),
        "unavailable-worker",
        r#"printf '%s\n' '{"schema_version":1,"status":"unavailable","pair":"zcash","direction":"taker_sells_lez","lez_units_per_lot":null,"foreign_units_per_lot":null,"source_revision":null,"as_of_unix_seconds":null}'"#,
    );
    let source = source(&worker, &module, Duration::from_millis(500)).unwrap();

    assert!(matches!(
        source.quote(route(), NOW),
        Err(PriceSourceError::UnavailableQuote)
    ));
}

#[test]
fn worker_abort_timeout_and_oversized_output_fail_closed() {
    let run = secure_run();
    let module = secure_file(run.path(), "price.so", b"module-v1", 0o600);
    for (name, body, timeout, expected_timeout) in [
        ("abort-worker", "kill -ABRT $$", 500, false),
        ("hang-worker", "while :; do :; done", 100, true),
        (
            "oversize-worker",
            "head -c 5000 /dev/zero | tr '\\0' x",
            500,
            false,
        ),
    ] {
        let worker = worker_script(run.path(), name, body);
        let source = source(&worker, &module, Duration::from_millis(timeout)).unwrap();
        let result = source.quote(route(), NOW);
        if expected_timeout {
            assert!(
                matches!(result, Err(PriceSourceError::SourceTimeout)),
                "{name}: {result:?}"
            );
        } else {
            assert!(
                matches!(result, Err(PriceSourceError::InvalidSource)),
                "{name}: {result:?}"
            );
        }
    }
}

#[test]
fn mutated_writable_hardlinked_and_wrong_hash_modules_fail_before_quote() {
    let run = secure_run();
    let worker = worker_script(run.path(), "marker-worker", "exit 91");

    let mutable = secure_file(run.path(), "mutable.so", b"module-v1", 0o600);
    let mutated_source = source(&worker, &mutable, Duration::from_millis(500)).unwrap();
    fs::write(&mutable, b"module-v2").unwrap();
    assert!(matches!(
        mutated_source.quote(route(), NOW),
        Err(PriceSourceError::InvalidSource)
    ));

    let writable = secure_file(run.path(), "writable.so", b"module-v1", 0o620);
    assert!(matches!(
        source(&worker, &writable, Duration::from_millis(500)),
        Err(PriceSourceError::InvalidSource)
    ));

    let linked = secure_file(run.path(), "linked.so", b"module-v1", 0o600);
    fs::hard_link(&linked, run.path().join("linked-copy.so")).unwrap();
    assert!(matches!(
        source(&worker, &linked, Duration::from_millis(500)),
        Err(PriceSourceError::InvalidSource)
    ));

    let exact = secure_file(run.path(), "exact.so", b"module-v1", 0o600);
    let wrong_hash = [0x55; 32];
    assert!(matches!(
        ProcessLogosPriceSource::new(worker, exact, wrong_hash, Duration::from_millis(500), 30,),
        Err(PriceSourceError::InvalidSource)
    ));
}

fn secure_run() -> TempDir {
    let run = tempdir().unwrap();
    fs::set_permissions(run.path(), fs::Permissions::from_mode(0o700)).unwrap();
    run
}

fn source(
    worker: &Path,
    module: &Path,
    timeout: Duration,
) -> Result<ProcessLogosPriceSource, PriceSourceError> {
    ProcessLogosPriceSource::new(
        worker.to_path_buf(),
        module.to_path_buf(),
        sha256(module),
        timeout,
        30,
    )
}

fn route() -> MakerRouteV1 {
    MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).unwrap()
}

fn sha256(path: &Path) -> [u8; 32] {
    Sha256::digest(fs::read(path).unwrap()).into()
}

fn secure_file(root: &Path, name: &str, bytes: &[u8], mode: u32) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}

fn worker_script(root: &Path, name: &str, body: &str) -> PathBuf {
    let script = format!("#!/bin/sh\n{body}\n");
    secure_file(root, name, script.as_bytes(), 0o700)
}
