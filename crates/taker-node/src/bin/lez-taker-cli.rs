use std::{io::Read as _, path::PathBuf, process::Stdio, str::FromStr as _, time::Duration};

#[cfg(feature = "test-crash-hooks")]
use std::{
    env, fs,
    fs::OpenOptions,
    io::Write as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    thread,
};

use anyhow::{Context as _, ensure};
use btc_reference_actor::{
    ActorCommand as BtcActorCommand, ActorConfig as BtcActorConfig, ActorRole as BtcActorRole,
    execute_actor_command as execute_btc_actor_command,
};
use clap::{ArgGroup, Args as ClapArgs, Parser, Subcommand, ValueEnum};
use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{
    MakerActorHeldLock, MakerRouteV1, SqliteXmrWorkflowJournal, StoreError,
    XmrWorkflowReconciliationV2, XmrWorkflowStep, maker_btc_chat_swap_id, maker_xmr_chat_swap_id,
};
use lez_taker_node::{
    DeliveryOfferQueryV1, NodeServiceAction, RunLocalDelivery, call_local_rpc,
    control_taker_service,
};
use secp256k1::PublicKey;
use serde::Serialize;
use wait_timeout::ChildExt as _;
use xmr_reference_actor::{
    ValidatedXmrEffectExecutionV3, XMR_EFFECT_OBSERVER_RESULT_MAX_BYTES, XmrEffectObserverStateV1,
    XmrPreparedEffectInvocationV1, load_validated_xmr_taker_authority_bytes,
    parse_xmr_effect_observer_result_v1,
};
use zec_reference_actor::{
    ActorCommand as ZecActorCommand, ActorConfig, ActorRole, execute_actor_command,
};

#[path = "support/taker_accept.rs"]
mod taker_accept;
#[path = "support/taker_accept_btc.rs"]
mod taker_accept_btc;
#[path = "support/taker_accept_xmr.rs"]
mod taker_accept_xmr;
use taker_accept_btc::{BtcTakeInput, load_btc_taker_actor_from_receipt, take_btc};
use taker_accept_xmr::{
    XmrEffectTakeInput, XmrTakeInput, XmrTakerEffectReceiptSelector, XmrTakerReceiptSelector,
    load_xmr_taker_effect_receipt_selector, load_xmr_taker_receipt_selector, take_xmr,
};

use taker_accept::{ZecTakeInput, load_taker_actor_from_receipt, take_zec};

// Exact LEZ observation verifies finalized block proofs locally. The dual-XMR
// actual-node run measured valid observations exceeding the generic 30-second
// effect-invocation bound, so observation gets its own bounded allowance.
const XMR_EFFECT_OBSERVATION_TIMEOUT: Duration = Duration::from_mins(2);

#[derive(Parser)]
#[command(about = "Operator CLI for the LEZ atomic-swap Taker Node")]
struct Arguments {
    /// Owner-only Taker Node socket used by operational commands.
    #[arg(long, default_value = "/run/lez/taker/node.sock")]
    socket: PathBuf,
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
    /// Plan this exact XMR offer before composing role-separated Stage A/B.
    #[arg(long)]
    plan_xmr_offer: Option<String>,
    /// Accept this exact ZEC offer instead of only listing discovery results.
    #[arg(long)]
    accept_zec_offer: Option<String>,
    /// Accept this exact BTC offer instead of only listing discovery results.
    #[arg(long)]
    accept_btc_offer: Option<String>,
    /// Accept this exact XMR offer using already role-separated Stage A/B material.
    #[arg(long)]
    accept_xmr_offer: Option<String>,
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
    /// Exact signed Maker contribution for contribution-bound BTC Chat v2.
    #[arg(long, requires = "taker_contribution_file")]
    maker_contribution_file: Option<PathBuf>,
    /// Exact local signed Taker contribution for contribution-bound BTC Chat v2.
    #[arg(long, requires = "maker_contribution_file")]
    taker_contribution_file: Option<PathBuf>,
    /// Existing owner-private Taker role root created before Chat v2.
    #[arg(
        long,
        requires_all = ["maker_contribution_file", "taker_contribution_file"]
    )]
    btc_role_root: Option<PathBuf>,
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
    /// Owner-private canonical dual-signed XMR Stage-A agreement.
    #[arg(long)]
    xmr_stage_a_file: Option<PathBuf>,
    /// Owner-private canonical dual-signed XMR Stage-B activation.
    #[arg(long)]
    xmr_activation_file: Option<PathBuf>,
    /// Existing owner-private Taker XMR role root.
    #[arg(long)]
    xmr_source_taker_root: Option<PathBuf>,
    /// Existing Taker public role packet.
    #[arg(long)]
    xmr_taker_public_packet: Option<PathBuf>,
    /// Existing Maker public role packet.
    #[arg(long)]
    xmr_maker_public_packet: Option<PathBuf>,
    /// Existing owner-private completed Taker adaptor-session journal.
    #[arg(long)]
    xmr_taker_role_journal: Option<PathBuf>,
    /// New owner-private root for the accepted Taker XMR actor bundle.
    #[arg(long)]
    xmr_taker_actor_root: Option<PathBuf>,
    /// New owner-private XMR acceptance receipt written after Maker completion.
    #[arg(long)]
    xmr_acceptance_receipt: Option<PathBuf>,
    /// Immutable owner-private role-fixed XMR chain-effect authority.
    #[arg(long)]
    xmr_effect_authority_file: Option<PathBuf>,
    /// New owner-private schema-v3 XMR effect manifest.
    #[arg(long)]
    xmr_effect_manifest_file: Option<PathBuf>,
    /// New or exactly replayed owner-private XMR workflow journal.
    #[arg(long)]
    xmr_workflow_journal: Option<PathBuf>,
    /// Run identity bound into XMR effect authority and workflow state.
    #[arg(long)]
    xmr_run_id: Option<String>,
}
#[derive(Subcommand)]
enum LifecycleCommand {
    /// Probe the role-fixed Taker Node through its owner socket.
    Health,
    /// Start only the packaged `lez-taker-node.service` through systemd.
    Start,
    /// Stop only the packaged `lez-taker-node.service` through systemd.
    Stop,
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

#[derive(Serialize)]
struct XmrPlanOutput {
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

#[allow(clippy::too_many_lines)] // Central CLI dispatch validates every mutually exclusive mode.
async fn execute() -> anyhow::Result<()> {
    let mut arguments = Arguments::parse();
    if let Some(command) = arguments.command.take() {
        return execute_lifecycle(command, &arguments.socket).await;
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
    let delivery_directory = arguments.delivery_directory.as_deref();
    let now_unix_seconds = arguments
        .now_unix_seconds
        .context("discovery requires --now-unix-seconds")?;
    let btc_take_requested = btc_take_was_requested(&arguments);
    let zec_take_requested = zec_take_was_requested(&arguments);
    let xmr_take_requested = xmr_take_was_requested(&arguments);
    let btc_plan_requested = arguments.plan_btc_offer.is_some();
    let xmr_plan_requested = arguments.plan_xmr_offer.is_some();
    ensure!(
        usize::from(btc_take_requested)
            + usize::from(zec_take_requested)
            + usize::from(xmr_take_requested)
            <= 1,
        "exactly one BTC, XMR, or ZEC acceptance may be requested"
    );
    ensure!(
        usize::from(btc_plan_requested)
            + usize::from(xmr_plan_requested)
            + usize::from(btc_take_requested || zec_take_requested || xmr_take_requested)
            <= 1,
        "planning and acceptance operations are mutually exclusive"
    );
    if btc_plan_requested {
        return execute_btc_plan(
            &arguments,
            &expected_maker,
            delivery_directory.context("BTC planning requires --delivery-directory")?,
            now_unix_seconds,
        )
        .await;
    }
    if xmr_plan_requested {
        return execute_xmr_plan(
            &arguments,
            &expected_maker,
            delivery_directory.context("XMR planning requires --delivery-directory")?,
            now_unix_seconds,
        )
        .await;
    }
    if btc_take_requested {
        return execute_btc_take(
            &arguments,
            &expected_maker,
            delivery_directory.context("BTC acceptance requires --delivery-directory")?,
            now_unix_seconds,
        )
        .await;
    }
    if xmr_take_requested {
        return execute_xmr_take(
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
            delivery_directory.context("ZEC acceptance requires --delivery-directory")?,
            now_unix_seconds,
        )
        .await;
    }
    let delivery_directory =
        delivery_directory.context("discovery requires --delivery-directory")?;
    let delivery = RunLocalDelivery::subscriber(delivery_directory.to_path_buf(), expected_maker)?;
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
            .is_none_or(|pair| matches!(pair, PairArgument::Zcash)),
        "ZEC acceptance supports only zcash"
    );
    let direction: SwapDirection = arguments
        .direction
        .unwrap_or(DirectionArgument::TakerSellsLez)
        .into();
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
        direction,
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
            .is_none_or(|pair| matches!(pair, PairArgument::Bitcoin)),
        "BTC acceptance supports only bitcoin"
    );
    let direction = arguments
        .direction
        .unwrap_or(DirectionArgument::TakerSellsForeign);
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
    let contribution_files = arguments
        .maker_contribution_file
        .as_deref()
        .zip(arguments.taker_contribution_file.as_deref());
    let role_root = if contribution_files.is_some() {
        ensure!(
            arguments.btc_source_taker_config.is_none()
                && arguments.btc_taker_actor_root.is_none()
                && arguments.btc_acceptance_receipt.is_none(),
            "BTC Chat v2 role acceptance does not accept fixture actor authority"
        );
        Some(required_btc_path(
            arguments.btc_role_root.as_deref(),
            "--btc-role-root",
        )?)
    } else {
        ensure!(
            arguments.btc_role_root.is_none(),
            "--btc-role-root requires contribution-bound BTC Chat v2"
        );
        None
    };
    let output = take_btc(BtcTakeInput {
        direction: direction.into(),
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
        contribution_files,
        role_root,
        taker_signing_key_file: required_btc_path(
            arguments.taker_signing_key_file.as_deref(),
            "--taker-signing-key-file",
        )?,
        agreement_output_file,
        source_taker_config_file: arguments.btc_source_taker_config.as_deref(),
        taker_actor_root: arguments.btc_taker_actor_root.as_deref(),
        acceptance_receipt_file: arguments.btc_acceptance_receipt.as_deref(),
    })
    .await?;
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

async fn execute_xmr_take(
    arguments: &Arguments,
    expected_maker: &PublicKey,
    delivery_directory: Option<&std::path::Path>,
    now_unix_seconds: u64,
) -> anyhow::Result<()> {
    ensure!(
        arguments
            .pair
            .is_none_or(|pair| matches!(pair, PairArgument::Monero))
            && arguments
                .direction
                .is_none_or(|direction| matches!(direction, DirectionArgument::TakerSellsLez)),
        "M5 XMR acceptance supports only monero/taker-sells-lez"
    );
    let actor_root = required_xmr_path(
        arguments.xmr_taker_actor_root.as_deref(),
        "--xmr-taker-actor-root",
    )?;
    let durable_actor = xmr_actor_bundle_is_durable(actor_root)?;
    let delivery = if durable_actor {
        None
    } else {
        Some(RunLocalDelivery::subscriber(
            delivery_directory
                .context("fresh XMR acceptance requires --delivery-directory")?
                .to_path_buf(),
            expected_maker.to_owned(),
        )?)
    };
    let effect = match (
        arguments.xmr_effect_authority_file.as_deref(),
        arguments.xmr_effect_manifest_file.as_deref(),
        arguments.xmr_workflow_journal.as_deref(),
        arguments.xmr_run_id.as_deref(),
    ) {
        (None, None, None, None) => None,
        (
            Some(effect_authority_file),
            Some(effect_manifest_file),
            Some(workflow_journal),
            Some(run_id),
        ) => Some(XmrEffectTakeInput {
            effect_authority_file,
            effect_manifest_file,
            workflow_journal,
            run_id,
        }),
        _ => anyhow::bail!(
            "effect-capable XMR acceptance requires --xmr-effect-authority-file, --xmr-effect-manifest-file, --xmr-workflow-journal, and --xmr-run-id together"
        ),
    };
    let output = take_xmr(XmrTakeInput {
        delivery: delivery.as_ref(),
        now_unix_seconds,
        offer_id: arguments
            .accept_xmr_offer
            .as_deref()
            .context("XMR acceptance requires --accept-xmr-offer")?,
        chat_socket: required_xmr_path(arguments.chat_socket.as_deref(), "--chat-socket")?,
        reservation_id: arguments
            .reservation_id
            .as_deref()
            .context("XMR acceptance requires --reservation-id")?,
        foreign_units: arguments
            .foreign_units
            .context("XMR acceptance requires --foreign-units")?,
        stage_a_file: required_xmr_path(
            arguments.xmr_stage_a_file.as_deref(),
            "--xmr-stage-a-file",
        )?,
        activation_file: required_xmr_path(
            arguments.xmr_activation_file.as_deref(),
            "--xmr-activation-file",
        )?,
        source_taker_root: required_xmr_path(
            arguments.xmr_source_taker_root.as_deref(),
            "--xmr-source-taker-root",
        )?,
        taker_public_packet: required_xmr_path(
            arguments.xmr_taker_public_packet.as_deref(),
            "--xmr-taker-public-packet",
        )?,
        maker_public_packet: required_xmr_path(
            arguments.xmr_maker_public_packet.as_deref(),
            "--xmr-maker-public-packet",
        )?,
        taker_role_journal: required_xmr_path(
            arguments.xmr_taker_role_journal.as_deref(),
            "--xmr-taker-role-journal",
        )?,
        taker_actor_root: actor_root,
        acceptance_receipt_file: required_xmr_path(
            arguments.xmr_acceptance_receipt.as_deref(),
            "--xmr-acceptance-receipt",
        )?,
        effect,
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
            .is_none_or(|pair| matches!(pair, PairArgument::Bitcoin)),
        "BTC planning supports only bitcoin"
    );
    ensure!(
        arguments.chat_socket.is_none()
            && arguments.unsigned_draft_file.is_none()
            && arguments.maker_contribution_file.is_none()
            && arguments.taker_contribution_file.is_none()
            && arguments.btc_role_root.is_none()
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
    let direction = arguments
        .direction
        .unwrap_or(DirectionArgument::TakerSellsForeign);
    let route = MakerRouteV1::new(Pair::Bitcoin, direction.into())?;
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

async fn execute_xmr_plan(
    arguments: &Arguments,
    expected_maker: &PublicKey,
    delivery_directory: &std::path::Path,
    now_unix_seconds: u64,
) -> anyhow::Result<()> {
    ensure!(
        arguments
            .pair
            .is_none_or(|pair| matches!(pair, PairArgument::Monero))
            && arguments
                .direction
                .is_none_or(|direction| matches!(direction, DirectionArgument::TakerSellsLez)),
        "M5 XMR planning supports only monero/taker-sells-lez"
    );
    ensure!(
        arguments.chat_socket.is_none()
            && arguments.unsigned_draft_file.is_none()
            && arguments.maker_contribution_file.is_none()
            && arguments.taker_contribution_file.is_none()
            && arguments.btc_role_root.is_none()
            && arguments.taker_signing_key_file.is_none()
            && arguments.agreement_output_file.is_none()
            && arguments.zec_source_taker_config.is_none()
            && arguments.zec_taker_actor_root.is_none()
            && arguments.zec_acceptance_receipt.is_none()
            && arguments.btc_source_taker_config.is_none()
            && arguments.btc_taker_actor_root.is_none()
            && arguments.btc_acceptance_receipt.is_none(),
        "XMR planning accepts no Chat, signing, agreement, actor, or receipt authority"
    );
    let offer_id = arguments
        .plan_xmr_offer
        .as_deref()
        .context("XMR planning requires --plan-xmr-offer")?;
    let reservation_id = RequestId::new(
        arguments
            .reservation_id
            .as_deref()
            .context("XMR planning requires --reservation-id")?,
    )?;
    let foreign_units = arguments
        .foreign_units
        .context("XMR planning requires --foreign-units")?;
    ensure!(foreign_units > 0, "XMR principal must be nonzero");
    let route = MakerRouteV1::new(Pair::Monero, SwapDirection::TakerSellsLez)?;
    let delivery =
        RunLocalDelivery::subscriber(delivery_directory.to_path_buf(), expected_maker.to_owned())?;
    let selected = delivery
        .discover(&DeliveryOfferQueryV1::for_route(route, now_unix_seconds))
        .await?
        .into_iter()
        .find(|candidate| candidate.offer().id().as_str() == offer_id)
        .context("selected XMR offer is unavailable, expired, or not authentic")?;
    let lez_units = selected.offer().quote_foreign_amount(foreign_units)?;
    let commitment = selected.commitment();
    let output = XmrPlanOutput {
        schema_version: 1,
        offer_id: offer_id.to_owned(),
        reservation_id: reservation_id.as_str().to_owned(),
        signed_envelope_sha256: hex::encode(commitment),
        swap_id: hex::encode(maker_xmr_chat_swap_id(&commitment, &reservation_id)),
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
    XmrMonitor(Box<XmrTakerReceiptSelector>),
    XmrEffect(Box<XmrTakerEffectReceiptSelector>),
}

#[derive(Serialize)]
struct BtcLifecycleOutput {
    pair: &'static str,
    #[serde(flatten)]
    output: btc_reference_actor::ActorCommandOutputV1,
}

#[derive(Serialize)]
struct XmrTakerMonitorOutput {
    schema_version: u16,
    pair: &'static str,
    role: &'static str,
    state: &'static str,
    phase: &'static str,
    claim_session: &'static str,
    refund_session: &'static str,
}

#[derive(Serialize)]
struct XmrTakerEffectMonitorOutput<'a> {
    schema_version: u16,
    pair: &'static str,
    role: &'static str,
    state: &'static str,
    phase: &'static str,
    run_id: &'a str,
    effect_authority: &'static str,
}

#[derive(Serialize)]
struct XmrTakerEffectActionOutput<'a> {
    schema_version: u16,
    pair: &'static str,
    role: &'static str,
    action: &'static str,
    step: &'static str,
    state: &'static str,
    run_id: &'a str,
    tool_plan_identity_sha256: String,
    chain_effect_finalized: bool,
}

async fn execute_lifecycle(
    command: LifecycleCommand,
    socket: &std::path::Path,
) -> anyhow::Result<()> {
    match command {
        LifecycleCommand::Health => execute_taker_health(socket).await,
        LifecycleCommand::Start => execute_taker_service_action(NodeServiceAction::Start),
        LifecycleCommand::Stop => execute_taker_service_action(NodeServiceAction::Stop),
        LifecycleCommand::Monitor(arguments) => {
            execute_actor_lifecycle(&arguments, LifecycleAction::Monitor).await
        }
        LifecycleCommand::Claim(arguments) => {
            execute_actor_lifecycle(&arguments, LifecycleAction::Claim).await
        }
        LifecycleCommand::Refund(arguments) => {
            execute_actor_lifecycle(&arguments, LifecycleAction::Refund).await
        }
    }
}

async fn execute_taker_health(socket: &std::path::Path) -> anyhow::Result<()> {
    let output: serde_json::Value = call_local_rpc(
        socket,
        "taker_health",
        &serde_json::json!({"schema_version": 1}),
    )
    .await?;
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

fn execute_taker_service_action(action: NodeServiceAction) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string(&control_taker_service(action)?)?
    );
    Ok(())
}

async fn execute_actor_lifecycle(
    arguments: &LifecycleArguments,
    action: LifecycleAction,
) -> anyhow::Result<()> {
    match load_taker_actor(arguments)? {
        LoadedTakerActor::Zec(config) => execute_zec_lifecycle(&config, action).await,
        LoadedTakerActor::Btc(config) => execute_btc_lifecycle(&config, action).await,
        LoadedTakerActor::XmrMonitor(selector) => execute_xmr_lifecycle(&selector, action),
        LoadedTakerActor::XmrEffect(selector) => execute_xmr_effect_lifecycle(&selector, action),
    }
}

fn load_taker_actor(arguments: &LifecycleArguments) -> anyhow::Result<LoadedTakerActor> {
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
            load_taker_actor_from_receipt(path).ok(),
            load_btc_taker_actor_from_receipt(path).ok(),
            load_xmr_taker_receipt_selector(path).ok(),
            load_xmr_taker_effect_receipt_selector(path).ok(),
        ) {
            (Some(config), None, None, None) => LoadedTakerActor::Zec(Box::new(config)),
            (None, Some(config), None, None) => LoadedTakerActor::Btc(Box::new(config)),
            (None, None, Some(selector), None) => LoadedTakerActor::XmrMonitor(Box::new(selector)),
            (None, None, None, Some(selector)) => LoadedTakerActor::XmrEffect(Box::new(selector)),
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
    Ok(config)
}

async fn execute_zec_lifecycle(
    config: &ActorConfig,
    action: LifecycleAction,
) -> anyhow::Result<()> {
    ensure!(
        config.role() == ActorRole::Taker,
        "Taker actor configuration has the wrong role"
    );
    let _held_lock = MakerActorHeldLock::acquire_for(config.swap_id(), config.role_state_db())
        .map_err(|_| anyhow::anyhow!("Taker actor is already running or unsafe"))?;
    let command = match action {
        LifecycleAction::Monitor => ZecActorCommand::Status,
        LifecycleAction::Claim => ZecActorCommand::Claim,
        LifecycleAction::Refund => ZecActorCommand::Recover,
    };
    let output = execute_actor_command(config, command)
        .await
        .map_err(|_| anyhow::anyhow!("Taker lifecycle command failed"))?;
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

async fn execute_btc_lifecycle(
    config: &BtcActorConfig,
    action: LifecycleAction,
) -> anyhow::Result<()> {
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
    let output = execute_btc_actor_command(config, command)
        .await
        .map_err(|_| anyhow::anyhow!("BTC Taker lifecycle command failed"))?;
    println!(
        "{}",
        serde_json::to_string(&BtcLifecycleOutput {
            pair: "bitcoin",
            output
        })?
    );
    Ok(())
}

fn execute_xmr_lifecycle(
    selector: &XmrTakerReceiptSelector,
    action: LifecycleAction,
) -> anyhow::Result<()> {
    let _held_lock = MakerActorHeldLock::acquire_for(selector.swap_id(), selector.state_database())
        .map_err(|_| anyhow::anyhow!("XMR Taker actor is already running or unsafe"))?;
    let authority = load_validated_xmr_taker_authority_bytes(selector.manifest_bytes())
        .map_err(|_| anyhow::anyhow!("XMR Taker actor authority is unavailable or unsafe"))?;
    ensure!(
        selector.receipt_matches(&authority),
        "receipt-bound XMR Taker actor semantics changed"
    );
    ensure!(
        matches!(action, LifecycleAction::Monitor),
        "XMR Taker claim and refund are not yet composed"
    );
    println!(
        "{}",
        serde_json::to_string(&XmrTakerMonitorOutput {
            schema_version: 1,
            pair: "monero",
            role: "taker",
            state: "active",
            phase: "application_activated",
            claim_session: "presignature_verified",
            refund_session: "presignature_verified",
        })?
    );
    Ok(())
}

fn execute_xmr_effect_lifecycle(
    selector: &XmrTakerEffectReceiptSelector,
    action: LifecycleAction,
) -> anyhow::Result<()> {
    let state_lock = MakerActorHeldLock::acquire_for(selector.swap_id(), selector.state_database())
        .map_err(|_| anyhow::anyhow!("XMR Taker actor is already running or unsafe"))?;
    let workflow_lock =
        MakerActorHeldLock::acquire_for(selector.swap_id(), selector.workflow_journal())
            .map_err(|_| anyhow::anyhow!("XMR Taker workflow is already running or unsafe"))?;
    let execution = selector
        .validate_execution()
        .map_err(|_| anyhow::anyhow!("XMR Taker effect authority is unavailable or unsafe"))?;
    match action {
        LifecycleAction::Monitor => println!(
            "{}",
            serde_json::to_string(&XmrTakerEffectMonitorOutput {
                schema_version: 2,
                pair: "monero",
                role: "taker",
                state: "active",
                phase: "application_activated",
                run_id: selector.run_id(),
                effect_authority: "validated",
            })?
        ),
        LifecycleAction::Claim => {
            let step = select_xmr_taker_claim_step(&execution)?;
            execute_xmr_taker_effect(
                selector.run_id(),
                &execution,
                step,
                &state_lock,
                &workflow_lock,
            )?;
        }
        LifecycleAction::Refund => execute_xmr_taker_effect(
            selector.run_id(),
            &execution,
            XmrWorkflowStep::RefundLezTag16,
            &state_lock,
            &workflow_lock,
        )?,
    }
    Ok(())
}

fn select_xmr_taker_claim_step(
    execution: &ValidatedXmrEffectExecutionV3,
) -> anyhow::Result<XmrWorkflowStep> {
    let workflow =
        SqliteXmrWorkflowJournal::open_existing(execution.effect_authority().workflow_journal())
            .map_err(|_| anyhow::anyhow!("XMR Taker workflow is unavailable or unsafe"))?;
    match workflow.step_revision(
        execution.workflow_identity(),
        XmrWorkflowStep::SweepMoneroClaim,
    ) {
        Ok(_) => Ok(XmrWorkflowStep::SweepMoneroClaim),
        Err(StoreError::MissingXmrWorkflowStep) => Ok(XmrWorkflowStep::AuthorizeLezTag14),
        Err(_) => Err(anyhow::anyhow!(
            "XMR Taker claim workflow step is unavailable or unsafe"
        )),
    }
}

fn execute_xmr_taker_effect(
    run_id: &str,
    execution: &ValidatedXmrEffectExecutionV3,
    step: XmrWorkflowStep,
    state_lock: &MakerActorHeldLock,
    workflow_lock: &MakerActorHeldLock,
) -> anyhow::Result<()> {
    let (action, step_name) = match step {
        XmrWorkflowStep::AuthorizeLezTag14 => ("claim", "authorize_lez_tag14"),
        XmrWorkflowStep::SweepMoneroClaim => ("claim", "sweep_monero_claim"),
        XmrWorkflowStep::RefundLezTag16 => ("refund", "refund_lez_tag16"),
        _ => return Err(anyhow::anyhow!("XMR Taker effect step is unsupported")),
    };
    if step == XmrWorkflowStep::RefundLezTag16
        || (step == XmrWorkflowStep::AuthorizeLezTag14
            && execution.effect_authority().tag14_release().is_some())
    {
        execute_xmr_taker_preflight(execution, step, action, state_lock, workflow_lock)?;
    }
    let prepared = execution
        .prepare_effect_invocation(step, state_lock, workflow_lock)
        .map_err(|_| anyhow::anyhow!("XMR Taker effect route is unavailable or unsafe"))?;
    let (state, plan, finalized) = match prepared {
        XmrPreparedEffectInvocationV1::InvokeOnce {
            mut command,
            tool_plan_identity_sha256,
        } => {
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let Ok(mut child) = command.spawn() else {
                mark_xmr_effect_unknown(execution, step)?;
                return Err(anyhow::anyhow!("XMR Taker effect invocation is ambiguous"));
            };
            let status = match child.wait_timeout(Duration::from_secs(30)) {
                Ok(Some(status)) => status,
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    mark_xmr_effect_unknown(execution, step)?;
                    return Err(anyhow::anyhow!("XMR Taker effect invocation timed out"));
                }
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    mark_xmr_effect_unknown(execution, step)?;
                    return Err(anyhow::anyhow!("XMR Taker effect invocation is ambiguous"));
                }
            };
            if !status.success() {
                mark_xmr_effect_unknown(execution, step)?;
                return Err(anyhow::anyhow!("XMR Taker effect invocation is ambiguous"));
            }
            #[cfg(not(feature = "test-crash-hooks"))]
            pause_xmr_taker_after_invoked(step);
            #[cfg(feature = "test-crash-hooks")]
            pause_xmr_taker_after_invoked(step)?;
            ("invoked_unreconciled", tool_plan_identity_sha256, false)
        }
        XmrPreparedEffectInvocationV1::ObserveOnly {
            tool_plan_identity_sha256,
        } => observe_xmr_taker_effect(
            execution,
            step,
            tool_plan_identity_sha256,
            state_lock,
            workflow_lock,
        )?,
        XmrPreparedEffectInvocationV1::Complete {
            tool_plan_identity_sha256,
        } => ("complete", tool_plan_identity_sha256, true),
    };
    println!(
        "{}",
        serde_json::to_string(&XmrTakerEffectActionOutput {
            schema_version: 3,
            pair: "monero",
            role: "taker",
            action,
            step: step_name,
            state,
            run_id,
            tool_plan_identity_sha256: hex::encode(plan),
            chain_effect_finalized: finalized,
        })?
    );
    Ok(())
}

#[cfg(not(feature = "test-crash-hooks"))]
fn pause_xmr_taker_after_invoked(_step: XmrWorkflowStep) {}

#[cfg(feature = "test-crash-hooks")]
fn pause_xmr_taker_after_invoked(step: XmrWorkflowStep) -> anyhow::Result<()> {
    let Some(expected_step) = env::var_os("LEZ_TAKER_TEST_PAUSE_AFTER_INVOKED_STEP") else {
        return Ok(());
    };
    ensure!(
        expected_step == step.name(),
        "Taker test pause selected a different workflow step"
    );
    let marker = PathBuf::from(
        env::var_os("LEZ_TAKER_TEST_PAUSE_MARKER")
            .ok_or_else(|| anyhow::anyhow!("Taker test pause marker is unavailable"))?,
    );
    ensure!(marker.is_absolute(), "Taker test pause marker is unsafe");
    let parent = marker
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Taker test pause marker is unsafe"))?;
    let parent_metadata = fs::symlink_metadata(parent)
        .map_err(|_| anyhow::anyhow!("Taker test pause marker parent is unavailable"))?;
    ensure!(
        parent_metadata.is_dir()
            && parent_metadata.permissions().mode().trailing_zeros() >= 6
            && parent_metadata.uid() == rustix::process::geteuid().as_raw()
            && fs::canonicalize(parent).ok().as_deref() == Some(parent),
        "Taker test pause marker parent is unsafe"
    );
    ensure!(
        matches!(fs::symlink_metadata(&marker), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
        "Taker test pause marker is not create-new"
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&marker)
        .map_err(|_| anyhow::anyhow!("Taker test pause marker could not be created"))?;
    serde_json::to_writer(
        &mut file,
        &serde_json::json!({
            "schema_version": 1,
            "state": "paused_after_invoked_before_stdout",
            "step": step.name(),
            "process_id": std::process::id()
        }),
    )?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    loop {
        thread::park();
    }
}

fn execute_xmr_taker_preflight(
    execution: &ValidatedXmrEffectExecutionV3,
    step: XmrWorkflowStep,
    action: &'static str,
    state_lock: &MakerActorHeldLock,
    workflow_lock: &MakerActorHeldLock,
) -> anyhow::Result<()> {
    let Some(mut command) = execution
        .prepare_effect_preflight(step, state_lock, workflow_lock)
        .map_err(|_| anyhow::anyhow!("XMR Taker effect route is unavailable or unsafe"))?
    else {
        return Ok(());
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|_| anyhow::anyhow!("XMR Taker {action} preflight is unavailable"))?;
    let status = match child.wait_timeout(Duration::from_secs(30)) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow::anyhow!("XMR Taker {action} preflight timed out"));
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow::anyhow!(
                "XMR Taker {action} preflight is unavailable"
            ));
        }
    };
    ensure!(
        status.success(),
        "XMR Taker {action} is not yet eligible or its preflight failed"
    );
    Ok(())
}

fn observe_xmr_taker_effect(
    execution: &ValidatedXmrEffectExecutionV3,
    step: XmrWorkflowStep,
    expected_plan: [u8; 32],
    state_lock: &MakerActorHeldLock,
    workflow_lock: &MakerActorHeldLock,
) -> anyhow::Result<(&'static str, [u8; 32], bool)> {
    let prepared = execution
        .prepare_effect_observation(step, state_lock, workflow_lock)
        .map_err(|_| anyhow::anyhow!("XMR Taker effect observation is unavailable or unsafe"))?;
    let (mut command, plan, source) = prepared.into_parts();
    ensure!(
        plan == expected_plan,
        "XMR Taker effect observation plan changed"
    );
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|_| anyhow::anyhow!("XMR Taker effect observation is unavailable"))?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(anyhow::anyhow!(
            "XMR Taker effect observer output is unavailable"
        ));
    };
    let status = match child.wait_timeout(XMR_EFFECT_OBSERVATION_TIMEOUT) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow::anyhow!("XMR Taker effect observation timed out"));
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow::anyhow!(
                "XMR Taker effect observation is unavailable"
            ));
        }
    };
    ensure!(
        status.success(),
        "XMR Taker effect observation is unavailable"
    );
    let mut bytes = Vec::new();
    stdout
        .by_ref()
        .take((XMR_EFFECT_OBSERVER_RESULT_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("read XMR Taker effect observation")?;
    ensure!(
        bytes.len() <= XMR_EFFECT_OBSERVER_RESULT_MAX_BYTES,
        "XMR Taker effect observation is oversized"
    );
    let result = parse_xmr_effect_observer_result_v1(&bytes, step)
        .map_err(|_| anyhow::anyhow!("XMR Taker effect observation is invalid"))?;
    match result.state() {
        XmrEffectObserverStateV1::Pending => Ok(("observe_only", plan, false)),
        XmrEffectObserverStateV1::Finalized => {
            let evidence = result
                .effect_evidence_sha256()
                .context("finalized XMR Taker effect lacks evidence")?;
            let reconciliation = XmrWorkflowReconciliationV2::new(evidence, plan, source)
                .map_err(|_| anyhow::anyhow!("XMR Taker effect evidence is invalid"))?;
            let mut workflow = SqliteXmrWorkflowJournal::open_existing(
                execution.effect_authority().workflow_journal(),
            )
            .map_err(|_| anyhow::anyhow!("XMR Taker workflow recovery is unavailable"))?;
            workflow
                .validate_initialized(execution.workflow_identity())
                .map_err(|_| anyhow::anyhow!("XMR Taker workflow recovery identity changed"))?;
            workflow
                .reconcile_succeeded(execution.workflow_identity(), step, &reconciliation)
                .map_err(|_| anyhow::anyhow!("XMR Taker effect reconciliation failed"))?;
            Ok(("complete", plan, true))
        }
    }
}

fn mark_xmr_effect_unknown(
    execution: &ValidatedXmrEffectExecutionV3,
    step: XmrWorkflowStep,
) -> anyhow::Result<()> {
    let mut workflow =
        SqliteXmrWorkflowJournal::open_existing(execution.effect_authority().workflow_journal())
            .map_err(|_| anyhow::anyhow!("XMR Taker workflow recovery is unavailable"))?;
    workflow
        .validate_initialized(execution.workflow_identity())
        .map_err(|_| anyhow::anyhow!("XMR Taker workflow recovery identity changed"))?;
    workflow
        .mark_unknown(execution.workflow_identity(), step)
        .map_err(|_| anyhow::anyhow!("XMR Taker workflow ambiguity is unavailable"))
}

fn btc_take_was_requested(arguments: &Arguments) -> bool {
    arguments.accept_btc_offer.is_some()
        || arguments.btc_source_taker_config.is_some()
        || arguments.btc_taker_actor_root.is_some()
        || arguments.btc_acceptance_receipt.is_some()
        || arguments.btc_role_root.is_some()
        || arguments.maker_contribution_file.is_some()
        || arguments.taker_contribution_file.is_some()
}

fn xmr_actor_bundle_is_durable(path: &std::path::Path) -> anyhow::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("inspect persisted XMR Taker actor"),
    }
}

fn xmr_take_was_requested(arguments: &Arguments) -> bool {
    arguments.accept_xmr_offer.is_some()
        || arguments.xmr_stage_a_file.is_some()
        || arguments.xmr_activation_file.is_some()
        || arguments.xmr_source_taker_root.is_some()
        || arguments.xmr_taker_public_packet.is_some()
        || arguments.xmr_maker_public_packet.is_some()
        || arguments.xmr_taker_role_journal.is_some()
        || arguments.xmr_taker_actor_root.is_some()
        || arguments.xmr_acceptance_receipt.is_some()
        || arguments.xmr_effect_authority_file.is_some()
        || arguments.xmr_effect_manifest_file.is_some()
        || arguments.xmr_workflow_journal.is_some()
        || arguments.xmr_run_id.is_some()
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
        || arguments.maker_contribution_file.is_some()
        || arguments.taker_contribution_file.is_some()
        || arguments.taker_signing_key_file.is_some()
        || arguments.agreement_output_file.is_some()
}

fn required_path<'a>(
    value: Option<&'a std::path::Path>,
    flag: &str,
) -> anyhow::Result<&'a std::path::Path> {
    value.with_context(|| format!("ZEC acceptance requires {flag}"))
}

fn required_xmr_path<'a>(
    value: Option<&'a std::path::Path>,
    flag: &str,
) -> anyhow::Result<&'a std::path::Path> {
    value.with_context(|| format!("XMR acceptance requires {flag}"))
}

fn required_btc_path<'a>(
    value: Option<&'a std::path::Path>,
    flag: &str,
) -> anyhow::Result<&'a std::path::Path> {
    value.with_context(|| format!("BTC acceptance requires {flag}"))
}

#[cfg(test)]
mod xmr_cli_tests {
    use super::xmr_actor_bundle_is_durable;

    #[test]
    fn actor_root_becomes_delivery_free_crash_latch_only_after_publication() {
        let parent = tempfile::tempdir().expect("temporary owner root");
        let actor_root = parent.path().join("xmr-taker-actor");
        assert!(!xmr_actor_bundle_is_durable(&actor_root).expect("inspect absent actor"));

        std::fs::create_dir(&actor_root).expect("publish actor-root latch");
        assert!(xmr_actor_bundle_is_durable(&actor_root).expect("inspect durable actor"));
    }
}
