#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly source_bin_dir="${SOURCE_BIN_DIR:-target/release}"
readonly destination_root="${DESTDIR:-}"
readonly unit_source="packaging/systemd/lez-maker-node.service"
readonly config_example="packaging/systemd/lez-maker-node.json.example"
readonly route_health_example="packaging/systemd/route-health.json.example"
readonly route_health_guide="packaging/systemd/README.md"
readonly launcher="modules/lez-role-node/node-entrypoint.sh"
# The LEZ sidecar links Python and is built in the builder image
# (deploy/scripts/from-scratch.sh --only build -> provision/data/sidecar).
readonly sidecar="${SIDECAR_PROGRAM:-$source_bin_dir/lez-v02-bridge-poc}"
readonly binaries=(lez-maker-node lez-maker-cli lez-zec-maker-actor lez-logos-price-worker lez-btc-maker-actor
  lez-runtime-healthcheck)

if [[ -n "$destination_root" && "$destination_root" != /* ]]; then
  echo "DESTDIR must be empty or an absolute staging root" >&2
  exit 2
fi
if [[ ! -d "$source_bin_dir" ]]; then
  echo "SOURCE_BIN_DIR does not exist: $source_bin_dir" >&2
  exit 1
fi

for source_path in "${binaries[@]/#/$source_bin_dir/}" "$sidecar" "$launcher"; do
  if [[ ! -f "$source_path" || ! -x "$source_path" || -L "$source_path" ]]; then
    echo "required built executable is missing, non-executable, or a symlink: $source_path" >&2
    exit 1
  fi
done
if [[ ! -f "$unit_source" || -L "$unit_source" ]]; then
  echo "systemd unit is missing or a symlink: $unit_source" >&2
  exit 1
fi

for binary in "${binaries[@]}"; do
  install -D -m 0755 "$source_bin_dir/$binary" \
    "$destination_root/usr/bin/$binary"
done
install -D -m 0755 "$sidecar" "$destination_root/usr/bin/lez-v02-bridge-poc"
install -D -m 0755 "$launcher" "$destination_root/usr/bin/lez-role-node-launcher"
install -D -m 0644 "$unit_source" \
  "$destination_root/usr/lib/systemd/system/lez-maker-node.service"
install -D -m 0600 "$config_example" \
  "$destination_root/etc/lez/maker/node.json.example"
install -D -m 0600 packaging/systemd/lez-maker-node.env.example \
  "$destination_root/etc/lez/maker/env.example"
# Chain outages are only reported when a route-health configuration names a probe per route;
# packaging/systemd/README.md says how to fill these in and why leaving a route out fails closed.
install -D -m 0600 "$route_health_example" \
  "$destination_root/etc/lez/maker/route-health.json.example"
install -D -m 0644 "$route_health_guide" \
  "$destination_root/usr/share/doc/lez-maker-node/README.md"
install -d -m 0700 "$destination_root/etc/lez/maker/credentials"

echo "installed canonical Maker Node artifacts below ${destination_root:-/}"
