# Built from the checksum- and Guix-attestation-verified official 31.1 release
# archive (cache carried over from the repository's own e2e provenance flow;
# bin/ payloads below were extracted from that verified archive).
FROM cgr.dev/chainguard/glibc-dynamic:latest@sha256:5bb932663cff4fb276186720e0357af55c5e47f12e4d9f44ea0060fa3e7bca88

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
