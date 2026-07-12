use clap::{Parser, Subcommand, ValueEnum};
use jsonrpsee::{core::client::ClientT, rpc_params};
use jsonrpsee_http_client::{HeaderMap, HeaderValue, HttpClientBuilder};
use lez_maker_node::{
    AlertAcknowledgeRequest, AlertListRequest, CreateSwapRequest, OperatorAlertView,
    RecoveryRequest, StatusRequest, SwapView,
};
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
    Alerts {
        #[arg(long)]
        id: String,
        #[arg(long, default_value_t = 0)]
        after: u64,
        #[arg(long)]
        all: bool,
    },
    AcknowledgeAlert {
        #[arg(long)]
        id: String,
        #[arg(long = "alert")]
        alert_sequence: u64,
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
    let client = rpc_client(&arguments.rpc_url, &arguments.rpc_token)?;
    let output = match arguments.command {
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
            let view: SwapView = client.request("swap_create", rpc_params![request]).await?;
            serde_json::to_value(view)?
        }
        Command::Status { id } => {
            let request = StatusRequest { id: id.into() };
            let view: SwapView = client.request("swap_status", rpc_params![request]).await?;
            serde_json::to_value(view)?
        }
        Command::Alerts { id, after, all } => {
            let request = AlertListRequest {
                id: id.into(),
                after_sequence: after,
                include_acknowledged: all,
            };
            let alerts: Vec<OperatorAlertView> =
                client.request("swap_alerts", rpc_params![request]).await?;
            serde_json::to_value(alerts)?
        }
        Command::AcknowledgeAlert { id, alert_sequence } => {
            let request = AlertAcknowledgeRequest {
                id: id.into(),
                alert_sequence,
            };
            let view: SwapView = client
                .request("swap_alert_acknowledge", rpc_params![request])
                .await?;
            serde_json::to_value(view)?
        }
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

fn rpc_client(rpc_url: &str, rpc_token: &str) -> anyhow::Result<jsonrpsee_http_client::HttpClient> {
    let mut headers = HeaderMap::new();
    let mut authorization = HeaderValue::from_str(&format!("Bearer {rpc_token}"))?;
    authorization.set_sensitive(true);
    headers.insert("authorization", authorization);
    HttpClientBuilder::default()
        .set_headers(headers)
        .build(rpc_url)
        .map_err(Into::into)
}
