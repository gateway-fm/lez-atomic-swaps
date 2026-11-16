FROM node:24.22.3-bookworm-slim AS install

# npm stops at btc-rpc-explorer 3.4.0 (June 2023), whose zeromq 5 addon no
# longer compiles against Node 22+ headers. Install upstream v3.5.1 from its
# GitHub tag commit instead: it depends on zeromq 6 (N-API, prebuilt binaries)
# and needs no compiler toolchain. git is only needed here, to fetch its two
# GitHub-hosted dependencies; the runtime stage below does not carry it.
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates git \
    && rm -rf /var/lib/apt/lists/*

ARG BTC_RPC_EXPLORER_REF=8ed77ab225f5507c521b570d5240624de597ad44
# Install scripts are ignored (npm 11 already skips them by default): zeromq
# ships its N-API binaries prebuilt and is only required by dead code, and the
# optional event-loop-stats addon is skipped, which the explorer tolerates.
# The tarball URL is pinned to a full commit sha, which DL3016 does not detect.
# hadolint ignore=DL3016
RUN npm install -g --ignore-scripts "https://codeload.github.com/janoside/btc-rpc-explorer/tar.gz/${BTC_RPC_EXPLORER_REF}" \
    && npm cache clean --force \
    && grep -c '"version": "3.5.1"' /usr/local/lib/node_modules/btc-rpc-explorer/package.json

# On a long-lived regtest chain (150-block halving interval, height 6000+) the
# subsidy era table is exhausted: blockRewardFunction returns undefined, which
# crashes the homepage supply estimate and block-page coinbase math with
# "[DecimalError] Invalid argument: undefined". Past the last era the subsidy
# is simply zero.
RUN sed -i \
      's|return blockRewardEras\[index\];|return (index < blockRewardEras.length) ? blockRewardEras[index] : 0;|' \
      /usr/local/lib/node_modules/btc-rpc-explorer/app/coins/btc.js \
    && grep -c "blockRewardEras.length" /usr/local/lib/node_modules/btc-rpc-explorer/app/coins/btc.js

# Past the last halving (regtest height >= 4800) nextHalvingEstimates returns
# no blocksUntilNextHalving, and the homepage halving card still feeds it to
# Decimal — guard it for the same exhausted-halving-schedule case.
RUN sed -i \
      's|^if (nextHalvingData)$|if (nextHalvingData \&\& nextHalvingData.blocksUntilNextHalving != null \&\& nextHalvingData.halvingBlockInterval)|' \
      /usr/local/lib/node_modules/btc-rpc-explorer/views/includes/index-network-summary.pug \
    && grep -c "nextHalvingData.blocksUntilNextHalving != null" \
      /usr/local/lib/node_modules/btc-rpc-explorer/views/includes/index-network-summary.pug

FROM node:24.22.3-bookworm-slim

COPY --from=install /usr/local/lib/node_modules/btc-rpc-explorer \
     /usr/local/lib/node_modules/btc-rpc-explorer
RUN ln -s ../lib/node_modules/btc-rpc-explorer/bin/cli.js /usr/local/bin/btc-rpc-explorer

ENV BTCEXP_HOST=0.0.0.0 \
    BTCEXP_PORT=3002 \
    BTCEXP_COOKIE=false \
    BTCEXP_NO_RATES=true \
    BTCEXP_RPC_PROTO=http \
    BTCEXP_NETWORK=regtest \
    BTCEXP_ADDRESS_API=none

EXPOSE 3002
USER node
CMD ["btc-rpc-explorer"]
