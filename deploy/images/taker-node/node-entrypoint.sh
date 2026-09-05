#!/usr/bin/env bash
# node-entrypoint.sh — starts one role Node (maker|taker); the Node spawns one
# LEZ sidecar per swap itself.
# and, once the market bootstrap has recorded the escrow deployment, the
# Node-owned Bitcoin lifecycle configuration (ADR 0213). Everything the Node
# needs is rendered here from mounted, read-only inputs so the state volume
# holds owner-private copies. Chains are selected purely by these inputs.
set -euo pipefail

role="${1:?maker|taker}"
state="/var/lib/lez/$role"
run="/run/lez/$role"
btc_state="$state/btc"
identity_src="/run-lez-identity"
manifest="/run-market/market-bootstrap.env"
cookie_src="/run-config/btc-rpc-cookie"
sidecar_port_base="${LEZ_SIDECAR_PORT_BASE:?}"
sequencer_url="${LEZ_SEQUENCER_URL:-http://sequencer:3040}"
indexer_url="${LEZ_INDEXER_URL:-http://indexer:8779}"
channel_id="${LEZ_V02_CHANNEL_ID:?}"
auth_transfer_program_id="${LEZ_AUTH_TRANSFER_PROGRAM_ID:-dcbbfebcd59399961ed9973b8307dc475fd4c5ca5779aacfe7588f7dbc3f4a71}"

umask 077
chmod 0700 "$state" 2>/dev/null || true

# Literal-loopback routes to the chains: the Bitcoin Core adapter, the funding
# wallet client and the LEZ sidecars accept only loopback endpoints, so this
# container forwards 127.0.0.1 ports to the services by name. Each connection
# is forwarded on its own, so a restarted service is reached again at once.
bitcoin_host="${LEZ_BITCOIN_CORE_HOST:-bitcoin-core}"
sequencer_host="${LEZ_SEQUENCER_HOST:-sequencer}"
indexer_host="${LEZ_INDEXER_HOST:-indexer}"
socat TCP-LISTEN:18443,bind=127.0.0.1,fork,reuseaddr "TCP:${bitcoin_host}:18443" &
socat TCP-LISTEN:3040,bind=127.0.0.1,fork,reuseaddr "TCP:${sequencer_host}:3040" &
socat TCP-LISTEN:8779,bind=127.0.0.1,fork,reuseaddr "TCP:${indexer_host}:8779" &
for _ in $(seq 1 60); do
  curl -sf --max-time 3 --user "$(cat "$cookie_src")" -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getblockcount","params":[]}' http://127.0.0.1:18443/ >/dev/null 2>&1 && break
  sleep 1
done
mkdir -p "$btc_state" "$btc_state/swaps"
rm -f "$run/node.sock" "$run/ready" "$run/chat.sock"

# Owner-private copies of the mounted identity and Bitcoin RPC credentials.
install -m 0600 "$identity_src/lez-signer.key" "$btc_state/lez-signer.key"
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
  bitcoin_genesis="$(curl -fsS --max-time 10 --user "$(cat "$btc_state/btc-rpc-cookie")" \
    --header 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getblockhash","params":[0]}' http://127.0.0.1:18443/ | jq -er '.result')"
  actor_program="/usr/local/bin/lez-btc-$role-actor"
  actor_sha="$(sha256sum "$actor_program" | cut -d' ' -f1)"
  wallet_json='null'
  [[ -n "${LEZ_BTC_WALLET:-}" ]] && wallet_json="\"${LEZ_BTC_WALLET}\""
  jq -n --arg swaps "$btc_state/swaps" --arg cookie "$btc_state/btc-rpc-cookie" \
    --argjson wallet "$wallet_json" --arg btc_genesis "$bitcoin_genesis" \
    --arg channel "$channel_id" --arg genesis "$lez_genesis_hash" --arg program "$escrow_program_id" \
    --arg transfer "$auth_transfer_program_id" --arg sidecar_program /usr/local/bin/lez-v02-bridge-poc \
    --arg sequencer "$sequencer_url" --arg indexer "$indexer_url" --argjson port_base "$sidecar_port_base" \
    --arg signer "$btc_state/lez-signer.key" \
    --arg actor "$actor_program" --arg actor_sha "$actor_sha" \
    --argjson csv "${LEZ_BTC_REFUND_CSV_BLOCKS:-144}" \
    --argjson cutoff "${LEZ_BTC_MAKER_LOCK_CUTOFF_SECONDS:-1800}" \
    --argjson earlier "${LEZ_BTC_EARLIER_REFUND_SECONDS:-3600}" \
    --argjson later "${LEZ_BTC_LATER_REFUND_SECONDS:-7200}" \
    --argjson margin "${LEZ_BTC_REFUND_MARGIN_SECONDS:-300}" '
    {schema_version:1, swaps_root:$swaps,
     bitcoin:{network:"regtest", endpoint:"http://127.0.0.1:18443/", cookie_file:$cookie, wallet:$wallet,
              genesis_block_hash:$btc_genesis, required_confirmations:1, refund_csv_blocks:$csv, claim_fee_sat:1000},
     lez:{channel_id:$channel, genesis_block_hash:$genesis, escrow_program_id:$program,
          authenticated_transfer_program_id:$transfer, sidecar_program:$sidecar_program,
          sequencer_url:$sequencer, indexer_url:$indexer, sidecar_port_base:$port_base, sidecar_port_count:400,
          signer_key_file:$signer,
          request_timeout_millis:120000, discovery_max_blocks:2048},
     recovery:{maker_second_lock_cutoff_seconds:$cutoff, earlier_refund_latest_seconds:$earlier,
               later_refund_earliest_seconds:$later, required_margin_seconds:$margin},
     actor:{program:$actor, program_sha256:$actor_sha}}' >"$btc_state/btc-role.json"
  chmod 0600 "$btc_state/btc-role.json"
  echo "$role Bitcoin lifecycle configured (escrow $escrow_program_id)"
else
  echo "$role: market bootstrap manifest absent; starting without the Bitcoin lifecycle"
fi

case "$role" in
  maker)
    if [[ ! -s "$state/delivery-signing.key" ]]; then
      od -An -N32 -tx1 /dev/urandom | tr -d ' \n' >"$state/delivery-signing.key"
      chmod 0600 "$state/delivery-signing.key"
    fi
    if [[ ! -s "$state/btc-chat-signing.key" ]]; then
      od -An -N32 -tx1 /dev/urandom | tr -d ' \n' >"$state/btc-chat-signing.key"
      chmod 0600 "$state/btc-chat-signing.key"
    fi
    identity="$(/usr/local/bin/lez-maker-cli delivery-identity --signing-key-file "$state/delivery-signing.key" \
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
    else
      # Without the lifecycle the Chat socket still needs a pair authority: a
      # static role root is the legacy shape; keep Chat off until bootstrap.
      args=(--socket "$run/node.sock" --database "$state/maker.sqlite3" --ready-file "$run/ready"
        --delivery-directory /delivery --delivery-signing-key-file "$state/delivery-signing.key"
        --actor-supervisor)
    fi
    exec /usr/local/bin/lez-maker-node "${args[@]}"
    ;;
  taker)
    for _ in $(seq 1 60); do
      [[ -s /delivery/maker-delivery-identity.pub ]] && break
      sleep 1
    done
    maker_identity="$(tr -d ' \n' </delivery/maker-delivery-identity.pub)"
    [[ "$maker_identity" =~ ^(02|03)[0-9a-f]{64}$ ]] || { echo "maker public identity unavailable" >&2; exit 1; }
    [[ -s "$state/registry.sqlite3" ]] || /usr/local/bin/lez-taker-registry-init --database "$state/registry.sqlite3"
    if [[ "$btc_lifecycle_ready" == 1 ]]; then
      jq -n --arg identity "$maker_identity" --arg registry "$state/registry.sqlite3" --arg role_config "$btc_state/btc-role.json" '
        {schema_version:1,
         delivery_sources:[{directory:"/delivery", maker_public_key:$identity, source_id:"local-maker"}],
         chat_socket:"/run/lez/maker/chat.sock", maximum_offers:16,
         initiation:{execute_prepared:true, registry_database:$registry, btc_role_config:$role_config}}' >"$state/role.json"
    else
      jq -n --arg identity "$maker_identity" '
        {schema_version:1,
         delivery_sources:[{directory:"/delivery", maker_public_key:$identity, source_id:"local-maker"}],
         maximum_offers:16}' >"$state/role.json"
    fi
    chmod 0600 "$state/role.json"
    exec /usr/local/bin/lez-taker-node --socket "$run/node.sock" --ready-file "$run/ready" --role-config "$state/role.json"
    ;;
  *) echo "unknown role $role" >&2; exit 2 ;;
esac
