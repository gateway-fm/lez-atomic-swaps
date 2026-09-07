//! `PoC` one-shot process boundary for crash-safe two-role adaptor signing.

#[cfg(not(unix))]
compile_error!("lez-adaptor-role-runner requires Unix file-permission semantics");

use std::{fmt, io, path::PathBuf};

use clap::{Parser, Subcommand};
use lez_adaptor_signature::{adapt_presignature, extract_adaptor_secret, verify_final_signature};
use subtle::ConstantTimeEq as _;
use thiserror::Error;

mod ceremony;
mod files;
mod protocol;

pub use ceremony::CeremonySeat;
pub use protocol::{PacketKind, Role, ValidatedSession};

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
    let session = ValidatedSession::load(&cli.session)?;
    let mut seat = CeremonySeat::open(&cli.journal, session, cli.role)?;
    match &cli.action {
        Action::Reserve {
            secret_key_file,
            output,
        } => files::write_public_new(
            output,
            &seat.reserve_with(|| files::read_secret_scalar(secret_key_file))?,
        ),
        Action::AcceptCommitment { input } => seat.accept_commitment(&files::read_public(input)?),
        Action::RevealNonce { output } => files::write_public_new(output, &seat.reveal_nonce()?),
        Action::AcceptNonceSign {
            input,
            secret_key_file,
            output,
        } => {
            let packet = files::read_public(input)?;
            let secret_key = files::read_secret_scalar(secret_key_file)?;
            files::write_public_new(output, &seat.accept_nonce_sign(&packet, &secret_key)?)
        }
        Action::ReplayPartial { output } => {
            files::write_public_new(output, &seat.replay_partial()?)
        }
        Action::AcceptPeerPartial { input, output } => files::write_public_new(
            output,
            &seat.accept_peer_partial(&files::read_public(input)?)?,
        ),
        Action::AdaptPresignature {
            input,
            adaptor_secret_file,
            output,
        } => {
            let presignature = read_durable_presignature(&seat, &files::read_public(input)?)?;
            let adaptor_secret = files::read_secret_scalar(adaptor_secret_file)?;
            let final_signature =
                adapt_presignature(seat.session().context(), presignature, adaptor_secret)
                    .map_err(|_| RunnerError::CryptographicValidation)?;
            files::write_public_new(
                output,
                &protocol::aggregate_packet_bytes(
                    PacketKind::FinalSignature,
                    seat.session(),
                    final_signature,
                )?,
            )
        }
        Action::ExtractAdaptorSecret {
            presignature,
            final_signature,
            output,
        } => {
            let presignature =
                read_durable_presignature(&seat, &files::read_public(presignature)?)?;
            let final_signature = protocol::read_aggregate_packet_bytes(
                &files::read_public(final_signature)?,
                PacketKind::FinalSignature,
                seat.session(),
            )?;
            let extracted =
                extract_adaptor_secret(seat.session().context(), presignature, final_signature)
                    .map_err(|_| RunnerError::CryptographicValidation)?;
            files::write_secret_scalar_new(output, &extracted)
        }
    }
}

/// Records one already authenticated peer partial and adapts the resulting
/// durable presignature without writing an intermediate packet or scalar copy.
///
/// This is the pair-neutral lifecycle bridge used after a chain-specific actor
/// has proved that the exact peer partial was finalized. The existing journal
/// must already contain the local partial and both nonce openings. The supplied
/// scalar remains in memory, is point-checked by the adaptor implementation,
/// and is zeroized on drop. Only the canonical aggregate final-signature packet
/// is written.
///
/// # Errors
///
/// Rejects a missing, role-crossed, incomplete, or conflicting journal; an
/// invalid peer partial or adaptor scalar; or an unsafe/pre-existing output.
pub fn accept_published_peer_partial_and_adapt(
    journal_path: &std::path::Path,
    session: &ValidatedSession,
    role: Role,
    peer_partial: [u8; 32],
    adaptor_secret: zeroize::Zeroizing<[u8; 32]>,
    output: &std::path::Path,
) -> Result<(), RunnerError> {
    let mut seat = CeremonySeat::open_existing(journal_path, session.clone(), role)?;
    let presignature = seat.verify_and_record_peer_partial(peer_partial)?;
    let final_signature = adapt_presignature(session.context(), presignature, adaptor_secret)
        .map_err(|_| RunnerError::CryptographicValidation)?;
    files::write_public_new(
        output,
        &protocol::aggregate_packet_bytes(PacketKind::FinalSignature, session, final_signature)?,
    )
}

#[must_use = "consume the verified scalar explicitly when reconstructing the shared spend key"]
pub struct VerifiedAdaptorSecret {
    bytes: zeroize::Zeroizing<[u8; 32]>,
}

impl VerifiedAdaptorSecret {
    /// Consumes this verifier boundary and returns the canonical secp256k1 big-endian bytes.
    ///
    /// The returned buffer remains zeroizing and must not be logged or persisted again.
    #[must_use]
    pub fn into_big_endian_bytes(self) -> zeroize::Zeroizing<[u8; 32]> {
        self.bytes
    }
}

impl fmt::Debug for VerifiedAdaptorSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VerifiedAdaptorSecret(REDACTED)")
    }
}

/// Extracts and verifies one adaptor scalar entirely in memory.
///
/// The role-fixed journal supplies the exact durable aggregate presignature.
/// The supplied final signature is verified against the validated session, and
/// the extracted scalar is point-checked by the adaptor implementation. No
/// scalar file is read or written.
///
/// # Errors
///
/// Rejects a missing, role-crossed, or incomplete journal and an invalid or
/// unrelated final signature. Errors and Debug output never contain secret bytes.
pub fn extract_verified_adaptor_secret(
    journal_path: &std::path::Path,
    session: &ValidatedSession,
    role: Role,
    final_signature: [u8; 64],
) -> Result<VerifiedAdaptorSecret, RunnerError> {
    let presignature =
        CeremonySeat::open_existing(journal_path, session.clone(), role)?.presignature()?;
    verify_final_signature(session.context(), final_signature)
        .map_err(|_| RunnerError::CryptographicValidation)?;
    let extracted = extract_adaptor_secret(session.context(), presignature, final_signature)
        .map_err(|_| RunnerError::CryptographicValidation)?;
    Ok(VerifiedAdaptorSecret { bytes: extracted })
}

/// Verifies an externally supplied adaptor scalar file against the
/// recomputed canonical secp256k1 big-endian scalar. Only the recomputed value is
/// returned, wrapped in an opaque zeroizing type.
///
/// # Errors
///
/// Rejects an unsafe scalar file, a missing or role/session-crossed journal, an
/// incomplete transcript, an invalid or unrelated final signature, or any scalar
/// mismatch. Errors and `Debug` output never contain secret bytes.
pub fn verify_extracted_adaptor_secret(
    journal_path: &std::path::Path,
    session: &ValidatedSession,
    role: Role,
    final_signature: [u8; 64],
    extracted_scalar_file: &std::path::Path,
) -> Result<VerifiedAdaptorSecret, RunnerError> {
    let extracted = extract_verified_adaptor_secret(journal_path, session, role, final_signature)?;
    let supplied = files::read_secret_scalar(extracted_scalar_file)?;
    if extracted.bytes[..].ct_eq(&supplied[..]).unwrap_u8() != 1 {
        return Err(RunnerError::CryptographicValidation);
    }
    Ok(extracted)
}

/// Reads and validates one canonical aggregate final-signature packet.
///
/// # Errors
///
/// Rejects an unsafe file, wrong packet kind, or any session/context drift.
pub fn read_final_signature_packet(
    path: &std::path::Path,
    session: &ValidatedSession,
) -> Result<[u8; 64], RunnerError> {
    read_final_signature_packet_bytes(&files::read_public(path)?, session)
}

/// Descriptor-native variant of
/// [`read_final_signature_packet`]. It preserves the same canonical packet,
/// kind, session, role-neutral sender, and context-binding checks without
/// reopening a path.
///
/// # Errors
///
/// Rejects malformed or noncanonical bytes, a wrong packet kind, or any
/// session/context drift.
pub fn read_final_signature_packet_bytes(
    bytes: &[u8],
    session: &ValidatedSession,
) -> Result<[u8; 64], RunnerError> {
    protocol::read_aggregate_packet_bytes(bytes, PacketKind::FinalSignature, session)
}

/// Records a final signature observed on chain next to the role's journal. The
/// function verifies the final signature, extracts and point-checks the adaptor
/// scalar in memory, immediately drops it, and writes only the public signature
/// packet. It never creates a plaintext scalar handoff.
///
/// # Errors
///
/// Rejects a missing, role-crossed, incomplete, or conflicting journal; an
/// invalid or unrelated final signature; or an unsafe/pre-existing output.
pub fn write_observed_final_signature_packet(
    journal_path: &std::path::Path,
    session: &ValidatedSession,
    role: Role,
    final_signature: [u8; 64],
    output: &std::path::Path,
) -> Result<(), RunnerError> {
    let presignature =
        CeremonySeat::open_existing(journal_path, session.clone(), role)?.presignature()?;
    verify_final_signature(session.context(), final_signature)
        .map_err(|_| RunnerError::CryptographicValidation)?;
    let extracted = extract_adaptor_secret(session.context(), presignature, final_signature)
        .map_err(|_| RunnerError::CryptographicValidation)?;
    drop(extracted);
    files::write_public_new(
        output,
        &protocol::aggregate_packet_bytes(PacketKind::FinalSignature, session, final_signature)?,
    )
}

fn read_durable_presignature(seat: &CeremonySeat, packet: &[u8]) -> Result<[u8; 65], RunnerError> {
    let supplied =
        protocol::read_aggregate_packet_bytes(packet, PacketKind::Presignature, seat.session())?;
    if supplied != seat.presignature()? {
        return Err(RunnerError::PublicPacketCrosswire);
    }
    Ok(supplied)
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
