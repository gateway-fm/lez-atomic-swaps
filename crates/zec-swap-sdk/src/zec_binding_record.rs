//! Versioned immutable ZEC profile and expected-output bindings.

use serde::{Deserialize, Serialize};
use zcash_protocol::{consensus::BranchId, value::Zatoshis};

use crate::{
    Bip199Contract, CanonicalZcashOutputObservation, ExpectedBip199Output, ProfileError,
    ZcashNetworkRecordV1, ZcashObservationEvent, ZecProfileId, ZecRefundProfile,
};

/// Stable primitive spelling of a reviewed ZEC profile.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZecProfileRecordV1 {
    /// Controlled standalone/Regtest profile.
    DeterministicLocalV1,
    /// LEZ and Zcash public-testnet acceptance profile.
    PublicTestnetV1,
}

impl From<ZecProfileId> for ZecProfileRecordV1 {
    fn from(value: ZecProfileId) -> Self {
        match value {
            ZecProfileId::DeterministicLocalV1 => Self::DeterministicLocalV1,
            ZecProfileId::PublicTestnetV1 => Self::PublicTestnetV1,
        }
    }
}

impl From<ZecProfileRecordV1> for ZecProfileId {
    fn from(value: ZecProfileRecordV1) -> Self {
        match value {
            ZecProfileRecordV1::DeterministicLocalV1 => Self::DeterministicLocalV1,
            ZecProfileRecordV1::PublicTestnetV1 => Self::PublicTestnetV1,
        }
    }
}

/// Trusted immutable binding reconstructed only after primitive revalidation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZecSwapBinding {
    profile_id: ZecProfileId,
    expected_output: ExpectedBip199Output,
}

impl ZecSwapBinding {
    /// Constructs a binding only when profile, network, and consensus branch agree.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileError`] when the expected output is incompatible with the
    /// selected immutable profile.
    pub fn new(
        profile_id: ZecProfileId,
        expected_output: ExpectedBip199Output,
    ) -> Result<Self, ProfileError> {
        ZecRefundProfile::for_id(profile_id).validate_consensus(
            expected_output.network(),
            expected_output.consensus_branch_id(),
        )?;
        Ok(Self {
            profile_id,
            expected_output,
        })
    }

    /// Reviewed immutable profile identifier.
    #[must_use]
    pub const fn profile_id(&self) -> ZecProfileId {
        self.profile_id
    }

    /// Exact negotiated BIP-199 output envelope.
    #[must_use]
    pub const fn expected_output(&self) -> &ExpectedBip199Output {
        &self.expected_output
    }

    /// Checks every canonical side of an event against negotiated output terms.
    ///
    /// Removal validates its previous observation; replacement validates both the
    /// removed previous observation and the new canonical output envelope. The
    /// outpoint is deliberately not part of this binding so conflicting chain
    /// replacements can still be journaled and classified safely.
    ///
    /// # Errors
    ///
    /// Returns [`ZecBindingRecordError::EventMismatch`] when network, branch,
    /// value, redeem script, or P2SH output differs from negotiated terms.
    pub fn validate_event(
        &self,
        event: &ZcashObservationEvent,
    ) -> Result<(), ZecBindingRecordError> {
        match event {
            ZcashObservationEvent::Canonical(canonical) => self.validate_canonical(canonical),
            ZcashObservationEvent::Removed(removed) => self.validate_canonical(removed.previous()),
            ZcashObservationEvent::Replaced { removed, canonical } => {
                self.validate_canonical(removed.previous())?;
                self.validate_canonical(canonical)
            }
        }
    }

    fn validate_canonical(
        &self,
        observation: &CanonicalZcashOutputObservation,
    ) -> Result<(), ZecBindingRecordError> {
        let expected = self.expected_output();
        let contract = expected.contract();
        if observation.network() != expected.network()
            || observation.consensus_branch_id() != expected.consensus_branch_id()
            || observation.output().value() != expected.value()
            || observation.output().script_pubkey().0.0 != contract.p2sh_script_pubkey()
            || observation.redeem_script() != contract.redeem_script()
            || observation.p2sh_script_pubkey() != contract.p2sh_script_pubkey()
        {
            return Err(ZecBindingRecordError::EventMismatch);
        }
        Ok(())
    }
}

/// Primitive expected-output payload retained in binding record version 1.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ExpectedBip199OutputRecordV1 {
    network: ZcashNetworkRecordV1,
    consensus_branch_id: u32,
    value_zatoshis: u64,
    refund_lock_time: u32,
    refund_pubkey_hash: [u8; 20],
    secret_digest: [u8; 32],
    claimant_pubkey_hash: [u8; 20],
    redeem_script: Vec<u8>,
    p2sh_script_pubkey: Vec<u8>,
}

/// Version-1 persistent ZEC profile and expected-output binding.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ZecSwapBindingRecordV1 {
    profile_id: ZecProfileRecordV1,
    expected_output: ExpectedBip199OutputRecordV1,
}

impl ZecSwapBindingRecordV1 {
    /// Encodes one already validated trusted binding as primitives.
    #[must_use]
    pub fn from_binding(value: &ZecSwapBinding) -> Self {
        let expected = value.expected_output();
        let contract = expected.contract();
        Self {
            profile_id: value.profile_id().into(),
            expected_output: ExpectedBip199OutputRecordV1 {
                network: expected.network().into(),
                consensus_branch_id: expected.consensus_branch_id().into(),
                value_zatoshis: u64::from(expected.value()),
                refund_lock_time: contract.refund_lock_time(),
                refund_pubkey_hash: contract.refund_pubkey_hash(),
                secret_digest: contract.secret_digest(),
                claimant_pubkey_hash: contract.claimant_pubkey_hash(),
                redeem_script: contract.redeem_script().to_vec(),
                p2sh_script_pubkey: contract.p2sh_script_pubkey().to_vec(),
            },
        }
    }

    /// Rebuilds all derived script bytes and checks profile consensus binding.
    ///
    /// # Errors
    ///
    /// Returns [`ZecBindingRecordError`] for unknown/invalid primitives, derived
    /// script drift, or a profile/network/branch mismatch.
    pub fn validate(&self) -> Result<ZecSwapBinding, ZecBindingRecordError> {
        let expected = &self.expected_output;
        let branch = BranchId::try_from(expected.consensus_branch_id)
            .map_err(|_| ZecBindingRecordError::UnknownBranch)?;
        let value = Zatoshis::from_u64(expected.value_zatoshis)
            .map_err(|_| ZecBindingRecordError::InvalidValue)?;
        let contract = Bip199Contract::new(
            expected.refund_lock_time,
            expected.refund_pubkey_hash,
            expected.secret_digest,
            expected.claimant_pubkey_hash,
        );
        if contract.redeem_script() != expected.redeem_script
            || contract.p2sh_script_pubkey() != expected.p2sh_script_pubkey
        {
            return Err(ZecBindingRecordError::DerivedScriptMismatch);
        }
        let output = ExpectedBip199Output::new(expected.network.into(), branch, value, contract);
        ZecSwapBinding::new(self.profile_id.into(), output).map_err(ZecBindingRecordError::from)
    }
}

/// Corrupt or incompatible immutable ZEC binding record.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ZecBindingRecordError {
    /// Consensus branch primitive is unknown.
    #[error("persisted ZEC binding uses an unknown consensus branch")]
    UnknownBranch,
    /// Zatoshi value exceeds the consensus range.
    #[error("persisted ZEC binding value is invalid")]
    InvalidValue,
    /// Persisted derived scripts disagree with the BIP-199 source terms.
    #[error("persisted ZEC binding scripts are inconsistent")]
    DerivedScriptMismatch,
    /// Observation event differs from the immutable expected-output envelope.
    #[error("Zcash observation does not match the immutable swap binding")]
    EventMismatch,
    /// Selected profile disagrees with the expected network or branch.
    #[error("persisted ZEC binding does not match its profile")]
    Profile(#[from] ProfileError),
}
