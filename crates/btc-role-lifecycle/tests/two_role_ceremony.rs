//! Two role roots bootstrap in-process, countersign a draft, run the two-leg
//! ceremony round by round as the Chat methods will, and each synthesizes an
//! actor configuration the actor's strict loader accepts.

#![allow(
    clippy::too_many_lines,
    reason = "the journey stays linear so the round order remains auditable"
)]

use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::Write as _,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::Path,
};

use bitcoin::secp256k1::{Keypair, Message, Secp256k1, SecretKey};
use btc_role_preflight::{
    AgreementDraftFacts, RoleBootstrapInput, RoleSecret, bootstrap_role_in_process,
    compose_agreement_draft_wire, persist_and_bind_countersigned_agreement,
};
use lez_adaptor_signature::verify_adaptor_presignature;
use lez_bridge_protocol::RequestId;
use lez_btc_role_lifecycle::{
    BtcRoleRuntime, LegSessions, MakerCeremony, SwapLayout, SwapSidecar, TakerCeremony,
    actor::{ActorSynthesis, synthesize},
    sidecar::{SidecarRecordV1, swap_run_id},
};
use lez_btc_swap_sdk::{
    BtcAgreementDraftV1, BtcAgreementV1, BtcChainPolicyV1, BtcLezChainIdentityV1,
    BtcMakerAgreementProposalV1,
};
use lez_swap_core::{Participant, SwapDirection};

fn private_dir(path: &Path) {
    DirBuilder::new().mode(0o700).create(path).unwrap();
}

fn private_file(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

fn bootstrap_input(role: Participant, owner: u8) -> RoleBootstrapInput {
    RoleBootstrapInput {
        role,
        direction: SwapDirection::TakerSellsForeign,
        offer_commitment: [0x20; 32],
        reservation_binding: b"delivery-reservation-17".to_vec(),
        bitcoin: BtcChainPolicyV1::new(
            bitcoin::hashes::Hash::to_byte_array(
                "0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206"
                    .parse::<bitcoin::BlockHash>()
                    .unwrap(),
            ),
            2,
        ),
        lez: BtcLezChainIdentityV1::new([0x22; 32], [0x23; 32], [0x24; 32], [0x25; 32]),
        lez_owner_account: [owner; 32],
        expires_at_unix_seconds: 2_000_000_000,
    }
}

fn schnorr(secret: &[u8; 32], commitment: [u8; 32]) -> [u8; 64] {
    let secp = Secp256k1::new();
    secp.sign_schnorr_no_aux_rand(
        &Message::from_digest(commitment),
        &Keypair::from_secret_key(&secp, &SecretKey::from_slice(secret).unwrap()),
    )
    .serialize()
}

fn role_config(root: &Path, wallet: Option<&str>) -> std::path::PathBuf {
    let swaps = root.join("swaps");
    private_dir(&swaps);
    private_file(&root.join("cookie"), b"user:pass\n");
    private_file(
        &root.join("capability"),
        b"0123456789abcdef0123456789abcdef\n",
    );
    private_file(
        &root.join("lez-signer.key"),
        format!("{}\n", hex::encode([0x77; 32])).as_bytes(),
    );
    let config = serde_json::json!({
        "schema_version": 1,
        "swaps_root": swaps,
        "bitcoin": {
            "network": "regtest",
            "endpoint": "http://127.0.0.1:18443/",
            "cookie_file": root.join("cookie"),
            "wallet": wallet,
            "genesis_block_hash": "0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206",
            "required_confirmations": 2,
            "refund_csv_blocks": 144,
            "claim_fee_sat": 1000
        },
        "lez": {
            "channel_id": hex::encode([0x23; 32]),
            "genesis_block_hash": hex::encode([0x22; 32]),
            "escrow_program_id": hex::encode([0x24; 32]),
            "authenticated_transfer_program_id": hex::encode([0x25; 32]),
            "sidecar_program": "/usr/local/bin/lez-v02-bridge-poc",
            "sequencer_url": "http://127.0.0.1:3040",
            "indexer_url": "http://127.0.0.1:8779",
            "sidecar_port_base": 19000,
            "sidecar_port_count": 100,
            "signer_key_file": root.join("lez-signer.key"),
            "request_timeout_millis": 5000,
            "discovery_max_blocks": 512
        },
        "recovery": {
            "maker_second_lock_cutoff_seconds": 1800,
            "earlier_refund_latest_seconds": 3600,
            "later_refund_earliest_seconds": 7200,
            "required_margin_seconds": 300
        },
        "actor": { "program": "/usr/local/bin/lez-btc-actor", "program_sha256": hex::encode([0x11; 32]) }
    });
    let path = root.join("btc-role.json");
    private_file(
        &path,
        serde_json::to_vec_pretty(&config).unwrap().as_slice(),
    );
    path
}

#[test]
fn both_roles_converge_and_synthesize_actor_configurations() {
    let directory = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(directory.path()).unwrap();
    let maker_home = base.join("maker");
    let taker_home = base.join("taker");
    private_dir(&maker_home);
    private_dir(&taker_home);
    let maker_runtime =
        BtcRoleRuntime::load(Participant::Maker, &role_config(&maker_home, None)).unwrap();
    let taker_runtime =
        BtcRoleRuntime::load(Participant::Taker, &role_config(&taker_home, Some("taker"))).unwrap();
    let reservation = RequestId::new("m9-reservation-0001").unwrap();
    let maker_layout = SwapLayout::new(&maker_runtime.config().swaps_root, &reservation);
    let taker_layout = SwapLayout::new(&taker_runtime.config().swaps_root, &reservation);
    maker_layout.create().unwrap();
    taker_layout.create().unwrap();

    // Reservation: both roles bootstrap; the Maker reuses its offer-bound key.
    let maker_key = zeroize::Zeroizing::new([0x31; 32]);
    let maker = bootstrap_role_in_process(
        &bootstrap_input(Participant::Maker, 0x41),
        Some(maker_key.clone()),
        maker_layout.role_root().root(),
    )
    .unwrap();
    let taker = bootstrap_role_in_process(
        &bootstrap_input(Participant::Taker, 0x42),
        None,
        taker_layout.role_root().root(),
    )
    .unwrap();
    assert_eq!(
        *maker_layout
            .role_root()
            .read_secret(RoleSecret::Agreement)
            .unwrap(),
        *maker_key
    );

    // Draft, proposal, countersignature, binding on both roots.
    let facts = AgreementDraftFacts {
        funding_transaction_id: [0x62; 32],
        funding_output_index: 0,
        funding_value_sat: 100_000,
        claim_value_sat: 99_000,
        refund_csv_blocks: 144,
        lez_aggregate_authority_account: [0x63; 32],
        lez_metadata_account: [0x64; 32],
        lez_custody_account: [0x65; 32],
        lez_amount: 5_000,
        lez_refund_at_ms: 1_700_000_100_000,
        lez_prepared_claim_message_hash: [0x66; 32],
        planned_bitcoin_funding_anchor_height: 1_000,
        bitcoin_refund_height: 1_144,
        maker_second_lock_cutoff_unix_seconds: 1_699_999_800,
        earlier_refund_latest_unix_seconds: 1_700_000_100,
        later_refund_earliest_unix_seconds: 1_700_000_500,
        required_margin_seconds: 300,
    };
    let composed =
        compose_agreement_draft_wire(&facts, &maker.contribution_wire, &taker.contribution_wire)
            .unwrap();
    let draft = BtcAgreementDraftV1::from_wire(&composed.wire).unwrap();
    let proposal = BtcMakerAgreementProposalV1::from_parts(
        draft,
        schnorr(&maker_key, composed.agreement_commitment),
    )
    .unwrap();
    let taker_key = taker_layout
        .role_root()
        .read_secret(RoleSecret::Agreement)
        .unwrap();
    let agreement = proposal
        .complete(schnorr(&taker_key, composed.agreement_commitment))
        .unwrap();
    let agreement_wire = agreement.encode_wire().unwrap();
    persist_and_bind_countersigned_agreement(
        maker_layout.role_root().root(),
        &taker.contribution_wire,
        &agreement_wire,
        1_700_000_000,
    )
    .unwrap();
    persist_and_bind_countersigned_agreement(
        taker_layout.role_root().root(),
        &maker.contribution_wire,
        &agreement_wire,
        1_700_000_000,
    )
    .unwrap();
    let agreement = BtcAgreementV1::from_wire(&agreement_wire).unwrap();

    // Ceremony: three Taker-initiated rounds, every one replayed once.
    let sessions = LegSessions::fresh().unwrap();
    let mut taker_seat = TakerCeremony::open(&taker_layout, &agreement, &sessions).unwrap();
    let mut maker_seat = MakerCeremony::open(&maker_layout, &agreement, &sessions).unwrap();
    let taker_commitments = taker_seat.commitments(&taker_key).unwrap();
    let maker_commitments = maker_seat
        .reserve_round(&taker_commitments, &maker_key)
        .unwrap();
    assert_eq!(
        maker_seat
            .reserve_round(&taker_commitments, &maker_key)
            .unwrap(),
        maker_commitments
    );
    taker_seat
        .accept_maker_commitments(&maker_commitments)
        .unwrap();
    let taker_nonces = taker_seat.nonces().unwrap();
    let (maker_nonces, maker_partials) = maker_seat.nonce_round(&taker_nonces, &maker_key).unwrap();
    assert_eq!(
        maker_seat.nonce_round(&taker_nonces, &maker_key).unwrap(),
        (maker_nonces.clone(), maker_partials.clone())
    );
    let taker_partials = taker_seat.sign(&maker_nonces, &taker_key).unwrap();
    assert!(!maker_seat.complete());
    let presignatures = maker_seat.partial_round(&taker_partials).unwrap();
    assert_eq!(
        maker_seat.partial_round(&taker_partials).unwrap(),
        presignatures
    );
    assert!(maker_seat.complete());
    let outcome = taker_seat.finish(&maker_partials, &presignatures).unwrap();
    let (bitcoin_session, lez_session) = sessions.validated(&agreement).unwrap();
    verify_adaptor_presignature(bitcoin_session.context(), outcome.bitcoin_presignature).unwrap();
    verify_adaptor_presignature(lez_session.context(), outcome.lez_presignature).unwrap();

    // Actor synthesis on both sides passes the actor's strict loader.
    let claim_json = br#"{"placeholder":true}"#;
    let sidecar_for = |layout: &SwapLayout| {
        SwapSidecar::from_record(SidecarRecordV1 {
            schema_version: 1,
            port: 19000,
            run_id: swap_run_id(&reservation).unwrap().as_str().to_owned(),
            capability_file: layout.root().join("sidecar-capability"),
            state_directory: layout.root().join("sidecar-state"),
            log_file: layout.root().join("sidecar.log"),
        })
        .unwrap()
    };
    let taker_sidecar = sidecar_for(&taker_layout);
    let maker_sidecar = sidecar_for(&maker_layout);
    let taker_config = synthesize(&ActorSynthesis {
        runtime: &taker_runtime,
        layout: &taker_layout,
        agreement: &agreement,
        agreement_wire: &agreement_wire,
        sidecar: &taker_sidecar,
        sessions,
        accepted_at_unix_seconds: 1_700_000_000,
        lez_discovery_start_height: 4_600,
        prepared_claim_json: claim_json,
        maker_lock: None,
    })
    .unwrap();
    assert_eq!(taker_config.role(), btc_reference_actor::ActorRole::Taker);
    assert!(taker_layout.actor_adaptor_secret_file().exists());
    assert!(
        taker_layout.actor_refund_key_file().exists(),
        "the Taker funds Bitcoin here"
    );
    let maker_error = synthesize(&ActorSynthesis {
        runtime: &maker_runtime,
        layout: &maker_layout,
        agreement: &agreement,
        agreement_wire: &agreement_wire,
        sidecar: &maker_sidecar,
        sessions,
        accepted_at_unix_seconds: 1_700_000_000,
        lez_discovery_start_height: 4_600,
        prepared_claim_json: claim_json,
        maker_lock: None,
    })
    .unwrap_err();
    assert!(
        maker_error.to_string().contains("maker lock material"),
        "a Maker without lock material must not synthesize: {maker_error}"
    );
}
