//! Crash-safe role-local journal for one-way `MuSig2` adaptor signing sessions.
//!
//! Cryptographic parsing and verification remain in the chain SDK. This module
//! owns only immutable transcript bytes, ordered delivery, one-use secret-nonce
//! persistence, and exact idempotent outbox replay.
//!
//! `PoC` security boundary: `nonce_encrypted_at_rest=false`. Secret nonce bytes
//! are plaintext in the owner-only `SQLite` database and `WAL` until consumption.
//! This is crash-safety evidence, not information-security or production signing
//! readiness. No deterministic fixture secret is part of this module.

use std::{fmt, path::Path};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use zeroize::{Zeroize as _, Zeroizing};

use crate::{StoreError, open_configured_connection, open_existing_configured_connection};

const SECRET_NONCE_FINGERPRINT_DOMAIN: &[u8] =
    b"lez-atomic-swaps/adaptor-secret-nonce-fingerprint/v1";

macro_rules! public_bytes {
    ($name:ident, $length:expr, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[must_use]
        pub struct $name([u8; $length]);

        impl $name {
            /// Retains already canonical bytes without interpreting their cryptography.
            pub const fn new(bytes: [u8; $length]) -> Self {
                Self(bytes)
            }

            /// Canonical fixed-width wire bytes.
            pub const fn bytes(&self) -> &[u8; $length] {
                &self.0
            }
        }
    };
}

public_bytes!(
    AdaptorNonceCommitment,
    32,
    "One role-bound commitment to a serialized public nonce."
);
public_bytes!(
    AdaptorPublicNonce,
    66,
    "One canonical two-point `MuSig2` public nonce."
);
public_bytes!(
    AdaptorPartialSignature,
    32,
    "One exact adaptor partial-signature outbox payload."
);
public_bytes!(
    AdaptorPresignature,
    65,
    "One verified aggregate adaptor presignature."
);

/// One serialized `MuSig2` secret nonce retained only until partial persistence.
///
/// Debug output is deliberately redacted and memory is overwritten on drop.
#[must_use]
pub struct SecretNonceBytes([u8; 97]);

impl SecretNonceBytes {
    /// Wraps canonical secret-nonce bytes supplied by the signing implementation.
    pub const fn new(bytes: [u8; 97]) -> Self {
        Self(bytes)
    }

    fn bytes(&self) -> &[u8; 97] {
        &self.0
    }
}

impl fmt::Debug for SecretNonceBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SecretNonceBytes")
            .field(&"[REDACTED]")
            .finish()
    }
}

impl Drop for SecretNonceBytes {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Fixed participant position in an ordered two-party transcript.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum AdaptorSessionRole {
    /// Liquidity provider and key index zero.
    Maker,
    /// Swap initiator and key index one.
    Taker,
}

impl AdaptorSessionRole {
    const fn name(self) -> &'static str {
        match self {
            Self::Maker => "maker",
            Self::Taker => "taker",
        }
    }

    fn parse(value: &str) -> Result<Self, AdaptorSessionJournalError> {
        match value {
            "maker" => Ok(Self::Maker),
            "taker" => Ok(Self::Taker),
            _ => Err(AdaptorSessionJournalError::CorruptSession),
        }
    }
}

/// Immutable byte identity for exactly one role-local signing session.
///
/// `signing_domain` is a caller-derived 32-byte commitment to the chain and
/// signature purpose. Public keys are compressed SEC1 bytes in maker/taker
/// order. Curve parsing remains the chain SDK's responsibility.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct AdaptorSessionIdentity {
    session_id: [u8; 32],
    local_role: AdaptorSessionRole,
    signing_domain: [u8; 32],
    exact_message: [u8; 32],
    adaptor_point: [u8; 33],
    ordered_public_keys: [[u8; 33]; 2],
}

impl AdaptorSessionIdentity {
    /// Constructs an immutable transcript identity from already canonical bytes.
    pub const fn new(
        session_id: [u8; 32],
        local_role: AdaptorSessionRole,
        signing_domain: [u8; 32],
        exact_message: [u8; 32],
        adaptor_point: [u8; 33],
        ordered_public_keys: [[u8; 33]; 2],
    ) -> Self {
        Self {
            session_id,
            local_role,
            signing_domain,
            exact_message,
            adaptor_point,
            ordered_public_keys,
        }
    }

    /// Caller-bound unique session identity.
    #[must_use]
    pub const fn session_id(&self) -> &[u8; 32] {
        &self.session_id
    }

    /// Role owning this database and secret nonce.
    pub const fn local_role(&self) -> AdaptorSessionRole {
        self.local_role
    }

    /// Chain-and-purpose domain commitment.
    #[must_use]
    pub const fn signing_domain(&self) -> &[u8; 32] {
        &self.signing_domain
    }

    /// Exact chain message that may be signed once.
    #[must_use]
    pub const fn exact_message(&self) -> &[u8; 32] {
        &self.exact_message
    }

    /// Public adaptor point shared by the linked chain sessions.
    #[must_use]
    pub const fn adaptor_point(&self) -> &[u8; 33] {
        &self.adaptor_point
    }

    /// Maker/taker compressed public keys in protocol order.
    #[must_use]
    pub const fn ordered_public_keys(&self) -> &[[u8; 33]; 2] {
        &self.ordered_public_keys
    }

    fn validate(&self) -> Result<(), AdaptorSessionJournalError> {
        if self.ordered_public_keys[0] == self.ordered_public_keys[1] {
            Err(AdaptorSessionJournalError::InvalidIdentity)
        } else {
            Ok(())
        }
    }
}

/// Fresh secret/public nonce material to reserve before commitment exposure.
#[must_use]
pub struct AdaptorSessionReservation {
    identity: AdaptorSessionIdentity,
    secret_nonce: SecretNonceBytes,
    own_public_nonce: AdaptorPublicNonce,
    own_commitment: AdaptorNonceCommitment,
}

impl AdaptorSessionReservation {
    /// Binds fresh secret nonce bytes to immutable public transcript bytes.
    pub const fn new(
        identity: AdaptorSessionIdentity,
        secret_nonce: SecretNonceBytes,
        own_public_nonce: AdaptorPublicNonce,
        own_commitment: AdaptorNonceCommitment,
    ) -> Self {
        Self {
            identity,
            secret_nonce,
            own_public_nonce,
            own_commitment,
        }
    }

    /// Immutable transcript identity being reserved.
    pub const fn identity(&self) -> &AdaptorSessionIdentity {
        &self.identity
    }

    /// Commitment safe to expose only after reservation commits.
    pub const fn own_commitment(&self) -> AdaptorNonceCommitment {
        self.own_commitment
    }
}

impl fmt::Debug for AdaptorSessionReservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdaptorSessionReservation")
            .field("identity", &self.identity)
            .field("secret_nonce", &"[REDACTED]")
            .field("own_public_nonce", &self.own_public_nonce)
            .field("own_commitment", &self.own_commitment)
            .finish()
    }
}

/// Monotonic durable phase of one adaptor session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum AdaptorSessionPhase {
    /// Fresh nonce is durable; only the local commitment may be exposed.
    Reserved,
    /// The peer commitment is durable, so the local public nonce may be revealed.
    CommitmentExchanged,
    /// The peer public nonce was verified by the SDK and is durable.
    NoncesExchanged,
    /// The secret nonce was consumed atomically with exact partial persistence.
    PartialPersisted,
    /// The peer partial was verified by the SDK and is durable.
    PeerPartialVerified,
    /// The verified aggregate presignature is durable.
    PresignatureVerified,
}

impl AdaptorSessionPhase {
    fn parse(value: &str) -> Result<Self, AdaptorSessionJournalError> {
        match value {
            "reserved" => Ok(Self::Reserved),
            "commitment_exchanged" => Ok(Self::CommitmentExchanged),
            "nonces_exchanged" => Ok(Self::NoncesExchanged),
            "partial_persisted" => Ok(Self::PartialPersisted),
            "peer_partial_verified" => Ok(Self::PeerPartialVerified),
            "presignature_verified" => Ok(Self::PresignatureVerified),
            _ => Err(AdaptorSessionJournalError::CorruptSession),
        }
    }
}

/// Result of a fresh or exact replayed nonce reservation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct ReservationCommit {
    phase: AdaptorSessionPhase,
    own_commitment: AdaptorNonceCommitment,
    was_replay: bool,
}

impl ReservationCommit {
    /// Current durable session phase.
    pub const fn phase(&self) -> AdaptorSessionPhase {
        self.phase
    }

    /// Commitment whose underlying nonce was already durably reserved.
    pub const fn own_commitment(&self) -> AdaptorNonceCommitment {
        self.own_commitment
    }

    /// Whether the exact reservation already existed.
    #[must_use]
    pub const fn was_replay(&self) -> bool {
        self.was_replay
    }
}

/// Result of an ordered, idempotent public-state transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct AdaptorTransitionCommit {
    phase: AdaptorSessionPhase,
    was_replay: bool,
}

impl AdaptorTransitionCommit {
    /// Current durable session phase.
    pub const fn phase(&self) -> AdaptorSessionPhase {
        self.phase
    }

    /// Whether the exact bytes were already durable.
    #[must_use]
    pub const fn was_replay(&self) -> bool {
        self.was_replay
    }
}

/// Exact partial-signature outbox bytes returned after first commit or replay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct PartialSignatureCommit {
    partial: AdaptorPartialSignature,
    was_replay: bool,
}

impl PartialSignatureCommit {
    /// Exact durable bytes that must be sent on every retry.
    pub const fn partial(&self) -> AdaptorPartialSignature {
        self.partial
    }

    /// Whether signing was skipped because the outbox already existed.
    #[must_use]
    pub const fn was_replay(&self) -> bool {
        self.was_replay
    }
}

/// Secret-free durable view of one signing session.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct AdaptorSessionSnapshot {
    identity: AdaptorSessionIdentity,
    phase: AdaptorSessionPhase,
    own_commitment: AdaptorNonceCommitment,
    own_public_nonce: Option<AdaptorPublicNonce>,
    peer_commitment: Option<AdaptorNonceCommitment>,
    peer_public_nonce: Option<AdaptorPublicNonce>,
    own_partial: Option<AdaptorPartialSignature>,
    peer_partial: Option<AdaptorPartialSignature>,
    presignature: Option<AdaptorPresignature>,
}

impl AdaptorSessionSnapshot {
    /// Immutable transcript identity.
    pub const fn identity(&self) -> &AdaptorSessionIdentity {
        &self.identity
    }

    /// Current monotonic phase.
    pub const fn phase(&self) -> AdaptorSessionPhase {
        self.phase
    }

    /// Local commitment available immediately after reservation.
    pub const fn own_commitment(&self) -> AdaptorNonceCommitment {
        self.own_commitment
    }

    /// Local public nonce, hidden until the peer commitment is durable.
    #[must_use]
    pub const fn own_public_nonce(&self) -> Option<AdaptorPublicNonce> {
        self.own_public_nonce
    }

    /// Exact peer commitment, when received.
    #[must_use]
    pub const fn peer_commitment(&self) -> Option<AdaptorNonceCommitment> {
        self.peer_commitment
    }

    /// Exact SDK-verified peer public nonce, when received.
    #[must_use]
    pub const fn peer_public_nonce(&self) -> Option<AdaptorPublicNonce> {
        self.peer_public_nonce
    }

    /// Exact local partial outbox bytes, when signing has completed.
    #[must_use]
    pub const fn own_partial(&self) -> Option<AdaptorPartialSignature> {
        self.own_partial
    }

    /// Exact SDK-verified peer partial bytes, when received.
    #[must_use]
    pub const fn peer_partial(&self) -> Option<AdaptorPartialSignature> {
        self.peer_partial
    }

    /// Exact SDK-verified aggregate presignature, when complete.
    #[must_use]
    pub const fn presignature(&self) -> Option<AdaptorPresignature> {
        self.presignature
    }
}

/// Immutable inputs made available to exactly one signing callback.
///
/// Debug output never includes the secret nonce.
#[must_use]
pub struct SigningMaterial<'a> {
    identity: &'a AdaptorSessionIdentity,
    secret_nonce: &'a SecretNonceBytes,
    own_public_nonce: AdaptorPublicNonce,
    peer_public_nonce: AdaptorPublicNonce,
}

impl SigningMaterial<'_> {
    /// Immutable transcript identity that the nonce is bound to.
    pub const fn identity(&self) -> &AdaptorSessionIdentity {
        self.identity
    }

    /// Serialized one-use secret nonce for the SDK signing primitive.
    #[must_use]
    pub const fn secret_nonce(&self) -> &[u8; 97] {
        &self.secret_nonce.0
    }

    /// Local public nonce corresponding to the secret nonce.
    pub const fn own_public_nonce(&self) -> AdaptorPublicNonce {
        self.own_public_nonce
    }

    /// SDK-verified peer public nonce fixed before signing.
    pub const fn peer_public_nonce(&self) -> AdaptorPublicNonce {
        self.peer_public_nonce
    }
}

impl fmt::Debug for SigningMaterial<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SigningMaterial")
            .field("identity", self.identity)
            .field("secret_nonce", &"[REDACTED]")
            .field("own_public_nonce", &self.own_public_nonce)
            .field("peer_public_nonce", &self.peer_public_nonce)
            .finish()
    }
}

/// Durable adaptor-session journal failure.
#[derive(Debug, Error)]
pub enum AdaptorSessionJournalError {
    /// The shared secure `SQLite` store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The caller supplied duplicate ordered participant keys.
    #[error("adaptor session identity is invalid")]
    InvalidIdentity,
    /// The requested session does not exist.
    #[error("adaptor session does not exist")]
    MissingSession,
    /// Immutable identity or exact bytes conflict with an existing session.
    #[error("adaptor session conflicts with durable bytes")]
    SessionConflict,
    /// A secret nonce fingerprint was already reserved by another session.
    #[error("adaptor secret nonce was already reserved")]
    SecretNonceReuse,
    /// The requested transition is out of order.
    #[error("adaptor session transition is out of order")]
    InvalidPhase,
    /// Persisted row bytes or phase invariants are invalid.
    #[error("persisted adaptor session is corrupt")]
    CorruptSession,
    /// The signing callback failed without producing an outbox payload.
    #[error("adaptor partial signing failed")]
    SigningFailed,
}

impl From<rusqlite::Error> for AdaptorSessionJournalError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(StoreError::Sqlite(error))
    }
}

/// `SQLite`-backed, role-local one-use adaptor-session journal.
#[derive(Debug)]
pub struct SqliteAdaptorSessionJournal {
    connection: Connection,
}

impl SqliteAdaptorSessionJournal {
    /// Opens or creates a journal using the store's owner-private file checks,
    /// `WAL`, `FULL` synchronous writes, secure-delete, and busy timeout.
    ///
    /// Maker and taker runners must pass different database paths.
    ///
    /// # Errors
    ///
    /// Returns an error when the private database cannot be opened, configured, or migrated.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AdaptorSessionJournalError> {
        let mut connection = open_configured_connection(path)?;
        migrate_adaptor_session_journal(&mut connection)?;
        Ok(Self { connection })
    }

    /// Opens an existing journal without creating a missing signer database.
    ///
    /// Schema migration may run for an existing owner-private database, but a
    /// missing, unsafe, or invalid path fails closed. Callers must still load
    /// and compare the expected session identity before using a presignature.
    ///
    /// # Errors
    ///
    /// Returns an error when the private database does not already exist or
    /// cannot be configured or migrated safely.
    pub fn open_existing(path: impl AsRef<Path>) -> Result<Self, AdaptorSessionJournalError> {
        let mut connection = open_existing_configured_connection(path)?;
        migrate_adaptor_session_journal(&mut connection)?;
        Ok(Self { connection })
    }

    /// Atomically reserves fresh secret nonce bytes before returning a commitment.
    ///
    /// An exact retry returns the original commitment and never rearms a consumed
    /// nonce. The same secret nonce cannot be reserved under another session ID.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid or conflicting identity, nonce reuse, or database failure.
    #[allow(clippy::needless_pass_by_value)] // Taking ownership guarantees zeroization on return.
    pub fn reserve(
        &mut self,
        reservation: AdaptorSessionReservation,
    ) -> Result<ReservationCommit, AdaptorSessionJournalError> {
        reservation.identity.validate()?;
        let fingerprint = secret_nonce_fingerprint(reservation.secret_nonce.bytes());
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        if let Some(existing) = load_session(&transaction, reservation.identity.session_id())? {
            existing.validate_integrity()?;
            if existing.identity != reservation.identity
                || existing.own_public_nonce != reservation.own_public_nonce
                || existing.own_commitment != reservation.own_commitment
                || existing.secret_nonce_fingerprint != fingerprint
            {
                return Err(AdaptorSessionJournalError::SessionConflict);
            }
            let result = ReservationCommit {
                phase: existing.phase,
                own_commitment: existing.own_commitment,
                was_replay: true,
            };
            transaction.commit()?;
            return Ok(result);
        }

        let reused = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM adaptor_sessions WHERE secret_nonce_fingerprint = ?1)",
            params![fingerprint.as_slice()],
            |row| row.get::<_, bool>(0),
        )?;
        if reused {
            return Err(AdaptorSessionJournalError::SecretNonceReuse);
        }

        transaction.execute(
            "
            INSERT INTO adaptor_sessions (
                session_id, local_role, signing_domain, exact_message,
                adaptor_point, maker_public_key, taker_public_key,
                secret_nonce, secret_nonce_fingerprint,
                own_commitment, own_public_nonce, phase
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'reserved')
            ",
            params![
                reservation.identity.session_id.as_slice(),
                reservation.identity.local_role.name(),
                reservation.identity.signing_domain.as_slice(),
                reservation.identity.exact_message.as_slice(),
                reservation.identity.adaptor_point.as_slice(),
                reservation.identity.ordered_public_keys[0].as_slice(),
                reservation.identity.ordered_public_keys[1].as_slice(),
                reservation.secret_nonce.bytes().as_slice(),
                fingerprint.as_slice(),
                reservation.own_commitment.bytes().as_slice(),
                reservation.own_public_nonce.bytes().as_slice(),
            ],
        )?;
        transaction.commit()?;
        Ok(ReservationCommit {
            phase: AdaptorSessionPhase::Reserved,
            own_commitment: reservation.own_commitment,
            was_replay: false,
        })
    }

    /// Records the peer commitment before either public nonce is revealed.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing/conflicting session, invalid phase, or database failure.
    pub fn record_peer_commitment(
        &mut self,
        identity: &AdaptorSessionIdentity,
        commitment: AdaptorNonceCommitment,
    ) -> Result<AdaptorTransitionCommit, AdaptorSessionJournalError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = required_session(&transaction, identity)?;
        if let Some(durable) = existing.peer_commitment {
            if durable != commitment {
                return Err(AdaptorSessionJournalError::SessionConflict);
            }
            let result = AdaptorTransitionCommit {
                phase: existing.phase,
                was_replay: true,
            };
            transaction.commit()?;
            return Ok(result);
        }
        if existing.phase != AdaptorSessionPhase::Reserved {
            return Err(AdaptorSessionJournalError::InvalidPhase);
        }
        let updated = transaction.execute(
            "
            UPDATE adaptor_sessions
            SET peer_commitment = ?1, phase = 'commitment_exchanged'
            WHERE session_id = ?2 AND phase = 'reserved' AND peer_commitment IS NULL
            ",
            params![
                commitment.bytes().as_slice(),
                identity.session_id.as_slice()
            ],
        )?;
        ensure_single_update(updated)?;
        transaction.commit()?;
        Ok(AdaptorTransitionCommit {
            phase: AdaptorSessionPhase::CommitmentExchanged,
            was_replay: false,
        })
    }

    /// Returns the exact local public nonce only after peer commitment persistence.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing/conflicting session, invalid phase, or database failure.
    pub fn reveal_own_public_nonce(
        &self,
        identity: &AdaptorSessionIdentity,
    ) -> Result<AdaptorPublicNonce, AdaptorSessionJournalError> {
        let existing = required_session(&self.connection, identity)?;
        if existing.peer_commitment.is_none() {
            Err(AdaptorSessionJournalError::InvalidPhase)
        } else {
            Ok(existing.own_public_nonce)
        }
    }

    /// Records a peer public nonce after the SDK verifies that it opens the
    /// previously persisted commitment.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing/conflicting session, invalid phase, or database failure.
    pub fn record_verified_peer_public_nonce(
        &mut self,
        identity: &AdaptorSessionIdentity,
        public_nonce: AdaptorPublicNonce,
    ) -> Result<AdaptorTransitionCommit, AdaptorSessionJournalError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = required_session(&transaction, identity)?;
        if let Some(durable) = existing.peer_public_nonce {
            if durable != public_nonce {
                return Err(AdaptorSessionJournalError::SessionConflict);
            }
            let result = AdaptorTransitionCommit {
                phase: existing.phase,
                was_replay: true,
            };
            transaction.commit()?;
            return Ok(result);
        }
        if existing.phase != AdaptorSessionPhase::CommitmentExchanged
            || existing.peer_commitment.is_none()
        {
            return Err(AdaptorSessionJournalError::InvalidPhase);
        }
        let updated = transaction.execute(
            "
            UPDATE adaptor_sessions
            SET peer_public_nonce = ?1, phase = 'nonces_exchanged'
            WHERE session_id = ?2 AND phase = 'commitment_exchanged'
              AND peer_commitment IS NOT NULL AND peer_public_nonce IS NULL
            ",
            params![
                public_nonce.bytes().as_slice(),
                identity.session_id.as_slice()
            ],
        )?;
        ensure_single_update(updated)?;
        transaction.commit()?;
        Ok(AdaptorTransitionCommit {
            phase: AdaptorSessionPhase::NoncesExchanged,
            was_replay: false,
        })
    }

    /// Runs one signing callback under an immediate transaction, then atomically
    /// removes the secret nonce and persists the exact partial outbox bytes.
    ///
    /// The callback is a pure cryptographic boundary: it must perform no I/O,
    /// spawn no work, and must not copy or otherwise let secret or partial bytes
    /// escape. Rust cannot prove closure purity. This `PoC` API returns the partial
    /// to its caller only after commit; production signing needs a narrow signer
    /// service or HSM boundary that enforces the same rule.
    ///
    /// Concurrent and post-restart retries return the original partial without
    /// invoking `signer`. If the process fails before commit, `SQLite` rolls back to
    /// the same immutable message and peer nonce, so retry cannot cross transcripts.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing/conflicting session, invalid phase, signer failure,
    /// corrupt nonce state, or database failure.
    pub fn sign_and_persist_partial<F>(
        &mut self,
        identity: &AdaptorSessionIdentity,
        signer: F,
    ) -> Result<PartialSignatureCommit, AdaptorSessionJournalError>
    where
        F: FnOnce(SigningMaterial<'_>) -> Result<AdaptorPartialSignature, ()>,
    {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = required_session(&transaction, identity)?;
        if let Some(partial) = existing.own_partial {
            let result = PartialSignatureCommit {
                partial,
                was_replay: true,
            };
            transaction.commit()?;
            return Ok(result);
        }
        if existing.phase != AdaptorSessionPhase::NoncesExchanged {
            return Err(AdaptorSessionJournalError::InvalidPhase);
        }
        let secret_nonce = existing
            .secret_nonce
            .as_ref()
            .ok_or(AdaptorSessionJournalError::CorruptSession)?;
        if secret_nonce_fingerprint(secret_nonce.bytes()) != existing.secret_nonce_fingerprint {
            return Err(AdaptorSessionJournalError::CorruptSession);
        }
        let peer_public_nonce = existing
            .peer_public_nonce
            .ok_or(AdaptorSessionJournalError::CorruptSession)?;
        let partial = signer(SigningMaterial {
            identity: &existing.identity,
            secret_nonce,
            own_public_nonce: existing.own_public_nonce,
            peer_public_nonce,
        })
        .map_err(|()| AdaptorSessionJournalError::SigningFailed)?;
        let updated = transaction.execute(
            "
            UPDATE adaptor_sessions
            SET secret_nonce = NULL, own_partial = ?1, phase = 'partial_persisted'
            WHERE session_id = ?2 AND phase = 'nonces_exchanged'
              AND secret_nonce IS NOT NULL AND own_partial IS NULL
            ",
            params![partial.bytes().as_slice(), identity.session_id.as_slice()],
        )?;
        ensure_single_update(updated)?;
        transaction.commit()?;
        Ok(PartialSignatureCommit {
            partial,
            was_replay: false,
        })
    }

    /// Persists an exact peer partial only after SDK verification.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing/conflicting session, invalid phase, or database failure.
    pub fn record_verified_peer_partial(
        &mut self,
        identity: &AdaptorSessionIdentity,
        partial: AdaptorPartialSignature,
    ) -> Result<AdaptorTransitionCommit, AdaptorSessionJournalError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = required_session(&transaction, identity)?;
        if let Some(durable) = existing.peer_partial {
            if durable != partial {
                return Err(AdaptorSessionJournalError::SessionConflict);
            }
            let result = AdaptorTransitionCommit {
                phase: existing.phase,
                was_replay: true,
            };
            transaction.commit()?;
            return Ok(result);
        }
        if existing.phase != AdaptorSessionPhase::PartialPersisted || existing.own_partial.is_none()
        {
            return Err(AdaptorSessionJournalError::InvalidPhase);
        }
        let updated = transaction.execute(
            "
            UPDATE adaptor_sessions
            SET peer_partial = ?1, phase = 'peer_partial_verified'
            WHERE session_id = ?2 AND phase = 'partial_persisted'
              AND own_partial IS NOT NULL AND peer_partial IS NULL
            ",
            params![partial.bytes().as_slice(), identity.session_id.as_slice()],
        )?;
        ensure_single_update(updated)?;
        transaction.commit()?;
        Ok(AdaptorTransitionCommit {
            phase: AdaptorSessionPhase::PeerPartialVerified,
            was_replay: false,
        })
    }

    /// Persists the exact aggregate presignature only after SDK verification.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing/conflicting session, invalid phase, or database failure.
    pub fn record_verified_presignature(
        &mut self,
        identity: &AdaptorSessionIdentity,
        presignature: AdaptorPresignature,
    ) -> Result<AdaptorTransitionCommit, AdaptorSessionJournalError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = required_session(&transaction, identity)?;
        if let Some(durable) = existing.presignature {
            if durable != presignature {
                return Err(AdaptorSessionJournalError::SessionConflict);
            }
            let result = AdaptorTransitionCommit {
                phase: existing.phase,
                was_replay: true,
            };
            transaction.commit()?;
            return Ok(result);
        }
        if existing.phase != AdaptorSessionPhase::PeerPartialVerified
            || existing.peer_partial.is_none()
        {
            return Err(AdaptorSessionJournalError::InvalidPhase);
        }
        let updated = transaction.execute(
            "
            UPDATE adaptor_sessions
            SET presignature = ?1, phase = 'presignature_verified'
            WHERE session_id = ?2 AND phase = 'peer_partial_verified'
              AND peer_partial IS NOT NULL AND presignature IS NULL
            ",
            params![
                presignature.bytes().as_slice(),
                identity.session_id.as_slice()
            ],
        )?;
        ensure_single_update(updated)?;
        transaction.commit()?;
        Ok(AdaptorTransitionCommit {
            phase: AdaptorSessionPhase::PresignatureVerified,
            was_replay: false,
        })
    }

    /// Loads one secret-free durable snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when durable bytes are corrupt or the database read fails.
    pub fn load(
        &self,
        session_id: &[u8; 32],
    ) -> Result<Option<AdaptorSessionSnapshot>, AdaptorSessionJournalError> {
        load_session(&self.connection, session_id)?
            .map(|session| {
                session.validate_integrity()?;
                Ok(session.snapshot())
            })
            .transpose()
    }
}

fn ensure_single_update(updated: usize) -> Result<(), AdaptorSessionJournalError> {
    if updated == 1 {
        Ok(())
    } else {
        Err(AdaptorSessionJournalError::SessionConflict)
    }
}

fn secret_nonce_fingerprint(secret_nonce: &[u8; 97]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(SECRET_NONCE_FINGERPRINT_DOMAIN);
    hasher.update(secret_nonce);
    hasher.finalize().into()
}

fn migrate_adaptor_session_journal(
    connection: &mut Connection,
) -> Result<(), AdaptorSessionJournalError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS adaptor_sessions (
            session_id               BLOB PRIMARY KEY CHECK (length(session_id) = 32),
            local_role               TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
            signing_domain           BLOB NOT NULL CHECK (length(signing_domain) = 32),
            exact_message            BLOB NOT NULL CHECK (length(exact_message) = 32),
            adaptor_point            BLOB NOT NULL CHECK (length(adaptor_point) = 33),
            maker_public_key         BLOB NOT NULL CHECK (length(maker_public_key) = 33),
            taker_public_key         BLOB NOT NULL CHECK (length(taker_public_key) = 33),
            secret_nonce             BLOB CHECK (secret_nonce IS NULL OR length(secret_nonce) = 97),
            secret_nonce_fingerprint BLOB NOT NULL UNIQUE
                CHECK (length(secret_nonce_fingerprint) = 32),
            own_commitment           BLOB NOT NULL CHECK (length(own_commitment) = 32),
            own_public_nonce         BLOB NOT NULL CHECK (length(own_public_nonce) = 66),
            peer_commitment          BLOB CHECK (peer_commitment IS NULL OR length(peer_commitment) = 32),
            peer_public_nonce        BLOB CHECK (peer_public_nonce IS NULL OR length(peer_public_nonce) = 66),
            own_partial              BLOB CHECK (own_partial IS NULL OR length(own_partial) = 32),
            peer_partial             BLOB CHECK (peer_partial IS NULL OR length(peer_partial) = 32),
            presignature             BLOB CHECK (presignature IS NULL OR length(presignature) = 65),
            phase                    TEXT NOT NULL CHECK (phase IN (
                'reserved', 'commitment_exchanged', 'nonces_exchanged',
                'partial_persisted', 'peer_partial_verified', 'presignature_verified'
            )),
            CHECK (maker_public_key <> taker_public_key),
            CHECK (
                (phase = 'reserved'
                    AND secret_nonce IS NOT NULL
                    AND peer_commitment IS NULL AND peer_public_nonce IS NULL
                    AND own_partial IS NULL AND peer_partial IS NULL AND presignature IS NULL)
                OR (phase = 'commitment_exchanged'
                    AND secret_nonce IS NOT NULL
                    AND peer_commitment IS NOT NULL AND peer_public_nonce IS NULL
                    AND own_partial IS NULL AND peer_partial IS NULL AND presignature IS NULL)
                OR (phase = 'nonces_exchanged'
                    AND secret_nonce IS NOT NULL
                    AND peer_commitment IS NOT NULL AND peer_public_nonce IS NOT NULL
                    AND own_partial IS NULL AND peer_partial IS NULL AND presignature IS NULL)
                OR (phase = 'partial_persisted'
                    AND secret_nonce IS NULL
                    AND peer_commitment IS NOT NULL AND peer_public_nonce IS NOT NULL
                    AND own_partial IS NOT NULL AND peer_partial IS NULL AND presignature IS NULL)
                OR (phase = 'peer_partial_verified'
                    AND secret_nonce IS NULL
                    AND peer_commitment IS NOT NULL AND peer_public_nonce IS NOT NULL
                    AND own_partial IS NOT NULL AND peer_partial IS NOT NULL AND presignature IS NULL)
                OR (phase = 'presignature_verified'
                    AND secret_nonce IS NULL
                    AND peer_commitment IS NOT NULL AND peer_public_nonce IS NOT NULL
                    AND own_partial IS NOT NULL AND peer_partial IS NOT NULL AND presignature IS NOT NULL)
            )
        ) STRICT;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

struct EncodedSession {
    session_id: Vec<u8>,
    local_role: String,
    signing_domain: Vec<u8>,
    exact_message: Vec<u8>,
    adaptor_point: Vec<u8>,
    maker_public_key: Vec<u8>,
    taker_public_key: Vec<u8>,
    secret_nonce: Option<Zeroizing<Vec<u8>>>,
    secret_nonce_fingerprint: Vec<u8>,
    own_commitment: Vec<u8>,
    own_public_nonce: Vec<u8>,
    peer_commitment: Option<Vec<u8>>,
    peer_public_nonce: Option<Vec<u8>>,
    own_partial: Option<Vec<u8>>,
    peer_partial: Option<Vec<u8>>,
    presignature: Option<Vec<u8>>,
    phase: String,
}

struct PersistedSession {
    identity: AdaptorSessionIdentity,
    secret_nonce: Option<SecretNonceBytes>,
    secret_nonce_fingerprint: [u8; 32],
    own_commitment: AdaptorNonceCommitment,
    own_public_nonce: AdaptorPublicNonce,
    peer_commitment: Option<AdaptorNonceCommitment>,
    peer_public_nonce: Option<AdaptorPublicNonce>,
    own_partial: Option<AdaptorPartialSignature>,
    peer_partial: Option<AdaptorPartialSignature>,
    presignature: Option<AdaptorPresignature>,
    phase: AdaptorSessionPhase,
}

impl PersistedSession {
    fn validate_integrity(&self) -> Result<(), AdaptorSessionJournalError> {
        self.identity.validate()?;
        match (&self.secret_nonce, self.phase) {
            (
                Some(secret),
                AdaptorSessionPhase::Reserved
                | AdaptorSessionPhase::CommitmentExchanged
                | AdaptorSessionPhase::NoncesExchanged,
            ) if secret_nonce_fingerprint(secret.bytes()) == self.secret_nonce_fingerprint => {
                Ok(())
            }
            (
                None,
                AdaptorSessionPhase::PartialPersisted
                | AdaptorSessionPhase::PeerPartialVerified
                | AdaptorSessionPhase::PresignatureVerified,
            ) => Ok(()),
            _ => Err(AdaptorSessionJournalError::CorruptSession),
        }
    }

    fn snapshot(&self) -> AdaptorSessionSnapshot {
        AdaptorSessionSnapshot {
            identity: self.identity.clone(),
            phase: self.phase,
            own_commitment: self.own_commitment,
            own_public_nonce: self.peer_commitment.map(|_| self.own_public_nonce),
            peer_commitment: self.peer_commitment,
            peer_public_nonce: self.peer_public_nonce,
            own_partial: self.own_partial,
            peer_partial: self.peer_partial,
            presignature: self.presignature,
        }
    }
}

fn required_session(
    connection: &Connection,
    identity: &AdaptorSessionIdentity,
) -> Result<PersistedSession, AdaptorSessionJournalError> {
    identity.validate()?;
    let session = load_session(connection, identity.session_id())?
        .ok_or(AdaptorSessionJournalError::MissingSession)?;
    session.validate_integrity()?;
    if session.identity != *identity {
        return Err(AdaptorSessionJournalError::SessionConflict);
    }
    Ok(session)
}

fn load_session(
    connection: &Connection,
    session_id: &[u8; 32],
) -> Result<Option<PersistedSession>, AdaptorSessionJournalError> {
    let encoded = connection
        .query_row(
            "
            SELECT session_id, local_role, signing_domain, exact_message,
                   adaptor_point, maker_public_key, taker_public_key,
                   secret_nonce, secret_nonce_fingerprint,
                   own_commitment, own_public_nonce, peer_commitment,
                   peer_public_nonce, own_partial, peer_partial, presignature, phase
            FROM adaptor_sessions WHERE session_id = ?1
            ",
            params![session_id.as_slice()],
            |row| {
                Ok(EncodedSession {
                    session_id: row.get(0)?,
                    local_role: row.get(1)?,
                    signing_domain: row.get(2)?,
                    exact_message: row.get(3)?,
                    adaptor_point: row.get(4)?,
                    maker_public_key: row.get(5)?,
                    taker_public_key: row.get(6)?,
                    secret_nonce: row.get::<_, Option<Vec<u8>>>(7)?.map(Zeroizing::new),
                    secret_nonce_fingerprint: row.get(8)?,
                    own_commitment: row.get(9)?,
                    own_public_nonce: row.get(10)?,
                    peer_commitment: row.get(11)?,
                    peer_public_nonce: row.get(12)?,
                    own_partial: row.get(13)?,
                    peer_partial: row.get(14)?,
                    presignature: row.get(15)?,
                    phase: row.get(16)?,
                })
            },
        )
        .optional()?;
    encoded.map(decode_session).transpose()
}

fn decode_session(encoded: EncodedSession) -> Result<PersistedSession, AdaptorSessionJournalError> {
    let identity = AdaptorSessionIdentity::new(
        fixed_bytes(encoded.session_id)?,
        AdaptorSessionRole::parse(&encoded.local_role)?,
        fixed_bytes(encoded.signing_domain)?,
        fixed_bytes(encoded.exact_message)?,
        fixed_bytes(encoded.adaptor_point)?,
        [
            fixed_bytes(encoded.maker_public_key)?,
            fixed_bytes(encoded.taker_public_key)?,
        ],
    );
    let session = PersistedSession {
        identity,
        secret_nonce: encoded
            .secret_nonce
            .map(|bytes| fixed_secret_nonce(&bytes))
            .transpose()?
            .map(SecretNonceBytes::new),
        secret_nonce_fingerprint: fixed_bytes(encoded.secret_nonce_fingerprint)?,
        own_commitment: AdaptorNonceCommitment::new(fixed_bytes(encoded.own_commitment)?),
        own_public_nonce: AdaptorPublicNonce::new(fixed_bytes(encoded.own_public_nonce)?),
        peer_commitment: encoded
            .peer_commitment
            .map(fixed_bytes)
            .transpose()?
            .map(AdaptorNonceCommitment::new),
        peer_public_nonce: encoded
            .peer_public_nonce
            .map(fixed_bytes)
            .transpose()?
            .map(AdaptorPublicNonce::new),
        own_partial: encoded
            .own_partial
            .map(fixed_bytes)
            .transpose()?
            .map(AdaptorPartialSignature::new),
        peer_partial: encoded
            .peer_partial
            .map(fixed_bytes)
            .transpose()?
            .map(AdaptorPartialSignature::new),
        presignature: encoded
            .presignature
            .map(fixed_bytes)
            .transpose()?
            .map(AdaptorPresignature::new),
        phase: AdaptorSessionPhase::parse(&encoded.phase)?,
    };
    session.validate_integrity()?;
    Ok(session)
}

fn fixed_secret_nonce(bytes: &Zeroizing<Vec<u8>>) -> Result<[u8; 97], AdaptorSessionJournalError> {
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| AdaptorSessionJournalError::CorruptSession)
}

fn fixed_bytes<const LENGTH: usize>(
    bytes: Vec<u8>,
) -> Result<[u8; LENGTH], AdaptorSessionJournalError> {
    bytes
        .try_into()
        .map_err(|_| AdaptorSessionJournalError::CorruptSession)
}
