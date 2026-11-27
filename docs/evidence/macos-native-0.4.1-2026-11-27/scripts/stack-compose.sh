#!/usr/bin/env bash
# stack-compose.sh <stack dir> <compose args...> — docker compose for the
# stack in <stack dir> with the environment its own scripts give it
# (release.env, scripts/lane.sh, runtime/runtime.env), so a single service
# can be recreated with one setting changed, e.g.
#   LEZ_XMR_FUNDING_WALLET_PORT=18183 stack-compose.sh <dir> --profile monero up -d --no-deps xmr-funding-wallet
# Extra compose files: COMPOSE_EXTRA="-f <override.yaml>".
set -euo pipefail
S="${1:?stack dir}"; shift
cd "$S"
keep_funding_port="${LEZ_XMR_FUNDING_WALLET_PORT:-}"
set -a
# shellcheck source=/dev/null
source release.env
# shellcheck source=/dev/null
source runtime/runtime.env
set +a
# shellcheck source=/dev/null
source scripts/lane.sh
[[ -z "$keep_funding_port" ]] || export LEZ_XMR_FUNDING_WALLET_PORT="$keep_funding_port"
# shellcheck disable=SC2155
export LEZ_HOST_UID="$(id -u)" LEZ_HOST_GID="$(id -g)" LEZ_MARKET_ROOT="$S/market"
# shellcheck disable=SC2086
exec docker compose --env-file runtime/runtime.env -f compose.yaml ${COMPOSE_EXTRA:-} "$@"
