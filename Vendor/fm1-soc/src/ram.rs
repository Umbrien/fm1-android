//! Plain little-endian RAM region at address 0.

pub struct Ram {
    data: Vec<u8>,
    /// Per 4-byte word: holds code the CPU has built blocks from (`mark_code`). The wasm JIT's
    /// inline stores read it too (`code_ptr`): a store to such a word takes the bus instead.
    /// (Words, not pages: the stock firmware's cpu1 keeps data next to its RAM code, and with
    /// 256-byte pages its stores all took the bus and left the compiled code: 2.7x slower.)
    code: Vec<u8>,
    /// Changes to code words so far: while it stays the same, no block built from RAM can have
    /// changed (the CPU skips comparing their bytes with memory).
    pub code_writes: u64,
}

impl Ram {
    pub(crate) fn proof_word(&self, offset: u32) -> Option<u32> {
        let a = offset as usize;
        self.data.get(a..a.checked_add(4)?).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }

    /// A proof may roll back data writes only. Code and its epoch are immutable
    /// during the trial, so decoded/JIT cache dependencies remain valid.
    pub(crate) fn proof_data_word(&self, offset: u32) -> bool {
        self.code.get(offset as usize >> 2) == Some(&0)
    }

    pub(crate) fn restore_proof_word(&mut self, offset: u32, value: u32) {
        let a = offset as usize;
        self.data[a..a+4].copy_from_slice(&value.to_le_bytes());
    }

    pub fn new(size: usize) -> Self {
        Self {
            data: vec![0; size],
            code: vec![0; size / 4 + 1],
            code_writes: 0,
        }
    }

    /// Bytes lo..hi hold code: their words are watched from now on. (From lo - 3: a store that
    /// reaches into a code word starts at most 3 bytes before it, and stores check the word they
    /// start in.)
    pub fn mark_code(&mut self, lo: u32, hi: u32) {
        let last = (hi.max(lo + 1) - 1) as usize >> 2;
        for w in (lo.saturating_sub(3) as usize >> 2)..=last.min(self.code.len() - 1) {
            self.code[w] = 1;
        }
    }

    /// Host pointer to the code-word flags (the wasm JIT's inline stores).
    pub fn code_ptr(&self) -> *const u8 {
        self.code.as_ptr()
    }

    #[inline(always)]
    fn wrote<const N: usize>(&mut self, a: usize, bytes: &[u8; N]) {
        // Ordinary data writes keep their original single-store path. A watched
        // word only advances the guard epoch if this store changes memory.
        if self.code[a >> 2] != 0 && self.data[a..a + N] != bytes[..] {
            self.code_writes += 1;
        }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Host pointer to the bytes (the JIT's inline loads and stores).
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.data.as_mut_ptr()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    #[inline(always)]
    pub fn read8(&self, addr: u32) -> u8 {
        self.data[addr as usize]
    }

    #[inline(always)]
    pub fn read16(&self, addr: u32) -> u16 {
        let a = addr as usize;
        u16::from_le_bytes([self.data[a], self.data[a + 1]])
    }

    #[inline(always)]
    pub fn read32(&self, addr: u32) -> u32 {
        let a = addr as usize;
        u32::from_le_bytes([
            self.data[a],
            self.data[a + 1],
            self.data[a + 2],
            self.data[a + 3],
        ])
    }

    #[inline(always)]
    pub fn write8(&mut self, addr: u32, value: u8) {
        self.wrote(addr as usize, &[value]);
        self.data[addr as usize] = value;
    }

    #[inline(always)]
    pub fn write16(&mut self, addr: u32, value: u16) {
        let a = addr as usize;
        let bytes = value.to_le_bytes();
        self.wrote(a, &bytes);
        self.data[a..a + 2].copy_from_slice(&bytes);
    }

    #[inline(always)]
    pub fn write32(&mut self, addr: u32, value: u32) {
        let a = addr as usize;
        let bytes = value.to_le_bytes();
        self.wrote(a, &bytes);
        self.data[a..a + 4].copy_from_slice(&bytes);
    }

    /// A word starting in this region whose tail may cross its end. Ordinary
    /// full-word accesses retain their existing fast path; absent tail bytes read zero.
    pub(crate) fn read32_bounded(&self, addr: u32) -> u32 {
        let offset=addr as usize;
        if offset.checked_add(4).is_some_and(|end|end<=self.data.len()) { return self.read32(addr); }
        let mut bytes=[0;4];
        for (destination,source) in bytes.iter_mut().zip(self.data.get(offset..).unwrap_or(&[])) {
            *destination=*source;
        }
        u32::from_le_bytes(bytes)
    }

    /// Clip a trailing partial word to mapped bytes, preserving watched-code
    /// epochs through the ordinary byte-store operation.
    pub(crate) fn write32_bounded(&mut self, addr: u32, value: u32) {
        let offset=addr as usize;
        if offset.checked_add(4).is_some_and(|end|end<=self.data.len()) { return self.write32(addr,value); }
        for (index,byte) in value.to_le_bytes().into_iter().enumerate() {
            let Some(address)=offset.checked_add(index).filter(|&a|a<self.data.len()) else { break; };
            self.write8(address as u32,byte);
        }
    }

    /// Load raw bytes at an offset (firmware loading).
    pub fn load(&mut self, offset: u32, bytes: &[u8]) {
        let o = offset as usize;
        self.code_writes += 1;
        self.data[o..o + bytes.len()].copy_from_slice(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_widths() {
        let mut ram = Ram::new(64);
        ram.write32(0, 0xDEAD_BEEF);
        assert_eq!(ram.read32(0), 0xDEAD_BEEF);
        assert_eq!(ram.read16(0), 0xBEEF);
        assert_eq!(ram.read8(1), 0xBE);
        ram.write16(8, 0x1234);
        assert_eq!(ram.read8(8), 0x34);
        assert_eq!(ram.read8(9), 0x12);
    }

    #[test]
    fn identical_watched_stores_preserve_code_epoch_at_every_width_and_alignment() {
        for width in [1, 2, 4] {
            for addr in 13..24 {
                let mut ram = Ram::new(64);
                ram.load(0, &(0..64u8).collect::<Vec<_>>());
                ram.mark_code(16, 24);
                let original = match width { 1 => ram.read8(addr) as u32,
                    2 => ram.read16(addr) as u32, _ => ram.read32(addr) };
                let store = |ram: &mut Ram, value| match width {
                    1 => ram.write8(addr, value as u8),
                    2 => ram.write16(addr, value as u16),
                    _ => ram.write32(addr, value),
                };
                let before = ram.code_writes;
                store(&mut ram, original);
                assert_eq!(ram.code_writes, before, "width={width} addr={addr}");
                store(&mut ram, original ^ 1);
                assert_eq!(ram.code_writes, before + 1, "width={width} addr={addr}");
                store(&mut ram, original ^ 1);
                assert_eq!(ram.code_writes, before + 1);
                // Bulk loading remains an explicit cache-guard barrier even for equal bytes.
                let bytes = ram.data[addr as usize..addr as usize + width].to_vec();
                ram.load(addr, &bytes);
                assert_eq!(ram.code_writes, before + 2);
            }
        }
    }

    #[test]
    fn code_tracking_catches_unaligned_overlapping_stores_and_bulk_loads() {
        let mut ram = Ram::new(64);
        ram.mark_code(16, 24);
        let mut version = ram.code_writes;
        ram.write32(0, 1);
        assert_eq!(ram.code_writes, version, "unrelated data is not code");
        ram.write32(13, 2); // begins in preceding word, overlaps byte 16
        version += 1;
        assert_eq!(ram.code_writes, version);
        ram.write16(15, 3);
        version += 1;
        assert_eq!(ram.code_writes, version);
        ram.write8(23, 4);
        version += 1;
        assert_eq!(ram.code_writes, version);
        ram.load(16, &[1, 2, 3, 4]);
        version += 1;
        assert_eq!(ram.code_writes, version);
    }
}
