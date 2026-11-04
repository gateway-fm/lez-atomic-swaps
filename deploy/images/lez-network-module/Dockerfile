# The LEZ v0.2 network node as a Logos (Basecamp) module: the same
# blockchain_module .lgx a Basecamp user installs (logos-modules-release
# blockchain_module-v0.2.4, node 0.2.4), hosted headless by logoscore of the
# same era (logos-logoscore-cli 0.2.3). It replaces the bedrock container and
# keeps its contract: the same node-config.yaml and deployment settings, the
# node HTTP API on :18080, state under /work/state.
FROM debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4 AS fetch
ARG TARGETARCH
ARG LOGOSCORE_VERSION=0.2.3
ARG LOGOSCORE_SHA256_AMD64=2e6eb1b7c971ceb35979d814ad1f16fc298f8f5d78a9e814546befbce871bbbb
ARG LOGOSCORE_SHA256_ARM64=cecb818c338fe98b6de3265126fa617738a588bf89caa3691066fb1d6a50cdd1
ARG BLOCKCHAIN_MODULE_VERSION=0.2.4
ARG BLOCKCHAIN_MODULE_SHA256=31819f06aa0639874fa0257f58c24e1add6fce2b489c843b64905e28f59f0c84
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
# hadolint ignore=DL3008
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends ca-certificates curl squashfs-tools >/dev/null && rm -rf /var/lib/apt/lists/*
WORKDIR /fetch
RUN set -eu; \
    case "${TARGETARCH:-amd64}" in \
      amd64) arch=x86_64; sum="$LOGOSCORE_SHA256_AMD64"; variant=linux-amd64 ;; \
      arm64) arch=aarch64; sum="$LOGOSCORE_SHA256_ARM64"; variant=linux-arm64 ;; \
      *) echo "unsupported arch ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o logoscore.tar.gz "https://github.com/logos-co/logos-logoscore-cli/releases/download/${LOGOSCORE_VERSION}/logoscore-${arch}-linux.tar.gz"; \
    echo "$sum  logoscore.tar.gz" | sha256sum -c -; \
    tar -xzf logoscore.tar.gz; \
    img="logoscore-${arch}.AppImage"; \
    # The AppImage's payload is a squashfs right after its ELF image; unpack it
    # without running the AppImage (offset = e_shoff + e_shentsize * e_shnum).
    shoff=$(od -An -t u8 -j 40 -N 8 "$img" | tr -d ' '); \
    shentsize=$(od -An -t u2 -j 58 -N 2 "$img" | tr -d ' '); \
    shnum=$(od -An -t u2 -j 60 -N 2 "$img" | tr -d ' '); \
    unsquashfs -q -o $((shoff + shentsize * shnum)) -d /opt/logoscore "$img" >/dev/null; \
    curl -fsSL -o module.lgx "https://github.com/logos-co/logos-modules-release/releases/download/blockchain_module-v${BLOCKCHAIN_MODULE_VERSION}/blockchain_module-${BLOCKCHAIN_MODULE_VERSION}.lgx"; \
    echo "$BLOCKCHAIN_MODULE_SHA256  module.lgx" | sha256sum -c -; \
    mkdir -p lgx /opt/lez-modules/blockchain_module; tar -xzf module.lgx -C lgx; \
    cp lgx/manifest.json /opt/lez-modules/blockchain_module/; \
    cp "lgx/variants/${variant}/"* /opt/lez-modules/blockchain_module/; \
    test -x /opt/logoscore/usr/bin/logoscore; test -f /opt/lez-modules/blockchain_module/blockchain_module_plugin.so

# logoscore's bundled libraries need glibc >= 2.39 (Debian 13).
FROM debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4
LABEL org.opencontainers.image.source="https://github.com/logos-co/logos-modules-release/releases/tag/blockchain_module-v0.2.4"
LABEL org.opencontainers.image.version="0.2.4"
LABEL org.logos-co.atomic-swaps.purpose="lez-v0.2-network-module"
COPY --from=fetch /opt/logoscore /opt/logoscore
COPY --from=fetch /opt/lez-modules /opt/lez-modules
COPY --chmod=0555 entrypoint.sh /usr/local/bin/lez-network-module
ENV HOME=/tmp QT_QPA_PLATFORM=offscreen
USER 65532:65532
WORKDIR /work
ENTRYPOINT ["/usr/local/bin/lez-network-module"]
