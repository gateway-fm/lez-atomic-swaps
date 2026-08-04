//! Canonical secret-free execution plan for one role-fixed XMR effect child.

use std::{
    fs::File,
    io::Read as _,
    os::unix::fs::PermissionsExt as _,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};
use lez_swap_core::Participant;
use lez_swap_store::XmrWorkflowStep;
use rustix::fs::{SealFlags, fcntl_get_seals};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    ActorRole, ValidatedXmrEffectAuthorityV1, effect_authority::valid_label,
    effect_input_custody::XMR_EFFECT_CHILD_PLAN_FD,
};

/// Maximum accepted canonical child-plan bytes.
pub const XMR_EFFECT_CHILD_PLAN_MAX_BYTES: usize = 8 * 1024;

/// Whether the selected child may send once or only observe.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[must_use]
pub enum XmrEffectChildModeV1 {
    /// One sending attempt authorized by the parent workflow CAS.
    Invoke,
    /// Read-only reconciliation after a prior attempt.
    Observe,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct XmrEffectChildPlanWireV1 {
    schema_version: u16,
    pair: String,
    role: ActorRole,
    mode: XmrEffectChildModeV1,
    step: String,
    run_id: String,
    swap_id: String,
    agreement_commitment: String,
    activation_commitment: String,
    executable_abi: String,
    sending_tool_plan_sha256: String,
    adaptor_journal: PathBuf,
    evidence_root: PathBuf,
    lez_sidecar_url: String,
    monero_daemon_url: String,
    monero_funding_wallet_url: String,
    monero_shared_wallet_url: String,
    monero_role_wallet_url: String,
}

/// Validated child plan reconstructed from sealed descriptor 217.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrEffectChildPlanV1 {
    role: ActorRole,
    mode: XmrEffectChildModeV1,
    step: XmrWorkflowStep,
    run_id: Box<str>,
    swap_id: [u8; 32],
    agreement_commitment: [u8; 32],
    activation_commitment: [u8; 32],
    executable_abi: Box<str>,
    sending_tool_plan_sha256: [u8; 32],
    adaptor_journal: PathBuf,
    evidence_root: PathBuf,
    lez_sidecar_url: Url,
    monero_daemon_url: Url,
    monero_funding_wallet_url: Url,
    monero_shared_wallet_url: Url,
    monero_role_wallet_url: Url,
}

impl XmrEffectChildPlanV1 {
    /// Role fixed by the validated application and effect authority.
    #[must_use]
    pub const fn role(&self) -> ActorRole {
        self.role
    }

    /// Whether this child is a sender or observer.
    pub const fn mode(&self) -> XmrEffectChildModeV1 {
        self.mode
    }

    /// Exact parent-selected workflow step.
    pub const fn step(&self) -> XmrWorkflowStep {
        self.step
    }

    /// Exact application run identity.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Exact swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> [u8; 32] {
        self.swap_id
    }

    /// Exact Stage-A agreement commitment.
    #[must_use]
    pub const fn agreement_commitment(&self) -> [u8; 32] {
        self.agreement_commitment
    }

    /// Exact Stage-B activation commitment.
    #[must_use]
    pub const fn activation_commitment(&self) -> [u8; 32] {
        self.activation_commitment
    }

    /// ABI of the executable pinned for this child.
    #[must_use]
    pub fn executable_abi(&self) -> &str {
        &self.executable_abi
    }

    /// Identity of the sending plan, including for an observer.
    #[must_use]
    pub const fn sending_tool_plan_sha256(&self) -> [u8; 32] {
        self.sending_tool_plan_sha256
    }

    /// Live role-local adaptor journal protected by inherited lock FD 198.
    #[must_use]
    pub fn adaptor_journal(&self) -> &Path {
        &self.adaptor_journal
    }

    /// Owner-private destination root for semantic evidence.
    #[must_use]
    pub fn evidence_root(&self) -> &Path {
        &self.evidence_root
    }

    /// Validated local LEZ sidecar origin.
    #[must_use]
    pub const fn lez_sidecar_url(&self) -> &Url {
        &self.lez_sidecar_url
    }

    /// Validated local Monero daemon origin.
    #[must_use]
    pub const fn monero_daemon_url(&self) -> &Url {
        &self.monero_daemon_url
    }

    /// Validated local funding-wallet origin.
    #[must_use]
    pub const fn monero_funding_wallet_url(&self) -> &Url {
        &self.monero_funding_wallet_url
    }

    /// Validated local reconstructed shared-wallet origin.
    #[must_use]
    pub const fn monero_shared_wallet_url(&self) -> &Url {
        &self.monero_shared_wallet_url
    }

    /// Validated local role-wallet origin.
    #[must_use]
    pub const fn monero_role_wallet_url(&self) -> &Url {
        &self.monero_role_wallet_url
    }
}

pub(crate) fn canonical_xmr_effect_child_plan_bytes(
    authority: &ValidatedXmrEffectAuthorityV1,
    mode: XmrEffectChildModeV1,
    step: XmrWorkflowStep,
    executable_abi: &str,
    sending_tool_plan_sha256: [u8; 32],
) -> Result<Vec<u8>> {
    let wire = XmrEffectChildPlanWireV1 {
        schema_version: 1,
        pair: "monero".to_owned(),
        role: authority.role(),
        mode,
        step: step.name().to_owned(),
        run_id: authority.run_id().to_owned(),
        swap_id: hex::encode(authority.swap_id()),
        agreement_commitment: hex::encode(authority.agreement_commitment()),
        activation_commitment: hex::encode(authority.activation_commitment()),
        executable_abi: executable_abi.to_owned(),
        sending_tool_plan_sha256: hex::encode(sending_tool_plan_sha256),
        adaptor_journal: authority.adaptor_journal().to_path_buf(),
        evidence_root: authority.evidence_root().to_path_buf(),
        lez_sidecar_url: authority.lez().sidecar_url().as_str().to_owned(),
        monero_daemon_url: authority.monero().daemon().url().as_str().to_owned(),
        monero_funding_wallet_url: authority
            .monero()
            .funding_wallet()
            .url()
            .as_str()
            .to_owned(),
        monero_shared_wallet_url: authority.monero().shared_wallet().url().as_str().to_owned(),
        monero_role_wallet_url: authority.monero().role_wallet().url().as_str().to_owned(),
    };
    let mut bytes = serde_json::to_vec(&wire).context("encode XMR effect child plan")?;
    bytes.push(b'\n');
    let _ = parse_xmr_effect_child_plan_v1(&bytes)?;
    Ok(bytes)
}

/// Parses one canonical, bounded, secret-free effect child plan.
///
/// # Errors
///
/// Rejects empty, oversized, noncanonical, wrong-role, invalid-step, unsafe
/// path, non-loopback RPC, invalid ABI, or malformed identity fields.
pub fn parse_xmr_effect_child_plan_v1(bytes: &[u8]) -> Result<XmrEffectChildPlanV1> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= XMR_EFFECT_CHILD_PLAN_MAX_BYTES,
        "XMR effect child plan is empty or oversized"
    );
    let wire: XmrEffectChildPlanWireV1 =
        serde_json::from_slice(bytes).context("XMR effect child plan is malformed")?;
    let mut canonical = serde_json::to_vec(&wire).context("encode XMR effect child plan")?;
    canonical.push(b'\n');
    ensure!(canonical == bytes, "XMR effect child plan is noncanonical");
    let step = XmrWorkflowStep::ALL
        .into_iter()
        .find(|candidate| candidate.name() == wire.step)
        .context("XMR effect child plan step is unsupported")?;
    let expected_role = match step.role() {
        Participant::Maker => ActorRole::Maker,
        Participant::Taker => ActorRole::Taker,
    };
    ensure!(
        wire.schema_version == 1
            && wire.pair == "monero"
            && wire.role == expected_role
            && valid_label(&wire.run_id)
            && valid_label(&wire.executable_abi)
            && normalized_absolute(&wire.adaptor_journal)
            && normalized_absolute(&wire.evidence_root)
            && wire.adaptor_journal != wire.evidence_root,
        "XMR effect child plan authority is invalid"
    );
    Ok(XmrEffectChildPlanV1 {
        role: wire.role,
        mode: wire.mode,
        step,
        run_id: wire.run_id.into_boxed_str(),
        swap_id: decode_nonzero_digest(&wire.swap_id)?,
        agreement_commitment: decode_nonzero_digest(&wire.agreement_commitment)?,
        activation_commitment: decode_nonzero_digest(&wire.activation_commitment)?,
        executable_abi: wire.executable_abi.into_boxed_str(),
        sending_tool_plan_sha256: decode_nonzero_digest(&wire.sending_tool_plan_sha256)?,
        adaptor_journal: wire.adaptor_journal,
        evidence_root: wire.evidence_root,
        lez_sidecar_url: loopback_url(&wire.lez_sidecar_url)?,
        monero_daemon_url: loopback_url(&wire.monero_daemon_url)?,
        monero_funding_wallet_url: loopback_url(&wire.monero_funding_wallet_url)?,
        monero_shared_wallet_url: loopback_url(&wire.monero_shared_wallet_url)?,
        monero_role_wallet_url: loopback_url(&wire.monero_role_wallet_url)?,
    })
}

/// Loads and validates the sealed child plan from fixed descriptor 217.
///
/// # Errors
///
/// Rejects a missing/non-file descriptor, incomplete memfd seals, wrong mode,
/// oversized bytes, or any semantic/canonical child-plan error.
pub fn load_xmr_effect_child_plan_fd() -> Result<XmrEffectChildPlanV1> {
    let path = format!("/proc/self/fd/{XMR_EFFECT_CHILD_PLAN_FD}");
    let mut file = File::open(path).context("open sealed XMR effect child plan")?;
    let metadata = file
        .metadata()
        .context("inspect sealed XMR effect child plan")?;
    let required = SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE;
    ensure!(
        metadata.file_type().is_file()
            && metadata.permissions().mode() & 0o7777 == 0o400
            && fcntl_get_seals(&file)
                .context("inspect XMR effect child plan seals")?
                .contains(required),
        "XMR effect child plan descriptor is unsafe"
    );
    let mut bytes = Vec::new();
    file.by_ref()
        .take(u64::try_from(XMR_EFFECT_CHILD_PLAN_MAX_BYTES).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut bytes)
        .context("read sealed XMR effect child plan")?;
    ensure!(
        bytes.len() <= XMR_EFFECT_CHILD_PLAN_MAX_BYTES
            && metadata.len() == u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "XMR effect child plan descriptor is oversized or changed"
    );
    parse_xmr_effect_child_plan_v1(&bytes)
}

/// Loads FD 217 and binds it to one worker's compiled route.
///
/// # Errors
///
/// Rejects every unsafe descriptor or plan condition plus a different role,
/// mode, step, or executable ABI.
pub fn load_xmr_effect_child_plan_fd_for(
    expected_role: ActorRole,
    expected_mode: XmrEffectChildModeV1,
    expected_step: XmrWorkflowStep,
    expected_abi: &str,
) -> Result<XmrEffectChildPlanV1> {
    let plan = load_xmr_effect_child_plan_fd()?;
    ensure!(
        plan.role == expected_role
            && plan.mode == expected_mode
            && plan.step == expected_step
            && plan.executable_abi.as_ref() == expected_abi,
        "XMR effect child plan differs from the compiled worker route"
    );
    Ok(plan)
}

fn decode_digest(value: &str) -> Result<[u8; 32]> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "XMR effect child plan digest is not canonical"
    );
    let mut digest = [0_u8; 32];
    hex::decode_to_slice(value, &mut digest).context("decode XMR effect child plan digest")?;
    Ok(digest)
}

fn decode_nonzero_digest(value: &str) -> Result<[u8; 32]> {
    let digest = decode_digest(value)?;
    ensure!(
        digest.iter().any(|byte| *byte != 0),
        "XMR effect child plan sending identity is zero"
    );
    Ok(digest)
}

fn normalized_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.file_name().is_some()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn loopback_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).context("parse XMR effect child RPC URL")?;
    ensure!(
        url.scheme() == "http"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/"
            && url
                .host_str()
                .and_then(|host| host.parse::<std::net::IpAddr>().ok())
                .is_some_and(|address| address.is_loopback())
            && url.port().is_some_and(|port| port != 0),
        "XMR effect child RPC URL is not a literal loopback origin"
    );
    Ok(url)
}
