//! What a Taker Node holds for a Zcash swap it prepared at take time
//! (`zec_reserve_v1`, `taker_service/zec_dynamic.rs`): its two role
//! configurations and, in the swap's own directory under the swaps root, the
//! record `zec-taker-prepared.json` that the Node reloads on every start. A
//! test that needs a prepared Zcash swap without a Maker or a Zebra writes
//! that record here, as a take interrupted after its preparation leaves it.
#![allow(dead_code)]

use std::{
    fs,
    os::unix::fs::{DirBuilderExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

fn private_directory(path: &Path) {
    if !path.is_dir() {
        fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    }
}

fn private_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// Writes the Taker's Bitcoin and Zcash role configurations under `home` and
/// returns the `initiation` object of its startup configuration. No chain is
/// contacted when the Node loads them. `zcash_key` is the Taker's own Zcash
/// key (32 raw bytes), which is also the key it signs agreements with.
pub fn take_time_initiation(home: &Path, registry: &Path, zcash_key: &Path) -> Value {
    private_directory(&home.join("swaps"));
    private_file(&home.join("cookie"), b"user:pass\n");
    private_file(
        &home.join("lez-signer.key"),
        format!("{}\n", hex::encode([0x78_u8; 32])).as_bytes(),
    );
    let btc_role = home.join("btc-role.json");
    private_file(
        &btc_role,
        &serde_json::to_vec(&json!({
            "schema_version": 1,
            "swaps_root": home.join("swaps"),
            "bitcoin": {
                "network": "regtest",
                "endpoint": "http://127.0.0.1:18443/",
                "cookie_file": home.join("cookie"),
                "genesis_block_hash": "0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206",
                "required_confirmations": 2,
                "refund_csv_blocks": 144,
                "second_lock_refund_csv_blocks": 7,
                "claim_fee_sat": 1000
            },
            "lez": {
                "channel_id": hex::encode([0x23_u8; 32]),
                "genesis_block_hash": hex::encode([0x22_u8; 32]),
                "escrow_program_id": hex::encode([0x24_u8; 32]),
                "authenticated_transfer_program_id": hex::encode([0x25_u8; 32]),
                // Never answers: the swaps here stop before their sidecar is used.
                "sidecar_program": "/usr/bin/true",
                "sequencer_url": "http://127.0.0.1:3040",
                "indexer_url": "http://127.0.0.1:8779",
                "sidecar_port_base": 21_000 + u16::try_from(std::process::id() % 400).unwrap() * 50,
                "sidecar_port_count": 10,
                "signer_key_file": home.join("lez-signer.key"),
                "request_timeout_millis": 5000,
                "discovery_max_blocks": 512
            },
            "recovery": {
                "maker_second_lock_cutoff_seconds": 1800,
                "earlier_refund_latest_seconds": 3600,
                "later_refund_earliest_seconds": 7200,
                "required_margin_seconds": 300
            },
            "actor": {"program": "/usr/bin/true", "program_sha256": hex::encode([0x11_u8; 32])}
        }))
        .unwrap(),
    );
    let zec_role = home.join("zec-role.json");
    private_file(
        &zec_role,
        &serde_json::to_vec(&json!({
            "schema_version": 1,
            "profile": "deterministic-local-devnet-v5",
            "zebra_endpoint": "http://127.0.0.1:18232",
            "zcash_key_file": zcash_key,
        }))
        .unwrap(),
    );
    json!({
        "registry_database": registry,
        "btc_role_config": btc_role,
        "zec_role_config": zec_role,
    })
}

/// Persists `entry` (a prepared swap as the take writes it) in the swap's
/// directory under the swaps root of [`take_time_initiation`], named by its
/// reservation id, and returns the record's path.
pub fn persist_take_time_swap(home: &Path, entry: &Value) -> PathBuf {
    let reservation = entry["reservation_id"].as_str().unwrap();
    let directory = home.join("swaps").join(reservation);
    private_directory(&home.join("swaps"));
    private_directory(&directory);
    let record = directory.join("zec-taker-prepared.json");
    private_file(&record, &serde_json::to_vec_pretty(entry).unwrap());
    record
}
