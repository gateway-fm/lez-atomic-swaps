//! Versioned exact-public-effect plan vocabulary.

use std::collections::HashSet;

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use sha2::{Digest as _, Sha256};

use crate::SchemaVersion;

const MAX_PLAN_STEPS: usize = 32;
const MAX_STEP_ID_BYTES: usize = 96;
const MAX_EXPECTED_ID_BYTES: usize = 512;
const MAX_EXACT_EFFECT_BYTES: usize = 4 * 1024 * 1024;
const MAX_PLAN_EXACT_BYTES: usize = 8 * 1024 * 1024;
const PLAN_COMMITMENT_DOMAIN: &[u8] = b"lez-swap-sdk-core/public-effect-plan/v1";

/// Schema version for [`ExactPublicEffectPlanV1`].
pub const EXACT_PUBLIC_EFFECT_PLAN_SCHEMA_V1: SchemaVersion = SchemaVersion::V1;

/// Stable pair-defined discriminator for one ordered public effect.
///
/// IDs use lowercase dot-separated ASCII components such as `lez.initialize`,
/// `lez.fund`, or `bitcoin.funding`. They are persisted as protocol vocabulary,
/// so changing an ID is a compatibility change.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
#[must_use]
pub struct PublicEffectStepId(Box<str>);

impl PublicEffectStepId {
    /// Validates a stable step discriminator.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, non-ASCII, uppercase, empty-component, or
    /// punctuation-bearing identifiers.
    pub fn new(value: impl Into<Box<str>>) -> Result<Self, PublicEffectPlanError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_STEP_ID_BYTES || !valid_step_id(&value) {
            return Err(PublicEffectPlanError::InvalidStepId);
        }
        Ok(Self(value))
    }

    /// Stable discriminator bytes.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PublicEffectStepId {
    fn deserialize<DeserializerT>(deserializer: DeserializerT) -> Result<Self, DeserializerT::Error>
    where
        DeserializerT: Deserializer<'de>,
    {
        Self::new(Box::<str>::deserialize(deserializer)?).map_err(DeserializerT::Error::custom)
    }
}

/// Expected chain-native identity for exact public bytes.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
#[must_use]
pub struct ExpectedPublicEffectId(Box<str>);

impl ExpectedPublicEffectId {
    /// Validates a bounded printable public identity.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, whitespace, control, or non-ASCII values.
    pub fn new(value: impl Into<Box<str>>) -> Result<Self, PublicEffectPlanError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_EXPECTED_ID_BYTES
            || !value.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(PublicEffectPlanError::InvalidExpectedPublicId);
        }
        Ok(Self(value))
    }

    /// Expected chain-native identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ExpectedPublicEffectId {
    fn deserialize<DeserializerT>(deserializer: DeserializerT) -> Result<Self, DeserializerT::Error>
    where
        DeserializerT: Deserializer<'de>,
    {
        Self::new(Box::<str>::deserialize(deserializer)?).map_err(DeserializerT::Error::custom)
    }
}

/// Complete byte-identical public transaction or instruction envelope.
///
/// This plaintext type must not hold an unrevealed preimage, adaptor secret,
/// nonce, private key, seed, or other recovery secret. Secret-bearing exact
/// effects require a pair-specific protected envelope instead.
#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(transparent)]
#[must_use]
pub struct ExactPublicEffectBytes(Box<[u8]>);

impl std::fmt::Debug for ExactPublicEffectBytes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExactPublicEffectBytes")
            .field("length", &self.0.len())
            .field("sha256", &Sha256::digest(&self.0))
            .finish_non_exhaustive()
    }
}

impl ExactPublicEffectBytes {
    /// Validates complete public wire bytes.
    ///
    /// # Errors
    ///
    /// Rejects empty or larger-than-4-MiB material.
    pub fn new(value: impl Into<Box<[u8]>>) -> Result<Self, PublicEffectPlanError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_EXACT_EFFECT_BYTES {
            return Err(PublicEffectPlanError::InvalidExactBytes);
        }
        Ok(Self(value))
    }

    /// Complete bytes that must be persisted and submitted unchanged.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// SHA-256 commitment to the complete bytes.
    #[must_use]
    pub fn sha256(&self) -> [u8; 32] {
        Sha256::digest(&self.0).into()
    }
}

impl<'de> Deserialize<'de> for ExactPublicEffectBytes {
    fn deserialize<DeserializerT>(deserializer: DeserializerT) -> Result<Self, DeserializerT::Error>
    where
        DeserializerT: Deserializer<'de>,
    {
        Self::new(Vec::<u8>::deserialize(deserializer)?).map_err(DeserializerT::Error::custom)
    }
}

/// One immutable step in an ordered exact public-effect plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[must_use]
pub struct PublicEffectStepV1 {
    step: PublicEffectStepId,
    expected_public_id: ExpectedPublicEffectId,
    exact_bytes: ExactPublicEffectBytes,
}

impl PublicEffectStepV1 {
    /// Creates one already-validated exact effect step.
    pub const fn new(
        step: PublicEffectStepId,
        expected_public_id: ExpectedPublicEffectId,
        exact_bytes: ExactPublicEffectBytes,
    ) -> Self {
        Self {
            step,
            expected_public_id,
            exact_bytes,
        }
    }

    /// Stable discriminator used by durable intent and replay records.
    pub const fn step(&self) -> &PublicEffectStepId {
        &self.step
    }

    /// Expected chain-native identity of the exact bytes.
    pub const fn expected_public_id(&self) -> &ExpectedPublicEffectId {
        &self.expected_public_id
    }

    /// Complete public wire bytes for this step.
    pub const fn exact_bytes(&self) -> &ExactPublicEffectBytes {
        &self.exact_bytes
    }
}

/// Version-1 ordered exact public-effect plan.
///
/// A maker lock may require multiple public effects. The vector order is the
/// only allowed execution order, while each stable step ID gives persistence a
/// chain-independent idempotency discriminator. The plan performs no I/O and
/// grants no submission authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[must_use]
pub struct ExactPublicEffectPlanV1 {
    schema_version: SchemaVersion,
    steps: Vec<PublicEffectStepV1>,
}

impl<'de> Deserialize<'de> for ExactPublicEffectPlanV1 {
    fn deserialize<DeserializerT>(deserializer: DeserializerT) -> Result<Self, DeserializerT::Error>
    where
        DeserializerT: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Record {
            schema_version: SchemaVersion,
            steps: Vec<PublicEffectStepV1>,
        }

        let record = Record::deserialize(deserializer)?;
        if record.schema_version != EXACT_PUBLIC_EFFECT_PLAN_SCHEMA_V1 {
            return Err(DeserializerT::Error::custom(
                PublicEffectPlanError::UnsupportedSchemaVersion,
            ));
        }
        Self::new(record.steps).map_err(DeserializerT::Error::custom)
    }
}

impl ExactPublicEffectPlanV1 {
    /// Validates a nonempty, bounded plan with unique step discriminators.
    ///
    /// # Errors
    ///
    /// Rejects empty/oversized plans, duplicate step IDs, or more than 8 MiB of
    /// aggregate exact public bytes.
    pub fn new(steps: Vec<PublicEffectStepV1>) -> Result<Self, PublicEffectPlanError> {
        if steps.is_empty() {
            return Err(PublicEffectPlanError::EmptyPlan);
        }
        if steps.len() > MAX_PLAN_STEPS {
            return Err(PublicEffectPlanError::TooManySteps);
        }

        let mut seen = HashSet::with_capacity(steps.len());
        let mut total_bytes = 0usize;
        for step in &steps {
            if !seen.insert(step.step().clone()) {
                return Err(PublicEffectPlanError::DuplicateStepId);
            }
            total_bytes = total_bytes
                .checked_add(step.exact_bytes().as_slice().len())
                .ok_or(PublicEffectPlanError::PlanBytesTooLarge)?;
            if total_bytes > MAX_PLAN_EXACT_BYTES {
                return Err(PublicEffectPlanError::PlanBytesTooLarge);
            }
        }

        Ok(Self {
            schema_version: EXACT_PUBLIC_EFFECT_PLAN_SCHEMA_V1,
            steps,
        })
    }

    /// Public schema version carried by this plan.
    pub const fn schema_version(&self) -> SchemaVersion {
        self.schema_version
    }

    /// Ordered effect steps. A later step must not run before earlier steps have
    /// the pair-specific canonical evidence required by its caller.
    pub fn steps(&self) -> &[PublicEffectStepV1] {
        &self.steps
    }

    /// Looks up a step by its stable discriminator.
    #[must_use]
    pub fn step(&self, step: &PublicEffectStepId) -> Option<&PublicEffectStepV1> {
        self.steps.iter().find(|candidate| candidate.step() == step)
    }

    /// Domain-separated commitment to schema, order, IDs, and complete bytes.
    #[must_use]
    pub fn commitment(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(PLAN_COMMITMENT_DOMAIN);
        digest.update(self.schema_version.get().to_be_bytes());
        let step_count = u32::try_from(self.steps.len()).unwrap_or(u32::MAX);
        digest.update(step_count.to_be_bytes());
        for step in &self.steps {
            update_len_prefixed(&mut digest, step.step().as_str().as_bytes());
            update_len_prefixed(&mut digest, step.expected_public_id().as_str().as_bytes());
            update_len_prefixed(&mut digest, step.exact_bytes().as_slice());
        }
        digest.finalize().into()
    }
}

/// Invalid exact-public-effect plan or component.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PublicEffectPlanError {
    /// Decoder received a schema other than the exact supported version.
    #[error("public-effect plan schema version is unsupported")]
    UnsupportedSchemaVersion,
    /// Stable step discriminator does not use the bounded canonical grammar.
    #[error("public-effect step discriminator is invalid")]
    InvalidStepId,
    /// Expected chain-native identity is empty, oversized, or not printable ASCII.
    #[error("expected public-effect identity is invalid")]
    InvalidExpectedPublicId,
    /// Exact public bytes are empty or exceed the per-effect limit.
    #[error("exact public-effect bytes are invalid")]
    InvalidExactBytes,
    /// A plan must contain at least one effect.
    #[error("public-effect plan is empty")]
    EmptyPlan,
    /// A plan exceeds the bounded number of effects.
    #[error("public-effect plan contains too many steps")]
    TooManySteps,
    /// Stable step IDs must be unique within a plan.
    #[error("public-effect plan repeats a step discriminator")]
    DuplicateStepId,
    /// Aggregate exact bytes exceed the plan limit.
    #[error("public-effect plan exact bytes exceed the aggregate limit")]
    PlanBytesTooLarge,
}

fn valid_step_id(value: &str) -> bool {
    value.split('.').all(|component| {
        let mut bytes = component.bytes();
        bytes.next().is_some_and(|first| first.is_ascii_lowercase())
            && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    })
}

fn update_len_prefixed(digest: &mut Sha256, value: &[u8]) {
    let length = u64::try_from(value.len()).unwrap_or(u64::MAX);
    digest.update(length.to_be_bytes());
    digest.update(value);
}

#[cfg(test)]
mod tests {
    use super::{
        ExactPublicEffectBytes, ExactPublicEffectPlanV1, ExpectedPublicEffectId,
        PublicEffectPlanError, PublicEffectStepId, PublicEffectStepV1,
    };

    fn step(id: &str, public_id: &str, bytes: &[u8]) -> PublicEffectStepV1 {
        PublicEffectStepV1::new(
            PublicEffectStepId::new(id).expect("step ID"),
            ExpectedPublicEffectId::new(public_id).expect("public ID"),
            ExactPublicEffectBytes::new(bytes.to_vec()).expect("exact bytes"),
        )
    }

    #[test]
    fn multi_step_plan_preserves_order_and_exact_identity() {
        let initialize = step("lez.initialize", "tx:init", &[1, 2, 3]);
        let fund = step("lez.fund", "tx:fund", &[4, 5]);
        let plan = ExactPublicEffectPlanV1::new(vec![initialize.clone(), fund.clone()])
            .expect("valid two-step maker lock");

        assert_eq!(plan.schema_version().get(), 1);
        assert_eq!(plan.steps(), &[initialize, fund]);
        assert_eq!(
            plan.step(&PublicEffectStepId::new("lez.fund").expect("step"))
                .expect("fund step")
                .exact_bytes()
                .as_slice(),
            [4, 5]
        );
    }

    #[test]
    fn commitment_binds_order_ids_and_exact_bytes() {
        let first = step("lez.initialize", "tx:init", &[1]);
        let second = step("lez.fund", "tx:fund", &[2]);
        let ordered = ExactPublicEffectPlanV1::new(vec![first.clone(), second.clone()])
            .expect("ordered plan");
        let reversed = ExactPublicEffectPlanV1::new(vec![second.clone(), first.clone()])
            .expect("reversed plan");
        let changed_id =
            ExactPublicEffectPlanV1::new(vec![first.clone(), step("lez.fund", "tx:other", &[2])])
                .expect("changed ID plan");
        let changed_bytes =
            ExactPublicEffectPlanV1::new(vec![first, step("lez.fund", "tx:fund", &[3])])
                .expect("changed bytes plan");

        assert_ne!(ordered.commitment(), reversed.commitment());
        assert_ne!(ordered.commitment(), changed_id.commitment());
        assert_ne!(ordered.commitment(), changed_bytes.commitment());
    }

    #[test]
    fn plan_rejects_duplicate_or_ambiguous_steps() {
        let duplicate = step("lez.fund", "tx:one", &[1]);
        assert_eq!(
            ExactPublicEffectPlanV1::new(vec![duplicate, step("lez.fund", "tx:two", &[2]),]),
            Err(PublicEffectPlanError::DuplicateStepId)
        );
        assert_eq!(
            PublicEffectStepId::new("lez..fund"),
            Err(PublicEffectPlanError::InvalidStepId)
        );
        assert_eq!(
            PublicEffectStepId::new("LEZ.fund"),
            Err(PublicEffectPlanError::InvalidStepId)
        );
        assert_eq!(
            ExpectedPublicEffectId::new("contains whitespace"),
            Err(PublicEffectPlanError::InvalidExpectedPublicId)
        );
        assert_eq!(
            ExactPublicEffectBytes::new(Vec::<u8>::new()),
            Err(PublicEffectPlanError::InvalidExactBytes)
        );
    }

    #[test]
    fn exact_bytes_debug_never_prints_payload() {
        let bytes =
            ExactPublicEffectBytes::new(b"public-but-large-payload".to_vec()).expect("exact bytes");
        let debug = format!("{bytes:?}");
        assert!(debug.contains("length"));
        assert!(debug.contains("sha256"));
        assert!(!debug.contains("public-but-large-payload"));
    }

    #[test]
    fn decoding_revalidates_schema_steps_and_bounds() {
        let valid = ExactPublicEffectPlanV1::new(vec![step("lez.fund", "tx:fund", &[1])])
            .expect("valid plan");
        let encoded = serde_json::to_vec(&valid).expect("encode plan");
        assert_eq!(
            serde_json::from_slice::<ExactPublicEffectPlanV1>(&encoded).expect("decode plan"),
            valid
        );

        let mut wrong_schema = serde_json::to_value(&valid).expect("plan value");
        wrong_schema["schema_version"] = serde_json::json!(2);
        assert!(serde_json::from_value::<ExactPublicEffectPlanV1>(wrong_schema).is_err());

        let mut invalid_step = serde_json::to_value(&valid).expect("plan value");
        invalid_step["steps"][0]["step"] = serde_json::json!("LEZ.fund");
        assert!(serde_json::from_value::<ExactPublicEffectPlanV1>(invalid_step).is_err());

        let mut duplicate = serde_json::to_value(&valid).expect("plan value");
        duplicate["steps"] = serde_json::json!([
            {"step":"lez.fund", "expected_public_id":"tx:one", "exact_bytes":[1]},
            {"step":"lez.fund", "expected_public_id":"tx:two", "exact_bytes":[2]}
        ]);
        assert!(serde_json::from_value::<ExactPublicEffectPlanV1>(duplicate).is_err());
    }
}
