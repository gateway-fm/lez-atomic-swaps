use std::{
    fs::{self, File},
    io::Write as _,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, ensure};
use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    DeliveryOfferQueryV1, RunLocalDelivery, ZecChatCompleteRequestV1, ZecChatCompleteResponseV1,
    ZecChatProposalV1, ZecChatProposeRequestV1, call_local_rpc,
};
use lez_swap_core::{Pair, SwapDirection, UnixSeconds};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{MakerOfferId, MakerRouteV1};
use lez_zec_swap_sdk::{
    MAX_ZEC_AGREEMENT_RECORD_BYTES, ZecAgreementDraftV1, ZecMakerAgreementProposalV1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tempfile::NamedTempFile;

use super::secure_file::{load_raw_secret, read_private_file};

pub(crate) struct ZecTakeInput<'a> {
    pub(crate) delivery: &'a RunLocalDelivery,
    pub(crate) expected_maker: &'a PublicKey,
    pub(crate) now_unix_seconds: u64,
    pub(crate) offer_id: &'a str,
    pub(crate) chat_socket: &'a Path,
    pub(crate) reservation_id: &'a str,
    pub(crate) foreign_units: u64,
    pub(crate) unsigned_draft_file: &'a Path,
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
}

#[derive(Serialize)]
struct ReplayOutput {
    proposal: bool,
    completion: bool,
    agreement_file: bool,
}

pub(crate) async fn take_zec(input: ZecTakeInput<'_>) -> anyhow::Result<ZecAcceptanceOutput> {
    let offer_id = MakerOfferId::new(input.offer_id)?;
    let reservation_id = RequestId::new(input.reservation_id)?;
    ensure!(input.foreign_units > 0, "ZEC principal must be nonzero");

    let route = MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez)?;
    let selected = input
        .delivery
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
    let agreement_file_was_replay = publish_exact_new(input.agreement_output_file, &final_wire)?;

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

fn publish_exact_new(path: &Path, bytes: &[u8]) -> anyhow::Result<bool> {
    ensure!(path.is_absolute(), "agreement output path must be absolute");
    match fs::symlink_metadata(path) {
        Ok(_) => return validate_existing_output(path, bytes).map(|()| true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect agreement output path"),
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("agreement output needs a parent directory")?;
    let parent_metadata =
        fs::symlink_metadata(parent).context("inspect agreement output parent")?;
    ensure!(
        parent_metadata.file_type().is_dir()
            && parent_metadata.uid() == rustix::process::geteuid().as_raw()
            && parent_metadata.permissions().mode() & 0o7777 == 0o700,
        "agreement output parent must be an owner-owned mode-0700 real directory"
    );
    let mut temporary = NamedTempFile::new_in(parent).context("create temporary agreement file")?;
    temporary
        .as_file_mut()
        .write_all(bytes)
        .context("write temporary agreement file")?;
    temporary
        .as_file_mut()
        .sync_all()
        .context("sync temporary agreement file")?;
    match temporary.persist_noclobber(path) {
        Ok(file) => {
            file.sync_all().context("sync persisted agreement file")?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .context("sync agreement output directory")?;
            validate_existing_output(path, bytes)?;
            Ok(false)
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            validate_existing_output(path, bytes)?;
            Ok(true)
        }
        Err(error) => Err(error.error).context("publish agreement file without clobber"),
    }
}

fn validate_existing_output(path: &Path, expected: &[u8]) -> anyhow::Result<()> {
    let actual = read_private_file(
        path,
        MAX_ZEC_AGREEMENT_RECORD_BYTES as u64,
        "countersigned ZEC agreement",
    )?;
    ensure!(
        actual.as_slice() == expected,
        "agreement output already exists with different bytes"
    );
    Ok(())
}
