#!/usr/bin/env bash
# Cut a release: tools/release.sh 2.1.0   (see tools/release.py; add --dry-run to rehearse)
cd "$(dirname "$0")/.."
exec "${PYTHON:-python3.14}" tools/release.py "$@"
