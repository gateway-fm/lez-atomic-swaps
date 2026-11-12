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
  printf 'M3_POC_LEZ_ESCROW_PROGRAM_ID=%s\nM3_POC_LEZ_GENESIS_BLOCK_HASH=%s\n' $(printf 'c%.0s' $(seq 64)) $(printf 'd%.0s' $(seq 64)) >/run-market/market-bootstrap.env
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
  extra=()
  if [[ -n "${WITH_ZEC:-}" ]]; then
    # One prepared ZEC corridor for this chain; the fake sidecar listens on its port.
    c=/var/lib/lez/zec/corridor/$role; mkdir -p $c
    port=19901; [[ $role == taker ]] && port=19902
    e=$(printf 'e%.0s' $(seq 64)); d=$(printf 'd%.0s' $(seq 64)); cc=$(printf 'c%.0s' $(seq 64))
    printf '{"run_id":"zec-run-1","bridge":{"endpoint":"http://127.0.0.1:%s","runtime":{"channel_id":"%s","genesis_block_hash":"%s","escrow_program_id":"%s"}}}' $port $e $d $cc >$c/actor-config.json
    printf '{"signer_account_id":"acc"}' >$c/lez-runtime.json; echo cap >$c/sidecar.capability; echo k >$c/lez-signer.key
    if [[ $role == taker ]]; then
      python3 -c 'import json; json.dump({"maker_public_key": list(bytes.fromhex("02"+"ab"*32))}, open("/var/lib/lez/zec/env.json","w"))'
      echo '[{"swap_id":"s","signed_envelope":{"path":"/var/lib/lez/zec/env.json"}}]' >/var/lib/lez/zec/prepared.json
    fi
    printf '#!/bin/bash\nfor a in "$@"; do [[ $prev == --listen-address ]] && l=$a; prev=$a; done\necho "sidecar $*" >>/out/calls\nexec /usr/bin/socat TCP-LISTEN:${l##*:},bind=127.0.0.1,fork,reuseaddr /dev/null\n' >/payload/lez-v02-bridge-poc
    printf '#!/bin/sh\nexit 0\n' >/payload/lez-zec-$role-actor
    chmod +x /payload/lez-v02-bridge-poc /payload/lez-zec-$role-actor
  fi
  if [[ -n "${WITH_SIGNER:-}" ]]; then
    mkdir -p /run/lez-wallet; rm -f /run/lez-wallet/signer.sock
    socat UNIX-LISTEN:/run/lez-wallet/signer.sock,fork /dev/null & sleep 0.5
    extra=(LEZ_WALLET_SIGNER_SOCKET=/run/lez-wallet/signer.sock LEZ_NODE_LOG_FILE=/run/lez/$role/node.log)
  fi
  env "${extra[@]}" PATH="/fakes:$PATH" LEZ_SIDECAR_PORT_BASE=19000 LEZ_V02_CHANNEL_ID=$(printf 'e%.0s' $(seq 64)) LEZ_BTC_WALLET=lez-$role \
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
mkdir -p "/w/$IMAGE"
for role in maker taker; do run /w/old.sh $role >"/w/$IMAGE/old-$role.txt" 2>&1; run /w/new.sh $role >"/w/$IMAGE/new-$role.txt" 2>&1; done
export WITH_SIGNER=1
for role in maker taker; do run /w/old.sh $role >"/w/$IMAGE/old-$role-signer.txt" 2>&1; run /w/new.sh $role >"/w/$IMAGE/new-$role-signer.txt" 2>&1; done
export WITH_ZEC=1
for role in maker taker; do run /w/old.sh $role >"/w/$IMAGE/old-$role-zec.txt" 2>&1; pkill -f "socat TCP-LISTEN:199" || true; run /w/new.sh $role >"/w/$IMAGE/new-$role-zec.txt" 2>&1; pkill -f "socat TCP-LISTEN:199" || true; done
