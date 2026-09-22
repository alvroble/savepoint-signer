# Local read-only directory iterator extension

Source: embedded-sdmmc 0.8.0 from crates.io, registry checksum
150f320125310e179b9e73b081173b349e63c5c7d4ca44db4e5b9121b10387ec.
MIT and Apache-2.0 license files are retained.

The original public iterate_dir API still reports only short entries. New
iterate_dir_raw methods on Directory, VolumeManager and FatVolume expose raw
32-byte slots plus a decoded short entry when applicable. A callback can stop
the traversal, permitting a bounded directory scan. FAT16/32 walkers pass LFN
and deleted slots through; only EndOfFile terminates cluster traversal normally.
Other chain errors now propagate instead of silently returning a partial list.

LFN assembly and checksum validation live outside this crate in src/sd_lfn.rs.
The seed opens the exact ShortFileName obtained from listing, not a reconstructed
name or an index in a fresh scan. No LFN creation or on-disk format changes.

This extension preserves the 0.8 ownership/Send behavior used by the cartridge's
existing multicore save flow. Upstream newer releases also support LFN iteration,
but change that ownership API:
https://github.com/rust-embedded-community/embedded-sdmmc-rs/releases

Coverage: signer-probe/tests/sd_psbt.rs exercises FAT16 and FAT32 listing, names
spanning clusters, 255 UTF-16-unit names, malformed and deleted LFN sequences,
checksum mismatch, correct selected-file reads, bounded lists/cycles and errors.
