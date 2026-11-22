#!/usr/bin/env bash
# macOS build -> dist/mac/ProxyDock.app (macOS only)
# Build logic lives in tools/build.py. Override the interpreter with PYTHON=/path/to/python3.14
cd "$(dirname "$0")"
exec "${PYTHON:-python3.14}" tools/build.py mac
