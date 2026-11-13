#!/bin/bash
# run-suite.sh <out dir> — node-e2e.py scenarios against the native Nodes the
# catalog-installed packages run, one after another, each with its own log;
# reads "<label> <node-e2e args>" lines on stdin. Run from the checkout's
# deploy/ (runtime/runtime.env and runtime/heartbeat link to the stack's).
set -u
out="${1:?out dir}"; mkdir -p "$out"
export LEZ_E2E_NATIVE_RUN_ROOT="$LEZ_NODE_HOME/run" LEZ_E2E_INDEXER_CHECK_ONLY=1
while read -r label args; do
  [[ -n "$label" ]] || continue
  # shellcheck disable=SC2086
  timeout -k 10 5400 python3 scripts/node-e2e.py $args </dev/null >"$out/$label.log" 2>&1
  echo "$label exit $?" | tee -a "$out/results.txt"
done
