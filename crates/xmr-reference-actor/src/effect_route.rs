//! Role-fixed one-attempt process composition for XMR application effects.

use std::process::Command;

use anyhow::{Context as _, Result, bail};
use lez_swap_store::{
    MakerActorHeldLock, SqliteXmrWorkflowJournal, XmrWorkflowDecision, XmrWorkflowStep,
};
use sha2::{Digest as _, Sha256};

use crate::{ActorRole, ValidatedXmrEffectExecutionV3, XmrEffectToolV1};

const TOOL_PLAN_DOMAIN_V1: &[u8] = b"lez-xmr-effect-tool-plan-v1\0";

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
        actor_lock: &MakerActorHeldLock,
        workflow_lock: &MakerActorHeldLock,
    ) -> Result<XmrPreparedEffectInvocationV1> {
        let tool = select_tool(self.effect_authority(), step)?;
        let digest = tool_plan_identity(self, step, tool);
        let executable = tool
            .verify_program_at_use()
            .context("pin role-fixed XMR effect tool")?;
        let inputs = self
            .effect_authority()
            .pin_effect_inputs_at_use()
            .context("pin role-fixed XMR effect inputs")?;
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
        let command = inputs
            .into_command(executable, actor_lock, workflow_lock)
            .context("compose role-fixed XMR effect child")?;

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
}

fn select_tool(
    authority: &crate::ValidatedXmrEffectAuthorityV1,
    step: XmrWorkflowStep,
) -> Result<&XmrEffectToolV1> {
    let tool = match (authority.role(), step) {
        (ActorRole::Maker, XmrWorkflowStep::FundMonero) => maker(authority)?.monero_fund(),
        (ActorRole::Maker, XmrWorkflowStep::ClaimLezTag15) => maker(authority)?.lez_claim(),
        (ActorRole::Maker, XmrWorkflowStep::SweepMoneroRefund) => maker(authority)?.monero_refund(),
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
    digest.update(step_name(step).as_bytes());
    digest.update([0]);
    digest.update(tool.abi().as_bytes());
    digest.update([0]);
    digest.update(tool.program_sha256());
    digest.update(execution.effect_authority_sha256());
    digest.finalize().into()
}

const fn step_name(step: XmrWorkflowStep) -> &'static str {
    match step {
        XmrWorkflowStep::InitializeLezTag13 => "initialize_lez_tag13",
        XmrWorkflowStep::FundLezTag13 => "fund_lez_tag13",
        XmrWorkflowStep::FundMonero => "fund_monero",
        XmrWorkflowStep::AuthorizeLezTag14 => "authorize_lez_tag14",
        XmrWorkflowStep::ClaimLezTag15 => "claim_lez_tag15",
        XmrWorkflowStep::SweepMoneroClaim => "sweep_monero_claim",
        XmrWorkflowStep::RefundLezTag16 => "refund_lez_tag16",
        XmrWorkflowStep::SweepMoneroRefund => "sweep_monero_refund",
    }
}
