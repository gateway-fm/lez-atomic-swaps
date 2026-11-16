#!/bin/bash
# run-suite-linux.sh <out dir> — node-e2e.py scenarios (deploy/ of the packages' source
# commit) against the two native Nodes the Basecamp desks started on this host; the chains
# are remote (RPC through the SSH tunnel), `docker` is docker-shim.py (bitcoin-cli only),
# ZEC corridors come from zec-provision.sh --native-home with the x86_64 tools in
# tools/zec. "<label> <args>" per stdin line, each with its own log, exit codes in results.txt.
. "$(dirname "$0")/env.sh"
out="$(realpath -m "${1:?out dir}")"; mkdir -p "$out"
export LEZ_NODE_HOME="$LV/h" SHIM_LOG="$out/docker-shim.log"
export LEZ_E2E_NATIVE_RUN_ROOT="$LEZ_NODE_HOME/run" LEZ_E2E_INDEXER_CHECK_ONLY=1
export LEZ_ZEC_NATIVE_TOOLS="$LV/tools/zec" PATH="$LV/tools/shim:$PATH"
cd "$LV/src/976f5f72/deploy" || exit 1
while read -r label args; do
  [[ -n "$label" ]] || continue
  echo "[$(date -u +%H:%M:%SZ)] start $label ($args)" | tee -a "$out/results.txt"
  # shellcheck disable=SC2086
  timeout -k 30 1800 python3 scripts/node-e2e.py $args </dev/null >"$out/$label.log" 2>&1
  rc=$?   # before any $(...) below resets $?
  echo "[$(date -u +%H:%M:%SZ)] $label exit $rc" | tee -a "$out/results.txt"
done
echo SUITE-DONE | tee -a "$out/results.txt"
