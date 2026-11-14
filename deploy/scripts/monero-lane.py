#!/usr/bin/env python3
"""The devnet's Monero on one lane (LEZ_MONERO=1): mine, fund a role, read its balance.

    monero-lane.py mine <blocks>                 generate blocks to the funding wallet
    monero-lane.py fund [--role R] [--xmr N]     the funding wallet pays the role's wallet N XMR
                                                 (10 when not given); ten blocks confirm it; the
                                                 role's balance is read through its own wallet-rpc
    monero-lane.py balance [--role R]            the role's balance, the same way, its
                                                 incoming transfers other than coinbase and
                                                 its outgoing ones

The lane is LEZ_LANE (1, the stack itself, when unset; scripts/lane.sh names
its containers, files and ports). monerod and the funding wallet-rpc are
reached on their published loopback ports, a role's wallet-rpc only through
its Node's loopback forward (`docker exec <role>-node curl 127.0.0.1:18083`),
the route the Node itself takes; every call carries the Digest login
gen-config.sh minted under runtime[/lanes/<k>]/monero. The funding wallet
("funding") and each role's ("maker", "taker") are opened, or created, when
their wallet-rpc has none open. Coinbase outputs unlock after 60 blocks and a
transfer takes 15 unlocked decoys beside its own output, so a chain under 100
blocks, or a funding wallet short of coins, gets 100 more (the isolated lane's
count).
"""
from __future__ import annotations

import argparse
import json
import os
import pathlib
import subprocess
import sys
import time

DEPLOY_ROOT = pathlib.Path(__file__).resolve().parent.parent
# The lane's names (scripts/lane.sh), as node-e2e.py takes them.
names = subprocess.run(["bash", "-c", "set -a; source runtime/runtime.env; set +a; source scripts/lane.sh; env -0"],
                       cwd=DEPLOY_ROOT, capture_output=True, check=True).stdout
LANE = dict(pair.decode().partition("=")[::2] for pair in names.split(b"\0") if pair.startswith(b"LEZ_"))
if LANE.get("LEZ_MONERO") != "1":
    sys.exit("Monero is off on this stack (LEZ_MONERO=1 scripts/up.sh starts it)")
PREFIX = LANE["LEZ_CONTAINER_PREFIX"]
LOGINS = DEPLOY_ROOT / LANE["LEZ_LANE_RUNTIME"] / "monero"
DAEMON = f"http://127.0.0.1:{LANE['LEZ_MONEROD_PORT']}/json_rpc"
FUNDING = f"http://127.0.0.1:{LANE['LEZ_XMR_FUNDING_WALLET_PORT']}/json_rpc"
PICONERO = 10**12
CONFIRMATIONS = 10  # the agreement's fixed depth (REQUIRED_MONERO_CONFIRMATIONS)


class Failure(Exception):
    """A Monero call failed or answered something else than asked for."""


def log(message: str) -> None:
    print(f"[monero {time.strftime('%H:%M:%S', time.gmtime())}] {message}", flush=True)


def call(where: str, method: str, params: dict | None = None) -> dict:
    """One JSON-RPC call to "daemon", "funding" or a role's wallet-rpc ("maker", "taker")."""
    login = (LOGINS / f"{where}.login").read_text().strip()
    # the login reaches curl on stdin, never on a command line
    config = f'user = "{login}"\n'
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}})
    curl = ["curl", "-sS", "--fail", "--max-time", "60", "--digest", "-K", "-",
            "-H", "content-type: application/json", "--data", body]
    if where in ("daemon", "funding"):
        command = [*curl, DAEMON if where == "daemon" else FUNDING]
    else:
        command = ["docker", "exec", "-i", f"{PREFIX}-{where}-node", *curl, "http://127.0.0.1:18083/json_rpc"]
    result = subprocess.run(command, input=config, capture_output=True, text=True, check=False)
    try:
        reply = json.loads(result.stdout)
    except json.JSONDecodeError:
        raise Failure(f"{where} {method}: no answer ({result.stderr.strip()[:200]})") from None
    if "error" in reply:
        raise Failure(f"{where} {method}: {json.dumps(reply['error'])[:200]}")
    return reply["result"]


def wait_ready(where: str, timeout: int = 120) -> None:
    deadline = time.time() + timeout
    while True:
        try:
            if where == "daemon":
                info = call("daemon", "get_info")
                if not (info["status"] == "OK" and info["nettype"] == "fakechain" and info["offline"]):
                    raise Failure(f"monerod is not the offline fakechain: {info['nettype']}, offline {info['offline']}")
            else:
                call(where, "get_version")
            return
        except Failure as failure:
            if time.time() > deadline:
                raise Failure(f"{where} did not answer within {timeout} s: {failure}") from failure
            time.sleep(2)


def open_wallet(where: str) -> str:
    """The address of the wallet open in `where`'s wallet-rpc; its own wallet (named
    after it) is opened, or created, when none is. A role's own wallet is always
    opened by name: its Node's Monero tools open a swap's wallets in the same
    wallet-rpc and may leave one open (call this only while no swap is live)."""
    wait_ready(where)
    if where == "funding":
        try:
            return call(where, "get_address", {"account_index": 0})["address"]
        except Failure:
            pass
    try:
        call(where, "open_wallet", {"filename": where, "password": ""})
    except Failure:
        call(where, "create_wallet", {"filename": where, "password": "", "language": "English"})
        log(f"{where}: created its wallet")
    return call(where, "get_address", {"account_index": 0})["address"]


def mine(blocks: int, why: str) -> int:
    """Generates `blocks` blocks to the funding wallet; returns the new height."""
    wait_ready("daemon")
    address = open_wallet("funding")
    began = time.time()
    height = call("daemon", "generateblocks", {"amount_of_blocks": blocks, "wallet_address": address})["height"]
    log(f"mined {blocks} block(s) in {time.time() - began:.1f} s → height {height} ({why})")
    return height


def balance(role: str) -> dict:
    """The role's balance, what it received apart from coinbase (the Node's
    own tools mine their confirmations, some of them to this wallet), and
    what it sent."""
    open_wallet(role)
    call(role, "refresh")
    transfers = call(role, "get_transfers", {"in": True, "out": True, "account_index": 0})
    return {**call(role, "get_balance", {"account_index": 0}),
            "received": [{"txid": t["txid"], "amount": t["amount"], "height": t["height"]}
                         for t in transfers.get("in", []) if t.get("type") == "in"],
            "sent": [{"txid": t["txid"], "amount": t["amount"], "height": t["height"]} for t in transfers.get("out", [])]}


def fund(role: str, xmr: int) -> dict:
    amount = xmr * PICONERO
    destination = open_wallet(role)
    open_wallet("funding")
    for _ in range(4):
        call("funding", "refresh")
        if call("funding", "get_balance", {"account_index": 0})["unlocked_balance"] > amount + PICONERO \
                and call("daemon", "get_info")["height"] >= 100:
            break
        mine(100, "coinbase unlocks after 60 blocks; a transfer's ring takes 15 unlocked decoys")
    else:
        raise Failure(f"the funding wallet cannot pay {xmr} XMR after mining")
    before = balance(role)["balance"]
    transfer = call("funding", "transfer", {"destinations": [{"amount": amount, "address": destination}],
                                            "account_index": 0, "priority": 1})
    txid = transfer["tx_hash"]
    log(f"funding → {role} {xmr} XMR in {txid} (fee {transfer['fee']} piconero)")
    began = time.time()
    height = mine(CONFIRMATIONS, "the agreement's ten confirmations")
    after = balance(role)
    found = call(role, "get_transfer_by_txid", {"txid": txid, "account_index": 0})["transfer"]
    log(f"{role} sees it {time.time() - began:.1f} s after the transfer: "
        f"{found['confirmations']} confirmation(s), locked {found['locked']}")
    if not (found["amount"] == amount and found["confirmations"] >= CONFIRMATIONS and not found["locked"]
            and after["balance"] == before + amount):
        raise Failure(f"{role} does not hold the {xmr} XMR at depth: {json.dumps(found)[:300]}")
    return {"lane": LANE["LEZ_LANE"], "role": role, "txid": txid, "amount_piconero": amount,
            "confirmations": found["confirmations"], "height": height,
            "balance_piconero": after["balance"], "unlocked_balance_piconero": after["unlocked_balance"]}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("mine").add_argument("blocks", type=int)
    for name in ("fund", "balance"):
        command = sub.add_parser(name)
        command.add_argument("--role", choices=["maker", "taker"], default="maker")
        if name == "fund":
            command.add_argument("--xmr", type=int, default=10)
    args = parser.parse_args()
    try:
        if args.command == "mine":
            mine(args.blocks, "by hand")
        elif args.command == "fund":
            print(json.dumps(fund(args.role, args.xmr)))
        else:
            print(json.dumps({"lane": LANE["LEZ_LANE"], "role": args.role, **balance(args.role)}))
    except Failure as failure:
        log(f"FAILED: {failure}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
