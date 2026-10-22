//! Scalar extraction from a 64-bit register pair, as observed in the five
//! corpus encodings and the Open firmware DSP path. Reserved fields fail closed.
use crate::{IsaEntry, SlotKind, SlotSpec};

pub(super) fn family(raw:u64)->bool { matches!(raw & 0xffff,0xe1de|0xf1de) }
pub(super) fn decode(raw:u64)->Option<&'static IsaEntry> {
    // Pair-low registers are even. Bit26 is absent from every known encoding;
    // do not guess whether it selects rounding or another operation.
    if !family(raw) || raw & ((1<<20)|(1<<26)) != 0 { return None; }
    Some(&ENTRIES[usize::from(raw & (1<<27)!=0)*2+usize::from(raw & (1<<12)!=0)])
}
static SLOTS:&[SlotSpec]=&[
    SlotSpec{kind:SlotKind::Imm,pieces:&[(28,31)],mult:0,adj:0,hi:0,lo:0},
    SlotSpec{kind:SlotKind::Adj,pieces:&[(20,23)],mult:0,adj:-1,hi:0,lo:0},
    SlotSpec{kind:SlotKind::Imm,pieces:&[(20,23)],mult:0,adj:0,hi:0,lo:0},
    SlotSpec{kind:SlotKind::Imm,pieces:&[(16,19),(24,25)],mult:0,adj:0,hi:0,lo:0},
];
macro_rules! entry {($syntax:expr)=>{IsaEntry{name:"pair_extract_craw",syntax:$syntax,len:4,count:0,
    mask:0,match_:0,group:"alu",alt:&[],samples:&[],slots:SLOTS}};}
static ENTRIES:[IsaEntry;4]=[
    entry!("R = R_R >> #i (u)"),entry!("R = R_R >> #i (u) #"),
    entry!("R = R_R >> #i (s)"),entry!("R = R_R >> #i (s) #"),
];
