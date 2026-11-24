#!/usr/bin/env bash
set -euo pipefail

# Decides whether a CI cache is saved, and records the outcome in the job
# summary, one line per cache.
#
#   ci-cache-gate.sh NAME CAP_MIB PATH...
#
# PATH is what the save step stores, written as actions/cache takes it: one
# path per argument or per line, `~/` for the home directory; no `!`
# exclusions (see below). CAP_MIB bounds what GitHub stores, the zstd archive that
# actions/cache uploads; this script measures it by compressing the same
# paths the same way (tar, then zstd -T0 --long=30 at level 3) before the
# save, on saving runs only.
#
# Environment:
#   CI_CACHE_SAVE         `true` on a run that saves caches (a push to main,
#                         or a dispatch with save-caches)
#   CI_CACHE_HIT          the restore step's cache-hit output
#   CI_CACHE_MATCHED_KEY  the restore step's cache-matched-key output
#   CI_CACHE_NOTE_FILE    optional: a file saying why there is nothing to save
#
# Writes `save=true` or `save=false` and `compressed-bytes` to GITHUB_OUTPUT.
# A cache above its cap is not saved and raises an error annotation; the job
# does not fail, because the release workflows require main's CI to be green.

if (( $# < 3 )); then
  echo "usage: $0 NAME CAP_MIB PATH..." >&2
  exit 2
fi
readonly name="$1"
readonly cap_mib="$2"
shift 2
[[ "$cap_mib" =~ ^[1-9][0-9]{0,5}$ ]] || {
  echo "cache cap must be a whole number of MiB: ${cap_mib}" >&2
  exit 2
}
readonly cap_bytes=$((cap_mib * 1024 * 1024))

cd "${GITHUB_WORKSPACE:-$PWD}"

includes=()
for argument in "$@"; do
  while IFS= read -r path; do
    path="${path#"${path%%[![:space:]]*}"}"
    path="${path%"${path##*[![:space:]]}"}"
    [[ -n "$path" ]] || continue
    # actions/cache matches `!` patterns against the paths it lists, not
    # against what tar then archives below a listed directory, so it would
    # store an excluded subdirectory anyway: remove it before the save.
    if [[ "$path" == '!'* ]]; then
      echo "${name}: actions/cache does not exclude ${path#!} below a cached directory; remove it before saving" >&2
      exit 2
    fi
    # shellcheck disable=SC2088 # the literal `~/` that actions/cache expands
    if [[ "$path" == '~/'* ]]; then
      path="${HOME}/${path#'~/'}"
    fi
    includes+=("$path")
  done <<<"$argument"
done
(( ${#includes[@]} > 0 )) || {
  echo "no cache paths given for ${name}" >&2
  exit 2
}

if [[ "${CI_CACHE_HIT:-}" == true ]]; then
  restored="restored (exact key)"
elif [[ -n "${CI_CACHE_MATCHED_KEY:-}" ]]; then
  restored="restored an older generation (${CI_CACHE_MATCHED_KEY})"
else
  restored="not restored (no cache for this key)"
fi

mib() {
  echo "$(( ($1 + 1024 * 1024 - 1) / (1024 * 1024) )) MiB"
}

# report SAVE OUTCOME
report() {
  local line="- Cache \`${name}\`: ${restored}; $2"
  echo "${line#- }"
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    echo "save=$1" >>"$GITHUB_OUTPUT"
  fi
  if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    echo "$line" >>"$GITHUB_STEP_SUMMARY"
  fi
}

if [[ "${CI_CACHE_SAVE:-false}" != true ]]; then
  report false "not saved: only a push to main or a dispatch with save-caches saves"
  exit 0
fi
if [[ "${CI_CACHE_HIT:-}" == true ]]; then
  report false "not saved: this key is stored already"
  exit 0
fi

existing=()
for path in "${includes[@]}"; do
  if [[ -e "$path" ]]; then
    existing+=("$path")
  fi
done
if (( ${#existing[@]} == 0 )); then
  reason="nothing to save"
  if [[ -n "${CI_CACHE_NOTE_FILE:-}" && -s "$CI_CACHE_NOTE_FILE" ]]; then
    reason="$(head -n 1 -- "$CI_CACHE_NOTE_FILE")"
  fi
  report false "not saved: ${reason}"
  exit 0
fi

command -v zstd >/dev/null || {
  echo "::error title=Cache not saved::${name}: zstd is missing, so the archive size cannot be measured"
  report false "not saved: zstd is missing, so the archive size cannot be measured"
  exit 0
}

size_file="$(mktemp)"
trap 'rm -f -- "$size_file"' EXIT
set +o pipefail
tar -c -f - -P -- "${existing[@]}" 2>/dev/null \
  | zstd -T0 --long=30 -3 -q -c | wc -c >"$size_file"
statuses=("${PIPESTATUS[@]}")
set -o pipefail
# GNU tar exits 1 when a file changed while it was read; the save would
# still store it.
if (( statuses[0] > 1 || statuses[1] != 0 )); then
  echo "::error title=Cache not saved::${name}: measuring the archive failed (tar ${statuses[0]}, zstd ${statuses[1]})"
  report false "not saved: measuring the archive failed"
  exit 0
fi
bytes="$(tr -d '[:space:]' <"$size_file")"
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  echo "compressed-bytes=${bytes}" >>"$GITHUB_OUTPUT"
fi

if (( bytes > cap_bytes )); then
  echo "::error title=Cache not saved::${name}: $(mib "$bytes") compressed, above its ${cap_mib} MiB cap"
  report false "not saved: $(mib "$bytes") compressed, above its ${cap_mib} MiB cap"
  exit 0
fi
report true "saving $(mib "$bytes") compressed (cap ${cap_mib} MiB)"
