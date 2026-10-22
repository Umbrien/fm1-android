//! Canonical special-register move fields. The corpus-fitted table misses
//! register combinations and permutes SP source bits for r2..r5; use the
//! architectural nibble layout for the whole supported family.

/// `64 e0 00 XY` rX = Y and `64 e0 80 XY` Y = rX, Y a special register: 0 reti, 1 rete,
/// 3 rets, 5 psr, 6 cnum, b icfg, c usp, d ssp, e sp (the corpora's 40 register / sreg pairs
/// all fit; Y = 2, 4, 7..a and f never appear, and cnum is never written).
pub(super) fn decode(win: u64) -> Option<&'static crate::IsaEntry> {
    if win & 0xffff != 0xe064 {
        return None;
    }
    let write = match (win >> 16) & 0xff { 0x00 => false, 0x80 => true, _ => return None };
    let i = match (win >> 24) & 0xf { 0 => 0, 1 => 1, 3 => 2, 5 => 3, 6 if !write => 4, 0xb => 5, 0xc => 6, 0xd => 7, 0xe => 8, _ => return None };
    Some(if write { &SREG_WRITE_CRAW[i] } else { &SREG_READ_CRAW[i] })
}
const SREG_SLOTS: &[crate::SlotSpec] =
    &[crate::SlotSpec { kind: crate::SlotKind::Imm, pieces: &[(28, 31)], mult: 0, adj: 0, hi: 0, lo: 0 }];
macro_rules! sreg_entries {
    ($($syntax:expr),*) => {
        [$(crate::IsaEntry {
            name: "sreg_mov_craw", syntax: $syntax, len: 4, count: 0, mask: 0, match_: 0, group: "misc",
            alt: &[], samples: &[], slots: SREG_SLOTS,
        }),*]
    };
}
static SREG_READ_CRAW: [crate::IsaEntry; 9] = sreg_entries!(
    "R = reti", "R = rete", "R = rets", "R = psr", "R = cnum", "R = icfg", "R = usp", "R = ssp", "R = sp");
static SREG_WRITE_CRAW: [crate::IsaEntry; 9] = sreg_entries!(
    "reti = R", "rete = R", "rets = R", "psr = R", "cnum = R", "icfg = R", "usp = R", "ssp = R", "sp = R");
