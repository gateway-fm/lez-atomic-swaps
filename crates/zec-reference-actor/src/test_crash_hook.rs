use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
};

use serde_json::{Value, json};

/// Stable failure from the compile-time-gated process crash seam.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TestCrashHookError {
    /// The requested operation or serialized actor result is not admissible.
    #[error("test crash hook request is invalid")]
    InvalidRequest,
    /// The marker parent or marker file is not owner-private and no-clobber.
    #[error("test crash hook marker is unsafe")]
    UnsafeMarker,
    /// The marker could not be completely written and synchronized.
    #[error("test crash hook marker is unavailable")]
    MarkerUnavailable,
}

/// Writes a private marker only after an exact submitted actor effect.
///
/// The caller parks the process after this returns `true`; an external test
/// coordinator can then prove exact process identity before sending `SIGKILL`.
/// This function is absent unless the `test-crash-hooks` feature is selected.
///
/// # Errors
///
/// Fails closed for an unknown operation, malformed or mismatched actor output,
/// a non-private marker parent, an existing marker, or an incomplete write.
pub fn arm_test_crash_hook(
    operation: &str,
    marker: &Path,
    swap_id: &str,
    role: &str,
    output: &str,
) -> Result<bool, TestCrashHookError> {
    if !matches!(
        operation,
        "lez_initialize"
            | "lez_fund"
            | "zcash_fund"
            | "lez_revealing_claim"
            | "zcash_followup_claim"
    ) || !matches!(role, "maker" | "taker")
    {
        return Err(TestCrashHookError::InvalidRequest);
    }
    let value: Value =
        serde_json::from_str(output).map_err(|_| TestCrashHookError::InvalidRequest)?;
    if value.get("schema_version") != Some(&Value::from(1))
        || value.get("role") != Some(&Value::from(role))
        || value.get("command") != Some(&Value::from("drive"))
    {
        return Err(TestCrashHookError::InvalidRequest);
    }
    if value.get("outcome") != Some(&Value::from("submitted"))
        || value.get("operation") != Some(&Value::from(operation))
    {
        return Ok(false);
    }

    let parent = marker.parent().ok_or(TestCrashHookError::UnsafeMarker)?;
    if !marker.is_absolute() || marker.file_name().is_none() {
        return Err(TestCrashHookError::UnsafeMarker);
    }
    let metadata = fs::symlink_metadata(parent).map_err(|_| TestCrashHookError::UnsafeMarker)?;
    let current_uid = fs::metadata("/proc/self")
        .map_err(|_| TestCrashHookError::UnsafeMarker)?
        .uid();
    if !metadata.file_type().is_dir()
        || metadata.permissions().mode() & 0o7777 != 0o700
        || metadata.uid() != current_uid
        || fs::canonicalize(parent).map_err(|_| TestCrashHookError::UnsafeMarker)? != parent
    {
        return Err(TestCrashHookError::UnsafeMarker);
    }

    let bytes = serde_json::to_vec(&json!({
        "schema_version": 1,
        "state": "paused_after_submitted_before_stdout",
        "process_id": std::process::id(),
        "swap_id": swap_id,
        "role": role,
        "operation": operation,
    }))
    .map_err(|_| TestCrashHookError::MarkerUnavailable)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(marker)
        .map_err(|_| TestCrashHookError::UnsafeMarker)?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| TestCrashHookError::MarkerUnavailable)?;
    Ok(true)
}
