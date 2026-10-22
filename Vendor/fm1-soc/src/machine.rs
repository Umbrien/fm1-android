//! Running the cores against the SoC: cpu0's block loop (fm1-live and the browser build share
//! it), and cpu1 next to it once the firmware releases it.
//!
//! cpu1 has no clock of its own. cpu0 runs in slices of `SLICE` instructions; after each,
//! cpu1 may run as many instructions as cycles passed (idle skips included, so it keeps the
//! hardware's pace, where both cores run at the CPU clock), through `Core1Bus`, whose tick
//! budget stops its interpreter and compiled code alike. It stops early once it is back at
//! the pc it started from with the same registers: a polling loop with nothing to do (the
//! stock firmware's job loop, X0X's), which would otherwise cost as much as cpu0.

use crate::{Soc, RAM_BASE, VEC_BASE};
use fm1_core::{Bus, Cpu};

/// A speculative reference cycle. Every admitted write has a complete rollback
/// path; unsupported peripheral effects are rejected before reaching the SoC.
struct ProofBus<'a> {
    soc: &'a mut Soc,
    words: Vec<(u32,u32)>,
    valid: bool,
}
#[derive(Clone,Copy)]
struct ProofRejected { kind: &'static str, addr:u32, width:u32 }

impl ProofBus<'_> {
    fn reject_at(&mut self,kind:&'static str,addr:u32,width:u32) -> ! {
        self.valid=false;
        // No panic hook: an unsupported speculative effect is ordinary rejection.
        std::panic::resume_unwind(Box::new(ProofRejected{kind,addr,width}))
    }
    fn read_allowed(&mut self, addr: u32, width: u32) -> bool {
        if !self.valid || !self.soc.proof_read_allowed(addr,width) { self.reject_at("read",addr,width); }
        true
    }

    fn write_allowed(&mut self, addr: u32, width: u32) -> bool {
        if !self.valid { self.reject_at("invalid",addr,width); }
        let addr = if width == 2 { addr & !1 } else { addr };
        let offset = addr.wrapping_sub(RAM_BASE);
        if offset as usize + width as usize <= self.soc.ram.len() {
            for word in (offset & !3..=(offset + width - 1) & !3).step_by(4) {
                if !self.soc.ram.proof_data_word(word) { self.reject_at("code-write",addr,width); }
                if !self.words.iter().any(|&(a,_)| a == word) {
                    if self.words.len() >= 128 { self.reject_at("journal-bound",addr,width); }
                    let Some(value) = self.soc.ram.proof_word(word) else { self.reject_at("ram-bounds",addr,width); };
                    self.words.push((word,value));
                }
            }
            true
        } else {
            // Narrow accesses merge inside their aligned register. An unaligned
            // word can straddle register/region boundaries and is not admitted.
            if (width == 4 && addr & 3 != 0) || !self.soc.proof_timer_write_allowed(addr & !3) { self.reject_at("write",addr,width); }
            true
        }
    }

    fn unchanged_words(&self) -> bool {
        self.words.iter().all(|&(a,v)| self.soc.ram.proof_word(a) == Some(v))
    }

    fn rollback_words(&mut self) {
        for &(a,v) in &self.words { self.soc.ram.restore_proof_word(a,v); }
    }
}

impl Bus for ProofBus<'_> {
    fn read8(&mut self,a:u32)->u8 { if self.read_allowed(a,1) { self.soc.read8_clock_frozen(a) } else { 0 } }
    fn read16(&mut self,a:u32)->u16 { if self.read_allowed(a,2) { self.soc.read16_clock_frozen(a) } else { 0 } }
    fn read32(&mut self,a:u32)->u32 { if self.read_allowed(a,4) { self.soc.read32_clock_frozen(a) } else { 0 } }
    fn write8(&mut self,a:u32,v:u8) { if self.write_allowed(a,1) { self.soc.write8(a,v); } }
    fn write16(&mut self,a:u32,v:u16) { if self.write_allowed(a,2) { self.soc.write16(a,v); } }
    fn write32(&mut self,a:u32,v:u32) { if self.write_allowed(a,4) { self.soc.write32(a,v); } }
    fn irq_pending(&self)->bool { self.soc.periph.any_pending() }
    fn code_writes(&self)->Option<u64> { self.soc.code_writes() }
    fn xip_writes(&self)->u64 { self.soc.xip_writes() }
    fn xip_mark(&mut self,lo:u32,hi:u32) { self.soc.xip_mark(lo,hi); }
    fn code_mark(&mut self,lo:u32,hi:u32) { self.soc.code_mark(lo,hi); }
    fn hw_lock(&mut self,_:u32,_:bool)->bool { self.reject_at("bus-effect",0,0) }
    fn idle(&mut self,_:u32) { self.reject_at("bus-effect",0,0) }
    fn irq_window(&mut self) { self.reject_at("bus-effect",0,0) }
}

/// Prove a deterministic fixed point against the frozen CPU1 bus. No hash or
/// firmware address is used: architecture, timer state and every written RAM
/// word must return exactly to their initial values. The trial is always rolled
/// back; its reference instructions are not reported as actual JIT retirement.
fn fixed_point_period(c1:&mut Cpu,soc:&mut Soc,probe:&(u32,[u32;16],u32))->Option<u32> {
    if c1.pair_pending.is_some() || c1.rep_end != 0 || !c1.pred_skips.is_empty() || c1.hold_irq != 0
        || soc.pending_irq_for(1).is_some() { return None; }
    let before = c1.architectural_state();
    let timer = soc.timer_proof_state();
    let retired = c1.insn_count;
    let decodes = c1.decodes;
    let syntax = c1.cur_syntax;
    let profiling = c1.profile; c1.profile=false;
    c1.begin_decode_trial(soc);
    let mmio_profile = soc.mmio_profile.take();
    let mut bus = ProofBus { soc, words:Vec::with_capacity(32), valid:true };
    let mut failure = ProofRejected{kind:"bound",addr:c1.pc,width:0};
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for count in 1..=256 {
            if c1.step(&mut bus).is_err() || !bus.valid {
                failure.kind="instruction"; return None;
            }
            if bus.soc.pending_irq_for(1).is_some() { failure.kind="interrupt"; return None; }
            // The original scheduler could stop at any run_fast return. Reject
            // every cycle containing its idle signature, preserving that boundary.
            if c1.pc == probe.0 && c1.regs == probe.1 && c1.sp == probe.2 {
                failure.kind="idle-boundary"; return None;
            }
            if c1.pc == before.pc() {
                if c1.architectural_state() != before { failure.kind="architecture"; return None; }
                if !bus.unchanged_words() { failure.kind="ram"; return None; }
                if bus.soc.timer_proof_state() != timer { failure.kind="timer"; return None; }
                return Some(count);
            }
        }
        None
    }));
    bus.rollback_words();
    bus.soc.restore_timer_proof_state(timer);
    bus.soc.mmio_profile=mmio_profile;
    c1.rollback_decode_trial();
    c1.restore_architectural_state(before); c1.insn_count=retired; c1.decodes=decodes;
    c1.cur_syntax=syntax; c1.profile=profiling;
    match result {
        Ok(Some(period)) => Some(period),
        result => {
            let payload=match result { Err(payload)=>Some(payload),_=>None };
            if let Some(rejected)=payload.as_ref().and_then(|p|p.downcast_ref::<ProofRejected>()) { failure=*rejected; }
            else if let Some(payload)=payload { std::panic::resume_unwind(payload); }
            if let Some(hist)=soc.cpu1_fixed_point_failures.as_mut() {
                *hist.entry(format!("{}:{:#x}:{}",failure.kind,failure.addr,failure.width)).or_default()+=1;
            }
            None
        }
    }
}

/// cpu0 runs in slices of this many instructions once cpu1 is running, cpu1 after each.
pub const SLICE: u32 = 1024;

/// cpu1's view of the SoC: no clock, a budget of ticks instead.
pub struct Core1Bus<'a> {
    pub soc: &'a mut Soc,
    pub budget: u64,
    /// During a batched idle probe, retain the original per-instruction IRQ
    /// observation boundary after a peripheral access or an interrupt gate change.
    pub observe_irq: bool,
}

impl Bus for Core1Bus<'_> {
    fn read8(&mut self, a: u32) -> u8 {
        self.soc.read8_clock_frozen(a)
    }
    fn read16(&mut self, a: u32) -> u16 {
        self.soc.read16_clock_frozen(a)
    }
    fn read32(&mut self, a: u32) -> u32 {
        self.soc.read32_clock_frozen(a)
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.soc.write8(a, v)
    }
    fn write16(&mut self, a: u32, v: u16) {
        self.soc.write16(a, v)
    }
    fn write32(&mut self, a: u32, v: u32) {
        self.soc.write32(a, v)
    }
    fn tick(&mut self) -> bool {
        self.budget = self.budget.saturating_sub(1);
        self.budget == 0 || (self.observe_irq && self.soc.pending_irq_for(1).is_some())
    }
    fn irq_pending(&self) -> bool {
        self.soc.periph.any_pending()
    }
    fn jit_mem(&mut self) -> Option<fm1_core::JitMem> {
        self.soc.jit_mem()
    }
    fn thread_mem(&mut self) -> Option<fm1_core::JitMem> { self.soc.thread_mem() }
    fn code_writes(&self) -> Option<u64> {
        self.soc.code_writes()
    }
    fn xip_writes(&self) -> u64 { self.soc.xip_writes() }
    fn xip_mark(&mut self, lo: u32, hi: u32) { self.soc.xip_mark(lo, hi) }
    fn code_mark(&mut self, lo: u32, hi: u32) {
        self.soc.code_mark(lo, hi)
    }
    fn tick_room(&self) -> u64 {
        self.budget.saturating_sub(1)
    }
    fn add_ticks(&mut self, n: u64) {
        self.budget = self.budget.saturating_sub(n);
    }
    fn hw_lock(&mut self, core: u32, set: bool) -> bool {
        self.soc.hw_lock(core, set)
    }
    fn irq_window(&mut self) {
        // (cpu1's loop looks at its interrupts before every run; a short stretch ends soon)
        if self.soc.periph.any_pending() {
            self.budget = self.budget.min(1);
        }
    }
    fn irq_window_for(&mut self, _core: u32, minimum_priority: u8) {
        if self.soc.pending_irq_for(1).is_some_and(|(_, priority)| priority >= minimum_priority) {
            self.budget = self.budget.min(1);
        }
    }
    fn next_tick_due(&self) -> bool {
        self.budget <= 1 || (self.observe_irq && self.soc.pending_irq_for(1).is_some())
    }
}

/// cpu0's block loop: interrupts taken between blocks, the SoC serviced when a tick is due.
/// `pending_tick` carries a tick already spent on the next instruction across calls.
pub fn run_blocks(cpu: &mut Cpu, soc: &mut Soc, n: u32, pending_tick: &mut bool) -> Result<(), String> {
    let mut done = 0u32;
    let mut ticked = std::mem::take(pending_tick);
    while done < n {
        if ticked || soc.periph.tick() {
            soc.advance(0);
        }
        ticked = false;
        if soc.periph.any_pending() {
            if let Some((irq, prio)) = soc.pending_irq() {
                if cpu.irq_ready(prio) {
                    let handler = soc.read32(VEC_BASE + 4 * irq as u32);
                    cpu.interrupt(handler, prio);
                }
            }
        }
        // Interrupt admission changes irq_levels before the first handler
        // instruction. Its TIMER4 reads must never use CPU0's idle heuristic.
        soc.periph.in_isr = !cpu.irq_levels.is_empty();
        match cpu.run_fast(soc, n - done) {
            Ok((0, _)) => {
                if let Err(e) = cpu.step(soc) {
                    return Err(format!("{e:?} at pc {:#010x}", cpu.pc));
                }
                done += 1;
            }
            Ok((k, due)) => {
                done += k;
                ticked = due;
            }
            Err(e) => return Err(format!("{e:?} at pc {:#010x}", cpu.pc)),
        }
        if soc.cpu1_release && soc.read32(0x1eee004) & 2 == 0 {
            break; // core 1 can start now (run_dual); reset-held clocks do not stop cpu0.
        }
    }
    // the next instruction's tick may already have happened: carry it
    *pending_tick = ticked;
    Ok(())
}

/// cpu1 once cpu0 released it (`0x10008 |= 8`): its entry is the word at 0x01C7FFF8 (the
/// stock firmware's stub at 0x020001B8 and X0X's entry both put it there; older images
/// without it: the RAM job loop at 0x01C022B6). It starts in supervisor mode.
pub fn released_cpu1(soc: &mut Soc, ram_size: usize) -> Option<Cpu> {
    if soc.cpu1_started { soc.cpu1_release = false; return None; }
    // A clock-release pulse can arrive before reset is deasserted. Keep it
    // latched and capture the mailbox only once the core may actually start.
    if soc.read32(0x1eee004) & 2 != 0 { return None; }
    if !std::mem::take(&mut soc.cpu1_release) && soc.read32(0x10008) & 8 == 0 {
        return None;
    }
    let entry = soc.read32(0x01C7_FFF8);
    let in_code = (0x0200_0000..0x0300_0000).contains(&entry) || (RAM_BASE..RAM_BASE + ram_size as u32).contains(&entry);
    if std::env::var_os("FM1_DUAL_TRACE").is_some() {
        eprintln!("cpu1 released at {} ms (entry {entry:#010x})", soc.periph.ms());
    }
    let mut c1 = Cpu::new(if in_code { entry } else { RAM_BASE + 0x22b6 });
    c1.core_id = 1;
    c1.usp = RAM_BASE + 0x15eb4;
    c1.sp = RAM_BASE + 0x16eb4;
    soc.cpu1_started = true;
    Some(c1)
}

/// Run cpu1 for up to `cycles` instructions: interrupts from its own bank, then blocks or
/// single steps; back where it started with nothing changed means it is idle.
pub fn run_cpu1(c1: &mut Cpu, soc: &mut Soc, cycles: u64) -> Result<(), String> {
    // Retain the original single-instruction probe as the reference scheduler.
    // Compiled execution observes exactly the same PC/IRQ boundaries in batches.
    let before = c1.insn_count;
    let result = run_cpu1_with_probe(c1, soc, cycles, c1.jit_on);
    if let Some(hist) = soc.cpu1_slice_profile.as_mut() {
        let bucket = (64 - cycles.leading_zeros() as usize).min(32);
        hist[bucket].0 += 1;
        hist[bucket].1 += c1.insn_count - before;
    }
    result
}

fn run_cpu1_with_probe(c1: &mut Cpu, soc: &mut Soc, cycles: u64, batched: bool) -> Result<(), String> {
    if soc.core_paused(1) {
        return Ok(());
    }
    let probe = (c1.pc, c1.regs, c1.sp);
    let mut left = cycles.min(1 << 20);
    // first a block at a time, looking for the way back to where it started (a polling loop
    // goes round in well under this); then, busy, in big stretches
    let mut probing = 512u64;
    let mut tried_fixed_point = false;
    while left > 0 {
        if batched && !tried_fixed_point && probing == 0 && left >= 8192 && c1.has_compiled_block() {
            tried_fixed_point = true;
            soc.cpu1_fixed_points[0] += 1;
            if let Some(period) = fixed_point_period(c1,soc,&probe) {
                let skipped = left / period as u64 * period as u64;
                c1.insn_count += skipped;
                left -= skipped;
                soc.cpu1_fixed_points[1] += 1;
                soc.cpu1_fixed_points[2] += skipped;
                if left == 0 { break; }
            }
        }
        if soc.periph.any_pending() {
            if let Some((irq, prio)) = soc.pending_irq_for(1) {
                if c1.irq_ready(prio) {
                    let handler = soc.read32(VEC_BASE + 4 * irq as u32);
                    c1.interrupt(handler, prio);
                }
            }
        }
        let chunk = if probing > 0 { if batched { left.min(probing) } else { 1 } } else { left.min(256) };
        c1.run_stop_pc = (batched && probing > 0).then_some(probe.0);
        let mut bus = Core1Bus { soc: &mut *soc, budget: chunk.max(64) + 1, observe_irq: batched && probing > 0 };
        let result = c1.run_fast(&mut bus, chunk as u32);
        c1.run_stop_pc = None;
        let k = match result {
            Ok((0, _)) => {
                if let Err(e) = c1.step(&mut bus) {
                    return Err(format!("cpu1: {e:?} at pc {:#010x}", c1.pc));
                }
                1
            }
            Ok((k, _)) => k as u64,
            Err(e) => return Err(format!("cpu1: {e:?} at pc {:#010x}", c1.pc)),
        };
        left = left.saturating_sub(k);
        probing = probing.saturating_sub(k);
        if c1.pc == probe.0 && c1.sp == probe.2 && c1.regs == probe.1 {
            break; // polling with nothing to do
        }
    }
    Ok(())
}

/// Run n instructions of cpu0 through `run0` (the block loop, or a caller's own), with cpu1
/// alongside once it is released.
pub fn run_dual(
    cpu1: &mut Option<Cpu>,
    soc: &mut Soc,
    n: u32,
    ram_size: usize,
    jit_on: bool,
    mut run0: impl FnMut(&mut Soc, u32) -> Result<(), String>,
) -> Result<(), String> {
    let mut done = 0;
    while cpu1.is_none() {
        // cpu0 alone, in stretches short enough to see a release soon (X0X holds the release
        // bit only while it waits for core 1 to report in, then gives up and clears it)
        *cpu1 = released_cpu1(soc, ram_size).map(|mut c| {
            c.jit_on = jit_on;
            c
        });
        if cpu1.is_some() || done >= n {
            break;
        }
        let k = (n - done).min(4096);
        soc.periph.room_cap = k as u64;
        run0(soc, k)?;
        done += k;
    }
    let Some(c1) = cpu1.as_mut() else { return Ok(()) };
    soc.periph.room_cap = SLICE as u64;
    while done < n {
        let k = (n - done).min(SLICE);
        let c0 = soc.periph.cycles;
        if soc.core_paused(0) {
            soc.advance(k as u64); // held by cpu1 (C0_CON bit 2): time runs on
        } else {
            run0(soc, k)?;
        }
        done += k;
        let dt = soc.periph.cycles - c0;
        run_cpu1(c1, soc, dt)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::XIP_BASE;

    #[test]
    fn first_interrupt_counter_read_cannot_apply_main_loop_idle_skip() {
        for jit in [false, true] {
            let mut image = vec![0; 256];
            image[64..68].copy_from_slice(&[0x10, 0x60, 0x80, 0]); // r0 = [r1]; rts
            let mut soc = Soc::new(512 << 10, image.clone());
            let mut cpu = Cpu::new(XIP_BASE); cpu.jit_on = jit;
            cpu.regs[1] = 0x10804; cpu.ie = true; cpu.sreg_store[0] = 0x100;
            let initial = cpu.architectural_state();
            if jit {
                cpu.pc = XIP_BASE + 64; cpu.rets = cpu.pc; cpu.ie = false;
                run_blocks(&mut cpu, &mut soc, 256, &mut false).unwrap();
                assert!(cpu.has_compiled_block());
                cpu.restore_architectural_state(initial);
                soc = Soc::new(512 << 10, image);
            }
            soc.write32(0x10800, 1); soc.advance(100); soc.periph.skip_idle = true;
            // Prime two close main-loop reads. A further non-ISR read would
            // trigger the optional skip, even on the very first handler op.
            soc.read32(0x10804); soc.read32(0x10804);
            let cycles = soc.periph.cycles;
            soc.write32(VEC_BASE + 4 * 120, XIP_BASE + 64);
            soc.periph.icfg[15] = 3; soc.periph.ilat = 1;
            run_blocks(&mut cpu, &mut soc, 1, &mut false).unwrap();
            assert_eq!(cpu.irq_levels, vec![1]);
            assert!(soc.periph.in_isr);
            assert_eq!(soc.periph.cycles, cycles + 1, "jit={jit}");
            assert_eq!(soc.periph.skipped, 0, "jit={jit}");
            assert_eq!(cpu.regs[0], (cycles + 1) as u32 / 10);
        }
    }

    #[test]
    fn cpu1_counter_reads_preserve_shared_clock_and_cpu0_polling_state() {
        for in_isr in [false, true] {
            let mut soc = Soc::new(512, vec![]);
            soc.write32(0x10800, 1); soc.advance(0x1234_5678 * 10);
            soc.periph.skip_idle = true;
            // Initialize a CPU0 streak without triggering a skip.
            soc.read32(0x10804); soc.read32(0x10804);
            soc.periph.in_isr = in_isr;
            let before = soc.periph.clone();
            {
                let mut bus = Core1Bus { soc: &mut soc, budget: 100, observe_irq: false };
                for _ in 0..8 {
                    assert_eq!(bus.read32(0x10804), 0x1234_5678);
                    for offset in 0..4 {
                        assert_eq!(bus.read8(0x10804 + offset), (0x1234_5678u32 >> (offset * 8)) as u8);
                        assert_eq!(bus.read16(0x10804 + offset), (0x1234_5678u32 >> ((offset & 2) * 8)) as u16);
                    }
                }
                assert_eq!(bus.budget, 100);
            }
            assert!(soc.periph == before, "CPU1 changed shared clock or CPU0 poll state");
            // Main-loop polling retains its original streak and still skips.
            if !in_isr {
                soc.read32(0x10804);
                assert!(soc.periph.cycles > before.cycles && soc.periph.skipped > before.skipped);
            }
        }
    }

    #[test]
    fn cpu1_release_accepts_both_register_paths_and_resumes_without_recreation() {
        for clock_path in [false, true] {
            let mut image = vec![0; 256]; image[64..68].copy_from_slice(&[0xc5, 0x21, 0x80, 0]);
            let mut soc = Soc::new(1 << 20, image);
            let entry = XIP_BASE + 64;
            soc.write32(0x01c7_fff8, entry);
            if clock_path { soc.write32(0x10008, 8); }
            else {
                soc.write32(0x1eee004, 2); // reset held
                soc.write32(0x1eee004, 10); // release requested, reset still held
                assert!(soc.core_paused(1));
                assert!(released_cpu1(&mut soc, 1 << 20).is_none());
                soc.write32(0x1eee004, 8); // deassert reset
            }
            let mut c1 = released_cpu1(&mut soc, 1 << 20).expect("CPU1 release");
            assert_eq!((c1.pc, c1.core_id), (entry, 1));
            assert!(!soc.cpu1_release);
            c1.rets = entry; c1.regs[15] = 0xabcd_1234;
            // Repeated pause/resume and clock pulses operate on this same CPU.
            // A changed mailbox must not replace its PC or register state.
            soc.write32(0x01c7_fff8, XIP_BASE + 128);
            for _ in 0..3 {
                soc.write32(0x1eee004, 4); assert!(soc.core_paused(1));
                soc.write32(0x1eee004, 8); assert!(!soc.core_paused(1));
                soc.write32(0x10008, 0); soc.write32(0x10008, 8);
                assert!(!soc.cpu1_release);
                assert!(released_cpu1(&mut soc, 1 << 20).is_none());
            }
            let mut cpu1 = Some(c1);
            run_dual(&mut cpu1, &mut soc, 128, 1 << 20, false, |soc, n| { soc.advance(n as u64); Ok(()) }).unwrap();
            let c1 = cpu1.unwrap();
            assert_eq!(c1.regs[15], 0xabcd_1234);
            assert!(c1.regs[5] > 0 && c1.insn_count > 0);
            assert!(!soc.cpu1_release);
        }
    }

    #[test]
    fn cpu1_clock_release_waits_for_reset_and_the_final_mailbox() {
        for clock_remains_on in [false,true] {
            let mut soc=Soc::new(1<<20,vec![0;256]);
            soc.write32(0x01c7fff8,XIP_BASE+32);
            soc.write32(0x1eee004,2); // Reset asserted before the clock starts.
            soc.write32(0x10008,8);
            assert!(released_cpu1(&mut soc,1<<20).is_none());
            assert!(soc.cpu1_release && !soc.cpu1_started);
            // A latched clock request while reset is held must not truncate
            // every CPU0 block to a single instruction.
            soc.write32(RAM_BASE,0x0080_21c5); // r5+=1; rts back to this entry.
            let mut cpu0=Cpu::new(RAM_BASE);cpu0.rets=RAM_BASE;
            let mut tick=false;run_blocks(&mut cpu0,&mut soc,128,&mut tick).unwrap();
            assert_eq!(cpu0.insn_count,128);assert_eq!(cpu0.regs[5],64);
            // Firmware may publish a new entry while the core is still held.
            soc.write32(0x01c7fff8,XIP_BASE+64);
            soc.write32(0x1eee004,10); // Run requested, but reset remains asserted.
            if !clock_remains_on {soc.write32(0x10008,0);}
            assert!(released_cpu1(&mut soc,1<<20).is_none());
            assert!(soc.cpu1_release && !soc.cpu1_started);
            soc.write32(0x1eee004,8);
            let mut c1=released_cpu1(&mut soc,1<<20).unwrap();
            assert_eq!(c1.pc,XIP_BASE+64);assert!(!soc.cpu1_release);
            c1.regs[15]=0x12345678;
            soc.write32(0x01c7fff8,XIP_BASE+128);
            soc.write32(0x1eee004,2);soc.write32(0x1eee004,8);
            assert!(released_cpu1(&mut soc,1<<20).is_none());
            assert_eq!(c1.regs[15],0x12345678);
        }
    }

    #[test]
    fn p33_mmio_transactions_are_shared_between_cpu_views_and_reset_per_soc() {
        fn xfer<B: Bus>(bus: &mut B, data: u8) -> u8 {
            bus.write8(0x13e0c, data);
            let con = bus.read32(0x13e08);
            bus.write32(0x13e08, con | 0x10);
            assert_eq!(bus.read32(0x13e08) & 0x12, 0);
            bus.read8(0x13e0c)
        }
        let mut soc = Soc::new(512, vec![]);
        soc.write32(0x13e08, 1);
        xfer(&mut soc, 0); xfer(&mut soc, 9);
        // CPU1 clocks the data byte of CPU0's selected command.
        { let mut other = Core1Bus { soc: &mut soc, budget: 64, observe_irq: false }; xfer(&mut other, 0x8f); }
        soc.write32(0x13e08, 0);
        soc.write32(0x13e08, 1);
        xfer(&mut soc, 0x80); xfer(&mut soc, 9);
        assert_eq!(xfer(&mut soc, 0x5e), 0x8f);
        assert!(soc.unknown_accesses().is_empty());
        let fresh = Soc::new(512, vec![]);
        assert_eq!(fresh.p33.read(0x13e08), 0);
        assert_eq!(fresh.p33.read(0x13e0c), 0);
    }

    fn fixture(code: &[u8], jit: bool, start: u32) -> (Cpu, Soc) {
        let mut image = vec![0; 256]; image[..code.len()].copy_from_slice(code);
        let mut cpu = Cpu::new(XIP_BASE + start); cpu.core_id = 1; cpu.jit_on = jit;
        cpu.rets = XIP_BASE; cpu.sp = RAM_BASE + 2048;
        (cpu, Soc::new(4096, image))
    }

    fn same(reference: &Cpu, compiled: &Cpu, a: &Soc, b: &Soc) {
        assert_eq!((reference.pc, reference.regs, reference.sp, reference.flags,
            reference.insn_count, reference.hold_irq, &reference.irq_levels),
            (compiled.pc, compiled.regs, compiled.sp, compiled.flags,
            compiled.insn_count, compiled.hold_irq, &compiled.irq_levels));
        assert_eq!((a.periph.cycles, a.periph.ilat, a.code_writes()),
            (b.periph.cycles, b.periph.ilat, b.code_writes()));
        assert!(compiled.run_stop_pc.is_none());
    }

    #[test]
    fn fixed_point_proof_accepts_idempotent_stack_and_timer_and_rolls_back_trials() {
        let code = [0xd9,0xe8,0xf0,0x0d, 0xde,0xee,0x21,0x10, 0xd1,0xe8,0xf0,0x0d, 0x80,0];
        let (mut cpu,mut soc)=fixture(&code,true,0);
        cpu.regs[2]=0x10904;
        // Prime the stack values; subsequent writes must be exactly idempotent.
        { let mut bus=Core1Bus { soc:&mut soc,budget:1025,observe_irq:false };
            for _ in 0..4 { cpu.step(&mut bus).unwrap(); }
        }
        let state=cpu.architectural_state(); let retired=cpu.insn_count;
        let timer=soc.timer_proof_state();
        let probe=(XIP_BASE+128,[u32::MAX;16],cpu.sp);
        assert_eq!(fixed_point_period(&mut cpu,&mut soc,&probe),Some(4));
        assert!(cpu.architectural_state()==state); assert_eq!(cpu.insn_count,retired);
        assert!(soc.timer_proof_state()==timer);
        // An SPI0 write is rejected before forwarding; no flash state can escape.
        cpu.regs[2]=0x11c08;
        let state=cpu.architectural_state(); let before_flash=soc.flash.mem.clone();
        assert_eq!(fixed_point_period(&mut cpu,&mut soc,&probe),None);
        assert!(cpu.architectural_state()==state); assert_eq!(soc.flash.mem,before_flash);
        assert!(soc.timer_proof_state()==timer);
    }

    #[test]
    fn fixed_point_proof_rejects_changed_data_idle_signatures_and_clock_reads() {
        let (mut cpu,mut soc)=fixture(&[0xde,0xee,0x21,0x10,0x80,0],true,0);
        cpu.regs[1]=0x55; cpu.regs[2]=RAM_BASE+128;
        let probe=(XIP_BASE+128,[u32::MAX;16],cpu.sp);
        let state=cpu.architectural_state();
        assert_eq!(fixed_point_period(&mut cpu,&mut soc,&probe),None);
        assert_eq!(soc.ram.read8(128),0,"failed trial restores data");
        assert!(cpu.architectural_state()==state);
        soc.ram.write8(128,0x55);
        assert_eq!(fixed_point_period(&mut cpu,&mut soc,&probe),Some(2));
        let original_idle=(cpu.pc,cpu.regs,cpu.sp);
        assert_eq!(fixed_point_period(&mut cpu,&mut soc,&original_idle),None);
        // This read would advance TIMER4's poll clock. It must abort immediately.
        { let mut bus=ProofBus { soc:&mut soc,words:Vec::new(),valid:true };
            let reject=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| bus.read32(0x10804)));
            assert!(reject.is_err()); assert!(!bus.valid);
        }
        assert!(cpu.architectural_state()==state);
    }

    #[test]
    fn proof_journal_restores_unaligned_word_writes_and_rejects_code() {
        let (_,mut soc)=fixture(&[],true,0);
        let mut bus=ProofBus { soc:&mut soc,words:Vec::new(),valid:true };
        bus.write32(RAM_BASE+127,0x12345678);
        assert_eq!(bus.words.len(),2); assert!(!bus.unchanged_words());
        bus.rollback_words(); assert_eq!(bus.soc.ram.read32(127),0);
        bus.soc.ram.mark_code(128,132);
        let reject=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| bus.write8(RAM_BASE+128,1)));
        assert!(reject.is_err()); assert_eq!(bus.soc.ram.read8(128),0);
    }

    #[test]
    fn proof_os_tick_reads_are_stable_but_adjacent_irq_registers_are_rejected() {
        let (_,mut soc)=fixture(&[],true,0);
        soc.periph.cycles=12345;
        let before=soc.timer_proof_state();
        let mut bus=ProofBus { soc:&mut soc,words:Vec::new(),valid:true };
        for _ in 0..3 {
            bus.read32(0x1eef0f0); bus.read8(0x1eef0f1); bus.read16(0x1eef0f3);
        }
        assert!(bus.soc.timer_proof_state()==before);
        let reject=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| bus.read32(0x1eef1a0)));
        assert!(reject.is_err());
    }

    #[test]
    fn large_cpu1_slice_accounts_exact_fixed_point_iterations_after_work() {
        let mut code=vec![0;2048];
        let loop_pc=XIP_BASE+1024;
        code[1024..1038].copy_from_slice(&[0xd9,0xe8,0xf0,0x0d,0xde,0xee,0x21,0x10,0xd1,0xe8,0xf0,0x0d,0x80,0]);
        let make=|jit| {
            let mut soc=Soc::new(4096,code.clone());
            let mut cpu=Cpu::new(loop_pc); cpu.core_id=1; cpu.jit_on=jit;
            cpu.rets=loop_pc; cpu.sp=RAM_BASE+2048; cpu.regs[2]=0x10904;
            // Heat the prefix and stable cycle with observation enabled.
            cpu.run_stop_pc=Some(u32::MAX);
            for _ in 0..40 {
                cpu.pc=XIP_BASE;
                let mut bus=Core1Bus { soc:&mut soc,budget:4097,observe_irq:false };
                assert_eq!(cpu.run_fast(&mut bus,2048).unwrap().0,2048);
            }
            cpu.run_stop_pc=None;
            cpu.pc=XIP_BASE; cpu.insn_count=0;
            (cpu,soc)
        };
        let (mut reference,mut a)=make(false); let (mut compiled,mut b)=make(true);
        run_cpu1_with_probe(&mut reference,&mut a,20003,false).unwrap();
        run_cpu1_with_probe(&mut compiled,&mut b,20003,true).unwrap();
        same(&reference,&compiled,&a,&b);
        assert_eq!(reference.insn_count,20003);
        assert_eq!(b.cpu1_fixed_points[0],1,"each slice gets at most one bounded proof");
        assert!(b.cpu1_fixed_points[2]>10000,"proof must account real guest work: {:?}",b.cpu1_fixed_points);
        assert!(reference.architectural_state()==compiled.architectural_state());
        for off in 0..4096 { assert_eq!(a.ram.read8(off),b.ram.read8(off)); }
        assert!(a.timer_proof_state()==b.timer_proof_state());
    }

    #[test]
    fn batched_cpu1_probe_matches_single_steps_for_idle_busy_and_interior_entry() {
        for code in [&[0, 0, 0, 0, 0, 0, 0x80, 0][..],
            &[0xc0, 0x21, 0xc1, 0x21, 0, 0, 0x80, 0][..]] {
            for start in [0, 2, 4] {
                let (mut reference, mut a) = fixture(code, false, start);
                let (mut compiled, mut b) = fixture(code, true, start);
                // Repeated calls heat all block entries and chains. Different budgets
                // cut inside a block, on the probe return, and beyond the probe window.
                for cycle in [1, 2, 3, 4, 7, 31, 511, 512, 513, 1024].into_iter().cycle().take(200) {
                    run_cpu1_with_probe(&mut reference, &mut a, cycle, false).unwrap();
                    run_cpu1_with_probe(&mut compiled, &mut b, cycle, true).unwrap();
                    same(&reference, &compiled, &a, &b);
                }
            }
        }
    }

    #[test]
    fn special_stack_preserves_nested_irq_save_and_restore_boundaries() {
        for core in [0, 1] { for jit in [false, true] { for pending in [false, true] { for pop in [false, true] {
            let image = || {
                let mut code = vec![0; 256];
                // Save/restore {psr,reti}, then increment r0. Saving opens
                // nesting; restoring keeps nesting closed despite a pending IRQ.
                code[..6].copy_from_slice(&[if pop { 0xa1 } else { 0xe1 }, 0x04, 0xc0, 0x21, 0x80, 0]);
                code[64..68].copy_from_slice(&[0xc4, 0x21, 0x80, 0]);
                code
            };
            let mut soc = Soc::new(512 << 10, image());
            let mut cpu = Cpu::new(XIP_BASE); cpu.core_id = core; cpu.jit_on = jit;
            cpu.sp = RAM_BASE + 2048; cpu.rets = XIP_BASE;
            cpu.ie = true; cpu.sreg_store[0] = 0x100; cpu.irq_levels = vec![1]; cpu.nest_ok = false;
            let initial = cpu.architectural_state();
            if jit {
                // Compile and chain the actual firmware block before raising
                // the IRQ, so this checks both native entry and warmed tails.
                if core == 0 { run_blocks(&mut cpu, &mut soc, 256, &mut false).unwrap(); }
                else { run_cpu1_with_probe(&mut cpu, &mut soc, 256, true).unwrap(); }
                cpu.restore_architectural_state(initial);
                assert!(cpu.has_compiled_block());
                soc = Soc::new(512 << 10, image());
            }
            if core == 0 { soc.periph.icfg[15] = 5; } else { soc.periph.icfg1[15] = 5; }
            soc.periph.ilat = u32::from(pending);
            soc.write32(VEC_BASE + 4 * 120, XIP_BASE + 64);
            assert_eq!(soc.read32(VEC_BASE + 4 * 120), XIP_BASE + 64);
            assert!(!cpu.irq_ready(2));
            let before = cpu.insn_count;
            if core == 0 { run_blocks(&mut cpu, &mut soc, 2, &mut false).unwrap(); }
            else { run_cpu1_with_probe(&mut cpu, &mut soc, 2, jit).unwrap(); }
            assert_eq!(cpu.insn_count - before, 2);
            assert_eq!((cpu.regs[0], cpu.regs[4]), if pending && !pop { (0, 1) } else { (1, 0) },
                "core={core} jit={jit} pending={pending} pop={pop}");
            assert_eq!(&cpu.irq_levels, if pending && !pop { &[1, 2][..] } else { &[1][..] });
            assert_eq!(cpu.nest_ok, !pop && !pending);
            assert_eq!(cpu.sp, RAM_BASE + if pop { 2056 } else { 2040 });
        }}}}
    }

    #[test]
    fn batched_cpu1_probe_observes_pending_and_newly_enabled_interrupts() {
        // First case unmasks a pending software IRQ. Second raises it through MMIO.
        for code in [&[0x61, 0, 0xc0, 0x21, 0x80, 0][..],
            &[0xde, 0xee, 0x21, 0x10, 0xc3, 0x21, 0x80, 0][..]] {
            let (mut reference, mut a) = fixture(code, false, 0);
            let (mut compiled, mut b) = fixture(code, true, 0);
            for (cpu, soc) in [(&mut reference, &mut a), (&mut compiled, &mut b)] {
                cpu.sreg_store[0] = 0x100;
                cpu.regs[1] = 1; cpu.regs[2] = 0x1eef1a0;
                cpu.ie = code[0] != 0x61;
                soc.periph.icfg1[15] = 3; // IRQ 120, priority 1, enabled.
                if code[0] == 0x61 { soc.periph.ilat = 1; }
                soc.write32(VEC_BASE + 4 * 120, XIP_BASE + 64);
                soc.xip[64..68].copy_from_slice(&[0xc4, 0x21, 0x80, 0]);
            }
            for _ in 0..100 {
                run_cpu1_with_probe(&mut reference, &mut a, 37, false).unwrap();
                run_cpu1_with_probe(&mut compiled, &mut b, 37, true).unwrap();
                same(&reference, &compiled, &a, &b);
            }
            assert!(!compiled.irq_levels.is_empty());
        }
    }
}
