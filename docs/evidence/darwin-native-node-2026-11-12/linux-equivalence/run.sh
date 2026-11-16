#!/usr/bin/env bash
# Runs the launcher before and after the change in the role image (Linux, the
# real container paths, setpriv and socat present), with fakes for the Node,
# its CLI and curl, and prints everything each run produced.
set -euo pipefail
command -v setpriv socat jq bash | sed 's/^/tool: /'
bash --version | head -1
run() {
  local script="$1" role="$2"
  cd /; rm -rf /var/lib/lez /run/lez /delivery /run-lez-identity /run-market /run-config /payload /out
  mkdir -p /var/lib/lez/$role /run/lez/$role /delivery /run-lez-identity /run-market /run-config /payload /fakes /out
  printf '{"account_id_hex":"%s"}\n' ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff >/run-lez-identity/identity.json
  printf '%s' 1111111111111111111111111111111111111111111111111111111111111111 >/run-lez-identity/lez-signer.key
  printf '%s' 2222222222222222222222222222222222222222222222222222222222222222 >/run-lez-identity/zcash.key
  printf 'M3_POC_LEZ_ESCROW_PROGRAM_ID=%s\nM3_POC_LEZ_GENESIS_BLOCK_HASH=%s\n' "$(printf 'c%.0s' $(seq 64))" "$(printf 'd%.0s' $(seq 64))" >/run-market/market-bootstrap.env
  echo 'lezrpc:secret' >/run-config/btc-rpc-cookie
  if [[ $role == taker ]]; then echo 02abababababababababababababababababababababababababababababababab >/delivery/maker-delivery-identity.pub; fi
  cp "$script" /payload/node-entrypoint.sh
  printf '#!/bin/sh\n{ echo "pid $$"; for a; do echo "arg $a"; done; } >/out/node\n' >/payload/lez-$role-node
  printf '#!/bin/sh\nexit 0\n' >/payload/lez-btc-$role-actor
  printf '#!/bin/sh\necho 02abababababababababababababababababababababababababababababababab\n' >/payload/lez-maker-cli
  printf '#!/bin/sh\n: >"$2"\n' >/payload/lez-taker-registry-init
  chmod +x /payload/*
  printf '#!/bin/sh\nfor a in "$@"; do case "$a" in *getblockhash*) echo "{\\"result\\":\\"0f91\\"}"; exit 0;; *getnewaddress*) echo "{\\"result\\":\\"bcrt1p\\"}"; exit 0;; *getblockcount*) exit 0;; esac; done; exit 22\n' >/fakes/curl
  printf '#!/bin/sh\necho "socat ppid-is-node=$([ "$PPID" = "$(sed -n "s/^pid //p" /out/node 2>/dev/null)" ] && echo yes || echo pending) $*" >>/out/calls\n' >/fakes/socat
  chmod +x /fakes/*
  real_setpriv="$(command -v setpriv)"
  printf '#!/bin/sh\necho "setpriv $*" >>/out/calls\nexec %s "$@"\n' "$real_setpriv" >/fakes/setpriv; chmod +x /fakes/setpriv
  PATH="/fakes:$PATH" LEZ_SIDECAR_PORT_BASE=19000 LEZ_V02_CHANNEL_ID="$(printf 'e%.0s' $(seq 64))" LEZ_BTC_WALLET=lez-$role \
    LEZ_BTC_CLAIM_DESTINATION=wallet /payload/node-entrypoint.sh $role >/out/stdout 2>/out/stderr || echo "exit $?" >>/out/stdout
  sleep 1
  echo "== $role stdout"; cat /out/stdout; echo "== $role stderr"; cat /out/stderr
  echo "== $role node args"; grep -v '^pid' /out/node || true
  echo "== $role helpers"; sed 's/ppid-is-node=[a-z]*//' /out/calls | sort
  echo "== $role files"; (cd / && find var/lib/lez run/lez delivery -type f -o -type d | sort | while read -r f; do printf '%s %s\n' "$(stat -c %a "$f")" "$f"; done)
  for f in /var/lib/lez/$role/btc/btc-role.json /var/lib/lez/$role/btc/runtime.json /var/lib/lez/$role/role.json; do
    if [[ -f $f ]]; then echo "== $f"; jq -S . "$f"; fi
  done
}
for role in maker taker; do run /w/old.sh $role >/w/old-$role.txt 2>&1; run /w/new.sh $role >/w/new-$role.txt 2>&1; done
