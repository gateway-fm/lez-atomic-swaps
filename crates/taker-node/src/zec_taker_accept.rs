#![doc(hidden)]
//! The Taker Node's acceptance of a Zcash agreement it reserved at take time
//! (`taker_service/zec_dynamic.rs`): it proposes the Maker's draft, countersigns
//! the Maker's proposal, provisions its actor and writes its receipt.

use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

use crate::{
    AuthenticatedOfferRefV1, ZecChatCompleteRequestV1, ZecChatCompleteResponseV1,
    ZecChatProposalV1, ZecChatProposeRequestV1,
    acceptance_files::{
        MAX_TAKER_RECEIPT_BYTES, ReplayOutput, decode_sha256, derived_request_id,
        normalized_absolute, publish_exact_new, resolved_new_path,
    },
    call_local_chat_rpc,
    secure_file::{load_raw_secret, read_private_file, read_private_file_snapshot},
};
use anyhow::{Context as _, ensure};
use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, Participant, SwapDirection, SwapId, UnixSeconds};
use lez_swap_store::{MakerOfferId, MakerRouteV1, maker_zec_chat_session_id};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, MAX_ZEC_AGREEMENT_RECORD_BYTES, ZecAgreementDraftV1,
    ZecMakerAgreementProposalV1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zec_reference_actor::{
    ActorConfig, ActorRole, ZecActorProvisionV1, provision_zec_taker_actor_from_config,
};

pub struct ZecTakeInput<'a> {
    pub expected_maker: &'a PublicKey,
    pub now_unix_seconds: u64,
    pub direction: SwapDirection,
    pub offer_id: &'a str,
    pub chat_socket: &'a Path,
    pub reservation_id: &'a str,
    pub foreign_units: u64,
    pub unsigned_draft_file: &'a Path,
    pub taker_actor_root: &'a Path,
    pub acceptance_receipt_file: &'a Path,
    pub taker_signing_key_file: &'a Path,
    pub agreement_output_file: &'a Path,
}

impl fmt::Debug for ZecTakeInput<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ZecTakeInput")
            .field("offer_id", &self.offer_id)
            .field("reservation_id", &self.reservation_id)
            .field("foreign_units", &self.foreign_units)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
pub struct ZecAcceptanceOutput {
    schema_version: u16,
    offer_id: String,
    offer_revision: u64,
    reservation_id: String,
    swap_id: Box<str>,
    agreement_file: PathBuf,
    agreement_sha256: String,
    replay: ReplayOutput,
    private_material_disclosed: bool,
    actor: ZecAcceptanceActorOutput,
}

impl fmt::Debug for ZecAcceptanceOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ZecAcceptanceOutput")
            .field("schema_version", &self.schema_version)
            .field("offer_id", &self.offer_id)
            .field("offer_revision", &self.offer_revision)
            .field("reservation_id", &self.reservation_id)
            .field("swap_id", &self.swap_id)
            .field("replay", &self.replay)
            .field(
                "private_material_disclosed",
                &self.private_material_disclosed,
            )
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
struct ZecAcceptanceActorOutput {
    role: ActorRole,
    receipt_file: PathBuf,
    receipt_sha256: String,
    provisioning_replay: bool,
    receipt_replay: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ZecAcceptanceReceiptV1 {
    schema_version: u16,
    swap_id: Box<str>,
    role: ActorRole,
    agreement_sha256: String,
    actor_config_file: PathBuf,
    actor_config_sha256: String,
    actor_state_database: PathBuf,
}

/// The stable name of the Maker's refusal of a ZEC `TakerSellsLez` take at its
/// capacity, as its message carries it.
pub const ZEC_TAKER_SELLS_LEZ_CAPACITY_REACHED: &str = "zec_taker_sells_lez_capacity_reached";

/// The Maker refused the take: it already holds as many open ZEC
/// `TakerSellsLez` swaps as its one-at-a-time LEZ claims settle inside the
/// profile's margin. Nothing was locked; the offer may be taken again once
/// one of the Maker's swaps ends.
#[derive(Debug, thiserror::Error)]
#[error(
    "zec_taker_sells_lez_capacity_reached: the Maker holds as many open ZEC TakerSellsLez swaps as it can settle; nothing was locked, take again later"
)]
pub struct ZecMakerCapacityReached;

/// The Maker's capacity refusal as [`ZecMakerCapacityReached`]; any other
/// failure unchanged.
fn named_maker_refusal(error: anyhow::Error) -> anyhow::Error {
    if format!("{error:#}").contains(ZEC_TAKER_SELLS_LEZ_CAPACITY_REACHED) {
        anyhow::Error::new(ZecMakerCapacityReached)
    } else {
        error
    }
}

/// Runs exact-offer ZEC acceptance for the offer the Node authenticated and
/// the actor template it wrote at take time.
///
/// The caller's digest-pinned actor authority is kept across acceptance and
/// provisioning: the source config path is not opened again.
pub async fn take_zec_with_authenticated_offer_and_actor_config(
    input: ZecTakeInput<'_>,
    selected: &AuthenticatedOfferRefV1,
    source_actor_config: &ActorConfig,
) -> anyhow::Result<ZecAcceptanceOutput> {
    validate_acceptance_paths(&input)?;
    let offer_id = MakerOfferId::new(input.offer_id)?;
    let reservation_id = RequestId::new(input.reservation_id)?;
    ensure!(input.foreign_units > 0, "ZEC principal must be nonzero");

    if persisted_agreement_exists(input.agreement_output_file)? {
        return resume_persisted_zec(
            &input,
            offer_id,
            reservation_id,
            selected,
            source_actor_config,
        )
        .await;
    }

    let route = selected_zec_route(&input, selected)?;
    let expected_lez_units = validate_authenticated_offer(&input, &offer_id, route, selected)?;
    let draft_wire = read_private_file(
        input.unsigned_draft_file,
        MAX_ZEC_AGREEMENT_RECORD_BYTES as u64,
        "unsigned ZEC agreement draft",
    )?;
    let now = UnixSeconds::new(input.now_unix_seconds);
    let validated_draft = ZecAgreementDraftV1::from_wire_at(&draft_wire, now)
        .context("validate unsigned ZEC agreement draft")?;
    let taker_secret_material =
        load_raw_secret(input.taker_signing_key_file, "taker agreement key")?;
    let taker_secret = SecretKey::from_slice(taker_secret_material.as_ref())
        .context("validate taker agreement key")?;
    let taker_public = PublicKey::from_secret_key(&Secp256k1::signing_only(), &taker_secret);
    validate_draft_offer_bindings(
        &input,
        &reservation_id,
        selected,
        expected_lez_units,
        &validated_draft,
        &taker_public,
    )?;
    // Before the Maker is asked to sign: the Taker's template must hold the
    // swap secret (its digest is checked once the agreement is countersigned).
    require_taker_claim_preimage(source_actor_config, None)?;

    let propose_request_id = derived_request_id(&reservation_id, "zec", b"propose")?;
    let proposal: ZecChatProposalV1 = call_local_chat_rpc(
        input.chat_socket,
        "zec_chat_propose_v1",
        &ZecChatProposeRequestV1 {
            schema_version: 1,
            request_id: propose_request_id,
            offer_id: offer_id.clone(),
            expected_offer_revision: 1,
            reservation_id: reservation_id.clone(),
            foreign_units: input.foreign_units,
            signed_offer_envelope: selected.signed_envelope().to_vec(),
            unsigned_draft_wire: draft_wire.to_vec(),
        },
    )
    .await
    .map_err(named_maker_refusal)?;
    let maker_proposal = ZecMakerAgreementProposalV1::from_wire_at(&proposal.proposal_wire, now)
        .context("validate maker-signed ZEC proposal")?;
    ensure!(
        proposal.schema_version == 1
            && proposal.offer_revision == 2
            && proposal.reservation_id == reservation_id
            && proposal.lez_units == expected_lez_units
            && proposal.maker_identity.as_slice() == input.expected_maker.serialize()
            && proposal.taker_identity.as_slice() == taker_public.serialize()
            && proposal.agreement_commitment == *maker_proposal.commitment()
            && maker_proposal.body() == validated_draft.body(),
        "maker proposal changed the selected offer, identities, or executable draft"
    );
    complete_zec(
        &input,
        offer_id,
        reservation_id,
        proposal,
        maker_proposal,
        taker_secret,
        source_actor_config,
    )
    .await
}

/// The Taker holds the swap secret in both directions: it locks first and
/// claims the Maker-funded leg first, revealing the preimage (its Node made
/// it before the reservation). A source template without the claim
/// preimage, or with one that does not hash to the agreement's digest, is
/// refused before anything is signed or provisioned.
fn require_taker_claim_preimage(
    source: &ActorConfig,
    secret_digest: Option<&[u8; 32]>,
) -> anyhow::Result<()> {
    let material = source
        .load_activate_material()
        .context("load the Taker template's activation material")?;
    let preimage = material.claim_preimage().context(
        "the Taker's actor template carries no claim preimage; the Taker holds the swap secret in both directions",
    )?;
    if let Some(secret_digest) = secret_digest {
        let digest: [u8; 32] = Sha256::digest(preimage.expose_secret()).into();
        ensure!(
            &digest == secret_digest,
            "the Taker's claim preimage does not match the agreement's secret digest"
        );
    }
    Ok(())
}

fn selected_zec_route(
    input: &ZecTakeInput<'_>,
    selected: &AuthenticatedOfferRefV1,
) -> anyhow::Result<MakerRouteV1> {
    let route = selected.offer().route();
    ensure!(
        route.pair() == Pair::Zcash && route.direction() == input.direction,
        "authenticated ZEC offer is unavailable"
    );
    Ok(route)
}

fn persisted_agreement_exists(path: &Path) -> anyhow::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("inspect persisted ZEC agreement"),
    }
}

async fn resume_persisted_zec(
    input: &ZecTakeInput<'_>,
    offer_id: MakerOfferId,
    reservation_id: RequestId,
    selected: &AuthenticatedOfferRefV1,
    source_actor_config: &ActorConfig,
) -> anyhow::Result<ZecAcceptanceOutput> {
    let final_wire = read_private_file(
        input.agreement_output_file,
        MAX_ZEC_AGREEMENT_RECORD_BYTES as u64,
        "persisted countersigned ZEC agreement",
    )?;
    let now = UnixSeconds::new(input.now_unix_seconds);
    let accepted = AcceptedZecAgreementV1::accept_wire_at(&final_wire, now, Participant::Taker, 0)
        .context("validate persisted countersigned ZEC agreement")?;
    let agreement = accepted.agreement();
    let draft_wire = read_private_file(
        input.unsigned_draft_file,
        MAX_ZEC_AGREEMENT_RECORD_BYTES as u64,
        "unsigned ZEC agreement draft",
    )?;
    let validated_draft = ZecAgreementDraftV1::from_wire_at(&draft_wire, now)
        .context("validate unsigned ZEC agreement draft for persisted retry")?;
    let taker_secret_material =
        load_raw_secret(input.taker_signing_key_file, "taker agreement key")?;
    let mut taker_secret = SecretKey::from_slice(taker_secret_material.as_ref())
        .context("validate taker agreement key")?;
    let taker_public = PublicKey::from_secret_key(&Secp256k1::signing_only(), &taker_secret);
    taker_secret.non_secure_erase();
    ensure!(
        validated_draft.taker_zcash_key() == &taker_public
            && validated_draft.maker_zcash_key() == input.expected_maker
            && validated_draft.zcash_amount_zatoshis() == input.foreign_units
            && validated_draft.body() == agreement.record().body(),
        "persisted agreement does not match the local taker, pinned maker, or executable draft"
    );
    let route = selected.offer().route();
    ensure!(
        route.pair() == Pair::Zcash,
        "authenticated ZEC offer is unavailable"
    );
    let expected_lez_units = validate_authenticated_offer(input, &offer_id, route, selected)?;
    validate_draft_offer_bindings(
        input,
        &reservation_id,
        selected,
        expected_lez_units,
        &validated_draft,
        &taker_public,
    )?;
    let receipt_exists = completed_receipt_is_present_and_valid(
        input,
        agreement.application_swap_id(),
        &final_wire,
    )?;
    let provisioned = provision_taker_actor(input, &final_wire, source_actor_config)?;
    if receipt_exists {
        return completed_offline_replay(
            input,
            &offer_id,
            &reservation_id,
            agreement.application_swap_id(),
            &final_wire,
            &provisioned,
        );
    }
    let completion: ZecChatCompleteResponseV1 = call_local_chat_rpc(
        input.chat_socket,
        "zec_chat_complete_v1",
        &ZecChatCompleteRequestV1 {
            schema_version: 1,
            request_id: derived_request_id(&reservation_id, "zec", b"complete")?,
            offer_id: offer_id.clone(),
            expected_offer_revision: 2,
            reservation_id: reservation_id.clone(),
            final_agreement_wire: final_wire.to_vec(),
        },
    )
    .await
    .map_err(named_maker_refusal)?;
    ensure!(
        completion.schema_version == 1
            && completion.offer_revision == 3
            && completion.swap_id.as_ref() == agreement.application_swap_id(),
        "maker completion result does not match the persisted countersigned agreement"
    );
    Ok(ZecAcceptanceOutput {
        schema_version: 1,
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
        private_material_disclosed: false,
        actor: publish_acceptance_receipt(input, &final_wire, &provisioned)?,
    })
}

async fn complete_zec(
    input: &ZecTakeInput<'_>,
    offer_id: MakerOfferId,
    reservation_id: RequestId,
    proposal: ZecChatProposalV1,
    maker_proposal: ZecMakerAgreementProposalV1,
    mut taker_secret: SecretKey,
    source_actor_config: &ActorConfig,
) -> anyhow::Result<ZecAcceptanceOutput> {
    let taker_signature = Secp256k1::signing_only()
        .sign_ecdsa(
            &Message::from_digest(proposal.agreement_commitment),
            &taker_secret,
        )
        .serialize_compact();
    taker_secret.non_secure_erase();
    let agreement = maker_proposal
        .complete_at(taker_signature, UnixSeconds::new(input.now_unix_seconds))
        .context("countersign maker ZEC proposal")?;
    require_taker_claim_preimage(source_actor_config, Some(agreement.secret_digest()))?;
    let final_wire = agreement.encode_wire()?;
    let agreement_file_was_replay = publish_exact_new(
        input.agreement_output_file,
        &final_wire,
        MAX_ZEC_AGREEMENT_RECORD_BYTES as u64,
        "countersigned ZEC agreement",
    )?;
    let provisioned = provision_taker_actor(input, &final_wire, source_actor_config)?;
    let complete_request_id = derived_request_id(&reservation_id, "zec", b"complete")?;
    let completion: ZecChatCompleteResponseV1 = call_local_chat_rpc(
        input.chat_socket,
        "zec_chat_complete_v1",
        &ZecChatCompleteRequestV1 {
            schema_version: 1,
            request_id: complete_request_id,
            offer_id: offer_id.clone(),
            expected_offer_revision: proposal.offer_revision,
            reservation_id: reservation_id.clone(),
            final_agreement_wire: final_wire.clone(),
        },
    )
    .await
    .map_err(named_maker_refusal)?;
    ensure!(
        completion.schema_version == 1
            && completion.offer_revision == 3
            && completion.swap_id.as_ref() == agreement.application_swap_id(),
        "maker completion result does not match the countersigned agreement"
    );

    Ok(ZecAcceptanceOutput {
        schema_version: 1,
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
        private_material_disclosed: false,
        actor: publish_acceptance_receipt(input, &final_wire, &provisioned)?,
    })
}

pub fn load_taker_actor_from_receipt(path: &Path) -> anyhow::Result<ActorConfig> {
    let bytes = read_private_file(path, MAX_TAKER_RECEIPT_BYTES, "Taker acceptance receipt")?;
    load_taker_actor_from_receipt_bytes(path, &bytes)
}

/// Loads one receipt selected by service-owned authority and binds its paths.
pub(crate) fn load_taker_actor_from_receipt_for_monitor(
    path: &Path,
    expected_actor_root: &Path,
    expected_swap_id: &SwapId,
    expected_sha256: [u8; 32],
    expected_device: u64,
    expected_inode: u64,
) -> anyhow::Result<ActorConfig> {
    let snapshot =
        read_private_file_snapshot(path, MAX_TAKER_RECEIPT_BYTES, "Taker acceptance receipt")?;
    let identity = snapshot.identity();
    ensure!(
        Sha256::digest(snapshot.bytes()).as_slice() == expected_sha256
            && identity.device() == expected_device
            && identity.inode() == expected_inode,
        "Taker acceptance receipt identity changed"
    );
    let receipt: ZecAcceptanceReceiptV1 =
        serde_json::from_slice(snapshot.bytes()).context("decode Taker acceptance receipt")?;
    ensure!(
        receipt.swap_id.as_ref() == expected_swap_id.as_str()
            && receipt.actor_config_file.starts_with(expected_actor_root)
            && receipt
                .actor_state_database
                .starts_with(expected_actor_root),
        "Taker acceptance receipt does not match service authority"
    );
    let config = load_taker_actor_from_receipt_bytes(path, snapshot.bytes())?;
    ensure!(
        config.swap_id() == expected_swap_id
            && config.role_state_db().starts_with(expected_actor_root)
            && config.bridge_journal_db().starts_with(expected_actor_root),
        "receipt-bound Taker actor is outside service authority"
    );
    Ok(config)
}

fn load_taker_actor_from_receipt_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<ActorConfig> {
    let receipt: ZecAcceptanceReceiptV1 =
        serde_json::from_slice(bytes).context("decode Taker acceptance receipt")?;
    ensure!(
        receipt.schema_version == 1 && receipt.role == ActorRole::Taker,
        "Taker acceptance receipt has an unsupported role or version"
    );
    ensure!(
        normalized_absolute(path)
            && normalized_absolute(&receipt.actor_config_file)
            && normalized_absolute(&receipt.actor_state_database),
        "Taker acceptance receipt paths must be normalized and absolute"
    );
    let config_sha256 = decode_sha256(&receipt.actor_config_sha256, "actor config")?;
    let agreement_sha256 = decode_sha256(&receipt.agreement_sha256, "agreement")?;
    let config = ActorConfig::load_private_pinned_sha256(&receipt.actor_config_file, config_sha256)
        .context("load receipt-bound Taker actor config")?;
    ensure!(
        config.role() == ActorRole::Taker
            && config.swap_id().as_str() == receipt.swap_id.as_ref()
            && config.role_state_db() == receipt.actor_state_database
            && config.signed_agreement_sha256() == agreement_sha256,
        "receipt-bound Taker actor semantics changed"
    );
    Ok(config)
}

fn validate_authenticated_offer(
    input: &ZecTakeInput<'_>,
    offer_id: &MakerOfferId,
    route: MakerRouteV1,
    selected: &AuthenticatedOfferRefV1,
) -> anyhow::Result<u128> {
    selected
        .offer()
        .validate()
        .map_err(|_| anyhow::anyhow!("authenticated ZEC offer binding is invalid"))?;
    ensure!(
        selected.maker_identity().as_slice() == input.expected_maker.serialize()
            && selected.offer().id() == offer_id
            && selected.offer().route() == route
            && selected.offer().created_at_unix_seconds() <= input.now_unix_seconds
            && input.now_unix_seconds < selected.offer().expires_at_unix_seconds(),
        "authenticated ZEC offer binding is invalid"
    );
    selected
        .offer()
        .quote_foreign_amount(input.foreign_units)
        .map_err(|_| anyhow::anyhow!("authenticated ZEC offer binding is invalid"))
}

fn validate_draft_offer_bindings(
    input: &ZecTakeInput<'_>,
    reservation_id: &RequestId,
    selected: &AuthenticatedOfferRefV1,
    expected_lez_units: u128,
    draft: &lez_zec_swap_sdk::ValidatedZecAgreementDraftV1,
    taker_public: &PublicKey,
) -> anyhow::Result<()> {
    let body = draft.body();
    let transcript = body.transcript();
    ensure!(
        draft.taker_zcash_key() == taker_public
            && draft.maker_zcash_key() == input.expected_maker
            && draft.zcash_amount_zatoshis() == input.foreign_units
            && body.direction() == selected.offer().route().direction()
            && body.direction() == input.direction
            && body.lez_terms().amount() == expected_lez_units
            && transcript.session_id() == &maker_zec_chat_session_id(reservation_id)
            && transcript.offer_commitment() == &selected.commitment()
            && transcript.expires_at_unix_seconds() == selected.offer().expires_at_unix_seconds(),
        "unsigned ZEC draft does not match authenticated offer authority"
    );
    Ok(())
}

fn validate_acceptance_paths(input: &ZecTakeInput<'_>) -> anyhow::Result<()> {
    let receipt = resolved_new_path(input.acceptance_receipt_file, "acceptance receipt")?;
    let actor_root = resolved_new_path(input.taker_actor_root, "Taker actor root")?;
    let agreement = resolved_new_path(input.agreement_output_file, "agreement output")?;
    ensure!(
        receipt != agreement && !receipt.starts_with(&actor_root),
        "acceptance receipt must be outside actor authority and agreement paths"
    );
    Ok(())
}

fn provision_taker_actor(
    input: &ZecTakeInput<'_>,
    final_wire: &[u8],
    source_actor_config: &ActorConfig,
) -> anyhow::Result<ZecActorProvisionV1> {
    let provisioned = provision_zec_taker_actor_from_config(
        source_actor_config,
        final_wire,
        UnixSeconds::new(input.now_unix_seconds),
        input.taker_actor_root,
    )
    .context("provision accepted Taker actor")?;
    ensure!(
        provisioned.role() == ActorRole::Taker,
        "provisioned actor has the wrong role"
    );
    Ok(provisioned)
}

fn completed_receipt_is_present_and_valid(
    input: &ZecTakeInput<'_>,
    swap_id: &str,
    final_wire: &[u8],
) -> anyhow::Result<bool> {
    match fs::symlink_metadata(input.acceptance_receipt_file) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("inspect completed Taker receipt"),
    }
    let bytes = read_private_file(
        input.acceptance_receipt_file,
        MAX_TAKER_RECEIPT_BYTES,
        "completed Taker acceptance receipt",
    )?;
    let receipt: ZecAcceptanceReceiptV1 =
        serde_json::from_slice(&bytes).context("decode completed Taker acceptance receipt")?;
    let agreement_sha256 = hex::encode(Sha256::digest(final_wire));
    ensure!(
        receipt.schema_version == 1
            && receipt.role == ActorRole::Taker
            && receipt.swap_id.as_ref() == swap_id
            && receipt.agreement_sha256 == agreement_sha256
            && normalized_absolute(&receipt.actor_config_file)
            && normalized_absolute(&receipt.actor_state_database)
            && receipt
                .actor_config_file
                .starts_with(input.taker_actor_root)
            && receipt
                .actor_state_database
                .starts_with(input.taker_actor_root),
        "completed Taker acceptance receipt binding is invalid"
    );
    let config = load_taker_actor_from_receipt_bytes(input.acceptance_receipt_file, &bytes)?;
    ensure!(
        config.role() == ActorRole::Taker
            && config.swap_id().as_str() == swap_id
            && config.role_state_db() == receipt.actor_state_database
            && config.signed_agreement_sha256()
                == decode_sha256(&receipt.agreement_sha256, "agreement")?,
        "completed Taker actor receipt binding is invalid"
    );
    Ok(true)
}

fn completed_offline_replay(
    input: &ZecTakeInput<'_>,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    swap_id: &str,
    final_wire: &[u8],
    provisioned: &ZecActorProvisionV1,
) -> anyhow::Result<ZecAcceptanceOutput> {
    ensure!(
        provisioned.was_replay() && provisioned.swap_id().as_str() == swap_id,
        "completed Taker actor replay binding is invalid"
    );
    let receipt_bytes = acceptance_receipt_bytes(input, final_wire, provisioned)?;
    let actual = read_private_file(
        input.acceptance_receipt_file,
        MAX_TAKER_RECEIPT_BYTES,
        "completed Taker acceptance receipt",
    )?;
    ensure!(
        actual.as_slice() == receipt_bytes.as_slice(),
        "completed Taker acceptance receipt binding is invalid"
    );
    let config = load_taker_actor_from_receipt_bytes(input.acceptance_receipt_file, &actual)?;
    ensure!(
        config.swap_id().as_str() == swap_id
            && config.role() == ActorRole::Taker
            && config.role_state_db() == provisioned.state_database()
            && config.signed_agreement_sha256() == provisioned.agreement_sha256(),
        "completed Taker actor receipt binding is invalid"
    );
    Ok(ZecAcceptanceOutput {
        schema_version: 1,
        offer_id: offer_id.as_str().to_owned(),
        offer_revision: 3,
        reservation_id: reservation_id.as_str().to_owned(),
        swap_id: swap_id.into(),
        agreement_file: input.agreement_output_file.to_path_buf(),
        agreement_sha256: hex::encode(Sha256::digest(final_wire)),
        replay: ReplayOutput {
            proposal: true,
            completion: true,
            agreement_file: true,
        },
        private_material_disclosed: false,
        actor: acceptance_actor_output(input, &receipt_bytes, provisioned, true),
    })
}

fn acceptance_receipt_bytes(
    input: &ZecTakeInput<'_>,
    final_wire: &[u8],
    provisioned: &ZecActorProvisionV1,
) -> anyhow::Result<Vec<u8>> {
    let agreement_sha256 = hex::encode(Sha256::digest(final_wire));
    ensure!(
        agreement_sha256 == hex::encode(provisioned.agreement_sha256()),
        "provisioned agreement identity changed"
    );
    ensure!(
        input.acceptance_receipt_file != provisioned.config_file()
            && input.acceptance_receipt_file != provisioned.state_database(),
        "acceptance receipt aliases actor state"
    );
    serde_json::to_vec(&ZecAcceptanceReceiptV1 {
        schema_version: 1,
        swap_id: provisioned.swap_id().as_str().into(),
        role: provisioned.role(),
        agreement_sha256,
        actor_config_file: provisioned.config_file().to_path_buf(),
        actor_config_sha256: hex::encode(provisioned.config_sha256()),
        actor_state_database: provisioned.state_database().to_path_buf(),
    })
    .context("encode Taker acceptance receipt")
}

fn publish_acceptance_receipt(
    input: &ZecTakeInput<'_>,
    final_wire: &[u8],
    provisioned: &ZecActorProvisionV1,
) -> anyhow::Result<ZecAcceptanceActorOutput> {
    let receipt_bytes = acceptance_receipt_bytes(input, final_wire, provisioned)?;
    let receipt_replay = publish_exact_new(
        input.acceptance_receipt_file,
        &receipt_bytes,
        MAX_TAKER_RECEIPT_BYTES,
        "Taker acceptance receipt",
    )?;
    Ok(acceptance_actor_output(
        input,
        &receipt_bytes,
        provisioned,
        receipt_replay,
    ))
}

fn acceptance_actor_output(
    input: &ZecTakeInput<'_>,
    receipt_bytes: &[u8],
    provisioned: &ZecActorProvisionV1,
    receipt_replay: bool,
) -> ZecAcceptanceActorOutput {
    ZecAcceptanceActorOutput {
        role: provisioned.role(),
        receipt_file: input.acceptance_receipt_file.to_path_buf(),
        receipt_sha256: hex::encode(Sha256::digest(receipt_bytes)),
        provisioning_replay: provisioned.was_replay(),
        receipt_replay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_makers_capacity_refusal_is_typed_for_the_taker() {
        // As `call_local_chat_rpc` reports the Maker's refusal of a propose
        // or a completion.
        let refused = anyhow::anyhow!(
            "local RPC error -32019: zec_taker_sells_lez_capacity_reached: the Maker holds 3 open ZEC TakerSellsLez swaps, its capacity is 3"
        );
        let named = named_maker_refusal(refused);
        assert!(named.downcast_ref::<ZecMakerCapacityReached>().is_some());
        assert!(
            named
                .to_string()
                .starts_with(ZEC_TAKER_SELLS_LEZ_CAPACITY_REACHED)
        );
        let other =
            named_maker_refusal(anyhow::anyhow!("local RPC error -32018: offer unavailable"));
        assert!(other.downcast_ref::<ZecMakerCapacityReached>().is_none());
    }
}
