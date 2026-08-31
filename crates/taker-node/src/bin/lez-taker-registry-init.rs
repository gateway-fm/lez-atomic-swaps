use std::path::PathBuf;

use anyhow::{Context as _, ensure};
use clap::Parser as _;
use lez_swap_store::SqliteTakerFacadeStore;

#[derive(clap::Parser)]
#[command(about = "Exclusively initialize one owner-private Taker facade registry")]
struct Arguments {
    /// New absolute owner-private registry database path.
    #[arg(long)]
    database: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    ensure!(
        arguments.database.is_absolute(),
        "Taker registry database path must be absolute"
    );
    drop(
        SqliteTakerFacadeStore::create_new(&arguments.database)
            .context("initialize owner-private Taker facade registry")?,
    );
    Ok(())
}
