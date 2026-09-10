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
FROM ubuntu:24.04

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
    && echo "deb [arch=arm64 signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu noble stable" \
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
ARG RISC0_TAG=v3.0.5
ARG RZUP_VERSION=0.5.1
ARG RAPIDSNARK_URL=https://github.com/logos-blockchain/logos-blockchain-rust-rapidsnark/releases/download/rapidsnark-pic-v0.0.8/rapidsnark-linux-aarch64-pic-v0.0.8.zip
ARG RAPIDSNARK_LIB_SHA256=43553a6ae796621c63837829fb8b35c46cd8f0ffdb1d88f3761eb10ddbe59657
ENV LEZ_TOOLS=/opt/lez-tools \
    RAPIDSNARK_LIB_DIR=/opt/lez-tools/rapidsnark \
    BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/aarch64-linux-gnu/13/include
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
RUN mkdir -p "$LEZ_TOOLS/rapidsnark" && tmp="$(mktemp -d)" \
    && curl -fsSL "$RAPIDSNARK_URL" -o "$tmp/r.zip" && unzip -qo "$tmp/r.zip" -d "$tmp/r" \
    && find "$tmp/r" -name '*.a' -exec cp {} "$LEZ_TOOLS/rapidsnark/" \; \
    && echo "$RAPIDSNARK_LIB_SHA256  $LEZ_TOOLS/rapidsnark/librapidsnark.a" | sha256sum -c --strict --quiet \
    && rm -rf "$tmp"
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

WORKDIR /workspace
