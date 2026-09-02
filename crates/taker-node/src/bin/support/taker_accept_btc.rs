use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, ensure};
use btc_reference_actor::{
    ActorConfig, ActorRole, BtcActorProvisionV1, provision_btc_taker_actor_from_config,
};
use btc_role_preflight::persist_and_bind_countersigned_agreement;
use lez_bridge_protocol::RequestId;
use lez_btc_swap_sdk::{
    BtcAgreementDraftV1, BtcAgreementV1, BtcMakerAgreementProposalV1, BtcRoleContributionPairV1,
    BtcRoleContributionV1, MAX_BTC_AGREEMENT_RECORD_BYTES, MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES,
    derive_btc_pre_session_id_v1,
};
use lez_swap_core::{Pair, Participant, SwapDirection};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{MakerOfferId, MakerRouteV1, maker_btc_chat_swap_id};
use lez_taker_node::{
    BtcChatCompleteRequestV1, BtcChatCompleteRequestV2, BtcChatCompleteResponseV1,
    BtcChatCompleteResponseV2, BtcChatProposalV1, BtcChatProposalV2, BtcChatProposeRequestV1,
    BtcChatProposeRequestV2, DeliveryOfferQueryV1, RunLocalDelivery, call_local_chat_rpc,
    secure_file::{load_raw_secret, read_private_file},
};
use secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::taker_accept::{
    MAX_TAKER_RECEIPT_BYTES, ReplayOutput, decode_sha256, normalized_absolute, publish_exact_new,
    resolved_new_path,
};

pub(crate) struct BtcTakeInput<'a> {
    pub(crate) direction: SwapDirection,
    pub(crate) delivery: Option<&'a RunLocalDelivery>,
    pub(crate) now_unix_seconds: u64,
    pub(crate) offer_id: &'a str,
    pub(crate) chat_socket: &'a Path,
    pub(crate) reservation_id: &'a str,
    pub(crate) foreign_units: u64,
    pub(crate) unsigned_draft_file: &'a Path,
    pub(crate) contribution_files: Option<(&'a Path, &'a Path)>,
    pub(crate) role_root: Option<&'a Path>,
    pub(crate) source_taker_config_file: Option<&'a Path>,
    pub(crate) taker_actor_root: Option<&'a Path>,
    pub(crate) acceptance_receipt_file: Option<&'a Path>,
    pub(crate) taker_signing_key_file: &'a Path,
    pub(crate) agreement_output_file: &'a Path,
}

#[derive(Serialize)]
pub(crate) struct BtcAcceptanceOutput {
    schema_version: u16,
    offer_id: String,
    offer_revision: u64,
    reservation_id: String,
    swap_id: Box<str>,
    agreement_file: PathBuf,
    agreement_sha256: String,
    replay: ReplayOutput,
    ready_for_public_effects: bool,
    fixture_actor_authority_used: bool,
    private_material_disclosed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    actor: Option<BtcAcceptanceActorOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agreement_binding: Option<BtcAgreementBindingOutput>,
}

#[derive(Serialize)]
struct BtcAgreementBindingOutput {
    role: ActorRole,
    binding_file: PathBuf,
    binding_sha256: String,
    was_replay: bool,
}

#[derive(Serialize)]
struct BtcAcceptanceActorOutput {
    role: ActorRole,
    receipt_file: PathBuf,
    receipt_sha256: String,
    provisioning_replay: bool,
    receipt_replay: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BtcAcceptanceReceiptV1 {
    schema_version: u16,
    pair: Box<str>,
    swap_id: Box<str>,
    role: ActorRole,
    agreement_sha256: String,
    actor_config_file: PathBuf,
    actor_config_sha256: String,
    actor_state_database: PathBuf,
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn take_btc(input: BtcTakeInput<'_>) -> anyhow::Result<BtcAcceptanceOutput> {
    validate_acceptance_paths(&input)?;
    let offer_id = MakerOfferId::new(input.offer_id)?;
    let reservation_id = RequestId::new(input.reservation_id)?;
    ensure!(input.foreign_units > 0, "BTC principal must be nonzero");

    match fs::symlink_metadata(input.agreement_output_file) {
        Ok(_) => return resume_persisted_btc(&input, offer_id, reservation_id).await,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect persisted BTC agreement"),
    }

    let route = MakerRouteV1::new(Pair::Bitcoin, input.direction)?;
    let selected = input
        .delivery
        .context("fresh BTC acceptance requires Delivery")?
        .discover(&DeliveryOfferQueryV1::for_route(
            route,
            input.now_unix_seconds,
        ))
        .await?
        .into_iter()
        .find(|candidate| candidate.offer().id() == &offer_id)
        .context("selected BTC offer is unavailable, expired, or not authentic")?;
    let expected_lez_units = selected.offer().quote_foreign_amount(input.foreign_units)?;
    let draft_wire = read_private_file(
        input.unsigned_draft_file,
        MAX_BTC_AGREEMENT_RECORD_BYTES as u64,
        "unsigned BTC agreement draft",
    )?;
    let draft = BtcAgreementDraftV1::from_wire(&draft_wire)
        .context("validate unsigned BTC agreement draft")?;
    let taker_secret_material =
        load_raw_secret(input.taker_signing_key_file, "taker BTC agreement key")?;
    let taker_secret = SecretKey::from_slice(taker_secret_material.as_ref())
        .context("validate taker BTC agreement key")?;
    let taker_public = PublicKey::from_secret_key(&Secp256k1::signing_only(), &taker_secret);
    let proposal = if let Some((maker_contribution_file, taker_contribution_file)) =
        input.contribution_files
    {
        let maker_contribution_wire = read_private_file(
            maker_contribution_file,
            MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES as u64,
            "Maker BTC role contribution",
        )?;
        let taker_contribution_wire = read_private_file(
            taker_contribution_file,
            MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES as u64,
            "Taker BTC role contribution",
        )?;
        let contributions = BtcRoleContributionPairV1::new(
            BtcRoleContributionV1::from_wire(&maker_contribution_wire)?,
            BtcRoleContributionV1::from_wire(&taker_contribution_wire)?,
        )?;
        validate_contributed_draft(
            &draft,
            selected.commitment(),
            &reservation_id,
            input.foreign_units,
            expected_lez_units,
            &taker_public,
            input.direction,
            selected.offer().expires_at_unix_seconds(),
            input.now_unix_seconds,
            &contributions,
        )?;
        let response: BtcChatProposalV2 = call_local_chat_rpc(
            input.chat_socket,
            "btc_chat_propose_v2",
            &BtcChatProposeRequestV2 {
                schema_version: 2,
                request_id: derived_request_id(&reservation_id, b"propose-v2")?,
                offer_id: offer_id.clone(),
                expected_offer_revision: 1,
                reservation_id: reservation_id.clone(),
                foreign_units: input.foreign_units,
                signed_offer_envelope: selected.signed_envelope().to_vec(),
                maker_contribution_wire: maker_contribution_wire.to_vec(),
                taker_contribution_wire: taker_contribution_wire.to_vec(),
                unsigned_draft_wire: draft_wire.to_vec(),
            },
        )
        .await?;
        ensure!(
            response.schema_version == 2
                && response.joint_swap_id == *contributions.swap_id()
                && response.maker_contribution_commitment
                    == *contributions.maker().contribution_commitment()
                && response.taker_contribution_commitment
                    == *contributions.taker().contribution_commitment(),
            "maker BTC proposal changed the signed contribution transcript"
        );
        BtcChatProposalV1 {
            schema_version: 1,
            offer_revision: response.offer_revision,
            was_replay: response.was_replay,
            reservation_id: response.reservation_id,
            lez_units: response.lez_units,
            maker_identity: response.maker_identity,
            taker_identity: response.taker_identity,
            agreement_commitment: response.agreement_commitment,
            proposal_wire: response.proposal_wire,
        }
    } else {
        validate_fresh_draft(
            &draft,
            selected.commitment(),
            &reservation_id,
            input.foreign_units,
            expected_lez_units,
            &taker_public,
            input.direction,
        )?;
        call_local_chat_rpc(
            input.chat_socket,
            "btc_chat_propose_v1",
            &BtcChatProposeRequestV1 {
                schema_version: 1,
                request_id: derived_request_id(&reservation_id, b"propose")?,
                offer_id: offer_id.clone(),
                expected_offer_revision: 1,
                reservation_id: reservation_id.clone(),
                foreign_units: input.foreign_units,
                signed_offer_envelope: selected.signed_envelope().to_vec(),
                unsigned_draft_wire: draft_wire.to_vec(),
            },
        )
        .await?
    };
    let maker_proposal = BtcMakerAgreementProposalV1::from_wire(&proposal.proposal_wire)
        .context("validate maker-signed BTC proposal")?;
    let maker_identity = maker_proposal
        .body()
        .participants()
        .for_participant(Participant::Maker)
        .musig2_public_key();
    ensure!(
        proposal.schema_version == 1
            && proposal.offer_revision == 2
            && proposal.reservation_id == reservation_id
            && proposal.lez_units == expected_lez_units
            && proposal.maker_identity.as_slice() == maker_identity
            && proposal.taker_identity.as_slice() == taker_public.serialize()
            && proposal.agreement_commitment == maker_proposal.commitment()
            && maker_proposal.body() == draft.body(),
        "maker BTC proposal changed the selected offer, identities, or executable draft"
    );
    complete_btc(
        &input,
        offer_id,
        reservation_id,
        proposal,
        maker_proposal,
        taker_secret,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
fn validate_contributed_draft(
    draft: &BtcAgreementDraftV1,
    offer_commitment: [u8; 32],
    reservation_id: &RequestId,
    foreign_units: u64,
    lez_units: u128,
    taker_public: &PublicKey,
    direction: SwapDirection,
    offer_expires_at_unix_seconds: u64,
    now_unix_seconds: u64,
    contributions: &BtcRoleContributionPairV1,
) -> anyhow::Result<()> {
    let body = draft.body();
    let expected_pre_session = derive_btc_pre_session_id_v1(
        &offer_commitment,
        reservation_id.as_str().as_bytes(),
        direction,
    )?;
    ensure!(
        contributions.maker().body().pre_session_id() == &expected_pre_session
            && contributions.maker().body().expires_at_unix_seconds()
                == offer_expires_at_unix_seconds
            && body.direction() == direction
            && body.funding_terms().value_sat() == foreign_units
            && body.lez_terms().amount() == lez_units
            && contributions
                .validate_agreement_body(body, now_unix_seconds)
                .is_ok()
            && contributions
                .taker()
                .body()
                .participant_identity()
                .musig2_public_key()
                == &taker_public.serialize(),
        "unsigned BTC draft is not bound to the offer and local signed role contributions"
    );
    Ok(())
}

fn load_contributions(input: &BtcTakeInput<'_>) -> anyhow::Result<BtcRoleContributionPairV1> {
    let (maker_file, taker_file) = input
        .contribution_files
        .context("contribution-bound BTC acceptance requires both role contributions")?;
    let maker_wire = read_private_file(
        maker_file,
        MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES as u64,
        "Maker BTC role contribution",
    )?;
    let taker_wire = read_private_file(
        taker_file,
        MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES as u64,
        "Taker BTC role contribution",
    )?;
    BtcRoleContributionPairV1::new(
        BtcRoleContributionV1::from_wire(&maker_wire)?,
        BtcRoleContributionV1::from_wire(&taker_wire)?,
    )
    .map_err(Into::into)
}

fn validate_fresh_draft(
    draft: &BtcAgreementDraftV1,
    offer_commitment: [u8; 32],
    reservation_id: &RequestId,
    foreign_units: u64,
    lez_units: u128,
    taker_public: &PublicKey,
    direction: SwapDirection,
) -> anyhow::Result<()> {
    let body = draft.body();
    ensure!(
        body.swap_id() == &maker_btc_chat_swap_id(&offer_commitment, reservation_id)
            && body.direction() == direction
            && body.funding_terms().value_sat() == foreign_units
            && body.lez_terms().amount() == lez_units
            && body
                .participants()
                .for_participant(Participant::Taker)
                .musig2_public_key()
                == &taker_public.serialize(),
        "unsigned BTC draft is not bound to the selected offer and local Taker"
    );
    Ok(())
}

async fn complete_btc(
    input: &BtcTakeInput<'_>,
    offer_id: MakerOfferId,
    reservation_id: RequestId,
    proposal: BtcChatProposalV1,
    maker_proposal: BtcMakerAgreementProposalV1,
    mut taker_secret: SecretKey,
) -> anyhow::Result<BtcAcceptanceOutput> {
    let secp = Secp256k1::signing_only();
    let mut signing_aux = [0_u8; 32];
    getrandom::fill(&mut signing_aux).context("Taker BTC signing randomness unavailable")?;
    let taker_signature = secp
        .sign_schnorr_with_aux_rand(
            &Message::from_digest(proposal.agreement_commitment),
            &Keypair::from_secret_key(&secp, &taker_secret),
            &signing_aux,
        )
        .serialize();
    signing_aux.fill(0);
    taker_secret.non_secure_erase();
    let agreement = maker_proposal
        .complete(taker_signature)
        .context("countersign maker BTC proposal")?;
    let final_wire = agreement.encode_wire()?;
    let agreement_file_was_replay = publish_exact_new(
        input.agreement_output_file,
        &final_wire,
        MAX_BTC_AGREEMENT_RECORD_BYTES as u64,
        "countersigned BTC agreement",
    )?;
    if input.contribution_files.is_some() {
        let binding = bind_taker_role(input, &final_wire)?;
        let completion = complete_with_maker_v2(
            input,
            &offer_id,
            &reservation_id,
            proposal.offer_revision,
            &final_wire,
        )
        .await?;
        validate_completion_v2(&completion, &agreement)?;
        return Ok(BtcAcceptanceOutput {
            schema_version: 2,
            offer_id: offer_id.as_str().to_owned(),
            offer_revision: completion.offer_revision,
            reservation_id: reservation_id.as_str().to_owned(),
            swap_id: completion.swap_id,
            agreement_file: input.agreement_output_file.to_path_buf(),
            agreement_sha256: hex::encode(Sha256::digest(&final_wire)),
            replay: ReplayOutput {
                proposal: proposal.was_replay,
                completion: completion.was_replay,
                agreement_file: agreement_file_was_replay,
            },
            ready_for_public_effects: false,
            fixture_actor_authority_used: false,
            private_material_disclosed: false,
            actor: None,
            agreement_binding: Some(binding),
        });
    }
    let provisioned = provision_taker_actor(input, &final_wire)?;
    let completion = complete_with_maker(
        input,
        &offer_id,
        &reservation_id,
        proposal.offer_revision,
        &final_wire,
    )
    .await?;
    validate_completion(&completion, &agreement)?;
    let replay = ReplayOutput {
        proposal: proposal.was_replay,
        completion: completion.was_replay,
        agreement_file: agreement_file_was_replay,
    };
    acceptance_output(
        input,
        &offer_id,
        &reservation_id,
        &final_wire,
        replay,
        completion,
        &provisioned,
    )
}

async fn resume_persisted_btc(
    input: &BtcTakeInput<'_>,
    offer_id: MakerOfferId,
    reservation_id: RequestId,
) -> anyhow::Result<BtcAcceptanceOutput> {
    let final_wire = read_private_file(
        input.agreement_output_file,
        MAX_BTC_AGREEMENT_RECORD_BYTES as u64,
        "persisted countersigned BTC agreement",
    )?;
    let agreement =
        BtcAgreementV1::from_wire(&final_wire).context("validate persisted BTC agreement")?;
    let draft_wire = read_private_file(
        input.unsigned_draft_file,
        MAX_BTC_AGREEMENT_RECORD_BYTES as u64,
        "unsigned BTC agreement draft",
    )?;
    let draft = BtcAgreementDraftV1::from_wire(&draft_wire)
        .context("validate unsigned BTC draft for persisted retry")?;
    let taker_secret_material =
        load_raw_secret(input.taker_signing_key_file, "taker BTC agreement key")?;
    let mut taker_secret = SecretKey::from_slice(taker_secret_material.as_ref())
        .context("validate taker BTC agreement key")?;
    let taker_public = PublicKey::from_secret_key(&Secp256k1::signing_only(), &taker_secret);
    taker_secret.non_secure_erase();
    ensure!(
        agreement.body() == draft.body()
            && agreement.direction() == input.direction
            && agreement.funding_terms().value_sat() == input.foreign_units
            && agreement
                .participant(Participant::Taker)
                .musig2_public_key()
                == &taker_public.serialize(),
        "persisted BTC agreement does not match the executable draft or local Taker"
    );
    if input.contribution_files.is_some() {
        let contributions = load_contributions(input)?;
        ensure!(
            contributions
                .validate_agreement_body_fields(agreement.body())
                .is_ok(),
            "persisted BTC agreement changed the signed role contributions"
        );
        let binding = bind_taker_role(input, &final_wire)?;
        let completion =
            complete_with_maker_v2(input, &offer_id, &reservation_id, 2, &final_wire).await?;
        validate_completion_v2(&completion, &agreement)?;
        return Ok(BtcAcceptanceOutput {
            schema_version: 2,
            offer_id: offer_id.as_str().to_owned(),
            offer_revision: completion.offer_revision,
            reservation_id: reservation_id.as_str().to_owned(),
            swap_id: completion.swap_id,
            agreement_file: input.agreement_output_file.to_path_buf(),
            agreement_sha256: hex::encode(Sha256::digest(&final_wire)),
            replay: ReplayOutput {
                proposal: true,
                completion: completion.was_replay,
                agreement_file: true,
            },
            ready_for_public_effects: false,
            fixture_actor_authority_used: false,
            private_material_disclosed: false,
            actor: None,
            agreement_binding: Some(binding),
        });
    }
    let provisioned = provision_taker_actor(input, &final_wire)?;
    let completion = complete_with_maker(input, &offer_id, &reservation_id, 2, &final_wire).await?;
    validate_completion(&completion, &agreement)?;
    let replay = ReplayOutput {
        proposal: true,
        completion: completion.was_replay,
        agreement_file: true,
    };
    acceptance_output(
        input,
        &offer_id,
        &reservation_id,
        &final_wire,
        replay,
        completion,
        &provisioned,
    )
}

async fn complete_with_maker(
    input: &BtcTakeInput<'_>,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    expected_offer_revision: u64,
    final_wire: &[u8],
) -> anyhow::Result<BtcChatCompleteResponseV1> {
    call_local_chat_rpc(
        input.chat_socket,
        "btc_chat_complete_v1",
        &BtcChatCompleteRequestV1 {
            schema_version: 1,
            request_id: derived_request_id(reservation_id, b"complete")?,
            offer_id: offer_id.clone(),
            expected_offer_revision,
            reservation_id: reservation_id.clone(),
            final_agreement_wire: final_wire.to_vec(),
        },
    )
    .await
}

async fn complete_with_maker_v2(
    input: &BtcTakeInput<'_>,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    expected_offer_revision: u64,
    final_wire: &[u8],
) -> anyhow::Result<BtcChatCompleteResponseV2> {
    call_local_chat_rpc(
        input.chat_socket,
        "btc_chat_complete_v2",
        &BtcChatCompleteRequestV2 {
            schema_version: 2,
            request_id: derived_request_id(reservation_id, b"complete-v2")?,
            offer_id: offer_id.clone(),
            expected_offer_revision,
            reservation_id: reservation_id.clone(),
            final_agreement_wire: final_wire.to_vec(),
        },
    )
    .await
}

fn validate_completion(
    completion: &BtcChatCompleteResponseV1,
    agreement: &BtcAgreementV1,
) -> anyhow::Result<()> {
    ensure!(
        completion.schema_version == 1
            && completion.offer_revision == 3
            && completion.swap_id.as_ref() == agreement.coordinator().id().as_str(),
        "maker completion result does not match the countersigned BTC agreement"
    );
    Ok(())
}

fn validate_completion_v2(
    completion: &BtcChatCompleteResponseV2,
    agreement: &BtcAgreementV1,
) -> anyhow::Result<()> {
    ensure!(
        completion.schema_version == 2
            && completion.offer_revision == 3
            && completion.swap_id.as_ref() == agreement.coordinator().id().as_str()
            && completion.maker_role_bound
            && !completion.ready_for_public_effects,
        "maker fixture-independent completion changed the agreement or effect gate"
    );
    Ok(())
}

fn bind_taker_role(
    input: &BtcTakeInput<'_>,
    final_wire: &[u8],
) -> anyhow::Result<BtcAgreementBindingOutput> {
    let role_root = input
        .role_root
        .context("contribution-bound BTC acceptance requires a Taker role root")?;
    let (maker_contribution_file, _) = input
        .contribution_files
        .context("contribution-bound BTC acceptance requires role contributions")?;
    let maker_wire = read_private_file(
        maker_contribution_file,
        MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES as u64,
        "Maker BTC role contribution",
    )?;
    let binding = persist_and_bind_countersigned_agreement(
        role_root,
        &maker_wire,
        final_wire,
        input.now_unix_seconds,
    )?;
    ensure!(
        !binding.ready_for_public_effects(),
        "Taker agreement binding unexpectedly authorized public effects"
    );
    let binding_bytes = read_private_file(
        binding.binding_file(),
        MAX_TAKER_RECEIPT_BYTES,
        "Taker BTC agreement binding",
    )?;
    Ok(BtcAgreementBindingOutput {
        role: ActorRole::Taker,
        binding_file: binding.binding_file().to_path_buf(),
        binding_sha256: hex::encode(Sha256::digest(&binding_bytes)),
        was_replay: binding.was_replay(),
    })
}

fn provision_taker_actor(
    input: &BtcTakeInput<'_>,
    final_wire: &[u8],
) -> anyhow::Result<BtcActorProvisionV1> {
    let source_file = input
        .source_taker_config_file
        .context("legacy BTC acceptance requires a source Taker actor config")?;
    let actor_root = input
        .taker_actor_root
        .context("legacy BTC acceptance requires a Taker actor root")?;
    let source = ActorConfig::load_private(source_file)
        .map_err(|_| anyhow::anyhow!("BTC source Taker config is unavailable"))?;
    ensure!(
        source.role() == ActorRole::Taker,
        "BTC source actor is not Taker"
    );
    let provisioned = provision_btc_taker_actor_from_config(
        &source,
        final_wire,
        input.now_unix_seconds,
        actor_root,
    )
    .map_err(|_| anyhow::anyhow!("provision accepted BTC Taker actor"))?;
    ensure!(
        provisioned.role() == ActorRole::Taker,
        "provisioned actor has the wrong role"
    );
    Ok(provisioned)
}

fn acceptance_output(
    input: &BtcTakeInput<'_>,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    final_wire: &[u8],
    replay: ReplayOutput,
    completion: BtcChatCompleteResponseV1,
    provisioned: &BtcActorProvisionV1,
) -> anyhow::Result<BtcAcceptanceOutput> {
    Ok(BtcAcceptanceOutput {
        schema_version: 1,
        offer_id: offer_id.as_str().to_owned(),
        offer_revision: completion.offer_revision,
        reservation_id: reservation_id.as_str().to_owned(),
        swap_id: completion.swap_id,
        agreement_file: input.agreement_output_file.to_path_buf(),
        agreement_sha256: hex::encode(Sha256::digest(final_wire)),
        replay,
        ready_for_public_effects: true,
        fixture_actor_authority_used: true,
        private_material_disclosed: false,
        actor: Some(publish_acceptance_receipt(input, final_wire, provisioned)?),
        agreement_binding: None,
    })
}

fn publish_acceptance_receipt(
    input: &BtcTakeInput<'_>,
    final_wire: &[u8],
    provisioned: &BtcActorProvisionV1,
) -> anyhow::Result<BtcAcceptanceActorOutput> {
    let receipt_file = input
        .acceptance_receipt_file
        .context("legacy BTC acceptance requires an acceptance receipt")?;
    let agreement_sha256 = hex::encode(Sha256::digest(final_wire));
    ensure!(
        agreement_sha256 == hex::encode(provisioned.agreement_sha256()),
        "provisioned BTC agreement identity changed"
    );
    ensure!(
        receipt_file != provisioned.config_file() && receipt_file != provisioned.state_database(),
        "BTC acceptance receipt aliases actor state"
    );
    let receipt = BtcAcceptanceReceiptV1 {
        schema_version: 1,
        pair: "bitcoin".into(),
        swap_id: provisioned.swap_id().as_str().into(),
        role: provisioned.role(),
        agreement_sha256,
        actor_config_file: provisioned.config_file().to_path_buf(),
        actor_config_sha256: hex::encode(provisioned.config_sha256()),
        actor_state_database: provisioned.state_database().to_path_buf(),
    };
    let receipt_bytes = serde_json::to_vec(&receipt).context("encode BTC acceptance receipt")?;
    let receipt_replay = publish_exact_new(
        receipt_file,
        &receipt_bytes,
        MAX_TAKER_RECEIPT_BYTES,
        "BTC Taker acceptance receipt",
    )?;
    Ok(BtcAcceptanceActorOutput {
        role: provisioned.role(),
        receipt_file: receipt_file.to_path_buf(),
        receipt_sha256: hex::encode(Sha256::digest(&receipt_bytes)),
        provisioning_replay: provisioned.was_replay(),
        receipt_replay,
    })
}

pub(crate) fn load_btc_taker_actor_from_receipt(path: &Path) -> anyhow::Result<ActorConfig> {
    let bytes = read_private_file(
        path,
        MAX_TAKER_RECEIPT_BYTES,
        "BTC Taker acceptance receipt",
    )?;
    let receipt: BtcAcceptanceReceiptV1 =
        serde_json::from_slice(&bytes).context("decode BTC Taker acceptance receipt")?;
    ensure!(
        receipt.schema_version == 1
            && receipt.pair.as_ref() == "bitcoin"
            && receipt.role == ActorRole::Taker,
        "BTC Taker acceptance receipt has an unsupported pair, role, or version"
    );
    ensure!(
        normalized_absolute(path)
            && normalized_absolute(&receipt.actor_config_file)
            && normalized_absolute(&receipt.actor_state_database),
        "BTC Taker acceptance receipt paths must be normalized and absolute"
    );
    let config_sha256 = decode_sha256(&receipt.actor_config_sha256, "BTC actor config")?;
    let agreement_sha256 = decode_sha256(&receipt.agreement_sha256, "BTC agreement")?;
    let config = ActorConfig::load_private_pinned_sha256(&receipt.actor_config_file, config_sha256)
        .context("load receipt-bound BTC Taker actor config")?;
    ensure!(
        config.role() == ActorRole::Taker
            && config.supervised_swap_id()?.as_str() == receipt.swap_id.as_ref()
            && config.state_db() == receipt.actor_state_database
            && config.agreement_sha256() == Some(agreement_sha256),
        "receipt-bound BTC Taker actor semantics changed"
    );
    Ok(config)
}

fn validate_acceptance_paths(input: &BtcTakeInput<'_>) -> anyhow::Result<()> {
    let agreement = resolved_new_path(input.agreement_output_file, "BTC agreement output")?;
    if input.contribution_files.is_some() {
        let role_root = input
            .role_root
            .context("contribution-bound BTC acceptance requires --btc-role-root")?;
        let metadata = fs::symlink_metadata(role_root).context("inspect BTC Taker role root")?;
        ensure!(
            normalized_absolute(role_root)
                && metadata.file_type().is_dir()
                && fs::canonicalize(role_root)? == role_root,
            "BTC Taker role root must be an existing canonical directory"
        );
        ensure!(
            !agreement.starts_with(role_root) && !role_root.starts_with(&agreement),
            "BTC agreement output must be disjoint from the Taker role root"
        );
        ensure!(
            input.source_taker_config_file.is_none()
                && input.taker_actor_root.is_none()
                && input.acceptance_receipt_file.is_none(),
            "contribution-bound BTC acceptance rejects fixture actor authority"
        );
        return Ok(());
    }
    ensure!(
        input.role_root.is_none(),
        "legacy BTC acceptance rejects a role root"
    );
    let receipt_file = input
        .acceptance_receipt_file
        .context("legacy BTC acceptance requires an acceptance receipt")?;
    let actor_root_file = input
        .taker_actor_root
        .context("legacy BTC acceptance requires a Taker actor root")?;
    let _ = input
        .source_taker_config_file
        .context("legacy BTC acceptance requires a source actor config")?;
    let receipt = resolved_new_path(receipt_file, "BTC acceptance receipt")?;
    let actor_root = resolved_new_path(actor_root_file, "BTC Taker actor root")?;
    ensure!(
        receipt != agreement && !receipt.starts_with(&actor_root),
        "BTC acceptance receipt must be outside actor authority and agreement paths"
    );
    Ok(())
}

fn derived_request_id(reservation_id: &RequestId, label: &[u8]) -> anyhow::Result<RequestId> {
    let mut digest = Sha256::new();
    digest.update(b"lez-atomic-swaps/btc-taker-chat-request/v1\0");
    digest.update(reservation_id.as_str().as_bytes());
    digest.update([0]);
    digest.update(label);
    RequestId::new(hex::encode(digest.finalize())).map_err(Into::into)
}
