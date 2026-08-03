use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Write as _},
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, bail, ensure};
use clap::{ArgGroup, Parser};
use jsonrpsee::server::{ServerBuilder, serve_with_graceful_shutdown, stop_channel};
use lez_maker_node::{
    BtcMakerActorProvisioner, MakerActorSupervisorCancellation, MakerActorSupervisorConfig,
    MakerRpc, ProcessLogosPriceSource, RunLocalDelivery, XmrMakerChatAuthority,
    ZecMakerActorProvisioner, chat_rpc_module, import_terminal_zec_maker_projection,
    owner_rpc_server::{OwnedPath, bind_owner_socket, server_config, validate_runtime_directory},
    rpc_module,
    secure_file::{load_raw_secret, load_secp256k1_secret, read_private_file},
    supervise_one_abandoned_maker_actor, supervise_one_abandoned_maker_actor_until,
    supervise_one_due_maker_actor_until,
};
use lez_swap_core::{Participant, SwapId};
use lez_swap_store::{
    MakerActorKindV1, MakerActorLeaseOwner, MakerActorManifestV1, SqliteSwapStore,
    SqliteZecRecoveryStore, validate_maker_actor_program,
};
use lez_xmr_swap_sdk::MoneroPrivateViewKey;
use lez_zec_swap_sdk::{ClaimPreimage, ProtectedClaimKey};
use rustix::fs::{CWD, FlockOperation, Mode, OFlags, ResolveFlags, flock, openat2};
use sd_notify::NotifyState;
use secp256k1::{PublicKey, SecretKey};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tokio::{net::UnixListener, task::JoinSet};
use xmr_reference_actor::{
    XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES, validate_maker_manifest_config_bytes,
};
const MAXIMUM_CONTROL_RPC_BODY_BYTES: u32 = 64 * 1024;
const MAXIMUM_CHAT_RPC_BODY_BYTES: u32 = 1024 * 1024;

type BtcChatAuthority = (SecretKey, BtcMakerActorProvisioner);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct XmrActorManifestRegistryV1 {
    schema_version: u16,
    actors: Vec<XmrActorManifestEntryV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct XmrActorManifestEntryV1 {
    swap_id: Box<str>,
    config_path: PathBuf,
    config_sha256: Box<str>,
    program_path: PathBuf,
    program_sha256: Box<str>,
    state_database_path: PathBuf,
}

#[derive(Parser)]
#[command(
    about = "Headless LEZ atomic-swap maker daemon",
    group(ArgGroup::new("pair_chat_authority")
        .args([
            "maker_claim_key_id",
            "btc_maker_signing_key_file",
            "xmr_actor_manifest_registry_file",
        ])
        .multiple(true))
)]
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
    #[arg(long, requires = "delivery_signing_key_file")]
    delivery_directory: Option<PathBuf>,
    /// Owner-only file containing one raw 32-byte secp256k1 key or its 64-byte
    /// lowercase hexadecimal encoding.
    #[arg(long, requires = "delivery_directory")]
    delivery_signing_key_file: Option<PathBuf>,
    /// Taker-facing run-local Chat socket, isolated from owner-control methods.
    #[arg(
        long,
        requires_all = [
            "delivery_directory",
            "delivery_signing_key_file",
            "pair_chat_authority"
        ]
    )]
    chat_socket: Option<PathBuf>,
    /// Non-secret rotation identifier for the maker claim-recovery key.
    #[arg(
        long,
        requires_all = [
            "delivery_directory",
            "maker_claim_key_file",
            "maker_claim_preimage_file",
            "zec_source_maker_config",
            "zec_maker_actor_root",
            "zec_actor_program",
            "zec_actor_program_sha256"
        ]
    )]
    maker_claim_key_id: Option<Box<str>>,
    /// Owner-only file containing one raw 32-byte claim-recovery key.
    #[arg(long, requires_all = ["delivery_directory", "maker_claim_key_id", "maker_claim_preimage_file"])]
    maker_claim_key_file: Option<PathBuf>,
    /// Owner-only file containing the maker-owned 32-byte claim preimage.
    #[arg(long, requires_all = ["delivery_directory", "maker_claim_key_id", "maker_claim_key_file"])]
    maker_claim_preimage_file: Option<PathBuf>,
    /// Existing owner-private per-swap Maker configs used as authority templates.
    #[arg(
        long,
        action = clap::ArgAction::Append,
        requires_all = [
            "delivery_directory",
            "maker_claim_key_id",
            "maker_claim_key_file",
            "maker_claim_preimage_file",
            "zec_maker_actor_root",
            "zec_actor_program",
            "zec_actor_program_sha256"
        ]
    )]
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
    /// Existing owner-private per-swap BTC Maker configs used as authority templates.
    #[arg(
        long,
        action = clap::ArgAction::Append,
        requires_all = [
            "delivery_directory",
            "btc_maker_signing_key_file",
            "btc_maker_actor_root",
            "btc_actor_program",
            "btc_actor_program_sha256"
        ]
    )]
    btc_source_maker_config: Vec<PathBuf>,
    /// Owner-only raw or hexadecimal secp256k1 key used to sign BTC agreements.
    #[arg(
        long,
        requires_all = [
            "delivery_directory",
            "btc_source_maker_config",
            "btc_maker_actor_root",
            "btc_actor_program",
            "btc_actor_program_sha256"
        ]
    )]
    btc_maker_signing_key_file: Option<PathBuf>,
    /// Existing owner-private mode-0700 base for deterministic BTC actor bundles.
    #[arg(long, requires_all = ["btc_source_maker_config", "btc_maker_signing_key_file", "btc_actor_program", "btc_actor_program_sha256"])]
    btc_maker_actor_root: Option<PathBuf>,
    /// Absolute exact BTC one-shot actor executable.
    #[arg(long, requires_all = ["btc_source_maker_config", "btc_maker_signing_key_file", "btc_maker_actor_root", "btc_actor_program_sha256"])]
    btc_actor_program: Option<PathBuf>,
    /// Exact 32-byte SHA-256 identity of the BTC actor executable.
    #[arg(long, requires_all = ["btc_source_maker_config", "btc_maker_signing_key_file", "btc_maker_actor_root", "btc_actor_program"])]
    btc_actor_program_sha256: Option<Box<str>>,
    /// Owner-only compressed secp256k1 Maker agreement public key.
    #[arg(
        long,
        requires_all = [
            "delivery_directory",
            "xmr_private_view_key_file",
            "xmr_actor_manifest_registry_file"
        ]
    )]
    xmr_maker_agreement_public_key_file: Option<PathBuf>,
    /// Owner-only raw 32-byte shared Monero private view key.
    #[arg(
        long,
        requires_all = [
            "delivery_directory",
            "xmr_maker_agreement_public_key_file",
            "xmr_actor_manifest_registry_file"
        ]
    )]
    xmr_private_view_key_file: Option<PathBuf>,
    /// Owner-only bounded JSON registry of Maker-only XMR actor manifests.
    #[arg(
        long,
        requires_all = [
            "delivery_directory",
            "xmr_maker_agreement_public_key_file",
            "xmr_private_view_key_file"
        ]
    )]
    xmr_actor_manifest_registry_file: Option<PathBuf>,
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
    /// Independent actor workers sharing one fenced daemon lease identity (1..=32).
    #[arg(long, requires = "actor_supervisor", value_parser = clap::value_parser!(u64).range(1..=32))]
    actor_worker_count: Option<u64>,
    /// Finite deadline for one actor status/effect cycle (1..=300000 milliseconds).
    #[arg(long, requires = "actor_supervisor", value_parser = clap::value_parser!(u64).range(1..=300_000))]
    actor_attempt_timeout_milliseconds: Option<u64>,
    /// Absolute Linux boot-time cutoff after which actor effects are forbidden.
    #[arg(long, requires = "actor_supervisor", value_parser = clap::value_parser!(u64).range(1..))]
    actor_effect_cutoff_boottime_milliseconds: Option<u64>,
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
) -> anyhow::Result<Vec<ActorSupervisorRuntime>> {
    let tuning = (
        arguments.actor_worker_count,
        arguments.actor_attempt_timeout_milliseconds,
        arguments.actor_effect_cutoff_boottime_milliseconds,
        arguments.actor_poll_milliseconds,
        arguments.actor_requeue_delay_seconds,
        arguments.actor_failure_backoff_seconds,
        arguments.actor_max_output_bytes,
    );
    if !arguments.actor_supervisor {
        ensure!(
            tuning == (None, None, None, None, None, None, None),
            "actor supervisor tuning requires explicit opt-in"
        );
        return Ok(Vec::new());
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
    let config = match arguments.actor_effect_cutoff_boottime_milliseconds {
        Some(cutoff) => config
            .with_effect_cutoff_boottime_milliseconds(cutoff)
            .context("validate actor effect cutoff")?,
        None => config,
    };

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

    actor_supervisor_runtimes(arguments, store, owner, &config)
}

fn actor_supervisor_runtimes(
    arguments: &Arguments,
    store: SqliteSwapStore,
    owner: MakerActorLeaseOwner,
    config: &MakerActorSupervisorConfig,
) -> anyhow::Result<Vec<ActorSupervisorRuntime>> {
    let worker_count = usize::try_from(arguments.actor_worker_count.unwrap_or(1))
        .context("convert validated actor worker count")?;
    let poll_interval = Duration::from_millis(arguments.actor_poll_milliseconds.unwrap_or(50));
    let mut runtimes = Vec::with_capacity(worker_count);
    runtimes.push(ActorSupervisorRuntime {
        store,
        owner,
        config: config.clone(),
        poll_interval,
    });
    for _ in 1..worker_count {
        runtimes.push(ActorSupervisorRuntime {
            store: SqliteSwapStore::open(&arguments.database)
                .context("open additional actor supervisor database connection")?,
            owner,
            config: config.clone(),
            poll_interval,
        });
    }
    Ok(runtimes)
}

fn run_actor_supervisor(
    mut runtime: ActorSupervisorRuntime,
    cancellation: &MakerActorSupervisorCancellation,
) -> anyhow::Result<()> {
    while !cancellation.is_cancelled() {
        let now = trusted_now_unix_seconds()?;
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
        thread::sleep(runtime.poll_interval);
    }
    Ok(())
}

fn run_actor_supervisors(
    runtimes: Vec<ActorSupervisorRuntime>,
    cancellation: &MakerActorSupervisorCancellation,
) -> anyhow::Result<()> {
    thread::scope(|scope| {
        let workers = runtimes
            .into_iter()
            .map(|runtime| {
                let cancellation = cancellation.clone();
                scope.spawn(move || {
                    let _cancellation_guard =
                        ActorSupervisorCancellationGuard(cancellation.clone());
                    run_actor_supervisor(runtime, &cancellation)
                })
            })
            .collect::<Vec<_>>();
        let mut first_error = None;
        for worker in workers {
            let result = match worker.join() {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("actor supervisor worker panicked")),
            };
            if first_error.is_none() {
                first_error = result.err();
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    })
}

#[tokio::main]
#[allow(clippy::too_many_lines)] // Keep startup, select, cancellation, and teardown order auditable.
async fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    let logos_price_source = configured_logos_price_source(&arguments)?;
    let zec_actor_provisioner = configured_zec_actor_provisioner(&arguments)?;
    let btc_chat_authority = configured_btc_chat_authority(&arguments)?;
    let xmr_chat_authority = configured_xmr_chat_authority(&arguments)?;
    let _state_lease = acquire_state_lease(&arguments.database)?;
    import_terminal_projection(&arguments).await?;
    let actor_supervisor = configured_actor_supervisor(&arguments)?;
    let (listener, _socket_guard) = bind_owner_socket(&arguments.socket)?;
    let (chat_listener, _chat_socket_guard) =
        bind_optional_owner_socket(arguments.chat_socket.as_deref())?;
    let context = maker_context(
        &arguments,
        zec_actor_provisioner,
        btc_chat_authority,
        xmr_chat_authority,
        logos_price_source,
    )?;
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
        .set_config(server_config(MAXIMUM_CONTROL_RPC_BODY_BYTES))
        .to_service_builder()
        .build(module, stop_handle.clone());
    let chat_service = chat_module.map(|module| {
        ServerBuilder::default()
            .set_config(server_config(MAXIMUM_CHAT_RPC_BODY_BYTES))
            .to_service_builder()
            .build(module, stop_handle.clone())
    });
    let mut connections = JoinSet::new();
    let supervisor_cancellation = MakerActorSupervisorCancellation::new();
    let _supervisor_cancellation_guard =
        ActorSupervisorCancellationGuard(supervisor_cancellation.clone());
    let mut supervisor_tasks = JoinSet::new();
    if !actor_supervisor.is_empty() {
        let cancellation = supervisor_cancellation.clone();
        supervisor_tasks
            .spawn_blocking(move || run_actor_supervisors(actor_supervisor, &cancellation));
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

fn configured_btc_chat_authority(
    arguments: &Arguments,
) -> anyhow::Result<Option<BtcChatAuthority>> {
    let deployment = (
        arguments.btc_maker_signing_key_file.as_ref(),
        arguments.btc_maker_actor_root.as_ref(),
        arguments.btc_actor_program.as_ref(),
        arguments.btc_actor_program_sha256.as_deref(),
    );
    if arguments.btc_source_maker_config.is_empty() {
        ensure!(
            deployment == (None, None, None, None),
            "BTC actor templates, signing key, root, program, and SHA-256 must be configured together"
        );
        return Ok(None);
    }
    let (Some(signing_key_file), Some(root), Some(program), Some(program_sha256)) = deployment
    else {
        bail!(
            "BTC actor templates, signing key, root, program, and SHA-256 must be configured together"
        );
    };
    validate_runtime_directory(root).context("validate BTC maker actor root")?;
    ensure!(
        root.is_absolute()
            && fs::canonicalize(root).context("canonicalize BTC maker actor root")? == *root,
        "BTC maker actor root must be absolute and canonical"
    );
    ensure!(
        program_sha256.len() == 64,
        "BTC actor program SHA-256 must contain exactly 32 bytes as hexadecimal"
    );
    let mut identity = [0_u8; 32];
    hex::decode_to_slice(program_sha256, &mut identity)
        .context("decode BTC actor program SHA-256")?;
    let signing_key = load_secp256k1_secret(signing_key_file, "BTC Maker signing key")?;
    let provisioner = BtcMakerActorProvisioner::new(
        &arguments.btc_source_maker_config,
        root.clone(),
        program.clone(),
        identity,
    )
    .context("validate BTC maker actor deployment")?;
    Ok(Some((signing_key, provisioner)))
}

fn configured_xmr_chat_authority(
    arguments: &Arguments,
) -> anyhow::Result<Option<XmrMakerChatAuthority>> {
    let configured = (
        arguments.xmr_maker_agreement_public_key_file.as_deref(),
        arguments.xmr_private_view_key_file.as_deref(),
        arguments.xmr_actor_manifest_registry_file.as_deref(),
    );
    let (public_key_file, view_key_file, registry_file) = match configured {
        (None, None, None) => return Ok(None),
        (Some(public_key), Some(view_key), Some(registry)) => (public_key, view_key, registry),
        _ => bail!(
            "XMR Maker agreement key, private view key, and actor registry must be configured together"
        ),
    };
    let maker_identity = load_compressed_public_key(public_key_file)?;
    let view_key = load_raw_secret(view_key_file, "XMR private view key")?;
    let private_view_key = MoneroPrivateViewKey::from_monero_little_endian(*view_key)
        .context("validate XMR private view key")?;
    let registry_bytes = read_private_file(registry_file, 1024 * 1024, "XMR actor registry")?;
    let registry: XmrActorManifestRegistryV1 =
        serde_json::from_slice(&registry_bytes).context("decode XMR actor registry")?;
    ensure!(
        registry.schema_version == 1 && !registry.actors.is_empty(),
        "XMR actor registry is empty or unsupported"
    );
    let actors = registry
        .actors
        .into_iter()
        .map(|entry| {
            let swap_id = SwapId::new(entry.swap_id).context("validate XMR actor swap ID")?;
            let mut binary_swap_id = [0_u8; 32];
            ensure!(
                swap_id.as_str().len() == 64
                    && hex::decode_to_slice(swap_id.as_str(), &mut binary_swap_id).is_ok()
                    && hex::encode(binary_swap_id) == swap_id.as_str(),
                "XMR actor swap ID must be exactly 32 lowercase hexadecimal bytes"
            );
            let config_sha256 = decode_sha256(&entry.config_sha256, "XMR actor config SHA-256")?;
            let program_sha256 = decode_sha256(&entry.program_sha256, "XMR actor program SHA-256")?;
            let config_bytes = read_private_file(
                &entry.config_path,
                XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
                "XMR Maker actor config",
            )?;
            let observed_config_sha256: [u8; 32] = Sha256::digest(config_bytes.as_slice()).into();
            ensure!(
                observed_config_sha256 == config_sha256,
                "XMR Maker actor config SHA-256 changed"
            );
            validate_maker_manifest_config_bytes(
                config_bytes.as_slice(),
                binary_swap_id,
                &entry.state_database_path,
            )
            .context("validate XMR Maker-only actor config semantics")?;
            validate_maker_actor_program(&entry.program_path, program_sha256)
                .context("validate XMR Maker actor program")?;
            MakerActorManifestV1::new(
                swap_id,
                MakerActorKindV1::Monero,
                entry.config_path,
                config_sha256,
                entry.program_path,
                program_sha256,
                entry.state_database_path,
            )
            .context("validate XMR Maker actor manifest")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    XmrMakerChatAuthority::new(maker_identity, private_view_key, actors)
        .context("validate XMR Chat authority")
        .map(Some)
}

fn load_compressed_public_key(path: &Path) -> anyhow::Result<[u8; 33]> {
    let encoded = read_private_file(path, 66, "XMR Maker agreement public key")?;
    let mut bytes = [0_u8; 33];
    match encoded.len() {
        33 => bytes.copy_from_slice(&encoded),
        66 => hex::decode_to_slice(&encoded, &mut bytes)
            .context("decode XMR Maker agreement public key")?,
        _ => bail!("XMR Maker agreement public key must be 33 raw or 66 hexadecimal bytes"),
    }
    let parsed =
        PublicKey::from_slice(&bytes).context("validate XMR Maker agreement public key")?;
    ensure!(
        parsed.serialize() == bytes,
        "XMR Maker agreement public key must be compressed"
    );
    Ok(bytes)
}

fn decode_sha256(encoded: &str, purpose: &str) -> anyhow::Result<[u8; 32]> {
    ensure!(
        encoded.len() == 64,
        "{purpose} must contain exactly 32 hexadecimal bytes"
    );
    let mut digest = [0_u8; 32];
    hex::decode_to_slice(encoded, &mut digest).with_context(|| format!("decode {purpose}"))?;
    Ok(digest)
}

fn maker_context(
    arguments: &Arguments,
    zec_actor_provisioner: Option<ZecMakerActorProvisioner>,
    btc_chat_authority: Option<BtcChatAuthority>,
    xmr_chat_authority: Option<XmrMakerChatAuthority>,
    logos_price_source: Option<ProcessLogosPriceSource>,
) -> anyhow::Result<MakerRpc> {
    let store = SqliteSwapStore::open(&arguments.database).context("open maker database")?;
    let delivery_configured = (
        arguments.delivery_directory.as_deref(),
        arguments.delivery_signing_key_file.as_deref(),
    );
    let (directory, signing_file) = match delivery_configured {
        (Some(directory), Some(signing_file)) => (directory, signing_file),
        (None, None) => {
            ensure!(
                arguments.chat_socket.is_none()
                    && arguments.maker_claim_key_id.is_none()
                    && arguments.maker_claim_key_file.is_none()
                    && arguments.maker_claim_preimage_file.is_none()
                    && zec_actor_provisioner.is_none()
                    && btc_chat_authority.is_none()
                    && xmr_chat_authority.is_none(),
                "Delivery, Chat, and pair authority require a complete Delivery transport"
            );
            let context = MakerRpc::new(store);
            return Ok(match logos_price_source {
                Some(source) => context.with_logos_price_source(source),
                None => context,
            });
        }
        _ => bail!("Delivery directory and signing key must be configured together"),
    };

    let zec_authority = match (
        arguments.maker_claim_key_id.as_deref(),
        arguments.maker_claim_key_file.as_deref(),
        arguments.maker_claim_preimage_file.as_deref(),
        zec_actor_provisioner,
    ) {
        (None, None, None, None) => None,
        (Some(claim_key_id), Some(claim_key_file), Some(preimage_file), Some(provisioner)) => {
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
            Some((recovery_store, preimage, provisioner))
        }
        _ => bail!(
            "ZEC claim, preimage, templates, root, program, and SHA-256 authority must be configured together"
        ),
    };
    if arguments.chat_socket.is_some() {
        ensure!(
            zec_authority.is_some() || btc_chat_authority.is_some() || xmr_chat_authority.is_some(),
            "Chat requires at least one complete pair authority"
        );
    } else {
        ensure!(
            zec_authority.is_none() && btc_chat_authority.is_none() && xmr_chat_authority.is_none(),
            "pair authority requires a Chat socket"
        );
    }

    let signing_key = load_delivery_key(signing_file)?;
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
    let context = MakerRpc::with_delivery_transport(store, delivery, signing_key);
    let context = match zec_authority {
        Some((recovery_store, preimage, provisioner)) => {
            context.with_zec_chat_authority(recovery_store, preimage, Some(provisioner))
        }
        None => context,
    };
    let context = match btc_chat_authority {
        Some((signing_key, provisioner)) => {
            context.with_btc_chat_authority(signing_key, provisioner)
        }
        None => context,
    };
    let context = match xmr_chat_authority {
        Some(authority) => context.with_xmr_chat_authority(authority),
        None => context,
    };
    Ok(match logos_price_source {
        Some(source) => context.with_logos_price_source(source),
        None => context,
    })
}

fn load_delivery_key(path: &Path) -> anyhow::Result<SecretKey> {
    load_secp256k1_secret(path, "Delivery signing key")
}

fn trusted_now_unix_seconds() -> anyhow::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")
        .map(|duration| duration.as_secs())
}

fn create_ready_file(path: &Path, socket: &Path) -> anyhow::Result<OwnedPath> {
    ensure!(path.is_absolute(), "maker readiness path must be absolute");
    ensure!(
        path.parent() == socket.parent(),
        "maker readiness file must share the socket runtime directory"
    );
    let parent = path
        .parent()
        .context("maker readiness path has no parent")?;
    let mut staged = tempfile::Builder::new()
        .prefix(".maker-ready.")
        .tempfile_in(parent)
        .context("stage maker readiness file")?;
    writeln!(staged, "{}", socket.display()).context("write maker readiness file")?;
    staged
        .as_file_mut()
        .sync_all()
        .context("sync staged maker readiness file")?;
    staged
        .persist_noclobber(path)
        .map_err(|error| error.error)
        .context("publish maker readiness file without clobber")?;
    File::open(parent)?.sync_all()?;
    let guard = OwnedPath::capture(path).context("capture maker readiness file identity")?;
    Ok(guard)
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
    fn chat_cli_requires_complete_xmr_authority() {
        let complete = vec![
            "lez-maker-daemon",
            "--database",
            "/tmp/maker.sqlite3",
            "--delivery-directory",
            "/tmp/delivery",
            "--delivery-signing-key-file",
            "/tmp/delivery.key",
            "--chat-socket",
            "/tmp/chat.sock",
            "--xmr-maker-agreement-public-key-file",
            "/tmp/xmr-maker.pub",
            "--xmr-private-view-key-file",
            "/tmp/xmr-view.key",
            "--xmr-actor-manifest-registry-file",
            "/tmp/xmr-actors.json",
        ];
        Arguments::try_parse_from(complete.clone()).expect("complete XMR Chat authority parses");

        for omitted in [
            "--xmr-maker-agreement-public-key-file",
            "--xmr-private-view-key-file",
            "--xmr-actor-manifest-registry-file",
        ] {
            let index = complete
                .iter()
                .position(|value| *value == omitted)
                .expect("flag exists");
            let mut incomplete = complete.clone();
            incomplete.drain(index..=index + 1);
            assert!(
                Arguments::try_parse_from(incomplete).is_err(),
                "XMR Chat authority must reject omission of {omitted}"
            );
        }
    }

    #[test]
    fn actor_supervisor_is_opt_in_and_accepts_complete_valid_tuning() {
        Arguments::try_parse_from(daemon_arguments())
            .expect("the actor supervisor remains disabled unless explicitly requested");

        let mut enabled = daemon_arguments();
        enabled.extend([
            "--actor-supervisor",
            "--actor-worker-count",
            "2",
            "--actor-attempt-timeout-milliseconds",
            "30000",
            "--actor-effect-cutoff-boottime-milliseconds",
            "1",
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
            ("--actor-worker-count", "2"),
            ("--actor-attempt-timeout-milliseconds", "30000"),
            ("--actor-effect-cutoff-boottime-milliseconds", "1"),
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
            ("--actor-worker-count", "0"),
            ("--actor-worker-count", "33"),
            ("--actor-attempt-timeout-milliseconds", "0"),
            ("--actor-effect-cutoff-boottime-milliseconds", "0"),
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
