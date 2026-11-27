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

# `curl_login <login file> <curl arguments...>`: curl with the RPC login
# (user:password) of <login file> as its --user, handed over in a config on
# its standard input (-K -), never on its command line, which any local user
# reads in ps. The login is quoted as curl's config syntax wants it, and is
# one line: a newline in it would start another config line.
curl_login() {
  local login
  login="$(sed -e 's/[\\"]/\\&/g' "$1")"
  shift
  case "$login" in
    *$'\n'*|*$'\r'*) echo "a login file holds one line: user:password" >&2; return 2 ;;
  esac
  printf 'user = "%s"\n' "$login" | curl -K - "$@"
}

# `probe-bitcoin <cookie file>`: the Nodes' route-health probe for Bitcoin
# (route-health.json, written below). The Node runs it with no environment;
# it succeeds while Core answers over this container's forward.
if [[ "${1:-}" == probe-bitcoin ]]; then
  curl_login "${2:?cookie file}" -sf --max-time 4 -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getblockcount","params":[]}' -o /dev/null http://127.0.0.1:18443/
  exit
fi
# `probe-monero <monerod login>`: the same for Monero; it succeeds while
# monerod answers over this container's forward, with the Digest login the
# Monero adapter uses. The role's wallet-rpc is not asked: it serves one
# request at a time and the Node's own tools hold it for seconds (each
# open_wallet 2-4 s, refreshes, a transfer and its mining), so a probe there
# read a busy wallet as down and the Maker withdrew its Monero offers for good.
# No -f: a Stagenet node without a login (a public one) answers the Digest
# exchange so that curl -f fails although the answer is 200; what decides is
# the answer, which must be get_info's JSON with status OK.
if [[ "${1:-}" == probe-monero ]]; then
  curl_login "${2:?monerod login}" -s --max-time 4 --digest -H 'content-type: application/json' \
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
    SEQUENCER_URL INDEXER_URL LEZ_SEQUENCER_ROUTE BITCOIN_CORE_HOST SEQUENCER_HOST INDEXER_HOST ZEBRA_HOST ZEBRA_COOKIE_FILE \
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
  # One Zcash directory for both roles (the Maker's actor roots are in it);
  # each role's own Zcash files (zec-role.json, the Taker's zcash.key) in a
  # directory of the role's beside them. The Taker's Zcash key, when its
  # operator supplies one, is in its identity directory.
  default_zec_state="$home/zec"
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
# Taker reaches (its Chat socket).
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
# How the Node and its sidecars reach the LEZ sequencer (LezSequencerRoute,
# crates/btc-role-lifecycle/src/config.rs): "local", literal-loopback HTTP (the
# devnet's forward below, or an operator's proxy), or "official_public", the
# official testnet origin itself over HTTPS, no proxy. The indexer stays
# loopback either way: the official origin serves none.
# The Node and the sidecars both refuse any other URL for the route
# (crates/btc-role-lifecycle/src/lez_endpoint.rs): a name, a Compose service
# name or localhost included, is not literal loopback, so the defaults are
# the loopback ports the forwards below listen on, on every host.
sequencer_route="${LEZ_LEZ_SEQUENCER_ROUTE:-local}"
case "$sequencer_route" in
  local) default_sequencer_url=http://127.0.0.1:3040 ;;
  official_public) default_sequencer_url=https://testnet.lez.logos.co/ ;;
  *) echo "$role: LEZ_LEZ_SEQUENCER_ROUTE must be local or official_public, not $sequencer_route" >&2; exit 2 ;;
esac
sequencer_url="${LEZ_SEQUENCER_URL:-$default_sequencer_url}"
indexer_url="${LEZ_INDEXER_URL:-http://127.0.0.1:8779}"
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
forward() { # forward <loopback port> <host>[:<port>]: the same port unless named
  # A service already on this loopback port needs no forward; one on another
  # loopback port does.
  case "$2" in
    127.0.0.1|localhost|"127.0.0.1:$1"|"localhost:$1") return 0 ;;
  esac
  local target="$2"
  [[ "$target" == *:* ]] || target="$target:$1"
  node_helper socat "TCP-LISTEN:$1,bind=127.0.0.1,fork,reuseaddr" "TCP:$target" &
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
  curl_login "$cookie_src" -sf --max-time 3 -H 'content-type: application/json' \
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
  if bitcoin_genesis="$(curl_login "$btc_state/btc-rpc-cookie" -fsS --max-time 10 \
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
      curl_login "$btc_state/btc-rpc-cookie" -fsS --max-time 10 \
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

# ---- Zcash ------------------------------------------------------------------
# A Zcash swap is agreed by the two Nodes when the Taker takes the Maker's
# offer (zec_reserve_v1): nothing about a swap is prepared ahead of the take
# and nothing is read here. Each Node writes its own half into the swap's
# directory under its swaps root and starts the swap's LEZ sidecar itself
# (crates/btc-role-lifecycle/src/sidecar.rs).
zec_state="${LEZ_ZEC_STATE_DIR:-$default_zec_state}"
# The devnet sidecar's test hooks (feature test-hooks, built only with
# LEZ_TEST_HOOKS=1): count files in <state>/test-hooks, read by the sidecars
# the Node spawns. A sidecar built without the feature never reads them.
export LEZ_TEST_HOOKS_DIR="$state/test-hooks"
# The Zcash role a Node takes swaps under at take time (zec_reserve_v1,
# crates/zec-reference-actor/src/reserve.rs): the devnet profile and the Zebra
# forward above; the Taker's own Zcash key (the Maker's is its Delivery key).
# (The role's own directory: the ZEC volume on Linux; on macOS, where both
# roles share the Zcash directory, a directory of the role's beside it.)
zec_role_root="${default_zec_role:-$zec_state}"
mkdir -p "$zec_state" "$zec_role_root"
chmod 0700 "$zec_state" "$zec_role_root"
zec_role_config="$zec_role_root/zec-role.json"
zec_key_field='{}'
# The Taker's own Zcash key, one role-level file of 32 raw bytes which every
# take-time swap names in place and binds by inode. Its operator may supply
# it (the identity's file, or a systemd deployment's credential,
# LEZ_ZCASH_KEY_SOURCE); where none is supplied the Node creates its own on
# its first start, owner-only, and keeps it: a key is never created over an
# existing one, and a key this Node created is never replaced by a supplied
# one (it may hold coins). A new key holds no coins: selling ZEC, its owner
# pays its address first (`lez-taker-cli balances --zcash` prints it).
zec_key_source="${LEZ_ZCASH_KEY_SOURCE:-$identity_src/$zcash_key_in_identity}"
if [[ "$role" == taker ]]; then
  zec_own_key="$zec_role_root/zcash.key"
  zec_created_here="$zec_role_root/zcash.key.created-here"
  if [[ -s "$zec_key_source" ]]; then
    # Rewritten only when the supplied key itself changes.
    if (( $(wc -c <"$zec_key_source") == 32 )); then
      install -m 0600 "$zec_key_source" "$zec_role_root/.zcash.key.new"
    else
      zec_key_hex="$(tr -d ' \r\n' <"$zec_key_source")"
      [[ "$zec_key_hex" =~ ^[0-9a-fA-F]{64}$ ]] || { echo "$role: $zec_key_source is not a Zcash key" >&2; exit 1; }
      # shellcheck disable=SC2059 # the format is the key's own \xHH escapes
      printf "$(sed 's/../\\x&/g' <<<"$zec_key_hex")" >"$zec_role_root/.zcash.key.new"
    fi
    if cmp -s "$zec_role_root/.zcash.key.new" "$zec_own_key"; then
      rm -f "$zec_role_root/.zcash.key.new"
    elif [[ -e "$zec_created_here" ]]; then
      rm -f "$zec_role_root/.zcash.key.new"
      echo "taker: this Node created its own Zcash key ($zec_own_key) and $zec_key_source now supplies another one. Nothing was replaced: the Node's key may hold coins. To take the supplied key, move $zec_own_key and $zec_created_here away first; to keep the Node's, stop supplying one" >&2
      exit 1
    else
      mv -f "$zec_role_root/.zcash.key.new" "$zec_own_key"
    fi
  elif [[ -n "${LEZ_ZCASH_KEY_SOURCE:-}" ]]; then
    echo "taker: LEZ_ZCASH_KEY_SOURCE names $zec_key_source, which is absent or empty: no key is created beside a named source"
  elif [[ ! -e "$zec_own_key" ]]; then
    head -c 32 /dev/urandom >"$zec_role_root/.zcash.key.new"
    (( $(wc -c <"$zec_role_root/.zcash.key.new") == 32 )) || { echo "taker: could not read 32 random bytes for its Zcash key" >&2; exit 1; }
    chmod 0600 "$zec_role_root/.zcash.key.new"
    mv -n "$zec_role_root/.zcash.key.new" "$zec_own_key"
    rm -f "$zec_role_root/.zcash.key.new"
    : >"$zec_created_here"
    echo "taker: created its own Zcash key ($zec_own_key). It holds no coins until its address is paid: lez-taker-cli balances --zcash prints the address"
  fi
  if [[ -s "$zec_own_key" ]]; then
    zec_key_field="$(jq -n --arg key "$zec_own_key" '{zcash_key_file:$key}')"
  fi
fi
# The Zebra the role's Zcash actors use (LEZ_ZEC_ZEBRA_ROUTE, the route kinds
# of crates/zec-reference-actor/src/local_poc.rs LocalPocZebraRoute), with the
# profile that goes with its network (LEZ_ZEC_PROFILE):
#   deterministic_local (the default): the Regtest Zebra on loopback. Its
#     JSON-RPC cookie where Zebra writes it (the devnet's zebra service:
#     zebrad_module, LEZ_ZEBRA_COOKIE_FILE): the actors read it at each call,
#     since Zebra writes a new one at every start. Unset: a Zebra without
#     cookie authentication.
#   self_hosted_cookie: the operator's own Zcash Testnet Zebra on loopback,
#     with its .cookie (LEZ_ZEBRA_COOKIE_FILE), under a public-testnet profile.
#   tatum_testnet_x_api_key: Tatum's Testnet Zebrad gateway, the one provider
#     origin the actors accept, with the owner-private key file
#     LEZ_ZEBRA_API_KEY_FILE names (sent as x-api-key).
zec_zebra_route="${LEZ_ZEC_ZEBRA_ROUTE:-deterministic_local}"
zec_zebra_endpoint=http://127.0.0.1:18232
zec_cookie_file="${LEZ_ZEBRA_COOKIE_FILE:-}"
zec_route_field='{}'
case "$zec_zebra_route" in
  deterministic_local)
    if [[ -n "$zec_cookie_file" ]]; then
      [[ "$zec_cookie_file" == /* ]] || { echo "$role: LEZ_ZEBRA_COOKIE_FILE must be an absolute path" >&2; exit 1; }
      zec_route_field="$(jq -n --arg cookie "$zec_cookie_file" '{zebra_route:{kind:"deterministic_local", cookie_file:$cookie}}')"
    fi ;;
  self_hosted_cookie)
    [[ "$zec_cookie_file" == /* ]] || { echo "$role: LEZ_ZEC_ZEBRA_ROUTE=self_hosted_cookie needs LEZ_ZEBRA_COOKIE_FILE, the absolute path of the Testnet Zebra's .cookie" >&2; exit 2; }
    zec_route_field="$(jq -n --arg cookie "$zec_cookie_file" '{zebra_route:{kind:"self_hosted_cookie", cookie_file:$cookie}}')" ;;
  tatum_testnet_x_api_key)
    [[ "${LEZ_ZEBRA_API_KEY_FILE:-}" == /* ]] || { echo "$role: LEZ_ZEC_ZEBRA_ROUTE=tatum_testnet_x_api_key needs LEZ_ZEBRA_API_KEY_FILE, the absolute path of the owner-private key file" >&2; exit 2; }
    zec_zebra_endpoint=https://zcash-testnet-zebrad.gateway.tatum.io
    zec_cookie_file=
    zec_route_field="$(jq -n --arg key "$LEZ_ZEBRA_API_KEY_FILE" '{zebra_route:{kind:"tatum_testnet_x_api_key", api_key_file:$key}}')" ;;
  *) echo "$role: LEZ_ZEC_ZEBRA_ROUTE must be deterministic_local, self_hosted_cookie or tatum_testnet_x_api_key, not '$zec_zebra_route'" >&2; exit 2 ;;
esac
# How many blocks below the mature tip the funder search reads (the Node's
# default when unset; on Testnet the Node bounds it).
zec_scan_field='{}'
if [[ -n "${LEZ_ZEC_FUNDING_SCAN_BLOCKS:-}" ]]; then
  [[ "$LEZ_ZEC_FUNDING_SCAN_BLOCKS" =~ ^[1-9][0-9]{0,5}$ ]] || { echo "$role: LEZ_ZEC_FUNDING_SCAN_BLOCKS must be a positive number of blocks" >&2; exit 2; }
  zec_scan_field="$(jq -n --argjson blocks "$LEZ_ZEC_FUNDING_SCAN_BLOCKS" '{funding_scan_blocks:$blocks}')"
fi
jq -n --arg profile "${LEZ_ZEC_PROFILE:-deterministic-local-devnet-v5}" --argjson key "$zec_key_field" \
  --argjson route "$zec_route_field" --arg endpoint "$zec_zebra_endpoint" --argjson scan "$zec_scan_field" '
  {schema_version:1, profile:$profile, zebra_endpoint:$endpoint} + $route + $key + $scan' >"$zec_role_config"
chmod 0600 "$zec_role_config"

# The Monero role a Node takes swaps under at take time (xmr_reserve_v1,
# crates/xmr-reference-actor/src/reserve.rs), only where the role has its
# Monero logins: monerod and the role's wallet-rpc over the forwards above,
# the role's own wallet (monero-lane.py creates it without a password), the
# recovery windows of gen-config.sh (both roles take the same), the Stage-A
# planner and, for the Maker, its six effect tools, all beside the Node.
# LEZ_XMR_NETWORK picks the Monero network, and with it the profile Stage A
# carries: regtest (the devnet's offline chain, the default) or stagenet
# (public Monero Stagenet: LEZ_MONEROD_HOST and LEZ_XMR_WALLET_HOST name the
# role's own daemon and wallet-rpc as host:port, docs/monero-stagenet-guide.md).
xmr_role_config="$state/xmr-role.json"
rm -f "$xmr_role_config"
case "${LEZ_XMR_NETWORK:-regtest}" in
  regtest) xmr_profile=accelerated-regtest ;;
  stagenet) xmr_profile=public-stagenet ;;
  mainnet|testnet)
    echo "$role: LEZ_XMR_NETWORK=${LEZ_XMR_NETWORK}: Monero mainnet and testnet are refused; regtest or stagenet" >&2
    exit 2 ;;
  *) echo "$role: LEZ_XMR_NETWORK must be regtest or stagenet, not '${LEZ_XMR_NETWORK}'" >&2; exit 2 ;;
esac
if [[ "$monero_ready" == 1 ]]; then
  # On macOS the Maker's Monero effect tools take their inputs as unlinked
  # owner-private copies instead of write-sealed memfds
  # (crates/xmr-reference-actor/src/effect_input_custody.rs): say so.
  if [[ "$role" == maker && "$(uname -s)" == Darwin ]]; then
    echo "maker: macOS: Monero effect inputs are unlinked owner-private copies, not sealed memfds (a process of this user can interfere with them, as it cannot on Linux)" >&2
  fi
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
    --argjson tools "$xmr_tools" --arg profile "$xmr_profile" '
    {schema_version:1, profile:$profile,
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
# (probe-bitcoin, above), Zcash with the package's health check (with Zebra's
# cookie when LEZ_ZEBRA_COOKIE_FILE names one; over the provider route the
# check, which speaks to loopback only, has nothing to ask, so Zcash has no
# probe: health lists no Zcash route and no take is refused on its account);
# both are the package's own files beside the Node. Monero, where the role has its logins,
# is asked for its one route (TakerSellsLez) with probe-monero, above.
launcher="$bin/${BASH_SOURCE[0]##*/}"
jq -n --arg launcher "$launcher" --arg launcher_sha "$(sha256_of "$launcher")" \
  --arg cookie "$btc_state/btc-rpc-cookie" --arg check "$bin/lez-runtime-healthcheck" \
  --arg check_sha "$(sha256_of "$bin/lez-runtime-healthcheck")" \
  --argjson monero "$monero_ready" --arg monerod_login "$monero_state/monerod.login" \
  --arg zec_cookie "$zec_cookie_file" --arg zec_route "$zec_zebra_route" '
  ([{pair:"Bitcoin", program:$launcher, program_sha256:$launcher_sha, args:["probe-bitcoin", $cookie]}]
   + if $zec_route == "tatum_testnet_x_api_key" then [] else
     [{pair:"Zcash", program:$check, program_sha256:$check_sha, args:(["tcp", "127.0.0.1:18232", "getblockchaininfo", "[]"]
        + if $zec_cookie == "" then [] else ["--cookie-file", $zec_cookie] end)}] end)
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
    # The Maker's Delivery signing key is also its Zcash key: a Zcash
    # agreement binds the Maker's Delivery identity, its Chat signing key and
    # its Zcash key together (crates/maker-node/src/zec_chat.rs,
    # crates/taker-node/src/zec_taker_accept.rs), and when the Taker sells LEZ
    # the Maker funds Zcash from it. Its operator may supply it (zcash.key in
    # the identity directory: on the devnet the key gen-config.sh minted,
    # which zebra mines to); where none is supplied the Node creates its own
    # on its first start, owner-only, and keeps it. A key is never created
    # over an existing one, and a key this Node created is never replaced by
    # a supplied one (it may hold coins). Replacing the key orphans offers
    # signed by the previous identity.
    pinned_delivery_key="$identity_src/zcash.key"
    delivery_key_created_here="$delivery_key.created-here"
    if [[ -r "$pinned_delivery_key" ]]; then
      if ! cmp -s "$pinned_delivery_key" "$delivery_key"; then
        if [[ -e "$delivery_key_created_here" ]]; then
          echo "maker: this Node created its own Delivery signing key ($delivery_key), which is also its Zcash key, and $pinned_delivery_key now supplies another one. Nothing was replaced: the Node's key may hold coins. To take the supplied key, move $delivery_key and $delivery_key_created_here away first; to keep the Node's, stop supplying one" >&2
          exit 1
        fi
        install -m 0600 "$pinned_delivery_key" "$delivery_key"
        echo "maker Delivery signing key is the minted Zcash key ($pinned_delivery_key)"
      fi
    elif [[ ! -s "$delivery_key" ]]; then
      od -An -N32 -tx1 /dev/urandom | tr -d ' \n' >"$delivery_key"
      chmod 0600 "$delivery_key"
      : >"$delivery_key_created_here"
      echo "maker: created its own Delivery signing key ($delivery_key), which is also its Zcash key. It holds no coins until its address is paid: lez-maker-cli balances --zcash prints the address"
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
      # The Zcash role: agreements composed at take time (zec_reserve_v1).
      if [[ -s "$zec_role_config" ]]; then
        # The authority group of crates/maker-node/src/bin/lez-maker-node.rs:
        # the Node's claim-recovery key and where it keeps the actor of each
        # swap it agrees. The Maker holds no claim preimage in either
        # direction (the Taker holds the swap secret). The claim-recovery key is the Node's own and lives
        # as long as its state volume: the ZEC recovery store re-validates
        # every stored claim journal with the daemon's key on each start
        # (swap-store zec_recovery.rs validate_existing_claim_envelopes; a
        # different key id is KeyIdMismatch), so a per-swap key would keep
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
        # No claim preimage: the Maker reads the secret from the Taker's
        # revealing claim.
        echo "maker ZEC authority configured (agreements at take time)"
      fi
      # The Monero role (xmr_reserve_v1) and its actor. A Monero attempt runs
      # a tool that sends and then mines its ten confirmations, or restores a
      # shared wallet: longer than the Node's default 30 s attempt. On
      # Stagenet a wallet restored through a public node takes minutes, and
      # the actor's own Monero tools get 4: the attempt gets the 5 the
      # supervisor admits.
      xmr_attempt_ms=120000
      [[ "$xmr_profile" != public-stagenet ]] || xmr_attempt_ms=300000
      if [[ -s "$xmr_role_config" ]]; then
        mkdir -p "$state/xmr-actors"
        chmod 0700 "$state/xmr-actors"
        args+=(--xmr-role-config "$xmr_role_config"
          --xmr-maker-actor-root "$state/xmr-actors"
          --xmr-actor-program "$bin/lez-xmr-maker-actor"
          --xmr-actor-program-sha256 "$(sha256_of "$bin/lez-xmr-maker-actor")"
          --actor-attempt-timeout-milliseconds "$xmr_attempt_ms")
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
    # As the Taker's: the devnet sidecar's test hooks (feature test-hooks).
    export LEZ_TEST_HOOKS_DIR="$state/test-hooks"
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
      # The Zcash role only with the Taker's Zcash key: take-time agreements
      # (zec_reserve_v1) need it, a Node without one takes no Zcash swap.
      zec_role_field='{}'
      if [[ -s "$zec_role_root/zcash.key" ]]; then
        zec_role_field="$(jq -n --arg config "$zec_role_config" '{zec_role_config:$config}')"
      fi
      # ... and the Monero role where the role has its Monero logins.
      xmr_role_field='{}'
      if [[ -s "$xmr_role_config" ]]; then
        # The Node refunds a Monero swap's LEZ by itself from R unless told
        # not to: LEZ_XMR_AUTOMATIC_REFUND=false, or the word false in
        # $state/xmr-automatic-refund (read at each start, so it can be
        # changed without recreating the container).
        xmr_automatic_refund="${LEZ_XMR_AUTOMATIC_REFUND:-true}"
        if [[ -f "$state/xmr-automatic-refund" ]]; then
          xmr_automatic_refund="$(tr -d '[:space:]' <"$state/xmr-automatic-refund")"
        fi
        [[ "$xmr_automatic_refund" == true || "$xmr_automatic_refund" == false ]] \
          || { echo "taker: xmr-automatic-refund must be true or false, not $xmr_automatic_refund" >&2; exit 2; }
        xmr_role_field="$(jq -n --arg config "$xmr_role_config" --argjson automatic "$xmr_automatic_refund" \
          '{xmr_role_config:$config, xmr_automatic_refund:$automatic}')"
      fi
      jq -n --arg identity "$maker_identity" --arg registry "$state/registry.sqlite3" --arg role_config "$btc_state/btc-role.json" \
        --arg delivery "$delivery" --arg chat_socket "$chat_socket" \
        --argjson zec_role "$zec_role_field" --argjson xmr_role "$xmr_role_field" '
        {schema_version:1,
         delivery_sources:[{directory:$delivery, maker_public_key:$identity, source_id:"local-maker"}],
         chat_socket:$chat_socket, maximum_offers:16,
         initiation:({execute_prepared:true, registry_database:$registry, btc_role_config:$role_config}
                     + $zec_role + $xmr_role)}' >"$state/role.json"
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
    # Where a Node built with the devnet's test hooks (feature test-hooks)
    # reads them; a release build ignores it.
    export LEZ_TEST_HOOKS_DIR="$state/test-hooks"
    exec "$bin/lez-taker-node" "${args[@]}"
    ;;
  *) echo "unknown role $role" >&2; exit 2 ;;
esac
