#!/usr/bin/env bash
# Sourced by scripts/run-ci-quality-gates.sh and scripts/check-all.sh: the
# paths the shell, Dockerfile and Compose gates cover. Monero, Zcash and the
# later milestones' files are outside the public release scope.

is_release_scope_path() {
  local path="$1"
  [[ "$path" != *monero* \
    && "$path" != *xmr* \
    && "$path" != *zcash* \
    && "$path" != *zebra* \
    && "$path" != *zec* \
    && "$path" != *m4-* \
    && "$path" != *m5-* \
    && "$path" != *m7-* \
    && "$path" != */milestone-7/* ]]
}
