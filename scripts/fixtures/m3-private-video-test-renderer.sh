#!/usr/bin/env bash
set -euo pipefail

(( $# == 2 )) || exit 64
readonly tape_file="$1"
readonly video_file="$2"
[[ -s "$tape_file" && ! -L "$tape_file" ]] || exit 65

# Minimal ISO-BMFF signature used only by the contract fixture. Production
# rendering cannot select this executable.
printf '\x00\x00\x00\x18ftypisom\x00\x00\x02\x00isomiso2' >"$video_file"
