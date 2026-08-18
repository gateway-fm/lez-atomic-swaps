#!/bin/bash
# Basecamp UI entrypoint: Xvfb + fluxbox + x11vnc + LogosBasecamp (role-selected)
set -euo pipefail

export HOME=/tmp
export XDG_CACHE_HOME=/tmp/.cache
export XDG_CONFIG_HOME=/tmp/.config
mkdir -p "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" /tmp/.X11-unix 2>/dev/null || true

# one app, both consoles: the default user dir carries both role plugins.
# Set BASECAMP_ROLE=maker|taker to start from a role-isolated dir instead.
role="${BASECAMP_ROLE:-both}"
case "$role" in
  maker) user_dir=/var/lez/basecamp-maker-user ;;
  taker) user_dir=/var/lez/basecamp-taker-user ;;
  both) user_dir="${BASECAMP_USER_DIR:-/var/lez/basecamp-user}" ;;
  *) echo "BASECAMP_ROLE must be maker, taker, or both" >&2; exit 2 ;;
esac

if ! pgrep -x Xvfb >/dev/null; then
  Xvfb :0 -screen 0 "${VNC_GEOMETRY:-1680x1050}"x24 -nolisten tcp >/tmp/xvfb.log 2>&1 &
  sleep 1
fi
if ! pgrep -x fluxbox >/dev/null; then
  fluxbox >/tmp/fluxbox.log 2>&1 &
  sleep 1
fi
if ! pgrep -x x11vnc >/dev/null; then
  # listen on all container interfaces: docker's port proxy dials the
  # container IP, not container-localhost. Safety comes from the compose
  # port binding being 127.0.0.1-only on the host.
  x11vnc -display :0 -forever -shared -nopw -rfbport 5900 -quiet >/tmp/x11vnc.log 2>&1 &
  sleep 1
fi

exec /opt/basecamp/bin/LogosBasecamp --user-dir "$user_dir"
