//! Host-testable core logic for the RP2350 Game Boy cartridge firmware.
//!
//! This crate deliberately avoids any `embassy-rp`, `cortex-m` or `defmt`
//! dependencies so its unit tests can run on a regular `cargo test` host
//! (e.g. macOS / Linux x86_64 / arm64). The cartridge firmware's
//! `src/gb_mbc.rs`, `src/gb_savefile.rs`, `src/gb_rtc.rs` and
//! `src/rom_info.rs` modules historically embedded this logic alongside
//! PIO/DMA glue. The MBC3 routing logic and the SRAM/RTC save-file flow
//! are now exercised here; the firmware's binary keeps the
//! embassy-dependent glue that drives the PIO state machines.
//!
//! Save persistence decisions and the `.SAV`/`.TMP` format live here and
//! are called from firmware. MBC3 routing is also here; `src/gb_mbc.rs`
//! still has a parallel copy of that state machine for the PIO loop.

#![cfg_attr(target_arch = "arm", no_std)]

pub mod mbc3;
pub mod rom_info;
pub mod rtc;
pub mod save_coordinator;
pub mod savefile;

pub mod seed_link;
