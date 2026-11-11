FROM docker.io/zfnd/zebra:7.0.0@sha256:a70d2be8ffef3a4b6e37355724e36a0e6c1838e46baa28bfb216739f00cf2d34 AS zebra

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:08476c244888bfb425bae1ae4dd7c17f093e972aab8426883d97f3c6f280f7c1

LABEL org.opencontainers.image.source="https://github.com/ZcashFoundation/zebra"
LABEL org.opencontainers.image.version="7.0.0"
LABEL org.opencontainers.image.revision="e8db87eb69254e0d4eedd058eadd48bdfa09065a"
LABEL org.opencontainers.image.licenses="MIT OR Apache-2.0"
LABEL org.logos-co.atomic-swaps.purpose="isolated-zebra-regtest"

COPY --from=zebra /usr/local/bin/zebrad /usr/local/bin/zebrad

ENV HOME=/tmp
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/zebrad"]
CMD ["start"]
