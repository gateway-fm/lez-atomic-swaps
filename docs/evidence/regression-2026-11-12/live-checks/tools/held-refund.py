#!/usr/bin/env python3
"""#103 item 6 live check, lane 1, fast profile: a BTC swap (TakerSellsForeign)
whose Taker locks Bitcoin, then the LEZ sequencer is stopped so neither the
Maker's LEZ lock nor LEZ finality can move; past the Maker's lock cutoff the
Taker asks for its refund, which the actor holds until finalized LEZ time
passes the cutoff. Reads the views and the desks, starts the sequencer, and
follows the refund to the end. Uses node-e2e.py's own helpers."""
import importlib.util, json, os, subprocess, sys, time
deploy = os.path.expanduser("~/lez-m2/deploy")
os.chdir(deploy)
spec = importlib.util.spec_from_file_location("e2e", "scripts/node-e2e.py")
e2e = importlib.util.module_from_spec(spec); spec.loader.exec_module(e2e)
log = e2e.log

def show(title, value):
    print(f"--- {title} [{time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}]"); print(json.dumps(value, indent=1, sort_keys=True), flush=True)

def desk(role, swap_id):
    out = subprocess.run(["docker", "exec", f"lez-{role}-node", "node", "/tmp/row-read.mjs", role, swap_id],
                         capture_output=True, text=True, stdin=subprocess.DEVNULL).stdout
    for line in out.splitlines():
        if "ROW" in line: print(line.strip(), flush=True)

def views(swap_id):
    t = e2e.call("taker", "taker_swap_monitor_v1", {"schema_version": 1, "swap_id": swap_id})
    show("taker_swap_monitor_v1 (state, available_action, last_failure_class, last_outcome, terms)",
         {k: t.get(k) for k in ("state", "available_action", "progress_generation", "last_failure_class", "last_outcome", "terms")})
    lst = e2e.call("taker", "taker_swap_list_v1", {"schema_version": 1})
    row = [s for s in lst["swaps"] if s["swap_id"] == swap_id][0]
    show("taker_swap_list_v1 row last_outcome", row.get("last_outcome"))
    m = e2e.rpc("maker", "maker_actor_monitor_v1", {"id": swap_id})
    r = m.get("result", m)
    show("maker_actor_monitor_v1", {k: r.get(k) for k in ("schedule_state", "attempt_count", "last_failure_class", "last_outcome", "progress", "error")})
    return t

stamp = time.strftime("r103held%H%M%S")
offer = e2e.publish_offer(stamp)
swap_id, _ = e2e.take(offer, stamp)
terms = e2e.taker_view(swap_id).get("terms") or {}
cutoff = int(terms["maker_second_lock_cutoff_unix_seconds"])
log(f"swap {swap_id}; Maker lock cutoff {cutoff} ({time.strftime('%H:%M:%S', time.localtime(cutoff))} local)")
txid = e2e.lock(swap_id)
log("$ docker stop lez-sequencer   (right after the Taker's Bitcoin lock: no Maker LEZ lock, LEZ finality frozen)")
subprocess.run(["docker", "stop", "lez-sequencer"], check=True, capture_output=True)
try:
    e2e.mine(12)
    while time.time() < cutoff + 40:
        log(f"  waiting for the cutoff: taker {e2e.taker_view(swap_id)['state']}, {cutoff + 40 - int(time.time())} s left")
        e2e.mine(1); time.sleep(20)
    view = e2e.taker_view(swap_id)
    show("taker view past the cutoff", {k: view.get(k) for k in ("state", "available_action", "progress_generation", "last_outcome")})
    reply = e2e.rpc("taker", "taker_swap_refund_v1", {"schema_version": 1, "request_id": f"r103-refund-{stamp}",
                                                      "swap_id": swap_id, "expected_generation": view["progress_generation"]})
    show("$ taker_swap_refund_v1", reply)
    for round_ in range(3):
        time.sleep(25)
        views(swap_id); desk("taker", swap_id); desk("maker", swap_id)
finally:
    log("$ docker start lez-sequencer")
    subprocess.run(["docker", "start", "lez-sequencer"], check=True, capture_output=True)
deadline = time.time() + 1500
while time.time() < deadline:
    v = e2e.taker_view(swap_id)
    if v["state"] == "refunded":
        break
    log(f"  taker {v['state']} | last_outcome {json.dumps(v.get('last_outcome'))[:500]}")
    e2e.mine(1); time.sleep(15)
views(swap_id); desk("taker", swap_id)
