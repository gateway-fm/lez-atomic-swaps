#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
umask 077

if [[ "${POC_DIRECTION:-taker_sells_lez}" != taker_sells_lez ]]; then
  printf '%s\n' 'M6 ZEC refund PoC requires POC_DIRECTION=taker_sells_lez' >&2
  exit 2
fi
if [[ -n "${M5_APPLICATION_MODE:-}" && "$M5_APPLICATION_MODE" != 1 ]]; then
  printf '%s\n' 'M6 ZEC refund PoC fixes M5_APPLICATION_MODE=1' >&2
  exit 2
fi
if [[ -n "${M6_TAKER_SERVICE_MODE:-}" && "$M6_TAKER_SERVICE_MODE" != 1 ]]; then
  printf '%s\n' 'run-m6-zec-taker-service-refund-poc.sh fixes M6_TAKER_SERVICE_MODE=1' >&2
  exit 2
fi
if [[ -n "${M6_ZEC_JOURNEY:-}" && "$M6_ZEC_JOURNEY" != refund ]]; then
  printf '%s\n' 'run-m6-zec-taker-service-refund-poc.sh fixes M6_ZEC_JOURNEY=refund' >&2
  exit 2
fi

export M5_APPLICATION_MODE=1
export M6_TAKER_SERVICE_MODE=1
export M6_ZEC_JOURNEY=refund
export POC_DIRECTION=taker_sells_lez

# Reuse the exact endpoint lock, local LEZ v0.2/Zebra nodes, role processes,
# safety clock, chain-effect guards, and scoped cleanup from the proven
# application corridor.
exec ./scripts/run-m2-taker-sells-lez-poc.sh "$@"
