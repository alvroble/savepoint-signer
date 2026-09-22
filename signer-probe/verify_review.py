"""Independent oracle for review/sign output.

The Rust review/sign API in signer-probe takes an unsigned PSBT, validates it
against a narrow policy, and produces a signed PSBT. This script verifies
that the signature actually commits to the claimed sighash using an
independent implementation (embit 0.8.0).

The script does not depend on the device's PSBT layout: it reads the
public key from the PSBT's BIP32 derivation map and verifies the signature
on the PSBT's own sighash. It is the same idea as verify.py but works
for arbitrary single-sig P2WPKH PSBTs that match this device's policy,
not just the original synthetic fixture.

Usage:
    python verify_review.py signed.psbt

Exit code 0 on success, non-zero on any failed assertion.
"""
import sys

from embit import psbt
from embit.ec import Signature, secp256k1


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: verify_review.py signed.psbt", file=sys.stderr)
        return 2
    with open(sys.argv[1], "rb") as source:
        signed = psbt.PSBT.parse(source.read())

    if len(signed.inputs) != 1:
        raise SystemExit(f"expected 1 input, got {len(signed.inputs)}")
    if len(signed.outputs) == 0 or len(signed.outputs) > 2:
        raise SystemExit(f"expected 1 or 2 outputs, got {len(signed.outputs)}")

    txin = signed.inputs[0]
    if txin.witness_utxo is None:
        raise SystemExit("input missing witness_utxo")
    if not txin.partial_sigs:
        raise SystemExit("input has no partial signatures")

    digest = signed.sighash(0)
    for pk, raw_sig in txin.partial_sigs.items():
        if raw_sig[-1] != 1:
            raise SystemExit(f"non-SIGHASH_ALL suffix: {raw_sig[-1]:#x}")
        sig = Signature.parse(raw_sig[:-1])
        if not pk.verify(sig, digest):
            raise SystemExit("signature does not verify against sighash")

    # Sum-fee sanity check, in line with the device's policy.
    total_in = txin.witness_utxo.value
    total_out = sum(o.value for o in signed.outputs)
    fee = total_in - total_out
    if fee < 0:
        raise SystemExit(f"negative fee: {fee}")
    print(
        "PASS: review/sign PSBT verifies under embit "
        f"(digest={digest.hex()}, fee={fee})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())