# Built from the checksum- and Guix-attestation-verified official 31.1 release
# archive (cache carried over from the repository's own e2e provenance flow;
# bin/ payloads below were extracted from that verified archive).
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:aded2458d026e046cb68199db0e5793e1028ffa143f7258f3c4278253e20add7

COPY --chmod=0555 bin/bitcoind /usr/local/bin/bitcoind
COPY --chmod=0555 bin/bitcoin-cli /usr/local/bin/bitcoin-cli

LABEL org.logos-co.atomic-swaps.purpose="local-regtest-bitcoin-core"
LABEL org.opencontainers.image.version="31.1"
LABEL org.opencontainers.image.licenses="MIT"

# Built from the checksum- and Guix-attestation-verified official 31.1 release
# archive (cache carried over from the repository's own e2e provenance flow;
# bin/ payloads below were extracted from that verified archive).
COPY --chmod=0555 bin/bitcoind /usr/local/bin/bitcoind
COPY --chmod=0555 bin/bitcoin-cli /usr/local/bin/bitcoin-cli

USER 65532:65532
ENTRYPOINT ["/usr/local/bin/bitcoind"]
CMD ["-conf=/run-config/bitcoin.conf", "-datadir=/var/lib/bitcoin"]
