use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context as _, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use jsonrpsee::server::{ServerBuilder, serve_with_graceful_shutdown, stop_channel};
use lez_node_common::{
    LogosChatGateway, LogosChatGatewayBindRequestV1, LogosChatGatewayIngestRequestV1,
    LogosChatGatewayOutboxAckRequestV1, LogosChatGatewayOutboxItemV1,
    LogosChatGatewayOutboxRequestV1, LogosChatGatewayRoleV1, call_local_chat_gateway_rpc,
    logos_chat_gateway_control_rpc_module, logos_chat_gateway_proxy_rpc_module,
    owner_rpc_server::{OwnedPath, bind_owner_socket, server_config},
    shutdown_signal,
};
use tokio::{net::UnixListener, task::JoinSet};

const MAXIMUM_CHAT_PROXY_RPC_BODY_BYTES: u32 = 1024 * 1024;
const MAXIMUM_GATEWAY_CONTROL_RPC_BODY_BYTES: u32 = 4 * 1024 * 1024;
const LOCAL_RELAY_SCHEMA_VERSION: u16 = 1;

#[derive(Parser)]
#[command(about = "Session-scoped Logos Chat RPC gateway and offline local relay")]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run one Maker or Taker gateway endpoint.
    Endpoint {
        /// Required by the generic compatibility binary and fixed by canonical role binaries.
        #[arg(long, value_enum)]
        role: Option<Role>,
        #[arg(long)]
        control_socket: PathBuf,
        #[arg(long, required_if_eq("role", "taker"))]
        proxy_socket: Option<PathBuf>,
        #[arg(long, required_if_eq("role", "maker"))]
        maker_chat_socket: Option<PathBuf>,
    },
    /// Relay gateway outboxes through an isolated Unix-only test network.
    LocalRelay {
        #[arg(long)]
        maker_control_socket: PathBuf,
        #[arg(long)]
        taker_control_socket: PathBuf,
        #[arg(long, default_value_t = 5)]
        poll_milliseconds: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Role {
    Maker,
    Taker,
}

impl From<Role> for LogosChatGatewayRoleV1 {
    fn from(role: Role) -> Self {
        match role {
            Role::Maker => Self::Maker,
            Role::Taker => Self::Taker,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Arguments::parse().command {
        Command::Endpoint {
            role,
            control_socket,
            proxy_socket,
            maker_chat_socket,
        } => {
            let fixed_role = fixed_role_from_executable();
            let role = resolve_endpoint_role(fixed_role, role)?;
            run_endpoint(role.into(), control_socket, proxy_socket, maker_chat_socket).await
        }
        Command::LocalRelay {
            maker_control_socket,
            taker_control_socket,
            poll_milliseconds,
        } => {
            ensure!(
                fixed_role_from_executable().is_none(),
                "role-fixed Chat gateways cannot run the development relay"
            );
            run_local_relay(
                &maker_control_socket,
                &taker_control_socket,
                Duration::from_millis(poll_milliseconds),
            )
            .await
        }
    }
}

fn resolve_endpoint_role(fixed: Option<Role>, configured: Option<Role>) -> anyhow::Result<Role> {
    match (fixed, configured) {
        (Some(fixed), Some(configured)) => {
            ensure!(
                fixed == configured,
                "role-fixed Chat gateway rejects the opposite role"
            );
            Ok(fixed)
        }
        (Some(fixed), None) => Ok(fixed),
        (None, Some(configured)) => Ok(configured),
        (None, None) => {
            anyhow::bail!("generic Chat gateway requires --role; prefer a role-fixed entrypoint")
        }
    }
}

fn fixed_role_from_executable() -> Option<Role> {
    match std::env::args_os()
        .next()
        .as_deref()
        .and_then(|value| std::path::Path::new(value).file_name())
        .and_then(std::ffi::OsStr::to_str)
    {
        Some("lez-maker-chat-gateway") => Some(Role::Maker),
        Some("lez-taker-chat-gateway") => Some(Role::Taker),
        _ => None,
    }
}

async fn run_endpoint(
    role: LogosChatGatewayRoleV1,
    control_socket: PathBuf,
    proxy_socket: Option<PathBuf>,
    maker_chat_socket: Option<PathBuf>,
) -> anyhow::Result<()> {
    ensure!(
        control_socket.is_absolute(),
        "control socket must be absolute"
    );
    let gateway = Arc::new(
        LogosChatGateway::new(role, maker_chat_socket)
            .context("validate role-fixed Logos Chat gateway")?,
    );
    let control_module = logos_chat_gateway_control_rpc_module(Arc::clone(&gateway))
        .context("build Logos Chat gateway control RPC")?;
    let (control_listener, control_guard) =
        bind_owner_socket(&control_socket).context("bind gateway control socket")?;
    match role {
        LogosChatGatewayRoleV1::Maker => {
            ensure!(
                proxy_socket.is_none(),
                "Maker gateway must not expose a Taker proxy"
            );
            serve_one(control_listener, control_guard, control_module).await
        }
        LogosChatGatewayRoleV1::Taker => {
            let proxy_socket = proxy_socket.context("Taker gateway requires --proxy-socket")?;
            ensure!(proxy_socket.is_absolute(), "proxy socket must be absolute");
            let proxy_module = logos_chat_gateway_proxy_rpc_module(gateway)
                .context("build Logos Chat Taker proxy RPC")?;
            let (proxy_listener, proxy_guard) =
                bind_owner_socket(&proxy_socket).context("bind gateway proxy socket")?;
            serve_two(
                control_listener,
                control_guard,
                control_module,
                proxy_listener,
                proxy_guard,
                proxy_module,
            )
            .await
        }
    }
}

async fn serve_one<Context>(
    listener: UnixListener,
    _guard: OwnedPath,
    module: jsonrpsee::RpcModule<Context>,
) -> anyhow::Result<()>
where
    Context: Send + Sync + 'static,
{
    let (stop_handle, server_handle) = stop_channel();
    let service = ServerBuilder::default()
        .set_config(server_config(MAXIMUM_GATEWAY_CONTROL_RPC_BODY_BYTES))
        .to_service_builder()
        .build(module, stop_handle.clone());
    let mut connections = JoinSet::new();
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut service_error = None;
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let connection_service = service.clone();
                        let connection_stop = stop_handle.clone();
                        connections.spawn(async move {
                            serve_with_graceful_shutdown(
                                stream,
                                connection_service,
                                connection_stop.shutdown(),
                            ).await
                        });
                    }
                    Err(error) => {
                        service_error = Some(anyhow::Error::new(error).context("accept gateway control connection"));
                        break;
                    }
                }
            }
            completed = connections.join_next(), if !connections.is_empty() => {
                let _connection_result =
                    finish_connection(completed.expect("active gateway connection"));
            }
            signal = &mut shutdown => {
                if let Err(error) = signal {
                    service_error = Some(anyhow::Error::new(error).context("wait for gateway shutdown"));
                }
                break;
            }
        }
    }
    server_handle.stop().context("stop gateway RPC")?;
    while let Some(completed) = connections.join_next().await {
        let _connection_result = finish_connection(completed);
    }
    drop(service);
    drop(stop_handle);
    server_handle.stopped().await;
    match service_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve_two<LeftContext, RightContext>(
    left_listener: UnixListener,
    _left_guard: OwnedPath,
    left_module: jsonrpsee::RpcModule<LeftContext>,
    right_listener: UnixListener,
    _right_guard: OwnedPath,
    right_module: jsonrpsee::RpcModule<RightContext>,
) -> anyhow::Result<()>
where
    LeftContext: Send + Sync + 'static,
    RightContext: Send + Sync + 'static,
{
    let (stop_handle, server_handle) = stop_channel();
    let left_service = ServerBuilder::default()
        .set_config(server_config(MAXIMUM_GATEWAY_CONTROL_RPC_BODY_BYTES))
        .to_service_builder()
        .build(left_module, stop_handle.clone());
    let right_service = ServerBuilder::default()
        .set_config(server_config(MAXIMUM_CHAT_PROXY_RPC_BODY_BYTES))
        .to_service_builder()
        .build(right_module, stop_handle.clone());
    let mut connections = JoinSet::new();
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut service_error = None;
    loop {
        tokio::select! {
            accepted = left_listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let service = left_service.clone();
                        let stop = stop_handle.clone();
                        connections.spawn(async move {
                            serve_with_graceful_shutdown(stream, service, stop.shutdown()).await
                        });
                    }
                    Err(error) => {
                        service_error = Some(anyhow::Error::new(error).context("accept gateway control connection"));
                        break;
                    }
                }
            }
            accepted = right_listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let service = right_service.clone();
                        let stop = stop_handle.clone();
                        connections.spawn(async move {
                            serve_with_graceful_shutdown(stream, service, stop.shutdown()).await
                        });
                    }
                    Err(error) => {
                        service_error = Some(anyhow::Error::new(error).context("accept gateway proxy connection"));
                        break;
                    }
                }
            }
            completed = connections.join_next(), if !connections.is_empty() => {
                let _connection_result =
                    finish_connection(completed.expect("active gateway connection"));
            }
            signal = &mut shutdown => {
                if let Err(error) = signal {
                    service_error = Some(anyhow::Error::new(error).context("wait for gateway shutdown"));
                }
                break;
            }
        }
    }
    server_handle.stop().context("stop gateway RPC")?;
    while let Some(completed) = connections.join_next().await {
        let _connection_result = finish_connection(completed);
    }
    drop(left_service);
    drop(right_service);
    drop(stop_handle);
    server_handle.stopped().await;
    match service_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn run_local_relay(
    maker_control_socket: &std::path::Path,
    taker_control_socket: &std::path::Path,
    poll_interval: Duration,
) -> anyhow::Result<()> {
    ensure!(
        !poll_interval.is_zero() && poll_interval <= Duration::from_secs(1),
        "local relay polling interval must be 1..=1000 milliseconds"
    );
    bind_local_session(
        maker_control_socket,
        "local-e2e-conversation-v1",
        "local://maker",
        "local://taker",
    )
    .await?;
    bind_local_session(
        taker_control_socket,
        "local-e2e-conversation-v1",
        "local://taker",
        "local://maker",
    )
    .await?;
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            result = &mut shutdown => return result.context("wait for local relay shutdown"),
            () = tokio::time::sleep(poll_interval) => {
                let maker_sent = transfer_one(
                    maker_control_socket,
                    taker_control_socket,
                    "local://maker",
                ).await.unwrap_or(false);
                let taker_sent = transfer_one(
                    taker_control_socket,
                    maker_control_socket,
                    "local://taker",
                ).await.unwrap_or(false);
                if maker_sent || taker_sent {
                    tokio::task::yield_now().await;
                }
            }
        }
    }
}

async fn bind_local_session(
    socket: &std::path::Path,
    conversation_id: &str,
    local_address: &str,
    peer_address: &str,
) -> anyhow::Result<()> {
    let _: lez_node_common::LogosChatGatewayAckV1 = call_local_chat_gateway_rpc(
        socket,
        "logos_chat_bind_session_v1",
        &LogosChatGatewayBindRequestV1 {
            schema_version: LOCAL_RELAY_SCHEMA_VERSION,
            conversation_id: conversation_id.into(),
            local_address: local_address.into(),
            peer_address: peer_address.into(),
        },
    )
    .await?;
    Ok(())
}

async fn transfer_one(
    source: &std::path::Path,
    target: &std::path::Path,
    sender_address: &str,
) -> anyhow::Result<bool> {
    let frame: Option<LogosChatGatewayOutboxItemV1> = call_local_chat_gateway_rpc(
        source,
        "logos_chat_outbox_peek_v1",
        &LogosChatGatewayOutboxRequestV1 {
            schema_version: LOCAL_RELAY_SCHEMA_VERSION,
        },
    )
    .await?;
    let Some(frame) = frame else {
        return Ok(false);
    };
    let _: lez_node_common::LogosChatGatewayAckV1 = call_local_chat_gateway_rpc(
        target,
        "logos_chat_ingest_v1",
        &LogosChatGatewayIngestRequestV1 {
            schema_version: LOCAL_RELAY_SCHEMA_VERSION,
            conversation_id: frame.conversation_id.clone(),
            sender_address: sender_address.into(),
            content: frame.content,
        },
    )
    .await?;
    let _: lez_node_common::LogosChatGatewayAckV1 = call_local_chat_gateway_rpc(
        source,
        "logos_chat_outbox_ack_v1",
        &LogosChatGatewayOutboxAckRequestV1 {
            schema_version: LOCAL_RELAY_SCHEMA_VERSION,
            frame_id: frame.frame_id,
            conversation_id: frame.conversation_id,
        },
    )
    .await?;
    Ok(true)
}

fn finish_connection(
    completed: Result<Result<(), jsonrpsee::core::BoxError>, tokio::task::JoinError>,
) -> anyhow::Result<()> {
    completed
        .context("join gateway RPC connection")?
        .map_err(|error| anyhow::anyhow!("serve gateway RPC connection: {error}"))
}


#[cfg(test)]
mod role_tests {
    use super::{Role, resolve_endpoint_role};

    #[test]
    fn role_fixed_entrypoints_reject_the_opposite_role() {
        assert_eq!(
            resolve_endpoint_role(Some(Role::Maker), None).unwrap(),
            Role::Maker
        );
        assert!(resolve_endpoint_role(Some(Role::Maker), Some(Role::Taker)).is_err());
        assert!(resolve_endpoint_role(None, None).is_err());
    }
}
