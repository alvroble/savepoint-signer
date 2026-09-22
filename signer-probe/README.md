# Shared signer backend

Despite its historical crate name, `signer-probe` is the no_std backend used by
the Crystal signer. `terminal::Terminal` owns recovery, identity, QR export,
PSBT review and approval states; `seed::SeedSession` implements derivation
and signing. There is one firmware adapter, `src/crystal_seed.rs`.

Public-vector examples and Python/embit helpers are retained for independent
cryptographic regression checks. They use public synthetic data and are not
USB or cartridge signing endpoints. Never fund the supplied test keys.

```sh
HOST=$(rustc -vV | sed -n 's/^host: //p')
cargo test -p signer-probe --target "$HOST" --locked
cargo build -p signer-probe --example crystal_ui_oracle --target "$HOST" --release --locked
```

`crystal_ui_oracle` connects the emulator test to the real Terminal. Its two file
entries are synthetic metadata, not evidence of physical SD operation. Runtime
limits and evidence are in [architecture](../docs/architecture.md) and
[testing](../docs/testing.md).
