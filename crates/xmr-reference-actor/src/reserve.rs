//! Take-time composition of one Monero agreement over Chat (`xmr_reserve_v1`).
//!
//! The Zcash model (`zec-reference-actor/src/reserve.rs`): the Taker reserves
//! at take time and the Maker answers with the facts only it holds. Each role
//! makes its own per-swap material in its own role root; the Taker also makes
//! the per-swap private view key and hands it to the Maker, which imports it
//! (the reference flow's direction). The unsigned Stage A commits LEZ message
//! hashes only the LEZ v0.2 libraries compute, so both roles run the same
//! chain-free planner ([`XmrStageAPlanner`]) on identical input: the Maker
//! returns its plan and signature, the Taker reruns the planner with its own
//! facts and the Maker's few and requires byte equality before it signs.

use std::{
    collections::BTreeMap,
    fs::File,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result, anyhow, ensure};
use lez_bridge_protocol::{Hex32, RequestId, RuntimeDescriptor};
use lez_node_common::{XmrReserveRequestV1, XmrReserveResponseV1};
use lez_swap_store::MakerOfferId;
use lez_xmr_monero_adapter::LoopbackRpcEndpoint;
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES,
    MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES, MoneroPrivateViewKey, ValidatedXmrAgreementBodyV1,
    XmrAgreementBodyV1, XmrAgreementV1, XmrNamedProfileV1, XmrRoleV1, XmrWindowsV1,
};
use rustix::{
    fs::{Mode, OFlags, openat},
    io::Errno,
};
use secp256k1::{Keypair, Message, Secp256k1};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::{
    ActorRole, FilePolicy, ROLE_PACKET_MAX_BYTES, SecureDestination, StageRolePackets,
    VIEW_KEY_FILE, ValidatedRolePacket, generate_role_material, open_path_no_symlinks,
    open_private_directory, provision_role_root, read_bounded_file, read_private_manifest_at,
    read_secret_hex_at, validate_stage_a_packets, write_new_at,
};

const ROLE_CONFIG_SCHEMA_VERSION: u16 = 1;
const STAGE_A_PLAN_SCHEMA_VERSION: u16 = 1;
/// The schema of `xmr_reserve_v1` and the ceremony rounds over Chat.
pub(crate) const XMR_CHAT_SCHEMA_VERSION: u16 = 1;
const MAX_ROLE_CONFIG_BYTES: u64 = 16 * 1024;
const MAX_LOGIN_FILE_BYTES: u64 = 256;
const MAX_WALLET_NAME_BYTES: usize = 64;
/// How far the Maker's LEZ anchor time may sit from the Taker's clock.
pub const MAX_ANCHOR_CLOCK_SKEW_SECONDS: u64 = 120;
/// How far above the Taker's own monerod tip the Maker's restore height may
/// sit (both read the one devnet chain, a few blocks apart).
pub const MAX_RESTORE_HEIGHT_SKEW_BLOCKS: u64 = 3;
/// Both roles' public packets, kept in each role root for provisioning.
pub const XMR_MAKER_PACKET_FILE: &str = "maker-packet.json";
pub const XMR_TAKER_PACKET_FILE: &str = "taker-packet.json";
/// The unsigned Stage A both roles composed, then the countersigned one.
pub const XMR_STAGE_A_UNSIGNED_FILE: &str = "stage-a.unsigned";
pub const XMR_STAGE_A_FILE: &str = "stage-a.wire";
/// The countersigned Stage B (activation).
pub const XMR_STAGE_B_FILE: &str = "stage-b.wire";
/// The role's adaptor journal holding both the claim and the refund session.
pub const XMR_ADAPTOR_JOURNAL_FILE: &str = "adaptor.sqlite3";

/// The only Monero profile a role takes swaps under (public Stagenet is out of
/// scope).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum XmrProfileName {
    AcceleratedRegtest,
}

/// A Node's Monero role: its monerod and wallet-rpc, the recovery windows it
/// takes swaps under and its Stage-A planner. One file, owner-private
/// (`--xmr-role-config` on the Maker, `initiation.xmr_role_config` on the
/// Taker).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XmrRoleConfigV1 {
    schema_version: u16,
    profile: XmrProfileName,
    /// Literal-loopback monerod root (`http://127.0.0.1:18081/`).
    monerod_endpoint: String,
    /// Owner-private `user:password`, as monerod's `--rpc-login`.
    monerod_login_file: PathBuf,
    /// Literal-loopback wallet-rpc root of this role.
    wallet_rpc_endpoint: String,
    wallet_rpc_login_file: PathBuf,
    /// The role wallet the wallet-rpc opens (`maker` or `taker`).
    wallet_name: String,
    /// Absent means the empty password (devnet).
    #[serde(default)]
    wallet_password_file: Option<PathBuf>,
    /// Offsets from the LEZ anchor: the Maker's funding cutoff `C`, the refund
    /// time `R` and the punish time `P`. Both roles must use the same three.
    funding_cutoff_seconds: u64,
    refund_seconds: u64,
    punish_seconds: u64,
    /// The chain-free `lez-v02-xmr-stage-a-plan` binary, pinned at load.
    stage_a_planner: PathBuf,
    /// Per-slot effect tool programs; their digests are taken at Node start.
    #[serde(default)]
    effect_tools: BTreeMap<String, PathBuf>,
    #[serde(skip)]
    stage_a_planner_sha256: [u8; 32],
}

impl std::fmt::Debug for XmrRoleConfigV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("XmrRoleConfigV1")
            .field("profile", &self.profile)
            .field("monerod_endpoint", &self.monerod_endpoint)
            .field("wallet_rpc_endpoint", &self.wallet_rpc_endpoint)
            .field("wallet_name", &self.wallet_name)
            .finish_non_exhaustive()
    }
}

impl XmrRoleConfigV1 {
    /// Loads and checks one owner-private role configuration and pins its
    /// Stage-A planner.
    ///
    /// # Errors
    ///
    /// Fails for an unsafe or malformed file, an unknown profile, windows out
    /// of order, an endpoint that is not literal loopback, an unsafe login
    /// file, or an unreadable planner.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = read_bounded_file(
            open_path_no_symlinks(path, "Monero role config")?,
            MAX_ROLE_CONFIG_BYTES,
            FilePolicy::Private,
            "Monero role config",
        )?;
        let mut config: Self =
            serde_json::from_slice(&bytes).context("Monero role config is invalid")?;
        ensure!(
            config.schema_version == ROLE_CONFIG_SCHEMA_VERSION,
            "unsupported Monero role config schema"
        );
        ensure!(
            0 < config.funding_cutoff_seconds
                && config.funding_cutoff_seconds < config.refund_seconds
                && config.refund_seconds < config.punish_seconds,
            "the Monero windows are not funding cutoff < refund < punish"
        );
        config.monerod()?;
        config.wallet_rpc()?;
        ensure!(
            !config.wallet_name.is_empty()
                && config.wallet_name.len() <= MAX_WALLET_NAME_BYTES
                && config
                    .wallet_name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
            "the Monero wallet name is invalid"
        );
        config.wallet_password()?;
        ensure!(
            config.effect_tools.values().all(|path| path.is_absolute()),
            "a Monero effect tool path is not absolute"
        );
        ensure!(
            config.stage_a_planner.is_absolute(),
            "the Stage-A planner path is not absolute"
        );
        config.stage_a_planner_sha256 = file_sha256(&config.stage_a_planner)?;
        Ok(config)
    }

    /// The profile every swap of this role is taken under.
    #[must_use]
    pub const fn profile(&self) -> XmrNamedProfileV1 {
        match self.profile {
            XmrProfileName::AcceleratedRegtest => XmrNamedProfileV1::AcceleratedRegtest,
        }
    }

    /// This role's monerod.
    ///
    /// # Errors
    ///
    /// Fails for an unsafe login file or an endpoint that is not literal loopback.
    pub fn monerod(&self) -> Result<LoopbackRpcEndpoint> {
        endpoint(&self.monerod_endpoint, &self.monerod_login_file, "monerod")
    }

    /// This role's wallet-rpc.
    ///
    /// # Errors
    ///
    /// Fails for an unsafe login file or an endpoint that is not literal loopback.
    pub fn wallet_rpc(&self) -> Result<LoopbackRpcEndpoint> {
        endpoint(
            &self.wallet_rpc_endpoint,
            &self.wallet_rpc_login_file,
            "wallet-rpc",
        )
    }

    /// The role wallet's name in its wallet-rpc.
    #[must_use]
    pub fn wallet_name(&self) -> &str {
        &self.wallet_name
    }

    /// The role wallet's password, empty when no file is named.
    ///
    /// # Errors
    ///
    /// Fails for an unsafe or non-UTF-8 password file.
    pub fn wallet_password(&self) -> Result<Zeroizing<String>> {
        let Some(path) = &self.wallet_password_file else {
            return Ok(Zeroizing::new(String::new()));
        };
        read_secret_line(path, "Monero wallet password")
    }

    /// The per-slot effect tool programs.
    #[must_use]
    pub const fn effect_tools(&self) -> &BTreeMap<String, PathBuf> {
        &self.effect_tools
    }

    /// The Stage-A windows `C`, `R` and `P` from the LEZ anchor, in ms.
    ///
    /// # Errors
    ///
    /// Fails on overflow.
    pub fn windows(&self, lez_anchor_unix_seconds: u64) -> Result<XmrWindowsV1> {
        let at = |offset: u64| {
            lez_anchor_unix_seconds
                .checked_add(offset)
                .and_then(|seconds| seconds.checked_mul(1_000))
                .context("Monero window overflow")
        };
        Ok(XmrWindowsV1::new(
            at(self.funding_cutoff_seconds)?,
            at(self.refund_seconds)?,
            at(self.punish_seconds)?,
        ))
    }

    /// The pinned Stage-A planner process.
    #[must_use]
    pub fn stage_a_planner(&self) -> XmrProcessStageAPlanner {
        XmrProcessStageAPlanner {
            program: self.stage_a_planner.clone(),
            program_sha256: self.stage_a_planner_sha256,
        }
    }
}

fn endpoint(url: &str, login_file: &Path, label: &'static str) -> Result<LoopbackRpcEndpoint> {
    let login = read_secret_line(login_file, "Monero RPC login")?;
    let (username, password) = login
        .split_once(':')
        .with_context(|| format!("the {label} login is not user:password"))?;
    LoopbackRpcEndpoint::new(url, username, password)
        .with_context(|| format!("the {label} endpoint or login is invalid"))
}

fn read_secret_line(path: &Path, label: &'static str) -> Result<Zeroizing<String>> {
    let bytes = Zeroizing::new(read_bounded_file(
        open_path_no_symlinks(path, label)?,
        MAX_LOGIN_FILE_BYTES,
        FilePolicy::Private,
        label,
    )?);
    let text = std::str::from_utf8(&bytes).with_context(|| format!("{label} is not UTF-8"))?;
    Ok(Zeroizing::new(
        text.strip_suffix('\n').unwrap_or(text).to_owned(),
    ))
}

fn file_sha256(path: &Path) -> Result<[u8; 32]> {
    let bytes = std::fs::read(path).context("the Stage-A planner is unreadable")?;
    Ok(Sha256::digest(bytes).into())
}

/// Everything the Stage-A planner composes from: both canonical packets and
/// the reservation's few public facts (the planner's stdin, as JSON). The
/// claim, refund and punish message hashes it adds are derived from these;
/// the LEZ authenticated-transfer program is compiled into the planner.
/// This is `lez-v02-xmr-stage-a-plan`'s input, field for field.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrStageAPlanInputV1 {
    pub schema_version: u16,
    /// Lowercase hex of the canonical public packet bytes.
    pub maker_packet: String,
    pub taker_packet: String,
    pub swap_id: Hex32,
    pub monero_amount_piconero: u64,
    pub lez_amount: u128,
    pub monero_genesis_hash: Hex32,
    pub lez_channel_id: Hex32,
    pub lez_genesis_hash: Hex32,
    pub maker_xmr_funding_cutoff_ms: u64,
    pub refund_at_ms: u64,
    pub punish_at_ms: u64,
    /// The Maker owner's finalized LEZ nonce; it feeds only the punish hash.
    pub maker_owner_nonce: u128,
}

impl XmrStageAPlanInputV1 {
    const fn windows(&self) -> XmrWindowsV1 {
        XmrWindowsV1::new(
            self.maker_xmr_funding_cutoff_ms,
            self.refund_at_ms,
            self.punish_at_ms,
        )
    }
}

/// Composes the canonical unsigned Stage-A wire from its input. The Nodes run
/// [`XmrProcessStageAPlanner`]; tests use a fixture.
pub trait XmrStageAPlanner {
    /// The canonical unsigned Stage-A wire for `input`.
    ///
    /// # Errors
    ///
    /// Fails when the planner cannot compose the input.
    fn plan(&self, input: &XmrStageAPlanInputV1) -> Result<Vec<u8>>;
}

/// The `lez-v02-xmr-stage-a-plan` child process: one JSON object on stdin,
/// the unsigned Stage-A wire on stdout. Its digest is pinned when the role
/// config loads and checked before every run.
#[derive(Clone, Debug)]
pub struct XmrProcessStageAPlanner {
    program: PathBuf,
    program_sha256: [u8; 32],
}

impl XmrStageAPlanner for XmrProcessStageAPlanner {
    fn plan(&self, input: &XmrStageAPlanInputV1) -> Result<Vec<u8>> {
        ensure!(
            file_sha256(&self.program)? == self.program_sha256,
            "the Stage-A planner changed since the role config loaded"
        );
        let request = serde_json::to_vec(input).context("encode the Stage-A plan input")?;
        let mut child = Command::new(&self.program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("start the Stage-A planner")?;
        let written = child
            .stdin
            .take()
            .context("the Stage-A planner has no stdin")?
            .write_all(&request);
        let output = child
            .wait_with_output()
            .context("wait for the Stage-A planner")?;
        written.context("write the Stage-A plan input")?;
        ensure!(output.status.success(), "the Stage-A planner failed");
        ensure!(
            output.stdout.len() <= MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES,
            "the Stage-A plan is oversized"
        );
        Ok(output.stdout)
    }
}

/// One role's own LEZ facts: the descriptor its sidecar runs under (whose
/// signer account is the role's LEZ owner account).
#[derive(Clone, Debug)]
pub struct XmrLezRole {
    pub runtime: RuntimeDescriptor,
}

impl XmrLezRole {
    const fn account(&self) -> [u8; 32] {
        *self.runtime.signer_account_id.as_bytes()
    }
}

/// What both roles agree before composing: the offer-bound terms.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmrReservationTerms {
    /// `maker_xmr_chat_swap_id(offer_commitment, reservation_id)`.
    pub swap_id: [u8; 32],
    pub amount_piconero: u64,
    pub lez_amount: u128,
}

/// What a role reads from its own monerod at take time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XmrMoneroChainView {
    pub genesis: [u8; 32],
    pub tip_height: u64,
}

/// What the Taker brings to a reservation: its public packet and the
/// per-swap private view key it made.
#[derive(Clone)]
pub struct XmrTakerContribution {
    pub taker_packet: Vec<u8>,
    /// Monero little-endian scalar.
    pub taker_private_view_key: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for XmrTakerContribution {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("XmrTakerContribution")
            .field("taker_packet_bytes", &self.taker_packet.len())
            .finish_non_exhaustive()
    }
}

/// The Maker's answer: its packet, its plan and signature, and the facts
/// only it chose.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmrMakerReservation {
    pub maker_packet: Vec<u8>,
    pub unsigned_stage_a_wire: Vec<u8>,
    pub maker_stage_a_signature: [u8; 64],
    /// The anchor `C`, `R` and `P` count from, Unix seconds.
    pub lez_anchor_unix_seconds: u64,
    pub monero_genesis: [u8; 32],
    /// The Maker's monerod height at reserve time; the funding is later.
    pub monero_restore_height: u64,
    pub maker_owner_nonce: u128,
}

impl XmrTakerContribution {
    /// The Taker's `xmr_reserve_v1` request (R1) carrying this contribution.
    #[must_use]
    pub fn reserve_request(
        &self,
        reservation_id: RequestId,
        offer_id: MakerOfferId,
        signed_offer_envelope: Vec<u8>,
        foreign_units: u64,
    ) -> XmrReserveRequestV1 {
        XmrReserveRequestV1 {
            schema_version: XMR_CHAT_SCHEMA_VERSION,
            reservation_id,
            offer_id,
            signed_offer_envelope,
            foreign_units,
            taker_packet: self.taker_packet.clone(),
            taker_private_view_key: *self.taker_private_view_key,
        }
    }
}

/// The Maker reads the Taker's contribution from its R1 request; the caller
/// checks the schema and the offer.
impl From<&XmrReserveRequestV1> for XmrTakerContribution {
    fn from(request: &XmrReserveRequestV1) -> Self {
        Self {
            taker_packet: request.taker_packet.clone(),
            taker_private_view_key: Zeroizing::new(request.taker_private_view_key),
        }
    }
}

impl XmrMakerReservation {
    /// The Maker's `xmr_reserve_v1` answer (R1) for `swap_id`.
    #[must_use]
    pub fn reserve_response(&self, swap_id: &str, was_replay: bool) -> XmrReserveResponseV1 {
        XmrReserveResponseV1 {
            schema_version: XMR_CHAT_SCHEMA_VERSION,
            was_replay,
            swap_id: swap_id.into(),
            maker_packet: self.maker_packet.clone(),
            unsigned_stage_a_wire: self.unsigned_stage_a_wire.clone(),
            maker_stage_a_signature: self.maker_stage_a_signature,
            lez_anchor_unix_seconds: self.lez_anchor_unix_seconds,
            monero_genesis: self.monero_genesis,
            monero_restore_height: self.monero_restore_height,
            maker_owner_nonce: self.maker_owner_nonce,
        }
    }
}

/// The Taker reads the Maker's reservation from its R1 answer; the caller
/// checks the schema and the swap id, [`check_xmr_taker_role`] the rest.
impl From<&XmrReserveResponseV1> for XmrMakerReservation {
    fn from(response: &XmrReserveResponseV1) -> Self {
        Self {
            maker_packet: response.maker_packet.clone(),
            unsigned_stage_a_wire: response.unsigned_stage_a_wire.clone(),
            maker_stage_a_signature: response.maker_stage_a_signature,
            lez_anchor_unix_seconds: response.lez_anchor_unix_seconds,
            monero_genesis: response.monero_genesis,
            monero_restore_height: response.monero_restore_height,
            maker_owner_nonce: response.maker_owner_nonce,
        }
    }
}

/// What the Taker checks the Maker's answer against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XmrTakerCheck {
    /// The Taker's clock when the answer arrived.
    pub now: u64,
    /// The Taker's own monerod.
    pub monero: XmrMoneroChainView,
    /// The offer's Maker identity, which is its agreement key.
    pub maker_identity: [u8; 33],
}

/// Makes the Taker's per-swap material before it reserves: a new private
/// `role_root` with fresh keys, Monero share and private view key, and its
/// packet beside them.
///
/// # Errors
///
/// Fails when `role_root` exists or cannot be written.
pub fn begin_xmr_taker_role(
    role_root: &Path,
    lez_account: [u8; 32],
) -> Result<XmrTakerContribution> {
    let packet = provision_role_root(ActorRole::Taker, role_root, lez_account, None, None)?;
    write_role_file(role_root, XMR_TAKER_PACKET_FILE, &packet, "Taker packet")?;
    xmr_taker_contribution(role_root)
}

/// Rereads the Taker's contribution from its role root, so a resent request
/// is byte-identical.
///
/// # Errors
///
/// Fails when the role root is not a Taker's or its packet is not the one its
/// manifest binds.
pub fn xmr_taker_contribution(role_root: &Path) -> Result<XmrTakerContribution> {
    let taker_packet = read_role_file(
        role_root,
        XMR_TAKER_PACKET_FILE,
        ROLE_PACKET_MAX_BYTES,
        "Taker packet",
    )?;
    let root = open_private_directory(role_root, "private role root")?;
    let manifest = read_private_manifest_at(&root)?;
    ensure!(
        manifest.role() == ActorRole::Taker
            && manifest.public_packet_sha256() == <[u8; 32]>::from(Sha256::digest(&taker_packet)),
        "the role root is not this Taker packet's"
    );
    Ok(XmrTakerContribution {
        taker_packet,
        taker_private_view_key: read_secret_hex_at(&root, VIEW_KEY_FILE, "Monero view key")?,
    })
}

/// The Maker's half of a reservation: checks the Taker's packet and view key,
/// makes its own material around the Taker's view key and its agreement key,
/// plans and signs Stage A at the LEZ anchor `now`, and writes its material,
/// both packets and the unsigned Stage A into a new private `role_root`.
///
/// # Errors
///
/// Fails for a Taker packet that is not a Taker's, aliases the Maker or does
/// not match the view key, a plan that is not these terms, or a Stage A the
/// SDK refuses. Nothing is written on failure.
#[allow(clippy::too_many_arguments)]
pub fn reserve_xmr_maker_role(
    config: &XmrRoleConfigV1,
    planner: &dyn XmrStageAPlanner,
    terms: &XmrReservationTerms,
    lez: &XmrLezRole,
    agreement_key: &Zeroizing<[u8; 32]>,
    taker: &XmrTakerContribution,
    monero: XmrMoneroChainView,
    maker_owner_nonce: u128,
    now: u64,
    role_root: &Path,
) -> Result<XmrMakerReservation> {
    let destination = SecureDestination::new(role_root, "private role root")?;
    destination.ensure_absent("private role root")?;
    let taker_packet = ValidatedRolePacket::from_bytes(&taker.taker_packet)?;
    ensure!(
        taker_packet.role() == ActorRole::Taker,
        "the Taker's packet is not a Taker's"
    );
    let view_key = MoneroPrivateViewKey::from_monero_little_endian(*taker.taker_private_view_key)
        .context("the Taker's view key is invalid")?;
    ensure!(
        view_key.public_key() == taker_packet.public_view_key(),
        "the Taker's view key is not its packet's"
    );
    ensure!(
        taker_packet.identity().lez_owner_account() != lez.account(),
        "the Taker's LEZ account is the Maker's"
    );
    let agreement = crate::GeneratedSecpKey::from_secret(
        Zeroizing::new(**agreement_key),
        "Maker agreement key",
    )?;
    let material =
        generate_role_material(ActorRole::Maker, lez.account(), view_key, Some(agreement))?;
    let maker_packet = ValidatedRolePacket::from_bytes(&material.packet_bytes)?;
    let packets = StageRolePackets::validated(maker_packet, taker_packet, None)?;
    let input = plan_input(
        terms,
        lez,
        &material.packet_bytes,
        &taker.taker_packet,
        monero.genesis,
        config.windows(now)?,
        maker_owner_nonce,
    );
    let wire = planner.plan(&input)?;
    let validated = validate_plan(&wire, &input, &packets)?;
    let maker_stage_a_signature = sign_commitment(agreement_key, validated.commitment())?;
    let maker_packet = material.packet_bytes.clone();
    material.publish(&destination)?;
    write_role_file(
        role_root,
        XMR_MAKER_PACKET_FILE,
        &maker_packet,
        "Maker packet",
    )?;
    write_role_file(
        role_root,
        XMR_TAKER_PACKET_FILE,
        &taker.taker_packet,
        "Taker packet",
    )?;
    write_role_file(
        role_root,
        XMR_STAGE_A_UNSIGNED_FILE,
        &wire,
        "unsigned Stage A",
    )?;
    Ok(XmrMakerReservation {
        maker_packet,
        unsigned_stage_a_wire: wire,
        maker_stage_a_signature,
        lez_anchor_unix_seconds: now,
        monero_genesis: monero.genesis,
        monero_restore_height: monero.tip_height,
        maker_owner_nonce,
    })
}

/// The Taker's half: bounds the Maker's anchors by its own clock and monerod,
/// replans Stage A from the offer terms, its own facts and the Maker's few,
/// requires byte equality with the Maker's plan and a valid Maker signature,
/// and countersigns. On success writes the Maker packet and both Stage-A
/// wires into its `role_root` (made by [`begin_xmr_taker_role`]) and returns
/// the countersigned Stage A.
///
/// # Errors
///
/// Fails for any field of the answer that is not exactly what the Taker
/// composes, anchors outside its view, a Maker that is not the offer's, or an
/// invalid Maker signature. Nothing is written on failure.
#[allow(clippy::too_many_arguments)]
pub fn check_xmr_taker_role(
    config: &XmrRoleConfigV1,
    planner: &dyn XmrStageAPlanner,
    terms: &XmrReservationTerms,
    lez: &XmrLezRole,
    role_root: &Path,
    maker: &XmrMakerReservation,
    check: &XmrTakerCheck,
) -> Result<Vec<u8>> {
    let contribution = xmr_taker_contribution(role_root)?;
    let taker_packet = ValidatedRolePacket::from_bytes(&contribution.taker_packet)?;
    ensure!(
        taker_packet.identity().lez_owner_account() == lez.account(),
        "the role root is not this Taker's LEZ account"
    );
    let maker_packet = ValidatedRolePacket::from_bytes(&maker.maker_packet)?;
    ensure!(
        maker_packet.role() == ActorRole::Maker
            && maker_packet.identity().agreement_public_key() == check.maker_identity,
        "the Maker's packet is not the offer's Maker"
    );
    ensure!(
        maker.lez_anchor_unix_seconds.abs_diff(check.now) <= MAX_ANCHOR_CLOCK_SKEW_SECONDS,
        "the Maker's LEZ anchor time is too far from this Taker's clock"
    );
    ensure!(
        maker.monero_genesis == check.monero.genesis,
        "the Maker's monerod is another chain"
    );
    ensure!(
        maker.monero_restore_height
            <= check
                .monero
                .tip_height
                .saturating_add(MAX_RESTORE_HEIGHT_SKEW_BLOCKS),
        "the Maker's Monero restore height is above this Taker's view of the chain"
    );
    let packets = StageRolePackets::validated(maker_packet, taker_packet, None)?;
    let input = plan_input(
        terms,
        lez,
        &maker.maker_packet,
        &contribution.taker_packet,
        check.monero.genesis,
        config.windows(maker.lez_anchor_unix_seconds)?,
        maker.maker_owner_nonce,
    );
    let expected = planner.plan(&input)?;
    ensure!(
        expected == maker.unsigned_stage_a_wire,
        "the Maker's Stage A is not the agreement this Taker composes from the offer and its facts"
    );
    let validated = validate_plan(&expected, &input, &packets)?;
    let root = open_private_directory(role_root, "private role root")?;
    let agreement_key = read_secret_hex_at(&root, crate::AGREEMENT_KEY_FILE, "agreement key")?;
    let taker_signature = sign_commitment(&agreement_key, validated.commitment())?;
    let agreement = validated
        .attach_signatures(maker.maker_stage_a_signature, taker_signature)
        .context("the Maker's Stage-A signature is invalid")?;
    let wire = agreement
        .encode_wire()
        .context("encode the signed Stage A")?;
    write_role_file(
        role_root,
        XMR_MAKER_PACKET_FILE,
        &maker.maker_packet,
        "Maker packet",
    )?;
    write_role_file(
        role_root,
        XMR_STAGE_A_UNSIGNED_FILE,
        &maker.unsigned_stage_a_wire,
        "unsigned Stage A",
    )?;
    write_role_file(role_root, XMR_STAGE_A_FILE, &wire, "signed Stage A")?;
    Ok(wire)
}

/// The Maker's acceptance of the countersigned Stage A (R2): it must be the
/// unsigned Stage A this role reserved, signed by both roles. Writes it into
/// `role_root`.
///
/// # Errors
///
/// Fails for an invalid wire or one over another body.
pub fn accept_xmr_maker_stage_a(role_root: &Path, agreement_wire: &[u8]) -> Result<XmrAgreementV1> {
    let unsigned = read_role_file(
        role_root,
        XMR_STAGE_A_UNSIGNED_FILE,
        MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES as u64,
        "unsigned Stage A",
    )?;
    let reserved = ValidatedXmrAgreementBodyV1::from_unsigned_wire(&unsigned)
        .context("the reserved Stage A is invalid")?;
    let agreement =
        XmrAgreementV1::from_wire(agreement_wire).context("the signed Stage A is invalid")?;
    ensure!(
        agreement.agreement_commitment() == reserved.commitment(),
        "the signed Stage A is not the one this Maker reserved"
    );
    write_role_file(
        role_root,
        XMR_STAGE_A_FILE,
        agreement_wire,
        "signed Stage A",
    )?;
    Ok(agreement)
}

fn plan_input(
    terms: &XmrReservationTerms,
    lez: &XmrLezRole,
    maker_packet: &[u8],
    taker_packet: &[u8],
    monero_genesis: [u8; 32],
    windows: XmrWindowsV1,
    maker_owner_nonce: u128,
) -> XmrStageAPlanInputV1 {
    XmrStageAPlanInputV1 {
        schema_version: STAGE_A_PLAN_SCHEMA_VERSION,
        maker_packet: hex::encode(maker_packet),
        taker_packet: hex::encode(taker_packet),
        swap_id: Hex32::from_bytes(terms.swap_id),
        monero_amount_piconero: terms.amount_piconero,
        lez_amount: terms.lez_amount,
        monero_genesis_hash: Hex32::from_bytes(monero_genesis),
        lez_channel_id: lez.runtime.channel_id,
        lez_genesis_hash: lez.runtime.genesis_block_hash,
        maker_xmr_funding_cutoff_ms: windows.maker_xmr_funding_cutoff_ms(),
        refund_at_ms: windows.refund_at_ms(),
        punish_at_ms: windows.punish_at_ms(),
        maker_owner_nonce,
    }
}

/// The planner's wire must be valid Stage A over exactly these packets and
/// terms (a planner fault must not reach a signature).
fn validate_plan(
    wire: &[u8],
    input: &XmrStageAPlanInputV1,
    packets: &StageRolePackets,
) -> Result<ValidatedXmrAgreementBodyV1> {
    let validated = ValidatedXmrAgreementBodyV1::from_unsigned_wire(wire)
        .context("the planned Stage A is invalid")?;
    validate_stage_a_packets(validated.body(), packets)?;
    ensure!(
        body_has_terms(validated.body(), input, packets),
        "the planned Stage A is not this reservation's terms"
    );
    Ok(validated)
}

fn body_has_terms(
    body: &XmrAgreementBodyV1,
    input: &XmrStageAPlanInputV1,
    packets: &StageRolePackets,
) -> bool {
    let maker = packets.maker.identity().lez_owner_account();
    let taker = packets.taker.identity().lez_owner_account();
    body.swap_id() == *input.swap_id.as_bytes()
        && body.monero().amount_piconero() == input.monero_amount_piconero
        && body.monero().genesis_hash() == *input.monero_genesis_hash.as_bytes()
        && body.lez().amount() == input.lez_amount
        && body.lez().channel_id() == *input.lez_channel_id.as_bytes()
        && body.lez().genesis_hash() == *input.lez_genesis_hash.as_bytes()
        && body.lez().depositor_account() == taker
        && body.lez().claimant_account() == maker
        && body
            .participants()
            .for_role(XmrRoleV1::Maker)
            .lez_owner_account()
            == maker
        && body.windows() == input.windows()
}

/// BIP340 over a Stage-A or Stage-B commitment, deterministic (no auxiliary
/// randomness) so a replayed round yields the same bytes.
pub(crate) fn sign_commitment(key: &Zeroizing<[u8; 32]>, commitment: [u8; 32]) -> Result<[u8; 64]> {
    let secp = Secp256k1::new();
    let mut keypair =
        Keypair::from_seckey_slice(&secp, key.as_ref()).context("agreement key is invalid")?;
    let signature = secp
        .sign_schnorr_no_aux_rand(&Message::from_digest(commitment), &keypair)
        .serialize();
    keypair.non_secure_erase();
    Ok(signature)
}

pub(crate) fn read_role_file(
    role_root: &Path,
    name: &str,
    max_bytes: u64,
    label: &'static str,
) -> Result<Vec<u8>> {
    let root = open_private_directory(role_root, "private role root")?;
    let file = openat(
        &root,
        name,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| anyhow!("{label} is unavailable"))?;
    read_bounded_file(file, max_bytes, FilePolicy::Private, label)
}

/// Writes `bytes` once into the role root; a replay with the same bytes is a
/// no-op and different bytes are refused.
pub(crate) fn write_role_file(
    role_root: &Path,
    name: &str,
    bytes: &[u8],
    label: &'static str,
) -> Result<()> {
    let root = open_private_directory(role_root, "private role root")?;
    let max = MAX_XMR_AGREEMENT_WIRE_BYTES.max(MAX_XMR_ACTIVATION_WIRE_BYTES) as u64;
    match openat(
        &root,
        name,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Err(Errno::NOENT) => write_new_at(&root, name, bytes, label),
        Ok(file) => {
            let existing = read_bounded_file(File::from(file), max, FilePolicy::Private, label)?;
            ensure!(existing == bytes, "{label} differs from the one on disk");
            Ok(())
        }
        Err(_) => Err(anyhow!("{label} is unsafe")),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    use lez_bridge_protocol::{Hex32, Participant as BridgeParticipant, RuntimeCompatibility};
    use lez_xmr_swap_sdk::{
        MoneroAddressNetworkV1, MoneroSharedAddressV1, XmrLezTermsV1, XmrMessagesV1,
        XmrMoneroTermsV1, XmrParticipantsV1, XmrSwapDirectionV1,
    };
    use secp256k1::{PublicKey, SecretKey};
    use tempfile::TempDir;

    use super::*;

    pub(crate) const MAKER_ACCOUNT: u8 = 0x11;
    pub(crate) const TAKER_ACCOUNT: u8 = 0x22;
    pub(crate) const NOW: u64 = 1_900_000_000;
    pub(crate) const MAKER_SECRET: [u8; 32] = [7; 32];
    const GENESIS: [u8; 32] = [31; 32];
    const TIP: u64 = 150;

    /// Composes a valid Stage A from the input as the real planner does, with
    /// message hashes derived from the whole input in place of the LEZ ones,
    /// so any differing fact yields a different body.
    pub(crate) struct FixturePlanner;

    impl XmrStageAPlanner for FixturePlanner {
        fn plan(&self, input: &XmrStageAPlanInputV1) -> Result<Vec<u8>> {
            let maker = ValidatedRolePacket::from_bytes(&hex::decode(&input.maker_packet)?)?;
            let taker = ValidatedRolePacket::from_bytes(&hex::decode(&input.taker_packet)?)?;
            let participants =
                XmrParticipantsV1::new(maker.identity().clone(), taker.identity().clone());
            let claim_key = participants.claim_aggregate_x_only_key()?;
            let refund_key = participants.refund_aggregate_x_only_key()?;
            let shared = MoneroSharedAddressV1::derive_from_public_view_key(
                MoneroAddressNetworkV1::Regtest,
                maker.proof(),
                taker.proof(),
                maker.public_view_key(),
            )?;
            let seed = serde_json::to_vec(input)?;
            let message = |purpose: &[u8]| -> [u8; 32] {
                Sha256::new()
                    .chain_update(&seed)
                    .chain_update(purpose)
                    .finalize()
                    .into()
            };
            let profile = XmrNamedProfileV1::AcceleratedRegtest;
            let body = XmrAgreementBodyV1::new(
                XmrSwapDirectionV1::TakerSellsLez,
                profile,
                *input.swap_id.as_bytes(),
                participants,
                XmrMoneroTermsV1::new(
                    MoneroAddressNetworkV1::Regtest,
                    *input.monero_genesis_hash.as_bytes(),
                    input.monero_amount_piconero,
                    profile.required_monero_confirmations(),
                    maker.proof().to_wire_bytes()?,
                    taker.proof().to_wire_bytes()?,
                    shared.public_view_key(),
                    shared.public_spend_key(),
                    shared.address_string(),
                ),
                XmrLezTermsV1::new(
                    *input.lez_channel_id.as_bytes(),
                    *input.lez_genesis_hash.as_bytes(),
                    [42; 8],
                    [43; 8],
                    profile.required_lez_finality_units(),
                    [44; 32],
                    [45; 32],
                    taker.identity().lez_owner_account(),
                    maker.identity().lez_owner_account(),
                    claim_key,
                    XmrLezTermsV1::authority_account_for_key(claim_key),
                    refund_key,
                    XmrLezTermsV1::authority_account_for_key(refund_key),
                    maker.proof().transcript_commitment(),
                    taker.proof().transcript_commitment(),
                    input.lez_amount,
                ),
                XmrMessagesV1::new(message(b"claim"), message(b"refund"), message(b"punish")),
                input.windows(),
            );
            Ok(ValidatedXmrAgreementBodyV1::validate(body)?.encode_unsigned_wire()?)
        }
    }

    fn private_dir(path: &Path) -> PathBuf {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::canonicalize(path).unwrap()
    }

    fn private_file(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn write_config(root: &Path, edit: impl FnOnce(&mut serde_json::Value)) -> PathBuf {
        private_file(&root.join("daemon.login"), b"daemon:0123abcd\n");
        private_file(&root.join("wallet.login"), b"maker:4567cdef\n");
        let planner = root.join("planner");
        private_file(&planner, b"#!/bin/sh\nexit 1\n");
        let mut value = serde_json::json!({
            "schema_version": 1,
            "profile": "accelerated-regtest",
            "monerod_endpoint": "http://127.0.0.1:18081/",
            "monerod_login_file": root.join("daemon.login"),
            "wallet_rpc_endpoint": "http://127.0.0.1:18083/",
            "wallet_rpc_login_file": root.join("wallet.login"),
            "wallet_name": "maker",
            "funding_cutoff_seconds": 260,
            "refund_seconds": 360,
            "punish_seconds": 480,
            "stage_a_planner": planner,
        });
        edit(&mut value);
        let path = root.join("xmr-role.json");
        private_file(&path, &serde_json::to_vec(&value).unwrap());
        path
    }

    pub(crate) fn config(root: &Path) -> XmrRoleConfigV1 {
        XmrRoleConfigV1::load(&write_config(root, |_| {})).unwrap()
    }

    pub(crate) fn lez(role: BridgeParticipant, account: u8) -> XmrLezRole {
        XmrLezRole {
            runtime: RuntimeDescriptor::new(
                role,
                RuntimeCompatibility::LeeV0_2_0,
                Hex32::from_bytes([1; 32]),
                Hex32::from_bytes([1; 32]),
                Hex32::from_bytes([2; 32]),
                Hex32::from_bytes([3; 32]),
                Hex32::from_bytes([account; 32]),
            ),
        }
    }

    pub(crate) fn terms() -> XmrReservationTerms {
        XmrReservationTerms {
            swap_id: [19; 32],
            amount_piconero: 1_000_000_000_000,
            lez_amount: 500,
        }
    }

    pub(crate) const fn monero() -> XmrMoneroChainView {
        XmrMoneroChainView {
            genesis: GENESIS,
            tip_height: TIP,
        }
    }

    pub(crate) fn maker_identity() -> [u8; 33] {
        PublicKey::from_secret_key(
            &Secp256k1::new(),
            &SecretKey::from_slice(&MAKER_SECRET).unwrap(),
        )
        .serialize()
    }

    pub(crate) fn taker_check() -> XmrTakerCheck {
        XmrTakerCheck {
            now: NOW + 5,
            monero: monero(),
            maker_identity: maker_identity(),
        }
    }

    pub(crate) struct Round {
        pub(crate) directory: TempDir,
        pub(crate) maker: XmrMakerReservation,
        pub(crate) maker_root: PathBuf,
        pub(crate) taker_root: PathBuf,
    }

    impl Round {
        pub(crate) fn config(&self) -> XmrRoleConfigV1 {
            config(self.directory.path())
        }

        pub(crate) fn check(
            &self,
            maker: &XmrMakerReservation,
            check: &XmrTakerCheck,
        ) -> Result<Vec<u8>> {
            check_xmr_taker_role(
                &self.config(),
                &FixturePlanner,
                &terms(),
                &lez(BridgeParticipant::Taker, TAKER_ACCOUNT),
                &self.taker_root,
                maker,
                check,
            )
        }
    }

    /// One reserve round as the Nodes run it: the Taker's material first,
    /// then the Maker's half.
    pub(crate) fn reserve() -> Round {
        let directory = TempDir::new().unwrap();
        let root = fs::canonicalize(directory.path()).unwrap();
        let maker_root = private_dir(&root.join("maker")).join("xmr");
        let taker_root = private_dir(&root.join("taker")).join("xmr");
        let taker = begin_xmr_taker_role(&taker_root, [TAKER_ACCOUNT; 32]).unwrap();
        let maker = reserve_xmr_maker_role(
            &config(&root),
            &FixturePlanner,
            &terms(),
            &lez(BridgeParticipant::Maker, MAKER_ACCOUNT),
            &Zeroizing::new(MAKER_SECRET),
            &taker,
            monero(),
            4,
            NOW,
            &maker_root,
        )
        .unwrap();
        Round {
            directory,
            maker,
            maker_root,
            taker_root,
        }
    }

    #[test]
    fn both_halves_compose_the_same_stage_a() {
        let round = reserve();
        let wire = round.check(&round.maker, &taker_check()).unwrap();
        // Replaying the check yields the same countersigned bytes.
        assert_eq!(round.check(&round.maker, &taker_check()).unwrap(), wire);
        let agreement = accept_xmr_maker_stage_a(&round.maker_root, &wire).unwrap();
        assert_eq!(agreement.encode_wire().unwrap(), wire);
        for name in [
            XMR_MAKER_PACKET_FILE,
            XMR_TAKER_PACKET_FILE,
            XMR_STAGE_A_UNSIGNED_FILE,
            XMR_STAGE_A_FILE,
            VIEW_KEY_FILE,
        ] {
            assert_eq!(
                fs::read(round.maker_root.join(name)).unwrap(),
                fs::read(round.taker_root.join(name)).unwrap(),
                "{name}"
            );
        }
        // Each role holds its own spend share and session keys.
        for name in [
            crate::XMR_SHARE_FILE,
            crate::CLAIM_KEY_FILE,
            crate::REFUND_KEY_FILE,
        ] {
            assert_ne!(
                fs::read(round.maker_root.join(name)).unwrap(),
                fs::read(round.taker_root.join(name)).unwrap(),
                "{name}"
            );
        }
        let body = agreement.body();
        assert_eq!(body.swap_id(), terms().swap_id);
        assert_eq!(
            body.participants()
                .for_role(XmrRoleV1::Maker)
                .agreement_public_key(),
            maker_identity()
        );
        assert_eq!(
            body.windows(),
            round.config().windows(NOW).unwrap(),
            "C, R and P count from the Maker's anchor"
        );
        assert_eq!(body.windows().refund_at_ms(), (NOW + 360) * 1_000);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn the_taker_refuses_each_tampered_field() {
        let round = reserve();
        let honest = &round.maker;
        let other = reserve();
        let refused = |maker: XmrMakerReservation, check: XmrTakerCheck, field: &str| {
            assert!(round.check(&maker, &check).is_err(), "{field}");
        };
        let check = taker_check();
        // Each field of the Maker's answer.
        let mut packet = honest.clone();
        packet.maker_packet.clone_from(&other.maker.maker_packet);
        refused(packet, check, "maker_packet");
        let mut wire = honest.clone();
        let last = wire.unsigned_stage_a_wire.len() - 1;
        wire.unsigned_stage_a_wire[last] ^= 1;
        refused(wire, check, "unsigned_stage_a_wire");
        let mut signature = honest.clone();
        signature.maker_stage_a_signature[0] ^= 1;
        refused(signature, check, "maker_stage_a_signature");
        let mut anchor = honest.clone();
        anchor.lez_anchor_unix_seconds += MAX_ANCHOR_CLOCK_SKEW_SECONDS + 10;
        refused(anchor, check, "lez_anchor_unix_seconds (skew)");
        let mut shifted = honest.clone();
        shifted.lez_anchor_unix_seconds += 1;
        refused(shifted, check, "lez_anchor_unix_seconds (windows)");
        let mut genesis = honest.clone();
        genesis.monero_genesis = [32; 32];
        refused(genesis, check, "monero_genesis");
        let mut height = honest.clone();
        height.monero_restore_height = TIP + MAX_RESTORE_HEIGHT_SKEW_BLOCKS + 1;
        refused(height, check, "monero_restore_height");
        let mut nonce = honest.clone();
        nonce.maker_owner_nonce += 1;
        refused(nonce, check, "maker_owner_nonce");
        // The Maker is not the offer's, or the Taker's own view differs.
        let mut identity = check;
        identity.maker_identity = PublicKey::from_secret_key(
            &Secp256k1::new(),
            &SecretKey::from_slice(&[8; 32]).unwrap(),
        )
        .serialize();
        refused(honest.clone(), identity, "maker_identity");
        let mut own_genesis = check;
        own_genesis.monero.genesis = [33; 32];
        refused(honest.clone(), own_genesis, "Taker monerod genesis");
        // A Stage A signed over other terms (another swap id).
        let other_terms = XmrReservationTerms {
            swap_id: [20; 32],
            ..terms()
        };
        assert!(
            check_xmr_taker_role(
                &round.config(),
                &FixturePlanner,
                &other_terms,
                &lez(BridgeParticipant::Taker, TAKER_ACCOUNT),
                &round.taker_root,
                honest,
                &check,
            )
            .is_err()
        );
        // A different Taker LEZ account than its packet's.
        assert!(
            check_xmr_taker_role(
                &round.config(),
                &FixturePlanner,
                &terms(),
                &lez(BridgeParticipant::Taker, 0x33),
                &round.taker_root,
                honest,
                &check,
            )
            .is_err()
        );
        // Nothing was written by the refusals; the honest answer still passes.
        assert!(!round.taker_root.join(XMR_STAGE_A_FILE).exists());
        assert!(!round.taker_root.join(XMR_MAKER_PACKET_FILE).exists());
        round.check(honest, &check).unwrap();
        // Restore heights at or below the tip skew are fine.
        let mut low = honest.clone();
        low.monero_restore_height = 0;
        assert!(round.check(&low, &check).is_ok());
    }

    #[test]
    fn the_maker_refuses_what_it_cannot_commit() {
        let round = reserve();
        let root = fs::canonicalize(round.directory.path()).unwrap();
        let taker = xmr_taker_contribution(&round.taker_root).unwrap();
        let attempt = |taker: &XmrTakerContribution, name: &str| {
            reserve_xmr_maker_role(
                &round.config(),
                &FixturePlanner,
                &terms(),
                &lez(BridgeParticipant::Maker, MAKER_ACCOUNT),
                &Zeroizing::new(MAKER_SECRET),
                taker,
                monero(),
                4,
                NOW,
                &root.join("maker").join(name),
            )
        };
        // A Maker packet in the Taker's place.
        let mut maker_packet = taker.clone();
        maker_packet
            .taker_packet
            .clone_from(&round.maker.maker_packet);
        assert!(attempt(&maker_packet, "a").is_err());
        // A view key that is not the Taker packet's.
        let mut view = taker.clone();
        view.taker_private_view_key = Zeroizing::new([1; 32]);
        assert!(attempt(&view, "b").is_err());
        // A Taker on the Maker's LEZ account.
        let directory = private_dir(&root.join("aliased"));
        let aliased = begin_xmr_taker_role(&directory.join("xmr"), [MAKER_ACCOUNT; 32]).unwrap();
        assert!(attempt(&aliased, "c").is_err());
        // An existing role root.
        assert!(attempt(&taker, "xmr").is_err());
        for name in ["a", "b", "c"] {
            assert!(
                !root.join("maker").join(name).exists(),
                "a refusal wrote {name}"
            );
        }
        attempt(&taker, "d").unwrap();
    }

    /// Q8 of the M4 design: a Taker packet whose DLEQ proof carries one extra
    /// proof byte (resealed so its transcript commitment matches) is refused.
    #[test]
    fn the_maker_refuses_a_padded_taker_dleq_proof() {
        let round = reserve();
        let root = fs::canonicalize(round.directory.path()).unwrap();
        let mut taker = xmr_taker_contribution(&round.taker_root).unwrap();
        let packet: serde_json::Value = serde_json::from_slice(&taker.taker_packet).unwrap();
        let original = packet["dleq_proof_wire"].as_str().unwrap();
        let padded = hex::encode(pad_dleq_proof(&hex::decode(original).unwrap()));
        // Key order is canonical as written, so replace the proof in place.
        let text = String::from_utf8(taker.taker_packet.clone()).unwrap();
        assert_eq!(text.matches(original).count(), 1);
        taker.taker_packet = text.replacen(original, &padded, 1).into_bytes();
        let result = reserve_xmr_maker_role(
            &round.config(),
            &FixturePlanner,
            &terms(),
            &lez(BridgeParticipant::Maker, MAKER_ACCOUNT),
            &Zeroizing::new(MAKER_SECRET),
            &taker,
            monero(),
            4,
            NOW,
            &root.join("maker").join("padded"),
        );
        assert!(result.is_err());
    }

    /// Appends one byte to the postcard proof inside a DLEQ wire
    /// (`u8 schema, [33], [32], proof, [u8; 32] commitment`) and recomputes
    /// the transcript commitment over it.
    fn pad_dleq_proof(wire: &[u8]) -> Vec<u8> {
        let (secp, ed) = (&wire[2..35], &wire[36..68]);
        let (mut length, mut shift, mut offset) = (0_usize, 0, 68);
        loop {
            let byte = wire[offset];
            length |= usize::from(byte & 0x7f) << shift;
            offset += 1;
            shift += 7;
            if byte & 0x80 == 0 {
                break;
            }
        }
        let mut proof = wire[offset..offset + length].to_vec();
        proof.push(0);
        let commitment: [u8; 32] = Sha256::new()
            .chain_update(b"lez-atomic-swaps/xmr/cross-curve-dleq-transcript/v1")
            .chain_update([1])
            .chain_update(secp)
            .chain_update(ed)
            .chain_update(u32::try_from(proof.len()).unwrap().to_le_bytes())
            .chain_update(&proof)
            .finalize()
            .into();
        let mut padded = wire[..68].to_vec();
        let mut remaining = proof.len();
        loop {
            let byte = u8::try_from(remaining & 0x7f).unwrap();
            remaining >>= 7;
            if remaining == 0 {
                padded.push(byte);
                break;
            }
            padded.push(byte | 0x80);
        }
        padded.extend_from_slice(&proof);
        padded.extend_from_slice(&commitment);
        assert_eq!(wire.len() + 1, padded.len(), "one proof byte added");
        padded
    }

    #[test]
    fn the_role_config_is_checked_and_its_planner_pinned() {
        let directory = TempDir::new().unwrap();
        let root = fs::canonicalize(directory.path()).unwrap();
        let loaded = config(&root);
        assert_eq!(loaded.profile(), XmrNamedProfileV1::AcceleratedRegtest);
        assert_eq!(loaded.wallet_name(), "maker");
        assert!(loaded.wallet_password().unwrap().is_empty());
        for (field, value) in [
            ("profile", serde_json::json!("public-stagenet")),
            (
                "monerod_endpoint",
                serde_json::json!("http://localhost:18081/"),
            ),
            (
                "wallet_rpc_endpoint",
                serde_json::json!("http://10.0.0.1:18083/"),
            ),
            ("refund_seconds", serde_json::json!(200)),
            ("wallet_name", serde_json::json!("../maker")),
            ("stage_a_planner", serde_json::json!("planner")),
            ("unknown", serde_json::json!(1)),
        ] {
            let path = write_config(&root, |json| {
                json[field] = value.clone();
            });
            assert!(XmrRoleConfigV1::load(&path).is_err(), "{field}");
        }
        // A login file others can read is refused.
        let path = write_config(&root, |_| {});
        fs::set_permissions(root.join("daemon.login"), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(XmrRoleConfigV1::load(&path).is_err());
        fs::set_permissions(root.join("daemon.login"), fs::Permissions::from_mode(0o600)).unwrap();

        // The process planner runs the pinned program and refuses a changed one.
        let fixture = root.join("stage-a.fixture");
        fs::write(&fixture, b"unsigned-stage-a").unwrap();
        let planner = root.join("planner");
        let script = format!("#!/bin/sh\ncat >/dev/null\ncat '{}'\n", fixture.display());
        fs::write(&planner, script).unwrap();
        fs::set_permissions(&planner, fs::Permissions::from_mode(0o700)).unwrap();
        let loaded = XmrRoleConfigV1::load(&root.join("xmr-role.json")).unwrap();
        let input = plan_input(
            &terms(),
            &lez(BridgeParticipant::Maker, MAKER_ACCOUNT),
            b"{}\n",
            b"{}\n",
            GENESIS,
            loaded.windows(NOW).unwrap(),
            0,
        );
        assert_eq!(
            loaded.stage_a_planner().plan(&input).unwrap(),
            b"unsigned-stage-a"
        );
        fs::write(&planner, b"#!/bin/sh\ncat >/dev/null\necho changed\n").unwrap();
        assert!(loaded.stage_a_planner().plan(&input).is_err());
    }
}
