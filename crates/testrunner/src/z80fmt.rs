//! The `Z80V` vector format: a compact binary rendering of the
//! SingleStepTests/z80 JSON.
//!
//! Upstream ships JSON only — 1.37 GB of it, against 2.9 GiB free on the
//! development machine — and no crate in this workspace has a JSON parser,
//! deliberately. So `fetchz80` converts each file as it downloads and this module
//! defines what it writes: little-endian, a magic, a count, then fixed records.
//! Measured shrink is 5.8x across all seven opcode pages, so the whole suite is
//! about 236 MB.
//!
//! Case **names are dropped**. Upstream's name is `"<PAGE> <OP> <index>"`, fully
//! recoverable from the filename and the case index, and storing 1.6 M of them
//! would be 30 MB of nothing.

/// `Z80V` in file order. As a little-endian `u32` that is `0x5630_385A` — the
/// digits in reading order would be `0x5A38_3056`, which is a different file.
pub const MAGIC: u32 = 0x5630_385A;

/// One RAM location a case seeds or checks.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RamEntry {
    pub addr: u16,
    pub val: u8,
}

/// A CPU state: the suite's 26 fields, plus the RAM the case declares.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct State {
    pub pc: u16,
    pub sp: u16,
    pub ix: u16,
    pub iy: u16,
    pub wz: u16,
    pub af_: u16,
    pub bc_: u16,
    pub de_: u16,
    pub hl_: u16,
    pub a: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub f: u8,
    pub h: u8,
    pub l: u8,
    pub i: u8,
    pub r: u8,
    pub ei: u8,
    pub im: u8,
    pub p: u8,
    pub q: u8,
    pub iff1: bool,
    pub iff2: bool,
    pub ram: Vec<RamEntry>,
}

/// One bus sample, taken **between** T-states.
///
/// `data` is `None` when the bus is electrically disconnected from the CPU. That
/// is not derivable from the pins: a read's request T-state has `rd`/`mreq` set
/// and no data, and the byte appears on a later sample with no pins set at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Cycle {
    pub addr: u16,
    pub data: Option<u8>,
    pub rd: bool,
    pub wr: bool,
    pub mreq: bool,
    pub ioreq: bool,
}

/// One I/O-space transaction. Present only on the `IN`/`OUT` pages.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Port {
    pub addr: u16,
    pub val: u8,
    /// `true` for `OUT`, `false` for `IN`.
    pub out: bool,
}

/// One test case.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Case {
    pub initial: State,
    pub final_: State,
    pub cycles: Vec<Cycle>,
    pub ports: Vec<Port>,
}

/// Serializes cases into the format this module documents.
///
/// # Panics
///
/// If a count exceeds its field: more than 255 RAM entries or ports, or more
/// than 65,535 cycles. Measured maxima upstream are 5, 1 and 23, so these
/// cannot fire on today's data — they exist because a silent `as u8` is how a
/// format grows a truncation bug when someone regenerates the suite with
/// different options. A checked bound can be raised safely; a truncating one
/// cannot.
#[must_use]
pub fn write_file(cases: &[Case]) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend_from_slice(&MAGIC.to_le_bytes());
    o.extend_from_slice(
        &u32::try_from(cases.len())
            .expect("case count fits u32")
            .to_le_bytes(),
    );
    for c in cases {
        write_state(&mut o, &c.initial);
        write_state(&mut o, &c.final_);
        let n = u16::try_from(c.cycles.len()).expect("cycle count fits u16");
        o.extend_from_slice(&n.to_le_bytes());
        for cy in &c.cycles {
            o.extend_from_slice(&cy.addr.to_le_bytes());
            o.push(cy.data.unwrap_or(0));
            let mut f = 0u8;
            if cy.data.is_some() {
                f |= 1;
            }
            if cy.rd {
                f |= 2;
            }
            if cy.wr {
                f |= 4;
            }
            if cy.mreq {
                f |= 8;
            }
            if cy.ioreq {
                f |= 16;
            }
            o.push(f);
        }
        o.push(u8::try_from(c.ports.len()).expect("port count fits u8"));
        for p in &c.ports {
            o.extend_from_slice(&p.addr.to_le_bytes());
            o.push(p.val);
            o.push(u8::from(p.out));
        }
    }
    o
}

fn write_state(o: &mut Vec<u8>, s: &State) {
    for v in [s.pc, s.sp, s.ix, s.iy, s.wz, s.af_, s.bc_, s.de_, s.hl_] {
        o.extend_from_slice(&v.to_le_bytes());
    }
    o.extend_from_slice(&[s.a, s.b, s.c, s.d, s.e, s.f, s.h, s.l, s.i, s.r]);
    o.extend_from_slice(&[s.ei, s.im, s.p, s.q, u8::from(s.iff1), u8::from(s.iff2)]);
    o.push(u8::try_from(s.ram.len()).expect("ram count fits u8"));
    for r in &s.ram {
        o.extend_from_slice(&r.addr.to_le_bytes());
        o.push(r.val);
    }
}

/// What can be wrong with a vector file.
#[derive(Debug)]
pub enum ParseError {
    /// The first four bytes are not [`MAGIC`].
    BadMagic { want: u32, got: u32 },
    /// The file ended mid-record.
    Truncated { at: usize, need: usize },
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ParseError::BadMagic { want, got } => {
                write!(f, "bad magic: want {want:08X}, got {got:08X}")
            }
            ParseError::Truncated { at, need } => {
                write!(f, "truncated at byte {at}: need {need} more")
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// A cursor that cannot read past its end.
struct Rd<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ParseError> {
        let end = self.at.checked_add(n).ok_or(ParseError::Truncated {
            at: self.at,
            need: n,
        })?;
        if end > self.b.len() {
            return Err(ParseError::Truncated {
                at: self.at,
                need: end - self.b.len(),
            });
        }
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, ParseError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ParseError> {
        let s = self.take(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }

    fn u32(&mut self) -> Result<u32, ParseError> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
}

/// Parses a whole vector file.
///
/// # Errors
///
/// [`ParseError::BadMagic`] if the header is not a `Z80V` file, or
/// [`ParseError::Truncated`] if it ends mid-record.
pub fn parse_file(bytes: &[u8]) -> Result<Vec<Case>, ParseError> {
    let mut r = Rd { b: bytes, at: 0 };
    let magic = r.u32()?;
    if magic != MAGIC {
        return Err(ParseError::BadMagic {
            want: MAGIC,
            got: magic,
        });
    }
    let n = r.u32()? as usize;
    // Not `with_capacity(n)`: `n` comes from the file, so a corrupt header would
    // ask for an arbitrary allocation before a single record is validated.
    let mut out = Vec::new();
    for _ in 0..n {
        let initial = read_state(&mut r)?;
        let final_ = read_state(&mut r)?;
        let ncyc = r.u16()? as usize;
        let mut cycles = Vec::with_capacity(ncyc);
        for _ in 0..ncyc {
            let addr = r.u16()?;
            let data = r.u8()?;
            let f = r.u8()?;
            cycles.push(Cycle {
                addr,
                data: if f & 1 != 0 { Some(data) } else { None },
                rd: f & 2 != 0,
                wr: f & 4 != 0,
                mreq: f & 8 != 0,
                ioreq: f & 16 != 0,
            });
        }
        let nport = r.u8()? as usize;
        let mut ports = Vec::with_capacity(nport);
        for _ in 0..nport {
            ports.push(Port {
                addr: r.u16()?,
                val: r.u8()?,
                out: r.u8()? != 0,
            });
        }
        out.push(Case {
            initial,
            final_,
            cycles,
            ports,
        });
    }
    Ok(out)
}

fn read_state(r: &mut Rd) -> Result<State, ParseError> {
    let mut s = State {
        pc: r.u16()?,
        sp: r.u16()?,
        ix: r.u16()?,
        iy: r.u16()?,
        wz: r.u16()?,
        af_: r.u16()?,
        bc_: r.u16()?,
        de_: r.u16()?,
        hl_: r.u16()?,
        ..State::default()
    };
    s.a = r.u8()?;
    s.b = r.u8()?;
    s.c = r.u8()?;
    s.d = r.u8()?;
    s.e = r.u8()?;
    s.f = r.u8()?;
    s.h = r.u8()?;
    s.l = r.u8()?;
    s.i = r.u8()?;
    s.r = r.u8()?;
    s.ei = r.u8()?;
    s.im = r.u8()?;
    s.p = r.u8()?;
    s.q = r.u8()?;
    s.iff1 = r.u8()? != 0;
    s.iff2 = r.u8()? != 0;
    let n = r.u8()? as usize;
    s.ram = Vec::with_capacity(n);
    for _ in 0..n {
        s.ram.push(RamEntry {
            addr: r.u16()?,
            val: r.u8()?,
        });
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The magic's four bytes read `Z80V` in **file order**.
    ///
    /// Pinned as bytes, not as the `u32`: `'Z','8','0','V'` as a little-endian
    /// `u32` is `0x5630385A`, and writing the digits in the order they are read
    /// gives `0x5A383056` — a value that looks right in a spec and fails at
    /// runtime. The spec records this reversal as a caught mistake.
    #[test]
    fn the_magic_reads_z80v_in_file_order() {
        assert_eq!(MAGIC.to_le_bytes(), *b"Z80V");
        assert_eq!(MAGIC, 0x5630_385A);
    }

    /// A file written then parsed is the same cases.
    ///
    /// The case is built by hand with every optional part populated — RAM in both
    /// halves, several cycles with mixed validity, and a port — because a
    /// round-trip over an empty case would pass with half the writer missing.
    #[test]
    fn a_written_file_parses_back_to_the_same_cases() {
        let c = Case {
            initial: State {
                pc: 0x1234,
                sp: 0xFFFE,
                ix: 0x1111,
                iy: 0x2222,
                wz: 0x3333,
                af_: 0x4444,
                bc_: 0x5555,
                de_: 0x6666,
                hl_: 0x7777,
                a: 1,
                b: 2,
                c: 3,
                d: 4,
                e: 5,
                f: 6,
                h: 7,
                l: 8,
                i: 9,
                r: 10,
                ei: 1,
                im: 2,
                p: 1,
                q: 1,
                iff1: true,
                iff2: false,
                ram: vec![RamEntry {
                    addr: 0x1234,
                    val: 0xED,
                }],
            },
            final_: State {
                pc: 0x1236,
                sp: 0xFFFE,
                ix: 0x1111,
                iy: 0x2222,
                wz: 0x9999,
                af_: 0x4444,
                bc_: 0x5555,
                de_: 0x6666,
                hl_: 0x7777,
                a: 0xFF,
                b: 2,
                c: 3,
                d: 4,
                e: 5,
                f: 0x42,
                h: 7,
                l: 8,
                i: 9,
                r: 11,
                ei: 0,
                im: 2,
                p: 0,
                q: 1,
                iff1: true,
                iff2: false,
                ram: vec![
                    RamEntry {
                        addr: 0x1234,
                        val: 0xED,
                    },
                    RamEntry {
                        addr: 0x5678,
                        val: 0x99,
                    },
                ],
            },
            cycles: vec![
                Cycle {
                    addr: 0x1234,
                    data: None,
                    rd: false,
                    wr: false,
                    mreq: false,
                    ioreq: false,
                },
                Cycle {
                    addr: 0x1234,
                    data: None,
                    rd: true,
                    wr: false,
                    mreq: true,
                    ioreq: false,
                },
                Cycle {
                    addr: 0x1234,
                    data: Some(0xED),
                    rd: false,
                    wr: false,
                    mreq: false,
                    ioreq: false,
                },
                Cycle {
                    addr: 0x5678,
                    data: Some(0x99),
                    rd: false,
                    wr: true,
                    mreq: true,
                    ioreq: false,
                },
            ],
            ports: vec![Port {
                addr: 0xBEEF,
                val: 0x5A,
                out: true,
            }],
        };
        let bytes = write_file(core::slice::from_ref(&c));
        let back = parse_file(&bytes).expect("round trip");
        assert_eq!(back.len(), 1);
        assert_eq!(back[0], c);
    }

    /// A file whose magic is wrong is an error naming both values.
    #[test]
    fn a_bad_magic_is_an_error_naming_what_was_expected() {
        let mut bytes = write_file(&[]);
        bytes[0] ^= 0xFF;
        let e = parse_file(&bytes).expect_err("must reject");
        let msg = e.to_string();
        assert!(msg.contains("magic"), "{msg}");
        assert!(
            msg.contains("5630385A") || msg.contains("5630_385A"),
            "{msg}"
        );
    }

    /// A truncated file is an error, not a panic and not a short read.
    ///
    /// Every prefix is tried, because a parser that checked only its own first
    /// field would pass a test that truncated only the header.
    #[test]
    fn every_truncation_is_an_error_rather_than_a_panic() {
        let full = write_file(&[Case::default()]);
        for n in 0..full.len() {
            let r = parse_file(&full[..n]);
            assert!(r.is_err(), "prefix of {n} bytes must not parse");
        }
        assert!(parse_file(&full).is_ok(), "and the whole file must");
    }

    /// `data_valid` is a bit of its own, not inferred from the pins.
    ///
    /// Measured upstream: `r-m-` samples carry **no** data (15,000 of 15,000) and
    /// `----` samples carry data about a quarter of the time. So a reader that
    /// derived validity from "is this a read cycle" would drop every byte the CPU
    /// returned. This asserts the two cases that shape the flag: an internal
    /// cycle *with* data, and a read request *without*.
    #[test]
    fn data_validity_is_independent_of_the_pins() {
        let c = Case {
            cycles: vec![
                Cycle {
                    addr: 0x100,
                    data: Some(0x42),
                    rd: false,
                    wr: false,
                    mreq: false,
                    ioreq: false,
                },
                Cycle {
                    addr: 0x100,
                    data: None,
                    rd: true,
                    wr: false,
                    mreq: true,
                    ioreq: false,
                },
            ],
            ..Case::default()
        };
        let back = parse_file(&write_file(&[c])).expect("round trip");
        assert_eq!(
            back[0].cycles[0].data,
            Some(0x42),
            "an internal cycle can carry data"
        );
        assert!(!back[0].cycles[0].rd, "and still not be a read");
        assert_eq!(back[0].cycles[1].data, None, "a read request carries none");
        assert!(back[0].cycles[1].rd, "and is still a read");
    }
}
