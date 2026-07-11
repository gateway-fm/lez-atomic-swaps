use clap::{Parser, Subcommand, ValueEnum};
use jsonrpsee::{core::client::ClientT, rpc_params};
use jsonrpsee_http_client::{HeaderMap, HeaderValue, HttpClientBuilder};
use lez_maker_node::{CreateSwapRequest, StatusRequest, SwapView};
use lez_swap_core::{Pair, SwapDirection};

#[derive(Parser)]
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
        #[arg(long, value_enum, default_value_t = DirectionArgument::TakerSellsForeign)]
        direction: DirectionArgument,
        #[arg(long)]
        confirmations: u32,
        #[arg(long)]
        maker_refund_at: u64,
        #[arg(long)]
        taker_refund_at: u64,
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

#[derive(Debug, Default, Clone, Copy, ValueEnum)]
enum DirectionArgument {
    #[default]
    TakerSellsForeign,
    TakerSellsLez,
}

impl From<DirectionArgument> for SwapDirection {
    fn from(direction: DirectionArgument) -> Self {
        match direction {
            DirectionArgument::TakerSellsForeign => Self::TakerSellsForeign,
            DirectionArgument::TakerSellsLez => Self::TakerSellsLez,
        }
    }
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
    let mut headers = HeaderMap::new();
    let mut authorization = HeaderValue::from_str(&format!("Bearer {}", arguments.rpc_token))?;
    authorization.set_sensitive(true);
    headers.insert("authorization", authorization);
    let client = HttpClientBuilder::default()
        .set_headers(headers)
        .build(&arguments.rpc_url)?;
    let view: SwapView = match arguments.command {
        Command::CreateSwap {
            id,
            pair,
            direction,
            confirmations,
            maker_refund_at,
            taker_refund_at,
        } => {
            let request = CreateSwapRequest {
                id: id.into(),
                pair: pair.into(),
                direction: direction.into(),
                confirmations,
                maker_refund_at,
                taker_refund_at,
            };
            client.request("swap_create", rpc_params![request]).await?
        }
        Command::Status { id } => {
            let request = StatusRequest { id: id.into() };
            client.request("swap_status", rpc_params![request]).await?
        }
    };
    println!("{}", serde_json::to_string(&view)?);
    Ok(())
}
