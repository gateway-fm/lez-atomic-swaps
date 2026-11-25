//! One-shot supervised Maker entrypoint for validated XMR application effects.
//!
//! `status` reads the swap's durable state only: the workflow journal, the
//! finalized LEZ evidence `drive` recorded in the evidence root, and the
//! clock against the Stage-A windows. `drive` does one step: it reads the
//! finalized LEZ chain through the swap's sidecar and records what it saw,
//! funds Monero once (the `monero_fund` tool) or observes that funding, or
//! activates the branch the finalized evidence proves (claim, refund,
//! punish). `claim` and `recover` run the selected branch's effect tool.
//!
//! The Maker funds Monero only once the Taker's LEZ lock (tag 13) is final
//! within the funding cutoff `C`, and only while the finalized LEZ clock is
//! still before `C`; it claims (tag 15) only on a finalized Taker
//! authorization (tag 14) seen while the finalized clock is well before the
//! refund time `R`; it sweeps its Monero back only on the Taker's finalized
//! refund (tag 16); it punishes (tag 17) only once the finalized clock is
//! past the punish time `P` and the finalized history to that tip holds no
//! refund. Every effect consumes its one invocation in the workflow journal
//! before it runs, so a crash resumes with observation, never a new send: a
//! claim, punishment or refund sweep whose tool stopped before recording its
//! submission is driven again: a LEZ tool under its fixed sidecar request
//! ids, which replay what was prepared and asked for, a sweep against the
//! one shared output the chain lets be spent once. A claim not final by `R`
//! gives way to the Taker's finalized refund or, past `P`, to the punishment.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _, stdin},
    os::{fd::AsFd as _, unix::fs::OpenOptionsExt as _},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, anyhow, ensure};
use clap::{Parser as _, Subcommand};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, SidecarCapability};
use lez_bridge_protocol::{
    ClassifyFinalizedNativeXmrEffectV3Request, ClassifyFinalizedNativeXmrEffectV3Result,
    DiscoveryWindow, FinalizedNativeXmrScanOutcomeV3, FinalizedNativeXmrTransactionTargetV3,
    MessageContext, ObserveFinalizedClockRequest, Participant, RequestId, RunId, RuntimeDescriptor,
    XmrNativeEffectV3, XmrNativeEscrowTermsV3,
};
use lez_swap_store::{
    ActorHeldLock, MAKER_ACTOR_CONFIG_FD, SqliteXmrWorkflowJournal, StoreError, XmrWorkflowBranch,
    XmrWorkflowReconciliationV2, XmrWorkflowStep,
};
use lez_xmr_swap_sdk::XmrAgreementV1;
use serde::{Deserialize, Serialize};
use wait_timeout::ChildExt as _;
use xmr_reference_actor::{
    ValidatedXmrEffectExecutionV3, XMR_EFFECT_OBSERVER_RESULT_MAX_BYTES, XMR_MAKER_ACTOR_ABI_V1,
    XMR_MAKER_ACTOR_NEXT_ACTION, XMR_MAKER_ACTOR_PROGRAM_ID, XMR_MAKER_FUNDING_EVIDENCE_FILE,
    XMR_MAKER_FUNDING_RECEIPT_FILE, XMR_MAKER_LEZ_SCAN_FILE, XMR_MONERO_FUNDING_NOT_SENT_EXIT,
    XmrEffectObserverStateV1, XmrMakerEffectSourcesV1, XmrPreparedEffectInvocationV1,
    activate_maker_claim_workflow_bytes, activate_maker_punish_workflow_bytes,
    activate_maker_refund_workflow_bytes, agreement_monero_network, complete_maker_claim_signature,
    ingest_maker_refund_signature, load_validated_xmr_maker_authority_fd,
    load_validated_xmr_maker_effect_execution_fd, parse_xmr_effect_observer_result_v1,
    reconcile_maker_monero_funding_bytes, xmr_maker_submission_unwritten,
};
#[cfg(feature = "test-crash-hooks")]
use zec_reference_actor::arm_test_crash_hook;
use zeroize::Zeroizing;

const EFFECT_TIMEOUT: Duration = Duration::from_secs(30);
/// A Monero effect mines its confirmations and restores a wallet.
const MONERO_EFFECT_TIMEOUT: Duration = Duration::from_mins(2);
/// The same on a public network: no mining, but a wallet restored or
/// refreshed through a public node pulls Stagenet's block hashes to its
/// restore height and the blocks after it (minutes on 2026-11-21), and the
/// node may stop answering this host for a while. Below the Maker Node's
/// attempt deadline on Stagenet (node-entrypoint.sh: 5 min, the most the
/// supervisor admits), which also covers the drive's other steps.
const PUBLIC_MONERO_EFFECT_TIMEOUT: Duration = Duration::from_mins(4);
const LEZ_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the funding's monerod probe waits to connect and for an answer.
const MONEROD_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// Finalized blocks one classification request scans, and pages per drive:
/// a page takes the swap's sidecar 0.5 to 3 s, and the Node's one actor
/// worker drives every swap of every pair in turn, so a drive scans at most
/// 128 blocks (a few minutes of LEZ blocks, more than one swap's drives are
/// apart) and the next one resumes from its cursor.
const SCAN_PAGE_BLOCKS: u32 = 16;
const SCAN_PAGES_PER_DRIVE: u32 = 8;
/// The same on a public Monero network: its windows are hours of LEZ
/// blocks (a block every two seconds or so on the devnet), so a look for the
/// Taker's authorisation or refund from the lock would take dozens of drives
/// at eight pages; 64 pages are 1,024 blocks a drive.
const PUBLIC_SCAN_PAGES_PER_DRIVE: u32 = 64;
/// Without the cursor the Node seeds at activation (a swap provisioned
/// before it was, or a lost file), blocks before the first drive's finalized
/// tip where the search for the Taker's lock starts: the Taker locks right
/// after activation, which the supervisor follows within seconds.
const TAG13_LOOKBACK_BLOCKS: u64 = 256;

/// The finalized LEZ evidence `drive` records in the evidence root.
const TAG13_FILE: &str = "tag13-funding-finalized.json";
const TAG14_FILE: &str = "tag14-finalized.json";
const TAG16_FILE: &str = "tag16-finalized.json";
/// The actor's own scan for the Maker's claim (the claim observer keeps its
/// finality evidence apart).
const TAG15_SCAN_FILE: &str = "tag15-scan-finalized.json";
const CLAIM_SIGNATURE_FILE: &str = "maker-claim-completed-signature.json";
const REFUND_SIGNATURE_FILE: &str = "maker-refund-observed-signature.json";
const CLAIM_ACTIVATION_FILE: &str = "maker-claim-activation.json";
const REFUND_ACTIVATION_FILE: &str = "maker-refund-activation.json";
const PUNISH_ACTIVATION_FILE: &str = "maker-punish-activation.json";
/// The funding tool's account that it failed before its transfer.
const FUNDING_NOT_SENT_FILE: &str = "monero-funding-not-sent.json";
const MAX_EVIDENCE_BYTES: u64 = 2 * 1024 * 1024;

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
    /// Observe the finalized chains and take the next step: fund Monero, or
    /// activate the branch the finalized evidence proves.
    Drive,
    /// Execute or reconcile the role-fixed Maker Tag15 Claim route.
    Claim,
    /// Execute or reconcile the Maker's refund sweep or Tag17 punishment.
    Recover,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ActorOutput {
    Status(StatusOutput),
    Effect(EffectOutput),
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
struct EffectOutput {
    schema_version: u16,
    role: &'static str,
    command: &'static str,
    outcome: &'static str,
    phase: &'static str,
    revision: u64,
    next_action: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RequestedEffect {
    Claim,
    Recover,
}

/// The Maker's own Monero funding in the workflow journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Funding {
    /// `FundMonero` has not consumed its invocation.
    NotInvoked,
    /// The funding tool ran (or may have): only observation remains.
    Invoked,
    /// The funding tool failed before its transfer: no Monero was sent.
    NotSent,
    /// Funding and receipt are reconciled.
    Reconciled,
}

/// Everything durable the status reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Facts {
    cutoff_ms: u64,
    refund_ms: u64,
    punish_ms: u64,
    /// From when a funding sent and not yet observed is in doubt: `C` on
    /// Regtest, where its tool mines it at once; the claim deadline on a
    /// public network, whose miners take tens of minutes to confirm it.
    funding_doubt_ms: u64,
    /// Consensus time of the block holding the Taker's finalized tag-13
    /// funding, once a drive saw it.
    tag13_final_ms: Option<u64>,
    funding: Funding,
    branch: Option<XmrWorkflowBranch>,
    /// The selected branch's effect is reconciled.
    branch_done: bool,
}

impl Facts {
    /// The latest finalized clock at which a seen tag 14 is still claimed:
    /// half the claim window before `R`, so tag 15 lands before `R`. A later
    /// tag 14 leaves the Taker its refund at `R` and the Maker its sweep.
    const fn claim_deadline_ms(&self) -> u64 {
        self.refund_ms
            .saturating_sub(self.refund_ms.saturating_sub(self.cutoff_ms) / 2)
    }

    /// Monotonic over every durable step.
    fn revision(&self) -> u64 {
        u64::from(self.tag13_final_ms.is_some())
            + match self.funding {
                Funding::NotInvoked => 0,
                Funding::Invoked => 1,
                Funding::NotSent | Funding::Reconciled => 2,
            }
            + u64::from(self.branch.is_some())
            + u64::from(self.branch_done)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Status {
    phase: &'static str,
    next_action: &'static str,
}

impl Status {
    const fn new(phase: &'static str, next_action: &'static str) -> Self {
        Self { phase, next_action }
    }
}

/// The status table: journal and evidence facts against the clock.
fn derive_status(facts: &Facts, now_ms: u64) -> Status {
    match (facts.branch, facts.branch_done) {
        (Some(XmrWorkflowBranch::Claim), true) => Status::new("completed", "complete"),
        (Some(XmrWorkflowBranch::Claim), false) => {
            Status::new("claim_evidence_available", "claim_lez")
        }
        (Some(XmrWorkflowBranch::Refund), true) => Status::new("refunded", "complete"),
        (Some(XmrWorkflowBranch::Refund), false) => {
            Status::new("maker_recovery_available", "refund_monero")
        }
        (Some(XmrWorkflowBranch::Punish), true) => Status::new("punished", "complete"),
        (Some(XmrWorkflowBranch::Punish), false) => {
            Status::new("maker_recovery_available", "punish_lez")
        }
        (None, _) => match facts.funding {
            Funding::Reconciled => Status::new("both_legs_locked", "observe_claim_or_refund"),
            // Still observed; past the cutoff a funding the observer has not
            // found needs the owner (the tool's failure is in the Node log).
            Funding::Invoked if now_ms >= facts.funding_doubt_ms => {
                Status::new("maker_funding_in_doubt", "fund_monero")
            }
            Funding::Invoked => Status::new("taker_lock_confirmed", "fund_monero"),
            // Nothing of the Maker's is at stake, and its one funding is spent.
            Funding::NotSent => Status::new("expired", "complete"),
            Funding::NotInvoked => match facts.tag13_final_ms {
                Some(final_ms) if final_ms > facts.cutoff_ms => Status::new("expired", "complete"),
                _ if now_ms >= facts.cutoff_ms => Status::new("expired", "complete"),
                Some(_) => Status::new("taker_lock_confirmed", "fund_monero"),
                None => Status::new("offered", "observe_lez_lock"),
            },
        },
    }
}

/// A finalized LEZ effect the Maker looks for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Watched {
    TakerLock,
    ClaimAuthorization,
    /// The Maker's own claim (tag 15): scanned only before a claim gives way
    /// to the punishment.
    Claim,
    Refund,
}

/// One classification page: the effect found, or the height after the
/// finalized window it scanned.
enum Page<T> {
    Found(T),
    Next(u64),
}

/// Pages through finalized history from `height` to `tip`, at most
/// `SCAN_PAGES_PER_DRIVE` pages, until `page` finds the effect. Returns it,
/// the height where the next scan resumes, and whether this one reached the
/// tip: an effect not found short of the tip may still be there.
fn scan_pages<T>(
    mut height: u64,
    tip: u64,
    pages: u32,
    mut page: impl FnMut(u64, u32) -> Result<Page<T>>,
) -> Result<(Option<T>, u64, bool)> {
    for _ in 0..pages {
        if height > tip {
            break;
        }
        let blocks = u32::try_from((tip - height + 1).min(u64::from(SCAN_PAGE_BLOCKS)))?;
        match page(height, blocks)? {
            Page::Found(found) => return Ok((Some(found), height, false)),
            Page::Next(next) => height = next,
        }
    }
    Ok((None, height, height > tip))
}

/// The outcome of one bounded finalized scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scan {
    /// Found final; for the Taker's lock, at this consensus time.
    Found(u64),
    /// Not found; `true` when the scan reached the finalized tip.
    Absent(bool),
}

/// The chains and durable state one drive works on.
trait MakerChains {
    fn facts(&self) -> Result<Facts>;
    /// The finalized LEZ clock's consensus time.
    fn lez_clock(&mut self) -> Result<u64>;
    /// Scans for `watched` and durably records it when found.
    fn scan(&mut self, watched: Watched) -> Result<Scan>;
    /// Whether monerod answers, asked before the one `FundMonero` invocation
    /// is consumed (the same daemon the Node's Monero route health asks).
    fn monero_answers(&mut self) -> bool;
    /// Consumes the one `FundMonero` invocation and runs the funding tool.
    fn invoke_fund(&mut self) -> Result<()>;
    /// Observes a consumed funding; true once reconciled.
    fn observe_fund(&mut self) -> Result<bool>;
    /// Activates `branch` from the recorded evidence.
    fn activate(&mut self, branch: XmrWorkflowBranch) -> Result<()>;
    /// Observes a spent branch effect sent under `plan`; true once reconciled.
    fn observe_effect(&mut self, step: XmrWorkflowStep, plan: [u8; 32]) -> Result<bool>;
    /// Whether the effect's tool stopped before recording its submission.
    fn submission_unwritten(&self, step: XmrWorkflowStep) -> Result<bool>;
    /// Runs the effect's tool again (`prepare_effect_redrive`).
    fn redrive(&mut self, step: XmrWorkflowStep) -> Result<()>;
}

/// One drive step; returns its outcome.
fn drive_once(chains: &mut impl MakerChains, now_ms: u64) -> Result<&'static str> {
    let facts = chains.facts()?;
    if facts.branch.is_some() || derive_status(&facts, now_ms).next_action == "complete" {
        return Ok("unchanged");
    }
    // The Taker authorizes its claim once it sees the funding at depth, about
    // when the Maker reconciles it: the drive that reconciles the funding also
    // looks for the claim authorization, so the claim window does not wait a
    // drive more for it (one drive is tens of seconds on a loaded host).
    // A failing observation (a public daemon refused as not synchronized or
    // stale, a wallet-rpc that does not answer) does not stop the drive: the
    // Taker's refund is still looked for below.
    let reconciled = facts.funding == Funding::Invoked
        && chains.observe_fund().unwrap_or_else(|error| {
            eprintln!("XMR Maker actor: the Monero funding is not reconciled yet: {error:#}");
            false
        });
    match facts.funding {
        Funding::NotSent => Ok("unchanged"),
        Funding::Invoked if !reconciled => {
            // A funding that may have been sent and is not recorded yet:
            // from R the Taker's refund (tag 16, whose signature reveals its
            // share) is looked for and recorded all the same, and the drive
            // that reconciles the funding activates the sweep back.
            if chains.lez_clock()? >= facts.refund_ms
                && matches!(chains.scan(Watched::Refund)?, Scan::Found(_))
            {
                return Ok("refund_seen_funding_unreconciled");
            }
            Ok("awaiting_observation")
        }
        Funding::NotInvoked => {
            if facts.tag13_final_ms.is_none() {
                return Ok(match chains.scan(Watched::TakerLock)? {
                    Scan::Found(_) => "observed",
                    Scan::Absent(_) => "unchanged",
                });
            }
            // Never after the cutoff: the finalized LEZ clock decides.
            if chains.lez_clock()? >= facts.cutoff_ms {
                return Ok("unchanged");
            }
            // The funding tool's attempts fail while monerod is down, and a
            // tool that fails before its transfer spends the swap's one
            // funding: the swap would expire with time left. While monerod
            // does not answer the invocation is kept for a later drive, until
            // the cutoff.
            if !chains.monero_answers() {
                return Ok("unchanged");
            }
            chains.invoke_fund()?;
            Ok("submitted")
        }
        Funding::Invoked | Funding::Reconciled => {
            let lez_ms = chains.lez_clock()?;
            let mut refund_absent_to_tip = false;
            if lez_ms >= facts.refund_ms {
                match chains.scan(Watched::Refund)? {
                    Scan::Found(_) => {
                        chains.activate(XmrWorkflowBranch::Refund)?;
                        return Ok("activated");
                    }
                    Scan::Absent(to_tip) => refund_absent_to_tip = to_tip,
                }
            }
            if lez_ms < facts.claim_deadline_ms()
                && matches!(chains.scan(Watched::ClaimAuthorization)?, Scan::Found(_))
            {
                chains.activate(XmrWorkflowBranch::Claim)?;
                return Ok("activated");
            }
            // Past `P` every block with a time before `P` is final, so a
            // refund absent to the tip can no longer land.
            if lez_ms >= facts.punish_ms && refund_absent_to_tip {
                chains.activate(XmrWorkflowBranch::Punish)?;
                return Ok("activated");
            }
            Ok(if reconciled {
                "reconciled"
            } else {
                "unchanged"
            })
        }
    }
}

/// Whether a claim may be sent at the finalized LEZ clock `lez_ms`: only
/// before the claim deadline, half the claim window before `R`, so that its
/// tag 15 lands before `R`. A tag 15 sent later may be rejected after its
/// completed signature, the Maker's key share, is out.
const fn claim_may_be_sent(facts: &Facts, lez_ms: u64) -> bool {
    lez_ms < facts.claim_deadline_ms()
}

/// A branch effect whose one invocation is spent: observed, or, when its
/// tool stopped before recording its submission (a punishment: whenever it is
/// not final), driven again under the same sidecar request ids
/// (`prepare_effect_redrive`). A claim the finalized
/// clock left behind gives way to the branch the chain proves: the Taker's
/// refund (the Maker then sweeps its Monero back) or, past `P` with no
/// refund to the tip, the punishment. Returns "completed" once reconciled.
fn settle_spent_effect(
    chains: &mut impl MakerChains,
    facts: &Facts,
    step: XmrWorkflowStep,
    plan: [u8; 32],
) -> Result<&'static str> {
    let unwritten = chains.submission_unwritten(step)?;
    // A sweep's observer needs its submission; the LEZ observers discover by
    // terms without one.
    if !(unwritten && step == XmrWorkflowStep::SweepMoneroRefund)
        && chains.observe_effect(step, plan)?
    {
        return Ok("completed");
    }
    if step == XmrWorkflowStep::ClaimLezTag15 {
        let lez_ms = chains.lez_clock()?;
        if lez_ms >= facts.refund_ms {
            let refund = chains.scan(Watched::Refund)?;
            // The claim observer reads an unavailable history or a spent
            // page budget as "not final yet": the punishment needs the
            // Maker's own claim proven absent to the finalized tip too, or a
            // claimed swap would switch to a tag 17 that can never land.
            if let Some(branch) = match refund {
                Scan::Found(_) => Some(XmrWorkflowBranch::Refund),
                Scan::Absent(true)
                    if lez_ms >= facts.punish_ms
                        && chains.scan(Watched::Claim)? == Scan::Absent(true) =>
                {
                    Some(XmrWorkflowBranch::Punish)
                }
                Scan::Absent(_) => None,
            } {
                chains.activate(branch)?;
                return Ok("activated");
            }
            // Tag 15 is valid only before `R`: what was sent is observed.
            return Ok("awaiting_observation");
        }
        // Driving the claim again may complete and send its signature now:
        // only while it still lands before `R`.
        if !claim_may_be_sent(facts, lez_ms) {
            return Ok("awaiting_observation");
        }
    }
    // A punishment not final is driven again even once recorded: the swap's
    // sidecar re-plans it at the live nonce when the Maker owner's account
    // passed its nonce without it (it can then never land, and after `P`
    // nothing else moves the escrow), and otherwise replays it unchanged.
    if unwritten || step == XmrWorkflowStep::PunishLezTag17 {
        chains.redrive(step)?;
        return Ok("resubmitted");
    }
    Ok("awaiting_observation")
}

fn main() {
    let arguments = Arguments::parse();
    let result = match arguments.command {
        Command::Status => status(arguments.config_fd).map(ActorOutput::Status),
        Command::Drive => drive(arguments.config_fd).map(ActorOutput::Effect),
        Command::Claim => {
            execute_effect(arguments.config_fd, RequestedEffect::Claim).map(ActorOutput::Effect)
        }
        Command::Recover => {
            execute_effect(arguments.config_fd, RequestedEffect::Recover).map(ActorOutput::Effect)
        }
    };
    match result {
        Ok(output) => match serde_json::to_string(&output) {
            Ok(json) => println!("{json}"),
            Err(_) => exit_with("XMR Maker actor output is unavailable"),
        },
        Err(error) => {
            if std::env::var_os("LEZ_BTC_ACTOR_TRACE").is_some() {
                eprintln!("XMR Maker actor: {error:#}");
            }
            exit_with("XMR Maker actor authority or effect is unavailable")
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn status(config_fd: i32) -> Result<StatusOutput> {
    let Ok(execution) = load_validated_xmr_maker_effect_execution_fd(config_fd) else {
        // A legacy bundle without an effect authority composes no chain
        // effect: it reports the pre-effect status, which stays blocked.
        let authority = load_validated_xmr_maker_authority_fd(config_fd)?;
        let _ = (authority.swap_id(), authority.state_database());
        return Ok(StatusOutput {
            schema_version: 1,
            actor_program: XMR_MAKER_ACTOR_PROGRAM_ID,
            actor_abi: XMR_MAKER_ACTOR_ABI_V1,
            role: "maker",
            state: "active",
            phase: "offered",
            revision: 0,
            next_action: XMR_MAKER_ACTOR_NEXT_ACTION,
            chain_effect_executed: false,
        });
    };
    let (agreement, _) = execution.agreement_and_terms()?;
    let facts = read_facts(&execution, windows_ms(&agreement))?;
    let status = derive_status(&facts, now_ms());
    Ok(StatusOutput {
        schema_version: 1,
        actor_program: XMR_MAKER_ACTOR_PROGRAM_ID,
        actor_abi: XMR_MAKER_ACTOR_ABI_V1,
        role: "maker",
        state: "active",
        phase: status.phase,
        revision: facts.revision(),
        next_action: status.next_action,
        chain_effect_executed: facts.funding != Funding::NotInvoked,
    })
}

fn effect_output(command: &'static str, outcome: &'static str, facts: &Facts) -> EffectOutput {
    effect_output_at(command, outcome, facts, now_ms())
}

/// The effect output against the facts at `now`. A drive that ends on an
/// expired swap reports the absorbing `unchanged`, whatever it did: a Taker
/// lock found final after the cutoff, or a scan the cutoff crossed, changes
/// nothing the Maker does, and the supervisor takes an expired drive only as
/// `unchanged` (it failed the schedule as `actor_output_invalid`).
fn effect_output_at(
    command: &'static str,
    outcome: &'static str,
    facts: &Facts,
    now_ms: u64,
) -> EffectOutput {
    let status = derive_status(facts, now_ms);
    let outcome = if command == "drive" && status.phase == "expired" {
        "unchanged"
    } else {
        outcome
    };
    EffectOutput {
        schema_version: 1,
        role: "maker",
        command,
        outcome,
        phase: status.phase,
        revision: facts.revision(),
        next_action: status.next_action,
    }
}

fn drive(config_fd: i32) -> Result<EffectOutput> {
    let mut maker = LiveMaker::open(config_fd)?;
    let outcome = drive_once(&mut maker, now_ms())?;
    Ok(effect_output("drive", outcome, &maker.facts()?))
}

/// Stage A's funding cutoff, refund and punish times. Validating Stage A
/// verifies both cross-curve proofs (about a second of CPU), so an actor
/// process does it once.
fn windows_ms(agreement: &XmrAgreementV1) -> [u64; 4] {
    let windows = agreement.body().windows();
    let (cutoff, refund) = (
        windows.maker_xmr_funding_cutoff_ms(),
        windows.refund_at_ms(),
    );
    let funding_doubt = if agreement_monero_network(agreement).mined_by_the_role() {
        cutoff
    } else {
        refund.saturating_sub(refund.saturating_sub(cutoff) / 2)
    };
    [cutoff, refund, windows.punish_at_ms(), funding_doubt]
}

/// How long one Monero effect tool may run on the agreement's network.
fn monero_effect_timeout(agreement: &XmrAgreementV1) -> Duration {
    if agreement_monero_network(agreement).mined_by_the_role() {
        MONERO_EFFECT_TIMEOUT
    } else {
        PUBLIC_MONERO_EFFECT_TIMEOUT
    }
}

/// The Maker's journal and evidence, with the clock windows of Stage A.
fn read_facts(
    execution: &ValidatedXmrEffectExecutionV3,
    [cutoff_ms, refund_ms, punish_ms, funding_doubt_ms]: [u64; 4],
) -> Result<Facts> {
    let identity = execution.workflow_identity();
    let workflow =
        SqliteXmrWorkflowJournal::open_existing(execution.effect_authority().workflow_journal())
            .context("open XMR Maker workflow")?;
    workflow
        .validate_initialized(identity)
        .context("validate XMR Maker workflow")?;
    let funding = match workflow.step_revision(identity, XmrWorkflowStep::FundMonero) {
        Err(StoreError::MissingXmrWorkflowStep) | Ok(0) => Funding::NotInvoked,
        Ok(_) => {
            if workflow
                .load_reconciliation(identity, XmrWorkflowStep::FundMonero)?
                .is_some()
            {
                Funding::Reconciled
            } else if execution
                .effect_authority()
                .evidence_root()
                .join(FUNDING_NOT_SENT_FILE)
                .exists()
            {
                Funding::NotSent
            } else {
                Funding::Invoked
            }
        }
        Err(error) => return Err(error).context("load XMR Maker funding"),
    };
    let branch = workflow.selected_branch(identity)?;
    let branch_done = match branch {
        Some(branch) => workflow
            .load_reconciliation(identity, branch_step(branch))?
            .is_some(),
        None => false,
    };
    let evidence_root = execution.effect_authority().evidence_root();
    let tag13_final_ms = read_finalized(&evidence_root.join(TAG13_FILE))?
        .map(|result| found_block(&result).map(|(_, timestamp)| timestamp))
        .transpose()?;
    Ok(Facts {
        cutoff_ms,
        refund_ms,
        punish_ms,
        funding_doubt_ms,
        tag13_final_ms,
        funding,
        branch,
        branch_done,
    })
}

const fn branch_step(branch: XmrWorkflowBranch) -> XmrWorkflowStep {
    match branch {
        XmrWorkflowBranch::Claim => XmrWorkflowStep::ClaimLezTag15,
        XmrWorkflowBranch::Refund => XmrWorkflowStep::SweepMoneroRefund,
        XmrWorkflowBranch::Punish => XmrWorkflowStep::PunishLezTag17,
    }
}

/// The containing block of a found finalized effect: height and time.
fn found_block(result: &ClassifyFinalizedNativeXmrEffectV3Result) -> Result<(u64, u64)> {
    match &result.outcome {
        FinalizedNativeXmrScanOutcomeV3::Found { facts, .. } => Ok((
            facts.containing_block.block_id,
            facts.containing_block.timestamp_ms,
        )),
        _ => Err(anyhow!("recorded LEZ evidence is not a found effect")),
    }
}

fn read_finalized(path: &Path) -> Result<Option<ClassifyFinalizedNativeXmrEffectV3Result>> {
    let Some(bytes) = read_private_file(path)? else {
        return Ok(None);
    };
    let result: ClassifyFinalizedNativeXmrEffectV3Result =
        serde_json::from_slice(&bytes).context("recorded LEZ evidence is malformed")?;
    ensure!(
        canonical_line(&result)? == bytes,
        "recorded LEZ evidence is noncanonical"
    );
    Ok(Some(result))
}

fn canonical_line(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value).context("encode canonical JSON")?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn read_private_file(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("open {}", path.display())),
    };
    let mut bytes = Vec::new();
    file.take(MAX_EVIDENCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {}", path.display()))?;
    ensure!(
        bytes.len() as u64 <= MAX_EVIDENCE_BYTES,
        "{} is oversized",
        path.display()
    );
    Ok(Some(bytes))
}

/// Writes owner-private evidence once (staged, then renamed into place); an
/// existing file must hold the same bytes. The actor's lock excludes any
/// other writer.
fn write_once(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(existing) = read_private_file(path)? {
        ensure!(
            existing == bytes,
            "{} differs from its replay",
            path.display()
        );
        return Ok(());
    }
    replace_private(path, bytes)
}

/// Writes owner-private bytes atomically, replacing what is there.
fn replace_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let staged = path.with_extension("staged");
    let _ = fs::remove_file(&staged);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staged)
        .with_context(|| format!("stage {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&staged, path).with_context(|| format!("publish {}", path.display()))?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

/// Where each watched effect's scan resumes: an advisory cursor, so a lost
/// file only rescans.
#[derive(Default, Deserialize, Serialize)]
struct ScanCursor {
    taker_lock: Option<u64>,
    claim_authorization: Option<u64>,
    #[serde(default)]
    claim: Option<u64>,
    refund: Option<u64>,
}

impl ScanCursor {
    const fn get(&self, watched: Watched) -> Option<u64> {
        match watched {
            Watched::TakerLock => self.taker_lock,
            Watched::ClaimAuthorization => self.claim_authorization,
            Watched::Claim => self.claim,
            Watched::Refund => self.refund,
        }
    }

    const fn set(&mut self, watched: Watched, height: u64) {
        match watched {
            Watched::TakerLock => self.taker_lock = Some(height),
            Watched::ClaimAuthorization => self.claim_authorization = Some(height),
            Watched::Claim => self.claim = Some(height),
            Watched::Refund => self.refund = Some(height),
        }
    }
}

/// A finalized observation: its evidence digest, the sending plan and the
/// reconciliation source the route derives.
type Observed = (
    [u8; 32],
    [u8; 32],
    lez_swap_store::XmrWorkflowReconciliationSource,
);

/// The live Maker: the validated execution, its locks, the swap's sidecar.
struct LiveMaker {
    execution: ValidatedXmrEffectExecutionV3,
    sources: XmrMakerEffectSourcesV1,
    terms: XmrNativeEscrowTermsV3,
    windows: [u64; 4],
    monero_timeout: Duration,
    scan_pages: u32,
    actor_lock: ActorHeldLock,
    workflow_lock: ActorHeldLock,
    runtime: tokio::runtime::Runtime,
    lez: Option<(BridgeClient, RuntimeDescriptor, RunId)>,
}

impl LiveMaker {
    fn open(config_fd: i32) -> Result<Self> {
        let execution = load_validated_xmr_maker_effect_execution_fd(config_fd)
            .context("validate XMR Maker effect execution")?;
        let sources = XmrMakerEffectSourcesV1::load_fd(config_fd)?;
        let (agreement, terms) = execution.agreement_and_terms()?;
        let (actor_lock, workflow_lock) = accept_locks(&execution)?;
        Ok(Self {
            execution,
            sources,
            terms,
            windows: windows_ms(&agreement),
            monero_timeout: monero_effect_timeout(&agreement),
            scan_pages: if agreement_monero_network(&agreement).mined_by_the_role() {
                SCAN_PAGES_PER_DRIVE
            } else {
                PUBLIC_SCAN_PAGES_PER_DRIVE
            },
            actor_lock,
            workflow_lock,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("start the XMR Maker actor runtime")?,
            lez: None,
        })
    }

    fn evidence(&self, name: &str) -> PathBuf {
        self.execution.effect_authority().evidence_root().join(name)
    }

    /// The swap's sidecar client, connected on first use.
    fn connect(&mut self) -> Result<()> {
        if self.lez.is_none() {
            let lez = self.execution.effect_authority().lez();
            let runtime: RuntimeDescriptor = serde_json::from_slice(
                &read_private_file(lez.runtime_file())?.context("the LEZ runtime is missing")?,
            )
            .context("the LEZ runtime is invalid")?;
            let capability = Zeroizing::new(String::from_utf8(
                read_private_file(lez.capability_file())?
                    .context("the LEZ capability is missing")?,
            )?);
            let run_id = RunId::new(self.execution.effect_authority().run_id().to_owned())?;
            let client = BridgeClient::connect(BridgeClientConfig::new(
                lez.sidecar_url().as_str(),
                SidecarCapability::new(capability.trim().to_owned())?,
                run_id.clone(),
                runtime.clone(),
                LEZ_REQUEST_TIMEOUT,
            ))
            .context("connect the swap's LEZ sidecar")?;
            self.lez = Some((client, runtime, run_id));
        }
        Ok(())
    }

    fn context(run_id: &RunId, label: &str) -> Result<MessageContext> {
        let request_id = RequestId::new(format!(
            "xmr-maker-{label}-{}-{}",
            now_ms(),
            std::process::id()
        ))?;
        Ok(MessageContext::new(
            run_id.clone(),
            request_id,
            Participant::Maker,
        ))
    }

    fn finalized_clock(&mut self) -> Result<(u64, u64)> {
        self.connect()?;
        let Some((client, runtime, run_id)) = &self.lez else {
            return Err(anyhow!("the swap's LEZ sidecar is not connected"));
        };
        let request =
            ObserveFinalizedClockRequest::new(Self::context(run_id, "clock")?, runtime.clone());
        let clock = self
            .runtime
            .block_on(client.observe_finalized_clock(request))
            .context("observe the finalized LEZ clock")?
            .clock;
        Ok((clock.height, clock.timestamp_ms))
    }

    fn cursor(&self) -> Result<ScanCursor> {
        Ok(read_private_file(&self.evidence(XMR_MAKER_LEZ_SCAN_FILE))?
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default())
    }

    /// Scans finalized history for `watched` from its cursor to the tip, at
    /// most a bounded number of pages per drive.
    fn classify(
        &mut self,
        watched: Watched,
    ) -> Result<(Option<ClassifyFinalizedNativeXmrEffectV3Result>, bool)> {
        let (tip, _) = self.finalized_clock()?;
        let mut cursor = self.cursor()?;
        let start = match (cursor.get(watched), watched) {
            (Some(height), _) => height,
            // LEZ block ids start at 1: a chain younger than the lookback
            // is scanned from its first block.
            (None, Watched::TakerLock) => tip.saturating_sub(TAG13_LOOKBACK_BLOCKS).max(1),
            (None, _) => {
                let lock = read_finalized(&self.evidence(TAG13_FILE))?
                    .context("the Taker's lock is not recorded")?;
                found_block(&lock)?.0.saturating_add(1)
            }
        };
        let effect = match watched {
            Watched::TakerLock => XmrNativeEffectV3::Fund,
            Watched::ClaimAuthorization => XmrNativeEffectV3::AuthorizeClaim,
            Watched::Claim => XmrNativeEffectV3::Claim,
            Watched::Refund => XmrNativeEffectV3::Refund,
        };
        let terms = self.terms;
        let Some((client, runtime, run_id)) = &self.lez else {
            return Err(anyhow!("the swap's LEZ sidecar is not connected"));
        };
        let (found, height, to_tip) = scan_pages(start, tip, self.scan_pages, |height, blocks| {
            let request = ClassifyFinalizedNativeXmrEffectV3Request::new(
                Self::context(run_id, "scan")?,
                runtime.clone(),
                terms,
                effect,
                FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {},
                DiscoveryWindow::new(height, blocks)?,
            );
            let result = self
                .runtime
                .block_on(client.classify_finalized_native_xmr_effect_v3(request))
                .context("classify finalized LEZ history")?;
            match &result.outcome {
                FinalizedNativeXmrScanOutcomeV3::Found { facts, .. } => {
                    ensure!(
                        facts.instruction.effect == effect,
                        "the classifier found another effect"
                    );
                    Ok(Page::Found(result))
                }
                // Uncertain says only that the escrow's state at the window's
                // end does not exclude the effect later: the window itself is
                // final and holds none, so the scan moves past it either way.
                FinalizedNativeXmrScanOutcomeV3::Absent { scanned_window, .. }
                | FinalizedNativeXmrScanOutcomeV3::Uncertain { scanned_window, .. } => {
                    Ok(Page::Next(
                        scanned_window
                            .start_height()
                            .saturating_add(u64::from(scanned_window.max_blocks())),
                    ))
                }
                FinalizedNativeXmrScanOutcomeV3::Unavailable { reason } => {
                    Err(anyhow!("finalized LEZ history unavailable: {reason:?}"))
                }
            }
        })?;
        if found.is_none() {
            cursor.set(watched, height);
            replace_private(
                &self.evidence(XMR_MAKER_LEZ_SCAN_FILE),
                &serde_json::to_vec(&cursor)?,
            )?;
        }
        Ok((found, to_tip))
    }

    fn manifest_inputs(&self) -> (&[u8], &[u8], &str) {
        (
            self.sources.manifest_bytes(),
            self.sources.authority_bytes(),
            self.execution.effect_authority().run_id(),
        )
    }

    /// Re-runs a branch activation from its durable inputs and records its
    /// summary; activations replay exactly.
    fn record_activation(&self, branch: XmrWorkflowBranch) -> Result<()> {
        let (manifest, authority, run_id) = self.manifest_inputs();
        let funding = self.evidence(XMR_MAKER_FUNDING_EVIDENCE_FILE);
        let receipt = self.evidence(XMR_MAKER_FUNDING_RECEIPT_FILE);
        let (summary, file) = match branch {
            XmrWorkflowBranch::Claim => {
                let signature = self.evidence(CLAIM_SIGNATURE_FILE);
                if !signature.exists() {
                    complete_maker_claim_signature(
                        &self.sources,
                        &self.evidence(TAG14_FILE),
                        &signature,
                    )?;
                }
                (
                    activate_maker_claim_workflow_bytes(
                        manifest,
                        authority,
                        run_id,
                        run_id,
                        &funding,
                        &receipt,
                        &self.evidence(TAG14_FILE),
                        &signature,
                    )?,
                    CLAIM_ACTIVATION_FILE,
                )
            }
            XmrWorkflowBranch::Refund => {
                let signature = self.evidence(REFUND_SIGNATURE_FILE);
                if !signature.exists() {
                    ingest_maker_refund_signature(
                        &self.sources,
                        &self.evidence(TAG16_FILE),
                        &signature,
                    )?;
                }
                (
                    activate_maker_refund_workflow_bytes(
                        manifest,
                        authority,
                        run_id,
                        run_id,
                        &funding,
                        &receipt,
                        &self.evidence(TAG16_FILE),
                        &signature,
                    )?,
                    REFUND_ACTIVATION_FILE,
                )
            }
            XmrWorkflowBranch::Punish => {
                // Tag 17 is sent only after this activation, which follows
                // a refund scan to the finalized tip, so it lands past that
                // scan's cursor: its observer scans from there, not from the
                // Taker's lock, a whole `[C, P)` before (hundreds of blocks,
                // more than one observer run scans on the fast profile).
                let start = if let Some(height) = self.cursor()?.refund {
                    height
                } else {
                    let lock = read_finalized(&self.evidence(TAG13_FILE))?
                        .context("the Taker's lock is not recorded")?;
                    found_block(&lock)?.0.saturating_add(1)
                };
                (
                    activate_maker_punish_workflow_bytes(
                        manifest, authority, run_id, run_id, &funding, &receipt, start,
                    )?,
                    PUNISH_ACTIVATION_FILE,
                )
            }
        };
        write_once(&self.evidence(file), &summary)
    }

    fn run_observer(&self, step: XmrWorkflowStep, timeout: Duration) -> Result<Option<Observed>> {
        let prepared = self
            .execution
            .prepare_effect_observation(step, &self.actor_lock, &self.workflow_lock)
            .context("prepare XMR Maker observation")?;
        let (mut command, plan, source) = prepared.into_parts();
        let bytes = run_observer_command(&mut command, timeout)?;
        let result = parse_xmr_effect_observer_result_v1(&bytes, step)
            .context("parse XMR Maker observer output")?;
        match result.state() {
            XmrEffectObserverStateV1::Pending => Ok(None),
            XmrEffectObserverStateV1::Finalized => Ok(Some((
                result
                    .effect_evidence_sha256()
                    .context("finalized XMR Maker observation lacks evidence")?,
                plan,
                source,
            ))),
        }
    }
}

impl MakerChains for LiveMaker {
    fn facts(&self) -> Result<Facts> {
        read_facts(&self.execution, self.windows)
    }

    fn lez_clock(&mut self) -> Result<u64> {
        Ok(self.finalized_clock()?.1)
    }

    fn scan(&mut self, watched: Watched) -> Result<Scan> {
        let file = match watched {
            Watched::TakerLock => TAG13_FILE,
            Watched::ClaimAuthorization => TAG14_FILE,
            Watched::Claim => TAG15_SCAN_FILE,
            Watched::Refund => TAG16_FILE,
        };
        // Found once is found: a later scan's clock and window differ, so a
        // drive that failed after recording the effect resumes from the record.
        if let Some(recorded) = read_finalized(&self.evidence(file))? {
            return Ok(Scan::Found(found_block(&recorded)?.1));
        }
        let (found, to_tip) = self.classify(watched)?;
        let Some(result) = found else {
            return Ok(Scan::Absent(to_tip));
        };
        write_once(&self.evidence(file), &canonical_line(&result)?)?;
        Ok(Scan::Found(found_block(&result)?.1))
    }

    fn monero_answers(&mut self) -> bool {
        let url = self.execution.effect_authority().monero().daemon().url();
        let answers = monerod_answers(url.as_str());
        if !answers {
            eprintln!(
                "XMR Maker actor: monerod does not answer at {url}; the Monero funding waits for a later drive"
            );
        }
        answers
    }

    fn invoke_fund(&mut self) -> Result<()> {
        let identity = self.execution.workflow_identity();
        let mut workflow = SqliteXmrWorkflowJournal::open_existing(
            self.execution.effect_authority().workflow_journal(),
        )?;
        workflow.validate_initialized(identity)?;
        workflow
            .prepare_step(identity, XmrWorkflowStep::FundMonero)
            .context("prepare the Maker's Monero funding")?;
        drop(workflow);
        match self
            .execution
            .prepare_effect_invocation(
                XmrWorkflowStep::FundMonero,
                &self.actor_lock,
                &self.workflow_lock,
            )
            .context("prepare the Maker's Monero funding")?
        {
            XmrPreparedEffectInvocationV1::InvokeOnce { mut command, .. } => {
                match run_silent(&mut command, self.monero_timeout) {
                    Ok(()) => Ok(()),
                    Err(error)
                        if error.downcast_ref::<ChildExit>()
                            == Some(&ChildExit(Some(XMR_MONERO_FUNDING_NOT_SENT_EXIT))) =>
                    {
                        write_once(
                            &self.evidence(FUNDING_NOT_SENT_FILE),
                            b"{\"schema_version\":1,\"step\":\"fund_monero\",\"sent\":false}\n",
                        )?;
                        Err(error.context("the Monero funding was not sent"))
                    }
                    Err(error) => {
                        mark_unknown(&self.execution, XmrWorkflowStep::FundMonero)?;
                        Err(error.context("the Monero funding is in doubt"))
                    }
                }
            }
            XmrPreparedEffectInvocationV1::ObserveOnly { .. }
            | XmrPreparedEffectInvocationV1::Complete { .. } => Ok(()),
        }
    }

    fn observe_fund(&mut self) -> Result<bool> {
        let Some((digest, _, _)) =
            self.run_observer(XmrWorkflowStep::FundMonero, self.monero_timeout)?
        else {
            return Ok(false);
        };
        let (manifest, authority, run_id) = self.manifest_inputs();
        let reconciled = reconcile_maker_monero_funding_bytes(
            manifest,
            authority,
            run_id,
            run_id,
            &self.evidence(XMR_MAKER_FUNDING_EVIDENCE_FILE),
            &self.evidence(XMR_MAKER_FUNDING_RECEIPT_FILE),
        )?;
        ensure!(
            reconciled == digest,
            "the Monero funding observer reported other evidence"
        );
        Ok(true)
    }

    fn activate(&mut self, branch: XmrWorkflowBranch) -> Result<()> {
        self.record_activation(branch)
    }

    fn observe_effect(&mut self, step: XmrWorkflowStep, plan: [u8; 32]) -> Result<bool> {
        let timeout = effect_timeout(step, self.monero_timeout);
        let Some((evidence, observed_plan, source)) = self.run_observer(step, timeout)? else {
            return Ok(false);
        };
        ensure!(observed_plan == plan, "XMR Maker recovery plan changed");
        let reconciliation = XmrWorkflowReconciliationV2::new(evidence, plan, source)
            .context("validate XMR Maker recovery evidence")?;
        let mut workflow = SqliteXmrWorkflowJournal::open_existing(
            self.execution.effect_authority().workflow_journal(),
        )
        .context("open XMR Maker recovery workflow")?;
        workflow
            .validate_initialized(self.execution.workflow_identity())
            .context("validate XMR Maker recovery workflow")?;
        workflow
            .reconcile_succeeded(self.execution.workflow_identity(), step, &reconciliation)
            .context("reconcile XMR Maker recovery")?;
        Ok(true)
    }

    fn submission_unwritten(&self, step: XmrWorkflowStep) -> Result<bool> {
        xmr_maker_submission_unwritten(self.execution.effect_authority().evidence_root(), step)
    }

    fn redrive(&mut self, step: XmrWorkflowStep) -> Result<()> {
        let mut command = self
            .execution
            .prepare_effect_redrive(step, &self.actor_lock, &self.workflow_lock)
            .context("prepare the XMR Maker effect's redrive")?;
        run_silent(&mut command, effect_timeout(step, self.monero_timeout)).map_err(|error| {
            error.context("the XMR Maker effect's redrive failed; it is observed and driven again")
        })
    }
}

/// Whether the daemon at `url` (a literal loopback `http://host:port/`)
/// answers HTTP at all: monerod's Digest challenge counts. The Node's forward
/// accepts the connection even while monerod is down, then closes it unanswered.
fn monerod_answers(url: &str) -> bool {
    use std::io::{Read as _, Write as _};
    let Some(authority) = url
        .strip_prefix("http://")
        .and_then(|rest| rest.split('/').next())
    else {
        return false;
    };
    let Some(address) = authority.parse::<std::net::SocketAddr>().ok() else {
        return false;
    };
    let body = r#"{"jsonrpc":"2.0","id":0,"method":"get_info"}"#;
    let request = format!(
        "POST /json_rpc HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&address, MONEROD_PROBE_TIMEOUT)
    else {
        return false;
    };
    let mut answer = [0_u8; 8];
    stream.set_read_timeout(Some(MONEROD_PROBE_TIMEOUT)).is_ok()
        && stream
            .set_write_timeout(Some(MONEROD_PROBE_TIMEOUT))
            .is_ok()
        && stream.write_all(request.as_bytes()).is_ok()
        && stream.read_exact(&mut answer).is_ok()
        && answer.starts_with(b"HTTP/1.")
}

/// A Monero effect mines its confirmations or waits on a public node
/// (`monero`, by the network); a LEZ effect asks the sidecar.
fn effect_timeout(step: XmrWorkflowStep, monero: Duration) -> Duration {
    if matches!(
        step,
        XmrWorkflowStep::FundMonero | XmrWorkflowStep::SweepMoneroRefund
    ) {
        monero
    } else {
        EFFECT_TIMEOUT
    }
}

/// Accepts the supervisor's transferred actor lock and takes the workflow's.
fn accept_locks(
    execution: &ValidatedXmrEffectExecutionV3,
) -> Result<(ActorHeldLock, ActorHeldLock)> {
    let identity = execution.workflow_identity();
    let authority = execution.effect_authority();
    let transferred = stdin()
        .as_fd()
        .try_clone_to_owned()
        .context("clone transferred XMR Maker actor lock")?;
    let actor_lock = ActorHeldLock::accept_transferred_for(
        identity.swap_id(),
        authority.adaptor_journal(),
        File::from(transferred),
    )
    .context("accept transferred XMR Maker actor lock")?;
    let workflow_lock =
        ActorHeldLock::acquire_for(identity.swap_id(), authority.workflow_journal())
            .context("acquire XMR Maker workflow lock")?;
    Ok((actor_lock, workflow_lock))
}

fn execute_effect(config_fd: i32, requested: RequestedEffect) -> Result<EffectOutput> {
    let mut maker = LiveMaker::open(config_fd)?;
    let recovery_step = selected_effect_step(&maker.execution, requested)?;
    // The LEZ effect tools reach the swap's sidecar, which a Node started
    // again respawns beside its actors: one not answering yet fails the
    // attempt here, before the step's one invocation is consumed.
    let lez_ms = if matches!(
        recovery_step,
        XmrWorkflowStep::ClaimLezTag15 | XmrWorkflowStep::PunishLezTag17
    ) {
        Some(
            maker
                .finalized_clock()
                .context("the swap's LEZ sidecar does not answer yet")?
                .1,
        )
    } else {
        None
    };
    let execution = &maker.execution;
    // The branch's observers read its activation summary: a crash between
    // the activation's journal commit and the summary's write leaves it to
    // be recorded here (activations replay exactly).
    if let Some((branch, file, input)) = match recovery_step {
        XmrWorkflowStep::ClaimLezTag15 => {
            Some((XmrWorkflowBranch::Claim, CLAIM_ACTIVATION_FILE, TAG14_FILE))
        }
        XmrWorkflowStep::PunishLezTag17 => Some((
            XmrWorkflowBranch::Punish,
            PUNISH_ACTIVATION_FILE,
            TAG13_FILE,
        )),
        _ => None,
    } && !maker.evidence(file).exists()
        && maker.evidence(input).exists()
    {
        maker.record_activation(branch)?;
    }
    let timeout = effect_timeout(recovery_step, maker.monero_timeout);

    if recovery_requires_preflight(recovery_step) {
        execute_preflight(
            execution,
            recovery_step,
            &maker.actor_lock,
            &maker.workflow_lock,
        )?;
    }
    let prepared = execution
        .prepare_effect_invocation(recovery_step, &maker.actor_lock, &maker.workflow_lock)
        .context("prepare XMR Maker recovery")?;
    let finalized = match prepared {
        XmrPreparedEffectInvocationV1::InvokeOnce {
            mut command,
            tool_plan_identity_sha256: _,
        } => {
            // A claim selected before a stop may first be invoked after the
            // claim deadline: its tag 15 could then land at or after `R`,
            // and a rejected tag 15 still carries the completed signature,
            // which reveals the Maker's key share. Past the deadline the
            // tool is not run. The invocation stays spent with nothing
            // written, so the observer decides: the Taker's refund at `R`,
            // or the punishment past `P` (`settle_spent_effect`).
            if recovery_step == XmrWorkflowStep::ClaimLezTag15 {
                let facts = maker.facts()?;
                if !lez_ms.is_some_and(|lez_ms| claim_may_be_sent(&facts, lez_ms)) {
                    eprintln!(
                        "XMR Maker claim withheld: the finalized LEZ clock is past the claim deadline"
                    );
                    return Ok(effect_output("claim", "awaiting_observation", &facts));
                }
            }
            if let Err(error) = run_silent(&mut command, timeout) {
                mark_unknown(execution, recovery_step)?;
                return Err(error.context("XMR Maker recovery invocation is ambiguous"));
            }
            #[cfg(feature = "test-crash-hooks")]
            pause_after_submitted_if_armed(execution, recovery_step, requested)?;
            false
        }
        XmrPreparedEffectInvocationV1::ObserveOnly {
            tool_plan_identity_sha256,
        } => {
            let facts = maker.facts()?;
            settle_spent_effect(&mut maker, &facts, recovery_step, tool_plan_identity_sha256)?
                == "completed"
        }
        XmrPreparedEffectInvocationV1::Complete {
            tool_plan_identity_sha256: _,
        } => true,
    };
    let facts = maker.facts()?;
    let (command, outcome) = match (requested, finalized, facts.branch) {
        (RequestedEffect::Claim, true, _) => ("claim", "completed"),
        (RequestedEffect::Claim, false, _) => ("claim", "awaiting_observation"),
        (RequestedEffect::Recover, true, Some(XmrWorkflowBranch::Punish)) => {
            ("recover", "punished")
        }
        (RequestedEffect::Recover, true, _) => ("recover", "refunded"),
        (RequestedEffect::Recover, false, _) => ("recover", "awaiting_observation"),
    };
    Ok(effect_output(command, outcome, &facts))
}

#[cfg(feature = "test-crash-hooks")]
fn pause_after_submitted_if_armed(
    execution: &ValidatedXmrEffectExecutionV3,
    recovery_step: XmrWorkflowStep,
    requested: RequestedEffect,
) -> Result<()> {
    if !matches!(
        recovery_step,
        XmrWorkflowStep::ClaimLezTag15 | XmrWorkflowStep::SweepMoneroRefund
    ) {
        return Ok(());
    }
    let (Some(operation), Some(marker)) = (
        std::env::var_os("LEZ_ACTOR_TEST_PAUSE_AFTER_SUBMITTED"),
        std::env::var_os("LEZ_ACTOR_TEST_PAUSE_MARKER"),
    ) else {
        return Ok(());
    };
    let operation = operation
        .to_str()
        .context("read XMR Maker test pause operation")?;
    let submitted = serde_json::json!({
        "schema_version": 1,
        "role": "maker",
        "command": match requested {
            RequestedEffect::Claim => "claim",
            RequestedEffect::Recover => "recover",
        },
        "outcome": "submitted",
        "operation": recovery_step.name(),
    })
    .to_string();
    if arm_test_crash_hook(
        operation,
        marker.as_ref(),
        execution.workflow_identity().swap_id().as_str(),
        "maker",
        &submitted,
    )
    .context("arm XMR Maker submitted-effect test pause")?
    {
        loop {
            std::thread::park();
        }
    }
    Ok(())
}

fn execute_preflight(
    execution: &ValidatedXmrEffectExecutionV3,
    recovery_step: XmrWorkflowStep,
    actor_lock: &ActorHeldLock,
    workflow_lock: &ActorHeldLock,
) -> Result<()> {
    let Some(mut command) = execution
        .prepare_effect_preflight(recovery_step, actor_lock, workflow_lock)
        .context("prepare XMR Maker recovery preflight")?
    else {
        return Ok(());
    };
    run_silent(&mut command, EFFECT_TIMEOUT).context("XMR Maker recovery preflight failed")
}

fn run_silent(command: &mut ProcessCommand, timeout: Duration) -> Result<()> {
    // The tool's own error line reaches the Node's log beside the actor's.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let mut child = command.spawn().context("spawn XMR Maker effect child")?;
    let status = match child.wait_timeout(timeout) {
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
    ensure!(status.success(), ChildExit(status.code()));
    Ok(())
}

/// An effect child's unsuccessful exit, with its status code.
#[derive(Debug, Eq, PartialEq)]
struct ChildExit(Option<i32>);

impl std::fmt::Display for ChildExit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(code) => write!(formatter, "XMR Maker effect child failed (exit {code})"),
            None => formatter.write_str("XMR Maker effect child failed (signal)"),
        }
    }
}

fn run_observer_command(command: &mut ProcessCommand, timeout: Duration) -> Result<Vec<u8>> {
    // As an effect tool's, an observer's error line reaches the Node's log.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = command.spawn().context("spawn XMR Maker observer")?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(anyhow!("XMR Maker observer output is unavailable"));
    };
    let status = match child.wait_timeout(timeout) {
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
    Ok(bytes)
}

fn mark_unknown(
    execution: &ValidatedXmrEffectExecutionV3,
    recovery_step: XmrWorkflowStep,
) -> Result<()> {
    let mut workflow =
        SqliteXmrWorkflowJournal::open_existing(execution.effect_authority().workflow_journal())
            .context("open ambiguous XMR Maker workflow")?;
    workflow
        .validate_initialized(execution.workflow_identity())
        .context("validate ambiguous XMR Maker workflow")?;
    workflow
        .mark_unknown(execution.workflow_identity(), recovery_step)
        .context("mark XMR Maker effect ambiguous")
}

fn selected_effect_step(
    execution: &ValidatedXmrEffectExecutionV3,
    requested: RequestedEffect,
) -> Result<XmrWorkflowStep> {
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
    match requested {
        RequestedEffect::Claim => match branch {
            XmrWorkflowBranch::Claim => Ok(XmrWorkflowStep::ClaimLezTag15),
            XmrWorkflowBranch::Refund | XmrWorkflowBranch::Punish => Err(anyhow!(
                "Maker Claim command requires the durable Claim branch"
            )),
        },
        RequestedEffect::Recover => recovery_step_for_branch(branch),
    }
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

    const C: u64 = 260_000;
    const R: u64 = 360_000;
    const P: u64 = 480_000;

    fn facts() -> Facts {
        Facts {
            cutoff_ms: C,
            refund_ms: R,
            punish_ms: P,
            funding_doubt_ms: C,
            tag13_final_ms: None,
            funding: Funding::NotInvoked,
            branch: None,
            branch_done: false,
        }
    }

    #[test]
    fn one_drive_scans_a_bounded_number_of_pages_and_the_next_resumes() {
        // 300 finalized blocks behind: one drive scans 128 and stops short of
        // the tip, so a watched effect not found there is not yet absent.
        let mut pages = Vec::new();
        let (found, next, to_tip) =
            scan_pages::<()>(1, 300, SCAN_PAGES_PER_DRIVE, |height, blocks| {
                pages.push((height, blocks));
                Ok(Page::Next(height + u64::from(blocks)))
            })
            .unwrap();
        assert_eq!((found, next, to_tip), (None, 129, false));
        assert_eq!(pages.len(), 8);
        assert_eq!(pages[7], (113, 16));
        // The next drive resumes there and reaches the tip.
        pages.clear();
        let (found, next, to_tip) =
            scan_pages::<()>(next, 300, SCAN_PAGES_PER_DRIVE, |height, blocks| {
                pages.push((height, blocks));
                Ok(Page::Next(height + u64::from(blocks)))
            })
            .unwrap();
        assert_eq!((found, next, to_tip), (None, 257, false));
        let (found, next, to_tip) =
            scan_pages::<()>(next, 300, SCAN_PAGES_PER_DRIVE, |height, blocks| {
                Ok(Page::Next(height + u64::from(blocks)))
            })
            .unwrap();
        assert_eq!((found, next, to_tip), (None, 301, true));
        // At the tip already: nothing to ask, and the tip is reached.
        let (found, _, to_tip) =
            scan_pages::<()>(301, 300, SCAN_PAGES_PER_DRIVE, |_, _| unreachable!()).unwrap();
        assert_eq!((found, to_tip), (None, true));
        // A found effect ends the scan where it was found.
        let (found, at, to_tip) = scan_pages(1, 300, SCAN_PAGES_PER_DRIVE, |height, blocks| {
            Ok(if height == 33 {
                Page::Found(height)
            } else {
                Page::Next(height + u64::from(blocks))
            })
        })
        .unwrap();
        assert_eq!((found, at, to_tip), (Some(33), 33, false));
        // On a public network one drive scans 1,024 blocks.
        let (found, next, to_tip) =
            scan_pages::<()>(1, 2_000, PUBLIC_SCAN_PAGES_PER_DRIVE, |height, blocks| {
                Ok(Page::Next(height + u64::from(blocks)))
            })
            .unwrap();
        assert_eq!((found, next, to_tip), (None, 1_025, false));
    }

    #[test]
    fn the_lock_search_starts_at_the_height_the_node_seeded_at_activation() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("evidence")).unwrap();
        xmr_reference_actor::seed_xmr_maker_lez_scan(directory.path(), 700).unwrap();
        // A provisioning run again keeps the lower height it seeded first.
        xmr_reference_actor::seed_xmr_maker_lez_scan(directory.path(), 900).unwrap();
        let cursor: ScanCursor = serde_json::from_slice(
            &fs::read(
                directory
                    .path()
                    .join("evidence")
                    .join(XMR_MAKER_LEZ_SCAN_FILE),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(cursor.get(Watched::TakerLock), Some(700));
        assert_eq!(cursor.get(Watched::ClaimAuthorization), None);
        assert_eq!(cursor.get(Watched::Refund), None);
    }

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
        assert_eq!(XmrWorkflowStep::ClaimLezTag15.name(), "claim_lez_tag15");
    }

    /// A Taker lock final after the cutoff, or a drive whose scan the cutoff
    /// crossed, ends the drive on `expired`: its output is the absorbing
    /// `unchanged` the supervisor accepts, not `observed` (which it took for
    /// invalid output and failed the schedule on).
    #[test]
    fn a_drive_that_ends_on_an_expired_swap_reports_unchanged() {
        let mut late = facts();
        late.tag13_final_ms = Some(C + 1);
        let output = effect_output_at("drive", "observed", &late, 1);
        assert_eq!(
            (output.outcome, output.phase, output.next_action),
            ("unchanged", "expired", "complete")
        );
        let mut crossed = facts();
        crossed.tag13_final_ms = Some(C - 1);
        let output = effect_output_at("drive", "observed", &crossed, C);
        assert_eq!(
            (output.outcome, output.phase, output.next_action),
            ("unchanged", "expired", "complete")
        );
        // Before the cutoff an observed lock stays observed.
        let output = effect_output_at("drive", "observed", &crossed, C - 1);
        assert_eq!(
            (output.outcome, output.phase),
            ("observed", "taker_lock_confirmed")
        );
    }

    #[test]
    fn the_status_table_follows_the_journal_the_evidence_and_the_clock() {
        let status = |facts: Facts, now| {
            let status = derive_status(&facts, now);
            (status.phase, status.next_action)
        };
        let mut swap = facts();
        assert_eq!(status(swap, 1), ("offered", "observe_lez_lock"));
        assert_eq!(status(swap, C), ("expired", "complete"));
        swap.tag13_final_ms = Some(C + 1);
        assert_eq!(status(swap, 1), ("expired", "complete"));
        swap.tag13_final_ms = Some(C);
        assert_eq!(status(swap, 1), ("taker_lock_confirmed", "fund_monero"));
        assert_eq!(status(swap, C), ("expired", "complete"));
        // A consumed funding is observed whatever the clock says; past the
        // cutoff, unobserved, it is in doubt. One that was not sent expires.
        swap.funding = Funding::Invoked;
        assert_eq!(status(swap, 1), ("taker_lock_confirmed", "fund_monero"));
        assert_eq!(status(swap, P), ("maker_funding_in_doubt", "fund_monero"));
        // On a public network a funding still confirming after C is not in
        // doubt until the claim deadline.
        let mut public = swap;
        public.funding_doubt_ms = public.claim_deadline_ms();
        assert_eq!(status(public, C), ("taker_lock_confirmed", "fund_monero"));
        assert_eq!(
            status(public, public.claim_deadline_ms()),
            ("maker_funding_in_doubt", "fund_monero")
        );
        swap.funding = Funding::NotSent;
        assert_eq!(status(swap, 1), ("expired", "complete"));
        swap.funding = Funding::Reconciled;
        assert_eq!(
            status(swap, P),
            ("both_legs_locked", "observe_claim_or_refund")
        );
        for (branch, open, done) in [
            (
                XmrWorkflowBranch::Claim,
                ("claim_evidence_available", "claim_lez"),
                ("completed", "complete"),
            ),
            (
                XmrWorkflowBranch::Refund,
                ("maker_recovery_available", "refund_monero"),
                ("refunded", "complete"),
            ),
            (
                XmrWorkflowBranch::Punish,
                ("maker_recovery_available", "punish_lez"),
                ("punished", "complete"),
            ),
        ] {
            swap.branch = Some(branch);
            swap.branch_done = false;
            assert_eq!(status(swap, 1), open);
            swap.branch_done = true;
            assert_eq!(status(swap, 1), done);
        }
        assert_eq!(swap.revision(), 5);
    }

    impl Default for Scan {
        fn default() -> Self {
            Self::Absent(true)
        }
    }

    /// The durable model a stub drive works on, with every effect counted
    /// and an optional crash right after an effect's durable consumption.
    #[derive(Default)]
    #[allow(
        clippy::struct_excessive_bools,
        reason = "each flag is one independent stubbed chain fact"
    )]
    struct Stub {
        facts: Option<Facts>,
        lez_ms: u64,
        tag13: Option<u64>,
        tag14: bool,
        tag16: bool,
        refund_scan_to_tip: bool,
        /// The Maker's tag 15 scan: found, or absent with whether it reached
        /// the tip.
        tag15: Scan,
        funds: u32,
        observed_funding: bool,
        activations: Vec<XmrWorkflowBranch>,
        crash_after_fund: bool,
        /// The branch effect is final on its chain.
        effect_final: bool,
        /// Its tool stopped before recording its submission.
        unwritten: bool,
        observations: u32,
        redrives: u32,
        /// monerod does not answer.
        monero_down: bool,
        /// The funding observer fails (its daemon is refused).
        observe_fails: bool,
        refund_scans: u32,
    }

    impl Stub {
        fn new() -> Self {
            Self {
                facts: Some(facts()),
                refund_scan_to_tip: true,
                tag15: Scan::Absent(true),
                ..Self::default()
            }
        }

        fn durable(&mut self) -> &mut Facts {
            self.facts.as_mut().unwrap()
        }
    }

    impl MakerChains for Stub {
        fn facts(&self) -> Result<Facts> {
            Ok(self.facts.unwrap())
        }

        fn lez_clock(&mut self) -> Result<u64> {
            Ok(self.lez_ms)
        }

        fn scan(&mut self, watched: Watched) -> Result<Scan> {
            Ok(match watched {
                Watched::TakerLock => match self.tag13 {
                    Some(final_ms) => {
                        self.durable().tag13_final_ms = Some(final_ms);
                        Scan::Found(final_ms)
                    }
                    None => Scan::Absent(true),
                },
                Watched::ClaimAuthorization if self.tag14 => Scan::Found(0),
                Watched::Refund if self.tag16 => {
                    self.refund_scans += 1;
                    Scan::Found(0)
                }
                Watched::Refund => Scan::Absent(self.refund_scan_to_tip),
                Watched::ClaimAuthorization => Scan::Absent(true),
                Watched::Claim => self.tag15,
            })
        }

        fn monero_answers(&mut self) -> bool {
            !self.monero_down
        }

        fn invoke_fund(&mut self) -> Result<()> {
            assert_eq!(self.durable().funding, Funding::NotInvoked);
            self.durable().funding = Funding::Invoked;
            self.funds += 1;
            if self.crash_after_fund {
                return Err(anyhow!("crashed after the funding"));
            }
            Ok(())
        }

        fn observe_fund(&mut self) -> Result<bool> {
            if self.observe_fails {
                return Err(anyhow!("the Monero daemon is not synchronized"));
            }
            if self.observed_funding {
                self.durable().funding = Funding::Reconciled;
            }
            Ok(self.observed_funding)
        }

        fn activate(&mut self, branch: XmrWorkflowBranch) -> Result<()> {
            // Only an unreconciled claim gives way (the journal's rule).
            let durable = self.durable();
            assert!(
                durable.branch.is_none()
                    || (durable.branch == Some(XmrWorkflowBranch::Claim)
                        && branch != XmrWorkflowBranch::Claim
                        && !durable.branch_done)
            );
            durable.branch = Some(branch);
            self.activations.push(branch);
            Ok(())
        }

        fn observe_effect(&mut self, _: XmrWorkflowStep, _: [u8; 32]) -> Result<bool> {
            self.observations += 1;
            if self.effect_final {
                self.durable().branch_done = true;
            }
            Ok(self.effect_final)
        }

        fn submission_unwritten(&self, _: XmrWorkflowStep) -> Result<bool> {
            Ok(self.unwritten)
        }

        fn redrive(&mut self, step: XmrWorkflowStep) -> Result<()> {
            assert!(
                self.unwritten || step == XmrWorkflowStep::PunishLezTag17,
                "only an unrecorded submission or a punishment is driven again"
            );
            self.redrives += 1;
            self.unwritten = false;
            Ok(())
        }
    }

    fn walk_to_both_legs_locked(stub: &mut Stub) {
        stub.lez_ms = 1_000;
        assert_eq!(drive_once(stub, 1_000).unwrap(), "unchanged");
        stub.tag13 = Some(2_000);
        assert_eq!(drive_once(stub, 2_000).unwrap(), "observed");
        assert_eq!(drive_once(stub, 3_000).unwrap(), "submitted");
        assert_eq!(drive_once(stub, 4_000).unwrap(), "awaiting_observation");
        stub.observed_funding = true;
        assert_eq!(drive_once(stub, 5_000).unwrap(), "reconciled");
        assert_eq!(
            derive_status(&stub.facts().unwrap(), 5_000).phase,
            "both_legs_locked"
        );
        assert_eq!(stub.funds, 1);
    }

    #[test]
    fn the_happy_path_funds_once_and_claims_on_the_taker_authorization() {
        let mut stub = Stub::new();
        walk_to_both_legs_locked(&mut stub);
        assert_eq!(drive_once(&mut stub, 6_000).unwrap(), "unchanged");
        stub.tag14 = true;
        assert_eq!(drive_once(&mut stub, 7_000).unwrap(), "activated");
        assert_eq!(stub.activations, [XmrWorkflowBranch::Claim]);
        assert_eq!(
            derive_status(&stub.facts().unwrap(), 7_000).next_action,
            "claim_lez"
        );
        // Once a branch is selected a drive changes nothing.
        assert_eq!(drive_once(&mut stub, 8_000).unwrap(), "unchanged");
        assert_eq!((stub.funds, stub.activations.len()), (1, 1));
    }

    #[test]
    fn the_drive_that_reconciles_the_funding_claims_a_final_authorization() {
        let mut stub = Stub::new();
        stub.tag13 = Some(2_000);
        stub.lez_ms = 2_000;
        assert_eq!(drive_once(&mut stub, 2_000).unwrap(), "observed");
        assert_eq!(drive_once(&mut stub, 3_000).unwrap(), "submitted");
        stub.observed_funding = true;
        stub.tag14 = true;
        assert_eq!(drive_once(&mut stub, 4_000).unwrap(), "activated");
        assert_eq!(stub.activations, [XmrWorkflowBranch::Claim]);
        assert_eq!(
            derive_status(&stub.facts().unwrap(), 4_000).next_action,
            "claim_lez"
        );
    }

    #[test]
    fn a_late_claim_authorization_is_left_to_the_takers_refund() {
        let mut stub = Stub::new();
        walk_to_both_legs_locked(&mut stub);
        stub.tag14 = true;
        let deadline = stub.facts().unwrap().claim_deadline_ms();
        stub.lez_ms = deadline;
        assert_eq!(drive_once(&mut stub, deadline).unwrap(), "unchanged");
        assert!(stub.activations.is_empty());
        stub.lez_ms = R;
        stub.tag16 = true;
        assert_eq!(drive_once(&mut stub, R).unwrap(), "activated");
        assert_eq!(stub.activations, [XmrWorkflowBranch::Refund]);
    }

    #[test]
    fn the_takers_refund_selects_the_monero_sweep() {
        let mut stub = Stub::new();
        walk_to_both_legs_locked(&mut stub);
        stub.tag16 = true;
        // A refund cannot exist before `R`: it is not looked for.
        stub.lez_ms = R - 1;
        assert_eq!(drive_once(&mut stub, R - 1).unwrap(), "unchanged");
        stub.lez_ms = R;
        assert_eq!(drive_once(&mut stub, R).unwrap(), "activated");
        assert_eq!(
            derive_status(&stub.facts().unwrap(), R).next_action,
            "refund_monero"
        );
    }

    /// A funding sent and not recorded (its observer refuses the daemon, or
    /// misses the output) does not stop the look for the Taker's refund:
    /// from R it is found and recorded, and the drive that reconciles the
    /// funding activates the sweep back.
    #[test]
    fn an_unreconciled_funding_still_watches_for_the_takers_refund() {
        let mut stub = Stub::new();
        stub.lez_ms = 1_000;
        stub.tag13 = Some(2_000);
        assert_eq!(drive_once(&mut stub, 2_000).unwrap(), "observed");
        assert_eq!(drive_once(&mut stub, 3_000).unwrap(), "submitted");
        stub.observe_fails = true;
        assert_eq!(
            drive_once(&mut stub, 4_000).unwrap(),
            "awaiting_observation"
        );
        stub.tag16 = true;
        stub.lez_ms = R;
        assert_eq!(
            drive_once(&mut stub, R).unwrap(),
            "refund_seen_funding_unreconciled"
        );
        assert_eq!(stub.refund_scans, 1);
        assert!(stub.activations.is_empty());
        stub.observe_fails = false;
        stub.observed_funding = true;
        assert_eq!(drive_once(&mut stub, R + 1).unwrap(), "activated");
        assert_eq!(stub.activations, [XmrWorkflowBranch::Refund]);
    }

    #[test]
    fn the_maker_punishes_only_past_p_with_no_refund_to_the_tip() {
        let mut stub = Stub::new();
        walk_to_both_legs_locked(&mut stub);
        stub.lez_ms = P - 1;
        assert_eq!(drive_once(&mut stub, P - 1).unwrap(), "unchanged");
        stub.lez_ms = P;
        stub.refund_scan_to_tip = false;
        assert_eq!(drive_once(&mut stub, P).unwrap(), "unchanged");
        stub.refund_scan_to_tip = true;
        assert_eq!(drive_once(&mut stub, P).unwrap(), "activated");
        assert_eq!(stub.activations, [XmrWorkflowBranch::Punish]);
        assert_eq!(
            derive_status(&stub.facts().unwrap(), P).next_action,
            "punish_lez"
        );
    }

    #[test]
    fn monero_is_never_funded_after_the_cutoff_or_before_the_lock_is_final() {
        // No final lock: nothing is funded, and the cutoff ends the swap.
        let mut stub = Stub::new();
        stub.lez_ms = C - 1;
        assert_eq!(drive_once(&mut stub, C - 1).unwrap(), "unchanged");
        assert_eq!(drive_once(&mut stub, C).unwrap(), "unchanged");
        assert_eq!(derive_status(&stub.facts().unwrap(), C).phase, "expired");
        // A lock final within the cutoff, but the finalized clock passed it.
        let mut stub = Stub::new();
        stub.tag13 = Some(C);
        assert_eq!(drive_once(&mut stub, 1).unwrap(), "observed");
        stub.lez_ms = C;
        assert_eq!(drive_once(&mut stub, C - 1).unwrap(), "unchanged");
        // A lock final after the cutoff.
        let mut stub = Stub::new();
        stub.tag13 = Some(C + 1);
        assert_eq!(drive_once(&mut stub, 1).unwrap(), "observed");
        assert_eq!(drive_once(&mut stub, 2).unwrap(), "unchanged");
        assert_eq!(derive_status(&stub.facts().unwrap(), 2).phase, "expired");
        assert_eq!(stub.funds, 0);
    }

    #[test]
    fn a_monerod_outage_before_the_funding_keeps_it_for_a_later_drive() {
        let mut stub = Stub::new();
        stub.tag13 = Some(2_000);
        stub.lez_ms = 2_000;
        assert_eq!(drive_once(&mut stub, 2_000).unwrap(), "observed");
        // monerod down: the one invocation is not spent, the swap stays open.
        stub.monero_down = true;
        for now in [3_000, 4_000, C - 1] {
            stub.lez_ms = now;
            assert_eq!(drive_once(&mut stub, now).unwrap(), "unchanged");
            assert_eq!(stub.durable().funding, Funding::NotInvoked);
            assert_eq!(
                derive_status(&stub.facts().unwrap(), now).phase,
                "taker_lock_confirmed"
            );
        }
        // Back before the cutoff: funded once.
        stub.monero_down = false;
        assert_eq!(drive_once(&mut stub, C - 1).unwrap(), "submitted");
        assert_eq!(stub.funds, 1);
        // Down until the cutoff: never funded, and the cutoff ends the swap.
        let mut stub = Stub::new();
        stub.tag13 = Some(2_000);
        assert_eq!(drive_once(&mut stub, 2_000).unwrap(), "observed");
        stub.monero_down = true;
        stub.lez_ms = C - 1;
        assert_eq!(drive_once(&mut stub, C - 1).unwrap(), "unchanged");
        stub.monero_down = false;
        stub.lez_ms = C;
        assert_eq!(drive_once(&mut stub, C).unwrap(), "unchanged");
        assert_eq!(derive_status(&stub.facts().unwrap(), C).phase, "expired");
        assert_eq!(stub.funds, 0);
    }

    #[test]
    fn monerod_answers_only_an_http_reply() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        // A forward that accepts and closes unanswered (monerod down).
        let closer = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            drop(stream);
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 512];
            let _ = stream.read(&mut buffer);
            stream
                .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });
        assert!(!monerod_answers(&url));
        assert!(monerod_answers(&url));
        closer.join().unwrap();
        assert!(!monerod_answers("http://127.0.0.1:1/"));
        assert!(!monerod_answers("https://127.0.0.1:1/"));
    }

    #[test]
    fn a_restart_at_each_step_resumes_without_repeating_an_effect() {
        let mut stub = Stub::new();
        stub.tag13 = Some(2_000);
        stub.lez_ms = 2_000;
        assert_eq!(drive_once(&mut stub, 2_000).unwrap(), "observed");
        // A crash right after the funding consumed its invocation.
        stub.crash_after_fund = true;
        assert!(drive_once(&mut stub, 3_000).is_err());
        // Every restart from here observes; none funds again, also past `C`.
        for now in [4_000, C, R] {
            assert_eq!(drive_once(&mut stub, now).unwrap(), "awaiting_observation");
        }
        stub.observed_funding = true;
        assert_eq!(drive_once(&mut stub, R).unwrap(), "reconciled");
        stub.lez_ms = 5_000;
        stub.tag14 = true;
        assert_eq!(drive_once(&mut stub, 5_000).unwrap(), "activated");
        assert_eq!(drive_once(&mut stub, 6_000).unwrap(), "unchanged");
        assert_eq!((stub.funds, stub.activations.len()), (1, 1));
    }

    /// A Maker in `branch` whose effect's tool was stopped after it reserved
    /// its submission evidence and before it wrote it.
    fn stopped_mid_effect(branch: XmrWorkflowBranch, lez_ms: u64) -> Stub {
        let mut stub = Stub::new();
        walk_to_both_legs_locked(&mut stub);
        stub.durable().branch = Some(branch);
        stub.lez_ms = lez_ms;
        stub.unwritten = true;
        stub
    }

    fn settle(stub: &mut Stub, step: XmrWorkflowStep) -> &'static str {
        let facts = stub.facts().unwrap();
        settle_spent_effect(stub, &facts, step, [7; 32]).unwrap()
    }

    #[test]
    fn a_claim_stopped_mid_tool_reconciles_a_final_tag15_found_by_its_terms() {
        let mut stub = stopped_mid_effect(XmrWorkflowBranch::Claim, R - 1);
        stub.effect_final = true;
        assert_eq!(
            settle(&mut stub, XmrWorkflowStep::ClaimLezTag15),
            "completed"
        );
        assert_eq!((stub.observations, stub.redrives), (1, 0));
        assert_eq!(derive_status(&stub.facts().unwrap(), R).phase, "completed");
    }

    #[test]
    fn a_claim_stopped_mid_tool_is_driven_again_only_before_r() {
        let deadline = Stub::new().facts().unwrap().claim_deadline_ms();
        let mut stub = stopped_mid_effect(XmrWorkflowBranch::Claim, deadline - 1);
        assert_eq!(
            settle(&mut stub, XmrWorkflowStep::ClaimLezTag15),
            "resubmitted"
        );
        // Recorded now: observed, never driven again.
        assert_eq!(
            settle(&mut stub, XmrWorkflowStep::ClaimLezTag15),
            "awaiting_observation"
        );
        assert_eq!(stub.redrives, 1);
        // From `R` a tag 15 is invalid: nothing is driven, the refund awaited.
        let mut stub = stopped_mid_effect(XmrWorkflowBranch::Claim, R);
        assert_eq!(
            settle(&mut stub, XmrWorkflowStep::ClaimLezTag15),
            "awaiting_observation"
        );
        assert_eq!((stub.redrives, stub.activations.len()), (0, 0));
        assert_eq!(
            derive_status(&stub.facts().unwrap(), R).next_action,
            "claim_lez"
        );
    }

    /// A claim selected before a stop, resumed past the claim deadline:
    /// nothing is sent, not even by a redrive, until the chain decides at
    /// `R` (the Taker's refund) or past `P` (the punishment).
    #[test]
    fn a_claim_resumed_past_its_deadline_is_never_sent() {
        let deadline = Stub::new().facts().unwrap().claim_deadline_ms();
        for lez_ms in [deadline, R - 1] {
            let mut stub = stopped_mid_effect(XmrWorkflowBranch::Claim, lez_ms);
            let facts = stub.facts().unwrap();
            assert!(!claim_may_be_sent(&facts, lez_ms));
            assert_eq!(
                settle(&mut stub, XmrWorkflowStep::ClaimLezTag15),
                "awaiting_observation"
            );
            assert_eq!((stub.redrives, stub.activations.len()), (0, 0));
        }
        let stub = stopped_mid_effect(XmrWorkflowBranch::Claim, deadline - 1);
        assert!(claim_may_be_sent(&stub.facts().unwrap(), deadline - 1));
    }

    #[test]
    fn a_claim_stopped_mid_tool_gives_way_to_the_takers_finalized_refund() {
        // Whatever the claim step was left in: a written or unwritten
        // submission, both lead to the Monero refund sweep.
        for unwritten in [true, false] {
            let mut stub = stopped_mid_effect(XmrWorkflowBranch::Claim, R);
            stub.unwritten = unwritten;
            stub.tag16 = true;
            assert_eq!(
                settle(&mut stub, XmrWorkflowStep::ClaimLezTag15),
                "activated"
            );
            assert_eq!(stub.activations, [XmrWorkflowBranch::Refund]);
            assert_eq!(stub.redrives, 0);
            assert_eq!(
                derive_status(&stub.facts().unwrap(), R).next_action,
                "refund_monero"
            );
        }
    }

    #[test]
    fn a_claim_never_final_by_p_gives_way_to_the_punishment() {
        let mut stub = stopped_mid_effect(XmrWorkflowBranch::Claim, P);
        stub.refund_scan_to_tip = false;
        assert_eq!(
            settle(&mut stub, XmrWorkflowStep::ClaimLezTag15),
            "awaiting_observation"
        );
        stub.refund_scan_to_tip = true;
        assert_eq!(
            settle(&mut stub, XmrWorkflowStep::ClaimLezTag15),
            "activated"
        );
        assert_eq!(stub.activations, [XmrWorkflowBranch::Punish]);
        assert_eq!(
            derive_status(&stub.facts().unwrap(), P).next_action,
            "punish_lez"
        );
    }

    /// Past `P` with the refund absent to the tip, a claimed swap used to
    /// switch to the punishment whenever the claim observer said "pending",
    /// which it also says for an unavailable history or a spent page budget.
    /// The Maker's own claim must now be absent to the tip as well.
    #[test]
    fn a_claim_gives_way_to_the_punishment_only_once_proven_absent() {
        for tag15 in [Scan::Absent(false), Scan::Found(0)] {
            let mut stub = stopped_mid_effect(XmrWorkflowBranch::Claim, P);
            stub.unwritten = false;
            stub.tag15 = tag15;
            assert_eq!(
                settle(&mut stub, XmrWorkflowStep::ClaimLezTag15),
                "awaiting_observation"
            );
            assert!(stub.activations.is_empty());
        }
        let mut stub = stopped_mid_effect(XmrWorkflowBranch::Claim, P);
        stub.unwritten = false;
        assert_eq!(
            settle(&mut stub, XmrWorkflowStep::ClaimLezTag15),
            "activated"
        );
        assert_eq!(stub.activations, [XmrWorkflowBranch::Punish]);
    }

    #[test]
    fn a_punishment_stopped_mid_tool_reconciles_or_is_driven_again() {
        // Tag 17 is discovered by its terms: final, it reconciles.
        let mut stub = stopped_mid_effect(XmrWorkflowBranch::Punish, P);
        stub.effect_final = true;
        assert_eq!(
            settle(&mut stub, XmrWorkflowStep::PunishLezTag17),
            "completed"
        );
        assert_eq!(stub.redrives, 0);
        // Not final: the punishment is driven again, also once recorded (its
        // sidecar re-plans one whose nonce the Maker's owner passed without
        // it), until it is final. A Taker refund cannot follow: tag 16 is
        // invalid from `P`.
        let mut stub = stopped_mid_effect(XmrWorkflowBranch::Punish, P);
        assert_eq!(
            settle(&mut stub, XmrWorkflowStep::PunishLezTag17),
            "resubmitted"
        );
        assert!(!stub.unwritten);
        assert_eq!(
            settle(&mut stub, XmrWorkflowStep::PunishLezTag17),
            "resubmitted"
        );
        assert_eq!((stub.observations, stub.redrives), (2, 2));
        stub.effect_final = true;
        assert_eq!(
            settle(&mut stub, XmrWorkflowStep::PunishLezTag17),
            "completed"
        );
        assert_eq!((stub.observations, stub.redrives), (3, 2));
        assert!(stub.activations.is_empty());
        assert_eq!(derive_status(&stub.facts().unwrap(), P).phase, "punished");
    }

    #[test]
    fn a_refund_sweep_stopped_mid_tool_is_driven_again_and_then_observed() {
        // Unrecorded, the sweep has nothing to observe: driven again (a spent
        // shared output is never spent twice), then observed until final.
        let mut stub = stopped_mid_effect(XmrWorkflowBranch::Refund, R);
        assert_eq!(
            settle(&mut stub, XmrWorkflowStep::SweepMoneroRefund),
            "resubmitted"
        );
        assert_eq!((stub.observations, stub.redrives), (0, 1));
        assert_eq!(
            settle(&mut stub, XmrWorkflowStep::SweepMoneroRefund),
            "awaiting_observation"
        );
        stub.effect_final = true;
        assert_eq!(
            settle(&mut stub, XmrWorkflowStep::SweepMoneroRefund),
            "completed"
        );
        assert_eq!((stub.observations, stub.redrives), (2, 1));
        assert_eq!(derive_status(&stub.facts().unwrap(), P).phase, "refunded");
    }
}
