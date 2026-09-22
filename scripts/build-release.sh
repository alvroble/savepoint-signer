#!/bin/sh
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
set -eu

ROOT="$(CDPATH= cd "$(dirname "$0")/.." && pwd)"
TARGET="thumbv8m.main-none-eabihf"

if [ -z "${GBDK_PATH:-}" ]; then
    if [ -x "$ROOT/target/toolchains/gbdk/bin/lcc" ]; then
        GBDK_PATH="$ROOT/target/toolchains/gbdk"
    elif [ -x "$ROOT/../toolchains/gbdk/bin/lcc" ]; then
        GBDK_PATH="$ROOT/../toolchains/gbdk"
    fi
fi

if [ -z "${GBDK_PATH:-}" ] || [ ! -x "$GBDK_PATH/bin/lcc" ]; then
    echo "error: GBDK lcc not found; set GBDK_PATH to the GBDK root" >&2
    exit 1
fi

# Unit separator (0x1f) encoded rustflags. This replaces, not merges,
# rustflags from any ancestor .cargo/config.toml.
SEP="$(printf '\037')"
ENCODED="-C${SEP}link-arg=--nmagic${SEP}-C${SEP}link-arg=-Tlink.x${SEP}-C${SEP}link-arg=-Tdefmt.x${SEP}-C${SEP}target-cpu=cortex-m33"

export GBDK_PATH
export CARGO_ENCODED_RUSTFLAGS="$ENCODED"
export DEFMT_LOG="${DEFMT_LOG:-debug}"
export VERSION_MAJOR="${VERSION_MAJOR:-255}"
export VERSION_MINOR="${VERSION_MINOR:-255}"
export VERSION_PATCH="${VERSION_PATCH:-255}"
export RELEASE_TYPE="${RELEASE_TYPE:-U}"
unset RUSTFLAGS

cd "$ROOT"
cargo build --release --locked --target "$TARGET" --manifest-path "$ROOT/Cargo.toml" "$@"

ELF="$ROOT/target/$TARGET/release/rp2350-gameboy-cartridge"
if [ ! -f "$ELF" ]; then
    echo "error: expected ELF was not produced: $ELF" >&2
    exit 1
fi

echo
echo "ELF     : $ELF"
printf 'SHA-256 : '
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$ELF" | awk '{print $1}'
else
    shasum -a 256 "$ELF" | awk '{print $1}'
fi
if command -v rust-size >/dev/null 2>&1; then
    rust-size "$ELF"
elif command -v llvm-size >/dev/null 2>&1; then
    llvm-size "$ELF"
fi
echo
echo "Host tests (cartridge-core and signer-probe) — host rustflags, not the thumb linker scripts:"
unset CARGO_ENCODED_RUSTFLAGS
# Alpine's GCC enables stack protection in secp256k1-sys's C code, but the
# Rust musl test linker does not provide __stack_chk_fail. This applies only
# to native test dependencies; the firmware ELF has already been built.
CFLAGS="${CFLAGS:+$CFLAGS }-fno-stack-protector"
export CFLAGS
HOST="$(rustc -vV | sed -n 's/^host: //p')"
cargo test -p cartridge-core -p signer-probe --locked --target "$HOST" --manifest-path "$ROOT/Cargo.toml"
