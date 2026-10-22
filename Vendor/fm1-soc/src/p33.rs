//! P33 serial register bridge at CON=0x13e08, DAT=0x13e0c.
//!
//! The SDK/HAL P33 access sequence selects CON bit0 (bit8 chooses RTC),
//! clocks command, address and data by strobing bit4, and polls busy bit1.
//! Commands encode read in bit7, write/or/and/xor in bits5..6, and the
//! address's upper two bits in bits0..1. Transfers complete synchronously.
//! This models the register transport/readback, not analog supply physics or
//! watchdog/reset timing. Both CPUs share the same serial transaction.
//! Evidence: firmware/hal/fm1_sys.h and fm1_power.h in the imported Melodee HAL.

pub(crate) struct P33 {
    con: u32,
    data: u8,
    registers: [[u8; 1024]; 2],
    phase: u8,
    command: u8,
    address: usize,
}

impl Default for P33 {
    fn default() -> Self {
        Self { con: 0, data: 0, registers: [[0; 1024]; 2], phase: 0, command: 0, address: 0 }
    }
}

impl P33 {
    pub fn read(&self, addr: u32) -> u32 {
        match addr { 0x13e08 => self.con, 0x13e0c => self.data as u32, _ => 0 }
    }

    pub fn write(&mut self, addr: u32, value: u32) {
        match addr {
            0x13e0c => self.data = value as u8,
            0x13e08 => {
                // A deselection or domain change cancels an incomplete command.
                if value & 1 == 0 || (self.con ^ value) & 0x101 != 0 { self.phase = 0; }
                self.con = value & !0x12; // transfer strobe self-clears; no busy latency
                if value & 0x11 == 0x11 { self.transfer(); }
            }
            _ => {}
        }
    }

    fn transfer(&mut self) {
        match self.phase {
            0 => { self.command = self.data; self.address = (self.command as usize & 3) << 8; self.data = 0; }
            1 => { self.address |= self.data as usize; self.data = 0; }
            2 => {
                let register = &mut self.registers[usize::from(self.con & 0x100 != 0)][self.address];
                if self.command & 0x80 == 0 {
                    *register = match (self.command >> 5) & 3 {
                        0 => self.data, 1 => *register | self.data,
                        2 => *register & self.data, _ => *register ^ self.data,
                    };
                }
                self.data = *register;
            }
            _ => {} // A new command requires a new chip-select window.
        }
        self.phase = self.phase.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn transfer(p: &mut P33, data: u8) -> u8 {
        p.write(0x13e0c, data as u32);
        p.write(0x13e08, p.read(0x13e08) | 0x10);
        assert_eq!(p.read(0x13e08) & 0x12, 0);
        p.read(0x13e0c) as u8
    }
    fn command(p: &mut P33, bank: u32, op: u8, address: u16, value: u8) -> u8 {
        p.write(0x13e08, 1 | bank << 8);
        transfer(p, op | (address >> 8) as u8);
        transfer(p, address as u8);
        let result = transfer(p, value);
        p.write(0x13e08, 0);
        result
    }
    #[test]
    fn readback_and_all_operations_cover_both_domains_and_address_bits() {
        let mut p = P33::default();
        for bank in 0..2 { for address in [0, 5, 9, 0x80, 0xa0, 0x100, 0x2ff, 0x3ff] {
            assert_eq!(command(&mut p, bank, 0x80, address, 0x5e), 0);
            command(&mut p, bank, 0, address, 0x34);
            command(&mut p, bank, 0x20, address, 0x81);
            command(&mut p, bank, 0x40, address, 0xf7);
            command(&mut p, bank, 0x60, address, 0x15);
            assert_eq!(command(&mut p, bank, 0x80, address, 0x5e), 0xa0);
        }}
        command(&mut p, 0, 0, 9, 0x11);
        assert_eq!(command(&mut p, 1, 0x80, 9, 0), 0xa0);
        assert_eq!(command(&mut p, 0, 0x80, 9, 0), 0x11);
    }
    #[test]
    fn ordering_deselection_shared_transaction_and_reset_are_exact() {
        let mut p = P33::default();
        transfer(&mut p, 0xff); // Unselected clock cannot begin a command.
        p.write(0x13e08, 1);
        transfer(&mut p, 0); transfer(&mut p, 7);
        p.write(0x13e08, 0); // An aborted write never changes the register.
        assert_eq!(command(&mut p, 0, 0x80, 7, 0), 0);
        p.write(0x13e08, 1); transfer(&mut p, 0); transfer(&mut p, 7);
        // A second CPU's DAT write is the data of this shared transaction.
        transfer(&mut p, 0x69); p.write(0x13e08, 0);
        assert_eq!(command(&mut p, 0, 0x80, 7, 0), 0x69);
        p.write(0x13e08, 1); transfer(&mut p, 0);
        p.write(0x13e08, 0x101); // Switching domains starts a fresh command.
        transfer(&mut p, 0); transfer(&mut p, 8); transfer(&mut p, 0x77); p.write(0x13e08, 0);
        assert_eq!(command(&mut p, 1, 0x80, 8, 0), 0x77);
        let mut fresh = P33::default();
        assert_eq!((fresh.read(0x13e08), fresh.read(0x13e0c)), (0, 0));
        assert_eq!(command(&mut fresh, 0, 0x80, 7, 0), 0);
    }
}
