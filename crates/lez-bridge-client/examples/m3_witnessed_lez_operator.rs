//! One-shot, fail-closed operator CLI for the M3 witnessed LEZ happy path.

#![forbid(unsafe_code)]

use std::{
    ffi::OsString,
    fmt,
    fs::{self, File},
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use lez_bridge_client::{BridgeClient, BridgeClientConfig, MAX_RPC_BODY_BYTES, SidecarCapability};
use lez_bridge_protocol::{
    CompleteWitnessedClaimRequest, DescribeRuntimeRequest, ObserveWitnessedEscrowRequest,
    Participant, PrepareWitnessedClaimRequest, PrepareWitnessedEscrowRequest, RunId,
    RuntimeDescriptor, SubmitTransactionRequest,
};
use serde::{Serialize, de::DeserializeOwned};
use zeroize::Zeroize as _;

const REQUEST_TIMEOUT: Duration = Duration::from_mins(1);
const MAX_RUNTIME_FILE_BYTES: usize = 16 * 1024;
const MAX_CAPABILITY_FILE_BYTES: usize = 130;
const MAX_CAPABILITY_FILE_BYTES_U64: u64 = 130;
const USAGE: &str = "usage: m3_witnessed_lez_operator <describe-runtime|prepare-witnessed-escrow|observe-witnessed-escrow|submit-transaction|prepare-witnessed-claim|complete-witnessed-claim> --endpoint <http://loopback:port/> --run-id <id> --sidecar-role <maker|taker> --capability-file <private-file> --runtime-file <json-file> --request-file <json-file>";

#[derive(Clone, Copy)]
enum Command {
    DescribeRuntime,
    PrepareWitnessedEscrow,
    ObserveWitnessedEscrow,
    SubmitTransaction,
    PrepareWitnessedClaim,
    CompleteWitnessedClaim,
}

impl Command {
    fn parse(value: &str) -> Result<Self, CliError> {
        match value {
            "describe-runtime" => Ok(Self::DescribeRuntime),
            "prepare-witnessed-escrow" => Ok(Self::PrepareWitnessedEscrow),
            "observe-witnessed-escrow" => Ok(Self::ObserveWitnessedEscrow),
            "submit-transaction" => Ok(Self::SubmitTransaction),
            "prepare-witnessed-claim" => Ok(Self::PrepareWitnessedClaim),
            "complete-witnessed-claim" => Ok(Self::CompleteWitnessedClaim),
            _ => Err(CliError::InvalidArguments),
        }
    }
}

struct Cli {
    command: Command,
    endpoint: String,
    run_id: RunId,
    sidecar_role: Participant,
    capability_file: PathBuf,
    runtime_file: PathBuf,
    request_file: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CliError {
    InvalidArguments,
    InvalidRunId,
    RuntimeInputUnavailable,
    RuntimeInputUnsafe,
    RuntimeInputInvalid,
    RequestInputUnavailable,
    RequestInputUnsafe,
    RequestInputInvalid,
    CapabilityFileUnavailable,
    CapabilityFileUnsafe,
    CapabilityInvalid,
    RuntimeRoleMismatch,
    ClientConfigurationInvalid,
    BridgeOperationFailed,
    OutputFailed,
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidArguments => "invalid or incomplete command arguments",
            Self::InvalidRunId => "run id is invalid",
            Self::RuntimeInputUnavailable => "runtime input is unavailable",
            Self::RuntimeInputUnsafe => "runtime input is not a bounded regular file",
            Self::RuntimeInputInvalid => "runtime input is not strict protocol JSON",
            Self::RequestInputUnavailable => "request input is unavailable",
            Self::RequestInputUnsafe => "request input is not a bounded regular file",
            Self::RequestInputInvalid => "request input is not strict protocol JSON",
            Self::CapabilityFileUnavailable => "sidecar capability file is unavailable",
            Self::CapabilityFileUnsafe => "sidecar capability file is unsafe",
            Self::CapabilityInvalid => "sidecar capability is invalid",
            Self::RuntimeRoleMismatch => "runtime sidecar role differs from explicit role",
            Self::ClientConfigurationInvalid => "bridge client configuration is invalid",
            Self::BridgeOperationFailed => "bridge operation failed",
            Self::OutputFailed => "result output failed",
        })
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(std::env::args_os()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("m3 witnessed LEZ operator: {error}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

async fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<(), CliError> {
    let cli = parse_arguments(arguments)?;
    let runtime: RuntimeDescriptor = read_public_json(
        &cli.runtime_file,
        MAX_RUNTIME_FILE_BYTES,
        CliError::RuntimeInputUnavailable,
        CliError::RuntimeInputUnsafe,
        CliError::RuntimeInputInvalid,
    )?;
    if runtime.sidecar_role != cli.sidecar_role {
        return Err(CliError::RuntimeRoleMismatch);
    }

    let capability = read_capability(&cli.capability_file)?;
    let client = BridgeClient::connect(BridgeClientConfig::new(
        cli.endpoint,
        capability,
        cli.run_id,
        runtime,
        REQUEST_TIMEOUT,
    ))
    .map_err(|_| CliError::ClientConfigurationInvalid)?;

    match cli.command {
        Command::DescribeRuntime => {
            let request = read_request::<DescribeRuntimeRequest>(&cli.request_file)?;
            let result = client
                .describe_runtime(request)
                .await
                .map_err(|_| CliError::BridgeOperationFailed)?;
            print_result(&result)
        }
        Command::PrepareWitnessedEscrow => {
            let request = read_request::<PrepareWitnessedEscrowRequest>(&cli.request_file)?;
            let result = client
                .prepare_witnessed_escrow(request)
                .await
                .map_err(|_| CliError::BridgeOperationFailed)?;
            print_result(&result)
        }
        Command::ObserveWitnessedEscrow => {
            let request = read_request::<ObserveWitnessedEscrowRequest>(&cli.request_file)?;
            let result = client
                .observe_witnessed_escrow(request)
                .await
                .map_err(|_| CliError::BridgeOperationFailed)?;
            print_result(&result)
        }
        Command::SubmitTransaction => {
            let request = read_request::<SubmitTransactionRequest>(&cli.request_file)?;
            let result = client
                .submit_transaction(request)
                .await
                .map_err(|_| CliError::BridgeOperationFailed)?;
            print_result(&result)
        }
        Command::PrepareWitnessedClaim => {
            let request = read_request::<PrepareWitnessedClaimRequest>(&cli.request_file)?;
            let result = client
                .prepare_witnessed_claim(request)
                .await
                .map_err(|_| CliError::BridgeOperationFailed)?;
            print_result(&result)
        }
        Command::CompleteWitnessedClaim => {
            let request = read_request::<CompleteWitnessedClaimRequest>(&cli.request_file)?;
            let result = client
                .complete_witnessed_claim(request)
                .await
                .map_err(|_| CliError::BridgeOperationFailed)?;
            print_result(&result)
        }
    }
}

fn parse_arguments(arguments: impl IntoIterator<Item = OsString>) -> Result<Cli, CliError> {
    let mut arguments = arguments.into_iter();
    let _program = arguments.next().ok_or(CliError::InvalidArguments)?;
    let command = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or(CliError::InvalidArguments)
        .and_then(|value| Command::parse(&value))?;

    let mut endpoint = None;
    let mut run_id = None;
    let mut sidecar_role = None;
    let mut capability_file = None;
    let mut runtime_file = None;
    let mut request_file = None;

    while let Some(flag) = arguments.next() {
        let flag = flag.into_string().map_err(|_| CliError::InvalidArguments)?;
        let value = arguments.next().ok_or(CliError::InvalidArguments)?;
        match flag.as_str() {
            "--endpoint" => set_once(&mut endpoint, value)?,
            "--run-id" => set_once(&mut run_id, value)?,
            "--sidecar-role" => set_once(&mut sidecar_role, value)?,
            "--capability-file" => set_once(&mut capability_file, value)?,
            "--runtime-file" => set_once(&mut runtime_file, value)?,
            "--request-file" => set_once(&mut request_file, value)?,
            _ => return Err(CliError::InvalidArguments),
        }
    }

    let endpoint = into_string(endpoint.ok_or(CliError::InvalidArguments)?)?;
    let run_id = RunId::new(into_string(run_id.ok_or(CliError::InvalidArguments)?)?)
        .map_err(|_| CliError::InvalidRunId)?;
    let sidecar_role = match into_string(sidecar_role.ok_or(CliError::InvalidArguments)?)?.as_str()
    {
        "maker" => Participant::Maker,
        "taker" => Participant::Taker,
        _ => return Err(CliError::InvalidArguments),
    };

    Ok(Cli {
        command,
        endpoint,
        run_id,
        sidecar_role,
        capability_file: PathBuf::from(capability_file.ok_or(CliError::InvalidArguments)?),
        runtime_file: PathBuf::from(runtime_file.ok_or(CliError::InvalidArguments)?),
        request_file: PathBuf::from(request_file.ok_or(CliError::InvalidArguments)?),
    })
}

fn set_once(slot: &mut Option<OsString>, value: OsString) -> Result<(), CliError> {
    if slot.replace(value).is_some() {
        return Err(CliError::InvalidArguments);
    }
    Ok(())
}

fn into_string(value: OsString) -> Result<String, CliError> {
    value.into_string().map_err(|_| CliError::InvalidArguments)
}

fn read_request<T: DeserializeOwned>(path: &Path) -> Result<T, CliError> {
    read_public_json(
        path,
        MAX_RPC_BODY_BYTES as usize,
        CliError::RequestInputUnavailable,
        CliError::RequestInputUnsafe,
        CliError::RequestInputInvalid,
    )
}

fn read_public_json<T: DeserializeOwned>(
    path: &Path,
    maximum_bytes: usize,
    unavailable: CliError,
    unsafe_file: CliError,
    invalid: CliError,
) -> Result<T, CliError> {
    let file = File::open(path).map_err(|_| unavailable)?;
    let metadata = file.metadata().map_err(|_| unavailable)?;
    if !metadata.file_type().is_file()
        || metadata.len() == 0
        || metadata.len() > maximum_bytes as u64
    {
        return Err(unsafe_file);
    }
    let input_length = usize::try_from(metadata.len()).map_err(|_| unsafe_file)?;
    let mut bytes = Vec::with_capacity(input_length);
    file.take(maximum_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable)?;
    if bytes.is_empty() || bytes.len() > maximum_bytes {
        return Err(unsafe_file);
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid)
}

fn read_capability(path: &Path) -> Result<SidecarCapability, CliError> {
    let before = fs::symlink_metadata(path).map_err(|_| CliError::CapabilityFileUnavailable)?;
    validate_capability_metadata(&before)?;

    let file = File::open(path).map_err(|_| CliError::CapabilityFileUnavailable)?;
    let opened = file
        .metadata()
        .map_err(|_| CliError::CapabilityFileUnavailable)?;
    validate_capability_metadata(&opened)?;
    if !same_capability_file(&before, &opened) {
        return Err(CliError::CapabilityFileUnsafe);
    }

    let mut bytes = Vec::with_capacity(MAX_CAPABILITY_FILE_BYTES + 1);
    file.take(MAX_CAPABILITY_FILE_BYTES_U64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| CliError::CapabilityFileUnavailable)?;

    let opened_after = fs::metadata(path).map_err(|_| CliError::CapabilityFileUnavailable)?;
    let path_after = fs::symlink_metadata(path).map_err(|_| CliError::CapabilityFileUnavailable)?;
    validate_capability_metadata(&opened_after)?;
    validate_capability_metadata(&path_after)?;
    if !stable_capability_file(&opened, &opened_after)
        || !stable_capability_file(&opened, &path_after)
    {
        bytes.zeroize();
        return Err(CliError::CapabilityFileUnsafe);
    }
    if bytes.is_empty() || bytes.len() > MAX_CAPABILITY_FILE_BYTES {
        bytes.zeroize();
        return Err(CliError::CapabilityFileUnsafe);
    }
    if bytes.ends_with(b"\r\n") {
        bytes.truncate(bytes.len() - 2);
    } else if bytes.ends_with(b"\n") {
        bytes.truncate(bytes.len() - 1);
    }
    let value = String::from_utf8(bytes).map_err(|error| {
        let mut bytes = error.into_bytes();
        bytes.zeroize();
        CliError::CapabilityInvalid
    })?;
    SidecarCapability::new(value).map_err(|_| CliError::CapabilityInvalid)
}

fn validate_capability_metadata(metadata: &fs::Metadata) -> Result<(), CliError> {
    if !metadata.file_type().is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_CAPABILITY_FILE_BYTES_U64
    {
        return Err(CliError::CapabilityFileUnsafe);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        if metadata.permissions().mode() & 0o7777 != 0o600 || metadata.nlink() != 1 {
            return Err(CliError::CapabilityFileUnsafe);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn same_capability_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_capability_file(_left: &fs::Metadata, _right: &fs::Metadata) -> bool {
    true
}

#[cfg(unix)]
fn stable_capability_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    same_capability_file(left, right)
        && left.len() == right.len()
        && left.mode() == right.mode()
        && left.nlink() == right.nlink()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(not(unix))]
fn stable_capability_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    same_capability_file(left, right) && left.len() == right.len()
}

fn print_result(result: &impl Serialize) -> Result<(), CliError> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, result).map_err(|_| CliError::OutputFailed)?;
    output.write_all(b"\n").map_err(|_| CliError::OutputFailed)
}

#[cfg(test)]
mod tests {
    use std::{
        fs::OpenOptions,
        io::Write as _,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    const CAPABILITY: &str = "operator-capability-000000000000001";
    const DESCRIBE_REQUEST: &str = r#"{
        "context": {
            "schema_version": 1,
            "run_id": "operator-test-run",
            "request_id": "describe-001",
            "sidecar_role": "maker"
        }
    }"#;

    #[test]
    fn arguments_require_every_explicit_identity_and_reject_extras() {
        let valid = arguments();
        let parsed = parse_arguments(valid.clone()).expect("valid arguments");
        assert_eq!(parsed.endpoint, "http://127.0.0.1:31415/");
        assert_eq!(parsed.run_id.as_str(), "operator-test-run");
        assert_eq!(parsed.sidecar_role, Participant::Maker);

        let mut trailing = valid.clone();
        trailing.push(OsString::from("unexpected"));
        assert_eq!(
            parse_arguments(trailing).err(),
            Some(CliError::InvalidArguments)
        );

        let mut duplicate = valid;
        duplicate.extend([OsString::from("--run-id"), OsString::from("another-run")]);
        assert_eq!(
            parse_arguments(duplicate).err(),
            Some(CliError::InvalidArguments)
        );
    }

    #[test]
    fn request_input_is_bounded_and_strict() {
        let directory = TestDirectory::new("request");
        let request = directory.path.join("request.json");
        fs::write(&request, DESCRIBE_REQUEST).expect("write valid request");
        let parsed: DescribeRuntimeRequest = read_request(&request).expect("strict request");
        assert_eq!(parsed.context.request_id.as_str(), "describe-001");

        fs::write(&request, format!("{DESCRIBE_REQUEST} trailing")).expect("write trailing input");
        assert_eq!(
            read_request::<DescribeRuntimeRequest>(&request).err(),
            Some(CliError::RequestInputInvalid)
        );

        fs::write(
            &request,
            DESCRIBE_REQUEST.replace("\"context\": {", "\"unknown\": 1, \"context\": {"),
        )
        .expect("write unknown field");
        assert_eq!(
            read_request::<DescribeRuntimeRequest>(&request).err(),
            Some(CliError::RequestInputInvalid)
        );
    }

    #[cfg(unix)]
    #[test]
    fn capability_is_private_single_link_and_errors_reveal_no_secret_or_path() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let directory = TestDirectory::new("private-marker");
        let capability = directory.path.join("secret-path-marker");
        write_private(&capability, format!("{CAPABILITY}\n").as_bytes());
        read_capability(&capability).expect("private single-link capability");

        let hard_link = directory.path.join("second-link");
        fs::hard_link(&capability, &hard_link).expect("create second link");
        assert_secret_free(
            read_capability(&capability).expect_err("multiple links must fail"),
            &capability,
        );
        fs::remove_file(&hard_link).expect("remove second link");

        fs::set_permissions(&capability, fs::Permissions::from_mode(0o640)).expect("weaken mode");
        assert_secret_free(
            read_capability(&capability).expect_err("public mode must fail"),
            &capability,
        );
        fs::set_permissions(&capability, fs::Permissions::from_mode(0o600)).expect("restore mode");

        let symlink_path = directory.path.join("secret-link-marker");
        symlink(&capability, &symlink_path).expect("create symlink");
        assert_secret_free(
            read_capability(&symlink_path).expect_err("symlink must fail"),
            &symlink_path,
        );
    }

    fn arguments() -> Vec<OsString> {
        [
            "m3_witnessed_lez_operator",
            "describe-runtime",
            "--endpoint",
            "http://127.0.0.1:31415/",
            "--run-id",
            "operator-test-run",
            "--sidecar-role",
            "maker",
            "--capability-file",
            "/private/capability",
            "--runtime-file",
            "/public/runtime.json",
            "--request-file",
            "/public/request.json",
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    }

    fn assert_secret_free(error: CliError, path: &Path) {
        for rendered in [error.to_string(), format!("{error:?}")] {
            assert!(!rendered.contains(CAPABILITY));
            assert!(!rendered.contains("secret-path-marker"));
            assert!(!rendered.contains(&path.display().to_string()));
        }
    }

    #[cfg(unix)]
    fn write_private(path: &Path, bytes: &[u8]) {
        use std::os::unix::fs::OpenOptionsExt as _;

        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .expect("create private file");
        file.write_all(bytes).expect("write private file");
    }

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "m3-witnessed-lez-operator-{label}-{}-{}",
                std::process::id(),
                TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create isolated test directory");
            Self { path }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
