use clap::{Parser, Subcommand, ValueEnum};
use jsonrpsee::{core::client::ClientT, rpc_params};
use jsonrpsee_http_client::{HeaderMap, HeaderValue, HttpClientBuilder};
use lez_maker_node::{CreateSwapRequest, RecoveryRequest, StatusRequest, SwapView};
use lez_swap_core::{ClockBasis, Pair, SwapDirection};

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
        #[arg(long, value_enum)]
        maker_refund_basis: Option<ClockArgument>,
        #[arg(long)]
        maker_refund_at: Option<u64>,
        #[arg(long, value_enum, default_value_t = ClockArgument::BlockHeight)]
        taker_refund_basis: ClockArgument,
        #[arg(long)]
        taker_refund_at: u64,
        #[arg(long)]
        earlier_refund_latest: Option<u64>,
        #[arg(long)]
        later_refund_earliest: Option<u64>,
        #[arg(long)]
        required_margin: Option<u64>,
        #[arg(long)]
        xmr_refund_event_confirmations: Option<u32>,
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

#[derive(Debug, Default, Clone, Copy, ValueEnum)]
enum ClockArgument {
    #[default]
    BlockHeight,
    Timestamp,
}

impl From<ClockArgument> for ClockBasis {
    fn from(basis: ClockArgument) -> Self {
        match basis {
            ClockArgument::BlockHeight => Self::BlockHeight,
            ClockArgument::Timestamp => Self::Timestamp,
        }
    }
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
            maker_refund_basis,
            maker_refund_at,
            taker_refund_basis,
            taker_refund_at,
            earlier_refund_latest,
            later_refund_earliest,
            required_margin,
            xmr_refund_event_confirmations,
        } => {
            let pair: Pair = pair.into();
            let direction: SwapDirection = direction.into();
            let recovery = if pair == Pair::Monero {
                anyhow::ensure!(
                    direction == SwapDirection::TakerSellsLez,
                    "Monero does not support direction {direction:?}"
                );
                anyhow::ensure!(
                    maker_refund_basis.is_none()
                        && maker_refund_at.is_none()
                        && earlier_refund_latest.is_none()
                        && later_refund_earliest.is_none()
                        && required_margin.is_none(),
                    "Monero recovery is event-gated and does not accept maker deadline fields"
                );
                RecoveryRequest::XmrLezFirst {
                    taker_refund_basis: taker_refund_basis.into(),
                    taker_refund_at,
                    refund_event_confirmations: xmr_refund_event_confirmations.unwrap_or(2),
                }
            } else {
                anyhow::ensure!(
                    xmr_refund_event_confirmations.is_none(),
                    "XMR refund-event confirmations apply only to Monero"
                );
                RecoveryRequest::Deadlines {
                    maker_refund_basis: maker_refund_basis
                        .unwrap_or(ClockArgument::BlockHeight)
                        .into(),
                    maker_refund_at: maker_refund_at
                        .ok_or_else(|| anyhow::anyhow!("--maker-refund-at is required"))?,
                    taker_refund_basis: taker_refund_basis.into(),
                    taker_refund_at,
                    earlier_refund_latest: earlier_refund_latest
                        .ok_or_else(|| anyhow::anyhow!("--earlier-refund-latest is required"))?,
                    later_refund_earliest: later_refund_earliest
                        .ok_or_else(|| anyhow::anyhow!("--later-refund-earliest is required"))?,
                    required_margin: required_margin
                        .ok_or_else(|| anyhow::anyhow!("--required-margin is required"))?,
                }
            };
            let request = CreateSwapRequest {
                id: id.into(),
                pair,
                direction,
                confirmations,
                recovery,
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
