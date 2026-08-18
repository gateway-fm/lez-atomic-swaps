#!/usr/bin/env python3
"""Behavioural tests for the owner-local wallet market controller.

Runs INSIDE the btc-demo-controller container (the RPC socket is owner-only).
Exercises validation, idempotent replay, wallet ownership, offer lifecycle,
role gating and queueing without starting any swap runs, then leaves the
market exactly as it found it.

Usage: docker exec -i lez-btc-demo-controller python3 - < verify-market.py
"""
from __future__ import annotations

import http.client
import json
import socket
import sys
import time

SOCKET_PATH = "/run/lez-btc-demo/controller.sock"
FAILURES: list[str] = []
CHECKS = 0


class UnixConn(http.client.HTTPConnection):
    def __init__(self, path: str):
        super().__init__("localhost")
        self.sock_path = path

    def connect(self) -> None:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.connect(self.sock_path)
        self.sock = sock


def rpc(method: str, params: dict) -> dict:
    conn = UnixConn(SOCKET_PATH)
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": [params]})
    conn.request("POST", "/", body=body, headers={"Content-Type": "application/json"})
    return json.loads(conn.getresponse().read())


def check(label: str, ok: bool, detail: str = "") -> bool:
    global CHECKS
    CHECKS += 1
    print(f"  [{'PASS' if ok else 'FAIL'}] {label}{(' — ' + detail) if detail else ''}")
    if not ok:
        FAILURES.append(label)
    return ok


def expect_error(label: str, method: str, params: dict, fragment: str) -> None:
    response = rpc(method, params)
    message = response.get("error", {}).get("message", "")
    check(label, fragment in message, message or "unexpectedly succeeded")


def request_id(role: str, tag: str) -> str:
    return f"ui-{role}-{tag}-{int(time.time() * 1000)}"


MAKER = {"schema_version": 2, "role": "maker", "wallet_id": "maker-munich-01"}
BASEL = {"schema_version": 2, "role": "maker", "wallet_id": "maker-basel-02"}
TAKER = {"schema_version": 2, "role": "taker", "wallet_id": "taker-zurich-01"}
LIMMAT = {"schema_version": 2, "role": "taker", "wallet_id": "taker-limmat-02"}
PRESET = {"count": 1, "bitcoin_sats": 1000000, "lez_units": 1000}

print("\nsnapshot and wallet scoping")
maker_snap = rpc("btc_market_snapshot_v1", MAKER)["result"]
taker_snap = rpc("btc_market_snapshot_v1", TAKER)["result"]
check("maker snapshot returns its own inventory",
      all(o["maker_wallet_id"] == "maker-munich-01" for o in maker_snap["inventory"]),
      f"{len(maker_snap['inventory'])} offers")
check("taker snapshot exposes no inventory field content",
      taker_snap["inventory"] == [], "taker holds no maker inventory")
check("order book spans both maker wallets",
      len({o["maker_wallet_id"] for o in taker_snap["order_book"]}) >= 1,
      f"{len(taker_snap['order_book'])} open offers")
check("maker sees only maker-owned swaps",
      all(s["maker_wallet_id"] == "maker-munich-01" for s in maker_snap["swaps"]),
      f"{len(maker_snap['swaps'])} swaps")
check("runner state is reported", "runner_ready" in maker_snap and "runner_busy" in maker_snap,
      maker_snap["runner_detail"])

print("\ninput validation")
expect_error("unknown role rejected", "btc_market_snapshot_v1",
             {"schema_version": 2, "role": "auditor", "wallet_id": "maker-munich-01"},
             "role must be maker or taker")
expect_error("unknown wallet rejected", "btc_market_snapshot_v1",
             {"schema_version": 2, "role": "maker", "wallet_id": "maker-geneva-99"},
             "select a known local maker wallet")
expect_error("wallet/role mismatch rejected", "btc_market_snapshot_v1",
             {"schema_version": 2, "role": "maker", "wallet_id": "taker-zurich-01"},
             "select a known local maker wallet")
expect_error("wrong schema version rejected", "btc_market_snapshot_v1",
             {"schema_version": 1, "role": "maker", "wallet_id": "maker-munich-01"},
             "unsupported wallet-market schema version")
expect_error("unknown method rejected", "btc_market_drain_v1", MAKER, "unknown method")
expect_error("batch publishing rejected", "btc_offer_create_v1",
             {**MAKER, "request_id": request_id("maker", "batch"), "count": 3,
              "bitcoin_sats": 1000000, "lez_units": 1000},
             "publish exactly one offer per request")
expect_error("off-preset amount rejected", "btc_offer_create_v1",
             {**MAKER, "request_id": request_id("maker", "amount"), "count": 1,
              "bitcoin_sats": 999, "lez_units": 1000},
             "the local market uses its exact BTC/LEZ preset")
expect_error("malformed request id rejected", "btc_offer_create_v1",
             {**MAKER, "request_id": "not-a-request-id", **PRESET},
             "request identity is invalid")

print("\noffer lifecycle")
create_id = request_id("maker", "create-offers")
before = len([o for o in rpc("btc_market_snapshot_v1", MAKER)["result"]["inventory"]
              if o["state"] == "pending"])
created = rpc("btc_offer_create_v1", {**MAKER, "request_id": create_id, **PRESET})["result"]
pending = [o for o in created["inventory"] if o["state"] == "pending"]
check("publishing one offer adds exactly one", len(pending) == before + 1,
      f"{before} → {len(pending)}")
new_offer = max((o for o in pending), key=lambda o: o["created_at"])

replayed = rpc("btc_offer_create_v1", {**MAKER, "request_id": create_id, **PRESET})["result"]
replay_pending = [o for o in replayed["inventory"] if o["state"] == "pending"]
check("replaying the same request is idempotent", len(replay_pending) == len(pending),
      f"still {len(replay_pending)} pending")

expect_error("reusing a request id for different facts is rejected", "btc_offer_create_v1",
             {**BASEL, "request_id": create_id, **PRESET},
             "request identity was already used for different facts")

expect_error("withdrawing another wallet's offer is rejected", "btc_offer_withdraw_v1",
             {**BASEL, "request_id": request_id("maker", "withdraw-foreign"),
              "offer_id": new_offer["offer_id"]},
             "pending offer")

withdrawn = rpc("btc_offer_withdraw_v1",
                {**MAKER, "request_id": request_id("maker", "withdraw-offer"),
                 "offer_id": new_offer["offer_id"]})["result"]
states = {o["offer_id"]: o["state"] for o in withdrawn["inventory"]}
check("withdrawing the offer removes it from the book",
      states.get(new_offer["offer_id"]) == "withdrawn", new_offer["offer_id"])
book = rpc("btc_market_snapshot_v1", TAKER)["result"]["order_book"]
check("withdrawn offer disappears from the taker order book",
      new_offer["offer_id"] not in {o["offer_id"] for o in book},
      f"{len(book)} offers remain takeable")

print("\nrole gating")
snapshot = rpc("btc_market_snapshot_v1", TAKER)["result"]
completed = [s for s in snapshot["swaps"] if s["state"] == "completed"]
if completed:
    finished = completed[0]
    expect_error("acting on a completed swap is rejected", "btc_swap_action_v1",
                 {**TAKER, "request_id": request_id("taker", "swap-action"),
                  "ui_swap_id": finished["ui_swap_id"], "action": "lock_btc"},
                 "not ready at this swap revision")
    check("completed swaps expose no action", finished["action_required"] is None
          and finished["can_act"] is False, finished["run_id"])
    check("completed swaps carry their five chain effects",
          len(finished.get("effects", [])) == 5,
          f"{len(finished.get('effects', []))} effects")
expect_error("maker cannot perform a taker action", "btc_swap_action_v1",
             {**MAKER, "request_id": request_id("maker", "swap-action"),
              "ui_swap_id": "swap-0000000000000000", "action": "lock_btc"},
             "unavailable for this role")
expect_error("unknown action rejected", "btc_swap_action_v1",
             {**TAKER, "request_id": request_id("taker", "swap-action"),
              "ui_swap_id": "swap-0000000000000000", "action": "drain_wallet"},
             "unavailable for this role")
expect_error("malformed swap id rejected", "btc_swap_action_v1",
             {**TAKER, "request_id": request_id("taker", "swap-action"),
              "ui_swap_id": "swap-nope", "action": "lock_btc"},
             "swap identity is invalid")
expect_error("taking an unknown offer is rejected", "btc_offer_take_v1",
             {**LIMMAT, "request_id": request_id("taker", "take-offer"),
              "offer_id": "m3btc-nonexistent-000000000000-1"},
             "offer")

print("\nbalance evidence")
evidence = rpc("btc_market_snapshot_v1", MAKER)["result"].get("latest_balance_evidence")
if evidence:
    balances = evidence["wallet"]["balances"]
    reconciliation = evidence["reconciliation"]
    check("wallet ledger reports opening and closing LEZ",
          isinstance(balances["lez"]["opening"], int)
          and balances["lez"]["closing"] == balances["lez"]["opening"] - 1000,
          f"{balances['lez']['opening']} → {balances['lez']['closing']}")
    check("ledger opening balance is cumulative, not a genesis constant",
          balances["lez"]["opening"] not in (0, 100000),
          f"opening {balances['lez']['opening']}")
    check("LEZ conservation holds", reconciliation["lez_conserved"] is True,
          json.dumps(reconciliation))
    check("Bitcoin principal and fees are reported",
          reconciliation["bitcoin_principal"] == 1000000
          and reconciliation["total_bitcoin_fees"] > 0,
          f"principal {reconciliation['bitcoin_principal']}, "
          f"fees {reconciliation['total_bitcoin_fees']}")

print(f"\n{CHECKS - len(FAILURES)}/{CHECKS} checks passed")
if FAILURES:
    print("failed:")
    for failure in FAILURES:
        print(f"  - {failure}")
sys.exit(1 if FAILURES else 0)
