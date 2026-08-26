use std::{fmt, path::PathBuf};

use clap::Parser;
use lez_bridge_protocol::{Hex32, RequestId};
use lez_swap_core::UnixSeconds;
use zec_reference_actor::prepare_local_v0_2_chat_draft;

#[derive(Clone, Parser)]
#[command(about = "Bind validated local Zcash/LEZ chain facts to one Delivery/Chat session")]
struct Arguments {
    /// Existing private countersigned agreement used only as a validated chain-fact template.
    #[arg(long, value_name = "PRIVATE_BORSH")]
    source_agreement_file: PathBuf,
    /// Trusted wall clock used to validate the template and new draft.
    #[arg(long)]
    now_unix_seconds: u64,
    /// Exact reservation ID returned by the maker's Chat endpoint.
    #[arg(long)]
    reservation_id: String,
    /// Lowercase SHA-256 commitment to the authenticated Delivery offer.
    #[arg(long)]
    offer_commitment: String,
    /// Exclusive Delivery offer expiry copied into the countersigned transcript.
    #[arg(long)]
    offer_expires_at_unix_seconds: u64,
    /// New owner-private unsigned draft file consumed by the maker daemon.
    #[arg(long, value_name = "NEW_PRIVATE_BORSH")]
    output_file: PathBuf,
}

impl fmt::Debug for Arguments {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Arguments")
            .field("source_agreement_file", &"[REDACTED]")
            .field("now_unix_seconds", &self.now_unix_seconds)
            .field("reservation_id", &"[REDACTED]")
            .field("offer_commitment", &"[REDACTED]")
            .field(
                "offer_expires_at_unix_seconds",
                &self.offer_expires_at_unix_seconds,
            )
            .field("output_file", &"[REDACTED]")
            .finish()
    }
}

fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    let reservation_id = RequestId::new(arguments.reservation_id)?;
    let offer_commitment = Hex32::from_hex(&arguments.offer_commitment)?;
    let summary = prepare_local_v0_2_chat_draft(
        &arguments.source_agreement_file,
        UnixSeconds::new(arguments.now_unix_seconds),
        reservation_id,
        offer_commitment,
        arguments.offer_expires_at_unix_seconds,
        &arguments.output_file,
    )?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
