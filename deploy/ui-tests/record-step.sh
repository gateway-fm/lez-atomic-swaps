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
# verify.mjs keeps the desk open for RECORD_MIN_DWELL seconds after its last
# step (an instantly satisfied wait still shows the rendered desk, and the last
# caption has frames to sit on); one more second here catches the close.
export RECORD_MIN_DWELL="${RECORD_MIN_DWELL:-6}"
node /ui-tests/verify.mjs "$role" || status=$?
sleep 1
kill -INT "$ffmpeg_pid" 2>/dev/null || true
wait "$ffmpeg_pid" 2>/dev/null || true
kill "$xvfb" 2>/dev/null || true
# narration → SRT: each cue lasts until the next one (or 6 s at the end).
# The segment starts where the desk is on screen: the step title is written at
# t=0 while the app is still starting (black, then the launcher); the first
# narration from inside the desk marks the trim point, and the title cue moves
# there with it, so no caption ever sits over a start-up frame.
trim_ms="$(node - "$events" "$srt" <<'JS'
const fs = require("node:fs");
const [events, srt] = process.argv.slice(2);
let cues = [];
try { cues = fs.readFileSync(events, "utf8").split("\n").filter((l) => l.trim()).map((l) => JSON.parse(l)); } catch {}
const title = process.env.STEP_TITLE || "";
const first = cues.findIndex((cue, i) => i > 0 || cue.text !== title);
// The desk narrates "opened" when its items exist; its first paint follows a
// moment later, so the trim point sits RECORD_TRIM_LEAD_MS after that cue.
const trim = first > 0 ? cues[first].t + Number(process.env.RECORD_TRIM_LEAD_MS || 1500) : 0;
cues = cues.map((cue) => ({ ...cue, t: Math.max(0, cue.t - trim) }));
// Cues from before the trim all land on 0 and would stack; every caption gets
// at least 1.5 s of its own, in order, so the lead's narration reads one line
// at a time instead of five at once.
let last = -1500;
cues = cues.map((cue) => { const t = Math.max(cue.t, last + 1500); last = t; return { ...cue, t }; });
const stamp = (ms) => { ms = Math.max(0, Math.round(ms)); const h = Math.floor(ms / 3600000); const m = Math.floor(ms % 3600000 / 60000); const s = Math.floor(ms % 60000 / 1000); const r = ms % 1000; return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")},${String(r).padStart(3, "0")}`; };
let out = "";
cues.forEach((cue, i) => {
  const next = i + 1 < cues.length ? cues[i + 1].t : cue.t + 6000;
  out += `${i + 1}\n${stamp(cue.t)} --> ${stamp(Math.max(next, cue.t + 1500))}\n${cue.text}\n\n`;
});
fs.writeFileSync(srt, out);
process.stdout.write(String(trim));
JS
)"
trim_ms="${trim_ms:-0}"
if [[ -s "$raw" ]]; then
  ffmpeg -hide_banner -loglevel error -y -ss "$(awk "BEGIN{printf \"%.3f\", ${trim_ms}/1000}")" -i "$raw" \
    -vf "subtitles=$srt:force_style='FontName=DejaVu Sans,FontSize=9,PrimaryColour=&H00FFFFFF,OutlineColour=&H00202020,BorderStyle=3,BackColour=&H90000000,Outline=1,Shadow=0,MarginV=14,MarginL=20,MarginR=20'" \
    -c:v libx264 -preset veryfast -crf 24 -pix_fmt yuv420p -an "$out" >>/tmp/ffmpeg-record.log 2>&1 && rm -f "$raw"
fi
exit "$status"
