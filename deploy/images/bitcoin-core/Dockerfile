# Built from the checksum- and Guix-attestation-verified official 31.1 release
# archive (cache carried over from the repository's own e2e provenance flow;
# bin/ payloads below were extracted from that verified archive).
FROM cgr.dev/chainguard/glibc-dynamic:latest@sha256:205572d5e48117e14b44b42627890fa8d3e8e65bb37a80abb3317e5151e7f35b

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
