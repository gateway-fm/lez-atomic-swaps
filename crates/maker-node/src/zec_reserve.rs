//! Maker side of the take-time Zcash agreement (`zec_reserve_v1`,
//! docs/sdk-lez-zec.md): the Bitcoin model of ADR 0213 S1.3 for Zcash.
//!
//! A reservation costs the Maker one Zebra read pass and a few private files
//! in the reservation's swap directory (beside the Bitcoin swaps, so both
//! share the sidecar port bookkeeping): its Zcash key, a recovery key, the
//! claim preimage when it funds Zcash, the unsigned draft and the record of
//! the round. The swap's sidecar and actor template are made only at
//! `zec_chat_complete_v1`, once a countersigned agreement exists; from then on
//! the template is in the actor registry and its sidecar is kept running, also
//! across a Node restart.

use std::{
    fs,
    os::unix::fs::DirBuilderExt as _,
    path::{Path, PathBuf},
};

use anyhow::Context as _;
use lez_bridge_protocol::{DiscoveryWindow, Hex32};
use lez_btc_role_lifecycle::{
    BtcRoleRuntime, SwapLayout, SwapSidecar,
    layout::{read_private, write_private_exact},
};
use lez_zec_swap_sdk::NegotiationTranscriptV1;
use sha2::Digest as _;
use zec_reference_actor::{
    CandidateOutpoint, ZecFunderBalanceV1, ZecLezRole, ZecReservationTerms, ZecRoleConfigV1,
    ZecRoleTemplate, ZecTakerContribution, reserve_zec_maker_role, write_zec_role_template,
    zec_dynamic_swap_id,
};
use zeroize::Zeroizing;

use super::*;

/// Reservations not yet completed that may exist at once.
const MAX_LIVE_RESERVATIONS: usize = 16;
/// How long an uncompleted reservation holds its funding output (the Bitcoin
/// lifecycle's unbound grace); a completion after it is refused.
const RESERVATION_GRACE_SECONDS: u64 = 900;
const MAX_RECORD_BYTES: usize = 64 * 1024;
const RECORD_FILE: &str = "zec-reservation.json";
/// The Maker's own material and template inside the swap directory.
const ROLE_DIRECTORY: &str = "zec";

/// The Maker's take-time Zcash authority.
pub struct ZecMakerReservations {
    config: ZecRoleConfigV1,
    /// The Node's LEZ role (identity, signer, swaps root, sidecar recipe),
    /// read from the same role configuration as its Bitcoin lifecycle.
    runtime: BtcRoleRuntime,
    rounds: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for ZecMakerReservations {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ZecMakerReservations")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// What the Maker decided at reservation time.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ZecReservationRecordV1 {
    schema_version: u16,
    request_sha256: [u8; 32],
    response: ZecReserveResponseV1,
    direction: SwapDirection,
    expires_at_unix_seconds: u64,
    /// The one outpoint the agreement commits (the Maker's or the Taker's).
    funding: ZecOutpointV1,
}

impl ZecReservationRecordV1 {
    fn load(root: &Path) -> anyhow::Result<Self> {
        let bytes = read_private(&root.join(RECORD_FILE), MAX_RECORD_BYTES)?;
        serde_json::from_slice(&bytes).context("Zcash reservation record")
    }
}

fn candidate(outpoint: &ZecOutpointV1) -> CandidateOutpoint {
    CandidateOutpoint::new(
        Hex32::from_bytes(outpoint.transaction_id),
        outpoint.output_index,
    )
}

fn wire_outpoint(candidate: &CandidateOutpoint) -> ZecOutpointV1 {
    ZecOutpointV1 {
        transaction_id: *candidate.transaction_id().as_bytes(),
        output_index: candidate.output_index(),
    }
}

fn internal(error: &dyn std::fmt::Display) -> ErrorObjectOwned {
    eprintln!("maker ZEC reservation: {error}");
    rpc_error(INTERNAL_ERROR, "maker ZEC reservation step failed")
}

fn template_file(layout: &SwapLayout) -> PathBuf {
    layout.root().join(ROLE_DIRECTORY).join("actor-config.json")
}

impl ZecMakerReservations {
    /// Binds the Zcash role configuration to the LEZ role of the Node-owned
    /// lifecycle's configuration file: its identity, signer, swaps root and
    /// sidecar recipe are shared, read as that lifecycle reads them.
    ///
    /// # Errors
    ///
    /// Fails when either configuration is invalid.
    pub fn load(config_file: &Path, role_config_file: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            config: ZecRoleConfigV1::load(config_file)?,
            runtime: BtcRoleRuntime::load(Participant::Maker, role_config_file)?,
            rounds: tokio::sync::Mutex::new(()),
        })
    }

    fn layout(&self, reservation_id: &RequestId) -> SwapLayout {
        SwapLayout::new(&self.runtime.config().swaps_root, reservation_id)
    }

    fn lez_role(&self) -> ZecLezRole {
        let runtime = &self.runtime;
        ZecLezRole {
            runtime: runtime.runtime_descriptor(),
            authenticated_transfer_program_id: *runtime
                .lez_identity()
                .authenticated_transfer_program_id(),
        }
    }

    /// Every reservation recorded under the swaps root.
    fn records(&self) -> Vec<(RequestId, SwapLayout, ZecReservationRecordV1)> {
        let Ok(entries) = fs::read_dir(&self.runtime.config().swaps_root) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let reservation_id = RequestId::new(entry.file_name().to_str()?.to_owned()).ok()?;
                let layout = self.layout(&reservation_id);
                let record = ZecReservationRecordV1::load(layout.root()).ok()?;
                Some((reservation_id, layout, record))
            })
            .collect()
    }

    /// Outpoints no new reservation may commit: those of completed swaps and
    /// of reservations still inside their grace. A spent one is skipped by
    /// the funder search anyway.
    fn committed(&self, now: u64) -> (Vec<CandidateOutpoint>, usize) {
        let mut committed = Vec::new();
        let mut live = 0;
        for (_, layout, record) in self.records() {
            let completed = template_file(&layout).is_file();
            if completed || now < record.expires_at_unix_seconds {
                committed.push(candidate(&record.funding));
            }
            if !completed && now < record.expires_at_unix_seconds {
                live += 1;
            }
        }
        (committed, live)
    }

    /// What this Maker can fund Zcash from (`maker_wallet_balances_v1`): its
    /// Zcash key is its Chat signing key.
    pub(super) async fn funder_balance(&self, signing_key: &SecretKey) -> ZecFunderBalanceV1 {
        let owner = PublicKey::from_secret_key(&Secp256k1::signing_only(), signing_key);
        let (committed, _) = self.committed(trusted_now_unix_seconds().unwrap_or_default());
        self.config.funder_balance(&owner, &committed).await
    }

    /// Refuses a proposal for a take-time reservation unless it is exactly
    /// the draft this Maker answered with and the reservation still holds.
    /// A reservation id this Maker never reserved is the static catalog's.
    pub(super) fn check_proposal(
        &self,
        reservation_id: &RequestId,
        draft_wire: &[u8],
        now: u64,
    ) -> RpcResult<()> {
        let layout = self.layout(reservation_id);
        let Ok(record) = ZecReservationRecordV1::load(layout.root()) else {
            return Ok(());
        };
        if record.response.unsigned_draft_wire != draft_wire {
            return Err(invalid_request(
                "the proposed ZEC draft is not the reserved draft",
            ));
        }
        if !template_file(&layout).is_file() && now >= record.expires_at_unix_seconds {
            return Err(invalid_request("the ZEC reservation expired"));
        }
        Ok(())
    }

    /// Before a completion: starts the reservation's sidecar, writes the
    /// Maker's actor template around the reserved draft and registers it, so
    /// the existing completion provisions the actor from it. A completion of
    /// a static catalog swap, or a replay, finds nothing to do.
    pub(super) fn materialize(
        &self,
        reservation_id: &RequestId,
        provisioner: &ZecMakerActorProvisioner,
        now: u64,
    ) -> RpcResult<()> {
        let _serial = self.rounds.blocking_lock();
        let layout = self.layout(reservation_id);
        let Ok(record) = ZecReservationRecordV1::load(layout.root()) else {
            return Ok(());
        };
        let template = template_file(&layout);
        if !template.is_file() {
            if now >= record.expires_at_unix_seconds {
                return Err(invalid_request("the ZEC reservation expired"));
            }
            let runtime = &self.runtime;
            let sidecar = SwapSidecar::ensure(runtime, &layout, reservation_id)
                .map_err(|error| internal(&error))?;
            let client = sidecar.client(runtime).map_err(|error| internal(&error))?;
            let finalized = tokio::runtime::Handle::current()
                .block_on(client.finalized_height())
                .map_err(|error| internal(&error))?;
            let discovery = DiscoveryWindow::new(
                finalized.saturating_sub(32).max(1),
                runtime.config().lez.discovery_max_blocks,
            )
            .map_err(|error| internal(&error))?;
            let swap_id =
                SwapId::new(record.response.swap_id.clone()).map_err(|error| internal(&error))?;
            write_zec_role_template(
                &self.config,
                &layout.root().join(ROLE_DIRECTORY),
                &ZecRoleTemplate {
                    role: ActorRole::Maker,
                    run_id: sidecar.run_id().clone(),
                    swap_id,
                    bridge_endpoint: sidecar.endpoint(),
                    capability_file: sidecar.capability_file().to_path_buf(),
                    runtime: runtime.runtime_descriptor(),
                    zebra_genesis: Hex32::from_bytes(record.response.zebra_genesis),
                    lez_discovery: discovery,
                    funding: record.response.maker_funding.as_ref().map(candidate),
                },
            )
            .map_err(|error| internal(&error))?;
        }
        provisioner
            .insert(&template)
            .map_err(|error| internal(&error))
    }

    /// Registers every completed reservation's template again after a
    /// restart and keeps their sidecars running, as the Bitcoin lifecycle
    /// keeps its swaps'.
    pub(super) fn restore(self: &Arc<Self>, provisioner: &ZecMakerActorProvisioner) {
        for (_, layout, _) in self.records() {
            let template = template_file(&layout);
            if template.is_file()
                && let Err(error) = provisioner.insert(&template)
            {
                eprintln!(
                    "maker ZEC reservation {}: template no longer loads ({error:#})",
                    layout.root().display()
                );
            }
        }
        let reservations = Arc::clone(self);
        std::thread::spawn(move || {
            loop {
                for (reservation_id, layout, _) in reservations.records() {
                    if template_file(&layout).is_file()
                        && let Err(error) =
                            SwapSidecar::ensure(&reservations.runtime, &layout, &reservation_id)
                    {
                        eprintln!(
                            "maker ZEC reservation {}: sidecar unavailable: {error:#}",
                            reservation_id.as_str()
                        );
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(15));
            }
        });
    }
}

#[allow(clippy::too_many_lines)]
pub(super) async fn reserve(
    request: ZecReserveRequestV1,
    context: Arc<MakerRpc>,
) -> RpcResult<ZecReserveResponseV1> {
    if request.schema_version != 1 || request.foreign_units == 0 {
        return Err(invalid_request("unsupported or empty ZEC reservation"));
    }
    let reservations = context
        .zec_reservations
        .as_ref()
        .ok_or_else(|| invalid_request("maker ZEC reservations are unavailable"))?;
    let signing_key = context
        .chat_signing_key
        .as_ref()
        .ok_or_else(|| invalid_request("maker Chat signer is unavailable"))?;
    let _serial = reservations.rounds.lock().await;
    let layout = reservations.layout(&request.reservation_id);
    let digest: [u8; 32] =
        Sha256::digest(serde_json::to_vec(&request).map_err(|error| internal(&error))?).into();
    if layout.root().exists() {
        return match ZecReservationRecordV1::load(layout.root()) {
            Ok(record) if record.request_sha256 == digest => Ok(ZecReserveResponseV1 {
                was_replay: true,
                ..record.response
            }),
            _ => Err(rpc_error(CONFLICT, "the reservation id is already taken")),
        };
    }
    let now = trusted_now_unix_seconds()?;
    let delivery = context
        .delivery
        .as_ref()
        .ok_or_else(|| invalid_request("maker Chat Delivery is unavailable"))?;
    let authenticated = delivery
        .authenticate_envelope(&request.signed_offer_envelope)
        .map_err(|error| invalid_request(error.to_string()))?;
    let offer = authenticated.offer();
    if offer.id() != &request.offer_id
        || offer.route().pair() != Pair::Zcash
        || offer.created_at_unix_seconds() > now
        || now >= offer.expires_at_unix_seconds()
        || request.foreign_units < offer.minimum_foreign_units()
        || request.foreign_units > offer.maximum_foreign_units()
    {
        return Err(invalid_request(
            "Delivery offer does not match the ZEC reservation",
        ));
    }
    let lez_units = offer
        .quote_foreign_amount(request.foreign_units)
        .map_err(invalid_request)?;
    let (committed, live) = reservations.committed(now);
    if live >= MAX_LIVE_RESERVATIONS {
        return Err(rpc_error(
            OFFER_UNAVAILABLE,
            "this Maker holds too many open ZEC reservations; take the offer again later",
        ));
    }
    let direction = offer.route().direction();
    let commitment = authenticated.commitment();
    let terms = ZecReservationTerms {
        direction,
        swap_id: zec_dynamic_swap_id(&commitment, &request.reservation_id)
            .map_err(|error| internal(&error))?,
        transcript: NegotiationTranscriptV1::new(
            maker_zec_chat_session_id(&request.reservation_id),
            commitment,
            offer.expires_at_unix_seconds(),
        ),
        zcash_principal: request.foreign_units,
        lez_amount: lez_units,
    };
    let taker = ZecTakerContribution {
        taker_zcash_key: request
            .taker_zcash_public_key
            .as_slice()
            .try_into()
            .map_err(|_| invalid_request("the Taker's Zcash key is not 33 bytes"))?,
        taker_lez_account: request.taker_lez_account,
        taker_funding: request.taker_funding.as_ref().map(candidate),
        taker_secret_digest: request.taker_secret_digest,
    };
    let lez = reservations.lez_role();
    let rpc = reservations
        .config
        .connect()
        .map_err(|error| internal(&error))?;
    // Everything is made in a private stage beside the swap directory and
    // published by one rename, so a crash leaves no half reservation.
    let stage = PathBuf::from(format!("{}.zec-stage", layout.root().display()));
    let _ = fs::remove_dir_all(&stage);
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&stage)
        .map_err(|error| internal(&error))?;
    let reserved = reserve_zec_maker_role(
        &reservations.config,
        &rpc,
        &terms,
        &lez,
        &Zeroizing::new(signing_key.secret_bytes()),
        &taker,
        &committed,
        now,
        &stage.join(ROLE_DIRECTORY),
    )
    .await;
    let reserved = match reserved {
        Ok(reserved) => reserved,
        Err(error) => {
            let _ = fs::remove_dir_all(&stage);
            eprintln!("maker ZEC reservation refused: {error:#}");
            return Err(invalid_request(format!("ZEC reservation refused: {error}")));
        }
    };
    let response = ZecReserveResponseV1 {
        schema_version: 1,
        was_replay: false,
        swap_id: terms.swap_id.as_str().into(),
        unsigned_draft_wire: reserved.draft_wire,
        maker_lez_account: *lez.runtime.signer_account_id.as_bytes(),
        zcash_anchor_height: reserved.zcash_anchor_height,
        lez_anchor_unix_seconds: reserved.lez_anchor_unix_seconds,
        zebra_genesis: *reserved.zebra_genesis.as_bytes(),
        maker_funding: reserved.maker_funding.as_ref().map(wire_outpoint),
        maker_secret_digest: reserved.maker_secret_digest,
    };
    let funding = response
        .maker_funding
        .or(request.taker_funding)
        .ok_or_else(|| internal(&"the reservation committed no funding output"))?;
    let record = ZecReservationRecordV1 {
        schema_version: 1,
        request_sha256: digest,
        response: response.clone(),
        direction,
        expires_at_unix_seconds: now
            .saturating_add(RESERVATION_GRACE_SECONDS)
            .min(offer.expires_at_unix_seconds()),
        funding,
    };
    let published = serde_json::to_vec_pretty(&record)
        .map_err(anyhow::Error::from)
        .and_then(|bytes| write_private_exact(&stage.join(RECORD_FILE), &bytes))
        .and_then(|_| fs::rename(&stage, layout.root()).map_err(anyhow::Error::from));
    if let Err(error) = published {
        let _ = fs::remove_dir_all(&stage);
        return Err(internal(&error));
    }
    Ok(response)
}
