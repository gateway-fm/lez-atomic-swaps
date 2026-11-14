#!/opt/homebrew/bin/bash
# mac-bc-open.sh <user dir> <log> <button>... — Basecamp 0.3.2 on <user dir>
# (no Node environment), click each named sidebar button (System Events
# accessibility), then quit; the log is Basecamp's stdout/stderr.
set -u
U="${1:?user dir}" LOG="${2:?log}"; shift 2
APP=<home>/tmp/lez-darwin-variant-work/basecamp-032/app/LogosBasecamp.app
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
env -i HOME="$HOME" USER="$USER" TMPDIR="$TMPDIR" PATH=/usr/bin:/bin:/usr/sbin:/sbin \
  SSL_CERT_FILE=<scratch>/previous/tls/bundle.pem QT_LOGGING_RULES="*.debug=true;qt.*.debug=false" \
  "$APP/Contents/MacOS/LogosBasecamp" --user-dir "$U" > "$LOG" 2>&1 < /dev/null &
echo "basecamp pid $! started $(date -u +%TZ)"
sleep 30
for b in "$@"; do osascript -l JavaScript "$here/ax-click.js" "$b"; sleep 20; done
"$here/quit-basecamp.sh" | tail -1
