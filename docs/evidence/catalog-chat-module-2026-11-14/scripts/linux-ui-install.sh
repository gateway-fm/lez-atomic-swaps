#!/bin/bash
# ui-install.sh <user dir> <log> <tile x> <prefix> — fresh Basecamp 0.3.2 under
# Xvfb; Applications → Manage Repositories → add the local catalog, Download
# source "HTTP only"; Applications → the LEZ tile at <tile x> → Install (one
# click) → Launch. Screenshots /w/shots/<prefix>-*.png. Coordinates are those
# of the 1600x1000 display.
set -u
U="${1:?}" LOG="${2:?}" X="${3:?}" P="${4:?}"
export DISPLAY=:99
bash /w/bc-stop.sh >/dev/null; rm -rf "$U"; bash /w/bc-start.sh "$U" "$LOG"; sleep 30
xdotool mousemove 840 294 click 1; sleep 6                       # Discover Applications
xdotool mousemove 1329 165 click 1; sleep 4                      # Manage Repositories
xdotool mousemove 890 397 click 1; sleep 1; xdotool type --delay 20 "https://localhost:8473/logos-repo.json"; sleep 1
xdotool mousemove 1485 397 click 1; sleep 6                      # Add
xdotool mousemove 465 287 click 1; sleep 2; xdotool mousemove 398 407 click 1; sleep 2   # HTTP only
bash /w/shot.sh "$P-1-repositories"
xdotool mousemove 44 737 click 1; sleep 6                        # Applications
bash /w/shot.sh "$P-2-applications"
xdotool mousemove "$X" 815 click 1; sleep 6                      # the tile
bash /w/shot.sh "$P-3-dialog"
echo "install clicked $(date -u +%TZ)"; xdotool mousemove 1000 385 click 1; sleep 90
bash /w/shot.sh "$P-4-installed"
xdotool mousemove 1000 417 click 1; sleep 25                     # Launch
bash /w/shot.sh "$P-5-launched"
for m in "$U"/modules/* "$U"/plugins/*; do echo "${m##*/} $(cat $m/variant 2>/dev/null) $(jq -r .version $m/manifest.json 2>/dev/null)"; done
