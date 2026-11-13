#!/usr/bin/env python3
# Fake docker and date for ui-e2e.sh (container mode): records every call; the
# Taker Node's swap list gains one swap after the desk's take step.
import json, os, sys
args = sys.argv[1:]
state = os.environ["FAKE_STATE"]
with open(os.environ["CALLS"], "a") as calls:
    calls.write(os.path.basename(sys.argv[0]) + " " + json.dumps(args) + "\n")
if os.path.basename(sys.argv[0]) == "date":
    print("1794500000" if "+%s" in args else "12:00:00"); sys.exit(0)
taken = os.path.exists(os.path.join(state, "taken"))
if args[:1] == ["exec"] and "taker_swap_list_v1" in " ".join(args):
    swaps = [{"swap_id": "ab" * 32, "state": "completed", "progress_generation": 4}] if taken else []
    print(json.dumps({"result": {"swaps": swaps}})); sys.exit(0)
if args[:1] == ["exec"] and "PREPARE_INTERACTIVE_BTC=1" in args:
    open(os.path.join(state, "taken"), "w").close()
if args[:1] == ["exec"] and "verify.mjs" in " ".join(args):
    print("  ✓ step"); print("1 passed, 0 failed"); sys.exit(0)
sys.exit(0)
