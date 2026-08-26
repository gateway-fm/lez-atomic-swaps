#![forbid(unsafe_code)]

use std::{io, path::PathBuf, process::ExitCode};

use anyhow::{Context as _, Result};
use btc_role_preflight::{bind_countersigned_agreement, bootstrap_role, compose_agreement_draft};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    #[command(subcommand)]
    action: Action,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Create one role's authority and signed public contribution.
    Bootstrap {
        /// Strict owner-private role bootstrap JSON.
        #[arg(long)]
        spec_file: PathBuf,
        /// New normalized absolute owner-private role root.
        #[arg(long)]
        output_root: PathBuf,
    },
    /// Compose a canonical unsigned draft from public role contributions and chain facts.
    ComposeDraft {
        /// Strict owner-private observed-chain and recovery-policy JSON.
        #[arg(long)]
        spec_file: PathBuf,
        /// Exact signed Maker role contribution.
        #[arg(long)]
        maker_contribution_file: PathBuf,
        /// Exact signed Taker role contribution.
        #[arg(long)]
        taker_contribution_file: PathBuf,
        /// New normalized absolute output root for the draft and public summary.
        #[arg(long)]
        output_root: PathBuf,
    },
    /// Bind a final countersigned agreement to one role root and its peer.
    BindAgreement {
        /// Existing normalized absolute owner-private role root.
        #[arg(long)]
        role_root: PathBuf,
        /// Exact peer contribution exchanged through Chat.
        #[arg(long)]
        peer_contribution_file: PathBuf,
        /// Exact final countersigned agreement.
        #[arg(long)]
        agreement_file: PathBuf,
        /// Trusted local acceptance time.
        #[arg(long)]
        accepted_at_unix_seconds: u64,
    },
}

fn main() -> ExitCode {
    match execute(Arguments::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Bitcoin role preflight failed: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn execute(arguments: Arguments) -> Result<()> {
    match arguments.action {
        Action::Bootstrap {
            spec_file,
            output_root,
        } => print_json(&bootstrap_role(&spec_file, &output_root)?),
        Action::ComposeDraft {
            spec_file,
            maker_contribution_file,
            taker_contribution_file,
            output_root,
        } => print_json(&compose_agreement_draft(
            &spec_file,
            &maker_contribution_file,
            &taker_contribution_file,
            &output_root,
        )?),
        Action::BindAgreement {
            role_root,
            peer_contribution_file,
            agreement_file,
            accepted_at_unix_seconds,
        } => print_json(&bind_countersigned_agreement(
            &role_root,
            &peer_contribution_file,
            &agreement_file,
            accepted_at_unix_seconds,
        )?),
    }
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    serde_json::to_writer(io::stdout().lock(), value).context("serialize public summary")?;
    println!();
    Ok(())
}
