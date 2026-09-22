//! Automatic save persistence decisions.
//!
//! The Game Boy expects SRAM writes to be instantaneous. SD card writes are
//! not, so core 0 only publishes activity from the MBC handler and core 1
//! decides when a coherent snapshot may be flushed.
//!
//! Design:
//! * MBC1/MBC3/MBC5 call [`publish_sram_transition`] on every SRAM enable
//!   change. A disable after enable sets [`SAVE_DIRTY`] and bumps
//!   [`SAVE_GENERATION`].
//! * [`AutoSaveEngine`] is the object the firmware worker actually polls.
//!   It restarts a quiet period on every new generation so a save session
//!   that toggles SRAM several times produces one flush, not one per toggle.
//! * The engine never treats enable/disable alone as "the user saved".
//!   Identical SRAM (title-screen read-only access) is skipped via CRC.
//! * Persistence is a separate step: the worker writes the live SRAM only
//!   after SRAM is disabled and the quiet period has elapsed. A 500 ms
//!   quiet window is far longer than an in-flight DMA beat, so we do not
//!   need a second 32 KiB copy to prove coherence.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Set by an MBC handler on enabled → disabled. Cleared by the worker.
pub static SAVE_DIRTY: AtomicBool = AtomicBool::new(false);

/// Bumped on every enabled → disabled transition.
pub static SAVE_GENERATION: AtomicU32 = AtomicU32::new(0);

/// Mirrors the live MBC SRAM-enable flag so the worker can refuse a flush
/// while the Game Boy still has the SRAM window mapped.
pub static SRAM_ENABLED: AtomicBool = AtomicBool::new(false);

/// Default coalesce window after the last SRAM disable.
pub const QUIET_PERIOD_MS: u64 = 500;

/// Publish an SRAM enable-state change from an MBC handler.
///
/// Only the enabled → disabled edge is treated as activity. Games also
/// enable SRAM to *read* (Crystal's title screen checks for a save). The
/// worker still wakes on that edge, then skips the SD write if the SRAM
/// CRC matches the last durable image.
pub fn publish_sram_transition(was_enabled: bool, now_enabled: bool) {
    SRAM_ENABLED.store(now_enabled, Ordering::Release);
    if was_enabled && !now_enabled {
        SAVE_GENERATION.fetch_add(1, Ordering::AcqRel);
        SAVE_DIRTY.store(true, Ordering::Release);
    }
}

/// Decision returned by [`AutoSaveEngine::poll`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistDecision {
    /// Nothing waiting, or a flush is already in flight.
    Idle,
    /// There is a pending generation but SRAM is currently mapped.
    WaitForDisable,
    /// Pending generation exists; wait until `quiet_period_ms` after the
    /// last activity before capturing.
    WaitForQuiet,
    /// SRAM is disabled and the quiet period has elapsed. The worker may
    /// write this generation.
    Persist { generation: u32 },
}

/// In-memory bookkeeping for the core-1 save worker.
///
/// The MBC publishes into the static atomics; the worker copies those
/// values in through [`AutoSaveEngine::poll`]. Tests drive the same
/// methods the firmware calls.
#[derive(Debug, Clone, Copy)]
pub struct AutoSaveEngine {
    pending_generation: u32,
    disk_generation: u32,
    last_activity_ms: u64,
    last_crc: Option<u32>,
    in_flight: bool,
    quiet_period_ms: u64,
}

impl AutoSaveEngine {
    pub const fn new(quiet_period_ms: u64) -> Self {
        Self {
            pending_generation: 0,
            disk_generation: 0,
            last_activity_ms: 0,
            last_crc: None,
            in_flight: false,
            quiet_period_ms,
        }
    }

    pub const fn standard() -> Self {
        Self::new(QUIET_PERIOD_MS)
    }

    /// Record MBC activity. Restarts the quiet period so a burst of
    /// enable/disable (one in-game Save session) collapses to a single
    /// flush.
    pub fn ingest_dirty(&mut self, global_generation: u32, now_ms: u64) {
        if global_generation > self.pending_generation {
            self.pending_generation = global_generation;
        }
        self.last_activity_ms = now_ms;
    }

    /// Firmware worker entry point. `dirty` is the value swapped out of
    /// [`SAVE_DIRTY`]; `global_generation` is loaded from
    /// [`SAVE_GENERATION`]; `sram_disabled` is the inverse of
    /// [`SRAM_ENABLED`].
    pub fn poll(
        &mut self,
        dirty: bool,
        global_generation: u32,
        sram_disabled: bool,
        now_ms: u64,
    ) -> PersistDecision {
        if dirty {
            self.ingest_dirty(global_generation, now_ms);
        }
        self.decide(sram_disabled, now_ms)
    }

    pub fn decide(&self, sram_disabled: bool, now_ms: u64) -> PersistDecision {
        if self.in_flight {
            return PersistDecision::Idle;
        }
        if self.pending_generation <= self.disk_generation {
            return PersistDecision::Idle;
        }
        if !sram_disabled {
            return PersistDecision::WaitForDisable;
        }
        if now_ms.saturating_sub(self.last_activity_ms) < self.quiet_period_ms {
            return PersistDecision::WaitForQuiet;
        }
        PersistDecision::Persist {
            generation: self.pending_generation,
        }
    }

    /// Mark a persist attempt so [`decide`] does not start another until
    /// success or failure is reported.
    pub fn begin_persist(&mut self) {
        self.in_flight = true;
    }

    /// Skip the SD write when SRAM has not changed since the last durable
    /// image (title-screen bookkeeping, in-game reads).
    pub fn should_skip_identical(&self, crc: u32) -> bool {
        self.last_crc == Some(crc)
    }

    pub fn on_success(&mut self, generation: u32, crc: u32) {
        if generation > self.disk_generation {
            self.disk_generation = generation;
        }
        self.last_crc = Some(crc);
        self.in_flight = false;
    }

    /// Failed open/write/close. Clears in-flight so the next poll retries
    /// the same pending generation.
    pub fn on_failure(&mut self) {
        self.in_flight = false;
    }

    /// Boot-time restore finished before reset release. Seeds CRC skip so
    /// the title screen's SRAM read does not rewrite the card.
    pub fn on_restored(&mut self, crc: u32) {
        self.last_crc = Some(crc);
        self.disk_generation = self.pending_generation;
    }

    pub fn needs_save(&self) -> bool {
        self.pending_generation > self.disk_generation
    }

    pub fn pending_generation(&self) -> u32 {
        self.pending_generation
    }

    pub fn disk_generation(&self) -> u32 {
        self.disk_generation
    }
}

impl Default for AutoSaveEngine {
    fn default() -> Self {
        Self::standard()
    }
}

// ---------------------------------------------------------------------------
// CRC32 (IEEE 802.3 polynomial 0xEDB88320). Used to skip identical SRAM.
// ---------------------------------------------------------------------------

const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut j = 0;
        while j < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            j += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

pub fn crc32(buf: &[u8]) -> u32 {
    let mut c: u32 = 0xFFFF_FFFF;
    for &b in buf {
        c = CRC32_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mbc3::{Mbc3State, MbcRamControl, MbcRtcControl};
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn exclusive() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn reset_atomics() {
        SAVE_DIRTY.store(false, Ordering::Release);
        SAVE_GENERATION.store(0, Ordering::Release);
        SRAM_ENABLED.store(false, Ordering::Release);
    }

    struct MockRam;
    impl MbcRamControl for MockRam {
        fn enable_ram_access(&mut self) {}
        fn disable_ram_access(&mut self) {}
        fn enable_rtc_access(&mut self) {}
    }
    struct MockRtc;
    impl MbcRtcControl for MockRtc {
        fn process(&mut self) {}
        fn trigger_latch(&mut self) {}
        fn activate_register(&mut self, _reg_num: u8) {}
    }

    fn mbc3_write(
        state: &mut Mbc3State,
        addr: u32,
        data: u8,
        rom: &mut u32,
        ram_ptr: &mut *mut u8,
        memory: &mut [u8],
    ) {
        let was = state.ram_enabled;
        let mut ram = MockRam;
        let mut rtc = MockRtc;
        state.step(addr, data, rom, ram_ptr, memory, &mut ram, &mut rtc);
        publish_sram_transition(was, state.ram_enabled);
    }

    #[test]
    fn crc32_matches_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(&[]), 0);
    }

    #[test]
    fn worker_stays_idle_without_ingesting_mbc_generation() {
        // Reproduction of the original bug: MBC bumps SAVE_GENERATION but
        // the worker never copies it into the coordinator, so decide() is
        // Idle forever.
        let engine = AutoSaveEngine::standard();
        assert_eq!(
            engine.decide(true, 10_000),
            PersistDecision::Idle,
            "an engine that never saw MBC activity must not persist"
        );
    }

    #[test]
    fn mbc3_activity_reaches_engine_without_a_button() {
        let _g = exclusive();
        reset_atomics();

        let mut state = Mbc3State::new();
        let mut rom = 0u32;
        let mut ram_ptr = core::ptr::null_mut();
        let mut memory = [0u8; 0x8000];

        mbc3_write(&mut state, 0x0000, 0x0A, &mut rom, &mut ram_ptr, &mut memory);
        assert!(SRAM_ENABLED.load(Ordering::Acquire));
        assert!(!SAVE_DIRTY.load(Ordering::Acquire));

        mbc3_write(&mut state, 0x0000, 0x00, &mut rom, &mut ram_ptr, &mut memory);
        assert!(!SRAM_ENABLED.load(Ordering::Acquire));
        assert!(SAVE_DIRTY.load(Ordering::Acquire), "disable must wake the worker");
        assert_eq!(SAVE_GENERATION.load(Ordering::Acquire), 1);

        let mut engine = AutoSaveEngine::standard();
        let dirty = SAVE_DIRTY.swap(false, Ordering::AcqRel);
        let gen = SAVE_GENERATION.load(Ordering::Acquire);
        let disabled = !SRAM_ENABLED.load(Ordering::Acquire);

        assert_eq!(
            engine.poll(dirty, gen, disabled, 1_000),
            PersistDecision::WaitForQuiet
        );
        assert_eq!(
            engine.poll(false, gen, true, 1_000 + QUIET_PERIOD_MS - 1),
            PersistDecision::WaitForQuiet
        );
        assert_eq!(
            engine.poll(false, gen, true, 1_000 + QUIET_PERIOD_MS),
            PersistDecision::Persist { generation: 1 }
        );
    }

    #[test]
    fn quiet_period_restarts_on_new_activity() {
        let mut engine = AutoSaveEngine::standard();
        engine.ingest_dirty(1, 0);
        assert_eq!(engine.decide(true, 400), PersistDecision::WaitForQuiet);

        engine.ingest_dirty(2, 400);
        assert_eq!(
            engine.decide(true, 700),
            PersistDecision::WaitForQuiet,
            "activity at t=400 must push the deadline to t=900"
        );
        assert_eq!(
            engine.decide(true, 900),
            PersistDecision::Persist { generation: 2 }
        );
    }

    #[test]
    fn refuses_to_persist_while_sram_is_mapped() {
        let mut engine = AutoSaveEngine::standard();
        engine.ingest_dirty(1, 0);
        assert_eq!(engine.decide(false, 1_000), PersistDecision::WaitForDisable);
        assert_eq!(
            engine.decide(true, 1_000),
            PersistDecision::Persist { generation: 1 }
        );
    }

    #[test]
    fn new_activity_during_in_flight_write_retries_after_success() {
        let mut engine = AutoSaveEngine::standard();
        engine.ingest_dirty(1, 0);
        assert!(matches!(
            engine.decide(true, 500),
            PersistDecision::Persist { generation: 1 }
        ));
        engine.begin_persist();
        assert_eq!(engine.decide(true, 600), PersistDecision::Idle);

        engine.ingest_dirty(2, 600);
        engine.on_success(1, 0x1111);
        assert!(engine.needs_save());
        assert_eq!(
            engine.decide(true, 600 + QUIET_PERIOD_MS),
            PersistDecision::Persist { generation: 2 }
        );
    }

    #[test]
    fn new_activity_during_snapshot_window_invalidates_quiet() {
        let mut engine = AutoSaveEngine::standard();
        engine.ingest_dirty(1, 0);
        // About to persist...
        assert!(matches!(engine.decide(true, 500), PersistDecision::Persist { .. }));
        // Game re-enabled SRAM (new session) before the worker captured.
        engine.ingest_dirty(2, 500);
        assert_eq!(engine.decide(false, 500), PersistDecision::WaitForDisable);
        assert_eq!(engine.decide(true, 500), PersistDecision::WaitForQuiet);
        assert_eq!(
            engine.decide(true, 1_000),
            PersistDecision::Persist { generation: 2 }
        );
    }

    #[test]
    fn retry_after_open_write_close_failure() {
        let mut engine = AutoSaveEngine::standard();
        engine.ingest_dirty(1, 0);
        engine.begin_persist();
        engine.on_failure();
        assert!(engine.needs_save());
        assert_eq!(
            engine.decide(true, 500),
            PersistDecision::Persist { generation: 1 },
            "failure must leave the generation pending"
        );

        engine.begin_persist();
        engine.on_success(1, 0xABCD);
        assert!(!engine.needs_save());
        assert_eq!(engine.decide(true, 1_000), PersistDecision::Idle);
    }

    #[test]
    fn first_save_then_subsequent_save() {
        let mut engine = AutoSaveEngine::standard();
        engine.ingest_dirty(1, 0);
        engine.begin_persist();
        engine.on_success(1, 1);
        assert_eq!(engine.disk_generation(), 1);

        engine.ingest_dirty(2, 1_000);
        assert_eq!(
            engine.decide(true, 1_000 + QUIET_PERIOD_MS),
            PersistDecision::Persist { generation: 2 }
        );
        engine.begin_persist();
        engine.on_success(2, 2);
        assert_eq!(engine.disk_generation(), 2);
        assert!(!engine.needs_save());
    }

    #[test]
    fn restart_with_existing_persisted_generation() {
        let mut engine = AutoSaveEngine::standard();
        // Boot: SRAM already restored, CRC seeded, no pending write.
        engine.on_restored(0xFEED);
        assert!(!engine.needs_save());
        assert!(engine.should_skip_identical(0xFEED));
        assert!(!engine.should_skip_identical(0xBEEF));

        // Title screen enables+disables SRAM with identical contents.
        engine.ingest_dirty(1, 0);
        engine.begin_persist();
        assert!(engine.should_skip_identical(0xFEED));
        engine.on_success(1, 0xFEED);
        assert!(!engine.needs_save());
    }

    #[test]
    fn skip_identical_sram_avoids_sd_write() {
        let mut engine = AutoSaveEngine::standard();
        engine.on_restored(crc32(&[1, 2, 3]));
        engine.ingest_dirty(1, 0);
        engine.begin_persist();
        assert!(engine.should_skip_identical(crc32(&[1, 2, 3])));
        engine.on_success(1, crc32(&[1, 2, 3]));
        // A real in-game save changes SRAM.
        engine.ingest_dirty(2, 1_000);
        engine.begin_persist();
        assert!(!engine.should_skip_identical(crc32(&[9, 9, 9])));
    }

    #[test]
    fn equal_rtc_timestamp_does_not_block_next_generation() {
        // Generation bookkeeping is independent of wall-clock equality.
        let mut engine = AutoSaveEngine::standard();
        engine.ingest_dirty(1, 0);
        engine.begin_persist();
        engine.on_success(1, 0);
        engine.ingest_dirty(2, 0);
        assert_eq!(
            engine.decide(true, QUIET_PERIOD_MS),
            PersistDecision::Persist { generation: 2 }
        );
    }
}
