#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
checker="${repo_root}/scripts/check-spin-lock-remediation.sh"

if [[ ! -x "$checker" ]]; then
  echo "missing executable spin lock remediation checker" >&2
  exit 1
fi

fixture="$(mktemp -d "${TMPDIR:-/tmp}/lez-spin-lock-policy.XXXXXX")"
trap 'rm -rf "$fixture"' EXIT

readonly -a guarded_paths=(
  Cargo.lock
  deny.toml
  compat/lez-standalone-e2e/Cargo.lock
  compat/lez-standalone-e2e/deny.toml
  compat/lez-v0.2-provisional/Cargo.lock
  compat/lez-v0.2-provisional/deny.toml
  compat/lez-v0.2-provisional/escrow/deployer/Cargo.lock
  compat/lez-v0.2-provisional/escrow/deployer/deny.toml
  compat/lez-v0.2-provisional/escrow/methods/Cargo.lock
  compat/lez-v0.2-provisional/escrow/methods/deny.toml
  compat/lez-v0.2-provisional/escrow/methods/guest/Cargo.lock
  compat/lez-v0.2-provisional/escrow/methods/guest/deny.toml
  compat/lez-v0_1_2-sidecar/Cargo.lock
  compat/lez-v0_1_2-sidecar/deny.toml
  compat/lez-v0_2-sidecar/Cargo.lock
  compat/lez-v0_2-sidecar/check-dependency-policy.sh
  compat/lez-v0_2-sidecar/deny.toml
  compat/spel-zec-escrow/Cargo.lock
  compat/spel-zec-escrow/deny.toml
  compat/spel-zec-escrow/methods/Cargo.lock
  compat/spel-zec-escrow/methods/guest/Cargo.lock
)

restore_fixture() {
  local path
  rm -rf "$fixture"
  mkdir -p "$fixture"
  for path in "${guarded_paths[@]}"; do
    mkdir -p "${fixture}/$(dirname "$path")"
    cp "${repo_root}/${path}" "${fixture}/${path}"
  done
}

expect_failure() {
  local expected="$1"
  local output
  if output="$(SPIN_POLICY_ROOT="$fixture" "$checker" 2>&1)"; then
    echo "spin lock checker unexpectedly accepted: ${expected}" >&2
    exit 1
  fi
  if [[ "$output" != *"$expected"* ]]; then
    echo "spin lock checker rejected for the wrong reason" >&2
    echo "expected: ${expected}" >&2
    echo "actual: ${output}" >&2
    exit 1
  fi
}

restore_fixture
SPIN_POLICY_ROOT="$fixture" "$checker"

perl -0pi -e 's/(name = "spin"\nversion = ")0\.9\.9/${1}0.9.8/' \
  "$fixture/compat/lez-v0.2-provisional/Cargo.lock"
expect_failure 'must resolve spin 0.9.9 exactly once'

restore_fixture
perl -0pi -e 's/yanked = "deny"/yanked = "allow"/' \
  "$fixture/compat/lez-v0.2-provisional/escrow/methods/deny.toml"
expect_failure 'must keep yanked = "deny"'

restore_fixture
printf '\nspin_features="$(dependency_features spin@0.9.9)"\n' >> \
  "$fixture/compat/lez-v0_2-sidecar/check-dependency-policy.sh"
expect_failure 'must not special-case spin'

echo "spin lock remediation regression tests passed"
