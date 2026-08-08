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

# Which crate's tests each set is scored against. A set omitted here is scored
# against `frontend`, which is where this harness started. Naming the crate
# matters: scoring a mutation of `machine` against `frontend`'s tests would report
# SURVIVE for every mutant, since those tests never load the mutated code.
CRATES: dict[str, str] = {"snapshot": "machine"}

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
    "pixels": (
        "crates/frontend/src/pixels.rs",
        [
            (
                "red-and-blue-swapped",
                "    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)\n}",
                "    (u32::from(b) << 16) | (u32::from(g) << 8) | u32::from(r)\n}",
                "KILL",
            ),
            (
                "red-in-the-alpha-byte",
                "    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)\n}",
                "    (u32::from(r) << 24) | (u32::from(g) << 8) | u32::from(b)\n}",
                "KILL",
            ),
            (
                "green-not-shifted",
                "    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)\n}",
                "    (u32::from(r) << 16) | u32::from(g) | u32::from(b)\n}",
                "KILL",
            ),
            ("buffer-never-cleared", "    out.clear();\n", "", "KILL"),
            (
                "only-part-of-the-frame-converted",
                "out.extend(v.fb.pens.iter().map",
                "out.extend(v.fb.pens.iter().take(1000).map",
                "KILL",
            ),
            (
                "every-pixel-takes-pen-zero",
                "out.extend(v.fb.pens.iter().map(|&pen| argb(pal[usize::from(pen)])));",
                "out.extend(v.fb.pens.iter().map(|&_pen| argb(pal[0])));",
                "KILL",
            ),
            # Control: the palette is read once into a local either way. Reading it
            # per pixel is slower and observably identical -- nothing here can tell
            # the two apart, which is the point of a control.
            (
                "CONTROL-palette-read-per-pixel",
                "    let pal = v.palette();\n    out.clear();\n    out.extend(v.fb.pens.iter().map(|&pen| argb(pal[usize::from(pen)])));",
                "    out.clear();\n    let pens = v.fb.pens.clone();\n    out.extend(pens.iter().map(|&pen| argb(v.palette()[usize::from(pen)])));",
                "SURVIVE",
            ),
        ],
    ),
    # Task 4: the snapshot. Every mutant here drops or corrupts one field, which is
    # the whole failure mode a save state has -- and the reason the load-bearing test
    # is a divergence test rather than `snapshot == snapshot`.
    "snapshot": (
        "crates/machine/src/cps1.rs",
        [
            # The three private fields, each dropped from `restore`.
            ("carry-not-restored", "        self.carry = s.carry;\n", "", "KILL"),
            (
                "vblank-not-restored",
                "        self.board.set_vblank_pending(s.vblank_pending);\n",
                "",
                "KILL",
            ),
            (
                "obj-latch-not-restored",
                "        self.video.set_obj_latch(&s.obj);\n",
                "",
                "KILL",
            ),
            # And each dropped from `snapshot`, which the divergence test must also
            # catch: a state that never carried the field restores just as wrong.
            ("carry-not-captured", "            carry: self.carry,", "            carry: 0,", "KILL"),
            (
                "vblank-not-captured",
                "            vblank_pending: self.board.vblank_pending(),",
                "            vblank_pending: false,",
                "KILL",
            ),
            (
                "obj-latch-not-captured",
                "            obj: self.video.obj_latch().clone(),",
                "            obj: video::sprites::ObjLatch::new(),",
                "KILL",
            ),
            # The bulk memory, and the scheduler's coarse position.
            ("ram-not-restored", "        self.board.ram.copy_from_slice(&s.ram[..]);\n", "", "KILL"),
            (
                "gfxram-not-restored",
                "        self.board.gfxram.copy_from_slice(&s.gfxram[..]);\n",
                "",
                "KILL",
            ),
            ("line-not-restored", "        self.line = s.line;\n", "", "KILL"),
            ("cpu-not-restored", "        self.cpu = s.cpu.clone();\n", "", "KILL"),
            (
                "total-cycles-not-restored",
                "        self.total_cycles = s.total_cycles;\n",
                "",
                "KILL",
            ),
            # A probe, not a control. If held inputs survive being dropped, the fix is
            # a test for restoring them -- not accepting the survivor.
            (
                "PROBE-inputs-not-captured",
                "            inputs: self.board.inputs,",
                "            inputs: crate::Inputs::idle(),",
                "KILL",
            ),
            # A restore written in terms of `assert_vblank` would inflate the trace's
            # vblank count on every load. This is why `set_vblank_pending` exists.
            (
                "restore-re-asserts-the-interrupt",
                "        self.board.set_vblank_pending(s.vblank_pending);",
                "        if s.vblank_pending {\n            self.board.assert_vblank();\n        }",
                "KILL",
            ),
            # Control: the trace is deliberately absent from a state, and
            # `a_snapshot_carries_no_rom_and_does_not_rewind_the_trace` is what pins
            # that. Restoring `cps_b` twice is a no-op -- idempotent, observably
            # identical, and exactly what a control should be.
            (
                "CONTROL-restore-cps-b-twice",
                "        self.board.cps_b = s.cps_b;\n",
                "        self.board.cps_b = s.cps_b;\n        self.board.cps_b = s.cps_b;\n",
                "SURVIVE",
            ),
        ],
    ),
    "state": (
        "crates/frontend/src/state.rs",
        [
            # The five refusals, each defeated in turn. A codec that accepts
            # everything is the failure mode a user only discovers when a state
            # loads as garbage.
            (
                "crc-check-always-passes",
                "    if found_crc != computed {",
                "    if false && found_crc != computed {",
                "KILL",
            ),
            (
                "version-check-always-passes",
                "    if bytes[7] != VERSION {",
                "    if false && bytes[7] != VERSION {",
                "KILL",
            ),
            (
                "magic-check-always-passes",
                "    if bytes[..7] != MAGIC[..7] {",
                "    if false && bytes[..7] != MAGIC[..7] {",
                "KILL",
            ),
            (
                "board-check-always-passes",
                "    if found_board != board {",
                "    if false && found_board != board {",
                "KILL",
            ),
            (
                "declared-length-check-removed",
                "    if bytes.len() < need {",
                "    if false && bytes.len() < need {",
                "KILL",
            ),
            # The three fields a save state is most likely to forget: two are
            # private on `Cps1` and one is a one-frame delay, so all three are
            # invisible to a whole-frame comparison. `machine`'s pass proved
            # `snapshot` carries them; this proves the *bytes* do.
            (
                "carry-omitted-from-the-payload",
                "    w.i64(s.carry);",
                "    w.i64(0);",
                "KILL",
            ),
            (
                "obj-latch-omitted-from-the-payload",
                "    w.words(s.obj.words());",
                "    w.words(&[0u16; OBJ_WORDS]);",
                "KILL",
            ),
            (
                "vblank-pending-omitted-from-the-payload",
                "    w.bool(s.vblank_pending);",
                "    w.bool(false);",
                "KILL",
            ),
            # Two adjacent same-width fields written in the other order. A
            # divergence test cannot see this -- the guest never reads either --
            # which is why `every_field_survives_the_round_trip` exists.
            (
                "two-adjacent-fields-swapped",
                "    w.bool(s.inputs.start1);\n    w.bool(s.inputs.start2);",
                "    w.bool(s.inputs.start2);\n    w.bool(s.inputs.start1);",
                "KILL",
            ),
            (
                "cpu-flag-bytes-swapped",
                "    w.bool(s.cpu.halted);\n    w.bool(s.cpu.stopped);",
                "    w.bool(s.cpu.stopped);\n    w.bool(s.cpu.halted);",
                "KILL",
            ),
            # The forward polynomial instead of the reflected one: a CRC that is
            # self-consistent, so only the spec's check value catches it.
            (
                "unreflected-crc-polynomial",
                "crc = (crc >> 1) ^ (0xEDB8_8320 & mask);",
                "crc = (crc >> 1) ^ (0x04C1_1DB7 & mask);",
                "KILL",
            ),
            # A boolean read as `== 1` rather than `!= 0`. One character, and the
            # documented permissiveness is gone.
            (
                "boolean-read-as-exactly-one",
                "        self.u8() != 0\n",
                "        self.u8() == 1\n",
                "KILL",
            ),
            # CONTROL: the wording of a refusal, not its existence. Nothing
            # asserts this text, and nothing should -- pinning a human-readable
            # message makes every rewording a test failure.
            (
                "CONTROL-display-text-reworded",
                'write!(f, "not a save state (wrong magic)")',
                'write!(f, "this file is not a save state")',
                "SURVIVE",
            ),
        ],
    ),
}


def run(name: str) -> int:
    src, mutants = SETS[name]
    crate = CRATES.get(name, "frontend")
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
                ["cargo", "test", "-p", crate, "--quiet"],
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
