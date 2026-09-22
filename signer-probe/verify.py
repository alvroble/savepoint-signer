"""Independent oracle: pip install embit==0.8.0; python verify.py probe.psbt."""
import sys
from embit import bip32, bip39, ec, psbt, script
from embit.networks import NETWORKS

with open(sys.argv[1], "rb") as source:
    signed = psbt.PSBT.parse(source.read())
seed = bip39.mnemonic_to_seed(bip39.mnemonic_from_bytes(bytes(16)))
key = bip32.HDKey.from_seed(seed).derive("m/84h/1h/0h/0/0")
public = key.key.get_public_key()
address = script.p2wpkh(public).address(NETWORKS["test"])
assert address == "tb1q6rz28mcfaxtmd6v789l9rrlrusdprr9pqcpvkl"
assert len(signed.inputs) == len(signed.outputs) == 1
assert signed.inputs[0].witness_utxo.value == 100_000
assert signed.outputs[0].value == 99_000
assert signed.outputs[0].script_pubkey == script.p2wpkh(public)
assert signed.inputs[0].witness_utxo.script_pubkey == script.p2wpkh(public)
assert len(signed.inputs[0].partial_sigs) == 1
signature = signed.inputs[0].partial_sigs[public]
assert signature[-1] == 1  # SIGHASH_ALL
digest = signed.sighash(0)
assert digest.hex() == "52f4d13355d9b3bf37467561c7f8ceb4fecd88b8ba6cdba3034093d8d44c7b90"
assert public.verify(ec.Signature.parse(signature[:-1]), digest)
print("PASS: independent address, amounts, sighash and signature verification")
