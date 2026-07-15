//! Actor-local, evidence-replayed recovery state for Bitcoin swap lifecycles.

use std::path::Path;

use lez_swap_core::{
    Chain, ChainProof, ClaimEvidence, Pair, Participant, Phase, SwapCoordinator, SwapId,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::{
    StoreError, open_configured_connection, open_existing_configured_connection, participant_name,
};

const EVIDENCE_PAYLOAD_VERSION: i64 = 1;
const EVIDENCE_CHAIN_VERSION: i64 = 1;
const SNAPSHOT_PAYLOAD_VERSION: i64 = 1;
const MAX_AGREEMENT_WIRE_BYTES: usize = 16 * 1024;
const MAX_CHAIN_EVIDENCE_BYTES: usize = 64 * 1024;
const MAX_ENCODED_EVIDENCE_BYTES: usize = 320 * 1024;
const REVEALING_WITNESS_BYTES: usize = 64;
const EVIDENCE_CHAIN_GENESIS_DOMAIN: &[u8] =
    b"lez-atomic-swaps/btc-recovery/evidence-chain/v1/genesis";
const EVIDENCE_CHAIN_APPEND_DOMAIN: &[u8] =
    b"lez-atomic-swaps/btc-recovery/evidence-chain/v1/append";

/// One actor's exact, already-validated acceptance of immutable Bitcoin swap terms.
///
/// This persistence type binds caller-provided bytes and initial state; it does not parse or
/// cryptographically validate the agreement. That validation remains the chain-adapter caller's
/// responsibility before construction.
#[derive(Clone, Eq, PartialEq)]
#[must_use]
pub struct BtcAgreementAcceptance {
    swap_id: SwapId,
    local_role: Participant,
    agreement_wire: Box<[u8]>,
    agreement_commitment: [u8; 32],
    initial_snapshot_digest: [u8; 32],
    accepted_at_unix_seconds: u64,
}

impl std::fmt::Debug for BtcAgreementAcceptance {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BtcAgreementAcceptance")
            .field("swap_id", &self.swap_id)
            .field("local_role", &self.local_role)
            .field("agreement_wire", &"[REDACTED]")
            .field("agreement_commitment", &"[REDACTED]")
            .field("initial_snapshot_digest", &"[REDACTED]")
            .field("accepted_at_unix_seconds", &self.accepted_at_unix_seconds)
            .finish()
    }
}

impl BtcAgreementAcceptance {
    /// Retains an exact agreement after the caller has validated its version and commitment.
    ///
    /// # Errors
    ///
    /// Returns [`BtcRecoveryError::InvalidAgreementAcceptance`] when the wire value is empty,
    /// exceeds the durable bound, or the acceptance timestamp cannot be represented by `SQLite`.
    pub fn new(
        initial: &SwapCoordinator,
        local_role: Participant,
        agreement_wire: Vec<u8>,
        agreement_commitment: [u8; 32],
        accepted_at_unix_seconds: u64,
    ) -> Result<Self, BtcRecoveryError> {
        if initial.pair() != Pair::Bitcoin || initial.phase() != Phase::Offered {
            return Err(BtcRecoveryError::InitialCoordinatorMismatch);
        }
        if agreement_wire.is_empty()
            || agreement_wire.len() > MAX_AGREEMENT_WIRE_BYTES
            || agreement_commitment.iter().all(|byte| *byte == 0)
            || i64::try_from(accepted_at_unix_seconds).is_err()
        {
            return Err(BtcRecoveryError::InvalidAgreementAcceptance);
        }
        let initial_snapshot_digest = canonical_initial_snapshot_digest(initial)?;
        Ok(Self {
            swap_id: initial.id().clone(),
            local_role,
            agreement_wire: agreement_wire.into_boxed_slice(),
            agreement_commitment,
            initial_snapshot_digest,
            accepted_at_unix_seconds,
        })
    }

    /// Stable swap identity covered by the agreement.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Actor whose local recovery path owns this database.
    #[must_use]
    pub const fn local_role(&self) -> Participant {
        self.local_role
    }

    /// Exact validated agreement wire bytes.
    #[must_use]
    pub fn agreement_wire(&self) -> &[u8] {
        &self.agreement_wire
    }

    /// Caller-validated commitment to the agreement.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    /// SHA-256 binding to the canonical serialized agreement-derived initial coordinator.
    #[must_use]
    pub const fn initial_snapshot_digest(&self) -> &[u8; 32] {
        &self.initial_snapshot_digest
    }

    /// Unix-second time at which this actor accepted the exact agreement.
    #[must_use]
    pub const fn accepted_at_unix_seconds(&self) -> u64 {
        self.accepted_at_unix_seconds
    }
}

/// Version-1 lifecycle transition represented by one exact chain evidence record.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BtcLifecycleEvidenceKind {
    /// The first-funding taker's lock became canonical enough to continue.
    TakerLock,
    /// The second-funding maker's lock became canonical enough to continue.
    MakerLock,
    /// The first claim exposed a one-way commitment to adaptor evidence.
    RevealingClaim,
    /// The counterparty's follow-up claim completed the happy path.
    FollowupClaim,
}

impl BtcLifecycleEvidenceKind {
    const fn name(self) -> &'static str {
        match self {
            Self::TakerLock => "taker_lock",
            Self::MakerLock => "maker_lock",
            Self::RevealingClaim => "revealing_claim",
            Self::FollowupClaim => "followup_claim",
        }
    }

    const fn at_revision(revision: u64) -> Option<Self> {
        match revision {
            1 => Some(Self::TakerLock),
            2 => Some(Self::MakerLock),
            3 => Some(Self::RevealingClaim),
            4 => Some(Self::FollowupClaim),
            _ => None,
        }
    }
}

/// Secret-free exact evidence for one of the four Bitcoin happy-path transitions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct BtcLifecycleEvidenceV1 {
    kind: BtcLifecycleEvidenceKind,
    chain: Chain,
    proof: ChainProof,
    chain_evidence: Box<[u8]>,
    revealing_public_witness: Option<Box<[u8]>>,
    claim_evidence: Option<ClaimEvidence>,
}

impl BtcLifecycleEvidenceV1 {
    /// Constructs taker-lock evidence with exact public chain-adapter evidence bytes.
    ///
    /// # Errors
    ///
    /// Returns [`BtcRecoveryError::InvalidEvidence`] for an invalid transaction ID or an empty or
    /// oversized exact chain-evidence value.
    pub fn taker_lock(
        chain: Chain,
        transaction_id: impl Into<Box<str>>,
        confirmations: u32,
        chain_evidence: Vec<u8>,
    ) -> Result<Self, BtcRecoveryError> {
        Self::non_revealing(
            BtcLifecycleEvidenceKind::TakerLock,
            chain,
            transaction_id,
            confirmations,
            chain_evidence,
        )
    }

    /// Constructs maker-lock evidence with exact public chain-adapter evidence bytes.
    ///
    /// # Errors
    ///
    /// Returns [`BtcRecoveryError::InvalidEvidence`] for an invalid transaction ID or an empty or
    /// oversized exact chain-evidence value.
    pub fn maker_lock(
        chain: Chain,
        transaction_id: impl Into<Box<str>>,
        confirmations: u32,
        chain_evidence: Vec<u8>,
    ) -> Result<Self, BtcRecoveryError> {
        Self::non_revealing(
            BtcLifecycleEvidenceKind::MakerLock,
            chain,
            transaction_id,
            confirmations,
            chain_evidence,
        )
    }

    fn non_revealing(
        kind: BtcLifecycleEvidenceKind,
        chain: Chain,
        transaction_id: impl Into<Box<str>>,
        confirmations: u32,
        chain_evidence: Vec<u8>,
    ) -> Result<Self, BtcRecoveryError> {
        if kind == BtcLifecycleEvidenceKind::FollowupClaim && confirmations == 0 {
            return Err(BtcRecoveryError::InvalidEvidence { revision: 0 });
        }
        let proof = validated_proof(transaction_id, confirmations)?;
        let chain_evidence = validated_chain_evidence(chain_evidence)?;
        Ok(Self {
            kind,
            chain,
            proof,
            chain_evidence,
            revealing_public_witness: None,
            claim_evidence: None,
        })
    }

    /// Constructs the revealing claim with its exact public signature/witness bytes.
    ///
    /// # Errors
    ///
    /// Returns [`BtcRecoveryError::InvalidEvidence`] for an invalid transaction ID or an empty or
    /// oversized exact chain-evidence value.
    pub fn revealing_claim(
        chain: Chain,
        transaction_id: impl Into<Box<str>>,
        confirmations: u32,
        chain_evidence: Vec<u8>,
        revealing_public_witness: [u8; REVEALING_WITNESS_BYTES],
        claim_evidence: ClaimEvidence,
    ) -> Result<Self, BtcRecoveryError> {
        if revealing_public_witness.iter().all(|byte| *byte == 0) {
            return Err(BtcRecoveryError::InvalidEvidence { revision: 0 });
        }
        let proof = validated_proof(transaction_id, confirmations)?;
        let chain_evidence = validated_chain_evidence(chain_evidence)?;
        Ok(Self {
            kind: BtcLifecycleEvidenceKind::RevealingClaim,
            chain,
            proof,
            chain_evidence,
            revealing_public_witness: Some(revealing_public_witness.to_vec().into_boxed_slice()),
            claim_evidence: Some(claim_evidence),
        })
    }

    /// Constructs the non-revealing follow-up claim evidence.
    ///
    /// # Errors
    ///
    /// Returns [`BtcRecoveryError::InvalidEvidence`] for an invalid transaction ID or an empty or
    /// oversized exact chain-evidence value.
    pub fn followup_claim(
        chain: Chain,
        transaction_id: impl Into<Box<str>>,
        confirmations: u32,
        chain_evidence: Vec<u8>,
    ) -> Result<Self, BtcRecoveryError> {
        Self::non_revealing(
            BtcLifecycleEvidenceKind::FollowupClaim,
            chain,
            transaction_id,
            confirmations,
            chain_evidence,
        )
    }

    /// Stable semantic transition kind.
    #[must_use]
    pub const fn kind(&self) -> BtcLifecycleEvidenceKind {
        self.kind
    }

    /// Chain on which this exact evidence was observed.
    #[must_use]
    pub const fn chain(&self) -> Chain {
        self.chain
    }

    /// Exact transaction proof supplied by the chain adapter.
    #[must_use]
    pub const fn proof(&self) -> &ChainProof {
        &self.proof
    }

    /// Exact bounded public chain-adapter DTO bytes used to validate the proof.
    #[must_use]
    pub fn chain_evidence(&self) -> &[u8] {
        &self.chain_evidence
    }

    /// Exact public revealing signature/witness retained for peerless recovery.
    #[must_use]
    pub fn revealing_public_witness(&self) -> Option<&[u8]> {
        self.revealing_public_witness.as_deref()
    }

    /// One-way revealing claim evidence, only for that transition kind.
    #[must_use]
    pub const fn claim_evidence(&self) -> Option<&ClaimEvidence> {
        self.claim_evidence.as_ref()
    }

    fn apply(
        &self,
        coordinator: &mut SwapCoordinator,
        revision: u64,
    ) -> Result<(), BtcRecoveryError> {
        let expected_chain = match self.kind {
            BtcLifecycleEvidenceKind::TakerLock | BtcLifecycleEvidenceKind::FollowupClaim => {
                coordinator.funded_chain(Participant::Taker)
            }
            BtcLifecycleEvidenceKind::MakerLock | BtcLifecycleEvidenceKind::RevealingClaim => {
                coordinator.funded_chain(Participant::Maker)
            }
        };
        let revealing_shape = self.kind == BtcLifecycleEvidenceKind::RevealingClaim;
        if self.chain != expected_chain
            || self.chain_evidence.is_empty()
            || self.chain_evidence.len() > MAX_CHAIN_EVIDENCE_BYTES
            || revealing_shape != self.claim_evidence.is_some()
            || revealing_shape != self.revealing_public_witness.is_some()
            || self
                .revealing_public_witness
                .as_ref()
                .is_some_and(|witness| witness.len() != REVEALING_WITNESS_BYTES)
            || self
                .revealing_public_witness
                .as_ref()
                .is_some_and(|witness| witness.iter().all(|byte| *byte == 0))
            || (self.kind == BtcLifecycleEvidenceKind::FollowupClaim
                && self.proof.confirmations() == 0)
        {
            return Err(BtcRecoveryError::InvalidEvidence { revision });
        }

        let result = match self.kind {
            BtcLifecycleEvidenceKind::TakerLock => {
                coordinator.observe_taker_lock(self.proof.clone())
            }
            BtcLifecycleEvidenceKind::MakerLock => {
                coordinator.observe_maker_lock(self.proof.clone())
            }
            BtcLifecycleEvidenceKind::RevealingClaim => coordinator.observe_revealing_claim(
                coordinator.first_claimant(),
                self.proof.clone(),
                self.claim_evidence
                    .clone()
                    .ok_or(BtcRecoveryError::InvalidEvidence { revision })?,
            ),
            BtcLifecycleEvidenceKind::FollowupClaim => coordinator
                .observe_followup_claim(coordinator.first_claimant().other(), self.proof.clone()),
        };
        result.map_err(|_| BtcRecoveryError::InvalidEvidence { revision })?;

        let expected_phase = match revision {
            1 => Phase::TakerLockConfirmed,
            2 => Phase::BothLegsLocked,
            3 => Phase::ClaimEvidenceAvailable,
            4 => Phase::Completed,
            _ => return Err(BtcRecoveryError::InvalidSequence { revision }),
        };
        if coordinator.phase() != expected_phase {
            return Err(BtcRecoveryError::InvalidEvidence { revision });
        }
        Ok(())
    }
}

fn validated_proof(
    transaction_id: impl Into<Box<str>>,
    confirmations: u32,
) -> Result<ChainProof, BtcRecoveryError> {
    ChainProof::new(transaction_id, confirmations)
        .map_err(|_| BtcRecoveryError::InvalidEvidence { revision: 0 })
}

fn validated_chain_evidence(chain_evidence: Vec<u8>) -> Result<Box<[u8]>, BtcRecoveryError> {
    if chain_evidence.is_empty() || chain_evidence.len() > MAX_CHAIN_EVIDENCE_BYTES {
        return Err(BtcRecoveryError::InvalidEvidence { revision: 0 });
    }
    Ok(chain_evidence.into_boxed_slice())
}

/// Result of one atomic evidence projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcProjectionCommit {
    revision: u64,
    was_replay: bool,
}

impl BtcProjectionCommit {
    /// Aggregate revision represented by the evidence.
    #[must_use]
    pub const fn revision(self) -> u64 {
        self.revision
    }

    /// Whether the exact evidence was already durable.
    #[must_use]
    pub const fn was_replay(self) -> bool {
        self.was_replay
    }
}

/// Terminal happy-path outcome available from local durable evidence alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BtcTerminalOutcome {
    /// Both claims were replayed and the coordinator reached `Completed`.
    Completed,
}

/// Reconstructed actor-local state that does not require live chain endpoints.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct BtcOfflineStatus {
    revision: u64,
    phase: Phase,
    terminal: Option<BtcTerminalOutcome>,
    revealing_public_witness: Option<Box<[u8]>>,
}

impl BtcOfflineStatus {
    /// Last contiguous evidence revision replayed into the coordinator.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Reconstructed coordinator phase.
    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }

    /// Terminal outcome when the exact four-record lifecycle completed.
    #[must_use]
    pub const fn terminal(&self) -> Option<BtcTerminalOutcome> {
        self.terminal
    }

    /// Exact public witness needed by the counterparty's recovery logic after restart.
    #[must_use]
    pub fn revealing_public_witness(&self) -> Option<&[u8]> {
        self.revealing_public_witness.as_deref()
    }
}

/// Failure while accepting or reconstructing actor-local Bitcoin recovery state.
#[derive(Debug, Error)]
pub enum BtcRecoveryError {
    /// Shared hardened `SQLite` opener or database operation failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The accepted agreement cannot be represented within bounded durable storage.
    #[error("accepted Bitcoin agreement is invalid")]
    InvalidAgreementAcceptance,
    /// A different exact agreement was already accepted for this swap and role.
    #[error("accepted Bitcoin agreement conflicts with durable actor state")]
    AgreementConflict,
    /// The same swap database path was opened as the other local actor.
    #[error("Bitcoin recovery database belongs to a different local role")]
    RolePathAlias,
    /// Caller-supplied agreement-derived initial state is not a fresh Bitcoin coordinator.
    #[error("initial Bitcoin coordinator does not match the accepted agreement identity")]
    InitialCoordinatorMismatch,
    /// An existing database has no durable acceptance for this swap.
    #[error("Bitcoin recovery database has no agreement acceptance")]
    MissingAgreementAcceptance,
    /// The caller attempted a non-replay transition from a stale revision.
    #[error("Bitcoin recovery predecessor is stale: expected {expected}, actual {actual}")]
    StalePredecessor {
        /// Revision supplied by the actor.
        expected: u64,
        /// Current contiguous durable revision.
        actual: u64,
    },
    /// A revision already contains a different exact evidence payload.
    #[error("Bitcoin lifecycle evidence conflicts at revision {revision}")]
    EvidenceConflict {
        /// Conflicting aggregate revision.
        revision: u64,
    },
    /// The evidence kind or durable revision is not the next reviewed transition.
    #[error("Bitcoin lifecycle evidence sequence is invalid at revision {revision}")]
    InvalidSequence {
        /// First invalid aggregate revision.
        revision: u64,
    },
    /// Chain, proof, claim shape, or resulting coordinator phase is invalid.
    #[error("Bitcoin lifecycle evidence is invalid at revision {revision}")]
    InvalidEvidence {
        /// Evidence revision, or zero for a constructor input.
        revision: u64,
    },
    /// An evidence row uses a payload version this binary cannot replay.
    #[error("unsupported Bitcoin evidence payload version {0}")]
    UnsupportedEvidenceVersion(i64),
    /// Aggregate snapshot uses an unsupported version.
    #[error("unsupported Bitcoin aggregate snapshot version {0}")]
    UnsupportedSnapshotVersion(i64),
    /// Stored snapshot differs from evidence replay and is never trusted as authority.
    #[error("Bitcoin aggregate snapshot does not match evidence replay")]
    SnapshotMismatch,
    /// Evidence-chain version cannot be recomputed by this binary.
    #[error("unsupported Bitcoin evidence-chain version {0}")]
    UnsupportedEvidenceChainVersion(i64),
    /// Aggregate evidence-chain head differs from exact persisted evidence replay.
    #[error("Bitcoin evidence chain does not match exact persisted evidence")]
    EvidenceChainMismatch,
    /// A lifecycle revision cannot be represented durably.
    #[error("Bitcoin lifecycle revision overflowed")]
    RevisionOverflow,
}

impl From<rusqlite::Error> for BtcRecoveryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(StoreError::Sqlite(error))
    }
}

impl From<serde_json::Error> for BtcRecoveryError {
    fn from(error: serde_json::Error) -> Self {
        Self::Store(StoreError::Serialization(error))
    }
}

/// SQLite-backed actor-local Bitcoin state reconstructed from exact evidence.
///
/// A versioned, domain-separated hash chain detects partial writes and accidental or unauthorized
/// row drift inside the owner-private store. It is an integrity consistency check, not
/// authenticity against a malicious database owner able to recompute and rewrite both evidence
/// and the aggregate head.
#[derive(Debug)]
pub struct SqliteBtcRecoveryStore {
    connection: Connection,
    acceptance: BtcAgreementAcceptance,
    initial: SwapCoordinator,
    acceptance_was_replay: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BtcStoreOpenMode {
    Activate,
    ExistingOnly,
}

impl SqliteBtcRecoveryStore {
    /// Opens or creates one actor store and reconstructs it from contiguous evidence.
    ///
    /// The supplied initial coordinator must be freshly derived from the caller-validated exact
    /// agreement on every open. Persisted aggregate JSON is checked against replay, never used as
    /// the phase authority.
    ///
    /// # Errors
    ///
    /// Returns [`BtcRecoveryError`] when the database cannot be opened securely, the exact
    /// agreement or role conflicts, the initial coordinator is not fresh Bitcoin state, or
    /// durable evidence, sequence, version, or snapshot validation fails.
    pub fn open(
        path: impl AsRef<Path>,
        acceptance: &BtcAgreementAcceptance,
        initial: &SwapCoordinator,
    ) -> Result<Self, BtcRecoveryError> {
        Self::open_with_mode(
            path.as_ref(),
            acceptance,
            initial,
            BtcStoreOpenMode::Activate,
        )
    }

    /// Opens an existing actor store without creating an agreement acceptance.
    ///
    /// The database file must already exist and contain the exact matching acceptance and
    /// aggregate row. Schema migration may run, but this method never inserts actor activation.
    ///
    /// # Errors
    ///
    /// Returns [`BtcRecoveryError::MissingAgreementAcceptance`] only for a valid database with no
    /// matching acceptance or actor aggregate/evidence rows. Unsafe, conflicting, incomplete, or
    /// corrupt state fails closed under its existing error category.
    pub fn open_existing(
        path: impl AsRef<Path>,
        acceptance: &BtcAgreementAcceptance,
        initial: &SwapCoordinator,
    ) -> Result<Self, BtcRecoveryError> {
        Self::open_with_mode(
            path.as_ref(),
            acceptance,
            initial,
            BtcStoreOpenMode::ExistingOnly,
        )
    }

    fn open_with_mode(
        path: &Path,
        acceptance: &BtcAgreementAcceptance,
        initial: &SwapCoordinator,
        mode: BtcStoreOpenMode,
    ) -> Result<Self, BtcRecoveryError> {
        validate_initial(acceptance, initial)?;
        let mut connection = match mode {
            BtcStoreOpenMode::Activate => open_configured_connection(path)?,
            BtcStoreOpenMode::ExistingOnly => open_existing_configured_connection(path)?,
        };
        migrate_btc_recovery(&connection)?;

        let initial_snapshot = serde_json::to_string(initial)?;
        let initial_evidence_chain_head = evidence_chain_genesis(acceptance);
        let accepted_at = i64::try_from(acceptance.accepted_at_unix_seconds)
            .map_err(|_| BtcRecoveryError::InvalidAgreementAcceptance)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
            .query_row(
                "
                SELECT local_role, agreement_wire, agreement_commitment,
                       initial_snapshot_digest, accepted_at_unix_seconds
                FROM btc_actor_agreements WHERE swap_id = ?1
                ",
                params![acceptance.swap_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?;

        let acceptance_was_replay = existing.is_some();
        if let Some((role, wire, commitment, initial_digest, durable_accepted_at)) = existing {
            if role != participant_name(acceptance.local_role) {
                return Err(BtcRecoveryError::RolePathAlias);
            }
            if wire.as_slice() != acceptance.agreement_wire()
                || commitment.as_slice() != acceptance.agreement_commitment()
                || initial_digest.as_slice() != acceptance.initial_snapshot_digest()
                || durable_accepted_at != accepted_at
            {
                return Err(BtcRecoveryError::AgreementConflict);
            }
        } else if mode == BtcStoreOpenMode::ExistingOnly {
            return Err(
                if has_actor_state_without_acceptance(&transaction, acceptance)? {
                    BtcRecoveryError::AgreementConflict
                } else {
                    BtcRecoveryError::MissingAgreementAcceptance
                },
            );
        } else {
            transaction.execute(
                "
                    INSERT INTO btc_actor_agreements (
                        swap_id, local_role, agreement_wire, agreement_commitment,
                        initial_snapshot_digest, accepted_at_unix_seconds
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                    ",
                params![
                    acceptance.swap_id.as_str(),
                    participant_name(acceptance.local_role),
                    acceptance.agreement_wire(),
                    acceptance.agreement_commitment().as_slice(),
                    acceptance.initial_snapshot_digest().as_slice(),
                    accepted_at,
                ],
            )?;
            transaction.execute(
                "
                    INSERT INTO btc_actor_aggregates (
                        swap_id, local_role, revision, snapshot_version, snapshot_json,
                        evidence_chain_version, evidence_chain_head
                    ) VALUES (?1, ?2, 0, ?3, ?4, ?5, ?6)
                    ",
                params![
                    acceptance.swap_id.as_str(),
                    participant_name(acceptance.local_role),
                    SNAPSHOT_PAYLOAD_VERSION,
                    initial_snapshot,
                    EVIDENCE_CHAIN_VERSION,
                    initial_evidence_chain_head.as_slice(),
                ],
            )?;
        }
        transaction.commit()?;

        let store = Self {
            connection,
            acceptance: acceptance.clone(),
            initial: initial.clone(),
            acceptance_was_replay,
        };
        reconstruct_checked(&store.connection, &store.acceptance, &store.initial)?;
        Ok(store)
    }

    /// Whether activation replayed an already matching exact durable acceptance.
    #[must_use]
    pub const fn acceptance_was_replay(&self) -> bool {
        self.acceptance_was_replay
    }

    /// Projects one exact lifecycle record in an immediate transaction with predecessor CAS.
    ///
    /// Exact replays are idempotent even after later revisions. A changed payload at the proposed
    /// revision conflicts, while a missing proposal from any stale predecessor returns the actual
    /// contiguous revision.
    ///
    /// # Errors
    ///
    /// Returns [`BtcRecoveryError`] for a stale predecessor, changed replay, invalid transition,
    /// corrupt reconstructed state, revision overflow, or atomic database failure.
    pub fn project(
        &mut self,
        expected_predecessor: u64,
        evidence: &BtcLifecycleEvidenceV1,
    ) -> Result<BtcProjectionCommit, BtcRecoveryError> {
        let proposed_revision = expected_predecessor
            .checked_add(1)
            .ok_or(BtcRecoveryError::RevisionOverflow)?;
        let proposed_sql = revision_to_sql(proposed_revision)?;
        let evidence_json = serde_json::to_string(evidence)?;
        if evidence_json.len() > MAX_ENCODED_EVIDENCE_BYTES {
            return Err(BtcRecoveryError::InvalidEvidence {
                revision: proposed_revision,
            });
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let reconstructed = reconstruct_checked(&transaction, &self.acceptance, &self.initial)?;
        let existing = load_evidence_at(&transaction, &self.acceptance, proposed_sql)?;
        if let Some((kind, version, payload)) = existing {
            if kind == evidence.kind.name()
                && version == EVIDENCE_PAYLOAD_VERSION
                && payload == evidence_json
            {
                transaction.commit()?;
                return Ok(BtcProjectionCommit {
                    revision: proposed_revision,
                    was_replay: true,
                });
            }
            return Err(BtcRecoveryError::EvidenceConflict {
                revision: proposed_revision,
            });
        }

        if reconstructed.revision != expected_predecessor {
            return Err(BtcRecoveryError::StalePredecessor {
                expected: expected_predecessor,
                actual: reconstructed.revision,
            });
        }
        if BtcLifecycleEvidenceKind::at_revision(proposed_revision) != Some(evidence.kind) {
            return Err(BtcRecoveryError::InvalidSequence {
                revision: proposed_revision,
            });
        }

        let mut next = reconstructed.coordinator;
        evidence.apply(&mut next, proposed_revision)?;
        let next_snapshot = serde_json::to_string(&next)?;
        let next_evidence_chain_head = evidence_chain_append(
            &reconstructed.evidence_chain_head,
            &self.acceptance,
            proposed_revision,
            evidence.kind.name(),
            EVIDENCE_PAYLOAD_VERSION,
            &evidence_json,
        );
        insert_evidence(
            &transaction,
            &self.acceptance,
            proposed_sql,
            evidence,
            &evidence_json,
        )?;
        let predecessor_sql = revision_to_sql(expected_predecessor)?;
        let updated = transaction.execute(
            "
            UPDATE btc_actor_aggregates
            SET revision = ?1, snapshot_version = ?2, snapshot_json = ?3,
                evidence_chain_version = ?4, evidence_chain_head = ?5
            WHERE swap_id = ?6 AND local_role = ?7 AND revision = ?8
              AND evidence_chain_version = ?9 AND evidence_chain_head = ?10
            ",
            params![
                proposed_sql,
                SNAPSHOT_PAYLOAD_VERSION,
                next_snapshot,
                EVIDENCE_CHAIN_VERSION,
                next_evidence_chain_head.as_slice(),
                self.acceptance.swap_id.as_str(),
                participant_name(self.acceptance.local_role),
                predecessor_sql,
                EVIDENCE_CHAIN_VERSION,
                reconstructed.evidence_chain_head.as_slice(),
            ],
        )?;
        if updated != 1 {
            return Err(BtcRecoveryError::StalePredecessor {
                expected: expected_predecessor,
                actual: reconstructed.revision,
            });
        }
        transaction.commit()?;
        Ok(BtcProjectionCommit {
            revision: proposed_revision,
            was_replay: false,
        })
    }

    /// Reconstructs current state using only the caller-derived initial state and durable evidence.
    ///
    /// # Errors
    ///
    /// Returns [`BtcRecoveryError`] when agreement, evidence, sequence, payload-version, or
    /// aggregate-snapshot validation fails.
    pub fn status(&self) -> Result<BtcOfflineStatus, BtcRecoveryError> {
        let reconstructed = reconstruct_checked(&self.connection, &self.acceptance, &self.initial)?;
        Ok(BtcOfflineStatus {
            revision: reconstructed.revision,
            phase: reconstructed.coordinator.phase(),
            terminal: (reconstructed.coordinator.phase() == Phase::Completed)
                .then_some(BtcTerminalOutcome::Completed),
            revealing_public_witness: reconstructed.revealing_public_witness,
        })
    }
}

fn has_actor_state_without_acceptance(
    connection: &Connection,
    acceptance: &BtcAgreementAcceptance,
) -> Result<bool, BtcRecoveryError> {
    Ok(connection.query_row(
        "
        SELECT EXISTS (
            SELECT 1 FROM btc_actor_aggregates WHERE swap_id = ?1
        ) OR EXISTS (
            SELECT 1 FROM btc_actor_evidence WHERE swap_id = ?1
        )
        ",
        [acceptance.swap_id.as_str()],
        |row| row.get(0),
    )?)
}

struct ReconstructedState {
    revision: u64,
    coordinator: SwapCoordinator,
    revealing_public_witness: Option<Box<[u8]>>,
    evidence_chain_head: [u8; 32],
}

struct DurableAggregate {
    revision: i64,
    snapshot_version: i64,
    snapshot_json: String,
    evidence_chain_version: i64,
    evidence_chain_head: Vec<u8>,
}

fn load_durable_aggregate(
    connection: &Connection,
    acceptance: &BtcAgreementAcceptance,
) -> Result<DurableAggregate, BtcRecoveryError> {
    connection
        .query_row(
            "
            SELECT revision, snapshot_version, snapshot_json,
                   evidence_chain_version, evidence_chain_head
            FROM btc_actor_aggregates WHERE swap_id = ?1 AND local_role = ?2
            ",
            params![
                acceptance.swap_id.as_str(),
                participant_name(acceptance.local_role)
            ],
            |row| {
                Ok(DurableAggregate {
                    revision: row.get(0)?,
                    snapshot_version: row.get(1)?,
                    snapshot_json: row.get(2)?,
                    evidence_chain_version: row.get(3)?,
                    evidence_chain_head: row.get(4)?,
                })
            },
        )
        .optional()?
        .ok_or(BtcRecoveryError::InvalidSequence { revision: 0 })
}

fn load_evidence_at(
    connection: &Connection,
    acceptance: &BtcAgreementAcceptance,
    revision: i64,
) -> Result<Option<(String, i64, String)>, BtcRecoveryError> {
    Ok(connection
        .query_row(
            "
            SELECT evidence_kind, payload_version, payload_json
            FROM btc_actor_evidence
            WHERE swap_id = ?1 AND local_role = ?2 AND aggregate_revision = ?3
            ",
            params![
                acceptance.swap_id.as_str(),
                participant_name(acceptance.local_role),
                revision,
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?)
}

fn insert_evidence(
    connection: &Connection,
    acceptance: &BtcAgreementAcceptance,
    revision: i64,
    evidence: &BtcLifecycleEvidenceV1,
    exact_payload_json: &str,
) -> Result<(), BtcRecoveryError> {
    connection.execute(
        "
        INSERT INTO btc_actor_evidence (
            swap_id, local_role, aggregate_revision, evidence_kind,
            payload_version, payload_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ",
        params![
            acceptance.swap_id.as_str(),
            participant_name(acceptance.local_role),
            revision,
            evidence.kind.name(),
            EVIDENCE_PAYLOAD_VERSION,
            exact_payload_json,
        ],
    )?;
    Ok(())
}

fn validate_initial(
    acceptance: &BtcAgreementAcceptance,
    initial: &SwapCoordinator,
) -> Result<(), BtcRecoveryError> {
    if initial.id() != acceptance.swap_id()
        || initial.pair() != Pair::Bitcoin
        || initial.phase() != Phase::Offered
        || canonical_initial_snapshot_digest(initial)? != *acceptance.initial_snapshot_digest()
    {
        return Err(BtcRecoveryError::InitialCoordinatorMismatch);
    }
    Ok(())
}

fn canonical_initial_snapshot_digest(
    initial: &SwapCoordinator,
) -> Result<[u8; 32], BtcRecoveryError> {
    Ok(Sha256::digest(serde_json::to_vec(initial)?).into())
}

fn evidence_chain_genesis(acceptance: &BtcAgreementAcceptance) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, EVIDENCE_CHAIN_GENESIS_DOMAIN);
    hasher.update(EVIDENCE_CHAIN_VERSION.to_be_bytes());
    hash_field(&mut hasher, acceptance.swap_id.as_str().as_bytes());
    hash_field(
        &mut hasher,
        participant_name(acceptance.local_role).as_bytes(),
    );
    hash_field(&mut hasher, acceptance.agreement_wire());
    hash_field(&mut hasher, acceptance.agreement_commitment());
    hash_field(&mut hasher, acceptance.initial_snapshot_digest());
    hasher.update(acceptance.accepted_at_unix_seconds.to_be_bytes());
    hasher.finalize().into()
}

fn evidence_chain_append(
    previous_head: &[u8; 32],
    acceptance: &BtcAgreementAcceptance,
    revision: u64,
    evidence_kind: &str,
    payload_version: i64,
    exact_payload_json: &str,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, EVIDENCE_CHAIN_APPEND_DOMAIN);
    hasher.update(EVIDENCE_CHAIN_VERSION.to_be_bytes());
    hash_field(&mut hasher, previous_head);
    hash_field(&mut hasher, acceptance.swap_id.as_str().as_bytes());
    hash_field(
        &mut hasher,
        participant_name(acceptance.local_role).as_bytes(),
    );
    hasher.update(revision.to_be_bytes());
    hash_field(&mut hasher, evidence_kind.as_bytes());
    hasher.update(payload_version.to_be_bytes());
    hash_field(&mut hasher, exact_payload_json.as_bytes());
    hasher.finalize().into()
}

fn hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    hasher.update(length.to_be_bytes());
    hasher.update(bytes);
}

fn migrate_btc_recovery(connection: &Connection) -> Result<(), BtcRecoveryError> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS btc_actor_agreements (
            swap_id TEXT PRIMARY KEY NOT NULL,
            local_role TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
            agreement_wire BLOB NOT NULL CHECK (length(agreement_wire) BETWEEN 1 AND 16384),
            agreement_commitment BLOB NOT NULL CHECK (
                length(agreement_commitment) = 32 AND agreement_commitment != zeroblob(32)
            ),
            initial_snapshot_digest BLOB NOT NULL CHECK (length(initial_snapshot_digest) = 32),
            accepted_at_unix_seconds INTEGER NOT NULL CHECK (accepted_at_unix_seconds >= 0)
        );

        CREATE TABLE IF NOT EXISTS btc_actor_aggregates (
            swap_id TEXT NOT NULL,
            local_role TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
            revision INTEGER NOT NULL CHECK (revision BETWEEN 0 AND 4),
            snapshot_version INTEGER NOT NULL,
            snapshot_json TEXT NOT NULL,
            evidence_chain_version INTEGER NOT NULL CHECK (evidence_chain_version >= 1),
            evidence_chain_head BLOB NOT NULL CHECK (length(evidence_chain_head) = 32),
            PRIMARY KEY (swap_id, local_role),
            FOREIGN KEY (swap_id) REFERENCES btc_actor_agreements(swap_id)
                ON DELETE RESTRICT
        );

        CREATE TABLE IF NOT EXISTS btc_actor_evidence (
            swap_id TEXT NOT NULL,
            local_role TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
            aggregate_revision INTEGER NOT NULL CHECK (aggregate_revision BETWEEN 1 AND 4),
            evidence_kind TEXT NOT NULL CHECK (evidence_kind IN (
                'taker_lock', 'maker_lock', 'revealing_claim', 'followup_claim'
            )),
            payload_version INTEGER NOT NULL,
            payload_json TEXT NOT NULL CHECK (
                length(CAST(payload_json AS BLOB)) BETWEEN 1 AND 327680
            ),
            PRIMARY KEY (swap_id, local_role, aggregate_revision),
            UNIQUE (swap_id, local_role, evidence_kind),
            FOREIGN KEY (swap_id, local_role)
                REFERENCES btc_actor_aggregates(swap_id, local_role) ON DELETE RESTRICT
        );
        ",
    )?;
    Ok(())
}

fn verify_acceptance(
    connection: &Connection,
    acceptance: &BtcAgreementAcceptance,
) -> Result<(), BtcRecoveryError> {
    let durable = connection
        .query_row(
            "
            SELECT local_role, agreement_wire, agreement_commitment,
                   initial_snapshot_digest, accepted_at_unix_seconds
            FROM btc_actor_agreements WHERE swap_id = ?1
            ",
            params![acceptance.swap_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?
        .ok_or(BtcRecoveryError::AgreementConflict)?;
    if durable.0 != participant_name(acceptance.local_role) {
        return Err(BtcRecoveryError::RolePathAlias);
    }
    let accepted_at = i64::try_from(acceptance.accepted_at_unix_seconds)
        .map_err(|_| BtcRecoveryError::InvalidAgreementAcceptance)?;
    if durable.1.as_slice() != acceptance.agreement_wire()
        || durable.2.as_slice() != acceptance.agreement_commitment()
        || durable.3.as_slice() != acceptance.initial_snapshot_digest()
        || durable.4 != accepted_at
    {
        return Err(BtcRecoveryError::AgreementConflict);
    }
    Ok(())
}

fn reconstruct_checked(
    connection: &Connection,
    acceptance: &BtcAgreementAcceptance,
    initial: &SwapCoordinator,
) -> Result<ReconstructedState, BtcRecoveryError> {
    verify_acceptance(connection, acceptance)?;
    let aggregate = load_durable_aggregate(connection, acceptance)?;
    let aggregate_revision = revision_from_sql(aggregate.revision)?;
    if aggregate.snapshot_version != SNAPSHOT_PAYLOAD_VERSION {
        return Err(BtcRecoveryError::UnsupportedSnapshotVersion(
            aggregate.snapshot_version,
        ));
    }
    if aggregate.evidence_chain_version != EVIDENCE_CHAIN_VERSION {
        return Err(BtcRecoveryError::UnsupportedEvidenceChainVersion(
            aggregate.evidence_chain_version,
        ));
    }
    let durable_evidence_chain_head: [u8; 32] = aggregate
        .evidence_chain_head
        .try_into()
        .map_err(|_| BtcRecoveryError::EvidenceChainMismatch)?;

    let mut coordinator = initial.clone();
    let mut revealing_public_witness = None;
    let mut evidence_chain_head = evidence_chain_genesis(acceptance);
    let mut statement = connection.prepare(
        "
        SELECT aggregate_revision, evidence_kind, payload_version, payload_json
        FROM btc_actor_evidence
        WHERE swap_id = ?1 AND local_role = ?2
        ORDER BY aggregate_revision ASC
        ",
    )?;
    let mut rows = statement.query(params![
        acceptance.swap_id.as_str(),
        participant_name(acceptance.local_role)
    ])?;
    let mut replayed_revision = 0_u64;
    while let Some(row) = rows.next()? {
        let durable_revision = revision_from_sql(row.get(0)?)?;
        let expected_revision = replayed_revision
            .checked_add(1)
            .ok_or(BtcRecoveryError::RevisionOverflow)?;
        if durable_revision != expected_revision {
            return Err(BtcRecoveryError::InvalidSequence {
                revision: expected_revision,
            });
        }
        let durable_kind: String = row.get(1)?;
        let payload_version: i64 = row.get(2)?;
        if payload_version != EVIDENCE_PAYLOAD_VERSION {
            return Err(BtcRecoveryError::UnsupportedEvidenceVersion(
                payload_version,
            ));
        }
        let payload_json: String = row.get(3)?;
        if payload_json.len() > MAX_ENCODED_EVIDENCE_BYTES {
            return Err(BtcRecoveryError::InvalidEvidence {
                revision: durable_revision,
            });
        }
        evidence_chain_head = evidence_chain_append(
            &evidence_chain_head,
            acceptance,
            durable_revision,
            &durable_kind,
            payload_version,
            &payload_json,
        );
        let evidence: BtcLifecycleEvidenceV1 = serde_json::from_str(&payload_json)?;
        if BtcLifecycleEvidenceKind::at_revision(durable_revision) != Some(evidence.kind)
            || durable_kind != evidence.kind.name()
        {
            return Err(BtcRecoveryError::InvalidSequence {
                revision: durable_revision,
            });
        }
        evidence.apply(&mut coordinator, durable_revision)?;
        if let Some(witness) = evidence.revealing_public_witness {
            revealing_public_witness = Some(witness);
        }
        replayed_revision = durable_revision;
    }
    drop(rows);
    drop(statement);

    if replayed_revision != aggregate_revision {
        return Err(BtcRecoveryError::InvalidSequence {
            revision: replayed_revision.saturating_add(1),
        });
    }
    if serde_json::to_string(&coordinator)? != aggregate.snapshot_json {
        return Err(BtcRecoveryError::SnapshotMismatch);
    }
    if evidence_chain_head != durable_evidence_chain_head {
        return Err(BtcRecoveryError::EvidenceChainMismatch);
    }
    Ok(ReconstructedState {
        revision: replayed_revision,
        coordinator,
        revealing_public_witness,
        evidence_chain_head,
    })
}

fn revision_to_sql(revision: u64) -> Result<i64, BtcRecoveryError> {
    i64::try_from(revision).map_err(|_| BtcRecoveryError::RevisionOverflow)
}

fn revision_from_sql(revision: i64) -> Result<u64, BtcRecoveryError> {
    u64::try_from(revision).map_err(|_| BtcRecoveryError::InvalidSequence { revision: 0 })
}
