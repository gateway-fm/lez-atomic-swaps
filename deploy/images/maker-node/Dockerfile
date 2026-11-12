# The Maker's Basecamp: the basecamp-ui image (build context "basecamp",
# compose.yaml) with the Maker's Node payload package installed. Its desk
# starts the Maker's LEZ signer in Basecamp's lez_core and has Basecamp's
# lez_swap_node launch and supervise the Maker Node from the installed
# package — the swap engine runs inside the party's Basecamp. Nothing of the
# Node is copied into this image on its own: the package
# (lez_maker_node.lgx, scripts/package-role-node.sh) carries the Node, its
# CLI, its actors, the LEZ sidecar and the launcher; this image installs it,
# and compose.yaml configures it. Only the Maker's executables are here
# (deploy/role-symmetry.json).

# "basecamp" is a build context (compose.yaml: service:basecamp-ui), not a registry image.
# hadolint ignore=DL3006
FROM basecamp

LABEL org.logos-co.atomic-swaps.purpose="local-maker-node"

# What the package's launcher runs besides the package's own executables.
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl jq libpcsclite1 procps socat python3 util-linux \
    && rm -rf /var/lib/apt/lists/*

# The state directories the Compose volumes mount here (maker_state, zec_maker,
# delivery_dir, ...): a new named volume takes the owner and mode of the image
# directory it is first mounted on, so no init container prepares them.
RUN groupadd -r -g 4713 lez && useradd -r -u 4713 -g lez -d /var/lib/lez -s /usr/sbin/nologin lez \
    && mkdir -p /var/lib/lez/maker /var/lib/lez/zec /run/lez/maker /run/lez-wallet/maker /delivery \
    && chown -R lez:lez /var/lib/lez /run/lez /run/lez-wallet /delivery \
    && chmod 0700 /var/lib/lez/maker /var/lib/lez/zec /run/lez/maker /run/lez-wallet/maker /delivery

# The package, installed where Basecamp installs a user's modules (<user
# dir>/modules, which Basecamp scans beside its own): the package's one
# platform variant, its manifest and the variant's name, as Basecamp's package
# manager lays a module out (the manifest's dependencies as bare names, the
# form Basecamp 0.2.0 reads: scripts/stage-basecamp-package.sh). The tools the
# stack calls by their usual names (the health check, the CLI,
# scripts/record-evidence.py, ...) link to the installed files.
COPY --chmod=0444 lez_maker_node.lgx /tmp/lez_maker_node.lgx
RUN set -eu; module=/var/lez/basecamp-maker-user/modules/lez_maker_node; unpacked="$(mktemp -d)"; \
    tar -xzf /tmp/lez_maker_node.lgx -C "$unpacked"; \
    variant="$(jq -r '.main | keys[0]' "$unpacked/manifest.json")"; \
    mkdir -p "$module"; cp -R "$unpacked/variants/$variant/." "$module/"; \
    jq 'if .dependencies then .dependencies |= map(if type == "object" then .name else . end) else . end' \
      "$unpacked/manifest.json" > "$module/manifest.json"; printf '%s\n' "$variant" > "$module/variant"; \
    rm -rf "$unpacked" /tmp/lez_maker_node.lgx; \
    test -x "$module/bin/node-entrypoint.sh"; test -x "$module/bin/lez-maker-node"; \
    for tool in "$module"/bin/*; do \
      case "${tool##*/}" in payload.json|node-entrypoint.sh) ;; *) ln -s "$tool" "/usr/local/bin/${tool##*/}" ;; esac; \
    done; \
    chown -R 4713:4713 /var/lez/basecamp-maker-user/modules

COPY --chmod=0444 build-source.txt sidecar-source.txt /usr/local/share/lez/
RUN chmod 0755 /usr/local/share/lez

USER lez
WORKDIR /var/lib/lez
# Basecamp's entrypoint (the base image's): Xvfb and VNC, the role's desk opened, then
# the desk starts the signer and the Node.
ENTRYPOINT ["/entrypoint.sh"]
