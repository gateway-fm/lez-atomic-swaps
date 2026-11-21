//! Taker side of the take-time Monero agreement (`xmr_reserve_v1`): the Zcash
//! model (`zec_dynamic.rs`) with the adaptor ceremony Bitcoin runs over Chat.
//!
//! The take reserves with the Maker (R1), checks the Maker's Stage A against
//! the one it composes itself and countersigns it (R2), runs the claim and
//! refund ceremony (R3–R5) and activates Stage B, all through the
//! `xmr-reference-actor` library. It then persists the swap as a prepared
//! entry; the prepared take (`execute_prepared_xmr`: actor provisioning, R6,
//! the acceptance receipt) runs it from there. Every step is idempotent on the
//! swap directory, and each Maker answer is kept as it came, so a replayed
//! initiation resends the same requests (the Maker answers replays) and
//! resumes where it stopped.

use std::{
    fs,
    os::unix::fs::DirBuilderExt as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};
use lez_bridge_protocol::RequestId;
use lez_btc_role_lifecycle::{
    SwapLayout, SwapSidecar,
    layout::{read_private, replace_private, write_private_exact},
};
use lez_node_common::{
    XmrCeremonyNonceRequestV1, XmrCeremonyNonceResponseV1, XmrCeremonyPartialResponseV1,
    XmrCeremonyReserveRequestV1, XmrCeremonyReserveResponseV1, XmrChatStageARequestV1,
    XmrChatStageAResponseV1, XmrReserveResponseV1,
};
use lez_swap_core::{Pair, SwapDirection, SwapId};
use lez_swap_store::{MakerOfferId, maker_xmr_chat_swap_id};
use lez_xmr_monero_adapter::{MoneroChainIdentityAttestor, MoneroNetwork};
use lez_xmr_swap_sdk::{XmrAgreementV1, XmrWindowsV1};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};
use xmr_reference_actor::{
    XMR_STAGE_A_FILE, XMR_STAGE_B_FILE, XmrLezRole, XmrMakerReservation, XmrMoneroChainView,
    XmrReservationTerms, XmrRoleConfigV1, XmrStageAPlanner, XmrTakerCeremony, XmrTakerCheck,
    begin_xmr_taker_role, check_xmr_taker_role, xmr_ceremony_partial_answer,
    xmr_taker_contribution,
};
use zeroize::Zeroizing;

use super::btc_dynamic::{DynamicBtcRole, TakeRequest};
use crate::{
    acceptance_files::derived_request_id,
    taker_service_config::prepared::{
        ImmutablePrivateFileV1, PreparedConfigurationV1, SecretPrivateFileV1,
    },
};

/// The reserve record keeps the Maker's R1 answer (two DLEQ proofs, ~720 KiB
/// as JSON at worst) and the smaller ceremony answers.
const MAX_RECORD_BYTES: usize = 2 * 1024 * 1024;
const MAX_FACTS_BYTES: usize = 64 * 1024;
const PREPARED_FILE: &str = "xmr-taker-prepared.json";
const RESERVE_FILE: &str = "xmr-taker-reserve.json";
/// The swap's public facts the supervisor reads (the catalog's actor config).
pub(super) const SWAP_FACTS_FILE: &str = "xmr-swap.json";
/// The Taker's per-swap agreement key as raw bytes, the catalog's signing key.
const AGREEMENT_KEY_COPY: &str = "xmr-agreement.key";
/// The Taker's own material, packets, Stage A/B and adaptor journal.
pub(super) const ROLE_DIRECTORY: &str = "xmr";
const ROLE_AGREEMENT_KEY_FILE: &str = "agreement.key";

/// The Taker's take-time Monero authority, over the Node-owned lifecycle's
/// LEZ role (identity, signer, swaps root, sidecar recipe).
pub struct DynamicXmrRole {
    config: XmrRoleConfigV1,
    btc: std::sync::Arc<DynamicBtcRole>,
    /// The supervisor refunds a swap's LEZ (tag 16) by itself from `R`.
    automatic_refund: bool,
}

impl std::fmt::Debug for DynamicXmrRole {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DynamicXmrRole")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// What a swap's supervisor needs beyond its receipt: the Maker's anchors.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct XmrTakerSwapFactsV1 {
    pub(super) schema_version: u16,
    pub(super) swap_id: SwapId,
    pub(super) reservation_id: RequestId,
    /// The Maker's monerod height at R1; the funding is later.
    pub(super) monero_restore_height: u64,
    pub(super) lez_anchor_unix_seconds: u64,
    /// The agreement's windows `C`, `R` and `P`, Unix ms (Stage A's).
    pub(super) maker_xmr_funding_cutoff_ms: u64,
    pub(super) refund_at_ms: u64,
    pub(super) punish_at_ms: u64,
}

/// What the negotiation leaves: the Maker's R1 answer and the windows the
/// countersigned Stage A commits.
pub(super) struct XmrNegotiated {
    pub(super) reserve: XmrReserveResponseV1,
    pub(super) windows: XmrWindowsV1,
}

/// The Maker's answers as they came, so a retried take resends the same
/// requests and checks the same answers. The R1 request is rebuilt from the
/// role root (byte-identical), so the per-swap view key is not kept here.
#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TakerXmrReserveRecordV1 {
    schema_version: u16,
    reserve: Option<XmrReserveResponseV1>,
    stage_a: Option<XmrChatStageAResponseV1>,
    commitments: Option<XmrCeremonyReserveResponseV1>,
    nonces: Option<XmrCeremonyNonceResponseV1>,
    partial: Option<XmrCeremonyPartialResponseV1>,
}

impl DynamicXmrRole {
    /// Loads the Taker's Monero role configuration.
    ///
    /// # Errors
    ///
    /// Fails when the configuration is invalid.
    pub fn load(config_file: &Path, btc: std::sync::Arc<DynamicBtcRole>) -> Result<Self> {
        Ok(Self {
            config: XmrRoleConfigV1::load(config_file)?,
            btc,
            automatic_refund: true,
        })
    }

    /// Whether the supervisor refunds a swap's LEZ (tag 16) by itself from
    /// `R` while the Maker has not claimed (on unless configured off).
    #[must_use]
    pub const fn with_automatic_refund(mut self, automatic_refund: bool) -> Self {
        self.automatic_refund = automatic_refund;
        self
    }

    pub(super) const fn automatic_refund(&self) -> bool {
        self.automatic_refund
    }

    pub(super) const fn config(&self) -> &XmrRoleConfigV1 {
        &self.config
    }

    pub(super) fn btc(&self) -> &DynamicBtcRole {
        &self.btc
    }

    pub(super) fn layout(&self, reservation_id: &RequestId) -> SwapLayout {
        self.btc.layout(reservation_id)
    }

    /// The one lock every Monero wallet-rpc user of this role holds
    /// (`<node state>/monero-rpc/wallet.lock`, beside the swaps root).
    pub(super) fn wallet_lock_file(&self) -> Result<PathBuf> {
        let state = self
            .btc
            .runtime()
            .config()
            .swaps_root
            .parent()
            .context("the swaps root has no parent")?
            .join("monero-rpc");
        if !state.is_dir() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&state)?;
        }
        Ok(state.join("wallet.lock"))
    }

    fn swap_directories(&self) -> Vec<PathBuf> {
        fs::read_dir(&self.btc.runtime().config().swaps_root)
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default()
    }

    /// Every swap this Node prepared at take time, as catalog entries.
    pub(crate) fn persisted_entries(&self) -> Vec<PreparedConfigurationV1> {
        self.swap_directories()
            .into_iter()
            .filter_map(|root| read_private(&root.join(PREPARED_FILE), MAX_FACTS_BYTES).ok())
            .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
            .collect()
    }

    /// Turns a persisted entry into a catalog entry, re-authenticating the
    /// offer against its Delivery source like the static catalog does.
    pub(crate) fn load_entry(
        &self,
        configured: &PreparedConfigurationV1,
    ) -> Result<crate::PreparedTakerInitiationV1> {
        self.btc.load_entry_as(Pair::Monero, false, configured)
    }

    /// Keeps every prepared swap's sidecar running, also after a restart.
    pub(crate) fn spawn_sidecar_keepalive(self: &std::sync::Arc<Self>) {
        let role = std::sync::Arc::clone(self);
        std::thread::spawn(move || {
            loop {
                for configured in role.persisted_entries() {
                    let layout = role.layout(&configured.reservation_id);
                    if let Err(error) =
                        SwapSidecar::ensure(role.btc.runtime(), &layout, &configured.reservation_id)
                    {
                        eprintln!(
                            "taker XMR swap {}: sidecar unavailable: {error:#}",
                            configured.swap_id.as_str()
                        );
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(15));
            }
        });
    }

    /// This role's LEZ facts: the descriptor its sidecars run under.
    pub(super) fn lez_role(&self) -> XmrLezRole {
        XmrLezRole {
            runtime: self.btc.runtime().runtime_descriptor(),
        }
    }

    /// What this Taker's own monerod says: its genesis and its tip.
    pub(super) async fn chain_view(&self) -> Result<XmrMoneroChainView> {
        let attestor =
            MoneroChainIdentityAttestor::new(MoneroNetwork::Regtest, &self.config.monerod()?)
                .context("Monero daemon")?;
        let genesis = attestor
            .discover()
            .await
            .context("Monero genesis")?
            .genesis_hash();
        let tip_height = attestor.tip_height().await.context("Monero tip")?;
        Ok(XmrMoneroChainView {
            genesis,
            tip_height,
        })
    }
}

/// The Taker's Monero role wallet as `taker_wallet_balances_v1 {monero:true}`
/// reports it (`balances["monero"]`); `unavailable` when its wallet-rpc or
/// monerod does not answer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum XmrWalletBalanceV1 {
    Available {
        address: String,
        balance_piconero: u64,
        unlocked_piconero: u64,
        daemon_height: u64,
    },
    Unavailable,
}

/// Reads of the role wallet one balance request makes: one that fails is
/// read once more (the read changes nothing), as a first read right after a
/// restart under load has failed on the devnet and the next one succeeded.
const XMR_BALANCE_READ_ATTEMPTS: u32 = 2;

impl DynamicXmrRole {
    /// Reads the role wallet under the role's wallet-rpc lock, which blocks,
    /// so on a blocking thread.
    pub(crate) async fn wallet_balance(self: &std::sync::Arc<Self>) -> XmrWalletBalanceV1 {
        let role = std::sync::Arc::clone(self);
        let handle = tokio::runtime::Handle::current();
        let read = tokio::task::spawn_blocking(move || {
            let _lock =
                lez_xmr_monero_adapter::MoneroWalletRpcLock::acquire(&role.wallet_lock_file()?)?;
            handle.block_on(async {
                let mut attempt = 1;
                loop {
                    match role.read_wallet_balance().await {
                        Err(error) if attempt < XMR_BALANCE_READ_ATTEMPTS => {
                            eprintln!("taker XMR balance read failed, reading again: {error:#}");
                            attempt += 1;
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                        read => break read,
                    }
                }
            })
        })
        .await;
        match read {
            Ok(Ok(balance)) => balance,
            Ok(Err(error)) => {
                eprintln!("taker XMR balance unavailable: {error:#}");
                XmrWalletBalanceV1::Unavailable
            }
            Err(_) => XmrWalletBalanceV1::Unavailable,
        }
    }

    /// One read of the role wallet and the daemon's tip; the caller holds
    /// the wallet-rpc lock.
    async fn read_wallet_balance(&self) -> anyhow::Result<XmrWalletBalanceV1> {
        let effects = lez_xmr_monero_adapter::MoneroRegtestWalletEffects::new(
            &self.config.monerod()?,
            &self.config.wallet_rpc()?,
        )?
        .with_role_wallet(
            self.config.wallet_name().to_owned(),
            self.config.wallet_password()?.to_string(),
        )?;
        let (address, balance_piconero, unlocked_piconero) = effects.role_wallet_balance().await?;
        let daemon_height = self.chain_view().await?.tip_height;
        Ok(XmrWalletBalanceV1::Available {
            address: address.to_string(),
            balance_piconero,
            unlocked_piconero,
            daemon_height,
        })
    }
}

/// The Maker's Chat methods as the take calls them; the Node's own Chat
/// socket in production, an in-process Maker in the unit tests.
#[async_trait::async_trait]
pub(super) trait XmrMakerChat: Sync {
    async fn call(
        &self,
        method: &'static str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value>;
}

struct SocketChat<'a>(&'a Path);

#[async_trait::async_trait]
impl XmrMakerChat for SocketChat<'_> {
    async fn call(
        &self,
        method: &'static str,
        request: serde_json::Value,
    ) -> Result<serde_json::Value> {
        crate::xmr_taker_accept::call_maker_xmr_round(self.0, method, &request)
            .await
            .context(method)
    }
}

async fn round<Request: Serialize + Sync, Response: DeserializeOwned>(
    chat: &dyn XmrMakerChat,
    method: &'static str,
    request: &Request,
) -> Result<Response> {
    let answer = chat.call(method, serde_json::to_value(request)?).await?;
    serde_json::from_value(answer).with_context(|| format!("{method}: the Maker's answer"))
}

/// One take's negotiation inputs, everything the rounds compose from.
pub(super) struct XmrNegotiation<'a> {
    pub(super) config: &'a XmrRoleConfigV1,
    pub(super) planner: &'a (dyn XmrStageAPlanner + Sync),
    pub(super) lez: XmrLezRole,
    /// The swap directory (it exists); the role root is its `xmr/`.
    pub(super) swap_root: &'a Path,
    pub(super) reservation_id: RequestId,
    pub(super) offer_id: MakerOfferId,
    pub(super) signed_offer_envelope: &'a [u8],
    pub(super) terms: XmrReservationTerms,
    pub(super) maker_identity: [u8; 33],
    pub(super) monero: XmrMoneroChainView,
}

fn store_record(swap_root: &Path, record: &TakerXmrReserveRecordV1) -> Result<()> {
    replace_private(&swap_root.join(RESERVE_FILE), &serde_json::to_vec(record)?)
}

/// R1–R5 and the Stage-B activation in the swap's role root; returns the
/// Maker's R1 answer and the agreement's windows. Idempotent: rounds whose answer is kept are not sent
/// again, and the ceremony's journal replays its own packets.
#[allow(clippy::too_many_lines)]
pub(super) async fn negotiate(
    negotiation: &XmrNegotiation<'_>,
    chat: &dyn XmrMakerChat,
    now: u64,
) -> Result<XmrNegotiated> {
    let role_root = negotiation.swap_root.join(ROLE_DIRECTORY);
    let reservation_id = &negotiation.reservation_id;
    let mut record = match read_private(&negotiation.swap_root.join(RESERVE_FILE), MAX_RECORD_BYTES)
    {
        Ok(bytes) => serde_json::from_slice::<TakerXmrReserveRecordV1>(&bytes)?,
        Err(_) => TakerXmrReserveRecordV1 {
            schema_version: 1,
            ..TakerXmrReserveRecordV1::default()
        },
    };

    // R1: the Taker's material is made once. A crash between making the
    // role root and writing its packet leaves a root without one; before the
    // Maker answered, that root is made again.
    let contribution = match xmr_taker_contribution(&role_root) {
        Ok(contribution) => contribution,
        Err(_) if record.reserve.is_none() => {
            let _ = fs::remove_dir_all(&role_root);
            begin_xmr_taker_role(
                &role_root,
                *negotiation.lez.runtime.signer_account_id.as_bytes(),
            )?
        }
        Err(error) => return Err(error.context("the reserved Taker role root is unreadable")),
    };
    let reserve = if let Some(reserve) = record.reserve.clone() {
        reserve
    } else {
        let request = contribution.reserve_request(
            reservation_id.clone(),
            negotiation.offer_id.clone(),
            negotiation.signed_offer_envelope.to_vec(),
            negotiation.terms.amount_piconero,
        );
        drop(contribution);
        let answer: XmrReserveResponseV1 = round(chat, "xmr_reserve_v1", &request).await?;
        drop(request);
        record.reserve = Some(answer.clone());
        store_record(negotiation.swap_root, &record)?;
        answer
    };
    ensure!(
        reserve.schema_version == 1
            && reserve.swap_id.as_ref() == hex::encode(negotiation.terms.swap_id),
        "the Maker answered for another swap"
    );

    // R2: the Taker composes Stage A itself, requires the Maker's to be it,
    // and countersigns. Checked once; the clock bound is the take's.
    let stage_a_file = role_root.join(XMR_STAGE_A_FILE);
    let stage_a = if stage_a_file.is_file() {
        read_private(&stage_a_file, MAX_RECORD_BYTES)?
    } else {
        check_xmr_taker_role(
            negotiation.config,
            negotiation.planner,
            &negotiation.terms,
            &negotiation.lez,
            &role_root,
            &XmrMakerReservation::from(&reserve),
            &XmrTakerCheck {
                now,
                monero: negotiation.monero,
                maker_identity: negotiation.maker_identity,
            },
        )?
    };
    let stage_a_answer = if let Some(answer) = record.stage_a.clone() {
        answer
    } else {
        let answer: XmrChatStageAResponseV1 = round(
            chat,
            "xmr_chat_stage_a_v1",
            &XmrChatStageARequestV1 {
                schema_version: 1,
                request_id: derived_request_id(reservation_id, "xmr", b"stage-a")?,
                offer_id: negotiation.offer_id.clone(),
                expected_offer_revision: 1,
                reservation_id: reservation_id.clone(),
                foreign_units: negotiation.terms.amount_piconero,
                signed_offer_envelope: negotiation.signed_offer_envelope.to_vec(),
                stage_a_wire: stage_a.clone(),
            },
        )
        .await?;
        record.stage_a = Some(answer.clone());
        store_record(negotiation.swap_root, &record)?;
        answer
    };
    let agreement = XmrAgreementV1::from_wire(&stage_a).context("the countersigned Stage A")?;
    ensure!(
        stage_a_answer.schema_version == 1
            && stage_a_answer.offer_revision == 2
            && &stage_a_answer.reservation_id == reservation_id
            && stage_a_answer.lez_units == negotiation.terms.lez_amount
            && stage_a_answer.swap_id.as_ref() == hex::encode(negotiation.terms.swap_id)
            && stage_a_answer.agreement_commitment == agreement.agreement_commitment(),
        "the Maker's Stage-A answer changed the reservation, quote or agreement"
    );
    let negotiated = XmrNegotiated {
        reserve,
        windows: agreement.body().windows(),
    };
    if role_root.join(XMR_STAGE_B_FILE).is_file() {
        return Ok(negotiated);
    }

    // R3–R5: both sessions advance together in the role's adaptor journal.
    let mut ceremony = XmrTakerCeremony::open(&role_root)?;
    let commitments = ceremony.commitments()?;
    let maker_commitments = if let Some(answer) = record.commitments.clone() {
        answer
    } else {
        let answer: XmrCeremonyReserveResponseV1 = round(
            chat,
            "xmr_ceremony_reserve_v1",
            &XmrCeremonyReserveRequestV1 {
                schema_version: 1,
                request_id: derived_request_id(reservation_id, "xmr", b"ceremony-reserve")?,
                reservation_id: reservation_id.clone(),
                taker_commitments: commitments.into(),
            },
        )
        .await?;
        record.commitments = Some(answer.clone());
        store_record(negotiation.swap_root, &record)?;
        answer
    };
    ensure!(
        maker_commitments.schema_version == 1,
        "unsupported ceremony answer"
    );
    ceremony.accept_maker_commitments(&maker_commitments.maker_commitments.into())?;
    let nonces = ceremony.nonces()?;
    let maker_nonces = if let Some(answer) = record.nonces.clone() {
        answer
    } else {
        let answer: XmrCeremonyNonceResponseV1 = round(
            chat,
            "xmr_ceremony_nonce_v1",
            &XmrCeremonyNonceRequestV1 {
                schema_version: 1,
                request_id: derived_request_id(reservation_id, "xmr", b"ceremony-nonce")?,
                reservation_id: reservation_id.clone(),
                taker_nonces: nonces.into(),
            },
        )
        .await?;
        record.nonces = Some(answer.clone());
        store_record(negotiation.swap_root, &record)?;
        answer
    };
    ensure!(
        maker_nonces.schema_version == 1,
        "unsupported ceremony answer"
    );
    // The Taker's claim partial stays in its journal: only the refund
    // partial travels, and tag 14 publishes the claim partial on LEZ.
    let refund_partial = ceremony.sign(&maker_nonces.maker_nonces.into())?;
    let stage_b = ceremony.finish(&maker_nonces.maker_partials.into())?;
    let partial = if let Some(answer) = record.partial.clone() {
        answer
    } else {
        let request = stage_b.partial_request(
            derived_request_id(reservation_id, "xmr", b"ceremony-partial")?,
            reservation_id.clone(),
            refund_partial,
        );
        let answer: XmrCeremonyPartialResponseV1 =
            round(chat, "xmr_ceremony_partial_v1", &request).await?;
        record.partial = Some(answer.clone());
        store_record(negotiation.swap_root, &record)?;
        answer
    };
    ensure!(partial.schema_version == 1, "unsupported ceremony answer");
    let (presignature, maker_signature) = xmr_ceremony_partial_answer(&partial)?;
    ceremony.activate(presignature, maker_signature)?;
    Ok(negotiated)
}

/// The per-swap agreement key as the catalog binds a signing key: 32 raw
/// bytes (the role root keeps it as hex).
fn agreement_key_bytes(role_root: &Path) -> Result<Zeroizing<[u8; 32]>> {
    let text = Zeroizing::new(read_private(&role_root.join(ROLE_AGREEMENT_KEY_FILE), 128)?);
    let text = std::str::from_utf8(&text).context("the agreement key is not text")?;
    let mut key = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(text.trim(), &mut *key).context("the agreement key is not hex")?;
    Ok(key)
}

/// Reserves with the Maker, checks and countersigns its Stage A, runs the
/// ceremony and persists the swap as a prepared entry. Idempotent on the swap
/// directory.
#[allow(clippy::too_many_lines)]
pub(super) async fn prepare(
    dynamic: &DynamicXmrRole,
    request: &TakeRequest,
    now: u64,
) -> Result<PreparedConfigurationV1> {
    ensure!(
        request.route.pair() == Pair::Monero
            && request.route.direction() == SwapDirection::TakerSellsLez,
        "not a Monero TakerSellsLez route"
    );
    let reservation_id = derived_request_id(&request.request_id, "xmr", b"node-reservation")?;
    let layout = dynamic.layout(&reservation_id);
    let prepared_file = layout.root().join(PREPARED_FILE);
    if layout.exists()
        && let Ok(bytes) = read_private(&prepared_file, MAX_FACTS_BYTES)
    {
        return Ok(serde_json::from_slice(&bytes)?);
    }
    let offer = dynamic
        .btc
        .authenticate_offer(
            &request.offer_id,
            request.route,
            &request.maker_identity,
            now,
            request.announcement.clone(),
        )
        .await?;
    ensure!(
        offer.commitment() == request.signed_envelope_sha256
            && offer.offer().route() == request.route
            && offer.offer().quote_foreign_amount(request.foreign_units)?
                == request.expected_lez_units
            && offer.offer().created_at_unix_seconds() <= now
            && now < offer.offer().expires_at_unix_seconds(),
        "the selected offer is not the live offer the request names"
    );
    let (source_id, _) = dynamic.btc.source_for(&request.maker_identity)?;
    let commitment = offer.commitment();
    let terms = XmrReservationTerms {
        swap_id: maker_xmr_chat_swap_id(&commitment, &reservation_id),
        amount_piconero: request.foreign_units,
        lez_amount: request.expected_lez_units,
    };
    if !layout.exists() {
        fs::DirBuilder::new().mode(0o700).create(layout.root())?;
    }
    let envelope_file = layout.root().join("xmr-signed-envelope.bin");
    write_private_exact(&envelope_file, offer.signed_envelope())?;
    let monero = dynamic.chain_view().await?;
    let planner = dynamic.config.stage_a_planner();
    let XmrNegotiated { reserve, windows } = negotiate(
        &XmrNegotiation {
            config: &dynamic.config,
            planner: &planner,
            lez: dynamic.lez_role(),
            swap_root: layout.root(),
            reservation_id: reservation_id.clone(),
            offer_id: request.offer_id.clone(),
            signed_offer_envelope: offer.signed_envelope(),
            terms: terms.clone(),
            maker_identity: request.maker_identity,
            monero,
        },
        &SocketChat(dynamic.btc.chat_socket()),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(now, |elapsed| elapsed.as_secs()),
    )
    .await?;

    let role_root = layout.root().join(ROLE_DIRECTORY);
    let swap_id =
        SwapId::new(reserve.swap_id.clone()).map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let facts = serde_json::to_vec_pretty(&XmrTakerSwapFactsV1 {
        schema_version: 1,
        swap_id: swap_id.clone(),
        reservation_id: reservation_id.clone(),
        monero_restore_height: reserve.monero_restore_height,
        lez_anchor_unix_seconds: reserve.lez_anchor_unix_seconds,
        maker_xmr_funding_cutoff_ms: windows.maker_xmr_funding_cutoff_ms(),
        refund_at_ms: windows.refund_at_ms(),
        punish_at_ms: windows.punish_at_ms(),
    })?;
    let facts_file = layout.root().join(SWAP_FACTS_FILE);
    write_private_exact(&facts_file, &facts)?;
    let key_file = layout.root().join(AGREEMENT_KEY_COPY);
    write_private_exact(&key_file, &*agreement_key_bytes(&role_root)?)?;
    let stage_b_file = role_root.join(XMR_STAGE_B_FILE);
    let stage_b = read_private(&stage_b_file, MAX_FACTS_BYTES)?;
    let configured = PreparedConfigurationV1 {
        source_id: source_id.into(),
        swap_id,
        offer_id: request.offer_id.clone(),
        reservation_id,
        foreign_units: request.foreign_units,
        lez_units: request.expected_lez_units,
        signed_envelope: ImmutablePrivateFileV1 {
            path: envelope_file,
            sha256: commitment,
        },
        // The activated Stage B: it commits Stage A and both sessions, and
        // stays small where the countersigned Stage A carries two proofs.
        unsigned_draft: ImmutablePrivateFileV1 {
            path: stage_b_file,
            sha256: Sha256::digest(&stage_b).into(),
        },
        signing_key: SecretPrivateFileV1 { path: key_file },
        source_config: ImmutablePrivateFileV1 {
            path: facts_file,
            sha256: Sha256::digest(&facts).into(),
        },
        agreement_output: role_root.join(XMR_STAGE_A_FILE),
        actor_root: layout.root().join("xmr-actor"),
        receipt_output: layout.root().join("xmr-receipt.json"),
    };
    write_private_exact(&prepared_file, &serde_json::to_vec_pretty(&configured)?)?;
    Ok(configured)
}

#[cfg(test)]
pub(super) mod tests {
    use std::{collections::BTreeMap, os::unix::fs::PermissionsExt as _, sync::Mutex};

    use lez_bridge_protocol::{Hex32, Participant, RuntimeCompatibility, RuntimeDescriptor};
    use lez_node_common::{XmrCeremonyPartialRequestV1, XmrReserveRequestV1};
    use lez_xmr_swap_sdk::{
        MoneroAddressNetworkV1, MoneroSharedAddressV1, ValidatedXmrAgreementBodyV1,
        XmrAgreementBodyV1, XmrLezTermsV1, XmrMessagesV1, XmrMoneroTermsV1, XmrNamedProfileV1,
        XmrParticipantsV1, XmrSwapDirectionV1,
    };
    use secp256k1::{PublicKey, Secp256k1, SecretKey};
    use xmr_reference_actor::{
        ValidatedRolePacket, XMR_ADAPTOR_JOURNAL_FILE, XMR_MAKER_PACKET_FILE,
        XMR_TAKER_PACKET_FILE, XmrMakerCeremony, XmrStageAPlanInputV1, XmrTakerContribution,
        accept_xmr_maker_stage_a, provision_xmr_taker_actor_from_material, reserve_xmr_maker_role,
        xmr_ceremony_partial_response,
    };

    use super::*;

    const NOW: u64 = 1_900_000_000;
    const MAKER_SECRET: [u8; 32] = [7; 32];
    const MONERO: XmrMoneroChainView = XmrMoneroChainView {
        genesis: [31; 32],
        tip_height: 150,
    };

    /// A valid Stage A from the planner input, with message hashes derived
    /// from the whole input in place of the LEZ ones (the chain-free stand-in
    /// for `lez-v02-xmr-stage-a-plan`, as xmr-reference-actor's tests use).
    struct FixturePlanner;

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
                lez_xmr_swap_sdk::XmrWindowsV1::new(
                    input.maker_xmr_funding_cutoff_ms,
                    input.refund_at_ms,
                    input.punish_at_ms,
                ),
            );
            Ok(ValidatedXmrAgreementBodyV1::validate(body)?.encode_unsigned_wire()?)
        }
    }

    fn private(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn private_dir(path: &Path) -> PathBuf {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::canonicalize(path).unwrap()
    }

    fn config(root: &Path, wallet: &str) -> XmrRoleConfigV1 {
        private(&root.join("daemon.login"), b"daemon:0123abcd\n");
        private(&root.join("wallet.login"), b"wallet:4567cdef\n");
        private(&root.join("planner"), b"#!/bin/sh\nexit 1\n");
        let value = serde_json::json!({
            "schema_version": 1,
            "profile": "accelerated-regtest",
            "monerod_endpoint": "http://127.0.0.1:18081/",
            "monerod_login_file": root.join("daemon.login"),
            "wallet_rpc_endpoint": "http://127.0.0.1:18083/",
            "wallet_rpc_login_file": root.join("wallet.login"),
            "wallet_name": wallet,
            "funding_cutoff_seconds": 260,
            "refund_seconds": 360,
            "punish_seconds": 480,
            "stage_a_planner": root.join("planner"),
        });
        private(
            &root.join("xmr-role.json"),
            &serde_json::to_vec(&value).unwrap(),
        );
        XmrRoleConfigV1::load(&root.join("xmr-role.json")).unwrap()
    }

    fn lez(role: Participant, account: u8) -> XmrLezRole {
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

    fn terms() -> XmrReservationTerms {
        XmrReservationTerms {
            swap_id: [19; 32],
            amount_piconero: 1_000_000_000_000,
            lez_amount: 500,
        }
    }

    /// The Maker's side of the six rounds over the S3 library, in process:
    /// what S5's Chat methods do, minus the store and the reservation file.
    pub(in crate::taker_service) struct FakeMaker {
        config: XmrRoleConfigV1,
        pub(in crate::taker_service) root: PathBuf,
        reserve: Mutex<Option<XmrReserveResponseV1>>,
        calls: Mutex<BTreeMap<&'static str, usize>>,
        /// Fails the first call of this method once (a lost answer).
        fail_once: Mutex<Option<&'static str>>,
        /// Rewrites the R1 answer before it is sent.
        tamper: Option<fn(&mut XmrReserveResponseV1)>,
    }

    impl FakeMaker {
        fn calls(&self, method: &str) -> usize {
            self.calls.lock().unwrap().get(method).copied().unwrap_or(0)
        }
    }

    #[async_trait::async_trait]
    impl XmrMakerChat for FakeMaker {
        async fn call(
            &self,
            method: &'static str,
            request: serde_json::Value,
        ) -> Result<serde_json::Value> {
            *self.calls.lock().unwrap().entry(method).or_default() += 1;
            if *self.fail_once.lock().unwrap() == Some(method) {
                *self.fail_once.lock().unwrap() = None;
                anyhow::bail!("{method}: transport lost the answer");
            }
            let maker_root = self.root.join(ROLE_DIRECTORY);
            Ok(match method {
                "xmr_reserve_v1" => {
                    let request: XmrReserveRequestV1 = serde_json::from_value(request)?;
                    let mut kept = self.reserve.lock().unwrap();
                    if kept.is_none() {
                        let reservation = reserve_xmr_maker_role(
                            &self.config,
                            &FixturePlanner,
                            &terms(),
                            &lez(Participant::Maker, 0x11),
                            &Zeroizing::new(MAKER_SECRET),
                            &XmrTakerContribution::from(&request),
                            MONERO,
                            4,
                            NOW,
                            &maker_root,
                        )?;
                        let mut answer =
                            reservation.reserve_response(&hex::encode(terms().swap_id), false);
                        if let Some(tamper) = self.tamper {
                            tamper(&mut answer);
                        }
                        *kept = Some(answer);
                    }
                    serde_json::to_value(kept.clone().unwrap())?
                }
                "xmr_chat_stage_a_v1" => {
                    let request: XmrChatStageARequestV1 = serde_json::from_value(request)?;
                    let agreement = accept_xmr_maker_stage_a(&maker_root, &request.stage_a_wire)?;
                    serde_json::to_value(XmrChatStageAResponseV1 {
                        schema_version: 1,
                        offer_revision: 2,
                        was_replay: false,
                        reservation_id: request.reservation_id,
                        lez_units: terms().lez_amount,
                        swap_id: hex::encode(terms().swap_id).into(),
                        agreement_commitment: agreement.agreement_commitment(),
                    })?
                }
                "xmr_ceremony_reserve_v1" => {
                    let request: XmrCeremonyReserveRequestV1 = serde_json::from_value(request)?;
                    let commitments = XmrMakerCeremony::open(&maker_root)?
                        .reserve_round(&request.taker_commitments.into())?;
                    serde_json::to_value(XmrCeremonyReserveResponseV1 {
                        schema_version: 1,
                        was_replay: false,
                        maker_commitments: commitments.into(),
                    })?
                }
                "xmr_ceremony_nonce_v1" => {
                    let request: XmrCeremonyNonceRequestV1 = serde_json::from_value(request)?;
                    let (nonces, partials) = XmrMakerCeremony::open(&maker_root)?
                        .nonce_round(&request.taker_nonces.into())?;
                    serde_json::to_value(XmrCeremonyNonceResponseV1 {
                        schema_version: 1,
                        was_replay: false,
                        maker_nonces: nonces.into(),
                        maker_partials: partials.into(),
                    })?
                }
                "xmr_ceremony_partial_v1" => {
                    let request: XmrCeremonyPartialRequestV1 = serde_json::from_value(request)?;
                    let answer = XmrMakerCeremony::open(&maker_root)?.partial_round(
                        &request.taker_refund_partial,
                        &request.unsigned_stage_b_wire,
                        request.taker_stage_b_signature,
                    )?;
                    serde_json::to_value(xmr_ceremony_partial_response(answer, false))?
                }
                other => anyhow::bail!("unexpected method {other}"),
            })
        }
    }

    pub(in crate::taker_service) struct Take {
        _directory: tempfile::TempDir,
        taker_config: XmrRoleConfigV1,
        pub(in crate::taker_service) taker_swap: PathBuf,
        pub(in crate::taker_service) maker: FakeMaker,
    }

    pub(in crate::taker_service) fn take(tamper: Option<fn(&mut XmrReserveResponseV1)>) -> Take {
        let directory = tempfile::TempDir::new().unwrap();
        let root = fs::canonicalize(directory.path()).unwrap();
        let taker_config = config(&private_dir(&root.join("taker-config")), "taker");
        let maker_config = config(&private_dir(&root.join("maker-config")), "maker");
        let taker_swap = private_dir(&root.join("taker-swap"));
        let maker_swap = private_dir(&root.join("maker-swap"));
        Take {
            _directory: directory,
            taker_config,
            taker_swap,
            maker: FakeMaker {
                config: maker_config,
                root: maker_swap,
                reserve: Mutex::new(None),
                calls: Mutex::new(BTreeMap::new()),
                fail_once: Mutex::new(None),
                tamper,
            },
        }
    }

    impl Take {
        pub(in crate::taker_service) async fn negotiate(&self) -> Result<XmrNegotiated> {
            let maker_identity = PublicKey::from_secret_key(
                &Secp256k1::new(),
                &SecretKey::from_slice(&MAKER_SECRET).unwrap(),
            )
            .serialize();
            negotiate(
                &XmrNegotiation {
                    config: &self.taker_config,
                    planner: &FixturePlanner,
                    lez: lez(Participant::Taker, 0x22),
                    swap_root: &self.taker_swap,
                    reservation_id: RequestId::new("xmr-reservation-001").unwrap(),
                    offer_id: MakerOfferId::new("xmr-offer-001").unwrap(),
                    signed_offer_envelope: &[1; 16],
                    terms: terms(),
                    maker_identity,
                    monero: MONERO,
                },
                &self.maker,
                NOW + 5,
            )
            .await
        }
    }

    const ROUNDS: [&str; 5] = [
        "xmr_reserve_v1",
        "xmr_chat_stage_a_v1",
        "xmr_ceremony_reserve_v1",
        "xmr_ceremony_nonce_v1",
        "xmr_ceremony_partial_v1",
    ];

    /// R1–R5 against the library Maker: the Maker accepts the Stage B the
    /// Taker activated, the windows are the agreement's, the Taker's role
    /// root provisions its actor as the prepared take does, and a replayed
    /// take sends nothing again.
    #[tokio::test]
    async fn the_take_reaches_one_activated_agreement_with_the_maker() {
        let take = take(None);
        let negotiated = take.negotiate().await.unwrap();
        let taker_root = take.taker_swap.join(ROLE_DIRECTORY);
        let maker_root = take.maker.root.join(ROLE_DIRECTORY);
        // R6 is the prepared take's; the Maker accepts what the Taker activated.
        let stage_b = fs::read(taker_root.join(XMR_STAGE_B_FILE)).unwrap();
        XmrMakerCeremony::open(&maker_root)
            .unwrap()
            .accept_activation(&stage_b)
            .unwrap();
        assert_eq!(
            fs::read(maker_root.join(XMR_STAGE_B_FILE)).unwrap(),
            stage_b
        );
        assert_eq!(
            negotiated.windows,
            take.taker_config.windows(NOW).unwrap(),
            "the windows are counted from the Maker's anchor"
        );
        for method in ROUNDS {
            assert_eq!(take.maker.calls(method), 1, "{method}");
        }
        // The record keeps no secret: the R1 request is rebuilt, not kept.
        let record = fs::read(take.taker_swap.join(RESERVE_FILE)).unwrap();
        let view_key = xmr_taker_contribution(&taker_root)
            .unwrap()
            .taker_private_view_key;
        let encoded = serde_json::to_vec(&*view_key).unwrap();
        assert!(!record.windows(encoded.len()).any(|bytes| bytes == encoded));

        take.negotiate().await.unwrap();
        for method in ROUNDS {
            assert_eq!(take.maker.calls(method), 1, "{method} was resent");
        }

        let provisioned = provision_xmr_taker_actor_from_material(
            &taker_root,
            &taker_root.join(XMR_TAKER_PACKET_FILE),
            &taker_root.join(XMR_MAKER_PACKET_FILE),
            &taker_root.join(XMR_STAGE_A_FILE),
            &taker_root.join(XMR_STAGE_B_FILE),
            &taker_root.join(XMR_ADAPTOR_JOURNAL_FILE),
            &take.taker_swap.join("xmr-actor"),
        )
        .unwrap();
        assert_eq!(provisioned.swap_id(), terms().swap_id);
    }

    /// A lost answer leaves the take resumable: the next attempt resends only
    /// the round whose answer it lacks, and the ceremony replays its packets.
    #[tokio::test]
    async fn a_lost_ceremony_answer_resumes_from_that_round() {
        let take = take(None);
        *take.maker.fail_once.lock().unwrap() = Some("xmr_ceremony_nonce_v1");
        assert!(take.negotiate().await.is_err());
        assert!(
            !take
                .taker_swap
                .join(ROLE_DIRECTORY)
                .join(XMR_STAGE_B_FILE)
                .exists()
        );
        take.negotiate().await.unwrap();
        for method in ROUNDS {
            let expected = usize::from(method == "xmr_ceremony_nonce_v1") + 1;
            assert_eq!(take.maker.calls(method), expected, "{method}");
        }
    }

    /// A Maker whose Stage A is not the one the Taker composes is refused
    /// before the Taker countersigns or sends anything further.
    #[tokio::test]
    async fn a_maker_stage_a_the_taker_does_not_compose_is_refused() {
        let take = take(Some(|answer| answer.maker_owner_nonce += 1));
        assert!(take.negotiate().await.is_err());
        assert!(
            !take
                .taker_swap
                .join(ROLE_DIRECTORY)
                .join(XMR_STAGE_A_FILE)
                .exists()
        );
        assert_eq!(take.maker.calls("xmr_chat_stage_a_v1"), 0);

        let take = take_with_anchor_skew();
        assert!(take.negotiate().await.is_err());
        assert_eq!(take.maker.calls("xmr_chat_stage_a_v1"), 0);
    }

    fn take_with_anchor_skew() -> Take {
        take(Some(|answer| {
            answer.lez_anchor_unix_seconds -= 3_600;
        }))
    }

    /// An answer for another swap is refused.
    #[tokio::test]
    async fn an_answer_for_another_swap_is_refused() {
        let take = take(Some(|answer| answer.swap_id = hex::encode([20; 32]).into()));
        let error = take.negotiate().await.err().unwrap();
        assert!(format!("{error:#}").contains("another swap"));
    }
}
