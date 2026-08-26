#[path = "../tests/support/mod.rs"]
mod support;

use std::{env, path::PathBuf};

use serde_json::json;
use support::actor_deployment;

fn main() {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    let root = arguments
        .next()
        .map(PathBuf::from)
        .expect("usage: m5-systemd-actor-fixture ABSOLUTE_PRIVATE_ROOT");
    assert!(arguments.next().is_none(), "unexpected extra argument");
    assert!(root.is_absolute(), "fixture root must be absolute");
    let actor = actor_deployment(&root, "m5-integration-authority-001");
    println!(
        "{}",
        json!({
            "source_config": actor.source_config,
            "actor_root": actor.root,
            "fixture_program": actor.program,
            "fixture_program_sha256": actor.program_sha256,
            "claim_key_file": actor.claim_key,
            "claim_preimage_file": actor.claim_preimage,
        })
    );
}
