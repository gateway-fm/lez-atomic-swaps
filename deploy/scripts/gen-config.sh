#!/usr/bin/env bash
# Generates the runtime config tree for the local LEZ v0.2 devnet + Bitcoin
# regtest stack. Mirrors scripts/run-lez-v02-stack.sh from the repo (channel,
# genesis allocations, deployment-settings rendering) but static, local-test
# oriented, and compose-first.
set -euo pipefail

DEPLOY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly DEPLOY_ROOT
# shellcheck source=host.sh
source "$DEPLOY_ROOT/scripts/host.sh"
RUNTIME="${1:-$DEPLOY_ROOT/runtime}"
readonly RUNTIME
readonly LEZ_SOURCE="$DEPLOY_ROOT/assets/lez-source"

readonly channel_id="b6adb2d238911395adde0b2f40b880ec03ffd1a3a8d97e7df8cacadf08873748"
readonly maker_account_id="B1UN3hPgxacgHKBRoThcAmsPajGcUf6YXUhgB36x4DAd"
readonly taker_account_id="34Kqgek6R7N1zU5FSJz8ziXwSPEPCuWGcn1T7GCVrfib"
# 2 000 000 LEZ each: a whole scenario matrix on one chain with margin. A ZEC
# corridor's lot is 50 000 LEZ and it leaves the depositor for good on a
# completed swap (the Taker in TakerSellsLez, the Maker in TakerSellsForeign);
# the old 200 000 covered four swaps, and the fifth failed its LEZ funding
# (the escrow program reports an insufficient depositor balance as
# "native funding account binding mismatch", error 6003).
readonly maker_genesis_allocation=2000000
readonly taker_genesis_allocation=2000000
# Persistent settlement wallets (owner account ids of the identities held in
# the market root, LEZ_MARKET_ROOT). Swaps run on this long-standing chain and
# these accounts accumulate real balances across swaps — the local analog of
# funded wallets on a real network.
# LEZ_WALLET_IDENTITIES (the market root's identities directory) overrides
# the recorded ids with the ids of the identities that directory holds.
wallet_account_id() { # wallet_account_id <wallet> <recorded-id>
  local identity="${LEZ_WALLET_IDENTITIES:-}/$1.json"
  if [[ -n "${LEZ_WALLET_IDENTITIES:-}" && -f "$identity" ]]; then jq -er '.account_id' "$identity"; else echo "$2"; fi
}
wallet_munich_account_id="$(wallet_account_id maker-munich-01 BD6TpNTSLjeonDFmA3PXg6YtDy7xXt2LTm46266NpwJY)"
wallet_basel_account_id="$(wallet_account_id maker-basel-02 A81AE1KTGdZ5GCDfy4XdUe9XvgNmkFzfgZcRkkQXm8vm)"
wallet_zurich_account_id="$(wallet_account_id taker-zurich-01 4vDRakzuvKqJFJZ6k4ig3ybzds6fTLv1xDpwU283SwBM)"
wallet_limmat_account_id="$(wallet_account_id taker-limmat-02 5A8bRmav5wjYQex6z7SpuuNNyhesqHwweAqjc3eWfchH)"
readonly wallet_munich_account_id wallet_basel_account_id wallet_zurich_account_id wallet_limmat_account_id
readonly wallet_maker_allocation=2000000
readonly wallet_taker_allocation=2000000
# The finality heartbeat's two accounts (sender, recipient): funded only when
# their identities exist in the market root (from-scratch.sh / start.sh mint
# them); 100 000 units is 23 days of one unit per 20 s. LEZ_HEARTBEAT_SECONDS
# (0 disables the service) and LEZ_HEARTBEAT_STALL_SECONDS reach up.sh and the
# service through runtime.env.
heartbeat_pulse_account_id="$(wallet_account_id heartbeat-pulse-01 "")"
heartbeat_echo_account_id="$(wallet_account_id heartbeat-echo-02 "")"
heartbeat_allocation=0
if [[ -n "$heartbeat_pulse_account_id" && -n "$heartbeat_echo_account_id" ]]; then heartbeat_allocation=100000; fi
readonly heartbeat_pulse_account_id heartbeat_echo_account_id heartbeat_allocation
# Regression lanes 3..9 (scripts/lane.sh) settle as wallets of their own,
# maker-laneNN and taker-laneNN, funded like the market wallets once their
# identities exist (scripts/lanes.sh mint); lane 2 uses basel/limmat above.
lane_genesis="[]"
for lane in 3 4 5 6 7 8 9; do
  for wallet in "$(printf 'maker-lane%02d' "$lane")" "$(printf 'taker-lane%02d' "$lane")"; do
    account="$(wallet_account_id "$wallet" "")"
    [[ -n "$account" ]] || continue
    lane_genesis="$(jq -c --arg account "$account" --argjson amount "$wallet_maker_allocation" \
      '. + [{"supply_account": {"account_id": $account, "balance": $amount}}]' <<<"$lane_genesis")"
  done
done
readonly lane_genesis
heartbeat_seconds="${LEZ_HEARTBEAT_SECONDS:-20}"
heartbeat_stall_seconds="${LEZ_HEARTBEAT_STALL_SECONDS:-60}"
[[ "$heartbeat_seconds" =~ ^[0-9]+$ && "$heartbeat_stall_seconds" =~ ^[0-9]+$ ]] || { echo "LEZ_HEARTBEAT_SECONDS and LEZ_HEARTBEAT_STALL_SECONDS must be integers" >&2; exit 1; }

# The stack this runtime belongs to (scripts/lane.sh derives its names and
# ports): a prefix or port offset other than the default is recorded in
# runtime.env, so every script run from this deploy directory acts on this
# stack, and a runtime made for one stack never serves another.
stack_prefix=lez; stack_offset=0
if [[ -f "$RUNTIME/runtime.env" ]]; then
  stack_prefix="$(sed -n 's/^LEZ_STACK_PREFIX=//p' "$RUNTIME/runtime.env" | head -1)"; stack_prefix="${stack_prefix:-lez}"
  stack_offset="$(sed -n 's/^LEZ_STACK_PORT_OFFSET=//p' "$RUNTIME/runtime.env" | head -1)"; stack_offset="${stack_offset:-0}"
fi
if [[ -n "${LEZ_STACK_PREFIX:-}" && "$LEZ_STACK_PREFIX" != "$stack_prefix" && -f "$RUNTIME/runtime.env" ]]; then
  echo "$RUNTIME belongs to the stack '$stack_prefix', not '$LEZ_STACK_PREFIX': a second stack runs from a deploy directory of its own" >&2
  exit 1
fi
stack_prefix="${LEZ_STACK_PREFIX:-$stack_prefix}"; stack_offset="${LEZ_STACK_PORT_OFFSET:-$stack_offset}"
[[ "$stack_prefix" =~ ^[a-z][a-z0-9]*(-[a-z0-9]+)*$ && "$stack_offset" =~ ^(0|[1-9][0-9]{0,4})$ ]] ||
  { echo "invalid LEZ_STACK_PREFIX or LEZ_STACK_PORT_OFFSET" >&2; exit 1; }
stack_lines=()
[[ "$stack_prefix" == lez ]] || stack_lines+=("LEZ_STACK_PREFIX=$stack_prefix")
[[ "$stack_offset" == 0 ]] || stack_lines+=("LEZ_STACK_PORT_OFFSET=$stack_offset")

# Preserve the selected volume namespace on ordinary restarts too; another
# stack's volumes are named after its prefix.
volume_prefix="${LEZ_VOLUME_PREFIX:-}"
if [[ -z "$volume_prefix" && -f "$RUNTIME/runtime.env" ]]; then
  volume_prefix="$(sed -n 's/^LEZ_VOLUME_PREFIX=//p' "$RUNTIME/runtime.env" | head -1)"
fi
volume_prefix="${volume_prefix:-$stack_prefix}"
[[ "$volume_prefix" =~ ^[a-zA-Z0-9][a-zA-Z0-9_.-]*$ ]] || { echo "invalid LEZ_VOLUME_PREFIX" >&2; exit 1; }

umask 077

# --- maker/taker runtime ----------------------------------------------------
# role_files <dir> <maker wallet> <taker wallet> <label>: the two Nodes' identities and
# keys under <dir>/lez/<role> and their Zebra's config under <dir>/zebra: the
# stack's own in runtime/, a regression lane's in runtime/lanes/<k>
# (scripts/lane.sh).
role_files() {
  local dir="$1" label="$4" pair role wallet
  identities_root="${LEZ_WALLET_IDENTITIES:-$market_root/identities}"
  # The role keys reach the Nodes (uid 4713) through a bind mount, which keeps host
  # ownership. On Linux the role container also carries the host user's group
  # (compose.yaml group_add, LEZ_HOST_GID), so the copies are group-readable only;
  # Docker Desktop on macOS shares files under its own access rules, so there they
  # stay world-readable (README, devnet caveat).
  if [[ "$(uname -s)" == Linux ]]; then key_mode=0640 key_dir_mode=0750; else key_mode=0644 key_dir_mode=0755; fi
  for pair in "maker:$2" "taker:$3"; do
    role="${pair%%:*}"; wallet="${pair##*:}"
    mkdir -p "$dir/lez/$role"
    chmod "$key_dir_mode" "$dir/lez/$role"
    if [[ -n "$identities_root" && -f "$identities_root/$wallet/lez-signer.key" ]]; then
      cp "$identities_root/$wallet/lez-signer.key" "$dir/lez/$role/lez-signer.key"
      cp "$identities_root/$wallet/identity.json" "$dir/lez/$role/identity.json"
    else
      echo "warning: $wallet identity unavailable; $role Node runs without the Bitcoin lifecycle" >&2
    fi
    # The entrypoint copies these owner-private into the Node's state volume.
    chmod "$key_mode" "$dir/lez/$role"/* 2>/dev/null || true
  done
  # Each role's Zcash key, minted once beside its LEZ identity and kept across
  # reruns: the Maker's is also its Delivery signing key (the entrypoint installs
  # it; the ZEC ceremony binds Delivery identity, Chat key and Zcash key into one
  # key) and zebra mines to its transparent address, so the Maker owns the
  # coinbase the ZEC provisioner spends; the Taker's key funds Zcash in
  # TakerSellsForeign (scripts/zec-provision.sh pays it once from the Maker's).
  # Nothing here is a constant: a fresh runtime mints fresh keys. The address
  # comes from zec-local-poc-identity, staged on the host by from-scratch.sh
  # --only rust or, once the stage is gone, run from the Taker image.
  zcash_transparent_address() { # zcash_transparent_address <owner-private key file>
    local key="$1" tool="$DEPLOY_ROOT/images/taker-node/zec-local-poc-identity" identity
    if [[ -x "$tool" && "$(uname -s)" == Linux ]]; then
      identity="$("$tool" --zcash-key-file "$key")"
    elif [[ -x "$tool" ]]; then
      # The staged tool is a Linux binary: macOS runs it in the Node images'
      # base (Docker Desktop), before any Taker image exists.
      identity="$(docker run --rm --network none --user "$(id -u):$(id -g)" -v "$tool:/zec-local-poc-identity:ro" \
        -v "$key:/in/zcash.key:ro" --entrypoint /zec-local-poc-identity debian:bookworm-slim \
        --zcash-key-file /in/zcash.key </dev/null)"
    else
      identity="$(docker run --rm --user "$(id -u):$(id -g)" -v "$key:/in/zcash.key:ro" \
        --entrypoint zec-local-poc-identity "${LEZ_IMAGE_PREFIX:-lez}-taker-node:${LEZ_IMAGE_TAG:-local}" \
        --zcash-key-file /in/zcash.key </dev/null)"
    fi || { echo "zec-local-poc-identity failed: stage it (from-scratch.sh --only rust) or build the Taker image first" >&2; return 1; }
    python3 -c 'import json, sys; print(json.load(sys.stdin)["transparent_address"])' <<<"$identity"
  }
  for role in maker taker; do
    if [[ ! -s "$dir/lez/$role/zcash.key" ]]; then
      (umask 077; openssl rand -hex 32 >"$dir/lez/$role/zcash.key")
      echo "minted the $role Zcash key: $label/lez/$role/zcash.key"
    fi
  done
  # The identity tool reads owner-private files only; the mounted copy is
  # readable by the Node like lez-signer.key (the entrypoint re-installs it 0600),
  # so derive from a private copy under runtime/secrets.
  # (an absolute path: the identity tool refuses a relative key path, and up.sh
  # passes the runtime directory as a relative one)
  # Without the tool (neither staged nor in a built Taker image, e.g. a checkout
  # that has not built anything yet) the ZEC lane cannot run, like a missing LEZ
  # identity above: zebra keeps the template's miner address and says so. A tool
  # that is present but fails stops the script.
  zec_identity_tool_available() {
    [[ -x "$DEPLOY_ROOT/images/taker-node/zec-local-poc-identity" ]] && return 0
    command -v docker >/dev/null 2>&1 &&
      docker image inspect "${LEZ_IMAGE_PREFIX:-lez}-taker-node:${LEZ_IMAGE_TAG:-local}" >/dev/null 2>&1
  }
  chmod "$key_mode" "$dir/lez/maker/zcash.key" "$dir/lez/taker/zcash.key"
  mkdir -p "$dir/zebra"
  if zec_identity_tool_available; then
    maker_zcash_private="$(mktemp "$(cd "$RUNTIME" && pwd -P)/secrets/zcash-key.XXXXXX")"
    trap 'rm -f "$maker_zcash_private"' EXIT
    install -m 0600 "$dir/lez/maker/zcash.key" "$maker_zcash_private"
    zebra_miner_address="$(zcash_transparent_address "$maker_zcash_private")" || exit 1
    rm -f "$maker_zcash_private"
    trap - EXIT
    sed "s|^miner_address = .*|miner_address = \"$zebra_miner_address\"|" "$DEPLOY_ROOT/assets/zebra/zebrad.toml" >"$dir/zebra/zebrad.toml"
    echo "zebra mines to the Maker's Zcash key ($zebra_miner_address): $label/zebra/zebrad.toml"
  else
    cp "$DEPLOY_ROOT/assets/zebra/zebrad.toml" "$dir/zebra/zebrad.toml"
    echo "warning: zec-local-poc-identity unavailable (stage it with from-scratch.sh --only rust or build the Taker image); zebra keeps the template miner address and the ZEC lane cannot run" >&2
  fi
  chmod 0644 "$dir/zebra/zebrad.toml"
}

# A regression lane (LEZ_LANE >= 2, scripts/lane.sh) renders its own role files
# only: its wallets' identities, its Zcash keys and its Zebra's miner address,
# under runtime/lanes/<k>. Everything else stays lane 1's.
if [[ "${LEZ_LANE:-1}" != 1 ]]; then
  # shellcheck source=lane.sh
  source "$DEPLOY_ROOT/scripts/lane.sh"
  [[ -s "$RUNTIME/runtime.env" ]] || { echo "lane $LEZ_LANE needs lane 1's runtime first (gen-config.sh runtime)" >&2; exit 1; }
  market_root="$(sed -n 's/^LEZ_MARKET_ROOT=//p' "$RUNTIME/runtime.env" | head -1)"
  [[ -n "$market_root" && -d "$market_root" ]] || { echo "lane $LEZ_LANE: runtime.env names no market root" >&2; exit 1; }
  lane_dir="$RUNTIME/lanes/$LEZ_LANE"
  mkdir -p "$lane_dir"
  role_files "$lane_dir" "$LEZ_LANE_MAKER_WALLET" "$LEZ_LANE_TAKER_WALLET" "runtime/lanes/$LEZ_LANE"
  echo "lane $LEZ_LANE generated at $lane_dir ($LEZ_LANE_MAKER_WALLET, $LEZ_LANE_TAKER_WALLET)"
  exit 0
fi

echo "runtime root: $RUNTIME"
# The LEZ network and the sequencer run as the host user (compose.yaml), so
# their state directories are the host user's own.
mkdir -p "$RUNTIME"/{config,bedrock,indexer,sequencer,sockets,btc,secrets}

# The proof view and the explorer's evidence index start from the certified
# sample until export-node-evidence.py publishes a swap the Nodes settled.
mkdir -p "$RUNTIME/evidence"
if [[ ! -s "$RUNTIME/m3-btc-ui-evidence.json" ]]; then
  cp "$DEPLOY_ROOT/assets/certified-evidence-m5arm-08180005-ui.json" \
    "$RUNTIME/m3-btc-ui-evidence.json"
  chmod 0666 "$RUNTIME/m3-btc-ui-evidence.json"
fi
[[ -s "$RUNTIME/evidence/certified-m5arm-08180005.json" ]] \
  || cp "$DEPLOY_ROOT/assets/certified-evidence-m5arm-08180005-ui.json" "$RUNTIME/evidence/certified-m5arm-08180005.json"
chmod 0644 "$RUNTIME/evidence/"*.json  # the explorer reads them as another uid
chmod 0666 "$RUNTIME/m3-btc-ui-evidence.json"
jq -e '
  .kind == "m3_btc_ui_evidence"
  and .result == "passed"
  and .terminal == {phase:"completed",revision:4}
  and (.effects | length) == 5
  and .private_material_disclosed == false
' "$RUNTIME/m3-btc-ui-evidence.json" >/dev/null

# --- LEZ: deployment settings ------------------------------------------------
# The timing profile is kept across reruns (read here, before the old file is
# replaced): a rerun without LEZ_TIMING_PROFILE must not silently move the
# Nodes back to `local`.
previous_timing_profile=""
previous_actor_trace=""
previous_lez_timing=()
if [[ -s "$RUNTIME/runtime.env" ]]; then
  previous_timing_profile="$(sed -n 's/^LEZ_TIMING_PROFILE=//p' "$RUNTIME/runtime.env" | head -1)"
  previous_actor_trace="$(sed -n 's/^LEZ_BTC_ACTOR_TRACE=//p' "$RUNTIME/runtime.env" | head -1)"
  for key in LEZ_LEZ_BLOCK_SECONDS LEZ_LEZ_STEP_SECONDS LEZ_LEZ_STALL_SECONDS LEZ_BTC_BLOCK_SECONDS; do
    previous_lez_timing+=("$(sed -n "s/^$key=//p" "$RUNTIME/runtime.env" | head -1)")
  done
fi

# Bedrock starts from upstream's genesis with three devnet changes.
# - Finality depth 2 (security_param, upstream 5). Bedrock's LIB trails its
#   tip by ~2 slots instead of ~5, and an epoch is 40 s instead of 100 s. 1 is
#   too low: Cryptarchia's s_gen (k / 4f) is then 0, which its bootstrap fork
#   choice refuses with a panic.
# - The 48 genesis notes of key 2e03.. hold 10^12 each instead of 10^6, and 40
#   of them belong to the node's own SDP key ed26.. (in its KMS and wallet)
#   instead: those pay for the sequencer's inscriptions, and the other 8 are the
#   block producer's alone. Bedrock's leader can only win a slot with a note
#   that is in its epoch's aged snapshot and still unspent, and the sequencer
#   spends one note per LEZ block, oldest first. Paid from the leader's own
#   notes (upstream), it went through all 48 in ~100 s at ~2 s a block, so the
#   leader ran on one or two eligible notes at the end of each epoch; a little
#   faster (a LEZ block every 1.5 s, the merged LEZ network's pace) and no note
#   was left: Bedrock stopped at an epoch turn, the wallet's reservations no
#   longer expired, and the sequencer stopped on "Wallet does not have enough
#   funds, available=0" (FREEZE-INVESTIGATION.md §3.3).
#   Bedrock re-prices storage at every epoch turn and the price only drifts up
#   (~+0.5 per turn: +45 an hour with 40 s epochs). Upstream's 48 x 10^6 ran
#   dry in 46 minutes at 1 s blocks. The sequencer's 4 x 10^13 lasts ~40 days at
#   the tip set below.
# - The chain's genesis time (in the genesis inscription) is when the chain is
#   made, not upstream's (2026-07-31). Bedrock counts slots (one a second) from
#   it, and the LEZ indexer's cold start reads every slot from 0 to the last
#   irreversible one in 100-slot batches before it stores the first LEZ block:
#   with upstream's time, ~9 million slots, ~100 s of a new chain's first
#   finality and of every re-index (scripts/repair-indexer.sh).
# The notes' values and owners are part of the genesis transaction and the time
# is part of the inscription, so the genesis block root and the SDP
# declaration's locked note id change with them. builder/regenesis computes
# both (README there), in the LEZ network's image, for a new chain: one whose
# Bedrock state is empty and whose time runtime.env does not record yet
# (LEZ_GENESIS_TIME; up.sh --fresh-lez drops it with the old chain). Every
# later run, a restart of the same chain too, renders the same genesis. The values
# for upstream's time were computed once and are checked here against the
# upstream file they were computed from; a chain made before LEZ_GENESIS_TIME
# was recorded, or by an image without regenesis, keeps them.
readonly upstream_genesis_root=cb5951ac1ffa1aa5d0e585fb54e784bd9c025b28d752324e98b3837f34648692
readonly upstream_locked_note=7e449a14172fc90679f6fca7b49a2d58c305ebf7ac42ef20202e533c31115222
readonly upstream_inscription=05302e322e3123766c6a2d2ddf918544bca603c5a291c7dd1b902d6769ff4b00021506780e075c06051a
readonly upstream_genesis_time=1785493027
readonly fixed_genesis_root=d0d73d3b253006bdb8086dc867442dac1a81e8fdbcdfceb74f0c7a9761db0192
readonly fixed_locked_note=a4c6c9828735678bf91667e26ab47e26b8196b6c1be137b7a9e686be22fd6205
readonly devnet_note_value=1000000000000
readonly devnet_leader_notes=8
# Both are public keys (note owners in upstream's public devnet genesis), not secrets.
readonly leader_note_owner=2e03b2eff5a45478e7e79668d2a146cf2c5c7925bce927f2b1c67f2ab4fc0d26
readonly sequencer_note_owner=ed266e6e887b9b97059dc1aa1b7b2e19b934291753c6336a163fe4ebaa28e717
readonly devnet_security_param=2
network_image="${LEZ_IMAGE_PREFIX:-lez}-network-module:${LEZ_IMAGE_TAG:-local}"
previous_genesis_time=""
[[ ! -s "$RUNTIME/runtime.env" ]] || previous_genesis_time="$(sed -n 's/^LEZ_GENESIS_TIME=//p' "$RUNTIME/runtime.env" | head -1)"
has_regenesis() { docker image inspect "$network_image" >/dev/null 2>&1 &&
  docker run --rm --network none --entrypoint /bin/sh "$network_image" -c 'test -x /usr/local/bin/regenesis' 2>/dev/null; }
regenesis_present=0
if [[ -n "$(ls -A "$RUNTIME/bedrock" 2>/dev/null)" || -n "$previous_genesis_time" ]]; then
  genesis_time="${previous_genesis_time:-$upstream_genesis_time}"
  [[ "$genesis_time" == "$upstream_genesis_time" ]] || ! has_regenesis || regenesis_present=1
elif has_regenesis; then
  genesis_time="$(date +%s)"; regenesis_present=1
else
  genesis_time="$upstream_genesis_time"
  echo "$network_image has no regenesis (from-scratch.sh --only build:regenesis, --only stage, then build it): the new chain keeps upstream's genesis time" >&2
fi
[[ "$genesis_time" =~ ^[1-9][0-9]{9}$ ]] || { echo "runtime.env's LEZ_GENESIS_TIME is not a unix time in seconds: '$genesis_time'" >&2; exit 1; }
if [[ "$genesis_time" == "$upstream_genesis_time" ]]; then
  devnet_genesis_root="$fixed_genesis_root"; devnet_locked_note="$fixed_locked_note"; devnet_inscription="$upstream_inscription"
else
  (( regenesis_present )) || { echo "this chain's genesis time $genesis_time needs regenesis in $network_image (from-scratch.sh --only build:regenesis), or a new chain: scripts/up.sh --fresh-lez" >&2; exit 1; }
  # It prints the root and the locked note as YAML byte lists, the inscription in hex.
  regenesis_out="$(docker run --rm --network none --entrypoint /usr/local/bin/regenesis \
    -v "$LEZ_SOURCE/bedrock/deployment-settings.yaml:/upstream.yaml:ro" "$network_image" /upstream.yaml \
    "$leader_note_owner" "$devnet_note_value" $(( 48 - devnet_leader_notes )) "$sequencer_note_owner" "time=$genesis_time")"
  regenesis_bytes() { awk -v key="$1" '$1 ~ /^[a-z_]+$/ { on = ($1 == key) }
    on { for (i = 1; i <= NF; i++) if ($i ~ /^[0-9]+$/) printf "%02x", $i }' <<<"$regenesis_out"; }
  devnet_genesis_root="$(regenesis_bytes block_root)"
  devnet_locked_note="$(regenesis_bytes locked_note_id)"
  devnet_inscription="$(sed -n 's/^inscription //p' <<<"$regenesis_out")"
  [[ "$devnet_genesis_root" =~ ^[0-9a-f]{64}$ && "$devnet_locked_note" =~ ^[0-9a-f]{64}$ && "$devnet_inscription" =~ ^[0-9a-f]{${#upstream_inscription}}$ ]] ||
    { echo "regenesis printed no genesis for time $genesis_time:" >&2; echo "$regenesis_out" >&2; exit 1; }
  echo "LEZ genesis time $genesis_time ($(date -u -d "@$genesis_time" +%FT%TZ 2>/dev/null || date -u -r "$genesis_time" +%FT%TZ)), root $devnet_genesis_root"
fi
readonly genesis_time devnet_genesis_root devnet_locked_note devnet_inscription
awk -v up_root="$upstream_genesis_root" -v root="$devnet_genesis_root" \
    -v up_locked="$upstream_locked_note" -v locked="$devnet_locked_note" \
    -v up_inscription="$upstream_inscription" -v inscription="$devnet_inscription" \
    -v value="$devnet_note_value" -v k="$devnet_security_param" \
    -v leader="$leader_note_owner" -v leader_notes="$devnet_leader_notes" -v sequencer="$sequencer_note_owner" '
  # a note line is followed by its owner: hold it until the owner is known
  held != "" { if (index($0, "pk: '\''" leader "'\''")) { sub(/value: 1000000$/, "value: " value, held); notes++
                 if (notes > leader_notes) { sub(leader, sequencer); moved++ } }
               print held; held = "" }
  /^ *- value: 1000000$/ { held = $0; next }
  /^  security_param: 5$/ { sub(/5$/, k); params++ }
  $0 ~ "block_root: " up_root "$" { sub(up_root, root); roots++ }
  $0 ~ "locked_note_id: .?" up_locked { sub(up_locked, locked); lockeds++ }
  $0 ~ "inscription: .?" up_inscription { sub(up_inscription, inscription); inscriptions++ }
  { print }
  END { if (notes != 48 || moved != 48 - leader_notes || params != 1 || roots != 1 || lockeds != 1 || inscriptions != 1) {
          printf "deployment-settings.yaml is not the upstream file the devnet genesis was computed from (%d notes, %d security_param, %d roots, %d locked notes, %d inscriptions)\n", notes, params, roots, lockeds, inscriptions > "/dev/stderr"; exit 1 } }
' "$LEZ_SOURCE/bedrock/deployment-settings.yaml" >"$RUNTIME/config/deployment-settings.yaml"

jq --arg channel "$channel_id" \
  '.bedrock_config.addr = "http://bedrock:18080" | .channel_id = $channel' \
  "$LEZ_SOURCE/indexer-config/indexer_config.json" \
  >"$RUNTIME/config/indexer_config.json"

# The sequencer pays for its inscriptions from its own genesis notes (above).
funding_key="$sequencer_note_owner"
# The sequencer makes a LEZ block every second (block_create_timeout, upstream
# 10 s) and retries a pending one after 2 s (upstream 7 s). Bedrock includes one
# inscription about every 2 s at this pace, so a transaction is finalized in the
# indexer ~5 s after it is sent (~9 s with upstream's timing).
# Bedrock funds each inscription at the storage price of the tip it is funded
# against, plus this tip (priority_fee, upstream 200). It re-prices storage at
# every epoch turn: by up to an eighth, and by at least one unit. An inscription
# in flight across a turn that raised the price no longer pays enough. Bedrock
# drops it (InsufficientBalance), and every later inscription with it, because
# each names the dropped one as its parent (InvalidParent). That is the finality
# freeze, logos-blockchain#3643. The tip has to absorb one price step:
# storage gas is about the inscription's bytes + 370, so 10^7 covers a 9 KB LEZ
# block until the price passes ~8000 (a week of drift at +45 an hour), and a
# 2 KB one for a month. It costs ~10^7 per LEZ block, ~1.7 x 10^10 an hour.
sequencer_priority_fee="${LEZ_SEQUENCER_PRIORITY_FEE:-10000000}"
[[ "$sequencer_priority_fee" =~ ^[0-9]+$ ]] || { echo "LEZ_SEQUENCER_PRIORITY_FEE must be a number" >&2; exit 1; }

jq --arg channel "$channel_id" --arg funding "$funding_key" --argjson priority_fee "$sequencer_priority_fee" \
  --arg maker "$maker_account_id" --argjson maker_amount "$maker_genesis_allocation" \
  --arg taker "$taker_account_id" --argjson taker_amount "$taker_genesis_allocation" \
  --arg munich "$wallet_munich_account_id" --arg basel "$wallet_basel_account_id" \
  --arg zurich "$wallet_zurich_account_id" --arg limmat "$wallet_limmat_account_id" \
  --argjson wallet_maker_amount "$wallet_maker_allocation" \
  --argjson wallet_taker_amount "$wallet_taker_allocation" \
  --arg pulse "$heartbeat_pulse_account_id" --arg echo "$heartbeat_echo_account_id" \
  --argjson heartbeat_amount "$heartbeat_allocation" \
  --argjson lanes "$lane_genesis" \
  '.home = "/var/lib/sequencer_service"
   | .bedrock_config.node_url = "http://bedrock:18080"
   | .bedrock_config.channel_id = $channel
   | .bedrock_config.funding_key = $funding
   | .bedrock_config.priority_fee = $priority_fee
   | .block_create_timeout = "1s"
   | .retry_pending_blocks_timeout = "2s"
   | del(.bedrock_config.backoff)
   | .genesis = [
       {"supply_account": {"account_id": $maker, "balance": $maker_amount}},
       {"supply_account": {"account_id": $taker, "balance": $taker_amount}},
       {"supply_account": {"account_id": $munich, "balance": $wallet_maker_amount}},
       {"supply_account": {"account_id": $basel, "balance": $wallet_maker_amount}},
       {"supply_account": {"account_id": $zurich, "balance": $wallet_taker_amount}},
       {"supply_account": {"account_id": $limmat, "balance": $wallet_taker_amount}}
     ] + (if $heartbeat_amount > 0 then [
       {"supply_account": {"account_id": $pulse, "balance": $heartbeat_amount}},
       {"supply_account": {"account_id": $echo, "balance": $heartbeat_amount}}
     ] else [] end) + $lanes' \
  "$LEZ_SOURCE/sequencer-config/sequencer_config.json" \
  >"$RUNTIME/config/sequencer_config.json"

jq -e --arg channel "$channel_id" '.channel_id == $channel' "$RUNTIME/config/indexer_config.json" >/dev/null
jq -e --arg channel "$channel_id" '.bedrock_config.channel_id == $channel' "$RUNTIME/config/sequencer_config.json" >/dev/null
expected_genesis_accounts=6; (( heartbeat_allocation > 0 )) && expected_genesis_accounts=8
expected_genesis_accounts=$(( expected_genesis_accounts + $(jq length <<<"$lane_genesis") ))
[[ "$(jq -r '.genesis | length' "$RUNTIME/config/sequencer_config.json")" == "$expected_genesis_accounts" ]]
[[ "$(jq -r '[.genesis[].supply_account.account_id] | unique | length' \
  "$RUNTIME/config/sequencer_config.json")" == "$expected_genesis_accounts" ]]

# The sequencer generates its own bedrock signing key under its state dir
# (load_or_create_signing_key); no key file is mounted for the local stack.

# --- Bitcoin regtest --------------------------------------------------------
# NOTE: rewrite files IN PLACE (never rm+recreate): docker bind-mounts pin
# the inode, so replacing the file would leave already-running containers
# reading a stale password forever.
if [[ -s "$RUNTIME/btc/bitcoin.conf" ]] && grep -q '^rpcpassword=' "$RUNTIME/btc/bitcoin.conf"; then
  btc_rpc_password="$(grep -E '^rpcpassword=' "$RUNTIME/btc/bitcoin.conf" | head -1 | cut -d= -f2)"
  echo "reusing existing bitcoin RPC password"
else
  btc_rpc_password="$(openssl rand -hex 16)"
fi
cat >"$RUNTIME/btc/bitcoin.conf" <<EOF
regtest=1
proxy=0
networkactive=0
dnsseed=0
listen=0

txindex=1
txospenderindex=1

[regtest]
rpcuser=lezrpc
rpcpassword=${btc_rpc_password}
rpcbind=0.0.0.0:18443
rpcallowip=0.0.0.0/0
server=1
fallbackfee=0.0002
EOF
chmod 0644 "$RUNTIME/btc/bitcoin.conf" || exit 1
# NOTE: 0644 not 0600 — the distroless bitcoin-core container reads this file
# as uid 65532 while the bind mount preserves host ownership. The RPC password
# inside is local-regtest-only.

# deterministic regtest mining key (well-known test key, local-only)
rm -f "$RUNTIME/secrets/mining.key"
cat >"$RUNTIME/secrets/mining.key" <<'EOF'
0000000000000000000000000000000000000000000000000000000000000001
EOF
chmod 0600 "$RUNTIME/secrets/mining.key" || exit 1
# Bitcoin Core credentials in cookie form for the Nodes and their actors
# (rewritten in place: bind mounts pin the inode).
printf 'lezrpc:%s\n' "$btc_rpc_password" >"$RUNTIME/secrets/btc-rpc-cookie"
chmod 0644 "$RUNTIME/secrets/btc-rpc-cookie"

# Per-role LEZ identities for the Node-owned Bitcoin lifecycle (ADR 0213): the
# Maker settles as maker-munich-01 and the Taker as taker-zurich-01, the two
# funded market wallets, so their balances move on the standing chain (a
# regression lane as its own pair, lane.sh).
# The market root holds the wallet identities and market-bootstrap.env (the
# escrow deployment the Nodes read); market-bootstrap.sh writes it once.
market_root="${LEZ_MARKET_ROOT:-}"
if [[ -z "$market_root" ]]; then
  for candidate in "$DEPLOY_ROOT/../../market" "$DEPLOY_ROOT/../market" "$DEPLOY_ROOT/../../runner-work/market" "$DEPLOY_ROOT/../runner-work/market"; do
    [[ -d "$candidate" ]] && { market_root="$(cd "$candidate" && pwd -P)"; break; }
  done
fi
[[ -n "$market_root" && -d "$market_root" ]] || {
  echo "LEZ_MARKET_ROOT must select the market root (wallet identities, market-bootstrap.env)" >&2
  exit 1
}
# A market root's escrow deployment and manifest belong to one stack's LEZ
# chain, so each stack has a root of its own (its identities may be copies): a
# stack other than the default one marks its root (market_root/stack) and takes
# no root that already carries a deployment or another stack's mark.
market_stack="$(cat "$market_root/stack" 2>/dev/null || true)"
if [[ -n "$market_stack" && "$market_stack" != "$stack_prefix" ]]; then
  echo "the market root $market_root belongs to the stack '$market_stack', not '$stack_prefix' (LEZ_MARKET_ROOT: a root of its own)" >&2
  exit 1
elif [[ -z "$market_stack" && "$stack_prefix" != lez ]]; then
  [[ ! -e "$market_root/bootstrap/deployment.json" && ! -e "$market_root/market-bootstrap.env" ]] || {
    echo "the market root $market_root carries another chain's escrow deployment; the stack '$stack_prefix' needs a root of its own (LEZ_MARKET_ROOT)" >&2
    exit 1
  }
  echo "$stack_prefix" >"$market_root/stack"
fi
# Only public deployment IDs are mounted from the market. Its private root
# is mode 0700 and cannot be traversed by the Nodes' uid on Linux.
if [[ -s "$market_root/market-bootstrap.env" ]]; then
  cat "$market_root/market-bootstrap.env" > "$RUNTIME/market-bootstrap.env"
else
  : > "$RUNTIME/market-bootstrap.env"
fi
chmod 0644 "$RUNTIME/market-bootstrap.env"
role_files "$RUNTIME" maker-munich-01 taker-zurich-01 runtime
mkdir -p "$RUNTIME/sockets"
# Recovery timing profile for the Node-owned Bitcoin lifecycle. `local` keeps
# network-like deadlines; `fast` shortens them so refund and abandonment paths
# can be exercised in minutes (Bitcoin CSV maturity is a block count, mined on
# demand on regtest; the LEZ deadlines are wall-clock). The Maker's second-lock
# cutoff must outlast a Bitcoin confirmation (one block every two minutes), and
# a Node refuses a profile whose refund delays, in blocks, break the order its
# times promise. The first value is the delay when Bitcoin is the first lock,
# the seventh when it is the second; the last two are seconds per block at the
# fastest and slowest pace: the regtest miner's own pace (MINE_INTERVAL below),
# two minutes on `local`; `fast` mines every LEZ_BTC_BLOCK_SECONDS, since every confirmation a
# swap waits for is one miner interval (a scenario that needs a transaction to
# stay unconfirmed stops the miner).
# The sixth value is the LEZ discovery window, in seconds here and in blocks
# in runtime.env: a Maker lock can only appear before the cutoff, and a
# refund asserts its absence only once that whole window is finalized, so the
# window must cover the cutoff at the fastest pace the devnet can make LEZ
# blocks, LEZ_LEZ_BLOCK_SECONDS (the sequencer's block_create_timeout, 1 s:
# the fast 240 s = 240 blocks), and nothing more than needed. Bedrock includes
# one about every 2.2 s, so the fast window's 300 blocks are ~11 minutes of
# wall clock. A faster pace than observed cannot make a window too short: the
# sequencer never makes blocks faster than its timeout, and the window is
# counted at that pace; a slower one only makes it longer in wall clock.
#
# `refund` is the devnet's refund scenarios' profile (regress.sh switches to it
# for them), derived from the LEZ chain's measured step latency rather than
# fixed: LEZ_LEZ_STEP_SECONDS is one LEZ step as the counterpart Node sees it
# (submit, inclusion, finality, the Nodes' polling: the Taker's observer sweeps
# every 3 s, the Maker's actor is requeued every 2 s), LEZ_LEZ_STALL_SECONDS
# the finality stall a step must survive. Nobody claims, so:
#   cutoff  = 2 steps (the Taker's lock, then the Maker's) + 20 s of desk + stall
#   earlier = cutoff + margin       (the least the agreement accepts)
#   second-lock CSV = earlier + margin, in regtest blocks
#   later   = cutoff + second-lock CSV + margin   (the least the order allows)
#   first-lock CSV  = later, in blocks; discovery window = cutoff + 30 s
# The regtest pace, LEZ_BTC_BLOCK_SECONDS (2 s), is the miner's interval and
# both block_seconds bounds on `fast` and `refund`; their CSV delays are the
# fewest blocks of that pace the refund order allows, so every confirmation a
# swap waits for is one short interval. Bitcoin's median time, the clock a
# Bitcoin cutoff is read on, then trails the wall clock by ~6 blocks (12 s),
# and a scenario's `mine(csv + 1)` (~420 blocks on `fast`) runs it ahead by
# about one second per six blocks, after the Maker's lock is in.
timing_profile="${LEZ_TIMING_PROFILE:-$previous_timing_profile}"
[[ -n "${LEZ_TIMING_PROFILE:-}" || -z "$timing_profile" ]] || echo "reusing existing timing profile ${timing_profile}"
timing_profile="${timing_profile:-local}"
lez_block_seconds="${LEZ_LEZ_BLOCK_SECONDS:-${previous_lez_timing[0]:-1}}"
lez_step_seconds="${LEZ_LEZ_STEP_SECONDS:-${previous_lez_timing[1]:-20}}"
lez_stall_seconds="${LEZ_LEZ_STALL_SECONDS:-${previous_lez_timing[2]:-40}}"
btc_block_seconds="${LEZ_BTC_BLOCK_SECONDS:-${previous_lez_timing[3]:-2}}"
for v in "$lez_block_seconds" "$lez_step_seconds" "$lez_stall_seconds" "$btc_block_seconds"; do
  [[ "$v" =~ ^[1-9][0-9]*$ ]] || { echo "LEZ_LEZ_BLOCK_SECONDS, LEZ_LEZ_STEP_SECONDS, LEZ_LEZ_STALL_SECONDS and LEZ_BTC_BLOCK_SECONDS must be positive integers" >&2; exit 1; }
done
csv() { echo $(( ($1 + btc_block_seconds - 1) / btc_block_seconds )); } # csv <seconds>: blocks at the regtest pace
case "$timing_profile" in
  local)  timing=(60 1800 3600 7200 300 3600 33 120 120) ;;
  fast)   timing=("$(csv 840)" 240 420 840 60 300 "$(csv 480)" "$btc_block_seconds" "$btc_block_seconds") ;;
  refund)
    cutoff=$(( 2 * lez_step_seconds + 20 + lez_stall_seconds )); margin=15
    earlier=$(( cutoff + margin ))
    second="$(csv $(( earlier + margin )))"
    later=$(( cutoff + second * btc_block_seconds + margin ))
    timing=("$(csv "$later")" "$cutoff" "$earlier" "$later" "$margin" $(( cutoff + 30 )) "$second" "$btc_block_seconds" "$btc_block_seconds")
    echo "refund timing: cutoff ${cutoff} s, refunds at ${earlier} and ${later} s (LEZ step ${lez_step_seconds} s, stall ${lez_stall_seconds} s)" ;;
  *) echo "LEZ_TIMING_PROFILE must be local, fast or refund" >&2; exit 1 ;;
esac
timing[5]=$(( (timing[5] + lez_block_seconds - 1) / lez_block_seconds ))
# The Maker's actor supervisor observes a live swap's actor 5 s after each
# attempt and retries a failed one after 30 s (the Node's defaults, kept on
# `local`). On the devnet's own profiles a step it waits for (the Taker's lock,
# a confirmation, finality) is seconds away, so it looks again after 2 s and
# retries after 10 s. The attempt deadline stays the Node's 30 s: one
# historical indexer read can take ~16 s (repair-indexer.sh).
# The Taker's observer drives its Node-owned Bitcoin swaps that wait on a chain
# once per sweep and sleeps 15 s between sweeps (the Node's default, kept on
# `local`); on the devnet's profiles it sleeps 3 s, about one regtest block.
# A sweep is sequential, one swap after the other: per swap waiting on a chain
# one status read (the actor's own database) and one observing drive (a few
# Bitcoin Core reads, and the sidecar's sequencer reads while a LEZ leg is
# watched); a finished swap costs only its status read. The pause starts
# after a sweep ends, so a slow indexer read lengthens the sweep and never
# overlaps the next one.
case "$timing_profile" in
  local) maker_actor=("" "") taker_sweep="" ;;
  *)     maker_actor=(2 10) taker_sweep=3 ;;
esac
# Both Nodes report the LEZ chain's progress in their health methods and turn
# degraded (`lez_finality_stalled`, #112) once the finalized block id stands
# still while the sequencer's tip moves; the Nodes' default threshold, 600 s,
# is ten blocks of the public testnet. The devnet makes a block a second, and
# its finality heartbeat already calls a chain stalled after
# LEZ_HEARTBEAT_STALL_SECONDS of that, so the Nodes take the same threshold on
# every profile: one definition of a stall on the devnet.
node_finality_stall="$heartbeat_stall_seconds"
(( node_finality_stall >= 10 && node_finality_stall <= 86400 )) ||
  { echo "LEZ_HEARTBEAT_STALL_SECONDS must be 10..=86400 for the Nodes' stall threshold" >&2; exit 1; }
printf '%s\n' \
  ${stack_lines[@]+"${stack_lines[@]}"} \
  "LEZ_VOLUME_PREFIX=$volume_prefix" \
  "LEZ_HOST_UID=$(id -u)" \
  "LEZ_HOST_GID=$(id -g)" \
  "LEZ_TIMING_PROFILE=$timing_profile" \
  "LEZ_BTC_REFUND_CSV_BLOCKS=${timing[0]}" \
  "LEZ_BTC_SECOND_LOCK_REFUND_CSV_BLOCKS=${timing[6]}" \
  "MINE_INTERVAL=${timing[7]}" \
  "LEZ_BTC_BLOCK_SECONDS_FASTEST=${timing[7]}" \
  "LEZ_BTC_BLOCK_SECONDS_SLOWEST=${timing[8]}" \
  "LEZ_BTC_MAKER_LOCK_CUTOFF_SECONDS=${timing[1]}" \
  "LEZ_BTC_EARLIER_REFUND_SECONDS=${timing[2]}" \
  "LEZ_BTC_LATER_REFUND_SECONDS=${timing[3]}" \
  "LEZ_BTC_REFUND_MARGIN_SECONDS=${timing[4]}" \
  "LEZ_LEZ_DISCOVERY_MAX_BLOCKS=${timing[5]}" \
  "LEZ_LEZ_BLOCK_SECONDS=$lez_block_seconds" \
  "LEZ_LEZ_STEP_SECONDS=$lez_step_seconds" \
  "LEZ_LEZ_STALL_SECONDS=$lez_stall_seconds" \
  "LEZ_BTC_BLOCK_SECONDS=$btc_block_seconds" \
  "LEZ_BTC_ACTOR_TRACE=${LEZ_BTC_ACTOR_TRACE:-$previous_actor_trace}" \
  "LEZ_MAKER_ACTOR_REQUEUE_SECONDS=${maker_actor[0]}" \
  "LEZ_MAKER_ACTOR_FAILURE_BACKOFF_SECONDS=${maker_actor[1]}" \
  "LEZ_TAKER_OBSERVER_SWEEP_SECONDS=$taker_sweep" \
  "LEZ_NODE_FINALITY_STALL_SECONDS=$node_finality_stall" \
  "LEZ_GENESIS_TIME=$genesis_time" \
  "LEZ_V02_CHANNEL_ID=$channel_id" \
  "LEZ_V02_MAKER_ACCOUNT_ID=$maker_account_id" \
  "LEZ_V02_TAKER_ACCOUNT_ID=$taker_account_id" \
  "BTC_RPC_USER=lezrpc" \
  "BTC_RPC_PASSWORD=$btc_rpc_password" \
  "LEZ_MARKET_ROOT=$market_root" \
  "LEZ_WALLET_MAKER_ALLOCATION=$wallet_maker_allocation" \
  "LEZ_WALLET_TAKER_ALLOCATION=$wallet_taker_allocation" \
  "LEZ_HEARTBEAT_ALLOCATION=$heartbeat_allocation" \
  "LEZ_HEARTBEAT_SECONDS=$heartbeat_seconds" \
  "LEZ_HEARTBEAT_STALL_SECONDS=$heartbeat_stall_seconds" \
  >"$RUNTIME/runtime.env.tmp"
chmod 0600 "$RUNTIME/runtime.env.tmp"
# Replaced in one step: every harness script (and a running regression lane)
# sources it, and must never find it missing or half written.
mv -f "$RUNTIME/runtime.env.tmp" "$RUNTIME/runtime.env"

# Configs contain public chain settings and are mounted as individual files.
chmod 0644 "$RUNTIME/config/"*.json "$RUNTIME/config/"*.yaml

echo "runtime generated at $RUNTIME"
