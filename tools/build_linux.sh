#!/usr/bin/env bash
# Linux build -> dist/linux/ProxyDock/ProxyDock (via Docker on Windows/macOS)
# Build logic lives in tools/build.py. Override the interpreter with PYTHON=/path/to/python3.14
cd "$(dirname "$0")/.."
exec "${PYTHON:-python3.14}" tools/build.py linux
