FROM cgr.dev/chainguard/glibc-dynamic:latest@sha256:205572d5e48117e14b44b42627890fa8d3e8e65bb37a80abb3317e5151e7f35b

LABEL org.logos-co.atomic-swaps.purpose="local-lez-v0.2-services"
LABEL org.opencontainers.image.source="https://github.com/logos-blockchain/logos-execution-zone"
LABEL org.opencontainers.image.version="v0.2.0"
LABEL org.opencontainers.image.revision="a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
LABEL org.opencontainers.image.licenses="MIT OR Apache-2.0"

COPY --chmod=0555 sequencer_service /usr/local/bin/sequencer_service
COPY --chmod=0555 indexer_service /usr/local/bin/indexer_service
COPY --chmod=0555 r0vm /usr/local/bin/r0vm

ENV HOME=/tmp
ENV RISC0_SERVER_PATH=/usr/local/bin/r0vm
USER 65532:65532
