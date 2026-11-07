# The LEZ v0.2 indexer in a Basecamp: the basecamp-ui image's Basecamp (build
# context "basecamp", compose.yaml), whose bundle carries lez_indexer_module
# built from its 1.1.1 source on LEZ v0.2.4 (+ the nix risc0 fetch fix) with one
# addition on each side, the indexer FFI's indexer_rpc_call and
# indexer_rpc_serve, and the module's serve_rpc(port) (patches under
# deploy/builder/patches/lez-indexer-module, staged by from-scratch.sh). Its LEZ
# Network desk starts the indexer (LEZ_INDEXER_CONFIG) and serve_rpc runs
# indexer_service's own JSON-RPC on :8779 inside the module process
# (LEZ_INDEXER_RPC_PORT), so the Nodes, the sidecars and the harness read it as
# before. Its database lives in the Basecamp user directory under /work/state.
# "basecamp" is a build context (compose.yaml: service:basecamp-ui), not a registry image.
# hadolint ignore=DL3006
FROM basecamp
LABEL org.logos-co.atomic-swaps.purpose="lez-v0.2-indexer-module"
COPY --chmod=0555 r0vm /usr/local/bin/r0vm
COPY --chmod=0555 health.sh /usr/local/bin/lez-indexer-module-health
ENV BASECAMP_ROLE=network BASECAMP_USER_DIR=/work/state/basecamp LANG=C.UTF-8 RISC0_SERVER_PATH=/usr/local/bin/r0vm
USER 65532:65532
WORKDIR /work
