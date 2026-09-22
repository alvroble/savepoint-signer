//! Seed PC terminal: recovery, public export and explicitly reviewed signing.
//! No USB secret-input API. SD transport stays in the firmware adapter.
use crate::seed::SeedSession;
use alloc::{format, string::String};
use bitcoin::{secp256k1::Secp256k1, Network};
use zeroize::{Zeroize, Zeroizing};

pub const BEGIN12: u8 = 1;
pub const BEGIN24: u8 = 2;
pub const DELETE: u8 = 3;
pub const NEXT: u8 = 4;
pub const PREV: u8 = 5;
pub const CHOOSE: u8 = 6;
pub const BACK: u8 = 7;
pub const RECOVER: u8 = 8;
pub const CONFIRM: u8 = 9;
pub const LOCK: u8 = 10;
pub const VERIFY_NEXT: u8 = 11;
pub const EXPORT_ACCOUNT: u8 = 12;
pub const EXPORT_BACK: u8 = 13;
pub const BEGIN_MAIN12: u8 = 14;
pub const BEGIN_MAIN24: u8 = 15;
pub const EXPORT_SEGWIT: u8 = 16;
pub const IMPORT_PSBT: u8 = 17;
pub const REVIEW_NEXT: u8 = 18;
pub const REVIEW_BACK: u8 = 19;
pub const APPROVE_PSBT: u8 = 20;
pub const CANCEL_PSBT: u8 = 21;
pub const FILE_NEXT: u8 = 22;
pub const FILE_PREV: u8 = 23;
pub const FILE_OPEN: u8 = 24;
pub const NAME_NEXT: u8 = 25;
pub const NAME_PREV: u8 = 26;
pub const PSBT_LISTING: u8 = 16;
pub const PSBT_FILES: u8 = 17;
#[derive(Clone)]
pub struct PsbtFile {
    pub name: String,
    pub alias: String,
    pub size: u32,
}
pub const PSBT_LOADING: u8 = 9;
pub const PSBT_REVIEW: u8 = 10;
pub const PSBT_SIGNING: u8 = 11;
pub const PSBT_ERROR: u8 = 13;
pub const PSBT_SAVING: u8 = 14;
pub const PSBT_SAVED: u8 = 15;
pub const QR_STATE_ERROR: u8 = 8;

/// Mailbox state for the QR-export view. The worker fills `qr_buffer` with the
/// bit-packed QR module grid (one bit per module, row-major) and reports
/// `qr_size` modules per side. The ROM reads the buffer from a cartridge RAM
/// window mapped just after the mailbox (B480 in Game Boy space).
pub const QR_STATE_READY: u8 = 7;
/// Largest QR module grid we will publish. qrcodegen picks Version 6-L (41x41)
/// for a 111-character extended public key. Larger grids are rejected.
pub const QR_MAX_MODULES: usize = 41;
pub const QR_BUFFER_BYTES: usize = (QR_MAX_MODULES * QR_MAX_MODULES + 7) / 8;

pub struct Terminal {
    pub state: u8, // 0 locked, 1 words, 2 passphrase, 3 busy, 4 identity, 5 active, 6 checksum/error
    files: alloc::vec::Vec<PsbtFile>,
    file_index: usize,
    name_page: usize,
    review: Option<crate::Review>,
    review_page: usize,
    psbt_message: String,
    network: Network,
    count: usize,
    slot: usize,
    words: Zeroizing<[u16; 24]>,
    prefix: Zeroizing<[u8; 4]>,
    prefix_len: usize,
    candidate: usize,
    pass: Zeroizing<[u8; 100]>,
    pass_len: usize,
    seed: Option<SeedSession>,
    identity: [u8; 52],
    address_index: u16,
    /// QR export state. `qr_size == 0` means "no QR generated yet".
    pub qr_size: u16,
    /// Bit-packed QR module grid (1 = dark, 0 = light). Row-major, size
    /// `qr_size * qr_size` rounded up to bytes. Cleared on LOCK.
    pub qr_buffer: [u8; QR_BUFFER_BYTES],
}
impl Default for Terminal {
    fn default() -> Self {
        Self {
            state: 0,
            files: alloc::vec::Vec::new(),
            file_index: 0,
            name_page: 0,
            review: None,
            review_page: 0,
            psbt_message: String::new(),
            network: Network::Testnet,
            count: 0,
            slot: 0,
            words: Zeroizing::new([0; 24]),
            prefix: Zeroizing::new([0; 4]),
            prefix_len: 0,
            candidate: 0,
            pass: Zeroizing::new([0; 100]),
            pass_len: 0,
            seed: None,
            identity: [0; 52],
            address_index: 0,
            qr_size: 0,
            qr_buffer: [0; QR_BUFFER_BYTES],
        }
    }
}
impl Terminal {
    fn clear_entry(&mut self) {
        self.words.zeroize();
        self.prefix.zeroize();
        self.pass.zeroize();
        self.prefix_len = 0;
        self.pass_len = 0;
        self.candidate = 0;
    }
    fn clear_qr(&mut self) {
        self.qr_size = 0;
        self.qr_buffer.fill(0);
    }
    /* Build a bounded QR for the loaded seed's account xpub and pack
     * the module grid into qr_buffer. Only public material is encoded. */
    fn export_qr(&mut self, segwit: bool) -> Result<(), &'static str> {
        let seed = self.seed.as_ref().ok_or("no seed loaded")?;
        let secp = Secp256k1::new();
        let xpub = seed
            .account_export(&secp, segwit)
            .map_err(|_| "xpub derivation failed")?;

        let qr = qrcodegen::QrCode::encode_text(&xpub, qrcodegen::QrCodeEcc::Low)
            .map_err(|_| "xpub too long for QR")?;
        let size = qr.size();
        if (size as usize) > QR_MAX_MODULES {
            return Err("QR too large for gameboy screen");
        }
        self.qr_size = size as u16;
        self.qr_buffer.fill(0);
        let mut i: usize = 0;
        for y in 0..size {
            for x in 0..size {
                if qr.get_module(x, y) {
                    self.qr_buffer[i / 8] |= 1 << (i % 8);
                }
                i += 1;
            }
        }
        self.state = QR_STATE_READY;
        Ok(())
    }
    /* Recompute the public address slot from the loaded seed. Used by the
     * receive-verification screen so the Game Boy can page through receive
     * addresses without exposing seed material. */
    fn refresh_address(&mut self) {
        if let Some(seed) = &self.seed {
            let secp = bitcoin::secp256k1::Secp256k1::new();
            match seed.receive_address(&secp, self.address_index as u32) {
                Ok(address) => {
                    let s = format!("{address}");
                    self.identity[8..].fill(0);
                    let n = s.len().min(44);
                    self.identity[8..8 + n].copy_from_slice(&s.as_bytes()[..n]);
                }
                Err(_) => {
                    self.address_index = self.address_index.saturating_sub(1);
                }
            }
        }
    }
    fn matches(&self) -> impl Iterator<Item = (usize, &'static str)> + '_ {
        bip39::Language::English
            .word_list()
            .iter()
            .enumerate()
            .filter(|(_, w)| w.as_bytes().starts_with(&self.prefix[..self.prefix_len]))
            .map(|(i, w)| (i, *w))
    }
    pub fn command(&mut self, c: u8) {
        if matches!(c, LOCK | BEGIN12 | BEGIN24 | BEGIN_MAIN12 | BEGIN_MAIN24) {
            self.files.clear();
            self.review.take();
            self.psbt_message.clear();
            self.seed.take();
            self.clear_entry();
            self.identity.fill(0);
            self.address_index = 0;
            self.clear_qr();
            self.slot = 0;
            self.count = 0;
            self.state = 0;
            if c != LOCK {
                self.network = if matches!(c, BEGIN_MAIN12 | BEGIN_MAIN24) {
                    Network::Bitcoin
                } else {
                    Network::Testnet
                };
                self.count = if matches!(c, BEGIN12 | BEGIN_MAIN12) {
                    12
                } else {
                    24
                };
                self.state = 1;
            }
            return;
        }
        if c == IMPORT_PSBT && self.state == 5 {
            self.review.take();
            self.review_page = 0;
            self.psbt_message.clear();
            self.files.clear();
            self.file_index = 0;
            self.name_page = 0;
            self.state = PSBT_LISTING;
            return;
        }
        if c == CANCEL_PSBT
            && matches!(
                self.state,
                PSBT_FILES | PSBT_REVIEW | PSBT_ERROR | PSBT_SAVED
            )
        {
            self.files.clear();
            self.review.take();
            self.psbt_message.clear();
            self.state = 5;
            return;
        }
        if self.state == PSBT_FILES && !self.files.is_empty() {
            let pages = self.files[self.file_index]
                .name
                .chars()
                .count()
                .div_ceil(60)
                .max(1);
            match c {
                FILE_NEXT => {
                    self.file_index = (self.file_index + 1) % self.files.len();
                    self.name_page = 0;
                }
                FILE_PREV => {
                    self.file_index = (self.file_index + self.files.len() - 1) % self.files.len();
                    self.name_page = 0;
                }
                NAME_NEXT if self.name_page + 1 < pages => self.name_page += 1,
                NAME_PREV if self.name_page > 0 => self.name_page -= 1,
                FILE_OPEN => {
                    self.files.clear();
                    self.state = PSBT_LOADING;
                }
                _ => {}
            }
            return;
        }
        if self.state == PSBT_REVIEW {
            let last = self
                .review
                .as_ref()
                .map(|r| r.outputs().len() + 1)
                .unwrap_or(0);
            match c {
                REVIEW_NEXT if self.review_page < last => self.review_page += 1,
                REVIEW_BACK if self.review_page > 0 => self.review_page -= 1,
                APPROVE_PSBT if self.review_page == last && self.review.is_some() => {
                    self.state = PSBT_SIGNING
                }
                _ => {}
            }
            return;
        }
        if c == CONFIRM && self.state == 4 {
            self.state = 5;
            return;
        }
        if c == VERIFY_NEXT && self.state == 5 && self.seed.is_some() {
            self.address_index = (self.address_index + 1) % 1001;
            self.refresh_address();
            return;
        }
        if matches!(c, EXPORT_ACCOUNT | EXPORT_SEGWIT) && self.state == 5 {
            self.clear_qr();
            if self.export_qr(c == EXPORT_SEGWIT).is_err() {
                self.clear_qr();
                self.state = QR_STATE_ERROR;
            }
            return;
        }
        if c == EXPORT_BACK && matches!(self.state, QR_STATE_READY | QR_STATE_ERROR) {
            self.clear_qr();
            self.state = 5;
            return;
        }
        if self.state == 6 && c == BACK {
            self.state = 1;
            self.slot = self.count - 1;
            return;
        }
        if self.state == 1 {
            match c {
                0x80..=0xde => {
                    let b = c - 0x80 + 32;
                    if b.is_ascii_lowercase() && self.prefix_len < 4 {
                        self.prefix[self.prefix_len] = b;
                        self.prefix_len += 1;
                        if self.matches().next().is_none() {
                            self.prefix_len -= 1;
                            self.prefix[self.prefix_len] = 0;
                        }
                        self.candidate = 0;
                    }
                }
                DELETE => {
                    if self.prefix_len > 0 {
                        self.prefix_len -= 1;
                        self.prefix[self.prefix_len] = 0;
                        self.candidate = 0;
                    }
                }
                NEXT => {
                    let n = self.matches().count();
                    self.candidate = (self.candidate + 1) % n;
                }
                PREV => {
                    let n = self.matches().count();
                    self.candidate = (self.candidate + n - 1) % n;
                }
                CHOOSE => {
                    let index = self.matches().nth(self.candidate).map(|(i, _)| i);
                    if let Some(i) = index {
                        self.words[self.slot] = i as u16;
                        self.prefix.zeroize();
                        self.prefix_len = 0;
                        self.candidate = 0;
                        if self.slot + 1 == self.count {
                            self.state = 2;
                        } else {
                            self.slot += 1;
                        }
                    }
                }
                BACK => {
                    if self.slot > 0 {
                        self.slot -= 1;
                    }
                    self.prefix.zeroize();
                    self.prefix_len = 0;
                    self.candidate = 0;
                }
                _ => {}
            }
        } else if self.state == 2 {
            match c {
                0x80..=0xde if self.pass_len < 100 => {
                    self.pass[self.pass_len] = c - 0x80 + 32;
                    self.pass_len += 1;
                }
                DELETE if self.pass_len > 0 => {
                    self.pass_len -= 1;
                    self.pass[self.pass_len] = 0;
                }
                BACK => {
                    self.state = 1;
                }
                RECOVER => {
                    self.state = 3;
                }
                _ => {}
            }
        }
    }
    /// Worker calls this only after publishing BUSY; never on the USB task.
    pub fn finish_recovery(&mut self) {
        if self.state != 3 {
            return;
        }
        let mut phrase = Zeroizing::new(String::with_capacity(216));
        for i in 0..self.count {
            if i > 0 {
                phrase.push(' ');
            }
            phrase.push_str(bip39::Language::English.word_list()[self.words[i] as usize]);
        }
        let pass = core::str::from_utf8(&self.pass[..self.pass_len]).unwrap();
        let result = SeedSession::recover(&phrase, pass, self.network);
        match result {
            Ok(seed) => {
                let secp = Secp256k1::new();
                match (seed.fingerprint(&secp), seed.receive_address(&secp, 0)) {
                    (Ok(fp), Ok(address)) => {
                        let fp = format!("{fp}");
                        let address = format!("{address}");
                        self.identity[..8].copy_from_slice(fp.as_bytes());
                        self.identity[8..8 + address.len()].copy_from_slice(address.as_bytes());
                        self.seed = Some(seed);
                        self.clear_entry();
                        self.state = 4;
                    }
                    _ => {
                        self.state = 6;
                    }
                }
            }
            Err(_) => {
                self.state = 6;
            }
        }
    }
    pub fn set_files(&mut self, files: alloc::vec::Vec<PsbtFile>) {
        if self.state != PSBT_LISTING {
            return;
        }
        if files.is_empty() {
            self.psbt_error("No .psbt/.psb files");
            return;
        }
        if files.len() > 32 {
            self.psbt_error("Too many PSBT files");
            return;
        }
        self.files = files;
        self.file_index = 0;
        self.name_page = 0;
        self.state = PSBT_FILES;
    }
    pub fn selected_file_index(&self) -> usize {
        self.file_index
    }
    pub fn psbt_error(&mut self, message: &str) {
        self.files.clear();
        self.review.take();
        self.psbt_message = String::from(message);
        self.state = PSBT_ERROR;
    }
    pub fn load_psbt(&mut self, bytes: &[u8]) {
        if self.state != PSBT_LOADING {
            return;
        }
        let result = self
            .seed
            .as_ref()
            .ok_or("Seed locked")
            .and_then(|w| w.review_psbt(bytes));
        match result {
            Ok(review) => {
                self.review = Some(review);
                self.review_page = 0;
                self.state = PSBT_REVIEW;
            }
            Err(message) => self.psbt_error(message),
        }
    }
    pub fn finish_signing(&mut self) -> Option<alloc::vec::Vec<u8>> {
        if self.state != PSBT_SIGNING {
            return None;
        }
        let review = self.review.take()?;
        let binding = crate::seed::ReviewBinding {
            psbt_bytes: Zeroizing::new(review.approved_psbt_bytes()),
            sighash: *review.sighash(),
            change_public_key: review.change_public_key,
            change_path: review.change_path.clone(),
        };
        let result = self.seed.as_ref().ok_or("Seed locked").and_then(|w| {
            w.sign_psbt(&Secp256k1::new(), &binding)
                .map_err(|_| "Signing failed")
        });
        match result {
            Ok(signed) => {
                self.state = PSBT_SAVING;
                Some(signed.signed_psbt)
            }
            Err(message) => {
                self.psbt_error(message);
                None
            }
        }
    }
    pub fn psbt_saved(&mut self, filename: &str) {
        if self.state == PSBT_SAVING {
            self.psbt_message = String::from(filename);
            self.state = PSBT_SAVED;
        }
    }
    /// Test-only: read-only view of the chosen word indices and the active
    /// count/slot. Always available (not gated on state==3) so the emulator
    /// bridge can inspect words at every step.
    pub fn debug_recovery_state(&self) -> (usize, usize, &[u16]) {
        (
            self.count,
            self.slot,
            &self.words[..self.count.max(self.slot + 1)],
        )
    }
    /// Test-only: read-only view of the current prefix bytes.
    #[allow(dead_code)]
    pub fn debug_prefix(&self) -> alloc::string::String {
        alloc::string::String::from_utf8_lossy(&self.prefix[..self.prefix_len]).into_owned()
    }
    /// Public view only: no complete mnemonic, passphrase, seed or private key.
    pub fn report(&self) -> [u8; 128] {
        let mut r = [0u8; 128];
        r[1] = self.state;
        r[2] = self.count as u8;
        r[3] = self.slot as u8;
        r[4] = self.prefix_len as u8;
        r[5] = self.pass_len as u8;
        r[6] = u8::from(self.network == Network::Bitcoin);
        r[10..14].copy_from_slice(&self.prefix[..]);
        if self.state == 1 {
            for (row, (_, w)) in self.matches().skip(self.candidate).take(3).enumerate() {
                r[16 + row * 9..16 + row * 9 + w.len()].copy_from_slice(w.as_bytes());
            }
        }
        if self.state == 4 || self.state == 5 {
            r[48..56].copy_from_slice(&self.identity[..8]);
            r[64..108].copy_from_slice(&self.identity[8..]);
        }
        // QR-export view: state == 7 means a fresh QR is waiting at the
        // cartridge RAM window. The two size bytes carry the module count.
        if self.state == QR_STATE_READY {
            r[64] = (self.qr_size & 0xFF) as u8;
            r[65] = ((self.qr_size >> 8) & 0xFF) as u8;
        }
        fn put(dst: &mut [u8], text: &str) {
            let n = dst.len().min(text.len());
            dst[..n].copy_from_slice(&text.as_bytes()[..n]);
        }
        if self.state == PSBT_FILES {
            let file = &self.files[self.file_index];
            r[8] = self.file_index as u8;
            r[9] = self.files.len() as u8;
            r[10] = self.name_page as u8;
            r[11] = file.name.chars().count().div_ceil(60).max(1) as u8;
            let display: String = file
                .name
                .chars()
                .skip(self.name_page * 60)
                .take(60)
                .map(|c| {
                    if c.is_ascii() && !c.is_ascii_control() {
                        c
                    } else {
                        '?'
                    }
                })
                .collect();
            put(&mut r[16..76], &display);
            let alias: String = file
                .alias
                .chars()
                .map(|c| {
                    if c.is_ascii() && !c.is_ascii_control() {
                        c
                    } else {
                        '?'
                    }
                })
                .collect();
            put(&mut r[80..92], &alias);
            put(&mut r[96..108], &format!("{}", file.size));
        }
        if self.state == PSBT_REVIEW {
            if let Some(review) = &self.review {
                let n = review.outputs().len();
                r[8] = self.review_page as u8;
                r[9] = (n + 2) as u8;
                if self.review_page < n {
                    let out = &review.outputs()[self.review_page];
                    put(
                        &mut r[16..36],
                        &format!(
                            "{} {}",
                            if out.is_change { "Change" } else { "Output" },
                            self.review_page + 1
                        ),
                    );
                    put(&mut r[36..56], &format!("{} sat", out.amount_sat));
                    put(&mut r[64..108], &out.address);
                } else if self.review_page == n {
                    put(&mut r[16..36], "Transaction fee");
                    put(&mut r[36..56], &format!("{} sat", review.fee_sat()));
                } else {
                    r[10] = 1;
                    put(&mut r[16..36], "Approve signing?");
                }
            }
        }
        if matches!(self.state, PSBT_ERROR | PSBT_SAVED) {
            put(&mut r[16..64], &self.psbt_message);
        }
        r[127] = 1;
        r
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    fn type_text(t: &mut Terminal, s: &str) {
        for b in s.bytes() {
            t.command(b - 32 + 0x80);
        }
    }

    #[test]
    fn signing_requires_complete_review_and_cancel_or_replacement_clears_it() {
        let words = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let mut t = Terminal::default();
        t.seed = Some(SeedSession::recover(words, "", Network::Testnet).unwrap());
        t.state = 5;
        let bytes = include_bytes!("../tests/fixtures/game-test.psb");
        for abort in [CANCEL_PSBT, LOCK, BEGIN_MAIN12] {
            if t.seed.is_none() {
                t.seed = Some(SeedSession::recover(words, "", Network::Testnet).unwrap());
            }
            t.state = 5;
            t.command(IMPORT_PSBT);
            t.set_files(alloc::vec![PsbtFile {
                name: String::from("test.psbt"),
                alias: String::from("TEST~1.PSB"),
                size: 1
            }]);
            t.command(FILE_OPEN);
            t.load_psbt(bytes);
            assert_eq!(t.state, PSBT_REVIEW);
            t.command(APPROVE_PSBT);
            assert_eq!(t.state, PSBT_REVIEW);
            assert!(t.finish_signing().is_none());
            for _ in 0..3 {
                t.command(REVIEW_NEXT);
            }
            assert_eq!(t.report()[10], 1);
            t.command(abort);
            assert!(t.review.is_none());
            t.command(APPROVE_PSBT);
            assert!(t.finish_signing().is_none());
        }
        t.seed = Some(SeedSession::recover(words, "", Network::Testnet).unwrap());
        t.state = 5;
        t.command(IMPORT_PSBT);
        t.set_files(alloc::vec![PsbtFile {
            name: String::from("test.psbt"),
            alias: String::from("TEST~1.PSB"),
            size: 1
        }]);
        t.command(FILE_OPEN);
        t.load_psbt(bytes);
        for _ in 0..3 {
            t.command(REVIEW_NEXT);
        }
        t.command(APPROVE_PSBT);
        assert!(t.finish_signing().is_some());
        assert!(t.finish_signing().is_none()); // never sign twice
        assert_eq!(t.state, PSBT_SAVING);
        t.psbt_saved("SIGNED.PSB");
        assert_eq!(t.state, PSBT_SAVED);
        t.command(CANCEL_PSBT);
        assert_eq!(t.state, 5);
        t.command(IMPORT_PSBT);
        t.set_files(alloc::vec![PsbtFile {
            name: String::from("test.psbt"),
            alias: String::from("TEST~1.PSB"),
            size: 1
        }]);
        t.command(FILE_OPEN);
        t.load_psbt(b"invalid");
        assert_eq!(t.state, PSBT_ERROR);
        assert!(t.review.is_none());
    }
    #[test]
    fn network_change_replaces_seed_and_clears_export() {
        let mut t = Terminal::default();
        for begin in [BEGIN_MAIN12, BEGIN12, BEGIN_MAIN24] {
            t.command(begin);
            assert!(t.seed.is_none());
            assert!(t.qr_buffer.iter().all(|b| *b == 0));
            assert!(t.identity.iter().all(|b| *b == 0));
            assert_eq!(t.report()[6], u8::from(begin != BEGIN12));
            if begin == BEGIN_MAIN24 {
                assert_eq!(t.count, 24);
                break;
            }
            for _ in 0..11 {
                type_text(&mut t, "aban");
                t.command(CHOOSE);
            }
            type_text(&mut t, "abou");
            t.command(CHOOSE);
            t.command(RECOVER);
            t.finish_recovery();
            t.command(CONFIRM);
            assert_eq!(t.state, 5);
            let prefix = if begin == BEGIN12 { b"tb1q" } else { b"bc1q" };
            assert_eq!(&t.report()[64..68], prefix);
            t.command(EXPORT_SEGWIT);
            assert_eq!(t.state, QR_STATE_READY);
        }
    }

    #[test]
    fn recover_confirm_lock() {
        let mut t = Terminal::default();
        t.command(BEGIN12);
        for _ in 0..11 {
            type_text(&mut t, "aban");
            t.command(CHOOSE);
        }
        type_text(&mut t, "abou");
        t.command(CHOOSE);
        assert_eq!(t.state, 2);
        t.command(RECOVER);
        assert_eq!(t.state, 3);
        t.finish_recovery();
        assert_eq!(t.state, 4);
        assert_eq!(&t.report()[48..56], b"73c5da0a");
        assert!(t.words.iter().all(|x| *x == 0));
        assert!(t.pass.iter().all(|x| *x == 0));
        t.command(CONFIRM);
        assert_eq!(t.state, 5);
        t.command(LOCK);
        assert_eq!(t.state, 0);
        assert!(t.seed.is_none());
        assert!(t.report()[48..108].iter().all(|x| *x == 0));
    }
    #[test]
    fn correction_bounds_and_error() {
        let mut t = Terminal::default();
        t.command(BEGIN24);
        type_text(&mut t, "zzzzzz");
        assert!(t.prefix_len <= 4);
        assert!(t.matches().count() > 0);
        t.command(BEGIN12);
        for _ in 0..12 {
            t.command(CHOOSE);
        }
        t.command(RECOVER);
        t.finish_recovery();
        assert_eq!(t.state, 6);
        t.command(BACK);
        assert_eq!(t.state, 1);
        assert_eq!(t.slot, 11);
        type_text(&mut t, "abou");
        t.command(CHOOSE);
        type_text(&mut t, &"x".repeat(110));
        assert_eq!(t.pass_len, 100);
        t.command(DELETE);
        assert_eq!(t.pass_len, 99);
        t.command(LOCK);
        assert!(t.pass.iter().all(|b| *b == 0));
    }
    #[test]
    fn verify_next_cycles_receive_addresses() {
        let mut t = Terminal::default();
        t.command(BEGIN12);
        for _ in 0..11 {
            type_text(&mut t, "aban");
            t.command(CHOOSE);
        }
        type_text(&mut t, "abou");
        t.command(CHOOSE);
        t.command(RECOVER);
        t.finish_recovery();
        assert_eq!(t.state, 4);
        t.command(CONFIRM);
        assert_eq!(t.state, 5);
        let first: Vec<u8> = t.report()[64..108].to_vec();
        assert!(
            first.iter().take_while(|&&b| b != 0).any(|&b| b == b't'),
            "expected a testnet bech32 prefix in receive[0]"
        );
        t.command(VERIFY_NEXT);
        assert_eq!(t.address_index, 1);
        let second: Vec<u8> = t.report()[64..108].to_vec();
        assert_ne!(
            first, second,
            "VERIFY_NEXT must change the published address"
        );
        t.command(LOCK);
        assert_eq!(t.address_index, 0);
        assert!(t.report()[64..108].iter().all(|&b| b == 0));
    }
    #[test]
    fn verify_next_ignored_without_seed() {
        let mut t = Terminal::default();
        t.command(VERIFY_NEXT);
        assert_eq!(t.address_index, 0);
        assert_eq!(t.state, 0);
    }
    #[test]
    fn export_account_packs_qr_for_loaded_seed() {
        // Recover the well-known TREZOR mnemonic and produce the QR for the
        // account public key. Check packing here; the emulator test independently
        // decodes the rendered LCD using zbar and compares with embit vectors.
        let mut t = Terminal::default();
        t.command(BEGIN12);
        for _ in 0..11 {
            type_text(&mut t, "aban");
            t.command(CHOOSE);
        }
        type_text(&mut t, "abou");
        t.command(CHOOSE);
        t.command(RECOVER);
        t.finish_recovery();
        t.command(CONFIRM);
        assert_eq!(t.state, 5);
        t.command(EXPORT_ACCOUNT);
        assert_eq!(t.state, QR_STATE_READY);
        assert!(t.qr_size > 0);
        assert!(t.qr_buffer.iter().any(|&b| b != 0), "qr buffer all zeros");
        // qr_size fits in the two bytes we publish.
        let report = t.report();
        let published_size = report[64] as u16 | ((report[65] as u16) << 8);
        assert_eq!(published_size, t.qr_size);
        // Sanity: a tpub at ECC Low fits in Version 6 (41x41). Bumping ECC
        // moves this up; the buffer and cartridge RAM window both still fit.
        assert_eq!(t.qr_size, 41);
        // Independent re-encoding must match the terminal's bit-packed grid.
        let secp = Secp256k1::new();
        let xpub = format!(
            "{}",
            t.seed.as_ref().unwrap().account_xpub(&secp).unwrap()
        );
        let qr = qrcodegen::QrCode::encode_text(&xpub, qrcodegen::QrCodeEcc::Low).unwrap();
        assert_eq!(qr.size() as u16, t.qr_size);
        let mut i: usize = 0;
        for y in 0..qr.size() {
            for x in 0..qr.size() {
                let expected_bit = qr.get_module(x, y);
                let actual_bit = (t.qr_buffer[i / 8] >> (i % 8)) & 1 != 0;
                assert_eq!(actual_bit, expected_bit, "module ({x},{y}) mismatch");
                i += 1;
            }
        }
        t.command(EXPORT_BACK);
        assert_eq!(t.state, 5);
        assert!(t.qr_buffer.iter().all(|b| *b == 0));
        t.command(EXPORT_ACCOUNT);
        assert_eq!(t.state, QR_STATE_READY);

        // LOCK clears the QR.
        t.command(LOCK);
        assert_eq!(t.qr_size, 0);
        assert!(t.qr_buffer.iter().all(|&b| b == 0));
    }
    #[test]
    fn export_account_without_seed_does_nothing() {
        let mut t = Terminal::default();
        t.command(EXPORT_ACCOUNT);
        // No seed -> state stays at 0, qr empty.
        assert_eq!(t.state, 0);
        assert_eq!(t.qr_size, 0);
        assert!(t.qr_buffer.iter().all(|&b| b == 0));
    }
}
