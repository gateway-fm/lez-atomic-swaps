use std::{
    fs::{DirBuilder, OpenOptions},
    io::{Cursor, Write as _},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, Result, bail, ensure};
use lez_bridge_protocol::{DiscoveryWindow, Hex32, RunId, RuntimeDescriptor};
use lez_swap_core::{Participant, SwapDirection, SwapId};
use lez_zebra_node_adapter::{
    HttpZebraRpc, HttpZebraRpcConfig, ZebraChainIdentity, ZebraRpc, ZebraRpcChain,
};
use lez_zec_swap_sdk::{
    Bip199Contract, ExpectedBip199Output, LezAssetV1, LezChainIdentityV1, LezEnvironmentV1,
    NegotiationTranscriptV1, PUBLIC_TESTNET_FASTEST_BLOCK_MILLIS,
    PUBLIC_TESTNET_SLOWEST_BLOCK_MILLIS, ZcashFundingInputSetV1, ZcashFundingInputV1,
    ZcashTransparentDestinationV1, ZecAgreementBodyV1, ZecAgreementV1, ZecLezTermsV1,
    ZecParticipantIdentityV1, ZecParticipantsV1, ZecProfileId, ZecProfileRecordV1, ZecRefundPlanV1,
    ZecRefundProfile, ZecSwapBinding, ZecSwapBindingRecordV1, ZecTransactionPolicyV1,
    derive_lez_metadata_account_v1, derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use url::Url;
use zcash_primitives::{block::BlockHash, transaction::Transaction};
use zcash_protocol::{
    TxId,
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{OutPoint, TxOut},
};
use zeroize::Zeroizing;

use crate::config::{DeterministicLocalV0_2ActorConfigInput, ProvisionedZebraRoute};
use crate::secure_file::{FilePrivacy, read_bounded_identified};
use crate::{ActorRole, CandidateOutpoint};

// Both keys' signing in one step, for the test that the two halves of a
// take-time agreement compose exactly that record.
#[cfg(test)]
use lez_swap_core::UnixSeconds;
#[cfg(test)]
use lez_zec_swap_sdk::{ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZecAgreementRecordV1};
#[cfg(test)]
use secp256k1::Message;

// Retained v0.2.0 fixture transfer program, kept for the record:
// `FrexXMbyY6iZjwUo8DV3jfB8donj8H4kLRHT7xswCfJg`. Newer chains differ.
/// The claim secret of one provisioned swap: 32 bytes from the OS RNG, owner-
/// private like the keys beside it (stored only in the Taker's bundle, in both
/// directions; the agreement carries its SHA-256). Never derived from public inputs such as
/// the swap id, which the agreement, the offers and the evidence all show.
pub(crate) fn fresh_claim_preimage() -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let mut preimage = Zeroizing::new([0_u8; 32]);
    getrandom::fill(&mut preimage[..]).map_err(|_| anyhow::anyhow!("claim preimage RNG"))?;
    Ok(preimage)
}
#[cfg(feature = "test-payer")]
const ZCASH_PRINCIPAL_ZATOSHIS: u64 = 100_000_000;
pub(crate) const FUNDING_FEE_ZATOSHIS: u64 = 10_000;
pub(crate) const MINIMUM_CHANGE_ZATOSHIS: u64 = 1_000;
const CLAIM_FEE_ZATOSHIS: u64 = 10_000;
const REFUND_FEE_ZATOSHIS: u64 = 10_000;
const COINBASE_MATURITY_CONFIRMATIONS: u32 = 100;
const COUNTERPARTY_SCAN_BLOCKS: u32 = 1_024;
/// Zcash Testnet's genesis block hash as `getblockhash 0` prints it.
const TESTNET_GENESIS_HASH_HEX: &str =
    "05a60a92d99d85997cce3b87616c089f6124d7342af37106edc76126334a2c38";
/// Blocks a Testnet funder-output search may read: ~18 h of Testnet at its
/// 22.7 s average. Each costs a hash, a block and its transactions over a
/// remote or rate-limited route; a search wider than this names a later
/// `zcash_funding_scan_from_height`.
pub(crate) const MAX_TESTNET_FUNDING_SCAN_BLOCKS: u32 = 2_880;

/// The Zebra a corridor is provisioned against, and its credential.
#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum LocalPocZebraRoute {
    /// The deterministic Regtest chain on loopback; `cookie_file` is its
    /// Zebra's owner-private `.cookie` when its JSON-RPC requires one (the
    /// devnet's `zebrad_module`: `<cacheDir>/.cookie`, new at every start).
    DeterministicLocal {
        #[serde(default)]
        cookie_file: Option<PathBuf>,
    },
    /// A self-hosted Zcash Testnet Zebra on loopback; `cookie_file` is its
    /// owner-private `.cookie` (`__cookie__:<password>`).
    SelfHostedCookie { cookie_file: PathBuf },
    /// Tatum's Testnet Zebrad gateway; `api_key_file` holds the owner-private
    /// key sent as `x-api-key`.
    TatumTestnetXApiKey { api_key_file: PathBuf },
}

impl Default for LocalPocZebraRoute {
    fn default() -> Self {
        Self::DeterministicLocal { cookie_file: None }
    }
}

impl LocalPocZebraRoute {
    const fn credential_file(&self) -> Option<&PathBuf> {
        match self {
            Self::DeterministicLocal { cookie_file } => cookie_file.as_ref(),
            Self::SelfHostedCookie { cookie_file } => Some(cookie_file),
            Self::TatumTestnetXApiKey { api_key_file } => Some(api_key_file),
        }
    }

    pub(crate) const fn network(&self) -> NetworkType {
        match self {
            Self::DeterministicLocal { .. } => NetworkType::Regtest,
            Self::SelfHostedCookie { .. } | Self::TatumTestnetXApiKey { .. } => NetworkType::Test,
        }
    }
}

/// Reads a route credential once: owner-private, one line of visible ASCII.
pub(crate) fn read_zebra_credential(
    route: &LocalPocZebraRoute,
) -> Result<Option<Zeroizing<Vec<u8>>>> {
    let Some(path) = route.credential_file() else {
        return Ok(None);
    };
    ensure!(path.is_absolute(), "Zebra credential path must be absolute");
    let (bytes, _) = read_bounded_identified(path, 1_026, FilePrivacy::OwnerPrivate)
        .map_err(|_| anyhow::anyhow!("Zebra credential file is unavailable or unsafe"))?;
    let mut bytes = bytes;
    while bytes
        .last()
        .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
    {
        bytes.pop();
    }
    ensure!(
        !bytes.is_empty() && bytes.iter().all(u8::is_ascii_graphic),
        "Zebra credential file must hold one line of visible ASCII"
    );
    Ok(Some(bytes))
}

/// A bounded Zebra client on `endpoint` over `route`, with its credential.
pub(crate) fn connect_zebra(
    endpoint: &Url,
    route: &LocalPocZebraRoute,
    credential: Option<&[u8]>,
) -> Result<HttpZebraRpc> {
    let config = match (route, credential) {
        (LocalPocZebraRoute::DeterministicLocal { cookie_file: None }, None) => {
            HttpZebraRpcConfig::new(endpoint.as_str())
        }
        (
            LocalPocZebraRoute::DeterministicLocal {
                cookie_file: Some(_),
            }
            | LocalPocZebraRoute::SelfHostedCookie { .. },
            Some(cookie),
        ) => HttpZebraRpcConfig::new(endpoint.as_str())
            .with_cookie_credentials(cookie)
            .context("invalid Zebra cookie")?,
        (LocalPocZebraRoute::TatumTestnetXApiKey { .. }, Some(api_key)) => {
            HttpZebraRpcConfig::public_https(endpoint.as_str())
                .context("the provider route accepts only Tatum's Testnet Zebrad origin")?
                .with_public_api_key(api_key)
                .context("invalid Tatum API key")?
        }
        _ => bail!("Zebra route and credential do not match"),
    };
    HttpZebraRpc::connect(
        &config
            .with_request_timeout(Duration::from_secs(30))
            .with_max_concurrent_requests(1),
    )
    .context("failed to connect bounded Zebra RPC")
}

/// What proves a Zebra is the chain a corridor is provisioned on, and when one
/// of the funder's outputs may fund it.
pub(crate) struct FundingChain {
    pub(crate) identity: ZebraChainIdentity,
    /// Depth a funder output needs: coinbase maturity on the Regtest fixture
    /// (every candidate waits it out, coinbase or not), the profile's Zcash
    /// confirmations on Testnet.
    pub(crate) min_confirmations: u32,
    /// Testnet makes its own blocks: the tip may move during the search.
    pub(crate) tip_may_advance: bool,
}

impl FundingChain {
    pub(crate) fn for_profile(profile: ZecRefundProfile) -> Result<Self> {
        match profile.zcash_network() {
            // The Regtest fixture's branch is its profile's: NU6.2 on the
            // Zebra 5.2 profiles, NU7 on the Zebra 7 ones (same genesis).
            NetworkType::Regtest => Ok(Self {
                identity: match profile.consensus_branch_id() {
                    BranchId::Nu6_2 => ZebraChainIdentity::deterministic_regtest_nu6_2(),
                    BranchId::Nu7 => ZebraChainIdentity::deterministic_regtest_nu7(),
                    _ => bail!("no Regtest fixture runs the profile's consensus branch"),
                },
                min_confirmations: COINBASE_MATURITY_CONFIRMATIONS,
                tip_may_advance: false,
            }),
            NetworkType::Test => {
                let mut genesis = [0_u8; 32];
                hex::decode_to_slice(TESTNET_GENESIS_HASH_HEX, &mut genesis)
                    .expect("the Testnet genesis constant is hex");
                genesis.reverse();
                Ok(Self {
                    identity: ZebraChainIdentity::new(
                        NetworkType::Test,
                        ZebraRpcChain::Test,
                        profile.consensus_branch_id(),
                        BlockHash(genesis),
                    )
                    .map_err(|_| anyhow::anyhow!("invalid Testnet chain identity"))?,
                    min_confirmations: profile.zcash_confirmations(),
                    tip_may_advance: true,
                })
            }
            NetworkType::Main => bail!("no corridor is provisioned on Zcash mainnet"),
        }
    }
}

/// Reads one owner-private Zcash key file: 32 raw bytes, or 64 hex characters
/// with an optional trailing newline, that form a valid secp256k1 secret.
pub(crate) fn read_zcash_key_file(path: &Path, role: &str) -> Result<Zeroizing<[u8; 32]>> {
    ensure!(path.is_absolute(), "{role} Zcash key path must be absolute");
    let (bytes, _) = read_bounded_identified(path, 65, FilePrivacy::OwnerPrivate)
        .map_err(|_| anyhow::anyhow!("{role} Zcash key file is unavailable or unsafe"))?;
    let bytes = Zeroizing::new(bytes);
    let mut key = Zeroizing::new([0_u8; 32]);
    match bytes.len() {
        32 => key.copy_from_slice(bytes.as_slice()),
        64 | 65 => {
            let text = std::str::from_utf8(&bytes[..64])
                .map_err(|_| anyhow::anyhow!("{role} Zcash key file is not hex"))?;
            ensure!(
                bytes.len() == 64 || bytes[64] == b'\n',
                "{role} Zcash key file has trailing content"
            );
            let mut decoded = Zeroizing::new([0_u8; 32]);
            hex::decode_to_slice(text, &mut decoded[..])
                .map_err(|_| anyhow::anyhow!("{role} Zcash key file is not hex"))?;
            key.copy_from_slice(&decoded[..]);
        }
        _ => bail!("{role} Zcash key file must hold 32 bytes or 64 hex characters"),
    }
    SecretKey::from_slice(&key[..])
        .map_err(|_| anyhow::anyhow!("{role} Zcash key is not a valid secret"))?;
    Ok(key)
}

/// A Zcash key's public identity on the deterministic Regtest fixture.
#[derive(Clone, Debug, Serialize)]
pub struct LocalPocZcashIdentity {
    /// Compressed secp256k1 public key, hex.
    pub public_key: String,
    /// HASH160 of the public key, hex: the BIP-199 participant hash.
    pub public_key_hash: String,
    /// The key's Regtest P2PKH address (`tm…`), zebra's `miner_address` form.
    /// Zcash Testnet uses the same prefix: it is also the key's Testnet address.
    pub transparent_address: String,
}

/// Regtest transparent P2PKH address prefix (ZIP-? constant shared with testnet).
const REGTEST_P2PKH_PREFIX: [u8; 2] = [0x1D, 0x25];

pub(crate) fn regtest_p2pkh_address(public_key: &PublicKey) -> Result<String> {
    let TransparentAddress::PublicKeyHash(hash) = lez_zec_swap_sdk::p2pkh_address(public_key)
    else {
        bail!("public key did not derive a P2PKH address");
    };
    let mut payload = Vec::with_capacity(26);
    payload.extend_from_slice(&REGTEST_P2PKH_PREFIX);
    payload.extend_from_slice(&hash);
    let checksum = Sha256::digest(Sha256::digest(&payload));
    payload.extend_from_slice(&checksum[..4]);
    Ok(bs58::encode(payload).into_string())
}

/// Reads an owner-private Zcash key file and reports its public identity.
///
/// # Errors
///
/// Fails for an absent, non-private, malformed or invalid key file.
pub fn zcash_identity_from_key_file(path: &Path) -> Result<LocalPocZcashIdentity> {
    let secret = read_zcash_key_file(path, "Zcash")?;
    let mut secret_key = SecretKey::from_slice(&secret[..])?;
    let public_key = PublicKey::from_secret_key(&Secp256k1::signing_only(), &secret_key);
    secret_key.non_secure_erase();
    Ok(LocalPocZcashIdentity {
        public_key: hex::encode(public_key.serialize()),
        public_key_hash: hex::encode(public_key_hash(&public_key)),
        transparent_address: regtest_p2pkh_address(&public_key)?,
    })
}

/// What one Regtest transfer between the roles' keys produced.
#[derive(Clone, Debug, Serialize)]
#[cfg(feature = "test-payer")]
pub struct LocalPocTransferSummary {
    /// The transfer's transaction id as the RPC displays it.
    pub transaction_id: String,
    /// Value paid to the recipient, zatoshis.
    pub value_zatoshis: u64,
    /// The sender's Regtest address (the spent mature output's owner).
    pub from_address: String,
    /// The recipient's Regtest address.
    pub to_address: String,
    /// Zebra tip height when the output was selected.
    pub zebra_tip_height: u32,
}

/// Moves `value_zatoshis` from the key in `from_key_file` to the key in
/// `to_key_file` with one plain transparent transaction on the deterministic
/// Regtest fixture, spending one mature output of the sender (the first, or
/// the one after `skip` mature outputs) and returning the change. The output
/// becomes a corridor funder candidate for the recipient once it is 100
/// blocks deep. Test lane only: it funds the Taker's key before a
/// `TakerSellsForeign` corridor is provisioned.
///
/// # Errors
///
/// Fails closed for unsafe key files, equal keys, a non-Regtest Zebra, a
/// missing mature output, an unaffordable value, or a rejected broadcast.
#[cfg(feature = "test-payer")]
pub async fn transfer_local_v0_2_regtest(
    zebra_endpoint: &Url,
    zebra_cookie_file: Option<&Path>,
    from_key_file: &Path,
    to_key_file: &Path,
    value_zatoshis: u64,
    skip: u32,
) -> Result<LocalPocTransferSummary> {
    let route = LocalPocZebraRoute::DeterministicLocal {
        cookie_file: zebra_cookie_file.map(Path::to_path_buf),
    };
    let secret = read_zebra_credential(&route)?;
    let rpc = connect_zebra(
        zebra_endpoint,
        &route,
        secret.as_ref().map(|s| s.as_slice()),
    )?;
    transfer_between_keys(
        &rpc,
        // The Regtest fixtures run Zebra 7 with NU7 from height 1.
        &FundingChain::for_profile(ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalV2))?,
        from_key_file,
        to_key_file,
        value_zatoshis,
        skip,
        1,
        // The first mature output that pays this transfer, at least of the
        // devnet's (1 ZEC + fees) shape: Regtest coinbases halve, and an
        // earlier output too small for a 2 ZEC payment was once picked first.
        value_zatoshis
            .max(ZCASH_PRINCIPAL_ZATOSHIS)
            .saturating_add(FUNDING_FEE_ZATOSHIS)
            .saturating_add(MINIMUM_CHANGE_ZATOSHIS),
        100,
    )
    .await
}

/// The credential of a Zcash Testnet route (the self-hosted Zebra's cookie
/// file or the Tatum API key file), owner-private.
#[derive(Clone)]
#[cfg(feature = "test-payer")]
pub enum TestnetZebraCredential {
    /// A self-hosted Testnet Zebra on loopback and its `.cookie` file.
    SelfHostedCookie(PathBuf),
    /// Tatum's Testnet Zebrad gateway and the API key file.
    TatumApiKey(PathBuf),
}

#[cfg(feature = "test-payer")]
impl std::fmt::Debug for TestnetZebraCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::SelfHostedCookie(_) => "SelfHostedCookie(<redacted>)",
            Self::TatumApiKey(_) => "TatumApiKey(<redacted>)",
        })
    }
}

#[cfg(feature = "test-payer")]
impl TestnetZebraCredential {
    fn route(&self) -> LocalPocZebraRoute {
        match self {
            Self::SelfHostedCookie(cookie_file) => LocalPocZebraRoute::SelfHostedCookie {
                cookie_file: cookie_file.clone(),
            },
            Self::TatumApiKey(api_key_file) => LocalPocZebraRoute::TatumTestnetXApiKey {
                api_key_file: api_key_file.clone(),
            },
        }
    }
}

/// [`transfer_local_v0_2_regtest`] on Zcash Testnet under `public-testnet-v2`
/// (NU7): spends an unspent output of the sender at least the value plus fee
/// and change, the profile's Zcash confirmations deep, found from
/// `scan_from_height` (within 2,880 blocks of the tip). The payment becomes
/// the recipient's funder candidate once it is as deep. Broadcasts; it is the
/// one real spend of the owner's TAZ before a Taker-funded corridor.
///
/// # Errors
///
/// As [`transfer_local_v0_2_regtest`], for a Zebra that is not Testnet on NU7,
/// an unsafe credential, or no such output.
#[cfg(feature = "test-payer")]
pub async fn transfer_v0_2_testnet(
    zebra_endpoint: &Url,
    credential: &TestnetZebraCredential,
    from_key_file: &Path,
    to_key_file: &Path,
    value_zatoshis: u64,
    skip: u32,
    scan_from_height: u32,
) -> Result<LocalPocTransferSummary> {
    let route = credential.route();
    let secret = read_zebra_credential(&route)?;
    let rpc = connect_zebra(
        zebra_endpoint,
        &route,
        secret.as_ref().map(|s| s.as_slice()),
    )?;
    let profile = ZecRefundProfile::for_id(ZecProfileId::PublicTestnetV2);
    transfer_between_keys(
        &rpc,
        &FundingChain::for_profile(profile)?,
        from_key_file,
        to_key_file,
        value_zatoshis,
        skip,
        scan_from_height,
        value_zatoshis
            .saturating_add(FUNDING_FEE_ZATOSHIS)
            .saturating_add(MINIMUM_CHANGE_ZATOSHIS),
        profile.expiry_delta_blocks(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "test-payer")]
async fn transfer_between_keys(
    rpc: &HttpZebraRpc,
    chain: &FundingChain,
    from_key_file: &Path,
    to_key_file: &Path,
    value_zatoshis: u64,
    skip: u32,
    scan_from_height: u32,
    min_value: u64,
    expiry_delta: u32,
) -> Result<LocalPocTransferSummary> {
    let from = read_zcash_key_file(from_key_file, "sender")?;
    let to = read_zcash_key_file(to_key_file, "recipient")?;
    ensure!(*from != *to, "sender and recipient Zcash keys must differ");
    let mut from_secret = SecretKey::from_slice(&from[..])?;
    let mut to_secret = SecretKey::from_slice(&to[..])?;
    let secp = Secp256k1::signing_only();
    let from_public = PublicKey::from_secret_key(&secp, &from_secret);
    let to_public = PublicKey::from_secret_key(&secp, &to_secret);
    to_secret.non_secure_erase();
    let (candidate, tip_height, _) = select_funding_candidate(
        rpc,
        chain,
        &from_public,
        skip,
        &[],
        scan_from_height,
        min_value,
    )
    .await?;
    let value = Zatoshis::from_u64(value_zatoshis).context("invalid transfer value")?;
    let request = TransparentFundingRequest::new(
        vec![TransparentUtxo::new(
            candidate.outpoint.clone(),
            candidate.output.clone(),
        )],
        from_public,
        value,
        Zatoshis::from_u64(FUNDING_FEE_ZATOSHIS)?,
        Zatoshis::from_u64(MINIMUM_CHANGE_ZATOSHIS)?,
        BlockHeight::from_u32(tip_height.saturating_add(expiry_delta)),
        chain.identity.consensus_branch_id(),
    )?;
    let transaction = build_transparent_transfer_transaction(&request, &to_public, &from_secret)?;
    from_secret.non_secure_erase();
    let mut bytes = Vec::new();
    transaction
        .write(&mut bytes)
        .context("transfer transaction serialization failed")?;
    let transaction_id = rpc
        .send_raw_transaction(&bytes)
        .await
        .context("Zebra rejected the transfer")?;
    Ok(LocalPocTransferSummary {
        transaction_id: hex::encode(txid_display_hex(transaction_id).as_bytes()),
        value_zatoshis,
        from_address: regtest_p2pkh_address(&from_public)?,
        to_address: regtest_p2pkh_address(&to_public)?,
        zebra_tip_height: tip_height,
    })
}

/// The named profiles a local corridor may be provisioned under.
///
/// `TakerSellsLez` is provisioned only under the Taker-reveals-first profiles
/// (`deterministic-local-v3`, `deterministic-local-devnet-v7`/`-v8`,
/// `public-testnet-v3`); `TakerSellsForeign` keeps the profiles it has always
/// signed under, and `public-testnet-v3` (its window sized for
/// `TakerSellsLez`) is refused for it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LocalPocProfile {
    /// `deterministic-local-v1`: the isolated test lane (60 s LEZ refund delay).
    DeterministicLocalV1,
    /// `deterministic-local-devnet-v1`: the same lane with a 30 min LEZ refund
    /// delay, for a chain that finalises in minutes.
    DeterministicLocalDevnetV1,
    /// `deterministic-local-devnet-v2`: the devnet lane with the eight-block
    /// Zcash refund window two swaps taken together need.
    DeterministicLocalDevnetV2,
    /// `deterministic-local-devnet-v3`: V2 with a 12 min LEZ refund delay, so
    /// a refund fits the devnet chain's ~27 min of finality.
    DeterministicLocalDevnetV3,
    /// `deterministic-local-devnet-v4`: V3 with a 4 min LEZ refund delay, for
    /// the devnet's refund journeys (they never claim).
    DeterministicLocalDevnetV4,
    /// `deterministic-local-v2`: the isolated test lane on Zebra 7's NU7.
    DeterministicLocalV2,
    /// `deterministic-local-devnet-v5`: V3 on NU7, the devnet's profile for
    /// swaps that claim once its Zebra runs Zebra 7.
    DeterministicLocalDevnetV5,
    /// `deterministic-local-devnet-v6`: V4 on NU7, the devnet's refund
    /// journeys' profile.
    DeterministicLocalDevnetV6,
    /// `public-testnet-v2`: Zcash Testnet on NU7 and the official LEZ v0.2
    /// testnet, funded from an ordinary transparent output of the funder's
    /// key (scripts/public-zec-testnet.sh).
    PublicTestnetV2,
    /// `deterministic-local-v3`: the isolated test lane's Taker-reveals-first
    /// profile (NU7), for `TakerSellsLez`.
    DeterministicLocalV3,
    /// `deterministic-local-devnet-v7`: the devnet's Taker-reveals-first
    /// profile for swaps that claim (NU7, 12 min LEZ refund delay, 16-block
    /// Zcash refund window).
    DeterministicLocalDevnetV7,
    /// `deterministic-local-devnet-v8`: V7 with an 8 min LEZ refund delay, for
    /// the devnet's refund journeys (they never claim).
    DeterministicLocalDevnetV8,
    /// `public-testnet-v3`: Zcash Testnet on NU7 and the official LEZ v0.2
    /// testnet, Taker reveals first, for `TakerSellsLez`.
    PublicTestnetV3,
}

impl LocalPocProfile {
    pub(crate) const fn id(self) -> ZecProfileId {
        match self {
            Self::DeterministicLocalV1 => ZecProfileId::DeterministicLocalV1,
            Self::DeterministicLocalDevnetV1 => ZecProfileId::DeterministicLocalDevnetV1,
            Self::DeterministicLocalDevnetV2 => ZecProfileId::DeterministicLocalDevnetV2,
            Self::DeterministicLocalDevnetV3 => ZecProfileId::DeterministicLocalDevnetV3,
            Self::DeterministicLocalDevnetV4 => ZecProfileId::DeterministicLocalDevnetV4,
            Self::DeterministicLocalV2 => ZecProfileId::DeterministicLocalV2,
            Self::DeterministicLocalDevnetV5 => ZecProfileId::DeterministicLocalDevnetV5,
            Self::DeterministicLocalDevnetV6 => ZecProfileId::DeterministicLocalDevnetV6,
            Self::PublicTestnetV2 => ZecProfileId::PublicTestnetV2,
            Self::DeterministicLocalV3 => ZecProfileId::DeterministicLocalV3,
            Self::DeterministicLocalDevnetV7 => ZecProfileId::DeterministicLocalDevnetV7,
            Self::DeterministicLocalDevnetV8 => ZecProfileId::DeterministicLocalDevnetV8,
            Self::PublicTestnetV3 => ZecProfileId::PublicTestnetV3,
        }
    }

    /// This profile's twin sized for `direction`, or itself when it admits
    /// `direction` or has no twin. Each profile admits one direction (ADR
    /// 0216); a take-time role names one profile and takes the other
    /// direction under its twin:
    /// `deterministic-local-devnet-v5`/`-v7`, `-v6`/`-v8`,
    /// `public-testnet-v2`/`-v3` and `deterministic-local-v2`/`-v3`.
    pub(crate) const fn for_direction(self, direction: SwapDirection) -> Self {
        match (direction, self) {
            (SwapDirection::TakerSellsLez, Self::DeterministicLocalDevnetV5) => {
                Self::DeterministicLocalDevnetV7
            }
            (SwapDirection::TakerSellsLez, Self::DeterministicLocalDevnetV6) => {
                Self::DeterministicLocalDevnetV8
            }
            (SwapDirection::TakerSellsLez, Self::PublicTestnetV2) => Self::PublicTestnetV3,
            (SwapDirection::TakerSellsLez, Self::DeterministicLocalV2) => {
                Self::DeterministicLocalV3
            }
            (SwapDirection::TakerSellsForeign, Self::DeterministicLocalDevnetV7) => {
                Self::DeterministicLocalDevnetV5
            }
            (SwapDirection::TakerSellsForeign, Self::DeterministicLocalDevnetV8) => {
                Self::DeterministicLocalDevnetV6
            }
            (SwapDirection::TakerSellsForeign, Self::PublicTestnetV3) => Self::PublicTestnetV2,
            (SwapDirection::TakerSellsForeign, Self::DeterministicLocalV3) => {
                Self::DeterministicLocalV2
            }
            (_, other) => other,
        }
    }
}

pub(crate) struct SelectedCandidate {
    pub(crate) outpoint: OutPoint,
    pub(crate) output: TxOut,
    pub(crate) rpc_transaction_id: Hex32,
    /// Height of the block that holds it.
    pub(crate) height: u32,
}

pub(crate) fn validate_role_authority(
    material: &crate::ActivateMaterial,
    agreement: &ZecAgreementV1,
    participant: Participant,
    configured_as_funder: bool,
) -> Result<()> {
    let mut secret =
        SecretKey::from_slice(material.zcash_secret_key()).context("actor Zcash key is invalid")?;
    let public = PublicKey::from_secret_key(&Secp256k1::signing_only(), &secret);
    secret.non_secure_erase();
    ensure!(
        public == *agreement.zcash_key(participant),
        "actor Zcash key does not match the final agreement"
    );
    // The Zcash funder receives the escrowed LEZ (`lez_claimant`).
    let expected_funder = agreement.lez_claimant() == participant;
    ensure!(
        configured_as_funder == expected_funder,
        "actor funder role does not match the final agreement"
    );
    // The preimage is the Taker's alone, in both directions: it claims the
    // Maker-funded leg first; the Maker reads the preimage from that claim.
    let expected_preimage_owner = agreement.revealing_claimant() == participant;
    match material.claim_preimage() {
        Some(preimage) if expected_preimage_owner => {
            let digest: [u8; 32] = Sha256::digest(preimage.expose_secret()).into();
            ensure!(
                digest == *agreement.secret_digest(),
                "actor preimage does not match the final agreement"
            );
        }
        None if !expected_preimage_owner => {}
        _ => bail!("actor preimage ownership does not match the final agreement"),
    }
    Ok(())
}

pub(crate) struct MaterialPaths {
    pub(crate) state_root: PathBuf,
    pub(crate) config: PathBuf,
    pub(crate) role_state_db: PathBuf,
    pub(crate) claim_recovery_key: PathBuf,
    pub(crate) claim_preimage: PathBuf,
    pub(crate) zcash_key: PathBuf,
    pub(crate) bridge_journal_db: PathBuf,
    pub(crate) capability: PathBuf,
    pub(crate) zebra_credential: PathBuf,
}

pub(crate) fn material_paths(root: &Path, state_root: &Path) -> MaterialPaths {
    MaterialPaths {
        state_root: state_root.to_path_buf(),
        config: root.join("actor-config.json"),
        role_state_db: state_root.join("actor.sqlite3"),
        claim_recovery_key: root.join("claim-recovery.key"),
        claim_preimage: root.join("claim-preimage.key"),
        zcash_key: root.join("zcash.key"),
        bridge_journal_db: state_root.join("bridge.sqlite3"),
        capability: root.join("sidecar.capability"),
        zebra_credential: root.join("zebra.credential"),
    }
}

/// What one role's actor config takes from its corridor besides the
/// agreement: the run, the role's sidecar, the Zebra route and the profile.
pub(crate) struct RoleCorridor<'a> {
    pub(crate) run_id: &'a RunId,
    pub(crate) bridge_endpoint: &'a Url,
    pub(crate) zebra_endpoint: &'a Url,
    pub(crate) zebra_route: &'a LocalPocZebraRoute,
    pub(crate) profile: ZecProfileId,
    /// A self-hosted Testnet Zebra's cookie was copied beside the role's keys;
    /// otherwise the actor names the cookie where the route names it.
    pub(crate) cookie_copied: bool,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn actor_config_input(
    corridor: &RoleCorridor<'_>,
    swap_id: &SwapId,
    role: ActorRole,
    agreement_file: &Path,
    agreement_sha256: Hex32,
    paths: &MaterialPaths,
    claim_preimage_file: Option<PathBuf>,
    runtime: RuntimeDescriptor,
    zebra_genesis_hash: Hex32,
    discovery: DiscoveryWindow,
    candidates: Vec<CandidateOutpoint>,
) -> DeterministicLocalV0_2ActorConfigInput {
    let role_name = match role {
        ActorRole::Maker => "maker",
        ActorRole::Taker => "taker",
    };
    let profile = ZecRefundProfile::for_id(corridor.profile);
    DeterministicLocalV0_2ActorConfigInput {
        role,
        run_id: corridor.run_id.clone(),
        swap_id: swap_id.clone(),
        signed_agreement_file: agreement_file.to_path_buf(),
        signed_agreement_sha256: agreement_sha256,
        role_state_db: paths.role_state_db.clone(),
        claim_recovery_key_id: format!("{}-{role_name}-claim", corridor.run_id.as_str()).into(),
        claim_recovery_key_file: paths.claim_recovery_key.clone(),
        claim_preimage_file,
        zcash_key_file: paths.zcash_key.clone(),
        bridge_endpoint: corridor.bridge_endpoint.clone(),
        bridge_journal_db: paths.bridge_journal_db.clone(),
        bridge_capability_file: paths.capability.clone(),
        bridge_runtime: runtime,
        zebra_endpoint: corridor.zebra_endpoint.clone(),
        zebra_route: match corridor.zebra_route {
            LocalPocZebraRoute::DeterministicLocal { cookie_file } => {
                ProvisionedZebraRoute::DeterministicLocal {
                    cookie_file: cookie_file.clone(),
                }
            }
            LocalPocZebraRoute::SelfHostedCookie { cookie_file } => {
                ProvisionedZebraRoute::SelfHostedTestnetCookie {
                    cookie_file: if corridor.cookie_copied {
                        paths.zebra_credential.clone()
                    } else {
                        cookie_file.clone()
                    },
                }
            }
            LocalPocZebraRoute::TatumTestnetXApiKey { .. } => {
                ProvisionedZebraRoute::TatumTestnetXApiKey {
                    api_key_file: paths.zebra_credential.clone(),
                }
            }
        },
        zebra_consensus_branch_id: profile.consensus_branch_id(),
        zebra_genesis_hash,
        // Remote-funding discovery reads the profile's whole refund window
        // (zebra-node-adapter first_lock.rs); the devnet's 1,024 cover it.
        counterparty_scan_blocks: COUNTERPARTY_SCAN_BLOCKS
            .max(profile.zcash_refund_blocks().saturating_add(1)),
        lez_discovery_window: discovery,
        zcash_funding_outpoints: candidates,
    }
}

/// Finds the funder's spendable output: the first (after `skip`) output paying
/// `funding_key` at least `min_value`, `chain.min_confirmations` deep and
/// unspent, reading blocks from `scan_from_height` up. Returns it with the tip
/// height it was selected at and the chain's genesis.
#[allow(clippy::too_many_lines)]
pub(crate) async fn select_funding_candidate<R>(
    rpc: &R,
    chain: &FundingChain,
    funding_key: &PublicKey,
    skip: u32,
    exclude: &[OutPoint],
    scan_from_height: u32,
    min_value: u64,
) -> Result<(SelectedCandidate, u32, BlockHash)>
where
    R: ZebraRpc,
    R::Error: std::error::Error + Send + Sync + 'static,
{
    let identity = chain.identity;
    let before = rpc
        .chain_info()
        .await
        .context("Zebra identity unavailable")?;
    let network_name = match identity.network() {
        NetworkType::Regtest => "the deterministic Regtest fixture on the profile's branch",
        NetworkType::Test | NetworkType::Main => "Zcash Testnet on the profile's branch",
    };
    ensure!(
        before.rpc_chain() == identity.rpc_chain()
            && before.consensus_branch_id() == identity.consensus_branch_id(),
        "Zebra is not {network_name}"
    );
    let genesis = rpc
        .block_hash(BlockHeight::from_u32(0))
        .await
        .context("Zebra genesis unavailable")?;
    ensure!(
        identity.genesis_hash() == genesis,
        "Zebra genesis does not match {network_name}"
    );

    let owner_script: Script = lez_zec_swap_sdk::p2pkh_address(funding_key).script().into();
    let tip_height = u32::from(before.tip_height());
    if chain.tip_may_advance {
        ensure!(
            scan_from_height >= 1
                && tip_height.saturating_sub(scan_from_height) < MAX_TESTNET_FUNDING_SCAN_BLOCKS,
            "a Testnet funder search names zcash_funding_scan_from_height within \
             {MAX_TESTNET_FUNDING_SCAN_BLOCKS} blocks of the tip ({tip_height})"
        );
    }
    let mut selected = None;
    let mut passed_over = 0u32;
    for height in scan_from_height.max(1)..=tip_height {
        if tip_height.saturating_sub(height).saturating_add(1) < chain.min_confirmations {
            break;
        }
        let block_hash = rpc
            .block_hash(BlockHeight::from_u32(height))
            .await
            .context("Zebra canonical block hash unavailable")?;
        let block = rpc
            .canonical_block(block_hash)
            .await
            .context("Zebra canonical block unavailable")?;
        ensure!(
            block.block_height() == BlockHeight::from_u32(height),
            "Zebra returned a mismatched block height"
        );
        for transaction_id in block.transaction_ids() {
            let Some(raw) = rpc
                .block_transaction(*transaction_id, block_hash)
                .await
                .context("Zebra block transaction unavailable")?
            else {
                bail!("Zebra omitted a transaction named by its canonical block");
            };
            let mut cursor = Cursor::new(raw.as_slice());
            // A v5 or later transaction names its own branch; the argument
            // only decides how a v4 or older one is read.
            let transaction = Transaction::read(&mut cursor, identity.consensus_branch_id())
                .context("Zebra returned a malformed transaction")?;
            ensure!(
                usize::try_from(cursor.position()).ok() == Some(raw.len())
                    && transaction.txid() == *transaction_id,
                "Zebra returned noncanonical transaction bytes"
            );
            let Some(bundle) = transaction.transparent_bundle() else {
                continue;
            };
            // Any transparent output paying the funder's key is a candidate:
            // the coinbase zebra mines to the Maker's key, the plain payment
            // the devnet provisioner makes to the Taker's key when the Taker
            // funds Zcash, or the owner's payment on Testnet. All wait out
            // the chain's depth below.
            for (index, output) in bundle.vout.iter().enumerate() {
                if output.script_pubkey() != &owner_script || u64::from(output.value()) < min_value
                {
                    continue;
                }
                let output_index = u32::try_from(index).context("Zcash vout index overflow")?;
                let outpoint = OutPoint::new(*transaction_id.as_ref(), output_index);
                // Committed to another live reservation of this funder.
                if exclude.contains(&outpoint) {
                    continue;
                }
                let Some(unspent) = rpc
                    .unspent_output(&outpoint)
                    .await
                    .context("Zebra exact UTXO query failed")?
                else {
                    continue;
                };
                ensure!(
                    chain.tip_may_advance || unspent.best_block() == before.tip_hash(),
                    "Zebra answered the UTXO query against another tip"
                );
                if unspent.confirmations() < chain.min_confirmations {
                    continue;
                }
                ensure!(
                    unspent.output() == output,
                    "Zebra UTXO facts differ from the canonical transaction"
                );
                if passed_over < skip {
                    passed_over += 1;
                    continue;
                }
                selected = Some(SelectedCandidate {
                    outpoint,
                    output: output.clone(),
                    rpc_transaction_id: txid_display_hex(*transaction_id),
                    height,
                });
                break;
            }
            if selected.is_some() {
                break;
            }
        }
        if selected.is_some() {
            break;
        }
    }
    let after = rpc
        .chain_info()
        .await
        .context("Zebra closing identity sample unavailable")?;
    if chain.tip_may_advance {
        // Testnet moved on meanwhile; it must still be the same chain and branch.
        ensure!(
            after.rpc_chain() == before.rpc_chain()
                && after.consensus_branch_id() == before.consensus_branch_id()
                && after.tip_height() >= before.tip_height(),
            "Zebra changed chain, branch or rewound during candidate selection"
        );
    } else {
        ensure!(
            before == after,
            "Zebra tip changed during candidate selection"
        );
    }
    // The refusal names the funder's address, so its owner knows what to pay.
    let address = regtest_p2pkh_address(funding_key).unwrap_or_default();
    let candidate = selected.with_context(|| match identity.network() {
        NetworkType::Regtest => format!(
            "no mature deterministic Zebra funder output for {address} is available after \
             passing over {skip}"
        ),
        NetworkType::Test | NetworkType::Main => format!(
            "no unspent funder output for {address} of at least {min_value} zatoshis, {} blocks \
             deep, from height {scan_from_height} after passing over {skip}",
            chain.min_confirmations
        ),
    })?;
    Ok((candidate, tip_height, genesis))
}

/// The LEZ chain a Zcash agreement settles against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LezChainFacts {
    pub(crate) channel_id: [u8; 32],
    pub(crate) genesis_block_hash: [u8; 32],
    pub(crate) escrow_program_id: Hex32,
    pub(crate) authenticated_transfer_program_id: [u8; 32],
}

/// Every public fact one Zcash agreement body is composed from. The two
/// roles of a take-time reservation (`zec_reserve_v1`) compose the same body
/// from the same facts, so each role can recompute what the other sent.
#[derive(Clone, Debug)]
pub(crate) struct AgreementFacts {
    pub(crate) profile: ZecProfileId,
    pub(crate) direction: SwapDirection,
    pub(crate) swap_id: SwapId,
    pub(crate) maker_lez_account: [u8; 32],
    pub(crate) taker_lez_account: [u8; 32],
    pub(crate) maker_zcash_key: PublicKey,
    pub(crate) taker_zcash_key: PublicKey,
    /// SHA-256 of the Taker's claim preimage (the Taker holds it in both
    /// directions, ADR 0216).
    pub(crate) secret_digest: [u8; 32],
    pub(crate) lez: LezChainFacts,
    pub(crate) lez_amount: u128,
    pub(crate) zcash_principal: u64,
    /// The Zcash funder's one committed funding input.
    pub(crate) funding_input: ZcashFundingInputV1,
    /// Zebra tip the anchor is fixed after (the anchor is the next block).
    pub(crate) zebra_tip_height: u32,
    /// The LEZ funding anchor, Unix seconds.
    pub(crate) now: u64,
    pub(crate) transcript: NegotiationTranscriptV1,
}

/// The role that funds Zcash in `direction`.
pub(crate) const fn zcash_funder(direction: SwapDirection) -> ActorRole {
    match direction {
        SwapDirection::TakerSellsForeign => ActorRole::Taker,
        SwapDirection::TakerSellsLez => ActorRole::Maker,
    }
}

/// Composes the unsigned agreement body and its Zcash refund height: the
/// anchor is the block after `zebra_tip_height`, the deadlines follow the
/// profile from `now`.
#[allow(clippy::too_many_lines)]
pub(crate) fn compose_agreement_body(facts: &AgreementFacts) -> Result<(ZecAgreementBodyV1, u32)> {
    let now = facts.now;
    let profile = ZecRefundProfile::for_id(facts.profile);
    let zcash_anchor = facts
        .zebra_tip_height
        .checked_add(1)
        .context("Zebra funding anchor overflow")?;
    let zcash_refund_height = zcash_anchor
        .checked_add(profile.zcash_refund_blocks())
        .context("Zebra refund height overflow")?;
    let lez_refund_seconds = now
        .checked_add(profile.lez_refund_delay().value())
        .context("LEZ refund time overflow")?;
    let (earlier_refund_latest_ms, later_refund_earliest) =
        refund_plan(profile, facts.direction, now, lez_refund_seconds)?;
    let maker_hash = public_key_hash(&facts.maker_zcash_key);
    let taker_hash = public_key_hash(&facts.taker_zcash_key);
    let (zcash_funder_hash, zcash_claimant_hash) = match zcash_funder(facts.direction) {
        ActorRole::Maker => (maker_hash, taker_hash),
        ActorRole::Taker => (taker_hash, maker_hash),
    };
    // The profile decides the claim branch: the Taker-reveals-first profiles check
    // that the preimage is 32 bytes, the length the LEZ escrow claims with.
    let contract = Bip199Contract::for_profile(
        facts.profile,
        zcash_refund_height,
        zcash_funder_hash,
        facts.secret_digest,
        zcash_claimant_hash,
    );
    let expected_output = ExpectedBip199Output::new(
        profile.zcash_network(),
        profile.consensus_branch_id(),
        Zatoshis::from_u64(facts.zcash_principal)?,
        contract,
    );
    let binding = ZecSwapBinding::new(facts.profile, expected_output)?;
    let funding_inputs = ZcashFundingInputSetV1::new(vec![facts.funding_input.clone()])?;

    let escrow_program = program_id_words(facts.lez.escrow_program_id);
    let authenticated_transfer_program = program_id_words(Hex32::from_bytes(
        facts.lez.authenticated_transfer_program_id,
    ));
    let onchain_swap_id = derive_lez_swap_id_v1(facts.swap_id.as_str().as_bytes());
    let metadata_account = derive_lez_metadata_account_v1(&escrow_program, &onchain_swap_id);
    let custody_account = derive_lez_native_custody_account_v1(&escrow_program, &onchain_swap_id);
    let body = ZecAgreementBodyV1::new(
        facts.swap_id.as_str(),
        facts.direction,
        ZecProfileRecordV1::from(facts.profile),
        ZecParticipantsV1::new(
            ZecParticipantIdentityV1::new(
                facts.maker_lez_account,
                facts.maker_zcash_key.serialize(),
            ),
            ZecParticipantIdentityV1::new(
                facts.taker_lez_account,
                facts.taker_zcash_key.serialize(),
            ),
        ),
        facts.secret_digest,
        ZecLezTermsV1::new(
            LezChainIdentityV1::new(
                match profile.zcash_network() {
                    NetworkType::Regtest => LezEnvironmentV1::DeterministicLocalV0_2,
                    // A public profile pairs Zcash Testnet with the official
                    // LEZ v0.2 testnet (zec-swap-sdk agreement_v1.rs).
                    NetworkType::Test | NetworkType::Main => LezEnvironmentV1::PublicTestnetV0_2,
                },
                facts.lez.channel_id,
                facts.lez.genesis_block_hash,
            ),
            escrow_program,
            LezAssetV1::Native {
                authenticated_transfer_program_id: authenticated_transfer_program,
            },
            facts.lez_amount,
            metadata_account,
            custody_account,
        ),
        ZecSwapBindingRecordV1::from_binding(&binding),
        ZecTransactionPolicyV1::new(
            funding_inputs.commitment(),
            ZcashTransparentDestinationV1::p2pkh(zcash_funder_hash),
            FUNDING_FEE_ZATOSHIS,
            MINIMUM_CHANGE_ZATOSHIS,
            ZcashTransparentDestinationV1::p2pkh(zcash_claimant_hash),
            CLAIM_FEE_ZATOSHIS,
            ZcashTransparentDestinationV1::p2pkh(zcash_funder_hash),
            REFUND_FEE_ZATOSHIS,
            profile.expiry_delta_blocks(),
        ),
        ZecRefundPlanV1::new(
            now,
            zcash_anchor,
            earlier_refund_latest_ms,
            later_refund_earliest,
        ),
        facts.transcript,
    );
    Ok((body, zcash_refund_height))
}

/// Signs one composed body with both roles' keys and validates the record at
/// `now`: the one-step countersignature the two halves of a take-time
/// agreement must equal.
#[cfg(test)]
pub(crate) fn sign_agreement_body(
    body: ZecAgreementBodyV1,
    maker_secret: &SecretKey,
    taker_secret: &SecretKey,
    now: u64,
) -> Result<Zeroizing<Vec<u8>>> {
    let commitment = body.commitment();
    let maker_signature = sign_agreement(commitment, maker_secret);
    let taker_signature = sign_agreement(commitment, taker_secret);
    let record = ZecAgreementRecordV1::from_parts(
        ZEC_CONCRETE_AGREEMENT_SCHEMA_V2,
        body,
        commitment,
        maker_signature,
        taker_signature,
    );
    let agreement = ZecAgreementV1::validate_at(record, UnixSeconds::new(now))?;
    Ok(Zeroizing::new(agreement.encode_wire()?))
}

/// The signed refund plan's two bounds, `(earlier_refund_latest_ms,
/// later_refund_earliest_unix_seconds)`, for an agreement anchored at `now`
/// whose LEZ refund opens at `lez_refund_seconds`.
///
/// `TakerSellsForeign`: the Maker's LEZ refund is the earlier one; its latest
/// time is the LEZ deadline, and the Taker's Zcash refund opens no sooner than
/// that plus the margin (Regtest: as declared; Testnet: its blocks at the
/// fastest pace).
///
/// `TakerSellsLez`: the Maker's Zcash refund is the earlier one and the
/// Taker's LEZ refund the later one, which opens exactly at the LEZ deadline.
/// The Zcash refund's latest time is declared as the LEZ deadline minus the
/// margin on Regtest (the harness mines every block) and as the anchor plus
/// its blocks and one at Testnet's slowest pace on Testnet, which must leave
/// the margin before the LEZ deadline.
fn refund_plan(
    profile: ZecRefundProfile,
    direction: SwapDirection,
    now: u64,
    lez_refund_seconds: u64,
) -> Result<(u64, u64)> {
    let margin = profile.required_margin().value();
    let lez_refund_ms = lez_refund_seconds
        .checked_mul(1_000)
        .context("LEZ refund millisecond overflow")?;
    match direction {
        SwapDirection::TakerSellsForeign => {
            let later_refund_earliest = match profile.zcash_network() {
                // The fixture's harness mines every block: the bound is the schedule's own.
                NetworkType::Regtest => lez_refund_seconds
                    .checked_add(margin)
                    .context("calibrated later refund time overflow")?,
                // Testnet makes its blocks: the refund height arrives no sooner than
                // its blocks at the fastest pace the profile is sized for.
                NetworkType::Test | NetworkType::Main => {
                    let earliest = now
                        .checked_add(
                            u64::from(profile.zcash_refund_blocks())
                                .saturating_mul(PUBLIC_TESTNET_FASTEST_BLOCK_MILLIS)
                                / 1_000,
                        )
                        .context("Testnet later refund time overflow")?;
                    ensure!(
                        earliest >= lez_refund_seconds.saturating_add(margin),
                        "the profile's Zcash refund window is shorter than its LEZ refund delay \
                         plus margin at Testnet's fastest pace"
                    );
                    earliest
                }
            };
            Ok((lez_refund_ms, later_refund_earliest))
        }
        SwapDirection::TakerSellsLez => {
            let earlier_refund_latest_ms = match profile.zcash_network() {
                NetworkType::Regtest => lez_refund_seconds
                    .checked_sub(margin)
                    .and_then(|seconds| seconds.checked_mul(1_000))
                    .context("the LEZ refund time precedes its margin")?,
                // The anchor is the tip plus one: the refund height is N blocks
                // after it, N + 1 after the tip, each at the slowest pace.
                NetworkType::Test | NetworkType::Main => {
                    let latest = u64::from(profile.zcash_refund_blocks())
                        .checked_add(1)
                        .and_then(|blocks| blocks.checked_mul(PUBLIC_TESTNET_SLOWEST_BLOCK_MILLIS))
                        .and_then(|window| now.checked_mul(1_000)?.checked_add(window))
                        .context("Testnet earlier refund time overflow")?;
                    ensure!(
                        latest.saturating_add(margin.saturating_mul(1_000)) <= lez_refund_ms,
                        "the profile's Zcash refund window at Testnet's slowest pace leaves \
                         less than its margin before the LEZ refund"
                    );
                    latest
                }
            };
            Ok((earlier_refund_latest_ms, lez_refund_seconds))
        }
    }
}

pub(crate) fn public_key_hash(key: &PublicKey) -> [u8; 20] {
    match lez_zec_swap_sdk::p2pkh_address(key) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("a public key always maps to P2PKH"),
    }
}

#[cfg(test)]
fn sign_agreement(commitment: [u8; 32], key: &SecretKey) -> [u8; 64] {
    Secp256k1::signing_only()
        .sign_ecdsa(&Message::from_digest(commitment), key)
        .serialize_compact()
}

fn program_id_words(value: Hex32) -> [u32; 8] {
    let mut words = [0_u32; 8];
    for (word, chunk) in words.iter_mut().zip(value.as_bytes().chunks_exact(4)) {
        *word = u32::from_le_bytes(chunk.try_into().expect("four-byte program word"));
    }
    words
}

pub(crate) fn txid_display_hex(transaction_id: TxId) -> Hex32 {
    let mut display = *transaction_id.as_ref();
    display.reverse();
    Hex32::from_bytes(display)
}

pub(crate) fn block_hash_display_hex(block_hash: BlockHash) -> Hex32 {
    let mut display = block_hash.0;
    display.reverse();
    Hex32::from_bytes(display)
}

pub(crate) fn create_private_directory(path: &Path) -> Result<()> {
    DirBuilder::new()
        .mode(0o700)
        .create(path)
        .with_context(|| format!("failed to create private directory {}", path.display()))
}

pub(crate) fn write_private_new(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(
        !bytes.is_empty(),
        "refusing to write empty private material"
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("failed to create private file {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("failed to write private file {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync private file {}", path.display()))
}

#[cfg(test)]
mod claim_preimage_tests {
    use lez_zec_swap_sdk::Bip199Contract;
    use sha2::{Digest as _, Sha256};

    use super::fresh_claim_preimage;

    /// Two swaps provisioned together get different secrets, hence different
    /// digests and different BIP-199 scripts even with the same keys, amount
    /// and refund height (the shape of two catalog entries at one anchor).
    #[test]
    fn two_provisioned_swaps_have_different_secrets_and_scripts() {
        let first = fresh_claim_preimage().expect("rng");
        let second = fresh_claim_preimage().expect("rng");
        assert_ne!(*first, *second);
        assert_ne!(*first, [0_u8; 32]);
        let digest = |preimage: &[u8; 32]| -> [u8; 32] { Sha256::digest(preimage).into() };
        let contract = |preimage: &[u8; 32]| {
            Bip199Contract::new(240, [0x11; 20], digest(preimage), [0x22; 20])
        };
        assert_ne!(digest(&first), digest(&second));
        assert_ne!(contract(&first), contract(&second));
    }
}

#[cfg(test)]
pub(crate) mod testnet_provision_tests {
    use std::collections::HashMap;

    use lez_zebra_node_adapter::{
        ZebraCanonicalBlock, ZebraChainInfo, ZebraTransactionState, ZebraUnspentOutput,
    };

    use super::*;

    /// Mined at Testnet height 4,465,026, the NU7 activation block, by the
    /// key below (zec-swap-sdk `tests/zcash_testnet_vectors.rs`): output 2
    /// pays 712,420,000 zatoshis back to it.
    const NU7_TX: &str = concat!(
        "050000800a27a726d90a197700000000fa21440001b87b4df4d035a31b018abd33efbed35c0e66163d25b3d8",
        "8979c75f0329e659d7020000006a473044022043c971e06555e2413276314bfc5d881487e4eaa749148f87f3",
        "ac55b2b8fe07f1022061d53ac6c9d6e053cca4ce5b072c34eb252683a4cbd14da6a5b0be1cc8e549e8012102",
        "bc0a8bf008166b8a99a1bcf27086172fd709467ea55b429449a3fe6a92f03738ffffffff0300000000000000",
        "001d6a1b535601bd8a560dfb415d4babb99662dc157267d166b7c90000000010270000000000001976a91400",
        "0000000000000000000000000000000000000088aca0aa762a000000001976a9141aded9db78a48344f8541f",
        "fa7d4300ada64fc68088ac000000",
    );
    const NU7_TX_HEIGHT: u32 = 4_465_026;
    /// The coinbase of height 1 on the pinned Zebra 7 Regtest fixture, NU7
    /// active from height 1 (tests/e2e/zebra/zebrad.toml, 2026-11-11): a v6
    /// transaction (version group d884b698, branch 77190ad9) paying
    /// 208,333,333 zatoshis to the fixture's miner key, `[4; 32]`.
    const REGTEST_NU7_COINBASE: &str = concat!(
        "0600008098b684d8d90a19770000000001000000010000000000000000000000000000000000000000000000",
        "000000000000000000ffffffff285126f09fa6933a2039601c2a397699d2de9efe8dbc1c15ea18b323e3bd7f",
        "9ae732933ca8eb11e3f4ffffffff0115ea6a0c000000001976a914888dbbd7998f9f80d4f82b4f9ce6d5882f",
        "310aec88ac00000000",
    );
    const SPENDER: &str = "02bc0a8bf008166b8a99a1bcf27086172fd709467ea55b429449a3fe6a92f03738";

    #[derive(Debug, thiserror::Error)]
    #[error("not served by the fake")]
    pub(crate) struct Unserved;

    /// A Testnet Zebra serving one real block at `NU7_TX_HEIGHT` (and empty
    /// blocks elsewhere) and the UTXO set it is told; with `regtest_nu7`, the
    /// Zebra 7 Regtest fixture's block 1 instead.
    pub(crate) struct FakeTestnet {
        pub(crate) tip: u32,
        tx_height: u32,
        branch: BranchId,
        genesis: BlockHash,
        transaction: Vec<u8>,
        pub(crate) unspent: HashMap<(TxId, u32), u32>,
    }

    impl FakeTestnet {
        fn new() -> Self {
            Self {
                tip: NU7_TX_HEIGHT + 11,
                tx_height: NU7_TX_HEIGHT,
                branch: BranchId::Nu7,
                genesis: FundingChain::for_profile(ZecRefundProfile::for_id(
                    ZecProfileId::PublicTestnetV2,
                ))
                .unwrap()
                .identity
                .genesis_hash(),
                transaction: hex::decode(NU7_TX).unwrap(),
                unspent: HashMap::new(),
            }
        }

        pub(crate) fn regtest_nu7() -> Self {
            Self {
                tip: 1 + COINBASE_MATURITY_CONFIRMATIONS,
                tx_height: 1,
                branch: BranchId::Nu7,
                genesis: ZebraChainIdentity::deterministic_regtest_nu7().genesis_hash(),
                transaction: hex::decode(REGTEST_NU7_COINBASE).unwrap(),
                unspent: HashMap::new(),
            }
        }

        pub(crate) fn txid(&self) -> TxId {
            Transaction::read(self.transaction.as_slice(), BranchId::Nu7)
                .unwrap()
                .txid()
        }

        fn hash_at(height: u32) -> BlockHash {
            let mut hash = [0_u8; 32];
            hash[..4].copy_from_slice(&height.to_le_bytes());
            hash[31] = 1;
            BlockHash(hash)
        }
    }

    #[async_trait::async_trait]
    impl ZebraRpc for FakeTestnet {
        type Error = Unserved;

        async fn chain_info(&self) -> Result<ZebraChainInfo, Unserved> {
            Ok(ZebraChainInfo::new(
                ZebraRpcChain::Test,
                BlockHeight::from_u32(self.tip),
                Self::hash_at(self.tip),
                self.branch,
            ))
        }

        async fn block_hash(&self, height: BlockHeight) -> Result<BlockHash, Unserved> {
            Ok(if u32::from(height) == 0 {
                self.genesis
            } else {
                Self::hash_at(u32::from(height))
            })
        }

        async fn canonical_block(&self, hash: BlockHash) -> Result<ZebraCanonicalBlock, Unserved> {
            let height = u32::from_le_bytes(hash.0[..4].try_into().unwrap());
            let transactions = if height == self.tx_height {
                vec![self.txid()]
            } else {
                Vec::new()
            };
            Ok(ZebraCanonicalBlock::new(
                hash,
                BlockHeight::from_u32(height),
                transactions,
            ))
        }

        async fn block_transaction(
            &self,
            transaction_id: TxId,
            _: BlockHash,
        ) -> Result<Option<Vec<u8>>, Unserved> {
            Ok((transaction_id == self.txid()).then(|| self.transaction.clone()))
        }

        async fn mempool_transaction_ids(&self) -> Result<Vec<TxId>, Unserved> {
            Err(Unserved)
        }

        async fn raw_transaction(&self, _: TxId) -> Result<Option<Vec<u8>>, Unserved> {
            Err(Unserved)
        }

        async fn transaction_state(
            &self,
            _: TxId,
        ) -> Result<Option<ZebraTransactionState>, Unserved> {
            Err(Unserved)
        }

        async fn unspent_output(
            &self,
            outpoint: &OutPoint,
        ) -> Result<Option<ZebraUnspentOutput>, Unserved> {
            let transaction =
                Transaction::read(self.transaction.as_slice(), BranchId::Nu7).unwrap();
            Ok(self
                .unspent
                .get(&(TxId::from_bytes(*outpoint.hash()), outpoint.n()))
                .map(|confirmations| {
                    ZebraUnspentOutput::new(
                        Self::hash_at(self.tip),
                        *confirmations,
                        transaction.transparent_bundle().unwrap().vout
                            [usize::try_from(outpoint.n()).unwrap()]
                        .clone(),
                    )
                }))
        }

        async fn send_raw_transaction(&self, _: &[u8]) -> Result<TxId, Unserved> {
            Err(Unserved)
        }
    }

    fn testnet() -> FundingChain {
        FundingChain::for_profile(ZecRefundProfile::for_id(ZecProfileId::PublicTestnetV2)).unwrap()
    }

    fn spender() -> PublicKey {
        PublicKey::from_slice(&hex::decode(SPENDER).unwrap()).unwrap()
    }

    #[test]
    fn the_testnet_chain_is_named_by_its_genesis_and_nu7() {
        let chain = testnet();
        assert_eq!(chain.identity.network(), NetworkType::Test);
        assert_eq!(chain.identity.consensus_branch_id(), BranchId::Nu7);
        assert_eq!(
            chain.identity.genesis_hash().to_string(),
            TESTNET_GENESIS_HASH_HEX
        );
        assert_eq!(chain.min_confirmations, 10);
        assert!(chain.tip_may_advance);
        let regtest = FundingChain::for_profile(ZecRefundProfile::for_id(
            ZecProfileId::DeterministicLocalDevnetV3,
        ))
        .unwrap();
        assert_eq!(
            regtest.identity,
            ZebraChainIdentity::deterministic_regtest_nu6_2()
        );
        assert_eq!(regtest.min_confirmations, COINBASE_MATURITY_CONFIRMATIONS);
        assert!(!regtest.tip_may_advance);
    }

    #[test]
    fn the_regtest_chain_follows_the_profile_branch() {
        for (profile, identity) in [
            (
                ZecProfileId::DeterministicLocalDevnetV4,
                ZebraChainIdentity::deterministic_regtest_nu6_2(),
            ),
            (
                ZecProfileId::DeterministicLocalDevnetV5,
                ZebraChainIdentity::deterministic_regtest_nu7(),
            ),
            (
                ZecProfileId::DeterministicLocalDevnetV6,
                ZebraChainIdentity::deterministic_regtest_nu7(),
            ),
            (
                ZecProfileId::DeterministicLocalV2,
                ZebraChainIdentity::deterministic_regtest_nu7(),
            ),
        ] {
            let chain = FundingChain::for_profile(ZecRefundProfile::for_id(profile)).unwrap();
            assert_eq!(chain.identity, identity, "{}", profile.as_str());
            assert_eq!(chain.min_confirmations, COINBASE_MATURITY_CONFIRMATIONS);
            assert!(!chain.tip_may_advance);
        }
        for spelling in [
            "deterministic-local-v2",
            "deterministic-local-devnet-v5",
            "deterministic-local-devnet-v6",
        ] {
            let profile: LocalPocProfile =
                serde_json::from_value(serde_json::json!(spelling)).unwrap();
            assert_eq!(profile.id().as_str(), spelling);
        }
    }

    /// Zebra 7's NU7 coinbase is a v6 transaction; the Regtest funder search
    /// reads it and takes its output once it is mature, and not before.
    #[tokio::test]
    async fn a_regtest_nu7_funder_output_is_a_mature_v6_coinbase() {
        let chain = FundingChain::for_profile(ZecRefundProfile::for_id(
            ZecProfileId::DeterministicLocalDevnetV5,
        ))
        .unwrap();
        let miner = PublicKey::from_secret_key(
            &Secp256k1::new(),
            &SecretKey::from_slice(&[4; 32]).unwrap(),
        );
        let mut zebra = FakeTestnet::regtest_nu7();
        let transaction = Transaction::read(zebra.transaction.as_slice(), BranchId::Nu7).unwrap();
        assert_eq!(transaction.version().header() & 0x7fff_ffff, 6);
        assert_eq!(transaction.consensus_branch_id(), BranchId::Nu7);
        assert_eq!(
            transaction.txid().to_string(),
            "7df23a26010cd4aa7e6174bf146dc9d0ab7ec3197d6aaa8d8cfee439c1b59c98"
        );
        let txid = zebra.txid();
        zebra
            .unspent
            .insert((txid, 0), COINBASE_MATURITY_CONFIRMATIONS + 1);
        let (candidate, tip, genesis) =
            select_funding_candidate(&zebra, &chain, &miner, 0, &[], 1, 100_000_000)
                .await
                .unwrap();
        assert_eq!(candidate.outpoint, OutPoint::new(*txid.as_ref(), 0));
        assert_eq!(u64::from(candidate.output.value()), 208_333_333);
        assert_eq!(candidate.height, 1);
        assert_eq!(tip, 1 + COINBASE_MATURITY_CONFIRMATIONS);
        assert_eq!(genesis, chain.identity.genesis_hash());

        // One block short of maturity there is nothing to take; nor on a
        // Zebra that still runs NU6.2.
        let mut young = FakeTestnet::regtest_nu7();
        young.tip = COINBASE_MATURITY_CONFIRMATIONS - 1;
        young
            .unspent
            .insert((txid, 0), COINBASE_MATURITY_CONFIRMATIONS - 1);
        assert!(
            select_funding_candidate(&young, &chain, &miner, 0, &[], 1, 100_000_000)
                .await
                .is_err()
        );
        let mut old_branch = FakeTestnet::regtest_nu7();
        old_branch.branch = BranchId::Nu6_2;
        old_branch
            .unspent
            .insert((txid, 0), COINBASE_MATURITY_CONFIRMATIONS + 1);
        assert!(
            select_funding_candidate(&old_branch, &chain, &miner, 0, &[], 1, 100_000_000)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_testnet_funder_output_is_an_ordinary_deep_unspent_payment() {
        let mut zebra = FakeTestnet::new();
        let txid = zebra.txid();
        zebra.unspent.insert((txid, 2), 12);
        let (candidate, tip, genesis) = select_funding_candidate(
            &zebra,
            &testnet(),
            &spender(),
            0,
            &[],
            NU7_TX_HEIGHT - 5,
            100_000_000 + FUNDING_FEE_ZATOSHIS + MINIMUM_CHANGE_ZATOSHIS,
        )
        .await
        .unwrap();
        assert_eq!(candidate.outpoint, OutPoint::new(*txid.as_ref(), 2));
        assert_eq!(u64::from(candidate.output.value()), 712_420_000);
        assert_eq!(candidate.height, NU7_TX_HEIGHT);
        assert_eq!(
            hex::encode(candidate.rpc_transaction_id.as_bytes()),
            "a85b0f6a352b9699d4221a93eeadbab457862c4dec2a3ac7ccd0d2f034fc89ea"
        );
        assert_eq!(tip, NU7_TX_HEIGHT + 11);
        assert_eq!(genesis, testnet().identity.genesis_hash());
    }

    #[tokio::test]
    async fn a_testnet_funder_search_refuses_what_it_cannot_spend() {
        let wanted = 100_000_000 + FUNDING_FEE_ZATOSHIS + MINIMUM_CHANGE_ZATOSHIS;
        let select = |zebra: FakeTestnet, from: u32, min_value: u64| async move {
            select_funding_candidate(&zebra, &testnet(), &spender(), 0, &[], from, min_value)
                .await
                .map(|(candidate, _, _)| candidate.outpoint)
        };
        let unspent = |confirmations: u32| {
            let mut zebra = FakeTestnet::new();
            let txid = zebra.txid();
            zebra.unspent.insert((txid, 2), confirmations);
            zebra
        };
        // Too shallow for the profile's ten confirmations, spent, or too small.
        assert!(select(unspent(9), NU7_TX_HEIGHT - 5, wanted).await.is_err());
        assert!(
            select(FakeTestnet::new(), NU7_TX_HEIGHT - 5, wanted)
                .await
                .is_err()
        );
        assert!(
            select(unspent(12), NU7_TX_HEIGHT - 5, 712_420_001)
                .await
                .is_err()
        );
        // The search needs a start within its bound of the tip.
        assert!(select(unspent(12), 0, wanted).await.is_err());
        assert!(
            select(
                unspent(12),
                NU7_TX_HEIGHT + 11 - MAX_TESTNET_FUNDING_SCAN_BLOCKS,
                wanted
            )
            .await
            .is_err()
        );
        // Another branch (NU6.2, the old profile's) or another genesis is not Testnet on NU7.
        let mut old_branch = unspent(12);
        old_branch.branch = BranchId::Nu6_2;
        assert!(select(old_branch, NU7_TX_HEIGHT - 5, wanted).await.is_err());
        let mut regtest = unspent(12);
        regtest.genesis = ZebraChainIdentity::deterministic_regtest_nu6_2().genesis_hash();
        assert!(select(regtest, NU7_TX_HEIGHT - 5, wanted).await.is_err());
    }

    #[test]
    fn the_taker_sells_lez_refund_plan_puts_the_makers_zcash_refund_first() {
        let now = 1_800_000_000;
        // devnet-v7: 12 min LEZ refund delay, 120 s margin. The Zcash refund
        // is declared at the LEZ refund time minus the margin; the Taker's LEZ
        // refund opens at the LEZ refund time.
        let v7 = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalDevnetV7);
        assert_eq!(
            refund_plan(v7, SwapDirection::TakerSellsLez, now, now + 720).unwrap(),
            ((now + 600) * 1_000, now + 720)
        );
        // public-testnet-v3: 95 blocks + 1 at 75 s = 7 200 s after the anchor,
        // which leaves the 7 200 s margin before the 14 400 s LEZ refund.
        let v3 = ZecRefundProfile::for_id(ZecProfileId::PublicTestnetV3);
        assert_eq!(
            refund_plan(v3, SwapDirection::TakerSellsLez, now, now + 14_400).unwrap(),
            (now * 1_000 + 7_200_000, now + 14_400)
        );
        // TakerSellsForeign keeps its plan: the Maker's LEZ refund first at
        // the LEZ refund time, the Taker's Zcash refund after it.
        let v5 = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalDevnetV5);
        assert_eq!(
            refund_plan(v5, SwapDirection::TakerSellsForeign, now, now + 720).unwrap(),
            (
                (now + 720) * 1_000,
                now + 720 + v5.required_margin().value()
            )
        );
        let v2 = ZecRefundProfile::for_id(ZecProfileId::PublicTestnetV2);
        assert_eq!(
            refund_plan(v2, SwapDirection::TakerSellsForeign, now, now + 1_800).unwrap(),
            (
                (now + 1_800) * 1_000,
                now + u64::from(v2.zcash_refund_blocks()) * PUBLIC_TESTNET_FASTEST_BLOCK_MILLIS
                    / 1_000
            )
        );
        // A Zcash window that does not leave the margin before the LEZ refund
        // is refused rather than signed.
        assert!(refund_plan(v3, SwapDirection::TakerSellsLez, now, now + 14_399).is_err());
    }
}

// The test payer's imports (feature test-payer).
#[cfg(feature = "test-payer")]
use lez_zec_swap_sdk::{
    TransparentFundingRequest, TransparentUtxo, build_transparent_transfer_transaction,
};

/// The test payer refuses a Zebra that is not the chain it pays on: a mainnet
/// Zebra is refused before anything is signed or sent.
#[cfg(all(test, feature = "test-payer"))]
mod test_payer_tests {
    use std::{
        io::{BufRead as _, BufReader, Read as _, Write as _},
        net::TcpListener,
        os::unix::fs::PermissionsExt as _,
        path::PathBuf,
    };

    use super::{TestnetZebraCredential, transfer_local_v0_2_regtest, transfer_v0_2_testnet};

    /// A Zebra on loopback that answers one JSON-RPC call as a mainnet node
    /// answers `getblockchaininfo`, and reports the methods it was asked.
    fn mainnet_zebra() -> (url::Url, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            let mut methods = Vec::new();
            if let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0_u8; length];
                reader.read_exact(&mut body).unwrap();
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                methods.push(request["method"].as_str().unwrap_or_default().to_owned());
                let answer = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "result": {
                        "chain": "main",
                        "blocks": 3_000_000,
                        "bestblockhash": "11".repeat(32),
                        "consensus": {"chaintip": "77190ad9", "nextblock": "77190ad9"},
                    },
                })
                .to_string();
                let mut stream = stream;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                )
                .unwrap();
            }
            methods
        });
        (url, server)
    }

    fn private_file(directory: &tempfile::TempDir, name: &str, contents: &str) -> PathBuf {
        let path = directory.path().join(name);
        std::fs::write(&path, contents).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    fn keys(directory: &tempfile::TempDir) -> (PathBuf, PathBuf) {
        (
            private_file(directory, "from.key", &"01".repeat(32)),
            private_file(directory, "to.key", &"02".repeat(32)),
        )
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_regtest_payment_refuses_a_mainnet_zebra_before_it_signs() {
        let directory = tempfile::tempdir().unwrap();
        let (from, to) = keys(&directory);
        let (url, server) = mainnet_zebra();
        let error = transfer_local_v0_2_regtest(&url, None, &from, &to, 100_000_000, 0)
            .await
            .expect_err("a mainnet Zebra must be refused");
        assert!(
            format!("{error:#}").contains("Zebra is not the deterministic Regtest fixture"),
            "{error:#}"
        );
        // It asked which chain this is and nothing else: no output read, nothing sent.
        assert_eq!(server.join().unwrap(), ["getblockchaininfo"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_testnet_payment_refuses_a_mainnet_zebra_before_it_signs() {
        let directory = tempfile::tempdir().unwrap();
        let (from, to) = keys(&directory);
        let cookie = private_file(&directory, ".cookie", "__cookie__:secret\n");
        let (url, server) = mainnet_zebra();
        let error = transfer_v0_2_testnet(
            &url,
            &TestnetZebraCredential::SelfHostedCookie(cookie),
            &from,
            &to,
            100_000_000,
            0,
            1,
        )
        .await
        .expect_err("a mainnet Zebra must be refused");
        assert!(
            format!("{error:#}").contains("Zebra is not Zcash Testnet"),
            "{error:#}"
        );
        assert_eq!(server.join().unwrap(), ["getblockchaininfo"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn equal_keys_are_refused_before_any_call() {
        let directory = tempfile::tempdir().unwrap();
        let key = private_file(&directory, "same.key", &"01".repeat(32));
        let url = url::Url::parse("http://127.0.0.1:9/").unwrap();
        let error = transfer_local_v0_2_regtest(&url, None, &key, &key, 1, 0)
            .await
            .expect_err("equal keys");
        assert!(format!("{error:#}").contains("must differ"), "{error:#}");
    }
}
