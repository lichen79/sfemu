//! Runs one Z80 vector case and says exactly what went wrong.
//!
//! The comparison has five steps and they run in a fixed order — registers, RAM,
//! T-state total, bus trace, ports — because the first difference is almost always
//! the informative one and a reader should not have to hunt past a hundred trace
//! mismatches caused by one wrong register.
//!
//! Failures are strings. They are read once by a person and never matched on, so
//! a struct would be ceremony; this is what the 68000 harness settled on.

use crate::z80bus::{Access, TraceBus};
use crate::z80fmt::{self, Case, Cycle, Port, State};
use std::path::Path;
use z80::Z80;

/// Loads a state into a fresh CPU.
fn load(s: &State) -> Z80 {
    Z80 {
        a: s.a,
        f: s.f,
        b: s.b,
        c: s.c,
        d: s.d,
        e: s.e,
        h: s.h,
        l: s.l,
        i: s.i,
        r: s.r,
        ix: s.ix,
        iy: s.iy,
        sp: s.sp,
        pc: s.pc,
        wz: s.wz,
        af_: s.af_,
        bc_: s.bc_,
        de_: s.de_,
        hl_: s.hl_,
        iff1: s.iff1,
        iff2: s.iff2,
        im: s.im,
        ei: s.ei,
        q: s.q,
        p: s.p,
        // Not a suite field: no case begins halted, so `false` is the only value
        // consistent with the vectors. See `Z80::halted`.
        halted: false,
    }
}

/// Reads a CPU back out as a state, so one comparison covers both directions.
fn unload(c: &Z80) -> State {
    State {
        a: c.a,
        f: c.f,
        b: c.b,
        c: c.c,
        d: c.d,
        e: c.e,
        h: c.h,
        l: c.l,
        i: c.i,
        r: c.r,
        ix: c.ix,
        iy: c.iy,
        sp: c.sp,
        pc: c.pc,
        wz: c.wz,
        af_: c.af_,
        bc_: c.bc_,
        de_: c.de_,
        hl_: c.hl_,
        iff1: c.iff1,
        iff2: c.iff2,
        im: c.im,
        ei: c.ei,
        q: c.q,
        p: c.p,
        ram: Vec::new(),
    }
}

/// Names every field that differs, with both values in hex.
///
/// All 25 register fields, individually. `ram` is [`diff_ram`]'s job because it
/// needs the bus, not the CPU.
#[must_use]
pub fn diff_state(want: &State, got: &State) -> Vec<String> {
    let mut d = Vec::new();
    macro_rules! w16 {
        ($($f:ident),*) => { $(
            if want.$f != got.$f {
                d.push(format!("{} want {:04X} got {:04X}", stringify!($f), want.$f, got.$f));
            }
        )* };
    }
    macro_rules! w8 {
        ($($f:ident),*) => { $(
            if want.$f != got.$f {
                d.push(format!("{} want {:02X} got {:02X}", stringify!($f), want.$f, got.$f));
            }
        )* };
    }
    macro_rules! wb {
        ($($f:ident),*) => { $(
            if want.$f != got.$f {
                d.push(format!("{} want {} got {}", stringify!($f), want.$f, got.$f));
            }
        )* };
    }
    w16!(pc, sp, ix, iy, wz, af_, bc_, de_, hl_);
    w8!(a, b, c, d, e, f, h, l, i, r, ei, im, p, q);
    wb!(iff1, iff2);
    d
}

/// Compares RAM at every address the case declares, and nowhere else.
///
/// Only the declared addresses: the case's `final.ram` is what upstream
/// guarantees, and a harness that compared all 64 KiB would flag the scratch a
/// generator happened not to mention.
#[must_use]
pub fn diff_ram(want: &State, bus: &TraceBus) -> Vec<String> {
    want.ram
        .iter()
        .filter(|e| bus.ram[usize::from(e.addr)] != e.val)
        .map(|e| {
            format!(
                "ram[{:04X}] want {:02X} got {:02X}",
                e.addr,
                e.val,
                bus.ram[usize::from(e.addr)]
            )
        })
        .collect()
}

/// Compares the bus trace against the accesses the core made.
///
/// The vectors sample the bus **between** T-states, so a memory access spans two
/// samples: the request, which asserts `mreq` with `rd` or `wr`, and — for a read
/// — the byte, which lands on the *immediately following* sample with no pins set
/// at all. A write carries its byte on the request sample itself. Verified over
/// 160,400 cases and 556,052 memory samples from all 1,604 files: every `mreq`+`rd`
/// sample is followed by a pin-free data-bearing one, every `mreq`+`wr` sample
/// carries data, and `rd`/`wr` and `mreq`/`ioreq` never pair up.
///
/// So this walks the *request* samples only, and takes a read's expected byte from
/// the next sample by position. It deliberately does not walk every data-bearing
/// sample looking for reads: `IN r,(C)` ends with a bare data sample of its own,
/// and a walker that consumed the log there would absorb a surplus memory access
/// instead of reporting it. Measured: that variant lets a spurious extra access
/// through on `ED 40`, `ED 58` and their siblings.
#[must_use]
pub fn diff_trace(want: &[Cycle], log: &[Access]) -> Vec<String> {
    let mut d = Vec::new();
    let mut at = 0usize;
    for (i, cy) in want.iter().enumerate() {
        // Only `mreq` requests: an `ioreq` sample is a port transaction, compared
        // by `diff_ports`, and a pin-free sample is either a read's byte (checked
        // with its request, below) or an internal cycle with nothing to check.
        if !cy.mreq || !(cy.rd || cy.wr) {
            continue;
        }
        let Some(access) = log.get(at) else {
            d.push(format!(
                "t{i}: expected an access at {:04X}, core made none",
                cy.addr
            ));
            return d;
        };
        at += 1;
        match access {
            Access::Read { addr, val } if cy.rd => {
                if *addr != cy.addr {
                    d.push(format!("t{i}: read want {:04X} got {addr:04X}", cy.addr));
                }
                // The byte is on the next sample. `data` there is the one place a
                // read's value is verifiable at all.
                if let Some(w) = want.get(i + 1).and_then(|n| n.data) {
                    if *val != w {
                        d.push(format!("t{}: read data want {w:02X} got {val:02X}", i + 1));
                    }
                }
            }
            Access::Write { addr, val } if cy.wr => {
                if *addr != cy.addr {
                    d.push(format!("t{i}: write want {:04X} got {addr:04X}", cy.addr));
                }
                if let Some(w) = cy.data {
                    if *val != w {
                        d.push(format!("t{i}: write data want {w:02X} got {val:02X}"));
                    }
                }
            }
            other => d.push(format!(
                "t{i}: want {} at {:04X}, core did {other:?}",
                if cy.rd { "read" } else { "write" },
                cy.addr
            )),
        }
        if d.len() > 8 {
            d.push("...".to_string());
            return d;
        }
    }
    if at < log.len() {
        d.push(format!(
            "core made {} accesses, the trace accounts for {at}: extra {:?}",
            log.len(),
            &log[at..log.len().min(at + 3)]
        ));
    }
    d
}

/// Compares port transactions in order, with direction.
#[must_use]
pub fn diff_ports(want: &[Port], got: &[Port]) -> Vec<String> {
    let mut d = Vec::new();
    if want.len() != got.len() {
        d.push(format!(
            "port count: want {} ports, got {}",
            want.len(),
            got.len()
        ));
    }
    for (i, (w, g)) in want.iter().zip(got).enumerate() {
        if w != g {
            d.push(format!(
                "port {i}: want {} {:04X}={:02X}, got {} {:04X}={:02X}",
                if w.out { "out" } else { "in" },
                w.addr,
                w.val,
                if g.out { "out" } else { "in" },
                g.addr,
                g.val
            ));
        }
    }
    d
}

/// Runs one case.
///
/// # Errors
///
/// A readable diff naming every difference, in the fixed order this module's docs
/// describe.
pub fn run_case(case: &Case, index: usize) -> Result<(), String> {
    let mut cpu = load(&case.initial);
    let mut bus = TraceBus::new(&case.initial.ram);
    bus.feed_ports(&case.ports);

    let taken = cpu.step(&mut bus);

    let mut d = diff_state(&case.final_, &unload(&cpu));
    d.extend(diff_ram(&case.final_, &bus));
    if taken as usize != case.cycles.len() {
        d.push(format!("t-states: want {} got {taken}", case.cycles.len()));
    }
    d.extend(diff_trace(&case.cycles, &bus.log));
    d.extend(diff_ports(&case.ports, &bus.ports));

    if d.is_empty() {
        Ok(())
    } else {
        Err(format!("case {index}: {}", d.join("; ")))
    }
}

/// The outcome of one vector file.
pub struct FileResult {
    pub total: usize,
    pub passed: usize,
    /// At most five. A sixth tells a reader nothing the first five did not.
    pub failures: Vec<String>,
}

/// Runs a whole vector file.
///
/// # Panics
///
/// If the file is missing or unreadable, naming it and the fetch command. Per
/// this project's rule, absent test data fails loudly — there is no skip path.
pub fn run_file(path: &Path) -> FileResult {
    let bytes = std::fs::read(path).unwrap_or_else(|e| {
        panic!(
            "missing {}: {e}\nrun `cargo run -p testrunner --release --bin fetchz80`",
            path.display()
        )
    });
    let cases = z80fmt::parse_file(&bytes)
        .unwrap_or_else(|e| panic!("corrupt {}: {e}\ndelete it and re-fetch", path.display()));
    assert!(
        !cases.is_empty(),
        "{} has no cases: delete it and re-fetch",
        path.display()
    );
    let mut r = FileResult {
        total: cases.len(),
        passed: 0,
        failures: Vec::new(),
    };
    for (i, c) in cases.iter().enumerate() {
        match run_case(c, i) {
            Ok(()) => r.passed += 1,
            Err(e) => {
                if r.failures.len() < 5 {
                    r.failures.push(e);
                }
            }
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::z80fmt::RamEntry;

    fn st() -> State {
        State {
            pc: 0x100,
            sp: 0xFFFF,
            ..State::default()
        }
    }

    /// Identical states diff to nothing.
    #[test]
    fn identical_states_produce_no_differences() {
        assert!(diff_state(&st(), &st()).is_empty());
    }

    /// Each differing field is named, with both values, in hex.
    ///
    /// Named individually rather than as a struct dump, because `Debug` on a
    /// 26-field struct puts the reader in the position of diffing it themselves —
    /// which is the job this function exists to do.
    #[test]
    fn a_differing_field_is_named_with_both_values() {
        let mut got = st();
        got.pc = 0x102;
        got.f = 0x42;
        got.q = 1;
        let d = diff_state(&st(), &got);
        assert_eq!(d.len(), 3, "three fields differ: {d:?}");
        let joined = d.join("; ");
        assert!(joined.contains("pc"), "{joined}");
        assert!(
            joined.contains("0100") && joined.contains("0102"),
            "{joined}"
        );
        assert!(joined.contains("f "), "{joined}");
        assert!(
            joined.contains("q "),
            "the undocumented fields too: {joined}"
        );
    }

    /// Every one of the 26 fields is actually compared.
    ///
    /// The point of the test: a `diff_state` that forgot `wz` or `q` would pass
    /// every other test in this file and then silently accept a core missing the
    /// two hardest pieces of Z80 state. Each field is perturbed in turn and must
    /// produce a difference naming it.
    #[test]
    fn all_twenty_six_fields_are_compared() {
        let base = st();
        let mut checked = 0;
        macro_rules! perturb {
            ($name:literal, $f:ident, $v:expr) => {{
                let mut g = base.clone();
                g.$f = $v;
                let d = diff_state(&base, &g);
                assert!(!d.is_empty(), concat!($name, " is not compared"));
                assert!(
                    d.iter().any(|s| s.starts_with(concat!($name, " "))),
                    concat!("the diff must name ", $name, ": {:?}"),
                    d
                );
                checked += 1;
            }};
        }
        perturb!("pc", pc, 0x1234);
        perturb!("sp", sp, 0x1234);
        perturb!("ix", ix, 0x1234);
        perturb!("iy", iy, 0x1234);
        perturb!("wz", wz, 0x1234);
        perturb!("af_", af_, 0x1234);
        perturb!("bc_", bc_, 0x1234);
        perturb!("de_", de_, 0x1234);
        perturb!("hl_", hl_, 0x1234);
        perturb!("a", a, 0x55);
        perturb!("b", b, 0x55);
        perturb!("c", c, 0x55);
        perturb!("d", d, 0x55);
        perturb!("e", e, 0x55);
        perturb!("f", f, 0x55);
        perturb!("h", h, 0x55);
        perturb!("l", l, 0x55);
        perturb!("i", i, 0x55);
        perturb!("r", r, 0x55);
        perturb!("ei", ei, 1);
        perturb!("im", im, 2);
        perturb!("p", p, 1);
        perturb!("q", q, 1);
        perturb!("iff1", iff1, true);
        perturb!("iff2", iff2, true);
        // `ram` is compared by `diff_ram`, not `diff_state` — 26th field, own path.
        let mut g = base.clone();
        g.ram.push(RamEntry { addr: 1, val: 2 });
        assert!(
            !diff_ram(&g, &TraceBus::new(&[])).is_empty(),
            "ram is not compared"
        );
        checked += 1;
        assert_eq!(checked, 26, "every state field must be exercised");
    }

    /// A perturbed field must not be reported as a *different* field.
    ///
    /// `all_twenty_six_fields_are_compared` anchors each message to its field
    /// name, but two fields formatted with a transposed pair of names would still
    /// satisfy it: perturb `h` and a diff saying `l want 00 got 55` names a field
    /// that appears in the list. This pins each message to exactly one field.
    #[test]
    fn a_perturbed_field_is_the_only_one_named() {
        let base = st();
        let mut g = base.clone();
        g.h = 0x55;
        let d = diff_state(&base, &g);
        assert_eq!(d, vec!["h want 00 got 55".to_string()], "h alone");
        let mut g = base.clone();
        g.l = 0x55;
        let d = diff_state(&base, &g);
        assert_eq!(d, vec!["l want 00 got 55".to_string()], "and l alone");
        let mut g = base.clone();
        g.iy = 0x1234;
        let d = diff_state(&base, &g);
        assert_eq!(d, vec!["iy want 0000 got 1234".to_string()]);
    }

    /// RAM is compared at every address the case declares, and nowhere else.
    #[test]
    fn ram_is_compared_at_the_declared_addresses() {
        let want = State {
            ram: vec![
                RamEntry {
                    addr: 0x100,
                    val: 0xAA,
                },
                RamEntry {
                    addr: 0x200,
                    val: 0xBB,
                },
            ],
            ..st()
        };
        let mut bus = TraceBus::new(&[]);
        bus.ram[0x100] = 0xAA;
        bus.ram[0x200] = 0x00; // wrong
        bus.ram[0x300] = 0xFF; // undeclared, and must not be flagged
        let d = diff_ram(&want, &bus);
        assert_eq!(d.len(), 1, "only the declared mismatch: {d:?}");
        assert!(d[0].contains("0200"), "{}", d[0]);
        assert!(d[0].contains("BB") && d[0].contains("00"), "{}", d[0]);
    }

    /// A read: request sample, then the byte on the sample after it.
    fn rd(addr: u16, val: u8) -> [Cycle; 2] {
        [
            Cycle {
                addr,
                data: None,
                rd: true,
                wr: false,
                mreq: true,
                ioreq: false,
            },
            // The address on the data sample is the refresh address upstream, and
            // irrelevant: only `data` is read from it.
            Cycle {
                addr: 0,
                data: Some(val),
                rd: false,
                wr: false,
                mreq: false,
                ioreq: false,
            },
        ]
    }

    /// A write: one sample, carrying its own byte.
    fn wr(addr: u16, val: u8) -> Cycle {
        Cycle {
            addr,
            data: Some(val),
            rd: false,
            wr: true,
            mreq: true,
            ioreq: false,
        }
    }

    /// The bus trace is compared sample by sample, and data only where valid.
    ///
    /// The `r-m-` sample below carries no data upstream, so the harness must not
    /// require the core to have produced a byte for it — while the `----` sample
    /// after it does carry one and must be checked. That asymmetry is the format's
    /// trap and this is the test that pins it.
    #[test]
    fn the_trace_compares_data_only_where_the_flag_is_set() {
        let want = rd(0x100, 0x3E).to_vec();
        // The core read 0x3E from 0x100: one access, matching both samples.
        let log = vec![Access::Read {
            addr: 0x100,
            val: 0x3E,
        }];
        assert!(diff_trace(&want, &log).is_empty(), "a correct read matches");

        // A core that returned a different byte must be caught by the second
        // sample even though the first carries no data at all.
        let log = vec![Access::Read {
            addr: 0x100,
            val: 0x00,
        }];
        let d = diff_trace(&want, &log);
        assert!(!d.is_empty(), "the wrong byte must be caught");
        assert!(d.join("; ").contains("3E"), "{d:?}");
    }

    /// Every way one memory access can be wrong is caught.
    ///
    /// Each mutation below is a real core bug — wrong address, wrong byte, dropped
    /// access, surplus access, reordered pair, read-instead-of-write — and a
    /// comparison that missed any one of them would report a green suite for a
    /// core that was corrupting memory. The trace here is a `LD (nn),A`: fetch,
    /// two operand reads, one write.
    #[test]
    fn each_way_an_access_can_be_wrong_is_caught() {
        let mut want = Vec::new();
        want.extend(rd(0x100, 0x32));
        want.extend(rd(0x101, 0x85));
        want.extend(rd(0x102, 0xC8));
        want.push(wr(0xC885, 0x97));
        let good = vec![
            Access::Read {
                addr: 0x100,
                val: 0x32,
            },
            Access::Read {
                addr: 0x101,
                val: 0x85,
            },
            Access::Read {
                addr: 0x102,
                val: 0xC8,
            },
            Access::Write {
                addr: 0xC885,
                val: 0x97,
            },
        ];
        assert!(
            diff_trace(&want, &good).is_empty(),
            "{:?}",
            diff_trace(&want, &good)
        );

        /// A named core bug: what it is called, and how to break the log.
        type Mutant = (&'static str, fn(&mut Vec<Access>));

        let mutate: [Mutant; 8] = [
            ("read address", |l| {
                l[1] = Access::Read {
                    addr: 0x1FF,
                    val: 0x85,
                };
            }),
            ("read data", |l| {
                l[1] = Access::Read {
                    addr: 0x101,
                    val: 0x00,
                };
            }),
            ("write address", |l| {
                l[3] = Access::Write {
                    addr: 0x0000,
                    val: 0x97,
                };
            }),
            ("write data", |l| {
                l[3] = Access::Write {
                    addr: 0xC885,
                    val: 0x00,
                };
            }),
            ("dropped last", |l| {
                l.pop();
            }),
            ("dropped first", |l| {
                l.remove(0);
            }),
            ("surplus", |l| {
                l.push(Access::Read {
                    addr: 0x1234,
                    val: 0x56,
                });
            }),
            ("write became read", |l| {
                l[3] = Access::Read {
                    addr: 0xC885,
                    val: 0x97,
                };
            }),
        ];
        for (name, f) in mutate {
            let mut log = good.clone();
            f(&mut log);
            assert!(!diff_trace(&want, &log).is_empty(), "{name} was not caught");
        }

        // Reordering: same accesses, wrong order.
        let mut swapped = good.clone();
        swapped.swap(0, 1);
        assert!(!diff_trace(&want, &swapped).is_empty(), "order matters");
    }

    /// A port's data sample must not consume a memory access.
    ///
    /// `IN r,(C)` ends with a pin-free data-bearing sample carrying the byte the
    /// device returned — indistinguishable, by pins alone, from a read's byte. A
    /// walker that stepped the memory log at every data-bearing sample would spend
    /// its cursor there and then account for a surplus memory access as if it were
    /// expected. Measured against real vectors: that variant lets an extra access
    /// through on `ED 40` and `ED 58`. This is the trace from `ed_40.json` case 5.
    #[test]
    fn a_ports_data_sample_does_not_absorb_a_memory_access() {
        let mut want = Vec::new();
        want.extend(rd(0x612B, 0xED));
        want.extend(rd(0x612C, 0x40));
        want.push(Cycle {
            addr: 0x1409,
            data: None,
            rd: false,
            wr: false,
            mreq: false,
            ioreq: false,
        });
        want.push(Cycle {
            addr: 0x1409,
            data: None,
            rd: true,
            wr: false,
            mreq: false,
            ioreq: true,
        });
        want.push(Cycle {
            addr: 0x1409,
            data: Some(0x56),
            rd: false,
            wr: false,
            mreq: false,
            ioreq: false,
        });
        let good = vec![
            Access::Read {
                addr: 0x612B,
                val: 0xED,
            },
            Access::Read {
                addr: 0x612C,
                val: 0x40,
            },
        ];
        assert!(
            diff_trace(&want, &good).is_empty(),
            "the two fetches and the port: {:?}",
            diff_trace(&want, &good)
        );

        let mut extra = good.clone();
        extra.push(Access::Read {
            addr: 0x1409,
            val: 0x56,
        });
        let d = diff_trace(&want, &extra);
        assert!(
            !d.is_empty(),
            "a core that read the port through memory must be caught"
        );
        assert!(d.join("; ").contains("extra"), "{d:?}");
    }

    /// Ports are compared in order, with direction, and a surplus is reported.
    #[test]
    fn ports_are_compared_in_order_with_direction() {
        let want = vec![
            Port {
                addr: 0x00FE,
                val: 0x5A,
                out: false,
            },
            Port {
                addr: 0x00FE,
                val: 0x99,
                out: true,
            },
        ];
        assert!(diff_ports(&want, &want).is_empty());

        // Same transactions, opposite order: a core that wrote before reading.
        let swapped = vec![want[1], want[0]];
        assert!(!diff_ports(&want, &swapped).is_empty(), "order matters");

        // Direction alone differing must be caught.
        let flipped = vec![
            Port {
                out: true,
                ..want[0]
            },
            want[1],
        ];
        assert!(!diff_ports(&want, &flipped).is_empty(), "direction matters");

        // An extra transaction the case did not declare.
        let mut extra = want.clone();
        extra.push(Port {
            addr: 1,
            val: 2,
            out: true,
        });
        let d = diff_ports(&want, &extra);
        assert!(
            d.join("; ").contains("2 ports") || d.join("; ").contains("count"),
            "{d:?}"
        );

        // A missing transaction, too: `zip` stops at the shorter side, so without
        // the length check a core that made no port access at all would pass.
        let d = diff_ports(&want, &want[..1]);
        assert!(!d.is_empty(), "a missing transaction must be caught");
    }

    /// `run_case` puts the whole comparison together on a real instruction.
    ///
    /// `NOP` is the one base opcode Task 5 implements that touches nothing, so this
    /// is the only end-to-end check available before Task 7 — and it is worth
    /// having, because `load`/`unload` are 25 field copies each and a transposed
    /// pair there would be invisible to every diff test above.
    #[test]
    fn run_case_accepts_a_correct_nop_and_rejects_a_wrong_expectation() {
        let initial = State {
            pc: 0x100,
            r: 0x00,
            ram: vec![RamEntry {
                addr: 0x100,
                val: 0x00,
            }],
            ..State::default()
        };
        let final_ = State {
            pc: 0x101,
            r: 0x01,
            ram: vec![RamEntry {
                addr: 0x100,
                val: 0x00,
            }],
            ..State::default()
        };
        let mut cycles = rd(0x100, 0x00).to_vec();
        cycles.push(Cycle::default());
        cycles.push(Cycle::default());
        let case = Case {
            initial,
            final_,
            cycles,
            ports: Vec::new(),
        };
        assert_eq!(run_case(&case, 0), Ok(()), "a correct NOP");

        // The index appears in the message, and a wrong expectation fails.
        let mut bad = case.clone();
        bad.final_.pc = 0x999;
        let e = run_case(&bad, 7).expect_err("must fail");
        assert!(e.starts_with("case 7:"), "{e}");
        assert!(e.contains("pc want 0999"), "{e}");
    }

    /// The T-state total is compared against the sample count.
    #[test]
    fn a_wrong_t_state_count_is_reported() {
        let initial = State {
            pc: 0x100,
            ram: vec![RamEntry {
                addr: 0x100,
                val: 0x00,
            }],
            ..State::default()
        };
        let final_ = State {
            pc: 0x101,
            r: 0x01,
            ram: Vec::new(),
            ..State::default()
        };
        // Three samples where `NOP` takes four: the core's 4 must be flagged.
        let case = Case {
            initial,
            final_,
            cycles: rd(0x100, 0x00)
                .to_vec()
                .into_iter()
                .chain([Cycle::default()])
                .collect(),
            ports: Vec::new(),
        };
        let e = run_case(&case, 0).expect_err("must fail");
        assert!(e.contains("t-states: want 3 got 4"), "{e}");
    }
}
