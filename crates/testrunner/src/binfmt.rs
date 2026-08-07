//! Parser for the SingleStepTests/m68000 `.json.bin` format.
//!
//! Despite the file extension these are not JSON: it is a little-endian binary
//! format, documented by the upstream `decode.py` and reproduced in the spec.

#[derive(Debug)]
pub enum ParseError {
    Truncated { at: usize, need: usize },
    BadMagic { at: usize, want: u32, got: u32 },
    BadKind { at: usize, got: u8 },
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ParseError::Truncated { at, need } => {
                write!(f, "truncated at byte {at}: need {need} more")
            }
            ParseError::BadMagic { at, want, got } => {
                write!(
                    f,
                    "bad magic at byte {at}: want {want:#010X}, got {got:#010X}"
                )
            }
            ParseError::BadKind { at, got } => {
                write!(f, "bad transaction kind at byte {at}: {got}")
            }
        }
    }
}

impl std::error::Error for ParseError {}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TxKind {
    Idle,
    Write,
    Read,
    Tas,
    /// Read that raised an address error: AS was never asserted, so this
    /// access must NOT be committed to the bus.
    ReadAddrErr,
    /// Write that raised an address error; likewise never committed.
    WriteAddrErr,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Transaction {
    pub kind: TxKind,
    pub cycles: u32,
    pub fc: u32,
    pub addr: u32,
    /// As seen on the real data bus: a byte via UDS reads as `0xAB00`.
    pub data: u32,
    pub uds: u32,
    pub lds: u32,
}

impl Transaction {
    /// True when this access is a word access (both strobes asserted).
    pub fn is_word(&self) -> bool {
        self.uds + self.lds == 2
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct State {
    pub d: [u32; 8],
    /// a0..a6 only; a7 is `usp` or `ssp` depending on the SR's S bit.
    pub a: [u32; 7],
    pub usp: u32,
    pub ssp: u32,
    pub sr: u16,
    pub pc: u32,
    pub prefetch: [u16; 2],
    /// Byte-granular memory, expanded from the file's 16-bit words.
    pub ram: Vec<(u32, u8)>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TestCase {
    pub name: String,
    pub initial: State,
    pub final_: State,
    pub transactions: Vec<Transaction>,
    /// Total cycles the instruction should consume.
    pub length: u32,
}

struct Cursor<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cursor<'a> {
    fn u8(&mut self) -> Result<u8, ParseError> {
        let v = *self.b.get(self.p).ok_or(ParseError::Truncated {
            at: self.p,
            need: 1,
        })?;
        self.p += 1;
        Ok(v)
    }

    fn u16(&mut self) -> Result<u16, ParseError> {
        let s = self.take(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }

    fn u32(&mut self) -> Result<u32, ParseError> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ParseError> {
        let end = self.p.checked_add(n).ok_or(ParseError::Truncated {
            at: self.p,
            need: n,
        })?;
        let s = self.b.get(self.p..end).ok_or(ParseError::Truncated {
            at: self.p,
            need: n,
        })?;
        self.p = end;
        Ok(s)
    }

    /// Reads the `numbytes`/`magic` pair that prefixes every block.
    fn block_header(&mut self, want: u32) -> Result<(), ParseError> {
        let _numbytes = self.u32()?;
        let at = self.p;
        let got = self.u32()?;
        if got != want {
            return Err(ParseError::BadMagic { at, want, got });
        }
        Ok(())
    }
}

fn read_state(c: &mut Cursor) -> Result<State, ParseError> {
    c.block_header(0x0123_4567)?;
    let mut st = State::default();
    for i in 0..8 {
        st.d[i] = c.u32()?;
    }
    for i in 0..7 {
        st.a[i] = c.u32()?;
    }
    st.usp = c.u32()?;
    st.ssp = c.u32()?;
    st.sr = c.u32()? as u16;
    st.pc = c.u32()?;
    st.prefetch = [c.u32()? as u16, c.u32()? as u16];

    let num_ram = c.u32()? as usize;
    st.ram = Vec::with_capacity(num_ram * 2);
    for _ in 0..num_ram {
        let addr = c.u32()?;
        let word = c.u16()?;
        st.ram.push((addr, (word >> 8) as u8));
        st.ram.push((addr | 1, (word & 0xFF) as u8));
    }
    Ok(st)
}

fn read_transactions(c: &mut Cursor) -> Result<(Vec<Transaction>, u32), ParseError> {
    c.block_header(0x4567_89AB)?;
    let num_cycles = c.u32()?;
    let n = c.u32()? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let at = c.p;
        let raw = c.u8()?;
        let cycles = c.u32()?;
        let kind = match raw {
            0 => TxKind::Idle,
            1 => TxKind::Write,
            2 => TxKind::Read,
            // Unexercised by the current corpus: `Tas` occurs 0 times in
            // 1,783,580 transactions (measured, Task 14). The arm stays because
            // `3` is part of the on-disk format — deleting it would turn a
            // valid-per-format file into `BadKind`, which is a worse failure
            // than a dead branch, and TAS's read-modify-write does have a
            // distinct bus signature a future suite version could emit.
            3 => TxKind::Tas,
            4 => TxKind::ReadAddrErr,
            5 => TxKind::WriteAddrErr,
            got => return Err(ParseError::BadKind { at, got }),
        };
        if kind == TxKind::Idle {
            out.push(Transaction {
                kind,
                cycles,
                fc: 0,
                addr: 0,
                data: 0,
                uds: 0,
                lds: 0,
            });
            continue;
        }
        let (fc, addr, data, uds, lds) = (c.u32()?, c.u32()?, c.u32()?, c.u32()?, c.u32()?);
        out.push(Transaction {
            kind,
            cycles,
            fc,
            addr,
            data,
            uds,
            lds,
        });
    }
    Ok((out, num_cycles))
}

/// Reads a case name, substituting U+FFFD for any invalid UTF-8 rather than failing.
///
/// Lossy on purpose: the name is a diagnostic label, printed in failure messages and
/// used by nothing that executes. A malformed name should not stop 2,500 cases of real
/// timing data from being checked, which is what returning `Err` here would do.
///
/// Measured: **0 of 317,500** names contain a replacement character and 0 are even
/// non-ASCII, so the lossy path is never taken on the current corpus. Control — the
/// detector was confirmed against `from_utf8_lossy([41 FF 42])`, which does yield
/// `"A\u{FFFD}B"`; without that check the zero would equally well have meant the query
/// was broken.
///
/// So the choice is currently untested-by-corpus rather than load-bearing. It is kept
/// because the alternative fails *more* on the same input, not because the input is
/// expected: if a future suite version writes a name in some other encoding, this
/// degrades one label and `from_utf8` would reject the whole file.
fn read_name(c: &mut Cursor) -> Result<String, ParseError> {
    c.block_header(0x89AB_CDEF)?;
    let len = c.u32()? as usize;
    let s = c.take(len)?;
    Ok(String::from_utf8_lossy(s).into_owned())
}

/// Parses a whole `.json.bin` file into its test cases.
pub fn parse_file(bytes: &[u8]) -> Result<Vec<TestCase>, ParseError> {
    let mut c = Cursor { b: bytes, p: 0 };
    let at = c.p;
    let magic = c.u32()?;
    if magic != 0x1A3F_5D71 {
        return Err(ParseError::BadMagic {
            at,
            want: 0x1A3F_5D71,
            got: magic,
        });
    }
    let n = c.u32()? as usize;
    let mut cases = Vec::with_capacity(n);
    for _ in 0..n {
        c.block_header(0xABC1_2367)?;
        let name = read_name(&mut c)?;
        let initial = read_state(&mut c)?;
        let final_ = read_state(&mut c)?;
        let (transactions, length) = read_transactions(&mut c)?;
        cases.push(TestCase {
            name,
            initial,
            final_,
            transactions,
            length,
        });
    }
    Ok(cases)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a one-case file in memory, exercising every block type.
    fn synth() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0x1A3F_5D71u32.to_le_bytes()); // file magic
        b.extend_from_slice(&1u32.to_le_bytes()); // num_tests

        let mut t = Vec::new();
        t.extend_from_slice(&0u32.to_le_bytes()); // numbytes (ignored)
        t.extend_from_slice(&0xABC1_2367u32.to_le_bytes());

        // name block
        t.extend_from_slice(&0u32.to_le_bytes());
        t.extend_from_slice(&0x89AB_CDEFu32.to_le_bytes());
        let name = b"TEST OP";
        t.extend_from_slice(&(name.len() as u32).to_le_bytes());
        t.extend_from_slice(name);

        // two identical state blocks
        for _ in 0..2 {
            t.extend_from_slice(&0u32.to_le_bytes());
            t.extend_from_slice(&0x0123_4567u32.to_le_bytes());
            for i in 0..19u32 {
                t.extend_from_slice(&i.to_le_bytes()); // d0..d7,a0..a6,usp,ssp,sr,pc
            }
            t.extend_from_slice(&0xAAAAu32.to_le_bytes()); // prefetch[0]
            t.extend_from_slice(&0xBBBBu32.to_le_bytes()); // prefetch[1]
            t.extend_from_slice(&1u32.to_le_bytes()); // num_ram
            t.extend_from_slice(&0x1234u32.to_le_bytes()); // addr
            t.extend_from_slice(&0xCAFEu16.to_le_bytes()); // word
        }

        // transactions: one idle, one read
        t.extend_from_slice(&0u32.to_le_bytes());
        t.extend_from_slice(&0x4567_89ABu32.to_le_bytes());
        t.extend_from_slice(&16u32.to_le_bytes()); // num_cycles
        t.extend_from_slice(&2u32.to_le_bytes()); // num_transactions
        t.push(0); // idle
        t.extend_from_slice(&2u32.to_le_bytes());
        t.push(2); // read
        t.extend_from_slice(&4u32.to_le_bytes());
        for v in [2u32, 0x00FF_0000, 0x1234, 1, 1] {
            t.extend_from_slice(&v.to_le_bytes());
        }

        b.extend_from_slice(&t);
        b
    }

    #[test]
    fn parses_a_synthetic_file() {
        let cases = parse_file(&synth()).expect("parse");
        assert_eq!(cases.len(), 1);
        let c = &cases[0];
        assert_eq!(c.name, "TEST OP");
        assert_eq!(c.length, 16);
        assert_eq!(c.initial.d[3], 3);
        assert_eq!(c.initial.a[0], 8);
        assert_eq!(c.initial.usp, 15);
        assert_eq!(c.initial.ssp, 16);
        assert_eq!(c.initial.sr, 17);
        assert_eq!(c.initial.pc, 18);
        assert_eq!(c.initial.prefetch, [0xAAAA, 0xBBBB]);
        // one RAM word expands to two bytes, high byte first
        assert_eq!(c.initial.ram, vec![(0x1234, 0xCA), (0x1235, 0xFE)]);
        assert_eq!(c.transactions.len(), 2);
        assert_eq!(c.transactions[0].kind, TxKind::Idle);
        assert_eq!(c.transactions[0].cycles, 2);
        assert_eq!(c.transactions[1].kind, TxKind::Read);
        assert_eq!(c.transactions[1].addr, 0x00FF_0000);
    }

    #[test]
    fn rejects_a_bad_magic() {
        let mut bad = synth();
        bad[0] ^= 0xFF;
        // The variant, not just `is_err()`: a truncation reported as BadMagic (or the
        // reverse) points a reader at the wrong half of the format.
        assert!(
            matches!(parse_file(&bad), Err(ParseError::BadMagic { .. })),
            "a corrupted magic word must be reported as BadMagic"
        );
    }

    /// Every prefix of a valid file must be rejected as [`ParseError::Truncated`].
    ///
    /// `Truncated` is constructed at three sites in `Cursor` and had **no test**: the
    /// only error-path test was `rejects_a_bad_magic`. A parser that read past the end
    /// of a short buffer — or that returned `Ok` with a half-populated case — would have
    /// been caught by nothing, and a truncated download is the most likely way a
    /// real-world file goes wrong.
    ///
    /// Sweeping every prefix rather than one hand-picked length is what makes this
    /// cover the three sites: `u8`, `u32`, and `take` each fail at different offsets,
    /// and a single truncation point exercises whichever one happens to sit there.
    #[test]
    fn rejects_every_truncation() {
        let full = synth();
        // Control: the untruncated buffer must parse, otherwise "everything shorter
        // fails" is trivially true and tests nothing.
        assert!(
            parse_file(&full).is_ok(),
            "the full synthetic file must parse"
        );

        for len in 0..full.len() {
            match parse_file(&full[..len]) {
                Err(ParseError::Truncated { at, need }) => {
                    // The offset must be inside the buffer we handed over, and the
                    // need must be non-zero — an error reporting `need: 0` would be
                    // describing a read that could not have failed.
                    assert!(
                        at <= len,
                        "truncation at {len}: reported offset {at} is past the input"
                    );
                    assert!(need > 0, "truncation at {len}: reported need of 0 bytes");
                }
                Err(ParseError::BadMagic { .. }) => {
                    // Legitimate: a prefix can cut a magic word in half so that the
                    // bytes present compare unequal before the cursor runs out.
                }
                Err(e @ ParseError::BadKind { .. }) => panic!(
                    "a {len}-byte prefix reported {e}: truncation cannot invent a \
                     transaction-kind byte, so this means the cursor read past the end \
                     of the input or resynchronised onto misaligned data"
                ),
                Ok(cases) => panic!(
                    "a {len}-byte prefix of a {}-byte file parsed to {} case(s)",
                    full.len(),
                    cases.len()
                ),
            }
        }
    }

    /// Reads a vector file, or fails naming the file and the fetch command.
    ///
    /// ⚠️ **This used to return `Option` and its two callers `eprintln!`ed and
    /// returned**, so with an empty `testdata/` both tests passed while asserting
    /// nothing — including `parses_every_suite_file`, whose entire content is a
    /// comparison against 127. A green run meant either "the parser is correct" or "the
    /// vectors are missing", and the two were indistinguishable in the output. Every
    /// other testdata reader in the crate panics with the fetch command
    /// (`runner::assert_group`, `tests/suite.rs`, `tests/disasm_group.rs`); these two
    /// were the only holdouts, against the project's fail-loudly-and-name-the-file
    /// rule.
    fn testdata(name: &str) -> Vec<u8> {
        let p =
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata")).join(name);
        std::fs::read(&p).unwrap_or_else(|e| {
            panic!(
                "cannot read {}: {e} — run `cargo run -p testrunner --bin fetch`",
                p.display()
            )
        })
    }

    /// Values here were confirmed by decoding `ADD.b` case 000 with the
    /// upstream `decode.py`, so this pins our parser to the real format.
    #[test]
    fn parses_real_add_b() {
        let bytes = testdata("ADD.b.json.bin");
        let cases = parse_file(&bytes).expect("parse ADD.b");
        assert_eq!(cases.len(), 2500);

        let c = &cases[0];
        assert_eq!(c.name, "000 ADD.b D3, (d16, A2) d72a");
        assert_eq!(c.initial.pc, 0x32CBB6);
        assert_eq!(c.initial.prefetch, [0xD72A, 0x9CBC]);
        assert_eq!(c.final_.pc, 0x32CBBA);
        assert_eq!(c.final_.prefetch, [0x77B3, 0x1B0A]);
        assert_eq!(c.length, 16);
        assert_eq!(c.transactions.len(), 4);
        assert_eq!(c.transactions[0].kind, TxKind::Read);
        assert_eq!(c.transactions[0].addr, 0x32CBB6);
        assert!(c.transactions[0].is_word());
        assert_eq!(c.transactions[3].kind, TxKind::Write);

        // PC is 4 ahead of the executing opcode, which lives in RAM at pc-4.
        let ram: std::collections::HashMap<u32, u8> = c.initial.ram.iter().copied().collect();
        let opcode = u16::from_be_bytes([ram[&0x32CBB2], ram[&0x32CBB3]]);
        assert_eq!(opcode, 0xD72A, "prefetch[0] must equal the word at pc-4");
    }

    /// Every file in the suite must parse.
    #[test]
    fn parses_every_suite_file() {
        let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata"));
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| {
            panic!(
                "cannot read {}: {e} — run `cargo run -p testrunner --bin fetch`",
                dir.display()
            )
        });
        let mut n = 0;
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("bin") {
                continue;
            }
            // Named, not `.unwrap()`: a bare unwrap here reports only "Os { code: 2 }"
            // with no indication of which of the 127 files could not be read.
            let bytes =
                std::fs::read(&p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()));
            let cases = parse_file(&bytes).unwrap_or_else(|err| panic!("{}: {err}", p.display()));
            assert!(!cases.is_empty(), "{} is empty", p.display());
            n += 1;
        }
        assert_eq!(n, 127, "expected 127 suite files");
    }
}
