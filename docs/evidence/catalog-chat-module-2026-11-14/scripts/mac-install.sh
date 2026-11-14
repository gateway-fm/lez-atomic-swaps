#!/opt/homebrew/bin/bash
# mac-install.sh <session dir> <catalog url> <desk> — a fresh logosctl 0.3.2
# session on this Mac, the local test catalog beside the official one, the
# HTTP download source, then ONE `package install <desk>` by name: the plan,
# `package deps --recursive`, and what is on disk.
set -u
S="${1:?session}" CAT="${2:?catalog url}" DESK="${3:?desk}"
L=<home>/tmp/logosctl-0.3.2-macos/x/logosctl-aarch64-macos/bin/logosctl
CA=<scratch>/previous/tls/bundle.pem
lc() { env -i HOME="$HOME" USER="$USER" TMPDIR="$TMPDIR" PATH=/usr/bin:/bin:/usr/sbin:/sbin SSL_CERT_FILE="$CA" "$L" --config-dir "$S" "$@"; }
run() { echo "[$(date -u +%H:%M:%SZ)] \$ logosctl $*"; lc "$@"; echo "[exit $?]"; echo; }
rm -rf "$S"; mkdir -p "$S"
echo "# uname: $(uname -sm)"
run --version
run daemon start --detach
run catalog add "$CAT"
run catalog source http
run catalog refresh
lc --json catalog ls | jq -c '.[] | {name,enabled,isDefault,indexUrl,resolveError}'
run package install "$DESK" -y
run --json package deps "$DESK" --recursive
echo "# installed: name variant version"
for m in "$S"/modules/* "$S"/plugins/*; do
  printf '%s %s %s\n' "${m##*/}" "$(cat "$m/variant" 2>/dev/null)" "$(jq -r .version "$m/manifest.json" 2>/dev/null)"
done
run daemon stop
