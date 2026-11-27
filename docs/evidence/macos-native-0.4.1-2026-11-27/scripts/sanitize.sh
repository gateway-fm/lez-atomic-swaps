#!/usr/bin/env bash
# sanitize.sh <dir> — rewrite every text file under <dir> in place so that it
# names no local path or user: the native homes and sessions become
# $LEZ_NODE_HOME / $SESSION, the work directory $WORK, the home directory
# $HOME and the user name $USER. Then scan for anything that looks secret
# (the stack's logins, the role keys, the cookies) and fail if one is found.
set -euo pipefail
dir="${1:?}"; W="$(cd "$(dirname "$0")/../.." && pwd)"; U="$(id -un)"
find "$dir" -type f \( -name '*.log' -o -name '*.txt' -o -name '*.json' -o -name '*.md' -o -name '*.sh' -o -name '*.py' -o -name '*.yaml' -o -name '*.tsv' \) -print0 |
  xargs -0 perl -pi -e "s#$W/h[ABT]2?#\\\$LEZ_NODE_HOME#g; s#$W/s[ABT]2?#\\\$SESSION#g; s#$W#\\\$WORK#g; s#$HOME#\\\$HOME#g; s#/private/tmp/claude-[0-9]+/[^ \"']*#\\\$TMPDIR#g; s#\\b$U\\b#\\\$USER#g"
# secrets: every login and key of both stacks, matched literally
found=0
for S in "$W/stack/lez-swap-stack-sha-2373e28735b7-arm64" $HOME/tmp/lez-m4/release-040/native/stack/lez-swap-stack-v0.4.0-arm64; do
  for f in "$S"/runtime/monero/*.login "$S"/runtime/lez/*/lez-signer.key "$S"/runtime/lez/*/zcash.key "$S"/runtime/secrets/btc-rpc-cookie; do
    [[ -s "$f" ]] || continue
    secret="$(tr -d '\n' <"$f" | sed 's/^[^:]*://')"
    [[ ${#secret} -ge 16 ]] || continue
    if grep -rqF -- "$secret" "$dir"; then echo "SECRET from $(basename "$(dirname "$f")")/$(basename "$f") found" >&2; found=1; fi
  done
done
grep -rlE 'password=|rpc-login=|daemon-login=' "$dir" && found=1 || true
exit $found
