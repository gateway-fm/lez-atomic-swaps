//! Shared Monero address and reconstructed spend-key boundary.

use core::fmt;

use monero::{Address, Network, util::key::PublicKey};
use sigma_fun::ed25519::curve25519_dalek::{
    constants::ED25519_BASEPOINT_TABLE,
    edwards::{CompressedEdwardsY, EdwardsPoint},
    scalar::Scalar as EdScalar,
    traits::Identity as _,
};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::{CrossCurveDleqError, CrossCurveDleqProofV1, CrossCurveScalar};

/// Address-domain selector supported by the progressive M4 implementation.
///
/// Official Monero Regtest uses mainnet-format wallet addresses. Keeping that
/// fact explicit prevents a local fakechain address from being mislabeled as a
/// public Stagenet address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoneroAddressNetworkV1 {
    /// Offline official `monerod --regtest`; encoded with mainnet address bytes.
    Regtest,
    /// Public or self-hosted Monero Stagenet.
    Stagenet,
}

impl MoneroAddressNetworkV1 {
    const fn monero_network(self) -> Network {
        match self {
            Self::Regtest => Network::Mainnet,
            Self::Stagenet => Network::Stagenet,
        }
    }
}

/// Canonical private Monero view key shared by the two swap roles.
///
/// A view key reveals transaction visibility but cannot spend the shared
/// output. Its bytes remain confidential and are redacted from `Debug`.
pub struct MoneroPrivateViewKey {
    bytes: Zeroizing<[u8; 32]>,
    public_key: [u8; 32],
}

impl MoneroPrivateViewKey {
    /// Parses a nonzero canonical Ed25519 scalar in Monero little-endian form.
    ///
    /// # Errors
    ///
    /// Rejects zero and noncanonical scalar encodings.
    pub fn from_monero_little_endian(bytes: [u8; 32]) -> Result<Self, MoneroSharedSpendError> {
        let scalar =
            EdScalar::from_canonical_bytes(bytes).ok_or(MoneroSharedSpendError::InvalidViewKey)?;
        if bytes == [0; 32] {
            return Err(MoneroSharedSpendError::InvalidViewKey);
        }
        let public_key = (&scalar * &ED25519_BASEPOINT_TABLE).compress().to_bytes();
        Ok(Self {
            bytes: Zeroizing::new(bytes),
            public_key,
        })
    }

    /// Returns the corresponding standard-basepoint Ed25519 public view key.
    #[must_use]
    pub const fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    /// Consumes this value for an owner-private Monero wallet-RPC handoff.
    #[must_use]
    pub fn into_monero_little_endian(self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(*self.bytes)
    }
}

impl fmt::Debug for MoneroPrivateViewKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MoneroPrivateViewKey([REDACTED])")
    }
}

/// Exact public Monero address agreed before either on-chain lock.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MoneroSharedAddressV1 {
    network: MoneroAddressNetworkV1,
    public_spend_key: [u8; 32],
    public_view_key: [u8; 32],
    address: String,
}

impl MoneroSharedAddressV1 {
    /// Derives the shared address from both actors' DLEQ-bound public shares
    /// and the shared view key.
    ///
    /// # Errors
    ///
    /// Rejects either invalid DLEQ proof or an aggregate spend point equal to
    /// the Ed25519 identity.
    pub fn derive(
        network: MoneroAddressNetworkV1,
        maker_proof: &CrossCurveDleqProofV1,
        taker_proof: &CrossCurveDleqProofV1,
        view_key: &MoneroPrivateViewKey,
    ) -> Result<Self, MoneroSharedSpendError> {
        Self::derive_from_public_view_key(network, maker_proof, taker_proof, view_key.public_key())
    }

    /// Derives the shared address from both verified spend shares and an exact
    /// public view key already exchanged in a countersigned agreement.
    ///
    /// This boundary deliberately needs no private view material, allowing an
    /// untrusted agreement to be checked without exposing wallet authority.
    ///
    /// # Errors
    ///
    /// Rejects either invalid DLEQ proof, a malformed/non-prime-order public
    /// view key, or an unsafe aggregate spend point.
    pub fn derive_from_public_view_key(
        network: MoneroAddressNetworkV1,
        maker_proof: &CrossCurveDleqProofV1,
        taker_proof: &CrossCurveDleqProofV1,
        public_view_key: [u8; 32],
    ) -> Result<Self, MoneroSharedSpendError> {
        maker_proof.verify()?;
        taker_proof.verify()?;
        let maker = validated_public_point(maker_proof.ed25519_public_key())?;
        let taker = validated_public_point(taker_proof.ed25519_public_key())?;
        let shared = maker + taker;
        if shared == EdwardsPoint::identity() || !shared.is_torsion_free() {
            return Err(MoneroSharedSpendError::InvalidSharedSpendKey);
        }
        let public_spend_key = shared.compress().to_bytes();
        validated_public_view_point(public_view_key)?;
        let public_spend = PublicKey::from_slice(&public_spend_key)
            .map_err(|_| MoneroSharedSpendError::AddressEncoding)?;
        let public_view = PublicKey::from_slice(&public_view_key)
            .map_err(|_| MoneroSharedSpendError::AddressEncoding)?;
        let address =
            Address::standard(network.monero_network(), public_spend, public_view).to_string();
        Ok(Self {
            network,
            public_spend_key,
            public_view_key,
            address,
        })
    }

    /// Explicit local/public address domain.
    #[must_use]
    pub const fn network(&self) -> MoneroAddressNetworkV1 {
        self.network
    }

    /// Aggregate Ed25519 public spend key funded by the Maker.
    #[must_use]
    pub const fn public_spend_key(&self) -> [u8; 32] {
        self.public_spend_key
    }

    /// Public view key paired with the aggregate spend key.
    #[must_use]
    pub const fn public_view_key(&self) -> [u8; 32] {
        self.public_view_key
    }

    /// Canonical Monero address string accepted by official wallet RPC.
    ///
    /// Regtest intentionally returns a mainnet-format address because the
    /// official functional-test wallet has no separate Regtest address domain.
    #[must_use]
    pub fn address_string(&self) -> String {
        self.address.clone()
    }
}

/// Reconstructed private spend authority owned by the role that observes the
/// peer's revealing LEZ signature.
pub struct ReconstructedMoneroSpendKey {
    bytes: Zeroizing<[u8; 32]>,
    public_key: [u8; 32],
}

impl ReconstructedMoneroSpendKey {
    /// Reconstructs and point-checks the private key for the already agreed
    /// shared address.
    ///
    /// The extracted adaptor scalar is secp256k1 big-endian. It is reversed
    /// exactly once into the Monero little-endian representation, checked
    /// against both public points of the revealing role's proof, added modulo
    /// the Ed25519 scalar order to the observer's retained share, and finally
    /// checked against the funded public spend key. This is symmetric: the
    /// Taker uses the Maker proof after claim, while the Maker uses the Taker
    /// proof after a signed timeout refund.
    ///
    /// # Errors
    ///
    /// Rejects a noncanonical or unrelated extracted scalar, an invalid DLEQ
    /// proof, a zero result, or any reconstructed key that does not open the
    /// exact agreed address.
    pub fn reconstruct(
        expected_address: &MoneroSharedAddressV1,
        revealed_proof: &CrossCurveDleqProofV1,
        retained_share: CrossCurveScalar,
        mut extracted_adaptor_scalar: Zeroizing<[u8; 32]>,
    ) -> Result<Self, MoneroSharedSpendError> {
        extracted_adaptor_scalar.reverse();
        let revealed_share =
            CrossCurveScalar::from_monero_little_endian(*extracted_adaptor_scalar)?;
        drop(extracted_adaptor_scalar);
        revealed_proof.verify_scalar(&revealed_share)?;

        let revealed_bytes = revealed_share.into_monero_little_endian();
        let retained_bytes = retained_share.into_monero_little_endian();
        let revealed_scalar = EdScalar::from_canonical_bytes(*revealed_bytes)
            .ok_or(MoneroSharedSpendError::InvalidReconstructedSpendKey)?;
        let retained_scalar = EdScalar::from_canonical_bytes(*retained_bytes)
            .ok_or(MoneroSharedSpendError::InvalidReconstructedSpendKey)?;
        drop(revealed_bytes);
        drop(retained_bytes);

        let reconstructed = revealed_scalar + retained_scalar;
        if reconstructed == EdScalar::zero() {
            return Err(MoneroSharedSpendError::InvalidReconstructedSpendKey);
        }
        let public_spend = (&reconstructed * &ED25519_BASEPOINT_TABLE)
            .compress()
            .to_bytes();
        if public_spend != expected_address.public_spend_key {
            return Err(MoneroSharedSpendError::ReconstructedSpendKeyMismatch);
        }
        Ok(Self {
            bytes: Zeroizing::new(reconstructed.to_bytes()),
            public_key: public_spend,
        })
    }

    /// Returns the public spend point for evidence without exposing key bytes.
    #[must_use]
    pub const fn public_key(&self) -> [u8; 32] {
        self.public_key
    }

    /// Consumes this key for an owner-private Monero wallet-RPC handoff.
    #[must_use]
    pub fn into_monero_little_endian(self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(*self.bytes)
    }
}

impl fmt::Debug for ReconstructedMoneroSpendKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReconstructedMoneroSpendKey([REDACTED])")
    }
}

/// Fail-closed errors for the shared-address/reconstructed-spend equation.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MoneroSharedSpendError {
    /// Cross-curve proof or scalar validation failed.
    #[error("cross-curve DLEQ or scalar validation failed")]
    CrossCurve(#[from] CrossCurveDleqError),
    /// Shared private view key is zero or noncanonical.
    #[error("Monero private view key is invalid")]
    InvalidViewKey,
    /// A public spend share is malformed, the identity, or not prime-order.
    #[error("Monero public spend-key share is invalid")]
    InvalidPublicSpendShare,
    /// Aggregate public spend key is the identity or otherwise unsafe.
    #[error("shared Monero public spend key is invalid")]
    InvalidSharedSpendKey,
    /// Public view key is malformed, the identity, or outside the prime-order subgroup.
    #[error("shared Monero public view key is invalid")]
    InvalidPublicViewKey,
    /// Canonical public keys could not be encoded as a Monero address.
    #[error("shared Monero address encoding failed")]
    AddressEncoding,
    /// Reconstructed private spend key is zero or noncanonical.
    #[error("reconstructed Monero private spend key is invalid")]
    InvalidReconstructedSpendKey,
    /// Reconstructed private key does not open the agreed funded address.
    #[error("reconstructed Monero spend key does not match the agreed address")]
    ReconstructedSpendKeyMismatch,
}

fn validated_public_point(bytes: [u8; 32]) -> Result<EdwardsPoint, MoneroSharedSpendError> {
    let point = CompressedEdwardsY(bytes)
        .decompress()
        .ok_or(MoneroSharedSpendError::InvalidPublicSpendShare)?;
    if point == EdwardsPoint::identity() || !point.is_torsion_free() {
        return Err(MoneroSharedSpendError::InvalidPublicSpendShare);
    }
    Ok(point)
}

fn validated_public_view_point(bytes: [u8; 32]) -> Result<EdwardsPoint, MoneroSharedSpendError> {
    let point = CompressedEdwardsY(bytes)
        .decompress()
        .ok_or(MoneroSharedSpendError::InvalidPublicViewKey)?;
    if point == EdwardsPoint::identity() || !point.is_torsion_free() {
        return Err(MoneroSharedSpendError::InvalidPublicViewKey);
    }
    Ok(point)
}

#[cfg(test)]
mod tests {
    use rand_chacha::{ChaCha20Rng, rand_core::SeedableRng as _};

    use super::*;

    fn scalar(value: u8) -> CrossCurveScalar {
        let mut bytes = [0_u8; 32];
        bytes[0] = value;
        CrossCurveScalar::from_monero_little_endian(bytes).expect("fixture scalar")
    }

    fn view_key(value: u8) -> MoneroPrivateViewKey {
        let mut bytes = [0_u8; 32];
        bytes[0] = value;
        MoneroPrivateViewKey::from_monero_little_endian(bytes).expect("fixture view key")
    }

    #[test]
    fn claim_and_signed_refund_reveal_orders_reconstruct_the_same_spend_key() {
        let maker = scalar(11);
        let taker = scalar(13);
        let maker_extracted = maker.adaptor_scalar_big_endian();
        let taker_extracted = taker.adaptor_scalar_big_endian();
        let maker_proof =
            CrossCurveDleqProofV1::prove(&maker, &mut ChaCha20Rng::from_seed([53; 32]))
                .expect("Maker DLEQ proof");
        let maker_wire = maker_proof.to_wire_bytes().expect("Maker proof wire");
        let maker_proof =
            CrossCurveDleqProofV1::from_wire_bytes(&maker_wire).expect("Maker proof round trip");
        let taker_proof =
            CrossCurveDleqProofV1::prove(&taker, &mut ChaCha20Rng::from_seed([54; 32]))
                .expect("Taker DLEQ proof");
        let address = MoneroSharedAddressV1::derive(
            MoneroAddressNetworkV1::Regtest,
            &maker_proof,
            &taker_proof,
            &view_key(17),
        )
        .expect("shared address");

        let claim_reconstructed = ReconstructedMoneroSpendKey::reconstruct(
            &address,
            &maker_proof,
            taker,
            maker_extracted,
        )
        .expect("claim-path reconstructed spend key");
        let refund_reconstructed = ReconstructedMoneroSpendKey::reconstruct(
            &address,
            &taker_proof,
            maker,
            taker_extracted,
        )
        .expect("refund-path reconstructed spend key");

        assert_eq!(claim_reconstructed.public_key(), address.public_spend_key());
        assert_eq!(
            refund_reconstructed.public_key(),
            address.public_spend_key()
        );
        assert_eq!(address.network(), MoneroAddressNetworkV1::Regtest);
        assert!(address.address_string().starts_with('4'));
    }

    #[test]
    fn unrelated_extracted_or_retained_share_cannot_open_the_funded_address() {
        let maker = scalar(11);
        let taker = scalar(13);
        let maker_proof =
            CrossCurveDleqProofV1::prove(&maker, &mut ChaCha20Rng::from_seed([53; 32]))
                .expect("Maker DLEQ proof");
        let taker_proof =
            CrossCurveDleqProofV1::prove(&taker, &mut ChaCha20Rng::from_seed([54; 32]))
                .expect("Taker DLEQ proof");
        let address = MoneroSharedAddressV1::derive(
            MoneroAddressNetworkV1::Regtest,
            &maker_proof,
            &taker_proof,
            &view_key(17),
        )
        .expect("shared address");

        assert!(matches!(
            ReconstructedMoneroSpendKey::reconstruct(
                &address,
                &maker_proof,
                scalar(13),
                scalar(19).adaptor_scalar_big_endian(),
            ),
            Err(MoneroSharedSpendError::CrossCurve(
                CrossCurveDleqError::AdaptorPointMismatch
            ))
        ));
        assert_eq!(
            ReconstructedMoneroSpendKey::reconstruct(
                &address,
                &maker_proof,
                scalar(23),
                maker.adaptor_scalar_big_endian(),
            )
            .expect_err("wrong Taker share"),
            MoneroSharedSpendError::ReconstructedSpendKeyMismatch
        );
    }
}
