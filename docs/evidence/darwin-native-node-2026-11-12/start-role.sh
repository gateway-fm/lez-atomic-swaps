#!/bin/bash
# start-role.sh <maker|taker> — what the role's desk does on open
# (apps/basecamp/common/lez_role_signer.h, then lez_role_node.h), as
# logosctl calls in the role's session. The key is passed to lez_core as a
# call argument and never printed.
set -euo pipefail
role="$1"
S=$LOGS/session.sh
H=$LEZ_NODE_HOME
dir="$H/wallet/$role/store"
socket="$H/wallet/$role/signer.sock"
mkdir -p "$dir"
account="$(jq -r .account_id_hex "$H/identity/$role/identity.json" | tr '[:upper:]' '[:lower:]')"
printf '{"sequencers":[{"sequencer_addr":"http://127.0.0.1:3040","basic_auth":null}],"seq_poll_timeout":"12s","seq_tx_poll_max_blocks":5,"seq_poll_max_retries":5,"seq_block_poll_max_amount":100}' >"$dir/wallet_config.json"
echo "\$ module load lez_${role}_node"; "$S" "$role" module load "lez_${role}_node"
if [[ -f "$dir/storage.json" ]]; then
  echo "\$ call lez_core open <config> <storage> <statistics>"
  "$S" "$role" call lez_core open "$dir/wallet_config.json" "$dir/storage.json" "$dir/statistics.json"
else
  echo "\$ call lez_core create_new <config> <storage> <statistics> <password>"
  "$S" "$role" call lez_core create_new "$dir/wallet_config.json" "$dir/storage.json" "$dir/statistics.json" "$(openssl rand -hex 16)" | sed 's/"result":"[^"]*"/"result":"<redacted>"/'
fi
echo "\$ call lez_core import_public_account <key>"
"$S" "$role" call lez_core import_public_account "$(tr -d ' \n' <"$H/identity/$role/lez-signer.key")"
echo "\$ call lez_swap_node serve_signer $socket $account"
"$S" "$role" call lez_swap_node serve_signer "$socket" "$account"
echo "\$ call lez_${role}_node launcher"
launcher="$("$S" "$role" call "lez_${role}_node" launcher | tee /dev/stderr | jq -r '.result // .')"
echo "\$ call lez_swap_node start_node $role $launcher $H/$role.out.log $H/$role.hold"
"$S" "$role" call lez_swap_node start_node "$role" "$launcher" "$H/$role.out.log" "$H/$role.hold"
