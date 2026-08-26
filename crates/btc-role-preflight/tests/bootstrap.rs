#![cfg(unix)]

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
};

use bitcoin::secp256k1::{Keypair, Message, Secp256k1, SecretKey};
use bitcoin::{Amount, OutPoint, ScriptBuf, TxOut, Txid, hashes::Hash as _};
use btc_role_preflight::{
    bind_countersigned_agreement, bootstrap_role, compose_agreement_draft,
    persist_and_bind_countersigned_agreement,
};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, BTC_AGREEMENT_SCHEMA_V1, BtcAgreementBodyV1, BtcAgreementRecordV1,
    BtcClaimTermsV1, BtcFundingTermsV1, BtcLezTermsV1, BtcP2trTermsV1, BtcRecoveryPlanV1,
    BtcRoleContributionPairV1, BtcRoleContributionV1, CooperativeKeyPathSpend, CsvBlockDelay,
    P2trSwapOutput, RefundXOnlyKey, TwoPartyAggregateKey,
};
use lez_swap_core::{Participant, SwapDirection};

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

fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

fn role_spec(role: &str, owner: u8) -> serde_json::Value {
    role_spec_for_direction(role, owner, "taker_sells_foreign")
}

fn role_spec_for_direction(role: &str, owner: u8, direction: &str) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "role": role,
        "direction": direction,
        "offer_commitment": hex::encode([0x20; 32]),
        "reservation_binding": hex::encode(b"delivery-reservation-17"),
        "bitcoin": {
            "genesis_block_hash": hex::encode([0x21; 32]),
            "required_confirmations": 2
        },
        "lez": {
            "genesis_block_hash": hex::encode([0x22; 32]),
            "channel_id": hex::encode([0x23; 32]),
            "escrow_program_id": hex::encode([0x24; 32]),
            "authenticated_transfer_program_id": hex::encode([0x25; 32])
        },
        "lez_owner_account": hex::encode([owner; 32]),
        "expires_at_unix_seconds": 2_000_000_000_u64
    })
}

fn draft_spec(direction: &str) -> serde_json::Value {
    let lez_refund_at_ms = match direction {
        "taker_sells_foreign" => 1_700_000_100_000_u64,
        "taker_sells_lez" => 1_700_000_500_000_u64,
        _ => unreachable!(),
    };
    serde_json::json!({
        "schema_version": 1,
        "bitcoin": {
            "funding_transaction_id": Txid::from_byte_array([0x62; 32]).to_string(),
            "funding_output_index": 0,
            "funding_value_sat": 100_000,
            "claim_value_sat": 99_000,
            "refund_csv_blocks": 144
        },
        "lez": {
            "aggregate_authority_account": hex::encode([0x63; 32]),
            "metadata_account": hex::encode([0x64; 32]),
            "custody_account": hex::encode([0x65; 32]),
            "amount": 5_000,
            "refund_at_ms": lez_refund_at_ms,
            "prepared_claim_message_hash": hex::encode([0x66; 32])
        },
        "recovery": {
            "planned_bitcoin_funding_anchor_height": 1_000,
            "bitcoin_refund_height": 1_144,
            "maker_second_lock_cutoff_unix_seconds": 1_699_999_800_u64,
            "earlier_refund_latest_unix_seconds": 1_700_000_100_u64,
            "later_refund_earliest_unix_seconds": 1_700_000_500_u64,
            "required_margin_seconds": 300
        }
    })
}

fn secret(path: &Path) -> SecretKey {
    SecretKey::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn agreement_signature(secret: &SecretKey, commitment: [u8; 32]) -> [u8; 64] {
    let secp = Secp256k1::new();
    secp.sign_schnorr_no_aux_rand(
        &Message::from_digest(commitment),
        &Keypair::from_secret_key(&secp, secret),
    )
    .serialize()
}

#[allow(clippy::too_many_lines)]
fn countersigned_agreement(maker_root: &Path, taker_root: &Path) -> Vec<u8> {
    let maker =
        BtcRoleContributionV1::from_wire(&fs::read(maker_root.join("contribution.borsh")).unwrap())
            .unwrap();
    let taker =
        BtcRoleContributionV1::from_wire(&fs::read(taker_root.join("contribution.borsh")).unwrap())
            .unwrap();
    let pair = BtcRoleContributionPairV1::new(maker, taker).unwrap();
    let adaptor = *pair.adaptor_point();
    let aggregate = AdaptorSessionContext::untweaked(
        [
            *pair
                .maker()
                .body()
                .participant_identity()
                .musig2_public_key(),
            *pair
                .taker()
                .body()
                .participant_identity()
                .musig2_public_key(),
        ],
        [0x60; 32],
        adaptor,
        [0x61; 32],
    )
    .unwrap()
    .output_key();
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(aggregate).unwrap(),
        RefundXOnlyKey::from_bytes(
            *pair
                .taker()
                .body()
                .participant_identity()
                .bitcoin_refund_key(),
        )
        .unwrap(),
        CsvBlockDelay::new(144).unwrap(),
    )
    .unwrap();
    let funding = BtcFundingTermsV1::new([0x62; 32], 0, 100_000);
    let spend = CooperativeKeyPathSpend::new(
        &contract,
        OutPoint {
            txid: Txid::from_byte_array(*funding.transaction_id()),
            vout: funding.output_index(),
        },
        Amount::from_sat(funding.value_sat()),
        vec![TxOut {
            value: Amount::from_sat(99_000),
            script_pubkey: ScriptBuf::from_bytes(
                pair.maker()
                    .body()
                    .participant_identity()
                    .claim_destination_script_pubkey()
                    .to_vec(),
            ),
        }],
    )
    .unwrap();
    let lez_chain = pair.maker().body().lez_chain_identity();
    let participants = pair.participants();
    let body = BtcAgreementBodyV1::new(
        *pair.swap_id(),
        SwapDirection::TakerSellsForeign,
        *pair.maker().body().bitcoin_chain_policy(),
        participants.clone(),
        adaptor,
        BtcLezTermsV1::new(
            *lez_chain.channel_id(),
            *lez_chain.genesis_block_hash(),
            *lez_chain.escrow_program_id(),
            *lez_chain.authenticated_transfer_program_id(),
            [0x63; 32],
            [0x64; 32],
            [0x65; 32],
            *participants
                .for_participant(Participant::Maker)
                .lez_owner_account(),
            *participants
                .for_participant(Participant::Taker)
                .lez_owner_account(),
            5_000,
            1_700_000_100_000,
            [0x66; 32],
        ),
        BtcP2trTermsV1::from_contract(&contract),
        funding,
        BtcClaimTermsV1::from_spend(&spend).unwrap(),
        BtcRecoveryPlanV1::new(
            1_000,
            1_144,
            1_699_999_800,
            1_700_000_100,
            1_700_000_500,
            300,
        ),
    );
    let commitment = body.commitment();
    BtcAgreementRecordV1::from_parts(
        BTC_AGREEMENT_SCHEMA_V1,
        body,
        commitment,
        agreement_signature(
            &secret(&maker_root.join("private/agreement.key")),
            commitment,
        ),
        agreement_signature(
            &secret(&taker_root.join("private/agreement.key")),
            commitment,
        ),
    )
    .encode_wire()
    .unwrap()
}

#[test]
fn separate_role_roots_form_one_joint_pair_without_peer_authority() {
    let temp = tempfile::tempdir().unwrap();
    let canonical_temp = fs::canonicalize(temp.path()).unwrap();
    let maker_spec = canonical_temp.join("maker.json");
    let taker_spec = canonical_temp.join("taker.json");
    write_private_json(&maker_spec, &role_spec("maker", 0x31));
    write_private_json(&taker_spec, &role_spec("taker", 0x32));
    let maker_root = canonical_temp.join("maker-role");
    let taker_root = canonical_temp.join("taker-role");

    let maker_summary = bootstrap_role(&maker_spec, &maker_root).unwrap();
    let taker_summary = bootstrap_role(&taker_spec, &taker_root).unwrap();
    let maker_wire = fs::read(maker_summary.contribution_file()).unwrap();
    let taker_wire = fs::read(taker_summary.contribution_file()).unwrap();
    let pair = BtcRoleContributionPairV1::new(
        BtcRoleContributionV1::from_wire(&maker_wire).unwrap(),
        BtcRoleContributionV1::from_wire(&taker_wire).unwrap(),
    )
    .unwrap();
    assert_ne!(pair.swap_id(), &[0; 32]);
    assert!(maker_root.join("private/agreement.key").is_file());
    assert!(taker_root.join("private/agreement.key").is_file());
    assert!(!maker_root.join("private/adaptor-scalar.key").exists());
    assert!(taker_root.join("private/adaptor-scalar.key").is_file());
    assert_eq!(fs::read_dir(maker_root.join("private")).unwrap().count(), 4);
    assert_eq!(fs::read_dir(taker_root.join("private")).unwrap().count(), 5);
    assert_ne!(
        fs::read(maker_root.join("private/agreement.key")).unwrap(),
        fs::read(taker_root.join("private/agreement.key")).unwrap()
    );

    for root in [&maker_root, &taker_root] {
        assert_eq!(
            fs::metadata(root).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        for entry in fs::read_dir(root.join("private")).unwrap() {
            let metadata = entry.unwrap().metadata().unwrap();
            assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
        }
        assert_eq!(
            fs::metadata(root.join("contribution.borsh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o600
        );
    }
    assert!(bootstrap_role(&maker_spec, &maker_root).is_err());
    assert!(bootstrap_role(&taker_spec, &taker_root).is_err());
}

#[test]
fn persisted_and_printable_summaries_do_not_disclose_role_secrets() {
    let temp = tempfile::tempdir().unwrap();
    let canonical_temp = fs::canonicalize(temp.path()).unwrap();
    let spec = canonical_temp.join("maker.json");
    write_private_json(&spec, &role_spec("maker", 0x41));
    let root = canonical_temp.join("maker-role");
    let summary = bootstrap_role(&spec, &root).unwrap();
    let printable = serde_json::to_string(&summary).unwrap();
    let persisted = fs::read_to_string(summary.summary_file()).unwrap();
    let private_root = root.join("private");
    for entry in fs::read_dir(private_root).unwrap() {
        let secret = fs::read(entry.unwrap().path()).unwrap();
        let encoded = hex::encode(secret);
        assert!(!printable.contains(&encoded));
        assert!(!persisted.contains(&encoded));
    }
    let json: serde_json::Value = serde_json::from_str(&persisted).unwrap();
    assert_eq!(json["private_material_disclosed"], false);
    assert_eq!(json["peer_private_material_created"], false);
    assert!(json.get("private_root").is_none());
}

#[test]
fn crosswired_chain_and_session_inputs_fail_before_creating_authority() {
    let temp = tempfile::tempdir().unwrap();
    let canonical_temp = fs::canonicalize(temp.path()).unwrap();
    let bad_spec = canonical_temp.join("bad.json");
    let mut value = role_spec("maker", 0x51);
    value["lez"]["authenticated_transfer_program_id"] = value["lez"]["escrow_program_id"].clone();
    write_private_json(&bad_spec, &value);
    let output = canonical_temp.join("bad-role");
    assert!(bootstrap_role(&bad_spec, &output).is_err());
    assert!(!output.exists());
}

#[test]
#[allow(clippy::too_many_lines)]
fn each_role_rebinds_the_countersigned_agreement_to_its_own_private_authority() {
    let temp = tempfile::tempdir().unwrap();
    let canonical_temp = fs::canonicalize(temp.path()).unwrap();
    let maker_spec = canonical_temp.join("maker.json");
    let taker_spec = canonical_temp.join("taker.json");
    write_private_json(&maker_spec, &role_spec("maker", 0x71));
    write_private_json(&taker_spec, &role_spec("taker", 0x72));
    let maker_root = canonical_temp.join("maker-role");
    let taker_root = canonical_temp.join("taker-role");
    bootstrap_role(&maker_spec, &maker_root).unwrap();
    bootstrap_role(&taker_spec, &taker_root).unwrap();
    let agreement_wire = countersigned_agreement(&maker_root, &taker_root);
    let external_agreement = canonical_temp.join("external-agreement.borsh");
    write_private(&external_agreement, &agreement_wire);
    let maker_contribution = fs::read(maker_root.join("contribution.borsh")).unwrap();
    let taker_contribution = fs::read(taker_root.join("contribution.borsh")).unwrap();
    let mut invalid_agreement = agreement_wire.clone();
    invalid_agreement[0] ^= 1;
    assert!(
        persist_and_bind_countersigned_agreement(
            &maker_root,
            &taker_contribution,
            &invalid_agreement,
            1_900_000_000,
        )
        .is_err()
    );
    assert!(!maker_root.join("peer-contribution.borsh").exists());
    assert!(!maker_root.join("agreement.borsh").exists());
    assert!(!maker_root.join("agreement-binding.json").exists());

    let maker = persist_and_bind_countersigned_agreement(
        &maker_root,
        &taker_contribution,
        &agreement_wire,
        1_900_000_000,
    )
    .unwrap();
    let taker = bind_countersigned_agreement(
        &taker_root,
        &maker_root.join("contribution.borsh"),
        &external_agreement,
        1_900_000_000,
    )
    .unwrap();
    let maker_json: serde_json::Value =
        serde_json::from_slice(&fs::read(maker.binding_file()).unwrap()).unwrap();
    let taker_json: serde_json::Value =
        serde_json::from_slice(&fs::read(taker.binding_file()).unwrap()).unwrap();
    assert_eq!(maker_json["swap_id"], taker_json["swap_id"]);
    assert_eq!(maker_json["local_private_authority_revalidated"], true);
    assert_eq!(taker_json["local_private_authority_revalidated"], true);
    assert_eq!(maker_json["ready_for_public_effects"], false);
    assert_eq!(taker_json["ready_for_public_effects"], false);
    assert_eq!(
        taker_json["agreement_file"],
        taker_root
            .join("agreement.borsh")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(
        fs::read(maker_root.join("agreement.borsh")).unwrap(),
        agreement_wire
    );
    let taker_cross_entry_replay = persist_and_bind_countersigned_agreement(
        &taker_root,
        &maker_contribution,
        &agreement_wire,
        2_100_000_000,
    )
    .unwrap();
    assert!(taker_cross_entry_replay.was_replay());
    assert_eq!(
        taker_cross_entry_replay.accepted_at_unix_seconds(),
        1_900_000_000
    );
    assert_eq!(
        fs::read(taker_root.join("peer-contribution.borsh")).unwrap(),
        maker_contribution
    );
    // A crash may publish the inactive acceptance receipt before both exact
    // artifacts reach the role root. Exact replay repairs that safe prefix
    // even after contribution expiry.
    fs::remove_file(maker_root.join("peer-contribution.borsh")).unwrap();
    fs::remove_file(maker_root.join("agreement.borsh")).unwrap();
    let replay = persist_and_bind_countersigned_agreement(
        &maker_root,
        &taker_contribution,
        &agreement_wire,
        2_100_000_000,
    )
    .unwrap();
    assert!(replay.was_replay());
    assert_eq!(replay.accepted_at_unix_seconds(), 1_900_000_000);
    assert!(!replay.ready_for_public_effects());
    assert_eq!(
        fs::read(maker_root.join("peer-contribution.borsh")).unwrap(),
        taker_contribution
    );
    assert_eq!(
        fs::read(maker_root.join("agreement.borsh")).unwrap(),
        agreement_wire
    );

    let mut substituted_agreement = agreement_wire;
    substituted_agreement[0] ^= 1;
    assert!(
        persist_and_bind_countersigned_agreement(
            &maker_root,
            &taker_contribution,
            &substituted_agreement,
            1_900_000_000,
        )
        .is_err()
    );
}

#[test]
fn unsigned_draft_is_composed_from_public_contributions_for_both_directions() {
    for (direction, maker_owner, taker_owner) in [
        ("taker_sells_foreign", 0x81, 0x91),
        ("taker_sells_lez", 0x82, 0x92),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let canonical_temp = fs::canonicalize(temp.path()).unwrap();
        let maker_spec = canonical_temp.join("maker.json");
        let taker_spec = canonical_temp.join("taker.json");
        let facts_file = canonical_temp.join("draft.json");
        write_private_json(
            &maker_spec,
            &role_spec_for_direction("maker", maker_owner, direction),
        );
        write_private_json(
            &taker_spec,
            &role_spec_for_direction("taker", taker_owner, direction),
        );
        write_private_json(&facts_file, &draft_spec(direction));
        let maker_root = canonical_temp.join("maker-role");
        let taker_root = canonical_temp.join("taker-role");
        bootstrap_role(&maker_spec, &maker_root).unwrap();
        bootstrap_role(&taker_spec, &taker_root).unwrap();
        let draft_root = canonical_temp.join("draft");

        let summary = compose_agreement_draft(
            &facts_file,
            &maker_root.join("contribution.borsh"),
            &taker_root.join("contribution.borsh"),
            &draft_root,
        )
        .unwrap();
        let draft = lez_btc_swap_sdk::BtcAgreementDraftV1::from_wire(
            &fs::read(summary.draft_file()).unwrap(),
        )
        .unwrap();
        let pair = BtcRoleContributionPairV1::new(
            BtcRoleContributionV1::from_wire(
                &fs::read(maker_root.join("contribution.borsh")).unwrap(),
            )
            .unwrap(),
            BtcRoleContributionV1::from_wire(
                &fs::read(taker_root.join("contribution.borsh")).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        pair.validate_agreement_body_fields(draft.body()).unwrap();
        let persisted_summary =
            fs::read_to_string(draft_root.join("unsigned-draft-summary.json")).unwrap();
        assert!(persisted_summary.contains("\"private_material_disclosed\": false"));
        assert!(!draft_root.join("private").exists());
        assert_eq!(fs::read_dir(&draft_root).unwrap().count(), 2);
        assert!(
            compose_agreement_draft(
                &facts_file,
                &maker_root.join("contribution.borsh"),
                &taker_root.join("contribution.borsh"),
                &draft_root,
            )
            .is_err()
        );
    }
}
