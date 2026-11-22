#!/usr/bin/env bash
# Build everything this machine can build (win / linux / mac) and print a summary
# Build logic lives in tools/build.py. Override the interpreter with PYTHON=/path/to/python3.14
cd "$(dirname "$0")"
exec "${PYTHON:-python3.14}" tools/build.py all
