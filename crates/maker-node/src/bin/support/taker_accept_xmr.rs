use std::path::{Path, PathBuf};

use anyhow::{Context as _, ensure};
use lez_bridge_protocol::{RequestId, RunId};
use lez_maker_node::{
    DeliveryOfferQueryV1, RunLocalDelivery, XmrChatActivateRequestV1, XmrChatActivateResponseV1,
    XmrChatStageARequestV1, XmrChatStageAResponseV1, call_local_chat_rpc,
};
use lez_swap_core::{Pair, SwapDirection, SwapId};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{MakerOfferId, MakerRouteV1, maker_xmr_chat_swap_id};
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES, XmrAgreementV1, XmrSwapDirectionV1,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use xmr_reference_actor::{
    ActorRole, ValidatedXmrEffectAuthorityV1, XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
    XMR_EFFECT_AUTHORITY_MAX_BYTES, XmrActorProvisionV1, XmrEffectProvisionV3,
    load_validated_xmr_effect_manifest_v3_bytes, load_validated_xmr_taker_authority_bytes,
    provision_xmr_effect_manifest_v3, provision_xmr_taker_actor_from_material,
    validate_taker_manifest_config_bytes, validate_xmr_effect_manifest_v3_projection_bytes,
};
use zeroize::Zeroizing;

use super::{
    secure_file::read_private_file,
    taker_accept::{
        MAX_TAKER_RECEIPT_BYTES, decode_sha256, normalized_absolute, publish_exact_new,
        resolved_new_path,
    },
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
    pub(crate) effect: Option<XmrEffectTakeInput<'a>>,
}

pub(crate) struct XmrEffectTakeInput<'a> {
    pub(crate) effect_authority_file: &'a Path,
    pub(crate) effect_manifest_file: &'a Path,
    pub(crate) workflow_journal: &'a Path,
    pub(crate) run_id: &'a str,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    effect_provisioning_replay: Option<bool>,
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
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct XmrAcceptanceReceiptV2 {
    activation_commitment: String,
    actor_manifest_file: PathBuf,
    actor_manifest_sha256: String,
    actor_state_database: PathBuf,
    agreement_commitment: String,
    effect_authority_file: PathBuf,
    effect_authority_sha256: String,
    effect_manifest_file: PathBuf,
    effect_manifest_sha256: String,
    offer_id: String,
    pair: Box<str>,
    reservation_id: String,
    role: ActorRole,
    run_id: String,
    schema_version: u16,
    stage_a_file: PathBuf,
    stage_a_sha256: String,
    stage_b_file: PathBuf,
    stage_b_sha256: String,
    swap_id: String,
    workflow_journal: PathBuf,
}

#[derive(Debug)]
struct XmrEffectReceiptBinding {
    swap_id: SwapId,
    swap_id_bytes: [u8; 32],
    agreement_commitment: [u8; 32],
    activation_commitment: [u8; 32],
    stage_a_file: PathBuf,
    stage_a_sha256: [u8; 32],
    stage_b_file: PathBuf,
    stage_b_sha256: [u8; 32],
    run_id: Box<str>,
    actor_manifest_file: PathBuf,
    actor_manifest_sha256: [u8; 32],
    actor_state_database: PathBuf,
    effect_manifest_file: PathBuf,
    effect_manifest_sha256: [u8; 32],
    effect_authority_file: PathBuf,
    effect_authority_sha256: [u8; 32],
    workflow_journal: PathBuf,
}

#[allow(clippy::similar_names)] // Stage-A/B receipt bindings are intentionally symmetric.
fn parse_effect_capable_receipt_bytes(bytes: &[u8]) -> anyhow::Result<XmrEffectReceiptBinding> {
    ensure!(
        !bytes.is_empty()
            && u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= MAX_TAKER_RECEIPT_BYTES,
        "XMR effect-capable acceptance receipt is oversized"
    );
    let value: Value =
        serde_json::from_slice(bytes).context("decode XMR effect-capable acceptance receipt")?;
    match value.get("schema_version").and_then(Value::as_u64) {
        Some(1) => anyhow::bail!("legacy XMR acceptance receipt is monitor-only"),
        Some(2) => {}
        _ => anyhow::bail!("unsupported XMR effect-capable acceptance receipt"),
    }
    let receipt: XmrAcceptanceReceiptV2 =
        serde_json::from_value(value).context("decode XMR acceptance receipt v2")?;
    ensure!(
        serde_json::to_vec(&receipt)? == bytes
            && receipt.schema_version == 2
            && receipt.pair.as_ref() == "monero"
            && receipt.role == ActorRole::Taker,
        "XMR acceptance receipt v2 is noncanonical or unsupported"
    );
    let _run_id = RunId::new(receipt.run_id.clone()).context("invalid XMR receipt run ID")?;
    let _offer_id =
        MakerOfferId::new(receipt.offer_id.clone()).context("invalid XMR receipt offer ID")?;
    let _reservation_id = RequestId::new(receipt.reservation_id.clone())
        .context("invalid XMR receipt reservation ID")?;
    for path in [
        &receipt.actor_manifest_file,
        &receipt.actor_state_database,
        &receipt.effect_manifest_file,
        &receipt.effect_authority_file,
        &receipt.workflow_journal,
        &receipt.stage_a_file,
        &receipt.stage_b_file,
    ] {
        ensure!(
            normalized_absolute(path),
            "XMR acceptance receipt v2 path is invalid"
        );
    }
    ensure!(
        receipt.actor_manifest_file != receipt.effect_manifest_file
            && receipt.actor_manifest_file != receipt.effect_authority_file
            && receipt.actor_manifest_file != receipt.actor_state_database
            && receipt.actor_manifest_file != receipt.workflow_journal
            && receipt.effect_manifest_file != receipt.effect_authority_file
            && receipt.effect_manifest_file != receipt.actor_state_database
            && receipt.effect_manifest_file != receipt.workflow_journal
            && receipt.effect_authority_file != receipt.actor_state_database
            && receipt.effect_authority_file != receipt.workflow_journal
            && receipt.actor_state_database != receipt.workflow_journal,
        "XMR acceptance receipt v2 authority paths overlap"
    );
    let digest = |value: &str, label: &str| -> anyhow::Result<[u8; 32]> {
        let decoded = decode_canonical_sha256(value, label)?;
        ensure!(
            decoded.iter().any(|byte| *byte != 0),
            "receipt {label} digest is zero"
        );
        Ok(decoded)
    };
    let swap_id_bytes = decode_swap_id(&receipt.swap_id)?;
    let swap_id = SwapId::new(receipt.swap_id).context("invalid XMR receipt swap ID")?;
    let stage_a_sha256 = digest(&receipt.stage_a_sha256, "XMR Stage A")?;
    let stage_b_sha256 = digest(&receipt.stage_b_sha256, "XMR Stage B")?;
    Ok(XmrEffectReceiptBinding {
        swap_id,
        swap_id_bytes,
        agreement_commitment: digest(&receipt.agreement_commitment, "XMR agreement commitment")?,
        activation_commitment: digest(&receipt.activation_commitment, "XMR activation commitment")?,
        stage_a_file: receipt.stage_a_file,
        stage_a_sha256,
        stage_b_file: receipt.stage_b_file,
        stage_b_sha256,
        run_id: receipt.run_id.into_boxed_str(),
        actor_manifest_file: receipt.actor_manifest_file,
        actor_manifest_sha256: digest(&receipt.actor_manifest_sha256, "XMR actor manifest")?,
        actor_state_database: receipt.actor_state_database,
        effect_manifest_file: receipt.effect_manifest_file,
        effect_manifest_sha256: digest(&receipt.effect_manifest_sha256, "XMR effect manifest")?,
        effect_authority_file: receipt.effect_authority_file,
        effect_authority_sha256: digest(&receipt.effect_authority_sha256, "XMR effect authority")?,
        workflow_journal: receipt.workflow_journal,
    })
}

/// Digest-pinned receipt-v2 bytes awaiting semantic validation under role lock.
pub(crate) struct XmrTakerEffectReceiptSelector {
    binding: XmrEffectReceiptBinding,
    actor_manifest_bytes: Zeroizing<Vec<u8>>,
    effect_manifest_bytes: Zeroizing<Vec<u8>>,
    effect_authority_bytes: Zeroizing<Vec<u8>>,
}

impl XmrTakerEffectReceiptSelector {
    pub(crate) fn swap_id(&self) -> &SwapId {
        &self.binding.swap_id
    }

    pub(crate) fn state_database(&self) -> &Path {
        &self.binding.actor_state_database
    }

    pub(crate) fn workflow_journal(&self) -> &Path {
        &self.binding.workflow_journal
    }

    pub(crate) fn run_id(&self) -> &str {
        &self.binding.run_id
    }

    pub(crate) fn validate_authority(&self) -> anyhow::Result<ValidatedXmrEffectAuthorityV1> {
        validate_xmr_effect_manifest_v3_projection_bytes(
            &self.effect_manifest_bytes,
            &self.actor_manifest_bytes,
            ActorRole::Taker,
            &self.binding.run_id,
        )
        .context("bind XMR receipt v2 to its legacy actor manifest")?;
        let legacy = load_validated_xmr_taker_authority_bytes(&self.actor_manifest_bytes)
            .context("semantically validate receipt-v2 XMR application authority")?;
        ensure!(
            legacy.published_stage_a() == self.binding.stage_a_file
                && legacy.stage_a_sha256() == self.binding.stage_a_sha256
                && legacy.published_stage_b() == self.binding.stage_b_file
                && legacy.stage_b_sha256() == self.binding.stage_b_sha256
                && legacy.agreement_commitment() == self.binding.agreement_commitment
                && legacy.activation_commitment() == self.binding.activation_commitment,
            "XMR receipt v2 duplicates differ from validated application authority"
        );
        let authority = load_validated_xmr_effect_manifest_v3_bytes(
            &self.effect_manifest_bytes,
            &self.effect_authority_bytes,
            ActorRole::Taker,
            &self.binding.run_id,
        )
        .context("semantically validate receipt-v2 XMR effect authority")?;
        ensure!(
            authority.role() == ActorRole::Taker
                && authority.swap_id() == self.binding.swap_id_bytes
                && authority.agreement_commitment() == self.binding.agreement_commitment
                && authority.activation_commitment() == self.binding.activation_commitment
                && authority.run_id() == self.binding.run_id.as_ref()
                && authority.adaptor_journal() == self.binding.actor_state_database
                && authority.workflow_journal() == self.binding.workflow_journal,
            "XMR receipt v2 differs from validated effect authority"
        );
        Ok(authority)
    }
}

pub(crate) fn load_xmr_taker_effect_receipt_selector(
    path: &Path,
) -> anyhow::Result<XmrTakerEffectReceiptSelector> {
    let receipt_bytes = read_private_file(
        path,
        MAX_TAKER_RECEIPT_BYTES,
        "XMR Taker effect-capable acceptance receipt",
    )?;
    let binding = parse_effect_capable_receipt_bytes(&receipt_bytes)?;
    let actor_manifest_bytes = read_private_file(
        &binding.actor_manifest_file,
        XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "receipt-v2 XMR actor manifest",
    )?;
    ensure!(
        Sha256::digest(actor_manifest_bytes.as_slice()).as_slice()
            == binding.actor_manifest_sha256.as_slice(),
        "receipt-v2 XMR actor manifest digest changed"
    );
    validate_taker_manifest_config_bytes(
        &actor_manifest_bytes,
        binding.swap_id_bytes,
        &binding.actor_state_database,
    )
    .context("structurally bind receipt v2 to XMR Taker actor manifest")?;

    let effect_manifest_bytes = read_private_file(
        &binding.effect_manifest_file,
        XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "receipt-v2 XMR effect manifest",
    )?;
    ensure!(
        Sha256::digest(effect_manifest_bytes.as_slice()).as_slice()
            == binding.effect_manifest_sha256.as_slice(),
        "receipt-v2 XMR effect manifest digest changed"
    );
    validate_xmr_effect_manifest_v3_projection_bytes(
        &effect_manifest_bytes,
        &actor_manifest_bytes,
        ActorRole::Taker,
        &binding.run_id,
    )
    .context("structurally bind receipt v2 schema-v3 projection")?;

    let effect_authority_bytes = read_private_file(
        &binding.effect_authority_file,
        XMR_EFFECT_AUTHORITY_MAX_BYTES,
        "receipt-v2 XMR effect authority",
    )?;
    ensure!(
        Sha256::digest(effect_authority_bytes.as_slice()).as_slice()
            == binding.effect_authority_sha256.as_slice(),
        "receipt-v2 XMR effect authority digest changed"
    );
    Ok(XmrTakerEffectReceiptSelector {
        binding,
        actor_manifest_bytes,
        effect_manifest_bytes,
        effect_authority_bytes,
    })
}

/// Structurally validated receipt selector whose manifest bytes are digest-pinned.
///
/// The CLI must acquire the role-state kernel lock before semantically validating
/// `manifest_bytes`; this split prevents a validate-then-lock TOCTOU window.
pub(crate) struct XmrTakerReceiptSelector {
    swap_id: SwapId,
    swap_id_bytes: [u8; 32],
    state_database: PathBuf,
    stage_a_file: PathBuf,
    stage_a_sha256: [u8; 32],
    stage_b_file: PathBuf,
    stage_b_sha256: [u8; 32],
    agreement_commitment: [u8; 32],
    activation_commitment: [u8; 32],
    manifest_bytes: Zeroizing<Vec<u8>>,
}

impl XmrTakerReceiptSelector {
    pub(crate) fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    pub(crate) fn state_database(&self) -> &Path {
        &self.state_database
    }

    pub(crate) fn manifest_bytes(&self) -> &[u8] {
        &self.manifest_bytes
    }

    pub(crate) fn receipt_matches(
        &self,
        authority: &xmr_reference_actor::ValidatedXmrTakerAuthorityV2,
    ) -> bool {
        authority.swap_id() == self.swap_id_bytes
            && authority.state_database() == self.state_database
            && authority.published_stage_a() == self.stage_a_file
            && authority.stage_a_sha256() == self.stage_a_sha256
            && authority.published_stage_b() == self.stage_b_file
            && authority.stage_b_sha256() == self.stage_b_sha256
            && authority.agreement_commitment() == self.agreement_commitment
            && authority.activation_commitment() == self.activation_commitment
    }
}

pub(crate) fn load_xmr_taker_receipt_selector(
    path: &Path,
) -> anyhow::Result<XmrTakerReceiptSelector> {
    let bytes = read_private_file(
        path,
        MAX_TAKER_RECEIPT_BYTES,
        "XMR Taker acceptance receipt",
    )?;
    let receipt: XmrAcceptanceReceiptV1 =
        serde_json::from_slice(&bytes).context("decode XMR Taker acceptance receipt")?;
    ensure!(
        serde_json::to_vec(&receipt)? == bytes.as_slice()
            && receipt.schema_version == 1
            && receipt.pair.as_ref() == "monero"
            && receipt.role == ActorRole::Taker,
        "XMR Taker acceptance receipt is noncanonical or unsupported"
    );
    ensure!(
        normalized_absolute(path)
            && normalized_absolute(&receipt.stage_a_file)
            && normalized_absolute(&receipt.stage_b_file)
            && normalized_absolute(&receipt.actor_manifest_file)
            && normalized_absolute(&receipt.actor_state_database),
        "XMR Taker acceptance receipt paths must be normalized and absolute"
    );
    let expected_manifest =
        decode_canonical_sha256(&receipt.actor_manifest_sha256, "XMR actor manifest")?;
    let manifest_bytes = read_private_file(
        &receipt.actor_manifest_file,
        XMR_ACTOR_PROVISION_MANIFEST_MAX_BYTES,
        "receipt-bound XMR Taker actor manifest",
    )?;
    ensure!(
        Sha256::digest(&manifest_bytes).as_slice() == expected_manifest.as_slice(),
        "receipt-bound XMR Taker actor manifest digest changed"
    );
    let swap_id_bytes = decode_swap_id(&receipt.swap_id)?;
    validate_taker_manifest_config_bytes(
        &manifest_bytes,
        swap_id_bytes,
        &receipt.actor_state_database,
    )
    .context("bind receipt to XMR Taker actor manifest")?;
    Ok(XmrTakerReceiptSelector {
        swap_id: SwapId::new(receipt.swap_id.clone())?,
        swap_id_bytes,
        state_database: receipt.actor_state_database,
        stage_a_file: receipt.stage_a_file,
        stage_a_sha256: decode_canonical_sha256(&receipt.stage_a_sha256, "XMR Stage A")?,
        stage_b_file: receipt.stage_b_file,
        stage_b_sha256: decode_canonical_sha256(&receipt.stage_b_sha256, "XMR Stage B")?,
        agreement_commitment: decode_canonical_sha256(
            &receipt.agreement_commitment,
            "XMR agreement commitment",
        )?,
        activation_commitment: decode_canonical_sha256(
            &receipt.activation_commitment,
            "XMR activation commitment",
        )?,
        manifest_bytes,
    })
}

fn decode_swap_id(value: &str) -> anyhow::Result<[u8; 32]> {
    decode_canonical_sha256(value, "XMR swap ID")
}

fn decode_canonical_sha256(value: &str, label: &str) -> anyhow::Result<[u8; 32]> {
    let decoded = decode_sha256(value, label)?;
    ensure!(
        hex::encode(decoded) == value,
        "receipt {label} digest is noncanonical"
    );
    Ok(decoded)
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
    let effect = input
        .effect
        .as_ref()
        .map(|effect| provision_effect_authority(&provisioned, effect))
        .transpose()?;
    let actor = publish_acceptance_receipt(
        &input,
        &offer_id,
        &reservation_id,
        &provisioned,
        effect.as_ref(),
    )?;
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

fn provision_effect_authority(
    provisioned: &XmrActorProvisionV1,
    input: &XmrEffectTakeInput<'_>,
) -> anyhow::Result<XmrEffectProvisionV3> {
    let effect = provision_xmr_effect_manifest_v3(
        provisioned.manifest_file(),
        ActorRole::Taker,
        input.effect_authority_file,
        input.workflow_journal,
        input.run_id,
        input.effect_manifest_file,
    )
    .context("provision accepted XMR Taker effect authority")?;
    ensure!(
        effect.role() == provisioned.role()
            && effect.swap_id() == provisioned.swap_id()
            && effect.agreement_commitment() == provisioned.agreement_commitment()
            && effect.activation_commitment() == provisioned.activation_commitment(),
        "provisioned XMR effect authority changed accepted application"
    );
    Ok(effect)
}

fn publish_acceptance_receipt(
    input: &XmrTakeInput<'_>,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    provisioned: &XmrActorProvisionV1,
    effect: Option<&XmrEffectProvisionV3>,
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
    let receipt_bytes = match effect {
        None => serde_json::to_vec(&receipt).context("encode XMR acceptance receipt v1")?,
        Some(effect) => serde_json::to_vec(&XmrAcceptanceReceiptV2 {
            activation_commitment: receipt.activation_commitment,
            actor_manifest_file: receipt.actor_manifest_file,
            actor_manifest_sha256: receipt.actor_manifest_sha256,
            actor_state_database: receipt.actor_state_database,
            agreement_commitment: receipt.agreement_commitment,
            effect_authority_file: effect.effect_authority_file().to_path_buf(),
            effect_authority_sha256: hex::encode(effect.effect_authority_sha256()),
            effect_manifest_file: effect.manifest_file().to_path_buf(),
            effect_manifest_sha256: hex::encode(effect.manifest_sha256()),
            offer_id: receipt.offer_id,
            pair: receipt.pair,
            reservation_id: receipt.reservation_id,
            role: receipt.role,
            run_id: effect.run_id().to_owned(),
            schema_version: 2,
            stage_a_file: receipt.stage_a_file,
            stage_a_sha256: receipt.stage_a_sha256,
            stage_b_file: receipt.stage_b_file,
            stage_b_sha256: receipt.stage_b_sha256,
            swap_id: receipt.swap_id,
            workflow_journal: effect.workflow_journal().to_path_buf(),
        })
        .context("encode XMR acceptance receipt v2")?,
    };
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
        effect_provisioning_replay: effect.map(XmrEffectProvisionV3::was_replay),
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
    if let Some(effect) = &input.effect {
        ensure!(
            receipt != effect.effect_authority_file
                && receipt != effect.effect_manifest_file
                && receipt != effect.workflow_journal,
            "XMR acceptance receipt must be separate from effect authority and workflow state"
        );
    }
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
#[cfg(test)]
mod receipt_v2_tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::parse_effect_capable_receipt_bytes;

    fn legacy_receipt_v1() -> Value {
        json!({
            "schema_version": 1,
            "pair": "monero",
            "role": "taker",
            "offer_id": "m5-xmr-offer-1",
            "reservation_id": "m5-xmr-reservation-1",
            "swap_id": "81".repeat(32),
            "agreement_commitment": "82".repeat(32),
            "activation_commitment": "83".repeat(32),
            "stage_a_file": "/var/lib/lez/taker/stage-a.bin",
            "stage_a_sha256": "84".repeat(32),
            "stage_b_file": "/var/lib/lez/taker/stage-b.bin",
            "stage_b_sha256": "85".repeat(32),
            "actor_manifest_file": "/var/lib/lez/taker/actor-provision.json",
            "actor_manifest_sha256": "86".repeat(32),
            "actor_state_database": "/var/lib/lez/taker/adaptor.sqlite"
        })
    }

    #[test]
    fn receipt_v2_is_the_only_effect_capable_taker_authority() {
        let mut v2 = legacy_receipt_v1();
        let object = v2.as_object_mut().expect("receipt object");
        object.insert("schema_version".to_owned(), json!(2));
        object.insert("run_id".to_owned(), json!("m5-xmr-taker-effect-run-1"));
        object.insert(
            "effect_manifest_file".to_owned(),
            json!("/var/lib/lez/taker/actor-effect-provision-v3.json"),
        );
        object.insert("effect_manifest_sha256".to_owned(), json!("87".repeat(32)));
        object.insert(
            "effect_authority_file".to_owned(),
            json!("/var/lib/lez/taker/effect-authority-v1.json"),
        );
        object.insert("effect_authority_sha256".to_owned(), json!("88".repeat(32)));
        object.insert(
            "workflow_journal".to_owned(),
            json!("/var/lib/lez/taker/xmr-workflow.sqlite"),
        );

        let bytes = serde_json::to_vec(&v2).expect("encode canonical receipt v2");
        let effect = parse_effect_capable_receipt_bytes(&bytes)
            .expect("receipt v2 authorizes the exact claim/refund workflow");
        assert_eq!(effect.run_id.as_ref(), "m5-xmr-taker-effect-run-1");
        assert_eq!(
            effect.effect_manifest_file,
            Path::new("/var/lib/lez/taker/actor-effect-provision-v3.json")
        );
        assert_eq!(effect.effect_manifest_sha256, [0x87; 32]);
        assert_eq!(
            effect.effect_authority_file,
            Path::new("/var/lib/lez/taker/effect-authority-v1.json")
        );
        assert_eq!(effect.effect_authority_sha256, [0x88; 32]);
        assert_eq!(
            effect.workflow_journal,
            Path::new("/var/lib/lez/taker/xmr-workflow.sqlite")
        );

        let legacy = serde_json::to_vec(&legacy_receipt_v1()).expect("encode legacy receipt v1");
        let error = parse_effect_capable_receipt_bytes(&legacy)
            .expect_err("receipt v1 is monitor-only and authorizes neither claim nor refund");
        assert_eq!(
            error.to_string(),
            "legacy XMR acceptance receipt is monitor-only"
        );
    }
}
