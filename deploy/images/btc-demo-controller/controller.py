#!/usr/bin/env python3
"""Owner-local wallet market and interactive M3 LEZ/Bitcoin controller.

The service has no TCP listener. It persists a small, fixed-profile offer book
by wallet and turns one accepted offer at a time into the repository's genuine
M3 Bitcoin application flow, in either swap direction (the Taker sells Bitcoin
or the Taker sells LEZ). The four effect-producing actor steps are gated by
the owning Maker or Taker dashboard; no request text becomes a shell command.
"""

from __future__ import annotations

import datetime as dt
import base64
import hashlib
import http.server
import json
import os
import pathlib
import re
import socket
import socketserver
import sqlite3
import threading
import time


SOCKET_PATH = pathlib.Path(os.environ.get(
    "LEZ_BTC_DEMO_SOCKET", "/run/lez-btc-demo/controller.sock"))
DATABASE_PATH = SOCKET_PATH.with_name("market.sqlite3")
LAUNCHER_SOCKET = os.environ.get(
    "LEZ_BTC_LAUNCHER_SOCKET", "/run/lez-btc-launcher/launcher.sock")
EVIDENCE_ROOT = pathlib.Path(os.environ.get("LEZ_M3_EVIDENCE_ROOT", "/runner-repo"))
EVIDENCE_OUTPUT = pathlib.Path(os.environ.get(
    "LEZ_M3_BTC_EVIDENCE_FILE", "/run/evidence/m3-btc-ui-evidence.json"))
RUN_SCRIPT = pathlib.Path("/controller-assets/run-full-swap.sh")
EXPORT_SCRIPT = pathlib.Path("/controller-assets/export-ui-evidence.sh")
RUNNER_EXPORT_SCRIPT = "/tmp/lez-export-btc-ui-evidence.sh"


def runner_script_paths(run_id: str) -> dict[str, str]:
    """Staged script paths for one run.

    Every run gets its own copies: bash reads a script incrementally, so a
    later run restaging a shared path would corrupt an earlier run still
    executing it (a silent status-1 death).
    """
    return {
        "run": f"/tmp/lez-run-full-btc-ui-{run_id}.sh",
        "outer": f"/tmp/lez-interactive-m3-outer-{run_id}.sh",
        "direction": f"/tmp/lez-interactive-m3-direction-{run_id}.sh",
    }

REQUEST_RE = re.compile(r"^ui-(?:maker|taker)-[a-z-]{2,24}-[0-9]{13}$")
OFFER_RE = re.compile(r"^[A-Za-z0-9._-]{8,64}$")
SWAP_RE = re.compile(r"^swap-[a-f0-9]{16}$")
RUN_RE = re.compile(r"^m5arm-[0-9]{10}$")

FIXED_BITCOIN_SATS = 1_000_000
FIXED_LEZ_UNITS = 1_000
MAX_OFFERS_PER_REQUEST = 1
MAX_OPEN_OFFERS_PER_WALLET = 20


# ---------------------------------------------------------------------------
# Swap directions. Both routes use the same fixed preset; they differ in who
# locks what first and which desk owns each interactive gate.
# ---------------------------------------------------------------------------

def _direction_spec(name: str, display: str, ui_direction: str, actions: dict,
                    ordered: tuple, flow: tuple, state_labels: dict,
                    progress: dict, work_phases: dict, work_markers: dict,
                    prep_checkpoints: tuple, gate_splices: tuple,
                    balances: str) -> dict:
    return {
        "name": name,
        "display": display,
        "ui_direction": ui_direction,
        "actions": actions,
        "ordered": ordered,
        "flow": flow,
        "state_labels": state_labels,
        "progress": progress,
        "work_phases": work_phases,
        "work_markers": work_markers,
        "prep_checkpoints": prep_checkpoints,
        "gate_splices": gate_splices,
        "balances": balances,
    }


DIRECTIONS: dict[str, dict] = {}

DIRECTIONS["taker_sells_foreign"] = _direction_spec(
    "taker_sells_foreign",
    "BTC → LEZ",
    "TakerSellsForeign",
    {
        "lock_btc": {
            "role": "taker", "ready_state": "awaiting_taker_lock",
            "working_state": "locking_btc", "label": "Lock 0.01000000 BTC",
        },
        "fund_lez": {
            "role": "maker", "ready_state": "awaiting_maker_fund",
            "working_state": "funding_lez", "label": "Fund 1,000 LEZ",
        },
        "claim_lez": {
            "role": "taker", "ready_state": "awaiting_taker_claim",
            "working_state": "claiming_lez", "label": "Claim 1,000 LEZ",
        },
        "claim_btc": {
            "role": "maker", "ready_state": "awaiting_maker_claim",
            "working_state": "claiming_btc", "label": "Claim Bitcoin",
        },
    },
    ("lock_btc", "fund_lez", "claim_lez", "claim_btc"),
    ("queued", "preparing", "awaiting_taker_lock", "locking_btc",
     "awaiting_maker_fund", "funding_lez", "awaiting_taker_claim",
     "claiming_lez", "awaiting_maker_claim", "claiming_btc", "publishing",
     "completed"),
    {
        "queued": "Queued · starts automatically when the runner frees up",
        "preparing": "Preparing fresh actors and authenticated agreement",
        "awaiting_taker_lock": "Waiting for Taker to lock Bitcoin",
        "locking_btc": "Confirming Taker Bitcoin lock",
        "awaiting_maker_fund": "Waiting for Maker to fund LEZ escrow",
        "funding_lez": "Finalizing Maker LEZ funding",
        "awaiting_taker_claim": "Both legs locked · waiting for Taker claim",
        "claiming_lez": "Finalizing Taker LEZ revealing claim",
        "awaiting_maker_claim": "Secret revealed · waiting for Maker claim",
        "claiming_btc": "Confirming Maker Bitcoin claim",
        "publishing": "Publishing five public chain proofs",
        "completed": "Completed · five chain effects published",
        "failed": "Stopped · operator attention required",
    },
    {
        "queued": 2, "preparing": 4, "awaiting_taker_lock": 42,
        "locking_btc": 44, "awaiting_maker_fund": 54, "funding_lez": 56,
        "awaiting_taker_claim": 74, "claiming_lez": 76,
        "awaiting_maker_claim": 88, "claiming_btc": 90,
        "publishing": 97, "completed": 100, "failed": 0,
    },
    {
        "locking_btc": (15, "Broadcasting the Bitcoin lock transaction"),
        "funding_lez": (45, "Funding the LEZ escrow and waiting for chain finality"),
        "claiming_lez": (30, "Revealing the secret and claiming the LEZ escrow"),
        "claiming_btc": (15, "Sweeping Bitcoin with the revealed secret"),
        "publishing": (20, "Exporting and validating the five public proofs"),
    },
    {
        "locking_btc": "lock_btc", "funding_lez": "fund_lez",
        "claiming_lez": "claim_lez", "claiming_btc": "claim_btc",
        "publishing": None,
    },
    (
        ("private/directions/taker_sells_foreign/planning.json",
         "Swap plan signed for the persistent wallets", 12, 22),
        ("evidence/node-startup-status.json",
         "Attached to the settlement chains", 18, 18),
        ("private/taker_sells_foreign-bitcoin-funding-source.json",
         "Bitcoin funding source reserved on-chain", 24, 16),
        ("evidence/taker_sells_foreign-stage-two.json",
         "Authenticated agreement staged", 30, 12),
        ("evidence/taker_sells_foreign-activation-maker.json",
         "Actors activated · opening the first gate", 38, 5),
    ),
    (
        ('    taker_sells_foreign)\n'
         '      direction_phase_begin first_lock_to_revision_one ||\n'
         '        fail "could not begin first-lock timing"\n'
         "      submit_taker_bitcoin_first_lock\n",
         '    taker_sells_foreign)\n'
         '      direction_phase_begin first_lock_to_revision_one ||\n'
         '        fail "could not begin first-lock timing"\n'
         "      interactive_ui_gate taker lock_btc 0\n"
         "      submit_taker_bitcoin_first_lock\n",
         "Taker Bitcoin lock gate"),
        ("      direction_phase_end first_lock_to_revision_one ||\n"
         '        fail "could not end first-lock timing"\n'
         "      direction_phase_begin second_lock_to_revision_two ||\n"
         '        fail "could not begin second-lock timing"\n'
         '      if [[ "$asset_mode" == "custom_token" ]]; then\n'
         "        submit_actor_maker_lez_asset_second_lock\n",
         "      direction_phase_end first_lock_to_revision_one ||\n"
         '        fail "could not end first-lock timing"\n'
         "      direction_phase_begin second_lock_to_revision_two ||\n"
         '        fail "could not begin second-lock timing"\n'
         "      interactive_ui_gate maker fund_lez 1\n"
         '      if [[ "$asset_mode" == "custom_token" ]]; then\n'
         "        submit_actor_maker_lez_asset_second_lock\n",
         "Maker LEZ funding gate"),
        ("run_actor_claim_flow() {\n"
         '  case "$M3_POC_DIRECTION" in\n'
         "    taker_sells_foreign)\n"
         "      direction_phase_begin revealing_claim_to_revision_three ||\n"
         '        fail "could not begin revealing-claim timing"\n'
         "      submit_actor_lez_claim taker 3 lez-revealing-claim\n",
         "run_actor_claim_flow() {\n"
         '  case "$M3_POC_DIRECTION" in\n'
         "    taker_sells_foreign)\n"
         "      direction_phase_begin revealing_claim_to_revision_three ||\n"
         '        fail "could not begin revealing-claim timing"\n'
         "      interactive_ui_gate taker claim_lez 2\n"
         "      submit_actor_lez_claim taker 3 lez-revealing-claim\n",
         "Taker LEZ claim gate"),
        ("      direction_phase_end revealing_claim_to_revision_three ||\n"
         '        fail "could not end revealing-claim timing"\n'
         "      direction_phase_begin followup_claim_to_revision_four ||\n"
         '        fail "could not begin follow-up-claim timing"\n'
         "      submit_actor_bitcoin_claim maker 4 bitcoin-followup-claim\n",
         "      direction_phase_end revealing_claim_to_revision_three ||\n"
         '        fail "could not end revealing-claim timing"\n'
         "      direction_phase_begin followup_claim_to_revision_four ||\n"
         '        fail "could not begin follow-up-claim timing"\n'
         "      interactive_ui_gate maker claim_btc 3\n"
         "      submit_actor_bitcoin_claim maker 4 bitcoin-followup-claim\n",
         "Maker Bitcoin claim gate"),
    ),
    "foreign",
)

DIRECTIONS["taker_sells_lez"] = _direction_spec(
    "taker_sells_lez",
    "LEZ → BTC",
    "TakerSellsLez",
    {
        "lock_lez": {
            "role": "taker", "ready_state": "awaiting_taker_lock",
            "working_state": "locking_lez", "label": "Lock 1,000 LEZ",
        },
        "lock_btc": {
            "role": "maker", "ready_state": "awaiting_maker_lock",
            "working_state": "locking_btc", "label": "Lock 0.01000000 BTC",
        },
        "claim_btc": {
            "role": "taker", "ready_state": "awaiting_taker_claim",
            "working_state": "claiming_btc", "label": "Claim Bitcoin",
        },
        "claim_lez": {
            "role": "maker", "ready_state": "awaiting_maker_claim",
            "working_state": "claiming_lez", "label": "Claim 1,000 LEZ",
        },
    },
    ("lock_lez", "lock_btc", "claim_btc", "claim_lez"),
    ("queued", "preparing", "awaiting_taker_lock", "locking_lez",
     "awaiting_maker_lock", "locking_btc", "awaiting_taker_claim",
     "claiming_btc", "awaiting_maker_claim", "claiming_lez", "publishing",
     "completed"),
    {
        "queued": "Queued · starts automatically when the runner frees up",
        "preparing": "Preparing fresh actors and authenticated agreement",
        "awaiting_taker_lock": "Waiting for Taker to lock LEZ",
        "locking_lez": "Confirming Taker LEZ lock",
        "awaiting_maker_lock": "Waiting for Maker to lock Bitcoin",
        "locking_btc": "Confirming Maker Bitcoin lock",
        "awaiting_taker_claim": "Both legs locked · waiting for Taker claim",
        "claiming_btc": "Finalizing Taker Bitcoin revealing claim",
        "awaiting_maker_claim": "Secret revealed · waiting for Maker claim",
        "claiming_lez": "Finalizing Maker LEZ claim",
        "publishing": "Publishing five public chain proofs",
        "completed": "Completed · five chain effects published",
        "failed": "Stopped · operator attention required",
    },
    {
        "queued": 2, "preparing": 4, "awaiting_taker_lock": 42,
        "locking_lez": 44, "awaiting_maker_lock": 54, "locking_btc": 56,
        "awaiting_taker_claim": 74, "claiming_btc": 76,
        "awaiting_maker_claim": 88, "claiming_lez": 90,
        "publishing": 97, "completed": 100, "failed": 0,
    },
    {
        "locking_lez": (30, "Locking the LEZ escrow and waiting for chain finality"),
        "locking_btc": (15, "Broadcasting the Maker Bitcoin lock"),
        "claiming_btc": (20, "Revealing the secret and claiming the Bitcoin escrow"),
        "claiming_lez": (30, "Claiming the LEZ escrow with the revealed secret"),
        "publishing": (20, "Exporting and validating the five public proofs"),
    },
    {
        "locking_lez": "lock_lez", "locking_btc": "lock_btc",
        "claiming_btc": "claim_btc", "claiming_lez": "claim_lez",
        "publishing": None,
    },
    (
        ("private/directions/taker_sells_lez/planning.json",
         "Swap plan signed for the persistent wallets", 12, 22),
        ("evidence/node-startup-status.json",
         "Attached to the settlement chains", 18, 18),
        ("private/taker_sells_lez-bitcoin-funding-source.json",
         "Maker Bitcoin funding source reserved on-chain", 24, 16),
        ("evidence/taker_sells_lez-stage-two.json",
         "Authenticated agreement staged", 30, 12),
        ("evidence/taker_sells_lez-activation-maker.json",
         "Actors activated · opening the first gate", 38, 5),
    ),
    (
        ('    taker_sells_lez)\n'
         '      direction_phase_begin first_lock_to_revision_one ||\n'
         '        fail "could not begin first-lock timing"\n'
         '      if [[ "$asset_mode" == "custom_token" ]]; then\n'
         "        submit_taker_lez_asset_first_lock\n",
         '    taker_sells_lez)\n'
         '      direction_phase_begin first_lock_to_revision_one ||\n'
         '        fail "could not begin first-lock timing"\n'
         "      interactive_ui_gate taker lock_lez 0\n"
         '      if [[ "$asset_mode" == "custom_token" ]]; then\n'
         "        submit_taker_lez_asset_first_lock\n",
         "Taker LEZ lock gate"),
        ("      direction_phase_end first_lock_to_revision_one ||\n"
         '        fail "could not end first-lock timing"\n'
         "      direction_phase_begin second_lock_to_revision_two ||\n"
         '        fail "could not begin second-lock timing"\n'
         "      submit_actor_maker_bitcoin_second_lock\n",
         "      direction_phase_end first_lock_to_revision_one ||\n"
         '        fail "could not end first-lock timing"\n'
         "      direction_phase_begin second_lock_to_revision_two ||\n"
         '        fail "could not begin second-lock timing"\n'
         "      interactive_ui_gate maker lock_btc 1\n"
         "      submit_actor_maker_bitcoin_second_lock\n",
         "Maker Bitcoin lock gate"),
        ("    taker_sells_lez)\n"
         "      direction_phase_begin revealing_claim_to_revision_three ||\n"
         '        fail "could not begin revealing-claim timing"\n'
         "      submit_actor_bitcoin_claim taker 3 bitcoin-revealing-claim\n",
         "    taker_sells_lez)\n"
         "      direction_phase_begin revealing_claim_to_revision_three ||\n"
         '        fail "could not begin revealing-claim timing"\n'
         "      interactive_ui_gate taker claim_btc 2\n"
         "      submit_actor_bitcoin_claim taker 3 bitcoin-revealing-claim\n",
         "Taker Bitcoin claim gate"),
        ("      direction_phase_end revealing_claim_to_revision_three ||\n"
         '        fail "could not end revealing-claim timing"\n'
         "      direction_phase_begin followup_claim_to_revision_four ||\n"
         '        fail "could not begin follow-up-claim timing"\n'
         "      submit_actor_lez_claim maker 4 lez-followup-claim\n",
         "      direction_phase_end revealing_claim_to_revision_three ||\n"
         '        fail "could not end revealing-claim timing"\n'
         "      direction_phase_begin followup_claim_to_revision_four ||\n"
         '        fail "could not begin follow-up-claim timing"\n'
         "      interactive_ui_gate maker claim_lez 3\n"
         "      submit_actor_lez_claim maker 4 lez-followup-claim\n",
         "Maker LEZ claim gate"),
    ),
    "lez",
)

FOREIGN = DIRECTIONS["taker_sells_foreign"]
LEZ = DIRECTIONS["taker_sells_lez"]

MAKER_WALLETS = (
    {"id": "maker-munich-01", "label": "Munich Vault 01", "role": "maker",
     "network": "LEZ private local", "accent": "violet"},
    {"id": "maker-basel-02", "label": "Basel Vault 02", "role": "maker",
     "network": "LEZ private local", "accent": "pink"},
)
TAKER_WALLETS = (
    {"id": "taker-zurich-01", "label": "Zurich Wallet 01", "role": "taker",
     "network": "Bitcoin Core regtest", "accent": "green"},
    {"id": "taker-limmat-02", "label": "Limmat Wallet 02", "role": "taker",
     "network": "Bitcoin Core regtest", "accent": "blue"},
)
WALLETS = {entry["id"]: entry for entry in MAKER_WALLETS + TAKER_WALLETS}

# Every action name a role can own across both directions, for the wallet
# attention counters.
ROLE_ACTIONS = {
    role: tuple({name for spec in DIRECTIONS.values()
                 for name, action in spec["actions"].items()
                 if action["role"] == role})
    for role in ("maker", "taker")
}


EFFECTS_CACHE: dict[str, list] = {}


def run_effects(run_id: str) -> list:
    """Public chain effects of a completed run, from its generated evidence."""
    if run_id in EFFECTS_CACHE:
        return EFFECTS_CACHE[run_id]
    path = (EVIDENCE_ROOT / ".e2e" / run_id / "m3-actor-poc" / "evidence" /
            "m3-btc-ui-evidence.json")
    try:
        entries = json.loads(path.read_text()).get("effects", [])
    except (OSError, ValueError):
        return []
    effects = [
        {"sequence": entry.get("sequence"), "chain": entry.get("chain"),
         "label": entry.get("label"), "transaction_id": entry["transaction_id"],
         "finality": entry.get("finality")}
        for entry in entries
        if isinstance(entry, dict)
        and isinstance(entry.get("transaction_id"), str)
        and re.fullmatch(r"[0-9a-f]{64}", entry["transaction_id"])
    ]
    if len(effects) == 5:
        EFFECTS_CACHE[run_id] = effects
    return effects


def format_eta(seconds: float) -> str:
    seconds = max(5, int(seconds))
    if seconds >= 90:
        return f"about {round(seconds / 60)} min left"
    return f"about {max(5, (seconds // 5) * 5)} s left"


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace(
        "+00:00", "Z")


def compact(value: object) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def launcher_call(request: dict) -> dict:
    """Calls the bounded host launcher over its owner-only Unix socket."""
    request = {"schema_version": 1, **request}
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(15)
    try:
        client.connect(LAUNCHER_SOCKET)
        client.sendall(compact(request).encode() + b"\n")
        raw = client.makefile("rb").readline(65537)
    finally:
        client.close()
    if not raw or len(raw) > 65536:
        raise RuntimeError("runner launcher returned an invalid response")
    response = json.loads(raw)
    if response.get("schema_version") != 1 or response.get("ok") is not True:
        raise RuntimeError(str(response.get("error", "runner launcher failed"))[:300])
    result = response.get("result")
    if not isinstance(result, dict):
        raise RuntimeError("runner launcher result is invalid")
    return result


def runner_info() -> dict:
    try:
        result = launcher_call({"operation": "runner_status"})
        return {key: result[key] for key in ("ready", "busy", "reason")}
    except Exception:
        return {"ready": False, "busy": False,
                "reason": "bounded runner launcher is unavailable"}


def replace_once(source: str, old: str, new: str, label: str) -> str:
    if source.count(old) != 1:
        raise RuntimeError(f"runner {label} splice point is not exact")
    return source.replace(old, new, 1)


GATE_HELPER_BLOCK = '''fail() {
  echo "M3 actor direction failed: $*" >&2
  exit 2
}
''' + r'''

interactive_ui_gate() {
  local role="$1" action="$2" expected_revision="$3"
  [[ "${LEZ_INTERACTIVE_UI_GATES:-0}" == 1 ]] || return 0
  local gate_root="${M3_POC_DIRECTION_ROOT}/interactive-gates"
  local ready="${gate_root}/${action}.ready.json"
  local permit="${gate_root}/${action}.permit.json"
  local partial="${ready}.partial"
  mkdir -p "$gate_root"
  chmod 0700 "$gate_root"
  [[ ! -e "$ready" && ! -L "$ready" && ! -e "$permit" && ! -L "$permit" ]] ||
    fail "interactive ${action} gate already exists"
  jq -n --arg run "$M3_POC_RUN_ID" --arg role "$role" --arg action "$action" \
    --arg maker_wallet "${LEZ_INTERACTIVE_MAKER_WALLET:?}" \
    --arg taker_wallet "${LEZ_INTERACTIVE_TAKER_WALLET:?}" \
    --argjson revision "$expected_revision" --arg ready_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" '
    {schema_version:1,run_id:$run,role:$role,action:$action,
     expected_revision:$revision,maker_wallet_id:$maker_wallet,
     taker_wallet_id:$taker_wallet,ready_at:$ready_at}
  ' >"$partial"
  chmod 0600 "$partial"
  mv "$partial" "$ready"
  for _ in {1..57600}; do
    if [[ -f "$permit" && ! -L "$permit" ]]; then
      [[ "$(stat -c '%u:%a' "$permit")" == "$(id -u):600" ]] ||
        fail "interactive ${action} permit is not owner private"
      jq -e --arg run "$M3_POC_RUN_ID" --arg role "$role" --arg action "$action" \
        --argjson revision "$expected_revision" '
        .schema_version == 1 and .run_id == $run and .role == $role
        and .action == $action and .expected_revision == $revision
      ' "$permit" >/dev/null || fail "interactive ${action} permit is inconsistent"
      return 0
    fi
    sleep 0.25
  done
  fail "interactive ${action} approval timed out"
}

''' + r'''# Closing balance at the finalized tip. The v0.2 indexer serves historical
# reads from state breakpoints it takes every 100 blocks, and on a long-lived
# chain it occasionally skips one; every read past the gap then fails until
# the next breakpoint lands. The chain only advances by clock invocations
# between the swap's last effect and the tip, so the current state is the
# state at the tip: fall back to it rather than fail a completed swap.
lez_closing_account() {
  local account="$1" block="$2" output="$3" partial
  partial="${output}.partial"
  for _ in {1..40}; do
    if rpc "$M3_POC_LEZ_INDEXER_RPC_URL" "$(jq -cn --arg account "$account" --argjson block "$block" \
        '{jsonrpc:"2.0",id:1,method:"getAccountAtBlock",params:[$account,$block]}')" >"$partial" 2>/dev/null &&
      jq -e '.error == null and .result != null' "$partial" >/dev/null 2>&1; then
      chmod 0600 "$partial"; mv "$partial" "$output"; return 0
    fi
    sleep 0.25
  done
  rpc_read_file "$M3_POC_LEZ_INDEXER_RPC_URL" \
    "$(jq -cn --arg account "$account" '{jsonrpc:"2.0",id:1,method:"getAccount",params:[$account]}')" \
    "$output"
}

interactive_publish_wallet_balances_FOREIGN() {
  [[ "${LEZ_INTERACTIVE_UI_GATES:-0}" == 1 ]] || return 0
  local tip maker_account taker_account maker_final taker_final output
  local maker_open taker_open funding claim
  tip="$(finalized_tip)"
  maker_account="$(jq -er '.account_id' "$M3_POC_MAKER_LEZ_IDENTITY")"
  taker_account="$(jq -er '.account_id' "$M3_POC_TAKER_LEZ_IDENTITY")"
  maker_final="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-maker-wallet-final.json"
  taker_final="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-taker-wallet-final.json"
  maker_open="${M3_POC_EVIDENCE_DIR}/maker-owner-after-vault-claim.json"
  taker_open="${M3_POC_EVIDENCE_DIR}/taker-owner-after-vault-claim.json"
  funding="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-funding-prepared.json"
  claim="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-bitcoin-followup-claim-confirmed.json"
  output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-interactive-wallet-balances.json"
  lez_closing_account "$maker_account" "$tip" "$maker_final"
  lez_closing_account "$taker_account" "$tip" "$taker_final"
  jq -n --arg run "$M3_POC_RUN_ID" --arg direction "$M3_POC_DIRECTION" \
    --arg maker_wallet "${LEZ_INTERACTIVE_MAKER_WALLET:?}" \
    --arg taker_wallet "${LEZ_INTERACTIVE_TAKER_WALLET:?}" \
    --argjson tip "$tip" --slurpfile maker_open "$maker_open" \
    --slurpfile taker_open "$taker_open" --slurpfile maker_final "$maker_final" \
    --slurpfile taker_final "$taker_final" --slurpfile funding "$funding" \
    --slurpfile claim "$claim" '
    ($funding[0].input_value_sat) as $taker_btc_open
    | ($funding[0].change_value_sat) as $taker_btc_close
    | ($funding[0].contract_value_sat) as $principal
    | ($funding[0].fee_sat) as $lock_fee
    | (($claim[0].result.vout[0].value * 100000000) | round) as $maker_btc_close
    | ($principal - $maker_btc_close) as $claim_fee
    | ($maker_open[0].result.balance) as $maker_lez_open
    | ($taker_open[0].result.balance) as $taker_lez_open
    | ($maker_final[0].result.balance) as $maker_lez_close
    | ($taker_final[0].result.balance) as $taker_lez_close
    | {
        schema_version:1,kind:"m3_interactive_wallet_balance_changes",run_id:$run,
        direction:$direction,finalized_lez_tip:$tip,
        units:{bitcoin:"satoshi",lez:"native unit"},
        wallets:[
          {role:"maker",wallet_id:$maker_wallet,balances:{
            bitcoin:{opening:0,credit:$principal,debit:0,fee:$claim_fee,
              closing:$maker_btc_close,net_change:$maker_btc_close},
            lez:{opening:$maker_lez_open,credit:0,debit:1000,fee:0,
              closing:$maker_lez_close,net_change:($maker_lez_close-$maker_lez_open)}}},
          {role:"taker",wallet_id:$taker_wallet,balances:{
            bitcoin:{opening:$taker_btc_open,credit:0,debit:$principal,fee:$lock_fee,
              closing:$taker_btc_close,net_change:($taker_btc_close-$taker_btc_open)},
            lez:{opening:$taker_lez_open,credit:1000,debit:0,fee:0,
              closing:$taker_lez_close,net_change:($taker_lez_close-$taker_lez_open)}}}
        ],
        reconciliation:{bitcoin_principal:$principal,total_bitcoin_fees:($lock_fee+$claim_fee),
          lez_principal:1000,lez_conserved:
            (($maker_lez_close+$taker_lez_close)==($maker_lez_open+$taker_lez_open))},
        sources:{bitcoin_funding:($direction+"-funding-prepared.json"),
          bitcoin_claim:($direction+"-bitcoin-followup-claim-confirmed.json"),
          lez_opening:["maker-owner-after-vault-claim.json","taker-owner-after-vault-claim.json"],
          lez_closing:[($direction+"-maker-wallet-final.json"),
            ($direction+"-taker-wallet-final.json")]},
        private_material_disclosed:false
      }
  ' >"${output}.partial"
  chmod 0600 "${output}.partial"
  jq -e '
    .kind == "m3_interactive_wallet_balance_changes"
    and (.wallets | length) == 2
    and .wallets[0].balances.bitcoin.closing == 999000
    and .wallets[0].balances.lez.net_change == -1000
    and .wallets[1].balances.bitcoin.net_change == -1001000
    and .wallets[1].balances.lez.net_change == 1000
    and .reconciliation == {bitcoin_principal:1000000,total_bitcoin_fees:2000,
      lez_principal:1000,lez_conserved:true}
    and .private_material_disclosed == false
  ' "${output}.partial" >/dev/null || fail "interactive wallet balances did not reconcile"
  mv "${output}.partial" "$output"
}

''' + r'''interactive_publish_wallet_balances_LEZ() {
  [[ "${LEZ_INTERACTIVE_UI_GATES:-0}" == 1 ]] || return 0
  local tip maker_account taker_account maker_final taker_final output
  local maker_open taker_open funding claim
  tip="$(finalized_tip)"
  maker_account="$(jq -er '.account_id' "$M3_POC_MAKER_LEZ_IDENTITY")"
  taker_account="$(jq -er '.account_id' "$M3_POC_TAKER_LEZ_IDENTITY")"
  maker_final="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-maker-wallet-final.json"
  taker_final="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-taker-wallet-final.json"
  maker_open="${M3_POC_EVIDENCE_DIR}/maker-owner-after-vault-claim.json"
  taker_open="${M3_POC_EVIDENCE_DIR}/taker-owner-after-vault-claim.json"
  funding="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-funding-prepared.json"
  claim="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-bitcoin-revealing-claim-confirmed.json"
  output="${M3_POC_EVIDENCE_DIR}/${M3_POC_DIRECTION}-interactive-wallet-balances.json"
  lez_closing_account "$maker_account" "$tip" "$maker_final"
  lez_closing_account "$taker_account" "$tip" "$taker_final"
  jq -n --arg run "$M3_POC_RUN_ID" --arg direction "$M3_POC_DIRECTION" \
    --arg maker_wallet "${LEZ_INTERACTIVE_MAKER_WALLET:?}" \
    --arg taker_wallet "${LEZ_INTERACTIVE_TAKER_WALLET:?}" \
    --argjson tip "$tip" --slurpfile maker_open "$maker_open" \
    --slurpfile taker_open "$taker_open" --slurpfile maker_final "$maker_final" \
    --slurpfile taker_final "$taker_final" --slurpfile funding "$funding" \
    --slurpfile claim "$claim" '
    ($funding[0].input_value_sat) as $maker_btc_open
    | ($funding[0].change_value_sat) as $maker_btc_close
    | ($funding[0].contract_value_sat) as $principal
    | ($funding[0].fee_sat) as $lock_fee
    | (($claim[0].result.vout[0].value * 100000000) | round) as $taker_btc_close
    | ($principal - $taker_btc_close) as $claim_fee
    | ($maker_open[0].result.balance) as $maker_lez_open
    | ($taker_open[0].result.balance) as $taker_lez_open
    | ($maker_final[0].result.balance) as $maker_lez_close
    | ($taker_final[0].result.balance) as $taker_lez_close
    | {
        schema_version:1,kind:"m3_interactive_wallet_balance_changes",run_id:$run,
        direction:$direction,finalized_lez_tip:$tip,
        units:{bitcoin:"satoshi",lez:"native unit"},
        wallets:[
          {role:"maker",wallet_id:$maker_wallet,balances:{
            bitcoin:{opening:$maker_btc_open,credit:0,debit:$principal,fee:$lock_fee,
              closing:$maker_btc_close,net_change:($maker_btc_close-$maker_btc_open)},
            lez:{opening:$maker_lez_open,credit:1000,debit:0,fee:0,
              closing:$maker_lez_close,net_change:($maker_lez_close-$maker_lez_open)}}},
          {role:"taker",wallet_id:$taker_wallet,balances:{
            bitcoin:{opening:0,credit:$principal,debit:0,fee:$claim_fee,
              closing:$taker_btc_close,net_change:$taker_btc_close},
            lez:{opening:$taker_lez_open,credit:0,debit:1000,fee:0,
              closing:$taker_lez_close,net_change:($taker_lez_close-$taker_lez_open)}}}
        ],
        reconciliation:{bitcoin_principal:$principal,total_bitcoin_fees:($lock_fee+$claim_fee),
          lez_principal:1000,lez_conserved:
            (($maker_lez_close+$taker_lez_close)==($maker_lez_open+$taker_lez_open))},
        sources:{bitcoin_funding:($direction+"-funding-prepared.json"),
          bitcoin_claim:($direction+"-bitcoin-revealing-claim-confirmed.json"),
          lez_opening:["maker-owner-after-vault-claim.json","taker-owner-after-vault-claim.json"],
          lez_closing:[($direction+"-maker-wallet-final.json"),
            ($direction+"-taker-wallet-final.json")]},
        private_material_disclosed:false
      }
  ' >"${output}.partial"
  chmod 0600 "${output}.partial"
  jq -e '
    .kind == "m3_interactive_wallet_balance_changes"
    and (.wallets | length) == 2
    and .wallets[0].balances.bitcoin.net_change == -1001000
    and .wallets[0].balances.lez.net_change == 1000
    and .wallets[1].balances.bitcoin.closing == 999000
    and .wallets[1].balances.bitcoin.net_change == 999000
    and .wallets[1].balances.lez.net_change == -1000
    and .reconciliation == {bitcoin_principal:1000000,total_bitcoin_fees:2000,
      lez_principal:1000,lez_conserved:true}
    and .private_material_disclosed == false
  ' "${output}.partial" >/dev/null || fail "interactive wallet balances did not reconcile"
  mv "${output}.partial" "$output"
}

interactive_publish_wallet_balances() {
  case "$M3_POC_DIRECTION" in
    taker_sells_foreign) interactive_publish_wallet_balances_FOREIGN ;;
    taker_sells_lez) interactive_publish_wallet_balances_LEZ ;;
    *) fail "interactive balances are unavailable for this direction" ;;
  esac
}
'''


def interactive_runner_scripts(run_id: str, direction: str) -> dict[str, bytes]:
    spec = DIRECTIONS[direction]
    paths = runner_script_paths(run_id)
    scripts = EVIDENCE_ROOT / "scripts"
    outer = scripts.joinpath("run-m3-actor-local-poc.sh").read_text()
    direction_script = scripts.joinpath("run-m3-actor-direction.sh").read_text()

    cd_line = 'cd "$(dirname "${BASH_SOURCE[0]}")/.."'
    interactive_cd = 'cd "${LEZ_INTERACTIVE_REPO_ROOT:?}"'
    outer = replace_once(outer, cd_line, interactive_cd, "outer working-directory")
    outer = replace_once(
        outer,
        'readonly direction_driver="${repo_root}/scripts/run-m3-actor-direction.sh"',
        f'readonly direction_driver="{paths["direction"]}"',
        "direction-driver",
    )
    outer = replace_once(
        outer,
        'offer_id="m5btc-offer-${run_id:0:24}"',
        'offer_id="${LEZ_INTERACTIVE_OFFER_ID:?}"',
        "offer identity",
    )
    outer = replace_once(
        outer,
        'reservation_id="m5btc-reservation-${run_id:0:24}"',
        'reservation_id="${LEZ_INTERACTIVE_RESERVATION_ID:?}"',
        "reservation identity",
    )

    direction_script = replace_once(
        direction_script, cd_line, interactive_cd, "direction working-directory")
    fail_block = '''fail() {
  echo "M3 actor direction failed: $*" >&2
  exit 2
}
'''
    direction_script = replace_once(
        direction_script, fail_block, GATE_HELPER_BLOCK, "gate helper")
    for old, new, label in spec["gate_splices"]:
        direction_script = replace_once(direction_script, old, new, label)
    direction_script = replace_once(
        direction_script,
        "  write_actual_effect_manifest\n"
        "  direction_phase_end terminal_evidence ||\n",
        "  write_actual_effect_manifest\n"
        "  interactive_publish_wallet_balances\n"
        "  direction_phase_end terminal_evidence ||\n",
        "wallet balance publication",
    )
    run_script = replace_once(
        RUN_SCRIPT.read_text(),
        "/tmp/lez-interactive-m3-outer.sh",
        paths["outer"],
        "interactive outer script",
    )
    return {
        pathlib.Path(paths["run"]).name: run_script.encode(),
        pathlib.Path(RUNNER_EXPORT_SCRIPT).name: EXPORT_SCRIPT.read_bytes(),
        pathlib.Path(paths["outer"]).name: outer.encode(),
        pathlib.Path(paths["direction"]).name: direction_script.encode(),
    }


def wait_exec(exec_id: str) -> int:
    """Polls in bounded launcher calls: a swap outlives the launcher socket timeout."""
    while True:
        result = launcher_call({"operation": "wait_swap", "exec_id": exec_id})
        if result.get("exit_code") is not None:
            return int(result["exit_code"])


def validate_public_evidence(value: dict, run_id: str, direction: str) -> None:
    effects = value.get("effects") if isinstance(value.get("effects"), list) else []
    identifiers = [effect.get("transaction_id") for effect in effects]
    if not (
        value.get("kind") == "m3_btc_ui_evidence"
        and value.get("result") == "passed"
        and value.get("run_id") == run_id
        and value.get("pair") == "Bitcoin"
        and value.get("direction") == DIRECTIONS[direction]["ui_direction"]
        and value.get("terminal") == {"phase": "completed", "revision": 4}
        and value.get("private_material_disclosed") is False
        and len(effects) == 5 and len(set(identifiers)) == 5
        and sum(effect.get("chain") == "Bitcoin" for effect in effects) == 2
        and sum(effect.get("chain") == "LEZ" for effect in effects) == 3
        and all(effect.get("finality") in ("Confirmed", "Finalized")
                for effect in effects)
    ):
        raise RuntimeError("fresh run evidence failed the public M3 schema checks")


class Market:
    def __init__(self) -> None:
        self.lock = threading.RLock()
        self._initialize()
        self._recover()

    def _connect(self) -> sqlite3.Connection:
        connection = sqlite3.connect(DATABASE_PATH, timeout=10)
        connection.row_factory = sqlite3.Row
        connection.execute("PRAGMA foreign_keys = ON")
        return connection

    def _initialize(self) -> None:
        os.umask(0o077)
        DATABASE_PATH.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        with self._connect() as connection:
            connection.executescript("""
                PRAGMA journal_mode = WAL;
                CREATE TABLE IF NOT EXISTS offers (
                    offer_id TEXT PRIMARY KEY,
                    maker_wallet_id TEXT NOT NULL,
                    state TEXT NOT NULL,
                    bitcoin_sats INTEGER NOT NULL,
                    lez_units INTEGER NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    taker_wallet_id TEXT,
                    ui_swap_id TEXT UNIQUE
                );
                CREATE INDEX IF NOT EXISTS offers_by_wallet_state
                    ON offers(maker_wallet_id, state, created_at, offer_id);
                CREATE TABLE IF NOT EXISTS swaps (
                    ui_swap_id TEXT PRIMARY KEY,
                    offer_id TEXT NOT NULL UNIQUE,
                    maker_wallet_id TEXT NOT NULL,
                    taker_wallet_id TEXT NOT NULL,
                    state TEXT NOT NULL,
                    action_required TEXT,
                    run_id TEXT UNIQUE,
                    protocol_swap_id TEXT,
                    exec_id TEXT,
                    started_at TEXT,
                    completed_at TEXT,
                    error TEXT,
                    FOREIGN KEY(offer_id) REFERENCES offers(offer_id)
                );
                CREATE INDEX IF NOT EXISTS swaps_by_maker
                    ON swaps(maker_wallet_id, state, ui_swap_id);
                CREATE INDEX IF NOT EXISTS swaps_by_taker
                    ON swaps(taker_wallet_id, state, ui_swap_id);
                CREATE TABLE IF NOT EXISTS requests (
                    request_id TEXT PRIMARY KEY,
                    method TEXT NOT NULL,
                    fingerprint TEXT NOT NULL,
                    recorded_at TEXT NOT NULL
                );
            """)
            # Direction support arrived after the first deployments: existing
            # books are entirely the forward route, so the backfill default
            # preserves every historical row.
            for table in ("offers", "swaps"):
                columns = {row["name"] for row in
                           connection.execute(f"PRAGMA table_info({table})")}
                if "direction" not in columns:
                    connection.execute(
                        f"ALTER TABLE {table} ADD COLUMN direction TEXT "
                        "NOT NULL DEFAULT 'taker_sells_foreign'")
        os.chmod(DATABASE_PATH, 0o600)

    def _recover(self) -> None:
        with self.lock, self._connect() as connection:
            active = connection.execute(
                "SELECT * FROM swaps WHERE state NOT IN ('queued','completed','failed') "
                "ORDER BY started_at LIMIT 1").fetchone()
        if active is not None and active["exec_id"]:
            threading.Thread(
                target=self._finish_run,
                args=(active["ui_swap_id"], active["run_id"], active["exec_id"],
                      active["direction"]),
                daemon=True,
            ).start()
        elif active is None:
            with self.lock, self._connect() as connection:
                self._maybe_start_next(connection)

    @staticmethod
    def _wallet(wallet_id: object, role: str) -> dict:
        wallet = WALLETS.get(str(wallet_id))
        if wallet is None or wallet["role"] != role:
            raise ValueError(f"select a known local {role} wallet")
        return wallet

    @staticmethod
    def _request(params: dict, method: str) -> tuple[str, str]:
        request_id = params.get("request_id")
        if not isinstance(request_id, str) or not REQUEST_RE.fullmatch(request_id):
            raise ValueError("request identity is invalid")
        fingerprint = hashlib.sha256(compact(params).encode()).hexdigest()
        return request_id, fingerprint

    @staticmethod
    def _record_request(connection: sqlite3.Connection, request_id: str,
                        method: str, fingerprint: str) -> bool:
        existing = connection.execute(
            "SELECT method, fingerprint FROM requests WHERE request_id = ?",
            (request_id,),
        ).fetchone()
        if existing is not None:
            if existing["method"] != method or existing["fingerprint"] != fingerprint:
                raise ValueError("request identity was already used for different facts")
            return True
        connection.execute(
            "INSERT INTO requests(request_id,method,fingerprint,recorded_at) "
            "VALUES(?,?,?,?)", (request_id, method, fingerprint, utc_now()))
        return False

    @staticmethod
    def _offer(row: sqlite3.Row) -> dict:
        direction = DIRECTIONS[row["direction"]]
        # Amount display is from the Taker's perspective: what a taking
        # wallet pays and receives for this offer.
        taker_pays_bitcoin = row["direction"] == "taker_sells_foreign"
        return {
            "offer_id": row["offer_id"],
            "maker_wallet_id": row["maker_wallet_id"],
            "maker_wallet_label": WALLETS[row["maker_wallet_id"]]["label"],
            "state": row["state"],
            "bitcoin_sats": row["bitcoin_sats"],
            "bitcoin_display": "0.01000000 BTC",
            "lez_units": row["lez_units"],
            "lez_display": "1,000 LEZ",
            "direction": row["direction"],
            "direction_display": direction["display"],
            "taker_pays_display": "0.01000000 BTC" if taker_pays_bitcoin else "1,000 LEZ",
            "taker_receives_display": "1,000 LEZ" if taker_pays_bitcoin else "0.01000000 BTC",
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
            "taker_wallet_id": row["taker_wallet_id"],
            "ui_swap_id": row["ui_swap_id"],
        }

    @staticmethod
    def _swap(row: sqlite3.Row) -> dict:
        spec = DIRECTIONS[row["direction"]]
        action = row["action_required"]
        action_spec = spec["actions"].get(action, {}) if action else {}
        return {
            "ui_swap_id": row["ui_swap_id"],
            "protocol_swap_id": row["protocol_swap_id"],
            "offer_id": row["offer_id"],
            "maker_wallet_id": row["maker_wallet_id"],
            "maker_wallet_label": WALLETS[row["maker_wallet_id"]]["label"],
            "taker_wallet_id": row["taker_wallet_id"],
            "taker_wallet_label": WALLETS[row["taker_wallet_id"]]["label"],
            "direction": row["direction"],
            "direction_display": spec["display"],
            "state": row["state"],
            "state_label": spec["state_labels"].get(row["state"], row["state"]),
            "progress_percent": spec["progress"].get(row["state"], 0),
            "progress_detail": None,
            "eta_seconds": None,
            "eta_display": None,
            "action_required": action,
            "action_role": action_spec.get("role"),
            "action_label": action_spec.get("label"),
            "run_id": row["run_id"],
            "started_at": row["started_at"],
            "completed_at": row["completed_at"],
            "error": row["error"],
        }

    def _live_progress(self, swap: dict) -> None:
        state = swap["state"]
        spec = DIRECTIONS[swap["direction"]]
        ready_details = {action["ready_state"]: (action["role"], action["label"])
                         for action in spec["actions"].values()}
        if state in ready_details:
            role_needed, label = ready_details[state]
            if swap.get("can_act"):
                swap["progress_detail"] = f"Your move — {label} is ready and waits for your click"
            else:
                desk = "Maker" if role_needed == "maker" else "Taker"
                swap["progress_detail"] = (
                    f"No timer here — the open gate waits for the {desk} desk to click {label}")
            return
        run_id = swap.get("run_id")
        if not run_id or state not in spec["work_phases"] and state != "preparing":
            return
        base = EVIDENCE_ROOT / ".e2e" / run_id / "m3-actor-poc"
        now = time.time()
        if state == "preparing":
            percent = spec["progress"]["preparing"]
            detail = "Preparing the swap on the settlement chains"
            remaining = 25.0
            marker_time = None
            for relative, label, checkpoint_percent, checkpoint_remaining in \
                    spec["prep_checkpoints"]:
                try:
                    mtime = (base / relative).stat().st_mtime
                except OSError:
                    continue
                percent, detail = checkpoint_percent, label
                remaining, marker_time = checkpoint_remaining, mtime
            if marker_time is None:
                try:
                    marker_time = dt.datetime.fromisoformat(
                        str(swap["started_at"]).replace("Z", "+00:00")).timestamp()
                except (TypeError, ValueError):
                    marker_time = now
            since_marker = max(0.0, now - marker_time)
            # Creep toward the next checkpoint so the bar visibly moves even
            # while a slow step (for example a cold Rust build) is running.
            percent = min(spec["progress"]["awaiting_taker_lock"] - 1,
                          percent + min(5, int(since_marker / 15)))
            eta = remaining - since_marker
        else:
            expected, detail = spec["work_phases"][state]
            marker_name = spec["work_markers"][state]
            try:
                if marker_name is None:
                    marker_time = (base / "evidence" / "m3-actor-local-poc.json")\
                        .stat().st_mtime
                else:
                    marker_time = (base / "private" / "directions" / swap["direction"]
                                   / "interactive-gates" / f"{marker_name}.permit.json")\
                        .stat().st_mtime
            except OSError:
                marker_time = now
            elapsed = max(0.0, now - marker_time)
            anchor = spec["progress"][state]
            next_anchor = spec["progress"][spec["flow"][spec["flow"].index(state) + 1]]
            percent = int(anchor + (next_anchor - 1 - anchor) * min(0.95, elapsed / expected))
            eta = expected - elapsed
        swap["progress_percent"] = percent
        swap["progress_detail"] = detail
        swap["eta_seconds"] = max(5, int(eta))
        swap["eta_display"] = (
            "running long · still working" if eta < -30 else format_eta(eta))

    @staticmethod
    def _gate_root(run_id: str, direction: str) -> pathlib.Path:
        return (EVIDENCE_ROOT / ".e2e" / run_id / "m3-actor-poc" / "private" /
                "directions" / direction / "interactive-gates")

    def _sync_active(self, connection: sqlite3.Connection) -> None:
        row = connection.execute(
            "SELECT * FROM swaps WHERE state NOT IN ('queued','completed','failed') "
            "ORDER BY started_at LIMIT 1").fetchone()
        if row is None or not row["run_id"]:
            return
        run_id = row["run_id"]
        direction = row["direction"]
        spec = DIRECTIONS[direction]
        gate_root = self._gate_root(run_id, direction)
        selected_state = row["state"]
        selected_action = row["action_required"]
        for action in spec["ordered"]:
            ready = gate_root / f"{action}.ready.json"
            permit = gate_root / f"{action}.permit.json"
            if ready.is_file():
                action_spec = spec["actions"][action]
                selected_state = action_spec[
                    "working_state" if permit.is_file() else "ready_state"]
                selected_action = None if permit.is_file() else action
        evidence = (EVIDENCE_ROOT / ".e2e" / run_id / "m3-actor-poc" /
                    "evidence" / "m3-actor-local-poc.json")
        if evidence.is_file():
            selected_state, selected_action = "publishing", None
        plan = (EVIDENCE_ROOT / ".e2e" / run_id / "m3-actor-poc" / "private" /
                "directions" / direction / "application" / "btc-plan.json")
        protocol_swap_id = row["protocol_swap_id"]
        if plan.is_file() and not protocol_swap_id:
            try:
                candidate = json.loads(plan.read_text()).get("swap_id")
                if isinstance(candidate, str) and re.fullmatch(r"[0-9a-f]{64}", candidate):
                    protocol_swap_id = candidate
            except (OSError, json.JSONDecodeError):
                pass
        if (selected_state != row["state"] or selected_action != row["action_required"]
                or protocol_swap_id != row["protocol_swap_id"]):
            connection.execute(
                "UPDATE swaps SET state=?, action_required=?, protocol_swap_id=? "
                "WHERE ui_swap_id=?",
                (selected_state, selected_action, protocol_swap_id, row["ui_swap_id"]),
            )

    def snapshot(self, params: dict) -> dict:
        role = params.get("role")
        if role not in ("maker", "taker"):
            raise ValueError("role must be maker or taker")
        wallet = self._wallet(params.get("wallet_id"), role)
        with self.lock, self._connect() as connection:
            self._sync_active(connection)
            offers = connection.execute(
                "SELECT * FROM offers ORDER BY created_at DESC, offer_id DESC LIMIT 200"
            ).fetchall()
            swaps = connection.execute(
                "SELECT * FROM swaps ORDER BY COALESCE(started_at,'') DESC, ui_swap_id DESC "
                "LIMIT 200"
            ).fetchall()
            queue_positions = {
                row["ui_swap_id"]: position + 1
                for position, row in enumerate(connection.execute(
                    "SELECT ui_swap_id FROM swaps WHERE state='queued' ORDER BY rowid"
                ).fetchall())
            }
            wallet_counts = {}
            for wallet_entry in MAKER_WALLETS + TAKER_WALLETS:
                wallet_id = wallet_entry["id"]
                if wallet_entry["role"] == "maker":
                    pending = connection.execute(
                        "SELECT count(*) FROM offers WHERE maker_wallet_id=? AND state='pending'",
                        (wallet_id,),
                    ).fetchone()[0]
                    active = connection.execute(
                        "SELECT count(*) FROM swaps WHERE maker_wallet_id=? "
                        "AND state NOT IN ('completed','failed')", (wallet_id,),
                    ).fetchone()[0]
                else:
                    pending = 0
                    active = connection.execute(
                        "SELECT count(*) FROM swaps WHERE taker_wallet_id=? "
                        "AND state NOT IN ('completed','failed')", (wallet_id,),
                    ).fetchone()[0]
                # An open gate belongs to exactly one wallet. Surfacing it per
                # wallet lets a desk point at whichever account is waiting,
                # instead of showing an empty NEEDS YOU for the selected one.
                role_actions = ROLE_ACTIONS[wallet_entry["role"]]
                column = ("maker_wallet_id" if wallet_entry["role"] == "maker"
                          else "taker_wallet_id")
                waiting = connection.execute(
                    f"SELECT count(*) FROM swaps WHERE {column}=? "
                    f"AND action_required IN ({','.join('?' * len(role_actions))})",
                    (wallet_id, *role_actions),
                ).fetchone()[0]
                wallet_counts[wallet_id] = {"pending_offers": pending,
                                            "active_swaps": active,
                                            "needs_action": waiting}
        runner = runner_info()
        public_wallets = []
        for entry in MAKER_WALLETS if role == "maker" else TAKER_WALLETS:
            public_wallets.append(dict(entry, **wallet_counts[entry["id"]]))
        inventory = [self._offer(row) for row in offers
                     if row["maker_wallet_id"] == wallet["id"]]
        order_book = [self._offer(row) for row in offers if row["state"] == "pending"]
        relevant_swaps = [self._swap(row) for row in swaps
                          if row[f"{role}_wallet_id"] == wallet["id"]]
        active_runs = sum(
            row["state"] not in ("queued", "completed", "failed") for row in swaps)
        for swap in relevant_swaps:
            swap["can_act"] = (
                swap["action_role"] == role
                and swap[f"{role}_wallet_id"] == wallet["id"]
            )
            if swap["state"] == "queued":
                position = queue_positions.get(swap["ui_swap_id"], 1)
                swap["progress_detail"] = (
                    f"Position {position} in line — starts automatically when the "
                    "active swap finishes" if active_runs
                    else "Starting shortly — the runner is picking this up")
            else:
                self._live_progress(swap)
            if swap["state"] == "completed" and isinstance(swap.get("run_id"), str):
                swap["effects"] = run_effects(swap["run_id"])
        latest_balance_evidence = None
        try:
            published = json.loads(EVIDENCE_OUTPUT.read_text())
            balance_changes = published.get("wallet_balance_changes", {})
            for entry in balance_changes.get("wallets", []):
                if entry.get("wallet_id") == wallet["id"] and entry.get("role") == role:
                    latest_balance_evidence = {
                        "run_id": published.get("run_id"),
                        "direction": published.get("direction"),
                        "completed_at": published.get("completed_at"),
                        "wallet": entry,
                        "reconciliation": balance_changes.get("reconciliation"),
                    }
                    break
        except (OSError, ValueError, TypeError):
            latest_balance_evidence = None
        direction_catalog = [
            {
                "direction": spec["name"],
                "display": spec["display"],
                "ui_direction": spec["ui_direction"],
                "bitcoin_sats": FIXED_BITCOIN_SATS,
                "bitcoin_display": "0.01000000 BTC",
                "lez_units": FIXED_LEZ_UNITS,
                "lez_display": "1,000 LEZ",
                "maker_label": "Sell " + (
                    "1,000 LEZ for 0.01000000 BTC"
                    if spec["name"] == "taker_sells_foreign"
                    else "0.01000000 BTC for 1,000 LEZ"),
                "maker_actions": [spec["actions"][name]["label"]
                                  for name in spec["ordered"]
                                  if spec["actions"][name]["role"] == "maker"],
                "taker_actions": [spec["actions"][name]["label"]
                                  for name in spec["ordered"]
                                  if spec["actions"][name]["role"] == "taker"],
            }
            for spec in DIRECTIONS.values()
        ]
        return {
            "schema_version": 2,
            "kind": "m3_btc_wallet_market",
            "role": role,
            "selected_wallet_id": wallet["id"],
            "wallets": public_wallets,
            "inventory": inventory,
            "order_book": order_book,
            "swaps": relevant_swaps,
            "latest_balance_evidence": latest_balance_evidence,
            "summary": {
                "pending_offers": sum(row["state"] == "pending" for row in offers),
                "accepted_swaps": len(swaps),
                "completed_swaps": sum(row["state"] == "completed" for row in swaps),
            },
            "preset": {
                "bitcoin_sats": FIXED_BITCOIN_SATS,
                "bitcoin_display": "0.01000000 BTC",
                "lez_units": FIXED_LEZ_UNITS,
                "lez_display": "1,000 LEZ",
                "direction": "BTC → LEZ",
            },
            "directions": direction_catalog,
            "runner_ready": runner["ready"],
            "runner_busy": runner["busy"],
            "runner_detail": runner["reason"],
        }

    def create_offers(self, params: dict) -> dict:
        wallet = self._wallet(params.get("wallet_id"), "maker")
        request_id, fingerprint = self._request(params, "create_offers")
        count = params.get("count")
        # The direction is carried either as an explicit parameter (the
        # rebuilt Maker plugin) or, for the original plugin signature, inside
        # the request identity segment ("sell-btc" / "sell-lez").
        direction = params.get("direction")
        if direction is None:
            direction = ("taker_sells_lez" if "-sell-lez-" in request_id
                         else "taker_sells_foreign")
        if direction not in DIRECTIONS:
            raise ValueError("the local market supports the two fixed swap directions")
        if not isinstance(count, int) or isinstance(count, bool) \
                or count != MAX_OFFERS_PER_REQUEST:
            raise ValueError("publish exactly one offer per request")
        if params.get("bitcoin_sats") != FIXED_BITCOIN_SATS \
                or params.get("lez_units") != FIXED_LEZ_UNITS:
            raise ValueError("the local market uses its exact BTC/LEZ preset")
        with self.lock, self._connect() as connection:
            replay = self._record_request(
                connection, request_id, "create_offers", fingerprint)
            if not replay:
                open_count = connection.execute(
                    "SELECT count(*) FROM offers WHERE maker_wallet_id=? AND state='pending'",
                    (wallet["id"],),
                ).fetchone()[0]
                if open_count + count > MAX_OPEN_OFFERS_PER_WALLET:
                    raise ValueError("this wallet may hold at most twenty pending offers")
                stamp = utc_now()
                digest = hashlib.sha256(
                    f"{request_id}:{direction}".encode()).hexdigest()[:12]
                suffix = "munich" if wallet["id"] == "maker-munich-01" else "basel"
                for index in range(1, count + 1):
                    offer_id = f"m3btc-{suffix}-{digest}-{index}"
                    connection.execute(
                        "INSERT INTO offers(offer_id,maker_wallet_id,state,"
                        "bitcoin_sats,lez_units,created_at,updated_at,direction) "
                        "VALUES(?,?,?,?,?,?,?,?)",
                        (offer_id, wallet["id"], "pending", FIXED_BITCOIN_SATS,
                         FIXED_LEZ_UNITS, stamp, stamp, direction),
                    )
        return self.snapshot({"role": "maker", "wallet_id": wallet["id"]})

    def withdraw_offer(self, params: dict) -> dict:
        wallet = self._wallet(params.get("wallet_id"), "maker")
        request_id, fingerprint = self._request(params, "withdraw_offer")
        offer_id = params.get("offer_id")
        if not isinstance(offer_id, str) or not OFFER_RE.fullmatch(offer_id):
            raise ValueError("offer identity is invalid")
        with self.lock, self._connect() as connection:
            replay = self._record_request(
                connection, request_id, "withdraw_offer", fingerprint)
            if not replay:
                changed = connection.execute(
                    "UPDATE offers SET state='withdrawn', updated_at=? "
                    "WHERE offer_id=? AND maker_wallet_id=? AND state='pending'",
                    (utc_now(), offer_id, wallet["id"]),
                ).rowcount
                if changed != 1:
                    raise ValueError("only this wallet's pending offer can be withdrawn")
        return self.snapshot({"role": "maker", "wallet_id": wallet["id"]})

    def take_offer(self, params: dict) -> dict:
        wallet = self._wallet(params.get("wallet_id"), "taker")
        request_id, fingerprint = self._request(params, "take_offer")
        offer_id = params.get("offer_id")
        if not isinstance(offer_id, str) or not OFFER_RE.fullmatch(offer_id):
            raise ValueError("offer identity is invalid")
        with self.lock, self._connect() as connection:
            replay = self._record_request(connection, request_id, "take_offer", fingerprint)
            if not replay:
                offer = connection.execute(
                    "SELECT * FROM offers WHERE offer_id=?", (offer_id,)).fetchone()
                if offer is None or offer["state"] != "pending":
                    raise ValueError("the selected offer is no longer pending")
                swap_digest = hashlib.sha256(
                    f"{offer_id}:{wallet['id']}".encode()).hexdigest()[:16]
                ui_swap_id = f"swap-{swap_digest}"
                connection.execute(
                    "UPDATE offers SET state='reserved', taker_wallet_id=?, "
                    "ui_swap_id=?, updated_at=? WHERE offer_id=? AND state='pending'",
                    (wallet["id"], ui_swap_id, utc_now(), offer_id),
                )
                connection.execute(
                    "INSERT INTO swaps(ui_swap_id,offer_id,maker_wallet_id,"
                    "taker_wallet_id,state,direction) VALUES(?,?,?,?,?,?)",
                    (ui_swap_id, offer_id, offer["maker_wallet_id"],
                     wallet["id"], "queued", offer["direction"]),
                )
            self._maybe_start_next(connection)
        return self.snapshot({"role": "taker", "wallet_id": wallet["id"]})

    def action(self, params: dict) -> dict:
        role = params.get("role")
        if role not in ("maker", "taker"):
            raise ValueError("role must be maker or taker")
        wallet = self._wallet(params.get("wallet_id"), role)
        request_id, fingerprint = self._request(params, "swap_action")
        ui_swap_id = params.get("ui_swap_id")
        action = params.get("action")
        if not isinstance(ui_swap_id, str) or not SWAP_RE.fullmatch(ui_swap_id):
            raise ValueError("swap identity is invalid")
        with self.lock, self._connect() as connection:
            self._sync_active(connection)
            replay = self._record_request(connection, request_id, "swap_action", fingerprint)
            row = connection.execute(
                "SELECT * FROM swaps WHERE ui_swap_id=?", (ui_swap_id,)).fetchone()
            if row is None or row[f"{role}_wallet_id"] != wallet["id"]:
                raise ValueError("the selected wallet does not own this swap action")
            spec = DIRECTIONS[row["direction"]]
            if action not in spec["actions"] or spec["actions"][action]["role"] != role:
                raise ValueError("that actor action is unavailable for this role")
            if not replay:
                action_spec = spec["actions"][action]
                if row["action_required"] != action \
                        or row["state"] != action_spec["ready_state"]:
                    raise ValueError("the requested action is not ready at this swap revision")
                run_id = row["run_id"]
                if not isinstance(run_id, str) or not RUN_RE.fullmatch(run_id):
                    raise RuntimeError("active run identity is unavailable")
                revision = spec["ordered"].index(action)
                launcher_call({
                    "operation": "approve_action", "run_id": run_id,
                    "direction": row["direction"], "role": role, "action": action,
                    "expected_revision": revision, "approved_at": utc_now(),
                })
                connection.execute(
                    "UPDATE swaps SET state=?, action_required=NULL WHERE ui_swap_id=?",
                    (action_spec["working_state"], ui_swap_id),
                )
        return self.snapshot({"role": role, "wallet_id": wallet["id"]})

    def _maybe_start_next(self, connection: sqlite3.Connection) -> None:
        active = connection.execute(
            "SELECT count(*) FROM swaps WHERE state NOT IN ('queued','completed','failed')"
        ).fetchone()[0]
        if active:
            return
        queued = connection.execute(
            "SELECT * FROM swaps WHERE state='queued' ORDER BY rowid LIMIT 1").fetchone()
        if queued is None:
            return
        runner = runner_info()
        if not runner["ready"] or runner["busy"]:
            return
        direction = queued["direction"]
        run_id = ""
        now = dt.datetime.now(dt.timezone.utc)
        for offset in range(60):
            candidate = "m5arm-" + (now + dt.timedelta(seconds=offset)).strftime("%m%d%H%M%S")
            used = connection.execute(
                "SELECT 1 FROM swaps WHERE run_id=?", (candidate,)).fetchone()
            if used is None and not EVIDENCE_ROOT.joinpath(".e2e", candidate).exists():
                run_id = candidate
                break
        if not RUN_RE.fullmatch(run_id):
            raise RuntimeError("a unique local run identity is unavailable")
        scripts = interactive_runner_scripts(run_id, direction)
        reservation = "ui-reserve-" + hashlib.sha256(
            queued["ui_swap_id"].encode()).hexdigest()[:20]
        run_result = launcher_call({
            "operation": "run_swap", "run_id": run_id, "direction": direction,
            "offer_id": queued["offer_id"], "reservation_id": reservation,
            "maker_wallet_id": queued["maker_wallet_id"],
            "taker_wallet_id": queued["taker_wallet_id"],
            "files": {
                name: base64.b64encode(data).decode("ascii")
                for name, data in scripts.items()
            },
        })
        if run_result.get("kind") != "RunSwapResultV1" \
                or run_result.get("run_id") != run_id:
            raise RuntimeError("runner returned an invalid RunSwapResultV1")
        exec_id = str(run_result.get("exec_id", ""))
        connection.execute(
            "UPDATE swaps SET state='preparing',run_id=?,exec_id=?,started_at=?,error=NULL "
            "WHERE ui_swap_id=?",
            (run_id, exec_id, utc_now(), queued["ui_swap_id"]),
        )
        connection.commit()
        threading.Thread(
            target=self._finish_run,
            args=(queued["ui_swap_id"], run_id, exec_id, direction),
            daemon=True,
        ).start()

    def _finish_run(self, ui_swap_id: str, run_id: str, exec_id: str,
                    direction: str) -> None:
        try:
            exit_code = wait_exec(exec_id)
            if exit_code != 0:
                raise RuntimeError(f"interactive M3 runner exited with status {exit_code}")
            export_result = launcher_call({
                "operation": "collect_result", "run_id": run_id,
                "direction": direction,
            })
            if export_result.get("kind") != "CollectSwapResultV1":
                raise RuntimeError("public evidence export failed")
            export_code = export_result.get("exit_code")
            if export_code is None:
                export_code = wait_exec(str(export_result.get("exec_id", "")))
            if export_code != 0:
                raise RuntimeError("public evidence export failed")
            generated_on_mount = (
                EVIDENCE_ROOT / ".e2e" / run_id / "m3-actor-poc" /
                "evidence" / "m3-btc-ui-evidence.json"
            )
            evidence = json.loads(generated_on_mount.read_text())
            validate_public_evidence(evidence, run_id, direction)
            balances_path = (
                EVIDENCE_ROOT / ".e2e" / run_id / "m3-actor-poc" / "evidence" /
                f"{direction}-interactive-wallet-balances.json"
            )
            balances = json.loads(balances_path.read_text())
            if not (
                balances.get("kind") == "m3_interactive_wallet_balance_changes"
                and balances.get("run_id") == run_id
                and balances.get("direction") == direction
                and balances.get("private_material_disclosed") is False
                and len(balances.get("wallets", [])) == 2
                and balances.get("reconciliation", {}).get("lez_conserved") is True
            ):
                raise RuntimeError("wallet balance evidence failed reconciliation")
            with self.lock, self._connect() as connection:
                row = connection.execute(
                    "SELECT * FROM swaps WHERE ui_swap_id=?", (ui_swap_id,)).fetchone()
                if row is None:
                    raise RuntimeError("interactive swap record disappeared")
                evidence["market_context"] = {
                    "ui_swap_id": ui_swap_id,
                    "offer_id": row["offer_id"],
                    "direction": direction,
                    "direction_display": DIRECTIONS[direction]["display"],
                    "maker_wallet_id": row["maker_wallet_id"],
                    "maker_wallet_label": WALLETS[row["maker_wallet_id"]]["label"],
                    "taker_wallet_id": row["taker_wallet_id"],
                    "taker_wallet_label": WALLETS[row["taker_wallet_id"]]["label"],
                    "interaction_model": "two_dashboard_four_actor_actions",
                    "wallet_authority_model":
                        "fresh_run_owned_keys_indexed_by_local_wallet_profile",
                }
                evidence["wallet_balance_changes"] = balances
                with EVIDENCE_OUTPUT.open("w") as destination:
                    json.dump(evidence, destination, indent=2)
                    destination.write("\n")
                os.chmod(EVIDENCE_OUTPUT, 0o666)
                connection.execute(
                    "UPDATE swaps SET state='completed',action_required=NULL,completed_at=?,"
                    "error=NULL WHERE ui_swap_id=?", (utc_now(), ui_swap_id))
                connection.execute(
                    "UPDATE offers SET state='completed',updated_at=? WHERE offer_id=?",
                    (utc_now(), row["offer_id"]),
                )
                self._maybe_start_next(connection)
        except Exception as error:
            with self.lock, self._connect() as connection:
                row = connection.execute(
                    "SELECT offer_id FROM swaps WHERE ui_swap_id=?", (ui_swap_id,)).fetchone()
                connection.execute(
                    "UPDATE swaps SET state='failed',action_required=NULL,error=? "
                    "WHERE ui_swap_id=?", (str(error)[:300], ui_swap_id))
                if row is not None:
                    connection.execute(
                        "UPDATE offers SET state='failed',updated_at=? WHERE offer_id=?",
                        (utc_now(), row["offer_id"]),
                    )
                self._maybe_start_next(connection)


MARKET = Market()


class RpcHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, _format: str, *_args: object) -> None:
        return

    def _reply(self, body: dict) -> None:
        encoded = compact(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(encoded)

    def do_POST(self) -> None:
        request_id = 1
        try:
            if self.path != "/":
                raise ValueError("invalid local RPC path")
            length = int(self.headers.get("Content-Length", "0"))
            if length <= 0 or length > 65536:
                raise ValueError("invalid request length")
            request = json.loads(self.rfile.read(length))
            request_id = request.get("id", 1)
            params_list = request.get("params", [])
            if request.get("jsonrpc") != "2.0" or request_id != 1 \
                    or not isinstance(params_list, list) or len(params_list) != 1 \
                    or not isinstance(params_list[0], dict):
                raise ValueError("invalid JSON-RPC request")
            method = request.get("method")
            params = params_list[0]
            if params.get("schema_version") != 2:
                raise ValueError("unsupported wallet-market schema version")
            if method == "btc_market_health_v1":
                runner = runner_info()
                result = {
                    "schema_version": 1,
                    "ready": runner["ready"],
                    "degraded": not runner["ready"],
                    "runner": runner,
                }
            elif method == "btc_market_snapshot_v1":
                result = MARKET.snapshot(params)
            elif method == "btc_offer_create_v1":
                result = MARKET.create_offers(params)
            elif method == "btc_offer_withdraw_v1":
                result = MARKET.withdraw_offer(params)
            elif method == "btc_offer_take_v1":
                result = MARKET.take_offer(params)
            elif method == "btc_swap_action_v1":
                result = MARKET.action(params)
            else:
                raise ValueError("unknown method")
            self._reply({"jsonrpc": "2.0", "id": request_id, "result": result})
        except ValueError as error:
            self._reply({"jsonrpc": "2.0", "id": request_id,
                         "error": {"code": -32602, "message": str(error)}})
        except Exception as error:
            self._reply({"jsonrpc": "2.0", "id": request_id,
                         "error": {"code": -32000, "message": str(error)[:300]}})


class UnixHttpServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    address_family = socket.AF_UNIX
    daemon_threads = True

    def server_bind(self) -> None:
        socketserver.TCPServer.server_bind(self)
        self.server_name = "localhost"
        self.server_port = 0


def main() -> None:
    SOCKET_PATH.parent.mkdir(parents=True, exist_ok=True)
    try:
        SOCKET_PATH.unlink()
    except FileNotFoundError:
        pass
    server = UnixHttpServer(str(SOCKET_PATH), RpcHandler)
    os.chmod(SOCKET_PATH, 0o600)
    server.serve_forever()


if __name__ == "__main__":
    main()
