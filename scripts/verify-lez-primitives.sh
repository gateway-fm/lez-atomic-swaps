#!/usr/bin/env bash
set -euo pipefail

readonly LEZ_REPOSITORY="https://github.com/logos-blockchain/logos-execution-zone.git"
# LEZ v0.2.0: the same commit deploy/scripts/from-scratch.sh builds the product from.
readonly LEZ_COMMIT="a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
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
# The validity type and the sequencer tests have moved between files across
# upstream commits (program.rs vs program/mod.rs, lib.rs vs tests.rs), so those
# checks search the owning directory rather than one file.
rg -F 'value >= start' lee/state_machine/core/src/
rg -F 'value < end' lee/state_machine/core/src/
rg -F '.push((TransactionOrigin::User, authenticated_tx))' lez/sequencer/service/src/service.rs
rg -F 'validate_on_state(' lez/sequencer/core/src/lib.rs
rg -F 'Signature::try_from(self.value.as_slice())' lee/state_machine/src/signature/mod.rs
rg -F 'replay_transactions_are_rejected_in_the_same_block' lez/sequencer/core/src/
rg -F 'block.body.transactions,' lez/sequencer/core/src/
rg -F 'tx.clone(),' lez/sequencer/core/src/

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
  # The sequencer reproducers are a standalone crate pinned to LEZ v0.2.0 by
  # Cargo, so this lane needs no upstream checkout or patch. It runs the real
  # mempool and block builder in-process with the mock publisher and deploys a
  # test guest, which is compiled with the RISC Zero Rust toolchain.
  if ! command -v r0vm >/dev/null 2>&1; then
    echo "sequencer verification requires r0vm 3.0.5; install it with 'rzup install r0vm 3.0.5'" >&2
    exit 1
  fi
  if ! command -v rzup >/dev/null 2>&1 || ! rzup show 2>/dev/null | rg -q '^rust'; then
    echo "sequencer verification compiles a guest program; install the toolchain with 'rzup install rust'" >&2
    exit 1
  fi

  # Leave the upstream checkout: its rust-toolchain file would otherwise select
  # upstream's compiler for this repository's crate.
  cd "${REPOSITORY_ROOT}"
  reproducers_manifest="${REPOSITORY_ROOT}/compat/lez-v0.2-sequencer-reproducers/Cargo.toml"
  sequencer_reproducers=(
    mempool_admits_then_block_rejects_insufficient_balance
    transaction_bytes_are_preserved_from_mempool_to_block
    valid_when_queued_expires_before_inclusion
    block_window_start_is_inclusive
    block_window_end_is_exclusive
    timestamp_window_expires_while_queued
    timestamp_window_bounds_apply_at_block_time
  )

  # The crate must resolve to the same upstream commit this script verifies.
  rg -F "lez_commit = \"${LEZ_COMMIT}\"" "${reproducers_manifest}"

  sequencer_tests="$(
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" \
      RISC0_DEV_MODE=1 \
      cargo test --locked --manifest-path "${reproducers_manifest}" \
        --test sequencer_reproducers -- --list
  )"
  for reproducer in "${sequencer_reproducers[@]}"; do
    rg -F "${reproducer}: test" <<<"${sequencer_tests}"
  done

  for reproducer in "${sequencer_reproducers[@]}"; do
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" \
      RISC0_DEV_MODE=1 \
      cargo test --locked --manifest-path "${reproducers_manifest}" \
        --test sequencer_reproducers "${reproducer}" -- --exact
  done
else
  echo "Skipping the sequencer reproducers; set LEZ_VERIFY_SEQUENCER=1 for the isolated heavy lane." >&2
fi
