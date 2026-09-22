/* RP2350 GameBoy cartridge
 * Copyright (C) 2025 Sebastian Quilitz
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation; either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

use embassy_rp::pio::{Instance, StateMachineRx};

use core::ptr::{self, NonNull};

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

pub trait Mbc {
    fn run(&mut self);
}

pub struct NoMbc {}

impl Mbc for NoMbc {
    fn run(&mut self) {
        loop {}
    }
}

pub struct Mbc1<'a, 'd, PIO: Instance, const SM: usize> {
    rx_fifo: &'a mut StateMachineRx<'d, PIO, SM>,
    current_rom_bank_pointer: NonNull<u32>,
    current_ram_bank_pointer: NonNull<*mut u8>,
    gb_ram_memory: &'a mut [u8],
    ram_control: &'a mut dyn MbcRamControl,
}

impl<'a, 'd, PIO: Instance, const SM: usize> Mbc1<'a, 'd, PIO, SM> {
    pub fn new(
        rx_fifo: &'a mut StateMachineRx<'d, PIO, SM>,
        current_rom_bank: *mut u32,
        ram_bank_pointer: *mut *mut u8,
        gb_ram_memory: &'a mut [u8],
        ram_control: &'a mut dyn MbcRamControl,
    ) -> Self {
        let current_rom_bank_pointer = NonNull::new(current_rom_bank).unwrap();
        let current_ram_bank_pointer = NonNull::new(ram_bank_pointer).unwrap();
        Self {
            rx_fifo,
            current_rom_bank_pointer,
            current_ram_bank_pointer,
            gb_ram_memory,
            ram_control,
        }
    }
}

impl<'a, 'd, PIO: Instance, const SM: usize> Mbc for Mbc1<'a, 'd, PIO, SM> {
    fn run(&mut self) {
        let mut rom_bank = 1u8;
        let mut rom_bank_new: u8;
        let mut rom_bank_high = 0u8;
        let mut rom_bank_low = 1u8;
        let mut mode = 0u8;
        let mut ram_enabled = false;

        let rom_bank_mask = 0x3Fu8;

        loop {
            while self.rx_fifo.empty() {}
            let addr = self.rx_fifo.pull();
            while self.rx_fifo.empty() {}
            let data = (self.rx_fifo.pull() & 0xFFu32) as u8;

            match addr & 0xE000u32 {
                0x0000u32 => {
                    let now_enabled = (data & 0x0F) == 0x0A;
                    if now_enabled {
                        self.ram_control.enable_ram_access();
                    } else {
                        self.ram_control.disable_ram_access();
                    }
                    cartridge_core::save_coordinator::publish_sram_transition(
                        ram_enabled,
                        now_enabled,
                    );
                    ram_enabled = now_enabled;
                }
                0x2000u32 => {
                    rom_bank_low = data & 0x1f;
                    if rom_bank_low == 0 {
                        rom_bank_low += 1;
                    }
                }
                0x4000u32 => {
                    if mode != 0 {
                        unsafe {
                            ptr::write_volatile(
                                self.current_ram_bank_pointer.as_ptr(),
                                self.gb_ram_memory
                                    .as_mut_ptr()
                                    .add((data & 0x03) as usize * 0x2000usize),
                            )
                        }
                    } else {
                        rom_bank_high = data & 0x03u8;
                    }
                }
                0x6000u32 => {
                    mode = data & 1u8;
                }
                _ => {}
            }

            if mode == 0 {
                rom_bank_new = (rom_bank_high << 5) | rom_bank_low;
            } else {
                rom_bank_new = rom_bank_low;
            }
            rom_bank_new = rom_bank_new & rom_bank_mask;

            if rom_bank != rom_bank_new {
                rom_bank = rom_bank_new;
                unsafe {
                    ptr::write_volatile(
                        self.current_rom_bank_pointer.as_ptr(),
                        rom_bank as u32 * 0x4000u32,
                    )
                };
            }
        }
    }
}

/// MBC3 cartridge state machine. Extracted from the legacy `Mbc3::run` loop
/// so the routing logic can be unit-tested with mock `MbcRamControl` /
/// `MbcRtcControl` implementations instead of requiring the real PIO/DMA
/// hardware. The cartridge firmware relies on every transition in this
/// state machine being correct, so host tests cover the SRAM enable/disable
/// sequencing, the four RAM banks, the RTC register selection, and the
/// ROM bank wrap-around from 0x00 to 0x01.
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
    const ROM_BANK_SIZE: usize = 0x4000;
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
                        ptr::write_volatile(
                            ram_bank_ptr,
                            gb_ram_memory.as_mut_ptr().add(
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
                ptr::write_volatile(rom_bank, self.rom_bank as u32 * Self::ROM_BANK_SIZE as u32);
            }
        }
    }
}

impl Default for Mbc3State {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Mbc3<'a, 'd, PIO: Instance, const SM: usize> {
    #[cfg(feature = "crystal-seed")]
    bank0: &'a mut [u8],
    #[cfg(feature = "crystal-seed")]
    seed_bus: crate::crystal_seed::Bus,
    rx_fifo: &'a mut StateMachineRx<'d, PIO, SM>,
    current_rom_bank_pointer: NonNull<u32>,
    current_ram_bank_pointer: NonNull<*mut u8>,
    gb_ram_memory: &'a mut [u8],
    ram_control: &'a mut dyn MbcRamControl,
    rtc_control: &'a mut dyn MbcRtcControl,
    state: Mbc3State,
}

impl<'a, 'd, PIO: Instance, const SM: usize> Mbc3<'a, 'd, PIO, SM> {
    pub fn new(
        rx_fifo: &'a mut StateMachineRx<'d, PIO, SM>,
        #[cfg(feature = "crystal-seed")] bank0: &'a mut [u8],
        current_rom_bank: *mut u32,
        ram_bank_pointer: *mut *mut u8,
        gb_ram_memory: &'a mut [u8],
        ram_control: &'a mut dyn MbcRamControl,
        rtc_control: &'a mut dyn MbcRtcControl,
    ) -> Self {
        rtc_control.process(); /* pre process RTC here */
        let current_rom_bank_pointer = NonNull::new(current_rom_bank).unwrap();
        let current_ram_bank_pointer = NonNull::new(ram_bank_pointer).unwrap();
        Self {
            #[cfg(feature = "crystal-seed")]
            seed_bus: crate::crystal_seed::Bus::new(bank0),
            #[cfg(feature = "crystal-seed")]
            bank0,
            rx_fifo,
            current_rom_bank_pointer,
            current_ram_bank_pointer,
            gb_ram_memory,
            ram_control,
            rtc_control,
            state: Mbc3State::new(),
        }
    }
}

impl<'a, 'd, PIO: Instance, const SM: usize> Mbc for Mbc3<'a, 'd, PIO, SM> {
    // Bank switches must not fetch instructions from flash while core 1 runs crypto.
    #[cfg_attr(feature = "crystal-seed", link_section = ".data.ram_func.mbc3")]
    #[inline(never)]
    fn run(&mut self) {
        loop {
            while self.rx_fifo.empty() {
                #[cfg(feature = "crystal-seed")]
                if crate::crystal_seed::computing() {
                    continue;
                }
                self.rtc_control.process();
            }
            let addr = self.rx_fifo.pull();
            while self.rx_fifo.empty() {}
            let data = (self.rx_fifo.pull() & 0xFFu32) as u8;

            #[cfg(feature = "crystal-seed")]
            {
                // The bank-select write is the critical path. Do not call the
                // mailbox or RTC code before publishing the DMA bank pointer.
                if addr & 0xe000 == 0x2000 {
                    let bank = (data & 0x7f).max(1);
                    self.state.rom_bank_pending = bank;
                    self.state.rom_bank = bank;
                    unsafe {
                        core::ptr::write_volatile(
                            self.current_rom_bank_pointer.as_ptr(),
                            u32::from(bank) * 0x4000,
                        );
                    }
                    continue;
                }
                if (0x7000..=0x7002).contains(&addr)
                    && self.seed_bus.write(addr, data, self.bank0)
                {
                    continue;
                }
            }
            let was_enabled = self.state.ram_enabled;
            self.state.step(
                addr,
                data,
                unsafe { self.current_rom_bank_pointer.as_mut() },
                unsafe { self.current_ram_bank_pointer.as_mut() },
                self.gb_ram_memory,
                self.ram_control,
                self.rtc_control,
            );

            cartridge_core::save_coordinator::publish_sram_transition(
                was_enabled,
                self.state.ram_enabled,
            );
        }
    }
}

pub struct Mbc5<'a, 'd, PIO: Instance, const SM: usize> {
    rx_fifo: &'a mut StateMachineRx<'d, PIO, SM>,
    current_rom_bank_pointer: NonNull<u32>,
    current_ram_bank_pointer: NonNull<*mut u8>,
    gb_ram_memory: &'a mut [u8],
    ram_control: &'a mut dyn MbcRamControl,
}

impl<'a, 'd, PIO: Instance, const SM: usize> Mbc5<'a, 'd, PIO, SM> {
    pub fn new(
        rx_fifo: &'a mut StateMachineRx<'d, PIO, SM>,
        current_rom_bank: *mut u32,
        ram_bank_pointer: *mut *mut u8,
        gb_ram_memory: &'a mut [u8],
        ram_control: &'a mut dyn MbcRamControl,
    ) -> Self {
        let current_rom_bank_pointer = NonNull::new(current_rom_bank).unwrap();
        let current_ram_bank_pointer = NonNull::new(ram_bank_pointer).unwrap();
        Self {
            rx_fifo,
            current_rom_bank_pointer,
            current_ram_bank_pointer,
            gb_ram_memory,
            ram_control,
        }
    }
}

impl<'a, 'd, PIO: Instance, const SM: usize> Mbc for Mbc5<'a, 'd, PIO, SM> {
    fn run(&mut self) {
        let mut rom_bank = 1u16;
        let mut rom_bank_new = 1u16;
        let mut ram_bank = 1u8;
        let mut ram_bank_new = 1u8;
        let mut ram_enabled = false;

        let rom_bank_mask = 0x1FFu16;
        let ram_bank_mask = 0x0Fu8;

        loop {
            while self.rx_fifo.empty() {}
            let addr = self.rx_fifo.pull();
            while self.rx_fifo.empty() {}
            let data = (self.rx_fifo.pull() & 0xFFu32) as u8;

            match addr & 0xF000u32 {
                0x0000u32 | 0x1000u32 => {
                    let now_enabled = (data & 0x0F) == 0x0A;
                    if now_enabled {
                        self.ram_control.enable_ram_access();
                    } else {
                        self.ram_control.disable_ram_access();
                    }
                    cartridge_core::save_coordinator::publish_sram_transition(
                        ram_enabled,
                        now_enabled,
                    );
                    ram_enabled = now_enabled;
                }
                0x2000u32 => {
                    rom_bank_new = (rom_bank & 0x0100) | data as u16;
                }
                0x3000u32 => {
                    rom_bank_new = (rom_bank & 0x00FF) | (((data as u16) << 8) & 0x0100);
                }
                0x4000u32 => {
                    ram_bank_new = data & 0x0F;
                }
                _ => {}
            }

            rom_bank_new = rom_bank_new & rom_bank_mask;
            ram_bank_new = ram_bank_new & ram_bank_mask;

            if rom_bank != rom_bank_new {
                rom_bank = rom_bank_new;
                unsafe {
                    ptr::write_volatile(
                        self.current_rom_bank_pointer.as_ptr(),
                        rom_bank as u32 * 0x4000u32,
                    )
                };
            }

            if ram_bank != ram_bank_new {
                ram_bank = ram_bank_new;

                unsafe {
                    ptr::write_volatile(
                        self.current_ram_bank_pointer.as_ptr(),
                        self.gb_ram_memory
                            .as_mut_ptr()
                            .add((ram_bank & 0x03) as usize * 0x2000usize),
                    )
                }
            }
        }
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
            state.step(
                0x0000,
                data,
                &mut rom,
                &mut ram_ptr,
                &mut memory,
                &mut ram,
                &mut rtc,
            );
            assert!(state.ram_enabled, "data {:#x} should enable SRAM", data);
            assert_eq!(ram.events, vec![RamEvent::EnableRam]);
        }
        // And the disable path: anything outside 0x0A disables.
        for data in [0x00u8, 0x01u8, 0x09u8, 0x0Bu8, 0xFFu8] {
            let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
            let mut ram = MockRam::default();
            let mut rtc = MockRtc::default();
            state.step(
                0x0000,
                data,
                &mut rom,
                &mut ram_ptr,
                &mut memory,
                &mut ram,
                &mut rtc,
            );
            assert!(!state.ram_enabled, "data {:#x} should disable SRAM", data);
            assert_eq!(ram.events, vec![RamEvent::DisableRam]);
        }
    }

    #[test]
    fn enable_with_rtc_bank_selects_rtc_path() {
        // Pre-select bank 0x08 (RTC seconds). Enabling SRAM after that
        // must route to enable_rtc_access, not enable_ram_access.
        let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
        let mut ram = MockRam::default();
        let mut rtc = MockRtc::default();
        state.step(
            0x4000,
            0x08,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        state.step(
            0x0000,
            0x0A,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        assert!(state.ram_enabled);
        assert_eq!(rtc.activated, vec![0]);
        assert_eq!(ram.events, vec![RamEvent::EnableRtc]);
    }

    #[test]
    fn disable_sram_after_enable() {
        let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
        let mut ram = MockRam::default();
        let mut rtc = MockRtc::default();
        state.step(
            0x0000,
            0x0A,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        state.step(
            0x0000,
            0x00,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        assert!(!state.ram_enabled);
        assert_eq!(ram.events, vec![RamEvent::EnableRam, RamEvent::DisableRam]);
    }

    #[test]
    fn ram_bank_pointer_is_set_for_every_normal_bank() {
        for bank in 0u8..=3 {
            let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
            let mut ram = MockRam::default();
            let mut rtc = MockRtc::default();
            state.step(
                0x4000,
                bank,
                &mut rom,
                &mut ram_ptr,
                &mut memory,
                &mut ram,
                &mut rtc,
            );
            assert_eq!(state.ram_bank, bank);
            assert_eq!(ram_ptr, ptr_to_bank(&mut memory, bank as usize));
            // SRAM is still disabled (we never wrote 0x0A to $0000), so no
            // enable call should happen here.
            assert_eq!(ram.events, Vec::new());
        }
    }

    #[test]
    fn ram_bank_pointer_is_set_when_sram_already_enabled() {
        let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
        let mut ram = MockRam::default();
        let mut rtc = MockRtc::default();
        // 1) enable SRAM
        state.step(
            0x0000,
            0x0A,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        // 2) pick bank 2 — must re-enable after the bank switch
        state.step(
            0x4000,
            2,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        assert_eq!(state.ram_bank, 2);
        assert_eq!(ram_ptr, ptr_to_bank(&mut memory, 2));
        assert_eq!(ram.events, vec![RamEvent::EnableRam, RamEvent::EnableRam]);
    }

    #[test]
    fn rtc_register_is_activated_for_each_bank_8_to_c() {
        for (raw_bank, expected_reg) in [(0x08u8, 0u8), (0x09, 1), (0x0A, 2), (0x0B, 3), (0x0C, 4)]
        {
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
            // Bank pointer must not be touched in RTC mode.
            assert!(ram_ptr.is_null());
        }
    }

    #[test]
    fn rom_bank_wrap_zero_to_one() {
        let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
        let mut ram = MockRam::default();
        let mut rtc = MockRtc::default();
        state.step(
            0x2000,
            0x00,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        assert_eq!(state.rom_bank, 1);
        assert_eq!(rom, 0x4000u32);
    }

    #[test]
    fn rom_bank_truncated_to_seven_bits() {
        let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
        let mut ram = MockRam::default();
        let mut rtc = MockRtc::default();
        state.step(
            0x2000,
            0xFF,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        assert_eq!(state.rom_bank, 0x7F);
        assert_eq!(rom, 0x7F as u32 * 0x4000);
    }

    #[test]
    fn rtc_latch_only_triggers_on_zero_to_one_transition() {
        let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
        let mut ram = MockRam::default();
        let mut rtc = MockRtc::default();
        // 1) 0 -> 1: latch triggers once
        state.step(
            0x6000,
            0x00,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        state.step(
            0x6000,
            0x01,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        assert!(state.rtc_latch);
        assert_eq!(rtc.triggers, 1);
        // 2) another 1: latch stays armed but does NOT re-trigger
        state.step(
            0x6000,
            0x01,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        assert_eq!(rtc.triggers, 1);
        // 3) back to 0: latch disarms
        state.step(
            0x6000,
            0x00,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        assert!(!state.rtc_latch);
        // 4) 0 -> 1 again: re-triggers
        state.step(
            0x6000,
            0x01,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        assert_eq!(rtc.triggers, 2);
    }

    #[test]
    fn crystal_save_round_trip_through_state_machine() {
        // Drive the exact sequence Crystal uses for an in-game save:
        // enable SRAM, pick bank 1, write a sentinel, then disable.
        let (mut state, mut rom, mut ram_ptr, mut memory) = new_state();
        let mut ram = MockRam::default();
        let mut rtc = MockRtc::default();
        state.step(
            0x0000,
            0x0A,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        state.step(
            0x4000,
            1,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        assert_eq!(ram_ptr, ptr_to_bank(&mut memory, 1));
        // Sentinel write happens via DMA into the now-exposed bank. We
        // simulate the last byte the engine writes by hand.
        unsafe {
            *ram_ptr.add(0xBFFF) = 0xA5;
        }
        state.step(
            0x0000,
            0x00,
            &mut rom,
            &mut ram_ptr,
            &mut memory,
            &mut ram,
            &mut rtc,
        );
        assert!(!state.ram_enabled);
        assert_eq!(memory[1 * 0x2000 + 0x1FFF], 0xA5);
    }
}
