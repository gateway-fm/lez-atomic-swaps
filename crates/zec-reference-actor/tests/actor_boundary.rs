use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::Parser as _;
use lez_swap_core::Participant;
use lez_swap_store::SqliteZecRecoveryStore;
use lez_zec_swap_sdk::ProtectedClaimKey;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use zec_reference_actor::{
    ActorCli, ActorCommand, ActorCommandError, ActorConfig, ActorRole, execute_actor_command,
    validate_actor_pair,
};

const CAPABILITY: &[u8] = b"actor_capability_0123456789abcdef";
const COOKIE: &[u8] = b"actor:private-cookie\n";

#[test]
fn cli_exposes_exact_one_shot_commands_and_requires_private_config() {
    for (spelling, expected) in [
        ("activate", ActorCommand::Activate),
        ("drive", ActorCommand::Drive),
        ("status", ActorCommand::Status),
    ] {
        let cli = ActorCli::try_parse_from(["actor", "--config", "actor.json", spelling])
            .expect("documented command parses");
        assert_eq!(cli.command, expected);
    }
    assert!(ActorCli::try_parse_from(["actor", "activate"]).is_err());
}

#[tokio::test]
async fn status_reports_versioned_not_activated_without_creating_role_state() {
    let fixture = PairFixture::new();
    let role_state = fixture.path("maker-state");
    let config = fixture.load("maker");

    let output = execute_actor_command(&config, ActorCommand::Status)
        .await
        .expect("missing durable state has a truthful offline status");

    assert_eq!(
        serde_json::to_value(output).expect("status serializes"),
        json!({"schema_version": 1, "role": "maker", "state": "not_activated"})
    );
    assert!(!role_state.exists(), "status must not create actor state");
}

#[tokio::test]
async fn existing_store_status_uses_offline_claim_capable_replay() {
    let fixture = PairFixture::new();
    let role_state = fixture.path("maker-state");
    drop(
        SqliteZecRecoveryStore::open_claim_capable(
            &role_state,
            Participant::Maker,
            ProtectedClaimKey::new("maker-claim-key-v1", [7; 32]).expect("claim key"),
        )
        .expect("create role-local store"),
    );
    for name in [
        "agreement",
        "maker-zcash-key",
        "maker-capability",
        "maker-cookie",
        "maker-preimage",
    ] {
        fs::remove_file(fixture.path(name)).unwrap();
    }
    let config = fixture.load("maker");

    let output = execute_actor_command(&config, ActorCommand::Status)
        .await
        .expect("existing store replays with no effect material or live RPC");

    assert_eq!(
        serde_json::to_value(output).expect("status serializes"),
        json!({"schema_version": 1, "role": "maker", "state": "not_activated"})
    );
}

#[tokio::test]
async fn status_failures_are_stable_and_payload_free() {
    let fixture = PairFixture::new();
    regular_file(
        &fixture.path("maker-state"),
        b"secret marker and invalid SQLite payload",
    );
    let config = fixture.load("maker");

    let error = execute_actor_command(&config, ActorCommand::Status)
        .await
        .expect_err("invalid durable state fails closed");

    assert_eq!(error, ActorCommandError::StatusStoreUnavailable);
    let diagnostics = format!("{error:?} {error}");
    for forbidden in [
        "secret marker",
        "swap-001",
        "maker-state",
        fixture.root.path().to_string_lossy().as_ref(),
    ] {
        assert!(!diagnostics.contains(forbidden));
    }
}

#[tokio::test]
async fn effect_commands_fail_closed_until_real_composition_exists() {
    for command in [ActorCommand::Activate, ActorCommand::Drive] {
        let fixture = PairFixture::new();
        let config = fixture.load("maker");

        assert_eq!(
            execute_actor_command(&config, command).await,
            Err(ActorCommandError::CommandUnavailable)
        );
        assert!(!fixture.path("maker-state").exists());
        assert!(!fixture.path("maker-journal").exists());
    }
}

#[test]
fn schema_v2_binds_complete_typed_runtime_and_one_isolated_pair() {
    let fixture = PairFixture::new();
    let maker = fixture.load("maker");
    let taker = fixture.load("taker");

    assert_eq!(maker.role(), ActorRole::Maker);
    assert_eq!(taker.role(), ActorRole::Taker);
    assert_eq!(maker.run_id().as_str(), "weekend-run");
    assert_eq!(maker.swap_id().as_str(), "swap-001");
    assert_eq!(maker.zcash_funding_outpoints().len(), 1);
    assert!(taker.zcash_funding_outpoints().is_empty());
    assert_eq!(maker.lez_discovery_window().start_height(), 1);
    assert_eq!(maker.lez_discovery_window().max_blocks(), 256);
    validate_actor_pair(&maker, &taker).expect("independent users form one run");
}

#[test]
fn generic_load_is_offline_and_status_requires_only_role_store_and_claim_key() {
    let fixture = PairFixture::new();
    for name in [
        "agreement",
        "maker-zcash-key",
        "maker-capability",
        "maker-cookie",
        "maker-preimage",
    ] {
        fs::remove_file(fixture.path(name)).unwrap();
    }

    let maker = fixture.load("maker");
    maker
        .load_status_material()
        .expect("offline status only needs claim recovery key");
    assert!(maker.load_activate_material().is_err());
    assert!(maker.load_drive_material().is_err());
}

#[test]
fn offline_status_allows_effect_paths_below_absent_parent_directories() {
    let fixture = PairFixture::new();
    regular_file(&fixture.path("maker-state"), b"existing role store");
    for (pointer, suffix) in [
        ("/signed_agreement_file", "agreement/wire"),
        ("/zcash_key_file", "zcash/key"),
        ("/claim_preimage_file", "preimage/value"),
        ("/bridge/capability_file", "sidecar/capability"),
        ("/zebra/cookie_file", "zebra/cookie"),
    ] {
        set(
            &fixture.maker_config,
            pointer,
            path_value(&fixture.root.path().join("absent").join(suffix)),
        );
    }

    let maker = fixture.load("maker");
    maker
        .load_status_material()
        .expect("offline status ignores unavailable effect-only parents");
}

#[test]
fn config_rejects_schema_unknown_fields_and_invalid_typed_ids() {
    for (pointer, value) in [
        ("/schema_version", json!(1)),
        ("/run_id", json!("bad run")),
        ("/swap_id", json!("")),
        ("/swap_id", json!("x".repeat(129))),
        ("/lez_discovery_window/max_blocks", json!(0)),
        ("/lez_discovery_window/max_blocks", json!(4097)),
    ] {
        let fixture = PairFixture::new();
        set(&fixture.maker_config, pointer, value);
        assert!(ActorConfig::load_private(&fixture.maker_config).is_err());
    }

    let fixture = PairFixture::new();
    edit(&fixture.maker_config, |value| {
        value["unexpected"] = json!(true);
    });
    assert!(ActorConfig::load_private(&fixture.maker_config).is_err());

    let fixture = PairFixture::new();
    set(
        &fixture.maker_config,
        "/claim_recovery/key_id",
        json!("line\nbreak"),
    );
    assert!(ActorConfig::load_private(&fixture.maker_config).is_err());
}

#[test]
fn private_config_rejects_empty_oversized_and_nonregular_files() {
    let fixture = PairFixture::new();
    private_bytes(&fixture.maker_config, b"");
    assert!(ActorConfig::load_private(&fixture.maker_config).is_err());

    let fixture = PairFixture::new();
    private_bytes(&fixture.maker_config, &vec![1; 64 * 1024 + 1]);
    assert!(ActorConfig::load_private(&fixture.maker_config).is_err());

    let fixture = PairFixture::new();
    let directory = fixture.root.path().join("config-directory");
    fs::create_dir(&directory).unwrap();
    assert!(ActorConfig::load_private(directory).is_err());
}

#[test]
fn endpoints_are_explicit_distinct_literal_loopback_http_services() {
    for endpoint in [
        "https://127.0.0.1:19001",
        "http://localhost:19001",
        "http://192.0.2.10:19001",
        "http://127.0.0.1",
        "http://127.0.0.1:0",
        "http://user:pass@127.0.0.1:19001",
        "http://127.0.0.1:19001/rpc",
        "http://127.0.0.1:19001/?token=secret",
        "http://127.0.0.1:19001/#fragment",
    ] {
        let fixture = PairFixture::new();
        set(&fixture.maker_config, "/bridge/endpoint", json!(endpoint));
        assert!(
            ActorConfig::load_private(&fixture.maker_config).is_err(),
            "endpoint {endpoint} must fail"
        );
    }

    let fixture = PairFixture::new();
    set(
        &fixture.maker_config,
        "/bridge/endpoint",
        json!("http://127.0.0.1:19101"),
    );
    assert!(ActorConfig::load_private(&fixture.maker_config).is_err());
}

#[test]
fn runtime_and_zebra_identity_are_role_correct_nonzero_and_immutable() {
    for pointer in [
        "/signed_agreement_sha256",
        "/bridge/runtime/chain_id",
        "/bridge/runtime/channel_id",
        "/bridge/runtime/genesis_block_hash",
        "/bridge/runtime/escrow_program_id",
        "/bridge/runtime/signer_account_id",
        "/zebra/identity/genesis_hash",
    ] {
        let fixture = PairFixture::new();
        set(&fixture.maker_config, pointer, json!("00".repeat(32)));
        assert!(ActorConfig::load_private(&fixture.maker_config).is_err());
    }

    for (pointer, value) in [
        ("/bridge/runtime/sidecar_role", json!("taker")),
        ("/zebra/identity/network", json!("main")),
        ("/zebra/identity/consensus_branch_id", json!("ffffffff")),
        ("/zebra/counterparty_scan_blocks", json!(0)),
        ("/zebra/counterparty_scan_blocks", json!(50_001)),
    ] {
        let fixture = PairFixture::new();
        set(&fixture.maker_config, pointer, value);
        assert!(ActorConfig::load_private(&fixture.maker_config).is_err());
    }
}

#[test]
fn candidate_outpoints_are_exact_bounded_unique_and_owned_with_the_preimage() {
    let valid = json!({"transaction_id": "aa".repeat(32), "output_index": 0});
    for candidates in [
        json!([]),
        json!([valid.clone(), valid.clone()]),
        json!([{"transaction_id": "00".repeat(32), "output_index": 0}]),
        json!([{"transaction_id": "AA".repeat(32), "output_index": 0}]),
        Value::Array(vec![valid.clone(); 65]),
    ] {
        let fixture = PairFixture::new();
        set(
            &fixture.maker_config,
            "/zcash_funding_outpoints",
            candidates,
        );
        assert!(ActorConfig::load_private(&fixture.maker_config).is_err());
    }

    let fixture = PairFixture::new();
    set(&fixture.maker_config, "/claim_preimage_file", Value::Null);
    assert!(ActorConfig::load_private(&fixture.maker_config).is_err());

    let fixture = PairFixture::new();
    set(
        &fixture.taker_config,
        "/zcash_funding_outpoints",
        json!([valid]),
    );
    assert!(ActorConfig::load_private(&fixture.taker_config).is_err());
}

#[test]
fn pair_requires_one_funder_and_matching_run_swap_chain_and_agreement() {
    for (pointer, value) in [
        ("/run_id", json!("another-run")),
        ("/swap_id", json!("swap-002")),
        ("/bridge/runtime/channel_id", json!("88".repeat(32))),
        ("/zebra/identity/genesis_hash", json!("99".repeat(32))),
        ("/lez_discovery_window/start_height", json!(2)),
        ("/signed_agreement_sha256", json!("ab".repeat(32))),
    ] {
        let fixture = PairFixture::new();
        set(&fixture.taker_config, pointer, value);
        assert!(validate_actor_pair(&fixture.load("maker"), &fixture.load("taker")).is_err());
    }

    let fixture = PairFixture::new();
    let other_agreement = fixture.root.path().join("other-agreement");
    regular_file(&other_agreement, b"other signed agreement");
    set(
        &fixture.taker_config,
        "/signed_agreement_file",
        path_value(&other_agreement),
    );
    assert!(validate_actor_pair(&fixture.load("maker"), &fixture.load("taker")).is_err());

    let fixture = PairFixture::new();
    make_funder(&fixture, "taker");
    assert!(validate_actor_pair(&fixture.load("maker"), &fixture.load("taker")).is_err());

    let fixture = PairFixture::new();
    make_nonfunder(&fixture, "maker");
    assert!(validate_actor_pair(&fixture.load("maker"), &fixture.load("taker")).is_err());
}

#[test]
fn existing_mutable_paths_reject_symlinks_dangling_links_and_nonregular_files() {
    #[cfg(unix)]
    {
        use std::os::unix::{fs::symlink, net::UnixListener};

        for pointer in ["/role_state_db", "/bridge/journal_db"] {
            let fixture = PairFixture::new();
            let path = config_path(&fixture.maker_config, pointer);
            let target = fixture.root.path().join("db-target");
            regular_file(&target, b"db");
            symlink(&target, &path).unwrap();
            assert!(ActorConfig::load_private(&fixture.maker_config).is_err());

            let fixture = PairFixture::new();
            let path = config_path(&fixture.maker_config, pointer);
            symlink(fixture.root.path().join("missing-target"), &path).unwrap();
            assert!(ActorConfig::load_private(&fixture.maker_config).is_err());

            let fixture = PairFixture::new();
            let path = config_path(&fixture.maker_config, pointer);
            fs::create_dir(&path).unwrap();
            assert!(ActorConfig::load_private(&fixture.maker_config).is_err());

            let fixture = PairFixture::new();
            let path = config_path(&fixture.maker_config, pointer);
            let _socket = UnixListener::bind(&path).unwrap();
            assert!(ActorConfig::load_private(&fixture.maker_config).is_err());
        }
    }
}

#[test]
fn config_rejects_internal_or_protected_path_aliases() {
    for pointer in path_pointers("maker") {
        let internal = if pointer == "/role_state_db" {
            "maker-journal"
        } else {
            "maker-state"
        };
        for target in [internal, "maker-config", "agreement"] {
            let fixture = PairFixture::new();
            set(
                &fixture.maker_config,
                pointer,
                path_value(&fixture.path(target)),
            );
            assert!(
                ActorConfig::load_private(&fixture.maker_config).is_err(),
                "alias at {pointer} to {target} must fail"
            );
        }
    }
}

#[test]
fn every_configured_path_is_absolute_and_lexically_normalized() {
    for path in [
        PathBuf::from("relative-state"),
        PathBuf::from("/tmp/actor/../state"),
        PathBuf::from("/tmp/actor/./state"),
        PathBuf::from("//tmp/actor/state"),
        PathBuf::from("/tmp//actor/state"),
        PathBuf::from("/tmp/actor/state/"),
    ] {
        let fixture = PairFixture::new();
        set(&fixture.maker_config, "/role_state_db", path_value(&path));
        assert!(
            ActorConfig::load_private(&fixture.maker_config).is_err(),
            "non-normalized path must fail: {}",
            path.display()
        );
    }
}

#[test]
fn pair_rejects_cross_actor_paths_or_config_aliases_but_shares_agreement() {
    let baseline = PairFixture::new();
    validate_actor_pair(&baseline.load("maker"), &baseline.load("taker"))
        .expect("shared signed agreement is intentional");

    for pointer in path_pointers("taker") {
        let fixture = PairFixture::new();
        set(
            &fixture.taker_config,
            pointer,
            path_value(&fixture.path("maker-state")),
        );
        let result = ActorConfig::load_private(&fixture.taker_config)
            .and_then(|taker| validate_actor_pair(&fixture.load("maker"), &taker));
        assert!(result.is_err(), "cross actor alias at {pointer} must fail");
    }

    for (role, target) in [("maker", "taker-config"), ("taker", "maker-config")] {
        let fixture = PairFixture::new();
        set(
            fixture.config(role),
            "/role_state_db",
            path_value(&fixture.path(target)),
        );
        let result = ActorConfig::load_private(fixture.config(role)).and_then(|changed| {
            if role == "maker" {
                validate_actor_pair(&changed, &fixture.load("taker"))
            } else {
                validate_actor_pair(&fixture.load("maker"), &changed)
            }
        });
        assert!(result.is_err());
    }
}

#[cfg(unix)]
#[test]
fn inode_identity_rejects_hard_link_aliases_within_config_and_across_actors() {
    let fixture = PairFixture::new();
    fs::remove_file(fixture.path("maker-zcash-key")).unwrap();
    fs::hard_link(
        fixture.path("maker-claim-key"),
        fixture.path("maker-zcash-key"),
    )
    .unwrap();
    assert!(ActorConfig::load_private(&fixture.maker_config).is_err());

    let fixture = PairFixture::new();
    let state_alias = fixture.path("maker-state-alias");
    set(
        &fixture.maker_config,
        "/role_state_db",
        path_value(&state_alias),
    );
    fs::hard_link(&fixture.maker_config, state_alias).unwrap();
    assert!(ActorConfig::load_private(&fixture.maker_config).is_err());

    let fixture = PairFixture::new();
    fs::remove_file(fixture.path("taker-zcash-key")).unwrap();
    fs::hard_link(
        fixture.path("maker-zcash-key"),
        fixture.path("taker-zcash-key"),
    )
    .unwrap();
    assert!(validate_actor_pair(&fixture.load("maker"), &fixture.load("taker")).is_err());
}

#[cfg(unix)]
#[test]
fn config_and_command_material_reject_permissions_and_symlinks_at_use_time() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    let fixture = PairFixture::new();
    fs::set_permissions(&fixture.maker_config, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(ActorConfig::load_private(&fixture.maker_config).is_err());

    let fixture = PairFixture::new();
    let link = fixture.root.path().join("maker-link");
    symlink(&fixture.maker_config, &link).unwrap();
    assert!(ActorConfig::load_private(&link).is_err());

    for (pointer, material) in [
        ("/claim_recovery/key_file", Material::Status),
        ("/claim_recovery/key_file", Material::Activate),
        ("/zcash_key_file", Material::Activate),
        ("/claim_preimage_file", Material::Activate),
        ("/bridge/capability_file", Material::Activate),
        ("/zcash_key_file", Material::Drive),
        ("/claim_preimage_file", Material::Drive),
        ("/bridge/capability_file", Material::Drive),
        ("/zebra/cookie_file", Material::Drive),
    ] {
        let fixture = PairFixture::new();
        let config = fixture.load("maker");
        let path = config_path(&fixture.maker_config, pointer);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(material.load(&config).is_err());

        let fixture = PairFixture::new();
        let config = fixture.load("maker");
        let path = config_path(&fixture.maker_config, pointer);
        let target = fixture.root.path().join("symlink-target");
        private_bytes(&target, &[1; 64]);
        fs::remove_file(&path).unwrap();
        symlink(&target, &path).unwrap();
        assert!(material.load(&config).is_err());
    }

    let fixture = PairFixture::new();
    let config = fixture.load("maker");
    let agreement = fixture.path("agreement");
    let target = fixture.root.path().join("agreement-target");
    regular_file(&target, b"signed agreement");
    fs::remove_file(&agreement).unwrap();
    symlink(&target, &agreement).unwrap();
    assert!(config.load_activate_material().is_err());
}

#[test]
fn command_material_enforces_bounds_and_existing_library_grammars() {
    for (name, contents, material) in [
        ("maker-claim-key", vec![7; 31], Material::Status),
        ("maker-zcash-key", vec![0; 32], Material::Activate),
        ("maker-preimage", vec![8; 31], Material::Activate),
        (
            "maker-capability",
            b"too-short".to_vec(),
            Material::Activate,
        ),
        ("maker-cookie", b"missing-colon".to_vec(), Material::Drive),
    ] {
        let fixture = PairFixture::new();
        private_bytes(&fixture.path(name), &contents);
        assert!(material.load(&fixture.load("maker")).is_err());
    }

    let fixture = PairFixture::new();
    regular_file(&fixture.path("agreement"), &vec![1; 16 * 1024 + 1]);
    assert!(fixture.load("maker").load_activate_material().is_err());

    let fixture = PairFixture::new();
    regular_file(&fixture.path("agreement"), b"");
    assert!(fixture.load("maker").load_activate_material().is_err());

    let fixture = PairFixture::new();
    private_bytes(&fixture.path("maker-claim-key"), &[0; 32]);
    assert!(fixture.load("maker").load_status_material().is_err());
}

#[test]
fn every_command_reopens_material_and_rejects_replacement() {
    for (name, replacement, material) in [
        ("maker-claim-key", vec![2; 32], Material::Status),
        ("maker-capability", CAPABILITY.to_vec(), Material::Activate),
        ("maker-cookie", COOKIE.to_vec(), Material::Drive),
    ] {
        let fixture = PairFixture::new();
        let config = fixture.load("maker");
        material.load(&config).expect("initial material");
        replace_inode(&fixture.path(name), &replacement);
        assert!(material.load(&config).is_err());
    }

    for (name, replacement, material) in [
        ("maker-claim-key", vec![3; 32], Material::Status),
        ("maker-capability", rotated_capability(), Material::Activate),
        (
            "maker-cookie",
            b"actor:rotated-cookie\n".to_vec(),
            Material::Drive,
        ),
    ] {
        let fixture = PairFixture::new();
        let config = fixture.load("maker");
        private_bytes(&fixture.path(name), &replacement);
        assert!(
            material.load(&config).is_err(),
            "same-inode rewrite of {name} must fail"
        );
    }
}

#[test]
fn activation_rejects_same_inode_agreement_rewrite_even_when_shape_is_valid() {
    let fixture = PairFixture::new();
    let maker = fixture.load("maker");
    regular_file(&fixture.path("agreement"), b"forged agreement wire");
    assert!(maker.load_activate_material().is_err());
}

#[test]
fn activation_rejects_agreement_digest_mismatch_present_before_config_load() {
    let fixture = PairFixture::new();
    regular_file(&fixture.path("agreement"), b"forged agreement wire");
    let maker = fixture.load("maker");
    assert!(maker.load_activate_material().is_err());
}

#[cfg(unix)]
#[test]
fn newly_created_private_file_cannot_hard_link_an_existing_binding() {
    let fixture = PairFixture::new();
    fs::remove_file(fixture.path("maker-cookie")).unwrap();
    let maker = fixture.load("maker");
    fs::hard_link(
        fixture.path("maker-claim-key"),
        fixture.path("maker-cookie"),
    )
    .unwrap();
    assert!(maker.load_drive_material().is_err());
}

#[cfg(unix)]
#[test]
fn late_public_agreement_cannot_alias_new_role_state_or_bridge_journal() {
    for mutable_name in ["maker-state", "maker-journal"] {
        let fixture = PairFixture::new();
        fs::remove_file(fixture.path("agreement")).unwrap();
        let maker = fixture.load("maker");
        regular_file(&fixture.path("agreement"), b"signed agreement wire");
        fs::hard_link(fixture.path("agreement"), fixture.path(mutable_name)).unwrap();
        assert!(
            maker.load_activate_material().is_err(),
            "late agreement alias with {mutable_name} must fail"
        );
    }
}

#[test]
fn debug_and_errors_redact_paths_credentials_and_key_material() {
    let fixture = PairFixture::new();
    let maker = fixture.load("maker");
    let config_debug = format!("{maker:?}");
    assert!(config_debug.contains("[REDACTED]"));
    assert!(!config_debug.contains(fixture.root.path().to_string_lossy().as_ref()));

    for debug in [
        format!("{:?}", maker.load_status_material().unwrap()),
        format!("{:?}", maker.load_activate_material().unwrap()),
        format!("{:?}", maker.load_drive_material().unwrap()),
    ] {
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("actor_capability"));
        assert!(!debug.contains("private-cookie"));
        assert!(!debug.contains(fixture.root.path().to_string_lossy().as_ref()));
    }

    private_bytes(&fixture.path("maker-claim-key"), b"DO_NOT_PRINT_SECRET");
    let error = maker.load_status_material().unwrap_err();
    let diagnostic = format!("{error:?} {error}");
    assert!(!diagnostic.contains("DO_NOT_PRINT_SECRET"));
    assert!(!diagnostic.contains(fixture.root.path().to_string_lossy().as_ref()));

    let cli = ActorCli::try_parse_from([
        "actor",
        "--config",
        fixture.maker_config.to_str().unwrap(),
        "status",
    ])
    .unwrap();
    let cli_debug = format!("{cli:?}");
    assert!(cli_debug.contains("[REDACTED]"));
    assert!(!cli_debug.contains(fixture.root.path().to_string_lossy().as_ref()));
}

#[derive(Clone, Copy)]
enum Material {
    Status,
    Activate,
    Drive,
}

impl Material {
    fn load(self, config: &ActorConfig) -> Result<(), zec_reference_actor::ActorConfigError> {
        match self {
            Self::Status => config.load_status_material().map(drop),
            Self::Activate => config.load_activate_material().map(drop),
            Self::Drive => config.load_drive_material().map(drop),
        }
    }
}

struct PairFixture {
    root: TempDir,
    maker_config: PathBuf,
    taker_config: PathBuf,
}

impl PairFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        regular_file(&root.path().join("agreement"), b"signed agreement wire");
        for role in ["maker", "taker"] {
            private_bytes(&root.path().join(format!("{role}-claim-key")), &[7; 32]);
            private_bytes(&root.path().join(format!("{role}-zcash-key")), &[8; 32]);
            private_bytes(&root.path().join(format!("{role}-capability")), CAPABILITY);
            private_bytes(&root.path().join(format!("{role}-cookie")), COOKIE);
        }
        private_bytes(&root.path().join("maker-preimage"), &[9; 32]);
        private_bytes(&root.path().join("taker-preimage"), &[10; 32]);

        let maker_config = root.path().join("maker-config");
        let taker_config = root.path().join("taker-config");
        private_json(&maker_config, &config_json(root.path(), "maker", true));
        private_json(&taker_config, &config_json(root.path(), "taker", false));
        Self {
            root,
            maker_config,
            taker_config,
        }
    }

    fn config(&self, role: &str) -> &Path {
        match role {
            "maker" => &self.maker_config,
            "taker" => &self.taker_config,
            _ => panic!("unknown role"),
        }
    }

    fn load(&self, role: &str) -> ActorConfig {
        ActorConfig::load_private(self.config(role)).expect("valid actor config")
    }

    fn path(&self, name: &str) -> PathBuf {
        match name {
            "maker-config" => self.maker_config.clone(),
            "taker-config" => self.taker_config.clone(),
            _ => self.root.path().join(name),
        }
    }
}

fn config_json(root: &Path, role: &str, funder: bool) -> Value {
    let sidecar_port = if role == "maker" { 19_001 } else { 19_002 };
    let signer = if role == "maker" {
        "55".repeat(32)
    } else {
        "66".repeat(32)
    };
    let preimage = funder.then(|| root.join(format!("{role}-preimage")));
    let candidates = if funder {
        json!([{"transaction_id": "aa".repeat(32), "output_index": 0}])
    } else {
        json!([])
    };
    json!({
        "schema_version": 2,
        "role": role,
        "run_id": "weekend-run",
        "swap_id": "swap-001",
        "signed_agreement_file": root.join("agreement"),
        "signed_agreement_sha256": sha256_hex(b"signed agreement wire"),
        "role_state_db": root.join(format!("{role}-state")),
        "claim_recovery": {
            "key_id": format!("{role}-claim-key-v1"),
            "key_file": root.join(format!("{role}-claim-key"))
        },
        "claim_preimage_file": preimage,
        "zcash_key_file": root.join(format!("{role}-zcash-key")),
        "bridge": {
            "endpoint": format!("http://127.0.0.1:{sidecar_port}"),
            "journal_db": root.join(format!("{role}-journal")),
            "capability_file": root.join(format!("{role}-capability")),
            "runtime": {
                "sidecar_role": role,
                "compatibility": "nssa_v0_1_2",
                "chain_id": "11".repeat(32),
                "channel_id": "22".repeat(32),
                "genesis_block_hash": "33".repeat(32),
                "escrow_program_id": "44".repeat(32),
                "signer_account_id": signer
            },
            "request_timeout_millis": 5000
        },
        "zebra": {
            "endpoint": "http://127.0.0.1:19101",
            "cookie_file": root.join(format!("{role}-cookie")),
            "identity": {
                "network": "regtest",
                "rpc_chain": "test",
                "consensus_branch_id": "c8e71055",
                "genesis_hash": "77".repeat(32)
            },
            "counterparty_scan_blocks": 1000
        },
        "lez_discovery_window": {"start_height": 1, "max_blocks": 256},
        "zcash_funding_outpoints": candidates
    })
}

fn make_funder(fixture: &PairFixture, role: &str) {
    set(
        fixture.config(role),
        "/claim_preimage_file",
        path_value(&fixture.path(&format!("{role}-preimage"))),
    );
    set(
        fixture.config(role),
        "/zcash_funding_outpoints",
        json!([{"transaction_id": "bb".repeat(32), "output_index": 1}]),
    );
}

fn make_nonfunder(fixture: &PairFixture, role: &str) {
    set(fixture.config(role), "/claim_preimage_file", Value::Null);
    set(fixture.config(role), "/zcash_funding_outpoints", json!([]));
}

fn path_pointers(role: &str) -> Vec<&'static str> {
    let mut pointers = vec![
        "/role_state_db",
        "/claim_recovery/key_file",
        "/zcash_key_file",
        "/bridge/journal_db",
        "/bridge/capability_file",
        "/zebra/cookie_file",
    ];
    if role == "maker" {
        pointers.push("/claim_preimage_file");
    }
    pointers
}

fn config_path(config: &Path, pointer: &str) -> PathBuf {
    let value: Value = serde_json::from_slice(&fs::read(config).unwrap()).unwrap();
    PathBuf::from(value.pointer(pointer).unwrap().as_str().unwrap())
}

fn path_value(path: &Path) -> Value {
    json!(path)
}

fn set(path: &Path, pointer: &str, replacement: Value) {
    edit(path, |value| {
        *value.pointer_mut(pointer).expect("fixture JSON pointer") = replacement;
    });
}

fn edit(path: &Path, mutation: impl FnOnce(&mut Value)) {
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    mutation(&mut value);
    private_json(path, &value);
}

fn private_json(path: &Path, value: &Value) {
    private_bytes(path, &serde_json::to_vec_pretty(value).unwrap());
}

fn private_bytes(path: &Path, contents: &[u8]) {
    fs::write(path, contents).unwrap();
    set_private_permissions(path);
}

fn regular_file(path: &Path, contents: &[u8]) {
    fs::write(path, contents).unwrap();
}

fn replace_inode(path: &Path, contents: &[u8]) {
    let replacement = path.with_extension("replacement");
    private_bytes(&replacement, contents);
    fs::rename(replacement, path).unwrap();
}

fn rotated_capability() -> Vec<u8> {
    b"other_capability_0123456789abcdef".to_vec()
}

fn sha256_hex(contents: &[u8]) -> String {
    use std::fmt::Write as _;

    Sha256::digest(contents)
        .iter()
        .fold(String::with_capacity(64), |mut encoded, byte| {
            write!(&mut encoded, "{byte:02x}").unwrap();
            encoded
        })
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) {}
