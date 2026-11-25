//! Zcash Chat negotiation and daemon-owned Maker provisioning.

use std::{fs, os::unix::fs::MetadataExt as _, path::PathBuf};

use anyhow::Context as _;

use lez_zec_swap_sdk::ZecProfileId;

use super::{zec_reserve::Materialized, *};

const MAX_ZEC_MAKER_AUTHORITY_TEMPLATES: usize = 256;

#[allow(
    clippy::too_many_lines,
    reason = "the propose handler reads in order: offer, draft, capacity, signature, staging"
)]
pub(super) fn register_zec_chat_methods(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
    module.register_async_method("zec_reserve_v1", |params, context, _| async move {
        let request: ZecReserveRequestV1 = params.one()?;
        super::zec_reserve::reserve(request, context).await
    })?;
    module.register_blocking_method::<RpcResult<ZecChatProposalV1>, _>(
        "zec_chat_propose_v1",
        |params, context, _| {
            let request: ZecChatProposeRequestV1 = params.one()?;
            validate_zec_chat_shape(&request)?;
            let now_unix_seconds = trusted_now_unix_seconds()?;
            let delivery = context
                .delivery
                .as_ref()
                .ok_or_else(|| invalid_request("maker Chat Delivery is unavailable"))?;
            let signing_key = context
                .chat_signing_key
                .as_ref()
                .ok_or_else(|| invalid_request("maker Chat signer is unavailable"))?;
            let authenticated = delivery
                .authenticate_envelope(&request.signed_offer_envelope)
                .map_err(|error| invalid_request(error.to_string()))?;
            let offer = authenticated.offer();
            if offer.id() != &request.offer_id
                || offer.route().pair() != Pair::Zcash
                || offer.created_at_unix_seconds() > now_unix_seconds
                || now_unix_seconds >= offer.expires_at_unix_seconds()
            {
                return Err(invalid_request(
                    "Delivery offer does not match live ZEC Chat request",
                ));
            }
            let validated = ZecAgreementDraftV1::from_wire_at(
                &request.unsigned_draft_wire,
                lez_swap_core::UnixSeconds::new(now_unix_seconds),
            )
            .map_err(invalid_request)?;
            let maker_key = PublicKey::from_secret_key(&Secp256k1::signing_only(), signing_key);
            let maker_identity = maker_key.serialize();
            let taker_identity = validated.taker_zcash_key().serialize();
            let lez_units = offer
                .quote_foreign_amount(request.foreign_units)
                .map_err(invalid_request)?;
            let offer_commitment = authenticated.commitment();
            if !zec_draft_matches_offer(
                &validated,
                &authenticated,
                offer,
                &maker_key,
                &request.reservation_id,
                request.foreign_units,
                lez_units,
            ) {
                return Err(invalid_request(
                    "unsigned ZEC draft is not bound to the selected offer",
                ));
            }
            if let Some(reservations) = &context.zec_reservations {
                reservations.check_proposal(
                    &request.reservation_id,
                    &request.unsigned_draft_wire,
                    now_unix_seconds,
                )?;
            }
            if validated.body().direction() == SwapDirection::TakerSellsLez
                && let Some(completion_store) = context.zec_completion_store.as_ref()
            {
                refuse_at_taker_sells_lez_capacity(
                    completion_store,
                    validated.body().profile_id(),
                    now_unix_seconds,
                )?;
            }
            let agreement_commitment = validated.commitment();
            let signature = Secp256k1::signing_only()
                .sign_ecdsa(&Message::from_digest(agreement_commitment), signing_key)
                .serialize_compact();
            let proposal = validated
                .with_maker_signature(signature)
                .map_err(invalid_request)?;
            let proposal_wire = proposal.encode_wire().map_err(invalid_request)?;
            let negotiation = MakerZecNegotiationV1::proposed(
                request.reservation_id.clone(),
                offer_commitment,
                maker_identity,
                taker_identity,
                request.foreign_units,
                lez_units,
                now_unix_seconds,
                agreement_commitment,
                proposal_wire.clone(),
            )
            .map_err(invalid_request)?;
            let mut store = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
            let commit = store
                .stage_zec_maker_negotiation(
                    &request.request_id,
                    &request.offer_id,
                    request.expected_offer_revision,
                    &negotiation,
                )
                .map_err(application_store_error)?;
            Ok(ZecChatProposalV1 {
                schema_version: 1,
                offer_revision: commit.revision(),
                was_replay: commit.was_replay(),
                reservation_id: request.reservation_id,
                lez_units,
                maker_identity: maker_identity.to_vec(),
                taker_identity: taker_identity.to_vec(),
                agreement_commitment,
                proposal_wire,
            })
        },
    )?;
    register_chat_complete_method(module)?;
    Ok(())
}

fn register_chat_complete_method(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
    module.register_blocking_method::<RpcResult<ZecChatCompleteResponseV1>, _>(
        "zec_chat_complete_v1",
        |params, context, _| {
            let request: ZecChatCompleteRequestV1 = params.one()?;
            complete_zec_chat(&request, &context)
        },
    )?;
    Ok(())
}

fn complete_zec_chat(
    request: &ZecChatCompleteRequestV1,
    context: &MakerRpc,
) -> RpcResult<ZecChatCompleteResponseV1> {
    if request.schema_version != 1 {
        return Err(invalid_request("unsupported ZEC Chat completion"));
    }
    let completion_store = context
        .zec_completion_store
        .as_ref()
        .ok_or_else(|| invalid_request("maker ZEC completion store is unavailable"))?;
    let now_unix_seconds = trusted_now_unix_seconds()?;
    let provisioner = context
        .zec_actor_provisioner
        .as_ref()
        .ok_or_else(|| rpc_error(INTERNAL_ERROR, "maker actor provisioning is unavailable"))?;
    // A take-time reservation's template is made now, before the lookup
    // below, and only for a final agreement that countersigns its draft.
    let materialized = match &context.zec_reservations {
        Some(reservations) => reservations.materialize(
            &request.reservation_id,
            provisioner,
            now_unix_seconds,
            &request.final_agreement_wire,
        )?,
        None => Materialized::NotTakeTime,
    };
    let take_time = materialized == Materialized::Open;
    let completed = complete_materialized_zec_chat(
        request,
        context,
        completion_store,
        provisioner,
        now_unix_seconds,
        take_time,
    );
    // A refusal after the template was made frees what the attempt took: the
    // sidecar is retired and the outpoint is held only inside the grace. A
    // swap completed before this attempt (a refused replay) is left alone.
    if completed.is_err()
        && take_time
        && let Some(reservations) = &context.zec_reservations
    {
        reservations.refuse(&request.reservation_id);
    }
    completed
}

fn complete_materialized_zec_chat(
    request: &ZecChatCompleteRequestV1,
    context: &MakerRpc,
    completion_store: &SqliteZecRecoveryStore,
    provisioner: &ZecMakerActorProvisioner,
    now_unix_seconds: u64,
    // The reservation was open when this attempt began: it records the
    // completion before the commit.
    take_time: bool,
) -> RpcResult<ZecChatCompleteResponseV1> {
    // The Taker holds the swap secret in both directions and reveals it with
    // its claim of the Maker-funded leg; the Maker reads it from that claim.
    // A pinned template that carries a preimage is refused before the replay
    // check, and without the transcript's expiry, so a replay is refused for
    // the same reason after it.
    if provisioner
        .template_carries_claim_preimage(&request.final_agreement_wire)
        .map_err(|_| rpc_error(INTERNAL_ERROR, "maker claim authority is unavailable"))?
    {
        return Err(invalid_request(
            "maker template carries a claim preimage; the Taker holds the swap secret",
        ));
    }
    if let Some(replay) = completion_store
        .preflight_maker_zec_scheduled_completion_replay_for_role(
            &request.request_id,
            &request.offer_id,
            request.expected_offer_revision,
            &request.reservation_id,
            &request.final_agreement_wire,
            None,
        )
        .map_err(application_store_error)?
    {
        if take_time && let Some(reservations) = &context.zec_reservations {
            reservations.mark_completed(&request.reservation_id, &request.final_agreement_wire)?;
        }
        return Ok(ZecChatCompleteResponseV1 {
            schema_version: 1,
            offer_revision: replay.offer_revision(),
            was_replay: true,
            swap_id: replay.swap_id().as_str().into(),
        });
    }
    let accepted = AcceptedZecAgreementV1::accept_wire_at(
        &request.final_agreement_wire,
        lez_swap_core::UnixSeconds::new(now_unix_seconds),
        Participant::Maker,
        0,
    )
    .map_err(invalid_request)?;
    let actor = provisioner
        .provision(&request.final_agreement_wire, now_unix_seconds)
        .map_err(|_| rpc_error(INTERNAL_ERROR, "maker actor provisioning failed"))?;
    let swap_id: Box<str> = accepted.agreement().coordinator().id().as_str().into();
    // Recorded before the commit: a crash between the two holds the outpoint
    // until the refund time, never frees it under a completed swap.
    if take_time && let Some(reservations) = &context.zec_reservations {
        reservations.mark_completed(&request.reservation_id, &request.final_agreement_wire)?;
    }
    let commit = completion_store
        .complete_maker_zec_negotiation_and_register_actor_within_capacity(
            &request.request_id,
            &request.offer_id,
            request.expected_offer_revision,
            &request.reservation_id,
            &accepted,
            &actor,
            now_unix_seconds,
            zec_taker_sells_lez_capacity(accepted.agreement().binding().profile_id()),
        )
        .map_err(application_store_error)?;
    Ok(ZecChatCompleteResponseV1 {
        schema_version: 1,
        offer_revision: commit.offer_revision(),
        was_replay: commit.was_replay(),
        swap_id,
    })
}

/// Validated deployment inputs for daemon-owned ZEC actor creation. The
/// template registry is fixed at start for a provisioned corridor and grows
/// at run time by one template per take-time reservation (`zec_reserve.rs`).
#[derive(Clone)]
pub struct ZecMakerActorProvisioner {
    source_maker_configs: Arc<std::sync::RwLock<Vec<ActorConfig>>>,
    actor_root: PathBuf,
    actor_program: PathBuf,
    actor_program_sha256: [u8; 32],
}

impl std::fmt::Debug for ZecMakerActorProvisioner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ZecMakerActorProvisioner")
            .field(
                "source_maker_config_count",
                &self
                    .source_maker_configs
                    .read()
                    .map_or(0, |sources| sources.len()),
            )
            .field("actor_root", &"[REDACTED]")
            .field("actor_program", &"[REDACTED]")
            .field(
                "actor_program_sha256",
                &hex::encode(self.actor_program_sha256),
            )
            .finish()
    }
}

impl ZecMakerActorProvisioner {
    /// The SHA-256 the ZEC Maker actor program was pinned at when this Node started.
    #[must_use]
    pub const fn actor_program_sha256(&self) -> [u8; 32] {
        self.actor_program_sha256
    }

    /// Validates a bounded per-swap Maker authority registry and exact actor program.
    ///
    /// # Errors
    ///
    /// Rejects an empty/oversized registry, duplicate swap or state identities,
    /// non-Maker templates, and unsafe or mismatched actor executables.
    pub fn new(
        source_maker_configs: &[PathBuf],
        actor_root: PathBuf,
        actor_program: PathBuf,
        actor_program_sha256: [u8; 32],
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !source_maker_configs.is_empty(),
            "ZEC Maker authority registry is empty or oversized"
        );
        Self::with_templates(
            source_maker_configs,
            actor_root,
            actor_program,
            actor_program_sha256,
        )
    }

    /// The same deployment without a static template: every template comes
    /// from a take-time reservation ([`Self::insert`]).
    ///
    /// # Errors
    ///
    /// Rejects an unsafe actor root or a mismatched actor executable.
    pub fn new_dynamic(
        actor_root: PathBuf,
        actor_program: PathBuf,
        actor_program_sha256: [u8; 32],
    ) -> anyhow::Result<Self> {
        Self::with_templates(&[], actor_root, actor_program, actor_program_sha256)
    }

    fn with_templates(
        source_maker_configs: &[PathBuf],
        actor_root: PathBuf,
        actor_program: PathBuf,
        actor_program_sha256: [u8; 32],
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            source_maker_configs.len() <= MAX_ZEC_MAKER_AUTHORITY_TEMPLATES,
            "ZEC Maker authority registry is empty or oversized"
        );
        let mut sources: Vec<ActorConfig> = Vec::with_capacity(source_maker_configs.len());
        for source_maker_config in source_maker_configs {
            let source_parent = source_maker_config
                .parent()
                .context("source maker config has no parent")?;
            let source_parent_metadata = fs::symlink_metadata(source_parent)
                .context("inspect source maker config parent")?;
            anyhow::ensure!(
                source_maker_config.is_absolute()
                    && source_parent_metadata.file_type().is_dir()
                    && source_parent_metadata.uid() == rustix::process::geteuid().as_raw()
                    && source_parent_metadata.mode() & 0o7777 == 0o700
                    && fs::canonicalize(source_parent)
                        .context("canonicalize source maker config parent")?
                        == source_parent,
                "source maker config parent is unsafe"
            );
            let source = ActorConfig::load_private(source_maker_config)
                .map_err(|_| anyhow::anyhow!("source maker config is unavailable"))?;
            anyhow::ensure!(
                source.role() == ActorRole::Maker,
                "source actor config is not Maker"
            );
            anyhow::ensure!(
                sources
                    .iter()
                    .all(|existing| existing.swap_id() != source.swap_id()
                        && existing.role_state_db() != source.role_state_db()),
                "ZEC Maker authority registry contains duplicate swap or state identity"
            );
            source
                .load_activate_material()
                .map_err(|_| anyhow::anyhow!("source maker authority is unavailable"))?;
            sources.push(source);
        }
        let root_metadata =
            fs::symlink_metadata(&actor_root).context("inspect ZEC maker actor root")?;
        anyhow::ensure!(
            actor_root.is_absolute()
                && root_metadata.file_type().is_dir()
                && root_metadata.uid() == rustix::process::geteuid().as_raw()
                && root_metadata.mode() & 0o7777 == 0o700
                && fs::canonicalize(&actor_root).context("canonicalize ZEC maker actor root")?
                    == actor_root,
            "ZEC maker actor root is unsafe"
        );
        validate_maker_actor_program(&actor_program, actor_program_sha256)
            .map_err(|_| anyhow::anyhow!("ZEC actor program is unavailable"))?;
        Ok(Self {
            source_maker_configs: Arc::new(std::sync::RwLock::new(sources)),
            actor_root,
            actor_program,
            actor_program_sha256,
        })
    }

    /// How many pinned Maker templates this catalog holds.
    #[must_use]
    pub fn template_count(&self) -> usize {
        self.source_maker_configs
            .read()
            .map_or(0, |sources| sources.len())
    }

    /// Adds one take-time reservation's template; adding it again is a no-op.
    ///
    /// # Errors
    ///
    /// Rejects a non-Maker or unloadable template, or one whose swap or state
    /// identity another template already holds.
    pub fn insert(&self, source_maker_config: &Path) -> anyhow::Result<()> {
        let source = ActorConfig::load_private(source_maker_config)
            .map_err(|_| anyhow::anyhow!("source maker config is unavailable"))?;
        anyhow::ensure!(
            source.role() == ActorRole::Maker,
            "source actor config is not Maker"
        );
        source
            .load_activate_material()
            .map_err(|_| anyhow::anyhow!("source maker authority is unavailable"))?;
        let mut sources = self
            .source_maker_configs
            .write()
            .map_err(|_| anyhow::anyhow!("ZEC Maker authority registry lock poisoned"))?;
        if sources.iter().any(|existing| existing == &source) {
            return Ok(());
        }
        anyhow::ensure!(
            sources
                .iter()
                .all(|existing| existing.swap_id() != source.swap_id()
                    && existing.role_state_db() != source.role_state_db()),
            "ZEC Maker authority registry contains duplicate swap or state identity"
        );
        sources.push(source);
        Ok(())
    }

    fn template_for(&self, swap_id: &str) -> Option<ActorConfig> {
        self.source_maker_configs
            .read()
            .ok()?
            .iter()
            .find(|source| source.swap_id().as_str() == swap_id)
            .cloned()
    }

    /// Whether the pinned template for this agreement carries a claim
    /// preimage. None may: the Taker holds the swap secret in both directions
    /// (the preparation gives it to the Taker's half only), and the Maker
    /// learns it from the Taker's revealing claim.
    ///
    /// The agreement is decoded and checked here only to find its template,
    /// so the transcript's expiry is not applied (the wire is validated at
    /// time zero, before any transcript expires): an exact replay of a
    /// completion is answered after the transcript expired, and a fresh
    /// completion is refused for expiry by its own acceptance at the current
    /// time.
    ///
    /// # Errors
    ///
    /// Returns the agreement decoding or the material loading failure.
    pub fn template_carries_claim_preimage(
        &self,
        final_agreement_wire: &[u8],
    ) -> anyhow::Result<bool> {
        let accepted = AcceptedZecAgreementV1::accept_wire_at(
            final_agreement_wire,
            lez_swap_core::UnixSeconds::new(0),
            Participant::Maker,
            0,
        )?;
        let Some(source) = self.template_for(accepted.agreement().application_swap_id()) else {
            return Ok(false);
        };
        Ok(source
            .load_activate_material()
            .context("load the template's activation material")?
            .claim_preimage()
            .is_some())
    }

    fn provision(
        &self,
        final_agreement_wire: &[u8],
        accepted_at_unix_seconds: u64,
    ) -> anyhow::Result<MakerActorManifestV1> {
        let accepted_at = lez_swap_core::UnixSeconds::new(accepted_at_unix_seconds);
        let accepted = AcceptedZecAgreementV1::accept_wire_at(
            final_agreement_wire,
            accepted_at,
            Participant::Maker,
            0,
        )?;
        let source = self
            .template_for(accepted.agreement().application_swap_id())
            .context("accepted ZEC swap has no pinned Maker authority template")?;
        let mut digest = Sha256::new();
        digest.update(b"lez-atomic-swaps/maker-actor-root/v1\0");
        digest.update(final_agreement_wire);
        let output_root = self.actor_root.join(hex::encode(digest.finalize()));
        let provisioned = provision_zec_maker_actor_from_config(
            &source,
            final_agreement_wire,
            accepted_at,
            &output_root,
        )?;
        let agreement_sha256: [u8; 32] = Sha256::digest(final_agreement_wire).into();
        anyhow::ensure!(
            provisioned.agreement_sha256() == agreement_sha256,
            "provisioned agreement identity changed"
        );
        MakerActorManifestV1::new(
            provisioned.swap_id().clone(),
            MakerActorKindV1::Zcash,
            provisioned.config_file().to_path_buf(),
            provisioned.config_sha256(),
            self.actor_program.clone(),
            self.actor_program_sha256,
            provisioned.state_database().to_path_buf(),
        )
        .map_err(Into::into)
    }
}

/// Refuses a new `TakerSellsLez` proposal while the Maker already holds as
/// many open ones as [`zec_taker_sells_lez_capacity`] allows for its profile;
/// the completion checks again in the transaction that accepts it.
fn refuse_at_taker_sells_lez_capacity(
    completion_store: &SqliteZecRecoveryStore,
    profile: ZecProfileId,
    now_unix_seconds: u64,
) -> RpcResult<()> {
    let capacity = zec_taker_sells_lez_capacity(profile);
    let open = completion_store
        .count_open_maker_zec_taker_sells_lez_swaps(now_unix_seconds)
        .map_err(application_store_error)?;
    if open >= capacity {
        return Err(application_store_error(
            StoreError::MakerZecTakerSellsLezCapacityReached { open, capacity },
        ));
    }
    Ok(())
}

/// The safety factor on one Maker LEZ claim's slot at the one-claim gate.
const LEZ_CLAIM_SLOT_SAFETY_FACTOR: u64 = 2;

/// The worst LEZ claim submit-to-final measured on the devnet, rounded up:
/// 6.3 s (2026-11-18, 3 LEZ claims read from the Nodes' actor lines, the same
/// claim transaction submitted and projected; 3 LEZ escrow fundings
/// 4.5–5.5 s). The devnet's LEZ blocks are about 1.5 s apart.
const DEVNET_LEZ_CLAIM_WORST_MS: u64 = 7_000;

/// LEZ v0.2 public testnet finality, which a claim waits out before the next
/// one is planned. No LEZ testnet claim latency has been measured; this is
/// the recorded LEZ funding finality: about an hour (docs/api/README.md) and
/// about two hours on 2026-09-19 (docs/evidence/testnet4-20260919). The
/// longer is taken.
const PUBLIC_TESTNET_LEZ_CLAIM_WORST_MS: u64 = 2 * 3_600_000;

/// How long a claim held at the gate can wait after the claim before it is
/// final: the holder's next attempt (requeue delay) that clears its in-flight
/// marker and the held claim's retry (failure backoff). The devnet runs the
/// Maker at 2 s / 10 s (`LEZ_MAKER_ACTOR_REQUEUE_SECONDS`,
/// `LEZ_MAKER_ACTOR_FAILURE_BACKOFF_SECONDS`), every other Node at the
/// defaults, 5 s / 30 s.
const DEVNET_HELD_CLAIM_RETRY_MS: u64 = 12_000;
const DEFAULT_HELD_CLAIM_RETRY_MS: u64 = 35_000;

/// How many open ZEC `TakerSellsLez` swaps a Maker holds at once under
/// `profile` (FINDING 3). The Taker chooses when to reveal, up to the claim
/// window's close at `T_lez - M`; the Maker's follow-up LEZ claims go one at
/// a time (one LEZ account nonce), earliest refund first. `N` swaps revealed
/// together leave the last claim `N` slots after the reveal, so
/// `N * slot <= M` with
///
/// `slot = SAFETY (2) * (worst LEZ claim submit-to-final + held-claim retry)`
///
/// and `capacity = floor(M / slot)`:
///
/// | profile | M | worst claim | retry | slot | capacity |
/// |---|---|---|---|---|---|
/// | devnet v7, v8 | 120 s | 7 s (measured) | 12 s | 38 s | 3 |
/// | deterministic-local v3 (test lane) | 30 s | 7 s (devnet bound) | 35 s | 84 s | 0 → 1 |
/// | public-testnet v3 | 7 200 s | 7 200 s (LEZ funding finality; no claim measured) | 35 s | 14 470 s | 0 → 1 |
///
/// A profile whose margin does not hold one slot keeps a capacity of one:
/// the cap bounds how many claims queue behind each other, and the single
/// claim's own fit in `M` is the profile's timing check (ADR 0216, I3),
/// which public-testnet v3 does not meet at the recorded LEZ finality.
///
/// The count and the gate agree on which swaps compete: a swap past its LEZ
/// refund time is not counted, and its claim, waiting or in flight, holds no
/// other (it can no longer be included; its actor waits for the Taker's LEZ
/// refund, then recovers its Zcash or stops by name, `lez_claim_cannot_land`). A claim whose attempt failed before submission
/// does not hold the others either, so a failing swap delays the queue by
/// at most its own attempt, not until its refund time.
#[must_use]
pub fn zec_taker_sells_lez_capacity(profile: ZecProfileId) -> usize {
    let (claim_worst_ms, held_retry_ms) = match profile {
        ZecProfileId::DeterministicLocalDevnetV1
        | ZecProfileId::DeterministicLocalDevnetV2
        | ZecProfileId::DeterministicLocalDevnetV3
        | ZecProfileId::DeterministicLocalDevnetV4
        | ZecProfileId::DeterministicLocalDevnetV5
        | ZecProfileId::DeterministicLocalDevnetV6
        | ZecProfileId::DeterministicLocalDevnetV7
        | ZecProfileId::DeterministicLocalDevnetV8 => {
            (DEVNET_LEZ_CLAIM_WORST_MS, DEVNET_HELD_CLAIM_RETRY_MS)
        }
        ZecProfileId::PublicTestnetV1
        | ZecProfileId::PublicTestnetV2
        | ZecProfileId::PublicTestnetV3 => (
            PUBLIC_TESTNET_LEZ_CLAIM_WORST_MS,
            DEFAULT_HELD_CLAIM_RETRY_MS,
        ),
        ZecProfileId::DeterministicLocalV1
        | ZecProfileId::DeterministicLocalV2
        | ZecProfileId::DeterministicLocalV3 => {
            (DEVNET_LEZ_CLAIM_WORST_MS, DEFAULT_HELD_CLAIM_RETRY_MS)
        }
    };
    let slot_ms = LEZ_CLAIM_SLOT_SAFETY_FACTOR * (claim_worst_ms + held_retry_ms);
    let margin_ms = ZecRefundProfile::for_id(profile)
        .required_margin()
        .value()
        .saturating_mul(1_000);
    usize::try_from(margin_ms / slot_ms)
        .unwrap_or(usize::MAX)
        .max(1)
}

fn validate_zec_chat_shape(request: &ZecChatProposeRequestV1) -> RpcResult<()> {
    if request.schema_version != 1 || request.foreign_units == 0 {
        return Err(invalid_request("unsupported or empty ZEC Chat proposal"));
    }
    Ok(())
}

fn zec_draft_matches_offer(
    validated: &ValidatedZecAgreementDraftV1,
    authenticated: &AuthenticatedOfferRefV1,
    offer: &MakerOfferV1,
    maker_key: &PublicKey,
    reservation_id: &RequestId,
    foreign_units: u64,
    lez_units: u128,
) -> bool {
    let transcript = validated.body().transcript();
    validated.maker_zcash_key() == maker_key
        && authenticated.maker_identity() == &maker_key.serialize()
        && offer.route().pair() == Pair::Zcash
        && validated.body().direction() == offer.route().direction()
        && validated.zcash_amount_zatoshis() == foreign_units
        && validated.body().lez_terms().amount() == lez_units
        && transcript.session_id() == &maker_zec_chat_session_id(reservation_id)
        && transcript.offer_commitment() == &authenticated.commitment()
        && transcript.expires_at_unix_seconds() == offer.expires_at_unix_seconds()
}

#[cfg(test)]
mod capacity_tests {
    use super::*;

    #[test]
    fn the_taker_sells_lez_capacity_fits_one_claim_slot_per_swap_in_the_margin() {
        assert_eq!(
            zec_taker_sells_lez_capacity(ZecProfileId::DeterministicLocalDevnetV7),
            3
        );
        assert_eq!(
            zec_taker_sells_lez_capacity(ZecProfileId::DeterministicLocalDevnetV8),
            3
        );
        assert_eq!(
            zec_taker_sells_lez_capacity(ZecProfileId::DeterministicLocalV3),
            1
        );
        assert_eq!(
            zec_taker_sells_lez_capacity(ZecProfileId::PublicTestnetV3),
            1
        );
        // The devnet's worst case: three claims revealed together settle in
        // the 120 s margin.
        let slot_ms =
            LEZ_CLAIM_SLOT_SAFETY_FACTOR * (DEVNET_LEZ_CLAIM_WORST_MS + DEVNET_HELD_CLAIM_RETRY_MS);
        assert!(3 * slot_ms <= 120_000);
    }

    #[test]
    fn the_capacity_refusal_reaches_the_taker_by_code_and_name() {
        let error = application_store_error(StoreError::MakerZecTakerSellsLezCapacityReached {
            open: 3,
            capacity: 3,
        });
        assert_eq!(error.code(), ZEC_TAKER_SELLS_LEZ_CAPACITY_REACHED);
        assert!(
            error
                .message()
                .starts_with("zec_taker_sells_lez_capacity_reached"),
            "{}",
            error.message()
        );
    }
}
