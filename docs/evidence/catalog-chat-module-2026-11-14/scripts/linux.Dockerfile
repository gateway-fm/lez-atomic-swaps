FROM debian:trixie-slim
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
      ca-certificates jq procps file socat curl xvfb xdotool x11-utils imagemagick \
      libgl1 libegl1 libopengl0 libfontconfig1 libfreetype6 libxkbcommon0 libxkbcommon-x11-0 libdbus-1-3 libglib2.0-0t64 \
      libxcb1 libxcb-cursor0 libxcb-icccm4 libxcb-image0 libxcb-keysyms1 libxcb-randr0 libxcb-render-util0 \
      libxcb-shape0 libxcb-xfixes0 libxcb-xinerama0 libxcb-xkb1 libxcb-sync1 libxcb-shm0 libxcb-render0 libxcb-glx0 \
      libx11-6 libx11-xcb1 libxext6 libxrender1 libxi6 libsm6 libice6 libnss3 libasound2t64 fonts-dejavu-core libharfbuzz0b libfribidi0 \
    >/dev/null && rm -rf /var/lib/apt/lists/*
COPY logosctl.AppImage basecamp.AppImage /opt/
RUN cd /opt && chmod +x logosctl.AppImage basecamp.AppImage \
 && ./logosctl.AppImage --appimage-extract >/dev/null && mv squashfs-root logosctl \
 && ./basecamp.AppImage --appimage-extract >/dev/null && mv squashfs-root basecamp \
 && rm logosctl.AppImage basecamp.AppImage
