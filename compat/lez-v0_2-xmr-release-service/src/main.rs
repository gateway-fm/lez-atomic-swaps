#![forbid(unsafe_code)]

use std::{
    fs,
    io::{self, Write as _},
    path::PathBuf,
};

use anyhow::{Context as _, Result, ensure};
use clap::Parser;
use lez_v0_2_xmr_release_service::{
    XmrReleaseServicePaths, read_xmr_release_service_config, run_xmr_release_service_once,
};

/// Publish one prepared XMR claim authorization from a sealed journal.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Bounded public JSON containing exact run, runtime, terms, and routes.
    #[arg(long)]
    public_config_file: PathBuf,
    /// Existing owner-private 0700 directory containing xmr-release.sqlite3.
    #[arg(long)]
    state_directory: PathBuf,
    /// Owner-private 0600 release-sidecar bearer file.
    #[arg(long)]
    sidecar_capability_file: PathBuf,
    /// Owner-private 0600 lowercase-hex journal protection-key file.
    #[arg(long)]
    protection_key_file: PathBuf,
}

#[tokio::main]
async fn main() {
    if let Err(error) = execute(Arguments::parse()).await {
        eprintln!("XMR release process failed: {error}");
        std::process::exit(1);
    }
}

async fn execute(arguments: Arguments) -> Result<()> {
    validate_distinct_paths(&arguments)?;
    let config = read_xmr_release_service_config(&arguments.public_config_file)
        .context("invalid public release configuration")?;
    let paths = XmrReleaseServicePaths::new(
        arguments.sidecar_capability_file,
        arguments.protection_key_file,
        arguments.state_directory,
    );
    let report = run_xmr_release_service_once(config, &paths)
        .await
        .context("release attempt failed")?;
    serde_json::to_writer(io::stdout().lock(), &report).context("encode release report")?;
    println!();
    io::stdout().flush().context("flush release report")?;
    ensure!(
        report.is_durably_admitted(),
        "release is not durably admitted"
    );
    Ok(())
}

fn validate_distinct_paths(arguments: &Arguments) -> Result<()> {
    let paths = [
        &arguments.public_config_file,
        &arguments.state_directory,
        &arguments.sidecar_capability_file,
        &arguments.protection_key_file,
    ];
    let canonical = paths
        .iter()
        .map(fs::canonicalize)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("release process file layout is unavailable")?;
    for (index, path) in canonical.iter().enumerate() {
        for other in canonical.iter().skip(index + 1) {
            ensure!(path != other, "release process files must be distinct");
        }
    }
    Ok(())
}
