use std::{error::Error, time::Duration};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use jsonrpsee::{
    core::{ClientError, client::ClientT},
    rpc_params,
};
use jsonrpsee_http_client::{HeaderMap, HeaderValue, HttpClient, HttpClientBuilder};
use lez_zec_swap_sdk::{MAX_FIRST_LOCK_SUBMISSION_BYTES, ZCASH_MAX_SCRIPT_BYTES};
use serde::Deserialize;
use zcash_encoding::ReverseHex;
use zcash_primitives::block::BlockHash;
use zcash_protocol::{
    TxId,
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_script::script::Code;
use zcash_transparent::{
    address::Script,
    bundle::{OutPoint, TxOut},
};

const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_CONCURRENT_REQUESTS: usize = 8;
const MAX_RPC_BODY_BYTES: u32 = 4_100_000;
const MAX_RPC_ENDPOINT_BYTES: usize = 2_048;
const MAX_PUBLIC_API_KEY_BYTES: usize = 1_024;
const TATUM_TESTNET_ZEBRA_ENDPOINT: &str = "https://zcash-testnet-zebrad.gateway.tatum.io";
const TRANSACTION_NOT_FOUND_CODE: i32 = -5;
const MAX_DISCOVERY_TRANSACTION_IDS: usize = 50_000;

/// Zebra's BIP-70-compatible chain spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZebraRpcChain {
    /// Zcash mainnet.
    Main,
    /// Zcash testnet or Regtest. Regtest is distinguished by genesis hash.
    Test,
}

impl ZebraRpcChain {
    fn parse(value: &str) -> Result<Self, HttpZebraRpcError> {
        match value {
            "main" => Ok(Self::Main),
            "test" => Ok(Self::Test),
            _ => Err(HttpZebraRpcError::UnknownChain(value.to_owned())),
        }
    }
}

/// Invalid immutable Zebra chain configuration.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ZebraIdentityError {
    /// Mainnet must use main; testnet and Regtest must use test.
    #[error("configured Zebra RPC chain does not match the Zcash network")]
    RpcChainMismatch,
    /// A zero genesis hash cannot identify a chain.
    #[error("configured Zebra genesis hash is zero")]
    ZeroGenesisHash,
}

/// Immutable network, RPC-chain, branch, and genesis binding for one adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZebraChainIdentity {
    network: NetworkType,
    rpc_chain: ZebraRpcChain,
    consensus_branch_id: BranchId,
    genesis_hash: BlockHash,
}

impl ZebraChainIdentity {
    /// Validates an explicit chain identity before any RPC is attempted.
    ///
    /// # Errors
    ///
    /// Rejects a BIP-70 chain/network mismatch or zero genesis hash.
    pub fn new(
        network: NetworkType,
        rpc_chain: ZebraRpcChain,
        consensus_branch_id: BranchId,
        genesis_hash: BlockHash,
    ) -> Result<Self, ZebraIdentityError> {
        let expected_rpc_chain = match network {
            NetworkType::Main => ZebraRpcChain::Main,
            NetworkType::Test | NetworkType::Regtest => ZebraRpcChain::Test,
        };
        if rpc_chain != expected_rpc_chain {
            return Err(ZebraIdentityError::RpcChainMismatch);
        }
        if genesis_hash == BlockHash([0; 32]) {
            return Err(ZebraIdentityError::ZeroGenesisHash);
        }
        Ok(Self {
            network,
            rpc_chain,
            consensus_branch_id,
            genesis_hash,
        })
    }

    /// Exact identity of the pinned deterministic Zebra Regtest fixture.
    #[must_use]
    pub const fn deterministic_regtest_nu6_2() -> Self {
        Self {
            network: NetworkType::Regtest,
            rpc_chain: ZebraRpcChain::Test,
            consensus_branch_id: BranchId::Nu6_2,
            genesis_hash: BlockHash([
                0x27, 0xe3, 0x01, 0x34, 0xd6, 0x20, 0xe9, 0xfe, 0x61, 0xf7, 0x19, 0x93, 0x83, 0x20,
                0xba, 0xb6, 0x3e, 0x7e, 0x72, 0xc9, 0x1b, 0x5e, 0x23, 0x02, 0x56, 0x76, 0xf9, 0x0e,
                0xd8, 0x11, 0x9f, 0x02,
            ]),
        }
    }

    /// Configured Zcash network.
    #[must_use]
    pub const fn network(self) -> NetworkType {
        self.network
    }

    /// Configured Zebra RPC chain spelling.
    #[must_use]
    pub const fn rpc_chain(self) -> ZebraRpcChain {
        self.rpc_chain
    }

    /// Configured transaction consensus branch.
    #[must_use]
    pub const fn consensus_branch_id(self) -> BranchId {
        self.consensus_branch_id
    }

    /// Configured network genesis block hash.
    #[must_use]
    pub const fn genesis_hash(self) -> BlockHash {
        self.genesis_hash
    }
}

/// One typed getblockchaininfo sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZebraChainInfo {
    rpc_chain: ZebraRpcChain,
    tip_height: BlockHeight,
    tip_hash: BlockHash,
    consensus_branch_id: BranchId,
}

impl ZebraChainInfo {
    /// Builds one primitive chain sample, primarily for typed RPC implementations.
    #[must_use]
    pub const fn new(
        rpc_chain: ZebraRpcChain,
        tip_height: BlockHeight,
        tip_hash: BlockHash,
        consensus_branch_id: BranchId,
    ) -> Self {
        Self {
            rpc_chain,
            tip_height,
            tip_hash,
            consensus_branch_id,
        }
    }

    /// RPC chain spelling.
    #[must_use]
    pub const fn rpc_chain(self) -> ZebraRpcChain {
        self.rpc_chain
    }

    /// Best-chain height.
    #[must_use]
    pub const fn tip_height(self) -> BlockHeight {
        self.tip_height
    }

    /// Best-chain hash.
    #[must_use]
    pub const fn tip_hash(self) -> BlockHash {
        self.tip_hash
    }

    /// Consensus branch Zebra reports at the tip.
    #[must_use]
    pub const fn consensus_branch_id(self) -> BranchId {
        self.consensus_branch_id
    }
}

/// Exact transaction identities from one hash-addressed canonical block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZebraCanonicalBlock {
    block_hash: BlockHash,
    block_height: BlockHeight,
    transaction_ids: Box<[TxId]>,
}

impl ZebraCanonicalBlock {
    /// Builds one bounded block inventory, primarily for typed RPC implementations.
    #[must_use]
    pub fn new(
        block_hash: BlockHash,
        block_height: BlockHeight,
        transaction_ids: Vec<TxId>,
    ) -> Self {
        Self {
            block_hash,
            block_height,
            transaction_ids: transaction_ids.into_boxed_slice(),
        }
    }

    /// Hash used to address and identify the block response.
    #[must_use]
    pub const fn block_hash(&self) -> BlockHash {
        self.block_hash
    }

    /// Height claimed by the hash-addressed block response.
    #[must_use]
    pub const fn block_height(&self) -> BlockHeight {
        self.block_height
    }

    /// Transaction identities in canonical block order.
    #[must_use]
    pub fn transaction_ids(&self) -> &[TxId] {
        &self.transaction_ids
    }
}

/// Typed state of an RPC-visible transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ZebraTransactionState {
    /// Transaction is visible but has no complete canonical inclusion context.
    Mempool {
        /// Exact bytes returned by verbose transaction lookup.
        raw_transaction: Vec<u8>,
    },
    /// Transaction has a complete claimed inclusion context for local validation.
    Confirmed {
        /// Exact bytes returned by verbose transaction lookup.
        raw_transaction: Vec<u8>,
        /// Claimed inclusion block hash.
        block_hash: BlockHash,
        /// Claimed inclusion height.
        block_height: BlockHeight,
        /// RPC-reported confirmation depth.
        confirmations: u32,
        /// RPC-reported active-chain flag.
        in_active_chain: bool,
    },
}

/// One exact `gettxout` result and the node view against which it was answered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZebraUnspentOutput {
    best_block: BlockHash,
    confirmations: u32,
    output: TxOut,
}

impl ZebraUnspentOutput {
    /// Builds primitive untrusted UTXO facts, primarily for typed RPC implementations.
    #[must_use]
    pub const fn new(best_block: BlockHash, confirmations: u32, output: TxOut) -> Self {
        Self {
            best_block,
            confirmations,
            output,
        }
    }

    /// Best-chain block against which Zebra answered `gettxout`.
    #[must_use]
    pub const fn best_block(&self) -> BlockHash {
        self.best_block
    }

    /// Confirmation count reported for the unspent output.
    #[must_use]
    pub const fn confirmations(&self) -> u32 {
        self.confirmations
    }

    /// Exact value and script returned for the requested outpoint.
    #[must_use]
    pub const fn output(&self) -> &TxOut {
        &self.output
    }
}

/// Whether a failed broadcast definitely rejected these bytes or left their fate unknown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZebraSubmissionFailure {
    /// Zebra synchronously rejected the exact transaction as invalid or missing inputs.
    DefinitiveRejection,
    /// Transport, timeout, server, or already-known ambiguity requires observation before retry.
    UnknownOutcome,
}

/// Narrow typed Zebra transport boundary used by production chain adapters.
#[async_trait]
pub trait ZebraRpc: Send + Sync {
    /// Structured transport or decoding error.
    type Error: Error + Send + Sync + 'static;

    /// Samples network, branch, and best-chain tip.
    async fn chain_info(&self) -> Result<ZebraChainInfo, Self::Error>;
    /// Resolves the canonical block hash at one height.
    async fn block_hash(&self, height: BlockHeight) -> Result<BlockHash, Self::Error>;
    /// Fetches a bounded transaction-ID inventory for one hash-addressed block.
    async fn canonical_block(
        &self,
        block_hash: BlockHash,
    ) -> Result<ZebraCanonicalBlock, Self::Error>;
    /// Fetches exact transaction bytes from one named block, mapping only code -5 to absence.
    async fn block_transaction(
        &self,
        transaction_id: TxId,
        block_hash: BlockHash,
    ) -> Result<Option<Vec<u8>>, Self::Error>;
    /// Lists a bounded current mempool transaction-ID snapshot.
    async fn mempool_transaction_ids(&self) -> Result<Vec<TxId>, Self::Error>;
    /// Fetches raw bytes, mapping only Zebra RPC code -5 to absence.
    async fn raw_transaction(&self, transaction_id: TxId) -> Result<Option<Vec<u8>>, Self::Error>;
    /// Fetches complete mempool or claimed canonical transaction context.
    async fn transaction_state(
        &self,
        transaction_id: TxId,
    ) -> Result<Option<ZebraTransactionState>, Self::Error>;
    /// Fetches one exact UTXO, mapping JSON `null` only to current-set absence.
    async fn unspent_output(
        &self,
        outpoint: &OutPoint,
    ) -> Result<Option<ZebraUnspentOutput>, Self::Error>;
    /// Broadcasts exact bytes and returns Zebra's parsed transaction identifier.
    async fn send_raw_transaction(&self, transaction: &[u8]) -> Result<TxId, Self::Error>;
    /// Conservatively classifies a broadcast failure without erasing its structured source.
    fn classify_submission_failure(_error: &Self::Error) -> ZebraSubmissionFailure {
        ZebraSubmissionFailure::UnknownOutcome
    }
}

/// Finite transport bounds for a Zebra HTTP JSON-RPC client.
#[derive(Clone, Eq, PartialEq)]
pub struct HttpZebraRpcConfig {
    endpoint: Box<str>,
    request_timeout: Duration,
    max_concurrent_requests: usize,
    mode: HttpZebraRpcMode,
    authorization: Option<HeaderValue>,
    api_key: Option<HeaderValue>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HttpZebraRpcMode {
    LoopbackHttp,
    PublicHttps,
}

impl std::fmt::Debug for HttpZebraRpcConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpZebraRpcConfig")
            .field("request_timeout", &self.request_timeout)
            .field("max_concurrent_requests", &self.max_concurrent_requests)
            .field("mode", &self.mode)
            .field("cookie_auth_enabled", &self.authorization.is_some())
            .field("api_key_auth_enabled", &self.api_key.is_some())
            .finish_non_exhaustive()
    }
}

impl HttpZebraRpcConfig {
    /// Creates configuration with a 30-second timeout and eight concurrent requests.
    #[must_use]
    pub fn new(endpoint: impl Into<Box<str>>) -> Self {
        Self {
            endpoint: endpoint.into(),
            request_timeout: DEFAULT_RPC_TIMEOUT,
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
            mode: HttpZebraRpcMode::LoopbackHttp,
            authorization: None,
            api_key: None,
        }
    }

    /// Creates public-provider configuration with finite default transport bounds.
    ///
    /// The endpoint must be the exact Tatum Testnet Zebrad HTTPS origin root.
    /// URL credentials, paths, queries, fragments, alternate providers, and
    /// explicit ports are rejected. A bounded API key must subsequently be installed with
    /// [`Self::with_public_api_key`] before connecting.
    ///
    /// # Errors
    ///
    /// Rejects any endpoint other than the exact Tatum Testnet Zebrad origin.
    pub fn public_https(endpoint: impl Into<Box<str>>) -> Result<Self, HttpZebraRpcError> {
        let endpoint = endpoint.into();
        if !is_public_https_endpoint(&endpoint) {
            return Err(HttpZebraRpcError::InvalidPublicHttpsEndpoint);
        }
        Ok(Self {
            endpoint,
            request_timeout: DEFAULT_RPC_TIMEOUT,
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
            mode: HttpZebraRpcMode::PublicHttps,
            authorization: None,
            api_key: None,
        })
    }

    /// Adds Zebra cookie-file credentials as a sensitive HTTP Basic Authorization header.
    ///
    /// One trailing LF or CRLF from reading the cookie file is ignored. The credential
    /// must otherwise be bounded visible ASCII in `username:password` form. Neither this
    /// configuration's `Debug` implementation nor the client `Debug` prints the secret.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, control-containing, or delimiter-incomplete credentials.
    pub fn with_cookie_credentials(
        mut self,
        cookie_contents: impl AsRef<[u8]>,
    ) -> Result<Self, HttpZebraRpcError> {
        if self.mode != HttpZebraRpcMode::LoopbackHttp {
            return Err(HttpZebraRpcError::CookieAuthOnPublicEndpoint);
        }
        let raw = cookie_contents.as_ref();
        let credential = raw
            .strip_suffix(b"\r\n")
            .or_else(|| raw.strip_suffix(b"\n"))
            .unwrap_or(raw);
        let delimiter = credential.iter().position(|byte| *byte == b':');
        if credential.is_empty()
            || credential.len() > 1_024
            || delimiter.is_none_or(|index| index == 0 || index + 1 == credential.len())
            || !credential.iter().all(|byte| (0x21..=0x7e).contains(byte))
        {
            return Err(HttpZebraRpcError::InvalidCookieCredentials);
        }
        let encoded = BASE64_STANDARD.encode(credential);
        let mut authorization = HeaderValue::from_bytes(format!("Basic {encoded}").as_bytes())
            .map_err(|_| HttpZebraRpcError::InvalidCookieCredentials)?;
        authorization.set_sensitive(true);
        self.authorization = Some(authorization);
        Ok(self)
    }

    /// Adds a public-provider credential as the fixed sensitive `x-api-key` header.
    ///
    /// The key must be bounded visible ASCII and is never included in this
    /// configuration's `Debug` output. This credential cannot be installed on the
    /// fail-closed loopback HTTP mode created by [`Self::new`].
    ///
    /// # Errors
    ///
    /// Rejects loopback mode or an empty, oversized, or non-visible API key.
    pub fn with_public_api_key(
        mut self,
        api_key: impl AsRef<[u8]>,
    ) -> Result<Self, HttpZebraRpcError> {
        if self.mode != HttpZebraRpcMode::PublicHttps {
            return Err(HttpZebraRpcError::ApiKeyAuthOnLoopbackEndpoint);
        }
        let api_key = api_key.as_ref();
        if api_key.is_empty()
            || api_key.len() > MAX_PUBLIC_API_KEY_BYTES
            || !api_key.iter().all(|byte| (0x21..=0x7e).contains(byte))
        {
            return Err(HttpZebraRpcError::InvalidPublicApiKey);
        }
        let mut header =
            HeaderValue::from_bytes(api_key).map_err(|_| HttpZebraRpcError::InvalidPublicApiKey)?;
        header.set_sensitive(true);
        self.api_key = Some(header);
        Ok(self)
    }

    /// Replaces the finite request timeout.
    #[must_use]
    pub const fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Replaces the finite concurrency bound.
    #[must_use]
    pub const fn with_max_concurrent_requests(mut self, maximum: usize) -> Self {
        self.max_concurrent_requests = maximum;
        self
    }
}

/// Bounded production HTTP implementation of [`ZebraRpc`].
#[derive(Clone)]
pub struct HttpZebraRpc {
    client: HttpClient,
}

impl std::fmt::Debug for HttpZebraRpc {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpZebraRpc")
            .finish_non_exhaustive()
    }
}

/// Structured HTTP transport, response-shape, or bounded-value failure.
#[derive(Debug, thiserror::Error)]
pub enum HttpZebraRpcError {
    /// Endpoint or HTTP client construction failed.
    #[error("failed to construct bounded Zebra HTTP client: {0}")]
    Build(#[source] ClientError),
    /// A JSON-RPC request failed.
    #[error("Zebra JSON-RPC request failed: {0}")]
    Request(#[source] ClientError),
    /// Zebra returned a chain spelling outside its documented finite set.
    #[error("Zebra returned unknown chain {0:?}")]
    UnknownChain(String),
    /// A response hash/transaction identifier was not exact lowercase 64-hex.
    #[error("Zebra returned malformed {field}")]
    MalformedHash {
        /// Response field being decoded.
        field: &'static str,
    },
    /// Zebra returned an unknown or malformed consensus branch identifier.
    #[error("Zebra returned malformed consensus branch identifier")]
    MalformedConsensusBranch,
    /// A numeric response value exceeded the supported u32 domain.
    #[error("Zebra returned out-of-range {field}")]
    OutOfRange {
        /// Response field being converted.
        field: &'static str,
    },
    /// Raw transaction text violated exact lowercase/bounded hex requirements.
    #[error("Zebra returned malformed or oversized raw transaction hex")]
    MalformedTransactionHex,
    /// A hash-addressed block response did not preserve its exact hash.
    #[error("Zebra returned malformed hash-addressed block context")]
    MalformedBlockContext,
    /// A block or mempool transaction inventory exceeded the explicit client bound.
    #[error("Zebra returned too many transaction identities")]
    TooManyTransactionIds,
    /// `gettxout` returned a non-exact, negative, exponential, or out-of-range amount.
    #[error("Zebra returned malformed UTXO amount")]
    MalformedUtxoAmount,
    /// `gettxout` returned an empty, oversized, odd-length, or non-lowercase script.
    #[error("Zebra returned malformed or oversized UTXO script")]
    MalformedUtxoScript,
    /// Verbose transaction context was only partially populated.
    #[error("Zebra returned partial transaction confirmation context")]
    PartialConfirmationContext,
    /// HTTP configuration disabled a required finite bound.
    #[error("Zebra HTTP timeout and concurrency limits must be nonzero")]
    InvalidTransportBounds,
    /// Zebra cookie contents were not bounded `username:password` text.
    #[error("Zebra cookie credentials are invalid")]
    InvalidCookieCredentials,
    /// Zebra RPC is intentionally restricted to an explicit loopback HTTP endpoint.
    #[error("Zebra HTTP endpoint must be explicit nonzero-port loopback")]
    NonLoopbackEndpoint,
    /// A public-provider endpoint was not an exact bounded remote HTTPS origin root.
    #[error("public Zebra endpoint must be a bounded credential-free remote HTTPS root")]
    InvalidPublicHttpsEndpoint,
    /// A public-provider API key was not bounded visible ASCII.
    #[error("public Zebra API key is invalid")]
    InvalidPublicApiKey,
    /// Public-provider mode cannot connect without its fixed API-key header.
    #[error("public Zebra endpoint requires an x-api-key credential")]
    MissingPublicApiKey,
    /// Cookie-file Basic authentication belongs only to loopback mode.
    #[error("Zebra cookie authentication is forbidden for public HTTPS endpoints")]
    CookieAuthOnPublicEndpoint,
    /// API-key authentication belongs only to public HTTPS mode.
    #[error("Zebra x-api-key authentication is forbidden for loopback HTTP endpoints")]
    ApiKeyAuthOnLoopbackEndpoint,
}

impl HttpZebraRpc {
    /// Connects a client with finite body, timeout, and concurrency limits.
    ///
    /// # Errors
    ///
    /// Rejects zero limits or an endpoint the HTTP client cannot parse.
    pub fn connect(config: &HttpZebraRpcConfig) -> Result<Self, HttpZebraRpcError> {
        if config.request_timeout.is_zero() || config.max_concurrent_requests == 0 {
            return Err(HttpZebraRpcError::InvalidTransportBounds);
        }
        let mut headers = HeaderMap::new();
        match config.mode {
            HttpZebraRpcMode::LoopbackHttp => {
                if !is_loopback_endpoint(&config.endpoint) {
                    return Err(HttpZebraRpcError::NonLoopbackEndpoint);
                }
                if config.api_key.is_some() {
                    return Err(HttpZebraRpcError::ApiKeyAuthOnLoopbackEndpoint);
                }
                if let Some(authorization) = &config.authorization {
                    headers.insert("authorization", authorization.clone());
                }
            }
            HttpZebraRpcMode::PublicHttps => {
                if !is_public_https_endpoint(&config.endpoint) {
                    return Err(HttpZebraRpcError::InvalidPublicHttpsEndpoint);
                }
                if config.authorization.is_some() {
                    return Err(HttpZebraRpcError::CookieAuthOnPublicEndpoint);
                }
                let api_key = config
                    .api_key
                    .as_ref()
                    .ok_or(HttpZebraRpcError::MissingPublicApiKey)?;
                headers.insert("x-api-key", api_key.clone());
            }
        }
        let client = HttpClientBuilder::default()
            .max_request_size(MAX_RPC_BODY_BYTES)
            .max_response_size(MAX_RPC_BODY_BYTES)
            .request_timeout(config.request_timeout)
            .max_concurrent_requests(config.max_concurrent_requests)
            .set_headers(headers)
            .build(&config.endpoint)
            .map_err(HttpZebraRpcError::Build)?;
        Ok(Self { client })
    }
}

#[derive(Debug, Deserialize)]
struct BlockchainInfoDto {
    chain: String,
    blocks: u64,
    bestblockhash: String,
    consensus: ConsensusDto,
}

#[derive(Debug, Deserialize)]
struct ConsensusDto {
    chaintip: String,
}

#[derive(Debug, Deserialize)]
struct VerboseRawTransactionDto {
    hex: String,
    height: Option<i64>,
    blockhash: Option<String>,
    confirmations: Option<u64>,
    in_active_chain: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct VerboseBlockDto {
    hash: String,
    height: u64,
    tx: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GetTxOutDto {
    bestblock: String,
    confirmations: u64,
    value: serde_json::Number,
    #[serde(rename = "scriptPubKey")]
    script_pub_key: ScriptPubKeyDto,
}

#[derive(Debug, Deserialize)]
struct ScriptPubKeyDto {
    hex: String,
}

#[async_trait]
impl ZebraRpc for HttpZebraRpc {
    type Error = HttpZebraRpcError;

    async fn chain_info(&self) -> Result<ZebraChainInfo, Self::Error> {
        let value: BlockchainInfoDto = self
            .client
            .request("getblockchaininfo", rpc_params![])
            .await
            .map_err(HttpZebraRpcError::Request)?;
        Ok(ZebraChainInfo::new(
            ZebraRpcChain::parse(&value.chain)?,
            BlockHeight::from_u32(
                u32::try_from(value.blocks)
                    .map_err(|_| HttpZebraRpcError::OutOfRange { field: "blocks" })?,
            ),
            BlockHash(parse_reverse_hash("bestblockhash", &value.bestblockhash)?),
            parse_branch(&value.consensus.chaintip)?,
        ))
    }

    async fn block_hash(&self, height: BlockHeight) -> Result<BlockHash, Self::Error> {
        let value: String = self
            .client
            .request("getblockhash", rpc_params![u32::from(height)])
            .await
            .map_err(HttpZebraRpcError::Request)?;
        Ok(BlockHash(parse_reverse_hash("block hash", &value)?))
    }

    async fn canonical_block(
        &self,
        block_hash: BlockHash,
    ) -> Result<ZebraCanonicalBlock, Self::Error> {
        let value: VerboseBlockDto = self
            .client
            .request("getblock", rpc_params![block_hash.to_string(), 1])
            .await
            .map_err(HttpZebraRpcError::Request)?;
        parse_canonical_block(value, block_hash)
    }

    async fn block_transaction(
        &self,
        transaction_id: TxId,
        block_hash: BlockHash,
    ) -> Result<Option<Vec<u8>>, Self::Error> {
        let response: Result<String, ClientError> = self
            .client
            .request(
                "getrawtransaction",
                rpc_params![transaction_id.to_string(), 0, block_hash.to_string()],
            )
            .await;
        optional_call(response)?.map_or(Ok(None), |value| parse_transaction_hex(&value).map(Some))
    }

    async fn mempool_transaction_ids(&self) -> Result<Vec<TxId>, Self::Error> {
        let values: Vec<String> = self
            .client
            .request("getrawmempool", rpc_params![false])
            .await
            .map_err(HttpZebraRpcError::Request)?;
        parse_transaction_ids(values)
    }

    async fn raw_transaction(&self, transaction_id: TxId) -> Result<Option<Vec<u8>>, Self::Error> {
        let response: Result<String, ClientError> = self
            .client
            .request(
                "getrawtransaction",
                rpc_params![transaction_id.to_string(), 0],
            )
            .await;
        optional_call(response)?.map_or(Ok(None), |value| parse_transaction_hex(&value).map(Some))
    }

    async fn transaction_state(
        &self,
        transaction_id: TxId,
    ) -> Result<Option<ZebraTransactionState>, Self::Error> {
        let response: Result<VerboseRawTransactionDto, ClientError> = self
            .client
            .request(
                "getrawtransaction",
                rpc_params![transaction_id.to_string(), 1],
            )
            .await;
        let Some(value) = optional_call(response)? else {
            return Ok(None);
        };
        parse_transaction_state(value).map(Some)
    }

    async fn unspent_output(
        &self,
        outpoint: &OutPoint,
    ) -> Result<Option<ZebraUnspentOutput>, Self::Error> {
        let value: Option<GetTxOutDto> = self
            .client
            .request(
                "gettxout",
                rpc_params![
                    TxId::from_bytes(*outpoint.hash()).to_string(),
                    outpoint.n(),
                    true
                ],
            )
            .await
            .map_err(HttpZebraRpcError::Request)?;
        value.map(parse_unspent_output).transpose()
    }

    async fn send_raw_transaction(&self, transaction: &[u8]) -> Result<TxId, Self::Error> {
        if transaction.is_empty() || transaction.len() > MAX_FIRST_LOCK_SUBMISSION_BYTES {
            return Err(HttpZebraRpcError::MalformedTransactionHex);
        }
        let value: String = self
            .client
            .request("sendrawtransaction", rpc_params![hex::encode(transaction)])
            .await
            .map_err(HttpZebraRpcError::Request)?;
        Ok(TxId::from_bytes(parse_reverse_hash(
            "submitted transaction id",
            &value,
        )?))
    }

    fn classify_submission_failure(error: &Self::Error) -> ZebraSubmissionFailure {
        match error {
            HttpZebraRpcError::Request(ClientError::Call(error))
                if matches!(error.code(), -22 | -25 | -26) =>
            {
                ZebraSubmissionFailure::DefinitiveRejection
            }
            _ => ZebraSubmissionFailure::UnknownOutcome,
        }
    }
}

fn parse_canonical_block(
    value: VerboseBlockDto,
    expected_hash: BlockHash,
) -> Result<ZebraCanonicalBlock, HttpZebraRpcError> {
    let block_hash = BlockHash(parse_reverse_hash("block response hash", &value.hash)?);
    if block_hash != expected_hash {
        return Err(HttpZebraRpcError::MalformedBlockContext);
    }
    let block_height = BlockHeight::from_u32(u32::try_from(value.height).map_err(|_| {
        HttpZebraRpcError::OutOfRange {
            field: "block height",
        }
    })?);
    Ok(ZebraCanonicalBlock::new(
        block_hash,
        block_height,
        parse_transaction_ids(value.tx)?,
    ))
}

fn parse_transaction_ids(values: Vec<String>) -> Result<Vec<TxId>, HttpZebraRpcError> {
    if values.len() > MAX_DISCOVERY_TRANSACTION_IDS {
        return Err(HttpZebraRpcError::TooManyTransactionIds);
    }
    values
        .into_iter()
        .map(|value| parse_reverse_hash("transaction id", &value).map(TxId::from_bytes))
        .collect()
}

fn parse_unspent_output(value: GetTxOutDto) -> Result<ZebraUnspentOutput, HttpZebraRpcError> {
    let confirmations =
        u32::try_from(value.confirmations).map_err(|_| HttpZebraRpcError::OutOfRange {
            field: "UTXO confirmations",
        })?;
    if confirmations == 0 {
        return Err(HttpZebraRpcError::OutOfRange {
            field: "UTXO confirmations",
        });
    }
    let amount = parse_zec_amount(&value.value)?;
    if value.script_pub_key.hex.is_empty()
        || value.script_pub_key.hex.len() > ZCASH_MAX_SCRIPT_BYTES.saturating_mul(2)
        || !value.script_pub_key.hex.len().is_multiple_of(2)
        || !is_exact_lower_hex(&value.script_pub_key.hex)
    {
        return Err(HttpZebraRpcError::MalformedUtxoScript);
    }
    let script = hex::decode(value.script_pub_key.hex)
        .map_err(|_| HttpZebraRpcError::MalformedUtxoScript)?;
    Ok(ZebraUnspentOutput::new(
        BlockHash(parse_reverse_hash("UTXO best block", &value.bestblock)?),
        confirmations,
        TxOut::new(amount, Script(Code(script))),
    ))
}

fn parse_zec_amount(value: &serde_json::Number) -> Result<Zatoshis, HttpZebraRpcError> {
    let rendered = value.to_string();
    if rendered.starts_with('-') || rendered.contains(['e', 'E', '+']) {
        return Err(HttpZebraRpcError::MalformedUtxoAmount);
    }
    let mut parts = rendered.split('.');
    let whole = parts.next().ok_or(HttpZebraRpcError::MalformedUtxoAmount)?;
    let fractional = parts.next().unwrap_or("");
    if parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fractional.len() > 8
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(HttpZebraRpcError::MalformedUtxoAmount);
    }
    let whole = whole
        .parse::<u64>()
        .map_err(|_| HttpZebraRpcError::MalformedUtxoAmount)?;
    let mut fractional_zatoshis = if fractional.is_empty() {
        0
    } else {
        fractional
            .parse::<u64>()
            .map_err(|_| HttpZebraRpcError::MalformedUtxoAmount)?
    };
    for _ in fractional.len()..8 {
        fractional_zatoshis = fractional_zatoshis
            .checked_mul(10)
            .ok_or(HttpZebraRpcError::MalformedUtxoAmount)?;
    }
    let zatoshis = whole
        .checked_mul(100_000_000)
        .and_then(|whole| whole.checked_add(fractional_zatoshis))
        .ok_or(HttpZebraRpcError::MalformedUtxoAmount)?;
    Zatoshis::from_u64(zatoshis).map_err(|_| HttpZebraRpcError::MalformedUtxoAmount)
}

fn optional_call<T>(response: Result<T, ClientError>) -> Result<Option<T>, HttpZebraRpcError> {
    match response {
        Ok(value) => Ok(Some(value)),
        Err(ClientError::Call(error)) if error.code() == TRANSACTION_NOT_FOUND_CODE => Ok(None),
        Err(error) => Err(HttpZebraRpcError::Request(error)),
    }
}

fn parse_transaction_state(
    value: VerboseRawTransactionDto,
) -> Result<ZebraTransactionState, HttpZebraRpcError> {
    let raw_transaction = parse_transaction_hex(&value.hex)?;
    match (
        value.height,
        value.blockhash,
        value.confirmations,
        value.in_active_chain,
    ) {
        (None, None, None, None) | (None, None, Some(0), None | Some(false)) => {
            Ok(ZebraTransactionState::Mempool { raw_transaction })
        }
        (Some(height), Some(block_hash), Some(confirmations), Some(in_active_chain))
            if height >= 0 && confirmations > 0 =>
        {
            Ok(ZebraTransactionState::Confirmed {
                raw_transaction,
                block_hash: BlockHash(parse_reverse_hash("transaction block hash", &block_hash)?),
                block_height: BlockHeight::from_u32(u32::try_from(height).map_err(|_| {
                    HttpZebraRpcError::OutOfRange {
                        field: "transaction height",
                    }
                })?),
                confirmations: u32::try_from(confirmations).map_err(|_| {
                    HttpZebraRpcError::OutOfRange {
                        field: "confirmations",
                    }
                })?,
                in_active_chain,
            })
        }
        _ => Err(HttpZebraRpcError::PartialConfirmationContext),
    }
}

fn parse_branch(value: &str) -> Result<BranchId, HttpZebraRpcError> {
    if value.len() != 8 || !is_exact_lower_hex(value) {
        return Err(HttpZebraRpcError::MalformedConsensusBranch);
    }
    let raw =
        u32::from_str_radix(value, 16).map_err(|_| HttpZebraRpcError::MalformedConsensusBranch)?;
    BranchId::try_from(raw).map_err(|_| HttpZebraRpcError::MalformedConsensusBranch)
}

fn parse_reverse_hash(field: &'static str, value: &str) -> Result<[u8; 32], HttpZebraRpcError> {
    if value.len() != 64 || !is_exact_lower_hex(value) {
        return Err(HttpZebraRpcError::MalformedHash { field });
    }
    ReverseHex::decode(value).ok_or(HttpZebraRpcError::MalformedHash { field })
}

fn parse_transaction_hex(value: &str) -> Result<Vec<u8>, HttpZebraRpcError> {
    if value.is_empty()
        || value.len() > MAX_FIRST_LOCK_SUBMISSION_BYTES.saturating_mul(2)
        || !value.len().is_multiple_of(2)
        || !is_exact_lower_hex(value)
    {
        return Err(HttpZebraRpcError::MalformedTransactionHex);
    }
    hex::decode(value).map_err(|_| HttpZebraRpcError::MalformedTransactionHex)
}

fn is_exact_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RootEndpoint<'a> {
    host: &'a str,
    port: Option<u16>,
}

fn parse_root_endpoint<'a>(endpoint: &'a str, scheme: &str) -> Option<RootEndpoint<'a>> {
    if endpoint.is_empty()
        || endpoint.len() > MAX_RPC_ENDPOINT_BYTES
        || !endpoint.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return None;
    }
    let authority = endpoint.strip_prefix(scheme)?;
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    if authority.is_empty()
        || authority
            .bytes()
            .any(|byte| matches!(byte, b'/' | b'?' | b'#' | b'@'))
    {
        return None;
    }

    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let closing = bracketed.find(']')?;
        let host = &bracketed[..closing];
        let remainder = &bracketed[closing + 1..];
        let port = if remainder.is_empty() {
            None
        } else {
            Some(remainder.strip_prefix(':')?.parse::<u16>().ok()?)
        };
        (host, port)
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if host.contains(':') {
            return None;
        }
        (host, Some(port.parse::<u16>().ok()?))
    } else {
        (authority, None)
    };
    if host.is_empty() || port == Some(0) {
        return None;
    }
    Some(RootEndpoint { host, port })
}

fn is_loopback_endpoint(endpoint: &str) -> bool {
    parse_root_endpoint(endpoint, "http://")
        .is_some_and(|parsed| parsed.port.is_some() && matches!(parsed.host, "127.0.0.1" | "::1"))
}

fn is_public_https_endpoint(endpoint: &str) -> bool {
    endpoint == TATUM_TESTNET_ZEBRA_ENDPOINT
        || endpoint.strip_suffix('/') == Some(TATUM_TESTNET_ZEBRA_ENDPOINT)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use base64::Engine as _;
    use jsonrpsee::core::ClientError;
    use jsonrpsee_http_client::types::ErrorObjectOwned;
    use jsonrpsee_server::{RpcModule, ServerBuilder};
    use serde_json::{Value, json};
    use zcash_encoding::ReverseHex;
    use zcash_primitives::block::BlockHash;
    use zcash_protocol::TxId;
    use zcash_protocol::consensus::{BranchId, NetworkType};
    use zcash_transparent::bundle::OutPoint;

    use super::{
        BASE64_STANDARD, BlockchainInfoDto, GetTxOutDto, HttpZebraRpc, HttpZebraRpcConfig,
        HttpZebraRpcError, MAX_DISCOVERY_TRANSACTION_IDS, MAX_FIRST_LOCK_SUBMISSION_BYTES,
        VerboseBlockDto, VerboseRawTransactionDto, ZebraChainIdentity, ZebraIdentityError,
        ZebraRpc, ZebraRpcChain, ZebraSubmissionFailure, ZebraTransactionState, optional_call,
        parse_branch, parse_canonical_block, parse_reverse_hash, parse_transaction_hex,
        parse_transaction_ids, parse_transaction_state, parse_unspent_output, parse_zec_amount,
    };

    const HASH: &str = "029f11d80ef9765602235e1bc9727e3eb6ba20839319f761fee920d63401e327";

    #[test]
    fn private_dtos_accept_documented_fields_and_harmless_extras() {
        let info: BlockchainInfoDto = serde_json::from_str(&format!(
            r#"{{"chain":"test","blocks":100,"bestblockhash":"{HASH}","consensus":{{"chaintip":"5437f330","nextblock":"ignored"}},"headers":101}}"#
        ))
        .expect("bounded documented response");
        assert_eq!(info.chain, "test");
        assert_eq!(info.blocks, 100);
        assert_eq!(
            parse_branch(&info.consensus.chaintip).expect("known branch ID"),
            BranchId::Nu6_2
        );
        assert_eq!(
            parse_reverse_hash("hash", &info.bestblockhash).expect("exact hash"),
            ReverseHex::decode(HASH).expect("fixed exact hash")
        );

        let verbose: VerboseRawTransactionDto = serde_json::from_str(&format!(
            r#"{{"hex":"0500","height":90,"blockhash":"{HASH}","confirmations":11,"in_active_chain":true,"size":2}}"#
        ))
        .expect("verbose response permits harmless additions");
        assert!(matches!(
            parse_transaction_state(verbose),
            Ok(ZebraTransactionState::Confirmed {
                block_height,
                confirmations: 11,
                in_active_chain: true,
                ..
            }) if u32::from(block_height) == 90
        ));
    }

    #[test]
    fn exact_hash_branch_and_transaction_hex_bounds_fail_closed() {
        for value in ["00", &"a".repeat(63), &"g".repeat(64), &HASH.to_uppercase()] {
            assert!(matches!(
                parse_reverse_hash("hash", value),
                Err(HttpZebraRpcError::MalformedHash { .. })
            ));
        }
        for branch in ["5437f33", "5437F330", "zzzzzzzz", "ffffffff"] {
            assert!(matches!(
                parse_branch(branch),
                Err(HttpZebraRpcError::MalformedConsensusBranch)
            ));
        }
        for value in ["", "0", "GG", "aA"] {
            assert!(matches!(
                parse_transaction_hex(value),
                Err(HttpZebraRpcError::MalformedTransactionHex)
            ));
        }
        let oversized = "aa".repeat(MAX_FIRST_LOCK_SUBMISSION_BYTES + 1);
        assert!(matches!(
            parse_transaction_hex(&oversized),
            Err(HttpZebraRpcError::MalformedTransactionHex)
        ));
    }

    #[test]
    fn block_and_mempool_inventories_are_exact_and_bounded() {
        let expected_hash = BlockHash(ReverseHex::decode(HASH).expect("fixed block hash"));
        let block = parse_canonical_block(
            VerboseBlockDto {
                hash: HASH.to_owned(),
                height: 91,
                tx: vec![HASH.to_owned()],
            },
            expected_hash,
        )
        .expect("exact hash-addressed block inventory");
        assert_eq!(block.block_hash(), expected_hash);
        assert_eq!(u32::from(block.block_height()), 91);
        assert_eq!(
            block.transaction_ids(),
            [TxId::from_bytes(
                ReverseHex::decode(HASH).expect("fixed txid")
            )]
        );

        assert!(matches!(
            parse_canonical_block(
                VerboseBlockDto {
                    hash: HASH.to_owned(),
                    height: 91,
                    tx: Vec::new(),
                },
                BlockHash([0x55; 32]),
            ),
            Err(HttpZebraRpcError::MalformedBlockContext)
        ));
        assert!(matches!(
            parse_transaction_ids(vec!["00".to_owned()]),
            Err(HttpZebraRpcError::MalformedHash { .. })
        ));
        assert!(matches!(
            parse_transaction_ids(vec![HASH.to_owned(); MAX_DISCOVERY_TRANSACTION_IDS + 1]),
            Err(HttpZebraRpcError::TooManyTransactionIds)
        ));
    }

    #[test]
    fn zec_amounts_are_exact_to_eight_decimals_and_integer_safe() {
        for (wire, expected) in [
            ("0", 0),
            ("1", 100_000_000),
            ("1.2", 120_000_000),
            ("0.00000001", 1),
            ("1.23456789", 123_456_789),
        ] {
            let number = serde_json::from_str(wire).expect("valid JSON number");
            let amount = parse_zec_amount(&number).expect("exact bounded amount");
            assert_eq!(u64::from(amount), expected, "wire amount {wire}");
        }

        for wire in ["-1", "1e2", "1.000000001", "18446744073709551615"] {
            let number = serde_json::from_str(wire).expect("syntactically valid JSON number");
            assert!(matches!(
                parse_zec_amount(&number),
                Err(HttpZebraRpcError::MalformedUtxoAmount)
            ));
        }
    }

    #[test]
    fn gettxout_shape_is_bounded_and_preserves_exact_facts() {
        let value: GetTxOutDto = serde_json::from_str(&format!(
            r#"{{"bestblock":"{HASH}","confirmations":7,"value":1.23456789,"scriptPubKey":{{"hex":"51","type":"nonstandard"}},"coinbase":false}}"#
        ))
        .expect("documented UTXO response");
        let parsed = parse_unspent_output(value).expect("bounded exact UTXO");
        assert_eq!(
            parsed.best_block().0,
            ReverseHex::decode(HASH).expect("hash")
        );
        assert_eq!(parsed.confirmations(), 7);
        assert_eq!(u64::from(parsed.output().value()), 123_456_789);
        assert_eq!(parsed.output().script_pubkey().0.0, [0x51]);

        for json in [
            format!(
                r#"{{"bestblock":"{HASH}","confirmations":0,"value":1,"scriptPubKey":{{"hex":"51"}}}}"#
            ),
            format!(
                r#"{{"bestblock":"{HASH}","confirmations":1,"value":1,"scriptPubKey":{{"hex":""}}}}"#
            ),
            format!(
                r#"{{"bestblock":"{HASH}","confirmations":1,"value":1,"scriptPubKey":{{"hex":"AA"}}}}"#
            ),
        ] {
            let value: GetTxOutDto = serde_json::from_str(&json).expect("response shape");
            assert!(parse_unspent_output(value).is_err());
        }
    }

    #[test]
    fn only_explicit_rejection_codes_are_definitive_broadcast_failures() {
        for code in [-22, -25, -26] {
            let error = HttpZebraRpcError::Request(ClientError::Call(ErrorObjectOwned::owned(
                code, "rejected", None::<()>,
            )));
            assert_eq!(
                <HttpZebraRpc as ZebraRpc>::classify_submission_failure(&error),
                ZebraSubmissionFailure::DefinitiveRejection
            );
        }
        for code in [-27, -28, -1] {
            let error = HttpZebraRpcError::Request(ClientError::Call(ErrorObjectOwned::owned(
                code,
                "ambiguous",
                None::<()>,
            )));
            assert_eq!(
                <HttpZebraRpc as ZebraRpc>::classify_submission_failure(&error),
                ZebraSubmissionFailure::UnknownOutcome
            );
        }
    }

    #[tokio::test]
    async fn loopback_wire_observes_before_byte_exact_rebroadcast() {
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut module = RpcModule::new(Arc::clone(&calls));
        module
            .register_method::<Result<Value, ErrorObjectOwned>, _>(
                "getrawtransaction",
                |params, calls, _| {
                    let (transaction_id, verbose): (String, u8) = params.parse()?;
                    calls
                        .lock()
                        .expect("call log")
                        .push(format!("observe:{transaction_id}:{verbose}"));
                    Err(ErrorObjectOwned::owned(-5, "not found", None::<()>))
                },
            )
            .expect("register observation");
        module
            .register_method::<Result<Value, ErrorObjectOwned>, _>(
                "gettxout",
                |params, calls, _| {
                    let (transaction_id, output_index, include_mempool): (String, u32, bool) =
                        params.parse()?;
                    calls.lock().expect("call log").push(format!(
                        "utxo:{transaction_id}:{output_index}:{include_mempool}"
                    ));
                    Ok(json!({
                        "bestblock": HASH,
                        "confirmations": 7,
                        "value": 1,
                        "scriptPubKey": { "hex": "51" }
                    }))
                },
            )
            .expect("register UTXO lookup");
        module
            .register_method::<Result<Value, ErrorObjectOwned>, _>(
                "sendrawtransaction",
                |params, calls, _| {
                    let exact_hex: String = params.one()?;
                    calls
                        .lock()
                        .expect("call log")
                        .push(format!("submit:{exact_hex}"));
                    Ok(json!(TxId::from_bytes([0x42; 32]).to_string()))
                },
            )
            .expect("register submission");

        let server = ServerBuilder::default()
            .build("127.0.0.1:0")
            .await
            .expect("bind isolated loopback server");
        let address = server.local_addr().expect("loopback address");
        let handle = server.start(module);
        let rpc = HttpZebraRpc::connect(&HttpZebraRpcConfig::new(format!("http://{address}")))
            .expect("bounded loopback client");
        let transaction_id = TxId::from_bytes([0x42; 32]);
        let outpoint = OutPoint::new([0x42; 32], 0);

        assert_eq!(
            rpc.transaction_state(transaction_id)
                .await
                .expect("exact identity observation"),
            None
        );
        assert!(
            rpc.unspent_output(&outpoint)
                .await
                .expect("exact UTXO lookup")
                .is_some()
        );
        assert_eq!(
            rpc.send_raw_transaction(&[0xde, 0xad, 0xbe, 0xef])
                .await
                .expect("byte-exact accepted submission"),
            transaction_id
        );
        assert_eq!(
            *calls.lock().expect("call log"),
            [
                format!("observe:{transaction_id}:1"),
                format!("utxo:{transaction_id}:0:true"),
                "submit:deadbeef".to_owned(),
            ]
        );
        handle.stop().expect("stop loopback server");
        handle.stopped().await;
    }

    #[tokio::test]
    async fn loopback_wire_scans_hash_addressed_blocks_and_bounded_mempool() {
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut module = RpcModule::new(Arc::clone(&calls));
        module
            .register_method::<Result<Value, ErrorObjectOwned>, _>(
                "getblock",
                |params, calls, _| {
                    let (block_hash, verbosity): (String, u8) = params.parse()?;
                    calls
                        .lock()
                        .expect("call log")
                        .push(format!("block:{block_hash}:{verbosity}"));
                    Ok(json!({ "hash": HASH, "height": 91, "tx": [HASH] }))
                },
            )
            .expect("register block inventory");
        module
            .register_method::<Result<Value, ErrorObjectOwned>, _>(
                "getrawmempool",
                |params, calls, _| {
                    let verbose: bool = params.one()?;
                    calls
                        .lock()
                        .expect("call log")
                        .push(format!("mempool:{verbose}"));
                    Ok(json!([HASH]))
                },
            )
            .expect("register mempool inventory");
        module
            .register_method::<Result<Value, ErrorObjectOwned>, _>(
                "getrawtransaction",
                |params, calls, _| {
                    let (transaction_id, verbosity, block_hash): (String, u8, String) =
                        params.parse()?;
                    calls.lock().expect("call log").push(format!(
                        "block-tx:{transaction_id}:{verbosity}:{block_hash}"
                    ));
                    Ok(json!("deadbeef"))
                },
            )
            .expect("register block transaction");

        let server = ServerBuilder::default()
            .build("127.0.0.1:0")
            .await
            .expect("bind isolated loopback server");
        let address = server.local_addr().expect("loopback address");
        let handle = server.start(module);
        let rpc = HttpZebraRpc::connect(&HttpZebraRpcConfig::new(format!("http://{address}")))
            .expect("bounded loopback client");
        let block_hash = BlockHash(ReverseHex::decode(HASH).expect("fixed block hash"));
        let transaction_id = TxId::from_bytes(ReverseHex::decode(HASH).expect("fixed txid"));

        let block = rpc
            .canonical_block(block_hash)
            .await
            .expect("hash-addressed block inventory");
        assert_eq!(block.transaction_ids(), [transaction_id]);
        assert_eq!(
            rpc.mempool_transaction_ids()
                .await
                .expect("bounded mempool inventory"),
            [transaction_id]
        );
        assert_eq!(
            rpc.block_transaction(transaction_id, block_hash)
                .await
                .expect("block-qualified transaction lookup"),
            Some(vec![0xde, 0xad, 0xbe, 0xef])
        );
        assert_eq!(
            *calls.lock().expect("call log"),
            [
                format!("block:{block_hash}:1"),
                "mempool:false".to_owned(),
                format!("block-tx:{transaction_id}:0:{block_hash}"),
            ]
        );
        handle.stop().expect("stop loopback server");
        handle.stopped().await;
    }

    #[test]
    fn verbose_state_rejects_every_partial_confirmation_shape() {
        let partial = [
            (Some(90), None, Some(11), Some(true)),
            (None, Some(HASH.to_owned()), Some(11), Some(true)),
            (Some(90), Some(HASH.to_owned()), None, Some(true)),
            (Some(90), Some(HASH.to_owned()), Some(11), None),
            (Some(-1), Some(HASH.to_owned()), Some(11), Some(true)),
            (Some(90), Some(HASH.to_owned()), Some(0), Some(true)),
        ];
        for (height, blockhash, confirmations, in_active_chain) in partial {
            let dto = VerboseRawTransactionDto {
                hex: "0500".to_owned(),
                height,
                blockhash,
                confirmations,
                in_active_chain,
            };
            assert!(matches!(
                parse_transaction_state(dto),
                Err(HttpZebraRpcError::PartialConfirmationContext)
            ));
        }
        for dto in [
            VerboseRawTransactionDto {
                hex: "0500".to_owned(),
                height: None,
                blockhash: None,
                confirmations: None,
                in_active_chain: None,
            },
            VerboseRawTransactionDto {
                hex: "0500".to_owned(),
                height: None,
                blockhash: None,
                confirmations: Some(0),
                in_active_chain: Some(false),
            },
        ] {
            assert!(matches!(
                parse_transaction_state(dto),
                Ok(ZebraTransactionState::Mempool { .. })
            ));
        }
    }

    #[test]
    fn only_rpc_call_code_minus_five_is_absence() {
        let missing: Result<(), ClientError> = Err(ClientError::Call(ErrorObjectOwned::owned(
            -5, "missing", None::<()>,
        )));
        assert!(matches!(optional_call(missing), Ok(None)));

        let other: Result<(), ClientError> = Err(ClientError::Call(ErrorObjectOwned::owned(
            -8,
            "not absence",
            None::<()>,
        )));
        assert!(matches!(
            optional_call(other),
            Err(HttpZebraRpcError::Request(ClientError::Call(_)))
        ));
    }

    #[test]
    fn immutable_identity_rejects_zero_and_network_rpc_mismatch() {
        assert_eq!(
            ZebraChainIdentity::new(
                NetworkType::Main,
                ZebraRpcChain::Test,
                BranchId::Nu6_2,
                BlockHash([1; 32]),
            ),
            Err(ZebraIdentityError::RpcChainMismatch)
        );
        assert_eq!(
            ZebraChainIdentity::new(
                NetworkType::Regtest,
                ZebraRpcChain::Test,
                BranchId::Nu6_2,
                BlockHash([0; 32]),
            ),
            Err(ZebraIdentityError::ZeroGenesisHash)
        );
        let deterministic = ZebraChainIdentity::deterministic_regtest_nu6_2();
        assert_eq!(
            deterministic.genesis_hash().0,
            ReverseHex::decode(HASH).expect("fixed exact hash")
        );
    }

    #[test]
    fn cookie_auth_is_bounded_sensitive_and_absent_from_debug() {
        let config = HttpZebraRpcConfig::new("http://127.0.0.1:18232")
            .with_cookie_credentials(b"__cookie__:top-secret\n")
            .expect("valid cookie-file line");
        let authorization = config.authorization.as_ref().expect("header configured");
        assert!(authorization.is_sensitive());
        let debug = format!("{config:?}");
        assert!(debug.contains("cookie_auth_enabled: true"));
        assert!(!debug.contains("top-secret"));
        assert!(!debug.contains(&BASE64_STANDARD.encode(b"__cookie__:top-secret")));

        for invalid in [
            b"".as_slice(),
            b"missing-delimiter".as_slice(),
            b":password".as_slice(),
            b"username:".as_slice(),
            b"user:has space".as_slice(),
            b"user:secret\nextra".as_slice(),
        ] {
            assert!(matches!(
                HttpZebraRpcConfig::new("http://127.0.0.1:18232").with_cookie_credentials(invalid),
                Err(HttpZebraRpcError::InvalidCookieCredentials)
            ));
        }
        assert!(matches!(
            HttpZebraRpcConfig::new("http://127.0.0.1:18232")
                .with_cookie_credentials(vec![b'a'; 1_025]),
            Err(HttpZebraRpcError::InvalidCookieCredentials)
        ));
    }

    #[test]
    fn public_https_client_uses_a_bounded_sensitive_api_key_without_connecting() {
        const TATUM_TESTNET_ZEBRAD: &str = "https://zcash-testnet-zebrad.gateway.tatum.io";
        const SECRET: &[u8] = b"dedicated-test-key";

        let config = HttpZebraRpcConfig::public_https(TATUM_TESTNET_ZEBRAD)
            .expect("documented root HTTPS endpoint")
            .with_public_api_key(SECRET)
            .expect("bounded visible API key");
        let api_key = config.api_key.as_ref().expect("API key header configured");
        assert!(api_key.is_sensitive());
        assert_eq!(api_key.as_bytes(), SECRET);

        let debug = format!("{config:?}");
        assert!(debug.contains("PublicHttps"));
        assert!(debug.contains("api_key_auth_enabled: true"));
        assert!(!debug.contains("dedicated-test-key"));

        HttpZebraRpc::connect(&config)
            .expect("Tatum root client construction is local and nonconnecting");
    }

    #[test]
    fn public_api_key_is_bounded_and_mode_specific() {
        const ENDPOINT: &str = "https://zcash-testnet-zebrad.gateway.tatum.io";

        for invalid in [
            b"".as_slice(),
            b"has space".as_slice(),
            b"has\nnewline".as_slice(),
        ] {
            assert!(matches!(
                HttpZebraRpcConfig::public_https(ENDPOINT)
                    .expect("valid public endpoint")
                    .with_public_api_key(invalid),
                Err(HttpZebraRpcError::InvalidPublicApiKey)
            ));
        }
        assert!(matches!(
            HttpZebraRpcConfig::public_https(ENDPOINT)
                .expect("valid public endpoint")
                .with_public_api_key(vec![b'a'; 1_025]),
            Err(HttpZebraRpcError::InvalidPublicApiKey)
        ));
        assert!(matches!(
            HttpZebraRpcConfig::new("http://127.0.0.1:18232")
                .with_public_api_key(b"not-for-loopback"),
            Err(HttpZebraRpcError::ApiKeyAuthOnLoopbackEndpoint)
        ));
        assert!(matches!(
            HttpZebraRpcConfig::public_https(ENDPOINT)
                .expect("valid public endpoint")
                .with_public_api_key(b"public-key")
                .expect("valid public API key")
                .with_cookie_credentials(b"__cookie__:not-for-public"),
            Err(HttpZebraRpcError::CookieAuthOnPublicEndpoint)
        ));
        assert!(matches!(
            HttpZebraRpc::connect(
                &HttpZebraRpcConfig::public_https(ENDPOINT).expect("valid public endpoint")
            ),
            Err(HttpZebraRpcError::MissingPublicApiKey)
        ));
    }

    #[test]
    fn public_https_endpoint_is_an_exact_bounded_credential_free_root() {
        for endpoint in [
            "http://zcash-testnet-zebrad.gateway.tatum.io",
            "https://127.0.0.1",
            "https://127.0.0.1:443",
            "https://10.0.0.1",
            "https://169.254.169.254",
            "https://[::1]",
            "https://localhost",
            "https://example.test",
            "https://user:secret@example.test",
            "https://example.test/rpc",
            "https://example.test/?api-key=secret",
            "https://example.test/#secret",
            "https://example.test:0",
            "https://",
        ] {
            assert!(matches!(
                HttpZebraRpcConfig::public_https(endpoint),
                Err(HttpZebraRpcError::InvalidPublicHttpsEndpoint)
            ));
        }
        assert!(matches!(
            HttpZebraRpcConfig::public_https(format!("https://{}.example.test", "a".repeat(2_048))),
            Err(HttpZebraRpcError::InvalidPublicHttpsEndpoint)
        ));

        let invalid = HttpZebraRpcConfig::public_https("https://user:secret@example.test");
        assert!(!format!("{invalid:?}").contains("secret"));
    }

    #[test]
    fn both_http_modes_reject_zero_transport_bounds() {
        let loopback =
            HttpZebraRpcConfig::new("http://127.0.0.1:18232").with_request_timeout(Duration::ZERO);
        assert!(matches!(
            HttpZebraRpc::connect(&loopback),
            Err(HttpZebraRpcError::InvalidTransportBounds)
        ));

        let public =
            HttpZebraRpcConfig::public_https("https://zcash-testnet-zebrad.gateway.tatum.io")
                .expect("valid public endpoint")
                .with_public_api_key(b"public-key")
                .expect("valid public API key")
                .with_max_concurrent_requests(0);
        assert!(matches!(
            HttpZebraRpc::connect(&public),
            Err(HttpZebraRpcError::InvalidTransportBounds)
        ));
    }

    #[test]
    fn http_client_rejects_nonloopback_and_zero_port_without_connecting() {
        for endpoint in [
            "http://0.0.0.0:18232",
            "http://example.test:18232",
            "https://127.0.0.1:18232",
            "http://127.0.0.1:0",
            "http://127.0.0.1:18232/path",
        ] {
            assert!(matches!(
                HttpZebraRpc::connect(&HttpZebraRpcConfig::new(endpoint)),
                Err(HttpZebraRpcError::NonLoopbackEndpoint)
            ));
        }
        HttpZebraRpc::connect(&HttpZebraRpcConfig::new("http://[::1]:18232/"))
            .expect("explicit IPv6 loopback client construction is local and nonconnecting");
    }
}
