//! Run-bound authentication and isolation proof for the local Monero Regtest.
//!
//! `monero-rpc` remains the typed chain/output client. Version 0.5.1 does not
//! expose the daemon's isolation fields, so this module adds only two bounded,
//! typed JSON-RPC reads: `get_info` and `get_connections`. Responses are capped
//! while streaming, reject recognized duplicate fields through Serde's struct
//! visitor, and are validated before they can mint an attestation.

use std::fmt;

use diqwest::WithDigestAuth;
use lez_bridge_protocol::RunId;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE};
use reqwest::{Response, StatusCode};
use serde::Deserialize;
use serde::de::{IgnoredAny, SeqAccess, Visitor};
use thiserror::Error;

use super::{
    LoopbackRpcEndpoint, MoneroChainIdentity, MoneroNetwork, RPC_REQUEST_TIMEOUT,
    VerifiedMoneroOutputObservation,
};

const RPC_ID: &str = "lez-m4-topology-v1";
const JSON_RPC_PATH: &str = "/json_rpc";
const MAX_TOPOLOGY_RESPONSE_BYTES: usize = 64 * 1024;
const GET_INFO_REQUEST: &str = r#"{"jsonrpc":"2.0","id":"lez-m4-topology-v1","method":"get_info"}"#;
const GET_CONNECTIONS_REQUEST: &str =
    r#"{"jsonrpc":"2.0","id":"lez-m4-topology-v1","method":"get_connections"}"#;
const GET_VERSION_REQUEST: &str =
    r#"{"jsonrpc":"2.0","id":"lez-m4-topology-v1","method":"get_version"}"#;

/// Verifies one exact run's isolated Monero daemon and wallet topology.
pub struct MoneroTopologyVerifier<'endpoint> {
    run_id: RunId,
    identity: MoneroChainIdentity,
    daemon: &'endpoint LoopbackRpcEndpoint,
    target_wallet: &'endpoint LoopbackRpcEndpoint,
    foreign_wallet: &'endpoint LoopbackRpcEndpoint,
    client: reqwest::Client,
}

impl<'endpoint> MoneroTopologyVerifier<'endpoint> {
    /// Creates an exact three-origin local Regtest topology verifier.
    ///
    /// The foreign wallet contributes credentials that must work at its own
    /// origin and finish with HTTP 401 when replayed against `target_wallet`.
    ///
    /// # Errors
    ///
    /// Rejects non-Regtest profiles, aliased origins, or HTTP-client failures.
    pub fn new(
        run_id: RunId,
        identity: MoneroChainIdentity,
        daemon: &'endpoint LoopbackRpcEndpoint,
        target_wallet: &'endpoint LoopbackRpcEndpoint,
        foreign_wallet: &'endpoint LoopbackRpcEndpoint,
    ) -> Result<Self, MoneroTopologyError> {
        if identity.network() != MoneroNetwork::Regtest {
            return Err(MoneroTopologyError::RequiresRegtest);
        }
        if daemon.base_url == target_wallet.base_url
            || daemon.base_url == foreign_wallet.base_url
            || target_wallet.base_url == foreign_wallet.base_url
        {
            return Err(MoneroTopologyError::AliasedRpcOrigins);
        }
        let client = reqwest::Client::builder()
            .timeout(RPC_REQUEST_TIMEOUT)
            .build()
            .map_err(|_| MoneroTopologyError::HttpClientBuild)?;
        Ok(Self {
            run_id,
            identity,
            daemon,
            target_wallet,
            foreign_wallet,
            client,
        })
    }

    /// Proves the exact daemon profile, empty peer set, wallet liveness, and
    /// cross-wallet Digest authentication rejection.
    ///
    /// # Errors
    ///
    /// Fails closed on transport/status/envelope/body-bound failure, network or
    /// genesis drift, any daemon connection, untrusted data, wallet-version
    /// failure, or failure to finish the foreign-credential request with 401.
    pub async fn verify(&self) -> Result<VerifiedMoneroTopologyAttestation, MoneroTopologyError> {
        let info: GetInfoResult = self
            .call_success(
                "daemon.get_info",
                self.daemon,
                self.daemon,
                GET_INFO_REQUEST,
            )
            .await?;
        validate_info(&info)?;

        let connections: GetConnectionsResult = self
            .call_success(
                "daemon.get_connections",
                self.daemon,
                self.daemon,
                GET_CONNECTIONS_REQUEST,
            )
            .await?;
        validate_connections(&connections)?;

        let target_version: GetVersionResult = self
            .call_success(
                "target_wallet.get_version",
                self.target_wallet,
                self.target_wallet,
                GET_VERSION_REQUEST,
            )
            .await?;
        validate_wallet_version("target_wallet.get_version", &target_version)?;

        let foreign_version: GetVersionResult = self
            .call_success(
                "foreign_wallet.get_version",
                self.foreign_wallet,
                self.foreign_wallet,
                GET_VERSION_REQUEST,
            )
            .await?;
        validate_wallet_version("foreign_wallet.get_version", &foreign_version)?;

        let crossed = self
            .send(self.target_wallet, self.foreign_wallet, GET_VERSION_REQUEST)
            .await?;
        if crossed.status() != StatusCode::UNAUTHORIZED {
            return Err(MoneroTopologyError::ForeignCredentialNotRejected {
                actual: crossed.status().as_u16(),
            });
        }

        let genesis = self
            .daemon
            .client()
            .map_err(|_| MoneroTopologyError::TypedDaemonClientBuild)?
            .daemon()
            .on_get_block_hash(0)
            .await
            .map_err(|_| MoneroTopologyError::GenesisRpc)?;
        if genesis.as_ref() != self.identity.genesis_hash() {
            return Err(MoneroTopologyError::GenesisMismatch);
        }

        Ok(VerifiedMoneroTopologyAttestation {
            run_id: self.run_id.clone(),
            identity: self.identity,
            daemon_origin: self.daemon.base_url.clone(),
            target_wallet_origin: self.target_wallet.base_url.clone(),
            foreign_wallet_origin: self.foreign_wallet.base_url.clone(),
            daemon_version: info.version,
            target_wallet_version: target_version.version,
            foreign_wallet_version: foreign_version.version,
            offline: info.offline,
            incoming_connections: info.incoming_connections_count,
            outgoing_connections: info.outgoing_connections_count,
            _non_forgeable: private::TopologySeal,
        })
    }

    async fn call_success<T: for<'de> Deserialize<'de>>(
        &self,
        operation: &'static str,
        target: &LoopbackRpcEndpoint,
        credentials: &LoopbackRpcEndpoint,
        body: &'static str,
    ) -> Result<T, MoneroTopologyError> {
        let response = self.send(target, credentials, body).await?;
        if response.status() != StatusCode::OK {
            return Err(MoneroTopologyError::UnexpectedHttpStatus {
                operation,
                actual: response.status().as_u16(),
            });
        }
        let bytes = read_bounded(operation, response).await?;
        let envelope: RpcEnvelope<T> = serde_json::from_slice(&bytes)
            .map_err(|_| MoneroTopologyError::MalformedRpcEnvelope { operation })?;
        envelope.into_result(operation)
    }

    async fn send(
        &self,
        target: &LoopbackRpcEndpoint,
        credentials: &LoopbackRpcEndpoint,
        body: &'static str,
    ) -> Result<Response, MoneroTopologyError> {
        self.client
            .post(format!("{}{JSON_RPC_PATH}", target.base_url))
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send_with_digest_auth(&credentials.username, &credentials.password)
            .await
            .map_err(|_| MoneroTopologyError::HttpTransport)
    }
}

impl fmt::Debug for MoneroTopologyVerifier<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoneroTopologyVerifier")
            .field("run_id", &self.run_id)
            .field("identity", &self.identity)
            .field("daemon_origin", &self.daemon.base_url)
            .field("target_wallet_origin", &self.target_wallet.base_url)
            .field("foreign_wallet_origin", &self.foreign_wallet.base_url)
            .field("credentials", &"[REDACTED]")
            .finish()
    }
}

/// Non-forgeable proof of one exact local Regtest topology and auth boundary.
///
/// Fields are private, no public constructor or status-validation port exists,
/// and the type deliberately does not implement `Clone`.
#[derive(Debug, Eq, PartialEq)]
#[must_use]
pub struct VerifiedMoneroTopologyAttestation {
    run_id: RunId,
    identity: MoneroChainIdentity,
    daemon_origin: String,
    target_wallet_origin: String,
    foreign_wallet_origin: String,
    daemon_version: String,
    target_wallet_version: u32,
    foreign_wallet_version: u32,
    offline: bool,
    incoming_connections: u64,
    outgoing_connections: u64,
    _non_forgeable: private::TopologySeal,
}

impl VerifiedMoneroTopologyAttestation {
    /// Exact actor/sidecar run identifier.
    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Exact verified Regtest chain identity.
    #[must_use]
    pub const fn chain_identity(&self) -> MoneroChainIdentity {
        self.identity
    }

    /// Exact authenticated daemon origin.
    #[must_use]
    pub fn daemon_origin(&self) -> &str {
        &self.daemon_origin
    }

    /// Exact authenticated target shared-wallet origin.
    #[must_use]
    pub fn target_wallet_origin(&self) -> &str {
        &self.target_wallet_origin
    }

    /// Exact authenticated foreign-wallet origin.
    #[must_use]
    pub fn foreign_wallet_origin(&self) -> &str {
        &self.foreign_wallet_origin
    }

    /// Official daemon version string reported by the isolated node.
    #[must_use]
    pub fn daemon_version(&self) -> &str {
        &self.daemon_version
    }

    /// Target wallet RPC version integer.
    #[must_use]
    pub const fn target_wallet_version(&self) -> u32 {
        self.target_wallet_version
    }

    /// Foreign wallet RPC version integer.
    #[must_use]
    pub const fn foreign_wallet_version(&self) -> u32 {
        self.foreign_wallet_version
    }

    /// Whether the daemon explicitly reported offline operation.
    #[must_use]
    pub const fn offline(&self) -> bool {
        self.offline
    }

    /// Exact verified total peer count.
    #[must_use]
    pub const fn peer_count(&self) -> u64 {
        self.incoming_connections + self.outgoing_connections
    }

    /// Cross-binds this topology to one exact run and output observation.
    ///
    /// # Errors
    ///
    /// Rejects run, chain, daemon-origin, or wallet-origin drift.
    pub fn validate_observation(
        &self,
        run_id: &RunId,
        observation: &VerifiedMoneroOutputObservation,
    ) -> Result<(), MoneroTopologyBindingError> {
        validate_binding(
            &self.run_id,
            self.identity,
            &self.daemon_origin,
            &self.target_wallet_origin,
            run_id,
            observation.network(),
            observation.genesis_hash(),
            observation.daemon_origin(),
            observation.wallet_origin(),
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_binding(
    attested_run: &RunId,
    attested_chain: MoneroChainIdentity,
    attested_daemon: &str,
    attested_wallet: &str,
    run_id: &RunId,
    network: MoneroNetwork,
    genesis_hash: [u8; 32],
    daemon_origin: &str,
    wallet_origin: &str,
) -> Result<(), MoneroTopologyBindingError> {
    if attested_run != run_id {
        return Err(MoneroTopologyBindingError::RunMismatch);
    }
    if attested_chain.network() != network || attested_chain.genesis_hash() != genesis_hash {
        return Err(MoneroTopologyBindingError::ChainMismatch);
    }
    if attested_daemon != daemon_origin {
        return Err(MoneroTopologyBindingError::DaemonOriginMismatch);
    }
    if attested_wallet != wallet_origin {
        return Err(MoneroTopologyBindingError::WalletOriginMismatch);
    }
    Ok(())
}

/// Failure to establish the local Regtest topology and authentication proof.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MoneroTopologyError {
    /// The isolation capability is deliberately local-Regtest-only.
    #[error("Monero topology attestation requires the Regtest profile")]
    RequiresRegtest,
    /// Every trust/auth role must have its own process origin.
    #[error("Monero daemon, target wallet, and foreign wallet origins must be distinct")]
    AliasedRpcOrigins,
    /// The bounded HTTP client could not be built.
    #[error("failed to build bounded Monero topology HTTP client")]
    HttpClientBuild,
    /// A Digest-authenticated HTTP exchange failed.
    #[error("bounded Monero topology HTTP exchange failed")]
    HttpTransport,
    /// A successful typed RPC was not HTTP 200.
    #[error("Monero topology operation `{operation}` returned HTTP {actual}, expected 200")]
    UnexpectedHttpStatus {
        /// Stable non-secret operation label.
        operation: &'static str,
        /// Exact returned status.
        actual: u16,
    },
    /// Foreign credentials were accepted or failed with something other than 401.
    #[error("foreign wallet credential replay returned HTTP {actual}, expected 401")]
    ForeignCredentialNotRejected {
        /// Exact final HTTP status.
        actual: u16,
    },
    /// Content-Length was malformed or ambiguous.
    #[error("Monero topology operation `{operation}` returned invalid Content-Length")]
    InvalidContentLength {
        /// Stable non-secret operation label.
        operation: &'static str,
    },
    /// A response exceeded the hard body cap before JSON decoding.
    #[error("Monero topology operation `{operation}` exceeded its response-body cap")]
    ResponseTooLarge {
        /// Stable non-secret operation label.
        operation: &'static str,
    },
    /// Streaming the bounded body failed.
    #[error("Monero topology operation `{operation}` body stream failed")]
    BodyRead {
        /// Stable non-secret operation label.
        operation: &'static str,
    },
    /// JSON, recognized field types, or recognized duplicate fields were invalid.
    #[error("Monero topology operation `{operation}` returned a malformed typed envelope")]
    MalformedRpcEnvelope {
        /// Stable non-secret operation label.
        operation: &'static str,
    },
    /// JSON-RPC version/id/result/error semantics were invalid.
    #[error("Monero topology operation `{operation}` returned a conflicting RPC envelope")]
    ConflictingRpcEnvelope {
        /// Stable non-secret operation label.
        operation: &'static str,
    },
    /// `get_info` did not prove the exact offline fakechain profile.
    #[error("Monero daemon did not report the exact offline peerless Regtest profile")]
    DaemonProfileMismatch,
    /// `get_connections` did not independently prove an empty connection set.
    #[error("Monero daemon returned a nonempty or untrusted connection set")]
    ConnectionsPresent,
    /// A wallet's correct credentials did not return one release-version response.
    #[error("Monero operation `{operation}` returned an invalid wallet release version")]
    WalletVersionMismatch {
        /// Stable non-secret wallet operation.
        operation: &'static str,
    },
    /// The maintained typed daemon client could not be constructed.
    #[error("failed to build typed Monero daemon client")]
    TypedDaemonClientBuild,
    /// The maintained typed daemon client could not read height zero.
    #[error("typed Monero daemon genesis query failed")]
    GenesisRpc,
    /// Height zero differed from the exact configured identity.
    #[error("Monero daemon genesis differs from the configured Regtest identity")]
    GenesisMismatch,
}

/// Mismatch while binding topology authority to an output observation.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MoneroTopologyBindingError {
    /// The Stage-B run differs from the attested run.
    #[error("Monero topology run does not match the requested run")]
    RunMismatch,
    /// The observed named network or genesis differs.
    #[error("Monero output chain does not match the attested topology")]
    ChainMismatch,
    /// The output used another daemon origin.
    #[error("Monero output daemon origin does not match the attested topology")]
    DaemonOriginMismatch,
    /// The output used another wallet origin.
    #[error("Monero output wallet origin does not match the attested topology")]
    WalletOriginMismatch,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RpcEnvelope<T> {
    jsonrpc: String,
    id: String,
    result: Option<T>,
    error: Option<RpcError>,
}

impl<T> RpcEnvelope<T> {
    fn into_result(self, operation: &'static str) -> Result<T, MoneroTopologyError> {
        if self.jsonrpc != "2.0" || self.id != RPC_ID {
            return Err(MoneroTopologyError::ConflictingRpcEnvelope { operation });
        }
        if self.error.is_some() {
            return Err(MoneroTopologyError::ConflictingRpcEnvelope { operation });
        }
        self.result
            .ok_or(MoneroTopologyError::ConflictingRpcEnvelope { operation })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RpcError {
    #[serde(rename = "code")]
    _code: i64,
    #[serde(rename = "message")]
    _message: String,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Deserialize)]
struct GetInfoResult {
    status: String,
    untrusted: bool,
    nettype: String,
    mainnet: bool,
    testnet: bool,
    stagenet: bool,
    offline: bool,
    incoming_connections_count: u64,
    outgoing_connections_count: u64,
    version: String,
}

#[derive(Debug, Deserialize)]
struct GetConnectionsResult {
    #[serde(rename = "connections")]
    _connections: EmptyArray,
    status: String,
    untrusted: bool,
}

#[derive(Debug, Deserialize)]
struct GetVersionResult {
    release: bool,
    version: u32,
}

#[derive(Debug)]
struct EmptyArray;

impl<'de> Deserialize<'de> for EmptyArray {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct EmptyArrayVisitor;

        impl<'de> Visitor<'de> for EmptyArrayVisitor {
            type Value = EmptyArray;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an empty array")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                if sequence.next_element::<IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::invalid_length(1, &self));
                }
                Ok(EmptyArray)
            }
        }

        deserializer.deserialize_seq(EmptyArrayVisitor)
    }
}

fn validate_info(info: &GetInfoResult) -> Result<(), MoneroTopologyError> {
    if info.status != "OK"
        || info.untrusted
        || info.nettype != "fakechain"
        || info.mainnet
        || info.testnet
        || info.stagenet
        || !info.offline
        || info.incoming_connections_count != 0
        || info.outgoing_connections_count != 0
        || info.version.is_empty()
        || info.version.len() > 128
        || !info
            .version
            .bytes()
            .all(|byte| (0x20..=0x7e).contains(&byte))
    {
        return Err(MoneroTopologyError::DaemonProfileMismatch);
    }
    Ok(())
}

fn validate_connections(connections: &GetConnectionsResult) -> Result<(), MoneroTopologyError> {
    if connections.status != "OK" || connections.untrusted {
        return Err(MoneroTopologyError::ConnectionsPresent);
    }
    Ok(())
}

fn validate_wallet_version(
    operation: &'static str,
    version: &GetVersionResult,
) -> Result<(), MoneroTopologyError> {
    if !version.release || version.version == 0 {
        return Err(MoneroTopologyError::WalletVersionMismatch { operation });
    }
    Ok(())
}

async fn read_bounded(
    operation: &'static str,
    mut response: Response,
) -> Result<Vec<u8>, MoneroTopologyError> {
    let mut lengths = response.headers().get_all(CONTENT_LENGTH).iter();
    if let Some(length) = lengths.next() {
        if lengths.next().is_some() {
            return Err(MoneroTopologyError::InvalidContentLength { operation });
        }
        let length = length
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(MoneroTopologyError::InvalidContentLength { operation })?;
        if length > MAX_TOPOLOGY_RESPONSE_BYTES as u64 {
            return Err(MoneroTopologyError::ResponseTooLarge { operation });
        }
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| MoneroTopologyError::BodyRead { operation })?
    {
        let next = body
            .len()
            .checked_add(chunk.len())
            .ok_or(MoneroTopologyError::ResponseTooLarge { operation })?;
        if next > MAX_TOPOLOGY_RESPONSE_BYTES {
            return Err(MoneroTopologyError::ResponseTooLarge { operation });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

mod private {
    #[derive(Debug, Eq, PartialEq)]
    pub(super) struct TopologySeal;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use digest_auth::{AuthContext, AuthorizationHeader};
    use static_assertions::assert_not_impl_any;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::Mutex;
    use tokio::task::JoinHandle;

    use super::*;

    const USER: &str = "wallet-user";
    const PASSWORD: &str = "wallet-secret";
    const FOREIGN_USER: &str = "foreign-user";
    const FOREIGN_PASSWORD: &str = "foreign-secret";
    const CHALLENGE: &str = "Digest realm=\"monero-rpc\", qop=\"auth\", algorithm=MD5, nonce=\"fixed-test-nonce\", opaque=\"fixed-test-opaque\"";

    #[derive(Clone)]
    struct RpcBehavior {
        info: String,
        connections: String,
        version: String,
        bad_auth_status: u16,
        authorized_status: u16,
    }

    impl RpcBehavior {
        fn valid() -> Self {
            Self {
                info: valid_info(0, 0),
                connections: success(r#"{"connections":[],"status":"OK","untrusted":false}"#),
                version: success(r#"{"release":true,"version":65567}"#),
                bad_auth_status: 401,
                authorized_status: 200,
            }
        }
    }

    struct TestRpcServer {
        endpoint: LoopbackRpcEndpoint,
        task: JoinHandle<()>,
    }

    impl Drop for TestRpcServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn spawn_server(
        username: &'static str,
        password: &'static str,
        behavior: RpcBehavior,
    ) -> TestRpcServer {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind isolated RPC fixture");
        let address = listener.local_addr().expect("fixture local address");
        let behavior = Arc::new(Mutex::new(behavior));
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _peer)) = listener.accept().await else {
                    break;
                };
                let behavior = Arc::clone(&behavior);
                tokio::spawn(async move {
                    let _result = handle_request(stream, username, password, behavior).await;
                });
            }
        });
        let endpoint = LoopbackRpcEndpoint::new(&format!("http://{address}"), username, password)
            .expect("fixture endpoint");
        TestRpcServer { endpoint, task }
    }

    async fn handle_request(
        mut stream: TcpStream,
        username: &str,
        password: &str,
        behavior: Arc<Mutex<RpcBehavior>>,
    ) -> Result<(), std::io::Error> {
        let request = read_request(&mut stream).await?;
        let has_authorization = request.authorization.is_some();
        let authorized = request
            .authorization
            .as_deref()
            .is_some_and(|header| verify_digest(header, username, password, &request.body));
        let behavior = behavior.lock().await.clone();
        if !authorized {
            let (status, challenge) = if has_authorization {
                (behavior.bad_auth_status, String::new())
            } else {
                (401, format!("WWW-Authenticate: {CHALLENGE}\r\n"))
            };
            return write_response(&mut stream, status, &challenge, "").await;
        }
        if behavior.authorized_status != 200 {
            return write_response(&mut stream, behavior.authorized_status, "", "").await;
        }
        let body = if request.body.contains(r#""method":"get_info""#) {
            behavior.info
        } else if request.body.contains(r#""method":"get_connections""#) {
            behavior.connections
        } else if request.body.contains(r#""method":"on_get_block_hash""#) {
            success(&format!("\"{}\"", "01".repeat(32)))
        } else {
            behavior.version
        };
        write_response(
            &mut stream,
            200,
            "Content-Type: application/json\r\n",
            &body,
        )
        .await
    }

    struct HttpRequest {
        authorization: Option<String>,
        body: String,
    }

    async fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, std::io::Error> {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 2_048];
        let header_end = loop {
            let count = stream.read(&mut buffer).await?;
            if count == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
            }
            bytes.extend_from_slice(&buffer[..count]);
            if bytes.len() > 16 * 1024 {
                return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
            }
            if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let headers = std::str::from_utf8(&bytes[..header_end])
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?
            .to_owned();
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
            })
            .unwrap_or(0);
        while bytes.len() < header_end + content_length {
            let count = stream.read(&mut buffer).await?;
            if count == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        let authorization = headers.lines().find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("authorization")
                    .then(|| value.trim().to_owned())
            })
        });
        let body = String::from_utf8(bytes[header_end..header_end + content_length].to_vec())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?;
        Ok(HttpRequest {
            authorization,
            body,
        })
    }

    fn verify_digest(header: &str, username: &str, password: &str, body: &str) -> bool {
        let Ok(mut supplied) = AuthorizationHeader::parse(header) else {
            return false;
        };
        if supplied.username != username || supplied.uri != JSON_RPC_PATH {
            return false;
        }
        let claimed = supplied.response.clone();
        supplied.digest(&AuthContext::new_post(
            username,
            password,
            JSON_RPC_PATH,
            Some(body.as_bytes()),
        ));
        supplied.response == claimed
    }

    async fn write_response(
        stream: &mut TcpStream,
        status: u16,
        extra_headers: &str,
        body: &str,
    ) -> Result<(), std::io::Error> {
        let reason = match status {
            200 => "OK",
            401 => "Unauthorized",
            403 => "Forbidden",
            500 => "Internal Server Error",
            _ => "Fixture",
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await?;
        stream.shutdown().await
    }

    fn success(result: &str) -> String {
        format!(r#"{{"jsonrpc":"2.0","id":"{RPC_ID}","result":{result}}}"#)
    }

    fn valid_info(incoming: u64, outgoing: u64) -> String {
        success(&format!(
            r#"{{"status":"OK","untrusted":false,"nettype":"fakechain","mainnet":false,"testnet":false,"stagenet":false,"offline":true,"incoming_connections_count":{incoming},"outgoing_connections_count":{outgoing},"version":"0.18.5.1-release"}}"#
        ))
    }

    fn run(value: &str) -> RunId {
        RunId::new(value).expect("valid fixture run id")
    }

    fn identity() -> MoneroChainIdentity {
        MoneroChainIdentity::new(MoneroNetwork::Regtest, [1; 32]).expect("fixture Regtest identity")
    }

    async fn valid_servers() -> (TestRpcServer, TestRpcServer, TestRpcServer) {
        let daemon = spawn_server("daemon-user", "daemon-secret", RpcBehavior::valid()).await;
        let target = spawn_server(USER, PASSWORD, RpcBehavior::valid()).await;
        let foreign = spawn_server(FOREIGN_USER, FOREIGN_PASSWORD, RpcBehavior::valid()).await;
        (daemon, target, foreign)
    }

    #[tokio::test]
    async fn valid_capability_proves_exact_run_chain_origins_and_facts() {
        let (daemon, target, foreign) = valid_servers().await;
        let verifier = MoneroTopologyVerifier::new(
            run("topology-valid-01"),
            identity(),
            &daemon.endpoint,
            &target.endpoint,
            &foreign.endpoint,
        )
        .expect("valid verifier");
        let attestation = verifier.verify().await.expect("valid topology proof");

        assert_eq!(attestation.run_id().as_str(), "topology-valid-01");
        assert_eq!(attestation.chain_identity(), identity());
        assert_eq!(attestation.daemon_origin(), daemon.endpoint.base_url());
        assert_eq!(
            attestation.target_wallet_origin(),
            target.endpoint.base_url()
        );
        assert_eq!(
            attestation.foreign_wallet_origin(),
            foreign.endpoint.base_url()
        );
        assert!(attestation.offline());
        assert_eq!(attestation.peer_count(), 0);
        assert_eq!(attestation.target_wallet_version(), 65567);
        assert_eq!(attestation.foreign_wallet_version(), 65567);
    }

    #[tokio::test]
    async fn aliases_and_foreign_origin_drift_fail_before_rpc() {
        let (daemon, target, _foreign) = valid_servers().await;
        assert!(matches!(
            MoneroTopologyVerifier::new(
                run("topology-alias-01"),
                identity(),
                &daemon.endpoint,
                &target.endpoint,
                &target.endpoint,
            ),
            Err(MoneroTopologyError::AliasedRpcOrigins)
        ));
    }

    #[tokio::test]
    async fn correct_credential_failure_never_mints() {
        let (daemon, _target, foreign) = valid_servers().await;
        let mut failed = RpcBehavior::valid();
        failed.authorized_status = 500;
        let target = spawn_server(USER, PASSWORD, failed).await;
        let verifier = MoneroTopologyVerifier::new(
            run("topology-wallet-fail"),
            identity(),
            &daemon.endpoint,
            &target.endpoint,
            &foreign.endpoint,
        )
        .expect("distinct verifier");
        assert!(matches!(
            verifier.verify().await,
            Err(MoneroTopologyError::UnexpectedHttpStatus {
                operation: "target_wallet.get_version",
                actual: 500
            })
        ));
    }

    #[tokio::test]
    async fn foreign_credentials_must_finish_with_401_not_any_error() {
        let (daemon, _target, foreign) = valid_servers().await;
        let mut forbidden = RpcBehavior::valid();
        forbidden.bad_auth_status = 403;
        let target = spawn_server(USER, PASSWORD, forbidden).await;
        let verifier = MoneroTopologyVerifier::new(
            run("topology-not-401"),
            identity(),
            &daemon.endpoint,
            &target.endpoint,
            &foreign.endpoint,
        )
        .expect("distinct verifier");
        assert!(matches!(
            verifier.verify().await,
            Err(MoneroTopologyError::ForeignCredentialNotRejected { actual: 403 })
        ));
    }

    #[tokio::test]
    async fn non_peerless_or_non_offline_daemon_never_mints() {
        let (_daemon, target, foreign) = valid_servers().await;
        let mut connected = RpcBehavior::valid();
        connected.info = valid_info(1, 0);
        let daemon = spawn_server("daemon-user", "daemon-secret", connected).await;
        let verifier = MoneroTopologyVerifier::new(
            run("topology-connected"),
            identity(),
            &daemon.endpoint,
            &target.endpoint,
            &foreign.endpoint,
        )
        .expect("distinct verifier");
        assert!(matches!(
            verifier.verify().await,
            Err(MoneroTopologyError::DaemonProfileMismatch)
        ));
    }

    #[test]
    fn run_chain_and_each_observation_origin_are_cross_bound() {
        let attested_run = run("topology-binding-a");
        let other_run = run("topology-binding-b");
        let chain = identity();
        let daemon = "http://127.0.0.1:18081";
        let wallet = "http://127.0.0.1:18083";

        assert_eq!(
            validate_binding(
                &attested_run,
                chain,
                daemon,
                wallet,
                &other_run,
                MoneroNetwork::Regtest,
                [1; 32],
                daemon,
                wallet,
            ),
            Err(MoneroTopologyBindingError::RunMismatch)
        );
        assert_eq!(
            validate_binding(
                &attested_run,
                chain,
                daemon,
                wallet,
                &attested_run,
                MoneroNetwork::Regtest,
                [9; 32],
                daemon,
                wallet,
            ),
            Err(MoneroTopologyBindingError::ChainMismatch)
        );
        assert_eq!(
            validate_binding(
                &attested_run,
                chain,
                daemon,
                wallet,
                &attested_run,
                MoneroNetwork::Regtest,
                [1; 32],
                "http://127.0.0.1:28081",
                wallet,
            ),
            Err(MoneroTopologyBindingError::DaemonOriginMismatch)
        );
        assert_eq!(
            validate_binding(
                &attested_run,
                chain,
                daemon,
                wallet,
                &attested_run,
                MoneroNetwork::Regtest,
                [1; 32],
                daemon,
                "http://127.0.0.1:28083",
            ),
            Err(MoneroTopologyBindingError::WalletOriginMismatch)
        );
    }

    #[test]
    fn credentials_are_redacted_and_capability_is_not_cloneable() {
        assert_not_impl_any!(VerifiedMoneroTopologyAttestation: Clone);
        let daemon = LoopbackRpcEndpoint::new(
            "http://127.0.0.1:18081",
            "daemon-user",
            "never-log-daemon-secret",
        )
        .expect("daemon endpoint");
        let target =
            LoopbackRpcEndpoint::new("http://127.0.0.1:18083", USER, "never-log-target-secret")
                .expect("target endpoint");
        let foreign = LoopbackRpcEndpoint::new(
            "http://127.0.0.1:18084",
            FOREIGN_USER,
            "never-log-foreign-secret",
        )
        .expect("foreign endpoint");
        let debug = format!(
            "{:?}",
            MoneroTopologyVerifier::new(
                run("topology-redaction"),
                identity(),
                &daemon,
                &target,
                &foreign,
            )
            .expect("verifier")
        );
        assert!(debug.contains("[REDACTED]"));
        for secret in [
            "never-log-daemon-secret",
            "never-log-target-secret",
            "never-log-foreign-secret",
        ] {
            assert!(!debug.contains(secret));
        }
    }

    #[tokio::test]
    async fn duplicate_conflicting_and_oversized_authority_fields_fail_closed() {
        let (_daemon, target, foreign) = valid_servers().await;
        let mut duplicate = RpcBehavior::valid();
        duplicate.info = success(
            r#"{"status":"OK","untrusted":false,"nettype":"fakechain","nettype":"mainnet","mainnet":false,"testnet":false,"stagenet":false,"offline":true,"incoming_connections_count":0,"outgoing_connections_count":0,"version":"0.18.5.1-release"}"#,
        );
        let daemon = spawn_server("daemon-user", "daemon-secret", duplicate).await;
        let verifier = MoneroTopologyVerifier::new(
            run("topology-duplicate"),
            identity(),
            &daemon.endpoint,
            &target.endpoint,
            &foreign.endpoint,
        )
        .expect("distinct verifier");
        assert!(matches!(
            verifier.verify().await,
            Err(MoneroTopologyError::MalformedRpcEnvelope {
                operation: "daemon.get_info"
            })
        ));

        let (_daemon, target, foreign) = valid_servers().await;
        let mut oversized = RpcBehavior::valid();
        oversized.info = "x".repeat(MAX_TOPOLOGY_RESPONSE_BYTES + 1);
        let daemon = spawn_server("daemon-user", "daemon-secret", oversized).await;
        let verifier = MoneroTopologyVerifier::new(
            run("topology-oversized"),
            identity(),
            &daemon.endpoint,
            &target.endpoint,
            &foreign.endpoint,
        )
        .expect("distinct verifier");
        assert!(matches!(
            verifier.verify().await,
            Err(MoneroTopologyError::ResponseTooLarge {
                operation: "daemon.get_info"
            })
        ));
    }

    #[test]
    fn missing_wrong_type_nonempty_connections_and_conflicting_envelopes_fail_closed() {
        let missing_offline = success(
            r#"{"status":"OK","untrusted":false,"nettype":"fakechain","mainnet":false,"testnet":false,"stagenet":false,"incoming_connections_count":0,"outgoing_connections_count":0,"version":"0.18.5.1-release"}"#,
        );
        assert!(
            serde_json::from_str::<RpcEnvelope<GetInfoResult>>(&missing_offline).is_err(),
            "a missing required isolation field must not decode"
        );

        let wrong_type = success(
            r#"{"status":"OK","untrusted":false,"nettype":"fakechain","mainnet":false,"testnet":false,"stagenet":false,"offline":"true","incoming_connections_count":0,"outgoing_connections_count":0,"version":"0.18.5.1-release"}"#,
        );
        assert!(
            serde_json::from_str::<RpcEnvelope<GetInfoResult>>(&wrong_type).is_err(),
            "a string must not substitute for the required boolean"
        );

        let nonempty_connections =
            success(r#"{"connections":[{"address":"127.0.0.1"}],"status":"OK","untrusted":false}"#);
        assert!(
            serde_json::from_str::<RpcEnvelope<GetConnectionsResult>>(&nonempty_connections)
                .is_err(),
            "connection details must never substitute for an empty-set proof"
        );

        let conflicting = format!(
            r#"{{"jsonrpc":"2.0","id":"{RPC_ID}","result":{{"release":true,"version":65567}},"error":{{"code":-1,"message":"conflict"}}}}"#
        );
        let envelope = serde_json::from_str::<RpcEnvelope<GetVersionResult>>(&conflicting)
            .expect("bounded conflicting envelope still has typed fields");
        assert!(matches!(
            envelope.into_result("fixture.conflict"),
            Err(MoneroTopologyError::ConflictingRpcEnvelope {
                operation: "fixture.conflict"
            })
        ));

        let wrong_id =
            r#"{"jsonrpc":"2.0","id":"another-run","result":{"release":true,"version":65567}}"#;
        let envelope = serde_json::from_str::<RpcEnvelope<GetVersionResult>>(wrong_id)
            .expect("typed wrong-id envelope");
        assert!(matches!(
            envelope.into_result("fixture.id"),
            Err(MoneroTopologyError::ConflictingRpcEnvelope {
                operation: "fixture.id"
            })
        ));
    }
}
