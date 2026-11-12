//! Taker side of the take-time Zcash agreement (`zec_reserve_v1`,
//! docs/sdk-lez-zec.md): the Bitcoin model of ADR 0213 S1.3 for Zcash.
//!
//! The take reserves with the Maker, recomposes the Maker's draft from the
//! offer, its own facts and the Maker's few and requires byte equality, writes
//! its own material and actor template into the swap directory, starts the
//! swap's sidecar and persists the swap as a prepared entry. The existing
//! Zcash take (`zec_chat_propose_v1`, `zec_chat_complete_v1`, acceptance) then
//! runs it unchanged. Every step is idempotent on the swap directory, so a
//! replayed initiation resumes where it stopped; on start the Node reloads
//! the entries and keeps their sidecars running.

use std::{
    fs,
    os::unix::fs::DirBuilderExt as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};
use lez_bridge_protocol::{DiscoveryWindow, Hex32, RequestId};
use lez_btc_role_lifecycle::{
    SwapLayout, SwapSidecar,
    layout::{read_private, write_private_exact},
};
use lez_node_common::{
    ZecOutpointV1, ZecReserveRequestV1, ZecReserveResponseV1, local_rpc::call_local_chat_rpc,
};
use lez_swap_core::{Pair, SwapDirection, SwapId};
use lez_swap_store::maker_zec_chat_session_id;
use lez_zec_swap_sdk::NegotiationTranscriptV1;
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zec_reference_actor::{
    ActorRole, CandidateOutpoint, ZEC_ROLE_DRAFT_FILE, ZecFunderBalanceV1, ZecLezRole,
    ZecMakerReservation, ZecReservationTerms, ZecRoleConfigV1, ZecRoleTemplate, ZecTakerCheck,
    ZecTakerContribution, begin_zec_taker_role, check_zec_taker_role, select_zec_funding,
    write_zec_role_template, zec_dynamic_swap_id, zec_tip_height,
};

use super::btc_dynamic::{DynamicBtcRole, TakeRequest};
use crate::{
    acceptance_files::derived_request_id,
    taker_service_config::prepared::{
        ImmutablePrivateFileV1, PreparedConfigurationV1, SecretPrivateFileV1,
    },
};

const MAX_RECORD_BYTES: usize = 64 * 1024;
const PREPARED_FILE: &str = "zec-taker-prepared.json";
const RESERVE_FILE: &str = "zec-taker-reserve.json";
/// The Taker's own material and template inside the swap directory.
const ROLE_DIRECTORY: &str = "zec";

/// The Taker's take-time Zcash authority, over the Node-owned lifecycle's LEZ
/// role (identity, signer, swaps root, sidecar recipe).
pub struct DynamicZecRole {
    config: ZecRoleConfigV1,
    btc: std::sync::Arc<DynamicBtcRole>,
}

impl std::fmt::Debug for DynamicZecRole {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DynamicZecRole")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// The reservation round as sent and answered, so a retried take sends the
/// same request (the Maker answers a replay) and checks the same answer.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TakerReserveRecordV1 {
    schema_version: u16,
    request: ZecReserveRequestV1,
    tip_before: u32,
    response: Option<ZecReserveResponseV1>,
}

fn candidate(outpoint: &ZecOutpointV1) -> CandidateOutpoint {
    CandidateOutpoint::new(
        Hex32::from_bytes(outpoint.transaction_id),
        outpoint.output_index,
    )
}

impl DynamicZecRole {
    /// Loads the Taker's Zcash role configuration; its Zcash key must load.
    ///
    /// # Errors
    ///
    /// Fails when the configuration or its key is invalid.
    pub fn load(config_file: &Path, btc: std::sync::Arc<DynamicBtcRole>) -> Result<Self> {
        let config = ZecRoleConfigV1::load(config_file)?;
        config.zcash_key()?;
        Ok(Self { config, btc })
    }

    fn layout(&self, reservation_id: &RequestId) -> SwapLayout {
        self.btc.layout(reservation_id)
    }

    fn swap_directories(&self) -> Vec<PathBuf> {
        fs::read_dir(&self.btc.runtime().config().swaps_root)
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default()
    }

    /// Every swap this Node prepared at take time, as catalog entries.
    pub(crate) fn persisted_entries(&self) -> Vec<PreparedConfigurationV1> {
        self.swap_directories()
            .into_iter()
            .filter_map(|root| read_private(&root.join(PREPARED_FILE), MAX_RECORD_BYTES).ok())
            .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
            .collect()
    }

    /// Turns a persisted entry into a catalog entry, re-authenticating the
    /// offer against its Delivery source like the static catalog does.
    pub(crate) fn load_entry(
        &self,
        configured: &PreparedConfigurationV1,
    ) -> Result<crate::PreparedTakerInitiationV1> {
        self.btc.load_entry_as(Pair::Zcash, false, configured)
    }

    /// Keeps every prepared swap's sidecar running, also after a restart.
    pub(crate) fn spawn_sidecar_keepalive(self: &std::sync::Arc<Self>) {
        let role = std::sync::Arc::clone(self);
        std::thread::spawn(move || {
            loop {
                for configured in role.persisted_entries() {
                    let layout = role.layout(&configured.reservation_id);
                    if let Err(error) =
                        SwapSidecar::ensure(role.btc.runtime(), &layout, &configured.reservation_id)
                    {
                        eprintln!(
                            "taker ZEC swap {}: sidecar unavailable: {error:#}",
                            configured.swap_id.as_str()
                        );
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(15));
            }
        });
    }

    /// Funding outpoints this Taker already committed to other reservations.
    fn committed_funding(&self, except: &Path) -> Vec<CandidateOutpoint> {
        self.swap_directories()
            .into_iter()
            .filter(|root| root != except)
            .filter_map(|root| read_private(&root.join(RESERVE_FILE), MAX_RECORD_BYTES).ok())
            .filter_map(|bytes| serde_json::from_slice::<TakerReserveRecordV1>(&bytes).ok())
            .filter_map(|record| record.request.taker_funding.as_ref().map(candidate))
            .collect()
    }

    /// What this Taker can fund Zcash from (`taker_wallet_balances_v1`);
    /// `None` when its Zcash key can no longer be read.
    pub(crate) async fn funder_balance(&self) -> Option<ZecFunderBalanceV1> {
        let secret = self.config.zcash_key().ok()?;
        let mut key = SecretKey::from_slice(secret.as_ref()).ok()?;
        let owner = PublicKey::from_secret_key(&Secp256k1::signing_only(), &key);
        key.non_secure_erase();
        let committed = self.committed_funding(Path::new(""));
        Some(self.config.funder_balance(&owner, &committed).await)
    }

    fn lez_role(&self) -> ZecLezRole {
        let runtime = self.btc.runtime();
        ZecLezRole {
            runtime: runtime.runtime_descriptor(),
            authenticated_transfer_program_id: *runtime
                .lez_identity()
                .authenticated_transfer_program_id(),
        }
    }
}

fn store_record(layout: &SwapLayout, record: &TakerReserveRecordV1) -> Result<()> {
    let path = layout.root().join(RESERVE_FILE);
    let bytes = serde_json::to_vec_pretty(record)?;
    if path.exists() {
        fs::write(&path, &bytes)?;
    } else {
        write_private_exact(&path, &bytes)?;
    }
    Ok(())
}

/// Reserves with the Maker, checks its draft, writes the Taker's own template
/// and persists the swap as a prepared entry. Idempotent on the swap directory.
#[allow(clippy::too_many_lines)]
pub(super) async fn prepare(
    dynamic: &DynamicZecRole,
    request: &TakeRequest,
    now: u64,
) -> Result<PreparedConfigurationV1> {
    ensure!(request.route.pair() == Pair::Zcash, "not a Zcash route");
    let reservation_id = derived_request_id(&request.request_id, "zec", b"node-reservation")?;
    let layout = dynamic.layout(&reservation_id);
    let prepared_file = layout.root().join(PREPARED_FILE);
    if layout.exists()
        && let Ok(bytes) = read_private(&prepared_file, MAX_RECORD_BYTES)
    {
        return Ok(serde_json::from_slice(&bytes)?);
    }
    let offer = dynamic
        .btc
        .authenticate_offer(
            &request.offer_id,
            request.route,
            &request.maker_identity,
            now,
            request.announcement.clone(),
        )
        .await?;
    ensure!(
        offer.commitment() == request.signed_envelope_sha256
            && offer.offer().route() == request.route
            && offer.offer().quote_foreign_amount(request.foreign_units)?
                == request.expected_lez_units
            && offer.offer().created_at_unix_seconds() <= now
            && now < offer.offer().expires_at_unix_seconds(),
        "the selected offer is not the live offer the request names"
    );
    let (source_id, _) = dynamic.btc.source_for(&request.maker_identity)?;
    let direction = request.route.direction();
    let commitment = offer.commitment();
    let terms = ZecReservationTerms {
        direction,
        swap_id: zec_dynamic_swap_id(&commitment, &reservation_id)?,
        transcript: NegotiationTranscriptV1::new(
            maker_zec_chat_session_id(&reservation_id),
            commitment,
            offer.offer().expires_at_unix_seconds(),
        ),
        zcash_principal: request.foreign_units,
        lez_amount: request.expected_lez_units,
    };
    let lez = dynamic.lez_role();
    let taker_secret = dynamic.config.zcash_key()?;
    let mut secret = SecretKey::from_slice(taker_secret.as_ref())?;
    let taker_public = PublicKey::from_secret_key(&Secp256k1::signing_only(), &secret);
    secret.non_secure_erase();
    let rpc = dynamic.config.connect()?;
    let role_root = layout.root().join(ROLE_DIRECTORY);
    if !layout.exists() {
        fs::DirBuilder::new().mode(0o700).create(layout.root())?;
    }
    let envelope_file = layout.root().join("zec-signed-envelope.bin");
    write_private_exact(&envelope_file, offer.signed_envelope())?;

    // The request, made once: in TakerSellsForeign the Taker funds Zcash, so
    // it makes its preimage and picks its own funding output first.
    let mut record =
        if let Ok(bytes) = read_private(&layout.root().join(RESERVE_FILE), MAX_RECORD_BYTES) {
            serde_json::from_slice::<TakerReserveRecordV1>(&bytes)?
        } else {
            let _ = fs::remove_dir_all(&role_root);
            let tip_before = zec_tip_height(&dynamic.config, &rpc).await?;
            let (taker_funding, taker_secret_digest) = match direction {
                SwapDirection::TakerSellsForeign => {
                    let funding = select_zec_funding(
                        &dynamic.config,
                        &rpc,
                        &taker_public,
                        request.foreign_units,
                        &dynamic.committed_funding(layout.root()),
                    )
                    .await?;
                    let digest = begin_zec_taker_role(&role_root, &taker_secret)?;
                    (
                        Some(ZecOutpointV1 {
                            transaction_id: *funding.transaction_id().as_bytes(),
                            output_index: funding.output_index(),
                        }),
                        Some(digest),
                    )
                }
                SwapDirection::TakerSellsLez => (None, None),
            };
            let record = TakerReserveRecordV1 {
                schema_version: 1,
                request: ZecReserveRequestV1 {
                    schema_version: 1,
                    reservation_id: reservation_id.clone(),
                    offer_id: request.offer_id.clone(),
                    signed_offer_envelope: offer.signed_envelope().to_vec(),
                    foreign_units: request.foreign_units,
                    taker_zcash_public_key: taker_public.serialize().to_vec(),
                    taker_lez_account: *lez.runtime.signer_account_id.as_bytes(),
                    taker_funding,
                    taker_secret_digest,
                },
                tip_before,
                response: None,
            };
            store_record(&layout, &record)?;
            record
        };
    let response = if let Some(response) = record.response.clone() {
        response
    } else {
        let response: ZecReserveResponseV1 =
            call_local_chat_rpc(dynamic.btc.chat_socket(), "zec_reserve_v1", &record.request)
                .await
                .context("zec_reserve_v1")?;
        record.response = Some(response.clone());
        store_record(&layout, &record)?;
        response
    };
    ensure!(
        response.schema_version == 1 && response.swap_id.as_ref() == terms.swap_id.as_str(),
        "the Maker answered for another swap"
    );
    let contribution = ZecTakerContribution {
        taker_zcash_key: taker_public.serialize(),
        taker_lez_account: record.request.taker_lez_account,
        taker_funding: record.request.taker_funding.as_ref().map(candidate),
        taker_secret_digest: record.request.taker_secret_digest,
    };
    let maker = ZecMakerReservation {
        draft_wire: response.unsigned_draft_wire.clone(),
        zcash_anchor_height: response.zcash_anchor_height,
        lez_anchor_unix_seconds: response.lez_anchor_unix_seconds,
        maker_funding: response.maker_funding.as_ref().map(candidate),
        maker_secret_digest: response.maker_secret_digest,
        zebra_genesis: Hex32::from_bytes(response.zebra_genesis),
    };
    let draft_file = role_root.join(ZEC_ROLE_DRAFT_FILE);
    if !draft_file.is_file() {
        // A check that failed after writing the Taker's secrets left them
        // behind; when the Taker funds nothing they are made again.
        if direction == SwapDirection::TakerSellsLez {
            let _ = fs::remove_dir_all(&role_root);
        }
        check_zec_taker_role(
            &dynamic.config,
            &rpc,
            &terms,
            &lez,
            &taker_secret,
            &contribution,
            response.maker_lez_account,
            &maker,
            &ZecTakerCheck {
                tip_before: record.tip_before,
                now: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(now, |elapsed| elapsed.as_secs()),
                maker_zcash_key: request.maker_identity,
            },
            &role_root,
        )
        .await?;
    }
    ensure!(
        fs::read(&draft_file)? == response.unsigned_draft_wire,
        "the checked draft changed on disk"
    );

    // The swap's own sidecar and the Taker's actor template around the draft.
    let runtime = dynamic.btc.runtime();
    let template = role_root.join("actor-config.json");
    if !template.is_file() {
        let sidecar = SwapSidecar::ensure(runtime, &layout, &reservation_id)?;
        let finalized = sidecar.client(runtime)?.finalized_height().await?;
        write_zec_role_template(
            &dynamic.config,
            &role_root,
            &ZecRoleTemplate {
                role: ActorRole::Taker,
                run_id: sidecar.run_id().clone(),
                swap_id: terms.swap_id.clone(),
                bridge_endpoint: sidecar.endpoint(),
                capability_file: sidecar.capability_file().to_path_buf(),
                runtime: runtime.runtime_descriptor(),
                zebra_genesis: maker.zebra_genesis,
                lez_discovery: DiscoveryWindow::new(
                    finalized.saturating_sub(32).max(1),
                    runtime.config().lez.discovery_max_blocks,
                )?,
                funding: contribution.taker_funding.clone(),
            },
        )?;
    }
    let template_bytes = fs::read(&template)?;
    let configured = PreparedConfigurationV1 {
        source_id: source_id.into(),
        swap_id: SwapId::new(response.swap_id.clone())
            .map_err(|error| anyhow::anyhow!("{error:?}"))?,
        offer_id: request.offer_id.clone(),
        reservation_id,
        foreign_units: request.foreign_units,
        lez_units: request.expected_lez_units,
        signed_envelope: ImmutablePrivateFileV1 {
            path: envelope_file,
            sha256: commitment,
        },
        unsigned_draft: ImmutablePrivateFileV1 {
            path: draft_file,
            sha256: Sha256::digest(&response.unsigned_draft_wire).into(),
        },
        // The agreement key is the Taker's Zcash key (zec_taker_accept.rs).
        signing_key: SecretPrivateFileV1 {
            path: role_root.join("zcash.key"),
        },
        source_config: ImmutablePrivateFileV1 {
            path: template,
            sha256: Sha256::digest(&template_bytes).into(),
        },
        agreement_output: layout.root().join("zec-agreement.borsh"),
        actor_root: layout.root().join("zec-actor"),
        receipt_output: layout.root().join("zec-receipt.json"),
    };
    write_private_exact(&prepared_file, &serde_json::to_vec_pretty(&configured)?)?;
    Ok(configured)
}
