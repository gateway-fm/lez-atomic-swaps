#!/bin/bash
# linux-install.sh — in a clean linux-arm64 container (Debian bookworm), the
# same catalog install with logosctl 0.3.2 (AppImage, extracted): a fresh
# session, the local test catalog beside the official one, both desks, then
# their dependency status and what is on disk.
set -u
L=(/opt/logosctl/AppRun --config-dir /tmp/s)
export SSL_CERT_FILE=/bundle.pem
run() { echo "[$(date -u +%H:%M:%SZ)] \$ logosctl ${*}"; "${L[@]}" "$@"; echo "[exit $?]"; echo; }
run --version
run daemon start --detach
run catalog add https://localhost:8443/logos-repo.json
run catalog refresh
"${L[@]}" --json catalog ls | jq -c '.[] | {name,enabled,isDefault,indexUrl,resolveError}'
for desk in lez_atomic_swap_maker lez_atomic_swap_taker; do
  run package install "$desk" -y
  run --json package deps "$desk" --recursive
done
run --json package deps lez_atomic_swap_maker --recursive
echo "# installed: name variant version root hash"
for m in /tmp/s/modules/* /tmp/s/plugins/*; do
  printf '%s %s %s %s\n' "${m##*/}" "$(cat "$m/variant" 2>/dev/null)" "$(jq -r .version "$m/manifest.json")" "$(jq -r .hashes.root "$m/manifest.json")"
done
echo "# lez_maker_node payload"; jq -c '{build_source, variant}' /tmp/s/modules/lez_maker_node/bin/payload.json 2>/dev/null || ls /tmp/s/modules/lez_maker_node/bin
