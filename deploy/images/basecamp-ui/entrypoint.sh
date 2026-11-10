#!/bin/bash
# Basecamp entrypoint: Xvfb + fluxbox + x11vnc + LogosBasecamp (role-selected)
set -euo pipefail

export HOME=/tmp
export XDG_CACHE_HOME=/tmp/.cache
export XDG_CONFIG_HOME=/tmp/.config
mkdir -p "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" /tmp/.X11-unix 2>/dev/null || true

# A SIGKILLed stop leaves Xvfb's stale lock behind; the next start then
# refuses display :0 ("Server is already active") and the UI aborts.
# The lock is only meaningful while an Xvfb holds it, so clear it here.
if ! pgrep -x Xvfb >/dev/null; then
  rm -f /tmp/.X0-lock /tmp/.X11-unix/X0 2>/dev/null || true
fi

# The role's Basecamp starts from its role-isolated user dir (one desk:
# compose.yaml maker-node, taker-node).
role="${BASECAMP_ROLE:-}"
case "$role" in
  maker) user_dir=/var/lez/basecamp-maker-user ;;
  taker) user_dir=/var/lez/basecamp-taker-user ;;
  # The LEZ network node or indexer's Basecamp (compose.yaml bedrock, indexer):
  # its user directory, where the modules keep their state, lives on the
  # service's state volume; the LEZ Network desk comes from the image.
  network)
    user_dir="${BASECAMP_USER_DIR:-/var/lez/basecamp-network-user}"
    mkdir -p "$user_dir/plugins"
    rm -rf "$user_dir/plugins/lez_network"
    cp -a /var/lez-assets/network-user/plugins/lez_network "$user_dir/plugins/"
    ;;
  *) echo "BASECAMP_ROLE must be maker, taker or network" >&2; exit 2 ;;
esac

if ! pgrep -x Xvfb >/dev/null; then
  Xvfb :0 -screen 0 "${VNC_GEOMETRY:-1680x1050}"x24 -nolisten tcp >/tmp/xvfb.log 2>&1 &
fi
# Everything below draws on display :0: wait for its socket, not a fixed time.
for _ in $(seq 100); do [[ -S /tmp/.X11-unix/X0 ]] && break; sleep 0.1; done
if ! pgrep -x fluxbox >/dev/null; then
  fluxbox >/tmp/fluxbox.log 2>&1 &
fi
if ! pgrep -x x11vnc >/dev/null; then
  # listen on all container interfaces: docker's port proxy dials the
  # container IP, not container-localhost. Safety comes from the compose
  # port binding being 127.0.0.1-only on the host.
  x11vnc -storepasswd "${VNC_PASSWORD:-lezswap}" /tmp/vnc.pass >/dev/null
  chmod 0600 /tmp/vnc.pass
  x11vnc -display :0 -forever -shared -rfbauth /tmp/vnc.pass -rfbport 5900 -quiet >/tmp/x11vnc.log 2>&1 &
fi

# A role's party runs its own Basecamp (compose.yaml maker-node, taker-node):
# its desk hosts the role's LEZ signer in Basecamp's lez_core and has
# Basecamp's lez_swap_node launch the role's Node, both when the desk opens;
# the same desk is the one the party uses over VNC and the UI tests drive.
# With either configured, open this Basecamp's desk once the launcher shows it.
# The LEZ network node or indexer's Basecamp opens its LEZ Network desk the same way.
# open-desks.mjs logs to /tmp/open-desks.log; "1 passed" there means the desk is open.
if [[ -n "${LEZ_MAKER_SIGNER_SOCKET:-}${LEZ_TAKER_SIGNER_SOCKET:-}${LEZ_MAKER_NODE_LAUNCHER:-}${LEZ_TAKER_NODE_LAUNCHER:-}${LEZ_NETWORK_NODE_CONFIG:-}${LEZ_INDEXER_CONFIG:-}" ]]; then
  rm -f /tmp/open-desks.log
  # open-desks.mjs connects to Basecamp's QML inspector once and gives up when
  # nothing listens yet: start it as soon as the inspector does.
  (for _ in $(seq 600); do (exec 3<>"/dev/tcp/localhost/${QML_INSPECTOR_PORT:-3768}") 2>/dev/null && break; sleep 0.2; done
   node /opt/lez-open-desks.mjs >/tmp/open-desks.log 2>&1) &
fi

exec /opt/basecamp/bin/LogosBasecamp --user-dir "$user_dir"
