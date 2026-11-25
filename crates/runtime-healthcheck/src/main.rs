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
    let (transport, endpoint, method, parameters, cookie_file) = match arguments.as_slice() {
        [_, transport, endpoint, method, parameters] => (
            transport.as_str(),
            endpoint.as_str(),
            method.as_str(),
            parameters.as_str(),
            None,
        ),
        [_, transport, endpoint, method, parameters, flag, cookie_file]
            if flag == "--cookie-file" =>
        {
            (
                transport.as_str(),
                endpoint.as_str(),
                method.as_str(),
                parameters.as_str(),
                Some(Path::new(cookie_file.as_str())),
            )
        }
        _ => {
            return Err(
                "usage: lez-runtime-healthcheck <tcp|uds> <endpoint> <method> <JSON params> [--cookie-file <path>]"
                    .into(),
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
    // A cookie-authenticated JSON-RPC (Zebra's `.cookie`, new at every start): read at
    // each probe, never printed.
    let authorization = match cookie_file {
        Some(path) => format!("Authorization: Basic {}\r\n", cookie_credential(path)?),
        None => String::new(),
    };
    let request = format!(
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n{authorization}Content-Length: {}\r\nConnection: close\r\n\r\n",
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

/// The base64 `user:password` of a cookie file: one line of visible ASCII with a
/// colon, at most 1 KiB, an optional line ending dropped.
fn cookie_credential(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut raw = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| "cookie file is unreadable")?
        .take(1_027)
        .read_to_end(&mut raw)
        .map_err(|_| "cookie file is unreadable")?;
    let credential = raw
        .strip_suffix(b"\r\n")
        .or_else(|| raw.strip_suffix(b"\n"))
        .unwrap_or(&raw);
    let delimiter = credential.iter().position(|byte| *byte == b':');
    if credential.is_empty()
        || credential.len() > 1_024
        || delimiter.is_none_or(|index| index == 0 || index + 1 == credential.len())
        || !credential.iter().all(u8::is_ascii_graphic)
    {
        return Err("cookie file does not hold one user:password line".into());
    }
    Ok(base64_encode(credential))
}

fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bytes = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let group = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        for (index, shift) in [18_u32, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(char::from(ALPHABET[((group >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
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
    use super::{base64_encode, cookie_credential, validate_response};

    #[test]
    fn encodes_cookie_credentials_as_basic_auth() {
        assert_eq!(base64_encode(b"__cookie__:a"), "X19jb29raWVfXzph");
        assert_eq!(base64_encode(b"u:pw"), "dTpwdw==");
        assert_eq!(base64_encode(b"u:p"), "dTpw");
        let directory = std::env::temp_dir().join(format!("lez-healthcheck-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("scratch directory");
        let cookie = directory.join(".cookie");
        std::fs::write(&cookie, b"__cookie__:secret\n").expect("cookie");
        assert_eq!(
            cookie_credential(&cookie).expect("valid cookie"),
            base64_encode(b"__cookie__:secret")
        );
        for invalid in [&b""[..], b"no-colon", b":secret", b"user:", b"user:se cret"] {
            std::fs::write(&cookie, invalid).expect("cookie");
            assert!(cookie_credential(&cookie).is_err());
        }
        std::fs::remove_dir_all(&directory).expect("cleanup");
    }

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
