#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

stage="$(mktemp -d "${TMPDIR:-/tmp}/lez-m5-systemd-stage.XXXXXX")"
readonly stage
cleanup() {
  rm -rf -- "$stage"
}
trap cleanup EXIT

cargo build --locked -p lez-maker-node --features pair-zec,pair-xmr --bins
cargo build --locked -p xmr-reference-actor --bin xmr-reference-monero-fund --bin xmr-reference-tag15 \
  --bin xmr-reference-tag15-finalized --bin xmr-reference-monero-refund --bin xmr-reference-monero-verify \
  --bin xmr-reference-tag17
cargo build --locked -p zec-reference-actor --bin lez-zec-maker-actor
cargo build --locked -p lez-logos-price-c-api --bin lez-logos-price-worker
cargo build --locked -p btc-reference-actor --bin lez-btc-maker-actor
cargo build --locked -p lez-runtime-healthcheck
# The LEZ sidecar and the Monero Stage-A planner come from the builder image; the staged
# install is only verified, never run.
for builder_program in lez-v02-bridge-poc lez-v02-xmr-stage-a-plan; do
  printf '#!/bin/sh\nexit 1\n' >"$stage/$builder_program"
  chmod 0755 "$stage/$builder_program"
done
SOURCE_BIN_DIR="${SOURCE_BIN_DIR:-target/debug}" \
SIDECAR_PROGRAM="${SIDECAR_PROGRAM:-$stage/lez-v02-bridge-poc}" \
DESTDIR="$stage" \
  ./scripts/install-maker-node-service.sh

install -d -m 0755 "$stage/etc"
printf '%s\n' \
  'root:x:0:0:root:/root:/bin/false' \
  'lez-swap:x:991:991:LEZ swap service:/var/lib/lez-atomic-swaps:/usr/sbin/nologin' \
  >"$stage/etc/passwd"
printf '%s\n' \
  'root:x:0:' \
  'lez-swap:x:991:' \
  >"$stage/etc/group"

readonly installed_unit="$stage/usr/lib/systemd/system/lez-maker-node.service"
readonly verified_unit="$stage/lez-maker-node-verify.service"
sed \
  -e "s#^User=lez-swap#User=$(id -un)#" \
  -e "s#^Group=lez-swap#Group=$(id -gn)#" \
  -e "s#^ExecStart=/usr/bin/#ExecStart=$stage/usr/bin/#" \
  "$installed_unit" >"$verified_unit"
systemd-analyze verify "$verified_unit"
# ... and with the Monero drop-in, as an operator who enables Monero installs it.
install -D -m 0644 "$stage/etc/lez/maker/monero.conf.example" "$verified_unit.d/monero.conf"
systemd-analyze verify "$verified_unit"

test "$(stat -c '%a' "$stage/usr/bin/lez-maker-node")" = 755
test "$(stat -c '%a' "$stage/usr/bin/lez-maker-cli")" = 755
test "$(stat -c '%a' "$stage/usr/lib/systemd/system/lez-maker-node.service")" = 644
test "$(stat -c '%a' "$stage/usr/bin/lez-zec-maker-actor")" = 755
test "$(stat -c '%a' "$stage/usr/bin/lez-logos-price-worker")" = 755
test "$(stat -c '%a' "$stage/usr/bin/lez-xmr-maker-actor")" = 755
test "$(stat -c '%a' "$stage/usr/bin/xmr-reference-tag17")" = 755
test "$(stat -c '%a' "$stage/usr/bin/lez-v02-xmr-stage-a-plan")" = 755
test "$(stat -c '%a' "$stage/etc/lez/maker/credentials")" = 700
test "$(stat -c '%a' "$stage/etc/lez/maker/node.json.example")" = 600

echo "M5 maker service staged-install and systemd verification passed"
