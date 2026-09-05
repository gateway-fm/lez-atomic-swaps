#!/usr/bin/env python3
"""End-to-end BTC↔LEZ swap scenarios driven through the two Nodes' owner APIs.

No desk, no runner: the harness talks to `lez-maker-node` and `lez-taker-node`
over their owner-only sockets (through `docker exec`), mines regtest blocks on
demand, and stops or restarts the Node containers where a scenario calls for
it. Each scenario is one subcommand with its own exit code and a JSON summary
under `runtime/e2e/<scenario>.json`, so CI can run them as separate jobs.

    node-e2e.py happy            BTC→LEZ: take, lock, Maker funds, claim, Maker claims
    node-e2e.py replay           every call repeated with its request id; no second effect
    node-e2e.py wrong-inputs     stale revisions, bad envelope hash, off-preset amount, consumed lot
    node-e2e.py restart-taker    Taker Node restarted between lock and claim
    node-e2e.py restart-maker    Maker Node restarted between lock and its funding
    node-e2e.py survivor         Taker Node stopped after its LEZ claim; Maker completes alone
    node-e2e.py concurrent       two swaps interleaved end to end
    node-e2e.py taker-refund     Maker stopped before funding; Taker refunds its Bitcoin
    node-e2e.py maker-refund     Taker never claims; Maker refunds LEZ, then Taker refunds Bitcoin
    node-e2e.py all              every scenario in sequence (stops at the first failure)

The refund scenarios need the `fast` timing profile
(`LEZ_TIMING_PROFILE=fast scripts/gen-config.sh runtime`, then recreate the
Nodes); the harness refuses to start them under the `local` profile.
"""
from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys
import time

DEPLOY_ROOT = pathlib.Path(__file__).resolve().parent.parent
RUNTIME = DEPLOY_ROOT / "runtime"
ROUTE = {"pair": "Bitcoin", "direction": "TakerSellsForeign"}
FOREIGN_UNITS = 1_000_000
LEZ_UNITS = 1_000
NODES = {"maker": ("lez-maker-node", "/run/lez/maker/node.sock"),
         "taker": ("lez-taker-node", "/run/lez/taker/node.sock")}
BTC_CLI = ["docker", "exec", "lez-bitcoin-core", "bitcoin-cli",
           "-conf=/run-config/bitcoin.conf", "-datadir=/var/lib/bitcoin"]
MINER_ADDRESS = "bcrt1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqc8gma6"
TERMINAL = {"completed", "refunded", "attention_required"}


class Failure(Exception):
    """A scenario assertion failed."""


def log(message: str) -> None:
    print(f"[{time.strftime('%H:%M:%S')}] {message}", flush=True)


def rpc(role: str, method: str, params: object, timeout: int = 300) -> dict:
    """One owner-RPC call; returns the full JSON-RPC envelope."""
    container, socket = NODES[role]
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": [params]})
    result = subprocess.run(
        ["docker", "exec", "-i", container, "curl", "-sS", "--max-time", str(timeout), "--unix-socket", socket,
         "-H", "content-type: application/json", "--data", body, "http://localhost/"],
        capture_output=True, text=True, check=False)
    if result.returncode != 0 or not result.stdout.strip():
        return {"error": {"code": -1, "message": f"transport: {result.stderr.strip()[:200]}"}}
    return json.loads(result.stdout)


def call(role: str, method: str, params: object, timeout: int = 300) -> dict:
    reply = rpc(role, method, params, timeout)
    if "error" in reply:
        raise Failure(f"{role} {method} failed: {json.dumps(reply['error'])[:300]}")
    return reply["result"]


def expect_error(label: str, role: str, method: str, params: object, category: str | None = None) -> None:
    reply = rpc(role, method, params)
    if "error" not in reply:
        raise Failure(f"{label}: expected an error, got {json.dumps(reply.get('result'))[:200]}")
    error = reply["error"]
    got = (error.get("data") or {}).get("category") if isinstance(error.get("data"), dict) else None
    if category and got != category:
        raise Failure(f"{label}: expected category {category}, got {json.dumps(error)[:200]}")
    log(f"  rejected as expected: {label} ({got or error.get('message')})")


def bitcoin(*command: str) -> object:
    result = subprocess.run(BTC_CLI + list(command), capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise Failure(f"bitcoin-cli {command[0]} failed: {result.stderr.strip()[:200]}")
    text = result.stdout.strip()
    try:
        return json.loads(text)
    except ValueError:
        return text


def mine(blocks: int) -> None:
    bitcoin("generatetoaddress", str(blocks), MINER_ADDRESS)


def docker(*command: str) -> None:
    subprocess.run(["docker", *command], check=True, capture_output=True)


def wait_healthy(role: str, timeout: int = 180) -> None:
    container, _ = NODES[role]
    deadline = time.time() + timeout
    while time.time() < deadline:
        status = subprocess.run(["docker", "inspect", container, "--format", "{{.State.Health.Status}}"],
                                capture_output=True, text=True, check=False).stdout.strip()
        if status == "healthy":
            return
        time.sleep(3)
    raise Failure(f"{container} did not become healthy within {timeout}s")


def timing_profile() -> dict:
    values = {}
    for line in (RUNTIME / "runtime.env").read_text().splitlines():
        if line.startswith(("LEZ_TIMING_PROFILE=", "LEZ_BTC_")):
            key, _, value = line.partition("=")
            values[key] = value
    return values


# ---- swap steps -------------------------------------------------------------------

def revision_for(method: str) -> int | None:
    for entry in call("maker", method, {}):
        route = (entry.get("value") or {}).get("route") or {}
        if route == ROUTE:
            return int(entry["revision"])
    return None


def publish_offer(stamp: str, suffix: str = "") -> str:
    request = {
        "request_id": f"e2e-route-{stamp}{suffix}",
        "configuration": {"route": ROUTE, "enabled": True, "price_source": "local",
                          "minimum_foreign_units": FOREIGN_UNITS, "maximum_foreign_units": FOREIGN_UNITS,
                          "offer_ttl_seconds": 3600},
        "price": {"route": ROUTE, "lez_units_per_lot": 1, "foreign_units_per_lot": 1000},
    }
    pair_revision, price_revision = revision_for("maker_pair_list"), revision_for("maker_local_price_list")
    if pair_revision is not None:
        request["expected_pair_revision"] = pair_revision
    if price_revision is not None:
        request["expected_price_revision"] = price_revision
    call("maker", "maker_local_route_save_v1", request)
    offer_id = f"offer-e2e-{stamp}{suffix}"
    call("maker", "maker_offer_publish", {"request_id": f"e2e-publish-{stamp}{suffix}", "offer_id": offer_id, "route": ROUTE})
    log(f"  Maker published {offer_id}")
    return offer_id


def discover(offer_id: str, timeout: int = 60) -> dict:
    deadline = time.time() + timeout
    while time.time() < deadline:
        for entry in call("taker", "taker_offer_list_v1", {"schema_version": 1, "route": ROUTE})["offers"]:
            if entry["offer"]["id"] == offer_id:
                return entry
        time.sleep(2)
    raise Failure(f"Taker did not discover {offer_id} within {timeout}s")


def take_request(offer_id: str, entry: dict, request_id: str) -> dict:
    return {"schema_version": 1, "request_id": request_id, "offer_id": offer_id, "route": ROUTE,
            "maker_identity": entry["maker_identity"], "signed_envelope_sha256": entry["signed_envelope_sha256"],
            "foreign_units": FOREIGN_UNITS, "expected_lez_units": LEZ_UNITS}


def initiate(request: dict) -> dict:
    """One take. A dependency-unavailable answer (a LEZ read that raced the
    moving tip, Core briefly busy) is retried with the same request id, which
    replays the take from its durable record instead of starting another."""
    for attempt in range(4):
        reply = rpc("taker", "taker_swap_initiate_v1", request, timeout=420)
        if "result" in reply:
            return reply["result"]
        if reply["error"].get("code") != -32010 or attempt == 3:
            raise Failure(f"taker taker_swap_initiate_v1 failed: {json.dumps(reply['error'])[:300]}")
        log(f"  take answered dependency-unavailable ({(reply['error'].get('data') or {}).get('category')}); retrying")
        time.sleep(10)
    raise Failure("unreachable")


def take(offer_id: str, stamp: str, suffix: str = "") -> tuple[str, dict]:
    entry = discover(offer_id)
    request = take_request(offer_id, entry, f"e2e-take-{stamp}{suffix}")
    result = initiate(request)
    swap_id = result["swap"]["swap_id"]
    log(f"  Taker took {offer_id}: swap {swap_id[:12]} ({result['swap']['state']})")
    return swap_id, request


def taker_view(swap_id: str) -> dict:
    # The monitor answers `taker_monitor_unavailable` while the observer holds
    # the actor's state store; a few retries separate that from a real outage.
    for attempt in range(6):
        reply = rpc("taker", "taker_swap_monitor_v1", {"schema_version": 1, "swap_id": swap_id})
        if "result" in reply:
            return reply["result"].get("swap", reply["result"])
        if (reply["error"].get("data") or {}).get("category") != "taker_monitor_unavailable" or attempt == 5:
            raise Failure(f"taker taker_swap_monitor_v1 failed: {json.dumps(reply['error'])[:300]}")
        time.sleep(5)
    raise Failure("unreachable")


def maker_view(swap_id: str) -> dict:
    return call("maker", "maker_actor_monitor_v1", {"id": swap_id})


def maker_phase(swap_id: str) -> str:
    """Progress-log helper only: a stopped or unreachable Maker reads as such."""
    reply = rpc("maker", "maker_actor_monitor_v1", {"id": swap_id}, timeout=30)
    if "error" in reply:
        return "unreachable" if reply["error"].get("code") == -1 else reply["error"].get("message", "?")
    view = reply["result"]
    return ((view.get("progress") or {}).get("observation") or {}).get("phase") or view.get("schedule_state") or "?"


def lock(swap_id: str) -> str:
    result = call("taker", "taker_swap_lock_v1", {"schema_version": 1, "swap_id": swap_id}, timeout=120)
    log(f"  Taker locked Bitcoin: {result['transaction_id'][:12]} (replay={result.get('was_replay')})")
    return result["transaction_id"]


def wait_taker(swap_id: str, states: set[str], timeout: int, mine_every: int = 0, describe: str = "") -> dict:
    """Polls the Taker view until its state is in `states`; fails on any other terminal state."""
    deadline = time.time() + timeout
    last = None
    next_mine = time.time() + mine_every if mine_every else None
    while time.time() < deadline:
        view = taker_view(swap_id)
        state = view["state"]
        if state != last:
            log(f"  taker {swap_id[:12]}: {state} (gen {view.get('progress_generation')}) | maker: {maker_phase(swap_id)}")
            last = state
        if state in states:
            return view
        if state in TERMINAL:
            raise Failure(f"swap {swap_id[:12]} ended in {state} while waiting for {sorted(states)} {describe}")
        if next_mine and time.time() >= next_mine:
            mine(1)
            next_mine = time.time() + mine_every
        time.sleep(10)
    raise Failure(f"swap {swap_id[:12]} did not reach {sorted(states)} within {timeout}s (last {last}) {describe}")


def claim(swap_id: str, stamp: str, suffix: str = "") -> dict:
    view = wait_taker(swap_id, {"claim_available"}, timeout=1500, describe="(Maker funding)")
    result = call("taker", "taker_swap_claim_v1", {"schema_version": 1, "request_id": f"e2e-claim-{stamp}{suffix}",
                                                   "swap_id": swap_id, "expected_generation": view["progress_generation"]},
                  timeout=300)
    log(f"  Taker claimed LEZ (replay={result.get('was_replay')})")
    return result


def wait_completed(swap_id: str, timeout: int = 1800) -> None:
    wait_taker(swap_id, {"completed"}, timeout, describe="(Maker follow-up claim)")
    deadline = time.time() + 300
    while time.time() < deadline:
        view = maker_view(swap_id)
        if view.get("schedule_state") == "terminal" and maker_phase(swap_id) == "completed":
            log(f"  both Nodes report {swap_id[:12]} completed")
            return
        time.sleep(10)
    raise Failure(f"Maker did not report {swap_id[:12]} completed/terminal")


def happy_swap(stamp: str, suffix: str = "") -> str:
    offer_id = publish_offer(stamp, suffix)
    swap_id, _ = take(offer_id, stamp, suffix)
    lock(swap_id)
    claim(swap_id, stamp, suffix)
    wait_completed(swap_id)
    return swap_id


# ---- scenarios --------------------------------------------------------------------

def scenario_happy(stamp: str) -> dict:
    swap_id = happy_swap(stamp)
    return {"swap_id": swap_id}


def scenario_replay(stamp: str) -> dict:
    offer_id = publish_offer(stamp)
    again = call("maker", "maker_offer_publish", {"request_id": f"e2e-publish-{stamp}", "offer_id": offer_id, "route": ROUTE})
    if again.get("was_replay") is not True:
        raise Failure(f"offer publication replay was not idempotent: {again}")
    entry = discover(offer_id)
    request = take_request(offer_id, entry, f"e2e-take-{stamp}")
    first = initiate(request)
    second = initiate(request)
    if second.get("was_replay") is not True or second["swap"]["swap_id"] != first["swap"]["swap_id"]:
        raise Failure(f"take replay was not idempotent: {second}")
    swap_id = first["swap"]["swap_id"]
    log(f"  take replayed for {swap_id[:12]}")
    txid = lock(swap_id)
    if lock(swap_id) != txid:
        raise Failure("lock replay broadcast a different transaction")
    view = wait_taker(swap_id, {"claim_available"}, timeout=1500)
    params = {"schema_version": 1, "request_id": f"e2e-claim-{stamp}", "swap_id": swap_id,
              "expected_generation": view["progress_generation"]}
    call("taker", "taker_swap_claim_v1", params, timeout=300)
    replay = call("taker", "taker_swap_claim_v1", params, timeout=300)
    if replay.get("was_replay") is not True:
        raise Failure(f"claim replay was not idempotent: {replay}")
    log("  claim replayed")
    wait_completed(swap_id)
    listed = [s for s in call("taker", "taker_swap_list_v1", {"schema_version": 1})["swaps"] if s["swap_id"] == swap_id]
    if len(listed) != 1 or listed[0]["state"] != "completed":
        raise Failure(f"swap list disagrees after replays: {listed}")
    return {"swap_id": swap_id, "lock_txid": txid}


def scenario_wrong_inputs(stamp: str) -> dict:
    offer_id = publish_offer(stamp)
    record = next(r for r in call("maker", "maker_offer_list", {}) if r["offer"]["id"] == offer_id)
    expect_error("reusing a publish request id for another offer", "maker", "maker_offer_publish",
                 {"request_id": f"e2e-publish-{stamp}", "offer_id": offer_id + "-other", "route": ROUTE})
    expect_error("withdrawing with a stale revision", "maker", "maker_offer_withdraw",
                 {"request_id": f"e2e-stale-{stamp}", "offer_id": offer_id, "expected_revision": record["revision"] + 5})
    entry = discover(offer_id)
    bad_hash = take_request(offer_id, entry, f"e2e-badhash-{stamp}")
    bad_hash["signed_envelope_sha256"] = [(b + 1) % 256 for b in entry["signed_envelope_sha256"]]
    expect_error("taking with a mismatched envelope hash", "taker", "taker_swap_initiate_v1", bad_hash)
    off_preset = take_request(offer_id, entry, f"e2e-offpreset-{stamp}")
    off_preset["foreign_units"] = FOREIGN_UNITS - 1
    expect_error("taking an off-preset amount", "taker", "taker_swap_initiate_v1", off_preset)
    wrong_quote = take_request(offer_id, entry, f"e2e-wrongquote-{stamp}")
    wrong_quote["expected_lez_units"] = LEZ_UNITS + 1
    expect_error("taking with a wrong LEZ quote", "taker", "taker_swap_initiate_v1", wrong_quote)
    expect_error("locking an unknown swap", "taker", "taker_swap_lock_v1",
                 {"schema_version": 1, "swap_id": "00" * 32}, "lock_swap_unknown")
    expect_error("claiming an unknown swap", "taker", "taker_swap_claim_v1",
                 {"schema_version": 1, "request_id": f"e2e-noswap-{stamp}", "swap_id": "00" * 32, "expected_generation": 0})
    swap_id, _ = take(offer_id, stamp)
    expect_error("claiming before the Maker funded", "taker", "taker_swap_claim_v1",
                 {"schema_version": 1, "request_id": f"e2e-early-{stamp}", "swap_id": swap_id, "expected_generation": 0})
    deadline = time.time() + 30
    while time.time() < deadline and offer_id in {e["offer"]["id"] for e in call("taker", "taker_offer_list_v1", {"schema_version": 1})["offers"]}:
        time.sleep(2)
    if offer_id in {e["offer"]["id"] for e in call("taker", "taker_offer_list_v1", {"schema_version": 1})["offers"]}:
        raise Failure("the consumed lot is still discoverable")
    expect_error("taking the consumed lot again", "taker", "taker_swap_initiate_v1",
                 take_request(offer_id, entry, f"e2e-again-{stamp}"))
    log("  consumed lot left Delivery and cannot be taken again")
    # finish the swap so the stack is left clean
    lock(swap_id)
    claim(swap_id, stamp)
    wait_completed(swap_id)
    return {"swap_id": swap_id}


def scenario_restart(stamp: str, role: str) -> dict:
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    lock(swap_id)
    container, _ = NODES[role]
    log(f"  restarting {container}")
    docker("restart", container)
    wait_healthy(role)
    claim(swap_id, stamp)
    wait_completed(swap_id)
    return {"swap_id": swap_id, "restarted": container}


def scenario_survivor(stamp: str) -> dict:
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    lock(swap_id)
    claim(swap_id, stamp)
    wait_taker(swap_id, {"claim_in_progress", "both_legs_locked", "completed"}, timeout=600)
    log("  stopping the Taker Node right after its revealing claim")
    docker("stop", NODES["taker"][0])
    try:
        deadline = time.time() + 1800
        while time.time() < deadline:
            phase = maker_phase(swap_id)
            log(f"  maker {swap_id[:12]}: {phase}")
            if phase == "completed":
                break
            time.sleep(20)
        else:
            raise Failure("Maker did not complete without the Taker")
    finally:
        docker("start", NODES["taker"][0])
        wait_healthy("taker")
    wait_completed(swap_id, timeout=900)
    return {"swap_id": swap_id}


def ensure_taker_coins(count: int) -> None:
    """Two concurrent swaps need two spendable coins: each funding plan locks
    its inputs, so a wallet with one coin can fund one swap at a time."""
    spendable = [u for u in bitcoin("-rpcwallet=lez-taker", "listunspent") if float(u["amount"]) >= 0.02]
    if len(spendable) >= count:
        return
    log(f"  splitting the Taker wallet into {count} coins")
    for _ in range(count - len(spendable) + 1):
        address = bitcoin("-rpcwallet=lez-taker", "getnewaddress", "", "bech32m")
        bitcoin("-rpcwallet=lez-taker", "sendtoaddress", str(address), "0.05")
    mine(1)
    time.sleep(3)


def scenario_concurrent(stamp: str) -> dict:
    ensure_taker_coins(2)
    ids = []
    for suffix in ("-a", "-b"):
        offer_id = publish_offer(stamp, suffix)
        swap_id, _ = take(offer_id, stamp, suffix)
        ids.append((suffix, swap_id))
    for _, swap_id in ids:
        lock(swap_id)
    for suffix, swap_id in ids:
        claim(swap_id, stamp, suffix)
    for _, swap_id in ids:
        wait_completed(swap_id)
    return {"swap_ids": [s for _, s in ids]}


def require_fast_profile() -> dict:
    profile = timing_profile()
    if profile.get("LEZ_TIMING_PROFILE") != "fast":
        raise Failure("this scenario needs LEZ_TIMING_PROFILE=fast (gen-config.sh, then recreate maker-node and taker-node)")
    return profile


def scenario_taker_refund(stamp: str) -> dict:
    profile = require_fast_profile()
    csv_blocks = int(profile["LEZ_BTC_REFUND_CSV_BLOCKS"])
    cutoff = int(profile["LEZ_BTC_MAKER_LOCK_CUTOFF_SECONDS"])
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    log("  stopping the Maker Node so it never funds")
    docker("stop", NODES["maker"][0])
    before = float(bitcoin("-rpcwallet=lez-taker", "getbalance"))
    try:
        txid = lock(swap_id)
        mine(csv_blocks + 1)
        log(f"  mined {csv_blocks + 1} blocks past the lock; waiting out the Maker cutoff ({cutoff}s) for refund eligibility")
        view = wait_taker(swap_id, {"refund_available"}, timeout=cutoff + 900, mine_every=60)
        result = call("taker", "taker_swap_refund_v1", {"schema_version": 1, "request_id": f"e2e-refund-{stamp}",
                                                        "swap_id": swap_id, "expected_generation": view["progress_generation"]},
                      timeout=300)
        log(f"  Taker refund submitted (replay={result.get('was_replay')})")
        wait_taker(swap_id, {"refunded"}, timeout=900, mine_every=60)
        after = float(bitcoin("-rpcwallet=lez-taker", "getbalance"))
        log(f"  lez-taker balance {before:.8f} → {after:.8f}")
    finally:
        docker("start", NODES["maker"][0])
        wait_healthy("maker")
    deadline = time.time() + 600
    while time.time() < deadline:
        view = maker_view(swap_id)
        if view.get("schedule_state") == "terminal":
            log(f"  Maker reconciled to {maker_phase(swap_id)}")
            break
        time.sleep(20)
    else:
        raise Failure("Maker did not reconcile the refunded swap to a terminal state")
    return {"swap_id": swap_id, "lock_txid": txid, "taker_balance_before": before, "taker_balance_after": after}


def scenario_maker_refund(stamp: str) -> dict:
    profile = require_fast_profile()
    earlier = int(profile["LEZ_BTC_EARLIER_REFUND_SECONDS"])
    later = int(profile["LEZ_BTC_LATER_REFUND_SECONDS"])
    csv_blocks = int(profile["LEZ_BTC_REFUND_CSV_BLOCKS"])
    started = time.time()
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    lock(swap_id)
    wait_taker(swap_id, {"claim_available"}, timeout=1500, describe="(Maker funding)")
    log(f"  Maker funded; the Taker never claims. Waiting for the LEZ refund deadline ({earlier}s after the take)")
    while time.time() < started + earlier + 30:
        time.sleep(20)
    view = maker_view(swap_id)
    result = call("maker", "maker_actor_refund_v1", {"request_id": f"e2e-maker-refund-{stamp}", "id": swap_id,
                                                      "expected_generation": view["lease_generation"]})
    log(f"  Maker refund queued: {json.dumps(result)[:160]}")
    deadline = time.time() + 900
    while time.time() < deadline:
        phase = maker_phase(swap_id)
        log(f"  maker {swap_id[:12]}: {phase}")
        if phase in {"maker_leg_refunded", "refunded"}:
            break
        time.sleep(20)
    else:
        raise Failure("Maker did not refund its LEZ leg")
    mine(csv_blocks + 1)
    while time.time() < started + later + 30:
        time.sleep(20)
    view = wait_taker(swap_id, {"refund_available", "refunded"}, timeout=900, mine_every=60)
    if view["state"] != "refunded":
        call("taker", "taker_swap_refund_v1", {"schema_version": 1, "request_id": f"e2e-refund-{stamp}", "swap_id": swap_id,
                                               "expected_generation": view["progress_generation"]}, timeout=300)
        wait_taker(swap_id, {"refunded"}, timeout=900, mine_every=60)
    log("  both legs refunded")
    return {"swap_id": swap_id}


SCENARIOS = {
    "happy": scenario_happy,
    "replay": scenario_replay,
    "wrong-inputs": scenario_wrong_inputs,
    "restart-taker": lambda stamp: scenario_restart(stamp, "taker"),
    "restart-maker": lambda stamp: scenario_restart(stamp, "maker"),
    "survivor": scenario_survivor,
    "concurrent": scenario_concurrent,
    "taker-refund": scenario_taker_refund,
    "maker-refund": scenario_maker_refund,
}


def preflight() -> None:
    """The standing LEZ indexer stops serving historical reads once its state
    breakpoints lag (the Maker's sidecar then reports `unavailable` and never
    funds); repair-indexer.sh checks and re-indexes when needed."""
    log("preflight: indexer historical reads")
    result = subprocess.run(["bash", str(DEPLOY_ROOT / "scripts" / "repair-indexer.sh")],
                            capture_output=True, text=True, check=False, cwd=DEPLOY_ROOT)
    if result.returncode != 0:
        raise Failure(f"repair-indexer.sh failed: {result.stdout[-300:]} {result.stderr[-300:]}")
    for line in result.stdout.strip().splitlines()[-2:]:
        log(f"  {line}")


def run(name: str) -> bool:
    stamp = str(int(time.time()))
    log(f"=== {name} (stamp {stamp})")
    started = time.time()
    summary = {"scenario": name, "stamp": stamp, "timing": timing_profile()}
    try:
        for role in ("maker", "taker"):
            wait_healthy(role, timeout=60)
        preflight()
        summary.update(SCENARIOS[name](stamp))
        summary["result"] = "passed"
    except Failure as failure:
        summary["result"] = "failed"
        summary["error"] = str(failure)
        log(f"FAILED: {failure}")
    summary["seconds"] = round(time.time() - started)
    out = RUNTIME / "e2e"
    out.mkdir(parents=True, exist_ok=True)
    (out / f"{name}.json").write_text(json.dumps(summary, indent=2) + "\n")
    log(f"=== {name}: {summary['result']} in {summary['seconds']}s")
    return summary["result"] == "passed"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("scenario", choices=[*SCENARIOS, "all"])
    args = parser.parse_args()
    names = list(SCENARIOS) if args.scenario == "all" else [args.scenario]
    for name in names:
        if not run(name):
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
