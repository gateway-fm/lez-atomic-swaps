//! Real-process acceptance contract for the isolated LEZ sidecar executable.

#![forbid(unsafe_code)]

use std::{
    fs::{self, OpenOptions},
    io::{BufRead as _, BufReader, Write as _},
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use lez_bridge_client::{BridgeClient, BridgeClientConfig, SidecarCapability};
use lez_bridge_protocol::{
    DescribeRuntimeRequest, Hex32, MessageContext, Participant, RequestId, RunId,
    RuntimeCompatibility, RuntimeDescriptor,
};
use nssa::{AccountId, PrivateKey, PublicKey};
use tempfile::TempDir;

const CAPABILITY: &str = "runner-capability-0000000000000001";
const WRONG_CAPABILITY: &str = "runner-capability-0000000000000002";

#[tokio::test]
async fn maker_and_taker_processes_bind_ephemeral_authenticated_sidecars() {
    let maker = RunnerFixture::new(Participant::Maker, 0x31, "runner-maker-0001");
    let taker = RunnerFixture::new(Participant::Taker, 0x32, "runner-taker-0001");
    let (mut maker_child, maker_readiness) = maker.spawn();
    let (mut taker_child, taker_readiness) = taker.spawn();
    assert_ne!(
        maker_readiness["endpoint"], taker_readiness["endpoint"],
        "concurrent actors must own distinct ephemeral listeners"
    );

    exercise_actor_sidecar(&maker, &mut maker_child, maker_readiness).await;
    exercise_actor_sidecar(&taker, &mut taker_child, taker_readiness).await;
}

async fn exercise_actor_sidecar(
    fixture: &RunnerFixture,
    child: &mut Child,
    readiness: serde_json::Value,
) {
    let role = fixture.runtime.sidecar_role;
    let run = &fixture.run;
    let endpoint = readiness
        .get("endpoint")
        .and_then(serde_json::Value::as_str)
        .expect("readiness endpoint");
    let endpoint_url = url::Url::parse(endpoint).expect("endpoint URL");
    assert_eq!(endpoint_url.scheme(), "http");
    assert!(
        endpoint_url
            .host_str()
            .expect("endpoint host")
            .parse::<std::net::IpAddr>()
            .expect("IP literal")
            .is_loopback()
    );
    assert_ne!(endpoint_url.port(), Some(0));
    assert_eq!(readiness["event"], "ready");
    assert_eq!(readiness["run_id"], run.as_str());
    assert!(
        !readiness.to_string().contains(CAPABILITY),
        "readiness must not expose the capability"
    );
    let reported: RuntimeDescriptor =
        serde_json::from_value(readiness["runtime"].clone()).expect("runtime");
    assert_eq!(reported, fixture.runtime);

    assert_rejected_identity(
        endpoint,
        WRONG_CAPABILITY,
        run,
        fixture.runtime.clone(),
        RequestId::new("wrong-capability").expect("request id"),
    )
    .await;
    assert_rejected_identity(
        endpoint,
        CAPABILITY,
        "runner-wrong-run",
        fixture.runtime.clone(),
        RequestId::new("wrong-run-id").expect("request id"),
    )
    .await;
    let mut wrong_role = fixture.runtime.clone();
    wrong_role.sidecar_role = match role {
        Participant::Maker => Participant::Taker,
        Participant::Taker => Participant::Maker,
    };
    assert_rejected_identity(
        endpoint,
        CAPABILITY,
        run,
        wrong_role,
        RequestId::new("wrong-role-id").expect("request id"),
    )
    .await;

    let run_id = RunId::new(run).expect("run id");
    let client = BridgeClient::connect(BridgeClientConfig::new(
        endpoint,
        SidecarCapability::new(CAPABILITY).expect("capability"),
        run_id.clone(),
        fixture.runtime.clone(),
        Duration::from_secs(2),
    ))
    .expect("client");
    let described = client
        .describe_runtime(DescribeRuntimeRequest::new(MessageContext::new(
            run_id,
            RequestId::new("describe-ready").expect("request id"),
            role,
        )))
        .await
        .expect("authenticated runtime description");
    assert_eq!(described.runtime, fixture.runtime);

    child
        .stdin
        .as_mut()
        .expect("child stdin")
        .write_all(b"shutdown\n")
        .expect("request graceful shutdown");
    assert!(wait_for_exit(child).success());
    assert!(fixture.state_file.is_file());
    assert_private_mode(&fixture.state_file);
}

#[cfg(unix)]
#[test]
fn executable_rejects_group_readable_secret_files() {
    use std::os::unix::fs::PermissionsExt as _;

    let fixture = RunnerFixture::new(Participant::Maker, 0x41, "runner-mode-0001");
    fs::set_permissions(&fixture.capability_file, fs::Permissions::from_mode(0o640))
        .expect("weaken capability permissions");
    let mut child = ChildGuard::new(fixture.command().spawn().expect("spawn sidecar"));
    assert!(!wait_for_exit(&mut child).success());
    assert!(!fixture.state_file.exists());
}

async fn assert_rejected_identity(
    endpoint: &str,
    capability: &str,
    run: &str,
    runtime: RuntimeDescriptor,
    request_id: RequestId,
) {
    let run_id = RunId::new(run).expect("run id");
    let role = runtime.sidecar_role;
    let client = BridgeClient::connect(BridgeClientConfig::new(
        endpoint,
        SidecarCapability::new(capability).expect("capability"),
        run_id.clone(),
        runtime,
        Duration::from_secs(2),
    ))
    .expect("bounded client");
    assert!(
        client
            .describe_runtime(DescribeRuntimeRequest::new(MessageContext::new(
                run_id, request_id, role,
            )))
            .await
            .is_err()
    );
}

struct RunnerFixture {
    _temporary: TempDir,
    runtime: RuntimeDescriptor,
    runtime_file: PathBuf,
    capability_file: PathBuf,
    signer_file: PathBuf,
    state_file: PathBuf,
    run: String,
}

impl RunnerFixture {
    fn new(role: Participant, key_byte: u8, run: &str) -> Self {
        let temporary = TempDir::new().expect("temporary runner");
        let key = PrivateKey::try_new([key_byte; 32]).expect("private key");
        let signer = AccountId::from(&PublicKey::new_from_private_key(&key));
        let runtime = RuntimeDescriptor::new(
            role,
            RuntimeCompatibility::NssaV0_1_2,
            Hex32::from_bytes([0x51; 32]),
            Hex32::from_bytes([0x52; 32]),
            Hex32::from_bytes([0x53; 32]),
            program_hex(&[0x54; 8]),
            Hex32::from_bytes(signer.into_value()),
        );
        let runtime_file = temporary.path().join("runtime.json");
        let capability_file = temporary.path().join("capability");
        let signer_file = temporary.path().join("signer-key");
        let state_file = temporary.path().join("state").join("idempotency.json");
        fs::create_dir(temporary.path().join("state")).expect("create private state directory");
        fs::write(
            &runtime_file,
            serde_json::to_vec(&runtime).expect("runtime JSON"),
        )
        .expect("write runtime");
        write_secret(&capability_file, CAPABILITY.as_bytes());
        write_secret(&signer_file, hex::encode([key_byte; 32]).as_bytes());
        Self {
            _temporary: temporary,
            runtime,
            runtime_file,
            capability_file,
            signer_file,
            state_file,
            run: run.to_owned(),
        }
    }

    fn command(&self) -> Command {
        let executable = env!("CARGO_BIN_EXE_lez-v0-1-2-sidecar");
        let mut command = Command::new(executable);
        command
            .arg("--node-endpoint")
            .arg("http://127.0.0.1:9")
            .arg("--run-id")
            .arg(&self.run)
            .arg("--runtime-file")
            .arg(&self.runtime_file)
            .arg("--capability-file")
            .arg(&self.capability_file)
            .arg("--signer-key-file")
            .arg(&self.signer_file)
            .arg("--state-file")
            .arg(&self.state_file)
            .arg("--shutdown-on-stdin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn spawn(&self) -> (ChildGuard, serde_json::Value) {
        let mut child = ChildGuard::new(self.command().spawn().expect("spawn sidecar"));
        let stdout = child.stdout.take().expect("child stdout");
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
            let _ = sender.send(result);
        });
        let line = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("sidecar readiness timeout")
            .expect("read readiness");
        let readiness = serde_json::from_str(&line).unwrap_or_else(|error| {
            let _ = child.kill();
            panic!("invalid readiness JSON: {error}: {line}");
        });
        (child, readiness)
    }
}

struct ChildGuard(Child);

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self(child)
    }
}

impl Deref for ChildGuard {
    type Target = Child;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn wait_for_exit(child: &mut Child) -> std::process::ExitStatus {
    for _ in 0..100 {
        if let Some(status) = child.try_wait().expect("poll child") {
            return status;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().expect("kill stuck child");
    child.wait().expect("reap killed child")
}

fn write_secret(path: &Path, bytes: &[u8]) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .expect("create secret");
        file.write_all(bytes).expect("write secret");
    }
    #[cfg(not(unix))]
    {
        fs::write(path, bytes).expect("write secret");
    }
}

fn assert_private_mode(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(path)
            .expect("state metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
    }
}

fn program_hex(words: &[u32; 8]) -> Hex32 {
    let mut bytes = [0_u8; 32];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(words) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    Hex32::from_bytes(bytes)
}
