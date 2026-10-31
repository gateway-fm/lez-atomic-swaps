//! The Taker Node supervises its Zcash actor the way the Maker Node
//! supervises its actors: from the take's activation through the first lock
//! and the observation of the Maker's legs to `claim_available`, with a
//! durable schedule (queued / leased / backoff / terminal / failed), bounded
//! attempts, the failure classes both Nodes share and one log line per
//! effect. The owner's boundary is unchanged: a claim or a refund runs only
//! after the owner admitted it (`taker_swap_claim_v1` / `taker_swap_refund_v1`);
//! the supervisor never drives an actor whose next action is a claim.
//! An open admission is read before the phase table, so a phase where the
//! table drives must also drive with the admission open when the admitted
//! action is done: selling ZEC, a finished revealing LEZ claim leaves
//! (`ClaimEvidenceAvailable`, `Wait`), where a drive observes the Maker's Zcash
//! follow-up claim (FINDINGS 22 of the M2 devnet run).
//!
//! The actor runs in-process under the swap's lock, as the Node's status,
//! claim and recover calls always did; the Maker's child-process mechanics
//! are not needed here. A read that meets the lock held by a drive answers
//! from the schedule's last snapshot ([`view_from_snapshot`]).

use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use lez_actor_supervision::{AttemptFailure, AttemptResolution, log_effect_value, terminal_phase};
use lez_swap_core::{Phase, SwapId};
use lez_swap_store::{
    ActorHeldLock, TakerActorLeaseV1, TakerActorProgressSnapshotV1, TakerActorScheduleRecordV1,
    TakerInitiationFactsV1,
};
use lez_zec_swap_sdk::ZecLifecycleAction;
use zec_reference_actor::{
    ActorCommand, ActorCommandError, record_revealing_claim_holder,
    revealing_claim_hold_needs_attention,
};

use super::{
    TakerServiceState,
    lifecycle::{
        LOCK_WAIT, LifecycleStatus, RouteActor, ZecActorStatus, lookup_admitted_action_for_swap,
        mark_claim_submitted, reload_action_actor_custody, revealing_claim_was_submitted,
        terminal_action_of, zec_actor_progress, zec_replay_actor_effect_required,
    },
};
use crate::{
    ConfiguredTakerInitiationContext, PreparedTakerInitiationV1, TakerSwapStateV1, TakerSwapViewV1,
    TakerTerminalActionV1,
};

/// The Taker settles every swap as one LEZ account, and the bridge plans a
/// revealing LEZ claim at that account's committed nonce. Selling ZEC, two
/// swaps whose claims are prepared before the first is final carry the same
/// nonce and one of them is superseded (FINDINGS 24: re-planned since, but a
/// mitigation still keeps them apart). So one revealing LEZ claim is in flight
/// per Node: another swap's claim waits (requeued, no failure class) while any
/// other bound swap's actor holds its in-flight marker — written when its
/// claim is submitted, removed when the claim is observed final. Durable
/// (restarts keep it) and never released on a timer.
const REVEALING_CLAIM_IN_FLIGHT: &str = "revealing-claim-in-flight";

/// The class of a revealing claim held behind another swap's while its own
/// LEZ refund time is near (a third of its LEZ refund delay,
/// [`zec_reference_actor::revealing_claim_hold_attention_ms`]):
/// the row reads as needing attention and names the holder; the claim is
/// still attempted at the requeue cadence and goes ahead once the gate opens.
pub(super) const REVEALING_CLAIM_HELD_NEAR_REFUND: &str = "lez_revealing_claim_held_near_refund";

/// The peer of `own` whose revealing LEZ claim is in flight
/// (`<actor root>/taker/revealing-claim-in-flight`), if any.
pub(super) fn peer_revealing_claim_holder<'a, Id>(
    own: &Path,
    peers: impl IntoIterator<Item = (&'a Path, Id)>,
) -> Option<Id> {
    peers
        .into_iter()
        .find(|(root, _)| {
            *root != own && root.join("taker").join(REVEALING_CLAIM_IN_FLIGHT).is_file()
        })
        .map(|(_, id)| id)
}

/// Whether any of `actor_roots` other than `own` holds a revealing LEZ claim
/// in flight.
#[cfg(test)]
pub(super) fn peer_revealing_claim_in_flight<'a>(
    own: &Path,
    actor_roots: impl IntoIterator<Item = &'a Path>,
) -> bool {
    peer_revealing_claim_holder(own, actor_roots.into_iter().map(|root| (root, ()))).is_some()
}

/// The gate for `prepared`'s revealing claim over every bound Zcash swap:
/// `Ok(None)` open, `Ok(Some(holder))` held behind that swap's claim, `Err`
/// held with the holder unknown (the context could not be read).
pub(super) fn lez_claim_holder(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    prepared: &PreparedTakerInitiationV1,
) -> Result<Option<String>, ()> {
    let Ok(context) = initiation.lock() else {
        return Err(());
    };
    let bound = context.zec_bound_swaps();
    Ok(peer_revealing_claim_holder(
        prepared.execution().actor_root(),
        bound.iter().map(|swap| {
            (
                swap.execution().actor_root(),
                swap.swap_id().as_str().to_owned(),
            )
        }),
    ))
}

/// The gate for `prepared`'s revealing claim, open or held.
pub(super) fn lez_claim_gate_open(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    prepared: &PreparedTakerInitiationV1,
) -> bool {
    matches!(lez_claim_holder(initiation, prepared), Ok(None))
}

/// A Zcash claim at both legs locked is the Taker's revealing LEZ claim
/// (`TakerSellsForeign`); selling LEZ the Taker's claim is on Zcash.
const fn submits_lez_claim(phase: Phase, command: ActorCommand) -> bool {
    matches!(command, ActorCommand::Claim) && matches!(phase, Phase::BothLegsLocked)
}

/// How often the supervisor looks for due swaps.
const TICK: Duration = Duration::from_secs(5);
/// A live actor is observed or driven again after this long.
const REQUEUE_SECONDS: u64 = 5;
/// A failed attempt is retried after this long.
const FAILURE_BACKOFF_SECONDS: u64 = 30;
/// One status or effect may take this long; a first lock prepares through the
/// sidecar and submits to the sequencer.
const ATTEMPT_TIMEOUT: Duration = Duration::from_mins(2);
/// Due swaps attempted per tick.
const DUE_PER_TICK: usize = 16;
const ROLE: &str = "taker actor";
/// A snapshot older than this is not served as a row (the actor is busy for
/// longer than one attempt should take; the read says unavailable instead).
const SNAPSHOT_FRESH_SECONDS: u64 = 60;

/// What was last logged per swap, so a swap that backs off every 30 s or
/// whose drive returns the same observation every 5 s prints one line and
/// then only a changed one (the rule of 11f9bbdf). The row's
/// `last_failure_class` is where the operator reads a standing failure.
#[derive(Default)]
struct LogMemory {
    effects: std::collections::HashMap<String, String>,
    failures: std::collections::HashMap<String, &'static str>,
}

impl LogMemory {
    fn effect_changed(&mut self, swap_id: &str, line: &str) -> bool {
        if self.effects.get(swap_id).map(String::as_str) == Some(line) {
            return false;
        }
        self.effects.insert(swap_id.to_owned(), line.to_owned());
        true
    }

    fn failure_changed(&mut self, swap_id: &str, class: &'static str) -> bool {
        if self.failures.get(swap_id).copied() == Some(class) {
            return false;
        }
        self.failures.insert(swap_id.to_owned(), class);
        true
    }

    fn clear_failure(&mut self, swap_id: &str) {
        self.failures.remove(swap_id);
    }
}

type SharedLogMemory = Arc<Mutex<LogMemory>>;

fn log_memory(memory: &SharedLogMemory) -> std::sync::MutexGuard<'_, LogMemory> {
    memory
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Starts the supervisor when a runtime is available.
pub(super) fn spawn_zec_supervisor(state: Arc<TakerServiceState>) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        eprintln!("taker actor supervisor: no async runtime; Zcash actors are not driven");
        return;
    };
    handle.spawn(async move {
        requeue_leased_on_start(&state).await;
        let memory: SharedLogMemory = Arc::default();
        loop {
            tokio::time::sleep(TICK).await;
            Box::pin(supervise_due_swaps(&state, &memory)).await;
        }
    });
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

async fn requeue_leased_on_start(state: &TakerServiceState) {
    let Some(initiation) = state.initiation.clone() else {
        return;
    };
    let requeued = tokio::task::spawn_blocking(move || {
        let mut context = initiation.lock().ok()?;
        context
            .schedule_mut()
            .requeue_leased_on_start(now_unix())
            .ok()
    })
    .await
    .ok()
    .flatten()
    .unwrap_or(0);
    if requeued > 0 {
        eprintln!(
            "taker actor supervisor: {requeued} swap(s) left leased by the previous process are queued again (coordinator_restarted)"
        );
    }
}

/// Registers every bound Zcash swap, then attempts the due ones in turn.
async fn supervise_due_swaps(state: &TakerServiceState, memory: &SharedLogMemory) {
    let Some(initiation) = state.initiation.clone() else {
        return;
    };
    let due = {
        let initiation = Arc::clone(&initiation);
        tokio::task::spawn_blocking(move || {
            let mut context = initiation.lock().ok()?;
            let now = now_unix();
            let bound = context.zec_bound_swaps();
            for prepared in &bound {
                let _ = context
                    .schedule_mut()
                    .register(prepared.swap_id().as_str(), now);
            }
            let due_ids = context.schedule_mut().list_due(now, DUE_PER_TICK).ok()?;
            Some(
                due_ids
                    .into_iter()
                    .filter_map(|id| {
                        bound
                            .iter()
                            .find(|prepared| prepared.swap_id().as_str() == id)
                            .cloned()
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
    };
    for prepared in due {
        Box::pin(supervise_one(&initiation, &prepared, memory)).await;
    }
}

/// One claimed attempt of one swap, resolved into the schedule.
async fn supervise_one(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    prepared: &PreparedTakerInitiationV1,
    memory: &SharedLogMemory,
) {
    let swap_id = prepared.swap_id().clone();
    let lease = {
        let initiation = Arc::clone(initiation);
        let id = swap_id.as_str().to_owned();
        tokio::task::spawn_blocking(move || {
            let mut context = initiation.lock().ok()?;
            context.schedule_mut().claim(&id, now_unix()).ok().flatten()
        })
        .await
        .ok()
        .flatten()
    };
    let Some(lease) = lease else {
        return;
    };
    let (attempt, snapshot) = attempt(initiation, prepared, memory).await;
    let now = now_unix();
    let resolution = match attempt {
        ClaimedAttempt::Requeue => {
            log_memory(memory).clear_failure(swap_id.as_str());
            AttemptResolution::Requeue {
                not_before: now.saturating_add(REQUEUE_SECONDS),
            }
        }
        ClaimedAttempt::HeldNearRefund => AttemptResolution::Backoff {
            not_before: now.saturating_add(REQUEUE_SECONDS),
            failure_class: REVEALING_CLAIM_HELD_NEAR_REFUND.to_owned(),
        },
        ClaimedAttempt::Terminal => AttemptResolution::Terminal,
        ClaimedAttempt::ManualActionCompleted => AttemptResolution::ManualActionCompleted,
        ClaimedAttempt::Failure(AttemptFailure::Retry(class)) => {
            if log_memory(memory).failure_changed(swap_id.as_str(), class) {
                eprintln!(
                    "{ROLE} {}: attempt backs off ({class}); retried every {FAILURE_BACKOFF_SECONDS} s, logged again only when the class changes",
                    swap_id.as_str()
                );
            }
            AttemptResolution::Backoff {
                not_before: now.saturating_add(FAILURE_BACKOFF_SECONDS),
                failure_class: class.to_owned(),
            }
        }
        ClaimedAttempt::Failure(AttemptFailure::Fail(class)) => {
            if log_memory(memory).failure_changed(swap_id.as_str(), class) {
                eprintln!(
                    "{ROLE} {}: attempt failed ({class}); the row needs attention until the owner queues a claim or refund",
                    swap_id.as_str()
                );
            }
            AttemptResolution::Failed {
                failure_class: class.to_owned(),
            }
        }
    };
    resolve(initiation, &lease, resolution, snapshot, now).await;
}

async fn resolve(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    lease: &TakerActorLeaseV1,
    resolution: AttemptResolution,
    snapshot: Option<TakerActorProgressSnapshotV1>,
    now: u64,
) {
    let initiation = Arc::clone(initiation);
    let leased = lease.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let mut context = initiation.lock().map_err(|_| "context")?;
        context
            .schedule_mut()
            .resolve(&leased, &resolution, snapshot.as_ref(), now)
            .map_err(|_| "schedule")
    })
    .await;
    if !matches!(outcome, Ok(Ok(()))) {
        eprintln!(
            "{ROLE} {}: the attempt's resolution was not recorded (lease generation {})",
            lease.swap_id(),
            lease.generation()
        );
    }
}

/// How one attempt ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClaimedAttempt {
    Requeue,
    /// The revealing claim is held behind another swap's and this swap's own
    /// LEZ refund time is near: requeued, with the attention class.
    HeldNearRefund,
    Terminal,
    ManualActionCompleted,
    Failure(AttemptFailure),
}

/// What the supervisor does with an actor in a given status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Decision {
    /// `activate`: the take left the actor not activated.
    Activate,
    /// `drive`: the actor's own first lock, or the observation of the
    /// Maker's lock and revealing claim.
    Drive,
    /// `claim`: the owner admitted it.
    Claim,
    /// `recover`: the owner admitted a refund.
    Recover,
    /// Nothing to do but observe: the owner's claim or refund is offered and
    /// not admitted, or the actor waits on the counterparty.
    Observe,
    /// The actor is in an absorbing phase.
    Terminal,
    /// The owner's admitted action has reached its outcome.
    ManualActionCompleted,
}

/// The decision table. `admitted` is the owner's open action and the
/// generation it was admitted at; no revealing claim was ever submitted.
#[cfg(test)]
pub(super) fn decide(
    status: ZecActorStatus,
    admitted: Option<(TakerTerminalActionV1, u64)>,
) -> Decision {
    decide_with(status, admitted, false)
}

/// The decision table. `claim_submitted`: this swap's actor has submitted a
/// revealing LEZ claim (its marker exists); a refund admitted in the claim
/// window is then never run (FINDINGS 24: the secret may be public).
pub(super) fn decide_with(
    status: ZecActorStatus,
    admitted: Option<(TakerTerminalActionV1, u64)>,
    claim_submitted: bool,
) -> Decision {
    let ZecActorStatus::Active {
        phase,
        revision,
        next_action,
        ..
    } = status
    else {
        return Decision::Activate;
    };
    if matches!(phase, Phase::Completed | Phase::Refunded) {
        return if admitted.is_some() {
            Decision::ManualActionCompleted
        } else {
            Decision::Terminal
        };
    }
    if let Some((action, expected_generation)) = admitted {
        let (state, available_action, privacy_guidance) = zec_actor_progress(phase, next_action);
        let status = LifecycleStatus::Active {
            phase,
            revision,
            state,
            available_action,
            privacy_guidance,
        };
        match zec_replay_actor_effect_required(status, expected_generation, action, claim_submitted)
        {
            Ok(true) => {
                return match action {
                    TakerTerminalActionV1::Claim => Decision::Claim,
                    TakerTerminalActionV1::Refund => Decision::Recover,
                };
            }
            // Selling ZEC, the admitted claim is the Taker's revealing LEZ
            // claim; once it is final (ClaimEvidenceAvailable, next action
            // Wait) the admission is done, but the swap completes only when
            // a drive observes the Maker's Zcash follow-up claim.
            Ok(false) | Err(_)
                if action == TakerTerminalActionV1::Claim
                    && phase == Phase::ClaimEvidenceAvailable
                    && next_action == ZecLifecycleAction::Wait =>
            {
                return Decision::Drive;
            }
            // Admitted and done, or admitted against a generation the actor
            // has left: the owner's paths say so; here there is only observing.
            Ok(false) | Err(_) => return Decision::Observe,
        }
    }
    match next_action {
        // The Taker's own first lock (LEZ, or Zcash in the reverse direction).
        ZecLifecycleAction::CreateAndFundLez | ZecLifecycleAction::FundZcash => Decision::Drive,
        // Observation phases: the Taker's lock confirming, the Maker's lock,
        // the Maker's revealing claim. A drive there only observes (the
        // non-claimant's `drive_claim` at BothLegsLocked is an observation).
        // In the reverse direction the Taker is the LEZ claimant: once its
        // revealing claim is final (ClaimEvidenceAvailable, next action Wait)
        // a drive only observes the Maker's Zcash follow-up claim.
        ZecLifecycleAction::Wait => match phase {
            Phase::Offered
            | Phase::TakerLockConfirmed
            | Phase::BothLegsLocked
            | Phase::ClaimEvidenceAvailable => Decision::Drive,
            _ => Decision::Observe,
        },
        // The owner's boundary: never driven without an admission (a drive at
        // ClaimEvidenceAvailable would be the Zcash claim).
        ZecLifecycleAction::ClaimZcash
        | ZecLifecycleAction::ClaimLez
        | ZecLifecycleAction::RefundZcash => Decision::Observe,
        ZecLifecycleAction::Complete => Decision::Terminal,
    }
}

/// The failure class of an actor error, and whether a retry may clear it.
pub(super) fn failure_of(error: ActorCommandError) -> AttemptFailure {
    match error {
        ActorCommandError::LezDepositorBalanceInsufficient => {
            AttemptFailure::Retry("lez_depositor_balance_insufficient")
        }
        // The LEZ first lock was re-planned the bounded number of times and
        // its nonce was taken again: nothing is on the chain, the swap stops
        // by name (D1).
        ActorCommandError::RevealingClaimReplanExhausted => {
            AttemptFailure::Fail("lez_revealing_claim_replan_exhausted")
        }
        ActorCommandError::FirstLockReplanExhausted => {
            AttemptFailure::Fail("lez_first_lock_replan_exhausted")
        }
        ActorCommandError::NotActivated => AttemptFailure::Retry("actor_not_activated"),
        // Material is read from files the corridor and the sidecar write (the
        // capability file, the keys): a file not there yet, or rotated, is
        // retried; the row names it meanwhile.
        ActorCommandError::ActivationMaterialUnavailable
        | ActorCommandError::DriveMaterialUnavailable
        | ActorCommandError::ClaimMaterialUnavailable
        | ActorCommandError::RecoveryMaterialUnavailable => {
            AttemptFailure::Retry("actor_material_unavailable")
        }
        ActorCommandError::DriveConfigurationUnavailable
        | ActorCommandError::ClaimConfigurationUnavailable
        | ActorCommandError::RecoveryConfigurationUnavailable => {
            AttemptFailure::Fail("actor_deployment_invalid")
        }
        ActorCommandError::DriveStoreUnavailable
        | ActorCommandError::ClaimStoreUnavailable
        | ActorCommandError::RecoveryStoreUnavailable => {
            AttemptFailure::Retry("actor_store_unavailable")
        }
        _ => AttemptFailure::Retry("actor_drive_unavailable"),
    }
}

enum LoadFailure {
    Bundle,
    Lock,
    Custody,
}

/// Loads the bound actor under its lock, telling an altered bundle (no retry
/// clears it) from a busy lock or a custody change (the next attempt may).
async fn load_supervised_actor(
    prepared: &PreparedTakerInitiationV1,
    receipt_binding: crate::taker_service_config::PreparedReceiptBindingV1,
) -> Result<(RouteActor, ActorHeldLock), LoadFailure> {
    let pair = prepared.facts().route().pair();
    let receipt = prepared.execution().receipt_output().to_path_buf();
    let actor_root = prepared.execution().actor_root().to_path_buf();
    let swap_id = prepared.swap_id().clone();
    let receipt_sha256 = receipt_binding.sha256();
    let receipt_identity = receipt_binding.identity();
    tokio::task::spawn_blocking(move || {
        let load = || {
            RouteActor::load_for_monitor(
                pair,
                &receipt,
                &actor_root,
                &swap_id,
                receipt_sha256,
                receipt_identity.device(),
                receipt_identity.inode(),
            )
            .map_err(|_| LoadFailure::Bundle)
        };
        let before = load()?;
        let held_lock =
            ActorHeldLock::acquire_for_within(before.swap_id(), before.state_db(), LOCK_WAIT)
                .map_err(|_| LoadFailure::Lock)?;
        let config = load()?;
        if before.custody_identity() != config.custody_identity() {
            return Err(LoadFailure::Custody);
        }
        held_lock
            .validate_for_state(config.swap_id(), config.state_db())
            .map_err(|_| LoadFailure::Custody)?;
        Ok((config, held_lock))
    })
    .await
    .map_err(|_| LoadFailure::Lock)?
}

/// The actor under its lock and its status, or the failure class of not
/// getting there.
async fn observe(
    prepared: &PreparedTakerInitiationV1,
    receipt_binding: crate::taker_service_config::PreparedReceiptBindingV1,
) -> Result<(RouteActor, ActorHeldLock, ZecActorStatus), AttemptFailure> {
    let (config, held_lock) = match load_supervised_actor(prepared, receipt_binding).await {
        Ok(loaded) => loaded,
        Err(LoadFailure::Bundle) => return Err(AttemptFailure::Fail("actor_deployment_invalid")),
        Err(LoadFailure::Lock) => return Err(AttemptFailure::Retry("actor_lock_unavailable")),
        Err(LoadFailure::Custody) => return Err(AttemptFailure::Retry("actor_custody_changed")),
    };
    let status = match tokio::time::timeout(ATTEMPT_TIMEOUT, config.zec_status()).await {
        Err(_) => return Err(AttemptFailure::Retry("actor_timeout")),
        Ok(Err(error)) => return Err(failure_of(error)),
        Ok(Ok(status)) => status,
    };
    Ok((config, held_lock, status))
}

/// One attempt: status, decision, at most one effect.
async fn attempt(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    prepared: &PreparedTakerInitiationV1,
    memory: &SharedLogMemory,
) -> (ClaimedAttempt, Option<TakerActorProgressSnapshotV1>) {
    let swap_id = prepared.swap_id().clone();
    let Some(receipt_binding) = prepared.execution().receipt_binding() else {
        return (ClaimedAttempt::Requeue, None);
    };
    let admitted = lookup_admitted_action_for_swap(initiation, &swap_id)
        .await
        .ok()
        .flatten()
        .map(|admission| {
            (
                terminal_action_of(admission.action()),
                admission.requested_after_generation(),
            )
        });
    let (config, held_lock, status) = match observe(prepared, receipt_binding).await {
        Ok(observed) => observed,
        Err(failure) => return (ClaimedAttempt::Failure(failure), None),
    };
    let snapshot = snapshot_of_status(status);
    let phase_before = match status {
        ZecActorStatus::Active { phase, .. } => Some(phase),
        ZecActorStatus::NotActivated => None,
    };
    let claim_submitted = revealing_claim_was_submitted(config.state_db());
    let command = match decide_with(status, admitted, claim_submitted) {
        Decision::Terminal => return (ClaimedAttempt::Terminal, snapshot),
        Decision::ManualActionCompleted => {
            return (ClaimedAttempt::ManualActionCompleted, snapshot);
        }
        Decision::Observe => return (ClaimedAttempt::Requeue, snapshot),
        Decision::Activate => ActorCommand::Activate,
        Decision::Drive => ActorCommand::Drive,
        Decision::Claim => ActorCommand::Claim,
        Decision::Recover => ActorCommand::Recover,
    };
    let lez_claim = phase_before.is_some_and(|phase| submits_lez_claim(phase, command));
    let holder = if lez_claim {
        match lez_claim_holder(initiation, prepared) {
            Ok(holder) => holder,
            Err(()) => return (ClaimedAttempt::Requeue, snapshot),
        }
    } else {
        None
    };
    if let Some(holder) = holder {
        let refund_and_attention_ms = match status {
            ZecActorStatus::Active {
                lez_refund_at_ms,
                revealing_claim_hold_attention_ms,
                ..
            } => lez_refund_at_ms.zip(revealing_claim_hold_attention_ms),
            ZecActorStatus::NotActivated => None,
        };
        return held_claim(
            &swap_id,
            &config,
            refund_and_attention_ms,
            &holder,
            memory,
            snapshot,
        );
    }
    // Not held (or not a revealing claim): no holder to name any more.
    record_revealing_claim_holder(config.state_db(), None);
    let (effect, snapshot) = match run_effect(
        prepared,
        receipt_binding,
        &config,
        &held_lock,
        command,
        snapshot,
        memory,
    )
    .await
    {
        Ok(ran) => ran,
        Err((failure, snapshot)) => return (ClaimedAttempt::Failure(failure), snapshot),
    };
    let phase = effect
        .get("phase")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("-");
    drop(held_lock);
    if terminal_phase(phase) {
        if admitted.is_some() {
            (ClaimedAttempt::ManualActionCompleted, snapshot)
        } else {
            (ClaimedAttempt::Terminal, snapshot)
        }
    } else {
        (ClaimedAttempt::Requeue, snapshot)
    }
}

/// A revealing claim held behind `holder`'s: requeued; within the attention
/// margin of this swap's own LEZ refund time the holder is named beside the
/// actor's state, the row needs attention, and the Node says so once.
fn held_claim(
    swap_id: &SwapId,
    config: &RouteActor,
    refund_and_attention_ms: Option<(u64, u64)>,
    holder: &str,
    memory: &SharedLogMemory,
    snapshot: Option<TakerActorProgressSnapshotV1>,
) -> (ClaimedAttempt, Option<TakerActorProgressSnapshotV1>) {
    let now_ms = now_unix().saturating_mul(1000);
    let Some(refund_at_ms) = refund_and_attention_ms
        .filter(|(at, attention)| revealing_claim_hold_needs_attention(*at, *attention, now_ms))
        .map(|(at, _)| at)
    else {
        record_revealing_claim_holder(config.state_db(), None);
        return (ClaimedAttempt::Requeue, snapshot);
    };
    record_revealing_claim_holder(config.state_db(), Some(holder));
    if log_memory(memory).failure_changed(swap_id.as_str(), REVEALING_CLAIM_HELD_NEAR_REFUND) {
        eprintln!(
            "{ROLE} {}: revealing LEZ claim held behind swap {holder}'s, {} s before this swap's LEZ refund time; needs attention ({REVEALING_CLAIM_HELD_NEAR_REFUND})",
            swap_id.as_str(),
            refund_at_ms.saturating_sub(now_ms) / 1000
        );
    }
    (ClaimedAttempt::HeldNearRefund, snapshot)
}

/// Reloads the config, runs one effect under the held lock, validates custody
/// afterwards and logs the effect once per distinct outcome. Errors carry the
/// failure class and the snapshot the status already produced.
async fn run_effect(
    prepared: &PreparedTakerInitiationV1,
    receipt_binding: crate::taker_service_config::PreparedReceiptBindingV1,
    config: &RouteActor,
    held_lock: &ActorHeldLock,
    command: ActorCommand,
    snapshot: Option<TakerActorProgressSnapshotV1>,
    memory: &SharedLogMemory,
) -> Result<
    (serde_json::Value, Option<TakerActorProgressSnapshotV1>),
    (AttemptFailure, Option<TakerActorProgressSnapshotV1>),
> {
    let swap_id = prepared.swap_id().clone();
    // The config that answered the status has its file bindings from before
    // that read; the actor's material check refuses an effect on a config
    // whose state database moved under it (the status opened it). Every Node
    // path reloads before an effect (reload_action_actor_custody); so here.
    let Ok(effect_config) =
        reload_action_actor_custody(prepared, receipt_binding, config, held_lock).await
    else {
        return Err((AttemptFailure::Retry("actor_custody_changed"), snapshot));
    };
    let effect =
        match tokio::time::timeout(ATTEMPT_TIMEOUT, effect_config.zec_effect(command)).await {
            Err(_) => return Err((AttemptFailure::Retry("actor_timeout"), snapshot)),
            Ok(Err(error)) => {
                eprintln!(
                    "{ROLE} {}: {} refused: {error}",
                    swap_id.as_str(),
                    command_name(command)
                );
                return Err((failure_of(error), snapshot));
            }
            Ok(Ok(effect)) => effect,
        };
    if command == ActorCommand::Claim {
        mark_claim_submitted(config.state_db());
    }
    if held_lock
        .validate_for_state(config.swap_id(), config.state_db())
        .is_err()
    {
        return Err((AttemptFailure::Retry("actor_custody_changed"), snapshot));
    }
    // One line per distinct effect: a drive that returns the same observation
    // every 5 s is logged once, a change (outcome, phase, revision, next) again.
    let field = |name: &str| {
        effect.get(name).map_or_else(
            || "-".to_owned(),
            |value| {
                value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned)
            },
        )
    };
    // A re-plan inside an otherwise identical `submitted` is a distinct effect.
    let replans = effect
        .get("replans")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    let line = format!(
        "{} {} {} {} {} {} replans={replans}",
        command_name(command),
        field("outcome"),
        field("operation"),
        field("phase"),
        field("revision"),
        field("next_action")
    );
    if log_memory(memory).effect_changed(swap_id.as_str(), &line) {
        log_effect_value(ROLE, swap_id.as_str(), command_name(command), &effect);
    }
    let snapshot = snapshot_of_effect(&effect).or(snapshot);
    Ok((effect, snapshot))
}

const fn command_name(command: ActorCommand) -> &'static str {
    match command {
        ActorCommand::Activate => "activate",
        ActorCommand::Drive => "drive",
        ActorCommand::Claim => "claim",
        ActorCommand::Recover => "recover",
        ActorCommand::Status => "status",
    }
}

/// The actor's spelling of a phase (its JSON output).
pub(super) const fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Offered => "offered",
        Phase::AwaitingTakerConfirmations => "awaiting_taker_confirmations",
        Phase::TakerLockConfirmed => "taker_lock_confirmed",
        Phase::AwaitingMakerConfirmations => "awaiting_maker_confirmations",
        Phase::BothLegsLocked => "both_legs_locked",
        Phase::TakerLockReorged => "taker_lock_reorged",
        Phase::MakerLockReorged => "maker_lock_reorged",
        Phase::ClaimEvidenceAvailable => "claim_evidence_available",
        Phase::Completed => "completed",
        Phase::MakerLegRefunded => "maker_leg_refunded",
        Phase::TakerLegRefunded => "taker_leg_refunded",
        Phase::Refunded => "refunded",
        Phase::MakerRecoveryAvailable => "maker_recovery_available",
    }
}

pub(super) fn phase_from_name(name: &str) -> Option<Phase> {
    [
        Phase::Offered,
        Phase::AwaitingTakerConfirmations,
        Phase::TakerLockConfirmed,
        Phase::AwaitingMakerConfirmations,
        Phase::BothLegsLocked,
        Phase::TakerLockReorged,
        Phase::MakerLockReorged,
        Phase::ClaimEvidenceAvailable,
        Phase::Completed,
        Phase::MakerLegRefunded,
        Phase::TakerLegRefunded,
        Phase::Refunded,
        Phase::MakerRecoveryAvailable,
    ]
    .into_iter()
    .find(|phase| phase_name(*phase) == name)
}

/// The actor's spelling of a next action (its JSON output).
pub(super) const fn action_name(action: ZecLifecycleAction) -> &'static str {
    match action {
        ZecLifecycleAction::Wait => "wait",
        ZecLifecycleAction::CreateAndFundLez => "create_and_fund_lez",
        ZecLifecycleAction::FundZcash => "fund_zcash",
        ZecLifecycleAction::ClaimLez => "claim_lez",
        ZecLifecycleAction::ClaimZcash => "claim_zcash",
        ZecLifecycleAction::RefundZcash => "refund_zcash",
        ZecLifecycleAction::Complete => "complete",
    }
}

pub(super) fn action_from_name(name: &str) -> Option<ZecLifecycleAction> {
    [
        ZecLifecycleAction::Wait,
        ZecLifecycleAction::CreateAndFundLez,
        ZecLifecycleAction::FundZcash,
        ZecLifecycleAction::ClaimLez,
        ZecLifecycleAction::ClaimZcash,
        ZecLifecycleAction::RefundZcash,
        ZecLifecycleAction::Complete,
    ]
    .into_iter()
    .find(|action| action_name(*action) == name)
}

fn snapshot_from(
    phase: Phase,
    revision: u64,
    next_action: ZecLifecycleAction,
) -> TakerActorProgressSnapshotV1 {
    let (state, available_action, _) = zec_actor_progress(phase, next_action);
    TakerActorProgressSnapshotV1 {
        phase: phase_name(phase).to_owned(),
        revision,
        next_action: action_name(next_action).to_owned(),
        state: serde_json::to_value(state)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "attention_required".to_owned()),
        available_action: available_action.and_then(|action| {
            serde_json::to_value(action)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
        }),
        observed_at: now_unix(),
    }
}

fn snapshot_of_status(status: ZecActorStatus) -> Option<TakerActorProgressSnapshotV1> {
    match status {
        ZecActorStatus::NotActivated => None,
        ZecActorStatus::Active {
            phase,
            revision,
            next_action,
            ..
        } => Some(snapshot_from(phase, revision, next_action)),
    }
}

fn snapshot_of_effect(effect: &serde_json::Value) -> Option<TakerActorProgressSnapshotV1> {
    let phase = phase_from_name(effect.get("phase")?.as_str()?)?;
    let revision = effect.get("revision")?.as_u64()?;
    let next_action = action_from_name(effect.get("next_action")?.as_str()?)?;
    Some(snapshot_from(phase, revision, next_action))
}

/// The schedule row of one swap, when the context can be read.
pub(super) async fn schedule_record(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    swap_id: &SwapId,
) -> Option<TakerActorScheduleRecordV1> {
    let initiation = Arc::clone(initiation);
    let id = swap_id.as_str().to_owned();
    tokio::task::spawn_blocking(move || {
        let mut context = initiation.lock().ok()?;
        context.schedule_mut().record(&id).ok().flatten()
    })
    .await
    .ok()
    .flatten()
}

/// The schedule's facts on the row: the last failure class, and a failed
/// row reads as needing attention with that class as the reason.
pub(super) fn overlay_schedule(
    view: &mut TakerSwapViewV1,
    record: Option<&TakerActorScheduleRecordV1>,
) {
    let Some(record) = record else {
        return;
    };
    view.last_failure_class = record.last_failure_class().map(str::to_owned);
    if record.schedule_state() == lez_actor_supervision::ScheduleState::Failed
        && !matches!(
            view.state,
            TakerSwapStateV1::Completed | TakerSwapStateV1::Refunded
        )
    {
        view.state = TakerSwapStateV1::AttentionRequired;
        view.available_action = None;
        if view.attention_reason.is_none() {
            view.attention_reason = record.last_failure_class().map(str::to_owned);
        }
    }
}

/// Whether a snapshot is recent enough to be served as a row.
pub(super) fn snapshot_is_fresh(snapshot: &TakerActorProgressSnapshotV1) -> bool {
    now_unix().saturating_sub(snapshot.observed_at) <= SNAPSHOT_FRESH_SECONDS
}

/// The row from the supervisor's last snapshot, for a read that met the
/// actor's lock held by a drive; `observed_at` says so.
pub(super) fn view_from_snapshot(
    facts: &TakerInitiationFactsV1,
    snapshot: &TakerActorProgressSnapshotV1,
) -> TakerSwapViewV1 {
    let (state, available_action, privacy_guidance) = match (
        phase_from_name(&snapshot.phase),
        action_from_name(&snapshot.next_action),
    ) {
        (Some(phase), Some(next_action)) => zec_actor_progress(phase, next_action),
        _ => (TakerSwapStateV1::AttentionRequired, None, None),
    };
    let privacy_guidance = privacy_guidance
        .filter(|_| facts.route().direction() == lez_swap_core::SwapDirection::TakerSellsLez);
    TakerSwapViewV1 {
        schema_version: 1,
        swap_id: facts.swap_id().clone(),
        offer_id: facts.offer_id().clone(),
        route: facts.route(),
        foreign_units: facts.foreign_units(),
        lez_units: facts.lez_units(),
        progress_generation: snapshot.revision,
        state,
        available_action,
        privacy_guidance,
        attention_reason: None,
        revealing_claim_held_by: None,
        last_failure_class: None,
        observed_at: Some(snapshot.observed_at),
        terms: None,
        effects: Vec::new(),
        bitcoin_lock_fee_sat: None,
    }
}

#[cfg(test)]
mod tests {
    use lez_actor_supervision::AttemptFailure;
    use lez_swap_core::Phase;
    use lez_zec_swap_sdk::ZecLifecycleAction;
    use zec_reference_actor::ActorCommandError;

    use super::{
        Decision, ZecActorStatus, action_from_name, action_name, decide, decide_with, failure_of,
        peer_revealing_claim_holder, peer_revealing_claim_in_flight, phase_from_name, phase_name,
        submits_lez_claim,
    };
    use crate::TakerTerminalActionV1;
    use zec_reference_actor::ActorCommand;

    const fn active(
        phase: Phase,
        revision: u64,
        next_action: ZecLifecycleAction,
    ) -> ZecActorStatus {
        ZecActorStatus::Active {
            phase,
            revision,
            next_action,
            lez_refund_at_ms: None,
            revealing_claim_hold_attention_ms: None,
        }
    }

    #[test]
    fn the_taker_is_activated_then_driven_through_its_lock_and_the_makers_legs() {
        assert_eq!(
            decide(ZecActorStatus::NotActivated, None),
            Decision::Activate
        );
        assert_eq!(
            decide(
                active(Phase::Offered, 0, ZecLifecycleAction::CreateAndFundLez),
                None
            ),
            Decision::Drive
        );
        // The reverse direction: the Taker's first lock is the Zcash funding.
        assert_eq!(
            decide(
                active(Phase::Offered, 0, ZecLifecycleAction::FundZcash),
                None
            ),
            Decision::Drive
        );
        for phase in [
            Phase::Offered,
            Phase::TakerLockConfirmed,
            Phase::BothLegsLocked,
        ] {
            assert_eq!(
                decide(active(phase, 3, ZecLifecycleAction::Wait), None),
                Decision::Drive,
                "{phase:?} is observed by a drive"
            );
        }
    }

    #[test]
    fn the_reverse_direction_funds_zcash_first_claims_lez_on_admission_and_observes_the_follow_up()
    {
        // TakerSellsForeign: the Taker's first lock is its Zcash funding.
        assert_eq!(
            decide(
                active(Phase::Offered, 0, ZecLifecycleAction::FundZcash),
                None
            ),
            Decision::Drive
        );
        // Both legs locked, the Taker is the LEZ claimant: its revealing claim
        // is the owner's, never driven; admitted at this generation it runs.
        let claimable = active(Phase::BothLegsLocked, 4, ZecLifecycleAction::ClaimLez);
        assert_eq!(decide(claimable, None), Decision::Observe);
        assert_eq!(
            decide(claimable, Some((TakerTerminalActionV1::Claim, 4))),
            Decision::Claim
        );
        // Its claim final, the Maker's Zcash follow-up claim is observed by a drive.
        assert_eq!(
            decide(
                active(Phase::ClaimEvidenceAvailable, 6, ZecLifecycleAction::Wait),
                None
            ),
            Decision::Drive
        );
        // The same while the owner's claim admission is still open (the live
        // shape: the admission at generation 4 is done, the actor moved on).
        for generation in [4, 6] {
            assert_eq!(
                decide(
                    active(Phase::ClaimEvidenceAvailable, 6, ZecLifecycleAction::Wait),
                    Some((TakerTerminalActionV1::Claim, generation))
                ),
                Decision::Drive
            );
        }
        // Selling ZEC the claim window also admits the owner's refund: a Taker
        // that never claims reaches its Zcash refund through the recovery.
        assert_eq!(
            decide(claimable, Some((TakerTerminalActionV1::Refund, 4))),
            Decision::Recover
        );
        // A refund admission at that shape stays an observation.
        assert_eq!(
            decide(
                active(Phase::ClaimEvidenceAvailable, 6, ZecLifecycleAction::Wait),
                Some((TakerTerminalActionV1::Refund, 6))
            ),
            Decision::Observe
        );
    }

    #[test]
    fn one_revealing_lez_claim_is_in_flight_per_node_until_it_is_final() {
        // The marker each actor writes beside its state (<actor root>/taker/)
        // is the gate: durable, released only when the actor removes it.
        let directory = tempfile::tempdir().unwrap();
        let roots: Vec<std::path::PathBuf> = ["a", "b", "c"]
            .iter()
            .map(|name| {
                let root = directory.path().join(name).join("actor");
                std::fs::create_dir_all(root.join("taker")).unwrap();
                root
            })
            .collect();
        let all = || roots.iter().map(std::path::PathBuf::as_path);
        assert!(!peer_revealing_claim_in_flight(&roots[1], all()));
        let marker = roots[0].join("taker").join("revealing-claim-in-flight");
        std::fs::write(&marker, b"in flight").unwrap();
        // The holder is not held back by itself; every other swap is.
        assert!(!peer_revealing_claim_in_flight(&roots[0], all()));
        assert!(peer_revealing_claim_in_flight(&roots[1], all()));
        assert!(peer_revealing_claim_in_flight(&roots[2], all()));
        // A swap that is no longer bound does not count.
        assert!(!peer_revealing_claim_in_flight(
            &roots[1],
            roots[1..].iter().map(std::path::PathBuf::as_path)
        ));
        // The held swap learns which swap holds it, to name it.
        let named = || {
            roots
                .iter()
                .map(std::path::PathBuf::as_path)
                .zip(["swap-a", "swap-b", "swap-c"])
        };
        assert_eq!(
            peer_revealing_claim_holder(&roots[2], named()),
            Some("swap-a")
        );
        assert_eq!(peer_revealing_claim_holder(&roots[0], named()), None);
        std::fs::remove_file(&marker).unwrap();
        assert!(!peer_revealing_claim_in_flight(&roots[1], all()));
        assert_eq!(peer_revealing_claim_holder(&roots[2], named()), None);
        // The marker name is the actor's.
        let state_db = roots[0].join("taker").join("state").join("zec.sqlite3");
        assert_eq!(
            zec_reference_actor::revealing_claim_in_flight_marker(&state_db).unwrap(),
            marker
        );
        // Only a claim at both legs locked is a LEZ claim.
        assert!(submits_lez_claim(
            Phase::BothLegsLocked,
            ActorCommand::Claim
        ));
        assert!(!submits_lez_claim(
            Phase::ClaimEvidenceAvailable,
            ActorCommand::Claim
        ));
        assert!(!submits_lez_claim(
            Phase::BothLegsLocked,
            ActorCommand::Drive
        ));
    }

    #[test]
    fn a_refund_in_the_claim_window_is_never_run_once_a_revealing_claim_was_submitted() {
        let claimable = active(Phase::BothLegsLocked, 4, ZecLifecycleAction::ClaimLez);
        assert_eq!(
            decide_with(claimable, Some((TakerTerminalActionV1::Refund, 4)), false),
            Decision::Recover
        );
        assert_eq!(
            decide_with(claimable, Some((TakerTerminalActionV1::Refund, 4)), true),
            Decision::Observe
        );
        // The claim itself is still pursued.
        assert_eq!(
            decide_with(claimable, Some((TakerTerminalActionV1::Claim, 4)), true),
            Decision::Claim
        );
    }

    #[test]
    fn claims_and_refunds_wait_for_the_owner_and_run_only_when_admitted() {
        // claim_available: never driven (a drive would be the Zcash claim).
        let claimable = active(
            Phase::ClaimEvidenceAvailable,
            5,
            ZecLifecycleAction::ClaimZcash,
        );
        assert_eq!(decide(claimable, None), Decision::Observe);
        assert_eq!(
            decide(claimable, Some((TakerTerminalActionV1::Claim, 5))),
            Decision::Claim
        );
        // Admitted at an older generation: the owner's paths say ProgressChanged; here, observe.
        assert_eq!(
            decide(claimable, Some((TakerTerminalActionV1::Claim, 4))),
            Decision::Observe
        );
        // A refund offered while the Maker's lock is awaited runs only once admitted.
        let refundable = active(Phase::TakerLockConfirmed, 2, ZecLifecycleAction::Wait);
        assert_eq!(decide(refundable, None), Decision::Drive);
        assert_eq!(
            decide(refundable, Some((TakerTerminalActionV1::Refund, 2))),
            Decision::Recover
        );
        // The Taker's leg is back, the Maker's refund still owed: the admitted refund keeps running.
        assert_eq!(
            decide(
                active(Phase::TakerLegRefunded, 7, ZecLifecycleAction::Wait),
                Some((TakerTerminalActionV1::Refund, 2))
            ),
            Decision::Recover
        );
        assert_eq!(
            decide(
                active(Phase::TakerLegRefunded, 7, ZecLifecycleAction::Wait),
                None
            ),
            Decision::Observe
        );
        // The Maker's phases the Taker never drives.
        for phase in [
            Phase::AwaitingTakerConfirmations,
            Phase::AwaitingMakerConfirmations,
            Phase::MakerLegRefunded,
            Phase::MakerRecoveryAvailable,
        ] {
            assert_eq!(
                decide(active(phase, 1, ZecLifecycleAction::Wait), None),
                Decision::Observe
            );
        }
    }

    #[test]
    fn terminal_phases_end_supervision_and_complete_an_admitted_action() {
        assert_eq!(
            decide(
                active(Phase::Completed, 9, ZecLifecycleAction::Complete),
                None
            ),
            Decision::Terminal
        );
        assert_eq!(
            decide(
                active(Phase::Completed, 9, ZecLifecycleAction::Complete),
                Some((TakerTerminalActionV1::Claim, 5))
            ),
            Decision::ManualActionCompleted
        );
        assert_eq!(
            decide(
                active(Phase::Refunded, 9, ZecLifecycleAction::Wait),
                Some((TakerTerminalActionV1::Refund, 2))
            ),
            Decision::ManualActionCompleted
        );
    }

    #[test]
    fn failure_classes_name_the_balance_refusal_and_tell_retries_from_stops() {
        assert_eq!(
            failure_of(ActorCommandError::LezDepositorBalanceInsufficient),
            AttemptFailure::Retry("lez_depositor_balance_insufficient")
        );
        assert_eq!(
            failure_of(ActorCommandError::DriveUnavailable),
            AttemptFailure::Retry("actor_drive_unavailable")
        );
        assert_eq!(
            failure_of(ActorCommandError::DriveConfigurationUnavailable),
            AttemptFailure::Fail("actor_deployment_invalid")
        );
        assert_eq!(
            failure_of(ActorCommandError::DriveMaterialUnavailable),
            AttemptFailure::Retry("actor_material_unavailable")
        );
        assert_eq!(
            failure_of(ActorCommandError::DriveStoreUnavailable),
            AttemptFailure::Retry("actor_store_unavailable")
        );
    }

    /// Every failure class this supervisor emits is a word of the shared
    /// alphabet, as the Maker's test asserts for its own.
    #[test]
    fn every_failure_class_the_taker_emits_is_in_the_shared_alphabet() {
        let classes =
            lez_actor_supervision::failure_class_literals(include_str!("zec_supervisor.rs"));
        assert!(classes.len() >= 7, "{classes:?}");
        for class in &classes {
            assert!(
                lez_actor_supervision::validate_failure_class(class),
                "{class} is not a failure class"
            );
        }
        assert!(lez_actor_supervision::validate_failure_class(
            lez_swap_store::COORDINATOR_RESTARTED
        ));
        assert!(lez_actor_supervision::validate_failure_class(
            super::REVEALING_CLAIM_HELD_NEAR_REFUND
        ));
    }

    #[test]
    fn the_actors_spellings_round_trip() {
        for phase in [
            Phase::Offered,
            Phase::ClaimEvidenceAvailable,
            Phase::MakerRecoveryAvailable,
        ] {
            assert_eq!(phase_from_name(phase_name(phase)), Some(phase));
        }
        for action in [
            ZecLifecycleAction::Wait,
            ZecLifecycleAction::CreateAndFundLez,
            ZecLifecycleAction::Complete,
        ] {
            assert_eq!(action_from_name(action_name(action)), Some(action));
        }
        assert_eq!(phase_from_name("locked"), None);
    }
}
