//! Bounded PSBT import and signed-PSBT export using scoped FAT handles.
//!
//! The PSBT picker lists .psbt/.psb names, validates FAT long-name metadata,
//! and opens the selected original short-name entry. The diagnostic API still
//! reads UNSIGNED.PSB. Signed exports use SIGNED.PSB then SIGNED1..9.PSB and
//! never overwrite an existing file. Long-name writes are intentionally absent.
extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use embedded_sdmmc::{BlockDevice, Mode, ShortFileName, TimeSource, VolumeIdx, VolumeManager};

#[path = "sd_lfn.rs"]
mod lfn;

pub const MAX_PSBT_FILES: usize = 32;
const MAX_DIRECTORY_SLOTS: usize = 4096;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsbtFile {
    pub name: String,
    pub short_name: ShortFileName,
    pub size: u32,
}

/// Root-only, bounded listing. Valid LFN entries are associated by checksum;
/// the alias remains the authoritative open identifier.
pub fn list<
    D: BlockDevice,
    T: TimeSource,
    const DIRS: usize,
    const FILES: usize,
    const VOLUMES: usize,
>(
    manager: &mut VolumeManager<D, T, DIRS, FILES, VOLUMES>,
) -> Result<Vec<PsbtFile>, ReadError> {
    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| ReadError::Volume)?;
    let mut root = volume.open_root_dir().map_err(|_| ReadError::Directory)?;
    let mut files = Vec::new();
    let mut name = lfn::LongName::new();
    let mut slots = 0;
    let mut overflow = false;
    root.iterate_dir_raw(|raw, entry| {
        slots += 1;
        if slots > MAX_DIRECTORY_SLOTS {
            overflow = true;
            return false;
        }
        if raw[0] == 0xe5 {
            name.reset();
            return true;
        }
        if raw[11] == 0x0f {
            name.slot(raw);
            return true;
        }
        let long = name.finish(raw);
        if let Some(entry) = entry {
            if entry.attributes.is_directory()
                || entry.attributes.is_volume()
                || entry.attributes.is_hidden()
                || entry.attributes.is_system()
            {
                return true;
            }
            let alias = format!("{}", entry.name);
            let display = long.unwrap_or_else(|| alias.clone());
            if display.starts_with('.') {
                return true;
            }
            let ext = display.rsplit('.').next().unwrap_or("");
            if !ext.eq_ignore_ascii_case("psbt") && !ext.eq_ignore_ascii_case("psb") {
                return true;
            }
            if files.len() == MAX_PSBT_FILES {
                overflow = true;
                return false;
            }
            files.push(PsbtFile {
                name: display,
                short_name: entry.name.clone(),
                size: entry.size,
            });
        }
        true
    })
    .map_err(|_| ReadError::Directory)?;
    if overflow {
        return Err(ReadError::ListingLimit);
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

// The FAT adapter uses short names: a four-character .PSBT extension is invalid.
pub const IMPORT_FILENAME: &str = "UNSIGNED.PSB";
/// First export filename. Subsequent exports use `/SIGNED1.PSB` … `/SIGNED9.PSB`.
pub const EXPORT_FIRST: &str = "SIGNED.PSB";
/// Highest suffix index. After `/SIGNED9.PSB` is taken, the write errors
/// out with `WriteError::TooManyExports` rather than overwriting.
pub const EXPORT_MAX_INDEX: u32 = 9;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    ListingLimit,
    Volume,
    Directory,
    Missing,
    Open,
    Empty,
    TooLarge,
    Read,
    UnexpectedEof,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    Volume,
    Directory,
    Open,
    ShortName,
    TooManyExports,
    Empty,
    Write,
    Flush,
    Close,
}

pub fn read<
    D: BlockDevice,
    T: TimeSource,
    const DIRS: usize,
    const FILES: usize,
    const VOLUMES: usize,
>(
    manager: &mut VolumeManager<D, T, DIRS, FILES, VOLUMES>,
    max_bytes: usize,
) -> Result<Vec<u8>, ReadError> {
    read_named(manager, IMPORT_FILENAME, max_bytes)
}

pub fn read_named<
    D: BlockDevice,
    T: TimeSource,
    const DIRS: usize,
    const FILES: usize,
    const VOLUMES: usize,
>(
    manager: &mut VolumeManager<D, T, DIRS, FILES, VOLUMES>,
    alias: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, ReadError> {
    let sfn = ShortFileName::create_from_str(alias).map_err(|_| ReadError::Open)?;
    read_short(manager, &sfn, max_bytes)
}
pub fn read_short<
    D: BlockDevice,
    T: TimeSource,
    const DIRS: usize,
    const FILES: usize,
    const VOLUMES: usize,
>(
    manager: &mut VolumeManager<D, T, DIRS, FILES, VOLUMES>,
    sfn: &ShortFileName,
    max_bytes: usize,
) -> Result<Vec<u8>, ReadError> {
    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| ReadError::Volume)?;
    let mut root = volume.open_root_dir().map_err(|_| ReadError::Directory)?;
    let mut file = root
        .open_file_in_dir(sfn, Mode::ReadOnly)
        .map_err(|error| {
            if matches!(error, embedded_sdmmc::Error::NotFound) {
                ReadError::Missing
            } else {
                ReadError::Open
            }
        })?;
    let size = file.length() as usize;
    if size == 0 {
        return Err(ReadError::Empty);
    }
    if size > max_bytes {
        return Err(ReadError::TooLarge);
    }
    let mut bytes = alloc::vec![0; size];
    let mut offset = 0;
    while offset < size {
        let end = (offset + 256).min(size);
        let count = file
            .read(&mut bytes[offset..end])
            .map_err(|_| ReadError::Read)?;
        if count == 0 {
            return Err(ReadError::UnexpectedEof);
        }
        offset += count;
    }
    file.close().map_err(|_| ReadError::Close)?;
    // Root and volume close on scope exit, including all failure paths above.
    Ok(bytes)
}

/// Write `bytes` to the next free signed-PSBT slot on the SD root.
/// Returns the filename used so the caller can surface it to the user.
///
/// Filename selection:
///   - `/SIGNED.PSB` if missing.
///   - else `/SIGNED1.PSB`, `/SIGNED2.PSB`, …, `/SIGNED9.PSB` if missing.
///   - else `WriteError::TooManyExports`.
///
/// A previous export is never overwritten: this is a property of the
/// "no overwrite" rule the M2 plan requires. The plan explicitly says
/// "support safe retries of the same result" — retries of the SAME
/// sign use the same input PSBT and therefore produce the same signed
/// PSBT, which would overwrite. We trade that off for retention of all
/// historical exports during bench runs.
pub fn write_signed<
    D: BlockDevice,
    T: TimeSource,
    const DIRS: usize,
    const FILES: usize,
    const VOLUMES: usize,
>(
    manager: &mut VolumeManager<D, T, DIRS, FILES, VOLUMES>,
    bytes: &[u8],
) -> Result<String, WriteError> {
    if bytes.is_empty() {
        return Err(WriteError::Empty);
    }
    let mut volume = manager
        .open_volume(VolumeIdx(0))
        .map_err(|_| WriteError::Volume)?;
    let mut root = volume.open_root_dir().map_err(|_| WriteError::Directory)?;

    // Find the first free name. Read-only opens that fail with
    // NotFound mean the name is available; other failures mean we
    // should skip past that name (treat as occupied).
    let chosen: String = {
        let first = EXPORT_FIRST;
        let sfn_first = ShortFileName::create_from_str(first).map_err(|_| WriteError::ShortName)?;
        let free_first = matches!(
            root.open_file_in_dir(&sfn_first, Mode::ReadOnly),
            Err(embedded_sdmmc::Error::NotFound)
        );
        if free_first {
            String::from(first)
        } else {
            let mut picked: Option<String> = None;
            for i in 1..=EXPORT_MAX_INDEX {
                let candidate = format!("SIGNED{}.PSB", i);
                let sfn = match ShortFileName::create_from_str(&candidate) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let free = matches!(
                    root.open_file_in_dir(&sfn, Mode::ReadOnly),
                    Err(embedded_sdmmc::Error::NotFound)
                );
                if free {
                    picked = Some(candidate);
                    break;
                }
            }
            picked.ok_or(WriteError::TooManyExports)?
        }
    };
    let sfn = ShortFileName::create_from_str(&chosen).map_err(|_| WriteError::ShortName)?;
    let mut file = root
        .open_file_in_dir(&sfn, Mode::ReadWriteCreateOrTruncate)
        .map_err(|_| WriteError::Open)?;

    // File::write consumes the full slice; no partial-write semantics
    // on FAT. Loop in 256-byte chunks to keep peak memory bounded.
    let mut offset = 0;
    while offset < bytes.len() {
        let end = (offset + 256).min(bytes.len());
        file.write(&bytes[offset..end])
            .map_err(|_| WriteError::Write)?;
        offset = end;
    }
    file.flush().map_err(|_| WriteError::Flush)?;
    file.close().map_err(|_| WriteError::Close)?;
    Ok(chosen)
}
