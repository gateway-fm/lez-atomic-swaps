#!/usr/bin/env bash
set -euo pipefail

: "${M6_BASECAMP_BIN:?set M6_BASECAMP_BIN to the pinned LogosBasecamp executable}"
: "${M6_BASECAMP_USER_DIR:?set M6_BASECAMP_USER_DIR to an isolated role directory}"

case "$M6_BASECAMP_BIN" in
  /*) ;;
  *) echo "M6_BASECAMP_BIN must be absolute" >&2; exit 2 ;;
esac
case "$M6_BASECAMP_USER_DIR" in
  /*) ;;
  *) echo "M6_BASECAMP_USER_DIR must be absolute" >&2; exit 2 ;;
esac

exec "$M6_BASECAMP_BIN" --user-dir "$M6_BASECAMP_USER_DIR" "$@"
