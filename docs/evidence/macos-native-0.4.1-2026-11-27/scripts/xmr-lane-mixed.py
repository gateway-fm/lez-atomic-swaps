#!/usr/bin/env python3
"""xmr-lane-mixed.py <monero-lane.py arguments> — the stack's
scripts/monero-lane.py (MIXED_STACK), unchanged, with the native role's own
wallet-rpc (MIXED_NATIVE_ROLE) reached on this host's 127.0.0.1:18083, where
the native Node reaches it, instead of through the role container's forward,
and the funding wallet-rpc on its moved host port (MIXED_FUNDING_PORT: the
native role's wallet-rpc holds 18083). Logins are read as monero-lane.py
reads them and reach curl on stdin only."""
from __future__ import annotations

import importlib.util
import json
import os
import pathlib
import subprocess
import sys

STACK = pathlib.Path(os.environ["MIXED_STACK"])
NATIVE_ROLE = os.environ["MIXED_NATIVE_ROLE"]
spec = importlib.util.spec_from_file_location("monero_lane", STACK / "scripts" / "monero-lane.py")
L = importlib.util.module_from_spec(spec)
sys.argv[0] = str(STACK / "scripts" / "monero-lane.py")
spec.loader.exec_module(L)
L.FUNDING = f"http://127.0.0.1:{os.environ['MIXED_FUNDING_PORT']}/json_rpc"
original_call = L.call


def call(where: str, method: str, params: dict | None = None) -> dict:
    if where != NATIVE_ROLE:
        return original_call(where, method, params)
    login = (L.LOGINS / f"{where}.login").read_text().strip()
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}})
    command = ["curl", "-sS", "--fail", "--max-time", "60", "--digest", "-K", "-",
               "-H", "content-type: application/json", "--data", body, "http://127.0.0.1:18083/json_rpc"]
    result = subprocess.run(command, input=f'user = "{login}"\n', capture_output=True, text=True, check=False)
    try:
        reply = json.loads(result.stdout)
    except json.JSONDecodeError:
        raise L.Failure(f"{where} {method}: no answer ({result.stderr.strip()[:200]})") from None
    if "error" in reply:
        raise L.Failure(f"{where} {method}: {json.dumps(reply['error'])[:200]}")
    return reply["result"]


L.call = call
sys.exit(L.main())
