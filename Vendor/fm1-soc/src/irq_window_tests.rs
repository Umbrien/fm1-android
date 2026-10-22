use crate::{machine::Core1Bus, Soc, RAM_BASE, VEC_BASE, XIP_BASE};
use fm1_core::{Bus, Cpu, JitMem};

/// Independent old-window oracle: leave irq_window_for at its default so every
/// gate change takes the original unfiltered Bus::irq_window path.
struct LegacyWindow<'a, B>(&'a mut B);
impl<B: Bus> Bus for LegacyWindow<'_, B> {
    fn read8(&mut self,a:u32)->u8{self.0.read8(a)}
    fn read16(&mut self,a:u32)->u16{self.0.read16(a)}
    fn read32(&mut self,a:u32)->u32{self.0.read32(a)}
    fn write8(&mut self,a:u32,v:u8){self.0.write8(a,v)}
    fn write16(&mut self,a:u32,v:u16){self.0.write16(a,v)}
    fn write32(&mut self,a:u32,v:u32){self.0.write32(a,v)}
    fn tick(&mut self)->bool{self.0.tick()}
    fn irq_pending(&self)->bool{self.0.irq_pending()}
    fn jit_mem(&mut self)->Option<JitMem>{self.0.jit_mem()}
    fn thread_mem(&mut self)->Option<JitMem>{self.0.thread_mem()}
    fn tick_room(&self)->u64{self.0.tick_room()}
    fn add_ticks(&mut self,n:u64){self.0.add_ticks(n)}
    fn next_tick_due(&self)->bool{self.0.next_tick_due()}
    fn irq_window(&mut self){self.0.irq_window()}
    fn code_writes(&self)->Option<u64>{self.0.code_writes()}
    fn xip_writes(&self)->u64{self.0.xip_writes()}
    fn xip_mark(&mut self,a:u32,b:u32){self.0.xip_mark(a,b)}
    fn code_mark(&mut self,a:u32,b:u32){self.0.code_mark(a,b)}
    fn hw_lock(&mut self,c:u32,s:bool)->bool{self.0.hw_lock(c,s)}
    fn idle(&mut self,c:u32){self.0.idle(c)}
}

fn execute<B:Bus>(cpu:&mut Cpu,bus:&mut B,n:u32)->(u32,bool) {
    match cpu.run_fast(bus,n).unwrap() {
        (0,_)=>{cpu.step(bus).unwrap();(1,false)}
        result=>result,
    }
}

/// Original scheduler observation points, with either the old or new bus hook.
fn run<const LEGACY:bool>(cpu:&mut Cpu,soc:&mut Soc,core:u32,n:u32) {
    let (mut done,mut ticked)=(0,false);
    while done<n {
        if core==0 {
            if ticked || soc.periph.tick(){soc.advance(0);}
            soc.periph.in_isr=!cpu.irq_levels.is_empty();
        }
        if let Some((irq,priority))=soc.pending_irq_for(core) {
            if cpu.irq_ready(priority) {
                let handler=soc.read32(VEC_BASE+4*irq as u32);
                cpu.interrupt(handler,priority);
            }
        }
        let result=if core==0 {
            if LEGACY{execute(cpu,&mut LegacyWindow(soc),n-done)}else{execute(cpu,soc,n-done)}
        }else{
            let mut bus=Core1Bus{soc,budget:(n-done).max(64) as u64+1,observe_irq:false};
            if LEGACY{execute(cpu,&mut LegacyWindow(&mut bus),n-done)}else{execute(cpu,&mut bus,n-done)}
        };
        done+=result.0;ticked=result.1;
    }
}

fn fixture(operation:u8)->Soc {
    let mut image=vec![0;256];
    let prefix:&[u8]=match operation {
        0=>&[0xe1,0x04], // [--sp] = {psr,reti}: open nesting.
        1=>&[0x61,0], // sti: enable the CPU gate.
        _=>&[0x64,0xe0,0x80,0x1b], // icfg = r1: restore the ICFG gate.
    };
    image[..prefix.len()].copy_from_slice(prefix);
    image[prefix.len()..prefix.len()+4].copy_from_slice(&[0xc0,0x21,0x80,0]);
    image[64..68].copy_from_slice(&[0xc4,0x21,0x80,0]);
    let mut soc=Soc::new(512<<10,image);
    soc.write32(VEC_BASE+4*120,XIP_BASE+64);
    soc.advance(0);
    soc
}

#[test]
fn filtered_windows_match_old_windows_for_both_cores_gates_and_priorities() {
    for core in [0,1] {for jit in [false,true] {for operation in 0..3 {
        let mut old=Cpu::new(XIP_BASE);old.core_id=core;old.jit_on=jit;
        old.sp=RAM_BASE+0x2000;old.rets=XIP_BASE;
        let mut new=Cpu::new(XIP_BASE);new.restore_architectural_state(old.architectural_state());new.jit_on=jit;
        if jit {
            let (mut a,mut b)=(fixture(operation),fixture(operation));
            for _ in 0..64 {
                old.pc=XIP_BASE;new.pc=XIP_BASE;
                old.sp=RAM_BASE+0x2000;new.sp=old.sp;
                run::<true>(&mut old,&mut a,core,3);run::<false>(&mut new,&mut b,core,3);
            }
            #[cfg(all(target_arch="aarch64",any(target_os="macos",target_os="android")))]
            assert!(old.jit_insns>0&&new.jit_insns>0);
        }
        for active in [None,Some(0),Some(3),Some(7)] {for nesting in [false,true] {
            for gates in 0..4 {for priority in 0..8 {for enabled in [false,true] {
                let mut a=fixture(operation);let mut b=fixture(operation);
                for cpu in [&mut old,&mut new] {
                    let mut initial=Cpu::new(XIP_BASE);initial.core_id=core;initial.sp=RAM_BASE+0x2000;
                    initial.rets=XIP_BASE;initial.ie=operation!=1&&gates&1!=0;
                    initial.sreg_store[0]=if operation!=2&&gates&2!=0{0x100}else{0};
                    initial.regs[1]=if gates&2!=0{0x100}else{0};
                    initial.irq_levels=active.into_iter().collect();initial.nest_ok=operation!=0&&nesting;
                    cpu.restore_architectural_state(initial.architectural_state());cpu.insn_count=0;
                }
                for soc in [&mut a,&mut b] {
                    // The opposite bank is deliberately enabled at another priority.
                    soc.periph.icfg[15]=15;soc.periph.icfg1[15]=15;
                    if core==0{soc.periph.icfg[15]=(priority as u32*2)|u32::from(enabled);}
                    else{soc.periph.icfg1[15]=(priority as u32*2)|u32::from(enabled);}
                    soc.periph.ilat=1;
                }
                run::<true>(&mut old,&mut a,core,2);run::<false>(&mut new,&mut b,core,2);
                assert!(old.architectural_state()==new.architectural_state(),
                    "core={core} jit={jit} op={operation} active={active:?} nest={nesting} gates={gates} prio={priority} enabled={enabled}");
                assert_eq!(old.insn_count,new.insn_count);
                assert_eq!((a.periph.cycles,a.periph.ilat,a.ram.code_writes,a.periph.audio.clone()),
                    (b.periph.cycles,b.periph.ilat,b.ram.code_writes,b.periph.audio.clone()));
                for address in (RAM_BASE+0x1fc0..RAM_BASE+0x2040).step_by(4) {
                    assert_eq!(a.read32(address),b.read32(address));
                }
                // A newly deliverable nested IRQ runs its handler before r0 += 1.
                let post_ie=operation==1||gates&1!=0;
                let post_mask=gates&2!=0;
                let post_nest=operation==0||nesting;
                let post_ready=post_ie&&post_mask&&active.map_or(true,|p|post_nest&&priority>p);
                if enabled&&post_ready {
                    assert_eq!((new.regs[0],new.regs[4]),(0,1));
                }
            }}}
        }}
    }}}
}

#[test]
fn filtered_windows_preserve_retiring_constructs_and_watched_stack_writes() {
    for core in [0,1] {for jit in [false,true] {for construct in 0..4 {for count in [1,2,4,8] {
        let (mut a,mut b)=(fixture(1),fixture(1));
        let mut old=Cpu::new(XIP_BASE);old.core_id=core;old.jit_on=jit;
        old.sp=RAM_BASE+0x2000;old.rets=XIP_BASE;old.sreg_store[0]=0x100;
        old.irq_levels=vec![1];old.nest_ok=true;
        match construct {
            0=>old.hold_irq=1,
            1=>{old.rep_start=XIP_BASE;old.rep_end=XIP_BASE+2;old.rep_count=1;}
            2=>old.pred_skips=vec![(XIP_BASE+2,XIP_BASE+4)],
            _=>{let mut changed=old.regs;changed[2]=7;old.pair_pending=Some((old.regs,changed));}
        }
        let mut new=Cpu::new(XIP_BASE);new.restore_architectural_state(old.architectural_state());new.jit_on=jit;
        for soc in [&mut a,&mut b] {
            soc.periph.icfg[15]=7;soc.periph.icfg1[15]=7;soc.periph.ilat=1;
        }
        run::<true>(&mut old,&mut a,core,count);run::<false>(&mut new,&mut b,core,count);
        assert!(old.architectural_state()==new.architectural_state(),
            "core={core} jit={jit} construct={construct} count={count}");
        assert_eq!((old.insn_count,a.periph.cycles),(new.insn_count,b.periph.cycles));
        if count>=2{assert!(new.irq_levels.contains(&3));}
    }}}}
    // An ineligible same-priority IRQ must not mask the independent code-write fence.
    for core in [0,1] {for jit in [false,true] {
        let (mut a,mut b)=(fixture(0),fixture(0));
        for soc in [&mut a,&mut b] {
            soc.periph.icfg[15]=3;soc.periph.icfg1[15]=3;soc.periph.ilat=1;
            soc.ram.mark_code(0x1ff8,0x2000);
        }
        let mut old=Cpu::new(XIP_BASE);old.core_id=core;old.jit_on=jit;old.sp=RAM_BASE+0x2000;
        old.rets=XIP_BASE;old.ie=true;old.sreg_store[0]=0x100;old.flags=15;old.irq_levels=vec![1];
        let mut new=Cpu::new(XIP_BASE);new.restore_architectural_state(old.architectural_state());new.jit_on=jit;
        run::<true>(&mut old,&mut a,core,2);run::<false>(&mut new,&mut b,core,2);
        assert!(old.architectural_state()==new.architectural_state());
        assert!(a.ram.code_writes>0);assert_eq!(a.ram.code_writes,b.ram.code_writes);
        assert_eq!(a.read32(RAM_BASE+0x1ffc),b.read32(RAM_BASE+0x1ffc));
        assert_eq!(a.read32(RAM_BASE+0x1ff8),b.read32(RAM_BASE+0x1ff8));
    }}
}

#[test]
fn filtered_window_keeps_existing_due_and_code_write_barriers() {
    let mut soc=fixture(0);soc.periph.ilat=1;soc.periph.icfg[15]=7;soc.periph.icfg1[15]=15;
    for minimum in 0..=8 {
        soc.periph.next_due=100;
        soc.irq_window_for(0,minimum);
        assert_eq!(soc.periph.next_due,if minimum<=3{0}else{100});
        soc.periph.next_due=100;
        soc.irq_window_for(1,minimum);
        assert_eq!(soc.periph.next_due,if minimum<=7{0}else{100});
        soc.periph.next_due=0;soc.irq_window_for(0,minimum);
        assert!(soc.next_tick_due());
        let mut bus=Core1Bus{soc:&mut soc,budget:30,observe_irq:false};
        bus.irq_window_for(0,minimum); // Core1Bus must use its own bank.
        assert_eq!(bus.budget,if minimum<=7{1}else{30});
        bus.budget=1;bus.irq_window_for(0,8);assert!(bus.next_tick_due());
    }
    soc.ram.mark_code(0,4);let epoch=soc.code_writes();
    soc.write32(RAM_BASE,0x1234);let changed=soc.code_writes();assert_ne!(epoch,changed);
    soc.irq_window_for(0,8);assert_eq!(soc.code_writes(),changed);
    // Timed work retains its original deadline when no IRQ is deliverable.
    soc.periph.ilat=0;soc.periph.next_due=soc.periph.cycles+3;
    soc.irq_window_for(0,8);
    assert!(!soc.tick());assert!(!soc.tick());assert!(soc.tick());
}
