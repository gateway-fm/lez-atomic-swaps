#!/usr/bin/env bash
set -euo pipefail

repository_root="${SPIN_POLICY_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
readonly repository_root
readonly spin_version="0.9.9"
readonly spin_source="registry+https://github.com/rust-lang/crates.io-index"
readonly spin_checksum="3763264f6b73151db08c50ff20d7d8a0b8796e021cdea7ceedad07b80155fa0e"

readonly -a lockfiles=(
  Cargo.lock
  compat/lez-standalone-e2e/Cargo.lock
  compat/lez-v0.2-provisional/Cargo.lock
  compat/lez-v0.2-provisional/escrow/deployer/Cargo.lock
  compat/lez-v0.2-provisional/escrow/methods/Cargo.lock
  compat/lez-v0.2-provisional/escrow/methods/guest/Cargo.lock
  compat/lez-v0_1_2-sidecar/Cargo.lock
  compat/lez-v0_2-sidecar/Cargo.lock
  compat/spel-zec-escrow/Cargo.lock
  compat/spel-zec-escrow/methods/Cargo.lock
  compat/spel-zec-escrow/methods/guest/Cargo.lock
)

readonly -a policies=(
  deny.toml
  compat/lez-standalone-e2e/deny.toml
  compat/lez-v0.2-provisional/deny.toml
  compat/lez-v0.2-provisional/escrow/deployer/deny.toml
  compat/lez-v0.2-provisional/escrow/methods/deny.toml
  compat/lez-v0.2-provisional/escrow/methods/guest/deny.toml
  compat/lez-v0_1_2-sidecar/deny.toml
  compat/lez-v0_2-sidecar/deny.toml
  compat/spel-zec-escrow/deny.toml
)

fail() {
  echo "spin lock remediation policy failed: $*" >&2
  exit 1
}

for lockfile in "${lockfiles[@]}"; do
  path="${repository_root}/${lockfile}"
  [[ -f "$path" ]] || fail "missing ${lockfile}"

  if ! awk \
    -v expected_version="$spin_version" \
    -v expected_source="$spin_source" \
    -v expected_checksum="$spin_checksum" '
      BEGIN { RS = ""; FS = "\n"; matches = 0; valid = 0 }
      $1 == "[[package]]" {
        name = ""
        version = ""
        source = ""
        checksum = ""
        for (line = 2; line <= NF; line += 1) {
          if ($line ~ /^name = /) {
            name = $line
            sub(/^name = "/, "", name)
            sub(/"$/, "", name)
          } else if ($line ~ /^version = /) {
            version = $line
            sub(/^version = "/, "", version)
            sub(/"$/, "", version)
          } else if ($line ~ /^source = /) {
            source = $line
            sub(/^source = "/, "", source)
            sub(/"$/, "", source)
          } else if ($line ~ /^checksum = /) {
            checksum = $line
            sub(/^checksum = "/, "", checksum)
            sub(/"$/, "", checksum)
          }
        }
        if (name == "spin") {
          matches += 1
          if (version == expected_version && source == expected_source && checksum == expected_checksum) {
            valid += 1
          }
        }
      }
      END { exit !(matches == 1 && valid == 1) }
    ' "$path"; then
    fail "${lockfile} must resolve spin 0.9.9 exactly once with the audited crates.io checksum"
  fi
done

for policy in "${policies[@]}"; do
  path="${repository_root}/${policy}"
  [[ -f "$path" ]] || fail "missing ${policy}"
  grep -Fqx 'yanked = "deny"' "$path" || fail "${policy} must keep yanked = \"deny\""
  if grep -Eiq '(^|[^[:alnum:]])spin([@[:space:]_]|$)' "$path"; then
    fail "${policy} must not carry a spin package exception"
  fi
done

sidecar_policy="${repository_root}/compat/lez-v0_2-sidecar/check-dependency-policy.sh"
[[ -f "$sidecar_policy" ]] || fail "missing v0.2 sidecar dependency policy"
if grep -Eiq '(^|[^[:alnum:]])spin([@[:space:]_]|$)' "$sidecar_policy"; then
  fail "v0.2 sidecar dependency policy must not special-case spin"
fi

echo "all eleven Rust lockfiles retain the audited spin 0.9.9 remediation"
