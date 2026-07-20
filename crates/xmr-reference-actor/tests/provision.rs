use std::{
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink},
    path::{Path, PathBuf},
    process::Command,
};

use lez_xmr_swap_sdk::CrossCurveDleqProofV1;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use xmr_reference_actor::{
    Action, ActorRole, Cli, ValidatedPrivateManifest, ValidatedRolePacket, execute,
};

const TAKER_OWNER: &str = "1515151515151515151515151515151515151515151515151515151515151515";
const MAKER_OWNER: &str = "2424242424242424242424242424242424242424242424242424242424242424";

fn owner_directory(parent: &Path, name: &str) -> PathBuf {
    let path = parent.join(name);
    fs::create_dir(&path).expect("create private directory");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("owner-only directory");
    path
}

fn owner_parents(directory: &TempDir) -> (PathBuf, PathBuf) {
    (
        owner_directory(directory.path(), "material"),
        owner_directory(directory.path(), "exchange"),
    )
}

fn assert_owner_private_file(path: &Path) {
    let metadata = fs::symlink_metadata(path).expect("private file metadata");
    assert!(metadata.file_type().is_file());
    assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
    assert_eq!(metadata.nlink(), 1);
}

fn assert_complete_private_root(path: &Path) {
    let metadata = fs::symlink_metadata(path).expect("private root metadata");
    assert!(metadata.file_type().is_dir());
    assert_eq!(metadata.permissions().mode() & 0o7777, 0o700);
    let mut names = fs::read_dir(path)
        .expect("private root")
        .map(|entry| {
            entry
                .expect("private entry")
                .file_name()
                .into_string()
                .expect("ASCII private filename")
        })
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        [
            "agreement.key",
            "claim.key",
            "manifest.json",
            "monero-view.key",
            "refund.key",
            "xmr-share.key",
        ]
    );
    assert!(!path.join("claim-adaptor.key").exists());
    for name in names {
        assert_owner_private_file(&path.join(name));
    }
}

fn assert_no_staging_entries(path: &Path) {
    for entry in fs::read_dir(path).expect("list parent") {
        let name = entry
            .expect("parent entry")
            .file_name()
            .into_string()
            .expect("ASCII test filename");
        assert!(!name.starts_with(".xmr-reference-actor-"), "{name}");
    }
}

fn provision_taker(private_root: PathBuf, public_packet: PathBuf) -> anyhow::Result<()> {
    execute(Cli {
        action: Action::Provision {
            role: ActorRole::Taker,
            private_root,
            lez_owner_account: TAKER_OWNER.to_owned(),
            shared_view_key_file: None,
            public_packet,
        },
    })
}

fn replace_packet_field(bytes: &[u8], field: &str, replacement: &str) -> Vec<u8> {
    let value: Value = serde_json::from_slice(bytes).expect("packet JSON");
    let old = value[field].as_str().expect("string packet field");
    let needle = format!("\"{field}\":\"{old}\"");
    let replacement = format!("\"{field}\":\"{replacement}\"");
    let text = std::str::from_utf8(bytes).expect("packet UTF-8");
    assert_eq!(text.matches(&needle).count(), 1);
    text.replacen(&needle, &replacement, 1).into_bytes()
}

#[test]
fn separate_role_provisioning_atomically_emits_bound_roots_and_validated_packets() {
    let directory = TempDir::new().expect("temporary root");
    let (material, exchange) = owner_parents(&directory);
    let taker_root = material.join("taker");
    let maker_root = material.join("maker");
    let taker_packet = exchange.join("taker.json");
    let maker_packet = exchange.join("maker.json");

    provision_taker(taker_root.clone(), taker_packet.clone()).expect("Taker provision");
    execute(Cli {
        action: Action::Provision {
            role: ActorRole::Maker,
            private_root: maker_root.clone(),
            lez_owner_account: MAKER_OWNER.to_owned(),
            shared_view_key_file: Some(taker_root.join("monero-view.key")),
            public_packet: maker_packet.clone(),
        },
    })
    .expect("Maker provision");

    let taker = ValidatedRolePacket::read(&taker_packet).expect("validated Taker packet");
    let maker = ValidatedRolePacket::read(&maker_packet).expect("validated Maker packet");
    assert_eq!(taker.role(), ActorRole::Taker);
    assert_eq!(maker.role(), ActorRole::Maker);
    assert_eq!(taker.public_view_key(), maker.public_view_key());
    assert_ne!(taker.identity(), maker.identity());
    assert_ne!(taker.proof(), maker.proof());
    assert_eq!(taker.identity().lez_owner_account(), [0x15; 32]);
    assert_eq!(maker.identity().lez_owner_account(), [0x24; 32]);

    for (root, packet, role, owner) in [
        (&taker_root, &taker_packet, ActorRole::Taker, [0x15; 32]),
        (&maker_root, &maker_packet, ActorRole::Maker, [0x24; 32]),
    ] {
        assert_complete_private_root(root);
        assert_owner_private_file(packet);
        let manifest = ValidatedPrivateManifest::read(root).expect("validated private manifest");
        assert_eq!(manifest.role(), role);
        assert_eq!(manifest.lez_owner_account(), owner);
        assert_eq!(
            manifest.public_packet_sha256(),
            <[u8; 32]>::from(Sha256::digest(fs::read(packet).expect("public packet")))
        );
    }
    assert_no_staging_entries(&material);
    assert_no_staging_entries(&exchange);

    let linked_view = exchange.join("linked-view.key");
    symlink(taker_root.join("monero-view.key"), &linked_view).expect("view-key symlink");
    let linked_error = execute(Cli {
        action: Action::Provision {
            role: ActorRole::Maker,
            private_root: material.join("maker-from-link"),
            lez_owner_account: MAKER_OWNER.to_owned(),
            shared_view_key_file: Some(linked_view),
            public_packet: exchange.join("maker-from-link.json"),
        },
    })
    .expect_err("view-key symlink must fail closed");
    assert!(linked_error.to_string().contains("unsafe"));
    assert!(!material.join("maker-from-link").exists());
    assert!(!exchange.join("maker-from-link.json").exists());

    let linked_packet = exchange.join("linked-packet.json");
    symlink(&taker_packet, &linked_packet).expect("packet symlink");
    assert!(ValidatedRolePacket::read(&linked_packet).is_err());
}

#[test]
fn separate_cli_processes_exchange_only_the_shared_view_key() {
    let directory = TempDir::new().expect("temporary root");
    let (material, exchange) = owner_parents(&directory);
    let taker_root = material.join("taker");
    let maker_root = material.join("maker");
    let taker_packet = exchange.join("taker.json");
    let maker_packet = exchange.join("maker.json");
    let binary = env!("CARGO_BIN_EXE_xmr-reference-actor");

    let taker = Command::new(binary)
        .args(["provision", "taker", "--private-root"])
        .arg(&taker_root)
        .args(["--lez-owner-account", TAKER_OWNER, "--public-packet"])
        .arg(&taker_packet)
        .output()
        .expect("spawn Taker process");
    assert!(
        taker.status.success(),
        "Taker failed: {}",
        String::from_utf8_lossy(&taker.stderr)
    );

    let maker = Command::new(binary)
        .args(["provision", "maker", "--private-root"])
        .arg(&maker_root)
        .args(["--lez-owner-account", MAKER_OWNER, "--shared-view-key-file"])
        .arg(taker_root.join("monero-view.key"))
        .arg("--public-packet")
        .arg(&maker_packet)
        .output()
        .expect("spawn Maker process");
    assert!(
        maker.status.success(),
        "Maker failed: {}",
        String::from_utf8_lossy(&maker.stderr)
    );

    let taker = ValidatedRolePacket::read(&taker_packet).expect("validated Taker packet");
    let maker = ValidatedRolePacket::read(&maker_packet).expect("validated Maker packet");
    assert_eq!(taker.public_view_key(), maker.public_view_key());
    assert_ne!(taker.identity(), maker.identity());
    assert_eq!(
        ValidatedPrivateManifest::read(&taker_root)
            .expect("Taker manifest")
            .role(),
        ActorRole::Taker
    );
    assert_eq!(
        ValidatedPrivateManifest::read(&maker_root)
            .expect("Maker manifest")
            .role(),
        ActorRole::Maker
    );
}

#[test]
fn invalid_policy_owner_and_destination_collisions_publish_nothing_partial() {
    let directory = TempDir::new().expect("temporary root");
    let (material, exchange) = owner_parents(&directory);

    let missing_view_root = material.join("missing-view");
    let missing_view_packet = exchange.join("missing-view.json");
    let error = execute(Cli {
        action: Action::Provision {
            role: ActorRole::Maker,
            private_root: missing_view_root.clone(),
            lez_owner_account: MAKER_OWNER.to_owned(),
            shared_view_key_file: None,
            public_packet: missing_view_packet.clone(),
        },
    })
    .expect_err("Maker cannot generate a new shared view key");
    assert!(error.to_string().contains("Maker must import"));
    assert!(!missing_view_root.exists());
    assert!(!missing_view_packet.exists());

    let zero_root = material.join("zero-owner");
    let zero_packet = exchange.join("zero-owner.json");
    let error = execute(Cli {
        action: Action::Provision {
            role: ActorRole::Taker,
            private_root: zero_root.clone(),
            lez_owner_account: "00".repeat(32),
            shared_view_key_file: None,
            public_packet: zero_packet.clone(),
        },
    })
    .expect_err("zero owner must fail");
    assert!(error.to_string().contains("owner account is invalid"));
    assert!(!zero_root.exists());
    assert!(!zero_packet.exists());

    let occupied_root = owner_directory(&material, "occupied");
    fs::write(occupied_root.join("marker"), b"untouched").expect("collision marker");
    let collision_packet = exchange.join("private-collision.json");
    let error = provision_taker(occupied_root.clone(), collision_packet.clone())
        .expect_err("private root collision must fail");
    assert!(error.to_string().contains("already exists"));
    assert_eq!(
        fs::read(occupied_root.join("marker")).expect("collision marker"),
        b"untouched"
    );
    assert!(!collision_packet.exists());

    let occupied_packet = exchange.join("occupied.json");
    fs::write(&occupied_packet, b"untouched").expect("public collision");
    fs::set_permissions(&occupied_packet, fs::Permissions::from_mode(0o600))
        .expect("private public collision mode");
    let public_collision_root = material.join("public-collision");
    let error = provision_taker(public_collision_root.clone(), occupied_packet.clone())
        .expect_err("public packet collision must fail");
    assert!(error.to_string().contains("already exists"));
    assert!(!public_collision_root.exists());
    assert_eq!(
        fs::read(occupied_packet).expect("public collision"),
        b"untouched"
    );
    assert_no_staging_entries(&material);
    assert_no_staging_entries(&exchange);
}

#[test]
fn packet_validation_rejects_compressed_xonly_and_dleq_key_reuse() {
    let directory = TempDir::new().expect("temporary root");
    let (material, exchange) = owner_parents(&directory);
    let root = material.join("taker");
    let packet = exchange.join("taker.json");
    provision_taker(root, packet.clone()).expect("Taker provision");
    let original = fs::read(&packet).expect("original packet");
    let value: Value = serde_json::from_slice(&original).expect("packet JSON");
    let agreement = value["agreement_public_key"]
        .as_str()
        .expect("agreement key");

    let compressed_alias = replace_packet_field(&original, "claim_session_public_key", agreement);
    fs::write(&packet, compressed_alias).expect("write compressed alias");
    let error = ValidatedRolePacket::read(&packet).expect_err("compressed alias rejected");
    assert!(error.to_string().contains("aliased"));

    let mut parity_alias = agreement.to_owned();
    parity_alias.replace_range(
        0..2,
        if agreement.starts_with("02") {
            "03"
        } else {
            "02"
        },
    );
    let xonly_alias = replace_packet_field(&original, "claim_session_public_key", &parity_alias);
    fs::write(&packet, xonly_alias).expect("write x-only alias");
    let error = ValidatedRolePacket::read(&packet).expect_err("x-only alias rejected");
    assert!(error.to_string().contains("x-only"));

    let proof_wire = hex::decode(value["dleq_proof_wire"].as_str().expect("DLEQ proof wire"))
        .expect("proof hex");
    let proof = CrossCurveDleqProofV1::from_wire_bytes(&proof_wire).expect("proof");
    let dleq_alias = replace_packet_field(
        &original,
        "agreement_public_key",
        &hex::encode(proof.secp256k1_public_key()),
    );
    fs::write(&packet, dleq_alias).expect("write DLEQ alias");
    let error = ValidatedRolePacket::read(&packet).expect_err("DLEQ alias rejected");
    assert!(error.to_string().contains("DLEQ point aliases"));
}
