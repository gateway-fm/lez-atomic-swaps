//! The LEZ chain's progress as this Node sees it, for the owner health
//! methods (`maker_health`, `taker_health`): the last finalized block id the
//! indexer answers, when this Node saw it advance, the sequencer's tip, and a
//! named reason once finality stops.
//!
//! A frozen finality used to be invisible (#112): both Nodes answered
//! `ready: true, degraded: false` for 29 hours while the indexer's finalized
//! height sat at one block and every swap waited on it.
//!
//! The stall rule is the devnet finality heartbeat's
//! (`deploy/images/lez-wallet-module/heartbeat.sh`): finality is stalled when
//! the finalized block id has not changed for the threshold while the
//! sequencer's tip moved past where it was when finality last moved. A chain
//! whose tip stands still too produces nothing to finalize and is not called
//! stalled. One case the heartbeat leaves open is decided here: a tip this
//! Node cannot read (a sequencer that is down) does not prove an idle chain,
//! so a flat finality with an unreadable tip is stalled as well.
//!
//! Nothing polls on its own: a health call samples the chain, at most once
//! per [`MINIMUM_SAMPLE_INTERVAL`], with one `getLastFinalizedBlockId` read of
//! the indexer and one `getLastBlockId` read of the sequencer, run side by
//! side with a short timeout. "When it advanced" is when this Node first saw
//! the current finalized id, so it is as fine as the health calls are
//! frequent (the desks ask every 10 s), and a Node started on a chain that is
//! already frozen reports the stall one threshold after its first sample.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jsonrpsee::core::client::ClientT as _;
use jsonrpsee::rpc_params;
use jsonrpsee_http_client::HttpClientBuilder;
use serde::{Deserialize, Serialize};

use crate::config::BtcRoleRuntime;

/// How long the finalized block id may stand still, while the tip moves,
/// before a Node reports the stall: 10 minutes, ten blocks of a chain that
/// makes one a minute (the public testnet). The devnet passes its heartbeat's
/// threshold instead (`--lez-finality-stall-seconds`, gen-config.sh).
pub const DEFAULT_LEZ_FINALITY_STALL_SECONDS: u64 = 600;

/// The thresholds a Node accepts, in seconds.
pub const LEZ_FINALITY_STALL_SECONDS_RANGE: std::ops::RangeInclusive<u64> = 10..=86_400;

/// A health call answers from the last sample while it is younger than this,
/// so a desk polling often costs the chain two reads per interval at most.
pub const MINIMUM_SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

/// One read's deadline: a health call must answer within the healthcheck's
/// budget (5 s) even when the indexer hangs.
const READ_TIMEOUT: Duration = Duration::from_secs(3);

/// Why a Node reports the LEZ chain as degraded. Additive `snake_case`
/// strings: a consumer must tolerate one it does not know.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LezChainDegradedReasonV1 {
    /// The finalized block id has not advanced for the threshold while the
    /// sequencer's tip moved (or could not be read).
    LezFinalityStalled,
    /// The indexer did not answer its finalized block id just now.
    LezIndexerUnreachable,
}

/// The LEZ chain's progress in a health response (`lez_chain`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LezChainProgressV1 {
    /// True while `reason` names a problem: swaps that wait on LEZ finality
    /// (every lock, claim and refund) wait until it is gone.
    pub degraded: bool,
    /// Why the chain is degraded, when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<LezChainDegradedReasonV1>,
    /// The last finalized block id the indexer answered (the last one known
    /// while it does not answer); absent before the first answer.
    #[serde(default)]
    pub finalized_block_id: Option<u64>,
    /// When this Node first saw `finalized_block_id` (Unix seconds).
    #[serde(default)]
    pub finalized_advanced_at_unix_seconds: Option<u64>,
    /// Seconds since then, at `observed_at_unix_seconds`.
    #[serde(default)]
    pub finalized_flat_seconds: Option<u64>,
    /// The sequencer's last block id, when it answered.
    #[serde(default)]
    pub tip_block_id: Option<u64>,
    /// This Node's stall threshold, in seconds.
    pub stall_threshold_seconds: u64,
    /// When the chain was sampled (Unix seconds).
    pub observed_at_unix_seconds: u64,
}

/// The stall rule over successive samples. Pure: the caller supplies the
/// time and what the two reads answered.
#[derive(Clone, Debug)]
pub struct LezFinalityRule {
    stall_threshold_seconds: u64,
    finalized: Option<u64>,
    advanced_at: Option<u64>,
    tip_when_advanced: Option<u64>,
}

impl LezFinalityRule {
    /// A rule with no sample yet.
    #[must_use]
    pub const fn new(stall_threshold_seconds: u64) -> Self {
        Self {
            stall_threshold_seconds,
            finalized: None,
            advanced_at: None,
            tip_when_advanced: None,
        }
    }

    /// Takes one sample: `finalized` is the indexer's answer (`None` when it
    /// did not answer), `tip` the sequencer's.
    pub fn observe(
        &mut self,
        now_unix_seconds: u64,
        finalized: Option<u64>,
        tip: Option<u64>,
    ) -> LezChainProgressV1 {
        let reason = if let Some(finalized) = finalized {
            // Any change counts, a lower id too: a recreated chain starts again.
            if self.finalized != Some(finalized) {
                self.finalized = Some(finalized);
                self.advanced_at = Some(now_unix_seconds);
                self.tip_when_advanced = tip;
            }
            let flat = self
                .advanced_at
                .map_or(0, |at| now_unix_seconds.saturating_sub(at));
            let tip_stood_still = matches!(
                (tip, self.tip_when_advanced),
                (Some(tip), Some(then)) if tip <= then
            );
            (flat >= self.stall_threshold_seconds && !tip_stood_still)
                .then_some(LezChainDegradedReasonV1::LezFinalityStalled)
        } else {
            Some(LezChainDegradedReasonV1::LezIndexerUnreachable)
        };
        LezChainProgressV1 {
            degraded: reason.is_some(),
            reason,
            finalized_block_id: self.finalized,
            finalized_advanced_at_unix_seconds: self.advanced_at,
            finalized_flat_seconds: self
                .advanced_at
                .map(|at| now_unix_seconds.saturating_sub(at)),
            tip_block_id: tip,
            stall_threshold_seconds: self.stall_threshold_seconds,
            observed_at_unix_seconds: now_unix_seconds,
        }
    }
}

/// Samples the role's indexer and sequencer for the health methods.
#[derive(Debug)]
pub struct LezFinalityWatch {
    indexer_url: String,
    sequencer_url: String,
    state: tokio::sync::Mutex<WatchState>,
}

#[derive(Debug)]
struct WatchState {
    rule: LezFinalityRule,
    last: Option<(Instant, LezChainProgressV1)>,
}

impl LezFinalityWatch {
    /// Watches the given indexer and sequencer endpoints.
    #[must_use]
    pub fn new(indexer_url: String, sequencer_url: String, stall_threshold_seconds: u64) -> Self {
        Self {
            indexer_url,
            sequencer_url,
            state: tokio::sync::Mutex::new(WatchState {
                rule: LezFinalityRule::new(stall_threshold_seconds),
                last: None,
            }),
        }
    }

    /// Watches the endpoints the role's configuration names, the ones its
    /// sidecars use.
    #[must_use]
    pub fn for_runtime(runtime: &BtcRoleRuntime, stall_threshold_seconds: u64) -> Self {
        let lez = &runtime.config().lez;
        Self::new(
            lez.indexer_url.clone(),
            lez.sequencer_url.clone(),
            stall_threshold_seconds,
        )
    }

    /// The chain's progress now: a fresh sample, or the last one while it is
    /// younger than [`MINIMUM_SAMPLE_INTERVAL`]. Concurrent callers share one
    /// sample.
    pub async fn progress(&self) -> LezChainProgressV1 {
        let mut state = self.state.lock().await;
        if let Some((at, progress)) = &state.last
            && at.elapsed() < MINIMUM_SAMPLE_INTERVAL
        {
            return progress.clone();
        }
        let (finalized, tip) = tokio::join!(
            read_block_id(&self.indexer_url, "getLastFinalizedBlockId"),
            read_block_id(&self.sequencer_url, "getLastBlockId"),
        );
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let progress = state.rule.observe(now, finalized, tip);
        state.last = Some((Instant::now(), progress.clone()));
        progress
    }
}

async fn read_block_id(url: &str, method: &str) -> Option<u64> {
    let client = HttpClientBuilder::default()
        .request_timeout(READ_TIMEOUT)
        .build(url)
        .ok()?;
    client.request::<u64, _>(method, rpc_params![]).await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const THRESHOLD: u64 = 60;

    #[test]
    fn advancing_finality_is_not_degraded() {
        let mut rule = LezFinalityRule::new(THRESHOLD);
        for (step, now) in (1_000..1_600).step_by(50).enumerate() {
            let height = 10 + step as u64;
            let progress = rule.observe(now, Some(height), Some(height + 40));
            assert!(!progress.degraded, "{progress:?}");
            assert_eq!(progress.reason, None);
            assert_eq!(progress.finalized_block_id, Some(height));
            assert_eq!(progress.finalized_advanced_at_unix_seconds, Some(now));
            assert_eq!(progress.finalized_flat_seconds, Some(0));
        }
    }

    #[test]
    fn flat_finality_while_the_tip_moves_stalls_at_the_threshold() {
        let mut rule = LezFinalityRule::new(THRESHOLD);
        assert!(!rule.observe(1_000, Some(84), Some(100)).degraded);
        let before = rule.observe(1_059, Some(84), Some(130));
        assert!(!before.degraded);
        assert_eq!(before.finalized_flat_seconds, Some(59));
        let stalled = rule.observe(1_060, Some(84), Some(131));
        assert!(stalled.degraded);
        assert_eq!(
            stalled.reason,
            Some(LezChainDegradedReasonV1::LezFinalityStalled)
        );
        assert_eq!(stalled.finalized_block_id, Some(84));
        assert_eq!(stalled.finalized_advanced_at_unix_seconds, Some(1_000));
        assert_eq!(stalled.finalized_flat_seconds, Some(60));
        assert_eq!(stalled.tip_block_id, Some(131));
        // Still stalled 29 hours later.
        let later = rule.observe(1_000 + 29 * 3_600, Some(84), Some(1_286));
        assert_eq!(
            later.reason,
            Some(LezChainDegradedReasonV1::LezFinalityStalled)
        );
        assert_eq!(later.finalized_flat_seconds, Some(29 * 3_600));
    }

    #[test]
    fn a_chain_whose_tip_stands_still_too_is_idle_not_stalled() {
        let mut rule = LezFinalityRule::new(THRESHOLD);
        rule.observe(1_000, Some(84), Some(100));
        let idle = rule.observe(1_500, Some(84), Some(100));
        assert!(!idle.degraded, "{idle:?}");
        assert_eq!(idle.finalized_flat_seconds, Some(500));
    }

    #[test]
    fn an_unreadable_tip_does_not_prove_an_idle_chain() {
        // The sequencer stopped: no tip, no new blocks to finalize.
        let mut rule = LezFinalityRule::new(THRESHOLD);
        rule.observe(1_000, Some(84), Some(100));
        assert!(!rule.observe(1_030, Some(84), None).degraded);
        let stalled = rule.observe(1_061, Some(84), None);
        assert_eq!(
            stalled.reason,
            Some(LezChainDegradedReasonV1::LezFinalityStalled)
        );
        assert_eq!(stalled.tip_block_id, None);
    }

    #[test]
    fn finality_that_moves_again_recovers_at_once() {
        let mut rule = LezFinalityRule::new(THRESHOLD);
        rule.observe(1_000, Some(84), Some(100));
        assert!(rule.observe(1_200, Some(84), Some(200)).degraded);
        let recovered = rule.observe(1_210, Some(85), Some(205));
        assert!(!recovered.degraded, "{recovered:?}");
        assert_eq!(recovered.reason, None);
        assert_eq!(recovered.finalized_advanced_at_unix_seconds, Some(1_210));
        assert_eq!(recovered.finalized_flat_seconds, Some(0));
        // A new chain (lower id) is an advance as well.
        assert!(!rule.observe(1_400, Some(3), Some(4)).degraded);
        assert_eq!(
            rule.observe(1_400, Some(3), Some(4)).finalized_block_id,
            Some(3)
        );
    }

    #[test]
    fn an_unreachable_indexer_is_its_own_reason_and_keeps_the_last_known_id() {
        let mut rule = LezFinalityRule::new(THRESHOLD);
        rule.observe(1_000, Some(84), Some(100));
        let unreachable = rule.observe(1_010, None, Some(105));
        assert!(unreachable.degraded);
        assert_eq!(
            unreachable.reason,
            Some(LezChainDegradedReasonV1::LezIndexerUnreachable)
        );
        assert_eq!(unreachable.finalized_block_id, Some(84));
        assert_eq!(unreachable.finalized_flat_seconds, Some(10));
        // Before any answer at all there is nothing to report but the reason.
        let mut fresh = LezFinalityRule::new(THRESHOLD);
        let never = fresh.observe(1_000, None, None);
        assert_eq!(
            never.reason,
            Some(LezChainDegradedReasonV1::LezIndexerUnreachable)
        );
        assert_eq!(never.finalized_block_id, None);
        assert_eq!(never.finalized_flat_seconds, None);
        // The indexer answers again with the same id: flat since 1 000, so
        // past the threshold with a moving tip it is a stall, not unreachable.
        let back = rule.observe(1_070, Some(84), Some(140));
        assert_eq!(
            back.reason,
            Some(LezChainDegradedReasonV1::LezFinalityStalled)
        );
    }

    #[test]
    fn progress_serializes_snake_case_and_omits_an_absent_reason() {
        let mut rule = LezFinalityRule::new(THRESHOLD);
        let healthy = rule.observe(1_000, Some(84), Some(100));
        let value = serde_json::to_value(&healthy).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "degraded": false,
                "finalized_block_id": 84,
                "finalized_advanced_at_unix_seconds": 1_000,
                "finalized_flat_seconds": 0,
                "tip_block_id": 100,
                "stall_threshold_seconds": 60,
                "observed_at_unix_seconds": 1_000,
            })
        );
        let stalled = rule.observe(1_100, Some(84), None);
        let value = serde_json::to_value(&stalled).unwrap();
        assert_eq!(value["reason"], "lez_finality_stalled");
        assert_eq!(value["tip_block_id"], serde_json::Value::Null);
        let decoded: LezChainProgressV1 = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, stalled);
        assert_eq!(
            serde_json::to_value(LezChainDegradedReasonV1::LezIndexerUnreachable).unwrap(),
            "lez_indexer_unreachable"
        );
    }

    #[tokio::test]
    async fn a_watch_on_endpoints_that_do_not_answer_reports_the_indexer_unreachable() {
        // Port 9 on loopback: nothing listens, the connection is refused.
        let watch = LezFinalityWatch::new(
            "http://127.0.0.1:9/".to_owned(),
            "http://127.0.0.1:9/".to_owned(),
            THRESHOLD,
        );
        let progress = watch.progress().await;
        assert_eq!(
            progress.reason,
            Some(LezChainDegradedReasonV1::LezIndexerUnreachable)
        );
        // The second call inside the sample interval answers from the first.
        assert_eq!(watch.progress().await, progress);
    }
}
