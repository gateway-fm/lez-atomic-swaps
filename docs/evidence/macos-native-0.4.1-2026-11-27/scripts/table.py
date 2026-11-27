#!/usr/bin/env python3
"""table.py <logs dir> <native maker home> <label=log-stem:e2e-json:role> ... — one Markdown row per
swap: swap id, take and end times (CET, the harness's own clock), outcome, the Monero
transactions on regtest and the LEZ tags (from the native Maker's own evidence, or the
harness's summary when the Maker is in its container)."""
import json, os, re, subprocess, sys

logs, maker_home = sys.argv[1], sys.argv[2]
here = os.path.dirname(os.path.abspath(__file__))
for spec in sys.argv[3:]:
    label, stem, e2e, role = spec.split(":")
    text = open(os.path.join(logs, stem + ".log")).read()
    summary = json.load(open(os.path.join(logs, e2e)))
    swap = summary.get("swap_id", "")
    took = re.search(r"^\[(\d\d:\d\d:\d\d)\]   Taker took", text, re.M)
    end = re.search(r"^\[(\d\d:\d\d:\d\d)\] === \S+: (\w+) in (\d+)s", text, re.M)
    facts = {}
    if role == "maker":
        out = subprocess.run([sys.executable, "-I", os.path.join(here, "swap-facts.py"), maker_home + "/maker", swap],
                             capture_output=True, text=True).stdout
        facts = json.loads(out)["evidence"] if out else {}
    elif os.path.exists(os.path.join(logs, role)):  # the container Maker's facts, read beside it
        facts = json.load(open(os.path.join(logs, role)))["evidence"]
    def get(name, key):
        return (facts.get(name) or {}).get(key)
    xmr = []
    if get("monero-funding-receipt.json", "transaction_id"):
        xmr.append(f"funding `{get('monero-funding-receipt.json', 'transaction_id')[:16]}…` (block {get('monero-funding-receipt.json', 'containing_block_height')})")
    if get("monero-refund-finalized.json", "transaction_id"):
        xmr.append(f"refund sweep `{get('monero-refund-finalized.json', 'transaction_id')[:16]}…` (block {get('monero-refund-finalized.json', 'containing_block_height')})")
    sweep = summary.get("taker_monero_sweep") or {}
    if sweep.get("txid"):
        xmr.append(f"Taker sweep `{sweep['txid'][:16]}…` (block {sweep.get('height')})")
    if not xmr and summary.get("maker_monero_funding"):
        f = summary["maker_monero_funding"]; xmr.append(f"funding `{f['txid'][:16]}…` (block {f.get('height')})")
    tags = []
    for tag in ("tag13", "tag14", "tag15", "tag16", "tag17"):
        name = f"{tag}-finalized.json" if tag != "tag13" else "tag13-funding-finalized.json"
        txid = get(name, "outcome.facts.transaction.transaction_id")
        if txid:
            tags.append(f"{tag[3:]} `{txid[:12]}…` @{get(name, 'outcome.facts.transaction.position.height')}")
    print(f"| {label} | `{swap[:16]}…` | {took.group(1) if took else '-'} | {end.group(1) if end else '-'} "
          f"({end.group(3) if end else '-'} s) | {end.group(2) if end else '-'} | {'; '.join(xmr) or '-'} | "
          f"{'; '.join(tags) or 'see the harness summary'} |")
