use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use url::{Host, Url};

use crate::ActorRole;

pub(crate) const MAX_AUTHORITY_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Tool {
    program: PathBuf,
    program_sha256: String,
    abi: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MakerTools {
    monero_fund: Tool,
    lez_claim: Tool,
    finalized_classifier: Tool,
    monero_refund: Tool,
    monero_verify: Tool,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TakerTools {
    tag14_authorize: Tool,
    finalized_classifier: Tool,
    monero_claim: Tool,
    monero_verify: Tool,
    tag16_refund: Tool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LezRpc {
    sidecar_url: String,
    runtime_file: PathBuf,
    runtime_sha256: String,
    capability_file: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AuthenticatedRpc {
    url: String,
    username_file: PathBuf,
    password_file: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MoneroRpc {
    daemon: AuthenticatedRpc,
    funding_wallet: AuthenticatedRpc,
    shared_wallet: AuthenticatedRpc,
    role_wallet: AuthenticatedRpc,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EffectAuthorityV1 {
    schema_version: u16,
    pair: String,
    role: ActorRole,
    swap_id: String,
    agreement_commitment: String,
    activation_commitment: String,
    run_id: String,
    workflow_journal: PathBuf,
    adaptor_journal: PathBuf,
    evidence_root: PathBuf,
    lez: LezRpc,
    monero: MoneroRpc,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    maker_tools: Option<MakerTools>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    taker_tools: Option<TakerTools>,
}

/// Fully validated role-fixed XMR effect authority.
#[derive(Debug)]
#[must_use]
pub struct ValidatedXmrEffectAuthorityV1 {
    role: ActorRole,
    swap_id: [u8; 32],
    run_id: Box<str>,
    workflow_journal: PathBuf,
    adaptor_journal: PathBuf,
}

impl ValidatedXmrEffectAuthorityV1 {
    /// Role fixed by the authority.
    #[must_use]
    pub const fn role(&self) -> ActorRole {
        self.role
    }

    /// Exact swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> [u8; 32] {
        self.swap_id
    }

    /// Exact run identity.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Separate mutable orchestration journal.
    #[must_use]
    pub fn workflow_journal(&self) -> &Path {
        &self.workflow_journal
    }

    /// Immutable adaptor transcript journal.
    #[must_use]
    pub fn adaptor_journal(&self) -> &Path {
        &self.adaptor_journal
    }
}

/// Validates canonical owner-private XMR effect-authority bytes.
///
/// # Errors
///
/// Rejects legacy schemas, crossed identities, noncanonical JSON, unsafe paths,
/// non-loopback RPCs, digest drift, and role/profile mismatches.
pub fn load_validated_xmr_effect_authority_bytes(
    bytes: &[u8],
    expected_role: ActorRole,
    expected_swap: [u8; 32],
    expected_agreement: [u8; 32],
    expected_activation: [u8; 32],
    expected_run: &str,
) -> Result<ValidatedXmrEffectAuthorityV1> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_AUTHORITY_BYTES,
        "XMR effect authority is oversized"
    );
    let authority: EffectAuthorityV1 =
        serde_json::from_slice(bytes).context("XMR effect authority is malformed")?;
    let mut canonical = serde_json::to_vec(&authority)?;
    canonical.push(b'\n');
    ensure!(canonical == bytes, "XMR effect authority is noncanonical");
    ensure!(
        authority.schema_version == 1
            && authority.pair == "monero"
            && authority.role == expected_role
            && decode_digest(&authority.swap_id)? == expected_swap
            && decode_digest(&authority.agreement_commitment)? == expected_agreement
            && decode_digest(&authority.activation_commitment)? == expected_activation
            && authority.run_id == expected_run
            && valid_label(&authority.run_id),
        "XMR effect authority identity is invalid"
    );
    let paths = [
        &authority.workflow_journal,
        &authority.adaptor_journal,
        &authority.evidence_root,
        &authority.lez.runtime_file,
        &authority.lez.capability_file,
        &authority.monero.daemon.username_file,
        &authority.monero.daemon.password_file,
        &authority.monero.funding_wallet.username_file,
        &authority.monero.funding_wallet.password_file,
        &authority.monero.shared_wallet.username_file,
        &authority.monero.shared_wallet.password_file,
        &authority.monero.role_wallet.username_file,
        &authority.monero.role_wallet.password_file,
    ];
    ensure!(
        paths.iter().all(|path| normalized_absolute(path)),
        "XMR effect authority path is invalid"
    );
    ensure!(
        authority.workflow_journal != authority.adaptor_journal
            && authority.workflow_journal != authority.evidence_root
            && authority.adaptor_journal != authority.evidence_root,
        "XMR effect authority path roles overlap"
    );
    validate_rpc(&authority.lez.sidecar_url)?;
    validate_rpc_set(&authority.monero)?;
    validate_digest(&authority.lez.runtime_sha256)?;
    validate_profile(&authority)?;
    Ok(ValidatedXmrEffectAuthorityV1 {
        role: authority.role,
        swap_id: expected_swap,
        run_id: authority.run_id.into_boxed_str(),
        workflow_journal: authority.workflow_journal,
        adaptor_journal: authority.adaptor_journal,
    })
}

fn validate_rpc_set(rpc: &MoneroRpc) -> Result<()> {
    for endpoint in [
        &rpc.daemon,
        &rpc.funding_wallet,
        &rpc.shared_wallet,
        &rpc.role_wallet,
    ] {
        validate_rpc(&endpoint.url)?;
        ensure!(
            endpoint.username_file != endpoint.password_file,
            "XMR RPC credential paths overlap"
        );
    }
    Ok(())
}

fn validate_profile(authority: &EffectAuthorityV1) -> Result<()> {
    match (
        authority.role,
        authority.maker_tools.as_ref(),
        authority.taker_tools.as_ref(),
    ) {
        (ActorRole::Maker, Some(tools), None) => {
            validate_tool_paths([
                &tools.monero_fund,
                &tools.lez_claim,
                &tools.finalized_classifier,
                &tools.monero_refund,
                &tools.monero_verify,
            ])?;
            validate_tool(&tools.monero_fund, "lez_xmr_monero_fund_v2")?;
            validate_tool(&tools.lez_claim, "lez_xmr_tag15_claim_v1")?;
            validate_tool(
                &tools.finalized_classifier,
                "lez_xmr_finalized_classifier_v1",
            )?;
            validate_tool(&tools.monero_refund, "lez_xmr_monero_refund_sweep_v3")?;
            validate_tool(&tools.monero_verify, "lez_xmr_monero_verify_v2")
        }
        (ActorRole::Taker, None, Some(tools)) => {
            validate_tool_paths([
                &tools.tag14_authorize,
                &tools.finalized_classifier,
                &tools.monero_claim,
                &tools.monero_verify,
                &tools.tag16_refund,
            ])?;
            validate_tool(&tools.tag14_authorize, "lez_xmr_tag14_authorize_v1")?;
            validate_tool(
                &tools.finalized_classifier,
                "lez_xmr_finalized_classifier_v1",
            )?;
            validate_tool(&tools.monero_claim, "lez_xmr_monero_claim_sweep_v2")?;
            validate_tool(&tools.monero_verify, "lez_xmr_monero_verify_v2")?;
            validate_tool(&tools.tag16_refund, "lez_xmr_tag16_refund_v1")
        }
        _ => anyhow::bail!("XMR effect authority role profile is invalid"),
    }
}

fn validate_tool_paths<const N: usize>(tools: [&Tool; N]) -> Result<()> {
    ensure!(
        tools.iter().all(|tool| normalized_absolute(&tool.program)),
        "XMR effect tool path is invalid"
    );
    Ok(())
}

fn validate_rpc(value: &str) -> Result<()> {
    let parsed = Url::parse(value).context("XMR effect RPC URL is invalid")?;
    let loopback = matches!(
        parsed.host(),
        Some(Host::Ipv4(address)) if address.is_loopback()
    ) || matches!(
        parsed.host(),
        Some(Host::Ipv6(address)) if address.is_loopback()
    );
    ensure!(
        parsed.scheme() == "http"
            && loopback
            && parsed.port().is_some_and(|port| port != 0)
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.path() == "/"
            && parsed.query().is_none()
            && parsed.fragment().is_none(),
        "XMR effect RPC URL is not a literal loopback root"
    );
    Ok(())
}

fn validate_tool(tool: &Tool, expected_abi: &str) -> Result<()> {
    validate_digest(&tool.program_sha256)?;
    ensure!(tool.abi == expected_abi, "XMR effect tool ABI is invalid");
    Ok(())
}

fn validate_digest(value: &str) -> Result<()> {
    let decoded = decode_digest(value)?;
    ensure!(
        decoded.iter().any(|byte| *byte != 0) && hex::encode(decoded) == value,
        "XMR effect authority digest is invalid"
    );
    Ok(())
}

fn decode_digest(value: &str) -> Result<[u8; 32]> {
    let decoded = hex::decode(value).context("XMR effect authority digest is invalid")?;
    decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("XMR effect authority digest is invalid"))
}

pub(crate) fn valid_label(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn normalized_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.file_name().is_some()
        && path
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
}
