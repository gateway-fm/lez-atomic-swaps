use std::{fmt, path::PathBuf};

use clap::Parser;
use zec_reference_actor::zcash_identity_from_key_file;

#[derive(Clone, Parser)]
#[command(
    about = "Print the public identity (key, hash, Regtest address) of an owner-private Zcash key"
)]
struct Arguments {
    /// Owner-private Zcash key: 32 raw bytes or 64 hex characters.
    #[arg(long, value_name = "PRIVATE_KEY")]
    zcash_key_file: PathBuf,
}

impl fmt::Debug for Arguments {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Arguments")
            .field("zcash_key_file", &"[REDACTED]")
            .finish()
    }
}

fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    let identity = zcash_identity_from_key_file(&arguments.zcash_key_file)?;
    println!("{}", serde_json::to_string_pretty(&identity)?);
    Ok(())
}
