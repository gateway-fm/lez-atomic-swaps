//! One-shot crash-isolated host for the provisional Logos price C API.

use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use lez_logos_price_c_api::{AbiDirectionV1, AbiPairV1, MAX_QUOTE_AGE_SECONDS, query_module_once};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Absolute path to the pinned Logos price module shared library.
    #[arg(long)]
    library: PathBuf,
    /// Numeric ABI v1 pair identifier.
    #[arg(long)]
    pair: u32,
    /// Numeric ABI v1 direction identifier.
    #[arg(long)]
    direction: u32,
    /// Daemon-supplied current Unix time.
    #[arg(long)]
    now_unix_seconds: u64,
    /// Maximum acceptable source observation age.
    #[arg(long, default_value_t = 30)]
    max_age_seconds: u64,
}

fn main() -> ExitCode {
    let arguments = Arguments::parse();
    match run(&arguments) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("price worker failed closed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: &Arguments) -> Result<String, Box<dyn std::error::Error>> {
    if !(1..=MAX_QUOTE_AGE_SECONDS).contains(&arguments.max_age_seconds) {
        return Err("maximum quote age must be between 1 and 3600 seconds".into());
    }
    let pair = AbiPairV1::try_from(arguments.pair)?;
    let direction = AbiDirectionV1::try_from(arguments.direction)?;
    let response = query_module_once(
        &arguments.library,
        pair,
        direction,
        arguments.now_unix_seconds,
        arguments.max_age_seconds,
    )?;
    Ok(serde_json::to_string(&response)?)
}
