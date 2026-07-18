//! Canonical durable lifecycle records and exact compare-and-swap storage.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use lez_swap_core::{Participant, SwapId};
use lez_swap_sdk_core::ExactPublicEffectPlanV1;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::*;

/// Maximum encoded version-one lifecycle record size.
pub const MAX_BTC_LIFECYCLE_RECORD_BYTES: usize = 32 * 1024 * 1024;
const BTC_LIFECYCLE_RECORD_SCHEMA_V1: u16 = 1;

/// A canonical, validated, secret-free lifecycle record for durable storage.
///
/// It contains only countersigned agreement bytes, public presignatures,
/// already signed public effects, and canonical public observations. Adaptor
/// scalars, private keys, nonces, seeds, and peer capabilities are excluded.
#[derive(Clone, Eq, PartialEq)]
#[must_use]
pub struct BtcLifecycleRecordV1 {
    exact_bytes: Box<[u8]>,
    swap_id: SwapId,
    local_participant: Participant,
    revision: u64,
    sha256: [u8; 32],
}

impl std::fmt::Debug for BtcLifecycleRecordV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BtcLifecycleRecordV1")
            .field("exact_bytes", &"[REDACTED]")
            .field("swap_id", &self.swap_id)
            .field("local_participant", &self.local_participant)
            .field("revision", &self.revision)
            .field("sha256", &self.sha256)
            .finish()
    }
}

impl BtcLifecycleRecordV1 {
    /// Canonically encodes and independently revalidates an active lifecycle.
    ///
    /// # Errors
    ///
    /// Returns a codec or protocol error for unrepresentable state.
    pub fn from_active(active: &ActiveBtcSwap) -> Result<Self, BtcLifecycleCodecError> {
        let durable = DurableRecordV1::from_envelope(&active.durable_envelope());
        let exact_bytes =
            serde_json::to_vec(&durable).map_err(|_| BtcLifecycleCodecError::Malformed)?;
        Self::from_exact_bytes(exact_bytes)
    }

    /// Validates bounded canonical bytes loaded from an untrusted store.
    ///
    /// Compact canonical encoding is mandatory. Whitespace, reordered,
    /// duplicate or unknown fields, trailing input, unsupported schemas, and
    /// every protocol substitution fail closed.
    ///
    /// # Errors
    ///
    /// Returns a structured codec or deterministic protocol error.
    pub fn from_exact_bytes(
        exact_bytes: impl Into<Box<[u8]>>,
    ) -> Result<Self, BtcLifecycleCodecError> {
        let exact_bytes = exact_bytes.into();
        if exact_bytes.is_empty() || exact_bytes.len() > MAX_BTC_LIFECYCLE_RECORD_BYTES {
            return Err(BtcLifecycleCodecError::InvalidLength);
        }
        let mut deserializer = serde_json::Deserializer::from_slice(&exact_bytes);
        let durable = DurableRecordV1::deserialize(&mut deserializer)
            .map_err(|_| BtcLifecycleCodecError::Malformed)?;
        deserializer
            .end()
            .map_err(|_| BtcLifecycleCodecError::Malformed)?;
        if durable.schema_version != BTC_LIFECYCLE_RECORD_SCHEMA_V1 {
            return Err(BtcLifecycleCodecError::UnsupportedSchema(
                durable.schema_version,
            ));
        }
        let canonical =
            serde_json::to_vec(&durable).map_err(|_| BtcLifecycleCodecError::Malformed)?;
        if canonical.as_slice() != exact_bytes.as_ref() {
            return Err(BtcLifecycleCodecError::NonCanonical);
        }
        let envelope = durable.into_envelope()?;
        let agreement = BtcAgreementV1::from_wire(&envelope.agreement_wire)?;
        let pair = BtcPairSdk::new(
            envelope.local_participant,
            *agreement.bitcoin_chain_policy(),
        );
        let active = pair.resume(envelope)?;
        let status = active.status();
        Ok(Self {
            sha256: Sha256::digest(&exact_bytes).into(),
            exact_bytes,
            swap_id: status.swap_id().clone(),
            local_participant: status.local_participant(),
            revision: status.revision(),
        })
    }

    /// Complete canonical record bytes.
    #[must_use]
    pub fn exact_bytes(&self) -> &[u8] {
        &self.exact_bytes
    }

    /// SHA-256 of the complete canonical record.
    #[must_use]
    pub const fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    /// Agreement-derived swap ID.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Role that owns this record.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Aggregate revision reconstructed from exact transitions.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Reconstructs an untrusted envelope for full deterministic replay.
    ///
    /// # Errors
    ///
    /// Returns a codec error if the retained bytes no longer decode.
    pub fn decode(&self) -> Result<BtcActiveSwapEnvelopeV1, BtcLifecycleCodecError> {
        let mut deserializer = serde_json::Deserializer::from_slice(&self.exact_bytes);
        let durable = DurableRecordV1::deserialize(&mut deserializer)
            .map_err(|_| BtcLifecycleCodecError::Malformed)?;
        deserializer
            .end()
            .map_err(|_| BtcLifecycleCodecError::Malformed)?;
        durable.into_envelope()
    }

    fn same_key(&self, other: &Self) -> bool {
        self.swap_id == other.swap_id && self.local_participant == other.local_participant
    }
}

/// Failure while encoding or validating a durable lifecycle record.
#[derive(Debug, thiserror::Error)]
pub enum BtcLifecycleCodecError {
    /// Record is empty or exceeds the explicit bound.
    #[error("BTC lifecycle record length is invalid")]
    InvalidLength,
    /// JSON is malformed, trailing, duplicated, or has unknown fields.
    #[error("BTC lifecycle record is malformed")]
    Malformed,
    /// Bytes differ from the one compact canonical encoding.
    #[error("BTC lifecycle record is not canonical")]
    NonCanonical,
    /// Durable schema is unsupported.
    #[error("BTC lifecycle record schema {0} is unsupported")]
    UnsupportedSchema(u16),
    /// A public effect has the wrong shape for its purpose.
    #[error("BTC lifecycle public-effect shape is invalid")]
    InvalidEffectShape,
    /// A bounded exact public-effect component is invalid.
    #[error(transparent)]
    InvalidEffect(#[from] PublicEffectPlanError),
    /// A fixed-size public value has the wrong length.
    #[error("BTC lifecycle fixed-size value is invalid")]
    InvalidFixedValue,
    /// Agreement validation failed.
    #[error(transparent)]
    Agreement(#[from] BtcAgreementV1Error),
    /// Deterministic lifecycle replay failed.
    #[error(transparent)]
    Protocol(#[from] BtcSdkError),
}

/// Result of atomically creating revision zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcLifecycleStoreCreateV1 {
    /// The exact record became durable.
    Created,
    /// The identical exact record already existed.
    ExistingSame,
    /// The role-local swap key contains different bytes.
    Conflict,
}

/// Result of atomically replacing an exact predecessor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum BtcLifecycleStoreCompareExchangeV1 {
    /// Exact predecessor was replaced by exact successor.
    Applied,
    /// The identical successor already won.
    ExistingSame,
    /// Durable state differs from the supplied predecessor.
    Conflict {
        /// Current revision if the key still exists.
        actual_revision: Option<u64>,
    },
}

/// Process-durable role-local lifecycle storage contract.
///
/// Implementations make create and compare-exchange atomic across crashes,
/// preserve exact bytes unchanged, and leave all interpretation to the SDK.
#[async_trait]
pub trait BtcLifecycleStore: Clone + Send + Sync {
    /// Structured adapter error.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Atomically creates one exact immutable revision-zero record.
    async fn create(
        &self,
        record: &BtcLifecycleRecordV1,
    ) -> Result<BtcLifecycleStoreCreateV1, Self::Error>;

    /// Loads one exact role-local record.
    async fn load(
        &self,
        swap_id: &SwapId,
        local_participant: Participant,
    ) -> Result<Option<BtcLifecycleRecordV1>, Self::Error>;

    /// Atomically replaces the exact predecessor with its successor.
    async fn compare_exchange(
        &self,
        current: &BtcLifecycleRecordV1,
        replacement: &BtcLifecycleRecordV1,
    ) -> Result<BtcLifecycleStoreCompareExchangeV1, Self::Error>;
}

/// In-memory M3 reference implementation of the process-store contract.
///
/// It exercises exact CAS semantics but does not survive process termination.
#[derive(Clone, Default)]
pub struct InMemoryBtcLifecycleStore {
    records: Arc<Mutex<Vec<BtcLifecycleRecordV1>>>,
}

impl std::fmt::Debug for InMemoryBtcLifecycleStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InMemoryBtcLifecycleStore")
            .field("records", &"[REDACTED]")
            .finish()
    }
}

/// In-memory reference-store failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum InMemoryBtcLifecycleStoreError {
    /// Another task poisoned the reference-store mutex.
    #[error("in-memory BTC lifecycle store is unavailable")]
    Unavailable,
    /// Replacement changes key or does not advance exactly one revision.
    #[error("in-memory BTC lifecycle replacement is invalid")]
    InvalidReplacement,
}

#[async_trait]
impl BtcLifecycleStore for InMemoryBtcLifecycleStore {
    type Error = InMemoryBtcLifecycleStoreError;

    async fn create(
        &self,
        record: &BtcLifecycleRecordV1,
    ) -> Result<BtcLifecycleStoreCreateV1, Self::Error> {
        if record.revision() != 0 {
            return Err(InMemoryBtcLifecycleStoreError::InvalidReplacement);
        }
        let mut records = self
            .records
            .lock()
            .map_err(|_| InMemoryBtcLifecycleStoreError::Unavailable)?;
        Ok(
            match records.iter().find(|candidate| candidate.same_key(record)) {
                Some(existing) if existing == record => BtcLifecycleStoreCreateV1::ExistingSame,
                Some(_) => BtcLifecycleStoreCreateV1::Conflict,
                None => {
                    records.push(record.clone());
                    BtcLifecycleStoreCreateV1::Created
                }
            },
        )
    }

    async fn load(
        &self,
        swap_id: &SwapId,
        local_participant: Participant,
    ) -> Result<Option<BtcLifecycleRecordV1>, Self::Error> {
        let records = self
            .records
            .lock()
            .map_err(|_| InMemoryBtcLifecycleStoreError::Unavailable)?;
        Ok(records
            .iter()
            .find(|candidate| {
                candidate.swap_id() == swap_id && candidate.local_participant() == local_participant
            })
            .cloned())
    }

    async fn compare_exchange(
        &self,
        current: &BtcLifecycleRecordV1,
        replacement: &BtcLifecycleRecordV1,
    ) -> Result<BtcLifecycleStoreCompareExchangeV1, Self::Error> {
        if !current.same_key(replacement)
            || replacement.revision() != current.revision().saturating_add(1)
        {
            return Err(InMemoryBtcLifecycleStoreError::InvalidReplacement);
        }
        let mut records = self
            .records
            .lock()
            .map_err(|_| InMemoryBtcLifecycleStoreError::Unavailable)?;
        let Some(index) = records
            .iter()
            .position(|candidate| candidate.same_key(current))
        else {
            return Ok(BtcLifecycleStoreCompareExchangeV1::Conflict {
                actual_revision: None,
            });
        };
        if records[index] == *replacement {
            return Ok(BtcLifecycleStoreCompareExchangeV1::ExistingSame);
        }
        if records[index] != *current {
            return Ok(BtcLifecycleStoreCompareExchangeV1::Conflict {
                actual_revision: Some(records[index].revision()),
            });
        }
        records[index] = replacement.clone();
        Ok(BtcLifecycleStoreCompareExchangeV1::Applied)
    }
}

/// Role-fixed deterministic lifecycle composed with a durable store.
#[derive(Clone, Debug)]
pub struct StoredBtcLifecycleSdk<Store> {
    pair: BtcPairSdk,
    store: Store,
}

impl<Store> StoredBtcLifecycleSdk<Store>
where
    Store: BtcLifecycleStore,
{
    /// Composes a fixed-role pair facade with a store port.
    pub const fn new(pair: BtcPairSdk, store: Store) -> Self {
        Self { pair, store }
    }

    /// Fixed-role deterministic pair facade.
    pub const fn pair_sdk(&self) -> &BtcPairSdk {
        &self.pair
    }

    /// Persists revision zero before returning an active swap.
    ///
    /// # Errors
    ///
    /// Rejects protocol, codec, storage, or same-key conflicts.
    pub async fn activate(
        &self,
        accepted: AcceptedBtcAgreementV1,
        prepared: BtcPreparedProtocolV1,
    ) -> Result<ActiveBtcSwap, BtcSdkError> {
        let active = self.pair.activate_prepared(accepted, prepared)?;
        let record = BtcLifecycleRecordV1::from_active(&active)
            .map_err(|error| BtcSdkError::LifecycleCodec(Box::new(error)))?;
        match self
            .store
            .create(&record)
            .await
            .map_err(|_| BtcSdkError::LifecyclePersistenceUnavailable)?
        {
            BtcLifecycleStoreCreateV1::Created | BtcLifecycleStoreCreateV1::ExistingSame => {
                Ok(active)
            }
            BtcLifecycleStoreCreateV1::Conflict => Err(BtcSdkError::LifecycleStoreConflict),
        }
    }

    /// Loads and completely revalidates a role-local durable lifecycle.
    ///
    /// # Errors
    ///
    /// Returns storage, codec, or deterministic replay errors.
    pub async fn resume(&self, swap_id: &SwapId) -> Result<Option<ActiveBtcSwap>, BtcSdkError> {
        let Some(record) = self
            .store
            .load(swap_id, self.pair.local_participant())
            .await
            .map_err(|_| BtcSdkError::LifecyclePersistenceUnavailable)?
        else {
            return Ok(None);
        };
        self.pair
            .resume(
                record
                    .decode()
                    .map_err(|error| BtcSdkError::LifecycleCodec(Box::new(error)))?,
            )
            .map(Some)
    }

    /// Clone-validates a transition then atomically publishes its successor.
    ///
    /// Historical replay writes nothing. A competing exact winner converges
    /// after reload; a different winner fails closed.
    ///
    /// # Errors
    ///
    /// Returns protocol, persistence, or concurrent-conflict errors.
    pub async fn apply_transition(
        &self,
        swap_id: &SwapId,
        transition: BtcLifecycleTransitionV1,
    ) -> Result<BtcLifecycleTransitionOutcomeV1, BtcSdkError> {
        let current = self
            .store
            .load(swap_id, self.pair.local_participant())
            .await
            .map_err(|_| BtcSdkError::LifecyclePersistenceUnavailable)?
            .ok_or(BtcSdkError::LifecycleNotActivated)?;
        let mut active = self.pair.resume(
            current
                .decode()
                .map_err(|error| BtcSdkError::LifecycleCodec(Box::new(error)))?,
        )?;
        let outcome = active.apply_transition(transition.clone())?;
        if matches!(
            outcome,
            BtcLifecycleTransitionOutcomeV1::AlreadyApplied { .. }
        ) {
            return Ok(outcome);
        }
        let replacement = BtcLifecycleRecordV1::from_active(&active)
            .map_err(|error| BtcSdkError::LifecycleCodec(Box::new(error)))?;
        match self
            .store
            .compare_exchange(&current, &replacement)
            .await
            .map_err(|_| BtcSdkError::LifecyclePersistenceUnavailable)?
        {
            BtcLifecycleStoreCompareExchangeV1::Applied
            | BtcLifecycleStoreCompareExchangeV1::ExistingSame => Ok(outcome),
            BtcLifecycleStoreCompareExchangeV1::Conflict { .. } => {
                let winner = self
                    .store
                    .load(swap_id, self.pair.local_participant())
                    .await
                    .map_err(|_| BtcSdkError::LifecyclePersistenceUnavailable)?
                    .ok_or(BtcSdkError::LifecycleStoreConflict)?;
                let mut winner = self.pair.resume(
                    winner
                        .decode()
                        .map_err(|error| BtcSdkError::LifecycleCodec(Box::new(error)))?,
                )?;
                match winner.apply_transition(transition) {
                    Ok(BtcLifecycleTransitionOutcomeV1::AlreadyApplied { revision }) => {
                        Ok(BtcLifecycleTransitionOutcomeV1::AlreadyApplied { revision })
                    }
                    _ => Err(BtcSdkError::LifecycleStoreConflict),
                }
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DurableRecordV1 {
    schema_version: u16,
    agreement_wire: Vec<u8>,
    local_participant: Participant,
    revision: u64,
    lock_effects: LockEffectsRecordV1,
    prepared: Option<PreparedRecordV1>,
    transitions: Vec<TransitionRecordV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LockEffectsRecordV1 {
    bitcoin: ExactPublicEffectPlanV1,
    lez: ExactPublicEffectPlanV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PreparedRecordV1 {
    bitcoin_presignature: Vec<u8>,
    lez_presignature: Vec<u8>,
    lez_claim_public_id: String,
    lez_claim_template: Vec<u8>,
    lez_claim_signature_offset: usize,
    bitcoin_refund_signature: Vec<u8>,
    lez_refund: ExactPublicEffectPlanV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "transition", rename_all = "snake_case", deny_unknown_fields)]
enum TransitionRecordV1 {
    FirstLockConfirmed { evidence: FirstLockRecordV1 },
    SecondLockConfirmed { evidence: FirstLockRecordV1 },
    RevealingClaimConfirmed { evidence: RevealingRecordV1 },
    FollowupClaimConfirmed { evidence: FollowupRecordV1 },
    RecoveryObserved { state: RecoveryRecordV1 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "chain", rename_all = "snake_case", deny_unknown_fields)]
enum FirstLockRecordV1 {
    Bitcoin {
        genesis_block_hash: [u8; 32],
        exact_transaction: Vec<u8>,
        confirmations: u32,
    },
    Lez {
        genesis_block_hash: [u8; 32],
        initialization_public_id: String,
        exact_initialization: Vec<u8>,
        funding_public_id: String,
        exact_funding: Vec<u8>,
        metadata_account: [u8; 32],
        custody_account: [u8; 32],
        #[serde(with = "canonical_u128")]
        amount: u128,
        finalized: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "chain", rename_all = "snake_case", deny_unknown_fields)]
enum RevealingRecordV1 {
    Bitcoin {
        claimant: Participant,
        genesis_block_hash: [u8; 32],
        exact_transaction: Vec<u8>,
        confirmations: u32,
    },
    Lez {
        claimant: Participant,
        genesis_block_hash: [u8; 32],
        public_id: String,
        exact_claim: Vec<u8>,
        signature: Vec<u8>,
        finalized: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "chain", rename_all = "snake_case", deny_unknown_fields)]
enum FollowupRecordV1 {
    Bitcoin {
        genesis_block_hash: [u8; 32],
        exact_transaction: Vec<u8>,
        confirmations: u32,
    },
    Lez {
        genesis_block_hash: [u8; 32],
        public_id: String,
        exact_claim: Vec<u8>,
        finalized: bool,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RecoveryStatusRecordV1 {
    Absent,
    Locked,
    Refunded,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct BitcoinRecoveryRecordV1 {
    status: RecoveryStatusRecordV1,
    genesis_block_hash: Option<[u8; 32]>,
    funding_transaction_id: Option<[u8; 32]>,
    refund_transaction_id: Option<[u8; 32]>,
    confirmations: u32,
    funding_output_unspent: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LezRecoveryRecordV1 {
    status: RecoveryStatusRecordV1,
    genesis_block_hash: Option<[u8; 32]>,
    initialization_public_id: Option<String>,
    funding_public_id: Option<String>,
    refund_public_id: Option<String>,
    finalized: bool,
    custody_unspent: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RecoveryRecordV1 {
    agreement_commitment: [u8; 32],
    direction: SwapDirection,
    bitcoin_best_height: u32,
    lez_unix_seconds: u64,
    bitcoin: BitcoinRecoveryRecordV1,
    lez: LezRecoveryRecordV1,
}

mod canonical_u128 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.is_empty()
            || (value.len() > 1 && value.starts_with('0'))
            || !value.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(serde::de::Error::custom("non-canonical u128"));
        }
        value
            .parse()
            .map_err(|_| serde::de::Error::custom("u128 out of range"))
    }
}

impl DurableRecordV1 {
    fn from_envelope(envelope: &BtcActiveSwapEnvelopeV1) -> Self {
        Self {
            schema_version: BTC_LIFECYCLE_RECORD_SCHEMA_V1,
            agreement_wire: envelope.agreement_wire.to_vec(),
            local_participant: envelope.local_participant,
            revision: envelope.revision,
            lock_effects: LockEffectsRecordV1 {
                bitcoin: envelope.lock_effects.bitcoin().plan().clone(),
                lez: envelope.lock_effects.lez().plan().clone(),
            },
            prepared: envelope
                .prepared_protocol
                .as_ref()
                .map(PreparedRecordV1::from_prepared),
            transitions: envelope
                .transitions
                .iter()
                .map(TransitionRecordV1::from_transition)
                .collect(),
        }
    }

    fn into_envelope(self) -> Result<BtcActiveSwapEnvelopeV1, BtcLifecycleCodecError> {
        let agreement = BtcAgreementV1::from_wire(&self.agreement_wire)?;
        let lock_effects = self.lock_effects.into_effects()?;
        match self.prepared {
            Some(prepared) => {
                let pair =
                    BtcPairSdk::new(self.local_participant, *agreement.bitcoin_chain_policy());
                let terms =
                    BtcProtocolTermsV1::new(agreement.record().clone(), lock_effects.clone())
                        .with_claim_effects(prepared.claim_effects(&agreement)?)
                        .with_recovery_effects(prepared.recovery_effects(&agreement)?);
                let validated = pair.validate_terms(&terms)?;
                let prepared = pair.prepare(validated)?;
                Ok(BtcActiveSwapEnvelopeV1::from_lifecycle_parts(
                    self.agreement_wire,
                    self.local_participant,
                    self.revision,
                    lock_effects,
                    prepared,
                    self.transitions
                        .into_iter()
                        .map(TransitionRecordV1::into_transition)
                        .collect::<Result<Vec<_>, _>>()?,
                ))
            }
            None => Ok(BtcActiveSwapEnvelopeV1::from_parts(
                self.agreement_wire,
                self.local_participant,
                self.revision,
                lock_effects,
            )),
        }
    }
}

impl LockEffectsRecordV1 {
    fn into_effects(self) -> Result<BtcPreparedLockEffectsV1, BtcLifecycleCodecError> {
        let [bitcoin] = self.bitcoin.steps() else {
            return Err(BtcLifecycleCodecError::InvalidEffectShape);
        };
        if bitcoin.step().as_str() != BITCOIN_FUNDING_STEP {
            return Err(BtcLifecycleCodecError::InvalidEffectShape);
        }
        let reconstructed_bitcoin = PreparedBitcoinFundingV1::new(
            bitcoin.expected_public_id().as_str(),
            bitcoin.exact_bytes().as_slice().to_vec(),
        )?;
        if reconstructed_bitcoin.plan() != &self.bitcoin {
            return Err(BtcLifecycleCodecError::InvalidEffectShape);
        }
        let [initialization, funding] = self.lez.steps() else {
            return Err(BtcLifecycleCodecError::InvalidEffectShape);
        };
        if initialization.step().as_str() != LEZ_INITIALIZE_STEP
            || funding.step().as_str() != LEZ_FUND_STEP
        {
            return Err(BtcLifecycleCodecError::InvalidEffectShape);
        }
        let reconstructed_lez = PreparedLezFundingV1::new(
            initialization.expected_public_id().as_str(),
            initialization.exact_bytes().as_slice().to_vec(),
            funding.expected_public_id().as_str(),
            funding.exact_bytes().as_slice().to_vec(),
        )?;
        if reconstructed_lez.plan() != &self.lez {
            return Err(BtcLifecycleCodecError::InvalidEffectShape);
        }
        Ok(BtcPreparedLockEffectsV1::new(
            reconstructed_bitcoin,
            reconstructed_lez,
        ))
    }
}

impl PreparedRecordV1 {
    fn from_prepared(prepared: &BtcPreparedProtocolV1) -> Self {
        let claims = prepared.claim_effects();
        let recovery = prepared
            .recovery_effects()
            .expect("full lifecycle preparation has recovery effects");
        Self {
            bitcoin_presignature: claims.bitcoin_presignature.to_vec(),
            lez_presignature: claims.lez_presignature.to_vec(),
            lez_claim_public_id: claims.lez_claim.expected_public_id.as_str().to_owned(),
            lez_claim_template: claims.lez_claim.exact_template.as_slice().to_vec(),
            lez_claim_signature_offset: claims.lez_claim.signature_offset,
            bitcoin_refund_signature: recovery.bitcoin.signature.to_vec(),
            lez_refund: recovery.lez.plan().clone(),
        }
    }

    fn claim_effects(
        &self,
        agreement: &BtcAgreementV1,
    ) -> Result<BtcPreparedClaimEffectsV1, BtcLifecycleCodecError> {
        let lez_claim = PreparedLezClaimTemplateV1::new(
            self.lez_claim_public_id.clone(),
            self.lez_claim_template.clone(),
            self.lez_claim_signature_offset,
        )?;
        Ok(BtcPreparedClaimEffectsV1::new(
            agreement,
            fixed::<65>(&self.bitcoin_presignature)?,
            fixed::<65>(&self.lez_presignature)?,
            lez_claim,
        ))
    }

    fn recovery_effects(
        &self,
        agreement: &BtcAgreementV1,
    ) -> Result<BtcPreparedRecoveryEffectsV1, BtcLifecycleCodecError> {
        let bitcoin = PreparedBitcoinRefundV1::new(
            agreement,
            fixed::<SCHNORR_SIGNATURE_BYTES>(&self.bitcoin_refund_signature)?,
        )?;
        let [lez] = self.lez_refund.steps() else {
            return Err(BtcLifecycleCodecError::InvalidEffectShape);
        };
        if lez.step().as_str() != LEZ_REFUND_STEP {
            return Err(BtcLifecycleCodecError::InvalidEffectShape);
        }
        let lez_effect = PreparedLezRefundV1::new(
            agreement,
            lez.expected_public_id().as_str(),
            lez.exact_bytes().as_slice().to_vec(),
        )?;
        if lez_effect.plan() != &self.lez_refund {
            return Err(BtcLifecycleCodecError::InvalidEffectShape);
        }
        Ok(BtcPreparedRecoveryEffectsV1::new(bitcoin, lez_effect))
    }
}

impl TransitionRecordV1 {
    fn from_transition(transition: &BtcLifecycleTransitionV1) -> Self {
        match transition {
            BtcLifecycleTransitionV1::FirstLockConfirmed(evidence) => Self::FirstLockConfirmed {
                evidence: FirstLockRecordV1::from_evidence(evidence),
            },
            BtcLifecycleTransitionV1::SecondLockConfirmed(evidence) => Self::SecondLockConfirmed {
                evidence: FirstLockRecordV1::from_evidence(evidence),
            },
            BtcLifecycleTransitionV1::RevealingClaimConfirmed(evidence) => {
                Self::RevealingClaimConfirmed {
                    evidence: RevealingRecordV1::from_evidence(evidence),
                }
            }
            BtcLifecycleTransitionV1::FollowupClaimConfirmed(evidence) => {
                Self::FollowupClaimConfirmed {
                    evidence: FollowupRecordV1::from_evidence(evidence),
                }
            }
            BtcLifecycleTransitionV1::RecoveryObserved(state) => Self::RecoveryObserved {
                state: RecoveryRecordV1::from_state(state),
            },
        }
    }

    fn into_transition(self) -> Result<BtcLifecycleTransitionV1, BtcLifecycleCodecError> {
        Ok(match self {
            Self::FirstLockConfirmed { evidence } => {
                BtcLifecycleTransitionV1::FirstLockConfirmed(evidence.into_evidence()?)
            }
            Self::SecondLockConfirmed { evidence } => {
                BtcLifecycleTransitionV1::SecondLockConfirmed(evidence.into_evidence()?)
            }
            Self::RevealingClaimConfirmed { evidence } => {
                BtcLifecycleTransitionV1::RevealingClaimConfirmed(evidence.into_evidence()?)
            }
            Self::FollowupClaimConfirmed { evidence } => {
                BtcLifecycleTransitionV1::FollowupClaimConfirmed(evidence.into_evidence()?)
            }
            Self::RecoveryObserved { state } => {
                BtcLifecycleTransitionV1::RecoveryObserved(state.into_state()?)
            }
        })
    }
}

impl FirstLockRecordV1 {
    fn from_evidence(evidence: &BtcFirstLockEvidenceV1) -> Self {
        match evidence {
            BtcFirstLockEvidenceV1::Bitcoin(value) => Self::Bitcoin {
                genesis_block_hash: value.genesis_block_hash,
                exact_transaction: value.exact_transaction.as_slice().to_vec(),
                confirmations: value.confirmations,
            },
            BtcFirstLockEvidenceV1::Lez(value) => Self::Lez {
                genesis_block_hash: value.genesis_block_hash,
                initialization_public_id: value.initialization_public_id.as_str().to_owned(),
                exact_initialization: value.exact_initialization.as_slice().to_vec(),
                funding_public_id: value.funding_public_id.as_str().to_owned(),
                exact_funding: value.exact_funding.as_slice().to_vec(),
                metadata_account: value.metadata_account,
                custody_account: value.custody_account,
                amount: value.amount,
                finalized: value.finalized,
            },
        }
    }

    fn into_evidence(self) -> Result<BtcFirstLockEvidenceV1, BtcLifecycleCodecError> {
        Ok(match self {
            Self::Bitcoin {
                genesis_block_hash,
                exact_transaction,
                confirmations,
            } => BtcFirstLockEvidenceV1::Bitcoin(BitcoinFirstLockEvidenceV1::new(
                genesis_block_hash,
                exact_transaction,
                confirmations,
            )?),
            Self::Lez {
                genesis_block_hash,
                initialization_public_id,
                exact_initialization,
                funding_public_id,
                exact_funding,
                metadata_account,
                custody_account,
                amount,
                finalized,
            } => BtcFirstLockEvidenceV1::Lez(LezFirstLockEvidenceV1::new(
                genesis_block_hash,
                initialization_public_id,
                exact_initialization,
                funding_public_id,
                exact_funding,
                metadata_account,
                custody_account,
                amount,
                finalized,
            )?),
        })
    }
}

impl RevealingRecordV1 {
    fn from_evidence(evidence: &BtcRevealingClaimEvidenceV1) -> Self {
        match evidence {
            BtcRevealingClaimEvidenceV1::Bitcoin(value) => Self::Bitcoin {
                claimant: value.claimant,
                genesis_block_hash: value.genesis_block_hash,
                exact_transaction: value.exact_transaction.as_slice().to_vec(),
                confirmations: value.confirmations,
            },
            BtcRevealingClaimEvidenceV1::Lez(value) => Self::Lez {
                claimant: value.claimant,
                genesis_block_hash: value.genesis_block_hash,
                public_id: value.public_id.as_str().to_owned(),
                exact_claim: value.exact_claim.as_slice().to_vec(),
                signature: value.signature.to_vec(),
                finalized: value.finalized,
            },
        }
    }

    fn into_evidence(self) -> Result<BtcRevealingClaimEvidenceV1, BtcLifecycleCodecError> {
        Ok(match self {
            Self::Bitcoin {
                claimant,
                genesis_block_hash,
                exact_transaction,
                confirmations,
            } => BtcRevealingClaimEvidenceV1::Bitcoin(BitcoinRevealingClaimEvidenceV1::new(
                claimant,
                genesis_block_hash,
                exact_transaction,
                confirmations,
            )?),
            Self::Lez {
                claimant,
                genesis_block_hash,
                public_id,
                exact_claim,
                signature,
                finalized,
            } => BtcRevealingClaimEvidenceV1::Lez(LezRevealingClaimEvidenceV1::new(
                claimant,
                genesis_block_hash,
                public_id,
                exact_claim,
                fixed::<SCHNORR_SIGNATURE_BYTES>(&signature)?,
                finalized,
            )?),
        })
    }
}

impl FollowupRecordV1 {
    fn from_evidence(evidence: &BtcFollowupClaimEvidenceV1) -> Self {
        match evidence {
            BtcFollowupClaimEvidenceV1::Bitcoin(value) => Self::Bitcoin {
                genesis_block_hash: value.genesis_block_hash,
                exact_transaction: value.exact_transaction.as_slice().to_vec(),
                confirmations: value.confirmations,
            },
            BtcFollowupClaimEvidenceV1::Lez(value) => Self::Lez {
                genesis_block_hash: value.genesis_block_hash,
                public_id: value.public_id.as_str().to_owned(),
                exact_claim: value.exact_claim.as_slice().to_vec(),
                finalized: value.finalized,
            },
        }
    }

    fn into_evidence(self) -> Result<BtcFollowupClaimEvidenceV1, BtcLifecycleCodecError> {
        Ok(match self {
            Self::Bitcoin {
                genesis_block_hash,
                exact_transaction,
                confirmations,
            } => BtcFollowupClaimEvidenceV1::Bitcoin(BitcoinFollowupClaimEvidenceV1::new(
                genesis_block_hash,
                exact_transaction,
                confirmations,
            )?),
            Self::Lez {
                genesis_block_hash,
                public_id,
                exact_claim,
                finalized,
            } => BtcFollowupClaimEvidenceV1::Lez(LezFollowupClaimEvidenceV1::new(
                genesis_block_hash,
                public_id,
                exact_claim,
                finalized,
            )?),
        })
    }
}

impl RecoveryRecordV1 {
    fn from_state(state: &BtcCanonicalRecoveryStateV1) -> Self {
        Self {
            agreement_commitment: state.agreement_commitment,
            direction: state.direction,
            bitcoin_best_height: state.bitcoin_best_height,
            lez_unix_seconds: state.lez_unix_seconds,
            bitcoin: BitcoinRecoveryRecordV1::from_state(&state.bitcoin),
            lez: LezRecoveryRecordV1::from_state(&state.lez),
        }
    }

    fn into_state(self) -> Result<BtcCanonicalRecoveryStateV1, BtcLifecycleCodecError> {
        Ok(BtcCanonicalRecoveryStateV1 {
            agreement_commitment: self.agreement_commitment,
            direction: self.direction,
            bitcoin_best_height: self.bitcoin_best_height,
            lez_unix_seconds: self.lez_unix_seconds,
            bitcoin: self.bitcoin.into_state(),
            lez: self.lez.into_state()?,
        })
    }
}

impl BitcoinRecoveryRecordV1 {
    fn from_state(state: &BitcoinCanonicalRecoveryStateV1) -> Self {
        Self {
            status: RecoveryStatusRecordV1::from_status(state.status),
            genesis_block_hash: state.genesis_block_hash,
            funding_transaction_id: state.funding_transaction_id,
            refund_transaction_id: state.refund_transaction_id,
            confirmations: state.confirmations,
            funding_output_unspent: state.funding_output_unspent,
        }
    }

    fn into_state(self) -> BitcoinCanonicalRecoveryStateV1 {
        BitcoinCanonicalRecoveryStateV1 {
            status: self.status.into_status(),
            genesis_block_hash: self.genesis_block_hash,
            funding_transaction_id: self.funding_transaction_id,
            refund_transaction_id: self.refund_transaction_id,
            confirmations: self.confirmations,
            funding_output_unspent: self.funding_output_unspent,
        }
    }
}

impl LezRecoveryRecordV1 {
    fn from_state(state: &LezCanonicalRecoveryStateV1) -> Self {
        Self {
            status: RecoveryStatusRecordV1::from_status(state.status),
            genesis_block_hash: state.genesis_block_hash,
            initialization_public_id: state
                .initialization_public_id
                .as_ref()
                .map(|value| value.as_str().to_owned()),
            funding_public_id: state
                .funding_public_id
                .as_ref()
                .map(|value| value.as_str().to_owned()),
            refund_public_id: state
                .refund_public_id
                .as_ref()
                .map(|value| value.as_str().to_owned()),
            finalized: state.finalized,
            custody_unspent: state.custody_unspent,
        }
    }

    fn into_state(self) -> Result<LezCanonicalRecoveryStateV1, BtcLifecycleCodecError> {
        Ok(LezCanonicalRecoveryStateV1 {
            status: self.status.into_status(),
            genesis_block_hash: self.genesis_block_hash,
            initialization_public_id: self
                .initialization_public_id
                .map(ExpectedPublicEffectId::new)
                .transpose()?,
            funding_public_id: self
                .funding_public_id
                .map(ExpectedPublicEffectId::new)
                .transpose()?,
            refund_public_id: self
                .refund_public_id
                .map(ExpectedPublicEffectId::new)
                .transpose()?,
            finalized: self.finalized,
            custody_unspent: self.custody_unspent,
        })
    }
}

impl RecoveryStatusRecordV1 {
    const fn from_status(status: CanonicalRecoveryStatusV1) -> Self {
        match status {
            CanonicalRecoveryStatusV1::Absent => Self::Absent,
            CanonicalRecoveryStatusV1::Locked => Self::Locked,
            CanonicalRecoveryStatusV1::Refunded => Self::Refunded,
        }
    }

    const fn into_status(self) -> CanonicalRecoveryStatusV1 {
        match self {
            Self::Absent => CanonicalRecoveryStatusV1::Absent,
            Self::Locked => CanonicalRecoveryStatusV1::Locked,
            Self::Refunded => CanonicalRecoveryStatusV1::Refunded,
        }
    }
}

fn fixed<const N: usize>(bytes: &[u8]) -> Result<[u8; N], BtcLifecycleCodecError> {
    bytes
        .try_into()
        .map_err(|_| BtcLifecycleCodecError::InvalidFixedValue)
}
