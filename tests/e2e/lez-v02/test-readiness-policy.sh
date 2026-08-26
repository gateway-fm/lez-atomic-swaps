#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../../.."

policy="tests/e2e/lez-v02/cryptarchia-advanced.jq"
if [[ ! -f "$policy" ]]; then
  echo "missing shared cryptarchia advancement policy: ${policy}" >&2
  exit 1
fi

scratch="$(mktemp -d "${TMPDIR:-/tmp}/lez-v02-readiness.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

printf '%s\n' '{"cryptarchia_info":{"height":7,"slot":11}}' >"${scratch}/same-1.json"
cp "${scratch}/same-1.json" "${scratch}/same-2.json"
if jq -e --slurp -f "$policy" "${scratch}/same-1.json" "${scratch}/same-2.json" >/dev/null; then
  echo "identical cryptarchia samples must not satisfy advancement" >&2
  exit 1
fi

printf '%s\n' '{"cryptarchia_info":{"height":7,"slot":12}}' >"${scratch}/slot-advanced.json"
jq -e --slurp -f "$policy" "${scratch}/same-1.json" "${scratch}/slot-advanced.json" >/dev/null

printf '%s\n' '{"cryptarchia_info":{"height":8,"slot":11}}' >"${scratch}/height-advanced.json"
jq -e --slurp -f "$policy" "${scratch}/same-1.json" "${scratch}/height-advanced.json" >/dev/null

printf '%s\n' '{"cryptarchia_info":{"height":6,"slot":12}}' >"${scratch}/height-regressed.json"
if jq -e --slurp -f "$policy" "${scratch}/same-1.json" "${scratch}/height-regressed.json" >/dev/null; then
  echo "advancement must not hide a regressed cryptarchia dimension" >&2
  exit 1
fi

echo "LEZ v0.2 cryptarchia advancement policy passed"
