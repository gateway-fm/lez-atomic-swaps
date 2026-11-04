# The LEZ v0.2 indexer as a Logos (Basecamp) module: lez_indexer_module built
# from its 1.1.1 source on LEZ v0.2.4 (+ the nix risc0 fetch fix) with one
# addition on each side, the indexer FFI's indexer_rpc_call and indexer_rpc_serve, and the module's
# serve_rpc(port) (patches under deploy/builder/patches/lez-indexer-module,
# staged as lez_indexer_module.lgx by from-scratch.sh --only stage). Hosted by
# the same headless logoscore as the network node. serve_rpc runs
# indexer_service's own JSON-RPC on :8779 inside the module process, over the
# indexer the module runs, so the Nodes and the sidecar read it as before.
FROM debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4 AS fetch
ARG TARGETARCH
ARG LOGOSCORE_VERSION=0.2.3
ARG LOGOSCORE_SHA256_AMD64=2e6eb1b7c971ceb35979d814ad1f16fc298f8f5d78a9e814546befbce871bbbb
ARG LOGOSCORE_SHA256_ARM64=cecb818c338fe98b6de3265126fa617738a588bf89caa3691066fb1d6a50cdd1
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
# hadolint ignore=DL3008
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends ca-certificates curl squashfs-tools >/dev/null && rm -rf /var/lib/apt/lists/*
WORKDIR /fetch
COPY lez_indexer_module.lgx module.lgx
RUN set -eu; \
    case "${TARGETARCH:-amd64}" in \
      amd64) arch=x86_64; sum="$LOGOSCORE_SHA256_AMD64"; variant=linux-amd64 ;; \
      arm64) arch=aarch64; sum="$LOGOSCORE_SHA256_ARM64"; variant=linux-arm64 ;; \
      *) echo "unsupported arch ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o logoscore.tar.gz "https://github.com/logos-co/logos-logoscore-cli/releases/download/${LOGOSCORE_VERSION}/logoscore-${arch}-linux.tar.gz"; \
    echo "$sum  logoscore.tar.gz" | sha256sum -c -; \
    tar -xzf logoscore.tar.gz; img="logoscore-${arch}.AppImage"; \
    shoff=$(od -An -t u8 -j 40 -N 8 "$img" | tr -d ' '); \
    shentsize=$(od -An -t u2 -j 58 -N 2 "$img" | tr -d ' '); \
    shnum=$(od -An -t u2 -j 60 -N 2 "$img" | tr -d ' '); \
    unsquashfs -q -o $((shoff + shentsize * shnum)) -d /opt/logoscore "$img" >/dev/null; \
    mkdir -p lgx /opt/lez-modules/lez_indexer_module; tar -xzf module.lgx -C lgx; \
    cp lgx/manifest.json /opt/lez-modules/lez_indexer_module/; \
    src="lgx/variants/${variant}"; [ -d "$src" ] || src="lgx/variants/${variant}-dev"; \
    cp -r "$src/"* /opt/lez-modules/lez_indexer_module/; \
    test -f /opt/lez-modules/lez_indexer_module/lez_indexer_module_plugin.so

FROM debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4
LABEL org.logos-co.atomic-swaps.purpose="lez-v0.2-indexer-module"
COPY --from=fetch /opt/logoscore /opt/logoscore
COPY --from=fetch /opt/lez-modules /opt/lez-modules
COPY --chmod=0555 r0vm /usr/local/bin/r0vm
COPY --chmod=0555 health.sh /usr/local/bin/lez-indexer-module-health
COPY --chmod=0555 entrypoint.sh /usr/local/bin/lez-indexer-module
ENV HOME=/tmp QT_QPA_PLATFORM=offscreen LANG=C.UTF-8 RISC0_SERVER_PATH=/usr/local/bin/r0vm
USER 65532:65532
WORKDIR /work
ENTRYPOINT ["/usr/local/bin/lez-indexer-module"]
