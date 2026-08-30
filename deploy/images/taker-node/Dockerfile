FROM debian:trixie-slim

LABEL org.logos-co.atomic-swaps.purpose="local-taker-node"

# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl jq procps tini \
    && rm -rf /var/lib/apt/lists/*

COPY --chmod=0555 lez-taker-node /usr/local/bin/lez-taker-node
COPY --chmod=0555 lez-taker-cli /usr/local/bin/lez-taker-cli
COPY --chmod=0555 lez-taker-chat-gateway /usr/local/bin/lez-taker-chat-gateway
COPY --chmod=0555 lez-taker-registry-init /usr/local/bin/lez-taker-registry-init
COPY --chmod=0555 lez-runtime-healthcheck /usr/local/bin/lez-runtime-healthcheck
RUN groupadd -r -g 4713 lez && useradd -r -u 4713 -g lez -d /var/lib/lez -s /usr/sbin/nologin lez \
    && mkdir -p /var/lib/lez/taker /run/lez/taker \
    && chown -R lez:lez /var/lib/lez /run/lez \
    && chmod 0700 /var/lib/lez/taker /run/lez/taker

USER lez
WORKDIR /var/lib/lez
ENTRYPOINT ["/usr/bin/tini", "--"]
