use signer_probe::{
    bitcoin::{self, psbt::Psbt, secp256k1::Secp256k1, Network},
    seed::{ReviewBinding, SeedSession},
};
use zeroize::Zeroizing;
const WORDS: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
fn fixture(main: bool) -> Vec<u8> {
    if main {
        include_bytes!("fixtures/game-main.psb").to_vec()
    } else {
        include_bytes!("fixtures/game-test.psb").to_vec()
    }
}
#[test]
fn both_networks_sign_nonzero_receive_index_and_verify_change() {
    for net in [Network::Bitcoin, Network::Testnet] {
        let seed = SeedSession::recover(WORDS, "", net).unwrap();
        let bytes = fixture(net == Network::Bitcoin);
        let review = seed.review_psbt(&bytes).unwrap();
        assert_eq!(review.fee_sat(), 1000);
        assert_eq!(review.outputs()[0].amount_sat, 60000);
        assert!(!review.outputs()[0].is_change);
        assert!(review.outputs()[1].is_change);
        let mut binding = ReviewBinding {
            psbt_bytes: Zeroizing::new(review.approved_psbt_bytes()),
            sighash: *review.sighash(),
            change_path: review.change_path.clone(),
            change_public_key: review.change_public_key,
        };
        let signed = seed.sign_psbt(&Secp256k1::new(), &binding).unwrap();
        assert_eq!(
            Psbt::deserialize(&signed.signed_psbt).unwrap().inputs[0]
                .partial_sigs
                .len(),
            1
        );
        assert!(seed.review_psbt(&signed.signed_psbt).is_err());
        binding.sighash[0] ^= 1;
        assert!(seed.sign_psbt(&Secp256k1::new(), &binding).is_err());
        assert!(seed
            .review_psbt(&fixture(net != Network::Bitcoin))
            .is_err());
    }
}
#[test]
fn rejects_tampered_origins_scripts_fees_and_unsupported_transactions() {
    let seed = SeedSession::recover(WORDS, "", Network::Bitcoin).unwrap();
    for mutation in 0..10 {
        let mut p = Psbt::deserialize(&fixture(true)).unwrap();
        match mutation {
            0 => p.inputs[0].bip32_derivation.clear(),
            1 => {
                p.inputs[0].bip32_derivation.values_mut().next().unwrap().0 =
                    bitcoin::bip32::Fingerprint::from([0; 4])
            }
            2 => {
                p.inputs[0].bip32_derivation.values_mut().next().unwrap().1 =
                    "m/84'/0'/0'/0/8".parse().unwrap()
            }
            3 => {
                p.inputs[0].witness_utxo.as_mut().unwrap().script_pubkey =
                    p.unsigned_tx.output[0].script_pubkey.clone()
            }
            4 => {
                p.outputs[1].bip32_derivation.values_mut().next().unwrap().1 =
                    "m/84'/0'/0'/1/4".parse().unwrap()
            }
            5 => p.unsigned_tx.output[0].value = bitcoin::Amount::from_sat(200000),
            6 => p.inputs[0].sighash_type = Some(bitcoin::EcdsaSighashType::None.into()),
            7 => p.inputs[0].witness_utxo = None,
            8 => p.inputs[0].non_witness_utxo = Some(p.unsigned_tx.clone()),
            _ => {
                p.inputs.push(p.inputs[0].clone());
                p.unsigned_tx.input.push(p.unsigned_tx.input[0].clone());
            }
        }
        assert!(
            seed.review_psbt(&p.serialize()).is_err(),
            "mutation {mutation}"
        );
    }
}

#[test]
fn ownership_errors_distinguish_network_origin_and_key() {
    let seed = SeedSession::recover(WORDS, "", Network::Bitcoin).unwrap();
    assert_eq!(seed.review_psbt(&fixture(false)).unwrap_err(), "Network mismatch    Check main/testnet");
    let mut p = Psbt::deserialize(&fixture(true)).unwrap();
    p.inputs[0].bip32_derivation.values_mut().next().unwrap().0 =
        bitcoin::bip32::Fingerprint::from([0; 4]);
    assert_eq!(seed.review_psbt(&p.serialize()).unwrap_err(), "Fingerprint mismatchCheck Sparrow origin");
    p = Psbt::deserialize(&fixture(true)).unwrap();
    p.inputs[0].bip32_derivation.values_mut().next().unwrap().1 =
        "m/84'/0'/0'/0/8".parse().unwrap();
    assert_eq!(seed.review_psbt(&p.serialize()).unwrap_err(), "Input key mismatch  Check seed + pass");
}
