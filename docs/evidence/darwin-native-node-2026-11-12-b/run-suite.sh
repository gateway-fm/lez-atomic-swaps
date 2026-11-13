#!/opt/homebrew/bin/bash
# run-suite.sh <name> <node-e2e args>... — native scenarios one after another,
# each with its own log under $LOGS/suite/; stops at nothing (records all).
set -u
L=$LOGS/suite; mkdir -p "$L"
cd "$CHECKOUT/deploy" || exit 1
export LEZ_E2E_NATIVE_RUN_ROOT=$LEZ_NODE_HOME/run LEZ_E2E_INDEXER_CHECK_ONLY=1
while read -r label args; do
  [[ -n "$label" ]] || continue
  # shellcheck disable=SC2086
  timeout -k 10 5400 python3 scripts/node-e2e.py $args </dev/null >"$L/$label.log" 2>&1
  echo "$label exit $?" | tee -a "$L/results.txt"
done <<LIST
$(cat)
LIST
