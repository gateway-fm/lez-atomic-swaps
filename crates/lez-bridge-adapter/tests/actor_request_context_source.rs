//! Contract for actor-owned random request IDs and authority-owned scan windows.

#![forbid(unsafe_code)]

use std::{
    collections::HashSet,
    convert::Infallible,
    fmt,
    sync::{Arc, Mutex},
};

use lez_bridge_adapter::{
    ActorBridgeRequestContextError, ActorBridgeRequestContextSource, BridgeDiscoveryWindowSource,
    BridgeRequestContextSource,
};
use lez_bridge_protocol::{DiscoveryWindow, RunId};
use lez_swap_core::{Participant, SwapId};
use lez_swap_store::{BridgeOperationKey, BridgeOperationKind};

const ALL_OPERATIONS: [BridgeOperationKind; 14] = [
    BridgeOperationKind::NativeEscrowPrepare,
    BridgeOperationKind::NativeEscrowExactObserve,
    BridgeOperationKind::NativeEscrowDiscoveryObserve,
    BridgeOperationKind::NativeEscrowInitializeSubmit,
    BridgeOperationKind::NativeEscrowFundSubmit,
    BridgeOperationKind::RevealingClaimPrepare,
    BridgeOperationKind::RevealingClaimExactObserve,
    BridgeOperationKind::RevealingClaimDiscoveryObserve,
    BridgeOperationKind::RevealingClaimSubmit,
    BridgeOperationKind::NativeRefundPrepare,
    BridgeOperationKind::NativeRefundEligibilityObserve,
    BridgeOperationKind::NativeRefundExactObserve,
    BridgeOperationKind::NativeRefundDiscoveryObserve,
    BridgeOperationKind::NativeRefundSubmit,
];

#[test]
fn os_random_request_ids_are_safe_and_fresh() {
    let source = ActorBridgeRequestContextSource::new(FixedWindows::default());
    let key = operation_key(BridgeOperationKind::NativeEscrowPrepare);
    let mut unique = HashSet::new();

    for _ in 0..512 {
        let request = source.next_request(&key).expect("random request context");
        let request_id = request.request_id().as_str();
        assert_eq!(request_id.len(), 36);
        assert!(request_id.starts_with("req-"));
        assert!(
            request_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        );
        assert!(unique.insert(request_id.to_owned()), "request ID collision");
        assert_eq!(request.discovery_window(), None);
    }
}

#[test]
fn every_operation_gets_exactly_its_required_window_shape() {
    let windows = FixedWindows::default();
    let calls = Arc::clone(&windows.calls);
    let source = ActorBridgeRequestContextSource::new(windows);

    for operation in ALL_OPERATIONS {
        let request = source
            .next_request(&operation_key(operation))
            .expect("operation request context");
        assert_eq!(
            request.discovery_window(),
            window_bearing(operation).then_some(window()),
            "wrong discovery-window shape for {operation:?}"
        );
    }

    assert_eq!(
        *calls.lock().expect("window-source calls"),
        vec![
            BridgeOperationKind::NativeEscrowDiscoveryObserve,
            BridgeOperationKind::RevealingClaimDiscoveryObserve,
            // The existing refund protocol scans a bounded window even when
            // matching the exact prepared refund identity.
            BridgeOperationKind::NativeRefundExactObserve,
            BridgeOperationKind::NativeRefundDiscoveryObserve,
        ]
    );
}

#[test]
fn authority_failure_is_typed_and_redacted_and_non_window_operations_do_not_call_it() {
    let source = ActorBridgeRequestContextSource::new(SensitiveFailingWindows);

    let _ = source
        .next_request(&operation_key(BridgeOperationKind::RevealingClaimPrepare))
        .expect("non-window operation bypasses window authority");
    let error = source
        .next_request(&operation_key(
            BridgeOperationKind::RevealingClaimDiscoveryObserve,
        ))
        .expect_err("window authority failure is retained as a category");
    assert!(matches!(
        error,
        ActorBridgeRequestContextError::DiscoveryWindowUnavailable
    ));

    for diagnostic in [format!("{error:?}"), error.to_string()] {
        assert!(!diagnostic.contains("window-source-secret"));
        assert!(!diagnostic.contains("authority-internal-path"));
    }
}

#[test]
fn context_source_debug_redacts_the_injected_authority() {
    let source = ActorBridgeRequestContextSource::new(SensitiveFailingWindows);
    let diagnostic = format!("{source:?}");

    assert!(diagnostic.contains("ActorBridgeRequestContextSource"));
    assert!(diagnostic.contains("[REDACTED]"));
    assert!(!diagnostic.contains("window-source-secret"));
    assert!(!diagnostic.contains("authority-internal-path"));
}

#[derive(Default)]
struct FixedWindows {
    calls: Arc<Mutex<Vec<BridgeOperationKind>>>,
}

impl BridgeDiscoveryWindowSource for FixedWindows {
    type Error = Infallible;

    fn discovery_window(&self, key: &BridgeOperationKey) -> Result<DiscoveryWindow, Self::Error> {
        self.calls
            .lock()
            .expect("window-source calls")
            .push(key.operation());
        Ok(window())
    }
}

struct SensitiveFailingWindows;

impl fmt::Debug for SensitiveFailingWindows {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("window-source-secret authority-internal-path")
    }
}

#[derive(Debug)]
struct SensitiveWindowError;

impl fmt::Display for SensitiveWindowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("window-source-secret authority-internal-path")
    }
}

impl std::error::Error for SensitiveWindowError {}

impl BridgeDiscoveryWindowSource for SensitiveFailingWindows {
    type Error = SensitiveWindowError;

    fn discovery_window(&self, _key: &BridgeOperationKey) -> Result<DiscoveryWindow, Self::Error> {
        Err(SensitiveWindowError)
    }
}

fn operation_key(operation: BridgeOperationKind) -> BridgeOperationKey {
    BridgeOperationKey::new(
        RunId::new("actor-context-run").expect("run id"),
        SwapId::new("actor-context-swap").expect("swap id"),
        Participant::Maker,
        operation,
    )
}

fn window_bearing(operation: BridgeOperationKind) -> bool {
    matches!(
        operation,
        BridgeOperationKind::NativeEscrowDiscoveryObserve
            | BridgeOperationKind::RevealingClaimDiscoveryObserve
            | BridgeOperationKind::NativeRefundExactObserve
            | BridgeOperationKind::NativeRefundDiscoveryObserve
    )
}

fn window() -> DiscoveryWindow {
    DiscoveryWindow::new(1_000, 64).expect("bounded discovery window")
}
