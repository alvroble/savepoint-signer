//! Crystal adapter: core 0 only copies bounded pages; core 1 owns Terminal and SD.
extern crate alloc;
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};
static COMPUTING: AtomicBool = AtomicBool::new(false);
#[inline(always)]
pub fn computing() -> bool {
    COMPUTING.load(Ordering::Acquire)
}
use cartridge_core::seed_link::{marker_offset, Link};
use embassy_rp::spinlock_mutex::blocking_mutex::SpinlockMutex;
use signer_probe::terminal::{PsbtFile, Terminal, PSBT_LISTING, PSBT_LOADING, PSBT_SIGNING};
static LINK: SpinlockMutex<2, RefCell<Link>> = SpinlockMutex::new(RefCell::new(Link::new()));
#[global_allocator]
static HEAP: embedded_alloc::LlffHeap = embedded_alloc::LlffHeap::empty();
#[repr(align(8))]
struct Bytes([u8; 65536]);
static mut BYTES: Bytes = Bytes([0; 65536]);
pub fn init() {
    unsafe {
        HEAP.init(core::ptr::addr_of_mut!(BYTES.0) as usize, 65536);
    }
}
pub struct Bus {
    offset: Option<usize>,
    command: u8,
    seq: u8,
}
impl Bus {
    pub fn new(rom: &[u8]) -> Self {
        Self {
            offset: marker_offset(rom),
            command: 0,
            seq: 0,
        }
    }
    pub fn write(&mut self, addr: u32, data: u8, rom: &mut [u8]) -> bool {
        let Some(off) = self.offset else { return false };
        match addr {
            0x7000 => {
                self.command = data;
            }
            0x7001 => {
                LINK.lock(|l| l.borrow_mut().submit(data, self.command));
            }
            0x7002 => {
                let mut page = LINK.lock(|l| l.borrow().page(data));
                self.seq = self.seq.wrapping_add(2);
                page[0] = self.seq;
                page[37] = self.seq;
                unsafe {
                    let p = rom.as_mut_ptr().add(off);
                    core::ptr::write_volatile(p, self.seq | 1);
                    cortex_m::asm::dmb();
                    for i in 1..38 {
                        core::ptr::write_volatile(p.add(i), page[i]);
                    }
                    cortex_m::asm::dmb();
                    core::ptr::write_volatile(p, self.seq);
                }
            }
            _ => return false,
        }
        true
    }
}
pub struct Worker {
    terminal: Terminal,
    aliases: alloc::vec::Vec<embedded_sdmmc::ShortFileName>,
}
impl Worker {
    pub fn new() -> Self {
        Self {
            terminal: Terminal::default(),
            aliases: alloc::vec::Vec::new(),
        }
    }
    pub fn poll<
        D: embedded_sdmmc::BlockDevice,
        T: embedded_sdmmc::TimeSource,
        const A: usize,
        const B: usize,
        const C: usize,
    >(
        &mut self,
        m: &mut embedded_sdmmc::VolumeManager<D, T, A, B, C>,
    ) {
        let Some(request) = LINK.lock(|l| l.borrow_mut().take()) else {
            return;
        };
        let t = &mut self.terminal;
        // Keep the MBC idle loop out of flash/RTC work during seed execution.
        COMPUTING.store(true, Ordering::Release);
        t.command(request.command);
        if t.state == 3 {
            t.finish_recovery();
        }
        if t.state == PSBT_LISTING {
            self.aliases.clear();
            match crate::sd_psbt::list(m) {
                Ok(files) => t.set_files(
                    files
                        .into_iter()
                        .map(|f| {
                            let alias = alloc::format!("{}", f.short_name);
                            self.aliases.push(f.short_name);
                            PsbtFile {
                                name: f.name,
                                alias,
                                size: f.size,
                            }
                        })
                        .collect(),
                ),
                Err(_) => t.psbt_error("SD listing failed"),
            }
        } else if t.state == PSBT_LOADING {
            let result = self
                .aliases
                .get(t.selected_file_index())
                .ok_or("Rescan SD")
                .and_then(|alias| {
                    crate::sd_psbt::read_short(m, alias, signer_probe::MAX_PSBT_BYTES)
                        .map_err(|_| "SD read failed")
                });
            match result {
                Ok(bytes) => t.load_psbt(&bytes),
                Err(e) => t.psbt_error(e),
            }
        } else if t.state == PSBT_SIGNING {
            if let Some(bytes) = t.finish_signing() {
                match crate::sd_psbt::write_signed(m, &bytes) {
                    Ok(name) => t.psbt_saved(&name),
                    Err(_) => t.psbt_error("SD write failed"),
                }
            }
        }
        let report = t.report();
        LINK.lock(|l| l.borrow_mut().finish(request, &report, &t.qr_buffer));
        COMPUTING.store(false, Ordering::Release);
    }
}
