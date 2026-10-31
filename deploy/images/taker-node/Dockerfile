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
COPY lez-zec-taker-actor /payload/lez-zec-taker-actor
COPY lez-v02-bridge-poc /payload/lez-v02-bridge-poc
# The ZEC corridor provisioners: a prepared ZEC swap is built from live
# chain facts (scripts/zec-provision.sh), not composed at take time.
COPY zec-local-poc-provision /payload/zec-local-poc-provision
COPY zec-local-poc-identity /payload/zec-local-poc-identity
COPY zec-local-poc-transfer /payload/zec-local-poc-transfer
COPY zec-local-poc-chat-draft /payload/zec-local-poc-chat-draft
COPY zec-local-poc-chat-finalize /payload/zec-local-poc-chat-finalize
RUN strip /payload/*

FROM python:3.12-slim-bookworm

LABEL org.logos-co.atomic-swaps.purpose="local-taker-node"

# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl jq libpcsclite1 procps socat tini \
    && rm -rf /var/lib/apt/lists/*

COPY --from=strip --chmod=0555 /payload/lez-taker-node /usr/local/bin/lez-taker-node
COPY --from=strip --chmod=0555 /payload/lez-taker-cli /usr/local/bin/lez-taker-cli
COPY --from=strip --chmod=0555 /payload/lez-taker-chat-gateway /usr/local/bin/lez-taker-chat-gateway
COPY --from=strip --chmod=0555 /payload/lez-taker-registry-init /usr/local/bin/lez-taker-registry-init
COPY --from=strip --chmod=0555 /payload/lez-runtime-healthcheck /usr/local/bin/lez-runtime-healthcheck
COPY --from=strip --chmod=0555 /payload/lez-btc-taker-actor /usr/local/bin/lez-btc-taker-actor
COPY --from=strip --chmod=0555 /payload/lez-zec-taker-actor /usr/local/bin/lez-zec-taker-actor
COPY --from=strip --chmod=0555 /payload/lez-v02-bridge-poc /usr/local/bin/lez-v02-bridge-poc
COPY --from=strip --chmod=0555 /payload/zec-local-poc-provision /usr/local/bin/zec-local-poc-provision
COPY --from=strip --chmod=0555 /payload/zec-local-poc-identity /usr/local/bin/zec-local-poc-identity
COPY --from=strip --chmod=0555 /payload/zec-local-poc-transfer /usr/local/bin/zec-local-poc-transfer
COPY --from=strip --chmod=0555 /payload/zec-local-poc-chat-draft /usr/local/bin/zec-local-poc-chat-draft
COPY --from=strip --chmod=0555 /payload/zec-local-poc-chat-finalize /usr/local/bin/zec-local-poc-chat-finalize
COPY --chmod=0555 node-entrypoint.sh /usr/local/bin/node-entrypoint.sh
RUN groupadd -r -g 4713 lez && useradd -r -u 4713 -g lez -d /var/lib/lez -s /usr/sbin/nologin lez \
    && mkdir -p /var/lib/lez/taker /run/lez/taker \
    && chown -R lez:lez /var/lib/lez /run/lez \
    && chmod 0700 /var/lib/lez/taker /run/lez/taker

COPY --chmod=0444 build-source.txt sidecar-source.txt sidecar-tree.txt sidecar-expected.txt /usr/local/share/lez/
# The staged sidecar must come from the same compat/lez-v0_2-sidecar tree the
# Node binaries were built beside (from-scratch.sh writes both stamps); a
# binary left over from an older checkout fails the build here.
RUN chmod 0755 /usr/local/share/lez \
    && [ "$(cat /usr/local/share/lez/sidecar-tree.txt)" = "$(cat /usr/local/share/lez/sidecar-expected.txt)" ] \
    || { echo "staged sidecar (tree $(cat /usr/local/share/lez/sidecar-tree.txt)) does not match the sidecar sources these Node binaries expect ($(cat /usr/local/share/lez/sidecar-expected.txt)); run from-scratch.sh --only build:sidecar and --only stage" >&2; exit 1; }

USER lez
WORKDIR /var/lib/lez
ENTRYPOINT ["/usr/bin/tini", "--"]
