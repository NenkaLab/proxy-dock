#!/bin/sh
# The Linux checks CI runs (clippy, tests, a release build) in Docker, from Windows or macOS:
#     docker run --rm -v "<repo>:/src" -v proxydock-cargo:/usr/local/cargo/registry \
#         -v proxydock-target:/target -e CARGO_TARGET_DIR=/target rust:bookworm sh /src/tools/docker_linux_check.sh
# The build and the crates stay in the two volumes between runs. The engines for Linux land in bin/linux-amd64.
set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends pkg-config python3 libfontconfig1-dev libfreetype-dev \
    libxkbcommon-dev libwayland-dev libgtk-3-dev libayatana-appindicator3-dev libxdo-dev >/dev/null
cd /src
python3 tools/fetch_singbox.py
# (a build folder outside the repository: the engines next to it, where the app and the TUN helper look)
if [ -n "$CARGO_TARGET_DIR" ]; then
    mkdir -p "$CARGO_TARGET_DIR/bin" && cp -r bin/linux-* "$CARGO_TARGET_DIR/bin/"
fi
cd native
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p proxydock
echo "linux checks passed"
