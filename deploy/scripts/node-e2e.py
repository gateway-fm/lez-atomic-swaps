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
    node-e2e.py kill-taker       restart-taker with SIGKILL of the Taker's container instead (R4)
    node-e2e.py kill-maker       restart-maker with SIGKILL of the Maker's container instead (R4)
    node-e2e.py survivor         Taker Node stopped after its LEZ claim; Maker completes alone
    node-e2e.py late-claim-confirms  the revealing claim sent, the Taker stopped, the claim confirms while it is down
    node-e2e.py late-claim-unmined   the revealing claim sent but never mined until the refund path is open
    node-e2e.py maker-cli        the happy swap with every Maker call made by lez-maker-cli; a non-owner is refused (U3)
    node-e2e.py maker-price-module  a route priced by the TEST Logos price module: quote, publish, a swap; then fail-closed (F8)
    node-e2e.py concurrent       two swaps interleaved end to end
    node-e2e.py early-lock       the Taker's lock is held unconfirmed; the Maker must not lock (R1)
    node-e2e.py offline-after-lock  Chat and Delivery cut after the lock; the swap finishes on chain state alone (R2)
    node-e2e.py outage-before-take  Chat and Delivery cut before the take: refused, no swap; the same take succeeds once back (R8)
    node-e2e.py zebra-down       the lane's Zebra stopped: a Bitcoin swap completes, then a Zcash one once it is back (R3)
    node-e2e.py taker-refund     Maker stopped before funding; Taker refunds its Bitcoin
    node-e2e.py maker-refund     Taker never claims; Maker refunds LEZ, then Taker refunds Bitcoin
    node-e2e.py refund-then-maker-lock  Refund admitted, then a late-but-timely maker lock; the observer must still recover
    node-e2e.py regenerated-config  live btc-role.json rendered again, Taker Node restarted: the swap reloads and completes
    node-e2e.py tampered-config     the swap's copied taker-role-config.json altered: the swap is refused after a restart
    node-e2e.py all              every scenario in sequence (stops at the first failure)

With `--pair Zcash` the harness runs the ZEC↔LEZ scenarios instead, in either
direction (`--direction TakerSellsLez`, the default for Zcash: the Taker locks
LEZ, the Maker funds Zcash; `--direction TakerSellsForeign`: the Taker funds
the Zcash HTLC, the Maker locks LEZ, the Taker's LEZ claim reveals the secret
and the Maker's Zcash claim completes the swap). A prepared corridor is
provisioned with scripts/zec-provision.sh --direction …, which also recreates
both Nodes, then

    node-e2e.py --pair Zcash zec-happy   take, the Taker Node drives its actor (activation,
                                         LEZ lock, observation), Maker funds Zcash, Maker
                                         claims LEZ, Taker claims Zcash; claim found in a
                                         zebra block. The harness only polls and mines zebra;
                                         ZEC_MANUAL_DRIVE=1 adds a hand drive per poll (for
                                         diagnosis, never the evidence)
    node-e2e.py --pair Zcash zec-taker-refund  Maker stopped before it funds; the Taker
                                         refunds its LEZ escrow; the Maker reconciles
    node-e2e.py --pair Zcash zec-maker-refund  both legs locked, the Taker refunds LEZ
                                         instead of waiting; the Maker refunds Zcash at
                                         the agreement's height; refund found in a block
    node-e2e.py --pair Zcash zec-balance-refusal  the Taker's LEZ account cannot fund the
                                         lot: the first drive is refused by name, the Node
                                         shows attention_required with the reason, the
                                         account is funded (the deferred vault claim), the
                                         next drive locks and the swap completes. Needs a
                                         chain created with LEZ_BOOTSTRAP_DEFER_CLAIM=taker-zurich-01

The ZEC refund windows come from the agreement, not the timing profile. The
refund journeys provision their corridor under deterministic-local-devnet-v6
(zec-provision.sh --profile), every other ZEC scenario under -v5 (both NU7, the
devnet Zebra 7's branch; -v3/-v4 are their NU6.2 namesakes): the Taker's
LEZ refund opens the profile's LEZ refund delay (4 min under -v6, 12 min under
-v5) after the corridor was provisioned and the Maker's Zcash refund at an
absolute zebra height (tip at provisioning + 9); the refund scenarios read
those from the corridor and wait for them rather than guessing, so neither
needs a BTC timing profile.

Every scenario runs in either Bitcoin direction: `--direction TakerSellsForeign`
(the default; the Taker locks Bitcoin, the Maker LEZ) or `--direction
TakerSellsLez` (the Taker locks LEZ first, the Maker Bitcoin, the Taker claims
Bitcoin). The summary of a reverse run is `runtime/e2e/<scenario>-lez.json`;
a Zcash run's is `runtime/e2e/<scenario>.json` (TakerSellsLez) or
`runtime/e2e/<scenario>-foreign.json` (TakerSellsForeign).

The BTC refund scenarios need the `refund` (or the `fast`) timing profile
(`LEZ_TIMING_PROFILE=refund scripts/up.sh`, which recreates the Nodes and the
miner; regress.sh switches by itself); the harness refuses to start them under
the `local` profile.
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

DEPLOY_ROOT = pathlib.Path(__file__).resolve().parent.parent
RUNTIME = DEPLOY_ROOT / "runtime"
# A regression lane (scripts/lane.sh) is one Maker/Taker pair with its own
# Bitcoin Core, miner and Zebra on the shared LEZ network. Unset, these name
# lane 1, the stack itself: lez-* containers, runtime/, zebra on :18232.
if os.environ.get("LEZ_LANE", "1") != "1" and "LEZ_LANE_RUNTIME" not in os.environ:
    # Run by hand with only LEZ_LANE set: take the lane's names from lane.sh.
    names = subprocess.run(["bash", "-c", "set -a; source runtime/runtime.env; set +a; source scripts/lane.sh; env -0"],
                           cwd=DEPLOY_ROOT, capture_output=True, check=True).stdout
    for pair in names.split(b"\0"):
        key, _, value = pair.decode().partition("=")
        if key.startswith("LEZ_"):
            os.environ[key] = value
elif "LEZ_CONTAINER_PREFIX" not in os.environ:
    # Lane 1 without its names (by hand, or regress.sh's one-lane run, which
    # exports runtime.env but not lane.sh): the stack's names and ports from
    # lane.sh, another stack than the default one when this deploy directory's
    # runtime.env names it.
    names = subprocess.run(["bash", "-c", "source scripts/lane.sh && env -0"],
                           cwd=DEPLOY_ROOT, capture_output=True, check=True).stdout
    for pair in names.split(b"\0"):
        key, _, value = pair.decode().partition("=")
        if key.startswith("LEZ_STACK_") or key in ("LEZ_CONTAINER_PREFIX", "LEZ_INDEXER_PORT", "LEZ_SEQUENCER_PORT",
                                                   "LEZ_ZEBRA_URL"):
            os.environ.setdefault(key, value)
PREFIX = os.environ.get("LEZ_CONTAINER_PREFIX", "lez")
# The stack's indexer and sequencer on the host (scripts/lane.sh: its port offset).
STACK_INDEXER_URL = os.environ.get("LEZ_STACK_INDEXER_URL", "http://127.0.0.1:8779/")
STACK_SEQUENCER_URL = f"http://127.0.0.1:{os.environ.get('LEZ_SEQUENCER_PORT', '3040')}/"
# The lane's own files: role identities and keys (lez/), corridors (zec/),
# summaries (e2e/) and exported evidence (evidence/). runtime.env and the
# heartbeat's status stay in RUNTIME.
LANE_RUNTIME = DEPLOY_ROOT / os.environ.get("LEZ_LANE_RUNTIME", "runtime")
ROUTE = {"pair": "Bitcoin", "direction": "TakerSellsForeign"}  # --direction / --pair replace it
REVERSE = False  # True when the Taker sells LEZ (locks LEZ first, claims the foreign coin)
FOREIGN_UNITS = 1_000_000
LEZ_UNITS = 1_000
# The ZEC corridor's template (zec-reference-actor local_poc.rs): 1 ZEC for 50 000 LEZ units.
ZEC_FOREIGN_UNITS = 100_000_000
ZEC_LEZ_UNITS = 50_000
ZEBRA_URL = os.environ.get("LEZ_ZEBRA_URL", "http://127.0.0.1:18232/")  # Compose publishes zebra's RPC on the host loopback
# The public Zcash Testnet lane (scripts/public-zec-testnet.sh): zebra is a
# Testnet node or provider, so nothing is ever mined; a wait for blocks waits
# for the chain's own (at most LEZ_ZEC_BLOCK_WAIT_SECONDS each), and every ZEC
# wait is LEZ_ZEC_TIMEOUT_SCALE times its devnet length. The route's credential
# is a file (Zebra's .cookie, or a provider key sent as x-api-key), read per call.
ZEC_PUBLIC = os.environ.get("LEZ_ZEC_PUBLIC_TESTNET") == "1"
ZEC_BLOCK_WAIT_SECONDS = int(os.environ.get("LEZ_ZEC_BLOCK_WAIT_SECONDS", "900"))
ZEC_TIMEOUT_SCALE = int(os.environ.get("LEZ_ZEC_TIMEOUT_SCALE", "1")) if ZEC_PUBLIC else 1
NODES = {"maker": (f"{PREFIX}-maker-node", "/run/lez/maker/node.sock"),
         "taker": (f"{PREFIX}-taker-node", "/run/lez/taker/node.sock")}
MINER = f"{PREFIX}-btc-miner"
# The market wallets the lane's Nodes settle as (zec-balance-refusal names them).
MAKER_WALLET = os.environ.get("LEZ_LANE_MAKER_WALLET", "maker-munich-01")
TAKER_WALLET = os.environ.get("LEZ_LANE_TAKER_WALLET", "taker-zurich-01")
BTC_CLI = ["docker", "exec", f"{PREFIX}-bitcoin-core", "bitcoin-cli",
           "-conf=/run-config/bitcoin.conf", "-datadir=/var/lib/bitcoin"]
MINER_ADDRESS = "bcrt1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqc8gma6"
TERMINAL = {"completed", "refunded", "attention_required"}


class Failure(Exception):
    """A scenario assertion failed."""


class NotExercised(Failure):
    """The scenario could not produce the situation it tests (a race it
    provokes went the other way); nothing was found wrong. The process exits
    with NOT_EXERCISED_EXIT so a runner can try the scenario again."""


NOT_EXERCISED_EXIT = 98
NOT_EXERCISED: list[str] = []


def log(message: str) -> None:
    print(f"[{time.strftime('%H:%M:%S')}] {message}", flush=True)


def rpc(role: str, method: str, params: object, timeout: int = 300) -> dict:
    """One owner-RPC call; returns the full JSON-RPC envelope."""
    if role == "taker" and TAKER_CLI:
        return taker_cli_rpc(method, params)
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
        if line.startswith(("LEZ_TIMING_PROFILE=", "LEZ_BTC_", "LEZ_LEZ_", "MINE_INTERVAL=", "LEZ_MAKER_ACTOR_",
                            "LEZ_TAKER_OBSERVER_")):
            key, _, value = line.partition("=")
            values[key] = value
    return values


def profile_seconds(profile: dict, key: str, default: int) -> int:
    """A profile's number of seconds; an empty value is the Node's default."""
    value = profile.get(key, "")
    return int(value) if value.strip().isdigit() else default


def maker_attention_seconds(profile: dict) -> int:
    """How long the Maker must be watched for it to have looked several times
    over: five rounds of its actor's requeue plus failure backoff (gen-config.sh:
    2 + 10 s on the devnet's profiles; the Node's 5 + 30 s on `local`), and never
    less than the 90 s the scenarios have always held."""
    rounds = (profile_seconds(profile, "LEZ_MAKER_ACTOR_REQUEUE_SECONDS", 5)
              + profile_seconds(profile, "LEZ_MAKER_ACTOR_FAILURE_BACKOFF_SECONDS", 30))
    return max(90, 5 * rounds)


# ---- what a wait saw (#103 item 12) -----------------------------------------------
# A wait that times out says what it waited for and what it saw last, and a long
# wait leaves a trail a reader can hold against the chain: each wait loop logs its
# state when it changes and at least every WAIT_LOG_SECONDS while it does not,
# every line with LEZ's finalized height and the sequencer's tip. A frozen chain
# then reads as finality standing still beside a moving tip, and a slow Node as a
# state that does not move while finality does.
WAIT_LOG_SECONDS = 60


def lez_rpc_number(url: str, method: str) -> int | None:
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": []}).encode()
    request = urllib.request.Request(url, data=body, headers={"content-type": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return int(json.load(response)["result"])
    except (OSError, ValueError, KeyError, TypeError):
        return None


def lez_chain_reading() -> dict:
    """LEZ's finalized height and the sequencer's tip as the host sees them: the
    finality heartbeat's status when it is fresh (it reads both every tick), else
    the stack's published indexer and sequencer ports. Never through a Node's
    loopback forwarder, which a scenario may have cut, and it never raises: a
    reading that failed is logged as `?`."""
    status = heartbeat_status()
    if status is not None:
        return {"finalized": status.get("finalized"), "tip": status.get("tip"), "source": "heartbeat"}
    return {"finalized": lez_rpc_number(STACK_INDEXER_URL, "getLastFinalizedBlockId"),
            "tip": lez_rpc_number(STACK_SEQUENCER_URL, "getLastBlockId"), "source": "rpc"}


def format_chain(reading: dict) -> str:
    finalized, tip = reading.get("finalized"), reading.get("tip")
    lag = f" ({tip - finalized} unfinalized)" if isinstance(finalized, int) and isinstance(tip, int) else ""
    return (f"LEZ finalized {'?' if finalized is None else finalized}, "
            f"tip {'?' if tip is None else tip}{lag} [{reading.get('source', '?')}]")


def format_wait_line(what: str, state: str, waited: float, reading: dict) -> str:
    return f"  [{int(waited)} s] waiting for {what}: {state} | {format_chain(reading)}"


def timeout_message(what: str, timeout: float, state: str, reading: dict) -> str:
    return f"timed out after {int(timeout)} s waiting for {what}; last seen: {state}; {format_chain(reading)}"


class Wait:
    """One wait loop's evidence and deadline:

        wait = Wait("the Maker's refund", timeout)
        while wait.running():
            wait.note(phase, lambda: f"maker {phase} | taker {state}")
            ...
        raise wait.failure()

    `note(key, state)` writes a line when `key` (cheap to compare) differs from
    the last one, or WAIT_LOG_SECONDS after the last line when it does not;
    `state` (a string, or a callable so that a costly detail is read only when a
    line is written) is what the line says, `str(key)` when it is left out."""

    def __init__(self, what: str, timeout: float, clock=time.time, every: float = WAIT_LOG_SECONDS):
        self.what, self.timeout, self.clock, self.every = what, timeout, clock, every
        self.started = clock()
        self.deadline = self.started + timeout
        self.key: object = None
        self.state = "nothing read yet"
        self.logged_at: float | None = None

    def running(self) -> bool:
        return self.clock() < self.deadline

    def note(self, key: object, state=None) -> bool:
        now = self.clock()
        if self.logged_at is not None and key == self.key and now - self.logged_at < self.every:
            return False
        self.key = key
        self.state = (state() if callable(state) else state) if state is not None else str(key)
        self.logged_at = now
        log(format_wait_line(self.what, self.state, now - self.started, lez_chain_reading()))
        return True

    def failure(self, kind: type[Failure] = Failure) -> Failure:
        return kind(timeout_message(self.what, self.timeout, self.state, lez_chain_reading()))


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
    wait = Wait(f"the Maker's own lock record for {swap_id[:12]}", timeout)
    while wait.running():
        recorded = maker_lock_recorded(swap_id)
        if recorded is not None:
            return recorded
        phase = maker_phase(swap_id)
        wait.note(phase, f"maker {phase}, no maker_lock effect")
        time.sleep(3)
    raise wait.failure()


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
    wait = Wait(f"the Taker's {swap_id[:12]} to reach {'/'.join(sorted(states))} {describe}".rstrip(), timeout)
    next_mine = time.time() + mine_every if mine_every else None
    while wait.running():
        view = taker_view(swap_id)
        state = view["state"]
        wait.note(state, lambda: f"taker {state} (gen {view.get('progress_generation')}) | maker {maker_phase(swap_id)}")
        if state in states:
            return view
        if state in TERMINAL:
            raise Failure(f"swap {swap_id[:12]} ended in {state} while waiting for {sorted(states)} {describe}")
        if next_mine and time.time() >= next_mine:
            mine(1)
            next_mine = time.time() + mine_every
        time.sleep(3)
    raise wait.failure()


def claim(swap_id: str, stamp: str, suffix: str = "") -> dict:
    view = wait_taker(swap_id, {"claim_available"}, timeout=1500, describe="(Maker funding)")
    result = call("taker", "taker_swap_claim_v1", {"schema_version": 1, "request_id": f"e2e-claim-{stamp}{suffix}",
                                                   "swap_id": swap_id, "expected_generation": view["progress_generation"]},
                  timeout=300)
    log(f"  Taker claimed {ROUTE['pair'] if REVERSE else 'LEZ'} (replay={result.get('was_replay')})")
    return result


def wait_completed(swap_id: str, timeout: int = 1800) -> None:
    wait_taker(swap_id, {"completed"}, timeout, describe="(Maker follow-up claim)")
    wait = Wait(f"the Maker to report {swap_id[:12]} completed and terminal", 300)
    while wait.running():
        view = maker_view(swap_id)
        phase = maker_phase(swap_id)
        if view.get("schedule_state") == "terminal" and phase == "completed":
            log(f"  both Nodes report {swap_id[:12]} completed")
            return
        wait.note((phase, view.get("schedule_state")), f"maker {phase} (schedule {view.get('schedule_state')})")
        time.sleep(3)
    raise wait.failure()


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


def kill_node(role: str) -> dict:
    """R4: the role's container killed with SIGKILL, not stopped. The container
    is the party's Basecamp, and the Node runs in it as a child of Basecamp's
    lez_swap_node module (modules/lez-role-node/node-entrypoint.sh), so the
    kill takes Basecamp, the Node and its helpers down together, none of them
    given a chance to clean up. Asserts the container exited by the signal
    (exit code 137, not killed for memory), then starts it again."""
    container, _ = NODES[role]
    log(f"  killing {container} with SIGKILL")
    docker("kill", "--signal", "KILL", container)
    state = json.loads(subprocess.run(["docker", "inspect", container, "--format", "{{json .State}}"],
                                      capture_output=True, text=True, check=True).stdout)
    if state.get("Running") or state.get("ExitCode") != 137 or state.get("OOMKilled"):
        raise Failure(f"{container} did not exit by SIGKILL: running {state.get('Running')}, exit code "
                      f"{state.get('ExitCode')}, OOMKilled {state.get('OOMKilled')}")
    log(f"  {container} exited by SIGKILL (exit code 137, not OOM-killed); starting it again")
    docker("start", container)
    wait_healthy(role)
    return {"container": container, "exit_code": state["ExitCode"], "oom_killed": state["OOMKilled"],
            "finished_at": state.get("FinishedAt")}


def scenario_restart(stamp: str, role: str, kill: bool = False) -> dict:
    """The role's Node restarted between the lock and the claim (`restart-<role>`,
    docker restart: SIGTERM), or killed there with SIGKILL and started again
    (`kill-<role>`, kill_node); either way the swap completes."""
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    lock(swap_id)
    container, _ = NODES[role]
    if kill:
        killed = kill_node(role)
    else:
        log(f"  restarting {container}")
        docker("restart", container)
        wait_healthy(role)
    claim(swap_id, stamp)
    wait_completed(swap_id)
    return {"swap_id": swap_id, "restarted": container, **({"killed": killed} if kill else {})}


def scenario_survivor(stamp: str) -> dict:
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    lock(swap_id)
    claim(swap_id, stamp)
    wait_taker(swap_id, {"claim_in_progress", "both_legs_locked", "completed"}, timeout=600)
    log("  stopping the Taker Node right after its revealing claim")
    docker("stop", NODES["taker"][0])
    try:
        wait = Wait("the Maker to complete without the Taker", 1800)
        while wait.running():
            phase = maker_phase(swap_id)
            if phase == "completed":
                break
            wait.note(phase, f"maker {phase}, the Taker Node stopped")
            time.sleep(5)
        else:
            raise wait.failure()
    finally:
        docker("start", NODES["taker"][0])
        wait_healthy("taker")
    wait_completed(swap_id, timeout=900)
    return {"swap_id": swap_id}


# ---- late claims (#103 item 14) ------------------------------------------------------
# A revealing claim that has been sent but is not yet on chain, with the Taker Node
# stopped across it: (a) late-claim-confirms, the claim confirms while the Taker is
# down; (b) late-claim-unmined, it is never mined until the refund path is open.
# Selling LEZ the claim is the Taker's Bitcoin spend of the Maker's lock, held in the
# mempool by stopping the lane's regtest miner (the harness mines what it needs); the
# refund path is opened in (b) by mining empty blocks (generateblock with no
# transactions) at the profile's own pace (MINE_INTERVAL), so the Maker's refund
# matures no faster than the agreement's block-time bound allows. Selling Bitcoin the
# claim is the Taker's LEZ claim, and LEZ has no mempool: (a) cuts the Taker's
# indexer forwarder the moment its claim is submitted, so the Taker cannot see it
# finalize, and stops it; (b) cuts its sequencer forwarder before the claim, so the
# claim the Node admits is never included.
TAKER_CLAIM_PENDING = {"claim_in_progress"}


def taker_claim_effect(view: dict) -> dict | None:
    for effect in view.get("effects") or []:
        if effect.get("kind") == "revealing_claim":
            return effect
    return None


def wait_bitcoin_claim_in_mempool(swap_id: str, timeout: int = 120) -> str:
    """The Taker's revealing Bitcoin claim, broadcast and waiting in the mempool
    (the miner is stopped): the mempool transaction that spends the Maker's
    lock, as Core decodes it, which is also the claim the Taker's view records
    when it lists one."""
    # The Taker can be at claim_available a drive before the Maker's own view records its lock.
    recorded = Wait(f"the Maker's record of its Bitcoin lock for {swap_id[:12]}", 90)
    maker_lock = maker_lock_recorded(swap_id)
    while maker_lock is None and recorded.running():
        recorded.note("none", lambda: f"maker {maker_phase(swap_id)}, no Bitcoin lock on record yet")
        time.sleep(2)
        maker_lock = maker_lock_recorded(swap_id)
    if maker_lock is None:
        raise Failure(f"the Maker has no Bitcoin lock on record for {swap_id[:12]}")
    wait = Wait(f"the Taker's Bitcoin claim of {swap_id[:12]} (a spend of {maker_lock[:12]}) in the mempool", timeout)
    while wait.running():
        pool = bitcoin("getrawmempool")
        for txid in pool:
            spent = {vin.get("txid") for vin in bitcoin("getrawtransaction", txid, "1").get("vin", [])}
            if maker_lock in spent:
                effect = taker_claim_effect(taker_view(swap_id))
                if effect and effect.get("transaction_id") not in (None, txid):
                    raise Failure(f"the mempool spends the Maker's lock with {txid[:12]}, but the Taker records "
                                  f"its claim as {effect['transaction_id'][:12]}")
                return txid
        wait.note(len(pool), f"mempool {[t[:12] for t in pool]}, none spends {maker_lock[:12]}")
        time.sleep(2)
    raise wait.failure()


def mine_empty_block() -> str:
    """A regtest block with no transaction in it: the mempool stays as it is."""
    return bitcoin("generateblock", MINER_ADDRESS, "[]")["hash"]


def drive_to_settlement(swap_id: str, stamp: str, timeout: int) -> tuple[dict, str]:
    """Lets both Nodes finish on their own, mining one block per round and
    doing only what each party's operator is offered: the Taker's refund when
    its row offers it (with its Bitcoin refund script matured first, once), the
    Maker's being queued already. Returns the Taker's last view and the Maker's
    phase once both are terminal."""
    profile = timing_profile()
    wait = Wait(f"both Nodes to settle {swap_id[:12]}", timeout)
    matured = False
    while wait.running():
        view = taker_view(swap_id)
        maker = maker_view(swap_id)
        phase = maker_phase(swap_id)
        if view["state"] == "attention_required":
            raise Failure(f"the Taker needs attention on {swap_id[:12]}: {view.get('attention_reason')}")
        if view["state"] in {"completed", "refunded"} and maker.get("schedule_state") == "terminal":
            return view, phase
        wait.note((view["state"], view.get("available_action"), phase),
                  f"taker {view['state']} offering {view.get('available_action')} | maker {phase} "
                  f"(schedule {maker.get('schedule_state')})")
        if view.get("available_action") == "refund":
            if not REVERSE and not matured:
                mine(int(profile["LEZ_BTC_REFUND_CSV_BLOCKS"]) + 1)
                matured = True
            rpc("taker", "taker_swap_refund_v1", {"schema_version": 1, "request_id": f"e2e-refund-{stamp}",
                                                 "swap_id": swap_id, "expected_generation": view["progress_generation"]},
                timeout=300)
        mine(1)
        time.sleep(5)
    raise wait.failure()


def assert_settled_consistently(before: dict, taker_state: str, maker_phase_end: str) -> dict:
    """No funds lost or doubly taken: the balances fit a completed or a refunded
    swap, and the one the Taker's Node reports."""
    after = party_balances()
    deltas = settlement_deltas(before, after)
    outcome = classify_settlement(deltas, REVERSE)
    log("  balances: " + "; ".join(f"{role} BTC {d['btc_sat']:+d} sat, LEZ {d['lez_units']:+d}"
                                   for role, d in deltas.items()))
    if outcome is None:
        raise Failure("the balances fit neither a completed nor a refunded swap (funds lost or taken twice): "
                      f"completed: {'; '.join(settlement_problems(deltas, 'completed', REVERSE))} / "
                      f"refunded: {'; '.join(settlement_problems(deltas, 'refunded', REVERSE))}")
    if outcome != taker_state:
        raise Failure(f"the balances say {outcome} but the Taker's Node reports {taker_state}")
    expected_maker = {"completed"} if outcome == "completed" else {"refunded", "maker_leg_refunded", "taker_leg_refunded"}
    if maker_phase_end not in expected_maker:
        raise Failure(f"the balances say {outcome} but the Maker's Node ended at {maker_phase_end}")
    log(f"  settled {outcome}: both Nodes and both chains agree")
    return {"balances_before": before, "balances_after": after, "deltas": deltas, "outcome": outcome}


def stop_taker() -> None:
    log("  stopping the Taker Node")
    docker("stop", NODES["taker"][0])


def start_taker() -> None:
    subprocess.run(["docker", "start", NODES["taker"][0]], check=False, capture_output=True)
    wait_healthy("taker")


def late_claim_setup(stamp: str) -> tuple[str, dict]:
    """Take, lock and wait until the Taker may claim; the balances before the take."""
    before = party_balances()
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    lock(swap_id)
    wait_taker(swap_id, {"claim_available"}, timeout=1500, describe="(Maker funding)")
    return swap_id, before


def send_claim(swap_id: str, stamp: str) -> dict:
    view = taker_view(swap_id)
    return rpc("taker", "taker_swap_claim_v1", {"schema_version": 1, "request_id": f"e2e-claim-{stamp}",
                                               "swap_id": swap_id, "expected_generation": view["progress_generation"]},
               timeout=300)


def scenario_late_claim_confirms(stamp: str) -> dict:
    """#103 item 14 (a): the Taker's revealing claim is sent, the Taker Node is
    stopped before it has seen the claim confirmed, and the claim confirms
    while it is down. The Maker must complete alone from the claim on chain,
    the Taker on its restart, and both chains must show the lots traded."""
    swap_id, before = late_claim_setup(stamp)
    evidence: dict = {}
    miner_stopped = False
    try:
        if REVERSE:
            log("  stopping the miner so the Taker's Bitcoin claim waits in the mempool")
            docker("stop", MINER)
            miner_stopped = True
            reply = send_claim(swap_id, stamp)
            if "error" in reply:
                raise Failure(f"taker_swap_claim_v1 failed: {json.dumps(reply['error'])[:300]}")
            claim_txid = wait_bitcoin_claim_in_mempool(swap_id)
            stop_taker()
            if claim_txid not in bitcoin("getrawmempool"):
                raise Failure(f"the claim {claim_txid[:12]} left the mempool before the Taker was down")
            [block] = bitcoin("generatetoaddress", "1", MINER_ADDRESS)
            if claim_txid not in bitcoin("getblock", block, "1")["tx"]:
                raise Failure(f"block {block[:12]} did not include the claim {claim_txid[:12]}")
            log(f"  the claim {claim_txid[:12]} confirmed in {block[:12]} with the Taker Node down")
            docker("start", MINER)
            miner_stopped = False
            evidence = {"claim": claim_txid, "claim_block": block}
        else:
            reply = send_claim(swap_id, stamp)
            # Blind the Taker to LEZ finality the moment the claim is out, then stop it.
            cut_lez_view("taker")
            if "error" in reply:
                raise Failure(f"taker_swap_claim_v1 failed: {json.dumps(reply['error'])[:300]}")
            view = taker_view(swap_id)
            effect = taker_claim_effect(view)
            if view["state"] not in TAKER_CLAIM_PENDING or not effect or not effect.get("pending"):
                raise NotExercised(f"the Taker had already seen its LEZ claim final ({view['state']}, "
                                   f"{json.dumps(effect)[:120]}) before it could be cut off")
            claim_txid = effect["transaction_id"]
            stop_taker()
            wait = Wait(f"the Taker's LEZ claim {claim_txid[:12]} included", 120)
            while wait.running():
                included = lez_transaction_block("maker", claim_txid)
                if included is not None:
                    break
                wait.note(None, "not included yet")
                time.sleep(2)
            else:
                raise wait.failure()
            wait_lez_final_through(included, timing_profile(), "the Taker's claim")
            log(f"  the claim {claim_txid[:12]} is final in LEZ block {included} with the Taker Node down")
            evidence = {"claim": claim_txid, "claim_block": included}
        wait = Wait("the Maker to complete with the Taker down", 900)
        while wait.running():
            phase = maker_phase(swap_id)
            if phase == "completed":
                break
            wait.note(phase, f"maker {phase}, the Taker Node stopped")
            time.sleep(5)
        else:
            raise wait.failure()
        log("  the Maker completed alone from the claim on chain")
    finally:
        if miner_stopped:
            subprocess.run(["docker", "start", MINER], check=False, capture_output=True)
        if not REVERSE:
            restore_lez_view("taker")  # a no-op on a stopped container; its start brings its own forwarders
        start_taker()
    wait_completed(swap_id, timeout=900)
    settlement = assert_settlement(before, "completed", "after the late claim")
    return {"swap_id": swap_id, **evidence, **settlement}


def scenario_late_claim_unmined(stamp: str) -> dict:
    """#103 item 14 (b): the Taker's revealing claim is sent but never mined, the
    Taker Node is stopped across the claim window, and nothing includes the
    claim until the refund path is open: the claim window closed (the
    agreement's earlier refund time) and, selling LEZ, the Maker's Bitcoin
    refund script matured, reached by empty blocks at the profile's pace with
    the claim still in the mempool. What each side does meanwhile is logged
    and, selling Bitcoin, the Maker is asserted not to claim (the secret never
    left the Taker). Then the Taker starts again with its route back or the
    miner resumes, the Maker's operator asks for its refund as the desk offers
    it, and both Nodes are left to settle (the Taker's refund asked for if its
    row offers one). The swap may end either way, but the balances must fit a
    completed or a refunded swap and agree with both Nodes: no party ends with
    both lots or without its own."""
    profile = timing_profile()
    pace = profile_seconds(profile, "MINE_INTERVAL", 2)
    swap_id, before = late_claim_setup(stamp)
    terms = taker_view(swap_id)["terms"]
    window_closes = int(terms["earlier_refund_latest_unix_seconds"])
    evidence: dict = {}
    if REVERSE:
        log("  stopping the miner so the Taker's Bitcoin claim waits in the mempool")
        docker("stop", MINER)
        try:
            reply = send_claim(swap_id, stamp)
            if "error" in reply:
                raise Failure(f"taker_swap_claim_v1 failed: {json.dumps(reply['error'])[:300]}")
            claim_txid = wait_bitcoin_claim_in_mempool(swap_id)
            stop_taker()
            maker_lock = maker_lock_recorded(swap_id)
            lock_height = int(bitcoin("getblockheader", str(bitcoin("getrawtransaction", maker_lock, "1")["blockhash"]))
                              ["height"]) if maker_lock else None
            refund_height = int(terms.get("bitcoin_refund_height") or 0) or (
                lock_height + int(profile["LEZ_BTC_SECOND_LOCK_REFUND_CSV_BLOCKS"]) if lock_height else None)
            if refund_height is None:
                raise Failure("neither the terms nor the Maker's lock give the Maker's Bitcoin refund height")
            log(f"  the claim {claim_txid[:12]} waits in the mempool with the Taker down; mining empty blocks every "
                f"{pace}s until the window closes ({window_closes}) and the Maker's refund opens at height {refund_height}")
            # Each round reads Core and the Maker before it mines, so it takes longer than the pace.
            bound = max(0, window_closes - time.time()) + (refund_height - int(bitcoin("getblockcount"))) * (pace + 3) + 600
            wait = Wait("the refund path to open with the claim unmined", bound)
            while wait.running():
                height = int(bitcoin("getblockcount"))
                if time.time() >= window_closes + 30 and height >= refund_height:
                    break
                if claim_txid not in bitcoin("getrawmempool"):
                    raise Failure(f"the claim {claim_txid[:12]} left the mempool although no block took it")
                wait.note(maker_phase(swap_id), lambda: f"maker {maker_phase(swap_id)}, height {height}, "
                                                        f"claim {claim_txid[:12]} in the mempool")
                mine_empty_block()
                time.sleep(pace)
            else:
                raise wait.failure()
            log(f"  the refund path is open at height {bitcoin('getblockcount')}; the claim was never mined; "
                f"the Maker is at {maker_phase(swap_id)}")
            evidence = {"claim": claim_txid, "maker_refund_height": refund_height}
            queue_maker_refund(swap_id, stamp, evidence)
        finally:
            docker("start", MINER)
            start_taker()
    else:
        log("  cutting the Taker's sequencer forwarder so the claim it sends is never included")
        zec_cut_routes("taker", "sequencer")
        try:
            reply = send_claim(swap_id, stamp)
            view = taker_view(swap_id)
            effect = taker_claim_effect(view)
            admitted = "result" in reply or view["state"] in TAKER_CLAIM_PENDING or bool(effect)
            log(f"  claim request {'admitted' if admitted else 'refused'}: "
                f"{json.dumps(reply.get('result') or reply.get('error'))[:160]}; taker {view['state']}, "
                f"claim effect {json.dumps(effect)[:120]}")
            if not admitted:
                raise Failure("the Taker Node did not admit a claim it could not submit, so no claim was sent; "
                              "this mechanism cannot hold a LEZ claim unincluded")
            claim_txid = (effect or {}).get("transaction_id")
            stop_taker()
            wait_until(window_closes + 30, "for the claim window to close with the claim never included",
                       lambda: f"maker {maker_phase(swap_id)}"
                               + (f", claim {claim_txid[:12]} included: {lez_transaction_block('maker', claim_txid)}"
                                  if claim_txid else ""))
            if claim_txid and lez_transaction_block("maker", claim_txid) is not None:
                raise Failure(f"the claim {claim_txid[:12]} was included although the Taker's sequencer route was cut")
            phase = maker_phase(swap_id)
            if phase in {"claim_evidence_available", "completed"}:
                raise Failure(f"the Maker reached {phase} although the claim never reached the chain")
            log(f"  the claim window closed with the claim never included; the Maker is at {phase}")
            evidence = {"claim": claim_txid}
            queue_maker_refund(swap_id, stamp, evidence)
        finally:
            zec_restore_routes("taker", "sequencer")  # a no-op on a stopped container; its start brings its own
            start_taker()
    try:
        taker_view_end, maker_phase_end = drive_to_settlement(
            swap_id, stamp, timeout=3 * int(profile["LEZ_LEZ_DISCOVERY_MAX_BLOCKS"])
            + int(profile["LEZ_BTC_LATER_REFUND_SECONDS"]) + 900)
    finally:
        if REVERSE:
            note_late_bitcoin_claim(evidence)
    settlement = assert_settled_consistently(before, taker_view_end["state"], maker_phase_end)
    return {"swap_id": swap_id, **evidence, "taker_terminal_state": taker_view_end["state"],
            "maker_terminal_phase": maker_phase_end, **settlement}


def note_late_bitcoin_claim(evidence: dict) -> None:
    """Bitcoin cannot expire a claim: the one held in the mempool is mined once
    the miner resumes, after the claim window, unless the Maker's refund took
    the lock first. Mined, it spent the Maker's lock, and the Maker must take
    the secret from it to its LEZ claim before the Taker's later refund time;
    the balances then say whether it did. Recorded either way, so a failure
    names which race it ran (the first run's claim was mined in the block
    after the refund opened, and the Maker never claimed)."""
    claim = evidence.get("claim")
    if not claim:
        return
    try:
        block = bitcoin("getrawtransaction", claim, "1").get("blockhash")
    except Failure:
        block = None
    height = int(bitcoin("getblockheader", block)["height"]) if block else None
    evidence["claim_mined_at_height"] = height
    log(f"  the held claim {claim[:12]} " + (f"was mined after all, at height {height}" if height is not None
                                            else "is not on chain"))


def queue_maker_refund(swap_id: str, stamp: str, evidence: dict) -> None:
    """The Maker's operator asks for its refund once the window has closed; the
    Node may refuse (a claim it can still see, say), which is recorded."""
    if maker_view(swap_id).get("schedule_state") == "terminal":
        evidence["maker_refund_request"] = "not needed: terminal"
        return
    reply = rpc("maker", "maker_actor_refund_v1", {"request_id": f"e2e-maker-refund-{stamp}", "id": swap_id,
                                                   "expected_generation": maker_view(swap_id)["lease_generation"]})
    evidence["maker_refund_request"] = reply.get("result") or reply.get("error")
    log(f"  the Maker's refund asked for: {json.dumps(evidence['maker_refund_request'])[:200]}")


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


# ---- balances on both chains (#103 items 10 and 14) ---------------------------------
# What each party holds, read from the chains rather than from the Nodes: its Node's
# Core wallet (getbalances, trusted + pending + immature, so that a coinbase maturing
# during the test moves nothing) and its LEZ account's finalized balance at the
# indexer. Fees are Bitcoin's alone: LEZ v0.2 charges no transaction fee (the
# sequencer pays Bedrock), and a Taker's LEZ lock and refund came back to the unit
# (regression-m2-2026-11-11, refund-then-maker-lock-lez.json). A party that sends
# Bitcoin pays at most two transactions' fees in one swap (a lock and its refund,
# or a claim): BTC_FEE_TOLERANCE_SAT is two transactions of at most 400 vB at the
# lock fee cap of 25 sat/vB (LEZ_BTC_LOCK_FEE_MAX_SAT_PER_VB); a lock and its refund
# cost 4,100 sat in that run.
BTC_FEE_TOLERANCE_SAT = 20_000


def wallet_total_sat(wallet: str) -> int:
    mine = bitcoin(f"-rpcwallet={wallet}", "getbalances")["mine"]
    return sum(round(float(mine.get(kind, 0)) * 100_000_000) for kind in ("trusted", "untrusted_pending", "immature"))


def party_balances() -> dict:
    """{role: {"btc_sat", "lez_units"}} for both parties, from the chains."""
    return {role: {"btc_sat": wallet_total_sat(node_bitcoin_wallet(role)),
                   "lez_units": lez_account_balance(lez_owner_account(role))}
            for role in ("maker", "taker")}


def settlement_deltas(before: dict, after: dict) -> dict:
    return {role: {asset: after[role][asset] - before[role][asset] for asset in before[role]} for role in before}


def expected_settlement(outcome: str, reverse: bool, foreign_units: int, lez_units: int) -> dict:
    """The nominal deltas of a swap that ended `completed` or `refunded`, fees
    aside: completed, each party has the other's lot; refunded, its own back."""
    if outcome == "refunded":
        return {role: {"btc_sat": 0, "lez_units": 0} for role in ("maker", "taker")}
    if outcome != "completed":
        raise ValueError(outcome)
    seller = -1 if not reverse else 1  # the Taker's Bitcoin: it sells it forward, buys it reversed
    return {"taker": {"btc_sat": seller * foreign_units, "lez_units": -seller * lez_units},
            "maker": {"btc_sat": -seller * foreign_units, "lez_units": seller * lez_units}}


def settlement_problems(deltas: dict, outcome: str, reverse: bool, foreign_units: int | None = None,
                        lez_units: int | None = None, btc_tolerance: int = BTC_FEE_TOLERANCE_SAT) -> list[str]:
    """What in `deltas` does not fit `outcome`: LEZ to the unit; Bitcoin up to
    the nominal and at most btc_tolerance below it for a party that sent a
    Bitcoin transaction (the one that locked Bitcoin, and in a completed swap
    the one that claimed it), to the satoshi for the other."""
    expected = expected_settlement(outcome, reverse, FOREIGN_UNITS if foreign_units is None else foreign_units,
                                   LEZ_UNITS if lez_units is None else lez_units)
    locker = "maker" if reverse else "taker"
    problems = []
    for role in ("maker", "taker"):
        got, want = deltas[role], expected[role]
        if got["lez_units"] != want["lez_units"]:
            problems.append(f"{role} LEZ {got['lez_units']:+d}, expected {want['lez_units']:+d}")
        sent_bitcoin = role == locker or outcome == "completed"
        low = want["btc_sat"] - (btc_tolerance if sent_bitcoin else 0)
        if not low <= got["btc_sat"] <= want["btc_sat"]:
            problems.append(f"{role} BTC {got['btc_sat']:+d} sat, expected {want['btc_sat']:+d}"
                            + (f" less at most {btc_tolerance} sat of fees" if sent_bitcoin else " exactly"))
    return problems


def classify_settlement(deltas: dict, reverse: bool, **amounts) -> str | None:
    """`completed` or `refunded` when the deltas fit that outcome, else None:
    a party holding both lots or neither."""
    for outcome in ("completed", "refunded"):
        if not settlement_problems(deltas, outcome, reverse, **amounts):
            return outcome
    return None


def assert_settlement(before: dict, outcome: str, label: str) -> dict:
    """Reads the balances again and fails unless they moved as `outcome` says."""
    after = party_balances()
    deltas = settlement_deltas(before, after)
    problems = settlement_problems(deltas, outcome, REVERSE)
    log(f"  balances {label}: " + "; ".join(f"{role} BTC {d['btc_sat']:+d} sat, LEZ {d['lez_units']:+d}"
                                             for role, d in deltas.items()))
    if problems:
        raise Failure(f"the balances do not fit a {outcome} swap ({label}): {'; '.join(problems)}")
    return {"balances_before": before, "balances_after": after, "deltas": deltas, "outcome": outcome}


def lez_transaction_block(role: str, transaction_id: str) -> int | None:
    """The block that role's own sequencer route (its loopback forwarder to
    3040) has a LEZ transaction in, None while the sequencer has not included it.
    The sequencer serves what it has included whether or not Bedrock has
    finalized it: it is where a LEZ transaction can be seen unconfirmed."""
    if not (len(transaction_id) == 64 and all(c in "0123456789abcdef" for c in transaction_id)):
        raise Failure(f"the LEZ lock's transaction id {transaction_id!r} is not a 32-byte hex hash")
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getTransaction", "params": [transaction_id]})
    result = subprocess.run(["docker", "exec", NODES[role][0], "curl", "-sS", "-m", "10",
                             "-H", "content-type: application/json", "--data", body, "http://127.0.0.1:3040/"],
                            capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL)
    try:
        reply = json.loads(result.stdout)
    except ValueError as error:
        raise Failure(f"the {role}'s sequencer route did not answer getTransaction: "
                      f"{result.stdout[:120]} {result.stderr[:120]}") from error
    if "error" in reply:
        raise Failure(f"the {role}'s sequencer refused getTransaction: {json.dumps(reply['error'])[:200]}")
    found = reply.get("result")
    return int(found[1]) if found else None


def lez_owner_account(role: str) -> str:
    """The LEZ account the role's Node pays from: its balance view's owner
    account, else the lane identity's (the market wallet its signer holds)."""
    if role == "maker":
        reply = rpc("maker", "maker_wallet_balances_v1", {})
    else:
        reply = rpc("taker", "taker_wallet_balances_v1", {"schema_version": 1})
    account = ((reply.get("result") or {}).get("lez") or {}).get("owner_account_base58")
    return account or json.loads((LANE_RUNTIME / "lez" / role / "identity.json").read_text())["account_id"]


def node_bitcoin_wallet(role: str) -> str:
    """The Core wallet the role's Node funds and is paid from (its balance view)."""
    if role == "maker":
        view = call("maker", "maker_wallet_balances_v1", {})
    else:
        view = call("taker", "taker_wallet_balances_v1", {"schema_version": 1})
    wallet = (view.get("bitcoin") or {}).get("wallet")
    if not wallet:
        raise Failure(f"the {role} Node names no Core wallet ({json.dumps(view.get('bitcoin'))[:120]}); "
                      "this scenario reads what that wallet spends")
    return wallet


def wallet_sends_since(wallet: str, block_hash: str) -> list[str]:
    """Transactions the Core wallet has sent since a block, in a later block or
    in the mempool: Core's own record of what left the wallet on chain. A
    funding that is only planned (inputs locked, nothing broadcast) is not one."""
    since = bitcoin(f"-rpcwallet={wallet}", "listsinceblock", block_hash, "1", "false", "true")
    return sorted({entry["txid"] for entry in since.get("transactions", []) if entry.get("category") == "send"})


def wait_lez_final_through(height: int, profile: dict, why: str) -> None:
    """Waits until the indexer has finalized `height`, so that anything
    submitted up to it can be read final; bounded by the profile's LEZ step and
    stall allowance (gen-config.sh: 20 + 40 s on the devnet)."""
    wait = Wait(f"LEZ finality to reach block {height} ({why})",
                3 * (profile_seconds(profile, "LEZ_LEZ_STEP_SECONDS", 20)
                     + profile_seconds(profile, "LEZ_LEZ_STALL_SECONDS", 40)))
    while wait.running():
        reading = lez_chain_reading()
        if isinstance(reading.get("finalized"), int) and reading["finalized"] >= height:
            return
        wait.note(reading.get("finalized"), format_chain(reading))
        time.sleep(2)
    raise wait.failure()


def scenario_early_lock_reverse(stamp: str) -> dict:
    """R1 mirrored: the Taker locks LEZ first, so the lock the Maker must not act on
    early is a LEZ one, and the lock it must not send is Bitcoin.

    What "seen but not confirmed" is on LEZ: the sequencer serves a transaction as
    soon as it includes it, and Bedrock finalizes it seconds later; a Node knows a
    LEZ lock is final only from the indexer, whose blocks are the finalized ones
    (the sidecar's observation reads its fixed finalized window there). LEZ has no
    mempool, and its finality cannot be held back without freezing the chain every
    lane shares, so the Maker is denied the finality source instead: its loopback
    forwarder to the indexer is stopped while its forwarder to the sequencer keeps
    running. The Taker's lock is then read back over the Maker's own sequencer
    route, included: the lock is in front of the Maker, and nothing it can read
    says it is final. For the profile's attention span (maker_attention_seconds)
    the Maker must not lock, which is checked three ways on every poll: its phase
    (nothing that implies an observed first leg), its own record (no maker_lock
    effect: maker_lock_recorded, not the Taker's view) and the chain (no send from
    the Maker's Core wallet since the hold began, in a block or in the mempool:
    listsinceblock). Once the view is back the Maker locks, and the same chain
    check is required to see that lock, so it cannot have been blind."""
    profile = timing_profile()
    held = maker_attention_seconds(profile)
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    wallet = node_bitcoin_wallet("maker")
    log("  cutting the Maker's indexer forwarder, its only source of LEZ finality; its sequencer forwarder stays")
    cut_lez_view("maker")
    try:
        wait_lez_view("maker", restored=False)
        start_block = str(bitcoin("getbestblockhash"))
        txid = lock(swap_id)
        wait = Wait(f"the Taker's LEZ lock {txid[:12]} on the Maker's own sequencer route", 120)
        while wait.running():
            included = lez_transaction_block("maker", txid)
            if included is not None:
                break
            wait.note(None, "not included yet")
            time.sleep(2)
        else:
            raise wait.failure()
        log(f"  the Taker's LEZ lock {txid[:12]} is in block {included} on the Maker's sequencer route, and the "
            f"Maker cannot read finality; watching it for {held}s")
        deadline = time.time() + held
        while time.time() < deadline:
            phase = maker_phase(swap_id)
            if phase in MAKER_OBSERVED_TAKER_LOCK:
                raise Failure(f"the Maker reached {phase} on a LEZ lock it could not see finalized (R1)")
            recorded = maker_lock_recorded(swap_id)
            if recorded is not None:
                raise Failure(f"the Maker recorded its Bitcoin lock {recorded[:12]} on a LEZ lock it could not see "
                              "finalized (R1)")
            sent = wallet_sends_since(wallet, start_block)
            if sent:
                raise Failure(f"the Maker's Core wallet {wallet} sent {[t[:12] for t in sent]} while it could not see "
                              "the Taker's lock finalized: a Bitcoin lock on chain or in the mempool (R1)")
            time.sleep(5)
        log(f"  for {held}s the Maker stayed at {maker_phase(swap_id)}: no maker_lock in its record, no send from "
            f"{wallet} since block {start_block[:12]} | {format_chain(lez_chain_reading())}")
    finally:
        restore_lez_view("maker")
    wait_lez_view("maker", restored=True)
    log("  the Maker can read LEZ finality again; it may now lock")
    maker_lock = wait_maker_past_lock(swap_id, timeout=900)
    sent = wallet_sends_since(wallet, start_block)
    if maker_lock not in sent:
        raise Failure(f"the Maker recorded its Bitcoin lock {maker_lock[:12]} but {wallet} shows no such send since "
                      f"{start_block[:12]} ({[t[:12] for t in sent]}): the chain check above could not have seen it")
    log(f"  the Maker's lock {maker_lock[:12]} is a send from {wallet}, so the check above was able to see one")
    claim(swap_id, stamp)
    wait_completed(swap_id)
    return {"swap_id": swap_id, "taker_lez_lock": txid, "seen_in_block": included, "held_seconds": held,
            "maker_bitcoin_lock": maker_lock, "bitcoin_claim": assert_cooperative_claim_is_key_path(swap_id)}


def scenario_early_lock(stamp: str) -> dict:
    """R1: the Maker does not lock until the Taker's lock is confirmed.

    Forward the Taker's lock is the Bitcoin one, and on regtest the harness decides when a
    transaction confirms: the miner is held so the lock sits in the mempool, below the
    agreement's required confirmations, and the Maker must sit with it. Its LEZ lock is
    checked absent in its own record (maker_lock_recorded) and on the chain (its LEZ
    account's finalized balance unchanged once finality has caught up with everything the
    sequencer had at the end of the hold). Mirrored the Taker's lock is LEZ:
    scenario_early_lock_reverse.
    """
    if REVERSE:
        return scenario_early_lock_reverse(stamp)
    profile = timing_profile()
    held = maker_attention_seconds(profile)
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    required = int(taker_view(swap_id)["terms"].get("required_bitcoin_confirmations") or 1)
    account = lez_owner_account("maker")
    balance = lez_account_balance(account)
    log("  stopping the miner so the Taker's lock stays unconfirmed")
    docker("stop", MINER)
    try:
        txid = lock(swap_id)
        confirmations = (bitcoin("getrawtransaction", txid, "1") or {}).get("confirmations", 0)
        if confirmations:
            raise Failure(f"the lock already has {confirmations} confirmations; the miner was not held")
        if txid not in bitcoin("getrawmempool"):
            raise Failure(f"the Taker's lock {txid[:12]} is not in the mempool: the Maker could not see it at all")
        log(f"  the Taker's lock {txid[:12]} is in the mempool with 0 of {required} required confirmations; "
            f"watching the Maker for {held}s")
        deadline = time.time() + held
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
        # The chain: a LEZ lock submitted in the hold is final once finality has
        # passed what the sequencer had at its end; the Maker's account must not
        # have paid anything by then.
        tip = lez_chain_reading().get("tip")
        if isinstance(tip, int):
            wait_lez_final_through(tip, profile, "everything submitted during the hold")
        if lez_account_balance(account) != balance:
            raise Failure(f"the Maker's LEZ account {account[:12]} went from {balance} to "
                          f"{lez_account_balance(account)} while the Taker's lock was unconfirmed (R1)")
        log(f"  the Maker stayed at {maker_phase(swap_id)} for {held}s with the lock unconfirmed; its LEZ account "
            f"still holds {balance} | {format_chain(lez_chain_reading())}")
        mine(1)
        log("  the Taker's lock is confirmed; the Maker may now lock")
        wait_maker_past_lock(swap_id, timeout=900)
    finally:
        docker("start", MINER)
        wait_healthy("maker")
    claim(swap_id, stamp)
    wait_completed(swap_id)
    return {"swap_id": swap_id, "lock_txid": txid, "held_seconds": held, "required_confirmations": required,
            "bitcoin_claim": assert_cooperative_claim_is_key_path(swap_id)}


# The two off-chain channels of this stack (docs/architecture/deployment-components-
# and-rpcs.md, "Component map"): the Maker Node's Chat listener is a Unix socket on
# the maker_socket volume, which the Taker's Basecamp mounts at the same path, and
# Delivery is the delivery_dir volume, written by the Maker and mounted read-only by
# the Taker. Both Nodes run as uid 4713 (compose.yaml `user:`) with no capabilities.
# The cut denies the inodes themselves: the socket to mode 0000 (connect(2) needs
# write permission on it) and the directory to 0000 (no listing, no lookup), so every
# non-root process on either side of the shared volume, the Taker Node included, is
# refused by the kernel, and a fresh connect or read is impossible rather than merely
# pointed elsewhere. Chat opens one connection per call (local_rpc.rs), so a
# connection that was open at the cut is a call in flight; the cut waits for the
# Maker's last accepted connection on the socket to close and then requires that no
# connection appears again (ChatConnections). Restoring puts back the modes the Nodes
# check for (owner_socket_is_available: 0600; Delivery: 0700).
CHAT_SOCKET = "/run/lez/maker/chat.sock"
DELIVERY_DIR = "/delivery"
CHAT_SOCKET_MODE, DELIVERY_MODE = "0600", "0700"
CHAT_DRAIN_SECONDS = 60


def offchain_cut_commands(prefix: str) -> list[list[str]]:
    maker = f"{prefix}-maker-node"
    return [["docker", "exec", maker, "chmod", "0000", CHAT_SOCKET],
            ["docker", "exec", maker, "chmod", "0000", DELIVERY_DIR]]


def offchain_restore_commands(prefix: str) -> list[list[str]]:
    maker = f"{prefix}-maker-node"
    return [["docker", "exec", maker, "chmod", CHAT_SOCKET_MODE, CHAT_SOCKET],
            ["docker", "exec", maker, "chmod", DELIVERY_MODE, DELIVERY_DIR]]


# Run in the Taker's container as its Node's user: what that user can still reach.
TAKER_OFFCHAIN_PROBE = (
    "import errno, json, os, socket, sys\n"
    "chat, delivery = sys.argv[1], sys.argv[2]\n"
    "out = {}\n"
    "s = socket.socket(socket.AF_UNIX)\n"
    "s.settimeout(5)\n"
    "try:\n"
    "    s.connect(chat); out['chat'] = 'connected'\n"
    "except OSError as e:\n"
    "    out['chat'] = errno.errorcode.get(e.errno, str(e.errno))\n"
    "finally:\n"
    "    s.close()\n"
    "try:\n"
    "    os.listdir(delivery); out['delivery'] = 'readable'\n"
    "except OSError as e:\n"
    "    out['delivery'] = errno.errorcode.get(e.errno, str(e.errno))\n"
    "fds = 0\n"
    "for pid in filter(str.isdigit, os.listdir('/proc')):\n"
    "    try:\n"
    "        names = os.listdir(f'/proc/{pid}/fd')\n"
    "    except OSError:\n"
    "        continue\n"
    "    for fd in names:\n"
    "        try:\n"
    "            fds += os.readlink(f'/proc/{pid}/fd/{fd}').startswith(delivery)\n"
    "        except OSError:\n"
    "            pass\n"
    "out['delivery_fds'] = fds\n"
    "out['uid'] = os.getuid()\n"
    "print(json.dumps(out))\n"
)


def taker_offchain_probe() -> dict:
    """What the Taker's side can still do: connect to the Maker's Chat socket,
    list Delivery, and how many open file descriptors in its container point
    into Delivery. Run as the container's user, the one its Node runs as."""
    result = subprocess.run(["docker", "exec", NODES["taker"][0], "python3", "-c", TAKER_OFFCHAIN_PROBE,
                             CHAT_SOCKET, DELIVERY_DIR], capture_output=True, text=True, check=False,
                            stdin=subprocess.DEVNULL)
    try:
        return json.loads(result.stdout)
    except ValueError as error:
        raise Failure(f"the Taker-side probe did not run: {result.stdout[:120]} {result.stderr[:200]}") from error


def connected_chat_sockets(proc_net_unix: str, path: str = CHAT_SOCKET) -> int:
    """Connections accepted on `path` that are still open, from a /proc/net/unix
    listing: an accepted socket carries its listener's address, and state 03 is
    connected (the listener itself is 01)."""
    count = 0
    for line in proc_net_unix.splitlines()[1:]:
        fields = line.split()
        if len(fields) >= 8 and fields[7] == path and fields[5] == "03":
            count += 1
    return count


class ChatConnections(threading.Thread):
    """Samples the Maker's open connections on its Chat socket every two seconds
    (its container's /proc/net/unix) while a cut is meant to hold, and remembers
    every sample that showed one."""

    def __init__(self) -> None:
        super().__init__(daemon=True)
        self.stopped = threading.Event()
        self.samples = 0
        self.seen: list[tuple[str, int]] = []

    def read(self) -> int | None:
        result = subprocess.run(["docker", "exec", NODES["maker"][0], "cat", "/proc/net/unix"],
                                capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL)
        return connected_chat_sockets(result.stdout) if result.returncode == 0 else None

    def run(self) -> None:
        while not self.stopped.wait(2):
            open_now = self.read()
            if open_now is None:
                continue
            self.samples += 1
            if open_now:
                self.seen.append((time.strftime("%H:%M:%S"), open_now))

    def stop(self) -> None:
        self.stopped.set()
        self.join(timeout=30)


def wait_chat_drained() -> None:
    """A call in flight at the cut keeps its connection until it ends; waits for
    the Maker to hold none (bounded by the Chat client's call timeout)."""
    wait = Wait("the Maker's Chat connections open at the cut to close", CHAT_DRAIN_SECONDS)
    probe = ChatConnections()
    while wait.running():
        open_now = probe.read()
        if open_now == 0:
            return
        wait.note(open_now, f"{open_now} connection(s) still open on {CHAT_SOCKET}")
        time.sleep(2)
    raise wait.failure()


def health(role: str) -> dict:
    if role == "maker":
        return call("maker", "maker_health", {})
    return call("taker", "taker_health", {"schema_version": 1})


def cut_delivery_and_chat() -> ChatConnections:
    """Cuts both channels and proves the cut from both sides before anything
    else happens: the Taker's user can still connect and list beforehand (so the
    probe is not blind), is refused both afterwards with EACCES, holds no file
    descriptor in Delivery, the Maker holds no Chat connection once a call in
    flight has ended, and both Nodes report chat and delivery unavailable and
    are degraded. Returns the Chat watch, running until the restore."""
    before = taker_offchain_probe()
    if before.get("chat") != "connected" or before.get("delivery") != "readable":
        raise Failure(f"before the cut the Taker's side already cannot reach Chat or Delivery: {before}")
    for command in offchain_cut_commands(PREFIX):
        subprocess.run(command, check=True, capture_output=True, stdin=subprocess.DEVNULL)
    after = taker_offchain_probe()
    if after.get("chat") != "EACCES" or after.get("delivery") != "EACCES" or after.get("delivery_fds"):
        raise Failure(f"the cut does not hold on the Taker's side (uid {after.get('uid')}): {after}")
    log(f"  the Taker's side (uid {after['uid']}) is refused: connect {after['chat']}, list Delivery "
        f"{after['delivery']}, {after['delivery_fds']} descriptors into it")
    wait_chat_drained()
    watch = ChatConnections()
    watch.start()
    for role in ("maker", "taker"):
        wait = Wait(f"the {role} Node to report chat and delivery unavailable", 120)
        while wait.running():
            reported = health(role)
            if reported.get("chat") == "unavailable" and reported.get("delivery") == "unavailable":
                break
            wait.note((reported.get("chat"), reported.get("delivery")),
                      f"chat {reported.get('chat')}, delivery {reported.get('delivery')}")
            time.sleep(5)
        else:
            raise wait.failure()
        if not reported.get("degraded"):
            raise Failure(f"the {role} reports both dependencies unavailable but is not degraded")
    log("  both Nodes report chat and delivery unavailable, and degraded; the Maker holds no Chat connection")
    return watch


def assert_cut_held(watch: ChatConnections) -> dict:
    """At the end of what ran under the cut: the Taker's side is still refused
    and no Chat connection was seen open at any sample since the drain."""
    watch.stop()
    after = taker_offchain_probe()
    if after.get("chat") != "EACCES" or after.get("delivery") != "EACCES" or after.get("delivery_fds"):
        raise Failure(f"the cut no longer held at the end: {after}")
    if watch.seen:
        raise Failure(f"a Chat connection was open on the Maker during the cut: {watch.seen[:5]}")
    if watch.samples == 0:
        raise Failure("the Maker's /proc/net/unix could not be read during the cut; nothing shows Chat stayed unused")
    log(f"  the cut held: {watch.samples} samples of the Maker's Chat socket, none with a connection; "
        "the Taker's side still refused")
    return {"chat_samples": watch.samples, "chat_connections_seen": 0, "taker_probe_at_end": after}


def restore_delivery_and_chat() -> None:
    for command in offchain_restore_commands(PREFIX):
        subprocess.run(command, check=False, capture_output=True, stdin=subprocess.DEVNULL)


def assert_restored() -> None:
    """Both channels back: the Taker's side connects and lists again and neither
    Node is degraded."""
    probe = taker_offchain_probe()
    if probe.get("chat") != "connected" or probe.get("delivery") != "readable":
        raise Failure(f"after the restore the Taker's side still cannot reach Chat or Delivery: {probe}")
    for role in ("maker", "taker"):
        wait = Wait(f"the {role} Node to leave degraded once both dependencies returned", 120)
        while wait.running():
            reported = health(role)
            if not reported.get("degraded"):
                break
            wait.note((reported.get("chat"), reported.get("delivery")),
                      f"chat {reported.get('chat')}, delivery {reported.get('delivery')}, degraded")
            time.sleep(5)
        else:
            raise wait.failure()
    log("  both dependencies are back and neither Node is degraded")


def scenario_offline_after_lock(stamp: str) -> dict:
    """R2: once the first lock is on chain the swap proceeds on chain state alone.

    Both off-chain dependencies are cut after the lock (cut_delivery_and_chat: the
    kernel refuses the Taker's Node user a connect to Chat and a read of Delivery, as
    proven from the Taker's container) and stay cut until the swap is complete, while
    the Maker's open Chat connections are sampled throughout and must stay none.
    Neither Node is restarted: the point is that a swap in flight does not need Logos
    Chat or Delivery, not that it survives a restart.
    """
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    lock(swap_id)
    log("  cutting Logos Chat and Delivery, with the first lock already on chain")
    watch = None
    try:
        watch = cut_delivery_and_chat()
        claim(swap_id, stamp)
        wait_completed(swap_id)
        held = assert_cut_held(watch)
        log("  the swap completed with both off-chain dependencies cut")
    finally:
        if watch is not None:
            watch.stop()
        restore_delivery_and_chat()
    assert_restored()
    return {"swap_id": swap_id, "cut": held, "bitcoin_claim": assert_cooperative_claim_is_key_path(swap_id)}


def maker_cli(*arguments: str) -> object:
    """One lez-maker-cli command in the Maker's container, as the Node's owner
    (the container's user); its JSON answer."""
    result = subprocess.run(["docker", "exec", NODES["maker"][0], "lez-maker-cli", *arguments],
                            capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL)
    if result.returncode != 0:
        raise Failure(f"lez-maker-cli {arguments[0]} failed ({result.returncode}): {result.stderr.strip()[-300:]}")
    return json.loads(result.stdout)


def scenario_maker_cli(stamp: str) -> dict:
    """U3/F9: the happy swap with every Maker call made by the operator CLI the
    role image ships (`docker exec <maker> lez-maker-cli`), the Taker's as
    always. The CLI has no command for maker_local_route_save_v1, which the
    other scenarios use to save the pair and its price at once, so the route is
    configured with configure-pair, then set-local-price, each against the
    revision `pairs` and `prices` list; then publish-offer, offers, monitor
    until the Maker reports the swap completed and terminal, and history. The
    owner boundary: the same CLI as another user (nobody, 65534) is refused
    connecting to the Node's 0600 socket, so no request is made."""
    route = ["--pair", "bitcoin", "--direction", "taker-sells-lez" if REVERSE else "taker-sells-foreign"]
    revision = lambda records: next((["--expected-revision", str(r["revision"])] for r in records
                                     if r["value"]["route"] == ROUTE), [])
    maker_cli("configure-pair", "--request-id", f"e2e-cli-pair-{stamp}", *revision(maker_cli("pairs")), *route,
              "--enabled", "true", "--price-source", "local", "--minimum-foreign-units", str(FOREIGN_UNITS),
              "--maximum-foreign-units", str(FOREIGN_UNITS), "--offer-ttl-seconds", "3600")
    maker_cli("set-local-price", "--request-id", f"e2e-cli-price-{stamp}", *revision(maker_cli("prices")), *route,
              "--lez-units-per-lot", "1", "--foreign-units-per-lot", "1000")
    offer_id = f"offer-e2e-cli-{stamp}"
    maker_cli("publish-offer", "--request-id", f"e2e-cli-publish-{stamp}", "--offer-id", offer_id, *route)
    listed = [record for record in maker_cli("offers") if record["offer"]["id"] == offer_id]
    if len(listed) != 1 or listed[0]["offer"]["pair_configuration"]["route"] != ROUTE:
        raise Failure(f"lez-maker-cli offers does not list {offer_id} on {ROUTE}: {listed}")
    log(f"  Maker CLI configured the route, set its price and published {offer_id} ({listed[0]['status']})")
    swap_id, _ = take(offer_id, stamp)
    lock(swap_id)
    claim(swap_id, stamp)
    wait_taker(swap_id, {"completed"}, 1800, describe="(Maker follow-up claim)")
    wait = Wait(f"lez-maker-cli monitor to report {swap_id[:12]} completed and terminal", 300)
    while wait.running():
        view = maker_cli("monitor", "--id", swap_id)
        phase = ((view.get("progress") or {}).get("observation") or {}).get("phase") or view.get("schedule_state")
        if view.get("schedule_state") == "terminal" and phase == "completed":
            break
        wait.note((phase, view.get("schedule_state")), f"maker {phase} (schedule {view.get('schedule_state')})")
        time.sleep(3)
    else:
        raise wait.failure()
    history = [entry for entry in maker_cli("history") if entry.get("id") == swap_id]
    if [entry["phase"] for entry in history] != ["Completed"]:
        raise Failure(f"lez-maker-cli history does not list {swap_id[:12]} Completed: {history}")
    log(f"  lez-maker-cli monitor reports {swap_id[:12]} completed and terminal; history lists it Completed")
    container, socket = NODES["maker"]
    denied = subprocess.run(["docker", "exec", "-u", "65534", container, "lez-maker-cli", "health"],
                            capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL)
    mode = subprocess.run(["docker", "exec", container, "stat", "-c", "%a %U", socket],
                          capture_output=True, text=True, check=False).stdout.strip()
    if denied.returncode == 0 or "connect local RPC socket" not in denied.stderr or "Permission denied" not in denied.stderr:
        raise Failure(f"lez-maker-cli health as uid 65534 was not refused at the socket: exit {denied.returncode}, "
                      f"{denied.stdout.strip()[:200]} {denied.stderr.strip()[:200]}")
    log(f"  as uid 65534 lez-maker-cli health is refused connecting to {socket} ({mode}): no request made")
    return {"swap_id": swap_id, "offer_id": offer_id, "history": history,
            "non_owner_health": {"exit": denied.returncode, "stderr": denied.stderr.strip()[-200:], "socket": mode}}


# ---- the Taker's side through its operator CLI (taker-cli) --------------------------
# With TAKER_CLI set, every owner call a scenario makes to the Taker Node is a
# `docker exec <taker> lez-taker-cli` command instead (taker_cli_rpc), so the
# scenario's Taker side is what an operator types; the Maker's is unchanged.
TAKER_CLI = False
# The CLI's reads, polled every few seconds; every other command's exit and
# duration is logged (the CLI's own timeouts: take 7 min, lock 2, claim/refund 5).
TAKER_CLI_READS = {"offers", "list", "monitor"}


def taker_cli_arguments(method: str, params: dict) -> list[str]:
    """The lez-taker-cli command for one Taker owner RPC (docs/api/README.md)."""
    if method == "taker_offer_list_v1":
        route = params.get("route")
        return ["swap", "offers"] + (["--pair", route["pair"].lower(), "--direction",
                                      "taker-sells-lez" if route["direction"] == "TakerSellsLez"
                                      else "taker-sells-foreign"] if route else [])
    if method == "taker_swap_initiate_v1":
        funding = params.get("funding_transaction_hex")
        return (["swap", "take", "--request-id", params["request_id"], "--offer-id", params["offer_id"],
                 "--foreign-units", str(params["foreign_units"])]
                + (["--funding-transaction-hex", funding] if funding else []))
    if method in ("taker_swap_monitor_v1", "taker_swap_lock_v1"):
        return ["swap", method.split("_")[2], "--swap-id", params["swap_id"]]
    if method in ("taker_swap_claim_v1", "taker_swap_refund_v1"):
        return ["swap", method.split("_")[2], "--swap-id", params["swap_id"], "--request-id", params["request_id"],
                "--expected-generation", str(params["expected_generation"])]
    commands = {"taker_swap_list_v1": ["swap", "list"], "taker_wallet_balances_v1": ["balances"],
                "taker_health": ["health"]}
    if method not in commands:
        raise Failure(f"lez-taker-cli has no command for {method}")
    return commands[method]


def taker_cli_rpc(method: str, params: dict) -> dict:
    """One Taker owner call through lez-taker-cli, answered as rpc() answers:
    the printed JSON as the result, or the printed `local RPC error <code>:
    <message> <data>` as the error."""
    arguments = taker_cli_arguments(method, params)
    started = time.time()
    result = subprocess.run(["docker", "exec", NODES["taker"][0], "lez-taker-cli", *arguments],
                            capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL, timeout=600)
    if arguments[0] == "swap" and arguments[1] not in TAKER_CLI_READS:
        log(f"  lez-taker-cli {' '.join(arguments[:2])}: exit {result.returncode} after {time.time() - started:.0f} s"
            + (f": {result.stderr.strip()[-300:]}" if result.returncode else ""))
    if result.returncode == 0:
        return {"result": json.loads(result.stdout)}
    printed = re.search(r"local RPC error (-?\d+): (.*)", result.stderr)
    if printed is None:
        return {"error": {"code": -1, "message": f"lez-taker-cli: {result.stderr.strip()[-300:]}"}}
    code, text = int(printed.group(1)), printed.group(2).strip()
    # the server's data follows the message as compact JSON
    for at in (i for i in range(len(text)) if text.startswith(" {", i)):
        try:
            return {"error": {"code": code, "message": text[:at], "data": json.loads(text[at + 1:])}}
        except ValueError:
            continue
    return {"error": {"code": code, "message": text}}


def through_taker_cli(scenario):
    """`scenario` with the Taker driven by lez-taker-cli (TAKER_CLI)."""
    def run(stamp: str) -> dict:
        global TAKER_CLI
        TAKER_CLI = True
        try:
            return scenario(stamp)
        finally:
            TAKER_CLI = False
    return run


# The Maker's state volume holds the TEST price modules: a directory of the
# Node's user, mode 0755, as the Node's file rules ask (logos_price_source.rs).
PRICE_MODULE_DIR = "/var/lib/lez/maker/price-module"


def install_test_price_module(variant: str) -> tuple[str, str]:
    """The repository's TEST price module (crates/logos-price-c-api/tests/
    fixtures/logos_price_fixture.c), compiled as the crate's test compiles it,
    in the image the Nodes are built in, with its FIXTURE_<variant> mode ("" for
    none), written into the Maker's container. Its path there and its sha256."""
    crate = DEPLOY_ROOT.parent / "crates/logos-price-c-api"
    name = f"lez-test-price-fixture-{variant.lower() or 'ok'}.so"
    with tempfile.TemporaryDirectory() as out:
        subprocess.run(["docker", "run", "--rm", "--user", f"{os.getuid()}:{os.getgid()}",
                        "-v", f"{crate}:/crate:ro", "-v", f"{out}:/out", "rust:1.96.0-bookworm",
                        "cc", "-shared", "-fPIC", "-std=c11", "-Wall", "-Wextra", "-Werror", "-I", "/crate/include",
                        *([f"-DFIXTURE_{variant}"] if variant else []),
                        "/crate/tests/fixtures/logos_price_fixture.c", "-o", f"/out/{name}"],
                       check=True, capture_output=True, stdin=subprocess.DEVNULL)
        module = (pathlib.Path(out) / name).read_bytes()
    path = f"{PRICE_MODULE_DIR}/{name}"
    subprocess.run(["docker", "exec", "-i", NODES["maker"][0], "sh", "-c",
                    f"umask 022 && mkdir -p {PRICE_MODULE_DIR} && cat >{path}"], input=module, check=True)
    return path, hashlib.sha256(module).hexdigest()


def recreate_maker_with_price_module(module: str = "", sha256: str = "") -> None:
    """The Maker's Basecamp recreated with LEZ_LOGOS_PRICE_MODULE and
    LEZ_LOGOS_PRICE_MODULE_SHA256 (empty: the default, no price module)."""
    compose = os.environ.get("LEZ_STACK_COMPOSE_ARGS", "--env-file runtime/runtime.env").split()
    subprocess.run(["docker", "compose", *compose, "up", "-d", "--no-build", "--force-recreate", "--no-deps", "maker-node"],
                   cwd=DEPLOY_ROOT, check=True, capture_output=True, stdin=subprocess.DEVNULL,
                   env={**os.environ, "LEZ_LOGOS_PRICE_MODULE": module, "LEZ_LOGOS_PRICE_MODULE_SHA256": sha256})
    wait_healthy("maker", timeout=300)
    arguments = subprocess.run(["docker", "exec", NODES["maker"][0], "ps", "-eo", "args"],
                               capture_output=True, text=True, check=False).stdout
    node = next((line for line in arguments.splitlines() if line.split(" ", 1)[0].endswith("/lez-maker-node")), "")
    if bool(module) != (f"--logos-price-module {module} --logos-price-module-sha256 {sha256}" in node):
        raise Failure(f"the recreated Maker Node does not run with price module {module or 'none'}: {node[-400:]}")
    log(f"  Maker's Basecamp recreated with {'price module ' + module if module else 'no price module'}")


def scenario_maker_price_module(stamp: str) -> dict:
    """F8: a Maker prices a route with a Logos C-API price module, the
    repository's TEST fixture (no real Logos price module exists). The Maker
    runs it through its package's lez-logos-price-worker (LEZ_LOGOS_PRICE_MODULE
    and its sha256); configure-pair selects the logos-c-api source; quote
    answers the module's price (5 LEZ units per 2 sat, source revision 7); two
    offers are published at it, carrying the module's sha256 as their source
    identity; the Taker takes one and the swap completes. Then the Maker runs
    the fixture's UNAVAILABLE build instead: quote and publish fail closed with
    the typed unavailable error (-32003), and the offer still out is listed
    active by the Maker and still offered to the Taker. The Maker goes back to
    no price module however the scenario ends."""
    global FOREIGN_UNITS, LEZ_UNITS
    kept_units = FOREIGN_UNITS, LEZ_UNITS
    FOREIGN_UNITS, LEZ_UNITS = 200_000, 500_000  # the fixture's 5 LEZ units per 2 sat
    route = ["--pair", "bitcoin", "--direction", "taker-sells-foreign"]
    try:
        module, module_sha256 = install_test_price_module("")
        recreate_maker_with_price_module(module, module_sha256)
        revision = next((["--expected-revision", str(r["revision"])] for r in maker_cli("pairs")
                         if r["value"]["route"] == ROUTE), [])
        maker_cli("configure-pair", "--request-id", f"e2e-price-pair-{stamp}", *revision, *route,
                  "--enabled", "true", "--price-source", "logos-c-api", "--minimum-foreign-units", str(FOREIGN_UNITS),
                  "--maximum-foreign-units", str(FOREIGN_UNITS), "--offer-ttl-seconds", "3600")
        quote = maker_cli("quote", *route)
        if (quote["price"]["lez_units_per_lot"], quote["price"]["foreign_units_per_lot"], quote["source_revision"]) != (5, 2, 7):
            raise Failure(f"quote is not the module's 5/2 at source revision 7: {quote}")
        log(f"  quote through the module: {quote['price']['lez_units_per_lot']} LEZ units per "
            f"{quote['price']['foreign_units_per_lot']} sat, source revision {quote['source_revision']}")
        offers = []
        for suffix in ("taken", "kept"):
            offer_id = f"offer-e2e-price-{suffix}-{stamp}"
            maker_cli("publish-offer", "--request-id", f"e2e-price-publish-{suffix}-{stamp}", "--offer-id", offer_id, *route)
            offer = next(r for r in maker_cli("offers") if r["offer"]["id"] == offer_id)["offer"]
            identity = offer.get("price_source_identity_sha256")
            identity = bytes(identity).hex() if isinstance(identity, list) else identity
            if identity != module_sha256 or offer["price"] != quote["price"]:
                raise Failure(f"{offer_id} is not at the module's price under its identity: {offer}")
            offers.append(offer_id)
        log(f"  published {', '.join(offers)} at the module's price, source identity {module_sha256[:16]}…")
        swap_id, _ = take(offers[0], stamp)
        lock(swap_id)
        claim(swap_id, stamp)
        wait_taker(swap_id, {"completed"}, 1800, describe="(Maker follow-up claim)")
        log(f"  swap {swap_id[:12]} on the module-priced offer completed")

        unavailable, unavailable_sha256 = install_test_price_module("UNAVAILABLE")
        recreate_maker_with_price_module(unavailable, unavailable_sha256)
        refused = {"quote": rpc("maker", "maker_price_quote", {"route": ROUTE}),
                   "publish": rpc("maker", "maker_offer_publish", {"request_id": f"e2e-price-publish-down-{stamp}",
                                                                   "offer_id": f"offer-e2e-price-down-{stamp}",
                                                                   "route": ROUTE})}
        for method, reply in refused.items():
            if (reply.get("error") or {}).get("code") != -32003:
                raise Failure(f"{method} with the module unavailable is not the typed unavailable error: {reply}")
        if any(r["offer"]["id"] == f"offer-e2e-price-down-{stamp}" for r in maker_cli("offers")):
            raise Failure("the refused publish left an offer behind")
        kept = next(r for r in maker_cli("offers") if r["offer"]["id"] == offers[1])
        if kept["status"] != "active":
            raise Failure(f"{offers[1]} is no longer active with the module unavailable: {kept['status']}")
        discover(offers[1])
        log(f"  module unavailable: quote and publish answer {refused['quote']['error']['message']!r} (-32003); "
            f"{offers[1]} is still active and offered to the Taker")
        maker_cli("withdraw-offer", "--request-id", f"e2e-price-withdraw-{stamp}", "--offer-id", offers[1],
                  "--expected-revision", str(kept["revision"]))
        return {"swap_id": swap_id, "offers": offers, "module_sha256": module_sha256, "quote": quote,
                "unavailable": {"module_sha256": unavailable_sha256,
                                **{method: reply["error"] for method, reply in refused.items()}}}
    finally:
        FOREIGN_UNITS, LEZ_UNITS = kept_units
        recreate_maker_with_price_module()
        subprocess.run(["docker", "exec", NODES["maker"][0], "rm", "-rf", PRICE_MODULE_DIR], check=False)


def take_after_outage(offer_id: str, stamp: str) -> tuple[str, dict]:
    """R8: Logos Chat and Delivery cut before the take (cut_delivery_and_chat,
    proven from the Taker's container; both Nodes report both unavailable and
    are degraded). The Taker's take of an offer it discovered before the cut
    is refused as dependency-unavailable with its own category,
    `authenticated_delivery_unavailable` (it cannot confirm the offer is still
    live); afterwards no swap names the offer in the Taker's list, the Maker's
    offer record and swap history are as they were, and no Chat connection
    reached the Maker. Once both are restored the same request, with the same
    request id, takes the offer."""
    entry = discover(offer_id)
    request = take_request(offer_id, entry, f"e2e-take-{stamp}")
    maker_offer = lambda: next(r for r in call("maker", "maker_offer_list", {}) if r["offer"]["id"] == offer_id)
    maker_swaps = lambda: sorted(view.get("id") for view in call("maker", "swap_history", {}))
    offer_before, swaps_before = maker_offer(), maker_swaps()
    log("  cutting Logos Chat and Delivery before the take")
    watch = None
    try:
        watch = cut_delivery_and_chat()
        reply = rpc("taker", "taker_swap_initiate_v1", request, timeout=420)
        error = reply.get("error") or {}
        category = (error.get("data") or {}).get("category") if isinstance(error.get("data"), dict) else None
        if "result" in reply or error.get("code") != -32010 or category != "authenticated_delivery_unavailable":
            raise Failure(f"the take under the outage was not refused as authenticated_delivery_unavailable: "
                          f"{json.dumps(reply)[:300]}")
        log(f"  the Taker refused the take: {error.get('message')} ({category})")
        taken = [swap for swap in call("taker", "taker_swap_list_v1", {"schema_version": 1})["swaps"]
                 if swap.get("offer_id") == offer_id]
        offer_after, swaps_after = maker_offer(), maker_swaps()
        if taken or offer_after != offer_before or swaps_after != swaps_before:
            raise Failure(f"the refused take left a swap: Taker {taken}, Maker offer {offer_before} -> {offer_after}, "
                          f"Maker history {len(swaps_before)} -> {len(swaps_after)} swaps")
        log(f"  no swap on either Node: none in the Taker's list for {offer_id}, the Maker's offer record unchanged "
            f"({offer_after['status']}, revision {offer_after['revision']}) and its history too")
        # The refusal takes about a second, less than the Chat watch's period:
        # the cut is held until it has sampled the Maker's socket twice.
        deadline = time.time() + 30
        while watch.samples < 2 and time.time() < deadline:
            time.sleep(1)
        held = assert_cut_held(watch)
    finally:
        if watch is not None:
            watch.stop()
        restore_delivery_and_chat()
    assert_restored()
    result = initiate(request)
    swap_id = result["swap"]["swap_id"]
    log(f"  restored, the same request {request['request_id']} took {offer_id}: swap {swap_id[:12]} "
        f"({result['swap']['state']}, replay={result.get('was_replay')})")
    return swap_id, {"refused": {"code": error.get("code"), "category": category}, "cut": held,
                     "maker_offer_status": offer_before["status"], "maker_offer_revision": offer_before["revision"],
                     "request_id": request["request_id"]}


def scenario_outage_before_take(stamp: str) -> dict:
    """R8: Chat and Delivery down before the take; the take is refused and
    leaves no swap, and the same take succeeds once they are back
    (take_after_outage). The swap then completes."""
    offer_id = publish_offer(stamp)
    swap_id, outage = take_after_outage(offer_id, stamp)
    lock(swap_id)
    claim(swap_id, stamp)
    wait_completed(swap_id)
    return {"swap_id": swap_id, "outage": outage}


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


# The timing profiles a refund scenario can run on (gen-config.sh): `refund`
# is the one regress.sh gives them, `fast` the one the other scenarios run on.
REFUND_PROFILES = {"refund", "fast"}


def require_fast_profile() -> dict:
    profile = timing_profile()
    if profile.get("LEZ_TIMING_PROFILE") not in REFUND_PROFILES:
        raise Failure("this scenario needs LEZ_TIMING_PROFILE=refund or fast (scripts/up.sh recreates the Nodes)")
    return profile


def request_refund_until_terminal(swap_id: str, stamp: str, timeout: int, mine_every: int = 10) -> dict:
    """The refund is a repeatable operator action: the Node answers
    `taker_action_execution_unavailable` while the actor's recovery checks are
    not yet satisfied and admits the request once they are. One request id is
    replayed throughout: a replay re-drives the admitted recovery, and the Node's
    own observer keeps driving it between requests. A block is mined between
    attempts so Bitcoin maturity and finality advance on regtest; `mine_every`
    is the pause between attempts."""
    wait = Wait(f"the Taker's refund of {swap_id[:12]} to land", timeout)
    attempt = 0
    category = None
    while wait.running():
        view = taker_view(swap_id)
        if view["state"] == "refunded":
            return view
        if view["state"] == "attention_required" or view["state"] == "completed":
            raise Failure(f"swap {swap_id[:12]} ended in {view['state']} instead of refunded")
        wait.note((view["state"], category),
                  lambda: f"taker {view['state']} (refund {category or 'admitted'}) | maker {maker_phase(swap_id)}")
        attempt += 1
        reply = rpc("taker", "taker_swap_refund_v1", {"schema_version": 1, "request_id": f"e2e-refund-{stamp}",
                                                     "swap_id": swap_id, "expected_generation": view["progress_generation"]},
                    timeout=300)
        if "result" in reply:
            category = None
            if not reply["result"].get("was_replay") or attempt % 5 == 1:
                log(f"  refund request {'replayed' if reply['result'].get('was_replay') else 'admitted'} "
                    f"(attempt {attempt}, state {view['state']})")
        else:
            category = (reply["error"].get("data") or {}).get("category")
            # progress_generation_conflict: the Node's observer advanced the
            # swap between the view and the request (it sweeps every 3 s on the
            # devnet's profiles); the next attempt reads a fresh view.
            if category not in {"taker_action_execution_unavailable", "taker_action_conflict", "taker_action_unavailable",
                                "progress_generation_conflict"}:
                raise Failure(f"taker_swap_refund_v1 failed: {json.dumps(reply['error'])[:300]}")
            if attempt % 5 == 1:
                log(f"  refund not yet accepted (attempt {attempt}: {category}); taker {view['state']} | maker: {maker_phase(swap_id)}")
        mine(1)
        time.sleep(mine_every)
    raise wait.failure()


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
    wait = Wait("the Maker to reconcile the refunded swap to a terminal state", 900)
    while wait.running():
        view = maker_view(swap_id)
        if view.get("schedule_state") == "terminal":
            log(f"  Maker reconciled to {maker_phase(swap_id)}")
            break
        wait.note(view.get("schedule_state"), lambda: f"maker {maker_phase(swap_id)} (schedule {view.get('schedule_state')})")
        mine(1)
        time.sleep(5)
    else:
        raise wait.failure()
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
    wait_until(started + earlier + 30, "for the Maker's refund deadline")
    result = maker_action_with_fresh_generation("maker_actor_refund_v1", swap_id, f"e2e-maker-refund-{stamp}")
    log(f"  Maker refund queued: {json.dumps(result)[:160]}")
    # The Maker asserts the absent lock only once its whole LEZ discovery window
    # is finalized, so a flat 15-minute wait raced the chain and failed a refund
    # the Node went on to complete. Size it from the profile's own window
    # (a LEZ block about every 2.2 s on the devnet, 3 s allowed) so the two
    # cannot drift apart again.
    wait = Wait("the Maker to refund its leg", 3 * int(profile["LEZ_LEZ_DISCOVERY_MAX_BLOCKS"]) + 600)
    while wait.running():
        phase = maker_phase(swap_id)
        if phase in {"maker_leg_refunded", "refunded"}:
            break
        wait.note(phase, f"maker {phase}")
        time.sleep(5)
    else:
        raise wait.failure()
    # The Taker's leg refunds at the later refund: its LEZ escrow's refund
    # time when it sold LEZ. Selling Bitcoin it is the CSV, mined above, and
    # the request is retried until the Node admits it, so nothing waits here.
    if REVERSE:
        wait_until(started + later + 30, "for the Taker's LEZ refund time")
    request_refund_until_terminal(swap_id, stamp, timeout=1500)
    log("  both legs refunded")
    return {"swap_id": swap_id,
            "bitcoin_refund": assert_refund_is_script_path(swap_id, required=True)}


def wait_until(unix_seconds: float, describe: str, state=None) -> None:
    """Sleeps until a moment, writing a wait line (wait_state's, or the chain's
    alone) when it starts and every WAIT_LOG_SECONDS until it ends."""
    remaining = unix_seconds - time.time()
    if remaining <= 0:
        return
    log(f"  waiting {int(remaining)}s {describe}")
    wait = Wait(describe, remaining)
    while (remaining := unix_seconds - time.time()) > 0:
        wait.note(None, state if state is not None else "sleeping")
        time.sleep(min(remaining, WAIT_LOG_SECONDS))


def wait_mempool_transaction(timeout: float) -> str:
    """The first transaction to reach Bitcoin Core's mempool. With the block
    miner stopped nothing else is broadcast, so it is the Maker's lock."""
    wait = Wait("the Maker's Bitcoin lock in the mempool (the miner held)", timeout)
    while wait.running():
        pool = bitcoin("getrawmempool")
        if isinstance(pool, list) and pool:
            return str(pool[0])
        wait.note("empty", "Bitcoin mempool empty")
        time.sleep(2)
    raise wait.failure()


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


# Taker states that can only follow an observed maker lock.
TAKER_SAW_MAKER_LOCK = {"both_legs_locked", "claim_available", "claim_in_progress", "completed"}
# A refund request the Node may answer this way for a moment: its observer moved
# the swap between the view and the request, or its recovery checks wait for the
# next sweep.
REFUND_NOT_YET = {"progress_generation_conflict", "taker_action_conflict", "taker_action_execution_unavailable"}


def assert_maker_lock_unobserved(view: dict, when: str, required_confirmations: int | None) -> None:
    """The Taker's view lists no maker lock it could rely on, and its state is
    one that comes before the second lock. A LEZ lock has no confirmations to
    count (they stay 0), so with `required_confirmations` None any maker_lock
    effect fails; a Bitcoin one fails once it has the agreement's depth."""
    if view["state"] in TAKER_SAW_MAKER_LOCK:
        raise Failure(f"the Taker was at {view['state']} {when}: it had observed the Maker's lock")
    for effect in view.get("effects") or []:
        if effect.get("kind") != "maker_lock":
            continue
        confirmations = int(effect.get("confirmations") or 0)
        if required_confirmations is None or confirmations >= required_confirmations:
            raise Failure(f"the Taker's view records the Maker's lock {str(effect.get('transaction_id'))[:12]} "
                          f"({confirmations} confirmations) {when}")


def admit_refund_unobserved(swap_id: str, stamp: str, required_confirmations: int | None) -> dict:
    """Asks for the Taker's refund until the Node admits it, asserting the
    Taker had not observed the Maker's lock when it asked and right after the
    admission. Bounded by ten of the Taker observer's sweeps, at least 60 s."""
    sweep = profile_seconds(timing_profile(), "LEZ_TAKER_OBSERVER_SWEEP_SECONDS", 15)
    wait = Wait(f"the Taker to admit the refund of {swap_id[:12]} with the Maker's lock unobserved",
                max(60, 10 * sweep))
    while wait.running():
        view = taker_view(swap_id)
        assert_maker_lock_unobserved(view, "when the refund was asked for", required_confirmations)
        reply = rpc("taker", "taker_swap_refund_v1", {
            "schema_version": 1, "request_id": f"e2e-refund-{stamp}",
            "swap_id": swap_id, "expected_generation": view["progress_generation"]})
        if "result" in reply:
            admitted = taker_view(swap_id)
            assert_maker_lock_unobserved(admitted, "right after the refund was admitted", required_confirmations)
            log(f"  refund admitted at generation {view['progress_generation']} ({view['state']} → "
                f"{admitted['state']}); the Taker's view lists no maker lock it could rely on")
            return {"asked_in_state": view["state"], "admitted_state": admitted["state"],
                    "generation": view["progress_generation"]}
        category = (reply["error"].get("data") or {}).get("category")
        if category not in REFUND_NOT_YET:
            raise Failure(f"the Taker did not admit the refund with the Maker's lock unobserved: "
                          f"{json.dumps(reply['error'])[:200]}")
        wait.note(category, f"refund not yet admitted ({category}); taker {view['state']}")
        time.sleep(3)
    raise wait.failure()


def refund_maker_leg(swap_id: str, stamp: str, mine_every: int = 0) -> str:
    """The Maker's own leg, refunded as its operator does once the agreement's
    earlier refund time has passed (maker-refund): queued, then followed to a
    refunded phase. Its LEZ refund asserts the claim's absence over the whole
    discovery window, so the wait is the profile's window in blocks at 3 s each
    plus ten minutes, as in maker-refund."""
    profile = timing_profile()
    terms = maker_view(swap_id).get("terms") or taker_view(swap_id)["terms"]
    wait_until(int(terms["earlier_refund_latest_unix_seconds"]) + 30, "for the Maker's refund time")
    if maker_view(swap_id).get("schedule_state") != "terminal":
        try:
            result = maker_action_with_fresh_generation("maker_actor_refund_v1", swap_id, f"e2e-maker-refund-{stamp}")
            log(f"  Maker refund queued: {json.dumps(result)[:160]}")
        except Failure as error:
            if "conflicts with durable state" not in str(error):
                raise
            log("  the Maker's refund request is already recorded; continuing")
    wait = Wait("the Maker to refund its own leg", 3 * int(profile["LEZ_LEZ_DISCOVERY_MAX_BLOCKS"]) + 600)
    while wait.running():
        phase = maker_phase(swap_id)
        if phase in {"maker_leg_refunded", "refunded"}:
            return phase
        wait.note(phase, f"maker {phase}")
        if mine_every:
            mine(1)
        time.sleep(mine_every or 5)
    raise wait.failure()


def scenario_refund_admitted_forward(stamp: str) -> dict:
    """The forward mirror of scenario_refund_admitted_then_maker_lock: here the
    Taker locks Bitcoin first and the second lock, the one the observer has to
    project after a refund is already admitted, is the Maker's LEZ lock.

    The Maker locks on time and its lock finalizes on time; only the Taker's view
    of LEZ is cut, so the Taker reaches its cutoff having observed no maker lock,
    routes to recovery and admits a refund. That it had not observed the lock is
    asserted on its view when the refund is asked for and right after it is
    admitted (admit_refund_unobserved: no maker_lock effect, a state before the
    second lock); a Node that will not admit the refund without LEZ fails the
    scenario. Restoring the view then presents the observer with exactly the
    reported condition — an admitted refund and a canonical, timely, unprojected
    maker lock — and it must finish the swap on its own. The Maker then refunds
    its LEZ, and both parties' balances on both chains must be what they were
    before the take, less the Taker's two Bitcoin fees (assert_settlement)."""
    require_fast_profile()
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    terms = taker_view(swap_id)["terms"]
    cutoff = int(terms["maker_second_lock_cutoff_unix_seconds"])
    before = party_balances()
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
        wait_until(cutoff + 30, "for the cutoff to pass with the Taker still blind to LEZ",
                   lambda: f"taker {taker_view(swap_id)['state']} | maker {maker_phase(swap_id)}")
        admission = admit_refund_unobserved(swap_id, stamp, required_confirmations=None)
    finally:
        restore_lez_view("taker")
    wait_lez_view("taker", restored=True)
    log("  the Taker's view of LEZ is back; the maker lock is now canonical and unprojected")
    mine(int(require_fast_profile()["LEZ_BTC_REFUND_CSV_BLOCKS"]) + 1)
    # Hand off to the Node observer alone. Do NOT drive the actor from here; only
    # wait (mining to advance regtest finality) for it to reach refunded. The
    # Taker's Bitcoin refund window is its CSV, just mined; the later refund
    # time is a wall-clock bound only a LEZ refund has.
    wait = Wait(f"the Taker's observer to carry the admitted refund of {swap_id[:12]} to refunded", 1800)
    while wait.running():
        state = taker_view(swap_id)["state"]
        if state == "refunded":
            break
        if state in ("completed", "attention_required"):
            raise Failure(f"swap {swap_id[:12]} ended in {state}, not refunded")
        wait.note(state, lambda: f"taker {state} | maker {maker_phase(swap_id)}")
        mine(1)
        time.sleep(5)
    else:
        raise wait.failure()
    log("  observer recovered the swap unaided; the Maker refunds its LEZ lock")
    maker_phase_end = refund_maker_leg(swap_id, stamp)
    settlement = assert_settlement(before, "refunded", "both legs refunded")
    return {"swap_id": swap_id, "taker_lock_txid": taker_lock, "maker_lock": maker_lock, "cutoff": cutoff,
            "refund_admission": admission, "maker_terminal_phase": maker_phase_end, **settlement}


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
    unaided. Reversed, the Taker may see the held lock in the mempool, so what is
    asserted at the admission is that it holds no maker lock with the agreement's
    confirmations and is before the second lock; a lock whose block's median time
    is past the cutoff (the miner was stopped too late for the case) is not the
    case this scenario is about and ends the try as NotExercised. Both parties'
    balances on both chains are compared once the Maker has refunded too."""
    if not REVERSE:
        return scenario_refund_admitted_forward(stamp)
    profile = require_fast_profile()
    offer_id = publish_offer(stamp)
    swap_id, _ = take(offer_id, stamp)
    terms = taker_view(swap_id)["terms"]
    cutoff = int(terms["maker_second_lock_cutoff_unix_seconds"])
    opens = int(terms["later_refund_earliest_unix_seconds"])
    required = int(terms.get("required_bitcoin_confirmations") or 1)
    before = party_balances()
    # Stop the miner, not the Maker: the Maker broadcasts its Bitcoin lock on
    # time (once the Taker's LEZ lock finalizes), but held out of a block it
    # stays unconfirmed, so the Taker never observes a maker lock and routes to
    # recovery at the cutoff. Confirming it afterwards makes it present and
    # timely-by-median — exactly the wild condition (a slow inclusion), which a
    # Maker held down past its own cutoff cannot reproduce (it refuses to lock).
    log("  stopping the miner so the Maker's lock is broadcast on time but not yet confirmed")
    docker("stop", MINER)
    miner_down = True
    try:
        lock(swap_id)
        maker_lock = wait_mempool_transaction(timeout=max(60, cutoff - int(time.time()) - 30))
        log(f"  Maker's Bitcoin lock {maker_lock[:12]} is broadcast (in mempool), unconfirmed")
        wait_until(cutoff + 30, "for the cutoff to pass with the maker lock still unconfirmed",
                   lambda: f"taker {taker_view(swap_id)['state']} | maker {maker_phase(swap_id)}")
        admission = admit_refund_unobserved(swap_id, stamp, required_confirmations=required)
        [block_hash] = bitcoin("generatetoaddress", "1", MINER_ADDRESS)
        blk = bitcoin("getblock", block_hash, "1")
        if maker_lock not in blk["tx"]:
            raise Failure(f"block {block_hash[:12]} did not include the maker lock {maker_lock[:12]}")
        if int(blk["mediantime"]) > cutoff:
            raise NotExercised(f"the maker lock confirmed with block median {blk['mediantime']}, past the cutoff "
                               f"{cutoff}: late by median time, not the timely-but-unobserved lock this tests")
        log(f"  maker lock confirmed; block median {blk['mediantime']} vs cutoff {cutoff} (timely by median)")
        mine(int(profile["LEZ_BTC_REFUND_CSV_BLOCKS"]) + 1)
        # Hand off to the Node observer alone. Do NOT drive the actor from here;
        # only wait (mining to advance regtest finality) for it to reach refunded.
        wait_until(opens + 5, "for the Taker's LEZ refund window to open",
                   lambda: f"taker {taker_view(swap_id)['state']} | maker {maker_phase(swap_id)}")
        wait = Wait(f"the Taker's observer to carry the admitted refund of {swap_id[:12]} to refunded", 1800)
        while wait.running():
            st = taker_view(swap_id)["state"]
            if st == "refunded":
                break
            if st in ("completed", "attention_required"):
                raise Failure(f"swap {swap_id[:12]} ended in {st}, not refunded")
            wait.note(st, lambda: f"taker {st} | maker {maker_phase(swap_id)}")
            mine(1)
            time.sleep(5)
        else:
            raise wait.failure()
        log("  observer recovered the swap unaided; the Maker refunds its Bitcoin lock")
    finally:
        if miner_down:
            docker("start", MINER)
    maker_phase_end = refund_maker_leg(swap_id, stamp, mine_every=5)
    settlement = assert_settlement(before, "refunded", "both legs refunded")
    return {"swap_id": swap_id, "maker_lock_txid": maker_lock, "cutoff": cutoff, "median_time": int(blk["mediantime"]),
            "refund_admission": admission, "maker_terminal_phase": maker_phase_end, **settlement}


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
    # A lane re-renders its own files only (gen-config.sh with LEZ_LANE set).
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
# recreates both Nodes). The Taker Node supervises its ZEC actor itself (the
# activation, the first LEZ lock and every observation up to claim_available);
# the harness only polls the Nodes and reads the actor's phase with
# `lez-zec-taker-actor … status`. Zebra has no miner: blocks are generated here, one
# when something waits in its mempool and one when the Taker reports it is
# waiting for safe funding depth (the pre-reveal gate wants two confirmations),
# never otherwise, because the agreement's Zcash refund height is absolute.

ZEC_CORRIDOR: dict = {}


def zebra_headers() -> dict:
    headers = {"content-type": "application/json"}
    if os.environ.get("LEZ_ZEBRA_API_KEY_FILE"):
        headers["x-api-key"] = pathlib.Path(os.environ["LEZ_ZEBRA_API_KEY_FILE"]).read_text().strip()
    elif os.environ.get("LEZ_ZEBRA_COOKIE_FILE"):
        import base64
        cookie = pathlib.Path(os.environ["LEZ_ZEBRA_COOKIE_FILE"]).read_text().strip()
        headers["authorization"] = "Basic " + base64.b64encode(cookie.encode()).decode()
    return headers


def zebra(method: str, params: list | None = None) -> object:
    import urllib.request
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or []}).encode()
    request = urllib.request.Request(ZEBRA_URL, data=body, headers=zebra_headers())
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            reply = json.load(response)
    except OSError as error:
        raise Failure(f"zebra {method} unreachable: {error}") from error
    if reply.get("error"):
        raise Failure(f"zebra {method} failed: {json.dumps(reply['error'])[:200]}")
    return reply["result"]


def zebra_mine(blocks: int, why: str) -> None:
    if ZEC_PUBLIC:
        zebra_wait_blocks(blocks, why)
        return
    zebra("generate", [blocks])
    log(f"  zebra: mined {blocks} block(s) → height {zebra('getblockcount')} ({why})")


def zebra_wait_blocks(blocks: int, why: str) -> None:
    """The public lane's zebra_mine: the chain makes its own blocks."""
    start = int(zebra("getblockcount"))
    deadline = time.time() + blocks * ZEC_BLOCK_WAIT_SECONDS
    log(f"  zebra at {start}: waiting for {blocks} Testnet block(s), never mining ({why})")
    while (height := int(zebra("getblockcount"))) < start + blocks:
        if time.time() > deadline:
            raise Failure(f"Zcash Testnet made {height - start} of {blocks} block(s) in {blocks * ZEC_BLOCK_WAIT_SECONDS}s ({why})")
        time.sleep(15)
    log(f"  zebra: Testnet reached height {height} ({why})")


def zec_timeout(seconds: int) -> int:
    """A ZEC wait's length: its devnet length, scaled on the public lane."""
    return seconds * ZEC_TIMEOUT_SCALE


ZEC_CORRIDORS: list[dict] = []


def zec_profile_refund_delay(profile: str) -> int:
    """The LEZ refund delay of a ZEC profile, seconds (zec-swap-sdk profile.rs:
    the only parameter the harness waits on)."""
    return {"deterministic-local-devnet-v1": 1800, "deterministic-local-devnet-v2": 1800,
            "deterministic-local-devnet-v3": 720, "deterministic-local-devnet-v4": 240,
            "deterministic-local-devnet-v5": 720, "deterministic-local-devnet-v6": 240,
            "public-testnet-v2": 7200}.get(profile, 60)


# The ZEC profile of the refund journeys (zec-swap-sdk profile.rs): nobody claims
# there, so the LEZ refund delay only has to outlast provisioning, the take and
# both first locks. Every other scenario keeps zec-provision.sh's default (-v5).
# Both are NU7 profiles: the devnet's Zebra 7 runs NU7 from height 1.
# The public lane has one profile for every scenario (zec-provision.sh
# --zcash-network test: public-testnet-v2), refund journeys included.
ZEC_REFUND_PROFILE = None if ZEC_PUBLIC else "deterministic-local-devnet-v6"
# The profiles whose corridors sign for NU7, the branch every Zebra the
# harness drives runs (Zebra 7 Regtest; public Testnet).
ZEC_NU7_PROFILES = {"deterministic-local-devnet-v5", "deterministic-local-devnet-v6", "public-testnet-v2"}


# How a Zcash scenario's agreement is made, read here only (LEZ_ZEC_AGREEMENT):
# `dealer`, zec-provision.sh signs it for both parties and recreates the Nodes;
# `take-time`, the Maker publishes an offer on its running Node and the Nodes
# compose the agreement when the scenario takes it (zec_reserve_v1). What the
# take-time path cannot serve stays on the dealer, and the log says why: the
# public lane and the scenarios that test the static catalog itself. A
# refund-profile corridor needs the Nodes on that profile (LEZ_ZEC_PROFILE,
# regress.sh's ZEC profile group).
ZEC_AGREEMENT = os.environ.get("LEZ_ZEC_AGREEMENT", "dealer")
ZEC_CATALOG_SCENARIOS = {"zec-regenerated-catalog", "zec-tampered-catalog"}
SCENARIO = ""  # the scenario run() is running


def zec_current_refund_delay() -> int:
    """The LEZ refund delay of the corridor the Nodes run with (runtime/zec/current)."""
    run_id = (LANE_RUNTIME / "zec" / "current").read_text().strip()
    spec = json.loads((LANE_RUNTIME / "zec" / run_id / "spec.json").read_text())
    return zec_profile_refund_delay(spec.get("profile", "deterministic-local-v1"))


def zec_provision(swaps: int = 1, swap_id: str | None = None, add: bool = False, profile: str | None = None) -> dict:
    """One fresh corridor run: zec-provision.sh end to end, Nodes recreated. With
    `swaps` 2 the run holds two prepared entries at one Zcash anchor; with `add`
    a second corridor ("-b": its own root, agreement, offer, sidecar ports,
    funder output and Zcash anchor) is provisioned beside the live one and the
    catalog lists both — the concurrent fixture, whose second swap needs a
    refund window of its own. `profile` names the agreement's ZEC profile
    (zec-provision.sh --profile; its default when None). Returns the first entry's corridor facts, or the
    entry of `swap_id` when the corridor is reused; every entry is kept in
    ZEC_CORRIDORS.

    With LEZ_ZEC_AGREEMENT=take-time the corridor is an offer the Maker
    publishes on its running Node (zec_take_time_offers); its agreement facts
    are filled in after the scenario's own take (zec_taker_actor_config)."""
    reuse = os.environ.get("LEZ_ZEC_REUSE_CORRIDOR") == "1"
    if reuse and swap_id and taker_has_take_time_swap(swap_id):
        # A swap agreed at take time (a desk's Zcash offer) has no corridor: its
        # facts are in the Taker's swap directory.
        return zec_dynamic_corridor(swap_id, "")
    if not reuse and ZEC_AGREEMENT == "take-time":
        why = ("the public lane" if ZEC_PUBLIC else
               f"{SCENARIO} tests the static catalog" if SCENARIO in ZEC_CATALOG_SCENARIOS else
               f"the Nodes take under {ZEC_DYNAMIC_PROFILE}, not {profile}" if profile and profile != ZEC_DYNAMIC_PROFILE
               else None)
        if why is None:
            return zec_take_time_offers(swaps, add)
        log(f"  take-time agreement not used here ({why}): the dealer provisions this corridor")
    if reuse:
        # The corridor the Nodes already run with (runtime/zec/current), for a
        # run that must not recreate them; its catalog entry must be untaken.
        log("  reusing the provisioned corridor the Nodes already loaded (LEZ_ZEC_REUSE_CORRIDOR=1)")
        # A corridor from before the devnet's Zebra moved to NU7 names NU6.2
        # and outputs of a chain that is gone: say so instead of a stuck swap.
        live_run = (LANE_RUNTIME / "zec" / "current").read_text().strip()
        live_profile = json.loads((LANE_RUNTIME / "zec" / live_run / "spec.json").read_text()).get(
            "profile", "deterministic-local-v1")
        if live_profile not in ZEC_NU7_PROFILES:
            raise Failure(f"the live corridor {live_run} was provisioned under {live_profile} (NU6.2), but the "
                          "devnet's Zebra runs NU7: provision a new one (run without LEZ_ZEC_REUSE_CORRIDOR)")
    else:
        log(f"  provisioning a ZEC corridor {'beside the live one' if add else f'with {swaps} prepared swap(s)'} "
            "(zec-provision.sh; this recreates both Nodes)")
        result = subprocess.run(["bash", str(DEPLOY_ROOT / "scripts" / "zec-provision.sh"), "--skip-chain-check",
                                 "--direction", ROUTE["direction"], "--swaps", str(swaps), *(["--add"] if add else []),
                                 *(["--profile", profile] if profile else []),
                                 *(["--zcash-network", "test"] if ZEC_PUBLIC else [])],
                                capture_output=True, text=True, check=False, cwd=DEPLOY_ROOT, stdin=subprocess.DEVNULL)
        for line in result.stdout.strip().splitlines()[-6:]:
            log(f"    {line}")
        if result.returncode == 3:
            # The provisioner cannot build a corridor for this network: not a
            # failed swap, a missing capability (public-zec-testnet.sh: blocked).
            log(f"  zec-provision.sh: blocked: {result.stderr.strip()[-400:]}")
            raise SystemExit(3)
        if result.returncode != 0:
            raise Failure(f"zec-provision.sh failed: {result.stderr.strip()[-300:] or result.stdout.strip()[-300:]}")
    run_id = (LANE_RUNTIME / "zec" / "current").read_text().strip()
    out = LANE_RUNTIME / "zec" / run_id
    entries = json.loads((out / "prepared.json").read_text())
    corridors = []
    for index, entry in enumerate(entries):
        # Entry k of the run: its files under runtime/zec/<run>/ (k = 0) or
        # runtime/zec/<run>/b/, its corridor root corridor/ or corridor-b/.
        entry_out = out if index == 0 else out / "b"
        corridor_root = "/var/lib/lez/zec/corridor" + ("" if index == 0 else "-b")
        summary = json.loads((entry_out / "provision-summary.json").read_text())
        # The Zcash funder's actor config names the outpoint its funding spends
        # (the Maker's coinbase when the Taker sells LEZ, the Taker's paid output
        # when it sells ZEC); it is the one fact that lets the scenario find the
        # funding and its spend on zebra. The summary says which role funds.
        funder = summary.get("zcash_candidate_owner", "maker")
        reader = (f"import json,sys; c=json.load(open(sys.argv[1] + '/{funder}/actor-config.json'));"
                  " print(json.dumps(c['zcash_funding_outpoints']))")
        read = subprocess.run(["docker", "exec", NODES[funder][0], "python3", "-c", reader, corridor_root],
                              capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL)
        # A desk journey may have the funder's Node stopped (zec-taker-refund):
        # the outpoint read once is kept beside the run's summaries and reused.
        cached = entry_out / "funding-outpoints.json"
        if read.returncode == 0 and read.stdout.strip():
            cached.write_text(read.stdout)
        elif cached.is_file():
            read = subprocess.CompletedProcess(read.args, 0, cached.read_text(), "")
        else:
            raise Failure(f"could not read the {funder}'s funding outpoint for entry {index}: {read.stderr.strip()[:200]}")
        spec = json.loads((entry_out / "spec.json").read_text())
        profile = spec.get("profile", "deterministic-local-v1")
        corridors.append({"run_id": run_id, "entry": index, "offer_id": entry["offer_id"], "catalog_swap_id": entry["swap_id"],
                          "profile": profile,
                          # zec-swap-sdk profile.rs: the only parameter the devnet profile changes.
                          "lez_refund_delay_seconds": zec_profile_refund_delay(profile),
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
    """The take wrote an acceptance receipt naming the provisioned Taker actor config.
    A take-time corridor's receipt is known once its offer is taken: its facts
    are read then and filled into the corridor the scenario holds."""
    if receipt_output.startswith("take-time:"):
        offer_id = receipt_output.split(":", 1)[1]
        swaps = [s for s in call("taker", "taker_swap_list_v1", {"schema_version": 1})["swaps"]
                 if s.get("offer_id") == offer_id]
        if not swaps:
            raise Failure(f"the take-time offer {offer_id} was not taken")
        swaps.sort(key=lambda s: s["state"] in TERMINAL)
        facts = zec_dynamic_corridor(swaps[0]["swap_id"], offer_id)
        for corridor in ZEC_CORRIDORS:
            if corridor["offer_id"] == offer_id:
                corridor.update({key: value for key, value in facts.items() if key != "entry"})
        receipt_output = facts["receipt_output"]
    result = subprocess.run(["docker", "exec", NODES["taker"][0], "cat", receipt_output],
                            capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL)
    if result.returncode != 0:
        raise Failure(f"no acceptance receipt at {receipt_output}: {result.stderr.strip()[:200]}")
    receipt = json.loads(result.stdout)
    log(f"  receipt: swap {receipt.get('swap_id')}, actor config {receipt.get('actor_config_file')}")
    return receipt["actor_config_file"]


# The Taker Node supervises its ZEC actor (activation, the LEZ lock, the
# observation of the Maker's legs); the harness never drives it on a happy
# path. ZEC_MANUAL_DRIVE=1 adds one hand drive per poll, for diagnosis of a
# Node whose supervisor is suspected, and the log says so.
MANUAL_DRIVE = os.environ.get("ZEC_MANUAL_DRIVE") == "1"


# The ZEC lane in words, per direction. Selling LEZ the Taker locks LEZ first,
# the Maker funds the Zcash HTLC and claims LEZ with the secret (revealing it),
# and the Taker's Zcash claim completes the swap. Selling ZEC the Taker funds
# the Zcash HTLC first, the Maker locks LEZ, the Taker's LEZ claim reveals the
# secret and the Maker's follow-up Zcash claim completes the swap. Either way
# the Node offers the Taker exactly one claim, and the Zcash funding and its
# spend are what the harness checks on zebra.
def zec_taker_sells_lez() -> bool:
    return ROUTE["direction"] == "TakerSellsLez"


def zec_zcash_funder() -> str:
    return "Maker" if zec_taker_sells_lez() else "Taker"


def zec_zcash_claimant() -> str:
    return "Taker" if zec_taker_sells_lez() else "Maker"


def zec_taker_claims() -> str:
    return "Zcash" if zec_taker_sells_lez() else "LEZ"


def zec_flow() -> str:
    return ("(Taker LEZ lock → Maker Zcash funding → Maker LEZ claim)" if zec_taker_sells_lez()
            else "(Taker Zcash funding → Maker LEZ lock → the Taker's revealing LEZ claim offered)")


def zec_poll_effect(config: str) -> dict | None:
    """The hand drive of one poll when ZEC_MANUAL_DRIVE=1; None otherwise."""
    return zec_drive(config) if MANUAL_DRIVE else None


def zec_actor_phase(config: str) -> str | None:
    """The Taker actor's durable phase, read without driving it."""
    status = zec_status(config)
    if not status or status.get("state") != "active":
        return None
    return status.get("phase")


def zec_take_or_resume(offer_id: str, stamp: str, suffix: str = "") -> str:
    """A prepared entry is one take: when this offer was already taken (a rerun
    after a harness failure), the existing swap is resumed instead. `suffix`
    keeps the take request ids of one scenario's several takes apart."""
    for swap in call("taker", "taker_swap_list_v1", {"schema_version": 1})["swaps"]:
        if swap.get("offer_id") == offer_id and swap["state"] not in TERMINAL:
            log(f"  resuming swap {swap['swap_id'][:12]} already taken for {offer_id} ({swap['state']})")
            return swap["swap_id"]
    swap_id, _ = take(offer_id, stamp, suffix)
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


def zec_drive_exit(config: str) -> tuple[int, str]:
    """One `drive` of the Taker's ZEC actor, with its exit code and stderr: the
    balance refusal is exit 3 (`LEZ_DEPOSITOR_BALANCE_INSUFFICIENT_EXIT_CODE`)."""
    result = subprocess.run(["docker", "exec", NODES["taker"][0], "/usr/local/bin/lez-zec-taker-actor",
                             "--config", config, "drive"],
                            capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL, timeout=300)
    return result.returncode, result.stderr


def lez_account_balance(account_id: str) -> int:
    """The LEZ indexer's balance of an account, over its published loopback port."""
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getAccount", "params": [account_id]}).encode()
    request = urllib.request.Request(STACK_INDEXER_URL, data=body, headers={"content-type": "application/json"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return int(json.load(response)["result"]["balance"])


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


def zec_replans(config: str) -> list[dict]:
    """The re-plans of the Taker actor's LEZ first lock (D1): one row per
    supersession with both nonces and both transaction ids, from the actor's
    status (`lez_first_lock_replans`, absent when none)."""
    status = zec_status(config) or {}
    return list(status.get("lez_first_lock_replans") or [])


def zec_log_replans(swap_id: str, config: str) -> list[dict]:
    replans = zec_replans(config)
    if not replans:
        log(f"  {swap_id[:12]}: the Taker re-planned its LEZ lock 0 times")
    for replan in replans:
        log(f"  {swap_id[:12]}: LEZ {replan.get('step')} re-plan {replan.get('replan_index')}: nonce "
            f"{replan.get('superseded_nonce')} → {replan.get('replanned_nonce')}, tx "
            f"{str(replan.get('superseded_transaction_id'))[:12]} → {str(replan.get('replanned_transaction_id'))[:12]}")
    return replans


def stamp_time(stamp_or_run_id: str) -> int:
    """The second a stamp was made in. A stamp is that second in ten digits,
    followed on lanes other than the first by the two-digit lane number that
    keeps ids apart on the shared LEZ chain; a run id ends in its stamp."""
    return int(stamp_or_run_id.rsplit("-", 1)[-1][:10])


def maker_effect_lines(swap_id: str, since: str) -> list[str]:
    """The Maker Node's effect lines for one swap, in order:
    `<command> → <outcome> <operation> (phase …, revision …, next …)`. Read
    from the Node's own log file (LEZ_NODE_LOG_FILE, compose.yaml): the
    container's output carries every module of the role's Basecamp, and
    Logos Delivery's lines split the Node's or rotate them away. A Node
    without the file (an older image) is read from the container's log since
    `since` (RFC 3339)."""
    container = NODES["maker"][0]
    result = subprocess.run(["docker", "exec", container, "cat", "/run/lez/maker/node.log"],
                            capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL, timeout=120)
    text = result.stdout
    if result.returncode != 0:
        result = subprocess.run(["docker", "logs", "--since", since, container],
                                capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL, timeout=120)
        text = result.stdout + result.stderr
    prefix = f"maker actor {swap_id}: "
    lines = []
    for line in text.splitlines():
        if prefix in line and " → " in line:
            lines.append(line.split(prefix, 1)[1].strip())
    return lines


def maker_funding_attempts(swap_id: str, since: str) -> tuple[int, int]:
    """(funding attempts, re-projections): the Maker's drives at
    taker_lock_confirmed up to and including the one that submitted the Zcash
    funding, and how many times it projected the Taker's first lock. D2 was a
    Maker that re-projected the same lock on every attempt and never funded."""
    attempts = 0
    reprojections = 0
    # The Maker's lock is the Zcash funding when the Taker sells LEZ and the
    # LEZ escrow funding (after its initialization) when the Taker sells ZEC.
    locked = "submitted zcash_fund" if zec_taker_sells_lez() else "submitted lez_fund"
    # A step resubmitted while it waits for inclusion (the same bytes, no
    # re-plan logged) is the designed idempotent recovery, not a new attempt:
    # each submitted operation counts once (2026-10-31, V3: lez_initialize and
    # lez_fund each went out twice on a lone swap).
    submitted: set[str] = set()
    for line in maker_effect_lines(swap_id, since):
        if "projected taker_first_lock" in line:
            reprojections += 1
        if line.startswith("drive → ") and "(phase taker_lock_confirmed" in line:
            operation = line.split(" (phase", 1)[0]
            if "→ submitted " in operation and "re-planned" not in line:
                if operation in submitted:
                    continue
                submitted.add(operation)
            attempts += 1
            if locked in line:
                break
    return attempts, reprojections


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


# The ZEC waits read both Nodes every ZEC_POLL_SECONDS and mine what waits in
# zebra's mempool at once; a block for depth (zec_mining_policy) comes at most
# every ZEC_DEPTH_SECONDS, the cadence it always had: the actor's phase, which
# says whether depth is still wanted, moves on the Taker Node's own tick, and
# a faster cadence would spend blocks of the Zcash refund window on nothing.
ZEC_POLL_SECONDS = 3
ZEC_DEPTH_SECONDS = 10


def zec_mine_pending() -> None:
    if ZEC_PUBLIC:
        return  # Testnet miners include it
    pool = zebra("getrawmempool")
    if isinstance(pool, list) and pool:
        zebra_mine(1, f"mempool had {[t[:12] for t in pool]}")


def zec_mining_policy(phase: str | None, corridor: dict) -> None:
    """Mines at most one zebra block per poll, and only for a reason; `phase` is
    the Taker actor's durable phase (read, not driven). On the public lane
    depth comes from Testnet's own blocks."""
    if ZEC_PUBLIC:
        return
    height = int(zebra("getblockcount"))
    pool = zebra("getrawmempool")
    if isinstance(pool, list) and pool:
        zebra_mine(1, f"mempool had {[t[:12] for t in pool]}")
        return
    # With both legs locked the Maker reveals the secret only once its Zcash
    # funding is two deep (zcash_claim_context.rs MIN_PRE_REVEAL); the actor's
    # phase says nothing about that, so the depth is read from zebra itself.
    if phase == "both_legs_locked":
        confirmations = zec_funding_confirmations(corridor)
        if confirmations is not None and confirmations < 2 and height + 1 < corridor["zcash_refund_height"]:
            zebra_mine(1, f"Maker's funding has {confirmations} confirmation(s); the pre-reveal gate wants 2")
            return
    if not zec_taker_sells_lez():
        # Selling ZEC the funding is the Taker's own, in a block from the
        # start; past the two pre-reveal confirmations nothing waits on zebra
        # depth (the Maker locks LEZ, the Taker claims LEZ), and every block
        # mined here shortens the window the Maker's follow-up Zcash claim
        # needs (two swaps of one corridor share it).
        if phase in {"taker_lock_confirmed", "both_legs_locked"}:
            confirmations = zec_funding_confirmations(corridor)
            if confirmations is not None and confirmations < 2 and height + 1 < corridor["zcash_refund_height"]:
                zebra_mine(1, f"Taker's funding has {confirmations} confirmation(s); the pre-reveal gate wants 2")
        return
    if phase in {"awaiting_maker_confirmations", "taker_lock_confirmed", "both_legs_locked"}:
        # Depth is mined only for a funding that exists: before the Maker has
        # funded there is nothing to bury, and every block spent here shortens
        # the Zcash refund window the Maker needs to fund safely at all.
        if zec_funding_confirmations(corridor) is None:
            if not corridor.get("_logged_waiting_for_funding"):
                log(f"  zebra at {height}: not mining, the Maker's Zcash funding is not out yet")
                corridor["_logged_waiting_for_funding"] = True
        elif height + 1 < corridor["zcash_refund_height"]:
            zebra_mine(1, f"Taker waits for Zcash funding depth (actor at {phase})")
        else:
            log(f"  zebra at {height}: not mining, the refund height {corridor['zcash_refund_height']} is next")


def zec_wait_until(swap_id: str, config: str, states: set[str], timeout: int, corridor: dict,
                   describe: str = "", mine_pending_only: bool = False) -> dict:
    """Polls both Nodes until the Taker's state is in `states`; the Taker Node's
    supervisor drives the actor meanwhile, the harness only mines zebra for a
    reason. With `mine_pending_only` a zebra block is generated only to include
    what waits in the mempool, never for depth: the Maker's funding then stays
    at one confirmation, below the pre-reveal gate, which is the refund
    journeys' setup."""
    timeout = zec_timeout(timeout)
    wait = Wait(f"the Taker's {swap_id[:12]} to reach {'/'.join(sorted(states))} {describe}".rstrip(), timeout)
    next_depth = 0.0
    while wait.running():
        effect = zec_poll_effect(config)
        phase = (effect or {}).get("phase") or zec_actor_phase(config)
        view = taker_view(swap_id)
        state = view["state"]
        reason = view.get("attention_reason") or view.get("last_failure_class")
        described = zec_describe(effect) if effect else f"actor at {phase or '-'}"
        wait.note((state, described, reason),
                  lambda: f"taker {state} (gen {view.get('progress_generation')}{', ' + reason if reason else ''}) "
                          f"| {described} | maker {maker_phase(swap_id)} | zebra {zebra('getblockcount')}")
        if state in states:
            # Selling ZEC, the Maker may lock LEZ while the Taker's funding is
            # one deep, and the Node then offers the claim before the actor's
            # pre-reveal gate (two confirmations) lets it run. A caller that
            # mines nothing afterwards (zec-survivor stops the Taker right after
            # its claim) would leave that claim refused and never retried.
            if (state == "claim_available" and not zec_taker_sells_lez() and not ZEC_PUBLIC
                    and not mine_pending_only):
                confirmations = zec_funding_confirmations(corridor)
                if (confirmations is not None and confirmations < 2
                        and int(zebra("getblockcount")) + 1 < corridor["zcash_refund_height"]):
                    zebra_mine(1, f"Taker's funding has {confirmations} confirmation(s); the pre-reveal gate wants 2")
            return view
        if state in TERMINAL:
            raise Failure(f"swap {swap_id[:12]} ended in {state}{' (' + reason + ')' if reason else ''} "
                          f"while waiting for {sorted(states)} {describe}")
        if mine_pending_only or time.time() < next_depth:
            zec_mine_pending()
        else:
            zec_mining_policy(phase, corridor)
            next_depth = time.time() + ZEC_DEPTH_SECONDS
        time.sleep(ZEC_POLL_SECONDS)
    raise wait.failure()


def scenario_zec_happy(stamp: str) -> dict:
    since = datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    corridor = zec_provision()
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    return zec_happy_after_take(stamp, since, swap_id, corridor)


def zec_happy_after_take(stamp: str, since: str, swap_id: str, corridor: dict) -> dict:
    """The happy ZEC swap from a take on: both locks, the one claim, completion
    on both Nodes and the Maker's restart after it."""
    config = zec_taker_actor_config(corridor["receipt_output"])
    outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
    # The Taker's first lock, the Maker's lock and (selling LEZ) its revealing
    # LEZ claim, all driven and observed by the Taker Node's supervisor; the
    # Node then offers the Taker its one claim.
    view = zec_wait_until(swap_id, config, {"claim_available"}, timeout=2400, corridor=corridor,
                           describe=zec_flow())
    funding = zec_spend_of(outpoint, corridor["zebra_tip_at_provisioning"] + 1)
    if funding is None:
        raise Failure("claim_available, but no zebra block spends the provisioned coinbase outpoint")
    log(f"  {zec_zcash_funder()}'s Zcash funding {funding[0]} is in zebra block {funding[1]}")
    result = call("taker", "taker_swap_claim_v1", {"schema_version": 1, "request_id": f"e2e-claim-{stamp}",
                                                   "swap_id": swap_id, "expected_generation": view["progress_generation"]},
                  timeout=300)
    log(f"  Taker claimed {zec_taker_claims()} (replay={result.get('was_replay')})")
    # The Zcash claim (the Taker's, or the Maker's follow-up after the Taker's
    # revealing LEZ claim) sits in zebra's mempool until a block is generated;
    # the actors observe it on their next drive and the swap completes on both Nodes.
    zec_wait_until(swap_id, config, {"completed"}, timeout=900, corridor=corridor, describe="(claim inclusion)")
    claim_tx = zec_spend_of((funding[0], 0), funding[1])
    if claim_tx is None:
        raise Failure("the Taker reports completed, but no zebra block spends the funding output")
    log(f"  {zec_zcash_claimant()}'s Zcash claim {claim_tx[0]} is in zebra block {claim_tx[1]}")
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
    # D1 acceptance on a lone swap: the Maker locked in one or two attempts
    # (the second only when its first drive saw the lock before its depth;
    # selling ZEC its LEZ lock is two submissions, initialization and funding,
    # so one more) and the Taker never re-planned its LEZ lock (no other swap
    # took its nonce; selling ZEC it has none).
    maker_attempts, reprojections = maker_funding_attempts(swap_id, since)
    replans = zec_log_replans(swap_id, config)
    most = 2 if zec_taker_sells_lez() else 3
    log(f"  Maker lock attempt count {maker_attempts} (projected the Taker's lock {reprojections} time(s); "
        f"{view.get('attempt_count')} supervisor attempts over the swap); Taker LEZ re-plan count {len(replans)}")
    if maker_attempts < 1 or maker_attempts > most or reprojections != 1:
        raise Failure(f"the Maker needed {maker_attempts} lock attempts and projected the Taker's lock "
                      f"{reprojections} time(s) for a lone swap; 1–{most} and 1 expected")
    if replans:
        raise Failure(f"the Taker re-planned its LEZ lock {len(replans)} times on a lone swap; 0 expected")
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "profile": corridor["profile"], "offer_id": corridor["offer_id"],
            "maker_restart_after_completion": "completed",
            "maker_funding_attempt_count": maker_attempts, "maker_taker_lock_projections": reprojections,
            "taker_lez_replans": replans,
            "zcash_funding": {"transaction_id": funding[0], "height": funding[1]},
            "zcash_claim": {"transaction_id": claim_tx[0], "height": claim_tx[1]},
            "zcash_refund_height": corridor["zcash_refund_height"]}


# ---- the take-time Zcash agreement (zec_reserve_v1, docs/sdk-lez-zec.md) ------------
# No corridor: the Maker publishes a Zcash offer with its CLI on its running
# Node, the Taker takes it with its CLI, and the two Nodes compose the agreement
# over Chat. Only the funder's Zcash output is the harness's to provide.

# The profile the Nodes take under: node-entrypoint.sh's zec-role.json, from
# LEZ_ZEC_PROFILE (compose.yaml; regress.sh's zec-v6 group switches it).
ZEC_DYNAMIC_PROFILE = os.environ.get("LEZ_ZEC_PROFILE", "deterministic-local-devnet-v5")
ZEC_PROFILE_REFUND_BLOCKS = {"deterministic-local-devnet-v5": 8, "deterministic-local-devnet-v6": 8}


def zec_dynamic_fund_taker() -> None:
    """Selling ZEC the Taker funds from its own key. Zebra mines to the Maker's,
    so the Maker's key pays the Taker's once (zec-local-poc-transfer in the
    Taker's container, the Maker key there only for the payment), and 100
    blocks make the payment a funder output (local_poc.rs: every Regtest
    candidate waits out coinbase maturity)."""
    container = NODES["taker"][0]
    key = (LANE_RUNTIME / "lez" / "maker" / "zcash.key").read_bytes()
    subprocess.run(["docker", "exec", "-i", container, "sh", "-c", "umask 077; cat >/tmp/zec-payer.key"],
                   input=key, capture_output=True, check=True)
    try:
        paid = subprocess.run(["docker", "exec", container, "zec-local-poc-transfer",
                               "--zebra-endpoint", "http://127.0.0.1:18232",
                               "--from-zcash-key-file", "/tmp/zec-payer.key",
                               "--to-zcash-key-file", "/var/lib/lez/zec/zcash.key",
                               "--value-zatoshis", str(ZEC_FOREIGN_UNITS + 20_000)],
                              capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL)
    finally:
        subprocess.run(["docker", "exec", container, "rm", "-f", "/tmp/zec-payer.key"], check=False)
    if paid.returncode != 0:
        raise Failure(f"paying the Taker's Zcash key failed: {paid.stderr.strip()[-300:]}")
    log(f"  the Maker's key paid the Taker's: {json.loads(paid.stdout)['transaction_id'][:16]}…")
    zebra_mine(1, "the payment to the Taker's key")
    zebra_mine(100, "the Taker's payment matures into a funder output")


def taker_has_take_time_swap(swap_id: str) -> bool:
    """Whether the Taker Node agreed `swap_id` at take time (zec_reserve_v1)."""
    found = subprocess.run(["docker", "exec", NODES["taker"][0], "sh", "-c",
                            f"grep -l '\"{swap_id}\"' /var/lib/lez/taker/btc/swaps/*/zec-taker-prepared.json"],
                           capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL)
    return found.returncode == 0 and bool(found.stdout.strip())


def zec_dynamic_corridor(swap_id: str, offer_id: str) -> dict:
    """The facts zec_happy_after_take reads from a corridor, here from the Taker's
    take-time swap directory: its receipt path, the funder's committed outpoint
    and the Zcash anchor the Maker fixed. The run id ends in the LEZ anchor,
    where the agreement's LEZ refund delay starts."""
    reader = ("import glob,json,sys\n"
              "for p in glob.glob('/var/lib/lez/taker/btc/swaps/*/zec-taker-prepared.json'):\n"
              "    e=json.load(open(p))\n"
              "    if e['swap_id']!=sys.argv[1]: continue\n"
              "    r=json.load(open(p.replace('zec-taker-prepared.json','zec-taker-reserve.json')))\n"
              "    print(json.dumps({'receipt_output':e['receipt_output'],'response':r['response'],"
              "'taker_funding':r['request']['taker_funding'],'offer_id':r['request']['offer_id']}))\n")
    read = subprocess.run(["docker", "exec", NODES["taker"][0], "python3", "-c", reader, swap_id],
                          capture_output=True, text=True, check=False, stdin=subprocess.DEVNULL)
    if read.returncode != 0 or not read.stdout.strip():
        raise Failure(f"the Taker has no take-time swap {swap_id[:12]}: {read.stderr.strip()[:200]}")
    facts = json.loads(read.stdout)
    response = facts["response"]
    funding = response["maker_funding"] or facts["taker_funding"]
    anchor = int(response["zcash_anchor_height"])
    corridor = {"run_id": f"take-time-{response['lez_anchor_unix_seconds']}", "entry": 0,
                "offer_id": offer_id or facts["offer_id"], "catalog_swap_id": swap_id,
                "profile": ZEC_DYNAMIC_PROFILE,
                "lez_refund_delay_seconds": zec_profile_refund_delay(ZEC_DYNAMIC_PROFILE),
                "receipt_output": facts["receipt_output"],
                "zebra_tip_at_provisioning": anchor - 1,
                "zcash_refund_height": anchor + ZEC_PROFILE_REFUND_BLOCKS[ZEC_DYNAMIC_PROFILE],
                "funding_outpoints": [{"transaction_id": bytes(funding["transaction_id"]).hex(),
                                       "output_index": int(funding["output_index"])}]}
    log(f"  take-time agreement: {zec_zcash_funder()} funds {corridor['funding_outpoints'][0]['transaction_id'][:16]}…"
        f":{funding['output_index']}, Zcash anchor {anchor}, refund height {corridor['zcash_refund_height']}")
    ZEC_CORRIDOR.clear()
    ZEC_CORRIDOR.update(corridor)
    return corridor


def zec_dynamic_publish(stamp: str, offers: list[tuple[str, str]]) -> None:
    """The Maker's CLI enables this direction's Zcash route at the lot's price
    and publishes each (request id, offer id) on its running Node."""
    route = ["--pair", "zcash", "--direction", "taker-sells-lez" if zec_taker_sells_lez() else "taker-sells-foreign"]
    revision = lambda records: next((["--expected-revision", str(r["revision"])] for r in records
                                     if r["value"]["route"] == ROUTE), [])
    maker_cli("configure-pair", "--request-id", f"e2e-zecdyn-pair-{stamp}", *revision(maker_cli("pairs")), *route,
              "--enabled", "true", "--price-source", "local", "--minimum-foreign-units", str(ZEC_FOREIGN_UNITS),
              "--maximum-foreign-units", str(ZEC_FOREIGN_UNITS), "--offer-ttl-seconds", "3600")
    maker_cli("set-local-price", "--request-id", f"e2e-zecdyn-price-{stamp}", *revision(maker_cli("prices")), *route,
              "--lez-units-per-lot", "1", "--foreign-units-per-lot", str(ZEC_FOREIGN_UNITS // ZEC_LEZ_UNITS))
    for request_id, offer_id in offers:
        maker_cli("publish-offer", "--request-id", request_id, "--offer-id", offer_id, *route)
        log(f"  Maker CLI published {offer_id} on its running Node (no corridor)")


# Regtest coinbases halve every 816 blocks (2.08, 1.04, 0.52 ZEC …) and a lane's
# Zebra outlives the LEZ chains: past this height the Maker's key is no longer
# mined outputs a 1 ZEC lot can be funded from, nor the dealer's 2 ZEC payment
# to the Taker. Such a Zebra is renewed: a new data volume, the same genesis.
ZEBRA_WORN_HEIGHT = 1500


def zec_renew_worn_zebra() -> None:
    """Between tests (never beside a live swap): a lane's worn Zebra recreated
    empty and mined past coinbase maturity, as zec-provision.sh does."""
    if ZEC_PUBLIC:
        return
    height = int(zebra("getblockcount"))
    if height < ZEBRA_WORN_HEIGHT:
        # a renewed chain (here or by zec-provision.sh) mined past maturity
        if height < 110:
            zebra_mine(110 - height, "coinbase maturity on a new chain")
        return
    container = f"{PREFIX}-zebra"
    volume = subprocess.run(["docker", "inspect", "-f", '{{range .Mounts}}{{if eq .Destination "/var/lib/zebra"}}'
                             '{{.Name}}{{end}}{{end}}', container], capture_output=True, text=True, check=True).stdout.strip()
    log(f"  zebra at {height} is worn (coinbases halve every 816 blocks): renewing {container} with a new volume")
    compose = ["docker", "compose", *os.environ.get("LEZ_STACK_COMPOSE_ARGS", "--env-file runtime/runtime.env").split()]
    subprocess.run([*compose, "rm", "-sf", "zebra"], cwd=DEPLOY_ROOT, check=True, capture_output=True)
    subprocess.run(["docker", "volume", "rm", volume], check=True, capture_output=True)
    subprocess.run([*compose, "up", "-d", "--no-deps", "--no-build", "zebra"], cwd=DEPLOY_ROOT, check=True, capture_output=True)
    deadline = time.time() + 120
    while True:
        try:
            zebra("getblockcount")
            break
        except Failure:
            if time.time() > deadline:
                raise
            time.sleep(2)
    zebra_mine(110, "coinbase maturity on the renewed chain")


def zec_fund() -> None:
    """What a take-time Zcash swap needs before its offer: a Zebra whose
    coinbases still fund the lot and, selling ZEC, one payment to the Taker's key."""
    zec_renew_worn_zebra()
    if not zec_taker_sells_lez():
        zec_dynamic_fund_taker()


def zec_take_time_offers(swaps: int, add: bool) -> dict:
    """zec_provision's take-time path: the funder made able to fund (selling
    ZEC, one payment to the Taker's key per offer) and `swaps` offers published,
    each a corridor whose agreement facts zec_taker_actor_config fills in after
    the scenario's own take. The first publication's request id is the dealer's
    (zec-devnet-publish-<run stamp>), which zec-replay and zec-wrong-inputs reuse.
    With `add` the offer joins the live corridors."""
    stamp = str(int(time.time())) + (f"{int(os.environ['LEZ_LANE']):02d}" if os.environ.get("LEZ_LANE", "1") != "1" else "")
    if not add:
        zec_renew_worn_zebra()
    for _ in range(swaps if not zec_taker_sells_lez() else 0):
        zec_dynamic_fund_taker()
    offers = [(f"zec-devnet-publish-{stamp}{'-' + str(index) if index else ''}",
               f"offer-e2e-zectt-{stamp}{'-' + str(index) if index else ''}") for index in range(swaps)]
    zec_dynamic_publish(stamp, offers)
    corridors = [{"run_id": f"take-time-{stamp}", "entry": index, "offer_id": offer_id, "catalog_swap_id": None,
                  "profile": ZEC_DYNAMIC_PROFILE,
                  "lez_refund_delay_seconds": zec_profile_refund_delay(ZEC_DYNAMIC_PROFILE),
                  "receipt_output": f"take-time:{offer_id}", "funding_outpoints": []}
                 for index, (_, offer_id) in enumerate(offers)]
    if not add:
        ZEC_CORRIDORS.clear()
    ZEC_CORRIDORS.extend(corridors)
    return ZEC_CORRIDORS[0]


def scenario_zec_dynamic_happy(stamp: str) -> dict:
    """F9/U5: a Zcash swap with no dealer and no Node recreation. The Maker's
    CLI configures the Zcash route, sets its price and publishes an offer on
    the running Node; the Taker's CLI lists and takes it; the Nodes compose
    the agreement at take time over Chat (zec_reserve_v1), each with its own
    keys; then the happy swap runs as zec-happy's does."""
    since = datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    zec_fund()
    offer_id = f"offer-e2e-zecdyn-{stamp}"
    zec_dynamic_publish(stamp, [(f"e2e-zecdyn-publish-{stamp}", offer_id)])
    swap_id = zec_take_or_resume(offer_id, stamp)
    corridor = zec_dynamic_corridor(swap_id, offer_id)
    return {**zec_happy_after_take(stamp, since, swap_id, corridor), "take_time_agreement": True}


def zec_wait_all_until(swaps: list[tuple[str, str, dict]], states: set[str], timeout: int, describe: str = "") -> dict[str, dict]:
    """Polls several swaps in turn until every swap's Node state is in `states`;
    the Taker Node's supervisor drives both actors, the mining policy is
    applied per corridor."""
    timeout = zec_timeout(timeout)
    wait = Wait(f"{len(swaps)} swaps to reach {'/'.join(sorted(states))} {describe}".rstrip(), timeout)
    done: dict[str, dict] = {}
    last: dict[str, tuple] = {}
    while wait.running():
        for swap_id, config, corridor in swaps:
            if swap_id in done:
                continue
            effect = zec_poll_effect(config)
            phase = (effect or {}).get("phase") or zec_actor_phase(config)
            view = taker_view(swap_id)
            state = view["state"]
            reason = view.get("attention_reason") or view.get("last_failure_class")
            described = zec_describe(effect) if effect else f"actor at {phase or '-'}"
            if last.get(swap_id) != (state, described, reason):
                log(f"  taker {swap_id}: {state} (gen {view.get('progress_generation')}{', ' + reason if reason else ''}) "
                    f"| {described} | maker: {maker_phase(swap_id)} | zebra {zebra('getblockcount')}")
                last[swap_id] = (state, described, reason)
            if state in states:
                done[swap_id] = view
            elif state in TERMINAL:
                raise Failure(f"swap {swap_id[:12]} ended in {state}{' (' + reason + ')' if reason else ''} "
                              f"while waiting for {sorted(states)} {describe}")
            else:
                zec_mining_policy(phase, corridor)
        if len(done) == len(swaps):
            return done
        wait.note(tuple(sorted(last.items())), lambda: "; ".join(f"{s[:12]} {v[0]} ({v[1]})" for s, v in last.items()))
        time.sleep(10)
    raise wait.failure()


def scenario_zec_concurrent(stamp: str) -> dict:
    """ZEC analogue of `concurrent`, in the BTC shape: two prepared entries of
    one corridor run (`--swaps 2`), both taken together, both Taker actors
    driven by the Taker Node with their LEZ steps interleaving on the same role accounts,
    two Maker fundings from distinct coinbase outputs, two Taker claims, both
    completed on both Nodes. The staggered variant (A final before B starts)
    is `zec-concurrent-staggered`, a fixture, not the evidence."""
    zec_provision(swaps=2)
    corridors = list(ZEC_CORRIDORS)
    if len(corridors) != 2:
        raise Failure(f"expected two prepared entries, the corridor has {len(corridors)}")
    swaps = []
    for index, corridor in enumerate(corridors):
        swap_id = zec_take_or_resume(corridor["offer_id"], stamp, "-ab"[index:index + 1].join(["-", ""]))
        config = zec_taker_actor_config(corridor["receipt_output"])
        swaps.append((swap_id, config, corridor))
    log(f"  both taken: {swaps[0][0]} and {swaps[1][0]}; driving both Taker actors in turn")
    views = zec_wait_all_until(swaps, {"claim_available"}, timeout=3000,
                                describe="(two swaps: " + zec_flow() + ")")
    fundings = {}
    for swap_id, _, corridor in swaps:
        outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
        funding = zec_spend_of(outpoint, corridor["zebra_tip_at_provisioning"] + 1)
        if funding is None:
            raise Failure(f"{swap_id[:12]} is claim_available, but no zebra block spends its provisioned coinbase outpoint")
        fundings[swap_id] = funding
        log(f"  {zec_zcash_funder()}'s Zcash funding for {swap_id[:12]}: {funding[0]} in zebra block {funding[1]}")
    if len({fundings[s[0]][0] for s in swaps}) != 2:
        raise Failure(f"the two {zec_zcash_funder()} fundings are one transaction: the corridors shared a funder output")
    for index, (swap_id, _, _) in enumerate(swaps):
        result = call("taker", "taker_swap_claim_v1", {"schema_version": 1, "request_id": f"e2e-claim-{stamp}-{index}",
                                                       "swap_id": swap_id, "expected_generation": views[swap_id]["progress_generation"]},
                      timeout=300)
        log(f"  Taker claimed {zec_taker_claims()} on {swap_id[:12]} (replay={result.get('was_replay')})")
    zec_wait_all_until(swaps, {"completed"}, timeout=1200, describe="(two claim inclusions)")
    claims = {}
    for swap_id, _, _ in swaps:
        funding = fundings[swap_id]
        claim_tx = zec_spend_of((funding[0], 0), funding[1])
        if claim_tx is None:
            raise Failure(f"{swap_id[:12]} reports completed, but no zebra block spends its funding output")
        claims[swap_id] = claim_tx
        log(f"  {zec_zcash_claimant()}'s Zcash claim for {swap_id[:12]}: {claim_tx[0]} in zebra block {claim_tx[1]}")
        wait_completed(swap_id, timeout=600)
    # D1: two Taker locks of one signer interleave; whichever lost its nonce
    # re-planned in the same drive attempt. The lineage (both nonces, both
    # transaction ids per re-plan) is the evidence, zero re-plans is a pass too.
    replans = {swap_id: zec_log_replans(swap_id, config) for swap_id, config, _ in swaps}
    for swap_id, rows in replans.items():
        for row in rows:
            if int(row.get("replanned_nonce", 0)) <= int(row.get("superseded_nonce", 0)):
                raise Failure(f"{swap_id[:12]}: a re-plan did not advance the nonce: {row}")
    return {"swaps": [{"swap_id": swap_id, "offer_id": corridor["offer_id"], "entry": corridor["entry"],
                       "zcash_funding": {"transaction_id": fundings[swap_id][0], "height": fundings[swap_id][1]},
                       "zcash_claim": {"transaction_id": claims[swap_id][0], "height": claims[swap_id][1]},
                       "taker_lez_replans": replans[swap_id]}
                      for swap_id, _, corridor in swaps],
            "corridor": corridors[0]["run_id"], "profile": corridors[0]["profile"]}


def zec_wait_refused(swap_id: str, timeout: int) -> dict:
    """Polls until the Taker row reads attention_required with the balance reason."""
    wait = Wait(f"the Taker's balance refusal on {swap_id[:12]}", timeout)
    while wait.running():
        view = taker_view(swap_id)
        key = (view["state"], view.get("attention_reason"), view.get("last_failure_class"))
        wait.note(key, lambda: f"taker {key[0]} (reason {key[1]}, class {key[2]}) | maker {maker_phase(swap_id)}")
        if view["state"] == "attention_required" and view.get("attention_reason") == "lez_depositor_balance_insufficient":
            if view.get("available_action"):
                raise Failure(f"the refused swap offers {view['available_action']}; it must offer nothing")
            return view
        if view["state"] not in {"not_activated", "initiating", "awaiting_first_lock", "attention_required"}:
            raise Failure(f"the Taker lists {swap_id[:12]} as {view['state']} before any refusal: the account was not short")
        time.sleep(5)
    raise wait.failure()


def zec_wait_recovered(swap_id: str, timeout: int) -> dict:
    """Polls until the row has left attention_required with no reason left on it."""
    wait = Wait(f"the Taker's {swap_id[:12]} to leave attention_required once funded", timeout)
    while wait.running():
        view = taker_view(swap_id)
        key = (view["state"], view.get("attention_reason"), view.get("last_failure_class"))
        wait.note(key, lambda: f"taker {key[0]} (reason {key[1]}, class {key[2]}) | maker {maker_phase(swap_id)}")
        if view["state"] != "attention_required":
            if view.get("attention_reason"):
                raise Failure(f"funded, the row reads {view['state']} but still carries attention_reason={view['attention_reason']}")
            return view
        time.sleep(5)
    raise wait.failure()


def scenario_zec_balance_refusal_foreign(stamp: str) -> dict:
    """TakerSellsForeign: the Maker is the LEZ depositor and cannot fund the
    lot. The Maker Node's signer is the market wallet maker-munich-01; a chain
    created with LEZ_BOOTSTRAP_DEFER_CLAIM=maker-munich-01 leaves its
    allocation in the vault. The Taker funds the Zcash HTLC as usual; the
    Maker's LEZ lock is refused by name (the supervisor names
    lez_depositor_balance_insufficient on the Maker's row and backs off,
    nothing locked); after the account is funded (bootstrap-market.sh claims
    the vault) the supervisor's next attempt locks LEZ with no owner action and
    the swap completes."""
    identity = json.loads((LANE_RUNTIME / "lez" / "maker" / "identity.json").read_text())
    account = identity["account_id"]
    before = lez_account_balance(account)
    if before >= ZEC_LEZ_UNITS:
        raise Failure(f"the Maker's LEZ account holds {before}, not less than the {ZEC_LEZ_UNITS} lot: recreate the "
                      "chain with LEZ_BOOTSTRAP_DEFER_CLAIM=maker-munich-01 (up.sh --fresh-lez) so its allocation "
                      "stays in the vault")
    log(f"  Maker LEZ account holds {before}, below the {ZEC_LEZ_UNITS} lot")
    corridor = zec_provision()
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    wait = Wait("the Maker's balance refusal", 900)
    refused = None
    while wait.running():
        view = maker_view(swap_id)
        key = (view.get("schedule_state"), view.get("attention_reason"), view.get("last_failure_class"), maker_phase(swap_id))
        wait.note(key, f"maker {swap_id[:12]}: schedule {key[0]}, reason {key[1]}, class {key[2]}, phase {key[3]}")
        if view.get("attention_reason") == "lez_depositor_balance_insufficient":
            refused = view
            break
        if view.get("schedule_state") in {"failed", "terminal"}:
            raise Failure(f"the Maker's row ended {view.get('schedule_state')} before any balance refusal")
        # the Taker's Zcash funding needs blocks before the Maker tries to lock
        zec_mining_policy(zec_actor_phase(config), corridor)
        time.sleep(10)
    if refused is None:
        raise wait.failure()
    time.sleep(45)
    again = maker_view(swap_id)
    if again.get("attention_reason") != "lez_depositor_balance_insufficient" or again.get("schedule_state") == "failed":
        raise Failure(f"45 s later the Maker's row reads {again.get('schedule_state')} ({again.get('attention_reason')}), "
                      "not the standing refusal")
    log("  45 s later the Maker's row still names the refusal (the supervisor backs off, nothing locked)")
    log("  funding the Maker's account: scripts/bootstrap-market.sh claims the deferred vault")
    result = subprocess.run(["bash", str(DEPLOY_ROOT / "scripts" / "bootstrap-market.sh")],
                            capture_output=True, text=True, check=False, cwd=DEPLOY_ROOT, timeout=1200)
    if result.returncode != 0:
        raise Failure(f"bootstrap-market.sh failed: {(result.stderr or result.stdout)[-300:]}")
    after = lez_account_balance(account)
    if after < ZEC_LEZ_UNITS:
        raise Failure(f"after the claim the Maker's LEZ account holds {after}, still below the lot")
    market_root = pathlib.Path(next(line.split("=", 1)[1] for line in (RUNTIME / "runtime.env").read_text().splitlines()
                                    if line.startswith("LEZ_MARKET_ROOT=")))
    claim = json.loads((market_root / "bootstrap" / f"{MAKER_WALLET}-claim.json").read_text())
    log(f"  Maker LEZ account holds {after} (vault claim {claim['transaction_id'][:16]}…)")
    zec_finish(swap_id, config, corridor, stamp)
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "profile": corridor["profile"], "offer_id": corridor["offer_id"],
            "lez_lot": ZEC_LEZ_UNITS, "maker_lez_balance_before": before, "maker_lez_balance_after_claim": after,
            "refusal": {"role": "maker", "attention_reason": "lez_depositor_balance_insufficient",
                        "schedule_state": refused.get("schedule_state")},
            "funding": {"vault_claim_transaction_id": claim["transaction_id"], "allocation": claim["allocation"]},
            "recovery": "the Maker Node's supervisor locked LEZ on its retry after funding; the swap completed"}


def scenario_zec_balance_refusal(stamp: str) -> dict:
    """The LEZ depositor cannot fund the lot. The Taker Node's signer is the
    market wallet taker-zurich-01; a chain created with
    LEZ_BOOTSTRAP_DEFER_CLAIM=taker-zurich-01 leaves its allocation in the
    vault, so the account holds nothing. The first drive is refused by name
    (actor exit 3), the Node lists the swap as attention_required with
    lez_depositor_balance_insufficient and offers no action, the Node's own
    re-driver leaves it so, and after the account is funded (bootstrap-market.sh
    run again claims the vault) the next drive submits the lock with no other
    owner action and the swap completes. The Taker Node does not drive the ZEC
    first lock itself (the desk journeys drive it between steps, as this
    harness does), so the recovery is the next drive, not a self-re-drive."""
    if not zec_taker_sells_lez():
        return scenario_zec_balance_refusal_foreign(stamp)
    identity = json.loads((LANE_RUNTIME / "lez" / "taker" / "identity.json").read_text())
    account = identity["account_id"]
    before = lez_account_balance(account)
    if before >= ZEC_LEZ_UNITS:
        raise Failure(f"the Taker's LEZ account holds {before}, not less than the {ZEC_LEZ_UNITS} lot: recreate the "
                      "chain with LEZ_BOOTSTRAP_DEFER_CLAIM=taker-zurich-01 (up.sh --fresh-lez) so its allocation "
                      "stays in the vault")
    log(f"  Taker LEZ account holds {before}, below the {ZEC_LEZ_UNITS} lot")
    corridor = zec_provision()
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    # The Taker Node's supervisor attempts the first lock within a tick; the
    # bridge refuses it by name and the row says why, with no owner action.
    refused = zec_wait_refused(swap_id, timeout=120)
    log(f"  taker {swap_id}: {refused['state']} ({refused['attention_reason']}, class {refused.get('last_failure_class')}), "
        f"no action offered | maker: {maker_phase(swap_id)}")
    if MANUAL_DRIVE:
        # Diagnosis only (ZEC_MANUAL_DRIVE=1): a hand drive meets the same refusal, exit 3.
        code, stderr = zec_drive_exit(config)
        last = (stderr.strip().splitlines() or [""])[-1]
        if code != 3:
            raise Failure(f"a hand drive exited {code}, not 3 (the balance refusal): {last[-200:]}")
        log(f"  a hand drive is refused the same way: exit {code}: {last[:160]}")
    # The supervisor keeps backing off (30 s) while the account is short.
    time.sleep(45)
    again = taker_view(swap_id)
    if again["state"] != "attention_required" or again.get("attention_reason") != "lez_depositor_balance_insufficient":
        raise Failure(f"45 s later the row reads {again['state']} ({again.get('attention_reason')}), not the standing refusal")
    log("  45 s later the row still reads the refusal (the supervisor backs off, nothing locked)")
    log("  funding the account: scripts/bootstrap-market.sh claims the deferred vault")
    result = subprocess.run(["bash", str(DEPLOY_ROOT / "scripts" / "bootstrap-market.sh")],
                            capture_output=True, text=True, check=False, cwd=DEPLOY_ROOT, timeout=1200)
    if result.returncode != 0:
        raise Failure(f"bootstrap-market.sh failed: {(result.stderr or result.stdout)[-300:]}")
    after = lez_account_balance(account)
    if after < ZEC_LEZ_UNITS:
        raise Failure(f"after the claim the Taker's LEZ account holds {after}, still below the lot")
    market_root = pathlib.Path(next(line.split("=", 1)[1] for line in (RUNTIME / "runtime.env").read_text().splitlines()
                                    if line.startswith("LEZ_MARKET_ROOT=")))
    claim = json.loads((market_root / "bootstrap" / f"{TAKER_WALLET}-claim.json").read_text())
    log(f"  Taker LEZ account holds {after} (vault claim {claim['transaction_id'][:16]}…)")
    # Recovery with no owner action: the supervisor's next attempt (its 30 s
    # backoff) prepares the lock, the marker goes, the row leaves
    # attention_required, and the swap runs to completed as usual.
    funded_at = time.time()
    view = zec_wait_recovered(swap_id, timeout=300)
    log(f"  taker {swap_id}: {view['state']} {int(time.time() - funded_at)} s after funding, no attention_reason "
        f"(the supervisor's retry; no hand drive)")
    zec_finish(swap_id, config, corridor, stamp)
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "profile": corridor["profile"], "offer_id": corridor["offer_id"],
            "lez_lot": ZEC_LEZ_UNITS, "taker_lez_balance_before": before, "taker_lez_balance_after_claim": after,
            "refusal": {"state": "attention_required", "attention_reason": "lez_depositor_balance_insufficient",
                        "manual_drive_checked": MANUAL_DRIVE},
            "funding": {"vault_claim_transaction_id": claim["transaction_id"], "allocation": claim["allocation"]},
            "recovery": "the Taker Node's supervisor locked LEZ on its retry after funding; the swap completed"}


def taker_node_log_since(since: str) -> str:
    """The Taker Node's log since `since` (RFC 3339), stdout and stderr."""
    read = subprocess.run(["docker", "logs", "--since", since, NODES["taker"][0]], capture_output=True,
                          text=True, check=False, stdin=subprocess.DEVNULL)
    return read.stdout + read.stderr


def taker_node_log_follow_until(marker: str, since: str, timeout: float) -> bool:
    """Follows the Taker Node's log from `since` (RFC 3339) and returns True the
    moment a line carries `marker`, False after `timeout` seconds."""
    follow = subprocess.Popen(["docker", "logs", "-f", "--since", since, NODES["taker"][0]],
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL, text=True,
                              errors="replace")
    found = threading.Event()

    def read() -> None:
        for line in follow.stdout:
            if marker in line:
                found.set()
                return

    reader = threading.Thread(target=read, daemon=True)
    reader.start()
    try:
        return found.wait(timeout)
    finally:
        follow.kill()
        follow.wait()


def scenario_zec_claim_collision(stamp: str) -> dict:
    """FINDINGS 24, live: a revealing LEZ claim whose nonce another transaction
    of the same account takes is Superseded, re-planned at the live nonce and
    lands (REVIEW 19:38Z). The one-claim gate keeps two claims apart, but not a
    claim and the account's other LEZ transactions, so the collision is made
    from a swap of the other direction on the same Taker Node.

    Swap A sells ZEC (run with --direction TakerSellsForeign) and is brought to
    claim_available; corridor C, selling LEZ, is added beside it (no zebra
    blocks are mined for it, so A keeps its Zcash window) and taken. The Taker
    Node submits C's LEZ first lock on its own; the moment its log says so the
    harness presses A's claim, which is planned from the account's live nonce
    while C's lock is not yet in a block (one about every 2 s): both carry one
    nonce, C's arrived first. A must then show its claim superseded and
    re-planned (both nonces, both ids, the margin to its LEZ refund time) and
    complete. When C's lock was already in a block by then, or the race went
    the other way (C's lock lost the nonce and was re-planned), the claim was
    never superseded: the scenario says so by name (NO-COLLISION) and exits
    NOT_EXERCISED_EXIT, and regress.sh runs it again. C is left to the Nodes;
    the scenario does not wait for it."""
    if zec_taker_sells_lez():
        raise Failure("zec-claim-collision runs with --direction TakerSellsForeign (swap A sells ZEC)")
    first = zec_provision()
    swap_a = zec_take_or_resume(first["offer_id"], stamp, "-a")
    config_a = zec_taker_actor_config(first["receipt_output"])
    zec_wait_until(swap_a, config_a, {"claim_available"}, timeout=2400, corridor=first,
                   describe=f"({swap_a}: {zec_flow()[1:-1]})")
    ROUTE["direction"] = "TakerSellsLez"
    try:
        zec_provision(add=True)
        corridors = list(ZEC_CORRIDORS)
        if len(corridors) != 2 or corridors[0]["catalog_swap_id"] != swap_a:
            raise Failure(f"expected the live entry {swap_a} plus one added, the catalog lists "
                          f"{[c['catalog_swap_id'] for c in corridors]}")
        view_a = taker_view(swap_a)
        if view_a["state"] != "claim_available":
            raise Failure(f"after the Nodes were recreated with corridor C, {swap_a[:12]} reads {view_a['state']}")
        since = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")
        swap_c = zec_take_or_resume(corridors[1]["offer_id"], stamp, "-c")
    finally:
        ROUTE["direction"] = "TakerSellsForeign"
    marker = f"taker actor {swap_c}: drive → submitted lez_"
    # The claim must be planned while C's transaction is still in the mempool,
    # and a LEZ block comes every second: the Node's log is followed as it is
    # written (a `docker logs` poll took longer than a block, the claim was
    # then planned one nonce later and raced C's next transaction instead).
    if not taker_node_log_follow_until(marker, since, timeout=600):
        raise Failure(f"the Taker Node did not submit {swap_c[:12]}'s LEZ first lock within 600 s")
    pressed = time.time()
    result = call("taker", "taker_swap_claim_v1", {"schema_version": 1, "request_id": f"e2e-claim-{stamp}-a",
                                                   "swap_id": swap_a, "expected_generation": view_a["progress_generation"]},
                  timeout=300)
    press_delay = round(time.time() - pressed, 1)
    log(f"  {swap_c[:12]}'s LEZ first lock submitted; A's claim admitted {press_delay} s later "
        f"(replay={result.get('was_replay')})")
    zec_wait_until(swap_a, config_a, {"completed"}, timeout=1200, corridor=corridors[0],
                   describe=f"({swap_a}: the revealing LEZ claim, the Maker's follow-up Zcash claim)")
    wait_completed(swap_a, timeout=600)
    node_log = taker_node_log_since(since)
    superseded = [line.strip() for line in node_log.splitlines() if "revealing LEZ claim superseded" in line]
    first_lock_replans = [line.strip() for line in node_log.splitlines()
                          if swap_c in line and "re-plan" in line]
    for line in superseded + first_lock_replans:
        log(f"  node: {line[:260]}")
    if not superseded:
        raise NotExercised("NO-COLLISION: A completed without a superseded claim (C's lock was in a block before "
                      "A's claim was planned, or C's lock lost the nonce instead"
                      f"{': ' + first_lock_replans[0][:120] if first_lock_replans else ''}); try again")
    return {"swap_a": swap_a, "swap_c": swap_c, "corridor": corridors[0]["run_id"],
            "claim_supersessions": superseded, "c_first_lock_replans": first_lock_replans,
            "claim_press_delay_seconds": press_delay}


def scenario_zec_concurrent_staggered(stamp: str) -> dict:
    """Two prepared entries of one corridor run, taken together and settled side
    by side: two agreements, two Maker fundings from distinct coinbase outputs,
    two Taker claims; both Nodes list both swaps completed.

    The two swaps are staggered, for two reasons found on the devnet
    (2026-10-29). Each role signs its LEZ transactions with one account: a first
    lock (or a Maker claim) prepared while the other swap's is still unfinalised
    goes out with a stale nonce, is rejected by the sequencer at execution
    ("Nonce mismatch") and is never re-planned by the actor. And two corridors
    provisioned at one Zcash anchor share one four-block refund window, which
    two fundings, their depth and two claims do not fit. So swap A is provisioned,
    taken and brought to claim_available by the Nodes (its Maker claim final);
    then corridor B is added beside it with its own anchor (zec-provision.sh
    --add, Nodes recreated with both), taken and brought there likewise; the
    Zcash claims overlap."""
    first = zec_provision()
    swap_a = zec_take_or_resume(first["offer_id"], stamp, "-a")
    config_a = zec_taker_actor_config(first["receipt_output"])
    views = {swap_a: zec_wait_until(swap_a, config_a, {"claim_available"}, timeout=2400, corridor=first,
                                     describe=f"({swap_a}: {zec_flow()[1:-1]})")}
    zec_provision(add=True)
    corridors = list(ZEC_CORRIDORS)
    if len(corridors) != 2 or corridors[0]["catalog_swap_id"] != swap_a:
        raise Failure(f"expected the live entry {swap_a} plus one added, the catalog lists "
                      f"{[c['catalog_swap_id'] for c in corridors]}")
    second = corridors[1]
    swap_b = zec_take_or_resume(second["offer_id"], stamp, "-b")
    config_b = zec_taker_actor_config(second["receipt_output"])
    log(f"  {swap_a} is claim_available and {swap_b} is taken beside it (Nodes recreated with both corridors)")
    views[swap_b] = zec_wait_until(swap_b, config_b, {"claim_available"}, timeout=2400, corridor=second,
                                    describe=f"({swap_b}: {zec_flow()[1:-1]})")
    swaps = [(swap_a, config_a, corridors[0]), (swap_b, config_b, second)]
    fundings = {}
    for swap_id, _, corridor in swaps:
        outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
        funding = zec_spend_of(outpoint, corridor["zebra_tip_at_provisioning"] + 1)
        if funding is None:
            raise Failure(f"{swap_id[:12]} is claim_available, but no zebra block spends its provisioned coinbase outpoint")
        fundings[swap_id] = funding
        log(f"  {zec_zcash_funder()}'s Zcash funding for {swap_id[:12]}: {funding[0]} in zebra block {funding[1]}")
    if len({fundings[s[0]][0] for s in swaps}) != 2:
        raise Failure(f"the two {zec_zcash_funder()} fundings are one transaction: the corridors shared a funder output")
    for index, (swap_id, _, _) in enumerate(swaps):
        result = call("taker", "taker_swap_claim_v1", {"schema_version": 1, "request_id": f"e2e-claim-{stamp}-{index}",
                                                       "swap_id": swap_id, "expected_generation": views[swap_id]["progress_generation"]},
                      timeout=300)
        log(f"  Taker claimed {zec_taker_claims()} on {swap_id[:12]} (replay={result.get('was_replay')})")
    zec_wait_all_until(swaps, {"completed"}, timeout=1200, describe="(two claim inclusions)")
    claims = {}
    for swap_id, _, _ in swaps:
        funding = fundings[swap_id]
        claim_tx = zec_spend_of((funding[0], 0), funding[1])
        if claim_tx is None:
            raise Failure(f"{swap_id[:12]} reports completed, but no zebra block spends its funding output")
        claims[swap_id] = claim_tx
        log(f"  {zec_zcash_claimant()}'s Zcash claim for {swap_id[:12]}: {claim_tx[0]} in zebra block {claim_tx[1]}")
        wait_completed(swap_id, timeout=600)
    return {"swaps": [{"swap_id": swap_id, "offer_id": corridor["offer_id"], "entry": corridor["entry"],
                       "zcash_funding": {"transaction_id": fundings[swap_id][0], "height": fundings[swap_id][1]},
                       "zcash_claim": {"transaction_id": claims[swap_id][0], "height": claims[swap_id][1]}}
                      for swap_id, _, corridor in swaps],
            "corridor": corridors[0]["run_id"], "profile": corridors[0]["profile"]}


def zec_finish(swap_id: str, config: str, corridor: dict, stamp: str, suffix: str = "") -> dict:
    """Drives a taken ZEC swap from wherever it is to completed on both Nodes:
    the actor to claim_available, the Taker's Zcash claim through the Node, the
    actor to completed. Returns the claim reply."""
    view = zec_wait_until(swap_id, config, {"claim_available"}, timeout=2400, corridor=corridor,
                           describe=zec_flow())
    result = call("taker", "taker_swap_claim_v1", {"schema_version": 1, "request_id": f"e2e-claim-{stamp}{suffix}",
                                                   "swap_id": swap_id, "expected_generation": view["progress_generation"]},
                  timeout=300)
    log(f"  Taker claimed {zec_taker_claims()} on {swap_id[:12]} (replay={result.get('was_replay')})")
    zec_wait_until(swap_id, config, {"completed"}, timeout=900, corridor=corridor, describe="(claim inclusion)")
    wait_completed(swap_id, timeout=600)
    return result


def scenario_zec_replay(stamp: str) -> dict:
    """ZEC analogue of `replay`: the corridor's offer publication replayed with
    the provisioner's own request id, the take replayed with its request id
    (same swap), the Zcash claim replayed. Skipped: the BTC lock replay —
    a ZEC swap's first LEZ lock is the actor's durable intent, driven by the
    Taker Node's supervisor, not a Node request; its idempotence is every
    attempt's."""
    corridor = zec_provision()
    run_stamp = corridor["run_id"].rsplit("-", 1)[1]
    again = call("maker", "maker_offer_publish", {"request_id": f"zec-devnet-publish-{run_stamp}",
                                                  "offer_id": corridor["offer_id"], "route": ROUTE})
    if again.get("was_replay") is not True:
        raise Failure(f"offer publication replay was not idempotent: {again}")
    log("  offer publication replayed")
    entry = discover(corridor["offer_id"])
    request = take_request(corridor["offer_id"], entry, f"e2e-take-{stamp}")
    first = initiate(request)
    second = initiate(request)
    if second.get("was_replay") is not True or second["swap"]["swap_id"] != first["swap"]["swap_id"]:
        raise Failure(f"take replay was not idempotent: {second}")
    swap_id = first["swap"]["swap_id"]
    log(f"  take replayed for {swap_id[:12]}")
    config = zec_taker_actor_config(corridor["receipt_output"])
    view = zec_wait_until(swap_id, config, {"claim_available"}, timeout=2400, corridor=corridor,
                           describe=zec_flow())
    params = {"schema_version": 1, "request_id": f"e2e-claim-{stamp}", "swap_id": swap_id,
              "expected_generation": view["progress_generation"]}
    call("taker", "taker_swap_claim_v1", params, timeout=300)
    replay = call("taker", "taker_swap_claim_v1", params, timeout=300)
    if replay.get("was_replay") is not True:
        raise Failure(f"claim replay was not idempotent: {replay}")
    log("  claim replayed")
    zec_wait_until(swap_id, config, {"completed"}, timeout=900, corridor=corridor, describe="(claim inclusion)")
    wait_completed(swap_id, timeout=600)
    listed = [s for s in call("taker", "taker_swap_list_v1", {"schema_version": 1})["swaps"] if s["swap_id"] == swap_id]
    if len(listed) != 1 or listed[0]["state"] != "completed":
        raise Failure(f"swap list disagrees after replays: {listed}")
    return {"swap_id": swap_id, "corridor": corridor["run_id"]}


def scenario_zec_wrong_inputs(stamp: str) -> dict:
    """ZEC analogue of `wrong-inputs`: a publish request id reused for another
    offer, a stale withdraw revision, a take with a mismatched envelope hash,
    an off-preset amount and a wrong LEZ quote, claim and refund on an unknown
    swap, a claim before the Maker funded, the consumed lot leaving Delivery
    and refusing a second take. Skipped: `taker_swap_lock_v1` on an unknown
    swap — a ZEC swap has no Node lock request (the LEZ lock is the actor's)."""
    corridor = zec_provision()
    offer_id = corridor["offer_id"]
    run_stamp = corridor["run_id"].rsplit("-", 1)[1]
    record = next(r for r in call("maker", "maker_offer_list", {}) if r["offer"]["id"] == offer_id)
    expect_error("reusing a publish request id for another offer", "maker", "maker_offer_publish",
                 {"request_id": f"zec-devnet-publish-{run_stamp}", "offer_id": offer_id + "-other", "route": ROUTE})
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
    expect_error("claiming an unknown swap", "taker", "taker_swap_claim_v1",
                 {"schema_version": 1, "request_id": f"e2e-noswap-{stamp}", "swap_id": "00" * 32, "expected_generation": 0})
    expect_error("refunding an unknown swap", "taker", "taker_swap_refund_v1",
                 {"schema_version": 1, "request_id": f"e2e-noswap-refund-{stamp}", "swap_id": "00" * 32, "expected_generation": 0})
    swap_id = zec_take_or_resume(offer_id, stamp)
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
    config = zec_taker_actor_config(corridor["receipt_output"])
    zec_finish(swap_id, config, corridor, stamp)
    return {"swap_id": swap_id, "corridor": corridor["run_id"]}


def scenario_zec_restart(stamp: str, role: str, kill: bool = False) -> dict:
    """ZEC analogue of `restart-<role>`: the Taker's LEZ lock final on both
    Nodes, the role's Node restarted, the swap carried to completed afterwards
    by the Nodes alone (each Node's supervisor resumes its actor from its
    store; a Taker row left leased by the restart is requeued as
    `coordinator_restarted`, which the Taker's view reports as
    last_failure_class until its next attempt). `zec-kill-<role>` kills the
    role's container with SIGKILL there instead (kill_node: Basecamp and the
    Node in it go down together) and starts it again."""
    corridor = zec_provision()
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    zec_wait_until(swap_id, config, {"refund_available"}, timeout=1200, corridor=corridor,
                    describe="(Taker first lock final)")
    container, _ = NODES[role]
    restarted_at = time.time()
    if kill:
        killed = kill_node(role)
    else:
        log(f"  restarting {container}")
        docker("restart", container)
        wait_healthy(role)
    classes = set()
    if role == "taker":
        # What the supervisor says about the restart: a row it held during the
        # restart comes back as coordinator_restarted; one between attempts
        # comes back as it was. Both are fine; the log records which.
        for _ in range(4):
            view = taker_view(swap_id)
            if view.get("last_failure_class"):
                classes.add(view["last_failure_class"])
            time.sleep(5)
        restart_lines = subprocess.run(["docker", "logs", "--since", f"{int(time.time() - restarted_at) + 5}s", container],
                                       capture_output=True, text=True, check=False).stderr
        if "coordinator_restarted" in restart_lines:
            classes.add("coordinator_restarted")
        log(f"  after the restart the Taker's schedule reports {sorted(classes) or 'no failure class'}; the supervisor resumes")
    zec_finish(swap_id, config, corridor, stamp)
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "restarted": container,
            "taker_schedule_after_restart": sorted(classes), **({"killed": killed} if kill else {})}


def scenario_zec_survivor(stamp: str) -> dict:
    """ZEC analogue of `survivor`: the Taker Node stopped right after its Zcash
    claim was broadcast; the Maker completes alone (its follow-up observes the
    claim once zebra includes it — zebra is mined from here, as always); the
    Taker Node restarted and its supervisor carrying the actor to completed.

    Selling LEZ the claim is a Zcash transaction, and nothing but this harness
    mines zebra, so a claim in zebra's mempool at the stop confirms while the
    Taker is down: #103 item 14's case (a) for the ZEC pair. The claim is looked
    for in the mempool before the stop, and when it is there its confirmation
    is asserted before the Taker comes back (`claim_confirmed_while_stopped`)."""
    corridor = zec_provision()
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    view = zec_wait_until(swap_id, config, {"claim_available"}, timeout=2400, corridor=corridor,
                           describe=zec_flow())
    call("taker", "taker_swap_claim_v1", {"schema_version": 1, "request_id": f"e2e-claim-{stamp}",
                                          "swap_id": swap_id, "expected_generation": view["progress_generation"]},
         timeout=300)
    held = []
    if zec_taker_sells_lez() and not ZEC_PUBLIC:
        try:
            zec_poll_until("the Taker's Zcash claim in zebra's mempool", 120, lambda: bool(zec_mempool()))
            held = list(zec_mempool())
            log(f"  the Taker's Zcash claim {held[0][:12]} waits in zebra's mempool, unconfirmed")
        except Failure as failure:
            log(f"  no Zcash claim in zebra's mempool before the stop ({failure}); the late-claim case is not held")
    log(f"  stopping the Taker Node right after its {zec_taker_claims()} claim")
    docker("stop", NODES["taker"][0])
    confirmed_while_stopped = False
    try:
        wait = Wait("the Maker to complete without the Taker", 1800)
        while wait.running():
            phase = maker_phase(swap_id)
            pool = zebra("getrawmempool")
            if phase == "completed":
                break
            wait.note(phase, f"maker {phase} | zebra mempool {len(pool) if isinstance(pool, list) else '?'}")
            if isinstance(pool, list) and pool:
                zebra_mine(1, f"mempool had {[t[:12] for t in pool]}")
            time.sleep(20)
        else:
            raise wait.failure()
        if held:
            confirmations = int((zebra("getrawtransaction", [held[0], 1]) or {}).get("confirmations") or 0)
            if confirmations < 1:
                raise Failure(f"the Maker completed but the Taker's claim {held[0][:12]} is still unconfirmed")
            confirmed_while_stopped = True
            log(f"  the Taker's claim {held[0][:12]} confirmed ({confirmations}) while the Taker Node was down")
    finally:
        docker("start", NODES["taker"][0])
        wait_healthy("taker")
    zec_wait_until(swap_id, config, {"completed"}, timeout=900, corridor=corridor, describe="(Taker back)")
    wait_completed(swap_id, timeout=600)
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "claim_confirmed_while_stopped": confirmed_while_stopped}


def scenario_zec_regenerated_catalog(stamp: str) -> dict:
    """ZEC analogue of `regenerated-config`: a swap taken under one rendering of
    the live role configuration (which carries the prepared ZEC catalog)
    survives a re-render and a Taker Node restart, reloads against its own
    receipt and stays actionable to completion."""
    corridor = zec_provision()
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    zec_wait_until(swap_id, config, {"refund_available"}, timeout=1200, corridor=corridor,
                    describe="(Taker first lock final)")
    regenerate_runtime_config()
    log("  restarting the Taker Node onto the re-rendered configuration")
    docker("restart", NODES["taker"][0])
    wait_healthy("taker")
    listed = [s for s in call("taker", "taker_swap_list_v1", {"schema_version": 1})["swaps"] if s["swap_id"] == swap_id]
    if len(listed) != 1 or listed[0]["state"] in TERMINAL:
        raise Failure(f"the swap did not reload after the restart: {listed}")
    log(f"  swap {swap_id[:12]} reloaded as {listed[0]['state']}")
    zec_finish(swap_id, config, corridor, stamp)
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "reloaded_state": listed[0]["state"]}


def scenario_zec_tampered_catalog(stamp: str) -> dict:
    """ZEC analogue of `tampered-config`: the actor configuration a swap was
    taken under (the swap's own copy, pinned by its receipt) is altered; after
    a Taker Node restart the swap is not actionable and a claim is refused.
    The catalog source itself is not altered: its sha256 is pinned at the
    Node's start and a mismatch keeps the whole catalog out, which the
    entrypoint already reports."""
    corridor = zec_provision()
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    docker("exec", NODES["taker"][0], "sh", "-c", f"printf '\n' >> {config}")
    log("  appended one byte to the swap's copied actor-config.json")
    docker("restart", NODES["taker"][0])
    wait_healthy("taker")
    listed = [s for s in call("taker", "taker_swap_list_v1", {"schema_version": 1})["swaps"] if s["swap_id"] == swap_id]
    state = listed[0]["state"] if listed else "absent"
    reply = rpc("taker", "taker_swap_claim_v1", {"schema_version": 1, "request_id": f"e2e-tampered-{stamp}",
                                                 "swap_id": swap_id, "expected_generation": 0}, timeout=120)
    if "result" in reply:
        raise Failure(f"a claim was accepted on a swap whose saved actor configuration was altered ({state})")
    if listed and state not in {"attention_required"}:
        raise Failure(f"the altered swap still lists as {state} instead of attention_required")
    category = (reply["error"].get("data") or {}).get("category") if isinstance(reply["error"].get("data"), dict) else None
    log(f"  swap {swap_id[:12]} after the tamper: {state}; claim refused ({category or reply['error'].get('message')})")
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "state_after_tamper": state, "claim_refusal": category}


def zec_export_tool(swap_id: str) -> bool:
    """`node-e2e.py --pair Zcash zec-export --swap-id …`: the ZEC counterpart of
    export-node-evidence.py for the desk journeys. A ZEC swap has no Bitcoin
    swap store to read; its evidence is the corridor (id, profile), the Taker
    actor's durable status, both Nodes' swap views, and the zebra transactions
    that spend the provisioned coinbase output (the Maker's funding) and the
    funding output itself (the Taker's claim or the Maker's refund), with their
    heights. Written to runtime/evidence/zec/<swap_id>.json."""
    # An export reads the corridor the Nodes already run with; provisioning a
    # fresh one here would recreate both Nodes and discard the swap it is
    # meant to document (it did, once, 2026-10-29 18:24Z).
    if os.environ.get("LEZ_ZEC_REUSE_CORRIDOR") != "1":
        raise Failure("zec-export reads the live corridor: run it with LEZ_ZEC_REUSE_CORRIDOR=1 "
                      "(and the runtime env), it never provisions")
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
    out = LANE_RUNTIME / "evidence" / "zec"; out.mkdir(parents=True, exist_ok=True)
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
    is its Node's re-drive of the admitted refund (or the desk's "Resubmit
    refund" once the row looks stuck), so it is left to the Taker."""
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
    """`zec-wait --swap-id a --swap-id b --until <state>`: the desk's concurrent
    journey waits for both swaps on the corridor the Nodes run with."""
    os.environ["LEZ_ZEC_REUSE_CORRIDOR"] = "1"
    swaps = []
    try:
        for swap_id in swap_ids:
            corridor = dict(zec_provision(swap_id=swap_id))
            config = zec_taker_actor_config(corridor["receipt_output"])
            swaps.append((swap_id, config, corridor))
        views = zec_wait_all_until(swaps, set(until.split("|")), timeout, describe=f"(zec-drive to {until})")
    except Failure as failure:
        log(f"FAILED: {failure}")
        return False
    for swap_id, view in views.items():
        log(f"  taker {swap_id[:12]} is {view.get('state')}")
    return True


def zec_drive_tool(swap_id: str, until: str, timeout: int, mine_pending_only: bool = False,
                   until_phase: str | None = None) -> bool:
    """`node-e2e.py --pair Zcash zec-wait --swap-id … --until <state>`: the desk
    journeys' wait between their steps (ui-e2e.sh): the Taker Node drives its
    actor, this polls both Nodes and mines zebra for a reason, on the corridor
    the Nodes currently run with. No preflight, no summary file. `zec-drive` is
    the same with a hand drive per poll (diagnosis)."""
    os.environ["LEZ_ZEC_REUSE_CORRIDOR"] = "1"
    corridor = zec_provision(swap_id=swap_id)
    config = zec_taker_actor_config(corridor["receipt_output"])
    try:
        if until_phase:
            # The Node reads refund_available from the Taker's confirmed LEZ lock
            # on; a refund journey has to stop at the actor's phase instead,
            # once the Maker's Zcash funding is in (both legs locked).
            view = zec_wait_until_phase(swap_id, config, set(until_phase.split("|")), timeout, corridor,
                                         mine_pending_only=mine_pending_only, describe=f"(zec-drive to phase {until_phase})")
        else:
            view = zec_wait_until(swap_id, config, set(until.split("|")), timeout, corridor, describe=f"(zec-drive to {until})",
                                   mine_pending_only=mine_pending_only)
    except Failure as failure:
        log(f"FAILED: {failure}")
        return False
    # A state drive returns the Taker's view, a phase drive the actor's last effect.
    log(f"  taker {swap_id[:12]} is {view.get('state') or view.get('phase')}")
    return True


def zec_wait_until_phase(swap_id: str, config: str, phases: set[str], timeout: int, corridor: dict,
                         mine_pending_only: bool, describe: str = "") -> dict:
    """Polls until the Taker actor's own durable phase is in `phases` (the Node
    reads refund_available from the confirmed LEZ lock on, so a refund
    journey stops at the phase instead). With `mine_pending_only` a zebra
    block is generated only to include what waits in the mempool, never for
    depth: that keeps the Maker's funding at one confirmation, below the
    pre-reveal gate."""
    timeout = zec_timeout(timeout)
    wait = Wait(f"the Taker actor of {swap_id[:12]} to reach phase {'/'.join(sorted(phases))} {describe}".rstrip(),
                timeout)
    next_depth = 0.0
    while wait.running():
        effect = zec_poll_effect(config)
        phase = (effect or {}).get("phase") or zec_actor_phase(config)
        view = taker_view(swap_id)
        state = view["state"]
        reason = view.get("attention_reason") or view.get("last_failure_class")
        described = zec_describe(effect) if effect else f"actor at {phase or '-'}"
        wait.note((state, described, reason),
                  lambda: f"taker {state}{' (' + reason + ')' if reason else ''} | {described} "
                          f"| maker {maker_phase(swap_id)} | zebra {zebra('getblockcount')}")
        if phase in phases:
            return {"phase": phase, "state": state}
        if state in TERMINAL:
            raise Failure(f"swap {swap_id[:12]} ended in {state} while waiting for phase {sorted(phases)} {describe}")
        if mine_pending_only or time.time() < next_depth:
            zec_mine_pending()
        else:
            zec_mining_policy(phase, corridor)
            next_depth = time.time() + ZEC_DEPTH_SECONDS
        time.sleep(ZEC_POLL_SECONDS)
    raise wait.failure()


# (swap id, request id) -> the generation the Taker admitted that refund request at.
ZEC_REFUND_REQUESTS: dict[tuple[str, str], int] = {}


def zec_refund_until_terminal(swap_id: str, stamp: str, timeout: int,
                              until: frozenset[str] = frozenset({"refunded"}), mine_pending: bool = False) -> dict:
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
    comes back for `refunded`. With `mine_pending` zebra is generated for
    whatever waits in its mempool (the Taker's own Zcash refund when it sold
    ZEC): a LEZ refund needs no block from here."""
    wait = Wait(f"the Taker's refund of {swap_id[:12]} to reach {'/'.join(sorted(until))}", timeout)
    attempt = 0
    request_id = f"e2e-refund-{stamp}"
    while wait.running():
        view = taker_view(swap_id)
        wait.note(view["state"], lambda: f"taker {view['state']} (generation {view.get('progress_generation')}) "
                                         f"| maker {maker_phase(swap_id)}")
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
        if mine_pending:
            zec_mine_pending()
        time.sleep(5)
    raise wait.failure()


def wait_maker_terminal(swap_id: str, timeout: int, describe: str) -> str:
    wait = Wait(f"the Maker's {swap_id[:12]} to reach a terminal state {describe}".rstrip(), timeout)
    while wait.running():
        view = maker_view(swap_id)
        phase = maker_phase(swap_id)
        wait.note(phase, f"maker {phase} (schedule {view.get('schedule_state')})")
        if view.get("schedule_state") == "terminal":
            return phase
        time.sleep(3)
    raise wait.failure()


def zec_outpoint_unspent(outpoint: tuple[str, int]) -> bool:
    return zebra("gettxout", [outpoint[0], outpoint[1]]) is not None


def zec_cut_maker_zebra_route() -> None:
    """The Maker Node reaches zebra through the socat forwarder its entrypoint
    starts (127.0.0.1:18232 -> zebra). Killing it leaves the Maker able to
    observe the Taker's LEZ lock through its sidecar but unable to plan or
    submit its Zcash funding (connection refused); the next container start
    brings the forwarder back."""
    docker("exec", NODES["maker"][0], "pkill", "-f", "TCP:[^ ]*:18232")


def zec_cut_maker_lez_submit_route() -> None:
    """The Maker Node reaches the LEZ sequencer through the socat forwarder its
    entrypoint starts (127.0.0.1:3040 -> sequencer); the indexer forwarder
    (8779) is another process. Killing the sequencer one leaves the Maker able
    to observe the Taker's Zcash funding through its sidecar but unable to
    submit its LEZ lock; the next container start brings the forwarder back."""
    docker("exec", NODES["maker"][0], "pkill", "-f", "TCP:[^ ]*:3040")


def wait_maker_phase(swap_id: str, phases: set[str], timeout: int, describe: str) -> str:
    wait = Wait(f"the Maker's {swap_id[:12]} to reach {'/'.join(sorted(phases))} {describe}".rstrip(), timeout)
    while wait.running():
        view = maker_view(swap_id)
        phase = maker_phase(swap_id)
        wait.note(phase, f"maker {phase} (schedule {view.get('schedule_state')})")
        if phase in phases:
            return phase
        time.sleep(3)
    raise wait.failure()


def scenario_zec_taker_refund(stamp: str) -> dict:
    """The Taker locks LEZ, the Maker never funds Zcash, the Taker refunds at
    the agreement's LEZ refund time (4 min after provisioning on the refund
    journeys' profile) and the Maker reconciles the refunded leg once it is back.

    The Maker has to observe the Taker's lock before it goes down: the SDK's
    agreement-ordered recovery starts from an observed first leg
    (`next_refund_step` has no Offered arm, the actor's recovery gate agrees),
    so a Maker that only ever saw the offer has nothing to reconcile and stays
    at `offered`. Its zebra route is therefore cut instead of stopping it at
    once: it observes the LEZ lock through the sidecar, cannot plan the Zcash
    funding, and is stopped at `taker_lock_confirmed`."""
    if not zec_taker_sells_lez():
        return scenario_zec_taker_refund_foreign(stamp)
    corridor = zec_provision(profile=ZEC_REFUND_PROFILE)
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
    log("  cutting the Maker Node's zebra route: it will observe the Taker's lock but cannot fund Zcash")
    zec_cut_maker_zebra_route()
    try:
        zec_wait_until_phase(swap_id, config, {"taker_lock_confirmed", "awaiting_maker_confirmations", "both_legs_locked"},
                              timeout=1500, corridor=corridor, mine_pending_only=True,
                              describe="(Taker LEZ lock confirmed, Maker without zebra)")
        wait_maker_phase(swap_id, {"taker_lock_confirmed", "awaiting_maker_confirmations", "both_legs_locked"},
                         timeout=900, describe="(Maker observing the Taker's LEZ lock without zebra)")
        if zec_outpoint_unspent(outpoint) is not True:
            raise Failure("the provisioned coinbase outpoint was spent although the Maker's zebra route was cut")
        log("  the Maker observed the Taker's LEZ lock; stopping the Maker Node so it never funds Zcash")
        docker("stop", NODES["maker"][0])
        # The LEZ escrow refunds only once the chain clock passes the agreement's
        # refund time: provisioning time + the profile's delay (4 min on the
        # refund journeys' profile). The provisioning summary does not carry it, so it is
        # taken from the corridor's run id stamp, which the script sets to the
        # provisioning second.
        opens = stamp_time(corridor["run_id"]) + corridor["lez_refund_delay_seconds"]
        wait_until(opens + 15, "for the agreement's LEZ refund time (the profile's delay after provisioning)")
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


def scenario_zec_taker_refund_foreign(stamp: str) -> dict:
    """TakerSellsForeign: the Taker funds the Zcash HTLC, the Maker never locks
    LEZ, the Taker refunds its Zcash at the agreement's refund height and the
    Maker reconciles the refunded leg once it is back. As in the other
    direction the Maker must have observed the Taker's first lock before it
    goes down, so its sequencer route is cut instead of stopping it at once:
    it observes the Zcash funding through its sidecar, cannot submit the LEZ
    lock, and is stopped at `taker_lock_confirmed`."""
    corridor = zec_provision(profile=ZEC_REFUND_PROFILE)
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
    log("  cutting the Maker Node's sequencer route: it will observe the Taker's Zcash funding but cannot lock LEZ")
    zec_cut_maker_lez_submit_route()
    try:
        zec_wait_until_phase(swap_id, config, {"taker_lock_confirmed", "awaiting_maker_confirmations", "both_legs_locked"},
                              timeout=1500, corridor=corridor, mine_pending_only=False,
                              describe="(Taker Zcash funding confirmed, Maker without a sequencer route)")
        wait_maker_phase(swap_id, {"taker_lock_confirmed", "awaiting_maker_confirmations", "both_legs_locked"},
                         timeout=900, describe="(Maker observing the Taker's Zcash funding without a sequencer route)")
        funding = zec_spend_of(outpoint, corridor["zebra_tip_at_provisioning"] + 1)
        if funding is None:
            raise Failure("the Taker's actor confirmed its lock, but no zebra block spends the provisioned outpoint")
        if zec_actor_phase(config) != "taker_lock_confirmed":
            raise Failure(f"the Maker locked LEZ although its sequencer route was cut (Taker actor at {zec_actor_phase(config)})")
        log(f"  Taker's Zcash funding {funding[0]} is in zebra block {funding[1]}; the Maker observed it; "
            "stopping the Maker Node so it never locks LEZ")
        docker("stop", NODES["maker"][0])
        height = int(zebra("getblockcount"))
        if height < corridor["zcash_refund_height"]:
            zebra_mine(corridor["zcash_refund_height"] - height, "the agreement's Zcash refund height")
        log("  the Taker's Zcash funding is confirmed, the Maker is down and the refund height is reached")
        zec_refund_until_terminal(swap_id, stamp, timeout=1800, mine_pending=True)
        log("  Taker refunded its Zcash HTLC")
    finally:
        docker("start", NODES["maker"][0])
        wait_healthy("maker")
    # Back up, the Maker re-observes the Taker's funding and finds it refunded;
    # the operator's refund action runs the agreement-ordered recovery and the
    # non-owner ends Refunded.
    if maker_view(swap_id).get("schedule_state") != "terminal":
        result = maker_action_with_fresh_generation("maker_actor_refund_v1", swap_id, f"e2e-zec-taker-refund-{stamp}")
        log(f"  Maker refund queued: {json.dumps(result)[:160]}")
    phase = wait_maker_terminal(swap_id, timeout=1200, describe="(reconciling the refunded Zcash leg)")
    refund_tx = zec_spend_of((funding[0], 0), funding[1])
    if refund_tx is None:
        raise Failure("no zebra block spends the Taker's funding output: the Taker did not refund Zcash")
    log(f"  Taker's Zcash refund {refund_tx[0]} is in zebra block {refund_tx[1]}; Maker ended {phase}")
    if phase not in {"refunded", "taker_leg_refunded"}:
        raise Failure(f"the Maker ended in {phase}, not a refunded phase")
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "profile": corridor["profile"],
            "maker_terminal_phase": phase,
            "zcash_funding": {"transaction_id": funding[0], "height": funding[1]},
            "zcash_refund": {"transaction_id": refund_tx[0], "height": refund_tx[1]},
            "zcash_refund_height": corridor["zcash_refund_height"]}


def scenario_zec_maker_refund(stamp: str) -> dict:
    """Both legs locked, the Maker's Zcash funding at one confirmation (below
    the pre-reveal gate), the Taker refunds LEZ at the agreement's refund time,
    zebra is mined to the agreement's refund height and the Maker refunds its
    Zcash; the Taker observes that refund and both Nodes end `refunded`."""
    if not zec_taker_sells_lez():
        return scenario_zec_maker_refund_foreign(stamp)
    corridor = zec_provision(profile=ZEC_REFUND_PROFILE)
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
    # Both legs locked, the Maker's funding at one confirmation: below the
    # pre-reveal gate (two), so the Maker cannot claim LEZ yet.
    zec_wait_until_phase(swap_id, config, {"both_legs_locked", "awaiting_maker_confirmations"}, timeout=2400,
                          corridor=corridor, mine_pending_only=True, describe="(Taker LEZ lock → Maker Zcash funding)")
    funding = zec_spend_of(outpoint, corridor["zebra_tip_at_provisioning"] + 1)
    if funding is None:
        raise Failure("the Taker observed the Maker's lock, but no zebra block spends the provisioned outpoint")
    log(f"  Maker's Zcash funding {funding[0]} is in zebra block {funding[1]}; the Taker refunds LEZ instead of waiting")
    opens = stamp_time(corridor["run_id"]) + corridor["lez_refund_delay_seconds"]
    wait_until(opens + 15, "for the agreement's LEZ refund time (the profile's delay after provisioning)")
    zec_refund_until_terminal(swap_id, stamp, timeout=1800, until=frozenset({"refund_in_progress", "refunded"}))
    log("  the Taker's LEZ refund is admitted and in progress on the Node; the Maker's Zcash refund is owed next")
    return zec_maker_refund_tail(swap_id, stamp, config, corridor, funding)


def scenario_zec_maker_refund_foreign(stamp: str) -> dict:
    """TakerSellsForeign: the Taker funds the Zcash HTLC, the Maker locks LEZ,
    the Taker's LEZ claim is offered and never pressed; at the agreement's LEZ
    refund time the Maker refunds its LEZ escrow (the queued recovery, as the
    desk's Refund would), then zebra is mined to the Zcash refund height and
    the Taker refunds its HTLC; both Nodes end `refunded`. The LEZ refund
    comes first and the Zcash refund outlives it (F4)."""
    corridor = zec_provision(profile=ZEC_REFUND_PROFILE)
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
    zec_wait_until(swap_id, config, {"claim_available"}, timeout=2400, corridor=corridor,
                   describe="(Taker Zcash funding → Maker LEZ lock; the claim is offered and left alone)")
    funding = zec_spend_of(outpoint, corridor["zebra_tip_at_provisioning"] + 1)
    if funding is None:
        raise Failure("the Taker's claim is offered, but no zebra block spends the provisioned outpoint")
    log(f"  Taker's Zcash funding {funding[0]} is in zebra block {funding[1]}; the Taker never claims")
    opens = stamp_time(corridor["run_id"]) + corridor["lez_refund_delay_seconds"]
    wait_until(opens + 15, "for the agreement's LEZ refund time (the profile's delay after provisioning)")
    if maker_view(swap_id).get("schedule_state") != "terminal":
        try:
            result = maker_action_with_fresh_generation("maker_actor_refund_v1", swap_id, f"e2e-zec-maker-refund-{stamp}")
            log(f"  Maker refund queued: {json.dumps(result)[:160]}")
        except Failure as error:
            if "conflicts with durable state" not in str(error):
                raise
            log("  Maker refund request already recorded for this run; continuing")
    phase = wait_maker_phase(swap_id, {"maker_leg_refunded", "refunded"}, timeout=1500,
                             describe="(the Maker refunding its LEZ escrow)")
    log(f"  the Maker's LEZ leg is back ({phase}); mining zebra to the agreement's refund height")
    height = int(zebra("getblockcount"))
    if height < corridor["zcash_refund_height"]:
        zebra_mine(corridor["zcash_refund_height"] - height, "Zcash refund height")
    taker = zec_refund_until_terminal(swap_id, stamp, timeout=1800, mine_pending=True)
    refund_tx = zec_spend_of((funding[0], 0), funding[1])
    if refund_tx is None:
        raise Failure("no zebra block spends the Taker's funding output: the Taker did not refund Zcash")
    maker_terminal = wait_maker_terminal(swap_id, timeout=900, describe="(observing the Taker's Zcash refund)")
    log(f"  Taker's Zcash refund {refund_tx[0]} is in zebra block {refund_tx[1]}; Taker ended {taker['state']}, Maker {maker_terminal}")
    if maker_terminal not in {"refunded", "maker_leg_refunded", "taker_leg_refunded"}:
        raise Failure(f"the Maker ended in {maker_terminal}, not a refunded phase")
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "profile": corridor["profile"],
            "zcash_funding": {"transaction_id": funding[0], "height": funding[1]},
            "zcash_refund": {"transaction_id": refund_tx[0], "height": refund_tx[1]},
            "zcash_refund_height": corridor["zcash_refund_height"], "maker_terminal_phase": maker_terminal,
            "taker_terminal_state": taker["state"]}


# ---- BTC/ZEC parity (REVIEW 21:08Z): early-lock, offline-after-lock, refund-then-maker-lock ----

# A Node's loopback forwarders (node-entrypoint.sh): the ZEC sidecars read LEZ
# through both the indexer and the sequencer one, and zebra through its own.
# indexer and sequencer are the shared LEZ network's; zebra is the lane's own.
ZEC_ROUTES = {"indexer": (8779, "indexer"), "sequencer": (3040, "sequencer"),
              "zebra": (18232, os.environ.get("LEZ_ZEBRA_HOST", "zebra"))}
# The Taker actor's phases once its own first lock is confirmed (and no further).
ZEC_FIRST_LOCK_CONFIRMED = {"taker_lock_confirmed"}
ZEC_PAST_FIRST_LOCK = {"taker_lock_confirmed", "awaiting_maker_confirmations", "both_legs_locked"}
# Phases that prove a party saw the first lock confirmed (maker_phase() falls
# back to scheduler words before an observation, so this disqualifies rather
# than allows).
ZEC_SAW_FIRST_LOCK = ZEC_PAST_FIRST_LOCK | {"claim_evidence_available", "completed"}


def zec_cut_routes(role: str, *names: str) -> None:
    for name in names:
        port, _ = ZEC_ROUTES[name]
        subprocess.run(["docker", "exec", "-u", "root", NODES[role][0], "pkill", "-f", f"TCP:[^ ]*:{port}"],
                       check=False, capture_output=True)


def zec_restore_routes(role: str, *names: str) -> None:
    """Idempotent (runs from a `finally`): clears, then starts each forwarder."""
    for name in names:
        port, host = ZEC_ROUTES[name]
        subprocess.run(["docker", "exec", "-u", "root", NODES[role][0], "pkill", "-f", f"TCP:[^ ]*:{port}"],
                       check=False, capture_output=True)
        subprocess.run(["docker", "exec", "-d", "-u", "root", NODES[role][0], "socat",
                        f"TCP-LISTEN:{port},bind=127.0.0.1,fork,reuseaddr", f"TCP:{host}:{port}"],
                       check=False, capture_output=True)


def zec_mempool() -> list:
    pool = zebra("getrawmempool")
    return pool if isinstance(pool, list) else []


def zec_poll_until(what: str, timeout: int, check) -> None:
    """Polls `check()` every 3 s without mining zebra."""
    wait = Wait(what, timeout)
    while wait.running():
        if check():
            return
        wait.note("not yet", lambda: f"not yet | zebra {zebra('getblockcount')}, mempool {len(zec_mempool())}")
        time.sleep(3)
    raise wait.failure()


def taker_lez_lock_effect(swap_id: str) -> str | None:
    """The Taker's own LEZ lock as its view records it, when it lists one."""
    for effect in taker_view(swap_id).get("effects") or []:
        if effect.get("kind") == "taker_lock" and str(effect.get("chain", "")).lower() == "lez":
            return effect.get("transaction_id")
    return None


def scenario_zec_early_lock(stamp: str) -> dict:
    """R1 for Zcash: the party that locks second must not lock on a first leg it
    has not seen confirmed. Selling LEZ the first leg is the Taker's LEZ lock and
    the second the Maker's Zcash funding: as in the BTC twin
    (scenario_early_lock_reverse), the Maker's indexer forwarder, its only
    source of LEZ finality, is cut right after the take while its sequencer
    forwarder keeps running; the Taker's lock is confirmed (its actor at
    taker_lock_confirmed) and, where the Taker's view lists the lock, read back
    included over the Maker's own sequencer route. For the Maker's attention
    span the Maker must not fund: its outpoint unspent, zebra's mempool empty,
    its phase before both legs. Selling ZEC the first leg is the Taker's Zcash
    funding: it is held unmined in zebra's mempool, below the pre-reveal depth,
    and the Maker must not lock LEZ: its phase stays before
    taker_lock_confirmed, and on the chain its LEZ account's finalized balance
    is unchanged once finality has passed everything the sequencer held at the
    end of the hold. The view is restored or the funding mined, and the swap
    completes."""
    profile = timing_profile()
    corridor = zec_provision()
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
    held = maker_attention_seconds(profile)
    seen_in_block = None
    if zec_taker_sells_lez():
        log("  cutting the Maker's indexer forwarder, its only source of LEZ finality; its sequencer forwarder stays")
        zec_cut_routes("maker", "indexer")
        try:
            wait_lez_view("maker", restored=False)
            zec_wait_until_phase(swap_id, config, ZEC_PAST_FIRST_LOCK, timeout=1500, corridor=corridor,
                                 mine_pending_only=True, describe="(the Taker's LEZ lock, the Maker without finality)")
            if zec_actor_phase(config) not in ZEC_FIRST_LOCK_CONFIRMED:
                raise Failure(f"the Taker actor is at {zec_actor_phase(config)}: the Maker funded without LEZ finality")
            lez_lock = taker_lez_lock_effect(swap_id)
            if lez_lock:
                seen_in_block = lez_transaction_block("maker", lez_lock)
                if seen_in_block is None:
                    raise Failure(f"the Taker's confirmed LEZ lock {lez_lock[:12]} is not on the Maker's sequencer route")
                log(f"  the Taker's LEZ lock {lez_lock[:12]} is in block {seen_in_block} on the Maker's sequencer route")
            else:
                log("  the Taker's view lists no LEZ lock effect; its actor at taker_lock_confirmed is what says it is in")
            log(f"  the Maker cannot read LEZ finality; watching it for {held}s")
            deadline = time.time() + held
            while time.time() < deadline:
                if zec_outpoint_unspent(outpoint) is not True or zec_mempool():
                    raise Failure("the Maker funded Zcash on a first leg it could not see finalized (R1)")
                if maker_phase(swap_id) in {"awaiting_maker_confirmations", "both_legs_locked"}:
                    raise Failure(f"the Maker reached {maker_phase(swap_id)} without LEZ finality for the Taker's lock")
                time.sleep(5)
            log(f"  the Maker stayed at {maker_phase(swap_id)} for {held}s and funded nothing: outpoint "
                f"{outpoint[0][:12]}:{outpoint[1]} unspent, zebra's mempool empty")
        finally:
            zec_restore_routes("maker", "indexer")
        wait_lez_view("maker", restored=True)
        log("  the Maker can read LEZ finality again; it may now fund Zcash")
    else:
        account = lez_owner_account("maker")
        balance = lez_account_balance(account)
        log("  the Taker Node funds the Zcash HTLC; the harness mines nothing, so it stays unconfirmed")
        zec_poll_until("the Taker's Zcash funding in zebra's mempool", 900, lambda: bool(zec_mempool()))
        log(f"  the Taker's funding {zec_mempool()[0][:12]} waits in the mempool; watching the Maker for {held}s")
        deadline = time.time() + held
        while time.time() < deadline:
            phase = maker_phase(swap_id)
            if phase in ZEC_SAW_FIRST_LOCK:
                raise Failure(f"the Maker reached {phase} on an unconfirmed Zcash funding (R1)")
            if zec_actor_phase(config) in ZEC_SAW_FIRST_LOCK:
                raise Failure(f"the Taker actor reached {zec_actor_phase(config)} on an unconfirmed funding")
            time.sleep(5)
        tip = lez_chain_reading().get("tip")
        if isinstance(tip, int):
            wait_lez_final_through(tip, profile, "everything submitted during the hold")
        if lez_account_balance(account) != balance:
            raise Failure(f"the Maker's LEZ account {account[:12]} went from {balance} to "
                          f"{lez_account_balance(account)} on an unconfirmed Zcash funding (R1)")
        log(f"  the Maker stayed at {maker_phase(swap_id)} for {held}s and its LEZ account still holds {balance}; "
            "mining the funding now")
    zec_finish(swap_id, config, corridor, stamp)
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "profile": corridor["profile"],
            "held_seconds": held, "second_leg_withheld": True, "seen_in_block": seen_in_block}


def scenario_zec_offline_after_lock(stamp: str) -> dict:
    """R2 for Zcash: once the first lock is confirmed the swap proceeds on chain
    state alone. Logos Chat and Delivery are cut as in the BTC twin
    (cut_delivery_and_chat, proven from both sides) and stay cut until the swap
    is complete; neither Node is restarted."""
    corridor = zec_provision()
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    zec_wait_until_phase(swap_id, config, ZEC_PAST_FIRST_LOCK, timeout=1500, corridor=corridor,
                         mine_pending_only=False, describe="(the first lock confirmed)")
    log("  cutting Logos Chat and Delivery, with the first lock confirmed")
    watch = None
    try:
        watch = cut_delivery_and_chat()
        zec_finish(swap_id, config, corridor, stamp)
        held = assert_cut_held(watch)
        log("  the swap completed with both off-chain dependencies cut")
    finally:
        if watch is not None:
            watch.stop()
        restore_delivery_and_chat()
    assert_restored()
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "profile": corridor["profile"], "cut": held}


def scenario_zec_outage_before_take(stamp: str) -> dict:
    """R8 for Zcash: the corridor's offer taken under the outage of
    take_after_outage, refused and left untaken, then taken with the same
    request once Chat and Delivery are back; the swap completes."""
    corridor = zec_provision()
    swap_id, outage = take_after_outage(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    zec_finish(swap_id, config, corridor, stamp)
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "outage": outage}


def scenario_zec_refund_then_maker_lock(stamp: str) -> dict:
    """A refund admitted while the second lock is unobserved, then the second
    lock appears; the Nodes finish the recovery (the Maker's refund is queued as
    the operator's action, as in every ZEC Maker refund). Selling LEZ: the
    Maker's Zcash funding is held unmined in zebra's mempool through the LEZ
    refund time, the Taker's refund is admitted, then the funding is mined; the
    Taker refunds LEZ and the Maker its Zcash at the height. Selling ZEC: the
    Taker's LEZ view is cut before the Maker locks LEZ; at the refund time the
    refund is asked for blind and must be admitted (admit_refund_unobserved: the
    Taker's view holds no maker lock and is before the second lock when it asks
    and right after; a Node that will not admit it without LEZ fails the
    scenario, where it used to pass with blind_admission false; every recorded
    run admitted it), the view comes back, the Maker refunds LEZ and the Taker
    its Zcash; both parties' LEZ accounts end where they started, to the unit."""
    corridor = zec_provision(profile=ZEC_REFUND_PROFILE)
    swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
    config = zec_taker_actor_config(corridor["receipt_output"])
    outpoint = (corridor["funding_outpoints"][0]["transaction_id"], int(corridor["funding_outpoints"][0]["output_index"]))
    opens = stamp_time(corridor["run_id"]) + corridor["lez_refund_delay_seconds"]
    if zec_taker_sells_lez():
        log("  the Taker Node locks LEZ; the Maker's Zcash funding will be held in zebra's mempool")
        zec_poll_until("the Taker's LEZ lock confirmed and the Maker's funding in the mempool", 1500,
                       lambda: zec_actor_phase(config) in ZEC_PAST_FIRST_LOCK and bool(zec_mempool()))
        # gettxout counts a mempool spend as spent; what matters is a block.
        if zec_spend_of(outpoint, corridor["zebra_tip_at_provisioning"] + 1) is not None:
            raise Failure("the Maker's funding is already in a block; the harness mined nothing")
        log(f"  the Maker's funding {zec_mempool()[0][:12]} is broadcast, unconfirmed; waiting for the LEZ refund time")
        wait_until(opens + 15, "for the agreement's LEZ refund time with the Maker's funding unconfirmed")
        view = zec_refund_until_terminal(swap_id, stamp, timeout=900,
                                         until=frozenset({"refund_in_progress", "refunded"}))
        log(f"  refund admitted ({view['state']}, generation {view.get('progress_generation')}), "
            "the Maker's funding still unconfirmed")
        zebra_mine(1, "the Maker's Zcash funding, now after the refund admission")
        funding = zec_spend_of(outpoint, corridor["zebra_tip_at_provisioning"] + 1)
        if funding is None:
            raise Failure("the mined block did not include the Maker's funding")
        log(f"  the Maker's funding {funding[0][:12]} is in zebra block {funding[1]}; the Nodes take it from here")
        result = zec_maker_refund_tail(swap_id, stamp, config, corridor, funding)
        return {**result, "refund_admitted_before_maker_lock": True}
    log("  the Taker Node funds Zcash; the Taker's view of LEZ is cut before the Maker locks")
    lez_accounts = {role: lez_owner_account(role) for role in ("maker", "taker")}
    lez_before = {role: lez_account_balance(account) for role, account in lez_accounts.items()}
    zec_poll_until("the Taker's Zcash funding in zebra's mempool", 900, lambda: bool(zec_mempool()))
    zec_cut_routes("taker", "indexer", "sequencer")
    admission = None
    try:
        wait_lez_view("taker", restored=False)
        zebra_mine(2, "the Taker's funding to the pre-reveal depth")
        funding = zec_spend_of(outpoint, corridor["zebra_tip_at_provisioning"] + 1)
        if funding is None:
            raise Failure("no zebra block spends the provisioned outpoint after mining the Taker's funding")
        maker = wait_maker_phase(swap_id, {"both_legs_locked", "awaiting_maker_confirmations"}, timeout=1200,
                                 describe="(the Maker locking LEZ, the Taker blind to it)")
        log(f"  the Maker locked LEZ ({maker}); the Taker actor is at {zec_actor_phase(config)}, blind to it")
        if zec_actor_phase(config) not in ZEC_FIRST_LOCK_CONFIRMED:
            raise Failure(f"the Taker actor reached {zec_actor_phase(config)} although its LEZ view was cut")
        wait_until(opens + 15, "for the agreement's LEZ refund time with the Taker blind to the Maker's lock",
                   lambda: f"taker actor {zec_actor_phase(config)} | maker {maker_phase(swap_id)}")
        if zec_actor_phase(config) not in ZEC_FIRST_LOCK_CONFIRMED:
            raise Failure(f"the Taker actor reached {zec_actor_phase(config)} although its LEZ view was cut")
        admission = admit_refund_unobserved(swap_id, stamp, required_confirmations=None)
        # Its replays carry the generation it was admitted at (zec_refund_until_terminal).
        ZEC_REFUND_REQUESTS[(swap_id, f"e2e-refund-{stamp}")] = admission["generation"]
    finally:
        zec_restore_routes("taker", "indexer", "sequencer")
    wait_lez_view("taker", restored=True)
    log("  the Taker sees LEZ again; the Maker refunds LEZ, then the Taker its Zcash")
    if maker_view(swap_id).get("schedule_state") != "terminal":
        try:
            result = maker_action_with_fresh_generation("maker_actor_refund_v1", swap_id, f"e2e-zec-maker-refund-{stamp}")
            log(f"  Maker refund queued: {json.dumps(result)[:160]}")
        except Failure as error:
            if "conflicts with durable state" not in str(error):
                raise
    wait_maker_phase(swap_id, {"maker_leg_refunded", "refunded"}, timeout=1500,
                     describe="(the Maker refunding its LEZ escrow)")
    height = int(zebra("getblockcount"))
    if height < corridor["zcash_refund_height"]:
        zebra_mine(corridor["zcash_refund_height"] - height, "Zcash refund height")
    taker = zec_refund_until_terminal(swap_id, stamp, timeout=1800, mine_pending=True)
    refund_tx = zec_spend_of((funding[0], 0), funding[1])
    if refund_tx is None:
        raise Failure("no zebra block spends the Taker's funding output: the Taker did not refund Zcash")
    maker_terminal = wait_maker_terminal(swap_id, timeout=900, describe="(observing the Taker's Zcash refund)")
    log(f"  Taker's Zcash refund {refund_tx[0][:12]} in zebra block {refund_tx[1]}; Taker {taker['state']}, Maker {maker_terminal}")
    if maker_terminal not in {"refunded", "maker_leg_refunded", "taker_leg_refunded"}:
        raise Failure(f"the Maker ended in {maker_terminal}, not a refunded phase")
    lez_after = {role: lez_account_balance(account) for role, account in lez_accounts.items()}
    log(f"  LEZ accounts {lez_before} → {lez_after}")
    if lez_after != lez_before:
        raise Failure(f"the LEZ accounts did not end where they started after both refunds: {lez_before} → {lez_after}")
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "profile": corridor["profile"],
            "blind_admission": True, "refund_admission": admission, "lez_balances_before": lez_before,
            "lez_balances_after": lez_after, "maker_terminal_phase": maker_terminal,
            "taker_terminal_state": taker["state"],
            "zcash_refund": {"transaction_id": refund_tx[0], "height": refund_tx[1]}}


ZEC_STEPS = ("maker-down-after-first-lock", "refund-point", "maker-up-reconcile", "maker-completes", "maker-lez-refund",
             "claim-window-closed")


def zec_step_tool(swap_id: str, step: str, timeout: int) -> bool:
    """`node-e2e.py --pair Zcash zec-step --swap-id … --step …`: the host side of
    the ZEC desk journeys that BTC's desks get from the chain or a stopped Node
    (REVIEW 21:08Z parity), on the corridor the Nodes run with:

    - maker-down-after-first-lock: the Maker observes the Taker's first lock
      (its second-leg route cut: zebra selling LEZ, the sequencer selling ZEC),
      then is stopped, so it never locks its own leg (zec-taker-refund);
    - refund-point: the Taker's refund is due: the LEZ refund time selling LEZ,
      the Zcash refund height (mined) selling ZEC;
    - maker-up-reconcile: the Maker is started again, its refund queued (the
      operator action the Maker desk has no button for), and it ends terminal;
    - maker-completes: zebra is mined for whatever waits in the mempool until
      the Maker reports the swap completed, without the Taker Node (survivor);
    - maker-lez-refund: selling ZEC with the claim never pressed: at the LEZ
      refund time the Maker's refund is queued, its LEZ comes back, zebra is
      mined to the Zcash refund height (maker-refund);
    - claim-window-closed: waits until the revealing claim window has closed
      (the agreement's LEZ refund time minus the profile's 30 s margin) and
      the Taker Node's row no longer offers the claim (FINDINGS 26)."""
    os.environ["LEZ_ZEC_REUSE_CORRIDOR"] = "1"
    # The corridor read asks the Zcash funder's container for its outpoint:
    # selling LEZ that is the Maker, which these steps may find stopped.
    if step == "maker-up-reconcile":
        docker("start", NODES["maker"][0])
        wait_healthy("maker")
    if step == "refund-point" and zec_taker_sells_lez():
        if taker_has_take_time_swap(swap_id):
            taken = zec_dynamic_corridor(swap_id, "")
            opens = stamp_time(taken["run_id"]) + taken["lez_refund_delay_seconds"]
        else:
            run_id = (LANE_RUNTIME / "zec" / "current").read_text().strip()
            opens = stamp_time(run_id) + zec_current_refund_delay()
        wait_until(opens + 15, "for the agreement's LEZ refund time (the profile's delay after provisioning)")
        log("  the Taker's refund is due")
        return True
    corridor = zec_provision(swap_id=swap_id)
    stamp = corridor["run_id"].rsplit("-", 1)[1]
    opens = stamp_time(stamp) + corridor["lez_refund_delay_seconds"]
    try:
        if step == "maker-down-after-first-lock":
            config = zec_taker_actor_config(corridor["receipt_output"])
            sells_lez = zec_taker_sells_lez()
            log("  cutting the Maker Node's " + ("zebra" if sells_lez else "sequencer")
                + " route: it observes the Taker's first lock but cannot lock its own leg")
            if sells_lez:
                zec_cut_maker_zebra_route()
            else:
                zec_cut_maker_lez_submit_route()
            zec_wait_until_phase(swap_id, config, ZEC_PAST_FIRST_LOCK, timeout=timeout, corridor=corridor,
                                 mine_pending_only=sells_lez, describe="(the Taker's first lock confirmed)")
            wait_maker_phase(swap_id, ZEC_PAST_FIRST_LOCK, timeout=900,
                             describe="(the Maker observing the Taker's first lock)")
            if zec_actor_phase(config) not in ZEC_FIRST_LOCK_CONFIRMED:
                raise Failure(f"the Maker locked its leg although its route was cut (Taker at {zec_actor_phase(config)})")
            log("  the Maker observed the Taker's first lock; stopping the Maker Node")
            docker("stop", NODES["maker"][0])
        elif step == "refund-point":
            if zec_taker_sells_lez():
                wait_until(opens + 15, "for the agreement's LEZ refund time (the profile's delay after provisioning)")
            else:
                height = int(zebra("getblockcount"))
                if height < corridor["zcash_refund_height"]:
                    zebra_mine(corridor["zcash_refund_height"] - height, "the agreement's Zcash refund height")
            log("  the Taker's refund is due")
        elif step == "maker-up-reconcile":
            if maker_view(swap_id).get("schedule_state") != "terminal":
                result = maker_action_with_fresh_generation("maker_actor_refund_v1", swap_id, f"e2e-zec-desk-refund-{stamp}")
                log(f"  Maker refund queued: {json.dumps(result)[:160]}")
            phase = wait_maker_terminal(swap_id, timeout=timeout, describe="(reconciling the refunded leg)")
            log(f"  the Maker ended {phase}")
        elif step == "maker-completes":
            wait = Wait(f"the Maker to complete {swap_id[:12]} on its own", timeout)
            while wait.running():
                pool = zec_mempool()
                if pool:
                    zebra_mine(1, f"mempool had {[t[:12] for t in pool]}")
                phase = maker_phase(swap_id)
                wait.note(phase, lambda: f"maker {phase} | zebra {zebra('getblockcount')}")
                if phase == "completed" and maker_view(swap_id).get("schedule_state") == "terminal":
                    break
                time.sleep(3)
            else:
                raise wait.failure()
        elif step == "claim-window-closed":
            closes = opens - 30
            wait_until(closes + 15, "for the revealing claim window to close (LEZ refund time minus the 30 s margin)")
            wait = Wait("the Taker row to stop offering the claim once the window closed", timeout)
            while wait.running():
                view = taker_view(swap_id)
                wait.note((view["state"], view.get("available_action")),
                          f"taker {view['state']} offering {view.get('available_action')}")
                if view.get("available_action") != "claim":
                    log(f"  the window closed; the Taker row reads {view['state']} with {view.get('available_action')}")
                    if view["state"] != "refund_available" or view.get("available_action") != "refund":
                        raise Failure(f"after the window the row reads {view['state']}/{view.get('available_action')}, not refund_available/refund")
                    break
                time.sleep(3)
            else:
                raise Failure(f"the Taker row still offers the claim after the claim window closed (FINDINGS 26): "
                              f"{wait.failure()}")
        elif step == "maker-lez-refund":
            wait_until(opens + 15, "for the agreement's LEZ refund time (the profile's delay after provisioning)")
            if maker_view(swap_id).get("schedule_state") != "terminal":
                try:
                    result = maker_action_with_fresh_generation("maker_actor_refund_v1", swap_id, f"e2e-zec-desk-refund-{stamp}")
                    log(f"  Maker refund queued: {json.dumps(result)[:160]}")
                except Failure as error:
                    if "conflicts with durable state" not in str(error):
                        raise
            phase = wait_maker_phase(swap_id, {"maker_leg_refunded", "refunded"}, timeout=timeout,
                                     describe="(the Maker refunding its LEZ escrow)")
            log(f"  the Maker's LEZ leg is back ({phase}); mining zebra to the Zcash refund height")
            height = int(zebra("getblockcount"))
            if height < corridor["zcash_refund_height"]:
                zebra_mine(corridor["zcash_refund_height"] - height, "Zcash refund height")
        else:
            raise Failure(f"unknown zec-step {step}")
    except Failure as failure:
        log(f"FAILED: {failure}")
        return False
    return True


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
    wait = Wait(f"the Maker's Zcash refund of {swap_id[:12]} in a block and its row terminal", 1500)
    refund_tx = None
    while wait.running():
        pool = zebra("getrawmempool")
        if isinstance(pool, list) and pool:
            zebra_mine(1, f"mempool had {[t[:12] for t in pool]}")
        refund_tx = zec_spend_of((funding[0], 0), funding[1])
        view = maker_view(swap_id)
        phase = maker_phase(swap_id)
        if refund_tx and view.get("schedule_state") == "terminal":
            break
        wait.note((phase, view.get("schedule_state"), bool(refund_tx)),
                  lambda: f"maker {phase} (schedule {view.get('schedule_state')}), refund "
                          f"{'in a block' if refund_tx else 'not in a block'} | zebra {zebra('getblockcount')}")
        time.sleep(5)
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


# ---- one chain down, the other pair still swaps (R3) ---------------------------------
# A lane's Bitcoin Core (with its miner) and its Zebra are each stopped while the
# other pair swaps on the same two Nodes. The Maker's route health
# (node-entrypoint.sh) reads the stopped chain's routes unavailable: the Maker is
# degraded, withdraws the offer published there before the stop and refuses a
# new one, and the other pair's routes stay available. The Taker has no
# per-chain health; what it says is recorded.

def use_route(pair: str, direction: str) -> None:
    """Switches the harness to the other pair inside one scenario, as main() sets it."""
    global REVERSE, FOREIGN_UNITS, LEZ_UNITS
    ROUTE.update({"pair": pair, "direction": direction})
    REVERSE = direction == "TakerSellsLez"
    # the Zcash corridor's lot, or the Bitcoin lot the module starts with
    FOREIGN_UNITS, LEZ_UNITS = (ZEC_FOREIGN_UNITS, ZEC_LEZ_UNITS) if pair == "Zcash" else (1_000_000, 1_000)


def node_reaches(role: str, port: int) -> bool:
    """Whether the role's Node gets an answer from a chain over its own loopback
    forwarder (node-entrypoint.sh); any HTTP answer counts, a refused or reset
    connection does not."""
    result = subprocess.run(["docker", "exec", NODES[role][0], "curl", "-s", "-o", "/dev/null", "-m", "10",
                             f"http://127.0.0.1:{port}/"], capture_output=True, check=False, stdin=subprocess.DEVNULL)
    return result.returncode == 0


def container_running(container: str) -> bool:
    return subprocess.run(["docker", "inspect", container, "--format", "{{.State.Running}}"],
                          capture_output=True, text=True, check=False).stdout.strip() == "true"


def stop_chain(containers: list[str], port: int) -> None:
    """Stops one chain's containers on this lane and proves it from both Nodes:
    each reached the chain over its forwarder before, neither does after, and
    every container is stopped."""
    for role in NODES:
        if not node_reaches(role, port):
            raise Failure(f"before the stop the {role} Node already cannot reach 127.0.0.1:{port}")
    for container in containers:
        log(f"  stopping {container}")
        docker("stop", container)
    still = [container for container in containers if container_running(container)]
    reached = [role for role in NODES if node_reaches(role, port)]
    if still or reached:
        raise Failure(f"the chain is not down: running {still}, reached by {reached}")
    log(f"  {', '.join(containers)} stopped; neither Node reaches 127.0.0.1:{port} any more")


def start_chain(containers: list[str], timeout: int = 180) -> None:
    """Starts them again, in order, each until healthy (or running, without a health check)."""
    for container in containers:
        docker("start", container)
        wait = Wait(f"{container} to be healthy again", timeout)
        while wait.running():
            status = subprocess.run(["docker", "inspect", container, "--format",
                                     "{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}"],
                                    capture_output=True, text=True, check=False).stdout.strip()
            if status in ("healthy", "running"):
                break
            wait.note(status, status)
            time.sleep(3)
        else:
            raise wait.failure()
    log(f"  {', '.join(containers)} started again")


def restart_nodes_while_down(pair: str) -> None:
    """R3: each Node, restarted while `pair`'s chain is down, starts again
    (its launcher reads nothing from that chain it has not kept)."""
    for role in NODES:
        log(f"  restarting the {role} Node with {pair}'s chain down")
        docker("restart", NODES[role][0])
        wait_healthy(role, timeout=300)
    log(f"  both Nodes started again with {pair}'s chain down")


def publish_on(pair: str, stamp: str) -> str:
    """An offer on `pair` in the test's direction; the harness's pair is kept."""
    kept, direction = ROUTE["pair"], ROUTE["direction"]
    use_route(pair, direction)
    try:
        return publish_offer(stamp, "-down")
    finally:
        use_route(kept, direction)


def ready_while_down(pair: str) -> dict:
    """Both Nodes answer health ready while `pair`'s chain is down; the Maker
    reads every `pair` route unavailable, every other route available, and is
    degraded. Returns what each says (degraded, its entries for that pair)."""
    seen = {}
    for role in ("maker", "taker"):
        reported = health(role)
        if reported.get("ready") is not True:
            raise Failure(f"the {role} Node is not ready while {pair} is down: {json.dumps(reported)[:300]}")
        entries = reported.get("routes") if role == "maker" else reported.get("pair_capabilities")
        seen[role] = {"ready": True, "degraded": reported.get("degraded"),
                      pair.lower(): [e for e in entries or [] if (e.get("route") or e).get("pair") == pair]}
    routes = health("maker").get("routes") or []
    wrong = [r for r in routes if (r["state"] == "unavailable") != (r["route"]["pair"] == pair)]
    if not seen["maker"][pair.lower()] or wrong or seen["maker"]["degraded"] is not True:
        raise Failure(f"with {pair} down the Maker's health is not {pair} unavailable, the rest available, "
                      f"degraded: degraded {seen['maker']['degraded']}, routes {routes}")
    return seen


def assert_offer_withdrawn_and_refused(pair: str, offer_id: str, stamp: str) -> None:
    """With `pair`'s chain down the Maker withdraws `offer_id` (published there
    before the stop) and refuses to publish another offer on that route."""
    wait = Wait(f"the Maker to withdraw {offer_id} with {pair} down", 60)
    while wait.running():
        status = next(r["status"] for r in call("maker", "maker_offer_list", {}) if r["offer"]["id"] == offer_id)
        if status == "withdrawn":
            break
        wait.note(status, f"offer {status}")
        time.sleep(2)
    else:
        raise wait.failure()
    reply = rpc("maker", "maker_offer_publish", {"request_id": f"e2e-down-publish-{stamp}",
                                                 "offer_id": f"offer-e2e-down-again-{stamp}",
                                                 "route": {"pair": pair, "direction": ROUTE["direction"]}})
    if "chain dependency is unavailable" not in (reply.get("error") or {}).get("message", ""):
        raise Failure(f"publishing on {pair} with its chain down was not refused for the route: {reply}")
    log(f"  the Maker withdrew {offer_id} and refuses to publish on {pair}: {reply['error']['message']}")


def scenario_zebra_down(stamp: str) -> dict:
    """R3: with this lane's Zebra stopped (stop_chain: neither Node reaches it),
    both Nodes restarted, a Bitcoin happy swap in the test's direction
    completes on both Nodes, both ready in health (route health as above)
    before the take, after it, after the claim and at completion. Zebra
    started again, a Zcash happy swap (scenario_zec_happy)
    completes in the same direction on the same Nodes."""
    zebra_container = f"{PREFIX}-zebra"
    seen = []
    down_offer = publish_on("Zcash", stamp)
    stop_chain([zebra_container], 18232)
    try:
        restart_nodes_while_down("Zcash")
        seen.append(ready_while_down("Zcash"))
        assert_offer_withdrawn_and_refused("Zcash", down_offer, stamp)
        offer_id = publish_offer(stamp, "-btc")
        swap_id, _ = take(offer_id, stamp, "-btc")
        seen.append(ready_while_down("Zcash"))
        lock(swap_id)
        claim(swap_id, stamp, "-btc")
        seen.append(ready_while_down("Zcash"))
        wait_completed(swap_id)
        seen.append(ready_while_down("Zcash"))
        log("  the Bitcoin swap completed with Zebra stopped, both Nodes ready throughout, the Maker's Zcash "
            "routes unavailable and Bitcoin's available")
    finally:
        start_chain([zebra_container])
    direction = ROUTE["direction"]
    use_route("Zcash", direction)
    try:
        zcash = scenario_zec_happy(stamp)
    finally:
        use_route("Bitcoin", direction)
    return {"swap_id": swap_id, "bitcoin_claim": assert_cooperative_claim_is_key_path(swap_id),
            "stopped": [zebra_container], "health_while_down": seen, "zcash_after_restart": zcash}


def scenario_zec_bitcoin_down(stamp: str) -> dict:
    """R3 mirrored: with this lane's Bitcoin Core and miner stopped (stop_chain:
    neither Node reaches Core), both Nodes restarted, a Zcash happy swap in
    the test's direction completes on both Nodes, both ready in health
    throughout; Core and the miner started again, a Bitcoin happy swap
    completes in the same direction. The corridor is provisioned before the
    stop: zec-provision.sh recreates both Nodes, and a Node's first start reads
    Bitcoin's genesis block (node-entrypoint.sh keeps it for later starts)."""
    chain = [f"{PREFIX}-btc-miner", f"{PREFIX}-bitcoin-core"]
    corridor = zec_provision()
    seen = []
    down_offer = publish_on("Bitcoin", stamp)
    stop_chain(chain, 18443)
    try:
        restart_nodes_while_down("Bitcoin")
        seen.append(ready_while_down("Bitcoin"))
        assert_offer_withdrawn_and_refused("Bitcoin", down_offer, stamp)
        swap_id = zec_take_or_resume(corridor["offer_id"], stamp)
        config = zec_taker_actor_config(corridor["receipt_output"])
        seen.append(ready_while_down("Bitcoin"))
        zec_finish(swap_id, config, corridor, stamp)
        seen.append(ready_while_down("Bitcoin"))
        log("  the Zcash swap completed with Bitcoin Core stopped, both Nodes ready throughout, the Maker's "
            "Bitcoin routes unavailable and Zcash's available")
    finally:
        start_chain(chain[::-1])
    direction = ROUTE["direction"]
    use_route("Bitcoin", direction)
    try:
        bitcoin_swap = happy_swap(stamp, "-btc")
        bitcoin_claim = assert_cooperative_claim_is_key_path(bitcoin_swap)
    finally:
        use_route("Zcash", direction)
    return {"swap_id": swap_id, "corridor": corridor["run_id"], "stopped": chain, "health_while_down": seen,
            "bitcoin_after_restart": {"swap_id": bitcoin_swap, "bitcoin_claim": bitcoin_claim}}


ZEC_SCENARIOS = {
    "zec-happy": scenario_zec_happy,
    "zec-taker-refund": scenario_zec_taker_refund,
    "zec-maker-refund": scenario_zec_maker_refund,
    "zec-concurrent": scenario_zec_concurrent,
    "zec-concurrent-staggered": scenario_zec_concurrent_staggered,
    "zec-claim-collision": scenario_zec_claim_collision,
    "zec-early-lock": scenario_zec_early_lock,
    "zec-offline-after-lock": scenario_zec_offline_after_lock,
    "zec-outage-before-take": scenario_zec_outage_before_take,
    "zec-refund-then-maker-lock": scenario_zec_refund_then_maker_lock,
    "zec-replay": scenario_zec_replay,
    "zec-wrong-inputs": scenario_zec_wrong_inputs,
    "zec-restart-taker": lambda stamp: scenario_zec_restart(stamp, "taker"),
    "zec-restart-maker": lambda stamp: scenario_zec_restart(stamp, "maker"),
    "zec-kill-taker": lambda stamp: scenario_zec_restart(stamp, "taker", kill=True),
    "zec-kill-maker": lambda stamp: scenario_zec_restart(stamp, "maker", kill=True),
    "zec-survivor": scenario_zec_survivor,
    "zec-regenerated-catalog": scenario_zec_regenerated_catalog,
    "zec-tampered-catalog": scenario_zec_tampered_catalog,
    "zec-balance-refusal": scenario_zec_balance_refusal,
    "zec-bitcoin-down": scenario_zec_bitcoin_down,
    "zec-taker-cli": through_taker_cli(scenario_zec_happy),
    "zec-dynamic-happy": through_taker_cli(scenario_zec_dynamic_happy),
}


SCENARIOS = {
    "happy": scenario_happy,
    "replay": scenario_replay,
    "wrong-inputs": scenario_wrong_inputs,
    "restart-taker": lambda stamp: scenario_restart(stamp, "taker"),
    "restart-maker": lambda stamp: scenario_restart(stamp, "maker"),
    "kill-taker": lambda stamp: scenario_restart(stamp, "taker", kill=True),
    "kill-maker": lambda stamp: scenario_restart(stamp, "maker", kill=True),
    "taker-cli": through_taker_cli(scenario_happy),
    "taker-cli-refund": through_taker_cli(scenario_taker_refund),
    "survivor": scenario_survivor,
    "late-claim-confirms": scenario_late_claim_confirms,
    "late-claim-unmined": scenario_late_claim_unmined,
    "concurrent": scenario_concurrent,
    "early-lock": scenario_early_lock,
    "offline-after-lock": scenario_offline_after_lock,
    "outage-before-take": scenario_outage_before_take,
    "taker-refund": scenario_taker_refund,
    "maker-refund": scenario_maker_refund,
    "refund-then-maker-lock": scenario_refund_admitted_then_maker_lock,
    "regenerated-config": scenario_regenerated_config,
    "tampered-config": scenario_tampered_config,
    "zebra-down": scenario_zebra_down,
    "maker-cli": scenario_maker_cli,
    "maker-price-module": scenario_maker_price_module,
}


def heartbeat_status() -> dict | None:
    """The heartbeat's status file when it is present and at most two ticks old
    (the service writes it every tick; LEZ_HEARTBEAT_SECONDS in runtime.env)."""
    status_file = RUNTIME / "heartbeat" / "status.json"
    try:
        status = json.loads(status_file.read_text())
        interval = int(next((line.split("=", 1)[1] for line in (RUNTIME / "runtime.env").read_text().splitlines()
                             if line.startswith("LEZ_HEARTBEAT_SECONDS=")), "20") or 20)
        if interval <= 0 or time.time() - int(status.get("at_unix", 0)) > 2 * interval + 15:
            return None
    except (OSError, ValueError, StopIteration, AttributeError, TypeError):
        return None
    return status


def heartbeat_verdict() -> tuple[bool, int, int] | None:
    """(stalled, seconds since finality moved, finalized height) from a fresh
    heartbeat status (heartbeat_status)."""
    status = heartbeat_status()
    if status is None:
        return None
    return bool(status.get("stalled")), int(status.get("finality_flat_seconds", 0)), int(status.get("finalized") or 0)


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
    # The finality heartbeat (deploy/README.md, Part C) already watches this
    # chain when it runs: its status file says whether finality is flat right
    # now and when it last moved, so a fresh status answers in one read. The
    # sample below stays as the fallback when the service is off or stale.
    verdict = heartbeat_verdict()
    if verdict is not None:
        stalled, flat_seconds, finalized = verdict
        if stalled:
            raise Failure(f"the finality heartbeat declares a stall: finalized {finalized} flat for {flat_seconds} s "
                          "while the sequencer advances; recreate the LEZ chain (see project notes) before running swaps")
        log(f"preflight: the finality heartbeat saw finality move {flat_seconds} s ago (finalized {finalized})")
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
    # Ids made from the stamp reach the one LEZ chain the lanes share
    # (zec-provision.sh does the same): no two lanes may make the same one.
    if os.environ.get("LEZ_LANE", "1") != "1":
        stamp += f"{int(os.environ['LEZ_LANE']):02d}"
    global SCENARIO
    SCENARIO = name
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
        summary["result"] = "not_exercised" if isinstance(failure, NotExercised) else "failed"
        summary["error"] = str(failure)
        if isinstance(failure, NotExercised):
            NOT_EXERCISED.append(name)
        log(f"FAILED: {failure}")
    summary["seconds"] = round(time.time() - started)
    out = LANE_RUNTIME / "e2e"
    out.mkdir(parents=True, exist_ok=True)
    suffix = ("" if REVERSE else "-foreign") if ROUTE["pair"] == "Zcash" else ("-lez" if REVERSE else "")
    (out / f"{name}{suffix}.json").write_text(json.dumps(summary, indent=2) + "\n")
    log(f"=== {name}: {summary['result']} in {summary['seconds']}s")
    return summary["result"] == "passed"


def main() -> int:
    global MANUAL_DRIVE
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("scenario", choices=[*SCENARIOS, *ZEC_SCENARIOS, "zec-wait", "zec-drive", "zec-export", "zec-refund-tail", "zec-step", "zec-publish", "zec-fund", "all"])
    parser.add_argument("--swap-id", action="append", default=[],
                        help="zec-drive / zec-export / zec-refund-tail: the swap to act on (zec-drive --until takes several)")
    parser.add_argument("--mine-pending-only", action="store_true",
                        help="zec-drive: mine zebra only for what waits in the mempool, never for depth (refund journeys)")
    parser.add_argument("--until", help="zec-drive: Taker state(s) to reach, '|'-separated")
    parser.add_argument("--until-phase", help="zec-drive: Taker actor phase(s) to reach instead, '|'-separated")
    parser.add_argument("--timeout", type=int, default=2400, help="zec-drive / zec-step: seconds")
    parser.add_argument("--step", choices=ZEC_STEPS, help="zec-step: the host step of a ZEC desk journey")
    parser.add_argument("--swaps", type=int, default=1, help="zec-publish: how many take-time offers")
    parser.add_argument("--add", action="store_true", help="zec-publish: beside the live ones")
    parser.add_argument("--direction", choices=["TakerSellsForeign", "TakerSellsLez"], default=None,
                        help="which side the Taker sells; TakerSellsLez locks LEZ first and claims the foreign coin "
                             "(Bitcoin defaults to TakerSellsForeign, Zcash to TakerSellsLez)")
    parser.add_argument("--pair", choices=["Bitcoin", "Zcash"], default="Bitcoin",
                        help="Zcash runs the zec-* scenarios on a prepared corridor, in either direction")
    args = parser.parse_args()
    global REVERSE, FOREIGN_UNITS, LEZ_UNITS
    if args.pair == "Zcash":
        ROUTE.update({"pair": "Zcash", "direction": args.direction or "TakerSellsLez"})
        REVERSE = ROUTE["direction"] == "TakerSellsLez"
        FOREIGN_UNITS, LEZ_UNITS = ZEC_FOREIGN_UNITS, ZEC_LEZ_UNITS
        if args.scenario in {"zec-wait", "zec-drive"}:
            # zec-wait polls the Nodes (the Taker Node drives its actor); zec-drive
            # is the same wait with a hand drive per poll, for diagnosis only.
            if args.scenario == "zec-drive":
                MANUAL_DRIVE = True
                log("  zec-drive: a hand drive per poll (ZEC_MANUAL_DRIVE); the Taker Node's supervisor drives too")
            if len(args.swap_id) > 1:
                if not args.until or args.until_phase or args.mine_pending_only:
                    parser.error(f"{args.scenario} with several --swap-id takes --until only")
                return 0 if zec_drive_many_tool(args.swap_id, args.until, args.timeout) else 1
            swap_id = args.swap_id[0] if args.swap_id else None
            if not (swap_id and (args.until or args.until_phase)):
                parser.error(f"{args.scenario} needs --swap-id and --until or --until-phase")
            return 0 if zec_drive_tool(swap_id, args.until or "", args.timeout, args.mine_pending_only, args.until_phase) else 1
        args.swap_id = args.swap_id[0] if args.swap_id else None
        if args.scenario == "zec-fund":
            try:
                zec_fund()
            except Failure as failure:
                log(f"FAILED: {failure}")
                return 1
            return 0
        if args.scenario == "zec-publish":
            # A desk journey's take-time corridor: the offers its desk takes.
            try:
                zec_take_time_offers(args.swaps, args.add)
            except Failure as failure:
                log(f"FAILED: {failure}")
                return 1
            return 0
        if args.scenario == "zec-refund-tail":
            if not args.swap_id:
                parser.error("zec-refund-tail needs --swap-id")
            return 0 if zec_refund_tail_tool(args.swap_id) else 1
        if args.scenario == "zec-step":
            if not (args.swap_id and args.step):
                parser.error("zec-step needs --swap-id and --step")
            return 0 if zec_step_tool(args.swap_id, args.step, args.timeout) else 1
        if args.scenario == "zec-export":
            if not args.swap_id:
                parser.error("zec-export needs --swap-id")
            return 0 if zec_export_tool(args.swap_id) else 1
        names = list(ZEC_SCENARIOS) if args.scenario == "all" else [args.scenario]
        unknown = [n for n in names if n not in ZEC_SCENARIOS]
        if unknown:
            parser.error(f"{unknown[0]} is a Bitcoin scenario; with --pair Zcash choose from {sorted(ZEC_SCENARIOS)}")
    else:
        ROUTE["direction"] = args.direction or "TakerSellsForeign"
        REVERSE = ROUTE["direction"] == "TakerSellsLez"
        names = list(SCENARIOS) if args.scenario == "all" else [args.scenario]
        if any(n in ZEC_SCENARIOS for n in names):
            parser.error("zec-* scenarios need --pair Zcash")
    for name in names:
        if not run(name):
            return NOT_EXERCISED_EXIT if name in NOT_EXERCISED else 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
