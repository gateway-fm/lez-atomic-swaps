#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

root_manifest="compat/lez-v0.2-provisional/Cargo.toml"
methods_manifest="compat/lez-v0.2-provisional/escrow/methods/Cargo.toml"
guest_manifest="compat/lez-v0.2-provisional/escrow/methods/guest/Cargo.toml"
deployer_manifest="compat/lez-v0.2-provisional/escrow/deployer/Cargo.toml"
artifact_manifest="compat/lez-v0.2-provisional/escrow/methods/guest/deployment-manifest.toml"
spel_commit="df17acd98436be4f09c55877dae1fe2e73cbcdca"
lez_commit="a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
compat_test_sha256="e5320fc8a6172755cca312409e120ee4dd4837f21274e3be7f3f383006eb52d1"
risc0_version="3.0.5"
risc0_rust_version="1.94.1"
rzup_version="0.5.1"
circuits_version="v0.4.2"
circuits_sha256="e9131ffac8b08a80e1a7152b34fdd5d5c52674d4cb396e8162131ca5dd7c858d"
expected_elf_sha256="c85055f6fe85b71535a322ba84ffc612f5d093954a721ba3b529428814dc9d2e"
expected_image_id="5cf8c5a4eedb3c2873956cb7898eb33a495407c9746fb1a065c99638159329c1"
risc0_guest_builder_tag="r0.1.94.1@sha256:c2f63fdd720337c0727e05c5e1733083baba04c00a864a89b0e3f4f8d92617be"
risc0_guest_builder="risczero/risc0-guest-builder:${risc0_guest_builder_tag}"
run_id="${RUN_ID:-local-$$}"
if [[ ! "$run_id" =~ ^[a-z0-9][a-z0-9_-]*$ ]]; then
  echo "RUN_ID must contain only lowercase letters, numbers, underscores, or hyphens" >&2
  exit 1
fi

export CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${TMPDIR:-/tmp}/lez-v02-provisional-target-${run_id}}"
export LEZ_V02_TOOL_DIR="${LEZ_V02_TOOL_DIR:-${TMPDIR:-/tmp}/lez-v02-provisional-tools-${run_id}}"
guest_target="${LEZ_V02_GUEST_TARGET_DIR:-${TMPDIR:-/tmp}/lez-v02-provisional-guest-${run_id}}"
artifact_target="${LEZ_V02_ARTIFACT_TARGET_DIR:-${TMPDIR:-/tmp}/lez-v02-provisional-artifact-${run_id}}"
risc0_home="${LEZ_V02_TOOL_DIR}/home"
isolated_cargo_home="${LEZ_V02_TOOL_DIR}/cargo-home"
rzup_bin="${LEZ_V02_TOOL_DIR}/bin/rzup"
r0vm_bin="${risc0_home}/extensions/v${risc0_version}-cargo-risczero-x86_64-unknown-linux-gnu/r0vm"
circuits_dir="${LOGOS_BLOCKCHAIN_CIRCUITS:-${LEZ_V02_TOOL_DIR}/logos-blockchain-circuits-${circuits_version}}"
mkdir -p \
  "$CARGO_TARGET_DIR" \
  "$guest_target" \
  "$artifact_target" \
  "${LEZ_V02_TOOL_DIR}/bin" \
  "$isolated_cargo_home"

for command in cargo cp curl cut docker find gcc mktemp rg sha256sum tar; do
  command -v "$command" >/dev/null || {
    echo "${command} is required by the provisional LEZ v0.2 verification" >&2
    exit 1
  }
done

if [[ ! -x "$rzup_bin" ]]; then
  CARGO_HOME="$isolated_cargo_home" \
    cargo install rzup --version "$rzup_version" --locked --root "$LEZ_V02_TOOL_DIR"
fi
if [[ "$($rzup_bin --version)" != "rzup ${rzup_version}" ]]; then
  echo "expected rzup ${rzup_version} at ${rzup_bin}" >&2
  exit 1
fi

if ! RISC0_HOME="$risc0_home" "$rzup_bin" show | rg -q "rust.*${risc0_rust_version}"; then
  RISC0_HOME="$risc0_home" CARGO_HOME="$isolated_cargo_home" \
    "$rzup_bin" install rust "$risc0_rust_version"
fi
if [[ ! -x "$r0vm_bin" ]]; then
  RISC0_HOME="$risc0_home" CARGO_HOME="$isolated_cargo_home" \
    "$rzup_bin" install r0vm "$risc0_version"
fi

export PATH="${isolated_cargo_home}/bin:${LEZ_V02_TOOL_DIR}/bin:${PATH}"
export RISC0_HOME="$risc0_home"
export RISC0_SERVER_PATH="$r0vm_bin"
if [[ "$(cargo risczero --version)" != "cargo-risczero ${risc0_version}" ]]; then
  echo "expected cargo-risczero ${risc0_version}" >&2
  exit 1
fi
if [[ "$($r0vm_bin --version)" != "risc0-r0vm ${risc0_version}" ]]; then
  echo "expected r0vm ${risc0_version}" >&2
  exit 1
fi

if [[ ! -f "${circuits_dir}/VERSION" ]] || [[ "$(<"${circuits_dir}/VERSION")" != "$circuits_version" ]]; then
  scratch="$(mktemp -d "${TMPDIR:-/tmp}/lez-v02-circuits-${run_id}.XXXXXX")"
  trap 'rm -rf "$scratch"' EXIT
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

cargo fmt --manifest-path "$root_manifest" -- --check
cargo test --locked --manifest-path "$root_manifest" --all-targets
cargo clippy --locked --manifest-path "$root_manifest" --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --manifest-path "$root_manifest" --no-deps

cargo fmt --manifest-path "$guest_manifest" -- --check
CARGO_TARGET_DIR="$guest_target" \
  cargo check --locked --manifest-path "$guest_manifest" --bins
CARGO_TARGET_DIR="$guest_target" \
  cargo clippy --locked --manifest-path "$guest_manifest" --bins -- -D warnings
CARGO_TARGET_DIR="$guest_target" RUSTDOCFLAGS="-D warnings" \
  cargo doc --locked --manifest-path "$guest_manifest" --no-deps --bins

# Build the deployment artifact with the official digest-pinned Risc0 builder
# in a run-local copy so concurrent work cannot share or overwrite guest target
# state. Host-side methods/deployer tests below independently embed a real guest.
guest_build_root="${artifact_target}/docker-guest-source"
guest_build_manifest_dir="${guest_build_root}/escrow/methods/guest"
guest_build_contract_dir="${guest_build_root}/escrow/src"
if [[ -e "$guest_build_root" ]]; then
  echo "run-local Docker guest source already exists: ${guest_build_root}" >&2
  exit 1
fi
mkdir -p "$guest_build_manifest_dir" "$guest_build_contract_dir"
cp "${guest_manifest%Cargo.toml}Cargo.toml" "$guest_build_manifest_dir/Cargo.toml"
cp "${guest_manifest%Cargo.toml}Cargo.lock" "$guest_build_manifest_dir/Cargo.lock"
cp -R "${guest_manifest%Cargo.toml}src" "$guest_build_manifest_dir/src"
cp "compat/lez-v0.2-provisional/escrow/src/lib.rs" "$guest_build_contract_dir/lib.rs"
(
  cd "$guest_build_root"
  export RISC0_DOCKER_CONTAINER_TAG="$risc0_guest_builder_tag"
  export CARGO_TARGET_DIR="${guest_build_root}/target"
  cargo risczero build --manifest-path escrow/methods/guest/Cargo.toml
)
mapfile -t docker_guest_elfs < <(
  find "$guest_build_root/target" -type f -name 'zec_escrow_v02.bin' -print
)
if [[ "${#docker_guest_elfs[@]}" -ne 1 ]]; then
  echo "expected exactly one digest-pinned Docker guest ELF, found ${#docker_guest_elfs[@]}" >&2
  exit 1
fi
docker_guest_elf="${docker_guest_elfs[0]}"
docker_elf_sha256="$(sha256sum "$docker_guest_elf" | cut -d ' ' -f 1)"
docker_image_id="$($r0vm_bin --elf "$docker_guest_elf" --id)"
if [[ "$docker_elf_sha256" != "$expected_elf_sha256" ]]; then
  echo "Docker-built v0.2 guest ELF digest drift: expected ${expected_elf_sha256}, got ${docker_elf_sha256}" >&2
  exit 1
fi
if [[ "$docker_image_id" != "$expected_image_id" ]]; then
  echo "Docker-built v0.2 guest ImageID drift: expected ${expected_image_id}, got ${docker_image_id}" >&2
  exit 1
fi
rg -Fqx "risc0_guest_builder = \"${risc0_guest_builder}\"" "$artifact_manifest"

cargo fmt --manifest-path "$methods_manifest" -- --check
CARGO_TARGET_DIR="$artifact_target" \
  cargo test --locked --manifest-path "$methods_manifest" --all-targets
CARGO_TARGET_DIR="$artifact_target" \
  cargo clippy --locked --manifest-path "$methods_manifest" --all-targets -- -D warnings
CARGO_TARGET_DIR="$artifact_target" RUSTDOCFLAGS="-D warnings" \
  cargo doc --locked --manifest-path "$methods_manifest" --no-deps

mapfile -t guest_elfs < <(
  find "$artifact_target/riscv-guest" -type f -name 'zec_escrow_v02.bin' -print
)
if [[ "${#guest_elfs[@]}" -ne 1 ]]; then
  echo "expected exactly one run-isolated combined v0.2 guest ELF, found ${#guest_elfs[@]}" >&2
  exit 1
fi
guest_elf="${guest_elfs[0]}"
actual_elf_sha256="$(sha256sum "$guest_elf" | cut -d ' ' -f 1)"
actual_image_id="$($r0vm_bin --elf "$guest_elf" --id)"
if [[ "$actual_elf_sha256" != "$expected_elf_sha256" ]]; then
  echo "v0.2 guest ELF digest drift: expected ${expected_elf_sha256}, got ${actual_elf_sha256}" >&2
  exit 1
fi
if [[ "$actual_image_id" != "$expected_image_id" ]]; then
  echo "v0.2 guest ImageID drift: expected ${expected_image_id}, got ${actual_image_id}" >&2
  exit 1
fi
rg -Fqx "elf_sha256 = \"${actual_elf_sha256}\"" "$artifact_manifest"
rg -Fqx "image_id = \"${actual_image_id}\"" "$artifact_manifest"
rg -Fqx 'artifact_status = "locally-built-artifact-checked"' "$artifact_manifest"
rg -Fqx 'transaction_hash = "pending"' "$artifact_manifest"
rg -Fqx 'inclusion_block_id = 0' "$artifact_manifest"
rg -Fqx 'inclusion_block_hash = "pending"' "$artifact_manifest"

cargo fmt --manifest-path "$deployer_manifest" -- --check
CARGO_TARGET_DIR="$artifact_target" \
  cargo test --locked --manifest-path "$deployer_manifest" --all-targets
CARGO_TARGET_DIR="$artifact_target" \
  cargo clippy --locked --manifest-path "$deployer_manifest" --all-targets -- -D warnings
CARGO_TARGET_DIR="$artifact_target" RUSTDOCFLAGS="-D warnings" \
  cargo doc --locked --manifest-path "$deployer_manifest" --no-deps --bins

lockfile="compat/lez-v0.2-provisional/Cargo.lock"
rg -Fq "?rev=${spel_commit}#${spel_commit}" "$lockfile" || {
  echo "provisional lockfile did not resolve exact SPEL PR head ${spel_commit}" >&2
  exit 1
}
rg -Fq "?tag=v0.2.0#${lez_commit}" "$lockfile" || {
  echo "provisional lockfile did not resolve LEZ v0.2.0 to ${lez_commit}" >&2
  exit 1
}
if rg -q 'logos-execution-zone\.git\?rev=' "$lockfile"; then
  echo "LEZ revision source would duplicate PR #238's tag-based lee_core types" >&2
  exit 1
fi
while IFS= read -r source; do
  if [[ "$source" != *"?tag=v0.2.0#${lez_commit}"* ]]; then
    echo "unexpected LEZ source identity: ${source}" >&2
    exit 1
  fi
done < <(rg 'source = "git\+https://github.com/logos-blockchain/logos-execution-zone\.git' "$lockfile")

check_locked_sources() {
  local label="$1"
  local nested_lockfile="$2"
  local require_spel="$3"

  [[ -f "$nested_lockfile" ]] || {
    echo "missing independently locked ${label} graph: ${nested_lockfile}" >&2
    exit 1
  }
  rg -Fq "?tag=v0.2.0#${lez_commit}" "$nested_lockfile" || {
    echo "${label} lockfile did not resolve LEZ v0.2.0 to ${lez_commit}" >&2
    exit 1
  }
  if rg -q 'logos-execution-zone\.git\?rev=' "$nested_lockfile"; then
    echo "${label} lockfile contains a revision-based duplicate LEZ identity" >&2
    exit 1
  fi
  while IFS= read -r source; do
    if [[ "$source" != *"?tag=v0.2.0#${lez_commit}"* ]]; then
      echo "unexpected ${label} LEZ source identity: ${source}" >&2
      exit 1
    fi
  done < <(rg 'source = "git\+https://github.com/logos-blockchain/logos-execution-zone\.git' "$nested_lockfile")

  if [[ "$require_spel" == "yes" ]]; then
    rg -Fq "?rev=${spel_commit}#${spel_commit}" "$nested_lockfile" || {
      echo "${label} lockfile did not resolve exact SPEL PR head ${spel_commit}" >&2
      exit 1
    }
  elif rg -q 'source = "git\+https://github.com/logos-co/spel\.git' "$nested_lockfile"; then
    echo "${label} unexpectedly contains the SPEL source graph" >&2
    exit 1
  fi
}

check_locked_sources \
  "methods" \
  "compat/lez-v0.2-provisional/escrow/methods/Cargo.lock" \
  yes
check_locked_sources \
  "guest" \
  "compat/lez-v0.2-provisional/escrow/methods/guest/Cargo.lock" \
  yes
check_locked_sources \
  "deployer" \
  "compat/lez-v0.2-provisional/escrow/deployer/Cargo.lock" \
  no

expected_root_advisories="$(printf '%s\n' \
  RUSTSEC-2023-0071 \
  RUSTSEC-2025-0055 \
  RUSTSEC-2026-0118 \
  RUSTSEC-2026-0119)"
actual_root_advisories="$(rg -o 'RUSTSEC-[0-9]{4}-[0-9]{4}' compat/lez-v0.2-provisional/deny.toml | sort -u)"
if [[ "$actual_root_advisories" != "$expected_root_advisories" ]]; then
  echo "provisional root advisory exceptions changed; review scope and reachability" >&2
  exit 1
fi

check_policy_advisories() {
  local label="$1"
  local policy="$2"
  shift 2
  local expected
  local actual
  expected="$(printf '%s\n' "$@" | sort -u)"
  actual="$(rg -o 'RUSTSEC-[0-9]{4}-[0-9]{4}' "$policy" | sort -u)"
  if [[ "$actual" != "$expected" ]]; then
    echo "${label} advisory exceptions changed; review graph-local scope and reachability" >&2
    exit 1
  fi
}

check_policy_advisories \
  "methods" \
  "compat/lez-v0.2-provisional/escrow/methods/deny.toml" \
  RUSTSEC-2023-0071 \
  RUSTSEC-2025-0055
check_policy_advisories \
  "guest" \
  "compat/lez-v0.2-provisional/escrow/methods/guest/deny.toml" \
  RUSTSEC-2023-0071 \
  RUSTSEC-2025-0055
check_policy_advisories \
  "deployer" \
  "compat/lez-v0.2-provisional/escrow/deployer/deny.toml" \
  RUSTSEC-2023-0071 \
  RUSTSEC-2025-0055 \
  RUSTSEC-2026-0118 \
  RUSTSEC-2026-0119

rsa_tree="$(cargo tree --locked --manifest-path "$root_manifest" -e features -i rsa@0.9.10)"
for dependency in "rzup v0.5.1" "risc0-zkvm v3.0.5" "lee_core v0.1.0"; do
  if ! rg -Fq "$dependency" <<<"$rsa_tree"; then
    echo "reviewed RSA advisory path changed: missing ${dependency}" >&2
    exit 1
  fi
done
if rg -q 'rzup feature "(publish|install)"' <<<"$rsa_tree"; then
  echo "unsafe rzup private-key/install feature entered the provisional graph" >&2
  exit 1
fi

tracing_tree="$(cargo tree --locked --manifest-path "$root_manifest" -e features -p tracing-subscriber@0.2.25)"
if rg -q 'tracing-subscriber feature "(fmt|ansi)"' <<<"$tracing_tree"; then
  echo "vulnerable tracing-subscriber formatter entered the provisional graph" >&2
  exit 1
fi
tracing_reverse="$(cargo tree --locked --manifest-path "$root_manifest" -e features -i tracing-subscriber@0.2.25)"
for dependency in "ark-relations v0.5.1" "risc0-groth16 v3.0.4" "risc0-zkvm v3.0.5" "lee_core v0.1.0"; do
  if ! rg -Fq "$dependency" <<<"$tracing_reverse"; then
    echo "reviewed tracing advisory path changed: missing ${dependency}" >&2
    exit 1
  fi
done

hickory_tree="$(cargo tree --locked --manifest-path "$root_manifest" -e features -p hickory-proto@0.25.0-alpha.5)"
if rg -q 'hickory-proto feature "dnssec-(ring|aws-lc-rs)"' <<<"$hickory_tree"; then
  echo "DNSSEC validation entered the advisory-constrained Hickory graph" >&2
  exit 1
fi
hickory_reverse="$(cargo tree --locked --manifest-path "$root_manifest" -e features -i hickory-proto@0.25.0-alpha.5)"
for dependency in "hickory-resolver v0.25.0-alpha.5" "libp2p-dns v0.43.0" "sequencer_service v0.1.0"; do
  if ! rg -Fq "$dependency" <<<"$hickory_reverse"; then
    echo "reviewed Hickory advisory path changed: missing ${dependency}" >&2
    exit 1
  fi
done

check_risc0_advisory_features() {
  local label="$1"
  local manifest="$2"
  local nested_rsa_tree
  local nested_tracing_tree

  nested_rsa_tree="$(cargo tree --locked --manifest-path "$manifest" -e features -i rsa@0.9.10)"
  for dependency in "rzup v0.5.1" "risc0-zkvm v3.0.5"; do
    if ! rg -Fq "$dependency" <<<"$nested_rsa_tree"; then
      echo "reviewed ${label} RSA advisory path changed: missing ${dependency}" >&2
      exit 1
    fi
  done
  if rg -q 'rzup feature "(publish|install)"' <<<"$nested_rsa_tree"; then
    echo "unsafe rzup private-key/install feature entered the ${label} graph" >&2
    exit 1
  fi

  nested_tracing_tree="$(cargo tree --locked --manifest-path "$manifest" -e features -p tracing-subscriber@0.2.25)"
  if rg -q 'tracing-subscriber feature "(fmt|ansi)"' <<<"$nested_tracing_tree"; then
    echo "vulnerable tracing-subscriber formatter entered the ${label} graph" >&2
    exit 1
  fi
}

check_risc0_advisory_features "methods" "$methods_manifest"
check_risc0_advisory_features "guest" "$guest_manifest"
check_risc0_advisory_features "deployer" "$deployer_manifest"

deployer_hickory_tree="$(cargo tree --locked --manifest-path "$deployer_manifest" -e features -p hickory-proto@0.25.0-alpha.5)"
if rg -q 'hickory-proto feature "dnssec-(ring|aws-lc-rs)"' <<<"$deployer_hickory_tree"; then
  echo "DNSSEC validation entered the deployer advisory-constrained Hickory graph" >&2
  exit 1
fi
deployer_hickory_reverse="$(cargo tree --locked --manifest-path "$deployer_manifest" -e features -i hickory-proto@0.25.0-alpha.5)"
rg -Fq "libp2p-dns v0.43.0" <<<"$deployer_hickory_reverse" || {
  echo "reviewed deployer Hickory advisory path changed" >&2
  exit 1
}
rg -Fq 'const OFFICIAL_RPC_URL: &str = "https://testnet.lez.logos.co";' \
  compat/lez-v0.2-provisional/escrow/deployer/src/main.rs
if rg -q '^sequencer_service[[:space:]]*=' \
  compat/lez-v0.2-provisional/escrow/deployer/Cargo.toml; then
  echo "executable sequencer service entered the bounded deployment graph" >&2
  exit 1
fi

compat_test="compat/lez-v0.2-provisional/tests/compatibility.rs"
actual_compat_test_sha256="$(sha256sum "$compat_test" | cut -d ' ' -f 1)"
if [[ "$actual_compat_test_sha256" != "$compat_test_sha256" ]]; then
  echo "compile-only compatibility test changed; review Hickory advisory exceptions" >&2
  exit 1
fi
rg -Fq 'drop(standalone);' "$compat_test" || {
  echo "compile-only standalone future is no longer explicitly dropped" >&2
  exit 1
}
for forbidden in '#[tokio::test' '.await' 'block_on(' 'check_health(' 'send_transaction('; do
  if rg -Fq "$forbidden" "$compat_test"; then
    echo "compile-only Hickory exception invalidated by executable test pattern: ${forbidden}" >&2
    exit 1
  fi
done
