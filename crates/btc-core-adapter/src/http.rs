use std::fs::{self, File};
use std::io::Read as _;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use bitcoin::{BlockHash, OutPoint, Txid};
use corepc_types::v31::{
    GetBlockHash, GetBlockHeaderVerbose, GetBlockchainInfo, GetIndexInfo, GetNetworkInfo,
    GetRawTransactionVerbose, GetTxSpendingPrevout, SendRawTransaction, TestMempoolAccept,
};
use jsonrpsee::{
    core::{ClientError, client::ClientT as _},
    rpc_params,
};
use jsonrpsee_core::middleware::layer::RpcLogger;
use jsonrpsee_http_client::transport::{Error as TransportError, HttpBackend};
use jsonrpsee_http_client::{
    HeaderMap, HeaderValue, HttpBody, HttpClient, HttpClientBuilder, HttpRequest, HttpResponse,
    RpcService,
};
use url::{Host, Url};
use zeroize::Zeroizing;

/// Bitcoin Core before 28, and the gateways providers put in front of any
/// version, answer in the JSON-RPC 1.x envelope: no `jsonrpc` member (or an
/// empty one), and both `result` and `error` present with one of them null. The client parses only
/// 2.0 and refused every reply from a public provider, so a 1.x reply is
/// rewritten into 2.0 first. A 2.0 reply passes through byte for byte.
#[derive(Clone)]
struct AcceptJsonRpc1<S>(S);

impl<S, B> tower::Service<HttpRequest> for AcceptJsonRpc1<S>
where
    S: tower::Service<HttpRequest, Response = HttpResponse<B>, Error = TransportError>,
    S::Future: Send + 'static,
    B: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Response = HttpResponse<HttpBody>;
    type Error = TransportError;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.0.poll_ready(context)
    }

    fn call(&mut self, request: HttpRequest) -> Self::Future {
        let reply = self.0.call(request);
        Box::pin(async move {
            let (mut parts, body) = reply.await?.into_parts();
            let (bytes, _) = jsonrpsee_core::http_helpers::read_body(
                &parts.headers,
                body,
                MAX_RPC_RESPONSE_BYTES,
            )
            .await?;
            // The rewritten body has another length.
            parts.headers.remove("content-length");
            Ok(HttpResponse::from_parts(
                parts,
                HttpBody::from(json_rpc_2_envelope(bytes)),
            ))
        })
    }
}

fn json_rpc_2_envelope(reply: Vec<u8>) -> Vec<u8> {
    let Ok(serde_json::Value::Object(mut envelope)) = serde_json::from_slice(&reply) else {
        return reply;
    };
    // Exactly "2.0" or not 2.0: one public provider's backends were seen to
    // answer with the member missing, and with it present but empty.
    if envelope.get("jsonrpc").and_then(serde_json::Value::as_str) == Some("2.0") {
        return reply;
    }
    envelope.insert("jsonrpc".to_owned(), "2.0".into());
    // 2.0 carries exactly one of the two members.
    let failed = envelope.get("error").is_some_and(|error| !error.is_null());
    envelope.remove(if failed { "result" } else { "error" });
    serde_json::to_vec(&envelope).unwrap_or(reply)
}

/// One `gettxspendingprevout` entry; `spendingtxid` is absent while unspent.
#[derive(serde::Deserialize)]
struct MempoolSpender {
    #[serde(default)]
    spendingtxid: Option<Txid>,
}

/// The part of `getblock <hash> 2` the spender scan reads.
#[derive(serde::Deserialize)]
struct BlockWithTransactions {
    tx: Vec<BlockTransaction>,
}

#[derive(serde::Deserialize)]
struct BlockTransaction {
    hex: String,
}

use crate::{
    BitcoinCoreAdapter, BitcoinCoreRpc, CoreConnectivityPolicy, CoreRpcRoute,
    MAX_RAW_TRANSACTION_BYTES, SendFailure,
};

const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_CONCURRENT_REQUESTS: usize = 1;
const MAX_RPC_TIMEOUT: Duration = Duration::from_mins(5);
const MAX_CONCURRENT_REQUESTS: usize = 16;
const MAX_RPC_ENDPOINT_BYTES: usize = 2_048;
const MAX_BASIC_CREDENTIAL_FILE_BYTES: usize = 1_024;
const MAX_RPC_REQUEST_BYTES: u32 = 2_100_000;
const MAX_RPC_RESPONSE_BYTES: u32 = 4_100_000;
const TRANSACTION_NOT_FOUND_CODE: i32 = -5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HttpBitcoinCoreRoute {
    LiteralLoopback,
    ExactHttpsBasic,
}

impl HttpBitcoinCoreRoute {
    const fn label(self) -> &'static str {
        match self {
            Self::LiteralLoopback => "literal_loopback",
            Self::ExactHttpsBasic => "exact_https_basic",
        }
    }

    const fn rpc_route(self) -> CoreRpcRoute {
        match self {
            Self::LiteralLoopback => CoreRpcRoute::LiteralLoopback,
            Self::ExactHttpsBasic => CoreRpcRoute::ExactHttpsBasic,
        }
    }
}

/// Finite Bitcoin Core HTTP JSON-RPC configuration.
#[derive(Clone, Eq, PartialEq)]
pub struct HttpBitcoinCoreConfig {
    endpoint: Box<str>,
    route: HttpBitcoinCoreRoute,
    request_timeout: Duration,
    max_concurrent_requests: usize,
    authorization: Option<HeaderValue>,
}

impl std::fmt::Debug for HttpBitcoinCoreConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpBitcoinCoreConfig")
            .field("route", &self.route.label())
            .field("request_timeout", &self.request_timeout)
            .field("max_concurrent_requests", &self.max_concurrent_requests)
            .field("basic_auth_enabled", &self.authorization.is_some())
            .field(
                "cookie_auth_enabled",
                &(self.route == HttpBitcoinCoreRoute::LiteralLoopback
                    && self.authorization.is_some()),
            )
            .finish_non_exhaustive()
    }
}

impl HttpBitcoinCoreConfig {
    /// Creates a finite client configuration for a literal loopback HTTP origin root.
    ///
    /// Accepted hosts are exactly `127.0.0.1` and `[::1]`, with an explicit nonzero
    /// port. Credentials, paths, queries, fragments, aliases, and public endpoints
    /// are rejected before client construction.
    ///
    /// # Errors
    ///
    /// Rejects anything other than the exact bounded loopback endpoint form.
    pub fn new(endpoint: impl Into<Box<str>>) -> Result<Self, HttpBitcoinCoreError> {
        let endpoint = endpoint.into();
        if !is_literal_loopback_endpoint(&endpoint) {
            return Err(HttpBitcoinCoreError::NonLoopbackEndpoint);
        }
        Ok(Self {
            endpoint,
            route: HttpBitcoinCoreRoute::LiteralLoopback,
            request_timeout: DEFAULT_RPC_TIMEOUT,
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
            authorization: None,
        })
    }

    /// Creates a finite TLS client for one exact owner-allowlisted HTTPS origin.
    ///
    /// Both endpoint arguments must be the same canonical HTTPS origin root. The
    /// host must be a DNS name, and explicit ports, credentials, paths, queries,
    /// fragments, IP literals, `localhost`, and wildcard hosts are rejected.
    /// Bounded Basic credentials are loaded from an owner-private stable file and
    /// installed as a sensitive header. Construction performs no network request.
    ///
    /// # Errors
    ///
    /// Rejects a non-exact or malformed route and an insecure or malformed
    /// credential file without retaining either input path.
    pub fn new_exact_https_basic_gateway(
        endpoint: impl Into<Box<str>>,
        allowlisted_endpoint: impl Into<Box<str>>,
        credential_file: impl AsRef<Path>,
    ) -> Result<Self, HttpBitcoinCoreError> {
        let endpoint = endpoint.into();
        let allowlisted_endpoint = allowlisted_endpoint.into();
        if endpoint != allowlisted_endpoint
            || !is_exact_https_origin(&endpoint)
            || !is_exact_https_origin(&allowlisted_endpoint)
        {
            return Err(HttpBitcoinCoreError::NonAllowlistedHttpsEndpoint);
        }
        let credential = read_private_basic_credentials(credential_file.as_ref()).map_err(
            |error| match error {
                PrivateBasicCredentialsError::Insecure => {
                    HttpBitcoinCoreError::InsecureBasicCredentialsFile
                }
                PrivateBasicCredentialsError::Invalid => {
                    HttpBitcoinCoreError::InvalidBasicCredentialsFile
                }
            },
        )?;
        let authorization = basic_authorization(&credential)
            .map_err(|()| HttpBitcoinCoreError::InvalidBasicCredentialsFile)?;
        Ok(Self {
            endpoint,
            route: HttpBitcoinCoreRoute::ExactHttpsBasic,
            request_timeout: DEFAULT_RPC_TIMEOUT,
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
            authorization: Some(authorization),
        })
    }

    /// Loads bounded Bitcoin Core cookie credentials from an owner-private regular file.
    ///
    /// Symlinks and files accessible by group or other users are rejected. Exactly one
    /// trailing LF or CRLF is ignored; the remainder must be visible ASCII in nonempty
    /// `username:password` form. The path and credential are not retained.
    ///
    /// # Errors
    ///
    /// Rejects unreadable, oversized, non-regular, symlinked, insufficiently private,
    /// or malformed cookie files without including the path or credential in the error.
    pub fn with_cookie_file(
        mut self,
        path: impl AsRef<Path>,
    ) -> Result<Self, HttpBitcoinCoreError> {
        if self.route != HttpBitcoinCoreRoute::LiteralLoopback {
            return Err(HttpBitcoinCoreError::NonLoopbackEndpoint);
        }
        let credential =
            read_private_basic_credentials(path.as_ref()).map_err(|error| match error {
                PrivateBasicCredentialsError::Insecure => HttpBitcoinCoreError::InsecureCookieFile,
                PrivateBasicCredentialsError::Invalid => HttpBitcoinCoreError::InvalidCookieFile,
            })?;
        self.authorization = Some(
            basic_authorization(&credential)
                .map_err(|()| HttpBitcoinCoreError::InvalidCookieFile)?,
        );
        Ok(self)
    }

    /// Replaces the finite request timeout.
    #[must_use]
    pub const fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Replaces the finite concurrent-request bound.
    #[must_use]
    pub const fn with_max_concurrent_requests(mut self, maximum: usize) -> Self {
        self.max_concurrent_requests = maximum;
        self
    }
}

/// Bounded HTTP implementation of [`BitcoinCoreRpc`].
///
/// Local routes use literal-loopback HTTP. Exact public gateways use the pinned
/// jsonrpsee Rustls/platform-verifier TLS stack. No redirect, retry, proxy, or
/// failover middleware is installed. Each trait method performs exactly one
/// JSON-RPC call.
#[derive(Clone)]
pub struct HttpBitcoinCoreRpc {
    client: HttpClient<RpcLogger<RpcService<AcceptJsonRpc1<HttpBackend>>>>,
    route: HttpBitcoinCoreRoute,
}

impl std::fmt::Debug for HttpBitcoinCoreRpc {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpBitcoinCoreRpc")
            .field("route", &self.route.label())
            .finish_non_exhaustive()
    }
}

/// Structured HTTP client, credential, or request failure.
#[derive(Debug, thiserror::Error)]
pub enum HttpBitcoinCoreError {
    /// The endpoint was not an exact, bounded loopback HTTP root with a nonzero port.
    #[error("Bitcoin Core endpoint must be literal loopback HTTP with an explicit nonzero port")]
    NonLoopbackEndpoint,
    /// The public endpoint was not one exact canonical allowlisted HTTPS DNS origin.
    #[error("Bitcoin Core HTTPS endpoint must exactly match one canonical allowlisted origin")]
    NonAllowlistedHttpsEndpoint,
    /// A timeout or concurrency bound was zero.
    #[error("Bitcoin Core HTTP timeout and concurrency limits must be nonzero")]
    InvalidTransportBounds,
    /// The cookie was not a stable owner-private regular file.
    #[error("Bitcoin Core cookie file must be owner-private, regular, and non-symlinked")]
    InsecureCookieFile,
    /// Cookie reading or bounded credential validation failed.
    #[error("Bitcoin Core cookie file is unreadable or invalid")]
    InvalidCookieFile,
    /// Public Basic credentials were not in a stable owner-private regular file.
    #[error("Bitcoin Core Basic credentials must be owner-private, regular, and non-symlinked")]
    InsecureBasicCredentialsFile,
    /// Public Basic credentials were unreadable, oversized, or malformed.
    #[error("Bitcoin Core Basic credentials are unreadable or invalid")]
    InvalidBasicCredentialsFile,
    /// The concrete HTTP route is incompatible with the selected chain profile.
    #[error("Bitcoin Core HTTP route is incompatible with the selected chain profile")]
    RouteProfileMismatch,
    /// A block's transaction was not the hex the node said it was.
    #[error("Bitcoin Core returned a block transaction that is not hex")]
    MalformedResponse,
    /// The client was not configured from a private credential file.
    #[error("Bitcoin Core HTTP requires file-backed Basic credentials")]
    MissingCookieCredentials,
    /// The bounded HTTP client could not be constructed.
    #[error("failed to construct bounded Bitcoin Core HTTP client")]
    Build(#[source] ClientError),
    /// One JSON-RPC request failed.
    #[error("Bitcoin Core JSON-RPC request failed")]
    Request(#[source] ClientError),
    /// An outgoing transaction violated the explicit raw transaction bound.
    #[error("outgoing Bitcoin transaction is empty or oversized")]
    MalformedOutgoingTransaction,
}

impl HttpBitcoinCoreRpc {
    /// Constructs a bounded client without opening a connection.
    ///
    /// # Errors
    ///
    /// Rejects disabled finite bounds, altered endpoints, or client build failures.
    pub fn connect(config: &HttpBitcoinCoreConfig) -> Result<Self, HttpBitcoinCoreError> {
        if config.request_timeout.is_zero()
            || config.request_timeout > MAX_RPC_TIMEOUT
            || config.max_concurrent_requests == 0
            || config.max_concurrent_requests > MAX_CONCURRENT_REQUESTS
        {
            return Err(HttpBitcoinCoreError::InvalidTransportBounds);
        }
        let route_is_valid = match config.route {
            HttpBitcoinCoreRoute::LiteralLoopback => is_literal_loopback_endpoint(&config.endpoint),
            HttpBitcoinCoreRoute::ExactHttpsBasic => is_exact_https_origin(&config.endpoint),
        };
        if !route_is_valid {
            return Err(match config.route {
                HttpBitcoinCoreRoute::LiteralLoopback => HttpBitcoinCoreError::NonLoopbackEndpoint,
                HttpBitcoinCoreRoute::ExactHttpsBasic => {
                    HttpBitcoinCoreError::NonAllowlistedHttpsEndpoint
                }
            });
        }
        let mut headers = HeaderMap::new();
        let authorization = config
            .authorization
            .as_ref()
            .ok_or(HttpBitcoinCoreError::MissingCookieCredentials)?;
        headers.insert("authorization", authorization.clone());
        let client = HttpClientBuilder::default()
            .max_request_size(MAX_RPC_REQUEST_BYTES)
            .max_response_size(MAX_RPC_RESPONSE_BYTES)
            .request_timeout(config.request_timeout)
            .max_concurrent_requests(config.max_concurrent_requests)
            .set_headers(headers)
            .set_http_middleware(tower::ServiceBuilder::new().layer_fn(AcceptJsonRpc1))
            .build(&config.endpoint)
            .map_err(HttpBitcoinCoreError::Build)?;
        Ok(Self {
            client,
            route: config.route,
        })
    }

    /// Constructs an adapter only when the concrete HTTP route matches the chain profile.
    ///
    /// Literal loopback is valid for both Regtest profiles and self-hosted Testnet4.
    /// Exact HTTPS is valid only for Testnet4. Construction performs no RPC.
    ///
    /// # Errors
    ///
    /// Rejects invalid transport bounds, missing credentials, client construction
    /// failures, and every route/profile mismatch before network I/O.
    pub fn connect_profiled(
        config: &HttpBitcoinCoreConfig,
        connectivity: CoreConnectivityPolicy,
    ) -> Result<BitcoinCoreAdapter<Self>, HttpBitcoinCoreError> {
        let adapter = BitcoinCoreAdapter::new(Self::connect(config)?, connectivity);
        adapter
            .ensure_route_compatible()
            .map_err(|_| HttpBitcoinCoreError::RouteProfileMismatch)?;
        Ok(adapter)
    }
}

#[async_trait]
impl BitcoinCoreRpc for HttpBitcoinCoreRpc {
    type Error = HttpBitcoinCoreError;

    async fn get_network_info(&self) -> Result<GetNetworkInfo, Self::Error> {
        self.client
            .request("getnetworkinfo", rpc_params![])
            .await
            .map_err(HttpBitcoinCoreError::Request)
    }

    async fn get_blockchain_info(&self) -> Result<GetBlockchainInfo, Self::Error> {
        self.client
            .request("getblockchaininfo", rpc_params![])
            .await
            .map_err(HttpBitcoinCoreError::Request)
    }

    async fn get_genesis_hash(&self) -> Result<GetBlockHash, Self::Error> {
        self.client
            .request("getblockhash", rpc_params![0_u32])
            .await
            .map_err(HttpBitcoinCoreError::Request)
    }

    async fn get_index_info(&self) -> Result<GetIndexInfo, Self::Error> {
        self.client
            .request("getindexinfo", rpc_params![])
            .await
            .map_err(HttpBitcoinCoreError::Request)
    }

    async fn get_raw_transaction(
        &self,
        transaction_id: Txid,
    ) -> Result<Option<GetRawTransactionVerbose>, Self::Error> {
        let response: Result<GetRawTransactionVerbose, ClientError> = self
            .client
            .request(
                "getrawtransaction",
                rpc_params![transaction_id.to_string(), true],
            )
            .await;
        optional_call(response)
    }

    async fn get_block_header(
        &self,
        block_hash: BlockHash,
    ) -> Result<GetBlockHeaderVerbose, Self::Error> {
        self.client
            .request("getblockheader", rpc_params![block_hash.to_string(), true])
            .await
            .map_err(HttpBitcoinCoreError::Request)
    }

    async fn get_tx_spending_prevout(
        &self,
        outpoint: OutPoint,
    ) -> Result<GetTxSpendingPrevout, Self::Error> {
        let outpoints = vec![serde_json::json!({
            "txid": outpoint.txid.to_string(),
            "vout": outpoint.vout
        })];
        self.client
            .request(
                "gettxspendingprevout",
                rpc_params![
                    outpoints,
                    serde_json::json!({
                        "mempool_only": false,
                        "return_spending_tx": true
                    })
                ],
            )
            .await
            .map_err(HttpBitcoinCoreError::Request)
    }

    async fn get_mempool_spender(&self, outpoint: OutPoint) -> Result<Option<Txid>, Self::Error> {
        // No options object: it is Core 31's, and an older node answers a call
        // that carries one with its help text.
        let outpoints = vec![serde_json::json!({
            "txid": outpoint.txid.to_string(),
            "vout": outpoint.vout
        })];
        let answer: Vec<MempoolSpender> = self
            .client
            .request("gettxspendingprevout", rpc_params![outpoints])
            .await
            .map_err(HttpBitcoinCoreError::Request)?;
        Ok(answer.into_iter().next().and_then(|item| item.spendingtxid))
    }

    async fn is_unspent(&self, outpoint: OutPoint) -> Result<bool, Self::Error> {
        let output: Option<serde_json::Value> = self
            .client
            .request(
                "gettxout",
                rpc_params![outpoint.txid.to_string(), outpoint.vout, false],
            )
            .await
            .map_err(HttpBitcoinCoreError::Request)?;
        Ok(output.is_some())
    }

    async fn get_block_transactions(
        &self,
        height: u32,
    ) -> Result<(BlockHash, Vec<Vec<u8>>), Self::Error> {
        let hash: BlockHash = self
            .client
            .request("getblockhash", rpc_params![height])
            .await
            .map_err(HttpBitcoinCoreError::Request)?;
        let block: BlockWithTransactions = self
            .client
            .request("getblock", rpc_params![hash.to_string(), 2])
            .await
            .map_err(HttpBitcoinCoreError::Request)?;
        let transactions = block
            .tx
            .into_iter()
            .map(|transaction| hex::decode(transaction.hex))
            .collect::<Result<_, _>>()
            .map_err(|_| HttpBitcoinCoreError::MalformedResponse)?;
        Ok((hash, transactions))
    }

    async fn test_mempool_accept(
        &self,
        transaction: &[u8],
    ) -> Result<TestMempoolAccept, Self::Error> {
        require_outgoing_transaction(transaction)?;
        self.client
            .request(
                "testmempoolaccept",
                rpc_params![vec![hex::encode(transaction)]],
            )
            .await
            .map_err(HttpBitcoinCoreError::Request)
    }

    async fn send_raw_transaction(
        &self,
        transaction: &[u8],
    ) -> Result<SendRawTransaction, Self::Error> {
        require_outgoing_transaction(transaction)?;
        self.client
            .request("sendrawtransaction", rpc_params![hex::encode(transaction)])
            .await
            .map_err(HttpBitcoinCoreError::Request)
    }

    fn deployment_route(&self) -> Option<CoreRpcRoute> {
        Some(self.route.rpc_route())
    }

    fn classify_send_failure(error: &Self::Error) -> SendFailure {
        match error {
            HttpBitcoinCoreError::Request(ClientError::Call(error))
                if matches!(error.code(), -22 | -25 | -26) =>
            {
                SendFailure::DefinitiveRejection
            }
            _ => SendFailure::Unknown,
        }
    }
}

fn optional_call<T>(response: Result<T, ClientError>) -> Result<Option<T>, HttpBitcoinCoreError> {
    match response {
        Ok(value) => Ok(Some(value)),
        Err(ClientError::Call(error)) if error.code() == TRANSACTION_NOT_FOUND_CODE => Ok(None),
        Err(error) => Err(HttpBitcoinCoreError::Request(error)),
    }
}

fn require_outgoing_transaction(transaction: &[u8]) -> Result<(), HttpBitcoinCoreError> {
    if transaction.is_empty() || transaction.len() > MAX_RAW_TRANSACTION_BYTES {
        return Err(HttpBitcoinCoreError::MalformedOutgoingTransaction);
    }
    Ok(())
}

fn is_literal_loopback_endpoint(endpoint: &str) -> bool {
    if endpoint.len() > MAX_RPC_ENDPOINT_BYTES {
        return false;
    }
    let Ok(parsed) = Url::parse(endpoint) else {
        return false;
    };
    if parsed.scheme() != "http"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return false;
    }
    let Some(port) = parsed.port() else {
        return false;
    };
    if port == 0 {
        return false;
    }
    let canonical = match parsed.host() {
        Some(Host::Ipv4(address)) if address.is_loopback() => {
            format!("http://127.0.0.1:{port}")
        }
        Some(Host::Ipv6(address)) if address.is_loopback() => format!("http://[::1]:{port}"),
        _ => return false,
    };
    endpoint == canonical || endpoint.strip_suffix('/') == Some(canonical.as_str())
}

fn is_exact_https_origin(endpoint: &str) -> bool {
    if endpoint.len() > MAX_RPC_ENDPOINT_BYTES {
        return false;
    }
    let Ok(parsed) = Url::parse(endpoint) else {
        return false;
    };
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.port().is_some()
    {
        return false;
    }
    let Some(Host::Domain(domain)) = parsed.host() else {
        return false;
    };
    if domain == "localhost"
        || domain.ends_with(".localhost")
        || domain.starts_with("*.")
        || !domain.contains('.')
    {
        return false;
    }
    endpoint == format!("https://{domain}/")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrivateBasicCredentialsError {
    Insecure,
    Invalid,
}

fn basic_authorization(credential: &[u8]) -> Result<HeaderValue, ()> {
    let encoded = Zeroizing::new(BASE64_STANDARD.encode(credential));
    let mut header = Zeroizing::new(Vec::with_capacity(6_usize.saturating_add(encoded.len())));
    header.extend_from_slice(b"Basic ");
    header.extend_from_slice(encoded.as_bytes());
    let mut authorization = HeaderValue::from_bytes(header.as_slice()).map_err(|_| ())?;
    authorization.set_sensitive(true);
    Ok(authorization)
}

fn read_private_basic_credentials(
    path: &Path,
) -> Result<Zeroizing<Vec<u8>>, PrivateBasicCredentialsError> {
    let path_metadata =
        fs::symlink_metadata(path).map_err(|_| PrivateBasicCredentialsError::Invalid)?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(PrivateBasicCredentialsError::Insecure);
    }
    #[cfg(not(unix))]
    {
        let _ = path_metadata;
        return Err(PrivateBasicCredentialsError::Insecure);
    }
    #[cfg(unix)]
    {
        if !private_basic_metadata_is_valid(&path_metadata) {
            return Err(PrivateBasicCredentialsError::Insecure);
        }
        let file = File::open(path).map_err(|_| PrivateBasicCredentialsError::Invalid)?;
        let opened_metadata = file
            .metadata()
            .map_err(|_| PrivateBasicCredentialsError::Invalid)?;
        if !private_basic_metadata_is_valid(&opened_metadata)
            || !same_unchanged_file(&path_metadata, &opened_metadata)
        {
            return Err(PrivateBasicCredentialsError::Insecure);
        }
        let mut raw = Zeroizing::new(Vec::with_capacity(
            MAX_BASIC_CREDENTIAL_FILE_BYTES.saturating_add(1),
        ));
        (&file)
            .take((MAX_BASIC_CREDENTIAL_FILE_BYTES + 1) as u64)
            .read_to_end(raw.as_mut())
            .map_err(|_| PrivateBasicCredentialsError::Invalid)?;
        if raw.len() > MAX_BASIC_CREDENTIAL_FILE_BYTES {
            return Err(PrivateBasicCredentialsError::Invalid);
        }
        let opened_after = file
            .metadata()
            .map_err(|_| PrivateBasicCredentialsError::Invalid)?;
        let path_after =
            fs::symlink_metadata(path).map_err(|_| PrivateBasicCredentialsError::Invalid)?;
        if !private_basic_metadata_is_valid(&opened_after)
            || !private_basic_metadata_is_valid(&path_after)
            || !same_unchanged_file(&opened_metadata, &opened_after)
            || !same_unchanged_file(&opened_metadata, &path_after)
        {
            return Err(PrivateBasicCredentialsError::Insecure);
        }
        validate_basic_credentials(&raw)
    }
}

#[cfg(unix)]
fn private_basic_metadata_is_valid(metadata: &fs::Metadata) -> bool {
    metadata.is_file() && metadata.permissions().mode() & 0o7777 == 0o600 && metadata.nlink() == 1
}

#[cfg(unix)]
fn same_unchanged_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.permissions().mode() == right.permissions().mode()
        && left.nlink() == right.nlink()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

fn validate_basic_credentials(
    raw: &[u8],
) -> Result<Zeroizing<Vec<u8>>, PrivateBasicCredentialsError> {
    let credential = raw
        .strip_suffix(b"\r\n")
        .or_else(|| raw.strip_suffix(b"\n"))
        .unwrap_or(raw);
    let delimiter = credential.iter().position(|byte| *byte == b':');
    if credential.is_empty()
        || delimiter.is_none_or(|index| index == 0 || index + 1 == credential.len())
        || !credential.iter().all(|byte| (0x21..=0x7e).contains(byte))
    {
        return Err(PrivateBasicCredentialsError::Invalid);
    }
    Ok(Zeroizing::new(credential.to_vec()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    use super::HttpBitcoinCoreConfig;

    #[test]
    fn basic_authorization_header_is_sensitive() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cookie");
        fs::write(&path, b"user:secret\r\n").expect("cookie file");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("owner-only cookie mode");
        let config = HttpBitcoinCoreConfig::new("http://127.0.0.1:18443")
            .expect("loopback endpoint")
            .with_cookie_file(path)
            .expect("valid credential");
        assert!(
            config
                .authorization
                .as_ref()
                .expect("authorization")
                .is_sensitive()
        );
    }
}

#[cfg(test)]
mod envelope_tests {
    use super::json_rpc_2_envelope;

    fn normalized(reply: &str) -> serde_json::Value {
        serde_json::from_slice(&json_rpc_2_envelope(reply.as_bytes().to_vec())).expect("JSON")
    }

    #[test]
    fn every_envelope_a_public_provider_was_seen_to_send_becomes_json_rpc_2() {
        let expected = serde_json::json!({"jsonrpc": "2.0", "result": 7, "id": 1});
        // The 1.x envelope, and the same with an empty member: both seen from
        // one provider's backends within a single run.
        assert_eq!(normalized(r#"{"result":7,"error":null,"id":1}"#), expected);
        assert_eq!(normalized(r#"{"jsonrpc":"","result":7,"id":1}"#), expected);
        assert_eq!(
            normalized(r#"{"result":null,"error":{"code":-5,"message":"no"},"id":1}"#),
            serde_json::json!({"jsonrpc": "2.0", "error": {"code": -5, "message": "no"}, "id": 1})
        );
        // A 2.0 reply, and anything that is not an envelope, are not touched.
        for untouched in [
            r#"{"jsonrpc":"2.0","result":0.00001000,"id":1}"#,
            "not json",
        ] {
            assert_eq!(
                json_rpc_2_envelope(untouched.as_bytes().to_vec()),
                untouched.as_bytes()
            );
        }
    }
}
