//! Prepared initiation catalog: owner-configured authorities the Taker Node may
//! admit and execute, one entry per swap, for the Bitcoin route and (with
//! `pair-zec`) the Zcash route. Every entry carries the same private material;
//! the pair decides which take path and which actor run it.

use super::{TakerServiceStartupError, validate_normalized_absolute};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Path, PathBuf},
};

use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, SwapId};
use lez_swap_store::{
    MakerOfferId, SqliteTakerFacadeStore, TakerInitiationAuthorityV1, TakerInitiationFactsV1,
    TakerPrivateFileBindingV1,
};
use secp256k1::SecretKey;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::{
    AuthenticatedOfferRefV1, RunLocalDelivery,
    secure_file::{PrivateFileIdentity, PrivateFileSnapshot, read_private_file_snapshot},
};

const MAX_PREPARED_INITIATIONS: usize = 256;
const MAX_PREPARED_INPUT_BYTES: u64 = 256 * 1024;
const MAX_PREPARED_RECEIPT_BYTES: u64 = 16 * 1024;
const MAX_SIGNING_KEY_BYTES: u64 = 32;

/// Existing registry plus a bounded static catalog of prepared ZEC authorities.
pub struct ConfiguredTakerInitiationContext {
    execute_prepared: bool,
    registry: SqliteTakerFacadeStore,
    prepared_by_offer: BTreeMap<Box<str>, PreparedTakerInitiationV1>,
}

impl ConfiguredTakerInitiationContext {
    /// Whether admitted prepared ZEC swaps execute Chat acceptance before response.
    #[must_use]
    pub const fn execution_enabled(&self) -> bool {
        self.execute_prepared
    }

    /// Reports whether at least one prepared entry runs the given pair.
    #[must_use]
    pub fn has_pair(&self, pair: Pair) -> bool {
        self.prepared_by_offer
            .values()
            .any(|prepared| prepared.facts().route().pair() == pair)
    }

    /// Number of configured role-fixed ZEC initiation entries.
    #[must_use]
    pub fn prepared_count(&self) -> usize {
        self.prepared_by_offer.len()
    }

    /// Looks up one fixed entry by authenticated offer identity.
    #[must_use]
    pub fn prepared_for_offer(
        &self,
        offer_id: &MakerOfferId,
    ) -> Option<&PreparedTakerInitiationV1> {
        self.prepared_by_offer.get(offer_id.as_str())
    }

    /// Looks up one fixed entry by application swap identity.
    ///
    /// Startup validation rejects duplicate swap identities and bounds the catalog.
    #[must_use]
    pub fn prepared_for_swap(&self, swap_id: &SwapId) -> Option<&PreparedTakerInitiationV1> {
        self.prepared_by_offer
            .values()
            .find(|prepared| prepared.swap_id() == swap_id)
    }

    /// Captures or revalidates the completed receipt for this process incarnation.
    pub(crate) fn bind_prepared_receipt(&mut self, swap_id: &SwapId) -> Result<(), ()> {
        let prepared = self
            .prepared_by_offer
            .values_mut()
            .find(|prepared| prepared.swap_id() == swap_id)
            .ok_or(())?;
        let binding =
            load_required_receipt_binding(prepared.execution.receipt_output()).map_err(|_| ())?;
        if prepared
            .execution
            .receipt_binding
            .is_some_and(|expected| expected != binding)
        {
            return Err(());
        }
        prepared.execution.receipt_binding = Some(binding);
        Ok(())
    }

    /// Mutably borrows the already-existing standalone registry.
    #[must_use]
    pub const fn registry_mut(&mut self) -> &mut SqliteTakerFacadeStore {
        &mut self.registry
    }
}

impl fmt::Debug for ConfiguredTakerInitiationContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfiguredTakerInitiationContext")
            .field("execution_enabled", &self.execute_prepared)
            .field("registry", &"[REDACTED]")
            .field("prepared_count", &self.prepared_by_offer.len())
            .finish_non_exhaustive()
    }
}

/// Static service-owned authority selected after a client supplies public facts.
#[derive(Clone)]
pub struct PreparedTakerInitiationV1 {
    facts: TakerInitiationFactsV1,
    reservation_id: RequestId,
    authority: TakerInitiationAuthorityV1,
    execution: PreparedExecutionV1,
}

impl PreparedTakerInitiationV1 {
    /// Exact route-fixed public facts prepared by the owner.
    #[must_use]
    pub const fn facts(&self) -> &TakerInitiationFactsV1 {
        &self.facts
    }

    /// Fixed application swap identity; never supplied by the client.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        self.facts.swap_id()
    }

    /// Fixed authenticated offer identity.
    #[must_use]
    pub const fn offer_id(&self) -> &MakerOfferId {
        self.facts.offer_id()
    }

    /// Fixed Maker reservation identity.
    pub const fn reservation_id(&self) -> &RequestId {
        &self.reservation_id
    }

    /// Maker identity derived from the referenced Delivery source.
    #[must_use]
    pub const fn maker_identity(&self) -> &[u8; 33] {
        self.facts.maker_identity()
    }

    /// Borrows the complete redacted authority for atomic registry admission.
    #[must_use]
    pub const fn authority(&self) -> &TakerInitiationAuthorityV1 {
        &self.authority
    }

    /// Borrows the redacted execution-ready material retained at startup.
    #[must_use]
    pub const fn execution(&self) -> &PreparedExecutionV1 {
        &self.execution
    }
}

impl fmt::Debug for PreparedTakerInitiationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedTakerInitiationV1")
            .field("configured", &true)
            .finish_non_exhaustive()
    }
}

/// Process-incarnation binding for one completed prepared receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PreparedReceiptBindingV1 {
    sha256: [u8; 32],
    identity: PrivateFileIdentity,
}

impl PreparedReceiptBindingV1 {
    pub(crate) const fn sha256(self) -> [u8; 32] {
        self.sha256
    }

    pub(crate) const fn identity(self) -> PrivateFileIdentity {
        self.identity
    }
}

/// Cloneable, execution-ready ZEC input retained inside the owner process.
///
/// Its public surface is intentionally opaque: only the service executor in
/// this crate can borrow private bytes or configured paths, while `Debug`
/// never reveals either.
#[derive(Clone)]
pub struct PreparedExecutionV1 {
    authenticated_offer: AuthenticatedOfferRefV1,
    unsigned_draft_path: PathBuf,
    unsigned_draft: Zeroizing<Vec<u8>>,
    unsigned_draft_sha256: [u8; 32],
    signing_key_path: PathBuf,
    signing_key: Zeroizing<[u8; 32]>,
    source_config_path: PathBuf,
    source_config_sha256: [u8; 32],
    chat_socket: PathBuf,
    agreement_output: PathBuf,
    actor_root: PathBuf,
    receipt_output: PathBuf,
    receipt_binding: Option<PreparedReceiptBindingV1>,
}

impl PreparedExecutionV1 {
    pub(crate) const fn authenticated_offer(&self) -> &AuthenticatedOfferRefV1 {
        &self.authenticated_offer
    }

    pub(crate) fn unsigned_draft_path(&self) -> &Path {
        &self.unsigned_draft_path
    }

    pub(crate) fn unsigned_draft(&self) -> &[u8] {
        &self.unsigned_draft
    }

    pub(crate) const fn unsigned_draft_sha256(&self) -> [u8; 32] {
        self.unsigned_draft_sha256
    }

    pub(crate) fn signing_key(&self) -> &[u8; 32] {
        &self.signing_key
    }

    pub(crate) fn signing_key_path(&self) -> &Path {
        &self.signing_key_path
    }

    pub(crate) fn source_config_path(&self) -> &Path {
        &self.source_config_path
    }

    pub(crate) const fn source_config_sha256(&self) -> [u8; 32] {
        self.source_config_sha256
    }

    pub(crate) fn chat_socket(&self) -> &Path {
        &self.chat_socket
    }

    pub(crate) fn agreement_output(&self) -> &Path {
        &self.agreement_output
    }

    pub(crate) fn actor_root(&self) -> &Path {
        &self.actor_root
    }

    pub(crate) fn receipt_output(&self) -> &Path {
        &self.receipt_output
    }

    pub(crate) const fn receipt_binding(&self) -> Option<PreparedReceiptBindingV1> {
        self.receipt_binding
    }
}

impl fmt::Debug for PreparedExecutionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedExecutionV1")
            .field("configured", &true)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InitiationConfigurationV1 {
    /// Execute the prepared take when an initiation is admitted (`execute_prepared_zec`
    /// is the historical spelling and still accepted).
    #[serde(default, alias = "execute_prepared_zec")]
    execute_prepared: bool,
    registry_database: PathBuf,
    #[cfg(feature = "pair-zec")]
    #[serde(default)]
    prepared_zec: Vec<PreparedConfigurationV1>,
    #[serde(default)]
    prepared_btc: Vec<PreparedConfigurationV1>,
}

impl InitiationConfigurationV1 {
    /// Every configured entry with the pair its array names.
    fn entries(&self) -> impl Iterator<Item = (Pair, &PreparedConfigurationV1)> {
        #[cfg(feature = "pair-zec")]
        let zec = self.prepared_zec.iter().map(|entry| (Pair::Zcash, entry));
        #[cfg(not(feature = "pair-zec"))]
        let zec = std::iter::empty();
        zec.chain(self.prepared_btc.iter().map(|entry| (Pair::Bitcoin, entry)))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedConfigurationV1 {
    source_id: Box<str>,
    swap_id: SwapId,
    offer_id: MakerOfferId,
    reservation_id: RequestId,
    foreign_units: u64,
    lez_units: u128,
    signed_envelope: ImmutablePrivateFileV1,
    unsigned_draft: ImmutablePrivateFileV1,
    signing_key: SecretPrivateFileV1,
    source_config: ImmutablePrivateFileV1,
    agreement_output: PathBuf,
    actor_root: PathBuf,
    receipt_output: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImmutablePrivateFileV1 {
    path: PathBuf,
    #[serde(deserialize_with = "deserialize_sha256")]
    sha256: [u8; 32],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretPrivateFileV1 {
    path: PathBuf,
}

pub(super) fn build_initiation_context(
    configuration: &InitiationConfigurationV1,
    source_bindings: &BTreeMap<Box<str>, (usize, [u8; 33])>,
    delivery_sources: &[RunLocalDelivery],
    chat_socket: Option<&Path>,
) -> Result<ConfiguredTakerInitiationContext, TakerServiceStartupError> {
    let registry = SqliteTakerFacadeStore::open_existing(&configuration.registry_database)
        .map_err(|_| TakerServiceStartupError::InitiationUnavailable)?;
    let mut prepared_by_offer = BTreeMap::new();

    for (expected_pair, configured) in configuration.entries() {
        let (source_index, maker_identity) = source_bindings
            .get(&configured.source_id)
            .copied()
            .ok_or(TakerServiceStartupError::InvalidConfiguration)?;
        let (signed_envelope, signed_snapshot) = read_immutable_snapshot_binding(
            &configured.signed_envelope,
            "prepared Taker signed envelope",
        )?;
        let authenticated = delivery_sources
            .get(source_index)
            .ok_or(TakerServiceStartupError::InvalidConfiguration)?
            .authenticate_envelope(signed_snapshot.bytes())
            .map_err(|_| TakerServiceStartupError::InvalidConfiguration)?;
        let route = authenticated.offer().route();
        if authenticated.maker_identity() != &maker_identity
            || authenticated.offer().id() != &configured.offer_id
            || route.pair() != expected_pair
            || authenticated
                .offer()
                .quote_foreign_amount(configured.foreign_units)
                .ok()
                != Some(configured.lez_units)
            || authenticated.commitment() != configured.signed_envelope.sha256
        {
            return Err(TakerServiceStartupError::InvalidConfiguration);
        }
        let (unsigned_draft, unsigned_draft_snapshot) = read_immutable_snapshot_binding(
            &configured.unsigned_draft,
            "prepared Taker unsigned draft",
        )?;
        let (signing_key, signing_key_bytes) =
            read_secret_binding(&configured.signing_key, "prepared Taker signing key")?;
        let source_config = read_immutable_binding(
            &configured.source_config,
            "prepared Taker actor configuration",
        )?;
        let authority = TakerInitiationAuthorityV1::new(
            configured.source_id.clone(),
            configured.reservation_id.clone(),
            signed_envelope,
            unsigned_draft,
            signing_key,
            source_config,
            configured.agreement_output.clone(),
            configured.actor_root.clone(),
            configured.receipt_output.clone(),
        )
        .map_err(|_| TakerServiceStartupError::InvalidConfiguration)?;
        let facts = TakerInitiationFactsV1::new(
            configured.swap_id.clone(),
            configured.offer_id.clone(),
            route,
            maker_identity,
            configured.signed_envelope.sha256,
            configured.foreign_units,
            configured.lez_units,
        )
        .map_err(|_| TakerServiceStartupError::InvalidConfiguration)?;
        let execution = PreparedExecutionV1 {
            authenticated_offer: authenticated,
            unsigned_draft_path: configured.unsigned_draft.path.clone(),
            unsigned_draft: unsigned_draft_snapshot.into_bytes(),
            unsigned_draft_sha256: configured.unsigned_draft.sha256,
            signing_key_path: configured.signing_key.path.clone(),
            signing_key: signing_key_bytes,
            source_config_path: configured.source_config.path.clone(),
            source_config_sha256: configured.source_config.sha256,
            chat_socket: chat_socket
                .ok_or(TakerServiceStartupError::InvalidConfiguration)?
                .to_path_buf(),
            agreement_output: configured.agreement_output.clone(),
            actor_root: configured.actor_root.clone(),
            receipt_output: configured.receipt_output.clone(),
            receipt_binding: load_optional_receipt_binding(&configured.receipt_output)?,
        };
        let entry = PreparedTakerInitiationV1 {
            facts,
            reservation_id: configured.reservation_id.clone(),
            authority,
            execution,
        };
        if prepared_by_offer
            .insert(configured.offer_id.as_str().into(), entry)
            .is_some()
        {
            return Err(TakerServiceStartupError::InvalidConfiguration);
        }
    }

    Ok(ConfiguredTakerInitiationContext {
        execute_prepared: configuration.execute_prepared,
        registry,
        prepared_by_offer,
    })
}

fn load_optional_receipt_binding(
    path: &Path,
) -> Result<Option<PreparedReceiptBindingV1>, TakerServiceStartupError> {
    match fs::symlink_metadata(path) {
        Ok(_) => load_required_receipt_binding(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(TakerServiceStartupError::InitiationUnavailable),
    }
}

fn load_required_receipt_binding(
    path: &Path,
) -> Result<PreparedReceiptBindingV1, TakerServiceStartupError> {
    let snapshot = read_private_file_snapshot(
        path,
        MAX_PREPARED_RECEIPT_BYTES,
        "prepared Taker acceptance receipt",
    )
    .map_err(|_| TakerServiceStartupError::InitiationUnavailable)?;
    Ok(PreparedReceiptBindingV1 {
        sha256: Sha256::digest(snapshot.bytes()).into(),
        identity: snapshot.identity(),
    })
}

fn read_immutable_binding(
    configured: &ImmutablePrivateFileV1,
    purpose: &str,
) -> Result<TakerPrivateFileBindingV1, TakerServiceStartupError> {
    read_immutable_snapshot_binding(configured, purpose).map(|(binding, _snapshot)| binding)
}

fn read_immutable_snapshot_binding(
    configured: &ImmutablePrivateFileV1,
    purpose: &str,
) -> Result<(TakerPrivateFileBindingV1, PrivateFileSnapshot), TakerServiceStartupError> {
    let snapshot = read_prepared_snapshot(&configured.path, MAX_PREPARED_INPUT_BYTES, purpose)?;
    if Sha256::digest(snapshot.bytes()).as_slice() != configured.sha256 {
        return Err(TakerServiceStartupError::InvalidConfiguration);
    }
    let identity = snapshot.identity();
    let binding = TakerPrivateFileBindingV1::immutable(
        configured.path.clone(),
        configured.sha256,
        identity.device(),
        identity.inode(),
    )
    .map_err(|_| TakerServiceStartupError::InvalidConfiguration)?;
    Ok((binding, snapshot))
}

fn read_secret_binding(
    configured: &SecretPrivateFileV1,
    purpose: &str,
) -> Result<(TakerPrivateFileBindingV1, Zeroizing<[u8; 32]>), TakerServiceStartupError> {
    let snapshot = read_prepared_snapshot(&configured.path, MAX_SIGNING_KEY_BYTES, purpose)?;
    let identity = snapshot.identity();
    if snapshot.bytes().len() != 32 || SecretKey::from_slice(snapshot.bytes()).is_err() {
        return Err(TakerServiceStartupError::InvalidConfiguration);
    }

    let mut key_bytes = Zeroizing::new([0_u8; 32]);
    key_bytes.copy_from_slice(snapshot.bytes());
    let binding = TakerPrivateFileBindingV1::secret(
        configured.path.clone(),
        identity.device(),
        identity.inode(),
    )
    .map_err(|_| TakerServiceStartupError::InvalidConfiguration)?;
    Ok((binding, key_bytes))
}

fn read_prepared_snapshot(
    path: &Path,
    maximum_bytes: u64,
    purpose: &str,
) -> Result<PrivateFileSnapshot, TakerServiceStartupError> {
    read_private_file_snapshot(path, maximum_bytes, purpose)
        .map_err(|_| TakerServiceStartupError::InitiationUnavailable)
}

fn deserialize_sha256<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;

    let encoded = Box::<str>::deserialize(deserializer)?;
    if encoded.len() != 64
        || encoded
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
    {
        return Err(D::Error::custom("invalid SHA-256"));
    }
    let mut digest = [0_u8; 32];
    hex::decode_to_slice(encoded.as_bytes(), &mut digest)
        .map_err(|_| D::Error::custom("invalid SHA-256"))?;
    Ok(digest)
}

/// Validates the prepared-ZEC catalog against the already-checked Delivery sources.
pub(super) fn validate_initiation(
    initiation: &InitiationConfigurationV1,
    source_ids: &BTreeSet<&str>,
    chat_socket_configured: bool,
) -> Result<(), TakerServiceStartupError> {
    let entry_count = initiation.entries().count();
    if entry_count > MAX_PREPARED_INITIATIONS
        || !validate_normalized_absolute(&initiation.registry_database)
        || (entry_count > 0 && !chat_socket_configured)
    {
        return Err(TakerServiceStartupError::InvalidConfiguration);
    }

    let mut swaps = BTreeSet::new();
    let mut offers = BTreeSet::new();
    let mut reservations = BTreeSet::new();
    let mut paths = BTreeSet::from([initiation.registry_database.as_path()]);
    for (_, prepared) in initiation.entries() {
        if !source_ids.contains(prepared.source_id.as_ref())
            || !swaps.insert(prepared.swap_id.as_str())
            || !offers.insert(prepared.offer_id.as_str())
            || !reservations.insert(prepared.reservation_id.as_str())
            || prepared.foreign_units == 0
            || prepared.lez_units == 0
        {
            return Err(TakerServiceStartupError::InvalidConfiguration);
        }
        for path in [
            &prepared.signed_envelope.path,
            &prepared.unsigned_draft.path,
            &prepared.signing_key.path,
            &prepared.source_config.path,
            &prepared.agreement_output,
            &prepared.actor_root,
            &prepared.receipt_output,
        ] {
            if !validate_normalized_absolute(path) || !paths.insert(path.as_path()) {
                return Err(TakerServiceStartupError::InvalidConfiguration);
            }
        }
    }
    Ok(())
}
