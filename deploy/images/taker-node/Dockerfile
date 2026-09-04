# The staged payloads are debug builds; their debug sections are most of the
# bytes. Strip them here so the image ships only what runs.
FROM debian:bookworm-slim AS strip
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends binutils \
    && rm -rf /var/lib/apt/lists/*
COPY lez-taker-node /payload/lez-taker-node
COPY lez-taker-cli /payload/lez-taker-cli
COPY lez-taker-chat-gateway /payload/lez-taker-chat-gateway
COPY lez-taker-registry-init /payload/lez-taker-registry-init
COPY lez-runtime-healthcheck /payload/lez-runtime-healthcheck
COPY lez-btc-taker-actor /payload/lez-btc-taker-actor
COPY lez-v02-bridge-poc /payload/lez-v02-bridge-poc
RUN strip /payload/*

FROM python:3.12-slim-bookworm

LABEL org.logos-co.atomic-swaps.purpose="local-taker-node"

# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl jq procps socat tini \
    && rm -rf /var/lib/apt/lists/*

COPY --from=strip --chmod=0555 /payload/lez-taker-node /usr/local/bin/lez-taker-node
COPY --from=strip --chmod=0555 /payload/lez-taker-cli /usr/local/bin/lez-taker-cli
COPY --from=strip --chmod=0555 /payload/lez-taker-chat-gateway /usr/local/bin/lez-taker-chat-gateway
COPY --from=strip --chmod=0555 /payload/lez-taker-registry-init /usr/local/bin/lez-taker-registry-init
COPY --from=strip --chmod=0555 /payload/lez-runtime-healthcheck /usr/local/bin/lez-runtime-healthcheck
COPY --from=strip --chmod=0555 /payload/lez-btc-taker-actor /usr/local/bin/lez-btc-taker-actor
COPY --from=strip --chmod=0555 /payload/lez-v02-bridge-poc /usr/local/bin/lez-v02-bridge-poc
COPY --chmod=0555 node-entrypoint.sh /usr/local/bin/node-entrypoint.sh
RUN groupadd -r -g 4713 lez && useradd -r -u 4713 -g lez -d /var/lib/lez -s /usr/sbin/nologin lez \
    && mkdir -p /var/lib/lez/taker /run/lez/taker \
    && chown -R lez:lez /var/lib/lez /run/lez \
    && chmod 0700 /var/lib/lez/taker /run/lez/taker

USER lez
WORKDIR /var/lib/lez
ENTRYPOINT ["/usr/bin/tini", "--"]
