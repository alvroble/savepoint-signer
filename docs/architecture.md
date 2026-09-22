# Crystal integration architecture

The cartridge remains a general Game Boy flashcart. A Crystal ROM containing the
`CSD2` marker enables an additional signer command interface. Cryptography and
SD access stay on the RP2350; the Game Boy owns presentation and physical input.

| Component | Role |
| --- | --- |
| `src/gb_bootloader.rs` | Game selection, ROM loading, restore save before reset release |
| `src/gb_mbc.rs` | Game mapper; SRAM-resident MBC3 bank-select path |
| `src/crystal_seed.rs` | Transport adapter and core-1 signer/SD worker |
| `cartridge-core/src/seed_link.rs` | Bounded request, cancellation and page publication model |
| `signer-probe/src/terminal.rs` | Single signer command/state machine |
| `signer-probe/src/seed.rs` | Seed, account, address and PSBT policy/signing |
| `src/sd_psbt.rs` | FAT32 enumeration, bounded import and non-overwriting export |
| `integrations/pokecrystal/native/seed.asm` | Crystal frontend and engine return |

## Transport and ownership

`$7000` sets a command, `$7001` submits a nonzero ticket, and `$7002` requests a
32-byte page. A 38-byte ROM0 window carries publication sequence, acknowledged
ticket, busy flag, page number, payload, protocol version and final sequence.
Crystal snapshots and validates a page before consuming it. The report is
128 bytes; public QR bytes follow it. No signer traffic uses cartridge SRAM.

Core 1 owns Terminal and SD operations. Core 0 only accepts bounded requests
and publishes response pages. Duplicate tickets do not execute twice. LOCK
supersedes pending work and invalidates late response publication using an epoch;
core 1 performs actual session cleanup when it processes LOCK. The UI waits for
that acknowledgement before returning. A missing response reaches a timeout.

Recovery previously failed physically while the mapper executed from shared XIP
flash. Moving the MBC3 loop and direct bank-select path into RAM, and deferring
idle RTC processing while signer commands execute, produced user-confirmed
recovery. This is hardware evidence for that path, not a proof of every timing
case. RTC catch-up resumes afterward. Preserve this constraint when refactoring.

## State and security boundaries

The frontend uses reserved bank-2 WRAM for reports, QR staging and passphrase
echo. Native engine bank selection is restored after each access. Text helpers
preserve their destination pointer. Exit wipes the complete private allocation
and page staging, then restores the native map/menu lifecycle. Game SRAM belongs
to Crystal and is saved normally; secrets are not intentionally written to SD.

Recovery accepts 12/24 English BIP39 words and an ASCII passphrase. The terminal
supports mainnet/testnet BIP84 accounts, receive addresses and xpub/tpub or
zpub/vpub QR exports. The active policy accepts a bounded PSBT (4096 bytes), one
P2WPKH input, up to two P2WPKH outputs, SIGHASH_ALL and recognized derivation
paths. Review is bound to the seed and transaction before signing. Signed
PSBTs are exported for the desktop coordinator to finalize/broadcast.

This remains experimental. RAM zeroization is best-effort: library/compiler
copies are not comprehensively proven absent, and the cartridge has no secure
boot or trusted-display attestation. A modified ROM/firmware can lie about
review or extract keys. There is no production security qualification.

## Saves and release boundaries

The save worker and signer worker share the SD manager serially on core 1.
Save handles are closed between operations. Ordinary game saves use the existing
quiet-period coordinator and TMP/SAV recovery; seed entry must never sanitize
or overwrite game save ranges.

Only the town-sign integration is maintained. Earlier standalone ROMs, USB
signing demos, stone injection and presentation-only builds are removed. Public
cryptographic vectors remain as regression tools, not alternate firmware modes.
A release contains firmware and the pinned game source patch/builder. Full ROMs,
SD contents, local audit logs and private working data are excluded.
