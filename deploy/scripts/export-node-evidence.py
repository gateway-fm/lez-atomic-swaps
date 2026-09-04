#!/usr/bin/env python3
"""Publish the public evidence of a swap the two Nodes settled.

The Taker actor's durable aggregate names four public transactions (its
Bitcoin lock, the Maker's LEZ funding, its own LEZ revealing claim, the Maker's
Bitcoin follow-up claim); the Maker's escrow preparation names the fifth (the
LEZ escrow initialization). Each is confirmed against its chain here, and the
result is written as the `m3_btc_ui_evidence` document that the LEZ explorer
and the Taker desk's proof view validate: `runtime/evidence/<swap>.json` for
the explorer's hash index and `runtime/m3-btc-ui-evidence.json` for the proof
view. No key, nonce or adaptor secret leaves the Nodes.

Usage (from deploy/): export-node-evidence.py [--swap SWAP_ID | --all]
Without arguments the newest completed swap is exported.
"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import subprocess
import sys
import urllib.request

DEPLOY_ROOT = pathlib.Path(__file__).resolve().parent.parent
TAKER_SOCKET = "/run/lez/taker/node.sock"
TAKER_SWAPS = "/var/lib/lez/taker/btc/swaps"
MAKER_SWAPS = "/var/lib/lez/maker/btc/swaps"
INDEXER = "http://127.0.0.1:8779"
LEZ_EXPLORER = "http://127.0.0.1:3003"
BTC_CLI = ["docker", "exec", "lez-bitcoin-core", "bitcoin-cli",
           "-conf=/run-config/bitcoin.conf", "-datadir=/var/lib/bitcoin"]


def fail(message: str) -> None:
    print(f"export-node-evidence: {message}", file=sys.stderr)
    sys.exit(1)


def docker(container: str, *command: str, stdin: str | None = None) -> str:
    result = subprocess.run(["docker", "exec", "-i", container, *command], input=stdin,
                            capture_output=True, text=True, check=False)
    if result.returncode != 0:
        fail(f"{container}: {' '.join(command[:2])} failed: {result.stderr.strip()[:300]}")
    return result.stdout


def taker_rpc(method: str, params: dict) -> dict:
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": [params]})
    raw = docker("lez-taker-node", "curl", "-sS", "--max-time", "30", "--unix-socket", TAKER_SOCKET,
                 "-H", "content-type: application/json", "--data", body, "http://localhost/")
    reply = json.loads(raw)
    if "error" in reply:
        fail(f"{method}: {reply['error']}")
    return reply["result"]


def indexer(method: str, params: list) -> object:
    request = urllib.request.Request(
        INDEXER, data=json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode(),
        headers={"content-type": "application/json"})
    with urllib.request.urlopen(request, timeout=30) as response:
        reply = json.loads(response.read())
    if "error" in reply:
        fail(f"indexer {method}: {reply['error']}")
    return reply["result"]


def bitcoin(*command: str) -> object:
    result = subprocess.run(BTC_CLI + list(command), capture_output=True, text=True, check=False)
    if result.returncode != 0:
        fail(f"bitcoin-cli {command[0]} failed: {result.stderr.strip()[:300]}")
    return json.loads(result.stdout)


# The Taker Node keeps each swap under its reservation directory; the
# aggregate snapshot lives in that directory's actor store. One Python run
# inside the container finds the directory for the swap id and reads both.
TAKER_READER = r"""
import glob, json, sqlite3, sys
wanted = sys.argv[1]
for record in glob.glob(sys.argv[2] + "/*/taker-swap.json"):
    with open(record) as handle:
        swap = json.load(handle)
    # A take that never reached its reservation has no swap id yet.
    if not swap.get("swap_id") or bytes(swap["swap_id"]).hex() != wanted:
        continue
    directory = record.rsplit("/", 1)[0]
    store = sqlite3.connect("file:" + directory + "/actor/state.sqlite3?mode=ro", uri=True)
    row = store.execute("select revision, snapshot_json from btc_actor_aggregates where swap_id = ?",
                        (wanted,)).fetchone()
    if row is None:
        row = store.execute("select revision, snapshot_json from btc_actor_aggregates").fetchone()
    print(json.dumps({"directory": directory.rsplit("/", 1)[1], "revision": row[0],
                      "snapshot": json.loads(row[1]), "direction": swap.get("direction")}))
    break
else:
    print("{}")
"""


def taker_aggregate(swap_id: str) -> dict:
    raw = docker("lez-taker-node", "python3", "-c", TAKER_READER, swap_id, TAKER_SWAPS)
    found = json.loads(raw or "{}")
    if not found:
        fail(f"the Taker Node has no swap directory for {swap_id[:12]}")
    return found


def maker_initialization(directory: str) -> str:
    raw = docker("lez-maker-node", "cat", f"{MAKER_SWAPS}/{directory}/lez/prepared-escrow.json")
    prepared = json.loads(raw)
    return prepared["initialization"]["transaction_id"]


def bitcoin_facts(txid: str) -> tuple[dict, int, str]:
    transaction = bitcoin("getrawtransaction", txid, "1")
    block_hash = transaction.get("blockhash")
    if not block_hash:
        fail(f"Bitcoin transaction {txid[:12]} is not in a block yet")
    header = bitcoin("getblockheader", block_hash)
    return transaction, int(header["height"]), block_hash


def btc_display(value: float) -> str:
    return f"{value:.8f} BTC"


class LezIndex:
    """Locates LEZ transactions by scanning the block window of the swap."""

    def __init__(self, from_unix: int, to_unix: int):
        self.blocks: dict[str, tuple[int, str, str]] = {}
        head = indexer("getBlocks", [None, 1])[0]["header"]["block_id"]
        # Block ids are dense and timestamps monotonic: binary-search the first
        # block at or after the window start, then read forward to its end.
        low, high = 0, head
        start_ms = (from_unix - 120) * 1000
        while low < high:
            middle = (low + high) // 2
            block = indexer("getBlockById", [middle])
            if block["header"]["timestamp"] < start_ms:
                low = middle + 1
            else:
                high = middle
        end_ms = (to_unix + 600) * 1000
        cursor = low
        while cursor <= head:
            # getBlocks(from, count) returns the blocks below `from` and
            # nothing at all when `from` lies beyond the head.
            page = indexer("getBlocks", [min(cursor + 50, head + 1), 50])
            page = [b for b in page if b["header"]["block_id"] >= cursor]
            if not page:
                break
            for block in page:
                header = block["header"]
                for transaction in block["body"]["transactions"]:
                    inner = next(iter(transaction.values()))
                    self.blocks[inner["hash"]] = (header["block_id"], header["hash"], block["bedrock_status"])
            cursor = max(b["header"]["block_id"] for b in page) + 1
            if min(b["header"]["timestamp"] for b in page) > end_ms:
                break

    def facts(self, hash_hex: str) -> tuple[int, str, str]:
        if hash_hex not in self.blocks:
            fail(f"LEZ transaction {hash_hex[:12]} was not found in the swap's block window")
        return self.blocks[hash_hex]


def build_evidence(view: dict, repository_commit: str) -> dict:
    swap_id = view["swap_id"]
    if view["state"] != "completed":
        fail(f"swap {swap_id[:12]} is {view['state']}, not completed")
    if view["route"]["direction"] != "TakerSellsForeign":
        fail("only TakerSellsForeign swaps are exported")
    aggregate = taker_aggregate(swap_id)
    snapshot = aggregate["snapshot"]
    if snapshot.get("phase") != "Completed" or aggregate["revision"] != 4:
        fail(f"Taker actor aggregate is {snapshot.get('phase')} at revision {aggregate['revision']}")
    ids = {
        "first_lock": snapshot["taker_lock_transaction_id"],
        "funding": snapshot["maker_lock_transaction_id"],
        "revealing_claim": snapshot["revealing_claim_transaction_id"],
        "followup_claim": snapshot["followup_claim_transaction_id"],
        "initialization": maker_initialization(aggregate["directory"]),
    }
    if len(set(ids.values())) != 5 or not all(isinstance(v, str) and len(v) == 64 for v in ids.values()):
        fail("the five public transaction ids are not distinct 32-byte hashes")

    lock_tx, lock_height, lock_block = bitcoin_facts(ids["first_lock"])
    claim_tx, claim_height, claim_block = bitcoin_facts(ids["followup_claim"])
    lez = LezIndex(lock_tx["blocktime"], claim_tx["blocktime"])
    foreign_btc = view["foreign_units"] / 1e8
    lez_units = int(view["lez_units"])
    claimed_btc = sum(output["value"] for output in claim_tx["vout"])

    def bitcoin_effect(sequence: int, actor: str, kind: str, label: str, txid: str,
                       tx: dict, height: int, block: str, amount: str) -> dict:
        return {"sequence": sequence, "chain": "Bitcoin", "actor": actor, "kind": kind, "label": label,
                "transaction_id": txid, "amount": amount, "block_height": height, "block_hash": block,
                "confirmations": int(tx["confirmations"]), "finality": "Confirmed",
                "explorer_url": f"{LEZ_EXPLORER}/#/evidence/tx/{txid}"}

    def lez_effect(sequence: int, actor: str, kind: str, label: str, txid: str, amount: str) -> dict:
        block_id, block_hash, status = lez.facts(txid)
        return {"sequence": sequence, "chain": "LEZ", "actor": actor, "kind": kind, "label": label,
                "transaction_id": txid, "amount": amount, "block_height": block_id, "block_hash": block_hash,
                "confirmations": None, "finality": status,
                "explorer_url": f"{LEZ_EXPLORER}/#/evidence/tx/{txid}"}

    lez_display = f"{lez_units:,} LEZ units"
    effects = [
        bitcoin_effect(1, "Taker", "first_lock", "Taker first lock", ids["first_lock"], lock_tx,
                       lock_height, lock_block, btc_display(foreign_btc)),
        lez_effect(2, "Maker", "initialization", "Escrow initialization", ids["initialization"],
                   "Escrow authority"),
        lez_effect(3, "Maker", "funding", "Maker second lock", ids["funding"], lez_display),
        lez_effect(4, "Taker", "revealing_claim", "Taker revealing claim", ids["revealing_claim"], lez_display),
        bitcoin_effect(5, "Maker", "followup_claim", "Maker follow-up claim", ids["followup_claim"],
                       claim_tx, claim_height, claim_block, btc_display(claimed_btc)),
    ]
    if not all(effect["finality"] in ("Confirmed", "Finalized") for effect in effects):
        fail("a LEZ effect is not finalized yet: " + ", ".join(f"{e['kind']}={e['finality']}" for e in effects))
    completed_at = dt.datetime.fromtimestamp(claim_tx["blocktime"], dt.timezone.utc)
    return {
        "schema_version": 1,
        "kind": "m3_btc_ui_evidence",
        "source_kind": "node_owned_swap",
        "source": "taker_node_actor_state",
        "result": "passed",
        "run_id": f"node-{swap_id[:12]}",
        "swap_id": swap_id,
        "offer_id": view["offer_id"],
        "completed_at": completed_at.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "repository_commit": repository_commit,
        "pair": "Bitcoin",
        "direction": "TakerSellsForeign",
        "journey": "claim",
        "terminal": {"phase": "completed", "revision": 4},
        "amounts": {"bitcoin_sats": int(view["foreign_units"]), "bitcoin_display": btc_display(foreign_btc),
                    "lez_units": lez_units, "lez_display": lez_display},
        "networks": {"bitcoin": "Bitcoin Core 31.1 · regtest", "lez": "LEZ v0.2.0 · private local"},
        "effect_counts": {"bitcoin": 2, "lez": 3, "total": 5},
        "replay_resubmission_count": 0,
        "private_material_disclosed": False,
        "effects": effects,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--swap", help="swap id to export (default: the newest completed swap)")
    parser.add_argument("--all", action="store_true", help="export every completed swap")
    args = parser.parse_args()

    swaps = taker_rpc("taker_swap_list_v1", {"schema_version": 1})["swaps"]
    completed = [swap for swap in swaps if swap["state"] == "completed"]
    if args.swap:
        chosen = [swap for swap in swaps if swap["swap_id"] == args.swap]
        if not chosen:
            fail(f"the Taker Node lists no swap {args.swap[:12]}")
    elif args.all:
        chosen = completed
    else:
        chosen = completed[-1:]
    if not chosen:
        fail("the Taker Node lists no completed swap")
    commit = subprocess.run(["git", "-C", str(DEPLOY_ROOT), "rev-parse", "HEAD"],
                            capture_output=True, text=True, check=False).stdout.strip() or "unknown"

    evidence_dir = DEPLOY_ROOT / "runtime" / "evidence"
    evidence_dir.mkdir(parents=True, exist_ok=True)
    proof_view = DEPLOY_ROOT / "runtime" / "m3-btc-ui-evidence.json"
    for view in chosen:
        evidence = build_evidence(view, commit)
        target = evidence_dir / f"{view['swap_id']}.json"
        target.write_text(json.dumps(evidence, indent=2) + "\n")
        # The proof view file is a bind mount: rewrite in place so the inode
        # the containers opened keeps pointing at the current evidence.
        with proof_view.open("w") as handle:
            handle.write(json.dumps(evidence, indent=2) + "\n")
        print(f"exported swap {view['swap_id'][:12]}: {target.relative_to(DEPLOY_ROOT)} "
              f"({len(evidence['effects'])} effects, completed {evidence['completed_at']})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
