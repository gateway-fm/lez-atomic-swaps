#!/usr/bin/env python3
"""Behavioural checks for the Node market: the Maker publishes, the Taker discovers.

Runs on the host against the two Nodes' owner-only sockets (through
`docker exec`). Publishes one uniquely named offer on the Maker Node, sees it
reach the Taker Node through Delivery, replays the publication idempotently,
rejects malformed and stale requests, withdraws the offer and sees it leave the
Taker's order book. It starts no swap and submits no chain effect.

Usage (from deploy/): python3 scripts/verify-market.py
"""
from __future__ import annotations

import json
import subprocess
import sys
import time

FAILURES: list[str] = []
CHECKS = 0
SOCKETS = {"maker": ("lez-maker-node", "/run/lez/maker/node.sock"),
           "taker": ("lez-taker-node", "/run/lez/taker/node.sock")}
ROUTE = {"pair": "Bitcoin", "direction": "TakerSellsForeign"}


def rpc(role: str, method: str, params: object) -> dict:
    container, socket = SOCKETS[role]
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": [params]})
    result = subprocess.run(
        ["docker", "exec", "-i", container, "curl", "-sS", "--max-time", "30", "--unix-socket", socket,
         "-H", "content-type: application/json", "--data", body, "http://localhost/"],
        capture_output=True, text=True, check=False)
    if result.returncode != 0 or not result.stdout:
        return {"error": {"code": -1, "message": f"transport: {result.stderr.strip()[:200]}"}}
    return json.loads(result.stdout)


def check(label: str, ok: bool, detail: str = "") -> bool:
    global CHECKS
    CHECKS += 1
    print(f"  [{'PASS' if ok else 'FAIL'}] {label}{(' — ' + detail) if detail and not ok else ''}")
    if not ok:
        FAILURES.append(label)
    return ok


def expect_error(label: str, role: str, method: str, params: object, fragment: str) -> None:
    reply = rpc(role, method, params)
    error = reply.get("error") or {}
    text = json.dumps(error)
    check(label, "error" in reply and fragment in text, text[:160] if error else "unexpectedly succeeded")


def maker_offers() -> list[dict]:
    return rpc("maker", "maker_offer_list", {})["result"]


def maker_offer(offer_id: str) -> dict | None:
    return next((record for record in maker_offers() if record["offer"]["id"] == offer_id), None)


def taker_offer_ids() -> set[str]:
    return {entry["offer"]["id"] for entry in rpc("taker", "taker_offer_list_v1", {"schema_version": 1})["result"]["offers"]}


def wait_until(predicate, timeout: float = 20.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(1)
    return predicate()


def main() -> int:
    stamp = int(time.time())
    offer_id = f"offer-verify-market-{stamp}"

    print("\nhealth")
    maker_health = rpc("maker", "maker_health", {})
    check("Maker Node answers health", "result" in maker_health, json.dumps(maker_health.get("error", ""))[:120])
    taker_health = rpc("taker", "taker_health", {"schema_version": 1})
    check("Taker Node answers health", "result" in taker_health, json.dumps(taker_health.get("error", ""))[:120])

    print("\nroute")

    def revision_for(method: str) -> int | None:
        listed = rpc("maker", method, {}).get("result") or []
        for entry in listed:
            route = (entry.get("value") or {}).get("route") or {}
            if route.get("pair") == "Bitcoin" and route.get("direction") == "TakerSellsForeign":
                return int(entry["revision"])
        return None

    route_request = {
        "request_id": f"verify-market-route-{stamp}",
        "configuration": {"route": ROUTE, "enabled": True, "price_source": "local",
                          "minimum_foreign_units": 1000000, "maximum_foreign_units": 1000000,
                          "offer_ttl_seconds": 3600},
        "price": {"route": ROUTE, "lez_units_per_lot": 1, "foreign_units_per_lot": 1000},
    }
    pair_revision = revision_for("maker_pair_list")
    price_revision = revision_for("maker_local_price_list")
    if pair_revision is not None:
        route_request["expected_pair_revision"] = pair_revision
    if price_revision is not None:
        route_request["expected_price_revision"] = price_revision
    saved = rpc("maker", "maker_local_route_save_v1", route_request)
    check("Bitcoin route saved at the fixed local preset", "result" in saved, json.dumps(saved.get("error", ""))[:160])

    print("\npublication")
    before = {record["offer"]["id"] for record in maker_offers()}
    request_id = f"verify-market-publish-{stamp}"
    published = rpc("maker", "maker_offer_publish", {"request_id": request_id, "offer_id": offer_id, "route": ROUTE})
    check("publishing one offer succeeds", "result" in published, json.dumps(published.get("error", ""))[:160])
    after = {record["offer"]["id"] for record in maker_offers()}
    check("publishing adds exactly one offer", after - before == {offer_id}, f"added {sorted(after - before)}")
    record = maker_offer(offer_id)
    check("the new offer is active at revision 1",
          record is not None and record["status"] == "active" and record["revision"] == 1,
          json.dumps({k: v for k, v in (record or {}).items() if k != "offer"}))
    replay = rpc("maker", "maker_offer_publish", {"request_id": request_id, "offer_id": offer_id, "route": ROUTE})
    check("replaying the same request is idempotent",
          "result" in replay and replay["result"].get("was_replay") is True
          and len({r["offer"]["id"] for r in maker_offers()}) == len(after),
          json.dumps(replay.get("result", replay.get("error")))[:160])
    expect_error("reusing a request id for a different offer is rejected", "maker", "maker_offer_publish",
                 {"request_id": request_id, "offer_id": offer_id + "-other", "route": ROUTE}, "")
    expect_error("wrong schema version is rejected", "taker", "taker_offer_list_v1", {"schema_version": 7}, "")
    expect_error("unknown method is rejected", "maker", "maker_offer_drain_v1", {}, "Method not found")

    print("\ndiscovery")
    check("the Taker Node discovers the offer through Delivery",
          wait_until(lambda: offer_id in taker_offer_ids()), "not discovered within 20 s")

    print("\nwithdrawal")
    stale = rpc("maker", "maker_offer_withdraw",
                {"request_id": f"verify-market-stale-{stamp}", "offer_id": offer_id, "expected_revision": 9})
    check("withdrawing with a stale revision is rejected", "error" in stale, json.dumps(stale.get("error", ""))[:160])
    withdrawn = rpc("maker", "maker_offer_withdraw",
                    {"request_id": f"verify-market-withdraw-{stamp}", "offer_id": offer_id,
                     "expected_revision": record["revision"] if record else 1})
    check("withdrawing the offer succeeds", "result" in withdrawn, json.dumps(withdrawn.get("error", ""))[:160])
    record = maker_offer(offer_id)
    check("the Maker keeps the withdrawn offer in its history",
          record is not None and record["status"] == "withdrawn", json.dumps(record and record["status"]))
    check("the withdrawn offer leaves the Taker order book",
          wait_until(lambda: offer_id not in taker_offer_ids()), "still discoverable after 20 s")

    print(f"\n{CHECKS - len(FAILURES)}/{CHECKS} market checks passed")
    for failure in FAILURES:
        print(f"  failed: {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())
