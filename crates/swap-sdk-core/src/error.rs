//! Structured pair-error classification shared across SDK facades.

use std::error::Error;

/// Stable semantic category for a pair SDK failure.
///
/// Pair errors retain their concrete fields and source error. This category is
/// the lossless high-level classification used by applications to decide
/// whether to wait, reject input, disable a capability, or involve an operator.
/// It intentionally does not encode whether funds are lost.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[must_use]
pub enum ErrorCategory {
    /// Canonical evidence has not yet reached the required observation depth.
    ObservationLag,
    /// The exact effect is present only in a mempool or equivalent pending set.
    MempoolResidence,
    /// A required node, transport, signer, or other dependency is unavailable.
    DependencyUnavailable,
    /// Previously observed canonical evidence was removed or reorganized.
    ChainReorganization,
    /// Evidence cannot be decoded or is structurally malformed.
    MalformedEvidence,
    /// Structurally valid evidence is not canonical for the selected chain.
    NonCanonicalEvidence,
    /// Evidence or configuration identifies a different chain or network.
    WrongNetwork,
    /// Evidence or terms identify a different asset.
    WrongAsset,
    /// A lock, claim, refund, or fee has an unexpected exact value.
    WrongValue,
    /// Negotiation bytes, signatures, or transcript commitment do not match.
    TranscriptMismatch,
    /// A peer violated the accepted protocol contract.
    CounterpartyProtocolViolation,
    /// The selected pair does not implement the requested direction.
    UnsupportedDirection,
    /// The selected adapter lacks a required operation or evidence capability.
    UnsupportedCapability,
    /// Confirmation or finality settings do not meet the safety policy.
    UnsafeConfirmationProfile,
    /// Deadlines or their reaction margin do not meet the safety policy.
    UnsafeDeadlineProfile,
    /// Fee or replacement settings do not meet the safety policy.
    UnsafeFeeProfile,
    /// Required state could not become durable before a transition or effect.
    PersistenceFailure,
    /// Recovery material could not be protected or authenticated before use.
    EncryptionFailure,
    /// Automatic progress is unsafe and an operator must inspect the swap.
    OperatorInterventionRequired,
}

impl ErrorCategory {
    /// Returns the stable application-level handling class.
    pub const fn disposition(self) -> ErrorDisposition {
        match self {
            Self::ObservationLag
            | Self::MempoolResidence
            | Self::DependencyUnavailable
            | Self::ChainReorganization => ErrorDisposition::Retryable,
            Self::MalformedEvidence
            | Self::NonCanonicalEvidence
            | Self::WrongNetwork
            | Self::WrongAsset
            | Self::WrongValue
            | Self::TranscriptMismatch
            | Self::CounterpartyProtocolViolation => ErrorDisposition::Terminal,
            Self::UnsupportedDirection | Self::UnsupportedCapability => {
                ErrorDisposition::Unsupported
            }
            Self::UnsafeConfirmationProfile
            | Self::UnsafeDeadlineProfile
            | Self::UnsafeFeeProfile => ErrorDisposition::UnsafeProfile,
            Self::PersistenceFailure | Self::EncryptionFailure => {
                ErrorDisposition::LocalDurabilityFailure
            }
            Self::OperatorInterventionRequired => ErrorDisposition::OperatorInterventionRequired,
        }
    }

    /// Whether retrying after a fresh observation or dependency recovery may
    /// make progress without changing accepted terms.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(self.disposition(), ErrorDisposition::Retryable)
    }
}

/// Exhaustive application-level handling class for [`ErrorCategory`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[must_use]
pub enum ErrorDisposition {
    /// Retry only after time, fresh chain evidence, or dependency recovery.
    Retryable,
    /// Reject the evidence or peer transition permanently for these terms.
    Terminal,
    /// The pair direction or runtime adapter capability is unavailable.
    Unsupported,
    /// The selected confirmation, deadline, or fee profile is unsafe.
    UnsafeProfile,
    /// Local persistence or encryption failed before durable progress.
    LocalDurabilityFailure,
    /// Automation must stop and surface the structured pair error to an operator.
    OperatorInterventionRequired,
}

/// Structured error contract implemented by every dedicated pair SDK error.
///
/// Implementations should be enums with typed fields and `source()` values;
/// string-only adapter errors should be wrapped before crossing the SDK
/// boundary. A category does not replace the concrete error.
pub trait ProtocolError: Error + Send + Sync + 'static {
    /// Returns the shared semantic category without discarding pair detail.
    fn category(&self) -> ErrorCategory;

    /// Returns the handling class derived from [`Self::category`].
    fn disposition(&self) -> ErrorDisposition {
        self.category().disposition()
    }

    /// Whether a fresh observation or recovered dependency may make progress.
    #[must_use]
    fn is_retryable(&self) -> bool {
        self.category().is_retryable()
    }
}

#[cfg(test)]
mod tests {
    use super::{ErrorCategory, ErrorDisposition};

    #[test]
    fn every_category_has_the_expected_disposition() {
        let retryable = [
            ErrorCategory::ObservationLag,
            ErrorCategory::MempoolResidence,
            ErrorCategory::DependencyUnavailable,
            ErrorCategory::ChainReorganization,
        ];
        assert!(retryable.into_iter().all(ErrorCategory::is_retryable));

        let terminal = [
            ErrorCategory::MalformedEvidence,
            ErrorCategory::NonCanonicalEvidence,
            ErrorCategory::WrongNetwork,
            ErrorCategory::WrongAsset,
            ErrorCategory::WrongValue,
            ErrorCategory::TranscriptMismatch,
            ErrorCategory::CounterpartyProtocolViolation,
        ];
        assert!(terminal.into_iter().all(|category| {
            category.disposition() == ErrorDisposition::Terminal && !category.is_retryable()
        }));

        assert_eq!(
            ErrorCategory::UnsupportedCapability.disposition(),
            ErrorDisposition::Unsupported
        );
        assert_eq!(
            ErrorCategory::UnsafeDeadlineProfile.disposition(),
            ErrorDisposition::UnsafeProfile
        );
        assert_eq!(
            ErrorCategory::PersistenceFailure.disposition(),
            ErrorDisposition::LocalDurabilityFailure
        );
        assert_eq!(
            ErrorCategory::OperatorInterventionRequired.disposition(),
            ErrorDisposition::OperatorInterventionRequired
        );
    }
}
