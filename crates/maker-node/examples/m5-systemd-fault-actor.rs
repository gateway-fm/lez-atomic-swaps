//! Feature-gated, node-free actor used only to prove systemd scheduler recovery.

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
};

use clap::Parser as _;
use serde_json::{Value, json};
use zec_reference_actor::{ActorCli, ActorCommand, ActorConfig, arm_test_crash_hook};

fn main() {
    run().unwrap_or_else(|message| {
        eprintln!("{message}");
        std::process::exit(2);
    });
}

fn run() -> Result<(), &'static str> {
    let cli = ActorCli::parse();
    let fd = cli.config_fd.ok_or("sealed configuration is unavailable")?;
    if cli.config.is_some() {
        return Err("sealed configuration is unavailable");
    }
    let config = ActorConfig::load_private_fd(fd).map_err(|_| "sealed configuration is invalid")?;
    if !Path::new("/proc/self/fd/198").exists() {
        return Err("inherited actor lock is unavailable");
    }
    let effect = effect_path(config.role_state_db());
    let is_crash_actor = config.swap_id().as_str().starts_with("aaa-");
    let output = match cli.command {
        ActorCommand::Status if is_crash_actor && !effect.exists() => json!({
            "schema_version": 1,
            "role": "maker",
            "state": "active",
            "phase": "offered",
            "revision": 1,
            "next_action": "fund_zcash"
        }),
        ActorCommand::Status => terminal_output("status"),
        ActorCommand::Drive if is_crash_actor && !effect.exists() => {
            persist_effect(&effect, config.swap_id().as_str())?;
            json!({
                "schema_version": 1,
                "role": "maker",
                "command": "drive",
                "outcome": "submitted",
                "operation": "zcash_fund",
                "phase": "awaiting_maker_confirmations",
                "revision": 2,
                "next_action": "wait"
            })
        }
        ActorCommand::Drive => terminal_output("drive"),
        ActorCommand::Activate => return Err("fixture actor is already active"),
        ActorCommand::Claim => return Err("fixture claim is unavailable"),
        ActorCommand::Recover => return Err("fixture recovery is unavailable"),
    };
    let encoded = serde_json::to_string(&output).map_err(|_| "fixture output is unavailable")?;
    if is_crash_actor
        && matches!(cli.command, ActorCommand::Drive)
        && output.get("outcome") == Some(&Value::from("submitted"))
        && arm_test_crash_hook(
            std::env::var("LEZ_ACTOR_TEST_PAUSE_AFTER_SUBMITTED")
                .map_err(|_| "fixture pause operation is unavailable")?
                .as_str(),
            std::env::var_os("LEZ_ACTOR_TEST_PAUSE_MARKER")
                .ok_or("fixture pause marker is unavailable")?
                .as_ref(),
            config.swap_id().as_str(),
            "maker",
            &encoded,
        )
        .map_err(|_| "fixture pause hook is unavailable")?
    {
        loop {
            std::thread::park();
        }
    }
    println!("{encoded}");
    Ok(())
}

fn terminal_output(command: &str) -> Value {
    if command == "status" {
        json!({
            "schema_version": 1,
            "role": "maker",
            "state": "active",
            "phase": "completed",
            "revision": 4,
            "next_action": "complete"
        })
    } else {
        json!({
            "schema_version": 1,
            "role": "maker",
            "command": "drive",
            "outcome": "completed",
            "phase": "completed",
            "revision": 4,
            "next_action": "complete"
        })
    }
}

fn effect_path(state_database: &Path) -> PathBuf {
    state_database.with_extension("m5-effect.json")
}

fn persist_effect(path: &Path, swap_id: &str) -> Result<(), &'static str> {
    let bytes = serde_json::to_vec(&json!({
        "schema_version": 1,
        "kind": "node_free_scheduler_fixture_effect",
        "swap_id": swap_id,
        "submission_count": 1
    }))
    .map_err(|_| "fixture effect is unavailable")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| "fixture effect already exists")?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| "fixture effect is unavailable")?;
    let parent = path
        .parent()
        .ok_or("fixture effect parent is unavailable")?;
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "fixture effect is unavailable")
}
