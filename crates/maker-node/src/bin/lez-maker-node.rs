use std::{
    ffi::OsString,
    fs::{self, File},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, bail, ensure};
use clap::{ArgGroup, Parser};
use jsonrpsee::server::{ServerBuilder, serve_with_graceful_shutdown, stop_channel};
#[cfg(feature = "pair-xmr")]
use lez_maker_node::XmrMakerReservations;
#[cfg(feature = "pair-zec")]
use lez_maker_node::secure_file::load_raw_secret;
use lez_maker_node::{
    BtcMakerActorProvisioner, BtcMakerLifecycle, BtcMakerRoleAgreementAuthority,
    MakerActorSupervisorCancellation, MakerActorSupervisorConfig, MakerRpc,
    ProcessLogosPriceSource, ProcessRouteHealthProbe, RunLocalDelivery, chat_rpc_module,
    clear_maker_lez_claim_admissions,
    node_config::load_node_arguments,
    owner_rpc_server::{
        OwnedPath, bind_owner_socket, publish_ready_file, server_config, validate_runtime_directory,
    },
    rpc_module,
    secure_file::{load_secp256k1_secret, read_private_file},
    supervise_one_abandoned_maker_actor, supervise_one_abandoned_maker_actor_until,
    supervise_one_due_maker_actor_until,
};
#[cfg(feature = "pair-zec")]
use lez_maker_node::{ZecMakerActorProvisioner, import_terminal_zec_maker_projection};
use lez_node_common::shutdown_signal;
#[cfg(feature = "pair-zec")]
use lez_swap_core::Participant;
#[cfg(feature = "pair-zec")]
use lez_swap_core::SwapId;
#[cfg(feature = "pair-zec")]
use lez_swap_store::SqliteZecRecoveryStore;
use lez_swap_store::{
    MakerActorLeaseOwner, SqliteSwapStore, is_owner_private_regular_file, open_no_symlinks,
};
#[cfg(feature = "pair-zec")]
use lez_zec_swap_sdk::ProtectedClaimKey;
use rustix::fs::{FlockOperation, Mode, OFlags, flock};
use sd_notify::NotifyState;
use secp256k1::SecretKey;
use tokio::{net::UnixListener, task::JoinSet};
/// How rarely a route-health reconciliation failure may be written. The reconciliation ticks
/// every second, so an unthrottled line per failure is a way to fill a disk.
const ROUTE_HEALTH_LOG_INTERVAL: Duration = Duration::from_mins(1);
const MAXIMUM_CONTROL_RPC_BODY_BYTES: u32 = 64 * 1024;
const MAXIMUM_CHAT_RPC_BODY_BYTES: u32 = 1024 * 1024;
type BtcChatAuthority = (
    SecretKey,
    Option<BtcMakerActorProvisioner>,
    Option<BtcMakerRoleAgreementAuthority>,
    Option<BtcMakerLifecycle>,
);

#[derive(Parser)]
#[command(
    about = "Owner-local LEZ atomic-swap Maker Node",
    group(ArgGroup::new("pair_chat_authority").multiple(true))
)]
struct Arguments {
    /// Owner-only Unix-domain control socket.
    #[arg(long, default_value = "/run/lez/maker/node.sock")]
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
    #[arg(long, group = "pair_chat_authority", requires = "delivery_directory")]
    btc_maker_signing_key_file: Option<PathBuf>,
    /// Existing owner-private role root created before Chat v2 negotiation.
    #[arg(
        long,
        requires_all = ["delivery_directory", "btc_maker_signing_key_file"]
    )]
    btc_maker_role_root: Option<PathBuf>,
    /// Existing owner-private mode-0700 base for deterministic BTC actor bundles.
    /// Node-owned Bitcoin lifecycle configuration (ADR 0213): per-reservation
    /// role roots, funding wallet, LEZ sidecar, ceremony and actor synthesis.
    #[arg(
        long,
        requires_all = ["delivery_directory", "btc_maker_signing_key_file"]
    )]
    btc_role_config: Option<PathBuf>,
    #[arg(long, requires_all = ["btc_source_maker_config", "btc_maker_signing_key_file", "btc_actor_program", "btc_actor_program_sha256"])]
    btc_maker_actor_root: Option<PathBuf>,
    /// Absolute exact BTC one-shot actor executable.
    #[arg(long, requires_all = ["btc_source_maker_config", "btc_maker_signing_key_file", "btc_maker_actor_root", "btc_actor_program_sha256"])]
    btc_actor_program: Option<PathBuf>,
    /// Exact 32-byte SHA-256 identity of the BTC actor executable.
    #[arg(long, requires_all = ["btc_source_maker_config", "btc_maker_signing_key_file", "btc_maker_actor_root", "btc_actor_program"])]
    btc_actor_program_sha256: Option<Box<str>>,
    /// ZEC pair authority; compiled only with the `pair-zec` feature.
    #[cfg(feature = "pair-zec")]
    #[command(flatten)]
    zec: ZecArguments,
    /// XMR pair authority; compiled only with the `pair-xmr` feature.
    #[cfg(feature = "pair-xmr")]
    #[command(flatten)]
    xmr: XmrArguments,
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
    /// Owner-private strict JSON mapping routes to hash-pinned semantic health commands.
    #[arg(long)]
    route_health_config: Option<PathBuf>,
    /// Automatic route-health reconciliation cadence; defaults to 1000 milliseconds.
    #[arg(
        long,
        requires = "route_health_config",
        value_parser = clap::value_parser!(u64).range(100..=60_000)
    )]
    route_health_poll_milliseconds: Option<u64>,
    /// Seconds the finalized LEZ block id may stand still, while the
    /// sequencer's tip moves, before `maker_health` reports
    /// `lez_finality_stalled` (Node-owned lifecycle only); 600 when not given.
    #[arg(long, value_parser = clap::value_parser!(u64).range(10..=86_400))]
    lez_finality_stall_seconds: Option<u64>,
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

/// Maker-owned ZEC claim, actor-deployment, and terminal-projection flags.
#[cfg(feature = "pair-zec")]
#[derive(clap::Args)]
struct ZecArguments {
    /// Non-secret rotation identifier for the maker claim-recovery key.
    #[arg(
        long,
        group = "pair_chat_authority",
        requires_all = [
            "delivery_directory",
            "maker_claim_key_file",
            "zec_role_config",
            "zec_maker_actor_root",
            "zec_actor_program",
            "zec_actor_program_sha256"
        ]
    )]
    maker_claim_key_id: Option<Box<str>>,
    /// Owner-only file containing one raw 32-byte claim-recovery key.
    #[arg(long, requires_all = ["delivery_directory", "maker_claim_key_id"])]
    maker_claim_key_file: Option<PathBuf>,
    /// Owner-private Zcash role config (profile, Zebra): each Zcash agreement
    /// is composed at take time (`zec_reserve_v1`). The Node has no other
    /// source of a Zcash swap.
    #[arg(long, requires_all = ["btc_role_config", "maker_claim_key_id", "zec_maker_actor_root"])]
    zec_role_config: Option<PathBuf>,
    /// Existing owner-private mode-0700 base for deterministic per-swap actor bundles.
    #[arg(long, requires_all = ["maker_claim_key_id", "zec_actor_program", "zec_actor_program_sha256"])]
    zec_maker_actor_root: Option<PathBuf>,
    /// Absolute exact ZEC one-shot actor executable.
    #[arg(long, requires_all = ["zec_maker_actor_root", "zec_actor_program_sha256"])]
    zec_actor_program: Option<PathBuf>,
    /// Exact 32-byte SHA-256 identity of the ZEC actor executable.
    #[arg(long, requires_all = ["zec_maker_actor_root", "zec_actor_program"])]
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
}

/// Maker-owned Monero take-time agreement and actor-deployment flags.
#[cfg(feature = "pair-xmr")]
#[allow(clippy::struct_field_names)] // The field names are the public --xmr-* flags.
#[derive(clap::Args)]
struct XmrArguments {
    /// Owner-private Monero role config (monerod, wallet-rpc, windows,
    /// Stage-A planner, effect tools): compose each Monero agreement at take
    /// time (`xmr_reserve_v1`).
    #[arg(
        long,
        group = "pair_chat_authority",
        requires_all = [
            "delivery_directory",
            "btc_role_config",
            "xmr_maker_actor_root",
            "xmr_actor_program",
            "xmr_actor_program_sha256"
        ]
    )]
    xmr_role_config: Option<PathBuf>,
    /// Existing owner-private mode-0700 base for per-swap XMR actor bundles.
    #[arg(long, requires = "xmr_role_config")]
    xmr_maker_actor_root: Option<PathBuf>,
    /// Absolute exact XMR Maker actor executable (`lez-xmr-maker-actor`).
    #[arg(long, requires = "xmr_role_config")]
    xmr_actor_program: Option<PathBuf>,
    /// Exact 32-byte SHA-256 identity of the XMR actor executable.
    #[arg(long, requires = "xmr_role_config")]
    xmr_actor_program_sha256: Option<Box<str>>,
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
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error).context("inspect actor test pause marker"),
                    Ok(metadata) => ensure!(
                        is_owner_private_regular_file(&metadata, 0o600),
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
    // No claim attempt runs yet: an admission marker now is a crashed Node's.
    clear_maker_lez_claim_admissions(&store);

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
    let arguments = load_arguments()?;
    let logos_price_source = configured_logos_price_source(&arguments)?;
    let route_health_probe = configured_route_health_probe(&arguments)?;
    let pair_authorities = PairAuthorities::configured(&arguments)?;
    let _state_lease = acquire_state_lease(&arguments.database)?;
    #[cfg(feature = "pair-zec")]
    lez_maker_node::refuse_start_with_legacy_zec_swaps(
        &SqliteSwapStore::open(&arguments.database).context("open maker database")?,
    )?;
    #[cfg(feature = "pair-zec")]
    import_terminal_projection(&arguments).await?;
    let actor_supervisor = configured_actor_supervisor(&arguments)?;
    let (listener, _socket_guard) = bind_owner_socket(&arguments.socket)?;
    let (chat_listener, _chat_socket_guard) =
        bind_optional_owner_socket(arguments.chat_socket.as_deref())?;
    let context = maker_context(&arguments, pair_authorities, logos_price_source)?;
    let context = attach_chat_health(context, arguments.chat_socket.as_deref());
    let context = match route_health_probe {
        Some(probe) => {
            // A route whose probe executable is missing or changed reads unavailable; it
            // does not stop the Maker, and it recovers without a restart once the operator
            // puts the executable in place. Name them so nobody has to guess.
            for route in probe.unverified_routes() {
                eprintln!(
                    "route health: {route:?} has no usable probe executable; this route is \
                     unavailable until it is installed"
                );
            }
            context.with_route_health_probe(Arc::new(probe))
        }
        None => context,
    };
    if context.route_health_is_configured() {
        // Best effort: the offers this would withdraw are withdrawn by the periodic
        // reconciliation too, and a Maker that refuses to start helps nobody.
        if let Err(error) = context.reconcile_route_health() {
            eprintln!("route health: initial reconciliation failed, continuing: {error}");
        }
    }
    let module = rpc_module(context.clone())?;
    let chat_module = if chat_listener.is_some() {
        Some(chat_rpc_module(context.clone())?)
    } else {
        None
    };
    let _ready_guard = arguments
        .ready_file
        .as_deref()
        .map(|path| publish_ready_file(path, &arguments.socket, "Maker"))
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
    let mut route_health_tasks = JoinSet::new();
    if !actor_supervisor.is_empty() {
        let cancellation = supervisor_cancellation.clone();
        supervisor_tasks
            .spawn_blocking(move || run_actor_supervisors(actor_supervisor, &cancellation));
    }
    let mut daemon_error = None;
    let route_health_enabled = context.route_health_is_configured();
    let mut route_health_failures: u64 = 0;
    let mut route_health_last_logged: Option<Instant> = None;
    let mut route_health_interval = tokio::time::interval(Duration::from_millis(
        arguments.route_health_poll_milliseconds.unwrap_or(1_000),
    ));
    route_health_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    route_health_interval.tick().await;
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
            _ = route_health_interval.tick(), if route_health_enabled && route_health_tasks.is_empty() => {
                let health_context = context.clone();
                route_health_tasks.spawn_blocking(move || health_context.reconcile_route_health());
            }
            result = route_health_tasks.join_next(), if !route_health_tasks.is_empty() => {
                let result = result.expect("enabled route health task is present");
                // Logged degradation, not an exit. This runs every second; a transient store
                // or clock error used to stop the daemon, and with Restart=on-failure and
                // StartLimitBurst=3 a repeatable one left the unit dead. The next tick
                // retries, and the withdrawal request ids are deterministic so it is idempotent.
                match result.context("join route health task").and_then(|value| value) {
                    Err(error) => {
                        // Throttled, because this ticks every second: an error that persists
                        // would otherwise write a line a second for as long as it lasts, and
                        // filling the journal is its own outage. The count says how many were
                        // suppressed, so a persistent fault still reads as persistent.
                        route_health_failures += 1;
                        if route_health_last_logged
                            .is_none_or(|at| at.elapsed() >= ROUTE_HEALTH_LOG_INTERVAL)
                        {
                            eprintln!(
                                "route health: reconciliation failed {route_health_failures} \
                                 time(s), retrying on the next tick: {error:#}"
                            );
                            route_health_last_logged = Some(Instant::now());
                            route_health_failures = 0;
                        }
                    }
                    Ok(()) => route_health_failures = 0,
                }
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
    while let Some(result) = route_health_tasks.join_next().await {
        result
            .context("join route health task during shutdown")?
            .context("stop route health task")?;
    }
    while let Some(connection) = connections.join_next().await {
        // A client that hung up before its reply was written is that client's
        // problem, not a reason to turn an orderly stop into a failed one.
        if let Err(error) = connection.context("join local RPC connection")? {
            eprintln!("maker RPC connection ended with an error: {error}");
        }
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

fn load_arguments() -> anyhow::Result<Arguments> {
    load_node_arguments("Maker")
}

#[cfg(test)]
fn parse_daemon_config(bytes: &[u8], executable: OsString) -> anyhow::Result<Arguments> {
    lez_maker_node::node_config::parse_node_config(bytes, executable, "Maker")
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
        NotifyState::Status("Maker Node is stopping"),
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
    let file = open_no_symlinks(
        &lease_path,
        OFlags::RDWR | OFlags::CREATE,
        Mode::RUSR | Mode::WUSR,
    )
    .context("open maker database lease")?;
    let metadata = file.metadata().context("inspect maker database lease")?;
    ensure!(
        is_owner_private_regular_file(&metadata, 0o600),
        "maker database lease must be an owner-owned, single-link mode-0600 regular file"
    );
    flock(&file, FlockOperation::NonBlockingLockExclusive)
        .context("acquire exclusive maker database lease")?;
    Ok(StateLease { _file: file })
}

#[cfg(feature = "pair-zec")]
async fn import_terminal_projection(arguments: &Arguments) -> anyhow::Result<()> {
    let configured = (
        arguments.zec.terminal_zec_maker_state_db.as_deref(),
        arguments.zec.terminal_zec_swap_id.as_deref(),
        arguments.zec.terminal_zec_claim_key_id.as_deref(),
        arguments.zec.terminal_zec_claim_key_file.as_deref(),
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

fn configured_route_health_probe(
    arguments: &Arguments,
) -> anyhow::Result<Option<ProcessRouteHealthProbe>> {
    let Some(path) = arguments.route_health_config.as_deref() else {
        ensure!(
            arguments.route_health_poll_milliseconds.is_none(),
            "route health poll cadence requires a route health configuration"
        );
        return Ok(None);
    };
    let bytes = read_private_file(path, 64 * 1024, "route health configuration")?;
    ProcessRouteHealthProbe::from_json_bytes(&bytes)
        .context("validate route health configuration")
        .map(Some)
}

#[cfg(feature = "pair-zec")]
fn configured_zec_actor_provisioner(
    arguments: &Arguments,
) -> anyhow::Result<Option<ZecMakerActorProvisioner>> {
    let deployment = (
        arguments.zec.zec_maker_actor_root.as_ref(),
        arguments.zec.zec_actor_program.as_ref(),
        arguments.zec.zec_actor_program_sha256.as_deref(),
    );
    if arguments.zec.zec_role_config.is_none() {
        ensure!(
            deployment == (None, None, None),
            "ZEC role config, actor root, program, and SHA-256 must be configured together"
        );
        return Ok(None);
    }
    let (Some(root), Some(program), Some(program_sha256)) = deployment else {
        bail!("ZEC role config, actor root, program, and SHA-256 must be configured together");
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
    ZecMakerActorProvisioner::new(root.clone(), program.clone(), identity)
        .context("validate ZEC maker actor deployment")
        .map(Some)
}

fn configured_btc_chat_authority(
    arguments: &Arguments,
) -> anyhow::Result<Option<BtcChatAuthority>> {
    let actor_deployment = (
        arguments.btc_maker_actor_root.as_ref(),
        arguments.btc_actor_program.as_ref(),
        arguments.btc_actor_program_sha256.as_deref(),
    );
    let any_authority = !arguments.btc_source_maker_config.is_empty()
        || arguments.btc_maker_role_root.is_some()
        || arguments.btc_role_config.is_some()
        || arguments.btc_maker_signing_key_file.is_some()
        || actor_deployment != (None, None, None);
    if !any_authority {
        return Ok(None);
    }
    let signing_key_file = arguments
        .btc_maker_signing_key_file
        .as_ref()
        .context("BTC Chat authority requires a Maker signing key")?;
    let signing_key = load_secp256k1_secret(signing_key_file, "BTC Maker signing key")?;
    let actor_provisioner = if arguments.btc_source_maker_config.is_empty() {
        ensure!(
            actor_deployment == (None, None, None),
            "BTC actor templates, root, program, and SHA-256 must be configured together"
        );
        None
    } else {
        let (Some(root), Some(program), Some(program_sha256)) = actor_deployment else {
            bail!("BTC actor templates, root, program, and SHA-256 must be configured together");
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
        Some(
            BtcMakerActorProvisioner::new(
                &arguments.btc_source_maker_config,
                root.clone(),
                program.clone(),
                identity,
            )
            .context("validate BTC maker actor deployment")?,
        )
    };
    let role_authority = arguments
        .btc_maker_role_root
        .as_ref()
        .map(|root| BtcMakerRoleAgreementAuthority::new(root.clone(), &signing_key))
        .transpose()
        .context("validate BTC Maker role-agreement authority")?;
    let lifecycle = arguments
        .btc_role_config
        .as_ref()
        .map(|path| BtcMakerLifecycle::load(path))
        .transpose()
        .context("validate BTC Maker lifecycle configuration")?
        .map(|lifecycle| match arguments.lez_finality_stall_seconds {
            Some(seconds) => lifecycle.with_lez_finality_stall_seconds(seconds),
            None => lifecycle,
        });
    ensure!(
        actor_provisioner.is_some() || role_authority.is_some() || lifecycle.is_some(),
        "BTC signing key requires actor templates, a role-agreement root, or a role configuration"
    );
    Ok(Some((
        signing_key,
        actor_provisioner,
        role_authority,
        lifecycle,
    )))
}

#[cfg(feature = "pair-xmr")]
fn configured_xmr_reservations(
    arguments: &Arguments,
) -> anyhow::Result<Option<XmrMakerReservations>> {
    let configured = (
        arguments.xmr.xmr_role_config.as_deref(),
        arguments.xmr.xmr_maker_actor_root.as_deref(),
        arguments.xmr.xmr_actor_program.as_deref(),
        arguments.xmr.xmr_actor_program_sha256.as_deref(),
        arguments.btc_role_config.as_deref(),
    );
    let (config, root, program, program_sha256, role_config) = match configured {
        (None, None, None, None, _) => return Ok(None),
        (Some(config), Some(root), Some(program), Some(sha256), Some(role)) => {
            (config, root, program, sha256, role)
        }
        _ => bail!(
            "XMR role config, actor root, program, SHA-256 and --btc-role-config must be configured together"
        ),
    };
    validate_runtime_directory(root).context("validate XMR maker actor root")?;
    ensure!(
        root.is_absolute()
            && fs::canonicalize(root).context("canonicalize XMR maker actor root")? == *root,
        "XMR maker actor root must be absolute and canonical"
    );
    let program_sha256 = decode_sha256(program_sha256, "XMR actor program SHA-256")?;
    XmrMakerReservations::load(config, role_config, root, program, program_sha256)
        .context("configure XMR reservations")
        .map(Some)
}

#[cfg(feature = "pair-xmr")]
fn decode_sha256(encoded: &str, purpose: &str) -> anyhow::Result<[u8; 32]> {
    ensure!(
        encoded.len() == 64,
        "{purpose} must contain exactly 32 hexadecimal bytes"
    );
    let mut digest = [0_u8; 32];
    hex::decode_to_slice(encoded, &mut digest).with_context(|| format!("decode {purpose}"))?;
    Ok(digest)
}

/// Every pair authority the daemon was asked to serve, validated before any
/// socket is bound. Pairs that are not compiled in have no field at all.
struct PairAuthorities {
    #[cfg(feature = "pair-zec")]
    zec_actor_provisioner: Option<ZecMakerActorProvisioner>,
    btc_chat: Option<BtcChatAuthority>,
    #[cfg(feature = "pair-xmr")]
    xmr_chat: Option<XmrMakerReservations>,
}

impl PairAuthorities {
    fn configured(arguments: &Arguments) -> anyhow::Result<Self> {
        Ok(Self {
            #[cfg(feature = "pair-zec")]
            zec_actor_provisioner: configured_zec_actor_provisioner(arguments)?,
            btc_chat: configured_btc_chat_authority(arguments)?,
            #[cfg(feature = "pair-xmr")]
            xmr_chat: configured_xmr_reservations(arguments)?,
        })
    }

    fn any_configured(&self) -> bool {
        #[cfg(feature = "pair-zec")]
        let zec = self.zec_actor_provisioner.is_some();
        #[cfg(not(feature = "pair-zec"))]
        let zec = false;
        #[cfg(feature = "pair-xmr")]
        let xmr = self.xmr_chat.is_some();
        #[cfg(not(feature = "pair-xmr"))]
        let xmr = false;
        self.btc_chat.is_some() || zec || xmr
    }
}

/// Whether the ZEC claim flags are absent; trivially true when ZEC is not built.
#[cfg(feature = "pair-zec")]
fn zec_claim_arguments_absent(arguments: &Arguments) -> bool {
    arguments.zec.maker_claim_key_id.is_none() && arguments.zec.maker_claim_key_file.is_none()
}

#[cfg(not(feature = "pair-zec"))]
const fn zec_claim_arguments_absent(_arguments: &Arguments) -> bool {
    true
}

#[cfg(feature = "pair-zec")]
type ZecAuthority = (SqliteZecRecoveryStore, ZecMakerActorProvisioner);

#[cfg(feature = "pair-zec")]
fn configured_zec_authority(
    arguments: &Arguments,
    zec_actor_provisioner: Option<ZecMakerActorProvisioner>,
) -> anyhow::Result<Option<ZecAuthority>> {
    match (
        arguments.zec.maker_claim_key_id.as_deref(),
        arguments.zec.maker_claim_key_file.as_deref(),
        zec_actor_provisioner,
    ) {
        (None, None, None) => Ok(None),
        // The Maker holds no claim preimage (the Taker holds the swap secret
        // in both directions), so its claim-recovery key is its only ZEC
        // claim secret.
        (Some(claim_key_id), Some(claim_key_file), Some(provisioner)) => {
            let claim_key_material = load_raw_secret(claim_key_file, "maker claim-recovery key")?;
            let claim_key = ProtectedClaimKey::new(claim_key_id, *claim_key_material)
                .context("validate maker claim-recovery key ID")?;
            let recovery_store = SqliteZecRecoveryStore::open_claim_capable(
                &arguments.database,
                Participant::Maker,
                claim_key,
            )
            .context("open maker ZEC recovery store")?;
            Ok(Some((recovery_store, provisioner)))
        }
        _ => bail!(
            "ZEC claim, role config, actor root, program, and SHA-256 authority must be configured together"
        ),
    }
}

fn maker_context(
    arguments: &Arguments,
    pairs: PairAuthorities,
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
                    && zec_claim_arguments_absent(arguments)
                    && !pairs.any_configured(),
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

    #[cfg(feature = "pair-zec")]
    let zec_authority = configured_zec_authority(arguments, pairs.zec_actor_provisioner)?;
    #[cfg(feature = "pair-zec")]
    let zec_configured = zec_authority.is_some();
    #[cfg(not(feature = "pair-zec"))]
    let zec_configured = false;
    #[cfg(feature = "pair-xmr")]
    let xmr_configured = pairs.xmr_chat.is_some();
    #[cfg(not(feature = "pair-xmr"))]
    let xmr_configured = false;
    let pair_authority_configured = pairs.btc_chat.is_some() || zec_configured || xmr_configured;
    if arguments.chat_socket.is_some() {
        ensure!(
            pair_authority_configured,
            "Chat requires at least one complete pair authority"
        );
    } else {
        ensure!(
            !pair_authority_configured,
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
    // A Delivery outage at start (R2) does not keep the Node and its swaps
    // down: maker_health reconciles the advertisements on every call (the
    // container's health check calls it every few seconds) once Delivery is back.
    if let Err(error) = delivery.reconcile(&active, now_unix_seconds) {
        eprintln!("maker: Delivery unavailable at start ({error}); reconciled once it is back");
    }
    let context = MakerRpc::with_delivery_transport(store, delivery, signing_key);
    #[cfg(feature = "pair-zec")]
    let context = match zec_authority {
        Some((recovery_store, provisioner)) => {
            context.with_directional_zec_chat_authority(recovery_store, provisioner)
        }
        None => context,
    };
    let context = match pairs.btc_chat {
        Some((signing_key, provisioner, role_authority, lifecycle)) => {
            let context =
                context.with_btc_chat_authorities(signing_key, provisioner, role_authority);
            match lifecycle {
                Some(lifecycle) => context.with_btc_lifecycle(lifecycle),
                None => context,
            }
        }
        None => context,
    };
    #[cfg(feature = "pair-zec")]
    let context = match &arguments.zec.zec_role_config {
        Some(path) => context
            .with_zec_reservations(
                path,
                arguments
                    .btc_role_config
                    .as_deref()
                    .context("ZEC reservations need --btc-role-config")?,
                signing_file,
            )
            .context("configure ZEC reservations")?,
        None => context,
    };
    #[cfg(feature = "pair-xmr")]
    let context = match pairs.xmr_chat {
        Some(reservations) => context.with_xmr_reservations(reservations),
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

#[cfg(test)]
mod tests {
    use super::{Arguments, parse_daemon_config};
    use clap::{Parser as _, error::ErrorKind};
    use std::ffi::OsString;

    fn daemon_arguments() -> Vec<&'static str> {
        vec!["lez-maker-node", "--database", "/tmp/maker.sqlite3"]
    }

    fn assert_cli_error(arguments: Vec<&'static str>, expected: ErrorKind) {
        let Err(error) = Arguments::try_parse_from(arguments) else {
            panic!("CLI must reject arguments");
        };
        assert_eq!(error.kind(), expected, "{error}");
    }

    #[cfg(feature = "pair-zec")]
    fn chat_arguments() -> Vec<&'static str> {
        vec![
            "lez-maker-node",
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
        ]
    }

    #[cfg(feature = "pair-zec")]
    #[test]
    fn chat_cli_requires_complete_zec_actor_deployment() {
        assert!(
            Arguments::try_parse_from(chat_arguments()).is_err(),
            "serving Chat without actor authority must fail during CLI parsing"
        );

        let deployment = [
            "--zec-maker-actor-root",
            "/tmp/actors",
            "--zec-actor-program",
            "/usr/bin/true",
            "--zec-actor-program-sha256",
            "00f5ba0b54bda7a48c9380b740fc38a8710a23c0f3f948cdcc9b7d5e712d1f8f",
        ];
        let mut without_role = chat_arguments();
        without_role.extend(deployment);
        assert_cli_error(without_role, ErrorKind::MissingRequiredArgument);

        let mut complete = chat_arguments();
        complete.extend(deployment);
        complete.extend([
            "--btc-maker-signing-key-file",
            "/tmp/btc.key",
            "--btc-role-config",
            "/tmp/btc-role.json",
            "--zec-role-config",
            "/tmp/zec-role.json",
        ]);
        Arguments::try_parse_from(complete).expect("complete Chat deployment parses");
    }

    /// The Node took static Zcash actor templates at start until Zcash swaps
    /// were agreed at take time only. A command line or a configuration that
    /// still names the flag is refused at start, by the flag's name.
    #[cfg(feature = "pair-zec")]
    #[test]
    fn the_removed_static_zec_template_flag_is_refused_by_name() {
        let mut arguments = chat_arguments();
        arguments.extend([
            "--btc-maker-signing-key-file",
            "/tmp/btc.key",
            "--btc-role-config",
            "/tmp/btc-role.json",
            "--zec-role-config",
            "/tmp/zec-role.json",
            "--zec-maker-actor-root",
            "/tmp/actors",
            "--zec-actor-program",
            "/usr/bin/true",
            "--zec-actor-program-sha256",
            "00f5ba0b54bda7a48c9380b740fc38a8710a23c0f3f948cdcc9b7d5e712d1f8f",
        ]);
        Arguments::try_parse_from(arguments.clone()).expect("the take-time deployment parses");
        arguments.extend(["--zec-source-maker-config", "/tmp/actor.json"]);

        let Err(refused) = Arguments::try_parse_from(arguments.clone()) else {
            panic!("the removed flag must be refused on the command line");
        };
        assert_eq!(refused.kind(), ErrorKind::UnknownArgument, "{refused}");
        assert!(
            refused.to_string().contains("'--zec-source-maker-config'"),
            "{refused}"
        );

        let configuration = serde_json::json!({
            "schema_version": 1,
            "arguments": &arguments[1..],
        });
        let Err(refused) = parse_daemon_config(
            configuration.to_string().as_bytes(),
            OsString::from("lez-maker-node"),
        ) else {
            panic!("the removed flag must be refused in a configuration");
        };
        assert!(
            format!("{refused:#}").contains("'--zec-source-maker-config'"),
            "{refused:#}"
        );
    }

    #[cfg(feature = "pair-xmr")]
    #[test]
    fn chat_cli_requires_complete_xmr_authority() {
        let complete = vec![
            "lez-maker-node",
            "--database",
            "/tmp/maker.sqlite3",
            "--delivery-directory",
            "/tmp/delivery",
            "--delivery-signing-key-file",
            "/tmp/delivery.key",
            "--chat-socket",
            "/tmp/chat.sock",
            "--btc-maker-signing-key-file",
            "/tmp/btc.key",
            "--btc-role-config",
            "/tmp/btc-role.json",
            "--xmr-role-config",
            "/tmp/xmr-role.json",
            "--xmr-maker-actor-root",
            "/tmp/xmr-actors",
            "--xmr-actor-program",
            "/usr/bin/true",
            "--xmr-actor-program-sha256",
            "00f5ba0b54bda7a48c9380b740fc38a8710a23c0f3f948cdcc9b7d5e712d1f8f",
        ];
        Arguments::try_parse_from(complete.clone()).expect("complete XMR Chat authority parses");

        for omitted in [
            "--btc-role-config",
            "--xmr-maker-actor-root",
            "--xmr-actor-program",
            "--xmr-actor-program-sha256",
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

    #[test]
    fn strict_daemon_config_parses_only_schema_one_clap_arguments() {
        parse_daemon_config(
            br#"{"schema_version":1,"arguments":["--database","/tmp/maker.sqlite3"]}"#,
            OsString::from("lez-maker-node"),
        )
        .expect("strict schema-one configuration parses");
        for invalid in [
            br#"{"schema_version":2,"arguments":["--database","/tmp/maker.sqlite3"]}"#.as_slice(),
            br#"{"schema_version":1,"arguments":["--config","recursive.json"]}"#.as_slice(),
            br#"{"schema_version":1,"arguments":["--unknown"]}"#.as_slice(),
            br#"{"schema_version":1,"arguments":[],"extra":true}"#.as_slice(),
        ] {
            assert!(
                parse_daemon_config(invalid, OsString::from("lez-maker-node")).is_err(),
                "invalid or recursive daemon configuration must fail closed"
            );
        }
    }
}
