# Built from the checksum- and Guix-attestation-verified official 31.1 release
# archive (cache carried over from the repository's own e2e provenance flow;
# bin/ payloads below were extracted from that verified archive).
# The data directory, owned by the runtime user: a new data volume takes its
# owner from here on its first mount (compose.yaml), so nothing chowns it.
FROM debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4 AS datadir
RUN mkdir /datadir

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

COPY --from=datadir --chown=65532:65532 /datadir /var/lib/bitcoin
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/bitcoind"]
CMD ["-conf=/run-config/bitcoin.conf", "-datadir=/var/lib/bitcoin"]
