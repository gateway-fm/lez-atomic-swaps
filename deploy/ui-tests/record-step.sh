#!/usr/bin/env bash
# record-step.sh — one desk step of ui-e2e.sh --record, inside the UI image:
# a private virtual display, an ffmpeg screen grab of it, the real desk driven
# on it by verify.mjs, then the narration verify.mjs wrote burned in as
# subtitles. Output: /recordings/<segment>.mp4 (silent).
#
#   record-step.sh <maker|taker> <segment-name>   (env: everything verify.mjs reads)
set -euo pipefail
role="$1"; segment="$2"
export HOME=/tmp XDG_CACHE_HOME=/tmp/.cache XDG_CONFIG_HOME=/tmp/.config
mkdir -p "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME" /recordings /tmp/.X11-unix 2>/dev/null || true
geometry="${RECORD_GEOMETRY:-1600x1000}"
export DISPLAY=:1 UI_PLATFORM=xcb UI_GEOMETRY="${geometry}+0+0"
rm -f /tmp/.X1-lock /tmp/.X11-unix/X1
Xvfb :1 -screen 0 "${geometry}x24" -nolisten tcp >/tmp/xvfb1.log 2>&1 &
xvfb=$!
sleep 1
raw="/recordings/$segment.raw.mp4"; events="/recordings/$segment.events"; srt="/recordings/$segment.srt"; out="/recordings/$segment.mp4"
rm -f "$raw" "$events" "$srt" "$out"
export NARRATION_FILE="$events"
NARRATION_T0="$(date +%s%3N)"; export NARRATION_T0
ffmpeg -hide_banner -loglevel error -f x11grab -video_size "$geometry" -framerate "${RECORD_FPS:-6}" -i :1 \
  -c:v libx264 -preset veryfast -crf 26 -pix_fmt yuv420p -g 30 "$raw" >/tmp/ffmpeg-record.log 2>&1 &
ffmpeg_pid=$!
status=0
node /ui-tests/verify.mjs "$role" || status=$?
sleep 2
kill -INT "$ffmpeg_pid" 2>/dev/null || true
wait "$ffmpeg_pid" 2>/dev/null || true
kill "$xvfb" 2>/dev/null || true
# narration → SRT: each cue lasts until the next one (or 6 s at the end)
node - "$events" "$srt" <<'JS'
const fs = require("node:fs");
const [events, srt] = process.argv.slice(2);
let cues = [];
try { cues = fs.readFileSync(events, "utf8").split("\n").filter((l) => l.trim()).map((l) => JSON.parse(l)); } catch {}
const stamp = (ms) => { ms = Math.max(0, Math.round(ms)); const h = Math.floor(ms / 3600000); const m = Math.floor(ms % 3600000 / 60000); const s = Math.floor(ms % 60000 / 1000); const r = ms % 1000; return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")},${String(r).padStart(3, "0")}`; };
let out = "";
cues.forEach((cue, i) => {
  const next = i + 1 < cues.length ? cues[i + 1].t : cue.t + 6000;
  out += `${i + 1}\n${stamp(cue.t)} --> ${stamp(Math.max(next, cue.t + 1500))}\n${cue.text}\n\n`;
});
fs.writeFileSync(srt, out);
JS
if [[ -s "$raw" ]]; then
  ffmpeg -hide_banner -loglevel error -y -i "$raw" \
    -vf "subtitles=$srt:force_style='FontName=DejaVu Sans,FontSize=9,PrimaryColour=&H00FFFFFF,OutlineColour=&H00202020,BorderStyle=3,BackColour=&H90000000,Outline=1,Shadow=0,MarginV=14,MarginL=20,MarginR=20'" \
    -c:v libx264 -preset veryfast -crf 24 -pix_fmt yuv420p -an "$out" >>/tmp/ffmpeg-record.log 2>&1 && rm -f "$raw"
fi
exit "$status"
