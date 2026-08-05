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

    // Address-error transactions must never have reached the bus.
    for t in &case.transactions {
        if matches!(t.kind, TxKind::ReadAddrErr | TxKind::WriteAddrErr) {
            let addr = t.addr & 0x00FF_FFFF;
            if log.iter().any(|a| a.addr == addr) {
                diffs.push(format!(
                    "access at {addr:06X} was committed, but the expected \
                     transaction is an address error (AS never asserted)"
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
        let want_addr = t.addr & 0x00FF_FFFF;
        let want_size = if t.is_word() { Size::Word } else { Size::Byte };
        if a.is_write != want_write || a.addr != want_addr || a.size != want_size {
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
/// Skips with a message when the vectors are absent, so a fresh checkout does
/// not fail before `fetch` has run.
pub fn assert_group(name: &str) {
    let path = testdata_dir().join(format!("{name}.json.bin"));
    if !path.exists() {
        eprintln!("skipping {name}: run `cargo run -p testrunner --bin fetch`");
        return;
    }
    let r = run_group(&path);
    if !r.is_clean() {
        let mut msg = format!("{}: {}/{} passed\n", r.group, r.passed, r.total);
        for f in &r.failures {
            msg.push_str(&f.to_string());
        }
        panic!("{msg}");
    }
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
}
