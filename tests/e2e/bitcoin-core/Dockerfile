FROM gcr.io/distroless/cc-debian13:nonroot@sha256:aded2458d026e046cb68199db0e5793e1028ffa143f7258f3c4278253e20add7

LABEL org.opencontainers.image.source="https://github.com/bitcoin/bitcoin"
LABEL org.opencontainers.image.version="31.1"
LABEL org.opencontainers.image.revision="9be056a8a72b624dae9623b2f7bded92c2a21c91"
LABEL org.opencontainers.image.licenses="MIT"
LABEL org.logos-co.atomic-swaps.purpose="isolated-bitcoin-core-regtest"
LABEL org.logos-co.atomic-swaps.release-archive-sha256="b80d9c3e04da78fb6f0569685673418cf686fadba9042d926d13fb87ff503f9e"
LABEL org.logos-co.atomic-swaps.guix-sigs-commit="11fb5156a16f27d71b61d18e23c5ffeb29cc6ee1"

# The build context is prepared from the checksum- and attestation-verified
# official archive. Keep only the daemon and RPC client in the runtime image.
COPY --chmod=0555 bin/bitcoind /usr/local/bin/bitcoind
COPY --chmod=0555 bin/bitcoin-cli /usr/local/bin/bitcoin-cli

ENV HOME=/tmp
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/bitcoind"]
CMD ["-conf=/run-config/bitcoin.conf", "-datadir=/var/lib/bitcoin"]
