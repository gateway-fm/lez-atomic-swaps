#!/usr/bin/env bash
set -euo pipefail

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/../.." && pwd)
deck="$script_dir/index.html"
output=${1:-"$repo_root/media/lez-btc-m1-m3-m6-submission-silent.mp4"}
poster="$repo_root/media/screenshots/lez-btc-m1-m3-m6-submission-cover.png"

chrome_default="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
chrome=${CHROME_BIN:-$chrome_default}
ffmpeg_bin=${FFMPEG_BIN:-ffmpeg}

if [[ ! -x "$chrome" ]]; then
  echo "Google Chrome was not found at: $chrome" >&2
  echo "Set CHROME_BIN to a Chromium-compatible executable." >&2
  exit 1
fi

if ! command -v "$ffmpeg_bin" >/dev/null 2>&1; then
  echo "ffmpeg is required to render the MP4." >&2
  exit 1
fi

render_tmp=$(mktemp -d "${TMPDIR:-/tmp}/lez-submission-render.XXXXXX")
trap 'rm -rf -- "$render_tmp"' EXIT

durations=()
while IFS= read -r duration; do
  durations+=("$duration")
done < <(sed -n 's/.*<section class="slide[^>]*data-duration="\([0-9][0-9.]*\)".*/\1/p' "$deck")

if [[ ${#durations[@]} -eq 0 ]]; then
  echo "No timed slides found in: $deck" >&2
  exit 1
fi

offsets=()
start=0
for ((index = 1; index < ${#durations[@]}; index++)); do
  start=$(awk -v start="$start" -v duration="${durations[$((index - 1))]}" \
    'BEGIN { printf "%.3f", start + duration - 0.5 }')
  offsets+=("$start")
done

echo "Rendering ${#durations[@]} HTML slides at 1920x1080..."
for index in "${!durations[@]}"; do
  slide=$((index + 1))
  frame=$(printf '%s/slide-%02d.png' "$render_tmp" "$slide")
  "$chrome" \
    --headless=new \
    --disable-background-networking \
    --disable-component-update \
    --disable-extensions \
    --disable-gpu \
    --hide-scrollbars \
    --allow-file-access-from-files \
    --force-device-scale-factor=1 \
    --window-size=1920,1080 \
    --virtual-time-budget=1200 \
    --screenshot="$frame" \
    "file://$deck?render=1&slide=$slide" >/dev/null 2>&1
done

cp "$render_tmp/slide-01.png" "$poster"

inputs=()
filter=""
for index in "${!durations[@]}"; do
  slide=$((index + 1))
  frame=$(printf '%s/slide-%02d.png' "$render_tmp" "$slide")
  inputs+=( -loop 1 -framerate 30 -t "${durations[$index]}" -i "$frame" )
  filter+="[$index:v]fps=30,settb=AVTB,format=yuv420p[v$index];"
done

previous="v0"
for transition in $(seq 1 $((${#durations[@]} - 1))); do
  offset=${offsets[$((transition - 1))]}
  filter+="[$previous][v$transition]xfade=transition=fade:duration=0.5:offset=$offset[x$transition];"
  previous="x$transition"
done

mkdir -p "$(dirname -- "$output")"
echo "Encoding silent presentation video..."
"$ffmpeg_bin" -hide_banner -loglevel warning -y \
  "${inputs[@]}" \
  -filter_complex "$filter" \
  -map "[$previous]" \
  -an \
  -r 30 \
  -c:v libx264 \
  -preset medium \
  -crf 18 \
  -pix_fmt yuv420p \
  -color_primaries bt709 \
  -color_trc bt709 \
  -colorspace bt709 \
  -movflags +faststart \
  -metadata title="LEZ and Bitcoin — M1, M3, M6 submission" \
  "$output"

echo "Wrote: $output"
echo "Poster: $poster"
