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
//! before it runs, so a crash resumes with observation, never a resend.

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
use serde::{Deserialize, Serialize};
use wait_timeout::ChildExt as _;
use xmr_reference_actor::{
    ValidatedXmrEffectExecutionV3, XMR_EFFECT_OBSERVER_RESULT_MAX_BYTES, XMR_MAKER_ACTOR_ABI_V1,
    XMR_MAKER_ACTOR_NEXT_ACTION, XMR_MAKER_ACTOR_PROGRAM_ID, XMR_MAKER_FUNDING_EVIDENCE_FILE,
    XMR_MAKER_FUNDING_RECEIPT_FILE, XMR_MONERO_FUNDING_NOT_SENT_EXIT, XmrEffectObserverStateV1,
    XmrMakerEffectSourcesV1, XmrPreparedEffectInvocationV1, activate_maker_claim_workflow_bytes,
    activate_maker_punish_workflow_bytes, activate_maker_refund_workflow_bytes,
    complete_maker_claim_signature, ingest_maker_refund_signature,
    load_validated_xmr_maker_authority_fd, load_validated_xmr_maker_effect_execution_fd,
    parse_xmr_effect_observer_result_v1, reconcile_maker_monero_funding_bytes,
};
#[cfg(feature = "test-crash-hooks")]
use zec_reference_actor::arm_test_crash_hook;
use zeroize::Zeroizing;

const EFFECT_TIMEOUT: Duration = Duration::from_secs(30);
/// A Monero effect mines its confirmations and restores a wallet.
const MONERO_EFFECT_TIMEOUT: Duration = Duration::from_mins(2);
const LEZ_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Finalized blocks one classification request scans, and pages per drive.
const SCAN_PAGE_BLOCKS: u32 = 16;
const SCAN_PAGES_PER_DRIVE: u32 = 64;
/// Blocks before the first drive's finalized tip where the search for the
/// Taker's lock starts: the Taker locks right after activation, which the
/// supervisor follows within seconds.
const TAG13_LOOKBACK_BLOCKS: u64 = 256;

/// The finalized LEZ evidence `drive` records in the evidence root.
const TAG13_FILE: &str = "tag13-funding-finalized.json";
const TAG14_FILE: &str = "tag14-finalized.json";
const TAG16_FILE: &str = "tag16-finalized.json";
const SCAN_CURSOR_FILE: &str = "maker-lez-scan.json";
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
    Refund,
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
    /// Consumes the one `FundMonero` invocation and runs the funding tool.
    fn invoke_fund(&mut self) -> Result<()>;
    /// Observes a consumed funding; true once reconciled.
    fn observe_fund(&mut self) -> Result<bool>;
    /// Activates `branch` from the recorded evidence.
    fn activate(&mut self, branch: XmrWorkflowBranch) -> Result<()>;
}

/// One drive step; returns its outcome.
fn drive_once(chains: &mut impl MakerChains, now_ms: u64) -> Result<&'static str> {
    let facts = chains.facts()?;
    if facts.branch.is_some() || derive_status(&facts, now_ms).next_action == "complete" {
        return Ok("unchanged");
    }
    match facts.funding {
        Funding::NotSent => Ok("unchanged"),
        Funding::Invoked => Ok(if chains.observe_fund()? {
            "reconciled"
        } else {
            "awaiting_observation"
        }),
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
            chains.invoke_fund()?;
            Ok("submitted")
        }
        Funding::Reconciled => {
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
            Ok("unchanged")
        }
    }
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
    let facts = read_facts(&execution)?;
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
    let status = derive_status(facts, now_ms());
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

/// The Maker's journal, evidence and the clock windows of Stage A.
fn read_facts(execution: &ValidatedXmrEffectExecutionV3) -> Result<Facts> {
    let (agreement, _) = execution.agreement_and_terms()?;
    let windows = agreement.body().windows();
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
        cutoff_ms: windows.maker_xmr_funding_cutoff_ms(),
        refund_ms: windows.refund_at_ms(),
        punish_ms: windows.punish_at_ms(),
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
    refund: Option<u64>,
}

impl ScanCursor {
    const fn get(&self, watched: Watched) -> Option<u64> {
        match watched {
            Watched::TakerLock => self.taker_lock,
            Watched::ClaimAuthorization => self.claim_authorization,
            Watched::Refund => self.refund,
        }
    }

    const fn set(&mut self, watched: Watched, height: u64) {
        match watched {
            Watched::TakerLock => self.taker_lock = Some(height),
            Watched::ClaimAuthorization => self.claim_authorization = Some(height),
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
        let (_, terms) = execution.agreement_and_terms()?;
        let (actor_lock, workflow_lock) = accept_locks(&execution)?;
        Ok(Self {
            execution,
            sources,
            terms,
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
        Ok(read_private_file(&self.evidence(SCAN_CURSOR_FILE))?
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
            (None, Watched::TakerLock) => tip.saturating_sub(TAG13_LOOKBACK_BLOCKS),
            (None, _) => {
                let lock = read_finalized(&self.evidence(TAG13_FILE))?
                    .context("the Taker's lock is not recorded")?;
                found_block(&lock)?.0.saturating_add(1)
            }
        };
        let effect = match watched {
            Watched::TakerLock => XmrNativeEffectV3::Fund,
            Watched::ClaimAuthorization => XmrNativeEffectV3::AuthorizeClaim,
            Watched::Refund => XmrNativeEffectV3::Refund,
        };
        let terms = self.terms;
        let mut height = start;
        let mut found = None;
        let mut to_tip = false;
        for _ in 0..SCAN_PAGES_PER_DRIVE {
            if height > tip {
                to_tip = true;
                break;
            }
            let blocks = u32::try_from((tip - height + 1).min(u64::from(SCAN_PAGE_BLOCKS)))?;
            let Some((client, runtime, run_id)) = &self.lez else {
                return Err(anyhow!("the swap's LEZ sidecar is not connected"));
            };
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
                    found = Some(result);
                    break;
                }
                // Uncertain says only that the escrow's state at the window's
                // end does not exclude the effect later: the window itself is
                // final and holds none, so the scan moves past it either way.
                FinalizedNativeXmrScanOutcomeV3::Absent { scanned_window, .. }
                | FinalizedNativeXmrScanOutcomeV3::Uncertain { scanned_window, .. } => {
                    height = scanned_window
                        .start_height()
                        .saturating_add(u64::from(scanned_window.max_blocks()));
                }
                FinalizedNativeXmrScanOutcomeV3::Unavailable { reason } => {
                    return Err(anyhow!("finalized LEZ history unavailable: {reason:?}"));
                }
            }
        }
        if found.is_none() {
            cursor.set(watched, height);
            replace_private(
                &self.evidence(SCAN_CURSOR_FILE),
                &serde_json::to_vec(&cursor)?,
            )?;
        }
        Ok((found, to_tip || height > tip))
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
                let lock = read_finalized(&self.evidence(TAG13_FILE))?
                    .context("the Taker's lock is not recorded")?;
                (
                    activate_maker_punish_workflow_bytes(
                        manifest,
                        authority,
                        run_id,
                        run_id,
                        &funding,
                        &receipt,
                        found_block(&lock)?.0.saturating_add(1),
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
        read_facts(&self.execution)
    }

    fn lez_clock(&mut self) -> Result<u64> {
        Ok(self.finalized_clock()?.1)
    }

    fn scan(&mut self, watched: Watched) -> Result<Scan> {
        let file = match watched {
            Watched::TakerLock => TAG13_FILE,
            Watched::ClaimAuthorization => TAG14_FILE,
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
                match run_silent(&mut command, MONERO_EFFECT_TIMEOUT) {
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
            self.run_observer(XmrWorkflowStep::FundMonero, MONERO_EFFECT_TIMEOUT)?
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
    let maker = LiveMaker::open(config_fd)?;
    let execution = &maker.execution;
    let recovery_step = selected_effect_step(execution, requested)?;
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
    let timeout = if recovery_step == XmrWorkflowStep::SweepMoneroRefund {
        MONERO_EFFECT_TIMEOUT
    } else {
        EFFECT_TIMEOUT
    };

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
        } => match maker.run_observer(recovery_step, timeout)? {
            None => false,
            Some((evidence, plan, source)) => {
                ensure!(
                    plan == tool_plan_identity_sha256,
                    "XMR Maker recovery plan changed"
                );
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
                true
            }
        },
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
            tag13_final_ms: None,
            funding: Funding::NotInvoked,
            branch: None,
            branch_done: false,
        }
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
        // A consumed funding is observed whatever the clock says; one that
        // was not sent expires.
        swap.funding = Funding::Invoked;
        assert_eq!(status(swap, P), ("taker_lock_confirmed", "fund_monero"));
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
        funds: u32,
        observed_funding: bool,
        activations: Vec<XmrWorkflowBranch>,
        crash_after_fund: bool,
    }

    impl Stub {
        fn new() -> Self {
            Self {
                facts: Some(facts()),
                refund_scan_to_tip: true,
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
                Watched::Refund if self.tag16 => Scan::Found(0),
                Watched::Refund => Scan::Absent(self.refund_scan_to_tip),
                Watched::ClaimAuthorization => Scan::Absent(true),
            })
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
            if self.observed_funding {
                self.durable().funding = Funding::Reconciled;
            }
            Ok(self.observed_funding)
        }

        fn activate(&mut self, branch: XmrWorkflowBranch) -> Result<()> {
            assert!(self.durable().branch.is_none());
            self.durable().branch = Some(branch);
            self.activations.push(branch);
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
}
