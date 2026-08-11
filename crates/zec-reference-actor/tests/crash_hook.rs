#![cfg(feature = "test-crash-hooks")]

use std::{fs, os::unix::fs::PermissionsExt as _};

use tempfile::tempdir;
use zec_reference_actor::{TestCrashHookError, arm_test_crash_hook};

const SUBMITTED: &str = r#"{"schema_version":1,"role":"maker","command":"drive","outcome":"submitted","operation":"zcash_fund"}"#;
const XMR_REFUND_SUBMITTED: &str = r#"{"schema_version":1,"role":"maker","command":"recover","outcome":"submitted","operation":"sweep_monero_refund"}"#;
const XMR_TAG15_SUBMITTED: &str = r#"{"schema_version":1,"role":"maker","command":"claim","outcome":"submitted","operation":"claim_lez_tag15"}"#;

#[test]
fn hook_arms_only_for_the_exact_submitted_operation_and_writes_private_marker() {
    let root = tempdir().expect("private hook root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let marker = root.path().join("actor-paused.json");

    assert!(!arm_test_crash_hook(
        "zcash_fund",
        &marker,
        "swap-a",
        "maker",
        r#"{"schema_version":1,"role":"maker","command":"drive","outcome":"awaiting_observation","operation":"zcash_fund"}"#,
    )
    .expect("non-submitted result is ignored"));
    assert!(
        arm_test_crash_hook("zcash_fund", &marker, "swap-a", "maker", SUBMITTED,)
            .expect("exact submitted result arms")
    );
    assert_eq!(
        fs::metadata(&marker).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    assert_eq!(value["swap_id"], "swap-a");
    assert_eq!(value["role"], "maker");
    assert_eq!(value["operation"], "zcash_fund");

    assert_eq!(
        arm_test_crash_hook("zcash_fund", &marker, "swap-a", "maker", SUBMITTED),
        Err(TestCrashHookError::UnsafeMarker),
        "an existing marker must never be replaced"
    );
}

#[test]
fn hook_rejects_unknown_malformed_or_unsafe_requests() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let marker = root.path().join("actor-paused.json");

    assert_eq!(
        arm_test_crash_hook("unknown", &marker, "swap-a", "maker", SUBMITTED),
        Err(TestCrashHookError::InvalidRequest)
    );
    assert_eq!(
        arm_test_crash_hook("zcash_fund", &marker, "swap-a", "maker", "not-json"),
        Err(TestCrashHookError::InvalidRequest)
    );
    assert_eq!(
        arm_test_crash_hook("zcash_fund", &marker, "swap-a", "other", SUBMITTED),
        Err(TestCrashHookError::InvalidRequest)
    );

    let unsafe_parent = root.path().join("unsafe");
    fs::create_dir(&unsafe_parent).unwrap();
    fs::set_permissions(&unsafe_parent, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        arm_test_crash_hook(
            "zcash_fund",
            &unsafe_parent.join("marker.json"),
            "swap-a",
            "maker",
            SUBMITTED,
        ),
        Err(TestCrashHookError::UnsafeMarker)
    );
}

#[test]
fn hook_arms_for_exact_monero_refund_recovery_only() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let marker = root.path().join("xmr-refund-paused.json");

    assert!(
        arm_test_crash_hook(
            "sweep_monero_refund",
            &marker,
            "swap-xmr",
            "maker",
            XMR_REFUND_SUBMITTED,
        )
        .expect("exact submitted XMR recovery arms")
    );
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    assert_eq!(value["operation"], "sweep_monero_refund");
    assert_eq!(value["state"], "paused_after_submitted_before_stdout");

    let other_marker = root.path().join("wrong-command.json");
    assert_eq!(
        arm_test_crash_hook(
            "sweep_monero_refund",
            &other_marker,
            "swap-xmr",
            "maker",
            r#"{"schema_version":1,"role":"maker","command":"drive","outcome":"submitted","operation":"sweep_monero_refund"}"#,
        ),
        Err(TestCrashHookError::InvalidRequest),
        "the XMR refund hook must never arm for a drive command"
    );
}

#[test]
fn hook_arms_for_exact_monero_tag15_claim_only() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let marker = root.path().join("xmr-tag15-paused.json");

    assert!(
        arm_test_crash_hook(
            "claim_lez_tag15",
            &marker,
            "swap-claim",
            "maker",
            XMR_TAG15_SUBMITTED,
        )
        .expect("exact submitted Tag15 Claim arms")
    );
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    assert_eq!(value["operation"], "claim_lez_tag15");
    assert_eq!(value["state"], "paused_after_submitted_before_stdout");
}
