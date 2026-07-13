//! Role-scoped transparent Zcash signing capability.

use std::{fmt, io};

use async_trait::async_trait;
use lez_swap_core::Participant;
use lez_zec_swap_sdk::{
    Bip199Contract, ClaimPreimage, TransactionBuildError, TransparentSpendRequest,
    build_claim_transaction, build_refund_transaction,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use zeroize::Zeroizing;

use crate::{ZebraClaimSigner, ZebraRefundSigner};

/// In-memory role-scoped key capability for the reference actor.
///
/// The secret is retained only in zeroizing storage. A short-lived
/// secp256k1 secret key is reconstructed for each canonical builder call and
/// erased immediately after that call. Production deployments may replace
/// this narrow trait implementation with an HSM or operating-system keystore.
#[derive(Clone)]
pub struct RoleKeyedZcashSigner {
    participant: Participant,
    public_key: PublicKey,
    secret_bytes: Zeroizing<[u8; 32]>,
}

impl RoleKeyedZcashSigner {
    /// Takes ownership of one validated secp256k1 key and fixes it to one role.
    #[must_use]
    pub fn new(participant: Participant, mut secret_key: SecretKey) -> Self {
        let public_key = PublicKey::from_secret_key(&Secp256k1::signing_only(), &secret_key);
        let secret_bytes = Zeroizing::new(secret_key.secret_bytes());
        secret_key.non_secure_erase();
        Self {
            participant,
            public_key,
            secret_bytes,
        }
    }

    /// Actor role that owns this key capability.
    #[must_use]
    pub const fn participant(&self) -> Participant {
        self.participant
    }

    /// Public key used by agreement and funding construction.
    #[must_use]
    pub const fn public_key(&self) -> PublicKey {
        self.public_key
    }

    fn with_secret<T>(
        &self,
        operation: impl FnOnce(&SecretKey) -> Result<T, TransactionBuildError>,
    ) -> Result<T, RoleKeyedZcashSignerError> {
        let mut secret_key = SecretKey::from_slice(self.secret_bytes.as_ref())
            .map_err(|_| RoleKeyedZcashSignerError::InvalidStoredKey)?;
        let result = operation(&secret_key);
        secret_key.non_secure_erase();
        result.map_err(RoleKeyedZcashSignerError::Build)
    }

    fn serialize(
        transaction: &zcash_primitives::transaction::Transaction,
    ) -> Result<Vec<u8>, RoleKeyedZcashSignerError> {
        let mut exact = Vec::new();
        transaction
            .write(&mut exact)
            .map_err(RoleKeyedZcashSignerError::Serialization)?;
        Ok(exact)
    }
}

impl fmt::Debug for RoleKeyedZcashSigner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RoleKeyedZcashSigner")
            .field("participant", &self.participant)
            .field("public_key", &self.public_key)
            .field("secret_bytes", &"[REDACTED]")
            .finish()
    }
}

/// Typed, non-secret signing failure.
#[derive(Debug, thiserror::Error)]
pub enum RoleKeyedZcashSignerError {
    /// Zeroizing storage no longer contained the validated constructor key.
    #[error("stored Zcash signing key is invalid")]
    InvalidStoredKey,
    /// Canonical librustzcash transaction construction rejected the request.
    #[error("canonical Zcash transaction construction failed: {0}")]
    Build(#[source] TransactionBuildError),
    /// Canonical transaction serialization failed.
    #[error("canonical Zcash transaction serialization failed: {0}")]
    Serialization(#[source] io::Error),
}

#[async_trait]
impl ZebraClaimSigner for RoleKeyedZcashSigner {
    type Error = RoleKeyedZcashSignerError;

    async fn sign_claim(
        &self,
        contract: &Bip199Contract,
        request: &TransparentSpendRequest,
        preimage: &ClaimPreimage,
    ) -> Result<Vec<u8>, Self::Error> {
        let transaction = self.with_secret(|secret_key| {
            build_claim_transaction(contract, request, secret_key, preimage.expose_secret())
        })?;
        Self::serialize(&transaction)
    }
}

#[async_trait]
impl ZebraRefundSigner for RoleKeyedZcashSigner {
    type Error = RoleKeyedZcashSignerError;

    fn participant(&self) -> Participant {
        self.participant
    }

    async fn sign_refund(
        &self,
        contract: &Bip199Contract,
        request: &TransparentSpendRequest,
    ) -> Result<Vec<u8>, Self::Error> {
        let transaction =
            self.with_secret(|secret_key| build_refund_transaction(contract, request, secret_key))?;
        Self::serialize(&transaction)
    }
}
