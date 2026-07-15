//! `PoC` one-shot process boundary for crash-safe two-role adaptor signing.

#[cfg(not(unix))]
compile_error!("lez-adaptor-role-runner requires Unix file-permission semantics");

use std::{fmt, io, path::PathBuf};

use clap::{Parser, Subcommand};
use lez_btc_swap_sdk::{
    FreshAdaptorNonce, PersistedAdaptorSigningMaterial, adapt_presignature,
    aggregate_adaptor_presignature, extract_adaptor_secret, sign_persisted_adaptor_partial,
    verify_adaptor_partial_signature, verify_nonce_commitment,
};
use lez_swap_store::{
    AdaptorNonceCommitment, AdaptorPartialSignature, AdaptorPresignature, AdaptorPublicNonce,
    AdaptorSessionIdentity, AdaptorSessionReservation, AdaptorSessionSnapshot, SecretNonceBytes,
    SqliteAdaptorSessionJournal,
};
use thiserror::Error;

mod files;
mod protocol;

pub use protocol::Role;
use protocol::{PacketKind, Session};

/// One phase performed by one fresh role process.
#[derive(Clone, Debug, Subcommand)]
pub enum Action {
    /// Reserve a fresh secret nonce durably, then emit its public commitment.
    Reserve {
        /// Owner-private lowercase-hex signing key file.
        #[arg(long, value_name = "PRIVATE_KEY_FILE")]
        secret_key_file: PathBuf,
        /// New canonical public commitment packet.
        #[arg(long, value_name = "NEW_PUBLIC_JSON")]
        output: PathBuf,
    },
    /// Persist the peer's exact commitment packet.
    AcceptCommitment {
        /// Canonical peer commitment packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        input: PathBuf,
    },
    /// Emit the local public nonce after peer commitment persistence.
    RevealNonce {
        /// New canonical public nonce packet.
        #[arg(long, value_name = "NEW_PUBLIC_JSON")]
        output: PathBuf,
    },
    /// Verify/persist the peer nonce, consume the secret nonce once, and emit a partial.
    AcceptNonceSign {
        /// Canonical peer public-nonce packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        input: PathBuf,
        /// Owner-private lowercase-hex signing key file.
        #[arg(long, value_name = "PRIVATE_KEY_FILE")]
        secret_key_file: PathBuf,
        /// New canonical public partial-signature packet.
        #[arg(long, value_name = "NEW_PUBLIC_JSON")]
        output: PathBuf,
    },
    /// Emit the exact durable partial after restart without loading a signing key.
    ReplayPartial {
        /// New canonical public partial-signature packet.
        #[arg(long, value_name = "NEW_PUBLIC_JSON")]
        output: PathBuf,
    },
    /// Verify/persist the peer partial and emit the verified aggregate presignature.
    AcceptPeerPartial {
        /// Canonical peer partial-signature packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        input: PathBuf,
        /// New role-neutral canonical presignature packet.
        #[arg(long, value_name = "NEW_PUBLIC_JSON")]
        output: PathBuf,
    },
    /// Adapt the exact durable aggregate presignature with the committed scalar.
    AdaptPresignature {
        /// Canonical role-neutral aggregate presignature packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        input: PathBuf,
        /// Owner-private lowercase-hex adaptor scalar file.
        #[arg(long, value_name = "PRIVATE_SCALAR_FILE")]
        adaptor_secret_file: PathBuf,
        /// New role-neutral canonical final-signature packet.
        #[arg(long, value_name = "NEW_PUBLIC_JSON")]
        output: PathBuf,
    },
    /// Extract the committed scalar from an observed exact final signature.
    ExtractAdaptorSecret {
        /// Canonical role-neutral aggregate presignature packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        presignature: PathBuf,
        /// Canonical role-neutral final-signature packet.
        #[arg(long, value_name = "PUBLIC_JSON")]
        final_signature: PathBuf,
        /// New owner-private lowercase-hex recovered scalar file.
        #[arg(long, value_name = "NEW_PRIVATE_SCALAR_FILE")]
        output: PathBuf,
    },
}

/// Arguments for one role-fixed process invocation.
#[derive(Clone, Parser)]
#[command(about = "PoC one-shot durable MuSig2 adaptor role runner")]
pub struct Cli {
    /// Fixed role for this process and owner-local journal.
    #[arg(value_enum)]
    role: Role,
    /// Caller-selected owner-private `SQLite` journal path.
    #[arg(long, value_name = "PRIVATE_SQLITE")]
    journal: PathBuf,
    /// Bounded public session JSON shared by both roles.
    #[arg(long, value_name = "SESSION_JSON")]
    session: PathBuf,
    /// Exactly one monotonic signing phase.
    #[command(subcommand)]
    action: Action,
}

impl fmt::Debug for Cli {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Cli")
            .field("role", &self.role)
            .field("journal", &self.journal)
            .field("session", &self.session)
            .field("action", &self.action)
            .finish()
    }
}

/// Executes one monotonic phase and exits without printing secret material.
///
/// # Errors
///
/// Returns a fail-closed error for unsafe files, malformed/cross-wired public
/// packets, invalid cryptographic material, or a journal transition failure.
pub fn execute(cli: &Cli) -> Result<(), RunnerError> {
    let session = Session::load(&cli.session)?;
    let identity = session.identity(cli.role);
    let mut journal = SqliteAdaptorSessionJournal::open(&cli.journal)?;
    match &cli.action {
        Action::Reserve {
            secret_key_file,
            output,
        } => reserve(
            &mut journal,
            &identity,
            &session,
            cli.role,
            secret_key_file,
            output,
        ),
        Action::AcceptCommitment { input } => {
            ensure_identity(&journal, &identity)?;
            let peer_commitment =
                protocol::read_peer_packet(input, PacketKind::NonceCommitment, cli.role, &session)?;
            let _ = journal
                .record_peer_commitment(&identity, AdaptorNonceCommitment::new(peer_commitment))?;
            Ok(())
        }
        Action::RevealNonce { output } => {
            ensure_identity(&journal, &identity)?;
            let public_nonce = journal.reveal_own_public_nonce(&identity)?;
            protocol::write_packet(
                output,
                PacketKind::PublicNonce,
                cli.role,
                &session,
                *public_nonce.bytes(),
            )
        }
        Action::AcceptNonceSign {
            input,
            secret_key_file,
            output,
        } => accept_nonce_and_sign(
            &mut journal,
            &identity,
            &session,
            cli.role,
            input,
            secret_key_file,
            output,
        ),
        Action::ReplayPartial { output } => {
            let snapshot = required_snapshot(&journal, &identity)?;
            let partial = snapshot
                .own_partial()
                .ok_or(RunnerError::PartialUnavailable)?;
            protocol::write_packet(
                output,
                PacketKind::PartialSignature,
                cli.role,
                &session,
                *partial.bytes(),
            )
        }
        Action::AcceptPeerPartial { input, output } => {
            accept_peer_partial(&mut journal, &identity, &session, cli.role, input, output)
        }
        Action::AdaptPresignature {
            input,
            adaptor_secret_file,
            output,
        } => adapt_durable_presignature(
            &journal,
            &identity,
            &session,
            input,
            adaptor_secret_file,
            output,
        ),
        Action::ExtractAdaptorSecret {
            presignature,
            final_signature,
            output,
        } => extract_durable_adaptor_secret(
            &journal,
            &identity,
            &session,
            presignature,
            final_signature,
            output,
        ),
    }
}

fn reserve(
    journal: &mut SqliteAdaptorSessionJournal,
    identity: &AdaptorSessionIdentity,
    session: &Session,
    role: Role,
    secret_key_file: &std::path::Path,
    output: &std::path::Path,
) -> Result<(), RunnerError> {
    if let Some(snapshot) = journal.load(identity.session_id())? {
        validate_snapshot_identity(&snapshot, identity)?;
        return protocol::write_packet(
            output,
            PacketKind::NonceCommitment,
            role,
            session,
            *snapshot.own_commitment().bytes(),
        );
    }
    let secret_key = files::read_secret_scalar(secret_key_file)?;
    let fresh = FreshAdaptorNonce::generate(session.context(), role.sdk(), *secret_key)
        .map_err(|_| RunnerError::CryptographicValidation)?;
    if fresh.context_binding() != session.context_binding() {
        return Err(RunnerError::CryptographicValidation);
    }
    let commit = journal.reserve(AdaptorSessionReservation::new(
        identity.clone(),
        SecretNonceBytes::new(*fresh.secret_nonce()),
        AdaptorPublicNonce::new(fresh.public_nonce()),
        AdaptorNonceCommitment::new(fresh.commitment()),
    ))?;
    protocol::write_packet(
        output,
        PacketKind::NonceCommitment,
        role,
        session,
        *commit.own_commitment().bytes(),
    )
}

fn accept_nonce_and_sign(
    journal: &mut SqliteAdaptorSessionJournal,
    identity: &AdaptorSessionIdentity,
    session: &Session,
    role: Role,
    input: &std::path::Path,
    secret_key_file: &std::path::Path,
    output: &std::path::Path,
) -> Result<(), RunnerError> {
    let peer_public_nonce =
        protocol::read_peer_packet(input, PacketKind::PublicNonce, role, session)?;
    let before = required_snapshot(journal, identity)?;
    let peer_commitment = before
        .peer_commitment()
        .ok_or(RunnerError::PeerCommitmentUnavailable)?;
    verify_nonce_commitment(
        session.context(),
        role.opposite().sdk(),
        *peer_commitment.bytes(),
        peer_public_nonce,
    )
    .map_err(|_| RunnerError::CryptographicValidation)?;
    let _ = journal
        .record_verified_peer_public_nonce(identity, AdaptorPublicNonce::new(peer_public_nonce))?;

    let ready = required_snapshot(journal, identity)?;
    let own_commitment = ready.own_commitment();
    let peer_commitment = ready
        .peer_commitment()
        .ok_or(RunnerError::PeerCommitmentUnavailable)?;
    let secret_key = files::read_secret_scalar(secret_key_file)?;
    let signed = journal.sign_and_persist_partial(identity, |material| {
        let persisted = PersistedAdaptorSigningMaterial::new(
            *material.identity().signing_domain(),
            material.secret_nonce(),
            *material.own_public_nonce().bytes(),
            *own_commitment.bytes(),
            *peer_commitment.bytes(),
            *material.peer_public_nonce().bytes(),
        );
        sign_persisted_adaptor_partial(session.context(), role.sdk(), *secret_key, persisted)
            .map(AdaptorPartialSignature::new)
            .map_err(|_| ())
    })?;
    protocol::write_packet(
        output,
        PacketKind::PartialSignature,
        role,
        session,
        *signed.partial().bytes(),
    )
}

fn accept_peer_partial(
    journal: &mut SqliteAdaptorSessionJournal,
    identity: &AdaptorSessionIdentity,
    session: &Session,
    role: Role,
    input: &std::path::Path,
    output: &std::path::Path,
) -> Result<(), RunnerError> {
    let peer_partial =
        protocol::read_peer_packet(input, PacketKind::PartialSignature, role, session)?;
    let snapshot = required_snapshot(journal, identity)?;
    let own_public_nonce = snapshot
        .own_public_nonce()
        .ok_or(RunnerError::PublicNonceUnavailable)?;
    let peer_public_nonce = snapshot
        .peer_public_nonce()
        .ok_or(RunnerError::PublicNonceUnavailable)?;
    let own_partial = snapshot
        .own_partial()
        .ok_or(RunnerError::PartialUnavailable)?;
    let (maker_public_nonce, taker_public_nonce, maker_partial, taker_partial) = match role {
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
        session.context(),
        role.opposite().sdk(),
        maker_public_nonce,
        taker_public_nonce,
        peer_partial,
    )
    .map_err(|_| RunnerError::CryptographicValidation)?;
    let presignature = aggregate_adaptor_presignature(
        session.context(),
        maker_public_nonce,
        taker_public_nonce,
        maker_partial,
        taker_partial,
    )
    .map_err(|_| RunnerError::CryptographicValidation)?;
    let _ = journal
        .record_verified_peer_partial(identity, AdaptorPartialSignature::new(peer_partial))?;
    let _ =
        journal.record_verified_presignature(identity, AdaptorPresignature::new(presignature))?;
    protocol::write_aggregate_packet(output, PacketKind::Presignature, session, presignature)
}

fn adapt_durable_presignature(
    journal: &SqliteAdaptorSessionJournal,
    identity: &AdaptorSessionIdentity,
    session: &Session,
    input: &std::path::Path,
    adaptor_secret_file: &std::path::Path,
    output: &std::path::Path,
) -> Result<(), RunnerError> {
    let presignature = read_durable_presignature(journal, identity, session, input)?;
    let adaptor_secret = files::read_secret_scalar(adaptor_secret_file)?;
    let final_signature = adapt_presignature(session.context(), presignature, adaptor_secret)
        .map_err(|_| RunnerError::CryptographicValidation)?;
    protocol::write_aggregate_packet(output, PacketKind::FinalSignature, session, final_signature)
}

fn extract_durable_adaptor_secret(
    journal: &SqliteAdaptorSessionJournal,
    identity: &AdaptorSessionIdentity,
    session: &Session,
    presignature_path: &std::path::Path,
    final_signature_path: &std::path::Path,
    output: &std::path::Path,
) -> Result<(), RunnerError> {
    let presignature = read_durable_presignature(journal, identity, session, presignature_path)?;
    let final_signature =
        protocol::read_aggregate_packet(final_signature_path, PacketKind::FinalSignature, session)?;
    let extracted = extract_adaptor_secret(session.context(), presignature, final_signature)
        .map_err(|_| RunnerError::CryptographicValidation)?;
    files::write_secret_scalar_new(output, &extracted)
}

fn read_durable_presignature(
    journal: &SqliteAdaptorSessionJournal,
    identity: &AdaptorSessionIdentity,
    session: &Session,
    input: &std::path::Path,
) -> Result<[u8; 65], RunnerError> {
    let supplied = protocol::read_aggregate_packet(input, PacketKind::Presignature, session)?;
    let durable = required_snapshot(journal, identity)?
        .presignature()
        .ok_or(RunnerError::PresignatureUnavailable)?;
    if supplied != *durable.bytes() {
        return Err(RunnerError::PublicPacketCrosswire);
    }
    Ok(supplied)
}

fn ensure_identity(
    journal: &SqliteAdaptorSessionJournal,
    identity: &AdaptorSessionIdentity,
) -> Result<(), RunnerError> {
    let _ = required_snapshot(journal, identity)?;
    Ok(())
}

fn required_snapshot(
    journal: &SqliteAdaptorSessionJournal,
    identity: &AdaptorSessionIdentity,
) -> Result<AdaptorSessionSnapshot, RunnerError> {
    let snapshot = journal
        .load(identity.session_id())?
        .ok_or(RunnerError::SessionUnavailable)?;
    validate_snapshot_identity(&snapshot, identity)?;
    Ok(snapshot)
}

fn validate_snapshot_identity(
    snapshot: &AdaptorSessionSnapshot,
    identity: &AdaptorSessionIdentity,
) -> Result<(), RunnerError> {
    if snapshot.identity() == identity {
        Ok(())
    } else {
        Err(RunnerError::JournalRoleOrSessionCrosswire)
    }
}

/// Fail-closed runner error with no secret byte payloads.
#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("input file I/O failed")]
    InputIo(#[source] io::Error),
    #[error("output file I/O failed")]
    OutputIo(#[source] io::Error),
    #[error("input must be a stable regular file")]
    UnsafeInputFile,
    #[error("secret scalar file must be owner-private and single-linked")]
    UnsafeSecretScalarFile,
    #[error("input file exceeds its protocol bound")]
    InputTooLarge,
    #[error("output file exceeds its protocol bound")]
    OutputTooLarge,
    #[error("secret scalar file is not exact lowercase 32-byte hex")]
    InvalidSecretScalarFile,
    #[error("session configuration is invalid")]
    InvalidSessionConfig,
    #[error("session configuration is not canonical JSON")]
    NoncanonicalSessionConfig,
    #[error("public packet is invalid")]
    InvalidPublicPacket,
    #[error("public packet is not canonical JSON")]
    NoncanonicalPublicPacket,
    #[error("public packet serialization failed")]
    PublicPacketSerialization,
    #[error("public packet contains noncanonical fixed-width hex")]
    InvalidCanonicalHex,
    #[error("public packet is for another session, context, or phase")]
    PublicPacketCrosswire,
    #[error("public packet sender role is not the expected peer")]
    PublicPacketRoleCrosswire,
    #[error("journal belongs to another role or session")]
    JournalRoleOrSessionCrosswire,
    #[error("session has not been reserved")]
    SessionUnavailable,
    #[error("peer commitment is unavailable")]
    PeerCommitmentUnavailable,
    #[error("durable partial is unavailable")]
    PartialUnavailable,
    #[error("durable public nonce is unavailable")]
    PublicNonceUnavailable,
    #[error("durable aggregate presignature is unavailable")]
    PresignatureUnavailable,
    #[error("secret scalar serialization failed")]
    SecretScalarSerialization,
    #[error("cryptographic transcript validation failed")]
    CryptographicValidation,
    #[error(transparent)]
    Journal(#[from] lez_swap_store::AdaptorSessionJournalError),
}
