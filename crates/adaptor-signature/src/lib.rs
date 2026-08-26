//! Pair-neutral role-separated `MuSig2` adaptor-signing session boundary.
//!
//! Public methods exchange canonical byte arrays so musig2 curve types remain
//! private to this crate and independent actors can use a stable wire boundary.

use musig2::secp::{MaybeScalar, Point, Scalar};
use musig2::{
    AdaptorSignature, AggNonce, KeyAggContext, LiftedSignature, PartialSignature, PubNonce,
    SecNonce,
};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use zeroize::{Zeroize as _, Zeroizing};

const COMMITMENT_DOMAIN: &[u8] = b"lez-atomic-swaps/musig2/nonce-commitment/v1";
const DURABLE_CONTEXT_DOMAIN: &[u8] = b"lez-atomic-swaps/musig2/durable-context/v1";

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
    taproot_merkle_root: Option<[u8; 32]>,
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
            taproot_merkle_root,
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

    /// Compressed participant keys in canonical maker/taker order.
    #[must_use]
    pub const fn ordered_public_keys(&self) -> [[u8; 33]; 2] {
        self.ordered_public_keys
    }

    /// Domain-separated commitment to every SDK input that affects signing.
    ///
    /// A durable actor stores this value alongside the serialized secret nonce
    /// and compares it again after restart. This prevents a valid nonce from
    /// being reconstructed under a changed message, participant order, adaptor
    /// point, session ID, or Taproot tweak.
    #[must_use]
    pub fn durable_context_binding(&self) -> [u8; 32] {
        let mut bytes =
            Vec::with_capacity(DURABLE_CONTEXT_DOMAIN.len() + 32 + 66 + 32 + 1 + 32 + 33 + 32 + 1);
        bytes.extend_from_slice(DURABLE_CONTEXT_DOMAIN);
        bytes.extend_from_slice(&self.session_id);
        bytes.extend_from_slice(&self.ordered_public_keys[0]);
        bytes.extend_from_slice(&self.ordered_public_keys[1]);
        bytes.extend_from_slice(&self.output_key);
        bytes.push(u8::from(self.output_key_has_even_y));
        bytes.extend_from_slice(&self.message);
        bytes.extend_from_slice(&self.adaptor_point);
        match self.taproot_merkle_root {
            Some(root) => {
                bytes.push(1);
                bytes.extend_from_slice(&root);
            }
            None => bytes.push(0),
        }
        Sha256::digest(&bytes).into()
    }

    fn public_key(&self, role: SigningRole) -> Result<Point, AdaptorSessionError> {
        Point::from_slice(&self.ordered_public_keys[usize::from(role.tag())])
            .map_err(|_| AdaptorSessionError::InvalidPublicKey(role))
    }

    fn adaptor_point_value(&self) -> Result<Point, AdaptorSessionError> {
        Point::from_slice(&self.adaptor_point).map_err(|_| AdaptorSessionError::InvalidAdaptorPoint)
    }
}

/// Fresh nonce bytes that must be durably reserved before the commitment is exposed.
///
/// The secret nonce uses the upstream `musig2` BIP-327 97-byte encoding and is
/// overwritten when this value is dropped. The public nonce and commitment are
/// safe to persist and exchange in the journal-defined order.
#[must_use]
pub struct FreshAdaptorNonce {
    secret_nonce: Zeroizing<[u8; 97]>,
    public_nonce: [u8; 66],
    commitment: [u8; 32],
    context_binding: [u8; 32],
}

impl std::fmt::Debug for FreshAdaptorNonce {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FreshAdaptorNonce")
            .field("secret_nonce", &"[REDACTED]")
            .field("public_nonce", &self.public_nonce)
            .field("commitment", &self.commitment)
            .field("context_binding", &self.context_binding)
            .finish()
    }
}

impl FreshAdaptorNonce {
    /// Generates fresh OS-random BIP-327 nonce material for one exact context and role.
    ///
    /// The caller must atomically persist `secret_nonce`, `public_nonce`,
    /// `commitment`, and `context_binding` before returning the commitment to a
    /// peer. The retained secret nonce is zeroized on drop.
    ///
    /// # Errors
    ///
    /// Rejects an invalid or role-mismatched key and unavailable OS entropy.
    pub fn generate(
        context: &AdaptorSessionContext,
        role: SigningRole,
        secret_key: [u8; 32],
    ) -> Result<Self, AdaptorSessionError> {
        let mut nonce_seed = [0_u8; 32];
        getrandom::fill(&mut nonce_seed).map_err(|_| AdaptorSessionError::RandomnessUnavailable)?;
        let result = Self::generate_with_nonce_seed(context, role, secret_key, nonce_seed);
        nonce_seed.zeroize();
        result
    }

    fn generate_with_nonce_seed(
        context: &AdaptorSessionContext,
        role: SigningRole,
        secret_key: [u8; 32],
        nonce_seed: [u8; 32],
    ) -> Result<Self, AdaptorSessionError> {
        let retained_key = Zeroizing::new(secret_key);
        let scalar = validated_secret_scalar(context, role, &retained_key)?;
        let nonce = SecNonce::generate(
            nonce_seed,
            scalar,
            context.key_aggregation.aggregated_pubkey::<Point>(),
            context.message,
            context.session_id,
        );
        let public_nonce = nonce.public_nonce().serialize();
        Ok(Self {
            secret_nonce: Zeroizing::new(nonce.serialize()),
            public_nonce,
            commitment: nonce_commitment(context, role, &public_nonce),
            context_binding: context.durable_context_binding(),
        })
    }

    /// Borrowed BIP-327 secret nonce bytes for immediate durable reservation.
    ///
    /// Any caller-created copy must itself be zeroized after the journal takes ownership.
    #[must_use]
    pub fn secret_nonce(&self) -> &[u8; 97] {
        &self.secret_nonce
    }

    /// Canonical two-point public nonce bytes.
    #[must_use]
    pub const fn public_nonce(&self) -> [u8; 66] {
        self.public_nonce
    }

    /// Context- and role-bound nonce commitment.
    #[must_use]
    pub const fn commitment(&self) -> [u8; 32] {
        self.commitment
    }

    /// Exact SDK context binding to persist with the nonce.
    #[must_use]
    pub const fn context_binding(&self) -> [u8; 32] {
        self.context_binding
    }
}

/// Borrowed durable bytes required to reconstruct one partial signature.
///
/// This type contains no key. Its secret nonce is borrowed from the journal's
/// one-shot signing callback and is never included in `Debug` output.
#[must_use]
pub struct PersistedAdaptorSigningMaterial<'a> {
    context_binding: [u8; 32],
    secret_nonce: &'a [u8; 97],
    own_public_nonce: [u8; 66],
    own_commitment: [u8; 32],
    peer_commitment: [u8; 32],
    peer_public_nonce: [u8; 66],
}

impl<'a> PersistedAdaptorSigningMaterial<'a> {
    /// Reconstructs the SDK-facing view of journal-owned signing bytes.
    pub const fn new(
        context_binding: [u8; 32],
        secret_nonce: &'a [u8; 97],
        own_public_nonce: [u8; 66],
        own_commitment: [u8; 32],
        peer_commitment: [u8; 32],
        peer_public_nonce: [u8; 66],
    ) -> Self {
        Self {
            context_binding,
            secret_nonce,
            own_public_nonce,
            own_commitment,
            peer_commitment,
            peer_public_nonce,
        }
    }
}

impl std::fmt::Debug for PersistedAdaptorSigningMaterial<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PersistedAdaptorSigningMaterial")
            .field("context_binding", &self.context_binding)
            .field("secret_nonce", &"[REDACTED]")
            .field("own_public_nonce", &self.own_public_nonce)
            .field("own_commitment", &self.own_commitment)
            .field("peer_commitment", &self.peer_commitment)
            .field("peer_public_nonce", &self.peer_public_nonce)
            .finish()
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
        let scalar = validated_secret_scalar(&context, role, &retained_key)?;
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

/// Verifies that a public nonce is canonical and opens the exact role-bound commitment.
///
/// This check must succeed before journal code records a peer public nonce.
///
/// # Errors
///
/// Rejects malformed nonce bytes or a commitment from another context, role, or nonce.
pub fn verify_nonce_commitment(
    context: &AdaptorSessionContext,
    role: SigningRole,
    expected_commitment: [u8; 32],
    public_nonce: [u8; 66],
) -> Result<(), AdaptorSessionError> {
    PubNonce::from_bytes(&public_nonce).map_err(|_| AdaptorSessionError::InvalidPublicNonce)?;
    if nonce_commitment(context, role, &public_nonce) != expected_commitment {
        return Err(AdaptorSessionError::NonceCommitmentMismatch);
    }
    Ok(())
}

/// Reconstructs and consumes a persisted secret nonce to create one adaptor partial.
///
/// The SDK first checks the durable context binding, the peer commitment, the
/// BIP-327 nonce encoding, and that the secret nonce derives the persisted local
/// public nonce. The journal remains responsible for invoking this function at
/// most once and atomically replacing its secret nonce with the returned outbox.
///
/// The local copy of `secret_key` is overwritten on return. Upstream `musig2`
/// scalar objects are not guaranteed to zeroize and remain a production-review item.
///
/// # Errors
///
/// Rejects changed context, malformed or mismatched nonce material, a
/// role-mismatched key, or partial-signing failure.
#[allow(clippy::needless_pass_by_value)] // Ownership lets this function zeroize its key copy.
pub fn sign_persisted_adaptor_partial(
    context: &AdaptorSessionContext,
    role: SigningRole,
    secret_key: [u8; 32],
    material: PersistedAdaptorSigningMaterial<'_>,
) -> Result<[u8; 32], AdaptorSessionError> {
    if material.context_binding != context.durable_context_binding() {
        return Err(AdaptorSessionError::DurableContextMismatch);
    }
    verify_nonce_commitment(
        context,
        role,
        material.own_commitment,
        material.own_public_nonce,
    )?;
    verify_nonce_commitment(
        context,
        role.opposite(),
        material.peer_commitment,
        material.peer_public_nonce,
    )?;
    let own_public_nonce = PubNonce::from_bytes(&material.own_public_nonce)
        .map_err(|_| AdaptorSessionError::InvalidPublicNonce)?;
    let peer_public_nonce = PubNonce::from_bytes(&material.peer_public_nonce)
        .map_err(|_| AdaptorSessionError::InvalidPublicNonce)?;
    let secret_nonce = SecNonce::from_bytes(material.secret_nonce)
        .map_err(|_| AdaptorSessionError::InvalidSecretNonce)?;
    if secret_nonce.public_nonce() != own_public_nonce {
        return Err(AdaptorSessionError::SecretNoncePublicNonceMismatch);
    }
    let retained_key = Zeroizing::new(secret_key);
    let scalar = validated_secret_scalar(context, role, &retained_key)?;
    let aggregate_nonce = aggregate_nonce(role, &own_public_nonce, &peer_public_nonce);
    let partial: PartialSignature = musig2::adaptor::sign_partial(
        &context.key_aggregation,
        scalar,
        secret_nonce,
        &aggregate_nonce,
        context.adaptor_point_value()?,
        context.message,
    )
    .map_err(|_| AdaptorSessionError::PartialSigningFailed)?;
    Ok(partial.serialize())
}

/// Verifies one role's partial against the exact ordered nonce transcript.
///
/// # Errors
///
/// Rejects malformed nonces or partial bytes and any message, key, adaptor, or
/// nonce transcript under which the partial does not verify.
pub fn verify_adaptor_partial_signature(
    context: &AdaptorSessionContext,
    signer_role: SigningRole,
    maker_public_nonce: [u8; 66],
    taker_public_nonce: [u8; 66],
    partial_signature: [u8; 32],
) -> Result<(), AdaptorSessionError> {
    let maker_nonce = PubNonce::from_bytes(&maker_public_nonce)
        .map_err(|_| AdaptorSessionError::InvalidPublicNonce)?;
    let taker_nonce = PubNonce::from_bytes(&taker_public_nonce)
        .map_err(|_| AdaptorSessionError::InvalidPublicNonce)?;
    let partial = MaybeScalar::from_slice(&partial_signature)
        .map_err(|_| AdaptorSessionError::InvalidPartialSignature)?;
    let aggregate_nonce = AggNonce::sum([&maker_nonce, &taker_nonce]);
    let signer_nonce = match signer_role {
        SigningRole::Maker => &maker_nonce,
        SigningRole::Taker => &taker_nonce,
    };
    musig2::adaptor::verify_partial(
        &context.key_aggregation,
        partial,
        &aggregate_nonce,
        context.adaptor_point_value()?,
        context.public_key(signer_role)?,
        signer_nonce,
        context.message,
    )
    .map_err(|_| AdaptorSessionError::PeerPartialVerificationFailed)
}

/// Verifies both partials and aggregates the exact transcript into a presignature.
///
/// # Errors
///
/// Rejects malformed or invalid nonce/partial bytes or an invalid aggregate.
pub fn aggregate_adaptor_presignature(
    context: &AdaptorSessionContext,
    maker_public_nonce: [u8; 66],
    taker_public_nonce: [u8; 66],
    maker_partial: [u8; 32],
    taker_partial: [u8; 32],
) -> Result<[u8; 65], AdaptorSessionError> {
    verify_adaptor_partial_signature(
        context,
        SigningRole::Maker,
        maker_public_nonce,
        taker_public_nonce,
        maker_partial,
    )?;
    verify_adaptor_partial_signature(
        context,
        SigningRole::Taker,
        maker_public_nonce,
        taker_public_nonce,
        taker_partial,
    )?;
    let maker_nonce = PubNonce::from_bytes(&maker_public_nonce)
        .map_err(|_| AdaptorSessionError::InvalidPublicNonce)?;
    let taker_nonce = PubNonce::from_bytes(&taker_public_nonce)
        .map_err(|_| AdaptorSessionError::InvalidPublicNonce)?;
    let maker_partial = MaybeScalar::from_slice(&maker_partial)
        .map_err(|_| AdaptorSessionError::InvalidPartialSignature)?;
    let taker_partial = MaybeScalar::from_slice(&taker_partial)
        .map_err(|_| AdaptorSessionError::InvalidPartialSignature)?;
    let aggregate_nonce = AggNonce::sum([&maker_nonce, &taker_nonce]);
    let adaptor_point = context.adaptor_point_value()?;
    let presignature = musig2::adaptor::aggregate_partial_signatures(
        &context.key_aggregation,
        &aggregate_nonce,
        adaptor_point,
        [maker_partial, taker_partial],
        context.message,
    )
    .map_err(|_| AdaptorSessionError::PresignatureAggregationFailed)?;
    musig2::adaptor::verify_single(
        context.key_aggregation.aggregated_pubkey::<Point>(),
        &presignature,
        context.message,
        adaptor_point,
    )
    .map_err(|_| AdaptorSessionError::PresignatureVerificationFailed)?;
    Ok(presignature.serialize())
}

/// Verifies an aggregate adaptor presignature under the exact context.
///
/// # Errors
///
/// Rejects malformed bytes or a signature from another message, key, or adaptor point.
pub fn verify_adaptor_presignature(
    context: &AdaptorSessionContext,
    presignature: [u8; 65],
) -> Result<(), AdaptorSessionError> {
    verified_presignature(context, &presignature).map(|_| ())
}

/// Point-checks a private adaptor scalar without creating a final signature.
///
/// # Errors
///
/// Rejects a zero, out-of-range, or differently committed scalar. The caller's
/// byte buffer is zeroized before this function returns.
pub fn verify_adaptor_secret(
    context: &AdaptorSessionContext,
    adaptor_secret: Zeroizing<[u8; 32]>,
) -> Result<(), AdaptorSessionError> {
    checked_adaptor_secret(context, adaptor_secret).map(drop)
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
    let secret = checked_adaptor_secret(context, adaptor_secret)?;
    let final_signature: LiftedSignature = presignature
        .adapt(secret)
        .ok_or(AdaptorSessionError::AdaptationFailed)?;
    let bytes = final_signature.serialize();
    verify_final_signature(context, bytes)?;
    Ok(bytes)
}

fn checked_adaptor_secret(
    context: &AdaptorSessionContext,
    adaptor_secret: Zeroizing<[u8; 32]>,
) -> Result<Scalar, AdaptorSessionError> {
    let secret = Scalar::from_slice(adaptor_secret.as_ref())
        .map_err(|_| AdaptorSessionError::InvalidAdaptorSecret)?;
    drop(adaptor_secret);
    if secret.base_point_mul() != context.adaptor_point_value()? {
        return Err(AdaptorSessionError::AdaptorSecretPointMismatch);
    }
    Ok(secret)
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

fn validated_secret_scalar(
    context: &AdaptorSessionContext,
    role: SigningRole,
    secret_key: &[u8; 32],
) -> Result<Scalar, AdaptorSessionError> {
    let scalar =
        Scalar::from_slice(secret_key).map_err(|_| AdaptorSessionError::InvalidSecretKey)?;
    if scalar.base_point_mul() != context.public_key(role)? {
        return Err(AdaptorSessionError::SecretKeyRoleMismatch);
    }
    Ok(scalar)
}

fn aggregate_nonce(role: SigningRole, own: &PubNonce, peer: &PubNonce) -> AggNonce {
    match role {
        SigningRole::Maker => AggNonce::sum([own, peer]),
        SigningRole::Taker => AggNonce::sum([peer, own]),
    }
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
    Sha256::digest(&bytes).into()
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
    /// A public nonce did not open its role-bound commitment.
    #[error("nonce commitment mismatch")]
    NonceCommitmentMismatch,
    /// Public nonce bytes were malformed.
    #[error("invalid public nonce")]
    InvalidPublicNonce,
    /// The restart-time context differs from the context reserved with the nonce.
    #[error("durable adaptor context binding mismatch")]
    DurableContextMismatch,
    /// Retained secret nonce bytes were malformed.
    #[error("invalid retained secret nonce")]
    InvalidSecretNonce,
    /// The retained secret nonce does not derive the persisted local public nonce.
    #[error("secret nonce does not match persisted public nonce")]
    SecretNoncePublicNonceMismatch,
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
    fn adaptor_secret_is_point_checked_without_creating_a_signature() {
        let context = context([0x96; 32]);
        verify_adaptor_secret(&context, Zeroizing::new(ADAPTOR_SECRET)).unwrap();
        assert_eq!(
            verify_adaptor_secret(&context, Zeroizing::new([0x54; 32])),
            Err(AdaptorSessionError::AdaptorSecretPointMismatch)
        );
        assert_eq!(
            verify_adaptor_secret(&context, Zeroizing::new([0; 32])),
            Err(AdaptorSessionError::InvalidAdaptorSecret)
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

    #[test]
    fn persisted_nonce_rehydration_matches_the_in_memory_signer() {
        let context = context([0xa1; 32]);
        let maker_nonce = FreshAdaptorNonce::generate_with_nonce_seed(
            &context,
            SigningRole::Maker,
            MAKER_SECRET,
            [0x71; 32],
        )
        .unwrap();
        let taker_nonce = FreshAdaptorNonce::generate_with_nonce_seed(
            &context,
            SigningRole::Taker,
            TAKER_SECRET,
            [0x82; 32],
        )
        .unwrap();

        let maker_partial = sign_persisted_adaptor_partial(
            &context,
            SigningRole::Maker,
            MAKER_SECRET,
            PersistedAdaptorSigningMaterial::new(
                maker_nonce.context_binding(),
                maker_nonce.secret_nonce(),
                maker_nonce.public_nonce(),
                maker_nonce.commitment(),
                taker_nonce.commitment(),
                taker_nonce.public_nonce(),
            ),
        )
        .unwrap();
        let taker_partial = sign_persisted_adaptor_partial(
            &context,
            SigningRole::Taker,
            TAKER_SECRET,
            PersistedAdaptorSigningMaterial::new(
                taker_nonce.context_binding(),
                taker_nonce.secret_nonce(),
                taker_nonce.public_nonce(),
                taker_nonce.commitment(),
                maker_nonce.commitment(),
                maker_nonce.public_nonce(),
            ),
        )
        .unwrap();
        verify_adaptor_partial_signature(
            &context,
            SigningRole::Maker,
            maker_nonce.public_nonce(),
            taker_nonce.public_nonce(),
            maker_partial,
        )
        .unwrap();
        verify_adaptor_partial_signature(
            &context,
            SigningRole::Taker,
            maker_nonce.public_nonce(),
            taker_nonce.public_nonce(),
            taker_partial,
        )
        .unwrap();
        let persisted_presignature = aggregate_adaptor_presignature(
            &context,
            maker_nonce.public_nonce(),
            taker_nonce.public_nonce(),
            maker_partial,
            taker_partial,
        )
        .unwrap();
        verify_adaptor_presignature(&context, persisted_presignature).unwrap();

        let (mut maker, mut taker) = signers(&context).unwrap();
        maker
            .accept_peer_commitment(taker.nonce_commitment())
            .unwrap();
        taker
            .accept_peer_commitment(maker.nonce_commitment())
            .unwrap();
        assert_eq!(maker.public_nonce().unwrap(), maker_nonce.public_nonce());
        assert_eq!(taker.public_nonce().unwrap(), taker_nonce.public_nonce());
        maker.accept_peer_nonce(taker_nonce.public_nonce()).unwrap();
        taker.accept_peer_nonce(maker_nonce.public_nonce()).unwrap();
        assert_eq!(maker.create_partial_signature().unwrap(), maker_partial);
        assert_eq!(taker.create_partial_signature().unwrap(), taker_partial);
        maker.accept_peer_partial_signature(taker_partial).unwrap();
        taker.accept_peer_partial_signature(maker_partial).unwrap();
        assert_eq!(maker.presignature().unwrap(), persisted_presignature);
        assert_eq!(taker.presignature().unwrap(), persisted_presignature);
    }

    #[test]
    fn persisted_signing_rejects_changed_context_commitment_and_nonce() {
        let changed_context = context([0xa3; 32]);
        let context = context([0xa2; 32]);
        let maker_nonce = FreshAdaptorNonce::generate_with_nonce_seed(
            &context,
            SigningRole::Maker,
            MAKER_SECRET,
            [0x74; 32],
        )
        .unwrap();
        let taker_nonce = FreshAdaptorNonce::generate_with_nonce_seed(
            &context,
            SigningRole::Taker,
            TAKER_SECRET,
            [0x85; 32],
        )
        .unwrap();

        let material = || {
            PersistedAdaptorSigningMaterial::new(
                maker_nonce.context_binding(),
                maker_nonce.secret_nonce(),
                maker_nonce.public_nonce(),
                maker_nonce.commitment(),
                taker_nonce.commitment(),
                taker_nonce.public_nonce(),
            )
        };
        assert_eq!(
            sign_persisted_adaptor_partial(
                &changed_context,
                SigningRole::Maker,
                MAKER_SECRET,
                material(),
            ),
            Err(AdaptorSessionError::DurableContextMismatch)
        );
        let mut wrong_own_commitment = maker_nonce.commitment();
        wrong_own_commitment[0] ^= 1;
        assert_eq!(
            sign_persisted_adaptor_partial(
                &context,
                SigningRole::Maker,
                MAKER_SECRET,
                PersistedAdaptorSigningMaterial::new(
                    maker_nonce.context_binding(),
                    maker_nonce.secret_nonce(),
                    maker_nonce.public_nonce(),
                    wrong_own_commitment,
                    taker_nonce.commitment(),
                    taker_nonce.public_nonce(),
                ),
            ),
            Err(AdaptorSessionError::NonceCommitmentMismatch)
        );
        let mut wrong_peer_commitment = taker_nonce.commitment();
        wrong_peer_commitment[0] ^= 1;
        assert_eq!(
            sign_persisted_adaptor_partial(
                &context,
                SigningRole::Maker,
                MAKER_SECRET,
                PersistedAdaptorSigningMaterial::new(
                    maker_nonce.context_binding(),
                    maker_nonce.secret_nonce(),
                    maker_nonce.public_nonce(),
                    maker_nonce.commitment(),
                    wrong_peer_commitment,
                    taker_nonce.public_nonce(),
                ),
            ),
            Err(AdaptorSessionError::NonceCommitmentMismatch)
        );
        assert_eq!(
            sign_persisted_adaptor_partial(
                &context,
                SigningRole::Maker,
                MAKER_SECRET,
                PersistedAdaptorSigningMaterial::new(
                    maker_nonce.context_binding(),
                    maker_nonce.secret_nonce(),
                    taker_nonce.public_nonce(),
                    nonce_commitment(&context, SigningRole::Maker, &taker_nonce.public_nonce()),
                    taker_nonce.commitment(),
                    taker_nonce.public_nonce(),
                ),
            ),
            Err(AdaptorSessionError::SecretNoncePublicNonceMismatch)
        );
        assert_eq!(
            sign_persisted_adaptor_partial(&context, SigningRole::Maker, TAKER_SECRET, material(),),
            Err(AdaptorSessionError::SecretKeyRoleMismatch)
        );
        assert_eq!(
            verify_nonce_commitment(
                &context,
                SigningRole::Maker,
                taker_nonce.commitment(),
                taker_nonce.public_nonce(),
            ),
            Err(AdaptorSessionError::NonceCommitmentMismatch)
        );
    }

    #[test]
    fn partial_verification_rejects_changed_message_and_nonce_transcript() {
        let changed_context = context([0xa5; 32]);
        let context = context([0xa4; 32]);
        let maker_nonce = FreshAdaptorNonce::generate_with_nonce_seed(
            &context,
            SigningRole::Maker,
            MAKER_SECRET,
            [0x76; 32],
        )
        .unwrap();
        let taker_nonce = FreshAdaptorNonce::generate_with_nonce_seed(
            &context,
            SigningRole::Taker,
            TAKER_SECRET,
            [0x87; 32],
        )
        .unwrap();
        let maker_partial = sign_persisted_adaptor_partial(
            &context,
            SigningRole::Maker,
            MAKER_SECRET,
            PersistedAdaptorSigningMaterial::new(
                maker_nonce.context_binding(),
                maker_nonce.secret_nonce(),
                maker_nonce.public_nonce(),
                maker_nonce.commitment(),
                taker_nonce.commitment(),
                taker_nonce.public_nonce(),
            ),
        )
        .unwrap();

        assert_eq!(
            verify_adaptor_partial_signature(
                &changed_context,
                SigningRole::Maker,
                maker_nonce.public_nonce(),
                taker_nonce.public_nonce(),
                maker_partial,
            ),
            Err(AdaptorSessionError::PeerPartialVerificationFailed)
        );
        assert_eq!(
            verify_adaptor_partial_signature(
                &context,
                SigningRole::Maker,
                taker_nonce.public_nonce(),
                maker_nonce.public_nonce(),
                maker_partial,
            ),
            Err(AdaptorSessionError::PeerPartialVerificationFailed)
        );
    }

    #[test]
    fn durable_material_debug_is_secret_free_and_taproot_tweak_is_bound() {
        let context = context([0xa6; 32]);
        let nonce = FreshAdaptorNonce::generate_with_nonce_seed(
            &context,
            SigningRole::Maker,
            MAKER_SECRET,
            [0x78; 32],
        )
        .unwrap();
        let secret_debug = format!("{:?}", nonce.secret_nonce());
        let nonce_debug = format!("{nonce:?}");
        assert!(nonce_debug.contains("[REDACTED]"));
        assert!(!nonce_debug.contains(&secret_debug));

        let material = PersistedAdaptorSigningMaterial::new(
            nonce.context_binding(),
            nonce.secret_nonce(),
            nonce.public_nonce(),
            nonce.commitment(),
            nonce.commitment(),
            nonce.public_nonce(),
        );
        let material_debug = format!("{material:?}");
        assert!(material_debug.contains("[REDACTED]"));
        assert!(!material_debug.contains(&secret_debug));

        let keys = context.ordered_public_keys();
        let first = AdaptorSessionContext::taproot(
            keys,
            [0x11; 32],
            context.message(),
            context.adaptor_point(),
            context.session_id(),
        )
        .unwrap();
        let second = AdaptorSessionContext::taproot(
            keys,
            [0x12; 32],
            context.message(),
            context.adaptor_point(),
            context.session_id(),
        )
        .unwrap();
        assert_ne!(
            first.durable_context_binding(),
            second.durable_context_binding()
        );
    }

    #[test]
    fn durable_hash_bindings_remain_byte_exact() {
        let context = context([0xa7; 32]);
        let nonce = FreshAdaptorNonce::generate_with_nonce_seed(
            &context,
            SigningRole::Maker,
            MAKER_SECRET,
            [0x79; 32],
        )
        .unwrap();

        assert_eq!(
            context.durable_context_binding(),
            [
                176, 160, 124, 39, 162, 46, 212, 8, 164, 69, 43, 153, 96, 98, 198, 107, 18, 198, 7,
                19, 139, 113, 73, 232, 51, 82, 51, 58, 117, 39, 241, 209,
            ]
        );
        assert_eq!(
            nonce.commitment(),
            [
                209, 119, 78, 167, 56, 230, 102, 178, 104, 0, 55, 97, 100, 133, 131, 174, 204, 4,
                139, 145, 90, 105, 240, 242, 78, 207, 251, 53, 246, 38, 167, 187,
            ]
        );
    }
}
