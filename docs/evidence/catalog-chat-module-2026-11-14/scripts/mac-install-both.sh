#!/opt/homebrew/bin/bash
# mac-install-both.sh <session dir> <catalog url> <basecamp user dir> — one
# fresh session, `package install` of the Maker desk by name, then of the
# Taker desk by name, deps of both; then modules/ and plugins/ copied into a
# fresh Basecamp user dir (the one host that runs both roles).
set -u
S="${1:?session}" CAT="${2:?catalog url}" U="${3:?basecamp user dir}"
L=$HOME/tmp/logosctl-0.3.2-macos/x/logosctl-aarch64-macos/bin/logosctl
CA=${SCRATCH:?}/previous/tls/bundle.pem
lc() { env -i HOME="$HOME" USER="$USER" TMPDIR="$TMPDIR" PATH=/usr/bin:/bin:/usr/sbin:/sbin SSL_CERT_FILE="$CA" "$L" --config-dir "$S" "$@"; }
run() { echo "[$(date -u +%H:%M:%SZ)] \$ logosctl $*"; lc "$@"; echo "[exit $?]"; echo; }
rm -rf "$S" "$U"; mkdir -p "$S"
run --version
run daemon start --detach
run catalog add "$CAT"
run catalog source http
run catalog refresh
for desk in lez_atomic_swap_maker lez_atomic_swap_taker; do
  run package install "$desk" -y
  run --json package deps "$desk" --recursive
done
run --json package deps lez_atomic_swap_maker --recursive
echo "# installed: name variant version"
for m in "$S"/modules/* "$S"/plugins/*; do
  printf '%s %s %s\n' "${m##*/}" "$(cat "$m/variant" 2>/dev/null)" "$(jq -r .version "$m/manifest.json" 2>/dev/null)"
done
run daemon stop
mkdir -p "$U" && cp -a "$S/modules" "$S/plugins" "$U/" && echo "copied modules/ plugins/ to the Basecamp user dir"
