#!/usr/bin/env bash
set -euo pipefail

readonly circuits_version="v0.4.2"
readonly circuits_sha256="e9131ffac8b08a80e1a7152b34fdd5d5c52674d4cb396e8162131ca5dd7c858d"
readonly circuits_dir="${1:?usage: install-ci-logos-circuits.sh ABSOLUTE_DESTINATION}"

if [[ "$circuits_dir" != /* || -L "$circuits_dir" ]]; then
  echo "circuits destination must be an absolute non-symlink path" >&2
  exit 1
fi
if [[ -f "${circuits_dir}/VERSION" ]] &&
   [[ "$(<"${circuits_dir}/VERSION")" == "$circuits_version" ]]; then
  exit 0
fi
if [[ -e "$circuits_dir" ]]; then
  echo "refusing to replace an unexpected circuits destination: ${circuits_dir}" >&2
  exit 1
fi

scratch="$(mktemp -d "${TMPDIR:-/tmp}/lez-ci-circuits.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
archive="${scratch}/logos-blockchain-circuits-${circuits_version}-linux-x86_64.tar.gz"
curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
  --retry 3 --retry-all-errors --output "$archive" \
  "https://github.com/logos-blockchain/logos-blockchain-circuits/releases/download/${circuits_version}/logos-blockchain-circuits-${circuits_version}-linux-x86_64.tar.gz"
actual_sha256="$(sha256sum "$archive")"
actual_sha256="${actual_sha256%% *}"
[[ "$actual_sha256" == "$circuits_sha256" ]] || {
  echo "circuits archive checksum mismatch" >&2
  exit 1
}
mkdir -p "$circuits_dir"
tar -xzf "$archive" -C "$circuits_dir" --strip-components=1
[[ "$(<"${circuits_dir}/VERSION")" == "$circuits_version" ]] || {
  echo "installed circuits version differs from ${circuits_version}" >&2
  exit 1
}
