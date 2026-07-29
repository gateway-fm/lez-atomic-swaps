use std::{path::PathBuf, str::FromStr as _};

use anyhow::{Context as _, ensure};
use btc_reference_actor::{
    ActorCommand as BtcActorCommand, ActorConfig as BtcActorConfig, ActorRole as BtcActorRole,
    execute_actor_command as execute_btc_actor_command,
};
use clap::{ArgGroup, Args as ClapArgs, Parser, Subcommand, ValueEnum};
use lez_bridge_protocol::RequestId;
use lez_maker_node::{DeliveryOfferQueryV1, RunLocalDelivery};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{MakerActorHeldLock, MakerRouteV1, maker_btc_chat_swap_id};
use secp256k1::PublicKey;
use serde::Serialize;
use zec_reference_actor::{
    ActorCommand as ZecActorCommand, ActorConfig, ActorRole, execute_actor_command,
};

#[path = "support/secure_file.rs"]
mod secure_file;
#[path = "support/taker_accept.rs"]
mod taker_accept;
#[path = "support/taker_accept_btc.rs"]
mod taker_accept_btc;
use taker_accept_btc::{BtcTakeInput, load_btc_taker_actor_from_receipt, take_btc};

use taker_accept::{ZecTakeInput, load_taker_actor_from_receipt, take_zec};

#[derive(Parser)]
#[command(about = "LEZ atomic-swap taker CLI")]
struct Arguments {
    /// Post-acceptance role-local lifecycle command.
    #[command(subcommand)]
    command: Option<LifecycleCommand>,
    /// Owner-private run-local Delivery directory.
    #[arg(long)]
    delivery_directory: Option<PathBuf>,
    /// Expected compressed secp256k1 maker identity in hex.
    #[arg(long)]
    maker_public_key: Option<String>,
    /// Trusted taker-local Unix time used for the half-open offer TTL.
    #[arg(long)]
    now_unix_seconds: Option<u64>,
    /// Optional exact pair filter.
    #[arg(long, value_enum)]
    pair: Option<PairArgument>,
    /// Optional exact direction filter; requires `--pair`.
    #[arg(long, value_enum)]
    direction: Option<DirectionArgument>,
    /// Plan this exact BTC offer before constructing its chain-complete draft.
    #[arg(long)]
    plan_btc_offer: Option<String>,
    /// Accept this exact ZEC offer instead of only listing discovery results.
    #[arg(long)]
    accept_zec_offer: Option<String>,
    /// Accept this exact BTC offer instead of only listing discovery results.
    #[arg(long)]
    accept_btc_offer: Option<String>,
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
    /// Owner-private source Taker actor authority template.
    #[arg(long)]
    zec_source_taker_config: Option<PathBuf>,
    /// New owner-private root for the accepted Taker actor bundle.
    #[arg(long)]
    zec_taker_actor_root: Option<PathBuf>,
    /// New owner-private acceptance receipt written after Maker completion.
    #[arg(long)]
    zec_acceptance_receipt: Option<PathBuf>,
    /// Owner-private source Taker Bitcoin actor authority template.
    #[arg(long)]
    btc_source_taker_config: Option<PathBuf>,
    /// New owner-private root for the accepted Taker Bitcoin actor bundle.
    #[arg(long)]
    btc_taker_actor_root: Option<PathBuf>,
    /// New owner-private Bitcoin acceptance receipt written after Maker completion.
    #[arg(long)]
    btc_acceptance_receipt: Option<PathBuf>,
}
#[derive(Subcommand)]
enum LifecycleCommand {
    /// Read one role-local durable status without chain or transport access.
    Monitor(LifecycleArguments),
    /// Attempt only the agreement-ordered Taker claim.
    Claim(LifecycleArguments),
    /// Attempt only the agreement-ordered Taker timeout recovery.
    Refund(LifecycleArguments),
}

#[derive(ClapArgs)]
#[command(group(ArgGroup::new("actor_source").required(true).multiple(false).args(["actor_config", "receipt"])))]
struct LifecycleArguments {
    /// Owner-private role-fixed Taker actor configuration.
    #[arg(long, value_name = "PRIVATE_JSON")]
    actor_config: Option<PathBuf>,
    /// Owner-private acceptance receipt selecting the exact Taker actor.
    #[arg(long, value_name = "PRIVATE_JSON")]
    receipt: Option<PathBuf>,
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
#[derive(Serialize)]
struct BtcPlanOutput {
    schema_version: u16,
    offer_id: String,
    reservation_id: String,
    signed_envelope_sha256: String,
    swap_id: String,
    foreign_units: u64,
    lez_units: u128,
    private_material_disclosed: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = execute().await {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

async fn execute() -> anyhow::Result<()> {
    let mut arguments = Arguments::parse();
    if let Some(command) = arguments.command.take() {
        return execute_lifecycle(command).await;
    }
    anyhow::ensure!(
        arguments.direction.is_none() || arguments.pair.is_some(),
        "--direction requires --pair"
    );
    let maker_public_key = arguments
        .maker_public_key
        .as_deref()
        .context("discovery requires --maker-public-key")?;
    let expected_maker = PublicKey::from_str(maker_public_key)
        .map_err(|_| anyhow::anyhow!("maker public key is invalid"))?;
    let delivery_directory = arguments
        .delivery_directory
        .as_ref()
        .context("discovery requires --delivery-directory")?;
    let now_unix_seconds = arguments
        .now_unix_seconds
        .context("discovery requires --now-unix-seconds")?;
    let btc_take_requested = btc_take_was_requested(&arguments);
    let zec_take_requested = zec_take_was_requested(&arguments);
    let btc_plan_requested = arguments.plan_btc_offer.is_some();
    ensure!(
        !(btc_take_requested && zec_take_requested),
        "exactly one BTC or ZEC acceptance may be requested"
    );
    ensure!(
        !(btc_plan_requested && (btc_take_requested || zec_take_requested)),
        "BTC planning and acceptance are mutually exclusive"
    );
    if btc_plan_requested {
        return execute_btc_plan(
            &arguments,
            &expected_maker,
            delivery_directory,
            now_unix_seconds,
        )
        .await;
    }
    if btc_take_requested {
        return execute_btc_take(
            &arguments,
            &expected_maker,
            delivery_directory,
            now_unix_seconds,
        )
        .await;
    }
    if zec_take_requested || shared_take_was_requested(&arguments) {
        return execute_zec_take(
            &arguments,
            &expected_maker,
            delivery_directory,
            now_unix_seconds,
        )
        .await;
    }
    let delivery = RunLocalDelivery::subscriber(delivery_directory.clone(), expected_maker)?;
    let query = match arguments.pair {
        Some(pair) => DeliveryOfferQueryV1::for_route(
            MakerRouteV1::new(
                pair.into(),
                arguments
                    .direction
                    .unwrap_or(DirectionArgument::TakerSellsForeign)
                    .into(),
            )?,
            now_unix_seconds,
        ),
        None => DeliveryOfferQueryV1::all(now_unix_seconds),
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

async fn execute_zec_take(
    arguments: &Arguments,
    expected_maker: &PublicKey,
    delivery_directory: &std::path::Path,
    now_unix_seconds: u64,
) -> anyhow::Result<()> {
    ensure!(
        arguments
            .pair
            .is_none_or(|pair| matches!(pair, PairArgument::Zcash))
            && arguments
                .direction
                .is_none_or(|direction| matches!(direction, DirectionArgument::TakerSellsLez)),
        "M5 ZEC acceptance supports only zcash/taker-sells-lez"
    );
    let agreement_output_file = required_path(
        arguments.agreement_output_file.as_deref(),
        "--agreement-output-file",
    )?;
    let agreement_is_durable = match std::fs::symlink_metadata(agreement_output_file) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error).context("inspect persisted ZEC agreement"),
    };
    let delivery = if agreement_is_durable {
        None
    } else {
        Some(RunLocalDelivery::subscriber(
            delivery_directory.to_path_buf(),
            expected_maker.to_owned(),
        )?)
    };
    let output = take_zec(ZecTakeInput {
        delivery: delivery.as_ref(),
        expected_maker,
        now_unix_seconds,
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
        agreement_output_file,
        source_taker_config_file: required_path(
            arguments.zec_source_taker_config.as_deref(),
            "--zec-source-taker-config",
        )?,
        taker_actor_root: required_path(
            arguments.zec_taker_actor_root.as_deref(),
            "--zec-taker-actor-root",
        )?,
        acceptance_receipt_file: required_path(
            arguments.zec_acceptance_receipt.as_deref(),
            "--zec-acceptance-receipt",
        )?,
    })
    .await?;
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

async fn execute_btc_take(
    arguments: &Arguments,
    expected_maker: &PublicKey,
    delivery_directory: &std::path::Path,
    now_unix_seconds: u64,
) -> anyhow::Result<()> {
    ensure!(
        arguments
            .pair
            .is_none_or(|pair| matches!(pair, PairArgument::Bitcoin))
            && arguments.direction.is_none_or(|direction| {
                matches!(direction, DirectionArgument::TakerSellsForeign)
            }),
        "M5 BTC acceptance supports only bitcoin/taker-sells-foreign"
    );
    let agreement_output_file = required_btc_path(
        arguments.agreement_output_file.as_deref(),
        "--agreement-output-file",
    )?;
    let agreement_is_durable = match std::fs::symlink_metadata(agreement_output_file) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error).context("inspect persisted BTC agreement"),
    };
    let delivery = if agreement_is_durable {
        None
    } else {
        Some(RunLocalDelivery::subscriber(
            delivery_directory.to_path_buf(),
            expected_maker.to_owned(),
        )?)
    };
    let output = take_btc(BtcTakeInput {
        delivery: delivery.as_ref(),
        now_unix_seconds,
        offer_id: arguments
            .accept_btc_offer
            .as_deref()
            .context("BTC acceptance requires --accept-btc-offer")?,
        chat_socket: required_btc_path(arguments.chat_socket.as_deref(), "--chat-socket")?,
        reservation_id: arguments
            .reservation_id
            .as_deref()
            .context("BTC acceptance requires --reservation-id")?,
        foreign_units: arguments
            .foreign_units
            .context("BTC acceptance requires --foreign-units")?,
        unsigned_draft_file: required_btc_path(
            arguments.unsigned_draft_file.as_deref(),
            "--unsigned-draft-file",
        )?,
        taker_signing_key_file: required_btc_path(
            arguments.taker_signing_key_file.as_deref(),
            "--taker-signing-key-file",
        )?,
        agreement_output_file,
        source_taker_config_file: required_btc_path(
            arguments.btc_source_taker_config.as_deref(),
            "--btc-source-taker-config",
        )?,
        taker_actor_root: required_btc_path(
            arguments.btc_taker_actor_root.as_deref(),
            "--btc-taker-actor-root",
        )?,
        acceptance_receipt_file: required_btc_path(
            arguments.btc_acceptance_receipt.as_deref(),
            "--btc-acceptance-receipt",
        )?,
    })
    .await?;
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

#[derive(Clone, Copy)]
enum LifecycleAction {
    Monitor,
    Claim,
    Refund,
}
async fn execute_btc_plan(
    arguments: &Arguments,
    expected_maker: &PublicKey,
    delivery_directory: &std::path::Path,
    now_unix_seconds: u64,
) -> anyhow::Result<()> {
    ensure!(
        arguments
            .pair
            .is_none_or(|pair| matches!(pair, PairArgument::Bitcoin))
            && arguments.direction.is_none_or(|direction| {
                matches!(direction, DirectionArgument::TakerSellsForeign)
            }),
        "M5 BTC planning supports only bitcoin/taker-sells-foreign"
    );
    ensure!(
        arguments.chat_socket.is_none()
            && arguments.unsigned_draft_file.is_none()
            && arguments.taker_signing_key_file.is_none()
            && arguments.agreement_output_file.is_none()
            && arguments.zec_source_taker_config.is_none()
            && arguments.zec_taker_actor_root.is_none()
            && arguments.zec_acceptance_receipt.is_none()
            && arguments.btc_source_taker_config.is_none()
            && arguments.btc_taker_actor_root.is_none()
            && arguments.btc_acceptance_receipt.is_none(),
        "BTC planning accepts no Chat, signing, agreement, actor, or receipt authority"
    );
    let offer_id = arguments
        .plan_btc_offer
        .as_deref()
        .context("BTC planning requires --plan-btc-offer")?;
    let reservation_id = RequestId::new(
        arguments
            .reservation_id
            .as_deref()
            .context("BTC planning requires --reservation-id")?,
    )?;
    let foreign_units = arguments
        .foreign_units
        .context("BTC planning requires --foreign-units")?;
    ensure!(foreign_units > 0, "BTC principal must be nonzero");
    let route = MakerRouteV1::new(Pair::Bitcoin, SwapDirection::TakerSellsForeign)?;
    let delivery =
        RunLocalDelivery::subscriber(delivery_directory.to_path_buf(), expected_maker.to_owned())?;
    let selected = delivery
        .discover(&DeliveryOfferQueryV1::for_route(route, now_unix_seconds))
        .await?
        .into_iter()
        .find(|candidate| candidate.offer().id().as_str() == offer_id)
        .context("selected BTC offer is unavailable, expired, or not authentic")?;
    let lez_units = selected.offer().quote_foreign_amount(foreign_units)?;
    let commitment = selected.commitment();
    let output = BtcPlanOutput {
        schema_version: 1,
        offer_id: offer_id.to_owned(),
        reservation_id: reservation_id.as_str().to_owned(),
        signed_envelope_sha256: hex::encode(commitment),
        swap_id: hex::encode(maker_btc_chat_swap_id(&commitment, &reservation_id)),
        foreign_units,
        lez_units,
        private_material_disclosed: false,
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

enum LoadedTakerActor {
    Zec(Box<ActorConfig>),
    Btc(Box<BtcActorConfig>),
}

#[derive(Serialize)]
struct BtcLifecycleOutput {
    pair: &'static str,
    #[serde(flatten)]
    output: btc_reference_actor::ActorCommandOutputV1,
}

async fn execute_lifecycle(command: LifecycleCommand) -> anyhow::Result<()> {
    let (arguments, action) = match command {
        LifecycleCommand::Monitor(arguments) => (arguments, LifecycleAction::Monitor),
        LifecycleCommand::Claim(arguments) => (arguments, LifecycleAction::Claim),
        LifecycleCommand::Refund(arguments) => (arguments, LifecycleAction::Refund),
    };
    let config = match (arguments.actor_config.as_ref(), arguments.receipt.as_ref()) {
        (Some(path), None) => match (
            ActorConfig::load_private(path),
            BtcActorConfig::load_private(path),
        ) {
            (Ok(config), Err(_)) => LoadedTakerActor::Zec(Box::new(config)),
            (Err(_), Ok(config)) => LoadedTakerActor::Btc(Box::new(config)),
            _ => {
                return Err(anyhow::anyhow!(
                    "Taker actor configuration is unavailable or ambiguous"
                ));
            }
        },
        (None, Some(path)) => match (
            load_taker_actor_from_receipt(path),
            load_btc_taker_actor_from_receipt(path),
        ) {
            (Ok(config), Err(_)) => LoadedTakerActor::Zec(Box::new(config)),
            (Err(_), Ok(config)) => LoadedTakerActor::Btc(Box::new(config)),
            _ => {
                return Err(anyhow::anyhow!(
                    "Taker acceptance receipt is unavailable or ambiguous"
                ));
            }
        },
        _ => {
            return Err(anyhow::anyhow!(
                "exactly one Taker actor source is required"
            ));
        }
    };
    match config {
        LoadedTakerActor::Zec(config) => {
            ensure!(
                config.role() == ActorRole::Taker,
                "Taker actor configuration has the wrong role"
            );
            let _held_lock =
                MakerActorHeldLock::acquire_for(config.swap_id(), config.role_state_db())
                    .map_err(|_| anyhow::anyhow!("Taker actor is already running or unsafe"))?;
            let command = match action {
                LifecycleAction::Monitor => ZecActorCommand::Status,
                LifecycleAction::Claim => ZecActorCommand::Claim,
                LifecycleAction::Refund => ZecActorCommand::Recover,
            };
            let output = execute_actor_command(&config, command)
                .await
                .map_err(|_| anyhow::anyhow!("Taker lifecycle command failed"))?;
            println!("{}", serde_json::to_string(&output)?);
        }
        LoadedTakerActor::Btc(config) => {
            ensure!(
                config.role() == BtcActorRole::Taker,
                "BTC Taker actor configuration has the wrong role"
            );
            let swap_id = config
                .supervised_swap_id()
                .map_err(|_| anyhow::anyhow!("BTC Taker actor agreement is unavailable"))?;
            let _held_lock = MakerActorHeldLock::acquire_for(&swap_id, config.state_db())
                .map_err(|_| anyhow::anyhow!("BTC Taker actor is already running or unsafe"))?;
            let command = match action {
                LifecycleAction::Monitor => BtcActorCommand::Status,
                LifecycleAction::Claim => BtcActorCommand::Drive,
                LifecycleAction::Refund => BtcActorCommand::Recover,
            };
            let output = execute_btc_actor_command(&config, command)
                .await
                .map_err(|_| anyhow::anyhow!("BTC Taker lifecycle command failed"))?;
            println!(
                "{}",
                serde_json::to_string(&BtcLifecycleOutput {
                    pair: "bitcoin",
                    output
                })?
            );
        }
    }
    Ok(())
}

fn btc_take_was_requested(arguments: &Arguments) -> bool {
    arguments.accept_btc_offer.is_some()
        || arguments.btc_source_taker_config.is_some()
        || arguments.btc_taker_actor_root.is_some()
        || arguments.btc_acceptance_receipt.is_some()
}

fn zec_take_was_requested(arguments: &Arguments) -> bool {
    arguments.accept_zec_offer.is_some()
        || arguments.zec_source_taker_config.is_some()
        || arguments.zec_taker_actor_root.is_some()
        || arguments.zec_acceptance_receipt.is_some()
}

fn shared_take_was_requested(arguments: &Arguments) -> bool {
    arguments.chat_socket.is_some()
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

fn required_btc_path<'a>(
    value: Option<&'a std::path::Path>,
    flag: &str,
) -> anyhow::Result<&'a std::path::Path> {
    value.with_context(|| format!("BTC acceptance requires {flag}"))
}
