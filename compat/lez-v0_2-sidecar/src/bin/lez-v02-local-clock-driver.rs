use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::OpenOptionsExt as _,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, ensure};
use clap::Parser;
use lez_bridge_client::{BridgeClient, BridgeClientConfig, SidecarCapability};
use lez_bridge_protocol::{
    ChainClock, Hex32, MessageContext, ObserveFinalizedClockRequest, Participant,
    PrepareCurrentProfileClockRequest, RequestId, RunId, RuntimeDescriptor,
    SubmitTransactionRequest, VerifyCurrentProfileClockRequest, XmrNativeEscrowTermsV3,
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

#[derive(Debug, Parser)]
struct Arguments {
    #[arg(long)]
    sidecar_endpoint: String,
    #[arg(long)]
    capability_file: PathBuf,
    #[arg(long)]
    runtime_file: PathBuf,
    #[arg(long)]
    terms_file: PathBuf,
    #[arg(long)]
    run_id: String,
    #[arg(long)]
    recipient_account_id: String,
    #[arg(long)]
    exclusive_punish_at_ms: u64,
    #[arg(long, default_value_t = 60)]
    finality_timeout_seconds: u64,
    #[arg(long)]
    output_evidence: PathBuf,
}

#[derive(Clone, Copy, Debug)]
struct FinalizedClockWait<'a> {
    identity: &'a [u8; 32],
    request_domain: &'a [u8],
    minimum_height: u64,
    exclusive_punish_at_ms: u64,
    timeout: Duration,
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    ensure!(
        !arguments.output_evidence.exists(),
        "output evidence already exists"
    );
    ensure!(
        (30..=120).contains(&arguments.finality_timeout_seconds),
        "finality timeout must be 30..=120 seconds"
    );
    let runtime: RuntimeDescriptor = read_json(&arguments.runtime_file)?;
    let terms: XmrNativeEscrowTermsV3 = read_json(&arguments.terms_file)?;
    let run_id = RunId::new(arguments.run_id).context("invalid run ID")?;
    let recipient_account_id =
        Hex32::from_hex(&arguments.recipient_account_id).context("invalid recipient account ID")?;
    let terms_input = terms.to_input();
    ensure!(
        runtime.sidecar_role == Participant::Taker,
        "Taker sidecar required"
    );
    ensure!(
        runtime.signer_account_id == terms_input.depositor_account_id
            && recipient_account_id == terms_input.claimant_account_id
            && arguments.exclusive_punish_at_ms == terms_input.punish_at_ms,
        "runtime, recipient, or cutoff differs from activated terms"
    );

    let client = BridgeClient::connect(BridgeClientConfig::new(
        arguments.sidecar_endpoint.clone(),
        read_capability(&arguments.capability_file)?,
        run_id.clone(),
        runtime.clone(),
        Duration::from_secs(90),
    ))
    .context("connect authenticated Taker sidecar")?;
    let observer = BridgeClient::connect(BridgeClientConfig::new(
        arguments.sidecar_endpoint,
        read_capability(&arguments.capability_file)?,
        run_id.clone(),
        runtime.clone(),
        Duration::from_secs(5),
    ))
    .context("connect authenticated finalized-clock observer")?;
    let (finalized_clock_before, finalized_observation_attempts_before) = observe_finalized_clock(
        &observer,
        &run_id,
        &runtime,
        FinalizedClockWait {
            identity: terms_input.swap_id.as_bytes(),
            request_domain: b"finalized-before",
            minimum_height: 0,
            exclusive_punish_at_ms: arguments.exclusive_punish_at_ms,
            timeout: Duration::from_secs(15),
        },
    )
    .await
    .context("observe stable finalized clock before local transaction")?;
    let preparation_request_id = clock_request_id(b"prepare", terms_input.swap_id.as_bytes())?;
    let preparation = client
        .prepare_current_profile_clock(PrepareCurrentProfileClockRequest::new(
            MessageContext::new(run_id.clone(), preparation_request_id, Participant::Taker),
            runtime.clone(),
            terms,
            recipient_account_id,
            arguments.exclusive_punish_at_ms,
        ))
        .await
        .context("prepare local current-profile clock transaction")?;
    let submission_request_id = preparation
        .transaction
        .transaction_id
        .submission_request_id();
    let submission = client
        .submit_transaction(SubmitTransactionRequest::new(
            MessageContext::new(run_id.clone(), submission_request_id, Participant::Taker),
            runtime.clone(),
            preparation.transaction.clone(),
        ))
        .await
        .context("submit exact local current-profile clock transaction")?;
    let verification_request_id =
        clock_request_id(b"verify", preparation.transaction.transaction_id.as_bytes())?;
    let result = client
        .verify_current_profile_clock(VerifyCurrentProfileClockRequest {
            context: MessageContext::new(
                run_id.clone(),
                verification_request_id,
                Participant::Taker,
            ),
            runtime: runtime.clone(),
            preparation,
            submission,
        })
        .await
        .context("verify local current-profile clock transaction")?;
    let (finalized_clock_after, finalized_observation_attempts_after) = observe_finalized_clock(
        &observer,
        &run_id,
        &runtime,
        FinalizedClockWait {
            identity: result.transaction_id.as_bytes(),
            request_domain: b"finalized-after",
            minimum_height: result.clock_after.height,
            exclusive_punish_at_ms: arguments.exclusive_punish_at_ms,
            timeout: Duration::from_secs(arguments.finality_timeout_seconds),
        },
    )
    .await
    .context("wait for the exact local transaction block to finalize")?;

    let mut evidence = serde_json::to_value(result).context("encode clock evidence")?;
    let Value::Object(ref mut object) = evidence else {
        anyhow::bail!("clock evidence is not an object");
    };
    object.insert(
        "schema".to_owned(),
        Value::String("lez_v02_m5_local_clock_driver_v1".to_owned()),
    );
    object.insert(
        "finalized_clock_before".to_owned(),
        serde_json::to_value(finalized_clock_before)
            .context("encode pre-submit finalized clock")?,
    );
    object.insert(
        "finalized_clock_after".to_owned(),
        serde_json::to_value(finalized_clock_after)
            .context("encode post-submit finalized clock")?,
    );
    object.insert(
        "finalized_observation_attempts_before".to_owned(),
        Value::from(finalized_observation_attempts_before),
    );
    object.insert(
        "finalized_observation_attempts_after".to_owned(),
        Value::from(finalized_observation_attempts_after),
    );
    object.insert(
        "finality_wait_timeout_seconds".to_owned(),
        Value::from(arguments.finality_timeout_seconds),
    );
    object.insert(
        "finality_source".to_owned(),
        Value::String("authenticated_genesis_bound_official_indexer".to_owned()),
    );
    let bytes = serde_json::to_vec_pretty(&evidence).context("serialize clock evidence")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&arguments.output_evidence)
        .context("create owner-only clock evidence")?;
    file.write_all(&bytes).context("write clock evidence")?;
    file.write_all(b"\n").context("finish clock evidence")?;
    file.sync_all().context("sync clock evidence")?;
    Ok(())
}

fn read_capability(path: &PathBuf) -> Result<SidecarCapability> {
    let capability = fs::read_to_string(path).context("read capability file")?;
    SidecarCapability::new(capability.trim().to_owned()).context("invalid sidecar capability")
}

async fn observe_finalized_clock(
    client: &BridgeClient,
    run_id: &RunId,
    runtime: &RuntimeDescriptor,
    wait: FinalizedClockWait<'_>,
) -> Result<(ChainClock, u32)> {
    let deadline = Instant::now() + wait.timeout;
    let mut attempts = 0_u32;
    loop {
        attempts = attempts
            .checked_add(1)
            .context("finalized observation attempt overflow")?;
        let request_id =
            clock_observation_request_id(wait.request_domain, wait.identity, attempts)?;
        let last_failure = match client
            .observe_finalized_clock(ObserveFinalizedClockRequest::new(
                MessageContext::new(run_id.clone(), request_id, Participant::Taker),
                runtime.clone(),
            ))
            .await
        {
            Ok(observation) => {
                ensure!(
                    observation.clock.timestamp_ms < wait.exclusive_punish_at_ms,
                    "finalized clock reached the exclusive punish boundary"
                );
                if observation.clock.height >= wait.minimum_height {
                    return Ok((observation.clock, attempts));
                }
                format!(
                    "finalized height {} is below required height {}",
                    observation.clock.height, wait.minimum_height
                )
            }
            Err(error) => format!("authenticated finalized-clock read failed: {error}"),
        };
        let now = Instant::now();
        if now >= deadline {
            anyhow::bail!(
                "finalized clock did not reach height {} in {} seconds after {attempts} read-only attempts: {last_failure}",
                wait.minimum_height,
                wait.timeout.as_secs()
            );
        }
        tokio::time::sleep((deadline - now).min(Duration::from_millis(250))).await;
    }
}

fn clock_observation_request_id(
    domain: &[u8],
    identity: &[u8; 32],
    attempt: u32,
) -> Result<RequestId> {
    ensure!(
        matches!(domain, b"finalized-before" | b"finalized-after"),
        "unsupported finalized-clock request domain"
    );
    let mut hasher = Sha256::new();
    hasher.update(b"lez-m5-finalized-clock-request-v1");
    hasher.update([0]);
    hasher.update(domain);
    hasher.update([0]);
    hasher.update(identity);
    hasher.update(attempt.to_be_bytes());
    RequestId::new(hex::encode(hasher.finalize()))
        .context("derive bounded finalized-clock request ID")
}

fn clock_request_id(domain: &[u8], identity: &[u8; 32]) -> Result<RequestId> {
    ensure!(
        matches!(domain, b"prepare" | b"verify"),
        "unsupported clock request domain"
    );
    let mut hasher = Sha256::new();
    hasher.update(b"lez-m5-local-clock-request-v1");
    hasher.update([0]);
    hasher.update(domain);
    hasher.update([0]);
    hasher.update(identity);
    RequestId::new(hex::encode(hasher.finalize())).context("derive bounded clock request ID")
}

fn read_json<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_request_ids_fit_protocol_and_are_domain_separated() {
        let identity = Hex32::from_bytes([0xabu8; 32]);
        let prepare =
            clock_request_id(b"prepare", identity.as_bytes()).expect("prepare request ID");
        let verify = clock_request_id(b"verify", identity.as_bytes()).expect("verify request ID");
        let finalized_before =
            clock_observation_request_id(b"finalized-before", identity.as_bytes(), 1)
                .expect("finalized-before request ID");
        let finalized_after =
            clock_observation_request_id(b"finalized-after", identity.as_bytes(), 1)
                .expect("finalized-after request ID");
        let finalized_after_retry =
            clock_observation_request_id(b"finalized-after", identity.as_bytes(), 2)
                .expect("finalized-after retry request ID");

        assert_eq!(prepare.as_str().len(), 64);
        assert_eq!(verify.as_str().len(), 64);
        assert_ne!(prepare, verify);
        assert_ne!(finalized_before, finalized_after);
        assert_ne!(finalized_after, finalized_after_retry);
        assert_eq!(finalized_before.as_str().len(), 64);
        assert_eq!(finalized_after.as_str().len(), 64);
        assert_eq!(
            prepare,
            clock_request_id(b"prepare", identity.as_bytes()).expect("stable prepare request ID")
        );
        assert!(
            prepare
                .as_str()
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
        assert!(verify.as_str().bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}
