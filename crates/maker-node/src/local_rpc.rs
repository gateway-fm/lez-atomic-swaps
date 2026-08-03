//! Owner-local JSON-RPC client over a Unix-domain socket.

use std::{path::Path, time::Duration};

use anyhow::{Context as _, bail, ensure};
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::{Request, header};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::net::UnixStream;

const JSON_RPC_VERSION: &str = "2.0";
const REQUEST_ID: u64 = 1;
const MAXIMUM_CONTROL_RPC_BODY_BYTES: usize = 64 * 1024;
const MAXIMUM_CHAT_RPC_BODY_BYTES: usize = 1024 * 1024;
const DEFAULT_LOCAL_RPC_TIMEOUT: Duration = Duration::from_secs(30);

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
    call_local_rpc_bounded(
        socket,
        method,
        parameter,
        MAXIMUM_CONTROL_RPC_BODY_BYTES,
        DEFAULT_LOCAL_RPC_TIMEOUT,
    )
    .await
}

/// Calls one typed taker-facing Chat method through a Unix socket.
///
/// Chat byte arrays need more JSON framing space than owner-control messages,
/// which remain independently constrained to 64 KiB.
///
/// # Errors
///
/// Returns the same bounded transport and JSON-RPC errors as `call_local_rpc`.
pub async fn call_local_chat_rpc<P, R>(
    socket: &Path,
    method: &str,
    parameter: &P,
) -> anyhow::Result<R>
where
    P: Serialize,
    R: DeserializeOwned,
{
    call_local_rpc_bounded(
        socket,
        method,
        parameter,
        MAXIMUM_CHAT_RPC_BODY_BYTES,
        DEFAULT_LOCAL_RPC_TIMEOUT,
    )
    .await
}

async fn call_local_rpc_bounded<P, R>(
    socket: &Path,
    method: &str,
    parameter: &P,
    maximum_body_bytes: usize,
    request_timeout: Duration,
) -> anyhow::Result<R>
where
    P: Serialize,
    R: DeserializeOwned,
{
    ensure!(
        !request_timeout.is_zero(),
        "local RPC timeout must be nonzero"
    );
    tokio::time::timeout(
        request_timeout,
        call_local_rpc_without_timeout(socket, method, parameter, maximum_body_bytes),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "local RPC method {method} timed out after {} milliseconds",
            request_timeout.as_millis()
        )
    })?
}

async fn call_local_rpc_without_timeout<P, R>(
    socket: &Path,
    method: &str,
    parameter: &P,
    maximum_body_bytes: usize,
) -> anyhow::Result<R>
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
        payload.len() <= maximum_body_bytes,
        "local RPC request exceeds {maximum_body_bytes} bytes"
    );

    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connect local RPC socket {}", socket.display()))?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .context("start local RPC HTTP connection")?;
    let connection = AbortOnDropTask::new(tokio::spawn(connection));
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
    let body = Limited::new(response.into_body(), maximum_body_bytes)
        .collect()
        .await
        .map_err(|error| anyhow::anyhow!("read bounded local RPC response: {error}"))?
        .to_bytes();
    drop(sender);
    connection
        .join()
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

#[derive(Debug)]
struct AbortOnDropTask<T>(Option<tokio::task::JoinHandle<T>>);

impl<T> AbortOnDropTask<T> {
    fn new(handle: tokio::task::JoinHandle<T>) -> Self {
        Self(Some(handle))
    }

    async fn join(mut self) -> Result<T, tokio::task::JoinError> {
        self.0.take().expect("connection task is present").await
    }
}

impl<T> Drop for AbortOnDropTask<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Write as _},
        os::unix::net::UnixListener,
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    use serde_json::{Value, json};

    use super::{MAXIMUM_CONTROL_RPC_BODY_BYTES, call_local_rpc, call_local_rpc_bounded};

    const TEST_GUARD_TIMEOUT: Duration = Duration::from_secs(1);

    #[tokio::test]
    async fn nonresponsive_unix_peer_times_out_and_closes_connection() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("nonresponsive.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (closed_sender, closed_receiver) = mpsc::sync_channel(1);
        let peer = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let result = stream.read_to_end(&mut request);
            closed_sender.send((result, request)).unwrap();
        });

        let request_timeout = Duration::from_millis(50);
        let started = Instant::now();
        let result = tokio::time::timeout(
            TEST_GUARD_TIMEOUT,
            call_local_rpc_bounded::<_, Value>(
                &socket,
                "stalled_method",
                &json!({}),
                MAXIMUM_CONTROL_RPC_BODY_BYTES,
                request_timeout,
            ),
        )
        .await
        .expect("local RPC client ignored its configured timeout");
        let error = result.expect_err("nonresponsive local RPC must fail");

        assert!(
            error.to_string().contains("stalled_method")
                && error.to_string().contains("50 milliseconds"),
            "unexpected timeout error: {error:#}"
        );
        assert!(started.elapsed() < TEST_GUARD_TIMEOUT);
        let (closed, request) =
            tokio::task::spawn_blocking(move || closed_receiver.recv_timeout(TEST_GUARD_TIMEOUT))
                .await
                .unwrap()
                .expect("timed-out client left its Unix connection open");
        closed.expect("read request until client closed");
        assert!(String::from_utf8_lossy(&request).contains("stalled_method"));
        peer.join().unwrap();
    }

    #[tokio::test]
    async fn responsive_unix_peer_still_returns_typed_result() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("responsive.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let peer = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let request = read_http_request(&mut stream);
            assert!(String::from_utf8_lossy(&request).contains("maker_health"));

            let body = r#"{"jsonrpc":"2.0","id":1,"result":{"ready":true}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            stream.flush().unwrap();
        });

        let result: Value = call_local_rpc(&socket, "maker_health", &json!({}))
            .await
            .unwrap();

        assert_eq!(result, json!({ "ready": true }));
        peer.join().unwrap();
    }

    fn read_http_request(stream: &mut std::os::unix::net::UnixStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            assert_ne!(read, 0, "client closed before completing request");
            request.extend_from_slice(&chunk[..read]);
            if request_is_complete(&request) {
                return request;
            }
        }
    }

    fn request_is_complete(request: &[u8]) -> bool {
        let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
            return false;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        request.len() >= header_end + 4 + content_length
    }
}
