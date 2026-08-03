#![doc(hidden)]
//! Reusable owner-private ZEC taker acceptance support.

use std::{
    fmt,
    fs::{self, File},
    io::Write as _,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Component, Path, PathBuf},
};

use crate::{
    AuthenticatedOfferRefV1, DeliveryOfferQueryV1, RunLocalDelivery, ZecChatCompleteRequestV1,
    ZecChatCompleteResponseV1, ZecChatProposalV1, ZecChatProposeRequestV1, call_local_chat_rpc,
    secure_file::{load_raw_secret, read_private_file},
};
use anyhow::{Context as _, ensure};
use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, Participant, SwapDirection, UnixSeconds};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{MakerOfferId, MakerRouteV1, maker_zec_chat_session_id};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, MAX_ZEC_AGREEMENT_RECORD_BYTES, ZecAgreementDraftV1,
    ZecMakerAgreementProposalV1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tempfile::NamedTempFile;
use zec_reference_actor::{
    ActorConfig, ActorRole, ZecActorProvisionV1, provision_zec_taker_actor_from_chat,
    provision_zec_taker_actor_from_config,
};

pub const MAX_TAKER_RECEIPT_BYTES: u64 = 16 * 1024;

pub struct ZecTakeInput<'a> {
    pub delivery: Option<&'a RunLocalDelivery>,
    pub expected_maker: &'a PublicKey,
    pub now_unix_seconds: u64,
    pub offer_id: &'a str,
    pub chat_socket: &'a Path,
    pub reservation_id: &'a str,
    pub foreign_units: u64,
    pub unsigned_draft_file: &'a Path,
    pub source_taker_config_file: &'a Path,
    pub taker_actor_root: &'a Path,
    pub acceptance_receipt_file: &'a Path,
    pub taker_signing_key_file: &'a Path,
    pub agreement_output_file: &'a Path,
}

impl fmt::Debug for ZecTakeInput<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ZecTakeInput")
            .field("delivery_configured", &self.delivery.is_some())
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

#[derive(Debug, Serialize)]
pub struct ReplayOutput {
    pub proposal: bool,
    pub completion: bool,
    pub agreement_file: bool,
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

/// Runs ZEC acceptance using authenticated Delivery discovery.
pub async fn take_zec(input: ZecTakeInput<'_>) -> anyhow::Result<ZecAcceptanceOutput> {
    take_zec_inner(input, None, None).await
}

/// Runs ZEC acceptance using one exact caller-retained authenticated offer.
///
/// The retained signed envelope is revalidated against the selected route,
/// trusted timestamp, maker, amount, draft, and Chat proposal before use.
pub async fn take_zec_with_authenticated_offer(
    input: ZecTakeInput<'_>,
    authenticated_offer: &AuthenticatedOfferRefV1,
) -> anyhow::Result<ZecAcceptanceOutput> {
    take_zec_inner(input, Some(authenticated_offer), None).await
}

/// Runs exact-offer ZEC acceptance using an already authenticated actor config.
///
/// This entry point preserves the caller's digest-pinned actor authority across
/// acceptance and provisioning instead of reopening the source config path.
pub async fn take_zec_with_authenticated_offer_and_actor_config(
    input: ZecTakeInput<'_>,
    authenticated_offer: &AuthenticatedOfferRefV1,
    source_actor_config: &ActorConfig,
) -> anyhow::Result<ZecAcceptanceOutput> {
    take_zec_inner(input, Some(authenticated_offer), Some(source_actor_config)).await
}

async fn take_zec_inner(
    input: ZecTakeInput<'_>,
    authenticated_offer: Option<&AuthenticatedOfferRefV1>,
    source_actor_config: Option<&ActorConfig>,
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
            authenticated_offer,
            source_actor_config,
        )
        .await;
    }

    let route = MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez)?;
    let discovered = if authenticated_offer.is_none() {
        Some(
            input
                .delivery
                .context("fresh ZEC acceptance requires Delivery")?
                .discover(&DeliveryOfferQueryV1::for_route(
                    route,
                    input.now_unix_seconds,
                ))
                .await?
                .into_iter()
                .find(|candidate| candidate.offer().id() == &offer_id)
                .context("selected ZEC offer is unavailable, expired, or not authentic")?,
        )
    } else {
        None
    };
    let selected = authenticated_offer
        .or(discovered.as_ref())
        .context("authenticated ZEC offer is unavailable")?;
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

    let propose_request_id = derived_request_id(&reservation_id, b"propose")?;
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
    .await?;
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
    authenticated_offer: Option<&AuthenticatedOfferRefV1>,
    source_actor_config: Option<&ActorConfig>,
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
    if let Some(selected) = authenticated_offer {
        let route = MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez)?;
        let expected_lez_units = validate_authenticated_offer(input, &offer_id, route, selected)?;
        validate_draft_offer_bindings(
            input,
            &reservation_id,
            selected,
            expected_lez_units,
            &validated_draft,
            &taker_public,
        )?;
    }
    let receipt_exists = if authenticated_offer.is_some() {
        completed_receipt_is_present_and_valid(input, agreement.application_swap_id(), &final_wire)?
    } else {
        false
    };
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
            request_id: derived_request_id(&reservation_id, b"complete")?,
            offer_id: offer_id.clone(),
            expected_offer_revision: 2,
            reservation_id: reservation_id.clone(),
            final_agreement_wire: final_wire.to_vec(),
        },
    )
    .await?;
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
    source_actor_config: Option<&ActorConfig>,
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
    let final_wire = agreement.encode_wire()?;
    let agreement_file_was_replay = publish_exact_new(
        input.agreement_output_file,
        &final_wire,
        MAX_ZEC_AGREEMENT_RECORD_BYTES as u64,
        "countersigned ZEC agreement",
    )?;
    let provisioned = provision_taker_actor(input, &final_wire, source_actor_config)?;
    let complete_request_id = derived_request_id(&reservation_id, b"complete")?;
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
    .await?;
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

pub fn decode_sha256(value: &str, label: &str) -> anyhow::Result<[u8; 32]> {
    let decoded = hex::decode(value).with_context(|| format!("decode receipt {label} digest"))?;
    decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("receipt {label} digest has the wrong length"))
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
            && body.direction() == SwapDirection::TakerSellsLez
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

pub fn resolved_new_path(path: &Path, label: &str) -> anyhow::Result<PathBuf> {
    ensure!(
        normalized_absolute(path),
        "{label} path must be normalized and absolute"
    );
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .with_context(|| format!("{label} path needs a parent directory"))?;
    let file_name = path
        .file_name()
        .with_context(|| format!("{label} path needs a file name"))?;
    let parent_metadata =
        fs::symlink_metadata(parent).with_context(|| format!("inspect {label} parent"))?;
    ensure!(
        parent_metadata.file_type().is_dir()
            && parent_metadata.uid() == rustix::process::geteuid().as_raw()
            && parent_metadata.permissions().mode() & 0o7777 == 0o700,
        "{label} parent must be an owner-owned mode-0700 real directory"
    );
    Ok(fs::canonicalize(parent)
        .with_context(|| format!("resolve {label} parent"))?
        .join(file_name))
}

#[must_use]
pub fn normalized_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn provision_taker_actor(
    input: &ZecTakeInput<'_>,
    final_wire: &[u8],
    source_actor_config: Option<&ActorConfig>,
) -> anyhow::Result<ZecActorProvisionV1> {
    let accepted_at = UnixSeconds::new(input.now_unix_seconds);
    let provisioned = match source_actor_config {
        Some(source) => provision_zec_taker_actor_from_config(
            source,
            final_wire,
            accepted_at,
            input.taker_actor_root,
        ),
        None => provision_zec_taker_actor_from_chat(
            input.source_taker_config_file,
            final_wire,
            accepted_at,
            input.taker_actor_root,
        ),
    }
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

fn derived_request_id(reservation_id: &RequestId, label: &[u8]) -> anyhow::Result<RequestId> {
    let mut digest = Sha256::new();
    digest.update(b"lez-atomic-swaps/zec-taker-chat-request/v1\0");
    digest.update(reservation_id.as_str().as_bytes());
    digest.update([0]);
    digest.update(label);
    RequestId::new(hex::encode(digest.finalize())).map_err(Into::into)
}

pub fn publish_exact_new(
    path: &Path,
    bytes: &[u8],
    max_bytes: u64,
    label: &'static str,
) -> anyhow::Result<bool> {
    ensure!(path.is_absolute(), "{label} path must be absolute");
    match fs::symlink_metadata(path) {
        Ok(_) => return validate_existing_output(path, bytes, max_bytes, label).map(|()| true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("inspect {label} path")),
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .with_context(|| format!("{label} needs a parent directory"))?;
    let parent_metadata =
        fs::symlink_metadata(parent).with_context(|| format!("inspect {label} parent"))?;
    ensure!(
        parent_metadata.file_type().is_dir()
            && parent_metadata.uid() == rustix::process::geteuid().as_raw()
            && parent_metadata.permissions().mode() & 0o7777 == 0o700,
        "{label} parent must be an owner-owned mode-0700 real directory"
    );
    let mut temporary =
        NamedTempFile::new_in(parent).with_context(|| format!("create temporary {label}"))?;
    temporary
        .as_file_mut()
        .write_all(bytes)
        .with_context(|| format!("write temporary {label}"))?;
    temporary
        .as_file_mut()
        .sync_all()
        .with_context(|| format!("sync temporary {label}"))?;
    match temporary.persist_noclobber(path) {
        Ok(file) => {
            file.sync_all()
                .with_context(|| format!("sync persisted {label}"))?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .with_context(|| format!("sync {label} directory"))?;
            validate_existing_output(path, bytes, max_bytes, label)?;
            Ok(false)
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            validate_existing_output(path, bytes, max_bytes, label)?;
            Ok(true)
        }
        Err(error) => Err(error.error).with_context(|| format!("publish {label} without clobber")),
    }
}

fn validate_existing_output(
    path: &Path,
    expected: &[u8],
    max_bytes: u64,
    label: &'static str,
) -> anyhow::Result<()> {
    let actual = read_private_file(path, max_bytes, label)?;
    ensure!(
        actual.as_slice() == expected,
        "{label} already exists with different bytes"
    );
    Ok(())
}
