#!/usr/bin/env python3
"""swap-facts.py <role state dir> <swap id> — the public facts a role Node's
own effect evidence holds for one Monero swap (xmr-effect/evidence/*.json in
the swap's directory): for each file, only transaction ids, block heights
and hashes, amounts, confirmations and outcomes. No key, signature, packet
or transaction body is read out."""
import json
import os
import sys

root, swap_id = sys.argv[1], sys.argv[2]
KEEP = {"transaction_id", "amount_piconero", "containing_block_height", "containing_block_hash", "confirmations",
        "submission_outcome", "destination_address", "shared_address", "attempt_state", "height", "block_id",
        "timestamp_ms", "network_scope", "swap_id", "outcome", "result", "state", "kind"}


def pick(value, path=""):
    out = {}
    if isinstance(value, dict):
        for key, item in value.items():
            if key in ("exact_bytes", "terms", "context", "signer_account_ids"):
                continue
            sub = f"{path}.{key}" if path else key
            if isinstance(item, (dict, list)):
                out.update(pick(item, sub))
            elif key in KEEP:
                out[sub] = item
    elif isinstance(value, list):
        for index, item in enumerate(value):
            out.update(pick(item, f"{path}.{index}"))
    return out


for top, _, files in os.walk(root):
    if "xmr-reservation.json" not in files:
        continue
    try:
        if json.load(open(os.path.join(top, "xmr-reservation.json")))["response"]["swap_id"] != swap_id:
            continue
    except (OSError, ValueError, KeyError, TypeError):
        continue
    evidence = os.path.join(top, "xmr-effect", "evidence")
    facts = {}
    for name in sorted(os.listdir(evidence)) if os.path.isdir(evidence) else []:
        if name.endswith(".json"):
            try:
                facts[name] = pick(json.load(open(os.path.join(evidence, name))))
            except (OSError, ValueError):
                facts[name] = "unreadable"
    print(json.dumps({"swap_id": swap_id, "swap_dir": os.path.basename(top), "evidence": facts}, indent=1))
    sys.exit(0)
sys.exit(f"no swap {swap_id} under the state directory")
