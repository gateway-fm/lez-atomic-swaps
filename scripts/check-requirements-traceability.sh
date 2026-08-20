#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

matrix=docs/requirements-traceability.md
required_ids=(
  F1 F2 F3 F4 F5 F6 F7 F8 F9
  U1 U2 U3 U4 U5 U6 U7 U8 U9 U10
  R1 R2 R3 R4 R5 R6 R7 R8
  P1
  S1 S2 S3 S4 S5 S6 S7 S8 S9 S10 S11 S12 S13
  D1
)

for requirement_id in "${required_ids[@]}"; do
  count="$(rg -c "^\\| ${requirement_id} " "$matrix" || true)"
  if [[ "$count" != "1" ]]; then
    echo "expected exactly one matrix row for ${requirement_id}; found ${count:-0}" >&2
    exit 1
  fi
done

actual_count="$(rg -c '^\| (F|U|R|P|S|D)[0-9]+ ' "$matrix")"
if [[ "$actual_count" != "${#required_ids[@]}" ]]; then
  echo "matrix has ${actual_count} requirement rows; expected ${#required_ids[@]}" >&2
  exit 1
fi
