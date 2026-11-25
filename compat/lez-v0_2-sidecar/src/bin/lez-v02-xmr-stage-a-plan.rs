//! Chain-free, one-shot M4 unsigned Stage-A planner.
//!
//! Reads one JSON `XmrStageAPlanInputV1` on stdin and writes the canonical
//! unsigned Stage-A wire on stdout. It has no chain client: both Nodes run it
//! on identical facts and compare the bytes.

#![forbid(unsafe_code)]

#[cfg(not(target_os = "linux"))]
compile_error!("the M4 Stage-A planner requires Linux file-safety semantics");

use std::io::{Read as _, Write as _};

use anyhow::{Context as _, Result, ensure};
use lez_v0_2_sidecar::plan_m4_stage_a_unsigned_wire;

/// Two hex-encoded role packets of at most 270 KiB each, plus the scalars.
const MAX_INPUT_BYTES: u64 = 2 * 1024 * 1024;

fn main() {
    if let Err(error) = execute() {
        eprintln!("M4 unsigned Stage-A planning failed: {error:#}");
        std::process::exit(1);
    }
}

fn execute() -> Result<()> {
    let mut input = Vec::new();
    std::io::stdin()
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut input)
        .context("read Stage-A plan input")?;
    ensure!(
        input.len() as u64 <= MAX_INPUT_BYTES,
        "Stage-A plan input is oversized"
    );
    let wire = plan_m4_stage_a_unsigned_wire(&input)?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&wire)
        .and_then(|()| stdout.flush())
        .context("write canonical unsigned Stage-A wire")
}
