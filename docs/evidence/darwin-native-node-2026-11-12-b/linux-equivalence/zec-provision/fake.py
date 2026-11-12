#!/usr/bin/env python3
"""Fake docker, curl and date for running zec-provision.sh (container mode)
without a stack: every call is appended to $CALLS; the answers are the
minimum the script needs to reach its end (--no-restart)."""
import json, os, sys, hashlib

name = os.path.basename(sys.argv[0])
args = sys.argv[1:]
state = os.environ["FAKE_STATE"]
with open(os.environ["CALLS"], "a") as calls:
    calls.write(name + " " + json.dumps(args) + "\n")

IDENTITY = "02" + "ab" * 32
ENVELOPE = b"signed envelope"
COMMITMENT = list(hashlib.sha256(ENVELOPE).digest())

def remember(key, value=None):
    path = os.path.join(state, key)
    if value is not None:
        open(path, "w").write(value)
    return open(path).read() if os.path.exists(path) else ""

if name == "date":
    print("1794500000" if "+%s" in args else "12:00:00")
    sys.exit(0)

if name == "curl":
    body = json.loads(args[args.index("--data") + 1])
    method = body["method"]
    result = {
        "getblockhash": "029f11d80ef9765602235e1bc9727e3eb6ba20839319f761fee920d63401e327",
        "getblockchaininfo": {"consensus": {"nextblock": "77190ad9"}},
        "getblockcount": 200,
        "getLastFinalizedBlockId": 5000,
    }.get(method)
    print(json.dumps({"jsonrpc": "2.0", "id": 1, "result": result}))
    sys.exit(0)

# docker
if args[:1] == ["inspect"]:
    print("healthy" if "Health" in " ".join(args) else "true"); sys.exit(0)
if args[:1] == ["logs"]:
    print("maker ZEC corridor ready"); sys.exit(0)
if args[:1] == ["compose"]:
    sys.exit(0)
if args[:1] == ["volume"]:
    sys.exit(0)
if args[:1] == ["exec"] and "bash" in args:
    sys.exit(0)
if args[:1] == ["exec"]:
    # rpc <role> <method>: docker exec C curl ... --data {...}
    body = json.loads(args[args.index("--data") + 1])
    method, params = body["method"], body["params"][0]
    if method == "maker_offer_publish":
        remember("offer", params["offer_id"])
    result = {
        "maker_offer_list": [], "maker_pair_list": [], "maker_local_price_list": [],
        "maker_local_route_save_v1": {}, "maker_offer_publish": {},
        "maker_health": {"ready": True, "degraded": False},
        "taker_offer_list_v1": {"offers": [{"offer": {"id": remember("offer"), "expires_at_unix_seconds": 1794586400},
                                            "signed_envelope_sha256": COMMITMENT, "maker_identity": IDENTITY}]},
    }[method]
    print(json.dumps({"jsonrpc": "2.0", "id": 1, "result": result}))
    sys.exit(0)
if args[:1] == ["run"]:
    joined = " ".join(args)
    if "delivery-identity" in joined or "maker-delivery-identity.pub" in joined:
        print(IDENTITY); sys.exit(0)
    if "zec-local-poc-transfer" in args:
        print(json.dumps({"transaction_id": "71" * 32, "to_address": "tmTaker"})); sys.exit(0)
    if "zec-local-poc-provision" in args:
        print(json.dumps({"run_id": "r", "signed_agreement_sha256": "aa" * 32, "zebra_tip_height": 200,
                          "zcash_refund_height": 209, "private_material_disclosed": False}))
        sys.exit(0)
    if "zec-local-poc-chat-draft" in args:
        print(json.dumps({"draft_sha256": "bb" * 32, "swap_id": "s", "offer_commitment": "cc" * 32}))
        sys.exit(0)
    if "sha256sum" in joined:
        print(f"{hashlib.sha256(ENVELOPE).hexdigest()}  signed-envelope.offer.json")
        print(f"{'dd' * 32}  unsigned-draft.borsh")
        print(f"{'ee' * 32}  actor-config.json")
        sys.exit(0)
    if "-i" in args:
        sys.stdin.read()
    sys.exit(0)
sys.exit(0)
