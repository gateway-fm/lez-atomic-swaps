//! Deterministic lifecycle contract implemented by each pair SDK.

use crate::ProtocolError;

/// One of the two chains in a pair's claim sequence.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[must_use]
pub enum ClaimLeg {
    /// Logos Execution Zone leg.
    Lez,
    /// The pair-specific Bitcoin, Monero, or transparent Zcash leg.
    Foreign,
}

/// Explicit revealing-then-follow-up claim ordering.
///
/// The order is pair- and direction-specific. Consumers must not infer it from
/// maker/taker role names.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[must_use]
pub struct ClaimOrder {
    revealing: ClaimLeg,
    followup: ClaimLeg,
}

impl ClaimOrder {
    /// Constructs an order containing each chain exactly once.
    ///
    /// # Errors
    ///
    /// Returns [`ClaimOrderError::RepeatedLeg`] if both steps name one leg.
    pub const fn new(revealing: ClaimLeg, followup: ClaimLeg) -> Result<Self, ClaimOrderError> {
        if matches!(
            (revealing, followup),
            (ClaimLeg::Lez, ClaimLeg::Lez) | (ClaimLeg::Foreign, ClaimLeg::Foreign)
        ) {
            return Err(ClaimOrderError::RepeatedLeg);
        }
        Ok(Self {
            revealing,
            followup,
        })
    }

    /// LEZ reveals claim material before the foreign-chain follow-up.
    pub const LEZ_THEN_FOREIGN: Self = Self {
        revealing: ClaimLeg::Lez,
        followup: ClaimLeg::Foreign,
    };

    /// The foreign chain reveals claim material before the LEZ follow-up.
    pub const FOREIGN_THEN_LEZ: Self = Self {
        revealing: ClaimLeg::Foreign,
        followup: ClaimLeg::Lez,
    };

    /// Chain on which the revealing claim must occur.
    pub const fn revealing(self) -> ClaimLeg {
        self.revealing
    }

    /// Chain on which the material-consuming follow-up claim must occur.
    pub const fn followup(self) -> ClaimLeg {
        self.followup
    }
}

/// Invalid explicit claim order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ClaimOrderError {
    /// A valid two-chain sequence cannot repeat one leg.
    #[error("revealing and follow-up claims must use different chain legs")]
    RepeatedLeg,
}

/// Adapter-independent deterministic lifecycle implemented by every pair SDK.
///
/// All associated values are pair-owned typed values. In particular, callers
/// cannot turn raw terms into `ValidatedTerms`, raw node responses into
/// `ConfirmedFirstLock`, or arbitrary bytes into `RecoveredClaimMaterial`
/// without invoking the pair implementation. Methods perform no network or
/// persistence I/O, which keeps replay and model tests deterministic.
pub trait SwapProtocol {
    /// Untrusted, versioned pair terms entering validation.
    type Terms;
    /// Pair-owned terms type that can only be produced by successful validation.
    type ValidatedTerms;
    /// Fully prepared protocol state, including pre-lock recovery material.
    type Prepared;
    /// Exact first-lock transaction or multi-step effect template.
    type FirstLockTemplate;
    /// Canonical typed evidence for the first lock.
    type FirstLockEvidence;
    /// Validated first-lock fact authorizing construction of the second lock.
    type ConfirmedFirstLock;
    /// Exact second-lock transaction or multi-step effect template.
    type SecondLockTemplate;
    /// Canonical typed evidence for the claim that reveals recovery material.
    type RevealingClaimEvidence;
    /// Pair-owned material extracted and verified from canonical claim evidence.
    type RecoveredClaimMaterial;
    /// Exact material-consuming follow-up claim template.
    type FollowupClaimTemplate;
    /// Canonical typed state used to select a recovery action.
    type CanonicalChainState;
    /// Pair-specific recovery action or explicit wait/operator outcome.
    type RecoveryAction;
    /// Structured pair error preserving typed context and sources.
    type Error: ProtocolError;

    /// Validates complete terms before any preparation or public effect.
    ///
    /// # Errors
    ///
    /// Returns a structured pair error for invalid versions, signatures,
    /// direction, identities, values, or safety profiles.
    fn validate_terms(&self, terms: &Self::Terms) -> Result<Self::ValidatedTerms, Self::Error>;

    /// Prepares all required pre-lock recovery material.
    ///
    /// # Errors
    ///
    /// Returns a structured pair error if complete recoverability cannot be
    /// established before the first public effect.
    fn prepare(&self, terms: Self::ValidatedTerms) -> Result<Self::Prepared, Self::Error>;

    /// Builds the taker-funded first-lock template.
    ///
    /// # Errors
    ///
    /// Returns a structured pair error when exact construction from the
    /// prepared terms fails.
    fn build_first_lock(
        &self,
        prepared: &Self::Prepared,
    ) -> Result<Self::FirstLockTemplate, Self::Error>;

    /// Validates canonical first-lock evidence against the prepared terms.
    ///
    /// # Errors
    ///
    /// Returns a structured pair error for malformed, non-canonical,
    /// insufficiently confirmed, or agreement-mismatched evidence.
    fn validate_first_lock(
        &self,
        prepared: &Self::Prepared,
        evidence: &Self::FirstLockEvidence,
    ) -> Result<Self::ConfirmedFirstLock, Self::Error>;

    /// Builds the maker-funded second lock only from a confirmed first lock.
    ///
    /// # Errors
    ///
    /// Returns a structured pair error when the first-lock proof or exact
    /// second-lock construction no longer satisfies accepted terms.
    fn build_second_lock(
        &self,
        prepared: &Self::Prepared,
        first: &Self::ConfirmedFirstLock,
    ) -> Result<Self::SecondLockTemplate, Self::Error>;

    /// Returns the explicit pair- and direction-specific claim order.
    fn claim_order(&self, prepared: &Self::Prepared) -> ClaimOrder;

    /// Extracts and verifies claim material from canonical revealing evidence.
    ///
    /// # Errors
    ///
    /// Returns a structured pair error when evidence is not canonical or the
    /// extracted material fails the pair-specific cryptographic relation.
    fn validate_revealing_claim(
        &self,
        prepared: &Self::Prepared,
        evidence: &Self::RevealingClaimEvidence,
    ) -> Result<Self::RecoveredClaimMaterial, Self::Error>;

    /// Builds the second claim from previously verified recovered material.
    ///
    /// # Errors
    ///
    /// Returns a structured pair error if the material or exact follow-up
    /// construction does not match the prepared swap.
    fn build_followup_claim(
        &self,
        prepared: &Self::Prepared,
        material: &Self::RecoveredClaimMaterial,
    ) -> Result<Self::FollowupClaimTemplate, Self::Error>;

    /// Selects a construction-ordered recovery action from canonical chain state.
    ///
    /// # Errors
    ///
    /// Returns a structured pair error for contradictory evidence, an unsafe
    /// recovery boundary, or a state requiring operator intervention.
    fn recovery_action(
        &self,
        prepared: &Self::Prepared,
        state: &Self::CanonicalChainState,
    ) -> Result<Self::RecoveryAction, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::{ClaimLeg, ClaimOrder, ClaimOrderError};

    #[test]
    fn claim_order_requires_both_legs() {
        assert_eq!(
            ClaimOrder::new(ClaimLeg::Lez, ClaimLeg::Lez),
            Err(ClaimOrderError::RepeatedLeg)
        );
        assert_eq!(
            ClaimOrder::new(ClaimLeg::Foreign, ClaimLeg::Lez),
            Ok(ClaimOrder::FOREIGN_THEN_LEZ)
        );
    }
}
