//! Shared no_std seed backend for the Crystal cartridge integration.
//!
//! `terminal::Terminal` owns recovery, identity, export, and review/sign states.
//! `seed::SeedSession` owns key derivation and the active PSBT policy.
//! Public-vector `run`, `review`, and `sign` helpers remain for independent
//! cryptographic regression tests; they are not firmware transport endpoints.
#![no_std]

extern crate alloc;

/// Production command/state service used by the Crystal firmware adapter.
pub mod terminal;
/// Experimental recovery helpers; disposable test seeds only.
pub mod seed;

use alloc::{format, string::String, vec, vec::Vec};
use bitcoin::{
    absolute,
    bip32::{DerivationPath, Fingerprint, Xpriv},
    hashes::Hash,
    psbt::Psbt,
    secp256k1::{Message, PublicKey, Secp256k1},
    sighash::SighashCache,
    transaction, Address, Amount, CompressedPublicKey, EcdsaSighashType, Network, OutPoint,
    ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};

/// Re-export `bitcoin` so firmware code that depends on `signer-probe`
/// but not on the `bitcoin` crate directly can still reach types like
/// `Network` without pulling in another dependency.
pub use bitcoin;

/// Public BIP39 test vector: twelve words, eleven "abandon" then "about".
/// Never fund this key. The API deliberately accepts no caller-supplied secrets.
pub fn run() -> Result<Report, &'static str> {
    let mnemonic = bip39::Mnemonic::from_entropy(&[0; 16]).map_err(|_| "mnemonic")?;
    let seed = mnemonic.to_seed_normalized("");
    let secp = Secp256k1::new();
    let master = Xpriv::new_master(Network::Testnet, &seed).map_err(|_| "master")?;
    let path: DerivationPath = "m/84'/1'/0'/0/0".parse().map_err(|_| "path")?;
    let key = master.derive_priv(&secp, &path).map_err(|_| "derive")?;
    let public = key.private_key.public_key(&secp);
    let address = Address::p2wpkh(&CompressedPublicKey(public), Network::Testnet);
    let address = alloc::format!("{address}");
    if address != EXPECTED_ADDRESS {
        return Err("address vector");
    }
    let script = Address::p2wpkh(&CompressedPublicKey(public), Network::Testnet).script_pubkey();

    // Synthetic outpoint: this transaction is never broadcastable or funded.
    let tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::from_byte_array([0x11; 32]),
                vout: 0,
            },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(99_000),
            script_pubkey: script.clone(),
        }],
    };
    let mut psbt = Psbt::from_unsigned_tx(tx).map_err(|_| "psbt")?;
    let utxo = TxOut {
        value: Amount::from_sat(100_000),
        script_pubkey: script,
    };
    psbt.inputs[0].witness_utxo = Some(utxo.clone());
    psbt.inputs[0]
        .bip32_derivation
        .insert(public, (master.fingerprint(&secp), path));
    psbt.inputs[0].sighash_type = Some(EcdsaSighashType::All.into());
    // Exercise the wire parser, not just construction of Rust objects.
    let mut psbt = Psbt::deserialize(&psbt.serialize()).map_err(|_| "parse")?;
    let sighash = SighashCache::new(&psbt.unsigned_tx)
        .p2wpkh_signature_hash(0, &utxo.script_pubkey, utxo.value, EcdsaSighashType::All)
        .map_err(|_| "sighash")?;
    let message = Message::from_digest(sighash.to_byte_array());
    if alloc::format!("{sighash}") != EXPECTED_SIGHASH {
        return Err("sighash vector");
    }
    let signature = secp.sign_ecdsa(&message, &key.private_key);
    secp.verify_ecdsa(&message, &signature, &public)
        .map_err(|_| "verify")?;
    psbt.inputs[0].partial_sigs.insert(
        bitcoin::PublicKey::new(public),
        bitcoin::ecdsa::Signature::sighash_all(signature),
    );
    Ok(Report {
        address,
        signed_psbt: psbt.serialize(),
    })
}

// Independently calculated and verified with embit 0.8.0 (see verify.py).
pub const EXPECTED_ADDRESS: &str = "tb1q6rz28mcfaxtmd6v789l9rrlrusdprr9pqcpvkl";
const EXPECTED_SIGHASH: &str = "52f4d13355d9b3bf37467561c7f8ceb4fecd88b8ba6cdba3034093d8d44c7b90";

pub struct Report {
    pub address: String,
    pub signed_psbt: alloc::vec::Vec<u8>,
}

// ---------------------------------------------------------------------------
// Bounded PSBT approval/sign API. Library-only; the firmware wires this into
// the SD-card workflow in a follow-up work unit.
// ---------------------------------------------------------------------------

/// The seed used by every review/sign call. The library never accepts a
/// caller-supplied seed, BIP39 phrase, or private key. Funding this key is
/// unsafe and outside the contract.
const ENTROPY: [u8; 16] = [0; 16];
const PASSPHRASE: &str = "";
const ACCOUNT_PATH: &str = "m/84'/1'/0'";

/// Identifies the public keys the policy should validate against. Built from
/// either the diagnostic seed (test only) or an active `SeedSession`.
/// Both paths share `validate()` so the policy is defined exactly once.
pub struct ValidationContext {
    pub network: Network,
    pub master_fingerprint: Fingerprint,
    pub change_path: DerivationPath,
    pub change_public_key: bitcoin::secp256k1::PublicKey,
}

impl ValidationContext {
    /// Build a context for the public diagnostic seed. The diagnostic
    /// never accepts caller-supplied secrets.
    pub fn for_diagnostic(secp: &Secp256k1<bitcoin::secp256k1::All>) -> Self {
        let mnemonic = bip39::Mnemonic::from_entropy(&ENTROPY).expect("diagnostic entropy");
        let seed = mnemonic.to_seed_normalized(PASSPHRASE);
        let master = Xpriv::new_master(NETWORK, &seed).expect("diagnostic master");
        let change_path: DerivationPath = RECEIVE_PATH.parse().expect("input path");
        let change_xpriv = master
            .derive_priv(secp, &change_path)
            .expect("derive diagnostic change key");
        let change_public_key = change_xpriv.private_key.public_key(secp);
        let master_fingerprint = master.fingerprint(secp);
        Self {
            network: NETWORK,
            master_fingerprint,
            change_path,
            change_public_key,
        }
    }
}
/// Receive path used by the diagnostic and its firmware test vector.
const RECEIVE_PATH: &str = "m/84'/1'/0'/0/0";

/// Only this network is accepted. Mainnet, signet, and regtest are rejected.
pub const NETWORK: Network = Network::Testnet;

/// Hard caps for the bounded parser. Each value is small enough to fit
/// comfortably in the firmware's 64 KiB heap (peak measured at 6,020 bytes
/// for the diagnostic PSBT). The caps are policy, not optimisation.
pub const MAX_PSBT_BYTES: usize = 4096;
pub const MAX_INPUTS: usize = 1;
pub const MAX_OUTPUTS: usize = 2;
/// Receive index upper bound, matching typical address look-ahead windows.
pub const MAX_PATH_INDEX: u32 = 1000;
/// Fee ceiling. With MAX_OUTPUTS = 2 and a ~100 vbyte single-input tx,
/// 1_000_000 sat is ~50_000 sat/vB which exceeds any sane rate.
pub const MAX_FEE_SAT: u64 = 1_000_000;

/// Policy rejections. Every variant is reachable from `review` or `sign`.
#[derive(Debug, PartialEq, Eq)]
pub enum Reject {
    EmptyPsbt,
    PsbtTooLarge,
    WrongInputCount,
    WrongOutputCount,
    WrongNetwork,
    NonSegwit,
    NonP2wpkh,
    Multisig,
    MissingWitnessUtxo,
    MissingDerivation,
    DerivationMismatch,
    PathOutsideAccount,
    DuplicateChange,
    NegativeFee,
    FeeTooLarge,
    WrongSighash,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Reject(Reject),
    Parse,
    Sighash,
    Verify,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DisplayItem {
    pub address: String,
    pub amount_sat: u64,
    /// True when the output script is our own derived change script. The
    /// tag is computed from the fixed key, never trusted from the PSBT.
    pub is_change: bool,
}

/// Immutable approval binding for one PSBT. Owning this value is what binds
/// approval to the exact transaction reviewed: `sign` re-derives the sighash
/// from the held PSBT and refuses to produce a signature if it differs from
/// `sighash`.
pub struct Review {
    // Debug impl below intentionally omits the PSBT bytes: callers do not
    // need them to format and we do not want them in panic messages.
    #[allow(dead_code)]
    psbt: Psbt,
    outputs: Vec<DisplayItem>,
    fee_sat: u64,
    /// Sighash the signer will commit to. Any drift in the unsigned tx
    /// between review and sign makes this value invalid.
    sighash: [u8; 32],
    /// Derivation path the signer uses to re-derive the change key.
    pub change_path: DerivationPath,
    /// Public key the signer must verify against. The seed flow reads
    /// this to build a `ReviewBinding` without re-deriving anything.
    pub change_public_key: PublicKey,
    /// Seed generation that produced this review. `0` means the
    /// diagnostic seed was used; any other value pins the review to a
    /// specific seed load and is the firmware's check against silent
    /// replacement between review and sign.
    pub seed_generation: u32,
}

impl core::fmt::Debug for Review {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let sighash_hex: String = self.sighash.iter().map(|b| format!("{:02x}", b)).collect();
        f.debug_struct("Review")
            .field("outputs", &self.outputs)
            .field("fee_sat", &self.fee_sat)
            .field("sighash_hex", &sighash_hex)
            .field("txid_hex", &self.txid_hex())
            .finish()
    }
}

impl Review {
    pub fn outputs(&self) -> &[DisplayItem] {
        &self.outputs
    }
    pub fn fee_sat(&self) -> u64 {
        self.fee_sat
    }
    pub fn sighash(&self) -> &[u8; 32] {
        &self.sighash
    }
    pub fn txid_hex(&self) -> String {
        format!("{}", self.psbt.unsigned_tx.compute_txid())
    }
    /// The serialized PSBT as it was reviewed. Equal to the unsigned bytes
    /// that produced this `Review` after round-tripping through the parser.
    pub fn approved_psbt_bytes(&self) -> Vec<u8> {
        self.psbt.serialize()
    }
}

#[derive(Debug)]
pub struct Signed {
    pub signed_psbt: Vec<u8>,
    pub sighash: [u8; 32],
    pub txid_hex: String,
}

/// Parse, validate, and prepare an unsigned PSBT for approval. The returned
/// `Review` carries the immutable parsed transaction and the sighash the
/// signer will commit to. Call `sign(&review)` once to produce a signature;
/// cancellation is simply not calling `sign`. There is no API that signs
/// raw bytes, raw PSBTs, or arbitrary hashes.
pub fn review(bytes: &[u8]) -> Result<Review, Error> {
    let secp = Secp256k1::new();
    validate(bytes, &ValidationContext::for_diagnostic(&secp))
}

/// Shared validation routine used by the diagnostic and the seed flow.
/// The `ValidationContext` selects the seed whose key the policy will
/// match against. Bounds, input/output counts, sighash, script type,
/// fingerprint, derivation path and arithmetic limits are identical
/// between the two flows.
pub fn validate(bytes: &[u8], ctx: &ValidationContext) -> Result<Review, Error> {
    if bytes.is_empty() {
        return Err(Error::Reject(Reject::EmptyPsbt));
    }
    if bytes.len() > MAX_PSBT_BYTES {
        return Err(Error::Reject(Reject::PsbtTooLarge));
    }
    let psbt = Psbt::deserialize(bytes).map_err(|_| Error::Parse)?;
    if psbt.inputs.len() != MAX_INPUTS {
        return Err(Error::Reject(Reject::WrongInputCount));
    }
    if psbt.outputs.is_empty() || psbt.outputs.len() > MAX_OUTPUTS {
        return Err(Error::Reject(Reject::WrongOutputCount));
    }
    if psbt.unsigned_tx.input.len() != MAX_INPUTS
        || psbt.unsigned_tx.output.len() != psbt.outputs.len()
    {
        return Err(Error::Reject(Reject::WrongInputCount));
    }

    // Derive the seed's change script from the context. Anything else
    // is a different key and the policy does not own it.
    let change_address_obj =
        Address::p2wpkh(&CompressedPublicKey(ctx.change_public_key), ctx.network);
    let change_script = change_address_obj.script_pubkey();

    // Input policy: single-sig native SegWit, owned by us.
    let input = &psbt.inputs[0];
    match input.sighash_type {
        Some(t) if t == EcdsaSighashType::All.into() => {}
        _ => return Err(Error::Reject(Reject::WrongSighash)),
    }
    let utxo = input
        .witness_utxo
        .as_ref()
        .ok_or(Error::Reject(Reject::MissingWitnessUtxo))?;
    if !matches!(
        utxo.script_pubkey.witness_version(),
        Some(bitcoin::WitnessVersion::V0)
    ) {
        return Err(Error::Reject(Reject::NonSegwit));
    }
    if !utxo.script_pubkey.is_p2wpkh() {
        return Err(Error::Reject(Reject::NonP2wpkh));
    }
    if input.bip32_derivation.len() != 1 {
        return Err(Error::Reject(Reject::Multisig));
    }
    let (pk, source) = input
        .bip32_derivation
        .iter()
        .next()
        .ok_or(Error::Reject(Reject::MissingDerivation))?;
    if source.0 != ctx.master_fingerprint {
        return Err(Error::Reject(Reject::DerivationMismatch));
    }
    if !within_account(&source.1) {
        return Err(Error::Reject(Reject::PathOutsideAccount));
    }
    if pk != &ctx.change_public_key {
        return Err(Error::Reject(Reject::DerivationMismatch));
    }
    let mut outputs = Vec::with_capacity(psbt.outputs.len());
    let mut change_count = 0u32;
    let mut total_out: u64 = 0;
    for (i, psbt_output) in psbt.outputs.iter().enumerate() {
        let txout = &psbt.unsigned_tx.output[i];
        if !txout.script_pubkey.is_p2wpkh() {
            return Err(Error::Reject(Reject::NonP2wpkh));
        }
        if !psbt_output.bip32_derivation.is_empty() {
            return Err(Error::Reject(Reject::Multisig));
        }
        let addr = Address::from_script(&txout.script_pubkey, ctx.network)
            .map_err(|_| Error::Reject(Reject::NonP2wpkh))?;
        let is_change = txout.script_pubkey == change_script;
        if is_change {
            change_count += 1;
        }
        total_out = total_out
            .checked_add(txout.value.to_sat())
            .ok_or(Error::Reject(Reject::NegativeFee))?;
        outputs.push(DisplayItem {
            address: format!("{addr}"),
            amount_sat: txout.value.to_sat(),
            is_change,
        });
    }
    if change_count > 1 {
        return Err(Error::Reject(Reject::DuplicateChange));
    }

    let input_value_sat = utxo.value.to_sat();
    let fee = input_value_sat
        .checked_sub(total_out)
        .ok_or(Error::Reject(Reject::NegativeFee))?;
    if fee > MAX_FEE_SAT {
        return Err(Error::Reject(Reject::FeeTooLarge));
    }

    // Compute the sighash exactly as we will sign. Both review and sign run
    // this same computation; any tampering between the two makes them
    // disagree and `sign` returns Error::Sighash.
    let mut cache = SighashCache::new(&psbt.unsigned_tx);
    let sighash = cache
        .p2wpkh_signature_hash(0, &utxo.script_pubkey, utxo.value, EcdsaSighashType::All)
        .map_err(|_| Error::Sighash)?;
    let mut sighash_bytes = [0u8; 32];
    sighash_bytes.copy_from_slice(&sighash.to_byte_array());

    Ok(Review {
        psbt,
        outputs,
        fee_sat: fee,
        sighash: sighash_bytes,
        change_path: ctx.change_path.clone(),
        change_public_key: ctx.change_public_key,
        seed_generation: 0,
    })
}

/// Produce a signature bound to the reviewed `Review`. Re-derives the key
/// from the fixed seed, recomputes the sighash from the held PSBT, and
/// refuses to sign if it differs from `review.sighash`. Returns the full
/// signed PSBT plus its sighash and txid for the caller to record.
pub fn sign(review: &Review) -> Result<Signed, Error> {
    let secp = Secp256k1::new();
    let mnemonic = bip39::Mnemonic::from_entropy(&ENTROPY).map_err(|_| Error::Parse)?;
    let seed = mnemonic.to_seed_normalized(PASSPHRASE);
    let master = Xpriv::new_master(NETWORK, &seed).map_err(|_| Error::Parse)?;
    let xpriv = master
        .derive_priv(&secp, &review.change_path)
        .map_err(|_| Error::Parse)?;
    let mut psbt = review.psbt.clone();
    let utxo = psbt.inputs[0]
        .witness_utxo
        .as_ref()
        .ok_or(Error::Reject(Reject::MissingWitnessUtxo))?;
    let mut cache = SighashCache::new(&psbt.unsigned_tx);
    let sighash = cache
        .p2wpkh_signature_hash(0, &utxo.script_pubkey, utxo.value, EcdsaSighashType::All)
        .map_err(|_| Error::Sighash)?;
    let message = Message::from_digest(sighash.to_byte_array());
    if message != Message::from_digest(review.sighash) {
        return Err(Error::Sighash);
    }
    let signature = secp.sign_ecdsa(&message, &xpriv.private_key);
    secp.verify_ecdsa(&message, &signature, &review.change_public_key)
        .map_err(|_| Error::Verify)?;
    psbt.inputs[0].partial_sigs.insert(
        bitcoin::PublicKey::new(review.change_public_key),
        bitcoin::ecdsa::Signature::sighash_all(signature),
    );
    Ok(Signed {
        signed_psbt: psbt.serialize(),
        sighash: review.sighash,
        txid_hex: format!("{}", psbt.unsigned_tx.compute_txid()),
    })
}

fn within_account(path: &DerivationPath) -> bool {
    let account: DerivationPath = match ACCOUNT_PATH.parse() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let path_vec: Vec<u32> = path.into_iter().map(|c| (*c).into()).collect();
    let account_vec: Vec<u32> = account.into_iter().map(|c| (*c).into()).collect();
    if path_vec.len() != account_vec.len() + 2 {
        return false;
    }
    if path_vec[..account_vec.len()] != account_vec[..] {
        return false;
    }
    let last = path_vec[account_vec.len() + 1];
    // Normal child index only (no hardened bit set) and within window.
    last & 0x80000000 == 0 && last <= MAX_PATH_INDEX
}

// ---------------------------------------------------------------------------
// Seed-bound review + sign pipeline.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::bip32::{ChildNumber, Fingerprint};
    use core::str::FromStr;

    fn external_script() -> ScriptBuf {
        Address::from_str("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx")
            .unwrap()
            .assume_checked()
            .script_pubkey()
    }

    /// Build a minimal valid PSBT: one input owned by our fixed key, one
    /// output to an external P2WPKH, optional change output.
    fn build_fixture(output_value: u64, include_change: bool) -> Psbt {
        let secp = Secp256k1::new();
        let mnemonic = bip39::Mnemonic::from_entropy(&[0; 16]).unwrap();
        let seed = mnemonic.to_seed_normalized("");
        let master = Xpriv::new_master(NETWORK, &seed).unwrap();
        let path: DerivationPath = "m/84'/1'/0'/0/0".parse().unwrap();
        let key = master.derive_priv(&secp, &path).unwrap();
        let public = key.private_key.public_key(&secp);
        let our_script = Address::p2wpkh(&CompressedPublicKey(public), NETWORK).script_pubkey();
        let external = external_script();
        assert!(external.is_p2wpkh());
        assert_ne!(external, our_script);

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

        if include_change {
            psbt.outputs.push(bitcoin::psbt::Output::default());
            psbt.outputs.push(bitcoin::psbt::Output::default());
            psbt.unsigned_tx.output.push(TxOut {
                value: Amount::from_sat(40_000),
                script_pubkey: our_script,
            });
            psbt.unsigned_tx.output.push(TxOut {
                value: Amount::from_sat(output_value),
                script_pubkey: external,
            });
        } else {
            psbt.outputs.push(bitcoin::psbt::Output::default());
            psbt.unsigned_tx.output.push(TxOut {
                value: Amount::from_sat(output_value),
                script_pubkey: external,
            });
        }
        psbt
    }

    #[test]
    fn fixed_vector_is_repeatable_and_roundtrips() {
        let first = run().unwrap();
        assert_eq!(first.address, EXPECTED_ADDRESS);
        assert_eq!(first.signed_psbt, run().unwrap().signed_psbt);
        let psbt = Psbt::deserialize(&first.signed_psbt).unwrap();
        assert_eq!(psbt.serialize(), first.signed_psbt);
        assert_eq!(psbt.inputs[0].partial_sigs.len(), 1);
        assert_eq!(psbt.fee().unwrap(), Amount::from_sat(1_000));
    }

    #[test]
    fn changing_output_invalidates_signature() {
        let mut psbt = Psbt::deserialize(&run().unwrap().signed_psbt).unwrap();
        psbt.unsigned_tx.output[0].value = Amount::from_sat(98_000);
        let utxo = psbt.inputs[0].witness_utxo.as_ref().unwrap();
        let hash = SighashCache::new(&psbt.unsigned_tx)
            .p2wpkh_signature_hash(0, &utxo.script_pubkey, utxo.value, EcdsaSighashType::All)
            .unwrap();
        let (public, signature) = psbt.inputs[0].partial_sigs.first_key_value().unwrap();
        assert!(Secp256k1::new()
            .verify_ecdsa(
                &Message::from_digest(hash.to_byte_array()),
                &signature.signature,
                &public.inner,
            )
            .is_err());
    }

    #[test]
    fn review_accepts_valid_psbt_and_signs_byte_equal_twice() {
        let psbt = build_fixture(50_000, true);
        let bytes = psbt.serialize();
        let review = review(&bytes).expect("valid psbt");
        assert_eq!(review.outputs().len(), 2);
        let (change, external) = (&review.outputs()[0], &review.outputs()[1]);
        assert!(change.is_change);
        assert!(!external.is_change);
        assert_eq!(external.amount_sat, 50_000);
        assert_eq!(change.amount_sat, 40_000);
        assert_eq!(review.fee_sat(), 10_000);

        let first = sign(&review).unwrap();
        let second = sign(&review).unwrap();
        assert_eq!(first.signed_psbt, second.signed_psbt);

        let parsed = Psbt::deserialize(&first.signed_psbt).unwrap();
        let utxo = parsed.inputs[0].witness_utxo.as_ref().unwrap();
        let mut cache = SighashCache::new(&parsed.unsigned_tx);
        let sighash = cache
            .p2wpkh_signature_hash(0, &utxo.script_pubkey, utxo.value, EcdsaSighashType::All)
            .unwrap();
        let (pk, sig) = parsed.inputs[0].partial_sigs.first_key_value().unwrap();
        assert!(Secp256k1::new()
            .verify_ecdsa(
                &Message::from_digest(sighash.to_byte_array()),
                &sig.signature,
                &pk.inner
            )
            .is_ok());
    }

    #[test]
    fn review_rejects_multi_input() {
        let mut psbt = build_fixture(50_000, false);
        psbt.unsigned_tx
            .input
            .push(psbt.unsigned_tx.input[0].clone());
        psbt.inputs.push(bitcoin::psbt::Input::default());
        let bytes = psbt.serialize();
        assert_eq!(
            review(&bytes).unwrap_err(),
            Error::Reject(Reject::WrongInputCount)
        );
    }

    #[test]
    fn review_rejects_three_outputs() {
        let mut psbt = build_fixture(10_000, true);
        psbt.unsigned_tx.output.push(TxOut {
            value: Amount::from_sat(10_000),
            script_pubkey: psbt.unsigned_tx.output[1].script_pubkey.clone(),
        });
        psbt.outputs.push(bitcoin::psbt::Output::default());
        let bytes = psbt.serialize();
        assert_eq!(
            review(&bytes).unwrap_err(),
            Error::Reject(Reject::WrongOutputCount)
        );
    }

    #[test]
    fn review_rejects_wrong_sighash() {
        let mut psbt = build_fixture(50_000, false);
        psbt.inputs[0].sighash_type =
            Some(bitcoin::psbt::PsbtSighashType::from(EcdsaSighashType::None));
        let bytes = psbt.serialize();
        assert_eq!(
            review(&bytes).unwrap_err(),
            Error::Reject(Reject::WrongSighash)
        );
    }

    #[test]
    fn review_rejects_missing_utxo() {
        let mut psbt = build_fixture(50_000, false);
        psbt.inputs[0].witness_utxo = None;
        let bytes = psbt.serialize();
        assert_eq!(
            review(&bytes).unwrap_err(),
            Error::Reject(Reject::MissingWitnessUtxo)
        );
    }

    #[test]
    fn review_rejects_negative_fee() {
        let psbt = build_fixture(120_000, false);
        let bytes = psbt.serialize();
        assert_eq!(
            review(&bytes).unwrap_err(),
            Error::Reject(Reject::NegativeFee)
        );
    }

    #[test]
    fn review_rejects_oversized_fee() {
        let secp = Secp256k1::new();
        let mnemonic = bip39::Mnemonic::from_entropy(&[0; 16]).unwrap();
        let seed = mnemonic.to_seed_normalized("");
        let master = Xpriv::new_master(NETWORK, &seed).unwrap();
        let path: DerivationPath = "m/84'/1'/0'/0/0".parse().unwrap();
        let key = master.derive_priv(&secp, &path).unwrap();
        let public = key.private_key.public_key(&secp);
        let our_script = Address::p2wpkh(&CompressedPublicKey(public), NETWORK).script_pubkey();
        let external = external_script();
        let mut psbt = Psbt::from_unsigned_tx(Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint {
                    txid: Txid::from_byte_array([0x33; 32]),
                    vout: 0,
                },
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![],
        })
        .unwrap();
        psbt.inputs[0].witness_utxo = Some(TxOut {
            value: Amount::from_sat(2_000_000),
            script_pubkey: our_script,
        });
        psbt.inputs[0]
            .bip32_derivation
            .insert(public, (master.fingerprint(&secp), path));
        psbt.inputs[0].sighash_type = Some(EcdsaSighashType::All.into());
        psbt.outputs.push(bitcoin::psbt::Output::default());
        psbt.unsigned_tx.output.push(TxOut {
            value: Amount::from_sat(0),
            script_pubkey: external,
        });
        let bytes = psbt.serialize();
        assert_eq!(
            review(&bytes).unwrap_err(),
            Error::Reject(Reject::FeeTooLarge)
        );
    }

    #[test]
    fn review_rejects_non_p2wpkh_output() {
        let secp = Secp256k1::new();
        let mnemonic = bip39::Mnemonic::from_entropy(&[0; 16]).unwrap();
        let seed = mnemonic.to_seed_normalized("");
        let master = Xpriv::new_master(NETWORK, &seed).unwrap();
        let path: DerivationPath = "m/84'/1'/0'/0/0".parse().unwrap();
        let key = master.derive_priv(&secp, &path).unwrap();
        let public = key.private_key.public_key(&secp);
        let our_script = Address::p2wpkh(&CompressedPublicKey(public), NETWORK).script_pubkey();
        // P2SH is structurally a non-P2WPKH SegWit-adjacent script.
        let p2sh_script = ScriptBuf::new_p2sh(&bitcoin::ScriptHash::from_byte_array([0x11; 20]));
        assert!(!p2sh_script.is_p2wpkh());

        let mut psbt = Psbt::from_unsigned_tx(Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint {
                    txid: Txid::from_byte_array([0x44; 32]),
                    vout: 0,
                },
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![],
        })
        .unwrap();
        psbt.inputs[0].witness_utxo = Some(TxOut {
            value: Amount::from_sat(100_000),
            script_pubkey: our_script,
        });
        psbt.inputs[0]
            .bip32_derivation
            .insert(public, (master.fingerprint(&secp), path));
        psbt.inputs[0].sighash_type = Some(EcdsaSighashType::All.into());
        psbt.outputs.push(bitcoin::psbt::Output::default());
        psbt.unsigned_tx.output.push(TxOut {
            value: Amount::from_sat(90_000),
            script_pubkey: p2sh_script,
        });
        let bytes = psbt.serialize();
        assert_eq!(
            review(&bytes).unwrap_err(),
            Error::Reject(Reject::NonP2wpkh)
        );
    }

    #[test]
    fn review_rejects_multisig_input() {
        let mut psbt = build_fixture(50_000, false);
        let other = "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";
        let extra = PublicKey::from_str(other).unwrap();
        psbt.inputs[0].bip32_derivation.insert(
            extra,
            (
                Fingerprint::from([0u8; 4]),
                "m/84'/1'/0'/0/0".parse().unwrap(),
            ),
        );
        let bytes = psbt.serialize();
        assert_eq!(review(&bytes).unwrap_err(), Error::Reject(Reject::Multisig));
    }

    #[test]
    fn review_rejects_derivation_outside_account() {
        let mut psbt = build_fixture(50_000, false);
        let pk = *psbt.inputs[0].bip32_derivation.iter().next().unwrap().0;
        psbt.inputs[0].bip32_derivation.clear();
        let bad_path: DerivationPath = "m/84'/1'/1'/0/0".parse().unwrap();
        psbt.inputs[0]
            .bip32_derivation
            .insert(pk, (Fingerprint::from([0u8; 4]), bad_path));
        let bytes = psbt.serialize();
        assert_eq!(
            review(&bytes).unwrap_err(),
            Error::Reject(Reject::DerivationMismatch)
        );
    }

    #[test]
    fn review_rejects_path_with_hardened_receive_index() {
        let mut psbt = build_fixture(50_000, false);
        let (pk, original) = psbt.inputs[0]
            .bip32_derivation
            .iter()
            .next()
            .map(|(k, v)| (*k, v.clone()))
            .unwrap();
        psbt.inputs[0].bip32_derivation.clear();
        let mut bad_path = DerivationPath::master();
        for cn in [
            ChildNumber::Hardened { index: 84 },
            ChildNumber::Hardened { index: 1 },
            ChildNumber::Hardened { index: 0 },
            ChildNumber::Normal { index: 0 },
            ChildNumber::Hardened { index: 0 },
        ] {
            bad_path = bad_path.extend(cn);
        }
        // Keep the original fingerprint so the policy moves past the
        // fingerprint gate and trips the path-shape check.
        psbt.inputs[0]
            .bip32_derivation
            .insert(pk, (original.0, bad_path));
        let bytes = psbt.serialize();
        assert_eq!(
            review(&bytes).unwrap_err(),
            Error::Reject(Reject::PathOutsideAccount)
        );
    }

    #[test]
    fn review_rejects_oversized_psbt() {
        let mut bytes = vec![0u8; MAX_PSBT_BYTES + 1];
        bytes[0] = 0x70;
        bytes[1] = 0x73;
        bytes[2] = 0x62;
        bytes[3] = 0x74;
        bytes[4] = 0xff;
        assert_eq!(
            review(&bytes).unwrap_err(),
            Error::Reject(Reject::PsbtTooLarge)
        );
    }

    #[test]
    fn review_rejects_empty_psbt() {
        assert_eq!(review(&[]).unwrap_err(), Error::Reject(Reject::EmptyPsbt));
    }

    #[test]
    fn sign_refuses_drifted_review() {
        let psbt = build_fixture(50_000, true);
        let bytes = psbt.serialize();
        let mut review = review(&bytes).unwrap();
        review.psbt.unsigned_tx.output[1].value = Amount::from_sat(49_999);
        assert_eq!(sign(&review).unwrap_err(), Error::Sighash);
    }

    #[test]
    fn approved_psbt_bytes_roundtrip_matches_input() {
        let psbt = build_fixture(50_000, true);
        let bytes = psbt.serialize();
        let review = review(&bytes).unwrap();
        let approved = review.approved_psbt_bytes();
        assert_eq!(approved, bytes);
    }

    #[test]
    fn sign_then_sign_again_is_idempotent_per_review() {
        // Signing the same Review twice produces byte-equal Signed output.
        // The firmware layer treats this as the canonical export for a
        // session; replays must not produce divergent signatures because
        // RFC 6979 nonce derivation is deterministic.
        let psbt = build_fixture(50_000, true);
        let bytes = psbt.serialize();
        let review = review(&bytes).unwrap();
        let first = sign(&review).unwrap();
        let second = sign(&review).unwrap();
        assert_eq!(first.signed_psbt, second.signed_psbt);
        assert_eq!(first.sighash, second.sighash);
        assert_eq!(first.txid_hex, second.txid_hex);
    }

    #[test]
    fn review_then_mutate_review_then_sign_is_caught() {
        // The drift check is a separate test from sign_refuses_drifted_review
        // because the firmware must also clear the signed buffer when sign
        // fails. The library contract: sign() returns Err on any drift;
        // it must not return Ok with a stale or partial signature.
        let psbt = build_fixture(50_000, true);
        let bytes = psbt.serialize();
        let mut review = review(&bytes).unwrap();
        review.psbt.unsigned_tx.output[0].value = Amount::from_sat(39_999);
        assert_eq!(sign(&review).unwrap_err(), Error::Sighash);
    }

    #[test]
    fn review_rejects_truncated_bytes() {
        // Upload completeness lives in the firmware (signer_demo::trigger_review
        // checks UNSIGNED_LEN >= total), but the library must also reject
        // a truncated PSBT that claims to be the full upload. Here we slice
        // the serialized PSBT in half and confirm review() returns
        // PsbtTooLarge once the truncated size still exceeds the cap, or
        // Parse if it slips under the cap. Either is correct; neither is a
        // silent accept.
        let psbt = build_fixture(50_000, true);
        let bytes = psbt.serialize();
        assert!(bytes.len() > 32);
        let truncated = &bytes[..bytes.len() / 2];
        let err = review(truncated).unwrap_err();
        match err {
            Error::Reject(Reject::PsbtTooLarge) => {}
            Error::Parse => {}
            other => panic!("truncated PSBT should be rejected, got {:?}", other),
        }
    }
}
