use std::{fmt, path::PathBuf};

use clap::Parser;
use lez_swap_core::UnixSeconds;
use zec_reference_actor::finalize_local_v0_2_chat_corridor;

#[derive(Clone, Parser)]
#[command(about = "Finalize isolated actors from one countersigned Delivery/Chat agreement")]
struct Arguments {
    /// Existing owner-private maker config that supplies validated chain facts and authority.
    #[arg(long, value_name = "PRIVATE_JSON")]
    source_maker_config: PathBuf,
    /// Existing owner-private taker config that supplies validated chain facts and authority.
    #[arg(long, value_name = "PRIVATE_JSON")]
    source_taker_config: PathBuf,
    /// Exact owner-private agreement returned by the maker's completed Chat session.
    #[arg(long, value_name = "PRIVATE_BORSH")]
    final_agreement_file: PathBuf,
    /// Trusted wall clock captured when the final agreement is accepted.
    #[arg(long)]
    accepted_at_unix_seconds: u64,
    /// New absolute directory for fresh role-local state and configs.
    #[arg(long, value_name = "NEW_PRIVATE_DIRECTORY")]
    output_root: PathBuf,
}

impl fmt::Debug for Arguments {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Arguments")
            .field("source_maker_config", &"[REDACTED]")
            .field("source_taker_config", &"[REDACTED]")
            .field("final_agreement_file", &"[REDACTED]")
            .field("accepted_at_unix_seconds", &self.accepted_at_unix_seconds)
            .field("output_root", &"[REDACTED]")
            .finish()
    }
}

fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    let summary = finalize_local_v0_2_chat_corridor(
        &arguments.source_maker_config,
        &arguments.source_taker_config,
        &arguments.final_agreement_file,
        UnixSeconds::new(arguments.accepted_at_unix_seconds),
        &arguments.output_root,
    )?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
