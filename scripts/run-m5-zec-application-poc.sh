#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
umask 077

if [[ "${POC_DIRECTION:-taker_sells_lez}" != taker_sells_lez ]]; then
  printf '%s\n' 'M5 ZEC application PoC currently requires POC_DIRECTION=taker_sells_lez' >&2
  exit 2
fi
if [[ -n "${M5_APPLICATION_MODE:-}" && "$M5_APPLICATION_MODE" != 1 ]]; then
  printf '%s\n' 'run-m5-zec-application-poc.sh fixes M5_APPLICATION_MODE=1' >&2
  exit 2
fi

export M5_APPLICATION_MODE=1
export POC_DIRECTION=taker_sells_lez

# The delegated runner retains the endpoint-tuple lock, provision-to-completion
# safety clock, exact role processes, chain-effect guards, and scoped cleanup.
exec ./scripts/run-m2-taker-sells-lez-poc.sh "$@"
