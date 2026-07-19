FROM gcr.io/distroless/cc-debian13:nonroot@sha256:aded2458d026e046cb68199db0e5793e1028ffa143f7258f3c4278253e20add7

LABEL org.opencontainers.image.source="https://github.com/monero-project/monero"
LABEL org.opencontainers.image.version="0.18.5.1"
LABEL org.opencontainers.image.revision="4f92268d7c16741cfb41e5bbe2aa46cc260a9ea5"
LABEL org.opencontainers.image.licenses="BSD-3-Clause"
LABEL org.logos-co.atomic-swaps.purpose="isolated-monero-regtest"
LABEL org.logos-co.atomic-swaps.release-archive-sha256="22a7dda7b0cb699fdd6b7674c3b4a4465b337cc98a54983523b759e1e7cc9958"
LABEL org.logos-co.atomic-swaps.release-signer="81AC591FE9C4B65C5806AFC3F0AF4D462A0BDF92"

# The build context is extracted only after the official signed hash manifest,
# archive checksum, exact members, and source tag have been verified.
COPY --chmod=0555 bin/monerod /usr/local/bin/monerod
COPY --chmod=0555 bin/monero-wallet-rpc /usr/local/bin/monero-wallet-rpc

ENV HOME=/tmp
USER 65532:65532
CMD ["/usr/local/bin/monerod", "--version"]
