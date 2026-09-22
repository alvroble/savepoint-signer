#!/usr/bin/env bash
# Build the maintained firmware and package source + ELF (never game ROMs/saves).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
bash "$ROOT/scripts/build-release.sh"
python3 "$ROOT/scripts/package-release.py"
