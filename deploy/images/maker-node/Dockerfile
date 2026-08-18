FROM debian:trixie-slim

LABEL org.logos-co.atomic-swaps.purpose="local-maker-taker-node"

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl jq procps tini \
    && rm -rf /var/lib/apt/lists/*

COPY --chmod=0555 lez-maker-daemon /usr/local/bin/lez-maker-daemon
COPY --chmod=0555 lez-maker /usr/local/bin/lez-maker
COPY --chmod=0555 lez-taker /usr/local/bin/lez-taker
COPY --chmod=0555 lez-taker-service /usr/local/bin/lez-taker-service
COPY --chmod=0555 lez-taker-registry-init /usr/local/bin/lez-taker-registry-init
COPY --chmod=0555 zec-local-poc-provision /usr/local/bin/zec-local-poc-provision
COPY --chmod=0555 zec-local-poc-chat-draft /usr/local/bin/zec-local-poc-chat-draft
COPY --chmod=0555 zec-local-poc-chat-finalize /usr/local/bin/zec-local-poc-chat-finalize
COPY --chmod=0555 zec-ui-fixture /usr/local/bin/zec-ui-fixture

RUN groupadd -r -g 4713 lez && useradd -r -u 4713 -g lez -d /var/lib/lez -s /usr/sbin/nologin lez \
    && mkdir -p /var/lib/lez /run/lez \
    && chown -R lez:lez /var/lib/lez /run/lez

USER lez
WORKDIR /var/lib/lez
ENTRYPOINT ["/usr/bin/tini", "--"]
