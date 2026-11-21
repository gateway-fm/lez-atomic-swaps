//! The real `lez-v02-xmr-stage-a-plan` behind the reference actor's process
//! planner: both halves of one reservation compose the same Stage A.

#![cfg(target_os = "linux")]

use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
};

use lez_bridge_protocol::{Hex32, Participant, RuntimeCompatibility, RuntimeDescriptor};
use tempfile::TempDir;
use xmr_reference_actor::{
    ValidatedRolePacket, XmrLezRole, XmrMoneroChainView, XmrReservationTerms, XmrRoleConfigV1,
    XmrTakerCheck, accept_xmr_maker_stage_a, begin_xmr_taker_role, check_xmr_taker_role,
    reserve_xmr_maker_role,
};
use zeroize::Zeroizing;

const NOW: u64 = 1_900_000_000;

fn private_dir(path: &Path) -> PathBuf {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    path.to_path_buf()
}

fn private_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn config(root: &Path) -> XmrRoleConfigV1 {
    private_file(&root.join("daemon.login"), b"daemon:0123abcd\n");
    private_file(&root.join("wallet.login"), b"wallet:4567cdef\n");
    let path = root.join("xmr-role.json");
    let value = serde_json::json!({
        "schema_version": 1,
        "profile": "accelerated-regtest",
        "monerod_endpoint": "http://127.0.0.1:18081/",
        "monerod_login_file": root.join("daemon.login"),
        "wallet_rpc_endpoint": "http://127.0.0.1:18083/",
        "wallet_rpc_login_file": root.join("wallet.login"),
        "wallet_name": "maker",
        "funding_cutoff_seconds": 260,
        "refund_seconds": 360,
        "punish_seconds": 480,
        "stage_a_planner": env!("CARGO_BIN_EXE_lez-v02-xmr-stage-a-plan"),
    });
    private_file(&path, &serde_json::to_vec(&value).unwrap());
    XmrRoleConfigV1::load(&path).expect("role config with the real planner")
}

fn lez(role: Participant, account: u8) -> XmrLezRole {
    XmrLezRole {
        runtime: RuntimeDescriptor::new(
            role,
            RuntimeCompatibility::LeeV0_2_0,
            Hex32::from_bytes([1; 32]),
            Hex32::from_bytes([40; 32]),
            Hex32::from_bytes([41; 32]),
            Hex32::from_bytes([3; 32]),
            Hex32::from_bytes([account; 32]),
        ),
    }
}

#[test]
fn both_halves_compose_the_same_stage_a_with_the_real_planner() {
    let directory = TempDir::new().unwrap();
    let root = fs::canonicalize(directory.path()).unwrap();
    let config = config(&root);
    let planner = config.stage_a_planner();
    let terms = XmrReservationTerms {
        swap_id: [19; 32],
        amount_piconero: 1_000_000_000_000,
        lez_amount: 500,
    };
    let monero = XmrMoneroChainView {
        genesis: [31; 32],
        tip_height: 150,
    };
    let maker_root = private_dir(&root.join("maker")).join("xmr");
    let taker_root = private_dir(&root.join("taker")).join("xmr");

    let taker = begin_xmr_taker_role(&taker_root, [0x22; 32]).unwrap();
    let maker = reserve_xmr_maker_role(
        &config,
        &planner,
        &terms,
        &lez(Participant::Maker, 0x11),
        &Zeroizing::new([7; 32]),
        &taker,
        monero,
        4,
        NOW,
        &maker_root,
    )
    .expect("the Maker's half plans through the real planner");
    let maker_identity = ValidatedRolePacket::from_bytes(&maker.maker_packet)
        .unwrap()
        .identity()
        .agreement_public_key();
    // The Taker replans with its own planner run and requires the same bytes.
    let wire = check_xmr_taker_role(
        &config,
        &planner,
        &terms,
        &lez(Participant::Taker, 0x22),
        &taker_root,
        &maker,
        &XmrTakerCheck {
            now: NOW + 5,
            monero,
            maker_identity,
        },
    )
    .expect("the Taker's half composes the Maker's Stage A");
    let agreement = accept_xmr_maker_stage_a(&maker_root, &wire).unwrap();
    assert_eq!(agreement.body().swap_id(), terms.swap_id);
    assert_eq!(
        fs::read(maker_root.join("stage-a.unsigned")).unwrap(),
        fs::read(taker_root.join("stage-a.unsigned")).unwrap()
    );

    // Another Maker owner nonce is another punish hash: the real planner
    // composes another Stage A, which the Taker refuses.
    let mut other = maker.clone();
    other.maker_owner_nonce += 1;
    assert!(
        check_xmr_taker_role(
            &config,
            &planner,
            &terms,
            &lez(Participant::Taker, 0x22),
            &taker_root,
            &other,
            &XmrTakerCheck {
                now: NOW + 5,
                monero,
                maker_identity,
            },
        )
        .is_err()
    );
}
