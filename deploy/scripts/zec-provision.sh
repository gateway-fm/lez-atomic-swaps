#!/usr/bin/env bash
# zec-provision.sh — build one prepared ZEC<->LEZ swap (TakerSellsLez) on the
# running devnet and hand it to both Nodes.
#
# A ZEC swap is prepared, not composed: the Taker Node takes a Zcash offer only
# from a static `prepared_zec` catalog entry whose files were provisioned from
# live chain facts (crates/taker-node/src/taker_service_config/prepared.rs),
# and the Maker daemon needs the corridor's Maker config as its actor template.
# This script runs that ceremony with the provisioners staged in the Taker
# image (zec-local-poc-provision, zec-local-poc-chat-draft):
#
#   0. the Maker's Delivery identity: the ZEC ceremony binds the Maker's
#      Delivery identity, Chat signing key and Zcash key into one key
#      (maker-node zec_chat.rs:362, taker-node zec_taker_accept.rs:541), and the
#      provisioner's Maker Zcash key is the regtest funder key [0x04; 32] that
#      owns the coinbase it spends. So the Maker's Delivery signing key is
#      pinned to that key (runtime/secrets/maker-delivery-signing.key, read by
#      the entrypoint). The first time, both Nodes are recreated and the offer
#      files signed by the old identity are removed: a Taker's discovery fails
#      outright on a file it cannot authenticate. Devnet only, obviously.
#   1. preconditions: Nodes and zebra up, LEZ finalising, >= 10 mature coinbase
#      outputs on zebra (mined to the miner address of assets/zebra/zebrad.toml,
#      whose key the provisioner holds; never another address);
#   2. spec from runtime/lez/<role>/identity.json, runtime.env and the market
#      bootstrap manifest (the same facts the entrypoints render for BTC);
#   3. provision the corridor into a staging volume, as the Node uid, at the
#      path the containers will see (/var/lib/lez/zec/corridor);
#   4. distribute shared/+maker/ into lez-zec-maker and shared/+taker/ into
#      lez-zec-taker (the previous corridor is archived, never merged);
#   5. publish the Zcash offer through the Maker's owner socket and discover
#      it on the Taker (commitment + expiry);
#   6. bind the template agreement to that offer: the unsigned Chat draft;
#   7. write the Taker catalog (prepared.json) next to the corridor;
#   8. recreate both Nodes so the entrypoints pick the material up, and check
#      the sidecars answer, maker_health is ready and the offer is listed.
#
# Secret-free summaries land in runtime/zec/<run_id>/ (runtime/ is gitignored)
# and runtime/zec/current names the run id. Every corridor is pinned to the
# chain it was provisioned on: after `up.sh --fresh-lez`, run this again.
#
#   scripts/zec-provision.sh [--no-restart] [--skip-chain-check]
#     --no-restart        steps 1-7 only (the Nodes keep running; the material
#                         is picked up on their next recreate)
#     --skip-chain-check  do not call ensure-chain.sh (which may recreate the
#                         chain); the caller vouches for finality
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
set -a
# shellcheck source=/dev/null
source runtime/runtime.env
set +a
export BTC_RPC_PASSWORD

RESTART=1
CHAIN_CHECK=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-restart) RESTART=0; shift ;;
    --skip-chain-check) CHAIN_CHECK=0; shift ;;
    -h|--help) sed -n '2,33p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done

compose=(docker compose --env-file runtime/runtime.env)
TAKER_IMAGE="${LEZ_IMAGE_PREFIX:-lez}-taker-node:${LEZ_IMAGE_TAG:-local}"
VOLUME_PREFIX="${LEZ_VOLUME_PREFIX:-lez}"
STAGING="$VOLUME_PREFIX-zec-staging"
MAKER_VOLUME="$VOLUME_PREFIX-zec-maker"
TAKER_VOLUME="$VOLUME_PREFIX-zec-taker"
DELIVERY_VOLUME="$VOLUME_PREFIX-delivery-dir"
NODE_UID=4713
ZEC_ROOT=/var/lib/lez/zec            # the volume, as the Node containers see it
CORRIDOR="$ZEC_ROOT/corridor"        # the provisioner's output root
ZEBRA_URL="${LEZ_ZEBRA_URL:-http://127.0.0.1:18232/}"
INDEXER_URL="${LEZ_INDEXER_URL:-http://127.0.0.1:8779/}"
# Loopback ports for the role sidecars inside the Node containers; outside
# both BTC per-swap ranges (maker 19000+400, taker 19500+400).
MAKER_BRIDGE_PORT="${LEZ_ZEC_MAKER_BRIDGE_PORT:-19901}"
TAKER_BRIDGE_PORT="${LEZ_ZEC_TAKER_BRIDGE_PORT:-19902}"
# The provisioner's template: 1 ZEC against 50 000 LEZ units (local_poc.rs).
FOREIGN_UNITS=100000000
LEZ_UNITS=50000
ROUTE='{"pair":"Zcash","direction":"TakerSellsLez"}'
# The offer's validity bounds the catalog's: the draft copies the offer expiry
# into the transcript and the take checks it. The store's maximum, so one
# corridor outlives one suite.
OFFER_TTL_SECONDS="${LEZ_ZEC_OFFER_TTL_SECONDS:-86400}"
MIN_MATURE_OUTPUTS=10
MATURITY=100
# The provisioner's Maker Zcash key (local_poc.rs FUNDED_ZCASH_KEY_BYTE), the
# key behind zebra's miner address; the Maker's Delivery identity must be it.
FUNDER_KEY_HEX="$(printf '04%.0s' $(seq 1 32))"
DELIVERY_KEY_FILE=runtime/secrets/maker-delivery-signing.key
MAKER_IMAGE="${LEZ_IMAGE_PREFIX:-lez}-maker-node:${LEZ_IMAGE_TAG:-local}"

log() { printf '[zec-provision %s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }
fail() { echo "zec-provision failed: $*" >&2; exit 1; }

# One owner-RPC call on a Node socket; prints the JSON-RPC envelope. No -i:
# curl never reads stdin and an attached stream hangs a detached runner.
rpc() { # rpc <maker|taker> <method> <params-json>
  local role="$1" method="$2" params="$3"
  docker exec "lez-$role-node" curl -sS --max-time 120 --unix-socket "/run/lez/$role/node.sock" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":[$params]}" http://localhost/ </dev/null
}
rpc_result() { # rpc_result <role> <method> <params>: the .result or a failure
  local reply
  reply="$(rpc "$@")" || fail "$1 $2: transport error"
  python3 -c 'import json,sys
reply=json.loads(sys.stdin.read())
if "error" in reply:
    sys.stderr.write(json.dumps(reply["error"])[:400]+"\n"); sys.exit(1)
json.dump(reply["result"], sys.stdout)' <<<"$reply" || fail "$1 $2 answered an error"
}
zebra() { # zebra <method> <params-json>
  curl -sS --max-time 30 -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":${2:-[]}}" "$ZEBRA_URL" |
    python3 -c 'import json,sys
reply=json.load(sys.stdin)
if reply.get("error"): sys.stderr.write(json.dumps(reply["error"])+"\n"); sys.exit(1)
json.dump(reply["result"], sys.stdout)'
}
# A throwaway container of the Taker image on the staging/role volumes.
# Root for file plumbing; the Node uid for anything that reads private files.
helper() { # helper <uid> [docker run flags...] -- <bash -c script>
  local uid="$1"; shift
  local flags=()
  while [[ "$1" != -- ]]; do flags+=("$1"); shift; done
  shift
  docker run --rm --user "$uid" --entrypoint bash "${flags[@]}" "$TAKER_IMAGE" -c "$1" </dev/null
}

wait_healthy() { # wait_healthy <container> <seconds>
  local c="$1" status
  for _ in $(seq 1 $(( ${2:-360} / 3 ))); do
    status="$(docker inspect -f '{{.State.Health.Status}}' "$c" 2>/dev/null || echo missing)"
    [[ "$status" == healthy ]] && return 0
    if [[ "$(docker inspect -f '{{.State.Running}}' "$c" 2>/dev/null)" != true ]]; then docker logs "$c" 2>&1 | tail -20; fail "$c exited"; fi
    sleep 3
  done
  fail "$c not healthy after ${2:-360} s"
}
# The Taker pins the Maker's Delivery identity when it starts, so the Maker
# must be up (and have republished its identity) before the Taker is recreated.
recreate_nodes() {
  "${compose[@]}" up -d --no-deps --force-recreate maker-node >/dev/null 2>&1
  wait_healthy lez-maker-node
  "${compose[@]}" up -d --no-deps --force-recreate taker-node >/dev/null 2>&1
  wait_healthy lez-taker-node
}

stamp="$(date +%s)"
run_id="zec-devnet-$stamp"
swap_id="zec-devnet-swap-$stamp"
offer_id="zec-devnet-offer-$stamp"
reservation_id="zec-devnet-res-$stamp"
out="runtime/zec/$run_id"
mkdir -p "$out"
chmod 0700 runtime/zec "$out"

# ---- 0. the Maker's Delivery identity ------------------------------------------
log "0/8 Maker Delivery identity"
for c in lez-maker-node lez-taker-node lez-zebra; do
  [[ "$(docker inspect -f '{{.State.Running}}' "$c" 2>/dev/null)" == true ]] || fail "$c is not running"
done
if [[ ! -s "$DELIVERY_KEY_FILE" ]] || [[ "$(tr -d ' \n' <"$DELIVERY_KEY_FILE")" != "$FUNDER_KEY_HEX" ]]; then
  # Same shape and mode as runtime/lez/<role>/lez-signer.key: the Node reads it as uid 4713.
  printf '%s' "$FUNDER_KEY_HEX" >"$DELIVERY_KEY_FILE"
  chmod 0644 "$DELIVERY_KEY_FILE"
  log "  wrote $DELIVERY_KEY_FILE (the regtest funder key)"
fi
# The CLI reads only an owner-owned 0600 file: copy the key into one first.
expected_identity="$(docker run --rm --user 0:0 -v "$PWD/$DELIVERY_KEY_FILE:/in/key:ro" --entrypoint bash "$MAKER_IMAGE" \
  -c 'umask 077; install -m 0600 /in/key /tmp/k && lez-maker-cli delivery-identity --signing-key-file /tmp/k' </dev/null |
  tr -d ' \n' | grep -o '0[23][0-9a-f]\{64\}')" ||
  fail "lez-maker-cli delivery-identity failed on $DELIVERY_KEY_FILE"
current_identity="$(docker run --rm -v "$DELIVERY_VOLUME:/d:ro" --entrypoint cat "$TAKER_IMAGE" /d/maker-delivery-identity.pub </dev/null 2>/dev/null | tr -d ' \n')"
if [[ "$current_identity" == "$expected_identity" ]]; then
  log "  Maker Delivery identity is the corridor key ($expected_identity)"
else
  [[ "$RESTART" == 1 ]] || fail "the Maker's Delivery identity ($current_identity) is not the corridor key ($expected_identity); rerun without --no-restart to pin it"
  log "  pinning the Maker Delivery identity $current_identity → $expected_identity (both Nodes recreated)"
  "${compose[@]}" stop maker-node taker-node >/dev/null 2>&1
  # Offer files signed by the previous identity would fail the Taker's
  # discovery outright (run_local_delivery.rs: verify_envelope(...)?).
  removed="$(helper 0:0 -v "$DELIVERY_VOLUME:/delivery" -- 'shopt -s nullglob; f=(/delivery/*.offer.json); rm -f -- "${f[@]}"; echo ${#f[@]}')"
  log "  removed $removed Delivery offer file(s) of the previous identity"
  recreate_nodes
  current_identity="$(docker run --rm -v "$DELIVERY_VOLUME:/d:ro" --entrypoint cat "$TAKER_IMAGE" /d/maker-delivery-identity.pub </dev/null | tr -d ' \n')"
  [[ "$current_identity" == "$expected_identity" ]] || fail "the Maker republished $current_identity, not $expected_identity"
  log "  Maker Delivery identity is now $expected_identity"
fi

# ---- 1. preconditions ---------------------------------------------------------
log "1/8 preconditions"
if [[ "$CHAIN_CHECK" == 1 ]]; then
  # ensure-chain.sh recreates a stalled chain, which throws away any suite in
  # flight; the tmux runners mark /tmp/fast.log with DONE or STALLED when done.
  if [[ -f /tmp/fast.log ]] && ! grep -qE "^(DONE|STALLED)$" /tmp/fast.log; then
    fail "a suite is still running (/tmp/fast.log has no DONE/STALLED); pass --skip-chain-check or wait"
  fi
  chain="$(bash scripts/ensure-chain.sh)" || fail "LEZ chain is $chain"
  log "  LEZ chain: $chain"
else
  log "  LEZ chain check skipped by the caller"
fi
zebra_height="$(zebra getblockcount)" || fail "zebra RPC unavailable at $ZEBRA_URL"
target=$(( MATURITY + MIN_MATURE_OUTPUTS ))
while (( zebra_height < target )); do
  batch=$(( target - zebra_height )); (( batch > 10 )) && batch=10
  zebra generate "[$batch]" >/dev/null || fail "zebra generate failed"
  zebra_height="$(zebra getblockcount)"
done
log "  zebra height $zebra_height (>= $target: at least $MIN_MATURE_OUTPUTS mature coinbase outputs)"

# ---- 2. spec ------------------------------------------------------------------
log "2/8 spec"
manifest=runtime/market-bootstrap.env
escrow="$(sed -n 's/^M3_POC_LEZ_ESCROW_PROGRAM_ID=//p' "$manifest" | head -1)"
genesis="$(sed -n 's/^M3_POC_LEZ_GENESIS_BLOCK_HASH=//p' "$manifest" | head -1)"
transfer="$(sed -n 's/^M3_POC_LEZ_AUTH_TRANSFER_PROGRAM_ID=//p' "$manifest" | head -1)"
transfer="${transfer:-fe96c4228babbe8bc578e3e25b884cacb07f8c86541f27ed676789875eef875a}"
channel="${LEZ_V02_CHANNEL_ID:?}"
for v in escrow genesis channel; do
  [[ "${!v}" =~ ^[0-9a-f]{64}$ ]] || fail "$v is not 64 hex ($manifest / runtime.env)"
done
finalized="$(curl -sS --max-time 10 -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' "$INDEXER_URL" |
  python3 -c 'import json,sys; print(int(json.load(sys.stdin)["result"]))')" || fail "indexer unavailable"
# The actors scan LEZ from start_height for max_blocks (<= 4096, ~90 min
# here): the lock they look for happens after this provisioning.
discovery_start=$(( finalized > 32 ? finalized - 32 : 1 ))
python3 - "$out/spec.json" "$run_id" "$swap_id" "$channel" "$genesis" "$escrow" "$transfer" \
  "$MAKER_BRIDGE_PORT" "$TAKER_BRIDGE_PORT" "$discovery_start" <<'PY'
import json, sys
(out, run_id, swap_id, channel, genesis, escrow, transfer, maker_port, taker_port, start) = sys.argv[1:]
ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
def base58(raw: bytes) -> str:
    n = int.from_bytes(raw, "big"); text = ""
    while n: n, r = divmod(n, 58); text = ALPHABET[r] + text
    return "1" * (len(raw) - len(raw.lstrip(b"\0"))) + text
ids = {r: json.load(open(f"runtime/lez/{r}/identity.json"))["account_id"] for r in ("maker", "taker")}
spec = {
    "schema_version": 1, "run_id": run_id, "swap_id": swap_id, "direction": "taker_sells_lez",
    "lez_runtime": {
        "chain_id": channel, "channel_id": channel, "genesis_block_hash": genesis,
        "escrow_program_id": escrow,
        "authenticated_transfer_program_id_base58": base58(bytes.fromhex(transfer)),
        "maker_signer_account_id_base58": ids["maker"], "taker_signer_account_id_base58": ids["taker"]},
    "bridge": {"maker_endpoint": f"http://127.0.0.1:{maker_port}", "taker_endpoint": f"http://127.0.0.1:{taker_port}"},
    "zebra_endpoint": "http://127.0.0.1:18232",
    "lez_discovery_start_height": int(start), "lez_discovery_max_blocks": 4096,
}
json.dump(spec, open(out, "w"), indent=2)
print(f"  spec: channel {channel[:12]}… escrow {escrow[:12]}… discovery from {start}")
PY
chmod 0600 "$out/spec.json"

# ---- 3. provision -------------------------------------------------------------
log "3/8 provision (staging volume $STAGING)"
docker volume rm -f "$STAGING" >/dev/null 2>&1 || true
docker volume create "$STAGING" >/dev/null
helper 0:0 -v "$STAGING:$ZEC_ROOT" -v "$PWD/$out/spec.json:/in/spec.json:ro" \
  -v "$PWD/runtime/lez/maker/lez-signer.key:/in/maker.key:ro" -v "$PWD/runtime/lez/taker/lez-signer.key:/in/taker.key:ro" -- "
  set -e; umask 077
  mkdir -p $ZEC_ROOT/signers
  install -m 0600 /in/spec.json $ZEC_ROOT/spec.json
  install -m 0600 /in/maker.key $ZEC_ROOT/signers/maker.key
  install -m 0600 /in/taker.key $ZEC_ROOT/signers/taker.key
  chown -R $NODE_UID:$NODE_UID $ZEC_ROOT && chmod 0700 $ZEC_ROOT $ZEC_ROOT/signers"
# --network host: the provisioner only accepts a loopback zebra endpoint, and
# Compose publishes zebra on 127.0.0.1:18232.
docker run --rm --network host --user "$NODE_UID:$NODE_UID" -v "$STAGING:$ZEC_ROOT" \
  --entrypoint zec-local-poc-provision "$TAKER_IMAGE" \
  --spec-file "$ZEC_ROOT/spec.json" --output-root "$CORRIDOR" \
  --maker-lez-signer-key-file "$ZEC_ROOT/signers/maker.key" \
  --taker-lez-signer-key-file "$ZEC_ROOT/signers/taker.key" </dev/null >"$out/provision-summary.json" ||
  fail "zec-local-poc-provision failed (see above)"
python3 - "$out/provision-summary.json" <<'PY'
import json, sys
s = json.load(open(sys.argv[1]))
print(f"  provisioned run {s['run_id']}: agreement sha256 {s['signed_agreement_sha256'][:16]}…, "
      f"zebra tip {s['zebra_tip_height']}, zcash refund height {s['zcash_refund_height']}, "
      f"private_material_disclosed={s['private_material_disclosed']}")
PY

# ---- 4. distribute ------------------------------------------------------------
log "4/8 distribute into $MAKER_VOLUME / $TAKER_VOLUME"
# The init services create the volumes with Compose's labels and own them to
# the Node uid; they are one-shots and harmless to rerun.
"${compose[@]}" up -d --no-deps maker-init taker-init >/dev/null 2>&1
helper 0:0 -v "$STAGING:/stage:ro" -v "$MAKER_VOLUME:/maker" -v "$TAKER_VOLUME:/taker" -- "
  set -e; umask 077
  for role in maker taker; do
    vol=/\$role
    if [[ -e \$vol/corridor ]]; then
      old=\$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[\"run_id\"])' \$vol/corridor/\$role/actor-config.json 2>/dev/null || date +%s)
      mkdir -p \$vol/archive && mv \$vol/corridor \$vol/archive/\$old
    fi
    rm -f \$vol/prepared.json
    mkdir -m 0700 \$vol/corridor
    cp -a /stage/corridor/shared \$vol/corridor/
    cp -a /stage/corridor/\$role \$vol/corridor/
    chown -R $NODE_UID:$NODE_UID \$vol && chmod -R go-rwx \$vol
  done
  mkdir -p /taker/taker-swaps/$run_id && chown $NODE_UID:$NODE_UID /taker/taker-swaps /taker/taker-swaps/$run_id && chmod 0700 /taker/taker-swaps /taker/taker-swaps/$run_id
  echo '  maker volume:'; find /maker/corridor -type f | sort | sed 's|^|    |'
  echo '  taker volume:'; find /taker/corridor -type f | sort | sed 's|^|    |'"
# The staging copy held both roles' secrets side by side; nothing reads it again.
docker volume rm -f "$STAGING" >/dev/null

# ---- 5. publish and discover the offer -----------------------------------------
log "5/8 Zcash offer $offer_id"
revision_for() { # revision_for <method>: the revision of the Zcash route entry, if any
  rpc_result maker "$1" '{}' | python3 -c 'import json,sys
route=json.loads(sys.argv[1])
for e in json.load(sys.stdin):
    if ((e.get("value") or {}).get("route") or {}) == route: print(int(e["revision"])); break' "$ROUTE"
}
pair_rev="$(revision_for maker_pair_list)"
price_rev="$(revision_for maker_local_price_list)"
# The local price is an exact lot ratio that the store only accepts in reduced
# form (maker_application.rs: gcd must be 1): 50 000 LEZ units per 1 ZEC is
# 1 LEZ unit per 2 000 zatoshis, and quoting FOREIGN_UNITS then yields LEZ_UNITS.
request="$(python3 -c 'import json,sys,math
stamp,route,fu,lu,pr,prr,ttl=sys.argv[1:]
g=math.gcd(int(lu),int(fu))
r={"request_id":f"zec-devnet-route-{stamp}",
   "configuration":{"route":json.loads(route),"enabled":True,"price_source":"local",
                    "minimum_foreign_units":int(fu),"maximum_foreign_units":int(fu),"offer_ttl_seconds":int(ttl)},
   "price":{"route":json.loads(route),"lez_units_per_lot":int(lu)//g,"foreign_units_per_lot":int(fu)//g}}
if pr: r["expected_pair_revision"]=int(pr)
if prr: r["expected_price_revision"]=int(prr)
print(json.dumps(r))' "$stamp" "$ROUTE" "$FOREIGN_UNITS" "$LEZ_UNITS" "$pair_rev" "$price_rev" "$OFFER_TTL_SECONDS")"
rpc_result maker maker_local_route_save_v1 "$request" >/dev/null
rpc_result maker maker_offer_publish "{\"request_id\":\"zec-devnet-publish-$stamp\",\"offer_id\":\"$offer_id\",\"route\":$ROUTE}" >/dev/null
entry=""
for _ in $(seq 1 30); do
  entry="$(rpc_result taker taker_offer_list_v1 "{\"schema_version\":1,\"route\":$ROUTE}" |
    python3 -c 'import json,sys
want=sys.argv[1]
for e in json.load(sys.stdin)["offers"]:
    if e["offer"]["id"]==want:
        print(json.dumps({"commitment":bytes(e["signed_envelope_sha256"]).hex(),
                          "expires_at":e["offer"]["expires_at_unix_seconds"],
                          "maker_identity":e["maker_identity"]})); break' "$offer_id")"
  [[ -n "$entry" ]] && break
  sleep 2
done
[[ -n "$entry" ]] || fail "Taker did not list $offer_id within 60 s"
commitment="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["commitment"])' "$entry")"
expires_at="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["expires_at"])' "$entry")"
log "  Taker lists it: commitment $commitment, expires at $expires_at"

# ---- 6. chat draft -----------------------------------------------------------
log "6/8 unsigned Chat draft (reservation $reservation_id)"
docker run --rm --user "$NODE_UID:$NODE_UID" -v "$TAKER_VOLUME:$ZEC_ROOT" \
  --entrypoint zec-local-poc-chat-draft "$TAKER_IMAGE" \
  --source-agreement-file "$CORRIDOR/shared/agreement-v2.borsh" --now-unix-seconds "$(date +%s)" \
  --reservation-id "$reservation_id" --offer-commitment "$commitment" \
  --offer-expires-at-unix-seconds "$expires_at" \
  --output-file "$CORRIDOR/taker/unsigned-draft.borsh" </dev/null >"$out/draft-summary.json" ||
  fail "zec-local-poc-chat-draft failed (see above)"
python3 - "$out/draft-summary.json" <<'PY'
import json, sys
s = json.load(open(sys.argv[1]))
print(f"  draft sha256 {s['draft_sha256'][:16]}… for swap {s['swap_id']}, offer commitment {s['offer_commitment'][:16]}…")
PY

# ---- 7. the Taker catalog ------------------------------------------------------
log "7/8 prepared.json"
# The Delivery file is the exact signed envelope (run_local_delivery.rs:
# publish writes `encoded`, authenticate hashes the file bytes), so its sha256
# must be the commitment the Taker listed.
sums="$(helper 0:0 -v "$DELIVERY_VOLUME:/delivery:ro" -v "$TAKER_VOLUME:$ZEC_ROOT" -- "
  set -e; umask 077
  install -m 0600 -o $NODE_UID -g $NODE_UID /delivery/$offer_id.offer.json $CORRIDOR/taker/signed-envelope.offer.json
  cd $CORRIDOR/taker && sha256sum signed-envelope.offer.json unsigned-draft.borsh actor-config.json")"
envelope_sha="$(awk '$2=="signed-envelope.offer.json"{print $1}' <<<"$sums")"
draft_sha="$(awk '$2=="unsigned-draft.borsh"{print $1}' <<<"$sums")"
config_sha="$(awk '$2=="actor-config.json"{print $1}' <<<"$sums")"
[[ "$envelope_sha" == "$commitment" ]] || fail "Delivery file sha256 $envelope_sha != listed commitment $commitment"
python3 - "$out/prepared.json" "$CORRIDOR" "$ZEC_ROOT" "$run_id" "$swap_id" "$offer_id" "$reservation_id" \
  "$FOREIGN_UNITS" "$LEZ_UNITS" "$envelope_sha" "$draft_sha" "$config_sha" <<'PY'
import json, sys
(out, corridor, root, run_id, swap_id, offer_id, reservation_id, fu, lu, env_sha, draft_sha, cfg_sha) = sys.argv[1:]
swap_dir = f"{root}/taker-swaps/{run_id}"
entry = {
    "source_id": "local-maker", "swap_id": swap_id, "offer_id": offer_id, "reservation_id": reservation_id,
    "foreign_units": int(fu), "lez_units": int(lu),
    "signed_envelope": {"path": f"{corridor}/taker/signed-envelope.offer.json", "sha256": env_sha},
    "unsigned_draft": {"path": f"{corridor}/taker/unsigned-draft.borsh", "sha256": draft_sha},
    "signing_key": {"path": f"{corridor}/taker/zcash.key"},
    "source_config": {"path": f"{corridor}/taker/actor-config.json", "sha256": cfg_sha},
    "agreement_output": f"{swap_dir}/agreement.borsh",
    "actor_root": f"{swap_dir}/actor",
    "receipt_output": f"{swap_dir}/receipt.json",
}
json.dump([entry], open(out, "w"), indent=2)
PY
chmod 0600 "$out/prepared.json"
docker run --rm -i --user 0:0 -v "$TAKER_VOLUME:$ZEC_ROOT" --entrypoint bash "$TAKER_IMAGE" -c \
  "umask 077; cat >$ZEC_ROOT/prepared.json && chown $NODE_UID:$NODE_UID $ZEC_ROOT/prepared.json" <"$out/prepared.json"
echo "$run_id" >runtime/zec/current
log "  catalog: one entry ($swap_id / $offer_id), swap files under $ZEC_ROOT/taker-swaps/$run_id"

if [[ "$RESTART" == 0 ]]; then
  log "8/8 skipped (--no-restart): recreate maker-node and taker-node to load corridor $run_id"
  exit 0
fi

# ---- 8. recreate the Nodes -----------------------------------------------------
log "8/8 recreating maker-node, then taker-node"
recreate_nodes
docker logs lez-maker-node 2>&1 | grep -E "ZEC|zec" | tail -3 | sed 's/^/  maker: /'
docker logs lez-taker-node 2>&1 | grep -E "ZEC|zec" | tail -3 | sed 's/^/  taker: /'
docker exec lez-maker-node bash -c "(exec 3<>/dev/tcp/127.0.0.1/$MAKER_BRIDGE_PORT)" </dev/null && log "  maker sidecar answers on 127.0.0.1:$MAKER_BRIDGE_PORT" || fail "maker sidecar not listening"
docker exec lez-taker-node bash -c "(exec 3<>/dev/tcp/127.0.0.1/$TAKER_BRIDGE_PORT)" </dev/null && log "  taker sidecar answers on 127.0.0.1:$TAKER_BRIDGE_PORT" || fail "taker sidecar not listening"
rpc_result maker maker_health '{"schema_version":1}' | python3 -c 'import json,sys; h=json.load(sys.stdin); print("  maker_health: ready=%s degraded=%s" % (h.get("ready"), h.get("degraded")))'
listed="$(rpc_result taker taker_offer_list_v1 "{\"schema_version\":1,\"route\":$ROUTE}" | python3 -c 'import json,sys; print(" ".join(e["offer"]["id"] for e in json.load(sys.stdin)["offers"]))')"
[[ " $listed " == *" $offer_id "* ]] || fail "Taker does not list $offer_id after the restart"
log "  Taker lists $offer_id; corridor $run_id is live"
