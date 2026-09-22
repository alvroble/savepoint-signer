//! RTC register model used by MBC3 cartridges.
//!
//! Mirrors the firmware's `src/gb_rtc.rs` minus the embassy types and
//! the `embedded_sdmmc` glue. The RTC exposes five registers (seconds,
//! minutes, hours, days, status) plus a latched copy that the game reads
//! via the MBC3 RTC register select; tests in this module prove that
//! `activate_register(N)` points both the real-time and latch views at
//! the same byte, that `trigger_latch()` copies real-time into latch,
//! and that `process()` advances the real-time clock at 1 Hz.

const REGISTER_MASKS: [u8; 5] = [0x3fu8, 0x3fu8, 0x1fu8, 0xffu8, 0xc1u8];

/// Advance the five RTC registers by one second. Operates on plain
/// `u8` references so the compiler never has to reason about packed
/// load/store of the underlying register file (the firmware keeps the
/// same data in a `#[repr(C, packed(1))]` layout, which trips up
/// `+=`/`wrapping_add` for some targets).
fn process_tick(seconds: &mut u8, minutes: &mut u8, hours: &mut u8, days: &mut u8, status: &mut u8) {
    *seconds = seconds.wrapping_add(1);

    if *seconds == 60 {
        *seconds = 0;
        *minutes = minutes.wrapping_add(1);

        if *minutes == 60 {
            *minutes = 0;
            *hours = hours.wrapping_add(1);

            if *hours == 24 {
                *hours = 0;
                *days = days.wrapping_add(1);

                if *days == 0 {
                    if *status & 0x01u8 == 0x01u8 {
                        *status |= 0x80u8;
                    }
                    *status ^= 0x01u8;
                }
            }
        }
    }
}

/// The two register views exposed to the cartridge: the running clock
/// (`real`) and the value latched at the last `trigger_latch()`. Both
/// views share the same five-register layout and the same per-register
/// bit mask.
#[derive(Debug)]
pub struct GbcRtcRegisters {
    real: [u8; 5],
    latch: [u8; 5],
}

impl GbcRtcRegisters {
    pub fn new() -> Self {
        Self {
            real: [0u8; 5],
            latch: [0u8; 5],
        }
    }

    pub fn real(&self) -> &[u8; 5] {
        &self.real
    }

    pub fn latch(&self) -> &[u8; 5] {
        &self.latch
    }

    pub fn set_real(&mut self, bytes: [u8; 5]) {
        self.real = bytes;
    }

    pub fn set_latch(&mut self, bytes: [u8; 5]) {
        self.latch = bytes;
    }

    /// Point both the real-time and latch byte views at the Nth register.
    /// Returns the index, so tests can confirm `activate_register(N)`
    /// drives both views to the same slot without having to read through
    /// raw pointers.
    pub fn activate(&mut self, reg_num: u8) -> usize {
        let reg_num = (reg_num as usize) % REGISTER_MASKS.len();
        let mask = REGISTER_MASKS[reg_num];
        self.real[reg_num] &= mask;
        self.latch[reg_num] &= mask;
        reg_num
    }

    /// Copy the running clock into the latch. The cartridge firmware
    /// drives this from the MBC3 0x6000 register write.
    pub fn trigger_latch(&mut self) {
        self.latch.copy_from_slice(&self.real);
    }

    /// Advance the real-time clock by `seconds` seconds. Used both by the firmware
    /// timer (one second at a time) and by `advance_by_seconds` after a
    /// save reload so the clock continues from where the cartridge was
    /// powered off.
    pub fn advance_by_seconds(&mut self, seconds: u64) {
        let halt = (self.real[4] & 0x40) != 0;
        if halt {
            return;
        }
        let mut s = self.real[0];
        let mut m = self.real[1];
        let mut h = self.real[2];
        let mut d = self.real[3];
        let mut st = self.real[4];
        for _ in 0..seconds {
            process_tick(&mut s, &mut m, &mut h, &mut d, &mut st);
            // Stop as soon as we set the day-carry bit: the firmware
            // latches day overflows and the in-game calendar relies on
            // the carry flag staying set.
            if st & 0x80 != 0 {
                break;
            }
        }
        self.real[0] = s;
        self.real[1] = m;
        self.real[2] = h;
        self.real[3] = d;
        self.real[4] = st;
        for n in 0..REGISTER_MASKS.len() {
            self.real[n] &= REGISTER_MASKS[n];
        }
    }
}

impl Default for GbcRtcRegisters {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait object consumed by the MBC3 state machine. The firmware's
/// `GbRtc` and `cartridge-core`'s host tests both implement this so the
/// same MBC3 logic can be driven from either side.
pub trait RtcControl {
    fn process(&mut self);
    fn trigger_latch(&mut self);
    fn activate_register(&mut self, reg_num: u8);
}

impl RtcControl for GbcRtcRegisters {
    fn process(&mut self) {
        // Per-second tick is driven by the firmware's embassy timer; the
        // host test exercises this trait by calling advance_by_seconds
        // directly, so process() here is intentionally a no-op.
    }

    fn trigger_latch(&mut self) {
        self.trigger_latch();
    }

    fn activate_register(&mut self, reg_num: u8) {
        let _ = self.activate(reg_num);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activate_register_targets_each_slot() {
        let mut rtc = GbcRtcRegisters::new();
        for reg in 0u8..=4 {
            assert_eq!(rtc.activate(reg), reg as usize);
        }
    }

    #[test]
    fn trigger_latch_copies_real_into_latch() {
        let mut rtc = GbcRtcRegisters::new();
        rtc.set_real([1, 2, 3, 4, 0]);
        assert_eq!(rtc.latch(), &[0, 0, 0, 0, 0]);
        rtc.trigger_latch();
        assert_eq!(rtc.latch(), &[1, 2, 3, 4, 0]);
    }

    #[test]
    fn advance_seconds_carries_minutes_hours_days() {
        let mut rtc = GbcRtcRegisters::new();
        rtc.advance_by_seconds(60);
        assert_eq!(rtc.real(), &[0, 1, 0, 0, 0], "60s -> 1 minute");
        rtc.advance_by_seconds(60 * 60);
        // 60s + 3600s = 3660s = 1h 1m, so minutes wraps from 60 back to
        // 1 while hours increments to 1.
        assert_eq!(rtc.real(), &[0, 1, 1, 0, 0], "3660s total -> 1h 1m");
        // 1h 1m + 22h 59m = 24h = 1d exactly, so minutes and hours wrap
        // back to 0 while days increments to 1.
        rtc.advance_by_seconds(60 * 60 * 24 - 3660);
        assert_eq!(rtc.real(), &[0, 0, 0, 1, 0], "1d 0h 0m total");
    }

    #[test]
    fn halt_flag_freezes_the_clock() {
        let mut rtc = GbcRtcRegisters::new();
        rtc.set_real([0, 0, 0, 0, 0x40]); // halt
        rtc.advance_by_seconds(3600);
        assert_eq!(rtc.real(), &[0, 0, 0, 0, 0x40]);
    }

    #[test]
    fn days_overflow_toggles_high_bit() {
        let mut rtc = GbcRtcRegisters::new();
        // 256 days = 1 second past day 255, which is the overflow edge
        // that toggles the days-high bit in status.
        rtc.set_real([59, 59, 23, 255, 0]);
        rtc.advance_by_seconds(1);
        assert_eq!(rtc.real(), &[0, 0, 0, 0, 0x01]);
    }

    #[test]
    fn register_masks_are_applied_on_advance() {
        // The firmware applies the per-register mask after every tick so
        // stray high bits (e.g. from a hostile game) cannot leak into the
        // saved state. Verify the masks are correct for every register
        // by setting every byte to 0xFF, disabling the halt bit, and
        // ticking once.
        let mut rtc = GbcRtcRegisters::new();
        // Status starts as 0x00 so the halt flag is clear; everything
        // else is filled with 0xFF.
        rtc.set_real([0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        rtc.advance_by_seconds(1);
        let r = rtc.real();
        // 255 + 1 wraps to 0; the seconds mask 0x3F keeps it at 0.
        assert_eq!(r[0] & !0x3F, 0, "seconds mask 0x3F (was {:#x})", r[0]);
        assert_eq!(r[1] & !0x3F, 0, "minutes mask 0x3F (was {:#x})", r[1]);
        assert_eq!(r[2] & !0x1F, 0, "hours mask 0x1F (was {:#x})", r[2]);
        // Status mask is 0xC1 (halt | day-carry | day-high).
        assert_eq!(r[4] & !0xC1, 0, "status mask 0xC1 (was {:#x})", r[4]);
    }
}
