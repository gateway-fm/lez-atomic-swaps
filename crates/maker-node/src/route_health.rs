//! Hash-pinned bounded process adapter for route-scoped semantic chain health.

use std::{
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};

use lez_swap_store::MakerRouteV1;
use serde::Deserialize;
use thiserror::Error;
use wait_timeout::ChildExt as _;

use crate::{
    MakerDependencyStateV1, MakerRouteHealthProbe, logos_price_source::validate_secure_file,
};

const MAX_CONFIG_BYTES: usize = 64 * 1024;
const MAX_COMMANDS: usize = 12;
const MAX_ARGUMENTS: usize = 32;
const MAX_ARGUMENT_BYTES: usize = 512;
const MAX_TIMEOUT: Duration = Duration::from_secs(5);

/// Invalid immutable health-command configuration.
#[derive(Debug, Error)]
pub enum RouteHealthProbeConfigError {
    /// The JSON shape, bounds, route set, digest, or executable identity is invalid.
    #[error("route health probe configuration is invalid")]
    Invalid,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteHealthConfigV1 {
    schema_version: u16,
    commands: Vec<RouteHealthCommandConfigV1>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteHealthCommandConfigV1 {
    route: MakerRouteV1,
    program: PathBuf,
    program_sha256: Box<str>,
    args: Vec<Box<str>>,
    timeout_milliseconds: u64,
}

#[derive(Clone)]
struct RouteHealthCommandV1 {
    route: MakerRouteV1,
    program: PathBuf,
    program_sha256: [u8; 32],
    args: Vec<Box<str>>,
    timeout: Duration,
}

/// Route-keyed semantic health commands executed without a shell or inherited environment.
#[derive(Clone)]
pub struct ProcessRouteHealthProbe {
    commands: Vec<RouteHealthCommandV1>,
}

impl std::fmt::Debug for ProcessRouteHealthProbe {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProcessRouteHealthProbe")
            .field("command_count", &self.commands.len())
            .field("commands", &"[REDACTED]")
            .finish()
    }
}

impl ProcessRouteHealthProbe {
    /// Parses and validates a bounded strict JSON configuration.
    ///
    /// A route may have multiple commands (for example one LEZ and one foreign-chain check);
    /// every command must succeed for that route to be available. Programs are re-hashed on
    /// every observation and run without a shell, inherited environment, stdin, or output.
    ///
    /// # Errors
    ///
    /// Rejects oversized or malformed JSON, unsupported schemas, empty/oversized command sets,
    /// unsafe or changed executables, invalid SHA-256 values, arguments, and timeouts.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, RouteHealthProbeConfigError> {
        if bytes.is_empty() || bytes.len() > MAX_CONFIG_BYTES {
            return Err(RouteHealthProbeConfigError::Invalid);
        }
        let config: RouteHealthConfigV1 =
            serde_json::from_slice(bytes).map_err(|_| RouteHealthProbeConfigError::Invalid)?;
        if config.schema_version != 1
            || config.commands.is_empty()
            || config.commands.len() > MAX_COMMANDS
        {
            return Err(RouteHealthProbeConfigError::Invalid);
        }
        let mut commands = Vec::with_capacity(config.commands.len());
        for command in config.commands {
            if command.program_sha256.len() != 64
                || command.args.len() > MAX_ARGUMENTS
                || command
                    .args
                    .iter()
                    .any(|arg| arg.is_empty() || arg.len() > MAX_ARGUMENT_BYTES)
            {
                return Err(RouteHealthProbeConfigError::Invalid);
            }
            let mut program_sha256 = [0_u8; 32];
            hex::decode_to_slice(command.program_sha256.as_ref(), &mut program_sha256)
                .map_err(|_| RouteHealthProbeConfigError::Invalid)?;
            let timeout = Duration::from_millis(command.timeout_milliseconds);
            if timeout.is_zero() || timeout > MAX_TIMEOUT {
                return Err(RouteHealthProbeConfigError::Invalid);
            }
            validate_secure_file(&command.program, true, Some(program_sha256))
                .map_err(|_| RouteHealthProbeConfigError::Invalid)?;
            commands.push(RouteHealthCommandV1 {
                route: command.route,
                program: command.program,
                program_sha256,
                args: command.args,
                timeout,
            });
        }
        Ok(Self { commands })
    }

    fn command_is_healthy(command: &RouteHealthCommandV1) -> bool {
        if validate_secure_file(&command.program, true, Some(command.program_sha256)).is_err() {
            return false;
        }
        let Ok(mut child) = Command::new(&command.program)
            .args(command.args.iter().map(AsRef::as_ref))
            .env_clear()
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };
        let status = match child.wait_timeout(command.timeout) {
            Ok(Some(status)) => status,
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Err(_) => return false,
        };
        status.success()
            && validate_secure_file(&command.program, true, Some(command.program_sha256)).is_ok()
    }
}

impl MakerRouteHealthProbe for ProcessRouteHealthProbe {
    fn state(&self, route: MakerRouteV1) -> MakerDependencyStateV1 {
        let mut selected = self
            .commands
            .iter()
            .filter(|command| command.route == route);
        let Some(first) = selected.next() else {
            return MakerDependencyStateV1::Disabled;
        };
        if Self::command_is_healthy(first) && selected.all(Self::command_is_healthy) {
            MakerDependencyStateV1::Available
        } else {
            MakerDependencyStateV1::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    use lez_swap_core::{Pair, SwapDirection};
    use sha2::{Digest as _, Sha256};
    use tempfile::tempdir;

    use super::*;

    fn config(program: &std::path::Path, digest: [u8; 32], args: &[&str]) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "commands": [{
                "route": {"pair": "Zcash", "direction": "TakerSellsLez"},
                "program": program,
                "program_sha256": hex::encode(digest),
                "args": args,
                "timeout_milliseconds": 100
            }]
        }))
        .unwrap()
    }

    #[test]
    fn command_exit_timeout_output_and_identity_are_fail_closed() {
        let run = tempdir().unwrap();
        fs::set_permissions(run.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let program = run.path().join("health");
        fs::write(&program, b"#!/bin/sh\nexit \"$1\"\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o500)).unwrap();
        let digest: [u8; 32] = Sha256::digest(fs::read(&program).unwrap()).into();
        let route = MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).unwrap();
        let healthy_config = config(&program, digest, &["0"]);
        let parsed: RouteHealthConfigV1 = serde_json::from_slice(&healthy_config).unwrap();
        assert_eq!(parsed.schema_version, 1);
        assert!(validate_secure_file(&program, true, None).is_ok());
        assert!(validate_secure_file(&program, true, Some(digest)).is_ok());
        let healthy = ProcessRouteHealthProbe::from_json_bytes(&healthy_config).unwrap();
        assert_eq!(healthy.state(route), MakerDependencyStateV1::Available);
        let failed =
            ProcessRouteHealthProbe::from_json_bytes(&config(&program, digest, &["7"])).unwrap();
        assert_eq!(failed.state(route), MakerDependencyStateV1::Unavailable);
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(&program, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o500)).unwrap();
        assert_eq!(healthy.state(route), MakerDependencyStateV1::Unavailable);
    }
}
