"""Read-only collector for one swap: Node views, scheduler/journal rows, and every
Bitcoin transaction looked up twice -- through the RPC provider the Nodes used and on
an independent block explorer. Derived from ../../testnet-20260918/tools/collect.py."""
import json, subprocess, sys, os, base64, urllib.request, datetime
OUT,PROVIDER,EXPLORER,ONLY=sys.argv[1:5]
UA="curl/8"  # the provider refuses urllib's default user agent (HTTP 403)
def dexec(c,*cmd,stdin=None):
    r=subprocess.run(["docker","exec","-i",c,*cmd],input=stdin,capture_output=True,text=True); return r.stdout
def rpc(role,method,params):
    body=json.dumps({"jsonrpc":"2.0","id":1,"method":method,"params":[params]})
    out=dexec(f"lez-testnet-{role}-node","curl","-sS","--max-time","60","--unix-socket",f"/run/lez/{role}/node.sock","-H","content-type: application/json","--data",body,"http://localhost/")
    return json.loads(out).get("result")
def core(m,*a):
    q=urllib.request.Request(PROVIDER,data=json.dumps({"jsonrpc":"1.0","id":1,"method":m,"params":list(a)}).encode(),
        headers={"content-type":"application/json","user-agent":UA})
    return json.load(urllib.request.urlopen(q,timeout=60))["result"]
def explorer(txid):
    t=json.load(urllib.request.urlopen(urllib.request.Request(f"{EXPLORER}/tx/{txid}",headers={"user-agent":UA}),timeout=60))
    return {"confirmed":t["status"].get("confirmed"),"block_height":t["status"].get("block_height"),"fee_sat":t["fee"],
            "outputs":[{"address":o.get("scriptpubkey_address"),"sat":o["value"]} for o in t["vout"]]}
DB=r'''
import sqlite3,glob,json,sys
role=sys.argv[1]; out={"scheduler":[], "manual_actions":[], "actors":{}}
if role=="maker":
    c=sqlite3.connect("file:/var/lib/lez/maker/maker.sqlite3?mode=ro",uri=True)
    for r in c.execute("select swap_id,schedule_state,attempt_count,last_failure_class,created_at,updated_at from maker_actor_processes"):
        out["scheduler"].append(dict(zip(["swap_id","schedule_state","attempt_count","last_failure_class","created_at","updated_at"],r)))
    for r in c.execute("select swap_id,action,state,created_at,updated_at from maker_actor_manual_actions"):
        out["manual_actions"].append(dict(zip(["swap_id","action","state","created_at","updated_at"],r)))
for p in glob.glob("/var/lib/lez/%s/btc/swaps/*/actor/state.sqlite3"%role):
    c=sqlite3.connect("file:"+p+"?mode=ro",uri=True)
    ev=c.execute("select swap_id,aggregate_revision,evidence_kind from btc_actor_evidence order by 2").fetchall()
    if not ev: continue
    sid=ev[0][0]; a={"evidence":[{"revision":e[1],"kind":e[2]} for e in ev],"effect_journal":[],"maker_lock_steps":[]}
    try:
        for r in c.execute("select chain,operation,predecessor_revision,expected_effect_id,state,attempt_count from public_effect_journal"):
            a["effect_journal"].append(dict(zip(["chain","operation","predecessor_revision","transaction_id","state","attempt_count"],r)))
    except Exception: pass
    try:
        for r in c.execute("select step_id,expected_public_id,submission_result,state,attempt_count from btc_maker_lock_steps order by step_index"):
            a["maker_lock_steps"].append(dict(zip(["step","transaction_id","submission_result","state","attempt_count"],r)))
    except Exception: pass
    out["actors"][sid]=a
print(json.dumps(out))
'''
dbs={role:json.loads(dexec(f"lez-testnet-{role}-node","python3","-",role,stdin=DB)) for role in ("maker","taker")}
swaps=rpc("taker","taker_swap_list_v1",{"schema_version":1})["swaps"]
index=[]
for s in sorted(swaps,key=lambda s:s.get("terms",{}).get("maker_second_lock_cutoff_unix_seconds",0)):
    sid=s["swap_id"]
    if sid!=ONLY: continue
    mon=rpc("maker","maker_actor_monitor_v1",{"id":sid}) or {}
    rec={"swap_id":sid,"route":s.get("route"),"foreign_units":s.get("foreign_units"),"lez_units":s.get("lez_units"),
         "taker":{"state":s["state"],"progress_generation":s["progress_generation"],"effects":s.get("effects"),"terms":s.get("terms"),
                  "actor":dbs["taker"]["actors"].get(sid)},
         "maker":{"schedule_state":mon.get("schedule_state"),"attempt_count":mon.get("attempt_count"),
                  "observation":(mon.get("progress") or {}).get("observation"),"effects":mon.get("effects"),
                  "scheduler_row":next((r for r in dbs["maker"]["scheduler"] if r["swap_id"]==sid),None),
                  "manual_actions":[m for m in dbs["maker"]["manual_actions"] if m["swap_id"]==sid],
                  "actor":dbs["maker"]["actors"].get(sid)},
         "bitcoin_verification":[]}
    txids=set()
    for side in ("taker","maker"):
        for e in (rec[side].get("effects") or []):
            if e.get("chain")=="Bitcoin": txids.add(e["transaction_id"])
        for j in ((rec[side].get("actor") or {}).get("effect_journal") or []):
            if j["chain"]=="bitcoin": txids.add(j["transaction_id"])
    for t in sorted(txids):
        try:
            x=core("getrawtransaction",t,1)
            h=core("getblockheader",x["blockhash"]) if x.get("blockhash") else {}
            rec["bitcoin_verification"].append({"txid":t,"confirmations":x.get("confirmations",0),"block_height":h.get("height"),"block_hash":x.get("blockhash"),
                "spends":[f'{i["txid"]}:{i["vout"]}' for i in x["vin"]],"outputs_sat":[int(round(o["value"]*1e8)) for o in x["vout"]],
                "witness_items":[len(i.get("txinwitness",[])) for i in x["vin"]],"explorer":explorer(t)})
        except Exception as e:
            rec["bitcoin_verification"].append({"txid":t,"error":str(e)[:120]})
    json.dump(rec,open(os.path.join(OUT,"swaps",sid[:12]+".json"),"w"),indent=2); index.append(sid)
snap={"captured_at_utc":datetime.datetime.now(datetime.UTC).isoformat(timespec="seconds"),
      "bitcoin":{k:core("getblockchaininfo")[k] for k in ("chain","blocks","bestblockhash")},
      "wallets":{"maker":rpc("maker","maker_wallet_balances_v1",{}),"taker":rpc("taker","taker_wallet_balances_v1",{"schema_version":1})},
      "swaps":index}
json.dump(snap,open(os.path.join(OUT,"snapshot.json"),"w"),indent=2)
print("collected",len(index),"swaps")
