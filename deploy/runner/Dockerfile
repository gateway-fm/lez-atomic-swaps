FROM ubuntu:24.04

ARG UID=501
ARG GID=20

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl git gnupg jq ripgrep python3 unzip tar xz-utils bzip2 \
        openssh-client pkg-config build-essential clang libssl-dev perl cmake \
        util-linux procps socat sudo less vim \
    && rm -rf /var/lib/apt/lists/*

# docker CLI (talks to the mounted host socket; daemon stays on the host)
RUN install -m 0755 -d /etc/apt/keyrings \
    && curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc \
    && chmod a+r /etc/apt/keyrings/docker.asc \
    && echo "deb [arch=amd64 signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu noble stable" \
        > /etc/apt/sources.list.d/docker.list \
    && apt-get update && apt-get install -y --no-install-recommends docker-ce-cli \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd -g ${GID} hostgroup 2>/dev/null || true; \
    useradd -m -u ${UID} -g ${GID} -s /bin/bash lez \
    && echo "lez ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/lez

USER lez
WORKDIR /workspace

RUN curl -fsSL https://sh.rustup.rs -o /tmp/rustup.sh && sh /tmp/rustup.sh -y --default-toolchain 1.96.0 --profile minimal \
    && rustup toolchain install 1.94.0 --profile minimal \
    && rustup component add clippy rustfmt --toolchain 1.96.0 || true
ENV PATH="/home/lez/.cargo/bin:${PATH}"
