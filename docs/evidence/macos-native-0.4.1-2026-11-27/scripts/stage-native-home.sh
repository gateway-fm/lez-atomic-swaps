#!/bin/bash
# stage-native-home.sh <stack dir> <node home> <role>
# A fresh LEZ_NODE_HOME for ONE native role Node on this Mac, from the
# running stack's runtime/ ("The Node on macOS", logos-native-packaging.md):
# identity/<role>/ (identity.json, lez-signer.key, the Maker's zcash.key and
# the role's two Monero logins: monerod's and its own wallet-rpc's),
# market-bootstrap.env, btc-rpc-cookie, Zebra's cookie (zebra-cookie), all
# 0600 in 0700 directories. Also the native monero-wallet-rpc's config
# (xmr/wallet-rpc.conf, 0600) with the role's wallet-rpc login and monerod's
# login, bound to 127.0.0.1:18083 and talking to the stack's monerod on
# 127.0.0.1:18081. Nothing secret is printed.
set -euo pipefail
S="${1:?stack dir}"; H="${2:?node home}"; role="${3:?maker|taker}"
R="$S/runtime"
prefix="$(sed -n 's/^LEZ_STACK_PREFIX=//p' "$R/runtime.env" | head -1)"
umask 077
test ! -e "$H" || { echo "$H exists" >&2; exit 2; }
mkdir -p "$H/identity/$role" "$H/xmr/wallets" "$H/xmr/log"
src="$R/lez/$role"
for f in identity.json lez-signer.key monerod.login monero-wallet-rpc.login; do install -m 0600 "$src/$f" "$H/identity/$role/$f"; done
[[ "$role" != maker ]] || install -m 0600 "$src/zcash.key" "$H/identity/$role/zcash.key"
install -m 0600 "$R/market-bootstrap.env" "$H/market-bootstrap.env"
install -m 0600 "$R/secrets/btc-rpc-cookie" "$H/btc-rpc-cookie"
docker exec "$prefix-zebra" cat /var/lib/zebra/regtest/.cookie > "$H/zebra-cookie"
[[ -s "$H/zebra-cookie" ]] || { echo "no Zebra cookie" >&2; exit 2; }
chmod 0600 "$H/zebra-cookie"
{
  echo "wallet-dir=$H/xmr/wallets"
  echo "rpc-bind-ip=127.0.0.1"
  echo "rpc-bind-port=18083"
  echo "rpc-login=$(cat "$R/monero/$role.login")"
  echo "rpc-ssl=disabled"
  echo "daemon-address=127.0.0.1:18081"
  echo "daemon-login=$(cat "$R/monero/daemon.login")"
  echo "daemon-ssl=disabled"
  echo "trusted-daemon=1"
  echo "allow-mismatched-daemon-version=1"
  echo "no-initial-sync=1"
  echo "non-interactive=1"
  echo "max-concurrency=1"
  echo "log-file=$H/xmr/log/wallet-rpc.log"
  echo "log-level=1"
} > "$H/xmr/wallet-rpc.conf"
chmod 0600 "$H/xmr/wallet-rpc.conf"
find "$H" -type d -exec chmod 700 {} +
echo "staged $role home (prefix $prefix): $(cd "$H" && find . -type f | sort | tr '\n' ' ')"
