#!/bin/bash
# shot.sh <name> [display] — screenshot of the Xvfb display into /w/shots/<name>.png
import -display "${2:-:99}" -window root "/w/shots/$1.png" && echo "/w/shots/$1.png"
