#[path = "../../src/sd_psbt.rs"]
mod reader;

use embedded_sdmmc::{
    Block, BlockCount, BlockDevice, BlockIdx, ShortFileName, TimeSource, Timestamp, VolumeManager,
};
use std::{cell::Cell, rc::Rc};

// Sparse, synthetic FAT32 disk: partition LBA 1, 32 reserved sectors,
// one 600-sector FAT, root cluster 2, file starting at cluster 3.
// Supports reads always; writes only when `writable` is true. Writes
// overwrite sector 634+ (file data clusters) and store into a backing
// map keyed by BlockIdx.
struct Disk {
    size: usize,
    present: bool,
    fail_data: Rc<Cell<bool>>,
    writable: bool,
    written: Rc<std::cell::RefCell<std::collections::HashMap<u32, Block>>>,
}
impl BlockDevice for Disk {
    type Error = ();
    fn num_blocks(&self) -> Result<BlockCount, ()> {
        Ok(BlockCount(70001))
    }
    fn write(&self, blocks: &[Block], start: BlockIdx) -> Result<(), ()> {
        assert!(self.writable, "read-only disk received write");
        let mut map = self.written.borrow_mut();
        for (i, block) in blocks.iter().enumerate() {
            map.insert(start.0 + i as u32, block.clone());
        }
        Ok(())
    }
    fn read(&self, blocks: &mut [Block], start: BlockIdx, _: &str) -> Result<(), ()> {
        let map = self.written.borrow();
        for (i, block) in blocks.iter_mut().enumerate() {
            let sector = start.0 as usize + i;
            let b = &mut block.contents;
            // Writable disk consults the in-memory map first.
            if let Some(stored) = map.get(&(sector as u32)) {
                b.copy_from_slice(&stored.contents);
                continue;
            }
            b.fill(0);
            match sector {
                0 => {
                    b[450] = 0x0c;
                    b[454..458].copy_from_slice(&1u32.to_le_bytes());
                    b[458..462].copy_from_slice(&70000u32.to_le_bytes());
                    b[510..512].copy_from_slice(&[0x55, 0xaa]);
                }
                1 => {
                    b[0..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
                    b[11..13].copy_from_slice(&512u16.to_le_bytes());
                    b[13] = 1;
                    b[14..16].copy_from_slice(&32u16.to_le_bytes());
                    b[16] = 1;
                    b[21] = 0xf8;
                    b[32..36].copy_from_slice(&70000u32.to_le_bytes());
                    b[36..40].copy_from_slice(&600u32.to_le_bytes());
                    b[44..48].copy_from_slice(&2u32.to_le_bytes());
                    b[48..50].copy_from_slice(&1u16.to_le_bytes());
                    b[510..512].copy_from_slice(&[0x55, 0xaa]);
                }
                2 => {
                    b[0..4].copy_from_slice(&0x41615252u32.to_le_bytes());
                    b[484..488].copy_from_slice(&0x61417272u32.to_le_bytes());
                    b[488..496].fill(0xff);
                    b[508..512].copy_from_slice(&0xaa550000u32.to_le_bytes());
                }
                33 => {
                    for cluster in 0..16 {
                        let value = if cluster < 3 || cluster >= 2 + self.size.div_ceil(512) {
                            0x0fffffff
                        } else {
                            cluster as u32 + 1
                        };
                        b[cluster * 4..cluster * 4 + 4].copy_from_slice(&value.to_le_bytes());
                    }
                }
                633 if self.present => {
                    b[..11].copy_from_slice(b"UNSIGNEDPSB");
                    b[11] = 0x20;
                    b[26..28].copy_from_slice(&3u16.to_le_bytes());
                    b[28..32].copy_from_slice(&(self.size as u32).to_le_bytes());
                }
                634..=650 => {
                    if self.fail_data.get() {
                        return Err(());
                    }
                    for (j, value) in b.iter_mut().enumerate() {
                        *value = (((sector - 634) * 512 + j) % 251) as u8;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}
struct Clock;
impl TimeSource for Clock {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp::from_calendar(2026, 9, 15, 0, 0, 0).unwrap()
    }
}
fn manager(size: usize, present: bool) -> (VolumeManager<Disk, Clock, 1, 1, 1>, Rc<Cell<bool>>) {
    manager_writable(size, present, false)
}
fn manager_writable(
    size: usize,
    present: bool,
    writable: bool,
) -> (VolumeManager<Disk, Clock, 1, 1, 1>, Rc<Cell<bool>>) {
    let fail = Rc::new(Cell::new(false));
    let written = Rc::new(std::cell::RefCell::new(std::collections::HashMap::new()));
    let disk = Disk {
        size,
        present,
        fail_data: fail.clone(),
        writable,
        written,
    };
    (VolumeManager::new_with_limits(disk, Clock, 0), fail)
}

#[test]
fn four_character_extension_was_rejected_before_reading() {
    assert!(ShortFileName::create_from_str("UNSIGNED.PSBT").is_err());
    assert!(ShortFileName::create_from_str(reader::IMPORT_FILENAME).is_ok());
}

#[test]
fn reads_partial_blocks_and_exact_limit_repeatedly() {
    for size in [226, 256, 512, 4096] {
        let (mut mgr, _) = manager(size, true);
        for _ in 0..4 {
            let bytes = reader::read(&mut mgr, 4096).unwrap();
            assert_eq!(
                bytes,
                (0..size).map(|i| (i % 251) as u8).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn errors_are_distinct_and_handles_are_released() {
    for (size, present, error) in [
        (0, true, reader::ReadError::Empty),
        (4097, true, reader::ReadError::TooLarge),
        (226, false, reader::ReadError::Missing),
    ] {
        let (mut mgr, _) = manager(size, present);
        for _ in 0..4 {
            assert_eq!(reader::read(&mut mgr, 4096), Err(error.clone()));
        }
    }
    let (mut mgr, fail) = manager(226, true);
    fail.set(true);
    assert_eq!(reader::read(&mut mgr, 4096), Err(reader::ReadError::Read));
    fail.set(false);
    assert_eq!(reader::read(&mut mgr, 4096).unwrap().len(), 226);
}

#[test]
fn export_uses_first_free_slot_and_never_overwrites() {
    let (mut mgr, _) = manager_writable(226, true, true);
    let payload = (0..333u32).map(|i| (i % 251) as u8).collect::<Vec<_>>();
    let n1 = reader::write_signed(&mut mgr, &payload).unwrap();
    assert_eq!(n1, reader::EXPORT_FIRST);
    let n2 = reader::write_signed(&mut mgr, &payload).unwrap();
    assert_eq!(n2, "SIGNED1.PSB");
    let n3 = reader::write_signed(&mut mgr, &payload).unwrap();
    assert_eq!(n3, "SIGNED2.PSB");
}

#[test]
fn export_too_many_exports_errors_after_ten() {
    let (mut mgr, _) = manager_writable(226, true, true);
    let payload = vec![0u8; 32];
    for _ in 0..reader::EXPORT_MAX_INDEX + 1 {
        reader::write_signed(&mut mgr, &payload).unwrap();
    }
    assert_eq!(
        reader::write_signed(&mut mgr, &payload),
        Err(reader::WriteError::TooManyExports)
    );
}

#[test]
fn export_empty_payload_errors() {
    let (mut mgr, _) = manager_writable(226, true, true);
    assert_eq!(
        reader::write_signed(&mut mgr, &[]),
        Err(reader::WriteError::Empty)
    );
}

#[test]
fn export_then_read_back_roundtrip() {
    let (mut mgr, _) = manager_writable(226, true, true);
    let payload = (0..400u32).map(|i| (i % 251) as u8).collect::<Vec<_>>();
    let name = reader::write_signed(&mut mgr, &payload).unwrap();
    assert_eq!(name, reader::EXPORT_FIRST);
    // Re-open the directory and read the file back.
    let mut volume = mgr.open_volume(embedded_sdmmc::VolumeIdx(0)).unwrap();
    let mut root = volume.open_root_dir().unwrap();
    let sfn = embedded_sdmmc::ShortFileName::create_from_str(&name).unwrap();
    let mut file = root
        .open_file_in_dir(&sfn, embedded_sdmmc::Mode::ReadOnly)
        .unwrap();
    let size = file.length() as usize;
    assert_eq!(size, payload.len());
    let mut readback = vec![0u8; size];
    let mut offset = 0;
    while offset < size {
        let end = (offset + 256).min(size);
        let count = file.read(&mut readback[offset..end]).unwrap();
        if count == 0 {
            break;
        }
        offset += count;
    }
    assert_eq!(readback, payload);
}

#[test]
fn seed_psbt_round_trip_through_fat_reader_and_writer() {
    use embedded_sdmmc::{Mode, VolumeIdx};
    use signer_probe::{
        bitcoin::{psbt::Psbt, secp256k1::Secp256k1, Network},
        seed::{ReviewBinding, SeedSession},
    };
    let (mut mgr, _) = manager_writable(226, true, true);
    let fixture = include_bytes!("fixtures/game-main.psb");
    {
        let mut volume = mgr.open_volume(VolumeIdx(0)).unwrap();
        let mut root = volume.open_root_dir().unwrap();
        let mut file = root
            .open_file_in_dir("UNSIGNED.PSB", Mode::ReadWriteCreateOrTruncate)
            .unwrap();
        file.write(fixture).unwrap();
        file.flush().unwrap();
        file.close().unwrap();
    }
    let imported = reader::read(&mut mgr, signer_probe::MAX_PSBT_BYTES).unwrap();
    assert_eq!(imported, fixture);
    let seed = SeedSession::recover(
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        "", Network::Bitcoin).unwrap();
    let review = seed.review_psbt(&imported).unwrap();
    let binding = ReviewBinding {
        psbt_bytes: zeroize::Zeroizing::new(review.approved_psbt_bytes()),
        sighash: *review.sighash(),
        change_path: review.change_path.clone(),
        change_public_key: review.change_public_key,
    };
    let signed = seed.sign_psbt(&Secp256k1::new(), &binding).unwrap();
    let name = reader::write_signed(&mut mgr, &signed.signed_psbt).unwrap();
    let mut volume = mgr.open_volume(VolumeIdx(0)).unwrap();
    let mut root = volume.open_root_dir().unwrap();
    let mut file = root
        .open_file_in_dir(name.as_str(), Mode::ReadOnly)
        .unwrap();
    let mut saved = vec![0; file.length() as usize];
    let mut n = 0;
    while n < saved.len() {
        let read = file.read(&mut saved[n..]).unwrap();
        assert!(read > 0);
        n += read;
    }
    file.close().unwrap();
    assert_eq!(saved, signed.signed_psbt);
    assert_eq!(
        Psbt::deserialize(&saved).unwrap().inputs[0]
            .partial_sigs
            .len(),
        1
    );
}

// Encode realistic FAT LFN entries, including sequence and SFN checksum.
fn long_entries(name: &str, alias: &[u8; 11], cluster: u16, size: u32) -> Vec<[u8; 32]> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let count = units.len().div_ceil(13);
    let checksum = alias
        .iter()
        .fold(0u8, |a, b| a.rotate_right(1).wrapping_add(*b));
    let mut result = Vec::new();
    for index in (1..=count).rev() {
        let mut raw = [0u8; 32];
        raw[0] = index as u8 | if index == count { 0x40 } else { 0 };
        raw[11] = 0x0f;
        raw[13] = checksum;
        for (n, offset) in [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
            .into_iter()
            .enumerate()
        {
            let pos = (index - 1) * 13 + n;
            let unit = if pos < units.len() {
                units[pos]
            } else if pos == units.len() {
                0
            } else {
                0xffff
            };
            raw[offset..offset + 2].copy_from_slice(&unit.to_le_bytes());
        }
        result.push(raw);
    }
    let mut short = [0u8; 32];
    short[..11].copy_from_slice(alias);
    short[11] = 0x20;
    short[26..28].copy_from_slice(&cluster.to_le_bytes());
    short[28..32].copy_from_slice(&size.to_le_bytes());
    result.push(short);
    result
}
fn install_directory(mgr: &mut VolumeManager<Disk, Clock, 1, 1, 1>, entries: &[[u8; 32]]) {
    let chunks = entries.len().div_ceil(16);
    let mut fat = [Block::new()];
    mgr.device().read(&mut fat, BlockIdx(33), "test").unwrap();
    for i in 0..chunks {
        let cluster = if i == 0 { 2 } else { 19 + i };
        let next = if i + 1 == chunks {
            0x0fffffff
        } else {
            (20 + i) as u32
        };
        fat[0].contents[cluster * 4..cluster * 4 + 4].copy_from_slice(&next.to_le_bytes());
        let mut block = Block::new();
        for (n, entry) in entries[i * 16..entries.len().min((i + 1) * 16)]
            .iter()
            .enumerate()
        {
            block.contents[n * 32..n * 32 + 32].copy_from_slice(entry);
        }
        mgr.device()
            .written
            .borrow_mut()
            .insert((631 + cluster) as u32, block);
    }
    mgr.device().written.borrow_mut().insert(33, fat[0].clone());
}
#[test]
fn picker_reads_long_names_across_clusters_and_opens_the_selected_alias() {
    let (mut mgr, _) = manager_writable(5, true, true);
    let mut deleted = [0u8; 32];
    deleted[0] = 0xe5;
    let mut entries = vec![deleted; 15];
    let long = format!("{}雪.psbt", "x".repeat(249)); // 255 UTF-16 units
    entries.extend(long_entries(&long, b"LONGNA~1PSB", 3, 5));
    entries.extend(long_entries(
        "Another Sparrow payment.PSBT",
        b"ANOTHE~1PSB",
        4,
        4,
    ));
    entries.extend(long_entries("ignore.txt", b"IGNORE  TXT", 5, 1));
    install_directory(&mut mgr, &entries);
    let mut a = Block::new();
    a.contents[..5].copy_from_slice(b"FIRST");
    let mut b = Block::new();
    b.contents[..4].copy_from_slice(b"NEXT");
    mgr.device().written.borrow_mut().insert(634, a);
    mgr.device().written.borrow_mut().insert(635, b);
    let files = reader::list(&mut mgr).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].name, "Another Sparrow payment.PSBT");
    assert_eq!(files[1].name, long);
    assert_eq!(
        reader::read_short(&mut mgr, &files[0].short_name, 4096).unwrap(),
        b"NEXT"
    );
    assert_eq!(
        reader::read_short(&mut mgr, &files[1].short_name, 4096).unwrap(),
        b"FIRST"
    );
}
#[test]
fn picker_ignores_invalid_lfn_associations_hidden_files_and_directories() {
    let (mut mgr, _) = manager_writable(5, true, true);
    let mut entries = long_entries("Wrong long name.psbt", b"FALLBACKPSB", 3, 5);
    entries[0][13] ^= 1; // checksum mismatch must never label another file
    let mut hidden = long_entries("Hidden.psbt", b"HIDDEN  PSB", 3, 5);
    hidden.last_mut().unwrap()[11] |= 2;
    entries.extend(hidden);
    let mut dir = long_entries("Folder.psbt", b"FOLDER  PSB", 3, 5);
    dir.last_mut().unwrap()[11] = 0x10;
    entries.extend(dir);
    entries.extend(long_entries("._AppleDouble.psbt", b"APPLE   PSB", 3, 5));
    install_directory(&mut mgr, &entries);
    let files = reader::list(&mut mgr).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name, "FALLBACK.PSB");
}
#[test]
fn picker_limits_list_without_silently_hiding_files() {
    let (mut mgr, _) = manager_writable(5, true, true);
    let mut entries = Vec::new();
    for i in 0..33 {
        let alias = format!("F{i:07}PSB");
        entries.extend(long_entries(
            &format!("payment {i}.psbt"),
            alias.as_bytes().try_into().unwrap(),
            3,
            5,
        ));
    }
    install_directory(&mut mgr, &entries);
    assert_eq!(reader::list(&mut mgr), Err(reader::ReadError::ListingLimit));
}
#[test]
fn picker_stops_corrupt_directory_cycles_and_reports_bad_chains() {
    let (mut mgr, _) = manager_writable(5, true, true);
    let mut deleted = [0u8; 32];
    deleted[0] = 0xe5;
    install_directory(&mut mgr, &vec![deleted; 16]);
    let mut fat = [Block::new()];
    mgr.device().read(&mut fat, BlockIdx(33), "test").unwrap();
    fat[0].contents[8..12].copy_from_slice(&2u32.to_le_bytes());
    mgr.device().written.borrow_mut().insert(33, fat[0].clone());
    assert_eq!(reader::list(&mut mgr), Err(reader::ReadError::ListingLimit));
    fat[0].contents[8..12].copy_from_slice(&0x0ffffff7u32.to_le_bytes());
    mgr.device().written.borrow_mut().insert(33, fat[0].clone());
    assert_eq!(reader::list(&mut mgr), Err(reader::ReadError::Directory));
}

#[test]
fn fat16_root_also_lists_long_names() {
    let (mut mgr, _) = manager_writable(5, true, true);
    let mut mbr = [Block::new()];
    mgr.device().read(&mut mbr, BlockIdx(0), "test").unwrap();
    mbr[0].contents[450] = 0x06;
    mgr.device().written.borrow_mut().insert(0, mbr[0].clone());
    let mut boot = Block::new();
    boot.contents[0..3].copy_from_slice(&[0xeb, 0x3c, 0x90]);
    boot.contents[11..13].copy_from_slice(&512u16.to_le_bytes());
    boot.contents[13] = 1;
    boot.contents[14..16].copy_from_slice(&1u16.to_le_bytes());
    boot.contents[16] = 1;
    boot.contents[17..19].copy_from_slice(&64u16.to_le_bytes());
    boot.contents[19..21].copy_from_slice(&7000u16.to_le_bytes());
    boot.contents[21] = 0xf8;
    boot.contents[22..24].copy_from_slice(&32u16.to_le_bytes());
    boot.contents[510..512].copy_from_slice(&[0x55, 0xaa]);
    mgr.device().written.borrow_mut().insert(1, boot);
    let entries = long_entries("Sparrow test transaction.psbt", b"SPARRO~1PSB", 3, 5);
    let mut root = Block::new();
    for (i, e) in entries.iter().enumerate() {
        root.contents[i * 32..i * 32 + 32].copy_from_slice(e);
    }
    mgr.device().written.borrow_mut().insert(34, root);
    let files = reader::list(&mut mgr).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name, "Sparrow test transaction.psbt");
}

#[test]
fn malformed_lfn_sequences_fall_back_to_the_actual_short_name() {
    for mutation in 0..4 {
        let (mut mgr, _) = manager_writable(5, true, true);
        let mut entries = long_entries("Long Sparrow payment.psbt", b"SAFEAL~1PSB", 3, 5);
        match mutation {
            0 => entries[0][0] = 0x43, // missing ordinal
            1 => entries[1][1..3].copy_from_slice(&0xd800u16.to_le_bytes()), // invalid UTF-16
            2 => entries[1][0] = 0xe5, // deleted LFN slot
            _ => entries[0][12] = 1, // reserved LFN type
        }
        install_directory(&mut mgr, &entries);
        let files = reader::list(&mut mgr).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "SAFEAL~1.PSB");
    }
}
