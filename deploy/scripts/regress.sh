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
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

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
  *) sed -n 2,28p "${BASH_SOURCE[0]}"; exit 64 ;;
esac

out="${LEZ_REGRESS_OUT:-runtime/regress/$(date -u +%Y%m%dT%H%M%SZ)}"
mkdir -p "$out/logs" "$out/e2e" "$out/zec"
flat_limit="${LEZ_REGRESS_FLAT_SECONDS:-60}"
tries="${LEZ_REGRESS_TRIES:-4}"
log() { echo "[$(date -u +%H:%M:%S)] $*" | tee -a "$out/regress.log"; }
finalized() {
  curl -s --max-time 10 -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' \
    http://127.0.0.1:8779/ 2>/dev/null | sed -n 's/.*"result":\([0-9][0-9]*\).*/\1/p'
}
load_env() { set -a; source runtime/runtime.env; set +a; }
# moving <seconds>: true as soon as the finalized height advances within that time
moving() {
  local first now deadline=$(( $(date +%s) + $1 ))
  first="$(finalized)"
  while (( $(date +%s) < deadline )); do
    sleep 10; now="$(finalized)"
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
  docker exec lez-bedrock bash -c 'exec 3<>/dev/tcp/127.0.0.1/18080 || exit 1
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
  setsid "$@" >"$logfile" 2>&1 </dev/null &
  local pid=$! last flat_since now
  last="$(finalized)"; flat_since="$(date +%s)"
  while kill -0 "$pid" 2>/dev/null; do
    for _ in 1 2 3 4 5 6; do sleep 5; kill -0 "$pid" 2>/dev/null || break 2; done
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
  local pair=(); [[ "$2" == zec ]] && pair=(--pair Zcash)
  if [[ "$1" == node ]]; then echo python3 scripts/node-e2e.py "${pair[@]}" --direction "$4" "$3"
  else echo bash scripts/ui-e2e.sh "$3" "${pair[@]}" --direction "$4"; fi
}
keep() { cp runtime/e2e/*.json "$out/e2e/" 2>/dev/null; cp runtime/evidence/zec/*.json "$out/zec/" 2>/dev/null; true; }

run_test() { # run_test <kind:pair:scenario:direction>
  local kind pair scenario direction try rc prepared started logfile verdict price
  IFS=: read -r kind pair scenario direction <<<"$1"
  [[ "$kind" =~ ^(node|ui)$ && "$pair" =~ ^(btc|zec)$ && -n "$scenario" && "$direction" =~ ^TakerSells(Lez|Foreign)$ ]] ||
    { log "=== $1: not a test (<node|ui>:<btc|zec>:<scenario>:<direction>)"; return 64; }
  use_profile "$(profile_of "$1")" || return 2
  for (( try = 1; try <= tries; try++ )); do
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
    # shellcheck disable=SC2046  # the command is a word list
    watched "$logfile" timeout -k 30 "${LEZ_REGRESS_TEST_TIMEOUT:-3600}" $(command_of "$kind" "$pair" "$scenario" "$direction"); rc=$?
    case "$rc" in 0) verdict=PASSED ;; 99) verdict=FROZEN ;; 124|137) verdict=TIMEOUT ;; *) verdict=FAILED ;; esac
    if [[ "$verdict" == FAILED || "$verdict" == TIMEOUT ]] && ! moving 30; then verdict=FROZEN; fi
    # what the harness spent before the scenario started (finality probe, a new
    # chain, reset-swaps.sh) apart from the scenario itself (its preflights included)
    log "=== $1 (try $try): $verdict in $(( $(date +%s) - started )) s (+$(( started - prepared )) s before it$([[ "$fresh" == 0 ]] || echo ', a new chain among them'))"
    printf '%s\t%s\t%s\t%s\t%s\n' "$1" "$try" "$verdict" "$(( $(date +%s) - started ))" "$(date -u +%FT%TZ)" >>"$out/results.tsv"
    keep
    [[ "$verdict" == PASSED ]] && return 0
    if [[ "$verdict" != FROZEN ]]; then
      sed 's/\x1b\[[0-9;]*m//g' "$logfile" | grep -E '✗|FAILED|Failure|Traceback|rror' | tail -3 | cut -c1-220 | sed 's/^/      /' | tee -a "$out/regress.log"
      return 1
    fi
    # a frozen chain never recovers: the next try starts on a new one (the refusal makes its own)
    [[ "$scenario" == zec-balance-refusal ]] || fresh_chain || return 2
  done
  return 1
}

load_env
[[ "${LEZ_TIMING_PROFILE:-local}" =~ ^(fast|refund)$ ]] || { echo "the regression needs the fast or the refund timing profile: LEZ_TIMING_PROFILE=fast scripts/up.sh" >&2; exit 1; }
began="$(date +%s)"; failed=()
log "regression ${mode}: ${#tests[@]} tests at $(git rev-parse --short HEAD 2>/dev/null || echo '?') (tree dirty: $(git status --short 2>/dev/null | wc -l)), results in $out"
log "profile $LEZ_TIMING_PROFILE: a Bitcoin block every ${MINE_INTERVAL:-?} s; Maker cutoff ${LEZ_BTC_MAKER_LOCK_CUTOFF_SECONDS} s, refunds at ${LEZ_BTC_EARLIER_REFUND_SECONDS} and ${LEZ_BTC_LATER_REFUND_SECONDS} s"
for t in "${tests[@]}"; do
  run_test "$t"; rc=$?
  if (( rc == 2 )); then log "no LEZ chain could be started; stopping"; failed+=("$t"); break; fi
  (( rc == 0 )) || failed+=("$t")
done
passed=$(( ${#tests[@]} - ${#failed[@]} ))
log "regression ${mode}: $passed of ${#tests[@]} passed in $(( ($(date +%s) - began) / 60 )) min, $chains new chains, $switches profile switches, $(awk -F'\t' '$3 == "FROZEN"' "$out/results.tsv" 2>/dev/null | wc -l) frozen tries"
for t in "${failed[@]}"; do log "  not passed: $t"; done
[[ ${#failed[@]} -eq 0 ]]
