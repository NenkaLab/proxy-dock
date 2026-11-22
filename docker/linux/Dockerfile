# Linux 빌드용 이미지 (build_linux.bat 이 사용)
# bookworm(glibc 2.36) 기반이라 비교적 오래된 배포판에서도 실행 파일이 돈다.
FROM python:3.14-slim-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
      binutils curl libatomic1 \
      libgl1 libegl1 libglib2.0-0 libdbus-1-3 libfontconfig1 libfreetype6 \
      libxkbcommon0 libxkbcommon-x11-0 libx11-xcb1 \
      libxcb1 libxcb-cursor0 libxcb-icccm4 libxcb-image0 libxcb-keysyms1 libxcb-randr0 \
      libxcb-render-util0 libxcb-shape0 libxcb-xinerama0 libxcb-xkb1 libxcb-xfixes0 \
 && rm -rf /var/lib/apt/lists/*
