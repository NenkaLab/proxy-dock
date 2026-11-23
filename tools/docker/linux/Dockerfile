# Image for Linux builds (used by build_linux.bat)
# Based on bookworm (glibc 2.36) so the executable also runs on fairly old distributions.
FROM python:3.14-slim-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
      binutils curl file libatomic1 \
      libgl1 libegl1 libglib2.0-0 libdbus-1-3 libfontconfig1 libfreetype6 \
      libxkbcommon0 libxkbcommon-x11-0 libx11-xcb1 \
      libxcb1 libxcb-cursor0 libxcb-icccm4 libxcb-image0 libxcb-keysyms1 libxcb-randr0 \
      libxcb-render-util0 libxcb-shape0 libxcb-xinerama0 libxcb-xkb1 libxcb-xfixes0 \
 && rm -rf /var/lib/apt/lists/*
