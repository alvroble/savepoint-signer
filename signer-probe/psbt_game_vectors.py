"""Public synthetic PSBTs for the game, generated and checked with embit."""
from pathlib import Path
from embit import bip39, bip32, psbt, transaction, script, ec
ROOT = Path(__file__).resolve().parent
WORDS = bip39.mnemonic_from_bytes(bytes(16))
def fixture(mainnet):
    root = bip32.HDKey.from_seed(bip39.mnemonic_to_seed(WORDS))
    base = [0x80000054, 0x80000000 + (0 if mainnet else 1), 0x80000000]
    source, change = base + [0, 7], base + [1, 3]
    pub = root.derive(source).key.get_public_key()
    change_pub = root.derive(change).key.get_public_key()
    recipient = ec.PrivateKey(bytes([7])*32).get_public_key()
    tx = transaction.Transaction(vin=[transaction.TransactionInput(bytes([0x22])*32, 0)],
        vout=[transaction.TransactionOutput(60000, script.p2wpkh(recipient)),
              transaction.TransactionOutput(39000, script.p2wpkh(change_pub))])
    p = psbt.PSBT(tx)
    p.inputs[0].witness_utxo = transaction.TransactionOutput(100000, script.p2wpkh(pub))
    p.inputs[0].bip32_derivations[pub] = psbt.DerivationPath(root.my_fingerprint, source)
    p.outputs[1].bip32_derivations[change_pub] = psbt.DerivationPath(root.my_fingerprint, change)
    return p
def verify_signed(payload, mainnet):
    signed = psbt.PSBT.parse(payload)
    original = fixture(mainnet)
    assert signed.tx.serialize() == original.tx.serialize()
    assert signed.outputs[1].bip32_derivations == original.outputs[1].bip32_derivations
    assert len(signed.inputs[0].partial_sigs) == 1
    key, sig = next(iter(signed.inputs[0].partial_sigs.items()))
    assert key == next(iter(original.inputs[0].bip32_derivations))
    assert sig[-1] == 1 and key.verify(ec.Signature.parse(sig[:-1]), original.sighash(0))
if __name__ == "__main__":
    folder = ROOT/'tests/fixtures'
    folder.mkdir(exist_ok=True)
    for main in (False, True):
        dest=folder/('game-main.psb' if main else 'game-test.psb')
        payload=fixture(main).serialize()
        if dest.exists(): assert dest.read_bytes() == payload
        else: dest.write_bytes(payload)
    print('PASS independent mainnet/testnet PSBT fixtures')
