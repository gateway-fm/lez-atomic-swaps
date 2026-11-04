#!/bin/bash
# lez-indexer-module <indexer_config.json> [--port N]
# Starts lez_indexer_module in a headless logoscore with the indexer_service
# config, then has the module serve indexer_service's JSON-RPC on the port
# indexer_service used (serve_rpc, inside the module process).
set -euo pipefail
config="${1:?indexer config}"; port=8779
[[ "${2:-}" == --port ]] && port="${3:?port}"
L=/opt/logoscore/AppRun
data=/work/state/logoscore; mkdir -p "$data"
cd /work/state
$L daemon --modules-dir /opt/lez-modules --persistence-path "$data" &
daemon=$!
stop() { $L call lez_indexer_module stop_indexer >/dev/null 2>&1 || true; kill -INT "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; exit 0; }
trap stop INT TERM
for _ in $(seq 1 60); do out="$($L load-module lez_indexer_module 2>&1 || true)"; [[ "$out" == *'"status":"ok"'* ]] && break; sleep 1; done
[[ "$out" == *'"status":"ok"'* ]] || { echo "lez_indexer_module did not load: $out" >&2; kill "$daemon"; exit 1; }
echo "lez-indexer-module: $out"
out="$($L call lez_indexer_module start_indexer "$config" 2>&1)"
echo "lez-indexer-module: start_indexer → $out"
[[ "$out" == *'"result":0'* ]] || { echo "start_indexer failed" >&2; kill -INT "$daemon"; exit 1; }
out="$($L call lez_indexer_module serve_rpc "$port" 2>&1)"
echo "lez-indexer-module: serve_rpc $port → $out"
[[ "$out" == *'"result":0'* ]] || { echo "serve_rpc failed" >&2; kill -INT "$daemon"; exit 1; }
wait "$daemon"
