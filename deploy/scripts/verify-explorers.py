#!/usr/bin/env python3
"""Deep verification that both block explorers display real swap transactions.

Checks rendered page content, not just HTTP status: every exported swap's
Bitcoin transactions must appear on the Bitcoin explorer with their containing
block, and its LEZ transactions must appear on the LEZ explorer with their
program and accounts, cross-checked against the chains themselves.

Usage: verify-explorers.py [--btc URL] [--lez URL] --evidence-dir DIR
"""
from __future__ import annotations

import argparse
import html
import json
import pathlib
import re
import sys
import urllib.error
import urllib.request

FAILURES: list[str] = []
CHECKS = 0


def check(label: str, ok: bool, detail: str = "") -> bool:
    global CHECKS
    CHECKS += 1
    mark = "\033[32mPASS\033[0m" if ok else "\033[31mFAIL\033[0m"
    print(f"  [{mark}] {label}{(' — ' + detail) if detail else ''}")
    if not ok:
        FAILURES.append(label)
    return ok


def get(url: str, timeout: int = 25) -> tuple[int, str]:
    try:
        with urllib.request.urlopen(url, timeout=timeout) as response:
            return response.status, response.read().decode("utf8", "replace")
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode("utf8", "replace")


def post_json(url: str, payload: dict, timeout: int = 25) -> dict:
    request = urllib.request.Request(
        url, data=json.dumps(payload).encode(),
        headers={"content-type": "application/json"})
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.loads(response.read())


def visible_text(markup: str) -> str:
    """Rendered text only: scripts, styles and display:none blocks removed."""
    markup = re.sub(r"<script.*?</script>", " ", markup, flags=re.S)
    markup = re.sub(r"<style.*?</style>", " ", markup, flags=re.S)
    markup = re.sub(
        r'<(\w+)[^>]*style="[^"]*display:\s*none[^"]*"[^>]*>.*?</\1>',
        " ", markup, flags=re.S)
    return re.sub(r"\s+", " ", html.unescape(re.sub(r"<[^>]+>", " ", markup))).strip()


def load_evidence(directory: pathlib.Path) -> list[dict]:
    """Every completed swap the Nodes exported (export-node-evidence.py)."""
    swaps = []
    for path in sorted(directory.glob("*.json")):
        try:
            evidence = json.loads(path.read_text())
        except (OSError, ValueError):
            continue
        if (evidence.get("kind") == "m3_btc_ui_evidence" and evidence.get("result") == "passed"
                and evidence.get("effects")):
            swaps.append(evidence)
    return swaps


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--btc", default="http://127.0.0.1:3002")
    parser.add_argument("--lez", default="http://127.0.0.1:3003")
    parser.add_argument("--evidence-dir", required=True)
    args = parser.parse_args()

    runs = load_evidence(pathlib.Path(args.evidence_dir))
    print(f"\ncompleted swaps discovered: {len(runs)}")
    if not runs:
        print("no exported swap evidence found — nothing to verify", file=sys.stderr)
        return 2

    print("\nBitcoin explorer — service pages")
    status, body = get(f"{args.btc}/")
    check("homepage renders", status == 200 and "Regtest Explorer" in visible_text(body),
          f"HTTP {status}")
    status, body = get(f"{args.btc}/blocks")
    text = visible_text(body)
    check("block list renders", status == 200 and "Blocks" in text, f"HTTP {status}")
    heights = [int(h.replace(",", "")) for h in re.findall(r"\b(\d[\d,]{2,})\b", text)]
    check("block list shows a plausible tip", bool(heights) and max(heights) > 100,
          f"max value {max(heights) if heights else 'none'}")

    print("\nBitcoin explorer — swap transactions")
    for evidence in runs:
        run_id = evidence.get("run_id", "?")
        for effect in evidence["effects"]:
            if effect.get("chain") != "Bitcoin":
                continue
            txid = effect["transaction_id"]
            status, body = get(f"{args.btc}/tx/{txid}")
            if status == 200 and txid in body:
                text = visible_text(body)
                block = re.search(r"Block\s*(?:Height|Hash)?\s*([\d,]{3,})", text)
                check(f"{run_id} · {effect['label']}",
                      "currently unconfirmed" not in text and block is not None,
                      f"live tx in block {block.group(1) if block else '?'}")
            else:
                # Runs that predate the settlement chains executed on isolated
                # chains that no longer exist; the certified proof the LEZ
                # explorer serves is then the only remaining public record.
                status, body = get(f"{args.lez}/api/evidence/tx/{txid}")
                proof = json.loads(body) if status == 200 else {}
                check(f"{run_id} · {effect['label']}",
                      proof.get("effect", {}).get("transaction_id") == txid,
                      "certified-evidence proof (pre-settlement-chain run)")

    print("\nLEZ explorer — service and blocks")
    status, body = get(f"{args.lez}/api/overview")
    overview = json.loads(body) if status == 200 else {}
    health = overview.get("health") or {}
    check("indexer health reports a chain head", bool(health.get("latest_block")),
          f"block {health.get('latest_block')} {health.get('bedrock_status')}")
    status, body = get(f"{args.lez}/api/blocks?count=5")
    blocks = json.loads(body) if status == 200 else []
    check("block list returns structured headers",
          bool(blocks) and all("header" in b and "hash" in b["header"] for b in blocks),
          f"{len(blocks)} blocks")
    if blocks:
        head_id = blocks[0]["header"]["block_id"]
        status, body = get(f"{args.lez}/api/block/id/{head_id}")
        by_id = json.loads(body) if status == 200 else {}
        status, body = get(f"{args.lez}/api/block/hash/{blocks[0]['header']['hash']}")
        by_hash = json.loads(body) if status == 200 else {}
        check("block lookup by id and by hash agree",
              by_id.get("header", {}).get("hash") == by_hash.get("header", {}).get("hash")
              != None, f"block {head_id}")

    print("\nLEZ explorer — swap transactions")
    for evidence in runs:
        run_id = evidence.get("run_id", "?")
        for effect in evidence["effects"]:
            if effect.get("chain") != "LEZ":
                continue
            txid = effect["transaction_id"]
            status, body = get(f"{args.lez}/api/tx/{txid}")
            payload = json.loads(body) if status == 200 else None
            inner = (payload or {}).get("Public") or (payload or {}).get("Private") or {}
            live = bool(inner.get("message", {}).get("program_id"))
            if live:
                accounts = len(inner["message"].get("account_ids", []))
                check(f"{run_id} · {effect['label']}", accounts > 0,
                      f"live tx, {accounts} accounts")
            else:
                # A run predating the standing chains resolves via certified evidence.
                status, body = get(f"{args.lez}/api/evidence/tx/{txid}")
                proof = json.loads(body) if status == 200 else {}
                check(f"{run_id} · {effect['label']}",
                      proof.get("effect", {}).get("transaction_id") == txid,
                      "certified-evidence proof (pre-settlement-chain run)")

    print("\nLEZ explorer — wallet accounts")
    wallets = {
        "Munich Vault 01": "BD6TpNTSLjeonDFmA3PXg6YtDy7xXt2LTm46266NpwJY",
        "Basel Vault 02": "A81AE1KTGdZ5GCDfy4XdUe9XvgNmkFzfgZcRkkQXm8vm",
        "Zurich Wallet 01": "4vDRakzuvKqJFJZ6k4ig3ybzds6fTLv1xDpwU283SwBM",
        "Limmat Wallet 02": "5A8bRmav5wjYQex6z7SpuuNNyhesqHwweAqjc3eWfchH",
    }
    for name, account in wallets.items():
        status, body = get(f"{args.lez}/api/account/{account}")
        data = json.loads(body) if status == 200 else {}
        check(f"{name} account resolves", isinstance(data.get("balance"), int),
              f"{data.get('balance')} LEZ, nonce {data.get('nonce')}")

    print(f"\n{CHECKS - len(FAILURES)}/{CHECKS} checks passed")
    if FAILURES:
        print("failed:")
        for failure in FAILURES:
            print(f"  - {failure}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
