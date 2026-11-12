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
# the helpers started here (loopback forwards, ZEC sidecars) are signalled
# when the Node goes (setpriv --pdeathsig), so none outlives it.
set -euo pipefail

# `probe-bitcoin <cookie file>`: the Maker's route-health probe for Bitcoin
# (route-health.json, written below). The Node runs it with no environment;
# it succeeds while Core answers over this container's forward.
if [[ "${1:-}" == probe-bitcoin ]]; then
  exec curl -sf --max-time 4 --user "$(cat "${2:?cookie file}")" -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getblockcount","params":[]}' -o /dev/null http://127.0.0.1:18443/
fi

role="${1:?maker|taker}"
bin="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# A helper started here ends with the Node (its parent once this script execs
# it): setpriv (util-linux) asks the kernel for that, where it is installed.
with_node=()
command -v setpriv >/dev/null && with_node=(setpriv --pdeathsig TERM)
state="/var/lib/lez/$role"
run="/run/lez/$role"
btc_state="$state/btc"

# The Node's own lines, also in a file of their own when LEZ_NODE_LOG_FILE names
# one. Inside Basecamp the Node shares the container's output with every module
# the desk hosts, and Logos Delivery's lines land in the middle of the Node's;
# a reader that needs whole Node lines (the devnet's tests) reads this file.
# The file starts again once it has passed 16 MB.
node_log_file() {
  local file="${LEZ_NODE_LOG_FILE:-}"
  [[ -n "$file" ]] || return 0
  if [[ -f "$file" ]] && (( $(stat -c %s "$file") > 16777216 )); then : >"$file"; fi
  exec > >(tee -a "$file") 2> >(tee -a "$file" >&2)
}
identity_src="/run-lez-identity"
manifest="/run-market/market-bootstrap.env"
cookie_src="/run-config/btc-rpc-cookie"
sidecar_port_base="${LEZ_SIDECAR_PORT_BASE:?}"
sequencer_url="${LEZ_SEQUENCER_URL:-http://sequencer:3040}"
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
indexer_url="${LEZ_INDEXER_URL:-http://indexer:8779}"
channel_id="${LEZ_V02_CHANNEL_ID:?}"
auth_transfer_program_id="${LEZ_AUTH_TRANSFER_PROGRAM_ID:-fe96c4228babbe8bc578e3e25b884cacb07f8c86541f27ed676789875eef875a}"

umask 077
chmod 0700 "$state" 2>/dev/null || true

# Literal-loopback routes to the chains: the Bitcoin Core adapter, the funding
# wallet client and the LEZ sidecars accept only loopback endpoints, so this
# container forwards 127.0.0.1 ports to the services by name. Each connection
# is forwarded on its own, so a restarted service is reached again at once.
bitcoin_host="${LEZ_BITCOIN_CORE_HOST:-bitcoin-core}"
sequencer_host="${LEZ_SEQUENCER_HOST:-sequencer}"
indexer_host="${LEZ_INDEXER_HOST:-indexer}"
"${with_node[@]}" socat TCP-LISTEN:18443,bind=127.0.0.1,fork,reuseaddr "TCP:${bitcoin_host}:18443" &
# (On the official_public route the sequencer is reached directly: no forward.)
if [[ "$sequencer_route" == local ]]; then
  "${with_node[@]}" socat TCP-LISTEN:3040,bind=127.0.0.1,fork,reuseaddr "TCP:${sequencer_host}:3040" &
fi
"${with_node[@]}" socat TCP-LISTEN:8779,bind=127.0.0.1,fork,reuseaddr "TCP:${indexer_host}:8779" &
# The ZEC actors reach Zebra the same way (deterministic_local route).
zebra_host="${LEZ_ZEBRA_HOST:-zebra}"
"${with_node[@]}" socat TCP-LISTEN:18232,bind=127.0.0.1,fork,reuseaddr "TCP:${zebra_host}:18232" &
for _ in $(seq 1 60); do
  curl -sf --max-time 3 --user "$(cat "$cookie_src")" -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getblockcount","params":[]}' http://127.0.0.1:18443/ >/dev/null 2>&1 && break
  sleep 1
done
mkdir -p "$btc_state" "$btc_state/swaps"
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
  "${with_node[@]}" bash -c 'while :; do
      rm -f "$1" "$2"; "${@:3}"
      echo "'"$role"': Chat gateway exited ($?); starting it again" >&2; sleep 1
    done' gateway "$gateway_socket" "$run/logos-chat-proxy.sock" \
    "$bin/lez-$role-chat-gateway" endpoint --control-socket "$gateway_socket" "$@" </dev/null &
  echo "$role: Logos transport, Chat gateway on $gateway_socket"
}

# The role's LEZ signer: the LEZ wallet as the Logos lez_core module
# (lez-wallet-signer, its socket at LEZ_WALLET_SIGNER_SOCKET) holds the key and
# the Node never sees it; without the socket the Node keeps a copy of the key
# file for its sidecars, as before.
lez_wallet_socket="${LEZ_WALLET_SIGNER_SOCKET:-}"
# Owner-private copies of the mounted identity and Bitcoin RPC credentials.
if [[ -z "$lez_wallet_socket" ]]; then
  install -m 0600 "$identity_src/lez-signer.key" "$btc_state/lez-signer.key"
else
  rm -f "$btc_state/lez-signer.key"
  for _ in $(seq 1 120); do [[ -S "$lez_wallet_socket" ]] && break; sleep 1; done
  [[ -S "$lez_wallet_socket" ]] || { echo "$role: the LEZ wallet signer socket $lez_wallet_socket is not up" >&2; exit 1; }
fi
install -m 0600 "$identity_src/identity.json" "$btc_state/identity.json"
install -m 0600 "$cookie_src" "$btc_state/btc-rpc-cookie"
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
  actor_sha="$(sha256sum "$actor_program" | cut -d' ' -f1)"
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
    --arg signer "$btc_state/lez-signer.key" --arg wallet_socket "$lez_wallet_socket" --arg signer_account "$signer_account" \
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
# scripts/zec-provision.sh fills /var/lib/lez/zec from the live chains with one
# prepared ZEC swap under corridor/ (the provisioner's output root, which must
# be new and is the prefix of every path inside its files): <role>/actor-config.json,
# lez-runtime.json, lez-signer.key, sidecar.capability and the claim keys of
# this role only, plus prepared.json (the Taker's catalog) beside it. The ZEC actors expect a running role-local LEZ sidecar
# at the config's loopback bridge.endpoint and neither Node launches one for
# ZEC, so it is started here with the recipe the BTC lifecycle uses for its
# per-swap sidecars (crates/btc-role-lifecycle/src/sidecar.rs). Material
# provisioned on another chain is ignored rather than loaded: a catalog whose
# envelope no longer authenticates would keep the Taker from starting at all.
zec_state="/var/lib/lez/zec"
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
    echo "$role: ZEC corridor ${zec_candidate%/$role/actor-config.json} was provisioned for another chain; ignored (re-run scripts/zec-provision.sh)"
    continue
  fi
  zec_candidate_role="${zec_candidate%/actor-config.json}"
  zec_run_id="$(jq -er '.run_id' "$zec_candidate")"
  zec_endpoint="$(jq -er '.bridge.endpoint' "$zec_candidate")"
  zec_listen="${zec_endpoint#http://}"
  zec_listen="${zec_listen%%/*}"
  [[ "$zec_listen" =~ ^127\.0\.0\.1:[0-9]+$ ]] || { echo "$role: ZEC bridge endpoint $zec_endpoint is not loopback" >&2; exit 1; }
  zec_sidecar_state="$zec_state/sidecar/$zec_run_id"
  mkdir -p "$zec_state/sidecar" "$zec_sidecar_state"
  chmod 0700 "$zec_state/sidecar" "$zec_sidecar_state"
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
    "${with_node[@]}" "$bin/lez-v02-bridge-poc" --listen-address "$zec_listen" "${sidecar_route_args[@]}" \
      --sequencer-url "$sequencer_url" --indexer-url "$indexer_url" --run-id "$zec_run_id" \
      --runtime-file "$zec_candidate_role/lez-runtime.json" --capability-file "$zec_candidate_role/sidecar.capability" \
      "${zec_signer_args[@]}" --state-directory "$zec_sidecar_state" \
      --authenticated-transfer-program-id "$auth_transfer_program_id" \
      </dev/null >>"$zec_state/sidecar.log" 2>&1 &
    zec_sidecar_pid=$!
    for _ in $(seq 1 90); do
      if (exec 3<>"/dev/tcp/127.0.0.1/${zec_listen##*:}") 2>/dev/null; then zec_candidate_ready=1; break; fi
      kill -0 "$zec_sidecar_pid" 2>/dev/null || break
      sleep 1
    done
    [[ "$zec_candidate_ready" == 1 ]] && break
    kill -0 "$zec_sidecar_pid" 2>/dev/null && break
    echo "$role ZEC corridor $zec_run_id: sidecar exited before listening (start $zec_start of 3, see $zec_state/sidecar.log)" >&2
    sleep 2
  done
  if [[ "$zec_candidate_ready" == 1 ]]; then
    echo "$role ZEC corridor $zec_run_id: sidecar listening on $zec_listen"
    zec_configs+=("$zec_candidate")
    [[ "$zec_candidate" == "$zec_config" ]] && zec_ready=1
  else
    echo "$role ZEC corridor $zec_run_id: sidecar did not answer on $zec_listen; ZEC stays off for it (see $zec_state/sidecar.log)" >&2
  fi
done
if [[ "$zec_ready" == 1 ]]; then
  zec_run_id="$(jq -er '.run_id' "$zec_config")"
fi

case "$role" in
  maker)
    if [[ ! -s "$state/delivery-signing.key" ]]; then
      od -An -N32 -tx1 /dev/urandom | tr -d ' \n' >"$state/delivery-signing.key"
      chmod 0600 "$state/delivery-signing.key"
    fi
    # The Maker's Delivery signing key is its minted Zcash key
    # (runtime/lez/maker/zcash.key, gen-config.sh): the ZEC corridor binds the
    # Maker's Delivery identity, its Chat signing key and its Zcash key
    # together (crates/maker-node/src/zec_chat.rs,
    # crates/taker-node/src/zec_taker_accept.rs), and zebra mines to that same
    # key. Replacing it orphans offers signed by the previous identity;
    # scripts/zec-provision.sh removes their Delivery files when it changes.
    pinned_delivery_key="/run-lez-identity/zcash.key"
    if [[ -r "$pinned_delivery_key" ]] && ! cmp -s "$pinned_delivery_key" "$state/delivery-signing.key"; then
      install -m 0600 "$pinned_delivery_key" "$state/delivery-signing.key"
      echo "maker Delivery signing key is the minted Zcash key ($pinned_delivery_key)"
    fi
    if [[ ! -s "$state/btc-chat-signing.key" ]]; then
      od -An -N32 -tx1 /dev/urandom | tr -d ' \n' >"$state/btc-chat-signing.key"
      chmod 0600 "$state/btc-chat-signing.key"
    fi
    identity="$("$bin/lez-maker-cli" delivery-identity --signing-key-file "$state/delivery-signing.key" \
      | tr -d ' \n' | grep -o '02[0-9a-f]\{64\}\|03[0-9a-f]\{64\}')"
    [[ -n "$identity" ]] || { echo "could not publish maker identity" >&2; exit 1; }
    printf '%s\n' "$identity" >/delivery/.maker-delivery-identity.pub.tmp
    chmod 0444 /delivery/.maker-delivery-identity.pub.tmp
    mv -f /delivery/.maker-delivery-identity.pub.tmp /delivery/maker-delivery-identity.pub
    args=(--socket "$run/node.sock" --database "$state/maker.sqlite3" --ready-file "$run/ready"
      --delivery-directory /delivery --delivery-signing-key-file "$state/delivery-signing.key"
      --chat-socket "$run/chat.sock" --btc-maker-signing-key-file "$state/btc-chat-signing.key"
      --actor-supervisor)
    if [[ "$btc_lifecycle_ready" == 1 ]]; then
      args+=(--btc-role-config "$btc_state/btc-role.json")
      if [[ "$zec_ready" == 1 ]]; then
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
        if [[ ! -s "$state/zec-claim-recovery.key" ]]; then
          head -c 32 /dev/urandom >"$state/zec-claim-recovery.key"
          chmod 0600 "$state/zec-claim-recovery.key"
        fi
        zec_actor_program="$bin/lez-zec-maker-actor"
        args+=(--maker-claim-key-id "${LEZ_ZEC_CLAIM_KEY_ID:-devnet-zec-claim-v1}"
          --maker-claim-key-file "$state/zec-claim-recovery.key"
          --zec-maker-actor-root "$zec_state/actors"
          --zec-actor-program "$zec_actor_program"
          --zec-actor-program-sha256 "$(sha256sum "$zec_actor_program" | cut -d' ' -f1)")
        for zec_template in "${zec_configs[@]}"; do
          args+=(--zec-source-maker-config "$zec_template")
        done
        # No Node-wide claim preimage: a completion takes the secret from the
        # agreement's own pinned template (each corridor entry carries one).
        echo "maker ZEC authority configured (corridor $zec_run_id, ${#zec_configs[@]} template(s))"
      fi
    else
      # Without the lifecycle the Chat socket still needs a pair authority: a
      # static role root is the legacy shape; keep Chat off until bootstrap.
      args=(--socket "$run/node.sock" --database "$state/maker.sqlite3" --ready-file "$run/ready"
        --delivery-directory /delivery --delivery-signing-key-file "$state/delivery-signing.key"
        --actor-supervisor)
    fi
    # Route health (packaging/systemd/README.md): a Bitcoin or Zcash route reads
    # unavailable while its chain does not answer over this container's
    # forward; the Maker then withdraws its offers there, refuses to quote or
    # publish on it and reports itself degraded. Bitcoin is asked with the
    # role's cookie (probe-bitcoin, above), Zcash with the package's health
    # check; both are the package's own files beside the Node.
    launcher="$bin/${BASH_SOURCE[0]##*/}"
    jq -n --arg launcher "$launcher" --arg launcher_sha "$(sha256sum "$launcher" | cut -d' ' -f1)" \
      --arg cookie "$btc_state/btc-rpc-cookie" --arg check "$bin/lez-runtime-healthcheck" \
      --arg check_sha "$(sha256sum "$bin/lez-runtime-healthcheck" | cut -d' ' -f1)" '
      [{pair:"Bitcoin", program:$launcher, program_sha256:$launcher_sha, args:["probe-bitcoin", $cookie]},
       {pair:"Zcash", program:$check, program_sha256:$check_sha, args:["tcp", "127.0.0.1:18232", "getblockchaininfo", "[]"]}]
      | {schema_version:1, commands:[.[] as $probe | ("TakerSellsForeign", "TakerSellsLez") as $direction
          | {route:{pair:$probe.pair, direction:$direction}} + ($probe | del(.pair)) + {timeout_milliseconds:5000}]}' \
      >"$state/route-health.json"
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
      for _ in $(seq 1 60); do
        [[ -s /delivery/maker-delivery-identity.pub ]] && break
        sleep 1
      done
      maker_identity="$(tr -d ' \n' </delivery/maker-delivery-identity.pub)"
      delivery=/delivery
      chat_socket=/run/lez/maker/chat.sock
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
          echo "taker: prepared ZEC catalog was built under another Maker identity; ignored (re-run scripts/zec-provision.sh)"
        fi
      fi
      jq -n --arg identity "$maker_identity" --arg registry "$state/registry.sqlite3" --arg role_config "$btc_state/btc-role.json" \
        --arg delivery "$delivery" --arg chat_socket "$chat_socket" --argjson prepared "$zec_catalog" '
        {schema_version:1,
         delivery_sources:[{directory:$delivery, maker_public_key:$identity, source_id:"local-maker"}],
         chat_socket:$chat_socket, maximum_offers:16,
         initiation:({execute_prepared:true, registry_database:$registry, btc_role_config:$role_config}
                     + (if ($prepared|length) > 0 then {prepared_zec:$prepared} else {} end))}' >"$state/role.json"
      echo "taker: $(jq length <<<"$zec_catalog") prepared ZEC entries configured"
    else
      jq -n --arg identity "$maker_identity" --arg delivery "$delivery" '
        {schema_version:1,
         delivery_sources:[{directory:$delivery, maker_public_key:$identity, source_id:"local-maker"}],
         maximum_offers:16}' >"$state/role.json"
    fi
    chmod 0600 "$state/role.json"
    args=(--socket "$run/node.sock" --ready-file "$run/ready" --role-config "$state/role.json")
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
