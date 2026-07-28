//! Bounded execution of one durable maker-actor scheduling attempt.

use std::{
    fs,
    io::{self, Read},
    os::unix::process::CommandExt as _,
    process::{Child, ExitStatus, Stdio},
    thread,
    time::Duration,
};

use lez_swap_core::SwapId;
use lez_swap_store::{
    MakerActorAttemptResolution, MakerActorHeldLock, MakerActorKindV1, MakerActorLeaseOwner,
    MakerActorLeaseV1, MakerActorProcessError, SqliteSwapStore,
};
use rustix::process::{Pid, Signal, kill_process_group};
use serde_json::Value;
use thiserror::Error;
use wait_timeout::ChildExt as _;

use super::prepare_maker_actor;

const MAX_ATTEMPT_TIMEOUT: Duration = Duration::from_mins(5);
const MIN_OUTPUT_BYTES: usize = 256;
const MAX_OUTPUT_BYTES: usize = 64 * 1_024;

/// Immutable bounds for one supervisor scheduling cycle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerActorSupervisorConfig {
    attempt_timeout: Duration,
    requeue_delay_seconds: u64,
    failure_backoff_seconds: u64,
    max_output_bytes: usize,
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
            attempt_timeout,
            requeue_delay_seconds,
            failure_backoff_seconds,
            max_output_bytes,
        })
    }
}

/// Durable classification produced by one bounded scheduling cycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MakerActorSupervisorResolution {
    /// Actor remains live and is queued for its next bounded observation/effect.
    Requeued,
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
    let Some(swap_id) = store.list_due_maker_actor_ids(now, 1)?.into_iter().next() else {
        return Ok(None);
    };
    let Some(lease) = store.claim_maker_actor(&swap_id, owner, now)? else {
        return Ok(None);
    };
    let generation = lease.generation();
    let claimed = match run_claimed_attempt(store, &lease, config) {
        Ok(value) => value,
        Err(ClaimedAttemptError::Scheduling(error)) => return Err(error.into()),
    };
    let attempt = claimed.attempt;
    let (durable, resolution) = match attempt {
        ClaimedAttempt::Requeue => (
            MakerActorAttemptResolution::Requeue {
                not_before: now.saturating_add(config.requeue_delay_seconds),
            },
            MakerActorSupervisorResolution::Requeued,
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
        ClaimedAttempt::Failed(failure_class) => (
            MakerActorAttemptResolution::Failed {
                failure_class: failure_class.into(),
            },
            MakerActorSupervisorResolution::Failed,
        ),
    };
    store.resolve_maker_actor_attempt(&lease, durable, now)?;
    drop(claimed.held_lock);
    Ok(Some(MakerActorSupervisorOutcome {
        swap_id,
        generation,
        resolution,
    }))
}

enum ClaimedAttempt {
    Requeue,
    Backoff(&'static str),
    Terminal,
    Failed(&'static str),
}

struct ClaimedAttemptResult {
    attempt: ClaimedAttempt,
    held_lock: Option<MakerActorHeldLock>,
}

enum ClaimedAttemptError {
    Scheduling(MakerActorProcessError),
}

#[derive(Clone, Copy)]
enum ActorEffectCommand {
    Activate,
    Drive,
    Recover,
}

impl ActorEffectCommand {
    const fn name(self) -> &'static str {
        match self {
            Self::Activate => "activate",
            Self::Drive => "drive",
            Self::Recover => "recover",
        }
    }
}

fn run_claimed_attempt(
    store: &mut SqliteSwapStore,
    lease: &MakerActorLeaseV1,
    config: &MakerActorSupervisorConfig,
) -> Result<ClaimedAttemptResult, ClaimedAttemptError> {
    let held_lock = match MakerActorHeldLock::acquire(lease.record()) {
        Ok(lock) => lock,
        Err(MakerActorProcessError::LockUnavailable) => {
            return Ok(ClaimedAttemptResult {
                attempt: ClaimedAttempt::Backoff("actor_lock_unavailable"),
                held_lock: None,
            });
        }
        Err(_) => {
            return Ok(ClaimedAttemptResult {
                attempt: ClaimedAttempt::Failed("actor_lock_invalid"),
                held_lock: None,
            });
        }
    };
    let attempt = (|| {
        let status = match run_child(store, lease, &held_lock, "status", config) {
            Ok(output) => output,
            Err(ChildRunError::Retry(class)) => return Ok(ClaimedAttempt::Backoff(class)),
            Err(ChildRunError::Fail(class)) => return Ok(ClaimedAttempt::Failed(class)),
            Err(ChildRunError::Scheduling(error)) => {
                return Err(ClaimedAttemptError::Scheduling(error));
            }
        };
        let command = match parse_status(&status, lease.record().manifest().kind()) {
            Ok(StatusDecision::Terminal) => return Ok(ClaimedAttempt::Terminal),
            Ok(StatusDecision::Run(command)) => command,
            Err(()) => return Ok(ClaimedAttempt::Failed("actor_output_invalid")),
        };
        let effect = match run_child(store, lease, &held_lock, command.name(), config) {
            Ok(output) => output,
            Err(ChildRunError::Retry(class)) => return Ok(ClaimedAttempt::Backoff(class)),
            Err(ChildRunError::Fail(class)) => return Ok(ClaimedAttempt::Failed(class)),
            Err(ChildRunError::Scheduling(error)) => {
                return Err(ClaimedAttemptError::Scheduling(error));
            }
        };
        match parse_effect(&effect, command, lease.record().manifest().kind()) {
            Ok(true) => Ok(ClaimedAttempt::Terminal),
            Ok(false) => Ok(ClaimedAttempt::Requeue),
            Err(()) => Ok(ClaimedAttempt::Failed("actor_output_invalid")),
        }
    })()?;
    Ok(ClaimedAttemptResult {
        attempt,
        held_lock: Some(held_lock),
    })
}

enum ChildRunError {
    Retry(&'static str),
    Fail(&'static str),
    Scheduling(MakerActorProcessError),
}

fn run_child(
    store: &mut SqliteSwapStore,
    lease: &MakerActorLeaseV1,
    held_lock: &MakerActorHeldLock,
    actor_command: &'static str,
    config: &MakerActorSupervisorConfig,
) -> Result<Vec<u8>, ChildRunError> {
    let artifacts =
        prepare_maker_actor(lease.record()).map_err(|error| classify_deployment(&error))?;
    let mut command = artifacts
        .into_command(held_lock)
        .map_err(|error| classify_deployment(&error))?;
    command
        .args(["--config-fd", "196", actor_command])
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
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
    let status = match child.wait_timeout(config.attempt_timeout) {
        Ok(Some(status)) => Ok(status),
        Ok(None) => {
            kill_and_reap(&mut child);
            Err(ChildRunError::Retry("actor_timeout"))
        }
        Err(_) => {
            kill_and_reap(&mut child);
            Err(ChildRunError::Retry("actor_wait_failed"))
        }
    };
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

fn kill_and_reap(child: &mut Child) {
    if let Some(pid) = Pid::from_raw(child.id().cast_signed()) {
        let _ = kill_process_group(pid, Signal::KILL);
    }
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

enum StatusDecision {
    Run(ActorEffectCommand),
    Terminal,
}

fn parse_status(bytes: &[u8], kind: MakerActorKindV1) -> Result<StatusDecision, ()> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    if value.get("schema_version").and_then(Value::as_u64) != Some(1)
        || value.get("role").and_then(Value::as_str) != Some("maker")
    {
        return Err(());
    }
    match value.get("state").and_then(Value::as_str) {
        Some("not_activated") => Ok(StatusDecision::Run(ActorEffectCommand::Activate)),
        Some("active") => {
            let phase = value.get("phase").and_then(Value::as_str).ok_or(())?;
            if terminal_phase(phase) {
                Ok(StatusDecision::Terminal)
            } else if kind == MakerActorKindV1::Bitcoin
                && value.get("next_action").and_then(Value::as_str) == Some("recover_taker_leg")
            {
                Ok(StatusDecision::Run(ActorEffectCommand::Recover))
            } else {
                Ok(StatusDecision::Run(ActorEffectCommand::Drive))
            }
        }
        _ => Err(()),
    }
}

fn parse_effect(
    bytes: &[u8],
    command: ActorEffectCommand,
    kind: MakerActorKindV1,
) -> Result<bool, ()> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    let outcome = value.get("outcome").and_then(Value::as_str).ok_or(())?;
    if value.get("schema_version").and_then(Value::as_u64) != Some(1)
        || value.get("role").and_then(Value::as_str) != Some("maker")
        || value.get("command").and_then(Value::as_str) != Some(command.name())
        || !known_effect_outcome(kind, outcome)
        || value.get("revision").and_then(Value::as_u64).is_none()
    {
        return Err(());
    }
    value
        .get("phase")
        .and_then(Value::as_str)
        .map(terminal_phase)
        .ok_or(())
}

fn known_effect_outcome(kind: MakerActorKindV1, outcome: &str) -> bool {
    match kind {
        MakerActorKindV1::Bitcoin => matches!(
            outcome,
            "activated"
                | "awaiting_observation"
                | "observed_then_projected"
                | "converged_on_existing_projection"
                | "not_yet_composed"
        ),
        MakerActorKindV1::Zcash => matches!(
            outcome,
            "activated"
                | "submitted"
                | "awaiting_observation"
                | "awaiting_safe_zcash_funding"
                | "unchanged"
                | "projected"
                | "completed"
        ),
    }
}

fn terminal_phase(phase: &str) -> bool {
    matches!(phase, "completed" | "refunded")
}
