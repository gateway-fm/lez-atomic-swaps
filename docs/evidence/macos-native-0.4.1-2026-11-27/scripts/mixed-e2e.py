#!/usr/bin/env python3
"""mixed-e2e.py <node-e2e.py arguments> — the stack's own scenario harness
(scripts/node-e2e.py of the stack in MIXED_STACK), unchanged, with ONE role's
Node native on this Mac and the other in its container.

node-e2e.py knows two layouts: both Nodes in containers, or both native
(LEZ_E2E_NATIVE_RUN_ROOT). Here MIXED_NATIVE_ROLE (maker or taker) is native,
started by its Logos host from its darwin-arm64 package under
MIXED_NATIVE_HOME (its LEZ_NODE_HOME), and the other role is the stack's
container. This wrapper loads node-e2e.py as a module and gives each
role-taking helper the native layout for the native role only (the
harness's own native code paths: owner RPC over the socket with this host's
curl, node_path/node_command, hold file and log file), and the container
layout for the other. Two helpers have no native path in node-e2e.py and
are replaced for the native role: the Maker's operator CLI (the package's
lez-maker-cli on the native socket, as `docker exec <maker> lez-maker-cli`
runs the image's) and the Monero lane (xmr-lane-mixed.py: the native role's
wallet-rpc on this host's 127.0.0.1:18083, the funding wallet on
MIXED_FUNDING_PORT). Nothing in the scenarios themselves is changed.
"""
from __future__ import annotations

import functools
import importlib.util
import json
import os
import pathlib
import subprocess
import sys

STACK = pathlib.Path(os.environ["MIXED_STACK"])
NATIVE_ROLE = os.environ["MIXED_NATIVE_ROLE"]
HOME = pathlib.Path(os.environ["MIXED_NATIVE_HOME"])
RUN_ROOT = str(HOME / "run")
HERE = pathlib.Path(__file__).resolve().parent
assert NATIVE_ROLE in ("maker", "taker")

spec = importlib.util.spec_from_file_location("node_e2e", STACK / "scripts" / "node-e2e.py")
M = importlib.util.module_from_spec(spec)
sys.modules["node_e2e"] = M
sys.argv[0] = str(STACK / "scripts" / "node-e2e.py")
spec.loader.exec_module(M)
assert M.NATIVE_RUN_ROOT == "", "run without LEZ_E2E_NATIVE_RUN_ROOT"
M.NATIVE_HOME = HOME
M.NATIVE_FILES = {role: {"hold": str(HOME / f"{role}.hold"), "log": str(HOME / f"{role}.node.log")}
                  for role in ("maker", "taker")}


def per_role(function):
    """`function(role, ...)` under the native layout for the native role only."""
    @functools.wraps(function)
    def wrapped(role, *args, **kwargs):
        saved = M.NATIVE_RUN_ROOT
        M.NATIVE_RUN_ROOT = RUN_ROOT if role == NATIVE_ROLE else ""
        try:
            return function(role, *args, **kwargs)
        finally:
            M.NATIVE_RUN_ROOT = saved
    return wrapped


for name in ("rpc", "native_pid", "native_bin", "node_path", "node_command", "node_command_stdin", "node_run",
             "node_stop", "node_start", "node_restart", "node_log_since", "wait_healthy"):
    setattr(M, name, per_role(getattr(M, name)))

if NATIVE_ROLE == "maker":
    def maker_cli(*arguments: str) -> object:
        """The package's lez-maker-cli beside the native Maker, on its owner socket."""
        cli = str(pathlib.Path(M.native_bin("maker")) / "lez-maker-cli")
        result = subprocess.run([cli, "--socket", f"{RUN_ROOT}/maker/node.sock", *arguments],
                                capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL)
        if result.returncode != 0:
            raise M.Failure(f"lez-maker-cli {arguments[0]} failed ({result.returncode}): {result.stderr.strip()[-300:]}")
        return json.loads(result.stdout)
    M.maker_cli = maker_cli


def xmr_lane(*argv: str) -> dict:
    """monero-lane.py's commands with the native role's wallet-rpc on this host."""
    result = subprocess.run([sys.executable, "-I", str(HERE / "xmr-lane-mixed.py"), *argv],
                            cwd=STACK, capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL)
    lines = result.stdout.strip().splitlines()
    for line in lines[:-1]:
        M.log(f"  {line}")
    if result.returncode != 0 or not lines:
        raise M.Failure(f"monero-lane.py {' '.join(argv)} failed: {(result.stdout + result.stderr).strip()[-300:]}")
    return json.loads(lines[-1])


M.xmr_lane = xmr_lane
M.LOG_CLOCK.start()
M.log(f"mixed layout: the {NATIVE_ROLE} Node native (run root $LEZ_NODE_HOME/run), the other in its container")
sys.exit(M.main())
