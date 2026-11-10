# The LEZ v0.2 network node in a Basecamp: the basecamp-ui image's Basecamp
# (build context "basecamp", compose.yaml), whose bundle carries the same
# blockchain_module .lgx a Basecamp user installs (logos-modules-release
# blockchain_module-v0.2.4, node 0.2.4; staged by from-scratch.sh). Its LEZ
# Network desk starts the node with the config and deployment settings the
# bedrock binary took (LEZ_NETWORK_NODE_CONFIG, LEZ_NETWORK_NODE_DEPLOYMENT).
# It keeps the bedrock service's contract: the node HTTP API on :18080, state
# under /work/state (the Basecamp user directory and the node's own ./state).
# "basecamp" is a build context (compose.yaml: service:basecamp-ui), not a registry image.
# hadolint ignore=DL3006
FROM basecamp
LABEL org.opencontainers.image.source="https://github.com/logos-co/logos-modules-release/releases/tag/blockchain_module-v0.2.4"
LABEL org.opencontainers.image.version="0.2.4"
LABEL org.logos-co.atomic-swaps.purpose="lez-v0.2-network-module"
ENV BASECAMP_ROLE=network BASECAMP_USER_DIR=/work/state/basecamp
USER 65532:65532
WORKDIR /work
