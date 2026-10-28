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
    node-e2e.py early-lock       the Taker's lock is held unconfirmed; the Maker must not lock (R1)
    node-e2e.py offline-after-lock  Chat and Delivery cut after the lock; the swap finishes on chain state alone (R2)
    node-e2e.py taker-refund     Maker stopped before funding; Taker refunds its Bitcoin
    node-e2e.py maker-refund     Taker never claims; Maker refunds LEZ, then Taker refunds Bitcoin
    node-e2e.py refund-then-maker-lock  Refund admitted, then a late-but-timely maker lock; the observer must still recover
    node-e2e.py regenerated-config  live btc-role.json rendered again, Taker Node restarted: the swap reloads and completes
    node-e2e.py tampered-config     the swap's copied taker-role-config.json altered: the swap is refused after a restart
    node-e2e.py all              every scenario in sequence (stops at the first failure)

With `--pair Zcash` the harness runs the ZEC↔LEZ scenarios instead (TakerSellsLez,
the only Node-driven ZEC direction): a prepared corridor is provisioned with
scripts/zec-provision.sh, which also recreates both Nodes, then

    node-e2e.py --pair Zcash zec-happy   take, Taker locks LEZ (actor driven by the
                                         harness), Maker funds Zcash, Maker claims LEZ,
                                         Taker claims Zcash; claim found in a zebra block
    node-e2e.py --pair Zcash zec-taker-refund  Maker stopped before it funds; the Taker
                                         refunds its LEZ escrow; the Maker reconciles
    node-e2e.py --pair Zcash zec-maker-refund  both legs locked, the Taker refunds LEZ
                                         instead of waiting; the Maker refunds Zcash at
                                         the agreement's height; refund found in a block

The ZEC refund windows come from the agreement, not the timing profile. Under
the devnet profile zec-provision.sh asks for (deterministic-local-devnet-v1)
the Taker's LEZ refund opens 30 minutes after the corridor was provisioned
and the Maker's Zcash refund at an absolute zebra height (tip at provisioning
+ 5); the refund scenarios read those from the corridor and wait for them
rather than guessing, so neither needs the `fast` timing profile.

Every scenario runs in either Bitcoin direction: `--direction TakerSellsForeign`
(the default; the Taker locks Bitcoin, the Maker LEZ) or `--direction
TakerSellsLez` (the Taker locks LEZ first, the Maker Bitcoin, the Taker claims
Bitcoin). The summary of a reverse run is `runtime/e2e/<scenario>-lez.json`.

The refund scenarios need the `fast` timing profile
(`LEZ_TIMING_PROFILE=fast scripts/gen-config.sh runtime`, then recreate the
Nodes); the harness refuses to start them under the `local` profile.
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
RUNTIME = DEPLOY_ROOT / "runtime"
ROUTE = {"pair": "Bitcoin", "direction": "TakerSellsForeign"}  # --direction / --pair replace it
REVERSE = False  # True when the Taker sells LEZ (locks LEZ first, claims the foreign coin)
FOREIGN_UNITS = 1_000_000
LEZ_UNITS = 1_000
# The ZEC corridor's template (zec-reference-actor local_poc.rs): 1 ZEC for 50 000 LEZ units.
ZEC_FOREIGN_UNITS = 100_000_000
ZEC_LEZ_UNITS = 50_000
ZEBRA_URL = "http://127.0.0.1:18232/"  # Compose publishes zebra's RPC on the host loopback
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
    # No -i: curl takes the body from --data and never reads stdin, while the
    # attached stream leaves `docker exec` waiting on a stdin that the detached
    # runner never closes. The curl inside the container has long exited by
    # then, so --max-time does not bound the wait and the scenario hangs until
    # its own timeout. Every other exec here already runs without -i.
    result = subprocess.run(
        ["docker", "exec", container, "curl", "-sS", "--max-time", str(timeout), "--unix-socket", socket,
         "-H", "content-type: application/json", "--data", body, "http://localhost/"],
        capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL)
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
        if line.startswith(("LEZ_TIMING_PROFILE=", "LEZ_BTC_", "LEZ_LEZ_")):
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


def sign_funding(required: dict) -> str:
    """A Taker Node without a Bitcoin wallet answers where to pay. The lock is
    then signed, not sent, in a wallet the Node has no access to (the Maker's,
    here only because it is the other funded wallet on the regtest node)."""
    log(f"  funding required: {required['amount_sat']} sat to {required['address']}; signing outside the Node")
    wallet = "-rpcwallet=lez-maker"
    outputs = json.dumps([{required["address"]: f"{required['amount_sat'] / 1e8:.8f}"}])
    funded = bitcoin(wallet, "walletcreatefundedpsbt", "[]", outputs, "0", json.dumps({"lockUnspents": True}))
    signed = bitcoin(wallet, "walletprocesspsbt", funded["psbt"])
    return bitcoin("finalizepsbt", signed["psbt"])["hex"]


# A take that answers dependency-unavailable is retried on this schedule. The
# cheap races it was written for (a LEZ read that overtook the moving tip, Core
# briefly busy) clear in seconds, but the Maker also answers it while it is
# still locking LEZ for an earlier swap -- it serialises that step and tells the
# Taker to come back once the lock lands, which takes minutes on a loaded host.
# Thirty seconds of retries turned that wait into a scenario failure.
TAKE_RETRY_BACKOFF = (10, 20, 40, 60, 60, 60)


def initiate(request: dict) -> dict:
    """One take, retried with the same request id so a retry replays the take
    from its durable record instead of starting another."""
    for attempt in range(len(TAKE_RETRY_BACKOFF) + 1):
        reply = rpc("taker", "taker_swap_initiate_v1", request, timeout=420)
        if "result" in reply:
            return reply["result"]
        if reply["error"].get("code") == -32018 and "funding_transaction_hex" not in request:
            request["funding_transaction_hex"] = sign_funding(reply["error"]["data"])
            continue
        if reply["error"].get("code") != -32010 or attempt == len(TAKE_RETRY_BACKOFF):
            raise Failure(f"taker taker_swap_initiate_v1 failed: {json.dumps(reply['error'])[:300]}")
        delay = TAKE_RETRY_BACKOFF[attempt]
        log(f"  take answered dependency-unavailable "
            f"({(reply['error'].get('data') or {}).get('category')}); retrying in {delay} s")
        time.sleep(delay)
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
    for attempt in range(12):
        reply = rpc("taker", "taker_swap_monitor_v1", {"schema_version": 1, "swap_id": swap_id})
        if "result" in reply:
            return reply["result"].get("swap", reply["result"])
        if (reply["error"].get("data") or {}).get("category") != "taker_monitor_unavailable" or attempt == 11:
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


# Every `Phase` except `offered`, i.e. every observation the Maker can report that proves it
# has seen the Taker's lock on chain. Written as what disqualifies rather than what is allowed
# because maker_phase() falls back to the Maker's *scheduler* state when there is no
# observation yet — `leased`, `reserved`, `activated` and friends are not phases at all, and an
# allowlist of them cannot be complete. Getting that wrong fails a scenario that is working.
MAKER_OBSERVED_TAKER_LOCK = (
    "awaiting_taker_confirmations", "taker_lock_confirmed", "awaiting_maker_confirmations",
    "both_legs_locked", "taker_lock_reorged", "maker_lock_reorged", "claim_evidence_available",
    "completed", "maker_leg_refunded", "taker_leg_refunded", "refunded",
    "maker_recovery_available",
)


def maker_lock_recorded(swap_id: str) -> str | None:
    """The Maker's own record of its own lock, on whichever chain carries it in this
    direction: LEZ when the Taker sells Bitcoin, Bitcoin when the Taker sells LEZ.

    Asking the Maker for the effect, rather than reading its phase, is deliberate. A phase
    has to be compared against a list of names, and the list is the part that goes wrong:
    `maker_phase` falls back to the Maker's scheduler state when there is no observation
    yet, and `queued` and `leased` are scheduler states nobody enumerated. An effect either
    exists or it does not.
    """
    for effect in (maker_view(swap_id).get("effects") or []):
        if effect.get("kind") != "maker_lock":
            continue
        if (effect.get("chain") == "Bitcoin") == REVERSE:
            return effect.get("transaction_id")
    return None


def wait_maker_past_lock(swap_id: str, timeout: int = 600) -> str:
    """Waits until the Maker's own record shows its lock is in, which is what
    frees the one LEZ escrow slot for the next Taker-sells-BTC swap."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        recorded = maker_lock_recorded(swap_id)
        if recorded is not None:
            return recorded
        time.sleep(5)
    raise Failure(f"the Maker still owes its lock for {swap_id[:12]} after {timeout}s "
                  f"(phase {maker_phase(swap_id)})")


def lock(swap_id: str) -> str:
    # Locking plans and broadcasts through Bitcoin Core and the swap's sidecar; a
    # transient dependency outage (-32010) is retried, the request is idempotent.
    for attempt in range(1, 5):
        reply = rpc("taker", "taker_swap_lock_v1", {"schema_version": 1, "swap_id": swap_id}, timeout=120)
        if "result" in reply or reply.get("error", {}).get("code") != -32010 or attempt == 4:
            break
        log(f"  lock unavailable (attempt {attempt}): {json.dumps(reply['error'])[:120]}; retrying in 15s")
        time.sleep(15)
    if "error" in reply:
        raise Failure(f"taker taker_swap_lock_v1 failed: {json.dumps(reply['error'])}")
    result = reply["result"]
    log(f"  Taker locked {result.get('chain', '?')}: {result['transaction_id'][:12]} (replay={result.get('was_replay')})")
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
    log(f"  Taker claimed {ROUTE['pair'] if REVERSE else 'LEZ'} (replay={result.get('was_replay')})")
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


# Shapes RFP F2 asks for, checked on the real transaction rather than trusted from
# the code that built it: a cooperative claim is an ordinary Taproot key-path spend
# (one witness item, the 64-byte Schnorr signature, no script and no control block),
# and a refund is the script path (signature, script, control block).
BITCOIN_CLAIM_KINDS = ("revealing_claim", "followup_claim")
BITCOIN_REFUND_KINDS = ("maker_refund", "taker_refund")


def bitcoin_effects(swap_id: str, kinds: tuple[str, ...]) -> list[str]:
    view = taker_view(swap_id)
    return [effect["transaction_id"] for effect in (view.get("effects") or [])
            if effect.get("chain") == "Bitcoin" and effect.get("kind") in kinds]


def assert_witness_shape(transaction_id: str, items: int, label: str) -> None:
    decoded = bitcoin("getrawtransaction", transaction_id, "1")
    inputs = decoded["vin"]
    if len(inputs) != 1:
        raise Failure(f"{label} {transaction_id[:12]} spends {len(inputs)} inputs, expected one")
    witness = inputs[0].get("txinwitness") or []
    if len(witness) != items:
        raise Failure(f"{label} {transaction_id[:12]} has {len(witness)} witness items, expected {items}: {witness}")
    if items == 1 and len(witness[0]) != 128:
        raise Failure(f"{label} {transaction_id[:12]} witness item is {len(witness[0]) // 2} bytes, expected a 64-byte Schnorr signature")


def assert_cooperative_claim_is_key_path(swap_id: str) -> str:
    """F2: the cooperative claim leaves no script footprint on Bitcoin."""
    claims = bitcoin_effects(swap_id, BITCOIN_CLAIM_KINDS)
    if len(claims) != 1:
        raise Failure(f"expected one Bitcoin claim for {swap_id[:12]}, found {len(claims)}")
    assert_witness_shape(claims[0], 1, "the cooperative Bitcoin claim")
    log(f"  the Bitcoin claim {claims[0][:12]} is a Taproot key-path spend: one 64-byte signature, no script")
    return claims[0]


def assert_refund_is_script_path(swap_id: str, required: bool) -> str | None:
    """The refund is the other branch: signature, script, control block.

    Only one leg of a swap is Bitcoin, so a scenario that refunds the LEZ leg alone has
    nothing to assert here. The caller knows which it is and says so, because "no Bitcoin
    refund was recorded" must not be allowed to pass for "the refund had the right shape".
    """
    refunds = bitcoin_effects(swap_id, BITCOIN_REFUND_KINDS)
    if not refunds:
        if required:
            raise Failure(f"no Bitcoin refund was recorded for {swap_id[:12]}")
        return None
    assert_witness_shape(refunds[0], 3, "the Bitcoin refund")
    log(f"  the Bitcoin refund {refunds[0][:12]} is a script-path spend: three witness items")
    return refunds[0]


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
    return {"swap_id": swap_id, "bitcoin_claim": assert_cooperative_claim_is_key_path(swap_id)}


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


def ensure_coins(wallet: str, count: int) -> None:
    """Two concurrent swaps need two spendable coins in the wallet that locks
    Bitcoin: each funding plan locks its inputs, so a wallet with one coin can
    fund one swap at a time."""
    spendable = [u for u in bitcoin(f"-rpcwallet={wallet}", "listunspent") if float(u["amount"]) >= 0.02]
    if len(spendable) >= count:
        return
    log(f"  splitting the {wallet} wallet into {count} coins")
    for _ in range(count - len(spendable) + 1):
        address = bitcoin(f"-rpcwallet={wallet}", "getnewaddress", "", "bech32m")
        bitcoin(f"-rpcwallet={wallet}", "sendtoaddress", str(address), "0.05")
    mine(1)
    time.sleep(3)


def node_balances(role: str) -> dict:
    """What the desk shows under the account: the Node's own wallet balances."""
    if role == "maker":
        view = call("maker", "maker_wallet_balances_v1", {})
    else:
        view = call("taker", "taker_wallet_balances_v1", {"schema_version": 1})
    return {"btc_trusted_sat": (view.get("bitcoin") or {}).get("trusted_sat"),
            "lez_units": (view.get("lez") or {}).get("balance_atomic_units")}


def scenario_early_lock_reverse(stamp: str) -> dict:
    """R1 mirrored: the Taker locks LEZ, so the lock the Maker must not act on early is
    the LEZ one, and the lock the Maker must not send is Bitcoin.

    Nothing can hold a LEZ transaction out of a block the way the miner holds a Bitcoin
    one, so the Maker's sight of LEZ is cut instead: its loopback forwarder to the indexer
    is stopped while the Taker locks. The lock is real and finalizes on time; the Maker
    simply cannot see it, which is the same thing its observer faces when the chain is
    behind. It must not lock Bitcoin on a first leg it has not observed.
    """
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    log("  cutting the Maker's view of LEZ so it cannot see the Taker's lock")
    cut_lez_view("maker")
    try:
        wait_lez_view("maker", restored=False)
        lock(swap_id)
        log("  the Taker's LEZ lock is sent, and the Maker is blind to it")
        # Long enough for the Maker's observer to have polled several times over.
        deadline = time.time() + 90
        while time.time() < deadline:
            phase = maker_phase(swap_id)
            if phase in MAKER_OBSERVED_TAKER_LOCK:
                raise Failure(f"the Maker reached {phase} without having seen the Taker's lock")
            if bitcoin_effects(swap_id, ("maker_lock",)):
                raise Failure(
                    "the Maker locked Bitcoin before observing the Taker's lock. Its reads "
                    "over 127.0.0.1:8779 were already failing when this started, so either it "
                    "sees LEZ by some route other than the indexer forwarder — in which case "
                    "the mechanism cannot withhold the first leg and this scenario is wrong — "
                    "or it locks on a leg it has not observed, which is the R1 defect itself")
            time.sleep(5)
        log(f"  the Maker stayed at {maker_phase(swap_id)} for 90s, blind to the first leg")
    finally:
        restore_lez_view("maker")
    wait_lez_view("maker", restored=True)
    log("  the Maker can see LEZ again; it may now lock")
    wait_maker_past_lock(swap_id, timeout=900)
    claim(swap_id, stamp)
    wait_completed(swap_id)
    return {"swap_id": swap_id, "bitcoin_claim": assert_cooperative_claim_is_key_path(swap_id)}


def scenario_early_lock(stamp: str) -> dict:
    """R1: the Maker does not lock until the Taker's lock is confirmed.

    Forward the Taker's lock is the Bitcoin one, and on regtest the harness decides when a
    transaction confirms: the miner is held so the lock sits in the mempool, and the Maker
    must sit with it. Mirrored the Taker's lock is LEZ, which has no mempool to hold it in,
    so scenario_early_lock_reverse withholds the Maker's sight of it instead.
    """
    if REVERSE:
        return scenario_early_lock_reverse(stamp)
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    log("  stopping the miner so the Taker's lock stays unconfirmed")
    docker("stop", "lez-btc-miner")
    try:
        txid = lock(swap_id)
        confirmations = (bitcoin("getrawtransaction", txid, "1") or {}).get("confirmations", 0)
        if confirmations:
            raise Failure(f"the lock already has {confirmations} confirmations; the miner was not held")
        # Long enough for the Maker's observer to have polled several times over.
        deadline = time.time() + 90
        while time.time() < deadline:
            phase = maker_phase(swap_id)
            if phase in MAKER_OBSERVED_TAKER_LOCK:
                raise Failure(f"the Maker reached {phase} while the Taker's lock was unconfirmed")
            # Forward the Maker's lock is the LEZ one, so it is the Maker's own record that
            # has to stay empty; a Bitcoin effect filter would never match and would leave
            # this assertion always true.
            if maker_lock_recorded(swap_id) is not None:
                raise Failure("the Maker locked LEZ before the Taker's lock confirmed")
            time.sleep(5)
        log(f"  the Maker stayed at {maker_phase(swap_id)} for 90s with the lock unconfirmed")
        mine(1)
        log("  the Taker's lock is confirmed; the Maker may now lock")
        wait_maker_past_lock(swap_id, timeout=900)
    finally:
        docker("start", "lez-btc-miner")
        wait_healthy("maker")
    claim(swap_id, stamp)
    wait_completed(swap_id)
    return {"swap_id": swap_id, "lock_txid": txid,
            "bitcoin_claim": assert_cooperative_claim_is_key_path(swap_id)}


# Chat is a unix socket on the volume both Nodes share; Delivery is a directory the Maker
# writes and the Taker reads. Renaming one and closing the other's mode is how the Rust
# process tests cut them, and it needs no container restart.
CHAT_SOCKET = "/run/lez/maker/chat.sock"
CHAT_SOCKET_OFFLINE = "/run/lez/maker/chat.offline"


def maker_health() -> dict:
    return call("maker", "maker_health", {})


def cut_delivery_and_chat() -> None:
    docker("exec", "lez-maker-node", "mv", CHAT_SOCKET, CHAT_SOCKET_OFFLINE)
    docker("exec", "lez-maker-node", "chmod", "0755", "/delivery")


def restore_delivery_and_chat() -> None:
    subprocess.run(["docker", "exec", "lez-maker-node", "mv", CHAT_SOCKET_OFFLINE, CHAT_SOCKET],
                   check=False, capture_output=True)
    subprocess.run(["docker", "exec", "lez-maker-node", "chmod", "0700", "/delivery"],
                   check=False, capture_output=True)


def scenario_offline_after_lock(stamp: str) -> dict:
    """R2: once the first lock is on chain the swap proceeds on chain state alone.

    Both off-chain dependencies are cut after the lock and stay down until the swap is
    complete. Neither Node is restarted: the point is that a swap in flight does not need
    Logos Chat or Delivery, not that it survives a restart.
    """
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    lock(swap_id)
    log("  cutting Logos Chat and Delivery, with the first lock already on chain")
    cut_delivery_and_chat()
    try:
        deadline = time.time() + 120
        while time.time() < deadline:
            health = maker_health()
            if health.get("chat") == "unavailable" and health.get("delivery") == "unavailable":
                break
            time.sleep(5)
        else:
            raise Failure(f"the Maker did not report both dependencies unavailable: {json.dumps(health)[:200]}")
        if not health.get("degraded"):
            raise Failure("the Maker reports both dependencies unavailable but is not degraded")
        log("  the Maker reports chat and delivery unavailable, and degraded")
        claim(swap_id, stamp)
        wait_completed(swap_id)
        log("  the swap completed with both off-chain dependencies down")
    finally:
        restore_delivery_and_chat()
    deadline = time.time() + 120
    while time.time() < deadline:
        health = maker_health()
        if not health.get("degraded"):
            log("  both dependencies are back and the Maker is no longer degraded")
            break
        time.sleep(5)
    else:
        raise Failure(f"the Maker stayed degraded after both dependencies returned: {json.dumps(health)[:200]}")
    return {"swap_id": swap_id, "bitcoin_claim": assert_cooperative_claim_is_key_path(swap_id)}


def scenario_concurrent(stamp: str) -> dict:
    ensure_coins("lez-maker" if REVERSE else "lez-taker", 2)
    ids = []
    if REVERSE:
        for suffix in ("-a", "-b"):
            offer_id = publish_offer(stamp, suffix)
            swap_id, _ = take(offer_id, stamp, suffix)
            ids.append((suffix, swap_id))
        for _, swap_id in ids:
            lock(swap_id)
    else:
        # The Maker locks LEZ here, and it prepares one LEZ escrow at a time: a
        # second swap taken while the first still owes its lock could miss its
        # cutoff on a slow-finality network and strand the Taker's Bitcoin for
        # the whole refund timelock. The Maker refuses it at the reservation,
        # before the Taker has locked anything, and admits it once the first
        # lock is confirmed. The two swaps still overlap from there on.
        offer_a, offer_b = publish_offer(stamp, "-a"), publish_offer(stamp, "-b")
        swap_a, _ = take(offer_a, stamp, "-a")
        expect_error("second Taker-sells-BTC swap while the Maker's LEZ lock is owed", "taker",
                     "taker_swap_initiate_v1", take_request(offer_b, discover(offer_b), f"e2e-take-{stamp}-b-early"),
                     category="initiation_execution_unavailable")
        lock(swap_a)
        wait_taker(swap_a, {"claim_available"}, timeout=1500, describe="(Maker funding)")
        # The Taker sees the Maker's LEZ lock as soon as it is final; the Maker
        # frees its escrow slot only once its own record has advanced past the
        # lock, which is a little later. The refusal is decided on the Maker's
        # view, so the second take waits for that, not for the Taker's.
        wait_maker_past_lock(swap_a, timeout=600)
        swap_b, _ = take(offer_b, stamp, "-b")
        lock(swap_b)
        ids = [("-a", swap_a), ("-b", swap_b)]
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


def request_refund_until_terminal(swap_id: str, stamp: str, timeout: int, mine_every: int = 60) -> dict:
    """The refund is a repeatable operator action: the Node answers
    `taker_action_execution_unavailable` while the actor's recovery checks are
    not yet satisfied and admits the request once they are. One request id is
    replayed throughout: a replay re-drives the admitted recovery, and the Node's
    own observer keeps driving it between requests. A block is mined between
    attempts so Bitcoin maturity and finality advance on regtest."""
    deadline = time.time() + timeout
    attempt = 0
    while time.time() < deadline:
        view = taker_view(swap_id)
        if view["state"] == "refunded":
            return view
        if view["state"] == "attention_required" or view["state"] == "completed":
            raise Failure(f"swap {swap_id[:12]} ended in {view['state']} instead of refunded")
        attempt += 1
        reply = rpc("taker", "taker_swap_refund_v1", {"schema_version": 1, "request_id": f"e2e-refund-{stamp}",
                                                     "swap_id": swap_id, "expected_generation": view["progress_generation"]},
                    timeout=300)
        if "result" in reply:
            if not reply["result"].get("was_replay") or attempt % 5 == 1:
                log(f"  refund request {'replayed' if reply['result'].get('was_replay') else 'admitted'} "
                    f"(attempt {attempt}, state {view['state']})")
        else:
            category = (reply["error"].get("data") or {}).get("category")
            if category not in {"taker_action_execution_unavailable", "taker_action_conflict", "taker_action_unavailable"}:
                raise Failure(f"taker_swap_refund_v1 failed: {json.dumps(reply['error'])[:300]}")
            if attempt % 5 == 1:
                log(f"  refund not yet accepted (attempt {attempt}: {category}); taker {view['state']} | maker: {maker_phase(swap_id)}")
        mine(1)
        time.sleep(mine_every)
    raise Failure(f"swap {swap_id[:12]} was not refunded within {timeout}s")


def scenario_taker_refund(stamp: str) -> dict:
    profile = require_fast_profile()
    csv_blocks = int(profile["LEZ_BTC_REFUND_CSV_BLOCKS"])
    cutoff = int(profile["LEZ_BTC_MAKER_LOCK_CUTOFF_SECONDS"])
    # The Taker's leg is locked first and refunds later: Bitcoin once the CSV
    # matures (forward), LEZ once the later refund opens (reverse).
    opens = int(profile["LEZ_BTC_LATER_REFUND_SECONDS"]) if REVERSE else cutoff
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    log(f"  stopping the Maker Node so it never {'locks Bitcoin' if REVERSE else 'funds'}")
    docker("stop", NODES["maker"][0])
    before = node_balances("taker")
    try:
        txid = lock(swap_id)
        mine(csv_blocks + 1)
        log(f"  mined {csv_blocks + 1} blocks past the lock; the Maker cutoff is {cutoff}s after the take, "
            f"the Taker's refund opens {opens}s after it")
        request_refund_until_terminal(swap_id, stamp, timeout=opens + 1500)
        after = node_balances("taker")
        log(f"  Taker refunded; Taker Node balances {before} → {after}")
    finally:
        docker("start", NODES["maker"][0])
        wait_healthy("maker")
    # The Maker follows the Taker's refund to Bitcoin finality; keep regtest
    # blocks coming so finality does not wait on the two-minute miner alone.
    deadline = time.time() + 900
    while time.time() < deadline:
        view = maker_view(swap_id)
        if view.get("schedule_state") == "terminal":
            log(f"  Maker reconciled to {maker_phase(swap_id)}")
            break
        mine(1)
        time.sleep(20)
    else:
        raise Failure("Maker did not reconcile the refunded swap to a terminal state")
    return {"swap_id": swap_id, "lock_txid": txid, "taker_balance_before": before,
            "taker_balance_after": after,
            "bitcoin_refund": assert_refund_is_script_path(swap_id, required=not REVERSE)}


STALE_GENERATION = "generation is stale"


def maker_action_with_fresh_generation(method: str, swap_id: str, request_id: str, attempts: int = 5) -> dict:
    """A manual Maker action (refund, claim) is guarded by the actor's lease
    generation: the request names the generation it was decided on and the
    Node refuses it once the worker has moved on. Between reading the view and
    sending, the worker may run; that refusal (and only that one) is answered
    by reading the generation again and resending the same request id, a
    bounded number of times. Any other error still fails the scenario."""
    last = None
    for attempt in range(1, attempts + 1):
        view = maker_view(swap_id)
        reply = rpc("maker", method, {"request_id": request_id, "id": swap_id,
                                      "expected_generation": view["lease_generation"]})
        if "error" not in reply:
            if attempt > 1:
                log(f"  {method} accepted at generation {view['lease_generation']} (attempt {attempt})")
            return reply["result"]
        last = reply["error"]
        if STALE_GENERATION not in str(last.get("message", "")):
            raise Failure(f"maker {method} failed: {json.dumps(last)[:300]}")
        log(f"  {method}: generation {view['lease_generation']} was stale (attempt {attempt}); reading it again")
        time.sleep(3)
    raise Failure(f"maker {method} kept refusing a stale generation after {attempts} attempts: {json.dumps(last)[:200]}")


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
    # Both legs are locked; the Bitcoin leg's CSV must mature before its
    # refund, whichever role locked it.
    mine(csv_blocks + 1)
    log(f"  Maker locked; the Taker never claims. Waiting for the Maker's refund deadline ({earlier}s after the take)")
    while time.time() < started + earlier + 30:
        time.sleep(20)
    result = maker_action_with_fresh_generation("maker_actor_refund_v1", swap_id, f"e2e-maker-refund-{stamp}")
    log(f"  Maker refund queued: {json.dumps(result)[:160]}")
    # The Maker asserts the absent lock only once its whole LEZ discovery window
    # is finalized, so a flat 15-minute wait raced the chain and failed a refund
    # the Node went on to complete. Size it from the profile's own window
    # (devnet slots are 10 s) so the two cannot drift apart again.
    deadline = time.time() + 10 * int(profile["LEZ_LEZ_DISCOVERY_MAX_BLOCKS"]) + 600
    while time.time() < deadline:
        phase = maker_phase(swap_id)
        log(f"  maker {swap_id[:12]}: {phase}")
        if phase in {"maker_leg_refunded", "refunded"}:
            break
        time.sleep(20)
    else:
        raise Failure("Maker did not refund its leg")
    while time.time() < started + later + 30:
        time.sleep(20)
    request_refund_until_terminal(swap_id, stamp, timeout=1500)
    log("  both legs refunded")
    return {"swap_id": swap_id,
            "bitcoin_refund": assert_refund_is_script_path(swap_id, required=True)}


def wait_until(unix_seconds: float, describe: str) -> None:
    remaining = unix_seconds - time.time()
    if remaining > 0:
        log(f"  waiting {int(remaining)}s {describe}")
        time.sleep(remaining)


def wait_mempool_transaction(timeout: float) -> str:
    """The first transaction to reach Bitcoin Core's mempool. With the block
    miner stopped nothing else is broadcast, so it is the Maker's lock."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        pool = bitcoin("getrawmempool")
        if isinstance(pool, list) and pool:
            return str(pool[0])
        time.sleep(5)
    raise Failure(f"no Maker lock reached the mempool within {int(timeout)}s; the scenario cannot make it late")


# The Node containers reach the chains over loopback forwarders their entrypoint
# starts, one socat per service. Stopping the Taker's forwarder to the LEZ indexer
# blinds the Taker to LEZ and nothing else: the Maker keeps its own, so its lock
# is submitted, included and finalized on time. That is the LEZ counterpart of
# holding a Bitcoin lock in the mempool — the lock is real and timely, and the
# Taker has simply not seen it yet. LEZ has no mempool, so there is no other way
# to separate "the lock happened" from "the Taker observed it".
INDEXER_FORWARDER = "TCP-LISTEN:8779"


def cut_lez_view(role: str) -> None:
    subprocess.run(["docker", "exec", "-u", "root", NODES[role][0], "pkill", "-f", INDEXER_FORWARDER],
                   check=False, capture_output=True)


def restore_lez_view(role: str) -> None:
    # Clear first: a restore that runs while an earlier forwarder is still bound leaves two
    # of them, and the one that wins the port is not necessarily the one that works. Killing
    # before starting makes this idempotent, which matters because it runs from a `finally`
    # and may run when the view was never cut.
    cut_lez_view(role)
    subprocess.run(["docker", "exec", "-d", "-u", "root", NODES[role][0], "socat",
                    f"{INDEXER_FORWARDER},bind=127.0.0.1,fork,reuseaddr", "TCP:indexer:8779"],
                   check=False, capture_output=True)


def lez_readable(role: str) -> bool:
    """Whether that Node can read the LEZ indexer over its own loopback forwarder."""
    container, _ = NODES[role]
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getLastFinalizedBlockId", "params": []})
    result = subprocess.run(["docker", "exec", container, "curl", "-sf", "-m", "10",
                             "-H", "content-type: application/json", "--data", body, "http://127.0.0.1:8779/"],
                            capture_output=True, text=True, check=False)
    try:
        return int(json.loads(result.stdout)["result"]) > 0
    except (ValueError, KeyError, TypeError):
        return False


def wait_lez_view(role: str, restored: bool, timeout: int = 120) -> None:
    """Waits until that Node's LEZ reads fail (or succeed again), so a scenario never
    races the forwarder it just stopped or started."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        if lez_readable(role) == restored:
            return
        time.sleep(3)
    raise Failure(f"the {role}'s LEZ view is still "
                  f"{'unreadable' if restored else 'readable'} after {timeout}s")


def scenario_refund_admitted_forward(stamp: str) -> dict:
    """The forward mirror of scenario_refund_admitted_then_maker_lock: here the
    Taker locks Bitcoin first and the second lock, the one the observer has to
    project after a refund is already admitted, is the Maker's LEZ lock.

    The Maker locks on time and its lock finalizes on time; only the Taker's view
    of LEZ is cut, so the Taker reaches its cutoff having observed no maker lock,
    routes to recovery and admits a refund. Restoring the view then presents the
    observer with exactly the reported condition — an admitted refund and a
    canonical, timely, unprojected maker lock — and it must finish the swap on
    its own."""
    require_fast_profile()
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    terms = taker_view(swap_id)["terms"]
    cutoff = int(terms["maker_second_lock_cutoff_unix_seconds"])
    opens = int(terms["later_refund_earliest_unix_seconds"])
    before = node_balances("taker")
    # Lock first, then blind: the Taker's own Bitcoin lock is planned through its
    # sidecar, which reads LEZ.
    taker_lock = lock(swap_id)
    log(f"  Taker's Bitcoin lock {taker_lock[:12]} is in; cutting the Taker's view of LEZ")
    cut_lez_view("taker")
    try:
        wait_lez_view("taker", restored=False)
        # The Maker is not blind, so its own record is the honest witness that the
        # LEZ lock went out on time, well before the cutoff.
        maker_lock = wait_maker_past_lock(swap_id, timeout=max(120, cutoff - int(time.time()) - 60))
        log(f"  the Maker's LEZ lock {maker_lock[:12]} is in, {cutoff - int(time.time())}s before "
            "the cutoff, and the Taker cannot see it")
        wait_until(cutoff + 30, "for the cutoff to pass with the Taker still blind to LEZ")
        if taker_view(swap_id)["state"] == "completed":
            raise Failure("the Taker completed the swap; its LEZ view was not cut")
        reply = rpc("taker", "taker_swap_refund_v1", {
            "schema_version": 1, "request_id": f"e2e-refund-{stamp}",
            "swap_id": swap_id, "expected_generation": taker_view(swap_id)["progress_generation"]})
        if "result" not in reply:
            # The expected way for this scenario to be impossible rather than failing: the
            # Taker's cutoff is a LEZ clock comparison, so a Taker that cannot read LEZ may
            # be unable to conclude the cutoff has passed and will refuse the refund. That is
            # an answer about the mechanism, not a defect in the Node — record it and drop
            # this scenario rather than trying to force the admission.
            raise Failure(
                f"refund admission failed while the Taker was blind to LEZ: "
                f"{json.dumps(reply.get('error'))[:200]}")
        log(f"  refund admitted at rev {taker_view(swap_id)['progress_generation']} (maker lock unobserved)")
    finally:
        restore_lez_view("taker")
    wait_lez_view("taker", restored=True)
    log("  the Taker's view of LEZ is back; the maker lock is now canonical and unprojected")
    mine(int(require_fast_profile()["LEZ_BTC_REFUND_CSV_BLOCKS"]) + 1)
    # Hand off to the Node observer alone. Do NOT drive the actor from here; only
    # wait (mining to advance regtest finality) for it to reach refunded.
    wait_until(opens + 5, "for the Taker's Bitcoin refund window to open")
    deadline = time.time() + 1800
    while time.time() < deadline:
        state = taker_view(swap_id)["state"]
        if state == "refunded":
            break
        if state in ("completed", "attention_required"):
            raise Failure(f"swap {swap_id[:12]} ended in {state}, not refunded")
        mine(1)
        time.sleep(30)
    else:
        raise Failure(f"the observer did not recover swap {swap_id[:12]} within 1800s "
                      "(the admitted-refund short-circuit is unfixed)")
    after = node_balances("taker")
    log(f"  observer recovered the swap unaided; Taker balances {before} → {after}")
    return {"swap_id": swap_id, "taker_lock_txid": taker_lock, "cutoff": cutoff,
            "taker_balance_before": before, "taker_balance_after": after}


def scenario_refund_admitted_then_maker_lock(stamp: str) -> dict:
    """A refund admitted at revision one, then a maker lock that arrives after
    the cutoff but is still timely by median time. The observer's admitted-
    refund branch ran only `recover` (which observes: "the lock is canonical,
    project it") and never `drive`, so nothing projected the lock and the swap
    froze at TakerLockConfirmed forever — the Taker's LEZ locked, every refund
    admitted, nothing sent (public testnet, swap 54184a0e, 2026-09-16). The fix
    drives the projection first in the observation phases; the observer then
    carries the swap through recovery on its own.

    Runs both ways, because the second lock — the one the observer has to project
    — is a different chain in each. Reversed it is Bitcoin and the harness holds
    it in the mempool; forward it is LEZ, which has no mempool to hold, so the
    Taker's own view of LEZ is cut instead (see scenario_refund_admitted_forward).
    The recovery is left entirely to the Node observer — the harness never drives
    the actor by hand — so a pass is proof the patched observer completes it
    unaided."""
    if not REVERSE:
        return scenario_refund_admitted_forward(stamp)
    require_fast_profile()
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    terms = taker_view(swap_id)["terms"]
    cutoff = int(terms["maker_second_lock_cutoff_unix_seconds"])
    opens = int(terms["later_refund_earliest_unix_seconds"])
    before = node_balances("taker")
    # Stop the miner, not the Maker: the Maker broadcasts its Bitcoin lock on
    # time (once the Taker's LEZ lock finalizes), but held out of a block it
    # stays unconfirmed, so the Taker never observes a maker lock and routes to
    # recovery at the cutoff. Confirming it afterwards makes it present and
    # timely-by-median — exactly the wild condition (a slow inclusion), which a
    # Maker held down past its own cutoff cannot reproduce (it refuses to lock).
    log("  stopping the miner so the Maker's lock is broadcast on time but not yet confirmed")
    docker("stop", "lez-btc-miner")
    miner_down = True
    try:
        lock(swap_id)
        maker_lock = wait_mempool_transaction(timeout=max(60, cutoff - int(time.time()) - 30))
        log(f"  Maker's Bitcoin lock {maker_lock[:12]} is broadcast (in mempool), unconfirmed")
        wait_until(cutoff + 30, "for the cutoff to pass with the maker lock still unconfirmed")
        reply = rpc("taker", "taker_swap_refund_v1", {
            "schema_version": 1, "request_id": f"e2e-refund-{stamp}",
            "swap_id": swap_id, "expected_generation": taker_view(swap_id)["progress_generation"]})
        if "result" not in reply:
            raise Failure(f"refund admission failed: {json.dumps(reply.get('error'))[:200]}")
        log(f"  refund admitted at rev {taker_view(swap_id)['progress_generation']} (maker lock still unconfirmed)")
        [block_hash] = bitcoin("generatetoaddress", "1", MINER_ADDRESS)
        blk = bitcoin("getblock", block_hash, "1")
        if maker_lock not in blk["tx"]:
            raise Failure(f"block {block_hash[:12]} did not include the maker lock {maker_lock[:12]}")
        by_median = "timely" if int(blk["mediantime"]) <= cutoff else "late"
        log(f"  maker lock confirmed; block median {blk['mediantime']} vs cutoff {cutoff} ({by_median} by median)")
        mine(int(require_fast_profile()["LEZ_BTC_REFUND_CSV_BLOCKS"]) + 1)
        # Hand off to the Node observer alone. Do NOT drive the actor from here;
        # only wait (mining to advance regtest finality) for it to reach refunded.
        wait_until(opens + 5, "for the Taker's LEZ refund window to open")
        deadline = time.time() + 1800
        while time.time() < deadline:
            st = taker_view(swap_id)["state"]
            if st == "refunded":
                break
            if st in ("completed", "attention_required"):
                raise Failure(f"swap {swap_id[:12]} ended in {st}, not refunded")
            mine(1)
            time.sleep(30)
        else:
            raise Failure(f"the observer did not recover swap {swap_id[:12]} within 1800s "
                          "(the admitted-refund short-circuit is unfixed)")
        after = node_balances("taker")
        log(f"  observer recovered the swap unaided; Taker balances {before} → {after}")
    finally:
        if miner_down:
            docker("start", "lez-btc-miner")
    return {"swap_id": swap_id, "maker_lock_txid": maker_lock, "cutoff": cutoff,
            "taker_balance_before": before, "taker_balance_after": after}


def taker_swap_directory(swap_id: str) -> str:
    """The Taker Node's per-swap directory (the record stores the swap id as bytes)."""
    reader = (
        "import glob, json, sys\n"
        "for f in glob.glob('/var/lib/lez/taker/btc/swaps/*/taker-swap.json'):\n"
        "    s = json.load(open(f))\n"
        "    if bytes(s.get('swap_id') or []).hex() == sys.argv[1]: print(f.rsplit('/', 1)[0]); break\n"
    )
    result = subprocess.run(["docker", "exec", NODES["taker"][0], "python3", "-c", reader, swap_id],
                            capture_output=True, text=True, check=False)
    directory = result.stdout.strip()
    if not directory:
        raise Failure(f"the Taker Node has no directory for swap {swap_id[:12]}")
    return directory


def regenerate_runtime_config() -> None:
    """Renders the runtime configuration again (same chain, same identities), as a
    maintenance run of gen-config.sh does; the live btc-role.json is rendered from
    it on the Node's next start."""
    result = subprocess.run(["bash", str(DEPLOY_ROOT / "scripts" / "gen-config.sh"), "runtime"],
                            capture_output=True, text=True, check=False, cwd=DEPLOY_ROOT)
    if result.returncode != 0:
        raise Failure(f"gen-config.sh failed: {result.stderr[-300:]}")
    log("  runtime configuration rendered again (chain and identities unchanged)")


def scenario_regenerated_config(stamp: str) -> dict:
    """Review follow-up on #28: a swap taken under one rendering of the live
    role configuration survives a re-render and a Taker Node restart, reloads
    against its own saved copy, and stays actionable to completion."""
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    lock(swap_id)
    regenerate_runtime_config()
    log("  restarting the Taker Node onto the re-rendered configuration")
    docker("restart", NODES["taker"][0])
    wait_healthy("taker")
    listed = [s for s in call("taker", "taker_swap_list_v1", {"schema_version": 1})["swaps"] if s["swap_id"] == swap_id]
    if len(listed) != 1 or listed[0]["state"] in TERMINAL:
        raise Failure(f"the swap did not reload after the restart: {listed}")
    log(f"  swap {swap_id[:12]} reloaded as {listed[0]['state']}")
    claim(swap_id, stamp)
    wait_completed(swap_id)
    return {"swap_id": swap_id, "reloaded_state": listed[0]["state"]}


def scenario_tampered_config(stamp: str) -> dict:
    """Review follow-up on #28: the copy of the role configuration a swap was
    taken under is integrity-checked; an altered copy is refused after a restart
    and the swap cannot be acted on."""
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    directory = taker_swap_directory(swap_id)
    docker("exec", NODES["taker"][0], "sh", "-c",
           f"printf '\n' >> {directory}/taker-role-config.json")
    log("  appended one byte to the swap's copied taker-role-config.json")
    docker("restart", NODES["taker"][0])
    wait_healthy("taker")
    listed = [s for s in call("taker", "taker_swap_list_v1", {"schema_version": 1})["swaps"] if s["swap_id"] == swap_id]
    state = listed[0]["state"] if listed else "absent"
    lock_reply = rpc("taker", "taker_swap_lock_v1", {"schema_version": 1, "swap_id": swap_id}, timeout=120)
    if "result" in lock_reply:
        raise Failure(f"the lock was accepted on a swap whose saved configuration was altered ({state})")
    if listed and state not in {"attention_required"}:
        raise Failure(f"the altered swap still lists as {state} instead of attention_required")
    log(f"  swap {swap_id[:12]} after the tamper: {state}; lock refused ({(lock_reply['error'].get('data') or {}).get('category')})")
    return {"swap_id": swap_id, "state_after_tamper": state,
            "lock_refusal": (lock_reply["error"].get("data") or {}).get("category")}


# ---- ZEC↔LEZ ------------------------------------------------------------------------
# A ZEC swap is prepared, not composed: the Taker takes a Zcash offer only from a
# static prepared_zec catalog entry, built from live chain facts by
# scripts/zec-provision.sh (which publishes the offer, writes the catalog and
# recreates both Nodes). The Taker Node runs its ZEC actor only for status, claim
# and recover; it never drives it, so the first LEZ lock and every observation up
# to claim_available are driven from here with `lez-zec-taker-actor … drive`
# inside the Taker container. Zebra has no miner: blocks are generated here, one
# when something waits in its mempool and one when the Taker reports it is
# waiting for safe funding depth (the pre-reveal gate wants two confirmations),
# never otherwise, because the agreement's Zcash refund height is absolute.

ZEC_CORRIDOR: dict = {}


def zebra(method: str, params: list | None = None) -> object:
    import urllib.request
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or []}).encode()
    request = urllib.request.Request(ZEBRA_URL, data=body, headers={"content-type": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            reply = json.load(response)
    except OSError as error:
        raise Failure(f"zebra {method} unreachable: {error}") from error
    if reply.get("error"):
        raise Failure(f"zebra {method} failed: {json.dumps(reply['error'])[:200]}")
    return reply["result"]


def zebra_mine(blocks: int, why: str) -> None:
    zebra("generate", [blocks])
    log(f"  zebra: mined {blocks} block(s) → height {zebra('getblockcount')} ({why})")


ZEC_CORRIDORS: list[dict] = []


def zec_provision(swaps: int = 1, swap_id: str | None = None) -> dict:
    """One fresh corridor run: zec-provision.sh end to end, Nodes recreated. With
    `swaps` 2 the run holds two prepared entries (the concurrent fixture: its own
    corridor root, agreement, offer, sidecar ports and funder output each).
    Returns the first entry's corridor facts, or the entry of `swap_id` when the
    corridor is reused; every entry is kept in ZEC_CORRIDORS."""
    if os.environ.get("LEZ_ZEC_REUSE_CORRIDOR") == "1":
        # The corridor the Nodes already run with (runtime/zec/current), for a
        # run that must not recreate them; its catalog entry must be untaken.
        log("  reusing the provisioned corridor the Nodes already loaded (LEZ_ZEC_REUSE_CORRIDOR=1)")
    else:
        log(f"  provisioning a ZEC corridor with {swaps} prepared swap(s) (zec-provision.sh; this recreates both Nodes)")
        result = subprocess.run(["bash", str(DEPLOY_ROOT / "scripts" / "zec-provision.sh"), "--skip-chain-check",
                                 "--swaps", str(swaps)],
                                capture_output=True, text=True, check=False, cwd=DEPLOY_ROOT, stdin=subprocess.DEVNULL)
        for line in result.stdout.strip().splitlines()[-6:]:
            log(f"    {line}")
        if result.returncode != 0:
            raise Failure(f"zec-provision.sh failed: {result.stderr.strip()[-300:] or result.stdout.strip()[-300:]}")
    run_id = (RUNTIME / "zec" / "current").read_text().strip()
    out = RUNTIME / "zec" / run_id
    entries = json.loads((out / "prepared.json").read_text())
    corridors = []
    for index, entry in enumerate(entries):
        # Entry k of the run: its files under runtime/zec/<run>/ (k = 0) or
        # runtime/zec/<run>/b/, its corridor root corridor/ or corridor-b/.
        entry_out = out if index == 0 else out / "b"
        corridor_root = "/var/lib/lez/zec/corridor" + ("" if index == 0 else "-b")
        summary = json.loads((entry_out / "provision-summary.json").read_text())
        # The Maker's actor config names the coinbase outpoint its funding spends; it
        # is the one fact that lets the scenario find the funding and claim on zebra.
        reader = ("import json,sys; c=json.load(open(sys.argv[1] + '/maker/actor-config.json'));"
                  " print(json.dumps(c['zcash_funding_outpoints']))")
        read = subprocess.run(["docker", "exec", NODES["maker"][0], "python3", "-c", reader, corridor_root],
                              capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL)
        if read.returncode != 0 or not read.stdout.strip():
            raise Failure(f"could not read the Maker's funding outpoint for entry {index}: {read.stderr.strip()[:200]}")
        spec = json.loads((entry_out / "spec.json").read_text())
        profile = spec.get("profile", "deterministic-local-v1")
        corridors.append({"run_id": run_id, "entry": index, "offer_id": entry["offer_id"], "catalog_swap_id": entry["swap_id"],
                          "profile": profile,
                          # zec-swap-sdk profile.rs: the only parameter the devnet profile changes.
                          "lez_refund_delay_seconds": 1800 if profile == "deterministic-local-devnet-v1" else 60,
                          "receipt_output": entry["receipt_output"], "actor_root": entry["actor_root"],
                          "zebra_tip_at_provisioning": int(summary["zebra_tip_height"]),
                          "zcash_refund_height": int(summary["zcash_refund_height"]),
                          "funding_outpoints": json.loads(read.stdout)})
    for corridor in corridors:
        log(f"  corridor {run_id} entry {corridor['entry']} ({corridor['profile']}): offer {corridor['offer_id']}, "
            f"zebra tip {corridor['zebra_tip_at_provisioning']}, Zcash refund height {corridor['zcash_refund_height']}")
    ZEC_CORRIDORS.clear()
    ZEC_CORRIDORS.extend(corridors)
    corridor = corridors[0]
    if swap_id is not None:
        matching = [c for c in corridors if c["catalog_swap_id"] == swap_id]
        if not matching:
            raise Failure(f"swap {swap_id[:12]} is not an entry of corridor {run_id} ({[c['catalog_swap_id'] for c in corridors]})")
        corridor = matching[0]
    ZEC_CORRIDOR.clear()
    ZEC_CORRIDOR.update(corridor)
    return corridor


def zec_taker_actor_config(receipt_output: str) -> str:
    """The take wrote an acceptance receipt naming the provisioned Taker actor config."""
    result = subprocess.run(["docker", "exec", NODES["taker"][0], "cat", receipt_output],
                            capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL)
    if result.returncode != 0:
        raise Failure(f"no acceptance receipt at {receipt_output}: {result.stderr.strip()[:200]}")
    receipt = json.loads(result.stdout)
    log(f"  receipt: swap {receipt.get('swap_id')}, actor config {receipt.get('actor_config_file')}")
    return receipt["actor_config_file"]


def zec_activate(config: str) -> None:
    """The take provisions the Taker's ZEC actor but the Node's activate step is a
    no-op for ZEC (lifecycle.rs), so the actor is activated from here; a replay
    answers `activated` again and is not an error."""
    result = subprocess.run(["docker", "exec", NODES["taker"][0], "/usr/local/bin/lez-zec-taker-actor",
                             "--config", config, "activate"],
                            capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL, timeout=300)
    text = result.stdout.strip()
    if result.returncode != 0 or not text:
        raise Failure(f"lez-zec-taker-actor activate failed: {(result.stderr.strip() or text)[-200:]}")
    effect = json.loads(text.splitlines()[-1])
    log(f"  Taker actor activated: {zec_describe(effect)}")


def zec_take_or_resume(offer_id: str, stamp: str) -> str:
    """A prepared entry is one take: when this offer was already taken (a rerun
    after a harness failure), the existing swap is resumed instead."""
    for swap in call("taker", "taker_swap_list_v1", {"schema_version": 1})["swaps"]:
        if swap.get("offer_id") == offer_id and swap["state"] not in TERMINAL:
            log(f"  resuming swap {swap['swap_id'][:12]} already taken for {offer_id} ({swap['state']})")
            return swap["swap_id"]
    swap_id, _ = take(offer_id, stamp)
    return swap_id


def zec_drive(config: str) -> dict | None:
    """One `drive` of the Taker's ZEC actor; None when the actor refused (transient
    sidecar/chain reads) so the caller keeps polling."""
    result = subprocess.run(["docker", "exec", NODES["taker"][0], "/usr/local/bin/lez-zec-taker-actor",
                             "--config", config, "drive"],
                            capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL, timeout=300)
    text = result.stdout.strip()
    if result.returncode != 0 or not text:
        log(f"  drive: exit {result.returncode}: {(result.stderr.strip() or text)[-200:]}")
        return None
    try:
        return json.loads(text.splitlines()[-1])
    except ValueError:
        log(f"  drive: unparsable output: {text[-200:]}")
        return None


def zec_status(config: str) -> dict | None:
    """One `status` of the Taker's ZEC actor (durable phase, no chain access)."""
    result = subprocess.run(["docker", "exec", NODES["taker"][0], "/usr/local/bin/lez-zec-taker-actor",
                             "--config", config, "status"],
                            capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL, timeout=120)
    text = result.stdout.strip()
    if result.returncode != 0 or not text:
        return None
    try:
        return json.loads(text.splitlines()[-1])
    except ValueError:
        return None


def zec_describe(effect: dict | None) -> str:
    if not effect:
        return "drive refused"
    return " ".join(str(effect.get(k)) for k in ("outcome", "operation", "phase", "next_action") if effect.get(k))


def zec_spend_of(outpoint: tuple[str, int], from_height: int) -> tuple[str, int] | None:
    """The transaction in a zebra block at or above `from_height` that spends `outpoint`."""
    tip = int(zebra("getblockcount"))
    for height in range(from_height, tip + 1):
        block = zebra("getblock", [str(height), 1])
        for txid in block.get("tx") or []:
            raw = zebra("getrawtransaction", [txid, 1])
            for vin in raw.get("vin") or []:
                if vin.get("txid") == outpoint[0] and int(vin.get("vout", -1)) == outpoint[1]:
                    return txid, height
    return None


def zec_funding_confirmations(corridor: dict) -> int | None:
    """Confirmations of the Maker's Zcash funding, once it is in a block."""
    outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
    if "funding_txid" not in corridor:
        found = zec_spend_of(outpoint, corridor["zebra_tip_at_provisioning"] + 1)
        if found is None:
            return None
        corridor["funding_txid"] = found[0]
    raw = zebra("getrawtransaction", [corridor["funding_txid"], 1])
    return int(raw.get("confirmations") or 0)


def zec_mining_policy(effect: dict | None, corridor: dict) -> None:
    """Mines at most one zebra block per poll, and only for a reason."""
    height = int(zebra("getblockcount"))
    pool = zebra("getrawmempool")
    if isinstance(pool, list) and pool:
        zebra_mine(1, f"mempool had {[t[:12] for t in pool]}")
        return
    # With both legs locked the Maker reveals the secret only once its Zcash
    # funding is two deep (zcash_claim_context.rs MIN_PRE_REVEAL); the Taker's
    # drive says nothing about that, so the depth is read from zebra itself.
    if effect and effect.get("phase") == "both_legs_locked":
        confirmations = zec_funding_confirmations(corridor)
        if confirmations is not None and confirmations < 2 and height + 1 < corridor["zcash_refund_height"]:
            zebra_mine(1, f"Maker's funding has {confirmations} confirmation(s); the pre-reveal gate wants 2")
            return
    if effect and effect.get("outcome") in {"awaiting_safe_zcash_funding", "awaiting_observation"} \
            and effect.get("operation") in {None, "zcash_fund", "maker_lock"} \
            and effect.get("phase") in {"awaiting_maker_confirmations", "taker_lock_confirmed", "both_legs_locked"}:
        if height + 1 < corridor["zcash_refund_height"]:
            zebra_mine(1, f"Taker waits for Zcash funding depth ({effect.get('outcome')})")
        else:
            log(f"  zebra at {height}: not mining, the refund height {corridor['zcash_refund_height']} is next")


def zec_drive_until(swap_id: str, config: str, states: set[str], timeout: int, corridor: dict,
                    describe: str = "", mine_pending_only: bool = False) -> dict:
    """Drives the Taker actor and polls both Nodes until the Taker's state is in `states`.
    With `mine_pending_only` a zebra block is generated only to include what waits
    in the mempool, never for depth: the Maker's funding then stays at one
    confirmation, below the pre-reveal gate, which is the refund journeys' setup."""
    deadline = time.time() + timeout
    last_state = last_effect = None
    while time.time() < deadline:
        effect = zec_drive(config)
        described = zec_describe(effect)
        view = taker_view(swap_id)
        state = view["state"]
        if state != last_state or described != last_effect:
            log(f"  taker {swap_id[:12]}: {state} (gen {view.get('progress_generation')}) | drive: {described} "
                f"| maker: {maker_phase(swap_id)} | zebra {zebra('getblockcount')}")
            last_state, last_effect = state, described
        if state in states:
            return view
        if state in TERMINAL:
            raise Failure(f"swap {swap_id[:12]} ended in {state} while waiting for {sorted(states)} {describe}")
        if mine_pending_only:
            pool = zebra("getrawmempool")
            if isinstance(pool, list) and pool:
                zebra_mine(1, f"mempool had {[t[:12] for t in pool]}")
        else:
            zec_mining_policy(effect, corridor)
        time.sleep(10)
    raise Failure(f"swap {swap_id[:12]} did not reach {sorted(states)} within {timeout}s (last {last_state}, "
                  f"drive {last_effect}) {describe}")


def scenario_zec_happy(stamp: str) -> dict:
    corridor = zec_provision()
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    zec_activate(config)
    outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
    # The Taker's LEZ lock, the Maker's Zcash funding and its revealing LEZ claim,
    # all observed by the driven Taker actor; the Node then offers the claim.
    view = zec_drive_until(swap_id, config, {"claim_available"}, timeout=2400, corridor=corridor,
                           describe="(Taker LEZ lock → Maker Zcash funding → Maker LEZ claim)")
    funding = zec_spend_of(outpoint, corridor["zebra_tip_at_provisioning"] + 1)
    if funding is None:
        raise Failure("claim_available, but no zebra block spends the provisioned coinbase outpoint")
    log(f"  Maker's Zcash funding {funding[0]} is in zebra block {funding[1]}")
    result = call("taker", "taker_swap_claim_v1", {"schema_version": 1, "request_id": f"e2e-claim-{stamp}",
                                                   "swap_id": swap_id, "expected_generation": view["progress_generation"]},
                  timeout=300)
    log(f"  Taker claimed Zcash (replay={result.get('was_replay')})")
    # The claim sits in zebra's mempool until a block is generated; the actor then
    # observes it on the next drive and the swap completes on both Nodes.
    zec_drive_until(swap_id, config, {"completed"}, timeout=900, corridor=corridor, describe="(claim inclusion)")
    claim_tx = zec_spend_of((funding[0], 0), funding[1])
    if claim_tx is None:
        raise Failure("the Taker reports completed, but no zebra block spends the funding output")
    log(f"  Taker's Zcash claim {claim_tx[0]} is in zebra block {claim_tx[1]}")
    wait_completed(swap_id, timeout=600)
    # The Maker's ZEC recovery store re-validates every stored claim journal
    # with the daemon's claim key on each start, so a Maker that completed a
    # ZEC swap must come back on the same key and still report it completed.
    log("  restarting the Maker Node after the completed swap (claim-journal revalidation)")
    docker("restart", NODES["maker"][0])
    wait_healthy("maker")
    view = maker_view(swap_id)
    if view.get("schedule_state") != "terminal" or maker_phase(swap_id) != "completed":
        raise Failure(f"after a restart the Maker reports {swap_id[:12]} as {maker_phase(swap_id)} "
                      f"({view.get('schedule_state')}), not completed/terminal")
    log("  Maker restarted and still reports the swap completed")
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "profile": corridor["profile"], "offer_id": corridor["offer_id"],
            "maker_restart_after_completion": "completed",
            "zcash_funding": {"transaction_id": funding[0], "height": funding[1]},
            "zcash_claim": {"transaction_id": claim_tx[0], "height": claim_tx[1]},
            "zcash_refund_height": corridor["zcash_refund_height"]}


def zec_drive_all_until(swaps: list[tuple[str, str, dict]], states: set[str], timeout: int, describe: str = "") -> dict[str, dict]:
    """Drives several Taker actors in turn (one drive each per round) until every
    swap's Node state is in `states`; the mining policy is applied per corridor."""
    deadline = time.time() + timeout
    done: dict[str, dict] = {}
    last: dict[str, tuple] = {}
    while time.time() < deadline:
        for swap_id, config, corridor in swaps:
            if swap_id in done:
                continue
            effect = zec_drive(config)
            described = zec_describe(effect)
            view = taker_view(swap_id)
            state = view["state"]
            if last.get(swap_id) != (state, described):
                log(f"  taker {swap_id[:12]}: {state} (gen {view.get('progress_generation')}) | drive: {described} "
                    f"| maker: {maker_phase(swap_id)} | zebra {zebra('getblockcount')}")
                last[swap_id] = (state, described)
            if state in states:
                done[swap_id] = view
            elif state in TERMINAL:
                raise Failure(f"swap {swap_id[:12]} ended in {state} while waiting for {sorted(states)} {describe}")
            else:
                zec_mining_policy(effect, corridor)
        if len(done) == len(swaps):
            return done
        time.sleep(10)
    pending = [s[0][:12] for s in swaps if s[0] not in done]
    raise Failure(f"swaps {pending} did not reach {sorted(states)} within {timeout}s {describe}")


def scenario_zec_concurrent(stamp: str) -> dict:
    """Two prepared entries of one corridor run, taken and settled side by side:
    two agreements, two Maker fundings from distinct coinbase outputs, two
    Taker claims; both Nodes list both swaps completed."""
    zec_provision(swaps=2)
    corridors = list(ZEC_CORRIDORS)
    if len(corridors) != 2:
        raise Failure(f"expected two prepared entries, the corridor has {len(corridors)}")
    swaps = []
    for corridor in corridors:
        swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
        config = zec_taker_actor_config(corridor["receipt_output"])
        zec_activate(config)
        swaps.append((swap_id, config, corridor))
    log(f"  both taken: {swaps[0][0][:12]} and {swaps[1][0][:12]}; driving both Taker actors in turn")
    views = zec_drive_all_until(swaps, {"claim_available"}, timeout=3000,
                                describe="(two Taker LEZ locks → two Maker Zcash fundings → two Maker LEZ claims)")
    fundings = {}
    for swap_id, _, corridor in swaps:
        outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
        funding = zec_spend_of(outpoint, corridor["zebra_tip_at_provisioning"] + 1)
        if funding is None:
            raise Failure(f"{swap_id[:12]} is claim_available, but no zebra block spends its provisioned coinbase outpoint")
        fundings[swap_id] = funding
        log(f"  Maker's Zcash funding for {swap_id[:12]}: {funding[0]} in zebra block {funding[1]}")
    if len({fundings[s[0]][0] for s in swaps}) != 2:
        raise Failure("the two Maker fundings are one transaction: the corridors shared a funder output")
    for index, (swap_id, _, _) in enumerate(swaps):
        result = call("taker", "taker_swap_claim_v1", {"schema_version": 1, "request_id": f"e2e-claim-{stamp}-{index}",
                                                       "swap_id": swap_id, "expected_generation": views[swap_id]["progress_generation"]},
                      timeout=300)
        log(f"  Taker claimed Zcash on {swap_id[:12]} (replay={result.get('was_replay')})")
    zec_drive_all_until(swaps, {"completed"}, timeout=1200, describe="(two claim inclusions)")
    claims = {}
    for swap_id, _, _ in swaps:
        funding = fundings[swap_id]
        claim_tx = zec_spend_of((funding[0], 0), funding[1])
        if claim_tx is None:
            raise Failure(f"{swap_id[:12]} reports completed, but no zebra block spends its funding output")
        claims[swap_id] = claim_tx
        log(f"  Taker's Zcash claim for {swap_id[:12]}: {claim_tx[0]} in zebra block {claim_tx[1]}")
        wait_completed(swap_id, timeout=600)
    return {"swaps": [{"swap_id": swap_id, "offer_id": corridor["offer_id"], "entry": corridor["entry"],
                       "zcash_funding": {"transaction_id": fundings[swap_id][0], "height": fundings[swap_id][1]},
                       "zcash_claim": {"transaction_id": claims[swap_id][0], "height": claims[swap_id][1]}}
                      for swap_id, _, corridor in swaps],
            "corridor": corridors[0]["run_id"], "profile": corridors[0]["profile"]}


def zec_export_tool(swap_id: str) -> bool:
    """`node-e2e.py --pair Zcash zec-export --swap-id …`: the ZEC counterpart of
    export-node-evidence.py for the desk journeys. A ZEC swap has no Bitcoin
    swap store to read; its evidence is the corridor (id, profile), the Taker
    actor's durable status, both Nodes' swap views, and the zebra transactions
    that spend the provisioned coinbase output (the Maker's funding) and the
    funding output itself (the Taker's claim or the Maker's refund), with their
    heights. Written to runtime/evidence/zec/<swap_id>.json."""
    corridor = zec_provision(swap_id=swap_id)
    config = zec_taker_actor_config(corridor["receipt_output"])
    outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
    funding = zec_spend_of(outpoint, corridor["zebra_tip_at_provisioning"] + 1)
    spend = zec_spend_of((funding[0], 0), funding[1]) if funding else None
    maker = rpc("maker", "maker_actor_monitor_v1", {"id": swap_id}, timeout=30)
    evidence = {
        "schema_version": 1, "swap_id": swap_id, "pair": "Zcash", "direction": ROUTE["direction"],
        "corridor": corridor["run_id"], "profile": corridor["profile"], "offer_id": corridor["offer_id"],
        "zcash_refund_height": corridor["zcash_refund_height"],
        "taker": {"node_view": taker_view(swap_id), "actor_status": zec_status(config)},
        "maker": {"node_view": maker.get("result"), "error": maker.get("error")},
        "zebra": {
            "tip_height": int(zebra("getblockcount")),
            "provisioned_outpoint": {"transaction_id": outpoint[0], "output_index": outpoint[1]},
            "maker_funding": {"transaction_id": funding[0], "height": funding[1]} if funding else None,
            "funding_output_spend": {"transaction_id": spend[0], "height": spend[1]} if spend else None,
        },
        "exported_at_unix_seconds": int(time.time()),
    }
    out = DEPLOY_ROOT / "runtime" / "evidence" / "zec"; out.mkdir(parents=True, exist_ok=True)
    path = out / f"{swap_id}.json"
    path.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n")
    log(f"  ZEC evidence written to {path} (taker {evidence['taker']['node_view'].get('state')}, "
        f"maker {((maker.get('result') or {}).get('progress') or {}).get('observation', {}).get('phase', '?')}, "
        f"funding {funding and funding[1]}, spend {spend and spend[1]})")
    return True


def zec_refund_tail_tool(swap_id: str) -> bool:
    """`node-e2e.py --pair Zcash zec-refund-tail --swap-id …`: the host side of a
    desk refund journey after the Taker desk admitted its LEZ refund. Mines
    zebra to the agreement's refund height, queues the Maker's refund (the
    operator action the Maker desk has no button for), and waits for the
    Maker's Zcash refund in a block; the Taker's own observation of that refund
    is the desk's "Retry refund", so it is left to the desk."""
    os.environ["LEZ_ZEC_REUSE_CORRIDOR"] = "1"
    corridor = zec_provision(swap_id=swap_id)
    config = zec_taker_actor_config(corridor["receipt_output"])
    outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
    funding = zec_spend_of(outpoint, corridor["zebra_tip_at_provisioning"] + 1)
    if funding is None:
        log("FAILED: no zebra block spends the provisioned outpoint (the Maker never funded)")
        return False
    stamp = corridor["run_id"].rsplit("-", 1)[1]
    try:
        result = zec_maker_refund_tail(swap_id, stamp, config, corridor, funding, taker_observes=False)
    except Failure as failure:
        log(f"FAILED: {failure}")
        return False
    log(f"  Maker refunded Zcash in block {result['zcash_refund']['height']} ({result['zcash_refund']['transaction_id'][:12]}…); Maker {result['maker_terminal_phase']}")
    return True


def zec_drive_many_tool(swap_ids: list[str], until: str, timeout: int) -> bool:
    """`zec-drive --swap-id a --swap-id b --until <state>`: the desk's concurrent
    journey drives both Taker actors in turn on the corridor the Nodes run with."""
    os.environ["LEZ_ZEC_REUSE_CORRIDOR"] = "1"
    swaps = []
    try:
        for swap_id in swap_ids:
            corridor = dict(zec_provision(swap_id=swap_id))
            config = zec_taker_actor_config(corridor["receipt_output"])
            status = zec_status(config)
            if not status or status.get("state") != "active":
                zec_activate(config)
            swaps.append((swap_id, config, corridor))
        views = zec_drive_all_until(swaps, set(until.split("|")), timeout, describe=f"(zec-drive to {until})")
    except Failure as failure:
        log(f"FAILED: {failure}")
        return False
    for swap_id, view in views.items():
        log(f"  taker {swap_id[:12]} is {view.get('state')}")
    return True


def zec_drive_tool(swap_id: str, until: str, timeout: int, mine_pending_only: bool = False,
                   until_phase: str | None = None) -> bool:
    """`node-e2e.py --pair Zcash zec-drive --swap-id … --until <state>`: the host-side
    actor driving the desk journeys need between their steps (ui-e2e.sh), on the
    corridor the Nodes currently run with. No preflight, no summary file."""
    os.environ["LEZ_ZEC_REUSE_CORRIDOR"] = "1"
    corridor = zec_provision(swap_id=swap_id)
    config = zec_taker_actor_config(corridor["receipt_output"])
    try:
        # The first drive after the desk's take activates the actor; later
        # drives (after the desk's claim) find it active and past the phase
        # where activation is admitted, so they only drive.
        status = zec_status(config)
        if not status or status.get("state") != "active":
            zec_activate(config)
        else:
            log(f"  Taker actor already active at {status.get('phase')} (revision {status.get('revision')})")
        if until_phase:
            # The Node reads refund_available from the Taker's confirmed LEZ lock
            # on; a refund journey has to stop at the actor's phase instead,
            # once the Maker's Zcash funding is in (both legs locked).
            view = zec_drive_until_phase(swap_id, config, set(until_phase.split("|")), timeout, corridor,
                                         mine_pending_only=mine_pending_only, describe=f"(zec-drive to phase {until_phase})")
        else:
            view = zec_drive_until(swap_id, config, set(until.split("|")), timeout, corridor, describe=f"(zec-drive to {until})",
                                   mine_pending_only=mine_pending_only)
    except Failure as failure:
        log(f"FAILED: {failure}")
        return False
    # A state drive returns the Taker's view, a phase drive the actor's last effect.
    log(f"  taker {swap_id[:12]} is {view.get('state') or view.get('phase')}")
    return True


def zec_drive_until_phase(swap_id: str, config: str, phases: set[str], timeout: int, corridor: dict,
                          mine_pending_only: bool, describe: str = "") -> dict:
    """Drives the Taker actor until its own phase (from the drive output) is in
    `phases`. With `mine_pending_only` a zebra block is generated only to
    include what waits in the mempool, never for depth: that keeps the Maker's
    funding at one confirmation, below the pre-reveal gate."""
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        effect = zec_drive(config)
        described = zec_describe(effect)
        state = taker_view(swap_id)["state"]
        if described != last:
            log(f"  taker {swap_id[:12]}: {state} | drive: {described} | maker: {maker_phase(swap_id)} "
                f"| zebra {zebra('getblockcount')}")
            last = described
        if effect and effect.get("phase") in phases:
            return effect
        if state in TERMINAL:
            raise Failure(f"swap {swap_id[:12]} ended in {state} while driving to {sorted(phases)} {describe}")
        if mine_pending_only:
            pool = zebra("getrawmempool")
            if isinstance(pool, list) and pool:
                zebra_mine(1, f"mempool had {[t[:12] for t in pool]}")
        else:
            zec_mining_policy(effect, corridor)
        time.sleep(10)
    raise Failure(f"the Taker actor did not reach {sorted(phases)} within {timeout}s (last {last}) {describe}")


# (swap id, request id) -> the generation the Taker admitted that refund request at.
ZEC_REFUND_REQUESTS: dict[tuple[str, str], int] = {}


def zec_refund_until_terminal(swap_id: str, stamp: str, timeout: int,
                              until: frozenset[str] = frozenset({"refunded"})) -> dict:
    """The Taker's LEZ refund as an operator action, repeated with one request id:
    the Node answers taker_action_execution_unavailable until the actor's recovery
    checks pass (lock confirmed, maker lock proven absent or the deadline gone),
    admits it once they do, and a replay re-drives the admitted recovery, which
    is also how the Taker observes its refund finalising (the Node never drives a
    ZEC actor on its own). Returns the Taker's view once its state is in `until`;
    attention_required and completed fail. With both legs locked the Node reads
    `refund_in_progress` from the admission until the Taker has observed the
    Maker's Zcash refund, then `refunded`; a caller that has to act in between
    (mine to the refund height, queue the Maker's refund) asks for either and
    comes back for `refunded`."""
    deadline = time.time() + timeout
    attempt = 0
    request_id = f"e2e-refund-{stamp}"
    last_state = None
    while time.time() < deadline:
        view = taker_view(swap_id)
        if view["state"] != last_state:
            log(f"  taker {swap_id[:12]}: {view['state']} (generation {view.get('progress_generation')})")
            last_state = view["state"]
        if view["state"] in until:
            return view
        if view["state"] in {"attention_required", "completed"}:
            raise Failure(f"swap {swap_id[:12]} ended in {view['state']} instead of {'/'.join(sorted(until))}")
        attempt += 1
        # A replay carries the generation the request was admitted at (as the
        # desk does): the Node matches the durable record on it, and admits a
        # refund replay through the leg-refunded phases even after the actor's
        # revision moved on. A fresh generation would be a different request.
        generation = ZEC_REFUND_REQUESTS.get((swap_id, request_id), view["progress_generation"])
        reply = rpc("taker", "taker_swap_refund_v1", {"schema_version": 1, "request_id": request_id,
                                                     "swap_id": swap_id, "expected_generation": generation},
                    timeout=300)
        if "result" in reply:
            ZEC_REFUND_REQUESTS.setdefault((swap_id, request_id), generation)
            if not reply["result"].get("was_replay") or attempt % 4 == 1:
                log(f"  refund request {'replayed' if reply['result'].get('was_replay') else 'admitted'} "
                    f"(attempt {attempt}, taker {view['state']} | maker: {maker_phase(swap_id)})")
        else:
            category = (reply["error"].get("data") or {}).get("category")
            if category not in {"taker_action_execution_unavailable", "taker_action_conflict", "taker_action_unavailable"}:
                raise Failure(f"taker_swap_refund_v1 failed: {json.dumps(reply['error'])[:300]}")
            if attempt % 4 == 1:
                log(f"  refund not yet accepted (attempt {attempt}: {category}); taker {view['state']} | maker: {maker_phase(swap_id)}")
        time.sleep(15)
    raise Failure(f"swap {swap_id[:12]} was not {'/'.join(sorted(until))} within {timeout}s")


def wait_maker_terminal(swap_id: str, timeout: int, describe: str) -> str:
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        view = maker_view(swap_id)
        phase = maker_phase(swap_id)
        if phase != last:
            log(f"  maker {swap_id[:12]}: {phase} (schedule {view.get('schedule_state')})")
            last = phase
        if view.get("schedule_state") == "terminal":
            return phase
        time.sleep(15)
    raise Failure(f"the Maker did not reach a terminal state within {timeout}s {describe} (last {last})")


def zec_outpoint_unspent(outpoint: tuple[str, int]) -> bool:
    return zebra("gettxout", [outpoint[0], outpoint[1]]) is not None


def zec_cut_maker_zebra_route() -> None:
    """The Maker Node reaches zebra through the socat forwarder its entrypoint
    starts (127.0.0.1:18232 -> zebra). Killing it leaves the Maker able to
    observe the Taker's LEZ lock through its sidecar but unable to plan or
    submit its Zcash funding (connection refused); the next container start
    brings the forwarder back."""
    docker("exec", NODES["maker"][0], "pkill", "-f", "TCP:[^ ]*:18232")


def wait_maker_phase(swap_id: str, phases: set[str], timeout: int, describe: str) -> str:
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        view = maker_view(swap_id)
        phase = maker_phase(swap_id)
        if phase != last:
            log(f"  maker {swap_id[:12]}: {phase} (schedule {view.get('schedule_state')})")
            last = phase
        if phase in phases:
            return phase
        time.sleep(15)
    raise Failure(f"the Maker did not reach {sorted(phases)} within {timeout}s {describe} (last {last})")


def scenario_zec_taker_refund(stamp: str) -> dict:
    """The Taker locks LEZ, the Maker never funds Zcash, the Taker refunds at
    the agreement's LEZ refund time (30 min after provisioning on the devnet
    profile) and the Maker reconciles the refunded leg once it is back.

    The Maker has to observe the Taker's lock before it goes down: the SDK's
    agreement-ordered recovery starts from an observed first leg
    (`next_refund_step` has no Offered arm, the actor's recovery gate agrees),
    so a Maker that only ever saw the offer has nothing to reconcile and stays
    at `offered`. Its zebra route is therefore cut instead of stopping it at
    once: it observes the LEZ lock through the sidecar, cannot plan the Zcash
    funding, and is stopped at `taker_lock_confirmed`."""
    corridor = zec_provision()
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    zec_activate(config)
    outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
    log("  cutting the Maker Node's zebra route: it will observe the Taker's lock but cannot fund Zcash")
    zec_cut_maker_zebra_route()
    try:
        zec_drive_until_phase(swap_id, config, {"taker_lock_confirmed", "awaiting_maker_confirmations", "both_legs_locked"},
                              timeout=1500, corridor=corridor, mine_pending_only=True,
                              describe="(Taker LEZ lock confirmed, Maker without zebra)")
        wait_maker_phase(swap_id, {"taker_lock_confirmed", "awaiting_maker_confirmations", "both_legs_locked"},
                         timeout=900, describe="(Maker observing the Taker's LEZ lock without zebra)")
        if zec_outpoint_unspent(outpoint) is not True:
            raise Failure("the provisioned coinbase outpoint was spent although the Maker's zebra route was cut")
        log("  the Maker observed the Taker's LEZ lock; stopping the Maker Node so it never funds Zcash")
        docker("stop", NODES["maker"][0])
        # The LEZ escrow refunds only once the chain clock passes the agreement's
        # refund time: provisioning time + the profile's delay (30 min on the
        # devnet profile). The provisioning summary does not carry it, so it is
        # taken from the corridor's run id stamp, which the script sets to the
        # provisioning second.
        opens = int(corridor["run_id"].rsplit("-", 1)[1]) + corridor["lez_refund_delay_seconds"]
        wait_until(opens + 15, "for the agreement's LEZ refund time (devnet profile: 30 min after provisioning)")
        log("  the Taker's LEZ lock is confirmed, the Maker is down and the LEZ refund time has passed")
        zec_refund_until_terminal(swap_id, stamp, timeout=1800)
        log("  Taker refunded its LEZ escrow")
    finally:
        docker("start", NODES["maker"][0])
        wait_healthy("maker")
    # Back up, the Maker's own drive re-observes the Taker's lock and finds the
    # escrow refunded, which its funding path refuses (no Zcash is ever sent).
    # The operator's refund action runs the agreement-ordered recovery: the
    # non-owner observes the Taker's LEZ refund and ends Refunded, as the
    # SDK's one-leg recovery test does for both roles.
    if maker_view(swap_id).get("schedule_state") != "terminal":
        result = maker_action_with_fresh_generation("maker_actor_refund_v1", swap_id, f"e2e-zec-taker-refund-{stamp}")
        log(f"  Maker refund queued: {json.dumps(result)[:160]}")
    phase = wait_maker_terminal(swap_id, timeout=1200, describe="(reconciling the refunded LEZ leg)")
    if zec_outpoint_unspent(outpoint) is not True:
        raise Failure("the provisioned coinbase outpoint was spent: the Maker funded Zcash after all")
    log(f"  zebra: the provisioned coinbase outpoint {outpoint[0][:12]}:{outpoint[1]} is still unspent; Maker ended {phase}")
    if phase not in {"refunded", "taker_leg_refunded"}:
        raise Failure(f"the Maker ended in {phase}, not a refunded phase")
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "profile": corridor["profile"],
            "maker_terminal_phase": phase, "zcash_funding_outpoint_unspent": True}


def scenario_zec_maker_refund(stamp: str) -> dict:
    """Both legs locked, the Maker's Zcash funding at one confirmation (below
    the pre-reveal gate), the Taker refunds LEZ at the agreement's refund time,
    zebra is mined to the agreement's refund height and the Maker refunds its
    Zcash; the Taker observes that refund and both Nodes end `refunded`."""
    corridor = zec_provision()
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    zec_activate(config)
    outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
    # Both legs locked, the Maker's funding at one confirmation: below the
    # pre-reveal gate (two), so the Maker cannot claim LEZ yet.
    zec_drive_until_phase(swap_id, config, {"both_legs_locked", "awaiting_maker_confirmations"}, timeout=2400,
                          corridor=corridor, mine_pending_only=True, describe="(Taker LEZ lock → Maker Zcash funding)")
    funding = zec_spend_of(outpoint, corridor["zebra_tip_at_provisioning"] + 1)
    if funding is None:
        raise Failure("the Taker observed the Maker's lock, but no zebra block spends the provisioned outpoint")
    log(f"  Maker's Zcash funding {funding[0]} is in zebra block {funding[1]}; the Taker refunds LEZ instead of waiting")
    opens = int(corridor["run_id"].rsplit("-", 1)[1]) + corridor["lez_refund_delay_seconds"]
    wait_until(opens + 15, "for the agreement's LEZ refund time (devnet profile: 30 min after provisioning)")
    zec_refund_until_terminal(swap_id, stamp, timeout=1800, until=frozenset({"refund_in_progress", "refunded"}))
    log("  the Taker's LEZ refund is admitted and in progress on the Node; the Maker's Zcash refund is owed next")
    return zec_maker_refund_tail(swap_id, stamp, config, corridor, funding)


def zec_maker_refund_tail(swap_id: str, stamp: str, config: str, corridor: dict, funding: tuple[str, int],
                          taker_observes: bool = True) -> dict:
    """From the Taker's refunded LEZ leg to both Nodes `refunded`: zebra is mined
    to the agreement's refund height, the Maker (which re-observes the LEZ leg
    on its next attempt) refunds its Zcash funding, and the Taker's replayed
    refund request re-drives its recovery until it has observed that refund."""
    log("  mining zebra to the agreement's refund height")
    height = int(zebra("getblockcount"))
    if height < corridor["zcash_refund_height"]:
        zebra_mine(corridor["zcash_refund_height"] - height, "Zcash refund height")
    # The Maker's supervisor keeps retrying the claim it was routed to at
    # both_legs_locked (now refused with RefundHeightReached); recovery is the
    # operator's call, as on the desk. The queued refund runs the agreement-
    # ordered recovery: the Maker observes the Taker's LEZ refund, then
    # refunds its own Zcash leg at the refund height.
    if maker_view(swap_id).get("schedule_state") != "terminal":
        try:
            result = maker_action_with_fresh_generation("maker_actor_refund_v1", swap_id, f"e2e-zec-maker-refund-{stamp}")
            log(f"  Maker refund queued: {json.dumps(result)[:160]}")
        except Failure as error:
            # A resumed run finds its own earlier request already recorded
            # under this id; the Maker keeps that record and refuses a second
            # one at another generation. The recovery it asked for still runs.
            if "conflicts with durable state" not in str(error):
                raise
            log("  Maker refund request already recorded for this run; continuing")
    deadline = time.time() + 1500
    refund_tx = None
    last = None
    while time.time() < deadline:
        pool = zebra("getrawmempool")
        if isinstance(pool, list) and pool:
            zebra_mine(1, f"mempool had {[t[:12] for t in pool]}")
        refund_tx = zec_spend_of((funding[0], 0), funding[1])
        view = maker_view(swap_id)
        phase = maker_phase(swap_id)
        if refund_tx and view.get("schedule_state") == "terminal":
            break
        if (phase, view.get("schedule_state")) != last:
            log(f"  maker {swap_id[:12]}: {phase} (schedule {view.get('schedule_state')}) | zebra {zebra('getblockcount')}")
            last = (phase, view.get("schedule_state"))
        time.sleep(20)
    if refund_tx is None:
        raise Failure("no zebra block spends the Maker's funding output: the Maker did not refund Zcash")
    phase = maker_phase(swap_id)
    log(f"  Maker's Zcash refund {refund_tx[0]} is in zebra block {refund_tx[1]}; Maker ended {phase}")
    if phase not in {"refunded", "maker_leg_refunded", "taker_leg_refunded"}:
        raise Failure(f"the Maker ended in {phase}, not a refunded phase")
    summary = {"swap_id": swap_id, "corridor": corridor["run_id"], "profile": corridor["profile"],
               "zcash_funding": {"transaction_id": funding[0], "height": funding[1]},
               "zcash_refund": {"transaction_id": refund_tx[0], "height": refund_tx[1]},
               "zcash_refund_height": corridor["zcash_refund_height"], "maker_terminal_phase": phase}
    if not taker_observes:
        return summary
    taker = zec_refund_until_terminal(swap_id, stamp, timeout=900)
    log(f"  Taker observed the Maker's Zcash refund; Taker ended {taker['state']}")
    return {**summary, "taker_terminal_state": taker["state"]}


ZEC_SCENARIOS = {
    "zec-happy": scenario_zec_happy,
    "zec-taker-refund": scenario_zec_taker_refund,
    "zec-maker-refund": scenario_zec_maker_refund,
    "zec-concurrent": scenario_zec_concurrent,
}


SCENARIOS = {
    "happy": scenario_happy,
    "replay": scenario_replay,
    "wrong-inputs": scenario_wrong_inputs,
    "restart-taker": lambda stamp: scenario_restart(stamp, "taker"),
    "restart-maker": lambda stamp: scenario_restart(stamp, "maker"),
    "survivor": scenario_survivor,
    "concurrent": scenario_concurrent,
    "early-lock": scenario_early_lock,
    "offline-after-lock": scenario_offline_after_lock,
    "taker-refund": scenario_taker_refund,
    "maker-refund": scenario_maker_refund,
    "refund-then-maker-lock": scenario_refund_admitted_then_maker_lock,
    "regenerated-config": scenario_regenerated_config,
    "tampered-config": scenario_tampered_config,
}


def preflight() -> None:
    """The standing LEZ indexer stops serving historical reads once its state
    breakpoints lag (the Maker's sidecar then reports `unavailable` and never
    funds); repair-indexer.sh checks and re-indexes when needed."""
    log("preflight: indexer historical reads")
    # The repair restarts the indexer when its historical reads fail; a run that
    # shares the stack with another suite must only check (and fail honestly).
    check_only = ["--check"] if os.environ.get("LEZ_E2E_INDEXER_CHECK_ONLY") == "1" else []
    result = subprocess.run(["bash", str(DEPLOY_ROOT / "scripts" / "repair-indexer.sh"), *check_only],
                            capture_output=True, text=True, check=False, cwd=DEPLOY_ROOT, stdin=subprocess.DEVNULL)
    if result.returncode != 0:
        raise Failure(f"repair-indexer.sh failed: {result.stdout[-300:]} {result.stderr[-300:]}")
    for line in result.stdout.strip().splitlines()[-2:]:
        log(f"  {line}")
    # LEZ finality must be moving: bedrock has frozen after a restart (its
    # wallet never came back), leaving the sequencer producing blocks nobody
    # finalizes. Every sidecar observation then sees the same clock forever.
    # Finality arrives from bedrock in bursts: a day-old bedrock pauses its
    # finality for minutes and then catches up in one step, so a short sample
    # can show no progress on a chain the swaps still complete on; a frozen
    # chain shows none at all.
    # The burst behaviour above means the sample window is a judgement call, so
    # it is configurable: a chain whose bedrock pauses for longer than the
    # default needs a longer look before it is called frozen.
    # On a chain whose finality only moves while swaps are running, this check
    # gates the very traffic that would satisfy it. The comment above already
    # notes swaps complete on chains that sample as frozen, so the check can be
    # waived deliberately; the scenario then proves the chain for itself.
    if os.environ.get("LEZ_E2E_SKIP_FINALITY_PREFLIGHT") == "1":
        log("preflight: LEZ finality check waived by request")
        return
    window = int(os.environ.get("LEZ_E2E_FINALITY_WINDOW_SECONDS", "300"))
    log(f"preflight: LEZ finality advancing (window {window} s)")
    first = lez_finalized_height()
    started = time.time()
    while time.time() < started + window:
        time.sleep(10)
        second = lez_finalized_height()
        if second > first:
            log(f"  indexer finalized {first} → {second} in {round(time.time() - started)} s")
            return
    raise Failure(f"LEZ finalized height stuck at {first} for {window} s: bedrock or the indexer is not advancing; "
                  "recreate the LEZ chain (see project notes) before running swaps")


def lez_finalized_height() -> int:
    """The indexer's last finalized L2 block, read through the Taker's loopback forwarder."""
    container, _ = NODES["taker"]
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getLastFinalizedBlockId", "params": []})
    result = subprocess.run(["docker", "exec", container, "curl", "-sf", "-m", "10",
                             "-H", "content-type: application/json", "--data", body, "http://127.0.0.1:8779/"],
                            capture_output=True, text=True, check=False)
    try:
        return int(json.loads(result.stdout)["result"])
    except (ValueError, KeyError, TypeError) as error:
        raise Failure(f"indexer height unreadable: {result.stdout[:120]} {result.stderr[:120]}") from error


def run(name: str) -> bool:
    stamp = str(int(time.time()))
    log(f"=== {name} (stamp {stamp})")
    started = time.time()
    summary = {"scenario": name, "pair": ROUTE["pair"], "direction": ROUTE["direction"], "stamp": stamp,
               "timing": timing_profile()}
    try:
        for role in ("maker", "taker"):
            wait_healthy(role, timeout=60)
        preflight()
        summary.update({**SCENARIOS, **ZEC_SCENARIOS}[name](stamp))
        summary["result"] = "passed"
    except Failure as failure:
        summary["result"] = "failed"
        summary["error"] = str(failure)
        log(f"FAILED: {failure}")
    summary["seconds"] = round(time.time() - started)
    out = RUNTIME / "e2e"
    out.mkdir(parents=True, exist_ok=True)
    suffix = "" if ROUTE["pair"] == "Zcash" else ("-lez" if REVERSE else "")
    (out / f"{name}{suffix}.json").write_text(json.dumps(summary, indent=2) + "\n")
    log(f"=== {name}: {summary['result']} in {summary['seconds']}s")
    return summary["result"] == "passed"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("scenario", choices=[*SCENARIOS, *ZEC_SCENARIOS, "zec-drive", "zec-export", "zec-refund-tail", "all"])
    parser.add_argument("--swap-id", action="append", default=[],
                        help="zec-drive / zec-export / zec-refund-tail: the swap to act on (zec-drive --until takes several)")
    parser.add_argument("--mine-pending-only", action="store_true",
                        help="zec-drive: mine zebra only for what waits in the mempool, never for depth (refund journeys)")
    parser.add_argument("--until", help="zec-drive: Taker state(s) to reach, '|'-separated")
    parser.add_argument("--until-phase", help="zec-drive: Taker actor phase(s) to reach instead, '|'-separated")
    parser.add_argument("--timeout", type=int, default=2400, help="zec-drive: seconds")
    parser.add_argument("--direction", choices=["TakerSellsForeign", "TakerSellsLez"], default="TakerSellsForeign",
                        help="which side the Taker sells; TakerSellsLez locks LEZ first and claims Bitcoin")
    parser.add_argument("--pair", choices=["Bitcoin", "Zcash"], default="Bitcoin",
                        help="Zcash runs the zec-* scenarios (TakerSellsLez only, prepared corridor)")
    args = parser.parse_args()
    global REVERSE, FOREIGN_UNITS, LEZ_UNITS
    if args.pair == "Zcash":
        ROUTE.update({"pair": "Zcash", "direction": "TakerSellsLez"})
        REVERSE = True
        FOREIGN_UNITS, LEZ_UNITS = ZEC_FOREIGN_UNITS, ZEC_LEZ_UNITS
        if args.scenario == "zec-drive" and len(args.swap_id) > 1:
            if not args.until or args.until_phase or args.mine_pending_only:
                parser.error("zec-drive with several --swap-id takes --until only")
            return 0 if zec_drive_many_tool(args.swap_id, args.until, args.timeout) else 1
        args.swap_id = args.swap_id[0] if args.swap_id else None
        if args.scenario == "zec-drive":
            if not (args.swap_id and (args.until or args.until_phase)):
                parser.error("zec-drive needs --swap-id and --until or --until-phase")
            return 0 if zec_drive_tool(args.swap_id, args.until or "", args.timeout, args.mine_pending_only, args.until_phase) else 1
        if args.scenario == "zec-refund-tail":
            if not args.swap_id:
                parser.error("zec-refund-tail needs --swap-id")
            return 0 if zec_refund_tail_tool(args.swap_id) else 1
        if args.scenario == "zec-export":
            if not args.swap_id:
                parser.error("zec-export needs --swap-id")
            return 0 if zec_export_tool(args.swap_id) else 1
        names = list(ZEC_SCENARIOS) if args.scenario == "all" else [args.scenario]
        unknown = [n for n in names if n not in ZEC_SCENARIOS]
        if unknown:
            parser.error(f"{unknown[0]} is a Bitcoin scenario; with --pair Zcash choose from {sorted(ZEC_SCENARIOS)}")
    else:
        ROUTE["direction"] = args.direction
        REVERSE = args.direction == "TakerSellsLez"
        names = list(SCENARIOS) if args.scenario == "all" else [args.scenario]
        if any(n in ZEC_SCENARIOS for n in names):
            parser.error("zec-* scenarios need --pair Zcash")
    for name in names:
        if not run(name):
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
