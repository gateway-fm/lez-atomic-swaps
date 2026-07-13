#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workflow="${repo_root}/.github/workflows/ci.yml"
quality_runner="${repo_root}/scripts/run-ci-quality-gates.sh"
pin_checker="${repo_root}/scripts/check-github-action-pins.sh"

fail() {
  echo "CI hardening contract failed: $*" >&2
  exit 1
}

require_fixed() {
  local needle="$1"
  local path="$2"
  rg -Fq -- "$needle" "$path" || fail "${path#"${repo_root}/"} is missing: ${needle}"
}

[[ -f "$quality_runner" ]] || fail "missing scripts/run-ci-quality-gates.sh"
[[ -f "$pin_checker" ]] || fail "missing scripts/check-github-action-pins.sh"

require_fixed 'tags: ["m*-complete*"]' "$workflow"
require_fixed './scripts/run-ci-quality-gates.sh' "$workflow"
require_fixed './scripts/check-github-action-pins.sh' "$workflow"

require_fixed "git ls-files --cached --others --exclude-standard -z -- '*.sh'" "$quality_runner"
require_fixed 'actionlint_1.7.12_linux_amd64.tar.gz' "$quality_runner"
require_fixed '8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8' "$quality_runner"
require_fixed 'hadolint-linux-x86_64' "$quality_runner"
require_fixed '6bf226944684f56c84dd014e8b979d27425c0148f61b3bd99bcc6f39e9dc5a47' "$quality_runner"
require_fixed 'docker-compose-linux-x86_64' "$quality_runner"
require_fixed 'f9ebc6ebdb19d769b793c245a736caaeb198c62587f13b25c660c13b4987f959' "$quality_runner"
require_fixed 'shellcheck-v0.11.0.linux.x86_64.tar.gz' "$quality_runner"
require_fixed 'b7af85e41cc99489dcc21d66c6d5f3685138f06d34651e6d34b42ec6d54fe6f6' "$quality_runner"
require_fixed '"$shellcheck" --severity=warning' "$quality_runner"
require_fixed 'git ls-files --cached --others --exclude-standard -z' "$quality_runner"
require_fixed 'config --quiet' "$quality_runner"

require_fixed 'Scan repository-owned runtime base for high and critical vulnerabilities' "$workflow"
require_fixed 'gcr.io/distroless/cc-debian13:nonroot@sha256:aded2458d026e046cb68199db0e5793e1028ffa143f7258f3c4278253e20add7' "$workflow"
require_fixed 'Report high and critical vulnerabilities in exact Logos Bedrock dependency' "$workflow"
require_fixed 'ghcr.io/logos-blockchain/logos-blockchain@sha256:91d6c5bf07e07fcfba5e7cf07d21ee686a6bc4b9f6210f2d28bffbcad9a3729f' "$workflow"
require_fixed 'Repository-owned findings are fail-hard; Logos-owned findings remain visible' "$workflow"
require_fixed 'rapidsnark_root="${RAPIDSNARK_LIB_DIR%/rapidsnark-linux-x86_64-pic-v0.0.8/lib}"' "$workflow"
require_fixed 'unzip -q "${rapidsnark_archive}" -d "${rapidsnark_root}"' "$workflow"

if rg -Uq $'readonly REPOSITORY_ROOT\nREPOSITORY_ROOT=|readonly repository_root\nrepository_root=' \
    "${repo_root}/scripts"; then
  fail "a shell wrapper assigns a variable only after marking it readonly"
fi

"$pin_checker"

echo "CI hardening contract is complete"
