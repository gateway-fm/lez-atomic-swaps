# The Taker's Basecamp: the basecamp-ui image (build context "basecamp",
# compose.yaml) with the Taker's Node payload package installed. Its desk
# starts the Taker's LEZ signer in Basecamp's lez_core and has Basecamp's
# lez_swap_node launch and supervise the Taker Node from the installed
# package — the swap engine runs inside the party's Basecamp. Nothing of the
# Node is copied into this image on its own: the package
# (lez_taker_node.lgx, scripts/package-role-node.sh) carries the Node, its
# CLI, its actors, the LEZ sidecar and the launcher; this image installs it,
# and compose.yaml configures it. Only the Taker's executables are here
# (deploy/role-symmetry.json). The ZEC corridor provisioners are not part of
# the package: scripts/zec-provision.sh runs them from this image to build a
# prepared ZEC swap from live chain facts.
#
# The staged provisioners are debug builds; their debug sections are most of
# the bytes. Strip them here so the image ships only what runs.
FROM debian:bookworm-slim AS strip
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends binutils \
    && rm -rf /var/lib/apt/lists/*
COPY zec-local-poc-provision zec-local-poc-identity zec-local-poc-transfer \
     zec-local-poc-chat-draft zec-local-poc-chat-finalize /payload/
RUN strip /payload/*

# "basecamp" is a build context (compose.yaml: service:basecamp-ui), not a registry image.
# hadolint ignore=DL3006
FROM basecamp

LABEL org.logos-co.atomic-swaps.purpose="local-taker-node"

# What the package's launcher runs besides the package's own executables.
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl jq libpcsclite1 procps socat python3 util-linux \
    && rm -rf /var/lib/apt/lists/*

# The state directories the Compose volumes mount here (taker_state, zec_taker,
# delivery_dir, ...): a new named volume takes the owner and mode of the image
# directory it is first mounted on, so no init container prepares them.
RUN groupadd -r -g 4713 lez && useradd -r -u 4713 -g lez -d /var/lib/lez -s /usr/sbin/nologin lez \
    && mkdir -p /var/lib/lez/taker /var/lib/lez/zec /run/lez/taker /run/lez-wallet/taker /delivery \
    && chown -R lez:lez /var/lib/lez /run/lez /run/lez-wallet /delivery \
    && chmod 0700 /var/lib/lez/taker /var/lib/lez/zec /run/lez/taker /run/lez-wallet/taker /delivery

# The package, installed where Basecamp installs a user's modules (<user
# dir>/modules, which Basecamp scans beside its own): the package's one
# platform variant, its manifest and the variant's name, as Basecamp's package
# manager lays a module out. The tools the stack calls by their usual names
# (the health check, the CLI, scripts/record-evidence.py, ...) link to the
# installed files.
COPY --chmod=0444 lez_taker_node.lgx /tmp/lez_taker_node.lgx
RUN set -eu; module=/var/lez/basecamp-taker-user/modules/lez_taker_node; unpacked="$(mktemp -d)"; \
    tar -xzf /tmp/lez_taker_node.lgx -C "$unpacked"; \
    variant="$(jq -r '.main | keys[0]' "$unpacked/manifest.json")"; \
    mkdir -p "$module"; cp -R "$unpacked/variants/$variant/." "$module/"; \
    cp "$unpacked/manifest.json" "$module/"; printf '%s\n' "$variant" > "$module/variant"; \
    rm -rf "$unpacked" /tmp/lez_taker_node.lgx; \
    test -x "$module/bin/node-entrypoint.sh"; test -x "$module/bin/lez-taker-node"; \
    for tool in "$module"/bin/*; do \
      case "${tool##*/}" in payload.json|node-entrypoint.sh) ;; *) ln -s "$tool" "/usr/local/bin/${tool##*/}" ;; esac; \
    done; \
    chown -R 4713:4713 /var/lez/basecamp-taker-user/modules

COPY --from=strip --chmod=0555 /payload/ /usr/local/bin/
COPY --chmod=0444 build-source.txt sidecar-source.txt /usr/local/share/lez/
RUN chmod 0755 /usr/local/share/lez

USER lez
WORKDIR /var/lib/lez
# Basecamp's entrypoint (the base image's): Xvfb and VNC, the role's desk opened, then
# the desk starts the signer and the Node.
ENTRYPOINT ["/entrypoint.sh"]
