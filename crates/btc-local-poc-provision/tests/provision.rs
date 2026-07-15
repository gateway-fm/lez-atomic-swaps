#![cfg(unix)]

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
    process::Command,
};

use bitcoin::hashes::Hash as _;
use btc_local_poc_provision::{finalize_stage2, generate_stage1};
use lez_btc_swap_sdk::BtcAgreementV1;
use tempfile::TempDir;

fn write_private_json(path: &Path, value: &serde_json::Value) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    serde_json::to_writer(&mut file, value).unwrap();
    file.write_all(b"\n").unwrap();
}

fn planning() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "maker_lez_owner_account": "1010101010101010101010101010101010101010101010101010101010101010",
        "taker_lez_owner_account": "1111111111111111111111111111111111111111111111111111111111111111",
        "refund_csv_blocks": 144
    })
}

fn stage2_spec(direction: &str, public_sha256: &str, aggregate_x_only: &str) -> serde_json::Value {
    let (depositor, claimant, refund_ms) = if direction == "taker_sells_foreign" {
        (
            "1010101010101010101010101010101010101010101010101010101010101010",
            "1111111111111111111111111111111111111111111111111111111111111111",
            1_700_000_100_000_u64,
        )
    } else {
        (
            "1111111111111111111111111111111111111111111111111111111111111111",
            "1010101010101010101010101010101010101010101010101010101010101010",
            1_700_000_500_000_u64,
        )
    };
    serde_json::json!({
        "schema_version": 1,
        "stage1_public_sha256": public_sha256,
        "swap_id": "2020202020202020202020202020202020202020202020202020202020202020",
        "direction": direction,
        "bitcoin": {
            "genesis_block_hash": "0f9188f13cb7b2c9e5f9a4f0c454796f3abf774a34f4a4f7f2f2f0f6f4f3f2f1",
            "required_confirmations": 1,
            "observed_confirmations": 1,
            "funding_transaction_id": "2121212121212121212121212121212121212121212121212121212121212121",
            "funding_output_index": 1,
            "funding_value_sat": 100_000,
            "funding_anchor_height": 1000,
            "funding_script_pubkey": "filled-after-stage1",
            "claim_value_sat": 99000
        },
        "lez_runtime": {
            "compatibility": "lee_v0_2_0",
            "chain_id": "1717171717171717171717171717171717171717171717171717171717171717",
            "channel_id": "1717171717171717171717171717171717171717171717171717171717171717",
            "genesis_block_hash": "1818181818181818181818181818181818181818181818181818181818181818",
            "escrow_program_id": "1515151515151515151515151515151515151515151515151515151515151515",
            "authenticated_transfer_program_id": "1616161616161616161616161616161616161616161616161616161616161616"
        },
        "lez_terms": {
            "aggregate_authority_mapping": {
                "schema": "lez-v0.2-nssa-account-id",
                "version": 1,
                "x_only_public_key": aggregate_x_only,
                "account_id": "1212121212121212121212121212121212121212121212121212121212121212"
            },
            "metadata_account": "1313131313131313131313131313131313131313131313131313131313131313",
            "custody_account": "1414141414141414141414141414141414141414141414141414141414141414",
            "depositor_account": depositor,
            "claimant_account": claimant,
            "amount": 5000,
            "refund_at_ms": refund_ms,
            "prepared_claim_message_hash": "1919191919191919191919191919191919191919191919191919191919191919"
        },
        "recovery": {
            "refund_csv_blocks": 144,
            "bitcoin_refund_height": 1144,
            "earlier_refund_latest_unix_seconds": 1_700_000_100,
            "later_refund_earliest_unix_seconds": 1_700_000_500,
            "required_margin_seconds": 300
        }
    })
}

fn provision(direction: &str) -> (TempDir, btc_local_poc_provision::Stage2Summary) {
    let temp = tempfile::tempdir().unwrap();
    let planning_path = temp.path().join("planning.json");
    write_private_json(&planning_path, &planning());
    let output = temp.path().join(format!("{direction}-fixture"));
    let stage1 = generate_stage1(&planning_path, &output).unwrap();
    let public: serde_json::Value =
        serde_json::from_slice(&fs::read(stage1.public_spec_file()).unwrap()).unwrap();
    let aggregate = public["aggregate_internal_key"].as_str().unwrap();
    let contract = &public["contracts"][direction];
    let mut spec = stage2_spec(direction, stage1.public_spec_sha256(), aggregate);
    spec["bitcoin"]["funding_script_pubkey"] = contract["script_pubkey"].clone();
    let stage2_path = temp.path().join("stage2.json");
    write_private_json(&stage2_path, &spec);
    let summary = finalize_stage2(&stage2_path, &output).unwrap();
    (temp, summary)
}

#[test]
fn happy_path_revalidates_agreement_in_both_directions() {
    for direction in ["taker_sells_foreign", "taker_sells_lez"] {
        let (_temp, summary) = provision(direction);
        let wire = fs::read(summary.agreement_file()).unwrap();
        let agreement = BtcAgreementV1::from_wire(&wire).unwrap();
        assert_eq!(agreement.encode_wire().unwrap(), wire);
        assert_eq!(summary.direction(), direction);
        assert_eq!(
            bitcoin::Txid::from_byte_array(*agreement.funding_terms().transaction_id()).to_string(),
            "2121212121212121212121212121212121212121212121212121212121212121"
        );
    }
}

#[test]
fn files_are_private_single_link_and_no_clobber() {
    let temp = tempfile::tempdir().unwrap();
    let planning_path = temp.path().join("planning.json");
    write_private_json(&planning_path, &planning());
    let output = temp.path().join("fixture");
    let summary = generate_stage1(&planning_path, &output).unwrap();
    for entry in fs::read_dir(output.join("private")).unwrap() {
        let metadata = entry.unwrap().metadata().unwrap();
        assert!(metadata.is_file());
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
        assert_eq!(metadata.nlink(), 1);
    }
    assert!(generate_stage1(&planning_path, &output).is_err());
    assert!(summary.public_spec_file().is_file());
}

#[test]
fn strict_inputs_and_crosswired_public_material_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let bad_planning = temp.path().join("bad-planning.json");
    let mut unknown = planning();
    unknown["unexpected"] = serde_json::json!(true);
    write_private_json(&bad_planning, &unknown);
    assert!(generate_stage1(&bad_planning, &temp.path().join("bad")).is_err());

    let planning_path = temp.path().join("planning.json");
    write_private_json(&planning_path, &planning());
    let left = temp.path().join("left");
    let right = temp.path().join("right");
    let left_summary = generate_stage1(&planning_path, &left).unwrap();
    let right_summary = generate_stage1(&planning_path, &right).unwrap();
    let left_public: serde_json::Value =
        serde_json::from_slice(&fs::read(left_summary.public_spec_file()).unwrap()).unwrap();
    let right_public: serde_json::Value =
        serde_json::from_slice(&fs::read(right_summary.public_spec_file()).unwrap()).unwrap();
    let mut spec = stage2_spec(
        "taker_sells_foreign",
        right_summary.public_spec_sha256(),
        right_public["aggregate_internal_key"].as_str().unwrap(),
    );
    spec["bitcoin"]["funding_script_pubkey"] =
        left_public["contracts"]["taker_sells_foreign"]["script_pubkey"].clone();
    let stage2_path = temp.path().join("crosswired.json");
    write_private_json(&stage2_path, &spec);
    assert!(finalize_stage2(&stage2_path, &left).is_err());

    fs::copy(
        right.join("private/maker-signing.key"),
        left.join("private/maker-signing.key"),
    )
    .unwrap();
    let mut private_crosswire = stage2_spec(
        "taker_sells_foreign",
        left_summary.public_spec_sha256(),
        left_public["aggregate_internal_key"].as_str().unwrap(),
    );
    private_crosswire["bitcoin"]["funding_script_pubkey"] =
        left_public["contracts"]["taker_sells_foreign"]["script_pubkey"].clone();
    let private_crosswire_path = temp.path().join("private-crosswire.json");
    write_private_json(&private_crosswire_path, &private_crosswire);
    assert!(finalize_stage2(&private_crosswire_path, &left).is_err());
}

#[test]
fn symlink_and_unsafe_secret_modes_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let planning_path = temp.path().join("planning.json");
    write_private_json(&planning_path, &planning());
    let output = temp.path().join("fixture");
    let summary = generate_stage1(&planning_path, &output).unwrap();
    let public: serde_json::Value =
        serde_json::from_slice(&fs::read(summary.public_spec_file()).unwrap()).unwrap();
    let mut spec = stage2_spec(
        "taker_sells_foreign",
        summary.public_spec_sha256(),
        public["aggregate_internal_key"].as_str().unwrap(),
    );
    spec["bitcoin"]["funding_script_pubkey"] =
        public["contracts"]["taker_sells_foreign"]["script_pubkey"].clone();
    let stage2_path = temp.path().join("stage2.json");
    write_private_json(&stage2_path, &spec);

    let signing = output.join("private/maker-signing.key");
    fs::set_permissions(&signing, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(finalize_stage2(&stage2_path, &output).is_err());

    fs::set_permissions(&signing, fs::Permissions::from_mode(0o600)).unwrap();
    let original = output.join("private/maker-signing.real");
    fs::rename(&signing, &original).unwrap();
    std::os::unix::fs::symlink(&original, &signing).unwrap();
    assert!(finalize_stage2(&stage2_path, &output).is_err());
}

#[test]
fn finalize_rejects_bad_authority_schedule_unknown_fields_and_clobber() {
    let temp = tempfile::tempdir().unwrap();
    let planning_path = temp.path().join("planning.json");
    write_private_json(&planning_path, &planning());
    let output = temp.path().join("fixture");
    let summary = generate_stage1(&planning_path, &output).unwrap();
    let public: serde_json::Value =
        serde_json::from_slice(&fs::read(summary.public_spec_file()).unwrap()).unwrap();
    let aggregate = public["aggregate_internal_key"].as_str().unwrap();
    let script = public["contracts"]["taker_sells_foreign"]["script_pubkey"].clone();

    let mut bad_authority = stage2_spec(
        "taker_sells_foreign",
        summary.public_spec_sha256(),
        aggregate,
    );
    bad_authority["bitcoin"]["funding_script_pubkey"] = script.clone();
    bad_authority["lez_terms"]["aggregate_authority_mapping"]["x_only_public_key"] =
        serde_json::json!("2323232323232323232323232323232323232323232323232323232323232323");
    let bad_authority_path = temp.path().join("bad-authority.json");
    write_private_json(&bad_authority_path, &bad_authority);
    assert!(finalize_stage2(&bad_authority_path, &output).is_err());

    let mut bad_schedule = stage2_spec(
        "taker_sells_foreign",
        summary.public_spec_sha256(),
        aggregate,
    );
    bad_schedule["bitcoin"]["funding_script_pubkey"] = script.clone();
    bad_schedule["recovery"]["bitcoin_refund_height"] = serde_json::json!(1_145);
    let bad_schedule_path = temp.path().join("bad-schedule.json");
    write_private_json(&bad_schedule_path, &bad_schedule);
    assert!(finalize_stage2(&bad_schedule_path, &output).is_err());

    let mut immature = stage2_spec(
        "taker_sells_foreign",
        summary.public_spec_sha256(),
        aggregate,
    );
    immature["bitcoin"]["funding_script_pubkey"] = script.clone();
    immature["bitcoin"]["observed_confirmations"] = serde_json::json!(0);
    let immature_path = temp.path().join("immature.json");
    write_private_json(&immature_path, &immature);
    assert!(finalize_stage2(&immature_path, &output).is_err());

    let mut unknown = stage2_spec(
        "taker_sells_foreign",
        summary.public_spec_sha256(),
        aggregate,
    );
    unknown["bitcoin"]["funding_script_pubkey"] = script.clone();
    unknown["lez_terms"]["unexpected"] = serde_json::json!(true);
    let unknown_path = temp.path().join("unknown.json");
    write_private_json(&unknown_path, &unknown);
    assert!(finalize_stage2(&unknown_path, &output).is_err());

    let mut valid = stage2_spec(
        "taker_sells_foreign",
        summary.public_spec_sha256(),
        aggregate,
    );
    valid["bitcoin"]["funding_script_pubkey"] = script;
    let valid_path = temp.path().join("valid.json");
    write_private_json(&valid_path, &valid);
    finalize_stage2(&valid_path, &output).unwrap();
    assert!(finalize_stage2(&valid_path, &output).is_err());
}

#[test]
fn generate_cli_stdout_is_strict_secret_free_json() {
    let temp = tempfile::tempdir().unwrap();
    let planning_path = temp.path().join("planning.json");
    write_private_json(&planning_path, &planning());
    let output_root = temp.path().join("fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_btc-local-poc-provision"))
        .arg("generate")
        .arg("--planning-file")
        .arg(&planning_path)
        .arg("--output-root")
        .arg(&output_root)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["private_material_disclosed"], false);
    assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 1);
    for entry in fs::read_dir(output_root.join("private")).unwrap() {
        let secret = fs::read(entry.unwrap().path()).unwrap();
        let secret_hex = hex::encode(&secret);
        assert!(
            !output
                .stdout
                .windows(secret.len())
                .any(|window| window == secret)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains(&secret_hex));
    }
}
