//! Secret-free inspection of one durable Maker Zcash lock intent.

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
#[command(about = "Inspect the exact durable Maker Zcash funding identity")]
struct Arguments {
    /// Existing owner-private daemon-provisioned Maker actor config.
    #[arg(long)]
    config: PathBuf,
    /// Existing finalized owner-private Taker actor config for exact pair validation.
    #[arg(long)]
    taker_config: PathBuf,
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
    if config.role() != ActorRole::Maker {
        bail!("lock-intent inspection requires the Maker role");
    }
    let taker = ActorConfig::load_private(&arguments.taker_config)
        .context("load private finalized Taker actor config")?;
    validate_rebound_actor_pair(&config, &taker)
        .context("daemon-provisioned Maker and finalized Taker are not one valid pair")?;
    let store = SqliteZecRecoveryStore::open(config.role_state_db(), Participant::Maker)
        .context("open Maker actor state")?;
    let intent = store
        .load_maker_lock_intent(config.swap_id())
        .await
        .context("load durable Maker lock intent")?
        .context("durable Maker lock intent is absent")?;
    let FirstLockPlanV1::Zcash { funding } = intent.plan() else {
        bail!("durable Maker lock intent is not Zcash funding");
    };
    let expected = *funding.expected_submission_id();
    let output = LockIntentOutput {
        schema_version: 1,
        swap_id: config.swap_id().as_str().into(),
        role: "maker",
        operation: "zcash_fund",
        staged_revision: intent.staged_revision(),
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
}
