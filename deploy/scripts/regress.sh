#!/usr/bin/env bash
# regress.sh — run the devnet regression, or a tier of it, and survive the LEZ
# devnet's finality freezes (README, "a frozen devnet") without a person watching.
#
#   scripts/regress.sh smoke             six happy swaps: both pairs, both directions, Node and desk
#   scripts/regress.sh refunds           every refund scenario
#   scripts/regress.sh full              the whole set (the M2 + M3 matrix, Node and desk)
#   scripts/regress.sh list <tier>       print the tier's tests, one per line
#   scripts/regress.sh run <test>...     the named tests, e.g.  ui:btc:happy:TakerSellsLez
#
# A test is <node|ui>:<btc|zec>:<scenario>:<direction>. Each one runs on a chain
# that is finalizing (a new one is made when it is not), after reset-swaps.sh.
# While it runs, the finalized height is read every 30 s; when it has stood
# still for LEZ_REGRESS_FLAT_SECONDS (60; it moves every ~2 s) the chain is frozen, the test is
# stopped at once, and it runs again on a new chain, LEZ_REGRESS_TRIES (4) times
# at most. A test that fails while finality moves has failed: it is not retried.
# A chain whose storage gas price has drifted to LEZ_REGRESS_MAX_STORAGE_PRICE
# (6000, about five days of chain time) is worn: the next test starts on a new
# chain before it can freeze.
#
# Results go to LEZ_REGRESS_OUT (runtime/regress/<UTC stamp>): results.tsv (one
# line per try), logs/ (each try's output), e2e/ and zec/ (the evidence the
# scenarios export). Exit status 0 when every test has a passing try.
#
# Needs the stack up on the fast or the refund timing profile
# (LEZ_TIMING_PROFILE=fast scripts/up.sh). The Bitcoin refund scenarios run on
# `refund`, every other Bitcoin test on `fast` (gen-config.sh); before a test
# that wants the other one, up.sh switches the profile (runtime.env, both Nodes
# and the miner recreated, the chain kept). A ZEC test runs on either: its
# windows come from the corridor's agreement.
#
# A test that exits 98 could not produce the situation it tests (node-e2e.py,
# NotExercised: zec-claim-collision's provoked nonce race went the other way).
# Its try is recorded as NOT-EXERCISED and it runs again on the same chain, up
# to LEZ_REGRESS_RACE_TRIES (8) times in all; it passes only when a try passes.
#
# LEZ_REGRESS_RECORD=1 records every desk test (ui-e2e.sh --record) and keeps
# the recordings in the run's videos/ directory.
#
# LEZ_LANES=N (N > 1) runs N tests at a time, one per lane: lane 1 is the stack,
# lanes 2..N each one more Maker/Taker pair with its own Bitcoin Core, miner and
# Zebra on the same LEZ chain (scripts/lane.sh, scripts/lanes.sh, which this
# brings up). Idle lanes take the longest test left (LEZ_REGRESS_DURATIONS, a
# results.tsv of an earlier run, says how long each took). One watcher reads
# the finalized height every LEZ_REGRESS_POLL_SECONDS (5); when the chain is
# frozen every lane's test is void: all are stopped, put back first in line,
# and one new chain serves every lane. A worn chain is replaced, and the timing
# profile switched, once every lane is idle. The zec-balance-refusal tests need
# a chain of their own and run last, on lane 1 alone. results.tsv gains the
# lane and the chain each try ran on. Unset or 1: one test at a time, as above.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1
# shellcheck source=host.sh
source scripts/host.sh
host_require_bash51 && host_require_timeout || exit
# The stack this directory runs (scripts/lane.sh): its LEZ network's container
# and its indexer on the host. The scripts each test runs take the rest of its
# names from lane.sh themselves.
stack="$(source scripts/lane.sh && echo "$LEZ_STACK_PREFIX $LEZ_STACK_INDEXER_URL")" || exit 64
read -r stack_prefix stack_indexer_url <<<"$stack"

directions=(TakerSellsLez TakerSellsForeign)
tier_tests() { # tier_tests <tier>
  local d s
  case "$1" in
    smoke)
      for d in "${directions[@]}"; do echo "node:btc:happy:$d"; echo "node:zec:zec-happy:$d"; done
      echo "ui:btc:happy:TakerSellsLez"; echo "ui:zec:zec-happy:TakerSellsForeign" ;;
    refunds)
      for d in "${directions[@]}"; do
        for s in zec-taker-refund zec-maker-refund zec-refund-then-maker-lock; do echo "node:zec:$s:$d"; done
        echo "ui:zec:zec-taker-refund:$d"
        for s in taker-refund maker-refund refund-then-maker-lock; do echo "node:btc:$s:$d"; done
        for s in taker-refund maker-refund; do echo "ui:btc:$s:$d"; done
      done
      echo "ui:zec:zec-maker-refund:TakerSellsLez"; echo "ui:zec:zec-maker-refund:TakerSellsForeign"
      echo "ui:zec:zec-stale-window:TakerSellsForeign" ;;
    full)
      tier_tests refunds
      for d in "${directions[@]}"; do
        for s in zec-happy zec-replay zec-wrong-inputs zec-restart-taker zec-restart-maker zec-survivor \
          zec-concurrent zec-regenerated-catalog zec-tampered-catalog zec-early-lock zec-offline-after-lock; do
          echo "node:zec:$s:$d"
        done
        for s in zec-happy zec-concurrent zec-restart-taker zec-restart-maker zec-survivor zec-node-offline; do
          echo "ui:zec:$s:$d"
        done
        for s in happy replay wrong-inputs restart-taker restart-maker survivor concurrent early-lock \
          offline-after-lock regenerated-config tampered-config; do
          echo "node:btc:$s:$d"
        done
        for s in happy restart-taker restart-maker survivor concurrent node-offline; do echo "ui:btc:$s:$d"; done
      done
      echo "node:zec:zec-claim-collision:TakerSellsForeign"
      echo "ui:zec:zec-concurrent-staggered:TakerSellsLez"
      # last: each makes its own chain, one no other test can run on
      for d in "${directions[@]}"; do echo "node:zec:zec-balance-refusal:$d"; done ;;
    *) echo "unknown tier: $1 (smoke, refunds, full)" >&2; return 64 ;;
  esac
}

mode="${1:-}"; shift || true
case "$mode" in
  smoke|refunds|full) mapfile -t tests < <(tier_tests "$mode") ;;
  list) tier_tests "${1:-full}"; exit ;;
  run) tests=("$@"); [[ ${#tests[@]} -gt 0 ]] || { echo "run needs at least one test" >&2; exit 64; } ;;
  *) sed -n 2,42p "${BASH_SOURCE[0]}"; exit 64 ;;
esac

out="${LEZ_REGRESS_OUT:-runtime/regress/$(date -u +%Y%m%dT%H%M%SZ)}"
mkdir -p "$out/logs" "$out/e2e" "$out/zec"
flat_limit="${LEZ_REGRESS_FLAT_SECONDS:-60}"
tries="${LEZ_REGRESS_TRIES:-4}"
# a test that could not produce its situation (exit 98) may need more goes than a frozen chain
race_tries="${LEZ_REGRESS_RACE_TRIES:-8}"
lane_count="${LEZ_LANES:-1}"
[[ "$lane_count" =~ ^[1-9]$ ]] || { echo "LEZ_LANES must be 1 to 9" >&2; exit 64; }
poll="${LEZ_REGRESS_POLL_SECONDS:-5}"
# The runner owns the chain: a ZEC desk test's zec-provision.sh must not
# recreate it from inside a test (ensure-chain.sh) behind the runner's back.
export LEZ_ZEC_SKIP_CHAIN_CHECK=1
log() { echo "[$(date -u +%H:%M:%S)] $*" | tee -a "$out/regress.log"; }
finalized() {
  curl -s --max-time 10 -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' \
    "$stack_indexer_url" 2>/dev/null | sed -n 's/.*"result":\([0-9][0-9]*\).*/\1/p'
}
load_env() { set -a; source runtime/runtime.env; set +a; }
# moving <seconds>: true as soon as the finalized height advances within that time
# (read every 2 s: a healthy chain finalizes a block every second or two)
moving() {
  local first now deadline=$(( $(date +%s) + $1 ))
  first="$(finalized)"
  while (( $(date +%s) < deadline )); do
    sleep 2; now="$(finalized)"
    [[ -n "$now" && -n "$first" && "$now" -gt "$first" ]] && return 0
    [[ -n "$first" ]] || first="$now"
  done
  return 1
}
# Bedrock's storage gas price drifts up at every epoch turn (~+45 an hour). Once
# one price step (an eighth of it) outgrows the sequencer's tip (gen-config.sh,
# priority_fee 10^7), an inscription in flight across a turn is dropped and the
# chain freezes. At 6000 a step still fits a 9 KB LEZ block, so a chain whose
# price has reached LEZ_REGRESS_MAX_STORAGE_PRICE is worn: the next test starts
# on a new chain instead.
max_storage_price="${LEZ_REGRESS_MAX_STORAGE_PRICE:-6000}"
storage_price() {
  docker exec "$stack_prefix-network" bash -c 'exec 3<>/dev/tcp/127.0.0.1/18080 || exit 1
    printf "GET /mantle/gas-prices HTTP/1.1\r\nHost: bedrock\r\nConnection: close\r\n\r\n" >&3
    timeout 5 cat <&3' 2>/dev/null | sed -n 's/.*"storage_gas_price":\([0-9][0-9]*\).*/\1/p'
}
chains=0; deferred_chain=0
fresh_chain() { # fresh_chain [ENV=VALUE...]: up.sh --fresh-lez checks the new chain finalizes itself
  chains=$((chains + 1))
  log "    new LEZ chain ($chains) $*"
  env "$@" SKIP_UI_VERIFY=1 LEZ_NO_BANNER=1 bash scripts/up.sh --fresh-lez >"$out/logs/chain-$chains.log" 2>&1 </dev/null ||
    { log "    up.sh --fresh-lez failed (logs/chain-$chains.log): $(grep -m1 -E 'does not finalize|bootstrap failed|never became healthy' "$out/logs/chain-$chains.log")"; return 1; }
  load_env
  fresh=1
}
# watched <log> <command...>: the command in its own process group; stopped with
# status 99 once the finalized height has stood still for $flat_limit seconds.
watched() {
  local logfile="$1"; shift
  host_setsid "$@" >"$logfile" 2>&1 </dev/null &
  local pid=$! last flat_since now
  last="$(finalized)"; flat_since="$(date +%s)"
  while kill -0 "$pid" 2>/dev/null; do
    # a second's granularity: the next test starts as soon as this one ends
    for _ in $(seq 30); do sleep 1; kill -0 "$pid" 2>/dev/null || break 2; done
    now="$(finalized)"
    if [[ -n "$now" && "$now" != "$last" ]]; then last="$now"; flat_since="$(date +%s)"; fi
    if (( $(date +%s) - flat_since >= flat_limit )); then
      log "    finality has stood at ${last:-nothing} for ${flat_limit} s: the chain is frozen, stopping the test"
      kill -TERM -- "-$pid" 2>/dev/null; sleep 20; kill -KILL -- "-$pid" 2>/dev/null
      wait "$pid" 2>/dev/null
      return 99
    fi
  done
  wait "$pid"
}
profile_of() { # profile_of <kind:pair:scenario:direction>: the timing profile the test needs, empty for either
  local kind pair scenario direction
  IFS=: read -r kind pair scenario direction <<<"$1"
  [[ "$pair" == btc ]] || return 0
  case "$scenario" in taker-refund|maker-refund|refund-then-maker-lock) echo refund ;; *) echo fast ;; esac
}
switches=0
use_profile() { # use_profile <profile>: switch the standing stack to it unless it runs on it already
  [[ -n "$1" && "$1" != "$LEZ_TIMING_PROFILE" ]] || return 0
  switches=$((switches + 1))
  log "    timing profile $LEZ_TIMING_PROFILE -> $1 (logs/profile-$switches.log)"
  if ! LEZ_TIMING_PROFILE="$1" SKIP_UI_VERIFY=1 LEZ_NO_BANNER=1 bash scripts/up.sh >"$out/logs/profile-$switches.log" 2>&1 </dev/null; then
    # up.sh on a frozen chain cannot finish; a new chain takes the profile too
    log "    up.sh did not switch the profile; a new chain on $1"
    fresh_chain LEZ_TIMING_PROFILE="$1" || return 1
  fi
  load_env
  [[ "$LEZ_TIMING_PROFILE" == "$1" ]] || { log "    runtime.env still names profile $LEZ_TIMING_PROFILE"; return 1; }
}
command_of() { # command_of <kind> <pair> <scenario> <direction>
  local pair=() record=(); [[ "$2" == zec ]] && pair=(--pair Zcash)
  [[ "${LEZ_REGRESS_RECORD:-0}" != 1 ]] || record=(--record)
  if [[ "$1" == node ]]; then echo python3 scripts/node-e2e.py "${pair[@]}" --direction "$4" "$3"
  else echo bash scripts/ui-e2e.sh "$3" "${pair[@]}" --direction "$4" "${record[@]}"; fi
}
# keep_video <scenario> <direction> <try> <verdict> <started>: with
# LEZ_REGRESS_RECORD=1 every desk test is recorded (ui-e2e.sh --record, on its
# lane's two Basecamp displays); the recording this try wrote is kept with the
# run, a try that did not pass under a name that says so.
keep_video() {
  [[ "${LEZ_REGRESS_RECORD:-0}" == 1 ]] || return 0
  local video="${LEZ_LANE_RUNTIME:-runtime}/evidence/videos/$2-$1.mp4" name="$2-$1"
  [[ -s "$video" && "$(host_mtime "$video")" -ge "$5" ]] || return 0
  [[ "$4" == PASSED ]] || name+="-try$3-$4"
  mkdir -p "$out/videos" && cp "$video" "$out/videos/$name.mp4"
}
keep() { cp runtime/e2e/*.json "$out/e2e/" 2>/dev/null; cp runtime/evidence/zec/*.json "$out/zec/" 2>/dev/null; true; }

run_test() { # run_test <kind:pair:scenario:direction>
  local kind pair scenario direction try rc prepared started logfile verdict price
  IFS=: read -r kind pair scenario direction <<<"$1"
  [[ "$kind" =~ ^(node|ui)$ && "$pair" =~ ^(btc|zec)$ && -n "$scenario" && "$direction" =~ ^TakerSells(Lez|Foreign)$ ]] ||
    { log "=== $1: not a test (<node|ui>:<btc|zec>:<scenario>:<direction>)"; return 64; }
  use_profile "$(profile_of "$1")" || return 2
  for (( try = 1; try <= (tries > race_tries ? tries : race_tries); try++ )); do
    fresh=0; prepared="$(date +%s)"
    if [[ "$scenario" == zec-balance-refusal ]]; then
      # needs a chain on which one wallet's vault claim was deferred: the wallet that would pay LEZ
      if [[ "$direction" == TakerSellsLez ]]; then fresh_chain LEZ_BOOTSTRAP_DEFER_CLAIM=taker-zurich-01 || return 2
      else fresh_chain LEZ_BOOTSTRAP_DEFER_CLAIM=maker-munich-01 || return 2; fi
      deferred_chain=1
    elif [[ "$deferred_chain" == 1 ]]; then
      # the last test's chain left one wallet unfunded
      fresh_chain || return 2; deferred_chain=0
    elif ! moving 30; then
      log "    finality is not moving before $1"; fresh_chain || return 2
    else
      price="$(storage_price)"
      if [[ -n "$price" ]] && (( price >= max_storage_price )); then
        log "    the chain is worn (storage gas price $price) before $1"; fresh_chain || return 2
      fi
    fi
    # a new chain's Nodes start without swaps (up.sh); otherwise forget the last test's
    [[ "$fresh" == 1 ]] || bash scripts/reset-swaps.sh --abandon-open >"$out/logs/reset.log" 2>&1 || log "    reset-swaps.sh failed before $1 (logs/reset.log)"
    logfile="$out/logs/$kind-$pair-$scenario-$direction-$try.log"; started="$(date +%s)"
    # The finality probe above and the watch below cover node-e2e.py's own
    # finality preflight (a 10 s sample when the heartbeat's status is stale).
    # shellcheck disable=SC2046  # the command is a word list
    watched "$logfile" env LEZ_E2E_SKIP_FINALITY_PREFLIGHT=1 timeout -k 30 "${LEZ_REGRESS_TEST_TIMEOUT:-3600}" \
      $(command_of "$kind" "$pair" "$scenario" "$direction"); rc=$?
    case "$rc" in 0) verdict=PASSED ;; 99) verdict=FROZEN ;; 98) verdict=NOT-EXERCISED ;; 124|137) verdict=TIMEOUT ;; *) verdict=FAILED ;; esac
    if [[ "$verdict" == FAILED || "$verdict" == TIMEOUT ]] && ! moving 30; then verdict=FROZEN; fi
    # what the harness spent before the scenario started (finality probe, a new
    # chain, reset-swaps.sh) apart from the scenario itself (its preflights included)
    log "=== $1 (try $try): $verdict in $(( $(date +%s) - started )) s (+$(( started - prepared )) s before it$([[ "$fresh" == 0 ]] || echo ', a new chain among them'))"
    printf '%s\t%s\t%s\t%s\t%s\n' "$1" "$try" "$verdict" "$(( $(date +%s) - started ))" "$(date -u +%FT%TZ)" >>"$out/results.tsv"
    keep
    [[ "$kind" != ui ]] || keep_video "$scenario" "$direction" "$try" "$verdict" "$started"
    [[ "$verdict" == PASSED ]] && return 0
    # the scenario's provoked race went the other way: the same chain, again
    if [[ "$verdict" == NOT-EXERCISED ]]; then (( try < race_tries )) && continue; return 1; fi
    if [[ "$verdict" != FROZEN ]]; then
      sed 's/\x1b\[[0-9;]*m//g' "$logfile" | grep -E '✗|FAILED|Failure|Traceback|rror' | tail -3 | cut -c1-220 | sed 's/^/      /' | tee -a "$out/regress.log"
      return 1
    fi
    (( try < tries )) || return 1
    # a frozen chain never recovers: the next try starts on a new one (the refusal makes its own)
    [[ "$scenario" == zec-balance-refusal ]] || fresh_chain || return 2
  done
  return 1
}

# ---- lanes (LEZ_LANES > 1) -------------------------------------------------------
# A test's expected length, to start the longest first: its last PASSED try in
# LEZ_REGRESS_DURATIONS when given, else its class's.
expected_seconds() { # expected_seconds <test>
  local seconds=""
  if [[ -n "${LEZ_REGRESS_DURATIONS:-}" && -s "${LEZ_REGRESS_DURATIONS}" ]]; then
    seconds="$(awk -F'\t' -v t="$1" '$1 == t && $3 == "PASSED" { s = $4 } END { print s }' "$LEZ_REGRESS_DURATIONS")"
  fi
  if [[ -z "$seconds" ]]; then
    case "$1" in
      *refund*|*stale-window*) seconds=1000 ;;
      ui:*) seconds=350 ;;
      *) seconds=250 ;;
    esac
  fi
  echo "$seconds"
}
# Tests that need a chain of their own: run last, on lane 1 alone (run_test).
serial_test() { [[ "$1" == *:zec-balance-refusal:* ]]; }
# A test's group is the timing profile it needs (profile_of; empty: either).
# Tests of one group run until none is left that may run, then every lane goes
# idle and enter_group switches the whole stack to the next profile (use_profile:
# up.sh recreates lane 1's Nodes, and lanes.sh every other lane's, LEZ_LANES
# being exported).
test_group() { profile_of "$1"; }
enter_group() { use_profile "$1"; }
lane_runtime() { if [[ "$1" == 1 ]]; then echo runtime; else echo "runtime/lanes/$1"; fi; }
keep_lane() { # keep_lane <lane>: the lane's exported summaries and evidence
  local dir; dir="$(lane_runtime "$1")"
  cp "$dir"/e2e/*.json "$out/e2e/" 2>/dev/null; cp "$dir"/evidence/zec/*.json "$out/zec/" 2>/dev/null; true
}
# chain_watch: returns once the finalized height has stood still for
# $flat_limit seconds (the chain is frozen); runs beside the lanes. A worn chain
# (storage_price) is marked in $out/worn: no test starts on it, and once every
# lane is idle a new chain replaces it.
chain_watch() {
  local last now since price
  last="$(finalized)"; since="$(date +%s)"
  while true; do
    sleep "$poll"; now="$(finalized)"
    if [[ -n "$now" && "$now" != "$last" ]]; then last="$now"; since="$(date +%s)"; fi
    if [[ ! -e "$out/worn" ]]; then
      price="$(storage_price)"
      if [[ -n "$price" ]] && (( price >= max_storage_price )); then
        log "    the chain is worn (storage gas price $price): no test starts on it"; touch "$out/worn"
      fi
    fi
    if (( $(date +%s) - since >= flat_limit )); then
      log "    finality has stood at ${last:-nothing} for ${flat_limit} s: the chain is frozen, every lane's test is void"
      return 0
    fi
  done
}
# lane_job <lane> <test> <try> <fresh>: one try on one lane, in the background;
# writes "<verdict>\t<seconds>" to $out/lanes/<lane>/verdict. The test's process
# group goes to .../pgid; a .../void marker means the runner stopped it (frozen).
lane_job() {
  local k="$1" t="$2" try="$3" fresh="$4" kind pair scenario direction dir="$out/lanes/$1" logfile started rc verdict
  IFS=: read -r kind pair scenario direction <<<"$t"
  export LEZ_LANE="$k" LEZ_REPAIR_INDEXER=0 LEZ_E2E_INDEXER_CHECK_ONLY=1
  # shellcheck source=lane.sh
  source scripts/lane.sh
  # a lane whose Nodes a new chain just recreated starts without swaps (up.sh)
  [[ "$fresh" == 1 ]] || bash scripts/reset-swaps.sh --abandon-open >"$dir/reset.log" 2>&1 ||
    log "    [lane $k] reset-swaps.sh failed before $t (lanes/$k/reset.log)"
  logfile="$out/logs/$kind-$pair-$scenario-$direction-$try.log"; started="$(date +%s)"
  # shellcheck disable=SC2046  # the command is a word list
  host_setsid env LEZ_E2E_SKIP_FINALITY_PREFLIGHT=1 timeout -k 30 "${LEZ_REGRESS_TEST_TIMEOUT:-3600}" \
    $(command_of "$kind" "$pair" "$scenario" "$direction") \
    >"$logfile" 2>&1 </dev/null &
  echo "$!" >"$dir/pgid"
  wait "$!"; rc=$?
  case "$rc" in 0) verdict=PASSED ;; 98) verdict=NOT-EXERCISED ;; 124|137) verdict=TIMEOUT ;; *) verdict=FAILED ;; esac
  if [[ -e "$dir/void" ]]; then verdict=FROZEN
  elif [[ "$verdict" != PASSED && "$verdict" != NOT-EXERCISED ]] && ! moving "${LEZ_REGRESS_MOVING_WINDOW:-30}"; then verdict=FROZEN; fi
  [[ "$kind" != ui ]] || keep_video "$scenario" "$direction" "$try" "$verdict" "$started"
  printf '%s\t%s\n' "$verdict" "$(( $(date +%s) - started ))" >"$dir/verdict"
}

lanes_main() {
  local -a queue=() serial=() requeue=()
  local -A busy=() busy_test=() busy_try=() tries_of=() fresh=()
  local k t try watcher done_pid verdict seconds group
  for t in "${tests[@]}"; do serial_test "$t" && serial+=("$t"); done
  # the longest first
  mapfile -t queue < <(
    for t in "${tests[@]}"; do
      serial_test "$t" || printf '%s\t%s\n' "$(expected_seconds "$t")" "$t"
    done | sort -t$'\t' -s -k1,1nr | cut -f2)
  export LEZ_LANES="$lane_count"
  for (( k = 1; k <= lane_count; k++ )); do mkdir -p "$out/lanes/$k"; fresh[$k]=0; done
  log "lanes: $lane_count (lane 1 is the stack; lanes 2..$lane_count: scripts/lanes.sh up)"
  bash scripts/lanes.sh up "$lane_count" >"$out/logs/lanes-up.log" 2>&1 ||
    { log "lanes 2..$lane_count did not come up (logs/lanes-up.log)"; failed=("${tests[@]}"); return 2; }
  rm -f "$out/worn"
  local price; price="$(storage_price)"
  if ! moving "${LEZ_REGRESS_MOVING_WINDOW:-30}" || { [[ -n "$price" ]] && (( price >= max_storage_price )); }; then
    log "    the chain is frozen or worn (storage gas price ${price:-?})"; fresh_chain || { failed=("${tests[@]}"); return 2; }
    for (( k = 1; k <= lane_count; k++ )); do fresh[$k]=1; done
  fi
  # a try is over: its verdict, its row, and what happens to the test
  finish_try() { # finish_try <lane>
    local k="$1" t="${busy_test[$1]}" try="${busy_try[$1]}" logfile kind pair scenario direction
    verdict=FAILED; seconds=0
    [[ ! -s "$out/lanes/$k/verdict" ]] || IFS=$'\t' read -r verdict seconds <"$out/lanes/$k/verdict"
    [[ -s "$out/lanes/$k/verdict" || ! -e "$out/lanes/$k/void" ]] || verdict=FROZEN
    log "=== $t (try $try, lane $k): $verdict in $seconds s"
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$t" "$try" "$verdict" "$seconds" "$(date -u +%FT%TZ)" "$k" "$chains" >>"$out/results.tsv"
    keep_lane "$k"
    unset 'busy[$k]' 'busy_test[$k]' 'busy_try[$k]'
    case "$verdict" in
      PASSED) ;;
      FROZEN)
        if (( try < tries )); then requeue+=("$t"); else failed+=("$t"); fi ;;
      NOT-EXERCISED)
        if (( try < race_tries )); then requeue+=("$t"); else failed+=("$t"); fi ;;
      *)
        IFS=: read -r kind pair scenario direction <<<"$t"
        logfile="$out/logs/$kind-$pair-$scenario-$direction-$try.log"
        sed 's/\x1b\[[0-9;]*m//g' "$logfile" 2>/dev/null | grep -E '✗|FAILED|Failure|Traceback|rror' | tail -3 | cut -c1-220 | sed 's/^/      /' | tee -a "$out/regress.log"
        failed+=("$t") ;;
    esac
  }
  # pick: the index of the first queued test that may run in the current group, or -1
  next_test() {
    local i g; pick=-1
    for i in "${!queue[@]}"; do
      g="$(test_group "${queue[$i]}")"
      [[ -z "$g" || "$g" == "$group" ]] && { pick="$i"; return; }
    done
  }
  local pick next=""
  # the group of the first (longest) test that names one, if any
  for t in "${queue[@]}"; do next="$(test_group "$t")"; [[ -z "$next" ]] || break; done
  group="$next"
  if [[ -n "$group" ]]; then
    log "group $group"; enter_group "$group" || { failed=("${queue[@]}" "${serial[@]}"); return 2; }
  fi
  chain_watch & watcher=$!
  while (( ${#queue[@]} > 0 || ${#busy[@]} > 0 )); do
    # a worn chain: once every lane is idle, a new one
    if [[ -e "$out/worn" ]] && (( ${#busy[@]} == 0 )); then
      kill "$watcher" 2>/dev/null; wait "$watcher" 2>/dev/null; rm -f "$out/worn"
      fresh_chain || { failed+=("${queue[@]}" "${serial[@]}"); queue=(); serial=(); log "no LEZ chain could be started; stopping"; return 2; }
      for (( k = 1; k <= lane_count; k++ )); do fresh[$k]=1; done
      chain_watch & watcher=$!
    fi
    # nothing left for this group: once every lane is idle, the stack moves on to
    # the group of the first test waiting
    next_test
    if (( pick < 0 && ${#busy[@]} == 0 )); then
      for t in "${queue[@]}"; do next="$(test_group "$t")"; [[ -z "$next" ]] || break; done
      group="$next"; log "group $group"
      enter_group "$group" || { failed+=("${queue[@]}"); queue=(); break; }
    fi
    for (( k = 1; k <= lane_count; k++ )); do
      [[ -z "${busy[$k]:-}" && ! -e "$out/worn" ]] || continue
      next_test; (( pick >= 0 )) || break
      t="${queue[$pick]}"; queue=("${queue[@]:0:pick}" "${queue[@]:pick+1}")
      try=$(( ${tries_of[$t]:-0} + 1 )); tries_of[$t]=$try
      rm -f "$out/lanes/$k/verdict" "$out/lanes/$k/void" "$out/lanes/$k/pgid"
      log "    [lane $k] $t (try $try)"
      lane_job "$k" "$t" "$try" "${fresh[$k]}" &
      busy[$k]=$!; busy_test[$k]="$t"; busy_try[$k]="$try"; fresh[$k]=0
    done
    if (( ${#busy[@]} == 0 )); then
      [[ -e "$out/worn" ]] || (( pick >= 0 )) || { failed+=("${queue[@]}"); queue=(); break; }
      continue
    fi
    wait -n -p done_pid
    if [[ "$done_pid" == "$watcher" ]]; then
      # frozen: every lane's test is void; they go back first in line
      for k in "${!busy[@]}"; do
        touch "$out/lanes/$k/void"
        [[ ! -s "$out/lanes/$k/pgid" ]] || kill -TERM -- "-$(cat "$out/lanes/$k/pgid")" 2>/dev/null
      done
      sleep "${LEZ_REGRESS_KILL_GRACE:-20}"
      for k in "${!busy[@]}"; do
        [[ ! -s "$out/lanes/$k/pgid" ]] || kill -KILL -- "-$(cat "$out/lanes/$k/pgid")" 2>/dev/null
        kill -KILL "${busy[$k]}" 2>/dev/null
        wait "${busy[$k]}" 2>/dev/null
        finish_try "$k"
      done
      queue=("${requeue[@]}" "${queue[@]}"); requeue=(); rm -f "$out/worn"
      fresh_chain || { failed+=("${queue[@]}" "${serial[@]}"); queue=(); serial=(); log "no LEZ chain could be started; stopping"; return 2; }
      for (( k = 1; k <= lane_count; k++ )); do fresh[$k]=1; done
      chain_watch & watcher=$!
      continue
    fi
    for k in "${!busy[@]}"; do
      [[ "${busy[$k]}" == "$done_pid" ]] || continue
      finish_try "$k"
      queue=("${requeue[@]}" "${queue[@]}"); requeue=()
    done
  done
  kill "$watcher" 2>/dev/null; wait "$watcher" 2>/dev/null
  (( ${#serial[@]} > 0 )) || return 0
  # the tests that need a chain of their own: lane 1 alone, one after another
  log "lanes: ${#serial[@]} test(s) on a chain of their own, lane 1 alone"
  bash scripts/lanes.sh stop "$lane_count" >"$out/logs/lanes-stop.log" 2>&1 || log "    lanes.sh stop failed (logs/lanes-stop.log)"
  export LEZ_LANES=1
  for t in "${serial[@]}"; do
    run_test "$t"; rc=$?
    if (( rc == 2 )); then log "no LEZ chain could be started; stopping"; failed+=("$t"); break; fi
    (( rc == 0 )) || failed+=("$t")
  done
}

load_env
[[ "${LEZ_TIMING_PROFILE:-local}" =~ ^(fast|refund)$ ]] || { echo "the regression needs the fast or the refund timing profile: LEZ_TIMING_PROFILE=fast scripts/up.sh" >&2; exit 1; }
began="$(date +%s)"; failed=()
log "regression ${mode}: ${#tests[@]} tests at $(git rev-parse --short HEAD 2>/dev/null || echo '?') (tree dirty: $(git status --short 2>/dev/null | wc -l)), results in $out"
log "profile $LEZ_TIMING_PROFILE: a Bitcoin block every ${MINE_INTERVAL:-?} s; Maker cutoff ${LEZ_BTC_MAKER_LOCK_CUTOFF_SECONDS} s, refunds at ${LEZ_BTC_EARLIER_REFUND_SECONDS} and ${LEZ_BTC_LATER_REFUND_SECONDS} s"
if (( lane_count > 1 )); then
  trap 'kill $(jobs -p) 2>/dev/null' EXIT
  lanes_main
else
for t in "${tests[@]}"; do
  run_test "$t"; rc=$?
  if (( rc == 2 )); then log "no LEZ chain could be started; stopping"; failed+=("$t"); break; fi
  (( rc == 0 )) || failed+=("$t")
done
fi
passed=$(( ${#tests[@]} - ${#failed[@]} ))
log "regression ${mode}: $passed of ${#tests[@]} passed in $(( ($(date +%s) - began) / 60 )) min, $chains new chains, $switches profile switches, $(awk -F'\t' '$3 == "FROZEN"' "$out/results.tsv" 2>/dev/null | wc -l) frozen tries"
for t in "${failed[@]}"; do log "  not passed: $t"; done
[[ ${#failed[@]} -eq 0 ]]
