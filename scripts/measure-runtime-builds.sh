#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

[[ "$(uname -s)" == Linux ]] || {
  echo "runtime build measurements are supported only on Linux" >&2
  exit 1
}
command -v /usr/bin/time >/dev/null || {
  echo "/usr/bin/time is required" >&2
  exit 1
}

readonly output="${1:-target/runtime-build-measurements.tsv}"
readonly measurement_root="${CARGO_TARGET_DIR:-target}/runtime-build-measurements"
mkdir -p "$(dirname "$output")" "$measurement_root"
printf 'schema_version\ttarget\tclean_elapsed_seconds\n' > "$output"

for target in lez-maker-node lez-taker-node lez-maker-cli lez-taker-cli \
  lez-maker-chat-gateway lez-taker-chat-gateway; do
  package=lez-maker-node
  [[ "$target" == lez-taker-* ]] && package=lez-taker-node
  target_dir="$measurement_root/$target"
  elapsed_file="$measurement_root/$target.elapsed"
  CARGO_TARGET_DIR="$target_dir" /usr/bin/time -f '%e' -o "$elapsed_file" \
    cargo check --locked -p "$package" --bin "$target"
  printf '1\t%s\t%s\n' "$target" "$(tr -d '[:space:]' < "$elapsed_file")" >> "$output"
done

printf 'runtime build measurements written to %s\n' "$output"
