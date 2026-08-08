//! A `z80::Bus` that remembers what happened.
//!
//! One 64 KiB array plus a log. The log is what makes the per-T-state comparison
//! possible: the vectors record every bus sample, so the harness has to know
//! every access the core made, in order.

use crate::z80fmt::{Port, RamEntry};
use z80::Bus;

/// One memory access, as the core made it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Access {
    Read { addr: u16, val: u8 },
    Write { addr: u16, val: u8 },
}

/// 64 KiB of RAM, a memory log, and a port log.
pub struct TraceBus {
    /// The whole address space. Boxed because 64 KiB on the stack is a stack
    /// overflow waiting for a deeper call tree.
    pub ram: Box<[u8; 0x1_0000]>,
    pub log: Vec<Access>,
    pub ports: Vec<Port>,
    /// Values the case says the devices return, consumed in order by `port_in`.
    pending_in: std::collections::VecDeque<u8>,
}

impl TraceBus {
    /// A bus with `seed` written into RAM and nothing logged.
    #[must_use]
    pub fn new(seed: &[RamEntry]) -> Self {
        let mut b = Self {
            ram: Box::new([0; 0x1_0000]),
            log: Vec::new(),
            ports: Vec::new(),
            pending_in: std::collections::VecDeque::new(),
        };
        for e in seed {
            b.ram[usize::from(e.addr)] = e.val;
        }
        b
    }

    /// Queues the values the case's `IN` transactions returned.
    ///
    /// The suite records what the device gave the CPU, and a harness that
    /// invented a value instead would fail every `IN` case for the wrong reason.
    /// Only `in` transactions are queued; `out` values come from the core.
    pub fn feed_ports(&mut self, ports: &[Port]) {
        for p in ports.iter().filter(|p| !p.out) {
            self.pending_in.push_back(p.val);
        }
    }
}

impl Bus for TraceBus {
    fn read(&mut self, addr: u16) -> u8 {
        let val = self.ram[usize::from(addr)];
        self.log.push(Access::Read { addr, val });
        val
    }

    fn write(&mut self, addr: u16, val: u8) {
        self.ram[usize::from(addr)] = val;
        self.log.push(Access::Write { addr, val });
    }

    fn port_in(&mut self, port: u16) -> u8 {
        // 0xFF when the case declared no value: an undeclared `IN` is a failure
        // for the comparison to report, not a panic to debug.
        let val = self.pending_in.pop_front().unwrap_or(0xFF);
        self.ports.push(Port {
            addr: port,
            val,
            out: false,
        });
        val
    }

    fn port_out(&mut self, port: u16, val: u8) {
        self.ports.push(Port {
            addr: port,
            val,
            out: true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seeded RAM is readable, and a read is recorded with the byte it returned.
    #[test]
    fn a_read_returns_the_seeded_byte_and_is_recorded() {
        let mut b = TraceBus::new(&[RamEntry {
            addr: 0x1234,
            val: 0xED,
        }]);
        assert_eq!(b.read(0x1234), 0xED);
        assert_eq!(b.read(0x1235), 0, "unseeded RAM reads as zero");
        assert_eq!(
            b.log,
            vec![
                Access::Read {
                    addr: 0x1234,
                    val: 0xED
                },
                Access::Read {
                    addr: 0x1235,
                    val: 0
                },
            ]
        );
    }

    /// A write lands in RAM and is recorded.
    #[test]
    fn a_write_lands_and_is_recorded() {
        let mut b = TraceBus::new(&[]);
        b.write(0x8000, 0x42);
        assert_eq!(b.ram[0x8000], 0x42);
        assert_eq!(
            b.log,
            vec![Access::Write {
                addr: 0x8000,
                val: 0x42
            }]
        );
        assert_eq!(b.read(0x8000), 0x42, "and is visible to a later read");
    }

    /// A port access does **not** touch memory.
    ///
    /// The whole reason `Bus` has four methods: a core that routed ports through
    /// `read`/`write` would corrupt RAM at the port address, and this is the test
    /// that would catch it.
    #[test]
    fn a_port_access_is_not_a_memory_access() {
        let mut b = TraceBus::new(&[RamEntry {
            addr: 0xBEEF,
            val: 0x11,
        }]);
        b.port_out(0xBEEF, 0x99);
        assert_eq!(
            b.ram[0xBEEF], 0x11,
            "memory at the port address is untouched"
        );
        assert_eq!(
            b.ports,
            vec![Port {
                addr: 0xBEEF,
                val: 0x99,
                out: true
            }]
        );
        assert!(b.log.is_empty(), "and no memory access was logged");
    }

    /// `port_in` replays the value the case recorded.
    ///
    /// The suite's `ports` array records the value the *device* returned, so the
    /// harness replays that rather than inventing one. See [`TraceBus::feed_ports`].
    #[test]
    fn port_in_replays_the_cases_recorded_value() {
        let mut b = TraceBus::new(&[]);
        b.feed_ports(&[Port {
            addr: 0x00FE,
            val: 0x5A,
            out: false,
        }]);
        assert_eq!(b.port_in(0x00FE), 0x5A);
        assert_eq!(
            b.ports,
            vec![Port {
                addr: 0x00FE,
                val: 0x5A,
                out: false
            }]
        );
    }

    /// Only `IN` values are queued, and they are replayed in order.
    ///
    /// A `feed_ports` that queued the `OUT` values too would hand the core the
    /// wrong byte on any case that writes a port before reading one — and the
    /// single-`IN` test above cannot see that, because with one entry every
    /// filtering bug produces the same queue.
    #[test]
    fn feed_ports_queues_only_the_reads_and_keeps_their_order() {
        let mut b = TraceBus::new(&[]);
        b.feed_ports(&[
            Port {
                addr: 0x10,
                val: 0xAA,
                out: true,
            },
            Port {
                addr: 0x20,
                val: 0x01,
                out: false,
            },
            Port {
                addr: 0x30,
                val: 0x02,
                out: false,
            },
        ]);
        assert_eq!(b.port_in(0x20), 0x01, "the first IN value, not the OUT's");
        assert_eq!(b.port_in(0x30), 0x02, "then the second, in order");
        assert_eq!(b.port_in(0x40), 0xFF, "and then the queue is empty");
    }

    /// An unexpected `IN` returns 0xFF and is still recorded.
    ///
    /// A core that read a port the case did not declare must produce a *visible*
    /// failure at step 5, not a panic here — the diff naming the extra transaction
    /// is far more useful than a backtrace inside the bus.
    #[test]
    fn an_undeclared_port_read_is_recorded_rather_than_fatal() {
        let mut b = TraceBus::new(&[]);
        assert_eq!(
            b.port_in(0x1234),
            0xFF,
            "the floating bus reads as all ones"
        );
        assert_eq!(b.ports.len(), 1, "and the surprise is on the record");
    }
}
