# The LEZ wallet as a Logos (Basecamp) module: lez_core 0.4.1 built on LEZ
# v0.2.4 (+ the nix risc0 fetch fix) with one addition, import_public_account
# (deploy/builder/patches/logos-execution-zone-module; staged as lez_core.lgx by
# from-scratch.sh --only stage), in the same headless logoscore as the network
# node and the indexer. lez-wallet-vault-claim does the market bootstrap's vault
# claims and lez-wallet-heartbeat the finality heartbeat's transfers through it,
# each with a throwaway wallet holding one imported account.

FROM debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4 AS fetch
ARG TARGETARCH
ARG LOGOSCORE_VERSION=0.2.3
ARG LOGOSCORE_SHA256_AMD64=2e6eb1b7c971ceb35979d814ad1f16fc298f8f5d78a9e814546befbce871bbbb
ARG LOGOSCORE_SHA256_ARM64=cecb818c338fe98b6de3265126fa617738a588bf89caa3691066fb1d6a50cdd1
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
# hadolint ignore=DL3008
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends ca-certificates curl squashfs-tools >/dev/null && rm -rf /var/lib/apt/lists/*
WORKDIR /fetch
COPY lez_core.lgx module.lgx
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
    mkdir -p lgx /opt/lez-modules/lez_core; tar -xzf module.lgx -C lgx; \
    cp lgx/manifest.json /opt/lez-modules/lez_core/; \
    src="lgx/variants/${variant}"; [ -d "$src" ] || src="lgx/variants/${variant}-dev"; \
    cp -r "$src/"* /opt/lez-modules/lez_core/; \
    test -f /opt/lez-modules/lez_core/lez_core_plugin.so

FROM debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4
LABEL org.logos-co.atomic-swaps.purpose="lez-v0.2-wallet-module"
# The market bootstrap runs in this image too (market-bootstrap.sh: bash, jq,
# curl, socat for the loopback-only official tools).
# hadolint ignore=DL3008
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends ca-certificates curl jq socat >/dev/null && rm -rf /var/lib/apt/lists/*
COPY --from=fetch /opt/logoscore /opt/logoscore
COPY --from=fetch /opt/lez-modules /opt/lez-modules
COPY --chmod=0444 wallet-lib.sh /usr/local/lib/lez-wallet-lib.sh
COPY --chmod=0555 vault-claim.sh /usr/local/bin/lez-wallet-vault-claim
COPY --chmod=0555 heartbeat.sh /usr/local/bin/lez-wallet-heartbeat
ENV HOME=/tmp QT_QPA_PLATFORM=offscreen LANG=C.UTF-8
USER 65532:65532
WORKDIR /tmp
