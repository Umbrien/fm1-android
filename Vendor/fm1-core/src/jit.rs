//! AArch64 JIT for the blocks of micro-ops `Cpu::run_fast` interprets (after esp32sim's
//! xtensa-lx7 JIT). A compiled block is its native-safe prefix, including audited flag
//! producers that other backends keep careful, with the interpreter as oracle: both leave
//! bit-identical machine state.
//!
//! Timing is kept exact the same way the interpreter does it. A block runs as native code only
//! when no tick inside it can be due (`Bus::tick_room`). Before any access that leaves the
//! inline RAM / XIP fast paths, the helper brings the clock to that instruction's tick; after
//! it, if the next tick would be due (a peripheral write, an idle skip), the block exits right
//! after the instruction, where the interpreter would have stopped as well.
//!
//! Blocks chain: an exit to a fixed pc jumps through a slot straight into the next block's
//! compiled body once that block is compiled (Rust fills the slot the first time the exit
//! returns there). Every body selects an unchecked full prefix or an instruction-bounded
//! prefix according to the call's tick limit; the helpers sync the clock with the running
//! instruction count, so timing stays exact across the chain.
//!
//! Register plan (AAPCS64 callee-saved, so helpers may be called freely):
//!   x19 = &Cpu, x20 = &Ctx, x21 = RAM host base, x22 = XIP host base,
//!   w23 / w24 = values kept across a helper call within one instruction, x25 = code flags,
//!   w26 = instructions done in this call before the current block, w27 = the limit
//!   (instructions this call may run). Guest registers, sp and rets stay in the Cpu struct;
//!   pc is stored at exit.

use crate::jit_a64::{Asm, Cond as AC, Label, Reg, SP};
use crate::{Base, Bus, Cond, Cpu, Fast, Op, RmwOp, SpecialReg};
use std::ffi::{c_int, c_void};
use std::time::Instant;
#[path = "jit_lower.rs"]
mod raw_lower;
#[path = "jit_fpr.rs"]
mod fpr;

/// Host view of the guest memory the inline paths read and write directly.
#[derive(Clone, Copy, Debug)]
pub struct JitMem {
    pub ram: *mut u8,
    pub ram_base: u32,
    pub ram_len: u32,
    /// RAM code-word flags: stores to watched words must pass through the bus.
    pub ram_code: *const u8,
    pub xip: *const u8,
    pub xip_base: u32,
    pub xip_len: u32,
}

/// Per-call context the helpers use (field offsets are used by generated code).
#[repr(C)]
pub struct Ctx {
    cpu: *mut Cpu,
    bus: *mut (),
    /// ticks of instruction `idx` = idx + extra (0: the first one was ticked by the caller)
    extra: u32,
    /// ticks already applied to the bus
    synced: u32,
    /// the next tick is due: leave after this instruction
    brk: u32,
    stop_pc: u32,
    /// the chain slot of the exit taken back to Rust (null: not a chainable exit)
    slot: *mut u64,
    code_version: Option<u64>,
    guard_version: u64,
    guard_tracked: u32,
    xip_version: u64,
    panic: Option<Box<dyn std::any::Any + Send>>,
}
const OFF_BRK: u32 = 24;
const OFF_STOP_PC: u32 = 28;
const OFF_SLOT: u32 = 32;
const _: () = assert!(std::mem::offset_of!(Ctx, brk) == OFF_BRK as usize);
const _: () = assert!(std::mem::offset_of!(Ctx, stop_pc) == OFF_STOP_PC as usize);
const _: () = assert!(std::mem::offset_of!(Ctx, slot) == OFF_SLOT as usize);

// ------------------------------------------------------------------ executable memory
extern "C" {
    fn mmap(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, off: i64) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> c_int;
}
#[cfg(target_os = "macos")]
extern "C" {
    fn pthread_jit_write_protect_np(enabled: c_int);
    fn sys_icache_invalidate(start: *mut c_void, len: usize);
}
#[cfg(target_os = "android")]
extern "C" {
    fn mprotect(addr: *mut c_void, len: usize, prot: c_int) -> c_int;
    fn getpagesize() -> c_int;
}

/// Clean data to the point of unification, invalidate instructions, then synchronize.
/// CTR_EL0 supplies each cache's line size; works on cores requiring both operations.
#[cfg(target_os = "android")]
unsafe fn flush_icache(start: *const u8, len: usize) {
    let ctr: u64;
    core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack));
    let end = start as usize + len;
    if ctr & (1 << 28) == 0 {
        let line = 4usize << ((ctr >> 16) & 15);
        let mut at = (start as usize) & !(line - 1);
        while at < end {
            core::arch::asm!("dc cvau, {}", in(reg) at, options(nostack));
            at += line;
        }
    }
    core::arch::asm!("dsb ish", options(nostack));
    if ctr & (1 << 29) == 0 {
        let line = 4usize << (ctr & 15);
        let mut at = (start as usize) & !(line - 1);
        while at < end {
            core::arch::asm!("ic ivau, {}", in(reg) at, options(nostack));
            at += line;
        }
        core::arch::asm!("dsb ish", options(nostack));
    }
    core::arch::asm!("isb", options(nostack));
}

pub struct CodeCache {
    base: *mut u8,
    size: usize,
    used: usize,
    /// Reservation granularity: Android's page size, one native word on macOS.
    page: usize,
    written: usize,
    publications: usize,
    #[cfg(test)]
    fail_next_publish: bool,
}

impl CodeCache {
    pub fn new(size: usize) -> Option<CodeCache> {
        #[cfg(target_os = "macos")]
        let (flags, prot) = (0x0002 | 0x1000 | 0x0800, 7); // PRIVATE | ANON | JIT
        #[cfg(target_os = "android")]
        let (flags, prot) = (0x02 | 0x20, 0); // PRIVATE | ANONYMOUS, reserve inaccessible
        #[cfg(target_os = "android")]
        let page = unsafe { getpagesize() } as usize;
        #[cfg(target_os = "macos")]
        let page = 4;
        #[cfg(target_os = "android")]
        if !page.is_power_of_two() || page > size { return None; }
        let p = unsafe { mmap(std::ptr::null_mut(), size, prot, flags, -1, 0) };
        if p as isize == -1 || p.is_null() { return None; }
        Some(CodeCache { base: p as *mut u8, size, used: 0, page, written: 0, publications: 0,
            #[cfg(test)] fail_next_publish: false,
        })
    }

    fn write(&mut self, words: &[u32]) -> Option<*const u8> {
        let bytes = words.len().checked_mul(4)?;
        let reserved = bytes.checked_add(self.page - 1)? & !(self.page - 1);
        if bytes == 0 || self.used.checked_add(reserved)? > self.size { return None; }
        // One serial owner: no generated code executes while compiling. Android gives each
        // batch fresh whole pages, RW during construction and RX afterwards, never RWX. Failed
        // protection changes cannot revoke execution from already published blocks.
        #[cfg(test)]
        let fail_publish = std::mem::take(&mut self.fail_next_publish);
        unsafe {
            let dst = self.base.add(self.used);
            #[cfg(target_os = "macos")]
            {
                pthread_jit_write_protect_np(0);
                std::ptr::copy_nonoverlapping(words.as_ptr() as *const u8, dst, bytes);
                pthread_jit_write_protect_np(1);
                sys_icache_invalidate(dst as *mut c_void, bytes);
                #[cfg(test)]
                if fail_publish { return None; }
            }
            #[cfg(target_os = "android")]
            {
                if mprotect(dst.cast(), reserved, 3) != 0 { return None; }
                std::ptr::copy_nonoverlapping(words.as_ptr() as *const u8, dst, bytes);
                flush_icache(dst, bytes);
                #[cfg(not(test))]
                let fail_publish = false;
                if fail_publish || mprotect(dst.cast(), reserved, 5) != 0 {
                    mprotect(dst.cast(), reserved, 0);
                    return None;
                }
            }
            self.used += reserved;
            self.written += bytes;
            self.publications += 1;
            Some(dst as *const u8)
        }
    }
}

impl Drop for CodeCache {
    fn drop(&mut self) {
        // No entry escapes the owning CPU's lifetime.
        unsafe { munmap(self.base.cast(), self.size); }
    }
}

/// JIT state kept in the Cpu.
pub struct Jit {
    cache: CodeCache,
    /// per block: runs counted until hot, then the compiled entry (or a failure)
    state: Vec<Entry>,
    // Boxes keep addresses embedded in generated code stable until cache reset.
    guards: Vec<Box<RamGuard>>,
    slots: Vec<Box<u64>>,
    helpers: Vec<Box<Fast>>,
    native_blocks: Vec<NativeBlockProfile>,
    /// compile helper calls that count themselves (`Cpu::profile` when the JIT was made)
    profile: bool,
    observe_pc: bool,
    targets: Box<[Target]>,
    /// Complete native blocks staged in non-executable memory. None of their
    /// addresses is exposed until the whole batch has become executable.
    pending_words: Vec<u32>,
    pending: Vec<Pending>,
    pending_counts: Option<(usize, usize, usize, usize)>,
    pending_visits: u32,
    /// Zero keeps macOS's immediate packed publication; Android targets one page.
    batch_bytes: usize,
    /// Lifetime totals survive executable-cache resets; no clock reads occur on
    /// ordinary block lookups or generated-code execution.
    compilation_cost: CompilationCost,
    publication_cost: CompilationCost,
}

#[derive(Default)]
struct CompilationCost { attempts: u64, nanoseconds: u64 }
impl CompilationCost {
    fn finish(&mut self, started: Instant) {
        self.attempts = self.attempts.saturating_add(1);
        self.nanoseconds = self.nanoseconds.saturating_add(started.elapsed().as_nanos().min(u64::MAX as u128) as u64);
    }
}

struct NativeBlockProfile {
    pc: u32,
    ops: String,
    // Full-prefix admission and bounded-path entry counts. Boxes keep the
    // addresses embedded in generated code stable as metadata grows.
    entries: Box<[u64; 2]>,
}

struct Pending {
    id: usize,
    pc: u32,
    start: usize,
    body: usize,
    len: u32,
}

/// Exact-PC tags make direct-map collisions harmless. Every body still performs
/// its own budget and code guards before retiring an instruction.
#[repr(C)]
#[derive(Clone, Copy)]
struct Target { pc: u32, _pad: u32, body: u64 }
const TARGET_BITS: u32 = 12;
const EMPTY_TARGET: Target = Target { pc: u32::MAX, _pad: 0, body: 0 };

#[derive(Clone, Copy)]
enum Entry {
    Cold(u32),
    Pending,
    /// function start, body (chain target), prefix length
    Code(*const u8, *const u8, u32),
    No,
}

// SAFETY: the code cache and the compiled entries are owned by one Cpu and only used by the
// thread that runs it; moving the Cpu (with them) to another thread is fine.
unsafe impl Send for Jit {}

impl std::fmt::Debug for Jit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Jit({}/{} native/reserved bytes, {} publications, {} blocks, {} pending)",
            self.cache.written, self.cache.used, self.cache.publications, self.state.len(), self.pending.len())
    }
}

/// Runs of a block before it is compiled.
const HOT: u32 = 32;
/// A lone hot block must not wait indefinitely for another block to compile.
const MAX_PENDING_VISITS: u32 = 8;
const MAX_PENDING_BLOCKS: usize = 8;

impl Jit {
    pub fn new(profile: bool) -> Option<Jit> {
        let cache = CodeCache::new(64 << 20)?;
        let batch_bytes = if cfg!(target_os = "android") { cache.page } else { 0 };
        Some(Jit { cache, state: Vec::new(), guards: Vec::new(), slots: Vec::new(), helpers: Vec::new(), native_blocks: Vec::new(), profile, observe_pc: false,
            pending_words: Vec::new(), pending: Vec::new(), pending_counts: None, pending_visits: 0, batch_bytes,
            compilation_cost: CompilationCost::default(), publication_cost: CompilationCost::default(),
            targets: vec![EMPTY_TARGET; 1 << TARGET_BITS].into_boxed_slice() })
    }

    /// PC observation is needed by CPU1's polling probes. CPU0's normal code can
    /// omit those checks. Upgrade an existing cache before exposing observation.
    pub(crate) fn observe_pc(&mut self) {
        if !self.observe_pc { self.reset(); self.observe_pc = true; }
    }

    /// The compiled code of block `id` (prefix length), compiling it once it is hot.
    pub(crate) fn entry<B: Bus>(&mut self, id: usize, ops: &[crate::BOp], pc0: u32, mem: &JitMem) -> Option<(*const u8, u32)> {
        if !self.pending.is_empty() {
            self.pending_visits += 1;
            if self.pending_visits >= MAX_PENDING_VISITS { self.flush_pending(); }
        }
        if self.state.len() <= id {
            self.state.resize(id + 1, Entry::Cold(0));
        }
        match self.state[id] {
            Entry::Code(p, body, n) => {
                self.targets[(pc0 >> 1) as usize & (self.targets.len() - 1)] = Target { pc: pc0, _pad: 0, body: body as u64 };
                Some((p, n))
            }
            Entry::No | Entry::Pending => None,
            Entry::Cold(c) if c + 1 < HOT => {
                self.state[id] = Entry::Cold(c + 1);
                None
            }
            Entry::Cold(_) => {
                if self.cache.used >= self.cache.size { self.state[id] = Entry::No; return None; }
                let counts = (self.guards.len(), self.slots.len(), self.helpers.len(), self.native_blocks.len());
                let plen = ops.iter().take_while(|o| native_prefix_op(o)).count();
                let counter = if self.profile && plen != 0 {
                    self.native_blocks.push(NativeBlockProfile { pc: pc0,
                        ops: ops[..plen].iter().map(|o|format!("{:?}",o.fast)).collect::<Vec<_>>().join("; "),
                        entries: Box::new([0;2]) });
                    Some(self.native_blocks.last_mut().unwrap().entries.as_mut_ptr())
                } else { None };
                let compiled = if plen == 0 {
                    None
                } else {
                    let started = Instant::now();
                    let result = compile::<B>(&ops[..plen], pc0, mem, &mut self.guards, &mut self.slots, &mut self.helpers, self.profile, self.observe_pc, self.targets.as_ptr(), counter);
                    self.compilation_cost.finish(started);
                    result
                };
                let Some((words, body)) = compiled else {
                    self.guards.truncate(counts.0); self.slots.truncate(counts.1); self.helpers.truncate(counts.2);
                    self.native_blocks.truncate(counts.3);
                    self.state[id] = Entry::No;
                    return None;
                };
                if self.pending.is_empty() { self.pending_counts = Some(counts); }
                let start = self.pending_words.len();
                self.pending.push(Pending { id, pc: pc0, start, body: start + body, len: plen as u32 });
                self.pending_words.extend(words);
                self.state[id] = Entry::Pending;
                if self.pending_words.len() * 4 >= self.batch_bytes || self.pending.len() >= MAX_PENDING_BLOCKS {
                    self.flush_pending();
                }
                match self.state[id] {
                    Entry::Code(p, body, n) => {
                        self.targets[(pc0 >> 1) as usize & (self.targets.len() - 1)] = Target { pc: pc0, _pad: 0, body: body as u64 };
                        Some((p, n))
                    }
                    _ => None,
                }
            }
        }
    }

    /// Finish construction before publishing any entry or indirect chain target.
    /// Compilation and publication run only between native execution entries.
    fn flush_pending(&mut self) {
        if self.pending.is_empty() { return; }
        let started = Instant::now();
        let code = self.cache.write(&self.pending_words);
        for pending in self.pending.drain(..) {
            self.state[pending.id] = match code {
                Some(base) => {
                    // SAFETY: compile returned in-block word offsets, and write
                    // published the complete concatenated allocation as executable.
                    let (entry, body) = unsafe { (base.add(pending.start * 4), base.add(pending.body * 4)) };
                    self.targets[(pending.pc >> 1) as usize & (self.targets.len() - 1)] = Target { pc: pending.pc, _pad: 0, body: body as u64 };
                    Entry::Code(entry, body, pending.len)
                }
                None => Entry::No,
            };
        }
        if code.is_none() {
            // All allocations since this batch started belong to this batch:
            // every successful publication drains the whole queue. Previously
            // published code therefore keeps all of its boxed dependencies.
            if let Some((guards, slots, helpers, native_blocks)) = self.pending_counts {
                self.guards.truncate(guards); self.slots.truncate(slots); self.helpers.truncate(helpers);
                self.native_blocks.truncate(native_blocks);
            }
        }
        self.pending_words.clear();
        self.pending_counts = None;
        self.pending_visits = 0;
        self.publication_cost.finish(started);
    }

    pub(crate) fn compilation_nanoseconds(&self) -> u64 {
        self.compilation_cost.nanoseconds.saturating_add(self.publication_cost.nanoseconds)
    }

    pub(crate) fn cache_profile_json(&self) -> String {
        format!("{{\"nativeBytes\":{},\"reservedBytes\":{},\"publications\":{},\"allocationGranularity\":{},\"pendingBytes\":{},\"pendingBlocks\":{},\"compileAttempts\":{},\"compileNanoseconds\":{},\"publicationAttempts\":{},\"publicationNanoseconds\":{}}}",
            self.cache.written, self.cache.used, self.cache.publications, self.cache.page, self.pending_words.len() * 4, self.pending.len(),
            self.compilation_cost.attempts, self.compilation_cost.nanoseconds,
            self.publication_cost.attempts, self.publication_cost.nanoseconds)
    }

    pub(crate) fn native_blocks_json(&self) -> String {
        let mut blocks:Vec<_>=self.native_blocks.iter().filter(|b|b.entries[0]+b.entries[1]!=0).collect();
        blocks.sort_by_key(|b|std::cmp::Reverse(b.entries[0]+b.entries[1]));
        let blocks=blocks.into_iter().take(64).map(|b|{
            let ops=b.ops.replace('\\',"\\\\").replace('"',"\\\"");
            format!("{{\"pc\":{},\"count\":{},\"fullPathEntries\":{},\"boundedPathEntries\":{},\"ops\":\"{ops}\"}}",
                b.pc,b.entries[0]+b.entries[1],b.entries[0],b.entries[1])
        }).collect::<Vec<_>>().join(",");
        format!("[{blocks}]")
    }

    /// The chain target of block `id`, if it is compiled.
    pub(crate) fn body(&self, id: usize) -> Option<*const u8> {
        match self.state.get(id) {
            Some(Entry::Code(_, b, _)) => Some(*b),
            _ => None,
        }
    }

    /// Forget every compiled block (the block table was rebuilt).
    pub fn reset(&mut self) {
        self.state.clear();
        self.pending.clear();
        self.pending_words.clear();
        self.pending_counts = None;
        self.pending_visits = 0;
        self.cache.used = 0;
        self.cache.written = 0;
        self.cache.publications = 0;
        self.guards.clear();
        self.slots.clear();
        self.helpers.clear();
        self.native_blocks.clear();
        self.targets.fill(EMPTY_TARGET);
    }
}

/// Run compiled code: returns the instructions executed; cpu.pc is set.
///
/// # Safety
/// `code` must come from `Jit::entry` for this bus type and the memory `mem` describes.
pub(crate) unsafe fn run<B: Bus>(code: *const u8, cpu: &mut Cpu, bus: &mut B, mem: &JitMem, extra: u32, cap: u32) -> (u32, *mut u64) {
    // instructions this call may run: their ticks ((k - 1) + extra) must all fit tick_room
    let limit = (bus.tick_room() + 1).saturating_sub(extra as u64).min(1 << 30).min(cap as u64) as u32;
    let code_version = bus.code_writes();
    let mut ctx = Ctx { cpu: cpu as *mut Cpu, bus: bus as *mut B as *mut (), extra, synced: 0, brk: 0, stop_pc: cpu.run_stop_pc.unwrap_or(u32::MAX), slot: std::ptr::null_mut(), code_version, guard_version: code_version.unwrap_or(0), guard_tracked: u32::from(code_version.is_some()), xip_version: bus.xip_writes(), panic: None };
    let f: extern "C" fn(*mut Cpu, *mut Ctx, *mut u8, *const u8, u32, *const u8) -> u32 = unsafe { std::mem::transmute(code) };
    let k = f(cpu as *mut Cpu, &mut ctx, mem.ram, mem.xip, limit, mem.ram_code);
    if let Some(panic) = ctx.panic.take() { std::panic::resume_unwind(panic); }
    if k > 0 {
        // the ticks of the instructions run, less those the helpers applied already
        let need = (k - 1) + extra;
        if need > ctx.synced {
            bus.add_ticks((need - ctx.synced) as u64);
        }
    }
    (k, ctx.slot)
}

// ------------------------------------------------------------------ helpers called from generated code

unsafe fn sync<B: Bus>(ctx: &mut Ctx, bus: &mut B, idx: u32) {
    let need = idx + ctx.extra;
    if need > ctx.synced {
        bus.add_ticks((need - ctx.synced) as u64);
        ctx.synced = need;
    }
}

// A peripheral error must never unwind through generated code (which has no unwind tables).
fn helper<T: Default>(ctx: *mut Ctx, action: impl FnOnce() -> T) -> T {
    if unsafe { (*ctx).panic.is_some() } { return T::default(); }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)) {
        Ok(value) => value,
        Err(error) => {
            unsafe { (*ctx).panic = Some(error); (*ctx).brk = 1; }
            T::default()
        }
    }
}

extern "C" fn h_read<B: Bus>(ctx: *mut Ctx, addr: u32, idx: u32, w: u32) -> u32 {
    helper(ctx, || {
    // SAFETY: ctx is the live context `run` passed in; the bus pointer is the caller's &mut B.
    let ctx = unsafe { &mut *ctx };
    let bus = unsafe { &mut *(ctx.bus as *mut B) };
    unsafe { sync(ctx, bus, idx) };
    let v = match w {
        1 => bus.read8(addr) as u32,
        2 => bus.read16(addr) as u32,
        _ => bus.read32(addr),
    };
    if bus.next_tick_due() || bus.code_writes() != ctx.code_version || bus.xip_writes() != ctx.xip_version {
        ctx.brk = 1;
    }
    v
    })
}

extern "C" fn h_write<B: Bus>(ctx: *mut Ctx, addr: u32, v: u32, idx: u32, w: u32) {
    helper(ctx, || {
    // SAFETY: as h_read.
    let ctx = unsafe { &mut *ctx };
    let bus = unsafe { &mut *(ctx.bus as *mut B) };
    unsafe { sync(ctx, bus, idx) };
    match w {
        1 => bus.write8(addr, v as u8),
        2 => bus.write16(addr, v as u16),
        _ => bus.write32(addr, v),
    }
    if bus.next_tick_due() || bus.code_writes() != ctx.code_version || bus.xip_writes() != ctx.xip_version {
        ctx.brk = 1;
    }
    })
}

/// A special-register stack transfer retains the scheduler's IRQ observation.
extern "C" fn h_stack_window<B: Bus, const PROF: bool>(ctx: *mut Ctx, idx: u32) {
    helper(ctx, || {
        // SAFETY: the context and its CPU/bus pointers are live for this run.
        let ctx = unsafe { &mut *ctx };
        let bus = unsafe { &mut *(ctx.bus as *mut B) };
        unsafe { sync(ctx, bus, idx) };
        if PROF { *unsafe { &mut *ctx.cpu }.jit_helpers.entry("stack_irq_window".into()).or_default() += 1; }
        unsafe { &*ctx.cpu }.request_irq_window(bus);
        if bus.next_tick_due() || bus.code_writes() != ctx.code_version || bus.xip_writes() != ctx.xip_version {
            ctx.brk = 1;
        }
    })
}

/// STI retains the bus's interrupt-window effect without the generic op dispatch.
extern "C" fn h_irq_window<B: Bus, const PROF: bool>(ctx: *mut Ctx, idx: u32) {
    helper(ctx, || {
        // SAFETY: the context and its CPU/bus pointers are live for this run.
        let ctx=unsafe { &mut *ctx };
        let bus=unsafe { &mut *(ctx.bus as *mut B) };
        unsafe { sync(ctx,bus,idx) };
        let cpu=unsafe { &mut *ctx.cpu };
        cpu.ie=true;
        if PROF { *cpu.jit_helpers.entry("irq_window".into()).or_default()+=1; }
        cpu.request_irq_window(bus);
        if bus.next_tick_due() || bus.code_writes()!=ctx.code_version || bus.xip_writes()!=ctx.xip_version {
            ctx.brk=1;
        }
    })
}

/// Any op through the interpreter's own executor: bit 32 = jumped (target in the low word).
// PROF: count the call (`Cpu::jit_helpers`); chosen when compiling, so code compiled without
// profiling pays nothing for it
extern "C" fn h_op<B: Bus, const PROF: bool>(ctx: *mut Ctx, op: *const Fast, idx: u32, next_pc: u32) -> u64 {
    helper(ctx, || {
    // SAFETY: as h_read; `op` points at a JIT-owned, immutable Fast.
    let ctx = unsafe { &mut *ctx };
    let bus = unsafe { &mut *(ctx.bus as *mut B) };
    let cpu = unsafe { &mut *ctx.cpu };
    unsafe { sync(ctx, bus, idx) };
    if PROF {
        cpu.profile_helper(unsafe { &*op });
    }
    let j = cpu.exec_fast(bus, unsafe { &*op }, next_pc);
    if bus.next_tick_due() || bus.code_writes() != ctx.code_version || bus.xip_writes() != ctx.xip_version {
        ctx.brk = 1;
    }
    match j {
        Some(t) => (1u64 << 32) | t as u64,
        None => 0,
    }
    })
}

// Each RAM body owns an immutable snapshot. Even an old incoming chain remains safe
// after its block has been rebuilt: it must validate the code it actually executes.
#[repr(C)]
struct RamGuard {
    pc: u32,
    bytes: Box<[u8]>,
    version: u64,
    tracked: u32,
}

extern "C" fn h_ram_ok<B: Bus>(ctx: *mut Ctx, guard: *mut RamGuard) -> u32 {
    helper(ctx, || {
    let ctx = unsafe { &mut *ctx };
    let bus = unsafe { &mut *(ctx.bus as *mut B) };
    let guard = unsafe { &mut *guard };
    let version = bus.code_writes();
    if version.is_some_and(|v| guard.tracked != 0 && guard.version == v) { return 1; }
    let valid = crate::ram_code(bus, guard.pc)
        .is_some_and(|now| now.len() >= guard.bytes.len() && now[..guard.bytes.len()] == *guard.bytes);
    if valid {
        guard.version = version.unwrap_or(0);
        guard.tracked = u32::from(version.is_some());
    }
    valid as u32
    })
}

// ------------------------------------------------------------------ code generation

const CPU: Reg = 19;
const CTX: Reg = 20;
const RAM: Reg = 21;
const XIP: Reg = 22;
const KEEP: Reg = 23;
const KEEP2: Reg = 24;
const FLAGS: Reg = 25;
const DONE: Reg = 26;
const LIMIT: Reg = 27;
const STOP: Reg = 28;
const FRAME: i32 = 96;

struct Gen<'a> {
    a: Asm,
    mem: &'a JitMem,
    slots: &'a mut Vec<Box<u64>>,
    helpers: &'a mut Vec<Box<Fast>>,
    exit: Label,
    h_read: u64,
    h_write: u64,
    h_op: u64,
    observe_pc: bool,
    targets: *const Target,
    known_regs: [Option<u32>; 16],
    cached_regs: Vec<(u8, Reg)>,
    fp_cached_regs: Vec<(u8,Reg)>,
    cache_active: bool,
    publish_flags: bool,
}

fn off_reg(r: u8) -> u32 {
    (std::mem::offset_of!(Cpu, regs) + 4 * (r & 15) as usize) as u32
}
fn off_sp() -> u32 {
    std::mem::offset_of!(Cpu, sp) as u32
}
fn off_rets() -> u32 {
    std::mem::offset_of!(Cpu, rets) as u32
}
fn off_hold() -> u32 {
    std::mem::offset_of!(Cpu, hold_irq) as u32
}
fn off_pc() -> u32 {
    std::mem::offset_of!(Cpu, pc) as u32
}

fn acond(c: Cond) -> AC {
    match c {
        Cond::Eq => AC::Eq,
        Cond::Ne => AC::Ne,
        Cond::LtU => AC::Lo,
        Cond::GtU => AC::Hi,
        Cond::LeU => AC::Ls,
        Cond::GeU => AC::Hs,
        Cond::LtS => AC::Lt,
        Cond::GtS => AC::Gt,
        Cond::LeS => AC::Le,
        Cond::GeS => AC::Ge,
        // after fcmp: less N, equal ZC, greater C, unordered CV
        Cond::FEq => AC::Eq,
        Cond::FUne => AC::Ne,
        Cond::FLt => AC::Mi,
        Cond::FUge => AC::Pl,
        Cond::FLe => AC::Ls,
        Cond::FUgt => AC::Hi,
        Cond::FGe => AC::Ge,
        Cond::FUlt => AC::Lt,
        Cond::FGt => AC::Gt,
        Cond::FUle => AC::Le,
    }
}

impl Gen<'_> {
    fn advance_constants(&mut self, f: Fast) {
        let reg = |r: u8| self.known_regs[(r & 15) as usize];
        let value = match f {
            Fast::MovI { d, v } => Some((d, Some(v))),
            Fast::Mov { d, s } => Some((d, reg(s))),
            Fast::AddRI { d, s, v } => Some((d, reg(s).map(|x| x.wrapping_add(v)))),
            Fast::AddRR { d, a, b } => Some((d, reg(a).zip(reg(b)).map(|(a,b)| a.wrapping_add(b)))),
            Fast::SubRR { d, a, b } => Some((d, reg(a).zip(reg(b)).map(|(a,b)| a.wrapping_sub(b)))),
            Fast::Alu3 { d, a, b, op } => Some((d, reg(a).zip(reg(b)).map(|(a,b)| crate::alu(op,a,b)))),
            Fast::Alu3I { d, a, v, op } => Some((d, reg(a).map(|a| crate::alu(op,a,v)))),
            Fast::Alu2 { d, b, op } => Some((d, reg(d).zip(reg(b)).map(|(a,b)| crate::alu(op,a,b)))),
            Fast::Alu2I { d, v, op } => Some((d, reg(d).map(|a| crate::alu(op,a,v)))),
            Fast::AndI { d, s, v } => Some((d, reg(s).map(|s| s & v))),
            Fast::MulI { d, s, v } => Some((d, reg(s).map(|s| s.wrapping_mul(v)))),
            Fast::ShlI { d, s, n } => Some((d, reg(s).map(|s| s.wrapping_shl(n as u32 & 31)))),
            Fast::ShrI { d, s, n } => Some((d, reg(s).map(|s| s.wrapping_shr(n as u32 & 31)))),
            Fast::A4 { d, s, op, rk, rv } => {
                let rhs = match rk { 0 => reg(rv as u8), 1 => Some(rv), _ => reg(rv as u8).map(|v| 1u32 << (v & 31)) };
                Some((d, reg(s).zip(rhs).map(|(a,b)| crate::alu(op,a,b))))
            }
            _ => None,
        };
        match f.regs_rw() {
            Some((_, writes)) => for r in 0..16 { if writes & (1 << r) != 0 { self.known_regs[r] = None; } },
            None => self.known_regs.fill(None),
        }
        if let Some((d, value)) = value { self.known_regs[(d & 15) as usize] = value; }
    }

    /// Constants are derived only from earlier instructions in this prefix. MMIO
    /// still goes through its exact helper; known RAM/XIP offsets skip range probes.
    fn load_base(&mut self, b: Base, off: u32, w: u8, signed: bool, idx: u32) -> bool {
        if let Base::Reg(r) = b {
            if let Some(base) = self.known_regs[(r & 15) as usize] {
                let addr = base.wrapping_add(off);
                for (host, base, len) in [(RAM, self.mem.ram_base, self.mem.ram_len), (XIP, self.mem.xip_base, self.mem.xip_len)] {
                    let mut offset = addr.wrapping_sub(base);
                    if w == 2 { offset &= !1; }
                    if len >= w as u32 && offset <= len - w as u32 {
                        self.a.mov32(3, offset);
                        match (w, signed) {
                            (4, _) => self.a.ldr_u(0, host, 3),
                            (2, false) => self.a.ldrh_u(0, host, 3),
                            (2, true) => self.a.ldrsh_u(0, host, 3),
                            (1, false) => self.a.ldrb_u(0, host, 3),
                            _ => self.a.ldrsb_u(0, host, 3),
                        }
                        return false;
                    }
                }
                self.a.mov32(0, addr); self.read_helper(w, signed, idx);
                return true;
            }
        }
        match b { Base::Reg(r) => self.ld_reg(0,r), Base::Sp => self.a.ldr(0,CPU,off_sp()) }
        self.add_imm(0,0,off); self.load(w,signed,idx);
        true
    }

    fn read_helper(&mut self, w: u8, signed: bool, idx: u32) {
        self.a.mov(1, 0); self.a.mov_x(0, CTX); self.idx_w2(idx);
        self.a.mov32(3, w as u32); self.call(self.h_read);
        match (w, signed) { (2,true) => self.a.sxth(0,0), (1,true) => self.a.sbfx(0,0,0,8), _ => {} }
    }

    fn write_helper(&mut self, w: u8, idx: u32) {
        self.a.mov(2, 1); self.a.mov(1, 0); self.a.mov_x(0, CTX);
        self.a.add_imm(3, DONE, idx); self.a.mov32(4, w as u32); self.call(self.h_write);
    }

    fn store_base(&mut self, b: Base, off: u32, s: u8, w: u8, idx: u32) {
        if let Base::Reg(r) = b {
            if let Some(base) = self.known_regs[(r & 15) as usize] {
                let addr = base.wrapping_add(off);
                let offset = addr.wrapping_sub(self.mem.ram_base);
                self.ld_reg(1,s);
                if self.mem.ram_len >= w as u32 && offset <= self.mem.ram_len - w as u32 && (w != 2 || addr & 1 == 0) {
                    let (slow, done) = (self.a.label(), self.a.label());
                    self.a.mov32(3, offset);
                    if !self.mem.ram_code.is_null() {
                        self.a.lsr_imm(2,3,2); self.a.ldrb_u(2,FLAGS,2); self.a.cbnz(2,slow);
                    }
                    match w { 4 => self.a.str_u(1,RAM,3), 2 => self.a.strh_u(1,RAM,3), _ => self.a.strb_u(1,RAM,3) }
                    self.a.b(done); self.a.bind(slow);
                    self.a.mov32(0,addr); self.write_helper(w,idx); self.a.bind(done);
                } else {
                    self.a.mov32(0,addr); self.write_helper(w,idx);
                }
                return;
            }
        }
        match b { Base::Reg(r) => self.ld_reg(0,r), Base::Sp => self.a.ldr(0,CPU,off_sp()) }
        self.add_imm(0,0,off); self.ld_reg(1,s); self.store(w,idx);
    }

    /// flags for a compare of w(x) with w(y): integer, or as two floats
    fn cmp(&mut self, float: bool, x: Reg, y: Reg) {
        if float {
            self.a.fmov_s_w(0, x);
            self.a.fmov_s_w(1, y);
            self.a.fcmp(0, 1);
        } else {
            self.a.cmp(x, y);
        }
    }
    fn ld_reg(&mut self, w: Reg, r: u8) {
        if let Some(host)=self.fp_cached(r){self.a.fmov_w_s(w,host);return;}
        if self.cache_active {
            if let Some(&(_, host)) = self.cached_regs.iter().find(|&&(guest,_)| guest == r & 15) {
                self.a.mov(w,host); return;
            }
        }
        self.a.ldr(w, CPU, off_reg(r));
    }
    fn st_reg(&mut self, w: Reg, r: u8) {
        if let Some(host)=self.fp_cached(r){self.a.fmov_s_w(host,w);return;}
        if self.cache_active {
            if let Some(&(_, host)) = self.cached_regs.iter().find(|&&(guest,_)| guest == r & 15) {
                self.a.mov(host,w); return;
            }
        }
        self.a.str(w, CPU, off_reg(r));
    }
    fn flush_regs(&mut self) {
        if self.cache_active { for &(r,host) in &self.cached_regs { self.a.str(host,CPU,off_reg(r)); } }
        if self.cache_active { for &(r,host) in &self.fp_cached_regs { self.a.str_s(host,CPU,off_reg(r)); } }
    }
    fn reload_regs(&mut self) {
        if self.cache_active { for &(r,host) in &self.cached_regs { self.a.ldr(host,CPU,off_reg(r)); } }
        if self.cache_active { for &(r,host) in &self.fp_cached_regs { self.a.ldr_s(host,CPU,off_reg(r)); } }
    }
    /// w(dst) = w(src) + imm (any 32-bit immediate)
    fn add_imm(&mut self, dst: Reg, src: Reg, imm: u32) {
        if imm == 0 {
            if dst != src {
                self.a.mov(dst, src);
            }
        } else if imm < 4096 {
            self.a.add_imm(dst, src, imm);
        } else if imm.wrapping_neg() < 4096 {
            self.a.sub_imm(dst, src, imm.wrapping_neg());
        } else {
            self.a.mov32(9, imm);
            self.a.add(dst, src, 9);
        }
    }
    /// w0 = op(w0, w1)
    fn alu(&mut self, op: Op) {
        let a = &mut self.a;
        match op {
            Op::Add => a.add(0, 0, 1),
            Op::Sub => a.sub(0, 0, 1),
            Op::RSub => a.sub(0, 1, 0),
            Op::And => a.and(0, 0, 1),
            Op::AndNot => a.bic(0, 0, 1),
            Op::Or => a.orr(0, 0, 1),
            Op::Xor => a.eor(0, 0, 1),
            Op::Mul => a.mul(0, 0, 1),
            Op::Shl => a.lslv(0, 0, 1),
            Op::Shr => a.lsrv(0, 0, 1),
            Op::Sar => a.asrv(0, 0, 1),
        }
    }
    /// Publish the arithmetic result's NZCV in the guest's N,C,Z,V bit order.
    /// CSET and the packing operations preserve the host condition flags.
    fn arithmetic_flags(&mut self) {
        self.a.cset(1, AC::Mi); self.a.cset(2, AC::Hs);
        self.a.add_lsl(1, 1, 2, 1); self.a.cset(2, AC::Eq);
        self.a.add_lsl(1, 1, 2, 2); self.a.cset(2, AC::Vs);
        self.a.add_lsl(1, 1, 2, 3);
        self.a.str(1, CPU, std::mem::offset_of!(Cpu, flags) as u32);
    }
    /// Register ALU instructions update guest flags for arithmetic. Memory ALU
    /// instructions use `alu` instead because their reference semantics preserve flags.
    fn alu_flags(&mut self, op: Op) {
        if !self.publish_flags { self.alu(op); return; }
        match op {
            Op::Add => self.a.adds(0, 0, 1),
            Op::Sub => self.a.subs(0, 0, 1),
            Op::RSub => self.a.subs(0, 1, 0),
            _ => { self.alu(op); return; }
        }
        self.arithmetic_flags();
    }
    /// w1 = an A4/BitBr right-hand side: rk 0 register, 1 immediate, 2 `1 << reg`
    fn rhs(&mut self, rk: u8, rv: u32) {
        match rk {
            0 => self.ld_reg(1, rv as u8),
            1 => self.a.mov32(1, rv),
            _ => {
                self.ld_reg(2, rv as u8);
                self.a.movz(1, 1, 0);
                self.a.lslv(1, 1, 2);
            }
        }
    }
    fn call(&mut self, f: u64) {
        self.flush_regs();
        self.a.mov64(9, f);
        self.a.blr(9);
        self.reload_regs();
    }
    /// Leave with pc = imm, `count` instructions of this block done.
    fn exit_imm(&mut self, pc: u32, count: u32) {
        self.flush_regs();
        self.a.mov32(9, pc);
        self.a.str(9, CPU, off_pc());
        self.a.add_imm(0, DONE, count);
        self.a.b(self.exit);
    }
    /// Leave with pc = w(reg).
    fn exit_reg(&mut self, reg: Reg, count: u32) {
        self.flush_regs();
        let back = self.a.label();
        // A bus helper may have changed code or unmasked an IRQ. Those boundaries
        // must return to the scheduler even when the destination is compiled.
        self.a.ldr(12, CTX, OFF_BRK);
        self.a.cbnz(12, back);
        if self.observe_pc {
            self.a.cmp(reg, STOP);
            self.a.b_cond(AC::Eq, back);
        }
        self.a.ubfx(10, reg, 1, TARGET_BITS);
        self.a.lsl_imm(10, 10, 4);
        self.a.mov64(11, self.targets as u64);
        self.a.add_x(10, 11, 10);
        self.a.ldr(12, 10, std::mem::offset_of!(Target, pc) as u32);
        self.a.cmp(12, reg);
        self.a.b_cond(AC::Ne, back);
        self.a.ldr_x(12, 10, std::mem::offset_of!(Target, body) as u32);
        self.a.cbz(12, back);
        self.a.add_imm(DONE, DONE, count);
        self.a.br(12);
        self.a.bind(back);
        self.a.str(reg, CPU, off_pc());
        self.a.add_imm(0, DONE, count);
        self.a.b(self.exit);
    }
    /// Continue at a fixed pc: straight into its compiled body when the slot holds one,
    /// else back to Rust, which fills the slot once that block is compiled.
    fn chain(&mut self, pc: u32, count: u32) {
        self.check_stop(pc, count, false);
        self.flush_regs();
        self.slots.push(Box::new(0));
        let slot = self.slots.last_mut().unwrap().as_mut();
        let back = self.a.label();
        self.a.add_imm(DONE, DONE, count);
        self.a.mov64(10, slot as *mut u64 as u64);
        self.a.ldr_x(9, 10, 0);
        self.a.cbz(9, back);
        self.a.br(9);
        self.a.bind(back);
        self.a.mov32(9, pc);
        self.a.str(9, CPU, off_pc());
        self.a.str_x(10, CTX, OFF_SLOT);
        self.a.mov(0, DONE);
        self.a.b(self.exit);
    }
    /// Observe a requested PC before another instruction can retire, including a
    /// chained loop back to the PC where this run started.
    fn check_stop(&mut self, pc: u32, count: u32, glue: bool) {
        if !self.observe_pc { return; }
        let go = self.a.label();
        self.a.mov32(9, pc);
        self.a.cmp(9, STOP);
        self.a.b_cond(AC::Ne, go);
        if glue {
            self.a.mov32(9, 1);
            self.a.str(9, CPU, off_hold());
        }
        self.exit_imm(pc, count);
        self.a.bind(go);
    }
    /// The helpers' instruction index of op `i` of this block: w2 = DONE + i.
    fn idx_w2(&mut self, i: u32) {
        self.a.add_imm(2, DONE, i);
    }

    /// Guard a whole stack transfer once. On success w10 is the lowest guest
    /// address and x12 its RAM pointer; the fallback has not observed any store.
    fn stack_ram(&mut self, words: u32, push: bool, slow: Label) {
        let bytes = words * 4;
        self.a.ldr(10, CPU, off_sp());
        if push { self.a.sub_imm(10, 10, bytes); }
        self.a.tbnz(10, 0, slow); self.a.tbnz(10, 1, slow);
        if self.mem.ram_len < bytes { self.a.b(slow); return; }
        self.a.mov32(2, self.mem.ram_base); self.a.sub(11, 10, 2);
        self.a.mov32(2, self.mem.ram_len - bytes); self.a.cmp(11, 2);
        self.a.b_cond(AC::Hi, slow);
        if push && !self.mem.ram_code.is_null() {
            self.a.lsr_imm(2, 11, 2); self.a.add_x(9, FLAGS, 2);
            // Read exactly the covered code flags, including the tail. A watched
            // word uses the ordinary stores, retaining invalidation and write order.
            let mut offset = 0;
            while offset + 8 <= words {
                self.a.ldr_x(0, 9, offset); self.a.cbnz_x(0, slow); offset += 8;
            }
            if offset + 4 <= words {
                self.a.ldr(0, 9, offset); self.a.cbnz(0, slow); offset += 4;
            }
            if offset + 2 <= words {
                self.a.ldrh(0, 9, offset); self.a.cbnz(0, slow); offset += 2;
            }
            if offset < words {
                self.a.mov32(2, offset); self.a.ldrb_u(0, 9, 2); self.a.cbnz(0, slow);
            }
        }
        self.a.add_x(12, RAM, 11);
    }

    /// w0 = load(width, signed) at guest address w0. Inline RAM / XIP, else the bus.
    fn load(&mut self, w: u8, signed: bool, idx: u32) {
        let (xip, slow, done) = (self.a.label(), self.a.label(), self.a.label());
        let mem = *self.mem;
        for (base_reg, gbase, len, miss) in [(RAM, mem.ram_base, mem.ram_len, xip), (XIP, mem.xip_base, mem.xip_len, slow)] {
            if base_reg == XIP {
                self.a.bind(xip);
            }
            if len < w as u32 { self.a.b(miss); continue; }
            // off = addr - base; the word holding it must lie inside (as the Soc's fast paths)
            self.a.mov32(2, gbase);
            self.a.sub(3, 0, 2);
            if w == 2 {
                self.a.mov32(2, !1u32);
                self.a.and(3, 3, 2); // a halfword is the one at addr & !1 (Soc read16)
            }
            self.a.mov32(2, len.saturating_sub(w as u32));
            self.a.cmp(3, 2);
            self.a.b_cond(AC::Hi, miss);
            match (w, signed) {
                (4, _) => self.a.ldr_u(0, base_reg, 3),
                (2, false) => self.a.ldrh_u(0, base_reg, 3),
                (2, true) => self.a.ldrsh_u(0, base_reg, 3),
                (1, false) => self.a.ldrb_u(0, base_reg, 3),
                _ => self.a.ldrsb_u(0, base_reg, 3),
            }
            self.a.b(done);
        }
        self.a.bind(slow);
        self.read_helper(w, signed, idx);
        self.a.bind(done);
    }

    /// store(width) of w1 at guest address w0. Inline RAM, else the bus.
    fn store(&mut self, w: u8, idx: u32) {
        let (slow, done) = (self.a.label(), self.a.label());
        let mem = *self.mem;
        if mem.ram_len < w as u32 { self.a.b(slow); }
        if w == 2 {
            self.a.tbnz(0, 0, slow); // an odd halfword goes the Soc's merging way
        }
        self.a.mov32(2, mem.ram_base);
        self.a.sub(3, 0, 2);
        self.a.mov32(2, mem.ram_len.saturating_sub(w as u32));
        self.a.cmp(3, 2);
        self.a.b_cond(AC::Hi, slow);
        if !mem.ram_code.is_null() {
            self.a.lsr_imm(2, 3, 2);
            self.a.ldrb_u(2, FLAGS, 2);
            self.a.cbnz(2, slow);
        }
        match w {
            4 => self.a.str_u(1, RAM, 3),
            2 => self.a.strh_u(1, RAM, 3),
            _ => self.a.strb_u(1, RAM, 3),
        }
        self.a.b(done);
        self.a.bind(slow);
        self.a.mov(2, 1);
        self.a.mov(1, 0);
        self.a.mov_x(0, CTX);
        self.a.add_imm(3, DONE, idx);
        self.a.mov32(4, w as u32);
        self.call(self.h_write);
        self.a.bind(done);
    }

    /// After an instruction that may have called the bus: leave if the next tick is due.
    fn check_brk_hold(&mut self, next_pc: u32, count: u32) {
        let go = self.a.label();
        self.a.ldr(9, CTX, OFF_BRK);
        self.a.cbz(9, go);
        self.a.mov32(9, 1);
        self.a.str(9, CPU, off_hold());
        self.exit_imm(next_pc, count);
        self.a.bind(go);
    }
    fn check_brk(&mut self, next_pc: u32, count: u32) {
        let go = self.a.label();
        self.a.ldr(9, CTX, OFF_BRK);
        self.a.cbz(9, go);
        self.exit_imm(next_pc, count);
        self.a.bind(go);
    }
}

/// Admit cached careful flag producers only where native semantics are exact.
fn native_prefix_op(op:&crate::BOp)->bool {
    if !op.careful{return true;}
    if op.pair_head{return false;}
    // Cached careful annotations also protect backends that do not publish
    // arithmetic flags. These exact native producers need no such fence. Pair,
    // repeat and predication state is still excluded by Cpu::run_fast admission.
    matches!(op.fast,Fast::AddRI {..}|Fast::AddRR {..}|Fast::SubRR {..}|Fast::Carry {..}
        |Fast::Alu3 {op:Op::Add|Op::Sub|Op::RSub,..}
        |Fast::Alu3I {op:Op::Add|Op::Sub|Op::RSub,..}
        |Fast::Alu2 {op:Op::Add|Op::Sub|Op::RSub,..}
        |Fast::Alu2I {op:Op::Add|Op::Sub|Op::RSub,..}
        |Fast::A4 {op:Op::Add|Op::Sub|Op::RSub,..})
}

/// Only a complete, uninterrupted prefix can omit an overwritten flag result.
/// Whitelist direct operations with no flag reads, bus access, helper or exit;
/// everything else is a barrier. Bounded paths always publish each result.
fn flags_overwritten(remaining: &[crate::BOp]) -> bool {
    for op in remaining {
        if op.careful { return false; }
        match op.fast {
            Fast::AddRI { .. } | Fast::AddRR { .. } | Fast::SubRR { .. }
            | Fast::Alu3 { op: Op::Add | Op::Sub | Op::RSub, .. }
            | Fast::Alu3I { op: Op::Add | Op::Sub | Op::RSub, .. }
            | Fast::Alu2 { op: Op::Add | Op::Sub | Op::RSub, .. }
            | Fast::Alu2I { op: Op::Add | Op::Sub | Op::RSub, .. }
            | Fast::A4 { op: Op::Add | Op::Sub | Op::RSub, .. } => return true,
            Fast::Nop | Fast::MovI { .. } | Fast::Mov { .. } | Fast::AndI { .. }
            | Fast::MulI { .. } | Fast::AndBit { .. } | Fast::ShlR { .. }
            | Fast::ShrR { .. } | Fast::ShlI { .. } | Fast::ShrI { .. }
            | Fast::SarI { .. } | Fast::SpFromR { .. } | Fast::SpAdd { .. }
            | Fast::RFromSp { .. } | Fast::Ext { .. } | Fast::BitExt { .. }
            | Fast::BitIns { .. } | Fast::Mov2 { .. } | Fast::Mov2I { .. }
            | Fast::Alu3 { .. } | Fast::Alu3I { .. } | Fast::Alu2 { .. }
            | Fast::Alu2I { .. } | Fast::A4 { .. } => {},
            _ => return false,
        }
    }
    false
}

/// Native code for a block prefix of plain micro-ops (none careful).
fn compile<B: Bus>(ops: &[crate::BOp], pc0: u32, mem: &JitMem, guards: &mut Vec<Box<RamGuard>>, slots: &mut Vec<Box<u64>>, helpers: &mut Vec<Box<Fast>>, profile: bool, observe_pc: bool, targets: *const Target, counter: Option<*mut u64>) -> Option<(Vec<u32>, usize)> {
    for o in [off_reg(15), off_sp(), off_rets(), off_pc(), off_hold()] {
        if o >= 16384 {
            return None;
        }
    }
    let mut a = Asm::new();
    let exit = a.label();
    let mut g = Gen {
        a,
        mem,
        slots,
        helpers,
        exit,
        h_read: h_read::<B> as usize as u64,
        h_write: h_write::<B> as usize as u64,
        h_op: if profile { h_op::<B, true> as usize as u64 } else { h_op::<B, false> as usize as u64 },
        observe_pc,
        targets,
        known_regs: [None; 16],
        cached_regs: Vec::new(),
        fp_cached_regs: Vec::new(),
        cache_active: false,
        publish_flags: true,
    };
    // prologue (entry from Rust); chained blocks enter at the body
    g.a.stp_pre(29, 30, SP, -FRAME);
    g.a.stp(CPU, CTX, SP, 16);
    g.a.stp(RAM, XIP, SP, 32);
    g.a.stp(KEEP, KEEP2, SP, 48);
    g.a.stp(25, DONE, SP, 64);
    g.a.stp(LIMIT, 28, SP, 80);
    g.a.mov_x(CPU, 0);
    g.a.mov_x(CTX, 1);
    g.a.mov_x(RAM, 2);
    g.a.mov_x(XIP, 3);
    g.a.mov(LIMIT, 4);
    g.a.mov_x(FLAGS, 5);
    if observe_pc { g.a.ldr(STOP, CTX, OFF_STOP_PC); }
    g.a.movz(DONE, 0, 0);
    let body = g.a.here();
    if !(0x0200_0000..0x0400_0000).contains(&pc0) {
        let off = pc0.wrapping_sub(mem.ram_base) as usize;
        let len = crate::block_code_len(pc0, ops);
        let len = len.min((mem.ram_len as usize).checked_sub(off)?);
        // SAFETY: jit_mem provides a live RAM allocation, and the range is checked above.
        let bytes = unsafe { std::slice::from_raw_parts(mem.ram.add(off), len) }.into();
        guards.push(Box::new(RamGuard { pc: pc0, bytes, version: 0, tracked: 0 }));
        let guard = guards.last_mut().unwrap().as_mut();
        let (valid, slow) = (g.a.label(), g.a.label());
        // Helpers leave immediately after any code write. Until then the per-call
        // generation is stable, so chained RAM bodies can validate it without a
        // Rust call. Untracked buses still compare the immutable bytes every time.
        g.a.mov64(1, guard as *mut RamGuard as u64);
        g.a.ldr(9, CTX, std::mem::offset_of!(Ctx, guard_tracked) as u32);
        g.a.cbz(9, slow);
        g.a.ldr(9, 1, std::mem::offset_of!(RamGuard, tracked) as u32);
        g.a.cbz(9, slow);
        g.a.ldr_x(9, CTX, std::mem::offset_of!(Ctx, guard_version) as u32);
        g.a.ldr_x(10, 1, std::mem::offset_of!(RamGuard, version) as u32);
        g.a.cmp_x(9, 10);
        g.a.b_cond(AC::Eq, valid);
        g.a.bind(slow);
        g.a.mov_x(0, CTX);
        g.call(h_ram_ok::<B> as *const () as u64);
        g.a.cbnz(0, valid);
        g.exit_imm(pc0, 0);
        g.a.bind(valid);
    }

    // A fixed allocation keeps branch arms coherent: every cache register is always
    // valid, helpers publish/reload it, and every exit publishes it. The selected
    // caller-saved registers are unused by the instruction emitters. w4 remains
    // scratch because write-helper arguments are prepared before cache flushing.
    let mut uses = [0u32; 16];
    for op in ops {
        if let Some((reads,writes)) = op.fast.regs_rw() {
            for r in 0..16 { uses[r] += u32::from(reads & (1 << r) != 0) + u32::from(writes & (1 << r) != 0); }
        }
    }
    g.fp_cached_regs=fpr::allocate(ops);
    let mut order: Vec<_> = (0..16).filter(|&r| uses[r] >= 6
        && !g.fp_cached_regs.iter().any(|&(guest,_)|guest as usize==r)).collect();
    order.sort_by_key(|&r| std::cmp::Reverse(uses[r]));
    g.cached_regs = order.into_iter().zip([14,15,16,17,5,6,7,8,13]).map(|(r,host)| (r as u8,host)).collect();
    g.cache_active = true;
    g.reload_regs();
    // Keep the full-prefix path free of per-instruction budget checks. A second copy
    // handles short CPU1 probes and peripheral deadlines without returning to Rust
    // just because the whole prefix does not fit. Both copies share the code guard.
    let partial = g.a.label();
    g.a.add_imm(9, DONE, ops.len() as u32);
    g.a.cmp(9, LIMIT);
    g.a.b_cond(AC::Hi, partial);
    // An observation PC strictly inside this prefix needs per-instruction checks.
    // The common case (including the prefix's own entry PC) keeps the full path.
    if observe_pc {
        let full = g.a.label();
        g.a.mov32(9, pc0);
        g.a.sub(9, STOP, 9);
        g.a.cbz(9, full);
        g.a.cmp_imm(9, ops.iter().map(|o| o.len as u32).sum());
        g.a.b_cond(AC::Lo, partial);
        g.a.bind(full);
    }
    for bounded in [false, true] {
    if bounded { g.a.bind(partial); }
    g.known_regs.fill(None);
    let mut pc = pc0;
    for (i, op) in ops.iter().enumerate() {
        let idx = i as u32;
        let next_pc = pc + op.len as u32;
        let count = idx + 1;
        if bounded {
            let fits = g.a.label();
            g.a.add_imm(9, DONE, count);
            g.a.cmp(9, LIMIT);
            g.a.b_cond(AC::Ls, fits);
            if i > 0 && ops[i - 1].glue {
                // The same split-pair interrupt barrier as run_fast's exact budget exit.
                g.a.mov32(9, 1);
                g.a.str(9, CPU, off_hold());
            }
            g.exit_imm(pc, idx);
            g.a.bind(fits);
        }
        if i == 0 {
            if let Some(counter) = counter {
                g.a.mov64(10,counter as u64);
                g.a.ldr_x(9,10,if bounded{8}else{0});
                g.a.add_imm_x(9,9,1);
                g.a.str_x(9,10,if bounded{8}else{0});
            }
        }
        g.publish_flags = bounded || !flags_overwritten(&ops[i + 1..]);
        let mut bus_used = false;
        match op.fast {
            fast @ Fast::FOp{..} if fpr::emit(&mut g,fast)=>{}
            Fast::Raw { route, raw } if raw_lower::emit(&mut g, route, raw, idx, &mut bus_used) => {}
            Fast::Nop => {}
            Fast::MovI { d, v } => {
                g.a.mov32(0, v);
                g.st_reg(0, d);
            }
            Fast::Mov { d, s } => {
                g.ld_reg(0, s);
                g.st_reg(0, d);
            }
            Fast::SetIe { on:false } => {
                g.a.mov32(0,std::mem::offset_of!(Cpu,ie) as u32);
                g.a.strb_u(crate::jit_a64::ZR,CPU,0);
            }
            Fast::SetIe { on:true } => {
                g.a.mov_x(0,CTX); g.a.add_imm(1,DONE,idx);
                g.call(if profile { h_irq_window::<B,true> as *const () as u64 }
                    else { h_irq_window::<B,false> as *const () as u64 });
                bus_used=true;
            }
            Fast::Div { d,a,b,k } => {
                g.ld_reg(0,a); g.ld_reg(1,b);
                if k==0 || k==2 { g.a.sdiv(2,0,1); } else { g.a.udiv(2,0,1); }
                if k>=2 {
                    g.a.mul(3,2,1); g.a.sub(0,0,3);
                    // AArch64 division by zero returns a zero quotient, whose
                    // raw remainder would be the dividend. The guest returns 0.
                    g.a.cmp_imm(1,0); g.a.csel(0,0,crate::jit_a64::ZR,AC::Ne);
                } else { g.a.mov(0,2); }
                g.st_reg(0,d);
            }
            Fast::Un { d,s,k } => {
                g.ld_reg(0,s);
                match k {
                    0 => { g.a.mov32(1,u32::MAX); g.a.eor(0,0,1); }
                    1 => g.a.neg(0,0),
                    2 => { g.a.cmp_imm(0,0); g.a.cneg(0,0,AC::Lt); }
                    3 => g.a.rev(0,0),
                    4 => g.a.clz(0,0),
                    _ => {
                        g.a.mov32(1,if k==5 { (-32768i32) as u32 } else { 0 });
                        g.a.cmp(0,1); g.a.csel(0,0,1,AC::Ge);
                        g.a.mov32(1,if k==5 { 32767 } else { 65535 });
                        g.a.cmp(0,1); g.a.csel(0,0,1,AC::Le);
                    }
                }
                g.st_reg(0,d);
            }
            Fast::AddRI { d, s, v } => {
                g.ld_reg(0, s);
                g.a.mov32(1, v);
                g.alu_flags(Op::Add);
                g.st_reg(0, d);
            }
            Fast::AddRR { d, a, b } | Fast::SubRR { d, a, b } => {
                g.ld_reg(0, a); g.ld_reg(1, b);
                g.alu_flags(if matches!(op.fast, Fast::AddRR { .. }) { Op::Add } else { Op::Sub });
                g.st_reg(0, d);
            }
            Fast::Carry {d,a,b,sub} => {
                g.ld_reg(0,a);g.ld_reg(1,b);
                g.a.ldr(2,CPU,std::mem::offset_of!(Cpu,flags) as u32);
                g.a.ubfx(2,2,1,1);g.a.cmp_imm(2,1);
                if sub{g.a.sbcs(0,0,1);}else{g.a.adcs(0,0,1);}
                if g.publish_flags{g.arithmetic_flags();}
                g.st_reg(0,d);
            }
            Fast::AndI { d, s, v } | Fast::MulI { d, s, v } => {
                g.ld_reg(0, s); g.a.mov32(1, v);
                if matches!(op.fast, Fast::AndI { .. }) { g.a.and(0, 0, 1); }
                else { g.a.mul(0, 0, 1); }
                g.st_reg(0, d);
            }
            Fast::AndBit { d, s, r } => {
                g.ld_reg(0, s); g.ld_reg(1, r); g.a.mov32(2, 1);
                g.a.lslv(1, 2, 1); g.a.and(0, 0, 1); g.st_reg(0, d);
            }
            Fast::ShlR { d, a, b } | Fast::ShrR { d, a, b } => {
                g.ld_reg(0, a); g.ld_reg(1, b);
                if matches!(op.fast, Fast::ShlR { .. }) { g.a.lslv(0, 0, 1); }
                else { g.a.lsrv(0, 0, 1); }
                g.st_reg(0, d);
            }
            Fast::ShrI { d, s, n } => {
                g.ld_reg(0, s); g.a.lsr_imm(0, 0, n as u32 & 31); g.st_reg(0, d);
            }
            Fast::SpFromR { s } => { g.ld_reg(0, s); g.a.str(0, CPU, off_sp()); }
            Fast::ShlI { d, s, n } => {
                g.ld_reg(0, s);
                g.a.lsl_imm(0, 0, n as u32 & 31);
                g.st_reg(0, d);
            }
            Fast::SarI { d, s, n } => {
                g.ld_reg(0, s);
                g.a.asr_imm(0, 0, n as u32 & 31);
                g.st_reg(0, d);
            }
            Fast::Alu3 { d, a, b, op } => {
                g.ld_reg(0, a);
                g.ld_reg(1, b);
                g.alu_flags(op);
                g.st_reg(0, d);
            }
            Fast::Alu3I { d, a, v, op } => {
                g.ld_reg(0, a);
                g.a.mov32(1, v);
                g.alu_flags(op);
                g.st_reg(0, d);
            }
            Fast::Alu2 { d, b, op } => {
                g.ld_reg(0, d);
                g.ld_reg(1, b);
                g.alu_flags(op);
                g.st_reg(0, d);
            }
            Fast::Alu2I { d, v, op } => {
                g.ld_reg(0, d);
                g.a.mov32(1, v);
                g.alu_flags(op);
                g.st_reg(0, d);
            }
            Fast::A4 { d, s, op, rk, rv } => {
                g.rhs(rk, rv);
                g.ld_reg(0, s);
                g.alu_flags(op);
                g.st_reg(0, d);
            }
            Fast::Ext { d, s, part, signed } => {
                g.ld_reg(0, s);
                match (part, signed) {
                    (0, false) => g.a.ubfx(0, 0, 0, 8),
                    (0, true) => g.a.sbfx(0, 0, 0, 8),
                    (1, false) => g.a.ubfx(0, 0, 0, 16),
                    (1, true) => g.a.sbfx(0, 0, 0, 16),
                    (_, false) => g.a.ubfx(0, 0, 16, 16),
                    (_, true) => g.a.sbfx(0, 0, 16, 16),
                }
                g.st_reg(0, d);
            }
            Fast::MinMax { d, a, b, kind } => {
                g.ld_reg(0, a);
                g.ld_reg(1, b);
                g.a.cmp(0, 1);
                let c = match kind { 0 => AC::Lt, 1 => AC::Gt, 2 => AC::Lo, _ => AC::Hi };
                g.a.csel(0, 0, 1, c);
                g.st_reg(0, d);
            }
            // ---- branches (always a block's last op)
            Fast::BrI { a, v, cond, target } => {
                let t = g.a.label();
                g.ld_reg(0, a);
                g.a.mov32(1, v);
                g.a.cmp(0, 1);
                g.a.b_cond(acond(cond), t);
                g.chain(next_pc, count);
                g.a.bind(t);
                g.chain(target, count);
            }
            Fast::BrIEq { a, v, target } | Fast::BrINe { a, v, target } => {
                let t = g.a.label();
                g.ld_reg(0, a);
                g.a.mov32(1, v);
                g.a.cmp(0, 1);
                g.a.b_cond(if matches!(op.fast, Fast::BrIEq { .. }) { AC::Eq } else { AC::Ne }, t);
                g.chain(next_pc, count);
                g.a.bind(t);
                g.chain(target, count);
            }
            Fast::BrR { a, b, cond, target } => {
                let t = g.a.label();
                g.ld_reg(0, a);
                g.ld_reg(1, b);
                g.cmp(cond.is_float(), 0, 1);
                g.a.b_cond(acond(cond), t);
                g.chain(next_pc, count);
                g.a.bind(t);
                g.chain(target, count);
            }
            Fast::BitBr { r, mk, ne, mv, target } => {
                let t = g.a.label();
                g.rhs(mk, mv);
                g.ld_reg(0, r);
                g.a.tst(0, 1);
                g.a.b_cond(if ne { AC::Ne } else { AC::Eq }, t);
                g.chain(next_pc, count);
                g.a.bind(t);
                g.chain(target, count);
            }
            Fast::DecBr { r, cond, v, target } => {
                let t = g.a.label();
                g.ld_reg(0, r);
                g.a.sub_imm(0, 0, 1);
                g.st_reg(0, r);
                g.a.mov32(1, v);
                g.a.cmp(0, 1);
                g.a.b_cond(acond(cond), t);
                g.chain(next_pc, count);
                g.a.bind(t);
                g.chain(target, count);
            }
            Fast::Goto { target } => g.chain(target, count),
            Fast::Call { target } => {
                g.a.mov32(0, next_pc);
                g.a.str(0, CPU, off_rets());
                g.chain(target, count);
            }
            Fast::CallReg { r } => {
                g.ld_reg(KEEP,r); g.a.mov32(0,next_pc); g.a.str(0,CPU,off_rets());
                g.exit_reg(KEEP,count);
            }
            Fast::CcBr { want,target } => {
                let taken=g.a.label();
                g.a.mov32(1,std::mem::offset_of!(Cpu,cc) as u32); g.a.ldrb_u(0,CPU,1);
                if want { g.a.cbnz(0,taken); } else { g.a.cbz(0,taken); }
                g.chain(next_pc,count); g.a.bind(taken); g.chain(target,count);
            }
            Fast::TestSet { r } => {
                g.ld_reg(KEEP,r); g.a.mov(0,KEEP); g.load(1,false,idx);
                g.a.cmp_imm(0,0); g.a.cset(1,AC::Ne);
                g.a.mov32(2,std::mem::offset_of!(Cpu,cc) as u32); g.a.strb_u(1,CPU,2);
                g.a.mov32(1,0x80); g.a.orr(1,0,1); g.a.mov(0,KEEP); g.store(1,idx);
                bus_used=true;
            }
            Fast::IfBlk { a, rk, fam, alt, has_else: false, rv, then_d, .. } => {
                // without an else part a predicated block is a conditional jump over its then part
                let taken = g.a.label();
                g.rhs(rk, rv);
                g.ld_reg(0, a);
                let c = if fam == 2 {
                    g.a.tst(0, 1);
                    if alt { AC::Ne } else { AC::Eq }
                } else {
                    g.cmp(fam >= 8, 0, 1);
                    acond(crate::cond_of_fam(fam as u64, alt))
                };
                g.a.b_cond(c, taken);
                g.chain(next_pc.wrapping_add(then_d as u32), count);
                g.a.bind(taken);
                g.chain(next_pc, count);
            }
            Fast::FOp { d, a, b, op: mac @ (7 | 8) } => {
                // Read all operands before writing d: any of them may alias.
                let (slow, done) = (g.a.label(), g.a.label());
                g.ld_reg(0, a); g.ld_reg(1, b); g.ld_reg(2, d);
                for r in 0..3 {
                    g.a.ubfx(9, r, 23, 8);
                    g.a.cmp_imm(9, 255);
                    g.a.b_cond(AC::Eq, slow);
                }
                g.a.fmov_s_w(0, 0); g.a.fmov_s_w(1, 1); g.a.fmov_s_w(2, 2);
                // PI32 rounds the product before accumulation: never emit FMLA.
                g.a.fop(0x1e20_0800, 0, 0, 1);
                g.a.fop(if mac == 7 { 0x1e20_2800 } else { 0x1e20_3800 }, 0, 2, 0);
                g.a.fmov_w_s(0, 0); g.st_reg(0, d);
                g.a.b(done);
                g.a.bind(slow);
                // Rust may commute FP operands, changing NaN payload selection. Preserve
                // its exact oracle for nonfinite operands, including invalid 0 * infinity.
                g.helpers.push(Box::new(op.fast));
                let p = g.helpers.last().unwrap().as_ref() as *const Fast;
                g.a.mov_x(0, CTX); g.a.mov64(1, p as u64);
                g.idx_w2(idx); g.a.mov32(3, next_pc); g.call(g.h_op);
                if op.glue { g.check_brk_hold(next_pc, count); }
                else { g.check_brk(next_pc, count); }
                g.a.bind(done);
            }
            Fast::SatRR {d,a,b,sub} => {
                g.ld_reg(0,a);g.ld_reg(1,b);g.a.sxtw(0,0);g.a.sxtw(1,1);
                if sub {g.a.sub_x(0,0,1);} else {g.a.add_x(0,0,1);}
                g.a.mov64(2,i32::MAX as u64);g.a.cmp_x(0,2);g.a.csel(3,2,0,AC::Gt);
                g.a.mov64(2,i32::MIN as i64 as u64);g.a.cmp_x(0,2);g.a.csel(0,2,3,AC::Lt);
                g.st_reg(0,d);
            }
            Fast::PairExtract {d,lo,n,signed} => {
                g.ld_reg(0,lo);g.ld_reg(1,lo+1);g.a.add_x_lsl(0,0,1,32);
                if signed{g.a.asr_imm_x(0,0,n as u32);}else{g.a.lsr_imm_x(0,0,n as u32);}
                g.st_reg(0,d);
            }
            Fast::FOp { d, a, b, op } => {
                g.ld_reg(0, a);
                match op {
                    0x1f | 0x5f => {
                        g.a.fmov_s_w(0, 0);
                        if op == 0x1f { g.a.fcvtzs(0, 0) } else { g.a.fcvtzu(0, 0) }
                    }
                    0x8f | 0x9f => {
                        if op == 0x8f { g.a.scvtf(0, 0) } else { g.a.ucvtf(0, 0) }
                        g.a.fmov_w_s(0, 0);
                    }
                    _ => {
                        g.ld_reg(1, b);
                        g.a.fmov_s_w(0, 0);
                        g.a.fmov_s_w(1, 1);
                        let opc = match op { 0 => 0x1e20_2800, 1 => 0x1e20_3800, 2 => 0x1e20_0800, 3 => 0x1e20_1800, 5 => 0x1e20_7800, _ => 0x1e20_6800 };
                        g.a.fop(opc, 0, 0, 1);
                        g.a.fmov_w_s(0, 0);
                    }
                }
                g.st_reg(0, d);
            }
            Fast::Rts => {
                g.a.ldr(9, CPU, off_rets());
                g.exit_reg(9, count);
            }
            // ---- memory
            Fast::Ld { d, b, off, w, signed } => {
                bus_used = g.load_base(b, off, w, signed, idx);
                g.st_reg(0, d);
            }
            Fast::LdW { d, b, off } => {
                bus_used = g.load_base(Base::Reg(b), off, 4, false, idx);
                g.st_reg(0, d);
            }
            Fast::LdWSp { d, off } => {
                g.a.ldr(0, CPU, off_sp());
                g.add_imm(0, 0, off);
                g.load(4, false, idx);
                g.st_reg(0, d);
                bus_used = true;
            }
            Fast::LdBu { d, b, off } => {
                bus_used = g.load_base(Base::Reg(b), off, 1, false, idx);
                g.st_reg(0, d);
            }
            Fast::St { s, b, off, w } => {
                g.store_base(b, off, s, w, idx);
                bus_used = true;
            }
            Fast::StW { s, b, off } => {
                g.store_base(Base::Reg(b), off, s, 4, idx);
                bus_used = true;
            }
            Fast::StWSp { s, off } => {
                g.a.ldr(0, CPU, off_sp());
                g.add_imm(0, 0, off);
                g.ld_reg(1, s);
                g.store(4, idx);
                bus_used = true;
            }
            Fast::LdIdx { d, b, i, sh, w, signed } => {
                g.ld_reg(0, b);
                g.ld_reg(1, i);
                g.a.add_lsl(0, 0, 1, sh as u32 & 31);
                g.load(w, signed, idx);
                g.st_reg(0, d);
                bus_used = true;
            }
            Fast::StIdx { s, b, i, sh, w } => {
                g.ld_reg(0, b);
                g.ld_reg(1, i);
                g.a.add_lsl(0, 0, 1, sh as u32 & 31);
                g.ld_reg(1, s);
                g.store(w, idx);
                bus_used = true;
            }
            Fast::LdPost { d, b, w, signed, inc } => {
                g.ld_reg(0, b);
                g.load(w, signed, idx);
                g.st_reg(0, d);
                g.ld_reg(0, b); // (after the load wrote d, as the interpreter)
                g.add_imm(0, 0, inc);
                g.st_reg(0, b);
                bus_used = true;
            }
            Fast::StPost { s, b, w, inc } => {
                g.ld_reg(0, b);
                g.ld_reg(1, s);
                g.store(w, idx);
                g.ld_reg(0, b);
                g.add_imm(0, 0, inc);
                g.st_reg(0, b);
                bus_used = true;
            }
            Fast::Rmw { b, w, op: rop, off, v } => {
                let known = g.known_regs[(b & 15) as usize].map(|a| a.wrapping_add(off));
                if let Some(addr) = known { g.a.mov32(KEEP,addr); }
                else { g.ld_reg(0,b); g.add_imm(KEEP,0,off); }
                if let RmwOp::Mov = rop {
                    g.a.mov32(1, v);
                } else {
                    if known.is_some() { g.load_base(Base::Reg(b),off,w,false,idx); }
                    else { g.a.mov(0,KEEP); g.load(w,false,idx); }
                    g.a.mov32(2, v);
                    match rop {
                        RmwOp::Or => g.a.orr(1, 0, 2),
                        RmwOp::And => g.a.and(1, 0, 2),
                        RmwOp::Xor => g.a.eor(1, 0, 2),
                        RmwOp::Add => g.a.add(1, 0, 2),
                        RmwOp::Sub => g.a.sub(1, 0, 2),
                        RmwOp::Mov => unreachable!(),
                    }
                }
                g.a.mov(0, KEEP);
                if known.is_some_and(|a| g.mem.ram_len < w as u32 || a.wrapping_sub(g.mem.ram_base) > g.mem.ram_len - w as u32) {
                    g.write_helper(w,idx);
                } else { g.store(w, idx); }
                bus_used = true;
            }
            Fast::MemA4 { b, off, op: aop, rk, rv } => {
                let known = g.known_regs[(b & 15) as usize].map(|a| a.wrapping_add(off as u32));
                g.rhs(rk, rv);
                g.a.mov(KEEP2, 1);
                if let Some(addr) = known { g.a.mov32(KEEP,addr); }
                else { g.ld_reg(0,b); g.add_imm(KEEP,0,off as u32); }
                if known.is_some() { g.load_base(Base::Reg(b),off as u32,4,false,idx); }
                else { g.a.mov(0,KEEP); g.load(4,false,idx); }
                g.a.mov(1, KEEP2);
                g.alu(aop);
                g.a.mov(1, 0);
                g.a.mov(0, KEEP);
                if known.is_some_and(|a| g.mem.ram_len < 4 || a.wrapping_sub(g.mem.ram_base) > g.mem.ram_len - 4) {
                    g.write_helper(4,idx);
                } else { g.store(4, idx); }
                bus_used = true;
            }
            Fast::SpAdd { v } => {
                g.a.ldr(0, CPU, off_sp()); g.add_imm(0, 0, v);
                g.a.str(0, CPU, off_sp());
            }
            Fast::RFromSp { d, v } => {
                g.a.ldr(0, CPU, off_sp()); g.add_imm(0, 0, v); g.st_reg(0, d);
            }
            Fast::Mov2 { dh, dl, sh, sl } => {
                g.ld_reg(0, sh); g.ld_reg(1, sl);
                g.st_reg(0, dh); g.st_reg(1, dl);
            }
            Fast::Mov2I { dh, dl, v } => {
                g.a.mov32(0, v); g.st_reg(0, dl);
                g.a.mov32(0, ((v as i32) >> 31) as u32); g.st_reg(0, dh);
            }
            Fast::BitExt { d, s, p, l, signed } => {
                let (p, l) = (p as u32 & 31, l as u32 & 31);
                g.ld_reg(0, s); g.a.lsr_imm(0, 0, p);
                if l != 0 {
                    if signed { g.a.sbfx(0, 0, 0, l); } else { g.a.ubfx(0, 0, 0, l); }
                }
                g.st_reg(0, d);
            }
            Fast::BitIns { d, s, p, l } => {
                let (p, l) = (p as u32 & 31, l as u32 & 31);
                let mask = if l == 0 { u32::MAX } else { (1u32 << l) - 1 };
                g.ld_reg(0, s); g.ld_reg(1, d);
                g.a.mov32(2, mask); g.a.and(0, 0, 2); g.a.lsl_imm(0, 0, p);
                g.a.mov32(2, !(mask << p)); g.a.and(1, 1, 2); g.a.orr(0, 0, 1);
                g.st_reg(0, d);
            }
            Fast::StackMask { mask, pop } => {
                let words = mask.count_ones();
                if words != 0 {
                    let (slow, done) = (g.a.label(), g.a.label());
                    g.stack_ram(words, !pop, slow);
                    let mut offset = 0;
                    for r in 0..16 { if mask >> r & 1 != 0 {
                        if pop { g.a.ldr(0, 12, offset); g.st_reg(0, r); }
                        else { g.ld_reg(0, r); g.a.str(0, 12, offset); }
                        offset += 4;
                    }}
                    if pop { g.a.add_imm(10, 10, words * 4); }
                    g.a.str(10, CPU, off_sp()); g.a.b(done);
                    g.a.bind(slow);
                    for i in 0..16 {
                        let r = if pop { i } else { 15 - i };
                        if mask >> r & 1 == 0 { continue; }
                        g.a.ldr(0, CPU, off_sp());
                        if pop {
                            g.load(4, false, idx); g.st_reg(0, r);
                            g.a.ldr(0, CPU, off_sp()); g.a.add_imm(0, 0, 4);
                            g.a.str(0, CPU, off_sp());
                        } else {
                            g.a.sub_imm(0, 0, 4); g.a.str(0, CPU, off_sp());
                            g.ld_reg(1, r); g.store(4, idx);
                        }
                    }
                    g.a.bind(done);
                    bus_used = true;
                }
            }
            Fast::SpecialStack { regs, len, pop }
                if !regs[..len as usize].contains(&SpecialReg::Sp)
                    && (pop || !regs[..len as usize].contains(&SpecialReg::Icfg)) => {
                let regs = &regs[..len as usize];
                let (slow, done) = (g.a.label(), g.a.label());
                g.stack_ram(len as u32, !pop, slow);
                let reti = regs.contains(&SpecialReg::Reti);
                if reti {
                    g.a.mov32(0, u32::from(!pop));
                    g.a.mov32(1, std::mem::offset_of!(Cpu, nest_ok) as u32);
                    g.a.strb_u(0, CPU, 1);
                }
                for i in 0..len as usize {
                    let reg = regs[if pop { len as usize - i - 1 } else { i }];
                    let offset = (len as usize - i - 1) as u32 * 4;
                    let field = match reg {
                        SpecialReg::Rets => off_rets(),
                        SpecialReg::Ssp => std::mem::offset_of!(Cpu, ssp) as u32,
                        SpecialReg::Usp => std::mem::offset_of!(Cpu, usp) as u32,
                        _ => std::mem::offset_of!(Cpu, sreg_store) as u32 + reg.store_index() as u32 * 4,
                    };
                    if pop {
                        // All operands are plain CPU fields in this path. Read
                        // ascending like the bus oracle, assigning reversed order.
                        g.a.ldr(0, 12, i as u32 * 4);
                        g.a.str(0, CPU, field);
                        if reg == SpecialReg::Psr {
                            g.a.mov32(1, 15); g.a.and(0, 0, 1);
                            g.a.str(0, CPU, std::mem::offset_of!(Cpu, flags) as u32);
                        }
                    } else {
                        g.a.ldr(0, CPU, field);
                        if reg == SpecialReg::Psr {
                            g.a.mov32(1, !15); g.a.and(0, 0, 1);
                            g.a.ldr(1, CPU, std::mem::offset_of!(Cpu, flags) as u32);
                            g.a.orr(0, 0, 1);
                        }
                        g.a.str(0, 12, offset);
                    }
                }
                if pop { g.a.add_imm(10, 10, len as u32 * 4); }
                g.a.str(10, CPU, off_sp());
                if (!pop && reti) || (pop && regs.contains(&SpecialReg::Icfg)) {
                    g.a.mov_x(0, CTX); g.a.add_imm(1, DONE, idx);
                    g.call(if profile { h_stack_window::<B, true> as *const () as u64 }
                        else { h_stack_window::<B, false> as *const () as u64 });
                }
                g.a.b(done);
                g.a.bind(slow);
                // MMIO, watched stores, unaligned or out-of-range RAM retain
                // the typed executor's exact SP changes and bus access order.
                g.helpers.push(Box::new(op.fast));
                let helper = g.helpers.last().unwrap().as_ref() as *const Fast;
                g.a.mov_x(0, CTX); g.a.mov64(1, helper as u64);
                g.idx_w2(idx); g.a.mov32(3, next_pc); g.call(g.h_op);
                g.a.bind(done);
                bus_used = true;
            }
            Fast::PushRets { n } => {
                let (top, bot) = ((n as i32).max(3), (n as i32).min(4));
                let words = (top - bot + 2) as u32;
                let (slow, done) = (g.a.label(), g.a.label());
                g.stack_ram(words, true, slow);
                g.a.ldr(1, CPU, off_rets()); g.a.str(1, 12, (words - 1) * 4);
                for r in (bot..=top).rev() {
                    g.ld_reg(1, r as u8); g.a.str(1, 12, (r - bot) as u32 * 4);
                }
                g.a.str(10, CPU, off_sp()); g.a.b(done);
                g.a.bind(slow);
                for r in std::iter::once(-1).chain((bot..=top).rev()) {
                    g.a.ldr(0, CPU, off_sp()); g.a.sub_imm(0, 0, 4);
                    g.a.str(0, CPU, off_sp());
                    if r < 0 { g.a.ldr(1, CPU, off_rets()); } else { g.ld_reg(1, r as u8); }
                    g.store(4, idx);
                }
                g.a.bind(done);
                bus_used = true;
            }
            Fast::PopPc { n } => {
                let (top, bot) = ((n as i32).max(3), (n as i32).min(4));
                let words = (top - bot + 2) as u32;
                let slow = g.a.label();
                g.stack_ram(words, false, slow);
                for r in bot..=top {
                    g.a.ldr(0, 12, (r - bot) as u32 * 4); g.st_reg(0, r as u8);
                }
                g.a.ldr(KEEP, 12, (words - 1) * 4);
                g.a.add_imm(10, 10, words * 4); g.a.str(10, CPU, off_sp());
                g.exit_reg(KEEP, count);
                g.a.bind(slow);
                for r in bot..=top {
                    g.a.ldr(0, CPU, off_sp()); g.load(4, false, idx); g.st_reg(0, r as u8);
                    g.a.ldr(0, CPU, off_sp()); g.a.add_imm(0, 0, 4); g.a.str(0, CPU, off_sp());
                }
                g.a.ldr(0, CPU, off_sp()); g.load(4, false, idx); g.a.mov(KEEP, 0);
                g.a.ldr(0, CPU, off_sp()); g.a.add_imm(0, 0, 4); g.a.str(0, CPU, off_sp());
                g.exit_reg(KEEP, count);
            }
            Fast::PopRets => {
                g.a.ldr(0, CPU, off_sp()); g.load(4, false, idx); g.a.str(0, CPU, off_rets());
                g.a.ldr(0, CPU, off_sp()); g.a.add_imm(0, 0, 4); g.a.str(0, CPU, off_sp());
                bus_used = true;
            }
            Fast::Tb { r, half } => {
                g.ld_reg(0, r); g.add_imm(0, 0, next_pc);
                g.load(if half { 2 } else { 1 }, false, idx);
                g.a.lsl_imm(0, 0, 1); g.add_imm(0, 0, next_pc);
                g.exit_reg(0, count);
            }
            Fast::PushRet => {
                g.a.ldr(0, CPU, off_sp());
                g.a.sub_imm(0, 0, 4);
                g.a.str(0, CPU, off_sp());
                g.a.ldr(1, CPU, off_rets());
                g.store(4, idx);
                bus_used = true;
            }
            Fast::PopRet => {
                g.a.ldr(0, CPU, off_sp());
                g.load(4, false, idx);
                g.a.mov(KEEP, 0);
                g.a.ldr(0, CPU, off_sp());
                g.a.add_imm(0, 0, 4);
                g.a.str(0, CPU, off_sp());
                g.exit_reg(KEEP, count);
            }
            // ---- everything else: the interpreter's executor for this one op
            other => {
                g.helpers.push(Box::new(other));
                let p = g.helpers.last().unwrap().as_ref() as *const Fast;
                g.a.mov_x(0, CTX);
                g.a.mov64(1, p as *const Fast as u64);
                g.idx_w2(idx);
                g.a.mov32(3, next_pc);
                g.call(g.h_op);
                let fall = g.a.label();
                g.a.tbz(0, 32, fall); // bit 32: jumped (x0)
                g.exit_reg(0, count);
                g.a.bind(fall);
                bus_used = true;
                if other.may_jump() {
                    // a branch through the interpreter (the float compares) that fell through
                    g.chain(next_pc, count);
                }
            }
        }
        g.advance_constants(op.fast);
        if bus_used && !op.fast.may_jump() {
            if op.glue {
                // leaving between a glued pair's two instructions: interrupts wait for the second
                g.check_brk_hold(next_pc, count);
            } else {
                g.check_brk(next_pc, count);
            }
        }
        if bounded && !op.fast.may_jump() { g.check_stop(next_pc, count, op.glue); }
        pc = next_pc;
        if op.fast.may_jump() {
            break; // (the block's last op)
        }
    }
    // fell off the end of the prefix
    let n = ops.len() as u32;
    if !ops.last().map_or(false, |o| o.fast.may_jump()) {
        g.chain(pc, n);
    }
    }
    g.a.bind(exit);
    g.a.ldp(LIMIT, 28, SP, 80);
    g.a.ldp(25, DONE, SP, 64);
    g.a.ldp(KEEP, KEEP2, SP, 48);
    g.a.ldp(RAM, XIP, SP, 32);
    g.a.ldp(CPU, CTX, SP, 16);
    g.a.ldp_post(29, 30, SP, FRAME);
    g.a.ret();
    Some((g.a.finish(), body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BOp;
    struct Memory { data: Vec<u8>, flags: Vec<u8>, version: u64, ticks: u64, due_at: u64, stop_on_write: bool, untracked_xip: bool, code_views: u32, irq_windows:u32, irq_effect:u8 }
    impl Memory {
        fn new() -> Self { Self { data: vec![0; 256], flags: vec![1; 65], version: 0, ticks: 0, due_at: 1001, stop_on_write: false, untracked_xip: false, code_views: 0, irq_windows:0, irq_effect:0 } }
        fn wrote(&mut self) {
            self.version += 1;
            if self.stop_on_write { self.due_at = self.ticks + 1; }
        }
    }
    impl Bus for Memory {
        fn read8(&mut self, a: u32) -> u8 { self.data[a as usize] }
        fn read16(&mut self, a: u32) -> u16 { u16::from_le_bytes(self.data[a as usize..a as usize+2].try_into().unwrap()) }
        fn read32(&mut self, a: u32) -> u32 { u32::from_le_bytes(self.data[a as usize..a as usize+4].try_into().unwrap()) }
        fn write8(&mut self, a: u32, v: u8) { self.data[a as usize] = v; self.wrote(); }
        fn write16(&mut self, a: u32, v: u16) { self.data[a as usize..a as usize+2].copy_from_slice(&v.to_le_bytes()); self.wrote(); }
        fn write32(&mut self, a: u32, v: u32) { self.data[a as usize..a as usize+4].copy_from_slice(&v.to_le_bytes()); self.wrote(); }
        fn tick_room(&self) -> u64 { self.due_at.saturating_sub(self.ticks + 1) }
        fn next_tick_due(&self) -> bool { self.ticks + 1 >= self.due_at }
        fn add_ticks(&mut self, n: u64) { self.ticks += n; }
        fn irq_window(&mut self) {
            self.irq_windows+=1;
            match self.irq_effect {
                1 => self.due_at=self.ticks+1,
                2 => self.version+=1,
                3 => panic!("synthetic interrupt-window failure"),
                _ => {}
            }
        }
        fn code_writes(&self) -> Option<u64> { if self.untracked_xip { None } else { Some(self.version) } }
        fn xip_writes(&self) -> u64 { if self.untracked_xip { self.version } else { 0 } }
        fn jit_mem(&mut self) -> Option<JitMem> {
            self.code_views += 1;
            Some(JitMem { ram: self.data.as_mut_ptr(), ram_base: 0, ram_len: 256,
                ram_code: self.flags.as_ptr(), xip: std::ptr::null(), xip_base: 0x02000000, xip_len: 0 })
        }
    }
    fn op(fast: Fast) -> BOp { BOp { fast, len: 2, careful: false, pair_head: false, glue: false } }
    fn hot(j: &mut Jit, id: usize, pc: u32, ops: &[BOp], mem: &JitMem) -> *const u8 {
        for _ in 0..HOT + MAX_PENDING_VISITS + 1 {
            if let Some((entry, _)) = j.entry::<Memory>(id, ops, pc, mem) { return entry; }
        }
        panic!("hot block was not published within the bounded visit count")
    }

    fn batching_jit() -> Jit {
        let mut j = Jit::new(false).unwrap();
        // Exercise Android's staging and page rounding on the host as well.
        // Android itself retains its actual system page size, including 16 KiB.
        #[cfg(target_os = "macos")] { j.cache.page = 4096; }
        j.batch_bytes = j.cache.page;
        j
    }

    fn queue_hot(j: &mut Jit, id: usize, pc: u32, ops: &[BOp], mem: &JitMem) -> Option<(*const u8, u32)> {
        if j.state.len() <= id { j.state.resize(id + 1, Entry::Cold(0)); }
        j.state[id] = Entry::Cold(HOT - 1);
        j.entry::<Memory>(id, ops, pc, mem)
    }

    #[test]
    fn compilation_cost_counts_only_codegen_and_publication_and_survives_reset() {
        let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
        let ops = [op(Fast::Nop)];
        let mut j = Jit::new(false).unwrap(); j.batch_bytes = 0;
        assert_eq!(j.compilation_nanoseconds(), 0);
        assert!(j.entry::<Memory>(0, &ops, 0x02000000, &mem).is_none());
        assert_eq!((j.compilation_cost.attempts, j.publication_cost.attempts), (0, 0));
        hot(&mut j, 0, 0x02000000, &ops, &mem);
        assert_eq!((j.compilation_cost.attempts, j.publication_cost.attempts), (1, 1));
        let cost = j.compilation_nanoseconds();
        assert!(cost > 0);
        for _ in 0..5 { assert!(j.entry::<Memory>(0, &ops, 0x02000000, &mem).is_some()); }
        assert_eq!(j.compilation_nanoseconds(), cost, "cache hits never read the timing clock");
        j.reset();
        assert_eq!(j.compilation_nanoseconds(), cost, "cache resets retain lifetime diagnostic totals");
        let mut invalid = mem; invalid.ram_len = 0;
        assert!(queue_hot(&mut j, 0, 2, &ops, &invalid).is_none());
        assert_eq!((j.compilation_cost.attempts, j.publication_cost.attempts), (2, 1));
        assert!(j.cache_profile_json().contains("\"compileAttempts\":2"));
    }

    #[test]
    fn native_block_profile_counts_chains_after_guards_and_exact_admission() {
        const FIRST:u32=0x02000000;const NEXT:u32=FIRST+64;const THIRD:u32=NEXT+64;const END:u32=THIRD+64;
        let mut bus=Memory::new();let mem=bus.jit_mem().unwrap();
        let first=[op(Fast::MovI{d:0,v:7}),op(Fast::Goto{target:NEXT})];
        let second=[op(Fast::AddRI{d:0,s:0,v:1}),op(Fast::CallReg{r:7})];
        let third=[op(Fast::MovI{d:1,v:9}),op(Fast::Goto{target:END})];
        let mut j=Jit::new(true).unwrap();j.observe_pc();
        let code=hot(&mut j,0,FIRST,&first,&mem);
        hot(&mut j,1,NEXT,&second,&mem);hot(&mut j,2,THIRD,&third,&mem);
        let mut cpu=Cpu::new(FIRST);cpu.regs[7]=THIRD;
        assert_eq!(unsafe{run(code,&mut cpu,&mut bus,&mem,0,0)}.0,0);
        assert_eq!(j.native_blocks_json(),"[]","a zero budget retires no block");
        let (count,slot)=unsafe{run(code,&mut cpu,&mut bus,&mem,0,6)};
        assert_eq!(count,2);assert!(!slot.is_null());
        unsafe{*slot=j.body(1).unwrap()as u64;}
        cpu.pc=FIRST;bus.ticks=0;
        assert_eq!(unsafe{run(code,&mut cpu,&mut bus,&mem,0,6)}.0,6);
        assert_eq!((cpu.pc,cpu.regs[0],cpu.regs[1]),(END,8,9));
        assert_eq!(*j.native_blocks[0].entries,[2,0]);
        assert_eq!(*j.native_blocks[1].entries,[1,0]);
        assert_eq!(*j.native_blocks[2].entries,[1,0],"indirect chains count too");
        for (stop,due,want) in [(Some(NEXT),1001,2),(Some(NEXT+2),1001,3),(None,1,1)] {
            cpu.pc=FIRST;cpu.run_stop_pc=stop;bus.ticks=0;bus.due_at=due;
            assert_eq!(unsafe{run(code,&mut cpu,&mut bus,&mem,0,6)}.0,want);
        }
        assert_eq!(*j.native_blocks[0].entries,[4,1]);
        assert_eq!(*j.native_blocks[1].entries,[1,1]);
        let json=j.native_blocks_json();
        assert!(json.contains("\"fullPathEntries\":4")&&json.contains("\"boundedPathEntries\":1"));
        assert!(json.contains("CallReg")&&json.contains("\"count\":5"));
        j.reset();assert!(j.native_blocks.is_empty());assert_eq!(j.native_blocks_json(),"[]");

        let mut plain=Jit::new(false).unwrap();hot(&mut plain,0,FIRST,&first,&mem);
        assert!(plain.native_blocks.is_empty(),"profiling disabled allocates no counters");
        assert_eq!(plain.native_blocks_json(),"[]");

        // A stale RAM dependency exits before the first guest instruction and
        // must not contribute an apparent hot native block.
        let mut stale=Jit::new(true).unwrap();let code=hot(&mut stale,0,0,&first,&mem);
        bus.write8(0,7);cpu.pc=0;cpu.run_stop_pc=None;bus.ticks=0;bus.due_at=1001;
        assert_eq!(unsafe{run(code,&mut cpu,&mut bus,&mem,0,2)}.0,0);
        assert_eq!(stale.native_blocks_json(),"[]");
    }

    #[test]
    fn batching_publishes_complete_packed_entries_and_chains_only_after_rx() {
        const FIRST: u32 = 0x0200_0000;
        const NEXT: u32 = FIRST + 0x20;
        const END: u32 = FIRST + 0x40;
        let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
        let mut j = batching_jit();
        let a = [op(Fast::MovI { d: 0, v: 7 }), op(Fast::Goto { target: NEXT })];
        let b = [op(Fast::MovI { d: 1, v: 9 }), op(Fast::Goto { target: END })];
        assert!(queue_hot(&mut j, 0, FIRST, &a, &mem).is_none());
        assert!(matches!(j.state[0], Entry::Pending));
        assert!(j.body(0).is_none());
        assert!(j.targets.iter().all(|t| t.body == 0));
        assert_eq!((j.cache.used, j.cache.written, j.cache.publications), (0, 0, 0));
        let first_bytes = j.pending_words.len() * 4;
        // The second complete block crosses the batch threshold. Both become
        // executable together, and share one rounded reservation.
        j.batch_bytes = first_bytes + 4;
        let second = queue_hot(&mut j, 1, NEXT, &b, &mem).unwrap().0;
        let first = j.entry::<Memory>(0, &a, FIRST, &mem).unwrap().0;
        assert!(j.pending.is_empty() && j.pending_words.is_empty());
        assert_eq!(j.cache.publications, 1);
        assert_eq!(j.cache.used, j.cache.page);
        assert!(j.cache.written < j.cache.used);
        assert!(second as usize > first as usize && (second as usize) < first as usize + j.cache.page);
        let mut cpu = Cpu::new(FIRST);
        let (count, slot) = unsafe { run(first, &mut cpu, &mut bus, &mem, 0, 4) };
        assert_eq!((count, cpu.pc, cpu.regs[0]), (2, NEXT, 7));
        assert!(!slot.is_null());
        unsafe { *slot = j.body(1).unwrap() as u64; }
        cpu.pc = FIRST; bus.ticks = 0;
        assert_eq!(unsafe { run(first, &mut cpu, &mut bus, &mem, 0, 4) }.0, 4);
        assert_eq!((cpu.pc, cpu.regs[0], cpu.regs[1]), (END, 7, 9));
        assert_eq!(bus.ticks, 3);
    }

    #[test]
    fn batching_bounds_wait_by_all_entry_visits_and_pending_block_count() {
        let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
        let mut j = batching_jit();
        let ops = [op(Fast::Goto { target: 0x0200_0040 })];
        assert!(queue_hot(&mut j, 0, 0x0200_0000, &ops, &mem).is_none());
        // Even visits to another cold block age the pending publication.
        for _ in 1..MAX_PENDING_VISITS {
            assert!(j.entry::<Memory>(100, &[], 0x0200_0080, &mem).is_none());
            assert!(j.body(0).is_none());
        }
        assert!(j.entry::<Memory>(100, &[], 0x0200_0080, &mem).is_none());
        assert!(j.body(0).is_some());
        assert_eq!(j.cache.publications, 1);

        let mut j = batching_jit(); j.batch_bytes = usize::MAX;
        for id in 0..MAX_PENDING_BLOCKS {
            let result = queue_hot(&mut j, id, 0x0200_0000 + id as u32 * 32, &ops, &mem);
            assert_eq!(result.is_some(), id + 1 == MAX_PENDING_BLOCKS);
        }
        assert!(j.pending.is_empty());
        assert_eq!(j.cache.publications, 1);
        for id in 0..MAX_PENDING_BLOCKS { assert!(j.body(id).is_some()); }
    }

    #[test]
    fn batching_failed_publication_keeps_prior_code_and_auxiliary_owners_alive() {
        let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
        let mut j = batching_jit();j.profile=true;
        // The RAM guard, boxed nonfinite FPU fallback, and exit slots must all
        // survive a later batch's failed publication and rollback.
        let ops = [op(Fast::FOp { d: 0, a: 1, b: 2, op: 7 }), op(Fast::Goto { target: 64 })];
        assert!(queue_hot(&mut j, 0, 0, &ops, &mem).is_none());
        j.flush_pending();
        let first = j.entry::<Memory>(0, &ops, 0, &mem).unwrap().0;
        let owners = (j.guards.len(), j.slots.len(), j.helpers.len(),j.native_blocks.len());
        assert!(owners.0 > 0 && owners.1 > 0 && owners.2 > 0);
        let occupied = (j.cache.used, j.cache.written, j.cache.publications);
        assert!(queue_hot(&mut j, 1, 16, &ops, &mem).is_none());
        assert!(queue_hot(&mut j, 2, 32, &ops, &mem).is_none());
        j.cache.fail_next_publish = true;
        j.flush_pending();
        assert!(j.body(1).is_none() && j.body(2).is_none());
        assert!(matches!((j.state[1], j.state[2]), (Entry::No, Entry::No)));
        assert_eq!((j.guards.len(), j.slots.len(), j.helpers.len(),j.native_blocks.len()), owners);
        assert_eq!((j.cache.used, j.cache.written, j.cache.publications), occupied);
        for pc in [16, 32] {
            assert_eq!(j.targets[(pc >> 1) & (j.targets.len() - 1)].body, 0);
        }
        let mut cpu = Cpu::new(0); cpu.regs[1] = 0x7fc0_1234; cpu.regs[2] = 2f32.to_bits();
        let want = crate::fpu_d(7, 0, cpu.regs[1], cpu.regs[2]);
        assert_eq!(unsafe { run(first, &mut cpu, &mut bus, &mem, 0, 2) }.0, 2);
        assert_eq!((cpu.pc, cpu.regs[0]), (64, want));
    }

    #[test]
    fn batching_reset_and_observation_upgrade_cancel_unpublished_code() {
        for observation in [false, true] {
            let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
            let mut j = batching_jit();j.profile=true;
            let ops = [op(Fast::FOp { d: 0, a: 1, b: 2, op: 7 }), op(Fast::Goto { target: 64 })];
            assert!(queue_hot(&mut j, 0, 0, &ops, &mem).is_none());
            assert!(!j.guards.is_empty() && !j.helpers.is_empty() && !j.slots.is_empty());
            if observation { j.observe_pc(); } else { j.reset(); }
            assert!(j.pending.is_empty() && j.pending_words.is_empty() && j.pending_counts.is_none());
            assert!(j.state.is_empty() && j.guards.is_empty() && j.helpers.is_empty() && j.slots.is_empty());
            assert!(j.native_blocks.is_empty());
            assert!(j.targets.iter().all(|t| t.body == 0));
            assert_eq!((j.cache.used, j.cache.written, j.cache.publications, j.pending_visits), (0, 0, 0, 0));
            for _ in 0..MAX_PENDING_VISITS { assert!(j.entry::<Memory>(0, &ops, 0, &mem).is_none()); }
            assert!(j.body(0).is_none(), "canceled bytes cannot reappear through an aged queue");
            let fresh = [op(Fast::MovI { d: 0, v: 42 }), op(Fast::Goto { target: 64 })];
            let entry = hot(&mut j, 1, 0, &fresh, &mem);
            let mut cpu = Cpu::new(0);
            assert_eq!(unsafe { run(entry, &mut cpu, &mut bus, &mem, 0, 2) }.0, 2);
            assert_eq!((cpu.pc, cpu.regs[0]), (64, 42));
        }
    }

    #[test]
    fn batching_pending_ram_guard_rejects_changed_code_before_retiring() {
        for untracked in [false, true] {
            let mut bus = Memory::new(); bus.untracked_xip = untracked;
            let mem = bus.jit_mem().unwrap(); let mut j = batching_jit();
            let ops = [op(Fast::MovI { d: 0, v: 42 }), op(Fast::Goto { target: 64 })];
            assert!(queue_hot(&mut j, 0, 0, &ops, &mem).is_none());
            assert!(j.body(0).is_none());
            // The source changes while the native vector is still ordinary data.
            bus.write8(0, 7);
            let entry = hot(&mut j, 0, 0, &ops, &mem);
            let mut cpu = Cpu::new(0);
            assert_eq!(unsafe { run(entry, &mut cpu, &mut bus, &mem, 0, 2) }.0, 0);
            assert_eq!((cpu.pc, cpu.regs[0], bus.ticks), (0, 0, 0));
            let fresh = [op(Fast::MovI { d: 0, v: 99 }), op(Fast::Goto { target: 64 })];
            let entry = hot(&mut j, 1, 0, &fresh, &mem);
            assert_eq!(unsafe { run(entry, &mut cpu, &mut bus, &mem, 0, 2) }.0, 2);
            assert_eq!((cpu.pc, cpu.regs[0]), (64, 99));
        }
    }

    #[test]
    fn native_carry_matches_syntax_flags_aliases_and_bounded_paths() {
        let values=[0,1,2,0x7ffffffe,0x7fffffff,0x80000000,0x80000001,0xfffffffe,u32::MAX];
        let mut bus=Memory::new();let mem=bus.jit_mem().unwrap();let mut j=Jit::new(true).unwrap();let mut id=0;
        for sub in [false,true] {for (d,a,b) in [(0,1,2),(0,0,1),(0,1,0),(0,1,1),(0,0,0)] {
            let raw=0xe0b8u64|((if sub{2}else{0})<<16)|(a<<20)|(b<<24)|(d<<28);
            let insn=crate::decode_any(raw,0x02000000).unwrap();
            let fast=crate::fast_of(&insn,crate::canon_syntax(insn.entry)).unwrap();
            let mut first=op(fast);first.len=4;
            let code=hot(&mut j,id,0x02000000,&[first,op(Fast::Nop)],&mem);id+=1;
            for x in values {for y in values {for carry in [0,crate::F_C] {for cap in 0..=2 {
                let mut cpu=Cpu::new(0x02000000);cpu.regs[0]=0x12345678;cpu.regs[1]=x;cpu.regs[2]=y;cpu.flags=0xfffffffdu32|carry;cpu.profile=true;
                let mut expected=Cpu::new(cpu.pc);expected.regs=cpu.regs;expected.flags=cpu.flags;
                if cap>0{assert!(matches!(expected.exec_syntax(&mut bus,&insn,cpu.pc+4),Ok(Some(None))));}
                bus.ticks=0;assert_eq!(unsafe{run(code,&mut cpu,&mut bus,&mem,0,cap)}.0,cap);
                assert_eq!((cpu.regs,cpu.flags),(expected.regs,expected.flags),"{fast:?} {x:x} {y:x} C={carry} cap={cap}");
                assert_eq!(cpu.pc,0x02000000+if cap==0{0}else{4+(cap-1)*2});
                assert_eq!(bus.ticks,cap.saturating_sub(1) as u64);assert!(cpu.jit_helpers.is_empty());
            }}}}
        }}
        // Carry reads the incoming flags, so a prior producer must publish them
        // even if a later arithmetic instruction overwrites the carry result flags.
        let ops=[op(Fast::AddRI{d:2,s:2,v:1}),op(Fast::Carry{d:0,a:0,b:1,sub:false}),
            op(Fast::SubRR{d:3,a:3,b:1}),op(Fast::Nop)];
        assert!(!flags_overwritten(&ops[1..]));
        let code=hot(&mut j,id,0x02000000,&ops,&mem);
        for cap in 0..=4 {
            let mut cpu=Cpu::new(0x02000000);cpu.regs=[u32::MAX;16];cpu.flags=0;
            let mut expected=Cpu::new(cpu.pc);expected.regs=cpu.regs;
            for o in ops.iter().take(cap as usize){expected.exec_fast(&mut bus,&o.fast,0);}
            bus.ticks=0;assert_eq!(unsafe{run(code,&mut cpu,&mut bus,&mem,0,cap)}.0,cap);
            assert_eq!((cpu.regs,cpu.flags),(expected.regs,expected.flags));
        }
    }

    #[test]
    fn native_division_and_unary_match_edges_aliases_flags_and_exact_budgets() {
        let values=[0,1,2,31,32,0x7fff,0x8000,0xffff,0x10000,0x7fffffff,0x80000000,0x80000001,0xfffffffe,u32::MAX];
        let mut bus=Memory::new(); let mem=bus.jit_mem().unwrap();
        let mut j=Jit::new(true).unwrap(); let mut id=0;
        let mut forms=Vec::new();
        for (d,a,b) in [(0,1,2),(0,0,1),(0,1,0),(0,1,1),(0,0,0)] {
            for k in [0,1,2,3,255] { forms.push(Fast::Div{d,a,b,k}); }
        }
        for (d,s) in [(0,1),(0,0)] { for k in 0..=7 { forms.push(Fast::Un{d,s,k}); } }
        for fast in forms {
            let code=hot(&mut j,id,0x02000000,&[op(fast),op(Fast::Nop)],&mem); id+=1;
            for x in values { for y in values { for cap in 0..=2 {
                let mut cpu=Cpu::new(0x02000000); cpu.flags=15; cpu.profile=true;
                cpu.regs[0]=x; cpu.regs[1]=y; cpu.regs[2]=x;
                let mut expected=Cpu::new(cpu.pc); expected.regs=cpu.regs; expected.flags=cpu.flags;
                if cap>0 { expected.exec_fast(&mut bus,&fast,cpu.pc+2); }
                bus.ticks=0;
                assert_eq!(unsafe { run(code,&mut cpu,&mut bus,&mem,0,cap) }.0,cap);
                assert_eq!((cpu.regs,cpu.flags),(expected.regs,expected.flags),"{fast:?} {x:x} {y:x} cap={cap}");
                assert_eq!(cpu.pc,0x02000000+2*cap);
                assert_eq!(bus.ticks,cap.saturating_sub(1) as u64);
                assert!(cpu.jit_helpers.is_empty(),"operation must run directly: {fast:?}");
            }}}
        }
    }

    #[test]
    fn native_interrupt_gate_preserves_hook_timing_epochs_and_glued_boundary() {
        for effect in 0..=2 { for untracked in [false,true] { for on in [false,true] { for glue in [false,true] {
            let mut bus=Memory::new(); bus.irq_effect=effect; bus.untracked_xip=untracked;
            let mem=bus.jit_mem().unwrap(); let mut j=Jit::new(true).unwrap();
            let mut gate=op(Fast::SetIe{on}); gate.glue=glue;
            let ops=[op(Fast::MovI{d:0,v:42}),gate,op(Fast::MovI{d:1,v:99})];
            let code=hot(&mut j,0,0x02000000,&ops,&mem);
            for cap in 0..=3 {
                bus.ticks=0; bus.due_at=1001; bus.irq_windows=0;
                let mut cpu=Cpu::new(0x02000000); cpu.ie=!on; cpu.flags=15; cpu.profile=true;
                let count=if on && effect!=0 { cap.min(2) } else { cap };
                assert_eq!(unsafe { run(code,&mut cpu,&mut bus,&mem,0,cap) }.0,count);
                assert_eq!(cpu.pc,0x02000000+2*count);
                assert_eq!(cpu.ie,if count>=2 { on } else { !on });
                assert_eq!(cpu.regs[0],if count>0 { 42 } else { 0 });
                assert_eq!(cpu.regs[1],if count>=3 { 99 } else { 0 });
                assert_eq!(cpu.flags,15);
                assert_eq!(bus.irq_windows,u32::from(on && count>=2));
                assert_eq!(bus.due_at,if on && count>=2 && effect==1 { 2 } else { 1001 },
                    "the hook must observe the enabling instruction's clock");
                assert_eq!(bus.ticks,count.saturating_sub(1) as u64);
                assert_eq!(cpu.hold_irq,u32::from(glue && count==2));
                assert!(!cpu.jit_helpers.contains_key("SetIe"));
                assert_eq!(cpu.jit_helpers.get("irq_window").copied().unwrap_or(0),u64::from(on && count>=2));
            }
        }}}}
        let mut bus=Memory::new(); bus.irq_effect=3;
        let mem=bus.jit_mem().unwrap(); let mut j=Jit::new(false).unwrap();
        let code=hot(&mut j,0,0x02000000,&[op(Fast::SetIe{on:true}),op(Fast::MovI{d:0,v:99})],&mem);
        let mut cpu=Cpu::new(0x02000000);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { run(code,&mut cpu,&mut bus,&mem,0,2) })).is_err());
        assert!(cpu.ie); assert_eq!(cpu.regs[0],0);
    }
    #[cfg(target_os = "android")]
    #[test]
    fn android_cache_publishes_rx_pages_without_writable_executable_memory() {
        let mut cache = CodeCache::new(64 << 10).unwrap();
        let first = cache.write(&[0xd65f03c0]).unwrap() as usize; // RET
        let second = cache.write(&[0xd65f03c0]).unwrap() as usize;
        assert_eq!(second - first, cache.page);
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        for address in [first, second] {
            let permissions = maps.lines().find_map(|line| {
                let mut fields = line.split_whitespace();
                let mut range = fields.next()?.split('-');
                let start = usize::from_str_radix(range.next()?, 16).ok()?;
                let end = usize::from_str_radix(range.next()?, 16).ok()?;
                (start <= address && address < end).then(|| fields.next().unwrap())
            }).unwrap();
            assert_eq!(permissions, "r-xp");
        }
        while cache.write(&[0xd65f03c0]).is_some() {}
        assert_eq!(cache.used, cache.size);
    }

    #[test]
    fn cache_reset_releases_auxiliary_allocations_and_reuses_code() {
        let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
        let mut j = Jit::new(false).unwrap();
        for _ in 0..20 {
            let code = hot(&mut j, 0, 0, &[op(Fast::Goto { target: 32 })], &mem);
            let mut cpu = Cpu::new(0);
            assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, 1) }.0, 1);
            assert!(!j.slots.is_empty());
            j.reset();
            assert!(j.state.is_empty() && j.guards.is_empty() && j.slots.is_empty() && j.helpers.is_empty());
            assert_eq!(j.cache.used, 0);
        }
    }

    #[test]
    fn helper_panics_leave_generated_frames_before_unwinding() {
        let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
        let mut j = Jit::new(false).unwrap();
        let code = hot(&mut j, 0, 0, &[op(Fast::Ld { d: 0, b: Base::Reg(1), off: 0, w: 4, signed: false })], &mem);
        let mut cpu = Cpu::new(0); cpu.regs[1] = 1000;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            unsafe { run(code, &mut cpu, &mut bus, &mem, 0, 1); }
        }));
        assert!(result.is_err());
    }

    #[test]
    fn ram_chain_rejects_changed_target_and_respects_budget() {
        let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
        let mut cpu = Cpu::new(0); let mut j = Jit::new(false).unwrap();
        let from = hot(&mut j, 0, 0, &[op(Fast::Goto { target: 32 })], &mem);
        hot(&mut j, 1, 32, &[op(Fast::MovI { d: 0, v: 42 })], &mem);
        let (_, slot) = unsafe { run(from, &mut cpu, &mut bus, &mem, 0, 10) };
        assert!(!slot.is_null());
        unsafe { *slot = j.body(1).unwrap() as u64; }
        assert_eq!(unsafe { run(from, &mut cpu, &mut bus, &mem, 0, 1) }.0, 1);
        assert_eq!(cpu.regs[0], 0);
        assert_eq!(unsafe { run(from, &mut cpu, &mut bus, &mem, 0, 10) }.0, 2);
        assert_eq!(cpu.regs[0], 42);
        bus.write8(32, 1); cpu.regs[0] = 0;
        assert_eq!(unsafe { run(from, &mut cpu, &mut bus, &mem, 0, 10) }.0, 1);
        assert_eq!(cpu.pc, 32); assert_eq!(cpu.regs[0], 0);
    }
    #[test]
    fn code_store_exits_before_following_instruction() {
        let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
        let mut cpu = Cpu::new(0); let mut j = Jit::new(false).unwrap();
        cpu.regs[0] = 123; cpu.regs[1] = 2;
        let code = hot(&mut j, 0, 0, &[
            op(Fast::St { s: 0, b: Base::Reg(1), off: 0, w: 4 }),
            op(Fast::MovI { d: 2, v: 99 })], &mem);
        assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, 10) }.0, 1);
        assert_eq!(cpu.pc, 2); assert_eq!(cpu.regs[2], 0);
        assert_eq!(bus.read32(2), 123); assert_eq!(bus.version, 1);
    }

    #[test]
    fn constant_mmio_rmw_preserves_width_order_flags_and_budgets() {
        let mut cases = vec![Fast::MemA4 { b: 1, off: 0, op: Op::Or, rk: 0, rv: 0 },
            Fast::MemA4 { b: 1, off: 0, op: Op::AndNot, rk: 1, rv: 0x1200 }];
        for w in [1,2,4] { for rmw in [RmwOp::Mov,RmwOp::Or,RmwOp::And,RmwOp::Xor,RmwOp::Add,RmwOp::Sub] {
            cases.push(Fast::Rmw { b: 1, w, op: rmw, off: 0, v: 0x87654321 });
        }}
        for fast in cases {
            let mut bus = Memory::new(); let mut mem = bus.jit_mem().unwrap(); mem.ram_len = 128;
            let mut j = Jit::new(true).unwrap();
            let ops = [op(Fast::MovI { d: 1, v: 192 }), op(Fast::MovI { d: 0, v: 3 }),
                op(Fast::AddRR { d: 0, a: 0, b: 0 }), op(Fast::AddRR { d: 0, a: 0, b: 0 }),
                op(Fast::AddRR { d: 0, a: 0, b: 0 }), op(fast), op(Fast::MovI { d: 2, v: 99 })];
            let code = hot(&mut j,0,0x02000000,&ops,&mem);
            for cap in 1..=7 {
                bus.ticks = 0; bus.data[192..196].copy_from_slice(&0x12345678u32.to_le_bytes());
                let mut reference = Memory::new(); reference.data.clone_from(&bus.data);
                let mut cpu = Cpu::new(0x02000000); cpu.profile = true;
                let mut expected = Cpu::new(cpu.pc); let count = cap.min(6);
                for (i,o) in ops.iter().take(count as usize).enumerate() {
                    let next = 0x02000000 + 2 * (i as u32 + 1);
                    expected.pc = expected.exec_fast(&mut reference,&o.fast,next).unwrap_or(next);
                }
                assert_eq!(unsafe { run(code,&mut cpu,&mut bus,&mem,0,cap) }.0,count,"{fast:?}");
                assert_eq!((cpu.regs,cpu.flags,cpu.pc),(expected.regs,expected.flags,expected.pc));
                assert_eq!(bus.data,reference.data,"{fast:?}");
                assert_eq!(bus.ticks,(count - 1) as u64);
            }
        }
    }

    #[test]
    fn allocated_registers_and_constant_addresses_match_helpers_aliases_and_budgets() {
        let ops = [op(Fast::MovI { d: 1, v: 64 }), op(Fast::MovI { d: 0, v: 5 }),
            op(Fast::AddRR { d: 0, a: 0, b: 0 }), op(Fast::AddRR { d: 0, a: 0, b: 0 }),
            op(Fast::AddRR { d: 0, a: 0, b: 0 }), op(Fast::Div { d: 0, a: 0, b: 2, k: 1 }),
            op(Fast::Alu2 { d: 0, b: 3, op: Op::Mul }),
            op(Fast::LdW { d: 4, b: 1, off: 0 }), op(Fast::LdW { d: 1, b: 4, off: 0 }),
            op(Fast::MovI { d: 5, v: 192 }), op(Fast::LdBu { d: 6, b: 5, off: 0 }),
            op(Fast::AddRR { d: 0, a: 0, b: 6 }), op(Fast::StW { s: 0, b: 1, off: 0 }),
            op(Fast::Goto { target: 224 })];
        let mut bus = Memory::new(); let mut mem = bus.jit_mem().unwrap(); mem.ram_len = 160;
        let mut j = Jit::new(false).unwrap();
        let code = hot(&mut j, 0, 0x02000000, &ops, &mem);
        for cap in 0..=ops.len() as u32 {
            let mut reference = Memory::new(); bus.data.fill(0); bus.flags.fill(0); bus.ticks = 0;
            bus.data[64..68].copy_from_slice(&96u32.to_le_bytes());
            bus.data[96..100].copy_from_slice(&128u32.to_le_bytes()); bus.data[192] = 7;
            reference.data.clone_from(&bus.data);
            let mut cpu = Cpu::new(0x02000000); cpu.regs[2] = 3; cpu.regs[3] = 11;
            let mut expected = Cpu::new(cpu.pc); expected.regs = cpu.regs;
            for (i,o) in ops.iter().take(cap as usize).enumerate() {
                let next = 0x02000000 + 2 * (i as u32 + 1);
                expected.pc = expected.exec_fast(&mut reference,&o.fast,next).unwrap_or(next);
            }
            assert_eq!(unsafe { run(code,&mut cpu,&mut bus,&mem,0,cap) }.0,cap);
            assert_eq!((cpu.regs,cpu.flags,cpu.pc),(expected.regs,expected.flags,expected.pc),"cap={cap}");
            assert_eq!(bus.data,reference.data,"cap={cap}");
        }
    }

    #[test]
    fn nonfinite_glued_mac_keeps_irq_hold_at_tick_boundary() {
        let mut bus = Memory::new(); bus.due_at = 1;
        let mem = bus.jit_mem().unwrap(); let mut j = Jit::new(false).unwrap();
        let mut mac = op(Fast::FOp { d: 0, a: 1, b: 2, op: 7 }); mac.glue = true;
        let code = hot(&mut j, 0, 0, &[mac, op(Fast::Nop)], &mem);
        let mut cpu = Cpu::new(0); cpu.regs[1] = 0x7fc12345; cpu.regs[2] = 1.0f32.to_bits();
        assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, 2) }.0, 1);
        assert_eq!((cpu.pc, cpu.hold_irq), (2, 1));
    }

    #[test]
    fn memory_ranges_smaller_than_access_width_use_bus_helpers() {
        for width in [1, 2, 4] { for len in 0..width {
            let mut bus = Memory::new(); bus.data[..4].copy_from_slice(&0x12345678u32.to_le_bytes());
            let mut mem = bus.jit_mem().unwrap();
            mem.ram = std::ptr::null_mut(); mem.ram_len = len as u32;
            mem.xip_base = 0; mem.xip_len = len as u32;
            let mut j = Jit::new(false).unwrap();
            let code = hot(&mut j, 0, 0x02000000, &[
                op(Fast::Ld { d: 0, b: Base::Reg(1), off: 0, w: width, signed: false }),
                op(Fast::St { s: 0, b: Base::Reg(1), off: 8, w: width }), op(Fast::Nop)], &mem);
            let mut cpu = Cpu::new(0x02000000);
            assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, 3) }.0, 2);
            let expected = 0x12345678 & (u32::MAX >> ((4 - width) * 8));
            assert_eq!(cpu.regs[0], expected);
            assert_eq!(&bus.data[8..8 + width as usize], &expected.to_le_bytes()[..width as usize]);
        }}
    }

    #[test]
    fn chained_ram_if_guard_checks_derived_body_extent_dependencies() {
        let mut bus = Memory::new();
        bus.data[32..36].copy_from_slice(&[0x1d, 0xea, 0x80, 0xc6]);
        for pair in bus.data[36..64].chunks_exact_mut(2) { pair.copy_from_slice(&[0xc0, 0x21]); }
        let mem = bus.jit_mem().unwrap();
        let mut j = Jit::new(false).unwrap();
        let source = hot(&mut j, 0, 0, &[op(Fast::Goto { target: 32 })], &mem);
        let mut head = op(Fast::IfBlk { a: 13, rk: 0, fam: 2, alt: true,
            has_else: false, rv: 6, then_d: 8, else_d: 0 });
        head.len = 4;
        let target = hot(&mut j, 1, 32, &[head], &mem);
        let mut cpu = Cpu::new(0);
        let (_, slot) = unsafe { run(source, &mut cpu, &mut bus, &mem, 0, 1) };
        unsafe { *slot = j.body(1).unwrap() as u64; }
        cpu.pc = 0;
        assert_eq!(unsafe { run(source, &mut cpu, &mut bus, &mem, 0, 2) }.0, 2);
        assert_eq!(cpu.pc, 44);
        // The unchanged head now skips a longer final body instruction. This
        // write lies beyond the former prefix-length+6 native guard snapshot.
        let header = bus.data[32..40].to_vec();
        bus.write32(42, 0x8000_e070);
        assert_eq!(bus.data[32..40], header);
        cpu.pc = 32;
        assert_eq!(unsafe { run(target, &mut cpu, &mut bus, &mem, 0, 1) }.0, 0);
        assert_eq!(cpu.pc, 32);
        cpu.pc = 0;
        assert_eq!(unsafe { run(source, &mut cpu, &mut bus, &mem, 0, 2) }.0, 1);
        assert_eq!(cpu.pc, 32, "old incoming chains must reject stale if extents");
        let mut rebuilt = Cpu::new(32); rebuilt.no_ram_blocks = false;
        assert_eq!(rebuilt.run_fast(&mut bus, 1).unwrap().0, 1);
        assert_eq!(rebuilt.pc, 46);
    }

    #[test]
    fn indirect_cache_checks_budget_stop_pc_mutation_collision_and_reset() {
        let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
        let mut j = Jit::new(false).unwrap(); j.observe_pc();
        let source = hot(&mut j, 0, 0, &[op(Fast::Rts)], &mem);
        hot(&mut j, 1, 32, &[op(Fast::MovI { d: 0, v: 42 }), op(Fast::Nop)], &mem);
        for cap in 1..=3 {
            let mut cpu = Cpu::new(0); cpu.rets = 32;
            assert_eq!(unsafe { run(source, &mut cpu, &mut bus, &mem, 0, cap) }.0, cap);
            assert_eq!(cpu.pc, 32 + 2 * (cap - 1));
            assert_eq!(cpu.regs[0], if cap == 1 { 0 } else { 42 });
        }
        for stop in [32, 34] {
            let mut cpu = Cpu::new(0); cpu.rets = 32; cpu.run_stop_pc = Some(stop);
            assert_eq!(unsafe { run(source, &mut cpu, &mut bus, &mem, 0, 30) }.0, (stop - 32) / 2 + 1);
            assert_eq!(cpu.pc, stop);
        }
        bus.write8(32, 1);
        let mut cpu = Cpu::new(0); cpu.rets = 32;
        assert_eq!(unsafe { run(source, &mut cpu, &mut bus, &mem, 0, 3) }.0, 1);
        assert_eq!((cpu.pc, cpu.regs[0]), (32, 0), "cached RAM body must reject changed bytes");
        // These XIP entries share the index but not the exact PC tag.
        for (id, pc, value) in [(2, 0x02000020, 7), (3, 0x02002020, 9)] {
            hot(&mut j, id, pc, &[op(Fast::MovI { d: 0, v: value }), op(Fast::Nop)], &mem);
        }
        let mut cpu = Cpu::new(0); cpu.rets = 0x02000020;
        assert_eq!(unsafe { run(source, &mut cpu, &mut bus, &mem, 0, 3) }.0, 1);
        assert_eq!(cpu.regs[0], 0, "collision must return to Rust");
        cpu.rets = 0x02002020;
        assert_eq!(unsafe { run(source, &mut cpu, &mut bus, &mem, 0, 3) }.0, 3);
        assert_eq!(cpu.regs[0], 9);
        j.reset();
        assert!(j.targets.iter().all(|t| t.pc == u32::MAX && t.body == 0));
    }

    #[test]
    fn tracked_ram_guard_skips_helper_until_full_width_epoch_changes() {
        let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
        let mut j = Jit::new(false).unwrap();
        let code = hot(&mut j, 0, 0, &[op(Fast::MovI { d: 0, v: 42 })], &mem);
        let mut cpu = Cpu::new(0);
        assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, 1) }.0, 1);
        let checked = bus.code_views;
        for _ in 0..10 {
            assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, 1) }.0, 1);
        }
        assert_eq!(bus.code_views, checked, "stable tracked code needs no guard helper");
        bus.write8(128, 1);
        assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, 1) }.0, 1);
        assert_eq!(bus.code_views, checked + 1, "a changed epoch rechecks the bytes");
        assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, 1) }.0, 1);
        assert_eq!(bus.code_views, checked + 1);
        // The low 32 bits match the validated epoch. A 32-bit compare would execute
        // stale code here instead of checking the changed immutable snapshot.
        bus.version += 1 << 32; bus.data[0] = 1; cpu.regs[0] = 0;
        assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, 1) }.0, 0);
        assert_eq!(cpu.regs[0], 0);
        bus.data[0] = 0; bus.untracked_xip = true;
        let checked = bus.code_views;
        for _ in 0..3 { assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, 1) }.0, 1); }
        assert_eq!(bus.code_views, checked + 3, "untracked code always validates bytes");
    }

    #[test]
    fn stop_pc_observes_interior_boundaries_and_chained_self_loops() {
        let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
        let mut j = Jit::new(false).unwrap();j.observe_pc();
        let mut head = op(Fast::MovI { d: 0, v: 1 }); head.glue = true;
        let code = hot(&mut j, 0, 0, &[head,
            op(Fast::MovI { d: 1, v: 2 }), op(Fast::Goto { target: 0 })], &mem);
        // Populate the self-chain before enabling observation, so returning to PC 0
        // must be caught in generated code rather than by an unlinked Rust exit.
        let mut cpu = Cpu::new(0);
        let (_, slot) = unsafe { run(code, &mut cpu, &mut bus, &mem, 0, 3) };
        assert!(!slot.is_null());unsafe { *slot = j.body(0).unwrap() as u64; }
        for (stop, count, hold, r1) in [(0, 3, 0, 2), (2, 1, 1, 0), (4, 2, 0, 2)] {
            for cap in [3, 31] {
                let mut cpu = Cpu::new(0); cpu.run_stop_pc = Some(stop);
                assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, cap) }.0, count);
                assert_eq!((cpu.pc, cpu.hold_irq, cpu.regs[1]), (stop, hold, r1));
            }
        }
    }

    #[test]
    fn bounded_prefix_matches_reference_at_every_budget_and_tick_boundary() {
        let ops = [op(Fast::AddRI { d: 0, s: 0, v: 3 }),
            op(Fast::Alu3 { d: 1, a: 0, b: 1, op: Op::Mul }),
            op(Fast::FOp { d: 2, a: 2, b: 3, op: 7 }),
            op(Fast::AddRI { d: 4, s: 1, v: u32::MAX }),
            op(Fast::Goto { target: 64 })];
        let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
        let mut j = Jit::new(false).unwrap();
        let code = hot(&mut j, 0, 0, &ops, &mem);
        for cap in 0..=7 { for room in 0..=7 { for extra in 0..=1 {
            bus.ticks = 0; bus.due_at = room + 1;
            let mut cpu = Cpu::new(0); let mut expected = Cpu::new(0);
            cpu.regs = [2; 16]; cpu.regs[2] = 1.25f32.to_bits(); cpu.regs[3] = 0.5f32.to_bits();
            expected.regs = cpu.regs;
            let count = cap.min((room + 1).saturating_sub(extra as u64) as u32).min(ops.len() as u32);
            let mut reference = Memory::new();
            for (i, o) in ops.iter().take(count as usize).enumerate() {
                let next = (i as u32 + 1) * 2;
                expected.pc = expected.exec_fast(&mut reference, &o.fast, next).unwrap_or(next);
            }
            assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, extra, cap) }.0, count,
                "cap={cap} room={room} extra={extra}");
            assert_eq!((cpu.regs, cpu.flags, cpu.pc), (expected.regs, expected.flags, expected.pc));
            assert_eq!(bus.ticks, if count == 0 { 0 } else { (count - 1 + extra) as u64 });
        }}}
    }

    #[test]
    fn bounded_chain_preserves_glued_pair_barrier_and_checks_ram_guard() {
        let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
        let mut j = Jit::new(false).unwrap();
        let from = hot(&mut j, 0, 0, &[op(Fast::Goto { target: 32 })], &mem);
        let mut head = op(Fast::AddRI { d: 0, s: 0, v: 1 }); head.glue = true;
        let target = hot(&mut j, 1, 32, &[head,
            op(Fast::AddRI { d: 1, s: 1, v: 1 }), op(Fast::Goto { target: 64 })], &mem);
        let mut cpu = Cpu::new(0);
        let (_, slot) = unsafe { run(from, &mut cpu, &mut bus, &mem, 0, 1) };
        unsafe { *slot = j.body(1).unwrap() as u64; }
        for cap in 1..=4 {
            let mut cpu = Cpu::new(0);
            assert_eq!(unsafe { run(from, &mut cpu, &mut bus, &mem, 0, cap) }.0, cap);
            assert_eq!(cpu.hold_irq, u32::from(cap == 2));
            assert_eq!(cpu.regs[0], u32::from(cap >= 2));
            assert_eq!(cpu.regs[1], u32::from(cap >= 3));
            assert_eq!(cpu.pc, [0, 32, 34, 36, 64][cap as usize]);
        }
        bus.write8(32, 1);
        let mut cpu = Cpu::new(32);
        assert_eq!(unsafe { run(target, &mut cpu, &mut bus, &mem, 0, 1) }.0, 0);
        assert_eq!((cpu.pc, cpu.regs[0]), (32, 0));
    }

    #[test]
    fn bounded_prefix_stops_after_code_write_and_peripheral_barrier() {
        for stop_on_write in [false, true] {
            let mut bus = Memory::new(); bus.stop_on_write = stop_on_write;
            let mem = bus.jit_mem().unwrap(); let mut j = Jit::new(false).unwrap();
            let code = hot(&mut j, 0, 0, &[
                op(Fast::AddRI { d: 0, s: 0, v: 1 }),
                op(Fast::St { s: 0, b: Base::Reg(1), off: 0, w: 4 }),
                op(Fast::MovI { d: 2, v: 99 }), op(Fast::Nop)], &mem);
            let mut cpu = Cpu::new(0); cpu.regs[1] = 8;
            assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 1, 3) }.0, 2);
            assert_eq!((cpu.pc, cpu.regs[2], bus.ticks), (4, 0, 2));
            assert_eq!(bus.read32(8), 1);
        }
    }

    #[test]
    fn xip_epoch_exits_helpers_even_without_ram_write_tracking() {
        for fast in [Fast::St { s: 0, b: Base::Reg(1), off: 0, w: 4 },
            Fast::BlockMove { base: 1, mask: 1, store: true, post: false }] {
            let mut bus = Memory::new(); bus.untracked_xip = true;
            let mem = bus.jit_mem().unwrap(); let mut j = Jit::new(false).unwrap();
            let code = hot(&mut j, 0, 0x02000000, &[op(fast),
                op(Fast::MovI { d: 2, v: 99 }), op(Fast::Nop)], &mem);
            for cap in [2, 3] {
                let mut cpu = Cpu::new(0x02000000); cpu.regs[1] = 128;
                assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, cap) }.0, 1);
                assert_eq!((cpu.pc, cpu.regs[2]), (0x02000002, 0));
            }
        }
    }

    #[test]
    fn floating_mac_matches_reference_bits_aliases_and_separate_rounding() {
        let values = [0, 0x80000000, 0x3f800001, 0x3f7ffffe, 0xbf800000,
            0x7f7fffff, 1, 0x80000001, 0x7f800000, 0xff800000,
            0x7fc12345, 0x7fa12345, 0xffc54321];
        let mut j = Jit::new(false).unwrap();
        let mut id = 0;
        for mac in [7, 8] { for (d, a, b) in [(0, 1, 2), (0, 0, 1), (0, 1, 0), (0, 1, 1), (0, 0, 0)] {
            let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
            let fast = Fast::FOp { d, a, b, op: mac };
            let code = hot(&mut j, id, 0, &[op(fast), op(Fast::Nop)], &mem); id += 1;
            for x in values { for y in values { for acc in values {
                for cap in [1, 2] {
                    let mut cpu = Cpu::new(0); let mut expected = Cpu::new(0);
                    cpu.regs[0] = acc; cpu.regs[1] = x; cpu.regs[2] = y;
                    expected.regs = cpu.regs;
                    expected.exec_fast(&mut bus, &fast, 2);
                    bus.ticks = 0;
                    assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, cap) }.0, cap);
                    assert_eq!(cpu.regs, expected.regs, "{fast:?} a={x:08x} b={y:08x} acc={acc:08x} cap={cap}");
                }
            }}}
        }}

        // This product rounds to 1 before accumulation, whereas FMA produces -2^-46.
        assert_ne!(f32::from_bits(0x3f800001).mul_add(f32::from_bits(0x3f7ffffe), -1.0).to_bits(), 0);
    }
    #[test]
    fn dead_flags_stop_at_helpers_memory_control_flow_and_partial_observation() {
        let writer = op(Fast::AddRI { d: 0, s: 0, v: 1 });
        let pure = op(Fast::MovI { d: 1, v: 42 });
        assert!(flags_overwritten(&[pure, writer]));
        for barrier in [Fast::LdW { d: 1, b: 2, off: 0 },
            Fast::StW { s: 1, b: 2, off: 0 }, Fast::Goto { target: 32 },
            Fast::Div { d: 0, a: 0, b: 1, k: 1 },
            Fast::FOp { d: 0, a: 1, b: 2, op: 7 },
            Fast::Raw { route: 16, raw: 0 }] {
            assert!(!flags_overwritten(&[op(barrier), writer]), "{barrier:?}");
        }
        let ops = [writer, pure, op(Fast::SubRR { d: 1, a: 1, b: 0 }), op(Fast::Nop)];
        let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
        let mut j = Jit::new(false).unwrap(); j.observe_pc();
        let code = hot(&mut j, 0, 0, &ops, &mem);
        for stop in [None, Some(2), Some(4), Some(6)] { for cap in 1..=4 {
            bus.ticks = 0;
            let mut cpu = Cpu::new(0); cpu.run_stop_pc = stop;
            cpu.regs[0] = 0x7fffffff; cpu.flags = 15;
            let mut expected = Cpu::new(0); expected.regs = cpu.regs; expected.flags = cpu.flags;
            let count = cap.min(stop.unwrap_or(8) / 2);
            for (i, o) in ops.iter().take(count as usize).enumerate() {
                let next = 2 * (i as u32 + 1);
                expected.pc = expected.exec_fast(&mut bus, &o.fast, next).unwrap_or(next);
            }
            bus.ticks = 0;
            assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, cap) }.0, count);
            assert_eq!((cpu.regs, cpu.flags, cpu.pc), (expected.regs, expected.flags, expected.pc));
        }}
    }

    #[test]
    fn native_saturating_scalar_preserves_flags_aliases_and_every_budget() {
        let mut bus=Memory::new();let mem=bus.jit_mem().unwrap();let mut j=Jit::new(false).unwrap();let mut id=0;
        for (d,a,b) in [(0,0,1),(1,0,1),(2,0,1),(5,5,7),(15,14,15),(7,7,7)] { for sub in [false,true] {
            let fast=Fast::SatRR{d,a,b,sub};let mut arithmetic=op(fast);arithmetic.len=4;
            let mut ops=vec![];
            for _ in 0..3 {for r in 0..9 {ops.push(op(Fast::Mov{d:r,s:r}));}}
            ops.extend([arithmetic,op(Fast::Nop)]);
            let code=hot(&mut j,id,0x02000000,&ops,&mem);id+=1;
            for x in [i32::MIN,-1,0,1,i32::MAX] {for y in [i32::MIN,-1,0,1,i32::MAX] {for cap in 0..=ops.len() as u32 {
                let mut cpu=Cpu::new(0x02000000);cpu.flags=0xfedcba98;cpu.regs=[0xa5a5a5a5;16];
                cpu.regs[a as usize]=x as u32;cpu.regs[b as usize]=y as u32;
                let mut expected=Cpu::new(cpu.pc);expected.restore_architectural_state(cpu.architectural_state());
                let mut next=cpu.pc;
                for op in ops.iter().take(cap as usize) {next+=op.len as u32;expected.pc=expected.exec_fast(&mut bus,&op.fast,next).unwrap_or(next);}
                bus.ticks=0;assert_eq!(unsafe{run(code,&mut cpu,&mut bus,&mem,0,cap)}.0,cap);
                assert!(cpu.architectural_state()==expected.architectural_state(),"{fast:?} x={x} y={y} cap={cap}");
                assert_eq!(cpu.flags,0xfedcba98);assert_eq!(bus.ticks,cap.saturating_sub(1) as u64);
            }}}
        }}
    }

    #[test]
    fn native_pair_extract_matches_signed_unsigned_aliases_and_exact_caps() {
        let mut bus=Memory::new();let mem=bus.jit_mem().unwrap();let mut j=Jit::new(false).unwrap();let mut id=0;
        for lo in [0,6,14] {for d in [lo,lo+1,3,15] {for n in [0,1,29,31,32,47,63] {for signed in [false,true] {
            let fast=Fast::PairExtract{d,lo,n,signed};let mut extract=op(fast);extract.len=4;
            let ops=[extract,op(Fast::Nop)];
            let code=hot(&mut j,id,0x02000000,&ops,&mem);id+=1;
            for value in [0,1,0x8000000000000000,0x0123456789abcdef,u64::MAX] {for cap in 0..=2 {
                let mut cpu=Cpu::new(0x02000000);cpu.flags=0xfedcba98;cpu.regs[lo as usize]=value as u32;cpu.regs[lo as usize+1]=(value>>32)as u32;
                let mut expected=Cpu::new(cpu.pc);expected.restore_architectural_state(cpu.architectural_state());
                let mut next=0x02000000;
                for o in ops.iter().take(cap as usize){next+=o.len as u32;expected.pc=expected.exec_fast(&mut bus,&o.fast,next).unwrap_or(next);}
                bus.ticks=0;assert_eq!(unsafe{run(code,&mut cpu,&mut bus,&mem,0,cap)}.0,cap);
                assert!(cpu.architectural_state()==expected.architectural_state(),"{fast:?} value={value:x} cap={cap}");
                assert_eq!(cpu.flags,0xfedcba98);assert_eq!(bus.ticks,cap.saturating_sub(1)as u64);
            }}
        }}}}
    }

    #[test]
    fn fpr_cache_matches_hot_conversion_chain_helpers_and_every_budget() {
        const PC:u32=0x02000000;
        let ops=[op(Fast::LdW{d:2,b:8,off:0}),op(Fast::LdW{d:4,b:0,off:0}),
            op(Fast::SarI{d:2,s:2,n:4}),op(Fast::FOp{d:2,a:2,b:0,op:0x8f}),
            op(Fast::FOp{d:1,a:2,b:1,op:2}),op(Fast::FOp{d:2,a:4,b:0,op:0x8f}),
            op(Fast::FOp{d:1,a:2,b:1,op:0}),op(Fast::FOp{d:1,a:1,b:0,op:0x1f}),
            op(Fast::StW{s:1,b:0,off:4}),op(Fast::Nop)];
        assert!(fpr::allocate(&ops).iter().any(|&(guest,_)|guest==1));
        assert!(!fpr::allocate(&ops).iter().any(|&(guest,_)|guest==2),"mixed integer register stays in the integer bank");
        let mut bus=Memory::new();let mut mem=bus.jit_mem().unwrap();mem.ram_len=128;
        let mut j=Jit::new(false).unwrap();j.observe_pc();let code=hot(&mut j,0,PC,&ops,&mem);
        for address in [64,160] {for input in [0u32,1,0x7fffffff,0x80000000,u32::MAX] {
            for initial in [0,0x80000000,0x3e800000,0x7f800000,0xff800000,0x7fc01234,0x7f801234] {
                for cap in 0..=ops.len()as u32 {for stop in [None,Some(PC+10)] {
                    bus.ticks=0;bus.version=0;bus.flags.fill(0);
                    bus.data[32..36].copy_from_slice(&input.to_le_bytes());bus.data[address as usize..address as usize+4].copy_from_slice(&input.rotate_left(13).to_le_bytes());
                    let mut reference=Memory::new();reference.data.clone_from(&bus.data);
                    let mut cpu=Cpu::new(PC);cpu.regs[0]=address;cpu.regs[8]=32;cpu.regs[1]=initial;cpu.flags=13;cpu.run_stop_pc=stop;
                    let mut expected=Cpu::new(PC);expected.restore_architectural_state(cpu.architectural_state());
                    let count=unsafe{run(code,&mut cpu,&mut bus,&mem,0,cap)}.0;
                    for(i,o)in ops.iter().take(count as usize).enumerate(){let next=PC+(i as u32+1)*2;expected.pc=expected.exec_fast(&mut reference,&o.fast,next).unwrap_or(next);}
                    assert!(cpu.architectural_state()==expected.architectural_state(),"address={address} input={input:x} initial={initial:x} cap={cap} stop={stop:?}");
                    assert_eq!(bus.data,reference.data);assert_eq!(bus.ticks,count.saturating_sub(1)as u64);
                }}
            }
        }}
    }

    #[test]
    fn fpr_cache_preserves_aliases_nonfinite_mac_fallback_and_stack_bits() {
        const PC:u32=0x02000000;
        for kind in [0,1,2,3,5,6,7,8,0x1f,0x5f,0x8f,0x9f] {for (d,a,b) in [(0,1,2),(1,1,2),(2,1,2),(0,1,1),(1,1,1)] {
            let mut ops=Vec::new();
            for _ in 0..3{for r in 0..3{ops.push(op(Fast::FOp{d:r,a:r,b:15,op:0}));}}
            ops.extend([op(Fast::FOp{d,a,b,op:kind}),op(Fast::StackMask{mask:7,pop:false}),
                op(Fast::MovI{d:0,v:0x7f801234}),op(Fast::StackMask{mask:7,pop:true}),op(Fast::Nop)]);
            assert!(fpr::allocate(&ops).len()>=3);
            let mut bus=Memory::new();let mut mem=bus.jit_mem().unwrap();mem.ram_len=128;
            let mut j=Jit::new(false).unwrap();let code=hot(&mut j,0,PC,&ops,&mem);
            for bits in [0,0x80000000,1,0x3f800001,0xbf800000,0x7f800000,0xff800000,0x7fc01234,0x7f801234] {
                for sp in [96,192] {for cap in [9,10,11,12,13,14] {
                    bus.flags.fill(0);bus.version=0;bus.ticks=0;bus.data.fill(0);
                    let mut reference=Memory::new();reference.flags.fill(0);
                    let mut cpu=Cpu::new(PC);cpu.regs[0]=0.5f32.to_bits();cpu.regs[1]=bits;cpu.regs[2]=1f32.to_bits();cpu.regs[15]=0;cpu.sp=sp;cpu.flags=15;
                    let mut expected=Cpu::new(PC);expected.restore_architectural_state(cpu.architectural_state());
                    let count=unsafe{run(code,&mut cpu,&mut bus,&mem,0,cap)}.0;
                    for(i,o)in ops.iter().take(count as usize).enumerate(){let next=PC+(i as u32+1)*2;expected.pc=expected.exec_fast(&mut reference,&o.fast,next).unwrap_or(next);}
                    assert!(cpu.architectural_state()==expected.architectural_state(),"op={kind} d={d} a={a} b={b} bits={bits:x} sp={sp} cap={cap}");
                    assert_eq!(bus.data,reference.data);
                }}
            }
        }}
    }

    #[test]
    fn wide_register_cache_preserves_raw_helpers_and_every_budget() {
        const PC:u32=0x02000000;
        // Nine simultaneously reused guest registers force allocation beyond
        // w14..w17. The pair lowering reads/writes cached operands; a slow write
        // prepares w4 before flush_regs and exercises the excluded argument slot.
        let mut ops=Vec::new();
        for _ in 0..3 {for r in 0..9 {
            ops.push(op(Fast::AddRR{d:r,a:r,b:(r+1)%9}));
        }}
        ops.extend([op(Fast::Raw{route:21,raw:0x6005e1d0}),
            op(Fast::LdW{d:9,b:10,off:0}),op(Fast::StW{s:0,b:10,off:4}),
            op(Fast::Goto{target:PC+128})]);
        let mut bus=Memory::new();let mut mem=bus.jit_mem().unwrap();mem.ram_len=128;
        let mut j=Jit::new(false).unwrap();let code=hot(&mut j,0,PC,&ops,&mem);
        for address in [64,160] {for seed in [0,1,0x7fffffff,u32::MAX] {
            for extra in [0,1] {for cap in 0..=ops.len()as u32 {
                bus.flags.fill(0);bus.ticks=0;bus.version=0;
                for i in 0..256{bus.data[i]=(i as u8).wrapping_mul(13);}
                let mut reference=Memory::new();reference.data.clone_from(&bus.data);reference.flags.fill(0);
                let mut cpu=Cpu::new(PC);for r in 0..16{cpu.regs[r]=seed.wrapping_add(r as u32);}
                cpu.regs[10]=address;cpu.flags=15;
                let mut expected=Cpu::new(PC);expected.restore_architectural_state(cpu.architectural_state());
                let count=unsafe{run(code,&mut cpu,&mut bus,&mem,extra,cap)}.0;
                assert_eq!(count,if address==160{cap.min(30)}else{cap});
                for (i,o) in ops.iter().take(count as usize).enumerate(){
                    let next=PC+(i as u32+1)*2;
                    expected.pc=expected.exec_fast(&mut reference,&o.fast,next).unwrap_or(next);
                }
                assert!(cpu.architectural_state()==expected.architectural_state(),
                    "address={address} seed={seed:x} extra={extra} cap={cap}");
                assert_eq!(bus.data,reference.data);
                assert_eq!(bus.ticks,if count==0{0}else{(count-1+extra)as u64});
            }}
        }}
    }

    #[test]
    fn specialized_integer_ops_match_registers_flags_and_aliases() {
        let values = [0, 1, 31, 32, 255, 0x7fffffff, 0x80000000, u32::MAX];
        let mut bus = Memory::new(); let mem = bus.jit_mem().unwrap();
        let mut j = Jit::new(false).unwrap(); let mut id = 0;
        for d in [0, 1, 2] {
            let mut ops = vec![Fast::AddRR { d, a: 0, b: 1 }, Fast::SubRR { d, a: 0, b: 1 },
                Fast::AndBit { d, s: 0, r: 1 }, Fast::ShlR { d, a: 0, b: 1 },
                Fast::ShrR { d, a: 0, b: 1 }, Fast::SpFromR { s: d }];
            for arithmetic in [Op::Add, Op::Sub, Op::RSub] {
                ops.push(Fast::Alu3 { d, a: 0, b: 1, op: arithmetic });
                ops.push(Fast::Alu2 { d, b: 1, op: arithmetic });
                for (rk, rv) in [(0, 1), (1, 0x80000000), (2, 1)] {
                    ops.push(Fast::A4 { d, s: 0, op: arithmetic, rk, rv });
                }
                for v in values {
                    ops.push(Fast::Alu3I { d, a: 0, v, op: arithmetic });
                    ops.push(Fast::Alu2I { d, v, op: arithmetic });
                }
            }
            for v in values {
                ops.push(Fast::AddRI { d, s: 0, v });
                ops.push(Fast::AndI { d, s: 0, v }); ops.push(Fast::MulI { d, s: 0, v });
                ops.push(Fast::ShrI { d, s: 0, n: v as u8 });
            }
            for fast in ops {
                let code = hot(&mut j, id, 0, &[op(fast), op(Fast::Nop)], &mem); id += 1;
                for x in values { for y in values { for cap in [1, 2] {
                    let mut cpu = Cpu::new(0); let mut expected = Cpu::new(0);
                    cpu.regs[0] = x; cpu.regs[1] = y; cpu.regs[2] = 0xa5a5a5a5;
                    cpu.flags = 15; expected.regs = cpu.regs; expected.flags = cpu.flags;
                    expected.exec_fast(&mut bus, &fast, 2);
                    bus.ticks = 0;
                    assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, cap) }.0, cap);
                    assert_eq!((cpu.regs, cpu.flags, cpu.sp), (expected.regs, expected.flags, expected.sp), "{fast:?} x={x:x} y={y:x}");
                }}}
            }
        }
    }

    #[test]
    fn mixed_stack_and_condition_ops_preserve_dirty_register_cache_at_every_cap() {
        let mut ops=vec![op(Fast::MovI{d:0,v:104})];
        ops.extend([op(Fast::AddRI{d:0,s:0,v:4});6]);
        ops.extend([op(Fast::TestSet{r:0}),op(Fast::StackMask{mask:1,pop:false}),
            op(Fast::MovI{d:0,v:0}),op(Fast::StackMask{mask:1,pop:true}),op(Fast::CallReg{r:0})]);
        for fallback in [false,true] {
            let mut bus=Memory::new(); bus.flags.fill(0);
            let mut mem=bus.jit_mem().unwrap(); if fallback { mem.ram_len=0; }
            let mut j=Jit::new(false).unwrap(); let code=hot(&mut j,0,0x02000000,&ops,&mem);
            for cap in 0..=ops.len() as u32 {
                bus.data.fill(0); bus.version=0; bus.ticks=0;
                let mut reference=Memory::new();
                let mut cpu=Cpu::new(0x02000000); cpu.sp=192; cpu.flags=13;
                let mut expected=Cpu::new(cpu.pc); expected.restore_architectural_state(cpu.architectural_state());
                let count=if fallback {cap.min(8)} else {cap};
                for (i,o) in ops.iter().take(count as usize).enumerate() {
                    let next=0x02000000+(i as u32+1)*2;
                    expected.pc=expected.exec_fast(&mut reference,&o.fast,next).unwrap_or(next);
                }
                assert_eq!(unsafe{run(code,&mut cpu,&mut bus,&mem,0,cap)}.0,count);
                assert!(cpu.architectural_state()==expected.architectural_state(),"cap={cap} fallback={fallback}");
                assert_eq!(bus.data,reference.data);
            }
        }
    }

    #[test]
    fn condition_byte_and_indirect_call_match_oracle_and_memory_barriers() {
        for r in [0,7,15] { for old in [0u8,1,0x7f,0x80,0xff] {
            for ram_len in [0,128,256] { for watched in [false,true] { for cap in [0,1,2] {
                let mut bus=Memory::new(); let mut reference=Memory::new();
                bus.data[128]=old; reference.data[128]=old;
                bus.flags.fill(u8::from(watched));
                let mut mem=bus.jit_mem().unwrap(); mem.ram_len=ram_len;
                let mut j=Jit::new(false).unwrap();
                let ops=[op(Fast::TestSet {r}),op(Fast::CcBr {want:true,target:0x02000100})];
                let code=hot(&mut j,0,0x02000000,&ops,&mem);
                let mut cpu=Cpu::new(0x02000000); cpu.regs[r as usize]=128; cpu.flags=15;
                let mut expected=Cpu::new(cpu.pc); expected.restore_architectural_state(cpu.architectural_state());
                let count=if cap==2 && (watched || ram_len<=128) {1} else {cap};
                for (i,o) in ops.iter().take(count as usize).enumerate() {
                    let next=0x02000000+(i as u32+1)*2;
                    expected.pc=expected.exec_fast(&mut reference,&o.fast,next).unwrap_or(next);
                }
                assert_eq!(unsafe{run(code,&mut cpu,&mut bus,&mem,0,cap)}.0,count);
                assert!(cpu.architectural_state()==expected.architectural_state(),"r={r} old={old} ram={ram_len} watched={watched} cap={cap}");
                assert_eq!(bus.data,reference.data);
                assert_eq!(bus.version,u64::from(count>0 && (watched || ram_len<=128)));
            }}}}
        }
        for r in [0,7,15] { for target in [0,0x02000000,0x02000100,u32::MAX] {
            let mut bus=Memory::new(); let mem=bus.jit_mem().unwrap(); let mut j=Jit::new(false).unwrap();
            let code=hot(&mut j,0,0x02000000,&[op(Fast::CallReg{r})],&mem);
            let mut cpu=Cpu::new(0x02000000); cpu.regs[r as usize]=target;
            assert_eq!(unsafe{run(code,&mut cpu,&mut bus,&mem,0,1)}.0,1);
            assert_eq!((cpu.pc,cpu.rets),(target,0x02000002));
        }}
    }

    #[test]
    fn special_stack_native_matches_typed_edges_flags_cache_and_irq_windows() {
        let cases = ["[--sp] = {psr, reti}", "{psr, reti} = [sp++]",
            "[--sp] = {psr, rets, reti}", "{psr, rets, reti} = [sp++]",
            "[--sp] = {sp, ssp, usp, icfg, psr, rets, retx, rete, reti}",
            "{ssp, usp, icfg, psr, rets, retx, rete, reti} = [sp++]",
            "{sp, psr, rets} = [sp++]", "[--sp] = {psr}", "{psr} = [sp++]",
            "[--sp] = {ssp, usp, psr, sr4, retx, rete}"];
        for syntax in cases {
            let fast @ Fast::SpecialStack { regs, len, pop } = crate::special_stack(syntax).unwrap() else { unreachable!() };
            let mut ops = vec![op(Fast::AddRI { d: 0, s: 0, v: 1 }); 6];
            ops.extend([op(fast), op(Fast::MovI { d: 7, v: 99 }), op(Fast::Nop)]);
            let mut compile_bus = Memory::new();
            let mut mem = compile_bus.jit_mem().unwrap(); mem.ram_len = 128;
            let mut j = Jit::new(true).unwrap();
            let entry = hot(&mut j, 0, 0x0200_0000, &ops, &mem);
            for sp in [64, 65, 120, 192] { for watched in [false, true] { for pending in [false, true] {
                let mut bus = Memory::new(); let mut reference_bus = Memory::new();
                for memory in [&mut bus, &mut reference_bus] {
                    memory.flags.fill(u8::from(watched)); memory.irq_effect = u8::from(pending);
                    for (i, word) in memory.data.chunks_exact_mut(4).enumerate() {
                        word.copy_from_slice(&(64u32 + i as u32 % 16 * 4).to_le_bytes());
                    }
                }
                let mut mem = bus.jit_mem().unwrap(); mem.ram_len = 128;
                // Generated code uses the live data/flag arguments, not the compile bus.
                for cap in 0..=ops.len() as u32 {
                    let initial_bytes = reference_bus.data.clone();
                    bus.data.copy_from_slice(&initial_bytes); bus.ticks = 0; bus.version = 0; bus.due_at = 1001; bus.irq_windows = 0;
                    reference_bus.ticks = 0; reference_bus.version = 0; reference_bus.due_at = 1001; reference_bus.irq_windows = 0;
                    let mut cpu = Cpu::new(0x0200_0000); cpu.sp = sp; cpu.ssp = 100; cpu.usp = 104;
                    cpu.rets = 0xabcd_4321; cpu.flags = 15; cpu.sreg_store = [0x100, 2, 3, 4, 5, 0x1234_ffff, 7];
                    cpu.ie = true; cpu.irq_levels = vec![1]; cpu.nest_ok = false; cpu.profile = true;
                    let mut reference = Cpu::new(cpu.pc); reference.restore_architectural_state(cpu.architectural_state());
                    let bytes = len as u32 * 4;
                    let low = if pop { sp } else { sp.wrapping_sub(bytes) };
                    let inline = sp & 3 == 0 && low <= 128 - bytes
                        && (pop || !watched) && !regs[..len as usize].contains(&SpecialReg::Sp)
                        && (pop || !regs[..len as usize].contains(&SpecialReg::Icfg));
                    let gate = (!pop && regs[..len as usize].contains(&SpecialReg::Reti))
                        || (pop && regs[..len as usize].contains(&SpecialReg::Icfg));
                    let mut count = 0;
                    for operation in ops.iter().take(cap as usize) {
                        if count != 0 { reference_bus.ticks += 1; }
                        reference.pc = reference.exec_fast(&mut reference_bus, &operation.fast, reference.pc + 2).unwrap_or(reference.pc + 2);
                        count += 1;
                        if count == 7 && ((gate && pending) || (!inline && !pop)) { break; }
                    }
                    let actual = unsafe { run(entry, &mut cpu, &mut bus, &mem, 0, cap) }.0;
                    assert_eq!(actual, count, "{syntax} sp={sp} watched={watched} pending={pending} cap={cap}");
                    assert!(cpu.architectural_state() == reference.architectural_state(),
                        "{syntax} sp={sp} watched={watched} pending={pending} cap={cap}");
                    assert_eq!(bus.data, reference_bus.data, "{syntax} sp={sp} cap={cap}");
                    assert_eq!((bus.ticks, bus.irq_windows), (reference_bus.ticks, reference_bus.irq_windows));
                    assert_eq!(cpu.jit_helpers.get("SpecialStack").copied().unwrap_or(0), u64::from(!inline && cap >= 7));
                    if inline && gate && cap >= 7 { assert_eq!(cpu.jit_helpers.get("stack_irq_window"), Some(&1)); }
                    reference_bus.data = initial_bytes;
                }
            }}}
        }
    }

    #[test]
    fn bulk_stack_transfers_match_ram_edges_unaligned_code_and_helper_paths() {
        let mut cases = Vec::new();
        for n in 0..=15 { for push in [false, true] {
            cases.push((if push { Fast::PushRets { n } } else { Fast::PopPc { n } },
                push, ((n as u32).max(3) - (n as u32).min(4) + 2) * 4));
        }}
        for mask in [1u16, 0x8000, 0xffff, 0x1234, 0x8001] { for pop in [false,true] {
            cases.push((Fast::StackMask { mask,pop }, !pop, mask.count_ones()*4));
        }}
        for (fast, push, size) in cases {
            for (ram_len, sp) in [(160, 128), (160, 127), (160, 129),
                (160, if push { size } else { 0 }),
                (160, if push { 160 } else { 160 - size }),
                (160, if push { size + 1 } else { 161 - size }),
                (160, 192), (0, 128), (1, 128)] {
                for watched in [-2i32, -1, 0, (size / 8) as i32, (size / 4 - 1) as i32] {
                    let mut bus = Memory::new(); let mut reference = Memory::new();
                    for i in 0..256 { bus.data[i] = (i as u8).wrapping_mul(13); }
                    reference.data.clone_from(&bus.data);
                    bus.flags.fill(0);
                    if watched == -1 { bus.flags.fill(1); }
                    else if watched >= 0 {
                        let low = if push { sp - size } else { sp };
                        bus.flags[(low / 4) as usize + watched as usize] = 1;
                    }
                    reference.flags.clone_from(&bus.flags);
                    let mut mem = bus.jit_mem().unwrap(); mem.ram_len = ram_len;
                    let mut j = Jit::new(false).unwrap();
                    let code = hot(&mut j, 0, 0x02000000, &[op(fast)], &mem);
                    let mut cpu = Cpu::new(0x02000000); cpu.sp = sp; cpu.rets = 0x87654321;
                    for r in 0..16 { cpu.regs[r] = (r as u32).wrapping_mul(0x12345678); }
                    cpu.flags = 13;
                    let mut expected = Cpu::new(cpu.pc); expected.sp = sp; expected.rets = cpu.rets;
                    expected.regs = cpu.regs; expected.flags = cpu.flags;
                    expected.pc = expected.exec_fast(&mut reference, &fast, cpu.pc + 2).unwrap_or(cpu.pc + 2);
                    assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, 1) }.0, 1);
                    assert_eq!((cpu.regs, cpu.sp, cpu.rets, cpu.pc, cpu.flags),
                        (expected.regs, expected.sp, expected.rets, expected.pc, expected.flags),
                        "{fast:?} ram={ram_len} sp={sp} watched={watched}");
                    assert_eq!(bus.data, reference.data, "{fast:?} ram={ram_len} sp={sp}");
                    if push && watched == -1 { assert_eq!(bus.version, reference.version, "every watched write must reach the bus"); }
                    if push && watched >= 0 { assert!(bus.version > 0, "the watched word must reach the bus"); }
                    if push && watched == -2 && sp % 4 == 0 && sp <= ram_len {
                        assert_eq!(bus.version, 0, "unwatched in-range stack must use native RAM stores");
                    }
                }
            }
        }
    }

    #[test]
    fn bulk_stack_transfers_flush_cached_registers_and_preserve_partial_state() {
        let mut ops = vec![op(Fast::AddRI { d: 0, s: 0, v: 3 }); 6];
        ops.extend([op(Fast::PushRets { n: 0 }), op(Fast::MovI { d: 0, v: 0 }), op(Fast::PopPc { n: 0 })]);
        let mut bus = Memory::new(); bus.flags.fill(0);
        let mem = bus.jit_mem().unwrap(); let mut j = Jit::new(false).unwrap();
        let code = hot(&mut j, 0, 0x02000000, &ops, &mem);
        for cap in 0..=ops.len() as u32 {
            bus.data.fill(0); bus.ticks = 0; bus.version = 0;
            let mut reference = Memory::new();
            let mut cpu = Cpu::new(0x02000000); cpu.sp = 128; cpu.rets = 0x02000100;
            cpu.regs = [0x80000000; 16]; cpu.flags = 15;
            let mut expected = Cpu::new(cpu.pc); expected.sp = cpu.sp; expected.rets = cpu.rets;
            expected.regs = cpu.regs; expected.flags = cpu.flags;
            for (i,o) in ops.iter().take(cap as usize).enumerate() {
                let next = 0x02000000 + (i as u32 + 1) * 2;
                expected.pc = expected.exec_fast(&mut reference, &o.fast, next).unwrap_or(next);
            }
            assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, cap) }.0, cap);
            assert_eq!((cpu.regs,cpu.sp,cpu.rets,cpu.pc,cpu.flags),
                (expected.regs,expected.sp,expected.rets,expected.pc,expected.flags), "cap={cap}");
            assert_eq!(bus.data, reference.data); assert_eq!(bus.version, 0);
        }
    }

    #[test]
    fn direct_ops_match_interpreter_with_aliases_and_edge_fields() {
        let mut cases = vec![Fast::SpAdd { v: u32::MAX - 3 }, Fast::RFromSp { d: 0, v: 17 },
            Fast::Mov2 { dh: 1, dl: 0, sh: 0, sl: 1 }, Fast::Mov2I { dh: 0, dl: 1, v: 0x80000001 },
            Fast::PopRets, Fast::Tb { r: 0, half: false }, Fast::Tb { r: 0, half: true }];
        for n in [0, 3, 4, 15] { cases.push(Fast::PushRets { n }); cases.push(Fast::PopPc { n }); }
        for p in [0, 1, 15, 31] { for l in [0, 1, 16, 31] {
            for signed in [false, true] { cases.push(Fast::BitExt { d: 1, s: 1, p, l, signed }); }
            cases.push(Fast::BitIns { d: 1, s: 1, p, l });
            cases.push(Fast::BitIns { d: 1, s: 2, p, l });
        }}
        let mut j = Jit::new(false).unwrap();
        for (id, fast) in cases.into_iter().enumerate() {
            let mut bus = Memory::new(); let mut reference = Memory::new();
            for i in 128..256 { bus.data[i] = i as u8; reference.data[i] = i as u8; }
            let mem = bus.jit_mem().unwrap();
            let mut cpu = Cpu::new(0); let mut expected = Cpu::new(0);
            cpu.regs = [0x87654321; 16]; cpu.regs[0] = 128; cpu.regs[2] = 0xfedcba98;
            cpu.sp = 192; cpu.rets = 0x12345678;
            expected.regs = cpu.regs; expected.sp = cpu.sp; expected.rets = cpu.rets;
            expected.pc = expected.exec_fast(&mut reference, &fast, 2).unwrap_or(2);
            let code = hot(&mut j, id, 0, &[op(fast)], &mem);
            assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, 1) }.0, 1, "{fast:?}");
            assert_eq!((cpu.regs, cpu.sp, cpu.rets, cpu.pc), (expected.regs, expected.sp, expected.rets, expected.pc), "{fast:?}");
            assert_eq!(bus.data, reference.data, "{fast:?}");
        }
    }

}
