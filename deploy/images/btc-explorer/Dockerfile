FROM node:20-bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        python3 make g++ libkrb5-dev \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g btc-rpc-explorer@3.4.0 && npm cache clean --force

# On a long-lived regtest chain (150-block halving interval, height 6000+) the
# subsidy era table is exhausted: blockRewardFunction returns undefined, which
# crashes the homepage supply estimate and block-page coinbase math with
# "[DecimalError] Invalid argument: undefined". Past the last era the subsidy
# is simply zero.
RUN sed -i \
      's|return blockRewardEras\[index\];|return (index < blockRewardEras.length) ? blockRewardEras[index] : 0;|' \
      /usr/local/lib/node_modules/btc-rpc-explorer/app/coins/btc.js \
    && grep -c "blockRewardEras.length" /usr/local/lib/node_modules/btc-rpc-explorer/app/coins/btc.js

# The homepage halving card dereferences nextHalvingData before checking it —
# guard it for the same exhausted-halving-schedule case.
RUN sed -i \
      -e 's|let halvingProgressPercent = new Decimal(100).minus(new Decimal(nextHalvingData.blocksUntilNextHalving).dividedBy(nextHalvingData.halvingBlockInterval).times(100));|let halvingProgressPercent = (nextHalvingData \&\& nextHalvingData.blocksUntilNextHalving != null \&\& nextHalvingData.halvingBlockInterval) ? new Decimal(100).minus(new Decimal(nextHalvingData.blocksUntilNextHalving).dividedBy(nextHalvingData.halvingBlockInterval).times(100)) : null;|' \
      -e 's|if (nextHalvingData \&\& halvingProgressPercent > 65)|if (nextHalvingData \&\& halvingProgressPercent \&\& halvingProgressPercent > 65)|' \
      /usr/local/lib/node_modules/btc-rpc-explorer/views/includes/index-network-summary.pug \
    && grep -c "halvingProgressPercent \&\& halvingProgressPercent > 65" \
      /usr/local/lib/node_modules/btc-rpc-explorer/views/includes/index-network-summary.pug

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
