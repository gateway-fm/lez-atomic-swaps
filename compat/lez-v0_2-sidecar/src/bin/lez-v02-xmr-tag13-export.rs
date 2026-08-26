#![forbid(unsafe_code)]

use anyhow::{Context as _, Result, ensure};
use clap::Parser;
use lez_bridge_protocol::{Hex32, RunId};
use lez_v0_2_sidecar::{M4Tag13Expectation, export_m4_tag13_handoff};
use std::path::PathBuf;

/// Verify finalized tag-13 state and export both role runtimes, terms, and receipt.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Existing owner-only tag-13 state directory, read in place.
    #[arg(long)]
    state_directory: PathBuf,
    /// Existing empty owner-only output directory.
    #[arg(long)]
    output_directory: PathBuf,
    /// Exact composed run identity.
    #[arg(long)]
    run_id: String,
    /// Expected lowercase SHA-256 of the canonical Stage-A agreement wire.
    #[arg(long)]
    stage_a_agreement_wire_sha256: String,
    /// Expected lowercase SHA-256 of the canonical Stage-B activation wire.
    #[arg(long)]
    stage_b_activation_wire_sha256: String,
    /// Expected deployed authenticated-transfer program ID.
    #[arg(long)]
    authenticated_transfer_program_id: String,
}

fn main() {
    if let Err(error) = execute(Arguments::parse()) {
        eprintln!("M4 tag-13 handoff export failed: {error:#}");
        std::process::exit(1);
    }
}

fn execute(arguments: Arguments) -> Result<()> {
    let authenticated_transfer_program_id =
        Hex32::from_hex(&arguments.authenticated_transfer_program_id)
            .context("invalid authenticated-transfer program ID")?;
    ensure!(
        authenticated_transfer_program_id != Hex32::from_bytes([0; 32]),
        "authenticated-transfer program ID must be nonzero"
    );
    let expected = M4Tag13Expectation::new(
        RunId::new(arguments.run_id).context("invalid run ID")?,
        Hex32::from_hex(&arguments.stage_a_agreement_wire_sha256)
            .context("invalid Stage-A agreement wire SHA-256")?,
        Hex32::from_hex(&arguments.stage_b_activation_wire_sha256)
            .context("invalid Stage-B activation wire SHA-256")?,
        authenticated_transfer_program_id,
    );
    export_m4_tag13_handoff(
        &arguments.state_directory,
        &arguments.output_directory,
        &expected,
    )
    .context("verify and export exact finalized tag-13 handoff")?;
    Ok(())
}
