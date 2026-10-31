# Zcash Regtest node for the local LEZ <-> ZEC swap stack. Mirrors the isolated
# lane in tests/e2e/zebra: the same digest-pinned upstream build, copied into a
# distroless runtime so the stack ships no shell. The healthcheck binary is the
# one the Nodes use, staged beside zebrad because distroless has no CLI.
FROM docker.io/zfnd/zebra:5.2.0@sha256:477e65add4dacf52074ba04da8d763c89c26cc57f911dba2127401f8e1da597d AS zebra

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:a77defd6fedbb3392b175ba8ea3d1c22be963c1597c248c3ba987ddd80bfb512
LABEL org.opencontainers.image.source="https://github.com/ZcashFoundation/zebra"
LABEL org.opencontainers.image.version="5.2.0"
LABEL org.opencontainers.image.revision="62e4a43879c9c86d23ecfcf5a02335eec8a1517d"
LABEL org.opencontainers.image.licenses="MIT OR Apache-2.0"
LABEL org.logos-co.atomic-swaps.purpose="local-zebra-regtest"
COPY --from=zebra /usr/local/bin/zebrad /usr/local/bin/zebrad
COPY --chmod=0555 lez-runtime-healthcheck /usr/local/bin/lez-runtime-healthcheck
ENV HOME=/tmp
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/zebrad"]
CMD ["--config", "/run-config/zebrad.toml", "start"]
