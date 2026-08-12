#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export POC_DIRECTION=taker_sells_foreign
export M5_APPLICATION_MODE=1
export M6_TAKER_SERVICE_MODE=1
export M6_ZEC_JOURNEY=first_lock_refund

exec ./scripts/run-m2-taker-sells-lez-poc.sh "$@"
