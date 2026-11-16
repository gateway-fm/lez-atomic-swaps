#!/bin/bash
# start-both.sh — what the two desks do on open, in the one session whose
# packages came from the catalog: one LEZ wallet (lez_core) holding both
# roles' accounts, one signer socket per role, both Nodes started by
# lez_swap_node from the installed role packages' launchers. The keys are
# passed to lez_core as call arguments and never printed.
set -euo pipefail
S="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/session.sh"
H=$LEZ_NODE_HOME
dir="$H/wallet/shared"
mkdir -p "$dir"
printf '{"sequencers":[{"sequencer_addr":"http://127.0.0.1:3040","basic_auth":null}],"seq_poll_timeout":"12s","seq_tx_poll_max_blocks":5,"seq_poll_max_retries":5,"seq_block_poll_max_amount":100}' >"$dir/wallet_config.json"
for role in maker taker; do echo "\$ module load lez_${role}_node"; "$S" module load "lez_${role}_node"; done
if [[ -f "$dir/storage.json" ]]; then
  echo "\$ call lez_core open <config> <storage> <statistics>"
  "$S" call lez_core open "$dir/wallet_config.json" "$dir/storage.json" "$dir/statistics.json"
else
  echo "\$ call lez_core create_new <config> <storage> <statistics> <password>"
  "$S" call lez_core create_new "$dir/wallet_config.json" "$dir/storage.json" "$dir/statistics.json" "$(openssl rand -hex 16)" | sed 's/"result":"[^"]*"/"result":"<redacted>"/'
fi
for role in maker taker; do
  account="$(jq -r .account_id_hex "$H/identity/$role/identity.json" | tr '[:upper:]' '[:lower:]')"
  echo "\$ call lez_core import_public_account <$role key>"
  "$S" call lez_core import_public_account "$(tr -d ' \n' <"$H/identity/$role/lez-signer.key")"
  echo "\$ call lez_swap_node serve_signer \$LEZ_NODE_HOME/wallet/$role.sock $account"
  "$S" call lez_swap_node serve_signer "$H/wallet/$role.sock" "$account"
done
for role in maker taker; do
  echo "\$ call lez_${role}_node launcher"
  launcher="$("$S" call "lez_${role}_node" launcher | tee /dev/stderr | jq -r '.result')"
  echo "\$ call lez_swap_node start_node $role $launcher \$LEZ_NODE_HOME/$role.out.log \$LEZ_NODE_HOME/$role.hold"
  "$S" call lez_swap_node start_node "$role" "$launcher" "$H/$role.out.log" "$H/$role.hold"
  # The Taker reads the Maker's Delivery identity at start: Maker first.
  if [[ $role == maker ]]; then
    for _ in $(seq 60); do [[ -S "$H/run/maker/node.sock" ]] && break; sleep 2; done
  fi
done
