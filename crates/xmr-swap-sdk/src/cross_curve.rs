//! Versioned secp256k1-to-Ed25519 DLEQ boundary.

use core::fmt;

use musig2::secp::{Point as MusigPoint, Scalar as MusigScalar};
use rand_chacha::ChaCha20Rng;
use rand_core::{CryptoRng, RngCore};
use sha2::{Digest as _, Sha256};
use sigma_fun::{
    HashTranscript,
    ed25519::curve25519_dalek::{
        edwards::CompressedEdwardsY, scalar::Scalar as EdScalar, traits::Identity as _,
    },
    ext::dl_secp256k1_ed25519_eq::{CrossCurveDLEQ, CrossCurveDLEQProof},
    secp256k1::fun::Point as SigmaSecpPoint,
};
use thiserror::Error;
use zeroize::Zeroizing;

/// Version of the public DLEQ envelope and transcript commitment.
pub const CROSS_CURVE_DLEQ_SCHEMA_V1: u8 = 1;

const TRANSCRIPT_COMMITMENT_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr/cross-curve-dleq-transcript/v1";
const MAX_PROOF_BYTES: usize = 128 * 1024;

// Public alternate generators used by the h4sh3d/COMIT construction. They are
// mathematical protocol parameters, not imported source code. The archived
// PoC documents the secp256k1 point as the Grin Pedersen generator and the
// Ed25519 point as Monero's historical RingCT alternate generator.
const SECP256K1_ALTERNATE_GENERATOR: [u8; 33] = [
    0x02, 0x50, 0x92, 0x9b, 0x74, 0xc1, 0xa0, 0x49, 0x54, 0xb7, 0x8b, 0x4b, 0x60, 0x35, 0xe9, 0x7a,
    0x5e, 0x07, 0x8a, 0x5a, 0x0f, 0x28, 0xec, 0x96, 0xd5, 0x47, 0xbf, 0xee, 0x9a, 0xce, 0x80, 0x3a,
    0xc0,
];
const ED25519_ALTERNATE_GENERATOR: [u8; 32] = [
    0x8b, 0x65, 0x59, 0x70, 0x15, 0x37, 0x99, 0xaf, 0x2a, 0xea, 0xdc, 0x9f, 0xf1, 0xad, 0xd0, 0xea,
    0x6c, 0x72, 0x51, 0xd5, 0x41, 0x54, 0xcf, 0xa9, 0x2c, 0x17, 0x3a, 0x0d, 0xd3, 0x9c, 0x1f, 0x94,
];

type DleqTranscript = HashTranscript<Sha256, ChaCha20Rng>;

/// A nonzero scalar with one canonical 252-bit little-endian representation on
/// both secp256k1 and the Ed25519 prime-order subgroup.
pub struct CrossCurveScalar(Zeroizing<[u8; 32]>);

impl CrossCurveScalar {
    /// Parses the Monero/Ed25519 little-endian representation.
    ///
    /// # Errors
    ///
    /// Rejects zero and every value whose upper four bits are set. Restricting
    /// the value to less than 2^252 makes the same integer canonical in both
    /// scalar fields without modular reduction.
    pub fn from_monero_little_endian(bytes: [u8; 32]) -> Result<Self, CrossCurveDleqError> {
        if bytes == [0; 32] || bytes[31] & 0xf0 != 0 {
            return Err(CrossCurveDleqError::InvalidScalar);
        }
        let scalar = EdScalar::from_canonical_bytes(bytes);
        if scalar.is_none() {
            return Err(CrossCurveDleqError::InvalidScalar);
        }
        Ok(Self(Zeroizing::new(bytes)))
    }

    /// Returns the equivalent big-endian scalar used by `musig2` adaptor APIs.
    #[must_use]
    pub fn adaptor_scalar_big_endian(&self) -> Zeroizing<[u8; 32]> {
        let mut bytes = Zeroizing::new(*self.0);
        bytes.reverse();
        bytes
    }

    /// Consumes this value and returns its Monero little-endian representation.
    #[must_use]
    pub fn into_monero_little_endian(self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(*self.0)
    }

    fn ed_scalar(&self) -> Result<EdScalar, CrossCurveDleqError> {
        EdScalar::from_canonical_bytes(*self.0).ok_or(CrossCurveDleqError::InvalidScalar)
    }
}

impl fmt::Debug for CrossCurveScalar {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CrossCurveScalar([REDACTED])")
    }
}

/// One versioned public cross-curve proof and its exact curve points.
#[derive(Clone, Eq, PartialEq)]
pub struct CrossCurveDleqProofV1 {
    secp256k1_public_key: [u8; 33],
    ed25519_public_key: [u8; 32],
    proof_bytes: Vec<u8>,
    transcript_commitment: [u8; 32],
}

impl CrossCurveDleqProofV1 {
    /// Creates and immediately verifies a proof for one canonical scalar.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid public parameters, serialization failure,
    /// an oversized proof, a curve-mapping mismatch, or failed self-verification.
    pub fn prove(
        scalar: &CrossCurveScalar,
        rng: &mut (impl CryptoRng + RngCore),
    ) -> Result<Self, CrossCurveDleqError> {
        let proof_system = proof_system()?;
        let (proof, (secp256k1_point, ed25519_point)) =
            proof_system.prove(&scalar.ed_scalar()?, rng);
        let secp256k1_public_key = secp256k1_point.to_bytes();
        let ed25519_public_key = ed25519_point.compress().to_bytes();

        let adaptor_scalar = scalar.adaptor_scalar_big_endian();
        let musig_scalar = MusigScalar::from_slice(adaptor_scalar.as_ref())
            .map_err(|_| CrossCurveDleqError::InvalidScalar)?;
        drop(adaptor_scalar);
        if musig_scalar.base_point_mul().serialize() != secp256k1_public_key {
            return Err(CrossCurveDleqError::AdaptorPointMismatch);
        }

        let proof_bytes =
            postcard::to_allocvec(&proof).map_err(|_| CrossCurveDleqError::ProofSerialization)?;
        if proof_bytes.len() > MAX_PROOF_BYTES {
            return Err(CrossCurveDleqError::ProofTooLarge);
        }
        let transcript_commitment =
            transcript_commitment(secp256k1_public_key, ed25519_public_key, &proof_bytes);
        let result = Self {
            secp256k1_public_key,
            ed25519_public_key,
            proof_bytes,
            transcript_commitment,
        };
        result.verify()?;
        Ok(result)
    }

    /// Verifies the exact public points, bounded proof encoding, and DLEQ.
    ///
    /// # Errors
    ///
    /// Rejects malformed points/proofs, trailing proof bytes, identity or
    /// torsion Ed25519 points, invalid DLEQ equations, and a changed envelope
    /// commitment.
    pub fn verify(&self) -> Result<(), CrossCurveDleqError> {
        if self.proof_bytes.is_empty() || self.proof_bytes.len() > MAX_PROOF_BYTES {
            return Err(CrossCurveDleqError::ProofTooLarge);
        }
        MusigPoint::from_slice(&self.secp256k1_public_key)
            .map_err(|_| CrossCurveDleqError::InvalidSecp256k1Point)?;
        let secp256k1_point = SigmaSecpPoint::from_bytes(self.secp256k1_public_key)
            .ok_or(CrossCurveDleqError::InvalidSecp256k1Point)?;
        let ed25519_point = CompressedEdwardsY(self.ed25519_public_key)
            .decompress()
            .ok_or(CrossCurveDleqError::InvalidEd25519Point)?;
        if ed25519_point == sigma_fun::ed25519::curve25519_dalek::edwards::EdwardsPoint::identity()
            || !ed25519_point.is_torsion_free()
        {
            return Err(CrossCurveDleqError::InvalidEd25519Point);
        }
        let proof: CrossCurveDLEQProof = postcard::from_bytes(&self.proof_bytes)
            .map_err(|_| CrossCurveDleqError::ProofSerialization)?;
        if !proof_system()?.verify(&proof, (secp256k1_point, ed25519_point)) {
            return Err(CrossCurveDleqError::InvalidProof);
        }
        let expected_commitment = transcript_commitment(
            self.secp256k1_public_key,
            self.ed25519_public_key,
            &self.proof_bytes,
        );
        if expected_commitment != self.transcript_commitment {
            return Err(CrossCurveDleqError::TranscriptCommitmentMismatch);
        }
        Ok(())
    }

    /// Compressed secp256k1 adaptor point accepted by the LEZ `MuSig2` path.
    #[must_use]
    pub const fn secp256k1_public_key(&self) -> [u8; 33] {
        self.secp256k1_public_key
    }

    /// Compressed Ed25519 point for the matching Monero spend-key share.
    #[must_use]
    pub const fn ed25519_public_key(&self) -> [u8; 32] {
        self.ed25519_public_key
    }

    /// Exact upstream proof serialization bounded by 128 KiB.
    #[must_use]
    pub fn proof_bytes(&self) -> &[u8] {
        &self.proof_bytes
    }

    /// Domain-separated commitment stored in agreement and LEZ metadata.
    #[must_use]
    pub const fn transcript_commitment(&self) -> [u8; 32] {
        self.transcript_commitment
    }
}

impl fmt::Debug for CrossCurveDleqProofV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CrossCurveDleqProofV1")
            .field("secp256k1_public_key", &self.secp256k1_public_key)
            .field("ed25519_public_key", &self.ed25519_public_key)
            .field("proof_bytes_len", &self.proof_bytes.len())
            .field("transcript_commitment", &self.transcript_commitment)
            .finish()
    }
}

/// Fail-closed errors exposed by the M4 cross-curve boundary.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CrossCurveDleqError {
    /// Scalar is zero, noncanonical, or wider than 252 bits.
    #[error("cross-curve scalar is not a nonzero canonical 252-bit value")]
    InvalidScalar,
    /// Fixed public alternate generators were invalid.
    #[error("cross-curve public parameters are invalid")]
    InvalidPublicParameters,
    /// secp256k1 public point is malformed.
    #[error("cross-curve secp256k1 point is invalid")]
    InvalidSecp256k1Point,
    /// Ed25519 public point is malformed, identity, or outside the prime subgroup.
    #[error("cross-curve Ed25519 point is invalid")]
    InvalidEd25519Point,
    /// Proof encoding could not be serialized or decoded canonically.
    #[error("cross-curve proof serialization is invalid")]
    ProofSerialization,
    /// Proof encoding is empty or exceeds the fixed bound.
    #[error("cross-curve proof exceeds its size contract")]
    ProofTooLarge,
    /// Upstream verifier rejected the proof relation.
    #[error("cross-curve proof verification failed")]
    InvalidProof,
    /// DLEQ secp256k1 point does not match the `MuSig2` adaptor scalar mapping.
    #[error("cross-curve secp256k1 point does not match the adaptor scalar")]
    AdaptorPointMismatch,
    /// Public proof bytes no longer match their agreement/metadata commitment.
    #[error("cross-curve transcript commitment mismatch")]
    TranscriptCommitmentMismatch,
}

fn proof_system() -> Result<CrossCurveDLEQ<DleqTranscript>, CrossCurveDleqError> {
    let secp256k1_generator = SigmaSecpPoint::from_bytes(SECP256K1_ALTERNATE_GENERATOR)
        .ok_or(CrossCurveDleqError::InvalidPublicParameters)?;
    let ed25519_generator = CompressedEdwardsY(ED25519_ALTERNATE_GENERATOR)
        .decompress()
        .ok_or(CrossCurveDleqError::InvalidPublicParameters)?;
    if ed25519_generator == sigma_fun::ed25519::curve25519_dalek::edwards::EdwardsPoint::identity()
        || !ed25519_generator.is_torsion_free()
    {
        return Err(CrossCurveDleqError::InvalidPublicParameters);
    }
    Ok(CrossCurveDLEQ::new(secp256k1_generator, ed25519_generator))
}

fn transcript_commitment(
    secp256k1_public_key: [u8; 33],
    ed25519_public_key: [u8; 32],
    proof_bytes: &[u8],
) -> [u8; 32] {
    let proof_length = u32::try_from(proof_bytes.len())
        .expect("the proof size contract is strictly smaller than u32::MAX");
    Sha256::new()
        .chain_update(TRANSCRIPT_COMMITMENT_DOMAIN)
        .chain_update([CROSS_CURVE_DLEQ_SCHEMA_V1])
        .chain_update(secp256k1_public_key)
        .chain_update(ed25519_public_key)
        .chain_update(proof_length.to_le_bytes())
        .chain_update(proof_bytes)
        .finalize()
        .into()
}
