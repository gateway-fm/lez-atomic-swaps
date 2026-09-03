//! One role's durable seat in a two-party adaptor ceremony, driven in-process.
//!
//! The one-shot CLI ([`crate::execute`]) and the Nodes' Chat methods share this
//! seat. Every step is a journal transition keyed by the session id, so a
//! replayed step returns the same public bytes and never consumes a second
//! secret nonce; the journal, not the process boundary, is what makes a retry
//! safe. Packets are the canonical JSON bytes both roles already exchange.

use std::fmt;

use lez_adaptor_signature::{
    FreshAdaptorNonce, PersistedAdaptorSigningMaterial, aggregate_adaptor_presignature,
    sign_persisted_adaptor_partial, verify_adaptor_partial_signature, verify_nonce_commitment,
};
use lez_swap_store::{
    AdaptorNonceCommitment, AdaptorPartialSignature, AdaptorPresignature, AdaptorPublicNonce,
    AdaptorSessionIdentity, AdaptorSessionPhase, AdaptorSessionReservation, AdaptorSessionSnapshot,
    SecretNonceBytes, SqliteAdaptorSessionJournal,
};
use zeroize::Zeroizing;

use crate::{
    RunnerError,
    protocol::{self, PacketKind, Role, ValidatedSession},
};

/// The journal-backed seat of one role in one signing session.
pub struct CeremonySeat {
    journal: SqliteAdaptorSessionJournal,
    identity: AdaptorSessionIdentity,
    session: ValidatedSession,
    role: Role,
}

impl fmt::Debug for CeremonySeat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CeremonySeat")
            .field("role", &self.role)
            .field("session_id", &hex::encode(self.session.session_id()))
            .finish_non_exhaustive()
    }
}

impl CeremonySeat {
    /// Opens (creating when absent) the role-local journal for `session`.
    ///
    /// # Errors
    /// Fails when the journal cannot be opened or belongs to another session or role.
    pub fn open(
        journal: &std::path::Path,
        session: ValidatedSession,
        role: Role,
    ) -> Result<Self, RunnerError> {
        let identity = session.identity(role);
        let journal = SqliteAdaptorSessionJournal::open(journal)?;
        Ok(Self {
            journal,
            identity,
            session,
            role,
        })
    }

    /// Opens a journal that must already exist.
    ///
    /// # Errors
    /// Fails when the journal is missing or belongs to another session or role.
    pub fn open_existing(
        journal: &std::path::Path,
        session: ValidatedSession,
        role: Role,
    ) -> Result<Self, RunnerError> {
        let identity = session.identity(role);
        let journal = SqliteAdaptorSessionJournal::open_existing(journal)?;
        Ok(Self {
            journal,
            identity,
            session,
            role,
        })
    }

    #[must_use]
    pub const fn role(&self) -> Role {
        self.role
    }

    pub const fn session(&self) -> &ValidatedSession {
        &self.session
    }

    /// The durable phase, or `None` before `reserve`.
    ///
    /// # Errors
    /// Fails when the journal cannot be read or belongs to another session or role.
    pub fn phase(&self) -> Result<Option<AdaptorSessionPhase>, RunnerError> {
        Ok(self.snapshot()?.map(|snapshot| snapshot.phase()))
    }

    /// Reserves this role's nonce (once) and returns the commitment packet.
    ///
    /// A replay returns the original commitment without touching the key.
    ///
    /// # Errors
    /// Fails when the key does not match this role's public key, the nonce was already
    /// consumed under another session, or the journal refuses the reservation.
    pub fn reserve(&mut self, secret_key: &Zeroizing<[u8; 32]>) -> Result<Vec<u8>, RunnerError> {
        self.reserve_with(|| Ok(Zeroizing::new(**secret_key)))
    }

    /// Like [`Self::reserve`], but loads the key only when a fresh reservation
    /// is needed, so a replay never opens key material.
    ///
    /// # Errors
    /// Fails as [`Self::reserve`] does, or with the loader's error.
    pub fn reserve_with(
        &mut self,
        load_secret_key: impl FnOnce() -> Result<Zeroizing<[u8; 32]>, RunnerError>,
    ) -> Result<Vec<u8>, RunnerError> {
        if let Some(snapshot) = self.snapshot()? {
            return self.packet(
                PacketKind::NonceCommitment,
                *snapshot.own_commitment().bytes(),
            );
        }
        let secret_key = load_secret_key()?;
        let fresh =
            FreshAdaptorNonce::generate(self.session.context(), self.role.sdk(), *secret_key)
                .map_err(|_| RunnerError::CryptographicValidation)?;
        if fresh.context_binding() != self.session.context_binding() {
            return Err(RunnerError::CryptographicValidation);
        }
        let commit = self.journal.reserve(AdaptorSessionReservation::new(
            self.identity.clone(),
            SecretNonceBytes::new(*fresh.secret_nonce()),
            AdaptorPublicNonce::new(fresh.public_nonce()),
            AdaptorNonceCommitment::new(fresh.commitment()),
        ))?;
        self.packet(
            PacketKind::NonceCommitment,
            *commit.own_commitment().bytes(),
        )
    }

    /// Records the peer's commitment packet; the same bytes replay cleanly.
    ///
    /// # Errors
    /// Fails before `reserve`, on a packet for another session, kind or sender, or when a
    /// different peer commitment is already durable.
    pub fn accept_commitment(&mut self, packet: &[u8]) -> Result<(), RunnerError> {
        let _ = self.required_snapshot()?;
        let peer_commitment: [u8; 32] = self.peer_payload(packet, PacketKind::NonceCommitment)?;
        let _ = self
            .journal
            .record_peer_commitment(&self.identity, AdaptorNonceCommitment::new(peer_commitment))?;
        Ok(())
    }

    /// Reveals this role's public nonce; refused until the peer committed.
    ///
    /// # Errors
    /// Fails before the peer's commitment is durable.
    pub fn reveal_nonce(&mut self) -> Result<Vec<u8>, RunnerError> {
        let _ = self.required_snapshot()?;
        let public_nonce = self.journal.reveal_own_public_nonce(&self.identity)?;
        self.packet(PacketKind::PublicNonce, *public_nonce.bytes())
    }

    /// Verifies the peer's nonce against its commitment, then signs this
    /// role's partial exactly once and returns the partial packet.
    ///
    /// # Errors
    /// Fails when the peer nonce does not open its commitment, the key does not match, or
    /// the journal is not in the nonces-exchanged phase.
    pub fn accept_nonce_sign(
        &mut self,
        packet: &[u8],
        secret_key: &Zeroizing<[u8; 32]>,
    ) -> Result<Vec<u8>, RunnerError> {
        let peer_public_nonce: [u8; 66] = self.peer_payload(packet, PacketKind::PublicNonce)?;
        let before = self.required_snapshot()?;
        let peer_commitment = before
            .peer_commitment()
            .ok_or(RunnerError::PeerCommitmentUnavailable)?;
        verify_nonce_commitment(
            self.session.context(),
            self.role.opposite().sdk(),
            *peer_commitment.bytes(),
            peer_public_nonce,
        )
        .map_err(|_| RunnerError::CryptographicValidation)?;
        let _ = self.journal.record_verified_peer_public_nonce(
            &self.identity,
            AdaptorPublicNonce::new(peer_public_nonce),
        )?;

        let ready = self.required_snapshot()?;
        let own_commitment = ready.own_commitment();
        let peer_commitment = ready
            .peer_commitment()
            .ok_or(RunnerError::PeerCommitmentUnavailable)?;
        let context = self.session.context();
        let role = self.role.sdk();
        let signed = self
            .journal
            .sign_and_persist_partial(&self.identity, |material| {
                let persisted = PersistedAdaptorSigningMaterial::new(
                    *material.identity().signing_domain(),
                    material.secret_nonce(),
                    *material.own_public_nonce().bytes(),
                    *own_commitment.bytes(),
                    *peer_commitment.bytes(),
                    *material.peer_public_nonce().bytes(),
                );
                sign_persisted_adaptor_partial(context, role, **secret_key, persisted)
                    .map(AdaptorPartialSignature::new)
                    .map_err(|_| ())
            })?;
        self.packet(PacketKind::PartialSignature, *signed.partial().bytes())
    }

    /// Re-emits the durable partial without loading a key.
    ///
    /// # Errors
    /// Fails until this role's partial is durable.
    pub fn replay_partial(&self) -> Result<Vec<u8>, RunnerError> {
        let partial = self
            .required_snapshot()?
            .own_partial()
            .ok_or(RunnerError::PartialUnavailable)?;
        self.packet(PacketKind::PartialSignature, *partial.bytes())
    }

    /// Verifies the peer's partial, aggregates and persists the adaptor
    /// presignature, and returns it as an aggregate packet.
    ///
    /// # Errors
    /// Fails when the peer partial does not verify against the transcript or a different
    /// presignature is already durable.
    pub fn accept_peer_partial(&mut self, packet: &[u8]) -> Result<Vec<u8>, RunnerError> {
        let peer_partial: [u8; 32] = self.peer_payload(packet, PacketKind::PartialSignature)?;
        let presignature = self.verify_and_record_peer_partial(peer_partial)?;
        protocol::aggregate_packet_bytes(PacketKind::Presignature, &self.session, presignature)
    }

    /// The durable aggregate presignature once both partials are verified.
    ///
    /// # Errors
    /// Fails until both partials are verified.
    pub fn presignature(&self) -> Result<[u8; 65], RunnerError> {
        Ok(*self
            .required_snapshot()?
            .presignature()
            .ok_or(RunnerError::PresignatureUnavailable)?
            .bytes())
    }

    pub(crate) fn verify_and_record_peer_partial(
        &mut self,
        peer_partial: [u8; 32],
    ) -> Result<[u8; 65], RunnerError> {
        let snapshot = self.required_snapshot()?;
        let own_public_nonce = snapshot
            .own_public_nonce()
            .ok_or(RunnerError::PublicNonceUnavailable)?;
        let peer_public_nonce = snapshot
            .peer_public_nonce()
            .ok_or(RunnerError::PublicNonceUnavailable)?;
        let own_partial = snapshot
            .own_partial()
            .ok_or(RunnerError::PartialUnavailable)?;
        let (maker_public_nonce, taker_public_nonce, maker_partial, taker_partial) = match self.role
        {
            Role::Maker => (
                *own_public_nonce.bytes(),
                *peer_public_nonce.bytes(),
                *own_partial.bytes(),
                peer_partial,
            ),
            Role::Taker => (
                *peer_public_nonce.bytes(),
                *own_public_nonce.bytes(),
                peer_partial,
                *own_partial.bytes(),
            ),
        };
        verify_adaptor_partial_signature(
            self.session.context(),
            self.role.opposite().sdk(),
            maker_public_nonce,
            taker_public_nonce,
            peer_partial,
        )
        .map_err(|_| RunnerError::CryptographicValidation)?;
        let presignature = aggregate_adaptor_presignature(
            self.session.context(),
            maker_public_nonce,
            taker_public_nonce,
            maker_partial,
            taker_partial,
        )
        .map_err(|_| RunnerError::CryptographicValidation)?;
        let _ = self.journal.record_verified_peer_partial(
            &self.identity,
            AdaptorPartialSignature::new(peer_partial),
        )?;
        let _ = self
            .journal
            .record_verified_presignature(&self.identity, AdaptorPresignature::new(presignature))?;
        Ok(presignature)
    }

    pub(crate) fn snapshot(&self) -> Result<Option<AdaptorSessionSnapshot>, RunnerError> {
        let Some(snapshot) = self.journal.load(self.identity.session_id())? else {
            return Ok(None);
        };
        if snapshot.identity() != &self.identity {
            return Err(RunnerError::JournalRoleOrSessionCrosswire);
        }
        Ok(Some(snapshot))
    }

    pub(crate) fn required_snapshot(&self) -> Result<AdaptorSessionSnapshot, RunnerError> {
        self.snapshot()?.ok_or(RunnerError::SessionUnavailable)
    }

    fn packet<const N: usize>(
        &self,
        kind: PacketKind,
        payload: [u8; N],
    ) -> Result<Vec<u8>, RunnerError> {
        protocol::packet_bytes(kind, self.role, &self.session, payload)
    }

    fn peer_payload<const N: usize>(
        &self,
        packet: &[u8],
        kind: PacketKind,
    ) -> Result<[u8; N], RunnerError> {
        protocol::read_peer_packet_bytes(packet, kind, self.role, &self.session)
    }
}
