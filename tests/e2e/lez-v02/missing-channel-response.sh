#!/usr/bin/env bash

lez_v02_is_missing_channel_response() {
  local status="$1"
  local body_file="$2"
  local body_size

  [[ "$status" == "404" || "$status" == "500" ]] || return 1
  [[ -f "$body_file" ]] || return 1

  body_size="$(wc -c <"$body_file" | tr -d '[:space:]')"

  [[ "$body_size" == "17" ]] &&
    [[ "$(<"$body_file")" == "channel not found" ]]
}
