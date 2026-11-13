#!/bin/bash
# macos-install.sh — on this Mac, a fresh logosctl session (an empty
# --config-dir) with the local test catalog beside the official one: both
# desks installed through the package manager, their dependency status, and
# what is on disk (variant, version, root hash of each installed package).
set -u
S="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/session.sh"
run() { echo "[$(date -u +%H:%M:%SZ)] \$ logosctl $*"; "$S" "$@"; echo "[exit $?]"; echo; }
run --version
run daemon-start
run catalog add https://localhost:8443/logos-repo.json
run catalog refresh
"$S" --json catalog ls | jq -c '.[] | {name,enabled,isDefault,indexUrl,resolveError}'
for desk in lez_atomic_swap_maker lez_atomic_swap_taker; do
  run package install "$desk" -y
  run --json package deps "$desk" --recursive
done
run --json package deps lez_atomic_swap_maker --recursive
echo "# installed: name variant version root hash"
for m in "$SESSION"/modules/* "$SESSION"/plugins/*; do
  printf '%s %s %s %s\n' "${m##*/}" "$(cat "$m/variant")" "$(jq -r .version "$m/manifest.json")" "$(jq -r .hashes.root "$m/manifest.json")"
done
echo "# role packages' payload"
for role in maker taker; do jq -c '{role, build_source, variant}' "$SESSION/modules/lez_${role}_node/bin/payload.json"; done
codesign -dv "$SESSION/modules/lez_maker_node/bin/lez-maker-node" 2>&1 | grep -E '^(Signature|CodeDirectory)'
