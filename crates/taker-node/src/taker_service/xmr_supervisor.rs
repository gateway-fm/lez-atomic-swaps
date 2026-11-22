//! The Taker Node's in-process supervisor of its Monero swaps (the Zcash
//! supervisor's model, `zec_supervisor.rs`, without its schedule: one bounded
//! step per swap and tick).
//!
//! After the take the LEZ lock (tag 13: initialize, then fund the escrow) is
//! automatic. The supervisor then watches the Maker's Monero output to the
//! shared address through a view-only wallet; once it is final the swap reads
//! `claim_available`. On the owner's command, before `R`, the claim publishes
//! the Taker's claim partial (tag 14); the Maker's claim (tag 15) then reveals
//! the Maker's Monero share, which with the Taker's opens the shared address,
//! and the supervisor sweeps it to the role wallet. From the agreement's
//! refund time `R` the refund (tag 16) is offered and runs on the owner's
//! command, and by itself unless the Node is configured otherwise
//! (`initiation.xmr_automatic_refund`); halfway through `[R, P)` a swap still
//! not refunded needs attention
//! (`refund_window_closing`), and from the punish time `P` it can no longer be
//! refunded (the Maker may take the LEZ). The refund's window is read from the
//! finalized LEZ time, not the host's clock, and a refund not yet final is
//! sent again, the same bytes, while that time is before `P`.
//!
//! The LEZ effects go through the swap's own sidecar, in process: the Taker's
//! effect-tool profile has no tag-13 slot, and its tag-14 tool does not exist
//! in this tree, so no schema-3 effect authority can be provisioned for it.
//!
//! What the supervisor learns is kept in `xmr-taker-progress.json` in the
//! swap directory; it is the only writer, and a swap's status is a function
//! of that record, the agreement's windows and the clock.

use std::{
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, ensure};
use lez_adaptor_role_runner::{Role, ValidatedSession, extract_verified_adaptor_secret};
use lez_bridge_adapter::XmrLezBridgeBindingV3;
use lez_bridge_client::{BridgeClient, BridgeClientConfig, SidecarCapability, XmrReleaseClient};
use lez_bridge_protocol::{
    AggregateBip340Signature, ClassifyFinalizedNativeXmrEffectV3Request,
    CompleteNativeXmrRefundV3Request, DiscoveryWindow, FinalizedNativeXmrEffectFactsV3,
    FinalizedNativeXmrScanOutcomeV3, FinalizedNativeXmrTransactionTargetV3, MessageContext,
    ObserveNativeXmrEscrowTipV3Request, Participant, PrepareNativeXmrClaimAuthorizationV3Request,
    PrepareNativeXmrEscrowV3Request, PrepareNativeXmrRefundV3Request, PreparedTransaction,
    RequestId, RuntimeDescriptor, SubmitNativeXmrClaimAuthorizationV3Request,
    SubmitTransactionRequest, XmrClaimPartialV3, XmrNativeEffectV3, XmrNativeEscrowStateV3,
    XmrNativeEscrowTermsV3,
};
use lez_btc_role_lifecycle::{
    SwapSidecar,
    layout::{read_private, replace_private},
};
use lez_swap_core::{Phase, SwapId};
use lez_swap_store::TakerFacadeActionV1;
use lez_xmr_monero_adapter::{
    ExpectedMoneroOutput, LoopbackRpcEndpoint, MoneroAddress, MoneroChainIdentityAttestor,
    MoneroOutputVerifier, MoneroRegtestMining, MoneroRegtestWalletEffects, MoneroTransactionId,
    MoneroWalletEffectError, MoneroWalletRpcLock,
};
use lez_xmr_swap_sdk::{
    CrossCurveScalar, MoneroPrivateViewKey, ReconstructedMoneroSpendKey, XmrActivatedAgreementV1,
    XmrAgreementV1,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use xmr_reference_actor::XmrTakerCeremony;
use zeroize::Zeroizing;

use super::{
    TakerServiceState,
    lifecycle::{LifecycleStatus, RouteActor},
    xmr_dynamic::{DynamicXmrRole, ROLE_DIRECTORY, SWAP_FACTS_FILE, XmrTakerSwapFactsV1},
};
use crate::{TakerSwapStateV1, TakerTerminalActionV1, acceptance_files::derived_request_id};

const PROGRESS_FILE: &str = "xmr-taker-progress.json";
const MAX_PROGRESS_BYTES: usize = 64 * 1024;
const MAX_WIRE_BYTES: usize = 300 * 1024;
const ROLE_SHARE_FILE: &str = "xmr-share.key";
/// A sweep that sent nothing is retried after this, doubling per attempt up
/// to `SWEEP_RETRY_MAX_MS`.
const SWEEP_RETRY_FIRST_MS: u64 = 15_000;
const SWEEP_RETRY_MAX_MS: u64 = 15 * 60_000;
/// Bounded finalized-history scans per effect and tick.
const SCANS_PER_TICK: usize = 8;
/// How long a Monero step waits for the role's wallet-rpc lock, and how long
/// a read-only Monero step (an observation, a re-verification, a sweep's
/// confirmation) may take in all. A public daemon or wallet-rpc that stalls
/// costs a swap's step this much at most; every swap's refund has already run
/// in the sweep before any Monero step ([`sweep_refunds_first`]).
const MONERO_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
const MONERO_STEP_BUDGET: std::time::Duration = std::time::Duration::from_mins(2);

/// One process writes progress records; the supervisor and the owner's refund
/// both update them, one read-modify-write at a time.
static PROGRESS_WRITE: Mutex<()> = Mutex::new(());

/// One finalized LEZ effect of this swap.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FinalizedLezEffectV1 {
    pub(super) transaction_id: String,
    pub(super) block_id: u64,
    /// The LEZ consensus time of the block that holds it, ms (a record an
    /// earlier Node wrote holds the finalized time it was first seen at).
    pub(super) finalized_at_ms: u64,
}

/// What the supervisor has observed and done for one swap.
#[allow(clippy::struct_excessive_bools)] // Each is one recorded fact.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct XmrTakerProgressV1 {
    pub(super) schema_version: u16,
    /// The finalized LEZ height scanning for this swap's next effect starts at.
    #[serde(default)]
    pub(super) scan_from: Option<u64>,
    #[serde(default)]
    pub(super) initialized: Option<FinalizedLezEffectV1>,
    #[serde(default)]
    pub(super) funded: Option<FinalizedLezEffectV1>,
    /// The shared view-only wallet exists on this role's wallet-rpc.
    #[serde(default)]
    pub(super) shared_wallet_restored: bool,
    /// The Maker's Monero output, verified final and unlocked.
    #[serde(default)]
    pub(super) monero_funding_transaction: Option<String>,
    /// The tag-16 refund as submitted, then as finalized.
    #[serde(default)]
    pub(super) refund_submitted: Option<String>,
    #[serde(default)]
    pub(super) refunded: Option<FinalizedLezEffectV1>,
    /// The claim authorization (tag 14) as prepared before its first
    /// submission, then as finalized.
    #[serde(default)]
    pub(super) claim_authorization: Option<String>,
    #[serde(default)]
    pub(super) claim_authorized: Option<FinalizedLezEffectV1>,
    /// The finalized LEZ height the claim's scans (tag 14, then the Maker's
    /// tag 15) resume at.
    #[serde(default)]
    pub(super) claim_scan_from: Option<u64>,
    /// The Maker's claim (tag 15) as finalized.
    #[serde(default)]
    pub(super) maker_claim: Option<XmrMakerClaimV1>,
    /// Sweeps of the shared output started, each into its own wallet file.
    #[serde(default)]
    pub(super) claim_sweep_attempts: u32,
    /// The last sweep started may have been sent and was not recorded: it is
    /// never resent.
    #[serde(default)]
    pub(super) claim_sweep_in_doubt: bool,
    /// The last sweep sent nothing; the next waits until then (host time,
    /// ms).
    #[serde(default)]
    pub(super) claim_sweep_retry_at_ms: Option<u64>,
    #[serde(default)]
    pub(super) claim_sweep: Option<XmrClaimSweepV1>,
    /// The swept output is final and unlocked in the role wallet.
    #[serde(default)]
    pub(super) claim_completed: bool,
    /// Past the funding cutoff, the lock was proven never funded: the
    /// finalized LEZ time reached R with nothing in the escrow at the tip.
    #[serde(default)]
    pub(super) lock_unfunded: bool,
    /// The Maker's punishment (tag 17), once final: past `P` a swap whose
    /// claim was authorised ends punished only on this, never on the clock
    /// alone, for the Maker may have claimed before `R` instead.
    #[serde(default)]
    pub(super) maker_punished: Option<FinalizedLezEffectV1>,
    #[serde(default)]
    pub(super) punish_scan_from: Option<u64>,
    /// The last look for the Maker's claim failed.
    #[serde(default)]
    pub(super) maker_claim_scan_failing: bool,
}

/// The Maker's finalized claim and its aggregate signature (public: it is on
/// LEZ), which reveals the Maker's Monero share.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct XmrMakerClaimV1 {
    pub(super) finalized: FinalizedLezEffectV1,
    pub(super) signature: String,
}

/// The sweep of the shared output to the role wallet, as sent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct XmrClaimSweepV1 {
    pub(super) transaction: String,
    pub(super) received_piconero: u64,
    /// Its confirmations are mined, or left to the network's miners: on
    /// Regtest the sender mines them, on Stagenet it only waits.
    #[serde(default)]
    pub(super) mined: bool,
}

/// The agreement's windows, Unix ms.
#[allow(clippy::struct_field_names)] // The unit suffix is the agreement's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct XmrWindows {
    pub(super) funding_cutoff_ms: u64,
    pub(super) refund_ms: u64,
    pub(super) punish_ms: u64,
}

impl XmrWindows {
    fn of(facts: &XmrTakerSwapFactsV1) -> Self {
        Self {
            funding_cutoff_ms: facts.maker_xmr_funding_cutoff_ms,
            refund_ms: facts.refund_at_ms,
            punish_ms: facts.punish_at_ms,
        }
    }

    /// Halfway through `[R, P)`: a swap still not refunded needs attention.
    const fn refund_attention_ms(self) -> u64 {
        self.refund_ms + (self.punish_ms.saturating_sub(self.refund_ms)) / 2
    }
}

/// One swap's progress as the desk and the action fences read it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct XmrTakerView {
    pub(super) phase: Phase,
    /// Strictly increases along every path through the table below.
    pub(super) revision: u64,
    pub(super) state: TakerSwapStateV1,
    pub(super) available_action: Option<TakerTerminalActionV1>,
    pub(super) attention_reason: Option<&'static str>,
    pub(super) actor_phase: &'static str,
}

const fn view(
    phase: Phase,
    revision: u64,
    state: TakerSwapStateV1,
    available_action: Option<TakerTerminalActionV1>,
    attention_reason: Option<&'static str>,
    actor_phase: &'static str,
) -> XmrTakerView {
    XmrTakerView {
        phase,
        revision,
        state,
        available_action,
        attention_reason,
        actor_phase,
    }
}

/// The status table: the record, the windows and the clock decide it.
#[allow(clippy::too_many_lines)] // One table, read row by row.
pub(super) fn xmr_taker_view(
    progress: &XmrTakerProgressV1,
    windows: XmrWindows,
    now_ms: u64,
) -> XmrTakerView {
    use TakerSwapStateV1 as State;
    let refund = Some(TakerTerminalActionV1::Refund);
    if progress.claim_completed {
        return view(
            Phase::Completed,
            12,
            State::Completed,
            None,
            None,
            "completed",
        );
    }
    // The Maker's claim took the LEZ: the Monero is the Taker's to sweep,
    // whatever the clock says (a claim final before R may be seen after it).
    if progress.maker_claim.is_some() {
        if progress.claim_sweep_in_doubt {
            return view(
                Phase::ClaimEvidenceAvailable,
                11,
                State::AttentionRequired,
                None,
                Some("xmr_claim_sweep_in_doubt"),
                "sweeping",
            );
        }
        return view(
            Phase::ClaimEvidenceAvailable,
            11,
            State::ClaimInProgress,
            None,
            None,
            "sweeping",
        );
    }
    if progress.refunded.is_some() {
        return view(Phase::Refunded, 9, State::Refunded, None, None, "refunded");
    }
    if progress.refund_submitted.is_some() {
        if now_ms >= windows.punish_ms {
            // Sent and not seen final by P: one that landed before P is
            // still recorded once seen final; otherwise it can no longer
            // land and the Maker may punish.
            return view(
                Phase::TakerLegRefunded,
                8,
                State::AttentionRequired,
                None,
                Some("xmr_refund_not_final_by_punish_time"),
                "refund_not_final",
            );
        }
        return view(
            Phase::TakerLegRefunded,
            8,
            State::RefundInProgress,
            None,
            None,
            "refund_in_flight",
        );
    }
    if progress.funded.is_none() {
        // Past the funding cutoff no lock is sent. One prepared before it may
        // still be final, or become final, without the Node having seen it:
        // until it is found or proven never funded the swap says so.
        if now_ms >= windows.funding_cutoff_ms
            && progress.scan_from.is_some()
            && !progress.lock_unfunded
        {
            let phase = if progress.initialized.is_some() {
                Phase::AwaitingTakerConfirmations
            } else {
                Phase::Offered
            };
            return view(
                phase,
                1,
                State::AttentionRequired,
                None,
                Some("xmr_lez_lock_unconfirmed"),
                "lez_lock_unconfirmed",
            );
        }
        // Nothing of the Taker's is locked and the take is over.
        if now_ms >= windows.funding_cutoff_ms {
            return view(
                Phase::Offered,
                1,
                State::Expired,
                None,
                None,
                "lez_lock_window_closed",
            );
        }
        let phase = if progress.initialized.is_some() {
            Phase::AwaitingTakerConfirmations
        } else {
            Phase::Offered
        };
        return view(
            phase,
            0,
            State::AwaitingFirstLock,
            None,
            None,
            "lez_locking",
        );
    }
    if now_ms >= windows.punish_ms
        && progress.claim_authorization.is_some()
        && progress.maker_punished.is_none()
    {
        // The claim was authorised: the Maker may have claimed before R (its
        // tag 15 not seen yet, the Monero then the Taker's to sweep) or may
        // punish. Until the chain shows which, the swap is not over.
        return view(
            Phase::MakerRecoveryAvailable,
            10,
            State::AttentionRequired,
            None,
            Some(if progress.maker_claim_scan_failing {
                "xmr_maker_claim_scan_failing"
            } else {
                "xmr_watching_for_maker_claim"
            }),
            "punish_open",
        );
    }
    if now_ms >= windows.punish_ms {
        // Tag 16 is valid only in [R, P): the LEZ is the Maker's to take by
        // its punishment, and nothing the Taker does changes that.
        return view(
            Phase::MakerRecoveryAvailable,
            10,
            State::Punished,
            None,
            None,
            "punish_open",
        );
    }
    if now_ms >= windows.refund_attention_ms() {
        return view(
            Phase::MakerLegRefunded,
            7,
            State::AttentionRequired,
            refund,
            Some("refund_window_closing"),
            "refund_available",
        );
    }
    if now_ms >= windows.refund_ms {
        if progress.claim_authorization.is_some() {
            // The claim was authorised (tag 14) and the Maker did not claim
            // before R; it no longer can. The escrow refunds from
            // XmrClaimAuthorized as from Funded: the Taker refunds in [R, P),
            // or the Maker punishes from P and the Monero stays locked.
            return view(
                Phase::MakerLegRefunded,
                6,
                State::AttentionRequired,
                refund,
                Some("xmr_maker_did_not_claim"),
                "refund_available",
            );
        }
        return view(
            Phase::MakerLegRefunded,
            6,
            State::RefundAvailable,
            refund,
            None,
            "refund_available",
        );
    }
    if progress.claim_authorized.is_some() {
        return view(
            Phase::BothLegsLocked,
            5,
            State::ClaimInProgress,
            None,
            None,
            "awaiting_maker_claim",
        );
    }
    if progress.claim_authorization.is_some() {
        return view(
            Phase::BothLegsLocked,
            4,
            State::ClaimInProgress,
            None,
            None,
            "claim_authorizing",
        );
    }
    if claim_locked(progress, windows) {
        return view(
            Phase::BothLegsLocked,
            3,
            State::ClaimAvailable,
            Some(TakerTerminalActionV1::Claim),
            None,
            "both_legs_locked",
        );
    }
    view(
        Phase::TakerLockConfirmed,
        2,
        State::AwaitingSecondLock,
        None,
        None,
        "taker_lock_confirmed",
    )
}

/// Both legs locked for the claim: the escrow funded within the funding
/// cutoff and the Maker's Monero output verified final and unlocked.
fn claim_locked(progress: &XmrTakerProgressV1, windows: XmrWindows) -> bool {
    progress
        .funded
        .as_ref()
        .is_some_and(|funded| funded.finalized_at_ms <= windows.funding_cutoff_ms)
        && progress.monero_funding_transaction.is_some()
}

/// One bound Monero swap's files around its receipt.
#[derive(Clone, Debug)]
pub(super) struct XmrSwapFiles {
    pub(super) swap_root: PathBuf,
    /// The receipt's published Stage A and B with their pinned digests.
    pub(super) stage_a: (PathBuf, [u8; 32]),
    pub(super) stage_b: (PathBuf, [u8; 32]),
}

/// A receipt-bound Monero swap as the lifecycle's route actor holds it.
#[derive(Debug)]
pub(crate) struct XmrBoundSwap {
    swap_id: SwapId,
    state_db: PathBuf,
    manifest_sha256: [u8; 32],
    files: XmrSwapFiles,
}

impl XmrBoundSwap {
    /// Reloads the swap from its receipt, pinning the receipt's digest and
    /// identity and confining its files to the swap's directory.
    pub(super) fn load(
        receipt: &Path,
        actor_root: &Path,
        swap_id: &SwapId,
        receipt_sha256: [u8; 32],
        device: u64,
        inode: u64,
    ) -> Result<Self> {
        let snapshot = crate::secure_file::read_private_file_snapshot(
            receipt,
            crate::acceptance_files::MAX_TAKER_RECEIPT_BYTES,
            "XMR Taker acceptance receipt",
        )?;
        let identity = snapshot.identity();
        ensure!(
            Sha256::digest(snapshot.bytes()).as_slice() == receipt_sha256
                && identity.device() == device
                && identity.inode() == inode,
            "XMR Taker acceptance receipt identity changed"
        );
        let selector = crate::xmr_taker_accept::load_xmr_taker_receipt_selector(receipt)?;
        let swap_root = actor_root
            .parent()
            .context("the actor root has no swap directory")?;
        let (stage_a, a_sha256) = selector.stage_a();
        let (stage_b, b_sha256) = selector.stage_b();
        ensure!(
            selector.swap_id() == swap_id
                && receipt.parent() == Some(swap_root)
                && stage_a.starts_with(actor_root)
                && stage_b.starts_with(actor_root)
                && selector.state_database()
                    == swap_root
                        .join(ROLE_DIRECTORY)
                        .join(xmr_reference_actor::XMR_ADAPTOR_JOURNAL_FILE),
            "XMR Taker acceptance receipt does not match service authority"
        );
        Ok(Self {
            swap_id: swap_id.clone(),
            state_db: selector.state_database().to_path_buf(),
            manifest_sha256: Sha256::digest(selector.manifest_bytes()).into(),
            files: XmrSwapFiles {
                swap_root: swap_root.to_path_buf(),
                stage_a: (stage_a.to_path_buf(), a_sha256),
                stage_b: (stage_b.to_path_buf(), b_sha256),
            },
        })
    }

    pub(super) const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    pub(super) fn state_db(&self) -> &Path {
        &self.state_db
    }

    pub(super) const fn files(&self) -> &XmrSwapFiles {
        &self.files
    }

    pub(super) fn custody_identity(&self) -> (SwapId, PathBuf, PathBuf, [u8; 32]) {
        (
            self.swap_id.clone(),
            self.state_db.clone(),
            self.files.swap_root.clone(),
            self.manifest_sha256,
        )
    }
}

/// What a Monero row adds to the generic view: its phase and, when the
/// clock or a closed window needs the owner, the reason.
pub(super) fn overlay_view(view: &mut crate::TakerSwapViewV1, files: &XmrSwapFiles) {
    // From the swap's facts, not by validating Stage A again (its proofs
    // take about a second of CPU, and this runs on every listing).
    view.monero_network = files.facts().ok().and_then(|facts| facts.monero_network);
    if let Ok(status) = files.status() {
        view.actor_phase = Some(status.actor_phase.to_owned());
        if let Some(reason) = status.attention_reason {
            view.attention_reason = Some(reason.to_owned());
        }
        match status.attention_reason {
            Some("xmr_maker_did_not_claim") => {
                view.status_detail = Some(
                    "the claim was authorised on LEZ (tag 14) and the Maker did not claim before the refund time, so it no longer can: refund (tag 16) before the punish time, or the Maker takes the LEZ and the Monero stays locked".to_owned(),
                );
            }
            Some("xmr_progress_unreadable") => {
                view.status_detail = Some(format!(
                    "the swap's progress record {} cannot be read, so the Node does not drive this swap; it is kept as it is for repair. If LEZ is locked, its refund (tag 16) is still due before the punish time",
                    files.swap_root.join(PROGRESS_FILE).display()
                ));
            }
            Some("xmr_refund_not_final_by_punish_time") => {
                view.status_detail = Some(
                    "the refund (tag 16) was sent and not seen final by the punish time: one that landed before it is recorded once seen final; otherwise it can no longer land, and the Maker can punish (tag 17) and take the LEZ".to_owned(),
                );
            }
            Some("xmr_claim_sweep_in_doubt") => {
                let attempt = files
                    .progress()
                    .map_or(0, |progress| progress.claim_sweep_attempts);
                let wallet = files.facts().map_or_else(
                    |_| "the swap's claim wallet".to_owned(),
                    |facts| claim_wallet_name(&facts.swap_id, attempt),
                );
                view.status_detail = Some(format!(
                    "a sweep of the shared Monero output was started and not recorded, so it is not resent: wallet {wallet} on the role's wallet-rpc holds the shared spend key"
                ));
            }
            _ => {}
        }
    }
}

impl XmrSwapFiles {
    fn role_root(&self) -> PathBuf {
        self.swap_root.join(ROLE_DIRECTORY)
    }

    pub(super) fn facts(&self) -> Result<XmrTakerSwapFactsV1> {
        let bytes = read_private(&self.swap_root.join(SWAP_FACTS_FILE), MAX_PROGRESS_BYTES)?;
        let facts: XmrTakerSwapFactsV1 = serde_json::from_slice(&bytes)?;
        ensure!(facts.schema_version == 1, "unsupported swap facts");
        Ok(facts)
    }

    pub(super) fn progress(&self) -> Result<XmrTakerProgressV1> {
        match read_private(&self.swap_root.join(PROGRESS_FILE), MAX_PROGRESS_BYTES) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(_) if !self.swap_root.join(PROGRESS_FILE).exists() => Ok(XmrTakerProgressV1 {
                schema_version: 1,
                ..XmrTakerProgressV1::default()
            }),
            Err(error) => Err(error),
        }
    }

    fn update(&self, edit: impl FnOnce(&mut XmrTakerProgressV1)) -> Result<XmrTakerProgressV1> {
        let _guard = PROGRESS_WRITE
            .lock()
            .map_err(|_| anyhow::anyhow!("progress lock poisoned"))?;
        let mut progress = self.progress()?;
        edit(&mut progress);
        replace_private(
            &self.swap_root.join(PROGRESS_FILE),
            &serde_json::to_vec_pretty(&progress)?,
        )?;
        Ok(progress)
    }

    /// The countersigned Stage A and the activated Stage B, opened with the
    /// swap's view key.
    fn agreement(&self) -> Result<(XmrAgreementV1, XmrActivatedAgreementV1, Zeroizing<[u8; 32]>)> {
        let pinned = |(path, sha256): &(PathBuf, [u8; 32]), label: &str| -> Result<Vec<u8>> {
            let bytes = read_private(path, MAX_WIRE_BYTES)?;
            ensure!(
                Sha256::digest(&bytes).as_slice() == sha256,
                "the swap's {label} changed"
            );
            Ok(bytes)
        };
        let agreement = XmrAgreementV1::from_wire(&pinned(&self.stage_a, "Stage A")?)
            .context("the swap's Stage A")?;
        let view_bytes =
            xmr_reference_actor::xmr_taker_contribution(&self.role_root())?.taker_private_view_key;
        let activation = XmrActivatedAgreementV1::from_wire(
            &agreement,
            &pinned(&self.stage_b, "Stage B")?,
            &view_key(&view_bytes)?,
        )
        .context("the swap's Stage B")?;
        Ok((agreement, activation, view_bytes))
    }

    /// The status a monitor answers: no chain is read. A progress record
    /// that cannot be read (cut short, say) is a row of its own: the
    /// supervisor cannot drive the swap until it is repaired, and the owner
    /// is told so rather than meeting a tick that fails in silence.
    pub(super) fn status(&self) -> Result<XmrTakerView> {
        let windows = XmrWindows::of(&self.facts()?);
        Ok(match self.progress() {
            Ok(progress) => xmr_taker_view(&progress, windows, now_ms()),
            Err(_) => view(
                Phase::Offered,
                0,
                TakerSwapStateV1::AttentionRequired,
                None,
                Some("xmr_progress_unreadable"),
                "progress_unreadable",
            ),
        })
    }
}

fn now_ms() -> u64 {
    let host = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        });
    #[cfg(feature = "test-hooks")]
    let host = host.saturating_add_signed(test_hooks::clock_offset_ms());
    host
}

/// The devnet's test hooks: files in the directory `LEZ_TEST_HOOKS_DIR`
/// names, read on every use. Built only with the `test-hooks` feature, which
/// the devnet's build enables and no release build does: a Node built without
/// it has none of this.
#[cfg(feature = "test-hooks")]
pub(super) mod test_hooks {
    use std::path::PathBuf;

    // A release build (the release workflows and packagers set
    // LEZ_RELEASE_BUILD) cannot carry the test hooks: it does not compile.
    const _: () = assert!(
        option_env!("LEZ_RELEASE_BUILD").is_none(),
        "the devnet test hooks (feature test-hooks) are never part of a release build (LEZ_RELEASE_BUILD)"
    );

    fn file(name: &str) -> Option<PathBuf> {
        std::env::var_os("LEZ_TEST_HOOKS_DIR").map(|directory| PathBuf::from(directory).join(name))
    }

    /// `clock-offset-ms`: a signed offset added to the host clock the
    /// supervisor reads, so the Node's clock runs ahead of (or behind) LEZ
    /// time.
    pub(crate) fn clock_offset_ms() -> i64 {
        file("clock-offset-ms")
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0)
    }

    /// A flag file that is present.
    pub(crate) fn flag(name: &str) -> bool {
        file(name).is_some_and(|path| path.exists())
    }
}

/// A cheating Taker, for the devnet's adversarial scenarios: with the
/// `test-hooks` feature, `cheat-refund-before-r` sends the refund (tag 16)
/// before LEZ time reaches R, and `cheat-refund-after-claim` sends it although
/// the Maker's claim (tag 15) landed. Always false without the feature.
fn cheat(name: &str) -> bool {
    #[cfg(feature = "test-hooks")]
    return test_hooks::flag(name);
    #[cfg(not(feature = "test-hooks"))]
    {
        let _ = name;
        false
    }
}

/// The swap's own sidecar as a bridge client, bound to its run.
struct SwapLez {
    client: BridgeClient,
    release: XmrReleaseClient,
    runtime: RuntimeDescriptor,
    run_id: lez_bridge_protocol::RunId,
    discovery_max_blocks: u32,
}

impl SwapLez {
    fn connect(dynamic: &DynamicXmrRole, reservation_id: &RequestId) -> Result<Self> {
        let btc = dynamic.btc();
        let runtime = btc.runtime();
        let layout = dynamic.layout(reservation_id);
        let sidecar = SwapSidecar::ensure(runtime, &layout, reservation_id)?;
        let capability = Zeroizing::new(String::from_utf8(read_private(
            sidecar.capability_file(),
            4096,
        )?)?);
        let descriptor = runtime.runtime_descriptor();
        let config = || -> Result<BridgeClientConfig> {
            Ok(BridgeClientConfig::new(
                sidecar.endpoint(),
                SidecarCapability::new(capability.trim().to_owned())
                    .context("sidecar capability")?,
                sidecar.run_id().clone(),
                descriptor.clone(),
                runtime.request_timeout(),
            ))
        };
        let client = BridgeClient::connect(config()?).context("connect the swap's LEZ sidecar")?;
        // Tag 14 has its own submission route, apart from generic submission.
        let release =
            XmrReleaseClient::connect(config()?).context("connect the swap's LEZ sidecar")?;
        Ok(Self {
            client,
            release,
            runtime: descriptor,
            run_id: sidecar.run_id().clone(),
            discovery_max_blocks: runtime.config().lez.discovery_max_blocks,
        })
    }

    fn context(&self, request_id: RequestId) -> MessageContext {
        MessageContext::new(self.run_id.clone(), request_id, Participant::Taker)
    }

    async fn submit(&self, transaction: &PreparedTransaction) -> Result<()> {
        let result = self
            .client
            .submit_transaction(SubmitTransactionRequest::new(
                self.context(transaction.transaction_id.submission_request_id()),
                self.runtime.clone(),
                transaction.clone(),
            ))
            .await
            .context("submit LEZ transaction")?;
        ensure!(
            result.transaction_id == transaction.transaction_id,
            "the sidecar submitted another transaction"
        );
        Ok(())
    }

    /// Scans finalized history from `cursor` for the exact `transaction`;
    /// returns it once found final, or the cursor to resume from.
    async fn find_final(
        &self,
        terms: &XmrNativeEscrowTermsV3,
        effect: XmrNativeEffectV3,
        transaction: &PreparedTransaction,
        cursor: u64,
    ) -> Result<std::result::Result<FinalizedLezEffectV1, u64>> {
        Ok(self
            .scan(
                terms,
                effect,
                &FinalizedNativeXmrTransactionTargetV3::exact(transaction.clone()),
                cursor,
            )
            .await?
            .map(|(finalized, _)| finalized))
    }

    /// Scans finalized history from `cursor` for the effect `target` names
    /// (exact bytes, or discovered from the terms); returns it with its facts
    /// once found final, or the cursor to resume from.
    async fn scan(
        &self,
        terms: &XmrNativeEscrowTermsV3,
        effect: XmrNativeEffectV3,
        target: &FinalizedNativeXmrTransactionTargetV3,
        mut cursor: u64,
    ) -> Result<std::result::Result<(FinalizedLezEffectV1, FinalizedNativeXmrEffectFactsV3), u64>>
    {
        // The sidecar classifies only a window that is final to its end.
        let tip = self.finalized_height().await?;
        for _ in 0..SCANS_PER_TICK {
            if cursor > tip {
                break;
            }
            let blocks =
                u32::try_from((tip - cursor + 1).min(u64::from(self.discovery_max_blocks)))?;
            let request_id = RequestId::new(hex::encode(Sha256::digest(
                format!("xmr-taker-scan/{effect:?}/{cursor}/{}", now_ms()).as_bytes(),
            )))?;
            let result = self
                .client
                .classify_finalized_native_xmr_effect_v3(
                    ClassifyFinalizedNativeXmrEffectV3Request::new(
                        self.context(request_id),
                        self.runtime.clone(),
                        *terms,
                        effect,
                        target.clone(),
                        DiscoveryWindow::new(cursor, blocks)?,
                    ),
                )
                .await
                .context("classify finalized LEZ history")?;
            match result.outcome {
                FinalizedNativeXmrScanOutcomeV3::Found { facts, .. } => {
                    if let FinalizedNativeXmrTransactionTargetV3::Exact { transaction } = target {
                        ensure!(
                            facts.transaction.transaction_id == transaction.transaction_id,
                            "the classifier found another transaction"
                        );
                    }
                    ensure!(
                        facts.instruction.effect == effect,
                        "the classifier found another effect"
                    );
                    return Ok(Ok((
                        recorded_effect(
                            hex::encode(facts.transaction.transaction_id.as_bytes()),
                            facts.containing_block,
                        ),
                        *facts,
                    )));
                }
                FinalizedNativeXmrScanOutcomeV3::Absent {
                    finalized_clock,
                    scanned_window,
                }
                | FinalizedNativeXmrScanOutcomeV3::Uncertain {
                    finalized_clock,
                    scanned_window,
                } => {
                    let next = scanned_window
                        .start_height()
                        .saturating_add(u64::from(scanned_window.max_blocks()));
                    let tip = finalized_clock.height.saturating_add(1);
                    if next >= tip {
                        return Ok(Err(tip.max(cursor)));
                    }
                    cursor = next;
                }
                FinalizedNativeXmrScanOutcomeV3::Unavailable { reason } => {
                    anyhow::bail!("finalized LEZ history unavailable: {reason:?}");
                }
            }
        }
        Ok(Err(cursor))
    }
}

/// One effect as found final: its transaction, its block, and the time it
/// counts at, its block's consensus time (not the end of the scan that found
/// it, which may be much later: the Node was down, or its scan lagged).
fn recorded_effect(
    transaction_id: String,
    block: lez_bridge_protocol::FinalizedBlockIdentity,
) -> FinalizedLezEffectV1 {
    FinalizedLezEffectV1 {
        transaction_id,
        block_id: block.block_id,
        finalized_at_ms: block.timestamp_ms,
    }
}

/// The value a Monero pair last raised the Node's LEZ lock gate to, and its
/// swap. A pair that dies (the funding cutoff passes with the escrow not
/// funded) leaves the gate ahead of the account, and every later lock, of
/// either pair, would wait for a nonce nothing sends: the swap gives it back.
static RAISED_LOCK_GATE: Mutex<Option<(String, u64)>> = Mutex::new(None);

/// Past the funding cutoff with the escrow not funded: lowers the gate to
/// the account's live nonce if it still holds what this swap's pair raised
/// it to (no lock of either pair planned since) and the account is short of
/// it. The pair's funding is never sent past the cutoff.
async fn release_lock_gate(dynamic: &DynamicXmrRole, swap_id: &SwapId) -> Result<()> {
    let raised = RAISED_LOCK_GATE
        .lock()
        .ok()
        .and_then(|raised| raised.clone())
        .filter(|(swap, _)| swap == swap_id.as_str());
    let Some((_, value)) = raised else {
        return Ok(());
    };
    let Ok(mut held) = super::btc_dynamic::LEZ_LOCK_GATE.try_lock() else {
        return Ok(());
    };
    if *held == value {
        let (_, nonce) =
            lez_btc_role_lifecycle::balances::lez_owner_sequenced_balance(dynamic.btc().runtime())
                .await?;
        *held = (*held).min(nonce);
    }
    if let Ok(mut raised) = RAISED_LOCK_GATE.lock() {
        *raised = None;
    }
    Ok(())
}

/// How long one step waits for the initialization to reach a block before
/// it sends the funding, and how often it looks (a LEZ block is a second or
/// two on the devnet).
const FUNDING_AFTER_INITIALIZATION_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
const FUNDING_AFTER_INITIALIZATION_POLL: std::time::Duration =
    std::time::Duration::from_millis(200);

/// Tag 13: prepares the escrow's initialization and funding once (the
/// sidecar replays its durable preparation), submits the exact bytes and
/// records each step once finalized, within the funding cutoff. The funding
/// goes out as soon as the initialization is in a block, not once it is
/// final: the pair is signed at consecutive nonces of the owner account the
/// Taker's swaps of the other pairs sign for at the live nonce, and a lock
/// of theirs signed in between would take the funding's nonce. Not before:
/// the sidecar refuses a funding whose initialization is not included and
/// keeps that answer for the funding's request, and the classifier needs
/// the two in separate blocks (the initialization's block must end with the
/// escrow empty).
async fn drive_lez_lock(
    dynamic: &DynamicXmrRole,
    files: &XmrSwapFiles,
    facts: &XmrTakerSwapFactsV1,
    progress: &XmrTakerProgressV1,
) -> Result<()> {
    // The pair is planned at the owner's live nonce by the first preparation;
    // it takes the Node's LEZ lock gate as a Bitcoin lock does, so that no
    // Bitcoin lock is prepared while the pair is not yet in blocks (that
    // lock is never re-planned) and the pair is not prepared while one is.
    // Busy, or the last lock not yet in a block: a later step plans it.
    let mut gate = None;
    if progress.scan_from.is_none() {
        let Ok(mut held) = super::btc_dynamic::LEZ_LOCK_GATE.try_lock() else {
            return Ok(());
        };
        let (_, nonce) =
            lez_btc_role_lifecycle::balances::lez_owner_sequenced_balance(dynamic.btc().runtime())
                .await?;
        if nonce < *held {
            return Ok(());
        }
        // Past the gate the account moved outside it: a claim, or a Zcash
        // lock, which is a pair planned at the live nonce without the gate,
        // and whose funding would take this pair's nonce once its
        // initialisation is in a block. So the pair is planned only at a
        // nonce that stood still for a step: the gate takes the live nonce
        // (which any lock waiting on it has reached) and a later step plans.
        if nonce > *held {
            *held = nonce;
            return Ok(());
        }
        gate = Some((held, nonce));
    }
    let chains = LiveLock::connect(dynamic, files, facts)?;
    let sent = lock_step(&chains, files, XmrWindows::of(facts), progress, now_ms()).await;
    if let (Some((mut held, nonce)), Ok(true)) = (gate, &sent) {
        // The next lock waits until the account has passed the pair.
        *held = nonce + 2;
        if let Ok(mut raised) = RAISED_LOCK_GATE.lock() {
            *raised = Some((facts.swap_id.as_str().to_owned(), *held));
        }
    }
    sent.map(drop)
}

/// The chain effects of the LEZ lock (tag 13), one seam so its steps run with
/// the chains stubbed: the swap's own LEZ sidecar.
#[async_trait::async_trait]
pub(super) trait XmrLockChains: Sync {
    type Transaction: Send + Sync;

    /// The initialization and the funding, prepared once (the sidecar
    /// replays its durable preparation: the same bytes on every call).
    async fn prepare_pair(&self) -> Result<(Self::Transaction, Self::Transaction)>;
    async fn finalized_height(&self) -> Result<u64>;
    async fn submit(&self, transaction: &Self::Transaction) -> Result<()>;
    /// Sends the funding once the initialization is in a block, looking for
    /// a bounded time (`submit_once_sequenced`).
    async fn submit_funding_after(
        &self,
        initialization: &Self::Transaction,
        funding: &Self::Transaction,
    ) -> Result<()>;
    async fn find_final(
        &self,
        effect: XmrNativeEffectV3,
        transaction: &Self::Transaction,
        cursor: u64,
    ) -> Result<std::result::Result<FinalizedLezEffectV1, u64>>;
    /// The finalized LEZ consensus time, ms.
    async fn lez_time_ms(&self) -> Result<u64>;
    /// The escrow's state at the sequencer tip.
    async fn escrow_state_at_tip(&self) -> Result<Option<XmrNativeEscrowStateV3>>;
}

/// One lock step; returns whether the pair's next transaction went out.
///
/// Before the funding cutoff C it sends the pair's next transaction and
/// looks for it. A send that fails, or whose answer was lost, does not stop
/// the look: what is final is found and recorded, and the next step's send
/// under the same request id is decided on the chain again by the sidecar
/// (looked up, then sent once more), not answered "unknown" from its journal. From C nothing is sent, but a
/// pair once prepared is still looked for, because a lock may land or be
/// seen final after C (the Node was down, its scan lagged, the sequencer
/// was slow; the escrow takes the funding until R), and a locked escrow the
/// Node does not know of is LEZ the Maker can punish at P. The look ends
/// once the funding is found, or once the take is proven never funded: the
/// finalized LEZ time is at or past R (no funding can land any more) and
/// the escrow at the sequencer tip holds nothing.
pub(super) async fn lock_step<C: XmrLockChains>(
    chains: &C,
    files: &XmrSwapFiles,
    windows: XmrWindows,
    progress: &XmrTakerProgressV1,
    now_ms: u64,
) -> Result<bool> {
    let past_cutoff = now_ms >= windows.funding_cutoff_ms;
    if progress.funded.is_some()
        || progress.lock_unfunded
        || (past_cutoff && progress.scan_from.is_none())
    {
        return Ok(false);
    }
    let (initialization, funding) = chains.prepare_pair().await?;
    let cursor = if let Some(cursor) = progress.scan_from {
        cursor
    } else {
        let height = chains.finalized_height().await?;
        files.update(|record| record.scan_from = Some(height))?;
        height
    };
    let (effect, transaction) = if progress.initialized.is_none() {
        (XmrNativeEffectV3::Initialize, &initialization)
    } else {
        (XmrNativeEffectV3::Fund, &funding)
    };
    let sent = if past_cutoff {
        Ok(false)
    } else {
        async {
            chains.submit(transaction).await?;
            if effect == XmrNativeEffectV3::Initialize {
                chains
                    .submit_funding_after(&initialization, &funding)
                    .await?;
            }
            Ok(true)
        }
        .await
    };
    let found = record_lock(chains, files, effect, transaction, cursor).await?;
    if found && effect == XmrNativeEffectV3::Initialize {
        // The funding is in a later block, possibly final already.
        let cursor = files.progress()?.scan_from.unwrap_or(cursor);
        record_lock(chains, files, XmrNativeEffectV3::Fund, &funding, cursor).await?;
    }
    if past_cutoff && files.progress()?.funded.is_none() {
        let unfunded = chains.lez_time_ms().await? >= windows.refund_ms
            && matches!(
                chains.escrow_state_at_tip().await?,
                None | Some(XmrNativeEscrowStateV3::Empty)
            );
        if unfunded {
            files.update(|record| record.lock_unfunded = true)?;
        }
    }
    sent
}

/// Looks for the pair's `effect` in finalized history from `cursor` and
/// records it once final, or the cursor to resume from.
async fn record_lock<C: XmrLockChains>(
    chains: &C,
    files: &XmrSwapFiles,
    effect: XmrNativeEffectV3,
    transaction: &C::Transaction,
    cursor: u64,
) -> Result<bool> {
    match chains.find_final(effect, transaction, cursor).await? {
        Ok(finalized) => {
            let next = finalized.block_id.saturating_add(1);
            files.update(|record| {
                record.scan_from = Some(next);
                if effect == XmrNativeEffectV3::Initialize {
                    record.initialized = Some(finalized);
                } else {
                    record.funded = Some(finalized);
                }
            })?;
            Ok(true)
        }
        Err(cursor) => {
            files.update(|record| record.scan_from = Some(cursor))?;
            Ok(false)
        }
    }
}

/// The lock's chain in the Node: the swap's sidecar and its sequencer.
struct LiveLock<'a> {
    dynamic: &'a DynamicXmrRole,
    facts: XmrTakerSwapFactsV1,
    lez: SwapLez,
    terms: XmrNativeEscrowTermsV3,
}

impl<'a> LiveLock<'a> {
    fn connect(
        dynamic: &'a DynamicXmrRole,
        files: &XmrSwapFiles,
        facts: &XmrTakerSwapFactsV1,
    ) -> Result<Self> {
        let (agreement, activation, _) = files.agreement()?;
        let terms = XmrLezBridgeBindingV3::new(&agreement, &activation)
            .map_err(|error| anyhow::anyhow!("derive the LEZ escrow terms: {error:?}"))?
            .terms();
        Ok(Self {
            dynamic,
            facts: facts.clone(),
            lez: SwapLez::connect(dynamic, &facts.reservation_id)?,
            terms,
        })
    }
}

#[async_trait::async_trait]
impl XmrLockChains for LiveLock<'_> {
    type Transaction = PreparedTransaction;

    async fn prepare_pair(&self) -> Result<(PreparedTransaction, PreparedTransaction)> {
        let prepared = self
            .lez
            .client
            .prepare_native_xmr_escrow_v3(PrepareNativeXmrEscrowV3Request::new(
                self.lez.context(derived_request_id(
                    &self.facts.reservation_id,
                    "xmr",
                    b"tag13-prepare",
                )?),
                self.lez.runtime.clone(),
                self.terms,
            ))
            .await
            .context("prepare the LEZ escrow")?;
        Ok((prepared.initialization, prepared.funding))
    }

    async fn finalized_height(&self) -> Result<u64> {
        self.lez.finalized_height().await
    }

    async fn submit(&self, transaction: &PreparedTransaction) -> Result<()> {
        self.lez.submit(transaction).await
    }

    async fn submit_funding_after(
        &self,
        initialization: &PreparedTransaction,
        funding: &PreparedTransaction,
    ) -> Result<()> {
        let runtime = self.dynamic.btc().runtime();
        submit_once_sequenced(
            || sequenced(runtime, initialization),
            || self.lez.submit(funding),
            FUNDING_AFTER_INITIALIZATION_WAIT,
            FUNDING_AFTER_INITIALIZATION_POLL,
        )
        .await
    }

    async fn find_final(
        &self,
        effect: XmrNativeEffectV3,
        transaction: &PreparedTransaction,
        cursor: u64,
    ) -> Result<std::result::Result<FinalizedLezEffectV1, u64>> {
        self.lez
            .find_final(&self.terms, effect, transaction, cursor)
            .await
    }

    async fn lez_time_ms(&self) -> Result<u64> {
        Ok(self.lez.finalized_clock().await?.timestamp_ms)
    }

    async fn escrow_state_at_tip(&self) -> Result<Option<XmrNativeEscrowStateV3>> {
        self.lez
            .escrow_state_at_tip(&self.facts.swap_id, &self.terms)
            .await
    }
}

/// Submits once `sequenced` says the predecessor is in a block, looking
/// every `poll` for up to `wait`; a failed look counts as not yet. Not sent
/// within `wait`, it is sent on a later step (at the latest once the
/// predecessor is final).
async fn submit_once_sequenced<S, SF, F, FF>(
    mut sequenced: S,
    submit: F,
    wait: std::time::Duration,
    poll: std::time::Duration,
) -> Result<()>
where
    S: FnMut() -> SF,
    SF: std::future::Future<Output = Result<bool>>,
    F: FnOnce() -> FF,
    FF: std::future::Future<Output = Result<()>>,
{
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        if sequenced().await.unwrap_or(false) {
            return submit().await;
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(());
        }
        tokio::time::sleep(poll).await;
    }
}

/// Whether the configured sequencer holds the exact transaction in a block
/// (the sidecar's own check before it sends a funding).
async fn sequenced(
    runtime: &lez_btc_role_lifecycle::BtcRoleRuntime,
    transaction: &PreparedTransaction,
) -> Result<bool> {
    use jsonrpsee::core::client::ClientT as _;
    let client = jsonrpsee_http_client::HttpClientBuilder::default()
        .request_timeout(runtime.request_timeout())
        .build(&runtime.config().lez.sequencer_url)
        .context("sequencer client")?;
    let found: Option<serde_json::Value> = client
        .request(
            "getTransaction",
            jsonrpsee::rpc_params![hex::encode(transaction.transaction_id.as_bytes())],
        )
        .await
        .context("sequencer getTransaction")?;
    Ok(found.is_some())
}

impl SwapLez {
    /// The escrow's state at the sequencer tip, not finalized.
    async fn escrow_state_at_tip(
        &self,
        swap_id: &SwapId,
        terms: &XmrNativeEscrowTermsV3,
    ) -> Result<Option<XmrNativeEscrowStateV3>> {
        let request_id = RequestId::new(hex::encode(Sha256::digest(
            format!("xmr-taker-escrow-tip/{}/{}", swap_id.as_str(), now_ms()).as_bytes(),
        )))?;
        Ok(self
            .client
            .observe_native_xmr_escrow_tip_v3(ObserveNativeXmrEscrowTipV3Request::new(
                self.context(request_id),
                self.runtime.clone(),
                *terms,
            ))
            .await
            .context("read the LEZ escrow at the sequencer tip")?
            .state)
    }

    async fn finalized_height(&self) -> Result<u64> {
        Ok(self.finalized_clock().await?.height)
    }

    /// The finalized LEZ clock: the latest finalized block's height and
    /// consensus timestamp.
    async fn finalized_clock(&self) -> Result<lez_bridge_protocol::ChainClock> {
        let observed = self
            .client
            .observe_finalized_clock(lez_bridge_protocol::ObserveFinalizedClockRequest {
                context: self.context(RequestId::new(hex::encode(Sha256::digest(
                    format!("xmr-taker-clock/{}", now_ms()).as_bytes(),
                )))?),
                runtime: self.runtime.clone(),
            })
            .await
            .context("observe the finalized LEZ clock")?;
        Ok(observed.clock)
    }
}

fn view_key(bytes: &Zeroizing<[u8; 32]>) -> Result<MoneroPrivateViewKey> {
    MoneroPrivateViewKey::from_monero_little_endian(**bytes).context("the swap's view key")
}

/// The shared view-only wallet's name and its (derived, stable) password.
fn shared_wallet(swap_id: &SwapId, view_key: &[u8; 32]) -> (String, Zeroizing<String>) {
    let digest = Sha256::new()
        .chain_update(b"lez-xmr-taker-shared-wallet-password/v1\0")
        .chain_update(view_key)
        .finalize();
    (
        format!("xmr-{}-view", swap_id.as_str()),
        Zeroizing::new(hex::encode(digest)),
    )
}

/// Watches the Maker's Monero output to the shared address in the swap's
/// view-only wallet, under the role's wallet-rpc lock (a blocking file lock,
/// so the whole sequence runs on a blocking thread). The verifier alone
/// decides: the output must be exact, final and unlocked.
async fn observe_monero(
    dynamic: &Arc<DynamicXmrRole>,
    files: &XmrSwapFiles,
    facts: &XmrTakerSwapFactsV1,
    progress: &XmrTakerProgressV1,
) -> Result<()> {
    let dynamic = Arc::clone(dynamic);
    let files = files.clone();
    let facts = facts.clone();
    let restored = progress.shared_wallet_restored;
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let _lock =
            MoneroWalletRpcLock::acquire_within(&dynamic.wallet_lock_file()?, MONERO_LOCK_WAIT)?;
        handle.block_on(within_monero_budget(
            MONERO_STEP_BUDGET,
            observe_monero_locked(&dynamic, &files, &facts, restored),
        ))
    })
    .await
    .context("the Monero observation thread")?
}

/// `work` within `budget`; past it the work is dropped (its connections
/// closed, the wallet-rpc lock released) and the step fails, to run again on
/// a later sweep.
async fn within_monero_budget<T>(
    budget: std::time::Duration,
    work: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::time::timeout(budget, work)
        .await
        .map_err(|_| anyhow::anyhow!("the Monero step took longer than {budget:?}"))?
}

async fn observe_monero_locked(
    dynamic: &DynamicXmrRole,
    files: &XmrSwapFiles,
    facts: &XmrTakerSwapFactsV1,
    restored: bool,
) -> Result<()> {
    let config = dynamic.config();
    let (agreement, _, view_bytes) = files.agreement()?;
    let (shared_name, shared_password) = shared_wallet(&facts.swap_id, &view_bytes);
    let monerod = config.monerod()?;
    let wallet_rpc = config.wallet_rpc()?;
    let role = MoneroRegtestWalletEffects::new(&monerod, &wallet_rpc)?.with_role_wallet(
        config.wallet_name().to_owned(),
        config.wallet_password()?.to_string(),
    )?;
    let shared = MoneroRegtestWalletEffects::new(&monerod, &wallet_rpc)?
        .with_role_wallet(shared_name.clone(), shared_password.to_string())?;
    let opened = if restored {
        shared.open_role_wallet().await.map_err(anyhow::Error::from)
    } else {
        // A crash after the wallet was made and before it was recorded leaves
        // its file: it is opened instead.
        match role
            .restore_shared_view_only(
                agreement.shared_address(),
                view_key(&view_bytes)?,
                shared_name,
                shared_password.to_string(),
                facts.monero_restore_height,
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(_) => shared.open_role_wallet().await.map_err(anyhow::Error::from),
        }
    };
    let observed = async {
        opened?;
        files.update(|record| record.shared_wallet_restored = true)?;
        shared
            .refresh_from_height(facts.monero_restore_height)
            .await?;
        let amount = agreement.body().monero().amount_piconero();
        let verifier = MoneroOutputVerifier::new(
            xmr_reference_actor::agreement_monero_identity(&agreement)?,
            &monerod,
            &wallet_rpc,
        )?;
        let Some(transaction) = verifier.find_incoming(amount).await? else {
            return Ok::<_, anyhow::Error>(());
        };
        let destination = MoneroAddress::from_str(&agreement.shared_address().address_string())
            .context("the shared address")?;
        if verifier
            .verify(&ExpectedMoneroOutput::new(
                transaction,
                destination,
                amount,
            )?)
            .await
            .is_ok()
        {
            // What lets the owner claim rests on this daemon's word: it must
            // serve the swap's network and, on a public one, be synchronized
            // with a recent tip.
            daemon_current(&agreement, &monerod).await?;
            files.update(|record| {
                record.monero_funding_transaction =
                    Some(hex::encode(AsRef::<[u8]>::as_ref(&transaction)));
            })?;
        }
        Ok(())
    }
    .await;
    // The role wallet is what every other user of this wallet-rpc expects.
    let _ = role.open_role_wallet().await;
    observed
}

/// The Maker's recorded funding, verified again in the swap's view-only
/// wallet on a daemon fit to decide on: the same exact output, final and
/// unlocked on the current chain. The caller holds the wallet-rpc lock.
async fn verify_recorded_funding(
    dynamic: &DynamicXmrRole,
    files: &XmrSwapFiles,
    facts: &XmrTakerSwapFactsV1,
) -> Result<()> {
    let progress = files.progress()?;
    let recorded = progress
        .monero_funding_transaction
        .as_deref()
        .context("no Monero funding is recorded")?;
    let mut transaction = [0_u8; 32];
    hex::decode_to_slice(recorded, &mut transaction).context("the recorded Monero funding")?;
    let config = dynamic.config();
    let (agreement, _, view_bytes) = files.agreement()?;
    let (shared_name, shared_password) = shared_wallet(&facts.swap_id, &view_bytes);
    let monerod = config.monerod()?;
    let wallet_rpc = config.wallet_rpc()?;
    daemon_current(&agreement, &monerod).await?;
    let role = MoneroRegtestWalletEffects::new(&monerod, &wallet_rpc)?.with_role_wallet(
        config.wallet_name().to_owned(),
        config.wallet_password()?.to_string(),
    )?;
    let shared = MoneroRegtestWalletEffects::new(&monerod, &wallet_rpc)?
        .with_role_wallet(shared_name, shared_password.to_string())?;
    let verified = async {
        shared.open_role_wallet().await?;
        shared
            .refresh_from_height(facts.monero_restore_height)
            .await?;
        let verifier = MoneroOutputVerifier::new(
            xmr_reference_actor::agreement_monero_identity(&agreement)?,
            &monerod,
            &wallet_rpc,
        )?;
        let destination = MoneroAddress::from_str(&agreement.shared_address().address_string())
            .context("the shared address")?;
        let _final = verifier
            .verify(&ExpectedMoneroOutput::new(
                MoneroTransactionId(transaction),
                destination,
                agreement.body().monero().amount_piconero(),
            )?)
            .await
            .context("the recorded Monero funding is no longer final and unlocked")?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    // The role wallet is what every other user of this wallet-rpc expects.
    let _ = role.open_role_wallet().await;
    verified
}

/// The role's daemon is fit to decide on now
/// ([`MoneroChainIdentityAttestor::verify_daemon_current`]): it serves the
/// swap's Monero network (a Regtest swap's daemon is the fakechain, never the
/// mainnet whose genesis it shares) and, on Stagenet, it is synchronized with
/// a tip at most an hour old.
async fn daemon_current(agreement: &XmrAgreementV1, monerod: &LoopbackRpcEndpoint) -> Result<()> {
    MoneroChainIdentityAttestor::new(
        xmr_reference_actor::agreement_monero_network(agreement),
        monerod,
    )?
    .verify_daemon_current()
    .await
    .context("the role's Monero daemon")
}

/// The Taker's own Monero spend share, little-endian.
fn role_share(files: &XmrSwapFiles) -> Result<Zeroizing<[u8; 32]>> {
    let text = Zeroizing::new(read_private(&files.role_root().join(ROLE_SHARE_FILE), 128)?);
    let mut share = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(
        std::str::from_utf8(&text)
            .context("the Monero share is not text")?
            .trim(),
        &mut *share,
    )
    .context("the Monero share is not hex")?;
    Ok(share)
}

/// The Taker's completed refund signature: its refund presignature (Stage B)
/// adapted with its own Monero share.
fn refund_signature(
    files: &XmrSwapFiles,
    agreement: &XmrAgreementV1,
    activation: &XmrActivatedAgreementV1,
) -> Result<[u8; 64]> {
    let context = agreement
        .refund_session_descriptor()
        .context()
        .context("the refund session")?;
    let share = CrossCurveScalar::from_monero_little_endian(*role_share(files)?)
        .context("the Monero share")?;
    let signature = lez_adaptor_signature::adapt_presignature(
        &context,
        activation.body().refund_presignature(),
        share.adaptor_scalar_big_endian(),
    )
    .map_err(|error| anyhow::anyhow!("adapt the refund presignature: {error:?}"))?;
    lez_adaptor_signature::verify_final_signature(&context, signature)
        .map_err(|error| anyhow::anyhow!("the refund signature: {error:?}"))?;
    Ok(signature)
}

/// The chain effects of the refund (tag 16), one seam so its steps run with
/// the chains stubbed: the swap's own LEZ sidecar.
#[async_trait::async_trait]
pub(super) trait XmrRefundChains: Sync {
    type Refund: Send + Sync;

    /// The finalized LEZ consensus time, ms (the latest finalized block's
    /// timestamp).
    async fn lez_time_ms(&self) -> Result<u64>;
    /// The escrow's state at the sequencer tip, not finalized (`None` while
    /// it is not initialized there).
    async fn escrow_state_at_tip(&self) -> Result<Option<XmrNativeEscrowStateV3>>;
    /// The completed refund as the sidecar holds it: prepared and completed
    /// once, then replayed (the same bytes on every call).
    async fn complete_refund(&self) -> Result<Self::Refund>;
    fn refund_id(refund: &Self::Refund) -> String;
    async fn submit_refund(&self, refund: &Self::Refund) -> Result<()>;
    async fn find_refund(
        &self,
        refund: &Self::Refund,
        cursor: u64,
    ) -> Result<std::result::Result<FinalizedLezEffectV1, u64>>;
}

/// One refund step (tag 16), on the chain's clock. LEZ admits a transaction
/// to its mempool and drops it at block building when its validity window is
/// not open (`docs/milestone-1/lez-primitive-verification.md`), so the window
/// is read from the finalized LEZ time, never from the host's clock: the
/// refund is first sent once the finalized LEZ time is at or past `R` (every
/// block still to come is later than the finalized one; the finality lag is
/// the margin), and while it is not final and the finalized LEZ time is
/// before `P` the identical bytes are sent again on every step. The sidecar
/// decides each of those submissions on the chain (already known once
/// included, sent again otherwise); the refund is completed once and nothing
/// is re-signed. From `P` it can no longer land and is only looked for. The
/// caller holds the swap's actor lock.
pub(super) async fn refund_step<C: XmrRefundChains>(
    chains: &C,
    files: &XmrSwapFiles,
    windows: XmrWindows,
    progress: &XmrTakerProgressV1,
) -> Result<()> {
    if progress.refunded.is_some() {
        return Ok(());
    }
    let lez_ms = chains.lez_time_ms().await?;
    if progress.refund_submitted.is_none()
        && !(windows.refund_ms <= lez_ms && lez_ms < windows.punish_ms)
        && !cheat("cheat-refund-before-r")
    {
        return Ok(());
    }
    // The completed refund signature reveals the Taker's Monero share. Sent
    // after the Maker's claim (tag 15) landed it fails on chain, but hands
    // the Maker the Taker's share while it holds its own: both would hold
    // the shared spend key and race for the Monero. So the escrow is read at
    // the sequencer tip, beyond what is final, before the refund is completed
    // or sent: a claim there (Claimed; from P also the punishment) holds the
    // refund back for good, and the claim's watch sweeps the Monero once the
    // claim is final. Our own refund there is followed, not sent again.
    //
    // What this cannot see is a tag 15 in the sequencer's mempool. It can no
    // longer land: tag 15 is valid only in blocks before R, and the refund
    // goes out only once the finalized LEZ time is at or past R, so every
    // block still to come is later. That holds while the one sequencer
    // stamps its blocks in order and keeps what it finalized; a sequencer
    // that does not (its operator, in collusion with the Maker) could include
    // a backdated tag 15 after it saw the refund in its mempool, and the
    // Maker would then race the Taker for the Monero. The same operator sees
    // the share of every refund it drops.
    let tip = chains.escrow_state_at_tip().await?;
    match tip {
        Some(XmrNativeEscrowStateV3::Claimed) if !cheat("cheat-refund-after-claim") => {
            return Ok(());
        }
        Some(
            XmrNativeEscrowStateV3::Claimed
            | XmrNativeEscrowStateV3::Funded
            | XmrNativeEscrowStateV3::ClaimAuthorized
            | XmrNativeEscrowStateV3::Refunded,
        ) => {}
        Some(XmrNativeEscrowStateV3::Empty) | None => {
            anyhow::bail!("the LEZ escrow is not funded at the sequencer tip")
        }
    }
    let refund = chains.complete_refund().await?;
    let id = C::refund_id(&refund);
    match &progress.refund_submitted {
        // From here the refund is followed, and sent until it lands or `P`.
        None => {
            files.update(|record| record.refund_submitted = Some(id))?;
        }
        Some(recorded) => ensure!(*recorded == id, "the sidecar completed another refund"),
    }
    // A failed send does not stop the look for an earlier one.
    let sent = if (lez_ms < windows.punish_ms || cheat("cheat-refund-before-r"))
        && tip != Some(XmrNativeEscrowStateV3::Refunded)
    {
        chains.submit_refund(&refund).await
    } else {
        Ok(())
    };
    let cursor = progress
        .scan_from
        .unwrap_or(progress.funded.as_ref().map_or(1, |funded| funded.block_id));
    match chains.find_refund(&refund, cursor).await? {
        Ok(finalized) => {
            files.update(|record| record.refunded = Some(finalized))?;
        }
        Err(cursor) => {
            files.update(|record| record.scan_from = Some(cursor))?;
        }
    }
    sent
}

/// The refund's chain in the Node: the swap's sidecar.
struct LiveRefund {
    files: XmrSwapFiles,
    facts: XmrTakerSwapFactsV1,
    lez: SwapLez,
    terms: XmrNativeEscrowTermsV3,
}

impl LiveRefund {
    fn connect(
        dynamic: &DynamicXmrRole,
        files: &XmrSwapFiles,
        facts: &XmrTakerSwapFactsV1,
    ) -> Result<Self> {
        let (agreement, activation, _) = files.agreement()?;
        let terms = XmrLezBridgeBindingV3::new(&agreement, &activation)
            .map_err(|error| anyhow::anyhow!("derive the LEZ escrow terms: {error:?}"))?
            .terms();
        Ok(Self {
            files: files.clone(),
            facts: facts.clone(),
            lez: SwapLez::connect(dynamic, &facts.reservation_id)?,
            terms,
        })
    }
}

#[async_trait::async_trait]
impl XmrRefundChains for LiveRefund {
    type Refund = PreparedTransaction;

    async fn lez_time_ms(&self) -> Result<u64> {
        Ok(self.lez.finalized_clock().await?.timestamp_ms)
    }

    async fn escrow_state_at_tip(&self) -> Result<Option<XmrNativeEscrowStateV3>> {
        self.lez
            .escrow_state_at_tip(&self.facts.swap_id, &self.terms)
            .await
    }

    async fn complete_refund(&self) -> Result<PreparedTransaction> {
        let (agreement, activation, _) = self.files.agreement()?;
        let prepared = self
            .lez
            .client
            .prepare_native_xmr_refund_v3(PrepareNativeXmrRefundV3Request::new(
                self.lez.context(derived_request_id(
                    &self.facts.reservation_id,
                    "xmr",
                    b"tag16-prepare",
                )?),
                self.lez.runtime.clone(),
                self.terms,
            ))
            .await
            .context("prepare the LEZ refund")?;
        let completed = self
            .lez
            .client
            .complete_native_xmr_refund_v3(
                CompleteNativeXmrRefundV3Request::new(
                    self.lez.context(derived_request_id(
                        &self.facts.reservation_id,
                        "xmr",
                        b"tag16-complete",
                    )?),
                    self.lez.runtime.clone(),
                    self.terms,
                    prepared.refund,
                    AggregateBip340Signature::from_bytes(refund_signature(
                        &self.files,
                        &agreement,
                        &activation,
                    )?),
                )
                .map_err(|error| anyhow::anyhow!("the refund completion: {error:?}"))?,
            )
            .await
            .context("complete the LEZ refund")?;
        Ok(completed.refund)
    }

    fn refund_id(refund: &PreparedTransaction) -> String {
        hex::encode(refund.transaction_id.as_bytes())
    }

    async fn submit_refund(&self, refund: &PreparedTransaction) -> Result<()> {
        self.lez.submit(refund).await
    }

    async fn find_refund(
        &self,
        refund: &PreparedTransaction,
        cursor: u64,
    ) -> Result<std::result::Result<FinalizedLezEffectV1, u64>> {
        self.lez
            .find_final(&self.terms, XmrNativeEffectV3::Refund, refund, cursor)
            .await
    }
}

/// Tag 14 publishes the Taker's claim partial: only on the owner's command,
/// once both legs are locked (the escrow funded within the funding cutoff,
/// the Maker's Monero verified final and unlocked), with no refund sent, and
/// before `R` (tag 14 is valid only in `..R`).
fn ensure_claim_authorizable(
    progress: &XmrTakerProgressV1,
    windows: XmrWindows,
    now_ms: u64,
    admitted: bool,
) -> Result<()> {
    ensure!(admitted, "the owner has not admitted the claim");
    ensure!(
        claim_locked(progress, windows) && progress.refund_submitted.is_none(),
        "the claim needs both legs locked and no refund sent"
    );
    ensure!(
        now_ms < windows.refund_ms,
        "the claim is valid only before the refund time"
    );
    Ok(())
}

/// Why a Monero sweep failed: before anything reached the wallet, or perhaps
/// after it was handed the transfer.
#[derive(Debug)]
pub(super) enum SweepFailure {
    NotSent(anyhow::Error),
    InDoubt(anyhow::Error),
}

/// The chain effects of the claim, one seam so its steps run with the chains
/// stubbed: the swap's LEZ sidecar and the role's Monero wallet-rpc.
#[async_trait::async_trait]
pub(super) trait XmrClaimChains: Sync {
    type Authorization: Send + Sync;

    /// The Maker's recorded funding output read again on the current chain:
    /// still exact, at the required depth and unlocked, on a daemon fit to
    /// decide on (TM-0034: a reorganisation or double spend after it was
    /// first seen).
    async fn funding_still_final(&self) -> Result<()>;
    /// Tag 14 as the sidecar prepares it (a replayed request: the same bytes).
    async fn prepare_authorization(&self) -> Result<Self::Authorization>;
    fn authorization_id(authorization: &Self::Authorization) -> String;
    /// Submits it. A resubmission of an accepted one is the sidecar's replay;
    /// one whose outcome is unknown (or whose sidecar stopped mid-send) is
    /// looked up and sent again, the same bytes, only before `R`.
    async fn submit_authorization(&self, authorization: &Self::Authorization) -> Result<()>;
    async fn find_authorization(
        &self,
        authorization: &Self::Authorization,
        cursor: u64,
    ) -> Result<std::result::Result<FinalizedLezEffectV1, u64>>;
    /// The Maker's claim (tag 15) once final, with the aggregate signature
    /// its facts carry.
    async fn find_maker_claim(
        &self,
        cursor: u64,
    ) -> Result<std::result::Result<(FinalizedLezEffectV1, Option<[u8; 64]>), u64>>;
    /// The Maker's punishment (tag 17) once final.
    async fn find_punishment(
        &self,
        cursor: u64,
    ) -> Result<std::result::Result<FinalizedLezEffectV1, u64>>;
    /// The signature completes the Taker's claim presignature and reveals the
    /// Maker's share, which with the Taker's opens the shared address.
    fn check_maker_claim_signature(&self, signature: [u8; 64]) -> Result<()>;
    /// Sweeps the shared output to the role wallet once, from wallet file
    /// `attempt`.
    async fn sweep(
        &self,
        signature: [u8; 64],
        attempt: u32,
    ) -> std::result::Result<XmrClaimSweepV1, SweepFailure>;
    /// Mines the sweep's confirmations on Regtest; on a public network does
    /// nothing, for its miners confirm.
    async fn mine(&self) -> Result<()>;
    /// The swept output is final and unlocked in the role wallet.
    async fn sweep_confirmed(&self, sweep: &XmrClaimSweepV1) -> Result<bool>;
}

/// One step of the claim. Each effect is journaled in the progress record
/// before or as it happens, so a crash anywhere resumes without repeating
/// one: tag 14 is recorded before its first submission and from then on only
/// submitted again under its request id (only before `R`; the sidecar sends
/// its identical bytes again only when their outcome is unknown) and followed
/// to finality; the Maker's tag 15
/// is recorded once final with a signature that opens the shared address; a
/// sweep is recorded as started before it is sent, and one started and not
/// recorded is never resent (`xmr_claim_sweep_in_doubt`).
pub(super) async fn claim_step<C: XmrClaimChains>(
    chains: &C,
    files: &XmrSwapFiles,
    windows: XmrWindows,
    progress: &XmrTakerProgressV1,
    now_ms: u64,
    admitted: bool,
) -> Result<()> {
    if progress.claim_completed {
        return Ok(());
    }
    if let Some(sweep) = &progress.claim_sweep {
        if !sweep.mined {
            chains.mine().await?;
            files.update(|record| {
                if let Some(sweep) = &mut record.claim_sweep {
                    sweep.mined = true;
                }
            })?;
        } else if chains.sweep_confirmed(sweep).await? {
            files.update(|record| record.claim_completed = true)?;
        }
        return Ok(());
    }
    if let Some(claim) = &progress.maker_claim {
        return sweep_step(chains, files, progress, claim, now_ms).await;
    }
    if let Some(authorized) = &progress.claim_authorized {
        return watch_maker_claim(chains, files, windows, progress, now_ms, authorized).await;
    }
    // Tag 14 publishes the Taker's claim partial, after which the Maker can
    // take the LEZ: the funding it rests on is read again right before it is
    // authorised and before each submission, never only the once it was seen.
    let mut funding_checked = false;
    if progress.claim_authorization.is_none() {
        ensure_claim_authorizable(progress, windows, now_ms, admitted)?;
        chains
            .funding_still_final()
            .await
            .context("the Maker's Monero funding, read again before the claim")?;
        funding_checked = true;
    }
    let authorization = chains.prepare_authorization().await?;
    let id = C::authorization_id(&authorization);
    match &progress.claim_authorization {
        None => {
            // From here the claim is followed whatever the clock says.
            files.update(|record| record.claim_authorization = Some(id))?;
        }
        Some(recorded) => ensure!(
            *recorded == id,
            "the sidecar prepared another claim authorization"
        ),
    }
    // A funding no longer there holds the submission back; what was sent
    // before is still looked for.
    let mut held_back = None;
    if now_ms < windows.refund_ms {
        let checked = if funding_checked {
            Ok(())
        } else {
            chains.funding_still_final().await
        };
        match checked {
            Ok(()) => chains.submit_authorization(&authorization).await?,
            Err(error) => {
                held_back = Some(error.context(
                    "the Maker's Monero funding, read again before the claim's submission",
                ));
            }
        }
    }
    let cursor = progress
        .claim_scan_from
        .unwrap_or_else(|| progress.funded.as_ref().map_or(1, |funded| funded.block_id));
    match chains.find_authorization(&authorization, cursor).await? {
        Ok(finalized) => {
            let block = finalized.block_id;
            files.update(|record| {
                record.claim_scan_from = Some(block);
                record.claim_authorized = Some(finalized);
            })?;
        }
        Err(cursor) => {
            files.update(|record| record.claim_scan_from = Some(cursor))?;
            let funded = progress.funded.as_ref().map_or(1, |funded| funded.block_id);
            look_for_punishment(chains, files, windows, now_ms, funded).await?;
        }
    }
    held_back.map_or(Ok(()), Err)
}

/// Tag 14 is final: looks for the Maker's claim (tag 15), and from `P` for
/// its punishment.
async fn watch_maker_claim<C: XmrClaimChains>(
    chains: &C,
    files: &XmrSwapFiles,
    windows: XmrWindows,
    progress: &XmrTakerProgressV1,
    now_ms: u64,
    authorized: &FinalizedLezEffectV1,
) -> Result<()> {
    let cursor = progress.claim_scan_from.unwrap_or(authorized.block_id);
    let found = match chains.find_maker_claim(cursor).await {
        Ok(found) => {
            if progress.maker_claim_scan_failing {
                files.update(|record| record.maker_claim_scan_failing = false)?;
            }
            found
        }
        Err(error) => {
            // Shown on the swap's row: past P it decides the ending.
            files.update(|record| record.maker_claim_scan_failing = true)?;
            return Err(error.context("look for the Maker's claim"));
        }
    };
    match found {
        Ok((finalized, signature)) => {
            let signature =
                signature.context("the finalized tag-15 facts omit the aggregate signature")?;
            chains
                .check_maker_claim_signature(signature)
                .context("the finalized tag-15 signature")?;
            files.update(|record| {
                record.maker_claim = Some(XmrMakerClaimV1 {
                    finalized,
                    signature: hex::encode(signature),
                });
            })?;
        }
        Err(cursor) => {
            files.update(|record| record.claim_scan_from = Some(cursor))?;
            look_for_punishment(chains, files, windows, now_ms, authorized.block_id).await?;
        }
    }
    Ok(())
}

/// From `P`, with the Maker's claim not found, looks for its punishment: tag
/// 15 and tag 17 exclude each other on the escrow, and only tag 17 ends an
/// authorised claim as punished.
async fn look_for_punishment<C: XmrClaimChains>(
    chains: &C,
    files: &XmrSwapFiles,
    windows: XmrWindows,
    now_ms: u64,
    from: u64,
) -> Result<()> {
    let progress = files.progress()?;
    if now_ms < windows.punish_ms || progress.maker_punished.is_some() {
        return Ok(());
    }
    match chains
        .find_punishment(progress.punish_scan_from.unwrap_or(from))
        .await?
    {
        Ok(finalized) => files.update(|record| record.maker_punished = Some(finalized))?,
        Err(cursor) => files.update(|record| record.punish_scan_from = Some(cursor))?,
    };
    Ok(())
}

/// The sweep of one claim: sent once from a fresh wallet file, recorded as
/// started before it is sent; one that sent nothing is retried after its
/// back-off.
async fn sweep_step<C: XmrClaimChains>(
    chains: &C,
    files: &XmrSwapFiles,
    progress: &XmrTakerProgressV1,
    claim: &XmrMakerClaimV1,
    now_ms: u64,
) -> Result<()> {
    if progress.claim_sweep_in_doubt
        || progress
            .claim_sweep_retry_at_ms
            .is_some_and(|retry_at| now_ms < retry_at)
    {
        return Ok(());
    }
    let signature = signature_bytes(&claim.signature)?;
    let attempt = progress.claim_sweep_attempts.saturating_add(1);
    files.update(|record| {
        record.claim_sweep_attempts = attempt;
        record.claim_sweep_in_doubt = true;
    })?;
    return match chains.sweep(signature, attempt).await {
        Ok(sweep) => {
            files.update(|record| {
                record.claim_sweep = Some(sweep);
                record.claim_sweep_in_doubt = false;
                record.claim_sweep_retry_at_ms = None;
            })?;
            Ok(())
        }
        // Nothing was sent: retried, never given up, after a back-off
        // that doubles per attempt up to a cap, so a sweep that cannot
        // go out yet is no hot loop and each attempt's wallet file is
        // not made every tick.
        Err(SweepFailure::NotSent(error)) => {
            let wait = SWEEP_RETRY_FIRST_MS
                .saturating_mul(1 << attempt.saturating_sub(1).min(16))
                .min(SWEEP_RETRY_MAX_MS);
            files.update(|record| {
                record.claim_sweep_in_doubt = false;
                record.claim_sweep_retry_at_ms = Some(now_ms.saturating_add(wait));
            })?;
            Err(error)
        }
        Err(SweepFailure::InDoubt(error)) => Err(error),
    };
}

fn signature_bytes(text: &str) -> Result<[u8; 64]> {
    let mut signature = [0_u8; 64];
    hex::decode_to_slice(text, &mut signature).context("the recorded tag-15 signature")?;
    Ok(signature)
}

/// The shared spend key the Maker's claim signature reveals: the signature
/// completes the Taker's durable claim presignature, the difference is the
/// Maker's adaptor secret (its Monero share, checked against its DLEQ proof),
/// and with the Taker's share it must open the agreed shared address.
fn maker_claim_key(
    files: &XmrSwapFiles,
    agreement: &XmrAgreementV1,
    signature: [u8; 64],
) -> Result<ReconstructedMoneroSpendKey> {
    let session = ValidatedSession::from_untweaked_context(
        agreement
            .claim_session_descriptor()
            .context()
            .context("the claim session")?,
    )
    .context("the claim session")?;
    let revealed = extract_verified_adaptor_secret(
        &files
            .role_root()
            .join(xmr_reference_actor::XMR_ADAPTOR_JOURNAL_FILE),
        &session,
        Role::Taker,
        signature,
    )
    .context("the Maker's claim signature does not complete the claim presignature")?;
    ReconstructedMoneroSpendKey::reconstruct(
        agreement.shared_address(),
        agreement.maker_proof(),
        CrossCurveScalar::from_monero_little_endian(*role_share(files)?)
            .context("the Monero share")?,
        revealed.into_big_endian_bytes(),
    )
    .context("the revealed share does not open the shared address")
}

/// The wallet file a sweep restores the shared output into, one per attempt
/// (a file left by a failed attempt is never reused), and its password,
/// derived from the Taker's own share (which the Maker never learns).
fn claim_wallet_name(swap_id: &SwapId, attempt: u32) -> String {
    format!("xmr-{}-claim-{attempt}", swap_id.as_str())
}

fn claim_wallet_password(share: &[u8; 32]) -> Zeroizing<String> {
    let digest = Sha256::new()
        .chain_update(b"lez-xmr-taker-claim-wallet-password/v1\0")
        .chain_update(share)
        .finalize();
    Zeroizing::new(hex::encode(digest))
}

/// Whether a sweep error proves the transfer never reached the wallet: the
/// adapter failed before `sweep_single`
/// ([`MoneroWalletEffectError::sweep_not_sent`]). Anything else may have been
/// sent.
fn sweep_not_sent(error: &MoneroWalletEffectError) -> bool {
    error.sweep_not_sent()
}

/// Runs `work` on a blocking thread under the role's wallet-rpc lock (a
/// blocking file lock, waited for at most [`MONERO_LOCK_WAIT`]), as the
/// Monero observation does; within `budget` when one is given. A sweep has
/// none: a sweep cut off after its transfer would read as never sent.
async fn under_wallet_lock<T, F, Fut>(
    dynamic: &Arc<DynamicXmrRole>,
    budget: Option<std::time::Duration>,
    work: F,
) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T>>,
{
    let lock_file = dynamic.wallet_lock_file()?;
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let _lock = MoneroWalletRpcLock::acquire_within(&lock_file, MONERO_LOCK_WAIT)?;
        match budget {
            Some(budget) => handle.block_on(within_monero_budget(budget, work())),
            None => handle.block_on(work()),
        }
    })
    .await
    .context("the Monero wallet thread")?
}

/// The claim's chains in the Node: the swap's sidecar and the role's
/// wallet-rpc.
struct LiveClaim {
    dynamic: Arc<DynamicXmrRole>,
    files: XmrSwapFiles,
    facts: XmrTakerSwapFactsV1,
    lez: SwapLez,
    terms: XmrNativeEscrowTermsV3,
}

impl LiveClaim {
    fn connect(
        dynamic: &Arc<DynamicXmrRole>,
        files: &XmrSwapFiles,
        facts: &XmrTakerSwapFactsV1,
    ) -> Result<Self> {
        let (agreement, activation, _) = files.agreement()?;
        let terms = XmrLezBridgeBindingV3::new(&agreement, &activation)
            .map_err(|error| anyhow::anyhow!("derive the LEZ escrow terms: {error:?}"))?
            .terms();
        Ok(Self {
            dynamic: Arc::clone(dynamic),
            files: files.clone(),
            facts: facts.clone(),
            lez: SwapLez::connect(dynamic, &facts.reservation_id)?,
            terms,
        })
    }

    /// The role wallet, reopened before each effect.
    fn role_wallet(dynamic: &DynamicXmrRole) -> Result<MoneroRegtestWalletEffects> {
        let config = dynamic.config();
        Ok(
            MoneroRegtestWalletEffects::new(&config.monerod()?, &config.wallet_rpc()?)?
                .with_role_wallet(
                    config.wallet_name().to_owned(),
                    config.wallet_password()?.to_string(),
                )?,
        )
    }
}

#[async_trait::async_trait]
impl XmrClaimChains for LiveClaim {
    type Authorization = PreparedTransaction;

    async fn funding_still_final(&self) -> Result<()> {
        let dynamic = Arc::clone(&self.dynamic);
        let files = self.files.clone();
        let facts = self.facts.clone();
        under_wallet_lock(
            &self.dynamic,
            Some(MONERO_STEP_BUDGET),
            move || async move { verify_recorded_funding(&dynamic, &files, &facts).await },
        )
        .await
    }

    async fn prepare_authorization(&self) -> Result<PreparedTransaction> {
        let (agreement, activation, _) = self.files.agreement()?;
        let partial = XmrTakerCeremony::open(&self.files.role_root())?.claim_partial()?;
        activation
            .verify_published_taker_claim_partial(&agreement, *partial)
            .context("the claim partial is not the one Stage B commits")?;
        let prepared = self
            .lez
            .client
            .prepare_native_xmr_claim_authorization_v3(
                PrepareNativeXmrClaimAuthorizationV3Request::new(
                    self.lez.context(derived_request_id(
                        &self.facts.reservation_id,
                        "xmr",
                        b"tag14-prepare",
                    )?),
                    self.lez.runtime.clone(),
                    self.terms,
                    XmrClaimPartialV3::new(*partial)
                        .map_err(|error| anyhow::anyhow!("the claim partial: {error:?}"))?,
                ),
            )
            .await
            .context("prepare the LEZ claim authorization")?;
        Ok(prepared.authorization)
    }

    fn authorization_id(authorization: &PreparedTransaction) -> String {
        hex::encode(authorization.transaction_id.as_bytes())
    }

    async fn submit_authorization(&self, authorization: &PreparedTransaction) -> Result<()> {
        let _admitted = self
            .lez
            .release
            .submit_native_xmr_claim_authorization_v3(
                SubmitNativeXmrClaimAuthorizationV3Request::new(
                    self.lez
                        .context(authorization.transaction_id.submission_request_id()),
                    self.lez.runtime.clone(),
                    self.terms,
                    authorization.clone(),
                ),
            )
            .await
            .context("submit the LEZ claim authorization")?;
        Ok(())
    }

    async fn find_authorization(
        &self,
        authorization: &PreparedTransaction,
        cursor: u64,
    ) -> Result<std::result::Result<FinalizedLezEffectV1, u64>> {
        self.lez
            .find_final(
                &self.terms,
                XmrNativeEffectV3::AuthorizeClaim,
                authorization,
                cursor,
            )
            .await
    }

    async fn find_maker_claim(
        &self,
        cursor: u64,
    ) -> Result<std::result::Result<(FinalizedLezEffectV1, Option<[u8; 64]>), u64>> {
        Ok(self
            .lez
            .scan(
                &self.terms,
                XmrNativeEffectV3::Claim,
                &FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {},
                cursor,
            )
            .await?
            .map(|(finalized, facts)| {
                let signature = facts.aggregate_signature.map(|value| *value.as_bytes());
                (finalized, signature)
            }))
    }

    async fn find_punishment(
        &self,
        cursor: u64,
    ) -> Result<std::result::Result<FinalizedLezEffectV1, u64>> {
        Ok(self
            .lez
            .scan(
                &self.terms,
                XmrNativeEffectV3::Punish,
                &FinalizedNativeXmrTransactionTargetV3::DiscoverByTerms {},
                cursor,
            )
            .await?
            .map(|(finalized, _)| finalized))
    }

    fn check_maker_claim_signature(&self, signature: [u8; 64]) -> Result<()> {
        let (agreement, _, _) = self.files.agreement()?;
        maker_claim_key(&self.files, &agreement, signature).map(drop)
    }

    async fn sweep(
        &self,
        signature: [u8; 64],
        attempt: u32,
    ) -> std::result::Result<XmrClaimSweepV1, SweepFailure> {
        let dynamic = Arc::clone(&self.dynamic);
        let files = self.files.clone();
        let facts = self.facts.clone();
        let swept = under_wallet_lock(&self.dynamic, None, move || async move {
            let (agreement, _, view_bytes) = files.agreement()?;
            daemon_current(&agreement, &dynamic.config().monerod()?).await?;
            let key = maker_claim_key(&files, &agreement, signature)?;
            let role = LiveClaim::role_wallet(&dynamic)?;
            let destination = role.primary_standard_address().await?;
            ensure!(
                xmr_reference_actor::agreement_monero_network(&agreement)
                    .admits_address(&destination),
                "the role wallet's address is not on the swap's Monero network"
            );
            let swept = role
                .restore_shared_and_sweep_once(
                    agreement.shared_address(),
                    key,
                    view_key(&view_bytes)?,
                    claim_wallet_name(&facts.swap_id, attempt),
                    claim_wallet_password(&*role_share(&files)?).to_string(),
                    facts.monero_restore_height,
                    agreement.body().monero().amount_piconero(),
                    destination,
                )
                .await;
            // The role wallet is what every other user of this wallet-rpc
            // expects.
            let _ = role.open_role_wallet().await;
            Ok::<_, anyhow::Error>(swept)
        })
        .await;
        match swept {
            Ok(Ok(sweep)) => Ok(XmrClaimSweepV1 {
                transaction: hex::encode(sweep.transaction_id().0),
                received_piconero: sweep.received_amount_piconero(),
                mined: false,
            }),
            Ok(Err(error)) if sweep_not_sent(&error) => Err(SweepFailure::NotSent(error.into())),
            Ok(Err(error)) => Err(SweepFailure::InDoubt(error.into())),
            // Nothing reached the wallet: the key, the lock or the role wallet failed.
            Err(error) => Err(SweepFailure::NotSent(error)),
        }
    }

    async fn mine(&self) -> Result<()> {
        let dynamic = Arc::clone(&self.dynamic);
        let files = self.files.clone();
        under_wallet_lock(
            &self.dynamic,
            Some(MONERO_STEP_BUDGET),
            move || async move {
                let (agreement, _, _) = files.agreement()?;
                let config = dynamic.config();
                // Regtest only: on a public network no mining capability exists,
                // and the sweep's confirmations are its miners'.
                let Some(mining) = MoneroRegtestMining::where_the_role_mines(
                    xmr_reference_actor::agreement_monero_identity(&agreement)?,
                    &config.monerod()?,
                )
                .await?
                else {
                    return Ok(());
                };
                let role = LiveClaim::role_wallet(&dynamic)?;
                role.mine_confirmations(&mining, role.primary_standard_address().await?)
                    .await?;
                Ok(())
            },
        )
        .await
    }

    async fn sweep_confirmed(&self, sweep: &XmrClaimSweepV1) -> Result<bool> {
        let dynamic = Arc::clone(&self.dynamic);
        let files = self.files.clone();
        let facts = self.facts.clone();
        let sweep = sweep.clone();
        under_wallet_lock(
            &self.dynamic,
            Some(MONERO_STEP_BUDGET),
            move || async move {
                let (agreement, _, _) = files.agreement()?;
                let config = dynamic.config();
                let role = LiveClaim::role_wallet(&dynamic)?;
                let destination =
                    MoneroAddress::from_str(&role.primary_standard_address().await?.to_string())
                        .context("the role wallet's address")?;
                role.refresh_from_height(facts.monero_restore_height)
                    .await?;
                let mut transaction = [0_u8; 32];
                hex::decode_to_slice(&sweep.transaction, &mut transaction)
                    .context("the recorded sweep")?;
                let verifier = MoneroOutputVerifier::new(
                    xmr_reference_actor::agreement_monero_identity(&agreement)?,
                    &config.monerod()?,
                    &config.wallet_rpc()?,
                )?;
                Ok(verifier
                    .verify(&ExpectedMoneroOutput::new(
                        MoneroTransactionId(transaction),
                        destination,
                        sweep.received_piconero,
                    )?)
                    .await
                    .is_ok())
            },
        )
        .await
    }
}

/// Whether the swap has a LEZ transaction of the Taker's owner account
/// prepared and not yet final while it can still land: tag 13 (its
/// initialisation and funding are prepared together) until the funding is
/// final, and tag 14. Each is planned at the account's nonce when it is
/// prepared, so another swap's prepare meanwhile would take the same nonce.
/// Tag 16 is not one: the swap's own refund authority signs it, at that
/// account's nonce, which no other transaction uses.
fn lez_in_flight(progress: &XmrTakerProgressV1, windows: XmrWindows, now_ms: u64) -> bool {
    (progress.scan_from.is_some()
        && progress.funded.is_none()
        && now_ms < windows.funding_cutoff_ms)
        || (progress.claim_authorization.is_some()
            && progress.claim_authorized.is_none()
            && now_ms < windows.refund_ms)
}

/// One supervision step of one swap, by its status. While another of the
/// Taker's Monero swaps has an owner transaction in flight (`owner_busy`),
/// the swap prepares no new one of its own: it waits for the next sweep.
/// `refunded`: its refund step already ran in this sweep's first pass.
async fn tick_swap(
    dynamic: &Arc<DynamicXmrRole>,
    files: &XmrSwapFiles,
    admitted: Option<TakerFacadeActionV1>,
    owner_busy: bool,
    refunded: bool,
) -> Result<()> {
    let facts = files.facts()?;
    let progress = files.progress()?;
    let now = now_ms();
    let status = xmr_taker_view(&progress, XmrWindows::of(&facts), now);
    let settle = || async {
        let claim = LiveClaim::connect(dynamic, files, &facts)?;
        let refund = LiveRefund::connect(dynamic, files, &facts)?;
        settle_step(
            &claim,
            &refund,
            files,
            XmrWindows::of(&facts),
            now,
            admitted,
            dynamic.automatic_refund(),
            owner_busy,
            refunded,
        )
        .await
    };
    match status.revision {
        0 if owner_busy && progress.scan_from.is_none() => Ok(()),
        0 => drive_lez_lock(dynamic, files, &facts, &progress).await,
        1 => {
            let released = release_lock_gate(dynamic, &facts.swap_id).await;
            if progress.scan_from.is_some() && !progress.lock_unfunded {
                drive_lez_lock(dynamic, files, &facts, &progress)
                    .await
                    .and(released)
            } else {
                released
            }
        }
        // Waiting for the Maker's Monero is no reason not to refund from R.
        2 if dynamic.automatic_refund() => {
            let observed = observe_monero(dynamic, files, &facts, &progress).await;
            observed.and(settle().await)
        }
        2 => observe_monero(dynamic, files, &facts, &progress).await,
        3..=8 | 10 | 11 => settle().await,
        12 if cheat("cheat-refund-after-claim") => settle().await,
        _ => Ok(()),
    }
}

/// Whether this tick runs the refund step: the escrow is funded and neither
/// refunded nor claimed by the Maker (a recorded tag 15 means the Monero is
/// swept instead), and the Node refunds by itself (`automatic`), the owner
/// admitted the refund, or one is already sent (it is driven until it is
/// final). By itself it runs on every funded row, also before the host's R:
/// the refund step sends nothing before the finalized LEZ time reaches R. So
/// a Taker Node that runs keeps its LEZ whether or not the Maker funded and
/// whether or not its owner acts (TM-0031). It never waits for another
/// swap's owner transaction: the swap's own refund authority signs it.
fn refund_runs(
    progress: &XmrTakerProgressV1,
    revision: u64,
    refund_admitted: bool,
    automatic: bool,
) -> bool {
    let row = if automatic {
        matches!(revision, 2..=8 | 10)
    } else {
        matches!(revision, 6..=8 | 10)
    } || cheat("cheat-refund-after-claim");
    row && progress.funded.is_some()
        && (progress.maker_claim.is_none() || cheat("cheat-refund-after-claim"))
        && progress.refunded.is_none()
        && (automatic || refund_admitted || progress.refund_submitted.is_some())
}

/// The claim and the refund of one tick, by the swap's status row. Tag 14
/// goes out only on the owner's command and is followed after; once it is
/// out the Maker's claim is watched past R as well (a tag 15 final before R
/// may be seen only afterwards). The refund runs after that watch, in the
/// same tick, and never once the Maker's claim is recorded.
#[allow(clippy::too_many_arguments)] // The row's inputs, one each.
async fn settle_step<CC: XmrClaimChains, RC: XmrRefundChains>(
    claim: &CC,
    refund: &RC,
    files: &XmrSwapFiles,
    windows: XmrWindows,
    now_ms: u64,
    admitted: Option<TakerFacadeActionV1>,
    automatic: bool,
    owner_busy: bool,
    refund_ran: bool,
) -> Result<()> {
    let progress = files.progress()?;
    let revision = xmr_taker_view(&progress, windows, now_ms).revision;
    let claim_admitted = admitted == Some(TakerFacadeActionV1::Claim);
    let claimed = match revision {
        3 if claim_admitted && owner_busy => Ok(()),
        3 if claim_admitted => claim_step(claim, files, windows, &progress, now_ms, true).await,
        4 | 5 | 11 => claim_step(claim, files, windows, &progress, now_ms, claim_admitted).await,
        6..=8 | 10 if progress.claim_authorization.is_some() => {
            claim_step(claim, files, windows, &progress, now_ms, claim_admitted).await
        }
        _ => Ok(()),
    };
    let progress = files.progress()?;
    if refund_ran
        || !refund_runs(
            &progress,
            revision,
            admitted == Some(TakerFacadeActionV1::Refund),
            automatic,
        )
    {
        return claimed;
    }
    let refunded = refund_step(refund, files, windows, &progress).await;
    claimed.and(refunded)
}

/// Runs the supervisor beside the Node's other loops, one pass per sweep.
pub(super) fn spawn_xmr_supervisor(state: Arc<TakerServiceState>) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        eprintln!("taker XMR supervisor: no async runtime; Monero swaps are not driven");
        return;
    };
    handle.spawn(async move {
        loop {
            Box::pin(supervise_once(&state)).await;
            tokio::time::sleep(state.observer_sweep_interval).await;
        }
    });
}

async fn supervise_once(state: &TakerServiceState) {
    let Some(initiation) = state.initiation.clone() else {
        return;
    };
    let (swaps, dynamic) = {
        let Ok(context) = initiation.lock() else {
            return;
        };
        (context.xmr_bound_swaps(), context.dynamic_xmr())
    };
    let Some(dynamic) = dynamic else {
        return;
    };
    // The swaps with an owner transaction in flight, kept current through
    // the sweep: all of them sign for the Taker's one LEZ account.
    let mut in_flight = std::collections::BTreeSet::new();
    for prepared in &swaps {
        if let Some(Ok(true)) = swap_in_flight(prepared).await {
            in_flight.insert(prepared.swap_id().as_str().to_owned());
        }
    }
    let sweep = LiveSweep {
        initiation: &initiation,
        dynamic,
        in_flight: Mutex::new(in_flight),
    };
    sweep_refunds_first(&sweep, &swaps).await;
}

/// The two passes of one sweep over the bound swaps, one seam so their order
/// is tested with the swaps stubbed.
#[async_trait::async_trait]
pub(super) trait XmrSweep: Sync {
    type Swap: Sync;
    /// The refund step alone, on LEZ alone, of a swap whose refund runs now
    /// (`refund_runs`); whether it ran.
    async fn refund_first(&self, swap: &Self::Swap) -> bool;
    /// The swap's whole tick: its Monero steps and claim, and its refund
    /// unless `refunded` (it ran in the first pass).
    async fn tick(&self, swap: &Self::Swap, refunded: bool);
}

/// One sweep: every swap's refund first, then every swap's tick. From `R`
/// the refund is the step with a deadline (tag 16 lands only before `P`),
/// and a swap's Monero reads may each take the full time-outs of a public
/// daemon that stalls: so no refund waits behind any Monero I/O, its own
/// swap's or another's.
pub(super) async fn sweep_refunds_first<S: XmrSweep>(sweep: &S, swaps: &[S::Swap]) {
    let mut refunded = Vec::with_capacity(swaps.len());
    for swap in swaps {
        refunded.push(sweep.refund_first(swap).await);
    }
    for (swap, refunded) in swaps.iter().zip(refunded) {
        sweep.tick(swap, refunded).await;
    }
}

/// The sweep in the Node.
struct LiveSweep<'a> {
    initiation: &'a Arc<Mutex<crate::ConfiguredTakerInitiationContext>>,
    dynamic: Arc<DynamicXmrRole>,
    in_flight: Mutex<std::collections::BTreeSet<String>>,
}

impl LiveSweep<'_> {
    async fn admitted(
        &self,
        prepared: &crate::PreparedTakerInitiationV1,
    ) -> Option<TakerFacadeActionV1> {
        match super::lifecycle::lookup_admitted_action_for_swap(self.initiation, prepared.swap_id())
            .await
        {
            Ok(Some(admission)) => Some(admission.action()),
            _ => None,
        }
    }

    /// The swap's route actor, under its actor lock when `lock` says so for
    /// its progress (the refund and the claim read the adaptor material:
    /// they run under that lock, as an owner's action does).
    async fn load(
        &self,
        prepared: &crate::PreparedTakerInitiationV1,
        lock: impl FnOnce(&XmrTakerProgressV1) -> bool + Send + 'static,
    ) -> Option<(RouteActor, Option<lez_swap_store::ActorHeldLock>)> {
        let binding = prepared.execution().receipt_binding()?;
        let receipt = prepared.execution().receipt_output().to_path_buf();
        let actor_root = prepared.execution().actor_root().to_path_buf();
        let swap_id = prepared.swap_id().clone();
        let loaded = tokio::task::spawn_blocking(move || {
            let actor = RouteActor::load_for_monitor(
                lez_swap_core::Pair::Monero,
                &receipt,
                &actor_root,
                &swap_id,
                binding.sha256(),
                binding.identity().device(),
                binding.identity().inode(),
            )?;
            let locked = actor
                .xmr_files()
                .and_then(|files| files.progress().ok())
                .is_some_and(|progress| lock(&progress));
            let held = if locked {
                Some(lez_swap_store::ActorHeldLock::acquire_for(
                    actor.swap_id(),
                    actor.state_db(),
                )?)
            } else {
                None
            };
            Ok::<_, anyhow::Error>((actor, held))
        })
        .await;
        match loaded {
            Ok(Ok(loaded)) => Some(loaded),
            _ => None,
        }
    }
}

fn report_step_failure(swap_id: &str, error: &anyhow::Error) {
    eprintln!(
        "{}",
        serde_json::json!({
            "event": "taker_xmr_supervisor_step_failed",
            "swap_id": swap_id,
            "error": format!("{error:#}"),
        })
    );
}

#[async_trait::async_trait]
impl XmrSweep for LiveSweep<'_> {
    type Swap = crate::PreparedTakerInitiationV1;

    async fn refund_first(&self, prepared: &crate::PreparedTakerInitiationV1) -> bool {
        let admitted = self.admitted(prepared).await;
        let automatic = self.dynamic.automatic_refund();
        let refund_admitted = admitted == Some(TakerFacadeActionV1::Refund);
        let runs = move |progress: &XmrTakerProgressV1, windows: XmrWindows| {
            let revision = xmr_taker_view(progress, windows, now_ms()).revision;
            refund_runs(progress, revision, refund_admitted, automatic)
        };
        let Some((actor, held)) = self
            .load(prepared, move |progress| {
                // The windows are read again below; any refund row is locked.
                progress.funded.is_some() && progress.refunded.is_none()
            })
            .await
        else {
            return false;
        };
        let Some(files) = actor.xmr_files() else {
            return false;
        };
        let (Ok(facts), Ok(progress)) = (files.facts(), files.progress()) else {
            return false;
        };
        let windows = XmrWindows::of(&facts);
        if held.is_none() || !runs(&progress, windows) {
            return false;
        }
        let refunded = async {
            let refund = LiveRefund::connect(&self.dynamic, files, &facts)?;
            refund_step(&refund, files, windows, &progress).await
        }
        .await;
        if let Err(error) = refunded {
            report_step_failure(prepared.swap_id().as_str(), &error);
        }
        true
    }

    async fn tick(&self, prepared: &crate::PreparedTakerInitiationV1, refunded: bool) {
        let admitted = self.admitted(prepared).await;
        let automatic = self.dynamic.automatic_refund();
        let owner = admitted.is_some();
        let Some((actor, _held)) = self
            .load(prepared, move |progress| {
                owner || drives_adaptor_material(progress, automatic)
            })
            .await
        else {
            return;
        };
        let Some(files) = actor.xmr_files() else {
            return;
        };
        let swap_id = prepared.swap_id().as_str().to_owned();
        let owner_busy = self
            .in_flight
            .lock()
            .is_ok_and(|in_flight| in_flight.iter().any(|swap| *swap != swap_id));
        let ticked = tick_swap(&self.dynamic, files, admitted, owner_busy, refunded).await;
        let now = now_ms();
        if let (Ok(progress), Ok(facts), Ok(mut in_flight)) =
            (files.progress(), files.facts(), self.in_flight.lock())
        {
            if lez_in_flight(&progress, XmrWindows::of(&facts), now) {
                in_flight.insert(swap_id.clone());
            } else {
                in_flight.remove(&swap_id);
            }
        }
        if let Err(error) = ticked {
            report_step_failure(&swap_id, &error);
        }
    }
}

/// Whether the supervisor's next step of the swap may read its adaptor
/// material without an owner's action: a claim it follows, a refund it sends
/// until it is final, or, when the Node refunds by itself, any funded escrow
/// neither refunded nor claimed by the Maker.
fn drives_adaptor_material(progress: &XmrTakerProgressV1, automatic_refund: bool) -> bool {
    (progress.claim_authorization.is_some() && !progress.claim_completed)
        || (progress.refund_submitted.is_some() && progress.refunded.is_none())
        || (automatic_refund
            && progress.funded.is_some()
            && progress.refunded.is_none()
            && progress.maker_claim.is_none())
}

/// Whether a bound swap has an owner transaction in flight, read from its
/// progress record without the swap's actor lock; `None` when it cannot be
/// loaded.
async fn swap_in_flight(prepared: &crate::PreparedTakerInitiationV1) -> Option<Result<bool>> {
    let binding = prepared.execution().receipt_binding()?;
    let receipt = prepared.execution().receipt_output().to_path_buf();
    let actor_root = prepared.execution().actor_root().to_path_buf();
    let swap_id = prepared.swap_id().clone();
    tokio::task::spawn_blocking(move || {
        let actor = RouteActor::load_for_monitor(
            lez_swap_core::Pair::Monero,
            &receipt,
            &actor_root,
            &swap_id,
            binding.sha256(),
            binding.identity().device(),
            binding.identity().inode(),
        )?;
        let files = actor.xmr_files().context("not a Monero swap")?;
        let facts = files.facts()?;
        Ok(lez_in_flight(
            &files.progress()?,
            XmrWindows::of(&facts),
            now_ms(),
        ))
    })
    .await
    .ok()
}

/// The status a monitor answers for a Monero swap.
pub(super) fn lifecycle_status(files: &XmrSwapFiles) -> Result<LifecycleStatus> {
    let view = files.status()?;
    Ok(LifecycleStatus::Active {
        phase: view.phase,
        revision: view.revision,
        state: view.state,
        available_action: view.available_action,
        privacy_guidance: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS: XmrWindows = XmrWindows {
        funding_cutoff_ms: 260_000,
        refund_ms: 360_000,
        punish_ms: 480_000,
    };

    /// Each RPC failure a sweep can meet before it sends proves it was not
    /// sent, so it is retried: the address read of a reopened shared wallet
    /// and the read of its outputs were classified as in doubt.
    #[test]
    fn a_sweep_that_failed_before_sending_is_not_sent() {
        let rpc = |operation| MoneroWalletEffectError::Rpc {
            operation,
            cause: "down".to_owned(),
        };
        for operation in [
            "restored wallet primary address",
            "read restored wallet outputs",
            "refresh restored wallet",
            "generate shared wallet from keys",
        ] {
            assert!(super::sweep_not_sent(&rpc(operation)), "{operation}");
        }
        assert!(!super::sweep_not_sent(&rpc("sweep reconstructed wallet")));
    }

    #[allow(clippy::unnecessary_wraps)] // Every use sets an optional field.
    fn effect(finalized_at_ms: u64) -> Option<FinalizedLezEffectV1> {
        Some(FinalizedLezEffectV1 {
            transaction_id: "ab".repeat(32),
            block_id: 7,
            finalized_at_ms,
        })
    }

    fn record() -> XmrTakerProgressV1 {
        XmrTakerProgressV1 {
            schema_version: 1,
            ..XmrTakerProgressV1::default()
        }
    }

    /// An owner transaction is in flight from its prepare until it is final,
    /// and only while it can still land.
    #[test]
    fn an_owner_transaction_is_in_flight_from_its_prepare_to_its_finality() {
        let mut progress = record();
        assert!(!lez_in_flight(&progress, WINDOWS, 1));
        progress.scan_from = Some(3);
        assert!(lez_in_flight(&progress, WINDOWS, 1));
        assert!(!lez_in_flight(
            &progress,
            WINDOWS,
            WINDOWS.funding_cutoff_ms
        ));
        progress.initialized = effect(10_000);
        assert!(lez_in_flight(&progress, WINDOWS, 1));
        let mut progress = locked();
        progress.scan_from = Some(3);
        assert!(!lez_in_flight(&progress, WINDOWS, 1));
        progress.claim_authorization = Some("ef".repeat(32));
        assert!(lez_in_flight(&progress, WINDOWS, 1));
        assert!(!lez_in_flight(&progress, WINDOWS, WINDOWS.refund_ms));
        progress.claim_authorized = effect(30_000);
        assert!(!lez_in_flight(&progress, WINDOWS, 1));
        let mut progress = locked();
        // The refund is the swap's own refund authority's, not the owner's.
        progress.refund_submitted = Some("01".repeat(32));
        assert!(!lez_in_flight(&progress, WINDOWS, WINDOWS.refund_ms));
    }

    /// The design's status table, row by row (M4 nodes design §4.1).
    #[test]
    fn the_status_table_follows_the_record_and_the_clock() {
        use TakerSwapStateV1 as State;
        let mut progress = record();
        let at = |progress: &XmrTakerProgressV1, now| xmr_taker_view(progress, WINDOWS, now);
        assert_eq!(at(&progress, 1_000).state, State::AwaitingFirstLock);
        assert_eq!(at(&progress, 1_000).phase, Phase::Offered);
        progress.initialized = effect(10_000);
        assert_eq!(
            at(&progress, 11_000).phase,
            Phase::AwaitingTakerConfirmations
        );
        progress.funded = effect(20_000);
        assert_eq!(at(&progress, 21_000).state, State::AwaitingSecondLock);
        progress.monero_funding_transaction = Some("cd".repeat(32));
        let claimable = at(&progress, 100_000);
        assert_eq!(claimable.state, State::ClaimAvailable);
        assert_eq!(
            claimable.available_action,
            Some(TakerTerminalActionV1::Claim)
        );
        let refundable = at(&progress, WINDOWS.refund_ms);
        assert_eq!(refundable.state, State::RefundAvailable);
        assert_eq!(
            refundable.available_action,
            Some(TakerTerminalActionV1::Refund)
        );
        let closing = at(&progress, WINDOWS.refund_attention_ms());
        assert_eq!(closing.state, State::AttentionRequired);
        assert_eq!(closing.attention_reason, Some("refund_window_closing"));
        assert_eq!(
            closing.available_action,
            Some(TakerTerminalActionV1::Refund)
        );
        // From P the LEZ is the Maker's to take: terminal, nothing offered,
        // nothing to attend to.
        let punished = at(&progress, WINDOWS.punish_ms);
        assert_eq!(punished.state, State::Punished);
        assert!(punished.state.is_terminal());
        assert_eq!(punished.attention_reason, None);
        assert_eq!(punished.available_action, None);
        progress.refund_submitted = Some("ef".repeat(32));
        assert_eq!(
            at(&progress, WINDOWS.refund_ms).state,
            State::RefundInProgress
        );
        progress.refunded = effect(400_000);
        assert_eq!(at(&progress, WINDOWS.punish_ms).state, State::Refunded);
    }

    /// A lock that misses the funding cutoff locks nothing (no funding goes
    /// out after it), and a funding final after it is never claimable: the
    /// Maker does not fund, the Taker refunds from `R`.
    #[test]
    fn the_funding_cutoff_bounds_the_lock_and_the_claim() {
        use TakerSwapStateV1 as State;
        let mut progress = record();
        progress.initialized = effect(250_000);
        // Proven never funded past the cutoff (or never sent): the take
        // expired, a terminal state.
        progress.lock_unfunded = true;
        let missed = xmr_taker_view(&progress, WINDOWS, WINDOWS.funding_cutoff_ms);
        assert_eq!(missed.state, State::Expired);
        assert!(missed.state.is_terminal());
        assert_eq!(missed.attention_reason, None);
        progress.lock_unfunded = false;
        progress.funded = effect(WINDOWS.funding_cutoff_ms + 1);
        progress.monero_funding_transaction = Some("cd".repeat(32));
        assert_eq!(
            xmr_taker_view(&progress, WINDOWS, 300_000).state,
            State::AwaitingSecondLock
        );
        assert_eq!(
            xmr_taker_view(&progress, WINDOWS, WINDOWS.refund_ms).state,
            State::RefundAvailable
        );
    }

    /// Runs `submit_once_sequenced` against the answers of the sequencer
    /// look, in order (then "not in a block"); returns how many looks it
    /// made, how many funding submissions, and its result.
    async fn sequence(looks: Vec<Result<bool>>, submitted: Result<()>) -> (usize, u32, Result<()>) {
        let looks = std::cell::RefCell::new(looks.into_iter());
        let made = std::cell::Cell::new(0);
        let sent = std::cell::Cell::new(0);
        let result = submit_once_sequenced(
            || {
                made.set(made.get() + 1);
                let look = looks.borrow_mut().next().unwrap_or(Ok(false));
                async move { look }
            },
            || {
                sent.set(sent.get() + 1);
                async move { submitted }
            },
            std::time::Duration::from_millis(50),
            std::time::Duration::from_millis(1),
        )
        .await;
        (made.get(), sent.get(), result)
    }

    /// The funding goes out on the first look that finds the initialization
    /// in a block (a failed look counts as not yet), once, and its failure is
    /// the step's.
    #[tokio::test]
    async fn the_funding_goes_out_once_the_initialization_is_in_a_block() {
        let (looks, sent, result) = sequence(
            vec![Ok(false), Err(anyhow::anyhow!("sequencer down")), Ok(true)],
            Ok(()),
        )
        .await;
        assert_eq!((looks, sent), (3, 1));
        result.expect("funding submitted");
        let (_, sent, result) =
            sequence(vec![Ok(true)], Err(anyhow::anyhow!("sidecar refused"))).await;
        assert_eq!(sent, 1);
        assert!(result.is_err());
    }

    /// While the initialization is not in a block nothing is sent: the step
    /// gives up after its wait and leaves the funding to a later step.
    #[tokio::test]
    async fn no_funding_goes_out_before_the_initialization_is_in_a_block() {
        let (looks, sent, result) = sequence(Vec::new(), Ok(())).await;
        assert!(looks > 1);
        assert_eq!(sent, 0);
        result.expect("a step without the funding is no failure");
    }

    /// The revision only grows along every path, so an owner's action admitted
    /// at one generation is never matched by an earlier state.
    #[test]
    fn the_revision_grows_along_every_path() {
        let mut progress = record();
        let mut last = 0;
        let mut step = |progress: &XmrTakerProgressV1, now| {
            let revision = xmr_taker_view(progress, WINDOWS, now).revision;
            assert!(revision >= last, "{revision} after {last}");
            last = revision;
        };
        step(&progress, 0);
        progress.initialized = effect(1);
        step(&progress, 2);
        progress.funded = effect(3);
        step(&progress, 4);
        progress.monero_funding_transaction = Some("cd".repeat(32));
        step(&progress, 5);
        for now in [
            WINDOWS.refund_ms,
            WINDOWS.refund_attention_ms(),
            WINDOWS.punish_ms - 1,
        ] {
            step(&progress, now);
        }
        progress.refund_submitted = Some("ef".repeat(32));
        step(&progress, WINDOWS.punish_ms);
        progress.refunded = effect(WINDOWS.punish_ms);
        step(&progress, WINDOWS.punish_ms + 1);

        // The claim path, and its branches into the refund and past P.
        let claimable = || {
            let mut progress = record();
            progress.funded = effect(3);
            progress.monero_funding_transaction = Some("cd".repeat(32));
            progress
        };
        for (maker_claims_at, seen_at) in [
            (Some(200_000), 200_001),
            (Some(WINDOWS.refund_ms - 1), WINDOWS.refund_attention_ms()),
            (Some(WINDOWS.refund_ms - 1), WINDOWS.punish_ms + 1),
            (None, WINDOWS.punish_ms + 1),
        ] {
            let mut progress = claimable();
            last = 0;
            let mut step = |progress: &XmrTakerProgressV1, now| {
                let revision = xmr_taker_view(progress, WINDOWS, now).revision;
                assert!(revision >= last, "{revision} after {last}");
                last = revision;
            };
            step(&progress, 100_000);
            progress.claim_authorization = Some("ab".repeat(32));
            step(&progress, 100_001);
            progress.claim_authorized = effect(110_000);
            step(&progress, 110_001);
            step(&progress, WINDOWS.refund_ms);
            if let Some(at) = maker_claims_at {
                progress.maker_claim = Some(maker_claim(at));
                step(&progress, seen_at);
                progress.claim_sweep = Some(sweep());
                progress.claim_completed = true;
                step(&progress, seen_at + 1);
            } else {
                step(&progress, seen_at);
            }
        }
    }

    fn maker_claim(finalized_at_ms: u64) -> XmrMakerClaimV1 {
        XmrMakerClaimV1 {
            finalized: effect(finalized_at_ms).unwrap(),
            signature: "11".repeat(64),
        }
    }

    fn sweep() -> XmrClaimSweepV1 {
        XmrClaimSweepV1 {
            transaction: "22".repeat(32),
            received_piconero: 999,
            mined: false,
        }
    }

    /// The claim path's rows (actor phases `claim_authorizing`,
    /// `awaiting_maker_claim`, `sweeping`, `completed`), and the Taker's
    /// position when the Maker does not claim after tag 14.
    #[test]
    fn the_claim_path_reads_in_the_status_table() {
        use TakerSwapStateV1 as State;
        let mut progress = record();
        progress.funded = effect(20_000);
        progress.monero_funding_transaction = Some("cd".repeat(32));
        progress.claim_authorization = Some("ab".repeat(32));
        let at = |progress: &XmrTakerProgressV1, now| xmr_taker_view(progress, WINDOWS, now);
        let authorizing = at(&progress, 100_000);
        assert_eq!(
            (authorizing.state, authorizing.actor_phase),
            (State::ClaimInProgress, "claim_authorizing")
        );
        assert_eq!(authorizing.available_action, None);
        progress.claim_authorized = effect(110_000);
        let waiting = at(&progress, 120_000);
        assert_eq!(
            (waiting.state, waiting.actor_phase),
            (State::ClaimInProgress, "awaiting_maker_claim")
        );
        // The Maker did not claim before R: the refund is the Taker's way
        // back, and the row says why it needs the owner.
        let unclaimed = at(&progress, WINDOWS.refund_ms);
        assert_eq!(unclaimed.state, State::AttentionRequired);
        assert_eq!(unclaimed.attention_reason, Some("xmr_maker_did_not_claim"));
        assert_eq!(
            unclaimed.available_action,
            Some(TakerTerminalActionV1::Refund)
        );
        // From P it is over only once the chain shows the punishment.
        let watching = at(&progress, WINDOWS.punish_ms);
        assert_eq!(watching.state, State::AttentionRequired);
        assert_eq!(
            watching.attention_reason,
            Some("xmr_watching_for_maker_claim")
        );
        let mut punished = progress.clone();
        punished.maker_punished = effect(WINDOWS.punish_ms + 5);
        assert_eq!(at(&punished, WINDOWS.punish_ms).state, State::Punished);
        // A claim final before R and seen after P is still swept.
        progress.maker_claim = Some(maker_claim(WINDOWS.refund_ms - 1));
        for now in [120_000, WINDOWS.refund_ms, WINDOWS.punish_ms + 1] {
            let sweeping = at(&progress, now);
            assert_eq!(
                (sweeping.state, sweeping.actor_phase),
                (State::ClaimInProgress, "sweeping")
            );
        }
        progress.claim_sweep_in_doubt = true;
        let in_doubt = at(&progress, 130_000);
        assert_eq!(in_doubt.state, State::AttentionRequired);
        assert_eq!(in_doubt.attention_reason, Some("xmr_claim_sweep_in_doubt"));
        progress.claim_sweep_in_doubt = false;
        progress.claim_sweep = Some(sweep());
        progress.claim_completed = true;
        let completed = at(&progress, WINDOWS.punish_ms + 1);
        assert_eq!(
            (completed.state, completed.actor_phase),
            (State::Completed, "completed")
        );
    }

    /// The chains, stubbed: every call is counted, each answer is set.
    #[derive(Default)]
    struct StubChains {
        calls: Mutex<std::collections::BTreeMap<&'static str, u32>>,
        authorization_final: Mutex<bool>,
        /// `Some(signature)` once the Maker's claim is final (whose facts may
        /// lack the signature).
        #[allow(clippy::option_option)]
        maker_claim: Mutex<Option<Option<[u8; 64]>>>,
        signature_invalid: bool,
        /// The next sweep fails: `Some(true)` before anything was sent.
        sweep_fails: Mutex<Option<bool>>,
        sweep_attempts: Mutex<Vec<u32>>,
        confirmed: Mutex<bool>,
        /// The recorded funding is gone from the chain (reorganised out or
        /// double spent) when read again.
        funding_gone: Mutex<bool>,
        /// The look for the Maker's claim fails (its sidecar is down).
        claim_scan_fails: Mutex<bool>,
        /// The Maker's punishment is final.
        punished: Mutex<bool>,
    }

    impl StubChains {
        fn called(&self, name: &'static str) {
            *self.calls.lock().unwrap().entry(name).or_default() += 1;
        }

        fn calls(&self, name: &str) -> u32 {
            self.calls.lock().unwrap().get(name).copied().unwrap_or(0)
        }
    }

    #[async_trait::async_trait]
    impl XmrClaimChains for StubChains {
        type Authorization = String;

        async fn funding_still_final(&self) -> Result<()> {
            self.called("funding_still_final");
            ensure!(!*self.funding_gone.lock().unwrap(), "the funding is gone");
            Ok(())
        }

        async fn prepare_authorization(&self) -> Result<String> {
            self.called("prepare");
            Ok("ab".repeat(32))
        }

        fn authorization_id(authorization: &String) -> String {
            authorization.clone()
        }

        async fn submit_authorization(&self, _: &String) -> Result<()> {
            self.called("submit");
            Ok(())
        }

        async fn find_authorization(
            &self,
            _: &String,
            cursor: u64,
        ) -> Result<std::result::Result<FinalizedLezEffectV1, u64>> {
            self.called("find_authorization");
            Ok(if *self.authorization_final.lock().unwrap() {
                Ok(effect(110_000).unwrap())
            } else {
                Err(cursor + 1)
            })
        }

        async fn find_maker_claim(
            &self,
            cursor: u64,
        ) -> Result<std::result::Result<(FinalizedLezEffectV1, Option<[u8; 64]>), u64>> {
            self.called("find_maker_claim");
            ensure!(
                !*self.claim_scan_fails.lock().unwrap(),
                "the sidecar is down"
            );
            Ok(match *self.maker_claim.lock().unwrap() {
                Some(signature) => Ok((effect(130_000).unwrap(), signature)),
                None => Err(cursor + 1),
            })
        }

        async fn find_punishment(
            &self,
            cursor: u64,
        ) -> Result<std::result::Result<FinalizedLezEffectV1, u64>> {
            self.called("find_punishment");
            Ok(if *self.punished.lock().unwrap() {
                Ok(effect(WINDOWS.punish_ms + 5).unwrap())
            } else {
                Err(cursor + 1)
            })
        }

        fn check_maker_claim_signature(&self, _: [u8; 64]) -> Result<()> {
            ensure!(!self.signature_invalid, "the signature opens nothing");
            Ok(())
        }

        async fn sweep(
            &self,
            _: [u8; 64],
            attempt: u32,
        ) -> std::result::Result<XmrClaimSweepV1, SweepFailure> {
            self.called("sweep");
            self.sweep_attempts.lock().unwrap().push(attempt);
            match self.sweep_fails.lock().unwrap().take() {
                Some(true) => Err(SweepFailure::NotSent(anyhow::anyhow!("wallet-rpc down"))),
                Some(false) => Err(SweepFailure::InDoubt(anyhow::anyhow!("sweep timed out"))),
                None => Ok(sweep()),
            }
        }

        async fn mine(&self) -> Result<()> {
            self.called("mine");
            Ok(())
        }

        async fn sweep_confirmed(&self, _: &XmrClaimSweepV1) -> Result<bool> {
            self.called("confirm");
            Ok(*self.confirmed.lock().unwrap())
        }
    }

    /// A swap directory holding only the progress record.
    fn swap(progress: &XmrTakerProgressV1) -> (tempfile::TempDir, XmrSwapFiles) {
        let directory = tempfile::TempDir::new().unwrap();
        let swap_root = std::fs::canonicalize(directory.path()).unwrap();
        let files = XmrSwapFiles {
            stage_a: (swap_root.join("stage-a"), [0; 32]),
            stage_b: (swap_root.join("stage-b"), [0; 32]),
            swap_root,
        };
        let progress = progress.clone();
        files.update(|record| *record = progress).unwrap();
        (directory, files)
    }

    fn locked() -> XmrTakerProgressV1 {
        let mut progress = record();
        progress.funded = effect(20_000);
        progress.monero_funding_transaction = Some("cd".repeat(32));
        progress
    }

    async fn step(
        chains: &StubChains,
        files: &XmrSwapFiles,
        now: u64,
        admitted: bool,
    ) -> Result<()> {
        claim_step(
            chains,
            files,
            WINDOWS,
            &files.progress().unwrap(),
            now,
            admitted,
        )
        .await
    }

    fn actor_phase(files: &XmrSwapFiles, now: u64) -> &'static str {
        xmr_taker_view(&files.progress().unwrap(), WINDOWS, now).actor_phase
    }

    /// The claim on the owner's command, one step at a time: tag 14 is
    /// recorded before it is first submitted and only replayed after, the
    /// Maker's tag 15 is recorded with its signature, the sweep is recorded,
    /// mined and confirmed, and a completed claim does nothing more.
    #[tokio::test]
    async fn the_claim_runs_step_by_step_and_records_each_effect() {
        let chains = StubChains::default();
        let (_directory, files) = swap(&locked());
        let now = 100_000;
        step(&chains, &files, now, true).await.unwrap();
        assert_eq!(
            files.progress().unwrap().claim_authorization,
            Some("ab".repeat(32))
        );
        assert_eq!(actor_phase(&files, now), "claim_authorizing");
        assert_eq!((chains.calls("prepare"), chains.calls("submit")), (1, 1));

        *chains.authorization_final.lock().unwrap() = true;
        step(&chains, &files, now, true).await.unwrap();
        let progress = files.progress().unwrap();
        assert_eq!(progress.claim_authorized, effect(110_000));
        assert_eq!(progress.claim_scan_from, Some(7));
        assert_eq!(actor_phase(&files, now), "awaiting_maker_claim");

        step(&chains, &files, now, true).await.unwrap();
        assert_eq!(files.progress().unwrap().claim_scan_from, Some(8));
        assert_eq!(actor_phase(&files, now), "awaiting_maker_claim");

        *chains.maker_claim.lock().unwrap() = Some(Some([0x11; 64]));
        step(&chains, &files, now, true).await.unwrap();
        assert_eq!(
            files.progress().unwrap().maker_claim,
            Some(XmrMakerClaimV1 {
                finalized: effect(130_000).unwrap(),
                signature: "11".repeat(64),
            })
        );
        assert_eq!(actor_phase(&files, now), "sweeping");

        step(&chains, &files, now, true).await.unwrap();
        let progress = files.progress().unwrap();
        assert_eq!(progress.claim_sweep, Some(sweep()));
        assert_eq!(progress.claim_sweep_attempts, 1);
        assert!(!progress.claim_sweep_in_doubt);
        step(&chains, &files, now, true).await.unwrap();
        assert!(files.progress().unwrap().claim_sweep.unwrap().mined);
        step(&chains, &files, now, true).await.unwrap();
        assert!(!files.progress().unwrap().claim_completed);
        *chains.confirmed.lock().unwrap() = true;
        step(&chains, &files, now, true).await.unwrap();
        assert!(files.progress().unwrap().claim_completed);
        assert_eq!(actor_phase(&files, now), "completed");

        let before = chains.calls.lock().unwrap().clone();
        step(&chains, &files, now, true).await.unwrap();
        assert_eq!(*chains.calls.lock().unwrap(), before);
        assert_eq!(
            (
                chains.calls("submit"),
                chains.calls("sweep"),
                chains.calls("mine")
            ),
            (2, 1, 1),
            "tag 14 replayed until final; one sweep, mined once"
        );
    }

    /// A Node restarted at any recorded point picks up the claim where it
    /// stands and repeats no effect: past R tag 14 is only followed, never
    /// submitted, and needs no admission; a sweep started and not recorded
    /// is never resent; one that failed before reaching the wallet is retried
    /// from a fresh wallet file.
    #[tokio::test]
    async fn a_restart_at_each_step_resumes_without_repeating_an_effect() {
        // Tag 14 recorded, then the Node restarts after R: it is followed.
        let mut progress = locked();
        progress.claim_authorization = Some("ab".repeat(32));
        let (_directory, files) = swap(&progress);
        let chains = StubChains::default();
        step(&chains, &files, WINDOWS.refund_ms, false)
            .await
            .unwrap();
        assert_eq!(
            (chains.calls("submit"), chains.calls("find_authorization")),
            (0, 1)
        );
        *chains.authorization_final.lock().unwrap() = true;
        step(&chains, &files, WINDOWS.punish_ms, false)
            .await
            .unwrap();
        assert!(files.progress().unwrap().claim_authorized.is_some());
        assert_eq!(chains.calls("submit"), 0);

        // Tag 14 final: only the Maker's claim is looked for.
        let mut progress = locked();
        progress.claim_authorization = Some("ab".repeat(32));
        progress.claim_authorized = effect(110_000);
        let (_directory, files) = swap(&progress);
        let chains = StubChains::default();
        step(&chains, &files, 120_000, false).await.unwrap();
        assert_eq!(
            (chains.calls("prepare"), chains.calls("find_maker_claim")),
            (0, 1)
        );

        // A sweep started and not recorded (a crash, or an answer lost after
        // the wallet had it) is never resent.
        progress.maker_claim = Some(maker_claim(130_000));
        progress.claim_sweep_attempts = 1;
        progress.claim_sweep_in_doubt = true;
        let (_directory, files) = swap(&progress);
        let chains = StubChains::default();
        step(&chains, &files, 140_000, false).await.unwrap();
        assert_eq!(chains.calls("sweep"), 0);
        assert_eq!(
            xmr_taker_view(&files.progress().unwrap(), WINDOWS, 140_000).attention_reason,
            Some("xmr_claim_sweep_in_doubt")
        );
        // The same, reached by a sweep whose outcome is unknown.
        progress.claim_sweep_attempts = 0;
        progress.claim_sweep_in_doubt = false;
        let (_directory, files) = swap(&progress);
        let chains = StubChains::default();
        *chains.sweep_fails.lock().unwrap() = Some(false);
        assert!(step(&chains, &files, 140_000, false).await.is_err());
        step(&chains, &files, 140_000, false).await.unwrap();
        assert_eq!(chains.calls("sweep"), 1);
        assert!(files.progress().unwrap().claim_sweep_in_doubt);

        // A sweep refused before the wallet had it is retried, from the next
        // wallet file, after its back-off.
        let (_directory, files) = swap(&progress);
        let chains = StubChains::default();
        *chains.sweep_fails.lock().unwrap() = Some(true);
        assert!(step(&chains, &files, 140_000, false).await.is_err());
        assert!(!files.progress().unwrap().claim_sweep_in_doubt);
        step(&chains, &files, 140_000 + SWEEP_RETRY_FIRST_MS, false)
            .await
            .unwrap();
        assert_eq!(*chains.sweep_attempts.lock().unwrap(), vec![1, 2]);
        assert!(files.progress().unwrap().claim_sweep.is_some());

        // Swept: the confirmations are mined once, then only observed.
        progress.claim_sweep = Some(XmrClaimSweepV1 {
            mined: true,
            ..sweep()
        });
        let (_directory, files) = swap(&progress);
        let chains = StubChains::default();
        step(&chains, &files, 150_000, false).await.unwrap();
        assert_eq!(
            (
                chains.calls("sweep"),
                chains.calls("mine"),
                chains.calls("confirm")
            ),
            (0, 0, 1)
        );
    }

    /// Tag 14 is never authorised without the owner's command, before the
    /// Maker's Monero is verified behind a lock funded within the cutoff,
    /// after a refund was sent, or from R on.
    #[tokio::test]
    async fn the_claim_is_refused_before_the_monero_lock_and_from_r() {
        let refused = |progress: XmrTakerProgressV1, now: u64, admitted: bool| async move {
            let (_directory, files) = swap(&progress);
            let chains = StubChains::default();
            assert!(step(&chains, &files, now, admitted).await.is_err());
            assert_eq!(chains.calls("prepare"), 0);
            assert_eq!(files.progress().unwrap().claim_authorization, None);
        };
        refused(locked(), 100_000, false).await;
        let mut unverified = locked();
        unverified.monero_funding_transaction = None;
        refused(unverified, 100_000, true).await;
        let mut late = locked();
        late.funded = effect(WINDOWS.funding_cutoff_ms + 1);
        refused(late, 300_000, true).await;
        let mut refunding = locked();
        refunding.refund_submitted = Some("ef".repeat(32));
        refused(refunding, WINDOWS.refund_ms - 1, true).await;
        refused(locked(), WINDOWS.refund_ms, true).await;
        refused(locked(), WINDOWS.punish_ms, true).await;

        let (_directory, files) = swap(&locked());
        let chains = StubChains::default();
        step(&chains, &files, WINDOWS.refund_ms - 1, true)
            .await
            .unwrap();
        assert_eq!(chains.calls("submit"), 1);
    }

    /// The funding the claim rests on is read again right before tag 14 is
    /// authorised and before each submission (TM-0034): one reorganised out
    /// or double spent since it was first seen stops the claim, and holds
    /// back a resubmission while what was sent is still looked for.
    #[tokio::test]
    async fn a_funding_gone_since_it_was_seen_stops_the_claim() {
        let (_directory, files) = swap(&locked());
        let chains = StubChains::default();
        *chains.funding_gone.lock().unwrap() = true;
        let error = step(&chains, &files, WINDOWS.refund_ms - 1, true)
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("read again before the claim"));
        assert_eq!(chains.calls("prepare"), 0);
        assert_eq!(chains.calls("submit"), 0);
        assert_eq!(files.progress().unwrap().claim_authorization, None);

        // Authorised and sent while the funding was there; gone at the next
        // step: nothing is sent again, and tag 14 is still looked for.
        let (_directory, files) = swap(&locked());
        let chains = StubChains::default();
        step(&chains, &files, 100_000, true).await.unwrap();
        assert_eq!(chains.calls("funding_still_final"), 1);
        assert_eq!(chains.calls("submit"), 1);
        *chains.funding_gone.lock().unwrap() = true;
        assert!(step(&chains, &files, 100_000, false).await.is_err());
        assert_eq!(chains.calls("submit"), 1);
        assert_eq!(chains.calls("find_authorization"), 2);
    }

    /// Past P an authorised claim is not over on the clock: a Maker that
    /// claimed before R, its tag 15 not yet seen (the Node was down, or its
    /// look keeps failing, which the row says), leaves the Monero to sweep;
    /// only its punishment, once final, ends the swap punished.
    #[tokio::test]
    async fn past_p_an_authorised_claim_ends_on_what_the_chain_shows() {
        let mut authorised = locked();
        authorised.claim_authorization = Some("ab".repeat(32));
        authorised.claim_authorized = effect(110_000);
        let now = WINDOWS.punish_ms + 1;

        // Claimed before R; the look fails past P, then recovers.
        let (_directory, files) = swap(&authorised);
        let chains = StubChains::default();
        *chains.maker_claim.lock().unwrap() = Some(Some([7; 64]));
        *chains.claim_scan_fails.lock().unwrap() = true;
        assert!(step(&chains, &files, now, false).await.is_err());
        let row = xmr_taker_view(&files.progress().unwrap(), WINDOWS, now);
        assert_eq!(row.state, TakerSwapStateV1::AttentionRequired);
        assert_eq!(row.attention_reason, Some("xmr_maker_claim_scan_failing"));
        *chains.claim_scan_fails.lock().unwrap() = false;
        step(&chains, &files, now, false).await.unwrap();
        let row = xmr_taker_view(&files.progress().unwrap(), WINDOWS, now);
        assert_eq!(row.state, TakerSwapStateV1::ClaimInProgress);
        assert_eq!(row.actor_phase, "sweeping");
        assert_eq!(chains.calls("find_punishment"), 0);

        // Not claimed: watched until the punishment is final.
        let (_directory, files) = swap(&authorised);
        let chains = StubChains::default();
        step(&chains, &files, now, false).await.unwrap();
        let row = xmr_taker_view(&files.progress().unwrap(), WINDOWS, now);
        assert_eq!(row.state, TakerSwapStateV1::AttentionRequired);
        assert_eq!(row.attention_reason, Some("xmr_watching_for_maker_claim"));
        *chains.punished.lock().unwrap() = true;
        step(&chains, &files, now, false).await.unwrap();
        let row = xmr_taker_view(&files.progress().unwrap(), WINDOWS, now);
        assert_eq!(row.state, TakerSwapStateV1::Punished);

        // Never authorised: the Maker cannot have claimed; punished from P.
        let row = xmr_taker_view(&locked(), WINDOWS, now);
        assert_eq!(row.state, TakerSwapStateV1::Punished);
    }

    /// A finalized tag 15 without an aggregate signature, or with one that
    /// does not open the shared address, is never recorded and never swept.
    #[tokio::test]
    async fn a_missing_or_wrong_maker_claim_signature_is_refused() {
        let mut progress = locked();
        progress.claim_authorization = Some("ab".repeat(32));
        progress.claim_authorized = effect(110_000);
        for (signature, invalid) in [(None, false), (Some([0x11; 64]), true)] {
            let (_directory, files) = swap(&progress);
            let chains = StubChains {
                signature_invalid: invalid,
                ..StubChains::default()
            };
            *chains.maker_claim.lock().unwrap() = Some(signature);
            assert!(step(&chains, &files, 120_000, true).await.is_err());
            assert!(step(&chains, &files, 120_000, true).await.is_err());
            let recorded = files.progress().unwrap();
            assert_eq!(recorded.maker_claim, None);
            assert_eq!(
                recorded.claim_scan_from, None,
                "the scan does not move past it"
            );
            assert_eq!(chains.calls("sweep"), 0);
        }
    }

    /// The Maker's claim signature, completed from the Taker's published
    /// partial with the Maker's Monero share as the Maker's tag 15 does,
    /// gives the Taker the spend key of the shared address; a signature
    /// that is not that completion gives nothing.
    #[tokio::test]
    async fn the_makers_claim_signature_reveals_the_shared_spend_key() {
        use lez_adaptor_role_runner::{
            accept_published_peer_partial_and_adapt, read_final_signature_packet,
        };
        use xmr_reference_actor::{XMR_STAGE_A_FILE, XMR_STAGE_B_FILE};

        let take = super::super::xmr_dynamic::tests::take(None);
        take.negotiate().await.unwrap();
        let pinned = |path: PathBuf| {
            let digest: [u8; 32] = Sha256::digest(std::fs::read(&path).unwrap()).into();
            (path, digest)
        };
        let taker_root = take.taker_swap.join(ROLE_DIRECTORY);
        let files = XmrSwapFiles {
            swap_root: take.taker_swap.clone(),
            stage_a: pinned(taker_root.join(XMR_STAGE_A_FILE)),
            stage_b: pinned(taker_root.join(XMR_STAGE_B_FILE)),
        };
        let maker = XmrSwapFiles {
            swap_root: take.maker.root.clone(),
            ..files.clone()
        };
        let (agreement, _, _) = files.agreement().unwrap();
        let session = ValidatedSession::from_untweaked_context(
            agreement.claim_session_descriptor().context().unwrap(),
        )
        .unwrap();
        let partial = XmrTakerCeremony::open(&taker_root)
            .unwrap()
            .claim_partial()
            .unwrap();
        let packet = take.maker.root.join("claim-final-signature.json");
        accept_published_peer_partial_and_adapt(
            &maker
                .role_root()
                .join(xmr_reference_actor::XMR_ADAPTOR_JOURNAL_FILE),
            &session,
            Role::Maker,
            *partial,
            CrossCurveScalar::from_monero_little_endian(*role_share(&maker).unwrap())
                .unwrap()
                .adaptor_scalar_big_endian(),
            &packet,
        )
        .unwrap();
        let signature = read_final_signature_packet(&packet, &session).unwrap();

        let key = maker_claim_key(&files, &agreement, signature).unwrap();
        assert_eq!(
            key.public_key(),
            agreement.shared_address().public_spend_key()
        );
        let mut wrong = signature;
        wrong[63] ^= 1;
        assert!(maker_claim_key(&files, &agreement, wrong).is_err());
        // Nor is any other signature.
        assert!(maker_claim_key(&files, &agreement, [0x11; 64]).is_err());
    }

    /// The refund's chain, stubbed: the finalized LEZ time, whether the
    /// refund is final, and every call counted.
    struct StubRefund {
        calls: Mutex<std::collections::BTreeMap<&'static str, u32>>,
        lez_ms: Mutex<u64>,
        final_at: Mutex<Option<u64>>,
        tip: Mutex<Option<XmrNativeEscrowStateV3>>,
    }

    impl StubRefund {
        fn at(lez_ms: u64) -> Self {
            Self {
                calls: Mutex::default(),
                lez_ms: Mutex::new(lez_ms),
                final_at: Mutex::new(None),
                tip: Mutex::new(Some(XmrNativeEscrowStateV3::Funded)),
            }
        }

        fn called(&self, name: &'static str) {
            *self.calls.lock().unwrap().entry(name).or_default() += 1;
        }

        fn calls(&self, name: &str) -> u32 {
            self.calls.lock().unwrap().get(name).copied().unwrap_or(0)
        }
    }

    #[async_trait::async_trait]
    impl XmrRefundChains for StubRefund {
        type Refund = String;

        async fn lez_time_ms(&self) -> Result<u64> {
            Ok(*self.lez_ms.lock().unwrap())
        }

        async fn escrow_state_at_tip(&self) -> Result<Option<XmrNativeEscrowStateV3>> {
            self.called("tip");
            Ok(*self.tip.lock().unwrap())
        }

        async fn complete_refund(&self) -> Result<String> {
            self.called("complete");
            Ok("16".repeat(32))
        }

        fn refund_id(refund: &String) -> String {
            refund.clone()
        }

        async fn submit_refund(&self, refund: &String) -> Result<()> {
            assert_eq!(*refund, "16".repeat(32), "the same bytes every time");
            self.called("submit");
            Ok(())
        }

        async fn find_refund(
            &self,
            _: &String,
            cursor: u64,
        ) -> Result<std::result::Result<FinalizedLezEffectV1, u64>> {
            self.called("find");
            Ok(match *self.final_at.lock().unwrap() {
                Some(at) => Ok(effect(at).unwrap()),
                None => Err(cursor + 1),
            })
        }
    }

    /// One refund step at the host time `_host_ms`, which plays no part: the
    /// refund runs on LEZ time.
    async fn refund_tick(chains: &StubRefund, files: &XmrSwapFiles, _host_ms: u64) -> Result<()> {
        refund_step(chains, files, WINDOWS, &files.progress().unwrap()).await
    }

    /// LEZ admits a refund to its mempool and drops it at block building
    /// when its window is not open: an accepted refund that never lands is
    /// sent again, the identical bytes, on every tick while LEZ time is
    /// before P, until it is final.
    #[tokio::test]
    async fn a_refund_accepted_and_never_included_is_sent_again_until_it_lands() {
        let (_directory, files) = swap(&locked());
        let chains = StubRefund::at(WINDOWS.refund_ms);
        refund_tick(&chains, &files, WINDOWS.refund_ms)
            .await
            .unwrap();
        assert_eq!(chains.calls("submit"), 1);
        assert_eq!(
            files.progress().unwrap().refund_submitted,
            Some("16".repeat(32))
        );
        // Accepted, dropped: the next ticks send it again.
        *chains.lez_ms.lock().unwrap() = WINDOWS.refund_ms + 1_000;
        refund_tick(&chains, &files, WINDOWS.refund_ms + 1_000)
            .await
            .unwrap();
        refund_tick(&chains, &files, WINDOWS.refund_ms + 2_000)
            .await
            .unwrap();
        assert_eq!(chains.calls("submit"), 3);
        assert_eq!(files.progress().unwrap().refunded, None);
        // A later send lands.
        *chains.final_at.lock().unwrap() = Some(WINDOWS.refund_ms + 3_000);
        refund_tick(&chains, &files, WINDOWS.refund_ms + 3_000)
            .await
            .unwrap();
        assert_eq!(
            files.progress().unwrap().refunded,
            effect(WINDOWS.refund_ms + 3_000)
        );
        assert_eq!(actor_phase(&files, WINDOWS.refund_ms + 3_000), "refunded");
        // Final: nothing more is sent.
        refund_tick(&chains, &files, WINDOWS.refund_ms + 4_000)
            .await
            .unwrap();
        assert_eq!(chains.calls("submit"), 4);
        assert_eq!(chains.calls("complete"), 4);
    }

    /// The refund's window is the chain's: a Taker whose clock runs ahead of
    /// LEZ time sends nothing before LEZ time reaches R, and sends it then;
    /// from P by LEZ time it is no longer sent, only looked for.
    #[tokio::test]
    async fn a_host_clock_ahead_of_lez_time_does_not_send_the_refund_early() {
        let (_directory, files) = swap(&locked());
        let chains = StubRefund::at(WINDOWS.refund_ms - 30_000);
        refund_tick(&chains, &files, WINDOWS.refund_ms + 60_000)
            .await
            .unwrap();
        assert_eq!((chains.calls("complete"), chains.calls("submit")), (0, 0));
        assert_eq!(files.progress().unwrap().refund_submitted, None);
        *chains.lez_ms.lock().unwrap() = WINDOWS.refund_ms;
        refund_tick(&chains, &files, WINDOWS.refund_ms + 90_000)
            .await
            .unwrap();
        assert_eq!(chains.calls("submit"), 1);
        // The host clock past P while LEZ time is not: still sent.
        *chains.lez_ms.lock().unwrap() = WINDOWS.punish_ms - 1;
        refund_tick(&chains, &files, WINDOWS.punish_ms + 30_000)
            .await
            .unwrap();
        assert_eq!(chains.calls("submit"), 2);
        // LEZ time at P: it can no longer land, it is only looked for.
        *chains.lez_ms.lock().unwrap() = WINDOWS.punish_ms;
        refund_tick(&chains, &files, WINDOWS.punish_ms)
            .await
            .unwrap();
        assert_eq!(chains.calls("submit"), 2);
        assert_eq!(chains.calls("find"), 3);
    }

    /// Past P a refund that is not final is no longer "in flight": the row
    /// says it did not land before the punish time and offers nothing, and a
    /// refund seen final afterwards still reads refunded.
    #[test]
    fn a_refund_not_final_at_p_says_so() {
        use TakerSwapStateV1 as State;
        let mut progress = locked();
        progress.refund_submitted = Some("16".repeat(32));
        let at = |progress: &XmrTakerProgressV1, now| xmr_taker_view(progress, WINDOWS, now);
        assert_eq!(
            at(&progress, WINDOWS.punish_ms - 1).actor_phase,
            "refund_in_flight"
        );
        let late = at(&progress, WINDOWS.punish_ms);
        assert_eq!(late.state, State::AttentionRequired);
        assert_eq!(late.actor_phase, "refund_not_final");
        assert_eq!(
            late.attention_reason,
            Some("xmr_refund_not_final_by_punish_time")
        );
        assert_eq!(late.available_action, None);
        progress.refunded = effect(WINDOWS.punish_ms - 1);
        let refunded = at(&progress, WINDOWS.punish_ms + 1);
        assert_eq!(refunded.state, State::Refunded);
        assert!(refunded.revision >= late.revision);
    }

    /// One tick's claim and refund, the Node's automatic refund on or off.
    async fn settle(
        claim: &StubChains,
        refund: &StubRefund,
        files: &XmrSwapFiles,
        now: u64,
        admitted: Option<TakerFacadeActionV1>,
        automatic: bool,
    ) -> Result<()> {
        settle_step(
            claim, refund, files, WINDOWS, now, admitted, automatic, false, false,
        )
        .await
    }

    /// The Node refunds by itself from R by LEZ time, with no owner's
    /// command: on a swap the Maker funded and on one it never funded (the
    /// unfunded punishment of TM-0031 needs the Taker's refund in [R, P)),
    /// and also while the host's clock is still before R. Off, only the
    /// owner's command refunds.
    #[tokio::test]
    async fn the_node_refunds_by_itself_from_r_without_an_owner_command() {
        let mut unfunded = locked();
        unfunded.monero_funding_transaction = None;
        for progress in [locked(), unfunded] {
            let (_directory, files) = swap(&progress);
            let claim = StubChains::default();
            let refund = StubRefund::at(WINDOWS.refund_ms - 1);
            settle(&claim, &refund, &files, WINDOWS.refund_ms, None, true)
                .await
                .unwrap();
            assert_eq!(refund.calls("submit"), 0, "LEZ time is before R");
            *refund.lez_ms.lock().unwrap() = WINDOWS.refund_ms;
            settle(&claim, &refund, &files, WINDOWS.refund_ms, None, true)
                .await
                .unwrap();
            assert_eq!(refund.calls("submit"), 1);
            assert_eq!(actor_phase(&files, WINDOWS.refund_ms), "refund_in_flight");
            assert_eq!(claim.calls("prepare"), 0, "no claim is authorised");
        }

        // The host's clock behind LEZ time: refunded from LEZ time's R.
        let (_directory, files) = swap(&locked());
        let (claim, refund) = (StubChains::default(), StubRefund::at(WINDOWS.refund_ms));
        settle(
            &claim,
            &refund,
            &files,
            WINDOWS.refund_ms - 5_000,
            None,
            true,
        )
        .await
        .unwrap();
        assert_eq!(refund.calls("submit"), 1);

        // Off: nothing without the owner's command, which still refunds.
        let (_directory, files) = swap(&locked());
        let (claim, refund) = (StubChains::default(), StubRefund::at(WINDOWS.refund_ms));
        settle(&claim, &refund, &files, WINDOWS.refund_ms, None, false)
            .await
            .unwrap();
        assert_eq!(refund.calls("complete"), 0);
        settle(
            &claim,
            &refund,
            &files,
            WINDOWS.refund_ms,
            Some(TakerFacadeActionV1::Refund),
            false,
        )
        .await
        .unwrap();
        assert_eq!(refund.calls("submit"), 1);
    }

    /// The Maker's claim (tag 15) landed: the Node refunds nothing, also by
    /// itself and on the owner's command, and sweeps the Monero instead.
    #[tokio::test]
    async fn after_the_makers_claim_the_node_sweeps_and_never_refunds() {
        let mut progress = locked();
        progress.claim_authorization = Some("ab".repeat(32));
        progress.claim_authorized = effect(110_000);
        for admitted in [None, Some(TakerFacadeActionV1::Refund)] {
            let (_directory, files) = swap(&progress);
            let claim = StubChains::default();
            *claim.maker_claim.lock().unwrap() = Some(Some([0x11; 64]));
            let refund = StubRefund::at(WINDOWS.refund_ms);
            let now = WINDOWS.refund_ms;
            settle(&claim, &refund, &files, now, admitted, true)
                .await
                .unwrap();
            assert!(files.progress().unwrap().maker_claim.is_some());
            settle(&claim, &refund, &files, now, admitted, true)
                .await
                .unwrap();
            assert_eq!(claim.calls("sweep"), 1);
            assert_eq!((refund.calls("complete"), refund.calls("submit")), (0, 0));
            assert_eq!(files.progress().unwrap().refund_submitted, None);
        }
    }

    /// The Maker's claim (tag 15) is at the sequencer tip and not yet final:
    /// the refund is not completed and not sent, by itself or on the owner's
    /// command (its signature would hand the Maker the Taker's share while
    /// the Maker holds its own); once the claim is final the Monero is swept.
    /// Our own refund at the tip is followed, not sent again.
    #[tokio::test]
    async fn a_maker_claim_at_the_tip_holds_the_refund_back() {
        let mut progress = locked();
        progress.claim_authorization = Some("ab".repeat(32));
        progress.claim_authorized = effect(110_000);
        let (_directory, files) = swap(&progress);
        let claim = StubChains::default();
        let refund = StubRefund::at(WINDOWS.refund_ms);
        *refund.tip.lock().unwrap() = Some(XmrNativeEscrowStateV3::Claimed);
        for admitted in [None, Some(TakerFacadeActionV1::Refund)] {
            settle(&claim, &refund, &files, WINDOWS.refund_ms, admitted, true)
                .await
                .unwrap();
        }
        assert_eq!((refund.calls("complete"), refund.calls("submit")), (0, 0));
        assert_eq!(files.progress().unwrap().refund_submitted, None);
        *claim.maker_claim.lock().unwrap() = Some(Some([0x11; 64]));
        for _ in 0..2 {
            settle(&claim, &refund, &files, WINDOWS.refund_ms, None, true)
                .await
                .unwrap();
        }
        assert_eq!(claim.calls("sweep"), 1);
        assert_eq!(refund.calls("submit"), 0);

        // Our refund at the tip: followed to finality, not sent again.
        let mut progress = locked();
        progress.refund_submitted = Some("16".repeat(32));
        let (_directory, files) = swap(&progress);
        let refund = StubRefund::at(WINDOWS.refund_ms);
        *refund.tip.lock().unwrap() = Some(XmrNativeEscrowStateV3::Refunded);
        refund_tick(&refund, &files, WINDOWS.refund_ms)
            .await
            .unwrap();
        assert_eq!((refund.calls("submit"), refund.calls("find")), (0, 1));
    }

    /// The lock's chain, stubbed: what is final, the submissions' answer,
    /// the finalized LEZ time and the escrow at the tip.
    struct StubLock {
        calls: Mutex<std::collections::BTreeMap<&'static str, u32>>,
        /// The effects found final, with their finalized time.
        initialized_at: Mutex<Option<u64>>,
        funded_at: Mutex<Option<u64>>,
        submit_fails: bool,
        lez_ms: Mutex<u64>,
        tip: Mutex<Option<XmrNativeEscrowStateV3>>,
    }

    impl StubLock {
        fn new() -> Self {
            Self {
                calls: Mutex::default(),
                initialized_at: Mutex::new(None),
                funded_at: Mutex::new(None),
                submit_fails: false,
                lez_ms: Mutex::new(0),
                tip: Mutex::new(None),
            }
        }

        fn called(&self, name: &'static str) {
            *self.calls.lock().unwrap().entry(name).or_default() += 1;
        }

        fn calls(&self, name: &str) -> u32 {
            self.calls.lock().unwrap().get(name).copied().unwrap_or(0)
        }
    }

    #[async_trait::async_trait]
    impl XmrLockChains for StubLock {
        type Transaction = &'static str;

        async fn prepare_pair(&self) -> Result<(&'static str, &'static str)> {
            self.called("prepare");
            Ok(("initialization", "funding"))
        }

        async fn finalized_height(&self) -> Result<u64> {
            Ok(3)
        }

        async fn submit(&self, _: &&'static str) -> Result<()> {
            self.called("submit");
            ensure!(!self.submit_fails, "unknown submission outcome");
            Ok(())
        }

        async fn submit_funding_after(&self, _: &&'static str, _: &&'static str) -> Result<()> {
            self.called("submit");
            ensure!(!self.submit_fails, "unknown submission outcome");
            Ok(())
        }

        async fn find_final(
            &self,
            kind: XmrNativeEffectV3,
            _: &&'static str,
            cursor: u64,
        ) -> Result<std::result::Result<FinalizedLezEffectV1, u64>> {
            self.called("find");
            let at = if kind == XmrNativeEffectV3::Initialize {
                *self.initialized_at.lock().unwrap()
            } else {
                *self.funded_at.lock().unwrap()
            };
            Ok(at.map_or(Err(cursor + 1), |at| Ok(effect(at).unwrap())))
        }

        async fn lez_time_ms(&self) -> Result<u64> {
            Ok(*self.lez_ms.lock().unwrap())
        }

        async fn escrow_state_at_tip(&self) -> Result<Option<XmrNativeEscrowStateV3>> {
            self.called("tip");
            Ok(*self.tip.lock().unwrap())
        }
    }

    fn lock_prepared() -> XmrTakerProgressV1 {
        let mut progress = record();
        progress.scan_from = Some(3);
        progress
    }

    /// A lock final before C that the Node had not seen final by C (it was
    /// down, or its scan lagged) is looked for and found after C; the swap
    /// then reads locked, and is refunded from R.
    #[tokio::test]
    async fn a_lock_final_before_c_and_seen_after_c_is_found() {
        let mut progress = lock_prepared();
        progress.initialized = effect(200_000);
        let (_directory, files) = swap(&progress);
        let chains = StubLock::new();
        *chains.funded_at.lock().unwrap() = Some(WINDOWS.funding_cutoff_ms - 1);
        let after_c = WINDOWS.funding_cutoff_ms + 30_000;
        assert_eq!(actor_phase(&files, after_c), "lez_lock_unconfirmed");
        lock_step(
            &chains,
            &files,
            WINDOWS,
            &files.progress().unwrap(),
            after_c,
        )
        .await
        .unwrap();
        assert_eq!(chains.calls("submit"), 0, "nothing is sent past C");
        assert_eq!(
            files.progress().unwrap().funded,
            effect(WINDOWS.funding_cutoff_ms - 1)
        );
        assert_eq!(actor_phase(&files, after_c), "taker_lock_confirmed");
        let (claim, refund) = (StubChains::default(), StubRefund::at(WINDOWS.refund_ms));
        settle(&claim, &refund, &files, WINDOWS.refund_ms, None, true)
            .await
            .unwrap();
        assert_eq!(refund.calls("submit"), 1);
    }

    /// A lock that becomes final only after C (the escrow takes the funding
    /// until R) is found, reads locked with no claim, and is refunded from R,
    /// by the Node itself or on the owner's command.
    #[tokio::test]
    async fn a_lock_final_after_c_is_found_and_refunded_from_r() {
        for (automatic, admitted) in [(true, None), (false, Some(TakerFacadeActionV1::Refund))] {
            let mut progress = lock_prepared();
            progress.initialized = effect(200_000);
            let (_directory, files) = swap(&progress);
            let chains = StubLock::new();
            let after_c = WINDOWS.funding_cutoff_ms + 30_000;
            lock_step(
                &chains,
                &files,
                WINDOWS,
                &files.progress().unwrap(),
                after_c,
            )
            .await
            .unwrap();
            assert_eq!(files.progress().unwrap().funded, None);
            *chains.funded_at.lock().unwrap() = Some(after_c);
            lock_step(
                &chains,
                &files,
                WINDOWS,
                &files.progress().unwrap(),
                after_c + 1,
            )
            .await
            .unwrap();
            assert_eq!(files.progress().unwrap().funded, effect(after_c));
            let refundable = xmr_taker_view(&files.progress().unwrap(), WINDOWS, WINDOWS.refund_ms);
            assert_eq!(
                refundable.available_action,
                Some(TakerTerminalActionV1::Refund)
            );
            let (claim, refund) = (StubChains::default(), StubRefund::at(WINDOWS.refund_ms));
            settle(
                &claim,
                &refund,
                &files,
                WINDOWS.refund_ms,
                admitted,
                automatic,
            )
            .await
            .unwrap();
            assert_eq!(refund.calls("submit"), 1);
        }
    }

    /// A submission whose answer was lost (the sidecar replays "unknown"
    /// for it from then on) does not stop the look for the lock: before C
    /// and after it, the pair is found once it is final.
    #[tokio::test]
    async fn a_lost_submit_answer_does_not_stop_the_look_for_the_lock() {
        let (_directory, files) = swap(&lock_prepared());
        let chains = StubLock {
            submit_fails: true,
            ..StubLock::new()
        };
        *chains.initialized_at.lock().unwrap() = Some(10_000);
        assert!(
            lock_step(&chains, &files, WINDOWS, &files.progress().unwrap(), 11_000)
                .await
                .is_err()
        );
        assert_eq!(files.progress().unwrap().initialized, effect(10_000));
        *chains.funded_at.lock().unwrap() = Some(12_000);
        let after_c = WINDOWS.funding_cutoff_ms + 1;
        lock_step(
            &chains,
            &files,
            WINDOWS,
            &files.progress().unwrap(),
            after_c,
        )
        .await
        .unwrap();
        assert_eq!(files.progress().unwrap().funded, effect(12_000));
    }

    /// Past C a lock not found final is looked for until it is proven never
    /// funded: once the finalized LEZ time is at or past R (the escrow takes
    /// no funding from then on) and the escrow at the tip holds nothing.
    /// Then, and only then, the take reads closed with nothing locked.
    #[tokio::test]
    async fn a_lock_not_found_reads_closed_only_once_it_is_proven_unfunded() {
        let (_directory, files) = swap(&lock_prepared());
        let chains = StubLock::new();
        let after_c = WINDOWS.funding_cutoff_ms + 1;
        *chains.lez_ms.lock().unwrap() = WINDOWS.refund_ms - 1;
        lock_step(
            &chains,
            &files,
            WINDOWS,
            &files.progress().unwrap(),
            after_c,
        )
        .await
        .unwrap();
        assert_eq!(actor_phase(&files, after_c), "lez_lock_unconfirmed");
        // Funded at the tip, not yet final: still looked for.
        *chains.lez_ms.lock().unwrap() = WINDOWS.refund_ms;
        *chains.tip.lock().unwrap() = Some(XmrNativeEscrowStateV3::Funded);
        lock_step(
            &chains,
            &files,
            WINDOWS,
            &files.progress().unwrap(),
            WINDOWS.refund_ms,
        )
        .await
        .unwrap();
        assert_eq!(
            actor_phase(&files, WINDOWS.refund_ms),
            "lez_lock_unconfirmed"
        );
        *chains.tip.lock().unwrap() = None;
        lock_step(
            &chains,
            &files,
            WINDOWS,
            &files.progress().unwrap(),
            WINDOWS.refund_ms,
        )
        .await
        .unwrap();
        assert!(files.progress().unwrap().lock_unfunded);
        assert_eq!(
            actor_phase(&files, WINDOWS.refund_ms),
            "lez_lock_window_closed"
        );
        let before = chains.calls("find");
        lock_step(
            &chains,
            &files,
            WINDOWS,
            &files.progress().unwrap(),
            WINDOWS.refund_ms,
        )
        .await
        .unwrap();
        assert_eq!(
            chains.calls("find"),
            before,
            "a proven-unfunded take is not looked at again"
        );
        // A take that never prepared a pair reads closed past C at once.
        let (_directory, files) = swap(&record());
        assert_eq!(actor_phase(&files, after_c), "lez_lock_window_closed");
    }

    /// A progress record cut short (a crash mid-write on an older Node, a
    /// full disk) is reported on the swap's row, not left as a tick that
    /// fails in silence; the record is kept for repair. Records are written
    /// whole: a temporary file, synced, renamed over the record.
    #[test]
    fn a_torn_progress_record_is_reported_and_records_are_written_whole() {
        let (_directory, files) = swap(&locked());
        let facts = XmrTakerSwapFactsV1 {
            schema_version: 1,
            swap_id: SwapId::new("xmr-torn-swap".to_owned()).unwrap(),
            reservation_id: RequestId::new("xmr-torn-reservation").unwrap(),
            monero_restore_height: 1,
            lez_anchor_unix_seconds: 1,
            maker_xmr_funding_cutoff_ms: WINDOWS.funding_cutoff_ms,
            refund_at_ms: WINDOWS.refund_ms,
            punish_at_ms: WINDOWS.punish_ms,
            monero_network: Some("stagenet".to_owned()),
        };
        let facts_file = files.swap_root.join(SWAP_FACTS_FILE);
        replace_private(&facts_file, &serde_json::to_vec(&facts).unwrap()).unwrap();
        let path = files.swap_root.join(PROGRESS_FILE);
        let whole = std::fs::read(&path).unwrap();
        std::fs::write(&path, &whole[..whole.len() / 2]).unwrap();
        let status = files.status().expect("a row, not an error");
        assert_eq!(status.state, TakerSwapStateV1::AttentionRequired);
        assert_eq!(status.attention_reason, Some("xmr_progress_unreadable"));
        assert_eq!(status.available_action, None);
        assert_eq!(std::fs::read(&path).unwrap(), &whole[..whole.len() / 2]);

        std::fs::write(&path, &whole).unwrap();
        files
            .update(|record| record.monero_funding_transaction = None)
            .unwrap();
        let names: Vec<_> = std::fs::read_dir(&files.swap_root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let mut names = names;
        names.sort();
        assert_eq!(
            names,
            [
                std::ffi::OsString::from(SWAP_FACTS_FILE),
                std::ffi::OsString::from(PROGRESS_FILE)
            ]
        );
        assert!(crate::taker_service::xmr_supervisor::tests::is_owner_private(&path));
    }

    fn is_owner_private(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777 == 0o600
    }

    /// The refund (tag 16) is signed by the swap's own refund authority, not
    /// the owner account, so another swap's owner transaction in flight (a
    /// lock stuck until its cutoff, say) never holds it back, and a refund
    /// in flight never holds back another swap's owner transaction.
    #[tokio::test]
    async fn the_refund_never_waits_behind_another_swaps_owner_transaction() {
        for (automatic, admitted) in [(true, None), (false, Some(TakerFacadeActionV1::Refund))] {
            let (_directory, files) = swap(&locked());
            let (claim, refund) = (StubChains::default(), StubRefund::at(WINDOWS.refund_ms));
            let owner_busy = true;
            settle_step(
                &claim,
                &refund,
                &files,
                WINDOWS,
                WINDOWS.refund_ms,
                admitted,
                automatic,
                owner_busy,
                false,
            )
            .await
            .unwrap();
            assert_eq!(refund.calls("submit"), 1);
        }
        let mut progress = locked();
        progress.refund_submitted = Some("16".repeat(32));
        assert!(!lez_in_flight(&progress, WINDOWS, WINDOWS.refund_ms));
    }

    /// A sweep whose first swap's Monero daemon stalls: the second swap,
    /// past R, sends its refund before any Monero read of the sweep starts,
    /// and the stalled read ends at its budget, not at the daemon's will.
    struct StalledSweep {
        files: Vec<XmrSwapFiles>,
        refund: StubRefund,
        daemon: LoopbackRpcEndpoint,
        events: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl XmrSweep for StalledSweep {
        type Swap = usize;

        async fn refund_first(&self, swap: &usize) -> bool {
            let files = &self.files[*swap];
            let progress = files.progress().unwrap();
            let revision = xmr_taker_view(&progress, WINDOWS, WINDOWS.refund_ms).revision;
            if !refund_runs(&progress, revision, false, true) {
                return false;
            }
            refund_step(&self.refund, files, WINDOWS, &progress)
                .await
                .unwrap();
            self.events.lock().unwrap().push(format!("refund {swap}"));
            true
        }

        async fn tick(&self, swap: &usize, refunded: bool) {
            self.events
                .lock()
                .unwrap()
                .push(format!("tick {swap} refunded={refunded}"));
            if *swap == 0 {
                let attestor = MoneroChainIdentityAttestor::new(
                    lez_xmr_monero_adapter::MoneroNetwork::Stagenet,
                    &self.daemon,
                )
                .unwrap();
                let read = within_monero_budget(std::time::Duration::from_millis(500), async {
                    attestor.discover().await.map_err(anyhow::Error::from)
                })
                .await;
                assert!(read.is_err());
                self.events
                    .lock()
                    .unwrap()
                    .push("stalled read ended".into());
            }
        }
    }

    #[tokio::test]
    async fn a_stalled_monero_daemon_never_holds_back_a_refund() {
        // A daemon that accepts and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        // Swap 0 sweeps the Maker's claim (Monero I/O, no refund); swap 1 is
        // past R with nothing claimed: its refund is due.
        let mut claimed = locked();
        claimed.claim_authorization = Some("14".repeat(32));
        claimed.claim_authorized = effect(30_000);
        claimed.maker_claim = Some(XmrMakerClaimV1 {
            finalized: effect(40_000).unwrap(),
            signature: "15".repeat(64),
        });
        let (_first, sweeping) = swap(&claimed);
        let (_second, refunding) = swap(&locked());
        let sweep = StalledSweep {
            files: vec![sweeping, refunding],
            refund: StubRefund::at(WINDOWS.refund_ms),
            daemon: LoopbackRpcEndpoint::new(
                &format!("http://127.0.0.1:{port}"),
                "user",
                "password",
            )
            .unwrap(),
            events: Mutex::default(),
        };
        let started = std::time::Instant::now();
        sweep_refunds_first(&sweep, &[0, 1]).await;
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        assert_eq!(sweep.refund.calls("submit"), 1);
        assert_eq!(
            *sweep.events.lock().unwrap(),
            [
                "refund 1",
                "tick 0 refunded=false",
                "stalled read ended",
                "tick 1 refunded=true",
            ]
        );
    }

    /// A sweep that sends nothing (`RestoredBalanceMismatch` is one: no
    /// unspent output of the swap's amount yet) is retried, never given up,
    /// but each retry waits longer, up to a cap: no hot loop, and wallet
    /// files grow slowly. The sweep spends the swap's output by its key
    /// image (`restore_shared_and_sweep_once`), so extra outputs at the
    /// shared address do not stop it.
    #[tokio::test]
    async fn a_sweep_that_sends_nothing_backs_off_and_is_retried() {
        let mut progress = locked();
        progress.claim_authorization = Some("ab".repeat(32));
        progress.claim_authorized = effect(110_000);
        progress.maker_claim = Some(maker_claim(130_000));
        let (_directory, files) = swap(&progress);
        let chains = StubChains::default();
        let mut now = 140_000;
        let mut waits = Vec::new();
        for _ in 0..12 {
            *chains.sweep_fails.lock().unwrap() = Some(true);
            let before = chains.calls("sweep");
            assert!(step(&chains, &files, now, false).await.is_err());
            assert_eq!(chains.calls("sweep"), before + 1);
            // Not again until its back-off has passed.
            let retry_at = files.progress().unwrap().claim_sweep_retry_at_ms.unwrap();
            step(&chains, &files, retry_at - 1, false).await.unwrap();
            assert_eq!(chains.calls("sweep"), before + 1);
            waits.push(retry_at - now);
            now = retry_at;
        }
        assert!(waits.windows(2).all(|pair| pair[1] >= pair[0]));
        assert_eq!(waits[0], SWEEP_RETRY_FIRST_MS);
        assert_eq!(*waits.last().unwrap(), SWEEP_RETRY_MAX_MS);
        step(&chains, &files, now, false).await.unwrap();
        assert!(files.progress().unwrap().claim_sweep.is_some());
        assert_eq!(files.progress().unwrap().claim_sweep_retry_at_ms, None);
    }

    /// An effect counts at its own block's time, not at the end of the scan
    /// that found it: a funding in a block before C that the Node first scans
    /// after C is a funding before C, and the claim is offered on it, as the
    /// Maker (which reads the containing block) funds the Monero on it.
    #[test]
    fn an_effect_counts_at_its_blocks_time_not_the_scans() {
        let block = lez_bridge_protocol::FinalizedBlockIdentity::new(
            41,
            lez_bridge_protocol::Hex32::from_bytes([7; 32]),
            WINDOWS.funding_cutoff_ms - 5_000,
        );
        // Found by a scan that ended a minute after C.
        let funded = recorded_effect("cd".repeat(32), block);
        assert_eq!(funded.block_id, 41);
        assert_eq!(funded.finalized_at_ms, WINDOWS.funding_cutoff_ms - 5_000);
        let mut progress = record();
        progress.funded = Some(funded);
        progress.monero_funding_transaction = Some("cd".repeat(32));
        assert_eq!(
            xmr_taker_view(&progress, WINDOWS, WINDOWS.funding_cutoff_ms + 60_000).state,
            TakerSwapStateV1::ClaimAvailable
        );
    }

    #[test]
    fn the_progress_record_round_trips() {
        let mut progress = record();
        progress.funded = effect(5);
        let bytes = serde_json::to_vec(&progress).unwrap();
        assert_eq!(
            serde_json::from_slice::<XmrTakerProgressV1>(&bytes).unwrap(),
            progress
        );
        assert!(
            serde_json::from_slice::<XmrTakerProgressV1>(br#"{"schema_version":1,"x":1}"#).is_err()
        );
    }
}
