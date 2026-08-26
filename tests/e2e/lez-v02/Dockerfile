FROM gcr.io/distroless/cc-debian13:nonroot@sha256:aded2458d026e046cb68199db0e5793e1028ffa143f7258f3c4278253e20add7

LABEL org.opencontainers.image.source="https://github.com/logos-blockchain/logos-execution-zone"
LABEL org.opencontainers.image.version="v0.2.0"
LABEL org.opencontainers.image.revision="a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
LABEL org.opencontainers.image.licenses="MIT OR Apache-2.0"
LABEL org.logos-co.atomic-swaps.purpose="isolated-lez-v0.2-runtime"

COPY --chmod=0555 sequencer_service /usr/local/bin/sequencer_service
COPY --chmod=0555 indexer_service /usr/local/bin/indexer_service
COPY --chmod=0555 r0vm /usr/local/bin/r0vm

ENV HOME=/tmp
ENV RISC0_SERVER_PATH=/usr/local/bin/r0vm
USER 65532:65532
