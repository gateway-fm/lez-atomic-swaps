#!/bin/bash
# Healthy when every configured role signer reports "serving" (lez_role_signer.h
# writes <socket>.status) and its socket exists; nothing to check without one.
for socket in "${LEZ_MAKER_SIGNER_SOCKET:-}" "${LEZ_TAKER_SIGNER_SOCKET:-}"; do
  [[ -z "$socket" ]] && continue
  [[ -S "$socket" ]] && grep -q '^serving ' "$socket.status" 2>/dev/null || exit 1
done
