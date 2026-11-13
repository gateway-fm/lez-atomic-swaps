//! The Taker Node's in-process supervisor of its Monero swaps (the Zcash
//! supervisor's model, `zec_supervisor.rs`, without its schedule: one bounded
//! step per swap and tick).
//!
//! After the take the LEZ lock (tag 13: initialize, then fund the escrow) is
//! automatic. The supervisor then watches the Maker's Monero output to the
//! shared address through a view-only wallet; once it is final the swap reads
//! `claim_available`. From the agreement's refund time `R` the refund (tag 16)
//! is offered and runs on the owner's command; halfway through `[R, P)` a swap
//! still not refunded needs attention (`refund_window_closing`), and from the
//! punish time `P` it can no longer be refunded (the Maker may take the LEZ).
//!
//! The LEZ effects go through the swap's own sidecar, in process: the Taker's
//! effect-tool profile has no tag-13 slot, and its tag-14 tool does not exist
//! in this tree, so no schema-3 effect authority can be provisioned for it.
//! The claim (tag 14, then the Monero sweep) is not composed here yet: a swap
//! at `claim_available` offers no action until it is.
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
use lez_bridge_adapter::XmrLezBridgeBindingV3;
use lez_bridge_client::{BridgeClient, BridgeClientConfig, SidecarCapability};
use lez_bridge_protocol::{
    AggregateBip340Signature, ClassifyFinalizedNativeXmrEffectV3Request,
    CompleteNativeXmrRefundV3Request, DiscoveryWindow, FinalizedNativeXmrScanOutcomeV3,
    FinalizedNativeXmrTransactionTargetV3, MessageContext, Participant,
    PrepareNativeXmrEscrowV3Request, PrepareNativeXmrRefundV3Request, PreparedTransaction,
    RequestId, RuntimeDescriptor, SubmitTransactionRequest, XmrNativeEffectV3,
    XmrNativeEscrowTermsV3,
};
use lez_btc_role_lifecycle::{
    SwapSidecar,
    layout::{read_private, write_private_exact},
};
use lez_swap_core::{Phase, SwapId};
use lez_xmr_monero_adapter::{
    ExpectedMoneroOutput, MoneroAddress, MoneroChainIdentity, MoneroNetwork, MoneroOutputVerifier,
    MoneroRegtestWalletEffects, MoneroWalletRpcLock,
};
use lez_xmr_swap_sdk::{
    CrossCurveScalar, MoneroPrivateViewKey, XmrActivatedAgreementV1, XmrAgreementV1,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
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
/// Bounded finalized-history scans per effect and tick.
const SCANS_PER_TICK: usize = 8;

/// One process writes progress records; the supervisor and the owner's refund
/// both update them, one read-modify-write at a time.
static PROGRESS_WRITE: Mutex<()> = Mutex::new(());

/// One finalized LEZ effect of this swap.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FinalizedLezEffectV1 {
    pub(super) transaction_id: String,
    pub(super) block_id: u64,
    /// The finalized LEZ consensus time it was observed final at, ms.
    pub(super) finalized_at_ms: u64,
}

/// What the supervisor has observed and done for one swap.
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
pub(super) fn xmr_taker_view(
    progress: &XmrTakerProgressV1,
    windows: XmrWindows,
    now_ms: u64,
) -> XmrTakerView {
    use TakerSwapStateV1 as State;
    let refund = Some(TakerTerminalActionV1::Refund);
    if progress.refunded.is_some() {
        return view(Phase::Refunded, 9, State::Refunded, None, None, "refunded");
    }
    if progress.refund_submitted.is_some() {
        return view(
            Phase::TakerLegRefunded,
            8,
            State::RefundInProgress,
            None,
            None,
            "refund_in_flight",
        );
    }
    let Some(funded) = &progress.funded else {
        // The escrow is funded only at or before the funding cutoff; past it
        // nothing of the Taker's is locked and the take is over.
        if now_ms >= windows.funding_cutoff_ms {
            return view(
                Phase::Offered,
                1,
                State::AttentionRequired,
                None,
                Some("xmr_lez_lock_window_closed"),
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
    };
    if now_ms >= windows.punish_ms {
        // Tag 16 is valid only in [R, P): the Maker may now punish.
        return view(
            Phase::MakerRecoveryAvailable,
            10,
            State::AttentionRequired,
            None,
            Some("xmr_refund_window_closed"),
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
        return view(
            Phase::MakerLegRefunded,
            6,
            State::RefundAvailable,
            refund,
            None,
            "refund_available",
        );
    }
    if funded.finalized_at_ms <= windows.funding_cutoff_ms
        && progress.monero_funding_transaction.is_some()
    {
        // The claim (tag 14, then the Monero sweep) is not composed on the
        // Taker Node yet: the state is shown, no action is offered.
        return view(
            Phase::BothLegsLocked,
            3,
            State::ClaimAvailable,
            None,
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
    if let Ok(status) = files.status() {
        view.actor_phase = Some(status.actor_phase.to_owned());
        if let Some(reason) = status.attention_reason {
            view.attention_reason = Some(reason.to_owned());
        }
        if status.state == TakerSwapStateV1::ClaimAvailable {
            view.status_detail = Some(
                "the Maker's Monero is locked and final; the Monero claim (tag 14, then the sweep) is not yet composed on the Taker Node".to_owned(),
            );
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
        let path = self.swap_root.join(PROGRESS_FILE);
        let bytes = serde_json::to_vec_pretty(&progress)?;
        if path.exists() {
            std::fs::write(&path, &bytes)?;
        } else {
            write_private_exact(&path, &bytes)?;
        }
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

    /// The status a monitor answers: no chain is read.
    pub(super) fn status(&self) -> Result<XmrTakerView> {
        Ok(xmr_taker_view(
            &self.progress()?,
            XmrWindows::of(&self.facts()?),
            now_ms(),
        ))
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The swap's own sidecar as a bridge client, bound to its run.
struct SwapLez {
    client: BridgeClient,
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
        let capability = read_private(sidecar.capability_file(), 4096)?;
        let capability = SidecarCapability::new(String::from_utf8(capability)?.trim().to_owned())
            .context("sidecar capability")?;
        let descriptor = runtime.runtime_descriptor();
        let client = BridgeClient::connect(BridgeClientConfig::new(
            sidecar.endpoint(),
            capability,
            sidecar.run_id().clone(),
            descriptor.clone(),
            runtime.request_timeout(),
        ))
        .context("connect the swap's LEZ sidecar")?;
        Ok(Self {
            client,
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
        mut cursor: u64,
    ) -> Result<std::result::Result<FinalizedLezEffectV1, u64>> {
        for _ in 0..SCANS_PER_TICK {
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
                        FinalizedNativeXmrTransactionTargetV3::exact(transaction.clone()),
                        DiscoveryWindow::new(cursor, self.discovery_max_blocks)?,
                    ),
                )
                .await
                .context("classify finalized LEZ history")?;
            match result.outcome {
                FinalizedNativeXmrScanOutcomeV3::Found {
                    finalized_clock,
                    facts,
                    ..
                } => {
                    ensure!(
                        facts.transaction.transaction_id == transaction.transaction_id,
                        "the classifier found another transaction"
                    );
                    return Ok(Ok(FinalizedLezEffectV1 {
                        transaction_id: hex::encode(transaction.transaction_id.as_bytes()),
                        block_id: facts.containing_block.block_id,
                        finalized_at_ms: finalized_clock.timestamp_ms,
                    }));
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

/// Tag 13: prepares the escrow's initialization and funding once (the
/// sidecar replays its durable preparation), submits the exact bytes and
/// records each step once finalized; the funding goes out only after the
/// initialization is final within the funding cutoff.
async fn drive_lez_lock(
    dynamic: &DynamicXmrRole,
    files: &XmrSwapFiles,
    facts: &XmrTakerSwapFactsV1,
    progress: &XmrTakerProgressV1,
) -> Result<()> {
    let (agreement, activation, _) = files.agreement()?;
    let terms = XmrLezBridgeBindingV3::new(&agreement, &activation)
        .map_err(|error| anyhow::anyhow!("derive the LEZ escrow terms: {error:?}"))?
        .terms();
    let lez = SwapLez::connect(dynamic, &facts.reservation_id)?;
    let prepared = lez
        .client
        .prepare_native_xmr_escrow_v3(PrepareNativeXmrEscrowV3Request::new(
            lez.context(derived_request_id(
                &facts.reservation_id,
                "xmr",
                b"tag13-prepare",
            )?),
            lez.runtime.clone(),
            terms,
        ))
        .await
        .context("prepare the LEZ escrow")?;
    let cursor = if let Some(cursor) = progress.scan_from {
        cursor
    } else {
        let height = lez.finalized_height().await?;
        files.update(|record| record.scan_from = Some(height))?;
        height
    };
    let (effect, transaction) = if progress.initialized.is_none() {
        (XmrNativeEffectV3::Initialize, &prepared.initialization)
    } else {
        (XmrNativeEffectV3::Fund, &prepared.funding)
    };
    lez.submit(transaction).await?;
    match lez.find_final(&terms, effect, transaction, cursor).await? {
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
        }
        Err(cursor) => {
            files.update(|record| record.scan_from = Some(cursor))?;
        }
    }
    Ok(())
}

impl SwapLez {
    async fn finalized_height(&self) -> Result<u64> {
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
        Ok(observed.clock.height)
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
        let _lock = MoneroWalletRpcLock::acquire(&dynamic.wallet_lock_file()?)?;
        handle.block_on(observe_monero_locked(&dynamic, &files, &facts, restored))
    })
    .await
    .context("the Monero observation thread")?
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
            MoneroChainIdentity::new(
                MoneroNetwork::Regtest,
                agreement.body().monero().genesis_hash(),
            )?,
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
    let text = Zeroizing::new(read_private(&files.role_root().join(ROLE_SHARE_FILE), 128)?);
    let mut share = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(
        std::str::from_utf8(&text)
            .context("the Monero share is not text")?
            .trim(),
        &mut *share,
    )
    .context("the Monero share is not hex")?;
    let share = CrossCurveScalar::from_monero_little_endian(*share).context("the Monero share")?;
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

/// One refund step (tag 16): submits the completed refund once, inside
/// `[R, P)`, then follows it to finality. The caller holds the swap's actor
/// lock.
pub(super) async fn refund_step(dynamic: &DynamicXmrRole, files: &XmrSwapFiles) -> Result<()> {
    let facts = files.facts()?;
    let progress = files.progress()?;
    let windows = XmrWindows::of(&facts);
    let (agreement, activation, _) = files.agreement()?;
    let terms = XmrLezBridgeBindingV3::new(&agreement, &activation)
        .map_err(|error| anyhow::anyhow!("derive the LEZ escrow terms: {error:?}"))?
        .terms();
    let lez = SwapLez::connect(dynamic, &facts.reservation_id)?;
    let prepared = lez
        .client
        .prepare_native_xmr_refund_v3(PrepareNativeXmrRefundV3Request::new(
            lez.context(derived_request_id(
                &facts.reservation_id,
                "xmr",
                b"tag16-prepare",
            )?),
            lez.runtime.clone(),
            terms,
        ))
        .await
        .context("prepare the LEZ refund")?;
    let completed = lez
        .client
        .complete_native_xmr_refund_v3(
            CompleteNativeXmrRefundV3Request::new(
                lez.context(derived_request_id(
                    &facts.reservation_id,
                    "xmr",
                    b"tag16-complete",
                )?),
                lez.runtime.clone(),
                terms,
                prepared.refund,
                AggregateBip340Signature::from_bytes(refund_signature(
                    files,
                    &agreement,
                    &activation,
                )?),
            )
            .map_err(|error| anyhow::anyhow!("the refund completion: {error:?}"))?,
        )
        .await
        .context("complete the LEZ refund")?;
    if progress.refund_submitted.is_none() {
        let now = now_ms();
        ensure!(
            windows.refund_ms <= now && now < windows.punish_ms,
            "the refund is valid only between the refund and the punish time"
        );
        lez.submit(&completed.refund).await?;
        files.update(|record| {
            record.refund_submitted = Some(hex::encode(completed.refund.transaction_id.as_bytes()));
        })?;
    }
    let cursor = progress
        .scan_from
        .unwrap_or(progress.funded.as_ref().map_or(1, |funded| funded.block_id));
    match lez
        .find_final(&terms, XmrNativeEffectV3::Refund, &completed.refund, cursor)
        .await?
    {
        Ok(finalized) => {
            files.update(|record| record.refunded = Some(finalized))?;
        }
        Err(cursor) => {
            files.update(|record| record.scan_from = Some(cursor))?;
        }
    }
    Ok(())
}

/// One supervision step of one swap, by its status.
async fn tick_swap(
    dynamic: &Arc<DynamicXmrRole>,
    files: &XmrSwapFiles,
    refund_admitted: bool,
) -> Result<()> {
    let facts = files.facts()?;
    let progress = files.progress()?;
    let status = xmr_taker_view(&progress, XmrWindows::of(&facts), now_ms());
    match status.revision {
        0 => drive_lez_lock(dynamic, files, &facts, &progress).await,
        2 => observe_monero(dynamic, files, &facts, &progress).await,
        6..=8 if refund_admitted => refund_step(dynamic, files).await,
        _ => Ok(()),
    }
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
    for prepared in swaps {
        let Some(binding) = prepared.execution().receipt_binding() else {
            continue;
        };
        let refund_admitted = matches!(
            super::lifecycle::lookup_admitted_action_for_swap(&initiation, prepared.swap_id()).await,
            Ok(Some(admission)) if admission.action() == lez_swap_store::TakerFacadeActionV1::Refund
        );
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
            // The refund reads the adaptor material: it runs under the
            // swap's actor lock, as an owner's refund does.
            let held = if refund_admitted {
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
        let Ok(Ok((actor, _held))) = loaded else {
            continue;
        };
        let Some(files) = actor.xmr_files() else {
            continue;
        };
        if let Err(error) = tick_swap(&dynamic, files, refund_admitted).await {
            eprintln!(
                "{}",
                serde_json::json!({
                    "event": "taker_xmr_supervisor_step_failed",
                    "swap_id": prepared.swap_id().as_str(),
                    "error": format!("{error:#}"),
                })
            );
        }
    }
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
        assert_eq!(claimable.available_action, None);
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
        let punished = at(&progress, WINDOWS.punish_ms);
        assert_eq!(punished.state, State::AttentionRequired);
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
        let missed = xmr_taker_view(&progress, WINDOWS, WINDOWS.funding_cutoff_ms);
        assert_eq!(missed.state, State::AttentionRequired);
        assert_eq!(missed.attention_reason, Some("xmr_lez_lock_window_closed"));
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
