# lez-builder — the ephemeral build image. Nothing runs from it for long: every
# use is one `docker run --rm` that builds one artifact into the provision
# directory (LEZ services, r0vm, the escrow artifact, the sidecar, identities)
# or runs the one-time market bootstrap against the stack. It holds no state,
# no identity and no Docker socket unless the single step that needs one (the
# reproducible Risc0 guest build) mounts it for that run.
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

WORKDIR /workspace
