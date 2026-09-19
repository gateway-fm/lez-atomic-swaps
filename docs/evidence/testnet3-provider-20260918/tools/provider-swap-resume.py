"""Picks an already locked swap up where provider-swap.py left it: wait for the
Maker's lock, request the Taker's claim, wait for both Nodes. It sends nothing
to Bitcoin and never calls a Maker action."""
import importlib.util, sys, time

E2E, SWAP = sys.argv[1], sys.argv[2]
spec = importlib.util.spec_from_file_location("e2e", E2E)
e = importlib.util.module_from_spec(spec); spec.loader.exec_module(e)
e.NODES = {"maker": ("lez-testnet-maker-node", "/run/lez/maker/node.sock"),
           "taker": ("lez-testnet-taker-node", "/run/lez/taker/node.sock")}
e.mine = lambda blocks: None
print("RESUMED", SWAP, time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), flush=True)
view = e.wait_taker(SWAP, {"claim_available"}, timeout=8 * 3600)
print("CLAIM_AVAILABLE gen", view["progress_generation"], flush=True)
e.claim(SWAP, str(int(time.time())))
print("TAKER_CLAIM_REQUESTED", flush=True)
e.wait_taker(SWAP, {"completed"}, timeout=8 * 3600)
print("TAKER_COMPLETED", flush=True)
e.wait_completed(SWAP, timeout=8 * 3600)
print("BOTH_COMPLETED", flush=True)
