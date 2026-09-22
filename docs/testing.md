# Verification and release checks

Host and emulator checks pass on the consolidated source. The user confirmed
physical recovery after the SRAM-resident MBC3 change. That confirmation does
not qualify every transaction, network, SD card or power-loss scenario.

## Firmware and backend

```sh
GBDK_PATH=/path/to/gbdk sh scripts/build-release.sh
```

This builds the default `crystal-seed` feature with locked dependencies and
runs `cartridge-core` and `signer-probe` on the host target. The current suite
has 104 tests: 54 cartridge/transport/save tests, 31 shared backend unit tests,
3 reviewed-signing tests, 14 SD tests, and 2 independent seed-vector tests.
Tests for removed alternate state machines are intentionally no longer counted.

On the tested macOS setup, use LLVM for the ARM C dependency:

```sh
env 'CC_thumbv8m.main_none_eabihf=/opt/homebrew/opt/llvm/bin/clang' \
    'AR_thumbv8m.main_none_eabihf=/opt/homebrew/opt/llvm/bin/llvm-ar' \
    GBDK_PATH=/path/to/gbdk sh scripts/build-release.sh
```

The default firmware's MBC3 loop must be in RAM, not XIP flash. Inspect the ELF
with `llvm-nm -C` and `llvm-objdump -D`: the mapper's `run` symbol must start at a
`0x200...` RAM address, and the bank-select path must not call flash routines
before publishing the DMA bank pointer.

## Active ROM regression

Use Python 3.12+, RGBDS 1.0.3, the pinned upstream commit, and a host Rust compiler:

```sh
python3 -m venv target/python
target/python/bin/pip install -r integrations/pokecrystal/requirements-test.txt
HOST=$(rustc -vV | sed -n 's/^host: //p')
cargo build -p signer-probe --example crystal_ui_oracle --release --locked --target "$HOST"
target/python/bin/python integrations/pokecrystal/build.py target/pokecrystal-upstream /path/to/rgbds-1.0.3
target/python/bin/python integrations/pokecrystal/test_seed.py target/crystal target/crystal-test --oracle "target/$HOST/release/examples/crystal_ui_oracle"
```

The test uses the actual Rust Terminal and a public mnemonic. It verifies six
menu cursor positions, word-count selection, passphrase-space editing, delayed
recovery, identity, every QR module, QR return, synthetic file navigation and
review of a public test PSBT,
LOCK, private-buffer wipe, unchanged game SRAM, ten entry/return cycles,
scrolling attributes, the actual game SAVE menu, and CONTINUE in a fresh
emulator instance. Screenshots are under `target/crystal-test/`.

The bus bridge and file listings are emulated. This does not test RP2350
scheduling, physical SD I/O or real-cartridge power loss. Hardware recovery was
confirmed on the pre-cleanup build; the consolidated firmware and latest UI
still need a physical smoke test after this source cleanup.

## Before publishing

- Run both workflows above and inspect the UI screenshots.
- Test recovery/lock/return and ordinary SAVE/power-cycle/CONTINUE on hardware.
- Test reviewed PSBT import/sign/export using public synthetic funds/fixtures.
- Stage and sign the source changes; rebuild from the resulting clean commit.
- Run `GBDK_PATH=/path/to/gbdk sh scripts/package-release.sh` and inspect
  `target/release/savepoint-signer-source-firmware.tar.gz` and `SHA256SUMS`.

The package includes the exact source snapshot, firmware ELF, and per-file
hashes. Its manifest records HEAD and whether the source was dirty. A local
pre-commit package is useful for review but should be rebuilt after signing.
Game ROMs, emulator saves and user PSBTs are excluded. The CI workflow runs both
firmware/host tests and the active ROM regression; the tag workflow waits for
those checks and creates a prerelease draft. Remote CI has not been run from
this local cleanup session.
