//! Prepare one sealed XMR claim-authorization release journal.

#![forbid(unsafe_code)]

use std::{
    io::{self, Write as _},
    path::PathBuf,
};

use clap::Parser;
use lez_v0_2_xmr_release_service::{
    XmrReleasePreparationPaths, prepare_xmr_release_service, read_xmr_release_preparation_config,
    read_xmr_release_service_config,
};

/// Prove the local happy-path preconditions and create one release journal.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Existing public release-service configuration.
    #[arg(long)]
    public_config_file: PathBuf,
    /// Existing public preparation configuration.
    #[arg(long)]
    preparation_config_file: PathBuf,
    /// Canonical countersigned Stage-A wire.
    #[arg(long)]
    agreement_wire_file: PathBuf,
    /// Canonical countersigned Stage-B wire.
    #[arg(long)]
    activation_wire_file: PathBuf,
    /// Owner-private shared Monero view key.
    #[arg(long)]
    monero_view_key_file: PathBuf,
    /// Completed owner-private Taker claim journal.
    #[arg(long)]
    taker_claim_journal: PathBuf,
    /// Ordinary Taker-side bridge capability.
    #[arg(long)]
    bridge_capability_file: PathBuf,
    /// Release-journal protection key.
    #[arg(long)]
    protection_key_file: PathBuf,
    /// Existing owner-private release state directory.
    #[arg(long)]
    state_directory: PathBuf,
    /// Monero daemon RPC username file.
    #[arg(long)]
    daemon_username_file: PathBuf,
    /// Monero daemon RPC password file.
    #[arg(long)]
    daemon_password_file: PathBuf,
    /// Shared-wallet RPC username file.
    #[arg(long)]
    target_wallet_username_file: PathBuf,
    /// Shared-wallet RPC password file.
    #[arg(long)]
    target_wallet_password_file: PathBuf,
    /// Foreign-wallet RPC username file.
    #[arg(long)]
    foreign_wallet_username_file: PathBuf,
    /// Foreign-wallet RPC password file.
    #[arg(long)]
    foreign_wallet_password_file: PathBuf,
}

#[tokio::main]
async fn main() {
    if Box::pin(execute(Arguments::parse())).await.is_err() {
        eprintln!("XMR release preparation failed");
        std::process::exit(1);
    }
}

async fn execute(arguments: Arguments) -> Result<(), ()> {
    let release = read_xmr_release_service_config(&arguments.public_config_file).map_err(|_| ())?;
    let preparation =
        read_xmr_release_preparation_config(&arguments.preparation_config_file).map_err(|_| ())?;
    let paths = XmrReleasePreparationPaths {
        agreement_wire_file: arguments.agreement_wire_file,
        activation_wire_file: arguments.activation_wire_file,
        monero_view_key_file: arguments.monero_view_key_file,
        taker_claim_journal: arguments.taker_claim_journal,
        bridge_capability_file: arguments.bridge_capability_file,
        protection_key_file: arguments.protection_key_file,
        state_directory: arguments.state_directory,
        daemon_username_file: arguments.daemon_username_file,
        daemon_password_file: arguments.daemon_password_file,
        target_wallet_username_file: arguments.target_wallet_username_file,
        target_wallet_password_file: arguments.target_wallet_password_file,
        foreign_wallet_username_file: arguments.foreign_wallet_username_file,
        foreign_wallet_password_file: arguments.foreign_wallet_password_file,
    };
    let report = Box::pin(prepare_xmr_release_service(release, preparation, &paths))
        .await
        .map_err(|_| ())?;
    serde_json::to_writer(io::stdout().lock(), &report).map_err(|_| ())?;
    println!();
    io::stdout().flush().map_err(|_| ())
}
