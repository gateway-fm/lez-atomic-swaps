//! What the Maker Node's actor supervisor and the Taker Node's Zcash actor
//! supervisor say in the same words: the durable schedule states, how one
//! bounded attempt resolves, the failure-class alphabet, which actor phases
//! are absorbing, and the one secret-free line logged per supervised effect.
//!
//! The process mechanics differ by Node (the Maker runs its actors as sealed
//! child processes, the Taker runs its Zcash actor in-process under the swap's
//! lock), so only the vocabulary lives here. The Maker's `actor_supervisor/
//! runtime.rs` state machine (its own schedule and attempt types in
//! `lez-swap-store`) is the reference the Taker's `zec_supervisor.rs` mirrors
//! until the Maker adopts these shared types (a follow-up, not M2); each Node
//! keeps a test that every failure class it emits is in this alphabet
//! ([`validate_failure_class`]), so the two cannot drift apart in their words.

/// The failure-class literals a source file emits as `Retry("…")`,
/// `Fail("…")` or `failure_class: "…"`, for the per-Node alphabet tests.
#[must_use]
pub fn failure_class_literals(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    for marker in ["Retry(\"", "Fail(\""] {
        for (index, _) in source.match_indices(marker) {
            let rest = &source[index + marker.len()..];
            if let Some(end) = rest.find('"') {
                found.push(rest[..end].to_owned());
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Durable process-scheduler state of one supervised actor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleState {
    /// Ready for a worker attempt.
    Queued,
    /// Owned by one generation-fenced worker.
    Leased,
    /// Waiting for a bounded retry.
    Backoff,
    /// The actor reported an absorbing protocol outcome.
    Terminal,
    /// Operator attention is required; nothing is attempted until an action is queued.
    Failed,
}

impl ScheduleState {
    /// The fixed storage spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Leased => "leased",
            Self::Backoff => "backoff",
            Self::Terminal => "terminal",
            Self::Failed => "failed",
        }
    }

    /// Parses the fixed storage spelling.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "queued" => Self::Queued,
            "leased" => Self::Leased,
            "backoff" => Self::Backoff,
            "terminal" => Self::Terminal,
            "failed" => Self::Failed,
            _ => return None,
        })
    }

    /// Whether a worker may claim the actor once its retry time has come.
    #[must_use]
    pub const fn is_runnable(self) -> bool {
        matches!(self, Self::Queued | Self::Backoff)
    }
}

/// How one bounded attempt ended, as the schedule records it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttemptResolution {
    /// The actor is live; observe or drive it again at `not_before`.
    Requeue {
        /// Unix seconds.
        not_before: u64,
    },
    /// The attempt failed for a reason that a retry may clear.
    Backoff {
        /// Unix seconds.
        not_before: u64,
        /// One of the failure classes ([`validate_failure_class`]).
        failure_class: String,
    },
    /// The actor reported an absorbing outcome; nothing is attempted again.
    Terminal,
    /// The owner's queued action reached its absorbing outcome.
    ManualActionCompleted,
    /// The attempt failed for a reason no retry clears; the owner must act.
    Failed {
        /// One of the failure classes ([`validate_failure_class`]).
        failure_class: String,
    },
}

/// What one attempt's failure means for the schedule: a retry after the
/// backoff, or a stop until the owner acts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttemptFailure {
    /// Retry after the failure backoff.
    Retry(&'static str),
    /// Stop; the row reads as needing attention with this class.
    Fail(&'static str),
}

impl AttemptFailure {
    /// The failure class either way.
    #[must_use]
    pub const fn class(self) -> &'static str {
        match self {
            Self::Retry(class) | Self::Fail(class) => class,
        }
    }
}

/// Longest failure class the schedules store.
pub const MAX_FAILURE_CLASS_BYTES: usize = 64;

/// A failure class is a short lowercase identifier: `lez_depositor_balance_insufficient`,
/// `actor_timeout`, `coordinator_restarted`.
#[must_use]
pub fn validate_failure_class(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_FAILURE_CLASS_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// The absorbing actor phases: nothing is supervised after them.
#[must_use]
pub fn terminal_phase(phase: &str) -> bool {
    matches!(phase, "completed" | "refunded")
}

/// One secret-free line per supervised effect: the actor's outcome,
/// operation, phase, revision and next action, so an operator can see what a
/// swap's attempts return without reading the actor's journal. `role` is the
/// supervising Node's ("maker actor" / "taker actor"); `effect` is the actor's
/// JSON output, printed field by field, never whole.
pub fn log_effect(role: &str, swap_id: &str, command: &str, effect: &[u8]) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(effect) else {
        return;
    };
    log_effect_value(role, swap_id, command, &value);
}

/// As [`log_effect`], for an effect already parsed.
pub fn log_effect_value(role: &str, swap_id: &str, command: &str, value: &serde_json::Value) {
    let field = |name: &str| {
        value.get(name).map_or_else(
            || "-".to_owned(),
            |v| v.as_str().map_or_else(|| v.to_string(), str::to_owned),
        )
    };
    eprintln!(
        "{role} {swap_id}: {command} → {} {} (phase {}, revision {}, next {}){}",
        field("outcome"),
        field("operation"),
        field("phase"),
        field("revision"),
        field("next_action"),
        replans_suffix(value)
    );
}

/// The re-plans of a LEZ first lock, each with both nonces and both
/// transaction ids, as the effect's `replans` array reports them; empty when
/// the effect had none.
#[must_use]
pub fn replans_suffix(value: &serde_json::Value) -> String {
    let Some(replans) = value.get("replans").and_then(serde_json::Value::as_array) else {
        return String::new();
    };
    if replans.is_empty() {
        return String::new();
    }
    let text = |replan: &serde_json::Value, name: &str| {
        replan.get(name).map_or_else(
            || "-".to_owned(),
            |v| v.as_str().map_or_else(|| v.to_string(), str::to_owned),
        )
    };
    let items: Vec<String> = replans
        .iter()
        .map(|replan| {
            format!(
                "{} {} nonce {}→{} tx {}→{}",
                text(replan, "replan_index"),
                text(replan, "step"),
                text(replan, "superseded_nonce"),
                text(replan, "replanned_nonce"),
                text(replan, "superseded_transaction_id"),
                text(replan, "replanned_transaction_id")
            )
        })
        .collect();
    format!(" re-planned [{}]", items.join("; "))
}

#[cfg(test)]
mod tests {
    use super::{AttemptFailure, ScheduleState, terminal_phase, validate_failure_class};

    #[test]
    fn schedule_states_round_trip_and_only_queued_or_backoff_run() {
        for state in [
            ScheduleState::Queued,
            ScheduleState::Leased,
            ScheduleState::Backoff,
            ScheduleState::Terminal,
            ScheduleState::Failed,
        ] {
            assert_eq!(ScheduleState::parse(state.as_str()), Some(state));
            assert_eq!(
                state.is_runnable(),
                matches!(state, ScheduleState::Queued | ScheduleState::Backoff)
            );
        }
        assert_eq!(ScheduleState::parse("running"), None);
    }

    #[test]
    fn failure_classes_are_short_lowercase_identifiers() {
        assert!(validate_failure_class("lez_depositor_balance_insufficient"));
        assert!(validate_failure_class("lez_first_lock_replan_exhausted"));
        assert!(validate_failure_class("actor_timeout"));
        assert!(!validate_failure_class(""));
        assert!(!validate_failure_class("Actor-Timeout"));
        assert!(!validate_failure_class(&"a".repeat(65)));
        assert_eq!(
            AttemptFailure::Fail("actor_deployment_invalid").class(),
            "actor_deployment_invalid"
        );
    }

    #[test]
    fn failure_class_literals_are_read_from_source() {
        let source = r#"Err(ChildRunError::Retry("actor_timeout")) ; AttemptFailure::Fail("actor_deployment_invalid") ; Retry("actor_timeout")"#;
        assert_eq!(
            super::failure_class_literals(source),
            vec![
                "actor_deployment_invalid".to_owned(),
                "actor_timeout".to_owned()
            ]
        );
    }

    #[test]
    fn only_completed_and_refunded_are_terminal() {
        assert!(terminal_phase("completed"));
        assert!(terminal_phase("refunded"));
        assert!(!terminal_phase("claim_evidence_available"));
        assert!(!terminal_phase("offered"));
    }
}
