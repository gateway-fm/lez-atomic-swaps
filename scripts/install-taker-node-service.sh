#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly source_bin_dir="${SOURCE_BIN_DIR:-target/release}"
readonly destination_root="${DESTDIR:-}"
readonly unit_source="packaging/systemd/lez-taker-node.service"

if [[ -n "$destination_root" && "$destination_root" != /* ]]; then
  echo "DESTDIR must be empty or an absolute staging root" >&2
  exit 2
fi
for path in \
  "$source_bin_dir/lez-taker-node" \
  "$source_bin_dir/lez-taker-cli" \
  "$unit_source" \
  packaging/systemd/lez-taker-node.json.example \
  packaging/systemd/lez-taker-role.json.example; do
  if [[ ! -f "$path" || -L "$path" ]]; then
    echo "required Taker Node artifact is missing or a symlink: $path" >&2
    exit 1
  fi
done

install -D -m 0755 "$source_bin_dir/lez-taker-node" "$destination_root/usr/bin/lez-taker-node"
install -D -m 0755 "$source_bin_dir/lez-taker-cli" "$destination_root/usr/bin/lez-taker-cli"
install -D -m 0644 "$unit_source" \
  "$destination_root/usr/lib/systemd/system/lez-taker-node.service"
install -D -m 0600 packaging/systemd/lez-taker-node.json.example \
  "$destination_root/etc/lez/taker/node.json.example"
install -D -m 0600 packaging/systemd/lez-taker-role.json.example \
  "$destination_root/etc/lez/taker/role.json.example"
install -d -m 0700 "$destination_root/etc/lez/taker/credentials"

echo "installed canonical Taker Node artifacts below ${destination_root:-/}"
