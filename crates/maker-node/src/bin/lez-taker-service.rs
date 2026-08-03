use std::{io, path::PathBuf};

use anyhow::{Context as _, ensure};
use clap::Parser;
use jsonrpsee::server::{ServerBuilder, serve_with_graceful_shutdown, stop_channel};
use lez_maker_node::{
    load_taker_service_backend,
    owner_rpc_server::{bind_owner_socket, server_config},
    taker_read_only_rpc_module,
};
use tokio::task::JoinSet;

const MAXIMUM_RPC_BODY_BYTES: u32 = 64 * 1024;

#[derive(Parser)]
#[command(about = "Owner-local read-only LEZ atomic-swap Taker service")]
struct Arguments {
    /// Owner-private startup configuration.
    #[arg(long)]
    config: PathBuf,
    /// Distinct owner-only Unix-domain Taker service socket.
    #[arg(long, default_value = "/run/lez-atomic-swaps/taker.sock")]
    socket: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    ensure!(
        arguments.socket.is_absolute(),
        "Taker service socket path must be absolute"
    );

    let backend = load_taker_service_backend(&arguments.config)
        .context("load Taker service startup configuration")?;
    let module = taker_read_only_rpc_module(backend).context("build read-only Taker RPC module")?;
    let (listener, _socket_guard) =
        bind_owner_socket(&arguments.socket).context("bind Taker service socket")?;

    let (stop_handle, server_handle) = stop_channel();
    let service = ServerBuilder::default()
        .set_config(server_config(MAXIMUM_RPC_BODY_BYTES))
        .to_service_builder()
        .build(module, stop_handle.clone());
    let mut connections = JoinSet::new();
    let mut service_error = None;
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("accept Taker RPC connection")?;
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
            completed = connections.join_next(), if !connections.is_empty() => {
                let completed = completed.expect("active Taker connection task is present");
                if let Err(error) = finish_connection(completed) {
                    service_error = Some(error);
                    break;
                }
            }
            signal = &mut shutdown => {
                if let Err(error) = signal {
                    service_error = Some(anyhow::Error::new(error).context("wait for shutdown"));
                }
                break;
            }
        }
    }

    server_handle.stop().context("stop Taker RPC")?;
    while let Some(completed) = connections.join_next().await {
        finish_connection(completed)?;
    }
    drop(service);
    drop(stop_handle);
    server_handle.stopped().await;

    match service_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn finish_connection(
    completed: Result<Result<(), jsonrpsee::core::BoxError>, tokio::task::JoinError>,
) -> anyhow::Result<()> {
    completed
        .context("join Taker RPC connection")?
        .map_err(|error| anyhow::anyhow!("serve Taker RPC connection: {error}"))
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
