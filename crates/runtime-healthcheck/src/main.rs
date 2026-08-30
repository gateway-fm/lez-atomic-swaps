use std::{
    env,
    io::{Read, Write as _},
    net::TcpStream,
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

const MAXIMUM_RESPONSE_BYTES: usize = 64 * 1024;

fn main() {
    if let Err(error) = run() {
        eprintln!("readiness probe failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = env::args().collect::<Vec<_>>();
    let (transport, endpoint, method, parameters) = match arguments.as_slice() {
        [_, transport, endpoint, method, parameters] => (
            transport.as_str(),
            endpoint.as_str(),
            method.as_str(),
            parameters.as_str(),
        ),
        _ => {
            return Err(
                "usage: lez-runtime-healthcheck <tcp|uds> <endpoint> <method> <JSON params>".into(),
            );
        }
    };
    if method.is_empty()
        || !method
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
    {
        return Err("RPC method is invalid".into());
    }
    let parameters: serde_json::Value = serde_json::from_str(parameters)?;
    if !parameters.is_array() {
        return Err("RPC parameters must be a JSON array".into());
    }
    let body = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": parameters,
    }))?;
    let request = format!(
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let response = match transport {
        "tcp" => {
            let mut stream = TcpStream::connect(endpoint)?;
            stream.set_read_timeout(Some(Duration::from_secs(3)))?;
            stream.write_all(request.as_bytes())?;
            stream.write_all(&body)?;
            read_bounded(&mut stream)?
        }
        "uds" => {
            if !Path::new(endpoint).is_absolute() {
                return Err("Unix socket endpoint must be absolute".into());
            }
            let mut stream = UnixStream::connect(endpoint)?;
            stream.set_read_timeout(Some(Duration::from_secs(3)))?;
            stream.write_all(request.as_bytes())?;
            stream.write_all(&body)?;
            read_bounded(&mut stream)?
        }
        _ => return Err("transport must be tcp or uds".into()),
    };
    validate_response(&response)
}

fn read_bounded(reader: &mut impl Read) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut response = Vec::new();
    reader
        .take(u64::try_from(MAXIMUM_RESPONSE_BYTES + 1)?)
        .read_to_end(&mut response)?;
    if response.len() > MAXIMUM_RESPONSE_BYTES {
        return Err("RPC response is too large".into());
    }
    Ok(response)
}

fn validate_response(response: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let separator = b"\r\n\r\n";
    let boundary = response
        .windows(separator.len())
        .position(|window| window == separator)
        .ok_or("RPC response has no HTTP body")?;
    let head = std::str::from_utf8(&response[..boundary])?;
    if !head
        .lines()
        .next()
        .is_some_and(|line| line.contains(" 200 "))
    {
        return Err("RPC response is not HTTP 200".into());
    }
    let value: serde_json::Value = serde_json::from_slice(&response[boundary + separator.len()..])?;
    let result = value.get("result");
    if value.get("error").is_some()
        || result.is_none_or(serde_json::Value::is_null)
        || result.and_then(|value| value.get("ready")) == Some(&serde_json::Value::Bool(false))
    {
        return Err("RPC returned an error or null result".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_response;

    #[test]
    fn accepts_only_successful_json_rpc_http() {
        validate_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 22\r\n\r\n{\"result\":{\"ready\":true}}",
        )
        .expect("valid result passes");
        assert!(validate_response(b"HTTP/1.1 500 Nope\r\n\r\n{\"result\":{}}").is_err());
        assert!(validate_response(b"HTTP/1.1 200 OK\r\n\r\n{\"error\":{}}").is_err());
        assert!(
            validate_response(b"HTTP/1.1 200 OK\r\n\r\n{\"result\":{\"ready\":false}}").is_err()
        );
    }
}
