#!/usr/bin/env bash
# Runs the format, lint and doc checks of every Cargo workspace in the
# repository, and the cheap repository linters, with the flags CI uses.
# Run it before every push. The lint jobs that open ci.yml run it with
# --rust-only: one --fast, the others --separate, sharded with --workspace
# (issue #171).
#
#   scripts/check-all.sh             every workspace: fmt, then clippy and doc
#                                    where CI or a verifier runs them
#   scripts/check-all.sh --fast      fmt of every workspace, clippy and doc of
#                                    the root workspace only
#   scripts/check-all.sh --separate  fmt of every workspace, clippy and doc of
#                                    the separate workspaces only (with --fast,
#                                    this splits a full run over two CI jobs)
#   scripts/check-all.sh --separate --workspace <manifest> [--workspace ...]
#                                    lint only the named separate workspaces
#                                    (the CI matrix shards them this way)
#   scripts/check-all.sh --rust-only skip the shell, workflow and Dockerfile
#                                    linters (CI runs them in its quality job)
#   scripts/check-all.sh --list-workspaces
#   scripts/check-all.sh --list-separate-lint (the ones CI must shard)
#
# The commands are those of the rust job in .github/workflows/ci.yml and of
# scripts/verify-lez-v02-provisional.sh, verify-lez-v02-sidecar.sh,
# verify-spel-compatibility.sh and run-lez-standalone-e2e.sh. Those still run
# their own checks; scripts/test-check-all-workspaces.sh fails when this list
# and the tree, or these commands and theirs, drift apart. Workspaces that
# embed a Risc0 guest are linted with RISC0_SKIP_BUILD=1: no guest build and
# no container. The pinned guest itself is built and checked by the slow jobs.
#
# Runs on Linux and macOS (bash 3.2 or later). Every check runs even after
# one fails; the summary lists what failed and what was skipped and why.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

usage() {
  sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed -e '$d' -e 's/^# \{0,1\}//'
}

mode=full
selected=()
rust_only=0
list_only=0
while (( $# > 0 )); do
  case "$1" in
    --fast) mode=fast ;;
    --separate) mode=separate ;;
    --workspace)
      (( $# > 1 )) || { echo "check-all: --workspace needs a manifest path" >&2; exit 2; }
      selected+=("$2")
      shift
      ;;
    --rust-only) rust_only=1 ;;
    --list-workspaces) list_only=1 ;;
    --list-separate-lint) list_only=2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "check-all: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

# Every Cargo workspace of the tree (third_party/ submodules excluded), and
# how clippy and doc run on it. test-check-all-workspaces.sh compares this
# list with the manifests it finds.
#   root         the rust job of ci.yml
#   provisional  scripts/verify-lez-v02-provisional.sh (root, guest, methods,
#                deployer)
#   sidecar      scripts/verify-lez-v02-sidecar.sh; needs RAPIDSNARK_LIB_DIR
#   spel-*       scripts/verify-spel-compatibility.sh and
#                scripts/run-lez-standalone-e2e.sh
#   fmt-only     no CI job or verifier lints it; fmt only
readonly workspaces=(
  "Cargo.toml|root"
  "fuzz/Cargo.toml|fmt-only"
  "compat/lez-v0.2-provisional/Cargo.toml|provisional-root"
  "compat/lez-v0.2-provisional/escrow/methods/guest/Cargo.toml|provisional-guest"
  "compat/lez-v0.2-provisional/escrow/methods/Cargo.toml|provisional-methods"
  "compat/lez-v0.2-provisional/escrow/deployer/Cargo.toml|provisional-deployer"
  "compat/lez-v0_2-sidecar/Cargo.toml|sidecar"
  "compat/spel-zec-escrow/Cargo.toml|spel-fixture"
  "compat/spel-zec-escrow/methods/guest/Cargo.toml|spel-guest"
  "compat/spel-zec-escrow/methods/Cargo.toml|spel-methods"
  "compat/lez-standalone-e2e/Cargo.toml|spel-standalone"
  "compat/lez-v0_1_2-sidecar/Cargo.toml|fmt-only"
  "compat/lez-v0_2-xmr-release-service/Cargo.toml|fmt-only"
  "compat/lez-v0.2-sequencer-reproducers/Cargo.toml|fmt-only"
  "compat/lez-v0.2-sequencer-reproducers/guest/Cargo.toml|fmt-only"
)

# XMR is parked until M4: the rust job of ci.yml leaves these crates out of
# its root clippy, test and doc runs.
readonly root_excludes=(
  lez-xmr-monero-adapter
  lez-xmr-release-authority
  xmr-reference-actor
  lez-xmr-swap-sdk
)

if (( list_only )); then
  for entry in "${workspaces[@]}"; do
    case "${entry#*|}" in
      root|fmt-only) (( list_only == 1 )) || continue ;;
    esac
    printf '%s\n' "${entry%%|*}"
  done
  exit 0
fi

command -v cargo >/dev/null || { echo "check-all: cargo is required" >&2; exit 1; }

repository_root="$(pwd -P)"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$( (getconf _NPROCESSORS_ONLN || sysctl -n hw.ncpu) 2>/dev/null || echo 2)}"
# The provisional verifier points bindgen at GCC's headers on Linux; so does
# this script, for the same graphs.
if [[ "$(uname -s)" == Linux ]] && command -v gcc >/dev/null; then
  BINDGEN_EXTRA_CLANG_ARGS="-I$(gcc -print-file-name=include)${BINDGEN_EXTRA_CLANG_ARGS:+ ${BINDGEN_EXTRA_CLANG_ARGS}}"
  export BINDGEN_EXTRA_CLANG_ARGS
fi
# The root workspace uses Cargo's own target directory; the separate
# workspaces share one, so their common LEZ and Risc0 graphs build once.
nested_target="${CHECK_ALL_TARGET_DIR:-${CARGO_TARGET_DIR:-${repository_root}/target}/check-all}"

failures=()
skips=()
timings=()
started=$SECONDS

note_skip() {
  skips+=("$1")
  printf 'check-all: skip: %s\n' "$1"
}

# run <label> <command...>: runs one check, records its time and result,
# and goes on after a failure.
run() {
  local label="$1"
  shift
  local step_started=$SECONDS
  printf '\ncheck-all: >> %s\n' "$label"
  if "$@"; then
    timings+=("$(printf '%4ss  ok    %s' "$((SECONDS - step_started))" "$label")")
  else
    timings+=("$(printf '%4ss  FAIL  %s' "$((SECONDS - step_started))" "$label")")
    failures+=("$label")
  fi
}

in_nested_target() {
  CARGO_TARGET_DIR="$nested_target" "$@"
}

skip_guest_build() {
  RISC0_SKIP_BUILD=1 CARGO_TARGET_DIR="$nested_target" "$@"
}

rustdoc_strict() {
  RUSTDOCFLAGS="-D warnings" "$@"
}

# bash -n reads only its first file argument; check each one.
bash_syntax() {
  local file status=0
  for file in "$@"; do
    bash -n "$file" || status=1
  done
  return "$status"
}

# --- repository linters (the quality job of ci.yml runs the pinned tools) ---

non_rust_checks() {
  # shellcheck source=scripts/ci-release-scope.sh
  source ./scripts/ci-release-scope.sh
  local quality_job="the 'shell, workflow, Dockerfile, and Compose quality' job of ci.yml"
  local shell_files=() dockerfiles=() workflow_files=() path

  while IFS= read -r -d '' path; do
    if is_release_scope_path "$path"; then
      shell_files+=("$path")
    fi
  done < <(git ls-files --cached --others --exclude-standard -z -- '*.sh')
  while IFS= read -r -d '' path; do
    case "${path##*/}" in
      Dockerfile|Dockerfile.*)
        if is_release_scope_path "$path"; then
          dockerfiles+=("$path")
        fi
        ;;
    esac
  done < <(git ls-files --cached --others --exclude-standard -z)
  while IFS= read -r -d '' path; do
    workflow_files+=("$path")
  done < <(git ls-files --cached --others --exclude-standard -z -- \
    '.github/workflows/*.yml' '.github/workflows/*.yaml')

  run "shell syntax (bash -n, ${#shell_files[@]} files)" bash_syntax "${shell_files[@]}"
  if command -v shellcheck >/dev/null; then
    run "shellcheck $(shellcheck --version | sed -n 's/^version: //p') (CI pins 0.11.0)" \
      shellcheck --severity=warning "${shell_files[@]}"
  else
    note_skip "shellcheck is not installed; ${quality_job} runs it"
  fi
  if command -v actionlint >/dev/null; then
    local actionlint_args=()
    if command -v shellcheck >/dev/null; then
      actionlint_args=(-shellcheck "$(command -v shellcheck)")
    fi
    run "actionlint $(actionlint -version | head -n 1) (CI pins 1.7.12)" \
      actionlint "${actionlint_args[@]+"${actionlint_args[@]}"}" "${workflow_files[@]}"
  else
    note_skip "actionlint is not installed; ${quality_job} runs it"
  fi
  if command -v hadolint >/dev/null; then
    run "hadolint (CI pins 2.14.0)" hadolint --failure-threshold warning "${dockerfiles[@]}"
  else
    note_skip "hadolint is not installed; ${quality_job} runs it"
  fi
  note_skip "docker compose config needs the CI environment; ${quality_job} runs it (scripts/run-ci-quality-gates.sh)"
  run "GitHub Action pins" ./scripts/check-github-action-pins.sh
  if command -v rg >/dev/null && command -v jq >/dev/null; then
    run "CI hardening policy" ./scripts/test-ci-hardening-policy.sh
  else
    note_skip "the CI hardening policy test needs rg and jq; ${quality_job} runs it"
  fi
}

# --- Rust ---

fmt_workspace() {
  local manifest="$1"
  if [[ "$manifest" == Cargo.toml ]]; then
    cargo fmt --all --check
  else
    cargo fmt --manifest-path "$manifest" --all -- --check
  fi
}

lint_workspace() {
  local manifest="$1"
  local kind="$2"
  case "$kind" in
    root)
      run "clippy ${manifest}" cargo clippy --locked --workspace --all-targets \
        "${root_exclude_args[@]}" -- -D warnings
      run "clippy ${manifest} (Node pair features)" cargo clippy --locked \
        -p lez-maker-node -p lez-taker-node --all-features --all-targets -- -D warnings
      run "doc ${manifest}" rustdoc_strict cargo doc --locked --workspace --no-deps \
        "${root_exclude_args[@]}"
      ;;
    provisional-root|spel-fixture)
      run "clippy ${manifest}" in_nested_target cargo clippy --locked \
        --manifest-path "$manifest" --all-targets -- -D warnings
      run "doc ${manifest}" rustdoc_strict in_nested_target cargo doc --locked \
        --manifest-path "$manifest" --no-deps
      ;;
    provisional-guest)
      run "clippy ${manifest}" in_nested_target cargo clippy --locked \
        --manifest-path "$manifest" --bins -- -D warnings
      run "doc ${manifest}" rustdoc_strict in_nested_target cargo doc --locked \
        --manifest-path "$manifest" --no-deps --bins
      ;;
    provisional-methods)
      run "clippy ${manifest} (RISC0_SKIP_BUILD=1)" skip_guest_build cargo clippy --locked \
        --manifest-path "$manifest" --all-targets -- -D warnings
      run "doc ${manifest} (RISC0_SKIP_BUILD=1)" rustdoc_strict skip_guest_build cargo doc \
        --locked --manifest-path "$manifest" --no-deps
      ;;
    provisional-deployer)
      run "clippy ${manifest} (RISC0_SKIP_BUILD=1)" skip_guest_build cargo clippy --locked \
        --manifest-path "$manifest" --all-targets -- -D warnings
      run "doc ${manifest} (RISC0_SKIP_BUILD=1)" rustdoc_strict skip_guest_build cargo doc \
        --locked --manifest-path "$manifest" --no-deps --bins
      ;;
    sidecar)
      if [[ -z "${RAPIDSNARK_LIB_DIR:-}" || ! -d "${RAPIDSNARK_LIB_DIR}" ]]; then
        note_skip "clippy and doc of ${manifest}: they link the checksum-pinned rapidsnark libraries (set RAPIDSNARK_LIB_DIR to run them); the lez-v02-sidecar job of ci.yml runs them (scripts/verify-lez-v02-sidecar.sh)"
        return
      fi
      run "clippy ${manifest}" in_nested_target cargo +1.96.0 clippy \
        --manifest-path "$manifest" --locked --all-targets --all-features -- -D warnings
      run "doc ${manifest}" in_nested_target cargo +1.96.0 rustdoc \
        --manifest-path "$manifest" --locked --all-features --lib -- -D warnings
      ;;
    spel-guest)
      run "clippy ${manifest}" in_nested_target cargo clippy --locked \
        --manifest-path "$manifest" -- -D warnings
      ;;
    spel-methods)
      run "clippy ${manifest} (RISC0_SKIP_BUILD=1)" skip_guest_build cargo clippy --locked \
        --manifest-path "$manifest" --all-targets -- -D warnings
      ;;
    spel-standalone)
      if [[ ! -d "${LOGOS_BLOCKCHAIN_CIRCUITS:-${HOME}/.logos-blockchain-circuits}" ]]; then
        note_skip "clippy of ${manifest}: its build scripts need the logos-blockchain-circuits release (set LOGOS_BLOCKCHAIN_CIRCUITS to run it; scripts/run-lez-standalone-e2e.sh downloads it, Linux only)"
        return
      fi
      run "clippy ${manifest}" in_nested_target cargo clippy --locked \
        --manifest-path "$manifest" --all-targets -- -D warnings
      ;;
    fmt-only)
      note_skip "clippy and doc of ${manifest}: no CI job or verifier lints it"
      ;;
    *)
      echo "check-all: unknown lint kind ${kind} for ${manifest}" >&2
      exit 1
      ;;
  esac
}

is_selected() {
  local candidate
  for candidate in "${selected[@]}"; do
    [[ "$candidate" == "$1" ]] && return 0
  done
  return 1
}

for manifest in "${selected[@]+"${selected[@]}"}"; do
  found_manifest=0
  for entry in "${workspaces[@]}"; do
    [[ "${entry%%|*}" == "$manifest" ]] && found_manifest=1
  done
  (( found_manifest )) || { echo "check-all: --workspace ${manifest} is not in the workspace list" >&2; exit 2; }
done
if (( ${#selected[@]} > 0 )); then
  note_skip "clippy and doc of the workspaces not named with --workspace"
fi

root_exclude_args=()
for crate in "${root_excludes[@]}"; do
  root_exclude_args+=(--exclude "$crate")
done

echo "check-all: ${mode} mode, $(cargo --version), CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}"
run "workspace list matches the tree" ./scripts/test-check-all-workspaces.sh

if (( rust_only )); then
  note_skip "shell, workflow and Dockerfile linters (--rust-only)"
else
  non_rust_checks
fi

for entry in "${workspaces[@]}"; do
  run "fmt ${entry%%|*}" fmt_workspace "${entry%%|*}"
done

for entry in "${workspaces[@]}"; do
  manifest="${entry%%|*}"
  kind="${entry#*|}"
  if [[ "$mode" == fast && "$kind" != root ]] || [[ "$mode" == separate && "$kind" == root ]]; then
    continue
  fi
  if (( ${#selected[@]} > 0 )) && ! is_selected "$manifest"; then
    continue
  fi
  lint_workspace "$manifest" "$kind"
done
if [[ "$mode" == fast ]]; then
  note_skip "clippy and doc of the separate workspaces (--fast; run without it before a push that touches compat/ or fuzz/)"
elif [[ "$mode" == separate ]]; then
  note_skip "clippy and doc of the root workspace (--separate)"
fi

printf '\ncheck-all: summary (%s mode, %ss)\n' "$mode" "$((SECONDS - started))"
printf '%s\n' "${timings[@]}"
if (( ${#skips[@]} > 0 )); then
  printf '\ncheck-all: skipped\n'
  printf '  - %s\n' "${skips[@]}"
fi
if (( ${#failures[@]} > 0 )); then
  printf '\ncheck-all: FAILED\n' >&2
  printf '  - %s\n' "${failures[@]}" >&2
  exit 1
fi
printf '\ncheck-all: passed\n'
