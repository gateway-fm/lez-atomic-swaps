#!/usr/bin/env bash
set -euo pipefail

readonly LEZ_REPOSITORY="https://github.com/logos-blockchain/logos-execution-zone.git"
readonly LEZ_COMMIT="cac4921581b37e85ae25e940f3a62412cd22308e"

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

git -C "${workdir}/lez" fetch --quiet --depth 1 origin "${LEZ_COMMIT}"
git -C "${workdir}/lez" checkout --quiet --detach "${LEZ_COMMIT}"

test "$(git -C "${workdir}/lez" rev-parse HEAD)" = "${LEZ_COMMIT}"

cd "${workdir}/lez"

# Fail loudly when upstream behavior or its traced acceptance path changes.
rg -F 'value >= start' lee/state_machine/core/src/program/mod.rs
rg -F 'value < end' lee/state_machine/core/src/program/mod.rs
rg -F '.push((TransactionOrigin::User, authenticated_tx))' lez/sequencer/service/src/service.rs
rg -F 'validate_on_state(' lez/sequencer/core/src/lib.rs
rg -F 'Signature::try_from(self.value.as_slice())' lee/state_machine/src/signature/mod.rs

RISC0_SKIP_BUILD=1 cargo test -p lee signature_verification_from_bip340_test_vectors

if [[ "${LEZ_VERIFY_GUESTS:-0}" = "1" ]]; then
  cargo test -p lee validity_window_works
else
  echo "Skipping guest-backed validity tests; set LEZ_VERIFY_GUESTS=1 after 'rzup install rust'." >&2
fi
