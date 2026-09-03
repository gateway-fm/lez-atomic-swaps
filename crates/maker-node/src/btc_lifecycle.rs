//! Maker side of the Node-owned Bitcoin lifecycle (ADR 0213, S1.3).
//!
//! Every method answers one Taker-initiated round. State lives in the swap
//! directory of the reservation (role root, journals, prepared LEZ material,
//! funding plan, actor bundle); each round records its request digest and
//! response so a replayed request returns the same answer and a different
//! request for the same round is refused.

use std::{fs, path::Path, sync::Arc};

use anyhow::{Context as _, ensure};
use btc_role_preflight::{
    RoleBootstrapInput, bootstrap_role_in_process, persist_and_bind_countersigned_agreement,
};
use lez_bridge_client::validate_prepared_witnessed_claim;
use lez_bridge_protocol::PrepareWitnessedClaimResult;
use lez_btc_role_lifecycle::{
    BitcoinWallet, BtcRoleRuntime, FundingPlan, LegSessions, LezSidecar, MakerCeremony,
    PreparedEscrow, SwapLayout, SwapSidecar,
    actor::{ActorSynthesis, MakerLockMaterial, activate, synthesize},
    layout::{read_private, write_private_exact},
    lez::{
        PlanningTermsInput, agreement_terms, planning_escrow_funding_placeholder, planning_terms,
    },
    swap_run_id,
    wire::{
        BtcCeremonyNonceRequestV1, BtcCeremonyNonceResponseV1, BtcCeremonyPartialRequestV1,
        BtcCeremonyPartialResponseV1, BtcCeremonyReserveRequestV1, BtcCeremonyReserveResponseV1,
        BtcFundingFactsV1, BtcReserveRequestV1, BtcReserveResponseV1, BtcSwapPlanV1,
    },
};
use lez_btc_swap_sdk::{
    CsvBlockDelay, MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES, P2trSwapOutput, RefundXOnlyKey,
    TwoPartyAggregateKey,
};
use zeroize::Zeroizing;

use super::*;

const MAX_ROUND_RECORD_BYTES: usize = 256 * 1024;
const MAX_PREPARED_CLAIM_BYTES: usize = 64 * 1024;
/// How far a proposed recovery timestamp may sit from this Node's own policy.
const PLAN_EARLY_SLACK_SECONDS: u64 = 120;
const PLAN_LATE_SLACK_SECONDS: u64 = 900;

/// The Maker's Bitcoin lifecycle authority: configuration plus one lock that
/// serializes every round across reservations.
pub struct BtcMakerLifecycle {
    runtime: BtcRoleRuntime,
    rounds: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for BtcMakerLifecycle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BtcMakerLifecycle")
            .field("swaps_root", &self.runtime.config().swaps_root)
            .finish_non_exhaustive()
    }
}

impl BtcMakerLifecycle {
    /// Loads the Maker role configuration.
    ///
    /// # Errors
    ///
    /// Fails when the configuration file is invalid.
    pub fn load(config_file: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            runtime: BtcRoleRuntime::load(Participant::Maker, config_file)?,
            rounds: tokio::sync::Mutex::new(()),
        })
    }

    /// Keeps every activated swap's sidecar running (respawning after a Node
    /// restart) so the supervised actors' LEZ observations have a peer. Runs
    /// on the current async runtime when one is available.
    pub fn spawn_sidecar_keepalive(self: &Arc<Self>) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            eprintln!(
                "maker BTC lifecycle: no async runtime; swap sidecars respawn only on demand"
            );
            return;
        };
        let lifecycle = Arc::clone(self);
        handle.spawn(async move {
            loop {
                let runtime_root = lifecycle.runtime.config().swaps_root.clone();
                let lifecycle_for_pass = Arc::clone(&lifecycle);
                let _ = tokio::task::spawn_blocking(move || {
                    for swap in lez_btc_role_lifecycle::sidecar::recorded_swaps(&runtime_root)
                        .unwrap_or_default()
                    {
                        if !swap.join("round-ceremony-partial.json").is_file() {
                            continue;
                        }
                        let Some(name) = swap.file_name().and_then(|name| name.to_str()) else {
                            continue;
                        };
                        let Ok(reservation_id) = RequestId::new(name.to_owned()) else {
                            continue;
                        };
                        let layout = lifecycle_for_pass.layout(&reservation_id);
                        if let Err(error) = SwapSidecar::ensure(
                            &lifecycle_for_pass.runtime,
                            &layout,
                            &reservation_id,
                        ) {
                            eprintln!(
                                "maker BTC lifecycle: sidecar for {name} unavailable: {error:#}"
                            );
                        }
                    }
                })
                .await;
                tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            }
        });
    }

    pub(super) fn layout(&self, reservation_id: &RequestId) -> SwapLayout {
        SwapLayout::new(&self.runtime.config().swaps_root, reservation_id)
    }

    /// The reservation's Maker contribution, if the reservation exists.
    pub(super) fn contribution_wire(&self, reservation_id: &RequestId) -> Option<Vec<u8>> {
        let layout = self.layout(reservation_id);
        layout.exists().then(|| {
            read_private(
                &layout.role_root().contribution_file(),
                MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES,
            )
            .ok()
        })?
    }

    /// Binds the countersigned agreement into the reservation's role root.
    pub(super) fn bind(
        &self,
        reservation_id: &RequestId,
        taker_contribution_wire: &[u8],
        final_agreement_wire: &[u8],
        accepted_at_unix_seconds: u64,
    ) -> anyhow::Result<u64> {
        let layout = self.layout(reservation_id);
        ensure!(layout.exists(), "unknown BTC reservation");
        let binding = persist_and_bind_countersigned_agreement(
            layout.role_root().root(),
            taker_contribution_wire,
            final_agreement_wire,
            accepted_at_unix_seconds,
        )?;
        ensure!(
            !binding.ready_for_public_effects(),
            "agreement binding unexpectedly authorized public effects"
        );
        Ok(binding.accepted_at_unix_seconds())
    }

    /// Checks a proposed draft against what this Maker planned for the
    /// reservation: its own funding outpoint, the LEZ accounts, the claim hash
    /// it prepared, and the recovery plan.
    pub(super) fn validate_draft(
        &self,
        reservation_id: &RequestId,
        draft: &BtcAgreementDraftV1,
    ) -> anyhow::Result<()> {
        let layout = self.layout(reservation_id);
        let record = ReservationRecordV1::load(&layout)?;
        let body = draft.body();
        ensure!(
            body.swap_id() == &record.swap_id,
            "draft swap id differs from the reservation"
        );
        let (metadata, custody) = lez_btc_role_lifecycle::lez::escrow_accounts(
            self.runtime.lez_identity().escrow_program_id(),
            &record.swap_id,
        );
        let lez = body.lez_terms();
        ensure!(
            lez.metadata_account() == &metadata && lez.custody_account() == &custody,
            "draft LEZ escrow accounts differ from the runtime derivation"
        );
        ensure!(
            lez.aggregate_authority_account()
                == &lez_btc_role_lifecycle::lez::aggregate_authority_account(
                    &record.aggregate_x_only
                ),
            "draft LEZ aggregate authority differs"
        );
        ensure!(
            lez.amount() == record.plan.lez_units,
            "draft LEZ amount differs from the plan"
        );
        ensure!(
            lez.refund_at_ms() == record.plan.lez_refund_at_ms,
            "draft LEZ refund time differs from the plan"
        );
        if let Some(hash) = record.maker_claim_message_hash {
            ensure!(
                lez.claim_message_hash() == &hash,
                "draft claim hash differs from the Maker's prepared claim"
            );
        }
        let recovery = body.recovery_plan();
        ensure!(
            recovery.maker_second_lock_cutoff_unix_seconds()
                == record.plan.maker_second_lock_cutoff_unix_seconds
                && recovery.earlier_refund_latest_unix_seconds()
                    == record.plan.earlier_refund_latest_unix_seconds
                && recovery.later_refund_earliest_unix_seconds()
                    == record.plan.later_refund_earliest_unix_seconds
                && recovery.required_margin_seconds() == record.plan.required_margin_seconds,
            "draft recovery plan differs from the reserved plan"
        );
        if let Some(plan) = &record.funding_plan {
            let funding = body.funding_terms();
            ensure!(
                funding.transaction_id() == &plan.transaction_id
                    && funding.output_index() == plan.output_index
                    && funding.value_sat() == plan.value_sat
                    && recovery.bitcoin_funding_anchor_height() == plan.anchor_height,
                "draft funding terms differ from the Maker's funding plan"
            );
        }
        let participants = body.participants();
        let funder = match body.direction() {
            SwapDirection::TakerSellsForeign => Participant::Taker,
            SwapDirection::TakerSellsLez => Participant::Maker,
        };
        let contract = P2trSwapOutput::new(
            TwoPartyAggregateKey::from_bytes(participants.aggregate_internal_key()?)?,
            RefundXOnlyKey::from_bytes(*participants.for_participant(funder).bitcoin_refund_key())?,
            CsvBlockDelay::new(self.runtime.config().bitcoin.refund_csv_blocks)?,
        )?;
        ensure!(
            body.p2tr_terms() == &lez_btc_swap_sdk::BtcP2trTermsV1::from_contract(&contract),
            "draft contract differs from the policy contract"
        );
        Ok(())
    }

    fn agreement_key(context: &MakerRpc) -> RpcResult<Zeroizing<[u8; 32]>> {
        let key = context
            .btc_chat_signing_key
            .as_ref()
            .ok_or_else(|| invalid_request("maker BTC Chat signer is unavailable"))?;
        Ok(Zeroizing::new(key.secret_bytes()))
    }

    /// The swap's own sidecar (spawned on first use, respawned when gone).
    fn sidecar(
        &self,
        layout: &SwapLayout,
        reservation_id: &RequestId,
    ) -> RpcResult<(SwapSidecar, LezSidecar)> {
        let sidecar = SwapSidecar::ensure(&self.runtime, layout, reservation_id)
            .map_err(|error| internal(&error))?;
        let client = sidecar
            .client(&self.runtime)
            .map_err(|error| internal(&error))?;
        Ok((sidecar, client))
    }

    fn wallet(&self) -> RpcResult<BitcoinWallet> {
        let bitcoin = &self.runtime.config().bitcoin;
        BitcoinWallet::connect(
            &bitcoin.endpoint,
            &bitcoin.cookie_file,
            bitcoin.wallet.as_deref(),
            bitcoin.network.network(),
            self.runtime.request_timeout(),
        )
        .map_err(|error| internal(&error))
    }
}

/// Everything the Maker decided at reservation time, replayed by later rounds.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReservationRecordV1 {
    schema_version: u16,
    offer_id: Box<str>,
    direction: SwapDirection,
    swap_id: [u8; 32],
    aggregate_x_only: [u8; 32],
    pre_session_id: [u8; 32],
    taker_contribution_sha256: [u8; 32],
    plan: BtcSwapPlanV1,
    funding_plan: Option<FundingPlan>,
    /// The claim hash the Maker prepared (Maker is LEZ claimant).
    maker_claim_message_hash: Option<[u8; 32]>,
    sessions: Option<LegSessions>,
    actor_activated: bool,
}

impl ReservationRecordV1 {
    fn file(layout: &SwapLayout) -> std::path::PathBuf {
        layout.root().join("reservation.json")
    }

    fn load(layout: &SwapLayout) -> anyhow::Result<Self> {
        let bytes = read_private(&Self::file(layout), MAX_ROUND_RECORD_BYTES)?;
        serde_json::from_slice(&bytes).context("reservation record")
    }

    fn store(&self, layout: &SwapLayout) -> anyhow::Result<()> {
        let path = Self::file(layout);
        let bytes = serde_json::to_vec_pretty(self)?;
        if path.exists() {
            fs::write(&path, &bytes)?;
        } else {
            write_private_exact(&path, &bytes)?;
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
struct RoundRecord {
    request_sha256: [u8; 32],
    response: serde_json::Value,
}

fn request_digest<T: Serialize>(request: &T) -> RpcResult<[u8; 32]> {
    let bytes = serde_json::to_vec(request).map_err(|error| internal(&error))?;
    Ok(Sha256::digest(bytes).into())
}

/// Returns the stored response when this exact request already succeeded,
/// fails on a different request for the same round, else `None`.
fn replay<T: Serialize + for<'de> Deserialize<'de>>(
    layout: &SwapLayout,
    round: &str,
    digest: [u8; 32],
) -> RpcResult<Option<T>> {
    let path = layout.root().join(format!("round-{round}.json"));
    if !path.exists() {
        return Ok(None);
    }
    let record: RoundRecord = serde_json::from_slice(
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
    let record = RoundRecord {
        request_sha256: digest,
        response: serde_json::to_value(response).map_err(|error| internal(&error))?,
    };
    write_private_exact(
        &layout.root().join(format!("round-{round}.json")),
        &serde_json::to_vec(&record).map_err(|error| internal(&error))?,
    )
    .map(|_| ())
    .map_err(|error| internal(&error))
}

fn internal(error: &dyn std::fmt::Display) -> ErrorObjectOwned {
    eprintln!("maker BTC lifecycle: {error}");
    rpc_error(INTERNAL_ERROR, "maker BTC lifecycle step failed")
}

fn plan_is_acceptable(
    plan: &BtcSwapPlanV1,
    runtime: &BtcRoleRuntime,
    reservation_id: &RequestId,
    now: u64,
    lez_units: u128,
) -> Result<(), &'static str> {
    let config = runtime.config();
    if plan.lez_units != lez_units {
        return Err("plan LEZ amount differs from the offer quote");
    }
    if plan.refund_csv_blocks != config.bitcoin.refund_csv_blocks {
        return Err("plan CSV differs from policy");
    }
    if plan.claim_fee_sat != config.bitcoin.claim_fee_sat {
        return Err("plan claim fee differs from policy");
    }
    if swap_run_id(reservation_id).is_ok_and(|expected| plan.bridge_run_id != expected.as_str())
        || swap_run_id(reservation_id).is_err()
    {
        return Err("plan bridge run id is not the swap's derived run id");
    }
    let within = |value: u64, offset: u64| {
        let expected = now.saturating_add(offset);
        value.saturating_add(PLAN_EARLY_SLACK_SECONDS) >= expected
            && value <= expected.saturating_add(PLAN_LATE_SLACK_SECONDS)
    };
    let policy = config.recovery;
    if !within(
        plan.maker_second_lock_cutoff_unix_seconds,
        policy.maker_second_lock_cutoff_seconds,
    ) || !within(
        plan.earlier_refund_latest_unix_seconds,
        policy.earlier_refund_latest_seconds,
    ) || !within(
        plan.later_refund_earliest_unix_seconds,
        policy.later_refund_earliest_seconds,
    ) || plan.required_margin_seconds != policy.required_margin_seconds
    {
        return Err("plan recovery schedule is outside this Node's policy");
    }
    if !plan.lez_refund_at_ms.is_multiple_of(1000) {
        return Err("plan LEZ refund time must be whole seconds");
    }
    Ok(())
}

/// The LEZ refund time the direction selects (ADR 0044 ordering).
const fn lez_refund_seconds(direction: SwapDirection, plan: &BtcSwapPlanV1) -> u64 {
    match direction {
        SwapDirection::TakerSellsForeign => plan.earlier_refund_latest_unix_seconds,
        SwapDirection::TakerSellsLez => plan.later_refund_earliest_unix_seconds,
    }
}

#[allow(clippy::too_many_lines)]
pub(super) async fn reserve(
    request: BtcReserveRequestV1,
    context: Arc<MakerRpc>,
) -> RpcResult<BtcReserveResponseV1> {
    if request.schema_version != 1
        || request.taker_contribution_wire.is_empty()
        || request.taker_contribution_wire.len() > MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES
    {
        return Err(invalid_request("unsupported or empty BTC reservation"));
    }
    let lifecycle = context
        .btc_lifecycle
        .as_ref()
        .ok_or_else(|| invalid_request("maker BTC lifecycle is unavailable"))?;
    let _serial = lifecycle.rounds.lock().await;
    let layout = lifecycle.layout(&request.reservation_id);
    let digest = request_digest(&request)?;
    if layout.exists() {
        if let Some(response) = replay::<BtcReserveResponseV1>(&layout, "reserve", digest)? {
            return Ok(BtcReserveResponseV1 {
                was_replay: true,
                ..response
            });
        }
        return Err(rpc_error(CONFLICT, "BTC reservation already exists"));
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
    if offer.id().as_str() != &*request.offer_id
        || offer.route().pair() != Pair::Bitcoin
        || offer.route().direction() != request.direction
        || offer.created_at_unix_seconds() > now
        || now >= offer.expires_at_unix_seconds()
        || request.plan.foreign_units < offer.minimum_foreign_units()
        || request.plan.foreign_units > offer.maximum_foreign_units()
    {
        return Err(invalid_request(
            "Delivery offer does not match the BTC reservation",
        ));
    }
    let lez_units = offer
        .quote_foreign_amount(request.plan.foreign_units)
        .map_err(invalid_request)?;
    plan_is_acceptable(
        &request.plan,
        &lifecycle.runtime,
        &request.reservation_id,
        now,
        lez_units,
    )
    .map_err(invalid_request)?;
    let lez_refund_seconds = lez_refund_seconds(request.direction, &request.plan);
    if request.plan.lez_refund_at_ms != lez_refund_seconds.saturating_mul(1000) {
        return Err(invalid_request(
            "plan LEZ refund time does not follow the direction",
        ));
    }
    let taker = BtcRoleContributionV1::from_wire(&request.taker_contribution_wire)
        .map_err(invalid_request)?;
    let expected_pre_session = derive_btc_pre_session_id_v1(
        &authenticated.commitment(),
        request.reservation_id.as_str().as_bytes(),
        request.direction,
    )
    .map_err(invalid_request)?;
    if taker.body().role() != Participant::Taker
        || taker.body().pre_session_id() != &expected_pre_session
        || taker.body().expires_at_unix_seconds() != offer.expires_at_unix_seconds()
        || taker.body().bitcoin_chain_policy() != lifecycle.runtime.bitcoin_policy()
        || taker.body().lez_chain_identity() != lifecycle.runtime.lez_identity()
    {
        return Err(invalid_request(
            "Taker contribution is not bound to this offer and chain",
        ));
    }

    layout.create().map_err(|error| internal(&error))?;
    write_private_exact(
        &layout.root().join("taker-contribution.borsh"),
        &request.taker_contribution_wire,
    )
    .map_err(|error| internal(&error))?;
    let maker_key = BtcMakerLifecycle::agreement_key(&context)?;
    let bootstrapped = bootstrap_role_in_process(
        &RoleBootstrapInput {
            role: Participant::Maker,
            direction: request.direction,
            offer_commitment: authenticated.commitment(),
            reservation_binding: request.reservation_id.as_str().as_bytes().to_vec(),
            bitcoin: *lifecycle.runtime.bitcoin_policy(),
            lez: *lifecycle.runtime.lez_identity(),
            lez_owner_account: lifecycle.runtime.lez_owner_account(),
            expires_at_unix_seconds: offer.expires_at_unix_seconds(),
        },
        Some(maker_key),
        layout.role_root().root(),
    )
    .map_err(|error| internal(&error))?;
    let maker = BtcRoleContributionV1::from_wire(&bootstrapped.contribution_wire)
        .map_err(|error| internal(&error))?;
    let pair = BtcRoleContributionPairV1::new(maker, taker).map_err(invalid_request)?;
    let participants = pair.participants();
    let aggregate = participants
        .aggregate_internal_key()
        .map_err(invalid_request)?;
    let swap_id = *pair.swap_id();

    // Facts only the Maker holds. The LEZ escrow itself waits for the bound
    // agreement: the sidecar plans it under the final terms hash and holds one
    // active escrow, so nothing is prepared against planning terms.
    let mut funding_plan = None;
    let mut maker_claim_message_hash = None;
    match request.direction {
        SwapDirection::TakerSellsLez => {
            let contract = P2trSwapOutput::new(
                TwoPartyAggregateKey::from_bytes(aggregate).map_err(invalid_request)?,
                RefundXOnlyKey::from_bytes(
                    *participants
                        .for_participant(Participant::Maker)
                        .bitcoin_refund_key(),
                )
                .map_err(invalid_request)?,
                CsvBlockDelay::new(request.plan.refund_csv_blocks).map_err(invalid_request)?,
            )
            .map_err(invalid_request)?;
            let wallet = lifecycle.wallet()?;
            let plan = wallet
                .plan_funding(contract.script_pubkey_bytes(), request.plan.foreign_units)
                .await
                .map_err(|error| internal(&error))?;
            wallet
                .test_mempool_accept(&plan.transaction_hex)
                .await
                .map_err(|error| internal(&error))?;
            write_private_exact(
                &layout.funding_plan_file(),
                &serde_json::to_vec_pretty(&plan).map_err(|error| internal(&error))?,
            )
            .map_err(|error| internal(&error))?;
            funding_plan = Some(plan);
            // The Maker claims LEZ: prepare the claim now; the draft binds its hash.
            let terms = planning_terms(&PlanningTermsInput {
                swap_id,
                terms_hash: expected_pre_session,
                depositor: Participant::Taker,
                depositor_account: *participants
                    .for_participant(Participant::Taker)
                    .lez_owner_account(),
                claimant: Participant::Maker,
                claimant_account: lifecycle.runtime.lez_owner_account(),
                aggregate_x_only: aggregate,
                amount: lez_units,
                refund_at_ms: request.plan.lez_refund_at_ms,
                authenticated_transfer_program_id: *lifecycle
                    .runtime
                    .lez_identity()
                    .authenticated_transfer_program_id(),
            })
            .map_err(|error| internal(&error))?;
            let (_, sidecar) = lifecycle.sidecar(&layout, &request.reservation_id)?;
            let prepared = sidecar
                .prepare_claim(terms, planning_escrow_funding_placeholder(&swap_id))
                .await
                .map_err(|error| internal(&error))?;
            write_private_exact(
                &layout.prepared_claim_file(),
                &serde_json::to_vec(&prepared).map_err(|error| internal(&error))?,
            )
            .map_err(|error| internal(&error))?;
            maker_claim_message_hash = Some(*prepared.claim.message_hash.as_bytes());
        }
        SwapDirection::TakerSellsForeign => {}
    }
    let record = ReservationRecordV1 {
        schema_version: 1,
        offer_id: request.offer_id.clone(),
        direction: request.direction,
        swap_id,
        aggregate_x_only: aggregate,
        pre_session_id: expected_pre_session,
        taker_contribution_sha256: Sha256::digest(&request.taker_contribution_wire).into(),
        plan: request.plan.clone(),
        funding_plan: funding_plan.clone(),
        maker_claim_message_hash,
        sessions: None,
        actor_activated: false,
    };
    record.store(&layout).map_err(|error| internal(&error))?;
    let response = BtcReserveResponseV1 {
        schema_version: 1,
        was_replay: false,
        maker_contribution_wire: bootstrapped.contribution_wire,
        maker_bitcoin_funding: funding_plan.map(|plan| BtcFundingFactsV1 {
            transaction_id: plan.transaction_id,
            output_index: plan.output_index,
            value_sat: plan.value_sat,
            anchor_height: plan.anchor_height,
        }),
        maker_claim_message_hash,
        swap_id,
    };
    record_round(&layout, "reserve", digest, &response)?;
    Ok(response)
}

fn load_bound_agreement(layout: &SwapLayout) -> RpcResult<(BtcAgreementV1, Vec<u8>)> {
    let wire = read_private(
        &layout.role_root().agreement_file(),
        MAX_BTC_AGREEMENT_RECORD_BYTES,
    )
    .map_err(|_| invalid_request("the reservation has no bound agreement yet"))?;
    let agreement = BtcAgreementV1::from_wire(&wire).map_err(|error| internal(&error))?;
    Ok((agreement, wire))
}

#[allow(clippy::too_many_lines)]
pub(super) async fn ceremony_reserve(
    request: BtcCeremonyReserveRequestV1,
    context: Arc<MakerRpc>,
) -> RpcResult<BtcCeremonyReserveResponseV1> {
    if request.schema_version != 1 {
        return Err(invalid_request("unsupported BTC ceremony reservation"));
    }
    let lifecycle = context
        .btc_lifecycle
        .as_ref()
        .ok_or_else(|| invalid_request("maker BTC lifecycle is unavailable"))?;
    let _serial = lifecycle.rounds.lock().await;
    let layout = lifecycle.layout(&request.reservation_id);
    if !layout.exists() {
        return Err(invalid_request("unknown BTC reservation"));
    }
    let digest = request_digest(&request)?;
    if let Some(response) =
        replay::<BtcCeremonyReserveResponseV1>(&layout, "ceremony-reserve", digest)?
    {
        return Ok(BtcCeremonyReserveResponseV1 {
            was_replay: true,
            ..response
        });
    }
    let mut record = ReservationRecordV1::load(&layout).map_err(|error| internal(&error))?;
    let (agreement, _) = load_bound_agreement(&layout)?;
    let sessions = LegSessions {
        bitcoin: request.bitcoin_session_id,
        lez: request.lez_session_id,
    };
    if record.sessions.is_some_and(|existing| existing != sessions) {
        return Err(rpc_error(
            CONFLICT,
            "the ceremony already runs under other session ids",
        ));
    }
    let final_terms = agreement_terms(&agreement).map_err(|error| internal(&error))?;
    let (_, sidecar) = lifecycle.sidecar(&layout, &request.reservation_id)?;
    let mut maker_prepared_claim = None;
    match agreement.lez_claimant() {
        Participant::Taker => {
            let bytes = request.prepared_claim_result.as_deref().ok_or_else(|| {
                invalid_request("the Taker must send its prepared witnessed claim")
            })?;
            if bytes.len() > MAX_PREPARED_CLAIM_BYTES {
                return Err(invalid_request("prepared claim exceeds its bound"));
            }
            let prepared: PrepareWitnessedClaimResult = serde_json::from_slice(bytes)
                .map_err(|_| invalid_request("prepared claim is not valid JSON"))?;
            if swap_run_id(&request.reservation_id)
                .is_ok_and(|expected| prepared.context.run_id != expected)
                || prepared.context.sidecar_role
                    != lez_btc_role_lifecycle::config::bridge_participant(Participant::Taker)
                || prepared.context.request_id != prepared.claim.preparation_request_id
                || validate_prepared_witnessed_claim(&prepared.claim).is_err()
                || prepared.claim.message_hash.as_bytes()
                    != agreement.lez_terms().claim_message_hash()
            {
                return Err(invalid_request(
                    "prepared claim does not match the agreement",
                ));
            }
            write_private_exact(&layout.prepared_claim_file(), bytes)
                .map_err(|error| internal(&error))?;
        }
        Participant::Maker => {
            // Prepared at reservation; its hash is what the agreement binds.
            let bytes = read_private(&layout.prepared_claim_file(), MAX_PREPARED_CLAIM_BYTES)
                .map_err(|error| internal(&error))?;
            let prepared: PrepareWitnessedClaimResult =
                serde_json::from_slice(&bytes).map_err(|error| internal(&error))?;
            if prepared.claim.message_hash.as_bytes() != agreement.lez_terms().claim_message_hash()
            {
                return Err(internal(&"prepared claim hash differs from the agreement"));
            }
            maker_prepared_claim = Some(bytes);
        }
    }
    if agreement.lez_depositor() == Participant::Maker && !layout.escrow_result_file().exists() {
        // The Maker's lock material: the final escrow under the agreement.
        let escrow = sidecar
            .prepare_escrow(final_terms)
            .await
            .map_err(|error| internal(&error))?;
        store_escrow(&layout, &escrow)?;
    }
    let maker_key = BtcMakerLifecycle::agreement_key(&context)?;
    let mut ceremony =
        MakerCeremony::open(&layout, &agreement, &sessions).map_err(|error| internal(&error))?;
    let maker_commitments = ceremony
        .reserve_round(&request.taker_commitments, &maker_key)
        .map_err(|error| invalid_request(format!("ceremony reservation failed: {error}")))?;
    record.sessions = Some(sessions);
    record.store(&layout).map_err(|error| internal(&error))?;
    let response = BtcCeremonyReserveResponseV1 {
        schema_version: 1,
        was_replay: false,
        maker_commitments,
        prepared_claim_result: maker_prepared_claim,
    };
    record_round(&layout, "ceremony-reserve", digest, &response)?;
    Ok(response)
}

fn store_escrow(layout: &SwapLayout, escrow: &PreparedEscrow) -> RpcResult<()> {
    let mut request = serde_json::to_vec(&escrow.request).map_err(|error| internal(&error))?;
    request.push(b'\n');
    let mut result = serde_json::to_vec(&escrow.result).map_err(|error| internal(&error))?;
    result.push(b'\n');
    write_private_exact(&layout.escrow_request_file(), &request)
        .map_err(|error| internal(&error))?;
    write_private_exact(&layout.escrow_result_file(), &result).map_err(|error| internal(&error))?;
    Ok(())
}

fn load_escrow(layout: &SwapLayout) -> RpcResult<PreparedEscrow> {
    let request = read_private(&layout.escrow_request_file(), MAX_ROUND_RECORD_BYTES)
        .map_err(|error| internal(&error))?;
    let result = read_private(&layout.escrow_result_file(), MAX_ROUND_RECORD_BYTES)
        .map_err(|error| internal(&error))?;
    Ok(PreparedEscrow {
        request: serde_json::from_slice(&request).map_err(|error| internal(&error))?,
        result: serde_json::from_slice(&result).map_err(|error| internal(&error))?,
    })
}

pub(super) async fn ceremony_nonce(
    request: BtcCeremonyNonceRequestV1,
    context: Arc<MakerRpc>,
) -> RpcResult<BtcCeremonyNonceResponseV1> {
    if request.schema_version != 1 {
        return Err(invalid_request("unsupported BTC ceremony nonce round"));
    }
    let lifecycle = context
        .btc_lifecycle
        .as_ref()
        .ok_or_else(|| invalid_request("maker BTC lifecycle is unavailable"))?;
    let _serial = lifecycle.rounds.lock().await;
    let layout = lifecycle.layout(&request.reservation_id);
    if !layout.exists() {
        return Err(invalid_request("unknown BTC reservation"));
    }
    let digest = request_digest(&request)?;
    if let Some(response) = replay::<BtcCeremonyNonceResponseV1>(&layout, "ceremony-nonce", digest)?
    {
        return Ok(BtcCeremonyNonceResponseV1 {
            was_replay: true,
            ..response
        });
    }
    let record = ReservationRecordV1::load(&layout).map_err(|error| internal(&error))?;
    let sessions = record
        .sessions
        .ok_or_else(|| invalid_request("the ceremony was not reserved"))?;
    let (agreement, _) = load_bound_agreement(&layout)?;
    let maker_key = BtcMakerLifecycle::agreement_key(&context)?;
    let mut ceremony =
        MakerCeremony::open(&layout, &agreement, &sessions).map_err(|error| internal(&error))?;
    let (maker_nonces, maker_partials) = ceremony
        .nonce_round(&request.taker_nonces, &maker_key)
        .map_err(|error| invalid_request(format!("ceremony nonce round failed: {error}")))?;
    let response = BtcCeremonyNonceResponseV1 {
        schema_version: 1,
        was_replay: false,
        maker_nonces,
        maker_partials,
    };
    record_round(&layout, "ceremony-nonce", digest, &response)?;
    Ok(response)
}

#[allow(clippy::too_many_lines)]
pub(super) async fn ceremony_partial(
    request: BtcCeremonyPartialRequestV1,
    context: Arc<MakerRpc>,
) -> RpcResult<BtcCeremonyPartialResponseV1> {
    if request.schema_version != 1 {
        return Err(invalid_request("unsupported BTC ceremony partial round"));
    }
    let lifecycle = context
        .btc_lifecycle
        .as_ref()
        .ok_or_else(|| invalid_request("maker BTC lifecycle is unavailable"))?;
    let _serial = lifecycle.rounds.lock().await;
    let layout = lifecycle.layout(&request.reservation_id);
    if !layout.exists() {
        return Err(invalid_request("unknown BTC reservation"));
    }
    let digest = request_digest(&request)?;
    if let Some(response) =
        replay::<BtcCeremonyPartialResponseV1>(&layout, "ceremony-partial", digest)?
    {
        return Ok(BtcCeremonyPartialResponseV1 {
            was_replay: true,
            ..response
        });
    }
    let mut record = ReservationRecordV1::load(&layout).map_err(|error| internal(&error))?;
    let sessions = record
        .sessions
        .ok_or_else(|| invalid_request("the ceremony was not reserved"))?;
    let (agreement, agreement_wire) = load_bound_agreement(&layout)?;
    let mut ceremony =
        MakerCeremony::open(&layout, &agreement, &sessions).map_err(|error| internal(&error))?;
    let presignatures = ceremony
        .partial_round(&request.taker_partials)
        .map_err(|error| invalid_request(format!("ceremony partial round failed: {error}")))?;

    // Both legs are presigned: synthesize and activate the Maker actor.
    let (swap_sidecar, sidecar) = lifecycle.sidecar(&layout, &request.reservation_id)?;
    let start_height = sidecar
        .finalized_height()
        .await
        .map_err(|error| internal(&error))?;
    let prepared_claim = read_private(&layout.prepared_claim_file(), MAX_PREPARED_CLAIM_BYTES)
        .map_err(|error| internal(&error))?;
    let funding_hex;
    let escrow;
    let maker_lock = match agreement.direction() {
        SwapDirection::TakerSellsLez => {
            let plan = record
                .funding_plan
                .as_ref()
                .ok_or_else(|| internal(&"missing Maker funding plan"))?;
            funding_hex = plan.transaction_hex.clone();
            MakerLockMaterial::Bitcoin {
                funding_transaction_hex: &funding_hex,
            }
        }
        SwapDirection::TakerSellsForeign => {
            escrow = load_escrow(&layout)?;
            MakerLockMaterial::Lez(&escrow)
        }
    };
    let accepted_at = trusted_now_unix_seconds()?;
    let config = synthesize(&ActorSynthesis {
        runtime: &lifecycle.runtime,
        layout: &layout,
        agreement: &agreement,
        agreement_wire: &agreement_wire,
        sidecar: &swap_sidecar,
        sessions,
        accepted_at_unix_seconds: accepted_at,
        lez_discovery_start_height: start_height,
        prepared_claim_json: &prepared_claim,
        maker_lock: Some(maker_lock),
    })
    .map_err(|error| internal(&error))?;
    activate(&config).await.map_err(|error| internal(&error))?;
    // Hand the activated actor to the Maker's supervisor, which observes it
    // and drives every next effect (LEZ funding, Bitcoin claim) on schedule.
    let program = &lifecycle.runtime.config().actor;
    let mut program_sha256 = [0_u8; 32];
    hex::decode_to_slice(&program.program_sha256, &mut program_sha256)
        .map_err(|_| internal(&"actor program digest is not 32 bytes of hex"))?;
    let config_sha256 = lez_btc_role_lifecycle::actor::file_sha256(&layout.actor_config_file())
        .map_err(|error| internal(&error))?;
    let manifest = MakerActorManifestV1::new(
        agreement.coordinator().id().clone(),
        MakerActorKindV1::Bitcoin,
        layout.actor_config_file(),
        config_sha256,
        program.program.clone(),
        program_sha256,
        layout.actor_state_db(),
    )
    .map_err(|error| internal(&error))?;
    {
        let mut store = context
            .store
            .lock()
            .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
        store
            .register_maker_actor(&manifest, accepted_at)
            .map_err(|error| internal(&error))?;
    }
    record.actor_activated = true;
    record.store(&layout).map_err(|error| internal(&error))?;
    let response = BtcCeremonyPartialResponseV1 {
        schema_version: 1,
        was_replay: false,
        presignatures,
        maker_actor_activated: true,
    };
    record_round(&layout, "ceremony-partial", digest, &response)?;
    Ok(response)
}

pub(super) fn register_btc_lifecycle_methods(
    module: &mut RpcModule<MakerRpc>,
) -> anyhow::Result<()> {
    module.register_async_method("btc_reserve_v1", |params, context, _| async move {
        let request: BtcReserveRequestV1 = params.one()?;
        reserve(request, context).await
    })?;
    module.register_async_method("btc_ceremony_reserve_v1", |params, context, _| async move {
        let request: BtcCeremonyReserveRequestV1 = params.one()?;
        ceremony_reserve(request, context).await
    })?;
    module.register_async_method("btc_ceremony_nonce_v1", |params, context, _| async move {
        let request: BtcCeremonyNonceRequestV1 = params.one()?;
        ceremony_nonce(request, context).await
    })?;
    module.register_async_method("btc_ceremony_partial_v1", |params, context, _| async move {
        let request: BtcCeremonyPartialRequestV1 = params.one()?;
        ceremony_partial(request, context).await
    })?;
    Ok(())
}
