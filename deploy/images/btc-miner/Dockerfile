FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        curl jq \
    && rm -rf /var/lib/apt/lists/*

USER 65532:65532
ENTRYPOINT ["/bin/bash", "/scripts/btc-miner.sh"]
