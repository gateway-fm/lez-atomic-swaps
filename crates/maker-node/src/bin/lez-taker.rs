use std::{path::PathBuf, str::FromStr as _};

use anyhow::{Context as _, ensure};
use clap::{Parser, ValueEnum};
use lez_maker_node::{DeliveryOfferQueryV1, RunLocalDelivery};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::MakerRouteV1;
use secp256k1::PublicKey;
use serde::Serialize;

#[path = "support/secure_file.rs"]
mod secure_file;
#[path = "support/taker_accept.rs"]
mod taker_accept;
use taker_accept::{ZecTakeInput, take_zec};

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
    /// Accept this exact ZEC offer instead of only listing discovery results.
    #[arg(long)]
    accept_zec_offer: Option<String>,
    /// Taker-facing maker Chat Unix socket.
    #[arg(long)]
    chat_socket: Option<PathBuf>,
    /// Stable Chat reservation identity already bound into the unsigned draft.
    #[arg(long)]
    reservation_id: Option<String>,
    /// Exact selected Zcash principal in zatoshis.
    #[arg(long)]
    foreign_units: Option<u64>,
    /// Owner-private canonical unsigned agreement draft.
    #[arg(long)]
    unsigned_draft_file: Option<PathBuf>,
    /// Owner-private raw 32-byte taker agreement key.
    #[arg(long)]
    taker_signing_key_file: Option<PathBuf>,
    /// New owner-private file for the exact countersigned agreement.
    #[arg(long)]
    agreement_output_file: Option<PathBuf>,
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
    let delivery =
        RunLocalDelivery::subscriber(arguments.delivery_directory.clone(), expected_maker)?;
    if take_was_requested(&arguments) {
        ensure!(
            arguments
                .pair
                .is_none_or(|pair| matches!(pair, PairArgument::Zcash))
                && arguments
                    .direction
                    .is_none_or(|direction| matches!(direction, DirectionArgument::TakerSellsLez)),
            "M5 ZEC acceptance supports only zcash/taker-sells-lez"
        );
        let output = take_zec(ZecTakeInput {
            delivery: &delivery,
            expected_maker: &expected_maker,
            now_unix_seconds: arguments.now_unix_seconds,
            offer_id: arguments
                .accept_zec_offer
                .as_deref()
                .context("ZEC acceptance requires --accept-zec-offer")?,
            chat_socket: required_path(arguments.chat_socket.as_deref(), "--chat-socket")?,
            reservation_id: arguments
                .reservation_id
                .as_deref()
                .context("ZEC acceptance requires --reservation-id")?,
            foreign_units: arguments
                .foreign_units
                .context("ZEC acceptance requires --foreign-units")?,
            unsigned_draft_file: required_path(
                arguments.unsigned_draft_file.as_deref(),
                "--unsigned-draft-file",
            )?,
            taker_signing_key_file: required_path(
                arguments.taker_signing_key_file.as_deref(),
                "--taker-signing-key-file",
            )?,
            agreement_output_file: required_path(
                arguments.agreement_output_file.as_deref(),
                "--agreement-output-file",
            )?,
        })
        .await?;
        println!("{}", serde_json::to_string(&output)?);
        return Ok(());
    }
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

fn take_was_requested(arguments: &Arguments) -> bool {
    arguments.accept_zec_offer.is_some()
        || arguments.chat_socket.is_some()
        || arguments.reservation_id.is_some()
        || arguments.foreign_units.is_some()
        || arguments.unsigned_draft_file.is_some()
        || arguments.taker_signing_key_file.is_some()
        || arguments.agreement_output_file.is_some()
}

fn required_path<'a>(
    value: Option<&'a std::path::Path>,
    flag: &str,
) -> anyhow::Result<&'a std::path::Path> {
    value.with_context(|| format!("ZEC acceptance requires {flag}"))
}
