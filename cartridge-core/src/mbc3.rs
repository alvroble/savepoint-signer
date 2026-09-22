//! MBC3 state machine. Mirrors the routing logic in the firmware's
//! `src/gb_mbc.rs` but stripped of any `embassy-rp` PIO/DMA dependency
//! so it can be exercised from host-side unit tests.

pub trait MbcRamControl {
    fn enable_ram_access(&mut self);
    fn disable_ram_access(&mut self);
    fn enable_rtc_access(&mut self);
}

pub trait MbcRtcControl {
    fn process(&mut self);
    fn trigger_latch(&mut self);
    fn activate_register(&mut self, reg_num: u8);
}

/// MBC3 cartridge state machine. The cartridge firmware relies on every
/// transition here being correct, so the host tests cover the SRAM
/// enable/disable sequencing, all four RAM banks, the RTC register
/// selection, the ROM-bank wrap-around from 0x00 to 0x01, and the
/// Crystal-style in-game save write sequence end-to-end.
#[derive(Debug, PartialEq, Eq)]
pub struct Mbc3State {
    pub rom_bank: u8,
    pub ram_bank: u8,
    pub ram_enabled: bool,
    pub rtc_latch: bool,
    rom_bank_pending: u8,
}

impl Mbc3State {
    pub const ROM_BANK_MASK: u8 = 0x7F;
    pub const RAM_BANK_MASK: u8 = 0x07;
    const ROM_BANK_SIZE: u32 = 0x4000;
    const RAM_BANK_SIZE: usize = 0x2000;

    pub const fn new() -> Self {
        Self {
            rom_bank: 1,
            rom_bank_pending: 1,
            ram_bank: 1,
            ram_enabled: false,
            rtc_latch: false,
        }
    }

    /// Drive the state machine with one cartridge write. `addr` is the
    /// 16-bit Game Boy address (we only look at $0000-$7FFF through the
    /// $E000 mask), `data` is the value the game wrote. `rom_bank` and
    /// `ram_bank_ptr` are the live pointers the DMA machine uses to map
    /// the active ROM/RAM bank into the Game Boy address window.
    /// `gb_ram_memory` is the full 32 KiB SRAM backing the four cartridge
    /// banks; the active bank is exposed via `ram_bank_ptr`.
    pub fn step(
        &mut self,
        addr: u32,
        data: u8,
        rom_bank: &mut u32,
        ram_bank_ptr: &mut *mut u8,
        gb_ram_memory: &mut [u8],
        ram_control: &mut dyn MbcRamControl,
        rtc_control: &mut dyn MbcRtcControl,
    ) {
        match addr & 0xE000u32 {
            0x0000u32 => {
                self.ram_enabled = (data & 0x0F) == 0x0A;
                if self.ram_enabled {
                    if self.ram_bank & 0x08u8 == 0x08u8 {
                        ram_control.enable_rtc_access();
                    } else {
                        ram_control.enable_ram_access();
                    }
                } else {
                    ram_control.disable_ram_access();
                }
            }
            0x2000u32 => {
                self.rom_bank_pending = data & Self::ROM_BANK_MASK;
                if self.rom_bank_pending == 0x00 {
                    self.rom_bank_pending = 0x01;
                }
            }
            0x4000u32 => {
                self.ram_bank = data;
                if self.ram_bank & 0x08u8 == 0x08u8 {
                    rtc_control.activate_register(self.ram_bank & Self::RAM_BANK_MASK);
                    if self.ram_enabled {
                        ram_control.enable_rtc_access();
                    }
                } else {
                    unsafe {
                        core::ptr::write_volatile(
                            ram_bank_ptr,
                            gb_ram_memory
                                .as_mut_ptr()
                                .add(
                                    (self.ram_bank & Self::RAM_BANK_MASK) as usize
                                        * Self::RAM_BANK_SIZE,
                                ),
                        )
                    }
                    if self.ram_enabled {
                        ram_control.enable_ram_access();
                    }
                }
            }
            0x6000u32 => {
                if data != 0 {
                    if !self.rtc_latch {
                        self.rtc_latch = true;
                        rtc_control.trigger_latch();
                    }
                } else {
                    self.rtc_latch = false;
                }
            }
            _ => {}
        }

        self.rom_bank_pending &= Self::ROM_BANK_MASK;
        if self.rom_bank != self.rom_bank_pending {
            self.rom_bank = self.rom_bank_pending;
            unsafe {
                core::ptr::write_volatile(
                    rom_bank,
                    self.rom_bank as u32 * Self::ROM_BANK_SIZE,
                );
            }
        }
    }
}

impl Default for Mbc3State {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    #[derive(Default)]
    struct MockRam {
        events: Vec<RamEvent>,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum RamEvent {
        EnableRam,
        DisableRam,
        EnableRtc,
    }

    impl MbcRamControl for MockRam {
        fn enable_ram_access(&mut self) {
            self.events.push(RamEvent::EnableRam);
        }
        fn disable_ram_access(&mut self) {
            self.events.push(RamEvent::DisableRam);
        }
        fn enable_rtc_access(&mut self) {
            self.events.push(RamEvent::EnableRtc);
        }
    }

    #[derive(Default)]
    struct MockRtc {
        triggers: u32,
        activated: Vec<u8>,
        processed: u32,
    }

    impl MbcRtcControl for MockRtc {
        fn process(&mut self) {
            self.processed += 1;
        }
        fn trigger_latch(&mut self) {
            self.triggers += 1;
        }
        fn activate_register(&mut self, reg_num: u8) {
            self.activated.push(reg_num);
        }
    }

    fn new_state() -> (Mbc3State, u32, *mut u8, [u8; 0x8000]) {
        (Mbc3State::new(), 0, core::ptr::null_mut(), [0u8; 0x8000])
    }

    fn ptr_to_bank(memory: &mut [u8; 0x8000], bank: usize) -> *mut u8 {
        unsafe { memory.as_mut_ptr().add(bank * 0x2000) }
    }

    #[test]
    fn initial_state_is_sram_disabled() {
        let (state, _, _, _) = new_state();
        assert_eq!(state.rom_bank, 1);
        assert_eq!(state.ram_bank, 1);
        assert!(!state.ram_enabled);
        assert!(!state.rtc_latch);
    }

    #[test]
    fn enable_sram_with_0x0a_only() {
        for data in [0x0Au8, 0x1Au8, 0x2Au8, 0xFAu8] {
            let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
            let mut ram = MockRam::default();
            let mut rtc = MockRtc::default();
            state.step(0x0000, data, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
            assert!(state.ram_enabled, "data {:#x} should enable SRAM", data);
            assert_eq!(ram.events, vec![RamEvent::EnableRam]);
        }
        for data in [0x00u8, 0x01u8, 0x09u8, 0x0Bu8, 0xFFu8] {
            let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
            let mut ram = MockRam::default();
            let mut rtc = MockRtc::default();
            state.step(0x0000, data, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
            assert!(!state.ram_enabled, "data {:#x} should disable SRAM", data);
            assert_eq!(ram.events, vec![RamEvent::DisableRam]);
        }
    }

    #[test]
    fn enable_with_rtc_bank_selects_rtc_path() {
        let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
        let mut ram = MockRam::default();
        let mut rtc = MockRtc::default();
        state.step(0x4000, 0x08, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        state.step(0x0000, 0x0A, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        assert!(state.ram_enabled);
        assert_eq!(rtc.activated, vec![0]);
        assert_eq!(ram.events, vec![RamEvent::EnableRtc]);
    }

    #[test]
    fn disable_sram_after_enable() {
        let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
        let mut ram = MockRam::default();
        let mut rtc = MockRtc::default();
        state.step(0x0000, 0x0A, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        state.step(0x0000, 0x00, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        assert!(!state.ram_enabled);
        assert_eq!(ram.events, vec![RamEvent::EnableRam, RamEvent::DisableRam]);
    }

    #[test]
    fn ram_bank_pointer_is_set_for_every_normal_bank() {
        for bank in 0u8..=3 {
            let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
            let mut ram = MockRam::default();
            let mut rtc = MockRtc::default();
            state.step(0x4000, bank, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
            assert_eq!(state.ram_bank, bank);
            assert_eq!(ram_ptr, ptr_to_bank(&mut memory, bank as usize));
            assert_eq!(ram.events, Vec::new());
        }
    }

    #[test]
    fn ram_bank_pointer_is_set_when_sram_already_enabled() {
        let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
        let mut ram = MockRam::default();
        let mut rtc = MockRtc::default();
        state.step(0x0000, 0x0A, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        state.step(0x4000, 2, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        assert_eq!(state.ram_bank, 2);
        assert_eq!(ram_ptr, ptr_to_bank(&mut memory, 2));
        assert_eq!(ram.events, vec![RamEvent::EnableRam, RamEvent::EnableRam]);
    }

    #[test]
    fn rtc_register_is_activated_for_each_bank_8_to_c() {
        for (raw_bank, expected_reg) in [
            (0x08u8, 0u8),
            (0x09, 1),
            (0x0A, 2),
            (0x0B, 3),
            (0x0C, 4),
        ] {
            let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
            let mut ram = MockRam::default();
            let mut rtc = MockRtc::default();
            state.step(
                0x4000,
                raw_bank,
                &mut rom,
                &mut ram_ptr,
                &mut memory,
                &mut ram,
                &mut rtc,
            );
            assert_eq!(state.ram_bank, raw_bank);
            assert_eq!(rtc.activated, vec![expected_reg]);
            assert!(ram_ptr.is_null());
        }
    }

    #[test]
    fn rom_bank_wrap_zero_to_one() {
        let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
        let mut ram = MockRam::default();
        let mut rtc = MockRtc::default();
        // First move to a non-1 bank so the pointer update actually fires.
        state.step(0x2000, 0x05, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        assert_eq!(rom, 5u32 * 0x4000);
        // Now write 0x00, which wraps to bank 1 and updates the pointer.
        state.step(0x2000, 0x00, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        assert_eq!(state.rom_bank, 1);
        assert_eq!(rom, 0x4000u32);
    }

    #[test]
    fn rom_bank_truncated_to_seven_bits() {
        let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
        let mut ram = MockRam::default();
        let mut rtc = MockRtc::default();
        state.step(0x2000, 0xFF, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        assert_eq!(state.rom_bank, 0x7F);
        assert_eq!(rom, 0x7F as u32 * 0x4000);
    }

    #[test]
    fn rtc_latch_only_triggers_on_zero_to_one_transition() {
        let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
        let mut ram = MockRam::default();
        let mut rtc = MockRtc::default();
        state.step(0x6000, 0x00, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        state.step(0x6000, 0x01, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        assert!(state.rtc_latch);
        assert_eq!(rtc.triggers, 1);
        state.step(0x6000, 0x01, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        assert_eq!(rtc.triggers, 1);
        state.step(0x6000, 0x00, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        assert!(!state.rtc_latch);
        state.step(0x6000, 0x01, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        assert_eq!(rtc.triggers, 2);
    }

    #[test]
    fn crystal_save_round_trip_through_state_machine() {
        let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
        let mut ram = MockRam::default();
        let mut rtc = MockRtc::default();
        // First move ROM off bank 1 so subsequent writes propagate.
        state.step(0x2000, 0x05, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        state.step(0x0000, 0x0A, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        state.step(0x4000, 1, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        assert_eq!(ram_ptr, ptr_to_bank(&mut memory, 1));
        // Write the last byte of bank 1 (offset 0x1FFF). The previous
        // version of this test wrote at offset 0xBFFF, which is one byte
        // past the end of a 32 KiB SRAM buffer and silently UB'd.
        unsafe {
            *ram_ptr.add(0x1FFF) = 0xA5;
        }
        state.step(0x0000, 0x00, &mut rom, &mut ram_ptr, &mut memory, &mut ram, &mut rtc);
        assert!(!state.ram_enabled);
        assert_eq!(memory[1 * 0x2000 + 0x1FFF], 0xA5);
    }
}
