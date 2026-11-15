//! Role-fixed one-attempt process composition for XMR application effects.

use std::process::Command;

use anyhow::{Context as _, Result, bail, ensure};
use lez_swap_store::{
    ActorHeldLock, SqliteXmrWorkflowJournal, XmrWorkflowDecision, XmrWorkflowReconciliationSource,
    XmrWorkflowStep,
};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use crate::{
    ActorRole, ValidatedXmrEffectExecutionV3, XmrEffectChildModeV1, XmrEffectToolV1,
    effect_child_plan::canonical_xmr_effect_child_plan_bytes,
};

const TOOL_PLAN_DOMAIN_V1: &[u8] = b"lez-xmr-effect-tool-plan-v1\0";
/// Maximum accepted typed observer-result JSON bytes.
pub const XMR_EFFECT_OBSERVER_RESULT_MAX_BYTES: usize = 1024;

/// State reported by one bounded, non-sending effect observer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum XmrEffectObserverStateV1 {
    /// The exact external effect is not yet durably provable.
    Pending,
    /// Canonical external evidence proves the exact effect finalized.
    Finalized,
}

/// Strict typed output from one role-fixed effect observer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrEffectObserverResultV1 {
    step: XmrWorkflowStep,
    state: XmrEffectObserverStateV1,
    effect_evidence_sha256: Option<[u8; 32]>,
}

impl XmrEffectObserverResultV1 {
    /// Exact workflow step selected by the parent route.
    pub const fn step(self) -> XmrWorkflowStep {
        self.step
    }

    /// Finalized or still-pending observation state.
    pub const fn state(self) -> XmrEffectObserverStateV1 {
        self.state
    }

    /// Canonical nonzero evidence digest, present only for Finalized.
    #[must_use]
    pub const fn effect_evidence_sha256(self) -> Option<[u8; 32]> {
        self.effect_evidence_sha256
    }
}

#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum XmrEffectObserverResultWireV1 {
    Pending {
        schema_version: u16,
        step: String,
    },
    Finalized {
        schema_version: u16,
        step: String,
        effect_evidence_sha256: String,
    },
}

/// Parses one strict bounded observer result for an expected parent-selected step.
///
/// Reconciliation source is deliberately absent from this child-controlled
/// format; the role-fixed route derives it from the workflow step.
///
/// # Errors
///
/// Rejects empty, oversized, malformed, unknown-field, wrong-schema,
/// wrong-step, state/evidence-shape, uppercase, zero, or invalid digests.
pub fn parse_xmr_effect_observer_result_v1(
    bytes: &[u8],
    expected_step: XmrWorkflowStep,
) -> Result<XmrEffectObserverResultV1> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= XMR_EFFECT_OBSERVER_RESULT_MAX_BYTES,
        "XMR effect observer result is empty or oversized"
    );
    let wire: XmrEffectObserverResultWireV1 =
        serde_json::from_slice(bytes).context("XMR effect observer result is malformed")?;
    let (schema_version, step, state, digest) = match wire {
        XmrEffectObserverResultWireV1::Pending {
            schema_version,
            step,
        } => (
            schema_version,
            step,
            XmrEffectObserverStateV1::Pending,
            None,
        ),
        XmrEffectObserverResultWireV1::Finalized {
            schema_version,
            step,
            effect_evidence_sha256,
        } => (
            schema_version,
            step,
            XmrEffectObserverStateV1::Finalized,
            Some(decode_observer_digest(&effect_evidence_sha256)?),
        ),
    };
    ensure!(schema_version == 1, "XMR effect observer schema changed");
    ensure!(
        step == expected_step.name(),
        "XMR effect observer selected a different step"
    );
    Ok(XmrEffectObserverResultV1 {
        step: expected_step,
        state,
        effect_evidence_sha256: digest,
    })
}

fn decode_observer_digest(value: &str) -> Result<[u8; 32]> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "XMR effect observer evidence digest is not canonical lowercase hex"
    );
    let mut digest = [0_u8; 32];
    hex::decode_to_slice(value, &mut digest).context("decode XMR effect observer evidence")?;
    ensure!(
        digest.iter().any(|byte| *byte != 0),
        "XMR effect observer evidence digest is zero"
    );
    Ok(digest)
}

/// One sealed, read-only observer command for a previously attempted effect.
#[must_use]
pub struct XmrPreparedEffectObservationV1 {
    command: Command,
    tool_plan_identity_sha256: [u8; 32],
    reconciliation_source: XmrWorkflowReconciliationSource,
}

impl std::fmt::Debug for XmrPreparedEffectObservationV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("XmrPreparedEffectObservationV1")
            .field(
                "tool_plan_identity_sha256",
                &hex::encode(self.tool_plan_identity_sha256),
            )
            .field("reconciliation_source", &self.reconciliation_source)
            .finish_non_exhaustive()
    }
}

impl XmrPreparedEffectObservationV1 {
    /// Consumes the preparation into its sealed command, sending-plan digest,
    /// and parent-derived reconciliation source.
    pub fn into_parts(self) -> (Command, [u8; 32], XmrWorkflowReconciliationSource) {
        (
            self.command,
            self.tool_plan_identity_sha256,
            self.reconciliation_source,
        )
    }
}

/// Result of preparing one role-fixed durable effect invocation.
#[must_use]
pub enum XmrPreparedEffectInvocationV1 {
    /// This caller owns the only invocation and must spawn/reap this command.
    InvokeOnce {
        /// Exact sealed executable, locks, runtime, and credentials.
        command: Command,
        /// Stable digest of the role-fixed authority/tool plan.
        tool_plan_identity_sha256: [u8; 32],
    },
    /// A prior process may have invoked the effect; classify without resending.
    ObserveOnly {
        /// Stable digest of the role-fixed authority/tool plan.
        tool_plan_identity_sha256: [u8; 32],
    },
    /// Exact external evidence already reconciled this effect.
    Complete {
        /// Stable digest of the role-fixed authority/tool plan.
        tool_plan_identity_sha256: [u8; 32],
    },
}

impl std::fmt::Debug for XmrPreparedEffectInvocationV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (state, digest) = match self {
            Self::InvokeOnce {
                tool_plan_identity_sha256,
                ..
            } => ("invoke_once", tool_plan_identity_sha256),
            Self::ObserveOnly {
                tool_plan_identity_sha256,
            } => ("observe_only", tool_plan_identity_sha256),
            Self::Complete {
                tool_plan_identity_sha256,
            } => ("complete", tool_plan_identity_sha256),
        };
        formatter
            .debug_struct("XmrPreparedEffectInvocationV1")
            .field("state", &state)
            .field("tool_plan_identity_sha256", &hex::encode(digest))
            .finish_non_exhaustive()
    }
}

impl ValidatedXmrEffectExecutionV3 {
    /// Composes a non-sending Tag14/Tag16 readiness process while leaving the
    /// workflow invocation authority unchanged.
    ///
    /// Returns `None` after the invocation CAS has already been consumed, so
    /// restart reconciliation never repeats preflight or attempts a send.
    ///
    /// # Errors
    ///
    /// Rejects every unsupported route, unsafe or changed program/input, crossed
    /// locks, foreign workflow identity, missing step, or corrupt state.
    pub fn prepare_effect_preflight(
        &self,
        step: XmrWorkflowStep,
        actor_lock: &ActorHeldLock,
        workflow_lock: &ActorHeldLock,
    ) -> Result<Option<Command>> {
        ensure!(
            (self.effect_authority().role() == ActorRole::Taker
                && (step == XmrWorkflowStep::RefundLezTag16
                    || (step == XmrWorkflowStep::AuthorizeLezTag14
                        && self.effect_authority().tag14_release().is_some())))
                || (self.effect_authority().role() == ActorRole::Maker
                    && self.effect_authority().schema_version() == 3
                    && step == XmrWorkflowStep::PunishLezTag17),
            "only semantic Taker Tag14 and Tag16 or Maker Tag17 routes support effect preflight"
        );
        let tool = select_tool(self.effect_authority(), step)?;
        let digest = tool_plan_identity(self, step, tool);
        let executable = tool
            .verify_program_at_use()
            .context("pin role-fixed XMR preflight tool")?;
        let identity = self.workflow_identity();
        actor_lock
            .validate_for_state(
                identity.swap_id(),
                self.effect_authority().adaptor_journal(),
            )
            .context("bind XMR preflight actor lock")?;
        workflow_lock
            .validate_for_state(
                identity.swap_id(),
                self.effect_authority().workflow_journal(),
            )
            .context("bind XMR preflight workflow lock")?;
        let command = if step == XmrWorkflowStep::AuthorizeLezTag14 {
            self.pin_tag14_release_inputs_at_use(XmrEffectChildModeV1::Preflight)
                .context("pin Tag14 release preflight inputs")?
                .into_command(executable, actor_lock, workflow_lock)
                .context("compose role-fixed Tag14 release preflight child")?
        } else {
            let child_plan = canonical_xmr_effect_child_plan_bytes(
                self.effect_authority(),
                XmrEffectChildModeV1::Preflight,
                step,
                tool.abi(),
                digest,
            )
            .context("compose XMR preflight child plan")?;
            self.effect_authority()
                .pin_effect_inputs_at_use()
                .context("pin role-fixed XMR preflight inputs")?
                .with_application_material(&self.application)
                .context("pin validated XMR preflight application inputs")?
                .with_child_plan(&child_plan)
                .context("pin XMR preflight child plan")?
                .with_invocation_material(
                    &self.application,
                    step,
                    self.effect_authority().evidence_root(),
                )
                .context("pin step-specific XMR preflight material")?
                .into_command(executable, actor_lock, workflow_lock)
                .context("compose role-fixed XMR preflight child")?
        };
        let workflow =
            SqliteXmrWorkflowJournal::open_existing(self.effect_authority().workflow_journal())
                .context("open XMR effect workflow for preflight")?;
        workflow
            .validate_initialized(identity)
            .context("bind XMR preflight workflow identity")?;
        if workflow
            .requires_invocation_preflight(identity, step)
            .context("validate XMR preflight eligibility")?
        {
            Ok(Some(command))
        } else {
            drop(command);
            Ok(None)
        }
    }

    /// Pins one role-fixed worker and all inputs before consuming its only
    /// durable invocation authority.
    ///
    /// # Errors
    ///
    /// Rejects wrong-role/unsupported steps, crossed locks, unsafe or changed
    /// programs and inputs, foreign workflow state, or a racing invalid CAS.
    pub fn prepare_effect_invocation(
        &self,
        step: XmrWorkflowStep,
        actor_lock: &ActorHeldLock,
        workflow_lock: &ActorHeldLock,
    ) -> Result<XmrPreparedEffectInvocationV1> {
        let (command, digest) = self.invocation_command(step, actor_lock, workflow_lock)?;
        let identity = self.workflow_identity();
        let mut workflow =
            SqliteXmrWorkflowJournal::open_existing(self.effect_authority().workflow_journal())
                .context("open XMR effect workflow")?;
        workflow
            .validate_initialized(identity)
            .context("bind XMR effect workflow identity")?;
        match workflow
            .authorize_once(identity, step)
            .context("consume XMR effect invocation authority")?
        {
            XmrWorkflowDecision::InvokeOnce => Ok(XmrPreparedEffectInvocationV1::InvokeOnce {
                command,
                tool_plan_identity_sha256: digest,
            }),
            XmrWorkflowDecision::ObserveOnly => {
                drop(command);
                Ok(XmrPreparedEffectInvocationV1::ObserveOnly {
                    tool_plan_identity_sha256: digest,
                })
            }
            XmrWorkflowDecision::Complete => {
                drop(command);
                Ok(XmrPreparedEffectInvocationV1::Complete {
                    tool_plan_identity_sha256: digest,
                })
            }
        }
    }

    /// Pins the sending tool of a Maker LEZ claim (tag 15), punishment
    /// (tag 17) or Monero refund sweep again, for a step whose one invocation
    /// is spent (Started or Unknown) while its tool's submission evidence is
    /// missing or reserved but empty: the tool was stopped, or failed, before
    /// it recorded what it sent. An empty reservation is removed, so the tool
    /// reserves it anew. The journal is not changed. A punishment is driven
    /// again also once its submission is recorded (that record is removed
    /// too): it may have lost its nonce to another transaction of the Maker's
    /// shared LEZ owner, which nothing else can repair after the punish time.
    ///
    /// The tool sends nothing new: the LEZ tools ask the swap's sidecar under
    /// their fixed request ids, which replays the one durable preparation and
    /// completion (it refuses any other) and the journaled outcome of a
    /// submission already asked for (an in-flight one is never sent again);
    /// a Monero sweep spends the one shared output, which the chain lets be
    /// spent once. The one exception is a punishment the sidecar re-planned
    /// because the owner's account passed its nonce without it: that one can
    /// never land, and the re-plan at the live nonce is sent instead.
    ///
    /// # Errors
    ///
    /// Rejects every other role or step, a Prepared or Succeeded step,
    /// written submission evidence of a claim or sweep, and every
    /// invocation-composition failure.
    pub fn prepare_effect_redrive(
        &self,
        step: XmrWorkflowStep,
        actor_lock: &ActorHeldLock,
        workflow_lock: &ActorHeldLock,
    ) -> Result<Command> {
        ensure!(
            self.effect_authority().role() == ActorRole::Maker,
            "only a Maker effect is driven again"
        );
        let evidence_root = self.effect_authority().evidence_root();
        let file = maker_submission_file(step)
            .context("XMR workflow step has no Maker submission evidence")?;
        ensure!(
            step == XmrWorkflowStep::PunishLezTag17
                || xmr_maker_submission_unwritten(evidence_root, step)?,
            "the XMR effect's submission evidence is written: it is only observed"
        );
        let (command, _) = self.invocation_command(step, actor_lock, workflow_lock)?;
        SqliteXmrWorkflowJournal::open_existing(self.effect_authority().workflow_journal())
            .context("open XMR effect workflow for a redrive")?
            .validate_observation_eligible(self.workflow_identity(), step)
            .context("validate XMR effect redrive eligibility")?;
        match std::fs::remove_file(evidence_root.join(file)) {
            Ok(()) => std::fs::File::open(evidence_root)
                .and_then(|directory| directory.sync_all())
                .context("sync the XMR effect evidence root")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("remove the XMR submission evidence"),
        }
        Ok(command)
    }

    /// Pins one role-fixed sending tool, its inputs and both held locks.
    fn invocation_command(
        &self,
        step: XmrWorkflowStep,
        actor_lock: &ActorHeldLock,
        workflow_lock: &ActorHeldLock,
    ) -> Result<(Command, [u8; 32])> {
        let tool = select_tool(self.effect_authority(), step)?;
        let digest = tool_plan_identity(self, step, tool);
        let executable = tool
            .verify_program_at_use()
            .context("pin role-fixed XMR effect tool")?;
        let identity = self.workflow_identity();
        actor_lock
            .validate_for_state(
                identity.swap_id(),
                self.effect_authority().adaptor_journal(),
            )
            .context("bind XMR actor lock")?;
        workflow_lock
            .validate_for_state(
                identity.swap_id(),
                self.effect_authority().workflow_journal(),
            )
            .context("bind XMR workflow lock")?;
        let command = if step == XmrWorkflowStep::AuthorizeLezTag14
            && self.effect_authority().tag14_release().is_some()
        {
            self.pin_tag14_release_inputs_at_use(XmrEffectChildModeV1::Invoke)
                .context("pin Tag14 release invocation inputs")?
                .into_command(executable, actor_lock, workflow_lock)
                .context("compose role-fixed Tag14 release child")?
        } else {
            let child_plan = canonical_xmr_effect_child_plan_bytes(
                self.effect_authority(),
                XmrEffectChildModeV1::Invoke,
                step,
                tool.abi(),
                digest,
            )
            .context("compose XMR sending child plan")?;
            self.effect_authority()
                .pin_effect_inputs_at_use()
                .context("pin role-fixed XMR effect inputs")?
                .with_application_material(&self.application)
                .context("pin validated XMR application inputs")?
                .with_child_plan(&child_plan)
                .context("pin XMR sending child plan")?
                .with_invocation_material(
                    &self.application,
                    step,
                    self.effect_authority().evidence_root(),
                )
                .context("pin step-specific XMR invocation material")?
                .into_command(executable, actor_lock, workflow_lock)
                .context("compose role-fixed XMR effect child")?
        };
        Ok((command, digest))
    }

    /// Pins one role-fixed non-sending observer for a Started or Unknown effect.
    ///
    /// The child ABI receives exactly two nonsecret argv values:
    /// `--xmr-workflow-step <stable-step-name>`. Runtime and all ten secrets
    /// remain confined to sealed FDs 200 through 210; the executable and both
    /// held locks remain FDs 197 through 199.
    ///
    /// All executable, input, and lock validation plus complete command
    /// composition happens before the read-only workflow eligibility check.
    /// The returned digest identifies the original sending tool, never the
    /// classifier/verifier executable.
    ///
    /// # Errors
    ///
    /// Rejects wrong-role/unsupported steps, unsafe or changed observers and
    /// inputs, crossed locks, foreign workflow state, Prepared, or Succeeded.
    pub fn prepare_effect_observation(
        &self,
        step: XmrWorkflowStep,
        actor_lock: &ActorHeldLock,
        workflow_lock: &ActorHeldLock,
    ) -> Result<XmrPreparedEffectObservationV1> {
        let sending_tool = select_tool(self.effect_authority(), step)?;
        let (observer, reconciliation_source) = select_observer(self.effect_authority(), step)?;
        let digest = tool_plan_identity(self, step, sending_tool);
        let child_plan = canonical_xmr_effect_child_plan_bytes(
            self.effect_authority(),
            XmrEffectChildModeV1::Observe,
            step,
            observer.abi(),
            digest,
        )
        .context("compose XMR observer child plan")?;
        let executable = observer
            .verify_program_at_use()
            .context("pin role-fixed XMR effect observer")?;
        let mut inputs = self
            .effect_authority()
            .pin_effect_inputs_at_use()
            .context("pin role-fixed XMR effect observer inputs")?
            .with_application_material(&self.application)
            .context("pin validated XMR observer application inputs")?
            .with_child_plan(&child_plan)
            .context("pin XMR observer child plan")?;
        if step == XmrWorkflowStep::AuthorizeLezTag14
            && self.effect_authority().tag14_release().is_some()
        {
            inputs = inputs
                .with_tag14_exact_transaction(self)
                .context("pin exact Tag14 observation transaction")?;
        }
        if step == XmrWorkflowStep::ClaimLezTag15 {
            inputs = inputs
                .with_tag15_exact_transaction(self)
                .context("pin exact Tag15 observation transaction")?;
        }
        let identity = self.workflow_identity();
        actor_lock
            .validate_for_state(
                identity.swap_id(),
                self.effect_authority().adaptor_journal(),
            )
            .context("bind XMR observer actor lock")?;
        workflow_lock
            .validate_for_state(
                identity.swap_id(),
                self.effect_authority().workflow_journal(),
            )
            .context("bind XMR observer workflow lock")?;
        let mut command = inputs
            .into_command(executable, actor_lock, workflow_lock)
            .context("compose role-fixed XMR effect observer child")?;
        command.arg("--xmr-workflow-step").arg(step.name());

        let workflow =
            SqliteXmrWorkflowJournal::open_existing(self.effect_authority().workflow_journal())
                .context("open XMR effect workflow for observation")?;
        workflow
            .validate_observation_eligible(identity, step)
            .context("validate XMR effect observation eligibility")?;
        Ok(XmrPreparedEffectObservationV1 {
            command,
            tool_plan_identity_sha256: digest,
            reconciliation_source,
        })
    }
}

/// The submission evidence the Maker's LEZ claim, punishment and Monero
/// refund sweep tools reserve before their first send and write after it.
const fn maker_submission_file(step: XmrWorkflowStep) -> Option<&'static str> {
    match step {
        XmrWorkflowStep::ClaimLezTag15 => Some("tag15-claim-submission.json"),
        XmrWorkflowStep::PunishLezTag17 => Some("tag17-punishment-submission.json"),
        XmrWorkflowStep::SweepMoneroRefund => Some("monero-refund-submission.json"),
        _ => None,
    }
}

/// Whether a Maker tool's submission evidence for `step` is missing or
/// reserved but empty: its tool stopped before it recorded what it sent,
/// if anything. Its write is one small write and a sync, so a stop leaves
/// it empty or complete.
///
/// # Errors
///
/// Rejects a step without Maker submission evidence, an evidence path that
/// is not a regular file, and an unreadable evidence root.
pub fn xmr_maker_submission_unwritten(
    evidence_root: &std::path::Path,
    step: XmrWorkflowStep,
) -> Result<bool> {
    let file = maker_submission_file(step)
        .context("XMR workflow step has no Maker submission evidence")?;
    match std::fs::symlink_metadata(evidence_root.join(file)) {
        Ok(metadata) => {
            ensure!(
                metadata.file_type().is_file(),
                "XMR submission evidence is not a regular file"
            );
            Ok(metadata.len() == 0)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error).context("inspect XMR submission evidence"),
    }
}

fn select_tool(
    authority: &crate::ValidatedXmrEffectAuthorityV1,
    step: XmrWorkflowStep,
) -> Result<&XmrEffectToolV1> {
    let tool = match (authority.role(), step) {
        (ActorRole::Maker, XmrWorkflowStep::FundMonero) => maker(authority)?.monero_fund(),
        (ActorRole::Maker, XmrWorkflowStep::ClaimLezTag15) => maker(authority)?.lez_claim(),
        (ActorRole::Maker, XmrWorkflowStep::SweepMoneroRefund) => maker(authority)?.monero_refund(),
        (ActorRole::Maker, XmrWorkflowStep::PunishLezTag17) => maker(authority)?
            .lez_punish()
            .context("Maker Tag17 effect tool is unavailable")?,
        (ActorRole::Taker, XmrWorkflowStep::AuthorizeLezTag14) => {
            taker(authority)?.tag14_authorize()
        }
        (ActorRole::Taker, XmrWorkflowStep::SweepMoneroClaim) => taker(authority)?.monero_claim(),
        (ActorRole::Taker, XmrWorkflowStep::RefundLezTag16) => taker(authority)?.tag16_refund(),
        (ActorRole::Maker | ActorRole::Taker, _) => {
            bail!("XMR workflow step has no role-fixed invocation slot")
        }
    };
    Ok(tool)
}

fn select_observer(
    authority: &crate::ValidatedXmrEffectAuthorityV1,
    step: XmrWorkflowStep,
) -> Result<(&XmrEffectToolV1, XmrWorkflowReconciliationSource)> {
    let observer = match (authority.role(), step) {
        (ActorRole::Maker, XmrWorkflowStep::ClaimLezTag15 | XmrWorkflowStep::PunishLezTag17) => (
            maker(authority)?.finalized_classifier(),
            XmrWorkflowReconciliationSource::LezFinalizedEvent,
        ),
        (
            ActorRole::Taker,
            XmrWorkflowStep::AuthorizeLezTag14 | XmrWorkflowStep::RefundLezTag16,
        ) => (
            taker(authority)?.finalized_classifier(),
            XmrWorkflowReconciliationSource::LezFinalizedEvent,
        ),
        (ActorRole::Maker, XmrWorkflowStep::FundMonero | XmrWorkflowStep::SweepMoneroRefund) => (
            maker(authority)?.monero_verify(),
            XmrWorkflowReconciliationSource::MoneroWalletTransaction,
        ),
        (ActorRole::Taker, XmrWorkflowStep::SweepMoneroClaim) => (
            taker(authority)?.monero_verify(),
            XmrWorkflowReconciliationSource::MoneroWalletTransaction,
        ),
        (ActorRole::Maker | ActorRole::Taker, _) => {
            bail!("XMR workflow step has no role-fixed observation slot")
        }
    };
    Ok(observer)
}

fn maker(
    authority: &crate::ValidatedXmrEffectAuthorityV1,
) -> Result<&crate::XmrMakerEffectToolsV1> {
    authority
        .maker_tools()
        .context("Maker XMR effect profile is unavailable")
}

fn taker(
    authority: &crate::ValidatedXmrEffectAuthorityV1,
) -> Result<&crate::XmrTakerEffectToolsV1> {
    authority
        .taker_tools()
        .context("Taker XMR effect profile is unavailable")
}

fn tool_plan_identity(
    execution: &ValidatedXmrEffectExecutionV3,
    step: XmrWorkflowStep,
    tool: &XmrEffectToolV1,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(TOOL_PLAN_DOMAIN_V1);
    digest.update(match execution.effect_authority().role() {
        ActorRole::Maker => b"maker".as_slice(),
        ActorRole::Taker => b"taker".as_slice(),
    });
    digest.update([0]);
    digest.update(step.name().as_bytes());
    digest.update([0]);
    digest.update(tool.abi().as_bytes());
    digest.update([0]);
    digest.update(tool.program_sha256());
    digest.update(execution.effect_authority_sha256());
    digest.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_submission_missing_or_reserved_empty_is_unwritten() {
        let root = tempfile::tempdir().unwrap();
        let step = XmrWorkflowStep::ClaimLezTag15;
        let file = root.path().join("tag15-claim-submission.json");
        assert!(xmr_maker_submission_unwritten(root.path(), step).unwrap());
        std::fs::write(&file, b"").unwrap();
        assert!(xmr_maker_submission_unwritten(root.path(), step).unwrap());
        std::fs::write(&file, b"{}\n").unwrap();
        assert!(!xmr_maker_submission_unwritten(root.path(), step).unwrap());
        std::fs::create_dir(root.path().join("monero-refund-submission.json")).unwrap();
        assert!(
            xmr_maker_submission_unwritten(root.path(), XmrWorkflowStep::SweepMoneroRefund)
                .is_err()
        );
        assert!(xmr_maker_submission_unwritten(root.path(), XmrWorkflowStep::FundMonero).is_err());
    }
}
