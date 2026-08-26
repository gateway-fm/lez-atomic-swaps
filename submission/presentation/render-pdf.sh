#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/../.." && pwd -P)"
input_html="$repo_root/media/lez-btc-m1-m3-m6-submission.html"
output_pdf="${1:-$repo_root/media/lez-btc-m1-m3-m6-submission.pdf}"

if [[ -n "${CHROME_BIN:-}" ]]; then
  chrome="$CHROME_BIN"
elif command -v google-chrome >/dev/null 2>&1; then
  chrome="$(command -v google-chrome)"
elif command -v chromium >/dev/null 2>&1; then
  chrome="$(command -v chromium)"
elif [[ -x "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" ]]; then
  chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
else
  echo "Chrome/Chromium not found; set CHROME_BIN" >&2
  exit 1
fi

[[ -f "$input_html" ]] || {
  echo "missing standalone deck: $input_html" >&2
  echo "run: node submission/presentation/build-standalone.mjs" >&2
  exit 1
}

mkdir -p "$(dirname "$output_pdf")"
profile_dir="$(mktemp -d "${TMPDIR:-/tmp}/lez-deck-pdf.XXXXXX")"
cleanup() {
  case "$profile_dir" in
    */lez-deck-pdf.*) rm -rf -- "$profile_dir" ;;
    *) echo "refusing unexpected cleanup path: $profile_dir" >&2 ;;
  esac
}
trap cleanup EXIT

"$chrome" \
  --headless=new \
  --disable-gpu \
  --no-pdf-header-footer \
  --print-to-pdf="$output_pdf" \
  --user-data-dir="$profile_dir" \
  "file://$input_html"

[[ -s "$output_pdf" ]] || {
  echo "PDF renderer did not create output: $output_pdf" >&2
  exit 1
}

echo "Wrote PDF presentation: $output_pdf"
