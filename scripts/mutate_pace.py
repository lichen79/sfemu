#!/usr/bin/env python3
"""Mutation pass over crates/frontend/src/pace.rs.

One replacement per mutant, applied to a `shutil.copy` backup and reverted the
same way -- never `git checkout`, which would destroy uncommitted work elsewhere
in the tree. Every mutant asserts its pattern occurs exactly once before
replacing; a pattern that is absent or matches twice is a NO-OP, not a result.

The control mutant must SURVIVE. A pass in which everything dies is more likely
broken than thorough.
"""

import shutil
import subprocess
import sys

SRC = "crates/frontend/src/pace.rs"
BACKUP = "/tmp/pace.rs.orig"

# (name, old, new, expectation)
MUTANTS = [
    ("dropped-never-counts", "self.dropped += owed - served;", "self.dropped += 0;", "KILL"),
    (
        "no-cap",
        "let served = u64::from(self.max_catch_up).min(owed);",
        "let served = owed;",
        "KILL",
    ),
    ("remainder-discarded", "self.owed_ns %= self.frame_ns;", "self.owed_ns = 0;", "KILL"),
    (
        "debt-not-accumulated",
        "self.owed_ns = self.owed_ns.saturating_add(elapsed_ns);",
        "self.owed_ns = elapsed_ns;",
        "KILL",
    ),
    ("wrong-period", "pub const FRAME_NS: u64 = 16_768_000;", "pub const FRAME_NS: u64 = 16_667_000;", "KILL"),
    ("wrong-cap", "pub const MAX_CATCH_UP: u32 = 4;", "pub const MAX_CATCH_UP: u32 = 5;", "KILL"),
    (
        "reset-clears-record-not-debt",
        "    pub fn reset(&mut self) {\n        self.owed_ns = 0;\n    }",
        "    pub fn reset(&mut self) {\n        self.dropped = 0;\n    }",
        "KILL",
    ),
    (
        "zero-period-guard-removed",
        "        if self.frame_ns == 0 {\n            return 0;\n        }\n",
        "",
        "KILL",
    ),
    # Control: nothing compares two pacers, so dropping the equality derives
    # changes no behaviour. This must survive.
    (
        "CONTROL-drop-eq-derives",
        "#[derive(Debug, Clone, PartialEq, Eq)]\npub struct FramePacer {",
        "#[derive(Debug, Clone)]\npub struct FramePacer {",
        "SURVIVE",
    ),
]


def main() -> int:
    shutil.copy(SRC, BACKUP)
    rows = []
    try:
        for name, old, new, expect in MUTANTS:
            src = open(BACKUP).read()
            n = src.count(old)
            if n != 1:
                rows.append((name, expect, f"NO-OP ({n} matches)"))
                continue
            open(SRC, "w").write(src.replace(old, new, 1))
            r = subprocess.run(
                ["cargo", "test", "-p", "frontend", "--quiet"],
                capture_output=True,
                text=True,
            )
            got = "SURVIVE" if r.returncode == 0 else "KILL"
            rows.append((name, expect, got))
    finally:
        shutil.copy(BACKUP, SRC)

    bad = 0
    for name, expect, got in rows:
        ok = got == expect
        bad += not ok
        print(f"{'ok  ' if ok else 'BAD '} {name:34} expect {expect:8} got {got}")
    print(f"\n{len(rows) - bad}/{len(rows)} as expected")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
