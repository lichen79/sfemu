//! Downloads the SingleStepTests/m68000 `v1/` vectors into `testdata/`.
//!
//! Shells out to `curl` rather than taking an HTTP dependency: this runs once
//! per checkout, and keeping the dependency tree empty is worth more than
//! elegance here. ~138 MB (132 MiB) over 127 files — 137,928,157 bytes, which is
//! 137.9 MB decimal and 131.5 MiB. README.md gives the same figure the same way;
//! it used to say "132 MB" here and "~138 MB" there, describing identical bytes
//! in two units without naming either.

// Each `bin` is its own crate root, so `lib.rs`'s attribute does not reach here.
#![forbid(unsafe_code)]

use std::path::Path;
use std::process::Command;

const RAW: &str = "https://raw.githubusercontent.com/SingleStepTests/m68000/main/v1";

/// The 127 suite files, as `(opcode, sizes)`. A `""` size means the file has no
/// size suffix.
const FILES: &[(&str, &[&str])] = &[
    ("ABCD", &[""]),
    ("ADD", &["b", "w", "l"]),
    ("ADDA", &["w", "l"]),
    ("ADDX", &["b", "w", "l"]),
    ("AND", &["b", "w", "l"]),
    ("ANDItoCCR", &[""]),
    ("ANDItoSR", &[""]),
    ("ASL", &["b", "w", "l"]),
    ("ASR", &["b", "w", "l"]),
    ("BCHG", &[""]),
    ("BCLR", &[""]),
    ("BSET", &[""]),
    ("BSR", &[""]),
    ("BTST", &[""]),
    ("Bcc", &[""]),
    ("CHK", &[""]),
    ("CLR", &["b", "w", "l"]),
    ("CMP", &["b", "w", "l"]),
    ("CMPA", &["w", "l"]),
    ("DBcc", &[""]),
    ("DIVS", &[""]),
    ("DIVU", &[""]),
    ("EOR", &["b", "w", "l"]),
    ("EORItoCCR", &[""]),
    ("EORItoSR", &[""]),
    ("EXG", &[""]),
    ("EXT", &["w", "l"]),
    ("ILLEGAL_LINEA", &[""]),
    ("ILLEGAL_LINEF", &[""]),
    ("JMP", &[""]),
    ("JSR", &[""]),
    ("LEA", &[""]),
    ("LINK", &[""]),
    ("LSL", &["b", "w", "l"]),
    ("LSR", &["b", "w", "l"]),
    ("MOVE", &["b", "w", "l", "q"]),
    ("MOVEA", &["w", "l"]),
    ("MOVEM", &["w", "l"]),
    ("MOVEP", &["w", "l"]),
    ("MOVEfromSR", &[""]),
    ("MOVEfromUSP", &[""]),
    ("MOVEtoCCR", &[""]),
    ("MOVEtoSR", &[""]),
    ("MOVEtoUSP", &[""]),
    ("MULS", &[""]),
    ("MULU", &[""]),
    ("NBCD", &[""]),
    ("NEG", &["b", "w", "l"]),
    ("NEGX", &["b", "w", "l"]),
    ("NOP", &[""]),
    ("NOT", &["b", "w", "l"]),
    ("OR", &["b", "w", "l"]),
    ("ORItoCCR", &[""]),
    ("ORItoSR", &[""]),
    ("PEA", &[""]),
    ("RESET", &[""]),
    ("ROL", &["b", "w", "l"]),
    ("ROR", &["b", "w", "l"]),
    ("ROXL", &["b", "w", "l"]),
    ("ROXR", &["b", "w", "l"]),
    ("RTE", &[""]),
    ("RTR", &[""]),
    ("RTS", &[""]),
    ("SBCD", &[""]),
    ("STOP", &[""]),
    ("SUB", &["b", "w", "l"]),
    ("SUBA", &["w", "l"]),
    ("SUBX", &["b", "w", "l"]),
    ("SWAP", &[""]),
    ("Scc", &[""]),
    ("TAS", &[""]),
    ("TRAP", &[""]),
    ("TRAPV", &[""]),
    ("TST", &["b", "w", "l"]),
    ("UNLINK", &[""]),
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata"));
    std::fs::create_dir_all(dir)?;

    let mut names = Vec::new();
    for (op, sizes) in FILES {
        for s in *sizes {
            names.push(if s.is_empty() {
                format!("{op}.json.bin")
            } else {
                format!("{op}.{s}.json.bin")
            });
        }
    }
    println!("fetching {} files into {}", names.len(), dir.display());

    for (i, name) in names.iter().enumerate() {
        let dest = dir.join(name);
        if dest.exists() {
            continue;
        }
        print!("[{}/{}] {name} ... ", i + 1, names.len());
        let tmp = dir.join(format!("{name}.part"));
        let st = Command::new("curl")
            .args(["-sfL", "--retry", "3", "-o"])
            .arg(&tmp)
            .arg(format!("{RAW}/{name}"))
            .status()?;
        if !st.success() {
            let _ = std::fs::remove_file(&tmp);
            println!("FAILED");
            return Err(format!("curl failed for {name}").into());
        }
        std::fs::rename(&tmp, &dest)?;
        println!("ok");
    }
    println!("done");
    Ok(())
}
