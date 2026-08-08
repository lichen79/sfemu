//! A 64 KiB bus for this crate's own tests.
//!
//! Deliberately not the harness's recording bus: that lives in `testrunner`, which
//! depends on this crate, and these tests must run under `cargo test -p z80` with
//! nothing else built.
//!
//! One bus, shared by every test module here. A `Mem` copied into `cpu.rs`,
//! `load.rs` and `flow.rs` separately is how a codebase grows three subtly
//! different buses and a test that passes only in its own file.

use crate::Bus;

/// 64 KiB of RAM, a log of port writes, and one canned port-read value.
pub struct Mem {
    pub ram: [u8; 0x1_0000],
    pub ports_out: Vec<(u16, u8)>,
    pub port_in_value: u8,
}

impl Mem {
    /// Empty RAM.
    #[must_use]
    pub fn new() -> Self {
        Mem {
            ram: [0; 0x1_0000],
            ports_out: Vec::new(),
            port_in_value: 0xFF,
        }
    }

    /// `prog` loaded at `pc`, which is where a test's `Z80::pc` should point.
    #[must_use]
    pub fn at(pc: u16, prog: &[u8]) -> Self {
        let mut m = Mem::new();
        for (i, b) in prog.iter().enumerate() {
            m.ram[usize::from(pc) + i] = *b;
        }
        m
    }
}

impl Default for Mem {
    fn default() -> Self {
        Self::new()
    }
}

impl Bus for Mem {
    fn read(&mut self, addr: u16) -> u8 {
        self.ram[usize::from(addr)]
    }
    fn write(&mut self, addr: u16, val: u8) {
        self.ram[usize::from(addr)] = val;
    }
    fn port_in(&mut self, _port: u16) -> u8 {
        self.port_in_value
    }
    fn port_out(&mut self, port: u16, val: u8) {
        self.ports_out.push((port, val));
    }
}
