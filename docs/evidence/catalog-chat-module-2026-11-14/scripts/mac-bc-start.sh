#!/opt/homebrew/bin/bash
# mac-bc-start.sh <user dir> <log> — Basecamp 0.3.2 on <user dir>, no Node
# environment, detached; the local catalog's CA is trusted (SSL_CERT_FILE).
set -u
APP=$HOME/tmp/lez-darwin-variant-work/basecamp-032/app/LogosBasecamp.app
env -i HOME="$HOME" USER="$USER" TMPDIR="$TMPDIR" PATH=/usr/bin:/bin:/usr/sbin:/sbin \
  SSL_CERT_FILE=${SCRATCH:?}/previous/tls/bundle.pem QT_LOGGING_RULES="*.debug=true;qt.*.debug=false" \
  "$APP/Contents/MacOS/LogosBasecamp" --user-dir "${1:?}" > "${2:?}" 2>&1 < /dev/null &
echo "basecamp pid $! started $(date -u +%TZ)"
