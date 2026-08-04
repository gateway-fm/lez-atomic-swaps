use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, ensure};
use lez_swap_store::PinnedExecutable;
use serde::{Deserialize, Serialize};
use url::{Host, Url};

use crate::ActorRole;

/// Maximum accepted canonical effect-authority byte length.
pub const XMR_EFFECT_AUTHORITY_MAX_BYTES: u64 = 64 * 1024;
pub(crate) const MAX_AUTHORITY_BYTES: usize = 64 * 1024;
const OFFICIAL_PUBLIC_INDEXER_ENDPOINT: &str = "https://testnet.lez.logos.co/";

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
    shared_wallet_file_password_file: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Tag14ReleaseNodeProfile {
    Local,
    OfficialPublic,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Tag14Release {
    sidecar_url: String,
    indexer_url: String,
    node_profile: Tag14ReleaseNodeProfile,
    state_directory: PathBuf,
    capability_file: PathBuf,
    protection_key_file: PathBuf,
    protection_key_id: String,
}

/// One validated executable slot in an XMR effect plan.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrEffectToolV1 {
    program: PathBuf,
    program_sha256: [u8; 32],
    abi: Box<str>,
}

impl XmrEffectToolV1 {
    /// Exact normalized executable path.
    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Pinned SHA-256 of the executable bytes.
    #[must_use]
    pub const fn program_sha256(&self) -> [u8; 32] {
        self.program_sha256
    }

    /// Fixed role-slot ABI.
    #[must_use]
    pub fn abi(&self) -> &str {
        &self.abi
    }

    /// Secure-opens, hash-checks, and seals the exact executable bytes.
    ///
    /// The returned value must be consumed through `PinnedExecutable::into_command`;
    /// it never reopens the named path and therefore cannot execute replacement bytes.
    ///
    /// # Errors
    ///
    /// Rejects unsafe ownership, modes, links, path identity, size, or digest drift.
    pub fn verify_program_at_use(&self) -> Result<PinnedExecutable> {
        PinnedExecutable::open(&self.program, self.program_sha256)
            .context("pin XMR effect executable at use")
    }
}

/// Validated local LEZ sidecar authority for one XMR effect plan.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrEffectLezRpcV1 {
    sidecar_url: Url,
    runtime_file: PathBuf,
    runtime_sha256: [u8; 32],
    capability_file: PathBuf,
}

impl XmrEffectLezRpcV1 {
    /// Literal-loopback sidecar root URL.
    #[must_use]
    pub const fn sidecar_url(&self) -> &Url {
        &self.sidecar_url
    }
    /// Exact runtime identity file.
    #[must_use]
    pub fn runtime_file(&self) -> &Path {
        &self.runtime_file
    }
    /// Pinned runtime-file SHA-256.
    #[must_use]
    pub const fn runtime_sha256(&self) -> [u8; 32] {
        self.runtime_sha256
    }
    /// Exact sidecar capability file.
    #[must_use]
    pub fn capability_file(&self) -> &Path {
        &self.capability_file
    }
}

/// One validated authenticated literal-loopback Monero RPC authority.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrEffectAuthenticatedRpcV1 {
    url: Url,
    username_file: PathBuf,
    password_file: PathBuf,
}

impl XmrEffectAuthenticatedRpcV1 {
    /// Literal-loopback RPC root URL.
    #[must_use]
    pub const fn url(&self) -> &Url {
        &self.url
    }
    /// Exact username file.
    #[must_use]
    pub fn username_file(&self) -> &Path {
        &self.username_file
    }
    /// Exact password file.
    #[must_use]
    pub fn password_file(&self) -> &Path {
        &self.password_file
    }
}

/// Role-separated Monero daemon and wallet RPC authorities.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrEffectMoneroRpcV1 {
    daemon: XmrEffectAuthenticatedRpcV1,
    funding_wallet: XmrEffectAuthenticatedRpcV1,
    shared_wallet: XmrEffectAuthenticatedRpcV1,
    role_wallet: XmrEffectAuthenticatedRpcV1,
    shared_wallet_file_password_file: PathBuf,
}

impl XmrEffectMoneroRpcV1 {
    /// Official Monero daemon RPC.
    pub const fn daemon(&self) -> &XmrEffectAuthenticatedRpcV1 {
        &self.daemon
    }
    /// Maker funding/mining wallet RPC.
    pub const fn funding_wallet(&self) -> &XmrEffectAuthenticatedRpcV1 {
        &self.funding_wallet
    }
    /// Neutral reconstructed shared-wallet RPC.
    pub const fn shared_wallet(&self) -> &XmrEffectAuthenticatedRpcV1 {
        &self.shared_wallet
    }
    /// Role destination wallet RPC.
    pub const fn role_wallet(&self) -> &XmrEffectAuthenticatedRpcV1 {
        &self.role_wallet
    }
    /// Password for the reconstructed shared-wallet file.
    #[must_use]
    pub fn shared_wallet_file_password_file(&self) -> &Path {
        &self.shared_wallet_file_password_file
    }
}

/// Finalized-indexer route selected by a schema-v2 Tag14 release authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum XmrTag14ReleaseNodeProfileV1 {
    /// Explicit literal-loopback local indexer.
    Local,
    /// Exact allowlisted Logos LEZ Testnet indexer.
    OfficialPublic,
}

/// Validated release-only authority for the Taker Tag14 worker.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrTag14ReleaseAuthorityV1 {
    sidecar_url: Url,
    indexer_url: Url,
    node_profile: XmrTag14ReleaseNodeProfileV1,
    state_directory: PathBuf,
    capability_file: PathBuf,
    protection_key_file: PathBuf,
    protection_key_id: Box<str>,
}

impl XmrTag14ReleaseAuthorityV1 {
    /// Literal-loopback release-only sidecar endpoint.
    #[must_use]
    pub const fn sidecar_url(&self) -> &Url {
        &self.sidecar_url
    }
    /// Local or exact allowlisted finalized-indexer endpoint.
    #[must_use]
    pub const fn indexer_url(&self) -> &Url {
        &self.indexer_url
    }
    /// Finalized-indexer route profile.
    pub const fn node_profile(&self) -> XmrTag14ReleaseNodeProfileV1 {
        self.node_profile
    }
    /// Owner-private directory containing the encrypted release journal.
    #[must_use]
    pub fn state_directory(&self) -> &Path {
        &self.state_directory
    }
    /// Release-only sidecar capability source.
    #[must_use]
    pub fn capability_file(&self) -> &Path {
        &self.capability_file
    }
    /// Release-journal protection-key source.
    #[must_use]
    pub fn protection_key_file(&self) -> &Path {
        &self.protection_key_file
    }
    /// Nonsecret protection-key rotation identifier.
    #[must_use]
    pub fn protection_key_id(&self) -> &str {
        &self.protection_key_id
    }
}

/// Fixed Maker effect tool profile.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrMakerEffectToolsV1 {
    monero_fund: XmrEffectToolV1,
    lez_claim: XmrEffectToolV1,
    finalized_classifier: XmrEffectToolV1,
    monero_refund: XmrEffectToolV1,
    monero_verify: XmrEffectToolV1,
}

impl XmrMakerEffectToolsV1 {
    /// Monero funding tool.
    pub const fn monero_fund(&self) -> &XmrEffectToolV1 {
        &self.monero_fund
    }
    /// LEZ tag-15 claim tool.
    pub const fn lez_claim(&self) -> &XmrEffectToolV1 {
        &self.lez_claim
    }
    /// Finalized native-effect classifier.
    pub const fn finalized_classifier(&self) -> &XmrEffectToolV1 {
        &self.finalized_classifier
    }
    /// Maker Monero refund-sweep tool.
    pub const fn monero_refund(&self) -> &XmrEffectToolV1 {
        &self.monero_refund
    }
    /// Monero receipt verifier.
    pub const fn monero_verify(&self) -> &XmrEffectToolV1 {
        &self.monero_verify
    }
}

/// Fixed Taker effect tool profile.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrTakerEffectToolsV1 {
    tag14_authorize: XmrEffectToolV1,
    finalized_classifier: XmrEffectToolV1,
    monero_claim: XmrEffectToolV1,
    monero_verify: XmrEffectToolV1,
    tag16_refund: XmrEffectToolV1,
}

impl XmrTakerEffectToolsV1 {
    /// LEZ tag-14 authorization tool.
    pub const fn tag14_authorize(&self) -> &XmrEffectToolV1 {
        &self.tag14_authorize
    }
    /// Finalized native-effect classifier.
    pub const fn finalized_classifier(&self) -> &XmrEffectToolV1 {
        &self.finalized_classifier
    }
    /// Taker Monero claim-sweep tool.
    pub const fn monero_claim(&self) -> &XmrEffectToolV1 {
        &self.monero_claim
    }
    /// Monero receipt verifier.
    pub const fn monero_verify(&self) -> &XmrEffectToolV1 {
        &self.monero_verify
    }
    /// LEZ tag-16 refund tool.
    pub const fn tag16_refund(&self) -> &XmrEffectToolV1 {
        &self.tag16_refund
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tag14_release: Option<Tag14Release>,
}

/// Fully validated role-fixed XMR effect authority.
#[derive(Debug)]
#[must_use]
pub struct ValidatedXmrEffectAuthorityV1 {
    schema_version: u16,
    role: ActorRole,
    swap_id: [u8; 32],
    agreement_commitment: [u8; 32],
    activation_commitment: [u8; 32],
    run_id: Box<str>,
    workflow_journal: PathBuf,
    adaptor_journal: PathBuf,
    evidence_root: PathBuf,
    lez: XmrEffectLezRpcV1,
    monero: XmrEffectMoneroRpcV1,
    maker_tools: Option<XmrMakerEffectToolsV1>,
    taker_tools: Option<XmrTakerEffectToolsV1>,
    tag14_release: Option<XmrTag14ReleaseAuthorityV1>,
}

impl ValidatedXmrEffectAuthorityV1 {
    /// Exact validated authority schema.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

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

    /// Exact countersigned agreement commitment.
    #[must_use]
    pub const fn agreement_commitment(&self) -> [u8; 32] {
        self.agreement_commitment
    }

    /// Exact countersigned activation commitment.
    #[must_use]
    pub const fn activation_commitment(&self) -> [u8; 32] {
        self.activation_commitment
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

    /// Owner-private root for effect evidence.
    #[must_use]
    pub fn evidence_root(&self) -> &Path {
        &self.evidence_root
    }

    /// Typed local LEZ sidecar authority.
    pub const fn lez(&self) -> &XmrEffectLezRpcV1 {
        &self.lez
    }

    /// Typed role-separated Monero RPC authority.
    pub const fn monero(&self) -> &XmrEffectMoneroRpcV1 {
        &self.monero
    }

    /// Fixed Maker tool profile, present only for Maker authority.
    #[must_use]
    pub const fn maker_tools(&self) -> Option<&XmrMakerEffectToolsV1> {
        self.maker_tools.as_ref()
    }

    /// Fixed Taker tool profile, present only for Taker authority.
    #[must_use]
    pub const fn taker_tools(&self) -> Option<&XmrTakerEffectToolsV1> {
        self.taker_tools.as_ref()
    }

    /// Schema-v2 Taker Tag14 release authority.
    #[must_use]
    pub const fn tag14_release(&self) -> Option<&XmrTag14ReleaseAuthorityV1> {
        self.tag14_release.as_ref()
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
        matches!(authority.schema_version, 1 | 2)
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
        &authority.monero.shared_wallet_file_password_file,
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
    validate_release_profile(&authority, &paths)?;
    let EffectAuthorityV1 {
        schema_version,
        role,
        run_id,
        workflow_journal,
        adaptor_journal,
        evidence_root,
        lez,
        monero,
        maker_tools,
        taker_tools,
        tag14_release,
        ..
    } = authority;
    Ok(ValidatedXmrEffectAuthorityV1 {
        schema_version,
        role,
        swap_id: expected_swap,
        agreement_commitment: expected_agreement,
        activation_commitment: expected_activation,
        run_id: run_id.into_boxed_str(),
        workflow_journal,
        adaptor_journal,
        evidence_root,
        lez: validated_lez(lez)?,
        monero: validated_monero(monero)?,
        maker_tools: maker_tools.map(validated_maker_tools).transpose()?,
        taker_tools: taker_tools.map(validated_taker_tools).transpose()?,
        tag14_release: tag14_release.map(validated_tag14_release).transpose()?,
    })
}

fn validated_tag14_release(release: Tag14Release) -> Result<XmrTag14ReleaseAuthorityV1> {
    Ok(XmrTag14ReleaseAuthorityV1 {
        sidecar_url: Url::parse(&release.sidecar_url)
            .context("parse validated Tag14 release sidecar URL")?,
        indexer_url: Url::parse(&release.indexer_url)
            .context("parse validated Tag14 release indexer URL")?,
        node_profile: match release.node_profile {
            Tag14ReleaseNodeProfile::Local => XmrTag14ReleaseNodeProfileV1::Local,
            Tag14ReleaseNodeProfile::OfficialPublic => XmrTag14ReleaseNodeProfileV1::OfficialPublic,
        },
        state_directory: release.state_directory,
        capability_file: release.capability_file,
        protection_key_file: release.protection_key_file,
        protection_key_id: release.protection_key_id.into_boxed_str(),
    })
}

fn validated_tool(tool: Tool) -> Result<XmrEffectToolV1> {
    Ok(XmrEffectToolV1 {
        program: tool.program,
        program_sha256: decode_digest(&tool.program_sha256)?,
        abi: tool.abi.into_boxed_str(),
    })
}

fn validated_lez(lez: LezRpc) -> Result<XmrEffectLezRpcV1> {
    Ok(XmrEffectLezRpcV1 {
        sidecar_url: Url::parse(&lez.sidecar_url).context("parse validated LEZ sidecar URL")?,
        runtime_file: lez.runtime_file,
        runtime_sha256: decode_digest(&lez.runtime_sha256)?,
        capability_file: lez.capability_file,
    })
}

fn validated_authenticated_rpc(rpc: AuthenticatedRpc) -> Result<XmrEffectAuthenticatedRpcV1> {
    Ok(XmrEffectAuthenticatedRpcV1 {
        url: Url::parse(&rpc.url).context("parse validated Monero RPC URL")?,
        username_file: rpc.username_file,
        password_file: rpc.password_file,
    })
}

fn validated_monero(monero: MoneroRpc) -> Result<XmrEffectMoneroRpcV1> {
    Ok(XmrEffectMoneroRpcV1 {
        daemon: validated_authenticated_rpc(monero.daemon)?,
        funding_wallet: validated_authenticated_rpc(monero.funding_wallet)?,
        shared_wallet: validated_authenticated_rpc(monero.shared_wallet)?,
        role_wallet: validated_authenticated_rpc(monero.role_wallet)?,
        shared_wallet_file_password_file: monero.shared_wallet_file_password_file,
    })
}

fn validated_maker_tools(tools: MakerTools) -> Result<XmrMakerEffectToolsV1> {
    Ok(XmrMakerEffectToolsV1 {
        monero_fund: validated_tool(tools.monero_fund)?,
        lez_claim: validated_tool(tools.lez_claim)?,
        finalized_classifier: validated_tool(tools.finalized_classifier)?,
        monero_refund: validated_tool(tools.monero_refund)?,
        monero_verify: validated_tool(tools.monero_verify)?,
    })
}

fn validated_taker_tools(tools: TakerTools) -> Result<XmrTakerEffectToolsV1> {
    Ok(XmrTakerEffectToolsV1 {
        tag14_authorize: validated_tool(tools.tag14_authorize)?,
        finalized_classifier: validated_tool(tools.finalized_classifier)?,
        monero_claim: validated_tool(tools.monero_claim)?,
        monero_verify: validated_tool(tools.monero_verify)?,
        tag16_refund: validated_tool(tools.tag16_refund)?,
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
    let credential_paths = [
        &rpc.daemon.username_file,
        &rpc.daemon.password_file,
        &rpc.funding_wallet.username_file,
        &rpc.funding_wallet.password_file,
        &rpc.shared_wallet.username_file,
        &rpc.shared_wallet.password_file,
        &rpc.role_wallet.username_file,
        &rpc.role_wallet.password_file,
        &rpc.shared_wallet_file_password_file,
    ];
    ensure!(
        credential_paths
            .iter()
            .enumerate()
            .all(|(index, path)| credential_paths[index + 1..]
                .iter()
                .all(|other| path != other)),
        "XMR credential paths overlap"
    );
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
            validate_tool(
                &tools.tag14_authorize,
                if authority.schema_version == 2 {
                    "lez_xmr_tag14_release_v2"
                } else {
                    "lez_xmr_tag14_authorize_v1"
                },
            )?;
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

fn validate_release_profile(
    authority: &EffectAuthorityV1,
    existing_paths: &[&PathBuf],
) -> Result<()> {
    let release = match (
        authority.schema_version,
        authority.role,
        authority.tag14_release.as_ref(),
    ) {
        (1, ActorRole::Maker | ActorRole::Taker, None) => return Ok(()),
        (2, ActorRole::Taker, Some(release)) => release,
        _ => anyhow::bail!("XMR Tag14 release authority schema/profile is invalid"),
    };
    let release_paths = [
        &release.state_directory,
        &release.capability_file,
        &release.protection_key_file,
    ];
    ensure!(
        release_paths.iter().all(|path| normalized_absolute(path))
            && release_paths
                .iter()
                .enumerate()
                .all(|(index, path)| release_paths[index + 1..].iter().all(|other| path != other))
            && release_paths
                .iter()
                .all(|path| existing_paths.iter().all(|existing| path != existing))
            && !release
                .capability_file
                .starts_with(&release.state_directory)
            && !release
                .protection_key_file
                .starts_with(&release.state_directory),
        "XMR Tag14 release authority paths are invalid or overlap"
    );
    validate_rpc(&release.sidecar_url)?;
    match release.node_profile {
        Tag14ReleaseNodeProfile::Local => validate_rpc(&release.indexer_url)?,
        Tag14ReleaseNodeProfile::OfficialPublic => ensure!(
            release.indexer_url == OFFICIAL_PUBLIC_INDEXER_ENDPOINT,
            "XMR Tag14 official indexer endpoint changed"
        ),
    }
    ensure!(
        !release.protection_key_id.is_empty()
            && release.protection_key_id.len() <= 128
            && release
                .protection_key_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')),
        "XMR Tag14 protection-key identifier is invalid"
    );
    Ok(())
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
