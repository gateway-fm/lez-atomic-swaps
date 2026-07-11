use clap::{Parser, Subcommand, ValueEnum};
use jsonrpsee::{core::client::ClientT, rpc_params};
use jsonrpsee_http_client::HttpClientBuilder;
use lez_maker_node::{CreateSwapRequest, StatusRequest, SwapView};
use lez_swap_core::Pair;

#[derive(Debug, Parser)]
#[command(about = "Operator CLI for the LEZ atomic-swap maker daemon")]
struct Arguments {
    #[arg(long, default_value = "http://127.0.0.1:9944")]
    rpc_url: String,
    #[arg(long, env = "LEZ_MAKER_RPC_TOKEN", hide_env_values = true)]
    rpc_token: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    CreateSwap {
        #[arg(long)]
        id: String,
        #[arg(long)]
        pair: PairArgument,
        #[arg(long)]
        confirmations: u32,
        #[arg(long)]
        lez_refund_at: u64,
        #[arg(long)]
        foreign_refund_at: u64,
    },
    Status {
        #[arg(long)]
        id: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PairArgument {
    Bitcoin,
    Monero,
    Zcash,
}

impl From<PairArgument> for Pair {
    fn from(pair: PairArgument) -> Self {
        match pair {
            PairArgument::Bitcoin => Self::Bitcoin,
            PairArgument::Monero => Self::Monero,
            PairArgument::Zcash => Self::Zcash,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    let client = HttpClientBuilder::default().build(&arguments.rpc_url)?;
    let view: SwapView = match arguments.command {
        Command::CreateSwap {
            id,
            pair,
            confirmations,
            lez_refund_at,
            foreign_refund_at,
        } => {
            let request = CreateSwapRequest {
                capability: arguments.rpc_token.into(),
                id: id.into(),
                pair: pair.into(),
                confirmations,
                lez_refund_at,
                foreign_refund_at,
            };
            client.request("swap_create", rpc_params![request]).await?
        }
        Command::Status { id } => {
            let request = StatusRequest {
                capability: arguments.rpc_token.into(),
                id: id.into(),
            };
            client.request("swap_status", rpc_params![request]).await?
        }
    };
    println!("{}", serde_json::to_string(&view)?);
    Ok(())
}
