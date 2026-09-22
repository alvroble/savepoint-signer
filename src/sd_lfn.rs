//! Read-only FAT long-name assembly; never trust an LFN without its SFN checksum.
extern crate alloc;
use alloc::string::String;

pub struct LongName {
    units: [u16; 260],
    next: u8,
    checksum: u8,
    count: usize,
}
impl LongName {
    pub fn new() -> Self {
        Self {
            units: [0xffff; 260],
            next: 0,
            checksum: 0,
            count: 0,
        }
    }
    pub fn reset(&mut self) {
        self.next = 0;
        self.count = 0;
    }
    pub fn slot(&mut self, raw: &[u8]) {
        let order = raw[0];
        let index = order & 0x1f;
        if raw[11] != 0x0f
            || raw[12] != 0
            || raw[26] != 0
            || raw[27] != 0
            || order & 0xa0 != 0
            || index == 0
            || index > 20
        {
            self.reset();
            return;
        }
        if order & 0x40 != 0 {
            self.units.fill(0xffff);
            self.count = index as usize * 13;
            self.next = index;
            self.checksum = raw[13];
        }
        if self.count == 0 || index != self.next || raw[13] != self.checksum {
            self.reset();
            return;
        }
        let offsets = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
        for (n, offset) in offsets.into_iter().enumerate() {
            self.units[(index as usize - 1) * 13 + n] =
                u16::from_le_bytes([raw[offset], raw[offset + 1]]);
        }
        self.next -= 1;
    }
    pub fn finish(&mut self, short: &[u8]) -> Option<String> {
        let checksum = short[..11]
            .iter()
            .fold(0u8, |sum, b| sum.rotate_right(1).wrapping_add(*b));
        let valid = self.count != 0 && self.next == 0 && checksum == self.checksum;
        let count = self.count;
        self.reset();
        if !valid {
            return None;
        }
        let len = self.units[..count]
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(count);
        if len == 0
            || len > 255
            || self.units[..len].contains(&0xffff)
            || self.units[len.saturating_add(1).min(count)..count]
                .iter()
                .any(|c| *c != 0xffff)
        {
            return None;
        }
        String::from_utf16(&self.units[..len]).ok()
    }
}
