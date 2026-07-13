#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
repo_root="$PWD"

risc0_version="3.0.5"
rzup_version="0.5.1"
circuits_version="v0.4.2"
circuits_sha256="e9131ffac8b08a80e1a7152b34fdd5d5c52674d4cb396e8162131ca5dd7c858d"
expected_elf_sha256="a324355c6417f6ac7265ab8ba880287d0976e8c27a672917d293bddd80be7006"
expected_image_id="c14c978abbaedeffb54c71aa6a96275d1fdb66fcf79f7343bf6bf7aee04f4483"
run_id="${RUN_ID:-local-$$}"
tool_dir="${LEZ_E2E_TOOL_DIR:-${TMPDIR:-/tmp}/lez-atomic-swaps-tools/risc0-${risc0_version}}"
risc0_home="${tool_dir}/home"
isolated_cargo_home="${tool_dir}/cargo-home"
rzup_bin="${tool_dir}/bin/rzup"
r0vm_bin="${risc0_home}/extensions/v${risc0_version}-cargo-risczero-x86_64-unknown-linux-gnu/r0vm"
circuits_dir="${LOGOS_BLOCKCHAIN_CIRCUITS:-${tool_dir}/logos-blockchain-circuits-${circuits_version}}"
guest_manifest="compat/spel-zec-escrow/methods/guest/Cargo.toml"
methods_manifest="compat/spel-zec-escrow/methods/Cargo.toml"
standalone_manifest="compat/lez-standalone-e2e/Cargo.toml"
artifact_manifest="compat/spel-zec-escrow/methods/guest/artifact-manifest.toml"
cost_evidence="docs/evidence/lez-v0.1.2-escrow-costs.json"
guest_elf="compat/spel-zec-escrow/methods/guest/target/riscv32im-risc0-zkvm-elf/docker/zec_escrow.bin"
guest_elf_absolute="${repo_root}/${guest_elf}"

mkdir -p "${tool_dir}/bin" "$isolated_cargo_home"

if [[ ! -x "$rzup_bin" ]]; then
  CARGO_HOME="$isolated_cargo_home" \
    cargo install rzup --version "$rzup_version" --locked --root "$tool_dir"
fi
if [[ "$($rzup_bin --version)" != "rzup ${rzup_version}" ]]; then
  echo "expected rzup ${rzup_version} at ${rzup_bin}" >&2
  exit 1
fi

if [[ ! -x "$r0vm_bin" ]]; then
  RISC0_HOME="$risc0_home" CARGO_HOME="$isolated_cargo_home" \
    "$rzup_bin" install r0vm "$risc0_version"
fi

export PATH="${isolated_cargo_home}/bin:${tool_dir}/bin:${PATH}"
export RISC0_HOME="$risc0_home"
export RISC0_SERVER_PATH="$r0vm_bin"
export CARGO_BUILD_JOBS=2

if [[ "$(cargo risczero --version)" != "cargo-risczero ${risc0_version}" ]]; then
  echo "expected cargo-risczero ${risc0_version}" >&2
  exit 1
fi
if [[ "$($r0vm_bin --version)" != "risc0-r0vm ${risc0_version}" ]]; then
  echo "expected r0vm ${risc0_version}" >&2
  exit 1
fi

if [[ ! -f "${circuits_dir}/VERSION" ]] || [[ "$(<"${circuits_dir}/VERSION")" != "$circuits_version" ]]; then
  scratch="$(mktemp -d "${TMPDIR:-/tmp}/lez-circuits-${run_id}.XXXXXX")"
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

for lockfile in \
  "${guest_manifest%Cargo.toml}Cargo.lock" \
  "${standalone_manifest%Cargo.toml}Cargo.lock"; do
  rg -Fq '#cf3639d8252040d13b3d4e933feb19b42c76e14a' "$lockfile"
done
rg -Fq '#73fc462eb8f0a4d00f1a846437c627ec2e523f83' \
  "${guest_manifest%Cargo.toml}Cargo.lock"

for manifest in "$guest_manifest" "$standalone_manifest"; do
  rsa_tree="$(cargo tree --locked --manifest-path "$manifest" -e features -i rsa@0.9.10)"
  if rg -q 'rzup feature "(publish|install)"' <<<"$rsa_tree"; then
    echo "unsafe rzup private-key/install feature entered ${manifest}" >&2
    exit 1
  fi
  tracing_tree="$(cargo tree --locked --manifest-path "$manifest" -e features -p tracing-subscriber@0.2.25)"
  if rg -q 'tracing-subscriber feature "(fmt|ansi)"' <<<"$tracing_tree"; then
    echo "vulnerable tracing-subscriber formatter entered ${manifest}" >&2
    exit 1
  fi
done

cargo fmt --manifest-path "$guest_manifest" -- --check
cargo clippy --locked --manifest-path "$guest_manifest" -- -D warnings
cargo risczero build --manifest-path "$guest_manifest"

actual_elf_sha256="$(sha256sum "$guest_elf" | cut -d' ' -f1)"
actual_image_id="$($r0vm_bin --elf "$guest_elf" --id)"
if [[ "$actual_elf_sha256" != "$expected_elf_sha256" ]]; then
  echo "guest ELF digest drift: expected ${expected_elf_sha256}, got ${actual_elf_sha256}" >&2
  exit 1
fi
if [[ "$actual_image_id" != "$expected_image_id" ]]; then
  echo "guest image ID drift: expected ${expected_image_id}, got ${actual_image_id}" >&2
  exit 1
fi
rg -Fqx "elf_sha256 = \"${actual_elf_sha256}\"" "$artifact_manifest"
rg -Fqx "image_id = \"${actual_image_id}\"" "$artifact_manifest"

methods_target="${LEZ_METHODS_TARGET_DIR:-${TMPDIR:-/tmp}/lez-methods-${run_id}}"
standalone_target="${LEZ_STANDALONE_TARGET_DIR:-${TMPDIR:-/tmp}/lez-standalone-${run_id}}"
RISC0_SKIP_BUILD=1 CARGO_TARGET_DIR="$methods_target" \
  cargo clippy --locked --manifest-path "$methods_manifest" --all-targets -- -D warnings
cargo fmt --manifest-path "$standalone_manifest" -- --check
CARGO_TARGET_DIR="$standalone_target" \
  cargo clippy --locked --manifest-path "$standalone_manifest" --all-targets -- -D warnings
CARGO_TARGET_DIR="$standalone_target" \
  cargo test --locked --manifest-path "$standalone_manifest" --all-targets -- \
    --test-threads=1
RISC0_DEV_MODE=1 LEZ_ESCROW_GUEST_ELF="$guest_elf_absolute" CARGO_TARGET_DIR="$standalone_target" \
  cargo test --locked --manifest-path "$standalone_manifest" --test deploy -- \
    --ignored --nocapture --test-threads=1

cost_output_dir="${LEZ_COST_OUTPUT_DIR:-${TMPDIR:-/tmp}/lez-costs-${run_id}}"
cost_log="${cost_output_dir}/cost.log"
cost_json="${cost_output_dir}/generated.json"
mkdir -p "$cost_output_dir"
RISC0_DEV_MODE=1 RISC0_INFO=1 \
  RUST_LOG=risc0_zkvm::host::server::session=info \
  LEZ_ESCROW_GUEST_ELF="$guest_elf_absolute" \
  CARGO_TARGET_DIR="$standalone_target" \
  cargo test --locked --manifest-path "$standalone_manifest" --test costs \
    -- --ignored --nocapture --test-threads=1 2>&1 | tee "$cost_log"
awk -f scripts/parse-lez-costs.awk "$cost_log" > "$cost_json"
diff -u "$cost_evidence" "$cost_json"

printf 'LEZ standalone guest native/token lifecycle proof passed: elf_sha256=%s image_id=%s\n' \
  "$actual_elf_sha256" "$actual_image_id"
printf 'LEZ native/token recursive cost evidence passed: %s\n' "$cost_json"
