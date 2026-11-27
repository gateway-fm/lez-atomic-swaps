#!/usr/bin/env bash
# The three Phase B scenarios with the Maker native, one after the other.
set -u
here="$(cd "$(dirname "$0")" && pwd)"; L="$here/../logs"
for s in xmr-happy xmr-taker-refund xmr-punish; do
  "$here/run-mixed.sh" "$L/B-maker-$s.log" "$s"
  cp "$MIXED_STACK/runtime/e2e/$s-lez.json" "$L/B-e2e-maker-$s-lez.json" 2>/dev/null
  echo "$s $(grep -oE '=== '"$s"': [a-z_]+ in [0-9]+s' "$L/B-maker-$s.log")"
  grep -q "=== $s: passed" "$L/B-maker-$s.log" || break
done
echo ALL-DONE
