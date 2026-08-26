#!/usr/bin/env bash
set -euo pipefail

readonly version="14.1.1"
readonly archive_sha256="4cf9f2741e6c465ffdb7c26f38056a59e2a2544b51f7cc128ef28337eeae4d8e"
readonly archive_name="ripgrep-14.1.1-x86_64-unknown-linux-musl.tar.gz"
readonly runner_temp="${RUNNER_TEMP:?RUNNER_TEMP is required}"
readonly github_path="${GITHUB_PATH:?GITHUB_PATH is required}"
readonly archive="${runner_temp}/${archive_name}"
readonly install_dir="${runner_temp}/ripgrep-${version}"

mkdir -p "$install_dir"
curl --fail --silent --show-error --location --retry 3 --retry-all-errors \
  --output "$archive" \
  "https://github.com/BurntSushi/ripgrep/releases/download/${version}/${archive_name}"
printf '%s  %s\n' "$archive_sha256" "$archive" | sha256sum --check --strict
tar -xzf "$archive" -C "$install_dir" --strip-components=1 \
  "ripgrep-${version}-x86_64-unknown-linux-musl/rg"
chmod 0555 "${install_dir}/rg"
printf '%s\n' "$install_dir" >>"$github_path"
