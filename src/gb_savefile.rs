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

use defmt::debug;
use embassy_time::Instant;
use embedded_sdmmc::{BlockDevice, Directory, Error as SdmmcError, TimeSource};
use rtcc::DateTimeAccess;

use crate::rom_info::RomInfo;
use cartridge_core::savefile::{
    persist_atomic, restore_best_in_place, DirectorySaveIo, RestoreSource,
    RtcSaveStateProvider, SavefileError,
};

pub trait GbRtcSaveStateProvider {
    fn retrieve_register_state(&self) -> ([u8; 5], [u8; 5]);
    fn restore_register_state(&mut self, regs: ([u8; 5], [u8; 5]));
    fn advance_by_seconds(&mut self, seconds: u64);
}

struct RtcBridge<'a>(&'a mut dyn GbRtcSaveStateProvider);

impl RtcSaveStateProvider for RtcBridge<'_> {
    fn retrieve_register_state(&self) -> ([u8; 5], [u8; 5]) {
        self.0.retrieve_register_state()
    }
    fn restore_register_state(&mut self, regs: ([u8; 5], [u8; 5])) {
        self.0.restore_register_state(regs)
    }
    fn advance_by_seconds(&mut self, seconds: u64) {
        self.0.advance_by_seconds(seconds)
    }
}

#[derive(Clone, Debug)]
pub enum GbSavefileError<E1, E2>
where
    E1: ::core::fmt::Debug,
    E2: ::core::fmt::Debug,
{
    Sdmmc(SdmmcError<E1>),
    Timesource(E2),
}

impl<E1, E2> From<SavefileError<E1>> for GbSavefileError<E1, E2>
where
    E1: ::core::fmt::Debug,
    E2: ::core::fmt::Debug,
{
    fn from(value: SavefileError<E1>) -> Self {
        match value {
            SavefileError::Sdmmc(e) => GbSavefileError::Sdmmc(e),
        }
    }
}

pub struct GbSavefile<
    'a,
    'd,
    D,
    T,
    const MAX_DIRS: usize,
    const MAX_FILES: usize,
    const MAX_VOLUMES: usize,
    DTAError,
> where
    D: BlockDevice,
    T: TimeSource,
    DTAError: core::fmt::Debug,
{
    directory: &'a mut Directory<'d, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>,
    rom_info: &'a RomInfo,
    timesource: &'a mut dyn DateTimeAccess<Error = DTAError>,
    rtc_state_provider: &'a mut dyn GbRtcSaveStateProvider,
}

impl<
        'a,
        'd,
        D,
        T,
        const MAX_DIRS: usize,
        const MAX_FILES: usize,
        const MAX_VOLUMES: usize,
        DTAError,
        E,
    > GbSavefile<'a, 'd, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES, DTAError>
where
    D: BlockDevice<Error = E>,
    T: TimeSource,
    E: ::core::fmt::Debug,
    DTAError: core::fmt::Debug,
{
    pub fn new(
        directory: &'a mut Directory<'d, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>,
        rom_info: &'a RomInfo,
        timesource: &'a mut dyn DateTimeAccess<Error = DTAError>,
        rtc_state_provider: &'a mut dyn GbRtcSaveStateProvider,
    ) -> Self {
        Self {
            directory,
            rom_info,
            timesource,
            rtc_state_provider,
        }
    }

    fn wall_clock_unix(&mut self) -> Result<u64, GbSavefileError<E, DTAError>> {
        let time = self
            .timesource
            .datetime()
            .map_err(GbSavefileError::Timesource)?;
        Ok(time.and_utc().timestamp() as u64)
    }

    /// Wall clock minus time already applied by the in-RAM RTC since
    /// boot, so we do not double-count elapsed seconds on restore.
    fn restore_unix(&mut self) -> Result<u64, GbSavefileError<E, DTAError>> {
        let now = self.wall_clock_unix()?;
        Ok(now.saturating_sub(Instant::now().as_secs()))
    }

    /// Synchronous restore used by the bootloader while the Game Boy is
    /// held in reset. Tries `.SAV` then `.TMP`. Missing files return
    /// `Ok(None)` so the caller can zero SRAM; IO errors are distinct.
    pub fn restore(
        &mut self,
        saveram_memory: &mut [u8],
    ) -> Result<Option<RestoreSource>, GbSavefileError<E, DTAError>> {
        let now_unix = self.restore_unix()?;
        let mut io = DirectorySaveIo::new(self.directory);
        let mut rtc = RtcBridge(self.rtc_state_provider);
        restore_best_in_place(
            &mut io,
            self.rom_info.savefile.as_str(),
            self.rom_info.has_rtc,
            saveram_memory,
            &mut rtc,
            now_unix,
        )
        .map_err(GbSavefileError::from)
    }

    pub fn load(
        &mut self,
        saveram_memory: &mut [u8],
    ) -> Result<Option<RestoreSource>, GbSavefileError<E, DTAError>> {
        let result = self.restore(saveram_memory)?;
        match &result {
            Some(src) => debug!("restored save from {:?}", defmt::Debug2Format(src)),
            None => debug!("no complete save file present"),
        }
        Ok(result)
    }

    /// Automatic persist: write `.TMP` then `.SAV`. Not durable until
    /// both write and close succeed.
    pub fn store(&mut self, saveram_memory: &[u8]) -> Result<(), GbSavefileError<E, DTAError>> {
        let now_unix = self.wall_clock_unix()?;
        let mut io = DirectorySaveIo::new(self.directory);
        let rtc = RtcBridge(self.rtc_state_provider);
        persist_atomic(
            &mut io,
            self.rom_info.savefile.as_str(),
            self.rom_info.has_rtc,
            saveram_memory,
            &rtc,
            now_unix,
        )
        .map_err(GbSavefileError::from)
    }
}
