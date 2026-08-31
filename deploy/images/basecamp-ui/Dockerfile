FROM debian:trixie-slim

ARG DEBIAN_FRONTEND=noninteractive
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        xvfb fluxbox x11vnc xdotool x11-apps imagemagick procps fonts-dejavu-core \
        ca-certificates nodejs \
        libopengl0 libgl1 libegl1 libxkbcommon-x11-0 libvulkan1 \
        libgles2 libgl1-mesa-dri \
    && rm -rf /var/lib/apt/lists/*

# portable (self-contained) Basecamp 0.2.0-RC3 test twin with the QML
# inspector enabled; pristine role install trees; qt-mcp test framework
COPY assets/bundle /opt/basecamp
COPY assets/maker-user /var/lez-assets/maker-user
COPY assets/taker-user /var/lez-assets/taker-user
COPY assets/qt-mcp /opt/qt-mcp
COPY entrypoint.sh /entrypoint.sh

RUN chmod 0755 /entrypoint.sh /opt/basecamp/bin/LogosBasecamp \
    && printf '#!/bin/sh\nexec /opt/basecamp/bin/LogosBasecamp --user-dir "${BASECAMP_USER_DIR:-/var/lez/basecamp-%s-user}" "$@"\n' maker > /usr/local/bin/lez-maker-ui \
    && printf '#!/bin/sh\nexec /opt/basecamp/bin/LogosBasecamp --user-dir "${BASECAMP_USER_DIR:-/var/lez/basecamp-%s-user}" "$@"\n' taker > /usr/local/bin/lez-taker-ui \
    && chmod 0755 /usr/local/bin/lez-maker-ui /usr/local/bin/lez-taker-ui \
    && mkdir -p /var/lez \
    && cp -a /var/lez-assets/maker-user /var/lez/basecamp-maker-user \
    && cp -a /var/lez-assets/taker-user /var/lez/basecamp-taker-user \
    && mkdir -p /var/lez-assets/both-user/plugins /var/lez/basecamp-user/plugins \
    && cp -a /var/lez-assets/maker-user/plugins/lez_atomic_swap_maker /var/lez-assets/both-user/plugins/ \
    && cp -a /var/lez-assets/taker-user/plugins/lez_atomic_swap_taker /var/lez-assets/both-user/plugins/ \
    && cp -a /var/lez-assets/both-user/plugins/. /var/lez/basecamp-user/plugins/ \
    && chown -R 4713:4713 /var/lez /var/lez-assets \
    && chmod -R u+rwX /var/lez /var/lez-assets

ENV DISPLAY=:0 \
    LIBGL_ALWAYS_SOFTWARE=1 \
    LEZ_MAKER_RPC_SOCKET=/run/lez/maker/node.sock \
    LEZ_TAKER_RPC_SOCKET=/run/lez/taker/node.sock \
    BASECAMP_ROLE=maker \
    VNC_GEOMETRY=1680x1050

EXPOSE 5900
HEALTHCHECK --interval=30s --timeout=10s --retries=10 --start-period=40s \
    CMD pgrep -f LogosBasecamp >/dev/null || exit 1

ENTRYPOINT ["/entrypoint.sh"]
