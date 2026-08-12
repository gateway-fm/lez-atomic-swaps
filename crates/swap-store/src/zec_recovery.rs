//! Role-fixed `SQLite` implementation of the concrete ZEC SDK recovery port.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use async_trait::async_trait;
use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, Participant, SwapCoordinator, SwapId, UnixSeconds};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementEnvelopeV1, AcceptedZecAgreementV1, CLAIM_RECORD_SCHEMA_V1,
    CLAIM_RECORD_SCHEMA_V2, ClaimIntentRecordV1, ClaimIntentV1, ClaimMaterialContext,
    ClaimMaterialPurpose, ClaimPreimage, ClaimRecoveryStore, ClaimStepV1, ClaimSubmissionContext,
    CreateAgreementOutcome, CreateFirstLockOutcome, FIRST_LOCK_RECORD_SCHEMA_V1,
    FirstLockIntentRecordV1, FirstLockIntentV1, FirstLockProjectionCommit,
    FirstLockTransitionRecordV1, FirstLockTransitionV1, FollowupClaimTransitionRecordV1,
    FollowupClaimTransitionV1, LezAssetV1, LezObservationTrackerV1, MAKER_LOCK_RECORD_SCHEMA_V1,
    MakerLockIntentRecordV1, MakerLockIntentV1, MakerLockTransitionRecordV1, MakerLockTransitionV1,
    OBSERVED_MAKER_LOCK_SCHEMA_V1, ObservedFollowupClaimTransitionRecordV1,
    ObservedFollowupClaimTransitionV1, ObservedMakerLockTransitionRecordV1,
    ObservedMakerLockTransitionV1, ObservedRevealingClaimTransitionRecordV1,
    ObservedRevealingClaimTransitionV1, ObservedTakerFirstLockTransitionRecordV1,
    ObservedTakerFirstLockTransitionV1, PROTECTED_CLAIM_SCHEMA_V1, PreparedClaimSubmissionV1,
    ProtectedClaimEnvelope, ProtectedClaimKey, ProtectedClaimPayloadEnvelope,
    REFUND_RECORD_SCHEMA_V1, RecoveryStore, RefundIntentRecordV1, RefundIntentV1,
    RefundRecoveryStore, RefundTransitionRecordV1, RefundTransitionV1,
    RevealingClaimTransitionRecordV1, RevealingClaimTransitionV1, ZcashObservationTracker,
    ZecMakerAgreementProposalV1, ZecSwapBindingRecordV1,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{
    MakerActorKindV1, MakerActorManifestV1, MakerOfferId, MakerOfferV1, SWAP_PAYLOAD_VERSION,
    StoreError, maker_actor_process::register_maker_actor_in_transaction,
    maker_actor_process::require_exact_maker_actor_in_transaction, maker_zec_chat_session_id,
    open_configured_connection, open_existing_configured_connection, participant_name,
    revision_from_sql,
};

const AGREEMENT_PAYLOAD_VERSION: i64 = 1;

/// Result of atomically accepting one countersigned maker ZEC negotiation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MakerZecAcceptanceCommit {
    offer_revision: u64,
    was_replay: bool,
}

impl MakerZecAcceptanceCommit {
    /// Consumed offer revision committed with the application and SDK state.
    #[must_use]
    pub const fn offer_revision(self) -> u64 {
        self.offer_revision
    }

    /// Whether the exact global request already committed this result.
    #[must_use]
    pub const fn was_replay(self) -> bool {
        self.was_replay
    }
}

/// Exact durable result of replaying one already-committed scheduled acceptance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerZecAcceptanceReplay {
    offer_revision: u64,
    swap_id: SwapId,
}

impl MakerZecAcceptanceReplay {
    /// Consumed offer revision from the original transaction.
    #[must_use]
    pub const fn offer_revision(&self) -> u64 {
        self.offer_revision
    }

    /// Application swap whose exact actor remains registered.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }
}

#[derive(Serialize)]
struct CompleteMakerZecRequest<'a> {
    offer_id: &'a MakerOfferId,
    expected_offer_revision: u64,
    reservation_id: &'a RequestId,
    agreement_wire_sha256: [u8; 32],
    secret_digest: [u8; 32],
    maker_claim_authority: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    actor: Option<&'a MakerActorManifestV1>,
}

#[derive(Deserialize)]
struct StoredCompleteMakerZecRequest {
    offer_id: MakerOfferId,
    expected_offer_revision: u64,
    reservation_id: RequestId,
    agreement_wire_sha256: [u8; 32],
    secret_digest: [u8; 32],
    #[serde(default = "default_true")]
    maker_claim_authority: bool,
    actor: Option<StoredMakerActorManifest>,
}

const fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
struct StoredMakerActorManifest {
    swap_id: SwapId,
    kind: String,
    config_path: PathBuf,
    config_sha256: [u8; 32],
    program_path: PathBuf,
    program_sha256: [u8; 32],
    state_database_path: PathBuf,
}

impl StoredMakerActorManifest {
    fn into_manifest(self) -> Result<MakerActorManifestV1, StoreError> {
        let kind = MakerActorKindV1::parse(&self.kind)
            .map_err(|_| StoreError::InvalidMakerActorRegistration)?;
        MakerActorManifestV1::new(
            self.swap_id,
            kind,
            self.config_path,
            self.config_sha256,
            self.program_path,
            self.program_sha256,
            self.state_database_path,
        )
        .map_err(|_| StoreError::InvalidMakerActorRegistration)
    }
}

#[derive(Deserialize, Serialize)]
struct CompleteMakerZecResult {
    schema_version: u16,
    offer_revision: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct MakerObservationTrackers {
    lez: LezObservationTrackerV1,
    zcash: ZcashObservationTracker,
}

/// Cloneable, role-fixed SDK recovery repository.
///
/// Clones share one configured `SQLite` connection. The local participant is
/// fixed when the adapter opens and is included in every composite key.
#[derive(Clone, Debug)]
pub struct SqliteZecRecoveryStore {
    local_participant: Participant,
    connection: Arc<Mutex<Connection>>,
    claim_key: Option<Arc<ProtectedClaimKey>>,
}

impl SqliteZecRecoveryStore {
    /// Opens or creates a schema-v10 recovery store for one local role.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` cannot open, configure, or migrate
    /// the database.
    pub fn open(
        path: impl AsRef<Path>,
        local_participant: Participant,
    ) -> Result<Self, StoreError> {
        Ok(Self {
            local_participant,
            connection: Arc::new(Mutex::new(open_configured_connection(path)?)),
            claim_key: None,
        })
    }

    /// Opens or creates a schema-v10 store with protected claim recovery enabled.
    ///
    /// The key is retained only in zeroizing process memory and is never written
    /// to the database. Reopening claim rows requires the same key ID and material.
    ///
    /// # Errors
    ///
    /// Returns a store error when `SQLite` cannot open, configure, or migrate the
    /// database, or when the supplied key cannot authenticate an existing
    /// role-local claim envelope.
    pub fn open_claim_capable(
        path: impl AsRef<Path>,
        local_participant: Participant,
        claim_key: ProtectedClaimKey,
    ) -> Result<Self, StoreError> {
        let connection = open_configured_connection(path)?;
        validate_existing_claim_envelopes(
            &connection,
            participant_name(local_participant),
            local_participant,
            &claim_key,
        )?;
        Ok(Self {
            local_participant,
            connection: Arc::new(Mutex::new(connection)),
            claim_key: Some(Arc::new(claim_key)),
        })
    }

    /// Opens an existing schema-v10 store with protected claim recovery enabled.
    ///
    /// Unlike [`Self::open_claim_capable`], this entry point never creates a
    /// missing database. It is intended for offline status, where observing an
    /// unactivated actor must not mutate durable state. The existing database
    /// receives the same private-file checks, configuration, migrations, and
    /// claim-envelope authentication as the create-capable entry point.
    ///
    /// # Errors
    ///
    /// Returns a store error when the path is missing or unsafe, `SQLite` cannot
    /// open, configure, or migrate it, or the supplied key cannot authenticate
    /// an existing role-local claim envelope.
    pub fn open_claim_capable_existing(
        path: impl AsRef<Path>,
        local_participant: Participant,
        claim_key: ProtectedClaimKey,
    ) -> Result<Self, StoreError> {
        let connection = open_existing_configured_connection(path)?;
        validate_existing_claim_envelopes(
            &connection,
            participant_name(local_participant),
            local_participant,
            &claim_key,
        )?;
        Ok(Self {
            local_participant,
            connection: Arc::new(Mutex::new(connection)),
            claim_key: Some(Arc::new(claim_key)),
        })
    }

    /// Participant fixed for every operation on this adapter.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Replays one exact already-committed scheduled maker acceptance without
    /// reparsing its agreement against the current wall clock.
    ///
    /// `Ok(None)` means the request ID has never committed. A present mutation
    /// must match every caller-provided identity, its completed negotiation, and
    /// its immutable actor row before the original result is returned.
    ///
    /// # Errors
    ///
    /// Fails closed on role mismatch, changed request bytes or metadata, a
    /// legacy unscheduled result, missing/drifted actor state, or corrupt storage.
    #[allow(clippy::too_many_arguments)]
    pub fn preflight_maker_zec_scheduled_completion_replay(
        &self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_offer_revision: u64,
        reservation_id: &RequestId,
        final_agreement_wire: &[u8],
        preimage: &ClaimPreimage,
    ) -> Result<Option<MakerZecAcceptanceReplay>, StoreError> {
        self.preflight_maker_zec_scheduled_completion_replay_for_role(
            request_id,
            offer_id,
            expected_offer_revision,
            reservation_id,
            final_agreement_wire,
            Some(preimage),
        )
    }

    /// Replays one exact scheduled acceptance with direction-derived Maker claim custody.
    ///
    /// # Errors
    ///
    /// Fails closed on changed request identity, claim ownership, actor state,
    /// negotiation state, or corrupt storage.
    #[allow(clippy::too_many_arguments)]
    pub fn preflight_maker_zec_scheduled_completion_replay_for_role(
        &self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_offer_revision: u64,
        reservation_id: &RequestId,
        final_agreement_wire: &[u8],
        maker_preimage: Option<&ClaimPreimage>,
    ) -> Result<Option<MakerZecAcceptanceReplay>, StoreError> {
        self.require_role(Participant::Maker)?;
        let agreement_wire_sha256: [u8; 32] = Sha256::digest(final_agreement_wire).into();
        let committed_revision = expected_offer_revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let replay = transaction
            .query_row(
                "SELECT operation, request_payload_version, request_json, result_json
                   FROM maker_application_mutations WHERE request_id = ?1",
                params![request_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((operation, stored_version, stored_request, stored_result)) = replay else {
            transaction.commit()?;
            return Ok(None);
        };
        if operation != "zec_negotiation_complete" || stored_version != 1 {
            return Err(StoreError::MakerOfferRequestConflict);
        }
        let stored: StoredCompleteMakerZecRequest = serde_json::from_str(&stored_request)?;
        let supplied_secret_digest =
            maker_preimage.map(|preimage| Sha256::digest(preimage.expose_secret()).into());
        if &stored.offer_id != offer_id
            || stored.expected_offer_revision != expected_offer_revision
            || &stored.reservation_id != reservation_id
            || stored.agreement_wire_sha256 != agreement_wire_sha256
            || (stored.maker_claim_authority
                && supplied_secret_digest != Some(stored.secret_digest))
        {
            return Err(StoreError::MakerOfferRequestConflict);
        }
        let actor = stored
            .actor
            .ok_or(StoreError::InvalidMakerActorRegistration)?
            .into_manifest()?;
        if actor.kind() != MakerActorKindV1::Zcash {
            return Err(StoreError::InvalidMakerActorRegistration);
        }
        let result: CompleteMakerZecResult = serde_json::from_str(&stored_result)?;
        if result.schema_version != 1 || result.offer_revision != committed_revision {
            return Err(StoreError::CorruptMakerOffer);
        }
        let negotiation = transaction
            .query_row(
                "SELECT final_agreement_wire, swap_id, state, updated_request_id
                   FROM maker_zec_negotiations WHERE offer_id = ?1",
                params![offer_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, Option<Vec<u8>>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or(StoreError::InvalidZecRecoveryState)?;
        if negotiation.0.as_deref() != Some(final_agreement_wire)
            || negotiation.1.as_deref() != Some(actor.swap_id().as_str())
            || negotiation.2 != "completed"
            || negotiation.3.as_deref() != Some(request_id.as_str())
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        require_exact_maker_actor_in_transaction(&transaction, &actor)
            .map_err(|_| StoreError::InvalidMakerActorRegistration)?;
        transaction.commit()?;
        Ok(Some(MakerZecAcceptanceReplay {
            offer_revision: result.offer_revision,
            swap_id: actor.swap_id().clone(),
        }))
    }

    /// Atomically completes one staged maker-first ZEC negotiation.
    ///
    /// The countersigned agreement, initial coordinator, immutable ZEC binding,
    /// protected first-claim material, completed negotiation, consumed offer,
    /// and global replay result share one `BEGIN IMMEDIATE` transaction. No
    /// actor is registered by this legacy migration/test entry point, and no
    /// first-lock authority exists until this method commits.
    ///
    /// # Errors
    ///
    /// Fails closed on role, revision, reservation, identity, session, offer,
    /// amount, expiry, proposal, agreement, preimage, replay, or `SQLite` errors.
    pub fn complete_maker_zec_negotiation(
        &self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_offer_revision: u64,
        reservation_id: &RequestId,
        accepted: &AcceptedZecAgreementV1,
        preimage: &ClaimPreimage,
    ) -> Result<MakerZecAcceptanceCommit, StoreError> {
        self.complete_maker_zec_negotiation_inner(
            request_id,
            offer_id,
            expected_offer_revision,
            reservation_id,
            accepted,
            Some(preimage),
            None,
        )
    }

    /// Atomically accepts a staged maker-first ZEC negotiation and schedules its actor.
    ///
    /// The immutable actor registration participates in the same `BEGIN IMMEDIATE`
    /// transaction as the agreement, coordinator, binding, protected claim material,
    /// consumed offer, completed negotiation, and replay record. A lost response may
    /// replay only the same manifest; scheduler timing may already have advanced.
    ///
    /// # Errors
    ///
    /// Fails closed on every ordinary completion error, a manifest for another swap
    /// or pair, an immutable registration conflict, or a missing/corrupt replay row.
    #[allow(clippy::too_many_arguments)]
    pub fn complete_maker_zec_negotiation_and_register_actor(
        &self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_offer_revision: u64,
        reservation_id: &RequestId,
        accepted: &AcceptedZecAgreementV1,
        preimage: &ClaimPreimage,
        actor: &MakerActorManifestV1,
        actor_not_before: u64,
    ) -> Result<MakerZecAcceptanceCommit, StoreError> {
        self.complete_maker_zec_negotiation_inner(
            request_id,
            offer_id,
            expected_offer_revision,
            reservation_id,
            accepted,
            Some(preimage),
            Some((actor, actor_not_before)),
        )
    }

    /// Atomically accepts either ZEC direction and schedules the Maker actor.
    ///
    /// Maker claim material is required exactly when the signed direction makes
    /// the Maker the first claimant; the reverse direction must pass `None`.
    ///
    /// # Errors
    ///
    /// Fails closed on any agreement/offer mismatch, incorrect direction-derived
    /// claim custody, actor registration conflict, replay conflict, or storage error.
    #[allow(clippy::too_many_arguments)]
    pub fn complete_maker_zec_negotiation_and_register_actor_for_role(
        &self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_offer_revision: u64,
        reservation_id: &RequestId,
        accepted: &AcceptedZecAgreementV1,
        maker_preimage: Option<&ClaimPreimage>,
        actor: &MakerActorManifestV1,
        actor_not_before: u64,
    ) -> Result<MakerZecAcceptanceCommit, StoreError> {
        self.complete_maker_zec_negotiation_inner(
            request_id,
            offer_id,
            expected_offer_revision,
            reservation_id,
            accepted,
            maker_preimage,
            Some((actor, actor_not_before)),
        )
    }

    #[allow(
        clippy::similar_names,
        clippy::too_many_lines,
        clippy::too_many_arguments
    )]
    fn complete_maker_zec_negotiation_inner(
        &self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_offer_revision: u64,
        reservation_id: &RequestId,
        accepted: &AcceptedZecAgreementV1,
        maker_preimage: Option<&ClaimPreimage>,
        actor: Option<(&MakerActorManifestV1, u64)>,
    ) -> Result<MakerZecAcceptanceCommit, StoreError> {
        self.require_role(Participant::Maker)?;
        self.require_role(accepted.local_participant())?;
        let agreement = accepted.agreement();
        let maker_claim_authority = agreement.lez_claimant() == Participant::Maker;
        if accepted.revision() != 0
            || agreement.coordinator().pair() != Pair::Zcash
            || maker_claim_authority != maker_preimage.is_some()
            || actor.is_some_and(|(manifest, _)| {
                manifest.swap_id() != agreement.coordinator().id()
                    || manifest.kind() != MakerActorKindV1::Zcash
            })
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let actual_secret_digest = *agreement.secret_digest();
        if maker_preimage.is_some_and(|preimage| {
            <[u8; 32]>::from(Sha256::digest(preimage.expose_secret())) != actual_secret_digest
        }) {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let agreement_wire = agreement.encode_wire()?;
        let agreement_wire_sha256: [u8; 32] = Sha256::digest(&agreement_wire).into();
        let committed_revision = expected_offer_revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let request_json = serde_json::to_string(&CompleteMakerZecRequest {
            offer_id,
            expected_offer_revision,
            reservation_id,
            agreement_wire_sha256,
            secret_digest: actual_secret_digest,
            maker_claim_authority,
            actor: actor.map(|(manifest, _)| manifest),
        })?;

        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let replay = transaction
            .query_row(
                "SELECT operation, request_payload_version, request_json, result_json
                   FROM maker_application_mutations WHERE request_id = ?1",
                params![request_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        if let Some((operation, stored_version, stored_request, stored_result)) = replay {
            if operation != "zec_negotiation_complete"
                || stored_version != 1
                || stored_request != request_json
            {
                return Err(StoreError::MakerOfferRequestConflict);
            }
            let result: CompleteMakerZecResult = serde_json::from_str(&stored_result)?;
            if result.schema_version != 1 || result.offer_revision != committed_revision {
                return Err(StoreError::CorruptMakerOffer);
            }
            if let Some((manifest, _)) = actor {
                require_exact_maker_actor_in_transaction(&transaction, manifest)
                    .map_err(|_| StoreError::InvalidMakerActorRegistration)?;
            }
            transaction.commit()?;
            return Ok(MakerZecAcceptanceCommit {
                offer_revision: result.offer_revision,
                was_replay: true,
            });
        }

        #[allow(clippy::type_complexity)]
        let row: Option<(
            i64,
            String,
            i64,
            String,
            i64,
            Option<String>,
            String,
            Vec<u8>,
            Vec<u8>,
            Vec<u8>,
            i64,
            Vec<u8>,
            i64,
            Vec<u8>,
            Vec<u8>,
            String,
        )> = transaction
            .query_row(
                "SELECT o.payload_version, o.payload_json, o.expires_at_unix_seconds,
                        o.state, o.revision, o.reservation_id,
                        n.reservation_id, n.offer_commitment,
                        n.maker_chat_identity, n.taker_chat_identity,
                        n.foreign_units, n.lez_units, n.reserved_at_unix_seconds,
                        n.agreement_commitment, n.maker_proposal_wire, n.state
                   FROM maker_offers o
                   JOIN maker_zec_negotiations n USING (offer_id)
                  WHERE o.offer_id = ?1",
                params![offer_id.as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                        row.get(12)?,
                        row.get(13)?,
                        row.get(14)?,
                        row.get(15)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            offer_payload_version,
            offer_json,
            expires_at,
            offer_state,
            offer_revision,
            offer_reservation,
            staged_reservation,
            offer_commitment,
            maker_chat_identity,
            taker_chat_identity,
            foreign_units,
            lez_units,
            reserved_at,
            staged_agreement_commitment,
            maker_proposal_wire,
            negotiation_state,
        )) = row
        else {
            return Err(StoreError::MissingMakerOffer);
        };
        if offer_payload_version != 1 {
            return Err(StoreError::UnsupportedPayloadVersion {
                kind: "maker offer",
                version: offer_payload_version,
            });
        }
        let offer: MakerOfferV1 = serde_json::from_str(&offer_json)?;
        offer.validate()?;
        let durable_offer_revision = revision_from_sql(offer_revision)?;
        let durable_expires_at = revision_from_sql(expires_at)?;
        let durable_reserved_at = revision_from_sql(reserved_at)?;
        let durable_foreign_units = revision_from_sql(foreign_units)?;
        let durable_lez_units = u128::from_be_bytes(fixed_bytes(lez_units)?);
        let durable_offer_commitment = fixed_bytes(offer_commitment)?;
        let durable_maker_identity = fixed_bytes(maker_chat_identity)?;
        let durable_taker_identity = fixed_bytes(taker_chat_identity)?;
        let durable_agreement_commitment = fixed_bytes(staged_agreement_commitment)?;
        if offer.id() != offer_id
            || offer.expires_at_unix_seconds() != durable_expires_at
            || durable_offer_revision != expected_offer_revision
            || offer_state != "reserved"
            || negotiation_state != "proposed"
            || offer_reservation.as_deref() != Some(reservation_id.as_str())
            || staged_reservation != reservation_id.as_str()
            || accepted.accepted_at().value() < durable_reserved_at
            || accepted.accepted_at().value() >= durable_expires_at
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let proposal = ZecMakerAgreementProposalV1::from_wire_at(
            &maker_proposal_wire,
            UnixSeconds::new(durable_reserved_at),
        )?;
        let transcript = agreement.transcript();
        if proposal.commitment() != &durable_agreement_commitment
            || agreement.agreement_commitment() != &durable_agreement_commitment
            || transcript.offer_commitment() != &durable_offer_commitment
            || transcript.session_id() != &maker_zec_chat_session_id(reservation_id)
            || transcript.expires_at_unix_seconds() != durable_expires_at
            || agreement.application_swap_id() != agreement.coordinator().id().as_str()
            || agreement.zcash_key(Participant::Maker).serialize() != durable_maker_identity
            || agreement.zcash_key(Participant::Taker).serialize() != durable_taker_identity
            || offer.route().pair() != Pair::Zcash
            || offer.route().direction() != agreement.direction()
            || agreement.zcash_amount_zatoshis() != durable_foreign_units
            || agreement.lez_amount() != durable_lez_units
            || offer.quote_foreign_amount(durable_foreign_units)? != durable_lez_units
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }

        let coordinator = agreement.coordinator();
        let coordinator_json = serde_json::to_string(coordinator)?;
        let binding_record = ZecSwapBindingRecordV1::from_binding(agreement.binding());
        binding_record.validate()?;
        let binding_json = serde_json::to_string(&binding_record)?;
        let protected = if let Some(preimage) = maker_preimage {
            Some(ProtectedClaimEnvelope::encrypt(
                preimage,
                self.claim_key()?,
                fresh_claim_nonce()?,
                claim_material_context(accepted, ClaimMaterialPurpose::LocalFirstClaim),
            )?)
        } else {
            None
        };
        transaction.execute(
            "INSERT INTO swaps (id, schema_version, state_json, revision)
             VALUES (?1, ?2, ?3, 0)",
            params![
                coordinator.id().as_str(),
                SWAP_PAYLOAD_VERSION,
                coordinator_json
            ],
        )?;
        if let Some((manifest, not_before)) = actor {
            register_maker_actor_in_transaction(&transaction, manifest, not_before)
                .map_err(|_| StoreError::InvalidMakerActorRegistration)?;
        }
        transaction.execute(
            "INSERT INTO zcash_swap_bindings (swap_id, payload_version, payload_json)
             VALUES (?1, 1, ?2)",
            params![coordinator.id().as_str(), binding_json],
        )?;
        transaction.execute(
            "INSERT INTO zec_sdk_agreements (
                 local_role, swap_id, payload_version, agreement_wire,
                 accepted_at, accepted_revision, active_revision
             ) VALUES ('maker', ?1, ?2, ?3, ?4, 0, 0)",
            params![
                coordinator.id().as_str(),
                AGREEMENT_PAYLOAD_VERSION,
                agreement_wire,
                sql_u64(accepted.accepted_at().value())?,
            ],
        )?;
        if let Some(protected) = &protected {
            insert_claim_material(
                &transaction,
                "maker",
                coordinator.id(),
                ClaimMaterialPurpose::LocalFirstClaim,
                0,
                protected,
            )?;
        }
        let completed_negotiation = transaction.execute(
            "UPDATE maker_zec_negotiations
                SET state = 'completed', final_agreement_wire = ?1,
                    swap_id = ?2, updated_request_id = ?3
              WHERE offer_id = ?4 AND reservation_id = ?5 AND state = 'proposed'",
            params![
                agreement_wire,
                coordinator.id().as_str(),
                request_id.as_str(),
                offer_id.as_str(),
                reservation_id.as_str(),
            ],
        )?;
        let consumed_offer = transaction.execute(
            "UPDATE maker_offers
                SET state = 'consumed', revision = ?1, swap_id = ?2,
                    updated_request_id = ?3
              WHERE offer_id = ?4 AND revision = ?5 AND state = 'reserved'
                AND reservation_id = ?6",
            params![
                sql_u64(committed_revision)?,
                coordinator.id().as_str(),
                request_id.as_str(),
                offer_id.as_str(),
                sql_u64(expected_offer_revision)?,
                reservation_id.as_str(),
            ],
        )?;
        if completed_negotiation != 1 || consumed_offer != 1 {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let result_json = serde_json::to_string(&CompleteMakerZecResult {
            schema_version: 1,
            offer_revision: committed_revision,
        })?;
        transaction.execute(
            "INSERT INTO maker_application_mutations (
                 request_id, operation, request_payload_version, request_json, result_json
             ) VALUES (?1, 'zec_negotiation_complete', 1, ?2, ?3)",
            params![request_id.as_str(), request_json, result_json],
        )?;
        transaction.commit()?;
        Ok(MakerZecAcceptanceCommit {
            offer_revision: committed_revision,
            was_replay: false,
        })
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>, StoreError> {
        self.connection
            .lock()
            .map_err(|_| StoreError::ZecRecoveryLockPoisoned)
    }

    fn role_name(&self) -> &'static str {
        participant_name(self.local_participant)
    }

    fn require_role(&self, actual: Participant) -> Result<(), StoreError> {
        if actual == self.local_participant {
            Ok(())
        } else {
            Err(StoreError::ZecRecoveryRoleMismatch)
        }
    }

    fn claim_key(&self) -> Result<&ProtectedClaimKey, StoreError> {
        self.claim_key
            .as_deref()
            .ok_or(StoreError::MissingZecClaimKey)
    }
}

#[async_trait]
impl RecoveryStore for SqliteZecRecoveryStore {
    type Error = StoreError;

    async fn create_agreement(
        &self,
        envelope: &AcceptedZecAgreementEnvelopeV1,
    ) -> Result<CreateAgreementOutcome, Self::Error> {
        let accepted = AcceptedZecAgreementV1::resume(envelope)?;
        self.require_role(accepted.local_participant())?;
        let swap_id = accepted.agreement().coordinator().id();
        let accepted_at = sql_u64(envelope.accepted_at().value())?;
        let accepted_revision = sql_u64(envelope.revision())?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = load_agreement_row(&transaction, self.role_name(), swap_id)?;
        let outcome = match existing {
            None => {
                transaction.execute(
                    "
                    INSERT INTO zec_sdk_agreements (
                        local_role, swap_id, payload_version, agreement_wire,
                        accepted_at, accepted_revision, active_revision
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                    ",
                    params![
                        self.role_name(),
                        swap_id.as_str(),
                        AGREEMENT_PAYLOAD_VERSION,
                        envelope.agreement_wire(),
                        accepted_at,
                        accepted_revision
                    ],
                )?;
                CreateAgreementOutcome::Created
            }
            Some(row) => {
                require_payload(
                    "SDK agreement",
                    row.payload_version,
                    AGREEMENT_PAYLOAD_VERSION,
                )?;
                let _ = validated_agreement(&row, self.local_participant, swap_id)?;
                if row.agreement_wire == envelope.agreement_wire()
                    && row.accepted_at == accepted_at
                    && row.accepted_revision == accepted_revision
                {
                    CreateAgreementOutcome::ExistingSame
                } else {
                    CreateAgreementOutcome::Conflict
                }
            }
        };
        transaction.commit()?;
        Ok(outcome)
    }

    async fn load_agreement(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<AcceptedZecAgreementEnvelopeV1>, Self::Error> {
        let connection = self.connection()?;
        load_agreement_row(&connection, self.role_name(), swap_id)?
            .map(|row| validated_agreement(&row, self.local_participant, swap_id))
            .transpose()?
            .map(|accepted| accepted.durable_envelope().map_err(StoreError::from))
            .transpose()
    }

    async fn create_first_lock_intent(
        &self,
        intent: &FirstLockIntentV1,
    ) -> Result<CreateFirstLockOutcome, Self::Error> {
        self.require_role(intent.local_participant())?;
        let record = FirstLockIntentRecordV1::from(intent);
        let payload_version = i64::from(record.schema_version());
        let payload_json = serde_json::to_string(&record)?;
        let predecessor = sql_u64(intent.predecessor_revision())?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let agreement_row = load_agreement_row(&transaction, self.role_name(), intent.swap_id())?
            .ok_or(StoreError::MissingZecRecoveryAgreement)?;
        let active_revision = revision_from_sql(agreement_row.active_revision)?;
        let accepted =
            validated_agreement(&agreement_row, self.local_participant, intent.swap_id())?;
        let trusted = record.revalidate(&accepted, active_revision)?;
        if &trusted != intent {
            return Err(StoreError::InvalidZecRecoveryState);
        }

        let existing = load_intent_row(&transaction, self.role_name(), intent.swap_id())?;
        let outcome = match existing {
            None => {
                transaction.execute(
                    "
                    INSERT INTO zec_sdk_first_lock_intents (
                        local_role, swap_id, predecessor_revision,
                        payload_version, payload_json, closed_revision
                    ) VALUES (?1, ?2, ?3, ?4, ?5, NULL)
                    ",
                    params![
                        self.role_name(),
                        intent.swap_id().as_str(),
                        predecessor,
                        payload_version,
                        payload_json
                    ],
                )?;
                CreateFirstLockOutcome::Created
            }
            Some(row)
                if row.closed_revision.is_none()
                    && row.predecessor_revision == predecessor
                    && row.payload_version == payload_version
                    && row.payload_json == payload_json =>
            {
                CreateFirstLockOutcome::ExistingSame
            }
            Some(_) => CreateFirstLockOutcome::Conflict,
        };
        transaction.commit()?;
        Ok(outcome)
    }

    async fn load_first_lock_intent(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<FirstLockIntentV1>, Self::Error> {
        let connection = self.connection()?;
        let Some(intent_row) = load_intent_row(&connection, self.role_name(), swap_id)? else {
            return Ok(None);
        };
        if intent_row.closed_revision.is_some() {
            return Ok(None);
        }
        let agreement_row = load_agreement_row(&connection, self.role_name(), swap_id)?
            .ok_or(StoreError::MissingZecRecoveryAgreement)?;
        let active_revision = revision_from_sql(agreement_row.active_revision)?;
        let accepted = validated_agreement(&agreement_row, self.local_participant, swap_id)?;
        let record = decode_intent_record(intent_row.payload_version, &intent_row.payload_json)?;
        Ok(Some(record.revalidate(&accepted, active_revision)?))
    }

    async fn commit_first_lock_transition(
        &self,
        transition: &FirstLockTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        let record = FirstLockTransitionRecordV1::from(transition);
        let payload_version = i64::from(record.schema_version());
        let payload_json = serde_json::to_string(&record)?;
        let predecessor = transition.predecessor_revision();
        let committed = predecessor
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let predecessor_sql = sql_u64(predecessor)?;
        let committed_sql = sql_u64(committed)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let agreement_row =
            load_agreement_row(&transaction, self.role_name(), transition.swap_id())?
                .ok_or(StoreError::MissingZecRecoveryAgreement)?;
        let active_revision = revision_from_sql(agreement_row.active_revision)?;
        validate_transition_journal(
            &transaction,
            self.role_name(),
            transition.swap_id(),
            active_revision,
        )?;
        let accepted =
            validated_agreement(&agreement_row, self.local_participant, transition.swap_id())?;
        let intent_row = load_intent_row(&transaction, self.role_name(), transition.swap_id())?
            .ok_or(StoreError::MissingZecFirstLockIntent)?;
        let intent_record =
            decode_intent_record(intent_row.payload_version, &intent_row.payload_json)?;
        let trusted = record.revalidate(&accepted, &intent_record, predecessor)?;
        if &trusted != transition {
            return Err(StoreError::InvalidZecRecoveryState);
        }

        if let Some(existing) = load_transition_row(
            &transaction,
            self.role_name(),
            transition.swap_id(),
            predecessor_sql,
        )? {
            let replay = validate_transition_replay(
                &existing,
                &intent_row,
                &accepted,
                transition,
                active_revision,
                committed,
            )?;
            transaction.commit()?;
            return Ok(replay);
        }
        if active_revision != predecessor
            || intent_row.predecessor_revision != predecessor_sql
            || intent_row.closed_revision.is_some()
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }

        insert_taker_first_lock_transition(
            &transaction,
            self.role_name(),
            transition,
            predecessor_sql,
            committed_sql,
            payload_version,
            &payload_json,
        )?;
        transaction.commit()?;
        Ok(FirstLockProjectionCommit::new(committed, false))
    }

    async fn load_first_lock_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<FirstLockTransitionV1>, Self::Error> {
        let connection = self.connection()?;
        let predecessor_sql = sql_u64(predecessor_revision)?;
        let agreement_row = load_agreement_row(&connection, self.role_name(), swap_id)?
            .ok_or(StoreError::MissingZecRecoveryAgreement)?;
        let active_revision = revision_from_sql(agreement_row.active_revision)?;
        validate_transition_journal(&connection, self.role_name(), swap_id, active_revision)?;
        let accepted = validated_agreement(&agreement_row, self.local_participant, swap_id)?;
        let Some(transition_row) =
            load_transition_row(&connection, self.role_name(), swap_id, predecessor_sql)?
        else {
            if active_revision != predecessor_revision {
                return Err(StoreError::InvalidZecRecoveryState);
            }
            if let Some(intent_row) = load_intent_row(&connection, self.role_name(), swap_id)? {
                let intent_predecessor = revision_from_sql(intent_row.predecessor_revision)?;
                let intent_record =
                    decode_intent_record(intent_row.payload_version, &intent_row.payload_json)?;
                match intent_row.closed_revision {
                    None => {
                        if intent_row.predecessor_revision != predecessor_sql {
                            return Err(StoreError::InvalidZecRecoveryState);
                        }
                        let _ = intent_record.revalidate(&accepted, predecessor_revision)?;
                    }
                    Some(closed_revision) => {
                        let historical = load_transition_row(
                            &connection,
                            self.role_name(),
                            swap_id,
                            intent_row.predecessor_revision,
                        )?
                        .ok_or(StoreError::InvalidZecRecoveryState)?;
                        if historical.committed_revision != closed_revision
                            || agreement_row.active_revision < closed_revision
                        {
                            return Err(StoreError::InvalidZecRecoveryState);
                        }
                        let transition_record = decode_transition_record(
                            historical.payload_version,
                            &historical.payload_json,
                        )?;
                        let _ = transition_record.revalidate(
                            &accepted,
                            &intent_record,
                            intent_predecessor,
                        )?;
                    }
                }
            }
            return Ok(None);
        };
        let intent_row = load_intent_row(&connection, self.role_name(), swap_id)?
            .ok_or(StoreError::MissingZecFirstLockIntent)?;
        let expected_committed = predecessor_sql
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        if transition_row.committed_revision != expected_committed
            || intent_row.predecessor_revision != predecessor_sql
            || intent_row.closed_revision != Some(expected_committed)
            || agreement_row.active_revision < expected_committed
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let intent_record =
            decode_intent_record(intent_row.payload_version, &intent_row.payload_json)?;
        let transition_record =
            decode_transition_record(transition_row.payload_version, &transition_row.payload_json)?;
        Ok(Some(transition_record.revalidate(
            &accepted,
            &intent_record,
            predecessor_revision,
        )?))
    }

    async fn commit_observed_taker_first_lock_transition(
        &self,
        transition: &ObservedTakerFirstLockTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        self.require_role(transition.local_participant())?;
        if self.local_participant != Participant::Maker {
            return Err(StoreError::ZecRecoveryRoleMismatch);
        }
        let record = ObservedTakerFirstLockTransitionRecordV1::from(transition);
        let payload_version = i64::from(record.schema_version());
        let payload_json = serde_json::to_string(&record)?;
        let predecessor = transition.predecessor_revision();
        let committed = predecessor
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let predecessor_sql = sql_u64(predecessor)?;
        let committed_sql = sql_u64(committed)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (accepted, active_revision, current, mut trackers) = validated_maker_journal_head(
            &transaction,
            self.role_name(),
            self.local_participant,
            self.claim_key.as_deref(),
            transition.swap_id(),
        )?;
        let trusted = record.revalidate(&accepted, predecessor)?;
        if &trusted != transition
            || load_intent_row(&transaction, self.role_name(), transition.swap_id())?.is_some()
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }

        if let Some(existing) = load_transition_row(
            &transaction,
            self.role_name(),
            transition.swap_id(),
            predecessor_sql,
        )? {
            if existing.committed_revision != committed_sql
                || active_revision < committed
                || decode_observed_taker_lock_record(
                    existing.payload_version,
                    &existing.payload_json,
                    &accepted,
                )?
                .revalidate(&accepted, predecessor)?
                    != *transition
            {
                return Err(StoreError::ConflictingZecFirstLockTransition);
            }
            transaction.commit()?;
            return Ok(FirstLockProjectionCommit::new(committed, true));
        }
        if active_revision != predecessor {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        apply_selected_observation_event(&mut trackers, &trusted, active_revision == 0)?;
        let _ = trusted.apply_to(accepted.agreement(), &current, predecessor)?;
        transaction.execute(
            "
            INSERT INTO zec_sdk_first_lock_transitions (
                local_role, swap_id, predecessor_revision, committed_revision,
                payload_version, payload_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ",
            params![
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql,
                committed_sql,
                payload_version,
                payload_json
            ],
        )?;
        let updated = transaction.execute(
            "
            UPDATE zec_sdk_agreements
            SET active_revision = ?1
            WHERE local_role = ?2 AND swap_id = ?3 AND active_revision = ?4
            ",
            params![
                committed_sql,
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        transaction.commit()?;
        Ok(FirstLockProjectionCommit::new(committed, false))
    }

    async fn load_observed_taker_first_lock_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<ObservedTakerFirstLockTransitionV1>, Self::Error> {
        if self.local_participant != Participant::Maker {
            return Err(StoreError::ZecRecoveryRoleMismatch);
        }
        let connection = self.connection()?;
        let predecessor_sql = sql_u64(predecessor_revision)?;
        let (accepted, active_revision, _, _) = validated_maker_journal_head(
            &connection,
            self.role_name(),
            self.local_participant,
            self.claim_key.as_deref(),
            swap_id,
        )?;
        if load_intent_row(&connection, self.role_name(), swap_id)?.is_some() {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let Some(row) =
            load_transition_row(&connection, self.role_name(), swap_id, predecessor_sql)?
        else {
            if active_revision < predecessor_revision {
                return Err(StoreError::InvalidZecRecoveryState);
            }
            return Ok(None);
        };
        let committed = predecessor_revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        if row.committed_revision != sql_u64(committed)? || active_revision < committed {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let record =
            decode_observed_taker_lock_record(row.payload_version, &row.payload_json, &accepted)?;
        Ok(Some(record.revalidate(&accepted, predecessor_revision)?))
    }

    async fn commit_observed_maker_lock_transition(
        &self,
        transition: &ObservedMakerLockTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        self.require_role(transition.local_participant())?;
        if self.local_participant != Participant::Taker {
            return Err(StoreError::ZecRecoveryRoleMismatch);
        }
        let record = ObservedMakerLockTransitionRecordV1::from(transition);
        let payload_version = i64::from(record.schema_version());
        let payload_json = serde_json::to_string(&record)?;
        let predecessor = transition.predecessor_revision();
        let committed = predecessor
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let predecessor_sql = sql_u64(predecessor)?;
        let committed_sql = sql_u64(committed)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (accepted, active_revision, current) = validated_taker_journal_head(
            &transaction,
            self.role_name(),
            self.local_participant,
            self.claim_key.as_deref(),
            transition.swap_id(),
        )?;

        let trusted = record.revalidate(&accepted, predecessor)?;
        if &trusted != transition {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        if let Some(existing) = load_observed_maker_transition_row(
            &transaction,
            self.role_name(),
            transition.swap_id(),
            predecessor_sql,
        )? {
            let durable = decode_observed_maker_lock_record(
                existing.payload_version,
                &existing.payload_json,
            )?
            .revalidate(&accepted, predecessor)?;
            if existing.committed_revision != committed_sql
                || active_revision < committed
                || durable != *transition
            {
                return Err(StoreError::ConflictingZecFirstLockTransition);
            }
            transaction.commit()?;
            return Ok(FirstLockProjectionCommit::new(committed, true));
        }
        if active_revision != predecessor {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let _ = trusted.apply_to(accepted.agreement(), &current, predecessor)?;
        transaction.execute(
            "INSERT INTO zec_sdk_observed_maker_lock_transitions (
                local_role, swap_id, predecessor_revision, committed_revision,
                payload_version, payload_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql,
                committed_sql,
                payload_version,
                payload_json
            ],
        )?;
        let updated = transaction.execute(
            "UPDATE zec_sdk_agreements SET active_revision = ?1
             WHERE local_role = ?2 AND swap_id = ?3 AND active_revision = ?4",
            params![
                committed_sql,
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        transaction.commit()?;
        Ok(FirstLockProjectionCommit::new(committed, false))
    }

    async fn load_observed_maker_lock_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<ObservedMakerLockTransitionV1>, Self::Error> {
        if self.local_participant != Participant::Taker {
            return Err(StoreError::ZecRecoveryRoleMismatch);
        }
        let connection = self.connection()?;
        let (accepted, active_revision, _) = validated_taker_journal_head(
            &connection,
            self.role_name(),
            self.local_participant,
            self.claim_key.as_deref(),
            swap_id,
        )?;
        if predecessor_revision > active_revision {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let predecessor_sql = sql_u64(predecessor_revision)?;
        let Some(row) = load_observed_maker_transition_row(
            &connection,
            self.role_name(),
            swap_id,
            predecessor_sql,
        )?
        else {
            return Ok(None);
        };
        let committed = predecessor_revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        if row.committed_revision != sql_u64(committed)? || active_revision < committed {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let record = decode_observed_maker_lock_record(row.payload_version, &row.payload_json)?;
        Ok(Some(record.revalidate(&accepted, predecessor_revision)?))
    }

    async fn create_maker_lock_intent(
        &self,
        intent: &MakerLockIntentV1,
    ) -> Result<CreateFirstLockOutcome, Self::Error> {
        self.require_role(intent.local_participant())?;
        if self.local_participant != Participant::Maker {
            return Err(StoreError::ZecRecoveryRoleMismatch);
        }
        let record = MakerLockIntentRecordV1::from(intent);
        let payload_version = i64::from(record.schema_version());
        let payload_json = serde_json::to_string(&record)?;
        let staged_revision = sql_u64(intent.staged_revision())?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (accepted, active_revision, _, _) = validated_maker_journal_head(
            &transaction,
            self.role_name(),
            self.local_participant,
            self.claim_key.as_deref(),
            intent.swap_id(),
        )?;
        let trusted = record.revalidate(&accepted)?;
        if &trusted != intent {
            return Err(StoreError::InvalidZecRecoveryState);
        }

        let existing = load_maker_intent_row(&transaction, self.role_name(), intent.swap_id())?;
        let outcome = match existing {
            None if active_revision == intent.staged_revision() => {
                transaction.execute(
                    "
                    INSERT INTO zec_sdk_maker_lock_intents (
                        local_role, swap_id, staged_revision,
                        payload_version, payload_json, closed_revision
                    ) VALUES (?1, ?2, ?3, ?4, ?5, NULL)
                    ",
                    params![
                        self.role_name(),
                        intent.swap_id().as_str(),
                        staged_revision,
                        payload_version,
                        payload_json
                    ],
                )?;
                CreateFirstLockOutcome::Created
            }
            Some(row)
                if row.closed_revision.is_none()
                    && row.staged_revision == staged_revision
                    && row.payload_version == payload_version
                    && row.payload_json == payload_json =>
            {
                CreateFirstLockOutcome::ExistingSame
            }
            Some(_) | None => CreateFirstLockOutcome::Conflict,
        };
        transaction.commit()?;
        Ok(outcome)
    }

    async fn load_maker_lock_intent(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<MakerLockIntentV1>, Self::Error> {
        if self.local_participant != Participant::Maker {
            return Err(StoreError::ZecRecoveryRoleMismatch);
        }
        let connection = self.connection()?;
        let (accepted, active_revision, _, _) = validated_maker_journal_head(
            &connection,
            self.role_name(),
            self.local_participant,
            self.claim_key.as_deref(),
            swap_id,
        )?;
        let Some(row) = load_maker_intent_row(&connection, self.role_name(), swap_id)? else {
            return Ok(None);
        };
        if row.closed_revision.is_some() {
            return Ok(None);
        }
        let record = decode_maker_intent_record(row.payload_version, &row.payload_json)?;
        let intent = record.revalidate(&accepted)?;
        if intent.staged_revision() > active_revision
            || row.staged_revision != sql_u64(intent.staged_revision())?
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        Ok(Some(intent))
    }

    async fn commit_maker_lock_transition(
        &self,
        transition: &MakerLockTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        self.require_role(Participant::Maker)?;
        let record = MakerLockTransitionRecordV1::from(transition);
        let payload_version = i64::from(record.schema_version());
        let payload_json = serde_json::to_string(&record)?;
        let predecessor = transition.predecessor_revision();
        let committed = predecessor
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let predecessor_sql = sql_u64(predecessor)?;
        let committed_sql = sql_u64(committed)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (accepted, active_revision, current, _) = validated_maker_journal_head(
            &transaction,
            self.role_name(),
            self.local_participant,
            self.claim_key.as_deref(),
            transition.swap_id(),
        )?;
        let intent_row =
            load_maker_intent_row(&transaction, self.role_name(), transition.swap_id())?
                .ok_or(StoreError::MissingZecFirstLockIntent)?;
        let intent_record =
            decode_maker_intent_record(intent_row.payload_version, &intent_row.payload_json)?;
        let trusted = record.revalidate(&accepted, &intent_record, predecessor)?;
        if &trusted != transition {
            return Err(StoreError::InvalidZecRecoveryState);
        }

        if let Some(existing) = load_maker_transition_row(
            &transaction,
            self.role_name(),
            transition.swap_id(),
            predecessor_sql,
        )? {
            if existing.committed_revision != committed_sql
                || existing.intent_staged_revision != intent_row.staged_revision
                || intent_row.closed_revision != Some(committed_sql)
                || active_revision < committed
                || decode_maker_transition_record(existing.payload_version, &existing.payload_json)?
                    .revalidate(&accepted, &intent_record, predecessor)?
                    != *transition
            {
                return Err(StoreError::ConflictingZecFirstLockTransition);
            }
            transaction.commit()?;
            return Ok(FirstLockProjectionCommit::new(committed, true));
        }
        if active_revision != predecessor
            || intent_row.closed_revision.is_some()
            || intent_row.staged_revision != sql_u64(transition.intent_staged_revision())?
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let _ = trusted.apply_to(accepted.agreement(), &current, predecessor)?;
        transaction.execute(
            "
            INSERT INTO zec_sdk_maker_lock_transitions (
                local_role, swap_id, predecessor_revision, committed_revision,
                intent_staged_revision, payload_version, payload_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ",
            params![
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql,
                committed_sql,
                intent_row.staged_revision,
                payload_version,
                payload_json
            ],
        )?;
        let agreement_updates = transaction.execute(
            "UPDATE zec_sdk_agreements SET active_revision = ?1
             WHERE local_role = ?2 AND swap_id = ?3 AND active_revision = ?4",
            params![
                committed_sql,
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql
            ],
        )?;
        let intent_updates = transaction.execute(
            "UPDATE zec_sdk_maker_lock_intents SET closed_revision = ?1
             WHERE local_role = ?2 AND swap_id = ?3
               AND staged_revision = ?4 AND closed_revision IS NULL",
            params![
                committed_sql,
                self.role_name(),
                transition.swap_id().as_str(),
                intent_row.staged_revision
            ],
        )?;
        if agreement_updates != 1 || intent_updates != 1 {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        transaction.commit()?;
        Ok(FirstLockProjectionCommit::new(committed, false))
    }

    async fn load_maker_lock_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<MakerLockTransitionV1>, Self::Error> {
        if self.local_participant != Participant::Maker {
            return Err(StoreError::ZecRecoveryRoleMismatch);
        }
        let connection = self.connection()?;
        let (accepted, active_revision, _, _) = validated_maker_journal_head(
            &connection,
            self.role_name(),
            self.local_participant,
            self.claim_key.as_deref(),
            swap_id,
        )?;
        if predecessor_revision > active_revision {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let predecessor_sql = sql_u64(predecessor_revision)?;
        let Some(row) =
            load_maker_transition_row(&connection, self.role_name(), swap_id, predecessor_sql)?
        else {
            return Ok(None);
        };
        let intent_row = load_maker_intent_row(&connection, self.role_name(), swap_id)?
            .ok_or(StoreError::MissingZecFirstLockIntent)?;
        let committed = predecessor_revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        if row.committed_revision != sql_u64(committed)?
            || row.intent_staged_revision != intent_row.staged_revision
            || intent_row.closed_revision != Some(row.committed_revision)
            || active_revision < committed
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let intent_record =
            decode_maker_intent_record(intent_row.payload_version, &intent_row.payload_json)?;
        let transition_record =
            decode_maker_transition_record(row.payload_version, &row.payload_json)?;
        Ok(Some(transition_record.revalidate(
            &accepted,
            &intent_record,
            predecessor_revision,
        )?))
    }
}

// Each long method below is one auditable SQLite IMMEDIATE transaction whose
// statement order is part of its crash-safety invariant.
#[allow(clippy::too_many_lines)]
#[async_trait]
impl ClaimRecoveryStore for SqliteZecRecoveryStore {
    async fn create_agreement_with_local_claim_material(
        &self,
        envelope: &AcceptedZecAgreementEnvelopeV1,
        preimage: &ClaimPreimage,
    ) -> Result<CreateAgreementOutcome, Self::Error> {
        let key = self.claim_key()?;
        let accepted = AcceptedZecAgreementV1::resume(envelope)?;
        self.require_role(accepted.local_participant())?;
        if accepted.revision() != 0
            || accepted.local_participant() != accepted.agreement().lez_claimant()
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let swap_id = accepted.agreement().coordinator().id();
        let protected = ProtectedClaimEnvelope::encrypt(
            preimage,
            key,
            fresh_claim_nonce()?,
            claim_material_context(&accepted, ClaimMaterialPurpose::LocalFirstClaim),
        )?;
        let accepted_at = sql_u64(envelope.accepted_at().value())?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let agreement_row = load_agreement_row(&transaction, self.role_name(), swap_id)?;
        match agreement_row {
            None => {
                transaction.execute(
                    "INSERT INTO zec_sdk_agreements (
                        local_role, swap_id, payload_version, agreement_wire,
                        accepted_at, accepted_revision, active_revision
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 0, 0)",
                    params![
                        self.role_name(),
                        swap_id.as_str(),
                        AGREEMENT_PAYLOAD_VERSION,
                        envelope.agreement_wire(),
                        accepted_at
                    ],
                )?;
            }
            Some(row) => {
                let durable = validated_agreement(&row, self.local_participant, swap_id)?;
                if durable.durable_envelope()? != *envelope || row.active_revision != 0 {
                    transaction.commit()?;
                    return Ok(CreateAgreementOutcome::Conflict);
                }
            }
        }
        let outcome = match load_claim_material_row(&transaction, self.role_name(), swap_id)? {
            None => {
                insert_claim_material(
                    &transaction,
                    self.role_name(),
                    swap_id,
                    ClaimMaterialPurpose::LocalFirstClaim,
                    0,
                    &protected,
                )?;
                CreateAgreementOutcome::Created
            }
            Some(row)
                if row.created_revision == 0
                    && claim_material_purpose(&row.purpose)?
                        == ClaimMaterialPurpose::LocalFirstClaim =>
            {
                let existing = decrypt_claim_material(&row, &accepted, key)?;
                if existing.expose_secret() == preimage.expose_secret() {
                    CreateAgreementOutcome::ExistingSame
                } else {
                    CreateAgreementOutcome::Conflict
                }
            }
            Some(_) => CreateAgreementOutcome::Conflict,
        };
        transaction.commit()?;
        Ok(outcome)
    }

    async fn load_claim_material(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<ClaimPreimage>, Self::Error> {
        let key = self.claim_key()?;
        let connection = self.connection()?;
        let (accepted, _, _) = validated_claim_journal_head(
            &connection,
            self.role_name(),
            self.local_participant,
            key,
            swap_id,
        )?;
        load_claim_material_row(&connection, self.role_name(), swap_id)?
            .map(|row| decrypt_claim_material(&row, &accepted, key))
            .transpose()
    }

    async fn protect_claim_submission(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        local_participant: Participant,
        staged_revision: u64,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<ProtectedClaimPayloadEnvelope, Self::Error> {
        self.require_role(local_participant)?;
        ProtectedClaimPayloadEnvelope::encrypt(
            prepared.exact_submission(),
            self.claim_key()?,
            fresh_claim_nonce()?,
            prepared_claim_submission_context(
                agreement,
                local_participant,
                staged_revision,
                prepared,
            ),
        )
        .map_err(StoreError::from)
    }

    async fn open_claim_submission(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        intent: &ClaimIntentV1,
        protected: &ProtectedClaimPayloadEnvelope,
    ) -> Result<PreparedClaimSubmissionV1, Self::Error> {
        self.require_role(intent.local_participant())?;
        intent.validate_protected_payload_fingerprint(protected.fingerprint())?;
        let exact = protected.decrypt(
            self.claim_key()?,
            claim_submission_context(agreement, intent),
        )?;
        PreparedClaimSubmissionV1::new(
            intent.step(),
            *intent.expected_submission_id(),
            exact.to_vec(),
        )
        .map_err(StoreError::from)
    }

    async fn create_claim_intent(
        &self,
        intent: &ClaimIntentV1,
        protected: &ProtectedClaimPayloadEnvelope,
    ) -> Result<CreateFirstLockOutcome, Self::Error> {
        self.require_role(intent.local_participant())?;
        let key = self.claim_key()?;
        intent.validate_protected_payload_fingerprint(protected.fingerprint())?;
        let record = ClaimIntentRecordV1::from(intent);
        let payload_json = serde_json::to_string(&record)?;
        let staged_sql = sql_u64(intent.staged_revision())?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (accepted, active_revision, coordinator) = validated_claim_journal_head(
            &transaction,
            self.role_name(),
            self.local_participant,
            key,
            intent.swap_id(),
        )?;
        if active_revision != intent.staged_revision() {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let trusted = record.revalidate(&accepted, &coordinator, active_revision)?;
        if &trusted != intent {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let material = load_claim_material_row(&transaction, self.role_name(), intent.swap_id())?
            .ok_or(StoreError::MissingZecClaimMaterial)?;
        if revision_from_sql(material.created_revision)? > intent.staged_revision() {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let _ = decrypt_claim_material(&material, &accepted, key)?;
        let exact =
            protected.decrypt(key, claim_submission_context(accepted.agreement(), intent))?;
        let _ = PreparedClaimSubmissionV1::new(
            intent.step(),
            *intent.expected_submission_id(),
            exact.to_vec(),
        )?;
        let outcome = match load_claim_intent_row(&transaction, self.role_name(), intent.swap_id())?
        {
            None => {
                transaction.execute(
                    "INSERT INTO zec_sdk_claim_intents (
                            local_role, swap_id, staged_revision, material_created_revision,
                            payload_version, payload_json, protected_version,
                            protected_ciphertext, protected_nonce, protected_key_id,
                            protected_fingerprint, closed_revision
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL)",
                    params![
                        self.role_name(),
                        intent.swap_id().as_str(),
                        staged_sql,
                        material.created_revision,
                        i64::from(record.schema_version()),
                        payload_json,
                        i64::from(PROTECTED_CLAIM_SCHEMA_V1),
                        protected.ciphertext(),
                        protected.nonce().as_slice(),
                        protected.key_id(),
                        protected.fingerprint().as_slice()
                    ],
                )?;
                CreateFirstLockOutcome::Created
            }
            Some(existing) => {
                let existing_record = decode_claim_intent_record(&existing)?;
                let existing_protected = protected_claim_payload(&existing)?;
                if existing.closed_revision.is_none()
                    && existing_record == record
                    && existing_protected == *protected
                {
                    CreateFirstLockOutcome::ExistingSame
                } else {
                    CreateFirstLockOutcome::Conflict
                }
            }
        };
        transaction.commit()?;
        Ok(outcome)
    }

    async fn load_claim_intent(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<(ClaimIntentV1, ProtectedClaimPayloadEnvelope)>, Self::Error> {
        let key = self.claim_key()?;
        let connection = self.connection()?;
        let (accepted, active_revision, coordinator) = validated_claim_journal_head(
            &connection,
            self.role_name(),
            self.local_participant,
            key,
            swap_id,
        )?;
        let Some(row) = load_claim_intent_row(&connection, self.role_name(), swap_id)? else {
            return Ok(None);
        };
        if row.closed_revision.is_some() {
            return Ok(None);
        }
        let intent = decode_claim_intent_record(&row)?.revalidate(
            &accepted,
            &coordinator,
            active_revision,
        )?;
        let protected = protected_claim_payload(&row)?;
        intent.validate_protected_payload_fingerprint(protected.fingerprint())?;
        let _ = protected.decrypt(key, claim_submission_context(accepted.agreement(), &intent))?;
        Ok(Some((intent, protected)))
    }

    async fn commit_revealing_claim_transition(
        &self,
        transition: &RevealingClaimTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        let key = self.claim_key()?;
        self.require_role(self.local_participant)?;
        let record = RevealingClaimTransitionRecordV1::from(transition);
        let payload_json = serde_json::to_string(&record)?;
        let predecessor = transition.predecessor_revision();
        let committed = predecessor
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let predecessor_sql = sql_u64(predecessor)?;
        let committed_sql = sql_u64(committed)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (accepted, active_revision, coordinator) = validated_claim_journal_head(
            &transaction,
            self.role_name(),
            self.local_participant,
            key,
            transition.swap_id(),
        )?;
        let intent_row =
            load_claim_intent_row(&transaction, self.role_name(), transition.swap_id())?
                .ok_or(StoreError::MissingZecClaimIntent)?;
        if let Some(existing) = load_owned_claim_transition_row(
            &transaction,
            self.role_name(),
            transition.swap_id(),
            predecessor_sql,
        )? {
            if existing.transition_kind == "revealing_lez"
                && existing.committed_revision == committed_sql
                && existing.intent_staged_revision == Some(intent_row.staged_revision)
                && existing.payload_version == i64::from(record.schema_version())
                && existing.payload_json == payload_json
                && intent_row.closed_revision == Some(committed_sql)
                && active_revision >= committed
            {
                transaction.commit()?;
                return Ok(FirstLockProjectionCommit::new(committed, true));
            }
            return Err(StoreError::ConflictingZecClaimTransition);
        }
        if active_revision != predecessor
            || intent_row.closed_revision.is_some()
            || intent_row.staged_revision != sql_u64(transition.intent_staged_revision())?
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let intent_record = decode_claim_intent_record(&intent_row)?;
        let material =
            load_claim_material_row(&transaction, self.role_name(), transition.swap_id())?
                .ok_or(StoreError::MissingZecClaimMaterial)?;
        if material.created_revision != 0
            || claim_material_purpose(&material.purpose)? != ClaimMaterialPurpose::LocalFirstClaim
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let preimage = decrypt_claim_material(&material, &accepted, key)?;
        let trusted = record.revalidate(
            &accepted,
            &coordinator,
            &intent_record,
            predecessor,
            preimage,
        )?;
        if RevealingClaimTransitionRecordV1::from(&trusted) != record {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let _ = transition.apply_to(accepted.agreement(), &coordinator, predecessor)?;
        transaction.execute(
            "INSERT INTO zec_sdk_owned_claim_transitions (
                local_role, swap_id, transition_kind, predecessor_revision,
                committed_revision, intent_staged_revision, payload_version, payload_json
             ) VALUES (?1, ?2, 'revealing_lez', ?3, ?4, ?5, ?6, ?7)",
            params![
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql,
                committed_sql,
                intent_row.staged_revision,
                i64::from(record.schema_version()),
                payload_json
            ],
        )?;
        let intent_updates = transaction.execute(
            "UPDATE zec_sdk_claim_intents SET closed_revision = ?1
             WHERE local_role = ?2 AND swap_id = ?3
               AND staged_revision = ?4 AND closed_revision IS NULL",
            params![
                committed_sql,
                self.role_name(),
                transition.swap_id().as_str(),
                intent_row.staged_revision
            ],
        )?;
        let agreement_updates = transaction.execute(
            "UPDATE zec_sdk_agreements SET active_revision = ?1
             WHERE local_role = ?2 AND swap_id = ?3 AND active_revision = ?4",
            params![
                committed_sql,
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql
            ],
        )?;
        if intent_updates != 1 || agreement_updates != 1 {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        transaction.commit()?;
        Ok(FirstLockProjectionCommit::new(committed, false))
    }

    async fn load_revealing_claim_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<RevealingClaimTransitionV1>, Self::Error> {
        let key = self.claim_key()?;
        let connection = self.connection()?;
        let (accepted, active_revision, _) = validated_claim_journal_head(
            &connection,
            self.role_name(),
            self.local_participant,
            key,
            swap_id,
        )?;
        if predecessor_revision > active_revision {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let predecessor_sql = sql_u64(predecessor_revision)?;
        let Some(row) = load_owned_claim_transition_row(
            &connection,
            self.role_name(),
            swap_id,
            predecessor_sql,
        )?
        else {
            return Ok(None);
        };
        if row.transition_kind != "revealing_lez" {
            return Ok(None);
        }
        let intent_row = load_claim_intent_row(&connection, self.role_name(), swap_id)?
            .ok_or(StoreError::MissingZecClaimIntent)?;
        if row.intent_staged_revision != Some(intent_row.staged_revision)
            || intent_row.closed_revision != Some(row.committed_revision)
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let coordinator = claim_coordinator_at(
            &connection,
            self.role_name(),
            &accepted,
            predecessor_revision,
            key,
        )?;
        let material = load_claim_material_row(&connection, self.role_name(), swap_id)?
            .ok_or(StoreError::MissingZecClaimMaterial)?;
        let preimage = decrypt_claim_material(&material, &accepted, key)?;
        require_revealing_payload("SDK revealing claim transition", row.payload_version)?;
        let record: RevealingClaimTransitionRecordV1 = serde_json::from_str(&row.payload_json)?;
        record
            .revalidate(
                &accepted,
                &coordinator,
                &decode_claim_intent_record(&intent_row)?,
                predecessor_revision,
                preimage,
            )
            .map(Some)
            .map_err(StoreError::from)
    }

    async fn commit_observed_revealing_claim_transition(
        &self,
        transition: &ObservedRevealingClaimTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        let key = self.claim_key()?;
        let record = ObservedRevealingClaimTransitionRecordV1::from(transition);
        let payload_json = serde_json::to_string(&record)?;
        let predecessor = transition.predecessor_revision();
        let committed = predecessor
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let predecessor_sql = sql_u64(predecessor)?;
        let committed_sql = sql_u64(committed)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (accepted, active_revision, coordinator) = validated_claim_journal_head(
            &transaction,
            self.role_name(),
            self.local_participant,
            key,
            transition.swap_id(),
        )?;
        if let Some(existing) = load_observed_claim_transition_row(
            &transaction,
            self.role_name(),
            transition.swap_id(),
            predecessor_sql,
        )? {
            if existing.transition_kind == "observed_revealing_lez"
                && existing.committed_revision == committed_sql
                && existing.material_created_revision == Some(committed_sql)
                && existing.payload_version == i64::from(record.schema_version())
                && existing.payload_json == payload_json
                && active_revision >= committed
            {
                transaction.commit()?;
                return Ok(FirstLockProjectionCommit::new(committed, true));
            }
            return Err(StoreError::ConflictingZecClaimTransition);
        }
        if active_revision != predecessor
            || load_claim_material_row(&transaction, self.role_name(), transition.swap_id())?
                .is_some()
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let trusted = record.revalidate(
            &accepted,
            &coordinator,
            predecessor,
            ClaimPreimage::new(*transition.evidence().preimage().expose_secret()),
        )?;
        if ObservedRevealingClaimTransitionRecordV1::from(&trusted) != record {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let _ = transition.apply_to(accepted.agreement(), &coordinator, predecessor)?;
        let protected = ProtectedClaimEnvelope::encrypt(
            transition.evidence().preimage(),
            key,
            fresh_claim_nonce()?,
            claim_material_context(&accepted, ClaimMaterialPurpose::ObservedFollowUpClaim),
        )?;
        insert_claim_material(
            &transaction,
            self.role_name(),
            transition.swap_id(),
            ClaimMaterialPurpose::ObservedFollowUpClaim,
            committed_sql,
            &protected,
        )?;
        transaction.execute(
            "INSERT INTO zec_sdk_observed_claim_transitions (
                local_role, swap_id, transition_kind, predecessor_revision,
                committed_revision, material_created_revision, payload_version, payload_json
             ) VALUES (?1, ?2, 'observed_revealing_lez', ?3, ?4, ?4, ?5, ?6)",
            params![
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql,
                committed_sql,
                i64::from(record.schema_version()),
                payload_json
            ],
        )?;
        let agreement_updates = transaction.execute(
            "UPDATE zec_sdk_agreements SET active_revision = ?1
             WHERE local_role = ?2 AND swap_id = ?3 AND active_revision = ?4",
            params![
                committed_sql,
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql
            ],
        )?;
        if agreement_updates != 1 {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        transaction.commit()?;
        Ok(FirstLockProjectionCommit::new(committed, false))
    }

    async fn load_observed_revealing_claim_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<ObservedRevealingClaimTransitionV1>, Self::Error> {
        let key = self.claim_key()?;
        let connection = self.connection()?;
        let (accepted, active_revision, _) = validated_claim_journal_head(
            &connection,
            self.role_name(),
            self.local_participant,
            key,
            swap_id,
        )?;
        if predecessor_revision > active_revision {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let Some(row) = load_observed_claim_transition_row(
            &connection,
            self.role_name(),
            swap_id,
            sql_u64(predecessor_revision)?,
        )?
        else {
            return Ok(None);
        };
        if row.transition_kind != "observed_revealing_lez" {
            return Ok(None);
        }
        let coordinator = claim_coordinator_at(
            &connection,
            self.role_name(),
            &accepted,
            predecessor_revision,
            key,
        )?;
        let material = load_claim_material_row(&connection, self.role_name(), swap_id)?
            .ok_or(StoreError::MissingZecClaimMaterial)?;
        if material.created_revision != row.committed_revision
            || claim_material_purpose(&material.purpose)?
                != ClaimMaterialPurpose::ObservedFollowUpClaim
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let preimage = decrypt_claim_material(&material, &accepted, key)?;
        require_revealing_payload(
            "SDK observed revealing claim transition",
            row.payload_version,
        )?;
        let record: ObservedRevealingClaimTransitionRecordV1 =
            serde_json::from_str(&row.payload_json)?;
        record
            .revalidate(&accepted, &coordinator, predecessor_revision, preimage)
            .map(Some)
            .map_err(StoreError::from)
    }

    async fn commit_followup_claim_transition(
        &self,
        transition: &FollowupClaimTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        let key = self.claim_key()?;
        let record = FollowupClaimTransitionRecordV1::from(transition);
        let payload_json = serde_json::to_string(&record)?;
        let predecessor = transition.predecessor_revision();
        let committed = predecessor
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let predecessor_sql = sql_u64(predecessor)?;
        let committed_sql = sql_u64(committed)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (accepted, active_revision, coordinator) = validated_claim_journal_head(
            &transaction,
            self.role_name(),
            self.local_participant,
            key,
            transition.swap_id(),
        )?;
        let intent_row =
            load_claim_intent_row(&transaction, self.role_name(), transition.swap_id())?
                .ok_or(StoreError::MissingZecClaimIntent)?;
        if let Some(existing) = load_owned_claim_transition_row(
            &transaction,
            self.role_name(),
            transition.swap_id(),
            predecessor_sql,
        )? {
            if existing.transition_kind == "followup_zcash"
                && existing.committed_revision == committed_sql
                && existing.intent_staged_revision == Some(intent_row.staged_revision)
                && existing.payload_version == i64::from(record.schema_version())
                && existing.payload_json == payload_json
                && intent_row.closed_revision == Some(committed_sql)
                && active_revision >= committed
            {
                transaction.commit()?;
                return Ok(FirstLockProjectionCommit::new(committed, true));
            }
            return Err(StoreError::ConflictingZecClaimTransition);
        }
        if active_revision != predecessor
            || intent_row.closed_revision.is_some()
            || intent_row.staged_revision != sql_u64(transition.intent_staged_revision())?
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let intent_record = decode_claim_intent_record(&intent_row)?;
        let material =
            load_claim_material_row(&transaction, self.role_name(), transition.swap_id())?
                .ok_or(StoreError::MissingZecClaimMaterial)?;
        if claim_material_purpose(&material.purpose)? != ClaimMaterialPurpose::ObservedFollowUpClaim
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let _ = decrypt_claim_material(&material, &accepted, key)?;
        let trusted = record.revalidate(&accepted, &coordinator, &intent_record, predecessor)?;
        if &trusted != transition {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let _ = transition.apply_to(accepted.agreement(), &coordinator, predecessor)?;
        transaction.execute(
            "INSERT INTO zec_sdk_owned_claim_transitions (
                local_role, swap_id, transition_kind, predecessor_revision,
                committed_revision, intent_staged_revision, payload_version, payload_json
             ) VALUES (?1, ?2, 'followup_zcash', ?3, ?4, ?5, ?6, ?7)",
            params![
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql,
                committed_sql,
                intent_row.staged_revision,
                i64::from(record.schema_version()),
                payload_json
            ],
        )?;
        let intent_updates = transaction.execute(
            "UPDATE zec_sdk_claim_intents SET closed_revision = ?1
             WHERE local_role = ?2 AND swap_id = ?3
               AND staged_revision = ?4 AND closed_revision IS NULL",
            params![
                committed_sql,
                self.role_name(),
                transition.swap_id().as_str(),
                intent_row.staged_revision
            ],
        )?;
        let agreement_updates = transaction.execute(
            "UPDATE zec_sdk_agreements SET active_revision = ?1
             WHERE local_role = ?2 AND swap_id = ?3 AND active_revision = ?4",
            params![
                committed_sql,
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql
            ],
        )?;
        if intent_updates != 1 || agreement_updates != 1 {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        transaction.commit()?;
        Ok(FirstLockProjectionCommit::new(committed, false))
    }

    async fn load_followup_claim_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<FollowupClaimTransitionV1>, Self::Error> {
        let key = self.claim_key()?;
        let connection = self.connection()?;
        let (accepted, active_revision, _) = validated_claim_journal_head(
            &connection,
            self.role_name(),
            self.local_participant,
            key,
            swap_id,
        )?;
        if predecessor_revision > active_revision {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let Some(row) = load_owned_claim_transition_row(
            &connection,
            self.role_name(),
            swap_id,
            sql_u64(predecessor_revision)?,
        )?
        else {
            return Ok(None);
        };
        if row.transition_kind != "followup_zcash" {
            return Ok(None);
        }
        let intent_row = load_claim_intent_row(&connection, self.role_name(), swap_id)?
            .ok_or(StoreError::MissingZecClaimIntent)?;
        if row.intent_staged_revision != Some(intent_row.staged_revision)
            || intent_row.closed_revision != Some(row.committed_revision)
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let coordinator = claim_coordinator_at(
            &connection,
            self.role_name(),
            &accepted,
            predecessor_revision,
            key,
        )?;
        require_payload(
            "SDK follow-up claim transition",
            row.payload_version,
            i64::from(CLAIM_RECORD_SCHEMA_V1),
        )?;
        let record: FollowupClaimTransitionRecordV1 = serde_json::from_str(&row.payload_json)?;
        record
            .revalidate(
                &accepted,
                &coordinator,
                &decode_claim_intent_record(&intent_row)?,
                predecessor_revision,
            )
            .map(Some)
            .map_err(StoreError::from)
    }

    async fn commit_observed_followup_claim_transition(
        &self,
        transition: &ObservedFollowupClaimTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        let key = self.claim_key()?;
        let record = ObservedFollowupClaimTransitionRecordV1::from(transition);
        let payload_json = serde_json::to_string(&record)?;
        let predecessor = transition.predecessor_revision();
        let committed = predecessor
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let predecessor_sql = sql_u64(predecessor)?;
        let committed_sql = sql_u64(committed)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (accepted, active_revision, coordinator) = validated_claim_journal_head(
            &transaction,
            self.role_name(),
            self.local_participant,
            key,
            transition.swap_id(),
        )?;
        if let Some(existing) = load_observed_claim_transition_row(
            &transaction,
            self.role_name(),
            transition.swap_id(),
            predecessor_sql,
        )? {
            if existing.transition_kind == "observed_followup_zcash"
                && existing.committed_revision == committed_sql
                && existing.material_created_revision.is_none()
                && existing.payload_version == i64::from(record.schema_version())
                && existing.payload_json == payload_json
                && active_revision >= committed
            {
                transaction.commit()?;
                return Ok(FirstLockProjectionCommit::new(committed, true));
            }
            return Err(StoreError::ConflictingZecClaimTransition);
        }
        if active_revision != predecessor {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let trusted = record.revalidate(&accepted, &coordinator, predecessor)?;
        if &trusted != transition {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let _ = transition.apply_to(accepted.agreement(), &coordinator, predecessor)?;
        transaction.execute(
            "INSERT INTO zec_sdk_observed_claim_transitions (
                local_role, swap_id, transition_kind, predecessor_revision,
                committed_revision, material_created_revision, payload_version, payload_json
             ) VALUES (?1, ?2, 'observed_followup_zcash', ?3, ?4, NULL, ?5, ?6)",
            params![
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql,
                committed_sql,
                i64::from(record.schema_version()),
                payload_json
            ],
        )?;
        let updates = transaction.execute(
            "UPDATE zec_sdk_agreements SET active_revision = ?1
             WHERE local_role = ?2 AND swap_id = ?3 AND active_revision = ?4",
            params![
                committed_sql,
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql
            ],
        )?;
        if updates != 1 {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        transaction.commit()?;
        Ok(FirstLockProjectionCommit::new(committed, false))
    }

    async fn load_observed_followup_claim_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<ObservedFollowupClaimTransitionV1>, Self::Error> {
        let key = self.claim_key()?;
        let connection = self.connection()?;
        let (accepted, active_revision, _) = validated_claim_journal_head(
            &connection,
            self.role_name(),
            self.local_participant,
            key,
            swap_id,
        )?;
        if predecessor_revision > active_revision {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let Some(row) = load_observed_claim_transition_row(
            &connection,
            self.role_name(),
            swap_id,
            sql_u64(predecessor_revision)?,
        )?
        else {
            return Ok(None);
        };
        if row.transition_kind != "observed_followup_zcash" {
            return Ok(None);
        }
        let coordinator = claim_coordinator_at(
            &connection,
            self.role_name(),
            &accepted,
            predecessor_revision,
            key,
        )?;
        require_payload(
            "SDK observed follow-up claim transition",
            row.payload_version,
            i64::from(CLAIM_RECORD_SCHEMA_V1),
        )?;
        let record: ObservedFollowupClaimTransitionRecordV1 =
            serde_json::from_str(&row.payload_json)?;
        record
            .revalidate(&accepted, &coordinator, predecessor_revision)
            .map(Some)
            .map_err(StoreError::from)
    }
}

#[async_trait]
#[allow(clippy::too_many_lines)]
impl RefundRecoveryStore for SqliteZecRecoveryStore {
    async fn create_refund_intent(
        &self,
        intent: &RefundIntentV1,
    ) -> Result<CreateFirstLockOutcome, Self::Error> {
        self.require_role(intent.local_participant())?;
        let record = RefundIntentRecordV1::from(intent);
        let payload_json = serde_json::to_string(&record)?;
        let staged_sql = sql_u64(intent.staged_revision())?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (accepted, active_revision, coordinator) = validated_refund_journal_head(
            &transaction,
            self.role_name(),
            self.local_participant,
            self.claim_key.as_deref(),
            intent.swap_id(),
        )?;
        if active_revision != intent.staged_revision()
            || record.revalidate(&accepted, &coordinator, active_revision)? != *intent
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let outcome =
            match load_refund_intent_row(&transaction, self.role_name(), intent.swap_id())? {
                None => {
                    transaction.execute(
                        "INSERT INTO zec_sdk_refund_intents (
                            local_role, swap_id, staged_revision, payload_version, payload_json
                         ) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            self.role_name(),
                            intent.swap_id().as_str(),
                            staged_sql,
                            i64::from(record.schema_version()),
                            payload_json
                        ],
                    )?;
                    CreateFirstLockOutcome::Created
                }
                Some(existing)
                    if existing.staged_revision == staged_sql
                        && existing.payload_version == i64::from(record.schema_version())
                        && existing.payload_json == payload_json =>
                {
                    CreateFirstLockOutcome::ExistingSame
                }
                Some(_) => CreateFirstLockOutcome::Conflict,
            };
        transaction.commit()?;
        Ok(outcome)
    }

    async fn load_refund_intent(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<RefundIntentV1>, Self::Error> {
        let connection = self.connection()?;
        let row = load_refund_intent_row(&connection, self.role_name(), swap_id)?;
        if row.is_none() {
            if load_agreement_row(&connection, self.role_name(), swap_id)?.is_none() {
                return Ok(None);
            }
            let _ = validated_refund_journal_head(
                &connection,
                self.role_name(),
                self.local_participant,
                self.claim_key.as_deref(),
                swap_id,
            )?;
            return Ok(None);
        }
        let (accepted, active_revision, coordinator) = validated_refund_journal_head(
            &connection,
            self.role_name(),
            self.local_participant,
            self.claim_key.as_deref(),
            swap_id,
        )?;
        let row = row.ok_or(StoreError::InvalidZecRecoveryState)?;
        if revision_from_sql(row.staged_revision)? > active_revision {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let record = decode_refund_intent_record(row.payload_version, &row.payload_json)?;
        record
            .revalidate(&accepted, &coordinator, active_revision)
            .map(Some)
            .map_err(StoreError::from)
    }

    async fn commit_refund_transition(
        &self,
        transition: &RefundTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        self.require_role(transition.local_participant())?;
        let record = RefundTransitionRecordV1::from(transition);
        let payload_json = serde_json::to_string(&record)?;
        let predecessor = transition.predecessor_revision();
        let committed = predecessor
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let predecessor_sql = sql_u64(predecessor)?;
        let committed_sql = sql_u64(committed)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (accepted, active_revision, coordinator) = validated_refund_journal_head(
            &transaction,
            self.role_name(),
            self.local_participant,
            self.claim_key.as_deref(),
            transition.swap_id(),
        )?;

        if let Some(existing) = load_refund_transition_row(
            &transaction,
            self.role_name(),
            transition.swap_id(),
            predecessor_sql,
        )? {
            let predecessor_coordinator = refund_coordinator_at(
                &transaction,
                self.role_name(),
                &accepted,
                predecessor,
                self.claim_key.as_deref(),
            )?;
            let retained = decode_retained_refund_intent(&existing)?;
            let durable =
                decode_refund_transition_record(existing.payload_version, &existing.payload_json)?
                    .revalidate(
                        &accepted,
                        &predecessor_coordinator,
                        predecessor,
                        retained.as_ref(),
                    )?;
            if existing.committed_revision != committed_sql
                || active_revision < committed
                || existing.transition_kind
                    != if transition.is_owned() {
                        "owned"
                    } else {
                        "observed"
                    }
                || existing.payload_version != i64::from(record.schema_version())
                || existing.payload_json != payload_json
                || durable != *transition
            {
                return Err(StoreError::ConflictingZecRefundTransition);
            }
            transaction.commit()?;
            return Ok(FirstLockProjectionCommit::new(committed, true));
        }
        if active_revision != predecessor {
            return Err(StoreError::InvalidZecRecoveryState);
        }

        let intent_row =
            load_refund_intent_row(&transaction, self.role_name(), transition.swap_id())?;
        let retained_record = match (transition.is_owned(), intent_row.as_ref()) {
            (true, Some(row)) => Some(decode_refund_intent_record(
                row.payload_version,
                &row.payload_json,
            )?),
            (false, None) => None,
            _ => return Err(StoreError::InvalidZecRecoveryState),
        };
        let trusted = record.revalidate(
            &accepted,
            &coordinator,
            predecessor,
            retained_record.as_ref(),
        )?;
        if trusted != *transition {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let _ = trusted.apply_to(accepted.agreement(), &coordinator, predecessor)?;
        let (kind, retained_version, retained_json, staged_revision) = match intent_row.as_ref() {
            Some(row) => (
                "owned",
                Some(row.payload_version),
                Some(row.payload_json.as_str()),
                Some(row.staged_revision),
            ),
            None => ("observed", None, None, None),
        };
        transaction.execute(
            "INSERT INTO zec_sdk_refund_transitions (
                local_role, swap_id, predecessor_revision, committed_revision,
                transition_kind, payload_version, payload_json,
                retained_intent_version, retained_intent_json, intent_staged_revision
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql,
                committed_sql,
                kind,
                i64::from(record.schema_version()),
                payload_json,
                retained_version,
                retained_json,
                staged_revision
            ],
        )?;
        let agreement_updates = transaction.execute(
            "UPDATE zec_sdk_agreements SET active_revision = ?1
             WHERE local_role = ?2 AND swap_id = ?3 AND active_revision = ?4",
            params![
                committed_sql,
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql
            ],
        )?;
        let intent_updates = if let Some(row) = intent_row {
            transaction.execute(
                "DELETE FROM zec_sdk_refund_intents
                 WHERE local_role = ?1 AND swap_id = ?2 AND staged_revision = ?3
                   AND payload_version = ?4 AND payload_json = ?5",
                params![
                    self.role_name(),
                    transition.swap_id().as_str(),
                    row.staged_revision,
                    row.payload_version,
                    row.payload_json
                ],
            )?
        } else {
            0
        };
        if agreement_updates != 1 || (transition.is_owned() && intent_updates != 1) {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        transaction.commit()?;
        Ok(FirstLockProjectionCommit::new(committed, false))
    }

    async fn load_refund_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<RefundTransitionV1>, Self::Error> {
        let connection = self.connection()?;
        let predecessor_sql = sql_u64(predecessor_revision)?;
        let row =
            load_refund_transition_row(&connection, self.role_name(), swap_id, predecessor_sql)?;
        if row.is_none() {
            if load_agreement_row(&connection, self.role_name(), swap_id)?.is_none() {
                return Ok(None);
            }
            let _ = validated_refund_journal_head(
                &connection,
                self.role_name(),
                self.local_participant,
                self.claim_key.as_deref(),
                swap_id,
            )?;
            return Ok(None);
        }
        let (accepted, active_revision, _) = validated_refund_journal_head(
            &connection,
            self.role_name(),
            self.local_participant,
            self.claim_key.as_deref(),
            swap_id,
        )?;
        if predecessor_revision > active_revision {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let row = row.ok_or(StoreError::InvalidZecRecoveryState)?;
        let committed = predecessor_revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        if row.committed_revision != sql_u64(committed)? || active_revision < committed {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let coordinator = refund_coordinator_at(
            &connection,
            self.role_name(),
            &accepted,
            predecessor_revision,
            self.claim_key.as_deref(),
        )?;
        let retained = decode_retained_refund_intent(&row)?;
        decode_refund_transition_record(row.payload_version, &row.payload_json)?
            .revalidate(
                &accepted,
                &coordinator,
                predecessor_revision,
                retained.as_ref(),
            )
            .map(Some)
            .map_err(StoreError::from)
    }
}

fn insert_taker_first_lock_transition(
    transaction: &rusqlite::Transaction<'_>,
    role: &str,
    transition: &FirstLockTransitionV1,
    predecessor: i64,
    committed: i64,
    payload_version: i64,
    payload_json: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "
        INSERT INTO zec_sdk_first_lock_transitions (
            local_role, swap_id, predecessor_revision, committed_revision,
            payload_version, payload_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ",
        params![
            role,
            transition.swap_id().as_str(),
            predecessor,
            committed,
            payload_version,
            payload_json
        ],
    )?;
    let agreement_updates = transaction.execute(
        "
        UPDATE zec_sdk_agreements
        SET active_revision = ?1
        WHERE local_role = ?2 AND swap_id = ?3 AND active_revision = ?4
        ",
        params![committed, role, transition.swap_id().as_str(), predecessor],
    )?;
    let intent_updates = transaction.execute(
        "
        UPDATE zec_sdk_first_lock_intents
        SET closed_revision = ?1
        WHERE local_role = ?2 AND swap_id = ?3
          AND predecessor_revision = ?4 AND closed_revision IS NULL
        ",
        params![committed, role, transition.swap_id().as_str(), predecessor],
    )?;
    if agreement_updates == 1 && intent_updates == 1 {
        Ok(())
    } else {
        Err(StoreError::InvalidZecRecoveryState)
    }
}

#[derive(Debug)]
struct AgreementRow {
    payload_version: i64,
    agreement_wire: Vec<u8>,
    accepted_at: i64,
    accepted_revision: i64,
    active_revision: i64,
}

#[derive(Debug)]
struct IntentRow {
    predecessor_revision: i64,
    payload_version: i64,
    payload_json: String,
    closed_revision: Option<i64>,
}

#[derive(Debug)]
struct TransitionRow {
    committed_revision: i64,
    payload_version: i64,
    payload_json: String,
}

#[derive(Debug)]
struct MakerIntentRow {
    staged_revision: i64,
    payload_version: i64,
    payload_json: String,
    closed_revision: Option<i64>,
}

#[derive(Debug)]
struct MakerTransitionRow {
    committed_revision: i64,
    intent_staged_revision: i64,
    payload_version: i64,
    payload_json: String,
}

#[derive(Debug)]
struct ClaimMaterialRow {
    purpose: String,
    created_revision: i64,
    envelope_version: i64,
    ciphertext: Vec<u8>,
    nonce: Vec<u8>,
    key_id: String,
    fingerprint: Vec<u8>,
}

#[derive(Debug)]
struct ClaimIntentRow {
    staged_revision: i64,
    material_created_revision: i64,
    payload_version: i64,
    payload_json: String,
    protected_version: i64,
    protected_ciphertext: Vec<u8>,
    protected_nonce: Vec<u8>,
    protected_key_id: String,
    protected_fingerprint: Vec<u8>,
    closed_revision: Option<i64>,
}

#[derive(Debug)]
struct ClaimTransitionRow {
    transition_kind: String,
    committed_revision: i64,
    intent_staged_revision: Option<i64>,
    material_created_revision: Option<i64>,
    payload_version: i64,
    payload_json: String,
}

#[derive(Debug)]
struct RefundIntentRow {
    staged_revision: i64,
    payload_version: i64,
    payload_json: String,
}

#[derive(Debug)]
struct RefundTransitionRow {
    committed_revision: i64,
    transition_kind: String,
    payload_version: i64,
    payload_json: String,
    retained_intent_version: Option<i64>,
    retained_intent_json: Option<String>,
    intent_staged_revision: Option<i64>,
}

fn load_agreement_row(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
) -> Result<Option<AgreementRow>, StoreError> {
    connection
        .query_row(
            "
            SELECT payload_version, agreement_wire, accepted_at,
                   accepted_revision, active_revision
            FROM zec_sdk_agreements
            WHERE local_role = ?1 AND swap_id = ?2
            ",
            params![role, swap_id.as_str()],
            |row| {
                Ok(AgreementRow {
                    payload_version: row.get(0)?,
                    agreement_wire: row.get(1)?,
                    accepted_at: row.get(2)?,
                    accepted_revision: row.get(3)?,
                    active_revision: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

fn load_intent_row(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
) -> Result<Option<IntentRow>, StoreError> {
    connection
        .query_row(
            "
            SELECT predecessor_revision, payload_version, payload_json, closed_revision
            FROM zec_sdk_first_lock_intents
            WHERE local_role = ?1 AND swap_id = ?2
            ",
            params![role, swap_id.as_str()],
            |row| {
                Ok(IntentRow {
                    predecessor_revision: row.get(0)?,
                    payload_version: row.get(1)?,
                    payload_json: row.get(2)?,
                    closed_revision: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

fn load_transition_row(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
    predecessor_revision: i64,
) -> Result<Option<TransitionRow>, StoreError> {
    connection
        .query_row(
            "
            SELECT committed_revision, payload_version, payload_json
            FROM zec_sdk_first_lock_transitions
            WHERE local_role = ?1 AND swap_id = ?2 AND predecessor_revision = ?3
            ",
            params![role, swap_id.as_str(), predecessor_revision],
            |row| {
                Ok(TransitionRow {
                    committed_revision: row.get(0)?,
                    payload_version: row.get(1)?,
                    payload_json: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

fn load_maker_intent_row(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
) -> Result<Option<MakerIntentRow>, StoreError> {
    connection
        .query_row(
            "
            SELECT staged_revision, payload_version, payload_json, closed_revision
            FROM zec_sdk_maker_lock_intents
            WHERE local_role = ?1 AND swap_id = ?2
            ",
            params![role, swap_id.as_str()],
            |row| {
                Ok(MakerIntentRow {
                    staged_revision: row.get(0)?,
                    payload_version: row.get(1)?,
                    payload_json: row.get(2)?,
                    closed_revision: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

fn load_maker_transition_row(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
    predecessor_revision: i64,
) -> Result<Option<MakerTransitionRow>, StoreError> {
    connection
        .query_row(
            "
            SELECT committed_revision, intent_staged_revision,
                   payload_version, payload_json
            FROM zec_sdk_maker_lock_transitions
            WHERE local_role = ?1 AND swap_id = ?2 AND predecessor_revision = ?3
            ",
            params![role, swap_id.as_str(), predecessor_revision],
            |row| {
                Ok(MakerTransitionRow {
                    committed_revision: row.get(0)?,
                    intent_staged_revision: row.get(1)?,
                    payload_version: row.get(2)?,
                    payload_json: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

fn load_observed_maker_transition_row(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
    predecessor_revision: i64,
) -> Result<Option<TransitionRow>, StoreError> {
    connection
        .query_row(
            "SELECT committed_revision, payload_version, payload_json
             FROM zec_sdk_observed_maker_lock_transitions
             WHERE local_role = ?1 AND swap_id = ?2 AND predecessor_revision = ?3",
            params![role, swap_id.as_str(), predecessor_revision],
            |row| {
                Ok(TransitionRow {
                    committed_revision: row.get(0)?,
                    payload_version: row.get(1)?,
                    payload_json: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

fn load_claim_material_row(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
) -> Result<Option<ClaimMaterialRow>, StoreError> {
    connection
        .query_row(
            "SELECT purpose, created_revision, envelope_version, ciphertext,
                    nonce, key_id, fingerprint
             FROM zec_sdk_claim_materials
             WHERE local_role = ?1 AND swap_id = ?2",
            params![role, swap_id.as_str()],
            |row| {
                Ok(ClaimMaterialRow {
                    purpose: row.get(0)?,
                    created_revision: row.get(1)?,
                    envelope_version: row.get(2)?,
                    ciphertext: row.get(3)?,
                    nonce: row.get(4)?,
                    key_id: row.get(5)?,
                    fingerprint: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

fn load_claim_intent_row(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
) -> Result<Option<ClaimIntentRow>, StoreError> {
    connection
        .query_row(
            "SELECT staged_revision, material_created_revision,
                    payload_version, payload_json, protected_version,
                    protected_ciphertext, protected_nonce, protected_key_id,
                    protected_fingerprint, closed_revision
             FROM zec_sdk_claim_intents
             WHERE local_role = ?1 AND swap_id = ?2",
            params![role, swap_id.as_str()],
            |row| {
                Ok(ClaimIntentRow {
                    staged_revision: row.get(0)?,
                    material_created_revision: row.get(1)?,
                    payload_version: row.get(2)?,
                    payload_json: row.get(3)?,
                    protected_version: row.get(4)?,
                    protected_ciphertext: row.get(5)?,
                    protected_nonce: row.get(6)?,
                    protected_key_id: row.get(7)?,
                    protected_fingerprint: row.get(8)?,
                    closed_revision: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

fn load_owned_claim_transition_row(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
    predecessor_revision: i64,
) -> Result<Option<ClaimTransitionRow>, StoreError> {
    connection
        .query_row(
            "SELECT transition_kind, committed_revision, intent_staged_revision,
                    payload_version, payload_json
             FROM zec_sdk_owned_claim_transitions
             WHERE local_role = ?1 AND swap_id = ?2 AND predecessor_revision = ?3",
            params![role, swap_id.as_str(), predecessor_revision],
            |row| {
                Ok(ClaimTransitionRow {
                    transition_kind: row.get(0)?,
                    committed_revision: row.get(1)?,
                    intent_staged_revision: Some(row.get(2)?),
                    material_created_revision: None,
                    payload_version: row.get(3)?,
                    payload_json: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

fn load_observed_claim_transition_row(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
    predecessor_revision: i64,
) -> Result<Option<ClaimTransitionRow>, StoreError> {
    connection
        .query_row(
            "SELECT transition_kind, committed_revision, material_created_revision,
                    payload_version, payload_json
             FROM zec_sdk_observed_claim_transitions
             WHERE local_role = ?1 AND swap_id = ?2 AND predecessor_revision = ?3",
            params![role, swap_id.as_str(), predecessor_revision],
            |row| {
                Ok(ClaimTransitionRow {
                    transition_kind: row.get(0)?,
                    committed_revision: row.get(1)?,
                    intent_staged_revision: None,
                    material_created_revision: row.get(2)?,
                    payload_version: row.get(3)?,
                    payload_json: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

fn load_refund_intent_row(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
) -> Result<Option<RefundIntentRow>, StoreError> {
    connection
        .query_row(
            "SELECT staged_revision, payload_version, payload_json
             FROM zec_sdk_refund_intents
             WHERE local_role = ?1 AND swap_id = ?2",
            params![role, swap_id.as_str()],
            |row| {
                Ok(RefundIntentRow {
                    staged_revision: row.get(0)?,
                    payload_version: row.get(1)?,
                    payload_json: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

fn load_refund_transition_row(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
    predecessor_revision: i64,
) -> Result<Option<RefundTransitionRow>, StoreError> {
    connection
        .query_row(
            "SELECT committed_revision, transition_kind, payload_version, payload_json,
                    retained_intent_version, retained_intent_json, intent_staged_revision
             FROM zec_sdk_refund_transitions
             WHERE local_role = ?1 AND swap_id = ?2 AND predecessor_revision = ?3",
            params![role, swap_id.as_str(), predecessor_revision],
            |row| {
                Ok(RefundTransitionRow {
                    committed_revision: row.get(0)?,
                    transition_kind: row.get(1)?,
                    payload_version: row.get(2)?,
                    payload_json: row.get(3)?,
                    retained_intent_version: row.get(4)?,
                    retained_intent_json: row.get(5)?,
                    intent_staged_revision: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

fn validate_transition_journal(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
    active_revision: u64,
) -> Result<(), StoreError> {
    let (count, distinct, minimum, maximum, invalid_commits): (
        i64,
        i64,
        Option<i64>,
        Option<i64>,
        i64,
    ) = connection
        .query_row(
            "
            SELECT COUNT(*), COUNT(DISTINCT predecessor_revision),
                   MIN(predecessor_revision), MAX(predecessor_revision),
                   COALESCE(SUM(
                       committed_revision != predecessor_revision + 1
                   ), 0)
            FROM (
                SELECT predecessor_revision, committed_revision
                FROM zec_sdk_first_lock_transitions
                WHERE local_role = ?1 AND swap_id = ?2
                UNION ALL
                SELECT predecessor_revision, committed_revision
                FROM zec_sdk_maker_lock_transitions
                WHERE local_role = ?1 AND swap_id = ?2
                UNION ALL
                SELECT predecessor_revision, committed_revision
                FROM zec_sdk_observed_maker_lock_transitions
                WHERE local_role = ?1 AND swap_id = ?2
                UNION ALL
                SELECT predecessor_revision, committed_revision
                FROM zec_sdk_owned_claim_transitions
                WHERE local_role = ?1 AND swap_id = ?2
                UNION ALL
                SELECT predecessor_revision, committed_revision
                FROM zec_sdk_observed_claim_transitions
                WHERE local_role = ?1 AND swap_id = ?2
                UNION ALL
                SELECT predecessor_revision, committed_revision
                FROM zec_sdk_refund_transitions
                WHERE local_role = ?1 AND swap_id = ?2
            )
            ",
            params![role, swap_id.as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(StoreError::from)?;
    let active = sql_u64(active_revision)?;
    let expected_maximum = active.checked_sub(1);
    if count == active
        && distinct == count
        && invalid_commits == 0
        && ((active == 0 && minimum.is_none() && maximum.is_none())
            || (active > 0 && minimum == Some(0) && maximum == expected_maximum))
    {
        Ok(())
    } else {
        Err(StoreError::InvalidZecRecoveryState)
    }
}

fn replay_maker_journal(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
    accepted: &AcceptedZecAgreementV1,
    active_revision: u64,
    claim_key: Option<&ProtectedClaimKey>,
) -> Result<(SwapCoordinator, MakerObservationTrackers), StoreError> {
    let mut coordinator = accepted.agreement().coordinator().clone();
    let mut trackers = MakerObservationTrackers::default();
    for predecessor in 0..active_revision {
        let predecessor_sql = sql_u64(predecessor)?;
        let observation = load_transition_row(connection, role, swap_id, predecessor_sql)?;
        let maker = load_maker_transition_row(connection, role, swap_id, predecessor_sql)?;
        let committed = predecessor
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let committed_sql = sql_u64(committed)?;
        match (observation, maker) {
            (Some(row), None) if row.committed_revision == committed_sql => {
                let record = decode_observed_taker_lock_record(
                    row.payload_version,
                    &row.payload_json,
                    accepted,
                )?;
                let transition = record.revalidate(accepted, predecessor)?;
                apply_selected_observation_event(&mut trackers, &transition, predecessor == 0)?;
                coordinator =
                    transition.apply_to(accepted.agreement(), &coordinator, predecessor)?;
            }
            (None, Some(row)) if row.committed_revision == committed_sql => {
                let intent_row = load_maker_intent_row(connection, role, swap_id)?
                    .ok_or(StoreError::MissingZecFirstLockIntent)?;
                if row.intent_staged_revision != intent_row.staged_revision
                    || intent_row.closed_revision != Some(committed_sql)
                {
                    return Err(StoreError::InvalidZecRecoveryState);
                }
                let intent = decode_maker_intent_record(
                    intent_row.payload_version,
                    &intent_row.payload_json,
                )?;
                let record =
                    decode_maker_transition_record(row.payload_version, &row.payload_json)?;
                let transition = record.revalidate(accepted, &intent, predecessor)?;
                coordinator =
                    transition.apply_to(accepted.agreement(), &coordinator, predecessor)?;
            }
            (None, None) => {
                coordinator = apply_terminal_journal_slot(
                    connection,
                    role,
                    swap_id,
                    accepted,
                    &coordinator,
                    predecessor,
                    committed_sql,
                    claim_key,
                )?;
            }
            _ => return Err(StoreError::InvalidZecRecoveryState),
        }
    }
    Ok((coordinator, trackers))
}

fn replay_taker_journal(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
    accepted: &AcceptedZecAgreementV1,
    active_revision: u64,
    claim_key: Option<&ProtectedClaimKey>,
) -> Result<SwapCoordinator, StoreError> {
    let mut coordinator = accepted.agreement().coordinator().clone();
    for predecessor in 0..active_revision {
        let predecessor_sql = sql_u64(predecessor)?;
        let first = load_transition_row(connection, role, swap_id, predecessor_sql)?;
        let observed =
            load_observed_maker_transition_row(connection, role, swap_id, predecessor_sql)?;
        let committed = predecessor
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let committed_sql = sql_u64(committed)?;
        match (first, observed) {
            (Some(row), None) if row.committed_revision == committed_sql => {
                let intent_row = load_intent_row(connection, role, swap_id)?
                    .ok_or(StoreError::MissingZecFirstLockIntent)?;
                if intent_row.predecessor_revision != predecessor_sql
                    || intent_row.closed_revision != Some(committed_sql)
                {
                    return Err(StoreError::InvalidZecRecoveryState);
                }
                let intent =
                    decode_intent_record(intent_row.payload_version, &intent_row.payload_json)?;
                let record = decode_transition_record(row.payload_version, &row.payload_json)?;
                let transition = record.revalidate(accepted, &intent, predecessor)?;
                coordinator = transition
                    .apply_to(accepted.agreement(), &coordinator, predecessor)
                    .map_err(|_| StoreError::InvalidZecRecoveryState)?;
            }
            (None, Some(row)) if row.committed_revision == committed_sql => {
                let record =
                    decode_observed_maker_lock_record(row.payload_version, &row.payload_json)?;
                let transition = record.revalidate(accepted, predecessor)?;
                coordinator =
                    transition.apply_to(accepted.agreement(), &coordinator, predecessor)?;
            }
            (None, None) => {
                coordinator = apply_terminal_journal_slot(
                    connection,
                    role,
                    swap_id,
                    accepted,
                    &coordinator,
                    predecessor,
                    committed_sql,
                    claim_key,
                )?;
            }
            _ => return Err(StoreError::InvalidZecRecoveryState),
        }
    }
    Ok(coordinator)
}

fn apply_selected_observation_event(
    trackers: &mut MakerObservationTrackers,
    transition: &ObservedTakerFirstLockTransitionV1,
    allow_initial_adapter_assertion: bool,
) -> Result<(), StoreError> {
    match (
        transition.lez_observation_event(),
        transition.zcash_observation_event(),
    ) {
        (Some(event), None) => trackers
            .lez
            .apply_committed(&event)
            .map_err(|_| StoreError::InvalidZecRecoveryState),
        (None, Some(event)) => trackers
            .zcash
            .apply_committed(&event)
            .map_err(|_| StoreError::InvalidZecRecoveryState),
        (None, None) if allow_initial_adapter_assertion => Ok(()),
        (None, None) | (Some(_), Some(_)) => Err(StoreError::InvalidZecRecoveryState),
    }
}

fn validated_maker_journal_head(
    connection: &Connection,
    role: &str,
    local_participant: Participant,
    claim_key: Option<&ProtectedClaimKey>,
    swap_id: &SwapId,
) -> Result<
    (
        AcceptedZecAgreementV1,
        u64,
        SwapCoordinator,
        MakerObservationTrackers,
    ),
    StoreError,
> {
    let agreement_row = load_agreement_row(connection, role, swap_id)?
        .ok_or(StoreError::MissingZecRecoveryAgreement)?;
    let active_revision = revision_from_sql(agreement_row.active_revision)?;
    let accepted = validated_agreement(&agreement_row, local_participant, swap_id)?;
    validate_transition_journal(connection, role, swap_id, active_revision)?;
    let (coordinator, tracker) = replay_maker_journal(
        connection,
        role,
        swap_id,
        &accepted,
        active_revision,
        claim_key,
    )?;
    if let Some(key) = claim_key {
        validate_claim_auxiliary_state(
            connection,
            role,
            &accepted,
            active_revision,
            &coordinator,
            key,
        )?;
    }
    Ok((accepted, active_revision, coordinator, tracker))
}

fn validated_taker_journal_head(
    connection: &Connection,
    role: &str,
    local_participant: Participant,
    claim_key: Option<&ProtectedClaimKey>,
    swap_id: &SwapId,
) -> Result<(AcceptedZecAgreementV1, u64, SwapCoordinator), StoreError> {
    let agreement_row = load_agreement_row(connection, role, swap_id)?
        .ok_or(StoreError::MissingZecRecoveryAgreement)?;
    let active_revision = revision_from_sql(agreement_row.active_revision)?;
    let accepted = validated_agreement(&agreement_row, local_participant, swap_id)?;
    validate_transition_journal(connection, role, swap_id, active_revision)?;
    let coordinator = replay_taker_journal(
        connection,
        role,
        swap_id,
        &accepted,
        active_revision,
        claim_key,
    )?;
    if let Some(key) = claim_key {
        validate_claim_auxiliary_state(
            connection,
            role,
            &accepted,
            active_revision,
            &coordinator,
            key,
        )?;
    }
    Ok((accepted, active_revision, coordinator))
}

fn validated_claim_journal_head(
    connection: &Connection,
    role: &str,
    local_participant: Participant,
    key: &ProtectedClaimKey,
    swap_id: &SwapId,
) -> Result<(AcceptedZecAgreementV1, u64, SwapCoordinator), StoreError> {
    match local_participant {
        Participant::Maker => {
            let (accepted, revision, coordinator, _) = validated_maker_journal_head(
                connection,
                role,
                local_participant,
                Some(key),
                swap_id,
            )?;
            Ok((accepted, revision, coordinator))
        }
        Participant::Taker => {
            validated_taker_journal_head(connection, role, local_participant, Some(key), swap_id)
        }
    }
}

fn validated_refund_journal_head(
    connection: &Connection,
    role: &str,
    local_participant: Participant,
    claim_key: Option<&ProtectedClaimKey>,
    swap_id: &SwapId,
) -> Result<(AcceptedZecAgreementV1, u64, SwapCoordinator), StoreError> {
    match local_participant {
        Participant::Maker => {
            let (accepted, revision, coordinator, _) = validated_maker_journal_head(
                connection,
                role,
                local_participant,
                claim_key,
                swap_id,
            )?;
            Ok((accepted, revision, coordinator))
        }
        Participant::Taker => {
            validated_taker_journal_head(connection, role, local_participant, claim_key, swap_id)
        }
    }
}

fn refund_coordinator_at(
    connection: &Connection,
    role: &str,
    accepted: &AcceptedZecAgreementV1,
    revision: u64,
    claim_key: Option<&ProtectedClaimKey>,
) -> Result<SwapCoordinator, StoreError> {
    match accepted.local_participant() {
        Participant::Maker => replay_maker_journal(
            connection,
            role,
            accepted.agreement().coordinator().id(),
            accepted,
            revision,
            claim_key,
        )
        .map(|(coordinator, _)| coordinator),
        Participant::Taker => replay_taker_journal(
            connection,
            role,
            accepted.agreement().coordinator().id(),
            accepted,
            revision,
            claim_key,
        ),
    }
}

fn validate_existing_claim_envelopes(
    connection: &Connection,
    role: &str,
    local_participant: Participant,
    key: &ProtectedClaimKey,
) -> Result<(), StoreError> {
    let swap_ids = {
        let mut statement = connection.prepare(
            "SELECT swap_id FROM zec_sdk_claim_materials WHERE local_role = ?1
             UNION
             SELECT swap_id FROM zec_sdk_claim_intents WHERE local_role = ?1
             ORDER BY swap_id",
        )?;
        let rows = statement.query_map(params![role], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for stored_swap_id in swap_ids {
        let swap_id =
            SwapId::new(stored_swap_id).map_err(|_| StoreError::InvalidZecRecoveryState)?;
        let _ = validated_claim_journal_head(connection, role, local_participant, key, &swap_id)?;
    }
    Ok(())
}

fn claim_coordinator_at(
    connection: &Connection,
    role: &str,
    accepted: &AcceptedZecAgreementV1,
    revision: u64,
    key: &ProtectedClaimKey,
) -> Result<SwapCoordinator, StoreError> {
    match accepted.local_participant() {
        Participant::Maker => replay_maker_journal(
            connection,
            role,
            accepted.agreement().coordinator().id(),
            accepted,
            revision,
            Some(key),
        )
        .map(|(coordinator, _)| coordinator),
        Participant::Taker => replay_taker_journal(
            connection,
            role,
            accepted.agreement().coordinator().id(),
            accepted,
            revision,
            Some(key),
        ),
    }
}

fn validated_agreement(
    row: &AgreementRow,
    local_participant: Participant,
    requested: &SwapId,
) -> Result<AcceptedZecAgreementV1, StoreError> {
    require_payload(
        "SDK agreement",
        row.payload_version,
        AGREEMENT_PAYLOAD_VERSION,
    )?;
    if row.active_revision < row.accepted_revision {
        return Err(StoreError::InvalidZecRecoveryState);
    }
    let accepted_at = UnixSeconds::new(revision_from_sql(row.accepted_at)?);
    let accepted_revision = revision_from_sql(row.accepted_revision)?;
    let accepted = AcceptedZecAgreementV1::resume_from_durable_parts(
        &row.agreement_wire,
        accepted_at,
        local_participant,
        accepted_revision,
    )?;
    if accepted.agreement().coordinator().id() != requested {
        return Err(StoreError::InvalidZecRecoveryState);
    }
    Ok(accepted)
}

fn decode_intent_record(
    payload_version: i64,
    payload_json: &str,
) -> Result<FirstLockIntentRecordV1, StoreError> {
    require_payload(
        "SDK first-lock intent",
        payload_version,
        i64::from(FIRST_LOCK_RECORD_SCHEMA_V1),
    )?;
    serde_json::from_str(payload_json).map_err(StoreError::from)
}

fn decode_transition_record(
    payload_version: i64,
    payload_json: &str,
) -> Result<FirstLockTransitionRecordV1, StoreError> {
    require_payload(
        "SDK first-lock transition",
        payload_version,
        i64::from(FIRST_LOCK_RECORD_SCHEMA_V1),
    )?;
    serde_json::from_str(payload_json).map_err(StoreError::from)
}

fn decode_observed_taker_lock_record(
    payload_version: i64,
    payload_json: &str,
    accepted: &AcceptedZecAgreementV1,
) -> Result<ObservedTakerFirstLockTransitionRecordV1, StoreError> {
    require_payload(
        "SDK observed taker first lock",
        payload_version,
        i64::from(FIRST_LOCK_RECORD_SCHEMA_V1),
    )?;
    let current_error = match serde_json::from_str(payload_json) {
        Ok(current) => return Ok(current),
        Err(error) => error,
    };
    if payload_json.contains("\"lez_change\"") {
        return Err(StoreError::from(current_error));
    }
    let mut value: serde_json::Value = serde_json::from_str(payload_json)?;
    if let Some(transaction) = value
        .get_mut("lez_canonical")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|canonical| canonical.get_mut("transaction"))
        .and_then(serde_json::Value::as_object_mut)
        && !transaction.contains_key("instruction")
        && let Some(swap_id) = transaction.remove("swap_id")
    {
        let instruction_name = match accepted.agreement().lez_terms().asset() {
            LezAssetV1::Native { .. } => "Native",
            LezAssetV1::FungibleToken { .. } => "Token",
        };
        let mut instruction_body = serde_json::Map::new();
        instruction_body.insert("swap_id".to_owned(), swap_id);
        let mut instruction = serde_json::Map::new();
        instruction.insert(
            instruction_name.to_owned(),
            serde_json::Value::Object(instruction_body),
        );
        transaction.insert(
            "instruction".to_owned(),
            serde_json::Value::Object(instruction),
        );
    }
    serde_json::from_value(value).map_err(StoreError::from)
}

fn decode_observed_maker_lock_record(
    payload_version: i64,
    payload_json: &str,
) -> Result<ObservedMakerLockTransitionRecordV1, StoreError> {
    require_payload(
        "SDK observed maker lock",
        payload_version,
        i64::from(OBSERVED_MAKER_LOCK_SCHEMA_V1),
    )?;
    serde_json::from_str(payload_json).map_err(StoreError::from)
}

fn decode_maker_intent_record(
    payload_version: i64,
    payload_json: &str,
) -> Result<MakerLockIntentRecordV1, StoreError> {
    require_payload(
        "SDK maker-lock intent",
        payload_version,
        i64::from(MAKER_LOCK_RECORD_SCHEMA_V1),
    )?;
    serde_json::from_str(payload_json).map_err(StoreError::from)
}

fn decode_maker_transition_record(
    payload_version: i64,
    payload_json: &str,
) -> Result<MakerLockTransitionRecordV1, StoreError> {
    require_payload(
        "SDK maker-lock transition",
        payload_version,
        i64::from(MAKER_LOCK_RECORD_SCHEMA_V1),
    )?;
    serde_json::from_str(payload_json).map_err(StoreError::from)
}

fn validate_transition_replay(
    row: &TransitionRow,
    intent_row: &IntentRow,
    accepted: &AcceptedZecAgreementV1,
    expected: &FirstLockTransitionV1,
    active_revision: u64,
    committed_revision: u64,
) -> Result<FirstLockProjectionCommit, StoreError> {
    let committed_sql = sql_u64(committed_revision)?;
    if row.committed_revision != committed_sql
        || intent_row.closed_revision != Some(committed_sql)
        || active_revision < committed_revision
    {
        return Err(StoreError::InvalidZecRecoveryState);
    }
    let intent_record = decode_intent_record(intent_row.payload_version, &intent_row.payload_json)?;
    let transition_record = decode_transition_record(row.payload_version, &row.payload_json)?;
    let trusted =
        transition_record.revalidate(accepted, &intent_record, expected.predecessor_revision())?;
    if &trusted != expected {
        return Err(StoreError::ConflictingZecFirstLockTransition);
    }
    Ok(FirstLockProjectionCommit::new(committed_revision, true))
}

fn claim_material_purpose(value: &str) -> Result<ClaimMaterialPurpose, StoreError> {
    match value {
        "local_first_claim" => Ok(ClaimMaterialPurpose::LocalFirstClaim),
        "observed_followup_claim" => Ok(ClaimMaterialPurpose::ObservedFollowUpClaim),
        _ => Err(StoreError::InvalidZecRecoveryState),
    }
}

fn claim_material_purpose_name(value: ClaimMaterialPurpose) -> &'static str {
    match value {
        ClaimMaterialPurpose::LocalFirstClaim => "local_first_claim",
        ClaimMaterialPurpose::ObservedFollowUpClaim => "observed_followup_claim",
        ClaimMaterialPurpose::LezClaimSubmission | ClaimMaterialPurpose::ZcashClaimSubmission => {
            unreachable!("submission purposes are stored with claim intents")
        }
    }
}

fn claim_material_context(
    accepted: &AcceptedZecAgreementV1,
    purpose: ClaimMaterialPurpose,
) -> ClaimMaterialContext<'_> {
    let agreement = accepted.agreement();
    ClaimMaterialContext::new(
        PROTECTED_CLAIM_SCHEMA_V1,
        agreement.coordinator().id(),
        Pair::Zcash,
        agreement.direction(),
        agreement.agreement_commitment(),
        accepted.local_participant(),
        purpose,
    )
}

fn claim_submission_context<'a>(
    agreement: &'a lez_zec_swap_sdk::ZecAgreementV1,
    intent: &'a ClaimIntentV1,
) -> ClaimSubmissionContext<'a> {
    let purpose = match intent.step() {
        ClaimStepV1::RevealingLez => ClaimMaterialPurpose::LezClaimSubmission,
        ClaimStepV1::FollowupZcash => ClaimMaterialPurpose::ZcashClaimSubmission,
    };
    ClaimSubmissionContext::new(
        ClaimMaterialContext::new(
            PROTECTED_CLAIM_SCHEMA_V1,
            agreement.coordinator().id(),
            Pair::Zcash,
            agreement.direction(),
            agreement.agreement_commitment(),
            intent.local_participant(),
            purpose,
        ),
        intent.step(),
        intent.staged_revision(),
        intent.expected_submission_id(),
    )
}

fn prepared_claim_submission_context<'a>(
    agreement: &'a lez_zec_swap_sdk::ZecAgreementV1,
    local: Participant,
    staged_revision: u64,
    prepared: &'a PreparedClaimSubmissionV1,
) -> ClaimSubmissionContext<'a> {
    let purpose = match prepared.step() {
        ClaimStepV1::RevealingLez => ClaimMaterialPurpose::LezClaimSubmission,
        ClaimStepV1::FollowupZcash => ClaimMaterialPurpose::ZcashClaimSubmission,
    };
    ClaimSubmissionContext::new(
        ClaimMaterialContext::new(
            PROTECTED_CLAIM_SCHEMA_V1,
            agreement.coordinator().id(),
            Pair::Zcash,
            agreement.direction(),
            agreement.agreement_commitment(),
            local,
            purpose,
        ),
        prepared.step(),
        staged_revision,
        prepared.expected_submission_id(),
    )
}

fn fresh_claim_nonce() -> Result<[u8; 24], StoreError> {
    let mut nonce = [0_u8; 24];
    getrandom::fill(&mut nonce).map_err(|_| StoreError::ClaimEntropy)?;
    Ok(nonce)
}

fn insert_claim_material(
    transaction: &rusqlite::Transaction<'_>,
    role: &str,
    swap_id: &SwapId,
    purpose: ClaimMaterialPurpose,
    created_revision: i64,
    protected: &ProtectedClaimEnvelope,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO zec_sdk_claim_materials (
            local_role, swap_id, purpose, created_revision, envelope_version,
            ciphertext, nonce, key_id, fingerprint
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            role,
            swap_id.as_str(),
            claim_material_purpose_name(purpose),
            created_revision,
            i64::from(PROTECTED_CLAIM_SCHEMA_V1),
            protected.ciphertext(),
            protected.nonce().as_slice(),
            protected.key_id(),
            protected.fingerprint().as_slice()
        ],
    )?;
    Ok(())
}

fn fixed_bytes<const N: usize>(value: Vec<u8>) -> Result<[u8; N], StoreError> {
    value
        .try_into()
        .map_err(|_| StoreError::InvalidZecRecoveryState)
}

fn protected_claim_material(row: &ClaimMaterialRow) -> Result<ProtectedClaimEnvelope, StoreError> {
    require_payload(
        "SDK protected claim material",
        row.envelope_version,
        i64::from(PROTECTED_CLAIM_SCHEMA_V1),
    )?;
    ProtectedClaimEnvelope::from_record_fields(
        fixed_bytes(row.ciphertext.clone())?,
        fixed_bytes(row.nonce.clone())?,
        row.key_id.clone(),
        fixed_bytes(row.fingerprint.clone())?,
    )
    .map_err(StoreError::from)
}

fn protected_claim_payload(
    row: &ClaimIntentRow,
) -> Result<ProtectedClaimPayloadEnvelope, StoreError> {
    require_payload(
        "SDK protected claim payload",
        row.protected_version,
        i64::from(PROTECTED_CLAIM_SCHEMA_V1),
    )?;
    ProtectedClaimPayloadEnvelope::from_record_fields(
        row.protected_ciphertext.clone(),
        fixed_bytes(row.protected_nonce.clone())?,
        row.protected_key_id.clone(),
        fixed_bytes(row.protected_fingerprint.clone())?,
    )
    .map_err(StoreError::from)
}

fn decrypt_claim_material(
    row: &ClaimMaterialRow,
    accepted: &AcceptedZecAgreementV1,
    key: &ProtectedClaimKey,
) -> Result<ClaimPreimage, StoreError> {
    let purpose = claim_material_purpose(&row.purpose)?;
    protected_claim_material(row)?
        .decrypt(key, claim_material_context(accepted, purpose))
        .map_err(StoreError::from)
}

fn decode_claim_intent_record(row: &ClaimIntentRow) -> Result<ClaimIntentRecordV1, StoreError> {
    require_payload(
        "SDK claim intent",
        row.payload_version,
        i64::from(CLAIM_RECORD_SCHEMA_V1),
    )?;
    serde_json::from_str(&row.payload_json).map_err(StoreError::from)
}

fn decode_refund_intent_record(
    payload_version: i64,
    payload_json: &str,
) -> Result<RefundIntentRecordV1, StoreError> {
    require_payload(
        "SDK refund intent",
        payload_version,
        i64::from(REFUND_RECORD_SCHEMA_V1),
    )?;
    serde_json::from_str(payload_json).map_err(StoreError::from)
}

fn decode_refund_transition_record(
    payload_version: i64,
    payload_json: &str,
) -> Result<RefundTransitionRecordV1, StoreError> {
    require_payload(
        "SDK refund transition",
        payload_version,
        i64::from(REFUND_RECORD_SCHEMA_V1),
    )?;
    serde_json::from_str(payload_json).map_err(StoreError::from)
}

fn decode_retained_refund_intent(
    row: &RefundTransitionRow,
) -> Result<Option<RefundIntentRecordV1>, StoreError> {
    match (
        row.transition_kind.as_str(),
        row.retained_intent_version,
        row.retained_intent_json.as_deref(),
        row.intent_staged_revision,
    ) {
        ("owned", Some(version), Some(json), Some(staged)) => {
            let record = decode_refund_intent_record(version, json)?;
            if record.staged_revision() != revision_from_sql(staged)? {
                return Err(StoreError::InvalidZecRecoveryState);
            }
            Ok(Some(record))
        }
        ("observed", None, None, None) => Ok(None),
        _ => Err(StoreError::InvalidZecRecoveryState),
    }
}

fn validate_claim_auxiliary_state(
    connection: &Connection,
    role: &str,
    accepted: &AcceptedZecAgreementV1,
    active_revision: u64,
    coordinator: &SwapCoordinator,
    key: &ProtectedClaimKey,
) -> Result<(), StoreError> {
    let swap_id = accepted.agreement().coordinator().id();
    let material = load_claim_material_row(connection, role, swap_id)?;
    if let Some(material) = material.as_ref() {
        let purpose = claim_material_purpose(&material.purpose)?;
        match purpose {
            ClaimMaterialPurpose::LocalFirstClaim
                if accepted.local_participant() == accepted.agreement().lez_claimant()
                    && material.created_revision == 0 => {}
            ClaimMaterialPurpose::ObservedFollowUpClaim
                if accepted.local_participant() != accepted.agreement().lez_claimant()
                    && material.created_revision > 0 =>
            {
                let predecessor = material
                    .created_revision
                    .checked_sub(1)
                    .ok_or(StoreError::InvalidZecRecoveryState)?;
                let transition =
                    load_observed_claim_transition_row(connection, role, swap_id, predecessor)?
                        .ok_or(StoreError::InvalidZecRecoveryState)?;
                if transition.transition_kind != "observed_revealing_lez"
                    || transition.committed_revision != material.created_revision
                    || transition.material_created_revision != Some(material.created_revision)
                {
                    return Err(StoreError::InvalidZecRecoveryState);
                }
            }
            _ => return Err(StoreError::InvalidZecRecoveryState),
        }
        let _ = decrypt_claim_material(material, accepted, key)?;
    }
    if let Some(intent_row) = load_claim_intent_row(connection, role, swap_id)? {
        let material = material
            .as_ref()
            .ok_or(StoreError::MissingZecClaimMaterial)?;
        if intent_row.material_created_revision != material.created_revision {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let record = decode_claim_intent_record(&intent_row)?;
        let intent = if intent_row.closed_revision.is_none() {
            record.revalidate(accepted, coordinator, active_revision)?
        } else {
            let committed = intent_row
                .closed_revision
                .ok_or(StoreError::InvalidZecRecoveryState)?;
            let predecessor = committed
                .checked_sub(1)
                .ok_or(StoreError::InvalidZecRecoveryState)?;
            let transition =
                load_owned_claim_transition_row(connection, role, swap_id, predecessor)?
                    .ok_or(StoreError::InvalidZecRecoveryState)?;
            if transition.committed_revision != committed
                || transition.intent_staged_revision != Some(intent_row.staged_revision)
            {
                return Err(StoreError::InvalidZecRecoveryState);
            }
            let predecessor_coordinator = claim_coordinator_at(
                connection,
                role,
                accepted,
                revision_from_sql(predecessor)?,
                key,
            )?;
            record.revalidate(
                accepted,
                &predecessor_coordinator,
                revision_from_sql(predecessor)?,
            )?
        };
        let protected = protected_claim_payload(&intent_row)?;
        intent.validate_protected_payload_fingerprint(protected.fingerprint())?;
        let exact =
            protected.decrypt(key, claim_submission_context(accepted.agreement(), &intent))?;
        let _ = PreparedClaimSubmissionV1::new(
            intent.step(),
            *intent.expected_submission_id(),
            exact.to_vec(),
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_terminal_journal_slot(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
    accepted: &AcceptedZecAgreementV1,
    coordinator: &SwapCoordinator,
    predecessor: u64,
    committed_sql: i64,
    claim_key: Option<&ProtectedClaimKey>,
) -> Result<SwapCoordinator, StoreError> {
    let predecessor_sql = sql_u64(predecessor)?;
    let owned_claim = load_owned_claim_transition_row(connection, role, swap_id, predecessor_sql)?;
    let observed_claim =
        load_observed_claim_transition_row(connection, role, swap_id, predecessor_sql)?;
    let refund = load_refund_transition_row(connection, role, swap_id, predecessor_sql)?;
    match (owned_claim.is_some() || observed_claim.is_some(), refund) {
        (true, None) => apply_claim_journal_slot(
            connection,
            role,
            swap_id,
            accepted,
            coordinator,
            predecessor,
            committed_sql,
            claim_key.ok_or(StoreError::MissingZecClaimKey)?,
        ),
        (false, Some(row)) => {
            apply_refund_journal_slot(&row, accepted, coordinator, predecessor, committed_sql)
        }
        _ => Err(StoreError::InvalidZecRecoveryState),
    }
}

fn apply_refund_journal_slot(
    row: &RefundTransitionRow,
    accepted: &AcceptedZecAgreementV1,
    coordinator: &SwapCoordinator,
    predecessor: u64,
    committed_sql: i64,
) -> Result<SwapCoordinator, StoreError> {
    if row.committed_revision != committed_sql {
        return Err(StoreError::InvalidZecRecoveryState);
    }
    let retained = decode_retained_refund_intent(row)?;
    let record = decode_refund_transition_record(row.payload_version, &row.payload_json)?;
    if record.is_owned() != retained.is_some() {
        return Err(StoreError::InvalidZecRecoveryState);
    }
    record
        .revalidate(accepted, coordinator, predecessor, retained.as_ref())?
        .apply_to(accepted.agreement(), coordinator, predecessor)
        .map_err(StoreError::from)
}

// Keeping the four typed claim variants together makes the exactly-one-row
// predecessor dispatch explicit during full-history replay.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn apply_claim_journal_slot(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
    accepted: &AcceptedZecAgreementV1,
    coordinator: &SwapCoordinator,
    predecessor: u64,
    committed_sql: i64,
    key: &ProtectedClaimKey,
) -> Result<SwapCoordinator, StoreError> {
    let predecessor_sql = sql_u64(predecessor)?;
    let owned = load_owned_claim_transition_row(connection, role, swap_id, predecessor_sql)?;
    let observed = load_observed_claim_transition_row(connection, role, swap_id, predecessor_sql)?;
    match (owned, observed) {
        (Some(row), None) if row.committed_revision == committed_sql => {
            let intent_row = load_claim_intent_row(connection, role, swap_id)?
                .ok_or(StoreError::MissingZecClaimIntent)?;
            if row.intent_staged_revision != Some(intent_row.staged_revision)
                || intent_row.closed_revision != Some(committed_sql)
            {
                return Err(StoreError::InvalidZecRecoveryState);
            }
            let intent_record = decode_claim_intent_record(&intent_row)?;
            let intent = intent_record.revalidate(accepted, coordinator, predecessor)?;
            let protected_payload = protected_claim_payload(&intent_row)?;
            intent.validate_protected_payload_fingerprint(protected_payload.fingerprint())?;
            let exact = protected_payload
                .decrypt(key, claim_submission_context(accepted.agreement(), &intent))?;
            let _ = PreparedClaimSubmissionV1::new(
                intent.step(),
                *intent.expected_submission_id(),
                exact.to_vec(),
            )?;
            let linked_material = load_claim_material_row(connection, role, swap_id)?
                .ok_or(StoreError::MissingZecClaimMaterial)?;
            if intent_row.material_created_revision != linked_material.created_revision {
                return Err(StoreError::InvalidZecRecoveryState);
            }
            let expected_purpose = match row.transition_kind.as_str() {
                "revealing_lez" => ClaimMaterialPurpose::LocalFirstClaim,
                "followup_zcash" => ClaimMaterialPurpose::ObservedFollowUpClaim,
                _ => return Err(StoreError::InvalidZecRecoveryState),
            };
            if claim_material_purpose(&linked_material.purpose)? != expected_purpose {
                return Err(StoreError::InvalidZecRecoveryState);
            }
            let _ = decrypt_claim_material(&linked_material, accepted, key)?;
            match row.transition_kind.as_str() {
                "revealing_lez" => {
                    require_revealing_payload(
                        "SDK revealing claim transition",
                        row.payload_version,
                    )?;
                    let preimage = decrypt_claim_material(&linked_material, accepted, key)?;
                    let record: RevealingClaimTransitionRecordV1 =
                        serde_json::from_str(&row.payload_json)?;
                    let transition = record.revalidate(
                        accepted,
                        coordinator,
                        &intent_record,
                        predecessor,
                        preimage,
                    )?;
                    transition
                        .apply_to(accepted.agreement(), coordinator, predecessor)
                        .map_err(StoreError::from)
                }
                "followup_zcash" => {
                    require_payload(
                        "SDK follow-up claim transition",
                        row.payload_version,
                        i64::from(CLAIM_RECORD_SCHEMA_V1),
                    )?;
                    let record: FollowupClaimTransitionRecordV1 =
                        serde_json::from_str(&row.payload_json)?;
                    let transition =
                        record.revalidate(accepted, coordinator, &intent_record, predecessor)?;
                    transition
                        .apply_to(accepted.agreement(), coordinator, predecessor)
                        .map_err(StoreError::from)
                }
                _ => Err(StoreError::InvalidZecRecoveryState),
            }
        }
        (None, Some(row)) if row.committed_revision == committed_sql => {
            match row.transition_kind.as_str() {
                "observed_revealing_lez" => {
                    if row.material_created_revision != Some(committed_sql) {
                        return Err(StoreError::InvalidZecRecoveryState);
                    }
                    require_revealing_payload(
                        "SDK observed revealing claim transition",
                        row.payload_version,
                    )?;
                    let material = load_claim_material_row(connection, role, swap_id)?
                        .ok_or(StoreError::MissingZecClaimMaterial)?;
                    if material.created_revision != committed_sql
                        || claim_material_purpose(&material.purpose)?
                            != ClaimMaterialPurpose::ObservedFollowUpClaim
                    {
                        return Err(StoreError::InvalidZecRecoveryState);
                    }
                    let preimage = decrypt_claim_material(&material, accepted, key)?;
                    let record: ObservedRevealingClaimTransitionRecordV1 =
                        serde_json::from_str(&row.payload_json)?;
                    let transition =
                        record.revalidate(accepted, coordinator, predecessor, preimage)?;
                    transition
                        .apply_to(accepted.agreement(), coordinator, predecessor)
                        .map_err(StoreError::from)
                }
                "observed_followup_zcash" => {
                    if row.material_created_revision.is_some() {
                        return Err(StoreError::InvalidZecRecoveryState);
                    }
                    require_payload(
                        "SDK observed follow-up claim transition",
                        row.payload_version,
                        i64::from(CLAIM_RECORD_SCHEMA_V1),
                    )?;
                    let record: ObservedFollowupClaimTransitionRecordV1 =
                        serde_json::from_str(&row.payload_json)?;
                    let transition = record.revalidate(accepted, coordinator, predecessor)?;
                    transition
                        .apply_to(accepted.agreement(), coordinator, predecessor)
                        .map_err(StoreError::from)
                }
                _ => Err(StoreError::InvalidZecRecoveryState),
            }
        }
        _ => Err(StoreError::InvalidZecRecoveryState),
    }
}

fn require_payload(kind: &'static str, version: i64, expected: i64) -> Result<(), StoreError> {
    if version == expected {
        Ok(())
    } else {
        Err(StoreError::UnsupportedPayloadVersion { kind, version })
    }
}

fn require_revealing_payload(kind: &'static str, version: i64) -> Result<(), StoreError> {
    if matches!(
        version,
        value if value == i64::from(CLAIM_RECORD_SCHEMA_V1)
            || value == i64::from(CLAIM_RECORD_SCHEMA_V2)
    ) {
        Ok(())
    } else {
        Err(StoreError::UnsupportedPayloadVersion { kind, version })
    }
}

fn sql_u64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::RevisionOverflow)
}
