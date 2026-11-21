//! Maker side of the take-time Monero agreement (`xmr_reserve_v1` and its
//! ceremony rounds, docs/sdk-lez-xmr.md): the Zcash model of `zec_reserve.rs`
//! with the adaptor ceremony the Bitcoin lifecycle runs over Chat.
//!
//! A reservation costs the Maker one monerod read pass and a few private
//! files in the reservation's swap directory (beside the Bitcoin and Zcash
//! swaps, so all share the sidecar port bookkeeping): its per-swap role root
//! (keys, Monero share, the Taker's per-swap view key, both packets, the
//! unsigned Stage A and later the adaptor journal) and the record of the
//! round. The swap's sidecar, actor bundle and effect authority are made only
//! at `xmr_chat_activate_v1`, once a countersigned Stage B exists; from then on
//! the actor is in the store's registry and its sidecar is kept running, also
//! across a Node restart.

use std::{
    fs,
    os::unix::fs::DirBuilderExt as _,
    path::{Path, PathBuf},
};

use anyhow::Context as _;
use lez_btc_role_lifecycle::{
    BtcRoleRuntime, SwapLayout, SwapSidecar,
    layout::{read_private, write_private_exact},
};
use lez_xmr_monero_adapter::{MoneroChainIdentityAttestor, MoneroNetwork};
use lez_xmr_swap_sdk::MoneroPrivateViewKey;
use xmr_reference_actor::{
    ActorRole, XMR_ADAPTOR_JOURNAL_FILE, XMR_MAKER_PACKET_FILE, XMR_STAGE_A_FILE, XMR_STAGE_B_FILE,
    XMR_TAKER_PACKET_FILE, XmrCeremonyPackets, XmrLezRole, XmrMakerCeremony, XmrMakerEffectFactsV1,
    XmrMakerEffectToolPinsV1, XmrMoneroChainView, XmrReservationTerms, XmrRoleConfigV1,
    XmrTakerContribution, XmrTakerStageB, accept_xmr_maker_stage_a,
    provision_xmr_effect_manifest_v3, provision_xmr_maker_actor_from_material,
    provision_xmr_maker_effect_authority, reserve_xmr_maker_role, seed_xmr_maker_lez_scan,
    xmr_ceremony_partial_response,
};
use zeroize::Zeroizing;

use super::*;

/// Reservations not yet activated that may exist at once.
const MAX_LIVE_RESERVATIONS: usize = 16;
/// How long an unactivated reservation stays open; a later round is refused.
const RESERVATION_GRACE_SECONDS: u64 = 900;
/// The record holds the R1 answer: two role packets with their DLEQ proofs.
const MAX_RECORD_BYTES: usize = 2 * 1024 * 1024;
const MAX_ROUND_RECORD_BYTES: usize = 64 * 1024;
const RECORD_FILE: &str = "xmr-reservation.json";
/// The Maker's per-swap role root inside the swap directory.
const ROLE_DIRECTORY: &str = "xmr";
/// The per-swap view key the Taker handed over, in the role root.
const VIEW_KEY_FILE: &str = "monero-view.key";
/// The swap's effect authority, its credentials, workflow journal and the
/// schema-3 actor manifest the store registers.
const EFFECT_DIRECTORY: &str = "xmr-effect";
const ACTOR_MANIFEST_FILE: &str = "actor-manifest.json";
const WORKFLOW_JOURNAL_FILE: &str = "workflow.sqlite3";
/// The Maker owner's LEZ nonce Stage A commits. It feeds only the punish
/// message hash, which the escrow guest does not check (the punish is signed
/// by the claimant directly), and no sidecar exposes an account read, so the
/// Maker commits 0 and the punish tool signs with the live nonce.
const MAKER_OWNER_NONCE: u128 = 0;

/// The Maker's take-time Monero authority.
pub struct XmrMakerReservations {
    config: XmrRoleConfigV1,
    tools: XmrMakerEffectToolPinsV1,
    /// The Node's LEZ role (identity, signer, swaps root, sidecar recipe),
    /// read from the same role configuration as its Bitcoin lifecycle.
    runtime: BtcRoleRuntime,
    actor_root: PathBuf,
    actor_program: PathBuf,
    actor_program_sha256: [u8; 32],
    rounds: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for XmrMakerReservations {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("XmrMakerReservations")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// What the Maker decided at reservation time.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct XmrReservationRecordV1 {
    schema_version: u16,
    request_sha256: [u8; 32],
    response: XmrReserveResponseV1,
    expires_at_unix_seconds: u64,
    amount_piconero: u64,
}

impl XmrReservationRecordV1 {
    fn load(root: &Path) -> anyhow::Result<Self> {
        let bytes = read_private(&root.join(RECORD_FILE), MAX_RECORD_BYTES)?;
        serde_json::from_slice(&bytes).context("Monero reservation record")
    }
}

/// One completed ceremony round: the request digest and the answer it got.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct XmrRoundRecordV1 {
    request_sha256: [u8; 32],
    response: serde_json::Value,
}

fn internal(error: &dyn std::fmt::Display) -> ErrorObjectOwned {
    eprintln!("maker XMR reservation: {error}");
    rpc_error(INTERNAL_ERROR, "maker XMR reservation step failed")
}

fn request_digest<T: Serialize>(request: &T) -> RpcResult<[u8; 32]> {
    let bytes = serde_json::to_vec(request).map_err(|error| internal(&error))?;
    Ok(Sha256::digest(bytes).into())
}

fn role_root(layout: &SwapLayout) -> PathBuf {
    layout.root().join(ROLE_DIRECTORY)
}

fn actor_manifest_file(layout: &SwapLayout) -> PathBuf {
    layout
        .root()
        .join(EFFECT_DIRECTORY)
        .join(ACTOR_MANIFEST_FILE)
}

/// Returns the stored answer when this exact request already completed the
/// round, fails on a different request for it, else `None`.
fn replay<T: for<'de> Deserialize<'de>>(
    layout: &SwapLayout,
    round: &str,
    digest: [u8; 32],
) -> RpcResult<Option<T>> {
    let path = layout.root().join(format!("round-xmr-{round}.json"));
    if !path.exists() {
        return Ok(None);
    }
    let record: XmrRoundRecordV1 = serde_json::from_slice(
        &read_private(&path, MAX_ROUND_RECORD_BYTES).map_err(|error| internal(&error))?,
    )
    .map_err(|error| internal(&error))?;
    if record.request_sha256 != digest {
        return Err(rpc_error(
            CONFLICT,
            "a different request already completed this round",
        ));
    }
    serde_json::from_value(record.response)
        .map(Some)
        .map_err(|error| internal(&error))
}

fn record_round<T: Serialize>(
    layout: &SwapLayout,
    round: &str,
    digest: [u8; 32],
    response: &T,
) -> RpcResult<()> {
    let record = XmrRoundRecordV1 {
        request_sha256: digest,
        response: serde_json::to_value(response).map_err(|error| internal(&error))?,
    };
    write_private_exact(
        &layout.root().join(format!("round-xmr-{round}.json")),
        &serde_json::to_vec(&record).map_err(|error| internal(&error))?,
    )
    .map(|_| ())
    .map_err(|error| internal(&error))
}

impl XmrMakerReservations {
    /// Binds the Monero role configuration to the LEZ role of the Node-owned
    /// lifecycle's configuration file (identity, signer, swaps root and
    /// sidecar recipe, read as that lifecycle reads them), pins the Maker's
    /// effect tools, and names where activated swaps' actor bundles go and
    /// which actor program runs them.
    ///
    /// # Errors
    ///
    /// Fails when either configuration is invalid, a Maker effect tool is
    /// missing, or the actor root or program is unsafe.
    pub fn load(
        config_file: &Path,
        role_config_file: &Path,
        actor_root: &Path,
        actor_program: &Path,
        actor_program_sha256: [u8; 32],
    ) -> anyhow::Result<Self> {
        let config = XmrRoleConfigV1::load(config_file)?;
        validate_maker_actor_program(actor_program, actor_program_sha256)
            .context("validate the XMR Maker actor program")?;
        Ok(Self {
            tools: XmrMakerEffectToolPinsV1::pin(&config)?,
            config,
            runtime: BtcRoleRuntime::load(Participant::Maker, role_config_file)?,
            actor_root: actor_root.to_path_buf(),
            actor_program: actor_program.to_path_buf(),
            actor_program_sha256,
            rounds: tokio::sync::Mutex::new(()),
        })
    }

    fn layout(&self, reservation_id: &RequestId) -> SwapLayout {
        SwapLayout::new(&self.runtime.config().swaps_root, reservation_id)
    }

    /// Every reservation recorded under the swaps root.
    fn records(&self) -> Vec<(RequestId, SwapLayout, XmrReservationRecordV1)> {
        let Ok(entries) = fs::read_dir(&self.runtime.config().swaps_root) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let reservation_id = RequestId::new(entry.file_name().to_str()?.to_owned()).ok()?;
                let layout = self.layout(&reservation_id);
                let record = XmrReservationRecordV1::load(layout.root()).ok()?;
                Some((reservation_id, layout, record))
            })
            .collect()
    }

    /// The reservation's layout while a round may still use it: recorded,
    /// and inside its grace unless its actor is already provisioned.
    fn open(&self, reservation_id: &RequestId, now: u64) -> RpcResult<SwapLayout> {
        let layout = self.layout(reservation_id);
        let record = XmrReservationRecordV1::load(layout.root())
            .map_err(|_| invalid_request("unknown XMR reservation"))?;
        if !actor_manifest_file(&layout).is_file() && now >= record.expires_at_unix_seconds {
            return Err(invalid_request("the XMR reservation expired"));
        }
        Ok(layout)
    }

    /// This Maker's monerod: its genesis and its tip height, which becomes
    /// the agreement's restore height.
    async fn chain_view(&self) -> anyhow::Result<XmrMoneroChainView> {
        let attestor =
            MoneroChainIdentityAttestor::new(MoneroNetwork::Regtest, &self.config.monerod()?)?;
        let identity = attestor.discover().await?;
        Ok(XmrMoneroChainView {
            genesis: identity.genesis_hash(),
            tip_height: attestor.tip_height().await?,
        })
    }

    /// Accepts the countersigned Stage A of a reservation (R2) when it is
    /// exactly the unsigned Stage A this Maker reserved, and keeps it in the
    /// role root for the ceremony.
    pub(super) fn check_stage_a(
        &self,
        reservation_id: &RequestId,
        stage_a_wire: &[u8],
        now: u64,
    ) -> RpcResult<()> {
        let _serial = self.rounds.blocking_lock();
        let layout = self.open(reservation_id, now)?;
        accept_xmr_maker_stage_a(&role_root(&layout), stage_a_wire)
            .map(|_| ())
            .map_err(|error| invalid_request(format!("XMR Stage A refused: {error}")))
    }

    /// Accepts the activated Stage B (R6) with the swap's own view key and,
    /// unless done before, provisions the swap's actor: its sidecar, the
    /// actor bundle, the effect authority and the schema-3 manifest the
    /// existing activation commits to the store.
    pub(super) fn activate(
        &self,
        reservation_id: &RequestId,
        agreement: &XmrAgreementV1,
        activation_wire: &[u8],
        now: u64,
    ) -> RpcResult<(XmrActivatedAgreementV1, MakerActorManifestV1)> {
        let _serial = self.rounds.blocking_lock();
        let layout = self.open(reservation_id, now)?;
        let root = role_root(&layout);
        let manifest = actor_manifest_file(&layout);
        let activation = if manifest.is_file() {
            // A replay: the provisioned actor pinned the adaptor journal's
            // bytes, so the journal is not opened again; the Stage B must be
            // the one kept, and it is validated with the swap's view key.
            replayed_activation(&root, agreement, activation_wire)
                .map_err(|error| invalid_request(format!("XMR Stage B refused: {error}")))?
        } else {
            let activation = XmrMakerCeremony::open(&root)
                .and_then(|ceremony| ceremony.accept_activation(activation_wire))
                .map_err(|error| invalid_request(format!("XMR Stage B refused: {error}")))?;
            self.provision(reservation_id, &layout, &activation)
                .map_err(|error| internal(&format!("{error:#}")))?;
            activation
        };
        let bytes = read_private(&manifest, MAX_RECORD_BYTES).map_err(|error| internal(&error))?;
        let actor = MakerActorManifestV1::new(
            SwapId::new(hex::encode(agreement.body().swap_id()))
                .map_err(|error| internal(&error))?,
            MakerActorKindV1::Monero,
            manifest,
            Sha256::digest(bytes).into(),
            self.actor_program.clone(),
            self.actor_program_sha256,
            root.join(XMR_ADAPTOR_JOURNAL_FILE),
        )
        .map_err(|error| internal(&error))?;
        Ok((activation, actor))
    }

    fn provision(
        &self,
        reservation_id: &RequestId,
        layout: &SwapLayout,
        activation: &XmrActivatedAgreementV1,
    ) -> anyhow::Result<()> {
        let root = role_root(layout);
        let sidecar = SwapSidecar::ensure(&self.runtime, layout, reservation_id)?;
        let stage_b = root.join(XMR_STAGE_B_FILE);
        let stage_b_sha256: [u8; 32] = Sha256::digest(fs::read(&stage_b)?).into();
        let bundle = provision_xmr_maker_actor_from_material(
            &root,
            &root.join(XMR_MAKER_PACKET_FILE),
            &root.join(XMR_TAKER_PACKET_FILE),
            &root.join(XMR_STAGE_A_FILE),
            &stage_b,
            &root.join(XMR_ADAPTOR_JOURNAL_FILE),
            &self.actor_root.join(hex::encode(stage_b_sha256)),
        )?;
        let effect = layout.root().join(EFFECT_DIRECTORY);
        if !effect.is_dir() {
            fs::DirBuilder::new().mode(0o700).create(&effect)?;
        }
        let workflow = effect.join(WORKFLOW_JOURNAL_FILE);
        let wallet_lock_file = self.wallet_lock_file()?;
        let authority = provision_xmr_maker_effect_authority(
            &self.config,
            &self.tools,
            &XmrMakerEffectFactsV1 {
                swap_id: bundle.swap_id(),
                agreement_commitment: bundle.agreement_commitment(),
                activation_commitment: activation.activation_commitment(),
                run_id: sidecar.run_id().as_str(),
                workflow_journal: &workflow,
                adaptor_journal: &root.join(XMR_ADAPTOR_JOURNAL_FILE),
                sidecar_url: &sidecar.endpoint(),
                // Where `SwapSidecar::spawn` writes the descriptor it runs under.
                runtime_file: &layout.root().join("sidecar-state").join("runtime.json"),
                capability_file: sidecar.capability_file(),
                wallet_lock_file: &wallet_lock_file,
            },
            &effect,
        )?;
        // The actor's search for the Taker's lock starts at the finalized
        // height now (the Taker locks once activated), not a fixed lookback
        // from its first drive's tip: one sidecar page per 16 blocks, seconds
        // each under load. Best effort: without it the actor looks back.
        match sidecar.client(&self.runtime).and_then(|client| {
            tokio::runtime::Handle::current().block_on(client.finalized_height())
        }) {
            Ok(height) => seed_xmr_maker_lez_scan(&effect, height)?,
            Err(error) => eprintln!(
                "maker XMR reservation {}: no finalized LEZ height to seed the lock search: {error:#}",
                reservation_id.as_str()
            ),
        }
        let _ = provision_xmr_effect_manifest_v3(
            bundle.manifest_file(),
            ActorRole::Maker,
            &authority,
            &workflow,
            sidecar.run_id().as_str(),
            &effect.join(ACTOR_MANIFEST_FILE),
        )?;
        Ok(())
    }

    /// The one lock every Monero wallet-rpc user of this role holds
    /// (`<node state>/monero-rpc/wallet.lock`, beside the swaps root), as
    /// the Taker Node's.
    fn wallet_lock_file(&self) -> anyhow::Result<PathBuf> {
        let state = self
            .runtime
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

    /// Keeps the sidecars of activated reservations running after a restart,
    /// as the Bitcoin lifecycle keeps its swaps'. The actors themselves are
    /// in the store's durable registry.
    pub(super) fn restore(self: &Arc<Self>) {
        let reservations = Arc::clone(self);
        std::thread::spawn(move || {
            loop {
                for (reservation_id, layout, _) in reservations.records() {
                    if actor_manifest_file(&layout).is_file()
                        && let Err(error) =
                            SwapSidecar::ensure(&reservations.runtime, &layout, &reservation_id)
                    {
                        eprintln!(
                            "maker XMR reservation {}: sidecar unavailable: {error:#}",
                            reservation_id.as_str()
                        );
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(15));
            }
        });
    }
}

/// The activated Stage B of an already provisioned swap, checked against the
/// one its role root keeps and validated with the swap's own view key.
fn replayed_activation(
    root: &Path,
    agreement: &XmrAgreementV1,
    activation_wire: &[u8],
) -> anyhow::Result<XmrActivatedAgreementV1> {
    anyhow::ensure!(
        read_private(&root.join(XMR_STAGE_B_FILE), MAX_XMR_ACTIVATION_WIRE_BYTES)?
            == activation_wire,
        "the Stage B is not the one this swap activated"
    );
    let encoded = Zeroizing::new(read_private(&root.join(VIEW_KEY_FILE), 128)?);
    let mut view_key = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(encoded.trim_ascii(), view_key.as_mut_slice())
        .context("the swap's Monero view key is not hex")?;
    let view_key = MoneroPrivateViewKey::from_monero_little_endian(*view_key)?;
    Ok(XmrActivatedAgreementV1::from_wire(
        agreement,
        activation_wire,
        &view_key,
    )?)
}

fn reservations(context: &MakerRpc) -> RpcResult<&XmrMakerReservations> {
    context
        .xmr_reservations
        .as_deref()
        .ok_or_else(|| invalid_request("maker XMR reservations are unavailable"))
}

/// R1. The request carries the per-swap private view key: it is never logged.
#[allow(clippy::too_many_lines)]
pub(super) async fn reserve(
    request: XmrReserveRequestV1,
    context: Arc<MakerRpc>,
) -> RpcResult<XmrReserveResponseV1> {
    if request.schema_version != 1 || request.foreign_units == 0 {
        return Err(invalid_request("unsupported or empty XMR reservation"));
    }
    // The id names the swap directory; decoding already refused any other.
    if !request.reservation_id.is_reservation_id() {
        return Err(invalid_request("the XMR reservation id is not path-safe"));
    }
    let reservations = reservations(&context)?;
    let signing_key = context
        .chat_signing_key
        .as_ref()
        .ok_or_else(|| invalid_request("maker Chat signer is unavailable"))?;
    let _serial = reservations.rounds.lock().await;
    let layout = reservations.layout(&request.reservation_id);
    let digest = request_digest(&request)?;
    if layout.root().exists() {
        return match XmrReservationRecordV1::load(layout.root()) {
            Ok(record) if record.request_sha256 == digest => Ok(XmrReserveResponseV1 {
                was_replay: true,
                ..record.response
            }),
            _ => Err(rpc_error(CONFLICT, "the reservation id is already taken")),
        };
    }
    let now = trusted_now_unix_seconds()?;
    let delivery = context
        .delivery
        .as_ref()
        .ok_or_else(|| invalid_request("maker Chat Delivery is unavailable"))?;
    let authenticated = delivery
        .authenticate_envelope(&request.signed_offer_envelope)
        .map_err(|error| invalid_request(error.to_string()))?;
    let offer = authenticated.offer();
    if offer.id() != &request.offer_id
        || offer.route().pair() != Pair::Monero
        || offer.route().direction() != SwapDirection::TakerSellsLez
        || offer.created_at_unix_seconds() > now
        || now >= offer.expires_at_unix_seconds()
        || request.foreign_units < offer.minimum_foreign_units()
        || request.foreign_units > offer.maximum_foreign_units()
    {
        return Err(invalid_request(
            "Delivery offer does not match the XMR reservation",
        ));
    }
    let lez_units = offer
        .quote_foreign_amount(request.foreign_units)
        .map_err(invalid_request)?;
    let live = reservations
        .records()
        .iter()
        .filter(|(_, layout, record)| {
            !actor_manifest_file(layout).is_file() && now < record.expires_at_unix_seconds
        })
        .count();
    if live >= MAX_LIVE_RESERVATIONS {
        return Err(rpc_error(
            OFFER_UNAVAILABLE,
            "this Maker holds too many open XMR reservations; take the offer again later",
        ));
    }
    let terms = XmrReservationTerms {
        swap_id: maker_xmr_chat_swap_id(&authenticated.commitment(), &request.reservation_id),
        amount_piconero: request.foreign_units,
        lez_amount: lez_units,
    };
    let monero = reservations
        .chain_view()
        .await
        .map_err(|error| internal(&format!("read monerod: {error:#}")))?;
    // Everything is made in a private stage outside the swaps root's id
    // namespace and published by one rename, so a crash leaves no half
    // reservation and no stage is ever another swap's directory.
    let stage = layout
        .fresh_stage("xmr")
        .map_err(|error| internal(&error))?;
    let reserved = reserve_xmr_maker_role(
        &reservations.config,
        &reservations.config.stage_a_planner(),
        &terms,
        &XmrLezRole {
            runtime: reservations.runtime.runtime_descriptor(),
        },
        &Zeroizing::new(signing_key.secret_bytes()),
        &XmrTakerContribution::from(&request),
        monero,
        MAKER_OWNER_NONCE,
        now,
        &stage.join(ROLE_DIRECTORY),
    );
    let reserved = match reserved {
        Ok(reserved) => reserved,
        Err(error) => {
            let _ = fs::remove_dir_all(&stage);
            eprintln!("maker XMR reservation refused: {error:#}");
            return Err(invalid_request(format!("XMR reservation refused: {error}")));
        }
    };
    let response = reserved.reserve_response(&hex::encode(terms.swap_id), false);
    let record = XmrReservationRecordV1 {
        schema_version: 1,
        request_sha256: digest,
        response: response.clone(),
        expires_at_unix_seconds: now
            .saturating_add(RESERVATION_GRACE_SECONDS)
            .min(offer.expires_at_unix_seconds()),
        amount_piconero: request.foreign_units,
    };
    let published = serde_json::to_vec_pretty(&record)
        .map_err(anyhow::Error::from)
        .and_then(|bytes| write_private_exact(&stage.join(RECORD_FILE), &bytes))
        .and_then(|_| fs::rename(&stage, layout.root()).map_err(anyhow::Error::from));
    if let Err(error) = published {
        let _ = fs::remove_dir_all(&stage);
        return Err(internal(&error));
    }
    Ok(response)
}

/// R3: the Maker's nonce commitments for both sessions.
pub(super) async fn ceremony_reserve(
    request: XmrCeremonyReserveRequestV1,
    context: Arc<MakerRpc>,
) -> RpcResult<XmrCeremonyReserveResponseV1> {
    if request.schema_version != 1 {
        return Err(invalid_request("unsupported XMR ceremony reservation"));
    }
    let reservations = reservations(&context)?;
    let _serial = reservations.rounds.lock().await;
    let layout = reservations.open(&request.reservation_id, trusted_now_unix_seconds()?)?;
    let digest = request_digest(&request)?;
    if let Some(response) = replay::<XmrCeremonyReserveResponseV1>(&layout, "reserve", digest)? {
        return Ok(XmrCeremonyReserveResponseV1 {
            was_replay: true,
            ..response
        });
    }
    let maker_commitments = XmrMakerCeremony::open(&role_root(&layout))
        .and_then(|mut ceremony| {
            ceremony.reserve_round(&XmrCeremonyPackets::from(request.taker_commitments.clone()))
        })
        .map_err(|error| invalid_request(format!("XMR ceremony reservation failed: {error}")))?;
    let response = XmrCeremonyReserveResponseV1 {
        schema_version: 1,
        was_replay: false,
        maker_commitments: maker_commitments.into(),
    };
    record_round(&layout, "reserve", digest, &response)?;
    Ok(response)
}

/// R4: the Maker's nonces and its partials for both sessions.
pub(super) async fn ceremony_nonce(
    request: XmrCeremonyNonceRequestV1,
    context: Arc<MakerRpc>,
) -> RpcResult<XmrCeremonyNonceResponseV1> {
    if request.schema_version != 1 {
        return Err(invalid_request("unsupported XMR ceremony nonce round"));
    }
    let reservations = reservations(&context)?;
    let _serial = reservations.rounds.lock().await;
    let layout = reservations.open(&request.reservation_id, trusted_now_unix_seconds()?)?;
    let digest = request_digest(&request)?;
    if let Some(response) = replay::<XmrCeremonyNonceResponseV1>(&layout, "nonce", digest)? {
        return Ok(XmrCeremonyNonceResponseV1 {
            was_replay: true,
            ..response
        });
    }
    let (maker_nonces, maker_partials) = XmrMakerCeremony::open(&role_root(&layout))
        .and_then(|mut ceremony| {
            ceremony.nonce_round(&XmrCeremonyPackets::from(request.taker_nonces.clone()))
        })
        .map_err(|error| invalid_request(format!("XMR ceremony nonce round failed: {error}")))?;
    let response = XmrCeremonyNonceResponseV1 {
        schema_version: 1,
        was_replay: false,
        maker_nonces: maker_nonces.into(),
        maker_partials: maker_partials.into(),
    };
    record_round(&layout, "nonce", digest, &response)?;
    Ok(response)
}

/// R5: the refund presignature and the Maker's Stage-B signature.
pub(super) async fn ceremony_partial(
    request: XmrCeremonyPartialRequestV1,
    context: Arc<MakerRpc>,
) -> RpcResult<XmrCeremonyPartialResponseV1> {
    if request.schema_version != 1 {
        return Err(invalid_request("unsupported XMR ceremony partial round"));
    }
    let reservations = reservations(&context)?;
    let _serial = reservations.rounds.lock().await;
    let layout = reservations.open(&request.reservation_id, trusted_now_unix_seconds()?)?;
    let digest = request_digest(&request)?;
    if let Some(response) = replay::<XmrCeremonyPartialResponseV1>(&layout, "partial", digest)? {
        return Ok(XmrCeremonyPartialResponseV1 {
            was_replay: true,
            ..response
        });
    }
    let stage_b = XmrTakerStageB::from(&request);
    let answer = XmrMakerCeremony::open(&role_root(&layout))
        .and_then(|mut ceremony| {
            ceremony.partial_round(
                &request.taker_refund_partial,
                &stage_b.unsigned_stage_b_wire,
                stage_b.taker_stage_b_signature,
            )
        })
        .map_err(|error| invalid_request(format!("XMR ceremony partial round failed: {error}")))?;
    let response = xmr_ceremony_partial_response(answer, false);
    record_round(&layout, "partial", digest, &response)?;
    Ok(response)
}
