//! Signed saturated scalar arithmetic with natural register fields. The
//! solver table overfits register bits in the e439 add family. Other
//! control nibbles are deliberately not interpreted as ordinary add/sub.
use crate::{IsaEntry, SlotKind, SlotSpec};

pub(super) fn family(raw: u64) -> bool { matches!(raw & 0xffff, 0xe439 | 0xf439) }
pub(super) fn decode(raw: u64) -> Option<&'static IsaEntry> {
    if !family(raw) || !matches!((raw >> 16) & 15, 0 | 8) { return None; }
    Some(&ENTRIES[usize::from(raw & (1 << 19) != 0) * 2 + usize::from(raw & (1 << 12) != 0)])
}
static SLOTS: &[SlotSpec] = &[
    SlotSpec { kind: SlotKind::Imm, pieces: &[(28,31)], mult: 0, adj: 0, hi: 0, lo: 0 },
    SlotSpec { kind: SlotKind::Imm, pieces: &[(20,23)], mult: 0, adj: 0, hi: 0, lo: 0 },
    SlotSpec { kind: SlotKind::Imm, pieces: &[(24,27)], mult: 0, adj: 0, hi: 0, lo: 0 },
];
macro_rules! entry { ($syntax:expr) => { IsaEntry { name: "saturating_craw", syntax: $syntax,
    len: 4, count: 0, mask: 0, match_: 0, group: "alu", alt: &[], samples: &[], slots: SLOTS } }; }
static ENTRIES: [IsaEntry;4] = [
    entry!("R = R + R (ssat)"), entry!("R = R + R (ssat) #"),
    entry!("R = R - R (ssat)"), entry!("R = R - R (ssat) #"),
];
