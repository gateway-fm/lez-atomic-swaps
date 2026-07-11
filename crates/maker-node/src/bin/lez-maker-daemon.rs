use std::{net::SocketAddr, path::PathBuf};

use anyhow::Context;
use clap::Parser;
use jsonrpsee::server::ServerBuilder;
use lez_maker_node::{MakerRpc, rpc_module, validate_capability};
use lez_swap_store::SqliteSwapStore;
use tower::ServiceBuilder;
use tower_http::validate_request::ValidateRequestHeaderLayer;

#[derive(Parser)]
#[command(about = "Headless LEZ atomic-swap maker daemon")]
struct Arguments {
    #[arg(long, default_value = "127.0.0.1:0")]
    listen: SocketAddr,
    #[arg(long)]
    database: PathBuf,
    #[arg(long, env = "LEZ_MAKER_RPC_TOKEN", hide_env_values = true)]
    rpc_token: String,
    /// Test/service-manager readiness handoff; contains only the loopback URL.
    #[arg(long)]
    ready_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    anyhow::ensure!(
        arguments.listen.ip().is_loopback(),
        "maker RPC must bind to a loopback address"
    );
    validate_capability(&arguments.rpc_token)?;
    let store = SqliteSwapStore::open(&arguments.database).context("open maker database")?;
    let module = rpc_module(MakerRpc::new(store))?;
    // The opaque capability is an intentionally small interim auth scheme for the loopback-only
    // adapter. ADR 0007 gates production on an owner-restricted local transport and credential.
    #[allow(deprecated)]
    let authentication =
        ServiceBuilder::new().layer(ValidateRequestHeaderLayer::bearer(&arguments.rpc_token));
    let server = ServerBuilder::default()
        .set_http_middleware(authentication)
        .build(arguments.listen)
        .await
        .context("bind maker RPC")?;
    let local_address = server.local_addr().context("read maker RPC address")?;
    let handle = server.start(module);

    if let Some(ready_file) = arguments.ready_file {
        std::fs::write(ready_file, format!("http://{local_address}"))
            .context("write maker readiness file")?;
    }

    tokio::signal::ctrl_c().await.context("wait for shutdown")?;
    handle.stop().context("stop maker RPC")?;
    handle.stopped().await;
    Ok(())
}
