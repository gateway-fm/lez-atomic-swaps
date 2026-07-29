//! Bitcoin Chat negotiation and daemon-owned Maker provisioning.

use std::{
    fs,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, ensure};

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

pub(super) fn register_btc_chat_methods(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
    module.register_blocking_method::<RpcResult<BtcChatProposalV1>, _>(
        "btc_chat_propose_v1",
        |params, context, _| {
            let request: BtcChatProposeRequestV1 = params.one()?;
            propose_btc_chat(&request, &context)
        },
    )?;
    module.register_blocking_method::<RpcResult<BtcChatCompleteResponseV1>, _>(
        "btc_chat_complete_v1",
        |params, context, _| {
            let request: BtcChatCompleteRequestV1 = params.one()?;
            complete_btc_chat(&request, &context)
        },
    )?;
    Ok(())
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
