//! Pre-lock and post-lock LEZ/ZEC SDK facades.

use lez_swap_core::{Participant, Phase, SwapId, UnixSeconds};

use crate::{
    AcceptedZecAgreementV1, CreateAgreementOutcome, NegotiationChannel, OfferDiscovery,
    RecoveryStore, ZecAgreementV1, ZecLifecycleAction, ZecSdkError, lifecycle::next_action,
};

/// Complete pre-lock facade composed from application-supplied ports.
///
/// Activation returns an [`ActiveZecSwap`], whose type deliberately omits the
/// discovery and negotiation handles.
pub struct ZecPairSdk<Discovery, Negotiation, Lez, Zcash, Store> {
    local_participant: Participant,
    discovery: Discovery,
    negotiation: Negotiation,
    lez: Lez,
    zcash: Zcash,
    store: Store,
}

impl<Discovery, Negotiation, Lez, Zcash, Store> std::fmt::Debug
    for ZecPairSdk<Discovery, Negotiation, Lez, Zcash, Store>
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ZecPairSdk")
            .field("local_participant", &self.local_participant)
            .field("capabilities", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl<Discovery, Negotiation, Lez, Zcash, Store>
    ZecPairSdk<Discovery, Negotiation, Lez, Zcash, Store>
{
    /// Composes a role-fixed SDK from narrow application capabilities.
    #[must_use]
    pub const fn new(
        local_participant: Participant,
        discovery: Discovery,
        negotiation: Negotiation,
        lez: Lez,
        zcash: Zcash,
        store: Store,
    ) -> Self {
        Self {
            local_participant,
            discovery,
            negotiation,
            lez,
            zcash,
            store,
        }
    }

    /// Participant fixed for every operation on this instance.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }
}

impl<Discovery, Negotiation, Lez, Zcash, Store>
    ZecPairSdk<Discovery, Negotiation, Lez, Zcash, Store>
where
    Discovery: OfferDiscovery,
    Negotiation: NegotiationChannel<OfferRef = Discovery::OfferRef>,
    Lez: Clone,
    Zcash: Clone,
    Store: RecoveryStore,
{
    /// Publishes a maker offer through the configured authenticated adapter.
    ///
    /// # Errors
    ///
    /// Returns [`ZecSdkError::WrongRole`] for a taker SDK or preserves the
    /// structured discovery-adapter source.
    pub async fn publish_offer(
        &self,
        offer: Discovery::Offer,
    ) -> Result<Discovery::OfferRef, ZecSdkError> {
        self.require_role(Participant::Maker)?;
        self.discovery
            .publish(offer)
            .await
            .map_err(|error| ZecSdkError::Discovery(Box::new(error)))
    }

    /// Discovers authenticated, unexpired offers without changing swap state.
    ///
    /// # Errors
    ///
    /// Preserves the structured discovery-adapter source.
    pub async fn discover_offers(
        &self,
        query: &Discovery::Query,
    ) -> Result<Vec<Discovery::OfferRef>, ZecSdkError> {
        self.discovery
            .discover(query)
            .await
            .map_err(|error| ZecSdkError::Discovery(Box::new(error)))
    }

    /// Obtains untrusted wire and validates it at a trusted local wall clock.
    ///
    /// The accepted role is fixed by this SDK and the initial durable revision
    /// is always zero. Callers cannot substitute either value.
    ///
    /// # Errors
    ///
    /// Preserves negotiation failures and returns [`ZecSdkError::InvalidAgreement`]
    /// for every bounded-wire, signature, profile, identity, or expiry failure.
    pub async fn negotiate_at(
        &self,
        offer: &Discovery::OfferRef,
        proposal: Negotiation::LocalProposal,
        accepted_at: UnixSeconds,
    ) -> Result<AcceptedZecAgreementV1, ZecSdkError> {
        let wire = self
            .negotiation
            .negotiate(self.local_participant, offer, proposal)
            .await
            .map_err(|error| ZecSdkError::Negotiation(Box::new(error)))?;
        AcceptedZecAgreementV1::accept_wire_at(&wire, accepted_at, self.local_participant, 0)
            .map_err(ZecSdkError::from)
    }

    /// Persists immutable accepted terms before returning post-lock capability.
    ///
    /// Exact retry is idempotent. A changed agreement under the same role-local
    /// swap key fails closed. The return type has no discovery or negotiation
    /// generic parameters, so those transports cannot become post-lock
    /// dependencies.
    ///
    /// # Errors
    ///
    /// Rejects a substituted role or nonzero initial revision, reports a
    /// conflict distinctly, and returns persistence errors before an active
    /// value exists.
    pub async fn activate(
        &self,
        accepted: AcceptedZecAgreementV1,
    ) -> Result<ActiveZecSwap<Lez, Zcash, Store>, ZecSdkError> {
        self.validate_local_role(&accepted)?;
        if accepted.revision() != 0 {
            return Err(ZecSdkError::InvalidActivationRevision(accepted.revision()));
        }
        let envelope = accepted.durable_envelope()?;
        let outcome = self
            .store
            .create_agreement(&envelope)
            .await
            .map_err(|error| ZecSdkError::Persistence(Box::new(error)))?;
        match outcome {
            CreateAgreementOutcome::Created | CreateAgreementOutcome::ExistingSame => {
                Ok(self.active(accepted))
            }
            CreateAgreementOutcome::Conflict => Err(ZecSdkError::AgreementConflict),
        }
    }

    /// Resumes a role-local active swap without consulting discovery or Chat.
    ///
    /// Durable wire is fully revalidated at the original trusted acceptance
    /// time, so an honestly accepted record remains resumable after transcript
    /// expiry. The requested ID, fixed role, commitment, signatures, and
    /// durable revision are checked independently of the store lookup.
    ///
    /// # Errors
    ///
    /// Preserves the structured recovery-store source and rejects every
    /// mismatched or malformed durable field.
    pub async fn resume(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<ActiveZecSwap<Lez, Zcash, Store>>, ZecSdkError> {
        let Some(envelope) = self
            .store
            .load_agreement(swap_id)
            .await
            .map_err(|error| ZecSdkError::Persistence(Box::new(error)))?
        else {
            return Ok(None);
        };
        let accepted = AcceptedZecAgreementV1::resume(&envelope)?;
        self.validate_local_role(&accepted)?;
        let actual = accepted.agreement().coordinator().id();
        if actual != swap_id {
            return Err(ZecSdkError::AgreementIdentityMismatch {
                requested: swap_id.clone(),
                actual: actual.clone(),
            });
        }
        Ok(Some(self.active(accepted)))
    }

    fn active(&self, accepted: AcceptedZecAgreementV1) -> ActiveZecSwap<Lez, Zcash, Store> {
        ActiveZecSwap {
            accepted,
            _lez: self.lez.clone(),
            _zcash: self.zcash.clone(),
            _store: self.store.clone(),
        }
    }

    fn validate_local_role(&self, accepted: &AcceptedZecAgreementV1) -> Result<(), ZecSdkError> {
        let actual = accepted.local_participant();
        if actual == self.local_participant {
            Ok(())
        } else {
            Err(ZecSdkError::LocalRoleMismatch {
                expected: self.local_participant,
                actual,
            })
        }
    }

    fn require_role(&self, expected: Participant) -> Result<(), ZecSdkError> {
        if self.local_participant == expected {
            Ok(())
        } else {
            Err(ZecSdkError::WrongRole {
                expected,
                actual: self.local_participant,
            })
        }
    }
}

/// Post-lock SDK state containing only concrete accepted terms and private
/// chain/recovery capabilities.
///
/// Raw adapters are deliberately not recoverable from this public type:
///
/// ```compile_fail
/// use lez_zec_swap_sdk::ActiveZecSwap;
///
/// fn cannot_escape<L, Z, S>(active: &ActiveZecSwap<L, Z, S>) {
///     let _ = active.lez_port();
///     let _ = active.zcash_port();
///     let _ = active.recovery_store();
/// }
/// ```
pub struct ActiveZecSwap<Lez, Zcash, Store> {
    accepted: AcceptedZecAgreementV1,
    _lez: Lez,
    _zcash: Zcash,
    _store: Store,
}

impl<Lez, Zcash, Store> std::fmt::Debug for ActiveZecSwap<Lez, Zcash, Store> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActiveZecSwap")
            .field("local_participant", &self.local_participant())
            .field("agreement", &"[REDACTED]")
            .field("revision", &self.revision())
            .field("capabilities", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl<Lez, Zcash, Store> ActiveZecSwap<Lez, Zcash, Store> {
    /// Fixed local participant; action methods never accept an arbitrary role.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.accepted.local_participant()
    }

    /// Immutable concrete dual-signed agreement.
    #[must_use]
    pub const fn agreement(&self) -> &ZecAgreementV1 {
        self.accepted.agreement()
    }

    /// Trusted original acceptance time used by durable revalidation.
    #[must_use]
    pub const fn accepted_at(&self) -> UnixSeconds {
        self.accepted.accepted_at()
    }

    /// Current deterministic protocol phase.
    #[must_use]
    pub const fn status(&self) -> Phase {
        self.agreement().coordinator().phase()
    }

    /// Last role-local durable revision loaded during activation/resume.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.accepted.revision()
    }

    /// Next construction-specific role action, if one is currently safe.
    #[must_use]
    pub fn next_action(&self) -> ZecLifecycleAction {
        next_action(self.agreement().coordinator(), self.local_participant())
    }
}
