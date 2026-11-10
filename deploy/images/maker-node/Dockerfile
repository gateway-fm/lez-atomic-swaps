# The Maker's Basecamp: the basecamp-ui image (build context "basecamp",
# compose.yaml) plus the Maker Node. Its desk starts the Maker's LEZ signer
# in Basecamp's lez_core and has Basecamp's lez_swap_node launch and supervise
# the Maker Node (node-entrypoint.sh) — the swap engine runs inside the party's
# Basecamp. Only the Maker's executables are here (deploy/role-symmetry.json).
#
# The staged payloads are debug builds; their debug sections are most of the
# bytes. Strip them here so the image ships only what runs.
FROM debian:bookworm-slim AS strip
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends binutils \
    && rm -rf /var/lib/apt/lists/*
COPY lez-maker-node /payload/lez-maker-node
COPY lez-maker-cli /payload/lez-maker-cli
COPY lez-maker-chat-gateway /payload/lez-maker-chat-gateway
COPY lez-runtime-healthcheck /payload/lez-runtime-healthcheck
COPY lez-btc-maker-actor /payload/lez-btc-maker-actor
COPY lez-zec-maker-actor /payload/lez-zec-maker-actor
COPY lez-v02-bridge-poc /payload/lez-v02-bridge-poc
RUN strip /payload/*

# "basecamp" is a build context (compose.yaml: service:basecamp-ui), not a registry image.
# hadolint ignore=DL3006
FROM basecamp

LABEL org.logos-co.atomic-swaps.purpose="local-maker-node"

# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl jq libpcsclite1 procps socat python3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=strip --chmod=0555 /payload/lez-maker-node /usr/local/bin/lez-maker-node
COPY --from=strip --chmod=0555 /payload/lez-maker-cli /usr/local/bin/lez-maker-cli
COPY --from=strip --chmod=0555 /payload/lez-maker-chat-gateway /usr/local/bin/lez-maker-chat-gateway
COPY --from=strip --chmod=0555 /payload/lez-runtime-healthcheck /usr/local/bin/lez-runtime-healthcheck
COPY --from=strip --chmod=0555 /payload/lez-btc-maker-actor /usr/local/bin/lez-btc-maker-actor
COPY --from=strip --chmod=0555 /payload/lez-zec-maker-actor /usr/local/bin/lez-zec-maker-actor
COPY --from=strip --chmod=0555 /payload/lez-v02-bridge-poc /usr/local/bin/lez-v02-bridge-poc
COPY --chmod=0555 node-entrypoint.sh /usr/local/bin/node-entrypoint.sh
# The state directories the Compose volumes mount here (maker_state, zec_maker,
# delivery_dir, ...): a new named volume takes the owner and mode of the image
# directory it is first mounted on, so no init container prepares them.
RUN groupadd -r -g 4713 lez && useradd -r -u 4713 -g lez -d /var/lib/lez -s /usr/sbin/nologin lez \
    && mkdir -p /var/lib/lez/maker /var/lib/lez/zec /run/lez/maker /run/lez-wallet/maker /delivery \
    && chown -R lez:lez /var/lib/lez /run/lez /run/lez-wallet /delivery \
    && chmod 0700 /var/lib/lez/maker /var/lib/lez/zec /run/lez/maker /run/lez-wallet/maker /delivery

COPY --chmod=0444 build-source.txt sidecar-source.txt sidecar-tree.txt sidecar-expected.txt /usr/local/share/lez/
# The staged sidecar must come from the same compat/lez-v0_2-sidecar tree the
# Node binaries were built beside (from-scratch.sh writes both stamps); a
# binary left over from an older checkout fails the build here.
RUN chmod 0755 /usr/local/share/lez \
    && [ "$(cat /usr/local/share/lez/sidecar-tree.txt)" = "$(cat /usr/local/share/lez/sidecar-expected.txt)" ] \
    || { echo "staged sidecar (tree $(cat /usr/local/share/lez/sidecar-tree.txt)) does not match the sidecar sources these Node binaries expect ($(cat /usr/local/share/lez/sidecar-expected.txt)); run from-scratch.sh --only build:sidecar and --only stage" >&2; exit 1; }

USER lez
WORKDIR /var/lib/lez
# Basecamp's entrypoint (the base image's): Xvfb and VNC, the role's desk opened, then
# the desk starts the signer and the Node.
ENTRYPOINT ["/entrypoint.sh"]
