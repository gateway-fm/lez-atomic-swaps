//! Node-owned Bitcoin take (ADR 0213, S1.3): the Taker prepares any live
//! Bitcoin offer at take time instead of reading a fixture catalog.
//!
//! The take runs as one `taker_swap_initiate_v1` call: reservation with the
//! Maker, funding plan from the Taker's wallet (when it funds Bitcoin), LEZ
//! claim or escrow preparation through the Taker's sidecar, draft, proposal,
//! countersignature, role binding, the three ceremony rounds, actor synthesis
//! and activation. Every step is idempotent on the swap directory, so a
//! replayed initiation resumes where it stopped. The swap is then a regular
//! prepared entry: monitoring, claim and refund need nothing new.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail, ensure};
use btc_role_preflight::{
    AgreementDraftFacts, RoleBootstrapInput, RoleSecret, bootstrap_role_in_process,
    compose_agreement_draft_wire, persist_and_bind_countersigned_agreement,
};
use lez_bridge_protocol::RequestId;
use lez_btc_role_lifecycle::{
    BitcoinWallet, BtcRoleRuntime, FundingPlan, LegSessions, LezSidecar, SwapLayout, SwapSidecar,
    TakerCeremony,
    actor::{ActorSynthesis, activate, synthesize},
    layout::{read_private, write_private_exact},
    lez::{
        PlanningTermsInput, aggregate_authority_account, agreement_terms, escrow_accounts,
        planning_escrow_funding_placeholder, planning_terms,
    },
    swap_run_id,
    wire::{
        BtcCeremonyNonceRequestV1, BtcCeremonyNonceResponseV1, BtcCeremonyPartialRequestV1,
        BtcCeremonyPartialResponseV1, BtcCeremonyReserveRequestV1, BtcCeremonyReserveResponseV1,
        BtcReserveRequestV1, BtcReserveResponseV1, BtcSwapPlanV1,
    },
};
use lez_btc_swap_sdk::{
    BtcAgreementDraftV1, BtcAgreementV1, BtcMakerAgreementProposalV1, BtcRoleContributionPairV1,
    BtcRoleContributionV1, CsvBlockDelay, MAX_BTC_AGREEMENT_RECORD_BYTES,
    MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES, P2trSwapOutput, RefundXOnlyKey, TwoPartyAggregateKey,
};
use lez_node_common::{
    AuthenticatedOfferRefV1, BtcChatCompleteRequestV2, BtcChatCompleteResponseV2,
    BtcChatProposalV2, BtcChatProposeRequestV2, DeliveryOfferQueryV1, RunLocalDelivery,
    local_rpc::call_local_chat_rpc,
};
use lez_swap_core::{Pair, Participant, SwapDirection, SwapId};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::MakerOfferId;
use secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{
    acceptance_files::derived_request_id,
    taker_service_config::prepared::{
        ImmutablePrivateFileV1, PreparedConfigurationV1, SecretPrivateFileV1,
    },
};

const MAX_RECORD_BYTES: usize = 256 * 1024;
const MAX_PREPARED_CLAIM_BYTES: usize = 64 * 1024;

/// The Taker's Node-owned Bitcoin lifecycle authority.
pub struct DynamicBtcRole {
    runtime: BtcRoleRuntime,
    config_file: PathBuf,
    chat_socket: PathBuf,
    /// Delivery sources by id: directory and Maker identity, to authenticate
    /// offers and to name the source a prepared entry binds to.
    sources: Vec<(Box<str>, PathBuf, [u8; 33])>,
}

impl std::fmt::Debug for DynamicBtcRole {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DynamicBtcRole")
            .field("swaps_root", &self.runtime.config().swaps_root)
            .field("sources", &self.sources.len())
            .finish_non_exhaustive()
    }
}

impl DynamicBtcRole {
    /// Loads the Taker's Bitcoin role configuration.
    ///
    /// # Errors
    ///
    /// Fails when the configuration is invalid or no Delivery source has an id.
    pub fn load(
        config_file: &Path,
        chat_socket: &Path,
        source_bindings: &BTreeMap<Box<str>, (usize, [u8; 33])>,
        delivery_sources: &[RunLocalDelivery],
    ) -> Result<Self> {
        let runtime = BtcRoleRuntime::load(Participant::Taker, config_file)?;
        let mut sources = Vec::new();
        for (source_id, (index, maker)) in source_bindings {
            let delivery = delivery_sources
                .get(*index)
                .context("delivery source index")?;
            sources.push((
                source_id.clone(),
                delivery.directory().to_path_buf(),
                *maker,
            ));
        }
        ensure!(
            !sources.is_empty(),
            "the Node-owned Bitcoin take needs a named Delivery source"
        );
        Ok(Self {
            runtime,
            config_file: config_file.to_path_buf(),
            chat_socket: chat_socket.to_path_buf(),
            sources,
        })
    }

    /// Every swap this Node prepared earlier, as catalog entries.
    #[must_use]
    pub(crate) fn persisted_entries(&self) -> Vec<PreparedConfigurationV1> {
        let Ok(entries) = fs::read_dir(&self.runtime.config().swaps_root) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path().join("taker-prepared.json");
            let Ok(bytes) = read_private(&path, MAX_RECORD_BYTES) else {
                continue;
            };
            if let Ok(configured) = serde_json::from_slice::<PreparedConfigurationV1>(&bytes) {
                found.push(configured);
            }
        }
        found
    }

    fn layout(&self, reservation_id: &RequestId) -> SwapLayout {
        SwapLayout::new(&self.runtime.config().swaps_root, reservation_id)
    }

    /// Turns a persisted entry into a catalog entry, re-authenticating the
    /// offer against its Delivery source like the static catalog does.
    pub(crate) fn load_entry(
        &self,
        configured: &PreparedConfigurationV1,
    ) -> Result<crate::PreparedTakerInitiationV1> {
        let mut bindings = BTreeMap::new();
        let mut subscribers = Vec::new();
        for (index, (source_id, directory, maker)) in self.sources.iter().enumerate() {
            bindings.insert(source_id.clone(), (index, *maker));
            let key = PublicKey::from_slice(maker).context("Maker identity")?;
            subscribers.push(RunLocalDelivery::subscriber(directory.clone(), key)?);
        }
        crate::taker_service_config::prepared::load_prepared_entry(
            Pair::Bitcoin,
            configured,
            &bindings,
            &subscribers,
            Some(&self.chat_socket),
            true,
        )
        .map_err(|error| anyhow::anyhow!("prepared entry: {error}"))
    }

    fn source_for(&self, maker_identity: &[u8; 33]) -> Result<(&str, &Path)> {
        self.sources
            .iter()
            .find(|(_, _, maker)| maker == maker_identity)
            .map(|(id, directory, _)| (id.as_ref(), directory.as_path()))
            .context("no Delivery source names this Maker")
    }

    /// Authenticates the offer from the request's announcement or Delivery.
    async fn authenticate_offer(
        &self,
        offer_id: &MakerOfferId,
        route: lez_swap_store::MakerRouteV1,
        maker_identity: &[u8; 33],
        now: u64,
        announcement: Option<AuthenticatedOfferRefV1>,
    ) -> Result<AuthenticatedOfferRefV1> {
        if let Some(offer) = announcement {
            ensure!(
                offer.maker_identity() == maker_identity,
                "announcement Maker differs"
            );
            return Ok(offer);
        }
        let (_, directory) = self.source_for(maker_identity)?;
        let maker = PublicKey::from_slice(maker_identity).context("Maker identity")?;
        let delivery = RunLocalDelivery::subscriber(directory.to_path_buf(), maker)?;
        delivery
            .discover(&DeliveryOfferQueryV1::for_route(route, now))
            .await?
            .into_iter()
            .find(|candidate| candidate.offer().id() == offer_id)
            .context("selected BTC offer is unavailable, expired, or not authentic")
    }

    fn wallet(&self) -> Result<BitcoinWallet> {
        let bitcoin = &self.runtime.config().bitcoin;
        BitcoinWallet::connect(
            &bitcoin.endpoint,
            &bitcoin.cookie_file,
            bitcoin.wallet.as_deref(),
            bitcoin.network.network(),
            self.runtime.request_timeout(),
        )
    }

    /// The swap's own sidecar (spawned on first use, respawned when gone).
    fn sidecar(
        &self,
        layout: &SwapLayout,
        reservation_id: &RequestId,
    ) -> Result<(SwapSidecar, LezSidecar)> {
        let sidecar = SwapSidecar::ensure(&self.runtime, layout, reservation_id)?;
        let client = sidecar.client(&self.runtime)?;
        Ok((sidecar, client))
    }
}

/// What one take needs from the initiation request.
#[derive(Clone, Debug)]
pub(super) struct TakeRequest {
    pub request_id: RequestId,
    pub offer_id: MakerOfferId,
    pub route: lez_swap_store::MakerRouteV1,
    pub maker_identity: [u8; 33],
    pub signed_envelope_sha256: [u8; 32],
    pub foreign_units: u64,
    pub expected_lez_units: u128,
    pub announcement: Option<AuthenticatedOfferRefV1>,
}

/// Durable progress of one take; every step checks it before acting.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
struct TakerSwapRecordV1 {
    schema_version: u16,
    direction: Option<SwapDirection>,
    swap_id: Option<[u8; 32]>,
    aggregate_x_only: Option<[u8; 32]>,
    plan: Option<BtcSwapPlanV1>,
    maker_bitcoin_funding: Option<lez_btc_role_lifecycle::wire::BtcFundingFactsV1>,
    funding_plan: Option<FundingPlan>,
    claim_message_hash: Option<[u8; 32]>,
    sessions: Option<LegSessions>,
    actor_activated: bool,
    funding_broadcast_transaction_id: Option<String>,
}

impl TakerSwapRecordV1 {
    fn file(layout: &SwapLayout) -> PathBuf {
        layout.root().join("taker-swap.json")
    }

    fn load(layout: &SwapLayout) -> Result<Self> {
        match read_private(&Self::file(layout), MAX_RECORD_BYTES) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("taker swap record"),
            Err(_) => Ok(Self {
                schema_version: 1,
                ..Self::default()
            }),
        }
    }

    fn store(&self, layout: &SwapLayout) -> Result<()> {
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

/// The prepared catalog entry the take produced (or resumed).
pub(super) struct DynamicTake {
    pub configured: PreparedConfigurationV1,
}

/// The reservation id every Chat round of this take is keyed on.
pub(super) fn reservation_id_for(request_id: &RequestId) -> Result<RequestId> {
    derived_request_id(request_id, "btc", b"node-reservation")
}

fn now_plan(
    runtime: &BtcRoleRuntime,
    reservation_id: &RequestId,
    direction: SwapDirection,
    now: u64,
    foreign_units: u64,
    lez_units: u128,
) -> Result<BtcSwapPlanV1> {
    let policy = runtime.config().recovery;
    let earlier = now + policy.earlier_refund_latest_seconds;
    let later = now + policy.later_refund_earliest_seconds;
    let refund_seconds = match direction {
        SwapDirection::TakerSellsForeign => earlier,
        SwapDirection::TakerSellsLez => later,
    };
    Ok(BtcSwapPlanV1 {
        foreign_units,
        lez_units,
        refund_csv_blocks: runtime.config().bitcoin.refund_csv_blocks,
        claim_fee_sat: runtime.config().bitcoin.claim_fee_sat,
        lez_refund_at_ms: refund_seconds * 1000,
        maker_second_lock_cutoff_unix_seconds: now + policy.maker_second_lock_cutoff_seconds,
        earlier_refund_latest_unix_seconds: earlier,
        later_refund_earliest_unix_seconds: later,
        required_margin_seconds: policy.required_margin_seconds,
        bridge_run_id: swap_run_id(reservation_id)?.as_str().to_owned(),
        taker_bitcoin_funding: None,
    })
}

/// Reserves with the Maker, plans, composes the draft and persists the swap
/// as a prepared entry. Idempotent: an existing swap directory is resumed.
#[allow(clippy::too_many_lines)]
pub(super) async fn prepare(
    dynamic: &DynamicBtcRole,
    request: &TakeRequest,
    now: u64,
) -> Result<DynamicTake> {
    let reservation_id = reservation_id_for(&request.request_id)?;
    let layout = dynamic.layout(&reservation_id);
    let direction = request.route.direction();
    ensure!(request.route.pair() == Pair::Bitcoin, "not a Bitcoin route");
    let prepared_file = layout.root().join("taker-prepared.json");
    // A replay of a take that already configured its swap is answered from the
    // bound record: the Maker withdraws a reserved lot from Delivery, so the
    // offer need not be discoverable any more.
    if layout.exists()
        && let Ok(bytes) = read_private(&prepared_file, MAX_RECORD_BYTES)
    {
        let configured: PreparedConfigurationV1 = serde_json::from_slice(&bytes)?;
        return Ok(DynamicTake { configured });
    }
    let offer = dynamic
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
            && offer.offer().quote_foreign_amount(request.foreign_units)?
                == request.expected_lez_units
            && offer.offer().created_at_unix_seconds() <= now
            && now < offer.offer().expires_at_unix_seconds(),
        "the selected offer is not the live offer the request names"
    );
    let (source_id, _) = dynamic.source_for(&request.maker_identity)?;
    if !layout.exists() {
        layout.create()?;
    }
    let mut record = TakerSwapRecordV1::load(&layout)?;
    write_private_exact(
        &layout.root().join("signed-envelope.bin"),
        offer.signed_envelope(),
    )?;

    // Role root (fresh keys) and the plan.
    let role_root = layout.role_root();
    if !role_root.contribution_file().exists() {
        bootstrap_role_in_process(
            &RoleBootstrapInput {
                role: Participant::Taker,
                direction,
                offer_commitment: offer.commitment(),
                reservation_binding: reservation_id.as_str().as_bytes().to_vec(),
                bitcoin: *dynamic.runtime.bitcoin_policy(),
                lez: *dynamic.runtime.lez_identity(),
                lez_owner_account: dynamic.runtime.lez_owner_account(),
                expires_at_unix_seconds: offer.offer().expires_at_unix_seconds(),
            },
            None,
            role_root.root(),
        )?;
    }
    let taker_wire = read_private(
        &role_root.contribution_file(),
        MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES,
    )?;
    let plan = if let Some(plan) = record.plan.clone() {
        plan
    } else {
        let plan = now_plan(
            &dynamic.runtime,
            &reservation_id,
            direction,
            now,
            request.foreign_units,
            request.expected_lez_units,
        )?;
        record.plan = Some(plan.clone());
        record.direction = Some(direction);
        record.store(&layout)?;
        plan
    };

    // Round 1: reservation.
    let reserve: BtcReserveResponseV1 = call_local_chat_rpc(
        &dynamic.chat_socket,
        "btc_reserve_v1",
        &BtcReserveRequestV1 {
            schema_version: 1,
            request_id: derived_request_id(&reservation_id, "btc", b"reserve")?,
            offer_id: request.offer_id.as_str().into(),
            expected_offer_revision: 1,
            reservation_id: reservation_id.clone(),
            direction,
            signed_offer_envelope: offer.signed_envelope().to_vec(),
            taker_contribution_wire: taker_wire.clone(),
            plan: plan.clone(),
        },
    )
    .await
    .context("btc_reserve_v1")?;
    ensure!(
        reserve.schema_version == 1,
        "unsupported reservation answer"
    );
    let maker = BtcRoleContributionV1::from_wire(&reserve.maker_contribution_wire)?;
    let taker = BtcRoleContributionV1::from_wire(&taker_wire)?;
    let pair = BtcRoleContributionPairV1::new(maker, taker)?;
    ensure!(
        *pair.swap_id() == reserve.swap_id,
        "the Maker answered with another swap id"
    );
    write_private_exact(
        &layout.root().join("maker-contribution.borsh"),
        &reserve.maker_contribution_wire,
    )?;
    let participants = pair.participants();
    let aggregate = participants
        .aggregate_internal_key()
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    record.swap_id = Some(reserve.swap_id);
    record.aggregate_x_only = Some(aggregate);
    record.maker_bitcoin_funding = reserve.maker_bitcoin_funding;
    record.store(&layout)?;

    // Facts for the draft.
    let (metadata, custody) = escrow_accounts(
        dynamic.runtime.lez_identity().escrow_program_id(),
        &reserve.swap_id,
    );
    let (funding_txid, funding_vout, funding_value, anchor_height) = match direction {
        SwapDirection::TakerSellsForeign => {
            let plan_record = if let Some(existing) = record.funding_plan.clone() {
                existing
            } else {
                let contract = P2trSwapOutput::new(
                    TwoPartyAggregateKey::from_bytes(aggregate)?,
                    RefundXOnlyKey::from_bytes(
                        *participants
                            .for_participant(Participant::Taker)
                            .bitcoin_refund_key(),
                    )?,
                    CsvBlockDelay::new(plan.refund_csv_blocks)?,
                )?;
                let wallet = dynamic.wallet()?;
                let funding = wallet
                    .plan_funding(contract.script_pubkey_bytes(), request.foreign_units)
                    .await?;
                wallet.test_mempool_accept(&funding.transaction_hex).await?;
                write_private_exact(
                    &layout.funding_plan_file(),
                    &serde_json::to_vec_pretty(&funding)?,
                )?;
                write_private_exact(
                    &layout.funding_transaction_file(),
                    format!("{}\n", funding.transaction_hex).as_bytes(),
                )?;
                record.funding_plan = Some(funding.clone());
                record.store(&layout)?;
                funding
            };
            (
                plan_record.transaction_id,
                plan_record.output_index,
                plan_record.value_sat,
                plan_record.anchor_height,
            )
        }
        SwapDirection::TakerSellsLez => {
            let facts = reserve
                .maker_bitcoin_funding
                .context("the Maker funds Bitcoin but sent no funding facts")?;
            (
                facts.transaction_id,
                facts.output_index,
                facts.value_sat,
                facts.anchor_height,
            )
        }
    };
    ensure!(
        funding_value == request.foreign_units,
        "funding value differs from the principal"
    );
    let claim_hash = if let Some(hash) = record.claim_message_hash {
        hash
    } else {
        let hash = match direction {
            SwapDirection::TakerSellsForeign => {
                // The Taker claims LEZ: its sidecar prepares the claim once; the
                // result is the actor's prepared claim and the draft binds its hash.
                let terms = planning_terms(&PlanningTermsInput {
                    swap_id: reserve.swap_id,
                    terms_hash: *pair.maker().body().pre_session_id(),
                    depositor: Participant::Maker,
                    depositor_account: *participants
                        .for_participant(Participant::Maker)
                        .lez_owner_account(),
                    claimant: Participant::Taker,
                    claimant_account: dynamic.runtime.lez_owner_account(),
                    aggregate_x_only: aggregate,
                    amount: request.expected_lez_units,
                    refund_at_ms: plan.lez_refund_at_ms,
                    authenticated_transfer_program_id: *dynamic
                        .runtime
                        .lez_identity()
                        .authenticated_transfer_program_id(),
                })?;
                let (_, sidecar) = dynamic.sidecar(&layout, &reservation_id)?;
                let prepared = sidecar
                    .prepare_claim(terms, planning_escrow_funding_placeholder(&reserve.swap_id))
                    .await?;
                write_private_exact(
                    &layout.prepared_claim_file(),
                    &serde_json::to_vec(&prepared)?,
                )?;
                *prepared.claim.message_hash.as_bytes()
            }
            SwapDirection::TakerSellsLez => reserve
                .maker_claim_message_hash
                .context("the Maker claims LEZ but sent no claim hash")?,
        };
        record.claim_message_hash = Some(hash);
        record.store(&layout)?;
        hash
    };
    let composed = compose_agreement_draft_wire(
        &AgreementDraftFacts {
            funding_transaction_id: funding_txid,
            funding_output_index: funding_vout,
            funding_value_sat: funding_value,
            claim_value_sat: funding_value
                .checked_sub(plan.claim_fee_sat)
                .context("claim fee exceeds the principal")?,
            refund_csv_blocks: plan.refund_csv_blocks,
            lez_aggregate_authority_account: aggregate_authority_account(&aggregate),
            lez_metadata_account: metadata,
            lez_custody_account: custody,
            lez_amount: request.expected_lez_units,
            lez_refund_at_ms: plan.lez_refund_at_ms,
            lez_prepared_claim_message_hash: claim_hash,
            planned_bitcoin_funding_anchor_height: anchor_height,
            bitcoin_refund_height: anchor_height
                .checked_add(plan.refund_csv_blocks)
                .context("refund height overflow")?,
            maker_second_lock_cutoff_unix_seconds: plan.maker_second_lock_cutoff_unix_seconds,
            earlier_refund_latest_unix_seconds: plan.earlier_refund_latest_unix_seconds,
            later_refund_earliest_unix_seconds: plan.later_refund_earliest_unix_seconds,
            required_margin_seconds: plan.required_margin_seconds,
        },
        &reserve.maker_contribution_wire,
        &taker_wire,
    )?;
    let draft_file = layout.root().join("unsigned-draft.borsh");
    write_private_exact(&draft_file, &composed.wire)?;
    let signed_envelope_file = layout.root().join("signed-envelope.bin");
    let configured = PreparedConfigurationV1 {
        source_id: source_id.into(),
        swap_id: SwapId::new(hex::encode(composed.swap_id))
            .map_err(|error| anyhow::anyhow!("{error:?}"))?,
        offer_id: request.offer_id.clone(),
        reservation_id: reservation_id.clone(),
        foreign_units: request.foreign_units,
        lez_units: request.expected_lez_units,
        signed_envelope: ImmutablePrivateFileV1 {
            path: signed_envelope_file,
            sha256: offer.commitment(),
        },
        unsigned_draft: ImmutablePrivateFileV1 {
            path: draft_file,
            sha256: Sha256::digest(&composed.wire).into(),
        },
        signing_key: SecretPrivateFileV1 {
            path: role_root.secret_file(RoleSecret::Agreement),
        },
        source_config: ImmutablePrivateFileV1 {
            path: dynamic.config_file.clone(),
            sha256: Sha256::digest(read_private(&dynamic.config_file, MAX_RECORD_BYTES)?).into(),
        },
        agreement_output: role_root.agreement_file(),
        actor_root: layout.actor_root(),
        receipt_output: layout.receipt_file(),
    };
    write_private_exact(&prepared_file, &serde_json::to_vec_pretty(&configured)?)?;
    Ok(DynamicTake { configured })
}

/// Runs the take to activation: proposal, countersignature, binding, the
/// ceremony, actor synthesis, activation and the acceptance receipt.
#[allow(clippy::too_many_lines)]
pub(super) async fn execute(
    dynamic: &DynamicBtcRole,
    reservation_id: &RequestId,
    offer: &AuthenticatedOfferRefV1,
    foreign_units: u64,
    now: u64,
) -> Result<()> {
    let layout = dynamic.layout(reservation_id);
    ensure!(layout.exists(), "unknown swap");
    let mut record = TakerSwapRecordV1::load(&layout)?;
    let role_root = layout.role_root();
    let taker_wire = read_private(
        &role_root.contribution_file(),
        MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES,
    )?;
    let maker_wire = read_private(
        &layout.root().join("maker-contribution.borsh"),
        MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES,
    )?;
    let draft_wire = read_private(
        &layout.root().join("unsigned-draft.borsh"),
        MAX_BTC_AGREEMENT_RECORD_BYTES,
    )?;
    let draft = BtcAgreementDraftV1::from_wire(&draft_wire)?;
    let contributions = BtcRoleContributionPairV1::new(
        BtcRoleContributionV1::from_wire(&maker_wire)?,
        BtcRoleContributionV1::from_wire(&taker_wire)?,
    )?;
    let taker_key = role_root.read_secret(RoleSecret::Agreement)?;
    let taker_secret = SecretKey::from_slice(taker_key.as_ref())?;
    let taker_public = PublicKey::from_secret_key(&Secp256k1::signing_only(), &taker_secret);

    // Agreement: propose, countersign, complete, bind (each replay-safe).
    let agreement_wire = if role_root.agreement_file().exists() {
        read_private(&role_root.agreement_file(), MAX_BTC_AGREEMENT_RECORD_BYTES)?
    } else {
        let proposal: BtcChatProposalV2 = call_local_chat_rpc(
            &dynamic.chat_socket,
            "btc_chat_propose_v2",
            &BtcChatProposeRequestV2 {
                schema_version: 2,
                request_id: derived_request_id(reservation_id, "btc", b"propose-v2")?,
                offer_id: offer.offer().id().clone(),
                expected_offer_revision: 1,
                reservation_id: reservation_id.clone(),
                foreign_units,
                signed_offer_envelope: offer.signed_envelope().to_vec(),
                maker_contribution_wire: maker_wire.clone(),
                taker_contribution_wire: taker_wire.clone(),
                unsigned_draft_wire: draft_wire.clone(),
            },
        )
        .await
        .context("btc_chat_propose_v2")?;
        ensure!(
            proposal.schema_version == 2
                && proposal.offer_revision == 2
                && proposal.reservation_id == *reservation_id
                && proposal.joint_swap_id == *contributions.swap_id()
                && proposal.taker_identity.as_slice() == taker_public.serialize(),
            "Maker proposal changed the transcript"
        );
        let maker_proposal = BtcMakerAgreementProposalV1::from_wire(&proposal.proposal_wire)?;
        ensure!(
            maker_proposal.body() == draft.body()
                && proposal.agreement_commitment == maker_proposal.commitment(),
            "Maker proposal changed the executable draft"
        );
        let secp = Secp256k1::signing_only();
        let mut aux = [0_u8; 32];
        getrandom::fill(&mut aux).map_err(|_| anyhow::anyhow!("OS randomness unavailable"))?;
        let signature = secp
            .sign_schnorr_with_aux_rand(
                &Message::from_digest(proposal.agreement_commitment),
                &Keypair::from_secret_key(&secp, &taker_secret),
                &aux,
            )
            .serialize();
        let agreement = maker_proposal.complete(signature)?;
        let wire = agreement.encode_wire()?;
        let binding =
            persist_and_bind_countersigned_agreement(role_root.root(), &maker_wire, &wire, now)?;
        ensure!(
            !binding.ready_for_public_effects(),
            "binding authorized effects early"
        );
        let completion: BtcChatCompleteResponseV2 = call_local_chat_rpc(
            &dynamic.chat_socket,
            "btc_chat_complete_v2",
            &BtcChatCompleteRequestV2 {
                schema_version: 2,
                request_id: derived_request_id(reservation_id, "btc", b"complete-v2")?,
                offer_id: offer.offer().id().clone(),
                expected_offer_revision: 2,
                reservation_id: reservation_id.clone(),
                final_agreement_wire: wire.clone(),
            },
        )
        .await
        .context("btc_chat_complete_v2")?;
        ensure!(
            completion.schema_version == 2
                && completion.offer_revision == 3
                && completion.swap_id.as_ref() == agreement.coordinator().id().as_str()
                && completion.maker_role_bound,
            "Maker completion does not match the countersigned agreement"
        );
        wire
    };
    let agreement = BtcAgreementV1::from_wire(&agreement_wire)?;

    // Final LEZ material under the agreement.
    let (swap_sidecar, sidecar) = dynamic.sidecar(&layout, reservation_id)?;
    let final_terms = agreement_terms(&agreement)?;
    let prepared_claim_json = if agreement.lez_claimant() == Participant::Taker {
        Some(read_private(
            &layout.prepared_claim_file(),
            MAX_PREPARED_CLAIM_BYTES,
        )?)
    } else {
        None
    };
    if agreement.lez_depositor() == Participant::Taker && !layout.escrow_result_file().exists() {
        let escrow = sidecar.prepare_escrow(final_terms).await?;
        let mut request = serde_json::to_vec(&escrow.request)?;
        request.push(b'\n');
        let mut result = serde_json::to_vec(&escrow.result)?;
        result.push(b'\n');
        write_private_exact(&layout.escrow_request_file(), &request)?;
        write_private_exact(&layout.escrow_result_file(), &result)?;
    }

    // Ceremony: three rounds, both legs.
    let sessions = if let Some(sessions) = record.sessions {
        sessions
    } else {
        let sessions = LegSessions::fresh()?;
        record.sessions = Some(sessions);
        record.store(&layout)?;
        sessions
    };
    let mut ceremony = TakerCeremony::open(&layout, &agreement, &sessions)?;
    let taker_commitments = ceremony.commitments(&taker_key)?;
    let reserve: BtcCeremonyReserveResponseV1 = call_local_chat_rpc(
        &dynamic.chat_socket,
        "btc_ceremony_reserve_v1",
        &BtcCeremonyReserveRequestV1 {
            schema_version: 1,
            request_id: derived_request_id(reservation_id, "btc", b"ceremony-reserve")?,
            reservation_id: reservation_id.clone(),
            bitcoin_session_id: sessions.bitcoin,
            lez_session_id: sessions.lez,
            prepared_claim_result: if agreement.lez_claimant() == Participant::Taker {
                prepared_claim_json.clone()
            } else {
                None
            },
            taker_commitments,
        },
    )
    .await
    .context("btc_ceremony_reserve_v1")?;
    let prepared_claim_json = match (prepared_claim_json, reserve.prepared_claim_result) {
        (Some(bytes), _) => bytes,
        (None, Some(bytes)) => {
            let prepared: lez_bridge_protocol::PrepareWitnessedClaimResult =
                serde_json::from_slice(&bytes)?;
            ensure!(
                prepared.context.run_id == *swap_sidecar.run_id()
                    && prepared.context.request_id == prepared.claim.preparation_request_id
                    && lez_bridge_client::validate_prepared_witnessed_claim(&prepared.claim)
                        .is_ok()
                    && prepared.claim.message_hash.as_bytes()
                        == agreement.lez_terms().claim_message_hash(),
                "the Maker's prepared claim does not match the agreement"
            );
            write_private_exact(&layout.prepared_claim_file(), &bytes)?;
            bytes
        }
        (None, None) => bail!("no prepared witnessed claim for this swap"),
    };
    ceremony.accept_maker_commitments(&reserve.maker_commitments)?;
    let taker_nonces = ceremony.nonces()?;
    let nonce: BtcCeremonyNonceResponseV1 = call_local_chat_rpc(
        &dynamic.chat_socket,
        "btc_ceremony_nonce_v1",
        &BtcCeremonyNonceRequestV1 {
            schema_version: 1,
            request_id: derived_request_id(reservation_id, "btc", b"ceremony-nonce")?,
            reservation_id: reservation_id.clone(),
            taker_nonces,
        },
    )
    .await
    .context("btc_ceremony_nonce_v1")?;
    let taker_partials = ceremony.sign(&nonce.maker_nonces, &taker_key)?;
    let partial: BtcCeremonyPartialResponseV1 = call_local_chat_rpc(
        &dynamic.chat_socket,
        "btc_ceremony_partial_v1",
        &BtcCeremonyPartialRequestV1 {
            schema_version: 1,
            request_id: derived_request_id(reservation_id, "btc", b"ceremony-partial")?,
            reservation_id: reservation_id.clone(),
            taker_partials,
        },
    )
    .await
    .context("btc_ceremony_partial_v1")?;
    let _outcome = ceremony.finish(&nonce.maker_partials, &partial.presignatures)?;
    ensure!(
        partial.maker_actor_activated,
        "the Maker did not activate its actor"
    );

    // Actor: synthesize, activate, receipt.
    if !record.actor_activated {
        let start_height = sidecar.finalized_height().await?;
        let config = synthesize(&ActorSynthesis {
            runtime: &dynamic.runtime,
            layout: &layout,
            agreement: &agreement,
            agreement_wire: &agreement_wire,
            sidecar: &swap_sidecar,
            sessions,
            accepted_at_unix_seconds: now,
            lez_discovery_start_height: start_height,
            prepared_claim_json: &prepared_claim_json,
            maker_lock: None,
        })?;
        activate(&config).await?;
        record.actor_activated = true;
        record.store(&layout)?;
        let receipt = serde_json::json!({
            "schema_version": 1,
            "pair": "bitcoin",
            "swap_id": agreement.coordinator().id().as_str(),
            "role": "taker",
            "agreement_sha256": hex::encode(Sha256::digest(&agreement_wire)),
            "actor_config_file": layout.actor_config_file(),
            "actor_config_sha256": hex::encode(Sha256::digest(read_private(&layout.actor_config_file(), MAX_RECORD_BYTES)?)),
            "actor_state_database": layout.actor_state_db(),
        });
        write_private_exact(&layout.receipt_file(), &serde_json::to_vec(&receipt)?)?;
    }
    Ok(())
}

/// The Taker's Bitcoin lock: broadcasts the exact funding transaction once.
pub(super) async fn lock(
    dynamic: &DynamicBtcRole,
    reservation_id: &RequestId,
) -> Result<(String, bool)> {
    let layout = dynamic.layout(reservation_id);
    ensure!(layout.exists(), "unknown swap");
    let mut record = TakerSwapRecordV1::load(&layout)?;
    ensure!(record.actor_activated, "the swap is not active yet");
    if let Some(txid) = record.funding_broadcast_transaction_id.clone() {
        return Ok((txid, true));
    }
    let plan = record
        .funding_plan
        .clone()
        .context("this role does not fund Bitcoin for this swap")?;
    let txid = dynamic.wallet()?.broadcast(&plan.transaction_hex).await?;
    ensure!(
        txid == plan.transaction_id_display(),
        "the node reported another funding transaction id"
    );
    record.funding_broadcast_transaction_id = Some(txid.clone());
    record.store(&layout)?;
    Ok((txid, false))
}

/// Keeps the swap's sidecar running (respawns it after a Node restart) so the
/// actor's LEZ observations have somewhere to go.
pub(super) fn ensure_sidecar(dynamic: &DynamicBtcRole, reservation_id: &RequestId) -> Result<()> {
    let layout = dynamic.layout(reservation_id);
    ensure!(layout.exists(), "unknown swap");
    SwapSidecar::ensure(&dynamic.runtime, &layout, reservation_id).map(|_| ())
}

/// Whether this swap's Taker funds Bitcoin (has a lock to perform).
pub(super) fn funds_bitcoin(dynamic: &DynamicBtcRole, reservation_id: &RequestId) -> bool {
    let layout = dynamic.layout(reservation_id);
    TakerSwapRecordV1::load(&layout).is_ok_and(|record| record.funding_plan.is_some())
}
