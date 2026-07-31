use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::OpenOptionsExt as _,
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context as _, Result, ensure};
use clap::Parser;
use lez_bridge_client::{BridgeClient, BridgeClientConfig, SidecarCapability};
use lez_bridge_protocol::{
    Hex32, MessageContext, Participant, PrepareCurrentProfileClockRequest, RequestId, RunId,
    RuntimeDescriptor, SubmitTransactionRequest, VerifyCurrentProfileClockRequest,
    XmrNativeEscrowTermsV3,
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
    #[arg(long)]
    output_evidence: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    ensure!(
        !arguments.output_evidence.exists(),
        "output evidence already exists"
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

    let capability =
        fs::read_to_string(&arguments.capability_file).context("read capability file")?;
    let capability = SidecarCapability::new(capability.trim().to_owned())
        .context("invalid sidecar capability")?;
    let client = BridgeClient::connect(BridgeClientConfig::new(
        arguments.sidecar_endpoint,
        capability,
        run_id.clone(),
        runtime.clone(),
        Duration::from_secs(90),
    ))
    .context("connect authenticated Taker sidecar")?;
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
            context: MessageContext::new(run_id, verification_request_id, Participant::Taker),
            runtime,
            preparation,
            submission,
        })
        .await
        .context("verify local current-profile clock transaction")?;

    let mut evidence = serde_json::to_value(result).context("encode clock evidence")?;
    let Value::Object(ref mut object) = evidence else {
        anyhow::bail!("clock evidence is not an object");
    };
    object.insert(
        "schema".to_owned(),
        Value::String("lez_v02_m5_local_clock_driver_v1".to_owned()),
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

        assert_eq!(prepare.as_str().len(), 64);
        assert_eq!(verify.as_str().len(), 64);
        assert_ne!(prepare, verify);
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
