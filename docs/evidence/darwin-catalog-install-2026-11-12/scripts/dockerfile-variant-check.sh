#!/bin/sh
# The variant selection of the maker-node Dockerfile, run on a merged package.
set -eu; unpacked="$(mktemp -d)"; tar -xzf "$1" -C "$unpacked"; \
    case "$(uname -m)" in aarch64|arm64) variant=linux-arm64 ;; x86_64|amd64) variant=linux-amd64 ;; \
      *) echo "no package variant for $(uname -m)" >&2; exit 1 ;; esac; \
    jq -e --arg v "$variant" '.main[$v] | strings' "$unpacked/manifest.json" >/dev/null \
      || { echo "lez_maker_node.lgx has no $variant variant" >&2; exit 1; }; \
    echo "$(uname -m): installs $variant ($(jq -r --arg v "$variant" '.main[$v]' "$unpacked/manifest.json"))"; ls "$unpacked/variants/$variant/bin" | head -3
