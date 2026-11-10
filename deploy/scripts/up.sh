#!/usr/bin/env bash
# up.sh — bring the whole local LEZ ↔ BTC swap stack up: chains, market and
# each party's Basecamp (its desk and Node). Idempotent on a standing stack.
#
#   ./scripts/up.sh               start or resume everything
#   ./scripts/up.sh --build       also rebuild the images from the staged payloads
#   ./scripts/up.sh --fresh-lez   recreate the LEZ chain only (Bedrock cannot
#                                 restart after long uptime); Bitcoin, wallets
#                                 and identities are kept, persisted swaps are
#                                 reset because they reference the old chain
#                                 (open swaps are abandoned, their keys kept
#                                 aside: never with real funds locked)
#   ./scripts/up.sh --fresh       wipe runtime state and volumes first, then start
#                                 under a new volume prefix (LEZ_VOLUME_PREFIX to pick one)
#
#   LEZ_IMAGES=present       use the images named by LEZ_IMAGE_PREFIX and
#                            LEZ_IMAGE_TAG as they are, pulling nothing (a
#                            bundle run on a developer's local build)
#   LEZ_IMAGES=pull          pull the images named by LEZ_IMAGE_PREFIX and
#                            LEZ_IMAGE_TAG instead of building them (the
#                            prebuilt-release path; scripts/start.sh sets it,
#                            bootstraps the market itself afterwards and
#                            prints its own banner: LEZ_NO_BANNER=1)
#   LEZ_LANES=N              also regression lanes 2..N (scripts/lanes.sh): stopped
#                            with lane 1 before a new LEZ chain, started on it
#   LEZ_API_ONLY=1           chains, market and Nodes only, wallets seeded
#                            for the evidence run; no desk suites
#   LEZ_EXPLORERS=1          also start the block explorers (Compose profile
#                            "explorers", off by default)
#
# Requires the payloads staged by from-scratch.sh (images/*/ contexts, the
# provision directory, the market root). SKIP_UI_VERIFY=1 skips the UI suites.
set -euo pipefail
DEPLOY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DEPLOY_ROOT"

BUILD=0; FRESH_LEZ=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --build) BUILD=1; shift ;;
    --fresh-lez) FRESH_LEZ=1; shift ;;
    --fresh)
      # A wipe recreates both chains; down.sh --wipe also retires the previous
      # chain's escrow deployment and bootstrap manifest in the market root, so
      # the market bootstrap deploys again on the new chain.
      echo "wiping runtime state and volumes…"
      bash scripts/down.sh --wipe >/dev/null 2>&1 || true
      # A fresh stack gets its own volume namespace, so it can never adopt
      # volumes another run or another checkout left behind under the default
      # prefix (a wipe under the old prefix does not touch those). gen-config
      # records it in runtime.env; the next down.sh --wipe removes exactly it.
      export LEZ_VOLUME_PREFIX="${LEZ_VOLUME_PREFIX:-lez-$(date +%Y%m%d%H%M%S)}"
      shift ;;
    -h|--help) sed -n 2,17p "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done

for tool in docker jq openssl xxd; do
  command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 1; }
done
mkdir -p runtime
# This stack is regression lane 1 (scripts/lane.sh): its Basecamps' names.
export LEZ_LANE=1
# shellcheck source=lane.sh
source scripts/lane.sh
maker_node="$LEZ_CONTAINER_PREFIX-maker-node"; taker_node="$LEZ_CONTAINER_PREFIX-taker-node"
load_env() { set -a; source runtime/runtime.env; set +a; export BTC_RPC_PASSWORD; }
# Every stage's wall time, in this script's own output, so each bring-up (and
# each regress.sh chain log) says where its time went.
stage_name=""; stage_began=$SECONDS
stage() { # stage <name>: closes the running stage, opens the next (none: just close)
  [[ -z "$stage_name" ]] || printf '  [time %s] %s: %d s (%d s since up.sh started)\n' \
    "$(date -u +%H:%M:%S)" "$stage_name" $((SECONDS - stage_began)) "$SECONDS"
  stage_name="${1:-}"; stage_began=$SECONDS
}
trap 'stage' EXIT

# The container health check answers before the Node serves its owner socket
# on a busy start (a Node with history reloads every swap first); the desk
# suites press "Check Node" within seconds of the deploy, so wait for the
# owner health call itself.
wait_owner_health() { # wait_owner_health <seconds>
  local deadline=$(( $(date +%s) + $1 ))
  owner_ok() { # owner_ok <role> <method> <params>
    docker exec "$LEZ_CONTAINER_PREFIX-$1-node" curl -sS --max-time 120 --unix-socket "/run/lez/$1/node.sock" \
      -H 'content-type: application/json' \
      --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":[$3]}" http://localhost/ 2>/dev/null | grep -q '"result"'
  }
  until owner_ok maker maker_health '{}' && owner_ok taker taker_health '{"schema_version":1}'; do
    (( $(date +%s) < deadline )) || { echo "the Nodes did not answer their owner health calls within $1 s" >&2; return 1; }
    sleep 3
  done
}
wait_healthy() { # wait_healthy <seconds> <container>...
  local timeout="$1"; shift
  local elapsed=0 c
  for c in "$@"; do
    until [[ "$(docker inspect -f '{{.State.Health.Status}}' "$c" 2>/dev/null)" == healthy ]]; do
      sleep 3; elapsed=$((elapsed + 3))
      [[ $elapsed -lt $timeout ]] || { echo "$c never became healthy" >&2; docker logs --tail 30 "$c" 2>&1 | tail -30; exit 1; }
    done
  done
}

if [[ "$FRESH_LEZ" == 1 && -f runtime/runtime.env ]]; then
  load_env
  stamp="$(date +%s)"
  echo "[lez] recreating the LEZ chain; old state → runtime/lez-broken-$stamp"
  # Forget the swaps that referenced the old chain first, while the Nodes still
  # run: the Taker then answers for its own swaps, so only a swap that is really
  # open has its per-swap keys archived (its LEZ leg goes with the chain, its
  # Bitcoin or Zcash leg may still need a refund). reset-swaps.sh --offline
  # stops both Nodes and leaves them stopped, so they start once on the new
  # chain instead of starting, stopping and starting. A retry (a new chain that
  # did not finalize) has nothing left to forget: the first attempt did it.
  if [[ "${LEZ_FRESH_ATTEMPT:-1}" == 1 ]] && docker volume inspect "${LEZ_VOLUME_PREFIX:-lez}-maker-state" >/dev/null 2>&1; then
    stage "forget the old chain's swaps (reset-swaps.sh --offline)"
    echo "  forgetting swaps that referenced the old chain"
    bash scripts/reset-swaps.sh --offline --abandon-open 2>&1 | tail -1
  fi
  stage "stop the old chain"
  docker compose stop maker-node taker-node >/dev/null 2>&1 || true
  # Regression lanes 2..N (LEZ_LANES, scripts/lanes.sh) share this chain: their
  # Nodes stop and forget their swaps too, and start again on the new chain.
  [[ "${LEZ_LANES:-1}" -le 1 ]] || bash scripts/lanes.sh stop "$LEZ_LANES"
  # The old chain is thrown away, so its services get no clean shutdown: they
  # stop on SIGINT only and the Basecamp-hosted one waits out its grace period.
  docker compose --profile explorers stop -t 2 lez-explorer sequencer network >/dev/null 2>&1 || true
  stage "move the old chain's state aside"
  mkdir -p "runtime/lez-broken-$stamp"
  # The LEZ services write their state as the host user. A chain from before
  # that belongs to the images' own user (65532, mode 0700), which the host
  # user cannot move on Linux (Docker Desktop maps every bind mount to the host
  # user); fall back to a root shell in a container that is already present.
  for d in bedrock sequencer indexer; do
    [[ -d "runtime/$d" ]] || continue
    mv "runtime/$d" "runtime/lez-broken-$stamp/" 2>/dev/null ||
      docker run --rm --user 0 --volume "$PWD/runtime:/runtime" \
        --entrypoint /bin/sh "${LEZ_IMAGE_PREFIX:-lez}-maker-node:${LEZ_IMAGE_TAG:-local}" \
        -c "mv '/runtime/$d' '/runtime/lez-broken-$stamp/'"
  done
  # A snapshot is 1 to 8 GB and root-owned on Linux; a day of recreations fills
  # the disk. Keep the newest LEZ_KEEP_BROKEN_CHAINS (1: the one just made).
  mapfile -t old_chains < <(find runtime -maxdepth 1 -name 'lez-broken-*' -printf '%f\n' | sort -t- -k3 -n | head -n "-${LEZ_KEEP_BROKEN_CHAINS:-1}")
  for d in "${old_chains[@]}"; do
    rm -rf "runtime/$d" 2>/dev/null ||
      docker run --rm --user 0 --volume "$PWD/runtime:/runtime" \
        --entrypoint /bin/sh "${LEZ_IMAGE_PREFIX:-lez}-maker-node:${LEZ_IMAGE_TAG:-local}" -c "rm -rf '/runtime/$d'"
  done
  # The escrow deployment and the manifest the Nodes render belong to the old
  # chain; gen-config.sh would otherwise copy that manifest into the runtime.
  market_root="$(sed -n 's/^LEZ_MARKET_ROOT=//p' runtime/runtime.env | head -1)"
  for f in bootstrap/deployment.json market-bootstrap.env; do
    [[ ! -f "$market_root/$f" ]] || mv "$market_root/$f" "$market_root/$f.chain-$stamp.bak"
  done
fi

stage "runtime config"
echo "[1/6] generating runtime config…"
env_before="$(sha256sum runtime/runtime.env 2>/dev/null | cut -c1-64 || true)"
# The genesis must fund the identities the Nodes settle as, on a rerun too:
# without this gen-config falls back to account ids recorded for another host.
if [[ -z "${LEZ_WALLET_IDENTITIES:-}" && -f runtime/runtime.env ]]; then
  market_root="$(sed -n 's/^LEZ_MARKET_ROOT=//p' runtime/runtime.env | head -1)"
  [[ ! -d "$market_root/identities" ]] || export LEZ_WALLET_IDENTITIES="$market_root/identities"
fi
bash scripts/gen-config.sh runtime
env_after="$(sha256sum runtime/runtime.env 2>/dev/null | cut -c1-64 || true)"
load_env

stage "images"
echo "[2/6] images…"
case "${LEZ_IMAGES:-build}" in
  build)
    missing=0
    for image in $(docker compose config --images 2>/dev/null | sort -u); do
      docker image inspect "$image" >/dev/null 2>&1 || missing=1
    done
    if [[ "$BUILD" == 1 || "$missing" == 1 ]]; then
      # BuildKit resolves every FROM against its registry with a short
      # deadline; pull the base images first, with retries, so a slow registry
      # cannot fail the build of payloads that are already staged.
      for image in $(grep -h '^FROM' images/*/Dockerfile | awk '{print $2}' | sort -u); do
        docker image inspect "$image" >/dev/null 2>&1 && continue
        echo "  pulling $image"
        for _ in 1 2 3; do docker pull -q "$image" >/dev/null && break; sleep 10; done
        docker image inspect "$image" >/dev/null 2>&1 || { echo "cannot pull $image" >&2; exit 1; }
      done
      for attempt in 1 2 3; do
        docker compose build && break
        [[ "$attempt" -lt 3 ]] || { echo "docker compose build failed three times" >&2; exit 1; }
        echo "  image build failed (registry deadline?); retrying in 30 s"
        sleep 30
      done
    else
      echo "  all images present (use --build to rebuild)"
    fi
    ;;
  pull)
    echo "  pulling ${LEZ_IMAGE_PREFIX:-lez}-*:${LEZ_IMAGE_TAG:-local}…"
    docker compose --profile tools pull --quiet
    ;;
  present)
    # A developer's bundle run on images built locally (tag `local`): nothing is
    # pulled; every image the compose file names must already exist.
    for image in $(docker compose config --images 2>/dev/null | sort -u); do
      docker image inspect "$image" >/dev/null 2>&1 || { echo "image $image is not present locally" >&2; exit 1; }
    done
    ;;
  *) echo "LEZ_IMAGES must be build, pull or present" >&2; exit 64 ;;
esac

# The images exist from here on (built above, pulled, or present). The role and
# LEZ Basecamp images are built on the basecamp-ui service (additional_contexts
# service:basecamp-ui), and for those `compose up` runs the whole build again on
# every call, cached but seconds long; --no-build starts what is there.
stage "start the chains (compose up)"
echo "[3/6] starting chains…"
# The LEZ services run as the host user; state an older stack left as the
# images' own user is not theirs to open.
for d in bedrock sequencer indexer; do
  [[ -O "runtime/$d" ]] || { echo "runtime/$d belongs to another user (a chain from an older stack): ./scripts/up.sh --fresh-lez" >&2; exit 1; }
done
# The containers of services this file no longer has: the bedrock and indexer
# Basecamps (now the one network Basecamp, whose aliases they would shadow) and
# the init containers the images' own data directories replaced.
docker rm -f lez-bedrock lez-indexer lez-state-init lez-bitcoin-init lez-zebra-init >/dev/null 2>&1 || true
docker compose up -d --no-build bitcoin-core btc-miner zebra network sequencer
[[ "${LEZ_EXPLORERS:-0}" != 1 ]] || docker compose --profile explorers up -d --no-build btc-explorer lez-explorer
stage "chains healthy"
wait_healthy 2400 lez-bitcoin-core lez-zebra lez-network lez-sequencer
# A new LEZ chain can stop finalizing within its first minute (README, "a
# frozen devnet"); everything after this point would then wait on it in vain
# (the indexer repair, the vault claims). A healthy new chain's indexer serves
# no finalized height (null) for ~100 s after the sequencer's first block, then
# moves every ~2 s (1 s blocks). So wait for the first height
# (LEZ_FRESH_FIRST_FINALITY_SECONDS, 300) and then for it to move
# (LEZ_FRESH_FINALITY_SECONDS, 60), and start another chain if either never comes.
if [[ "$FRESH_LEZ" == 1 ]]; then
  stage "the new chain finalizes"
  lez_finalized() {
    curl -s --max-time 10 -H 'content-type: application/json' \
      --data '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' \
      http://127.0.0.1:8779/ 2>/dev/null | sed -n 's/.*"result":\([0-9][0-9]*\).*/\1/p'
  }
  # read every 2 s: once it serves a height, a healthy chain finalizes every second or two
  first=""; moved=0; now=""
  first_deadline=$(( SECONDS + ${LEZ_FRESH_FIRST_FINALITY_SECONDS:-300} ))
  while (( SECONDS < first_deadline )); do
    first="$(lez_finalized)"; [[ -z "$first" ]] || break
    sleep 2
  done
  if [[ -n "$first" ]]; then
    for _ in $(seq 1 $(( ${LEZ_FRESH_FINALITY_SECONDS:-60} / 2 ))); do
      sleep 2; now="$(lez_finalized)"
      if [[ -n "$now" && "$now" -gt "$first" ]]; then moved=1; break; fi
    done
  fi
  if (( ! moved )); then
    [[ "${LEZ_FRESH_ATTEMPT:-1}" -lt "${LEZ_FRESH_ATTEMPTS:-6}" ]] || { echo "the new LEZ chain does not finalize (attempt ${LEZ_FRESH_ATTEMPT:-1})" >&2; exit 1; }
    echo "  the new LEZ chain does not finalize (finalized ${first:-none}); starting another one"
    stage; LEZ_FRESH_ATTEMPT=$(( ${LEZ_FRESH_ATTEMPT:-1} + 1 )) exec bash "$0" --fresh-lez
  fi
  echo "  the new LEZ chain finalizes (${first} -> ${now})"
fi
stage "indexer check (repair-indexer.sh)"
bash scripts/repair-indexer.sh >/dev/null 2>&1 || true
stage "Bitcoin Core wallets (seed-btc-wallets.py)"
# The Nodes' Core wallets (lez-maker, lez-taker) and the Taker's regtest
# balance; idempotent, and after a wipe nothing else creates them.
python3 scripts/seed-btc-wallets.py

before="$(sha256sum runtime/market-bootstrap.env 2>/dev/null | cut -c1-64 || true)"
if [[ "${LEZ_IMAGES:-build}" == pull || "${LEZ_IMAGES:-build}" == present ]]; then
  echo "[4/6] market: bootstrapped by start.sh from the tools image after this"
else
  stage "market bootstrap"
  echo "[4/6] market (escrow program, vault claims, bootstrap manifest)…"
  if ! bash scripts/bootstrap-market.sh 2>&1 | tail -3; then
    # A new chain can stop finalizing within its first minute (README, "a frozen
    # devnet"); the bootstrap then waits for its claims in vain and only another
    # chain helps.
    [[ "$FRESH_LEZ" == 1 && "${LEZ_FRESH_ATTEMPT:-1}" -lt "${LEZ_FRESH_ATTEMPTS:-6}" ]] || exit 1
    echo "  the new chain does not finalize; recreating it once more"
    stage; LEZ_FRESH_ATTEMPT=$(( ${LEZ_FRESH_ATTEMPT:-1} + 1 )) exec bash "$0" --fresh-lez
  fi
fi
after="$(sha256sum runtime/market-bootstrap.env 2>/dev/null | cut -c1-64 || true)"

stage "start the Nodes (compose up)"
echo "[5/6] Nodes…"
# The containers of services this file no longer has: the third Basecamp that
# held both desks (it publishes 5901, the Maker's Basecamp's VNC port now) and
# the init containers the role images' own state directories replaced.
docker rm -f lez-basecamp-ui lez-maker-init lez-taker-init >/dev/null 2>&1 || true
# A changed runtime.env (a timing profile switch, say) reaches the Nodes only
# through a recreate; entrypoints read it once.
if [[ "$before" != "$after" || "$env_before" != "$env_after" || "$FRESH_LEZ" == 1 ]]; then
  if docker container inspect "$maker_node" "$taker_node" >/dev/null 2>&1; then
    # Both Basecamps start together. The Taker's Node pins the Maker's Delivery
    # identity when it starts, which is why taker-node waits for a healthy
    # maker-node (compose.yaml); with the identity the last Maker published
    # removed first, the Taker's Node waits for the one the new Maker publishes
    # (node-entrypoint.sh) instead. Both containers existing means their
    # volumes exist, so --no-deps skips nothing else.
    docker run --rm --network none --user 4713:4713 --volumes-from "$maker_node" --entrypoint rm \
      "$(docker inspect -f '{{.Config.Image}}' "$maker_node")" -f /delivery/maker-delivery-identity.pub
    docker compose up -d --no-build --force-recreate maker-node
    docker compose up -d --no-build --force-recreate --no-deps taker-node
  else
    # Basecamp, which hosts the roles' LEZ signers, starts first (depends_on).
    docker compose up -d --no-build --force-recreate maker-node taker-node
  fi
else
  docker compose up -d --no-build maker-node taker-node
fi
stage "Nodes healthy"
wait_healthy 1200 "$maker_node" "$taker_node"
stage "Nodes answer owner health"
wait_owner_health 120
if [[ "${LEZ_LANES:-1}" -gt 1 ]]; then
  stage "lanes 2..$LEZ_LANES (lanes.sh up)"
  bash scripts/lanes.sh up "$LEZ_LANES"
fi
stage "market check (verify-market.py)"
[[ "${LEZ_IMAGES:-build}" == pull || "${LEZ_IMAGES:-build}" == present ]] || python3 scripts/verify-market.py 2>&1 | tail -1

# The finality heartbeat (Part C): on unless runtime.env says LEZ_HEARTBEAT_SECONDS=0
# or the market root has no heartbeat identities (an older root; gen-config
# then funds no heartbeat account and the bootstrap claims none).
stage "finality heartbeat"
heartbeat_seconds="$(sed -n 's/^LEZ_HEARTBEAT_SECONDS=//p' runtime/runtime.env | head -1)"
heartbeat_allocation="$(sed -n 's/^LEZ_HEARTBEAT_ALLOCATION=//p' runtime/runtime.env | head -1)"
if [[ "${heartbeat_seconds:-0}" != 0 && "${heartbeat_allocation:-0}" != 0 ]]; then
  echo "[5/6] finality heartbeat (one unit every ${heartbeat_seconds} s)…"
  mkdir -p runtime/heartbeat
  docker compose --profile heartbeat up -d --no-build heartbeat
else
  echo "[5/6] finality heartbeat off (LEZ_HEARTBEAT_SECONDS=${heartbeat_seconds:-0}, allocation ${heartbeat_allocation:-0})"
fi

if [[ "${LEZ_API_ONLY:-0}" == 1 ]]; then
  echo "API stack ready; run python3 scripts/record-evidence.py after market bootstrap."
  exit 0
fi
# Each party's desk is the one its own Basecamp opened (maker-node, taker-node).
stage "desk suites"
echo "[6/6] the parties' desks…"
ui_failed=0
if [[ "${SKIP_UI_VERIFY:-0}" != "1" ]]; then
  for role in maker taker; do
    if ! docker exec "$LEZ_CONTAINER_PREFIX-$role-node" node /ui-tests/verify.mjs "$role" 2>&1 | grep -E '✓|✗|passed|failed|^    [A-Za-z]'; then
      ui_failed=1
    fi
  done
fi

# start.sh prints its own banner once the market is bootstrapped.
[[ "${LEZ_NO_BANNER:-0}" != 1 ]] || exit "$ui_failed"
if [[ "${LEZ_EXPLORERS:-0}" == 1 ]]; then
  explorers="BTC http://127.0.0.1:3002, LEZ http://127.0.0.1:3003"
else
  explorers="off (LEZ_EXPLORERS=1 ./scripts/up.sh starts them)"
fi
cat <<BANNER

──────────────────────────────────────────────────────────────────
 LEZ swap stack is up (volumes: ${LEZ_VOLUME_PREFIX:-lez}-*)

   Bitcoin regtest RPC   http://127.0.0.1:18443   (auto-mining, 1 block / ${MINE_INTERVAL:-120} s)
   LEZ indexer RPC       http://127.0.0.1:8779
   explorers             ${explorers}
   Maker's Basecamp      vnc://127.0.0.1:5901     (VNC password: ${VNC_PASSWORD:-lezswap}; the Maker desk)
   Taker's Basecamp      vnc://127.0.0.1:5902     (VNC password: ${VNC_PASSWORD:-lezswap}; the Taker desk)
   Maker Node            docker exec lez-maker-node lez-maker-cli --socket /run/lez/maker/node.sock health

 logs:  docker compose logs -f <service>      down:  ./scripts/down.sh [--wipe]
 a swap looks stuck?  ./scripts/verify-all.sh, then ./scripts/up.sh --fresh-lez if Bedrock's slot is frozen
──────────────────────────────────────────────────────────────────
BANNER
[[ "$ui_failed" == 0 ]] || { echo "a desk suite failed; see above" >&2; exit 1; }
