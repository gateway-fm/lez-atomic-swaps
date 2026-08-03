use std::path::{Path, PathBuf};

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    AlertAcknowledgeRequest, AlertListRequest, CreateSwapRequest, ListRequest,
    LocalPriceSetRequest, MakerActorActionCommitV1, MakerActorActionRequestV1,
    MakerActorMonitorRequestV1, MakerActorMonitorV1, MakerHealthV1, MakerServiceAction,
    OfferPublishRequest, OfferWithdrawRequest, OperatorAlertView, PairConfigureRequest,
    PriceQuoteRequest, PriceQuoteV1, RecoveryRequest, StatusRequest, SwapView, call_local_rpc,
    control_maker_service, secure_file::load_secp256k1_secret,
};
use lez_swap_core::{ClockBasis, Pair, SwapDirection};
use lez_swap_store::{
    LocalPriceV1, MakerConfigurationCommit, MakerOfferCommit, MakerOfferId, MakerOfferRecordV1,
    MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1, VersionedMakerRecord,
};
use secp256k1::{PublicKey, Secp256k1};
use serde::Serialize;

#[derive(Parser)]
#[command(about = "Operator CLI for the LEZ atomic-swap maker daemon")]
struct Arguments {
    #[arg(long, default_value = "/run/lez-atomic-swaps/maker.sock")]
    socket: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Derives the public Delivery identity without contacting the Maker daemon.
    DeliveryIdentity {
        /// Existing owner-private raw or hexadecimal secp256k1 signing key.
        #[arg(long, value_name = "PRIVATE_KEY")]
        signing_key_file: PathBuf,
    },
    /// Starts only the packaged lez-maker-daemon.service through systemd.
    Start,
    /// Stops only the packaged lez-maker-daemon.service through systemd.
    Stop,
    ConfigurePair {
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        expected_revision: Option<u64>,
        #[arg(long)]
        pair: PairArgument,
        #[arg(long, value_enum, default_value_t = DirectionArgument::TakerSellsForeign)]
        direction: DirectionArgument,
        #[arg(long, action = ArgAction::Set, required = true)]
        enabled: bool,
        #[arg(long, value_enum, default_value_t = PriceSourceArgument::Local)]
        price_source: PriceSourceArgument,
        #[arg(long)]
        minimum_foreign_units: u64,
        #[arg(long)]
        maximum_foreign_units: u64,
        #[arg(long)]
        offer_ttl_seconds: u64,
    },
    SetLocalPrice {
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        expected_revision: Option<u64>,
        #[arg(long)]
        pair: PairArgument,
        #[arg(long, value_enum, default_value_t = DirectionArgument::TakerSellsForeign)]
        direction: DirectionArgument,
        #[arg(long)]
        lez_units_per_lot: u64,
        #[arg(long)]
        foreign_units_per_lot: u64,
    },
    /// Lists durable pair policies in stable route order.
    Pairs,
    /// Lists durable local prices in stable route order.
    Prices,
    /// Probes the daemon and its `SQLite` reader through owner-local RPC.
    Health,
    /// Reads one exact route quote through the configured runtime boundary.
    Quote {
        #[arg(long)]
        pair: PairArgument,
        #[arg(long, value_enum, default_value_t = DirectionArgument::TakerSellsForeign)]
        direction: DirectionArgument,
    },
    /// Publishes one expiring offer from the current enabled local route.
    PublishOffer {
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        offer_id: String,
        #[arg(long)]
        pair: PairArgument,
        #[arg(long, value_enum, default_value_t = DirectionArgument::TakerSellsForeign)]
        direction: DirectionArgument,
    },
    /// Lists complete durable offer history in stable identity order.
    Offers,
    /// Withdraws one active unreserved offer.
    WithdrawOffer {
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        offer_id: String,
        #[arg(long)]
        expected_revision: u64,
    },
    /// Lists durable swap summaries in stable identifier order.
    History,
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
    /// Reads one allowlisted durable Maker actor lifecycle snapshot.
    Monitor {
        #[arg(long)]
        id: String,
    },
    /// Queues one replay-safe Zcash Maker claim action.
    Claim {
        #[arg(long)]
        id: String,
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        expected_generation: u64,
    },
    /// Queues one replay-safe Maker timeout-recovery action.
    Refund {
        #[arg(long)]
        id: String,
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        expected_generation: u64,
    },
}

#[derive(Serialize)]
struct DeliveryIdentityOutput {
    schema_version: u16,
    public_key: String,
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

#[derive(Debug, Default, Clone, Copy, ValueEnum)]
enum PriceSourceArgument {
    #[default]
    Local,
    LogosCApi,
}

impl From<PriceSourceArgument> for MakerPriceSourceKind {
    fn from(source: PriceSourceArgument) -> Self {
        match source {
            PriceSourceArgument::Local => Self::Local,
            PriceSourceArgument::LogosCApi => Self::LogosCApi,
        }
    }
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
    let output = execute(&arguments.socket, arguments.command).await?;
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

async fn execute(socket: &Path, command: Command) -> anyhow::Result<serde_json::Value> {
    match command {
        Command::DeliveryIdentity { signing_key_file } => delivery_identity(&signing_key_file),
        command @ (Command::Start | Command::Stop) => control_service(&command),
        command @ Command::ConfigurePair { .. } => configure_pair(socket, command).await,
        command @ Command::SetLocalPrice { .. } => set_local_price(socket, command).await,
        command @ Command::CreateSwap { .. } => create_swap(socket, command).await,
        command @ Command::PublishOffer { .. } => publish_offer(socket, command).await,
        command @ Command::WithdrawOffer { .. } => withdraw_offer(socket, command).await,
        Command::Pairs => {
            let pairs: Vec<VersionedMakerRecord<MakerPairConfigurationV1>> =
                call_local_rpc(socket, "maker_pair_list", &ListRequest::default()).await?;
            serde_json::to_value(pairs).map_err(Into::into)
        }
        Command::Prices => {
            let prices: Vec<VersionedMakerRecord<LocalPriceV1>> =
                call_local_rpc(socket, "maker_local_price_list", &ListRequest::default()).await?;
            serde_json::to_value(prices).map_err(Into::into)
        }
        Command::Health => {
            let health: MakerHealthV1 =
                call_local_rpc(socket, "maker_health", &ListRequest::default()).await?;
            serde_json::to_value(health).map_err(Into::into)
        }
        Command::Quote { pair, direction } => {
            let request = PriceQuoteRequest {
                route: MakerRouteV1::new(pair.into(), direction.into())?,
            };
            let quote: PriceQuoteV1 = call_local_rpc(socket, "maker_price_quote", &request).await?;
            serde_json::to_value(quote).map_err(Into::into)
        }
        Command::Offers => {
            let offers: Vec<MakerOfferRecordV1> =
                call_local_rpc(socket, "maker_offer_list", &ListRequest::default()).await?;
            serde_json::to_value(offers).map_err(Into::into)
        }
        Command::History => {
            let history: Vec<SwapView> =
                call_local_rpc(socket, "swap_history", &ListRequest::default()).await?;
            serde_json::to_value(history).map_err(Into::into)
        }
        Command::Status { id } => {
            let request = StatusRequest { id: id.into() };
            let view: SwapView = call_local_rpc(socket, "swap_status", &request).await?;
            serde_json::to_value(view).map_err(Into::into)
        }
        Command::Alerts { id, after, all } => {
            let request = AlertListRequest {
                id: id.into(),
                after_sequence: after,
                include_acknowledged: all,
            };
            let alerts: Vec<OperatorAlertView> =
                call_local_rpc(socket, "swap_alerts", &request).await?;
            serde_json::to_value(alerts).map_err(Into::into)
        }
        Command::AcknowledgeAlert { id, alert_sequence } => {
            let request = AlertAcknowledgeRequest {
                id: id.into(),
                alert_sequence,
            };
            let view: SwapView = call_local_rpc(socket, "swap_alert_acknowledge", &request).await?;
            serde_json::to_value(view).map_err(Into::into)
        }
        Command::Monitor { id } => {
            let request = MakerActorMonitorRequestV1 { id: id.into() };
            let view: MakerActorMonitorV1 =
                call_local_rpc(socket, "maker_actor_monitor_v1", &request).await?;
            serde_json::to_value(view).map_err(Into::into)
        }
        Command::Claim {
            id,
            request_id,
            expected_generation,
        } => {
            maker_actor_action(
                socket,
                id,
                request_id,
                expected_generation,
                "maker_actor_claim_v1",
            )
            .await
        }
        Command::Refund {
            id,
            request_id,
            expected_generation,
        } => {
            maker_actor_action(
                socket,
                id,
                request_id,
                expected_generation,
                "maker_actor_refund_v1",
            )
            .await
        }
    }
}

fn delivery_identity(signing_key_file: &Path) -> anyhow::Result<serde_json::Value> {
    let signing_key = load_secp256k1_secret(signing_key_file, "Delivery signing key")?;
    let public_key =
        PublicKey::from_secret_key(&Secp256k1::signing_only(), &signing_key).serialize();
    serde_json::to_value(DeliveryIdentityOutput {
        schema_version: 1,
        public_key: hex::encode(public_key),
    })
    .map_err(Into::into)
}

async fn maker_actor_action(
    socket: &Path,
    id: String,
    request_id: String,
    expected_generation: u64,
    method: &str,
) -> anyhow::Result<serde_json::Value> {
    let request = MakerActorActionRequestV1 {
        request_id: RequestId::new(request_id)?,
        id: id.into(),
        expected_generation,
    };
    let commit: MakerActorActionCommitV1 = call_local_rpc(socket, method, &request).await?;
    serde_json::to_value(commit).map_err(Into::into)
}

fn control_service(command: &Command) -> anyhow::Result<serde_json::Value> {
    let action = match command {
        Command::Start => MakerServiceAction::Start,
        Command::Stop => MakerServiceAction::Stop,
        _ => unreachable!("service helper receives only lifecycle commands"),
    };
    serde_json::to_value(control_maker_service(action)?).map_err(Into::into)
}

async fn configure_pair(socket: &Path, command: Command) -> anyhow::Result<serde_json::Value> {
    let Command::ConfigurePair {
        request_id,
        expected_revision,
        pair,
        direction,
        enabled,
        price_source,
        minimum_foreign_units,
        maximum_foreign_units,
        offer_ttl_seconds,
    } = command
    else {
        unreachable!("configure_pair receives only its matching command")
    };
    let route = MakerRouteV1::new(pair.into(), direction.into())?;
    let configuration = MakerPairConfigurationV1::new(
        route,
        enabled,
        price_source.into(),
        minimum_foreign_units,
        maximum_foreign_units,
        offer_ttl_seconds,
    )?;
    let request = PairConfigureRequest {
        request_id: RequestId::new(request_id)?,
        expected_revision,
        configuration,
    };
    let commit: MakerConfigurationCommit =
        call_local_rpc(socket, "maker_pair_configure", &request).await?;
    serde_json::to_value(commit).map_err(Into::into)
}

async fn set_local_price(socket: &Path, command: Command) -> anyhow::Result<serde_json::Value> {
    let Command::SetLocalPrice {
        request_id,
        expected_revision,
        pair,
        direction,
        lez_units_per_lot,
        foreign_units_per_lot,
    } = command
    else {
        unreachable!("set_local_price receives only its matching command")
    };
    let route = MakerRouteV1::new(pair.into(), direction.into())?;
    let price = LocalPriceV1::new(route, lez_units_per_lot, foreign_units_per_lot)?;
    let request = LocalPriceSetRequest {
        request_id: RequestId::new(request_id)?,
        expected_revision,
        price,
    };
    let commit: MakerConfigurationCommit =
        call_local_rpc(socket, "maker_local_price_set", &request).await?;
    serde_json::to_value(commit).map_err(Into::into)
}

async fn publish_offer(socket: &Path, command: Command) -> anyhow::Result<serde_json::Value> {
    let Command::PublishOffer {
        request_id,
        offer_id,
        pair,
        direction,
    } = command
    else {
        unreachable!("publish_offer receives only its matching command")
    };
    let request = OfferPublishRequest {
        request_id: RequestId::new(request_id)?,
        offer_id: MakerOfferId::new(offer_id)?,
        route: MakerRouteV1::new(pair.into(), direction.into())?,
    };
    let commit: MakerOfferCommit = call_local_rpc(socket, "maker_offer_publish", &request).await?;
    serde_json::to_value(commit).map_err(Into::into)
}

async fn withdraw_offer(socket: &Path, command: Command) -> anyhow::Result<serde_json::Value> {
    let Command::WithdrawOffer {
        request_id,
        offer_id,
        expected_revision,
    } = command
    else {
        unreachable!("withdraw_offer receives only its matching command")
    };
    let request = OfferWithdrawRequest {
        request_id: RequestId::new(request_id)?,
        offer_id: MakerOfferId::new(offer_id)?,
        expected_revision,
    };
    let commit: MakerOfferCommit = call_local_rpc(socket, "maker_offer_withdraw", &request).await?;
    serde_json::to_value(commit).map_err(Into::into)
}

async fn create_swap(socket: &Path, command: Command) -> anyhow::Result<serde_json::Value> {
    let Command::CreateSwap {
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
    } = command
    else {
        unreachable!("create_swap receives only its matching command")
    };
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
    let view: SwapView = call_local_rpc(socket, "swap_create", &request).await?;
    serde_json::to_value(view).map_err(Into::into)
}
