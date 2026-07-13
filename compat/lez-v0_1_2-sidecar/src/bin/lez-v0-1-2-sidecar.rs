//! Process entry point for one role-isolated official LEZ sidecar.

#![forbid(unsafe_code)]

use std::{
    env,
    ffi::OsString,
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use lez_bridge_protocol::{Hex32, RunId, RuntimeDescriptor};
use lez_v0_1_2_sidecar::{
    BridgeServerCapability, BridgeServerConfig, NativeEscrowPlanner, OfficialNodeRpc, SidecarError,
    start_bridge_server,
};
use nssa::{AccountId, PrivateKey, PublicKey};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use zeroize::Zeroize as _;

const MAX_PUBLIC_CONFIG_BYTES: u64 = 16 * 1024;
const MAX_CAPABILITY_FILE_BYTES: u64 = 256;
const MAX_SIGNER_FILE_BYTES: u64 = 128;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("LEZ sidecar failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), RunnerError> {
    let arguments = Arguments::parse(env::args_os())?;
    if arguments.capability_file == arguments.signer_key_file
        || arguments.capability_file == arguments.state_file
        || arguments.signer_key_file == arguments.state_file
    {
        return Err(RunnerError::InvalidConfiguration);
    }
    let run_id = RunId::new(arguments.run_id).map_err(|_| RunnerError::InvalidConfiguration)?;
    let runtime_bytes = read_regular_file(
        &arguments.runtime_file,
        MAX_PUBLIC_CONFIG_BYTES,
        SecretPolicy::Public,
    )?;
    let runtime: RuntimeDescriptor =
        serde_json::from_slice(&runtime_bytes).map_err(|_| RunnerError::InvalidConfiguration)?;
    let capability = read_secret_text(&arguments.capability_file, MAX_CAPABILITY_FILE_BYTES)?;
    let capability =
        BridgeServerCapability::new(capability).map_err(|_| RunnerError::InvalidConfiguration)?;
    let mut signer_hex = read_secret_text(&arguments.signer_key_file, MAX_SIGNER_FILE_BYTES)?;
    let mut signer_bytes = [0_u8; 32];
    if signer_hex.len() != 64
        || signer_hex
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
        || hex::decode_to_slice(&signer_hex, &mut signer_bytes).is_err()
    {
        signer_hex.zeroize();
        signer_bytes.zeroize();
        return Err(RunnerError::InvalidConfiguration);
    }
    signer_hex.zeroize();
    let signer_key =
        PrivateKey::try_new(signer_bytes).map_err(|_| RunnerError::InvalidConfiguration)?;
    signer_bytes.zeroize();
    let signer_account_id = AccountId::from(&PublicKey::new_from_private_key(&signer_key));
    if runtime.signer_account_id != Hex32::from_bytes(signer_account_id.into_value()) {
        return Err(RunnerError::InvalidConfiguration);
    }
    let escrow_program_id = program_id_words(runtime.escrow_program_id);
    let node = Arc::new(OfficialNodeRpc::connect(
        &arguments.node_endpoint,
        runtime.sidecar_role,
        signer_account_id,
        runtime.clone(),
    )?);
    let planner = Arc::new(NativeEscrowPlanner::new(
        runtime.sidecar_role,
        signer_key,
        escrow_program_id,
        runtime.clone(),
        Arc::clone(&node),
    )?);
    let server = start_bridge_server(
        BridgeServerConfig::new(
            run_id.clone(),
            runtime.clone(),
            capability,
            arguments.state_file,
        ),
        planner,
        node,
    )
    .await?;
    let readiness = Readiness {
        event: "ready",
        endpoint: server.endpoint(),
        run_id: run_id.as_str(),
        runtime: &runtime,
    };
    serde_json::to_writer(io::stdout().lock(), &readiness).map_err(|_| RunnerError::Readiness)?;
    println!();
    io::stdout().flush().map_err(|_| RunnerError::Readiness)?;

    if arguments.shutdown_on_stdin {
        let mut line = String::new();
        BufReader::new(tokio::io::stdin())
            .read_line(&mut line)
            .await
            .map_err(|_| RunnerError::Shutdown)?;
        line.zeroize();
    } else {
        tokio::signal::ctrl_c()
            .await
            .map_err(|_| RunnerError::Shutdown)?;
    }
    server.stop().await?;
    Ok(())
}

#[derive(Debug)]
struct Arguments {
    node_endpoint: String,
    run_id: String,
    runtime_file: PathBuf,
    capability_file: PathBuf,
    signer_key_file: PathBuf,
    state_file: PathBuf,
    shutdown_on_stdin: bool,
}

impl Arguments {
    fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, RunnerError> {
        let mut values = arguments.into_iter();
        let _program = values.next().ok_or(RunnerError::InvalidArguments)?;
        let mut node_endpoint = None;
        let mut run_id = None;
        let mut runtime_file = None;
        let mut capability_file = None;
        let mut signer_key_file = None;
        let mut state_file = None;
        let mut shutdown_on_stdin = false;
        while let Some(flag) = values.next() {
            match flag.to_str().ok_or(RunnerError::InvalidArguments)? {
                "--node-endpoint" => {
                    node_endpoint = Some(next_string(&mut values)?);
                }
                "--run-id" => {
                    run_id = Some(next_string(&mut values)?);
                }
                "--runtime-file" => {
                    runtime_file = Some(next_path(&mut values)?);
                }
                "--capability-file" => {
                    capability_file = Some(next_path(&mut values)?);
                }
                "--signer-key-file" => {
                    signer_key_file = Some(next_path(&mut values)?);
                }
                "--state-file" => {
                    state_file = Some(next_path(&mut values)?);
                }
                "--shutdown-on-stdin" if !shutdown_on_stdin => {
                    shutdown_on_stdin = true;
                }
                _ => return Err(RunnerError::InvalidArguments),
            }
        }
        Ok(Self {
            node_endpoint: node_endpoint.ok_or(RunnerError::InvalidArguments)?,
            run_id: run_id.ok_or(RunnerError::InvalidArguments)?,
            runtime_file: runtime_file.ok_or(RunnerError::InvalidArguments)?,
            capability_file: capability_file.ok_or(RunnerError::InvalidArguments)?,
            signer_key_file: signer_key_file.ok_or(RunnerError::InvalidArguments)?,
            state_file: state_file.ok_or(RunnerError::InvalidArguments)?,
            shutdown_on_stdin,
        })
    }
}

fn next_string(values: &mut impl Iterator<Item = OsString>) -> Result<String, RunnerError> {
    values
        .next()
        .and_then(|value| value.into_string().ok())
        .filter(|value| !value.is_empty())
        .ok_or(RunnerError::InvalidArguments)
}

fn next_path(values: &mut impl Iterator<Item = OsString>) -> Result<PathBuf, RunnerError> {
    values
        .next()
        .map(PathBuf::from)
        .filter(|value| !value.as_os_str().is_empty())
        .ok_or(RunnerError::InvalidArguments)
}

#[derive(Clone, Copy, Debug)]
enum SecretPolicy {
    Public,
    Secret,
}

fn read_regular_file(
    path: &Path,
    maximum: u64,
    policy: SecretPolicy,
) -> Result<Vec<u8>, RunnerError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| RunnerError::ConfigurationFile)?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > maximum {
        return Err(RunnerError::ConfigurationFile);
    }
    #[cfg(unix)]
    if matches!(policy, SecretPolicy::Secret) {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(RunnerError::ConfigurationFile);
        }
    }
    fs::read(path).map_err(|_| RunnerError::ConfigurationFile)
}

fn read_secret_text(path: &Path, maximum: u64) -> Result<String, RunnerError> {
    let mut bytes = read_regular_file(path, maximum, SecretPolicy::Secret)?;
    if bytes.ends_with(b"\r\n") {
        bytes.truncate(bytes.len() - 2);
    } else if bytes.ends_with(b"\n") {
        bytes.truncate(bytes.len() - 1);
    }
    let mut value = String::from_utf8(bytes).map_err(|error| {
        let mut bytes = error.into_bytes();
        bytes.zeroize();
        RunnerError::ConfigurationFile
    })?;
    if value.is_empty() || value.contains('\r') || value.contains('\n') {
        value.zeroize();
        return Err(RunnerError::ConfigurationFile);
    }
    Ok(value)
}

fn program_id_words(program_id: Hex32) -> [u32; 8] {
    let mut words = [0_u32; 8];
    for (word, bytes) in words.iter_mut().zip(program_id.as_bytes().chunks_exact(4)) {
        *word = u32::from_le_bytes(bytes.try_into().expect("four-byte chunk"));
    }
    words
}

#[derive(Serialize)]
struct Readiness<'a> {
    event: &'static str,
    endpoint: &'a str,
    run_id: &'a str,
    runtime: &'a RuntimeDescriptor,
}

#[derive(Debug, thiserror::Error)]
enum RunnerError {
    #[error("invalid sidecar arguments")]
    InvalidArguments,
    #[error("invalid sidecar configuration")]
    InvalidConfiguration,
    #[error("sidecar configuration file is unavailable or unsafe")]
    ConfigurationFile,
    #[error("sidecar node configuration is invalid or unavailable")]
    Sidecar(#[from] SidecarError),
    #[error("sidecar bridge server is unavailable")]
    Server(#[from] lez_v0_1_2_sidecar::BridgeServerError),
    #[error("sidecar readiness output failed")]
    Readiness,
    #[error("sidecar shutdown failed")]
    Shutdown,
}
