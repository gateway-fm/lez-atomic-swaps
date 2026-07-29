//! Secret-free validation of one separately published ZEC actor pair.

use anyhow::{Context as _, Result, ensure};
use clap::Parser as _;
use serde::Serialize;
use zec_reference_actor::{ActorConfig, ActorRole, validate_rebound_actor_pair};

#[derive(clap::Parser)]
#[command(about = "Validate one independently published Maker and Taker actor pair")]
struct Arguments {
    /// Existing owner-private daemon-provisioned Maker actor config.
    #[arg(long)]
    maker_config: std::path::PathBuf,
    /// Existing owner-private acceptance-provisioned Taker actor config.
    #[arg(long)]
    taker_config: std::path::PathBuf,
}

#[derive(Serialize)]
struct Output {
    schema_version: u16,
    swap_id: Box<str>,
    actor_pair_validated: bool,
    private_material_disclosed: bool,
}

fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let maker = ActorConfig::load_private(&arguments.maker_config)
        .context("load private daemon-provisioned Maker actor config")?;
    let taker = ActorConfig::load_private(&arguments.taker_config)
        .context("load private acceptance-provisioned Taker actor config")?;
    ensure!(
        maker.role() == ActorRole::Maker && taker.role() == ActorRole::Taker,
        "actor configs do not have the required Maker and Taker roles"
    );
    validate_rebound_actor_pair(&maker, &taker)
        .context("independently published actors are not one valid pair")?;
    ensure!(
        maker.swap_id() == taker.swap_id(),
        "validated actors do not have one swap ID"
    );

    let output = Output {
        schema_version: 1,
        swap_id: maker.swap_id().as_str().into(),
        actor_pair_validated: true,
        private_material_disclosed: false,
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}
