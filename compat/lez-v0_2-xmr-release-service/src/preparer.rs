//! One-shot composition of the evidence required to seal an XMR release journal.

use std::{
    fmt, fs,
    fs::File,
    io::Read as _,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    str::FromStr as _,
    time::Duration,
};

use lez_bridge_adapter::{
    CapabilityFileBridgeClientFactory, FreshLezBridgeTransportFactory as _, LezBridgeAdapter,
    XmrLezBridgeBindingV3,
};
use lez_bridge_protocol::{
    DiscoveryWindow, Hex32, MessageContext, Participant as BridgeParticipant,
    PrepareNativeXmrEscrowV3Request, RequestId, RunId,
};
use lez_swap_core::Participant;
use lez_xmr_monero_adapter::{
    ExpectedMoneroOutput, LoopbackRpcEndpoint, MoneroAddress, MoneroChainIdentity, MoneroNetwork,
    MoneroOutputVerifier, MoneroTopologyVerifier, MoneroTransactionId,
};
use lez_xmr_release_authority::{
    PublicationProtectionKey, ReleaseError, ReleaseSnapshot, ReleaseState, ReleaseStore,
};
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES, MoneroAddressNetworkV1,
    MoneroPrivateViewKey, XmrActivatedAgreementV1, XmrAgreementV1,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use zeroize::{Zeroize as _, Zeroizing};

use super::{
    RELEASE_JOURNAL_NAME, ReleaseNodeRouteProfile, XmrReleaseServiceConfig, stable_public_file,
    validate_config,
};

const MAX_PREPARATION_CONFIG_BYTES: usize = 64 * 1024;
const MAX_PRIVATE_TEXT_BYTES: usize = 256;
const VIEW_KEY_HEX_BYTES: usize = 64;
const PREPARATION_REQUEST_TIMEOUT: Duration = Duration::from_mins(1);

/// Current strict preparation configuration schema.
pub const XMR_RELEASE_PREPARATION_SCHEMA_VERSION: u16 = 1;

/// Public inputs selecting one exact release preparation attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrReleasePreparationConfig {
    /// Strict preparation schema.
    pub schema_version: u16,
    /// Original request identity that owns the durable tag-13 reservation.
    pub escrow_prepare_request_id: RequestId,
    /// Request identity for finalized LEZ Fund classification.
    pub fund_finality_request_id: RequestId,
    /// Request identity for tag-14 preparation.
    pub authorization_prepare_request_id: RequestId,
    /// Bounded finalized LEZ scan containing the exact Fund transaction.
    pub fund_finality_window: DiscoveryWindow,
    /// Exact Monero funding transaction committed to this attempt.
    pub monero_funding_transaction_id: Hex32,
    /// Digest-authenticated local Monero daemon origin.
    pub monero_daemon_endpoint: String,
    /// Digest-authenticated shared-wallet origin.
    pub monero_target_wallet_endpoint: String,
    /// Distinct wallet origin whose credential must fail at the target wallet.
    pub monero_foreign_wallet_endpoint: String,
}

/// Owner-controlled files consumed by one release preparation process.
pub struct XmrReleasePreparationPaths {
    /// Canonical countersigned Stage-A wire.
    pub agreement_wire_file: PathBuf,
    /// Canonical countersigned Stage-B wire.
    pub activation_wire_file: PathBuf,
    /// Owner-private shared Monero view key.
    pub monero_view_key_file: PathBuf,
    /// Completed owner-private Taker claim journal.
    pub taker_claim_journal: PathBuf,
    /// Ordinary Taker-side bridge capability used only during preparation.
    pub bridge_capability_file: PathBuf,
    /// Release-journal authentication key.
    pub protection_key_file: PathBuf,
    /// Existing owner-private directory in which the journal is created.
    pub state_directory: PathBuf,
    /// Monero daemon RPC username.
    pub daemon_username_file: PathBuf,
    /// Monero daemon RPC password.
    pub daemon_password_file: PathBuf,
    /// Shared-wallet RPC username.
    pub target_wallet_username_file: PathBuf,
    /// Shared-wallet RPC password.
    pub target_wallet_password_file: PathBuf,
    /// Foreign-wallet RPC username.
    pub foreign_wallet_username_file: PathBuf,
    /// Foreign-wallet RPC password.
    pub foreign_wallet_password_file: PathBuf,
}

impl fmt::Debug for XmrReleasePreparationPaths {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("XmrReleasePreparationPaths")
            .field("paths", &"[REDACTED]")
            .finish()
    }
}

/// Payload-free result of sealing one exact release journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XmrReleasePreparationReport {
    /// Strict report schema.
    pub schema_version: u16,
    /// Stable operation name.
    pub event: &'static str,
    /// Authenticated journal state after restart-style reload.
    pub durable_state: &'static str,
    /// Node profile admitted by this local `PoC` preparer.
    pub node_profile: &'static str,
}

impl XmrReleasePreparationReport {
    const fn prepared() -> Self {
        Self {
            schema_version: XMR_RELEASE_PREPARATION_SCHEMA_VERSION,
            event: "xmr_claim_authorization_preparation",
            durable_state: "prepared",
            node_profile: "local",
        }
    }
}

/// Redacted preparation failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum XmrReleasePreparationError {
    /// Public release or preparation configuration is invalid.
    #[error("XMR release preparation public configuration is invalid")]
    InvalidPublicConfiguration,
    /// Stage-A, Stage-B, or exact funding material is invalid.
    #[error("XMR release preparation stage material is invalid")]
    InvalidStageMaterial,
    /// A private input or private path layout is invalid.
    #[error("XMR release preparation private input is invalid")]
    InvalidPrivateInput,
    /// The release journal was not absent at the create-new boundary.
    #[error("XMR release journal is not create-new")]
    JournalNotCreateNew,
    /// The authenticated ordinary bridge client could not be created.
    #[error("XMR release preparation bridge client is unavailable")]
    BridgeClientUnavailable,
    /// The durable sidecar could not return the exact original tag-13 reservation.
    #[error("XMR release preparation escrow reservation is unavailable")]
    EscrowReservationUnavailable,
    /// Exact LEZ Fund finality could not be proved.
    #[error("XMR release preparation finalized Fund is unavailable")]
    FinalizedFundUnavailable,
    /// The isolated authenticated Monero topology could not be proved.
    #[error("XMR release preparation Monero topology is unavailable")]
    MoneroTopologyUnavailable,
    /// The exact confirmed Monero output could not be proved.
    #[error("XMR release preparation Monero output is unavailable")]
    MoneroOutputUnavailable,
    /// The completed Taker journal could not prepare tag 14.
    #[error("XMR release preparation authorization is unavailable")]
    AuthorizationUnavailable,
    /// The release-journal protection key could not be loaded.
    #[error("XMR release preparation protection key is unavailable")]
    ProtectionKeyUnavailable,
    /// The create-new authenticated journal could not be sealed and reloaded.
    #[error("XMR release preparation journal sealing failed")]
    JournalSealingFailed,
}

/// Reads one bounded stable owner-controlled preparation configuration.
///
/// # Errors
///
/// Rejects missing, linked, writable-by-others, unstable, oversized, or invalid JSON.
pub fn read_xmr_release_preparation_config(
    path: impl AsRef<Path>,
) -> Result<XmrReleasePreparationConfig, XmrReleasePreparationError> {
    read_public_json(path.as_ref(), MAX_PREPARATION_CONFIG_BYTES)
        .map_err(|()| XmrReleasePreparationError::InvalidPublicConfiguration)
}

struct ValidatedPreparationInputs {
    agreement: XmrAgreementV1,
    activation: XmrActivatedAgreementV1,
    binding: XmrLezBridgeBindingV3,
    daemon: LoopbackRpcEndpoint,
    target_wallet: LoopbackRpcEndpoint,
    foreign_wallet: LoopbackRpcEndpoint,
    identity: MoneroChainIdentity,
    expected_output: ExpectedMoneroOutput,
    protection_key: PublicationProtectionKey,
}

fn validate_and_reserve_inputs(
    release: &XmrReleaseServiceConfig,
    preparation: &XmrReleasePreparationConfig,
    paths: &XmrReleasePreparationPaths,
) -> Result<ValidatedPreparationInputs, XmrReleasePreparationError> {
    validate_preparation_config(release, preparation)?;
    validate_private_layout(paths)?;
    let agreement_bytes =
        read_public_bytes(&paths.agreement_wire_file, MAX_XMR_AGREEMENT_WIRE_BYTES)?;
    let agreement = XmrAgreementV1::from_wire(&agreement_bytes)
        .map_err(|_| XmrReleasePreparationError::InvalidStageMaterial)?;
    let view_key = read_view_key(&paths.monero_view_key_file)?;
    let activation_bytes =
        read_public_bytes(&paths.activation_wire_file, MAX_XMR_ACTIVATION_WIRE_BYTES)?;
    let activation = XmrActivatedAgreementV1::from_wire(&agreement, &activation_bytes, &view_key)
        .map_err(|_| XmrReleasePreparationError::InvalidStageMaterial)?;
    let binding = XmrLezBridgeBindingV3::new(&agreement, &activation)
        .map_err(|_| XmrReleasePreparationError::InvalidStageMaterial)?;
    if binding.terms() != release.terms {
        return Err(XmrReleasePreparationError::InvalidStageMaterial);
    }
    let daemon = read_rpc_endpoint(
        &preparation.monero_daemon_endpoint,
        &paths.daemon_username_file,
        &paths.daemon_password_file,
    )?;
    let target_wallet = read_rpc_endpoint(
        &preparation.monero_target_wallet_endpoint,
        &paths.target_wallet_username_file,
        &paths.target_wallet_password_file,
    )?;
    let foreign_wallet = read_rpc_endpoint(
        &preparation.monero_foreign_wallet_endpoint,
        &paths.foreign_wallet_username_file,
        &paths.foreign_wallet_password_file,
    )?;
    let monero_terms = agreement.body().monero();
    if monero_terms.network() != MoneroAddressNetworkV1::Regtest {
        return Err(XmrReleasePreparationError::InvalidStageMaterial);
    }
    let identity = MoneroChainIdentity::new(MoneroNetwork::Regtest, monero_terms.genesis_hash())
        .map_err(|_| XmrReleasePreparationError::InvalidStageMaterial)?;
    let address = MoneroAddress::from_str(monero_terms.address())
        .map_err(|_| XmrReleasePreparationError::InvalidStageMaterial)?;
    let expected_output = ExpectedMoneroOutput::new(
        MoneroTransactionId(*preparation.monero_funding_transaction_id.as_bytes()),
        address,
        monero_terms.amount_piconero(),
    )
    .map_err(|_| XmrReleasePreparationError::InvalidStageMaterial)?;
    let protection_key = PublicationProtectionKey::from_owner_private_file(
        release.protection_key_id.clone(),
        &paths.protection_key_file,
    )
    .map_err(|_| XmrReleasePreparationError::ProtectionKeyUnavailable)?;
    Ok(ValidatedPreparationInputs {
        agreement,
        activation,
        binding,
        daemon,
        target_wallet,
        foreign_wallet,
        identity,
        expected_output,
        protection_key,
    })
}

/// Proves every Stage-B release precondition and seals one create-new journal.
///
/// This function has no publication client and cannot send tag 14. It consumes
/// finalized LEZ Fund evidence, authenticated isolated Monero evidence, and a
/// completed Taker journal into the existing at-most-once release store.
///
/// # Errors
///
/// Fails closed on public, private, stage, bridge, chain, authorization, or
/// journal errors. Returned errors and the success report contain no payloads.
#[allow(
    clippy::too_many_lines,
    reason = "one linear release-authority flow keeps fail-closed evidence ordering auditable"
)]
pub async fn prepare_xmr_release_service(
    release: XmrReleaseServiceConfig,
    preparation: XmrReleasePreparationConfig,
    paths: &XmrReleasePreparationPaths,
) -> Result<XmrReleasePreparationReport, XmrReleasePreparationError> {
    let ValidatedPreparationInputs {
        agreement,
        activation,
        binding,
        daemon,
        target_wallet,
        foreign_wallet,
        identity,
        expected_output,
        protection_key,
    } = validate_and_reserve_inputs(&release, &preparation, paths)?;
    let topology_verifier = MoneroTopologyVerifier::new(
        release.run_id.clone(),
        identity,
        &daemon,
        &target_wallet,
        &foreign_wallet,
    )
    .map_err(|_| XmrReleasePreparationError::MoneroTopologyUnavailable)?;
    let output_verifier = MoneroOutputVerifier::new(identity, &daemon, &target_wallet)
        .map_err(|_| XmrReleasePreparationError::MoneroOutputUnavailable)?;
    let store = create_new_release_store(paths)?;

    let client = CapabilityFileBridgeClientFactory::new(
        release.sidecar_endpoint.clone(),
        &paths.bridge_capability_file,
        release.run_id.clone(),
        release.runtime.clone(),
        PREPARATION_REQUEST_TIMEOUT,
    )
    .fresh_transport()
    .map_err(|_| XmrReleasePreparationError::BridgeClientUnavailable)?;
    let preparation_context = MessageContext::new(
        release.run_id.clone(),
        preparation.escrow_prepare_request_id,
        BridgeParticipant::Taker,
    );
    let reserved = client
        .prepare_native_xmr_escrow_v3(PrepareNativeXmrEscrowV3Request::new(
            preparation_context.clone(),
            release.runtime.clone(),
            binding.terms(),
        ))
        .await
        .map_err(|_| XmrReleasePreparationError::EscrowReservationUnavailable)?;
    if reserved.context != preparation_context || reserved.terms != binding.terms() {
        return Err(XmrReleasePreparationError::EscrowReservationUnavailable);
    }
    let adapter = LezBridgeAdapter::new(
        client,
        release.run_id.clone(),
        release.runtime.clone(),
        Participant::Taker,
    )
    .map_err(|_| XmrReleasePreparationError::BridgeClientUnavailable)?;
    let first_lock = adapter
        .prove_finalized_xmr_first_lock_v3(
            &binding,
            preparation.fund_finality_request_id,
            reserved.funding,
            preparation.fund_finality_window,
        )
        .await
        .map_err(|_| XmrReleasePreparationError::FinalizedFundUnavailable)?;

    let topology = topology_verifier
        .verify()
        .await
        .map_err(|_| XmrReleasePreparationError::MoneroTopologyUnavailable)?;
    let observation = output_verifier
        .verify(&expected_output)
        .await
        .map_err(|_| XmrReleasePreparationError::MoneroOutputUnavailable)?;

    let authorization = adapter
        .prepare_xmr_claim_authorization_from_taker_journal_v3(
            &agreement,
            &activation,
            &binding,
            preparation.authorization_prepare_request_id,
            &paths.taker_claim_journal,
        )
        .await
        .map_err(|_| XmrReleasePreparationError::AuthorizationUnavailable)?;
    let snapshot = store
        .prepare_xmr_claim_release(
            &agreement,
            &activation,
            first_lock,
            authorization,
            observation,
            topology,
            &protection_key,
        )
        .map_err(|_| XmrReleasePreparationError::JournalSealingFailed)?;
    if snapshot.state() != ReleaseState::Prepared {
        return Err(XmrReleasePreparationError::JournalSealingFailed);
    }
    authenticate_reopened_release(
        store,
        paths,
        &agreement,
        &release.run_id,
        &protection_key,
        &snapshot,
    )?;
    Ok(XmrReleasePreparationReport::prepared())
}

fn authenticate_reopened_release(
    store: ReleaseStore,
    paths: &XmrReleasePreparationPaths,
    agreement: &XmrAgreementV1,
    run_id: &RunId,
    protection_key: &PublicationProtectionKey,
    expected: &ReleaseSnapshot,
) -> Result<(), XmrReleasePreparationError> {
    drop(store);
    let reopened = ReleaseStore::open(paths.state_directory.join(RELEASE_JOURNAL_NAME))
        .map_err(|_| XmrReleasePreparationError::JournalSealingFailed)?;
    let reloaded = reopened
        .load_xmr_claim_release(agreement.body().swap_id(), run_id, protection_key)
        .map_err(|_| XmrReleasePreparationError::JournalSealingFailed)?;
    if reloaded.state() != ReleaseState::Prepared || &reloaded != expected {
        return Err(XmrReleasePreparationError::JournalSealingFailed);
    }
    Ok(())
}

fn validate_preparation_config(
    release: &XmrReleaseServiceConfig,
    preparation: &XmrReleasePreparationConfig,
) -> Result<(), XmrReleasePreparationError> {
    validate_config(release).map_err(|_| XmrReleasePreparationError::InvalidPublicConfiguration)?;
    if preparation.schema_version != XMR_RELEASE_PREPARATION_SCHEMA_VERSION
        || release.node_profile != ReleaseNodeRouteProfile::Local
        || preparation.monero_funding_transaction_id == Hex32::from_bytes([0; 32])
        || preparation.escrow_prepare_request_id == preparation.fund_finality_request_id
        || preparation.escrow_prepare_request_id == preparation.authorization_prepare_request_id
        || preparation.fund_finality_request_id == preparation.authorization_prepare_request_id
    {
        return Err(XmrReleasePreparationError::InvalidPublicConfiguration);
    }
    Ok(())
}

fn validate_private_layout(
    paths: &XmrReleasePreparationPaths,
) -> Result<(), XmrReleasePreparationError> {
    let state_metadata = fs::symlink_metadata(&paths.state_directory)
        .map_err(|_| XmrReleasePreparationError::InvalidPrivateInput)?;
    if !state_metadata.file_type().is_dir()
        || state_metadata.uid() != rustix::process::geteuid().as_raw()
        || state_metadata.permissions().mode() & 0o7777 != 0o700
    {
        return Err(XmrReleasePreparationError::InvalidPrivateInput);
    }
    let journal = paths.state_directory.join(RELEASE_JOURNAL_NAME);
    match fs::symlink_metadata(&journal) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => return Err(XmrReleasePreparationError::JournalNotCreateNew),
        Err(_) => return Err(XmrReleasePreparationError::InvalidPrivateInput),
    }
    for (path, maximum) in [
        (&paths.monero_view_key_file, MAX_PRIVATE_TEXT_BYTES as u64),
        (&paths.taker_claim_journal, u64::MAX),
        (&paths.bridge_capability_file, 512),
        (&paths.protection_key_file, 512),
        (&paths.daemon_username_file, MAX_PRIVATE_TEXT_BYTES as u64),
        (&paths.daemon_password_file, MAX_PRIVATE_TEXT_BYTES as u64),
        (
            &paths.target_wallet_username_file,
            MAX_PRIVATE_TEXT_BYTES as u64,
        ),
        (
            &paths.target_wallet_password_file,
            MAX_PRIVATE_TEXT_BYTES as u64,
        ),
        (
            &paths.foreign_wallet_username_file,
            MAX_PRIVATE_TEXT_BYTES as u64,
        ),
        (
            &paths.foreign_wallet_password_file,
            MAX_PRIVATE_TEXT_BYTES as u64,
        ),
    ] {
        validate_private_metadata_at(path, 1, maximum)?;
        validate_trusted_parent(path)?;
    }

    let state = fs::canonicalize(&paths.state_directory)
        .map_err(|_| XmrReleasePreparationError::InvalidPrivateInput)?;
    let input_paths = [
        &paths.agreement_wire_file,
        &paths.activation_wire_file,
        &paths.monero_view_key_file,
        &paths.taker_claim_journal,
        &paths.bridge_capability_file,
        &paths.protection_key_file,
        &paths.daemon_username_file,
        &paths.daemon_password_file,
        &paths.target_wallet_username_file,
        &paths.target_wallet_password_file,
        &paths.foreign_wallet_username_file,
        &paths.foreign_wallet_password_file,
    ];
    let canonical = input_paths
        .into_iter()
        .map(|path| {
            fs::canonicalize(path).map_err(|_| XmrReleasePreparationError::InvalidPrivateInput)
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (index, path) in canonical.iter().enumerate() {
        if path == &state || path.starts_with(&state) {
            return Err(XmrReleasePreparationError::InvalidPrivateInput);
        }
        if canonical.iter().skip(index + 1).any(|other| other == path) {
            return Err(XmrReleasePreparationError::InvalidPrivateInput);
        }
    }
    Ok(())
}

fn read_rpc_endpoint(
    endpoint: &str,
    username_path: &Path,
    password_path: &Path,
) -> Result<LoopbackRpcEndpoint, XmrReleasePreparationError> {
    let username = read_private_text(username_path)?;
    let password = read_private_text(password_path)?;
    LoopbackRpcEndpoint::new(endpoint, username.to_string(), password.to_string())
        .map_err(|_| XmrReleasePreparationError::InvalidPrivateInput)
}

fn read_view_key(path: &Path) -> Result<MoneroPrivateViewKey, XmrReleasePreparationError> {
    let mut encoded = read_private_text(path)?;
    if encoded.len() != VIEW_KEY_HEX_BYTES
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(XmrReleasePreparationError::InvalidPrivateInput);
    }
    let mut bytes = Zeroizing::new([0_u8; 32]);
    if hex::decode_to_slice(encoded.as_bytes(), bytes.as_mut()).is_err() {
        return Err(XmrReleasePreparationError::InvalidPrivateInput);
    }
    encoded.zeroize();
    MoneroPrivateViewKey::from_monero_little_endian(*bytes)
        .map_err(|_| XmrReleasePreparationError::InvalidPrivateInput)
}

fn read_private_text(path: &Path) -> Result<Zeroizing<String>, XmrReleasePreparationError> {
    let mut bytes = Zeroizing::new(read_stable_bytes(path, MAX_PRIVATE_TEXT_BYTES, true)?);
    if bytes.ends_with(b"\r\n") {
        let new_length = bytes.len() - 2;
        bytes.truncate(new_length);
    } else if bytes.ends_with(b"\n") {
        let new_length = bytes.len() - 1;
        bytes.truncate(new_length);
    }
    if bytes.is_empty() || bytes.iter().any(u8::is_ascii_whitespace) {
        return Err(XmrReleasePreparationError::InvalidPrivateInput);
    }
    let owned = String::from_utf8(std::mem::take(&mut *bytes))
        .map_err(|_| XmrReleasePreparationError::InvalidPrivateInput)?;
    Ok(Zeroizing::new(owned))
}

fn read_public_bytes(path: &Path, max_bytes: usize) -> Result<Vec<u8>, XmrReleasePreparationError> {
    read_stable_bytes(path, max_bytes, false)
        .map_err(|_| XmrReleasePreparationError::InvalidStageMaterial)
}

fn read_public_json<T: DeserializeOwned>(path: &Path, max_bytes: usize) -> Result<T, ()> {
    let bytes = read_stable_bytes(path, max_bytes, false).map_err(|_| ())?;
    serde_json::from_slice(&bytes).map_err(|_| ())
}

fn read_stable_bytes(
    path: &Path,
    max_bytes: usize,
    private: bool,
) -> Result<Vec<u8>, XmrReleasePreparationError> {
    let before =
        fs::symlink_metadata(path).map_err(|_| XmrReleasePreparationError::InvalidPrivateInput)?;
    validate_file_metadata(&before, max_bytes, private)?;
    let file = File::open(path).map_err(|_| XmrReleasePreparationError::InvalidPrivateInput)?;
    let opened = file
        .metadata()
        .map_err(|_| XmrReleasePreparationError::InvalidPrivateInput)?;
    validate_file_metadata(&opened, max_bytes, private)?;
    if before.dev() != opened.dev() || before.ino() != opened.ino() {
        return Err(XmrReleasePreparationError::InvalidPrivateInput);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(opened.len()).unwrap_or(max_bytes) + 1);
    (&file)
        .take(u64::try_from(max_bytes).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| XmrReleasePreparationError::InvalidPrivateInput)?;
    let opened_after = file
        .metadata()
        .map_err(|_| XmrReleasePreparationError::InvalidPrivateInput)?;
    let path_after =
        fs::symlink_metadata(path).map_err(|_| XmrReleasePreparationError::InvalidPrivateInput)?;
    validate_file_metadata(&opened_after, max_bytes, private)?;
    validate_file_metadata(&path_after, max_bytes, private)?;
    if !stable_public_file(&opened, &opened_after)
        || !stable_public_file(&opened, &path_after)
        || bytes.is_empty()
        || bytes.len() > max_bytes
    {
        bytes.zeroize();
        return Err(XmrReleasePreparationError::InvalidPrivateInput);
    }
    Ok(bytes)
}

fn validate_private_metadata_at(
    path: &Path,
    min_bytes: u64,
    max_bytes: u64,
) -> Result<(), XmrReleasePreparationError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| XmrReleasePreparationError::InvalidPrivateInput)?;
    if !metadata.file_type().is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o7777 != 0o600
        || metadata.len() < min_bytes
        || metadata.len() > max_bytes
    {
        return Err(XmrReleasePreparationError::InvalidPrivateInput);
    }
    Ok(())
}

fn validate_trusted_parent(path: &Path) -> Result<(), XmrReleasePreparationError> {
    let parent = path
        .parent()
        .ok_or(XmrReleasePreparationError::InvalidPrivateInput)?;
    let canonical =
        fs::canonicalize(parent).map_err(|_| XmrReleasePreparationError::InvalidPrivateInput)?;
    let metadata = fs::symlink_metadata(canonical)
        .map_err(|_| XmrReleasePreparationError::InvalidPrivateInput)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(XmrReleasePreparationError::InvalidPrivateInput);
    }
    Ok(())
}

fn validate_file_metadata(
    metadata: &fs::Metadata,
    max_bytes: usize,
    private: bool,
) -> Result<(), XmrReleasePreparationError> {
    let mode = metadata.permissions().mode() & 0o7777;
    if !metadata.file_type().is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() != 1
        || metadata.len() == 0
        || metadata.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX)
        || (private && mode != 0o600)
        || (!private && mode & 0o022 != 0)
    {
        return Err(XmrReleasePreparationError::InvalidPrivateInput);
    }
    Ok(())
}

fn create_new_release_store(
    paths: &XmrReleasePreparationPaths,
) -> Result<ReleaseStore, XmrReleasePreparationError> {
    let journal_path = paths.state_directory.join(RELEASE_JOURNAL_NAME);
    ReleaseStore::create_new(journal_path).map_err(|error| match error {
        ReleaseError::DatabaseAlreadyExists => XmrReleasePreparationError::JournalNotCreateNew,
        _ => XmrReleasePreparationError::JournalSealingFailed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_is_exact_and_payload_free() {
        assert_eq!(
            serde_json::to_value(XmrReleasePreparationReport::prepared()).unwrap(),
            serde_json::json!({
                "schema_version": 1,
                "event": "xmr_claim_authorization_preparation",
                "durable_state": "prepared",
                "node_profile": "local"
            })
        );
    }

    #[test]
    fn private_text_requires_exact_owner_only_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credential");
        fs::write(&path, b"private-value\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            read_private_text(&path).unwrap_err(),
            XmrReleasePreparationError::InvalidPrivateInput
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(&*read_private_text(&path).unwrap(), "private-value");
    }

    #[test]
    fn release_store_is_create_new_and_owner_only() {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let paths = test_paths(directory.path());
        let store = create_new_release_store(&paths).unwrap();
        drop(store);
        let journal = directory.path().join(RELEASE_JOURNAL_NAME);
        assert_eq!(
            fs::metadata(&journal).unwrap().permissions().mode() & 0o7777,
            0o600
        );
        assert!(matches!(
            create_new_release_store(&paths),
            Err(XmrReleasePreparationError::JournalNotCreateNew)
        ));
    }

    #[test]
    fn paths_and_errors_are_redacted() {
        let paths = test_paths(Path::new("/secret/state"));
        assert!(!format!("{paths:?}").contains("/secret"));
        for error in [
            XmrReleasePreparationError::InvalidPublicConfiguration,
            XmrReleasePreparationError::InvalidStageMaterial,
            XmrReleasePreparationError::InvalidPrivateInput,
            XmrReleasePreparationError::JournalNotCreateNew,
            XmrReleasePreparationError::BridgeClientUnavailable,
            XmrReleasePreparationError::EscrowReservationUnavailable,
            XmrReleasePreparationError::FinalizedFundUnavailable,
            XmrReleasePreparationError::MoneroTopologyUnavailable,
            XmrReleasePreparationError::MoneroOutputUnavailable,
            XmrReleasePreparationError::AuthorizationUnavailable,
            XmrReleasePreparationError::ProtectionKeyUnavailable,
            XmrReleasePreparationError::JournalSealingFailed,
        ] {
            assert!(!error.to_string().contains("/secret"));
            assert!(!format!("{error:?}").contains("/secret"));
        }
    }

    fn test_paths(state_directory: &Path) -> XmrReleasePreparationPaths {
        XmrReleasePreparationPaths {
            agreement_wire_file: "/secret/agreement".into(),
            activation_wire_file: "/secret/activation".into(),
            monero_view_key_file: "/secret/view".into(),
            taker_claim_journal: "/secret/claim".into(),
            bridge_capability_file: "/secret/capability".into(),
            protection_key_file: "/secret/key".into(),
            state_directory: state_directory.to_path_buf(),
            daemon_username_file: "/secret/daemon-user".into(),
            daemon_password_file: "/secret/daemon-pass".into(),
            target_wallet_username_file: "/secret/target-user".into(),
            target_wallet_password_file: "/secret/target-pass".into(),
            foreign_wallet_username_file: "/secret/foreign-user".into(),
            foreign_wallet_password_file: "/secret/foreign-pass".into(),
        }
    }
}
