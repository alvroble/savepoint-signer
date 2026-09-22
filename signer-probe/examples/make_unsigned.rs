//! Build the bounded-policy fixture and write the unsigned PSBT to disk.
//!
//! Used by `usb_review.py` to drive the on-device review flow without
//! needing a separate desktop coordinator. The fixture matches the policy in
//! `signer-probe::review`: one input owned by the fixed test seed, two
//! outputs (one change back to the same key, one external destination),
//! SIGHASH_ALL, fee 10 000 sat.

use std::env;
use std::fs;
use std::io::Write;
use std::process::ExitCode;
use std::str::FromStr;

use bitcoin::{
    absolute,
    bip32::{DerivationPath, Xpriv},
    hashes::Hash,
    psbt::Psbt,
    secp256k1::Secp256k1,
    transaction, Address, Amount, CompressedPublicKey, EcdsaSighashType, Network, OutPoint,
    ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("usage: make-unsigned OUT.psbt");
        return ExitCode::from(2);
    }
    let output_path = &args[1];

    let secp = Secp256k1::new();
    let mnemonic = bip39::Mnemonic::from_entropy(&[0; 16]).unwrap();
    let seed = mnemonic.to_seed_normalized("");
    let master = Xpriv::new_master(Network::Testnet, &seed).unwrap();
    let path: DerivationPath = "m/84'/1'/0'/0/0".parse().unwrap();
    let key = master.derive_priv(&secp, &path).unwrap();
    let public = key.private_key.public_key(&secp);
    let our_script = Address::p2wpkh(&CompressedPublicKey(public), Network::Testnet).script_pubkey();
    let external = Address::from_str("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx")
        .unwrap()
        .assume_checked()
        .script_pubkey();

    let tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::from_byte_array([0x22; 32]),
                vout: 0,
            },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![],
    };
    let mut psbt = Psbt::from_unsigned_tx(tx).unwrap();
    psbt.inputs[0].witness_utxo = Some(TxOut {
        value: Amount::from_sat(100_000),
        script_pubkey: our_script.clone(),
    });
    psbt.inputs[0]
        .bip32_derivation
        .insert(public, (master.fingerprint(&secp), path));
    psbt.inputs[0].sighash_type = Some(EcdsaSighashType::All.into());
    psbt.outputs.push(Default::default());
    psbt.outputs.push(Default::default());
    psbt.unsigned_tx.output.push(TxOut {
        value: Amount::from_sat(40_000),
        script_pubkey: our_script,
    });
    psbt.unsigned_tx.output.push(TxOut {
        value: Amount::from_sat(50_000),
        script_pubkey: external,
    });
    let bytes = psbt.serialize();

    // Sanity-check the policy locally before handing the PSBT to the cart.
    let review = signer_probe::review(&bytes).expect("fixture passes policy");
    assert_eq!(review.outputs().len(), 2);
    assert!(review.outputs()[0].is_change);
    assert!(!review.outputs()[1].is_change);
    assert_eq!(review.fee_sat(), 10_000);

    let mut file = fs::File::create(output_path).expect("create output");
    file.write_all(&bytes).expect("write output");
    eprintln!("wrote {} bytes to {}", bytes.len(), output_path);
    ExitCode::SUCCESS
}
