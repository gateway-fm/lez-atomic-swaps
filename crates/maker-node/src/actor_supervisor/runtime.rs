//! Bounded execution of one durable maker-actor scheduling attempt.

#[cfg(feature = "test-crash-hooks")]
use std::path::PathBuf;
use std::{
    fs,
    io::{self, Read},
    os::unix::process::CommandExt as _,
    process::{Child, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use lez_swap_core::SwapId;
use lez_swap_store::{
    MakerActorAttemptResolution, MakerActorHeldLock, MakerActorKindV1, MakerActorLeaseOwner,
    MakerActorLeaseV1, MakerActorManualAction, MakerActorProcessError,
    MakerActorProgressObservationV1, SqliteSwapStore,
};
use rustix::process::{Pid, Signal, kill_process_group};
use rustix::time::{ClockId, clock_gettime};
use serde_json::Value;
use thiserror::Error;
use wait_timeout::ChildExt as _;
use xmr_reference_actor::{
    XMR_MAKER_ACTOR_ABI_V1, XMR_MAKER_ACTOR_NEXT_ACTION, XMR_MAKER_ACTOR_PROGRAM_ID,
};

use super::prepare_maker_actor;

const MAX_ATTEMPT_TIMEOUT: Duration = Duration::from_mins(5);
const CHILD_WAIT_POLL: Duration = Duration::from_millis(20);
const MIN_OUTPUT_BYTES: usize = 256;
const MAX_OUTPUT_BYTES: usize = 64 * 1_024;
const XMR_BLOCKED_RECHECK_SECONDS: u64 = 60;

/// Immutable bounds for one supervisor scheduling cycle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerActorSupervisorConfig {
    attempt_timeout: Duration,
    requeue_delay_seconds: u64,
    failure_backoff_seconds: u64,
    max_output_bytes: usize,
    effect_cutoff_boottime_milliseconds: Option<u64>,
    #[cfg(feature = "test-crash-hooks")]
    test_pause: Option<MakerActorTestPause>,
}

#[cfg(feature = "test-crash-hooks")]
#[derive(Clone, Debug, Eq, PartialEq)]
struct MakerActorTestPause {
    swap_id: SwapId,
    operation: Box<str>,
    marker: PathBuf,
}

impl MakerActorSupervisorConfig {
    /// Validates finite process, retry, and output bounds.
    ///
    /// # Errors
    ///
    /// Rejects a zero or excessive timeout/delay, or an output cap outside the
    /// actor protocol's conservative range.
    pub fn new(
        attempt_timeout: Duration,
        requeue_delay_seconds: u64,
        failure_backoff_seconds: u64,
        max_output_bytes: usize,
    ) -> Result<Self, MakerActorSupervisorError> {
        if attempt_timeout.is_zero()
            || attempt_timeout > MAX_ATTEMPT_TIMEOUT
            || requeue_delay_seconds == 0
            || failure_backoff_seconds == 0
            || !(MIN_OUTPUT_BYTES..=MAX_OUTPUT_BYTES).contains(&max_output_bytes)
        {
            return Err(MakerActorSupervisorError::InvalidConfig);
        }
        Ok(Self {
            effect_cutoff_boottime_milliseconds: None,
            attempt_timeout,
            requeue_delay_seconds,
            failure_backoff_seconds,
            max_output_bytes,
            #[cfg(feature = "test-crash-hooks")]
            test_pause: None,
        })
    }

    /// Applies one absolute Linux boot-time cutoff to effect-capable children.
    ///
    /// # Errors
    ///
    /// Rejects zero, which cannot identify a valid post-boot deadline.
    pub fn with_effect_cutoff_boottime_milliseconds(
        mut self,
        cutoff_boottime_milliseconds: u64,
    ) -> Result<Self, MakerActorSupervisorError> {
        if cutoff_boottime_milliseconds == 0 {
            return Err(MakerActorSupervisorError::InvalidConfig);
        }
        self.effect_cutoff_boottime_milliseconds = Some(cutoff_boottime_milliseconds);
        Ok(self)
    }

    fn effect_cutoff_reached(&self) -> bool {
        self.effect_cutoff_boottime_milliseconds
            .is_some_and(|cutoff| boottime_milliseconds() >= cutoff)
    }

    /// Arms one exact submitted-effect pause in feature-gated fault tests.
    ///
    /// This API and its child environment injection do not exist in default
    /// production builds.
    ///
    /// # Errors
    ///
    /// Rejects an unknown submitted operation or a non-absolute marker path.
    #[cfg(feature = "test-crash-hooks")]
    pub fn with_test_pause_after_submitted(
        mut self,
        swap_id: SwapId,
        operation: impl Into<Box<str>>,
        marker: PathBuf,
    ) -> Result<Self, MakerActorSupervisorError> {
        let operation = operation.into();
        if !matches!(
            operation.as_ref(),
            "lez_initialize"
                | "lez_fund"
                | "zcash_fund"
                | "lez_revealing_claim"
                | "zcash_followup_claim"
        ) || !marker.is_absolute()
        {
            return Err(MakerActorSupervisorError::InvalidConfig);
        }
        self.test_pause = Some(MakerActorTestPause {
            swap_id,
            operation,
            marker,
        });
        Ok(self)
    }
}

fn boottime_milliseconds() -> u64 {
    let now = clock_gettime(ClockId::Boottime);
    let seconds = u64::try_from(now.tv_sec).unwrap_or(u64::MAX);
    let nanoseconds = u64::try_from(now.tv_nsec).unwrap_or(u64::MAX);
    seconds
        .saturating_mul(1_000)
        .saturating_add(nanoseconds / 1_000_000)
}

/// Cloneable one-way stop signal for an in-flight bounded actor cycle.
#[derive(Clone, Debug, Default)]
pub struct MakerActorSupervisorCancellation {
    cancelled: Arc<AtomicBool>,
}

impl MakerActorSupervisorCancellation {
    /// Creates an unset cancellation signal.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Repeated calls are idempotent.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Reports whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Durable classification produced by one bounded scheduling cycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MakerActorSupervisorResolution {
    /// Actor remains live and is queued for its next bounded observation/effect.
    Requeued,
    /// Valid XMR authority is queued, but chain effects are explicitly not composed yet.
    Blocked,
    /// A transient process/dependency failure was durably backed off.
    Backoff,
    /// Actor reported an absorbing completed/refunded phase.
    Terminal,
    /// Deployment or output violated the supervisor contract.
    Failed,
}

/// Secret-free result of one claimed actor cycle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerActorSupervisorOutcome {
    swap_id: SwapId,
    generation: u64,
    resolution: MakerActorSupervisorResolution,
}

impl MakerActorSupervisorOutcome {
    /// Application swap processed by this cycle.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Exact monotonic lease generation used by this cycle.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Durable scheduling result.
    #[must_use]
    pub const fn resolution(&self) -> MakerActorSupervisorResolution {
        self.resolution
    }
}

/// Failure to preserve the supervisor's durable scheduling contract.
#[derive(Debug, Error)]
pub enum MakerActorSupervisorError {
    /// Runtime bounds are absent or excessive.
    #[error("maker actor supervisor configuration is invalid")]
    InvalidConfig,
    /// A durable scheduling transition could not be completed safely.
    #[error("maker actor supervisor scheduling failed")]
    Scheduling(#[from] MakerActorProcessError),
}

/// Claims and runs at most one due actor under finite process/output bounds.
///
/// The pair-neutral scheduler first executes offline `status` from the exact
/// sealed deployment. It then chooses `activate`, `drive`, or BTC `recover`
/// from that output, while retaining one per-swap kernel lock across both
/// subprocesses. Every child identity is generation-fenced before waiting and
/// exact-cleared only after reap. Process failures become durable backoff or
/// failed states rather than escaping as an unowned lease.
///
/// # Errors
///
/// Returns an error only when the durable store/fence cannot safely complete;
/// actor deployment, process, timeout, and output failures are classified and
/// committed to the scheduler row.
pub fn supervise_one_due_maker_actor(
    store: &mut SqliteSwapStore,
    owner: MakerActorLeaseOwner,
    now: u64,
    config: &MakerActorSupervisorConfig,
) -> Result<Option<MakerActorSupervisorOutcome>, MakerActorSupervisorError> {
    supervise_one_due_maker_actor_until(
        store,
        owner,
        now,
        config,
        &MakerActorSupervisorCancellation::new(),
    )
}

/// Claims and runs at most one due actor with prompt cooperative cancellation.
///
/// Cancellation before claim performs no write. Cancellation after claim kills
/// and reaps the current isolated process group, exact-clears its child identity,
/// and durably backs off the leased row before returning.
///
/// # Errors
///
/// Returns an error only when the durable store/fence cannot safely complete.
pub fn supervise_one_due_maker_actor_until(
    store: &mut SqliteSwapStore,
    owner: MakerActorLeaseOwner,
    now: u64,
    config: &MakerActorSupervisorConfig,
    cancellation: &MakerActorSupervisorCancellation,
) -> Result<Option<MakerActorSupervisorOutcome>, MakerActorSupervisorError> {
    if cancellation.is_cancelled() || config.effect_cutoff_reached() {
        return Ok(None);
    }
    let Some(swap_id) = store.list_due_maker_actor_ids(now, 1)?.into_iter().next() else {
        return Ok(None);
    };
    let Some(lease) = store.claim_maker_actor(&swap_id, owner, now)? else {
        return Ok(None);
    };
    let claimed = match run_claimed_attempt(store, &lease, config, cancellation) {
        Ok(value) => value,
        Err(ClaimedAttemptError::Scheduling(error)) => return Err(error.into()),
    };
    resolve_claimed_attempt(store, &lease, now, config, claimed).map(Some)
}

/// Recovers and runs at most one abandoned durable actor lease.
///
/// A lease is eligible only when this process can acquire its exact per-swap
/// kernel lock, proving that neither the old coordinator nor any actor child
/// still owns the inherited descriptor. The owner/generation transfer is one
/// durable compare-and-swap and the lock remains held through execution and
/// resolution, so the row is never exposed as queued or unowned in between.
///
/// # Errors
///
/// Returns an error when lock validation or a durable scheduling transition
/// cannot be completed safely.
pub fn supervise_one_abandoned_maker_actor(
    store: &mut SqliteSwapStore,
    owner: MakerActorLeaseOwner,
    now: u64,
    config: &MakerActorSupervisorConfig,
) -> Result<Option<MakerActorSupervisorOutcome>, MakerActorSupervisorError> {
    supervise_one_abandoned_maker_actor_until(
        store,
        owner,
        now,
        config,
        &MakerActorSupervisorCancellation::new(),
    )
}

/// Recovers and runs at most one abandoned lease with cooperative cancellation.
///
/// Cancellation before the durable generation transfer leaves the old lease
/// untouched. Cancellation after transfer resolves the new generation to a
/// finite durable backoff while retaining the exact lock.
///
/// # Errors
///
/// Returns an error when lock validation or a durable scheduling transition
/// cannot be completed safely.
pub fn supervise_one_abandoned_maker_actor_until(
    store: &mut SqliteSwapStore,
    owner: MakerActorLeaseOwner,
    now: u64,
    config: &MakerActorSupervisorConfig,
    cancellation: &MakerActorSupervisorCancellation,
) -> Result<Option<MakerActorSupervisorOutcome>, MakerActorSupervisorError> {
    if cancellation.is_cancelled() || config.effect_cutoff_reached() {
        return Ok(None);
    }
    for lease in store.list_leased_maker_actors()? {
        if cancellation.is_cancelled() || config.effect_cutoff_reached() {
            return Ok(None);
        }
        let held_lock = match MakerActorHeldLock::acquire(lease.record()) {
            Ok(lock) => lock,
            Err(MakerActorProcessError::LockUnavailable) => continue,
            Err(error) => return Err(error.into()),
        };
        if cancellation.is_cancelled() || config.effect_cutoff_reached() {
            return Ok(None);
        }
        let recovered = match store.recover_abandoned_maker_actor(&lease, &held_lock, owner, now) {
            Ok(recovered) => recovered,
            Err(MakerActorProcessError::LeaseConflict) => continue,
            Err(error) => return Err(error.into()),
        };
        let claimed =
            match run_claimed_attempt_with_lock(store, &recovered, held_lock, config, cancellation)
            {
                Ok(value) => value,
                Err(ClaimedAttemptError::Scheduling(error)) => return Err(error.into()),
            };
        return resolve_claimed_attempt(store, &recovered, now, config, claimed).map(Some);
    }
    Ok(None)
}

fn resolve_claimed_attempt(
    store: &mut SqliteSwapStore,
    lease: &MakerActorLeaseV1,
    now: u64,
    config: &MakerActorSupervisorConfig,
    claimed: ClaimedAttemptResult,
) -> Result<MakerActorSupervisorOutcome, MakerActorSupervisorError> {
    let ClaimedAttemptResult {
        attempt,
        progress,
        held_lock,
    } = claimed;
    let (durable, resolution) = match attempt {
        ClaimedAttempt::Requeue => (
            MakerActorAttemptResolution::Requeue {
                not_before: now.saturating_add(config.requeue_delay_seconds),
            },
            MakerActorSupervisorResolution::Requeued,
        ),
        ClaimedAttempt::Blocked => (
            MakerActorAttemptResolution::Requeue {
                not_before: now.saturating_add(
                    config
                        .requeue_delay_seconds
                        .max(XMR_BLOCKED_RECHECK_SECONDS),
                ),
            },
            MakerActorSupervisorResolution::Blocked,
        ),
        ClaimedAttempt::Backoff(failure_class) => (
            MakerActorAttemptResolution::Backoff {
                not_before: now.saturating_add(config.failure_backoff_seconds),
                failure_class: failure_class.into(),
            },
            MakerActorSupervisorResolution::Backoff,
        ),
        ClaimedAttempt::Terminal => (
            MakerActorAttemptResolution::Terminal,
            MakerActorSupervisorResolution::Terminal,
        ),
        ClaimedAttempt::ManualActionCompleted => (
            MakerActorAttemptResolution::ManualActionCompleted,
            MakerActorSupervisorResolution::Terminal,
        ),
        ClaimedAttempt::Failed(failure_class) => (
            MakerActorAttemptResolution::Failed {
                failure_class: failure_class.into(),
            },
            MakerActorSupervisorResolution::Failed,
        ),
    };
    if let Some(progress) = progress.as_ref() {
        store.resolve_maker_actor_attempt_with_progress(lease, durable, progress, now)?;
    } else {
        store.resolve_maker_actor_attempt(lease, durable, now)?;
    }
    drop(held_lock);
    Ok(MakerActorSupervisorOutcome {
        swap_id: lease.record().swap_id().clone(),
        generation: lease.generation(),
        resolution,
    })
}

enum ClaimedAttempt {
    Requeue,
    Blocked,
    Backoff(&'static str),
    Terminal,
    ManualActionCompleted,
    Failed(&'static str),
}

struct ClaimedAttemptResult {
    attempt: ClaimedAttempt,
    progress: Option<MakerActorProgressObservationV1>,
    held_lock: Option<MakerActorHeldLock>,
}

enum ClaimedAttemptError {
    Scheduling(MakerActorProcessError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActorEffectCommand {
    Activate,
    Drive,
    Claim,
    Recover,
}

impl ActorEffectCommand {
    const fn name(self) -> &'static str {
        match self {
            Self::Activate => "activate",
            Self::Drive => "drive",
            Self::Claim => "claim",
            Self::Recover => "recover",
        }
    }
}

#[derive(Clone, Copy)]
enum ActorInvocation {
    Status,
    Effect(ActorEffectCommand),
}

impl ActorInvocation {
    const fn name(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Effect(command) => command.name(),
        }
    }

    const fn is_effect(self) -> bool {
        matches!(self, Self::Effect(_))
    }
}

fn run_claimed_attempt(
    store: &mut SqliteSwapStore,
    lease: &MakerActorLeaseV1,
    config: &MakerActorSupervisorConfig,
    cancellation: &MakerActorSupervisorCancellation,
) -> Result<ClaimedAttemptResult, ClaimedAttemptError> {
    let held_lock = match MakerActorHeldLock::acquire(lease.record()) {
        Ok(lock) => lock,
        Err(MakerActorProcessError::LockUnavailable) => {
            return Ok(ClaimedAttemptResult {
                attempt: ClaimedAttempt::Backoff("actor_lock_unavailable"),
                progress: None,
                held_lock: None,
            });
        }
        Err(_) => {
            return Ok(ClaimedAttemptResult {
                attempt: ClaimedAttempt::Failed("actor_lock_invalid"),
                progress: None,
                held_lock: None,
            });
        }
    };
    run_claimed_attempt_with_lock(store, lease, held_lock, config, cancellation)
}

fn run_claimed_attempt_with_lock(
    store: &mut SqliteSwapStore,
    lease: &MakerActorLeaseV1,
    held_lock: MakerActorHeldLock,
    config: &MakerActorSupervisorConfig,
    cancellation: &MakerActorSupervisorCancellation,
) -> Result<ClaimedAttemptResult, ClaimedAttemptError> {
    if cancellation.is_cancelled() {
        return Ok(ClaimedAttemptResult {
            attempt: ClaimedAttempt::Backoff("actor_cancelled"),
            progress: None,
            held_lock: Some(held_lock),
        });
    }
    if config.effect_cutoff_reached() {
        return Ok(ClaimedAttemptResult {
            attempt: ClaimedAttempt::Backoff("actor_effect_cutoff"),
            progress: None,
            held_lock: Some(held_lock),
        });
    }
    let manual_action = store
        .claim_maker_actor_manual_action(lease)
        .map_err(ClaimedAttemptError::Scheduling)?
        .map(|action| action.action());
    let mut progress = None;
    let attempt = (|| {
        let status = match run_child(
            store,
            lease,
            &held_lock,
            ActorInvocation::Status,
            config,
            cancellation,
        ) {
            Ok(output) => output,
            Err(ChildRunError::Retry(class)) => return Ok(ClaimedAttempt::Backoff(class)),
            Err(ChildRunError::Fail(class)) => return Ok(ClaimedAttempt::Failed(class)),
            Err(ChildRunError::Scheduling(error)) => {
                return Err(ClaimedAttemptError::Scheduling(error));
            }
        };
        let Ok(parsed_status) = parse_status(&status, lease.record().manifest().kind()) else {
            return Ok(ClaimedAttempt::Failed("actor_output_invalid"));
        };
        let ParsedStatus {
            decision: status_decision,
            progress: status_progress,
            revision: status_revision,
        } = parsed_status;
        progress = Some(status_progress);
        if matches!(status_decision, StatusDecision::Blocked) && manual_action.is_none() {
            return Ok(ClaimedAttempt::Blocked);
        }
        let command = match manual_action {
            Some(action) => manual_effect_command(lease.record().manifest().kind(), action),
            None => match status_decision {
                StatusDecision::Blocked => return Ok(ClaimedAttempt::Blocked),
                StatusDecision::Terminal => return Ok(ClaimedAttempt::Terminal),
                StatusDecision::Run(command) => command,
            },
        };
        if config.effect_cutoff_reached() {
            return Ok(ClaimedAttempt::Backoff("actor_effect_cutoff"));
        }
        let effect = match run_child(
            store,
            lease,
            &held_lock,
            ActorInvocation::Effect(command),
            config,
            cancellation,
        ) {
            Ok(output) => output,
            Err(ChildRunError::Retry(class)) => return Ok(ClaimedAttempt::Backoff(class)),
            Err(ChildRunError::Fail(class)) => return Ok(ClaimedAttempt::Failed(class)),
            Err(ChildRunError::Scheduling(error)) => {
                return Err(ClaimedAttemptError::Scheduling(error));
            }
        };
        let Ok(parsed_effect) = parse_effect(&effect, command, lease.record().manifest().kind())
        else {
            return Ok(ClaimedAttempt::Failed("actor_output_invalid"));
        };
        if status_revision.is_some_and(|revision| parsed_effect.revision < revision) {
            return Ok(ClaimedAttempt::Failed("actor_output_invalid"));
        }
        progress = Some(parsed_effect.progress);
        match (parsed_effect.terminal, manual_action) {
            (true, Some(_)) => Ok(ClaimedAttempt::ManualActionCompleted),
            (true, None) => Ok(ClaimedAttempt::Terminal),
            (false, _) => Ok(ClaimedAttempt::Requeue),
        }
    })()?;
    Ok(ClaimedAttemptResult {
        attempt,
        progress,
        held_lock: Some(held_lock),
    })
}

const fn manual_effect_command(
    kind: MakerActorKindV1,
    action: MakerActorManualAction,
) -> ActorEffectCommand {
    match (kind, action) {
        (MakerActorKindV1::Bitcoin, MakerActorManualAction::Claim) => ActorEffectCommand::Drive,
        (MakerActorKindV1::Monero | MakerActorKindV1::Zcash, MakerActorManualAction::Claim) => {
            ActorEffectCommand::Claim
        }
        (_, MakerActorManualAction::Refund) => ActorEffectCommand::Recover,
    }
}

enum ChildRunError {
    Retry(&'static str),
    Fail(&'static str),
    Scheduling(MakerActorProcessError),
}

fn ensure_invocation_admitted(
    invocation: ActorInvocation,
    config: &MakerActorSupervisorConfig,
    cancellation: &MakerActorSupervisorCancellation,
) -> Result<(), ChildRunError> {
    if cancellation.is_cancelled() {
        return Err(ChildRunError::Retry("actor_cancelled"));
    }
    if invocation.is_effect() && config.effect_cutoff_reached() {
        return Err(ChildRunError::Retry("actor_effect_cutoff"));
    }
    Ok(())
}

fn run_child(
    store: &mut SqliteSwapStore,
    lease: &MakerActorLeaseV1,
    held_lock: &MakerActorHeldLock,
    invocation: ActorInvocation,
    config: &MakerActorSupervisorConfig,
    cancellation: &MakerActorSupervisorCancellation,
) -> Result<Vec<u8>, ChildRunError> {
    ensure_invocation_admitted(invocation, config, cancellation)?;
    let actor_command = invocation.name();
    let artifacts =
        prepare_maker_actor(lease.record()).map_err(|error| classify_deployment(&error))?;
    let transfers_lock =
        lease.record().manifest().kind() == MakerActorKindV1::Monero && invocation.is_effect();
    let mut command = if transfers_lock {
        artifacts.into_effect_command(held_lock)
    } else {
        artifacts.into_command(held_lock)
    }
    .map_err(|error| classify_deployment(&error))?;
    ensure_invocation_admitted(invocation, config, cancellation)?;
    command
        .args(["--config-fd", "196", actor_command])
        .env_clear()
        .current_dir("/")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    if !transfers_lock {
        command.stdin(Stdio::null());
    }
    #[cfg(feature = "test-crash-hooks")]
    if actor_command == "drive"
        && config
            .test_pause
            .as_ref()
            .is_some_and(|pause| pause.swap_id == *lease.record().swap_id())
    {
        let pause = config.test_pause.as_ref().expect("matching test pause");
        command
            .env(
                "LEZ_ACTOR_TEST_PAUSE_AFTER_SUBMITTED",
                pause.operation.as_ref(),
            )
            .env("LEZ_ACTOR_TEST_PAUSE_MARKER", &pause.marker);
    }
    ensure_invocation_admitted(invocation, config, cancellation)?;
    let mut child = command
        .spawn()
        .map_err(|_| ChildRunError::Retry("actor_spawn_failed"))?;
    let pid = child.id();
    let Some(stdout) = child.stdout.take() else {
        kill_and_reap(&mut child);
        return Err(ChildRunError::Fail("actor_output_invalid"));
    };
    let Ok(start_ticks) = child_start_ticks(pid) else {
        kill_and_reap(&mut child);
        return Err(ChildRunError::Retry("actor_identity_unavailable"));
    };
    if let Err(error) = store.record_maker_actor_child(lease, pid, start_ticks) {
        kill_and_reap(&mut child);
        return Err(ChildRunError::Scheduling(error));
    }
    let maximum = config.max_output_bytes;
    let Ok(reader) = thread::Builder::new()
        .name("lez-maker-actor-output".to_owned())
        .spawn(move || read_bounded_and_drain(stdout, maximum))
    else {
        kill_and_reap(&mut child);
        store
            .clear_maker_actor_child(lease, pid, start_ticks)
            .map_err(ChildRunError::Scheduling)?;
        return Err(ChildRunError::Retry("actor_output_unavailable"));
    };
    let status = wait_for_child(&mut child, invocation, config, cancellation);
    let output = match reader.join() {
        Ok(result) => result.map_err(|_| ChildRunError::Fail("actor_output_invalid")),
        Err(_) => Err(ChildRunError::Fail("actor_output_invalid")),
    };
    store
        .clear_maker_actor_child(lease, pid, start_ticks)
        .map_err(ChildRunError::Scheduling)?;
    let status = status?;
    let output = output?;
    if !status.success() {
        return Err(classify_exit(status));
    }
    if output.len() > maximum || output.is_empty() {
        return Err(ChildRunError::Fail("actor_output_invalid"));
    }
    Ok(output)
}

fn classify_deployment(error: &MakerActorProcessError) -> ChildRunError {
    match error {
        MakerActorProcessError::ArtifactPreparation | MakerActorProcessError::LockInheritance => {
            ChildRunError::Retry("actor_spawn_failed")
        }
        _ => ChildRunError::Fail("actor_deployment_invalid"),
    }
}

fn classify_exit(_status: ExitStatus) -> ChildRunError {
    ChildRunError::Retry("actor_exit_failed")
}

fn wait_for_child(
    child: &mut Child,
    invocation: ActorInvocation,
    config: &MakerActorSupervisorConfig,
    cancellation: &MakerActorSupervisorCancellation,
) -> Result<ExitStatus, ChildRunError> {
    let deadline = Instant::now() + config.attempt_timeout;
    loop {
        if let Err(error) = ensure_invocation_admitted(invocation, config, cancellation) {
            kill_and_reap(child);
            return Err(error);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            kill_and_reap(child);
            return Err(ChildRunError::Retry("actor_timeout"));
        }
        let remaining = if invocation.is_effect() {
            config
                .effect_cutoff_boottime_milliseconds
                .map(|cutoff| Duration::from_millis(cutoff.saturating_sub(boottime_milliseconds())))
                .map_or(remaining, |budget| remaining.min(budget))
        } else {
            remaining
        };
        match child.wait_timeout(remaining.min(CHILD_WAIT_POLL)) {
            Ok(Some(status)) => {
                terminate_process_group(child.id());
                return Ok(status);
            }
            Ok(None) => {}
            Err(_) => {
                kill_and_reap(child);
                return Err(ChildRunError::Retry("actor_wait_failed"));
            }
        }
    }
}

fn terminate_process_group(raw_pid: u32) {
    if let Some(pid) = Pid::from_raw(raw_pid.cast_signed()) {
        let _ = kill_process_group(pid, Signal::KILL);
    }
}
fn kill_and_reap(child: &mut Child) {
    terminate_process_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

fn read_bounded_and_drain(mut stream: impl Read, maximum: usize) -> io::Result<Vec<u8>> {
    let mut retained = Vec::with_capacity(maximum.saturating_add(1));
    let mut buffer = [0_u8; 8_192];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if retained.len() <= maximum {
            let remaining = maximum.saturating_add(1).saturating_sub(retained.len());
            retained.extend_from_slice(&buffer[..read.min(remaining)]);
        }
    }
    Ok(retained)
}

fn child_start_ticks(pid: u32) -> Result<u64, ()> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|_| ())?;
    let (_, fields) = stat.rsplit_once(") ").ok_or(())?;
    fields
        .split_whitespace()
        .nth(19)
        .ok_or(())?
        .parse()
        .map_err(|_| ())
}

struct ParsedStatus {
    decision: StatusDecision,
    progress: MakerActorProgressObservationV1,
    revision: Option<u64>,
}

#[derive(Clone, Copy)]
enum StatusDecision {
    Run(ActorEffectCommand),
    Blocked,
    Terminal,
}

fn exact_xmr_pre_effect_status_shape(value: &Value) -> bool {
    const KEYS: [&str; 9] = [
        "schema_version",
        "actor_program",
        "actor_abi",
        "role",
        "state",
        "phase",
        "revision",
        "next_action",
        "chain_effect_executed",
    ];
    value.as_object().is_some_and(|object| {
        object.len() == KEYS.len()
            && KEYS.iter().all(|key| object.contains_key(*key))
            && object.get("chain_effect_executed").and_then(Value::as_bool) == Some(false)
    })
}

fn parse_status(bytes: &[u8], kind: MakerActorKindV1) -> Result<ParsedStatus, ()> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    if value.get("schema_version").and_then(Value::as_u64) != Some(1)
        || value.get("role").and_then(Value::as_str) != Some("maker")
        || (kind == MakerActorKindV1::Monero
            && (!exact_xmr_pre_effect_status_shape(&value)
                || value.get("actor_program").and_then(Value::as_str)
                    != Some(XMR_MAKER_ACTOR_PROGRAM_ID)
                || value.get("actor_abi").and_then(Value::as_str) != Some(XMR_MAKER_ACTOR_ABI_V1)))
    {
        return Err(());
    }
    match value.get("state").and_then(Value::as_str) {
        Some("not_activated") if kind == MakerActorKindV1::Monero => Err(()),
        Some("not_activated") => Ok(ParsedStatus {
            decision: StatusDecision::Run(ActorEffectCommand::Activate),
            progress: MakerActorProgressObservationV1::NotActivated,
            revision: None,
        }),
        Some("active") => {
            let phase = value.get("phase").and_then(Value::as_str).ok_or(())?;
            let next_action = value.get("next_action").and_then(Value::as_str).ok_or(())?;
            let revision = value.get("revision").and_then(Value::as_u64).ok_or(())?;
            let progress = parse_active_progress(&value, kind)?;
            if terminal_phase(phase) != (next_action == "complete") {
                return Err(());
            }
            let decision = if kind == MakerActorKindV1::Monero {
                if phase != "offered" || revision != 0 || next_action != XMR_MAKER_ACTOR_NEXT_ACTION
                {
                    return Err(());
                }
                StatusDecision::Blocked
            } else if terminal_phase(phase) {
                StatusDecision::Terminal
            } else if kind == MakerActorKindV1::Zcash
                && matches!(next_action, "claim_lez" | "claim_zcash")
            {
                StatusDecision::Run(ActorEffectCommand::Claim)
            } else if matches!(
                (kind, next_action),
                (MakerActorKindV1::Zcash, "refund_zcash")
                    | (MakerActorKindV1::Bitcoin, "recover_taker_leg")
            ) {
                StatusDecision::Run(ActorEffectCommand::Recover)
            } else {
                StatusDecision::Run(ActorEffectCommand::Drive)
            };
            Ok(ParsedStatus {
                decision,
                progress,
                revision: Some(revision),
            })
        }
        _ => Err(()),
    }
}

struct ParsedEffect {
    terminal: bool,
    progress: MakerActorProgressObservationV1,
    revision: u64,
}

fn parse_effect(
    bytes: &[u8],
    command: ActorEffectCommand,
    kind: MakerActorKindV1,
) -> Result<ParsedEffect, ()> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    let outcome = value.get("outcome").and_then(Value::as_str).ok_or(())?;
    if value.get("schema_version").and_then(Value::as_u64) != Some(1)
        || value.get("role").and_then(Value::as_str) != Some("maker")
        || value.get("command").and_then(Value::as_str) != Some(command.name())
        || !known_effect_outcome(kind, command, outcome)
    {
        return Err(());
    }
    let phase = value.get("phase").and_then(Value::as_str).ok_or(())?;
    let revision = value.get("revision").and_then(Value::as_u64).ok_or(())?;
    let progress = parse_active_progress(&value, kind)?;
    let next_action = value.get("next_action").and_then(Value::as_str).ok_or(())?;
    if terminal_phase(phase) != (next_action == "complete") {
        return Err(());
    }
    let terminal = terminal_phase(phase);
    let projected_zec_claim = kind == MakerActorKindV1::Zcash
        && matches!(
            command,
            ActorEffectCommand::Drive | ActorEffectCommand::Claim
        )
        && outcome == "projected";
    let projected_zec_claim_terminal = if projected_zec_claim {
        match (
            value.get("operation").and_then(Value::as_str),
            phase,
            next_action,
        ) {
            (Some("lez_revealing_claim"), "claim_evidence_available", "wait") => false,
            (Some("zcash_followup_claim"), "completed", "complete") => true,
            (Some("lez_revealing_claim" | "zcash_followup_claim"), _, _) => return Err(()),
            _ if matches!(command, ActorEffectCommand::Claim) => return Err(()),
            _ => false,
        }
    } else {
        false
    };
    let exact_absorbing =
        exact_absorbing_effect(kind, command, outcome, phase) || projected_zec_claim_terminal;
    let terminal_outcome =
        matches!(outcome, "completed" | "refunded") || projected_zec_claim_terminal;
    if terminal != exact_absorbing
        || (kind == MakerActorKindV1::Zcash && terminal_outcome != exact_absorbing)
    {
        return Err(());
    }
    Ok(ParsedEffect {
        terminal,
        progress,
        revision,
    })
}

fn exact_absorbing_effect(
    kind: MakerActorKindV1,
    command: ActorEffectCommand,
    outcome: &str,
    phase: &str,
) -> bool {
    match kind {
        MakerActorKindV1::Monero => matches!(
            (command, outcome, phase),
            (ActorEffectCommand::Recover, "refunded", "refunded")
        ),
        MakerActorKindV1::Zcash => matches!(
            (command, outcome, phase),
            (
                ActorEffectCommand::Drive | ActorEffectCommand::Claim,
                "completed",
                "completed"
            ) | (ActorEffectCommand::Recover, "refunded", "refunded")
        ),
        MakerActorKindV1::Bitcoin => matches!(
            (command, outcome, phase),
            (
                ActorEffectCommand::Drive,
                "observed_then_projected" | "converged_on_existing_projection",
                "completed"
            ) | (
                ActorEffectCommand::Recover,
                "observed_then_projected" | "converged_on_existing_projection",
                "refunded"
            )
        ),
    }
}

fn parse_active_progress(
    value: &Value,
    kind: MakerActorKindV1,
) -> Result<MakerActorProgressObservationV1, ()> {
    let phase = value.get("phase").and_then(Value::as_str).ok_or(())?;
    let revision = value.get("revision").and_then(Value::as_u64).ok_or(())?;
    let next_action = value.get("next_action").and_then(Value::as_str).ok_or(())?;
    if !known_phase(phase) || !known_next_action(kind, next_action) {
        return Err(());
    }
    MakerActorProgressObservationV1::active(phase, revision, next_action).map_err(|_| ())
}

fn known_effect_outcome(
    kind: MakerActorKindV1,
    command: ActorEffectCommand,
    outcome: &str,
) -> bool {
    match (kind, command) {
        (MakerActorKindV1::Bitcoin | MakerActorKindV1::Zcash, ActorEffectCommand::Activate) => {
            outcome == "activated"
        }
        (MakerActorKindV1::Bitcoin, ActorEffectCommand::Drive) => matches!(
            outcome,
            "awaiting_observation"
                | "observed_then_projected"
                | "converged_on_existing_projection"
                | "not_yet_composed"
        ),
        (MakerActorKindV1::Bitcoin, ActorEffectCommand::Recover) => matches!(
            outcome,
            "awaiting_observation" | "observed_then_projected" | "converged_on_existing_projection"
        ),
        (MakerActorKindV1::Zcash, ActorEffectCommand::Drive) => matches!(
            outcome,
            "submitted"
                | "awaiting_observation"
                | "awaiting_safe_zcash_funding"
                | "unchanged"
                | "projected"
                | "completed"
        ),
        (MakerActorKindV1::Zcash, ActorEffectCommand::Claim) => matches!(
            outcome,
            "submitted"
                | "awaiting_observation"
                | "awaiting_safe_zcash_funding"
                | "projected"
                | "completed"
        ),
        (MakerActorKindV1::Zcash, ActorEffectCommand::Recover) => matches!(
            outcome,
            "submitted"
                | "awaiting_observation"
                | "awaiting_funding"
                | "awaiting_deadline"
                | "submission_rejected"
                | "submission_outcome_unknown"
                | "projected"
                | "refunded"
        ),
        (MakerActorKindV1::Monero, ActorEffectCommand::Recover) => {
            matches!(outcome, "awaiting_observation" | "refunded")
        }
        (MakerActorKindV1::Monero, _) | (MakerActorKindV1::Bitcoin, ActorEffectCommand::Claim) => {
            false
        }
    }
}

fn known_phase(phase: &str) -> bool {
    matches!(
        phase,
        "offered"
            | "awaiting_taker_confirmations"
            | "taker_lock_confirmed"
            | "awaiting_maker_confirmations"
            | "both_legs_locked"
            | "taker_lock_reorged"
            | "maker_lock_reorged"
            | "claim_evidence_available"
            | "completed"
            | "maker_leg_refunded"
            | "taker_leg_refunded"
            | "refunded"
            | "maker_recovery_available"
    )
}

fn known_next_action(kind: MakerActorKindV1, next_action: &str) -> bool {
    match kind {
        MakerActorKindV1::Monero => matches!(next_action, XMR_MAKER_ACTOR_NEXT_ACTION | "complete"),
        MakerActorKindV1::Zcash => matches!(
            next_action,
            "wait"
                | "create_and_fund_lez"
                | "fund_zcash"
                | "claim_lez"
                | "claim_zcash"
                | "refund_zcash"
                | "complete"
        ),
        MakerActorKindV1::Bitcoin => matches!(
            next_action,
            "observe_taker_first_lock"
                | "observe_maker_second_lock_or_recover_taker_leg"
                | "observe_revealing_claim"
                | "observe_followup_claim"
                | "recover_taker_leg"
                | "later_revision_not_yet_composed"
                | "complete"
        ),
    }
}

fn terminal_phase(phase: &str) -> bool {
    matches!(phase, "completed" | "refunded")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effect(
        command: &str,
        outcome: &str,
        phase: &str,
        revision: u64,
        next_action: &str,
    ) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "role": "maker",
            "command": command,
            "outcome": outcome,
            "phase": phase,
            "revision": revision,
            "next_action": next_action
        }))
        .unwrap()
    }

    #[test]
    fn manual_actions_map_to_pair_semantic_commands() {
        assert_eq!(
            manual_effect_command(MakerActorKindV1::Bitcoin, MakerActorManualAction::Claim),
            ActorEffectCommand::Drive
        );
        assert_eq!(
            manual_effect_command(MakerActorKindV1::Zcash, MakerActorManualAction::Claim),
            ActorEffectCommand::Claim
        );
        assert_eq!(
            manual_effect_command(MakerActorKindV1::Monero, MakerActorManualAction::Claim),
            ActorEffectCommand::Claim
        );
        for kind in [
            MakerActorKindV1::Bitcoin,
            MakerActorKindV1::Monero,
            MakerActorKindV1::Zcash,
        ] {
            assert_eq!(
                manual_effect_command(kind, MakerActorManualAction::Refund),
                ActorEffectCommand::Recover
            );
        }
    }

    fn zec_projected_claim_terminal_effect(operation: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "role": "maker",
            "command": "claim",
            "outcome": "projected",
            "operation": operation,
            "phase": "completed",
            "revision": 4,
            "next_action": "complete"
        }))
        .unwrap()
    }

    fn status(phase: &str, revision: u64, next_action: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "role": "maker",
            "state": "active",
            "phase": phase,
            "revision": revision,
            "next_action": next_action
        }))
        .unwrap()
    }

    fn xmr_pre_effect_status() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "actor_program": XMR_MAKER_ACTOR_PROGRAM_ID,
            "actor_abi": XMR_MAKER_ACTOR_ABI_V1,
            "role": "maker",
            "state": "active",
            "phase": "offered",
            "revision": 0,
            "next_action": XMR_MAKER_ACTOR_NEXT_ACTION,
            "chain_effect_executed": false
        }))
        .unwrap()
    }

    #[test]
    fn xmr_pre_effect_status_is_typed_blocked_and_binds_program_abi() {
        let parsed = parse_status(&xmr_pre_effect_status(), MakerActorKindV1::Monero).unwrap();
        assert!(matches!(parsed.decision, StatusDecision::Blocked));
        assert_eq!(
            parsed.progress,
            MakerActorProgressObservationV1::active(
                "offered",
                0,
                "xmr_chain_effects_not_yet_composed"
            )
            .unwrap()
        );

        for (field, value) in [
            ("actor_program", "not-the-xmr-reference-actor"),
            ("actor_abi", "lez_maker_xmr_future_v2"),
        ] {
            let mut output: Value = serde_json::from_slice(&xmr_pre_effect_status()).unwrap();
            output[field] = Value::from(value);
            assert!(
                parse_status(
                    &serde_json::to_vec(&output).unwrap(),
                    MakerActorKindV1::Monero
                )
                .is_err()
            );
        }

        for invalid in [
            serde_json::json!(true),
            serde_json::json!("false"),
            Value::Null,
        ] {
            let mut output: Value = serde_json::from_slice(&xmr_pre_effect_status()).unwrap();
            output["chain_effect_executed"] = invalid;
            assert!(
                parse_status(
                    &serde_json::to_vec(&output).unwrap(),
                    MakerActorKindV1::Monero
                )
                .is_err()
            );
        }
        let mut missing: Value = serde_json::from_slice(&xmr_pre_effect_status()).unwrap();
        missing
            .as_object_mut()
            .unwrap()
            .remove("chain_effect_executed");
        assert!(
            parse_status(
                &serde_json::to_vec(&missing).unwrap(),
                MakerActorKindV1::Monero
            )
            .is_err()
        );
        let mut extra: Value = serde_json::from_slice(&xmr_pre_effect_status()).unwrap();
        extra["unexpected"] = Value::from(false);
        assert!(
            parse_status(
                &serde_json::to_vec(&extra).unwrap(),
                MakerActorKindV1::Monero
            )
            .is_err()
        );
    }

    #[test]
    fn xmr_recover_effect_is_exactly_nonterminal_until_finalized() {
        let pending = effect(
            "recover",
            "awaiting_observation",
            "maker_recovery_available",
            1,
            XMR_MAKER_ACTOR_NEXT_ACTION,
        );
        let pending = parse_effect(
            &pending,
            ActorEffectCommand::Recover,
            MakerActorKindV1::Monero,
        )
        .expect("unreconciled Tag17 is retryable observation state");
        assert!(!pending.terminal);
        assert_eq!(
            pending.progress,
            MakerActorProgressObservationV1::active(
                "maker_recovery_available",
                1,
                XMR_MAKER_ACTOR_NEXT_ACTION,
            )
            .unwrap()
        );

        let finalized = effect("recover", "refunded", "refunded", 2, "complete");
        let finalized = parse_effect(
            &finalized,
            ActorEffectCommand::Recover,
            MakerActorKindV1::Monero,
        )
        .expect("finalized Tag17 is an absorbing Maker recovery");
        assert!(finalized.terminal);
        assert_eq!(
            finalized.progress,
            MakerActorProgressObservationV1::active("refunded", 2, "complete").unwrap()
        );

        for crossed in [
            effect("claim", "refunded", "refunded", 2, "complete"),
            effect("recover", "completed", "completed", 2, "complete"),
        ] {
            assert!(
                parse_effect(
                    &crossed,
                    ActorEffectCommand::Recover,
                    MakerActorKindV1::Monero,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn zec_status_routes_claim_and_refund_without_generic_drive() {
        for (next_action, expected) in [
            ("claim_lez", ActorEffectCommand::Claim),
            ("claim_zcash", ActorEffectCommand::Claim),
            ("refund_zcash", ActorEffectCommand::Recover),
            ("wait", ActorEffectCommand::Drive),
        ] {
            let parsed = parse_status(
                &status("both_legs_locked", 3, next_action),
                MakerActorKindV1::Zcash,
            )
            .unwrap();
            let StatusDecision::Run(actual) = parsed.decision else {
                panic!("nonterminal status must select one command");
            };
            assert_eq!(actual.name(), expected.name());
        }
    }

    #[test]
    fn parser_accepts_real_revision_zero_and_pair_specific_terminal_effects() {
        let activated = parse_status(
            &status("offered", 0, "observe_taker_first_lock"),
            MakerActorKindV1::Bitcoin,
        )
        .unwrap();
        assert_eq!(
            activated.progress,
            MakerActorProgressObservationV1::active("offered", 0, "observe_taker_first_lock")
                .unwrap()
        );

        for (kind, command, output, phase) in [
            (
                MakerActorKindV1::Zcash,
                ActorEffectCommand::Claim,
                effect("claim", "completed", "completed", 4, "complete"),
                "completed",
            ),
            (
                MakerActorKindV1::Zcash,
                ActorEffectCommand::Recover,
                effect("recover", "refunded", "refunded", 4, "complete"),
                "refunded",
            ),
            (
                MakerActorKindV1::Bitcoin,
                ActorEffectCommand::Drive,
                effect(
                    "drive",
                    "observed_then_projected",
                    "completed",
                    4,
                    "complete",
                ),
                "completed",
            ),
            (
                MakerActorKindV1::Bitcoin,
                ActorEffectCommand::Recover,
                effect(
                    "recover",
                    "converged_on_existing_projection",
                    "refunded",
                    4,
                    "complete",
                ),
                "refunded",
            ),
        ] {
            let parsed = parse_effect(&output, command, kind).unwrap();
            assert!(parsed.terminal);
            assert_eq!(
                parsed.progress,
                MakerActorProgressObservationV1::active(phase, 4, "complete").unwrap()
            );
        }
    }

    #[test]
    fn parser_accepts_exact_nonterminal_zec_claim_projection_only() {
        let exact = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "role": "maker",
            "command": "claim",
            "outcome": "projected",
            "operation": "lez_revealing_claim",
            "phase": "claim_evidence_available",
            "revision": 3,
            "next_action": "wait"
        }))
        .unwrap();
        let parsed =
            parse_effect(&exact, ActorEffectCommand::Claim, MakerActorKindV1::Zcash).unwrap();
        assert!(!parsed.terminal);

        let mut drive: Value = serde_json::from_slice(&exact).unwrap();
        drive["command"] = Value::from("drive");
        let parsed_drive = parse_effect(
            &serde_json::to_vec(&drive).unwrap(),
            ActorEffectCommand::Drive,
            MakerActorKindV1::Zcash,
        )
        .unwrap();
        assert!(!parsed_drive.terminal);

        for operation in [None, Some("lez_refund")] {
            let mut output: Value = serde_json::from_slice(&exact).unwrap();
            match operation {
                Some(operation) => output["operation"] = Value::from(operation),
                None => {
                    output.as_object_mut().unwrap().remove("operation");
                }
            }
            assert!(
                parse_effect(
                    &serde_json::to_vec(&output).unwrap(),
                    ActorEffectCommand::Claim,
                    MakerActorKindV1::Zcash,
                )
                .is_err()
            );
        }

        for (command, phase, next_action) in [
            (ActorEffectCommand::Claim, "completed", "complete"),
            (
                ActorEffectCommand::Claim,
                "claim_evidence_available",
                "claim_zcash",
            ),
            (ActorEffectCommand::Drive, "completed", "complete"),
            (
                ActorEffectCommand::Drive,
                "claim_evidence_available",
                "claim_zcash",
            ),
        ] {
            let mut output: Value = serde_json::from_slice(&exact).unwrap();
            output["command"] = Value::from(command.name());
            output["phase"] = Value::from(phase);
            output["next_action"] = Value::from(next_action);
            assert!(
                parse_effect(
                    &serde_json::to_vec(&output).unwrap(),
                    command,
                    MakerActorKindV1::Zcash,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn parser_accepts_exact_terminal_zec_followup_projection_only() {
        let exact = serde_json::json!({
            "schema_version": 1,
            "role": "maker",
            "outcome": "projected",
            "operation": "zcash_followup_claim",
            "phase": "completed",
            "revision": 4,
            "next_action": "complete"
        });

        for command in [ActorEffectCommand::Claim, ActorEffectCommand::Drive] {
            let mut output = exact.clone();
            output["command"] = Value::from(command.name());
            let parsed = parse_effect(
                &serde_json::to_vec(&output).unwrap(),
                command,
                MakerActorKindV1::Zcash,
            )
            .unwrap();
            assert!(parsed.terminal);
        }

        for (operation, phase, next_action) in [
            ("lez_revealing_claim", "completed", "complete"),
            ("zcash_followup_claim", "claim_evidence_available", "wait"),
            ("unknown_claim", "completed", "complete"),
        ] {
            for command in [ActorEffectCommand::Claim, ActorEffectCommand::Drive] {
                let mut output = exact.clone();
                output["command"] = Value::from(command.name());
                output["operation"] = Value::from(operation);
                output["phase"] = Value::from(phase);
                output["next_action"] = Value::from(next_action);
                assert!(
                    parse_effect(
                        &serde_json::to_vec(&output).unwrap(),
                        command,
                        MakerActorKindV1::Zcash,
                    )
                    .is_err()
                );
            }
        }

        for command in [ActorEffectCommand::Claim, ActorEffectCommand::Drive] {
            let mut output = exact.clone();
            output["command"] = Value::from(command.name());
            output.as_object_mut().unwrap().remove("operation");
            assert!(
                parse_effect(
                    &serde_json::to_vec(&output).unwrap(),
                    command,
                    MakerActorKindV1::Zcash,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn parser_rejects_cross_pair_unknown_and_incoherent_progress() {
        for (kind, bytes) in [
            (
                MakerActorKindV1::Zcash,
                status("both_legs_locked", 3, "observe_revealing_claim"),
            ),
            (
                MakerActorKindV1::Bitcoin,
                status("future_phase", 3, "observe_revealing_claim"),
            ),
            (MakerActorKindV1::Zcash, status("completed", 4, "claim_lez")),
            (
                MakerActorKindV1::Zcash,
                status("both_legs_locked", 3, "complete"),
            ),
        ] {
            assert!(parse_status(&bytes, kind).is_err());
        }

        for (command, output_command, outcome, phase, next_action) in [
            (
                ActorEffectCommand::Claim,
                "claim",
                "refunded",
                "refunded",
                "complete",
            ),
            (
                ActorEffectCommand::Recover,
                "recover",
                "completed",
                "completed",
                "complete",
            ),
            (
                ActorEffectCommand::Claim,
                "claim",
                "completed",
                "both_legs_locked",
                "claim_lez",
            ),
            (
                ActorEffectCommand::Claim,
                "drive",
                "completed",
                "completed",
                "complete",
            ),
        ] {
            assert!(
                parse_effect(
                    &effect(output_command, outcome, phase, 4, next_action),
                    command,
                    MakerActorKindV1::Zcash,
                )
                .is_err()
            );
        }

        for bytes in [
            effect("claim", "projected", "completed", 4, "complete"),
            zec_projected_claim_terminal_effect("lez_revealing_claim"),
        ] {
            assert!(
                parse_effect(&bytes, ActorEffectCommand::Claim, MakerActorKindV1::Zcash,).is_err()
            );
        }
    }
}
