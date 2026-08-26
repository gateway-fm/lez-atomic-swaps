FROM docker.io/zfnd/zebra:5.2.0@sha256:477e65add4dacf52074ba04da8d763c89c26cc57f911dba2127401f8e1da597d AS zebra

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:a77defd6fedbb3392b175ba8ea3d1c22be963c1597c248c3ba987ddd80bfb512

LABEL org.opencontainers.image.source="https://github.com/ZcashFoundation/zebra"
LABEL org.opencontainers.image.version="5.2.0"
LABEL org.opencontainers.image.revision="62e4a43879c9c86d23ecfcf5a02335eec8a1517d"
LABEL org.opencontainers.image.licenses="MIT OR Apache-2.0"
LABEL org.logos-co.atomic-swaps.purpose="isolated-zebra-regtest"

COPY --from=zebra /usr/local/bin/zebrad /usr/local/bin/zebrad

ENV HOME=/tmp
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/zebrad"]
CMD ["start"]
