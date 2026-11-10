use std::{fmt, path::PathBuf};

use clap::Parser;
use url::Url;
use zec_reference_actor::transfer_local_v0_2_regtest;

#[derive(Clone, Parser)]
#[command(
    about = "Move Regtest ZEC between two owner-private keys with one plain transparent transaction"
)]
struct Arguments {
    /// Loopback Zebra RPC endpoint of the deterministic Regtest fixture.
    #[arg(long, value_name = "URL")]
    zebra_endpoint: Url,
    /// Owner-private sender key (must own a mature P2PKH output).
    #[arg(long, value_name = "PRIVATE_KEY")]
    from_zcash_key_file: PathBuf,
    /// Owner-private recipient key.
    #[arg(long, value_name = "PRIVATE_KEY")]
    to_zcash_key_file: PathBuf,
    /// Value to pay the recipient, zatoshis.
    #[arg(long, value_name = "ZATOSHIS")]
    value_zatoshis: u64,
    /// Mature sender outputs to pass over before spending one.
    #[arg(long, value_name = "N", default_value_t = 0)]
    skip: u32,
}

impl fmt::Debug for Arguments {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Arguments")
            .field("zebra_endpoint", &self.zebra_endpoint)
            .field("from_zcash_key_file", &"[REDACTED]")
            .field("to_zcash_key_file", &"[REDACTED]")
            .field("value_zatoshis", &self.value_zatoshis)
            .field("skip", &self.skip)
            .finish()
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    let summary = transfer_local_v0_2_regtest(
        &arguments.zebra_endpoint,
        &arguments.from_zcash_key_file,
        &arguments.to_zcash_key_file,
        arguments.value_zatoshis,
        arguments.skip,
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
