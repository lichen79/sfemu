//! Runs suite cases against the core and reports differences readably.

use crate::binfmt::{parse_file, State, TestCase};
use crate::testbus::{Access, TestBus};
use m68k::cpu::SR_S;
use m68k::decode::Decoder;
use m68k::M68k;
use std::path::{Path, PathBuf};

/// Groups the suite itself documents as not-yet-correct. Asserted as failing
/// rather than skipped, so an upstream fix surfaces instead of going unnoticed.
pub const KNOWN_BAD: &[&str] = &["TAS", "TRAPV"];

/// Where the fetched vectors live.
pub fn testdata_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata"))
}

#[derive(Debug)]
pub struct Failure {
    pub name: String,
    pub diffs: Vec<String>,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "case {}", self.name)?;
        for d in &self.diffs {
            writeln!(f, "  {d}")?;
        }
        Ok(())
    }
}

fn load_state(cpu: &mut M68k, st: &State) {
    cpu.d = st.d;
    cpu.a[..7].copy_from_slice(&st.a);
    cpu.sr = st.sr;
    cpu.usp = st.usp;
    cpu.ssp = st.ssp;
    // a[7] mirrors whichever stack pointer the S bit selects.
    cpu.a[7] = if st.sr & SR_S != 0 { st.ssp } else { st.usp };
    cpu.pc = st.pc;
    cpu.prefetch = st.prefetch;
    cpu.halted = false;
    cpu.stopped = false;
    cpu.pending_irq = 0;
    cpu.in_exception = false;
}

/// Compares the bus accesses we actually made against the case's expected
/// transactions, in order.
///
/// End state alone can be right for the wrong reasons — a write to the correct
/// address at the wrong point in the sequence, or an extra read the hardware
/// never performs, both leave memory looking correct. Comparing the sequence
/// catches those.
///
/// Two kinds of expected transaction are deliberately not matched against our
/// log: `Idle` (no bus activity to observe) and the address-error kinds, where
/// AS is never asserted and so the access must NOT appear in our log at all.
/// The latter is checked as an absence.
fn compare_accesses(case: &TestCase, log: &[Access]) -> Vec<String> {
    use crate::binfmt::TxKind;
    use crate::testbus::Size;

    let mut diffs = Vec::new();

    // Address-error transactions must never have reached the bus (AS never
    // asserted).  The suite records the *aligned* address; the actual fault
    // address is `recorded | 1` (odd).  A core that wrongly commits the access
    // may do so at either address, so check both.  Also distinguish direction
    // so a legitimate access of the opposite direction is not flagged.
    for t in &case.transactions {
        if matches!(t.kind, TxKind::ReadAddrErr | TxKind::WriteAddrErr) {
            let aligned = t.addr & 0x00FF_FFFF;
            let odd = aligned | 1;
            let want_write = matches!(t.kind, TxKind::WriteAddrErr);
            if log
                .iter()
                .any(|a| (a.addr == aligned || a.addr == odd) && a.is_write == want_write)
            {
                diffs.push(format!(
                    "access at {:06X}/{:06X} was committed, but the expected \
                     transaction is an address error (AS never asserted)",
                    aligned, odd
                ));
            }
        }
    }

    let expected: Vec<_> = case
        .transactions
        .iter()
        .filter(|t| matches!(t.kind, TxKind::Read | TxKind::Write | TxKind::Tas))
        .collect();

    if expected.len() != log.len() {
        diffs.push(format!(
            "bus accesses: made {}, expected {}",
            log.len(),
            expected.len()
        ));
    }

    for (i, (t, a)) in expected.iter().zip(log.iter()).enumerate() {
        let want_write = matches!(t.kind, TxKind::Write);

        // Byte accesses: the suite records a word-aligned address.
        // UDS-only (uds=1, lds=0): the live byte is at `addr`      (upper half).
        // LDS-only (uds=0, lds=1): the live byte is at `addr + 1`  (lower half).
        // A correct core calls read8/write8 at the odd address for LDS; we must
        // compare against that odd address rather than the recorded even one.
        // For word accesses and UDS-byte accesses the recorded address is already
        // correct (even), so both use `t.addr & MASK` directly.
        let want_addr = if !t.is_word() && t.uds == 0 {
            (t.addr | 1) & 0x00FF_FFFF // LDS: byte at odd address
        } else {
            t.addr & 0x00FF_FFFF // word or UDS: use the recorded (even) address
        };
        let want_size = if t.is_word() { Size::Word } else { Size::Byte };

        // Expected data value, strobe-adjusted:
        // - Word:      full 16-bit value.
        // - UDS byte:  upper byte of the data word, shifted into the low byte
        //              position to match how read8/write8 return/accept bare bytes.
        // - LDS byte:  lower byte of the data word.
        let want_data: u16 = if t.is_word() {
            (t.data & 0xFFFF) as u16
        } else if t.uds != 0 {
            ((t.data >> 8) & 0xFF) as u16 // UDS byte, bare
        } else {
            (t.data & 0xFF) as u16 // LDS byte, bare
        };

        let addr_ok = a.addr == want_addr;
        let size_ok = a.size == want_size;
        let dir_ok = a.is_write == want_write;
        let data_ok = a.val == want_data;

        if !dir_ok || !addr_ok || !size_ok {
            let dir = |w| if w { "write" } else { "read" };
            diffs.push(format!(
                "access {i}: got {} {:?} at {:06X}, want {} {:?} at {:06X}",
                dir(a.is_write),
                a.size,
                a.addr,
                dir(want_write),
                want_size,
                want_addr
            ));
        } else if !data_ok {
            diffs.push(format!(
                "access {i}: data got {:04X} want {:04X} at {:06X}",
                a.val, want_data, want_addr
            ));
        }
    }
    diffs
}

/// Runs one case: seed, step once, compare everything the suite specifies.
pub fn run_case(dec: &Decoder, case: &TestCase) -> Result<(), Failure> {
    let mut cpu = M68k::new();
    load_state(&mut cpu, &case.initial);
    let mut bus = TestBus::from_state(&case.initial);

    let cycles = cpu.step_with(dec, &mut bus);

    let mut diffs = Vec::new();
    let want = &case.final_;

    for i in 0..8 {
        if cpu.d[i] != want.d[i] {
            diffs.push(format!("d{i}: got {:08X} want {:08X}", cpu.d[i], want.d[i]));
        }
    }
    for i in 0..7 {
        if cpu.a[i] != want.a[i] {
            diffs.push(format!("a{i}: got {:08X} want {:08X}", cpu.a[i], want.a[i]));
        }
    }

    // Both stack pointers, reading the active one back out of a[7].
    let (got_usp, got_ssp) = if cpu.sr & SR_S != 0 {
        (cpu.usp, cpu.a[7])
    } else {
        (cpu.a[7], cpu.ssp)
    };
    if got_usp != want.usp {
        diffs.push(format!("usp: got {got_usp:08X} want {:08X}", want.usp));
    }
    if got_ssp != want.ssp {
        diffs.push(format!("ssp: got {got_ssp:08X} want {:08X}", want.ssp));
    }
    if cpu.sr != want.sr {
        diffs.push(format!("sr: got {:04X} want {:04X}", cpu.sr, want.sr));
    }
    if cpu.pc != want.pc {
        diffs.push(format!("pc: got {:08X} want {:08X}", cpu.pc, want.pc));
    }
    if cpu.prefetch != want.prefetch {
        diffs.push(format!(
            "prefetch: got [{:04X},{:04X}] want [{:04X},{:04X}]",
            cpu.prefetch[0], cpu.prefetch[1], want.prefetch[0], want.prefetch[1]
        ));
    }
    if cycles != case.length {
        diffs.push(format!("cycles: got {cycles} want {}", case.length));
    }
    for (addr, val) in &want.ram {
        let got = bus.mem.get(addr).copied().unwrap_or(0);
        if got != *val {
            diffs.push(format!("ram[{addr:06X}]: got {got:02X} want {val:02X}"));
        }
    }

    diffs.extend(compare_accesses(case, &bus.log));

    if diffs.is_empty() {
        Ok(())
    } else {
        // A wall of diffs for one case is noise; the first few identify the bug.
        diffs.truncate(12);
        Err(Failure {
            name: case.name.clone(),
            diffs,
        })
    }
}

pub struct GroupResult {
    pub group: String,
    pub total: usize,
    pub passed: usize,
    pub failures: Vec<Failure>,
}

impl GroupResult {
    pub fn is_clean(&self) -> bool {
        self.passed == self.total
    }
}

/// Runs every case in one suite file.
pub fn run_group(path: &Path) -> GroupResult {
    let group = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .trim_end_matches(".json.bin")
        .to_string();
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let cases = parse_file(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));

    // One decoder for the whole file: building it fills 65536 entries, which is
    // wasteful to redo 2500 times.
    let dec = Decoder::new();
    let mut passed = 0;
    let mut failures = Vec::new();
    for c in &cases {
        match run_case(&dec, c) {
            Ok(()) => passed += 1,
            Err(f) => {
                if failures.len() < 5 {
                    failures.push(f);
                }
            }
        }
    }
    GroupResult {
        group,
        total: cases.len(),
        passed,
        failures,
    }
}

/// Asserts a group passes completely, printing a readable report if not.
///
/// Fails immediately if the vector file is absent: a missing file is a host
/// fault, not a skip.  Run `cargo run -p testrunner --bin fetch` to populate
/// `testdata/` before running the suite.
///
/// Also fails if the file parsed to zero cases, which would otherwise yield a
/// vacuous pass.
pub fn assert_group(name: &str) {
    let path = testdata_dir().join(format!("{name}.json.bin"));
    assert!(
        path.exists(),
        "missing {}: run `cargo run -p testrunner --bin fetch`",
        path.display()
    );
    let r = run_group(&path);
    assert!(
        r.total > 0,
        "{}: parsed to zero cases — vector file may be corrupt",
        r.group
    );
    if !r.is_clean() {
        let mut msg = format!("{}: {}/{} passed\n", r.group, r.passed, r.total);
        for f in &r.failures {
            msg.push_str(&f.to_string());
        }
        panic!("{msg}");
    }
}

/// Asserts a group is *partially* green: some cases pass and some fail.
///
/// This is the assertion for a group whose vectors are known-bad upstream. It
/// exists because `#[should_panic]` around [`assert_group`] is unsound for that
/// purpose: `assert_group` names the group in **all** of its panic messages, so a
/// deleted or truncated vector file also produces the expected panic and the test
/// passes for entirely the wrong reason — inverting the project's "missing data
/// fails loudly" rule.
///
/// This fails in three distinct ways instead, all of them informative:
///
/// * the file is missing or unparsable — the host is at fault, same as any group;
/// * **zero** cases pass — the handler is broken, not merely the vectors;
/// * **all** cases pass — upstream fixed the vectors, so this group should be
///   promoted to [`assert_group`] and removed from the known-bad list.
pub fn assert_known_bad(name: &str, why: &str) {
    let path = testdata_dir().join(format!("{name}.json.bin"));
    assert!(
        path.exists(),
        "missing {}: run `cargo run -p testrunner --bin fetch`",
        path.display()
    );
    let r = run_group(&path);
    assert!(
        r.total > 0,
        "{}: parsed to zero cases — vector file may be corrupt",
        r.group
    );
    assert!(
        r.passed > 0,
        "{}: 0/{} passed — known-bad upstream ({why}), but a total failure means \
         the handler is broken rather than the vectors",
        r.group,
        r.total
    );
    assert!(
        !r.is_clean(),
        "{}: {}/{} passed — this group is listed as known-bad upstream ({why}) but \
         now passes completely. Upstream has likely been fixed: move it to \
         assert_group and drop it from the known-bad list.",
        r.group,
        r.passed,
        r.total
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binfmt::{Transaction, TxKind};
    use crate::testbus::Size;

    fn tx(kind: TxKind, addr: u32, uds: u32, lds: u32) -> Transaction {
        Transaction {
            kind,
            cycles: 4,
            fc: 2,
            addr,
            data: 0,
            uds,
            lds,
        }
    }

    fn tx_data(kind: TxKind, addr: u32, uds: u32, lds: u32, data: u32) -> Transaction {
        Transaction {
            kind,
            cycles: 4,
            fc: 2,
            addr,
            data,
            uds,
            lds,
        }
    }

    fn case_with(transactions: Vec<Transaction>) -> TestCase {
        TestCase {
            name: "synthetic".into(),
            initial: State::default(),
            final_: State::default(),
            transactions,
            length: 4,
        }
    }

    #[test]
    fn matching_sequence_produces_no_diffs() {
        let case = case_with(vec![
            tx(TxKind::Idle, 0, 0, 0),
            tx(TxKind::Read, 0x1000, 1, 1),
            tx(TxKind::Write, 0x2000, 1, 0),
        ]);
        let log = vec![
            Access {
                is_write: false,
                addr: 0x1000,
                size: Size::Word,
                val: 0,
            },
            Access {
                is_write: true,
                addr: 0x2000,
                size: Size::Byte,
                val: 0,
            },
        ];
        assert!(compare_accesses(&case, &log).is_empty());
    }

    #[test]
    fn out_of_order_accesses_are_reported() {
        let case = case_with(vec![
            tx(TxKind::Read, 0x1000, 1, 1),
            tx(TxKind::Write, 0x2000, 1, 1),
        ]);
        // Same end state, wrong order.
        let log = vec![
            Access {
                is_write: true,
                addr: 0x2000,
                size: Size::Word,
                val: 0,
            },
            Access {
                is_write: false,
                addr: 0x1000,
                size: Size::Word,
                val: 0,
            },
        ];
        assert_eq!(compare_accesses(&case, &log).len(), 2);
    }

    #[test]
    fn a_missing_access_is_reported() {
        let case = case_with(vec![tx(TxKind::Read, 0x1000, 1, 1)]);
        assert!(!compare_accesses(&case, &[]).is_empty());
    }

    /// An address-error access must never reach the bus.
    #[test]
    fn committing_an_address_error_access_is_reported() {
        let case = case_with(vec![tx(TxKind::ReadAddrErr, 0x1001, 1, 1)]);
        let log = vec![Access {
            is_write: false,
            addr: 0x1001,
            size: Size::Word,
            val: 0,
        }];
        let diffs = compare_accesses(&case, &log);
        assert!(
            diffs.iter().any(|d| d.contains("address error")),
            "got {diffs:?}"
        );
    }

    /// A correct address/size/direction but wrong data value must be reported.
    #[test]
    fn wrong_data_value_is_reported() {
        // Word write: expected 0xBEEF, core wrote 0xDEAD.
        let case = case_with(vec![tx_data(TxKind::Write, 0x1000, 1, 1, 0xBEEF)]);
        let log = vec![Access {
            is_write: true,
            addr: 0x1000,
            size: Size::Word,
            val: 0xDEAD,
        }];
        let diffs = compare_accesses(&case, &log);
        assert!(
            diffs.iter().any(|d| d.contains("data")),
            "wrong data must be reported; got {diffs:?}"
        );
    }

    /// UDS-only byte: expected upper half 0xAB, core read 0x00.
    #[test]
    fn wrong_data_uds_byte_is_reported() {
        // UDS-only: data = 0xAB00 on the bus, live byte = 0xAB.
        let case = case_with(vec![tx_data(TxKind::Read, 0x1000, 1, 0, 0xAB00)]);
        let log = vec![Access {
            is_write: false,
            addr: 0x1000, // even address, UDS
            size: Size::Byte,
            val: 0x00, // wrong bare byte
        }];
        let diffs = compare_accesses(&case, &log);
        assert!(
            diffs.iter().any(|d| d.contains("data")),
            "wrong UDS byte must be reported; got {diffs:?}"
        );
    }

    /// LDS-only byte: expected lower half 0xCD, core read at correct odd address.
    #[test]
    fn correct_lds_byte_passes() {
        // LDS-only: data = 0x00CD on the bus, live byte = 0xCD at addr+1.
        let case = case_with(vec![tx_data(TxKind::Read, 0x1000, 0, 1, 0x00CD)]);
        let log = vec![Access {
            is_write: false,
            addr: 0x1001, // odd address, LDS
            size: Size::Byte,
            val: 0xCD, // correct bare byte
        }];
        assert!(
            compare_accesses(&case, &log).is_empty(),
            "correct LDS byte must produce no diffs"
        );
    }
}
