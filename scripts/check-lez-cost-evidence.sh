#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 EXPECTED.json ACTUAL.json" >&2
  exit 2
fi

expected="$1"
actual="$2"

jq -e . "$expected" >/dev/null
jq -e '
  .schema_version == 1 and
  (.measured_on | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}$")) and
  all(.operations[];
    (.sessions | length) > 0 and
    .recursive_total_cycles == ([.sessions[].total_cycles] | add) and
    .recursive_user_cycles == ([.sessions[].user_cycles] | add) and
    .recursive_user_cycles <= .recursive_user_cycle_budget and
    all(.sessions[];
      .segments == 1 and
      .total_cycles == (.user_cycles + .paging_cycles + .reserved_cycles)
    )
  )
' "$actual" >/dev/null

stable_projection='{
  schema_version,
  execution,
  operations: [.operations[] | {
    name,
    sessions: [.sessions[] | {
      position,
      role,
      segments,
      total_cycles
    }],
    recursive_total_cycles,
    recursive_user_cycle_budget
  }]
}'

diff -u \
  <(jq -S "$stable_projection" "$expected") \
  <(jq -S "$stable_projection" "$actual")
