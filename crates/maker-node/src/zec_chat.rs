//! Zcash Chat negotiation and daemon-owned Maker provisioning.

use std::{fs, os::unix::fs::MetadataExt as _, path::PathBuf};

use anyhow::Context as _;

use super::*;

const MAX_ZEC_MAKER_AUTHORITY_TEMPLATES: usize = 256;

pub(super) fn register_zec_chat_methods(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
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
    if let Some(replay) = completion_store
        .preflight_maker_zec_scheduled_completion_replay_for_role(
            &request.request_id,
            &request.offer_id,
            request.expected_offer_revision,
            &request.reservation_id,
            &request.final_agreement_wire,
            context.maker_claim_preimage.as_deref(),
        )
        .map_err(application_store_error)?
    {
        return Ok(ZecChatCompleteResponseV1 {
            schema_version: 1,
            offer_revision: replay.offer_revision(),
            was_replay: true,
            swap_id: replay.swap_id().as_str().into(),
        });
    }
    let now_unix_seconds = trusted_now_unix_seconds()?;
    let provisioner = context
        .zec_actor_provisioner
        .as_ref()
        .ok_or_else(|| rpc_error(INTERNAL_ERROR, "maker actor provisioning is unavailable"))?;
    let accepted = AcceptedZecAgreementV1::accept_wire_at(
        &request.final_agreement_wire,
        lez_swap_core::UnixSeconds::new(now_unix_seconds),
        Participant::Maker,
        0,
    )
    .map_err(invalid_request)?;
    let maker_preimage = if accepted.agreement().lez_claimant() == Participant::Maker {
        Some(
            context
                .maker_claim_preimage
                .as_deref()
                .ok_or_else(|| invalid_request("maker claim authority is unavailable"))?,
        )
    } else {
        None
    };
    let actor = provisioner
        .provision(&request.final_agreement_wire, now_unix_seconds)
        .map_err(|_| rpc_error(INTERNAL_ERROR, "maker actor provisioning failed"))?;
    let swap_id: Box<str> = accepted.agreement().coordinator().id().as_str().into();
    let commit = completion_store
        .complete_maker_zec_negotiation_and_register_actor_for_role(
            &request.request_id,
            &request.offer_id,
            request.expected_offer_revision,
            &request.reservation_id,
            &accepted,
            maker_preimage,
            &actor,
            now_unix_seconds,
        )
        .map_err(application_store_error)?;
    Ok(ZecChatCompleteResponseV1 {
        schema_version: 1,
        offer_revision: commit.offer_revision(),
        was_replay: commit.was_replay(),
        swap_id,
    })
}

/// Validated immutable deployment inputs for daemon-owned ZEC actor creation.
#[derive(Clone)]
pub struct ZecMakerActorProvisioner {
    source_maker_configs: Box<[ActorConfig]>,
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
                &self.source_maker_configs.len(),
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
            !source_maker_configs.is_empty()
                && source_maker_configs.len() <= MAX_ZEC_MAKER_AUTHORITY_TEMPLATES,
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
            source_maker_configs: sources.into_boxed_slice(),
            actor_root,
            actor_program,
            actor_program_sha256,
        })
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
            .source_maker_configs
            .iter()
            .find(|source| source.swap_id().as_str() == accepted.agreement().application_swap_id())
            .context("accepted ZEC swap has no pinned Maker authority template")?;
        let mut digest = Sha256::new();
        digest.update(b"lez-atomic-swaps/maker-actor-root/v1\0");
        digest.update(final_agreement_wire);
        let output_root = self.actor_root.join(hex::encode(digest.finalize()));
        let provisioned = provision_zec_maker_actor_from_config(
            source,
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
