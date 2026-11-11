# Zcash Regtest node for the local LEZ <-> ZEC swap stack. Mirrors the isolated
# lane in tests/e2e/zebra: the same digest-pinned upstream build, copied into a
# distroless runtime so the stack ships no shell. The healthcheck binary is the
# one the Nodes use, staged beside zebrad because distroless has no CLI.
FROM docker.io/zfnd/zebra:7.0.0@sha256:a70d2be8ffef3a4b6e37355724e36a0e6c1838e46baa28bfb216739f00cf2d34 AS zebra

# The data directory, owned by the runtime user: a new data volume takes its
# owner from here on its first mount (compose.yaml), so nothing chowns it.
FROM debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4 AS datadir
RUN mkdir /datadir

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:a77defd6fedbb3392b175ba8ea3d1c22be963c1597c248c3ba987ddd80bfb512
LABEL org.opencontainers.image.source="https://github.com/ZcashFoundation/zebra"
LABEL org.opencontainers.image.version="7.0.0"
LABEL org.opencontainers.image.revision="e8db87eb69254e0d4eedd058eadd48bdfa09065a"
LABEL org.opencontainers.image.licenses="MIT OR Apache-2.0"
LABEL org.logos-co.atomic-swaps.purpose="local-zebra-regtest"
COPY --from=zebra /usr/local/bin/zebrad /usr/local/bin/zebrad
COPY --chmod=0555 lez-runtime-healthcheck /usr/local/bin/lez-runtime-healthcheck
COPY --from=datadir --chown=65532:65532 /datadir /var/lib/zebra
ENV HOME=/tmp
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/zebrad"]
CMD ["--config", "/run-config/zebrad.toml", "start"]
