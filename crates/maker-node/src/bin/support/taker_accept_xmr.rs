use std::path::{Path, PathBuf};

use anyhow::{Context as _, ensure};
use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    DeliveryOfferQueryV1, RunLocalDelivery, XmrChatActivateRequestV1, XmrChatActivateResponseV1,
    XmrChatStageARequestV1, XmrChatStageAResponseV1, call_local_chat_rpc,
};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{MakerOfferId, MakerRouteV1, maker_xmr_chat_swap_id};
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES, XmrAgreementV1, XmrSwapDirectionV1,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use xmr_reference_actor::{
    ActorRole, XmrActorProvisionV1, provision_xmr_taker_actor_from_material,
};

use super::{
    secure_file::read_private_file,
    taker_accept::{MAX_TAKER_RECEIPT_BYTES, publish_exact_new, resolved_new_path},
};

pub(crate) struct XmrTakeInput<'a> {
    pub(crate) delivery: Option<&'a RunLocalDelivery>,
    pub(crate) now_unix_seconds: u64,
    pub(crate) offer_id: &'a str,
    pub(crate) chat_socket: &'a Path,
    pub(crate) reservation_id: &'a str,
    pub(crate) foreign_units: u64,
    pub(crate) stage_a_file: &'a Path,
    pub(crate) activation_file: &'a Path,
    pub(crate) source_taker_root: &'a Path,
    pub(crate) taker_public_packet: &'a Path,
    pub(crate) maker_public_packet: &'a Path,
    pub(crate) taker_role_journal: &'a Path,
    pub(crate) taker_actor_root: &'a Path,
    pub(crate) acceptance_receipt_file: &'a Path,
}

#[derive(Serialize)]
pub(crate) struct XmrAcceptanceOutput {
    schema_version: u16,
    offer_id: String,
    offer_revision: u64,
    reservation_id: String,
    swap_id: String,
    agreement_commitment: String,
    activation_commitment: String,
    replay: XmrReplayOutput,
    private_material_disclosed: bool,
    actor: XmrAcceptanceActorOutput,
}

#[derive(Serialize)]
struct XmrReplayOutput {
    stage_a: bool,
    activation: bool,
}

#[derive(Serialize)]
struct XmrAcceptanceActorOutput {
    role: ActorRole,
    receipt_file: PathBuf,
    receipt_sha256: String,
    provisioning_replay: bool,
    receipt_replay: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct XmrAcceptanceReceiptV1 {
    schema_version: u16,
    pair: Box<str>,
    role: ActorRole,
    offer_id: String,
    reservation_id: String,
    swap_id: String,
    agreement_commitment: String,
    activation_commitment: String,
    stage_a_file: PathBuf,
    stage_a_sha256: String,
    stage_b_file: PathBuf,
    stage_b_sha256: String,
    actor_manifest_file: PathBuf,
    actor_manifest_sha256: String,
    actor_state_database: PathBuf,
}

#[allow(clippy::too_many_lines)] // Keeps the two RPC commits and receipt ordering visible together.
pub(crate) async fn take_xmr(input: XmrTakeInput<'_>) -> anyhow::Result<XmrAcceptanceOutput> {
    validate_acceptance_paths(&input)?;
    let offer_id = MakerOfferId::new(input.offer_id)?;
    let reservation_id = RequestId::new(input.reservation_id)?;
    ensure!(input.foreign_units > 0, "XMR principal must be nonzero");
    let stage_a_wire = read_private_file(
        input.stage_a_file,
        u64::try_from(MAX_XMR_AGREEMENT_WIRE_BYTES).unwrap_or(u64::MAX),
        "signed XMR Stage A",
    )?;
    let agreement = XmrAgreementV1::from_wire(&stage_a_wire)
        .context("validate canonical dual-signed XMR Stage A")?;
    validate_public_stage_a(&input, &agreement)?;

    let stage_a_was_replay = if let Some(delivery) = input.delivery {
        let selected = delivery
            .discover(&DeliveryOfferQueryV1::for_route(
                MakerRouteV1::new(Pair::Monero, SwapDirection::TakerSellsLez)?,
                input.now_unix_seconds,
            ))
            .await?
            .into_iter()
            .find(|candidate| candidate.offer().id() == &offer_id)
            .context("selected XMR offer is unavailable, expired, or not authentic")?;
        let lez_units = selected.offer().quote_foreign_amount(input.foreign_units)?;
        ensure!(
            agreement.body().swap_id()
                == maker_xmr_chat_swap_id(&selected.commitment(), &reservation_id)
                && agreement.body().lez().amount() == lez_units,
            "XMR Stage A is not bound to the selected Delivery offer and quote"
        );
        let staged: XmrChatStageAResponseV1 = call_local_chat_rpc(
            input.chat_socket,
            "xmr_chat_stage_a_v1",
            &XmrChatStageARequestV1 {
                schema_version: 1,
                request_id: derived_request_id(&reservation_id, b"stage-a")?,
                offer_id: offer_id.clone(),
                expected_offer_revision: 1,
                reservation_id: reservation_id.clone(),
                foreign_units: input.foreign_units,
                signed_offer_envelope: selected.signed_envelope().to_vec(),
                stage_a_wire: stage_a_wire.to_vec(),
            },
        )
        .await?;
        ensure!(
            staged.schema_version == 1
                && staged.offer_revision == 2
                && staged.reservation_id == reservation_id
                && staged.lez_units == lez_units
                && staged.swap_id.as_ref() == hex::encode(agreement.body().swap_id())
                && staged.agreement_commitment == agreement.agreement_commitment(),
            "Maker XMR Stage-A result changed the reservation, quote, or agreement"
        );
        staged.was_replay
    } else {
        true
    };

    // Ordering is deliberate: the actor root is the local crash latch proving
    // that this exact Stage A returned durably before activation was attempted.
    let provisioned = provision_taker_actor(&input)?;
    ensure!(
        provisioned.swap_id() == agreement.body().swap_id()
            && provisioned.agreement_commitment() == agreement.agreement_commitment(),
        "provisioned XMR actor changed Stage A"
    );
    let activation_wire = read_private_file(
        input.activation_file,
        u64::try_from(MAX_XMR_ACTIVATION_WIRE_BYTES).unwrap_or(u64::MAX),
        "signed XMR Stage B",
    )?;
    let activated: XmrChatActivateResponseV1 = call_local_chat_rpc(
        input.chat_socket,
        "xmr_chat_activate_v1",
        &XmrChatActivateRequestV1 {
            schema_version: 1,
            request_id: derived_request_id(&reservation_id, b"activate")?,
            offer_id: offer_id.clone(),
            expected_offer_revision: 2,
            reservation_id: reservation_id.clone(),
            activation_wire: activation_wire.to_vec(),
        },
    )
    .await?;
    let swap_id = hex::encode(provisioned.swap_id());
    ensure!(
        activated.schema_version == 1
            && activated.offer_revision == 3
            && activated.swap_id.as_ref() == swap_id
            && activated.activation_commitment == provisioned.activation_commitment(),
        "Maker XMR activation result changed the swap or Stage B"
    );
    let actor = publish_acceptance_receipt(&input, &offer_id, &reservation_id, &provisioned)?;
    Ok(XmrAcceptanceOutput {
        schema_version: 1,
        offer_id: offer_id.as_str().to_owned(),
        offer_revision: activated.offer_revision,
        reservation_id: reservation_id.as_str().to_owned(),
        swap_id,
        agreement_commitment: hex::encode(provisioned.agreement_commitment()),
        activation_commitment: hex::encode(provisioned.activation_commitment()),
        replay: XmrReplayOutput {
            stage_a: stage_a_was_replay,
            activation: activated.was_replay,
        },
        private_material_disclosed: false,
        actor,
    })
}

fn validate_public_stage_a(
    input: &XmrTakeInput<'_>,
    agreement: &XmrAgreementV1,
) -> anyhow::Result<()> {
    let body = agreement.body();
    ensure!(
        body.direction() == XmrSwapDirectionV1::TakerSellsLez
            && body.monero().amount_piconero() == input.foreign_units,
        "XMR Stage A changed the direction or principal"
    );
    Ok(())
}

fn provision_taker_actor(input: &XmrTakeInput<'_>) -> anyhow::Result<XmrActorProvisionV1> {
    let provisioned = provision_xmr_taker_actor_from_material(
        input.source_taker_root,
        input.taker_public_packet,
        input.maker_public_packet,
        input.stage_a_file,
        input.activation_file,
        input.taker_role_journal,
        input.taker_actor_root,
    )
    .map_err(|_| anyhow::anyhow!("provision accepted XMR Taker actor"))?;
    ensure!(
        provisioned.role() == ActorRole::Taker,
        "provisioned XMR actor has the wrong role"
    );
    Ok(provisioned)
}

fn publish_acceptance_receipt(
    input: &XmrTakeInput<'_>,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    provisioned: &XmrActorProvisionV1,
) -> anyhow::Result<XmrAcceptanceActorOutput> {
    let receipt = XmrAcceptanceReceiptV1 {
        schema_version: 1,
        pair: "monero".into(),
        role: provisioned.role(),
        offer_id: offer_id.as_str().to_owned(),
        reservation_id: reservation_id.as_str().to_owned(),
        swap_id: hex::encode(provisioned.swap_id()),
        agreement_commitment: hex::encode(provisioned.agreement_commitment()),
        activation_commitment: hex::encode(provisioned.activation_commitment()),
        stage_a_file: provisioned.stage_a_file().to_path_buf(),
        stage_a_sha256: hex::encode(provisioned.stage_a_sha256()),
        stage_b_file: provisioned.stage_b_file().to_path_buf(),
        stage_b_sha256: hex::encode(provisioned.stage_b_sha256()),
        actor_manifest_file: provisioned.manifest_file().to_path_buf(),
        actor_manifest_sha256: hex::encode(provisioned.manifest_sha256()),
        actor_state_database: provisioned.state_database().to_path_buf(),
    };
    let receipt_bytes = serde_json::to_vec(&receipt).context("encode XMR acceptance receipt")?;
    let receipt_replay = publish_exact_new(
        input.acceptance_receipt_file,
        &receipt_bytes,
        MAX_TAKER_RECEIPT_BYTES,
        "XMR Taker acceptance receipt",
    )?;
    Ok(XmrAcceptanceActorOutput {
        role: provisioned.role(),
        receipt_file: input.acceptance_receipt_file.to_path_buf(),
        receipt_sha256: hex::encode(Sha256::digest(&receipt_bytes)),
        provisioning_replay: provisioned.was_replay(),
        receipt_replay,
    })
}

fn validate_acceptance_paths(input: &XmrTakeInput<'_>) -> anyhow::Result<()> {
    let receipt = resolved_new_path(input.acceptance_receipt_file, "XMR acceptance receipt")?;
    let actor_root = resolved_new_path(input.taker_actor_root, "XMR Taker actor root")?;
    ensure!(
        !receipt.starts_with(&actor_root) && receipt != input.taker_role_journal,
        "XMR acceptance receipt must be outside actor authority and state"
    );
    Ok(())
}

fn derived_request_id(reservation_id: &RequestId, label: &[u8]) -> anyhow::Result<RequestId> {
    let mut digest = Sha256::new();
    digest.update(b"lez-atomic-swaps/xmr-taker-chat-request/v1\0");
    digest.update(reservation_id.as_str().as_bytes());
    digest.update([0]);
    digest.update(label);
    RequestId::new(hex::encode(digest.finalize())).map_err(Into::into)
}
