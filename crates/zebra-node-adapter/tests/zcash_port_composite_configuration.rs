//! Checked construction and shared configuration for the Zebra Zcash composite.

#![forbid(unsafe_code)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use lez_swap_core::Participant;
use lez_zebra_node_adapter::{
    RoleKeyedZcashSigner, ZebraChainIdentity, ZebraRpcZcashPort, ZebraRpcZcashPortConfigError,
};
use lez_zec_swap_sdk::{ZecProfileId, ZecRefundProfile};
use secp256k1::SecretKey;

#[test]
fn wrong_signer_role_fails_before_rpc_clone() {
    let identity = ZebraChainIdentity::deterministic_regtest_nu6_2();
    let clones = Arc::new(AtomicUsize::new(0));
    let rpc = CloneProbe::new(clones.clone());
    let wrong_signer = RoleKeyedZcashSigner::new(Participant::Taker, key(2));

    let error = ZebraRpcZcashPort::new(rpc, wrong_signer, identity, Participant::Maker)
        .expect_err("wrong signer role fails before cloning RPC");

    assert_eq!(
        error,
        ZebraRpcZcashPortConfigError::SignerRoleMismatch {
            expected: Participant::Maker,
            actual: Participant::Taker,
        }
    );
    assert_eq!(clones.load(Ordering::Relaxed), 0);
}

#[test]
fn one_identity_role_and_scan_horizon_are_shared_and_cloneable() {
    let identity = ZebraChainIdentity::deterministic_regtest_nu6_2();
    let clones = Arc::new(AtomicUsize::new(0));
    let rpc = CloneProbe::new(clones.clone());
    let signer = RoleKeyedZcashSigner::new(Participant::Maker, key(1));
    let port = ZebraRpcZcashPort::new(rpc, signer, identity, Participant::Maker)
        .expect("role-matched composite");

    assert_eq!(clones.load(Ordering::Relaxed), 1, "one shared RPC clone");
    assert_eq!(port.identity(), identity);
    assert_eq!(port.local_participant(), Participant::Maker);
    assert_eq!(
        port.counterparty_scan_blocks(),
        ZecRefundProfile::for_id(ZecProfileId::PublicTestnetV1)
            .zcash_refund_blocks()
            .saturating_add(1)
    );

    let duplicate = port.clone();
    assert_eq!(duplicate.identity(), port.identity());
    assert_eq!(duplicate.local_participant(), port.local_participant());
    assert_eq!(
        duplicate.counterparty_scan_blocks(),
        port.counterparty_scan_blocks()
    );
    assert_eq!(
        port.clone()
            .with_counterparty_scan_blocks(0)
            .counterparty_scan_blocks(),
        1
    );
    assert_eq!(
        port.with_counterparty_scan_blocks(77)
            .counterparty_scan_blocks(),
        77
    );
}

#[test]
fn composite_debug_redacts_rpc_and_signer_capabilities() {
    let identity = ZebraChainIdentity::deterministic_regtest_nu6_2();
    let rpc = CloneProbe::new(Arc::new(AtomicUsize::new(0)));
    let signer = RoleKeyedZcashSigner::new(Participant::Maker, key(1));
    let port = ZebraRpcZcashPort::new(rpc, signer, identity, Participant::Maker)
        .expect("role-matched composite");

    let diagnostic = format!("{port:?}");
    assert!(diagnostic.contains("ZebraRpcZcashPort"));
    assert!(diagnostic.contains("[REDACTED]"));
    assert!(!diagnostic.contains(CloneProbe::PRIVATE_MARKER));
    assert!(!diagnostic.contains(&hex::encode([1_u8; 32])));
}

struct CloneProbe {
    clones: Arc<AtomicUsize>,
    private_marker: &'static str,
}

impl CloneProbe {
    const PRIVATE_MARKER: &'static str = "private-rpc-capability-marker";

    fn new(clones: Arc<AtomicUsize>) -> Self {
        Self {
            clones,
            private_marker: Self::PRIVATE_MARKER,
        }
    }
}

impl Clone for CloneProbe {
    fn clone(&self) -> Self {
        self.clones.fetch_add(1, Ordering::Relaxed);
        Self {
            clones: self.clones.clone(),
            private_marker: self.private_marker,
        }
    }
}

fn key(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).expect("valid test key")
}
