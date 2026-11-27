#!/bin/bash
# start-native.sh <lez_core.lgx> <lez_swap_node.lgx> <lez_<role>_node.lgx>
# What the role's desk does on open, headless in the role's one logosctl
# session: install the three packages, load the role module, open (or create)
# the LEZ wallet in lez_core, import the role's key, serve the signer socket
# through lez_swap_node, ask the role module for its launcher and have
# lez_swap_node start the Node from it. The key goes to lez_core as a call
# argument and is never printed; the wallet password is random, never kept
# or printed (create_new's result is redacted).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
S="$here/session.sh"; H=$LEZ_NODE_HOME; role=$ROLE
red() { sed -e "s#$H#\$LEZ_NODE_HOME#g" -e "s#$SESSION#\$SESSION#g" -e 's/"result":"[0-9a-f]\{40,\}"/"result":"<redacted>"/'; }
for p in "$@"; do echo "\$ package install $(basename "$p") -y"; "$S" package install "$p" -y 2>&1 | red; done
echo "\$ module load lez_${role}_node"; "$S" module load "lez_${role}_node" 2>&1 | red
dir="$H/wallet/lez"
printf '{"sequencers":[{"sequencer_addr":"http://127.0.0.1:3040","basic_auth":null}],"seq_poll_timeout":"12s","seq_tx_poll_max_blocks":5,"seq_poll_max_retries":5,"seq_block_poll_max_amount":100}' >"$dir/wallet_config.json"
if [[ -f "$dir/storage.json" ]]; then
  echo "\$ call lez_core open <config> <storage> <statistics>"
  "$S" call lez_core open "$dir/wallet_config.json" "$dir/storage.json" "$dir/statistics.json" | red
else
  echo "\$ call lez_core create_new <config> <storage> <statistics> <password>"
  "$S" call lez_core create_new "$dir/wallet_config.json" "$dir/storage.json" "$dir/statistics.json" "$(openssl rand -hex 16)" | sed 's/"result":"[^"]*"/"result":"<redacted>"/'
fi
account="$(jq -r .account_id_hex "$H/identity/$role/identity.json" | tr '[:upper:]' '[:lower:]')"
echo "\$ call lez_core import_public_account <$role key>"
"$S" call lez_core import_public_account "$(tr -d ' \n' <"$H/identity/$role/lez-signer.key")" | red
echo "\$ call lez_swap_node serve_signer \$LEZ_NODE_HOME/wallet/$role.sock $account"
"$S" call lez_swap_node serve_signer "$H/wallet/$role.sock" "$account" | red
echo "\$ call lez_${role}_node launcher"
launcher="$("$S" call "lez_${role}_node" launcher | jq -r '.result')"
echo "{\"result\":\"$launcher\"}" | red
echo "\$ call lez_swap_node start_node $role <launcher> \$LEZ_NODE_HOME/$role.out.log \$LEZ_NODE_HOME/$role.hold"
"$S" call lez_swap_node start_node "$role" "$launcher" "$H/$role.out.log" "$H/$role.hold" | red
for _ in $(seq 90); do [[ -S "$H/run/$role/node.sock" ]] && break; sleep 2; done
[[ -S "$H/run/$role/node.sock" ]] && echo "node socket up" || { echo "no node socket after 180 s" >&2; exit 1; }
