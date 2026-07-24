use std::{path::PathBuf, str::FromStr as _};

use clap::{Parser, ValueEnum};
use lez_maker_node::{DeliveryOfferQueryV1, RunLocalDelivery};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::MakerRouteV1;
use secp256k1::PublicKey;
use serde::Serialize;

#[derive(Parser)]
#[command(about = "LEZ atomic-swap taker CLI")]
struct Arguments {
    /// Owner-private run-local Delivery directory.
    #[arg(long)]
    delivery_directory: PathBuf,
    /// Expected compressed secp256k1 maker identity in hex.
    #[arg(long)]
    maker_public_key: String,
    /// Trusted taker-local Unix time used for the half-open offer TTL.
    #[arg(long)]
    now_unix_seconds: u64,
    /// Optional exact pair filter.
    #[arg(long, value_enum)]
    pair: Option<PairArgument>,
    /// Optional exact direction filter; requires `--pair`.
    #[arg(long, value_enum)]
    direction: Option<DirectionArgument>,
}

#[derive(Clone, Copy, ValueEnum)]
enum PairArgument {
    Bitcoin,
    Monero,
    Zcash,
}

impl From<PairArgument> for Pair {
    fn from(value: PairArgument) -> Self {
        match value {
            PairArgument::Bitcoin => Self::Bitcoin,
            PairArgument::Monero => Self::Monero,
            PairArgument::Zcash => Self::Zcash,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum DirectionArgument {
    TakerSellsForeign,
    TakerSellsLez,
}

impl From<DirectionArgument> for SwapDirection {
    fn from(value: DirectionArgument) -> Self {
        match value {
            DirectionArgument::TakerSellsForeign => Self::TakerSellsForeign,
            DirectionArgument::TakerSellsLez => Self::TakerSellsLez,
        }
    }
}

#[derive(Serialize)]
struct DiscoveryOutput<'a> {
    schema_version: u16,
    offers: Vec<OfferView<'a>>,
}

#[derive(Serialize)]
struct OfferView<'a> {
    offer: &'a lez_swap_store::MakerOfferV1,
    maker_public_key: String,
    signed_envelope_sha256: String,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = execute().await {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

async fn execute() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    anyhow::ensure!(
        arguments.direction.is_none() || arguments.pair.is_some(),
        "--direction requires --pair"
    );
    let expected_maker = PublicKey::from_str(&arguments.maker_public_key)
        .map_err(|_| anyhow::anyhow!("maker public key is invalid"))?;
    let delivery = RunLocalDelivery::subscriber(arguments.delivery_directory, expected_maker)?;
    let query = match arguments.pair {
        Some(pair) => DeliveryOfferQueryV1::for_route(
            MakerRouteV1::new(
                pair.into(),
                arguments
                    .direction
                    .unwrap_or(DirectionArgument::TakerSellsForeign)
                    .into(),
            )?,
            arguments.now_unix_seconds,
        ),
        None => DeliveryOfferQueryV1::all(arguments.now_unix_seconds),
    };
    let offers = delivery.discover(&query).await?;
    let output = DiscoveryOutput {
        schema_version: 1,
        offers: offers
            .iter()
            .map(|offer| OfferView {
                offer: offer.offer(),
                maker_public_key: hex::encode(offer.maker_identity()),
                signed_envelope_sha256: hex::encode(offer.commitment()),
            })
            .collect(),
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}
