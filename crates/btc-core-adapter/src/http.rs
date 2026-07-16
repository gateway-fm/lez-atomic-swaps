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
use jsonrpsee_http_client::{HeaderMap, HeaderValue, HttpClient, HttpClientBuilder};
use url::{Host, Url};
use zeroize::Zeroizing;

use crate::{BitcoinCoreRpc, MAX_RAW_TRANSACTION_BYTES, SendFailure};

const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_CONCURRENT_REQUESTS: usize = 1;
const MAX_RPC_TIMEOUT: Duration = Duration::from_mins(5);
const MAX_CONCURRENT_REQUESTS: usize = 16;
const MAX_RPC_ENDPOINT_BYTES: usize = 2_048;
const MAX_COOKIE_FILE_BYTES: usize = 1_024;
const MAX_RPC_REQUEST_BYTES: u32 = 2_100_000;
const MAX_RPC_RESPONSE_BYTES: u32 = 4_100_000;
const TRANSACTION_NOT_FOUND_CODE: i32 = -5;

/// Finite, loopback-only Bitcoin Core HTTP JSON-RPC configuration.
#[derive(Clone, Eq, PartialEq)]
pub struct HttpBitcoinCoreConfig {
    endpoint: Box<str>,
    request_timeout: Duration,
    max_concurrent_requests: usize,
    authorization: Option<HeaderValue>,
}

impl std::fmt::Debug for HttpBitcoinCoreConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpBitcoinCoreConfig")
            .field("request_timeout", &self.request_timeout)
            .field("max_concurrent_requests", &self.max_concurrent_requests)
            .field("cookie_auth_enabled", &self.authorization.is_some())
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
            request_timeout: DEFAULT_RPC_TIMEOUT,
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
            authorization: None,
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
        let credential = read_private_cookie(path.as_ref())?;
        let encoded = Zeroizing::new(BASE64_STANDARD.encode(credential.as_slice()));
        let mut header = Zeroizing::new(Vec::with_capacity(6_usize.saturating_add(encoded.len())));
        header.extend_from_slice(b"Basic ");
        header.extend_from_slice(encoded.as_bytes());
        let mut authorization = HeaderValue::from_bytes(header.as_slice())
            .map_err(|_| HttpBitcoinCoreError::InvalidCookieFile)?;
        authorization.set_sensitive(true);
        self.authorization = Some(authorization);
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

/// Bounded loopback HTTP implementation of [`BitcoinCoreRpc`].
///
/// The pinned client has TLS disabled and installs no redirect, retry, proxy, or
/// public-endpoint middleware. Each trait method performs exactly one JSON-RPC call.
#[derive(Clone)]
pub struct HttpBitcoinCoreRpc {
    client: HttpClient,
}

impl std::fmt::Debug for HttpBitcoinCoreRpc {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpBitcoinCoreRpc")
            .finish_non_exhaustive()
    }
}

/// Structured local HTTP client, credential, or request failure.
#[derive(Debug, thiserror::Error)]
pub enum HttpBitcoinCoreError {
    /// The endpoint was not an exact, bounded loopback HTTP root with a nonzero port.
    #[error("Bitcoin Core endpoint must be literal loopback HTTP with an explicit nonzero port")]
    NonLoopbackEndpoint,
    /// A timeout or concurrency bound was zero.
    #[error("Bitcoin Core HTTP timeout and concurrency limits must be nonzero")]
    InvalidTransportBounds,
    /// The cookie was not a stable owner-private regular file.
    #[error("Bitcoin Core cookie file must be owner-private, regular, and non-symlinked")]
    InsecureCookieFile,
    /// Cookie reading or bounded credential validation failed.
    #[error("Bitcoin Core cookie file is unreadable or invalid")]
    InvalidCookieFile,
    /// The client was not configured from a private credential file.
    #[error("Bitcoin Core HTTP requires file-backed Basic credentials")]
    MissingCookieCredentials,
    /// The bounded local HTTP client could not be constructed.
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
    /// Constructs a local client without opening a connection.
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
        if !is_literal_loopback_endpoint(&config.endpoint) {
            return Err(HttpBitcoinCoreError::NonLoopbackEndpoint);
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
            .build(&config.endpoint)
            .map_err(HttpBitcoinCoreError::Build)?;
        Ok(Self { client })
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

fn read_private_cookie(path: &Path) -> Result<Zeroizing<Vec<u8>>, HttpBitcoinCoreError> {
    let path_metadata =
        fs::symlink_metadata(path).map_err(|_| HttpBitcoinCoreError::InvalidCookieFile)?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(HttpBitcoinCoreError::InsecureCookieFile);
    }
    #[cfg(not(unix))]
    {
        let _ = path_metadata;
        return Err(HttpBitcoinCoreError::InsecureCookieFile);
    }
    #[cfg(unix)]
    {
        if !private_cookie_metadata_is_valid(&path_metadata) {
            return Err(HttpBitcoinCoreError::InsecureCookieFile);
        }
        let file = File::open(path).map_err(|_| HttpBitcoinCoreError::InvalidCookieFile)?;
        let opened_metadata = file
            .metadata()
            .map_err(|_| HttpBitcoinCoreError::InvalidCookieFile)?;
        if !private_cookie_metadata_is_valid(&opened_metadata)
            || !same_unchanged_file(&path_metadata, &opened_metadata)
        {
            return Err(HttpBitcoinCoreError::InsecureCookieFile);
        }
        let mut raw = Zeroizing::new(Vec::with_capacity(MAX_COOKIE_FILE_BYTES.saturating_add(1)));
        (&file)
            .take((MAX_COOKIE_FILE_BYTES + 1) as u64)
            .read_to_end(raw.as_mut())
            .map_err(|_| HttpBitcoinCoreError::InvalidCookieFile)?;
        if raw.len() > MAX_COOKIE_FILE_BYTES {
            return Err(HttpBitcoinCoreError::InvalidCookieFile);
        }
        let opened_after = file
            .metadata()
            .map_err(|_| HttpBitcoinCoreError::InvalidCookieFile)?;
        let path_after =
            fs::symlink_metadata(path).map_err(|_| HttpBitcoinCoreError::InvalidCookieFile)?;
        if !private_cookie_metadata_is_valid(&opened_after)
            || !private_cookie_metadata_is_valid(&path_after)
            || !same_unchanged_file(&opened_metadata, &opened_after)
            || !same_unchanged_file(&opened_metadata, &path_after)
        {
            return Err(HttpBitcoinCoreError::InsecureCookieFile);
        }
        validate_cookie(&raw)
    }
}

#[cfg(unix)]
fn private_cookie_metadata_is_valid(metadata: &fs::Metadata) -> bool {
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

fn validate_cookie(raw: &[u8]) -> Result<Zeroizing<Vec<u8>>, HttpBitcoinCoreError> {
    let credential = raw
        .strip_suffix(b"\r\n")
        .or_else(|| raw.strip_suffix(b"\n"))
        .unwrap_or(raw);
    let delimiter = credential.iter().position(|byte| *byte == b':');
    if credential.is_empty()
        || delimiter.is_none_or(|index| index == 0 || index + 1 == credential.len())
        || !credential.iter().all(|byte| (0x21..=0x7e).contains(byte))
    {
        return Err(HttpBitcoinCoreError::InvalidCookieFile);
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
