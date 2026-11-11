# The LEZ v0.2 network in a Basecamp: the basecamp-ui image's Basecamp (build
# context "basecamp", compose.yaml) hosting two Logos modules, both runtime
# modules of its bundle (staged by from-scratch.sh):
# - blockchain_module, the same .lgx a Basecamp user installs
#   (logos-modules-release blockchain_module-v0.2.4, node 0.2.4): the devnet's
#   block producer, with the config and deployment settings the bedrock binary
#   took (LEZ_NETWORK_NODE_CONFIG, LEZ_NETWORK_NODE_DEPLOYMENT); its HTTP API on
#   :18080, its state in the working directory's ./state;
# - lez_indexer_module, built from its 1.1.1 source on LEZ v0.2.4 (+ the nix
#   risc0 fetch fix) with the indexer FFI's indexer_rpc_serve and the module's
#   serve_rpc(port) (deploy/builder/patches/lez-indexer-module): the indexer
#   (LEZ_INDEXER_CONFIG) serving indexer_service's own JSON-RPC on :8779
#   (LEZ_INDEXER_RPC_PORT), executing programs with r0vm, its database in the
#   Basecamp user directory.
# Its LEZ Network desk starts both and re-indexes on request (reindex.mjs).
# builder/regenesis rides along when from-scratch.sh staged it: gen-config.sh
# runs it from this image to give each new chain its own genesis time.
# "basecamp" is a build context (compose.yaml: service:basecamp-ui), not a registry image.
# hadolint ignore=DL3006
FROM basecamp
LABEL org.opencontainers.image.source="https://github.com/logos-co/logos-modules-release/releases/tag/blockchain_module-v0.2.4"
LABEL org.opencontainers.image.version="0.2.4"
LABEL org.logos-co.atomic-swaps.purpose="lez-v0.2-network-module"
COPY --chmod=0555 r0vm regenesi[s] /usr/local/bin/
COPY --chmod=0555 health.sh /usr/local/bin/lez-network-health
COPY --chmod=0444 reindex.mjs /opt/lez-network-reindex.mjs
ENV BASECAMP_ROLE=network BASECAMP_USER_DIR=/work/basecamp LANG=C.UTF-8 RISC0_SERVER_PATH=/usr/local/bin/r0vm
USER 65532:65532
WORKDIR /work
