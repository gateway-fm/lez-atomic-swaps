//! ZEC-only Maker application projections: terminal actor import and Zcash
//! funding reconciliation. Compiled only with the `pair-zec` feature.

use super::*;

/// Replays one stopped Maker actor offline and atomically imports its terminal operator view.
///
/// Unit chain ports make the replay incapable of RPC or chain effects. The application store
/// validates that the actor's exact signed agreement is the one previously completed through
/// Maker Chat; the ordinary application aggregate remains untouched.
///
/// # Errors
///
/// Fails when the source is missing, not a fully replayable terminal Maker actor, aliases the
/// application database, lacks the matching claim key, or disagrees with application history.
pub async fn import_terminal_zec_maker_projection(
    application_database: &Path,
    actor_state_database: &Path,
    swap_id: &SwapId,
    claim_key: ProtectedClaimKey,
) -> anyhow::Result<OperatorTerminalProjectionCommit> {
    anyhow::ensure!(
        application_database != actor_state_database,
        "terminal actor state must be separate from the application database"
    );
    let actor_store = SqliteZecRecoveryStore::open_claim_capable_existing(
        actor_state_database,
        Participant::Maker,
        claim_key,
    )?;
    let sdk: ZecPairSdk<(), (), (), (), SqliteZecRecoveryStore> =
        ZecPairSdk::new(Participant::Maker, (), (), (), (), actor_store);
    let active = sdk
        .resume_all_capable(swap_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("terminal Maker actor is not activated"))?;
    let terminal = active.terminal_coordinator().ok_or_else(|| {
        anyhow::anyhow!("Maker actor has not reached an absorbing terminal phase")
    })?;
    let agreement_wire = active.agreement().encode_wire()?;
    let source_revision = active.revision();
    let mut application = SqliteSwapStore::open(application_database)?;
    application
        .project_zec_terminal_for_operator(terminal, source_revision, &agreement_wire)
        .map_err(Into::into)
}

/// Result of one committed Zcash funding reconciliation.
#[derive(Debug)]
pub struct AppliedZcashFundingEvent {
    swap: SwapCoordinator,
    commit: EventCommit,
    outcome: ZcashFundingProjectionOutcome,
    alert_sequence: Option<u64>,
}

/// Durable runtime classification of one Zcash funding event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZcashFundingProjectionOutcome {
    /// The event changed or refreshed non-terminal protocol state.
    Applied,
    /// Chain truth replaced a transaction ID already committed to the protocol.
    ReplacementConflict {
        /// Direction-derived participant whose transaction remains pinned.
        funded_by: Participant,
    },
    /// A removal/replacement affected an absorbing lifecycle outcome.
    TerminalReorgDetected {
        /// Completed or refunded lifecycle result retained for audit.
        terminal_phase: Phase,
        /// Direction-derived participant whose funding evidence changed.
        funded_by: Participant,
    },
}

impl AppliedZcashFundingEvent {
    /// Durable aggregate after the event or replay reload.
    #[must_use]
    pub const fn swap(&self) -> &SwapCoordinator {
        &self.swap
    }

    /// Durable commit metadata.
    #[must_use]
    pub const fn commit(&self) -> EventCommit {
        self.commit
    }

    /// Durable projection classification used to gate subsequent effects.
    #[must_use]
    pub const fn outcome(&self) -> ZcashFundingProjectionOutcome {
        self.outcome
    }

    /// Durable operator-alert cursor for attention-requiring outcomes.
    #[must_use]
    pub const fn alert_sequence(&self) -> Option<u64> {
        self.alert_sequence
    }
}

/// Failure while projecting and atomically committing a Zcash funding event.
#[derive(Debug, thiserror::Error)]
pub enum ZcashFundingApplyError {
    /// Persistence or optimistic concurrency failed.
    #[error("Zcash funding persistence failed")]
    Store(#[from] StoreError),
    /// Valid chain evidence conflicts with the current protocol aggregate.
    #[error("Zcash funding evidence conflicts with swap state")]
    Core(#[from] Error),
    /// The selected swap is not a Zcash swap.
    #[error("Zcash funding event was routed to a non-Zcash swap")]
    WrongPair,
    /// Legacy or corrupt storage omitted immutable ZEC profile/output terms.
    #[error("Zcash swap has no immutable profile/output binding")]
    MissingZcashBinding,
    /// Chain evidence differs from the swap's immutable profile/output terms.
    #[error("Zcash funding event does not match immutable swap binding")]
    Binding(#[from] ZecBindingRecordError),
    /// Coordinator leg policies disagree with the immutable named profile.
    #[error("Zcash swap confirmation policies do not match immutable profile")]
    BindingPolicyMismatch,
    /// Historical event records are corrupt or out of order.
    #[error("Zcash observation history cannot be replayed")]
    HistoricalReplay(#[from] HistoricalReplayError),
}

/// Restores the historical watcher head for the direction-derived ZEC-funded role.
///
/// The returned tracker is not fresh canonical evidence. Reconcile it with a new
/// stable Zebra snapshot before enabling any external effect.
///
/// # Errors
///
/// Returns [`ZcashFundingApplyError`] for missing storage, a non-Zcash swap,
/// corrupt records, or impossible event order.
pub fn load_zcash_observation_tracker(
    store: &SqliteSwapStore,
    id: &SwapId,
) -> Result<ZcashObservationTracker, ZcashFundingApplyError> {
    let swap = store.load(id)?.ok_or(StoreError::MissingSwap)?;
    let funded_by = zcash_funder(&swap)?;
    let binding = store
        .load_zcash_binding(id)?
        .ok_or(ZcashFundingApplyError::MissingZcashBinding)?;
    validate_binding_policies(&swap, &binding)?;
    let records = store.load_zcash_events(id, funded_by)?;
    replay_zcash_observation_history(&records).map_err(ZcashFundingApplyError::from)
}

/// Projects one validated Zcash watcher event and commits it with the aggregate.
///
/// The ZEC funder is derived from immutable swap direction rather than caller input.
/// An exact predecessor-slot replay reloads durable state before any core mutation,
/// which makes retries safe after an unknown successful commit outcome.
///
/// # Errors
///
/// Returns [`ZcashFundingApplyError`] for missing/stale storage, a non-Zcash swap,
/// invalid core transition, record/proof conversion, or transaction failure.
pub fn apply_zcash_funding_event(
    store: &mut SqliteSwapStore,
    predecessor_revision: u64,
    id: &SwapId,
    event: &ZcashObservationEvent,
) -> Result<AppliedZcashFundingEvent, ZcashFundingApplyError> {
    let record = ZcashObservationEventRecordV1::from_event(event);
    let mut swap = store.load(id)?.ok_or(StoreError::MissingSwap)?;
    let funded_by = zcash_funder(&swap)?;
    let binding = store
        .load_zcash_binding(id)?
        .ok_or(ZcashFundingApplyError::MissingZcashBinding)?;
    validate_binding_policies(&swap, &binding)?;
    binding.validate_event(event)?;
    if store
        .committed_zcash_event(predecessor_revision, id, funded_by, &record)?
        .is_some()
    {
        swap = store.load(id)?.ok_or(StoreError::MissingSwap)?;
        let outcome = terminal_reorg_outcome(&swap, funded_by, event)
            .or_else(|| replacement_conflict_outcome(&swap, funded_by, event))
            .unwrap_or(ZcashFundingProjectionOutcome::Applied);
        let alert = operator_alert(outcome, event)?;
        let transition = store.commit_zcash_transition(
            predecessor_revision,
            &swap,
            funded_by,
            &record,
            alert.as_ref(),
        )?;
        return Ok(AppliedZcashFundingEvent {
            swap,
            commit: transition.event(),
            outcome,
            alert_sequence: transition.alert_sequence(),
        });
    }

    let records = store.load_zcash_events(id, funded_by)?;
    let mut historical_tracker = replay_zcash_observation_history(&records)?;
    historical_tracker
        .apply_committed(event)
        .map_err(HistoricalReplayError::from)?;

    let outcome = if let Some(outcome) = terminal_reorg_outcome(&swap, funded_by, event) {
        outcome
    } else {
        project_zcash_funding_event(&mut swap, funded_by, event)?
    };
    let alert = operator_alert(outcome, event)?;
    let transition = store.commit_zcash_transition(
        predecessor_revision,
        &swap,
        funded_by,
        &record,
        alert.as_ref(),
    )?;
    Ok(AppliedZcashFundingEvent {
        swap,
        commit: transition.event(),
        outcome,
        alert_sequence: transition.alert_sequence(),
    })
}

fn operator_alert(
    outcome: ZcashFundingProjectionOutcome,
    event: &ZcashObservationEvent,
) -> Result<Option<OperatorAlertRecordV1>, ZcashFundingApplyError> {
    match outcome {
        ZcashFundingProjectionOutcome::Applied => Ok(None),
        ZcashFundingProjectionOutcome::ReplacementConflict { funded_by } => {
            OperatorAlertRecordV1::replacement_conflict(funded_by, event)
                .map(Some)
                .map_err(ZcashFundingApplyError::from)
        }
        ZcashFundingProjectionOutcome::TerminalReorgDetected {
            terminal_phase,
            funded_by,
        } => OperatorAlertRecordV1::terminal_reorg(terminal_phase, funded_by, event)
            .map(Some)
            .map_err(ZcashFundingApplyError::from),
    }
}

fn validate_binding_policies(
    swap: &SwapCoordinator,
    binding: &ZecSwapBinding,
) -> Result<(), ZcashFundingApplyError> {
    let profile = ZecRefundProfile::for_id(binding.profile_id());
    for participant in [Participant::Maker, Participant::Taker] {
        let expected = if swap.funded_chain(participant) == Chain::Zcash {
            profile.zcash_confirmations()
        } else {
            profile.lez_confirmations()
        };
        if swap.required_confirmations(participant) != expected {
            return Err(ZcashFundingApplyError::BindingPolicyMismatch);
        }
    }
    Ok(())
}

fn replacement_conflict_outcome(
    swap: &SwapCoordinator,
    funded_by: Participant,
    event: &ZcashObservationEvent,
) -> Option<ZcashFundingProjectionOutcome> {
    let ZcashObservationEvent::Replaced { canonical, .. } = event else {
        return None;
    };
    let role_is_reorged = matches!(
        (funded_by, swap.phase()),
        (Participant::Taker, Phase::TakerLockReorged)
            | (Participant::Maker, Phase::MakerLockReorged)
    );
    let replacement_id = canonical.transaction_id().to_string();
    (role_is_reorged
        && swap
            .funding_transaction_id(funded_by)
            .is_some_and(|pinned| pinned != replacement_id))
    .then_some(ZcashFundingProjectionOutcome::ReplacementConflict { funded_by })
}

fn terminal_reorg_outcome(
    swap: &SwapCoordinator,
    funded_by: Participant,
    event: &ZcashObservationEvent,
) -> Option<ZcashFundingProjectionOutcome> {
    (matches!(swap.phase(), Phase::Completed | Phase::Refunded)
        && matches!(
            event,
            ZcashObservationEvent::Removed(_) | ZcashObservationEvent::Replaced { .. }
        ))
    .then_some(ZcashFundingProjectionOutcome::TerminalReorgDetected {
        terminal_phase: swap.phase(),
        funded_by,
    })
}

fn zcash_funder(swap: &SwapCoordinator) -> Result<Participant, ZcashFundingApplyError> {
    if swap.pair() != Pair::Zcash {
        return Err(ZcashFundingApplyError::WrongPair);
    }
    Ok(if swap.funded_chain(Participant::Taker) == Chain::Zcash {
        Participant::Taker
    } else {
        Participant::Maker
    })
}

fn project_zcash_funding_event(
    swap: &mut SwapCoordinator,
    funded_by: Participant,
    event: &ZcashObservationEvent,
) -> Result<ZcashFundingProjectionOutcome, Error> {
    match event {
        ZcashObservationEvent::Canonical(canonical) => {
            swap.observe_funding(funded_by, canonical.chain_proof()?)?;
            Ok(ZcashFundingProjectionOutcome::Applied)
        }
        ZcashObservationEvent::Removed(removed) => {
            swap.observe_funding_removed(
                funded_by,
                &removed.previous().transaction_id().to_string(),
            )?;
            Ok(ZcashFundingProjectionOutcome::Applied)
        }
        ZcashObservationEvent::Replaced { removed, canonical } => {
            swap.observe_funding_removed(
                funded_by,
                &removed.previous().transaction_id().to_string(),
            )?;
            match swap.observe_funding(funded_by, canonical.chain_proof()?) {
                Ok(()) => Ok(ZcashFundingProjectionOutcome::Applied),
                Err(Error::ConflictingTakerLock) if funded_by == Participant::Taker => {
                    Ok(ZcashFundingProjectionOutcome::ReplacementConflict { funded_by })
                }
                Err(Error::ConflictingMakerLock) if funded_by == Participant::Maker => {
                    Ok(ZcashFundingProjectionOutcome::ReplacementConflict { funded_by })
                }
                Err(error) => Err(error),
            }
        }
    }
}
