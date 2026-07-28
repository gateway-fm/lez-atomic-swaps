use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Write as _},
    os::unix::fs::{FileTypeExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, bail, ensure};
use clap::Parser;
use jsonrpsee::server::{
    BatchRequestConfig, ServerBuilder, ServerConfig, serve_with_graceful_shutdown, stop_channel,
};
use lez_maker_node::{
    MakerActorSupervisorCancellation, MakerActorSupervisorConfig, MakerRpc,
    ProcessLogosPriceSource, RunLocalDelivery, ZecMakerActorProvisioner, chat_rpc_module,
    import_terminal_zec_maker_projection, rpc_module, supervise_one_abandoned_maker_actor,
    supervise_one_abandoned_maker_actor_until, supervise_one_due_maker_actor_until,
};
use lez_swap_core::{Participant, SwapId};
use lez_swap_store::{MakerActorLeaseOwner, SqliteSwapStore, SqliteZecRecoveryStore};
use lez_zec_swap_sdk::{ClaimPreimage, ProtectedClaimKey};
use rustix::fs::{CWD, FlockOperation, Mode, OFlags, ResolveFlags, flock, openat2};
use sd_notify::NotifyState;
use secp256k1::SecretKey;
use tokio::{net::UnixListener, task::JoinSet};
use zeroize::Zeroizing;

#[path = "support/secure_file.rs"]
mod secure_file;
use secure_file::{load_raw_secret, read_private_file};

const MAXIMUM_RPC_BODY_BYTES: u32 = 64 * 1024;
const MAXIMUM_CONNECTIONS: u32 = 16;

#[derive(Parser)]
#[command(about = "Headless LEZ atomic-swap maker daemon")]
struct Arguments {
    /// Owner-only Unix-domain control socket.
    #[arg(long, default_value = "/run/lez-atomic-swaps/maker.sock")]
    socket: PathBuf,
    #[arg(long)]
    database: PathBuf,
    /// Optional no-clobber readiness handoff containing only the socket path.
    #[arg(long)]
    ready_file: Option<PathBuf>,
    /// Owner-private run-local Delivery directory; requires the signing key file.
    #[arg(long, requires_all = ["delivery_signing_key_file", "chat_socket"])]
    delivery_directory: Option<PathBuf>,
    /// Owner-only file containing one raw 32-byte secp256k1 key or its 64-byte
    /// lowercase hexadecimal encoding.
    #[arg(long, requires_all = ["delivery_directory", "chat_socket"])]
    delivery_signing_key_file: Option<PathBuf>,
    /// Taker-facing run-local Chat socket, isolated from owner-control methods.
    #[arg(
        long,
        requires_all = [
            "delivery_directory",
            "delivery_signing_key_file",
            "maker_claim_key_id",
            "maker_claim_key_file",
            "maker_claim_preimage_file",
            "zec_source_maker_config",
            "zec_maker_actor_root",
            "zec_actor_program",
            "zec_actor_program_sha256"
        ]
    )]
    chat_socket: Option<PathBuf>,
    /// Non-secret rotation identifier for the maker claim-recovery key.
    #[arg(long, requires_all = ["delivery_directory", "maker_claim_key_file", "maker_claim_preimage_file"])]
    maker_claim_key_id: Option<Box<str>>,
    /// Owner-only file containing one raw 32-byte claim-recovery key.
    #[arg(long, requires_all = ["delivery_directory", "maker_claim_key_id", "maker_claim_preimage_file"])]
    maker_claim_key_file: Option<PathBuf>,
    /// Owner-only file containing the maker-owned 32-byte claim preimage.
    #[arg(long, requires_all = ["delivery_directory", "maker_claim_key_id", "maker_claim_key_file"])]
    maker_claim_preimage_file: Option<PathBuf>,
    /// Existing owner-private per-swap Maker configs used as authority templates.
    #[arg(long, action = clap::ArgAction::Append, requires_all = ["delivery_directory", "zec_maker_actor_root", "zec_actor_program", "zec_actor_program_sha256"])]
    zec_source_maker_config: Vec<PathBuf>,
    /// Existing owner-private mode-0700 base for deterministic per-swap actor bundles.
    #[arg(long, requires_all = ["zec_source_maker_config", "zec_actor_program", "zec_actor_program_sha256"])]
    zec_maker_actor_root: Option<PathBuf>,
    /// Absolute exact ZEC one-shot actor executable.
    #[arg(long, requires_all = ["zec_source_maker_config", "zec_maker_actor_root", "zec_actor_program_sha256"])]
    zec_actor_program: Option<PathBuf>,
    /// Exact 32-byte SHA-256 identity of the ZEC actor executable.
    #[arg(long, requires_all = ["zec_source_maker_config", "zec_maker_actor_root", "zec_actor_program"])]
    zec_actor_program_sha256: Option<Box<str>>,
    /// Stopped owner-private Maker actor database imported only as a terminal operator view.
    #[arg(long, requires_all = ["terminal_zec_swap_id", "terminal_zec_claim_key_id", "terminal_zec_claim_key_file"])]
    terminal_zec_maker_state_db: Option<PathBuf>,
    /// Exact swap ID expected in both the stopped actor and completed Maker Chat history.
    #[arg(long, requires_all = ["terminal_zec_maker_state_db", "terminal_zec_claim_key_id", "terminal_zec_claim_key_file"])]
    terminal_zec_swap_id: Option<Box<str>>,
    /// Rotation ID for the stopped Maker actor's claim-recovery key.
    #[arg(long, requires_all = ["terminal_zec_maker_state_db", "terminal_zec_swap_id", "terminal_zec_claim_key_file"])]
    terminal_zec_claim_key_id: Option<Box<str>>,
    /// Owner-only raw 32-byte claim key for offline terminal history replay.
    #[arg(long, requires_all = ["terminal_zec_maker_state_db", "terminal_zec_swap_id", "terminal_zec_claim_key_id"])]
    terminal_zec_claim_key_file: Option<PathBuf>,
    /// Absolute crash-isolated worker executable for one pinned Logos price module.
    #[arg(long, requires_all = ["logos_price_module", "logos_price_module_sha256"])]
    logos_price_worker: Option<PathBuf>,
    /// Absolute path to the pinned Logos price module artifact.
    #[arg(long, requires_all = ["logos_price_worker", "logos_price_module_sha256"])]
    logos_price_module: Option<PathBuf>,
    /// Exact lowercase or uppercase 32-byte SHA-256 module identity.
    #[arg(long, requires_all = ["logos_price_worker", "logos_price_module"])]
    logos_price_module_sha256: Option<Box<str>>,
    /// Per-invocation worker deadline; defaults to 1000 milliseconds when configured.
    #[arg(long, requires = "logos_price_worker")]
    logos_price_timeout_milliseconds: Option<u64>,
    /// Maximum external observation age; defaults to 30 seconds when configured.
    #[arg(long, requires = "logos_price_worker")]
    logos_price_max_age_seconds: Option<u64>,
    /// Runs the persistent pair-neutral actor coordinator on a dedicated store connection.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    actor_supervisor: bool,
    /// Finite deadline for one actor status/effect cycle (1..=300000 milliseconds).
    #[arg(long, requires = "actor_supervisor", value_parser = clap::value_parser!(u64).range(1..=300_000))]
    actor_attempt_timeout_milliseconds: Option<u64>,
    /// Idle scheduling poll interval (1..=1000 milliseconds).
    #[arg(long, requires = "actor_supervisor", value_parser = clap::value_parser!(u64).range(1..=1_000))]
    actor_poll_milliseconds: Option<u64>,
    /// Delay before observing a still-live actor again.
    #[arg(long, requires = "actor_supervisor", value_parser = clap::value_parser!(u64).range(1..))]
    actor_requeue_delay_seconds: Option<u64>,
    /// Delay after a transient actor/dependency failure.
    #[arg(long, requires = "actor_supervisor", value_parser = clap::value_parser!(u64).range(1..))]
    actor_failure_backoff_seconds: Option<u64>,
    /// Maximum stdout bytes accepted from one actor command (256..=65536).
    #[arg(
        long,
        requires = "actor_supervisor",
        value_parser = clap::value_parser!(u64).range(256..=65_536)
    )]
    actor_max_output_bytes: Option<u64>,
    /// Exact swap armed for the compile-time-gated submitted-effect pause.
    #[cfg(feature = "test-crash-hooks")]
    #[arg(long, hide = true, requires_all = ["actor_supervisor", "actor_test_pause_operation", "actor_test_pause_marker"])]
    actor_test_pause_swap_id: Option<Box<str>>,
    /// Allowlisted submitted operation armed only in fault-test builds.
    #[cfg(feature = "test-crash-hooks")]
    #[arg(long, hide = true, requires_all = ["actor_supervisor", "actor_test_pause_swap_id", "actor_test_pause_marker"])]
    actor_test_pause_operation: Option<Box<str>>,
    /// Private no-clobber marker beneath an owner-only canonical directory.
    #[cfg(feature = "test-crash-hooks")]
    #[arg(long, hide = true, requires_all = ["actor_supervisor", "actor_test_pause_swap_id", "actor_test_pause_operation"])]
    actor_test_pause_marker: Option<PathBuf>,
}

struct ActorSupervisorRuntime {
    store: SqliteSwapStore,
    owner: MakerActorLeaseOwner,
    config: MakerActorSupervisorConfig,
    poll_interval: Duration,
}

struct ActorSupervisorCancellationGuard(MakerActorSupervisorCancellation);

impl Drop for ActorSupervisorCancellationGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

fn configured_actor_supervisor(
    arguments: &Arguments,
) -> anyhow::Result<Option<ActorSupervisorRuntime>> {
    let tuning = (
        arguments.actor_attempt_timeout_milliseconds,
        arguments.actor_poll_milliseconds,
        arguments.actor_requeue_delay_seconds,
        arguments.actor_failure_backoff_seconds,
        arguments.actor_max_output_bytes,
    );
    if !arguments.actor_supervisor {
        ensure!(
            tuning == (None, None, None, None, None),
            "actor supervisor tuning requires explicit opt-in"
        );
        return Ok(None);
    }
    let max_output_bytes = usize::try_from(arguments.actor_max_output_bytes.unwrap_or(8_192))
        .context("convert validated actor output bound")?;
    let config = MakerActorSupervisorConfig::new(
        Duration::from_millis(
            arguments
                .actor_attempt_timeout_milliseconds
                .unwrap_or(30_000),
        ),
        arguments.actor_requeue_delay_seconds.unwrap_or(5),
        arguments.actor_failure_backoff_seconds.unwrap_or(30),
        max_output_bytes,
    )
    .context("validate actor supervisor bounds")?;
    #[cfg(feature = "test-crash-hooks")]
    let config = {
        let hook = (
            arguments.actor_test_pause_swap_id.as_deref(),
            arguments.actor_test_pause_operation.as_deref(),
            arguments.actor_test_pause_marker.as_ref(),
        );
        match hook {
            (None, None, None) => config,
            (Some(swap_id), Some(operation), Some(marker)) => {
                ensure!(
                    marker.is_absolute(),
                    "actor test pause marker must be one absolute path"
                );
                let parent = marker
                    .parent()
                    .context("actor test pause marker needs a parent")?;
                validate_runtime_directory(parent)
                    .context("validate actor test pause marker parent")?;
                ensure!(
                    fs::canonicalize(parent)
                        .context("canonicalize actor test pause marker parent")?
                        == parent,
                    "actor test pause marker parent must be canonical"
                );
                match fs::symlink_metadata(marker) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error).context("inspect actor test pause marker"),
                    Ok(metadata) => ensure!(
                        metadata.file_type().is_file()
                            && metadata.uid() == rustix::process::geteuid().as_raw()
                            && metadata.mode() & 0o7777 == 0o600
                            && metadata.nlink() == 1,
                        "existing actor test pause marker must be an owner-only single-link file"
                    ),
                }
                config.with_test_pause_after_submitted(
                    SwapId::new(swap_id.to_owned()).context("validate actor test pause swap ID")?,
                    operation,
                    marker.clone(),
                )?
            }
            _ => bail!("actor test pause arguments must be configured together"),
        }
    };
    let owner = MakerActorLeaseOwner::random().context("generate actor supervisor lease owner")?;
    let mut store =
        SqliteSwapStore::open(&arguments.database).context("open actor supervisor database")?;

    loop {
        let now = trusted_now_unix_seconds()?;
        if supervise_one_abandoned_maker_actor(&mut store, owner, now, &config)
            .context("recover abandoned maker actor before readiness")?
            .is_none()
        {
            break;
        }
    }

    Ok(Some(ActorSupervisorRuntime {
        store,
        owner,
        config,
        poll_interval: Duration::from_millis(arguments.actor_poll_milliseconds.unwrap_or(50)),
    }))
}

fn run_actor_supervisor(
    mut runtime: ActorSupervisorRuntime,
    cancellation: &MakerActorSupervisorCancellation,
) -> anyhow::Result<()> {
    while !cancellation.is_cancelled() {
        let now = trusted_now_unix_seconds()?;
        if supervise_one_abandoned_maker_actor_until(
            &mut runtime.store,
            runtime.owner,
            now,
            &runtime.config,
            cancellation,
        )
        .context("recover abandoned maker actor")?
        .is_some()
        {
            continue;
        }
        if supervise_one_due_maker_actor_until(
            &mut runtime.store,
            runtime.owner,
            now,
            &runtime.config,
            cancellation,
        )
        .context("supervise due maker actor")?
        .is_some()
        {
            continue;
        }
        thread::sleep(runtime.poll_interval);
    }
    Ok(())
}

#[tokio::main]
#[allow(clippy::too_many_lines)] // Keep startup, select, cancellation, and teardown order auditable.
async fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    let logos_price_source = configured_logos_price_source(&arguments)?;
    let zec_actor_provisioner = configured_zec_actor_provisioner(&arguments)?;
    let _state_lease = acquire_state_lease(&arguments.database)?;
    import_terminal_projection(&arguments).await?;
    let actor_supervisor = configured_actor_supervisor(&arguments)?;
    let (listener, _socket_guard) = bind_owner_socket(&arguments.socket)?;
    let (chat_listener, _chat_socket_guard) =
        bind_optional_owner_socket(arguments.chat_socket.as_deref())?;
    let context = maker_context(&arguments, zec_actor_provisioner, logos_price_source)?;
    let context = attach_chat_health(context, arguments.chat_socket.as_deref());
    let module = rpc_module(context.clone())?;
    let chat_module = if chat_listener.is_some() {
        Some(chat_rpc_module(context)?)
    } else {
        None
    };
    let _ready_guard = arguments
        .ready_file
        .as_deref()
        .map(|path| create_ready_file(path, &arguments.socket))
        .transpose()?;
    let (stop_handle, server_handle) = stop_channel();
    let service = ServerBuilder::default()
        .set_config(server_config())
        .to_service_builder()
        .build(module, stop_handle.clone());
    let chat_service = chat_module.map(|module| {
        ServerBuilder::default()
            .set_config(server_config())
            .to_service_builder()
            .build(module, stop_handle.clone())
    });
    let mut connections = JoinSet::new();
    let supervisor_cancellation = MakerActorSupervisorCancellation::new();
    let _supervisor_cancellation_guard =
        ActorSupervisorCancellationGuard(supervisor_cancellation.clone());
    let mut supervisor_tasks = JoinSet::new();
    if let Some(runtime) = actor_supervisor {
        let cancellation = supervisor_cancellation.clone();
        supervisor_tasks.spawn_blocking(move || run_actor_supervisor(runtime, &cancellation));
    }
    let mut daemon_error = None;
    notify_ready()?;
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("accept local RPC connection")?;
                let connection_service = service.clone();
                let connection_stop = stop_handle.clone();
                connections.spawn(async move {
                    serve_with_graceful_shutdown(
                        stream,
                        connection_service,
                        connection_stop.shutdown(),
                    )
                    .await
                });
            }
            accepted = async {
                chat_listener
                    .as_ref()
                    .expect("Chat listener is present when branch is enabled")
                    .accept()
                    .await
            }, if chat_listener.is_some() => {
                let (stream, _) = accepted.context("accept local Chat connection")?;
                let connection_service = chat_service
                    .as_ref()
                    .expect("Chat service is present with Chat listener")
                    .clone();
                let connection_stop = stop_handle.clone();
                connections.spawn(async move {
                    serve_with_graceful_shutdown(
                        stream,
                        connection_service,
                        connection_stop.shutdown(),
                    )
                    .await
                });
            }
            result = supervisor_tasks.join_next(), if !supervisor_tasks.is_empty() => {
                let result = result.expect("enabled supervisor task is present");
                daemon_error = Some(match result {
                    Ok(Ok(())) => anyhow::anyhow!("actor supervisor exited unexpectedly"),
                    Ok(Err(error)) => error.context("actor supervisor failed"),
                    Err(error) => anyhow::Error::new(error).context("join actor supervisor"),
                });
                notify_stopping();
                supervisor_cancellation.cancel();
                break;
            }
            signal = &mut shutdown => {
                if let Err(error) = signal {
                    daemon_error = Some(anyhow::Error::new(error).context("wait for shutdown"));
                }
                notify_stopping();
                supervisor_cancellation.cancel();
                break;
            }
        }
    }

    supervisor_cancellation.cancel();
    server_handle.stop().context("stop maker RPC")?;
    while let Some(result) = supervisor_tasks.join_next().await {
        result
            .context("join actor supervisor during shutdown")?
            .context("stop actor supervisor")?;
    }
    while let Some(connection) = connections.join_next().await {
        connection
            .context("join local RPC connection")?
            .map_err(|error| anyhow::anyhow!("serve local RPC connection: {error}"))?;
    }
    drop(service);
    drop(chat_service);
    drop(stop_handle);
    server_handle.stopped().await;
    match daemon_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn shutdown_signal() -> io::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        received = terminate.recv() => received.ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "SIGTERM signal stream closed")
        }),
    }
}

fn attach_chat_health(context: MakerRpc, socket: Option<&Path>) -> MakerRpc {
    match socket {
        Some(socket) => context.with_chat_socket(socket.to_path_buf()),
        None => context,
    }
}

fn notify_ready() -> anyhow::Result<()> {
    sd_notify::notify(&[
        NotifyState::Ready,
        NotifyState::Status("maker RPC and durable state are ready"),
    ])
    .context("notify service manager of maker readiness")
}

fn notify_stopping() {
    let _ = sd_notify::notify(&[
        NotifyState::Stopping,
        NotifyState::Status("maker daemon is stopping"),
    ]);
}

fn bind_optional_owner_socket(
    path: Option<&Path>,
) -> anyhow::Result<(Option<UnixListener>, Option<OwnedPath>)> {
    Ok(path
        .map(bind_owner_socket)
        .transpose()?
        .map_or((None, None), |(listener, guard)| {
            (Some(listener), Some(guard))
        }))
}

#[derive(Debug)]
struct StateLease {
    _file: File,
}

fn acquire_state_lease(database: &Path) -> anyhow::Result<StateLease> {
    ensure!(
        database.is_absolute(),
        "maker database path must be absolute"
    );
    let mut lease_name = OsString::from(database.as_os_str());
    lease_name.push(".lock");
    let lease_path = PathBuf::from(lease_name);
    let file = openat2(
        CWD,
        &lease_path,
        OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
        ResolveFlags::NO_SYMLINKS,
    )
    .map(File::from)
    .context("open maker database lease")?;
    let metadata = file.metadata().context("inspect maker database lease")?;
    ensure!(
        metadata.file_type().is_file()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.mode() & 0o7777 == 0o600
            && metadata.nlink() == 1,
        "maker database lease must be an owner-owned, single-link mode-0600 regular file"
    );
    flock(&file, FlockOperation::NonBlockingLockExclusive)
        .context("acquire exclusive maker database lease")?;
    Ok(StateLease { _file: file })
}

async fn import_terminal_projection(arguments: &Arguments) -> anyhow::Result<()> {
    let configured = (
        arguments.terminal_zec_maker_state_db.as_deref(),
        arguments.terminal_zec_swap_id.as_deref(),
        arguments.terminal_zec_claim_key_id.as_deref(),
        arguments.terminal_zec_claim_key_file.as_deref(),
    );
    let (Some(actor_database), Some(swap_id), Some(key_id), Some(key_file)) = configured else {
        ensure!(
            configured == (None, None, None, None),
            "terminal ZEC projection arguments must be configured together"
        );
        return Ok(());
    };
    let swap_id = SwapId::new(swap_id.to_owned()).context("validate terminal ZEC swap ID")?;
    let key_material = load_raw_secret(key_file, "terminal Maker claim-recovery key")?;
    let claim_key = ProtectedClaimKey::new(key_id, *key_material)
        .context("validate terminal Maker claim-recovery key ID")?;
    let commit = import_terminal_zec_maker_projection(
        &arguments.database,
        actor_database,
        &swap_id,
        claim_key,
    )
    .await
    .context("import terminal Maker actor projection")?;
    ensure!(
        commit.source_revision() > 0,
        "terminal Maker actor projection has no durable source revision"
    );
    Ok(())
}

fn configured_logos_price_source(
    arguments: &Arguments,
) -> anyhow::Result<Option<ProcessLogosPriceSource>> {
    let configured = (
        arguments.logos_price_worker.as_ref(),
        arguments.logos_price_module.as_ref(),
        arguments.logos_price_module_sha256.as_deref(),
    );
    let (Some(worker), Some(module), Some(module_sha256)) = configured else {
        ensure!(
            configured == (None, None, None)
                && arguments.logos_price_timeout_milliseconds.is_none()
                && arguments.logos_price_max_age_seconds.is_none(),
            "Logos price worker, module, and module SHA-256 must be configured together"
        );
        return Ok(None);
    };
    ensure!(
        module_sha256.len() == 64,
        "Logos price module SHA-256 must contain exactly 32 bytes as hexadecimal"
    );
    let mut identity = [0_u8; 32];
    hex::decode_to_slice(module_sha256, &mut identity)
        .context("decode Logos price module SHA-256")?;
    let timeout =
        Duration::from_millis(arguments.logos_price_timeout_milliseconds.unwrap_or(1_000));
    let max_age_seconds = arguments.logos_price_max_age_seconds.unwrap_or(30);
    ProcessLogosPriceSource::new(
        worker.clone(),
        module.clone(),
        identity,
        timeout,
        max_age_seconds,
    )
    .context("validate Logos price source")
    .map(Some)
}

fn configured_zec_actor_provisioner(
    arguments: &Arguments,
) -> anyhow::Result<Option<ZecMakerActorProvisioner>> {
    let deployment = (
        arguments.zec_maker_actor_root.as_ref(),
        arguments.zec_actor_program.as_ref(),
        arguments.zec_actor_program_sha256.as_deref(),
    );
    if arguments.zec_source_maker_config.is_empty() {
        ensure!(
            deployment == (None, None, None),
            "ZEC actor templates, root, program, and SHA-256 must be configured together"
        );
        return Ok(None);
    }
    let (Some(root), Some(program), Some(program_sha256)) = deployment else {
        bail!("ZEC actor templates, root, program, and SHA-256 must be configured together");
    };
    validate_runtime_directory(root).context("validate ZEC maker actor root")?;
    ensure!(
        root.is_absolute()
            && fs::canonicalize(root).context("canonicalize ZEC maker actor root")? == *root,
        "ZEC maker actor root must be absolute and canonical"
    );
    ensure!(
        program_sha256.len() == 64,
        "ZEC actor program SHA-256 must contain exactly 32 bytes as hexadecimal"
    );
    let mut identity = [0_u8; 32];
    hex::decode_to_slice(program_sha256, &mut identity)
        .context("decode ZEC actor program SHA-256")?;
    ZecMakerActorProvisioner::new(
        &arguments.zec_source_maker_config,
        root.clone(),
        program.clone(),
        identity,
    )
    .context("validate ZEC maker actor deployment")
    .map(Some)
}

fn maker_context(
    arguments: &Arguments,
    zec_actor_provisioner: Option<ZecMakerActorProvisioner>,
    logos_price_source: Option<ProcessLogosPriceSource>,
) -> anyhow::Result<MakerRpc> {
    let store = SqliteSwapStore::open(&arguments.database).context("open maker database")?;
    let configured = (
        arguments.delivery_directory.as_deref(),
        arguments.delivery_signing_key_file.as_deref(),
        arguments.maker_claim_key_id.as_deref(),
        arguments.maker_claim_key_file.as_deref(),
        arguments.maker_claim_preimage_file.as_deref(),
    );
    let (
        Some(directory),
        Some(signing_file),
        Some(claim_key_id),
        Some(claim_key_file),
        Some(preimage_file),
    ) = configured
    else {
        ensure!(
            configured == (None, None, None, None, None),
            "Delivery, Chat, claim-recovery, and preimage authority must be configured together"
        );
        let context = MakerRpc::new(store);
        return Ok(match logos_price_source {
            Some(source) => context.with_logos_price_source(source),
            None => context,
        });
    };
    let signing_key = load_delivery_key(signing_file)?;
    let claim_key_material = load_raw_secret(claim_key_file, "maker claim-recovery key")?;
    let claim_key = ProtectedClaimKey::new(claim_key_id, *claim_key_material)
        .context("validate maker claim-recovery key ID")?;
    let preimage_material = load_raw_secret(preimage_file, "maker claim preimage")?;
    let preimage = ClaimPreimage::new(*preimage_material);
    let recovery_store = SqliteZecRecoveryStore::open_claim_capable(
        &arguments.database,
        Participant::Maker,
        claim_key,
    )
    .context("open maker ZEC recovery store")?;
    let delivery = RunLocalDelivery::publisher(directory.to_path_buf(), signing_key)
        .context("open maker Delivery publisher")?;
    let now_unix_seconds = trusted_now_unix_seconds()?;
    let active = store
        .list_retryable_maker_offers(now_unix_seconds)
        .context("load retryable offers for Delivery reconciliation")?
        .into_iter()
        .map(|record| record.offer().clone())
        .collect::<Vec<_>>();
    delivery
        .reconcile(&active, now_unix_seconds)
        .context("reconcile Delivery advertisements")?;
    let context = MakerRpc::with_delivery(
        store,
        delivery,
        signing_key,
        recovery_store,
        preimage,
        zec_actor_provisioner,
    );
    Ok(match logos_price_source {
        Some(source) => context.with_logos_price_source(source),
        None => context,
    })
}

fn server_config() -> ServerConfig {
    ServerConfig::builder()
        .max_request_body_size(MAXIMUM_RPC_BODY_BYTES)
        .max_response_body_size(MAXIMUM_RPC_BODY_BYTES)
        .max_connections(MAXIMUM_CONNECTIONS)
        .set_batch_request_config(BatchRequestConfig::Disabled)
        .http_only()
        .build()
}

fn load_delivery_key(path: &Path) -> anyhow::Result<SecretKey> {
    let encoded = read_private_file(path, 65, "Delivery signing key")?;
    if encoded.len() == 32 {
        return SecretKey::from_slice(encoded.as_slice()).context("validate Delivery signing key");
    }
    let text = std::str::from_utf8(&encoded)
        .context("Delivery signing key must be raw bytes or UTF-8 hex")?
        .trim();
    ensure!(
        text.len() == 64,
        "Delivery signing key must contain exactly 32 raw bytes or 32 bytes as hex"
    );
    let mut bytes = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(text, bytes.as_mut()).context("decode Delivery signing key")?;
    SecretKey::from_slice(bytes.as_ref()).context("validate Delivery signing key")
}

fn trusted_now_unix_seconds() -> anyhow::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")
        .map(|duration| duration.as_secs())
}

fn bind_owner_socket(path: &Path) -> anyhow::Result<(UnixListener, OwnedPath)> {
    ensure!(path.is_absolute(), "maker RPC socket path must be absolute");
    let runtime = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("maker RPC socket needs a runtime directory")?;
    validate_runtime_directory(runtime)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect maker RPC socket path"),
        Ok(_) => bail!("refusing to replace existing maker RPC socket path"),
    }

    let listener = UnixListener::bind(path).context("bind maker RPC Unix socket")?;
    let guard = OwnedPath::capture(path).context("capture maker RPC socket identity")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .context("set maker RPC socket mode")?;
    let metadata = fs::symlink_metadata(path).context("verify maker RPC socket")?;
    ensure!(
        metadata.file_type().is_socket()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.mode() & 0o7777 == 0o600,
        "maker RPC socket is not an owner-only socket"
    );
    Ok((listener, guard))
}

fn validate_runtime_directory(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path).context("inspect maker RPC runtime directory")?;
    ensure!(
        metadata.file_type().is_dir(),
        "maker RPC runtime path must be a real directory"
    );
    ensure!(
        metadata.uid() == rustix::process::geteuid().as_raw(),
        "maker RPC runtime directory must be owned by the daemon user"
    );
    ensure!(
        metadata.mode() & 0o7777 == 0o700,
        "maker RPC runtime directory must have mode 0700"
    );
    Ok(())
}

fn create_ready_file(path: &Path, socket: &Path) -> anyhow::Result<OwnedPath> {
    ensure!(path.is_absolute(), "maker readiness path must be absolute");
    ensure!(
        path.parent() == socket.parent(),
        "maker readiness file must share the socket runtime directory"
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context("create maker readiness file")?;
    let guard = OwnedPath::capture(path).context("capture maker readiness file identity")?;
    writeln!(file, "{}", socket.display()).context("write maker readiness file")?;
    file.sync_all().context("sync maker readiness file")?;
    Ok(guard)
}

#[derive(Debug)]
struct OwnedPath {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl OwnedPath {
    fn capture(path: &Path) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

impl Drop for OwnedPath {
    fn drop(&mut self) {
        let Ok(metadata) = fs::symlink_metadata(&self.path) else {
            return;
        };
        if metadata.dev() == self.device && metadata.ino() == self.inode {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Arguments;
    use clap::{Parser as _, error::ErrorKind};

    fn daemon_arguments() -> Vec<&'static str> {
        vec!["lez-maker-daemon", "--database", "/tmp/maker.sqlite3"]
    }

    fn assert_cli_error(arguments: Vec<&'static str>, expected: ErrorKind) {
        let Err(error) = Arguments::try_parse_from(arguments) else {
            panic!("CLI must reject arguments");
        };
        assert_eq!(error.kind(), expected, "{error}");
    }

    fn chat_arguments() -> Vec<&'static str> {
        vec![
            "lez-maker-daemon",
            "--database",
            "/tmp/maker.sqlite3",
            "--delivery-directory",
            "/tmp/delivery",
            "--delivery-signing-key-file",
            "/tmp/delivery.key",
            "--chat-socket",
            "/tmp/chat.sock",
            "--maker-claim-key-id",
            "maker-claim-v1",
            "--maker-claim-key-file",
            "/tmp/claim.key",
            "--maker-claim-preimage-file",
            "/tmp/preimage.key",
        ]
    }

    #[test]
    fn chat_cli_requires_complete_zec_actor_deployment() {
        assert!(
            Arguments::try_parse_from(chat_arguments()).is_err(),
            "serving Chat without actor authority must fail during CLI parsing"
        );

        let mut complete = chat_arguments();
        complete.extend([
            "--zec-source-maker-config",
            "/tmp/actor.json",
            "--zec-maker-actor-root",
            "/tmp/actors",
            "--zec-actor-program",
            "/usr/bin/true",
            "--zec-actor-program-sha256",
            "00f5ba0b54bda7a48c9380b740fc38a8710a23c0f3f948cdcc9b7d5e712d1f8f",
        ]);
        Arguments::try_parse_from(complete.clone()).expect("complete Chat deployment parses");
        complete.extend(["--zec-source-maker-config", "/tmp/second-actor.json"]);
        Arguments::try_parse_from(complete).expect("per-swap source registry parses");
    }

    #[test]
    fn actor_supervisor_is_opt_in_and_accepts_complete_valid_tuning() {
        Arguments::try_parse_from(daemon_arguments())
            .expect("the actor supervisor remains disabled unless explicitly requested");

        let mut enabled = daemon_arguments();
        enabled.extend([
            "--actor-supervisor",
            "--actor-attempt-timeout-milliseconds",
            "30000",
            "--actor-poll-milliseconds",
            "50",
            "--actor-requeue-delay-seconds",
            "5",
            "--actor-failure-backoff-seconds",
            "30",
            "--actor-max-output-bytes",
            "8192",
        ]);
        Arguments::try_parse_from(enabled)
            .expect("an explicitly enabled actor supervisor accepts complete valid tuning");
    }

    #[test]
    fn actor_supervisor_tuning_requires_explicit_opt_in() {
        for (flag, value) in [
            ("--actor-attempt-timeout-milliseconds", "30000"),
            ("--actor-poll-milliseconds", "50"),
            ("--actor-requeue-delay-seconds", "5"),
            ("--actor-failure-backoff-seconds", "30"),
            ("--actor-max-output-bytes", "8192"),
        ] {
            let mut arguments = daemon_arguments();
            arguments.extend([flag, value]);
            assert_cli_error(arguments, ErrorKind::MissingRequiredArgument);
        }
    }

    #[test]
    fn actor_supervisor_rejects_out_of_range_tuning() {
        for (flag, value) in [
            ("--actor-attempt-timeout-milliseconds", "0"),
            ("--actor-attempt-timeout-milliseconds", "300001"),
            ("--actor-poll-milliseconds", "0"),
            ("--actor-poll-milliseconds", "1001"),
            ("--actor-requeue-delay-seconds", "0"),
            ("--actor-failure-backoff-seconds", "0"),
            ("--actor-max-output-bytes", "255"),
            ("--actor-max-output-bytes", "65537"),
        ] {
            let mut arguments = daemon_arguments();
            arguments.extend(["--actor-supervisor", flag, value]);
            assert_cli_error(arguments, ErrorKind::ValueValidation);
        }
    }
}
