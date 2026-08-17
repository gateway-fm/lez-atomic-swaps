FROM node:20-bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        python3 make g++ libkrb5-dev \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g btc-rpc-explorer@3.4.0 && npm cache clean --force

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
