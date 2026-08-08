#!/usr/bin/env python3
"""A mutation-testing harness for this workspace.

Usage: mutate.py <mutant-set-name>

Each mutant is one exact string replacement in one file. Discipline this
enforces, all of it learned the hard way:

- `shutil.copy` for the backup and the revert, **never** `git checkout` --
  reverting with git would destroy uncommitted work elsewhere in that file.
- The pattern must occur **exactly once**. Zero or two matches is reported as
  NO-OP, not as a result: a mutant that did not apply tells you nothing, and
  silently counting it as a kill inflates the score.
- Every set includes at least one **control** mutant that must SURVIVE. A pass
  where everything dies is more likely a broken harness than a thorough suite.
"""

import shutil
import subprocess
import sys

# name -> (file, [(mutant-name, old, new, expectation), ...])
SETS: dict[str, tuple[str, list[tuple[str, str, str, str]]]] = {
    "pace": (
        "crates/frontend/src/pace.rs",
        [
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
            (
                "wrong-period",
                "pub const FRAME_NS: u64 = 16_768_000;",
                "pub const FRAME_NS: u64 = 16_667_000;",
                "KILL",
            ),
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
            (
                "CONTROL-drop-eq-derives",
                "#[derive(Debug, Clone, PartialEq, Eq)]\npub struct FramePacer {",
                "#[derive(Debug, Clone)]\npub struct FramePacer {",
                "SURVIVE",
            ),
        ],
    ),
    "keys": (
        "crates/frontend/src/keys.rs",
        [
            # The edge/level asymmetry, from both directions.
            (
                "edge-becomes-level",
                "let edge = |k: Key| now.contains(k) && !self.was.contains(k);",
                "let edge = |k: Key| now.contains(k);",
                "KILL",
            ),
            (
                "previous-frame-never-stored",
                "        self.was = now;\n        actions",
                "        let _ = now;\n        actions",
                "KILL",
            ),
            (
                "pause-is-level-triggered",
                "pause_toggled: edge(Key::P),",
                "pause_toggled: now.contains(Key::P),",
                "KILL",
            ),
            (
                "step-is-level-triggered",
                "step: edge(Key::Period),",
                "step: now.contains(Key::Period),",
                "KILL",
            ),
            (
                "save-is-level-triggered",
                "save: edge(Key::F5),",
                "save: now.contains(Key::F5),",
                "KILL",
            ),
            (
                "test-switch-is-edge-triggered",
                "inputs.test = now.contains(Key::F2);",
                "inputs.test = edge(Key::F2);",
                "KILL",
            ),
            # The map itself.
            (
                "kick-reads-a-punch-key",
                "        inputs.p1.kick = [\n            now.contains(Key::Z),",
                "        inputs.p1.kick = [\n            now.contains(Key::A),",
                "KILL",
            ),
            (
                "punch-key-order-swapped",
                "            now.contains(Key::A),\n            now.contains(Key::S),",
                "            now.contains(Key::S),\n            now.contains(Key::A),",
                "KILL",
            ),
            ("coin-is-a-start-key", "inputs.coin1 = now.contains(Key::Num5);", "inputs.coin1 = now.contains(Key::Num1);", "KILL"),
            ("stick-up-is-down", "inputs.p1.up = now.contains(Key::Up);", "inputs.p1.up = now.contains(Key::Down);", "KILL"),
            # A P2 field written by a P1 key: the absence test must catch it.
            (
                "a-key-reaches-player-two",
                "inputs.p1.right = now.contains(Key::Right);",
                "inputs.p2.right = now.contains(Key::Right);",
                "KILL",
            ),
            # Two keys sharing a bit.
            ("two-keys-share-a-bit", "Key::D => 6,", "Key::D => 5,", "KILL"),
            # The DIP switches, which idle() sets and this module must not touch.
            (
                "dip-switches-all-on",
                "let mut inputs = Inputs::idle();",
                "let mut inputs = Inputs {\n            dsw: [0x00; 3],\n            ..Inputs::idle()\n        };",
                "KILL",
            ),
            # Control: the bit *values* are arbitrary -- only their distinctness
            # matters, which is what every_key_has_its_own_slot tests. Moving
            # Escape to another free bit changes nothing observable. Worth having
            # precisely because it looks like it should fail.
            ("CONTROL-escape-moves-to-another-free-bit", "Key::Escape => 21,", "Key::Escape => 25,", "SURVIVE"),
        ],
    ),
}


def run(name: str) -> int:
    src, mutants = SETS[name]
    backup = f"/tmp/mutate-{name}.orig"
    shutil.copy(src, backup)
    rows = []
    try:
        for mname, old, new, expect in mutants:
            text = open(backup).read()
            n = text.count(old)
            if n != 1:
                rows.append((mname, expect, f"NO-OP ({n} matches)"))
                continue
            open(src, "w").write(text.replace(old, new, 1))
            r = subprocess.run(
                ["cargo", "test", "-p", "frontend", "--quiet"],
                capture_output=True,
                text=True,
            )
            rows.append((mname, expect, "SURVIVE" if r.returncode == 0 else "KILL"))
    finally:
        shutil.copy(backup, src)

    bad = 0
    for mname, expect, got in rows:
        ok = got == expect
        bad += not ok
        print(f"{'ok  ' if ok else 'BAD '} {mname:42} expect {expect:8} got {got}")
    print(f"\n{len(rows) - bad}/{len(rows)} as expected")
    return 1 if bad else 0


if __name__ == "__main__":
    if len(sys.argv) != 2 or sys.argv[1] not in SETS:
        print(f"usage: {sys.argv[0]} {{{','.join(SETS)}}}", file=sys.stderr)
        sys.exit(2)
    sys.exit(run(sys.argv[1]))
