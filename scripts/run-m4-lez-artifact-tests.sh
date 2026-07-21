#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly risc0_version="3.0.5"
readonly risc0_rust_version="1.94.1"
readonly rzup_version="0.5.1"
readonly circuits_version="v0.4.2"
readonly circuits_sha256="e9131ffac8b08a80e1a7152b34fdd5d5c52674d4cb396e8162131ca5dd7c858d"
readonly builder_tag="r0.1.94.1@sha256:c2f63fdd720337c0727e05c5e1733083baba04c00a864a89b0e3f4f8d92617be"
readonly expected_elf_sha256="dc370bc34b432317730c51b49342760dbc675fca700e300b30b5fadefe5b7292"
readonly expected_image_id="4d6590332948743c2db88a183755815354ef92560550cd206ac27bddeea12c82"
readonly run_id="${RUN_ID:-local-$$}"
readonly manifest="compat/lez-v0.2-provisional/escrow/methods/guest/m4-deployment-manifest.toml"
readonly methods_manifest="compat/lez-v0.2-provisional/escrow/methods/Cargo.toml"
readonly artifact_root="${LEZ_M4_ARTIFACT_ROOT:-${TMPDIR:-/tmp}/lez-m4-artifact-${run_id}}"
readonly artifact_target="${artifact_root}/target"
readonly evidence_dir="${artifact_root}/evidence"
readonly tool_dir="${LEZ_M4_TOOL_DIR:-${TMPDIR:-/tmp}/lez-m4-risc0-tools-${run_id}}"
readonly keep_build="${LEZ_M4_KEEP_BUILD:-0}"
readonly risc0_home="${tool_dir}/home"
readonly isolated_cargo_home="${tool_dir}/cargo-home"
readonly rzup_bin="${tool_dir}/bin/rzup"
readonly r0vm_bin="${risc0_home}/extensions/v${risc0_version}-cargo-risczero-x86_64-unknown-linux-gnu/r0vm"
readonly circuits_dir="${LOGOS_BLOCKCHAIN_CIRCUITS:-${tool_dir}/logos-blockchain-circuits-${circuits_version}}"
if [[ -n "${LEZ_M4_TOOL_DIR:-}" ]]; then
  readonly tool_dir_is_run_owned=0
else
  readonly tool_dir_is_run_owned=1
fi
scratch=""

fail() {
  echo "M4 LEZ artifact test failed: $*" >&2
  exit 2
}

require_manifest_line() {
  rg -Fqx "$1" "$manifest" || fail "M4 artifact manifest mismatch: $1"
}

require_sha256() {
  local expected="$1" path="$2" actual
  [[ -f "$path" && ! -L "$path" ]] || fail "source identity is missing or unsafe: $path"
  actual="$(sha256sum "$path")"
  actual="${actual%% *}"
  [[ "$actual" == "$expected" ]] ||
    fail "source SHA-256 drift for $path: expected $expected, got $actual"
}

verify_source_boundary() {
  local runner_sha256

  require_sha256 "ee3bf98ee33f39071db3ae56c2efdc629370744274b6bdc4f79e11d3bfd56f34" \
    "compat/lez-v0.2-provisional/escrow/src/lib.rs"
  require_sha256 "ad2d8d2c16d7c785c813c11e8a4fa96ede4d94de8acbe2684779d1e8d1d3a412" \
    "compat/lez-v0.2-provisional/escrow/methods/guest/Cargo.toml"
  require_sha256 "dd8702e6e87e517a36f0c201e37bde351a924a88abcca903013b78e6ecc96868" \
    "compat/lez-v0.2-provisional/escrow/methods/guest/Cargo.lock"
  require_sha256 "8266638301f2179a808d0dd295e9c0e946e13ad12fbaf6a0321182299e1e49df" \
    "compat/lez-v0.2-provisional/escrow/methods/guest/src/bin/zec_escrow_v02.rs"
  require_sha256 "b837b231477f79d3a765d95d8760dc3466726183d2a8bd63e889f0d9aafe02f9" \
    "compat/lez-v0.2-provisional/escrow/methods/Cargo.toml"
  require_sha256 "9553fca19b9f62de15d9e860ca8934d43b4e698ed1c27bd5127ce8744d1e53ff" \
    "compat/lez-v0.2-provisional/escrow/methods/Cargo.lock"
  require_sha256 "ad63e5ee71b2173785a241e5f565313155b96c92e37e9d7ea6e42537f80e0ddc" \
    "compat/lez-v0.2-provisional/escrow/methods/build.rs"
  require_sha256 "0e127c21a387c17fa7f221cfbf4759c44d818786f5707c9051531ade1c515f27" \
    "compat/lez-v0.2-provisional/escrow/methods/tests/recursive_witnessed_claim.rs"
  require_sha256 "effccab44b7c5fe9a3e393478622bcdb48c934ae784d7c4d6c1364c1718a9cd0" \
    "compat/lez-v0.2-provisional/escrow/methods/guest/deployment-manifest.toml"
  require_sha256 "3023045ccd61c9e7a788a6c69e365b6158e7425a6c152ccc9a7aa01a2090f59c" \
    "scripts/run-m3-lez-bootstrap.sh"

  require_manifest_line 'artifact_status = "local-checked-artifact"'
  require_manifest_line 'public_deployment = false'
  require_manifest_line 'risc0_version = "3.0.5"'
  require_manifest_line 'risc0_rust_version = "1.94.1"'
  require_manifest_line 'rzup_version = "0.5.1"'
  require_manifest_line 'risc0_guest_builder = "risczero/risc0-guest-builder:r0.1.94.1@sha256:c2f63fdd720337c0727e05c5e1733083baba04c00a864a89b0e3f4f8d92617be"'
  require_manifest_line 'instruction_count = 18'
  require_manifest_line 'initialize_native_xmr_variant = 13'
  require_manifest_line 'authorize_native_xmr_claim_variant = 14'
  require_manifest_line 'claim_native_xmr_variant = 15'
  require_manifest_line 'refund_native_xmr_variant = 16'
  require_manifest_line 'punish_native_xmr_variant = 17'
  require_manifest_line 'deployment_manifest = "compat/lez-v0.2-provisional/escrow/methods/guest/deployment-manifest.toml"'
  require_manifest_line 'deployment_manifest_sha256 = "effccab44b7c5fe9a3e393478622bcdb48c934ae784d7c4d6c1364c1718a9cd0"'
  require_manifest_line 'm3_bootstrap = "scripts/run-m3-lez-bootstrap.sh"'
  require_manifest_line 'm3_bootstrap_sha256 = "3023045ccd61c9e7a788a6c69e365b6158e7425a6c152ccc9a7aa01a2090f59c"'
  require_manifest_line 'runtime_external_resources = []'
  require_manifest_line "elf_sha256 = \"${expected_elf_sha256}\""
  require_manifest_line "image_id = \"${expected_image_id}\""
  runner_sha256="$(sha256sum scripts/run-m4-lez-artifact-tests.sh)"
  runner_sha256="${runner_sha256%% *}"
  require_manifest_line "artifact_runner_sha256 = \"${runner_sha256}\""
}

if [[ ! "$run_id" =~ ^[a-z0-9][a-z0-9_-]*$ ]]; then
  fail "RUN_ID must contain only lowercase letters, numbers, underscores, or hyphens"
fi
[[ "$artifact_root" == /* && "$tool_dir" == /* && "$circuits_dir" == /* ]] ||
  fail "artifact, tool, and circuits paths must be absolute"
[[ "$keep_build" == 0 || "$keep_build" == 1 ]] ||
  fail "LEZ_M4_KEEP_BUILD must be 0 or 1"

for command_name in cargo cp curl cut docker du find gcc mkdir mktemp rg rm sha256sum tar tee; do
  command -v "$command_name" >/dev/null || fail "missing required tool: $command_name"
done

verify_source_boundary

if [[ "${1:-execute}" == "verify-source" ]]; then
  [[ "$#" -eq 1 ]] || fail "verify-source accepts no other arguments"
  echo "M4 LEZ source and manifest boundary passed"
  exit 0
fi
[[ "${1:-execute}" == "execute" && "$#" -le 1 ]] ||
  fail "expected execute or verify-source"

[[ ! -e "$artifact_root" && ! -L "$artifact_root" ]] ||
  fail "refusing to reuse artifact root: $artifact_root"
mkdir -m 0700 "$artifact_root"
mkdir -m 0700 "$artifact_target" "$evidence_dir"

cleanup_run_owned_paths() {
  local status="$?" target_kib=0 tool_kib=0
  trap - EXIT
  set +e
  if [[ -n "$scratch" && -d "$scratch" && ! -L "$scratch" ]]; then
    rm -rf -- "$scratch"
  fi
  if [[ "$keep_build" == 0 && -d "$artifact_target" && ! -L "$artifact_target" ]]; then
    target_kib="$(du -sk "$artifact_target" 2>/dev/null)"
    target_kib="${target_kib%%[[:space:]]*}"
    rm -rf -- "$artifact_target"
    echo "Removed exact run-owned Cargo target: ${artifact_target} (${target_kib:-0} KiB)"
  fi
  if [[ "$keep_build" == 0 && "$tool_dir_is_run_owned" == 1 &&
    -d "$tool_dir" && ! -L "$tool_dir" ]]; then
    tool_kib="$(du -sk "$tool_dir" 2>/dev/null)"
    tool_kib="${tool_kib%%[[:space:]]*}"
    rm -rf -- "$tool_dir"
    echo "Removed exact run-owned Risc0 tools: ${tool_dir} (${tool_kib:-0} KiB)"
  fi
  exit "$status"
}
trap cleanup_run_owned_paths EXIT

mkdir -p "${tool_dir}/bin" "$isolated_cargo_home"

if [[ ! -x "$rzup_bin" ]]; then
  [[ "$tool_dir_is_run_owned" == 1 ]] ||
    fail "explicit LEZ_M4_TOOL_DIR is missing pinned rzup: $rzup_bin"
  CARGO_HOME="$isolated_cargo_home" \
    cargo install rzup --version "$rzup_version" --locked --root "$tool_dir"
fi
[[ "$($rzup_bin --version)" == "rzup ${rzup_version}" ]] ||
  fail "expected rzup ${rzup_version} at $rzup_bin"

readonly rzup_show="$(RISC0_HOME="$risc0_home" "$rzup_bin" show)"
if ! grep -Fqx "* ${risc0_rust_version}" <<<"$rzup_show"; then
  [[ "$tool_dir_is_run_owned" == 1 ]] ||
    fail "explicit LEZ_M4_TOOL_DIR is missing Risc0 Rust ${risc0_rust_version}"
  RISC0_HOME="$risc0_home" CARGO_HOME="$isolated_cargo_home" \
    "$rzup_bin" install rust "$risc0_rust_version"
fi
if [[ ! -x "$r0vm_bin" ]]; then
  [[ "$tool_dir_is_run_owned" == 1 ]] ||
    fail "explicit LEZ_M4_TOOL_DIR is missing r0vm ${risc0_version}: $r0vm_bin"
  RISC0_HOME="$risc0_home" CARGO_HOME="$isolated_cargo_home" \
    "$rzup_bin" install r0vm "$risc0_version"
fi

export PATH="${isolated_cargo_home}/bin:${tool_dir}/bin:${PATH}"
export RISC0_HOME="$risc0_home"
export RISC0_SERVER_PATH="$r0vm_bin"
export RISC0_DOCKER_CONTAINER_TAG="$builder_tag"
export CARGO_BUILD_JOBS=2

[[ "$(cargo risczero --version)" == "cargo-risczero ${risc0_version}" ]] ||
  fail "cargo-risczero version drift"
[[ "$($r0vm_bin --version)" == "risc0-r0vm ${risc0_version}" ]] ||
  fail "r0vm version drift"

if [[ ! -f "${circuits_dir}/VERSION" ]] ||
  [[ "$(<"${circuits_dir}/VERSION")" != "$circuits_version" ]]; then
  scratch="$(mktemp -d "${TMPDIR:-/tmp}/lez-m4-circuits-${run_id}.XXXXXX")"
  archive="${scratch}/logos-blockchain-circuits-${circuits_version}-linux-x86_64.tar.gz"
  curl --fail --silent --show-error --location --retry 3 \
    --output "$archive" \
    "https://github.com/logos-blockchain/logos-blockchain-circuits/releases/download/${circuits_version}/logos-blockchain-circuits-${circuits_version}-linux-x86_64.tar.gz"
  printf '%s  %s\n' "$circuits_sha256" "$archive" | sha256sum --check --strict
  mkdir -p "$circuits_dir"
  tar -xzf "$archive" -C "$circuits_dir" --strip-components=1
fi
export LOGOS_BLOCKCHAIN_CIRCUITS="$circuits_dir"

gcc_include="$(gcc -print-file-name=include)"
export BINDGEN_EXTRA_CLANG_ARGS="-I${gcc_include}${BINDGEN_EXTRA_CLANG_ARGS:+ ${BINDGEN_EXTRA_CLANG_ARGS}}"

cargo fmt --manifest-path "$methods_manifest" -- --check
CARGO_TARGET_DIR="$artifact_target" \
  cargo test --locked --manifest-path "$methods_manifest" \
    --test recursive_witnessed_claim --no-run

mapfile -t guest_elfs < <(
  find "$artifact_target/riscv-guest" -type f -name 'zec_escrow_v02.bin' -print
)
[[ "${#guest_elfs[@]}" -eq 1 ]] ||
  fail "expected exactly one freshly embedded guest ELF, found ${#guest_elfs[@]}"
guest_elf="${guest_elfs[0]}"
actual_elf_sha256="$(sha256sum "$guest_elf")"
actual_elf_sha256="${actual_elf_sha256%% *}"
actual_image_id="$($r0vm_bin --elf "$guest_elf" --id)"
[[ "$actual_elf_sha256" == "$expected_elf_sha256" ]] ||
  fail "M4 guest ELF drift: expected $expected_elf_sha256, got $actual_elf_sha256"
[[ "$actual_image_id" == "$expected_image_id" ]] ||
  fail "M4 guest ImageID drift: expected $expected_image_id, got $actual_image_id"

mapfile -t test_binaries < <(
  find "$artifact_target/debug/deps" -maxdepth 1 -type f \
    -name 'recursive_witnessed_claim-*' -perm -u+x -print
)
[[ "${#test_binaries[@]}" -eq 1 ]] ||
  fail "expected exactly one recursive test binary, found ${#test_binaries[@]}"
"${test_binaries[0]}" --nocapture --test-threads=1 | tee "${evidence_dir}/test.log"

verify_source_boundary

checked_elf="${evidence_dir}/zec_escrow_v02_m4.bin"
cp -- "$guest_elf" "$checked_elf"
checked_elf_sha256="$(sha256sum "$checked_elf")"
checked_elf_sha256="${checked_elf_sha256%% *}"
checked_image_id="$($r0vm_bin --elf "$checked_elf" --id)"
[[ "$checked_elf_sha256" == "$actual_elf_sha256" ]] ||
  fail "evidence ELF copy SHA-256 mismatch"
[[ "$checked_image_id" == "$actual_image_id" ]] ||
  fail "evidence ELF copy ImageID mismatch"

cat >"${evidence_dir}/artifact.toml" <<EOF
format_version = 1
milestone = "M4"
run_id = "${run_id}"
elf_path = "${checked_elf}"
elf_sha256 = "${checked_elf_sha256}"
image_id = "${checked_image_id}"
test_target = "recursive_witnessed_claim"
test_count = 5
result = "passed"
runtime_external_resources = []
cold_setup_external_resources = [
  "https://github.com/logos-blockchain/logos-blockchain-circuits/releases/tag/v0.4.2",
  "https://crates.io and pinned Git dependency sources when Cargo caches are cold",
  "Docker registry for the digest-pinned risczero/risc0-guest-builder image when absent locally",
  "Risc0 release endpoints used by rzup when exact tools are absent",
]
cold_setup_network_observation = "not instrumented; availability depends on cache state"
cold_setup_flakiness = "Cold-cache DNS, registry, GitHub, crates.io, rate-limit, or availability failures can block setup; checked-test runtime uses no RPC, faucet, public chain, or other external resource."
EOF

echo "M4 LEZ artifact proof passed: elf_sha256=${actual_elf_sha256} image_id=${actual_image_id}"
echo "Run-owned evidence: ${evidence_dir}/artifact.toml"
