"""Adds `lez_verification` to a swap record: every LEZ transaction the Nodes
name, looked up on the public sequencer (through the local TLS proxy; the
SHA-256 of the returned bytes is kept) and on the finalized indexer."""
import base64, hashlib, json, sys, urllib.request

RECORD, SEQUENCER, INDEXER = sys.argv[1:4]


def rpc(url, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getTransaction", "params": params}).encode()
    request = urllib.request.Request(url, data=body, headers={"content-type": "application/json"})
    return json.load(urllib.request.urlopen(request, timeout=60)).get("result")


record = json.load(open(RECORD))
seen, checks = set(), []
for side in ("taker", "maker"):
    for effect in record[side].get("effects") or []:
        txid = effect["transaction_id"]
        if effect.get("chain") != "Lez" or txid in seen:
            continue
        seen.add(txid)
        sequencer, indexer = rpc(SEQUENCER, [txid]), rpc(INDEXER, [txid])
        checks.append({
            "transaction_id": txid, "kind": effect["kind"],
            "public_sequencer": {"found": bool(sequencer), "block_id": sequencer[1] if sequencer else None,
                                 "bytes_sha256": hashlib.sha256(base64.b64decode(sequencer[0])).hexdigest() if sequencer else None},
            "finalized_indexer": {"found": bool(indexer),
                                  "hash": next(iter(indexer.values())).get("hash") if indexer else None}})
record["lez_verification"] = checks
json.dump(record, open(RECORD, "w"), indent=2)
print(json.dumps([(c["kind"], c["public_sequencer"]["found"], c["public_sequencer"]["block_id"], c["finalized_indexer"]["found"]) for c in checks]))
