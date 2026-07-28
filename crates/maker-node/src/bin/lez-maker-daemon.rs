use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Write as _},
    os::unix::fs::{FileTypeExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, bail, ensure};
use clap::Parser;
use jsonrpsee::server::{
    BatchRequestConfig, ServerBuilder, ServerConfig, serve_with_graceful_shutdown, stop_channel,
};
use lez_maker_node::{
    MakerRpc, RunLocalDelivery, chat_rpc_module, import_terminal_zec_maker_projection, rpc_module,
};
use lez_swap_core::{Participant, SwapId};
use lez_swap_store::{SqliteSwapStore, SqliteZecRecoveryStore};
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
    #[arg(long, requires_all = ["delivery_directory", "delivery_signing_key_file"])]
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    let _state_lease = acquire_state_lease(&arguments.database)?;
    import_terminal_projection(&arguments).await?;
    let (listener, _socket_guard) = bind_owner_socket(&arguments.socket)?;
    let (chat_listener, _chat_socket_guard) =
        bind_optional_owner_socket(arguments.chat_socket.as_deref())?;
    let context = maker_context(
        &arguments.database,
        arguments.delivery_directory.as_deref(),
        arguments.delivery_signing_key_file.as_deref(),
        arguments.maker_claim_key_id.as_deref(),
        arguments.maker_claim_key_file.as_deref(),
        arguments.maker_claim_preimage_file.as_deref(),
    )?;
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
            signal = &mut shutdown => {
                signal.context("wait for shutdown")?;
                notify_stopping();
                break;
            }
        }
    }

    server_handle.stop().context("stop maker RPC")?;
    while let Some(connection) = connections.join_next().await {
        connection
            .context("join local RPC connection")?
            .map_err(|error| anyhow::anyhow!("serve local RPC connection: {error}"))?;
    }
    drop(service);
    drop(chat_service);
    drop(stop_handle);
    server_handle.stopped().await;
    Ok(())
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

fn maker_context(
    database: &Path,
    delivery_directory: Option<&Path>,
    delivery_signing_key_file: Option<&Path>,
    maker_claim_key_id: Option<&str>,
    maker_claim_key_file: Option<&Path>,
    maker_claim_preimage_file: Option<&Path>,
) -> anyhow::Result<MakerRpc> {
    let store = SqliteSwapStore::open(database).context("open maker database")?;
    let configured = (
        delivery_directory,
        delivery_signing_key_file,
        maker_claim_key_id,
        maker_claim_key_file,
        maker_claim_preimage_file,
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
        return Ok(MakerRpc::new(store));
    };
    let signing_key = load_delivery_key(signing_file)?;
    let claim_key_material = load_raw_secret(claim_key_file, "maker claim-recovery key")?;
    let claim_key = ProtectedClaimKey::new(claim_key_id, *claim_key_material)
        .context("validate maker claim-recovery key ID")?;
    let preimage_material = load_raw_secret(preimage_file, "maker claim preimage")?;
    let preimage = ClaimPreimage::new(*preimage_material);
    let recovery_store =
        SqliteZecRecoveryStore::open_claim_capable(database, Participant::Maker, claim_key)
            .context("open maker ZEC recovery store")?;
    let delivery = RunLocalDelivery::publisher(directory.to_path_buf(), signing_key)
        .context("open maker Delivery publisher")?;
    let now_unix_seconds = trusted_now_unix_seconds()?;
    let active = store
        .list_discoverable_maker_offers(now_unix_seconds)
        .context("load active offers for Delivery reconciliation")?
        .into_iter()
        .map(|record| record.offer().clone())
        .collect::<Vec<_>>();
    delivery
        .reconcile(&active, now_unix_seconds)
        .context("reconcile Delivery advertisements")?;
    Ok(MakerRpc::with_delivery(
        store,
        delivery,
        signing_key,
        recovery_store,
        preimage,
    ))
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
