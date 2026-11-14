//! Sealed no-argument Maker Monero funding worker (`lez_xmr_monero_fund_v2`).
//!
//! The workflow's one `FundMonero` invocation: under the role's wallet-rpc
//! lock it sends the exact Stage-A amount from the role wallet to the shared
//! address, mines the confirmations, and records the funding with its receipt
//! in the evidence root (`xmr_reference_actor::fund_maker_monero`).

#![forbid(unsafe_code)]

// Only the Linux sealed-descriptor reader reads.
#[cfg_attr(target_os = "macos", allow(unused_imports))]
use std::io::Read as _;
use std::{fs::File, os::unix::fs::PermissionsExt as _};

use anyhow::{Context as _, Result, ensure};
use lez_swap_store::XmrWorkflowStep;
use lez_xmr_monero_adapter::LoopbackRpcEndpoint;
use lez_xmr_swap_sdk::{MAX_XMR_AGREEMENT_WIRE_BYTES, MoneroPrivateViewKey, XmrAgreementV1};
#[cfg(target_os = "linux")]
use rustix::fs::{SealFlags, fcntl_get_seals};
use xmr_reference_actor::{
    ActorRole, MoneroFundingNotSent, XMR_EFFECT_DAEMON_PASSWORD_FD, XMR_EFFECT_DAEMON_USERNAME_FD,
    XMR_EFFECT_PRIVATE_VIEW_KEY_FD, XMR_EFFECT_ROLE_PASSWORD_FD, XMR_EFFECT_ROLE_USERNAME_FD,
    XMR_EFFECT_SHARED_WALLET_FILE_PASSWORD_FD, XMR_EFFECT_STAGE_A_FD,
    XMR_MONERO_FUNDING_NOT_SENT_EXIT, XmrEffectChildModeV1, XmrRoleWalletRpcV1, fund_maker_monero,
    load_xmr_effect_child_plan_fd_for,
};
use zeroize::{Zeroize as _, Zeroizing};

const ABI: &str = "lez_xmr_monero_fund_v2";
const MAX_SECRET_BYTES: usize = 256;

#[tokio::main]
async fn main() {
    if let Err(error) = execute().await {
        eprintln!("Maker Monero funding worker failed: {error:#}");
        // The actor reads this status as "nothing was sent".
        let not_sent = error.downcast_ref::<MoneroFundingNotSent>().is_some();
        std::process::exit(if not_sent {
            XMR_MONERO_FUNDING_NOT_SENT_EXIT
        } else {
            1
        });
    }
}

async fn execute() -> Result<()> {
    ensure!(
        std::env::args_os().len() == 1,
        "Monero funding worker accepts no arguments"
    );
    let plan = load_xmr_effect_child_plan_fd_for(
        ActorRole::Maker,
        XmrEffectChildModeV1::Invoke,
        XmrWorkflowStep::FundMonero,
        ABI,
    )
    .context("load Monero funding child plan")?;
    let agreement = XmrAgreementV1::from_wire(&read_sealed_fd(
        XMR_EFFECT_STAGE_A_FD,
        MAX_XMR_AGREEMENT_WIRE_BYTES,
        "Stage-A agreement",
    )?)
    .context("Stage-A agreement is invalid")?;
    ensure!(
        agreement.body().swap_id() == plan.swap_id()
            && agreement.agreement_commitment() == plan.agreement_commitment(),
        "Monero funding child application identity changed"
    );
    let view_key = parse_view_key(&Zeroizing::new(read_sealed_fd(
        XMR_EFFECT_PRIVATE_VIEW_KEY_FD,
        MAX_SECRET_BYTES,
        "Monero view key",
    )?))?;
    let rpc = XmrRoleWalletRpcV1::new(
        &plan,
        endpoint_from_fds(
            plan.monero_daemon_url().as_str(),
            XMR_EFFECT_DAEMON_USERNAME_FD,
            XMR_EFFECT_DAEMON_PASSWORD_FD,
            "daemon",
        )?,
        endpoint_from_fds(
            plan.monero_role_wallet_url().as_str(),
            XMR_EFFECT_ROLE_USERNAME_FD,
            XMR_EFFECT_ROLE_PASSWORD_FD,
            "role wallet",
        )?,
    )?;
    let password = read_secret_text_fd(
        XMR_EFFECT_SHARED_WALLET_FILE_PASSWORD_FD,
        "shared-wallet file password",
    )?;
    fund_maker_monero(&rpc, &plan, &agreement, view_key, &password)
        .await
        .map(|_| ())
}

fn endpoint_from_fds(
    url: &str,
    username_fd: i32,
    password_fd: i32,
    label: &'static str,
) -> Result<LoopbackRpcEndpoint> {
    let username = read_secret_text_fd(username_fd, label)?;
    let password = read_secret_text_fd(password_fd, label)?;
    LoopbackRpcEndpoint::new(url, username.as_str(), password.as_str())
        .with_context(|| format!("{label} RPC authority is invalid"))
}

fn read_secret_text_fd(fd: i32, label: &'static str) -> Result<Zeroizing<String>> {
    let bytes = Zeroizing::new(read_sealed_fd(fd, MAX_SECRET_BYTES, label)?);
    let mut text = Zeroizing::new(
        String::from_utf8(bytes.to_vec()).with_context(|| format!("{label} is not UTF-8"))?,
    );
    while text.ends_with(['\n', '\r']) {
        text.pop();
    }
    ensure!(!text.is_empty(), "{label} is empty");
    Ok(text)
}

fn parse_view_key(bytes: &[u8]) -> Result<MoneroPrivateViewKey> {
    let mut text =
        Zeroizing::new(String::from_utf8(bytes.to_vec()).context("Monero view key is not UTF-8")?);
    while text.ends_with(['\n', '\r']) {
        text.pop();
    }
    ensure!(
        text.len() == 64
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Monero view key is not exact lowercase hex"
    );
    let mut scalar = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(text.as_bytes(), scalar.as_mut()).context("decode Monero view key")?;
    text.zeroize();
    MoneroPrivateViewKey::from_monero_little_endian(*scalar)
        .context("Monero view key is not a canonical scalar")
}

#[cfg(target_os = "linux")]
fn read_sealed_fd(fd: i32, maximum: usize, label: &'static str) -> Result<Vec<u8>> {
    let mut file =
        File::open(format!("/proc/self/fd/{fd}")).with_context(|| format!("open {label} FD"))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect {label} FD"))?;
    let required = SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE;
    ensure!(
        metadata.file_type().is_file()
            && metadata.permissions().mode() & 0o7777 == 0o400
            && fcntl_get_seals(&file)
                .with_context(|| format!("inspect {label} seals"))?
                .contains(required),
        "{label} FD is unsafe"
    );
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(u64::try_from(maximum).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {label} FD"))?;
    ensure!(
        bytes.len() <= maximum && metadata.len() == u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "{label} FD is oversized or changed"
    );
    Ok(bytes)
}

/// Not ported to macOS: this input arrives as a write-sealed memfd and macOS
/// has no seal to check, so the descriptor is never trusted.
#[cfg(target_os = "macos")]
fn read_sealed_fd(fd: i32, _maximum: usize, label: &'static str) -> Result<Vec<u8>> {
    anyhow::bail!("{label} FD {fd} needs Linux memfd seals; XMR effects are not supported on macOS")
}
