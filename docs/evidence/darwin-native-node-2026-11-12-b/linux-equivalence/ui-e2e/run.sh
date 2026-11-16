#!/opt/homebrew/bin/bash
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"; src=$CHECKOUT/deploy
for version in old new; do for direction in TakerSellsForeign TakerSellsLez; do
  w="$(mktemp -d $TMPDIR/ui.XXXXXX)"; mkdir -p "$w/deploy/scripts" "$w/deploy/runtime" "$w/bin" "$w/state"
  cp "$src/scripts/lane.sh" "$src/scripts/host.sh" "$w/deploy/scripts/"; cp "$here/$version.sh" "$w/deploy/scripts/ui-e2e.sh"
  for t in docker date; do ln -s "$here/fake.py" "$w/bin/$t"; done
  echo "LEZ_TIMING_PROFILE=fast" >"$w/deploy/runtime/runtime.env"
  (cd "$w/deploy" && env PATH="$w/bin:$PATH" CALLS="$w/calls" FAKE_STATE="$w/state" LEZ_REPAIR_INDEXER=0 LEZ_EXPORT_EVIDENCE=0 \
    bash scripts/ui-e2e.sh happy --direction "$direction" >"$w/out" 2>&1; echo "exit $?" >>"$w/out") || true
  { cat "$w/out"; echo "== calls"; cat "$w/calls"; } | sed "s|$w|<w>|g" >"$here/$version-$direction.txt"
done; done
