# shellcheck shell=bash
# lane.sh — sourced: the names of the stack LEZ_STACK_PREFIX (lez when unset)
# and of its regression lane LEZ_LANE (1 when unset).
#
# A stack is one LEZ network (network, sequencer, heartbeat, the explorers),
# its Docker network and Compose project, and its regression lanes. The default
# stack (prefix lez, offset 0) keeps every name, port and path it always had.
# A second, independent stack on the same host runs from its own deploy
# directory (its own runtime/, so its own chains, keys and Node state) under
# another prefix and a port offset: gen-config.sh records both in that
# directory's runtime.env, and this reads them from there when they are not in
# the environment, and refuses a prefix the directory's runtime.env does not
# name (deploy/README.md, "a second stack").
#
# A lane is one Maker/Taker pair (each party's own Basecamp) with its own
# Bitcoin Core, miner and Zebra; every lane shares the one LEZ network
# (network, sequencer, heartbeat) of lane 1, which is the stack compose.yaml
# runs. Lane 1 is that stack exactly as before: sourcing this for lane 1
# changes no name, path or port. Lane k >= 2 runs as the Compose project
# <prefix>-lane-<k> with compose.lane.yaml on top (scripts/lanes.sh brings it up).
#
# Run from the deploy directory after runtime/runtime.env is loaded. Exports:
#   LEZ_STACK_PREFIX         lez | <name>: <prefix>-network, -sequencer, -heartbeat,
#                            -btc-explorer, -lez-explorer (compose.yaml)
#   LEZ_STACK_PORT_OFFSET    0 | added to every port the stack and its lanes
#                            publish on 127.0.0.1
#   LEZ_STACK_PROJECT        <prefix>-swap-stack: lane 1's Compose project
#   LEZ_STACK_NETWORK        <prefix>-swap-chains: the Docker network
#   LEZ_BTC_RPC_PORT, LEZ_SEQUENCER_PORT, LEZ_INDEXER_PORT,
#   LEZ_BTC_EXPLORER_PORT, LEZ_LEZ_EXPLORER_PORT
#                            the stack's ports on the host (18443, 3040, 8779,
#                            3002, 3003 plus the offset)
#   LEZ_STACK_INDEXER_URL    http://127.0.0.1:<indexer port>/
#   LEZ_LANE                 the lane number
#   LEZ_CONTAINER_PREFIX     <prefix> | <prefix>-l<k>  (lez-l2-maker-node, lez-l2-zebra, ...)
#   LEZ_LANE_RUNTIME         runtime | runtime/lanes/<k>: the lane's lez/<role>
#                            identities and keys, zebra/, zec/, e2e/, evidence/
#   LEZ_ZEBRA_PORT, LEZ_ZEBRA_URL, LEZ_ZEBRA_HOST
#                            the lane's Zebra on the host / inside the lane
#   LEZ_MAKER_VNC_PORT, LEZ_TAKER_VNC_PORT
#   LEZ_LANE_MAKER_WALLET, LEZ_LANE_TAKER_WALLET   the market wallets it settles as
#   LEZ_STACK_COMPOSE_ARGS   the docker compose arguments of the lane's project
#   and, for k >= 2, LEZ_VOLUME_PREFIX (<the stack's volume prefix>-l<k>).
# lane_compose (an array) holds LEZ_STACK_COMPOSE_ARGS split into words.

# shellcheck source=host.sh
source "$(dirname "${BASH_SOURCE[0]}")/host.sh"

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

# A release bundle (scripts/package-dist.sh) carries release.env beside
# compose.yaml, naming the published images, and no images/ build contexts.
# Every script that runs the stack sources this file, so the bundle's names are
# taken here wherever the environment does not give them: only start.sh read
# release.env, and a standalone up.sh --fresh-lez or ui-e2e.sh looked for
# lez-*:local and tried to build (#103 item 4). A bundle never builds: unless
# LEZ_IMAGES says otherwise it is start.sh's choice, `present` for a
# developer's own `local` images and `pull` for a release's. A checkout has no
# release.env and is unchanged.
lane_deploy_dir="$(dirname "${BASH_SOURCE[0]}")/.."
if [[ -f "$lane_deploy_dir/release.env" && -f "$lane_deploy_dir/compose.yaml" ]]; then
  for lane_release_key in LEZ_IMAGE_PREFIX LEZ_IMAGE_TAG LEZ_ESCROW_PROGRAM_ID LEZ_RELEASE_COMMIT; do
    if [[ -z "${!lane_release_key:-}" ]]; then
      lane_release_value="$(sed -n "s/^$lane_release_key=//p" "$lane_deploy_dir/release.env" | head -1)"
      if [[ -n "$lane_release_value" ]]; then
        printf -v "$lane_release_key" '%s' "$lane_release_value"
        export "${lane_release_key?}"
      fi
    fi
  done
  if [[ -z "${LEZ_IMAGES:-}" && -n "${LEZ_IMAGE_TAG:-}" ]]; then
    if [[ "$LEZ_IMAGE_TAG" == local ]]; then LEZ_IMAGES=present; else LEZ_IMAGES=pull; fi
    export LEZ_IMAGES
  fi
  unset lane_release_key lane_release_value
fi
unset lane_deploy_dir

# The stack this deploy directory runs: its runtime.env records a prefix and an
# offset other than the default (gen-config.sh); none recorded is the default.
lane_stack_env="$(dirname "${BASH_SOURCE[0]}")/../runtime/runtime.env"
lane_recorded_prefix=lez; lane_recorded_offset=0
if [[ -f "$lane_stack_env" ]]; then
  lane_recorded_prefix="$(sed -n 's/^LEZ_STACK_PREFIX=//p' "$lane_stack_env" | head -1)"
  lane_recorded_offset="$(sed -n 's/^LEZ_STACK_PORT_OFFSET=//p' "$lane_stack_env" | head -1)"
  lane_recorded_prefix="${lane_recorded_prefix:-lez}"; lane_recorded_offset="${lane_recorded_offset:-0}"
  if [[ -n "${LEZ_STACK_PREFIX:-}" && "$LEZ_STACK_PREFIX" != "$lane_recorded_prefix" ]]; then
    echo "this deploy directory runs the stack '$lane_recorded_prefix' (runtime/runtime.env), not '$LEZ_STACK_PREFIX': a second stack runs from a deploy directory of its own" >&2
    return 64 2>/dev/null || exit 64
  fi
fi
LEZ_STACK_PREFIX="${LEZ_STACK_PREFIX:-$lane_recorded_prefix}"
LEZ_STACK_PORT_OFFSET="${LEZ_STACK_PORT_OFFSET:-$lane_recorded_offset}"
[[ "$LEZ_STACK_PREFIX" =~ ^[a-z][a-z0-9]*(-[a-z0-9]+)*$ ]] || { echo "LEZ_STACK_PREFIX must be a lower-case name, not '$LEZ_STACK_PREFIX'" >&2; return 64 2>/dev/null || exit 64; }
# (lanes add 10 per lane to the Zebra and VNC ports: an offset of 100 or more
# keeps two stacks' lanes apart)
[[ "$LEZ_STACK_PORT_OFFSET" =~ ^(0|[1-9][0-9]{0,4})$ ]] && (( LEZ_STACK_PORT_OFFSET <= 40000 )) ||
  { echo "LEZ_STACK_PORT_OFFSET must be 0 to 40000, not '$LEZ_STACK_PORT_OFFSET'" >&2; return 64 2>/dev/null || exit 64; }
LEZ_STACK_PROJECT="$LEZ_STACK_PREFIX-swap-stack"
LEZ_STACK_NETWORK="$LEZ_STACK_PREFIX-swap-chains"
LEZ_BTC_RPC_PORT=$(( 18443 + LEZ_STACK_PORT_OFFSET ))
LEZ_SEQUENCER_PORT=$(( 3040 + LEZ_STACK_PORT_OFFSET ))
LEZ_INDEXER_PORT=$(( 8779 + LEZ_STACK_PORT_OFFSET ))
LEZ_BTC_EXPLORER_PORT=$(( 3002 + LEZ_STACK_PORT_OFFSET ))
LEZ_LEZ_EXPLORER_PORT=$(( 3003 + LEZ_STACK_PORT_OFFSET ))
LEZ_STACK_INDEXER_URL="http://127.0.0.1:$LEZ_INDEXER_PORT/"
export LEZ_STACK_PREFIX LEZ_STACK_PORT_OFFSET LEZ_STACK_PROJECT LEZ_STACK_NETWORK LEZ_BTC_RPC_PORT \
  LEZ_SEQUENCER_PORT LEZ_INDEXER_PORT LEZ_BTC_EXPLORER_PORT LEZ_LEZ_EXPLORER_PORT LEZ_STACK_INDEXER_URL
unset lane_stack_env lane_recorded_prefix lane_recorded_offset

LEZ_LANE="${LEZ_LANE:-1}"
[[ "$LEZ_LANE" =~ ^[1-9][0-9]?$ ]] || { echo "LEZ_LANE must be a lane number, not '$LEZ_LANE'" >&2; return 64 2>/dev/null || exit 64; }
read -r LEZ_LANE_MAKER_WALLET LEZ_LANE_TAKER_WALLET < <(lane_wallets "$LEZ_LANE")
if [[ "$LEZ_LANE" == 1 ]]; then
  # (the public testnet stack, docs/testnet-run.md, names its own prefix)
  LEZ_CONTAINER_PREFIX="${LEZ_CONTAINER_PREFIX:-$LEZ_STACK_PREFIX}"
  LEZ_LANE_RUNTIME=runtime
  LEZ_ZEBRA_HOST=zebra
  LEZ_ZEBRA_PORT=$(( 18232 + LEZ_STACK_PORT_OFFSET ))
  LEZ_MAKER_VNC_PORT=$(( 5901 + LEZ_STACK_PORT_OFFSET ))
  LEZ_TAKER_VNC_PORT=$(( 5902 + LEZ_STACK_PORT_OFFSET ))
  LEZ_STACK_COMPOSE_ARGS="${LEZ_STACK_COMPOSE_ARGS:---env-file runtime/runtime.env}"
else
  LEZ_CONTAINER_PREFIX="$LEZ_STACK_PREFIX-l$LEZ_LANE"
  LEZ_LANE_RUNTIME="runtime/lanes/$LEZ_LANE"
  LEZ_ZEBRA_HOST="$LEZ_CONTAINER_PREFIX-zebra"
  LEZ_ZEBRA_PORT=$(( 18232 + LEZ_STACK_PORT_OFFSET + 10 * (LEZ_LANE - 1) ))
  LEZ_MAKER_VNC_PORT=$(( 5901 + LEZ_STACK_PORT_OFFSET + 10 * (LEZ_LANE - 1) ))
  LEZ_TAKER_VNC_PORT=$(( 5902 + LEZ_STACK_PORT_OFFSET + 10 * (LEZ_LANE - 1) ))
  # Once per process: a lane's prefix is derived from the stack's, not stacked.
  if [[ "${LEZ_LANE_VOLUME_BASE:-}" == "" ]]; then LEZ_LANE_VOLUME_BASE="${LEZ_VOLUME_PREFIX:-$LEZ_STACK_PREFIX}"; fi
  LEZ_VOLUME_PREFIX="$LEZ_LANE_VOLUME_BASE-l$LEZ_LANE"
  LEZ_STACK_COMPOSE_ARGS="-p $LEZ_STACK_PREFIX-lane-$LEZ_LANE --env-file runtime/runtime.env -f compose.yaml -f compose.lane.yaml"
  export LEZ_LANE_VOLUME_BASE LEZ_VOLUME_PREFIX
fi
LEZ_ZEBRA_URL="http://127.0.0.1:$LEZ_ZEBRA_PORT/"
export LEZ_LANE LEZ_CONTAINER_PREFIX LEZ_LANE_RUNTIME LEZ_ZEBRA_HOST LEZ_ZEBRA_PORT LEZ_ZEBRA_URL \
  LEZ_MAKER_VNC_PORT LEZ_TAKER_VNC_PORT LEZ_LANE_MAKER_WALLET LEZ_LANE_TAKER_WALLET LEZ_STACK_COMPOSE_ARGS
# shellcheck disable=SC2034  # used by the scripts that source this
read -r -a lane_compose <<<"$LEZ_STACK_COMPOSE_ARGS"
