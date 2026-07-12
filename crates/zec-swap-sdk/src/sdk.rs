//! Pre-lock and post-lock LEZ/ZEC SDK facades.

use lez_swap_core::{Participant, Phase, SwapCoordinator, SwapId, UnixSeconds};

use crate::{
    AcceptedZecAgreementV1, CreateAgreementOutcome, CreateFirstLockOutcome,
    FirstLockConfirmedEvidenceV1, FirstLockDriveOutcome, FirstLockIntentV1, FirstLockObservation,
    FirstLockPlanV1, FirstLockProjectionCommit, FirstLockTransitionV1, LezFirstLockPort,
    LezObservationEventV1, LezObservationReconciliationV1, LezObservationTrackerV1,
    LezTakerFirstLockObservationPort, MakerFundingEligibilityOutcome, NegotiationChannel,
    ObserveTakerFirstLockOutcome, ObservedTakerFirstLockTransitionV1, OfferDiscovery,
    PreparedFirstLockSubmissionV1, RecoveryStore, TakerFirstLockObservationV1, ZcashFirstLockPort,
    ZcashObservationEvent, ZcashObservationReconciliation, ZcashObservationTracker,
    ZcashTakerFirstLockObservationPort, ZecAgreementV1, ZecLifecycleAction, ZecSdkError,
    lifecycle::next_action, observed_taker_lock::taker_first_lock_step,
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
        let mut active = self.active(accepted);
        active.replay_first_lock_transition().await?;
        Ok(Some(active))
    }

    fn active(&self, accepted: AcceptedZecAgreementV1) -> ActiveZecSwap<Lez, Zcash, Store> {
        let coordinator = accepted.agreement().coordinator().clone();
        let revision = accepted.revision();
        ActiveZecSwap {
            accepted,
            coordinator,
            revision,
            zcash_taker_lock_tracker: ZcashObservationTracker::default(),
            lez_taker_lock_tracker: LezObservationTrackerV1::default(),
            lez: self.lez.clone(),
            zcash: self.zcash.clone(),
            store: self.store.clone(),
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
    coordinator: SwapCoordinator,
    revision: u64,
    zcash_taker_lock_tracker: ZcashObservationTracker,
    lez_taker_lock_tracker: LezObservationTrackerV1,
    lez: Lez,
    zcash: Zcash,
    store: Store,
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
        self.coordinator.phase()
    }

    /// Last role-local durable revision loaded during activation/resume.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Next construction-specific role action, if one is currently safe.
    ///
    /// A maker-local first-lock observation is deliberately non-authorizing
    /// until a fresh canonical reorg-safe eligibility check exists. Therefore
    /// the active SDK returns Wait after replaying or committing that
    /// observation instead of advertising the second lock.
    #[must_use]
    pub fn next_action(&self) -> ZecLifecycleAction {
        if self.local_participant() == Participant::Maker
            && self.status() == Phase::TakerLockConfirmed
        {
            return ZecLifecycleAction::Wait;
        }
        next_action(&self.coordinator, self.local_participant())
    }
}

impl<Lez, Zcash, Store> ActiveZecSwap<Lez, Zcash, Store>
where
    Store: RecoveryStore,
{
    /// Atomically projects confirmed final first-lock evidence, probing an exact
    /// predecessor slot after an unknown store outcome.
    ///
    /// In-memory coordinator state changes only after the store proves the
    /// exact transition durable.
    ///
    /// # Errors
    ///
    /// Rejects invalid/sub-threshold evidence, context drift, a missing intent,
    /// an invalid store revision, or a structured persistence failure.
    pub async fn project_first_lock(
        &mut self,
        evidence: FirstLockConfirmedEvidenceV1,
    ) -> Result<FirstLockProjectionCommit, ZecSdkError> {
        let swap_id = self.coordinator.id().clone();
        let Some(intent) = self
            .store
            .load_first_lock_intent(&swap_id)
            .await
            .map_err(|error| ZecSdkError::Persistence(Box::new(error)))?
        else {
            return Err(ZecSdkError::MissingFirstLockIntent);
        };
        let transition =
            FirstLockTransitionV1::from_active(self.agreement(), &intent, self.revision, evidence)?;
        let next = transition.apply_to(self.agreement(), &self.coordinator, self.revision)?;
        let expected_revision =
            self.revision
                .checked_add(1)
                .ok_or(ZecSdkError::InvalidProjectionRevision {
                    expected: u64::MAX,
                    actual: u64::MAX,
                })?;

        let commit = match self.store.commit_first_lock_transition(&transition).await {
            Ok(commit) => commit,
            Err(error) => {
                let probe = self
                    .store
                    .load_first_lock_transition(&swap_id, self.revision)
                    .await
                    .map_err(|probe_error| ZecSdkError::Persistence(Box::new(probe_error)))?;
                if probe.as_ref() == Some(&transition) {
                    FirstLockProjectionCommit::new(expected_revision, true)
                } else {
                    return Err(ZecSdkError::Persistence(Box::new(error)));
                }
            }
        };
        if commit.revision() != expected_revision {
            return Err(ZecSdkError::InvalidProjectionRevision {
                expected: expected_revision,
                actual: commit.revision(),
            });
        }
        self.coordinator = next;
        self.revision = commit.revision();
        Ok(commit)
    }

    async fn replay_first_lock_transition(&mut self) -> Result<(), ZecSdkError> {
        if self.local_participant() == Participant::Maker {
            return self.replay_observed_taker_first_lock_transition().await;
        }
        let Some(transition) = self
            .store
            .load_first_lock_transition(self.coordinator.id(), self.revision)
            .await
            .map_err(|error| ZecSdkError::Persistence(Box::new(error)))?
        else {
            return Ok(());
        };
        let next = transition.apply_to(self.agreement(), &self.coordinator, self.revision)?;
        self.revision =
            self.revision
                .checked_add(1)
                .ok_or(ZecSdkError::InvalidProjectionRevision {
                    expected: u64::MAX,
                    actual: u64::MAX,
                })?;
        self.coordinator = next;
        Ok(())
    }

    async fn replay_observed_taker_first_lock_transition(&mut self) -> Result<(), ZecSdkError> {
        loop {
            let Some(transition) = self
                .store
                .load_observed_taker_first_lock_transition(self.coordinator.id(), self.revision)
                .await
                .map_err(|error| ZecSdkError::Persistence(Box::new(error)))?
            else {
                return Ok(());
            };
            let next_zcash_tracker = self.apply_committed_zcash_event(&transition)?;
            let next_lez_tracker = self.apply_committed_lez_event(&transition)?;
            let next = transition.apply_to(self.agreement(), &self.coordinator, self.revision)?;
            self.revision =
                self.revision
                    .checked_add(1)
                    .ok_or(ZecSdkError::InvalidProjectionRevision {
                        expected: u64::MAX,
                        actual: u64::MAX,
                    })?;
            self.coordinator = next;
            self.zcash_taker_lock_tracker = next_zcash_tracker;
            self.lez_taker_lock_tracker = next_lez_tracker;
        }
    }

    fn apply_committed_zcash_event(
        &self,
        transition: &ObservedTakerFirstLockTransitionV1,
    ) -> Result<ZcashObservationTracker, ZecSdkError> {
        let mut next = self.zcash_taker_lock_tracker.clone();
        if let Some(event) = transition.zcash_observation_event() {
            next.apply_committed(&event)?;
        }
        Ok(next)
    }

    fn propose_zcash_event(
        &self,
        transition: &ObservedTakerFirstLockTransitionV1,
    ) -> Result<Option<ZcashObservationTracker>, ZecSdkError> {
        let Some(event) = transition.zcash_observation_event() else {
            return Ok(Some(self.zcash_taker_lock_tracker.clone()));
        };
        let input = match &event {
            ZcashObservationEvent::Canonical(canonical) => {
                ZcashObservationReconciliation::Canonical(canonical.clone())
            }
            ZcashObservationEvent::Removed(removed) => {
                ZcashObservationReconciliation::Removed(removed.clone())
            }
            ZcashObservationEvent::Replaced { removed, canonical } => {
                ZcashObservationReconciliation::Replaced {
                    removed: removed.clone(),
                    canonical: canonical.clone(),
                }
            }
        };
        if self.zcash_taker_lock_tracker.propose(&input)?.is_none() {
            return Ok(None);
        }
        Ok(Some(self.apply_committed_zcash_event(transition)?))
    }

    fn apply_committed_lez_event(
        &self,
        transition: &ObservedTakerFirstLockTransitionV1,
    ) -> Result<LezObservationTrackerV1, ZecSdkError> {
        let mut next = self.lez_taker_lock_tracker.clone();
        if let Some(event) = transition.lez_observation_event() {
            next.apply_committed(&event)?;
        }
        Ok(next)
    }

    fn propose_lez_event(
        &self,
        transition: &ObservedTakerFirstLockTransitionV1,
    ) -> Result<Option<LezObservationTrackerV1>, ZecSdkError> {
        let Some(event) = transition.lez_observation_event() else {
            return Ok(Some(self.lez_taker_lock_tracker.clone()));
        };
        let input = match &event {
            LezObservationEventV1::Canonical(canonical) => {
                LezObservationReconciliationV1::Canonical(canonical.clone())
            }
            LezObservationEventV1::Removed(removed) => {
                LezObservationReconciliationV1::Removed(removed.clone())
            }
            LezObservationEventV1::Replaced { removed, canonical } => {
                LezObservationReconciliationV1::Replaced {
                    removed: removed.clone(),
                    canonical: canonical.clone(),
                }
            }
        };
        if self.lez_taker_lock_tracker.propose(&input)?.is_none() {
            return Ok(None);
        }
        Ok(Some(self.apply_committed_lez_event(transition)?))
    }

    /// Stages exact role-local first-lock recovery material before any node call.
    ///
    /// This method performs no chain effect and never advances the coordinator.
    /// Exact retry is idempotent; changed material under the same swap key
    /// conflicts. The signed direction selects Zcash funding or the ordered LEZ
    /// initialize/fund pair, so callers cannot choose the first-lock chain.
    ///
    /// # Errors
    ///
    /// Rejects the maker role, a direction/plan mismatch, malformed material,
    /// a durable conflict, or a structured store error.
    pub async fn stage_first_lock(
        &self,
        plan: FirstLockPlanV1,
    ) -> Result<CreateFirstLockOutcome, ZecSdkError> {
        if self.local_participant() != Participant::Taker {
            return Err(ZecSdkError::WrongRole {
                expected: Participant::Taker,
                actual: self.local_participant(),
            });
        }
        if self.status() != Phase::Offered {
            return Err(ZecSdkError::FirstLockNotOffered(self.status()));
        }
        let intent = FirstLockIntentV1::from_active(
            self.agreement(),
            self.local_participant(),
            self.revision(),
            plan,
        )?;
        let outcome = self
            .store
            .create_first_lock_intent(&intent)
            .await
            .map_err(|error| ZecSdkError::Persistence(Box::new(error)))?;
        match outcome {
            CreateFirstLockOutcome::Created | CreateFirstLockOutcome::ExistingSame => Ok(outcome),
            CreateFirstLockOutcome::Conflict => Err(ZecSdkError::FirstLockConflict),
        }
    }
}

impl<Lez, Zcash, Store> ActiveZecSwap<Lez, Zcash, Store>
where
    Lez: LezTakerFirstLockObservationPort,
    Zcash: ZcashTakerFirstLockObservationPort,
    Store: RecoveryStore,
{
    /// Observes and durably projects the taker's first lock from the maker's
    /// own selected chain node.
    ///
    /// Absence and unstable evidence never advance state. Confirmed evidence
    /// commits before in-memory apply, with an exact predecessor-slot probe
    /// after an unknown store outcome.
    ///
    /// # Errors
    ///
    /// Rejects the taker role, wrong-chain or insufficient evidence, invalid
    /// phase/revision, and structured chain/store failures.
    pub async fn observe_taker_first_lock(
        &mut self,
    ) -> Result<ObserveTakerFirstLockOutcome, ZecSdkError> {
        if self.local_participant() != Participant::Maker {
            return Err(ZecSdkError::WrongRole {
                expected: Participant::Maker,
                actual: self.local_participant(),
            });
        }
        self.replay_observed_taker_first_lock_transition().await?;
        if self.status() != Phase::Offered
            && !matches!(
                self.status(),
                Phase::AwaitingTakerConfirmations | Phase::TakerLockConfirmed
            )
        {
            return Err(ZecSdkError::FirstLockNotOffered(self.status()));
        }
        let step = taker_first_lock_step(self.agreement().direction());
        let observation = match step {
            crate::FirstLockStepV1::ZcashFund => self
                .zcash
                .observe_taker_first_lock(self.agreement())
                .await
                .map_err(|error| ZecSdkError::ZcashTakerFirstLockObservation(Box::new(error)))?,
            crate::FirstLockStepV1::LezFund => self
                .lez
                .observe_taker_first_lock(self.agreement(), self.lez_taker_lock_tracker.current())
                .await
                .map_err(|error| ZecSdkError::LezTakerFirstLockObservation(Box::new(error)))?,
            crate::FirstLockStepV1::LezInitialize => unreachable!("not a final lock step"),
        };
        let Some(evidence) = observed_taker_lock_evidence(observation) else {
            return Ok(ObserveTakerFirstLockOutcome::AwaitingStableObservation(
                step,
            ));
        };
        let transition = ObservedTakerFirstLockTransitionV1::from_active(
            self.agreement(),
            self.local_participant(),
            self.revision,
            evidence,
        )?;
        let Some(next_tracker) = self.propose_zcash_event(&transition)? else {
            return Ok(ObserveTakerFirstLockOutcome::Unchanged(step));
        };
        let Some(next_lez_tracker) = self.propose_lez_event(&transition)? else {
            return Ok(ObserveTakerFirstLockOutcome::Unchanged(step));
        };
        let next = transition.apply_to(self.agreement(), &self.coordinator, self.revision)?;
        let expected_revision =
            self.revision
                .checked_add(1)
                .ok_or(ZecSdkError::InvalidProjectionRevision {
                    expected: u64::MAX,
                    actual: u64::MAX,
                })?;
        let commit = match self
            .store
            .commit_observed_taker_first_lock_transition(&transition)
            .await
        {
            Ok(commit) => commit,
            Err(error) => {
                let probe = self
                    .store
                    .load_observed_taker_first_lock_transition(self.coordinator.id(), self.revision)
                    .await
                    .map_err(|probe_error| ZecSdkError::Persistence(Box::new(probe_error)))?;
                if probe.as_ref() == Some(&transition) {
                    FirstLockProjectionCommit::new(expected_revision, true)
                } else {
                    return Err(ZecSdkError::Persistence(Box::new(error)));
                }
            }
        };
        if commit.revision() != expected_revision {
            return Err(ZecSdkError::InvalidProjectionRevision {
                expected: expected_revision,
                actual: commit.revision(),
            });
        }
        self.coordinator = next;
        self.revision = commit.revision();
        self.zcash_taker_lock_tracker = next_tracker;
        self.lez_taker_lock_tracker = next_lez_tracker;
        self.replay_observed_taker_first_lock_transition().await?;
        Ok(ObserveTakerFirstLockOutcome::Projected(commit))
    }

    /// Performs the distinct fresh canonical check required immediately before
    /// a future maker second-lock effect.
    ///
    /// Eligibility is deliberately not cached and does not change
    /// `next_action`. A maker effect must invoke this boundary itself and consume
    /// an Eligible result in the same operation. Any newly observed
    /// depth/reorg state is durably projected first and requires a subsequent
    /// fresh poll.
    ///
    /// # Errors
    ///
    /// Rejects the taker role, invalid exact-head history or policy evidence,
    /// and structured port/store failures.
    pub async fn refresh_maker_funding_eligibility(
        &mut self,
    ) -> Result<MakerFundingEligibilityOutcome, ZecSdkError> {
        if self.local_participant() != Participant::Maker {
            return Err(ZecSdkError::WrongRole {
                expected: Participant::Maker,
                actual: self.local_participant(),
            });
        }
        let direction = self.agreement().direction();
        match self.observe_taker_first_lock().await? {
            ObserveTakerFirstLockOutcome::Unchanged(_) => Ok(match direction {
                lez_swap_core::SwapDirection::TakerSellsForeign => {
                    classify_unchanged_maker_eligibility(self.status(), self.revision)
                }
                lez_swap_core::SwapDirection::TakerSellsLez => {
                    classify_unchanged_lez_maker_eligibility(
                        self.status(),
                        self.revision,
                        self.coordinator.required_confirmations(Participant::Taker),
                        self.agreement().lez_terms().chain().environment(),
                        self.lez_taker_lock_tracker.current().map(|current| {
                            (current.confirmations().get(), current.inclusion_status())
                        }),
                    )
                }
            }),
            ObserveTakerFirstLockOutcome::AwaitingStableObservation(step) => Ok(
                MakerFundingEligibilityOutcome::AwaitingStableObservation(step),
            ),
            ObserveTakerFirstLockOutcome::Projected(commit) => Ok(
                MakerFundingEligibilityOutcome::CanonicalStateChanged(commit),
            ),
        }
    }
}

fn observed_taker_lock_evidence(
    observation: TakerFirstLockObservationV1,
) -> Option<crate::ObservedTakerFirstLockEvidenceV1> {
    match observation {
        TakerFirstLockObservationV1::Confirmed(evidence) => Some(evidence),
        TakerFirstLockObservationV1::CanonicalLez(canonical) => Some(
            crate::ObservedTakerFirstLockEvidenceV1::from_canonical_lez(*canonical),
        ),
        TakerFirstLockObservationV1::LezRemoved(removed) => {
            Some(crate::ObservedTakerFirstLockEvidenceV1::from_canonical_lez_removal(*removed))
        }
        TakerFirstLockObservationV1::LezReplaced { removed, canonical } => Some(
            crate::ObservedTakerFirstLockEvidenceV1::from_canonical_lez_replacement(
                *removed, *canonical,
            ),
        ),
        TakerFirstLockObservationV1::CanonicalZcash(canonical) => {
            Some(crate::ObservedTakerFirstLockEvidenceV1::from_canonical_zcash(*canonical))
        }
        TakerFirstLockObservationV1::ZcashRemoved(removed) => {
            Some(crate::ObservedTakerFirstLockEvidenceV1::from_canonical_zcash_removal(*removed))
        }
        TakerFirstLockObservationV1::ZcashReplaced { removed, canonical } => Some(
            crate::ObservedTakerFirstLockEvidenceV1::from_canonical_zcash_replacement(
                *removed, *canonical,
            ),
        ),
        TakerFirstLockObservationV1::Absent | TakerFirstLockObservationV1::Unstable => None,
    }
}

fn classify_unchanged_maker_eligibility(
    status: Phase,
    revision: u64,
) -> MakerFundingEligibilityOutcome {
    if status == Phase::TakerLockConfirmed {
        MakerFundingEligibilityOutcome::Eligible { revision }
    } else {
        MakerFundingEligibilityOutcome::AwaitingConfirmations
    }
}

fn classify_unchanged_lez_maker_eligibility(
    status: Phase,
    revision: u64,
    required_confirmations: u32,
    environment: crate::LezEnvironmentV1,
    current: Option<(u32, crate::LezInclusionStatusV1)>,
) -> MakerFundingEligibilityOutcome {
    let Some((confirmations, inclusion_status)) = current else {
        return MakerFundingEligibilityOutcome::AwaitingStableObservation(
            crate::FirstLockStepV1::LezFund,
        );
    };
    if status != Phase::TakerLockConfirmed || confirmations < required_confirmations {
        return MakerFundingEligibilityOutcome::AwaitingConfirmations;
    }
    if environment == crate::LezEnvironmentV1::PublicTestnetV0_2
        && inclusion_status != crate::LezInclusionStatusV1::Finalized
    {
        return MakerFundingEligibilityOutcome::AwaitingLezFinality(inclusion_status);
    }
    MakerFundingEligibilityOutcome::Eligible { revision }
}

impl<Lez, Zcash, Store> ActiveZecSwap<Lez, Zcash, Store>
where
    Lez: LezFirstLockPort,
    Zcash: ZcashFirstLockPort,
    Store: RecoveryStore,
{
    /// Observes a durable first-lock plan before any byte-identical rebroadcast.
    ///
    /// LEZ initialization must be confirmed before its funding transaction can
    /// be observed or submitted. A node acceptance or confirmed observation
    /// does not advance the coordinator here; atomic durable evidence projection
    /// remains a separate required boundary.
    ///
    /// # Errors
    ///
    /// Rejects a missing/substituted durable intent and preserves structured
    /// store, LEZ, or Zcash adapter errors.
    pub async fn drive_first_lock(&self) -> Result<FirstLockDriveOutcome, ZecSdkError> {
        let swap_id = self.agreement().coordinator().id();
        let Some(intent) = self
            .store
            .load_first_lock_intent(swap_id)
            .await
            .map_err(|error| ZecSdkError::Persistence(Box::new(error)))?
        else {
            return Err(ZecSdkError::MissingFirstLockIntent);
        };
        intent.validate_for_active(self.agreement(), self.local_participant(), self.revision())?;

        match intent.plan() {
            FirstLockPlanV1::Zcash { funding } => self.drive_zcash_step(funding).await,
            FirstLockPlanV1::Lez { initialize, fund } => {
                match self.observe_lez_step(initialize).await? {
                    FirstLockObservation::Absent => {
                        self.submit_lez_step(initialize).await?;
                        Ok(FirstLockDriveOutcome::Submitted(initialize.step()))
                    }
                    FirstLockObservation::Unstable => Ok(
                        FirstLockDriveOutcome::AwaitingStableObservation(initialize.step()),
                    ),
                    FirstLockObservation::Confirmed => match self.observe_lez_step(fund).await? {
                        FirstLockObservation::Absent => {
                            self.submit_lez_step(fund).await?;
                            Ok(FirstLockDriveOutcome::Submitted(fund.step()))
                        }
                        FirstLockObservation::Unstable => Ok(
                            FirstLockDriveOutcome::AwaitingStableObservation(fund.step()),
                        ),
                        FirstLockObservation::Confirmed => {
                            Ok(FirstLockDriveOutcome::ReadyForFundingProjection)
                        }
                    },
                }
            }
        }
    }

    async fn drive_zcash_step(
        &self,
        funding: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockDriveOutcome, ZecSdkError> {
        match self
            .zcash
            .observe_first_lock(self.agreement(), funding)
            .await
            .map_err(|error| ZecSdkError::ZcashFirstLock(Box::new(error)))?
        {
            FirstLockObservation::Absent => {
                self.zcash
                    .submit_first_lock(self.agreement(), funding)
                    .await
                    .map_err(|error| ZecSdkError::ZcashFirstLock(Box::new(error)))?;
                Ok(FirstLockDriveOutcome::Submitted(funding.step()))
            }
            FirstLockObservation::Unstable => Ok(FirstLockDriveOutcome::AwaitingStableObservation(
                funding.step(),
            )),
            FirstLockObservation::Confirmed => Ok(FirstLockDriveOutcome::ReadyForFundingProjection),
        }
    }

    async fn observe_lez_step(
        &self,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, ZecSdkError> {
        self.lez
            .observe_first_lock(self.agreement(), submission)
            .await
            .map_err(|error| ZecSdkError::LezFirstLock(Box::new(error)))
    }

    async fn submit_lez_step(
        &self,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), ZecSdkError> {
        self.lez
            .submit_first_lock(self.agreement(), submission)
            .await
            .map_err(|error| ZecSdkError::LezFirstLock(Box::new(error)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_evidence_requires_confirmed_phase_for_maker_eligibility() {
        assert_eq!(
            classify_unchanged_maker_eligibility(Phase::AwaitingTakerConfirmations, 7),
            MakerFundingEligibilityOutcome::AwaitingConfirmations
        );
        assert_eq!(
            classify_unchanged_maker_eligibility(Phase::TakerLockConfirmed, 7),
            MakerFundingEligibilityOutcome::Eligible { revision: 7 }
        );
    }

    #[test]
    fn reverse_lez_eligibility_separates_depth_from_public_finality() {
        let classify = |environment, current| {
            classify_unchanged_lez_maker_eligibility(
                Phase::TakerLockConfirmed,
                9,
                3,
                environment,
                current,
            )
        };
        assert_eq!(
            classify(
                crate::LezEnvironmentV1::DeterministicLocalV0_2,
                Some((3, crate::LezInclusionStatusV1::Pending))
            ),
            MakerFundingEligibilityOutcome::Eligible { revision: 9 }
        );
        assert_eq!(
            classify(
                crate::LezEnvironmentV1::PublicTestnetV0_2,
                Some((3, crate::LezInclusionStatusV1::Pending))
            ),
            MakerFundingEligibilityOutcome::AwaitingLezFinality(
                crate::LezInclusionStatusV1::Pending
            )
        );
        assert_eq!(
            classify(
                crate::LezEnvironmentV1::PublicTestnetV0_2,
                Some((3, crate::LezInclusionStatusV1::Safe))
            ),
            MakerFundingEligibilityOutcome::AwaitingLezFinality(crate::LezInclusionStatusV1::Safe)
        );
        assert_eq!(
            classify(
                crate::LezEnvironmentV1::PublicTestnetV0_2,
                Some((3, crate::LezInclusionStatusV1::Finalized))
            ),
            MakerFundingEligibilityOutcome::Eligible { revision: 9 }
        );
        assert_eq!(
            classify(
                crate::LezEnvironmentV1::PublicTestnetV0_2,
                Some((2, crate::LezInclusionStatusV1::Finalized))
            ),
            MakerFundingEligibilityOutcome::AwaitingConfirmations
        );
        assert_eq!(
            classify(crate::LezEnvironmentV1::PublicTestnetV0_2, None),
            MakerFundingEligibilityOutcome::AwaitingStableObservation(
                crate::FirstLockStepV1::LezFund
            )
        );
    }
}
