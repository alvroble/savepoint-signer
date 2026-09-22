# Build the Savepoint Signer Crystal ROM

This is the only supported Crystal integration: the existing New Bark town sign
opens a native signer UI after two interactions. No extra overworld sprites or
map tiles are installed. The RP2350 performs recovery, account derivation and
reviewed PSBT signing; Crystal draws screens and sends commands.

## Build

Use Python 3.12+, make, a C compiler, patch, and RGBDS 1.0.3. The upstream commit
and tool version are pinned in `upstream.json`.

```sh
git clone https://github.com/pret/pokecrystal target/pokecrystal-upstream
python3 integrations/pokecrystal/build.py target/pokecrystal-upstream /path/to/rgbds-1.0.3
```

`build.py` archives the pinned upstream commit into a temporary tree, applies
`crystal.patch`, adds `native/seed.asm`, and invokes the upstream build. It
never edits the supplied upstream checkout. Output is `target/crystal/`:
`pokecrystal.gbc`, matching symbols/map, and a SHA-256 manifest.

Use the default cartridge firmware from `scripts/build-release.sh`. Copy the
ROM to the SD with the existing Crystal filename if preserving a save. Talk to
the New Bark town sign at `(8,8)` twice, dismissing the original dialogue between
interactions. B from the signer menu locks and returns to gameplay.

## Source layout

| File | Responsibility |
| --- | --- |
| `upstream.json` | Pinned source revision, RGBDS version and entry location |
| `crystal.patch` | Map event and assembly include |
| `native/seed.asm` | Native menu, recovery keyboard, review, QR and return wrapper |
| `build.py` | Isolated build and artifact hashes |
| `test_seed.py` | Actual backend oracle, UI, scrolling and game SAVE/CONTINUE |

The ROM transport uses command writes at `$7000–$7002` and a marked ROM0 response
window. It never uses Crystal's battery-backed SRAM as a signer mailbox. The UI
uses reserved WRAM bank 2, preserves engine bank selection and wipes its private
buffers on exit. See [architecture](../../docs/architecture.md).

## UI and controls

The menu fits the 20×18 tile screen. Labels and cursor use matching two-row
spacing; status identifies a loaded seed and its network. All six options
remain visible. The recovery screen shows word position/total and candidate
focus. A types or selects, B deletes, SELECT changes focus or passphrase keyboard
page; START submits the passphrase. The QR uses two pixels per module with a
white quiet zone and a B Back hint below it.

Tests and physical verification limits are recorded in
[testing](../../docs/testing.md). Generated images, game ROMs and saves belong
under ignored `target/`, not in the source tree.
