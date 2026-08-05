//! A `Bus` that serves a test case's seeded RAM and records every access.

use crate::binfmt::State;
use m68k::Bus;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Size {
    Byte,
    Word,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Access {
    pub is_write: bool,
    pub addr: u32,
    pub size: Size,
    pub val: u16,
}

/// Flat 24-bit memory, sparse, with an access log.
///
/// Unseeded addresses read as 0 and are recorded: the suite seeds everything an
/// instruction legitimately touches, so a read of unseeded memory is itself a
/// signal that the core went somewhere it should not have.
pub struct TestBus {
    pub mem: HashMap<u32, u8>,
    pub log: Vec<Access>,
    pub unseeded_reads: Vec<u32>,
}

impl TestBus {
    pub fn from_state(st: &State) -> Self {
        Self {
            mem: st.ram.iter().copied().collect(),
            log: Vec::new(),
            unseeded_reads: Vec::new(),
        }
    }

    fn raw_read(&mut self, addr: u32) -> u8 {
        match self.mem.get(&addr) {
            Some(v) => *v,
            None => {
                self.unseeded_reads.push(addr);
                0
            }
        }
    }
}

impl Bus for TestBus {
    fn read8(&mut self, addr: u32) -> u8 {
        let a = addr & 0x00FF_FFFF;
        let v = self.raw_read(a);
        self.log.push(Access {
            is_write: false,
            addr: a,
            size: Size::Byte,
            val: v as u16,
        });
        v
    }

    fn read16(&mut self, addr: u32) -> u16 {
        let a = addr & 0x00FF_FFFF;
        let hi = self.raw_read(a);
        let lo = self.raw_read(a.wrapping_add(1) & 0x00FF_FFFF);
        let v = u16::from_be_bytes([hi, lo]);
        self.log.push(Access {
            is_write: false,
            addr: a,
            size: Size::Word,
            val: v,
        });
        v
    }

    fn write8(&mut self, addr: u32, val: u8) {
        let a = addr & 0x00FF_FFFF;
        self.mem.insert(a, val);
        self.log.push(Access {
            is_write: true,
            addr: a,
            size: Size::Byte,
            val: val as u16,
        });
    }

    fn write16(&mut self, addr: u32, val: u16) {
        let a = addr & 0x00FF_FFFF;
        let [hi, lo] = val.to_be_bytes();
        self.mem.insert(a, hi);
        self.mem.insert(a.wrapping_add(1) & 0x00FF_FFFF, lo);
        self.log.push(Access {
            is_write: true,
            addr: a,
            size: Size::Word,
            val,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(ram: &[(u32, u8)]) -> State {
        State {
            ram: ram.to_vec(),
            ..Default::default()
        }
    }

    #[test]
    fn reads_seeded_memory_big_endian() {
        let mut b = TestBus::from_state(&state_with(&[(0x100, 0xCA), (0x101, 0xFE)]));
        assert_eq!(b.read16(0x100), 0xCAFE);
        assert_eq!(b.log.len(), 1);
        assert_eq!(b.log[0].size, Size::Word);
        assert!(b.unseeded_reads.is_empty());
    }

    #[test]
    fn writes_are_visible_and_logged() {
        let mut b = TestBus::from_state(&state_with(&[]));
        b.write16(0x200, 0xBEEF);
        assert_eq!(b.read16(0x200), 0xBEEF);
        assert_eq!(
            b.log[0],
            Access {
                is_write: true,
                addr: 0x200,
                size: Size::Word,
                val: 0xBEEF
            }
        );
    }

    #[test]
    fn records_unseeded_reads() {
        let mut b = TestBus::from_state(&state_with(&[]));
        b.read8(0x1234);
        assert_eq!(b.unseeded_reads, vec![0x1234]);
    }

    #[test]
    fn masks_addresses_to_24_bits() {
        let mut b = TestBus::from_state(&state_with(&[(0x00FF_FFFF, 0x42)]));
        assert_eq!(b.read8(0xFFFF_FFFF), 0x42);
    }
}
