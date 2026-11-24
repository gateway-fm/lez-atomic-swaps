# lez-builder — the ephemeral build image. Nothing runs from it for long: every
# use is one `docker run --rm` that builds one artifact into the provision
# directory (LEZ services, the escrow artifact, the sidecar, identities) or
# runs the one-time market bootstrap against the stack. It holds no state,
# no identity and no Docker socket unless the single step that needs one (the
# reproducible Risc0 guest build) mounts it for that run.
#
# It also carries the pinned prover toolchain (rzup, cargo-risczero and r0vm
# at the risc0 tag, the Logos rapidsnark libraries) so no run builds it:
# upstream ships no arm64 assets and building cargo-risczero alone takes about
# 1.5 hours. Nothing in that layer depends on this repository, so the image is
# published once per pin change (`.github/workflows/builder-image.yml`) and
# `scripts/from-scratch.sh` pulls it by the digest in `image.lock`, or builds
# it here when the registry is unreachable or `LEZ_BUILDER_IMAGE=local`.
#
# The base is pinned by digest: the layer cache of builder-image.yml is keyed
# on it, so a republished `ubuntu:24.04` tag would rebuild every layer. Bump
# the digest by hand (`docker buildx imagetools inspect ubuntu:24.04`).
FROM ubuntu:24.04@sha256:562111b88caa79c71391c0c4d3c2114a7d5c35915c20561fd893cb1e07929e6b

ENV DEBIAN_FRONTEND=noninteractive
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl git gnupg jq ripgrep python3 unzip tar xz-utils bzip2 \
        pkg-config build-essential clang libssl-dev perl cmake \
        util-linux procps socat sqlite3 xxd \
        python3-dev libpython3.12-dev \
    && rm -rf /var/lib/apt/lists/*

# docker CLI + buildx: only the escrow guest step uses them, against a socket
# mounted for that run; the daemon stays on the host.
# hadolint ignore=DL3008
RUN install -m 0755 -d /etc/apt/keyrings \
    && curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc \
    && chmod a+r /etc/apt/keyrings/docker.asc \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu noble stable" \
        > /etc/apt/sources.list.d/docker.list \
    && apt-get update && apt-get install -y --no-install-recommends docker-ce-cli docker-buildx-plugin \
    && rm -rf /var/lib/apt/lists/*

# Toolchains are system-wide so the image runs as any uid; the caller mounts a
# cache volume as CARGO_HOME for registries and another as CARGO_TARGET_DIR.
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH="/opt/cargo/bin:${PATH}"
RUN curl -fsSL https://sh.rustup.rs -o /tmp/rustup.sh \
    && sh /tmp/rustup.sh -y --no-modify-path --default-toolchain 1.96.0 --profile minimal \
    && rustup toolchain install 1.94.0 --profile minimal \
    && rustup component add clippy rustfmt --toolchain 1.96.0 \
    && rm -f /tmp/rustup.sh \
    && chmod -R a+rwX /opt/rustup /opt/cargo

# ---- the pinned prover toolchain -------------------------------------------
# The same pins as scripts/from-scratch.sh (RISC0_TAG, RAPIDSNARK_URL and its
# digest) and scripts/verify-lez-v02-provisional.sh (rzup, cargo-risczero).
# from-scratch.sh seeds provision/data/tools-arm and rapidsnark-arm from
# /opt/lez-tools, and every version is checked again where it is used.
# The image is built for linux/arm64 and linux/amd64 (builder-image.yml); the
# build platform picks the rapidsnark release and the GCC include directory.
ARG RISC0_TAG=v3.0.5
ARG RZUP_VERSION=0.5.1
ARG RAPIDSNARK_RELEASE=https://github.com/logos-blockchain/logos-blockchain-rust-rapidsnark/releases/download/rapidsnark-pic-v0.0.8
ARG RAPIDSNARK_ARM64_LIB_SHA256=43553a6ae796621c63837829fb8b35c46cd8f0ffdb1d88f3761eb10ddbe59657
ARG RAPIDSNARK_AMD64_LIB_SHA256=d4133227f845ff5bfa3672eb5b9c018a6a086bfa164b176bdaf76949c7d1f423
ARG TARGETARCH
ENV LEZ_TOOLS=/opt/lez-tools \
    RAPIDSNARK_LIB_DIR=/opt/lez-tools/rapidsnark \
    BINDGEN_EXTRA_CLANG_ARGS=-I/opt/lez-tools/gcc-include
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
RUN arch="${TARGETARCH:-$(dpkg --print-architecture)}" \
    && case "$arch" in \
         arm64) triple=aarch64-linux-gnu; asset=rapidsnark-linux-aarch64-pic-v0.0.8.zip; RAPIDSNARK_LIB_SHA256="$RAPIDSNARK_ARM64_LIB_SHA256" ;; \
         amd64) triple=x86_64-linux-gnu; asset=rapidsnark-linux-x86_64-pic-v0.0.8.zip; RAPIDSNARK_LIB_SHA256="$RAPIDSNARK_AMD64_LIB_SHA256" ;; \
         *) echo "no rapidsnark release for $arch" >&2; exit 1 ;; \
       esac \
    && [[ "$(dpkg --print-architecture)" == "$arch" ]] \
    && mkdir -p "$LEZ_TOOLS/rapidsnark" && ln -s "/usr/lib/gcc/$triple/13/include" "$LEZ_TOOLS/gcc-include" \
    && [[ -f "$LEZ_TOOLS/gcc-include/stddef.h" ]] \
    && tmp="$(mktemp -d)" \
    && curl -fsSL "$RAPIDSNARK_RELEASE/$asset" -o "$tmp/r.zip" && unzip -qo "$tmp/r.zip" -d "$tmp/r" \
    && find "$tmp/r" -name '*.a' -exec cp {} "$LEZ_TOOLS/rapidsnark/" \; \
    && echo "$RAPIDSNARK_LIB_SHA256  $LEZ_TOOLS/rapidsnark/librapidsnark.a" | sha256sum -c --strict --quiet \
    && rm -rf "$tmp"
# RISC Zero's recursion circuit archive. risc0-circuit-recursion's build.rs
# (feature `prove`: r0vm, cargo-risczero) downloads it from RISC Zero's bucket,
# which stopped serving it on 2026-11-23, unless RECURSION_SRC_PATH names a
# copy with the SHA-256 it pins. The same bytes come from this project's
# build-inputs-1 release, checked here; every later build in the image, and
# every `docker run` of it, finds them through RECURSION_SRC_PATH. The same
# pin as scripts/from-scratch.sh (RISC0_RECURSION_ZKR_URL and _SHA256).
ARG RISC0_RECURSION_ZKR_URL=https://github.com/gateway-fm/lez-atomic-swaps/releases/download/build-inputs-1/risc0-recursion-zkr-744b999f0a35b3c86753311c7efb2a0054be21727095cf105af6ee7d3f4d8849.zip
ARG RISC0_RECURSION_ZKR_SHA256=744b999f0a35b3c86753311c7efb2a0054be21727095cf105af6ee7d3f4d8849
ENV RECURSION_SRC_PATH=/opt/lez-tools/risc0-recursion-zkr.zip
RUN curl -fsSL --retry 3 "$RISC0_RECURSION_ZKR_URL" -o "$RECURSION_SRC_PATH" \
    && echo "$RISC0_RECURSION_ZKR_SHA256  $RECURSION_SRC_PATH" | sha256sum -c --strict --quiet \
    && chmod 0444 "$RECURSION_SRC_PATH"
# rzup and cargo-risczero from crates.io, r0vm from the risc0 tag (no arm64
# release assets exist); the registries used for the installs are dropped.
RUN cargo +1.96.0 install rzup --version "$RZUP_VERSION" --locked --root "$LEZ_TOOLS" \
    && git clone --quiet --depth 1 --branch "$RISC0_TAG" https://github.com/risc0/risc0.git /tmp/risc0 \
    && cargo +1.96.0 install --path /tmp/risc0/risc0/r0vm --locked --root "$LEZ_TOOLS" \
    && rm -rf /tmp/risc0 \
    && cargo +1.96.0 install cargo-risczero --version "${RISC0_TAG#v}" --locked --bin cargo-risczero --root "$LEZ_TOOLS" \
    && rm -rf /opt/cargo/registry /opt/cargo/git "$LEZ_TOOLS/.crates.toml" "$LEZ_TOOLS/.crates2.json" \
    && chmod -R a+rX "$LEZ_TOOLS" \
    && [[ "$("$LEZ_TOOLS/bin/rzup" --version)" == "rzup $RZUP_VERSION" ]] \
    && [[ "$("$LEZ_TOOLS/bin/r0vm" --version)" == "risc0-r0vm ${RISC0_TAG#v}" ]] \
    && [[ "$("$LEZ_TOOLS/bin/cargo-risczero" risczero --version)" == "cargo-risczero ${RISC0_TAG#v}" ]]

# LEZ v0.2.4's wallet crate, which the escrow deployer and the sidecar build,
# links libpcsclite (keycard). Last, so the toolchain layer above stays cached.
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends libpcsclite-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
