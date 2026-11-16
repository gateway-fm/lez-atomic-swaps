#!/bin/bash
# bc-start.sh <user dir> <log> [display] — start Basecamp 0.3.2 (extracted AppImage)
# on an Xvfb display with software Qt rendering, detached, stdin closed.
set -u
U="${1:?user dir}" LOG="${2:?log}" DPY="${3:-:99}"
if ! pgrep -f "Xvfb $DPY" >/dev/null; then
  Xvfb "$DPY" -screen 0 1600x1000x24 -nolisten tcp >/tmp/xvfb${DPY#:}.log 2>&1 &
  sleep 2
fi
export DISPLAY="$DPY" QT_QUICK_BACKEND=software LIBGL_ALWAYS_SOFTWARE=1 QT_QPA_PLATFORM=xcb QT_LOGGING_RULES="*.debug=true;qt.*.debug=false"
mkdir -p "$U"
nohup /opt/basecamp/AppRun --user-dir "$U" >"$LOG" 2>&1 </dev/null &
echo "basecamp pid $!"
