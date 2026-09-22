//! `RomInfo` parsing for the cartridge header bytes at $0100-$014F.
//!
//! Mirrors the firmware's `src/rom_info.rs` but stripped of `defmt` so
//! it can be exercised on host. Used by both the MBC3 save tests and
//! the integration packaging to verify Crystal's ROM header requests
//! MBC3+TIMER+RAM+BATTERY with a 2 MiB ROM and 32 KiB SRAM.

#![allow(unused)]

use arrayvec::ArrayString;
use core::str;

#[derive(Debug, PartialEq, Eq)]
pub enum MbcType {
    None,
    Mbc1,
    Mbc2,
    Mbc3,
    Mbc5,
}

#[derive(Debug)]
pub struct RomInfo {
    pub ram_bank_count: u8,
    pub rom_bank_count: u16,
    pub has_rtc: bool,
    pub mbc: MbcType,
    pub savefile: ArrayString<16>,
}

impl RomInfo {
    pub fn from_rom_bytes(first_bank: &[u8], savefile_str: &str) -> Option<Self> {
        if first_bank.len() < 0x150 {
            return None;
        }
        let mbc_dat = first_bank[0x147];

        let (mbc, has_rtc) = match mbc_dat {
            0x00u8 => (MbcType::None, false),
            0x01u8..=0x03u8 => (MbcType::Mbc1, false),
            0x05u8..=0x07u8 => (MbcType::Mbc2, false),
            0x0Fu8..=0x10u8 => (MbcType::Mbc3, true),
            0x11u8..=0x13u8 => (MbcType::Mbc3, false),
            0x19u8..=0x1Eu8 => (MbcType::Mbc5, false),
            _ => return None,
        };

        let lookup = [0u8, 0u8, 1u8, 4u8, 16u8, 8u8];
        let ram_bank_count_value = first_bank[0x149];
        let ram_bank_count = *lookup.get(ram_bank_count_value as usize)?;

        let rom_bank_count = 1u16 << (first_bank[0x0148] as u16 + 1);

        let mut savefile = ArrayString::<16>::new();
        savefile.push_str(savefile_str);

        Some(Self {
            ram_bank_count,
            rom_bank_count,
            has_rtc,
            mbc,
            savefile,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_header(cart_type: u8, rom_size: u8, ram_size: u8) -> Vec<u8> {
        let mut header = vec![0u8; 0x200];
        header[0x147] = cart_type;
        header[0x148] = rom_size;
        header[0x149] = ram_size;
        header
    }

    #[test]
    fn crystal_header_decodes_to_mbc3_rtc_with_2_mib_and_32_kib() {
        let header = make_header(0x10, 0x06, 0x03);
        let info = RomInfo::from_rom_bytes(&header, "POKECRYSTAL.SAV").unwrap();
        assert_eq!(info.mbc, MbcType::Mbc3);
        assert!(info.has_rtc, "0x10 must advertise RTC");
        assert_eq!(info.rom_bank_count, 128, "0x06 -> 128 banks = 2 MiB");
        assert_eq!(info.ram_bank_count, 4, "0x03 -> 4 banks = 32 KiB");
        assert_eq!(&info.savefile[..], "POKECRYSTAL.SAV");
    }

    #[test]
    fn mbc3_without_rtc_keeps_rtc_flag_false() {
        let header = make_header(0x11, 0x06, 0x03);
        let info = RomInfo::from_rom_bytes(&header, "x").unwrap();
        assert_eq!(info.mbc, MbcType::Mbc3);
        assert!(!info.has_rtc);
    }

    #[test]
    fn unknown_cartridge_type_returns_none() {
        let header = make_header(0xFF, 0x06, 0x03);
        assert!(RomInfo::from_rom_bytes(&header, "x").is_none());
    }

    #[test]
    fn ram_size_lookup_table_covers_all_legal_values() {
        // The lookup table lives inside from_rom_bytes; verify by feeding
        // every documented RAM size byte (0x00..=0x05) and ensuring the
        // count never exceeds the documented maximum of 16 banks.
        for ram_size in 0u8..=5 {
            let header = make_header(0x10, 0x06, ram_size);
            let info = RomInfo::from_rom_bytes(&header, "x").unwrap();
            assert!(info.ram_bank_count <= 16);
        }
    }
}
