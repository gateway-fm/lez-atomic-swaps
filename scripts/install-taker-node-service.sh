#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly source_bin_dir="${SOURCE_BIN_DIR:-target/release}"
readonly destination_root="${DESTDIR:-}"
readonly unit_source="packaging/systemd/lez-taker-node.service"
readonly launcher="modules/lez-role-node/node-entrypoint.sh"
# The LEZ sidecar links Python and is built in the builder image
# (deploy/scripts/from-scratch.sh --only build -> provision/data/sidecar).
readonly sidecar="${SIDECAR_PROGRAM:-$source_bin_dir/lez-v02-bridge-poc}"
readonly binaries=(lez-taker-node lez-taker-cli lez-btc-taker-actor lez-taker-registry-init lez-runtime-healthcheck)

if [[ -n "$destination_root" && "$destination_root" != /* ]]; then
  echo "DESTDIR must be empty or an absolute staging root" >&2
  exit 2
fi
for path in \
  "${binaries[@]/#/$source_bin_dir/}" \
  "$sidecar" \
  "$launcher" \
  "$unit_source" \
  packaging/systemd/lez-taker-node.env.example \
  packaging/systemd/lez-taker-node.json.example \
  packaging/systemd/lez-taker-role.json.example; do
  if [[ ! -f "$path" || -L "$path" ]]; then
    echo "required Taker Node artifact is missing or a symlink: $path" >&2
    exit 1
  fi
done

for binary in "${binaries[@]}"; do
  install -D -m 0755 "$source_bin_dir/$binary" "$destination_root/usr/bin/$binary"
done
install -D -m 0755 "$sidecar" "$destination_root/usr/bin/lez-v02-bridge-poc"
install -D -m 0755 "$launcher" "$destination_root/usr/bin/lez-role-node-launcher"
install -D -m 0644 "$unit_source" \
  "$destination_root/usr/lib/systemd/system/lez-taker-node.service"
install -D -m 0600 packaging/systemd/lez-taker-node.json.example \
  "$destination_root/etc/lez/taker/node.json.example"
install -D -m 0600 packaging/systemd/lez-taker-role.json.example \
  "$destination_root/etc/lez/taker/role.json.example"
install -D -m 0600 packaging/systemd/lez-taker-node.env.example \
  "$destination_root/etc/lez/taker/env.example"
install -d -m 0700 "$destination_root/etc/lez/taker/credentials"

echo "installed canonical Taker Node artifacts below ${destination_root:-/}"
