#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../../.."
source tests/e2e/lez-v02/missing-channel-response.sh

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

printf 'channel not found' >"${tmp_dir}/missing"
printf 'internal database failure' >"${tmp_dir}/internal"
printf 'channel not found\n' >"${tmp_dir}/newline"

lez_v02_is_missing_channel_response 500 "${tmp_dir}/missing"
lez_v02_is_missing_channel_response 404 "${tmp_dir}/missing"
if lez_v02_is_missing_channel_response 500 "${tmp_dir}/internal"; then
  echo "generic HTTP 500 must not satisfy missing-channel readiness" >&2
  exit 1
fi
if lez_v02_is_missing_channel_response 500 "${tmp_dir}/newline"; then
  echo "missing-channel readiness requires the exact audited response body" >&2
  exit 1
fi
if lez_v02_is_missing_channel_response 200 "${tmp_dir}/missing"; then
  echo "HTTP 200 must not be treated as a missing channel" >&2
  exit 1
fi

echo "LEZ v0.2 missing-channel response policy passed"
