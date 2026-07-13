use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::Parser as _;
use tempfile::TempDir;
use zec_reference_actor::{ActorCli, ActorCommand, ActorConfig, ActorRole, validate_actor_pair};

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

#[test]
fn separate_maker_and_taker_users_bind_one_run_without_sharing_private_state() {
    let fixture = PairFixture::new();
    let maker = ActorConfig::load_private(&fixture.maker_config).expect("maker config");
    let taker = ActorConfig::load_private(&fixture.taker_config).expect("taker config");
    assert_eq!(maker.role(), ActorRole::Maker);
    assert_eq!(taker.role(), ActorRole::Taker);
    assert_eq!(maker.run_id(), taker.run_id());
    validate_actor_pair(&maker, &taker).expect("independent users form one run");
}

#[test]
fn pair_rejects_each_shared_role_local_path_after_canonical_resolution() {
    for field in ["state", "journal", "capability", "key"] {
        let fixture = PairFixture::new();
        let maker = ActorConfig::load_private(&fixture.maker_config).expect("maker config");
        let mut taker_json = fs::read_to_string(&fixture.taker_config).unwrap();
        taker_json = taker_json.replace(
            &fixture.path(field, "taker").display().to_string(),
            &fixture.path(field, "maker").display().to_string(),
        );
        private_file(&fixture.taker_config, &taker_json);
        let taker = ActorConfig::load_private(&fixture.taker_config).expect("taker config");
        assert!(
            validate_actor_pair(&maker, &taker).is_err(),
            "shared {field} path must fail"
        );
    }
}

#[test]
fn pair_rejects_different_runs_and_each_config_rejects_internal_path_reuse() {
    let fixture = PairFixture::new();
    let maker = ActorConfig::load_private(&fixture.maker_config).expect("maker config");
    replace(&fixture.taker_config, "weekend-run", "other-run");
    let taker = ActorConfig::load_private(&fixture.taker_config).expect("taker config");
    assert!(validate_actor_pair(&maker, &taker).is_err());

    let fixture = PairFixture::new();
    replace(
        &fixture.maker_config,
        &fixture.path("journal", "maker").display().to_string(),
        &fixture.path("state", "maker").display().to_string(),
    );
    assert!(ActorConfig::load_private(&fixture.maker_config).is_err());
}

#[test]
fn config_rejects_uppercase_run_id_and_state_path_that_would_overwrite_config() {
    let fixture = PairFixture::new();
    replace(&fixture.maker_config, "weekend-run", "Weekend-run");
    assert!(ActorConfig::load_private(&fixture.maker_config).is_err());

    let fixture = PairFixture::new();
    replace(
        &fixture.maker_config,
        &fixture.path("state", "maker").display().to_string(),
        &fixture.maker_config.display().to_string(),
    );
    assert!(ActorConfig::load_private(&fixture.maker_config).is_err());
}

#[test]
fn pair_rejects_either_actor_path_aliasing_the_other_private_config() {
    for (owner_role, other_config) in [("maker", "taker"), ("taker", "maker")] {
        let fixture = PairFixture::new();
        let owner_config = if owner_role == "maker" {
            &fixture.maker_config
        } else {
            &fixture.taker_config
        };
        let other_config = if other_config == "maker" {
            &fixture.maker_config
        } else {
            &fixture.taker_config
        };
        replace(
            owner_config,
            &fixture.path("state", owner_role).display().to_string(),
            &other_config.display().to_string(),
        );
        let maker = ActorConfig::load_private(&fixture.maker_config).expect("maker config");
        let taker = ActorConfig::load_private(&fixture.taker_config).expect("taker config");
        assert!(validate_actor_pair(&maker, &taker).is_err());
    }
}

#[cfg(unix)]
#[test]
fn private_config_requires_exact_0600_regular_nonsymlink_file() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    let fixture = PairFixture::new();
    fs::set_permissions(&fixture.maker_config, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(ActorConfig::load_private(&fixture.maker_config).is_err());

    let fixture = PairFixture::new();
    let link = fixture.root.path().join("maker-link.json");
    symlink(&fixture.maker_config, &link).unwrap();
    assert!(ActorConfig::load_private(&link).is_err());
}

#[test]
fn debug_and_errors_redact_all_role_local_paths_and_malformed_contents() {
    let fixture = PairFixture::new();
    let maker = ActorConfig::load_private(&fixture.maker_config).expect("maker config");
    let debug = format!("{maker:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains(fixture.root.path().to_string_lossy().as_ref()));

    private_file(&fixture.maker_config, "DO_NOT_PRINT_SECRET");
    let error = ActorConfig::load_private(&fixture.maker_config).unwrap_err();
    let diagnostic = format!("{error:?} {error}");
    assert!(!diagnostic.contains("DO_NOT_PRINT_SECRET"));
    assert!(!diagnostic.contains(fixture.root.path().to_string_lossy().as_ref()));
}

struct PairFixture {
    root: TempDir,
    maker_config: PathBuf,
    taker_config: PathBuf,
}

impl PairFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let maker_config = root.path().join("maker.json");
        let taker_config = root.path().join("taker.json");
        private_file(&maker_config, &config_json(root.path(), "maker"));
        private_file(&taker_config, &config_json(root.path(), "taker"));
        Self {
            root,
            maker_config,
            taker_config,
        }
    }

    fn path(&self, field: &str, role: &str) -> PathBuf {
        let suffix = match field {
            "state" => "state.sqlite",
            "journal" => "journal.sqlite",
            "capability" => "cap",
            "key" => "key",
            _ => panic!("unknown path field"),
        };
        self.root.path().join(format!("{role}-{suffix}"))
    }
}

fn config_json(root: &Path, role: &str) -> String {
    format!(
        r#"{{
          "schema_version": 1,
          "role": "{role}",
          "run_id": "weekend-run",
          "role_state_db": "{}",
          "bridge_journal_db": "{}",
          "bridge_capability_file": "{}",
          "zcash_key_file": "{}"
        }}"#,
        root.join(format!("{role}-state.sqlite")).display(),
        root.join(format!("{role}-journal.sqlite")).display(),
        root.join(format!("{role}-cap")).display(),
        root.join(format!("{role}-key")).display(),
    )
}

fn replace(path: &Path, from: &str, to: &str) {
    let contents = fs::read_to_string(path).unwrap();
    private_file(path, &contents.replace(from, to));
}

fn private_file(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
