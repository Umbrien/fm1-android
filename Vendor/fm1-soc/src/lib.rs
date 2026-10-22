//! FM-1 SoC: memory map + peripherals.
//!
//! Memory map (recon + Felucca HAL headers + corpus literals):
//!   0x0200_0000  XIP flash (app window) — code runs from here
//!   0x01c0_0000  data RAM (≥512 KiB; BOOT_STATE at 0x01C7FD80 observed)
//!   0x5000_0-    GPIO ports A..H (0x40 stride: OUT+0, DIR+8, DIE+C, PU+10, PD+14)
//!   0x5100_|     IOMAP
//!   0x1000_-0x1002  clock
//!   0x1080_0/0x1090_0  TIMER4/TIMER5 (CNT reads = synthetic progress)
//!   0x11c0_0-    SPI0 = external NOR flash (model in spiflash.rs; CS = PD0)
//!   0x11d0_0-    SPI1 = LCD (ST7789-class 240×240, model in lcd.rs)
//!   0x1180_0-    USB
//!   0x1210_0-    UART1 (MIDI IN)
//!   0x1310_0-    ADC
//!   0x1eee000-   interrupt controller (0x1eee008 bit14 = ready, polled by the
//!                RAM early-init; the rest stored bits)
//!   0x1eee240    DBG_MSG debug box (console)
//!   0x01f0_0000  SRAM banks (256 KiB; cleared by the early init, config block at 0x1f28000)
//!   0x01c0xxxx 'negative' long-call targets are RAM-resident .data code
//!   (copied from flash 0x02084820 by the CRT), not a mask ROM
//!
//! Policy: unknown MMIO accesses are logged and return a benign default —
//! never silently swallowed. The access log is the reverse-engineering
//! roadmap.

use fm1_core::Bus;

pub mod script;
pub mod fwsc;
mod lcd;
pub mod machine;
pub mod periph;
mod ram;
mod p33;
pub mod smk;
mod spiflash;

pub use lcd::{write_bmp, Lcd};
pub use periph::{Action, Periph, CPU_HZ, VEC_BASE};
pub use ram::Ram;
pub use spiflash::{SpiFlash, FLASH_SIZE};

pub const XIP_BASE: u32 = 0x0200_0000;
pub const RAM_BASE: u32 = 0x01c0_0000;
pub const OVL_BASE: u32 = 0x0400_0000;
pub const SRAM2_BASE: u32 = 0x01f0_0000;
/// The stock V13 firmware's .bd_base: what the BT baseband's window register 0x2FC44 reads
/// before a firmware writes it (each stock image writes its own .bd_base there: V13 this,
/// the SMK-37's v016 0x01C09A18).
pub const BT_WINDOW_BASE: u32 = 0x01c0_9e7c;
/// The flash offset behind XIP 0x02000000 (the stock layout: the app area starts at 0x4000).
pub const XIP_FLASH_OFFSET: usize = 0x4000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownAccess {
    pub addr: u32,
    pub width: u8,
    pub is_write: bool,
    pub value: u32,
}

#[derive(Debug, PartialEq, Eq)]
enum Region {
    Xip,
    Ram,
    Ovl,
    Gpio,
    Iomap,
    Clk,
    Timer,
    Spi0,
    Spi1,
    Usb,
    Uart1,
    Adc,
    Ana,
    Bt,
    Spi2,
    Crc,
    Rand,
    P33,
    XipFlash,
    Dbg,
    Irqc,
    /// Interrupt controller enable/priority words and software latch.
    Irqc2,
    /// ALNK0 I2S out.
    Alnk,
    Sram2,
    Unknown,
}

/// The SoC bus: XIP flash + data RAM + modeled peripherals + a logged stub.
/// A cheap hasher for register addresses (the SoC's register map): `std`'s SipHash cost the
/// SMK-37's stock firmware, which touches registers about a million times a second, a few
/// percent of its time.
#[derive(Default)]
pub struct AddrHasher(u64);

impl std::hash::Hasher for AddrHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    fn write_u32(&mut self, v: u32) {
        self.0 = (v as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

/// The board around the chip: the FM-1's panel, or the SMK-37's (SMK-37 Pro / Elite, MKE-P37
/// and Starrykey 37 Play share one: fm1-to-smk37 docs/01-hardware.md). Same chip, flash
/// layout, audio and update path; the panel's wiring differs (smk.rs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Board {
    #[default]
    Fm1,
    Smk37,
}

impl Board {
    /// The board a package is for, from its identity (`FM-1_015`, `SMK-37 Pro_016`, ...).
    pub fn of_identity(id: &str) -> Board {
        let u = id.to_ascii_uppercase();
        if ["SMK", "MKE", "STARRYKEY"].iter().any(|p| u.starts_with(p)) {
            Board::Smk37
        } else {
            Board::Fm1
        }
    }
}

pub struct Soc {
    pub board: Board,
    /// The BT baseband's window base (0x2FC44), as the firmware set it.
    bt_window: u32,
    pub xip: Vec<u8>,
    /// Executable flash generation, shared by both CPUs' cache guards.
    xip_version: u64,
    /// Union of decoded XIP words across both CPUs, indexed in physical flash.
    /// Retained across invalidations so no surviving decode dependency is lost.
    xip_code: Vec<u8>,
    pub ram: Ram,
    pub lcd: Lcd,
    /// SPI0 + the external NOR flash (1 MiB; `--flash` image loaded at 0).
    pub flash: SpiFlash,
    /// Console bytes written through DBG_MSG / UART-style debug boxes.
    pub console: Vec<u8>,
    /// Overlay RAM (second bank, 0x04000000), written by the app CRT.
    pub ovl: Ram,
    /// SRAM banks at 0x01f00000 (cleared by the RAM early init).
    pub sram2: Ram,
    /// monotonic "time" for peripheral counters
    pub ticks: u64,
    /// Timers, audio DMA, ADC, interrupt controller and the front panel.
    pub periph: Periph,
    sfr: std::collections::HashMap<u32, u32, std::hash::BuildHasherDefault<AddrHasher>>,
    /// Core 0 / 1 paused by the other (C0_CON / C1_CON bit 2 pulses, bit 3 resumes).
    paused: [bool; 2],
    /// The XIP window continues into the flash chip past the image (the stock layout, which
    /// the updater gives every firmware: April OS keeps its samples at flash 0x74000, read
    /// through XIP 0x02070000, and waits at boot until it finds them or a blank chip's 0xff).
    pub xip_flash: bool,
    /// cpu0 set 0x10008 bit 3 (core 1's release); latched until the runner starts cpu1, as
    /// firmwares hold the bit only while they wait for core 1 to report in.
    pub cpu1_release: bool,
    pub(crate) cpu1_started: bool,
    p33: p33::P33,
    /// FM1_AUDIO_TRACE: log each audio half-buffer (debugging the DMA's pace).
    audio_trace: bool,
    /// The CRC-16 unit's register (0x13504).
    crc: u16,
    /// The random number generator's state (JL_RAND, 0x13B00: R64L / R64H).
    rand: u64,
    /// The cores' hardware lock (`lockset` / `lockclr`): which core holds it.
    lock_owner: Option<u32>,
    /// FM1_GPIO_TRACE: GPIO register writes by address (debugging port use).
    pub gpio_writes: Option<std::collections::BTreeMap<u32, u64>>,
    /// Opt-in benchmark counters; ordinary audio execution does not allocate entries.
    pub mmio_profile: Option<std::collections::BTreeMap<(bool, u32), u64>>,
    /// Log2 CPU1 slice-size buckets: calls and retired instructions, diagnostic only.
    pub cpu1_slice_profile: Option<[(u64, u64); 33]>,
    /// Bounded fixed-point trials: attempts, accepted cycles, guest instructions
    /// accounted from exact repetition (not instructions run through generated code).
    pub cpu1_fixed_points: [u64;3],
    /// Opt-in exact-proof rejection reasons; address and width are diagnostic.
    pub cpu1_fixed_point_failures: Option<std::collections::BTreeMap<String,u64>>,
    unknown_log: Vec<UnknownAccess>,
}

/// State touched by the proof bus's admitted MMIO (the timer region only).
/// Flash, LCD, DMA, debug, GPIO, locks and unknown accesses are rejected before
/// execution. RAM uses a bounded exact write journal rather than a bulk copy.
#[derive(Clone, PartialEq)]
pub(crate) struct TimerProofState {
    periph: Periph,
    sfr: std::collections::HashMap<u32,u32,std::hash::BuildHasherDefault<AddrHasher>>,
    ticks: u64,
}

fn dbg_line(addr: u32) -> &'static str {
    match addr & !3 {
        0x1eee240 => "wr_en",
        0x1eee244 => "msg",
        0x1eee248 => "msg_clr",
        _ => "dbg",
    }
}

impl Soc {
    /// Replace the physical flash image without resetting CPU or peripheral state.
    /// Publish only changed physical bytes into the active XIP view, following the
    /// SPI update path. The package's unchanged raw bytes may be scrambled while
    /// `xip` contains their decoded image; copying the whole image would corrupt it.
    /// Call only between CPU execution entries, as with other host state mutations.
    pub fn restore_flash(&mut self, image: &[u8]) -> bool {
        if image.len() != self.flash.mem.len() {
            return false;
        }
        let mut executable_changed = false;
        if self.xip_flash {
            for (physical, (&old, &new)) in self.flash.mem.iter().zip(image).enumerate().skip(XIP_FLASH_OFFSET) {
                if old == new { continue; }
                let changed = match self.xip.get_mut(physical - XIP_FLASH_OFFSET) {
                    Some(mapped) => {
                        let changed = *mapped != new;
                        *mapped = new;
                        changed
                    }
                    None => true, // The XIP continuation reads directly from flash.mem.
                };
                if changed && self.xip_code.get(physical / 4) == Some(&1) {
                    executable_changed = true;
                }
            }
        }
        self.flash.mem.copy_from_slice(image);
        if executable_changed {
            self.xip_version = self.xip_version.wrapping_add(1);
        }
        true
    }

    pub(crate) fn timer_proof_state(&self) -> TimerProofState {
        TimerProofState { periph:self.periph.clone(), sfr:self.sfr.clone(), ticks:self.ticks }
    }

    pub(crate) fn restore_timer_proof_state(&mut self, state: TimerProofState) {
        self.periph=state.periph; self.sfr=state.sfr; self.ticks=state.ticks;
    }

    pub(crate) fn proof_read_allowed(&self, addr: u32, width: u32) -> bool {
        let addr = if width == 2 { addr & !1 } else { addr };
        match self.region(addr) {
            Region::Ram => addr.wrapping_sub(RAM_BASE) as usize + width as usize <= self.ram.len(),
            Region::Ovl => addr.wrapping_sub(OVL_BASE) as usize + width as usize <= self.ovl.len(),
            Region::Sram2 => addr.wrapping_sub(SRAM2_BASE) as usize + width as usize <= self.sram2.len(),
            Region::Xip | Region::XipFlash => {
                addr.wrapping_sub(XIP_BASE) as usize + XIP_FLASH_OFFSET + width as usize <= FLASH_SIZE
            }
            // TIMER4 reads can advance emulated time through skip_idle. All other
            // timer mutations, including fallback counters, belong to the snapshot.
            Region::Timer => addr & !3 != 0x10804,
            // The OS tick count is pure arithmetic over frozen peripheral
            // cycles. Other interrupt-controller registers remain unsupported.
            Region::Irqc2 => addr & !3 == 0x1eef0f0 && (width != 4 || addr & 3 == 0),
            _ => false,
        }
    }

    pub(crate) fn proof_timer_write_allowed(&self, addr: u32) -> bool {
        matches!(self.region(addr), Region::Timer)
    }

    /// After an erase or page program: the XIP window shows the flash chip, also where the
    /// window's contents come from the package (`xip`, the app area as installed). A booted
    /// package's window reaches flash 0x93000, and April OS writes its samples from 0x74000
    /// and reads them back through XIP.
    fn sync_xip(&mut self) {
        let Some((a, b)) = self.flash.changed.take() else { return };
        if !self.xip_flash {
            return;
        }
        // Data-only flash traffic must not discard decoded/compiled code. Marked
        // windows include decoder lookahead and if-block extent scans, in either
        // CPU's direct image or its flash-backed continuation.
        let lo = a.max(XIP_FLASH_OFFSET).min(spiflash::FLASH_SIZE);
        let hi = b.min(spiflash::FLASH_SIZE);
        if lo < hi && self.xip_code[lo / 4..hi.div_ceil(4)].contains(&1) {
            self.xip_version = self.xip_version.wrapping_add(1);
        }
        for o in a.max(XIP_FLASH_OFFSET)..b {
            match self.xip.get_mut(o - XIP_FLASH_OFFSET) {
                Some(x) => *x = self.flash.mem[o],
                None => break,
            }
        }
    }

    /// `ram_size` in bytes; `xip` = the app binary mapped at XIP_BASE.
    pub fn new(ram_size: usize, xip: Vec<u8>) -> Self {
        Self {
            board: Board::Fm1,
            bt_window: BT_WINDOW_BASE,
            xip,
            xip_version: 0,
            xip_code: vec![0; spiflash::FLASH_SIZE.div_ceil(4)],
            ram: Ram::new(ram_size),
            ovl: Ram::new(512 * 1024),
            sram2: Ram::new(256 * 1024),
            lcd: Lcd::new(),
            flash: SpiFlash::new(),
            console: Vec::new(),
            ticks: 0,
            periph: Periph::default(),
            sfr: Default::default(),
            paused: [false; 2],
            lock_owner: None,
            crc: 0,
            rand: 0x9e37_79b9_7f4a_7c15,
            cpu1_release: false,
            cpu1_started: false,
            p33: p33::P33::default(),
            audio_trace: std::env::var_os("FM1_AUDIO_TRACE").is_some(),
            xip_flash: true,
            gpio_writes: std::env::var("FM1_GPIO_TRACE").ok().map(|_| Default::default()),
            mmio_profile: std::env::var_os("FM1_PROFILE").map(|_| Default::default()),
            cpu1_slice_profile: std::env::var_os("FM1_SCHED_PROFILE").map(|_| [(0, 0); 33]),
            cpu1_fixed_points: [0;3],
            cpu1_fixed_point_failures: std::env::var_os("FM1_PROFILE").map(|_| Default::default()),
            unknown_log: Vec::new(),
        }
    }

    /// Wire the chip into `board`'s panel (and its flash chip's id).
    pub fn set_board(&mut self, board: Board) {
        self.board = board;
        self.periph.board = board;
        if board == Board::Smk37 {
            self.flash.jedec = [0xeb, 0x60, 0x14];
        }
    }

    /// All unknown accesses so far — the peripheral bring-up TODO list.
    /// Advance the peripheral clock by `n` CPU cycles, moving any finished
    /// audio half out of RAM (int32 L/R holding a 24-bit sample in the low
    /// bits — Felucca writes Q15 << 7, audio.c OUT_SHIFT — so >> 8 to i16).
    pub fn advance(&mut self, n: u64) {
        let due = self.periph.advance(n);
        if let Some((at, b)) = due.uart_rx {
            self.write8(at, b);
        }
        if let Some((adr, words)) = due.audio_half {
            if self.audio_trace {
                eprintln!("half {} ms {}", self.periph.ms(), self.periph.cycles);
            }
            for i in 0..words {
                let address = adr.wrapping_add(i * due.audio_word_bytes as u32);
                let sample = if due.audio_word_bytes == 2 { self.read16(address) as i16 }
                    else { (self.read32(address) as i32 >> 8).clamp(-32768, 32767) as i16 };
                self.periph.audio.push(sample);
            }
        }
        for (adr, words, width) in due.audio_rx.into_iter().flatten() {
            for i in 0..words {
                // Use the ordinary bus: DMA into watched executable RAM must
                // invalidate decoded and compiled instructions just like a CPU store.
                let address = adr.wrapping_add(i * width as u32);
                if width == 2 { self.write16(address, 0); } else { self.write32(address, 0); }
            }
        }
    }

    /// The highest-priority enabled pending interrupt: (irq, prio).
    #[inline]
    pub fn pending_irq(&self) -> Option<(u8, u8)> {
        self.periph.pending_irq()
    }

    /// Core `core` (0, 1) is paused by the other through its CON register's bit 2
    /// (0x1EEE000 / 0x1EEE004): the stock firmware stops core 1 around flash work.
    pub fn core_paused(&self, core: u32) -> bool {
        core < 2 && self.paused[core as usize]
    }

    /// The highest-priority interrupt pending for core `core` (its own enable bank).
    pub fn pending_irq_for(&self, core: u32) -> Option<(u8, u8)> {
        self.periph.pending_irq_for(core)
    }

    pub fn unknown_accesses(&self) -> &[UnknownAccess] {
        &self.unknown_log
    }

    /// The debug console contents (DBG_MSG box writes).
    pub fn console_string(&self) -> String {
        String::from_utf8_lossy(&self.console).into_owned()
    }

    fn log_unknown(&mut self, addr: u32, width: u8, is_write: bool, value: u32) {
        self.unknown_log.push(UnknownAccess {
            addr,
            width,
            is_write,
            value,
        });
        // keep the last 8192..16384 (dropping one at a time shifted the whole log per access)
        if self.unknown_log.len() >= 16384 {
            self.unknown_log.drain(..8192);
        }
    }

    #[inline]
    fn region(&self, addr: u32) -> Region {
        // Every memory window starts at or above RAM_BASE. Low MMIO therefore
        // needs no dynamic memory checks; retain the original window priority
        // for high addresses, including unusual overlapping RAM/XIP views.
        if addr >= RAM_BASE {
            if addr >= XIP_BASE && addr - XIP_BASE < self.xip.len() as u32 { return Region::Xip; }
            if self.xip_flash && addr >= XIP_BASE && (addr - XIP_BASE) as usize + XIP_FLASH_OFFSET < FLASH_SIZE { return Region::XipFlash; }
            if addr - RAM_BASE < self.ram.len() as u32 { return Region::Ram; }
            if addr >= OVL_BASE && addr - OVL_BASE < self.ovl.len() as u32 { return Region::Ovl; }
            if addr >= SRAM2_BASE && addr - SRAM2_BASE < self.sram2.len() as u32 { return Region::Sram2; }
        }
        // Register blocks occupy 256-byte pages. Classify the page once, then
        // check only a short block's tail rather than testing every full range.
        let offset = addr as u8;
        match addr >> 8 {
            0x100 if offset <= 0x1f => Region::Clk,
            0x104..=0x108 => Region::Timer,
            0x109 if offset <= 0x1f => Region::Timer,
            0x118 if offset <= 0x3f => Region::Usb,
            0x119 => Region::Ana,
            0x11c if offset <= 0x1f => Region::Spi0,
            0x11d if offset <= 0x1f => Region::Spi1,
            0x11e if offset <= 0x1f => Region::Spi2,
            0x121 if offset <= 0x2f => Region::Uart1,
            0x12e if offset <= 0x3f => Region::Alnk,
            0x131 if offset <= 0x1f => Region::Adc,
            0x135 if offset <= 0x0f => Region::Crc,
            0x13b if offset <= 0x07 => Region::Rand,
            0x13e if (0x08..=0x0f).contains(&offset) => Region::P33,
            0x168..=0x169 if self.board == Board::Smk37 => Region::Usb,
            0x16a if offset <= 0x1f && self.board == Board::Smk37 => Region::Usb,
            0x200..=0x2ff => Region::Bt,
            0x500..=0x501 => Region::Gpio,
            0x510 if offset <= 0x3f => Region::Iomap,
            0x1eee0 => Region::Irqc,
            0x1eee2 if (0x40..=0x4f).contains(&offset) => Region::Dbg,
            0x1eef0..=0x1eef3 => Region::Irqc2,
            _ => Region::Unknown,
        }
    }

    // Independent pre-optimization range classifier, retained only as an oracle.
    #[cfg(test)]
    fn region_reference(&self, addr: u32) -> Region {
        match addr {
            a if a >= XIP_BASE && (a - XIP_BASE) < self.xip.len() as u32 => Region::Xip,
            // past the loaded image the XIP window still shows the flash chip (the stock
            // firmware reads its USR preset area, which it writes over SPI, through XIP)
            a if self.xip_flash && a >= XIP_BASE && (a - XIP_BASE) as usize + XIP_FLASH_OFFSET < spiflash::FLASH_SIZE => Region::XipFlash,
            a if a >= RAM_BASE && (a - RAM_BASE) < self.ram.len() as u32 => Region::Ram,
            a if a >= OVL_BASE && (a - OVL_BASE) < self.ovl.len() as u32 => Region::Ovl,
            a if a >= SRAM2_BASE && (a - SRAM2_BASE) < self.sram2.len() as u32 => Region::Sram2,
            0x1eee000..=0x1eee0ff => Region::Irqc,
            0x1eef000..=0x1eef3ff => Region::Irqc2,
            0x12e00..=0x12e3f => Region::Alnk,
            0x50000..=0x501ff => Region::Gpio,
            0x51000..=0x5103f => Region::Iomap,
            0x10000..=0x1001f => Region::Clk,
            0x10400..=0x1091f => Region::Timer,
            0x11c00..=0x11c1f => Region::Spi0,
            0x11d00..=0x11d1f => Region::Spi1,
            0x11e00..=0x11e1f => Region::Spi2,
            0x11800..=0x1183f => Region::Usb,
            // the second USB PHY (the SMK-37's LED commons at 0x16A0C, its pads used as GPIO; its
            // stock firmware powers the PHY up at boot and waits for 0x16800 bit 4). The FM-1's
            // stock firmware touches the block too, as before: unknown.
            0x16800..=0x16a1f if self.board == Board::Smk37 => Region::Usb,
            0x12100..=0x1212f => Region::Uart1,
            0x13100..=0x1311f => Region::Adc,
            0x13500..=0x1350f => Region::Crc,
            0x13b00..=0x13b07 => Region::Rand,
            0x13e08..=0x13e0f => Region::P33,
            0x11900..=0x119ff => Region::Ana,
            0x20000..=0x2ffff => Region::Bt,
            0x1eee240..=0x1eee24f => Region::Dbg,
            _ => Region::Unknown,
        }
    }

    fn sfr_get(&mut self, addr: u32) -> u32 {
        *self.sfr.get(&addr).unwrap_or(&0)
    }

    fn sfr_set(&mut self, addr: u32, v: u32) {
        self.sfr.insert(addr, v);
    }

    /// SPI1/MMIO side effects on 32-bit writes.
    fn spi1_write(&mut self, addr: u32, value: u32) {
        match addr {
            0x11d00 => {
                // CON: bit14 clears the pending transfer-done (and its IRQ 16)
                if value & 0x4000 != 0 {
                    self.periph.spi1_pending = false;
                }
                let stored = self.sfr_get(addr);
                self.sfr_set(addr, (stored & !0x4000) | (value & !0x4000));
            }
            0x11d04 => self.sfr_set(addr, value), // BAUD
            0x11d08 => {
                // BUF: single byte, D/C = PC8 state (SMK-37: PB5; its group B is the pad LEDs)
                if let Some(dc) = self.lcd_dc() {
                    self.lcd.feed(value as u8, dc);
                }
                self.sfr_set(addr, value);
                self.spi1_done(1);
            }
            0x11d0c => self.sfr_set(addr, value), // ADR
            0x11d10 => {
                // CNT: DMA `value` bytes from RAM[ADR] through the panel
                let adr = self.sfr_get(0x11d0c);
                let n = value.min(0x40000);
                // the DMA source is a bus address (RAM, SRAM or XIP)
                let bytes: Vec<u8> = (0..n).map(|i| self.read8(adr.wrapping_add(i))).collect();
                match self.lcd_dc() {
                    Some(dc) => {
                        for b in bytes {
                            self.lcd.feed(b, dc);
                        }
                    }
                    None => self.periph.smk.pad_frame(&bytes),
                }
                self.sfr_set(addr, 0);
                self.spi1_done(n);
            }
            _ => self.sfr_set(addr, value),
        }
    }

    /// Where SPI1's bytes go: the LCD with its D/C pin's state (FM-1: PC8; SMK-37: PB5, while
    /// IOMAP_CON1 bit 4 picks pin group A), or None: the SMK-37's pad LEDs (group B).
    fn lcd_dc(&mut self) -> Option<bool> {
        match self.board {
            Board::Fm1 => Some(self.sfr_get(0x50080) & 0x100 != 0),
            Board::Smk37 => (self.sfr_get(0x51020) & 0x10 == 0).then(|| self.sfr_get(0x50040) & 0x20 != 0),
        }
    }

    /// SPI2 (0x11E00, CON/BAUD/BUF/ADR/CNT like SPI1): the stock firmware shifts the key
    /// matrix's column pattern into the two 74HC595s with it, one 2-byte DMA per column,
    /// driven by its IRQ 37 handler (which also reads the rows and pulses RCLK on PA1).
    fn spi2_write(&mut self, addr: u32, value: u32) {
        match addr {
            0x11e00 => {
                if value & 0x4000 != 0 {
                    self.periph.spi2_pending = false;
                }
                self.sfr_set(addr, value & !0xc000);
            }
            0x11e08 => {
                self.periph.sr_shift_byte(value as u8);
                self.sfr_set(addr, value);
                self.spi2_started(1);
            }
            0x11e10 => {
                let adr = self.sfr_get(0x11e0c);
                for i in 0..value.min(64) {
                    let b = self.read8(adr.wrapping_add(i));
                    self.periph.sr_shift_byte(b);
                }
                self.sfr_set(addr, 0);
                self.spi2_started(value);
            }
            _ => self.sfr_set(addr, value),
        }
    }

    fn spi2_started(&mut self, bytes: u32) {
        let con = self.sfr_get(0x11e00);
        let baud = self.sfr_get(0x11e04);
        self.periph.spi2_start(bytes, baud, con & 0x2000 != 0);
    }

    /// A SPI1 transfer finished (instantly here): with CON bit 13 (interrupt enable) that is
    /// IRQ 16, the stock LCD driver's DMA-done (its layer flush waits for the handler).
    /// (SMK-37: it takes the bytes' time on the wire, see `Periph::spi1_start`.)
    fn spi1_done(&mut self, bytes: u32) {
        let irq = self.sfr_get(0x11d00) & 0x2000 != 0;
        if self.board == Board::Smk37 {
            let baud = self.sfr_get(0x11d04);
            self.periph.spi1_start(bytes, baud, irq);
        } else if irq {
            self.periph.spi1_pending = true;
        }
    }

    fn timer_read(&mut self, addr: u32) -> u32 {
        // "time" base ticks so timeout/wait loops make progress; count
        // registers report a moving value, the rest stored bits.
        self.ticks = self.ticks.wrapping_add(1);
        if addr & 0xff == 0x04 {
            let t = (self.ticks * 7) as u32;
            return t;
        }
        self.sfr_get(addr)
    }
}

impl Bus for Soc {
    fn irq_window(&mut self) {
        if self.periph.any_pending() {
            self.periph.next_due = 0;
        }
    }
    fn irq_window_for(&mut self, core: u32, minimum_priority: u8) {
        if self.periph.pending_irq_for(core).is_some_and(|(_, priority)| priority >= minimum_priority) {
            self.periph.next_due = 0;
        }
    }

    fn idle(&mut self, core: u32) {
        if core == 0 {
            self.periph.idle();
        }
    }

    fn hw_lock(&mut self, core: u32, set: bool) -> bool {
        match (set, self.lock_owner) {
            (true, Some(c)) if c != core => false,
            (true, _) => {
                self.lock_owner = Some(core);
                true
            }
            (false, Some(c)) if c == core => {
                self.lock_owner = None;
                true
            }
            (false, _) => true,
        }
    }

    #[inline(always)]
    fn tick(&mut self) -> bool {
        self.periph.tick()
    }
    #[inline(always)]
    fn irq_pending(&self) -> bool {
        self.periph.any_pending()
    }
    fn jit_mem(&mut self) -> Option<fm1_core::JitMem> {
        Some(fm1_core::JitMem {
            ram: self.ram.as_mut_ptr(),
            ram_base: RAM_BASE,
            ram_len: self.ram.len() as u32,
            ram_code: self.ram.code_ptr(),
            xip: self.xip.as_ptr(),
            xip_base: XIP_BASE,
            xip_len: self.xip.len() as u32,
        })
    }
    fn thread_mem(&mut self) -> Option<fm1_core::JitMem> { self.jit_mem() }
    fn code_writes(&self) -> Option<u64> {
        Some(self.ram.code_writes.wrapping_add(self.xip_version))
    }
    fn xip_writes(&self) -> u64 { self.xip_version }
    fn xip_mark(&mut self, lo: u32, hi: u32) {
        let Some(lo) = lo.checked_sub(XIP_BASE) else { return };
        let Some(hi) = hi.checked_sub(XIP_BASE) else { return };
        let lo = (lo as usize + XIP_FLASH_OFFSET).min(spiflash::FLASH_SIZE);
        let hi = (hi as usize + XIP_FLASH_OFFSET).min(spiflash::FLASH_SIZE);
        if lo < hi { self.xip_code[lo / 4..hi.div_ceil(4)].fill(1); }
    }
    fn code_mark(&mut self, lo: u32, hi: u32) {
        self.ram.mark_code(lo.wrapping_sub(RAM_BASE), hi.wrapping_sub(RAM_BASE));
    }
    #[inline]
    fn tick_room(&self) -> u64 {
        self.periph.tick_room()
    }
    #[inline]
    fn add_ticks(&mut self, n: u64) {
        self.periph.cycles += n;
    }
    #[inline]
    fn next_tick_due(&self) -> bool {
        self.periph.next_tick_due()
    }

    // RAM fast paths: same results as the region dispatch below (byte and
    // halfword accesses select within the aligned word), without walking it

    #[inline(always)]
    fn read8(&mut self, addr: u32) -> u8 {
        let o = addr.wrapping_sub(RAM_BASE);
        if (o as usize) < self.ram.len() {
            return self.ram.read8(o);
        }
        self.read32(addr & !3).to_le_bytes()[(addr & 3) as usize]
    }

    #[inline(always)]
    fn read16(&mut self, addr: u32) -> u16 {
        // halfwords sit at offsets 0 and 2 of the word (the old `>> 8` for
        // offset 2 returned bytes 1-2 and sheared every 16-bit pixel canvas)
        let v = self.read32(addr & !3);
        (v >> ((addr & 2) * 8)) as u16
    }

    #[inline(always)]
    fn read32(&mut self, addr: u32) -> u32 {
        let o = addr.wrapping_sub(RAM_BASE) as usize;
        if o + 4 <= self.ram.len() {
            return self.ram.read32(o as u32);
        }
        let x = addr.wrapping_sub(XIP_BASE) as usize;
        if x + 4 <= self.xip.len() {
            return u32::from_le_bytes([self.xip[x], self.xip[x + 1], self.xip[x + 2], self.xip[x + 3]]);
        }
        self.read32_slow(addr)
    }

    #[inline(always)]
    fn write8(&mut self, addr: u32, value: u8) {
        let o = addr.wrapping_sub(RAM_BASE);
        if (o as usize) < self.ram.len() {
            return self.ram.write8(o, value);
        }
        self.write8_slow(addr, value)
    }

    #[inline(always)]
    fn write16(&mut self, addr: u32, value: u16) {
        // merged at (addr & 2) within the aligned word, as the slow path does
        let o = (addr & !1).wrapping_sub(RAM_BASE) as usize;
        if o + 2 <= self.ram.len() && addr & 1 == 0 {
            return self.ram.write16(o as u32, value);
        }
        self.write16_slow(addr, value)
    }

    #[inline(always)]
    fn write32(&mut self, addr: u32, value: u32) {
        let o = addr.wrapping_sub(RAM_BASE) as usize;
        if o + 4 <= self.ram.len() {
            return self.ram.write32(o as u32, value);
        }
        self.write32_slow(addr, value)
    }
}

impl Soc {
    #[inline]
    fn profile_mmio(&mut self, write: bool, addr: u32) {
        if let Some(counts) = self.mmio_profile.as_mut() {
            if addr < 0x0100_0000 || (0x1eee000..=0x1eef3ff).contains(&addr) {
                *counts.entry((write, addr)).or_default() += 1;
            }
        }
    }

    /// CPU1 does not own the shared clock. Preserve TIMER4's value and read
    /// accounting, while leaving CPU0's polling streak and idle heuristic alone.
    pub(crate) fn read8_clock_frozen(&mut self, addr: u32) -> u8 {
        if addr & !3 == 0x10804 {
            (self.read32_clock_frozen(addr & !3) >> ((addr & 3) * 8)) as u8
        } else { self.read8(addr) }
    }

    pub(crate) fn read16_clock_frozen(&mut self, addr: u32) -> u16 {
        if addr & !3 == 0x10804 {
            (self.read32_clock_frozen(addr & !3) >> ((addr & 2) * 8)) as u16
        } else { self.read16(addr) }
    }

    pub(crate) fn read32_clock_frozen(&mut self, addr: u32) -> u32 {
        if addr == 0x10804 {
            self.profile_mmio(false, addr);
            self.periph.timer4_count()
        } else { self.read32(addr) }
    }

    pub fn mmio_profile_json(&self) -> String {
        let mut counts: Vec<_> = self.mmio_profile.iter().flat_map(|m| m.iter()).collect();
        counts.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        format!("[{}]", counts.into_iter().take(32).map(|(&(write, addr), count)|
            format!("{{\"write\":{write},\"address\":{addr},\"count\":{count}}}")
        ).collect::<Vec<_>>().join(","))
    }

    #[inline(never)]
    fn read32_slow(&mut self, addr: u32) -> u32 {
        self.profile_mmio(false, addr);
        self.read32_routed::<true>(addr)
    }

    // The false instantiation keeps the original dispatch order as a test oracle.
    // Periph::read is a no-op for addresses outside its modeled register set.
    #[inline(always)]
    fn read32_routed<const PERIPHERAL_FIRST: bool>(&mut self, addr: u32) -> u32 {
        // Preserve the map's RAM-first priority even for unusual oversized RAM
        // views that overlap the high interrupt-controller register window.
        if PERIPHERAL_FIRST && addr.wrapping_sub(RAM_BASE) >= self.ram.len() as u32 {
            if let Some(value) = self.periph.read(addr) { return value; }
        }
        match self.region(addr) {
            Region::Xip => {
                let o = (addr - XIP_BASE) as usize;
                let g = |i: usize| self.xip.get(i).copied().unwrap_or(0);
                u32::from_le_bytes([g(o), g(o + 1), g(o + 2), g(o + 3)])
            }
            Region::Ram => self.ram.read32_bounded(addr - RAM_BASE),
            Region::XipFlash => {
                let o = (addr - XIP_BASE) as usize + XIP_FLASH_OFFSET;
                let g = |i: usize| self.flash.mem.get(i).copied().unwrap_or(0xff);
                u32::from_le_bytes([g(o), g(o + 1), g(o + 2), g(o + 3)])
            }
            Region::Ovl => self.ovl.read32_bounded(addr - OVL_BASE),
            Region::Sram2 => self.sram2.read32_bounded(addr - SRAM2_BASE),
            Region::Irqc => {
                // 0x1eee008 bit 14: controller ready (the early init spins on it);
                // 0x1eee000 / 0x1eee004 (C0_CON / C1_CON): see `core_paused`
                let v = self.sfr_get(addr);
                match addr {
                    0x1eee008 => v | 0x4000,
                    0x1eee000 | 0x1eee004 => {
                        let core = ((addr >> 2) & 1) as usize;
                        (v & !0x14) | if self.paused[core] { 0x10 } else { 0 }
                    }
                    _ => v,
                }
            }
            // (the second USB PHY ready: bit 4 of its control word, once it is powered up)
            Region::Usb if addr == 0x16800 => self.sfr_get(addr) | 0x10,
            Region::Iomap | Region::Usb | Region::Clk => self.sfr_get(addr),
            // JL_ANA (analog: WLA, DAC trim). 0x11978 is the DAC trim measurement after the
            // strobe at the same word: bit 5 = conversion done, bits 8-15 = the value (mid
            // scale here), bit 18 = "this trim step is right". With these the stock
            // firmware's audio_ana calibration finishes at once instead of polling forever
            // (a trim only nudges the DAC's DC offset).
            Region::Ana => {
                let v = self.sfr_get(addr);
                if addr == 0x11978 { (v & !0xff00) | 0x4_0000 | 0x8000 | 0x20 } else { v }
            }
            // the Bluetooth baseband (bsfr 0x20000; no radio here): reads 0, writes are
            // dropped (self-clearing requests like bt_clkn_compare_arm's capture at 0x2001C
            // must read done), except 0x2FC44, the base of the RAM window its buffers live in,
            // which the BT init writes (its .bd_base, which opens the BT bss, SDK
            // btctler_lib_bss.ld). The stock BT stack checks buffers against it: descriptors
            // within 2 KiB and below its own .bd_base + 1088, bulk buffers within 32 KiB; with
            // V13's base kept, the SMK-37 firmware's check failed and it reset.
            Region::Bt => {
                if addr == 0x2fc44 { self.bt_window } else { 0 }
            }
            Region::Gpio | Region::Adc | Region::Alnk | Region::Irqc2 | Region::Uart1 => {
                if PERIPHERAL_FIRST { self.sfr_get(addr) }
                else { self.periph.read(addr).unwrap_or_else(|| self.sfr_get(addr)) }
            }
            Region::Timer => {
                if PERIPHERAL_FIRST { self.timer_read(addr) }
                else { self.periph.read(addr).unwrap_or_else(|| self.timer_read(addr)) }
            }
            Region::Spi0 => self.flash.read(addr),
            Region::P33 => self.p33.read(addr),
            // SPI2: transfers finish at once (CON bit 15 reads done)
            Region::Spi2 => self.sfr_get(addr) | if addr == 0x11e00 { 0x8000 } else { 0 },
            Region::Spi1 => {
                if addr == 0x11d00 {
                    // CON: transfer-done (the FM-1: always, so polls finish; the SMK-37: once the
                    // bytes have gone out)
                    let busy = self.board == Board::Smk37 && self.periph.cycles < self.periph.spi1_done_at;
                    self.sfr_get(addr) | if busy { 0 } else { 0x8000 }
                } else if addr == 0x11d10 {
                    self.sfr_get(addr)
                } else {
                    self.sfr_get(addr)
                }
            }
            Region::Dbg => self.sfr_get(addr),
            Region::Crc => if addr == 0x13504 { self.crc as u32 } else { 0 },
            // JL_RAND (0x13B00 R64L, 0x13B04 R64H): a new number at every read, from a fixed
            // seed so runs repeat. Read as 0 it hung the stock firmware ~8 s after boot: a
            // routine in its audio driver parks the task when a random word equals a fixed
            // descrambled one, which real hardware never hits.
            Region::Rand => {
                let mut x = self.rand;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                self.rand = x;
                (if addr == 0x13b00 { x } else { x >> 32 }) as u32
            }
            Region::Unknown => {
                self.log_unknown(addr, 4, false, 0);
                0
            }
        }
    }

    #[inline(never)]
    fn write8_slow(&mut self, addr: u32, value: u8) {
        let cur = self.read32(addr & !3);
        let shift = (addr & 3) * 8;
        let mask = !(0xffu32 << shift);
        let merged = (cur & mask) | ((value as u32) << shift);
        self.write32(addr & !3, merged);
    }

    #[inline(never)]
    fn write16_slow(&mut self, addr: u32, value: u16) {
        let cur = self.read32(addr & !3);
        let shift = (addr & 2) * 8;
        let mask = !(0xffffu32 << shift);
        let merged = (cur & mask) | ((value as u32) << shift);
        self.write32(addr & !3, merged);
    }

    #[inline(never)]
    fn write32_slow(&mut self, addr: u32, value: u32) {
        self.profile_mmio(true, addr);
        match self.region(addr) {
            Region::Xip | Region::XipFlash => self.log_unknown(addr, 4, true, value), // flash writes go via the ROM
            Region::Ram => self.ram.write32_bounded(addr - RAM_BASE, value),
            Region::Ovl => self.ovl.write32_bounded(addr - OVL_BASE, value),
            Region::Sram2 => self.sram2.write32_bounded(addr - SRAM2_BASE, value),
            Region::Irqc => {
                if addr == 0x1eee000 || addr == 0x1eee004 {
                    // C0_CON / C1_CON: writing bit 2 pauses that core (bit 4 reads "paused");
                    // a write with bit 3 and without bit 2 runs it again. Both are pulses:
                    // the stock IPC pauses with `|= 4` and resumes with `|= 8` on the same word.
                    let core = ((addr >> 2) & 1) as usize;
                    let previous = self.sfr_get(addr);
                    if value & 4 != 0 || (core == 1 && value & 2 != 0) {
                        self.paused[core] = true;
                    } else if value & 8 != 0 {
                        self.paused[core] = false;
                    }
                    // The SDK also releases CPU1 through C1_CON, after writing
                    // its entry mailbox: set bit3 and deassert reset bit1.
                    // Ordinary resumes of an existing CPU must not relatch the
                    // startup event or make run_blocks stop forever.
                    if core == 1 && !self.cpu1_started && value & 0xa == 8
                        && (previous & 8 == 0 || previous & 2 != 0) {
                        self.cpu1_release = true;
                        self.periph.next_due = 0;
                    }
                    self.sfr_set(addr, value & !0x14);
                } else {
                    self.sfr_set(addr, value);
                }
            }
            Region::Gpio => {
                if let Some(h) = self.gpio_writes.as_mut() {
                    *h.entry(addr).or_insert(0) += 1;
                }
                // PD0 drives the flash chip select; PA1/PA3/PA4 clock the
                // key-matrix shift registers
                if addr == 0x500c0 {
                    self.flash.set_cs(value & 1 == 0);
                    self.sync_xip();
                }
                self.periph.write(addr, value);
                self.sfr_set(addr, value)
            }
            Region::Alnk | Region::Irqc2 => {
                if !self.periph.write(addr, value) {
                    self.sfr_set(addr, value);
                }
            }
            Region::Spi0 => {
                self.flash.write(addr, value);
                self.sync_xip();
            }
            Region::P33 => self.p33.write(addr, value),
            Region::Iomap => self.sfr_set(addr, value),
            Region::Ana => self.sfr_set(addr, value),
            Region::Bt => {
                if addr == 0x2fc44 {
                    self.bt_window = value;
                }
            }
            Region::Clk => {
                if addr == 0x10008 && !self.cpu1_started && value & 8 != 0 && self.sfr_get(addr) & 8 == 0 {
                    self.cpu1_release = true;
                    self.periph.next_due = 0; // the runner looks at it before the next instruction
                }
                self.sfr_set(addr, value)
            }
            Region::Usb => {
                if addr == 0x16a0c && self.board == Board::Smk37 {
                    self.periph.smk.commons = value;
                }
                self.sfr_set(addr, value)
            }
            Region::Uart1 => {
                if !self.periph.write(addr, value) {
                    self.sfr_set(addr, value);
                }
            }
            Region::Adc => {
                if !self.periph.write(addr, value) {
                    self.sfr_set(addr, value);
                }
            }
            Region::Timer => {
                if !self.periph.write(addr, value) {
                    self.sfr_set(addr, value);
                }
            }
            Region::Spi1 => self.spi1_write(addr, value),
            Region::Spi2 => self.spi2_write(addr, value),
            // the CRC-16 unit (chip_crc16): [+4] = the register (the SDK seeds it with 0),
            // each write to [+0] feeds one byte through CRC-16/CCITT (0x1021, MSB first).
            // The stock NOR file system checks every directory entry with it.
            Region::Rand => {}
            Region::Crc => match addr {
                0x13500 => {
                    let mut c = self.crc ^ ((value as u16 & 0xff) << 8);
                    for _ in 0..8 {
                        c = if c & 0x8000 != 0 { (c << 1) ^ 0x1021 } else { c << 1 };
                    }
                    self.crc = c;
                }
                0x13504 => self.crc = value as u16,
                _ => {}
            },
            Region::Dbg => {
                match dbg_line(addr) {
                    "msg" => {
                        self.console.extend_from_slice(&value.to_le_bytes());
                    }
                    _ => {}
                }
                self.sfr_set(addr, value);
            }
            Region::Unknown => self.log_unknown(addr, 4, true, value),
        }
    }
}

/// M-VAVE's stock firmware out of its update package: the XIP window's contents (the app
/// area, unscrambled), the whole flash (the image as written, grown to the chip's size, the
/// USR patch area given this emulator's default bank when blank and then an optional DX7
/// bank in slots 1-32) and the entry point.
pub struct Stock {
    pub xip: Vec<u8>,
    pub flash: Vec<u8>,
    pub entry: u32,
    /// The board the package is for (its identity), with its identity.
    pub board: Board,
    pub identity: String,
}

pub fn stock_from_package(pkg: &[u8], syx: Option<&[u8]>) -> Result<Stock, String> {
    let u = fwsc::unpack(pkg)?;
    let board = Board::of_identity(&u.identity);
    let mut flash = fwsc::full_flash(&u.raw, FLASH_SIZE);
    if board == Board::Smk37 {
        // a unit's factory calibration record, with the stock defaults
        fwsc::smk_trim_fill_defaults(&mut flash);
    }
    if let Some(usr) = fwsc::usr_offset(&u).filter(|_| board == Board::Fm1) {
        fwsc::usr_fill_defaults(&mut flash, usr);
        if let Some(b) = syx {
            fwsc::usr_load_syx(&mut flash, usr, b, 0)?;
        }
    }
    let app = u.entry("app.bin").map(|h| h.offset).unwrap_or(0x120);
    Ok(Stock { xip: u.xip, flash, entry: XIP_BASE + app as u32, board, identity: u.identity })
}

/// Ready a Soc for a package's firmware: its board, its flash chip, and the SPL's boot
/// parameters at `params`. Returns the value for r0.
pub fn stock_boot(soc: &mut Soc, flash: &[u8], params: u32, board: Board) -> u32 {
    soc.set_board(board);
    soc.flash.load(flash);
    soc.xip_flash = true;
    write_boot_info(soc, params, flash)
}

/// The boot-parameter block the SPL (uboot.boot) hands the stock app in r0, as the SDK's
/// `BOOT_DEVICE_INFO` (include_lib/system/boot.h) and the SPL's own code at 0x31B2..0x3200
/// build it: fs_info (-> the flash header copied right after the struct), sfc_base_addr (the
/// flash offset behind XIP 0x02000000: 0x4000), app_addr 0x02000000, chip_id, trim_value, an
/// 8-byte MAC, then the 32-byte flash header unmasked (CRC-16 keystream from 0xFFFF; its CRC
/// checks). The app copies parts of it to 0x01C7FD50 (boot_hwinfo_save) and its NOR file
/// system finds the flash from sfc_base_addr. Writes the block at `at` (RAM); returns `at`.
pub fn write_boot_info(soc: &mut Soc, at: u32, flash: &[u8]) -> u32 {
    let mut head = [0u8; 32];
    head.copy_from_slice(&flash[..32]);
    let mut key: u16 = 0xffff;
    for b in head.iter_mut() {
        *b ^= key as u8;
        key = (key << 1) ^ if key & 0x8000 != 0 { 0x1021 } else { 0 };
    }
    let words = [at + 24, 0x4000, XIP_BASE, 0, 0, 0];
    for (i, w) in words.iter().enumerate() {
        soc.write32(at + 4 * i as u32, *w);
    }
    for (i, b) in head.iter().enumerate() {
        soc.write8(at + 24 + i as u32, *b);
    }
    at
}

#[cfg(test)]
mod irq_window_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_tail_words_are_padded_and_clipped_without_losing_code_epochs() {
        for region in 0..3 { for tail in 1..=3u32 {
            let mut soc=Soc::new(64,vec![]);
            let (base,length)=match region {0=>(RAM_BASE,soc.ram.len()),1=>(OVL_BASE,soc.ovl.len()),_=>(SRAM2_BASE,soc.sram2.len())};
            let address=base+length as u32-tail;
            for index in 0..tail {soc.write8(address+index,0xa0+index as u8);}
            let bytes=[0xa0,if tail>1{0xa1}else{0},if tail>2{0xa2}else{0},0];
            assert_eq!(soc.read32(address),u32::from_le_bytes(bytes));
            let ram=match region {0=>&mut soc.ram,1=>&mut soc.ovl,_=>&mut soc.sram2};
            ram.mark_code(length as u32-tail,length as u32);let epoch=ram.code_writes;
            soc.write32(address,0x12345678);
            for (index,byte) in [0x78,0x56,0x34].into_iter().take(tail as usize).enumerate() {
                assert_eq!(soc.read8(address+index as u32),byte);
            }
            let ram=match region {0=>&soc.ram,1=>&soc.ovl,_=>&soc.sram2};
            assert!(ram.code_writes>epoch);
            assert_eq!(soc.read8(base+length as u32-tail-1),0);
        }}
        let mut soc=Soc::new(65,vec![]);soc.write8(RAM_BASE+64,0xab);
        assert_eq!(soc.read16(RAM_BASE+64),0xab);
        soc.write16(RAM_BASE+64,0x1234);assert_eq!(soc.read8(RAM_BASE+64),0x34);
        for address in [u32::MAX-2,u32::MAX-1,u32::MAX] {
            assert_eq!(soc.read32(address),0);soc.write32(address,0x12345678);
        }
        assert_eq!(soc.read8(RAM_BASE+64),0x34);
    }

    #[test]
    fn alnk_rx_dma_handles_region_edges_and_wrapped_addresses() {
        for width in [2,4] { for address in [RAM_BASE+62,u32::MAX-1] {
            let mut soc=Soc::new(64,vec![]);
            for i in 0..64 {soc.ram.write8(i,0xa5);}
            soc.ram.mark_code(62,64);let epoch=soc.ram.code_writes;
            for (a,v) in [(0x12e10,address),(0x12e20,2),(0x12e04,if width==4{0xd}else{9}),(0x12e00,0x800)] {
                soc.write32(a,v);
            }
            soc.advance(5440);
            for i in 0..64 {assert_eq!(soc.ram.read8(i),if address==RAM_BASE+62&&i>=62{0}else{0xa5});}
            assert_eq!(soc.ram.code_writes>epoch,address==RAM_BASE+62);
            assert_eq!(soc.read32(0x12e08),0x10);
            assert!(soc.periph.audio.is_empty());
        }}
    }

    #[test]
    fn alnk_rx_dma_writes_silence_and_tx3_keeps_one_exact_stream() {
        for width in [2u32, 4] {
            let mut soc = Soc::new(4096, vec![]);
            let (rx, tx, words) = (RAM_BASE + 0x100, RAM_BASE + 0x200, 6u32);
            let bytes = 2 * words * width;
            for i in 0..bytes { soc.write8(rx+i, 0xa5); }
            soc.write8(rx-1, 0x69); soc.write8(rx+bytes, 0x96);
            for i in 0..2*words {
                if width == 2 { soc.write16(tx+i*width, (i as i16 * -1000) as u16); }
                else { soc.write32(tx+i*width, ((i as i32 * -1000) << 8) as u32); }
            }
            soc.ram.mark_code(0x100, 0x100 + bytes);
            let before = soc.ram.code_writes;
            for (a,v) in [(0x12e10,rx),(0x12e1c,tx),(0x12e20,words),
                (0x12e04,if width==4 {0x500d}else{0x1009}),
                (0x1eef104,9<<12),(0x12e00,0x800)] { soc.write32(a,v); }
            let period = words as u64 / 2 * 544 * 10;
            soc.advance(period - 1); assert!(soc.periph.audio.is_empty());
            assert_eq!(soc.ram.code_writes, before);
            soc.advance(1);
            assert_eq!(soc.periph.audio, (0..words).map(|i| i as i16 * -1000).collect::<Vec<_>>());
            assert_eq!(soc.read32(0x12e08), 0x90);
            assert_eq!(soc.pending_irq(), Some((11,4)));
            assert!(soc.ram.code_writes > before);
            for i in 0..words*width { assert_eq!(soc.read8(rx+i),0); }
            for i in words*width..bytes { assert_eq!(soc.read8(rx+i),0xa5); }
            soc.write32(0x12e08,9); assert!(soc.pending_irq().is_none());
            soc.advance(period);
            assert_eq!(soc.periph.audio.len(),2*words as usize);
            assert_eq!(soc.periph.audio, (0..2*words).map(|i| i as i16 * -1000).collect::<Vec<_>>());
            for i in 0..bytes { assert_eq!(soc.read8(rx+i),0); }
            assert_eq!((soc.read8(rx-1),soc.read8(rx+bytes)),(0x69,0x96));
            assert_eq!(soc.periph.audio_halves,2);
            // Removing RX preserves the old TX3-only pending mask, count and clock.
            soc.write32(0x12e08,15); soc.write32(0x12e04,if width==4 {0x5000}else{0x1000});
            soc.write8(rx,0x42); soc.advance(period);
            assert_eq!(soc.periph.audio.len(),3*words as usize);
            assert_eq!(soc.read32(0x12e08),0x80); assert_eq!(soc.read8(rx),0x42);
            soc.write32(0x12e08,15); soc.write32(0x12e04,0); soc.advance(period);
            assert_eq!(soc.periph.audio.len(),3*words as usize);
            assert_eq!(soc.read32(0x12e08),0);
        }
    }

    #[test]
    fn alnk_rx_dma_invalidates_both_warmed_ram_code_caches() {
        for jit in [false,true] {
            let mut soc = Soc::new(4096,vec![]);
            soc.write32(RAM_BASE+0x100,0x0000_2a40); // r0=10 followed by nop.
            let mut cpus = [fm1_core::Cpu::new(RAM_BASE+0x100),fm1_core::Cpu::new(RAM_BASE+0x100)];
            cpus[1].core_id=1;
            for cpu in &mut cpus { cpu.jit_on=jit; for _ in 0..64 {
                cpu.pc=RAM_BASE+0x100; cpu.regs[0]=0;
                assert_eq!(cpu.run_fast(&mut soc,1).unwrap().0,1); assert_eq!(cpu.regs[0],10);
            }}
            #[cfg(all(target_arch="aarch64",any(target_os="macos",target_os="android")))]
            if jit { for cpu in &cpus { assert!(cpu.jit_insns>0); } }
            let epoch=soc.ram.code_writes;
            for (a,v) in [(0x12e10,RAM_BASE+0x100),(0x12e20,2),(0x12e04,0xd),(0x12e00,0x800)] {soc.write32(a,v);}
            soc.advance(5440);
            assert!(soc.ram.code_writes>epoch);
            assert!(soc.periph.audio.is_empty()); // Input DMA cannot manufacture output.
            assert_eq!(soc.read32(0x12e08),0x10);
            for cpu in &mut cpus {
                cpu.pc=RAM_BASE+0x100;cpu.regs[0]=0;
                assert_eq!(cpu.run_fast(&mut soc,1).unwrap().0,1);assert_eq!(cpu.regs[0],0);
            }
        }
    }

    #[test]
    fn register_page_dispatch_matches_original_ranges_and_memory_priority() {
        for board in [Board::Fm1, Board::Smk37] {
            for ram_size in [65536, (XIP_BASE - RAM_BASE + 0x10000) as usize] {
                let mut soc = Soc::new(ram_size, vec![0xaa; 0x10000]);
                soc.set_board(board);
                for mapped in [false, true] {
                    soc.xip_flash = mapped;
                    for addr in 0..0x60000 {
                        assert_eq!(soc.region(addr), soc.region_reference(addr), "{board:?} {addr:#x}");
                    }
                    let mut addresses = vec![u32::MAX, 0x01000000, 0x1eee000, 0x1eee100,
                        0x1eee240, 0x1eee250, 0x1eef000, 0x1eef400];
                    for base in [RAM_BASE, XIP_BASE, SRAM2_BASE, OVL_BASE,
                        RAM_BASE + soc.ram.len() as u32, XIP_BASE + soc.xip.len() as u32,
                        XIP_BASE + (FLASH_SIZE - XIP_FLASH_OFFSET) as u32,
                        SRAM2_BASE + soc.sram2.len() as u32, OVL_BASE + soc.ovl.len() as u32] {
                        addresses.extend((0..8).map(|n| base.wrapping_sub(4).wrapping_add(n)));
                    }
                    addresses.extend(0x1eee000..0x1ef0000);
                    for addr in addresses {
                        assert_eq!(soc.region(addr), soc.region_reference(addr), "{board:?} {addr:#x}");
                    }
                }
            }
        }
    }

    #[test]
    fn live_flash_restore_invalidates_both_warmed_cpu_caches() {
        for jit in [false, true] {
            let mut code = vec![0; 128];
            for bytes in code.chunks_exact_mut(2) { bytes.copy_from_slice(&0x2a40u16.to_le_bytes()); }
            let mut soc = Soc::new(4096, code.clone());
            soc.flash.mem[XIP_FLASH_OFFSET..XIP_FLASH_OFFSET + code.len()].copy_from_slice(&code);
            let mut cpus = [fm1_core::Cpu::new(XIP_BASE), fm1_core::Cpu::new(XIP_BASE)];
            cpus[1].core_id = 1;
            for (core, cpu) in cpus.iter_mut().enumerate() {
                cpu.jit_on = jit;
                for _ in 0..128 {
                    cpu.pc = XIP_BASE;
                    let retired = if core == 0 { cpu.run_fast(&mut soc, 1).unwrap().0 }
                        else {
                            let mut bus = machine::Core1Bus { soc: &mut soc, budget: 1025, observe_irq: false };
                            cpu.run_fast(&mut bus, 1).unwrap().0
                        };
                    assert_eq!(retired, 1);
                    assert_eq!(cpu.regs[0], 10);
                }
            }
            #[cfg(all(target_arch = "aarch64", any(target_os = "macos", target_os = "android")))]
            if jit { assert!(cpus.iter().all(|cpu| cpu.jit_entries > 0)); }
            let mut image = soc.flash.mem.clone();
            image[XIP_FLASH_OFFSET..XIP_FLASH_OFFSET + 2].copy_from_slice(&0x2440u16.to_le_bytes());
            let before = soc.xip_writes();
            assert!(soc.restore_flash(&image));
            assert_eq!(soc.xip_writes(), before + 1);
            assert_eq!(soc.read16(XIP_BASE), 0x2440);
            cpus[0].pc = XIP_BASE;
            cpus[0].step(&mut soc).unwrap();
            cpus[1].pc = XIP_BASE;
            let mut bus = machine::Core1Bus { soc: &mut soc, budget: 1025, observe_irq: false };
            assert_eq!(cpus[1].run_fast(&mut bus, 1).unwrap().0, 1);
            assert!(cpus.iter().all(|cpu| cpu.regs[0] == 4 && cpu.xip_invalidations == 1));
            assert!(soc.restore_flash(&image));
            assert_eq!(soc.xip_writes(), before + 1, "identical restore must retain caches");
        }
    }

    #[test]
    fn flash_restore_preserves_unchanged_decoded_bytes_and_rejects_lengths_before_mutation() {
        let mut soc = Soc::new(4096, vec![0x42; 128]);
        soc.flash.mem.fill(0xa5); // Raw package bytes can differ from decoded XIP.
        soc.xip_mark(XIP_BASE, XIP_BASE + 8);
        let mut image = soc.flash.mem.clone();
        image[XIP_FLASH_OFFSET + 64] = 0x12; // Unwatched data within the direct image.
        image[0x90000] = 0x34; // Unwatched flash-backed continuation.
        assert!(soc.restore_flash(&image));
        assert_eq!(&soc.xip[..8], &[0x42; 8]);
        assert_eq!(soc.xip[64], 0x12);
        assert_eq!(soc.read8(XIP_BASE + 0x90000 - XIP_FLASH_OFFSET as u32), 0x34);
        assert_eq!(soc.xip_writes(), 0);
        image[XIP_FLASH_OFFSET] = 0x42;
        assert!(soc.restore_flash(&image));
        assert_eq!(soc.xip_writes(), 0, "a changed physical byte with an identical mapped value must retain caches");
        let xip = soc.xip.clone();
        let flash = soc.flash.mem.clone();
        for invalid in [&image[..image.len() - 1], &[0x55][..]] {
            assert!(!soc.restore_flash(invalid));
            assert_eq!(soc.xip, xip);
            assert_eq!(soc.flash.mem, flash);
            assert_eq!(soc.xip_writes(), 0);
        }
        image.push(0);
        assert!(!soc.restore_flash(&image));
        assert_eq!(soc.xip, xip);
        assert_eq!(soc.flash.mem, flash);
        assert_eq!(soc.xip_writes(), 0);
    }

    #[test]
    fn restore_tracks_watched_flash_continuation_and_leaves_unmapped_xip_immutable() {
        let mut soc = Soc::new(4096, vec![0x42; 128]);
        let physical = XIP_FLASH_OFFSET + 0x1000;
        soc.xip_mark(XIP_BASE + 0x1000, XIP_BASE + 0x1008);
        let mut image = soc.flash.mem.clone();
        image[physical] ^= 1;
        assert!(soc.restore_flash(&image));
        assert_eq!(soc.xip_writes(), 1);
        assert_eq!(soc.read8(XIP_BASE + 0x1000), image[physical]);
        soc.xip_flash = false;
        let xip = soc.xip.clone();
        image[physical] ^= 1;
        image[XIP_FLASH_OFFSET] ^= 1;
        assert!(soc.restore_flash(&image));
        assert_eq!(soc.xip_writes(), 1);
        assert_eq!(soc.xip, xip);
    }

    fn flash_command(soc: &mut Soc, cmd: &[u8]) {
        soc.write32(0x500c0, 0);
        for &b in cmd { soc.write32(spiflash::SPI0_BASE + 8, b as u32); }
        soc.write32(0x500c0, 1);
    }

    #[test]
    fn peripheral_first_reads_preserve_routing_values_side_effects_and_boundaries() {
        for board in [Board::Fm1, Board::Smk37] {
            let create = || {
                let mut soc = Soc::new(1 << 16, vec![0xaa; 64]);
                soc.set_board(board);
                for (addr, value) in [(0x10800, 1), (0x10908, 1000), (0x10900, 1),
                    (0x1eef0f4, 2_400_000), (0x1eef0ec, 1), (0x50000, 17),
                    (0x11d00, 0x2000), (0x12100, 1), (0x13100, 0x40)] {
                    soc.write32(addr, value);
                }
                soc.advance(12345);
                soc
            };
            let mut fast = create(); let mut reference = create();
            // Every modeled register block, unmodeled offsets, widths at the
            // dispatch boundaries, RNG side effects, and partial XIP reads.
            let mut addresses = vec![0x1eee008, 0x1eee000, 0x1eee004, 0x1eee240,
                RAM_BASE, OVL_BASE, SRAM2_BASE, XIP_BASE + 63, XIP_BASE + 64];
            for base in [0x10000, 0x10400, 0x10500, 0x10600, 0x10700, 0x10800,
                0x10900, 0x11800, 0x11900, 0x11c00, 0x11d00, 0x11e00, 0x12100,
                0x12e00, 0x13100, 0x13500, 0x13b00, 0x16800, 0x16a00,
                0x20000, 0x2fc00, 0x50000, 0x50040, 0x501c0, 0x51000,
                0x1eef0e0, 0x1eef100, 0x1eef1a0, 0x1eef300] {
                addresses.extend((0..=0x44).map(|off| base + off));
            }
            for pass in 0..3 {
                for &addr in &addresses {
                    assert_eq!(fast.read32_slow(addr), reference.read32_routed::<false>(addr),
                        "board={board:?} addr={addr:#x} pass={pass}");
                    assert_eq!((fast.periph.cycles, fast.periph.next_due, fast.rand),
                        (reference.periph.cycles, reference.periph.next_due, reference.rand));
                    assert_eq!(fast.periph.pending_irq_for(0), reference.periph.pending_irq_for(0));
                    assert_eq!(fast.periph.pending_irq_for(1), reference.periph.pending_irq_for(1));
                }
                fast.advance(37); reference.advance(37);
            }
            assert_eq!(fast.unknown_accesses(), reference.unknown_accesses());
            assert_eq!(fast.sfr, reference.sfr);
        }
        let mut overlap = Soc::new((0x1eef140 - RAM_BASE + 4) as usize, vec![]);
        overlap.write32(0x1eef100, 0xdead_beef);
        assert_eq!(overlap.read32_routed::<true>(0x1eef100), 0xdead_beef);
        assert_eq!(overlap.read32_routed::<false>(0x1eef100), 0xdead_beef);
    }

    #[test]
    fn wrapped_flash_program_still_invalidates_watched_xip() {
        let mut soc = Soc::new(1 << 16, vec![0; 128]);
        soc.xip[..2].copy_from_slice(&0x2a40u16.to_le_bytes());
        let mut cpu = fm1_core::Cpu::new(XIP_BASE);
        cpu.step(&mut soc).unwrap();
        flash_command(&mut soc, &[0x06]);
        let mut program = vec![0x02, 0x0f, 0xff, 0xff];
        program.resize(4 + XIP_FLASH_OFFSET + 2, 0);
        flash_command(&mut soc, &program);
        assert_eq!(soc.xip_writes(), 1);
        assert_eq!(soc.read8(XIP_BASE), 0);
    }

    #[test]
    fn data_only_flash_updates_keep_executable_caches() {
        let mut soc = Soc::new(1 << 16, vec![0; 0x2000]);
        for bytes in soc.xip.chunks_exact_mut(2) { bytes.copy_from_slice(&0x2a40u16.to_le_bytes()); }
        let mut cpu = fm1_core::Cpu::new(XIP_BASE);
        cpu.run_fast(&mut soc, 1).unwrap();
        let decoded = cpu.decodes;
        flash_command(&mut soc, &[0x06]);
        flash_command(&mut soc, &[0x20, 0x00, 0x50, 0x00]);
        flash_command(&mut soc, &[0x06]);
        flash_command(&mut soc, &[0x02, 0x00, 0x50, 0x00, 0x12, 0x34]);
        assert_eq!(soc.read32(XIP_BASE + 0x1000), 0xffff_3412);
        cpu.pc = XIP_BASE;
        cpu.run_fast(&mut soc, 1).unwrap();
        assert_eq!((soc.xip_writes(), cpu.xip_invalidations, cpu.decodes), (0, 0, decoded));
    }

    #[test]
    fn xip_marks_include_decode_lookahead_and_cpu1_if_extent_scans() {
        for extent in [false, true] {
            let mut soc = Soc::new(1 << 16, vec![0; 128]);
            for bytes in soc.xip.chunks_exact_mut(2) { bytes.copy_from_slice(&0x2a40u16.to_le_bytes()); }
            if extent {
                // The if head depends on all five body units even when none has executed.
                soc.xip[..4].copy_from_slice(&[0x1d, 0xea, 0x80, 0xa6]);
            }
            let mut cpu = fm1_core::Cpu::new(XIP_BASE);
            let mut bus = machine::Core1Bus { soc: &mut soc, budget: 100, observe_irq: false };
            cpu.step(&mut bus).unwrap();
            let byte = if extent { 15 } else { 7 }; // beyond the instruction's own bytes
            flash_command(&mut soc, &[0x06]);
            flash_command(&mut soc, &[0x02, 0x00, 0x40, byte, 0]);
            assert_eq!(soc.xip_writes(), 1, "extent={extent}");
        }
    }

    #[test]
    fn xip_generation_covers_flash_beyond_the_direct_image() {
        let mut soc = Soc::new(1 << 16, vec![0xaa; 128]);
        soc.xip_mark(XIP_BASE + 0x1000, XIP_BASE + 0x1008);
        flash_command(&mut soc, &[0x06]);
        flash_command(&mut soc, &[0x20, 0x00, 0x50, 0x00]);
        assert_eq!(soc.xip_writes(), 1);
        assert_eq!(soc.read32(XIP_BASE), 0xaaaa_aaaa);
        assert_eq!(soc.read32(XIP_BASE + 0x1000), 0xffff_ffff);
        soc.xip_flash = false;
        flash_command(&mut soc, &[0x06]);
        flash_command(&mut soc, &[0x20, 0x00, 0x40, 0x00]);
        assert_eq!(soc.xip_writes(), 1, "unmapped flash cannot change executable XIP");
    }

    #[test]
    fn programmed_xip_invalidates_both_cpu_caches_and_compiled_code() {
        for jit in [false, true] {
            let mut soc = Soc::new(1 << 16, vec![0; 128]);
            for bytes in soc.xip.chunks_exact_mut(2) { bytes.copy_from_slice(&0x2a40u16.to_le_bytes()); }
            let mut cpus = [fm1_core::Cpu::new(XIP_BASE), fm1_core::Cpu::new(XIP_BASE)];
            for cpu in &mut cpus {
                cpu.jit_on = jit;
                // Warm the block enough to exercise compiled-cache invalidation on
                // supported hosts, while the other CPU retains its independent cache.
                for _ in 0..64 {
                    cpu.pc = XIP_BASE;
                    assert_eq!(cpu.run_fast(&mut soc, 1).unwrap().0, 1);
                    assert_eq!(cpu.regs[0], 10);
                }
            }
            #[cfg(all(target_arch = "aarch64", any(target_os = "macos", target_os = "android")))]
            if jit { assert!(cpus.iter().all(|cpu| cpu.jit_entries > 0)); }
            let version = soc.code_writes();
            flash_command(&mut soc, &[0x06]);
            flash_command(&mut soc, &[0x20, 0x00, 0x40, 0x00]);
            flash_command(&mut soc, &[0x06]);
            let mut program = vec![0x02, 0x00, 0x40, 0x00];
            for _ in 0..64 { program.extend(0x2440u16.to_le_bytes()); }
            flash_command(&mut soc, &program);
            assert_ne!(version, soc.code_writes());
            assert_eq!(soc.xip_writes(), 2);
            cpus[0].pc = XIP_BASE;
            cpus[0].step(&mut soc).unwrap();
            cpus[1].pc = XIP_BASE;
            assert_eq!(cpus[1].run_fast(&mut soc, 1).unwrap().0, 1);
            assert!(cpus.iter().all(|cpu| cpu.regs[0] == 4));
        }
    }

    #[test]
    fn flash_writes_show_through_xip() {
        // a booted package's window (0x80000 bytes, flash 0x4000..0x84000): erasing the sector
        // at flash 0x74000 and programming two bytes there reads back through XIP 0x02070000,
        // as April OS's samples do
        let mut soc = Soc::new(1 << 16, vec![0xaa; 0x80000]);
        for cmd in [&[0x06][..], &[0x20, 0x07, 0x40, 0x00], &[0x06], &[0x02, 0x07, 0x40, 0x00, 0x12, 0x34]] {
            soc.write32(0x500c0, 0); // PD0 low: chip select
            for &b in cmd {
                soc.write32(spiflash::SPI0_BASE + 8, b as u32);
            }
            soc.write32(0x500c0, 1);
        }
        assert_eq!(soc.read32(0x0207_0000), 0xffff_3412);
        assert_eq!(soc.read32(0x0207_0ffc), 0xffff_ffff, "the rest of the sector erased");
        assert_eq!(soc.read32(0x0207_1000), 0xaaaa_aaaa, "the next sector as installed");
    }
}
