use std::{
    fs::{self, File},
    io::Write as _,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context as _, ensure};
use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    DeliveryOfferQueryV1, RunLocalDelivery, ZecChatCompleteRequestV1, ZecChatCompleteResponseV1,
    ZecChatProposalV1, ZecChatProposeRequestV1, call_local_rpc,
};
use lez_swap_core::{Pair, Participant, SwapDirection, UnixSeconds};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{MakerOfferId, MakerRouteV1};
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
};

use super::secure_file::{load_raw_secret, read_private_file};

pub(crate) const MAX_TAKER_RECEIPT_BYTES: u64 = 16 * 1024;

pub(crate) struct ZecTakeInput<'a> {
    pub(crate) delivery: Option<&'a RunLocalDelivery>,
    pub(crate) expected_maker: &'a PublicKey,
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
pub(crate) struct ZecAcceptanceOutput {
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

#[derive(Serialize)]
pub(crate) struct ReplayOutput {
    pub(crate) proposal: bool,
    pub(crate) completion: bool,
    pub(crate) agreement_file: bool,
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
pub(crate) struct ZecAcceptanceReceiptV1 {
    schema_version: u16,
    swap_id: Box<str>,
    role: ActorRole,
    agreement_sha256: String,
    actor_config_file: PathBuf,
    actor_config_sha256: String,
    actor_state_database: PathBuf,
}

pub(crate) async fn take_zec(input: ZecTakeInput<'_>) -> anyhow::Result<ZecAcceptanceOutput> {
    validate_acceptance_paths(&input)?;
    let offer_id = MakerOfferId::new(input.offer_id)?;
    let reservation_id = RequestId::new(input.reservation_id)?;
    ensure!(input.foreign_units > 0, "ZEC principal must be nonzero");

    match fs::symlink_metadata(input.agreement_output_file) {
        Ok(_) => return resume_persisted_zec(&input, offer_id, reservation_id).await,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect persisted ZEC agreement"),
    }

    let route = MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez)?;
    let selected = input
        .delivery
        .context("fresh ZEC acceptance requires Delivery")?
        .discover(&DeliveryOfferQueryV1::for_route(
            route,
            input.now_unix_seconds,
        ))
        .await?
        .into_iter()
        .find(|candidate| candidate.offer().id() == &offer_id)
        .context("selected ZEC offer is unavailable, expired, or not authentic")?;
    let expected_lez_units = selected.offer().quote_foreign_amount(input.foreign_units)?;
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
    ensure!(
        validated_draft.taker_zcash_key() == &taker_public
            && validated_draft.maker_zcash_key() == input.expected_maker,
        "unsigned draft participant identities do not match the local taker and pinned maker"
    );

    let propose_request_id = derived_request_id(&reservation_id, b"propose")?;
    let proposal: ZecChatProposalV1 = call_local_rpc(
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
    )
    .await
}

async fn resume_persisted_zec(
    input: &ZecTakeInput<'_>,
    offer_id: MakerOfferId,
    reservation_id: RequestId,
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
    let provisioned = provision_taker_actor(input, &final_wire)?;
    let completion: ZecChatCompleteResponseV1 = call_local_rpc(
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
    let provisioned = provision_taker_actor(input, &final_wire)?;
    let complete_request_id = derived_request_id(&reservation_id, b"complete")?;
    let completion: ZecChatCompleteResponseV1 = call_local_rpc(
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

pub(crate) fn load_taker_actor_from_receipt(path: &Path) -> anyhow::Result<ActorConfig> {
    let bytes = read_private_file(path, MAX_TAKER_RECEIPT_BYTES, "Taker acceptance receipt")?;
    let receipt: ZecAcceptanceReceiptV1 =
        serde_json::from_slice(&bytes).context("decode Taker acceptance receipt")?;
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

pub(crate) fn decode_sha256(value: &str, label: &str) -> anyhow::Result<[u8; 32]> {
    let decoded = hex::decode(value).with_context(|| format!("decode receipt {label} digest"))?;
    decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("receipt {label} digest has the wrong length"))
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

pub(crate) fn resolved_new_path(path: &Path, label: &str) -> anyhow::Result<PathBuf> {
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

pub(crate) fn normalized_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn provision_taker_actor(
    input: &ZecTakeInput<'_>,
    final_wire: &[u8],
) -> anyhow::Result<ZecActorProvisionV1> {
    let provisioned = provision_zec_taker_actor_from_chat(
        input.source_taker_config_file,
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

fn publish_acceptance_receipt(
    input: &ZecTakeInput<'_>,
    final_wire: &[u8],
    provisioned: &ZecActorProvisionV1,
) -> anyhow::Result<ZecAcceptanceActorOutput> {
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
    let receipt = ZecAcceptanceReceiptV1 {
        schema_version: 1,
        swap_id: provisioned.swap_id().as_str().into(),
        role: provisioned.role(),
        agreement_sha256,
        actor_config_file: provisioned.config_file().to_path_buf(),
        actor_config_sha256: hex::encode(provisioned.config_sha256()),
        actor_state_database: provisioned.state_database().to_path_buf(),
    };
    let receipt_bytes = serde_json::to_vec(&receipt).context("encode Taker acceptance receipt")?;
    let receipt_replay = publish_exact_new(
        input.acceptance_receipt_file,
        &receipt_bytes,
        MAX_TAKER_RECEIPT_BYTES,
        "Taker acceptance receipt",
    )?;
    Ok(ZecAcceptanceActorOutput {
        role: provisioned.role(),
        receipt_file: input.acceptance_receipt_file.to_path_buf(),
        receipt_sha256: hex::encode(Sha256::digest(&receipt_bytes)),
        provisioning_replay: provisioned.was_replay(),
        receipt_replay,
    })
}

fn derived_request_id(reservation_id: &RequestId, label: &[u8]) -> anyhow::Result<RequestId> {
    let mut digest = Sha256::new();
    digest.update(b"lez-atomic-swaps/zec-taker-chat-request/v1\0");
    digest.update(reservation_id.as_str().as_bytes());
    digest.update([0]);
    digest.update(label);
    RequestId::new(hex::encode(digest.finalize())).map_err(Into::into)
}

pub(crate) fn publish_exact_new(
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
