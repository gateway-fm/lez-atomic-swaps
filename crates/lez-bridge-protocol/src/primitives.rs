use std::fmt;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Exact protocol schema understood by this crate.
pub const SCHEMA_VERSION: u16 = 1;
/// Additive BTC witnessed-asset terms version; v1 native RPC messages stay unchanged.
pub const WITNESSED_LEZ_ASSET_TERMS_VERSION: u16 = 2;
/// Maximum decoded transaction size accepted at the process boundary.
pub const MAX_TRANSACTION_BYTES: usize = 2_000_000;
/// Maximum account or signer entries accepted in one field.
pub const MAX_ACCOUNT_IDS: usize = 16;
/// Maximum UTF-8 byte length of an error returned over the protocol.
pub const MAX_ERROR_MESSAGE_BYTES: usize = 256;
/// Maximum blocks a single terms-based discovery request may scan.
pub const MAX_DISCOVERY_BLOCKS: u32 = 4_096;

/// Rejection reason for a bounded protocol value.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ProtocolValueError {
    /// A run or request identifier was outside its safe grammar.
    #[error("{0} must be 8..=64 ASCII characters from [A-Za-z0-9._-]")]
    InvalidIdentifier(&'static str),
    /// A fixed-width identifier was not canonical lowercase hexadecimal.
    #[error("value must be exactly 64 lowercase hexadecimal characters")]
    InvalidHex32,
    /// Exact transaction bytes were empty or exceeded the protocol maximum.
    #[error("transaction bytes must contain 1..={MAX_TRANSACTION_BYTES} bytes")]
    InvalidTransactionLength,
    /// Exact transaction bytes were not canonically base64 encoded.
    #[error("transaction bytes must be canonical base64")]
    InvalidTransactionEncoding,
    /// Exact unsigned message bytes were empty or exceeded the protocol maximum.
    #[error("message bytes must contain 1..={MAX_TRANSACTION_BYTES} bytes")]
    InvalidMessageLength,
    /// Exact unsigned message bytes were not canonically base64 encoded.
    #[error("message bytes must be canonical base64")]
    InvalidMessageEncoding,
    /// A completed aggregate signature was not exactly 64 canonical bytes.
    #[error("aggregate BIP340 signature must be exactly 128 lowercase hexadecimal characters")]
    InvalidBip340Signature,
    /// An account or signer list exceeded its hard bound.
    #[error("account list cannot contain more than {MAX_ACCOUNT_IDS} entries")]
    TooManyAccountIds,
    /// Error text exceeded its hard UTF-8 byte bound.
    #[error("error message cannot exceed {MAX_ERROR_MESSAGE_BYTES} UTF-8 bytes")]
    ErrorMessageTooLong,
    /// The escrow roles were not distinct.
    #[error("escrow depositor and claimant must be distinct")]
    SameEscrowRoles,
    /// A zero amount cannot represent a native escrow.
    #[error("native escrow amount must be nonzero")]
    ZeroEscrowAmount,
    /// A native amount was not canonical unsigned decimal or exceeded `u128`.
    #[error("native amount must be canonical unsigned decimal in the u128 range")]
    InvalidNativeAmount,
    /// The escrow actor account identities were not distinct.
    #[error("escrow depositor and claimant account ids must be distinct")]
    SameEscrowAccounts,
    /// The pinned guest rejects a zero secret digest.
    #[error("native escrow secret digest must be nonzero")]
    ZeroSecretDigest,
    /// The pinned guest rejects a zero refund timestamp.
    #[error("native escrow refund_at_ms must be nonzero")]
    ZeroRefundAt,
    /// The pinned guest rejects the default authenticated-transfer program.
    #[error("authenticated-transfer program id must be nonzero")]
    ZeroAuthenticatedTransferProgram,
    /// The witnessed claimant destination and aggregate signing authority were aliased.
    #[error("witnessed claimant destination and aggregate authority must be distinct")]
    SameWitnessedClaimAccounts,
    /// The witnessed aggregate public key was the invalid all-zero sentinel.
    #[error("witnessed aggregate x-only public key must be nonzero")]
    ZeroAggregatePublicKey,
    /// The witnessed token agreement commitment was the invalid all-zero sentinel.
    #[error("witnessed token terms hash must be nonzero")]
    ZeroWitnessedTokenTermsHash,
    /// One account or program identity in witnessed token terms was all zeroes.
    #[error("witnessed token {0} must be nonzero")]
    ZeroWitnessedTokenIdentity(&'static str),
    /// Two semantically distinct token accounts or programs used the same identity.
    #[error("witnessed token {0} and {1} must be distinct")]
    AliasedWitnessedTokenIdentities(&'static str, &'static str),
    /// The additive witnessed-asset envelope used an unsupported exact version.
    #[error(
        "unsupported witnessed LEZ asset terms version {0}; expected {WITNESSED_LEZ_ASSET_TERMS_VERSION}"
    )]
    UnsupportedWitnessedLezAssetTermsVersion(u16),
    /// A native or token preparation omitted, duplicated, or reordered an exact effect.
    #[error("witnessed asset preparation effects do not match the asset-specific order")]
    InvalidWitnessedAssetPrepareEffects,
    /// Decoded instruction, metadata, or custody facts disagreed with exact asset terms.
    #[error("witnessed asset facts mismatch: {0}")]
    WitnessedAssetFactsMismatch(&'static str),
    /// A discovery window was empty, oversized, or overflowed its height range.
    #[error("discovery window must cover 1..={MAX_DISCOVERY_BLOCKS} non-overflowing blocks")]
    InvalidDiscoveryWindow,
    /// The XMR-native agreement did not preserve the fixed Taker-to-Maker LEZ direction.
    #[error("XMR native escrow requires Taker depositor and Maker claimant roles")]
    InvalidXmrRoleMapping,
    /// One agreement-bound XMR value used an invalid all-zero sentinel.
    #[error("XMR native escrow {0} must be nonzero")]
    ZeroXmrValue(&'static str),
    /// Two semantically distinct XMR identities or commitments were aliased.
    #[error("XMR native escrow {0} and {1} must be distinct")]
    AliasedXmrValues(&'static str, &'static str),
    /// The XMR refund and punish timestamps did not form two nonempty ordered windows.
    #[error("XMR native escrow requires 0 < refund_at_ms < punish_at_ms")]
    InvalidXmrWindows,
    /// A standalone XMR terms envelope used an unsupported exact version.
    #[error("unsupported XMR native escrow terms version {0}; expected 3")]
    UnsupportedXmrNativeTermsVersion(u16),
    /// An XMR instruction or finalized evidence bundle contradicted the exact terms.
    #[error("XMR native escrow facts mismatch: {0}")]
    XmrFactsMismatch(&'static str),
}

/// The only accepted protocol version.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[must_use]
pub struct SchemaVersion;

impl SchemaVersion {
    /// Returns the current, and only, schema version.
    pub const fn current() -> Self {
        Self
    }
}

impl Serialize for SchemaVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u16(SCHEMA_VERSION)
    }
}

impl<'de> Deserialize<'de> for SchemaVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let version = u16::deserialize(deserializer)?;
        if version == SCHEMA_VERSION {
            Ok(Self)
        } else {
            Err(D::Error::custom(format_args!(
                "unsupported schema version {version}; expected {SCHEMA_VERSION}"
            )))
        }
    }
}

macro_rules! safe_identifier {
    ($name:ident, $kind:literal) => {
        #[doc = concat!("A bounded, log-safe ", $kind, ".")]
        #[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        #[must_use]
        pub struct $name(String);

        impl $name {
            #[doc = concat!("Validates and constructs a ", $kind, ".")]
            ///
            /// # Errors
            ///
            /// Returns an error unless the value is 8..=64 bytes in the safe ASCII grammar.
            pub fn new(value: impl Into<String>) -> Result<Self, ProtocolValueError> {
                let value = value.into();
                if (8..=64).contains(&value.len())
                    && value.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                    })
                {
                    Ok(Self(value))
                } else {
                    Err(ProtocolValueError::InvalidIdentifier($kind))
                }
            }

            #[doc = concat!("Borrows the validated ", $kind, ".")]
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = ProtocolValueError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

safe_identifier!(RunId, "run id");
safe_identifier!(RequestId, "request id");

/// Fixed participant role assigned to one sidecar process.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[must_use]
pub enum Participant {
    /// The maker actor and its signer.
    Maker,
    /// The taker actor and its signer.
    Taker,
}

/// Versioned identity carried by every request and response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct MessageContext {
    /// Exact schema version.
    pub schema_version: SchemaVersion,
    /// Isolation identifier shared by the composed run.
    pub run_id: RunId,
    /// Idempotency and correlation identifier for one request.
    pub request_id: RequestId,
    /// Participant whose dedicated sidecar receives the message.
    pub sidecar_role: Participant,
}

impl MessageContext {
    /// Creates a context at the current schema version.
    pub const fn new(run_id: RunId, request_id: RequestId, sidecar_role: Participant) -> Self {
        Self {
            schema_version: SchemaVersion::current(),
            run_id,
            request_id,
            sidecar_role,
        }
    }
}

/// A primitive 32-byte identifier serialized as canonical lowercase hex.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
#[must_use]
pub struct Hex32([u8; 32]);

impl Hex32 {
    /// Constructs an identifier from its exact bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parses exactly 64 lowercase hexadecimal characters.
    ///
    /// # Errors
    ///
    /// Returns an error if the value has the wrong width or is not lowercase hex.
    pub fn from_hex(value: &str) -> Result<Self, ProtocolValueError> {
        decode_hex32(value).map(Self)
    }

    /// Returns the exact identifier bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    fn encode_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Debug for Hex32 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Hex32")
            .field(&self.encode_hex())
            .finish()
    }
}

impl Serialize for Hex32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.encode_hex())
    }
}

impl<'de> Deserialize<'de> for Hex32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(D::Error::custom)
    }
}

/// Official-decoder transaction identity, without importing an official type.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
#[must_use]
pub struct TransactionId(Hex32);

impl TransactionId {
    /// Constructs an ID from official-decoder hash bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Hex32::from_bytes(bytes))
    }

    /// Returns the exact ID bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }
}

/// Nonempty inner official `PublicTransaction::to_bytes()` with a 2 MB hard limit.
///
/// This wrapper must never contain an outer runtime transaction-enum encoding.
#[derive(Clone, Eq, PartialEq)]
#[must_use]
pub struct ExactTransactionBytes(Vec<u8>);

impl ExactTransactionBytes {
    /// Validates exact inner `PublicTransaction::to_bytes()` at the protocol boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when the byte sequence is empty or exceeds 2 MB.
    pub fn new(bytes: Vec<u8>) -> Result<Self, ProtocolValueError> {
        if bytes.is_empty() || bytes.len() > MAX_TRANSACTION_BYTES {
            Err(ProtocolValueError::InvalidTransactionLength)
        } else {
            Ok(Self(bytes))
        }
    }

    /// Borrows inner public-transaction bytes that must be decoded or submitted unchanged.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Consumes the wrapper and returns the exact bytes.
    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }
}

impl fmt::Debug for ExactTransactionBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExactTransactionBytes")
            .field("len", &self.0.len())
            .finish()
    }
}

impl Serialize for ExactTransactionBytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for ExactTransactionBytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let maximum_encoded_len = MAX_TRANSACTION_BYTES.div_ceil(3) * 4;
        if encoded.len() > maximum_encoded_len {
            return Err(D::Error::custom(
                ProtocolValueError::InvalidTransactionLength,
            ));
        }
        let bytes = STANDARD
            .decode(&encoded)
            .map_err(|_| D::Error::custom(ProtocolValueError::InvalidTransactionEncoding))?;
        if STANDARD.encode(&bytes) != encoded {
            return Err(D::Error::custom(
                ProtocolValueError::InvalidTransactionEncoding,
            ));
        }
        Self::new(bytes).map_err(D::Error::custom)
    }
}

/// Canonical Borsh bytes of one unsigned official LEZ public message.
///
/// This is deliberately distinct from [`ExactTransactionBytes`]: an external
/// signer signs the official message hash, while submission accepts only the
/// completed public-transaction encoding.
#[derive(Clone, Eq, PartialEq)]
#[must_use]
pub struct ExactMessageBytes(Vec<u8>);

impl ExactMessageBytes {
    /// Validates canonical unsigned-message bytes at the protocol boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when the byte sequence is empty or exceeds 2 MB.
    pub fn new(bytes: Vec<u8>) -> Result<Self, ProtocolValueError> {
        if bytes.is_empty() || bytes.len() > MAX_TRANSACTION_BYTES {
            Err(ProtocolValueError::InvalidMessageLength)
        } else {
            Ok(Self(bytes))
        }
    }

    /// Borrows the exact bytes that must be hashed and decoded unchanged.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for ExactMessageBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExactMessageBytes")
            .field("len", &self.0.len())
            .finish()
    }
}

impl Serialize for ExactMessageBytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for ExactMessageBytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let maximum_encoded_len = MAX_TRANSACTION_BYTES.div_ceil(3) * 4;
        if encoded.len() > maximum_encoded_len {
            return Err(D::Error::custom(ProtocolValueError::InvalidMessageLength));
        }
        let bytes = STANDARD
            .decode(&encoded)
            .map_err(|_| D::Error::custom(ProtocolValueError::InvalidMessageEncoding))?;
        if STANDARD.encode(&bytes) != encoded {
            return Err(D::Error::custom(ProtocolValueError::InvalidMessageEncoding));
        }
        Self::new(bytes).map_err(D::Error::custom)
    }
}

/// One externally completed 64-byte aggregate BIP340 signature.
#[derive(Clone, Copy, Eq, PartialEq)]
#[must_use]
pub struct AggregateBip340Signature([u8; 64]);

impl AggregateBip340Signature {
    /// Wraps exact aggregate signature bytes. The pinned official LEZ runtime
    /// validates completion, and the bridge client independently verifies a
    /// signature before accepting its finalized observation.
    pub const fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    /// Returns exact aggregate signature bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }
}

impl fmt::Debug for AggregateBip340Signature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AggregateBip340Signature")
            .field("len", &self.0.len())
            .finish()
    }
}

impl Serialize for AggregateBip340Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for AggregateBip340Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        if encoded.len() != 128
            || !encoded
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(D::Error::custom(ProtocolValueError::InvalidBip340Signature));
        }
        let mut bytes = [0_u8; 64];
        hex::decode_to_slice(&encoded, &mut bytes)
            .map_err(|_| D::Error::custom(ProtocolValueError::InvalidBip340Signature))?;
        Ok(Self(bytes))
    }
}

/// Ordered account or signer identifiers with a fixed 16-entry maximum.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<Hex32>", into = "Vec<Hex32>")]
#[must_use]
pub struct AccountIds(Vec<Hex32>);

impl AccountIds {
    /// Validates an ordered account or signer list.
    ///
    /// # Errors
    ///
    /// Returns an error when the list contains more than 16 identifiers.
    pub fn new(ids: Vec<Hex32>) -> Result<Self, ProtocolValueError> {
        if ids.len() > MAX_ACCOUNT_IDS {
            Err(ProtocolValueError::TooManyAccountIds)
        } else {
            Ok(Self(ids))
        }
    }

    /// Borrows the ordered identifiers.
    pub fn as_slice(&self) -> &[Hex32] {
        &self.0
    }
}

impl TryFrom<Vec<Hex32>> for AccountIds {
    type Error = ProtocolValueError;

    fn try_from(value: Vec<Hex32>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<AccountIds> for Vec<Hex32> {
    fn from(value: AccountIds) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryWindowWire {
    start_height: u64,
    max_blocks: u32,
}

/// Explicit bounded canonical scan range for terms-based discovery.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "DiscoveryWindowWire", into = "DiscoveryWindowWire")]
#[must_use]
pub struct DiscoveryWindow(DiscoveryWindowWire);

impl DiscoveryWindow {
    /// Validates a bounded inclusive scan beginning at `start_height`.
    ///
    /// # Errors
    ///
    /// Returns an error for zero, oversized, or height-overflowing ranges.
    pub fn new(start_height: u64, max_blocks: u32) -> Result<Self, ProtocolValueError> {
        if max_blocks == 0
            || max_blocks > MAX_DISCOVERY_BLOCKS
            || start_height
                .checked_add(u64::from(max_blocks.saturating_sub(1)))
                .is_none()
        {
            return Err(ProtocolValueError::InvalidDiscoveryWindow);
        }
        Ok(Self(DiscoveryWindowWire {
            start_height,
            max_blocks,
        }))
    }

    /// Returns the first height in the declared canonical scan.
    #[must_use]
    pub const fn start_height(self) -> u64 {
        self.0.start_height
    }

    /// Returns the maximum number of blocks in the declared scan.
    #[must_use]
    pub const fn max_blocks(self) -> u32 {
        self.0.max_blocks
    }
}

impl TryFrom<DiscoveryWindowWire> for DiscoveryWindow {
    type Error = ProtocolValueError;

    fn try_from(value: DiscoveryWindowWire) -> Result<Self, Self::Error> {
        Self::new(value.start_height, value.max_blocks)
    }
}

impl From<DiscoveryWindow> for DiscoveryWindowWire {
    fn from(value: DiscoveryWindow) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeEscrowTermsWire {
    swap_id: Hex32,
    terms_hash: Hex32,
    secret_digest: Hex32,
    depositor: Participant,
    depositor_account_id: Hex32,
    claimant: Participant,
    claimant_account_id: Hex32,
    amount: NativeAmount,
    refund_at_ms: u64,
    authenticated_transfer_program_id: Hex32,
}

/// Full-width native value serialized as a canonical decimal string.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[must_use]
pub struct NativeAmount(u128);

impl NativeAmount {
    /// Wraps a full-width native value.
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    /// Returns the full-width native value.
    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0
    }
}

impl Serialize for NativeAmount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for NativeAmount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        if encoded.is_empty()
            || (encoded.len() > 1 && encoded.starts_with('0'))
            || !encoded.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(D::Error::custom(ProtocolValueError::InvalidNativeAmount));
        }
        encoded
            .parse::<u128>()
            .map(Self)
            .map_err(|_| D::Error::custom(ProtocolValueError::InvalidNativeAmount))
    }
}

/// Complete primitive input required by the pinned native guest initializer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct NativeEscrowTermsInput {
    /// Swap identifier used by the metadata and custody PDA seeds.
    ///
    /// This is the SDK's raw domain-separated SHA-256 output. The SDK does not
    /// remap or reserve the all-zero digest, so this boundary preserves it.
    pub swap_id: Hex32,
    /// Exact signed agreement commitment passed to the guest as `terms_hash`.
    ///
    /// This is the SDK's raw domain-separated SHA-256 output. The SDK does not
    /// remap or reserve the all-zero digest, so this boundary preserves it.
    pub terms_hash: Hex32,
    /// SHA-256 digest checked by the revealing claim instruction.
    pub secret_digest: Hex32,
    /// Signed agreement role assigned to the depositor account.
    pub depositor: Participant,
    /// Exact native depositor account identity.
    pub depositor_account_id: Hex32,
    /// Signed agreement role assigned to the claimant account.
    pub claimant: Participant,
    /// Exact native claimant account identity.
    pub claimant_account_id: Hex32,
    /// Full-width native amount.
    pub amount: u128,
    /// LEZ wall-clock refund timestamp in Unix milliseconds.
    pub refund_at_ms: u64,
    /// Exact authenticated-transfer program identity used by native accounts.
    pub authenticated_transfer_program_id: Hex32,
}

/// Primitive native-currency escrow values signed by both actors.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "NativeEscrowTermsWire", into = "NativeEscrowTermsWire")]
#[must_use]
pub struct NativeEscrowTerms(NativeEscrowTermsWire);

impl NativeEscrowTerms {
    /// Validates native escrow terms.
    ///
    /// # Errors
    ///
    /// Returns an error when actor bindings differ from valid native guest terms.
    pub fn new(input: NativeEscrowTermsInput) -> Result<Self, ProtocolValueError> {
        if input.depositor == input.claimant {
            return Err(ProtocolValueError::SameEscrowRoles);
        }
        if input.depositor_account_id == input.claimant_account_id {
            return Err(ProtocolValueError::SameEscrowAccounts);
        }
        if input.amount == 0 {
            return Err(ProtocolValueError::ZeroEscrowAmount);
        }
        if input.secret_digest == Hex32::from_bytes([0; 32]) {
            return Err(ProtocolValueError::ZeroSecretDigest);
        }
        if input.refund_at_ms == 0 {
            return Err(ProtocolValueError::ZeroRefundAt);
        }
        if input.authenticated_transfer_program_id == Hex32::from_bytes([0; 32]) {
            return Err(ProtocolValueError::ZeroAuthenticatedTransferProgram);
        }
        Ok(Self(NativeEscrowTermsWire {
            swap_id: input.swap_id,
            terms_hash: input.terms_hash,
            secret_digest: input.secret_digest,
            depositor: input.depositor,
            depositor_account_id: input.depositor_account_id,
            claimant: input.claimant,
            claimant_account_id: input.claimant_account_id,
            amount: NativeAmount::new(input.amount),
            refund_at_ms: input.refund_at_ms,
            authenticated_transfer_program_id: input.authenticated_transfer_program_id,
        }))
    }

    /// Returns the swap identifier.
    pub const fn swap_id(&self) -> Hex32 {
        self.0.swap_id
    }

    /// Returns the exact signed agreement commitment passed as `terms_hash`.
    pub const fn terms_hash(&self) -> Hex32 {
        self.0.terms_hash
    }

    /// Returns the exact secret digest checked by the guest.
    pub const fn secret_digest(&self) -> Hex32 {
        self.0.secret_digest
    }

    /// Returns the escrow depositor.
    pub const fn depositor(&self) -> Participant {
        self.0.depositor
    }

    /// Returns the exact depositor account identity.
    pub const fn depositor_account_id(&self) -> Hex32 {
        self.0.depositor_account_id
    }

    /// Returns the escrow claimant.
    pub const fn claimant(&self) -> Participant {
        self.0.claimant
    }

    /// Returns the exact claimant account identity.
    pub const fn claimant_account_id(&self) -> Hex32 {
        self.0.claimant_account_id
    }

    /// Returns the native amount.
    pub const fn amount(&self) -> NativeAmount {
        self.0.amount
    }

    /// Returns the LEZ wall-clock refund timestamp in Unix milliseconds.
    #[must_use]
    pub const fn refund_at_ms(&self) -> u64 {
        self.0.refund_at_ms
    }

    /// Returns the exact authenticated-transfer program identity.
    pub const fn authenticated_transfer_program_id(&self) -> Hex32 {
        self.0.authenticated_transfer_program_id
    }
}

impl TryFrom<NativeEscrowTermsWire> for NativeEscrowTerms {
    type Error = ProtocolValueError;

    fn try_from(value: NativeEscrowTermsWire) -> Result<Self, Self::Error> {
        Self::new(NativeEscrowTermsInput {
            swap_id: value.swap_id,
            terms_hash: value.terms_hash,
            secret_digest: value.secret_digest,
            depositor: value.depositor,
            depositor_account_id: value.depositor_account_id,
            claimant: value.claimant,
            claimant_account_id: value.claimant_account_id,
            amount: value.amount.as_u128(),
            refund_at_ms: value.refund_at_ms,
            authenticated_transfer_program_id: value.authenticated_transfer_program_id,
        })
    }
}

impl From<NativeEscrowTerms> for NativeEscrowTermsWire {
    fn from(value: NativeEscrowTerms) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WitnessedNativeEscrowTermsWire {
    swap_id: Hex32,
    terms_hash: Hex32,
    depositor: Participant,
    depositor_account_id: Hex32,
    claimant: Participant,
    claimant_account_id: Hex32,
    aggregate_authority_account_id: Hex32,
    aggregate_x_only_public_key: Hex32,
    amount: NativeAmount,
    refund_at_ms: u64,
    authenticated_transfer_program_id: Hex32,
}

/// Complete signed-agreement input for an aggregate-witness native escrow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct WitnessedNativeEscrowTermsInput {
    /// Swap identifier used by the metadata and custody PDA seeds.
    pub swap_id: Hex32,
    /// Exact signed agreement commitment passed to the guest.
    pub terms_hash: Hex32,
    /// Role depositing the LEZ asset.
    pub depositor: Participant,
    /// Exact depositor asset account.
    pub depositor_account_id: Hex32,
    /// Role receiving the LEZ asset.
    pub claimant: Participant,
    /// Exact claimant asset destination, which never supplies the claim signature.
    pub claimant_account_id: Hex32,
    /// Aggregate public account that alone supplies the claim signature.
    pub aggregate_authority_account_id: Hex32,
    /// Exact aggregate x-only BIP340 key whose account ID must match the authority.
    pub aggregate_x_only_public_key: Hex32,
    /// Full-width native amount.
    pub amount: u128,
    /// LEZ wall-clock refund timestamp in Unix milliseconds.
    pub refund_at_ms: u64,
    /// Exact authenticated-transfer program identity.
    pub authenticated_transfer_program_id: Hex32,
}

/// Primitive witnessed native-escrow values signed by both swap actors.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    try_from = "WitnessedNativeEscrowTermsWire",
    into = "WitnessedNativeEscrowTermsWire"
)]
#[must_use]
pub struct WitnessedNativeEscrowTerms(WitnessedNativeEscrowTermsWire);

impl WitnessedNativeEscrowTerms {
    /// Validates witnessed native escrow terms before they cross the sidecar boundary.
    ///
    /// # Errors
    ///
    /// Rejects aliased roles/accounts, zero amount/refund/program, and a zero
    /// aggregate key. The pinned official LEZ key implementation performs the
    /// curve and authority-account checks inside the sidecar.
    pub fn new(input: WitnessedNativeEscrowTermsInput) -> Result<Self, ProtocolValueError> {
        if input.depositor == input.claimant {
            return Err(ProtocolValueError::SameEscrowRoles);
        }
        if input.depositor_account_id == input.claimant_account_id {
            return Err(ProtocolValueError::SameEscrowAccounts);
        }
        if input.claimant_account_id == input.aggregate_authority_account_id
            || input.depositor_account_id == input.aggregate_authority_account_id
        {
            return Err(ProtocolValueError::SameWitnessedClaimAccounts);
        }
        if input.aggregate_x_only_public_key == Hex32::from_bytes([0; 32]) {
            return Err(ProtocolValueError::ZeroAggregatePublicKey);
        }
        if input.amount == 0 {
            return Err(ProtocolValueError::ZeroEscrowAmount);
        }
        if input.refund_at_ms == 0 {
            return Err(ProtocolValueError::ZeroRefundAt);
        }
        if input.authenticated_transfer_program_id == Hex32::from_bytes([0; 32]) {
            return Err(ProtocolValueError::ZeroAuthenticatedTransferProgram);
        }
        Ok(Self(WitnessedNativeEscrowTermsWire {
            swap_id: input.swap_id,
            terms_hash: input.terms_hash,
            depositor: input.depositor,
            depositor_account_id: input.depositor_account_id,
            claimant: input.claimant,
            claimant_account_id: input.claimant_account_id,
            aggregate_authority_account_id: input.aggregate_authority_account_id,
            aggregate_x_only_public_key: input.aggregate_x_only_public_key,
            amount: NativeAmount::new(input.amount),
            refund_at_ms: input.refund_at_ms,
            authenticated_transfer_program_id: input.authenticated_transfer_program_id,
        }))
    }

    pub const fn swap_id(&self) -> Hex32 {
        self.0.swap_id
    }

    pub const fn terms_hash(&self) -> Hex32 {
        self.0.terms_hash
    }

    pub const fn depositor(&self) -> Participant {
        self.0.depositor
    }

    pub const fn depositor_account_id(&self) -> Hex32 {
        self.0.depositor_account_id
    }

    pub const fn claimant(&self) -> Participant {
        self.0.claimant
    }

    pub const fn claimant_account_id(&self) -> Hex32 {
        self.0.claimant_account_id
    }

    pub const fn aggregate_authority_account_id(&self) -> Hex32 {
        self.0.aggregate_authority_account_id
    }

    pub const fn aggregate_x_only_public_key(&self) -> Hex32 {
        self.0.aggregate_x_only_public_key
    }

    pub const fn amount(&self) -> NativeAmount {
        self.0.amount
    }

    #[must_use]
    pub const fn refund_at_ms(&self) -> u64 {
        self.0.refund_at_ms
    }

    pub const fn authenticated_transfer_program_id(&self) -> Hex32 {
        self.0.authenticated_transfer_program_id
    }
}

impl TryFrom<WitnessedNativeEscrowTermsWire> for WitnessedNativeEscrowTerms {
    type Error = ProtocolValueError;

    fn try_from(value: WitnessedNativeEscrowTermsWire) -> Result<Self, Self::Error> {
        Self::new(WitnessedNativeEscrowTermsInput {
            swap_id: value.swap_id,
            terms_hash: value.terms_hash,
            depositor: value.depositor,
            depositor_account_id: value.depositor_account_id,
            claimant: value.claimant,
            claimant_account_id: value.claimant_account_id,
            aggregate_authority_account_id: value.aggregate_authority_account_id,
            aggregate_x_only_public_key: value.aggregate_x_only_public_key,
            amount: value.amount.as_u128(),
            refund_at_ms: value.refund_at_ms,
            authenticated_transfer_program_id: value.authenticated_transfer_program_id,
        })
    }
}

impl From<WitnessedNativeEscrowTerms> for WitnessedNativeEscrowTermsWire {
    fn from(value: WitnessedNativeEscrowTerms) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WitnessedTokenEscrowTermsV2Wire {
    swap_id: Hex32,
    terms_hash: Hex32,
    depositor: Participant,
    depositor_owner_account_id: Hex32,
    depositor_ata_account_id: Hex32,
    claimant: Participant,
    claimant_owner_account_id: Hex32,
    claimant_ata_account_id: Hex32,
    custody_ata_account_id: Hex32,
    token_program_id: Hex32,
    ata_program_id: Hex32,
    token_definition_account_id: Hex32,
    aggregate_authority_account_id: Hex32,
    aggregate_x_only_public_key: Hex32,
    amount: NativeAmount,
    refund_at_ms: u64,
}

/// Complete input for one aggregate-witness custom-token escrow.
///
/// The protocol binds the exact official account identities but deliberately
/// does not reimplement ATA derivation or the LEZ aggregate-key mapping. The
/// official sidecar must rederive and compare both before preparing bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct WitnessedTokenEscrowTermsV2Input {
    /// Swap identifier used by the metadata PDA seed.
    pub swap_id: Hex32,
    /// Exact nonzero countersigned agreement commitment stored by the guest.
    pub terms_hash: Hex32,
    /// Role depositing the custom token.
    pub depositor: Participant,
    /// Exact depositor owner that signs initialization and funding.
    pub depositor_owner_account_id: Hex32,
    /// Exact depositor ATA for the selected definition.
    pub depositor_ata_account_id: Hex32,
    /// Role receiving the custom token.
    pub claimant: Participant,
    /// Immutable claimant owner; it is not the witnessed claim signer.
    pub claimant_owner_account_id: Hex32,
    /// Exact immutable claimant ATA for the selected definition.
    pub claimant_ata_account_id: Hex32,
    /// Exact custody `ATA(metadata, definition)`.
    pub custody_ata_account_id: Hex32,
    /// Token program that owns the definition and all three token holdings.
    pub token_program_id: Hex32,
    /// Official ATA program that derives custody and owner holdings.
    pub ata_program_id: Hex32,
    /// Exact fungible token definition account.
    pub token_definition_account_id: Hex32,
    /// Aggregate public account that alone signs the witnessed claim.
    pub aggregate_authority_account_id: Hex32,
    /// Exact aggregate x-only BIP340 key mapped to the authority account.
    pub aggregate_x_only_public_key: Hex32,
    /// Full-width custom-token amount.
    pub amount: u128,
    /// LEZ wall-clock refund timestamp in Unix milliseconds.
    pub refund_at_ms: u64,
}

/// Strict v2 wire terms for one aggregate-witness custom-token escrow.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    try_from = "WitnessedTokenEscrowTermsV2Wire",
    into = "WitnessedTokenEscrowTermsV2Wire"
)]
#[must_use]
pub struct WitnessedTokenEscrowTermsV2(WitnessedTokenEscrowTermsV2Wire);

impl WitnessedTokenEscrowTermsV2 {
    /// Validates exact custom-token terms before sidecar or chain I/O.
    ///
    /// # Errors
    ///
    /// Rejects same-role terms, zero values required nonzero by the guest, and
    /// every alias among programs, definition, owners, ATAs, custody, and
    /// aggregate authority. Official ATA/key derivation remains a sidecar duty.
    pub fn new(input: WitnessedTokenEscrowTermsV2Input) -> Result<Self, ProtocolValueError> {
        if input.depositor == input.claimant {
            return Err(ProtocolValueError::SameEscrowRoles);
        }
        if input.terms_hash == Hex32::from_bytes([0; 32]) {
            return Err(ProtocolValueError::ZeroWitnessedTokenTermsHash);
        }
        if input.aggregate_x_only_public_key == Hex32::from_bytes([0; 32]) {
            return Err(ProtocolValueError::ZeroAggregatePublicKey);
        }
        if input.amount == 0 {
            return Err(ProtocolValueError::ZeroEscrowAmount);
        }
        if input.refund_at_ms == 0 {
            return Err(ProtocolValueError::ZeroRefundAt);
        }

        let identities = [
            ("token program id", input.token_program_id),
            ("ATA program id", input.ata_program_id),
            (
                "token definition account id",
                input.token_definition_account_id,
            ),
            (
                "depositor owner account id",
                input.depositor_owner_account_id,
            ),
            ("depositor ATA account id", input.depositor_ata_account_id),
            ("claimant owner account id", input.claimant_owner_account_id),
            ("claimant ATA account id", input.claimant_ata_account_id),
            ("custody ATA account id", input.custody_ata_account_id),
            (
                "aggregate authority account id",
                input.aggregate_authority_account_id,
            ),
        ];
        for (index, (name, identity)) in identities.iter().copied().enumerate() {
            if identity == Hex32::from_bytes([0; 32]) {
                return Err(ProtocolValueError::ZeroWitnessedTokenIdentity(name));
            }
            for (other_name, other_identity) in identities.iter().copied().skip(index + 1) {
                if identity == other_identity {
                    return Err(ProtocolValueError::AliasedWitnessedTokenIdentities(
                        name, other_name,
                    ));
                }
            }
        }

        Ok(Self(WitnessedTokenEscrowTermsV2Wire {
            swap_id: input.swap_id,
            terms_hash: input.terms_hash,
            depositor: input.depositor,
            depositor_owner_account_id: input.depositor_owner_account_id,
            depositor_ata_account_id: input.depositor_ata_account_id,
            claimant: input.claimant,
            claimant_owner_account_id: input.claimant_owner_account_id,
            claimant_ata_account_id: input.claimant_ata_account_id,
            custody_ata_account_id: input.custody_ata_account_id,
            token_program_id: input.token_program_id,
            ata_program_id: input.ata_program_id,
            token_definition_account_id: input.token_definition_account_id,
            aggregate_authority_account_id: input.aggregate_authority_account_id,
            aggregate_x_only_public_key: input.aggregate_x_only_public_key,
            amount: NativeAmount::new(input.amount),
            refund_at_ms: input.refund_at_ms,
        }))
    }

    /// Returns the swap identifier.
    pub const fn swap_id(&self) -> Hex32 {
        self.0.swap_id
    }

    /// Returns the exact countersigned agreement commitment.
    pub const fn terms_hash(&self) -> Hex32 {
        self.0.terms_hash
    }

    /// Returns the depositor role.
    pub const fn depositor(&self) -> Participant {
        self.0.depositor
    }

    /// Returns the exact depositor owner.
    pub const fn depositor_owner_account_id(&self) -> Hex32 {
        self.0.depositor_owner_account_id
    }

    /// Returns the exact depositor ATA.
    pub const fn depositor_ata_account_id(&self) -> Hex32 {
        self.0.depositor_ata_account_id
    }

    /// Returns the claimant role.
    pub const fn claimant(&self) -> Participant {
        self.0.claimant
    }

    /// Returns the exact immutable claimant owner.
    pub const fn claimant_owner_account_id(&self) -> Hex32 {
        self.0.claimant_owner_account_id
    }

    /// Returns the exact immutable claimant ATA.
    pub const fn claimant_ata_account_id(&self) -> Hex32 {
        self.0.claimant_ata_account_id
    }

    /// Returns the exact metadata-owned custody ATA.
    pub const fn custody_ata_account_id(&self) -> Hex32 {
        self.0.custody_ata_account_id
    }

    /// Returns the exact token program.
    pub const fn token_program_id(&self) -> Hex32 {
        self.0.token_program_id
    }

    /// Returns the exact ATA program.
    pub const fn ata_program_id(&self) -> Hex32 {
        self.0.ata_program_id
    }

    /// Returns the exact fungible definition account.
    pub const fn token_definition_account_id(&self) -> Hex32 {
        self.0.token_definition_account_id
    }

    /// Returns the aggregate witnessed-claim authority account.
    pub const fn aggregate_authority_account_id(&self) -> Hex32 {
        self.0.aggregate_authority_account_id
    }

    /// Returns the exact aggregate x-only BIP340 key.
    pub const fn aggregate_x_only_public_key(&self) -> Hex32 {
        self.0.aggregate_x_only_public_key
    }

    /// Returns the custom-token amount.
    pub const fn amount(&self) -> NativeAmount {
        self.0.amount
    }

    /// Returns the LEZ refund timestamp in Unix milliseconds.
    #[must_use]
    pub const fn refund_at_ms(&self) -> u64 {
        self.0.refund_at_ms
    }
}

impl TryFrom<WitnessedTokenEscrowTermsV2Wire> for WitnessedTokenEscrowTermsV2 {
    type Error = ProtocolValueError;

    fn try_from(value: WitnessedTokenEscrowTermsV2Wire) -> Result<Self, Self::Error> {
        Self::new(WitnessedTokenEscrowTermsV2Input {
            swap_id: value.swap_id,
            terms_hash: value.terms_hash,
            depositor: value.depositor,
            depositor_owner_account_id: value.depositor_owner_account_id,
            depositor_ata_account_id: value.depositor_ata_account_id,
            claimant: value.claimant,
            claimant_owner_account_id: value.claimant_owner_account_id,
            claimant_ata_account_id: value.claimant_ata_account_id,
            custody_ata_account_id: value.custody_ata_account_id,
            token_program_id: value.token_program_id,
            ata_program_id: value.ata_program_id,
            token_definition_account_id: value.token_definition_account_id,
            aggregate_authority_account_id: value.aggregate_authority_account_id,
            aggregate_x_only_public_key: value.aggregate_x_only_public_key,
            amount: value.amount.as_u128(),
            refund_at_ms: value.refund_at_ms,
        })
    }
}

impl From<WitnessedTokenEscrowTermsV2> for WitnessedTokenEscrowTermsV2Wire {
    fn from(value: WitnessedTokenEscrowTermsV2) -> Self {
        value.0
    }
}

/// Native or custom-token LEZ terms selected by the additive v2 envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "terms", rename_all = "snake_case")]
#[must_use]
pub enum WitnessedLezAssetV2 {
    /// Existing witnessed-native terms, byte-for-byte unchanged inside the envelope.
    Native(WitnessedNativeEscrowTerms),
    /// Exact witnessed custom-token owner, ATA, definition, program, and authority terms.
    CustomToken(WitnessedTokenEscrowTermsV2),
}

impl WitnessedLezAssetV2 {
    /// Returns existing native terms only for the native variant.
    #[must_use]
    pub const fn native(&self) -> Option<&WitnessedNativeEscrowTerms> {
        match self {
            Self::Native(terms) => Some(terms),
            Self::CustomToken(_) => None,
        }
    }

    /// Returns custom-token terms only for the custom-token variant.
    #[must_use]
    pub const fn custom_token(&self) -> Option<&WitnessedTokenEscrowTermsV2> {
        match self {
            Self::Native(_) => None,
            Self::CustomToken(terms) => Some(terms),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum NativeAssetKindV2 {
    Native,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum CustomTokenAssetKindV2 {
    CustomToken,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeAssetV2Wire {
    kind: NativeAssetKindV2,
    terms: WitnessedNativeEscrowTerms,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CustomTokenAssetV2Wire {
    kind: CustomTokenAssetKindV2,
    terms: WitnessedTokenEscrowTermsV2,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WitnessedLezAssetV2Wire {
    Native(NativeAssetV2Wire),
    CustomToken(CustomTokenAssetV2Wire),
}

impl<'de> Deserialize<'de> for WitnessedLezAssetV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match WitnessedLezAssetV2Wire::deserialize(deserializer)? {
            WitnessedLezAssetV2Wire::Native(wire) => {
                let NativeAssetKindV2::Native = wire.kind;
                Ok(Self::Native(wire.terms))
            }
            WitnessedLezAssetV2Wire::CustomToken(wire) => {
                let CustomTokenAssetKindV2::CustomToken = wire.kind;
                Ok(Self::CustomToken(wire.terms))
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WitnessedLezAssetTermsV2Wire {
    asset_terms_version: u16,
    asset: WitnessedLezAssetV2,
}

/// Explicitly versioned additive BTC witnessed-LEZ asset terms envelope.
///
/// This model does not replace `WitnessedNativeEscrowTerms` or any existing
/// `lez_bridge.v1.*` request. It gives later v2 RPC methods one unambiguous
/// native-or-token boundary while v1 JSON stays byte-for-byte compatible.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    try_from = "WitnessedLezAssetTermsV2Wire",
    into = "WitnessedLezAssetTermsV2Wire"
)]
#[must_use]
pub struct WitnessedLezAssetTermsV2(WitnessedLezAssetTermsV2Wire);

impl WitnessedLezAssetTermsV2 {
    /// Wraps established witnessed-native terms without changing their inner JSON.
    pub const fn native(terms: WitnessedNativeEscrowTerms) -> Self {
        Self(WitnessedLezAssetTermsV2Wire {
            asset_terms_version: WITNESSED_LEZ_ASSET_TERMS_VERSION,
            asset: WitnessedLezAssetV2::Native(terms),
        })
    }

    /// Wraps strict witnessed custom-token terms.
    pub const fn custom_token(terms: WitnessedTokenEscrowTermsV2) -> Self {
        Self(WitnessedLezAssetTermsV2Wire {
            asset_terms_version: WITNESSED_LEZ_ASSET_TERMS_VERSION,
            asset: WitnessedLezAssetV2::CustomToken(terms),
        })
    }

    /// Returns the exact additive terms version.
    #[must_use]
    pub const fn asset_terms_version(&self) -> u16 {
        self.0.asset_terms_version
    }

    /// Borrows the exact native or custom-token selection.
    pub const fn asset(&self) -> &WitnessedLezAssetV2 {
        &self.0.asset
    }
}

impl TryFrom<WitnessedLezAssetTermsV2Wire> for WitnessedLezAssetTermsV2 {
    type Error = ProtocolValueError;

    fn try_from(value: WitnessedLezAssetTermsV2Wire) -> Result<Self, Self::Error> {
        if value.asset_terms_version != WITNESSED_LEZ_ASSET_TERMS_VERSION {
            return Err(
                ProtocolValueError::UnsupportedWitnessedLezAssetTermsVersion(
                    value.asset_terms_version,
                ),
            );
        }
        Ok(Self(value))
    }
}

impl From<WitnessedLezAssetTermsV2> for WitnessedLezAssetTermsV2Wire {
    fn from(value: WitnessedLezAssetTermsV2) -> Self {
        value.0
    }
}

/// Revealed 32-byte preimage. Debug output never includes secret bytes.
#[derive(Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
#[must_use]
pub struct RevealingPreimage([u8; 32]);

impl RevealingPreimage {
    /// Wraps the exact preimage bytes.
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Explicitly exposes the preimage for official transaction construction.
    #[must_use]
    pub const fn expose_secret(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for RevealingPreimage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RevealingPreimage([REDACTED])")
    }
}

impl Serialize for RevealingPreimage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for RevealingPreimage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        decode_hex32(&value).map(Self).map_err(D::Error::custom)
    }
}

/// Primitive tip returned before or after an observation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ChainTip {
    /// Node-reported block hash; upstream `BlockId` itself is numeric.
    pub block_hash: Hex32,
    /// Node-reported height.
    pub height: u64,
}

/// Primitive canonical chain clock including the consensus-visible millisecond timestamp.
///
/// This is additive rather than changing [`ChainTip`], preserving its existing
/// wire shape and users while refund eligibility obtains the clock domain it needs.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ChainClock {
    /// Node-reported block hash; the official block hash commits the timestamp.
    pub block_hash: Hex32,
    /// Node-reported block height.
    pub height: u64,
    /// Consensus-visible Unix timestamp in milliseconds.
    pub timestamp_ms: u64,
}

impl ChainClock {
    /// Creates primitive canonical clock facts.
    pub const fn new(block_hash: Hex32, height: u64, timestamp_ms: u64) -> Self {
        Self {
            block_hash,
            height,
            timestamp_ms,
        }
    }
}

impl ChainTip {
    /// Creates primitive tip facts.
    pub const fn new(block_hash: Hex32, height: u64) -> Self {
        Self { block_hash, height }
    }
}

/// Primitive transaction placement returned by the node scan.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ChainPosition {
    /// Block containing the transaction according to the node response.
    pub block_hash: Hex32,
    /// Reported block height.
    pub height: u64,
    /// Transaction index in the reported block.
    pub transaction_index: u32,
}

impl ChainPosition {
    /// Creates primitive placement facts.
    pub const fn new(block_hash: Hex32, height: u64, transaction_index: u32) -> Self {
        Self {
            block_hash,
            height,
            transaction_index,
        }
    }
}

/// Stable error categories that callers may branch on.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[must_use]
pub enum ErrorCode {
    /// Request failed schema or value validation.
    InvalidRequest,
    /// The request was sent to the wrong participant sidecar.
    WrongSidecarRole,
    /// The official node or decoder rejected the transaction.
    InvalidTransaction,
    /// A required node fact could not be obtained.
    Unavailable,
    /// Terms-based discovery found more than one matching transaction.
    AmbiguousDiscovery,
    /// A unique terms match conflicted with the expected signed transcript.
    ConflictingDiscovery,
    /// The before and after node tips differed.
    MovingTip,
    /// Submission may have reached the node, but its outcome is unknown.
    UnknownSubmissionOutcome,
    /// An internal bounded sidecar operation failed.
    Internal,
}

/// Bounded human-readable protocol error text.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
#[must_use]
pub struct ErrorMessage(String);

impl ErrorMessage {
    /// Validates an error message's UTF-8 byte length.
    ///
    /// # Errors
    ///
    /// Returns an error when the message exceeds 256 UTF-8 bytes.
    pub fn new(value: impl Into<String>) -> Result<Self, ProtocolValueError> {
        let value = value.into();
        if value.len() <= MAX_ERROR_MESSAGE_BYTES {
            Ok(Self(value))
        } else {
            Err(ProtocolValueError::ErrorMessageTooLong)
        }
    }

    /// Borrows the error text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ErrorMessage {
    type Error = ProtocolValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ErrorMessage> for String {
    fn from(value: ErrorMessage) -> Self {
        value.0
    }
}

fn decode_hex32(value: &str) -> Result<[u8; 32], ProtocolValueError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ProtocolValueError::InvalidHex32);
    }
    let mut bytes = [0_u8; 32];
    hex::decode_to_slice(value, &mut bytes).map_err(|_| ProtocolValueError::InvalidHex32)?;
    Ok(bytes)
}
