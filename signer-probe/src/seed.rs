//! Experimental stateless recovery, using disposable test seeds only.
//!
//! The Crystal terminal uses this module for recovery and reviewed SD PSBT signing.
//! Owned seed and mnemonic buffers are zeroized. Dependency temporaries,
//! caller input, compiler copies and SRAM after reset are not covered by this
//! guarantee; hardware memory-lifetime qualification remains outstanding.

use alloc::{format, string::String, vec::Vec};
use bitcoin::{
    bip32::{ChildNumber, DerivationPath, Fingerprint, Xpriv, Xpub},
    hashes::Hash,
    psbt::Psbt,
    secp256k1::{All, Message, Secp256k1},
    sighash::SighashCache,
    Address, EcdsaSighashType, Network,
};
use zeroize::{Zeroize, Zeroizing};

pub const MAX_MNEMONIC_BYTES: usize = 24 * 9;
pub const MAX_PASSPHRASE_BYTES: usize = 100;
pub const MAX_ADDRESS_INDEX: u32 = 1000;
pub const EXPORT_PRIVACY_NOTICE: &str =
    "Account export reveals this account's addresses and transaction history.";

/// Errors expose positions, never mnemonic words or passphrase contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoverError {
    MnemonicTooLong,
    WordCount,
    UnknownWord(usize),
    Checksum,
    PassphraseTooLong,
    UnsupportedCharacter,
    UnsupportedNetwork,
    InvalidPath,
    Locked,
}

/// Explicitly owned seed. No Debug/Clone or seed-export API.
/// Lock erases the designated buffer in place and is idempotent.
pub struct SeedSession {
    seed: Zeroizing<[u8; 64]>,
    network: Network,
    locked: bool,
}

/// Reasons the seed signing path can refuse an operation. None of these
/// echo the seed, the PSBT, or any derivation intermediate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignError {
    /// Seed is locked or has never been loaded.
    Locked,
    /// PSBT could not be re-parsed. Should never happen if the firmware
    /// only stores validated reviews.
    Parse,
    /// Derivation path was rejected by the BIP32 stack.
    InvalidPath,
    /// PSBT is missing its `witness_utxo`.
    MissingWitnessUtxo,
    /// Recomputed sighash differs from the stored review. The PSBT was
    /// mutated after review, or a wrong PSBT was bound to this review.
    Sighash,
    /// secp256k1 refused to verify the produced signature.
    Verify,
}

/// A reviewed PSBT bound to one seed generation. The firmware stores
/// this with the bytes it intends to sign so a later approval can detect
/// that the seed has been replaced or locked.
#[derive(Debug, Clone)]
pub struct ReviewBinding {
    /// Serialised unsigned PSBT. Required by `sign_psbt` so it can recompute
    /// the sighash and refuse to sign a drifted transaction.
    pub psbt_bytes: Zeroizing<Vec<u8>>,
    /// Sighash the signer will commit to.
    pub sighash: [u8; 32],
    /// Derived change public key. Stored so the signature can be verified
    /// against the same key the user reviewed.
    pub change_public_key: bitcoin::secp256k1::PublicKey,
    /// Derivation path to the change key (e.g., `m/84'/1'/0'/0/0`).
    pub change_path: DerivationPath,
}

/// A signed PSBT bound to the seed generation that produced it. The
/// firmware must call `assert_current` before exporting the bytes so a
/// silent seed replacement cannot ship a stale signature.
#[derive(Debug, Clone)]
pub struct SignedBinding {
    pub signed_psbt: Vec<u8>,
    /// Generation of the seed at the moment of signing.
    pub generation: u32,
    /// Hex txid for the coordinator.
    pub txid_hex: String,
}

/// Erase accessible fields on every exit, without invalidating Rust enum values.
/// secp256k1 explicitly does not guarantee removal of compiler/library copies.
struct PrivateNode(Xpriv);
impl Drop for PrivateNode {
    fn drop(&mut self) {
        self.0.private_key.non_secure_erase();
        let chain: &mut [u8; 32] = self.0.chain_code.as_mut();
        chain.zeroize();
    }
}

impl SeedSession {
    /// English, lowercase words; ASCII whitespace separates words.
    /// Passphrases accept printable ASCII only, whose NFKD is unchanged.
    /// Spaces in passphrases are significant and are never trimmed.
    pub fn recover(
        mnemonic: &str,
        passphrase: &str,
        network: Network,
    ) -> Result<Self, RecoverError> {
        if !matches!(
            network,
            Network::Bitcoin | Network::Testnet | Network::Signet | Network::Regtest
        ) {
            return Err(RecoverError::UnsupportedNetwork);
        }
        if mnemonic.len() > MAX_MNEMONIC_BYTES {
            return Err(RecoverError::MnemonicTooLong);
        }
        if !mnemonic
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_whitespace())
        {
            return Err(RecoverError::UnsupportedCharacter);
        }
        if !matches!(mnemonic.split_ascii_whitespace().count(), 12 | 24) {
            return Err(RecoverError::WordCount);
        }
        if passphrase.len() > MAX_PASSPHRASE_BYTES {
            return Err(RecoverError::PassphraseTooLong);
        }
        if !passphrase.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
            return Err(RecoverError::UnsupportedCharacter);
        }
        let mnemonic = bip39::Mnemonic::parse_in_normalized(bip39::Language::English, mnemonic)
            .map_err(|e| match e {
                bip39::Error::UnknownWord(i) => RecoverError::UnknownWord(i),
                bip39::Error::BadWordCount(_) => RecoverError::WordCount,
                _ => RecoverError::Checksum,
            })?;
        Ok(Self {
            seed: Zeroizing::new(mnemonic.to_seed_normalized(passphrase)),
            network,
            locked: false,
        })
    }

    pub fn lock(&mut self) {
        self.seed.zeroize();
        self.locked = true;
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    fn master(&self) -> Result<PrivateNode, RecoverError> {
        if self.locked {
            return Err(RecoverError::Locked);
        }
        Xpriv::new_master(self.network, self.seed.as_ref())
            .map(PrivateNode)
            .map_err(|_| RecoverError::InvalidPath)
    }

    pub fn fingerprint(&self, secp: &Secp256k1<All>) -> Result<Fingerprint, RecoverError> {
        Ok(self.master()?.0.fingerprint(secp))
    }

    pub fn coin_type(&self) -> u32 {
        if self.network == Network::Bitcoin {
            0
        } else {
            1
        }
    }

    /// Native SegWit account zero, with the selected network's coin type.
    pub fn account_export(
        &self,
        secp: &Secp256k1<All>,
        segwit: bool,
    ) -> Result<String, RecoverError> {
        let key = self.account_xpub(secp)?;
        if !segwit {
            return Ok(format!("{key}"));
        }
        let mut bytes = key.encode();
        let version: u32 = if self.network == Network::Bitcoin {
            0x04b24746
        } else {
            0x045f1cf6
        };
        bytes[..4].copy_from_slice(&version.to_be_bytes());
        Ok(bitcoin::base58::encode_check(&bytes))
    }

    /// Only Native SegWit account zero is supported.
    pub fn account_xpub(&self, secp: &Secp256k1<All>) -> Result<Xpub, RecoverError> {
        let mut node = self.master()?;
        // One derivation step per owned node bounds accessible private temporaries.
        for index in [84, self.coin_type(), 0] {
            let path = DerivationPath::from(alloc::vec![
                ChildNumber::from_hardened_idx(index).map_err(|_| RecoverError::InvalidPath)?
            ]);
            node = PrivateNode(
                node.0
                    .derive_priv(secp, &path)
                    .map_err(|_| RecoverError::InvalidPath)?,
            );
        }
        Ok(Xpub::from_priv(secp, &node.0))
    }

    /// Public key at the receive index 0 path used by the current
    /// transaction policy. Returns the same value every call; the
    /// derivation uses the selected network's BIP44 coin type.
    pub fn change_public_key(
        &self,
        secp: &Secp256k1<All>,
    ) -> Result<bitcoin::secp256k1::PublicKey, RecoverError> {
        let mut node = self.master()?;
        // First three indices are hardened (purpose / coin / account).
        for index in [84u32, self.coin_type(), 0] {
            let hardened =
                ChildNumber::from_hardened_idx(index).map_err(|_| RecoverError::InvalidPath)?;
            let path = DerivationPath::from(alloc::vec![hardened]);
            node = PrivateNode(
                node.0
                    .derive_priv(secp, &path)
                    .map_err(|_| RecoverError::InvalidPath)?,
            );
        }
        // Branch 0 (receive) and index 0 are normal.
        for index in [0u32, 0] {
            let normal =
                ChildNumber::from_normal_idx(index).map_err(|_| RecoverError::InvalidPath)?;
            let path = DerivationPath::from(alloc::vec![normal]);
            node = PrivateNode(
                node.0
                    .derive_priv(secp, &path)
                    .map_err(|_| RecoverError::InvalidPath)?,
            );
        }
        Ok(node.0.private_key.public_key(secp))
    }

    /// Derive only a bounded BIP84 account-zero child of this session.
    fn public_at(
        &self,
        secp: &Secp256k1<All>,
        path: &DerivationPath,
    ) -> Result<bitcoin::secp256k1::PublicKey, &'static str> {
        let v: Vec<u32> = path.into_iter().map(|c| (*c).into()).collect();
        if v.len() != 5
            || v[..3] != [0x80000054, 0x80000000 | self.coin_type(), 0x80000000]
            || v[3] > 1
            || v[4] > MAX_ADDRESS_INDEX
        {
            return Err("Wrong account/path");
        }
        let mut node = self.master().map_err(|_| "Seed locked")?;
        for child in path.into_iter() {
            node = PrivateNode(
                node.0
                    .derive_priv(secp, &DerivationPath::from(alloc::vec![*child]))
                    .map_err(|_| "Key derivation failed")?,
            );
        }
        Ok(node.0.private_key.public_key(secp))
    }

    /// Bounded Crystal seed policy. The held PSBT is never reread after review.
    pub fn review_psbt(&self, bytes: &[u8]) -> Result<crate::Review, &'static str> {
        if bytes.is_empty() || bytes.len() > crate::MAX_PSBT_BYTES {
            return Err("PSBT size: 1-4096");
        }
        let psbt = Psbt::deserialize(bytes).map_err(|_| "Invalid binary PSBT")?;
        if psbt.version != 0 {
            return Err("Only PSBT version 0");
        }
        if psbt.inputs.len() != 1 || psbt.unsigned_tx.input.len() != 1 {
            return Err("Need exactly 1 input");
        }
        if psbt.outputs.is_empty()
            || psbt.outputs.len() > 2
            || psbt.outputs.len() != psbt.unsigned_tx.output.len()
        {
            return Err("Need 1 or 2 outputs");
        }
        let secp = Secp256k1::new();
        let fingerprint = self.fingerprint(&secp).map_err(|_| "Seed locked")?;
        let input = &psbt.inputs[0];
        if input
            .sighash_type
            .is_some_and(|t| t != EcdsaSighashType::All.into())
        {
            return Err("Only SIGHASH_ALL");
        }
        if input.final_script_sig.is_some()
            || input.final_script_witness.is_some()
            || !input.partial_sigs.is_empty()
        {
            return Err("PSBT already signed");
        }
        if input.bip32_derivation.len() != 1 {
            return Err("Need input key origin");
        }
        let (public, (fp, path)) = input.bip32_derivation.iter().next().unwrap();
        // A bare account xpub does not carry the master fingerprint or full
        // origin. Diagnose coordinator metadata separately from key ownership.
        let indices = path.as_ref();
        if indices.len() == 5 && u32::from(indices[0]) == 0x80000054 {
            let coin = u32::from(indices[1]);
            if matches!(coin, 0x80000000 | 0x80000001)
                && coin != (0x80000000 | self.coin_type())
            {
                return Err("Network mismatch    Check main/testnet");
            }
        }
        if *fp != fingerprint {
            return Err("Fingerprint mismatchCheck Sparrow origin");
        }
        if self.public_at(&secp, path)? != *public {
            return Err("Input key mismatch  Check seed + pass");
        }
        let utxo = input.witness_utxo.as_ref().ok_or("Missing witness UTXO")?;
        if utxo.value.to_sat() > 2_100_000_000_000_000 {
            return Err("Invalid input amount");
        }
        let owned =
            Address::p2wpkh(&bitcoin::CompressedPublicKey(*public), self.network).script_pubkey();
        if utxo.script_pubkey != owned {
            return Err("Input script mismatch");
        }
        if let Some(prev) = &input.non_witness_utxo {
            let outpoint = psbt.unsigned_tx.input[0].previous_output;
            if prev.compute_txid() != outpoint.txid
                || prev.output.get(outpoint.vout as usize) != Some(utxo)
            {
                return Err("Previous TX mismatch");
            }
        }
        let mut outputs = Vec::new();
        let mut total = 0u64;
        for (i, meta) in psbt.outputs.iter().enumerate() {
            let out = &psbt.unsigned_tx.output[i];
            if out.value.to_sat() > 2_100_000_000_000_000 {
                return Err("Invalid output amount");
            }
            if !out.script_pubkey.is_p2wpkh() {
                return Err("Only Native SegWit");
            }
            if meta.bip32_derivation.len() > 1 {
                return Err("No multisig support");
            }
            let mut is_change = false;
            for (key, (fp, path)) in &meta.bip32_derivation {
                if *fp == fingerprint {
                    if self.public_at(&secp, path)? != *key
                        || Address::p2wpkh(&bitcoin::CompressedPublicKey(*key), self.network)
                            .script_pubkey()
                            != out.script_pubkey
                    {
                        return Err("Change key mismatch");
                    }
                    is_change = u32::from(path.as_ref()[3]) == 1;
                }
            }
            total = total
                .checked_add(out.value.to_sat())
                .ok_or("Invalid amounts")?;
            outputs.push(crate::DisplayItem {
                address: format!(
                    "{}",
                    Address::from_script(&out.script_pubkey, self.network)
                        .map_err(|_| "Invalid address")?
                ),
                amount_sat: out.value.to_sat(),
                is_change,
            });
        }
        let fee = utxo
            .value
            .to_sat()
            .checked_sub(total)
            .ok_or("Negative fee")?;
        if fee > crate::MAX_FEE_SAT {
            return Err("Fee exceeds limit");
        }
        let sighash = SighashCache::new(&psbt.unsigned_tx)
            .p2wpkh_signature_hash(0, &utxo.script_pubkey, utxo.value, EcdsaSighashType::All)
            .map_err(|_| "Invalid sighash")?
            .to_byte_array();
        let path = path.clone();
        let public = *public;
        Ok(crate::Review {
            psbt,
            outputs,
            fee_sat: fee,
            sighash,
            change_path: path,
            change_public_key: public,
            seed_generation: 0,
        })
    }

    /// Sign a PSBT that has been bound to this seed via a `ReviewBinding`.
    /// Re-derives the change key, recomputes the sighash, refuses to sign
    /// on drift, and verifies the produced signature before returning.
    /// The intermediate private key is wiped on scope exit.
    pub fn sign_psbt(
        &self,
        secp: &Secp256k1<All>,
        review: &ReviewBinding,
    ) -> Result<SignedBinding, SignError> {
        if self.locked {
            return Err(SignError::Locked);
        }
        let psbt = Psbt::deserialize(review.psbt_bytes.as_slice()).map_err(|_| SignError::Parse)?;
        if psbt.inputs.is_empty() || psbt.outputs.is_empty() {
            return Err(SignError::Parse);
        }
        let utxo = psbt.inputs[0]
            .witness_utxo
            .as_ref()
            .ok_or(SignError::MissingWitnessUtxo)?;
        // Recompute the sighash. If it differs from the stored value the
        // review was bound to a different PSBT (or one that was mutated).
        let mut cache = SighashCache::new(&psbt.unsigned_tx);
        let sighash = cache
            .p2wpkh_signature_hash(0, &utxo.script_pubkey, utxo.value, EcdsaSighashType::All)
            .map_err(|_| SignError::Sighash)?;
        let mut sighash_bytes = [0u8; 32];
        sighash_bytes.copy_from_slice(&sighash.to_byte_array());
        if sighash_bytes != review.sighash {
            return Err(SignError::Sighash);
        }
        // Derive the change key on demand. PrivateNode wipes its fields on
        // drop; the chain code zeroize here is a defense-in-depth.
        let mut node = self.master().map_err(|_| SignError::InvalidPath)?;
        for child in review.change_path.into_iter() {
            node = PrivateNode(
                node.0
                    .derive_priv(secp, &DerivationPath::from(alloc::vec![*child]))
                    .map_err(|_| SignError::InvalidPath)?,
            );
        }
        let message = Message::from_digest(sighash_bytes);
        let signature = secp.sign_ecdsa(&message, &node.0.private_key);
        secp.verify_ecdsa(&message, &signature, &review.change_public_key)
            .map_err(|_| SignError::Verify)?;
        let mut signed = psbt;
        signed.inputs[0].partial_sigs.insert(
            bitcoin::PublicKey::new(review.change_public_key),
            bitcoin::ecdsa::Signature::sighash_all(signature),
        );
        Ok(SignedBinding {
            signed_psbt: signed.serialize(),
            generation: 0,
            txid_hex: format!("{}", signed.unsigned_tx.compute_txid()),
        })
    }

    pub fn receive_address(
        &self,
        secp: &Secp256k1<All>,
        index: u32,
    ) -> Result<Address, RecoverError> {
        if self.locked {
            return Err(RecoverError::Locked);
        }
        if index > MAX_ADDRESS_INDEX {
            return Err(RecoverError::InvalidPath);
        }
        let path = DerivationPath::from(alloc::vec![
            ChildNumber::from_normal_idx(0).map_err(|_| RecoverError::InvalidPath)?,
            ChildNumber::from_normal_idx(index).map_err(|_| RecoverError::InvalidPath)?,
        ]);
        let public = self
            .account_xpub(secp)?
            .derive_pub(secp, &path)
            .map_err(|_| RecoverError::InvalidPath)?;
        Ok(Address::p2wpkh(
            &bitcoin::CompressedPublicKey(public.public_key),
            self.network,
        ))
    }

    /// Public descriptor body; coordinator must add its descriptor checksum.
    /// Caller must obtain explicit export approval before exposing this data.
    pub fn descriptor(&self, secp: &Secp256k1<All>, change: bool) -> Result<String, RecoverError> {
        Ok(format!(
            "wpkh([{}/84h/{}h/0h]{}/{}/*)",
            self.fingerprint(secp)?,
            self.coin_type(),
            self.account_xpub(secp)?,
            u8::from(change)
        ))
    }
}

impl Drop for SeedSession {
    fn drop(&mut self) {
        self.lock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const WORDS: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn bip39_trezor_seed_vectors() {
        // Public BIP39 vectors also shipped by bip39 2.2.2's test corpus.
        let words24 = "abandon ".repeat(23) + "art";
        for (words, expected) in [
            (WORDS, "c55257c360c07c72029aebc1b53c05ed0362ada38ead3e3e9efa3708e53495531f09a6987599d18264c1e1c92f2cf141630c7a3c4ab7c81b2f001698e7463b04"),
            (words24.as_str(), "bda85446c68413707090a52022edd26a1c9462295029f2e60cd7c4f2bbd3097170af7a4d73245cafa9c3cca8d561a7c3de6f5d4a10be8ed2a5e608d68f92fcc8"),
        ] {
            let seed = SeedSession::recover(words, "TREZOR", Network::Testnet).unwrap();
            for (index, pair) in expected.as_bytes().chunks_exact(2).enumerate() {
                let byte = u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap();
                assert_eq!(seed.seed[index], byte);
            }
        }
    }

    #[test]
    fn lock_erases_owned_buffer_and_rejects_every_public_derivation() {
        let secp = Secp256k1::new();
        let mut seed = SeedSession::recover(WORDS, "", Network::Testnet).unwrap();
        assert!(seed.seed.iter().any(|b| *b != 0));
        seed.lock();
        seed.lock();
        assert!(seed.seed.iter().all(|b| *b == 0));
        assert!(seed.is_locked());
        assert_eq!(seed.fingerprint(&secp), Err(RecoverError::Locked));
        assert_eq!(seed.account_xpub(&secp), Err(RecoverError::Locked));
        assert_eq!(seed.receive_address(&secp, 0), Err(RecoverError::Locked));
        assert_eq!(seed.descriptor(&secp, false), Err(RecoverError::Locked));
        assert_eq!(seed.descriptor(&secp, true), Err(RecoverError::Locked));
    }

    #[test]
    fn rejects_unsupported_inputs_without_echoing_secrets() {
        assert_eq!(
            SeedSession::recover(WORDS, "", Network::Testnet4).err(),
            Some(RecoverError::UnsupportedNetwork)
        );
        assert_eq!(
            SeedSession::recover("abandon", "", Network::Testnet).err(),
            Some(RecoverError::WordCount)
        );
        let invalid = WORDS.replace("about", "notaword");
        assert_eq!(
            SeedSession::recover(&invalid, "", Network::Testnet).err(),
            Some(RecoverError::UnknownWord(11))
        );
        assert_eq!(
            SeedSession::recover(&WORDS.replace("about", "abandon"), "", Network::Testnet).err(),
            Some(RecoverError::Checksum)
        );
        for pass in ["é", "line\nbreak", "nul\0byte"] {
            assert_eq!(
                SeedSession::recover(WORDS, pass, Network::Testnet).err(),
                Some(RecoverError::UnsupportedCharacter)
            );
        }
        assert_eq!(
            SeedSession::recover(WORDS, &"x".repeat(101), Network::Testnet).err(),
            Some(RecoverError::PassphraseTooLong)
        );
        assert_eq!(
            SeedSession::recover(&"x".repeat(217), "", Network::Testnet).err(),
            Some(RecoverError::MnemonicTooLong)
        );
        let seed = SeedSession::recover(WORDS, "", Network::Testnet).unwrap();
        assert_eq!(
            seed.receive_address(&Secp256k1::new(), 1001),
            Err(RecoverError::InvalidPath)
        );
    }
}
