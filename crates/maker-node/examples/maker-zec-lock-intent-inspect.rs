//! Secret-free inspection of one durable Zcash lock intent.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use clap::Parser as _;
use lez_swap_core::Participant;
use lez_swap_store::SqliteZecRecoveryStore;
use lez_zec_swap_sdk::{FirstLockPlanV1, RecoveryStore as _};
use serde::Serialize;
use zcash_primitives::transaction::TxId;
use zec_reference_actor::{ActorConfig, ActorRole, validate_rebound_actor_pair};

#[derive(clap::Parser)]
#[command(about = "Inspect the exact durable role-owned Zcash funding identity")]
struct Arguments {
    /// Existing owner-private funding actor config.
    #[arg(long)]
    config: PathBuf,
    /// Existing finalized owner-private peer actor config for exact pair validation.
    #[arg(long, visible_alias = "taker-config")]
    peer_config: PathBuf,
}

#[derive(Serialize)]
struct LockIntentOutput {
    schema_version: u16,
    swap_id: Box<str>,
    role: &'static str,
    operation: &'static str,
    staged_revision: u64,
    expected_submission_id_internal_hex: String,
    expected_zebra_txid: String,
    actor_pair_validated: bool,
    exact_submission_disclosed: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let config = ActorConfig::load_private(&arguments.config)
        .context("load private daemon-provisioned Maker actor config")?;
    let peer = ActorConfig::load_private(&arguments.peer_config)
        .context("load private finalized peer actor config")?;
    let (participant, role) = match config.role() {
        ActorRole::Maker => {
            validate_rebound_actor_pair(&config, &peer)
                .context("Maker and finalized Taker are not one valid pair")?;
            (Participant::Maker, "maker")
        }
        ActorRole::Taker => {
            validate_rebound_actor_pair(&peer, &config)
                .context("finalized Maker and Taker are not one valid pair")?;
            (Participant::Taker, "taker")
        }
    };
    let store = SqliteZecRecoveryStore::open(config.role_state_db(), participant)
        .context("open funding actor state")?;
    let (staged_revision, plan) = match config.role() {
        ActorRole::Maker => {
            let intent = store
                .load_maker_lock_intent(config.swap_id())
                .await
                .context("load durable Maker lock intent")?
                .context("durable Maker lock intent is absent")?;
            (intent.staged_revision(), intent.plan().clone())
        }
        ActorRole::Taker => {
            let intent = store
                .load_first_lock_intent(config.swap_id())
                .await
                .context("load durable Taker first-lock intent")?
                .context("durable Taker first-lock intent is absent")?;
            (intent.predecessor_revision(), intent.plan().clone())
        }
    };
    let FirstLockPlanV1::Zcash { funding } = plan else {
        bail!("durable role-owned lock intent is not Zcash funding");
    };
    let expected = *funding.expected_submission_id();
    let output = LockIntentOutput {
        schema_version: 1,
        swap_id: config.swap_id().as_str().into(),
        role,
        operation: "zcash_fund",
        staged_revision,
        expected_submission_id_internal_hex: hex::encode(expected),
        expected_zebra_txid: zebra_txid(expected),
        exact_submission_disclosed: false,
        actor_pair_validated: true,
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

fn zebra_txid(internal: [u8; 32]) -> String {
    TxId::from_bytes(internal).to_string()
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::Arguments;
    use super::zebra_txid;

    #[test]
    fn zebra_display_uses_the_consensus_txid_conversion() {
        let mut internal = [0_u8; 32];
        for (index, byte) in internal.iter_mut().enumerate() {
            *byte = u8::try_from(index).unwrap();
        }
        assert_eq!(
            zebra_txid(internal),
            "1f1e1d1c1b1a191817161514131211100f0e0d0c0b0a09080706050403020100"
        );
    }

    #[test]
    fn peer_flag_and_legacy_taker_alias_select_the_same_input() {
        let peer = Arguments::try_parse_from([
            "intent-inspect",
            "--config",
            "/tmp/funder.json",
            "--peer-config",
            "/tmp/peer.json",
        ])
        .unwrap();
        let legacy = Arguments::try_parse_from([
            "intent-inspect",
            "--config",
            "/tmp/funder.json",
            "--taker-config",
            "/tmp/peer.json",
        ])
        .unwrap();
        assert_eq!(peer.config, legacy.config);
        assert_eq!(peer.peer_config, legacy.peer_config);
    }
}
