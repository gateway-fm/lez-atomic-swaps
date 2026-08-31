#!/usr/bin/env python3
"""Bounded owner-local readiness probe for the BTC demo controller."""

import json
import os
import socket


path = os.environ.get("LEZ_BTC_DEMO_SOCKET", "/run/lez-btc-demo/controller.sock")
body = json.dumps({
    "jsonrpc": "2.0", "id": 1, "method": "btc_market_health_v1",
    "params": [{"schema_version": 2}],
}, separators=(",", ":")).encode()
request = (
    b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n"
    + f"Content-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode()
    + body
)
client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
client.settimeout(3)
client.connect(path)
client.sendall(request)
response = b""
while len(response) <= 65536:
    part = client.recv(4096)
    if not part:
        break
    response += part
client.close()
_, separator, payload = response.partition(b"\r\n\r\n")
value = json.loads(payload) if separator else {}
if value.get("result", {}).get("ready") is not True:
    raise SystemExit("BTC demo controller is not ready")
