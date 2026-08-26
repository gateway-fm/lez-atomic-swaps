#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly manifest="docs/milestone-7/hard-requirements.tsv"

fail() {
  echo "M7 hard-requirement audit failed: $*" >&2
  exit 1
}

[[ -s "$manifest" ]] || fail "missing non-empty ${manifest}"

readonly expected_header=$'id\tstate\tgate_script\tevidence\tremaining'
IFS= read -r header <"$manifest"
[[ "$header" == "$expected_header" ]] || fail "unexpected manifest header"

expected_ids=(
  F1 F2 F3 F4 F5 F6 F7 F8 F9
  U1 U2 U3 U4 U5 U6 U7 U8 U9 U10
  R1 R2 R3 R4 R5 R6 R7 R8
  P1
)

declare -A seen=()
open=0
deferred=0
rows=0
while IFS=$'\t' read -r id state gate_script evidence remaining extra; do
  [[ -n "$id" ]] || fail "blank row"
  [[ -z "${extra:-}" ]] || fail "${id} has extra columns"
  [[ -z "${seen[$id]:-}" ]] || fail "duplicate ${id}"
  seen[$id]=1
  ((rows += 1))

  case "$state" in
    green)
      [[ "$remaining" == "none" ]] || fail "${id} is green but has remaining work"
      ;;
    open)
      [[ -n "$remaining" && "$remaining" != "none" ]] \
        || fail "${id} is open without a concrete remaining item"
      ((open += 1))
      ;;
    policy-deferred|upstream-deferred)
      [[ -n "$remaining" && "$remaining" != "none" ]] \
        || fail "${id} is deferred without a concrete disclosure"
      ((deferred += 1))
      ;;
    *) fail "${id} has invalid state ${state}" ;;
  esac

  [[ "$gate_script" == ./scripts/* ]] || fail "${id} gate is not repository-owned"
  [[ "$gate_script" != *[[:space:]]* ]] || fail "${id} gate must be one executable path"
  [[ -x "${gate_script#./}" ]] || fail "${id} gate is missing or not executable: ${gate_script}"
  [[ "$evidence" == docs/* ]] || fail "${id} evidence is outside docs"
  [[ -s "$evidence" ]] || fail "${id} evidence is missing or empty: ${evidence}"
done < <(tail -n +2 "$manifest")

[[ "$rows" -eq "${#expected_ids[@]}" ]] \
  || fail "expected ${#expected_ids[@]} rows, found ${rows}"
for id in "${expected_ids[@]}"; do
  [[ -n "${seen[$id]:-}" ]] || fail "missing ${id}"
done

if [[ "${M7_REQUIRE_CLOSED:-0}" == 1 && "$open" -ne 0 ]]; then
  fail "strict closure requested with ${open} repository-owned requirements still open"
fi

echo "M7 hard-requirement inventory passed: ${rows} rows, ${open} open, ${deferred} deferred"
