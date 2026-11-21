# The data directories, owned by the runtime user: a new data volume takes its
# owner from here on its first mount (the devnet's Monero services,
# deploy/compose.yaml, build this file too), so nothing chowns it.
FROM debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4 AS datadir
RUN mkdir /datadir

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:a77defd6fedbb3392b175ba8ea3d1c22be963c1597c248c3ba987ddd80bfb512

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
COPY --from=datadir --chown=65532:65532 /datadir /var/lib/monero
COPY --from=datadir --chown=65532:65532 /datadir /var/lib/monero-wallet

ENV HOME=/tmp
USER 65532:65532
CMD ["/usr/local/bin/monerod", "--version"]
