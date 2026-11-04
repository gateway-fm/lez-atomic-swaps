#!/bin/bash
# lez-network-module <node-config.yaml> --deployment <deployment-settings.yaml>
# Hosts blockchain_module in a headless logoscore and starts the node with the
# same two files the bedrock binary took. Ctrl-C/SIGTERM stop the node through
# the module (a node that stopped on its own keeps its ports until stop is
# called, upstream #111), then the daemon.
set -euo pipefail
config="${1:?node config}"; [[ "${2:-}" == --deployment ]] || { echo "usage: $0 <node-config> --deployment <settings>" >&2; exit 64; }
deployment="${3:?deployment settings}"
L=/opt/logoscore/AppRun
data=/work/state/logoscore; mkdir -p "$data"
$L daemon --modules-dir /opt/lez-modules --persistence-path "$data" &
daemon=$!
stop() { $L call blockchain_module stop >/dev/null 2>&1 || true; kill -INT "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; exit 0; }
trap stop INT TERM
for _ in $(seq 1 60); do
  out="$($L load-module blockchain_module 2>&1 || true)"
  [[ "$out" == *'"status":"ok"'* ]] && break; sleep 1
done
[[ "$out" == *'"status":"ok"'* ]] || { echo "blockchain_module did not load: $out" >&2; kill "$daemon"; exit 1; }
echo "lez-network-module: $out"
out="$($L call blockchain_module start "$config" "$deployment" 2>&1)"
echo "lez-network-module: start → $out"
[[ "$out" == *'"success":true'* ]] || { echo "blockchain_module start failed" >&2; $L call blockchain_module stop >/dev/null 2>&1 || true; kill -INT "$daemon"; exit 1; }
wait "$daemon"
