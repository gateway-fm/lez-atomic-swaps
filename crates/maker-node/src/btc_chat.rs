//! Bitcoin Chat negotiation and daemon-owned Maker provisioning.

use std::{
    fs,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, ensure};
use btc_role_preflight::persist_and_bind_countersigned_agreement;

use super::*;

const MAX_BTC_MAKER_AUTHORITY_TEMPLATES: usize = 256;

#[derive(Clone)]
struct BtcMakerAuthority {
    config: BtcActorConfig,
    swap_id: SwapId,
}

/// Validated immutable deployment inputs for daemon-owned BTC actor creation.
#[derive(Clone)]
pub struct BtcMakerActorProvisioner {
    sources: Box<[BtcMakerAuthority]>,
    actor_root: PathBuf,
    actor_program: PathBuf,
    actor_program_sha256: [u8; 32],
}

impl std::fmt::Debug for BtcMakerActorProvisioner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BtcMakerActorProvisioner")
            .field("source_count", &self.sources.len())
            .field("actor_root", &"[REDACTED]")
            .field("actor_program", &"[REDACTED]")
            .field(
                "actor_program_sha256",
                &hex::encode(self.actor_program_sha256),
            )
            .finish()
    }
}

impl BtcMakerActorProvisioner {
    /// Validates a bounded per-swap Maker authority registry and actor program.
    ///
    /// # Errors
    ///
    /// Rejects empty or duplicate authority, unsafe paths, non-Maker sources,
    /// invalid supervised agreements, and a mismatched actor executable.
    pub fn new(
        source_maker_configs: &[PathBuf],
        actor_root: PathBuf,
        actor_program: PathBuf,
        actor_program_sha256: [u8; 32],
    ) -> anyhow::Result<Self> {
        ensure!(
            !source_maker_configs.is_empty()
                && source_maker_configs.len() <= MAX_BTC_MAKER_AUTHORITY_TEMPLATES,
            "BTC Maker authority registry is empty or oversized"
        );
        let mut sources: Vec<BtcMakerAuthority> = Vec::with_capacity(source_maker_configs.len());
        for source_path in source_maker_configs {
            validate_private_config_parent(source_path)?;
            let config = BtcActorConfig::load_private(source_path)
                .map_err(|_| anyhow::anyhow!("BTC source Maker config is unavailable"))?;
            ensure!(
                config.role() == BtcActorRole::Maker,
                "BTC source actor config is not Maker"
            );
            let swap_id = config
                .supervised_swap_id()
                .map_err(|_| anyhow::anyhow!("BTC source Maker agreement is unavailable"))?;
            ensure!(
                sources.iter().all(|existing| {
                    existing.swap_id != swap_id && existing.config.state_db() != config.state_db()
                }),
                "BTC Maker authority registry contains duplicate swap or state identity"
            );
            sources.push(BtcMakerAuthority { config, swap_id });
        }
        validate_private_directory(&actor_root, "BTC Maker actor root")?;
        validate_maker_actor_program(&actor_program, actor_program_sha256)
            .map_err(|_| anyhow::anyhow!("BTC actor program is unavailable"))?;
        Ok(Self {
            sources: sources.into_boxed_slice(),
            actor_root,
            actor_program,
            actor_program_sha256,
        })
    }

    fn supports_draft(&self, draft: &BtcAgreementDraftV1) -> bool {
        let swap_id = hex::encode(draft.body().swap_id());
        self.sources
            .iter()
            .any(|source| source.swap_id.as_str() == swap_id)
    }

    fn provision(
        &self,
        final_agreement_wire: &[u8],
        accepted_at_unix_seconds: u64,
    ) -> anyhow::Result<MakerActorManifestV1> {
        let agreement = BtcAgreementV1::from_wire(final_agreement_wire)
            .context("validate final BTC agreement before Maker provisioning")?;
        let swap_id = agreement.coordinator().id();
        let source = self
            .sources
            .iter()
            .find(|source| &source.swap_id == swap_id)
            .context("accepted BTC swap has no pinned Maker authority template")?;
        let mut digest = Sha256::new();
        digest.update(b"lez-atomic-swaps/maker-btc-actor-root/v1\0");
        digest.update(final_agreement_wire);
        let output_root = self.actor_root.join(hex::encode(digest.finalize()));
        let provisioned = provision_btc_maker_actor_from_config(
            &source.config,
            final_agreement_wire,
            accepted_at_unix_seconds,
            &output_root,
        )
        .map_err(|_| anyhow::anyhow!("provision accepted BTC Maker actor"))?;
        let agreement_sha256: [u8; 32] = Sha256::digest(final_agreement_wire).into();
        ensure!(
            provisioned.role() == BtcActorRole::Maker
                && provisioned.swap_id() == swap_id
                && provisioned.agreement_sha256() == agreement_sha256,
            "provisioned BTC Maker actor identity changed"
        );
        MakerActorManifestV1::new(
            provisioned.swap_id().clone(),
            MakerActorKindV1::Bitcoin,
            provisioned.config_file().to_path_buf(),
            provisioned.config_sha256(),
            self.actor_program.clone(),
            self.actor_program_sha256,
            provisioned.state_database().to_path_buf(),
        )
        .map_err(Into::into)
    }
}

/// Daemon-owned Maker role root used by contribution-bound Chat v2.
///
/// Unlike [`BtcMakerActorProvisioner`], this authority is created before the
/// final agreement and contains only the Maker's own keys and signed public
/// contribution. It cannot register an actor or authorize a chain effect.
#[derive(Clone)]
pub struct BtcMakerRoleAgreementAuthority {
    role_root: PathBuf,
    maker_contribution_wire: Vec<u8>,
}

impl std::fmt::Debug for BtcMakerRoleAgreementAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BtcMakerRoleAgreementAuthority")
            .field("role_root", &"[REDACTED]")
            .field(
                "maker_contribution_sha256",
                &hex::encode(Sha256::digest(&self.maker_contribution_wire)),
            )
            .finish()
    }
}

impl BtcMakerRoleAgreementAuthority {
    /// Loads one role-local Maker contribution and proves it uses the daemon's
    /// agreement signing key.
    ///
    /// # Errors
    ///
    /// Rejects an unsafe root, malformed/non-Maker contribution, or signer drift.
    pub fn new(role_root: PathBuf, signing_key: &SecretKey) -> anyhow::Result<Self> {
        validate_private_directory(&role_root, "BTC Maker role root")?;
        let wire = secure_file::read_private_file(
            &role_root.join("contribution.borsh"),
            MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES as u64,
            "Maker BTC role contribution",
        )?;
        let contribution = BtcRoleContributionV1::from_wire(&wire)
            .context("validate Maker BTC role contribution")?;
        let expected = PublicKey::from_secret_key(&Secp256k1::signing_only(), signing_key);
        ensure!(
            contribution.body().role() == Participant::Maker
                && contribution
                    .body()
                    .participant_identity()
                    .musig2_public_key()
                    == &expected.serialize(),
            "BTC Maker role contribution does not match the daemon signer"
        );
        Ok(Self {
            role_root,
            maker_contribution_wire: wire.to_vec(),
        })
    }

    fn matches_maker_contribution(&self, wire: &[u8]) -> bool {
        self.maker_contribution_wire == wire
    }

    fn bind(
        &self,
        taker_contribution_wire: &[u8],
        final_agreement_wire: &[u8],
        accepted_at_unix_seconds: u64,
    ) -> anyhow::Result<u64> {
        let binding = persist_and_bind_countersigned_agreement(
            &self.role_root,
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
}

pub(super) fn register_btc_chat_methods(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
    module.register_blocking_method::<RpcResult<BtcChatProposalV1>, _>(
        "btc_chat_propose_v1",
        |params, context, _| {
            let request: BtcChatProposeRequestV1 = params.one()?;
            propose_btc_chat(&request, &context)
        },
    )?;
    module.register_blocking_method::<RpcResult<BtcChatProposalV2>, _>(
        "btc_chat_propose_v2",
        |params, context, _| {
            let request: BtcChatProposeRequestV2 = params.one()?;
            propose_btc_chat_v2(&request, &context)
        },
    )?;
    module.register_blocking_method::<RpcResult<BtcChatCompleteResponseV1>, _>(
        "btc_chat_complete_v1",
        |params, context, _| {
            let request: BtcChatCompleteRequestV1 = params.one()?;
            complete_btc_chat(&request, &context)
        },
    )?;
    module.register_blocking_method::<RpcResult<BtcChatCompleteResponseV2>, _>(
        "btc_chat_complete_v2",
        |params, context, _| {
            let request: BtcChatCompleteRequestV2 = params.one()?;
            complete_btc_chat_v2(&request, &context)
        },
    )?;
    super::btc_lifecycle::register_btc_lifecycle_methods(module)?;
    Ok(())
}

/// Which authority vouches for the Maker contribution of a reservation: the
/// Node-owned per-reservation role root, or the legacy single static root.
enum MakerContributionAuthority<'a> {
    Reservation(&'a BtcMakerLifecycle),
    Legacy(&'a BtcMakerRoleAgreementAuthority),
}

impl MakerContributionAuthority<'_> {
    fn resolve(context: &MakerRpc) -> RpcResult<MakerContributionAuthority<'_>> {
        if let Some(lifecycle) = context.btc_lifecycle.as_deref() {
            return Ok(MakerContributionAuthority::Reservation(lifecycle));
        }
        context
            .btc_role_agreement_authority
            .as_deref()
            .map(MakerContributionAuthority::Legacy)
            .ok_or_else(|| invalid_request("maker BTC role agreement authority is unavailable"))
    }

    fn matches_maker_contribution(&self, reservation_id: &RequestId, wire: &[u8]) -> bool {
        match self {
            Self::Reservation(lifecycle) => lifecycle
                .contribution_wire(reservation_id)
                .is_some_and(|known| known == wire),
            Self::Legacy(authority) => authority.matches_maker_contribution(wire),
        }
    }

    fn validate_draft(
        &self,
        reservation_id: &RequestId,
        draft: &BtcAgreementDraftV1,
    ) -> anyhow::Result<()> {
        match self {
            Self::Reservation(lifecycle) => lifecycle.validate_draft(reservation_id, draft),
            Self::Legacy(_) => Ok(()),
        }
    }

    fn bind(
        &self,
        reservation_id: &RequestId,
        taker_contribution_wire: &[u8],
        final_agreement_wire: &[u8],
        accepted_at_unix_seconds: u64,
    ) -> anyhow::Result<u64> {
        match self {
            Self::Reservation(lifecycle) => lifecycle.bind(
                reservation_id,
                taker_contribution_wire,
                final_agreement_wire,
                accepted_at_unix_seconds,
            ),
            Self::Legacy(authority) => authority.bind(
                taker_contribution_wire,
                final_agreement_wire,
                accepted_at_unix_seconds,
            ),
        }
    }
}

#[allow(clippy::too_many_lines)]
fn propose_btc_chat_v2(
    request: &BtcChatProposeRequestV2,
    context: &MakerRpc,
) -> RpcResult<BtcChatProposalV2> {
    validate_btc_proposal_v2_shape(request)?;
    let now_unix_seconds = trusted_now_unix_seconds()?;
    let delivery = context
        .delivery
        .as_ref()
        .ok_or_else(|| invalid_request("maker Chat Delivery is unavailable"))?;
    let signing_key = context
        .btc_chat_signing_key
        .as_ref()
        .ok_or_else(|| invalid_request("maker BTC Chat signer is unavailable"))?;
    let role_authority = MakerContributionAuthority::resolve(context)?;
    let authenticated = delivery
        .authenticate_envelope(&request.signed_offer_envelope)
        .map_err(|error| invalid_request(error.to_string()))?;
    let offer = authenticated.offer();
    if offer.id() != &request.offer_id
        || offer.route().pair() != Pair::Bitcoin
        || offer.created_at_unix_seconds() > now_unix_seconds
        || now_unix_seconds >= offer.expires_at_unix_seconds()
    {
        return Err(invalid_request(
            "Delivery offer does not match live BTC Chat request",
        ));
    }
    let maker_contribution = BtcRoleContributionV1::from_wire(&request.maker_contribution_wire)
        .map_err(invalid_request)?;
    let taker_contribution = BtcRoleContributionV1::from_wire(&request.taker_contribution_wire)
        .map_err(invalid_request)?;
    let contributions = BtcRoleContributionPairV1::new(maker_contribution, taker_contribution)
        .map_err(invalid_request)?;
    if !role_authority
        .matches_maker_contribution(&request.reservation_id, &request.maker_contribution_wire)
    {
        return Err(invalid_request(
            "supplied Maker contribution differs from local role authority",
        ));
    }
    let draft =
        BtcAgreementDraftV1::from_wire(&request.unsigned_draft_wire).map_err(invalid_request)?;
    let maker_key = PublicKey::from_secret_key(&Secp256k1::signing_only(), signing_key);
    let lez_units = offer
        .quote_foreign_amount(request.foreign_units)
        .map_err(invalid_request)?;
    let offer_commitment = authenticated.commitment();
    if !btc_draft_matches_contributions(
        &draft,
        offer,
        &maker_key,
        &request.reservation_id,
        offer_commitment,
        request.foreign_units,
        lez_units,
        now_unix_seconds,
        &contributions,
    ) {
        return Err(invalid_request(
            "unsigned BTC draft is not bound to the offer and signed role contributions",
        ));
    }
    role_authority
        .validate_draft(&request.reservation_id, &draft)
        .map_err(|error| invalid_request(format!("draft differs from the reservation: {error}")))?;
    let agreement_commitment = draft.commitment();
    let keypair = Keypair::from_secret_key(&Secp256k1::signing_only(), signing_key);
    let signature = Secp256k1::signing_only()
        .sign_schnorr_no_aux_rand(&Message::from_digest(agreement_commitment), &keypair)
        .serialize();
    let maker_identity = maker_key.serialize();
    let taker_identity = *contributions
        .taker()
        .body()
        .participant_identity()
        .musig2_public_key();
    let maker_contribution_commitment = *contributions.maker().contribution_commitment();
    let taker_contribution_commitment = *contributions.taker().contribution_commitment();
    let joint_swap_id = *contributions.swap_id();
    let proposal =
        BtcMakerAgreementProposalV1::from_parts(draft, signature).map_err(invalid_request)?;
    let proposal_wire = proposal.encode_wire().map_err(invalid_request)?;
    let negotiation = MakerBtcNegotiationV1::proposed_with_contributions(
        request.reservation_id.clone(),
        offer_commitment,
        maker_identity,
        taker_identity,
        request.foreign_units,
        lez_units,
        now_unix_seconds,
        agreement_commitment,
        proposal_wire.clone(),
        request.maker_contribution_wire.clone(),
        request.taker_contribution_wire.clone(),
    )
    .map_err(invalid_request)?;
    let mut store = context
        .store
        .lock()
        .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
    let commit = store
        .stage_btc_maker_negotiation(
            &request.request_id,
            &request.offer_id,
            request.expected_offer_revision,
            &negotiation,
        )
        .map_err(application_store_error)?;
    Ok(BtcChatProposalV2 {
        schema_version: 2,
        offer_revision: commit.revision(),
        was_replay: commit.was_replay(),
        reservation_id: request.reservation_id.clone(),
        lez_units,
        maker_identity: maker_identity.to_vec(),
        taker_identity: taker_identity.to_vec(),
        joint_swap_id,
        maker_contribution_commitment,
        taker_contribution_commitment,
        agreement_commitment,
        proposal_wire,
    })
}

#[allow(clippy::too_many_lines)]
fn complete_btc_chat_v2(
    request: &BtcChatCompleteRequestV2,
    context: &MakerRpc,
) -> RpcResult<BtcChatCompleteResponseV2> {
    if request.schema_version != 2
        || request.final_agreement_wire.is_empty()
        || request.final_agreement_wire.len() > MAX_BTC_AGREEMENT_RECORD_BYTES
    {
        return Err(invalid_request(
            "unsupported or empty contribution-bound BTC Chat completion",
        ));
    }
    let now_unix_seconds = trusted_now_unix_seconds()?;
    let role_authority = MakerContributionAuthority::resolve(context)?;
    let agreement =
        BtcAgreementV1::from_wire(&request.final_agreement_wire).map_err(invalid_request)?;
    let initial = agreement.coordinator().clone();
    let taker_contribution_wire = {
        let store = context
            .store
            .lock()
            .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
        let negotiation = store
            .load_btc_maker_negotiation(&request.offer_id)
            .map_err(application_store_error)?
            .ok_or_else(|| invalid_request("BTC Chat negotiation is unavailable"))?;
        let (maker_wire, taker_wire) = negotiation.contribution_wires().ok_or_else(|| {
            invalid_request("BTC Chat v2 completion requires signed role contributions")
        })?;
        if negotiation.reservation_id() != &request.reservation_id
            || !role_authority.matches_maker_contribution(&request.reservation_id, maker_wire)
        {
            return Err(invalid_request(
                "BTC Chat completion changed the local contribution or reservation",
            ));
        }
        taker_wire.to_vec()
    };
    let binding_accepted_at_unix_seconds = role_authority
        .bind(
            &request.reservation_id,
            &taker_contribution_wire,
            &request.final_agreement_wire,
            now_unix_seconds,
        )
        .map_err(|error| {
            eprintln!("Maker BTC role binding failed: {error:#}");
            rpc_error(
                INTERNAL_ERROR,
                "Maker role could not persist the validated countersigned agreement",
            )
        })?;
    let acceptance = BtcAgreementAcceptance::new(
        &initial,
        Participant::Maker,
        request.final_agreement_wire.clone(),
        *agreement.agreement_commitment(),
        binding_accepted_at_unix_seconds,
    )
    .map_err(invalid_request)?;
    let swap_id: Box<str> = initial.id().as_str().into();
    let mut store = context
        .store
        .lock()
        .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
    let commit = store
        .accept_maker_btc_negotiation_without_actor(
            &request.request_id,
            &request.offer_id,
            request.expected_offer_revision,
            &request.reservation_id,
            &acceptance,
            &initial,
        )
        .map_err(application_store_error)?;
    Ok(BtcChatCompleteResponseV2 {
        schema_version: 2,
        offer_revision: commit.offer_revision(),
        was_replay: commit.was_replay(),
        swap_id,
        maker_role_bound: true,
        ready_for_public_effects: false,
    })
}

fn validate_btc_proposal_v2_shape(request: &BtcChatProposeRequestV2) -> RpcResult<()> {
    if request.schema_version != 2
        || request.foreign_units == 0
        || request.unsigned_draft_wire.is_empty()
        || request.unsigned_draft_wire.len() > MAX_BTC_AGREEMENT_RECORD_BYTES
        || request.maker_contribution_wire.is_empty()
        || request.maker_contribution_wire.len() > MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES
        || request.taker_contribution_wire.is_empty()
        || request.taker_contribution_wire.len() > MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES
    {
        Err(invalid_request(
            "unsupported or empty contribution-bound BTC Chat proposal",
        ))
    } else {
        Ok(())
    }
}

fn propose_btc_chat(
    request: &BtcChatProposeRequestV1,
    context: &MakerRpc,
) -> RpcResult<BtcChatProposalV1> {
    validate_btc_proposal_shape(request)?;
    let now_unix_seconds = trusted_now_unix_seconds()?;
    let delivery = context
        .delivery
        .as_ref()
        .ok_or_else(|| invalid_request("maker Chat Delivery is unavailable"))?;
    let signing_key = context
        .btc_chat_signing_key
        .as_ref()
        .ok_or_else(|| invalid_request("maker BTC Chat signer is unavailable"))?;
    let provisioner = context
        .btc_actor_provisioner
        .as_ref()
        .ok_or_else(|| invalid_request("maker BTC actor authority is unavailable"))?;
    let authenticated = delivery
        .authenticate_envelope(&request.signed_offer_envelope)
        .map_err(|error| invalid_request(error.to_string()))?;
    let offer = authenticated.offer();
    if offer.id() != &request.offer_id
        || offer.route().pair() != Pair::Bitcoin
        || offer.created_at_unix_seconds() > now_unix_seconds
        || now_unix_seconds >= offer.expires_at_unix_seconds()
    {
        return Err(invalid_request(
            "Delivery offer does not match live BTC Chat request",
        ));
    }
    let draft =
        BtcAgreementDraftV1::from_wire(&request.unsigned_draft_wire).map_err(invalid_request)?;
    let maker_key = PublicKey::from_secret_key(&Secp256k1::signing_only(), signing_key);
    let lez_units = offer
        .quote_foreign_amount(request.foreign_units)
        .map_err(invalid_request)?;
    let offer_commitment = authenticated.commitment();
    if !btc_draft_matches_offer(
        &draft,
        offer,
        &maker_key,
        &request.reservation_id,
        offer_commitment,
        request.foreign_units,
        lez_units,
    ) || !provisioner.supports_draft(&draft)
    {
        return Err(invalid_request(
            "unsigned BTC draft is not bound to the selected offer and authority",
        ));
    }
    let agreement_commitment = draft.commitment();
    let keypair = Keypair::from_secret_key(&Secp256k1::signing_only(), signing_key);
    let signature = Secp256k1::signing_only()
        .sign_schnorr_no_aux_rand(&Message::from_digest(agreement_commitment), &keypair)
        .serialize();
    let maker_identity = maker_key.serialize();
    let taker_identity = *draft
        .body()
        .participants()
        .for_participant(Participant::Taker)
        .musig2_public_key();
    let proposal =
        BtcMakerAgreementProposalV1::from_parts(draft, signature).map_err(invalid_request)?;
    let proposal_wire = proposal.encode_wire().map_err(invalid_request)?;
    let negotiation = MakerBtcNegotiationV1::proposed(
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
        .stage_btc_maker_negotiation(
            &request.request_id,
            &request.offer_id,
            request.expected_offer_revision,
            &negotiation,
        )
        .map_err(application_store_error)?;
    Ok(BtcChatProposalV1 {
        schema_version: 1,
        offer_revision: commit.revision(),
        was_replay: commit.was_replay(),
        reservation_id: request.reservation_id.clone(),
        lez_units,
        maker_identity: maker_identity.to_vec(),
        taker_identity: taker_identity.to_vec(),
        agreement_commitment,
        proposal_wire,
    })
}

fn validate_btc_proposal_shape(request: &BtcChatProposeRequestV1) -> RpcResult<()> {
    if request.schema_version != 1
        || request.foreign_units == 0
        || request.unsigned_draft_wire.is_empty()
        || request.unsigned_draft_wire.len() > MAX_BTC_AGREEMENT_RECORD_BYTES
    {
        Err(invalid_request("unsupported or empty BTC Chat proposal"))
    } else {
        Ok(())
    }
}

fn complete_btc_chat(
    request: &BtcChatCompleteRequestV1,
    context: &MakerRpc,
) -> RpcResult<BtcChatCompleteResponseV1> {
    if request.schema_version != 1 {
        return Err(invalid_request("unsupported BTC Chat completion"));
    }
    {
        let mut store = context
            .store
            .lock()
            .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
        if let Some(replay) = store
            .preflight_maker_btc_scheduled_completion_replay(
                &request.request_id,
                &request.offer_id,
                request.expected_offer_revision,
                &request.reservation_id,
                &request.final_agreement_wire,
            )
            .map_err(application_store_error)?
        {
            return Ok(BtcChatCompleteResponseV1 {
                schema_version: 1,
                offer_revision: replay.offer_revision(),
                was_replay: true,
                swap_id: replay.swap_id().as_str().into(),
            });
        }
        let negotiation = store
            .load_btc_maker_negotiation(&request.offer_id)
            .map_err(application_store_error)?
            .ok_or_else(|| invalid_request("BTC Chat negotiation is unavailable"))?;
        if negotiation.contribution_wires().is_some() {
            return Err(invalid_request(
                "contribution-bound BTC negotiation requires Chat completion v2",
            ));
        }
    }
    let now_unix_seconds = trusted_now_unix_seconds()?;
    let provisioner = context.btc_actor_provisioner.as_ref().ok_or_else(|| {
        rpc_error(
            INTERNAL_ERROR,
            "maker BTC actor provisioning is unavailable",
        )
    })?;
    let agreement =
        BtcAgreementV1::from_wire(&request.final_agreement_wire).map_err(invalid_request)?;
    let initial = agreement.coordinator().clone();
    let acceptance = BtcAgreementAcceptance::new(
        &initial,
        Participant::Maker,
        request.final_agreement_wire.clone(),
        *agreement.agreement_commitment(),
        now_unix_seconds,
    )
    .map_err(invalid_request)?;
    let actor = provisioner
        .provision(&request.final_agreement_wire, now_unix_seconds)
        .map_err(|_| rpc_error(INTERNAL_ERROR, "maker BTC actor provisioning failed"))?;
    let swap_id: Box<str> = initial.id().as_str().into();
    let mut store = context
        .store
        .lock()
        .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
    let commit = store
        .complete_maker_btc_negotiation_and_register_actor(
            &request.request_id,
            &request.offer_id,
            request.expected_offer_revision,
            &request.reservation_id,
            &acceptance,
            &initial,
            &actor,
            now_unix_seconds,
        )
        .map_err(application_store_error)?;
    Ok(BtcChatCompleteResponseV1 {
        schema_version: 1,
        offer_revision: commit.offer_revision(),
        was_replay: commit.was_replay(),
        swap_id,
    })
}

fn btc_draft_matches_offer(
    draft: &BtcAgreementDraftV1,
    offer: &MakerOfferV1,
    maker_key: &PublicKey,
    reservation_id: &RequestId,
    offer_commitment: [u8; 32],
    foreign_units: u64,
    lez_units: u128,
) -> bool {
    let body = draft.body();
    body.swap_id() == &maker_btc_chat_swap_id(&offer_commitment, reservation_id)
        && body.direction() == offer.route().direction()
        && body.funding_terms().value_sat() == foreign_units
        && body.lez_terms().amount() == lez_units
        && body
            .participants()
            .for_participant(Participant::Maker)
            .musig2_public_key()
            == &maker_key.serialize()
}

#[allow(clippy::too_many_arguments)]
fn btc_draft_matches_contributions(
    draft: &BtcAgreementDraftV1,
    offer: &MakerOfferV1,
    maker_key: &PublicKey,
    reservation_id: &RequestId,
    offer_commitment: [u8; 32],
    foreign_units: u64,
    lez_units: u128,
    now_unix_seconds: u64,
    contributions: &BtcRoleContributionPairV1,
) -> bool {
    let body = draft.body();
    let Ok(expected_pre_session) = derive_btc_pre_session_id_v1(
        &offer_commitment,
        reservation_id.as_str().as_bytes(),
        offer.route().direction(),
    ) else {
        return false;
    };
    let contribution = contributions.maker().body();
    contribution.pre_session_id() == &expected_pre_session
        && contribution.expires_at_unix_seconds() == offer.expires_at_unix_seconds()
        && contribution.direction() == offer.route().direction()
        && body.direction() == offer.route().direction()
        && body.funding_terms().value_sat() == foreign_units
        && body.lez_terms().amount() == lez_units
        && contributions
            .validate_agreement_body(body, now_unix_seconds)
            .is_ok()
        && contribution.participant_identity().musig2_public_key() == &maker_key.serialize()
}

fn validate_private_config_parent(path: &Path) -> anyhow::Result<()> {
    ensure!(
        path.is_absolute(),
        "BTC source Maker config must be absolute"
    );
    let parent = path
        .parent()
        .context("BTC source Maker config has no parent")?;
    validate_private_directory(parent, "BTC source Maker config parent")
}

fn validate_private_directory(path: &Path, label: &str) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path).with_context(|| format!("inspect {label}"))?;
    ensure!(
        path.is_absolute()
            && metadata.file_type().is_dir()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.mode() & 0o7777 == 0o700
            && fs::canonicalize(path).with_context(|| format!("canonicalize {label}"))? == path,
        "{label} is unsafe"
    );
    Ok(())
}
