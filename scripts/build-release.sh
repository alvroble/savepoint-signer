#!/usr/bin/env bash
# Build the release firmware ELF with a single, explicit rustflags set.
#
# This repository's `.cargo/config.toml` already lists the required
# linker arguments. A nested checkout makes Cargo merge parent and child
# rustflags arrays, which duplicates `-Tlink.x` and fails with
# "FLASH region already defined".
#
# CARGO_ENCODED_RUSTFLAGS overrides config rustflags entirely, so this
# script produces the same artifact from a nested worktree or a
# standalone checkout.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="thumbv8m.main-none-eabihf"

if [[ -z "${GBDK_PATH:-}" ]]; then
    if [[ -x "$ROOT/target/toolchains/gbdk/bin/lcc" ]]; then
        GBDK_PATH="$ROOT/target/toolchains/gbdk"
    elif [[ -x "$ROOT/../toolchains/gbdk/bin/lcc" ]]; then
        GBDK_PATH="$ROOT/../toolchains/gbdk"
    fi
fi

if [[ -z "${GBDK_PATH:-}" || ! -x "$GBDK_PATH/bin/lcc" ]]; then
    echo "error: GBDK lcc not found; set GBDK_PATH to the GBDK root" >&2
    exit 1
fi

# Unit separator (0x1f) encoded rustflags. This replaces, not merges,
# rustflags from any ancestor .cargo/config.toml.
ENCODED=$'-C\x1flink-arg=--nmagic\x1f-C\x1flink-arg=-Tlink.x\x1f-C\x1flink-arg=-Tdefmt.x\x1f-C\x1ftarget-cpu=cortex-m33'

export GBDK_PATH
export CARGO_ENCODED_RUSTFLAGS="$ENCODED"
export DEFMT_LOG="${DEFMT_LOG:-debug}"
export VERSION_MAJOR="${VERSION_MAJOR:-255}"
export VERSION_MINOR="${VERSION_MINOR:-255}"
export VERSION_PATCH="${VERSION_PATCH:-255}"
export RELEASE_TYPE="${RELEASE_TYPE:-U}"
unset RUSTFLAGS || true

cd "$ROOT"
cargo build --release --locked --target "$TARGET" --manifest-path "$ROOT/Cargo.toml" "$@"

ELF="$ROOT/target/$TARGET/release/rp2350-gameboy-cartridge"
if [[ ! -f "$ELF" ]]; then
    echo "error: expected ELF was not produced: $ELF" >&2
    exit 1
fi

echo
echo "ELF     : $ELF"
echo -n "SHA-256 : "
shasum -a 256 "$ELF" | awk '{print $1}'
if command -v rust-size >/dev/null 2>&1; then
    rust-size "$ELF"
elif command -v llvm-size >/dev/null 2>&1; then
    llvm-size "$ELF"
fi
echo
echo "Host tests (cartridge-core and signer-probe) — host rustflags, not the thumb linker scripts:"
unset CARGO_ENCODED_RUSTFLAGS
HOST="$(rustc -vV | sed -n 's/^host: //p')"
cargo test -p cartridge-core -p signer-probe --locked --target "$HOST" --manifest-path "$ROOT/Cargo.toml"
