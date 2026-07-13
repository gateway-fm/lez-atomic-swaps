#!/usr/bin/env bash

lez_v02_is_missing_channel_response() {
  local status="$1"
  local body_file="$2"

  [[ "$status" == "404" || "$status" == "500" ]] &&
    [[ -f "$body_file" ]] &&
    [[ "$(wc -c <"$body_file")" == "17" ]] &&
    [[ "$(<"$body_file")" == "channel not found" ]]
}
