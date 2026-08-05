//! One-shot supervised Maker entrypoint for validated XMR application effects.

use std::{
    fs::File,
    io::{Read as _, stdin},
    os::fd::AsFd as _,
    process::{Command as ProcessCommand, Stdio},
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow, ensure};
use clap::{Parser as _, Subcommand};
use lez_swap_store::{
    MAKER_ACTOR_CONFIG_FD, MakerActorHeldLock, SqliteXmrWorkflowJournal, XmrWorkflowBranch,
    XmrWorkflowReconciliationV2, XmrWorkflowStep,
};
use serde::Serialize;
use wait_timeout::ChildExt as _;
use xmr_reference_actor::{
    ValidatedXmrEffectExecutionV3, XMR_EFFECT_OBSERVER_RESULT_MAX_BYTES, XMR_MAKER_ACTOR_ABI_V1,
    XMR_MAKER_ACTOR_NEXT_ACTION, XMR_MAKER_ACTOR_PROGRAM_ID, XmrEffectObserverStateV1,
    XmrPreparedEffectInvocationV1, load_validated_xmr_maker_authority_fd,
    load_validated_xmr_maker_effect_execution_fd, parse_xmr_effect_observer_result_v1,
};

const EFFECT_TIMEOUT: Duration = Duration::from_secs(30);
#[derive(Debug, clap::Parser)]
#[command(about = "One-shot supervised LEZ/XMR Maker actor")]
struct Arguments {
    #[arg(long, value_name = "FD", value_parser = parse_config_fd)]
    config_fd: i32,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, Debug, Subcommand)]
enum Command {
    /// Validate the complete immutable authority and report its current state.
    Status,
    /// Execute or reconcile the role-fixed Maker Tag17 recovery route.
    Recover,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ActorOutput {
    Status(StatusOutput),
    Recover(RecoverOutput),
}

#[derive(Serialize)]
struct StatusOutput {
    schema_version: u16,
    actor_program: &'static str,
    actor_abi: &'static str,
    role: &'static str,
    state: &'static str,
    phase: &'static str,
    revision: u64,
    next_action: &'static str,
    chain_effect_executed: bool,
}

#[derive(Serialize)]
struct RecoverOutput {
    schema_version: u16,
    role: &'static str,
    command: &'static str,
    outcome: &'static str,
    phase: &'static str,
    revision: u64,
    next_action: &'static str,
}

fn main() {
    let arguments = Arguments::parse();
    let result = match arguments.command {
        Command::Status => status(arguments.config_fd).map(ActorOutput::Status),
        Command::Recover => recover(arguments.config_fd).map(ActorOutput::Recover),
    };
    match result {
        Ok(output) => match serde_json::to_string(&output) {
            Ok(json) => println!("{json}"),
            Err(_) => exit_with("XMR Maker actor output is unavailable"),
        },
        Err(_) => exit_with("XMR Maker actor authority or effect is unavailable"),
    }
}

fn status(config_fd: i32) -> Result<StatusOutput> {
    if let Ok(execution) = load_validated_xmr_maker_effect_execution_fd(config_fd) {
        let _ = (
            execution.workflow_identity(),
            execution.effect_authority(),
            execution.effect_authority_sha256(),
        );
    } else {
        let authority = load_validated_xmr_maker_authority_fd(config_fd)?;
        let _ = (
            authority.swap_id(),
            authority.agreement_commitment(),
            authority.activation_commitment(),
            authority.state_database(),
        );
    }
    Ok(StatusOutput {
        schema_version: 1,
        actor_program: XMR_MAKER_ACTOR_PROGRAM_ID,
        actor_abi: XMR_MAKER_ACTOR_ABI_V1,
        role: "maker",
        state: "active",
        phase: "offered",
        revision: 0,
        next_action: XMR_MAKER_ACTOR_NEXT_ACTION,
        chain_effect_executed: false,
    })
}

fn recover(config_fd: i32) -> Result<RecoverOutput> {
    let execution = load_validated_xmr_maker_effect_execution_fd(config_fd)
        .context("validate XMR Maker effect execution")?;
    let identity = execution.workflow_identity();
    let authority = execution.effect_authority();
    let input = stdin();
    let transferred = input
        .as_fd()
        .try_clone_to_owned()
        .context("clone transferred XMR Maker actor lock")?;
    let actor_lock = MakerActorHeldLock::accept_transferred_for(
        identity.swap_id(),
        authority.adaptor_journal(),
        File::from(transferred),
    )
    .context("accept transferred XMR Maker actor lock")?;
    let workflow_lock =
        MakerActorHeldLock::acquire_for(identity.swap_id(), authority.workflow_journal())
            .context("acquire XMR Maker workflow lock")?;

    let recovery_step = selected_recovery_step(&execution)?;

    if recovery_requires_preflight(recovery_step) {
        execute_preflight(&execution, recovery_step, &actor_lock, &workflow_lock)?;
    }
    let prepared = execution
        .prepare_effect_invocation(recovery_step, &actor_lock, &workflow_lock)
        .context("prepare XMR Maker recovery")?;
    let finalized = match prepared {
        XmrPreparedEffectInvocationV1::InvokeOnce {
            mut command,
            tool_plan_identity_sha256: _,
        } => {
            if run_silent(&mut command).is_err() {
                mark_unknown(&execution, recovery_step)?;
                return Err(anyhow!("XMR Maker recovery invocation is ambiguous"));
            }
            false
        }
        XmrPreparedEffectInvocationV1::ObserveOnly {
            tool_plan_identity_sha256,
        } => observe_and_reconcile(
            &execution,
            recovery_step,
            tool_plan_identity_sha256,
            &actor_lock,
            &workflow_lock,
        )?,
        XmrPreparedEffectInvocationV1::Complete {
            tool_plan_identity_sha256: _,
        } => true,
    };
    let revision = workflow_revision(&execution, recovery_step)?;
    Ok(if finalized {
        RecoverOutput {
            schema_version: 1,
            role: "maker",
            command: "recover",
            outcome: "refunded",
            phase: "refunded",
            revision,
            next_action: "complete",
        }
    } else {
        RecoverOutput {
            schema_version: 1,
            role: "maker",
            command: "recover",
            outcome: "awaiting_observation",
            phase: "maker_recovery_available",
            revision,
            next_action: XMR_MAKER_ACTOR_NEXT_ACTION,
        }
    })
}

fn execute_preflight(
    execution: &ValidatedXmrEffectExecutionV3,
    recovery_step: XmrWorkflowStep,
    actor_lock: &MakerActorHeldLock,
    workflow_lock: &MakerActorHeldLock,
) -> Result<()> {
    let Some(mut command) = execution
        .prepare_effect_preflight(recovery_step, actor_lock, workflow_lock)
        .context("prepare XMR Maker recovery preflight")?
    else {
        return Ok(());
    };
    run_silent(&mut command).context("XMR Maker recovery preflight failed")
}

fn run_silent(command: &mut ProcessCommand) -> Result<()> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().context("spawn XMR Maker effect child")?;
    let status = match child.wait_timeout(EFFECT_TIMEOUT) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!("XMR Maker effect child timed out"));
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error).context("wait for XMR Maker effect child");
        }
    };
    ensure!(status.success(), "XMR Maker effect child failed");
    Ok(())
}

fn observe_and_reconcile(
    execution: &ValidatedXmrEffectExecutionV3,
    recovery_step: XmrWorkflowStep,
    expected_plan: [u8; 32],
    actor_lock: &MakerActorHeldLock,
    workflow_lock: &MakerActorHeldLock,
) -> Result<bool> {
    let prepared = execution
        .prepare_effect_observation(recovery_step, actor_lock, workflow_lock)
        .context("prepare XMR Maker recovery observation")?;
    let (mut command, plan, source) = prepared.into_parts();
    ensure!(plan == expected_plan, "XMR Maker recovery plan changed");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().context("spawn XMR Maker observer")?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(anyhow!("XMR Maker observer output is unavailable"));
    };
    let status = match child.wait_timeout(EFFECT_TIMEOUT) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!("XMR Maker observer timed out"));
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error).context("wait for XMR Maker observer");
        }
    };
    ensure!(status.success(), "XMR Maker observer failed");
    let mut bytes = Vec::new();
    stdout
        .by_ref()
        .take((XMR_EFFECT_OBSERVER_RESULT_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("read XMR Maker observer output")?;
    ensure!(
        bytes.len() <= XMR_EFFECT_OBSERVER_RESULT_MAX_BYTES,
        "XMR Maker observer output is oversized"
    );
    let result = parse_xmr_effect_observer_result_v1(&bytes, recovery_step)
        .context("parse XMR Maker observer output")?;
    match result.state() {
        XmrEffectObserverStateV1::Pending => Ok(false),
        XmrEffectObserverStateV1::Finalized => {
            let evidence = result
                .effect_evidence_sha256()
                .context("finalized XMR Maker recovery lacks evidence")?;
            let reconciliation = XmrWorkflowReconciliationV2::new(evidence, plan, source)
                .context("validate XMR Maker recovery evidence")?;
            let mut workflow = SqliteXmrWorkflowJournal::open_existing(
                execution.effect_authority().workflow_journal(),
            )
            .context("open XMR Maker recovery workflow")?;
            workflow
                .validate_initialized(execution.workflow_identity())
                .context("validate XMR Maker recovery workflow")?;
            workflow
                .reconcile_succeeded(
                    execution.workflow_identity(),
                    recovery_step,
                    &reconciliation,
                )
                .context("reconcile XMR Maker recovery")?;
            Ok(true)
        }
    }
}

fn workflow_revision(
    execution: &ValidatedXmrEffectExecutionV3,
    recovery_step: XmrWorkflowStep,
) -> Result<u64> {
    let workflow =
        SqliteXmrWorkflowJournal::open_existing(execution.effect_authority().workflow_journal())
            .context("open XMR Maker recovery workflow revision")?;
    workflow
        .validate_initialized(execution.workflow_identity())
        .context("validate XMR Maker recovery workflow revision")?;
    workflow
        .step_revision(execution.workflow_identity(), recovery_step)
        .context("load XMR Maker recovery workflow revision")
}

fn mark_unknown(
    execution: &ValidatedXmrEffectExecutionV3,
    recovery_step: XmrWorkflowStep,
) -> Result<()> {
    let mut workflow =
        SqliteXmrWorkflowJournal::open_existing(execution.effect_authority().workflow_journal())
            .context("open ambiguous XMR Maker recovery workflow")?;
    workflow
        .validate_initialized(execution.workflow_identity())
        .context("validate ambiguous XMR Maker recovery workflow")?;
    workflow
        .mark_unknown(execution.workflow_identity(), recovery_step)
        .context("mark XMR Maker recovery ambiguous")
}

fn selected_recovery_step(execution: &ValidatedXmrEffectExecutionV3) -> Result<XmrWorkflowStep> {
    let workflow =
        SqliteXmrWorkflowJournal::open_existing(execution.effect_authority().workflow_journal())
            .context("open XMR Maker recovery workflow branch")?;
    workflow
        .validate_initialized(execution.workflow_identity())
        .context("validate XMR Maker recovery workflow branch")?;
    let branch = workflow
        .selected_branch(execution.workflow_identity())
        .context("load XMR Maker recovery branch")?
        .context("XMR Maker recovery branch is not selected")?;
    recovery_step_for_branch(branch)
}

fn recovery_requires_preflight(step: XmrWorkflowStep) -> bool {
    step == XmrWorkflowStep::PunishLezTag17
}

fn recovery_step_for_branch(branch: XmrWorkflowBranch) -> Result<XmrWorkflowStep> {
    match branch {
        XmrWorkflowBranch::Refund => Ok(XmrWorkflowStep::SweepMoneroRefund),
        XmrWorkflowBranch::Punish => Ok(XmrWorkflowStep::PunishLezTag17),
        XmrWorkflowBranch::Claim => Err(anyhow!("claim branch has no Maker recovery effect")),
    }
}

fn parse_config_fd(value: &str) -> Result<i32, String> {
    let fd = value
        .parse::<i32>()
        .map_err(|_| "invalid config descriptor".to_owned())?;
    if fd == MAKER_ACTOR_CONFIG_FD {
        Ok(fd)
    } else {
        Err(format!("config descriptor must be {MAKER_ACTOR_CONFIG_FD}"))
    }
}

fn exit_with(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maker_recovery_step_is_derived_only_from_the_durable_branch() {
        assert_eq!(
            recovery_step_for_branch(XmrWorkflowBranch::Refund).unwrap(),
            XmrWorkflowStep::SweepMoneroRefund
        );
        assert_eq!(
            recovery_step_for_branch(XmrWorkflowBranch::Punish).unwrap(),
            XmrWorkflowStep::PunishLezTag17
        );
        assert!(recovery_step_for_branch(XmrWorkflowBranch::Claim).is_err());
        assert!(recovery_requires_preflight(XmrWorkflowStep::PunishLezTag17));
        assert!(!recovery_requires_preflight(
            XmrWorkflowStep::SweepMoneroRefund
        ));
    }
}
