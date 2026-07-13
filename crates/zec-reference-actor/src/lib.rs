//! Secure configuration and one-shot command boundary for role-fixed actors.

#[cfg(not(unix))]
compile_error!("zec-reference-actor requires Unix file permissions and inode identity");

use std::{fmt, path::PathBuf};

use clap::{Parser, Subcommand};

mod config;
mod secure_file;

pub use config::{
    ActivateMaterial, ActorConfig, ActorConfigError, ActorRole, CandidateOutpoint, DriveMaterial,
    StatusMaterial, ZcashNetworkConfig, ZebraRpcChainConfig, validate_actor_pair,
};

/// Exactly one lifecycle action performed by an actor process.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Subcommand)]
pub enum ActorCommand {
    /// Validate and durably activate the signed agreement.
    Activate,
    /// Reconcile and attempt one eligible chain effect.
    Drive,
    /// Return a secret-free durable status snapshot.
    Status,
}

/// Process arguments for the one-shot actor.
#[derive(Clone, Parser)]
#[command(about = "One-shot role-fixed LEZ/Zcash reference actor")]
pub struct ActorCli {
    /// Owner-private, bounded JSON configuration.
    #[arg(long, value_name = "PRIVATE_JSON")]
    pub config: PathBuf,
    /// Single lifecycle action; the process exits after it completes.
    #[command(subcommand)]
    pub command: ActorCommand,
}

impl fmt::Debug for ActorCli {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActorCli")
            .field("config", &"[REDACTED]")
            .field("command", &self.command)
            .finish()
    }
}
