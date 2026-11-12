#!/opt/homebrew/bin/bash
# Runs zec-provision.sh before (old.sh) and after (new.sh) in container mode
# against fakes of docker, curl and date (fake.py), in a throwaway deploy
# tree, and prints what each run called and wrote. Both directions.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
src=$CHECKOUT/deploy
python3 "$here/zebra.py" 18299 & zebra_pid=$!; trap 'kill $zebra_pid' EXIT; sleep 0.5
for version in old new; do for direction in TakerSellsLez TakerSellsForeign; do for restart in --no-restart --restart; do
  w="$(mktemp -d $TMPDIR/zp.XXXXXX)"
  mkdir -p "$w/deploy/scripts" "$w/bin" "$w/state" "$w/deploy/runtime/lez/maker" "$w/deploy/runtime/lez/taker"
  cp "$src/scripts/lane.sh" "$src/scripts/host.sh" "$w/deploy/scripts/"
  cp "$here/$version.sh" "$w/deploy/scripts/zec-provision.sh"
  for t in docker curl date; do ln -s "$here/fake.py" "$w/bin/$t"; done
  printf 'LEZ_V02_CHANNEL_ID=%s\nLEZ_VOLUME_PREFIX=lez\nLEZ_STACK_PORT_OFFSET=67\n' "$(printf 'e%.0s' {1..64})" >"$w/deploy/runtime/runtime.env"
  printf 'M3_POC_LEZ_ESCROW_PROGRAM_ID=%s\nM3_POC_LEZ_GENESIS_BLOCK_HASH=%s\n' "$(printf 'c%.0s' {1..64})" "$(printf 'd%.0s' {1..64})" >"$w/deploy/runtime/market-bootstrap.env"
  for r in maker taker; do
    echo "{\"account_id\":\"Acc$r\",\"account_id_hex\":\"$(printf 'f%.0s' {1..64})\"}" >"$w/deploy/runtime/lez/$r/identity.json"
    echo k >"$w/deploy/runtime/lez/$r/zcash.key"; echo k >"$w/deploy/runtime/lez/$r/lez-signer.key"
  done
  (cd "$w/deploy" && env PATH="$w/bin:$PATH" CALLS="$w/calls" FAKE_STATE="$w/state" \
     bash scripts/zec-provision.sh --skip-chain-check ${restart/--restart/} --direction "$direction" >"$w/stdout" 2>"$w/stderr"; echo "exit $?" >>"$w/stdout") || true
  {
    echo "== stdout"; cat "$w/stdout"; echo "== stderr"; cat "$w/stderr"
    echo "== calls"; sed "s|$w|<w>|g" "$w/calls"
    echo "== files"; (cd "$w/deploy/runtime" && find zec -type f | sort | while read -r f; do echo "-- $f"; cat "$f"; done)
  } | sed "s|$w|<w>|g" >"$here/$version-$direction$restart.txt"
done; done; done
