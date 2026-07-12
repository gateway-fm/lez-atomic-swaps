//! Pre-lock and post-lock LEZ/ZEC SDK facades.

use lez_swap_core::{Participant, Phase, SwapCoordinator, SwapId, UnixSeconds};

use crate::{
    AcceptedZecAgreementV1, CreateAgreementOutcome, CreateFirstLockOutcome,
    FirstLockConfirmedEvidenceV1, FirstLockDriveOutcome, FirstLockIntentV1, FirstLockObservation,
    FirstLockPlanV1, FirstLockProjectionCommit, FirstLockTransitionV1, LezFirstLockPort,
    LezTakerFirstLockObservationPort, NegotiationChannel, ObserveTakerFirstLockOutcome,
    ObservedTakerFirstLockTransitionV1, OfferDiscovery, PreparedFirstLockSubmissionV1,
    RecoveryStore, TakerFirstLockObservationV1, ZcashFirstLockPort,
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
        let Some(transition) = self
            .store
            .load_observed_taker_first_lock_transition(self.coordinator.id(), self.revision)
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
        if self.status() != Phase::Offered {
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
                .observe_taker_first_lock(self.agreement())
                .await
                .map_err(|error| ZecSdkError::LezTakerFirstLockObservation(Box::new(error)))?,
            crate::FirstLockStepV1::LezInitialize => unreachable!("not a final lock step"),
        };
        let evidence = match observation {
            TakerFirstLockObservationV1::Confirmed(evidence) => evidence,
            TakerFirstLockObservationV1::CanonicalZcash(canonical) => {
                crate::ObservedTakerFirstLockEvidenceV1::from_canonical_zcash(*canonical)
            }
            TakerFirstLockObservationV1::Absent | TakerFirstLockObservationV1::Unstable => {
                return Ok(ObserveTakerFirstLockOutcome::AwaitingStableObservation(
                    step,
                ));
            }
        };
        let transition = ObservedTakerFirstLockTransitionV1::from_active(
            self.agreement(),
            self.local_participant(),
            self.revision,
            evidence,
        )?;
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
        Ok(ObserveTakerFirstLockOutcome::Projected(commit))
    }
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
