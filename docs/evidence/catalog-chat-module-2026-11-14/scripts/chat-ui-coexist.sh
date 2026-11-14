#!/opt/homebrew/bin/bash
# chat-ui-coexist.sh — a user who also installs Logos's own Chat app (chat_ui):
# A) the Maker desk, then chat_ui; B) chat_ui, then the Maker desk. Each in a
# fresh logosctl 0.3.2 session with the local catalog beside the official one.
set -u
L=<home>/tmp/logosctl-0.3.2-macos/x/logosctl-aarch64-macos/bin/logosctl
CA=<scratch>/previous/tls/bundle.pem
plan() { jq -c 'if .changes then [.changes[] | "\(.action) \(.name) \(.fromVersion) -> \(.toVersion)"] else . end' 2>/dev/null; }
deps() { jq -c '[.[] | "\(.name) \(.requiredVersion // "*") -> \(.version) \(.status)"]' 2>/dev/null; }
for order in "lez_atomic_swap_maker chat_ui" "chat_ui lez_atomic_swap_maker"; do
  set -- $order
  S=<scratch>/sess/coexist-$1; rm -rf "$S"; mkdir -p "$S"
  lc() { env -i HOME="$HOME" USER="$USER" TMPDIR="$TMPDIR" PATH=/usr/bin:/bin SSL_CERT_FILE="$CA" "$L" --config-dir "$S" "$@"; }
  echo "## install $1, then $2"
  lc daemon start --detach >/dev/null; lc catalog add https://localhost:8473/logos-repo.json >/dev/null; lc catalog source http >/dev/null; lc catalog refresh >/dev/null
  for p in "$@"; do
    echo "\$ logosctl package install $p -y"; lc package install "$p" -y 2>&1 | plan
  done
  for p in "$@"; do echo "\$ logosctl package deps $p --recursive"; lc --json package deps "$p" --recursive 2>&1 | deps; done
  echo "installed: $(for m in "$S"/modules/chat_module "$S"/modules/delivery_module; do printf '%s %s; ' "${m##*/}" "$(jq -r .version "$m/manifest.json")"; done)"
  lc daemon stop >/dev/null
done
