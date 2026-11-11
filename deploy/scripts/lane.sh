# shellcheck shell=bash
# lane.sh — sourced: the names of regression lane LEZ_LANE (1 when unset).
#
# A lane is one Maker/Taker pair (each party's own Basecamp) with its own
# Bitcoin Core, miner and Zebra; every lane shares the one LEZ network
# (network, sequencer, heartbeat) of lane 1, which is the stack compose.yaml
# runs. Lane 1 is that stack exactly as before: sourcing this for lane 1
# changes no name, path or port. Lane k >= 2 runs as the Compose project
# lez-lane-<k> with compose.lane.yaml on top (scripts/lanes.sh brings it up).
#
# Run from the deploy directory after runtime/runtime.env is loaded. Exports:
#   LEZ_LANE                 the lane number
#   LEZ_CONTAINER_PREFIX     lez | lez-l<k>  (lez-l2-maker-node, lez-l2-zebra, ...)
#   LEZ_LANE_RUNTIME         runtime | runtime/lanes/<k>: the lane's lez/<role>
#                            identities and keys, zebra/, zec/, e2e/, evidence/
#   LEZ_ZEBRA_PORT, LEZ_ZEBRA_URL, LEZ_ZEBRA_HOST
#                            the lane's Zebra on the host / inside the lane
#   LEZ_MAKER_VNC_PORT, LEZ_TAKER_VNC_PORT
#   LEZ_LANE_MAKER_WALLET, LEZ_LANE_TAKER_WALLET   the market wallets it settles as
#   LEZ_STACK_COMPOSE_ARGS   the docker compose arguments of the lane's project
#   and, for k >= 2, LEZ_VOLUME_PREFIX (<the stack's prefix>-l<k>).
# lane_compose (an array) holds LEZ_STACK_COMPOSE_ARGS split into words.

# lane_wallets <k>: "<maker wallet> <taker wallet>" of lane k. Lane 2 settles as
# the second pair of market wallets, already in every genesis and claimed by
# every bootstrap; lanes 3 and up have their own (minted and funded by
# from-scratch.sh build:identities, gen-config.sh and market-bootstrap.sh).
lane_wallets() {
  case "$1" in
    1) echo "maker-munich-01 taker-zurich-01" ;;
    2) echo "maker-basel-02 taker-limmat-02" ;;
    *) printf 'maker-lane%02d taker-lane%02d\n' "$1" "$1" ;;
  esac
}

LEZ_LANE="${LEZ_LANE:-1}"
[[ "$LEZ_LANE" =~ ^[1-9][0-9]?$ ]] || { echo "LEZ_LANE must be a lane number, not '$LEZ_LANE'" >&2; return 64 2>/dev/null || exit 64; }
read -r LEZ_LANE_MAKER_WALLET LEZ_LANE_TAKER_WALLET < <(lane_wallets "$LEZ_LANE")
if [[ "$LEZ_LANE" == 1 ]]; then
  # (the public testnet stack, docs/testnet-run.md, names its own prefix)
  LEZ_CONTAINER_PREFIX="${LEZ_CONTAINER_PREFIX:-lez}"
  LEZ_LANE_RUNTIME=runtime
  LEZ_ZEBRA_HOST=zebra
  LEZ_ZEBRA_PORT=18232
  LEZ_MAKER_VNC_PORT=5901
  LEZ_TAKER_VNC_PORT=5902
  LEZ_STACK_COMPOSE_ARGS="${LEZ_STACK_COMPOSE_ARGS:---env-file runtime/runtime.env}"
else
  LEZ_CONTAINER_PREFIX="lez-l$LEZ_LANE"
  LEZ_LANE_RUNTIME="runtime/lanes/$LEZ_LANE"
  LEZ_ZEBRA_HOST="$LEZ_CONTAINER_PREFIX-zebra"
  LEZ_ZEBRA_PORT=$(( 18232 + 10 * (LEZ_LANE - 1) ))
  LEZ_MAKER_VNC_PORT=$(( 5901 + 10 * (LEZ_LANE - 1) ))
  LEZ_TAKER_VNC_PORT=$(( 5902 + 10 * (LEZ_LANE - 1) ))
  # Once per process: a lane's prefix is derived from the stack's, not stacked.
  if [[ "${LEZ_LANE_VOLUME_BASE:-}" == "" ]]; then LEZ_LANE_VOLUME_BASE="${LEZ_VOLUME_PREFIX:-lez}"; fi
  LEZ_VOLUME_PREFIX="$LEZ_LANE_VOLUME_BASE-l$LEZ_LANE"
  LEZ_STACK_COMPOSE_ARGS="-p lez-lane-$LEZ_LANE --env-file runtime/runtime.env -f compose.yaml -f compose.lane.yaml"
  export LEZ_LANE_VOLUME_BASE LEZ_VOLUME_PREFIX
fi
LEZ_ZEBRA_URL="http://127.0.0.1:$LEZ_ZEBRA_PORT/"
export LEZ_LANE LEZ_CONTAINER_PREFIX LEZ_LANE_RUNTIME LEZ_ZEBRA_HOST LEZ_ZEBRA_PORT LEZ_ZEBRA_URL \
  LEZ_MAKER_VNC_PORT LEZ_TAKER_VNC_PORT LEZ_LANE_MAKER_WALLET LEZ_LANE_TAKER_WALLET LEZ_STACK_COMPOSE_ARGS
# shellcheck disable=SC2034  # used by the scripts that source this
read -r -a lane_compose <<<"$LEZ_STACK_COMPOSE_ARGS"
