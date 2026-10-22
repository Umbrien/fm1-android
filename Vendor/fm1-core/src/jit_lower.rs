//! Native lowering of raw families, preserving the interpreter's operand and
//! memory transaction order. Each family remains one retired guest instruction.
use super::{Gen, CPU, KEEP, KEEP2, off_sp};

fn pair(g: &mut Gen<'_>, p: u32) {
    g.ld_reg(0, (p * 2) as u8);
    g.ld_reg(1, (p * 2 + 1) as u8);
    g.a.add_x_lsl(0, 0, 1, 32);
}

fn store_pair(g: &mut Gen<'_>, p: u32) {
    g.st_reg(0, (p * 2) as u8);
    g.a.lsr_imm_x(0, 0, 32);
    g.st_reg(0, (p * 2 + 1) as u8);
}

/// Returns false without emitting anything when this family still needs the
/// original helper. `memory` requests the caller's ordinary post-op bus barrier.
pub(super) fn emit(g: &mut Gen<'_>, route: u8, raw: u32, idx: u32, memory: &mut bool) -> bool {
    match route {
        21 => {
            let p = raw >> 29;
            let signed = raw & (1 << 28) != 0;
            match raw & 0xff {
                0xd0 | 0xd8 => {
                    let by_reg = raw & 0xff == 0xd8;
                    pair(g, p);
                    let op = if by_reg { (raw >> 16) & 3 } else { (raw >> 26) & 3 };
                    if by_reg {
                        g.ld_reg(2, ((raw >> 24) & 15) as u8);
                        match op {
                            0 | 1 => g.a.lslv_x(0, 0, 2),
                            2 => g.a.lsrv_x(0, 0, 2),
                            _ => g.a.asrv_x(0, 0, 2),
                        }
                    } else {
                        let n = ((raw >> 16) & 15) | (((raw >> 24) & 3) << 4);
                        match op {
                            0 | 1 => g.a.lsl_imm_x(0, 0, n),
                            2 => g.a.lsr_imm_x(0, 0, n),
                            _ => g.a.asr_imm_x(0, 0, n),
                        }
                    }
                }
                0xf8 | 0xfc | 0xfe => {
                    g.ld_reg(0, ((raw >> 20) & 15) as u8);
                    g.ld_reg(1, ((raw >> 24) & 15) as u8);
                    if signed { g.a.smull(2, 0, 1); } else { g.a.umull(2, 0, 1); }
                    match raw & 0xff {
                        0xfc => { pair(g, p); g.a.add_x(0, 0, 2); }
                        0xfe => { pair(g, p); g.a.sub_x(0, 0, 2); }
                        _ => g.a.mov_x(0, 2),
                    }
                }
                _ => {
                    pair(g, (raw >> 21) & 7);
                    g.ld_reg(1, ((raw >> 24) & 15) as u8);
                    // AArch64 matches wrapping signed division, including zero and MIN/-1.
                    if signed { g.a.sxtw(1, 1); g.a.sdiv_x(0, 0, 1); }
                    else { g.a.udiv_x(0, 0, 1); }
                }
            }
            store_pair(g, p);
        }
        22 => {
            g.ld_reg(0, ((raw >> 24) & 15) as u8);
            g.a.rev(0, 0);
            g.st_reg(0, (raw >> 28) as u8);
        }
        16 => {
            let m = crate::decode_pair_mem(raw as u64);
            if let Some(b) = m.base { g.ld_reg(0, b as u8); }
            else { g.a.ldr(0, CPU, off_sp()); }
            if let Some(i) = m.index { g.ld_reg(1, i as u8); }
            else { g.a.mov32(1, m.off as u32); }
            g.a.add(KEEP2, 0, 1);
            g.a.mov(KEEP, if m.post { 0 } else { KEEP2 });
            if let Some(b) = m.base.filter(|_| m.pre || m.post) { g.st_reg(KEEP2, b as u8); }
            for r in [2 * m.pair, 2 * m.pair + 1] {
                g.a.mov(0, KEEP);
                if r & 1 != 0 { g.a.add_imm(0, 0, 4); }
                if m.store { g.ld_reg(1, r as u8); g.store(4, idx); }
                else { g.load(4, false, idx); g.st_reg(0, r as u8); }
            }
            *memory = true;
        }
        18 | 19 => {
            let w = match (raw >> 8) & 0xff { 0xec => 4, 0xed => 2, _ => 1 };
            let (d, b) = ((raw >> 28) as u8, ((raw >> 20) & 15) as u8);
            let f = ((raw >> 16) & 15) | (((raw >> 24) & 15) << 4);
            let word_off = route == 18 && w == 4 && raw & (1 << 17) == 0;
            let (store, inc, signed) = if route == 18 && w == 4 {
                let v = ((raw >> 18) & 3) | (((raw >> 24) & 15) << 2) | ((raw & 7) << 6);
                (raw & (1 << 16) != 0, 4 * ((v as i32) << 23 >> 23), false)
            } else if w == 1 {
                (raw & 2 != 0, if raw & 1 != 0 { f as i32 - 256 } else { f as i32 }, raw & 4 != 0)
            } else {
                let v = (f & !1) | ((raw & 3) << 8);
                (raw & (1 << 16) != 0, (v as i32) << 22 >> 22, w == 2 && raw & 4 != 0)
            };
            g.ld_reg(0, b);
            if route == 19 {
                g.a.mov(KEEP, 0);
                g.add_imm(KEEP2, 0, inc as u32);
            } else {
                g.add_imm(0, 0, inc as u32);
                if !word_off { g.st_reg(0, b); }
            }
            if store { g.ld_reg(1, d); g.store(w, idx); }
            else { g.load(w, signed, idx); g.st_reg(0, d); }
            // Raw19's base is derived from its old value even when the load targets it.
            if route == 19 { g.st_reg(KEEP2, b); }
            *memory = true;
        }
        32 => {
            let Some(m) = crate::idx_inc(raw as u64) else { return true };
            g.ld_reg(0, m.b as u8);
            g.ld_reg(1, m.i as u8);
            g.a.add(KEEP2, 0, 1);
            g.a.mov(0, if m.pre { KEEP2 } else { 0 });
            if m.store {
                g.ld_reg(1, m.d as u8);
                if m.high { g.a.lsr_imm(1, 1, 16); }
                g.store(m.w, idx);
            } else { g.load(m.w, m.signed, idx); g.st_reg(0, m.d as u8); }
            g.st_reg(KEEP2, m.b as u8);
            *memory = true;
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use crate::{BOp, Bus, Cpu, Fast};
    use super::super::{Jit, JitMem, HOT, MAX_PENDING_VISITS, run};
    #[derive(Clone)]
    struct Memory { data: Vec<u8>, events: Vec<(bool, u8, u32, u32)>, direct: bool }
    impl Memory {
        fn new(direct: bool) -> Self { Self { data: (0..4096).map(|i| (i * 71) as u8).collect(), events: vec![], direct } }
        fn value(&self, a: u32, w: u8) -> u32 {
            let a = if w == 2 { a & !1 } else { a };
            (0..w).fold(0, |v, i| v | ((self.data[a.wrapping_add(i as u32) as usize & 4095] as u32) << (8 * i)))
        }
        fn load(&mut self, a: u32, w: u8) -> u32 {
            let v = self.value(a, w); self.events.push((false, w, a, v)); v
        }
        fn store(&mut self, a: u32, w: u8, v: u32) {
            self.events.push((true, w, a, v & (u32::MAX >> (32 - 8 * w))));
            for i in 0..w { self.data[a.wrapping_add(i as u32) as usize & 4095] = (v >> (8 * i)) as u8; }
        }
    }
    impl Bus for Memory {
        fn read8(&mut self, a: u32) -> u8 { self.load(a, 1) as u8 }
        fn read16(&mut self, a: u32) -> u16 { self.load(a, 2) as u16 }
        fn read32(&mut self, a: u32) -> u32 { self.load(a, 4) }
        fn write8(&mut self, a: u32, v: u8) { self.store(a, 1, v as u32); }
        fn write16(&mut self, a: u32, v: u16) { self.store(a, 2, v as u32); }
        fn write32(&mut self, a: u32, v: u32) { self.store(a, 4, v); }
        fn tick_room(&self) -> u64 { 1000 }
        fn next_tick_due(&self) -> bool { false }
        fn jit_mem(&mut self) -> Option<JitMem> {
            static FLAGS: [u8; 1025] = [0; 1025];
            Some(JitMem { ram: self.data.as_mut_ptr(), ram_base: 0x1000,
                ram_len: if self.direct { 4096 } else { 0 }, ram_code: FLAGS.as_ptr(),
                xip: std::ptr::null(), xip_base: 0x0200_0000, xip_len: 0 })
        }
    }
    fn compare(cases: &[(u8, u32)], memory: bool) {
        const PC: u32 = 0x0200_0000;
        let mut j = Jit::new(false).unwrap();
        for (id, &(route, raw)) in cases.iter().enumerate() {
            let mut compile_bus = Memory::new(true);
            let mem = compile_bus.jit_mem().unwrap();
            let op = |fast, len| BOp { fast, len, careful: false, pair_head: false, glue: false };
            let fast = Fast::Raw { route, raw };
            let ops = [op(fast, 4), op(Fast::Nop, 2)];
            let code = (0..HOT + MAX_PENDING_VISITS + 1)
                .find_map(|_| j.entry::<Memory>(id, &ops, PC, &mem)).expect("bounded publication").0;
            for direct in [false, true] {
                for seed in 0..12u32 {
                    let mut bus = Memory::new(direct);
                    // Compile maps have the direct region; an address outside it exercises
                    // width-preserving helpers and transaction ordering instead.
                    let mut cpu = Cpu::new(PC);
                    let values = [0, 1, 31, 32, 63, 0x8000_0000, u32::MAX, 0x7fff_ffff];
                    for (r, v) in cpu.regs.iter_mut().enumerate() {
                        *v = if memory {
                            let offset = if r % 3 == 0 { 0 } else { ((r as u32 * 61 + seed * 37) & 127) + 0x100 };
                            (if direct { 0x1000u32 } else { 0x4000_0000u32 }).wrapping_add(offset)
                        } else { values[(seed as usize + r) % values.len()] };
                    }
                    cpu.sp = if direct { 0x1400 } else { 0x4000_0400 };
                    cpu.flags = seed & 15;
                    let initial = (cpu.regs, cpu.sp, cpu.flags);
                    for cap in [1, 2] {
                        let mut expected_bus = bus.clone();
                        let original_bus = bus.clone();
                        let mut expected = Cpu::new(PC);
                        expected.regs = initial.0; expected.sp = initial.1; expected.flags = initial.2;
                        cpu.regs = initial.0; cpu.sp = initial.1; cpu.flags = initial.2; cpu.pc = PC;
                        expected.exec_fast(&mut expected_bus, &fast, PC + 4);
                        let mem = bus.jit_mem().unwrap();
                        // Full and bounded copies must both retire this compound operation once.
                        assert_eq!(unsafe { run(code, &mut cpu, &mut bus, &mem, 0, cap) }.0, cap);
                        assert_eq!((cpu.regs, cpu.sp, cpu.flags, cpu.pc),
                            (expected.regs, expected.sp, expected.flags, PC + 4 + 2 * (cap - 1)),
                            "route={route} raw={raw:08x} seed={seed} direct={direct} cap={cap}");
                        assert_eq!(bus.data, expected_bus.data, "route={route} raw={raw:08x}");
                        if !direct { assert_eq!(bus.events, expected_bus.events, "route={route} raw={raw:08x}"); }
                        bus = original_bus;
                    }
                }
            }
        }
    }

    #[test]
    fn raw_pair_arithmetic_and_rev_match_flags_aliases_and_edge_values() {
        let mut cases = Vec::new();
        for p in 0..8u32 {
            for op in 0..4 { for n in [0, 1, 31, 32, 63] {
                cases.push((21, 0xe1d0 | (p << 29) | (op << 26) | ((n & 15) << 16) | ((n >> 4) << 24)));
            }}
            for op in 0..4 { for source in [p * 2, p * 2 + 1, (p * 2 + 5) & 15] {
                cases.push((21, 0xe1d8 | (p << 29) | (op << 16) | (source << 24)));
            }}
            for signed in [0, 1] { for form in [0xf8, 0xfc, 0xfe, 0xf6] {
                for (a, b) in [(p * 2, p * 2 + 1), (p * 2 + 1, p * 2), (2, 7)] {
                    cases.push((21, 0xe100 | form | (p << 29) | (signed << 28) | (a << 20) | (b << 24)));
                }
            }}
        }
        for d in 0..16 { for s in [d, (d + 1) & 15, (d + 7) & 15] {
            cases.push((22, 0xe070 | (d << 28) | (s << 24)));
        }}
        compare(&cases, false);
    }

    #[test]
    fn raw_memory_matches_width_order_and_aliases_on_inline_and_bus_paths() {
        let mut cases = Vec::new();
        for route in [18, 19] { for w in [0xec, 0xed, 0xee] {
            for (d, b) in [(0, 0), (0, 1), (1, 0), (15, 15)] {
                for bits in [0, 1, 2, 3, 4, 5, 6, 7] {
                    for f in [0, 0x2a, 0xff] {
                        cases.push((route, 0xd0 | (w << 8) | bits | ((f & 15) << 16)
                            | ((f >> 4) << 24) | (d << 28) | (b << 20)));
                    }
                }
            }
        }}
        // Pair memory: immediate offset, pre/post increment, indexed and SP-relative.
        for form in [0x50, 0x54, 0x58, 0x5c] { for sub in [0, 1, 2, 3, 14, 15] {
            for (p, b, i) in [(0, 0, 1), (0, 1, 0), (7, 15, 14), (3, 2, 7)] {
                cases.push((16, form | 0xec00 | (sub << 16) | (p << 29) | (b << 20) | (i << 24)));
            }
        }}
        for p in [0, 3, 7] { for offset in [0, 1, 0x7ff] { for store in [0, 1] {
            cases.push((16, 0xe9d0 | (p << 29) | (offset << 17) | (store << 16)));
        }}}
        for form in [0xdc, 0xde] { for w in [0xec, 0xed, 0xee] { for sub in 0..8 {
            for (d, b, i) in [(0, 0, 0), (0, 1, 0), (1, 0, 1), (15, 15, 14)] {
                cases.push((32, form | (w << 8) | (sub << 16) | (d << 28) | (b << 20) | (i << 24)));
            }
        }}}
        compare(&cases, true);
    }
}
