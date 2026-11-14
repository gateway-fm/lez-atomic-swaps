#!/usr/bin/env python3
"""docker-shim.py — installed as `docker` on the PATH of the swap harness on the Linux
host, where there is no Docker. The chains run in Docker on a remote machine and only
their RPC ports reach this host (SSH reverse tunnel). The harness (node-e2e.py,
zec-provision.sh) uses `docker exec <prefix>-bitcoin-core bitcoin-cli ...` to talk to
Bitcoin Core; this shim answers exactly that as a JSON-RPC call on 127.0.0.1:18443 with
the RPC cookie. Every invocation is logged; anything else is refused (exit 1), so the log
shows what the harness needed that a pure RPC provider does not give.
Env: LEZ_NODE_HOME (btc-rpc-cookie), SHIM_LOG (log file)."""
import base64
import json
import os
import sys
import time
import urllib.error
import urllib.request

LOG = os.environ.get("SHIM_LOG", "/dev/stderr")
args = sys.argv[1:]


def log(line: str) -> None:
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(f"{time.strftime('%H:%M:%S')} {line}\n")


def refuse(why: str) -> None:
    log(f"REFUSED ({why}): docker {' '.join(args)[:300]}")
    print(f"docker-shim: refused ({why}): docker {' '.join(args)[:200]}", file=sys.stderr)
    sys.exit(1)


def param(text: str):
    try:
        return json.loads(text)
    except ValueError:
        return text


if len(args) >= 3 and args[0] == "exec" and args[1].endswith("-bitcoin-core") and args[2] == "bitcoin-cli":
    rest = args[3:]
    wallet, named, i = None, False, 0
    while i < len(rest) and rest[i].startswith("-"):
        opt = rest[i]
        if opt.startswith("-rpcwallet="):
            wallet = opt.split("=", 1)[1]
        elif opt == "-named":
            named = True
        elif not (opt.startswith("-conf=") or opt.startswith("-datadir=")):
            refuse(f"bitcoin-cli option {opt}")
        i += 1
    if i >= len(rest):
        refuse("bitcoin-cli without a method")
    method, raw = rest[i], rest[i + 1:]
    params = {k: param(v) for k, v in (a.split("=", 1) for a in raw)} if named else [param(a) for a in raw]
    cookie = open(os.path.join(os.environ["LEZ_NODE_HOME"], "btc-rpc-cookie")).read().strip()
    url = "http://127.0.0.1:18443/" + (f"wallet/{wallet}" if wallet else "")
    body = json.dumps({"jsonrpc": "1.0", "id": "shim", "method": method, "params": params}).encode()
    req = urllib.request.Request(url, body, {"content-type": "application/json",
                                             "authorization": "Basic " + base64.b64encode(cookie.encode()).decode()})
    try:
        reply = json.loads(urllib.request.urlopen(req, timeout=120).read())
    except urllib.error.HTTPError as e:
        reply = json.loads(e.read() or b"{}")
    log(f"bitcoin-cli {('-rpcwallet=' + wallet + ' ') if wallet else ''}{method} {json.dumps(params)[:160]}"
        f" -> {'error' if reply.get('error') else 'ok'}")
    if reply.get("error"):
        print(f"error code: {reply['error'].get('code')}\nerror message:\n{reply['error'].get('message')}", file=sys.stderr)
        sys.exit(1)
    result = reply.get("result")
    print(result if isinstance(result, str) else json.dumps(result, indent=2))
    sys.exit(0)

# `docker inspect -f {{.State.Running}} <prefix>-<chain>`: a chain counts as running when
# its RPC answers through the tunnel (this reports reachability, not the container's state).
PROBES = {"zebra": ("18232", '{"jsonrpc":"2.0","id":1,"method":"getblockcount","params":[]}'),
          "bitcoin-core": ("18443", '{"jsonrpc":"1.0","id":1,"method":"getblockcount","params":[]}'),
          "sequencer": ("3040", None), "network": ("8779", None)}
if len(args) == 4 and args[0] == "inspect" and args[1] == "-f" and args[2] == "{{.State.Running}}":
    chain = next((c for c in PROBES if args[3].endswith("-" + c)), None)
    if chain is None:
        refuse("inspect of a non-chain container")
    port, body = PROBES[chain]
    headers = {"content-type": "application/json"}
    if chain == "bitcoin-core":
        cookie = open(os.path.join(os.environ["LEZ_NODE_HOME"], "btc-rpc-cookie")).read().strip()
        headers["authorization"] = "Basic " + base64.b64encode(cookie.encode()).decode()
    try:
        urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{port}/", body.encode() if body else None,
                                                      headers), timeout=10)
        up = True
    except urllib.error.HTTPError:
        up = True   # an HTTP answer at all: the service is there
    except Exception:  # noqa: BLE001 - unreachable is the answer
        up = False
    log(f"inspect State.Running {args[3]} -> {'true' if up else 'false'} (RPC probe on 127.0.0.1:{port})")
    print("true" if up else "false")
    sys.exit(0)

refuse("not a bitcoin-cli call")
