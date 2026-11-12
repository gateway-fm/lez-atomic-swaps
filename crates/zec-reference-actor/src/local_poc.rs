use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::{Cursor, Write as _},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, bail, ensure};
use lez_bridge_protocol::{
    DiscoveryWindow, Hex32, Participant as BridgeParticipant, RequestId, RunId,
    RuntimeCompatibility, RuntimeDescriptor,
};
use lez_swap_core::{Participant, SwapDirection, SwapId, UnixSeconds};
use lez_swap_store::maker_zec_chat_session_id;
use lez_zebra_node_adapter::{
    HttpZebraRpc, HttpZebraRpcConfig, ZebraChainIdentity, ZebraRpc, ZebraRpcChain,
};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, Bip199Contract, ExpectedBip199Output, LezAssetV1, LezChainIdentityV1,
    LezEnvironmentV1, MAX_ZEC_AGREEMENT_RECORD_BYTES, NegotiationTranscriptV1,
    PUBLIC_TESTNET_FASTEST_BLOCK_MILLIS, TransparentFundingRequest, TransparentUtxo,
    ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashFundingInputSetV1, ZcashFundingInputV1,
    ZcashTransparentDestinationV1, ZecAgreementBodyV1, ZecAgreementDraftV1, ZecAgreementRecordV1,
    ZecAgreementV1, ZecLezTermsV1, ZecParticipantIdentityV1, ZecParticipantsV1, ZecProfileId,
    ZecProfileRecordV1, ZecRefundPlanV1, ZecRefundProfile, ZecSwapBinding, ZecSwapBindingRecordV1,
    ZecTransactionPolicyV1, build_transparent_transfer_transaction, derive_lez_metadata_account_v1,
    derive_lez_native_custody_account_v1, derive_lez_public_account_v0_2, derive_lez_swap_id_v1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
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

use crate::config::{
    DeterministicLocalV0_2ActorConfigInput, ProvisionedZebraRoute,
    encode_deterministic_local_v0_2_actor_config, encode_rebound_local_v0_2_actor_config,
};
use crate::secure_file::{FilePrivacy, read_bounded_identified};
use crate::{ActorConfig, ActorRole, CandidateOutpoint, validate_actor_pair};

const SPEC_SCHEMA_VERSION: u16 = 1;
const MAX_SPEC_BYTES: usize = 32 * 1024;
const MAX_LEZ_SIGNER_KEY_BYTES: usize = 65;
// Retained v0.2.0 fixture transfer program, kept for the record:
// `FrexXMbyY6iZjwUo8DV3jfB8donj8H4kLRHT7xswCfJg`. Newer chains differ.
const FUNDED_ZCASH_KEY_BYTE: u8 = 4;
const OTHER_ZCASH_KEY_BYTE: u8 = 2;
/// The claim secret of one provisioned swap: 32 bytes from the OS RNG, owner-
/// private like the keys beside it (stored only in the funder's bundle; the
/// agreement carries its SHA-256). Never derived from public inputs such as
/// the swap id, which the agreement, the offers and the evidence all show.
pub(crate) fn fresh_claim_preimage() -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let mut preimage = Zeroizing::new([0_u8; 32]);
    getrandom::fill(&mut preimage[..]).map_err(|_| anyhow::anyhow!("claim preimage RNG"))?;
    Ok(preimage)
}
const LEZ_NATIVE_AMOUNT: u128 = 50_000;
const ZCASH_PRINCIPAL_ZATOSHIS: u64 = 100_000_000;
pub(crate) const FUNDING_FEE_ZATOSHIS: u64 = 10_000;
pub(crate) const MINIMUM_CHANGE_ZATOSHIS: u64 = 1_000;
const CLAIM_FEE_ZATOSHIS: u64 = 10_000;
const REFUND_FEE_ZATOSHIS: u64 = 10_000;
const COINBASE_MATURITY_CONFIRMATIONS: u32 = 100;
const AGREEMENT_LIFETIME_SECONDS: u64 = 3_600;
const COUNTERPARTY_SCAN_BLOCKS: u32 = 1_024;
/// Zcash Testnet's genesis block hash as `getblockhash 0` prints it.
const TESTNET_GENESIS_HASH_HEX: &str =
    "05a60a92d99d85997cce3b87616c089f6124d7342af37106edc76126334a2c38";
/// Blocks a Testnet funder-output search may read: ~18 h of Testnet at its
/// 22.7 s average. Each costs a hash, a block and its transactions over a
/// remote or rate-limited route; a search wider than this names a later
/// `zcash_funding_scan_from_height`.
pub(crate) const MAX_TESTNET_FUNDING_SCAN_BLOCKS: u32 = 2_880;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvisionSpec {
    schema_version: u16,
    run_id: RunId,
    swap_id: String,
    direction: LocalPocDirection,
    /// Recovery profile; absent means the deterministic test-lane profile.
    #[serde(default)]
    profile: LocalPocProfile,
    lez_runtime: LezRuntimeSpec,
    bridge: BridgeSpec,
    zebra_endpoint: Url,
    lez_discovery_start_height: u64,
    lez_discovery_max_blocks: u32,
    /// Mature funder outputs to pass over before taking one. Two corridors
    /// provisioned side by side (a concurrent-swap fixture) would otherwise
    /// both be funded from the first mature output, since neither has spent
    /// it yet when the second is provisioned; the second names 1.
    #[serde(default)]
    zcash_funding_skip: u32,
    /// Lowest Zebra height the funder-output search reads; absent, height 1.
    /// The search reads every block from there to 100 below the tip, ~3 min
    /// for a devnet zebra 23 000 blocks tall: the devnet provisioner names the
    /// height of the payments it just made to a Taker's key, below which that
    /// key holds nothing it means to spend.
    #[serde(default)]
    zcash_funding_scan_from_height: u32,
    /// How the provisioner and both actors reach `zebra_endpoint`; absent,
    /// the deterministic Regtest fixture on loopback.
    #[serde(default)]
    zebra_route: LocalPocZebraRoute,
    /// The swap's Zcash principal, zatoshis; absent, 1 ZEC (the devnet's).
    #[serde(default)]
    zcash_principal_zatoshis: Option<u64>,
}

impl ProvisionSpec {
    fn zcash_principal(&self) -> u64 {
        self.zcash_principal_zatoshis
            .unwrap_or(ZCASH_PRINCIPAL_ZATOSHIS)
    }
}

/// The Zebra a corridor is provisioned against, and its credential.
#[derive(Clone, Default, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum LocalPocZebraRoute {
    /// The deterministic Regtest fixture: loopback, no credential.
    #[default]
    DeterministicLocal,
    /// A self-hosted Zcash Testnet Zebra on loopback; `cookie_file` is its
    /// owner-private `.cookie` (`__cookie__:<password>`).
    SelfHostedCookie { cookie_file: PathBuf },
    /// Tatum's Testnet Zebrad gateway; `api_key_file` holds the owner-private
    /// key sent as `x-api-key`.
    TatumTestnetXApiKey { api_key_file: PathBuf },
}

impl LocalPocZebraRoute {
    const fn credential_file(&self) -> Option<&PathBuf> {
        match self {
            Self::DeterministicLocal => None,
            Self::SelfHostedCookie { cookie_file } => Some(cookie_file),
            Self::TatumTestnetXApiKey { api_key_file } => Some(api_key_file),
        }
    }

    pub(crate) const fn network(&self) -> NetworkType {
        match self {
            Self::DeterministicLocal => NetworkType::Regtest,
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
        (LocalPocZebraRoute::DeterministicLocal, None) => {
            HttpZebraRpcConfig::new(endpoint.as_str())
        }
        (LocalPocZebraRoute::SelfHostedCookie { .. }, Some(cookie)) => {
            HttpZebraRpcConfig::new(endpoint.as_str())
                .with_cookie_credentials(cookie)
                .context("invalid self-hosted Zebra cookie")?
        }
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

/// Owner-private LEZ signer files provisioned before a fresh local genesis.
#[derive(Clone)]
pub struct LocalPocLezSignerFiles {
    maker: PathBuf,
    taker: PathBuf,
}

impl std::fmt::Debug for LocalPocLezSignerFiles {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalPocLezSignerFiles")
            .field("maker", &"<redacted>")
            .field("taker", &"<redacted>")
            .finish()
    }
}

/// Owner-private Zcash key files of both roles: 32 raw bytes or 64 hex
/// characters each. Absent, the deterministic test-lane keys apply.
pub struct LocalPocZcashKeyFiles {
    maker: PathBuf,
    taker: PathBuf,
}

impl std::fmt::Debug for LocalPocZcashKeyFiles {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalPocZcashKeyFiles")
            .field("maker", &"<redacted>")
            .field("taker", &"<redacted>")
            .finish()
    }
}

impl LocalPocZcashKeyFiles {
    /// Binds one complete Maker/Taker Zcash key pair.
    #[must_use]
    pub fn new(maker: PathBuf, taker: PathBuf) -> Self {
        Self { maker, taker }
    }
}

/// The two roles' Zcash secrets the corridor is built with.
struct ZcashKeyMaterial {
    maker: Zeroizing<[u8; 32]>,
    taker: Zeroizing<[u8; 32]>,
}

impl ZcashKeyMaterial {
    /// The test-lane constants: the funder of `direction` owns the coinbase
    /// key, the other role the second constant.
    fn deterministic(direction: LocalPocDirection) -> Self {
        let (maker_byte, taker_byte) = match direction.zcash_funder() {
            ActorRole::Maker => (FUNDED_ZCASH_KEY_BYTE, OTHER_ZCASH_KEY_BYTE),
            ActorRole::Taker => (OTHER_ZCASH_KEY_BYTE, FUNDED_ZCASH_KEY_BYTE),
        };
        Self {
            maker: Zeroizing::new([maker_byte; 32]),
            taker: Zeroizing::new([taker_byte; 32]),
        }
    }

    fn funder(&self, direction: LocalPocDirection) -> &Zeroizing<[u8; 32]> {
        match direction.zcash_funder() {
            ActorRole::Maker => &self.maker,
            ActorRole::Taker => &self.taker,
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

fn regtest_p2pkh_address(public_key: &PublicKey) -> Result<String> {
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
pub async fn transfer_local_v0_2_regtest(
    zebra_endpoint: &Url,
    from_key_file: &Path,
    to_key_file: &Path,
    value_zatoshis: u64,
    skip: u32,
) -> Result<LocalPocTransferSummary> {
    let rpc = connect_zebra(
        zebra_endpoint,
        &LocalPocZebraRoute::DeterministicLocal,
        None,
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
        // Any mature output of the devnet's (1 ZEC + fees) shape pays a transfer.
        ZCASH_PRINCIPAL_ZATOSHIS
            .saturating_add(FUNDING_FEE_ZATOSHIS)
            .saturating_add(MINIMUM_CHANGE_ZATOSHIS),
        100,
    )
    .await
}

/// The credential of a Zcash Testnet route (the self-hosted Zebra's cookie
/// file or the Tatum API key file), owner-private.
#[derive(Clone)]
pub enum TestnetZebraCredential {
    /// A self-hosted Testnet Zebra on loopback and its `.cookie` file.
    SelfHostedCookie(PathBuf),
    /// Tatum's Testnet Zebrad gateway and the API key file.
    TatumApiKey(PathBuf),
}

impl std::fmt::Debug for TestnetZebraCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::SelfHostedCookie(_) => "SelfHostedCookie(<redacted>)",
            Self::TatumApiKey(_) => "TatumApiKey(<redacted>)",
        })
    }
}

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

fn load_zcash_key_material(
    key_files: Option<&LocalPocZcashKeyFiles>,
    direction: LocalPocDirection,
) -> Result<ZcashKeyMaterial> {
    let Some(files) = key_files else {
        return Ok(ZcashKeyMaterial::deterministic(direction));
    };
    let maker = read_zcash_key_file(&files.maker, "Maker")?;
    let taker = read_zcash_key_file(&files.taker, "Taker")?;
    ensure!(*maker != *taker, "Maker and Taker Zcash keys must differ");
    Ok(ZcashKeyMaterial { maker, taker })
}

impl LocalPocLezSignerFiles {
    /// Binds one complete Maker/Taker signer pair for private no-clobber copying.
    #[must_use]
    pub fn new(maker: PathBuf, taker: PathBuf) -> Self {
        Self { maker, taker }
    }
}

struct LezSignerMaterial {
    maker: Zeroizing<Vec<u8>>,
    taker: Zeroizing<Vec<u8>>,
}

/// The named profiles a local corridor may be provisioned under.
#[derive(Clone, Copy, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LocalPocProfile {
    /// `deterministic-local-v1`: the isolated test lane (60 s LEZ refund delay).
    #[default]
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
        }
    }
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum LocalPocDirection {
    TakerSellsForeign,
    TakerSellsLez,
}

impl LocalPocDirection {
    const fn protocol(self) -> SwapDirection {
        match self {
            Self::TakerSellsForeign => SwapDirection::TakerSellsForeign,
            Self::TakerSellsLez => SwapDirection::TakerSellsLez,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::TakerSellsForeign => "taker_sells_foreign",
            Self::TakerSellsLez => "taker_sells_lez",
        }
    }

    const fn zcash_funder(self) -> ActorRole {
        match self {
            Self::TakerSellsForeign => ActorRole::Taker,
            Self::TakerSellsLez => ActorRole::Maker,
        }
    }

    const fn lez_depositor(self) -> ActorRole {
        match self {
            Self::TakerSellsForeign => ActorRole::Maker,
            Self::TakerSellsLez => ActorRole::Taker,
        }
    }

    const fn preimage_owner(self) -> ActorRole {
        self.zcash_funder()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LezRuntimeSpec {
    chain_id: Hex32,
    channel_id: Hex32,
    genesis_block_hash: Hex32,
    escrow_program_id: Hex32,
    authenticated_transfer_program_id_base58: String,
    maker_signer_account_id_base58: String,
    taker_signer_account_id_base58: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BridgeSpec {
    maker_endpoint: Url,
    taker_endpoint: Url,
}

/// Secret-free result of provisioning one pair of local actor inputs.
#[derive(Debug, Serialize)]
pub struct LocalPocProvisionSummary {
    schema_version: u16,
    run_id: RunId,
    direction: &'static str,
    agreement_file: PathBuf,
    signed_agreement_sha256: Hex32,
    maker_zcash_public_key: String,
    taker_zcash_public_key: String,
    authenticated_transfer_program_id: Hex32,
    authenticated_transfer_program_id_words: [u32; 8],
    maker: RolePaths,
    taker: RolePaths,
    zebra_tip_height: u32,
    zcash_refund_height: u32,
    /// Height of the funder output's block, on Testnet: the next corridor's
    /// funder search starts there (its change lands later).
    #[serde(skip_serializing_if = "Option::is_none")]
    zcash_candidate_height: Option<u32>,
    zcash_candidate_owner: &'static str,
    lez_native_amount: u128,
    lez_depositor_role: &'static str,
    lez_depositor_account_id_base58: String,
    private_material_disclosed: bool,
    actor_pair_validated: bool,
}

/// Secret-free result of binding a validated chain-fact template to Chat.
#[derive(Debug, Serialize)]
pub struct LocalPocChatDraftSummary {
    schema_version: u16,
    draft_file: PathBuf,
    draft_sha256: Hex32,
    swap_id: String,
    maker_public_key: String,
    reservation_id: RequestId,
    offer_commitment: Hex32,
    offer_expires_at_unix_seconds: u64,
    private_material_disclosed: bool,
}

/// Secret-free result of producing fresh actors from a Chat agreement.
#[derive(Debug, Serialize)]
pub struct LocalPocChatFinalizeSummary {
    schema_version: u16,
    run_id: RunId,
    swap_id: String,
    agreement_file: PathBuf,
    signed_agreement_sha256: Hex32,
    maker_config_file: PathBuf,
    taker_config_file: PathBuf,
    maker_state_root: PathBuf,
    taker_state_root: PathBuf,
    private_material_disclosed: bool,
    actor_pair_validated: bool,
}

#[derive(Debug, Serialize)]
struct RolePaths {
    state_root: PathBuf,
    config_file: PathBuf,
    runtime_file: PathBuf,
    lez_signer_key_file: PathBuf,
    sidecar_capability_file: PathBuf,
}

pub(crate) struct SelectedCandidate {
    pub(crate) outpoint: OutPoint,
    pub(crate) output: TxOut,
    pub(crate) rpc_transaction_id: Hex32,
    /// Height of the block that holds it.
    pub(crate) height: u32,
}

struct AgreementFixture {
    wire: Zeroizing<Vec<u8>>,
    sha256: Hex32,
    maker_zcash_public_key: [u8; 33],
    taker_zcash_public_key: [u8; 33],
    maker_zcash_key: Zeroizing<[u8; 32]>,
    taker_zcash_key: Zeroizing<[u8; 32]>,
    preimage: Zeroizing<[u8; 32]>,
    zcash_candidate: CandidateOutpoint,
    zcash_refund_height: u32,
}

/// Provisions a shared countersigned agreement and two isolated actor inputs.
///
/// The spec selects either supported ZEC direction. The taker always funds the
/// first leg; the participant funding Zcash exclusively receives the preimage
/// and exact mature Regtest candidate, while the opposite participant funds
/// LEZ. Both local identities use the deterministic keys already funded by the
/// retained v0.2 and Zebra fixtures.
///
/// # Errors
///
/// Fails closed for an unsafe spec, unexpected deterministic runtime identity,
/// non-Regtest Zebra, unstable RPC view, missing mature actor-owned output,
/// invalid agreement, unsafe output path, file collision, or actor-pair check.
#[allow(clippy::too_many_lines)]
pub async fn provision_local_v0_2_corridor(
    spec_file: &Path,
    output_root: &Path,
) -> Result<LocalPocProvisionSummary> {
    provision_local_v0_2_corridor_with_signers(spec_file, output_root, None).await
}

/// Provisions a local corridor with an optional pre-provisioned LEZ signer pair.
///
/// The external path is reserved for a fresh genesis whose public accounts and
/// owner-private signer files were created together by the canonical identity
/// provisioner. The supplied bytes are read once, cryptographically bound to
/// the spec accounts, and copied no-clobber into the isolated actor roots.
///
/// # Errors
///
/// In addition to [`provision_local_v0_2_corridor`], fails for absent, aliased,
/// non-canonical, non-private, malformed, equal, or account-mismatched signers.
pub async fn provision_local_v0_2_corridor_with_signers(
    spec_file: &Path,
    output_root: &Path,
    signer_files: Option<&LocalPocLezSignerFiles>,
) -> Result<LocalPocProvisionSummary> {
    provision_local_v0_2_corridor_with_material(spec_file, output_root, signer_files, None).await
}

/// Provisions a local corridor with optional pre-provisioned LEZ signers and
/// optional explicit Zcash keys for both roles.
///
/// With key files, the role that funds Zcash in the spec's direction must own
/// mature Regtest outputs under its key (zebra mines to the Maker's; a Taker
/// that funds is paid by a plain transfer first). Without them the
/// deterministic test-lane keys apply, as before.
///
/// # Errors
///
/// In addition to [`provision_local_v0_2_corridor_with_signers`], fails for an
/// absent, non-private, malformed or equal Zcash key file pair.
#[allow(clippy::too_many_lines)]
pub async fn provision_local_v0_2_corridor_with_material(
    spec_file: &Path,
    output_root: &Path,
    signer_files: Option<&LocalPocLezSignerFiles>,
    zcash_key_files: Option<&LocalPocZcashKeyFiles>,
) -> Result<LocalPocProvisionSummary> {
    let spec = load_spec(spec_file)?;
    let zcash_keys = load_zcash_key_material(zcash_key_files, spec.direction)?;
    let signer_material = load_lez_signer_material(
        signer_files,
        &spec.lez_runtime.maker_signer_account_id_base58,
        &spec.lez_runtime.taker_signer_account_id_base58,
    )?;
    validate_spec(&spec)?;
    validate_new_output_root(output_root)?;

    let maker_lez_account = decode_base58_32(&spec.lez_runtime.maker_signer_account_id_base58)
        .context("invalid deterministic maker LEZ account")?;
    let taker_lez_account = decode_base58_32(&spec.lez_runtime.taker_signer_account_id_base58)
        .context("invalid deterministic taker LEZ account")?;
    let authenticated_transfer =
        decode_base58_32(&spec.lez_runtime.authenticated_transfer_program_id_base58)
            .context("invalid authenticated-transfer program")?;

    let zebra_credential = read_zebra_credential(&spec.zebra_route)?;
    let rpc = connect_zebra(
        &spec.zebra_endpoint,
        &spec.zebra_route,
        zebra_credential.as_ref().map(|secret| secret.as_slice()),
    )?;
    let mut funding_zcash_secret = SecretKey::from_slice(&zcash_keys.funder(spec.direction)[..])
        .context("invalid Zcash funder key")?;
    let funding_zcash_public =
        PublicKey::from_secret_key(&Secp256k1::signing_only(), &funding_zcash_secret);
    funding_zcash_secret.non_secure_erase();
    let (candidate, tip_height, genesis_hash) = select_funding_candidate(
        &rpc,
        &FundingChain::for_profile(ZecRefundProfile::for_id(spec.profile.id()))?,
        &funding_zcash_public,
        spec.zcash_funding_skip,
        &[],
        spec.zcash_funding_scan_from_height,
        spec.zcash_principal()
            .saturating_add(FUNDING_FEE_ZATOSHIS)
            .saturating_add(MINIMUM_CHANGE_ZATOSHIS),
    )
    .await?;

    let swap_id = SwapId::new(spec.swap_id.clone()).context("invalid swap ID")?;
    let fixture = build_agreement(
        &spec,
        &zcash_keys,
        &swap_id,
        maker_lez_account,
        taker_lez_account,
        authenticated_transfer,
        &candidate,
        tip_height,
    )?;

    create_private_directory(output_root)?;
    let shared_root = output_root.join("shared");
    let maker_root = output_root.join("maker");
    let taker_root = output_root.join("taker");
    for directory in [&shared_root, &maker_root, &taker_root] {
        create_private_directory(directory)?;
    }
    let maker_state = maker_root.join("state");
    let taker_state = taker_root.join("state");
    create_private_directory(&maker_state)?;
    create_private_directory(&taker_state)?;

    let agreement_file = shared_root.join("agreement-v2.borsh");
    write_private_new(&agreement_file, fixture.wire.as_slice())?;

    let maker_paths = material_paths(&maker_root, &maker_state);
    let taker_paths = material_paths(&taker_root, &taker_state);
    let mut maker_recovery = Zeroizing::new([0_u8; 32]);
    let mut taker_recovery = Zeroizing::new([0_u8; 32]);
    getrandom::fill(&mut maker_recovery[..])
        .map_err(|_| anyhow::anyhow!("maker recovery randomness unavailable"))?;
    getrandom::fill(&mut taker_recovery[..])
        .map_err(|_| anyhow::anyhow!("taker recovery randomness unavailable"))?;
    avoid_zero_or_equal(&mut maker_recovery, &mut taker_recovery);
    write_private_new(&maker_paths.claim_recovery_key, maker_recovery.as_ref())?;
    write_private_new(&taker_paths.claim_recovery_key, taker_recovery.as_ref())?;
    write_private_new(&maker_paths.zcash_key, fixture.maker_zcash_key.as_ref())?;
    write_private_new(&taker_paths.zcash_key, fixture.taker_zcash_key.as_ref())?;
    match spec.direction.preimage_owner() {
        ActorRole::Maker => {
            write_private_new(&maker_paths.claim_preimage, fixture.preimage.as_ref())?;
        }
        ActorRole::Taker => {
            write_private_new(&taker_paths.claim_preimage, fixture.preimage.as_ref())?;
        }
    }
    write_private_new(
        &maker_paths.lez_signer_key,
        signer_material.maker.as_slice(),
    )?;
    write_private_new(
        &taker_paths.lez_signer_key,
        signer_material.taker.as_slice(),
    )?;

    let maker_capability = random_capability()?;
    let mut taker_capability = random_capability()?;
    while maker_capability.as_slice() == taker_capability.as_slice() {
        taker_capability = random_capability()?;
    }
    write_private_new(&maker_paths.capability, maker_capability.as_slice())?;
    write_private_new(&taker_paths.capability, taker_capability.as_slice())?;
    // Each actor reads its own copy of a Testnet route's credential, beside
    // its keys; the Regtest fixture has none.
    if let Some(secret) = &zebra_credential {
        write_private_new(&maker_paths.zebra_credential, secret.as_slice())?;
        write_private_new(&taker_paths.zebra_credential, secret.as_slice())?;
    }

    let discovery = DiscoveryWindow::new(
        spec.lez_discovery_start_height,
        spec.lez_discovery_max_blocks,
    )
    .context("invalid LEZ discovery window")?;
    let maker_runtime = runtime_descriptor(
        &spec,
        BridgeParticipant::Maker,
        Hex32::from_bytes(maker_lez_account),
    );
    let taker_runtime = runtime_descriptor(
        &spec,
        BridgeParticipant::Taker,
        Hex32::from_bytes(taker_lez_account),
    );
    write_private_new(
        &maker_paths.runtime,
        &serde_json::to_vec_pretty(&maker_runtime)
            .context("failed to encode maker runtime descriptor")?,
    )?;
    write_private_new(
        &taker_paths.runtime,
        &serde_json::to_vec_pretty(&taker_runtime)
            .context("failed to encode taker runtime descriptor")?,
    )?;
    let zebra_genesis = block_hash_display_hex(genesis_hash);
    let (maker_candidates, taker_candidates) = match spec.direction.zcash_funder() {
        ActorRole::Maker => (vec![fixture.zcash_candidate], Vec::new()),
        ActorRole::Taker => (Vec::new(), vec![fixture.zcash_candidate]),
    };

    let maker_config = encode_deterministic_local_v0_2_actor_config(actor_config_input(
        &RoleCorridor::of_spec(&spec, ActorRole::Maker),
        &swap_id,
        ActorRole::Maker,
        &agreement_file,
        fixture.sha256,
        &maker_paths,
        (spec.direction.preimage_owner() == ActorRole::Maker)
            .then(|| maker_paths.claim_preimage.clone()),
        maker_runtime,
        zebra_genesis,
        discovery,
        maker_candidates,
    ))
    .context("failed to encode maker actor config")?;
    let taker_config = encode_deterministic_local_v0_2_actor_config(actor_config_input(
        &RoleCorridor::of_spec(&spec, ActorRole::Taker),
        &swap_id,
        ActorRole::Taker,
        &agreement_file,
        fixture.sha256,
        &taker_paths,
        (spec.direction.preimage_owner() == ActorRole::Taker)
            .then(|| taker_paths.claim_preimage.clone()),
        taker_runtime,
        zebra_genesis,
        discovery,
        taker_candidates,
    ))
    .context("failed to encode taker actor config")?;
    write_private_new(&maker_paths.config, &maker_config)?;
    write_private_new(&taker_paths.config, &taker_config)?;

    let maker = ActorConfig::load_private(&maker_paths.config)
        .context("provisioned maker config did not reload")?;
    let taker = ActorConfig::load_private(&taker_paths.config)
        .context("provisioned taker config did not reload")?;
    ensure!(
        maker.bridge_runtime().escrow_program_id == spec.lez_runtime.escrow_program_id
            && taker.bridge_runtime().escrow_program_id == spec.lez_runtime.escrow_program_id,
        "provisioned actor runtime changed the checked escrow program"
    );
    validate_actor_pair(&maker, &taker).context("provisioned actor pair is not isolated")?;
    let _maker_material = maker
        .load_activate_material()
        .context("provisioned maker activation material is invalid")?;
    let _taker_material = taker
        .load_activate_material()
        .context("provisioned taker activation material is invalid")?;

    Ok(LocalPocProvisionSummary {
        schema_version: 1,
        run_id: spec.run_id,
        direction: spec.direction.as_str(),
        agreement_file,
        signed_agreement_sha256: fixture.sha256,
        maker_zcash_public_key: hex::encode(fixture.maker_zcash_public_key),
        taker_zcash_public_key: hex::encode(fixture.taker_zcash_public_key),
        authenticated_transfer_program_id: Hex32::from_bytes(authenticated_transfer),
        authenticated_transfer_program_id_words: program_id_words(Hex32::from_bytes(
            authenticated_transfer,
        )),
        maker: role_summary(&maker_paths),
        taker: role_summary(&taker_paths),
        zebra_tip_height: tip_height,
        zcash_refund_height: fixture.zcash_refund_height,
        zcash_candidate_height: (spec.zebra_route.network() == NetworkType::Test)
            .then_some(candidate.height),
        zcash_candidate_owner: match spec.direction.zcash_funder() {
            ActorRole::Maker => "maker",
            ActorRole::Taker => "taker",
        },
        lez_native_amount: LEZ_NATIVE_AMOUNT,
        lez_depositor_role: match spec.direction.lez_depositor() {
            ActorRole::Maker => "maker",
            ActorRole::Taker => "taker",
        },
        lez_depositor_account_id_base58: match spec.direction.lez_depositor() {
            ActorRole::Maker => spec.lez_runtime.maker_signer_account_id_base58.clone(),
            ActorRole::Taker => spec.lez_runtime.taker_signer_account_id_base58.clone(),
        },
        private_material_disclosed: false,
        actor_pair_validated: true,
    })
}

/// Rebinds an already validated local chain-fact template to one Delivery offer
/// and Chat reservation without either participant's signing authority.
///
/// # Errors
///
/// Fails closed for unsafe input/output files, an invalid or expired template,
/// invalid transcript expiry, or a draft that changes executable chain facts.
pub fn prepare_local_v0_2_chat_draft(
    source_agreement_file: &Path,
    now: UnixSeconds,
    reservation_id: RequestId,
    offer_commitment: Hex32,
    offer_expires_at_unix_seconds: u64,
    output_file: &Path,
) -> Result<LocalPocChatDraftSummary> {
    ensure!(
        offer_expires_at_unix_seconds > now.value(),
        "Delivery offer is already expired"
    );
    let (source_wire, _) = read_bounded_identified(
        source_agreement_file,
        MAX_ZEC_AGREEMENT_RECORD_BYTES,
        FilePrivacy::Public,
    )
    .map_err(|_| anyhow::anyhow!("source agreement is unavailable or unsafe"))?;
    let source = ZecAgreementV1::from_wire_at(source_wire.as_slice(), now)
        .context("source agreement is not a valid chain-fact template")?;
    let transcript = NegotiationTranscriptV1::new(
        maker_zec_chat_session_id(&reservation_id),
        *offer_commitment.as_bytes(),
        offer_expires_at_unix_seconds,
    );
    let draft = ZecAgreementDraftV1::rebind_validated_transcript(&source, transcript);
    let wire = Zeroizing::new(draft.encode_wire().context("failed to encode Chat draft")?);
    let _validated = ZecAgreementDraftV1::from_wire_at(wire.as_slice(), now)
        .context("prepared Chat draft did not revalidate")?;
    let digest: [u8; 32] = Sha256::digest(wire.as_slice()).into();
    write_private_new(output_file, wire.as_slice())?;
    Ok(LocalPocChatDraftSummary {
        schema_version: 1,
        draft_file: output_file.to_path_buf(),
        draft_sha256: Hex32::from_bytes(digest),
        swap_id: source.application_swap_id().to_owned(),
        maker_public_key: hex::encode(source.zcash_key(Participant::Maker).serialize()),
        reservation_id,
        offer_commitment,
        offer_expires_at_unix_seconds,
        private_material_disclosed: false,
    })
}

/// Validates a real Chat result against its chain-fact template and produces a
/// fresh isolated pair of role configurations and mutable state locations.
///
/// Existing authority files remain referenced in place and are never copied or
/// returned. Every executable agreement field except the authenticated Chat
/// transcript must match the validated source; both role keys and the funder's
/// hash preimage are checked against the final countersigned record.
///
/// # Errors
///
/// Fails closed for unsafe files, invalid actor pairs, expired or mismatched
/// agreements, incorrect role authority, changed chain facts, output collision,
/// or a generated config that cannot be securely reloaded and activated.
#[allow(clippy::too_many_lines, clippy::similar_names)]
pub fn finalize_local_v0_2_chat_corridor(
    source_maker_config_file: &Path,
    source_taker_config_file: &Path,
    final_agreement_file: &Path,
    accepted_at: UnixSeconds,
    output_root: &Path,
) -> Result<LocalPocChatFinalizeSummary> {
    validate_new_output_root(output_root)?;
    let source_maker = ActorConfig::load_private(source_maker_config_file)
        .context("source maker config is unsafe or invalid")?;
    let source_taker = ActorConfig::load_private(source_taker_config_file)
        .context("source taker config is unsafe or invalid")?;
    ensure!(
        source_maker.role() == ActorRole::Maker && source_taker.role() == ActorRole::Taker,
        "source configs do not represent maker and taker"
    );
    validate_actor_pair(&source_maker, &source_taker)
        .context("source actor pair is not isolated")?;
    let maker_material = source_maker
        .load_activate_material()
        .context("source maker activation material is invalid")?;
    let taker_material = source_taker
        .load_activate_material()
        .context("source taker activation material is invalid")?;
    ensure!(
        maker_material.signed_agreement_wire() == taker_material.signed_agreement_wire(),
        "source actors do not share the exact agreement"
    );
    let source_agreement =
        ZecAgreementV1::from_wire_at(maker_material.signed_agreement_wire(), accepted_at)
            .context("source agreement is invalid at final acceptance")?;

    let (final_wire, _) = read_bounded_identified(
        final_agreement_file,
        MAX_ZEC_AGREEMENT_RECORD_BYTES,
        FilePrivacy::OwnerPrivate,
    )
    .map_err(|_| anyhow::anyhow!("final Chat agreement is unavailable or unsafe"))?;
    let accepted_maker = AcceptedZecAgreementV1::accept_wire_at(
        final_wire.as_slice(),
        accepted_at,
        Participant::Maker,
        0,
    )
    .context("final Chat agreement is invalid for maker")?;
    let accepted_taker = AcceptedZecAgreementV1::accept_wire_at(
        final_wire.as_slice(),
        accepted_at,
        Participant::Taker,
        0,
    )
    .context("final Chat agreement is invalid for taker")?;
    ensure!(
        accepted_maker.agreement() == accepted_taker.agreement(),
        "roles accepted different final agreements"
    );
    let final_agreement = accepted_maker.agreement();
    ensure!(
        final_agreement.application_swap_id() == source_maker.swap_id().as_str()
            && final_agreement.application_swap_id() == source_taker.swap_id().as_str(),
        "final agreement swap ID differs from source actors"
    );
    let expected = ZecAgreementDraftV1::rebind_validated_transcript(
        &source_agreement,
        *final_agreement.transcript(),
    );
    let expected_wire = expected
        .encode_wire()
        .context("failed to encode expected Chat body")?;
    let expected = ZecAgreementDraftV1::from_wire_at(&expected_wire, accepted_at)
        .context("expected Chat body did not revalidate")?;
    ensure!(
        expected.body() == final_agreement.record().body(),
        "final agreement changed executable chain facts outside the Chat transcript"
    );
    validate_role_authority(
        &maker_material,
        final_agreement,
        Participant::Maker,
        source_maker.is_local_zcash_funder(),
    )?;
    validate_role_authority(
        &taker_material,
        final_agreement,
        Participant::Taker,
        source_taker.is_local_zcash_funder(),
    )?;

    create_private_directory(output_root)?;
    let shared_root = output_root.join("shared");
    let maker_root = output_root.join("maker");
    let taker_root = output_root.join("taker");
    let maker_state = maker_root.join("state");
    let taker_state = taker_root.join("state");
    for directory in [
        &shared_root,
        &maker_root,
        &taker_root,
        &maker_state,
        &taker_state,
    ] {
        create_private_directory(directory)?;
    }
    let agreement_file = shared_root.join("agreement-v2.borsh");
    write_private_new(&agreement_file, final_wire.as_slice())?;
    let agreement_digest: [u8; 32] = Sha256::digest(final_wire.as_slice()).into();
    let agreement_sha256 = Hex32::from_bytes(agreement_digest);
    let swap_id = SwapId::new(final_agreement.application_swap_id().to_owned())
        .context("final application swap ID is invalid")?;
    let maker_config_file = maker_root.join("actor-config.json");
    let taker_config_file = taker_root.join("actor-config.json");
    let maker_config = encode_rebound_local_v0_2_actor_config(
        &source_maker,
        swap_id.clone(),
        agreement_file.clone(),
        agreement_sha256,
        maker_state.join("actor.sqlite3"),
        maker_state.join("bridge.sqlite3"),
    )
    .context("failed to encode rebound maker config")?;
    let taker_config = encode_rebound_local_v0_2_actor_config(
        &source_taker,
        swap_id,
        agreement_file.clone(),
        agreement_sha256,
        taker_state.join("actor.sqlite3"),
        taker_state.join("bridge.sqlite3"),
    )
    .context("failed to encode rebound taker config")?;
    write_private_new(&maker_config_file, &maker_config)?;
    write_private_new(&taker_config_file, &taker_config)?;
    let maker = ActorConfig::load_private(&maker_config_file)
        .context("rebound maker config did not reload")?;
    let taker = ActorConfig::load_private(&taker_config_file)
        .context("rebound taker config did not reload")?;
    validate_actor_pair(&maker, &taker).context("rebound actor pair is not isolated")?;
    let _maker_activation = maker
        .load_activate_material()
        .context("rebound maker activation material is invalid")?;
    let _taker_activation = taker
        .load_activate_material()
        .context("rebound taker activation material is invalid")?;

    Ok(LocalPocChatFinalizeSummary {
        schema_version: 1,
        run_id: maker.run_id().clone(),
        swap_id: final_agreement.application_swap_id().to_owned(),
        agreement_file,
        signed_agreement_sha256: agreement_sha256,
        maker_config_file,
        taker_config_file,
        maker_state_root: maker_state,
        taker_state_root: taker_state,
        private_material_disclosed: false,
        actor_pair_validated: true,
    })
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
    let expected_funder = agreement.lez_claimant() == participant;
    ensure!(
        configured_as_funder == expected_funder,
        "actor funder role does not match the final agreement"
    );
    match material.claim_preimage() {
        Some(preimage) if expected_funder => {
            let digest: [u8; 32] = Sha256::digest(preimage.expose_secret()).into();
            ensure!(
                digest == *agreement.secret_digest(),
                "actor preimage does not match the final agreement"
            );
        }
        None if !expected_funder => {}
        _ => bail!("actor preimage ownership does not match the final agreement"),
    }
    Ok(())
}

pub(crate) struct MaterialPaths {
    pub(crate) state_root: PathBuf,
    pub(crate) config: PathBuf,
    pub(crate) runtime: PathBuf,
    pub(crate) role_state_db: PathBuf,
    pub(crate) claim_recovery_key: PathBuf,
    pub(crate) claim_preimage: PathBuf,
    pub(crate) zcash_key: PathBuf,
    pub(crate) lez_signer_key: PathBuf,
    pub(crate) bridge_journal_db: PathBuf,
    pub(crate) capability: PathBuf,
    pub(crate) zebra_credential: PathBuf,
}

pub(crate) fn material_paths(root: &Path, state_root: &Path) -> MaterialPaths {
    MaterialPaths {
        state_root: state_root.to_path_buf(),
        config: root.join("actor-config.json"),
        runtime: root.join("lez-runtime.json"),
        role_state_db: state_root.join("actor.sqlite3"),
        claim_recovery_key: root.join("claim-recovery.key"),
        claim_preimage: root.join("claim-preimage.key"),
        zcash_key: root.join("zcash.key"),
        lez_signer_key: root.join("lez-signer.key"),
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
}

impl<'a> RoleCorridor<'a> {
    fn of_spec(spec: &'a ProvisionSpec, role: ActorRole) -> Self {
        Self {
            run_id: &spec.run_id,
            bridge_endpoint: match role {
                ActorRole::Maker => &spec.bridge.maker_endpoint,
                ActorRole::Taker => &spec.bridge.taker_endpoint,
            },
            zebra_endpoint: &spec.zebra_endpoint,
            zebra_route: &spec.zebra_route,
            profile: spec.profile.id(),
        }
    }
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
            LocalPocZebraRoute::DeterministicLocal => ProvisionedZebraRoute::DeterministicLocal,
            LocalPocZebraRoute::SelfHostedCookie { .. } => {
                ProvisionedZebraRoute::SelfHostedTestnetCookie {
                    cookie_file: paths.zebra_credential.clone(),
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

fn runtime_descriptor(
    spec: &ProvisionSpec,
    role: BridgeParticipant,
    signer_account_id: Hex32,
) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::LeeV0_2_0,
        spec.lez_runtime.chain_id,
        spec.lez_runtime.channel_id,
        spec.lez_runtime.genesis_block_hash,
        spec.lez_runtime.escrow_program_id,
        signer_account_id,
    )
}

fn role_summary(paths: &MaterialPaths) -> RolePaths {
    RolePaths {
        state_root: paths.state_root.clone(),
        config_file: paths.config.clone(),
        runtime_file: paths.runtime.clone(),
        lez_signer_key_file: paths.lez_signer_key.clone(),
        sidecar_capability_file: paths.capability.clone(),
    }
}

fn load_spec(path: &Path) -> Result<ProvisionSpec> {
    let (bytes, _) = read_bounded_identified(path, MAX_SPEC_BYTES, FilePrivacy::OwnerPrivate)
        .map_err(|_| anyhow::anyhow!("local PoC spec is unavailable or unsafe"))?;
    serde_json::from_slice(bytes.as_slice()).context("local PoC spec is invalid")
}

fn load_lez_signer_material(
    signer_files: Option<&LocalPocLezSignerFiles>,
    maker_account_id: &str,
    taker_account_id: &str,
) -> Result<LezSignerMaterial> {
    let (maker, taker) = if let Some(signer_files) = signer_files {
        for path in [&signer_files.maker, &signer_files.taker] {
            ensure!(path.is_absolute(), "LEZ signer path must be absolute");
            ensure!(
                path.components().all(|component| matches!(
                    component,
                    Component::RootDir | Component::Normal(_)
                )),
                "LEZ signer path must be normalized"
            );
            ensure!(
                fs::canonicalize(path).context("LEZ signer path is unavailable")? == path.as_path(),
                "LEZ signer path must be canonical"
            );
        }
        let (maker, maker_location) = read_bounded_identified(
            &signer_files.maker,
            MAX_LEZ_SIGNER_KEY_BYTES,
            FilePrivacy::OwnerPrivate,
        )
        .map_err(|_| anyhow::anyhow!("Maker LEZ signer file is unavailable or unsafe"))?;
        let (taker, taker_location) = read_bounded_identified(
            &signer_files.taker,
            MAX_LEZ_SIGNER_KEY_BYTES,
            FilePrivacy::OwnerPrivate,
        )
        .map_err(|_| anyhow::anyhow!("Taker LEZ signer file is unavailable or unsafe"))?;
        ensure!(
            !maker_location.aliases(&taker_location),
            "Maker and Taker LEZ signer files must not alias"
        );
        validate_external_lez_signer_encoding(maker.as_slice())?;
        validate_external_lez_signer_encoding(taker.as_slice())?;
        (maker, taker)
    } else {
        (
            Zeroizing::new(hex::encode([1_u8; 32]).into_bytes()),
            Zeroizing::new(hex::encode([2_u8; 32]).into_bytes()),
        )
    };

    let maker_account = derive_lez_signer_account(maker.as_slice())?;
    let taker_account = derive_lez_signer_account(taker.as_slice())?;
    ensure!(
        maker_account != taker_account,
        "Maker and Taker LEZ signers must differ"
    );
    ensure!(
        bs58::encode(maker_account).into_string() == maker_account_id,
        "Maker LEZ signer does not match the configured account"
    );
    ensure!(
        bs58::encode(taker_account).into_string() == taker_account_id,
        "Taker LEZ signer does not match the configured account"
    );
    Ok(LezSignerMaterial { maker, taker })
}

fn validate_external_lez_signer_encoding(bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() == MAX_LEZ_SIGNER_KEY_BYTES
            && bytes.ends_with(b"\n")
            && bytes[..64]
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b"a"[0]..=b"f"[0]).contains(byte)),
        "external LEZ signer must be 64 lowercase hexadecimal characters plus newline"
    );
    Ok(())
}

fn derive_lez_signer_account(bytes: &[u8]) -> Result<[u8; 32]> {
    let encoded = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    ensure!(
        encoded.len() == 64
            && encoded
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b"a"[0]..=b"f"[0]).contains(byte)),
        "LEZ signer must be exactly 64 lowercase hexadecimal characters"
    );
    let mut scalar = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(encoded, scalar.as_mut())
        .context("LEZ signer must be lowercase hexadecimal")?;
    let mut secret = SecretKey::from_slice(scalar.as_ref()).context("invalid LEZ signer scalar")?;
    let public = PublicKey::from_secret_key(&Secp256k1::signing_only(), &secret);
    secret.non_secure_erase();
    let x_only = public.x_only_public_key().0.serialize();
    Ok(derive_lez_public_account_v0_2(&x_only))
}

fn validate_spec(spec: &ProvisionSpec) -> Result<()> {
    ensure!(
        spec.schema_version == SPEC_SCHEMA_VERSION,
        "unsupported local PoC spec schema"
    );
    // The authenticated-transfer program is whatever the chain in front of us
    // deployed; the spec names it from the same manifest the Nodes read. It
    // was once pinned to the retained v0.2.0 fixture, which refused every
    // newer local chain (the v0.2.4 devnet ships `fe96c4...`, not `dcbbfe...`).
    // The base58 value is still decoded and bound into the agreement below.
    ensure!(
        [
            spec.lez_runtime.chain_id,
            spec.lez_runtime.channel_id,
            spec.lez_runtime.genesis_block_hash,
            spec.lez_runtime.escrow_program_id,
        ]
        .into_iter()
        .all(|value| value.as_bytes() != &[0; 32]),
        "LEZ runtime identity contains a zero value"
    );
    ensure!(
        spec.lez_runtime.chain_id == spec.lez_runtime.channel_id,
        "local v0.2 chain ID must equal the channel because upstream exposes no chain-ID RPC"
    );
    ensure!(
        spec.bridge.maker_endpoint != spec.bridge.taker_endpoint
            && spec.bridge.maker_endpoint != spec.zebra_endpoint
            && spec.bridge.taker_endpoint != spec.zebra_endpoint,
        "maker, taker, and Zebra endpoints must be distinct"
    );
    ensure!(
        ZecRefundProfile::for_id(spec.profile.id()).zcash_network() == spec.zebra_route.network(),
        "the profile's Zcash network is not the Zebra route's"
    );
    ensure!(
        spec.zcash_principal() > 0,
        "the Zcash principal must be positive"
    );
    Ok(())
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
    let candidate = selected.with_context(|| match identity.network() {
        NetworkType::Regtest => format!(
            "no mature deterministic Zebra funder output is available after passing over {skip}"
        ),
        NetworkType::Test | NetworkType::Main => format!(
            "no unspent funder output of at least {min_value} zatoshis, {} blocks deep, \
             from height {scan_from_height} after passing over {skip}",
            chain.min_confirmations
        ),
    })?;
    Ok((candidate, tip_height, genesis))
}

#[allow(clippy::too_many_arguments)]
fn build_agreement(
    spec: &ProvisionSpec,
    zcash_keys: &ZcashKeyMaterial,
    swap_id: &SwapId,
    maker_lez_account: [u8; 32],
    taker_lez_account: [u8; 32],
    authenticated_transfer: [u8; 32],
    candidate: &SelectedCandidate,
    zebra_tip_height: u32,
) -> Result<AgreementFixture> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock precedes Unix epoch")?
        .as_secs();
    let maker_bytes = zcash_keys.maker.clone();
    let taker_bytes = zcash_keys.taker.clone();
    let mut maker_secret = SecretKey::from_slice(maker_bytes.as_ref())?;
    let mut taker_secret = SecretKey::from_slice(taker_bytes.as_ref())?;
    let secp = Secp256k1::signing_only();
    let maker_public = PublicKey::from_secret_key(&secp, &maker_secret);
    let taker_public = PublicKey::from_secret_key(&secp, &taker_secret);
    // One secret per swap. Fixture swaps taken together share keys, amounts
    // and (at one anchor) the refund height; a shared preimage made their
    // BIP-199 scripts identical, and the second swap's Taker discovered the
    // first swap's funding as its Maker lock, then tried to spend an output
    // already claimed (zec-concurrent, chain 37, 2026-10-29).
    let preimage = fresh_claim_preimage()?;
    let facts = AgreementFacts {
        profile: spec.profile.id(),
        direction: spec.direction.protocol(),
        swap_id: swap_id.clone(),
        maker_lez_account,
        taker_lez_account,
        maker_zcash_key: maker_public,
        taker_zcash_key: taker_public,
        secret_digest: Sha256::digest(&preimage[..]).into(),
        lez: LezChainFacts {
            channel_id: *spec.lez_runtime.channel_id.as_bytes(),
            genesis_block_hash: *spec.lez_runtime.genesis_block_hash.as_bytes(),
            escrow_program_id: spec.lez_runtime.escrow_program_id,
            authenticated_transfer_program_id: authenticated_transfer,
        },
        lez_amount: LEZ_NATIVE_AMOUNT,
        zcash_principal: spec.zcash_principal(),
        funding_input: candidate.funding_input(),
        zebra_tip_height,
        now,
        transcript: NegotiationTranscriptV1::new(
            labeled_hash(spec.run_id.as_str(), b"session"),
            labeled_hash(spec.run_id.as_str(), b"offer"),
            now.checked_add(AGREEMENT_LIFETIME_SECONDS)
                .context("agreement expiry overflow")?,
        ),
    };
    let (body, zcash_refund_height) = compose_agreement_body(&facts)?;
    let wire = sign_agreement_body(body, &maker_secret, &taker_secret, now)?;
    maker_secret.non_secure_erase();
    taker_secret.non_secure_erase();
    let sha256 = Hex32::from_bytes(Sha256::digest(wire.as_slice()).into());
    Ok(AgreementFixture {
        wire,
        sha256,
        maker_zcash_public_key: maker_public.serialize(),
        taker_zcash_public_key: taker_public.serialize(),
        maker_zcash_key: maker_bytes,
        taker_zcash_key: taker_bytes,
        preimage,
        zcash_candidate: CandidateOutpoint::new(
            candidate.rpc_transaction_id,
            candidate.outpoint.n(),
        ),
        zcash_refund_height,
    })
}

impl SelectedCandidate {
    fn funding_input(&self) -> ZcashFundingInputV1 {
        ZcashFundingInputV1::new(
            *self.outpoint.hash(),
            self.outpoint.n(),
            u64::from(self.output.value()),
            self.output.script_pubkey().0.0.clone(),
        )
    }
}

/// The LEZ chain a Zcash agreement settles against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LezChainFacts {
    pub(crate) channel_id: [u8; 32],
    pub(crate) genesis_block_hash: [u8; 32],
    pub(crate) escrow_program_id: Hex32,
    pub(crate) authenticated_transfer_program_id: [u8; 32],
}

/// Every public fact one Zcash agreement body is composed from. The dealer
/// (`zec-local-poc-provision`) and the two roles of a take-time reservation
/// (`zec_reserve_v1`) compose the same body from the same facts, so each
/// role can recompute what the other sent.
#[derive(Clone, Debug)]
pub(crate) struct AgreementFacts {
    pub(crate) profile: ZecProfileId,
    pub(crate) direction: SwapDirection,
    pub(crate) swap_id: SwapId,
    pub(crate) maker_lez_account: [u8; 32],
    pub(crate) taker_lez_account: [u8; 32],
    pub(crate) maker_zcash_key: PublicKey,
    pub(crate) taker_zcash_key: PublicKey,
    /// SHA-256 of the Zcash funder's claim preimage.
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

/// The role that funds Zcash (and owns the claim preimage) in `direction`.
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
    let earlier_refund_latest_ms = lez_refund_seconds
        .checked_mul(1_000)
        .context("LEZ refund millisecond overflow")?;
    let later_refund_earliest = match profile.zcash_network() {
        // The fixture's harness mines every block: the bound is the schedule's own.
        NetworkType::Regtest => lez_refund_seconds
            .checked_add(profile.required_margin().value())
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
                earliest >= lez_refund_seconds.saturating_add(profile.required_margin().value()),
                "the profile's Zcash refund window is shorter than its LEZ refund delay \
                 plus margin at Testnet's fastest pace"
            );
            earliest
        }
    };
    let maker_hash = public_key_hash(&facts.maker_zcash_key);
    let taker_hash = public_key_hash(&facts.taker_zcash_key);
    let (zcash_funder_hash, zcash_claimant_hash) = match zcash_funder(facts.direction) {
        ActorRole::Maker => (maker_hash, taker_hash),
        ActorRole::Taker => (taker_hash, maker_hash),
    };
    let contract = Bip199Contract::new(
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
/// `now`: the dealer's one-step countersignature.
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

pub(crate) fn public_key_hash(key: &PublicKey) -> [u8; 20] {
    match lez_zec_swap_sdk::p2pkh_address(key) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("a public key always maps to P2PKH"),
    }
}

fn sign_agreement(commitment: [u8; 32], key: &SecretKey) -> [u8; 64] {
    Secp256k1::signing_only()
        .sign_ecdsa(&Message::from_digest(commitment), key)
        .serialize_compact()
}

fn labeled_hash(run_id: &str, label: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"logos.gateway.local-poc.v1\0");
    hasher.update(run_id.as_bytes());
    hasher.update([0]);
    hasher.update(label);
    hasher.finalize().into()
}

fn program_id_words(value: Hex32) -> [u32; 8] {
    let mut words = [0_u32; 8];
    for (word, chunk) in words.iter_mut().zip(value.as_bytes().chunks_exact(4)) {
        *word = u32::from_le_bytes(chunk.try_into().expect("four-byte program word"));
    }
    words
}

fn decode_base58_32(value: &str) -> Result<[u8; 32]> {
    let decoded = bs58::decode(value).into_vec()?;
    decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("base58 value is not 32 bytes"))
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

fn random_capability() -> Result<Zeroizing<Vec<u8>>> {
    let mut random = Zeroizing::new([0_u8; 32]);
    getrandom::fill(&mut random[..])
        .map_err(|_| anyhow::anyhow!("sidecar capability randomness unavailable"))?;
    Ok(Zeroizing::new(hex::encode(&random[..]).into_bytes()))
}

fn avoid_zero_or_equal(maker: &mut [u8; 32], taker: &mut [u8; 32]) {
    if maker.iter().all(|byte| *byte == 0) {
        maker[31] = 1;
    }
    if taker.iter().all(|byte| *byte == 0) || maker == taker {
        taker[31] ^= 1;
    }
}

pub(crate) fn validate_new_output_root(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "output root must be absolute");
    ensure!(
        path.components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_))),
        "output root must be normalized"
    );
    ensure!(!path.exists(), "output root already exists");
    let parent = path.parent().context("output root has no parent")?;
    let canonical_parent = fs::canonicalize(parent).context("output parent is unavailable")?;
    ensure!(canonical_parent == parent, "output parent is not canonical");
    ensure!(
        fs::symlink_metadata(parent)
            .context("output parent is unavailable")?
            .is_dir(),
        "output parent is not a directory"
    );
    Ok(())
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
mod zcash_identity_tests {
    use super::*;

    #[test]
    fn the_test_lane_funder_key_is_the_asset_miner_address() {
        let secret = SecretKey::from_slice(&[FUNDED_ZCASH_KEY_BYTE; 32]).unwrap();
        let public = PublicKey::from_secret_key(&Secp256k1::signing_only(), &secret);
        // tests/e2e/zebra/zebrad.toml (the isolated lane) mines to this address;
        // the devnet mints its own Maker key (deploy/scripts/gen-config.sh).
        assert_eq!(
            regtest_p2pkh_address(&public).unwrap(),
            "tmNAP26Sw5Ra2jepAoTr1kqdkggawba6Akd"
        );
    }
}

#[cfg(test)]
mod signer_tests {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    use tempfile::tempdir;

    use super::{
        LocalPocLezSignerFiles, derive_lez_signer_account, load_lez_signer_material,
        validate_external_lez_signer_encoding,
    };

    const MAKER: &str = "B1UN3hPgxacgHKBRoThcAmsPajGcUf6YXUhgB36x4DAd";
    const TAKER: &str = "34Kqgek6R7N1zU5FSJz8ziXwSPEPCuWGcn1T7GCVrfib";

    fn write_signer(path: &std::path::Path, byte: u8) {
        let mut encoded = hex::encode([byte; 32]);
        encoded.push(char::from(10));
        fs::write(path, encoded).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn account(path: &std::path::Path) -> String {
        let bytes = fs::read(path).unwrap();
        bs58::encode(derive_lez_signer_account(&bytes).unwrap()).into_string()
    }

    #[test]
    fn deterministic_signers_remain_bound_to_official_fixture_accounts() {
        let material = load_lez_signer_material(None, MAKER, TAKER).unwrap();
        assert_eq!(
            material.maker.as_slice(),
            hex::encode([1_u8; 32]).as_bytes()
        );
        assert_eq!(
            material.taker.as_slice(),
            hex::encode([2_u8; 32]).as_bytes()
        );
    }

    #[test]
    fn fresh_signers_are_account_bound_and_preserved_exactly() {
        let root = tempdir().unwrap();
        let maker = root.path().join("maker.key");
        let taker = root.path().join("taker.key");
        write_signer(&maker, 3);
        write_signer(&taker, 4);
        let maker_account = account(&maker);
        let taker_account = account(&taker);
        let files = LocalPocLezSignerFiles::new(maker.clone(), taker.clone());
        let debug = format!("{files:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains(maker.to_str().unwrap()));
        assert!(!debug.contains(taker.to_str().unwrap()));

        let material =
            load_lez_signer_material(Some(&files), &maker_account, &taker_account).unwrap();
        assert_eq!(material.maker.as_slice(), fs::read(maker).unwrap());
        assert_eq!(material.taker.as_slice(), fs::read(taker).unwrap());
    }

    #[test]
    fn fresh_signers_reject_cross_wire_alias_and_unsafe_encoding() {
        let root = tempdir().unwrap();
        let maker = root.path().join("maker.key");
        let taker = root.path().join("taker.key");
        write_signer(&maker, 3);
        write_signer(&taker, 4);
        let maker_account = account(&maker);
        let taker_account = account(&taker);
        let files = LocalPocLezSignerFiles::new(maker.clone(), taker.clone());

        assert!(load_lez_signer_material(Some(&files), &taker_account, &maker_account).is_err());
        let alias = LocalPocLezSignerFiles::new(maker.clone(), maker.clone());
        assert!(load_lez_signer_material(Some(&alias), &maker_account, &maker_account).is_err());

        fs::set_permissions(&maker, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(load_lez_signer_material(Some(&files), &maker_account, &taker_account).is_err());
        fs::set_permissions(&maker, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&maker, hex::encode([3_u8; 32])).unwrap();
        assert!(load_lez_signer_material(Some(&files), &maker_account, &taker_account).is_err());
        assert!(validate_external_lez_signer_encoding(&[0_u8; 65]).is_err());
    }

    #[test]
    fn fresh_signers_reject_zero_and_out_of_range_scalars() {
        let mut zero = hex::encode([0_u8; 32]).into_bytes();
        zero.push(10);
        assert!(validate_external_lez_signer_encoding(&zero).is_ok());
        assert!(derive_lez_signer_account(&zero).is_err());

        let mut overflow = vec![b'f'; 64];
        overflow.push(10);
        assert!(validate_external_lez_signer_encoding(&overflow).is_ok());
        assert!(derive_lez_signer_account(&overflow).is_err());
    }
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

    fn spec(profile: &str, route: &serde_json::Value) -> ProvisionSpec {
        serde_json::from_value(serde_json::json!({
            "schema_version": 1, "run_id": "zec-testnet-1", "swap_id": "zec-testnet-swap-1",
            "direction": "taker_sells_lez", "profile": profile,
            "lez_runtime": {
                "chain_id": "01".repeat(32), "channel_id": "01".repeat(32),
                "genesis_block_hash": "02".repeat(32), "escrow_program_id": "03".repeat(32),
                "authenticated_transfer_program_id_base58": "FrexXMbyY6iZjwUo8DV3jfB8donj8H4kLRHT7xswCfJg",
                "maker_signer_account_id_base58": "B1UN3hPgxacgHKBRoThcAmsPajGcUf6YXUhgB36x4DAd",
                "taker_signer_account_id_base58": "34Kqgek6R7N1zU5FSJz8ziXwSPEPCuWGcn1T7GCVrfib"},
            "bridge": {"maker_endpoint": "http://127.0.0.1:19901", "taker_endpoint": "http://127.0.0.1:19902"},
            "zebra_endpoint": "https://zcash-testnet-zebrad.gateway.tatum.io",
            "lez_discovery_start_height": 1, "lez_discovery_max_blocks": 4096,
            "zcash_funding_scan_from_height": 4_600_000,
            "zebra_route": route,
            "zcash_principal_zatoshis": 1_000_000,
        }))
        .unwrap()
    }

    #[test]
    fn a_testnet_spec_names_its_route_and_the_profile_must_agree() {
        let tatum = serde_json::json!({"kind": "tatum_testnet_x_api_key", "api_key_file": "/run/secrets/tatum.key"});
        let testnet = spec("public-testnet-v2", &tatum);
        validate_spec(&testnet).unwrap();
        assert_eq!(testnet.zcash_principal(), 1_000_000);
        // A devnet profile on a Testnet route, or the Testnet profile on the fixture, is refused.
        assert!(validate_spec(&spec("deterministic-local-devnet-v3", &tatum)).is_err());
        assert!(
            validate_spec(&spec(
                "public-testnet-v2",
                &serde_json::json!({"kind": "deterministic_local"})
            ))
            .is_err()
        );
        // Only V2 is a Testnet profile a corridor may be provisioned under.
        assert!(
            serde_json::from_value::<LocalPocProfile>(serde_json::json!("public-testnet-v1"))
                .is_err()
        );
    }

    fn tatum() -> serde_json::Value {
        serde_json::json!({"kind": "tatum_testnet_x_api_key", "api_key_file": "/run/secrets/tatum.key"})
    }

    #[test]
    fn a_testnet_corridor_agrees_on_nu7_the_lez_testnet_and_v2_deadlines() {
        let spec = spec("public-testnet-v2", &tatum());
        let keys = ZcashKeyMaterial::deterministic(spec.direction);
        let swap_id = SwapId::new(spec.swap_id.clone()).unwrap();
        let mut zebra = FakeTestnet::new();
        zebra.unspent.insert((zebra.txid(), 2), 12);
        let candidate = SelectedCandidate {
            outpoint: OutPoint::new(*zebra.txid().as_ref(), 2),
            output: TxOut::new(
                Zatoshis::from_u64(712_420_000).unwrap(),
                lez_zec_swap_sdk::p2pkh_address(&spender()).script().into(),
            ),
            rpc_transaction_id: txid_display_hex(zebra.txid()),
            height: NU7_TX_HEIGHT,
        };
        let tip = 4_607_100;
        let fixture = build_agreement(
            &spec, &keys, &swap_id, [7; 32], [8; 32], [9; 32], &candidate, tip,
        )
        .unwrap();
        assert_eq!(fixture.zcash_refund_height, tip + 1 + 3_700);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let agreement = ZecAgreementV1::from_wire_at(&fixture.wire, UnixSeconds::new(now)).unwrap();
        let output = agreement.binding().expected_output();
        assert_eq!(
            agreement.binding().profile_id(),
            ZecProfileId::PublicTestnetV2
        );
        assert_eq!(output.network(), NetworkType::Test);
        assert_eq!(
            output.consensus_branch_id(),
            zcash_protocol::consensus::BranchId::Nu7
        );
        assert_eq!(u64::from(output.value()), 1_000_000);
        assert_eq!(output.contract().refund_lock_time(), tip + 1 + 3_700);
        // Validation under V2 accepted the official LEZ testnet environment
        // (agreement_v1.rs validate_lez_chain_profile) and a Zcash refund no
        // earlier than the LEZ refund delay plus margin.
    }

    /// A take-time role writes, for its own role, exactly the actor config
    /// the dealer writes for that role of the same corridor.
    #[test]
    fn a_take_time_template_is_the_dealers_role_config() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(directory.path()).unwrap();
        let role_root = root.join("maker");
        create_private_directory(&role_root).unwrap();
        create_private_directory(&role_root.join("state")).unwrap();
        for (name, bytes) in [
            ("zcash.key", vec![2_u8; 32]),
            ("claim-recovery.key", vec![3_u8; 32]),
            ("draft.borsh", vec![4_u8; 64]),
            (
                "sidecar-capability",
                b"0123456789abcdef0123456789abcdef\n".to_vec(),
            ),
        ] {
            write_private_new(&role_root.join(name), &bytes).unwrap();
        }
        let role_config = root.join("zec-role.json");
        fs::write(
            &role_config,
            br#"{"schema_version":1,"profile":"deterministic-local-devnet-v5","zebra_endpoint":"http://127.0.0.1:18232"}"#,
        )
        .unwrap();
        fs::set_permissions(&role_config, fs::Permissions::from_mode(0o600)).unwrap();
        let mut spec = spec(
            "deterministic-local-devnet-v5",
            &serde_json::json!({"kind": "deterministic_local"}),
        );
        spec.zebra_endpoint = Url::parse("http://127.0.0.1:18232").unwrap();
        spec.run_id = RunId::new("swap-zec-take-time-0001").unwrap();
        let swap_id = SwapId::new("zec-take-time-swap-0001").unwrap();
        let runtime =
            runtime_descriptor(&spec, BridgeParticipant::Maker, Hex32::from_bytes([7; 32]));
        let discovery = DiscoveryWindow::new(38, 512).unwrap();
        let written = crate::reserve::write_zec_role_template(
            &crate::ZecRoleConfigV1::load(&role_config).unwrap(),
            &role_root,
            &crate::ZecRoleTemplate {
                role: ActorRole::Maker,
                run_id: spec.run_id.clone(),
                swap_id: swap_id.clone(),
                bridge_endpoint: spec.bridge.maker_endpoint.to_string(),
                capability_file: role_root.join("sidecar-capability"),
                runtime: runtime.clone(),
                zebra_genesis: Hex32::from_bytes([6; 32]),
                lez_discovery: discovery,
                funding: None,
            },
        )
        .unwrap();
        let mut paths = material_paths(&role_root, &role_root.join("state"));
        paths.capability = role_root.join("sidecar-capability");
        let dealer = encode_deterministic_local_v0_2_actor_config(actor_config_input(
            &RoleCorridor::of_spec(&spec, ActorRole::Maker),
            &swap_id,
            ActorRole::Maker,
            &role_root.join("draft.borsh"),
            Hex32::from_bytes(Sha256::digest([4_u8; 64]).into()),
            &paths,
            None,
            runtime,
            Hex32::from_bytes([6; 32]),
            discovery,
            Vec::new(),
        ))
        .unwrap();
        assert_eq!(fs::read(written).unwrap(), dealer);
    }

    #[test]
    fn a_testnet_actor_config_names_the_route_nu7_and_the_whole_refund_window() {
        let spec = spec("public-testnet-v2", &tatum());
        let root = PathBuf::from("/var/lib/lez/zec/corridor/maker");
        let paths = material_paths(&root, &root.join("state"));
        let runtime =
            runtime_descriptor(&spec, BridgeParticipant::Maker, Hex32::from_bytes([7; 32]));
        let config = encode_deterministic_local_v0_2_actor_config(actor_config_input(
            &RoleCorridor::of_spec(&spec, ActorRole::Maker),
            &SwapId::new(spec.swap_id.clone()).unwrap(),
            ActorRole::Maker,
            Path::new("/var/lib/lez/zec/corridor/shared/agreement-v2.borsh"),
            Hex32::from_bytes([5; 32]),
            &paths,
            None,
            runtime,
            Hex32::from_bytes([6; 32]),
            DiscoveryWindow::new(1, 4096).unwrap(),
            Vec::new(),
        ))
        .unwrap();
        let raw: serde_json::Value = serde_json::from_slice(&config).unwrap();
        assert_eq!(raw["zebra"]["route"]["kind"], "tatum_testnet_x_api_key");
        assert_eq!(
            raw["zebra"]["route"]["endpoint"],
            "https://zcash-testnet-zebrad.gateway.tatum.io/"
        );
        assert_eq!(
            raw["zebra"]["route"]["api_key_file"],
            "/var/lib/lez/zec/corridor/maker/zebra.credential"
        );
        assert_eq!(raw["zebra"]["identity"]["network"], "test");
        assert_eq!(raw["zebra"]["identity"]["consensus_branch_id"], "77190ad9");
        assert_eq!(raw["zebra"]["counterparty_scan_blocks"], 3_701);
    }
}
