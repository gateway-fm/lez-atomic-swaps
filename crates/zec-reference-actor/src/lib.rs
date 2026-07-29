//! Secure configuration and one-shot command boundary for role-fixed actors.

#[cfg(not(unix))]
compile_error!("zec-reference-actor requires Unix file permissions and inode identity");

use std::{fmt, path::PathBuf};

use clap::{ArgGroup, Parser, Subcommand};
use lez_swap_store::MAKER_ACTOR_CONFIG_FD;

mod command;
mod config;
mod local_poc;
mod maker_provision;
mod secure_file;
#[cfg(feature = "test-crash-hooks")]
mod test_crash_hook;

pub use command::{
    ActorCommandError, ActorCommandOutputV1, ActorEffectOutputV1, ActorStatusV1,
    execute_actor_command,
};
pub use config::{
    ActivateMaterial, ActorConfig, ActorConfigError, ActorRole, CandidateOutpoint, DriveMaterial,
    StatusMaterial, ZcashNetworkConfig, ZebraRpcChainConfig, validate_actor_pair,
    validate_maker_manifest_config_bytes, validate_rebound_actor_pair,
};
pub use local_poc::{
    LocalPocChatDraftSummary, LocalPocChatFinalizeSummary, LocalPocLezSignerFiles,
    LocalPocProvisionSummary, finalize_local_v0_2_chat_corridor, prepare_local_v0_2_chat_draft,
    provision_local_v0_2_corridor, provision_local_v0_2_corridor_with_signers,
};
pub use maker_provision::{
    ZecActorProvisionV1, ZecMakerActorProvisionV1, provision_zec_maker_actor_from_chat,
    provision_zec_maker_actor_from_config, provision_zec_taker_actor_from_chat,
    provision_zec_taker_actor_from_config,
};
#[cfg(feature = "test-crash-hooks")]
pub use test_crash_hook::{TestCrashHookError, arm_test_crash_hook};

/// Exactly one lifecycle action performed by an actor process.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Subcommand)]
pub enum ActorCommand {
    /// Validate and durably activate the signed agreement.
    Activate,
    /// Reconcile and attempt one eligible chain effect.
    Drive,
    /// Reconcile and attempt only one agreement-ordered claim effect.
    Claim,
    /// Reconcile and attempt one agreement-ordered timeout recovery effect.
    Recover,
    /// Return a secret-free durable status snapshot.
    Status,
}

/// Process arguments for the one-shot actor.
#[derive(Clone, Parser)]
#[command(
    about = "One-shot role-fixed LEZ/Zcash reference actor",
    group(ArgGroup::new("config_source").required(true).multiple(false).args(["config", "config_fd"]))
)]
pub struct ActorCli {
    /// Owner-private, bounded JSON configuration.
    #[arg(long, value_name = "PRIVATE_JSON")]
    pub config: Option<PathBuf>,
    /// Fixed inherited descriptor containing one anonymous, fully sealed configuration.
    #[arg(long, value_name = "FD", value_parser = parse_config_fd)]
    pub config_fd: Option<i32>,
    /// Single lifecycle action; the process exits after it completes.
    #[command(subcommand)]
    pub command: ActorCommand,
}

fn parse_config_fd(value: &str) -> Result<i32, String> {
    let fd = value
        .parse::<i32>()
        .map_err(|_| "invalid config descriptor".to_owned())?;
    if fd == MAKER_ACTOR_CONFIG_FD {
        Ok(fd)
    } else {
        Err(format!("config descriptor must be {MAKER_ACTOR_CONFIG_FD}"))
    }
}

impl fmt::Debug for ActorCli {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActorCli")
            .field("config", &"[REDACTED]")
            .field("config_fd", &"[REDACTED]")
            .field("command", &self.command)
            .finish()
    }
}
