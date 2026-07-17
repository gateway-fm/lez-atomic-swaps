#![cfg(unix)]

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
    process::Command,
};

use bitcoin::{
    Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness, absolute,
    consensus::{deserialize, serialize},
    hashes::Hash as _,
    key::{Keypair, TweakedPublicKey},
    secp256k1::{Message, Secp256k1, SecretKey},
    sighash::{Prevouts, SighashCache, TapSighashType},
    taproot, transaction,
};
use btc_local_poc_provision::{
    finalize_asset_extension, finalize_stage2, generate_stage1, prepare_funding,
};
use lez_btc_swap_sdk::{BtcAgreementV1, BtcLezAssetExtensionV1, BtcLezAssetV1};
use sha2::{Digest as _, Sha256};
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

fn write_private_bytes(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
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
            "funding_signed_transaction": "filled-after-stage1",
            "funding_signed_transaction_sha256": "filled-after-stage1",
            "funding_input_value_sat": 126_000,
            "funding_input_script_pubkey": "filled-after-stage1",
            "funding_transaction_id": "filled-after-stage1",
            "funding_output_index": 1,
            "funding_value_sat": 100_000,
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
            "planned_bitcoin_funding_anchor_height": 1000,
            "bitcoin_refund_height": 1144,
            "maker_second_lock_cutoff_unix_seconds": 1_699_999_800,
            "earlier_refund_latest_unix_seconds": 1_700_000_100,
            "later_refund_earliest_unix_seconds": 1_700_000_500,
            "required_margin_seconds": 300
        }
    })
}

fn signed_funding_transaction(contract_script: &str) -> Transaction {
    let secret = SecretKey::from_slice(&[0x07; 32]).unwrap();
    let input_script = rawtr_input(&secret);
    let mut transaction = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::from_byte_array([0x31; 32]),
                vout: 0,
            },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::new(),
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(25_000),
                script_pubkey: ScriptBuf::from_bytes(vec![0x51]),
            },
            TxOut {
                value: Amount::from_sat(100_000),
                script_pubkey: ScriptBuf::from_bytes(hex::decode(contract_script).unwrap()),
            },
        ],
    };
    let prevouts = [TxOut {
        value: Amount::from_sat(126_000),
        script_pubkey: input_script,
    }];
    let sighash = SighashCache::new(&transaction)
        .taproot_key_spend_signature_hash(0, &Prevouts::All(&prevouts), TapSighashType::Default)
        .unwrap();
    let keypair = Keypair::from_secret_key(&Secp256k1::new(), &secret);
    let signature = Secp256k1::new()
        .sign_schnorr_no_aux_rand(&Message::from_digest(sighash.to_byte_array()), &keypair);
    transaction.input[0].witness =
        Witness::p2tr_key_spend(&taproot::Signature::from_slice(&signature.serialize()).unwrap());
    transaction
}

fn bind_funding(spec: &mut serde_json::Value, contract_script: &str) -> Transaction {
    let transaction = signed_funding_transaction(contract_script);
    let raw = serialize(&transaction);
    spec["bitcoin"]["funding_signed_transaction"] = serde_json::json!(hex::encode(&raw));
    spec["bitcoin"]["funding_signed_transaction_sha256"] =
        serde_json::json!(hex::encode(Sha256::digest(&raw)));
    spec["bitcoin"]["funding_input_script_pubkey"] = serde_json::json!(hex::encode(
        rawtr_input(&SecretKey::from_slice(&[0x07; 32]).unwrap()).as_bytes()
    ));
    spec["bitcoin"]["funding_transaction_id"] =
        serde_json::json!(transaction.compute_txid().to_string());
    transaction
}

fn rawtr_input(secret: &SecretKey) -> ScriptBuf {
    let keypair = Keypair::from_secret_key(&Secp256k1::new(), secret);
    ScriptBuf::new_p2tr_tweaked(TweakedPublicKey::dangerous_assume_tweaked(
        keypair.x_only_public_key().0,
    ))
}

fn funding_preparation_spec(
    direction: &str,
    public_sha256: &str,
    secret_file: &Path,
    input_script: &ScriptBuf,
) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "stage1_public_sha256": public_sha256,
        "direction": direction,
        "service_input": {
            "transaction_id": Txid::from_byte_array([0x31; 32]).to_string(),
            "output_index": 0,
            "value_sat": 200_000,
            "script_pubkey": hex::encode(input_script.as_bytes()),
            "signing_secret_key_file": secret_file
        },
        "contract_value_sat": 100_000,
        "fee_sat": 1_000
    })
}

fn provision(direction: &str) -> (TempDir, btc_local_poc_provision::Stage2Summary, Transaction) {
    let temp = tempfile::tempdir().unwrap();
    let planning_path = temp.path().join("planning.json");
    write_private_json(&planning_path, &planning());
    let output = temp.path().join(format!("{direction}-fixture"));
    let stage1 = generate_stage1(&planning_path, &output).unwrap();
    let public: serde_json::Value =
        serde_json::from_slice(&fs::read(stage1.public_spec_file()).unwrap()).unwrap();
    let aggregate = public["aggregate_internal_key"].as_str().unwrap();
    let service_secret = SecretKey::from_slice(&[0x07; 32]).unwrap();
    let service_secret_file = temp.path().join(format!("{direction}-service.key"));
    write_private_bytes(&service_secret_file, &service_secret.secret_bytes());
    let service_script = rawtr_input(&service_secret);
    let funding_spec_path = temp.path().join("funding.json");
    write_private_json(
        &funding_spec_path,
        &funding_preparation_spec(
            direction,
            stage1.public_spec_sha256(),
            &service_secret_file,
            &service_script,
        ),
    );
    let prepared = prepare_funding(&funding_spec_path, &output).unwrap();
    let raw_hex = fs::read_to_string(prepared.signed_transaction_file()).unwrap();
    let raw = hex::decode(raw_hex.trim()).unwrap();
    let transaction: Transaction = deserialize(&raw).unwrap();
    let mut spec = stage2_spec(direction, stage1.public_spec_sha256(), aggregate);
    spec["bitcoin"]["funding_signed_transaction"] = serde_json::json!(raw_hex.trim());
    spec["bitcoin"]["funding_signed_transaction_sha256"] =
        serde_json::json!(hex::encode(Sha256::digest(&raw)));
    spec["bitcoin"]["funding_input_value_sat"] = serde_json::json!(200_000);
    spec["bitcoin"]["funding_input_script_pubkey"] =
        serde_json::json!(hex::encode(service_script.as_bytes()));
    spec["bitcoin"]["funding_transaction_id"] =
        serde_json::json!(transaction.compute_txid().to_string());
    spec["bitcoin"]["funding_output_index"] = serde_json::json!(0);
    let stage2_path = temp.path().join("stage2.json");
    write_private_json(&stage2_path, &spec);
    let summary = finalize_stage2(&stage2_path, &output).unwrap();
    (temp, summary, transaction)
}

fn asset_extension_spec(agreement: &BtcAgreementV1) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "expected_agreement_sha256": hex::encode(Sha256::digest(
            agreement.encode_wire().unwrap()
        )),
        "expected_agreement_commitment": hex::encode(agreement.agreement_commitment()),
        "token_program_id": hex::encode([0x40; 32]),
        "ata_program_id": hex::encode([0x41; 32]),
        "token_definition_account": hex::encode([0x42; 32]),
        "depositor_ata_account": hex::encode([0x43; 32]),
        "claimant_ata_account": hex::encode([0x44; 32]),
        "custody_ata_account": hex::encode([0x45; 32])
    })
}

#[test]
fn countersigned_custom_token_extension_is_canonical_agreement_bound_and_secret_free() {
    let (temp, agreement_summary, _) = provision("taker_sells_foreign");
    let fixture_root = temp.path().join("taker_sells_foreign-fixture");
    let agreement_wire = fs::read(agreement_summary.agreement_file()).unwrap();
    let agreement = BtcAgreementV1::from_wire(&agreement_wire).unwrap();
    let spec_path = temp.path().join("asset-extension.json");
    write_private_json(&spec_path, &asset_extension_spec(&agreement));

    let summary = finalize_asset_extension(&spec_path, &fixture_root).unwrap();
    let extension_wire = fs::read(summary.asset_extension_file()).unwrap();
    let extension = BtcLezAssetExtensionV1::from_wire(&extension_wire, &agreement).unwrap();
    let BtcLezAssetV1::CustomToken(token) = extension.asset() else {
        panic!("expected custom-token extension")
    };
    assert_eq!(token.token_program_id(), &[0x40; 32]);
    assert_eq!(token.ata_program_id(), &[0x41; 32]);
    assert_eq!(token.token_definition_account(), &[0x42; 32]);
    assert_eq!(
        token.depositor_owner_account(),
        agreement.lez_terms().depositor_account()
    );
    assert_eq!(token.depositor_ata_account(), &[0x43; 32]);
    assert_eq!(
        token.claimant_owner_account(),
        agreement.lez_terms().claimant_account()
    );
    assert_eq!(token.claimant_ata_account(), &[0x44; 32]);
    assert_eq!(token.custody_ata_account(), &[0x45; 32]);
    assert_eq!(token.amount(), agreement.lez_terms().amount());
    assert_eq!(token.refund_at_ms(), agreement.lez_terms().refund_at_ms());
    assert_eq!(
        token.aggregate_authority_account(),
        agreement.lez_terms().aggregate_authority_account()
    );
    assert_eq!(
        token.aggregate_x_only_public_key(),
        &agreement.p2tr_contract().aggregate_internal_key_bytes()
    );
    assert_eq!(extension.encode_wire().unwrap(), extension_wire);

    let persisted_summary: serde_json::Value = serde_json::from_slice(
        &fs::read(fixture_root.join("lez-asset-extension-summary.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(persisted_summary["private_material_disclosed"], false);
    assert_eq!(persisted_summary["extension_revalidated"], true);
    assert_eq!(
        persisted_summary["asset_commitment"],
        hex::encode(extension.asset_commitment())
    );
    assert_eq!(
        fs::metadata(summary.asset_extension_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o600
    );
    assert!(finalize_asset_extension(&spec_path, &fixture_root).is_err());
}

#[test]
fn happy_path_revalidates_agreement_in_both_directions() {
    for direction in ["taker_sells_foreign", "taker_sells_lez"] {
        let (temp, summary, funding_transaction) = provision(direction);
        let wire = fs::read(summary.agreement_file()).unwrap();
        let agreement = BtcAgreementV1::from_wire(&wire).unwrap();
        assert_eq!(agreement.encode_wire().unwrap(), wire);
        assert_eq!(summary.direction(), direction);
        assert_eq!(
            bitcoin::Txid::from_byte_array(*agreement.funding_terms().transaction_id()).to_string(),
            funding_transaction.compute_txid().to_string()
        );
        let persisted = fs::read_to_string(
            temp.path()
                .join(format!("{direction}-fixture/funding-transaction.hex")),
        )
        .unwrap();
        assert_eq!(
            persisted.trim(),
            hex::encode(serialize(&funding_transaction))
        );
        let stage2_summary: serde_json::Value = serde_json::from_slice(
            &fs::read(
                temp.path()
                    .join(format!("{direction}-fixture/agreement-summary.json")),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(stage2_summary["bitcoin_funding_authorization"], "verified");
        assert_eq!(stage2_summary["bitcoin_node_state"], "not_asserted");
    }
}

#[test]
fn walletless_rawtr_funding_is_created_offline_for_both_directions() {
    for direction in ["taker_sells_foreign", "taker_sells_lez"] {
        let temp = tempfile::tempdir().unwrap();
        let planning_path = temp.path().join("planning.json");
        write_private_json(&planning_path, &planning());
        let output = temp.path().join(format!("{direction}-fixture"));
        let stage1 = generate_stage1(&planning_path, &output).unwrap();
        let public: serde_json::Value =
            serde_json::from_slice(&fs::read(stage1.public_spec_file()).unwrap()).unwrap();

        let secret = SecretKey::from_slice(&[0x07; 32]).unwrap();
        let secret_file = temp.path().join(format!("{direction}-service.key"));
        write_private_bytes(&secret_file, &secret.secret_bytes());
        let input_script = rawtr_input(&secret);
        let prepare_spec = funding_preparation_spec(
            direction,
            stage1.public_spec_sha256(),
            &secret_file,
            &input_script,
        );
        let prepare_path = temp.path().join(format!("{direction}-funding.json"));
        write_private_json(&prepare_path, &prepare_spec);
        let summary = prepare_funding(&prepare_path, &output).unwrap();

        let raw_hex = fs::read_to_string(summary.signed_transaction_file()).unwrap();
        let transaction: Transaction = deserialize(&hex::decode(raw_hex.trim()).unwrap()).unwrap();
        assert_eq!(transaction.input.len(), 1);
        assert_eq!(transaction.output.len(), 2);
        assert_eq!(
            transaction.input[0].previous_output.txid,
            Txid::from_byte_array([0x31; 32])
        );
        assert_eq!(transaction.input[0].previous_output.vout, 0);
        assert_eq!(transaction.output[0].value, Amount::from_sat(100_000));
        assert_eq!(
            hex::encode(transaction.output[0].script_pubkey.as_bytes()),
            public["contracts"][direction]["script_pubkey"]
                .as_str()
                .unwrap()
        );
        assert_eq!(transaction.output[1].value, Amount::from_sat(99_000));
        assert_eq!(transaction.output[1].script_pubkey, input_script);

        let witness = transaction.input[0].witness.iter().collect::<Vec<_>>();
        assert_eq!(witness.len(), 1);
        let signature = taproot::Signature::from_slice(witness[0]).unwrap();
        let prevouts = [TxOut {
            value: Amount::from_sat(200_000),
            script_pubkey: input_script,
        }];
        let sighash = SighashCache::new(&transaction)
            .taproot_key_spend_signature_hash(0, &Prevouts::All(&prevouts), TapSighashType::Default)
            .unwrap();
        Secp256k1::verification_only()
            .verify_schnorr(
                &signature.signature,
                &Message::from_digest(sighash.to_byte_array()),
                &Keypair::from_secret_key(&Secp256k1::new(), &secret)
                    .x_only_public_key()
                    .0,
            )
            .unwrap();

        let metadata = fs::metadata(summary.signed_transaction_file()).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
        assert_eq!(metadata.nlink(), 1);
        assert!(prepare_funding(&prepare_path, &output).is_err());
    }
}

#[test]
fn offline_funding_rejects_crosswired_service_key_and_unsafe_secret_file() {
    let temp = tempfile::tempdir().unwrap();
    let planning_path = temp.path().join("planning.json");
    write_private_json(&planning_path, &planning());
    let output = temp.path().join("fixture");
    let stage1 = generate_stage1(&planning_path, &output).unwrap();
    let secret = SecretKey::from_slice(&[0x07; 32]).unwrap();
    let different = SecretKey::from_slice(&[0x08; 32]).unwrap();
    let secret_file = temp.path().join("service.key");
    write_private_bytes(&secret_file, &secret.secret_bytes());

    let crosswired = funding_preparation_spec(
        "taker_sells_foreign",
        stage1.public_spec_sha256(),
        &secret_file,
        &rawtr_input(&different),
    );
    let crosswired_path = temp.path().join("crosswired.json");
    write_private_json(&crosswired_path, &crosswired);
    assert!(prepare_funding(&crosswired_path, &output).is_err());

    fs::set_permissions(&secret_file, fs::Permissions::from_mode(0o644)).unwrap();
    let unsafe_spec = funding_preparation_spec(
        "taker_sells_foreign",
        stage1.public_spec_sha256(),
        &secret_file,
        &rawtr_input(&secret),
    );
    let unsafe_path = temp.path().join("unsafe.json");
    write_private_json(&unsafe_path, &unsafe_spec);
    assert!(prepare_funding(&unsafe_path, &output).is_err());
}

#[test]
fn prepare_funding_cli_stdout_is_strict_secret_free_json() {
    let temp = tempfile::tempdir().unwrap();
    let planning_path = temp.path().join("planning.json");
    write_private_json(&planning_path, &planning());
    let output_root = temp.path().join("fixture");
    let stage1 = generate_stage1(&planning_path, &output_root).unwrap();
    let secret = SecretKey::from_slice(&[0x07; 32]).unwrap();
    let secret_file = temp.path().join("service.key");
    write_private_bytes(&secret_file, &secret.secret_bytes());
    let prepare_path = temp.path().join("funding.json");
    write_private_json(
        &prepare_path,
        &funding_preparation_spec(
            "taker_sells_foreign",
            stage1.public_spec_sha256(),
            &secret_file,
            &rawtr_input(&secret),
        ),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_btc-local-poc-provision"))
        .arg("prepare-funding")
        .arg("--spec-file")
        .arg(&prepare_path)
        .arg("--output-root")
        .arg(&output_root)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["private_material_disclosed"], false);
    assert_eq!(value["node_state_asserted"], false);
    assert!(value["contract_merkle_root"].as_str().is_some_and(|root| {
        root.len() == 64
            && root
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }));
    assert!(value.get("raw_transaction").is_none());
    assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 1);
    assert!(
        !output
            .stdout
            .windows(32)
            .any(|window| window == secret.secret_bytes())
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&hex::encode(secret.secret_bytes())));
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
    bind_funding(
        &mut spec,
        left_public["contracts"]["taker_sells_foreign"]["script_pubkey"]
            .as_str()
            .unwrap(),
    );
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
    bind_funding(
        &mut private_crosswire,
        left_public["contracts"]["taker_sells_foreign"]["script_pubkey"]
            .as_str()
            .unwrap(),
    );
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
    bind_funding(
        &mut spec,
        public["contracts"]["taker_sells_foreign"]["script_pubkey"]
            .as_str()
            .unwrap(),
    );
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
    let script = public["contracts"]["taker_sells_foreign"]["script_pubkey"]
        .as_str()
        .unwrap();

    let mut bad_authority = stage2_spec(
        "taker_sells_foreign",
        summary.public_spec_sha256(),
        aggregate,
    );
    bind_funding(&mut bad_authority, script);
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
    bind_funding(&mut bad_schedule, script);
    bad_schedule["recovery"]["bitcoin_refund_height"] = serde_json::json!(1_145);
    let bad_schedule_path = temp.path().join("bad-schedule.json");
    write_private_json(&bad_schedule_path, &bad_schedule);
    assert!(finalize_stage2(&bad_schedule_path, &output).is_err());

    let mut premature = stage2_spec(
        "taker_sells_foreign",
        summary.public_spec_sha256(),
        aggregate,
    );
    bind_funding(&mut premature, script);
    premature["bitcoin"]["observed_confirmations"] = serde_json::json!(0);
    let premature_path = temp.path().join("premature.json");
    write_private_json(&premature_path, &premature);
    assert!(finalize_stage2(&premature_path, &output).is_err());

    let mut unknown = stage2_spec(
        "taker_sells_foreign",
        summary.public_spec_sha256(),
        aggregate,
    );
    bind_funding(&mut unknown, script);
    unknown["lez_terms"]["unexpected"] = serde_json::json!(true);
    let unknown_path = temp.path().join("unknown.json");
    write_private_json(&unknown_path, &unknown);
    assert!(finalize_stage2(&unknown_path, &output).is_err());

    let mut valid = stage2_spec(
        "taker_sells_foreign",
        summary.public_spec_sha256(),
        aggregate,
    );
    bind_funding(&mut valid, script);
    let valid_path = temp.path().join("valid.json");
    write_private_json(&valid_path, &valid);
    finalize_stage2(&valid_path, &output).unwrap();
    assert!(finalize_stage2(&valid_path, &output).is_err());
}

#[test]
fn prelock_funding_rejects_malformed_or_crosswired_exact_transaction_facts() {
    let temp = tempfile::tempdir().unwrap();
    let planning_path = temp.path().join("planning.json");
    write_private_json(&planning_path, &planning());
    let output = temp.path().join("fixture");
    let stage1 = generate_stage1(&planning_path, &output).unwrap();
    let public: serde_json::Value =
        serde_json::from_slice(&fs::read(stage1.public_spec_file()).unwrap()).unwrap();
    let aggregate = public["aggregate_internal_key"].as_str().unwrap();
    let contract_script = public["contracts"]["taker_sells_foreign"]["script_pubkey"]
        .as_str()
        .unwrap();
    let mut valid = stage2_spec(
        "taker_sells_foreign",
        stage1.public_spec_sha256(),
        aggregate,
    );
    let transaction = bind_funding(&mut valid, contract_script);

    let mut cases = Vec::new();

    let mut malformed = valid.clone();
    malformed["bitcoin"]["funding_signed_transaction"] = serde_json::json!("00");
    cases.push(("malformed", malformed));

    let mut trailing = valid.clone();
    trailing["bitcoin"]["funding_signed_transaction"] =
        serde_json::json!(format!("{}ff", hex::encode(serialize(&transaction))));
    cases.push(("trailing", trailing));

    let mut txid_mismatch = valid.clone();
    txid_mismatch["bitcoin"]["funding_transaction_id"] =
        serde_json::json!(Txid::from_byte_array([0x42; 32]).to_string());
    cases.push(("txid-mismatch", txid_mismatch));

    let mut wrong_vout = valid.clone();
    wrong_vout["bitcoin"]["funding_output_index"] = serde_json::json!(2);
    cases.push(("wrong-vout", wrong_vout));

    let mut wrong_value = valid.clone();
    wrong_value["bitcoin"]["funding_value_sat"] = serde_json::json!(99_999);
    cases.push(("wrong-value", wrong_value));

    let mut wrong_input_value = valid.clone();
    wrong_input_value["bitcoin"]["funding_input_value_sat"] = serde_json::json!(126_001);
    cases.push(("wrong-input-value", wrong_input_value));

    let mut wrong_input_script = valid.clone();
    wrong_input_script["bitcoin"]["funding_input_script_pubkey"] = serde_json::json!(hex::encode(
        rawtr_input(&SecretKey::from_slice(&[0x08; 32]).unwrap()).as_bytes()
    ));
    cases.push(("wrong-input-script", wrong_input_script));

    let mut wrong_hash = valid.clone();
    wrong_hash["bitcoin"]["funding_signed_transaction_sha256"] =
        serde_json::json!(hex::encode([0x55; 32]));
    cases.push(("wrong-hash", wrong_hash));

    let mut wrong_script_tx = transaction.clone();
    wrong_script_tx.output[1].script_pubkey = ScriptBuf::from_bytes(vec![0x51]);
    let mut wrong_script = valid.clone();
    wrong_script["bitcoin"]["funding_signed_transaction"] =
        serde_json::json!(hex::encode(serialize(&wrong_script_tx)));
    wrong_script["bitcoin"]["funding_signed_transaction_sha256"] =
        serde_json::json!(hex::encode(Sha256::digest(serialize(&wrong_script_tx))));
    wrong_script["bitcoin"]["funding_transaction_id"] =
        serde_json::json!(wrong_script_tx.compute_txid().to_string());
    cases.push(("wrong-script", wrong_script));

    let mut unsigned_tx = transaction;
    unsigned_tx.input[0].witness = Witness::new();
    let mut unsigned = valid.clone();
    unsigned["bitcoin"]["funding_signed_transaction"] =
        serde_json::json!(hex::encode(serialize(&unsigned_tx)));
    unsigned["bitcoin"]["funding_signed_transaction_sha256"] =
        serde_json::json!(hex::encode(Sha256::digest(serialize(&unsigned_tx))));
    unsigned["bitcoin"]["funding_transaction_id"] =
        serde_json::json!(unsigned_tx.compute_txid().to_string());
    cases.push(("unsigned", unsigned));

    let mut bad_signature_tx = valid["bitcoin"]["funding_signed_transaction"]
        .as_str()
        .and_then(|raw| hex::decode(raw).ok())
        .and_then(|raw| deserialize::<Transaction>(&raw).ok())
        .unwrap();
    let mut bad_signature = bad_signature_tx.input[0].witness[0].to_vec();
    bad_signature[0] ^= 1;
    bad_signature_tx.input[0].witness = Witness::from_slice(&[bad_signature]);
    let bad_signature_raw = serialize(&bad_signature_tx);
    let mut invalid_signature = valid.clone();
    invalid_signature["bitcoin"]["funding_signed_transaction"] =
        serde_json::json!(hex::encode(&bad_signature_raw));
    invalid_signature["bitcoin"]["funding_signed_transaction_sha256"] =
        serde_json::json!(hex::encode(Sha256::digest(&bad_signature_raw)));
    cases.push(("invalid-signature", invalid_signature));

    for (name, spec) in cases {
        let path = temp.path().join(format!("{name}.json"));
        write_private_json(&path, &spec);
        assert!(
            finalize_stage2(&path, &output).is_err(),
            "accepted invalid pre-lock funding case {name}"
        );
    }
}

#[test]
fn prelock_schema_cannot_assert_broadcast_or_confirmation_state() {
    let temp = tempfile::tempdir().unwrap();
    let planning_path = temp.path().join("planning.json");
    write_private_json(&planning_path, &planning());
    let output = temp.path().join("fixture");
    let stage1 = generate_stage1(&planning_path, &output).unwrap();
    let public: serde_json::Value =
        serde_json::from_slice(&fs::read(stage1.public_spec_file()).unwrap()).unwrap();
    let mut spec = stage2_spec(
        "taker_sells_foreign",
        stage1.public_spec_sha256(),
        public["aggregate_internal_key"].as_str().unwrap(),
    );
    bind_funding(
        &mut spec,
        public["contracts"]["taker_sells_foreign"]["script_pubkey"]
            .as_str()
            .unwrap(),
    );

    for (name, field, value) in [
        ("broadcast", "broadcast", serde_json::json!(false)),
        (
            "confirmations",
            "observed_confirmations",
            serde_json::json!(0),
        ),
        (
            "anchor-observation",
            "funding_anchor_height",
            serde_json::json!(1_000),
        ),
        (
            "script-claim",
            "funding_script_pubkey",
            serde_json::json!(public["contracts"]["taker_sells_foreign"]["script_pubkey"]),
        ),
    ] {
        let mut premature = spec.clone();
        premature["bitcoin"][field] = value;
        let path = temp.path().join(format!("{name}.json"));
        write_private_json(&path, &premature);
        assert!(
            finalize_stage2(&path, &output).is_err(),
            "accepted premature node-state field {field}"
        );
    }
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
