use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, ensure};
use btc_reference_actor::{
    ActorConfig, ActorRole, BtcActorProvisionV1, provision_btc_taker_actor_from_config,
};
use lez_bridge_protocol::RequestId;
use lez_btc_swap_sdk::{
    BtcAgreementDraftV1, BtcAgreementV1, BtcMakerAgreementProposalV1,
    MAX_BTC_AGREEMENT_RECORD_BYTES,
};
use lez_maker_node::{
    BtcChatCompleteRequestV1, BtcChatCompleteResponseV1, BtcChatProposalV1,
    BtcChatProposeRequestV1, DeliveryOfferQueryV1, RunLocalDelivery, call_local_rpc,
    secure_file::{load_raw_secret, read_private_file},
};
use lez_swap_core::{Pair, Participant, SwapDirection};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{MakerOfferId, MakerRouteV1, maker_btc_chat_swap_id};
use secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::taker_accept::{
    MAX_TAKER_RECEIPT_BYTES, ReplayOutput, decode_sha256, normalized_absolute, publish_exact_new,
    resolved_new_path,
};

pub(crate) struct BtcTakeInput<'a> {
    pub(crate) delivery: Option<&'a RunLocalDelivery>,
    pub(crate) now_unix_seconds: u64,
    pub(crate) offer_id: &'a str,
    pub(crate) chat_socket: &'a Path,
    pub(crate) reservation_id: &'a str,
    pub(crate) foreign_units: u64,
    pub(crate) unsigned_draft_file: &'a Path,
    pub(crate) source_taker_config_file: &'a Path,
    pub(crate) taker_actor_root: &'a Path,
    pub(crate) acceptance_receipt_file: &'a Path,
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
    private_material_disclosed: bool,
    actor: BtcAcceptanceActorOutput,
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

    let route = MakerRouteV1::new(Pair::Bitcoin, SwapDirection::TakerSellsForeign)?;
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
    validate_fresh_draft(
        &draft,
        selected.commitment(),
        &reservation_id,
        input.foreign_units,
        expected_lez_units,
        &taker_public,
    )?;

    let proposal: BtcChatProposalV1 = call_local_rpc(
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
    .await?;
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

fn validate_fresh_draft(
    draft: &BtcAgreementDraftV1,
    offer_commitment: [u8; 32],
    reservation_id: &RequestId,
    foreign_units: u64,
    lez_units: u128,
    taker_public: &PublicKey,
) -> anyhow::Result<()> {
    let body = draft.body();
    ensure!(
        body.swap_id() == &maker_btc_chat_swap_id(&offer_commitment, reservation_id)
            && body.direction() == SwapDirection::TakerSellsForeign
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
    let taker_signature = secp
        .sign_schnorr_no_aux_rand(
            &Message::from_digest(proposal.agreement_commitment),
            &Keypair::from_secret_key(&secp, &taker_secret),
        )
        .serialize();
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
            && agreement.direction() == SwapDirection::TakerSellsForeign
            && agreement.funding_terms().value_sat() == input.foreign_units
            && agreement
                .participant(Participant::Taker)
                .musig2_public_key()
                == &taker_public.serialize(),
        "persisted BTC agreement does not match the executable draft or local Taker"
    );
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
    call_local_rpc(
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

fn provision_taker_actor(
    input: &BtcTakeInput<'_>,
    final_wire: &[u8],
) -> anyhow::Result<BtcActorProvisionV1> {
    let source = ActorConfig::load_private(input.source_taker_config_file)
        .map_err(|_| anyhow::anyhow!("BTC source Taker config is unavailable"))?;
    ensure!(
        source.role() == ActorRole::Taker,
        "BTC source actor is not Taker"
    );
    let provisioned = provision_btc_taker_actor_from_config(
        &source,
        final_wire,
        input.now_unix_seconds,
        input.taker_actor_root,
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
        private_material_disclosed: false,
        actor: publish_acceptance_receipt(input, final_wire, provisioned)?,
    })
}

fn publish_acceptance_receipt(
    input: &BtcTakeInput<'_>,
    final_wire: &[u8],
    provisioned: &BtcActorProvisionV1,
) -> anyhow::Result<BtcAcceptanceActorOutput> {
    let agreement_sha256 = hex::encode(Sha256::digest(final_wire));
    ensure!(
        agreement_sha256 == hex::encode(provisioned.agreement_sha256()),
        "provisioned BTC agreement identity changed"
    );
    ensure!(
        input.acceptance_receipt_file != provisioned.config_file()
            && input.acceptance_receipt_file != provisioned.state_database(),
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
        input.acceptance_receipt_file,
        &receipt_bytes,
        MAX_TAKER_RECEIPT_BYTES,
        "BTC Taker acceptance receipt",
    )?;
    Ok(BtcAcceptanceActorOutput {
        role: provisioned.role(),
        receipt_file: input.acceptance_receipt_file.to_path_buf(),
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
    let receipt = resolved_new_path(input.acceptance_receipt_file, "BTC acceptance receipt")?;
    let actor_root = resolved_new_path(input.taker_actor_root, "BTC Taker actor root")?;
    let agreement = resolved_new_path(input.agreement_output_file, "BTC agreement output")?;
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
