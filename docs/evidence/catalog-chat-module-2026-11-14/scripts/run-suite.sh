#!/opt/homebrew/bin/bash
# run-suite.sh <out dir> — node-e2e.py scenarios (from this checkout's deploy/)
# against the native Nodes the Basecamp desks started; "<label> <args>" per
# stdin line, each with its own log, exit codes in results.txt.
set -u
out="${1:?out dir}"; mkdir -p "$out"
export PATH=/opt/homebrew/bin:$PATH
export LEZ_ZEC_NATIVE_TOOLS LEZ_E2E_NATIVE_RUN_ROOT="${LEZ_NODE_HOME:?}/run" LEZ_E2E_INDEXER_CHECK_ONLY=1
cd ${CHECKOUT:?}/deploy || exit 1
while read -r label args; do
  [[ -n "$label" ]] || continue
  echo "[$(date -u +%H:%M:%SZ)] start $label ($args)" | tee -a "$out/results.txt"
  # shellcheck disable=SC2086
  timeout -k 30 1800 python3 scripts/node-e2e.py $args </dev/null >"$out/$label.log" 2>&1
  rc=$?
  echo "[$(date -u +%H:%M:%SZ)] $label exit $rc" | tee -a "$out/results.txt"
done
echo SUITE-DONE | tee -a "$out/results.txt"
