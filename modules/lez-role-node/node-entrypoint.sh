#!/usr/bin/env bash
# node-entrypoint.sh — starts one role Node (maker|taker); the Node spawns one
# LEZ sidecar per swap itself.
# and, once the market bootstrap has recorded the escrow deployment, the
# Node-owned Bitcoin lifecycle configuration (ADR 0213). Everything the Node
# needs is rendered here from mounted, read-only inputs so the state volume
# holds owner-private copies. Chains are selected purely by these inputs.
#
# This launcher travels in the role's payload package (lez_maker_node,
# lez_taker_node; modules/lez-maker-node, deploy/scripts/package-role-node.sh)
# beside the executables it runs, which it finds next to itself. Basecamp's
# lez_swap_node starts it with the role as its argument and owns the Node's
# life: the Node is the process group's leader (this script execs it), and
# the helpers started here (loopback forwards, ZEC sidecars) are in its group,
# which lez_swap_node ends when the Node goes; on Linux they are also
# signalled when the Node goes (setpriv --pdeathsig), so none outlives it.
#
# It runs in the role's container image on Linux and natively on macOS, under
# any bash from 3.2 (macOS's /bin/bash) on. Its inputs and the Node's files
# are at the container's mount points on Linux and under
# ~/Library/Application Support/LEZ on macOS; each can be named by its own
# variable (below).
set -euo pipefail

# `probe-bitcoin <cookie file>`: the Nodes' route-health probe for Bitcoin
# (route-health.json, written below). The Node runs it with no environment;
# it succeeds while Core answers over this container's forward.
if [[ "${1:-}" == probe-bitcoin ]]; then
  exec curl -sf --max-time 4 --user "$(cat "${2:?cookie file}")" -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getblockcount","params":[]}' -o /dev/null http://127.0.0.1:18443/
fi
# `probe-monero <monerod login>`: the same for Monero; it succeeds while
# monerod answers over this container's forward, with the Digest login the
# Monero adapter uses. The role's wallet-rpc is not asked: it serves one
# request at a time and the Node's own tools hold it for seconds (each
# open_wallet 2-4 s, refreshes, a transfer and its mining), so a probe there
# read a busy wallet as down and the Maker withdrew its Monero offers for good.
if [[ "${1:-}" == probe-monero ]]; then
  curl -sf --max-time 2 --digest --user "$(cat "${2:?monerod login}")" -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"get_info"}' http://127.0.0.1:18081/json_rpc |
    jq -e '.result.status == "OK"' >/dev/null || exit 1
  exit 0
fi

role="${1:?maker|taker}"
bin="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Group- or other-writable directories (issue 151). The Maker's actor
# supervisor refuses to run an actor from a directory others can write
# (swap-store maker_actor_process.rs validate_program_parent), and every LEZ
# sidecar refuses a state directory with a symlink or a group- or
# other-writable, non-sticky ancestor, or one owned by another user
# (lez-v0_2-sidecar durable_reservation.rs validate_trusted_parent_chain). Both
# checks stay. A package manager running under umask 002 (Ubuntu's default)
# leaves this package's own directories group-writable, so the launcher removes
# group and other write from what it owns of its package; a directory it does
# not own is reported, with the path and the fix, before the Node starts.
owner_and_mode() { # owner_and_mode <path>: "<uid> <octal mode>", the path itself (no symlink followed)
  stat -c '%u %a' -- "$1" 2>/dev/null || stat -f '%u %Lp' -- "$1"
}
unsafe_dir() { # unsafe_dir <dir>: why the sidecar's ancestor rule refuses <dir> and the fix, or nothing
  local uid mode
  if [[ -L "$1" ]]; then
    echo "is a symbolic link. Fix: name the directory it points to ($(cd "$1" 2>/dev/null && pwd -P)) instead"; return
  fi
  [[ -d "$1" ]] || { echo "is not a directory"; return; }
  read -r uid mode <<<"$(owner_and_mode "$1")"
  if [[ "$uid" != 0 && "$uid" != "$(id -u)" ]]; then
    echo "is owned by another user (uid $uid). Fix: keep the Node's files under a directory you own"; return
  fi
  # Sticky bit (BSD %Lp leaves it out of the mode; [[ -k ]] reads it on both).
  if (( 8#$mode & 8#022 )) && [[ ! -k "$1" ]]; then
    echo "is writable by its group or by others (mode $mode). Fix: chmod go-w '$1'"
  fi
}
refuse_unsafe_ancestors() { # refuse_unsafe_ancestors <dir> <what>: every existing directory from <dir> up to /
  local dir="$1" why
  while :; do
    if [[ -e "$dir" || -L "$dir" ]]; then
      why="$(unsafe_dir "$dir")"
      if [[ -n "$why" ]]; then
        echo "$role: $dir $why. The Node's LEZ sidecars keep $2 below it, and they refuse a state directory with an ancestor that is a symbolic link, writable by its group or others, or owned by another user, so no swap could start. Another place for the Node's files: LEZ_NODE_HOME, or LEZ_${role_upper}_NODE_STATE_DIR and LEZ_${role_upper}_ZEC_SIDECAR_DIR." >&2
        exit 78
      fi
    fi
    [[ "$dir" == / ]] && return 0
    dir="$(dirname "$dir")"
  done
}
role_upper="$(tr '[:lower:]' '[:upper:]' <<<"$role")"
for owned in "$bin/.." "$bin" "$bin"/*; do
  if [[ -O "$owned" && ! -L "$owned" ]] && (( 8#$(owner_and_mode "$owned" | cut -d' ' -f2) & 8#022 )); then
    chmod go-w "$owned" 2>/dev/null || true
  fi
done
# Each setting below may be given for one role only: LEZ_MAKER_<NAME> or
# LEZ_TAKER_<NAME> (LEZ_MAKER_SIDECAR_PORT_BASE for LEZ_SIDECAR_PORT_BASE)
# takes precedence over LEZ_<NAME> for that role. One host can then start
# both roles' Nodes from one environment (one Basecamp with both desks), where
# the names both roles share would otherwise collide; a role's container sets
# the shared names for its one role.
role_prefix="LEZ_$(tr '[:lower:]' '[:upper:]' <<<"$role")_"
for name in SIDECAR_PORT_BASE WALLET_SIGNER_SOCKET BTC_WALLET BTC_CLAIM_DESTINATION NODE_LOG_FILE \
    NODE_STATE_DIR NODE_RUN_ROOT ZEC_STATE_DIR ZEC_SIDECAR_DIR NODE_IDENTITY_DIR MARKET_MANIFEST BTC_RPC_COOKIE_FILE DELIVERY_DIR \
    SEQUENCER_URL INDEXER_URL LEZ_SEQUENCER_ROUTE BITCOIN_CORE_HOST SEQUENCER_HOST INDEXER_HOST ZEBRA_HOST \
    MONEROD_HOST XMR_WALLET_HOST \
    V02_CHANNEL_ID AUTH_TRANSFER_PROGRAM_ID BTC_NETWORK LEZ_NETWORK \
    BTC_REFUND_CSV_BLOCKS BTC_SECOND_LOCK_REFUND_CSV_BLOCKS BTC_BLOCK_SECONDS_FASTEST BTC_BLOCK_SECONDS_SLOWEST \
    BTC_MAKER_LOCK_CUTOFF_SECONDS BTC_EARLIER_REFUND_SECONDS BTC_LATER_REFUND_SECONDS BTC_REFUND_MARGIN_SECONDS \
    BTC_LOCK_FEE_CONFIRMATION_TARGET BTC_LOCK_FEE_FALLBACK_SAT_PER_VB BTC_LOCK_FEE_MAX_SAT_PER_VB \
    BTC_LOCK_FEE_MAX_PERCENT LEZ_DISCOVERY_MAX_BLOCKS ZEC_CLAIM_KEY_ID NODE_FINALITY_STALL_SECONDS; do
  specific="$role_prefix$name"
  if [[ -n "${!specific+set}" ]]; then export "LEZ_$name=${!specific}"; fi
done
# A helper started here ends with the Node (its parent once this script execs
# it): setpriv (util-linux) asks the kernel for that, where it is installed.
# Elsewhere (macOS) the helper is ended with the Node's process group.
with_node=()
command -v setpriv >/dev/null && with_node=(setpriv --pdeathsig TERM)
# Run only in the background (`node_helper ... &`): it execs, so the helper
# is this script's child (the Node's, once it execs) and not a subshell's.
# ("${with_node[@]}" on an empty array is an unbound variable to bash < 4.4.)
node_helper() { exec ${with_node[@]+"${with_node[@]}"} "$@"; }

# Where things are. Linux: the role container's mounts and volumes. macOS: one
# directory per installation (LEZ_NODE_HOME), both roles side by side in it,
# the chains reached on loopback directly. Every path may be set on its own.
if [[ "$(uname -s)" == Darwin ]]; then
  home="${LEZ_NODE_HOME:-$HOME/Library/Application Support/LEZ}"
  default_state="$home/$role"
  default_run_root="$home/run"
  # One corridor root for both roles, as zec-catalog-fixture.sh --native-home
  # writes it; each role's ZEC sidecars keep their state apart.
  default_zec_state="$home/zec"
  default_zec_sidecar="$home/zec/sidecar-$role"
  # ... and the role's take-time Zcash files (zec-role.json, the Taker's
  # zcash.key) theirs; the Taker's Zcash key is in its identity directory.
  default_zec_role="$home/zec/role-$role"
  zcash_key_in_identity=zcash.key
  monero_logins_in_identity=.
  default_identity="$home/identity/$role"
  default_manifest="$home/market-bootstrap.env"
  default_cookie="$home/btc-rpc-cookie"
  default_delivery="$home/delivery"
  default_chain_host=127.0.0.1
else
  default_state="/var/lib/lez/$role"
  default_run_root=/run/lez
  default_zec_state=/var/lib/lez/zec
  default_zec_sidecar=
  default_zec_role=
  zcash_key_in_identity="$role/zcash.key"
  monero_logins_in_identity="$role"
  default_identity=/run-lez-identity
  default_manifest=/run-market/market-bootstrap.env
  default_cookie=/run-config/btc-rpc-cookie
  default_delivery=/delivery
  default_chain_host=
fi
# The Node's state (0700), its sockets and ready file, the Maker's sockets the
# Taker reaches (its Chat socket), the prepared ZEC corridor.
state="${LEZ_NODE_STATE_DIR:-$default_state}"
run_root="${LEZ_NODE_RUN_ROOT:-$default_run_root}"
run="$run_root/$role"
btc_state="$state/btc"
# Unix socket paths are limited (104 bytes on macOS, 108 on Linux).
(( ${#run} + 10 < 104 )) || { echo "$role: the socket directory $run is too long for a Unix socket path; set LEZ_NODE_RUN_ROOT" >&2; exit 2; }

# The Node's own lines, also in a file of their own when LEZ_NODE_LOG_FILE names
# one. Inside Basecamp the Node shares the container's output with every module
# the desk hosts, and Logos Delivery's lines land in the middle of the Node's;
# a reader that needs whole Node lines (the devnet's tests) reads this file.
# The file starts again once it has passed 16 MB.
node_log_file() {
  local file="${LEZ_NODE_LOG_FILE:-}"
  [[ -n "$file" ]] || return 0
  if [[ -f "$file" ]] && (( $(wc -c <"$file") > 16777216 )); then : >"$file"; fi
  exec > >(tee -a "$file") 2> >(tee -a "$file" >&2)
}
# The role's identity (identity.json; lez-signer.key without a signer socket;
# the Maker's zcash.key), the market bootstrap manifest, Bitcoin Core's RPC
# cookie, and the Delivery directory the Maker publishes into and the Taker
# reads: the container's read-only mounts, the macOS layout, or where a
# systemd deployment keeps them (packaging/systemd/lez-<role>-node.env.example,
# which names the identity LEZ_IDENTITY_DIR).
identity_src="${LEZ_NODE_IDENTITY_DIR:-${LEZ_IDENTITY_DIR:-$default_identity}}"
manifest="${LEZ_MARKET_MANIFEST:-$default_manifest}"
cookie_src="${LEZ_BTC_RPC_COOKIE_FILE:-$default_cookie}"
delivery="${LEZ_DELIVERY_DIR:-$default_delivery}"
sidecar_port_base="${LEZ_SIDECAR_PORT_BASE:?}"
sequencer_url="${LEZ_SEQUENCER_URL:-http://${default_chain_host:-sequencer}:3040}"
# How the Node and its sidecars reach the LEZ sequencer (LezSequencerRoute,
# crates/btc-role-lifecycle/src/config.rs): "local", literal-loopback HTTP (the
# devnet's forward below, or an operator's proxy), or "official_public", the
# official testnet origin itself over HTTPS (LEZ_SEQUENCER_URL
# https://testnet.lez.logos.co/), no proxy. The indexer stays loopback either
# way: the official origin serves none.
sequencer_route="${LEZ_LEZ_SEQUENCER_ROUTE:-local}"
case "$sequencer_route" in
  local) sidecar_route_args=(--node-profile local) ;;
  official_public) sidecar_route_args=(--node-profile official_public --indexer-profile local) ;;
  *) echo "$role: LEZ_LEZ_SEQUENCER_ROUTE must be local or official_public, not $sequencer_route" >&2; exit 2 ;;
esac
indexer_url="${LEZ_INDEXER_URL:-http://${default_chain_host:-indexer}:8779}"
channel_id="${LEZ_V02_CHANNEL_ID:?}"
auth_transfer_program_id="${LEZ_AUTH_TRANSFER_PROGRAM_ID:-fe96c4228babbe8bc578e3e25b884cacb07f8c86541f27ed676789875eef875a}"

umask 077
mkdir -p "$state" "$run"
chmod 0700 "$state" 2>/dev/null || true

sha256_of() {
  if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

# Literal-loopback routes to the chains: the Bitcoin Core adapter, the funding
# wallet client and the LEZ sidecars accept only loopback endpoints, so this
# container forwards 127.0.0.1 ports to the services by name. Each connection
# is forwarded on its own, so a restarted service is reached again at once.
# A service that is already on this host's loopback (macOS: the chains'
# containers publish these ports on 127.0.0.1) is reached directly.
# LEZ_CHAIN_FORWARDS=0 leaves them all out where the chains already answer on
# loopback (a systemd deployment, where both roles share one network).
chain_forwards="${LEZ_CHAIN_FORWARDS:-1}"
forward() {
  case "$2" in
    127.0.0.1|localhost) return 0 ;;
  esac
  node_helper socat "TCP-LISTEN:$1,bind=127.0.0.1,fork,reuseaddr" "TCP:$2:$1" &
}
bitcoin_host="${LEZ_BITCOIN_CORE_HOST:-${default_chain_host:-bitcoin-core}}"
sequencer_host="${LEZ_SEQUENCER_HOST:-${default_chain_host:-sequencer}}"
indexer_host="${LEZ_INDEXER_HOST:-${default_chain_host:-indexer}}"
if [[ "$chain_forwards" == 1 ]]; then
  forward 18443 "$bitcoin_host"
  # (On the official_public route the sequencer is reached directly: no forward.)
  if [[ "$sequencer_route" == local ]]; then
    forward 3040 "$sequencer_host"
  fi
  forward 8779 "$indexer_host"
  # The ZEC actors reach Zebra the same way (deterministic_local route).
  zebra_host="${LEZ_ZEBRA_HOST:-${default_chain_host:-zebra}}"
  forward 18232 "$zebra_host"
fi
# Monero, only where the role has the RPC logins beside its identity
# (gen-config.sh, LEZ_MONERO=1): monerod and the role's own wallet-rpc, which
# the Monero adapter likewise reaches on literal loopback only. A systemd
# deployment names the directory of its two login credentials instead
# (LEZ_MONERO_LOGIN_DIR); empty credentials leave Monero off.
monero_logins="${LEZ_MONERO_LOGIN_DIR:-$identity_src/$monero_logins_in_identity}"
monero_ready=0
[[ -s "$monero_logins/monerod.login" && -s "$monero_logins/monero-wallet-rpc.login" ]] && monero_ready=1
if [[ "$chain_forwards" == 1 && "$monero_ready" == 1 ]]; then
  forward 18081 "${LEZ_MONEROD_HOST:-${default_chain_host:-monerod}}"
  forward 18083 "${LEZ_XMR_WALLET_HOST:-${default_chain_host:-$role-xmr-wallet}}"
fi
for _ in $(seq 1 60); do
  curl -sf --max-time 3 --user "$(cat "$cookie_src")" -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getblockcount","params":[]}' http://127.0.0.1:18443/ >/dev/null 2>&1 && break
  sleep 1
done
mkdir -p "$btc_state" "$btc_state/swaps"
refuse_unsafe_ancestors "$btc_state/swaps" "each Bitcoin swap's state"
if [[ "$role" == maker ]]; then
  why="$(unsafe_dir "$bin")"
  if [[ -n "$why" ]]; then
    echo "$role: the package's program directory $bin $why. The Maker's actor supervisor refuses to run an actor from it (actor_deployment_invalid)." >&2
    exit 78
  fi
fi
rm -f "$run/node.sock" "$run/ready" "$run/chat.sock"

# How the two roles find and talk to each other (LEZ_TRANSPORT). "local", the
# default: the Taker reads the Maker's offers and Delivery identity from a
# shared Delivery directory (/delivery) and calls the Maker's Chat socket
# directly (/run/lez/maker/chat.sock), both mounted from the Maker's side.
# "logos": nothing is shared; offers and the Chat rounds travel through the
# Logos Delivery and Chat modules of the two Basecamps (the desks' Logos Chat
# bridge, apps/basecamp/common/logos_chat_bridge.cpp). Each role then runs its
# Chat gateway (lez-<role>-chat-gateway) on the control socket the desk's
# bridge reaches at LEZ_LOGOS_CHAT_GATEWAY_SOCKET: the Maker's hands the
# Taker's requests to its Node's Chat socket, the Taker's serves the Node a
# proxy socket in place of the Maker's, and the Taker learns the Maker's
# Delivery identity from LEZ_MAKER_DELIVERY_IDENTITY. The gateway is started
# again a second after it exits (its sessions are app-lifetime: the bridge
# binds again and the Maker's next 10 s rebroadcast refills the offer index)
# and ends with the Node's process group.
transport="${LEZ_TRANSPORT:-local}"
gateway_socket="${LEZ_LOGOS_CHAT_GATEWAY_SOCKET:-$run/logos-chat-control.sock}"
case "$transport" in
  local) ;;
  logos) [[ "$gateway_socket" == "$run"/* ]] || { echo "$role: LEZ_LOGOS_CHAT_GATEWAY_SOCKET must be in $run" >&2; exit 2; } ;;
  *) echo "$role: LEZ_TRANSPORT must be local or logos, not $transport" >&2; exit 2 ;;
esac
start_chat_gateway() { # start_chat_gateway <gateway endpoint arguments...>
  rm -f "$gateway_socket" "$run/logos-chat-proxy.sock"
  node_helper bash -c 'while :; do
      rm -f "$1" "$2"; "${@:3}"
      echo "'"$role"': Chat gateway exited ($?); starting it again" >&2; sleep 1
    done' gateway "$gateway_socket" "$run/logos-chat-proxy.sock" \
    "$bin/lez-$role-chat-gateway" endpoint --control-socket "$gateway_socket" "$@" </dev/null &
  echo "$role: Logos transport, Chat gateway on $gateway_socket"
}

# The role's LEZ signer: the LEZ wallet as the Logos lez_core module
# (lez-wallet-signer, its socket at LEZ_WALLET_SIGNER_SOCKET) holds the key and
# the Node never sees it; without the socket the Node keeps a copy of the key
# file for its sidecars, as before, or (LEZ_SIGNER_KEY_FILE, a systemd
# deployment) reads the key where it is named: its credential, never copied.
lez_wallet_socket="${LEZ_WALLET_SIGNER_SOCKET:-}"
signer_key="${LEZ_SIGNER_KEY_FILE:-$btc_state/lez-signer.key}"
# Owner-private copies of the mounted identity and Bitcoin RPC credentials.
if [[ -z "$lez_wallet_socket" ]]; then
  [[ -n "${LEZ_SIGNER_KEY_FILE:-}" ]] || install -m 0600 "$identity_src/lez-signer.key" "$btc_state/lez-signer.key"
else
  rm -f "$btc_state/lez-signer.key"
  for _ in $(seq 1 120); do [[ -S "$lez_wallet_socket" ]] && break; sleep 1; done
  [[ -S "$lez_wallet_socket" ]] || { echo "$role: the LEZ wallet signer socket $lez_wallet_socket is not up" >&2; exit 1; }
fi
install -m 0600 "$identity_src/identity.json" "$btc_state/identity.json"
install -m 0600 "$cookie_src" "$btc_state/btc-rpc-cookie"
# ... and of the Monero RPC logins, which the Monero probe below reads.
monero_state="$state/monero-rpc"
rm -rf "$monero_state"
if [[ "$monero_ready" == 1 ]]; then
  mkdir -p "$monero_state"
  install -m 0600 "$monero_logins/monerod.login" "$monero_logins/monero-wallet-rpc.login" "$monero_state/"
fi
signer_account="$(jq -er '.account_id_hex' "$btc_state/identity.json")"

btc_lifecycle_ready=0
if [[ -s "$manifest" ]]; then
  escrow_program_id="$(sed -n 's/^M3_POC_LEZ_ESCROW_PROGRAM_ID=//p' "$manifest" | head -1)"
  lez_genesis_hash="$(sed -n 's/^M3_POC_LEZ_GENESIS_BLOCK_HASH=//p' "$manifest" | head -1)"
  if [[ "$escrow_program_id" =~ ^[0-9a-f]{64}$ && "$lez_genesis_hash" =~ ^[0-9a-f]{64}$ ]]; then
    btc_lifecycle_ready=1
  fi
fi

if [[ "$btc_lifecycle_ready" == 1 ]]; then
  jq -n --arg role "$role" --arg chain "$channel_id" --arg genesis "$lez_genesis_hash" \
    --arg program "$escrow_program_id" --arg signer "$signer_account" '
    {sidecar_role:$role,compatibility:"lee_v0_2_0",chain_id:$chain,channel_id:$chain,
     genesis_block_hash:$genesis,escrow_program_id:$program,signer_account_id:$signer}' \
    >"$btc_state/runtime.json"
  chmod 0600 "$btc_state/runtime.json"
  # Bitcoin's genesis as Core reports it, which the Node refuses unless it is
  # the named network's (btc-role-lifecycle config.rs, ensure_networks_agree).
  # It is kept, so a Node restarted while Core is down still starts: the
  # Bitcoin adapter checks Core's genesis again before every Bitcoin step
  # (btc-core-adapter ensure_ready), and only the Bitcoin pair waits for Core.
  if bitcoin_genesis="$(curl -fsS --max-time 10 --user "$(cat "$btc_state/btc-rpc-cookie")" \
    --header 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getblockhash","params":[0]}' http://127.0.0.1:18443/ | jq -er '.result')"; then
    printf '%s\n' "$bitcoin_genesis" >"$btc_state/bitcoin-genesis"
  elif [[ -s "$btc_state/bitcoin-genesis" ]]; then
    bitcoin_genesis="$(cat "$btc_state/bitcoin-genesis")"
    echo "$role: Bitcoin Core does not answer; starting with the genesis it reported before, the Bitcoin pair waits for Core" >&2
  else
    echo "$role: Bitcoin Core does not answer and never has to this Node; its first start needs Core" >&2
    exit 1
  fi
  actor_program="$bin/lez-btc-$role-actor"
  actor_sha="$(sha256_of "$actor_program")"
  wallet_json='null'
  [[ -n "${LEZ_BTC_WALLET:-}" ]] && wallet_json="\"${LEZ_BTC_WALLET}\""
  # "wallet" pays this role's claims into its own Core wallet, so the desk's
  # balance shows them; the address is kept so a restart reuses it.
  claim_destination="${LEZ_BTC_CLAIM_DESTINATION:-}"
  if [[ "$claim_destination" == wallet ]]; then
    if [[ ! -s "$btc_state/claim-destination" ]]; then
      curl -fsS --max-time 10 --user "$(cat "$btc_state/btc-rpc-cookie")" \
        --header 'content-type: application/json' \
        --data '{"jsonrpc":"2.0","id":1,"method":"getnewaddress","params":["","bech32m"]}' \
        "http://127.0.0.1:18443/wallet/${LEZ_BTC_WALLET:?claim destination needs a wallet}" \
        | jq -er '.result' >"$btc_state/claim-destination"
    fi
    claim_destination="$(cat "$btc_state/claim-destination")"
  fi
  claim_destination_json='null'
  if [[ -n "$claim_destination" ]]; then
    claim_destination_json="\"$claim_destination\""
  fi
  jq -n --arg swaps "$btc_state/swaps" --arg cookie "$btc_state/btc-rpc-cookie" \
    --argjson wallet "$wallet_json" --argjson claim_destination "$claim_destination_json" \
    --arg btc_genesis "$bitcoin_genesis" \
    --arg channel "$channel_id" --arg genesis "$lez_genesis_hash" --arg program "$escrow_program_id" \
    --arg transfer "$auth_transfer_program_id" --arg sidecar_program "$bin/lez-v02-bridge-poc" \
    --arg sequencer "$sequencer_url" --arg indexer "$indexer_url" --argjson port_base "$sidecar_port_base" \
    --arg route "$sequencer_route" \
    --arg signer "$signer_key" --arg wallet_socket "$lez_wallet_socket" --arg signer_account "$signer_account" \
    --arg actor "$actor_program" --arg actor_sha "$actor_sha" \
    --arg btc_network "${LEZ_BTC_NETWORK:-regtest}" \
    --arg lez_network "${LEZ_LEZ_NETWORK:-devnet}" \
    --argjson csv "${LEZ_BTC_REFUND_CSV_BLOCKS:-60}" \
    --argjson csv_second "${LEZ_BTC_SECOND_LOCK_REFUND_CSV_BLOCKS:-33}" \
    --argjson block_fastest "${LEZ_BTC_BLOCK_SECONDS_FASTEST:-120}" \
    --argjson block_slowest "${LEZ_BTC_BLOCK_SECONDS_SLOWEST:-120}" \
    --argjson cutoff "${LEZ_BTC_MAKER_LOCK_CUTOFF_SECONDS:-1800}" \
    --argjson earlier "${LEZ_BTC_EARLIER_REFUND_SECONDS:-3600}" \
    --argjson later "${LEZ_BTC_LATER_REFUND_SECONDS:-7200}" \
    --argjson margin "${LEZ_BTC_REFUND_MARGIN_SECONDS:-300}" \
    --argjson fee_target "${LEZ_BTC_LOCK_FEE_CONFIRMATION_TARGET:-6}" \
    --argjson fee_fallback "${LEZ_BTC_LOCK_FEE_FALLBACK_SAT_PER_VB:-20}" \
    --argjson fee_max_rate "${LEZ_BTC_LOCK_FEE_MAX_SAT_PER_VB:-25}" \
    --argjson fee_max_percent "${LEZ_BTC_LOCK_FEE_MAX_PERCENT:-5}" \
    --argjson discovery "${LEZ_LEZ_DISCOVERY_MAX_BLOCKS:-2048}" '
    {schema_version:1, swaps_root:$swaps,
     bitcoin:{network:$btc_network, endpoint:"http://127.0.0.1:18443/", cookie_file:$cookie, wallet:$wallet,
              claim_destination_address:$claim_destination,
              genesis_block_hash:$btc_genesis, required_confirmations:1, refund_csv_blocks:$csv,
              second_lock_refund_csv_blocks:$csv_second,
              block_seconds:{fastest:$block_fastest, slowest:$block_slowest}, claim_fee_sat:1000,
              lock_fee:{confirmation_target:$fee_target, fallback_sat_per_vb:$fee_fallback,
                        max_sat_per_vb:$fee_max_rate, max_percent_of_value:$fee_max_percent}},
     lez:({network:$lez_network, channel_id:$channel, genesis_block_hash:$genesis, escrow_program_id:$program,
          authenticated_transfer_program_id:$transfer, sidecar_program:$sidecar_program,
          sequencer_url:$sequencer, indexer_url:$indexer, sidecar_port_base:$port_base, sidecar_port_count:400,
          signer_key_file:(if $wallet_socket == "" then $signer else null end),
          wallet_signer:(if $wallet_socket == "" then null else {socket:$wallet_socket, account_id:($signer_account | ascii_downcase)} end),
          request_timeout_millis:120000, discovery_max_blocks:$discovery}
          + (if $route == "local" then {} else {sequencer_route:$route} end)),
     recovery:{maker_second_lock_cutoff_seconds:$cutoff, earlier_refund_latest_seconds:$earlier,
               later_refund_earliest_seconds:$later, required_margin_seconds:$margin},
     actor:{program:$actor, program_sha256:$actor_sha}}' >"$btc_state/btc-role.json"
  chmod 0600 "$btc_state/btc-role.json"
  echo "$role Bitcoin lifecycle configured (escrow $escrow_program_id)"
else
  echo "$role: market bootstrap manifest absent; starting without the Bitcoin lifecycle"
fi

# ---- Zcash corridor (optional) ----------------------------------------------
# tests/fixtures/zec-catalog-fixture.sh fills /var/lib/lez/zec (LEZ_ZEC_STATE_DIR) from the live chains with one
# prepared ZEC swap under corridor/ (the provisioner's output root, which must
# be new and is the prefix of every path inside its files): <role>/actor-config.json,
# lez-runtime.json, lez-signer.key, sidecar.capability and the claim keys of
# this role only, plus prepared.json (the Taker's catalog) beside it. The ZEC actors expect a running role-local LEZ sidecar
# at the config's loopback bridge.endpoint and neither Node launches one for
# ZEC, so it is started here with the recipe the BTC lifecycle uses for its
# per-swap sidecars (crates/btc-role-lifecycle/src/sidecar.rs). Material
# provisioned on another chain is ignored rather than loaded: a catalog whose
# envelope no longer authenticates would keep the Taker from starting at all.
zec_state="${LEZ_ZEC_STATE_DIR:-$default_zec_state}"
# The ZEC sidecars' state (one directory per corridor run) and their log
# (<dir>.log): in the role's own ZEC volume on Linux.
zec_sidecar_root="${LEZ_ZEC_SIDECAR_DIR:-${default_zec_sidecar:-$zec_state/sidecar}}"
zec_role="$zec_state/corridor/$role"
zec_config="$zec_role/actor-config.json"
zec_ready=0
# Every corridor root the provisioner left (corridor/, and corridor-b/ for the
# concurrent fixture) gets its own sidecar on the loopback port its config
# names; the Maker takes every corridor's config as an actor template. The
# Node is ZEC-ready when the first corridor's sidecar answers.
zec_configs=()
for zec_candidate in "$zec_state"/corridor/"$role"/actor-config.json "$zec_state"/corridor-*/"$role"/actor-config.json; do
  [[ -s "$zec_candidate" && "$btc_lifecycle_ready" == 1 ]] || continue
  chmod 0700 "$zec_state" 2>/dev/null || true
  zec_chain="$(jq -r '.bridge.runtime | "\(.channel_id) \(.genesis_block_hash) \(.escrow_program_id)"' "$zec_candidate")"
  if [[ "$zec_chain" != "$channel_id $lez_genesis_hash $escrow_program_id" ]]; then
    echo "$role: ZEC corridor ${zec_candidate%/$role/actor-config.json} was provisioned for another chain; ignored (re-run tests/fixtures/zec-catalog-fixture.sh)"
    continue
  fi
  zec_candidate_role="${zec_candidate%/actor-config.json}"
  zec_run_id="$(jq -er '.run_id' "$zec_candidate")"
  zec_endpoint="$(jq -er '.bridge.endpoint' "$zec_candidate")"
  zec_listen="${zec_endpoint#http://}"
  zec_listen="${zec_listen%%/*}"
  [[ "$zec_listen" =~ ^127\.0\.0\.1:[0-9]+$ ]] || { echo "$role: ZEC bridge endpoint $zec_endpoint is not loopback" >&2; exit 1; }
  zec_sidecar_state="$zec_sidecar_root/$zec_run_id"
  mkdir -p "$zec_sidecar_root" "$zec_sidecar_state"
  chmod 0700 "$zec_sidecar_root" "$zec_sidecar_state"
  refuse_unsafe_ancestors "$zec_sidecar_root" "each ZEC corridor's sidecar state"
  # The ZEC sidecar signs through the same LEZ wallet module when it is up; its
  # account is the one the prepared runtime names.
  if [[ -n "$lez_wallet_socket" ]]; then
    zec_signer_args=(--wallet-signer-socket "$lez_wallet_socket" --signer-account-id "$(jq -er '.signer_account_id' "$zec_candidate_role/lez-runtime.json")")
  else
    zec_signer_args=(--private-key-file "$zec_candidate_role/lez-signer.key")
  fi
  # A sidecar that exits before it listens (its start-up reads can race the
  # chain, or the chain can be briefly unavailable) is started again, at most
  # three times; one that runs but does not listen within 90 s is left alone.
  zec_candidate_ready=0
  for zec_start in 1 2 3; do
    node_helper "$bin/lez-v02-bridge-poc" --listen-address "$zec_listen" "${sidecar_route_args[@]}" \
      --sequencer-url "$sequencer_url" --indexer-url "$indexer_url" --run-id "$zec_run_id" \
      --runtime-file "$zec_candidate_role/lez-runtime.json" --capability-file "$zec_candidate_role/sidecar.capability" \
      "${zec_signer_args[@]}" --state-directory "$zec_sidecar_state" \
      --authenticated-transfer-program-id "$auth_transfer_program_id" \
      </dev/null >>"$zec_sidecar_root.log" 2>&1 &
    zec_sidecar_pid=$!
    for _ in $(seq 1 90); do
      if (exec 3<>"/dev/tcp/127.0.0.1/${zec_listen##*:}") 2>/dev/null; then zec_candidate_ready=1; break; fi
      kill -0 "$zec_sidecar_pid" 2>/dev/null || break
      sleep 1
    done
    [[ "$zec_candidate_ready" == 1 ]] && break
    kill -0 "$zec_sidecar_pid" 2>/dev/null && break
    echo "$role ZEC corridor $zec_run_id: sidecar exited before listening (start $zec_start of 3, see $zec_sidecar_root.log)" >&2
    sleep 2
  done
  if [[ "$zec_candidate_ready" == 1 ]]; then
    echo "$role ZEC corridor $zec_run_id: sidecar listening on $zec_listen"
    zec_configs+=("$zec_candidate")
    [[ "$zec_candidate" == "$zec_config" ]] && zec_ready=1
  else
    echo "$role ZEC corridor $zec_run_id: sidecar did not answer on $zec_listen; ZEC stays off for it (see $zec_sidecar_root.log)" >&2
  fi
done
if [[ "$zec_ready" == 1 ]]; then
  zec_run_id="$(jq -er '.run_id' "$zec_config")"
fi
# The Zcash role a Node takes swaps under at take time (zec_reserve_v1,
# crates/zec-reference-actor/src/reserve.rs): the devnet profile and the Zebra
# forward above; the Taker's own Zcash key (the Maker's is its Delivery key).
# (The role's own directory: the ZEC volume on Linux; on macOS, where both
# roles share the corridor root, a directory of the role's beside it.)
zec_role_root="${default_zec_role:-$zec_state}"
mkdir -p "$zec_state" "$zec_role_root"
chmod 0700 "$zec_state" "$zec_role_root"
zec_role_config="$zec_role_root/zec-role.json"
zec_key_field='{}'
# The Taker's Zcash key: the identity's file, or (a systemd deployment) its credential.
zec_key_source="${LEZ_ZCASH_KEY_SOURCE:-$identity_src/$zcash_key_in_identity}"
if [[ "$role" == taker && -s "$zec_key_source" ]]; then
  # One role-level copy as 32 raw bytes, which every take-time swap names in
  # place and binds by inode: rewritten only when the key itself changes.
  if (( $(wc -c <"$zec_key_source") == 32 )); then
    install -m 0600 "$zec_key_source" "$zec_role_root/.zcash.key.new"
  else
    zec_key_hex="$(tr -d ' \r\n' <"$zec_key_source")"
    [[ "$zec_key_hex" =~ ^[0-9a-fA-F]{64}$ ]] || { echo "$role: $zec_key_source is not a Zcash key" >&2; exit 1; }
    # shellcheck disable=SC2059 # the format is the key's own \xHH escapes
    printf "$(sed 's/../\\x&/g' <<<"$zec_key_hex")" >"$zec_role_root/.zcash.key.new"
  fi
  if cmp -s "$zec_role_root/.zcash.key.new" "$zec_role_root/zcash.key"; then
    rm -f "$zec_role_root/.zcash.key.new"
  else
    mv -f "$zec_role_root/.zcash.key.new" "$zec_role_root/zcash.key"
  fi
  zec_key_field="$(jq -n --arg key "$zec_role_root/zcash.key" '{zcash_key_file:$key}')"
fi
jq -n --arg profile "${LEZ_ZEC_PROFILE:-deterministic-local-devnet-v5}" --argjson key "$zec_key_field" '
  {schema_version:1, profile:$profile, zebra_endpoint:"http://127.0.0.1:18232"} + $key' >"$zec_role_config"
chmod 0600 "$zec_role_config"

# The Monero role a Node takes swaps under at take time (xmr_reserve_v1,
# crates/xmr-reference-actor/src/reserve.rs), only where the role has its
# Monero logins: monerod and the role's wallet-rpc over the forwards above,
# the role's own wallet (monero-lane.py creates it without a password), the
# recovery windows of gen-config.sh (both roles take the same), the Stage-A
# planner and, for the Maker, its six effect tools, all beside the Node.
xmr_role_config="$state/xmr-role.json"
rm -f "$xmr_role_config"
if [[ "$monero_ready" == 1 ]]; then
  xmr_tools='{}'
  if [[ "$role" == maker ]]; then
    xmr_tools="$(jq -n --arg bin "$bin" '{monero_fund:"\($bin)/xmr-reference-monero-fund",
      lez_claim:"\($bin)/xmr-reference-tag15", finalized_classifier:"\($bin)/xmr-reference-tag15-finalized",
      monero_refund:"\($bin)/xmr-reference-monero-refund", monero_verify:"\($bin)/xmr-reference-monero-verify",
      lez_punish:"\($bin)/xmr-reference-tag17"}')"
  fi
  # A package without the Monero programs (a build without pair-xmr) cannot take the role.
  xmr_missing=()
  for xmr_program in "$bin/lez-v02-xmr-stage-a-plan" $(jq -r '.[]' <<<"$xmr_tools") \
      $([[ "$role" != maker ]] || echo "$bin/lez-xmr-maker-actor"); do
    [[ -x "$xmr_program" ]] || xmr_missing+=("${xmr_program##*/}")
  done
  if (( ${#xmr_missing[@]} > 0 )); then
    echo "$role: the Monero role needs ${xmr_missing[*]} in $bin: install the $role from a build with pair-xmr and the Monero Stage-A planner, or remove its Monero logins" >&2
    exit 1
  fi
  jq -n --arg role "$role" --arg monerod_login "$monero_state/monerod.login" \
    --arg wallet_login "$monero_state/monero-wallet-rpc.login" --arg planner "$bin/lez-v02-xmr-stage-a-plan" \
    --argjson cutoff "${LEZ_XMR_MAKER_FUNDING_CUTOFF_SECONDS:?the Monero role needs LEZ_XMR_MAKER_FUNDING_CUTOFF_SECONDS}" \
    --argjson refund "${LEZ_XMR_REFUND_SECONDS:?the Monero role needs LEZ_XMR_REFUND_SECONDS}" \
    --argjson punish "${LEZ_XMR_PUNISH_SECONDS:?the Monero role needs LEZ_XMR_PUNISH_SECONDS}" \
    --argjson tools "$xmr_tools" '
    {schema_version:1, profile:"accelerated-regtest",
     monerod_endpoint:"http://127.0.0.1:18081/", monerod_login_file:$monerod_login,
     wallet_rpc_endpoint:"http://127.0.0.1:18083/", wallet_rpc_login_file:$wallet_login, wallet_name:$role,
     funding_cutoff_seconds:$cutoff, refund_seconds:$refund, punish_seconds:$punish,
     stage_a_planner:$planner, effect_tools:$tools}' >"$xmr_role_config"
  chmod 0600 "$xmr_role_config"
fi

# Route health (packaging/systemd/README.md): a Bitcoin or Zcash route reads
# unavailable while its chain does not answer over this container's
# forward; the Maker then withdraws its offers there and refuses to quote or
# publish on it, the Taker refuses a new take on it, and both report
# themselves degraded. Bitcoin is asked with the role's cookie
# (probe-bitcoin, above), Zcash with the package's health check; both are the
# package's own files beside the Node. Monero, where the role has its logins,
# is asked for its one route (TakerSellsLez) with probe-monero, above.
launcher="$bin/${BASH_SOURCE[0]##*/}"
jq -n --arg launcher "$launcher" --arg launcher_sha "$(sha256_of "$launcher")" \
  --arg cookie "$btc_state/btc-rpc-cookie" --arg check "$bin/lez-runtime-healthcheck" \
  --arg check_sha "$(sha256_of "$bin/lez-runtime-healthcheck")" \
  --argjson monero "$monero_ready" --arg monerod_login "$monero_state/monerod.login" '
  [{pair:"Bitcoin", program:$launcher, program_sha256:$launcher_sha, args:["probe-bitcoin", $cookie]},
   {pair:"Zcash", program:$check, program_sha256:$check_sha, args:["tcp", "127.0.0.1:18232", "getblockchaininfo", "[]"]}]
  | {schema_version:1, commands:([.[] as $probe | ("TakerSellsForeign", "TakerSellsLez") as $direction
      | {route:{pair:$probe.pair, direction:$direction}} + ($probe | del(.pair)) + {timeout_milliseconds:5000}]
      + if $monero == 1 then [{route:{pair:"Monero", direction:"TakerSellsLez"}, program:$launcher,
          program_sha256:$launcher_sha, args:["probe-monero", $monerod_login], timeout_milliseconds:5000}]
        else [] end)}' \
  >"$state/route-health.json"

case "$role" in
  maker)
    # A systemd deployment names its Delivery key, a credential, instead.
    delivery_key="${LEZ_DELIVERY_SIGNING_KEY_FILE:-$state/delivery-signing.key}"
    if [[ ! -s "$delivery_key" ]]; then
      od -An -N32 -tx1 /dev/urandom | tr -d ' \n' >"$delivery_key"
      chmod 0600 "$delivery_key"
    fi
    # The Maker's Delivery signing key is its minted Zcash key
    # (runtime/lez/maker/zcash.key, gen-config.sh): the ZEC corridor binds the
    # Maker's Delivery identity, its Chat signing key and its Zcash key
    # together (crates/maker-node/src/zec_chat.rs,
    # crates/taker-node/src/zec_taker_accept.rs), and zebra mines to that same
    # key. Replacing it orphans offers signed by the previous identity;
    # tests/fixtures/zec-catalog-fixture.sh removes their Delivery files when it changes.
    pinned_delivery_key="$identity_src/zcash.key"
    if [[ -r "$pinned_delivery_key" ]] && ! cmp -s "$pinned_delivery_key" "$delivery_key"; then
      install -m 0600 "$pinned_delivery_key" "$delivery_key"
      echo "maker Delivery signing key is the minted Zcash key ($pinned_delivery_key)"
    fi
    if [[ ! -s "$state/btc-chat-signing.key" ]]; then
      od -An -N32 -tx1 /dev/urandom | tr -d ' \n' >"$state/btc-chat-signing.key"
      chmod 0600 "$state/btc-chat-signing.key"
    fi
    identity="$("$bin/lez-maker-cli" delivery-identity --signing-key-file "$delivery_key" \
      | tr -d ' \n' | grep -o '02[0-9a-f]\{64\}\|03[0-9a-f]\{64\}')"
    [[ -n "$identity" ]] || { echo "could not publish maker identity" >&2; exit 1; }
    # Best-effort (R2): a Maker restarted while Delivery is cut starts all the
    # same; the identity, unchanged while its key is, is written again at the
    # next start.
    if ! { mkdir -p "$delivery" &&
           printf '%s\n' "$identity" >"$delivery/.maker-delivery-identity.pub.tmp" &&
           chmod 0444 "$delivery/.maker-delivery-identity.pub.tmp" &&
           mv -f "$delivery/.maker-delivery-identity.pub.tmp" "$delivery/maker-delivery-identity.pub"; } 2>/dev/null; then
      echo "maker: Delivery unavailable; its identity is published at the next start" >&2
    fi
    args=(--socket "$run/node.sock" --database "$state/maker.sqlite3" --ready-file "$run/ready"
      --delivery-directory "$delivery" --delivery-signing-key-file "$delivery_key"
      --chat-socket "$run/chat.sock" --btc-maker-signing-key-file "$state/btc-chat-signing.key"
      --actor-supervisor)
    if [[ "$btc_lifecycle_ready" == 1 ]]; then
      args+=(--btc-role-config "$btc_state/btc-role.json")
      # Corridor templates when zec-catalog-fixture.sh left some; the Zcash role
      # (take-time agreements, zec_reserve_v1) in any case.
      if [[ "$zec_ready" == 1 || -s "$zec_role_config" ]]; then
        # The authority group of crates/maker-node/src/bin/lez-maker-node.rs:
        # the Node's claim-recovery key, the Zcash funder's preimage (the
        # Maker in TakerSellsLez), the provisioned Maker config as the one
        # actor template. The claim-recovery key is the Node's own and lives
        # as long as its state volume: the ZEC recovery store re-validates
        # every stored claim journal with the daemon's key on each start
        # (swap-store zec_recovery.rs validate_existing_claim_envelopes; a
        # different key id is KeyIdMismatch), so a per-corridor key would keep
        # the Maker from starting after its first ZEC swap.
        mkdir -p "$zec_state/actors"
        chmod 0700 "$zec_state/actors"
        # A systemd deployment names its claim-recovery key, a credential, instead.
        zec_claim_key="${LEZ_ZEC_CLAIM_KEY_FILE:-$state/zec-claim-recovery.key}"
        if [[ ! -s "$zec_claim_key" ]]; then
          head -c 32 /dev/urandom >"$zec_claim_key"
          chmod 0600 "$zec_claim_key"
        fi
        zec_actor_program="$bin/lez-zec-maker-actor"
        args+=(--maker-claim-key-id "${LEZ_ZEC_CLAIM_KEY_ID:-devnet-zec-claim-v1}"
          --maker-claim-key-file "$zec_claim_key"
          --zec-role-config "$zec_role_config"
          --zec-maker-actor-root "$zec_state/actors"
          --zec-actor-program "$zec_actor_program"
          --zec-actor-program-sha256 "$(sha256_of "$zec_actor_program")")
        for zec_template in "${zec_configs[@]}"; do
          args+=(--zec-source-maker-config "$zec_template")
        done
        # No Node-wide claim preimage: a completion takes the secret from the
        # agreement's own pinned template (each corridor entry carries one).
        echo "maker ZEC authority configured (corridor ${zec_run_id:-none}, ${#zec_configs[@]} template(s))"
      fi
      # The Monero role (xmr_reserve_v1) and its actor. A Monero attempt runs
      # a tool that sends and then mines its ten confirmations, or restores a
      # shared wallet: longer than the Node's default 30 s attempt.
      if [[ -s "$xmr_role_config" ]]; then
        mkdir -p "$state/xmr-actors"
        chmod 0700 "$state/xmr-actors"
        args+=(--xmr-role-config "$xmr_role_config"
          --xmr-maker-actor-root "$state/xmr-actors"
          --xmr-actor-program "$bin/lez-xmr-maker-actor"
          --xmr-actor-program-sha256 "$(sha256_of "$bin/lez-xmr-maker-actor")"
          --actor-attempt-timeout-milliseconds 120000)
        echo "maker XMR authority configured"
      fi
    else
      # Without the lifecycle the Chat socket still needs a pair authority: a
      # static role root is the legacy shape; keep Chat off until bootstrap.
      args=(--socket "$run/node.sock" --database "$state/maker.sqlite3" --ready-file "$run/ready"
        --delivery-directory "$delivery" --delivery-signing-key-file "$delivery_key"
        --actor-supervisor)
    fi
    args+=(--route-health-config "$state/route-health.json")
    # The devnet's timing profiles observe a live swap's actor sooner and retry
    # a failed attempt sooner (gen-config.sh); unset, the Node's own defaults.
    [[ -z "${LEZ_MAKER_ACTOR_REQUEUE_SECONDS:-}" ]] || args+=(--actor-requeue-delay-seconds "$LEZ_MAKER_ACTOR_REQUEUE_SECONDS")
    [[ -z "${LEZ_MAKER_ACTOR_FAILURE_BACKOFF_SECONDS:-}" ]] || args+=(--actor-failure-backoff-seconds "$LEZ_MAKER_ACTOR_FAILURE_BACKOFF_SECONDS")
    # How long LEZ finality may stand still before maker_health says so
    # (gen-config.sh: the devnet heartbeat's threshold); unset, the Node's 600 s.
    [[ -z "${LEZ_NODE_FINALITY_STALL_SECONDS:-}" ]] || args+=(--lez-finality-stall-seconds "$LEZ_NODE_FINALITY_STALL_SECONDS")
    # A Logos price module (deploy/README.md): LEZ_LOGOS_PRICE_MODULE names the
    # module file, LEZ_LOGOS_PRICE_MODULE_SHA256 its pinned sha256; the Node runs
    # it in the package's crash-isolated worker for the pairs configured with
    # the logos-c-api price source. Both or neither; unset, no external source.
    if [[ -n "${LEZ_LOGOS_PRICE_MODULE:-}${LEZ_LOGOS_PRICE_MODULE_SHA256:-}" ]]; then
      args+=(--logos-price-worker "$bin/lez-logos-price-worker"
        --logos-price-module "${LEZ_LOGOS_PRICE_MODULE:?LEZ_LOGOS_PRICE_MODULE_SHA256 needs LEZ_LOGOS_PRICE_MODULE}"
        --logos-price-module-sha256 "${LEZ_LOGOS_PRICE_MODULE_SHA256:?LEZ_LOGOS_PRICE_MODULE needs LEZ_LOGOS_PRICE_MODULE_SHA256}")
    fi
    [[ "$transport" != logos ]] || start_chat_gateway --maker-chat-socket "$run/chat.sock"
    node_log_file
    exec "$bin/lez-maker-node" "${args[@]}"
    ;;
  taker)
    if [[ "$transport" == logos ]]; then
      # No shared directory: the Maker's offers arrive over Logos Delivery
      # through the gateway, and the Node's Delivery source for that Maker is
      # an empty directory of its own.
      maker_identity="$(tr -d ' \n' <<<"${LEZ_MAKER_DELIVERY_IDENTITY:-}")"
      delivery="$state/delivery"
      install -d -m 0700 "$delivery"
      chat_socket="$run/logos-chat-proxy.sock"
      start_chat_gateway --proxy-socket "$chat_socket"
    else
      # The Maker identity this Taker last started with is kept owner-private
      # in its state: a restart while the Delivery directory cannot be entered
      # (cut, R2) starts from it. Whenever Delivery can be read its copy is
      # used, as before, and a change is logged: open swaps keep the identity
      # they were taken under, new takes use Delivery's.
      pinned="$state/maker-delivery-identity.pub"
      for _ in $(seq 1 60); do
        [[ -s "$delivery/maker-delivery-identity.pub" ]] && break
        [[ ! -x "$delivery" && -s "$pinned" ]] && break
        sleep 1
      done
      maker_identity="$({ tr -d ' \n' <"$delivery/maker-delivery-identity.pub"; } 2>/dev/null || true)"
      last="$({ tr -d ' \n' <"$pinned"; } 2>/dev/null || true)"
      if [[ -z "$maker_identity" && ! -x "$delivery" ]]; then
        maker_identity="$last"
        echo "taker: Delivery unavailable; starting with the Maker identity it last used"
      elif [[ "$maker_identity" != "$last" && "$maker_identity" =~ ^(02|03)[0-9a-f]{64}$ ]]; then
        [[ -z "$last" ]] || echo "taker: the Maker's Delivery identity changed from $last to $maker_identity"
        printf '%s\n' "$maker_identity" >"$pinned"
      fi
      chat_socket="$run_root/maker/chat.sock"
    fi
    [[ "$maker_identity" =~ ^(02|03)[0-9a-f]{64}$ ]] || { echo "maker public identity unavailable" >&2; exit 1; }
    [[ -s "$state/registry.sqlite3" ]] || "$bin/lez-taker-registry-init" --database "$state/registry.sqlite3"
    if [[ "$btc_lifecycle_ready" == 1 ]]; then
      # The prepared ZEC catalog (taker_service_config/prepared.rs) only when
      # this corridor's sidecar answers; the key stays absent otherwise so a
      # Node built without pair-zec still accepts the role config.
      zec_catalog='[]'
      if [[ "$zec_ready" == 1 && -s "$zec_state/prepared.json" ]]; then
        # Every entry's signed envelope must carry the Maker identity this Node
        # pins now (prepared.rs authenticates it at load and a mismatch fails
        # the whole start); a catalog built under another identity is dropped.
        if python3 - "$zec_state/prepared.json" "$maker_identity" <<'PY'
import json, sys
entries = json.load(open(sys.argv[1]))
for entry in entries:
    envelope = json.load(open(entry["signed_envelope"]["path"]))
    if bytes(envelope["maker_public_key"]).hex() != sys.argv[2]:
        sys.exit(1)
sys.exit(0 if entries else 1)
PY
        then
          zec_catalog="$(jq -c '.' "$zec_state/prepared.json")"
        else
          echo "taker: prepared ZEC catalog was built under another Maker identity; ignored (re-run tests/fixtures/zec-catalog-fixture.sh)"
        fi
      fi
      # The Zcash role only with the Taker's Zcash key: take-time agreements
      # (zec_reserve_v1) need it, a Node without one takes catalog swaps only.
      zec_role_field='{}'
      if [[ -s "$zec_role_root/zcash.key" ]]; then
        zec_role_field="$(jq -n --arg config "$zec_role_config" '{zec_role_config:$config}')"
      fi
      # ... and the Monero role where the role has its Monero logins.
      xmr_role_field='{}'
      if [[ -s "$xmr_role_config" ]]; then
        xmr_role_field="$(jq -n --arg config "$xmr_role_config" '{xmr_role_config:$config}')"
      fi
      jq -n --arg identity "$maker_identity" --arg registry "$state/registry.sqlite3" --arg role_config "$btc_state/btc-role.json" \
        --arg delivery "$delivery" --arg chat_socket "$chat_socket" --argjson prepared "$zec_catalog" \
        --argjson zec_role "$zec_role_field" --argjson xmr_role "$xmr_role_field" '
        {schema_version:1,
         delivery_sources:[{directory:$delivery, maker_public_key:$identity, source_id:"local-maker"}],
         chat_socket:$chat_socket, maximum_offers:16,
         initiation:({execute_prepared:true, registry_database:$registry, btc_role_config:$role_config}
                     + (if ($prepared|length) > 0 then {prepared_zec:$prepared} else {} end) + $zec_role + $xmr_role)}' >"$state/role.json"
      echo "taker: $(jq length <<<"$zec_catalog") prepared ZEC entries configured"
    else
      jq -n --arg identity "$maker_identity" --arg delivery "$delivery" '
        {schema_version:1,
         delivery_sources:[{directory:$delivery, maker_public_key:$identity, source_id:"local-maker"}],
         maximum_offers:16}' >"$state/role.json"
    fi
    chmod 0600 "$state/role.json"
    args=(--socket "$run/node.sock" --ready-file "$run/ready" --role-config "$state/role.json"
      --route-health-config "$state/route-health.json")
    # The devnet's timing profiles sweep the Bitcoin swaps sooner
    # (gen-config.sh); unset, the Node's own default.
    [[ -z "${LEZ_TAKER_OBSERVER_SWEEP_SECONDS:-}" ]] || args+=(--observer-sweep-seconds "$LEZ_TAKER_OBSERVER_SWEEP_SECONDS")
    # As the Maker: how long LEZ finality may stand still before taker_health says so.
    [[ -z "${LEZ_NODE_FINALITY_STALL_SECONDS:-}" ]] || args+=(--lez-finality-stall-seconds "$LEZ_NODE_FINALITY_STALL_SECONDS")
    node_log_file
    exec "$bin/lez-taker-node" "${args[@]}"
    ;;
  *) echo "unknown role $role" >&2; exit 2 ;;
esac
