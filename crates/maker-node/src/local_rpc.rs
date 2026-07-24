//! Owner-local JSON-RPC client over a Unix-domain socket.

use std::path::Path;

use anyhow::{Context as _, bail, ensure};
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::{Request, header};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::net::UnixStream;

const JSON_RPC_VERSION: &str = "2.0";
const REQUEST_ID: u64 = 1;
const MAXIMUM_RPC_BODY_BYTES: usize = 64 * 1024;

#[derive(Serialize)]
struct RpcRequest<'a, P> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    params: [&'a P; 1],
}

#[derive(Deserialize)]
struct RpcResponse<R> {
    jsonrpc: Box<str>,
    id: u64,
    result: Option<R>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    code: i32,
    message: Box<str>,
}

/// Calls one typed JSON-RPC method through an owner-local Unix socket.
///
/// The client opens a fresh connection for every request. This keeps retries
/// explicit at the command layer and prevents a dropped connection from
/// silently replaying a mutation.
///
/// # Errors
///
/// Returns an error for an invalid method name, socket/HTTP failure, an
/// oversized response, malformed JSON-RPC, or a server-side RPC error.
pub async fn call_local_rpc<P, R>(socket: &Path, method: &str, parameter: &P) -> anyhow::Result<R>
where
    P: Serialize,
    R: DeserializeOwned,
{
    ensure!(
        !method.is_empty() && method.len() <= 128 && method.is_ascii(),
        "local RPC method must be 1..=128 ASCII bytes"
    );
    let payload = serde_json::to_vec(&RpcRequest {
        jsonrpc: JSON_RPC_VERSION,
        id: REQUEST_ID,
        method,
        params: [parameter],
    })
    .context("encode local JSON-RPC request")?;
    ensure!(
        payload.len() <= MAXIMUM_RPC_BODY_BYTES,
        "local RPC request exceeds {MAXIMUM_RPC_BODY_BYTES} bytes"
    );

    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connect local RPC socket {}", socket.display()))?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .context("start local RPC HTTP connection")?;
    let connection = tokio::spawn(connection);
    let request = Request::post("/")
        .header(header::HOST, "localhost")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(payload)))
        .context("build local RPC HTTP request")?;
    let response = sender
        .send_request(request)
        .await
        .context("send local RPC request")?;
    if !response.status().is_success() {
        bail!("local RPC returned HTTP status {}", response.status());
    }
    let body = Limited::new(response.into_body(), MAXIMUM_RPC_BODY_BYTES)
        .collect()
        .await
        .map_err(|error| anyhow::anyhow!("read bounded local RPC response: {error}"))?
        .to_bytes();
    drop(sender);
    connection
        .await
        .context("join local RPC connection")?
        .context("finish local RPC connection")?;

    let response: RpcResponse<R> =
        serde_json::from_slice(&body).context("decode local JSON-RPC response")?;
    ensure!(
        response.jsonrpc.as_ref() == JSON_RPC_VERSION && response.id == REQUEST_ID,
        "local RPC response version or request ID mismatch"
    );
    match (response.result, response.error) {
        (Some(result), None) => Ok(result),
        (None, Some(error)) => bail!("local RPC error {}: {}", error.code, error.message),
        _ => bail!("local RPC response must contain exactly one result or error"),
    }
}
