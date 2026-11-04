FROM cgr.dev/chainguard/glibc-dynamic:latest@sha256:5bb932663cff4fb276186720e0357af55c5e47f12e4d9f44ea0060fa3e7bca88

LABEL org.logos-co.atomic-swaps.purpose="local-lez-v0.2-services"
LABEL org.opencontainers.image.source="https://github.com/logos-blockchain/logos-execution-zone"
LABEL org.opencontainers.image.version="v0.2.4"
LABEL org.opencontainers.image.revision="47eba256479f6f785acbd138834340703cd03401"
LABEL org.opencontainers.image.licenses="MIT OR Apache-2.0"

COPY --chmod=0555 sequencer_service /usr/local/bin/sequencer_service
COPY --chmod=0555 indexer_service /usr/local/bin/indexer_service
COPY --chmod=0555 r0vm /usr/local/bin/r0vm
COPY --chmod=0555 lez-runtime-healthcheck /usr/local/bin/lez-runtime-healthcheck

ENV HOME=/tmp
ENV RISC0_SERVER_PATH=/usr/local/bin/r0vm
USER 65532:65532
