#!/bin/bash
# bc-stop.sh — stop every Basecamp process (incl. ui-host / logos_host children) in this container.
for p in $(pgrep -f '/opt/basecamp/usr/bin/'); do kill $p 2>/dev/null; done
sleep 4
for p in $(pgrep -f '/opt/basecamp/usr/bin/'); do kill -9 $p 2>/dev/null; done
pgrep -af '/opt/basecamp/usr/bin/' | grep -v defunct | wc -l
