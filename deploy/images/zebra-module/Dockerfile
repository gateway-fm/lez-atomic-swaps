# The devnet's Zcash Regtest node in a Basecamp: the basecamp-ui image's
# Basecamp (build context "basecamp", compose.yaml) hosting zebrad_module
# (logos-zebrad-module, Zebra 7.1.0 in-process; a runtime module of its
# bundle, built and pinned by from-scratch.sh build:zebrad-module). Its Zcash
# Node desk configures the module from LEZ_ZEBRA_NETWORK and LEZ_ZEBRA_CONFIG
# (gen-config.sh renders runtime/zebra/zebrad-module.json) and starts it; its
# Reset chain starts a new Regtest chain (reset-chain.mjs, pressed by
# scripts/node-e2e.py for a worn chain).
#
# It runs as the Nodes' user (4713): Zebra writes its JSON-RPC cookie
# owner-only into the chain's directory, and the Nodes' Zcash actors read it
# there, from the same volume mounted read-only (compose.yaml). The state
# directory is owned by that user here, so a new volume takes its owner on
# its first mount. The healthcheck binary is the one the Nodes use.
# "basecamp" is a build context (compose.yaml: service:basecamp-ui), not a registry image.
# hadolint ignore=DL3006
FROM basecamp
LABEL org.opencontainers.image.source="https://github.com/logos-co/logos-zebrad-module"
LABEL org.opencontainers.image.revision="ab43ad345ea9d89ceb08750f46621389262eb508"
LABEL org.logos-co.atomic-swaps.purpose="local-zebra-regtest-module"
USER root
RUN mkdir -p /var/lib/zebra && chown 4713:4713 /var/lib/zebra && chmod 0700 /var/lib/zebra
COPY --chmod=0555 lez-runtime-healthcheck /usr/local/bin/lez-runtime-healthcheck
COPY --chmod=0444 reset-chain.mjs /opt/lez-zcash-reset-chain.mjs
ENV BASECAMP_ROLE=zcash BASECAMP_USER_DIR=/var/lib/zebra/basecamp LANG=C.UTF-8
USER 4713:4713
WORKDIR /var/lib/zebra
