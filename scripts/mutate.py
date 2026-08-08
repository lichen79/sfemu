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
CRATES: dict[str, str] = {
    "snapshot": "machine",
    "peek": "machine",
    "peekcps1": "machine",
    "loop": "sfemu",
    "wiring": "sfemu",
}

# How long one mutant's test run may take before it is declared killed. The whole
# workspace suite is under 3 s in this tree, so 120 is two orders of magnitude of
# headroom -- generous enough that a slow cold build is never mistaken for a hang.
MUTANT_TIMEOUT_S = 120

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
    # E2 Task 3: the font. Two classes of mutant, and the second is the interesting
    # one. The first is ordinary breakage -- wrong pixels, no clipping. The second is
    # a *transposition*: two glyphs' bitmaps swapped. That survives
    # `every_glyph_is_distinct` (they are still distinct) and it survives every panel
    # test, because `read_text` inverts the same table and reads a transposed font
    # back exactly as wrongly as it renders it. Only the hand-typed literals catch it,
    # which is the entire reason they exist.
    "font": (
        "crates/frontend/src/font.rs",
        [
            (
                "eight-and-b-transposed",
                '    g!(".##.", "#..#", ".##.", "#..#", ".##.", "...."), // \'8\'',
                '    g!("###.", "#..#", "###.", "#..#", "###.", "...."), // \'8\'',
                "KILL",
            ),
            (
                "one-pixel-wrong-in-a-hex-digit",
                '    g!(".##.", "#..#", ".###", "...#", ".##.", "...."), // \'9\'',
                '    g!(".##.", "#..#", ".###", "..##", ".##.", "...."), // \'9\'',
                "KILL",
            ),
            # A whole row gone rather than a pixel. Coarser than the mutant above and
            # a different failure: `5` missing its waist is still a distinct glyph, so
            # `every_glyph_is_distinct` cannot see it and only the literals can.
            (
                "a-row-dropped-from-a-hex-digit",
                '    g!("####", "#...", "###.", "...#", "###.", "...."), // \'5\'',
                '    g!("####", "#...", "....", "...#", "###.", "...."), // \'5\'',
                "KILL",
            ),
            # The art parser, from both ends: a mirrored row, and every row blank.
            (
                "art-parsed-right-to-left",
                "            bits |= 1 << (GLYPH_W - 1 - i);",
                "            bits |= 1 << i;",
                "KILL",
            ),
            ("art-parses-to-nothing", "        if b[i] == b'#' {", "        if false {", "KILL"),
            # The spacing. Without the blank column, `#..#` beside `#..#` is one
            # eight-pixel shape and a hex dump reads as a hedge.
            (
                "no-gap-between-glyphs",
                "pub const ADVANCE: usize = GLYPH_W + 1;",
                "pub const ADVANCE: usize = GLYPH_W;",
                "KILL",
            ),
            # Clipping, which is what stops a glyph at the right edge reappearing on
            # the far left one row down -- a bug that reads as a fault in the *game*.
            (
                "no-horizontal-clip",
                "                if px >= WIDTH {\n                    continue;\n                }",
                "                let px = px % WIDTH;",
                "KILL",
            ),
            (
                "no-vertical-clip-in-fill-rect",
                "    for py in y..(y + h).min(HEIGHT) {",
                "    for py in y..(y + h) {",
                "KILL",
            ),
            # The cursor advancing only for glyphs that were drawn, which slides the
            # tail of a clipped string into the middle of the screen.
            (
                "cursor-does-not-advance-past-the-edge",
                "        cx += ADVANCE;",
                "        if cx < WIDTH {\n            cx += ADVANCE;\n        }",
                "KILL",
            ),
            # The cursor not advancing at all: every character of a string drawn over
            # the first, which is one illegible blob rather than a register dump.
            (
                "the-cursor-never-advances",
                "        cx += ADVANCE;",
                "        cx += 0;",
                "KILL",
            ),
            # `fill_rect` one column too wide. Not caught by this module's own clipping
            # test -- that one fills at the right edge, where the extra column is
            # clipped away anyway. It is `overlay`'s
            # `a_panel_leaves_the_rest_of_the_frame_alone` that sees it, which is the
            # point: a panel background one pixel wider than its box eats into its
            # neighbour.
            (
                "fill-rect-one-column-too-wide",
                "        for px in x..(x + w).min(WIDTH) {",
                "        for px in x..(x + w + 1).min(WIDTH) {",
                "KILL",
            ),
            # Text painting its own background, which would leave a rectangle of it
            # around every character on top of the game frame.
            (
                "text-paints-its-own-background",
                "                if bits & (1 << (GLYPH_W - 1 - col)) == 0 {\n                    continue;\n                }",
                "                let ink = bits & (1 << (GLYPH_W - 1 - col)) != 0;",
                "KILL",
            ),
            # The out-of-range fallback: an unprintable character drawing nothing is
            # indistinguishable from an empty string.
            (
                "unprintable-draws-a-space",
                "        return GLYPHS['?' as usize - FIRST as usize];",
                "        return GLYPHS[0];",
                "KILL",
            ),
            # CONTROL: `LINE`, which nothing in this module uses -- it is `overlay.rs`
            # that lays lines out, and Task 4 is what will pin it. Changing it here is
            # observably identical to this module's tests, and saying so is more
            # honest than pretending the font set covers it.
            (
                "CONTROL-line-height-unused-until-task-4",
                "pub const LINE: usize = GLYPH_H + 1;",
                "pub const LINE: usize = GLYPH_H + 2;",
                "SURVIVE",
            ),
        ],
    ),
    # E2 Task 4: the panels. The failure mode a debugger has is not a crash, it is
    # showing you a plausible wrong number -- an address four bytes off, a shadow
    # stack pointer, a gap rendered as data. Every mutant here is one of those.
    "overlay": (
        "crates/frontend/src/overlay.rs",
        [
            # The prefetch offset, which is the load-bearing arithmetic in this module.
            # `pc` itself, and the offset in the wrong direction: both show an address
            # that looks entirely reasonable.
            (
                "pc-not-adjusted-for-prefetch",
                "    m.cpu.pc.wrapping_sub(4)",
                "    m.cpu.pc",
                "KILL",
            ),
            (
                "prefetch-offset-added",
                "    m.cpu.pc.wrapping_sub(4)",
                "    m.cpu.pc.wrapping_add(4)",
                "KILL",
            ),
            (
                "prefetch-offset-one-word",
                "    m.cpu.pc.wrapping_sub(4)",
                "    m.cpu.pc.wrapping_sub(2)",
                "KILL",
            ),
            # A7 from the shadow rather than the active pointer. Wrong precisely when
            # you are inside an exception handler, which is when you are reading it.
            (
                "a7-read-from-the-shadow",
                'let line = format!("D{i} {:08X} A{i} {:08X}", m.cpu.d[i], m.cpu.a[i]);',
                'let line = format!("D{i} {:08X} A{i} {:08X}", m.cpu.d[i], if i == 7 { m.cpu.ssp } else { m.cpu.a[i] });',
                "KILL",
            ),
            # The two register labels swapped, values left where they were: the panel
            # shows every data register under an address register's name. Invisible to
            # any test that read the numbers without reading the label beside them,
            # which is why `read_text` reads the whole row.
            (
                "the-register-labels-are-swapped",
                'let line = format!("D{i} {:08X} A{i} {:08X}", m.cpu.d[i], m.cpu.a[i]);',
                'let line = format!("A{i} {:08X} D{i} {:08X}", m.cpu.d[i], m.cpu.a[i]);',
                "KILL",
            ),
            # The listing advancing by a constant rather than by the instruction's
            # length: every line after the first lands mid-instruction and disassembles
            # to something that was never there.
            (
                "listing-advances-by-a-word",
                "        a = a.wrapping_add(insn.len);",
                "        a = a.wrapping_add(2);",
                "KILL",
            ),
            # The two markers, each defeated. A marker on every line is as useless as
            # none, and a marker in the wrong colour is invisible on a dark panel.
            (
                "every-line-is-marked-as-executing",
                "        let marker = if a == pc { '>' } else { ' ' };",
                "        let marker = '>';",
                "KILL",
            ),
            (
                "breakpoints-are-never-marked",
                "        let brk = if bp.contains(&a) { '*' } else { ' ' };",
                "        let brk = ' ';",
                "KILL",
            ),
            (
                "the-breakpoint-marker-is-not-recoloured",
                '        if brk == \'*\' {\n            draw_text(buf, x + ADVANCE, y + row * LINE, "*", BP);\n        }',
                "",
                "KILL",
            ),
            # The gap/all-ones distinction, from both sides. A dump that renders an
            # undecoded address as data sends you looking for a chip that is not there;
            # one that renders a decoded 0xFFFF as a gap loses the fact that something
            # answered.
            (
                "a-gap-is-shown-as-data",
                "                None => line.push_str(\"   --\"),",
                "                None => line.push_str(\" FFFF\"),",
                "KILL",
            ),
            (
                "all-ones-is-shown-as-a-gap",
                "                Some(v) => line.push_str(&format!(\" {v:04X}\")),",
                "                Some(v) => line.push_str(if v == 0xFFFF { \"   --\" } else { &format!(\" {v:04X}\") }),",
                "KILL",
            ),
            # The debugger reading through the CPU's path. This is the mutant Task 2's
            # `&self` cannot prevent -- the signature stops `peek_word` from having side
            # effects, not a caller from reaching for something else. `read16` needs
            # `&mut`, which `draw` does not have, so the reachable form is a *panel*
            # that stops using `peek_word`'s honest `None`.
            (
                "the-dump-invents-a-value-for-a-gap",
                "            match m.peek_word(a) {",
                "            match m.peek_word(a).or(Some(0)) {",
                "KILL",
            ),
            # The status line: HALT and STOP are different machines, and confusing them
            # sends you to the wrong question entirely.
            (
                "halted-and-stopped-confused",
                '    let run = if m.cpu.halted {\n        "HALT"\n    } else if m.cpu.stopped {\n        "STOP"',
                '    let run = if m.cpu.halted {\n        "STOP"\n    } else if m.cpu.stopped {\n        "HALT"',
                "KILL",
            ),
            (
                "a-halted-cpu-looks-like-it-is-running",
                "    let run = if m.cpu.halted {",
                "    let run = if false {",
                "KILL",
            ),
            (
                "flags-read-inverted",
                "    let f = |bit: u16, c: char| if m.cpu.sr & bit != 0 { c } else { '-' };",
                "    let f = |bit: u16, c: char| if m.cpu.sr & bit == 0 { c } else { '-' };",
                "KILL",
            ),
            (
                "two-flag-bits-swapped",
                "        f(0x0004, 'Z'),\n        f(0x0002, 'V'),",
                "        f(0x0002, 'Z'),\n        f(0x0004, 'V'),",
                "KILL",
            ),
            # The layout. Two boxes on the same pixels hides one behind the other --
            # invisible to a test that draws one panel at a time.
            (
                "the-disassembly-overlaps-the-top-band",
                "const TOP_ROWS: usize = taller(REGS_ROWS, MEM_ROWS);",
                "const TOP_ROWS: usize = REGS_ROWS;",
                "KILL",
            ),
            (
                "a-panel-runs-off-the-bottom",
                "pub const STATUS_Y: usize = HEIGHT - LINE - 2 * PAD - 1;",
                "pub const STATUS_Y: usize = HEIGHT - LINE;",
                "KILL",
            ),
            # The off switch. An overlay that drew its background regardless would
            # black out the game when switched off.
            (
                "panels-draw-even-when-disabled",
                "    if p.regs {\n        draw_regs(buf, m);\n    }",
                "    draw_regs(buf, m);",
                "KILL",
            ),
            # CONTROL: the background colour. Nothing pins it and nothing should -- the
            # tests read text in `FG`, `HI`, and `BP`, which is the contrast that
            # matters; the exact dark behind it is taste. Legibility on a real display
            # is the one criterion in this sub-project a test cannot settle, and the
            # README says so.
            (
                "CONTROL-background-shade",
                "const BG: u32 = 0x0000_0020;",
                "const BG: u32 = 0x0010_0018;",
                "SURVIVE",
            ),
        ],
    ),
    # E2 Task 5: the debugger's state. Every mutant here is a debugger that still
    # works -- it draws, it scrolls, it stops -- and lies about one thing. The two
    # worst are in the set: a breakpoint compared against `cpu.pc`, which fires at an
    # address that is not an instruction boundary, and a suppression that never
    # expires, which is a breakpoint you can reach and never pass.
    "debug": (
        "crates/frontend/src/debug.rs",
        [
            # The prefetch, in the one place it decides whether the machine stops.
            # `cpu.pc` is four bytes past the executing instruction, so this fires late
            # -- and for the fixture's three-word instruction, at 0x1004, which is the
            # middle of it.
            (
                "a-breakpoint-is-compared-against-the-pc",
                "self.stopped_at != Some(m.total_cycles) && self.breakpoints.contains(&executing_pc(m))",
                "self.stopped_at != Some(m.total_cycles) && self.breakpoints.contains(&m.cpu.pc)",
                "KILL",
            ),
            # The same error one step earlier: `F7` marking the PC rather than the
            # instruction. Distinct from the mutant above, because a debugger that set
            # and compared *both* at the PC would be self-consistent and still stop two
            # instructions from where you asked.
            (
                "f7-marks-the-pc-not-the-instruction",
                "            let at = executing_pc(m);",
                "            let at = m.cpu.pc;",
                "KILL",
            ),
            # The suppression that never expires: once a breakpoint has fired, it never
            # fires again for the rest of the session. Which is worse than it sounds --
            # you get exactly one stop and then the debugger silently does nothing.
            (
                "the-suppression-never-expires",
                "self.stopped_at != Some(m.total_cycles) && self.breakpoints.contains(&executing_pc(m))",
                "self.stopped_at.is_none() && self.breakpoints.contains(&executing_pc(m))",
                "KILL",
            ),
            # The suppression recorded as an address rather than as this stop's cycle
            # count. `should_break` still compares against `total_cycles`, so the two
            # never agree and the breakpoint re-fires on the instruction it stopped at,
            # forever: set it and you can never get past it.
            (
                "note-stopped-records-the-address",
                "        self.stopped_at = Some(m.total_cycles);",
                "        self.stopped_at = Some(u64::from(executing_pc(m)));",
                "KILL",
            ),
            # The focus not reaching the scroll. Both panels move together, which reads
            # as `F6` being broken rather than as the scroll being wrong.
            (
                "the-focus-does-not-affect-which-panel-scrolls",
                "    fn scroll(&mut self, m: &Cps1, forward: bool) {\n        match self.focus {",
                "    fn scroll(&mut self, m: &Cps1, forward: bool) {\n        match Focus::Disasm {",
                "KILL",
            ),
            (
                "the-scroll-directions-are-swapped",
                "                self.disasm_at = Some(if forward {\n                    from.wrapping_add(DIS_PAGE)\n                } else {\n                    from.wrapping_sub(DIS_PAGE)",
                "                self.disasm_at = Some(if forward {\n                    from.wrapping_sub(DIS_PAGE)\n                } else {\n                    from.wrapping_add(DIS_PAGE)",
                "KILL",
            ),
            # The first scroll of a following listing has to materialise an address, and
            # the address is the one on screen. Starting from zero sends the listing to
            # the reset vector, nowhere near what you were reading.
            (
                "the-first-scroll-starts-from-zero",
                "                let from = self.disasm_at.unwrap_or_else(|| executing_pc(m));",
                "                let from = self.disasm_at.unwrap_or(0);",
                "KILL",
            ),
            # `Home` as `Some(pc)` rather than `None`. Identical on the frame it is
            # pressed and wrong on the very next step: the listing stops following.
            (
                "home-sets-the-current-pc-rather-than-following",
                "                Focus::Disasm => self.disasm_at = None,",
                "                Focus::Disasm => self.disasm_at = Some(executing_pc(m)),",
                "KILL",
            ),
            # `Home` on the dump reading the shadow stack pointer: a plausible address,
            # wrong exactly inside an exception handler, which is where you press it.
            (
                "home-sends-the-dump-to-the-shadow-pointer",
                "                Focus::Mem => self.mem_at = m.cpu.a[7],",
                "                Focus::Mem => self.mem_at = m.cpu.ssp,",
                "KILL",
            ),
            # A toggle that clears the list instead of removing one entry: every other
            # breakpoint lost on the press that was meant to clear one.
            (
                "clearing-one-breakpoint-clears-them-all",
                "                self.breakpoints.remove(i);",
                "                self.breakpoints.clear();",
                "KILL",
            ),
            (
                "f1-only-ever-turns-the-overlay-on",
                "            self.panels = if self.panels.any() {\n                Panels::none()\n            } else {\n                Panels::on()\n            };",
                "            self.panels = Panels::on();",
                "KILL",
            ),
            # `F6` focusing the dump without showing it. The default panel set does not
            # include the dump, so the key that moves the focus to memory appears to do
            # nothing at all -- and it is the key you press to look at memory.
            (
                "focusing-the-dump-does-not-show-it",
                "            if self.focus == Focus::Mem {\n                self.panels.mem = true;\n            }",
                "",
                "KILL",
            ),
            # The focus stuck: `F6` moves it to memory and never back.
            (
                "the-focus-does-not-cycle-back",
                "            Focus::Mem => Focus::Disasm,",
                "            Focus::Mem => Focus::Mem,",
                "KILL",
            ),
            # EQUIVALENT, and documented rather than dressed up as a control: the guard
            # in `draw` really is redundant, because `overlay::draw` with `Panels::none()`
            # draws nothing (`nothing_enabled_draws_nothing` proves it). What the guard
            # buys is the `buf.len()` assert not firing on a frame-sized buffer the
            # caller has yet to fill -- which no test reaches, and which the loop's own
            # ordering makes unreachable. Kept in the set so that the day the guard
            # becomes load-bearing, this line stops surviving and says so.
            (
                "EQUIVALENT-the-drawing-guard-is-redundant",
                "        if !self.panels.any() {\n            return;\n        }\n",
                "",
                "SURVIVE",
            ),
            # CONTROL: how far a page of disassembly moves. What is pinned is that a
            # page forward and a page back cancel, which any constant satisfies; the
            # distance itself is taste, and the docs say two bytes per row is a
            # compromise rather than a fact. The tests spell it `DIS_PAGE` for exactly
            # this reason.
            (
                "CONTROL-disassembly-page-size",
                "const DIS_PAGE: u32 = (DIS_ROWS * 2) as u32;",
                "const DIS_PAGE: u32 = (DIS_ROWS * 3) as u32;",
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
            # A debugger key made level-triggered. It lives in this set rather than in
            # `debug`, because this is the file where a key becomes an edge -- `debug.rs`
            # is handed an `Actions` and never sees a keyboard. It is killed from both
            # ends: `keys`'s own `a_held_key_acts_once` and `debug`'s
            # `a_held_debugger_key_acts_once`, which is what proves the debugger is
            # driven by the edge rather than by the level.
            (
                "breakpoint-is-level-triggered",
                "breakpoint_toggled: edge(Key::F7),",
                "breakpoint_toggled: now.contains(Key::F7),",
                "KILL",
            ),
            (
                "instruction-step-is-level-triggered",
                "step_instruction: edge(Key::F4),",
                "step_instruction: now.contains(Key::F4),",
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
            #
            # ⚠️ 30, and not 25, which is what this said until Task 5 gave bit 25 to
            # `F7`. The control then *died*, correctly: it had quietly become a
            # two-keys-share-a-bit mutant, which the suite is supposed to kill. This is
            # the failure mode a control exists to expose in the harness itself, and it
            # is why `--all` is run rather than one set at a time. 29 keys hold bits
            # 0-28, so 30 is free and stays free unless a key is added -- at which
            # point this control dies again and says so.
            ("CONTROL-escape-moves-to-another-free-bit", "Key::Escape => 21,", "Key::Escape => 30,", "SURVIVE"),
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
    # E2 Task 2: the debugger's read path. Every mutant here is a way for a read that
    # is supposed to observe the machine to *change* it instead, or to observe a
    # different machine than the one running. Both are silent: a debugger that
    # acknowledges the interrupt it is watching still displays plausible numbers.
    "peek": (
        "crates/machine/src/board.rs",
        [
            # The CPU's own bookkeeping, removed. Note what is *absent* from this set:
            # a mutant that gives `peek_word` a side effect. There is no way to write
            # one -- `&self` refuses it at compile time, so the failure mode is
            # unrepresentable rather than merely untested.
            (
                "read-no-longer-acknowledges",
                "        self.note_possible_ack(addr);\n        let v = self.peek_word(addr);",
                "        let v = self.peek_word(addr);",
                "KILL",
            ),
            (
                "read-no-longer-records-the-unmapped-access",
                "            self.trace.unmapped_reads.record(addr);\n",
                "",
                "KILL",
            ),
            # The two answers a debugger must be able to tell apart. Returning a value
            # for an undecoded address makes a memory panel show 0xFFFF where the
            # honest answer is "nothing lives here" -- and the CPU's own trace of
            # unmapped reads goes empty at the same time, since `read_word` reads
            # `is_none()`.
            (
                "undecoded-reads-as-all-ones",
                "            _ => None,\n        }\n    }",
                "            _ => Some(UNMAPPED),\n        }\n    }",
                "KILL",
            ),
            # One arm of the map wrong: RAM read from the ROM image. Peek and read
            # agree with each other perfectly here -- they share the map -- so only a
            # test that knows what RAM should contain catches it.
            (
                "ram-peeks-into-the-rom",
                "            0xFF_0000..=0xFF_FFFF => Some(self.ram[Self::ram_index(addr)]),",
                "            0xFF_0000..=0xFF_FFFF => Some(u16::from(self.rom[Self::ram_index(addr)])),",
                "KILL",
            ),
            # A computed I/O range answered from the wrong side of the word. The
            # interesting case, because it is the one the design worried could not be
            # `&self` at all.
            (
                "dip-byte-in-the-low-half",
                "                Some((u16::from(byte) << 8) | 0x00FF)",
                "                Some(u16::from(byte) | 0xFF00)",
                "KILL",
            ),
            # CONTROL: `note_possible_ack` hoisted back inside a ROM-range guard. The
            # address it tests for is 0x68, which is in ROM space, so the guard is
            # redundant and this is observably identical. It is the right control for
            # this set because it looks exactly like the shape the refactor removed.
            (
                "CONTROL-ack-back-inside-a-rom-guard",
                "        self.note_possible_ack(addr);\n",
                "        if addr <= 0x3F_FFFF {\n            self.note_possible_ack(addr);\n        }\n",
                "SURVIVE",
            ),
        ],
    ),
    # The delegate on `Cps1`, which is what the debugger actually calls. A separate
    # set because it is a different file: `peek` proves the board's map and side
    # effects, this proves the one line between them is not a stub.
    "peekcps1": (
        "crates/machine/src/cps1.rs",
        [
            (
                "the-delegate-answers-nothing",
                "    pub fn peek_word(&self, addr: u32) -> Option<u16> {\n        self.board.peek_word(addr)",
                "    pub fn peek_word(&self, addr: u32) -> Option<u16> {\n        let _ = addr;\n        None",
                "KILL",
            ),
            (
                "the-delegate-drops-the-address",
                "        self.board.peek_word(addr)\n    }",
                "        self.board.peek_word(0)\n    }",
                "KILL",
            ),
            # CONTROL: the address rounded down to a word boundary on the way through.
            # Observably identical, and not obviously so -- which is what earns it a
            # place here. Every range in the map starts even and ends odd, and every
            # arm already discards bit 0 (`addr & !1` in the ROM arm, `>> 1` in the RAM,
            # gfx, and DIP index arithmetic), so clearing it can neither leave a range
            # nor select a different word.
            (
                "CONTROL-address-rounded-to-a-word",
                "        self.board.peek_word(addr)\n    }",
                "        self.board.peek_word(addr & !1)\n    }",
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
    "loop": (
        "crates/sfemu/src/loop_.rs",
        [
            ("step-runs-two-frames", "        let frames = if a.step {\n            1", "        let frames = if a.step {\n            2", "KILL"),
            (
                "paused-still-runs-frames",
                "        } else if paused {\n            0\n        } else {",
                "        } else if paused {\n            pacer.tick(elapsed)\n        } else {",
                "KILL",
            ),
            (
                "pause-only-ever-pauses",
                "            paused = !paused;",
                "            paused = true;",
                "KILL",
            ),
            # The reset genuinely removed. Anchored on the trailing comment line so
            # the pattern is unique -- `pacer.reset();` alone also matches the F3
            # branch, and a two-match pattern is a NO-OP rather than a result.
            (
                "no-pacer-reset-on-pause",
                "            // this line and it is not the real one.\n            pacer.reset();",
                "            // this line and it is not the real one.",
                "KILL",
            ),
            # The `render` *moved* inside the frame loop -- not duplicated. A copy
            # leaves the unconditional call in place and changes nothing, which is a
            # mutant that reports SURVIVE for being a no-op.
            (
                "render-only-when-frames-ran",
                "        m.render();\n        pens_to_argb(&m.video, &mut buf);",
                "        pens_to_argb(&m.video, &mut buf);",
                "KILL",
            ),
            (
                "inputs-never-reach-the-board",
                "        m.board.inputs = a.inputs;",
                "        let _ = a.inputs;",
                "KILL",
            ),
            (
                "a-failed-save-panics",
                'Err(e) => note(s, format!("cannot write `{}`: {e}", o.state_path.display())),',
                'Err(e) => panic!("cannot write `{}`: {e}", o.state_path.display()),',
                "KILL",
            ),
            # A notice per press rather than one per distinct problem.
            (
                "notices-are-not-deduplicated",
                "    if !s.notices.contains(&msg) {\n        s.notices.push(msg);\n    }",
                "    s.notices.push(msg);",
                "KILL",
            ),
            # A load applied despite a refusal: the machine ends up neither the saved
            # one nor the running one.
            (
                "a-refused-load-is-still-applied",
                "        Err(e) => note(s, format!(\"cannot load `{}`: {e}\", o.state_path.display())),",
                "        Err(_) => m.reset(),",
                "KILL",
            ),
            (
                "quit-is-ignored",
                "        if a.quit {\n            break;\n        }",
                "        if a.quit && false {\n            break;\n        }",
                "KILL",
            ),
            # The board tag written into the file. A whole-crate mutant rather than a
            # per-function one: `save` and `load` share the constant, so a round trip
            # agrees with itself whatever it says, and only a test that reads the
            # bytes off disk can tell. This one survived until
            # `a_saved_state_is_tagged_with_this_build_s_board` was written.
            (
                "states-tagged-for-another-board",
                "const BOARD: u32 = frontend::BOARD_SF2;",
                "const BOARD: u32 = 0x5346_3100;",
                "KILL",
            ),
            # The overlay drawn before the pen conversion rather than after. Written
            # out at length because the naive form of this mutant is not a mutant: the
            # honest version has to `resize` first, or `overlay::draw`'s "not a frame"
            # assert fires on the empty `Vec` and the run panics for a reason that has
            # nothing to do with the ordering. And the draw-after call has to *move*,
            # not be duplicated -- with both present the later one masks the earlier
            # and the mutant survives as a no-op.
            #
            # What it then does is worth naming: `pens_to_argb` clears the buffer and
            # refills it, so an overlay drawn first is not merely miscoloured, it is
            # gone. Killed by `the_overlay_reaches_the_presented_buffer`.
            (
                "overlay-drawn-before-the-pen-conversion",
                "        pens_to_argb(&m.video, &mut buf);\n        // After the conversion, never before: the overlay's pixels are already\n        // `0x00RRGGBB`, while `m.video`'s are CPS-1 pens. Drawn into the pen buffer\n        // they would be run through the palette and come out as whatever colours\n        // those indices happen to name.\n        dbg.draw(&mut buf, m);",
                "        buf.resize(machine::video::WIDTH * machine::video::HEIGHT, 0);\n        dbg.draw(&mut buf, m);\n        pens_to_argb(&m.video, &mut buf);",
                "KILL",
            ),
            # CONTROL: the title's exact wording. Nothing pins the phrasing, and
            # nothing should -- the tests look for "paused", "halted", and the drop
            # count, not for a sentence.
            (
                "CONTROL-title-wording",
                'let mut t = String::from("sfemu");',
                'let mut t = String::from("sfemu -- CPS-1");',
                "SURVIVE",
            ),
        ],
    ),
    # The wiring, not the logic. A separate set because it mutates a different file:
    # what a whole-crate view sees that a per-module one cannot is that the modules
    # can each be right and be plugged together wrong.
    "wiring": (
        "crates/sfemu/src/main.rs",
        [
            # Two same-typed fields swapped. Compiles; F5 then writes a save state
            # over your screenshot and F12 a screenshot over your save.
            (
                "state-and-shot-paths-swapped",
                "        state_path: args.state.clone(),\n        shot_path: default_shot_path(&args.path),",
                "        state_path: default_shot_path(&args.path),\n        shot_path: args.state.clone(),",
                "KILL",
            ),
            # CONTROL: the same two fields, written in the other order. A struct
            # initializer's field order is not its layout, so this is observably
            # identical -- and it is the right control for this set, because it is
            # the one edit in this neighbourhood that *should* pass while the swap
            # above must not.
            (
                "CONTROL-initializer-field-order",
                "        state_path: args.state.clone(),\n        shot_path: default_shot_path(&args.path),",
                "        shot_path: default_shot_path(&args.path),\n        state_path: args.state.clone(),",
                "SURVIVE",
            ),
        ],
    ),
}


def run_rows(name: str) -> list[tuple[str, str, str]]:
    """Applies every mutant of one set, returning (name, expectation, result)."""
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
            # A timeout, because a mutant can hang rather than fail. Measured, not
            # hypothetical: dropping the cycle charge in `Cps1::step_instruction`
            # leaves `run_scanline`'s `while self.line == line` spinning forever, so
            # the suite never returns a verdict at all. Without this the harness
            # itself hangs -- and a harness that hangs on a mutant it should kill
            # reads, from the outside, exactly like a slow build.
            #
            # A timeout counts as KILL. That is the right reading: the mutant made the
            # suite stop passing. It is recorded distinctly in the result string so a
            # *control* that times out cannot masquerade as a clean SURVIVE->KILL
            # discrepancy -- a control is meant to run to completion, and one that
            # hangs means the timeout is too short rather than the mutant fatal.
            try:
                r = subprocess.run(
                    ["cargo", "test", "-p", crate, "--quiet"],
                    capture_output=True,
                    text=True,
                    timeout=MUTANT_TIMEOUT_S,
                )
                got = "SURVIVE" if r.returncode == 0 else "KILL"
            except subprocess.TimeoutExpired:
                got = "KILL"
                mname = f"{mname} (hung)"
            rows.append((mname, expect, got))
    finally:
        shutil.copy(backup, src)
    return rows


def run(name: str) -> int:
    rows = run_rows(name)
    bad = 0
    for mname, expect, got in rows:
        ok = got == expect
        bad += not ok
        print(f"{'ok  ' if ok else 'BAD '} {mname:42} expect {expect:8} got {got}")
    print(f"\n{len(rows) - bad}/{len(rows)} as expected")
    return 1 if bad else 0


def run_all() -> int:
    """Every set, one after another, with a roll-up.

    The point of running them together rather than one at a time: a set that has
    started reporting NO-OP because the code it mutates was reworded is invisible
    when you only run the set you are working on.
    """
    total = killed = survived = noop = bad = 0
    for name in SETS:
        print(f"=== {name} ===")
        rows = run_rows(name)
        for mname, expect, got in rows:
            ok = got == expect
            bad += not ok
            total += 1
            if got.startswith("NO-OP"):
                noop += 1
            elif got == "KILL":
                killed += 1
            else:
                survived += 1
            print(f"{'ok  ' if ok else 'BAD '} {mname:42} expect {expect:8} got {got}")
        print()
    print(f"total {total}  killed {killed}  survived {survived}  no-op {noop}")
    print(f"{total - bad}/{total} as expected")
    return 1 if bad else 0


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "--all":
        sys.exit(run_all())
    if len(sys.argv) != 2 or sys.argv[1] not in SETS:
        print(f"usage: {sys.argv[0]} {{{','.join(SETS)},--all}}", file=sys.stderr)
        sys.exit(2)
    sys.exit(run(sys.argv[1]))
