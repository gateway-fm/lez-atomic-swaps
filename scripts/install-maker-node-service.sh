#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly source_bin_dir="${SOURCE_BIN_DIR:-target/release}"
readonly destination_root="${DESTDIR:-}"
readonly unit_source="packaging/systemd/lez-maker-node.service"
readonly config_example="packaging/systemd/lez-maker-node.json.example"

if [[ -n "$destination_root" && "$destination_root" != /* ]]; then
  echo "DESTDIR must be empty or an absolute staging root" >&2
  exit 2
fi
if [[ ! -d "$source_bin_dir" ]]; then
  echo "SOURCE_BIN_DIR does not exist: $source_bin_dir" >&2
  exit 1
fi

for binary in lez-maker-node lez-maker-cli lez-zec-maker-actor; do
  source_path="$source_bin_dir/$binary"
  if [[ ! -f "$source_path" || ! -x "$source_path" || -L "$source_path" ]]; then
    echo "required built executable is missing, non-executable, or a symlink: $source_path" >&2
    exit 1
  fi
done
if [[ ! -f "$unit_source" || -L "$unit_source" ]]; then
  echo "systemd unit is missing or a symlink: $unit_source" >&2
  exit 1
fi

for binary in lez-maker-node lez-maker-cli lez-zec-maker-actor; do
  install -D -m 0755 "$source_bin_dir/$binary" \
    "$destination_root/usr/bin/$binary"
done
install -D -m 0644 "$unit_source" \
  "$destination_root/usr/lib/systemd/system/lez-maker-node.service"
install -D -m 0600 "$config_example" \
  "$destination_root/etc/lez/maker/node.json.example"
install -d -m 0700 "$destination_root/etc/lez/maker/credentials"

echo "installed canonical Maker Node artifacts below ${destination_root:-/}"
