#!/bin/sh
# Build the maintained firmware and package source + ELF (never game ROMs/saves).
set -eu
ROOT="$(CDPATH= cd "$(dirname "$0")/.." && pwd)"
sh "$ROOT/scripts/build-release.sh"
python3 "$ROOT/scripts/package-release.py"
