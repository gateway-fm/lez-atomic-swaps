"""Minimal Python 3.10+ owner API client. No dependencies or automatic retries.

Usage: python3 owner_rpc.py /run/lez/maker/node.sock maker_health '{}'
See docs/api/README.md for the public method contracts and mutation semantics.
"""

import http.client
import json
import math
import socket
import sys


MAX_BODY_BYTES = 64 * 1024


class RpcError(Exception):
    """A node rejected a request; retain its code and optional structured data."""

    def __init__(self, code, message, data=None):
        super().__init__(f"RPC {code}: {message}")
        self.code = code
        self.message = message
        self.data = data


class _UnixConnection(http.client.HTTPConnection):
    def __init__(self, path, timeout):
        super().__init__("localhost", timeout=timeout)
        self.path = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(self.path)


class OwnerRpc:
    """Call an owner-authorized node socket with exact JSON integer amounts.

    A fresh connection is opened for each call. A timeout or lost response does
    not prove that a mutation failed; reconcile or replay its *original* durable
    request_id and payload. The HTTP timeout is per blocking socket operation.
    """

    def __init__(self, socket_path, timeout=120):
        if not math.isfinite(timeout) or timeout <= 0:
            raise ValueError("timeout must be finite and positive")
        self.socket_path = str(socket_path)
        self.timeout = timeout

    def call(self, method, parameter):
        if not isinstance(method, str) or not method.isascii() or not 1 <= len(method) <= 128:
            raise ValueError("method must be 1..128 ASCII characters")
        if not isinstance(parameter, dict):
            raise ValueError("supply one parameter object")
        payload = json.dumps({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": [parameter]
        }, allow_nan=False, separators=(",", ":")).encode()
        if len(payload) > MAX_BODY_BYTES:
            raise ValueError("request exceeds 64 KiB")
        connection = _UnixConnection(self.socket_path, self.timeout)
        try:
            connection.request("POST", "/", payload, {"Content-Type": "application/json"})
            response = connection.getresponse()
            if response.status != 200:
                raise RuntimeError(f"owner RPC HTTP status {response.status}")
            body = response.read(MAX_BODY_BYTES + 1)
            if len(body) > MAX_BODY_BYTES:
                raise ValueError("response exceeds 64 KiB")
        finally:
            connection.close()
        envelope = json.loads(body)
        if (not isinstance(envelope, dict) or envelope.get("jsonrpc") != "2.0"
                or type(envelope.get("id")) is not int or envelope["id"] != 1
                or (("result" in envelope) == ("error" in envelope))):
            raise ValueError("invalid JSON-RPC response envelope")
        if "error" in envelope:
            error = envelope["error"]
            if (not isinstance(error, dict) or type(error.get("code")) is not int
                    or not isinstance(error.get("message"), str)):
                raise ValueError("invalid JSON-RPC error")
            raise RpcError(error["code"], error["message"], error.get("data"))
        return envelope["result"]


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit("usage: owner_rpc.py SOCKET METHOD PARAMETER_JSON")
    try:
        result = OwnerRpc(sys.argv[1]).call(sys.argv[2], json.loads(sys.argv[3]))
        print(json.dumps(result, indent=2))
    except (OSError, ValueError, RuntimeError, RpcError, http.client.HTTPException) as error:
        sys.exit(str(error))
