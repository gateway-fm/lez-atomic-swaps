#!/bin/bash
# quit-basecamp.sh — quit Basecamp 0.3.2 via System Events (it ignores SIGTERM)
osascript -e 'tell application "System Events" to tell process "LogosBasecamp.bin" to set frontmost to true' -e 'tell application "System Events" to keystroke "q" using command down' 2>&1
for i in $(seq 60); do n=$(pgrep -f 'LogosBasecamp.app/Contents/MacOS' | wc -l | tr -d ' '); echo "+${i}s remaining processes: $n"; [[ $n == 0 ]] && break; sleep 1; done
