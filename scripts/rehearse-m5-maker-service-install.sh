#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

stage="$(mktemp -d "${TMPDIR:-/tmp}/lez-m5-systemd-stage.XXXXXX")"
readonly stage
cleanup() {
  rm -rf -- "$stage"
}
trap cleanup EXIT

cargo build --locked -p lez-maker-node --bins
cargo build --locked -p zec-reference-actor --bin lez-zec-maker-actor
SOURCE_BIN_DIR="${SOURCE_BIN_DIR:-target/debug}" \
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
  -e "s#^ExecStart=/usr/bin/lez-maker-node#ExecStart=$stage/usr/bin/lez-maker-node#" \
  "$installed_unit" >"$verified_unit"
systemd-analyze verify "$verified_unit"

test "$(stat -c '%a' "$stage/usr/bin/lez-maker-node")" = 755
test "$(stat -c '%a' "$stage/usr/bin/lez-maker-cli")" = 755
test "$(stat -c '%a' "$stage/usr/lib/systemd/system/lez-maker-node.service")" = 644
test "$(stat -c '%a' "$stage/usr/bin/lez-zec-maker-actor")" = 755
test "$(stat -c '%a' "$stage/etc/lez/maker/credentials")" = 700
test "$(stat -c '%a' "$stage/etc/lez/maker/node.json.example")" = 600

echo "M5 maker service staged-install and systemd verification passed"
