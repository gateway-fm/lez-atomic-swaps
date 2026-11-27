#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly ci_workflow=".github/workflows/ci.yml"
readonly public_workflow=".github/workflows/public-readiness.yml"
readonly quality_runner="scripts/run-ci-quality-gates.sh"
readonly ripgrep_installer="scripts/install-ci-ripgrep.sh"
readonly provisional_verifier="scripts/verify-lez-v02-provisional.sh"
readonly provisional_methods_build="compat/lez-v0.2-provisional/escrow/methods/build.rs"
readonly provisional_artifact_manifest="compat/lez-v0.2-provisional/escrow/methods/guest/deployment-manifest.toml"
readonly core_runner="scripts/run-bitcoin-core-e2e.sh"
readonly core_evidence="docs/evidence/m3-bitcoin-core-smoke-a7393df-20260714.json"
readonly core_musig_evidence="docs/evidence/m3-bitcoin-core-musig2-f5a9caa-20260715.json"
readonly btc_sdk_manifest="crates/btc-swap-sdk/Cargo.toml"
readonly core_adapter_manifest="crates/btc-core-adapter/Cargo.toml"

fail() {
  echo "CI hardening policy: $*" >&2
  exit 1
}

require_fixed() {
  local needle="$1"
  local path="$2"
  rg -Fq -- "$needle" "$path" || fail "$path is missing: $needle"
}

workflow_step() {
  local name="$1"
  awk -v marker="      - name: ${name}" '
    $0 == marker { found = 1 }
    found && $0 != marker && $0 ~ /^      - / { exit }
    found { print }
    END { if (!found) exit 1 }
  ' "$ci_workflow"
}

assert_trivy_step() {
  local name="$1"
  local image="$2"
  local exit_code="$3"
  local step
  step="$(workflow_step "$name")" || fail "missing Trivy step: $name"
  for required in \
    'uses: aquasecurity/trivy-action@ed142fd0673e97e23eac54620cfb913e5ce36c25' \
    "image-ref: ${image}" \
    'format: table' \
    "exit-code: \"${exit_code}\"" \
    'ignore-unfixed: false' \
    'vuln-type: os,library' \
    'severity: HIGH,CRITICAL'; do
    rg -Fq -- "$required" <<<"$step" \
      || fail "Trivy step '$name' is missing: $required"
  done
  ! rg -Fq -- 'continue-on-error' <<<"$step" \
    || fail "Trivy step '$name' must not continue on error"
}

for required_path in \
  "$quality_runner" \
  "$ripgrep_installer" \
  "$provisional_verifier" \
  "$provisional_methods_build" \
  "$provisional_artifact_manifest" \
  "$core_runner" \
  "$core_evidence" \
  "$core_musig_evidence" \
  "$btc_sdk_manifest" \
  "$core_adapter_manifest"; do
  [[ -f "$required_path" ]] || fail "missing $required_path"
done
[[ -x "$quality_runner" && -x "$ripgrep_installer" && -x "$core_runner" ]] \
  || fail "CI runners and installers must remain executable"

for workflow in "$ci_workflow" "$public_workflow"; do
  [[ -f "$workflow" ]] || fail "missing $workflow"
  rg -F 'permissions:' "$workflow" >/dev/null || fail "$workflow lacks explicit permissions"
  rg -F 'contents: read' "$workflow" >/dev/null || fail "$workflow must default to read-only contents"
  ! rg -F 'pull_request_target:' "$workflow" >/dev/null \
    || fail "$workflow must not use pull_request_target"

  while IFS= read -r checkout_line; do
    sha="${checkout_line#*@}"
    sha="${sha%% *}"
    [[ "$sha" =~ ^[0-9a-f]{40}$ ]] \
      || fail "$workflow has a non-SHA checkout pin: $checkout_line"
  done < <(rg -o 'actions/checkout@[0-9a-f]+[^[:space:]]*([[:space:]]+#.*)?' "$workflow")

  checkout_count="$(rg -c 'uses: actions/checkout@' "$workflow")"
  credential_count="$(rg -c 'persist-credentials: false' "$workflow")"
  [[ "$checkout_count" == "$credential_count" ]] \
    || fail "$workflow must disable persisted credentials for every checkout"
done

./scripts/check-github-action-pins.sh

# Every commit on main and every tag gets a complete CI run (issue #171):
# only a pull request's newer push may cancel its older run, and outside pull
# requests the concurrency group names the commit.
for workflow in "$ci_workflow" "$public_workflow"; do
  require_fixed "cancel-in-progress: \${{ github.event_name == 'pull_request' }}" "$workflow"
  require_fixed "\${{ github.event_name != 'pull_request' && format('-{0}', github.sha) || '' }}" "$workflow"
  ! rg -Fq 'cancel-in-progress: true' "$workflow" \
    || fail "$workflow cancels runs outside pull requests"
done

# Release gate (issue #171): the first job of each release workflow checks
# that the commit has finished, green CI, and every other job waits for it.
for workflow in .github/workflows/release-images.yml .github/workflows/release-packages.yml; do
  require_fixed 'run: ./scripts/check-release-ci.sh' "$workflow"
  require_fixed 'GH_TOKEN: ${{ github.token }}' "$workflow"
  ungated="$(awk '
    /^jobs:/ { in_jobs = 1; next }
    in_jobs && /^  [A-Za-z0-9_-]+:$/ { flush(); job = substr($1, 1, length($1) - 1); has_needs = 0; next }
    in_jobs && /^    needs:/ { has_needs = 1 }
    function flush() { if (job != "" && job != "ci-gate" && !has_needs) print job }
    END { flush() }
  ' "$workflow")"
  [[ -z "$ungated" ]] || fail "$workflow jobs start without the release CI gate: ${ungated//$'\n'/ }"
  first_job="$(awk '/^jobs:/ { getline; sub(/:$/, ""); sub(/^ +/, ""); print; exit }' "$workflow")"
  [[ "$first_job" == ci-gate ]] || fail "$workflow must start with the ci-gate job"
done
require_fixed 'readonly gated_workflows=(ci.yml public-readiness.yml)' scripts/check-release-ci.sh

# Image tags (issue #191): only a run on a tag publishes under a release's
# name, and only under its own. The ci-gate job refuses any other tag before a
# build, the plan job decides the tag every push uses with the same script,
# and the free-text dispatch input never reaches a push as given.
readonly images_workflow=.github/workflows/release-images.yml
readonly image_tag_script=scripts/release-image-tag.sh
[[ -x "$image_tag_script" ]] || fail "$image_tag_script must exist and be executable"
images_gate="$(awk '/^  ci-gate:$/ { on = 1 } /^  plan:$/ { on = 0 } on' "$images_workflow")"
images_plan="$(awk '/^  plan:$/ { on = 1 } /^  rust:$/ { on = 0 } on' "$images_workflow")"
rg -Fq 'run: ./scripts/release-image-tag.sh' <<<"$images_gate" \
  || fail "the ci-gate job of $images_workflow must refuse a release's image tag on a branch"
rg -Fq 'IMAGE_TAG_INPUT: ${{ inputs.image_tag }}' <<<"$images_gate" \
  || fail "the ci-gate job of $images_workflow must check the dispatch's image_tag"
rg -Fq 'tag="$(./scripts/release-image-tag.sh)"' <<<"$images_plan" \
  || fail "the plan job of $images_workflow must take its tag from $image_tag_script"
[[ "$(rg -c 'inputs\.image_tag' "$images_workflow")" == 2 ]] \
  || fail "$images_workflow uses the image_tag input outside the two $image_tag_script steps"
require_fixed "readonly release_shaped='^v[0-9]+\.[0-9]+\.[0-9]+([-+.][0-9A-Za-z.+-]*)?\$'" "$image_tag_script"
require_fixed "readonly namespaced='^(sha|branch)-[A-Za-z0-9][A-Za-z0-9_.-]*\$'" "$image_tag_script"
require_fixed "name: \${{ github.ref_type == 'tag' && 'catalog-packages' || format('catalog-packages-test-{0}', github.sha) }}" \
  .github/workflows/release-packages.yml
# The release Taker image carries the test payer and no dealer tool (#190),
# and the smoke job runs a short regression from the bundle after start.sh.
require_fixed 'cp deploy/images/taker-node/zec-local-poc-identity deploy/images/taker-node/zec-local-poc-transfer _out/images/taker-node/' "$images_workflow"
! rg -q 'zec-local-poc-(provision|chat-draft|chat-finalize)|zec-local-poc-\*' "$images_workflow" \
  || fail "$images_workflow ships a dealer test fixture tool"
images_smoke="$(awk '/^  smoke:$/ { on = 1 } /^  publish:$/ { on = 0 } on' "$images_workflow")"
for required in './scripts/start.sh' 'LEZ_TIMING_PROFILE=fast SKIP_UI_VERIFY=1 LEZ_NO_BANNER=1 bash scripts/up.sh' \
  'bash scripts/regress.sh run' 'node:zec:zec-happy:TakerSellsLez node:zec:zec-happy:TakerSellsForeign' \
  'node:btc:regenerated-config:TakerSellsForeign node:btc:btc-custody-credit:TakerSellsForeign' \
  'name: bundle-regression-logs'; do
  rg -Fq -- "$required" <<<"$images_smoke" || fail "the smoke job of $images_workflow is missing: $required"
done
[[ "$(rg -n 'bash scripts/regress.sh run' <<<"$images_smoke" | cut -d: -f1)" -gt "$(rg -n './scripts/start.sh' <<<"$images_smoke" | cut -d: -f1)" ]] \
  || fail "the smoke job of $images_workflow must run the short regression after start.sh"
# and the rule holds when run: a fake gh knows the tags v0.3.6 and v0.4.0
image_tag_bin="$(mktemp -d)"
trap 'rm -rf "$image_tag_bin"' EXIT
cat >"$image_tag_bin/gh" <<'GH'
#!/usr/bin/env bash
case "$2" in
  */git/ref/tags/v0.3.6|*/git/ref/tags/v0.4.0) echo "refs/tags/${2##*/git/ref/tags/}" ;;
  *) echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
esac
GH
chmod +x "$image_tag_bin/gh"
image_tag() { # image_tag <ref type> <ref name> <input>: the tag, or nothing when refused
  PATH="$image_tag_bin:$PATH" GITHUB_EVENT_NAME=workflow_dispatch GITHUB_REF_TYPE="$1" GITHUB_REF_NAME="$2" \
    GITHUB_SHA=957c80160000000000000000000000000000beef GITHUB_REPOSITORY=gateway-fm/lez-atomic-swaps \
    IMAGE_TAG_INPUT="$3" "$image_tag_script" 2>/dev/null || true
}
[[ "$(image_tag tag v0.4.0 '')" == v0.4.0 ]] || fail "$image_tag_script: a run on v0.4.0 must publish under v0.4.0"
[[ "$(image_tag branch main '')" == sha-957c80160000 ]] || fail "$image_tag_script: a branch must default to sha-<12>"
[[ "$(image_tag branch main branch-rehearsal)" == branch-rehearsal ]] || fail "$image_tag_script: a branch- tag must pass"
for refused in "branch main v0.3.4" "branch main v0.4.0" "branch main v9.9.9" "branch main latest" "tag v0.4.0 v0.3.6"; do
  read -r ref_type ref_name input <<<"$refused"
  [[ -z "$(image_tag "$ref_type" "$ref_name" "$input")" ]] \
    || fail "$image_tag_script lets a dispatch on $ref_type $ref_name publish under $input"
done

require_fixed 'tags: ["m*-complete*"]' "$ci_workflow"
require_fixed './scripts/run-ci-quality-gates.sh' "$ci_workflow"
require_fixed 'run: ./scripts/check-all.sh --fast --rust-only' "$ci_workflow"
require_fixed './scripts/check-all.sh --separate --rust-only "${args[@]}"' "$ci_workflow"
[[ "$(awk '/^jobs:/ { getline; while ($0 ~ /^ *#/) getline; sub(/:$/, ""); sub(/^ +/, ""); print; exit }' "$ci_workflow")" == lint ]] \
  || fail "$ci_workflow must open with the lint job"
require_fixed './scripts/test-spin-lock-remediation.sh' "$ci_workflow"
require_fixed './scripts/test-ci-cache-scripts.sh' "$ci_workflow"
require_fixed './scripts/check-spin-lock-remediation.sh' "$ci_workflow"
require_fixed './scripts/check-github-action-pins.sh' "$ci_workflow"
require_fixed './scripts/install-ci-ripgrep.sh' "$ci_workflow"
require_fixed 'ripgrep-14.1.1-x86_64-unknown-linux-musl.tar.gz' "$ripgrep_installer"
require_fixed '4cf9f2741e6c465ffdb7c26f38056a59e2a2544b51f7cc128ef28337eeae4d8e' "$ripgrep_installer"
require_fixed 'npm audit --audit-level=moderate' "$ci_workflow"
require_fixed 'npm run audit:licenses' "$ci_workflow"
require_fixed 'actions/setup-node@949feb2413d6458794dcd2491c4babbbce0c15c1' "$ci_workflow"
require_fixed 'node-version: 24.22.3' "$ci_workflow"
require_fixed 'gitleaks_8.30.1_linux_x64.tar.gz' "$public_workflow"
require_fixed '551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb' "$public_workflow"
require_fixed 'cargo-deny-0.20.2-x86_64-unknown-linux-musl.tar.gz' "$public_workflow"
require_fixed '9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f' "$public_workflow"
[[ "$(rg -Fc 'sha256sum --check --strict -' "$public_workflow")" == 2 ]] \
  || fail "$public_workflow must verify both downloaded archives before extraction"
require_fixed 'PUPPETEER_EXECUTABLE_PATH: /usr/bin/google-chrome' "$ci_workflow"
require_fixed 'PUPPETEER_SKIP_DOWNLOAD: "true"' "$ci_workflow"
require_fixed 'M6_UI_TEST_ROOT: ${{ runner.temp }}/lez-m6-ui-${{ github.run_id }}-${{ github.run_attempt }}' "$ci_workflow"
require_fixed 'npm run test:m6:prototype' "$ci_workflow"
require_fixed 'npm run test:m6:basecamp:contract' "$ci_workflow"
require_fixed 'Maker and Taker Basecamp integration' "$ci_workflow"
require_fixed 'bash /repo/scripts/test-m6-basecamp-integration.sh' "$ci_workflow"
# The per-role catalog flakes take the Chat revision the aggregate lock pins,
# so evaluating them asks the GitHub API for no tag.
[[ -x scripts/basecamp-chat-module-ref.sh ]] || fail "scripts/basecamp-chat-module-ref.sh must remain executable"
require_fixed '--override-input chat_module "$chat_module_ref"' scripts/test-m6-basecamp-integration.sh
require_fixed 'nixos/nix:2.30.2@sha256:7894650fb65234b35c80010e6ca44149b70a4a216118a6b7e5c6f6ae377c8d21' "$ci_workflow"
require_fixed 'cargo fmt --all --check' "$ci_workflow"
require_fixed 'cargo clippy --locked --workspace --all-targets' "$ci_workflow"
require_fixed 'cargo test --locked --workspace --all-targets' "$ci_workflow"
require_fixed 'cargo run --locked -p lez-xmr-swap-sdk --example full-lifecycle' "$ci_workflow"
require_fixed 'cargo run --locked -p xmr-reference-actor --example full-lifecycle' "$ci_workflow"
require_fixed 'cargo test --locked --manifest-path compat/lez-v0.2-provisional/Cargo.toml -p lez-zec-escrow-v02 --lib' "$ci_workflow"
require_fixed 'cargo clippy --locked -p zec-reference-actor --features dealer-fixture --all-targets -- -D warnings' "$ci_workflow"
# The test payer the release Taker image carries is linted and tested under
# the release mark, without the dealer fixture (#190).
require_fixed 'cargo clippy --locked -p zec-reference-actor --features test-payer --all-targets -- -D warnings' "$ci_workflow"
require_fixed 'cargo test --locked -p zec-reference-actor --features test-payer' "$ci_workflow"
require_fixed 'cargo doc --locked --workspace --no-deps' "$ci_workflow"

# Build caches (actions/cache restore and save, never the combined action,
# which would also save from pull requests). Only a run with CI_CACHE_SAVE
# true saves: a push to main, or a maintainer's dispatch with save-caches,
# whose caches GitHub scopes to the dispatched branch. A pull request, a
# fork's included, only restores. Every key names what it was built from;
# the steps below keep guest builds and the repository's own crates out of
# every Cargo cache, and scripts/ci-cache-gate.sh bounds every save.
readonly cache_restore='uses: actions/cache/restore@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0'
readonly cache_save='uses: actions/cache/save@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0'
readonly cargo_prune='scripts/ci-cargo-cache-prune.sh'
readonly cache_gate='scripts/ci-cache-gate.sh'
readonly nix_store_cache='scripts/ci-basecamp-nix-store-cache.sh'
readonly expected_cache_restores=6
readonly expected_cache_saves=5
# The sum of the caps passed to ci-cache-gate.sh, in MiB: what this
# workflow's caches may hold at once, beside the 1300 MiB other actions'
# caches hold, within the repository's 10 GB (the budget comment in ci.yml).
readonly cache_cap_budget_mib=6976
[[ -x "$cargo_prune" && -x "$nix_store_cache" && -x "$cache_gate" ]] || fail "cache scripts must remain executable"
for workflow in "$ci_workflow" "$public_workflow"; do
  ! rg -q 'uses: actions/cache@' "$workflow" \
    || fail "$workflow uses actions/cache, which saves from pull requests; use restore and save"
done
[[ "$(rg -Fc -- "$cache_restore" "$ci_workflow")" == "$expected_cache_restores" ]] \
  || fail "expected $expected_cache_restores pinned cache restores in $ci_workflow"
[[ "$(rg -Fc -- "$cache_save" "$ci_workflow")" == "$expected_cache_saves" ]] \
  || fail "expected $expected_cache_saves pinned cache saves in $ci_workflow"
[[ "$(rg -c 'uses: actions/cache/' "$ci_workflow")" == "$((expected_cache_restores + expected_cache_saves))" ]] \
  || fail "every cache step in $ci_workflow must use the pinned restore or save"
# Which runs save is decided once, at the top of the workflow; the dispatch
# input that opts in defaults to off.
require_fixed "  CI_CACHE_SAVE: \${{ (github.event_name == 'push' && github.ref == 'refs/heads/main') || (github.event_name == 'workflow_dispatch' && inputs.save-caches) }}" "$ci_workflow"
[[ "$(rg -c 'CI_CACHE_SAVE: ' "$ci_workflow")" == 1 ]] \
  || fail "$ci_workflow must set CI_CACHE_SAVE in exactly one place"
dispatch_block="$(awk '/^  workflow_dispatch:$/ { found = 1 } found && /^[a-z]/ { exit } found { print }' "$ci_workflow")"
for required in '      save-caches:' '        type: boolean' '        default: false'; do
  rg -Fxq -- "$required" <<<"$dispatch_block" \
    || fail "$ci_workflow must offer save-caches as a boolean dispatch input that defaults to off: $required"
done
! rg -q 'workflow_run:' "$ci_workflow" \
  || fail "$ci_workflow must not run on workflow_run, which saves with the default branch's scope"
# Every save runs only on a saving run, and only once its gate allowed it.
unguarded_saves="$(awk '
  function check() {
    if (step ~ /actions\/cache\/save@/ && step !~ /\n        if: (matrix\.cache && )?env\.CI_CACHE_SAVE == .true. && steps\.[a-z-]+-gate\.outputs\.save == .true.\n/) print step
  }
  /^      - / { check(); step = "" }
  { step = step $0 "\n" }
  END { check() }
' "$ci_workflow")"
[[ -z "$unguarded_saves" ]] || fail "a cache save runs without CI_CACHE_SAVE and its gate: ${unguarded_saves}"
require_fixed 'if [[ "${CI_CACHE_SAVE:-false}" != true ]]; then' "$cache_gate"
require_fixed 'if (( bytes > cap_bytes )); then' "$cache_gate"
# The pinned actions/cache compresses with zstdmt (zstd -T0, level 3, no
# --long); the gate must measure the same, or its sizes are not GitHub's.
require_fixed '| zstd -T0 -3 -q -c | wc -c >"$size_file"' "$cache_gate"
! rg -q -- '\| zstd .*--long' "$cache_gate" || fail "$cache_gate must compress as actions/cache does, without --long"
require_fixed 'echo "$line" >>"$GITHUB_STEP_SUMMARY"' "$cache_gate"
# One gate per save, each with a cap; together the caps fit the budget.
mapfile -t gate_caps < <(rg -o '\./scripts/ci-cache-gate\.sh \S+ [0-9]+ ' "$ci_workflow" | cut -d' ' -f3)
[[ "${#gate_caps[@]}" == "$expected_cache_saves" ]] \
  || fail "every cache save in $ci_workflow must be measured by ci-cache-gate.sh with a cap (found ${#gate_caps[@]})"
cap_sum=0
for cap in "${gate_caps[@]}"; do
  cap_sum=$((cap_sum + cap))
done
(( cap_sum <= cache_cap_budget_mib )) \
  || fail "the cache caps in $ci_workflow add up to ${cap_sum} MiB, above the ${cache_cap_budget_mib} MiB budget"
require_fixed "#   caps together                   ${cache_cap_budget_mib} MiB" "$ci_workflow"
# A cache's restore, gate and save name the same paths: one job variable
# for each Cargo and tools cache, one directory for the Nix store.
for paths in CARGO_CACHE_PATHS TOOLS_CACHE_PATHS; do
  gates="$(rg -c "ci-cache-gate\\.sh \\S+ [0-9]+ \"\\\$${paths}\"\$" "$ci_workflow" || echo 0)"
  [[ "$(rg -Fxc "          path: \${{ env.${paths} }}" "$ci_workflow")" == "$((gates * 2))" ]] \
    || fail "every cache on ${paths} must restore, measure and save the same paths"
done
# The lint shard restores the provisional job's cache read-only: same key,
# same paths, same debug-info setting.
[[ "$(rg -Fxc '          path: ${{ env.PROVISIONAL_CACHE_PATHS }}' "$ci_workflow")" == 1 ]] \
  || fail "only the provisional lint shard restores the provisional job's cache"
env_block() { # env_block NAME: the lines of a multi-line job variable
  awk -v name="$1" '
    $0 == "      " name ": |" { found = 1; next }
    found && /^        / { print; next }
    found { exit }
  ' "$ci_workflow"
}
[[ -n "$(env_block PROVISIONAL_CACHE_PATHS)" \
  && "$(env_block PROVISIONAL_CACHE_PATHS)" == "$(awk '/^  lez-v02-provisional:$/ { found = 1 } found' "$ci_workflow" \
    | awk '$0 == "      CARGO_CACHE_PATHS: |" { f = 1; next } f && /^        / { print; next } f { exit }')" ]] \
  || fail "the lint shard must restore the provisional cache with the provisional job's exact paths"
[[ "$(rg -Fc "key: lez-v02-provisional-\${{ runner.os }}-\${{ runner.arch }}-\${{ env.CARGO_PROFILE_DEV_DEBUG }}-rustc-" "$ci_workflow")" == 2 ]] \
  || fail "the lint shard must restore the provisional job's key"
! rg -q '^        /tmp/lez-v02-provisional-artifact-target' "$ci_workflow" \
  || fail "the provisional methods and deployer target is not cached"
[[ "$(rg -Fxc '          path: ${{ runner.temp }}/basecamp-nix-store/store' "$ci_workflow")" == 2 ]] \
  || fail "the Basecamp Nix store restore and save must name the same directory"
require_fixed './scripts/ci-cache-gate.sh basecamp-nix-store 384 "${RUNNER_TEMP}/basecamp-nix-store/store"' "$ci_workflow"
while IFS= read -r key; do
  [[ "$key" == *'steps.'*'.outputs.cache-primary-key }}' || "$key" == *'hashFiles('* ]] \
    || fail "cache key does not hash its inputs: $key"
  [[ "$key" == *'cache-primary-key'* || "$key" == *'runner.os'*'runner.arch'* ]] \
    || fail "cache key does not name the runner: $key"
done < <(rg '^ +key: ' "$ci_workflow")
[[ "$(rg -Fc '${{ env.CARGO_PROFILE_DEV_DEBUG }}-rustc-${{ steps.toolchain.outputs.cachekey }}-${{ hashFiles(' "$ci_workflow")" == 4 ]] \
  || fail "every Cargo cache key must name the debug-info setting and the rustc version and hash the lock files"
[[ "$(rg -c '^      CARGO_PROFILE_DEV_DEBUG: ' "$ci_workflow")" == 4 \
  && "$(rg -Fxc '      CARGO_PROFILE_DEV_DEBUG: "0"' "$ci_workflow")" == 3 \
  && "$(rg -Fxc '      CARGO_INCREMENTAL: "0"' "$ci_workflow")" == 4 ]] \
  || fail "every job with a Cargo cache must set its debug-info and incremental settings"
require_fixed "key: lez-v02-provisional-tools-\${{ runner.os }}-\${{ runner.arch }}-\${{ hashFiles('scripts/verify-lez-v02-provisional.sh') }}" "$ci_workflow"
require_fixed "hashFiles('apps/basecamp/flake.lock', 'apps/basecamp/flake.nix', 'apps/basecamp/*/flake.nix', 'scripts/ci-basecamp-nix-store-cache.sh')" "$ci_workflow"
! rg -q 'advisory-db|/bin$' "$ci_workflow" \
  || fail "the cargo-deny advisory database and Cargo bin directories are never cached"
# Each Cargo cache is pruned after its restore and before Cargo runs.
# The provisional root target is pruned three times: after the lint shard's
# restore, after the provisional job's, and before that job's save.
for target in 'target Cargo.toml|2' \
  '/tmp/lez-v02-sidecar-target compat/lez-v0_2-sidecar/Cargo.toml|2' \
  '/tmp/lez-v02-provisional-target compat/lez-v0.2-provisional/Cargo.toml|3' \
  '/tmp/lez-v02-provisional-guest-target compat/lez-v0.2-provisional/escrow/methods/guest/Cargo.toml|2'; do
  count="${target##*|}"
  target="${target%|*}"
  [[ "$(rg -Fc -- "./scripts/ci-cargo-cache-prune.sh ${target}" "$ci_workflow")" == "$count" ]] \
    || fail "$ci_workflow must prune ${target%% *} after the restore and before the save"
done
provisional_order="$(rg -n -F -e 'Restore the provisional LEZ v0.2 Cargo registry' \
  -e 'ci-cargo-cache-prune.sh /tmp/lez-v02-provisional-guest-target' \
  -e 'run: ./scripts/verify-lez-v02-provisional.sh' "$ci_workflow" | cut -d: -f1 | head -n 3 | paste -sd ' ' -)"
read -r restore_line prune_line verify_line <<<"$provisional_order"
(( restore_line < prune_line && prune_line < verify_line )) \
  || fail "the provisional guest builds must be pruned after the restore and before the verifier"
# actions/cache would still store a `!`-excluded directory below a cached
# one; what a cache must not hold is removed before the save instead.
! rg -q '^ +!/|^ +!~' "$ci_workflow" \
  || fail "$ci_workflow excludes a cache path with !, which actions/cache does not honour below a cached directory"
require_fixed 'actions/cache does not exclude' "$cache_gate"
require_fixed 'rm -rf -- "${LEZ_V02_TOOL_DIR}/cargo-home/registry" "${LEZ_V02_TOOL_DIR}/cargo-home/git"' "$ci_workflow"
require_fixed 'rm -rf -- "${target_dir}/riscv-guest" "${target_dir}/docker-guest-source" "${target_dir}/doc" \' "$cargo_prune"
require_fixed '-name riscv-guest' "$cargo_prune"
require_fixed "cargo metadata --no-deps --format-version 1 --manifest-path \"\$manifest\"" "$cargo_prune"
# The Nix archive never holds an output built from the repository, the
# tested ones included, and a failed export never fails the job.
require_fixed "bash /repo/scripts/ci-basecamp-nix-store-cache.sh import" "$ci_workflow"
require_fixed "bash /repo/scripts/ci-basecamp-nix-store-cache.sh export" "$ci_workflow"
require_fixed "BASECAMP_NIX_CACHE_SAVE: \${{ env.CI_CACHE_SAVE == 'true' && steps.basecamp-nix-cache.outputs.cache-hit != 'true' }}" "$ci_workflow"
require_fixed 'nix-store --query --outputs "${root_drvs[@]}" | sort -u >"$tested"' "$nix_store_cache"
require_fixed 'xargs -r nix-store --query --outputs <"$own" | sort -u >"$own_outputs"' "$nix_store_cache"
require_fixed 'if [[ -n "$(comm -12 "$closure" "$own_outputs")" ]]; then' "$nix_store_cache"
require_fixed 'if [[ -n "$(comm -12 "$closure" "$tested")" ]]; then' "$nix_store_cache"
require_fixed 'if ! bash "${BASH_SOURCE[0]}" export-now; then' "$nix_store_cache"

# GitHub fetches are authenticated with the job's own token, so the shared
# unauthenticated API budget cannot fail a run: rzup's release requests
# (rzup reads GITHUB_TOKEN) and Nix's github: inputs (NIX_CONFIG
# access-tokens). The token goes to the steps that need it, never to a job,
# and is never printed.
steps_lacking() { # steps_lacking <workflow> <ERE a step runs> <fixed line it needs>
  awk -v pattern="$2" -v needle="$3" '
    function check() { if (step ~ pattern && index(step, needle) == 0) print first }
    /^      - / { check(); step = ""; first = $0 }
    /^  [A-Za-z0-9_-]+:$/ { check(); step = ""; first = "" }
    { step = step $0 "\n" }
    END { check() }
  ' "$1"
}
readonly rzup_token_env='          RZUP_GITHUB_TOKEN: ${{ github.token }}'
readonly nix_token_env='          NIX_CONFIG: access-tokens = github.com=${{ github.token }}'
for workflow in .github/workflows/*.yml; do
  lacking="$(steps_lacking "$workflow" 'run: [.]/scripts/verify-lez-v02-provisional[.]sh|rzup install' "$rzup_token_env")"
  [[ -z "$lacking" ]] || fail "$workflow runs rzup without RZUP_GITHUB_TOKEN: ${lacking}"
  lacking="$(steps_lacking "$workflow" 'nixos/nix:[^\n]*\n[^\n]*bash|NIX_IMAGE" |from-scratch[.]sh [^\n]*--only (nix|build:[a-z-]*-module|build:role-node-modules)' "$nix_token_env")"
  [[ -z "$lacking" ]] || fail "$workflow evaluates flakes without NIX_CONFIG access-tokens: ${lacking}"
  ! rg -q -- '-e NIX_CONFIG=|--env NIX_CONFIG=|(echo|printf|tee)[^#]*(RZUP_GITHUB_TOKEN|NIX_CONFIG|github\.token)' "$workflow" \
    || fail "$workflow prints a token or puts it on a command line"
  ! rg -q '^ +GITHUB_TOKEN: \$\{\{ (github\.token|secrets\.GITHUB_TOKEN) \}\}$' "$workflow" \
    || fail "$workflow exposes GITHUB_TOKEN to a whole step; pass RZUP_GITHUB_TOKEN to rzup alone"
done
require_fixed '          --env NIX_CONFIG' "$ci_workflow"
[[ "$(rg -Fc -- '-e NIX_CONFIG' deploy/scripts/from-scratch.sh)" == 5 ]] \
  || fail "every Nix container of from-scratch.sh must receive NIX_CONFIG by name"
[[ "$(rg -Fc -- '-e LGX_FLAKE -e NIX_CONFIG "$NIX_IMAGE"' .github/workflows/release-packages.yml)" == 2 ]] \
  || fail "the release-packages lgx containers must receive NIX_CONFIG by name"
require_fixed 'rzup_github_token="${RZUP_GITHUB_TOKEN:-}"' "$provisional_verifier"
require_fixed 'unset RZUP_GITHUB_TOKEN' "$provisional_verifier"
require_fixed 'GITHUB_TOKEN="$rzup_github_token" "$rzup_bin" install "$@"' "$provisional_verifier"
[[ "$(rg -Fc '"$rzup_bin" install' "$provisional_verifier")" == 2 ]] \
  || fail "$provisional_verifier must install through rzup_install only"
! rg -q 'set -x|set -o xtrace|(echo|printf)[^#]*(rzup_github_token|RZUP_GITHUB_TOKEN|NIX_CONFIG)' \
  "$provisional_verifier" deploy/scripts/from-scratch.sh scripts/test-m6-basecamp-integration.sh "$nix_store_cache" \
  || fail "a script that receives a token traces or prints it"

readonly cargo_deny_action='uses: EmbarkStudios/cargo-deny-action@17aeb3c839e42274e2b8adf721c8c65edfdc74dd'
readonly cargo_deny_policy='advisories bans licenses sources'
readonly expected_cargo_deny_steps=10
cargo_deny_steps="$(rg -Fc -- "$cargo_deny_action" "$ci_workflow")"
cargo_deny_policy_steps="$(rg -Fc -- "$cargo_deny_policy" "$ci_workflow")"
[[ "$cargo_deny_steps" == "$expected_cargo_deny_steps" ]] \
  || fail "expected $expected_cargo_deny_steps pinned cargo-deny steps; found $cargo_deny_steps"
[[ "$cargo_deny_policy_steps" == "$expected_cargo_deny_steps" ]] \
  || fail "every cargo-deny step must enforce the complete policy"

for manifest in \
  compat/lez-v0_2-sidecar/Cargo.toml \
  compat/lez-v0.2-provisional/Cargo.toml \
  compat/lez-v0.2-provisional/escrow/methods/Cargo.toml \
  compat/lez-v0.2-provisional/escrow/methods/guest/Cargo.toml \
  compat/lez-v0.2-provisional/escrow/deployer/Cargo.toml \
  compat/lez-v0_2-xmr-release-service/Cargo.toml \
  fuzz/Cargo.toml; do
  require_fixed "manifest-path: $manifest" "$ci_workflow"
done

require_fixed 'actionlint_1.7.12_linux_amd64.tar.gz' "$quality_runner"
require_fixed '8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8' "$quality_runner"
require_fixed 'hadolint-linux-x86_64' "$quality_runner"
require_fixed '6bf226944684f56c84dd014e8b979d27425c0148f61b3bd99bcc6f39e9dc5a47' "$quality_runner"
require_fixed 'docker-compose-linux-x86_64' "$quality_runner"
require_fixed 'f9ebc6ebdb19d769b793c245a736caaeb198c62587f13b25c660c13b4987f959' "$quality_runner"
require_fixed 'shellcheck-v0.11.0.linux.x86_64.tar.gz' "$quality_runner"
require_fixed 'b7af85e41cc99489dcc21d66c6d5f3685138f06d34651e6d34b42ec6d54fe6f6' "$quality_runner"
require_fixed '"$shellcheck" --severity=warning' "$quality_runner"
require_fixed "git ls-files --cached --others --exclude-standard -z -- '*.sh'" "$quality_runner"
require_fixed './scripts/test-v0-1-1-release-media-contract.sh' "$quality_runner"
require_fixed './scripts/check-m3-cryptographic-vectors.sh' "$quality_runner"
require_fixed './scripts/test-bitcoin-testnet4-route-contract.sh' "$quality_runner"
require_fixed 'node ./scripts/check-m6-prototype-contract.mjs' "$quality_runner"
require_fixed 'node ./scripts/check-m6-basecamp-package-contract.mjs' "$quality_runner"
require_fixed './scripts/check-desk-views-generated.sh' "$quality_runner"
require_fixed 'git ls-files --cached --others --exclude-standard -z' "$quality_runner"
require_fixed 'config --quiet' "$quality_runner"
require_fixed 'BTC_RPC_PASSWORD="ci-quality-${RANDOM}-${RANDOM}"' "$quality_runner"
require_fixed 'LEZ_MARKET_ROOT=/tmp/lez-ci-quality-market' "$quality_runner"

if rg -F 'COPY --chmod=0555 zec-' deploy/images/maker-node/Dockerfile >/dev/null; then
  fail "the public BTC Basecamp image must not package future ZEC executables"
fi
if rg -F 'lez-taker-' deploy/images/maker-node/Dockerfile >/dev/null; then
  fail "the Maker image must not package Taker executables"
fi
if rg -F 'lez-maker-' deploy/images/taker-node/Dockerfile >/dev/null; then
  fail "the Taker image must not package Maker executables"
fi

require_fixed 'Verify M3 official cryptographic vectors' "$ci_workflow"
require_fixed 'cargo test --locked -p lez-btc-swap-sdk --test bip340_vectors --test bip327_vectors --test adaptor_vectors' "$ci_workflow"
require_fixed 'Verify Bitcoin Testnet4 route contract' "$ci_workflow"
require_fixed 'Fund, claim, and refund a real HTLC through Zebra consensus' "$ci_workflow"
require_fixed './scripts/run-zebra-e2e.sh' "$ci_workflow"
require_fixed 'k256 = { version = "=0.13.4", default-features = false, features = ["schnorr"] }' "$btc_sdk_manifest"
require_fixed 'jsonrpsee-http-client = { version = "=0.26.0", default-features = false, features = ["tls"] }' "$core_adapter_manifest"

# M2 is the ZEC leg, so the Zebra regtest gate is in scope and is required
# below rather than forbidden here. Monero and the M4/M5/M7 gates stay out.
for forbidden in \
  'm4-monero' \
  'monero-regtest' \
  'lez-v02-xmr-release-worker' \
  'coordinator-fuzz' \
  'test-m7-' \
  'test-m5-' \
  'test-m4-'; do
  ! rg -F "$forbidden" .github/workflows scripts/run-ci-quality-gates.sh >/dev/null \
    || fail "out-of-scope release gate remains: $forbidden"
done

# The one fuzz job: each target for a bounded time on a schedule or on demand, through the
# pinned-nightly container runner. A nightly toolchain never gates a pull request, so the
# workflow has no pull_request or push trigger and ci.yml never runs the fuzzer; the stable
# corpus replay tests are the per-change gate and the fuzz graph is audited with the others.
readonly fuzz_workflow=".github/workflows/fuzz.yml"
for required in \
  'workflow_dispatch:' \
  'schedule:' \
  'permissions:' \
  'contents: read' \
  'timeout-minutes: 45' \
  'target: [coordinator, coordinator_store, offer_announcement, chat_frame, zec_agreement_wire, xmr_reserve_wire]' \
  './scripts/run-fuzz.sh "${{ matrix.target }}" 120 ' \
  'persist-credentials: false' \
  'if: failure()'; do
  require_fixed "$required" "$fuzz_workflow"
done
! rg -q -e '^  (pull_request|pull_request_target|push):' -e 'secrets\.' -e 'write' "$fuzz_workflow" \
  || fail "$fuzz_workflow must stay scheduled, read-only and secret-free"
! rg -Fq -e 'run-fuzz.sh' -e 'cargo fuzz' -e 'nightly' "$ci_workflow" "$quality_runner" \
  || fail "the fuzzer must not run in the required CI workflow"

trivy_fail_hard_steps="$(rg -c 'exit-code: "1"' "$ci_workflow")"
(( trivy_fail_hard_steps >= 2 )) \
  || fail "runtime and Bitcoin image scans must fail hard"

rg -F './scripts/check-bitcoin-core-isolation.sh' "$ci_workflow" >/dev/null \
  || fail "Bitcoin isolation contract is not enforced"
rg -F './scripts/check-lez-v02-docker-isolation.sh' "$ci_workflow" >/dev/null \
  || fail "LEZ isolation contract is not enforced"

assert_trivy_step \
  'Scan repository-owned runtime base for high and critical vulnerabilities' \
  'cgr.dev/chainguard/glibc-dynamic:latest@sha256:5bb932663cff4fb276186720e0357af55c5e47f12e4d9f44ea0060fa3e7bca88' 1
assert_trivy_step \
  'Report pinned Risc0 guest builder high and critical vulnerabilities' \
  'risczero/risc0-guest-builder:r0.1.94.1@sha256:c2f63fdd720337c0727e05c5e1733083baba04c00a864a89b0e3f4f8d92617be' 0
assert_trivy_step \
  'Report high and critical vulnerabilities in exact Logos Bedrock dependency' \
  'ghcr.io/logos-blockchain/logos-blockchain@sha256:70539769f3a2a014b36e608f76264e5d2297f14ee3f7fabf4715f071c7bbf8e7' 0
assert_trivy_step \
  'Scan exact Bitcoin Core image for high and critical vulnerabilities' \
  'lez-atomic-swaps-bitcoin-core:github-btc-${{ github.run_id }}-${{ github.run_attempt }}' 1
assert_trivy_step \
  'Report exact Zebra image high and critical vulnerabilities' \
  'lez-atomic-swaps-zebra:github-zec-${{ github.run_id }}-${{ github.run_attempt }}' 0
assert_trivy_step \
  'Report exact Monero Stagenet image high and critical vulnerabilities' \
  'lez-atomic-swaps-monero-stagenet:github-xmr-stagenet-${{ github.run_id }}-${{ github.run_attempt }}' 0
report_only_scans="$(rg -Fc -- 'exit-code: "0"' "$ci_workflow")"
[[ "$report_only_scans" == 4 ]] \
  || fail "exactly four classified upstream scans may report without failing"

require_fixed 'isolated Bitcoin Core 31.1 Regtest MuSig2 adaptor P2TR spend' "$ci_workflow"
require_fixed 'BITCOIN_CORE_E2E_KEEP_RUNNING: "1"' "$ci_workflow"
require_fixed 'BITCOIN_CORE_E2E_REQUIRE_CLEAN: "1"' "$ci_workflow"
require_fixed 'readonly actor_allowlist="getblockchaininfo,getnetworkinfo,getblockhash,getblock,getblockheader,getrawtransaction,gettxout,gettxspendingprevout,getindexinfo,getmempoolinfo,getrawmempool,getmempoolentry,testmempoolaccept,sendrawtransaction"' "$core_runner"
require_fixed 'create_basic_credentials' "$core_runner"
require_fixed 'BITCOIN_CORE_MAKER_BASIC_CREDENTIALS=' "$core_runner"
require_fixed 'BITCOIN_CORE_TAKER_BASIC_CREDENTIALS=' "$core_runner"
require_fixed 'mapfile -t containers < <(docker container ls --all --quiet' "$ci_workflow"
require_fixed 'if (( ${#containers[@]} > 1 )); then' "$ci_workflow"
require_fixed 'docker container rm --force "${containers[0]}"' "$ci_workflow"
require_fixed 'org.logos-co.atomic-swaps.run=${RUN_ID}' "$ci_workflow"

jq -e '
  .schema_version == 1
  and .milestone == "M3"
  and .scope == "bitcoin_core_infrastructure_only"
  and .result == "passed"
  and .worktree_clean_before_run == true
  and .core.version == "31.1"
  and .chain.network == "regtest"
  and .isolation.rpc_publication == "dynamic_literal_loopback_only"
  and .isolation.p2p_port_published == false
  and .actor_rpc.credentials_distinct == true
  and .actor_rpc.plaintext_credentials_disclosed == false
  and .actor_rpc.crossed_credentials_http_401_count == 2
  and .external_dependencies.runtime_external_resources == []
  and .external_dependencies.public_rpc_used == false
  and .cleanup.exact_run_resources_absent == true
  and .cleanup.foreign_sentinel_survived_exact_cleanup == true
' "$core_evidence" >/dev/null || fail "retained Bitcoin Core evidence invariants failed"

jq -e '
  .schema_version == 1
  and .milestone == "M3"
  and .scope == "bitcoin_core_musig2_adaptor_p2tr_fixture"
  and .result == "passed"
  and .clean_worktree_required == true
  and .contract.signing_protocol == "BIP327_MUSIG2_SCHNORR_ADAPTOR"
  and .contract.signer_order == ["maker", "taker"]
  and .cooperative_key_path_claim.adaptor_presignature_bytes == 65
  and .cooperative_key_path_claim.adaptor_presignature_verified == true
  and .cooperative_key_path_claim.final_signature_verified_under_q == true
  and .cooperative_key_path_claim.extracted_point_matches == true
  and .security_claims.musig2_taproot_fixture_proven == true
  and .security_claims.adaptor_signature_fixture_proven == true
  and .security_claims.scalar_extraction_fixture_proven == true
  and .external_dependencies.runtime_external_resources == []
  and .cleanup.exact_run_resources_absent == true
  and .cleanup.foreign_sentinel_survived_exact_cleanup == true
' "$core_musig_evidence" >/dev/null || fail "retained Bitcoin Core MuSig2 evidence invariants failed"

require_fixed 'cargo check --locked --manifest-path "$guest_manifest" --bins' "$provisional_verifier"
require_fixed 'cargo clippy --locked --manifest-path "$guest_manifest" --bins -- -D warnings' "$provisional_verifier"
require_fixed 'risc0_rust_version="1.94.1"' "$provisional_verifier"
require_fixed 'risc0_guest_builder_tag="r0.1.94.1@sha256:c2f63fdd720337c0727e05c5e1733083baba04c00a864a89b0e3f4f8d92617be"' "$provisional_verifier"
require_fixed 'export RISC0_DOCKER_CONTAINER_TAG="$risc0_guest_builder_tag"' "$provisional_verifier"
require_fixed 'risc0_build::embed_methods_with_options' "$provisional_methods_build"
require_fixed 'r0.1.94.1@sha256:c2f63fdd720337c0727e05c5e1733083baba04c00a864a89b0e3f4f8d92617be' "$provisional_methods_build"
require_fixed '.root_dir("../..")' "$provisional_methods_build"
require_fixed 'expected_elf_sha256="3a016fd0059390384da2d6f86d366e6f2036b6ffdda3c8b5cd4a7d965bdc47f3"' "$provisional_verifier"
require_fixed 'expected_image_id="ca66e0dc4118597e0a9010670fd49aec923bba9c332e9e6a26d9208dbdf6af98"' "$provisional_verifier"
require_fixed 'elf_sha256 = "3a016fd0059390384da2d6f86d366e6f2036b6ffdda3c8b5cd4a7d965bdc47f3"' "$provisional_artifact_manifest"
require_fixed 'image_id = "ca66e0dc4118597e0a9010670fd49aec923bba9c332e9e6a26d9208dbdf6af98"' "$provisional_artifact_manifest"
if rg -Fq 'risc0_build::embed_methods();' "$provisional_methods_build"; then
  fail "v0.2 methods must embed the canonical Docker-built guest"
fi
require_fixed 'cargo risczero build --manifest-path escrow/methods/guest/Cargo.toml' "$provisional_verifier"
if rg -Fq 'cargo test --locked --manifest-path "$guest_manifest"' "$provisional_verifier"; then
  fail "zkVM guest verifier must not build a host unit-test harness"
fi

# RISC Zero's recursion circuit archive comes from this project's
# build-inputs-1 release, always checked against the SHA-256 the crate and
# LEZ's flake pin; RISC Zero's bucket is never fetched from.
readonly zkr_sha256='744b999f0a35b3c86753311c7efb2a0054be21727095cf105af6ee7d3f4d8849'
readonly zkr_url="https://github.com/gateway-fm/lez-atomic-swaps/releases/download/build-inputs-1/risc0-recursion-zkr-${zkr_sha256}.zip"
readonly builder_dockerfile="deploy/builder/Dockerfile"
require_fixed "readonly RISC0_RECURSION_ZKR_SHA256=${zkr_sha256}" deploy/scripts/from-scratch.sh
require_fixed "readonly RISC0_RECURSION_ZKR_URL=${zkr_url}" deploy/scripts/from-scratch.sh
require_fixed 'nix-store --check-validity \$zkr 2>/dev/null || nix store prefetch-file --name $RISC0_RECURSION_ZKR_SHA256.zip \\' deploy/scripts/from-scratch.sh
require_fixed '--expected-hash sha256:$RISC0_RECURSION_ZKR_SHA256 $RISC0_RECURSION_ZKR_URL' deploy/scripts/from-scratch.sh
require_fixed '--expected-hash "sha256:$zkr_sha256" "$(pin RISC0_RECURSION_ZKR_URL)"' deploy/scripts/build-darwin-packages.sh
require_fixed 'zkr_sha256="$(pin RISC0_RECURSION_ZKR_SHA256)"' deploy/scripts/build-darwin-packages.sh
[[ "$(rg -c '^FROM ' "$builder_dockerfile")" == 1 ]] && rg -q '^FROM ubuntu:24\.04@sha256:[0-9a-f]{64}$' "$builder_dockerfile" \
  || fail "$builder_dockerfile must pin its one base image by digest (the layer cache is keyed on it)"
require_fixed "ARG RISC0_RECURSION_ZKR_URL=${zkr_url}" "$builder_dockerfile"
require_fixed "ARG RISC0_RECURSION_ZKR_SHA256=${zkr_sha256}" "$builder_dockerfile"
require_fixed 'ENV RECURSION_SRC_PATH=/opt/lez-tools/risc0-recursion-zkr.zip' "$builder_dockerfile"
require_fixed 'echo "$RISC0_RECURSION_ZKR_SHA256  $RECURSION_SRC_PATH" | sha256sum -c --strict --quiet' "$builder_dockerfile"
# the archive is in place before the first Cargo build of the image
[[ "$(rg -n 'ENV RECURSION_SRC_PATH=' "$builder_dockerfile" | cut -d: -f1)" -lt "$(rg -n 'cargo \+1\.96\.0 install' "$builder_dockerfile" | head -1 | cut -d: -f1)" ]] \
  || fail "$builder_dockerfile must set RECURSION_SRC_PATH before it builds r0vm and cargo-risczero"
! rg -n 'risc0-artifacts\.s3' .github scripts deploy --glob '!scripts/test-ci-hardening-policy.sh' \
  || fail "a build fetches from RISC Zero's bucket; take the build-inputs-1 copy by SHA-256"

# The builder image is published for both Linux architectures as one
# multi-platform image, pinned by one digest that every consumer pulls.
readonly builder_workflow=".github/workflows/builder-image.yml"
require_fixed '          - arch: arm64' "$builder_workflow"
require_fixed '            runner: ubuntu-24.04-arm' "$builder_workflow"
require_fixed '          - arch: amd64' "$builder_workflow"
require_fixed '            runner: ubuntu-24.04' "$builder_workflow"
require_fixed 'docker buildx imagetools create -t "$tag" "$tag-arm64" "$tag-amd64"' "$builder_workflow"
require_fixed '[[ "$platforms" == "linux/amd64 linux/arm64" ]]' "$builder_workflow"
rg -qx 'ghcr\.io/gateway-fm/lez-atomic-swaps/lez-builder@sha256:[0-9a-f]{64}' deploy/builder/image.lock \
  && [[ "$(wc -l < deploy/builder/image.lock)" -eq 1 ]] \
  || fail "deploy/builder/image.lock must hold exactly one pinned lez-builder digest"
! rg -n 'docker build[^\n]*deploy/builder|-t lez-builder:local' .github/workflows --glob '!builder-image.yml' \
  || fail "a release workflow builds the builder image; pull the digest pinned in deploy/builder/image.lock"
# Both architectures build against their registry layer cache; only a push
# to main or a manual dispatch writes it, never a pull request.
require_fixed 'BUILDKIT_IMAGE: moby/buildkit:v0.34.2@sha256:c5f8862f52499773af85bc07695b2379ef08e8002352b0df2c4864c91d78dabe' "$builder_workflow"
require_fixed 'docker buildx create --name lez-builder --driver docker-container --driver-opt "image=$BUILDKIT_IMAGE" --use --bootstrap' "$builder_workflow"
require_fixed 'cache="$IMAGE-cache:$ARCH"' "$builder_workflow"
require_fixed 'args=(--cache-from "type=registry,ref=$cache")' "$builder_workflow"
require_fixed 'args+=(--cache-to "type=registry,ref=$cache,mode=max,image-manifest=true,oci-mediatypes=true")' "$builder_workflow"
require_fixed "CACHE_WRITE: \${{ (github.event_name == 'push' && github.ref == 'refs/heads/main') || github.event_name == 'workflow_dispatch' }}" "$builder_workflow"
require_fixed 'if [[ "$CACHE_WRITE" == true ]]; then' "$builder_workflow"
require_fixed 'docker buildx build --builder lez-builder --platform "linux/$ARCH" "${args[@]}"' "$builder_workflow"
[[ "$(rg -c 'cache-to' "$builder_workflow")" == 1 ]] \
  || fail "$builder_workflow must write the layer cache from exactly one guarded place"
! rg -n '^\s*(run: )?docker build ' "$builder_workflow" \
  || fail "$builder_workflow builds without the registry layer cache"

if rg -Uq $'readonly REPOSITORY_ROOT\nREPOSITORY_ROOT=|readonly repository_root\nrepository_root=' scripts; then
  fail "a shell wrapper assigns a variable only after marking it readonly"
fi

echo "CI hardening policy passed for the M1/M3/M6 public release scope"
