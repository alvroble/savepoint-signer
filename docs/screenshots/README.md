# Savepoint Signer in the game

These captures come directly from the 160 × 144 pixel PyBoy regression of the
patched Crystal ROM. The emulator talks to the production Rust seed state
machine through a test bridge. It uses the public 12-word `abandon … about`
BIP39 vector and a synthetic PSBT; no real seed or funds are pictured. The
file listing is supplied by the test bridge, so these images do not prove
physical SD-card operation.

| Step | Screenshot | What happens |
| --- | --- | --- |
| Enter | ![New Bark Town sign](new-bark-sign.png) | Talk to the New Bark Town sign twice. |
| Open | ![Signer menu](signer-menu.png) | Choose recovery, address, export, signing or return. |
| Recover | ![Mnemonic keyboard](recovery-keyboard.png) | Select BIP39 words using D-pad, A and SELECT. |
| Derive | ![Deriving keys](deriving-keys.png) | The RP2350 derives keys after the passphrase is submitted. |
| Verify | ![Public key identity](verify-keys.png) | Compare the fingerprint and address with a separate coordinator. |
| Export | ![Account QR](account-qr.png) | Display an account export QR, then B to return. |
| Select | ![PSBT picker](psbt-picker.png) | Pick a PSBT from the SD root on hardware. |
| Review | ![Output](review-output.png) ![Change](review-change.png) ![Fee](review-fee.png) ![Approval](review-approve.png) | Inspect each output and the fee before approval. |
| Return | ![Back in New Bark Town](return-to-game.png) | Lock the seed session and continue playing. |

These screenshots show a working emulator path, not a security certification.
**Do not recover a seed holding real funds or sign a real transaction with
this experimental build.** See [verification limits](../testing.md) and the
[README warning](../../README.md).
