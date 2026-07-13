#!/usr/bin/env bash
set -euo pipefail

readonly LEZ_REPOSITORY="https://github.com/logos-blockchain/logos-execution-zone.git"
readonly LEZ_COMMIT="cac4921581b37e85ae25e940f3a62412cd22308e"
readonly LEZ_REF="${LEZ_REF:-${LEZ_COMMIT}}"
REPOSITORY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly REPOSITORY_ROOT

workdir="$(mktemp -d -t lez-atomic-swaps-lez-verify.XXXXXX)"
cleanup() {
  chmod -R u+w "${workdir}" 2>/dev/null || true
  rm -rf -- "${workdir}"
}
trap cleanup EXIT

if [[ -n "${LEZ_SOURCE:-}" ]]; then
  git clone --quiet --no-hardlinks "${LEZ_SOURCE}" "${workdir}/lez"
else
  git clone --quiet --filter=blob:none --no-checkout "${LEZ_REPOSITORY}" "${workdir}/lez"
fi

git -C "${workdir}/lez" fetch --quiet --depth 1 origin "${LEZ_REF}"
git -C "${workdir}/lez" checkout --quiet --detach FETCH_HEAD

resolved_commit="$(git -C "${workdir}/lez" rev-parse HEAD)"
if [[ "${LEZ_REF}" = "${LEZ_COMMIT}" ]]; then
  test "${resolved_commit}" = "${LEZ_COMMIT}"
fi
echo "Verifying LEZ ${LEZ_REF} at ${resolved_commit}" >&2

cd "${workdir}/lez"

# Fail loudly when upstream behavior or its traced acceptance path changes.
rg -F 'value >= start' lee/state_machine/core/src/program/mod.rs
rg -F 'value < end' lee/state_machine/core/src/program/mod.rs
rg -F '.push((TransactionOrigin::User, authenticated_tx))' lez/sequencer/service/src/service.rs
rg -F 'validate_on_state(' lez/sequencer/core/src/lib.rs
rg -F 'Signature::try_from(self.value.as_slice())' lee/state_machine/src/signature/mod.rs
rg -F 'replay_transactions_are_rejected_in_the_same_block' lez/sequencer/core/src/tests.rs
rg -F 'block.body.transactions,' lez/sequencer/core/src/tests.rs
rg -F 'tx.clone(),' lez/sequencer/core/src/tests.rs

CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" \
  RISC0_SKIP_BUILD=1 \
  cargo test -p lee_core --features test_utils 'program::tests::validity_window_'

CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" \
  RISC0_SKIP_BUILD=1 \
  cargo test -p lee signature_verification_from_bip340_test_vectors

if [[ "${LEZ_VERIFY_GUESTS:-0}" = "1" ]]; then
  CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo test -p lee validity_window_works
else
  echo "Skipping guest-backed validity tests; set LEZ_VERIFY_GUESTS=1 after 'rzup install rust'." >&2
fi

if [[ "${LEZ_VERIFY_SEQUENCER:-0}" = "1" ]]; then
  if ! command -v r0vm >/dev/null 2>&1; then
    echo "sequencer verification requires r0vm 3.0.5; install it with 'rzup install r0vm 3.0.5'" >&2
    exit 1
  fi

  git apply --check "${REPOSITORY_ROOT}/tests/upstream/lez-sequencer-reproducers.patch"
  git apply "${REPOSITORY_ROOT}/tests/upstream/lez-sequencer-reproducers.patch"

  if ! command -v unzip >/dev/null 2>&1; then
    busybox_path="$(command -v busybox || true)"
    if [[ -z "${busybox_path}" ]]; then
      echo "sequencer verification requires unzip or busybox" >&2
      exit 1
    fi
    mkdir -p "${workdir}/tools"
    ln -s "${busybox_path}" "${workdir}/tools/unzip"
    export PATH="${workdir}/tools:${PATH}"
  fi

  if [[ -z "${BINDGEN_EXTRA_CLANG_ARGS:-}" ]]; then
    gcc_include="$(cc -print-file-name=include)"
    if [[ -f "${gcc_include}/stdbool.h" ]]; then
      export BINDGEN_EXTRA_CLANG_ARGS="-I${gcc_include}"
    fi
  fi

  sequencer_tests="$(
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" \
      RISC0_DEV_MODE=1 \
      RISC0_SKIP_BUILD=1 \
      cargo test -p sequencer_core --features mock -- --list
  )"
  rg -F 'tests::atomic_swap_reproducer_mempool_admits_then_block_rejects: test' \
    <<<"${sequencer_tests}"
  rg -F 'tests::replay_transactions_are_rejected_in_the_same_block: test' \
    <<<"${sequencer_tests}"

  CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" \
    RISC0_DEV_MODE=1 \
    RISC0_SKIP_BUILD=1 \
    cargo test -p sequencer_core --features mock \
      tests::atomic_swap_reproducer_mempool_admits_then_block_rejects -- --exact
  CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" \
    RISC0_DEV_MODE=1 \
    RISC0_SKIP_BUILD=1 \
    cargo test -p sequencer_core --features mock \
      tests::replay_transactions_are_rejected_in_the_same_block -- --exact
else
  echo "Skipping native sequencer test; set LEZ_VERIFY_SEQUENCER=1 for the isolated heavy lane." >&2
fi
