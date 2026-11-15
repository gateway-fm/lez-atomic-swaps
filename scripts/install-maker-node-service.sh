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
# The Monero role is optional: its actor, its six effect tools (a pair-xmr build) and the
# Stage-A planner, built beside the sidecar (from-scratch.sh --only build:sidecar), are
# installed with the drop-in that enables it when the build has them, all or none.
readonly planner="${STAGE_A_PLANNER_PROGRAM:-$(dirname "$sidecar")/lez-v02-xmr-stage-a-plan}"
readonly monero_dropin="packaging/systemd/lez-maker-node-monero.conf"
readonly monero_binaries=(lez-xmr-maker-actor xmr-reference-monero-fund xmr-reference-tag15
  xmr-reference-tag15-finalized xmr-reference-monero-refund xmr-reference-monero-verify xmr-reference-tag17)

if [[ -n "$destination_root" && "$destination_root" != /* ]]; then
  echo "DESTDIR must be empty or an absolute staging root" >&2
  exit 2
fi
if [[ ! -d "$source_bin_dir" ]]; then
  echo "SOURCE_BIN_DIR does not exist: $source_bin_dir" >&2
  exit 1
fi

monero_sources=("${monero_binaries[@]/#/$source_bin_dir/}" "$planner")
monero_missing=()
for source_path in "${monero_sources[@]}"; do
  [[ -e "$source_path" || -L "$source_path" ]] || monero_missing+=("$source_path")
done
monero=0
if (( ${#monero_missing[@]} == 0 )); then
  monero=1
elif (( ${#monero_missing[@]} < ${#monero_sources[@]} )); then
  echo "a partial Monero set: install all of the Monero actor, its six tools and the Stage-A planner, or none; missing: ${monero_missing[*]}" >&2
  exit 1
fi
optional_sources=()
(( monero == 0 )) || optional_sources=("${monero_sources[@]}")

for source_path in "${binaries[@]/#/$source_bin_dir/}" "$sidecar" "$launcher" ${optional_sources[@]+"${optional_sources[@]}"}; do
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
if (( monero == 1 )); then
  for binary in "${monero_binaries[@]}"; do
    install -D -m 0755 "$source_bin_dir/$binary" "$destination_root/usr/bin/$binary"
  done
  install -D -m 0755 "$planner" "$destination_root/usr/bin/lez-v02-xmr-stage-a-plan"
  # Copied into lez-maker-node.service.d/, this drop-in loads the Monero logins.
  install -D -m 0644 "$monero_dropin" "$destination_root/etc/lez/maker/monero.conf.example"
fi
# Chain outages are only reported when a route-health configuration names a probe per route;
# packaging/systemd/README.md says how to fill these in and why leaving a route out fails closed.
install -D -m 0600 "$route_health_example" \
  "$destination_root/etc/lez/maker/route-health.json.example"
install -D -m 0644 "$route_health_guide" \
  "$destination_root/usr/share/doc/lez-maker-node/README.md"
install -d -m 0700 "$destination_root/etc/lez/maker/credentials"

echo "installed canonical Maker Node artifacts below ${destination_root:-/}$( (( monero == 0 )) || echo ', with the Monero role')"
