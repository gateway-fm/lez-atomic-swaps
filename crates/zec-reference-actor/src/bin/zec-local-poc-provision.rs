use std::{fmt, path::PathBuf};

use clap::Parser;
use zec_reference_actor::provision_local_v0_2_corridor;

#[derive(Clone, Parser)]
#[command(about = "Provision one private deterministic LEZ-v0.2/Zebra corridor fixture")]
struct Arguments {
    /// Owner-private JSON containing non-secret, manifest-derived runtime facts.
    #[arg(long, value_name = "PRIVATE_JSON")]
    spec_file: PathBuf,
    /// New absolute directory that will contain the isolated actor inputs.
    #[arg(long, value_name = "NEW_PRIVATE_DIRECTORY")]
    output_root: PathBuf,
}

impl fmt::Debug for Arguments {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Arguments")
            .field("spec_file", &"[REDACTED]")
            .field("output_root", &"[REDACTED]")
            .finish()
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    let summary =
        provision_local_v0_2_corridor(&arguments.spec_file, &arguments.output_root).await?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
