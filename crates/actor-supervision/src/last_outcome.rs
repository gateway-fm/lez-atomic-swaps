//! What a swap's actor last did, or why it did not act, kept beside the
//! swap's state (#103 item 6). A refund the actor holds until finalized LEZ
//! time passes the agreement's cutoff read for hours as a bare
//! `refund_in_progress`, the same as a dead Node; with this the owner API and
//! the desks say what the actor is waiting for and until when.
//!
//! The facts come from the actor's own output: its effect JSON (`outcome`,
//! `operation`, `revision`, and `wait` when it held a step for a reason it
//! can bound), or the failure class the supervising Node named. Nothing here
//! reads a chain. Both Nodes keep the last outcome in memory per swap
//! ([`LastOutcomes`]); a restarted Node has none until its next attempt, a
//! few seconds later.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The `outcome` of a failed attempt; [`ActorLastOutcomeV1::failure_class`]
/// names why.
pub const FAILED_OUTCOME: &str = "failed";

/// How many swaps' last outcomes a Node keeps; the oldest goes first.
pub const MAX_LAST_OUTCOMES: usize = 1024;

/// What the actor's last attempt waited for, with the bound it read, when it
/// held a step for a reason it can name. Every bound is the chain's own
/// reading at that attempt, never the Node's wall clock.
///
/// `kind` is one of:
/// - `lez_finality_past_cutoff`: the Taker's first-lock refund is held until
///   finalized LEZ time passes the Maker's second-lock cutoff, the only proof
///   that the Maker's LEZ lock can no longer land (`until_unix_seconds` the
///   cutoff, `chain_unix_seconds` the finalized LEZ time read);
/// - `bitcoin_time_past_cutoff`: the same with the Maker's lock on Bitcoin,
///   held until the stable tip's median time passes the cutoff
///   (`chain_height` the stable tip);
/// - `bitcoin_lock_confirmations`: the counterparty's Bitcoin lock is seen but
///   not yet confirmed deep enough;
/// - `bitcoin_refund_maturity`: the Bitcoin refund's relative timelock
///   (`until_height` the first block that may hold it, `chain_height` the
///   stable tip);
/// - `bitcoin_refund_confirmations`: the refund is sent and confirming
///   (`confirmations` of `required_confirmations`);
/// - `lez_finality_past_refund_time`: a LEZ refund is held until finalized LEZ
///   time passes the agreement's LEZ refund time;
/// - `maker_lock`: the Maker's second lock, until its cutoff;
/// - `counterparty_claim`: the other party's claim, until the claim window
///   closes when the agreement bounds it;
/// - `lez_refund_time`: a Zcash swap's LEZ refund waits for the agreement's
///   LEZ refund time (`until_unix_seconds`, when the actor's status names it);
/// - `zcash_refund_height`: a Zcash swap's Zcash refund waits for its refund
///   height (no bound: the actor does not report the height).
///
/// New kinds may be added; a reader shows an unknown kind by its bounds.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorWaitV1 {
    /// What is waited for (see the type's documentation).
    pub kind: String,
    /// The chain whose progress ends the wait: `lez`, `bitcoin` or `zcash`.
    pub chain: String,
    /// The time the chain's clock must pass, Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_unix_seconds: Option<u64>,
    /// The chain's clock as the actor read it (finalized LEZ time, Bitcoin
    /// median time), Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_unix_seconds: Option<u64>,
    /// The height the chain must reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_height: Option<u64>,
    /// The chain's height as the actor read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_height: Option<u64>,
    /// Confirmations seen so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmations: Option<u32>,
    /// Confirmations the agreement requires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_confirmations: Option<u32>,
}

impl ActorWaitV1 {
    /// A wait of `kind` on `chain` with no bound yet.
    #[must_use]
    pub fn new(kind: &str, chain: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            chain: chain.to_owned(),
            ..Self::default()
        }
    }

    /// Until the chain's clock passes `until`, read at `chain_time`.
    #[must_use]
    pub const fn until_time(mut self, until: u64, chain_time: Option<u64>) -> Self {
        self.until_unix_seconds = Some(until);
        self.chain_unix_seconds = chain_time;
        self
    }

    /// Until the chain reaches `until`, read at `chain_height`.
    #[must_use]
    pub const fn until_height(mut self, until: u64, chain_height: Option<u64>) -> Self {
        self.until_height = Some(until);
        self.chain_height = chain_height;
        self
    }

    /// The chain's height as the actor read it, with no height bound.
    #[must_use]
    pub const fn at_height(mut self, chain_height: u64) -> Self {
        self.chain_height = Some(chain_height);
        self
    }

    /// `confirmations` of `required` seen.
    #[must_use]
    pub const fn confirming(mut self, confirmations: u32, required: u32) -> Self {
        self.confirmations = Some(confirmations);
        self.required_confirmations = Some(required);
        self
    }

    fn is_well_formed(&self) -> bool {
        valid_label(&self.kind) && valid_label(&self.chain)
    }
}

/// The last thing a swap's actor did, or why it did not act, and when.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorLastOutcomeV1 {
    /// When the Node got this outcome, Unix seconds.
    pub at_unix_seconds: u64,
    /// The actor step that ran: `activate`, `drive`, `claim` or `recover`
    /// (`status` when the actor could not even be read).
    pub step: String,
    /// The actor's outcome word (`awaiting_observation`,
    /// `observed_then_projected`, `submitted`, `awaiting_deadline`, …), or
    /// `failed` with [`Self::failure_class`].
    pub outcome: String,
    /// The step the outcome is about, when the actor names one (Zcash:
    /// `lez_refund`, `zcash_refund`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    /// The actor's revision after the attempt, when it reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    /// Why the attempt failed (the shared failure-class alphabet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<String>,
    /// What the actor waits for, when it held a step for a reason it can bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<ActorWaitV1>,
}

impl ActorLastOutcomeV1 {
    /// The outcome of one effect from the actor's JSON output; `None` when the
    /// output names no outcome. A `wait` the actor attached is kept when it is
    /// well formed and dropped otherwise (it is reporting, never a decision).
    #[must_use]
    pub fn from_effect(step: &str, effect: &serde_json::Value, at: u64) -> Option<Self> {
        let outcome = effect.get("outcome")?.as_str().filter(|o| valid_label(o))?;
        let label = |name: &str| {
            effect
                .get(name)
                .and_then(serde_json::Value::as_str)
                .filter(|value| valid_label(value))
                .map(str::to_owned)
        };
        let waiting_for = effect
            .get("wait")
            .and_then(|wait| serde_json::from_value::<ActorWaitV1>(wait.clone()).ok())
            .filter(ActorWaitV1::is_well_formed);
        Some(Self {
            at_unix_seconds: at,
            step: step.to_owned(),
            outcome: outcome.to_owned(),
            operation: label("operation"),
            revision: effect.get("revision").and_then(serde_json::Value::as_u64),
            failure_class: None,
            waiting_for,
        })
    }

    /// A failed attempt of `step`, named by its failure class.
    #[must_use]
    pub fn failed(step: &str, failure_class: &str, at: u64) -> Self {
        Self {
            at_unix_seconds: at,
            step: step.to_owned(),
            outcome: FAILED_OUTCOME.to_owned(),
            operation: None,
            revision: None,
            failure_class: Some(failure_class.to_owned()),
            waiting_for: None,
        }
    }

    /// Adds a wait the supervising Node knows and the actor did not name.
    #[must_use]
    pub fn waiting(mut self, wait: ActorWaitV1) -> Self {
        if self.waiting_for.is_none() && self.failure_class.is_none() {
            self.waiting_for = Some(wait);
        }
        self
    }

    /// Whether the attempt failed.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        self.failure_class.is_some()
    }
}

/// The last outcome of each swap a Node supervises, in memory.
#[derive(Debug, Default)]
pub struct LastOutcomes {
    by_swap: Mutex<HashMap<String, ActorLastOutcomeV1>>,
}

impl LastOutcomes {
    /// An empty record.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Keeps `outcome` as `swap_id`'s last. Past [`MAX_LAST_OUTCOMES`] swaps
    /// the oldest outcome is forgotten first.
    pub fn record(&self, swap_id: &str, outcome: ActorLastOutcomeV1) {
        let Ok(mut by_swap) = self.by_swap.lock() else {
            return;
        };
        if by_swap.len() >= MAX_LAST_OUTCOMES
            && !by_swap.contains_key(swap_id)
            && let Some(oldest) = by_swap
                .iter()
                .min_by_key(|(_, kept)| kept.at_unix_seconds)
                .map(|(id, _)| id.clone())
        {
            by_swap.remove(&oldest);
        }
        by_swap.insert(swap_id.to_owned(), outcome);
    }

    /// `swap_id`'s last outcome, when this Node has one.
    #[must_use]
    pub fn get(&self, swap_id: &str) -> Option<ActorLastOutcomeV1> {
        self.by_swap.lock().ok()?.get(swap_id).cloned()
    }
}

/// A short lowercase identifier, as the actors spell their words.
fn valid_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::{ActorLastOutcomeV1, ActorWaitV1, LastOutcomes, MAX_LAST_OUTCOMES};
    use serde_json::json;

    #[test]
    fn an_effect_with_a_wait_keeps_the_wait_and_its_bounds() {
        let effect = json!({
            "schema_version": 1, "role": "taker", "command": "recover",
            "outcome": "awaiting_observation", "chain": "bitcoin",
            "phase": "taker_lock_confirmed", "revision": 1, "next_action": "recover_taker_leg",
            "wait": {"kind": "lez_finality_past_cutoff", "chain": "lez",
                     "until_unix_seconds": 1_762_877_100_u64, "chain_unix_seconds": 1_762_876_680_u64}
        });
        let outcome = ActorLastOutcomeV1::from_effect("recover", &effect, 1_762_876_700).unwrap();
        assert_eq!(outcome.outcome, "awaiting_observation");
        assert_eq!(outcome.revision, Some(1));
        assert!(!outcome.is_failure());
        let wait = outcome.waiting_for.unwrap();
        assert_eq!(wait.kind, "lez_finality_past_cutoff");
        assert_eq!(wait.until_unix_seconds, Some(1_762_877_100));
        assert_eq!(wait.chain_unix_seconds, Some(1_762_876_680));
        assert_eq!(wait.until_height, None);
    }

    #[test]
    fn an_effect_without_a_wait_has_none_and_a_zcash_operation_is_kept() {
        let effect = json!({"outcome": "awaiting_deadline", "operation": "lez_refund",
                            "phase": "both_legs_locked", "revision": 3});
        let outcome = ActorLastOutcomeV1::from_effect("recover", &effect, 10).unwrap();
        assert_eq!(outcome.operation.as_deref(), Some("lez_refund"));
        assert_eq!(outcome.waiting_for, None);
        // Nothing to name without an outcome; a malformed wait is dropped.
        assert_eq!(
            ActorLastOutcomeV1::from_effect("drive", &json!({}), 10),
            None
        );
        let odd = json!({"outcome": "awaiting_observation", "wait": {"kind": "Not A Label", "chain": "lez"}});
        assert_eq!(
            ActorLastOutcomeV1::from_effect("drive", &odd, 10)
                .unwrap()
                .waiting_for,
            None
        );
    }

    #[test]
    fn a_failure_names_its_class_and_never_carries_a_wait() {
        let failed = ActorLastOutcomeV1::failed("recover", "actor_observation_unavailable", 5)
            .waiting(ActorWaitV1::new("maker_lock", "lez").until_time(9, None));
        assert!(failed.is_failure());
        assert_eq!(failed.outcome, "failed");
        assert_eq!(failed.waiting_for, None);
    }

    #[test]
    fn the_serialized_shape_omits_what_is_unknown_and_reads_back() {
        let outcome = ActorLastOutcomeV1::from_effect(
            "recover",
            &json!({"outcome": "awaiting_observation", "revision": 3}),
            7,
        )
        .unwrap()
        .waiting(
            ActorWaitV1::new("bitcoin_refund_maturity", "bitcoin").until_height(1234, Some(1229)),
        );
        let value = serde_json::to_value(&outcome).unwrap();
        assert_eq!(
            value,
            json!({"at_unix_seconds": 7, "step": "recover", "outcome": "awaiting_observation",
                   "revision": 3,
                   "waiting_for": {"kind": "bitcoin_refund_maturity", "chain": "bitcoin",
                                   "until_height": 1234, "chain_height": 1229}})
        );
        assert_eq!(
            serde_json::from_value::<ActorLastOutcomeV1>(value).unwrap(),
            outcome
        );
    }

    #[test]
    fn the_record_keeps_one_outcome_per_swap_and_forgets_the_oldest_first() {
        let outcomes = LastOutcomes::new();
        outcomes.record("a", ActorLastOutcomeV1::failed("drive", "actor_timeout", 1));
        outcomes.record(
            "a",
            ActorLastOutcomeV1::failed("drive", "actor_lock_unavailable", 2),
        );
        assert_eq!(
            outcomes.get("a").unwrap().failure_class.as_deref(),
            Some("actor_lock_unavailable")
        );
        for index in 1..MAX_LAST_OUTCOMES {
            outcomes.record(
                &format!("s{index}"),
                ActorLastOutcomeV1::failed("drive", "actor_timeout", 100 + index as u64),
            );
        }
        assert!(outcomes.get("a").is_some());
        outcomes.record(
            "new",
            ActorLastOutcomeV1::failed("drive", "actor_timeout", 9_999),
        );
        assert_eq!(outcomes.get("a"), None, "the oldest outcome goes first");
        assert!(outcomes.get("new").is_some());
    }
}
