"""One forward swap (the Taker pays BTC) with Bitcoin served by a public RPC
provider: both Nodes have no Bitcoin wallet, so the Taker's lock is signed here,
in a wallet neither Node can reach, and handed to the Node unsent.

The owner wallet is a Bitcoin Core with no network at all (`--network none`,
`-connect=0`): it only holds the key and signs. What it spends is looked up on a
block explorer, as any light wallet would."""
import importlib.util, json, subprocess, sys, time, urllib.request

E2E = sys.argv[1]            # deploy/scripts/node-e2e.py of the commit under test
OWNER = sys.argv[2]          # the owner wallet's funded address
FEE_SAT = 1_000
EXPLORER = "https://mempool.space/testnet/api"

spec = importlib.util.spec_from_file_location("e2e", E2E)
e = importlib.util.module_from_spec(spec); spec.loader.exec_module(e)
e.NODES = {"maker": ("lez-testnet-maker-node", "/run/lez/maker/node.sock"),
           "taker": ("lez-testnet-taker-node", "/run/lez/taker/node.sock")}
e.FOREIGN_UNITS = 10_000
e.LEZ_UNITS = 10
e.mine = lambda blocks: None


def explorer(path: str):
    with urllib.request.urlopen(f"{EXPLORER}{path}", timeout=30) as reply:
        return json.load(reply)


def signer(*command: str):
    out = subprocess.run(["docker", "exec", "lez-t3-signer", "bitcoin-cli", "-testnet",
                          "-datadir=/var/lib/bitcoin", "-rpcwallet=owner", *command],
                         capture_output=True, text=True, check=True).stdout.strip()
    try:
        return json.loads(out)
    except ValueError:
        return out


def sign_funding(required: dict) -> str:
    print("FUNDING_REQUIRED", json.dumps(required), flush=True)
    coin = max(explorer(f"/address/{OWNER}/utxo"), key=lambda u: u["value"])
    script = explorer(f"/tx/{coin['txid']}")["vout"][coin["vout"]]["scriptpubkey"]
    change = coin["value"] - required["amount_sat"] - FEE_SAT
    raw = signer("createrawtransaction",
                 json.dumps([{"txid": coin["txid"], "vout": coin["vout"]}]),
                 json.dumps([{required["address"]: f"{required['amount_sat'] / 1e8:.8f}"},
                             {OWNER: f"{change / 1e8:.8f}"}]))
    signed = signer("signrawtransactionwithwallet", raw,
                    json.dumps([{"txid": coin["txid"], "vout": coin["vout"], "scriptPubKey": script,
                                 "amount": f"{coin['value'] / 1e8:.8f}"}]))
    assert signed["complete"], signed
    print("SIGNED_OUTSIDE_THE_NODE spends", f"{coin['txid']}:{coin['vout']}", "fee", FEE_SAT, flush=True)
    return signed["hex"]


e.sign_funding = sign_funding
stamp = str(int(time.time()))
offer = e.publish_offer(stamp)
swap_id, _ = e.take(offer, stamp)
print("SWAP", swap_id, flush=True)
print("LOCK", e.lock(swap_id), flush=True)
view = e.wait_taker(swap_id, {"claim_available"}, timeout=8 * 3600)
print("CLAIM_AVAILABLE gen", view["progress_generation"], flush=True)
e.claim(swap_id, stamp)
print("TAKER_CLAIM_REQUESTED", flush=True)
e.wait_taker(swap_id, {"completed"}, timeout=8 * 3600)
print("TAKER_COMPLETED", flush=True)
e.wait_completed(swap_id, timeout=8 * 3600)
print("BOTH_COMPLETED", flush=True)
