//! Save-file persistence for cartridge SRAM + optional RTC state.
//!
//! This is the format the bootloader already restores: `<basename>.SAV`
//! containing raw SRAM followed by an optional 48-byte RTC tail. Automatic
//! persistence writes the same image to `<basename>.TMP` first, then to
//! `.SAV`. A power loss during the `.SAV` rewrite leaves a complete `.TMP`
//! that boot will load; a power loss during `.TMP` leaves the previous
//! `.SAV` intact.
//!
//! `File::write` in the vendored embedded-sdmmc 0.8 API returns
//! `Result<(), Error>`, not a byte count. There is no short-write-count
//! bug to paper over. Close errors are failures: a generation is not
//! durable until both write and close succeed.
//!
//! Power-loss window (honest): software cannot promise survival if power
//! disappears after the Game Boy has finished its own SAVE and before the
//! SD controller has accepted the last block plus the directory update.
//! The quiet period plus TMP-then-SAV ordering only protects the previous
//! complete image, not an in-flight write.

use arrayvec::ArrayString;
use embedded_sdmmc::{BlockDevice, Directory, Error as SdmmcError, Mode, TimeSource};

use crate::rom_info::RomInfo;

pub const RTC_TAIL_LEN: usize = 48;

/// Storage interface used by both the FAT-backed firmware path and host
/// mocks. Firmware and tests call [`persist_atomic`] / [`restore_best`]
/// against this trait so failure injection exercises the same code.
pub trait SaveIo {
    type Error: core::fmt::Debug;

    /// Create-or-truncate `name` and write SRAM followed by an optional
    /// RTC tail. Must not return `Ok` unless the data is committed as far
    /// as this IO layer can guarantee (for FAT: write *and* close).
    fn write_save(
        &mut self,
        name: &str,
        sram: &[u8],
        rtc_tail: Option<&[u8; RTC_TAIL_LEN]>,
    ) -> Result<(), Self::Error>;

    /// Open `name`. `Ok(None)` means the name does not exist. `Ok(Some(len))`
    /// fills `sram` (up to `sram.len()` bytes) and `rtc_tail` when the file
    /// is long enough. Other errors are real IO failures.
    fn read_save(
        &mut self,
        name: &str,
        sram: &mut [u8],
        rtc_tail: &mut [u8; RTC_TAIL_LEN],
    ) -> Result<Option<u32>, Self::Error>;
}

pub trait RtcSaveStateProvider {
    fn retrieve_register_state(&self) -> ([u8; 5], [u8; 5]);
    fn restore_register_state(&mut self, regs: ([u8; 5], [u8; 5]));
    fn advance_by_seconds(&mut self, seconds: u64);
}

#[derive(Clone, Debug)]
pub enum SavefileError<E>
where
    E: core::fmt::Debug,
{
    Sdmmc(SdmmcError<E>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreSource {
    Primary,
    Temp,
}

/// 8.3 sibling of `primary` with a `.TMP` extension. Strips spaces so a
/// padded basename can never produce `CRYSTAL .TMP`.
pub fn temp_save_name(primary: &str) -> ArrayString<12> {
    let raw = primary.split('.').next().unwrap_or(primary).as_bytes();
    let mut out = ArrayString::<12>::new();
    for &b in raw.iter().take(8) {
        if b == b' ' || b == 0 {
            break;
        }
        let _ = out.try_push(char::from(b.to_ascii_uppercase()));
    }
    let _ = out.try_push_str(".TMP");
    out
}

pub fn expected_len(sram_len: usize, has_rtc: bool) -> usize {
    sram_len + if has_rtc { RTC_TAIL_LEN } else { 0 }
}

pub fn is_full_save(file_len: u32, sram_len: usize, has_rtc: bool) -> bool {
    file_len as usize >= expected_len(sram_len, has_rtc)
}

pub fn is_sram_complete(file_len: u32, sram_len: usize) -> bool {
    file_len as usize >= sram_len
}

pub fn pack_rtc(real: &[u8; 5], latch: &[u8; 5], unix: u64, out: &mut [u8; RTC_TAIL_LEN]) {
    for i in 0..5 {
        out[i * 4..(i + 1) * 4].copy_from_slice(&(real[i] as u32).to_le_bytes());
        out[20 + i * 4..20 + (i + 1) * 4].copy_from_slice(&(latch[i] as u32).to_le_bytes());
    }
    out[40..48].copy_from_slice(&unix.to_le_bytes());
}

pub fn unpack_rtc(buf: &[u8; RTC_TAIL_LEN]) -> ([u8; 5], [u8; 5], u64) {
    let mut real = [0u8; 5];
    let mut latch = [0u8; 5];
    for i in 0..5 {
        real[i] = u32::from_le_bytes(buf[i * 4..(i + 1) * 4].try_into().unwrap()) as u8;
        latch[i] = u32::from_le_bytes(buf[20 + i * 4..20 + (i + 1) * 4].try_into().unwrap()) as u8;
    }
    let unix = u64::from_le_bytes(buf[40..48].try_into().unwrap());
    (real, latch, unix)
}

/// Apply a packed RTC tail. Equal timestamps are a no-op advance (immediate
/// reboot). A saved timestamp in the future of `now_unix` keeps the
/// registers and does not fail: discarding SRAM because the wall clock
/// went backwards was wiping valid games.
pub fn apply_rtc_tail(
    tail: &[u8; RTC_TAIL_LEN],
    now_unix: u64,
    rtc: &mut dyn RtcSaveStateProvider,
) {
    let (real, latch, saved) = unpack_rtc(tail);
    rtc.restore_register_state((real, latch));
    if now_unix >= saved {
        let diff = now_unix - saved;
        if diff > 0 {
            rtc.advance_by_seconds(diff);
        }
    }
}

fn rtc_tail_from_provider(
    rtc: &dyn RtcSaveStateProvider,
    now_unix: u64,
) -> [u8; RTC_TAIL_LEN] {
    let (real, latch) = rtc.retrieve_register_state();
    let mut tail = [0u8; RTC_TAIL_LEN];
    pack_rtc(&real, &latch, now_unix, &mut tail);
    tail
}

/// Write TMP, then the primary `.SAV`. The previous complete `.SAV` is
/// retained if TMP fails. If `.SAV` fails after TMP succeeds, boot loads
/// TMP.
pub fn persist_atomic<I: SaveIo>(
    io: &mut I,
    primary: &str,
    has_rtc: bool,
    sram: &[u8],
    rtc: &dyn RtcSaveStateProvider,
    now_unix: u64,
) -> Result<(), I::Error> {
    let tmp = temp_save_name(primary);
    let tail = if has_rtc {
        Some(rtc_tail_from_provider(rtc, now_unix))
    } else {
        None
    };
    let tail_ref = tail.as_ref();
    io.write_save(tmp.as_str(), sram, tail_ref)?;
    io.write_save(primary, sram, tail_ref)?;
    Ok(())
}

pub fn pick_restore_slot(
    primary_len: Option<u32>,
    tmp_len: Option<u32>,
    sram_len: usize,
    has_rtc: bool,
) -> Option<RestoreSource> {
    let primary_full = primary_len
        .map(|n| is_full_save(n, sram_len, has_rtc))
        .unwrap_or(false);
    let tmp_full = tmp_len
        .map(|n| is_full_save(n, sram_len, has_rtc))
        .unwrap_or(false);
    if primary_full {
        return Some(RestoreSource::Primary);
    }
    if tmp_full {
        return Some(RestoreSource::Temp);
    }
    let primary_sram = primary_len
        .map(|n| is_sram_complete(n, sram_len))
        .unwrap_or(false);
    let tmp_sram = tmp_len
        .map(|n| is_sram_complete(n, sram_len))
        .unwrap_or(false);
    if primary_sram {
        return Some(RestoreSource::Primary);
    }
    if tmp_sram {
        return Some(RestoreSource::Temp);
    }
    None
}

/// Restore SRAM (and RTC when present) from the newest complete image.
/// Lengths are probed with a zero-byte read so a truncated `.SAV` cannot
/// clobber live SRAM before we fall back to `.TMP`. Returns `Ok(None)`
/// when nothing usable exists — the caller zeros SRAM. A future RTC
/// timestamp is not fatal.
pub fn restore_best_in_place<I: SaveIo>(
    io: &mut I,
    primary: &str,
    has_rtc: bool,
    sram: &mut [u8],
    rtc: &mut dyn RtcSaveStateProvider,
    now_unix: u64,
) -> Result<Option<RestoreSource>, I::Error> {
    let tmp_name = temp_save_name(primary);
    let sram_len = sram.len();
    let mut unused_sram_probe = [];
    let mut unused_tail = [0u8; RTC_TAIL_LEN];

    // Length-only probe: read 0 SRAM bytes so we do not clobber `sram`
    // until a slot is chosen. IO implementations must still return the
    // real file length.
    let primary_len = match io.read_save(primary, &mut unused_sram_probe, &mut unused_tail) {
        Ok(v) => v,
        Err(_) => None,
    };
    let tmp_len = match io.read_save(tmp_name.as_str(), &mut unused_sram_probe, &mut unused_tail) {
        Ok(v) => v,
        Err(_) => None,
    };

    let pick = pick_restore_slot(primary_len, tmp_len, sram_len, has_rtc);
    let Some(source) = pick else {
        return Ok(None);
    };
    let name = match source {
        RestoreSource::Primary => primary,
        RestoreSource::Temp => tmp_name.as_str(),
    };
    let mut tail = [0u8; RTC_TAIL_LEN];
    match io.read_save(name, sram, &mut tail)? {
        Some(len) if is_sram_complete(len, sram_len) => {
            if has_rtc && len as usize >= expected_len(sram_len, true) {
                apply_rtc_tail(&tail, now_unix, rtc);
            }
            Ok(Some(source))
        }
        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// FAT directory adapter used by the firmware and by host FAT tests.
// ---------------------------------------------------------------------------

pub struct DirectorySaveIo<'a, 'd, D, T, const MAX_DIRS: usize, const MAX_FILES: usize, const MAX_VOLUMES: usize>
where
    D: BlockDevice,
    T: TimeSource,
{
    directory: &'a mut Directory<'d, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>,
}

impl<'a, 'd, D, T, const MAX_DIRS: usize, const MAX_FILES: usize, const MAX_VOLUMES: usize>
    DirectorySaveIo<'a, 'd, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>
where
    D: BlockDevice,
    T: TimeSource,
{
    pub fn new(
        directory: &'a mut Directory<'d, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>,
    ) -> Self {
        Self { directory }
    }
}

impl<'a, 'd, D, T, const MAX_DIRS: usize, const MAX_FILES: usize, const MAX_VOLUMES: usize> SaveIo
    for DirectorySaveIo<'a, 'd, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>
where
    D: BlockDevice,
    T: TimeSource,
    D::Error: core::fmt::Debug,
{
    type Error = SavefileError<D::Error>;

    fn write_save(
        &mut self,
        name: &str,
        sram: &[u8],
        rtc_tail: Option<&[u8; RTC_TAIL_LEN]>,
    ) -> Result<(), Self::Error> {
        let mut file = self
            .directory
            .open_file_in_dir(name, Mode::ReadWriteCreateOrTruncate)
            .map_err(SavefileError::Sdmmc)?;
        let write_result = (|| {
            if !sram.is_empty() {
                file.write(sram).map_err(SavefileError::Sdmmc)?;
            }
            if let Some(tail) = rtc_tail {
                file.write(tail.as_slice()).map_err(SavefileError::Sdmmc)?;
            }
            Ok(())
        })();
        match write_result {
            Ok(()) => file.close().map_err(SavefileError::Sdmmc),
            Err(e) => {
                let _ = file.close();
                Err(e)
            }
        }
    }

    fn read_save(
        &mut self,
        name: &str,
        sram: &mut [u8],
        rtc_tail: &mut [u8; RTC_TAIL_LEN],
    ) -> Result<Option<u32>, Self::Error> {
        let mut file = match self
            .directory
            .open_file_in_dir(name, Mode::ReadOnly)
        {
            Ok(f) => f,
            Err(SdmmcError::NotFound) => return Ok(None),
            Err(e) => return Err(SavefileError::Sdmmc(e)),
        };
        let len = file.length();
        let result = (|| {
            if !sram.is_empty() && len > 0 {
                let want = core::cmp::min(sram.len(), len as usize);
                let n = file.read(&mut sram[..want]).map_err(SavefileError::Sdmmc)?;
                if n < want {
                    return Err(SavefileError::Sdmmc(SdmmcError::EndOfFile));
                }
            }
            if len as usize >= sram.len() + RTC_TAIL_LEN {
                let n = file.read(rtc_tail.as_mut_slice()).map_err(SavefileError::Sdmmc)?;
                if n != RTC_TAIL_LEN {
                    return Err(SavefileError::Sdmmc(SdmmcError::EndOfFile));
                }
            }
            Ok(())
        })();
        let close = file.close().map_err(SavefileError::Sdmmc);
        result?;
        close?;
        Ok(Some(len))
    }
}

/// Convenience wrapper kept so existing FAT round-trip tests (and the
/// firmware, which already constructs this type) share the helpers above.
pub struct Savefile<
    'a,
    'd,
    D,
    T,
    const MAX_DIRS: usize,
    const MAX_FILES: usize,
    const MAX_VOLUMES: usize,
> where
    D: BlockDevice,
    T: TimeSource,
{
    io: DirectorySaveIo<'a, 'd, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>,
    rom_info: &'a RomInfo,
    now_unix: u64,
    rtc_state_provider: &'a mut dyn RtcSaveStateProvider,
}

impl<
        'a,
        'd,
        D,
        T,
        const MAX_DIRS: usize,
        const MAX_FILES: usize,
        const MAX_VOLUMES: usize,
    > Savefile<'a, 'd, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>
where
    D: BlockDevice,
    T: TimeSource,
    D::Error: core::fmt::Debug,
{
    pub fn new(
        directory: &'a mut Directory<'d, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>,
        rom_info: &'a RomInfo,
        now_unix: u64,
        rtc_state_provider: &'a mut dyn RtcSaveStateProvider,
    ) -> Self {
        Self {
            io: DirectorySaveIo::new(directory),
            rom_info,
            now_unix,
            rtc_state_provider,
        }
    }

    pub fn persist(&mut self, saveram_memory: &[u8]) -> Result<(), SavefileError<D::Error>> {
        persist_atomic(
            &mut self.io,
            self.rom_info.savefile.as_str(),
            self.rom_info.has_rtc,
            saveram_memory,
            self.rtc_state_provider,
            self.now_unix,
        )
    }

    pub fn restore(
        &mut self,
        saveram_memory: &mut [u8],
    ) -> Result<Option<RestoreSource>, SavefileError<D::Error>> {
        restore_best_in_place(
            &mut self.io,
            self.rom_info.savefile.as_str(),
            self.rom_info.has_rtc,
            saveram_memory,
            self.rtc_state_provider,
            self.now_unix,
        )
    }

    /// Load the primary `.SAV` only. Prefer [`Self::restore`] at boot.
    pub fn load(
        &mut self,
        saveram_memory: &mut [u8],
    ) -> Result<(), SavefileError<D::Error>> {
        match self.restore(saveram_memory)? {
            Some(_) => Ok(()),
            None => Err(SavefileError::Sdmmc(SdmmcError::NotFound)),
        }
    }

    /// Persist TMP then SAV. This is the firmware write path.
    pub fn store(
        &mut self,
        saveram_memory: &[u8],
    ) -> Result<(), SavefileError<D::Error>> {
        self.persist(saveram_memory)
    }
}

// ---------------------------------------------------------------------------
// Host tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rom_info::{MbcType, RomInfo};
    use crate::save_coordinator::AutoSaveEngine;
    use embedded_sdmmc::{
        Block, BlockCount, BlockIdx, ShortFileName, TimeSource, VolumeIdx,
        VolumeManager,
    };
    use std::collections::BTreeMap;
    use std::string::String;
    use std::sync::{Arc, Mutex};
    use std::vec::Vec;

    const BLOCK_SIZE: usize = 512;
    const BLOCK_COUNT: u32 = 4 * 1024 * 1024;
    const PARTITION_START_BLOCK: u32 = 2048;

    #[derive(Clone)]
    struct MemBlockDevice {
        blocks: Arc<Mutex<BTreeMap<u32, [u8; BLOCK_SIZE]>>>,
    }

    impl MemBlockDevice {
        fn new() -> Self {
            Self {
                blocks: Arc::new(Mutex::new(BTreeMap::new())),
            }
        }
    }

    impl BlockDevice for MemBlockDevice {
        type Error = std::convert::Infallible;

        fn read(
            &self,
            blocks: &mut [Block],
            start_block_idx: BlockIdx,
            _reason: &str,
        ) -> Result<(), Self::Error> {
            let storage = self.blocks.lock().unwrap();
            for (i, block) in blocks.iter_mut().enumerate() {
                let idx = start_block_idx.0 + i as u32;
                if let Some(data) = storage.get(&idx) {
                    block.contents.copy_from_slice(data);
                } else {
                    block.contents.fill(0);
                }
            }
            Ok(())
        }

        fn write(
            &self,
            blocks: &[Block],
            start_block_idx: BlockIdx,
        ) -> Result<(), Self::Error> {
            let mut storage = self.blocks.lock().unwrap();
            for (i, block) in blocks.iter().enumerate() {
                let idx = start_block_idx.0 + i as u32;
                storage.insert(idx, block.contents);
            }
            Ok(())
        }

        fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
            Ok(BlockCount(BLOCK_COUNT))
        }
    }

    fn install_partition_table(blk: &MemBlockDevice) {
        let mut storage = blk.blocks.lock().unwrap();
        let reserved_sectors: u16 = 32;
        let fat_size: u32 = 128;

        let mut mbr = [0u8; BLOCK_SIZE];
        mbr[510] = 0x55;
        mbr[511] = 0xAA;
        mbr[446 + 4] = 0x0C;
        mbr[446 + 8..446 + 12].copy_from_slice(&PARTITION_START_BLOCK.to_le_bytes());
        mbr[446 + 12..446 + 16]
            .copy_from_slice(&(BLOCK_COUNT - PARTITION_START_BLOCK).to_le_bytes());
        storage.insert(0, mbr);

        let mut bpb = [0u8; BLOCK_SIZE];
        bpb[0..8].copy_from_slice(b"MSDOS5.0");
        let bytes_per_sector: u16 = BLOCK_SIZE as u16;
        bpb[11..13].copy_from_slice(&bytes_per_sector.to_le_bytes());
        bpb[13] = 8;
        bpb[14..16].copy_from_slice(&reserved_sectors.to_le_bytes());
        bpb[16] = 2;
        let total_sectors = (BLOCK_COUNT - PARTITION_START_BLOCK) as u32;
        bpb[32..36].copy_from_slice(&total_sectors.to_le_bytes());
        bpb[36..40].copy_from_slice(&fat_size.to_le_bytes());
        bpb[42..44].copy_from_slice(&0u16.to_le_bytes());
        bpb[44..48].copy_from_slice(&2u32.to_le_bytes());
        bpb[48..50].copy_from_slice(&1u16.to_le_bytes());
        bpb[50..52].copy_from_slice(&6u16.to_le_bytes());
        bpb[64] = 0x80;
        bpb[66] = 0x29;
        bpb[67..71].copy_from_slice(&0x12345678u32.to_le_bytes());
        bpb[71..82].copy_from_slice(b"NO RTC     ");
        bpb[82..90].copy_from_slice(b"FAT32   ");
        bpb[510] = 0x55;
        bpb[511] = 0xAA;
        storage.insert(PARTITION_START_BLOCK, bpb);

        let mut fat = [0u8; BLOCK_SIZE];
        let end_of_chain = 0x0FFFFFF8u32;
        fat[0..4].copy_from_slice(&0xFFFFFFF8u32.to_le_bytes());
        fat[4..8].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
        fat[8..12].copy_from_slice(&end_of_chain.to_le_bytes());
        for copy in 0..2 {
            storage.insert(
                PARTITION_START_BLOCK + reserved_sectors as u32 + copy * fat_size,
                fat,
            );
        }

        let mut info = [0u8; BLOCK_SIZE];
        info[0..4].copy_from_slice(&0x41615252u32.to_le_bytes());
        info[484..488].copy_from_slice(&0x61417272u32.to_le_bytes());
        info[488..492].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
        info[508..512].copy_from_slice(&0xAA550000u32.to_le_bytes());
        storage.insert(PARTITION_START_BLOCK + 1, info);
    }

    struct ZeroTime;
    impl TimeSource for ZeroTime {
        fn get_timestamp(&self) -> embedded_sdmmc::Timestamp {
            embedded_sdmmc::Timestamp {
                year_since_1970: 56,
                zero_indexed_month: 0,
                zero_indexed_day: 0,
                hours: 0,
                minutes: 0,
                seconds: 0,
            }
        }
    }

    fn open_volume() -> VolumeManager<MemBlockDevice, ZeroTime, 4, 4, 1> {
        let blk = MemBlockDevice::new();
        install_partition_table(&blk);
        VolumeManager::new(blk, ZeroTime)
    }

    fn rom_info(name: &str, banks: u8, has_rtc: bool) -> RomInfo {
        RomInfo {
            ram_bank_count: banks,
            rom_bank_count: 128,
            has_rtc,
            mbc: MbcType::Mbc3,
            savefile: ArrayString::from(name).unwrap(),
        }
    }

    struct DummyRtc {
        real: [u8; 5],
        latch: [u8; 5],
        advanced: u64,
    }

    impl DummyRtc {
        fn new(real: [u8; 5]) -> Self {
            Self {
                real,
                latch: real,
                advanced: 0,
            }
        }
    }

    impl RtcSaveStateProvider for DummyRtc {
        fn retrieve_register_state(&self) -> ([u8; 5], [u8; 5]) {
            (self.real, self.latch)
        }
        fn restore_register_state(&mut self, regs: ([u8; 5], [u8; 5])) {
            self.real = regs.0;
            self.latch = regs.1;
        }
        fn advance_by_seconds(&mut self, seconds: u64) {
            self.advanced += seconds;
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum MockError {
        Open,
        Write,
        Close,
    }

    struct MockFs {
        files: BTreeMap<String, Vec<u8>>,
        fail_open: bool,
        fail_write: bool,
        fail_close: bool,
        writes: Vec<String>,
    }

    impl MockFs {
        fn new() -> Self {
            Self {
                files: BTreeMap::new(),
                fail_open: false,
                fail_write: false,
                fail_close: false,
                writes: Vec::new(),
            }
        }
    }

    impl SaveIo for MockFs {
        type Error = MockError;

        fn write_save(
            &mut self,
            name: &str,
            sram: &[u8],
            rtc_tail: Option<&[u8; RTC_TAIL_LEN]>,
        ) -> Result<(), Self::Error> {
            self.writes.push(String::from(name));
            if self.fail_open {
                return Err(MockError::Open);
            }
            if self.fail_write {
                return Err(MockError::Write);
            }
            if self.fail_close {
                return Err(MockError::Close);
            }
            let mut data = sram.to_vec();
            if let Some(tail) = rtc_tail {
                data.extend_from_slice(tail);
            }
            self.files.insert(String::from(name), data);
            Ok(())
        }

        fn read_save(
            &mut self,
            name: &str,
            sram: &mut [u8],
            rtc_tail: &mut [u8; RTC_TAIL_LEN],
        ) -> Result<Option<u32>, Self::Error> {
            if self.fail_open {
                return Err(MockError::Open);
            }
            let Some(data) = self.files.get(name) else {
                return Ok(None);
            };
            let n = core::cmp::min(sram.len(), data.len());
            sram[..n].copy_from_slice(&data[..n]);
            if data.len() >= sram.len() + RTC_TAIL_LEN {
                rtc_tail.copy_from_slice(&data[sram.len()..sram.len() + RTC_TAIL_LEN]);
            }
            Ok(Some(data.len() as u32))
        }
    }

    // -- filenames ----------------------------------------------------------

    #[test]
    fn fat_parser_rejects_space_padded_journal_names() {
        // Reproduction of the original bug: shorten_basename padded to 8
        // with spaces, producing "CRYSTAL .J1".
        assert!(ShortFileName::create_from_str("CRYSTAL .J1").is_err());
        assert!(ShortFileName::create_from_str("CRYSTAL .SAV").is_err());
    }

    #[test]
    fn filenames_for_1_7_and_8_char_basenames_are_valid_8_3() {
        for (primary, tmp) in [
            ("A.SAV", "A.TMP"),
            ("CRYSTAL.SAV", "CRYSTAL.TMP"),
            ("ABCDEFG.SAV", "ABCDEFG.TMP"),
            ("ABCDEFGH.SAV", "ABCDEFGH.TMP"),
        ] {
            assert!(
                ShortFileName::create_from_str(primary).is_ok(),
                "primary {primary}"
            );
            assert_eq!(temp_save_name(primary).as_str(), tmp);
            assert!(
                ShortFileName::create_from_str(tmp).is_ok(),
                "temp {tmp}"
            );
        }
    }

    #[test]
    fn temp_name_strips_padding_spaces() {
        assert_eq!(temp_save_name("CRYSTAL .SAV").as_str(), "CRYSTAL.TMP");
    }

    // -- restore slot pick --------------------------------------------------

    #[test]
    fn truncated_primary_falls_back_to_complete_temp() {
        let sram_len = 32 * 1024;
        assert_eq!(
            pick_restore_slot(Some(100), Some((sram_len + 48) as u32), sram_len, true),
            Some(RestoreSource::Temp)
        );
    }

    #[test]
    fn complete_primary_wins_over_complete_temp() {
        let sram_len = 8192;
        let full = expected_len(sram_len, true) as u32;
        assert_eq!(
            pick_restore_slot(Some(full), Some(full), sram_len, true),
            Some(RestoreSource::Primary)
        );
    }

    #[test]
    fn missing_both_slots_is_none() {
        assert_eq!(pick_restore_slot(None, None, 8192, false), None);
        assert_eq!(pick_restore_slot(Some(10), Some(10), 8192, false), None);
    }

    #[test]
    fn corrupt_or_short_read_does_not_select_slot_for_overwrite() {
        // Persist always writes TMP first, then SAV. A read error on SAV
        // must not cause us to treat TMP as missing and smash it; restore
        // simply prefers whichever slot is complete.
        let sram_len = 100;
        assert_eq!(
            pick_restore_slot(None, Some(100), sram_len, false),
            Some(RestoreSource::Temp)
        );
    }

    // -- mock persist / restore ---------------------------------------------

    fn fill_pattern(n: usize) -> Vec<u8> {
        (0..n).map(|i| ((i * 17 + 3) & 0xFF) as u8).collect()
    }

    #[test]
    fn persist_then_restore_preserves_every_sram_byte() {
        let mut fs = MockFs::new();
        let sram = fill_pattern(0x8000);
        let rtc = DummyRtc::new([1, 2, 3, 4, 0]);
        persist_atomic(&mut fs, "CRYSTAL.SAV", true, &sram, &rtc, 1_700_000_000).unwrap();
        assert!(fs.files.contains_key("CRYSTAL.SAV"));
        assert!(fs.files.contains_key("CRYSTAL.TMP"));
        assert_eq!(fs.writes, ["CRYSTAL.TMP", "CRYSTAL.SAV"]);

        let mut loaded = vec![0u8; 0x8000];
        let mut rtc2 = DummyRtc::new([0; 5]);
        let src = restore_best_in_place(
            &mut fs,
            "CRYSTAL.SAV",
            true,
            &mut loaded,
            &mut rtc2,
            1_700_000_010,
        )
        .unwrap();
        assert_eq!(src, Some(RestoreSource::Primary));
        assert_eq!(loaded, sram);
        assert_eq!(rtc2.real, [1, 2, 3, 4, 0]);
        assert_eq!(rtc2.advanced, 10);
    }

    #[test]
    fn restore_completes_before_reset_release() {
        let mut fs = MockFs::new();
        let sram = fill_pattern(8192);
        let mut rtc = DummyRtc::new([0; 5]);
        persist_atomic(&mut fs, "A.SAV", false, &sram, &rtc, 10).unwrap();

        let mut live = vec![0u8; 8192];
        let src =
            restore_best_in_place(&mut fs, "A.SAV", false, &mut live, &mut rtc, 10).unwrap();
        assert_eq!(src, Some(RestoreSource::Primary));
        assert_eq!(live, sram, "SRAM must be restored before reset would be released");
        assert_eq!(live[0], sram[0]);
        assert_eq!(live[8191], sram[8191]);
    }

    #[test]
    fn legacy_sav_without_tmp_is_imported() {
        let mut fs = MockFs::new();
        let sram = fill_pattern(8192);
        fs.files.insert("CRYSTAL.SAV".into(), sram.clone());
        let mut loaded = vec![0u8; 8192];
        let mut rtc = DummyRtc::new([0; 5]);
        let src = restore_best_in_place(
            &mut fs,
            "CRYSTAL.SAV",
            false,
            &mut loaded,
            &mut rtc,
            0,
        )
        .unwrap();
        assert_eq!(src, Some(RestoreSource::Primary));
        assert_eq!(loaded, sram);
    }

    #[test]
    fn interrupted_sav_update_recovers_tmp() {
        let mut fs = MockFs::new();
        let old = vec![0x11u8; 8192];
        let new = vec![0x22u8; 8192];
        let mut rtc = DummyRtc::new([0; 5]);
        persist_atomic(&mut fs, "G.SAV", false, &old, &rtc, 1).unwrap();
        // Simulate TMP written with the new image and SAV truncated.
        fs.files.insert("G.TMP".into(), new.clone());
        fs.files.insert("G.SAV".into(), vec![0x22u8; 100]);

        let mut loaded = vec![0u8; 8192];
        let src =
            restore_best_in_place(&mut fs, "G.SAV", false, &mut loaded, &mut rtc, 2).unwrap();
        assert_eq!(src, Some(RestoreSource::Temp));
        assert_eq!(loaded, new);
    }

    #[test]
    fn ram_sizes_including_rtc_only_and_128k() {
        let mut rtc = DummyRtc::new([9, 0, 0, 0, 0]);
        for (banks, has_rtc) in [(0u8, true), (1, false), (4, true), (16, false)] {
            let mut fs = MockFs::new();
            let n = banks as usize * 0x2000;
            let sram = fill_pattern(n.max(1))[..n].to_vec();
            persist_atomic(&mut fs, "GAME.SAV", has_rtc, &sram, &rtc, 50).unwrap();
            let mut loaded = vec![0xFFu8; n];
            let src = restore_best_in_place(
                &mut fs,
                "GAME.SAV",
                has_rtc,
                &mut loaded,
                &mut rtc,
                50,
            )
            .unwrap();
            assert_eq!(src, Some(RestoreSource::Primary), "banks={banks}");
            assert_eq!(loaded, sram, "banks={banks}");
        }
    }

    #[test]
    fn retry_after_open_write_close_failure_uses_same_persist() {
        let sram = fill_pattern(256);
        let mut rtc = DummyRtc::new([0; 5]);
        let mut engine = AutoSaveEngine::new(0);
        engine.ingest_dirty(1, 0);
        engine.begin_persist();

        let mut fs = MockFs::new();
        fs.fail_open = true;
        assert_eq!(
            persist_atomic(&mut fs, "X.SAV", false, &sram, &rtc, 1),
            Err(MockError::Open)
        );
        engine.on_failure();

        fs.fail_open = false;
        fs.fail_write = true;
        engine.begin_persist();
        assert_eq!(
            persist_atomic(&mut fs, "X.SAV", false, &sram, &rtc, 1),
            Err(MockError::Write)
        );
        engine.on_failure();

        fs.fail_write = false;
        fs.fail_close = true;
        engine.begin_persist();
        assert_eq!(
            persist_atomic(&mut fs, "X.SAV", false, &sram, &rtc, 1),
            Err(MockError::Close)
        );
        assert!(
            !fs.files.contains_key("X.SAV"),
            "close failure must not mark the file durable"
        );
        engine.on_failure();

        fs.fail_close = false;
        engine.begin_persist();
        persist_atomic(&mut fs, "X.SAV", false, &sram, &rtc, 1).unwrap();
        engine.on_success(1, 0);
        assert!(!engine.needs_save());

        let mut loaded = vec![0u8; 256];
        restore_best_in_place(&mut fs, "X.SAV", false, &mut loaded, &mut rtc, 1).unwrap();
        assert_eq!(loaded, sram);
    }

    #[test]
    fn equal_rtc_timestamp_keeps_sram_and_does_not_advance() {
        let mut tail = [0u8; RTC_TAIL_LEN];
        pack_rtc(&[5, 0, 0, 0, 0], &[5, 0, 0, 0, 0], 1000, &mut tail);
        let mut rtc = DummyRtc::new([0; 5]);
        apply_rtc_tail(&tail, 1000, &mut rtc);
        assert_eq!(rtc.real[0], 5);
        assert_eq!(rtc.advanced, 0);
    }

    #[test]
    fn future_timestamp_keeps_sram_without_advance() {
        let mut tail = [0u8; RTC_TAIL_LEN];
        pack_rtc(&[7, 0, 0, 0, 0], &[7, 0, 0, 0, 0], 2000, &mut tail);
        let mut rtc = DummyRtc::new([0; 5]);
        apply_rtc_tail(&tail, 1000, &mut rtc);
        assert_eq!(rtc.real[0], 7, "registers must still be restored");
        assert_eq!(rtc.advanced, 0);
    }

    #[test]
    fn elapsed_time_is_applied_on_restore() {
        let mut tail = [0u8; RTC_TAIL_LEN];
        pack_rtc(&[0; 5], &[0; 5], 100, &mut tail);
        let mut rtc = DummyRtc::new([0; 5]);
        apply_rtc_tail(&tail, 250, &mut rtc);
        assert_eq!(rtc.advanced, 150);
    }

    // -- FAT-backed round trip (same Directory adapter the firmware uses) --

    #[test]
    fn fat_persist_restore_round_trip_32k_with_rtc() {
        let mut mgr = open_volume();
        let mut volume = mgr.open_volume(VolumeIdx(0)).unwrap();
        let mut root = volume.open_root_dir().unwrap();
        let info = rom_info("CRYSTAL.SAV", 4, true);
        let mut rtc = DummyRtc::new([10, 20, 5, 100, 0]);
        let saveram = fill_pattern(0x8000);

        {
            let mut sf = Savefile::new(&mut root, &info, 1_700_000_000, &mut rtc);
            sf.persist(&saveram).expect("persist");
        }

        let mut loaded = vec![0u8; 0x8000];
        let mut rtc2 = DummyRtc::new([0; 5]);
        let mut sf = Savefile::new(&mut root, &info, 1_700_001_000, &mut rtc2);
        let src = sf.restore(&mut loaded).expect("restore");
        assert_eq!(src, Some(RestoreSource::Primary));
        assert_eq!(loaded, saveram);
        assert_eq!(rtc2.real, [10, 20, 5, 100, 0]);
        assert_eq!(rtc2.advanced, 1000);
    }

    #[test]
    fn fat_accepts_1_7_8_char_names() {
        let mut mgr = open_volume();
        let mut volume = mgr.open_volume(VolumeIdx(0)).unwrap();
        let mut root = volume.open_root_dir().unwrap();
        for name in ["A.SAV", "CRYSTAL.SAV", "ABCDEFGH.SAV"] {
            let info = rom_info(name, 1, false);
            let mut rtc = DummyRtc::new([0; 5]);
            let sram = fill_pattern(0x2000);
            let mut sf = Savefile::new(&mut root, &info, 1, &mut rtc);
            sf.persist(&sram)
                .unwrap_or_else(|e| panic!("persist {name}: {e:?}"));
            let mut loaded = vec![0u8; 0x2000];
            sf.restore(&mut loaded)
                .unwrap_or_else(|e| panic!("restore {name}: {e:?}"));
            assert_eq!(loaded, sram, "{name}");
        }
    }
}
