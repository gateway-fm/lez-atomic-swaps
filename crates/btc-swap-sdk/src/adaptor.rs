//! Role-separated `MuSig2` adaptor-signing session boundary.
//!
//! Public methods exchange canonical byte arrays so musig2 curve types remain
//! private to this crate and independent actors can use a stable wire boundary.

use bitcoin::hashes::{Hash as _, sha256};
use musig2::secp::{MaybeScalar, Point, Scalar};
use musig2::{
    AdaptorSignature, AggNonce, KeyAggContext, LiftedSignature, PartialSignature, PubNonce,
    SecNonce,
};
use thiserror::Error;
use zeroize::{Zeroize as _, Zeroizing};

const COMMITMENT_DOMAIN: &[u8] = b"lez-atomic-swaps/musig2/nonce-commitment/v1";

/// Fixed participant position in every two-party signing transcript.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum SigningRole {
    /// Liquidity provider and signer index zero.
    Maker,
    /// Swap initiator and signer index one.
    Taker,
}

impl SigningRole {
    const fn tag(self) -> u8 {
        match self {
            Self::Maker => 0,
            Self::Taker => 1,
        }
    }

    const fn opposite(self) -> Self {
        match self {
            Self::Maker => Self::Taker,
            Self::Taker => Self::Maker,
        }
    }
}

/// Complete immutable context shared by both signers.
///
/// Keys are compressed SEC1 bytes in maker/taker order. The message is the
/// exact 32-byte chain message. The session ID must commit the wider agreement
/// and the purpose of this signature.
#[derive(Clone, Debug)]
pub struct AdaptorSessionContext {
    key_aggregation: KeyAggContext,
    ordered_public_keys: [[u8; 33]; 2],
    output_key: [u8; 32],
    output_key_has_even_y: bool,
    message: [u8; 32],
    adaptor_point: [u8; 33],
    session_id: [u8; 32],
}

impl AdaptorSessionContext {
    /// Builds an untweaked context for a LEZ aggregate witnessed claim.
    ///
    /// # Errors
    ///
    /// Rejects malformed, duplicate, or non-aggregatable keys and a malformed
    /// adaptor point.
    pub fn untweaked(
        ordered_public_keys: [[u8; 33]; 2],
        message: [u8; 32],
        adaptor_point: [u8; 33],
        session_id: [u8; 32],
    ) -> Result<Self, AdaptorSessionError> {
        Self::new(
            ordered_public_keys,
            message,
            adaptor_point,
            session_id,
            None,
        )
    }

    /// Builds a BIP-341 x-only-tweaked context for a Taproot key-path claim.
    ///
    /// # Errors
    ///
    /// Rejects malformed, duplicate, or non-aggregatable keys, an invalid
    /// tweak result, or a malformed adaptor point.
    pub fn taproot(
        ordered_public_keys: [[u8; 33]; 2],
        merkle_root: [u8; 32],
        message: [u8; 32],
        adaptor_point: [u8; 33],
        session_id: [u8; 32],
    ) -> Result<Self, AdaptorSessionError> {
        Self::new(
            ordered_public_keys,
            message,
            adaptor_point,
            session_id,
            Some(merkle_root),
        )
    }

    fn new(
        ordered_public_keys: [[u8; 33]; 2],
        message: [u8; 32],
        adaptor_point: [u8; 33],
        session_id: [u8; 32],
        taproot_merkle_root: Option<[u8; 32]>,
    ) -> Result<Self, AdaptorSessionError> {
        let maker = Point::from_slice(&ordered_public_keys[0])
            .map_err(|_| AdaptorSessionError::InvalidPublicKey(SigningRole::Maker))?;
        let taker = Point::from_slice(&ordered_public_keys[1])
            .map_err(|_| AdaptorSessionError::InvalidPublicKey(SigningRole::Taker))?;
        if maker == taker {
            return Err(AdaptorSessionError::DuplicatePublicKeys);
        }
        Point::from_slice(&adaptor_point).map_err(|_| AdaptorSessionError::InvalidAdaptorPoint)?;
        let mut key_aggregation = KeyAggContext::new([maker, taker])
            .map_err(|_| AdaptorSessionError::InvalidKeyAggregation)?;
        if let Some(merkle_root) = taproot_merkle_root {
            key_aggregation = key_aggregation
                .with_taproot_tweak(&merkle_root)
                .map_err(|_| AdaptorSessionError::InvalidTaprootTweak)?;
        }
        let output_point = key_aggregation.aggregated_pubkey::<Point>();
        let output_key = output_point.serialize_xonly();
        Ok(Self {
            key_aggregation,
            ordered_public_keys,
            output_key,
            output_key_has_even_y: output_point.has_even_y(),
            message,
            adaptor_point,
            session_id,
        })
    }

    /// Exact x-only key under which the final signature verifies.
    #[must_use]
    pub const fn output_key(&self) -> [u8; 32] {
        self.output_key
    }

    /// Whether the full aggregate output point has even Y parity.
    #[must_use]
    pub const fn output_key_has_even_y(&self) -> bool {
        self.output_key_has_even_y
    }

    /// Exact chain message bound to this session.
    #[must_use]
    pub const fn message(&self) -> [u8; 32] {
        self.message
    }

    /// Public adaptor point shared by both chain sessions.
    #[must_use]
    pub const fn adaptor_point(&self) -> [u8; 33] {
        self.adaptor_point
    }

    /// Immutable caller-supplied transcript identity.
    #[must_use]
    pub const fn session_id(&self) -> [u8; 32] {
        self.session_id
    }

    fn public_key(&self, role: SigningRole) -> Result<Point, AdaptorSessionError> {
        Point::from_slice(&self.ordered_public_keys[usize::from(role.tag())])
            .map_err(|_| AdaptorSessionError::InvalidPublicKey(role))
    }

    fn adaptor_point_value(&self) -> Result<Point, AdaptorSessionError> {
        Point::from_slice(&self.adaptor_point).map_err(|_| AdaptorSessionError::InvalidAdaptorPoint)
    }
}

/// One role's one-way in-memory signing state.
///
/// This type is intentionally neither cloneable nor serializable. It zeroizes
/// its retained key bytes and serialized secret nonce on drop. The upstream
/// musig2 scalar objects are not zeroizing and remain a production-review item.
pub struct AdaptorSigner {
    context: AdaptorSessionContext,
    role: SigningRole,
    secret_key: Option<Zeroizing<[u8; 32]>>,
    secret_nonce: Option<Zeroizing<[u8; 97]>>,
    public_nonce: PubNonce,
    commitment: [u8; 32],
    peer_commitment: Option<[u8; 32]>,
    peer_nonce: Option<PubNonce>,
    own_partial: Option<PartialSignature>,
    peer_partial: Option<PartialSignature>,
}

impl std::fmt::Debug for AdaptorSigner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdaptorSigner")
            .field("role", &self.role)
            .field("session_id", &self.context.session_id)
            .field("secret_key", &"[REDACTED]")
            .field("secret_nonce", &"[REDACTED]")
            .field("peer_commitment_received", &self.peer_commitment.is_some())
            .field("peer_nonce_verified", &self.peer_nonce.is_some())
            .field("partial_created", &self.own_partial.is_some())
            .field("peer_partial_verified", &self.peer_partial.is_some())
            .finish_non_exhaustive()
    }
}

impl AdaptorSigner {
    /// Creates a signer with fresh OS-random BIP-327 nonce entropy.
    ///
    /// The caller must clear any other copy of the supplied key.
    ///
    /// # Errors
    ///
    /// Rejects a malformed or role-mismatched key, or unavailable OS entropy.
    pub fn new(
        context: AdaptorSessionContext,
        role: SigningRole,
        secret_key: [u8; 32],
    ) -> Result<Self, AdaptorSessionError> {
        let mut nonce_seed = [0_u8; 32];
        getrandom::fill(&mut nonce_seed).map_err(|_| AdaptorSessionError::RandomnessUnavailable)?;
        let result = Self::new_with_nonce_seed(context, role, secret_key, nonce_seed);
        nonce_seed.zeroize();
        result
    }

    fn new_with_nonce_seed(
        context: AdaptorSessionContext,
        role: SigningRole,
        secret_key: [u8; 32],
        nonce_seed: [u8; 32],
    ) -> Result<Self, AdaptorSessionError> {
        let retained_key = Zeroizing::new(secret_key);
        let scalar = Scalar::from_slice(retained_key.as_ref())
            .map_err(|_| AdaptorSessionError::InvalidSecretKey)?;
        if scalar.base_point_mul() != context.public_key(role)? {
            return Err(AdaptorSessionError::SecretKeyRoleMismatch);
        }
        let nonce = SecNonce::generate(
            nonce_seed,
            scalar,
            context.key_aggregation.aggregated_pubkey::<Point>(),
            context.message,
            context.session_id,
        );
        let public_nonce = nonce.public_nonce();
        let commitment = nonce_commitment(&context, role, &public_nonce.serialize());
        Ok(Self {
            context,
            role,
            secret_key: Some(retained_key),
            secret_nonce: Some(Zeroizing::new(nonce.serialize())),
            public_nonce,
            commitment,
            peer_commitment: None,
            peer_nonce: None,
            own_partial: None,
            peer_partial: None,
        })
    }

    /// Commitment to exchange before either public nonce is revealed.
    #[must_use]
    pub const fn nonce_commitment(&self) -> [u8; 32] {
        self.commitment
    }

    /// Accepts the peer commitment.
    ///
    /// # Errors
    ///
    /// Rejects duplicate or out-of-order delivery.
    pub fn accept_peer_commitment(
        &mut self,
        commitment: [u8; 32],
    ) -> Result<(), AdaptorSessionError> {
        if self.peer_commitment.is_some() || self.peer_nonce.is_some() {
            return Err(AdaptorSessionError::InvalidPhase);
        }
        self.peer_commitment = Some(commitment);
        Ok(())
    }

    /// Reveals this role's public nonce after commitment exchange.
    ///
    /// # Errors
    ///
    /// Rejects reveal before the peer commitment is accepted.
    pub fn public_nonce(&self) -> Result<[u8; 66], AdaptorSessionError> {
        self.peer_commitment
            .ok_or(AdaptorSessionError::InvalidPhase)?;
        Ok(self.public_nonce.serialize())
    }

    /// Verifies that the peer nonce opens its prior commitment.
    ///
    /// # Errors
    ///
    /// Rejects missing commitment, malformed or duplicate nonce, and mismatch.
    pub fn accept_peer_nonce(&mut self, nonce_bytes: [u8; 66]) -> Result<(), AdaptorSessionError> {
        let expected = self
            .peer_commitment
            .ok_or(AdaptorSessionError::InvalidPhase)?;
        if self.peer_nonce.is_some() || self.own_partial.is_some() {
            return Err(AdaptorSessionError::InvalidPhase);
        }
        let nonce = PubNonce::from_bytes(&nonce_bytes)
            .map_err(|_| AdaptorSessionError::InvalidPublicNonce)?;
        let actual = nonce_commitment(&self.context, self.role.opposite(), &nonce_bytes);
        if actual != expected {
            return Err(AdaptorSessionError::NonceCommitmentMismatch);
        }
        self.peer_nonce = Some(nonce);
        Ok(())
    }

    /// Consumes the secret nonce and creates a 32-byte adaptor partial.
    ///
    /// # Errors
    ///
    /// Rejects out-of-order or duplicate signing, corrupt retained state, and
    /// signing failure.
    pub fn create_partial_signature(&mut self) -> Result<[u8; 32], AdaptorSessionError> {
        if self.own_partial.is_some() || self.peer_nonce.is_none() {
            return Err(AdaptorSessionError::InvalidPhase);
        }
        let key_bytes = self
            .secret_key
            .take()
            .ok_or(AdaptorSessionError::InvalidPhase)?;
        let nonce_bytes = self
            .secret_nonce
            .take()
            .ok_or(AdaptorSessionError::InvalidPhase)?;
        let scalar = Scalar::from_slice(key_bytes.as_ref())
            .map_err(|_| AdaptorSessionError::InvalidSecretKey)?;
        let nonce = SecNonce::from_bytes(nonce_bytes.as_ref())
            .map_err(|_| AdaptorSessionError::InvalidSecretNonce)?;
        let partial: PartialSignature = musig2::adaptor::sign_partial(
            &self.context.key_aggregation,
            scalar,
            nonce,
            &self.aggregate_nonce()?,
            self.context.adaptor_point_value()?,
            self.context.message,
        )
        .map_err(|_| AdaptorSessionError::PartialSigningFailed)?;
        let bytes = partial.serialize();
        self.own_partial = Some(partial);
        Ok(bytes)
    }

    /// Verifies and retains the peer adaptor partial.
    ///
    /// # Errors
    ///
    /// Rejects out-of-order, malformed, duplicate, or invalid partials.
    pub fn accept_peer_partial_signature(
        &mut self,
        partial_bytes: [u8; 32],
    ) -> Result<(), AdaptorSessionError> {
        if self.own_partial.is_none() || self.peer_partial.is_some() {
            return Err(AdaptorSessionError::InvalidPhase);
        }
        let partial = MaybeScalar::from_slice(&partial_bytes)
            .map_err(|_| AdaptorSessionError::InvalidPartialSignature)?;
        let peer_nonce = self
            .peer_nonce
            .as_ref()
            .ok_or(AdaptorSessionError::InvalidPhase)?;
        musig2::adaptor::verify_partial(
            &self.context.key_aggregation,
            partial,
            &self.aggregate_nonce()?,
            self.context.adaptor_point_value()?,
            self.context.public_key(self.role.opposite())?,
            peer_nonce,
            self.context.message,
        )
        .map_err(|_| AdaptorSessionError::PeerPartialVerificationFailed)?;
        self.peer_partial = Some(partial);
        Ok(())
    }

    /// Aggregates both verified partials into a 65-byte presignature.
    ///
    /// # Errors
    ///
    /// Rejects incomplete rounds or an invalid aggregate.
    pub fn presignature(&self) -> Result<[u8; 65], AdaptorSessionError> {
        let own = self.own_partial.ok_or(AdaptorSessionError::InvalidPhase)?;
        let peer = self.peer_partial.ok_or(AdaptorSessionError::InvalidPhase)?;
        let partials = match self.role {
            SigningRole::Maker => [own, peer],
            SigningRole::Taker => [peer, own],
        };
        let adaptor_point = self.context.adaptor_point_value()?;
        let presignature = musig2::adaptor::aggregate_partial_signatures(
            &self.context.key_aggregation,
            &self.aggregate_nonce()?,
            adaptor_point,
            partials,
            self.context.message,
        )
        .map_err(|_| AdaptorSessionError::PresignatureAggregationFailed)?;
        musig2::adaptor::verify_single(
            self.context.key_aggregation.aggregated_pubkey::<Point>(),
            &presignature,
            self.context.message,
            adaptor_point,
        )
        .map_err(|_| AdaptorSessionError::PresignatureVerificationFailed)?;
        Ok(presignature.serialize())
    }

    fn aggregate_nonce(&self) -> Result<AggNonce, AdaptorSessionError> {
        let peer = self
            .peer_nonce
            .as_ref()
            .ok_or(AdaptorSessionError::InvalidPhase)?;
        Ok(match self.role {
            SigningRole::Maker => AggNonce::sum([&self.public_nonce, peer]),
            SigningRole::Taker => AggNonce::sum([peer, &self.public_nonce]),
        })
    }
}

/// Adapts a verified presignature with the committed scalar.
///
/// # Errors
///
/// Rejects malformed or mismatched inputs and an invalid final signature.
pub fn adapt_presignature(
    context: &AdaptorSessionContext,
    presignature: [u8; 65],
    adaptor_secret: Zeroizing<[u8; 32]>,
) -> Result<[u8; 64], AdaptorSessionError> {
    let presignature = verified_presignature(context, &presignature)?;
    let secret = Scalar::from_slice(adaptor_secret.as_ref())
        .map_err(|_| AdaptorSessionError::InvalidAdaptorSecret)?;
    drop(adaptor_secret);
    if secret.base_point_mul() != context.adaptor_point_value()? {
        return Err(AdaptorSessionError::AdaptorSecretPointMismatch);
    }
    let final_signature: LiftedSignature = presignature
        .adapt(secret)
        .ok_or(AdaptorSessionError::AdaptationFailed)?;
    let bytes = final_signature.serialize();
    verify_final_signature(context, bytes)?;
    Ok(bytes)
}

/// Extracts and point-checks the adaptor scalar from a related final signature.
///
/// # Errors
///
/// Rejects malformed, invalid, unrelated, zero, or point-mismatched inputs.
pub fn extract_adaptor_secret(
    context: &AdaptorSessionContext,
    presignature: [u8; 65],
    final_signature: [u8; 64],
) -> Result<Zeroizing<[u8; 32]>, AdaptorSessionError> {
    let presignature = verified_presignature(context, &presignature)?;
    let final_signature = parsed_final_signature(context, &final_signature)?;
    let extracted: MaybeScalar = presignature
        .reveal_secret(&final_signature)
        .ok_or(AdaptorSessionError::ExtractionFailed)?;
    let MaybeScalar::Valid(extracted) = extracted else {
        return Err(AdaptorSessionError::InvalidExtractedScalar);
    };
    if extracted.base_point_mul() != context.adaptor_point_value()? {
        return Err(AdaptorSessionError::ExtractedScalarPointMismatch);
    }
    Ok(Zeroizing::new(extracted.serialize()))
}

/// Verifies a final signature under the exact key and message.
///
/// # Errors
///
/// Rejects malformed or invalid signature bytes.
pub fn verify_final_signature(
    context: &AdaptorSessionContext,
    final_signature: [u8; 64],
) -> Result<(), AdaptorSessionError> {
    parsed_final_signature(context, &final_signature).map(|_| ())
}

fn parsed_final_signature(
    context: &AdaptorSessionContext,
    final_signature: &[u8; 64],
) -> Result<LiftedSignature, AdaptorSessionError> {
    let signature = LiftedSignature::from_bytes(final_signature)
        .map_err(|_| AdaptorSessionError::InvalidFinalSignature)?;
    musig2::verify_single(
        context.key_aggregation.aggregated_pubkey::<Point>(),
        signature,
        context.message,
    )
    .map_err(|_| AdaptorSessionError::FinalSignatureVerificationFailed)?;
    Ok(signature)
}

fn verified_presignature(
    context: &AdaptorSessionContext,
    presignature: &[u8; 65],
) -> Result<AdaptorSignature, AdaptorSessionError> {
    let signature = AdaptorSignature::from_bytes(presignature)
        .map_err(|_| AdaptorSessionError::InvalidPresignature)?;
    musig2::adaptor::verify_single(
        context.key_aggregation.aggregated_pubkey::<Point>(),
        &signature,
        context.message,
        context.adaptor_point_value()?,
    )
    .map_err(|_| AdaptorSessionError::PresignatureVerificationFailed)?;
    Ok(signature)
}

fn nonce_commitment(
    context: &AdaptorSessionContext,
    role: SigningRole,
    public_nonce: &[u8; 66],
) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(COMMITMENT_DOMAIN.len() + 1 + 32 + 66 + 32 + 32 + 33 + 66);
    bytes.extend_from_slice(COMMITMENT_DOMAIN);
    bytes.push(role.tag());
    bytes.extend_from_slice(&context.session_id);
    bytes.extend_from_slice(&context.ordered_public_keys[0]);
    bytes.extend_from_slice(&context.ordered_public_keys[1]);
    bytes.extend_from_slice(&context.output_key);
    bytes.extend_from_slice(&context.message);
    bytes.extend_from_slice(&context.adaptor_point);
    bytes.extend_from_slice(public_nonce);
    sha256::Hash::hash(&bytes).to_byte_array()
}

/// Failures from the `MuSig2` adaptor session boundary.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AdaptorSessionError {
    /// An ordered public key was malformed.
    #[error("invalid {0:?} public key")]
    InvalidPublicKey(SigningRole),
    /// Both roles used the same public key.
    #[error("maker and taker public keys must be distinct")]
    DuplicatePublicKeys,
    /// Ordered key aggregation failed.
    #[error("invalid MuSig2 key aggregation")]
    InvalidKeyAggregation,
    /// Applying the Taproot tweak failed.
    #[error("invalid Taproot tweak")]
    InvalidTaprootTweak,
    /// The adaptor point was malformed.
    #[error("invalid adaptor point")]
    InvalidAdaptorPoint,
    /// A signing key was malformed.
    #[error("invalid signing secret key")]
    InvalidSecretKey,
    /// A signing key did not match its role.
    #[error("signing secret key does not match role public key")]
    SecretKeyRoleMismatch,
    /// OS entropy was unavailable.
    #[error("OS randomness unavailable")]
    RandomnessUnavailable,
    /// A method was called outside its one-way phase.
    #[error("invalid adaptor signing phase")]
    InvalidPhase,
    /// The peer nonce did not open its commitment.
    #[error("peer nonce commitment mismatch")]
    NonceCommitmentMismatch,
    /// Peer nonce bytes were malformed.
    #[error("invalid peer public nonce")]
    InvalidPublicNonce,
    /// Retained secret nonce bytes were malformed.
    #[error("invalid retained secret nonce")]
    InvalidSecretNonce,
    /// Local partial signing failed.
    #[error("adaptor partial signing failed")]
    PartialSigningFailed,
    /// Peer partial bytes were malformed.
    #[error("invalid peer partial signature")]
    InvalidPartialSignature,
    /// The peer partial did not verify.
    #[error("peer adaptor partial verification failed")]
    PeerPartialVerificationFailed,
    /// Partial aggregation failed.
    #[error("adaptor presignature aggregation failed")]
    PresignatureAggregationFailed,
    /// Presignature bytes were malformed.
    #[error("invalid adaptor presignature")]
    InvalidPresignature,
    /// The presignature did not verify.
    #[error("adaptor presignature verification failed")]
    PresignatureVerificationFailed,
    /// The adaptor scalar was malformed.
    #[error("invalid adaptor secret")]
    InvalidAdaptorSecret,
    /// The scalar did not match its point.
    #[error("adaptor secret does not match committed point")]
    AdaptorSecretPointMismatch,
    /// Adaptation failed.
    #[error("adaptor signature adaptation failed")]
    AdaptationFailed,
    /// Final signature bytes were malformed.
    #[error("invalid final signature")]
    InvalidFinalSignature,
    /// The final signature did not verify.
    #[error("final signature verification failed")]
    FinalSignatureVerificationFailed,
    /// The final signature was unrelated to the presignature.
    #[error("adaptor scalar extraction failed")]
    ExtractionFailed,
    /// Extraction returned zero.
    #[error("extracted adaptor scalar is zero")]
    InvalidExtractedScalar,
    /// The extracted scalar did not match its point.
    #[error("extracted adaptor scalar does not match committed point")]
    ExtractedScalarPointMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAKER_SECRET: [u8; 32] = [0x31; 32];
    const TAKER_SECRET: [u8; 32] = [0x42; 32];
    const ADAPTOR_SECRET: [u8; 32] = [0x53; 32];

    fn context(message: [u8; 32]) -> AdaptorSessionContext {
        let maker = Scalar::from_slice(&MAKER_SECRET).unwrap();
        let taker = Scalar::from_slice(&TAKER_SECRET).unwrap();
        let adaptor = Scalar::from_slice(&ADAPTOR_SECRET).unwrap();
        AdaptorSessionContext::untweaked(
            [
                maker.base_point_mul().serialize(),
                taker.base_point_mul().serialize(),
            ],
            message,
            adaptor.base_point_mul().serialize(),
            [0x61; 32],
        )
        .unwrap()
    }

    fn signers(
        context: &AdaptorSessionContext,
    ) -> Result<(AdaptorSigner, AdaptorSigner), AdaptorSessionError> {
        Ok((
            AdaptorSigner::new_with_nonce_seed(
                context.clone(),
                SigningRole::Maker,
                MAKER_SECRET,
                [0x71; 32],
            )?,
            AdaptorSigner::new_with_nonce_seed(
                context.clone(),
                SigningRole::Taker,
                TAKER_SECRET,
                [0x82; 32],
            )?,
        ))
    }

    fn complete(context: &AdaptorSessionContext) -> Result<[u8; 65], AdaptorSessionError> {
        let (mut maker, mut taker) = signers(context)?;
        maker.accept_peer_commitment(taker.nonce_commitment())?;
        taker.accept_peer_commitment(maker.nonce_commitment())?;
        let maker_nonce = maker.public_nonce()?;
        let taker_nonce = taker.public_nonce()?;
        maker.accept_peer_nonce(taker_nonce)?;
        taker.accept_peer_nonce(maker_nonce)?;
        let maker_partial = maker.create_partial_signature()?;
        let taker_partial = taker.create_partial_signature()?;
        maker.accept_peer_partial_signature(taker_partial)?;
        taker.accept_peer_partial_signature(maker_partial)?;
        let maker_presignature = maker.presignature()?;
        assert_eq!(maker_presignature, taker.presignature()?);
        Ok(maker_presignature)
    }

    #[test]
    fn dual_role_happy_path_adapts_extracts_and_verifies() {
        let context = context([0x91; 32]);
        let presignature = complete(&context).unwrap();
        let final_signature =
            adapt_presignature(&context, presignature, Zeroizing::new(ADAPTOR_SECRET)).unwrap();
        verify_final_signature(&context, final_signature).unwrap();
        assert_eq!(
            *extract_adaptor_secret(&context, presignature, final_signature).unwrap(),
            ADAPTOR_SECRET
        );
    }

    #[test]
    fn commitment_must_precede_nonce_reveal() {
        let context = context([0x92; 32]);
        let (maker, _) = signers(&context).unwrap();
        assert_eq!(maker.public_nonce(), Err(AdaptorSessionError::InvalidPhase));
    }

    #[test]
    fn commitment_mismatch_and_nonce_reuse_fail_closed() {
        let context = context([0x93; 32]);
        let (mut maker, mut taker) = signers(&context).unwrap();
        let mut wrong_commitment = taker.nonce_commitment();
        wrong_commitment[0] ^= 1;
        maker.accept_peer_commitment(wrong_commitment).unwrap();
        taker
            .accept_peer_commitment(maker.nonce_commitment())
            .unwrap();
        assert_eq!(
            maker.accept_peer_nonce(taker.public_nonce().unwrap()),
            Err(AdaptorSessionError::NonceCommitmentMismatch)
        );

        let (mut maker, mut taker) = signers(&context).unwrap();
        maker
            .accept_peer_commitment(taker.nonce_commitment())
            .unwrap();
        taker
            .accept_peer_commitment(maker.nonce_commitment())
            .unwrap();
        maker
            .accept_peer_nonce(taker.public_nonce().unwrap())
            .unwrap();
        taker
            .accept_peer_nonce(maker.public_nonce().unwrap())
            .unwrap();
        maker.create_partial_signature().unwrap();
        assert_eq!(
            maker.create_partial_signature(),
            Err(AdaptorSessionError::InvalidPhase)
        );
    }

    #[test]
    fn message_and_adaptor_point_are_bound() {
        let current_context = context([0x94; 32]);
        let presignature = complete(&current_context).unwrap();
        let final_signature = adapt_presignature(
            &current_context,
            presignature,
            Zeroizing::new(ADAPTOR_SECRET),
        )
        .unwrap();
        let changed_message = context([0x95; 32]);
        assert_eq!(
            verify_final_signature(&changed_message, final_signature),
            Err(AdaptorSessionError::FinalSignatureVerificationFailed)
        );
        assert_eq!(
            adapt_presignature(&current_context, presignature, Zeroizing::new([0x54; 32])),
            Err(AdaptorSessionError::AdaptorSecretPointMismatch)
        );
    }
}
