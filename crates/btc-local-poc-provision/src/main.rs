#![forbid(unsafe_code)]

use std::{
    io::{self, Write as _},
    path::PathBuf,
    process::ExitCode,
};

use anyhow::{Context as _, Result};
use btc_local_poc_provision::{
    export_draft, finalize_asset_extension, finalize_stage2, generate_stage1, prepare_funding,
};
use clap::{Parser, Subcommand};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    #[command(subcommand)]
    action: Action,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Generate fresh private keys and a secret-free public planning document.
    Generate {
        /// Strict owner-private planning JSON.
        #[arg(long)]
        planning_file: PathBuf,
        /// New normalized absolute owner-private fixture root.
        #[arg(long)]
        output_root: PathBuf,
    },
    /// Offline-sign an exact funding transaction from one actual rawtr service UTXO.
    PrepareFunding {
        /// Strict owner-private funding-preparation JSON.
        #[arg(long)]
        spec_file: PathBuf,
        /// Existing stage-one normalized absolute fixture root.
        #[arg(long)]
        output_root: PathBuf,
    },
    /// Bind actual local-node facts and emit a countersigned canonical agreement.
    Finalize {
        /// Strict owner-private finalization JSON.
        #[arg(long)]
        spec_file: PathBuf,
        /// Existing stage-one normalized absolute fixture root.
        #[arg(long)]
        output_root: PathBuf,
    },
    /// Bind and countersign one custom-token selection to a finalized agreement.
    FinalizeAssetExtension {
        /// Strict owner-private custom-token policy JSON.
        #[arg(long)]
        spec_file: PathBuf,
        /// Existing finalized normalized absolute fixture root.
        #[arg(long)]
        output_root: PathBuf,
    },
    /// Extract one canonical unsigned application draft from a finalized agreement.
    ExportDraft {
        /// Existing owner-private canonical finalized agreement.
        #[arg(long)]
        agreement_file: PathBuf,
        /// New owner-private no-clobber canonical draft file.
        #[arg(long)]
        output_file: PathBuf,
    },
}

fn main() -> ExitCode {
    match execute(Arguments::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("local Bitcoin PoC provisioning failed: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn execute(arguments: Arguments) -> Result<()> {
    match arguments.action {
        Action::Generate {
            planning_file,
            output_root,
        } => print_json(&generate_stage1(&planning_file, &output_root)?),
        Action::PrepareFunding {
            spec_file,
            output_root,
        } => print_json(&prepare_funding(&spec_file, &output_root)?),
        Action::Finalize {
            spec_file,
            output_root,
        } => print_json(&finalize_stage2(&spec_file, &output_root)?),
        Action::FinalizeAssetExtension {
            spec_file,
            output_root,
        } => print_json(&finalize_asset_extension(&spec_file, &output_root)?),
        Action::ExportDraft {
            agreement_file,
            output_file,
        } => print_json(&export_draft(&agreement_file, &output_file)?),
    }
}

fn print_json(value: &impl Serialize) -> Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, value).context("serialize public output")?;
    output.write_all(b"\n").context("write public output")
}
