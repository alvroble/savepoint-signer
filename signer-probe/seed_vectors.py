"""Reproduce public M3 recovery fixtures with independent embit.
Run with target/embit-oracle/bin/python; never use private mnemonics here.
"""
from embit import bip39, bip32, networks, script
from embit.descriptor import Descriptor
from pathlib import Path

rows = []
for count, passphrase in [(12, ""), (12, "TREZOR"), (24, "TREZOR"), (12, " TREZOR ")]:
    words = bip39.mnemonic_from_bytes(bytes(16 if count == 12 else 32))
    master = bip32.HDKey.from_seed(bip39.mnemonic_to_seed(words, passphrase))
    account = master.derive("m/84h/1h/0h").to_public()
    addresses = [script.p2wpkh(account.derive([0, i]).key).address(networks.NETWORKS["test"]) for i in [0, 1000]]
    xpub = account.to_base58(version=networks.NETWORKS["test"]["xpub"])
    for branch in [0, 1]:
        body = f"wpkh([{master.my_fingerprint.hex()}/84h/1h/0h]{xpub}/{branch}/*)"
        imported = Descriptor.from_string(body)
        for index in [0, 1000]:
            assert imported.derive(index).script_pubkey().data == script.p2wpkh(account.derive([branch, index]).key).data
    rows.append("\t".join([words, passphrase, master.my_fingerprint.hex(),
                           account.to_base58(version=networks.NETWORKS["test"]["xpub"]), *addresses]))
expected = "\n".join(rows) + "\n"
path = Path(__file__).parent / "tests" / "seed-vectors.tsv"
if path.exists():
    assert path.read_text() == expected, "recovery vector mismatch"
    print("PASS: four recovery/account/address vectors match embit")
else:
    path.write_text(expected)
    print(f"Wrote public vectors to {path}")
