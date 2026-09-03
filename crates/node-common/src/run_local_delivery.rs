//! Authenticated, bounded run-local adapter for the Delivery discovery port.

use std::{
    fs::{self, DirBuilder, File},
    io::{self, Write as _},
    os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use lez_swap_sdk_core::OfferDiscovery;
use lez_swap_store::{
    MakerOfferError, MakerOfferRecordV1, MakerOfferStatus, MakerOfferV1, MakerRouteV1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey, ecdsa::Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tempfile::NamedTempFile;
use thiserror::Error;

const DELIVERY_OFFER_SCHEMA_V1: u16 = 1;
const DELIVERY_OFFER_DOMAIN_V1: &[u8] = b"lez-atomic-swaps/run-local-delivery/offer/v1";
const LOGOS_OFFER_ANNOUNCEMENT_SCHEMA_V1: u16 = 1;
const LOGOS_OFFER_ANNOUNCEMENT_DOMAIN_V1: &[u8] =
    b"lez-atomic-swaps/logos-delivery/offer-announcement/v1";
const MAXIMUM_ENVELOPE_BYTES: u64 = 65_536;
// Keep the canonical bytes small enough that standard Base64 plus JSON framing
// always fits one 48 KiB snapshot page on the 64 KiB owner-local RPC.
#[doc(hidden)]
pub const MAXIMUM_LOGOS_OFFER_ANNOUNCEMENT_BYTES: usize = 32 * 1024;
#[doc(hidden)]
pub const MAXIMUM_LOGOS_OFFER_ANNOUNCEMENT_BASE64_BYTES: usize =
    4 * MAXIMUM_LOGOS_OFFER_ANNOUNCEMENT_BYTES.div_ceil(3);
const MAXIMUM_CHAT_ADDRESS_BYTES: usize = 1024;
const MAXIMUM_ANNOUNCEMENT_CLOCK_SKEW_SECONDS: u64 = 5;
const MAXIMUM_DISCOVERY_ENTRIES: usize = 1_024;
const OFFER_FILE_SUFFIX: &str = ".offer.json";

/// Exact LIP-23-style content topic used by the Basecamp offer index.
pub const LOGOS_OFFER_CONTENT_TOPIC_V1: &str = "/lez-atomic-swaps/1/offers/json";
/// Maker refresh interval; live announcements expire if three refreshes are missed.
pub const LOGOS_OFFER_REBROADCAST_SECONDS_V1: u64 = 10;
/// Signed app-lifetime availability lease carried by each announcement.
pub const LOGOS_OFFER_ANNOUNCEMENT_TTL_SECONDS_V1: u64 = 30;

/// One store-produced offer plus the publisher's trusted local timestamp.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeliveryPublicationV1 {
    offer: MakerOfferV1,
    now_unix_seconds: u64,
}

impl DeliveryPublicationV1 {
    /// Creates one publication request.
    #[must_use]
    pub const fn new(offer: MakerOfferV1, now_unix_seconds: u64) -> Self {
        Self {
            offer,
            now_unix_seconds,
        }
    }
}

/// Read-only route and trusted-time filter used by a discovery client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeliveryOfferQueryV1 {
    route: Option<MakerRouteV1>,
    now_unix_seconds: u64,
}

impl DeliveryOfferQueryV1 {
    /// Queries all supported routes at one trusted local timestamp.
    #[must_use]
    pub const fn all(now_unix_seconds: u64) -> Self {
        Self {
            route: None,
            now_unix_seconds,
        }
    }

    /// Queries one exact pair/direction route at one trusted local timestamp.
    #[must_use]
    pub const fn for_route(route: MakerRouteV1, now_unix_seconds: u64) -> Self {
        Self {
            route: Some(route),
            now_unix_seconds,
        }
    }
}

/// Authenticated immutable offer returned to a taker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedOfferRefV1 {
    offer: MakerOfferV1,
    maker_identity: [u8; 33],
    signed_envelope: Vec<u8>,
}

impl AuthenticatedOfferRefV1 {
    /// Verified immutable offer terms.
    #[must_use]
    pub const fn offer(&self) -> &MakerOfferV1 {
        &self.offer
    }

    /// Compressed secp256k1 maker identity that authenticated the offer.
    #[must_use]
    pub const fn maker_identity(&self) -> &[u8; 33] {
        &self.maker_identity
    }

    /// Exact bounded signed envelope retained for Chat negotiation binding.
    #[must_use]
    pub fn signed_envelope(&self) -> &[u8] {
        &self.signed_envelope
    }

    /// Commitment used to bind a later countersigned pair agreement.
    #[must_use]
    pub fn commitment(&self) -> [u8; 32] {
        Sha256::digest(&self.signed_envelope).into()
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SignedOfferEnvelopeV1 {
    schema_version: u16,
    maker_public_key: Vec<u8>,
    offer_json: Vec<u8>,
    signature: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LogosOfferAnnouncementBodyV1 {
    schema_version: u16,
    maker_public_key: Vec<u8>,
    maker_chat_address: Box<str>,
    offer_revision: u64,
    status: MakerOfferStatus,
    announced_at_unix_seconds: u64,
    valid_until_unix_seconds: u64,
    signed_offer_envelope: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SignedLogosOfferAnnouncementV1 {
    #[serde(flatten)]
    body: LogosOfferAnnouncementBodyV1,
    signature: Vec<u8>,
}

/// Authenticated Delivery announcement retained by the Taker app-lifetime index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedLogosOfferAnnouncementV1 {
    offer: AuthenticatedOfferRefV1,
    maker_chat_address: Box<str>,
    offer_revision: u64,
    status: MakerOfferStatus,
    announced_at_unix_seconds: u64,
    valid_until_unix_seconds: u64,
    encoded: Vec<u8>,
}

impl AuthenticatedLogosOfferAnnouncementV1 {
    /// Exact immutable offer and its original signed offer envelope.
    #[must_use]
    pub const fn offer(&self) -> &AuthenticatedOfferRefV1 {
        &self.offer
    }

    /// Current app-lifetime direct Chat address signed by the offer Maker.
    #[must_use]
    pub const fn maker_chat_address(&self) -> &str {
        &self.maker_chat_address
    }

    /// Monotonic durable offer revision advertised by the Maker store.
    #[must_use]
    pub const fn offer_revision(&self) -> u64 {
        self.offer_revision
    }

    /// Advisory lifecycle projection; the Maker store remains authoritative.
    #[must_use]
    pub const fn status(&self) -> MakerOfferStatus {
        self.status
    }

    /// Maker timestamp covered by the signature.
    #[must_use]
    pub const fn announced_at_unix_seconds(&self) -> u64 {
        self.announced_at_unix_seconds
    }

    /// Exclusive local-index lease boundary covered by the signature.
    #[must_use]
    pub const fn valid_until_unix_seconds(&self) -> u64 {
        self.valid_until_unix_seconds
    }

    /// Exact canonical announcement bytes received from Delivery.
    #[must_use]
    pub fn encoded(&self) -> &[u8] {
        &self.encoded
    }
}

/// Failure at the owner-private run-local Delivery boundary.
#[derive(Debug, Error)]
pub enum RunLocalDeliveryError {
    /// The adapter directory is missing, shared, symlinked, or owned by another user.
    #[error("Delivery directory must be an owner-owned non-symlink directory with mode 0700")]
    InsecureDirectory,
    /// This adapter instance has no maker signing capability.
    #[error("Delivery adapter is discovery-only")]
    DiscoveryOnly,
    /// The offer is malformed, not yet active, or already expired.
    #[error("Delivery offer is invalid at the trusted publication time")]
    InvalidOffer,
    /// An immutable advertisement with this offer ID already exists.
    #[error("Delivery offer already exists")]
    AlreadyExists,
    /// A file with the same ID authenticates different immutable terms.
    #[error("Delivery offer ID already authenticates different terms")]
    ConflictingOffer,
    /// The removable projection does not exactly match the durable retryable-offer set.
    #[error("Delivery projection differs from the durable retryable-offer set")]
    ProjectionDrift,
    /// The directory exceeded its explicit discovery bound.
    #[error("Delivery directory exceeds the {MAXIMUM_DISCOVERY_ENTRIES}-entry bound")]
    TooManyEntries,
    /// An envelope exceeded its explicit wire/storage bound.
    #[error("Delivery envelope exceeds the {MAXIMUM_ENVELOPE_BYTES}-byte bound")]
    OversizedEnvelope,
    /// A live Delivery announcement exceeded its explicit wire bound.
    #[error(
        "Logos Delivery announcement exceeds the {MAXIMUM_LOGOS_OFFER_ANNOUNCEMENT_BYTES}-byte bound"
    )]
    OversizedAnnouncement,
    /// An envelope was malformed, non-canonical, or used an unsupported schema.
    #[error("Delivery envelope is malformed or unsupported")]
    MalformedEnvelope,
    /// The configured maker identity did not authenticate the exact offer bytes.
    #[error("Delivery offer authentication failed")]
    Authentication,
    /// Filesystem persistence or discovery failed.
    #[error("Delivery filesystem operation failed")]
    Io(#[source] io::Error),
    /// Offer validation rejected untrusted bytes.
    #[error("Delivery offer snapshot validation failed")]
    Offer(#[source] MakerOfferError),
}

impl From<io::Error> for RunLocalDeliveryError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<MakerOfferError> for RunLocalDeliveryError {
    fn from(error: MakerOfferError) -> Self {
        Self::Offer(error)
    }
}

/// Filesystem-backed Delivery-compatible adapter for isolated local runs.
///
/// The publisher holds the maker key; a taker instance holds only the expected
/// compressed public identity. The shared directory is transport, not protocol
/// state, and can be removed after negotiation.
pub struct RunLocalDelivery {
    directory: PathBuf,
    expected_maker: PublicKey,
    signing_key: Option<SecretKey>,
}

impl RunLocalDelivery {
    /// The Delivery directory this transport reads or writes.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

impl std::fmt::Debug for RunLocalDelivery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunLocalDelivery")
            .field("directory", &self.directory)
            .field("expected_maker", &self.expected_maker)
            .field("can_publish", &self.signing_key.is_some())
            .finish()
    }
}

impl RunLocalDelivery {
    /// Creates or opens an owner-private publisher directory.
    ///
    /// # Errors
    ///
    /// Rejects insecure directory ownership, permissions, type, or symlinks.
    pub fn publisher(
        directory: impl Into<PathBuf>,
        signing_key: SecretKey,
    ) -> Result<Self, RunLocalDeliveryError> {
        let directory = directory.into();
        ensure_private_directory(&directory)?;
        let expected_maker = PublicKey::from_secret_key(&Secp256k1::signing_only(), &signing_key);
        Ok(Self {
            directory,
            expected_maker,
            signing_key: Some(signing_key),
        })
    }

    /// Opens an owner-private discovery directory without maker signing authority.
    ///
    /// # Errors
    ///
    /// Rejects insecure directory ownership, permissions, type, or symlinks.
    pub fn subscriber(
        directory: impl Into<PathBuf>,
        expected_maker: PublicKey,
    ) -> Result<Self, RunLocalDeliveryError> {
        let directory = directory.into();
        validate_private_directory(&directory)?;
        Ok(Self {
            directory,
            expected_maker,
            signing_key: None,
        })
    }

    /// Authenticates exact bounded envelope bytes against this adapter's pinned maker key.
    ///
    /// This does not apply route or time filtering; a Chat caller must cross-bind
    /// the returned immutable offer to its trusted local time and selected route.
    ///
    /// # Errors
    ///
    /// Rejects oversized, malformed, noncanonical, wrongly keyed, wrongly signed,
    /// or invalid offer bytes.
    pub fn authenticate_envelope(
        &self,
        encoded: &[u8],
    ) -> Result<AuthenticatedOfferRefV1, RunLocalDeliveryError> {
        if encoded.len() as u64 > MAXIMUM_ENVELOPE_BYTES {
            return Err(RunLocalDeliveryError::OversizedEnvelope);
        }
        verify_envelope(encoded, &self.expected_maker)
    }

    /// Signs one canonical, short-lived Logos Delivery announcement from the
    /// durable offer record and binds it to the Maker app's current Chat address.
    ///
    /// The nested offer envelope remains the immutable agreement commitment;
    /// the outer signature prevents substitution of Chat address, revision, or
    /// availability state. Reservation and swap identifiers are never exposed.
    ///
    /// # Errors
    ///
    /// Rejects discovery-only instances, malformed records or addresses, time
    /// overflow, and oversized/noncanonical signed output.
    pub fn sign_logos_offer_announcement(
        &self,
        record: &MakerOfferRecordV1,
        maker_chat_address: &str,
        now_unix_seconds: u64,
    ) -> Result<Vec<u8>, RunLocalDeliveryError> {
        let signing_key = self
            .signing_key
            .as_ref()
            .ok_or(RunLocalDeliveryError::DiscoveryOnly)?;
        validate_chat_address(maker_chat_address)?;
        record.offer().validate()?;
        if record.revision() == 0 || now_unix_seconds > i64::MAX as u64 {
            return Err(RunLocalDeliveryError::InvalidOffer);
        }
        let mut valid_until_unix_seconds = now_unix_seconds
            .checked_add(LOGOS_OFFER_ANNOUNCEMENT_TTL_SECONDS_V1)
            .ok_or(RunLocalDeliveryError::InvalidOffer)?;
        if record.status() == MakerOfferStatus::Active {
            if now_unix_seconds < record.offer().created_at_unix_seconds()
                || now_unix_seconds >= record.offer().expires_at_unix_seconds()
            {
                return Err(RunLocalDeliveryError::InvalidOffer);
            }
            valid_until_unix_seconds =
                valid_until_unix_seconds.min(record.offer().expires_at_unix_seconds());
        }
        let signed_offer_envelope =
            signed_offer_envelope(record.offer(), &self.expected_maker, signing_key)?;
        let body = LogosOfferAnnouncementBodyV1 {
            schema_version: LOGOS_OFFER_ANNOUNCEMENT_SCHEMA_V1,
            maker_public_key: self.expected_maker.serialize().to_vec(),
            maker_chat_address: maker_chat_address.into(),
            offer_revision: record.revision(),
            status: record.status(),
            announced_at_unix_seconds: now_unix_seconds,
            valid_until_unix_seconds,
            signed_offer_envelope,
        };
        let signature = Secp256k1::signing_only()
            .sign_ecdsa(
                &Message::from_digest(logos_offer_announcement_digest(&body)?),
                signing_key,
            )
            .serialize_compact();
        let envelope = SignedLogosOfferAnnouncementV1 {
            body,
            signature: signature.to_vec(),
        };
        let encoded =
            serde_json::to_vec(&envelope).map_err(|_| RunLocalDeliveryError::MalformedEnvelope)?;
        if encoded.len() > MAXIMUM_LOGOS_OFFER_ANNOUNCEMENT_BYTES {
            return Err(RunLocalDeliveryError::OversizedAnnouncement);
        }
        Ok(encoded)
    }

    /// Publishes an offer or verifies that a prior crash/restart published the exact same offer.
    ///
    /// # Errors
    ///
    /// Rejects a conflicting existing advertisement, invalid terms, insecure storage, or I/O.
    pub fn publish_or_verify(
        &self,
        publication: &DeliveryPublicationV1,
    ) -> Result<AuthenticatedOfferRefV1, RunLocalDeliveryError> {
        match self.publish_sync(publication) {
            Ok(authenticated) => Ok(authenticated),
            Err(RunLocalDeliveryError::AlreadyExists) => {
                let path = offer_path(&self.directory, publication.offer.id().as_str());
                let encoded = read_regular_offer_file(&path)?;
                let authenticated = verify_envelope(&encoded, &self.expected_maker)?;
                if authenticated.offer() != &publication.offer {
                    return Err(RunLocalDeliveryError::ConflictingOffer);
                }
                Ok(authenticated)
            }
            Err(error) => Err(error),
        }
    }

    /// Removes one advertisement after the durable offer has been withdrawn.
    ///
    /// Missing files are idempotent so an exact RPC replay can finish cleanup.
    ///
    /// # Errors
    ///
    /// Rejects discovery-only instances, mismatched/tampered files, insecure storage, or I/O.
    pub fn withdraw(
        &self,
        offer_id: &lez_swap_store::MakerOfferId,
    ) -> Result<(), RunLocalDeliveryError> {
        validate_private_directory(&self.directory)?;
        if self.signing_key.is_none() {
            return Err(RunLocalDeliveryError::DiscoveryOnly);
        }
        let path = offer_path(&self.directory, offer_id.as_str());
        let encoded = match read_regular_offer_file(&path) {
            Ok(encoded) => encoded,
            Err(RunLocalDeliveryError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let authenticated = verify_envelope(&encoded, &self.expected_maker)?;
        if authenticated.offer().id() != offer_id {
            return Err(RunLocalDeliveryError::ConflictingOffer);
        }
        fs::remove_file(path)?;
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }

    /// Verifies the removable mailbox against the exact durable retryable-offer set.
    ///
    /// This read-only check never repairs, signs, removes, or creates a projection.
    /// It is suitable for dependency health reporting while `SQLite` remains the
    /// authoritative application state.
    ///
    /// # Errors
    ///
    /// Rejects insecure or missing storage, invalid/tampered files, unexpected
    /// advertisements, missing retryable offers, excess entries, or I/O.
    pub fn projection_health(
        &self,
        active: &[MakerOfferV1],
        now_unix_seconds: u64,
    ) -> Result<(), RunLocalDeliveryError> {
        validate_private_directory(&self.directory)?;
        let mut projected_entries = 0_usize;
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(OFFER_FILE_SUFFIX))
            {
                projected_entries += 1;
                if projected_entries > MAXIMUM_DISCOVERY_ENTRIES {
                    return Err(RunLocalDeliveryError::TooManyEntries);
                }
            }
        }
        if projected_entries != active.len() {
            return Err(RunLocalDeliveryError::ProjectionDrift);
        }
        for expected in active {
            expected.validate()?;
            if now_unix_seconds < expected.created_at_unix_seconds()
                || now_unix_seconds >= expected.expires_at_unix_seconds()
            {
                return Err(RunLocalDeliveryError::ProjectionDrift);
            }
            let encoded =
                read_regular_offer_file(&offer_path(&self.directory, expected.id().as_str()))?;
            let authenticated = verify_envelope(&encoded, &self.expected_maker)?;
            if authenticated.offer() != expected {
                return Err(RunLocalDeliveryError::ProjectionDrift);
            }
        }
        Ok(())
    }

    /// Reconciles the removable mailbox to the caller's exact retained-offer set.
    ///
    /// Exact retained files are kept and missing retained files are republished;
    /// authenticated files absent from `active` are removed. Any malformed or
    /// wrong-key entry fails startup closed instead of being silently deleted.
    ///
    /// # Errors
    ///
    /// Rejects invalid active offers, conflicting/tampered files, insecure
    /// storage, discovery-only instances, excess entries, or I/O.
    pub fn reconcile(
        &self,
        active: &[MakerOfferV1],
        now_unix_seconds: u64,
    ) -> Result<(), RunLocalDeliveryError> {
        validate_private_directory(&self.directory)?;
        if self.signing_key.is_none() {
            return Err(RunLocalDeliveryError::DiscoveryOnly);
        }
        for offer in active {
            self.publish_or_verify(&DeliveryPublicationV1::new(offer.clone(), now_unix_seconds))?;
        }

        let mut paths = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(OFFER_FILE_SUFFIX))
            {
                paths.push(path);
                if paths.len() > MAXIMUM_DISCOVERY_ENTRIES {
                    return Err(RunLocalDeliveryError::TooManyEntries);
                }
            }
        }
        let mut removed = false;
        for path in paths {
            let authenticated =
                verify_envelope(&read_regular_offer_file(&path)?, &self.expected_maker)?;
            if !active
                .iter()
                .any(|offer| offer.id() == authenticated.offer().id())
            {
                fs::remove_file(path)?;
                removed = true;
            }
        }
        if removed {
            File::open(&self.directory)?.sync_all()?;
        }
        Ok(())
    }

    fn publish_sync(
        &self,
        publication: &DeliveryPublicationV1,
    ) -> Result<AuthenticatedOfferRefV1, RunLocalDeliveryError> {
        validate_private_directory(&self.directory)?;
        let signing_key = self
            .signing_key
            .as_ref()
            .ok_or(RunLocalDeliveryError::DiscoveryOnly)?;
        publication.offer.validate()?;
        if publication.now_unix_seconds < publication.offer.created_at_unix_seconds()
            || publication.now_unix_seconds >= publication.offer.expires_at_unix_seconds()
        {
            return Err(RunLocalDeliveryError::InvalidOffer);
        }

        let encoded = signed_offer_envelope(&publication.offer, &self.expected_maker, signing_key)?;
        let destination = offer_path(&self.directory, publication.offer.id().as_str());
        if destination.exists() {
            return Err(RunLocalDeliveryError::AlreadyExists);
        }
        let mut staged = NamedTempFile::new_in(&self.directory)?;
        staged.as_file_mut().set_len(0)?;
        staged.write_all(&encoded)?;
        staged.as_file_mut().sync_all()?;
        staged
            .persist_noclobber(&destination)
            .map_err(|error| match error.error.kind() {
                io::ErrorKind::AlreadyExists => RunLocalDeliveryError::AlreadyExists,
                _ => RunLocalDeliveryError::Io(error.error),
            })?;
        File::open(&self.directory)?.sync_all()?;
        verify_envelope(&encoded, &self.expected_maker)
    }

    fn discover_sync(
        &self,
        query: &DeliveryOfferQueryV1,
    ) -> Result<Vec<AuthenticatedOfferRefV1>, RunLocalDeliveryError> {
        validate_private_directory(&self.directory)?;
        let mut paths = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let path = entry.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(OFFER_FILE_SUFFIX))
            {
                paths.push(path);
                if paths.len() > MAXIMUM_DISCOVERY_ENTRIES {
                    return Err(RunLocalDeliveryError::TooManyEntries);
                }
            }
        }
        paths.sort_unstable();
        let mut offers = Vec::new();
        for path in paths {
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.file_type().is_file() {
                return Err(RunLocalDeliveryError::MalformedEnvelope);
            }
            if metadata.len() > MAXIMUM_ENVELOPE_BYTES {
                return Err(RunLocalDeliveryError::OversizedEnvelope);
            }
            let encoded = fs::read(path)?;
            if encoded.len() as u64 > MAXIMUM_ENVELOPE_BYTES {
                return Err(RunLocalDeliveryError::OversizedEnvelope);
            }
            let authenticated = verify_envelope(&encoded, &self.expected_maker)?;
            let offer = authenticated.offer();
            if offer.created_at_unix_seconds() <= query.now_unix_seconds
                && query.now_unix_seconds < offer.expires_at_unix_seconds()
                && query.route.is_none_or(|route| route == offer.route())
            {
                offers.push(authenticated);
            }
        }
        Ok(offers)
    }
}

#[async_trait]
impl OfferDiscovery for RunLocalDelivery {
    type Error = RunLocalDeliveryError;
    type Offer = DeliveryPublicationV1;
    type OfferRef = AuthenticatedOfferRefV1;
    type Query = DeliveryOfferQueryV1;

    async fn publish(&self, offer: Self::Offer) -> Result<Self::OfferRef, Self::Error> {
        self.publish_sync(&offer)
    }

    async fn discover(&self, query: &Self::Query) -> Result<Vec<Self::OfferRef>, Self::Error> {
        self.discover_sync(query)
    }
}

/// Verifies one exact canonical Delivery announcement at trusted local time.
///
/// Maker identities are self-authenticating at discovery time: the outer key
/// must authenticate both the live Chat binding and the nested immutable offer
/// envelope. The same identity and signed-offer digest are then reviewed and
/// bound into negotiation, so Delivery routing never becomes protocol authority.
///
/// # Errors
///
/// Rejects malformed, noncanonical, oversized, expired, far-future, wrongly
/// signed, address-substituted, or internally inconsistent announcements.
pub fn verify_logos_offer_announcement(
    encoded: &[u8],
    now_unix_seconds: u64,
) -> Result<AuthenticatedLogosOfferAnnouncementV1, RunLocalDeliveryError> {
    if encoded.len() > MAXIMUM_LOGOS_OFFER_ANNOUNCEMENT_BYTES {
        return Err(RunLocalDeliveryError::OversizedAnnouncement);
    }
    let envelope: SignedLogosOfferAnnouncementV1 =
        serde_json::from_slice(encoded).map_err(|_| RunLocalDeliveryError::MalformedEnvelope)?;
    if serde_json::to_vec(&envelope).map_err(|_| RunLocalDeliveryError::MalformedEnvelope)?
        != encoded
        || envelope.body.schema_version != LOGOS_OFFER_ANNOUNCEMENT_SCHEMA_V1
        || envelope.body.offer_revision == 0
    {
        return Err(RunLocalDeliveryError::MalformedEnvelope);
    }
    validate_chat_address(&envelope.body.maker_chat_address)?;
    let maximum_future = now_unix_seconds
        .checked_add(MAXIMUM_ANNOUNCEMENT_CLOCK_SKEW_SECONDS)
        .ok_or(RunLocalDeliveryError::InvalidOffer)?;
    if envelope.body.announced_at_unix_seconds > maximum_future
        || envelope.body.valid_until_unix_seconds <= now_unix_seconds
        || envelope.body.valid_until_unix_seconds <= envelope.body.announced_at_unix_seconds
        || envelope.body.valid_until_unix_seconds - envelope.body.announced_at_unix_seconds
            > LOGOS_OFFER_ANNOUNCEMENT_TTL_SECONDS_V1
    {
        return Err(RunLocalDeliveryError::InvalidOffer);
    }
    if envelope.body.maker_public_key.len() != 33 {
        return Err(RunLocalDeliveryError::Authentication);
    }
    let public_key = PublicKey::from_slice(&envelope.body.maker_public_key)
        .map_err(|_| RunLocalDeliveryError::Authentication)?;
    let signature = Signature::from_compact(&envelope.signature)
        .map_err(|_| RunLocalDeliveryError::Authentication)?;
    let mut normalized = signature;
    normalized.normalize_s();
    if normalized != signature {
        return Err(RunLocalDeliveryError::Authentication);
    }
    Secp256k1::verification_only()
        .verify_ecdsa(
            &Message::from_digest(logos_offer_announcement_digest(&envelope.body)?),
            &signature,
            &public_key,
        )
        .map_err(|_| RunLocalDeliveryError::Authentication)?;
    let offer = verify_envelope(&envelope.body.signed_offer_envelope, &public_key)?;
    if envelope.body.status == MakerOfferStatus::Active
        && (offer.offer().created_at_unix_seconds() > now_unix_seconds
            || now_unix_seconds >= offer.offer().expires_at_unix_seconds()
            || envelope.body.valid_until_unix_seconds > offer.offer().expires_at_unix_seconds())
    {
        return Err(RunLocalDeliveryError::InvalidOffer);
    }
    Ok(AuthenticatedLogosOfferAnnouncementV1 {
        offer,
        maker_chat_address: envelope.body.maker_chat_address,
        offer_revision: envelope.body.offer_revision,
        status: envelope.body.status,
        announced_at_unix_seconds: envelope.body.announced_at_unix_seconds,
        valid_until_unix_seconds: envelope.body.valid_until_unix_seconds,
        encoded: encoded.to_vec(),
    })
}

fn signed_offer_envelope(
    offer: &MakerOfferV1,
    maker: &PublicKey,
    signing_key: &SecretKey,
) -> Result<Vec<u8>, RunLocalDeliveryError> {
    offer.validate()?;
    let offer_json =
        serde_json::to_vec(offer).map_err(|_| RunLocalDeliveryError::MalformedEnvelope)?;
    let maker_public_key = maker.serialize();
    let signature = Secp256k1::signing_only()
        .sign_ecdsa(
            &Message::from_digest(offer_digest(&maker_public_key, &offer_json)),
            signing_key,
        )
        .serialize_compact();
    let envelope = SignedOfferEnvelopeV1 {
        schema_version: DELIVERY_OFFER_SCHEMA_V1,
        maker_public_key: maker_public_key.to_vec(),
        offer_json,
        signature: signature.to_vec(),
    };
    let encoded =
        serde_json::to_vec(&envelope).map_err(|_| RunLocalDeliveryError::MalformedEnvelope)?;
    if encoded.len() as u64 > MAXIMUM_ENVELOPE_BYTES {
        return Err(RunLocalDeliveryError::OversizedEnvelope);
    }
    Ok(encoded)
}

fn logos_offer_announcement_digest(
    body: &LogosOfferAnnouncementBodyV1,
) -> Result<[u8; 32], RunLocalDeliveryError> {
    let encoded = serde_json::to_vec(body).map_err(|_| RunLocalDeliveryError::MalformedEnvelope)?;
    let mut hasher = Sha256::new();
    hasher.update(LOGOS_OFFER_ANNOUNCEMENT_DOMAIN_V1);
    hasher.update(LOGOS_OFFER_ANNOUNCEMENT_SCHEMA_V1.to_be_bytes());
    hasher.update((encoded.len() as u64).to_be_bytes());
    hasher.update(encoded);
    Ok(hasher.finalize().into())
}

fn validate_chat_address(value: &str) -> Result<(), RunLocalDeliveryError> {
    if value.is_empty()
        || value.len() > MAXIMUM_CHAT_ADDRESS_BYTES
        || value.chars().any(char::is_control)
    {
        Err(RunLocalDeliveryError::MalformedEnvelope)
    } else {
        Ok(())
    }
}

fn verify_envelope(
    encoded: &[u8],
    expected_maker: &PublicKey,
) -> Result<AuthenticatedOfferRefV1, RunLocalDeliveryError> {
    let envelope: SignedOfferEnvelopeV1 =
        serde_json::from_slice(encoded).map_err(|_| RunLocalDeliveryError::MalformedEnvelope)?;
    if serde_json::to_vec(&envelope).map_err(|_| RunLocalDeliveryError::MalformedEnvelope)?
        != encoded
        || envelope.schema_version != DELIVERY_OFFER_SCHEMA_V1
        || envelope.maker_public_key.as_slice() != expected_maker.serialize()
    {
        return Err(RunLocalDeliveryError::Authentication);
    }
    let public_key = PublicKey::from_slice(&envelope.maker_public_key)
        .map_err(|_| RunLocalDeliveryError::Authentication)?;
    let signature = Signature::from_compact(&envelope.signature)
        .map_err(|_| RunLocalDeliveryError::Authentication)?;
    let mut normalized = signature;
    normalized.normalize_s();
    if normalized != signature {
        return Err(RunLocalDeliveryError::Authentication);
    }
    let digest = offer_digest(&public_key.serialize(), &envelope.offer_json);
    Secp256k1::verification_only()
        .verify_ecdsa(&Message::from_digest(digest), &signature, &public_key)
        .map_err(|_| RunLocalDeliveryError::Authentication)?;
    let offer: MakerOfferV1 = serde_json::from_slice(&envelope.offer_json)
        .map_err(|_| RunLocalDeliveryError::MalformedEnvelope)?;
    offer.validate()?;
    if serde_json::to_vec(&offer).map_err(|_| RunLocalDeliveryError::MalformedEnvelope)?
        != envelope.offer_json
    {
        return Err(RunLocalDeliveryError::MalformedEnvelope);
    }
    Ok(AuthenticatedOfferRefV1 {
        offer,
        maker_identity: public_key.serialize(),
        signed_envelope: encoded.to_vec(),
    })
}

fn offer_digest(maker_public_key: &[u8; 33], offer_json: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(DELIVERY_OFFER_DOMAIN_V1);
    hasher.update(DELIVERY_OFFER_SCHEMA_V1.to_be_bytes());
    hasher.update(maker_public_key);
    hasher.update((offer_json.len() as u64).to_be_bytes());
    hasher.update(offer_json);
    hasher.finalize().into()
}

fn offer_path(directory: &Path, offer_id: &str) -> PathBuf {
    directory.join(format!("{offer_id}{OFFER_FILE_SUFFIX}"))
}

fn read_regular_offer_file(path: &Path) -> Result<Vec<u8>, RunLocalDeliveryError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() != 1
    {
        return Err(RunLocalDeliveryError::MalformedEnvelope);
    }
    if metadata.len() > MAXIMUM_ENVELOPE_BYTES {
        return Err(RunLocalDeliveryError::OversizedEnvelope);
    }
    let encoded = fs::read(path)?;
    if encoded.len() as u64 > MAXIMUM_ENVELOPE_BYTES {
        return Err(RunLocalDeliveryError::OversizedEnvelope);
    }
    Ok(encoded)
}

fn ensure_private_directory(directory: &Path) -> Result<(), RunLocalDeliveryError> {
    match fs::symlink_metadata(directory) {
        Ok(_) => validate_private_directory(directory),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            DirBuilder::new().mode(0o700).create(directory)?;
            validate_private_directory(directory)
        }
        Err(error) => Err(error.into()),
    }
}

fn validate_private_directory(directory: &Path) -> Result<(), RunLocalDeliveryError> {
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.file_type().is_dir()
        || metadata.permissions().mode() & 0o7777 != 0o700
        || metadata.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(RunLocalDeliveryError::InsecureDirectory);
    }
    Ok(())
}
