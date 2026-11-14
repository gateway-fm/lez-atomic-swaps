#!/bin/bash
# install.sh <config dir> <desk> — in the linux-arm64 container: a fresh
# logosctl 0.3.2 session, the local catalog (localhost:8473, forwarded to the
# host) beside the official one, HTTP download source, ONE install by name.
set -u
S="${1:?config dir}" DESK="${2:?desk}"
L=(/opt/logosctl/AppRun --config-dir "$S")
run() { echo "[$(date -u +%H:%M:%SZ)] \$ logosctl ${*}"; "${L[@]}" "$@"; echo "[exit $?]"; echo; }
rm -rf "$S"; mkdir -p "$S"
echo "# uname: $(uname -sm); $(. /etc/os-release; echo "$PRETTY_NAME")"
run --version
run daemon start --detach
run catalog add https://localhost:8473/logos-repo.json
run catalog source http
run catalog refresh
"${L[@]}" --json catalog ls | jq -c '.[] | {name,enabled,isDefault,indexUrl,resolveError}'
run package install "$DESK" -y
run --json package deps "$DESK" --recursive
echo "# installed: name variant version"
for m in "$S"/modules/* "$S"/plugins/*; do
  printf '%s %s %s\n' "${m##*/}" "$(cat "$m/variant" 2>/dev/null)" "$(jq -r .version "$m/manifest.json" 2>/dev/null)"
done
run daemon stop
