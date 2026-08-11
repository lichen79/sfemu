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
- A test run that fails without naming a single failing test is **NO-BUILD**, not
  a kill. Same principle one step later: the mutation applied but the crate does
  not compile, so nothing was measured. Kept as a verdict distinct from NO-OP
  because the fault is in a different place -- NO-OP means the pattern no longer
  matches the source, NO-BUILD means it matched and the result is not valid Rust.
  This is not hypothetical: `overlay`'s `all-ones-is-shown-as-a-gap` was an E0716
  from the day it was written and every run scored it KILL until the verdict
  existed.
- A KILL records **which tests** failed. "The crate went red" is what the vector
  suites already say; a mutation pass is worth running for the name beside it,
  because a mutant killed only by a test with nothing to do with the mutated rule
  means the rule's own test asserts nothing.
- Every set includes at least one **control** mutant that must SURVIVE. A pass
  where everything dies is more likely a broken harness than a thorough suite.
- A set may span files: a mutant's optional fifth element names its own file.
- A kill signal restores the tree. SIGTERM and SIGHUP raise, so the `finally`
  clauses run; without that a run killed at a wall-clock cap leaves a live mutant
  in tracked source with nothing announcing it, which happened once.

Usage: `mutate.py <set>` for one set, `mutate.py --all` for every set. Prefer
`--all` before a commit: a set nobody is working on is a set whose mutants have
gone stale or stopped compiling, and only the roll-up looks at it.
"""

import re
import shutil
import signal
import subprocess
import sys

# How many failing test names to record beside a KILL.
#
# A bare KILL is not the number this pass is for. "Something in the crate went
# red" is what 1,604,000 vectors already tell you; what a mutation pass is worth
# is *which hand-written test* noticed, because a mutant killed only by a test
# whose name has nothing to do with the mutated rule means the rule's own test
# asserts nothing. Two names is enough to see that, and short enough that a row
# still fits a terminal line.
NAMED_KILLERS = 2

# Both of libtest's failure lines. `--quiet` prints `<name> --- FAILED` inline
# among the progress dots; the verbose form is `test <name> ... FAILED`. Matching
# only the verbose one made every real kill in the first `z80flags` run report
# NO-BUILD -- a harness bug that reads exactly like twelve mutants that do not
# compile, which is why NO-BUILD is a distinct verdict rather than a KILL.
_FAILED = re.compile(r"^(?:test )?(\S+) (?:\.\.\.|---) FAILED$", re.M)

# Which crate's tests each set is scored against. A set omitted here is scored
# against `frontend`, which is where this harness started. Naming the crate
# matters: scoring a mutation of `machine` against `frontend`'s tests would report
# SURVIVE for every mutant, since those tests never load the mutated code.
#
# A value may be a **list** of crates, all passed to one `cargo test` invocation.
# `ymsound` needs that and it is not a convenience: its subject spans two crates by
# construction. The 1,000-case YM2151 suite lives in `testrunner/tests/ymsuite.rs`
# while every hand-written chip test lives in `ym2151`, and the set's control mutant
# is precisely the claim that *one dies to the suite and the other to a unit test*.
# Scored against either crate alone, that control cannot be stated: against `ym2151`
# the suite never runs, and against `testrunner` the unit test never does.
CRATES: dict[str, str | list[str]] = {
    "snapshot": "machine",
    "peek": "machine",
    "peekcps1": "machine",
    "loop": "sfemu",
    "wiring": "sfemu",
    # `z80` and not the workspace, and that is the load-bearing choice for these
    # two sets. The 1,604,000 vector cases live in the `testrunner` crate, so
    # `cargo test -p z80` runs the hand-written tests and nothing else -- which is
    # how "which hand-written test kills this" gets asked at all.
    "z80flags": "z80",
    "z80int": "z80",
    # Both, for the reason above. The cost is real -- `testrunner` reads the
    # 1,000-case suite off disk for every mutant -- and it is what the set is for.
    "ymsound": ["ym2151", "testrunner"],
    "ymsched": "machine",
}

# How long one mutant's test run may take before it is declared killed. The whole
# workspace suite is under 3 s in this tree, so 120 is two orders of magnitude of
# headroom -- generous enough that a slow cold build is never mistaken for a hang.
MUTANT_TIMEOUT_S = 120

# name -> (default file, [(mutant-name, old, new, expectation[, file[, extra]]), ...])
#
# A mutant may carry a **fifth** element naming its own file, overriding the set's
# default. Added for the `z80flags` and `z80int` sets, whose subject is not a file
# but a *property*: the Z80's undocumented flag bits are computed in five files
# (`flags.rs`, `ops/alu.rs`, `ops/bits.rs`, `ops/load.rs`, `decode.rs`) and the
# interrupt sequence in three. Splitting either into one set per file would score
# each file's mutants against the same crate suite anyway while hiding the thing
# worth seeing -- that one hand-written test is the only killer for a bit that
# five files write. The field is trailing and optional, so every set above is
# unchanged.
#
# A mutant may carry a **sixth** element: a list of `(file, old, new)` triples
# applied *alongside* the main one, so one mutant can be several simultaneous edits.
# Added for `ymsound`'s control, which is only meaningful as two edits at once --
# forcing the prepare() gate eager while removing the suite's CSM cases. Applied
# separately the first dies to the suite and the second changes nothing; applied
# together they are the claim that the CSM cases are what make the suite able to see
# the gate. Every extra edit obeys the same exactly-once rule as the main one, and a
# NO-OP in any of them is a NO-OP for the whole mutant: a control that silently
# applied half of itself would report SURVIVE and mean nothing.
#
# A mutant may carry a **seventh** element: its own crate or crate list, overriding
# the set's. `ymsound` needs it to state its central claim as an actual
# SURVIVE-versus-KILL discrepancy rather than as an argument about which test names
# appear in two rows. The same eager-gate edit is scored twice -- against `testrunner`
# alone with the suite's CSM cases skipped, where it must SURVIVE, and against
# `ym2151` alone, where the unit test must KILL it. A pair of rows disagreeing is
# evidence; two KILLs with different killers named is a paragraph asking to be
# trusted.
SETS: dict[str, tuple[str, list[tuple[str, ...]]]] = {
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
            # ⚠️ This mutant did not compile until 2026-08-09 and was scored KILL by
            # every run before then, because the harness read any non-zero exit as a
            # kill. `push_str(if .. { ".." } else { &format!(..) })` is an E0716: the
            # `format!` temporary is dropped at the end of the `if` while the borrow is
            # still live. Found by the NO-BUILD verdict added with the z80 sets, which
            # is the finding that verdict exists to produce -- a mutant that does not
            # build measures nothing, and counting it as a kill inflates the score.
            # The `match` form below has the same effect and does compile.
            (
                "all-ones-is-shown-as-a-gap",
                "                Some(v) => line.push_str(&format!(\" {v:04X}\")),",
                "                Some(0xFFFF) => line.push_str(\"   --\"),\n                Some(v) => line.push_str(&format!(\" {v:04X}\")),",
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
            # ⚠️ It has moved twice, both times because a later task took the bit it
            # was parked on: E2 took 25 for `F7`, and E3 took 30 for `GfxView`. Each
            # time the control *died*, correctly -- it had quietly become a
            # two-keys-share-a-bit mutant, which the suite is supposed to kill. That is
            # the failure mode a control exists to expose in the harness itself, and it
            # is why `--all` is run rather than one set at a time.
            #
            # The denominator, written down this time: 34 keys hold bits 0-33, and
            # `KeySet` is a `u64`, so everything from 34 up is free. 62 leaves room
            # above and below. This control will die again if a key is ever given bit
            # 62, and that death is the signal it exists for, not a mutant to
            # re-expect.
            ("CONTROL-escape-moves-to-another-free-bit", "Key::Escape => 21,", "Key::Escape => 62,", "SURVIVE"),
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
    # E3 Task 6: the graphics viewer's state machine. Its characteristic bug is not a
    # wrong pixel, it is a key that acts on the wrong view -- five keys with four
    # meanings each is twenty behaviours, and nineteen of them right reads on screen as
    # a key that does nothing.
    "gfx": (
        "crates/frontend/src/gfx.rs",
        [
            # `Enter`'s dispatch pinned to one view. Anchored on the `fn act` line
            # because `match self.state.view {` opens `step` as well, and a two-match
            # pattern is a NO-OP rather than a result.
            (
                "enter-always-acts-on-the-tile-view",
                "    fn act(&mut self) {\n        match self.state.view {",
                "    fn act(&mut self) {\n        match View::Tiles {",
                "KILL",
            ),
            # The guard genuinely removed, not disabled: a hidden viewer that cycles
            # its view is the bug where `]` pages something you cannot see, and the
            # game's own bracket keys stop being free.
            (
                "a-hidden-viewer-still-takes-keys",
                "        if !self.on {\n            return false;\n        }\n",
                "",
                "KILL",
            ),
            # The mask reset by `F9`. The single most tempting wrong decision in this
            # module -- it reads as tidy -- and it makes "show me the game with scroll
            # 1 off" unreachable, which is the whole point of a layer mask.
            (
                "toggling-the-viewer-clears-the-mask",
                "        if a.gfx_toggled {\n            self.on = !self.on;\n        }",
                "        if a.gfx_toggled {\n            self.on = !self.on;\n            self.state.mask = LayerMask::all();\n        }",
                "KILL",
            ),
            (
                "the-cursor-keeps-following-after-a-move",
                "                self.state.map_at = Some((c, r));",
                "                self.state.map_at = None;",
                "KILL",
            ),
            # The reset dropped from the layer arm, anchored on its comment so the
            # pattern does not also match the assignment in `step`.
            (
                "a-new-layer-keeps-the-old-cursor",
                "                // old one means nothing. Back to following the beam.\n                self.state.map_at = None;",
                "                // old one means nothing. Back to following the beam.",
                "KILL",
            ),
            ("the-viewer-starts-shown", "            on: false,", "            on: true,", "KILL"),
            # Wrapping instead of saturating at the end of the ROM: reads as the view
            # resetting itself, and in release it lands on a real tile.
            (
                "paging-wraps-at-the-bottom",
                "        at.saturating_sub(page)",
                "        at.wrapping_sub(page)",
                "KILL",
            ),
            # A half-row palette step. This one is here for a specific reason: it
            # SURVIVES `the_palette_cursor_wraps_within_the_palette`, whose
            # `assert_eq!(pal_at, PAL_PAGE)` and `PENS - PAL_PAGE` hold for *any* value
            # of `PAL_PAGE` -- a claim that cannot fail. Only
            # `the_palette_cursor_moves_by_one_row_of_swatches`, which reads the step
            # off `pal_cell`, can tell.
            (
                "the-palette-steps-by-one-entry",
                "const PAL_PAGE: usize = 64;",
                "const PAL_PAGE: usize = 1;",
                "KILL",
            ),
            (
                "the-row-selection-sticks-at-the-top",
                "                    (self.state.row + ROWS - 1) % ROWS",
                "                    self.state.row.saturating_sub(1)",
                "KILL",
            ),
            # Two rows sharing a layer: the table that is right in three places and
            # wrong in the fourth, which subtracts a layer you can still see.
            (
                "row-one-toggles-the-sprites",
                "            1 => m.scroll1 = !m.scroll1,",
                "            1 => m.sprites = !m.sprites,",
                "KILL",
            ),
            # The layout SF2's scroll 1 actually uses, skipped. A browser that cannot
            # reach it shows scroll 1 at the wrong x bias and looks like a decoder bug.
            (
                "enter-skips-the-odd-tile-layout",
                "        TileKind::Tile8x8 => TileKind::Tile8x8Odd,",
                "        TileKind::Tile8x8 => TileKind::Tile16x16,",
                "KILL",
            ),
            # CONTROL: two fields of the initial `ViewState`, written in the other
            # order. A struct initializer's field order is not its layout, so this is
            # observably identical.
            #
            # Not the plan's proposed control -- `tile_at: 0` -> `0x40` -- which was
            # tried and dies honestly: `the_tile_view_pages_by_a_screenful_and_stops_at_zero`
            # reads `tile_at` after one page and expects exactly one page. Every other
            # field of `new()` is pinned by a test for the same reason, which is the
            # answer to "why is this set's control so unambitious": in a constructor
            # whose every value is asserted, the only safe edit is one that changes no
            # value at all.
            (
                "CONTROL-initial-state-field-order",
                "                map_at: None,\n                row: 0,",
                "                row: 0,\n                map_at: None,",
                "SURVIVE",
            ),
        ],
    ),
    # E3 Task 6: the four views. What this set is really testing is the module's one
    # rule -- that nothing here re-derives what the renderer knows. Three of the
    # mutants below are re-derivations that look right and are wrong in exactly the
    # case the renderer already handles.
    "gfxpanels": (
        "crates/frontend/src/gfxpanels.rs",
        [
            # `View::cycled` lives here, not in `gfx.rs` -- the plan tabled this mutant
            # under the `gfx` set, which is a plan/implementation mismatch, not a
            # second copy of the function.
            (
                "the-view-does-not-cycle-back",
                "            Self::Layers => Self::Tiles,",
                "            Self::Layers => Self::Layers,",
                "KILL",
            ),
            (
                "the-tile-pens-are-not-the-roms",
                "                    let pen = tile_pen(rom, s.kind, code, x, y);",
                "                    let pen = 0;",
                "KILL",
            ),
            (
                "the-greys-are-inverted",
                "    GREYS[(pen & 0x0F) as usize]",
                "    GREYS[15 - (pen & 0x0F) as usize]",
                "KILL",
            ),
            # The mapper's `None` printed as a tile number. The one failure the picture
            # cannot show, turned into a diagnostic that sends you to tile 0.
            (
                "an-unmapped-code-shows-as-tile-zero",
                '        None => text(buf, tx, ty + 2 * LINE, "ROM ----", OFF),',
                '        None => text(buf, tx, ty + 2 * LINE, "ROM 0000", OFF),',
                "KILL",
            ),
            # Three re-derivations of `map_origin`'s one line, which is four decisions
            # long. Each is a viewer that names a tile the renderer never fetched.
            (
                "the-cursor-ignores-the-scroll",
                "    let x = VISIBLE_X + i32::from(m.board.cps_a[sx] as i16);",
                "    let x = VISIBLE_X;",
                "KILL",
            ),
            # Expected KILL when it was written, and it survived. Diagnosed rather
            # than re-expected: the two readings differ by exactly 65536, and 64*8,
            # 64*16 and 64*32 all divide 65536, so `map_axis`'s Euclidean wrap gives
            # the same tile and the same offset for either -- no register value on any
            # layer can separate them. Proven for all 65536 values of all three
            # layers, and the precondition is now pinned by
            # `a_map_span_divides_the_register_range`. The line keeps `as i16`
            # because the intermediate value is a scroll and -64 is not 65472; the
            # mutant is an equivalent, not a gap.
            (
                "EQUIVALENT-the-cursor-reads-the-scroll-unsigned",
                "    let x = VISIBLE_X + i32::from(m.board.cps_a[sx] as i16);",
                "    let x = VISIBLE_X + i32::from(m.board.cps_a[sx] as i32);",
                "SURVIVE",
            ),
            (
                "the-cursor-forgets-the-visible-origin",
                "    let x = VISIBLE_X + i32::from(m.board.cps_a[sx] as i16);",
                "    let x = i32::from(m.board.cps_a[sx] as i16);",
                "KILL",
            ),
            # The layers view answering "is this layer enabled" itself. The re-derived
            # form is not sloppy -- it is the layer-control bit, correctly -- and it is
            # wrong only for scrolls 2 and 3, which have a second gate in videocontrol.
            # That is precisely the case `the_layers_view_agrees_with_the_renderer`'s
            # third stanza was added for; the first two stanzas move the one bit both
            # versions agree about, so this mutant survives them.
            (
                "the-layers-view-derives-its-own-enable",
                "                    Some(layer_enabled(cfg, layer, layercontrol, videocontrol)),",
                "                    Some(layercontrol & cfg.layer_enable_mask[n - 1] != 0),",
                "KILL",
            ),
            # The box two pixels wider than its border. `WIDTH - 2` and not `WIDTH`: at
            # `WIDTH` the `put` guard's range runs past the row and the run panics on an
            # index, which is a kill for the wrong reason. This one draws, and draws
            # into the border -- killed only by the literal-`2` half of
            # `every_view_stays_inside_its_box`, since the derived half widens with it.
            (
                "a-view-runs-into-its-border",
                "const VW: usize = WIDTH - 4;",
                "const VW: usize = WIDTH - 2;",
                "KILL",
            ),
            (
                "a-swatch-is-not-the-windows-own-colour",
                "        let fill = crate::pixels::argb(entry);",
                "        let fill = EDGE;",
                "KILL",
            ),
            # "Past the end of the ROM" drawn as a tile as well as a dot: `tile_pen`
            # returns the transparent pen out there, which is a solid white square.
            (
                "an-off-rom-tile-is-drawn-anyway",
                "                put(buf, cx, cy, OFF);\n                continue;",
                "                put(buf, cx, cy, OFF);",
                "KILL",
            ),
            (
                "the-sprites-are-given-a-hardware-enable",
                '            0 => ("OB", 0u8, None, s.mask.permits(None)),',
                '            0 => ("OB", 0u8, Some(true), s.mask.permits(None)),',
                "KILL",
            ),
            # The two columns collapsed into one: the mask column showing the
            # hardware's answer means the view can no longer tell you what *you* did.
            (
                "the-mask-column-is-the-hardwares",
                "                    s.mask.permits(Some(layer)),",
                "                    layer_enabled(cfg, layer, layercontrol, videocontrol),",
                "KILL",
            ),
            # CONTROL: a swatch's border shade. Nothing pins it and nothing should --
            # the tests read a swatch's *fill*, one pixel in, and the border exists to
            # separate neighbouring swatches rather than to be a particular grey.
            (
                "CONTROL-the-swatch-border-shade",
                "const EDGE: u32 = 0x0080_8080;",
                "const EDGE: u32 = 0x0070_7070;",
                "SURVIVE",
            ),
        ],
    ),
    # D1 Task 16: the flag rules. This set exists for a reason the others do not
    # share. Every mutant below is also caught by the vector suite -- 1,604,000 cases
    # compare `f` on every one -- so a pass that ran the suite would report 12/12 and
    # measure nothing. It is scored against `cargo test -p z80`, which holds **only**
    # the hand-written tests: the vectors live in the `testrunner` crate and never
    # load this one's `#[cfg(test)]` modules.
    #
    # So a SURVIVE here does not mean the core is wrong. It means the crate has a
    # flag test that asserts nothing, and nobody would find out until the day the
    # suite goes red for an unrelated reason and there is no smaller signal to work
    # with. That is the whole question this set answers, and it is why each row
    # names its killer in the comment and the runner prints the killer it got.
    "z80flags": (
        "crates/z80/src/flags.rs",
        [
            # Parity inverted. Reaches every logical operation, every CB rotate and
            # both block-compare forms -- deliberately the broadest mutant in the set,
            # because if the *narrowest* rule in the file can be inverted with only
            # the suite noticing, nothing else here is worth measuring.
            (
                "parity-inverted",
                "    v.count_ones().is_multiple_of(2)",
                "    !v.count_ones().is_multiple_of(2)",
                "KILL",
            ),
            # S dropped from `sz53`'s mask. The two undocumented bits stay, so this is
            # not "the helper is broken" -- it is the one documented bit of the three
            # going missing, which is what a mask typed as a literal gets wrong.
            (
                "sz53-drops-the-sign-bit",
                "    (v & (S | F5 | F3)) | if v == 0 { Z } else { 0 }",
                "    (v & (F5 | F3)) | if v == 0 { Z } else { 0 }",
                "KILL",
            ),
            # `CP`'s deviation removed: F3/F5 left where `sub` put them, which is the
            # *result*. The single most plausible wrong implementation of `CP` on the
            # chip, because it is what every other arithmetic operation does.
            (
                "cp-takes-f3-f5-from-the-result",
                "    cpu.f = (cpu.f & !(F5 | F3)) | (v & (F5 | F3));",
                "    cpu.f = (cpu.f & !(F5 | F3)) | (cpu.f & (F5 | F3));",
                "KILL",
                "crates/z80/src/ops/alu.rs",
            ),
            # `INC` clearing carry. `INC`/`DEC` preserving C is what makes a 16-bit
            # increment loop work at all, and it is invisible in any test that does
            # not set C before the increment -- which is most of them, since
            # `Z80::new()` leaves every flag set and a test that never touches `f`
            # sees C set before and after either way.
            (
                "inc-clears-the-carry",
                "    let r = v.wrapping_add(1);\n    cpu.f = (cpu.f & C)",
                "    let r = v.wrapping_add(1);\n    cpu.f = (cpu.f & 0)",
                "KILL",
                "crates/z80/src/ops/alu.rs",
            ),
            # `ADD HL,rr`'s half-carry from bit 12 rather than bit 11. Wrong on
            # exactly the operands where the low three nibbles carry and the fourth
            # does not -- so a test with tidy round numbers cannot see it.
            (
                "add16-half-carry-from-bit-twelve",
                "    let h = (a & 0x0FFF) + (b & 0x0FFF) > 0x0FFF;",
                "    let h = (a & 0x1FFF) + (b & 0x1FFF) > 0x1FFF;",
                "KILL",
                "crates/z80/src/ops/alu.rs",
            ),
            # The preservation defeated: S and Z written from the 16-bit result. Reads
            # as an improvement -- the manual's table for `ADD HL,rr` looks like an
            # omission until you check it against hardware.
            (
                "add16-writes-sign-and-zero",
                "    cpu.f = (cpu.f & (S | Z | PV))\n        | (((r >> 8) as u8) & (F5 | F3))",
                "    cpu.f = (cpu.f & PV)\n        | flags::sz53(((r >> 8) as u8))",
                "KILL",
                "crates/z80/src/ops/alu.rs",
            ),
            # EQUIVALENT, and proven rather than asserted. Expected KILL when it was
            # written, and it survived; the diagnosis is that `parity(masked)` and
            # `masked == 0` are the same function on every input `bit_test` can
            # receive. `masked` is `v & (1 << b)` with `b` from bits 3-5 of a CB
            # opcode, so `b <= 7` and `masked` holds **at most one bit**: zero bits is
            # both even parity and zero, one bit is both odd parity and non-zero.
            # Checked exhaustively over all 256 x 8 = 2,048 reachable `(v, b)` pairs:
            # 0 disagree. No test can separate them because no *input* can, so this is
            # not a gap in `bit_tests_a_bit_and_copies_z_into_parity`.
            #
            # Kept in the set rather than deleted: the day `bit_test` is given a
            # multi-bit mask -- a `BIT` over a nibble, say -- the two stop agreeing and
            # this row stops surviving and says so. The mutant the *plan* meant is the
            # row below, which is a real one.
            (
                "EQUIVALENT-bit-parity-of-a-single-bit-mask",
                "        (cpu.f & C) | H | (f35 & (F5 | F3)) | if masked == 0 { Z | PV } else { 0 } | (masked & S);",
                "        (cpu.f & C) | H | (f35 & (F5 | F3)) | if masked == 0 { Z } else { 0 } | (masked & S)\n            | if crate::flags::parity(masked) { PV } else { 0 };",
                "SURVIVE",
                "crates/z80/src/ops/bits.rs",
            ),
            # `BIT` taking P/V from the parity of the **operand** rather than copying Z.
            # This is the honest form of the mutant above, and the plausible wrong
            # implementation: it is what every logical operation on the chip does, so
            # writing `sz53p`-style parity here reads as consistency. It disagrees with
            # the copy whenever the operand's parity differs from "the tested bit is
            # clear", which is most of the time.
            (
                "bit-takes-parity-from-the-operand",
                "        (cpu.f & C) | H | (f35 & (F5 | F3)) | if masked == 0 { Z | PV } else { 0 } | (masked & S);",
                "        (cpu.f & C) | H | (f35 & (F5 | F3)) | if masked == 0 { Z } else { 0 } | (masked & S)\n            | if crate::flags::parity(v) { PV } else { 0 };",
                "KILL",
                "crates/z80/src/ops/bits.rs",
            ),
            # `BIT` setting S for every bit rather than only bit 7. `masked & S` is
            # already the whole rule, so the mutant is the *obvious* reading -- "the
            # result is non-zero, so it is negative" -- and it is wrong for bits 0-6.
            (
                "bit-sets-sign-for-every-bit",
                "| (masked & S);",
                "| if masked != 0 { S } else { 0 };",
                "KILL",
                "crates/z80/src/ops/bits.rs",
            ),
            # `RLCA` writing the full flag set instead of preserving S/Z/PV. The four
            # accumulator rotates are the only rotates that preserve them; `CB 07`
            # (`RLC A`) does not, and the two are one keystroke apart in a decode
            # table.
            (
                "rlca-writes-the-whole-flag-set",
                "    cpu.f = (cpu.f & (S | Z | PV)) | (result & (F5 | F3)) | u8::from(carry);",
                "    cpu.f = crate::flags::sz53p(result) | u8::from(carry);",
                "KILL",
                "crates/z80/src/ops/bits.rs",
            ),
            # `SCF`/`CCF` taking F3/F5 from `A` alone, ignoring `Q`. Measured wrong on
            # 229 and 219 of 1,000 cases respectively -- so it is right 77% of the
            # time, which is exactly the shape a hand-written test misses if its
            # fixture happens to be one of the agreeing cases. `Q` is what makes this
            # the hardest rule in the file, and this mutant is the reason
            # `q` is not a boolean.
            (
                "scf-ignores-q",
                "    let f35 = (cpu.a | (cpu.f & !cpu.q)) & (F5 | F3);",
                "    let f35 = cpu.a & (F5 | F3);",
                "KILL",
                "crates/z80/src/decode.rs",
            ),
            # `DAA` ignoring N: the correction always added. N selects the direction
            # and that is the only thing N is for, so this is a `DAA` that is correct
            # after every `ADD` and wrong after every `SUB` -- and a BCD test that only
            # ever adds cannot tell.
            (
                "daa-ignores-n",
                "    let result = if cpu.f & N != 0 {\n        a.wrapping_sub(adjust)\n    } else {\n        a.wrapping_add(adjust)\n    };",
                "    let result = a.wrapping_add(adjust);",
                "KILL",
                "crates/z80/src/decode.rs",
            ),
            # `LDI`/`LDD`'s P/V as real parity rather than `BC != 0`. The two agree
            # whenever the moved byte's parity happens to match "more to go", and the
            # bit is what `LDIR`'s loop termination is read from -- so a wrong P/V here
            # is a block move that stops one byte early on some byte values and not
            # others. Anchored with the F5 line above it: `if cpu.bc() != 0 { PV }`
            # alone also matches `cpi_cpd`, and a two-match pattern is a NO-OP.
            (
                "ldi-parity-instead-of-the-count",
                "        | if n & 0x02 != 0 { F5 } else { 0 }\n        | if cpu.bc() != 0 { PV } else { 0 };\n    cpu.q = cpu.f;\n}\n\n/// `CPI`",
                "        | if n & 0x02 != 0 { F5 } else { 0 }\n        | if flags::parity(v) { PV } else { 0 };\n    cpu.q = cpu.f;\n}\n\n/// `CPI`",
                "KILL",
                "crates/z80/src/ops/load.rs",
            ),
            # CONTROL: a word added to the module's own doc comment. Nothing compiles
            # differently and nothing can observe it, so a death here means the
            # harness is comparing something other than test outcomes -- a stale
            # backup, a concurrent `cargo`, a mutant left applied from the row before.
            # Deliberately the emptiest possible edit: a control that changes a
            # *value* is a control one bit away from being a real mutant, which is how
            # `keys`'s escape-bit control died twice.
            (
                "CONTROL-a-word-in-the-module-doc",
                "//! The flag register's bits and the rules that fill them.",
                "//! The flag register's eight bits and the rules that fill them.",
                "SURVIVE",
            ),
        ],
    ),
    # D1 Task 16: the interrupt sequence. The opposite problem to `z80flags`, and the
    # reason these are two sets rather than one: **no vector reaches `interrupt.rs`.**
    # SingleStepTests drives instructions, and accepting an interrupt is not an
    # instruction. So a survivor here is not a test that asserts nothing -- it is
    # behaviour with no verification at all, in the module a sound CPU spends its life
    # in. Every mutant is a machine that still runs and still services interrupts, and
    # gets one thing wrong in a way that shows up as the *game* hanging.
    "z80int": (
        "crates/z80/src/interrupt.rs",
        [
            # The one-instruction arming delay gone. `EI; RETI` re-enters its own ISR,
            # the stack grows until it eats the program, and the crash is nowhere near
            # the cause.
            (
                "ei-delay-ignored",
                "        if !self.irq || !self.iff1 || self.ei != 0 {",
                "        if !self.irq || !self.iff1 {",
                "KILL",
            ),
            # `iff2` not saved across an NMI. `RETN` then restores whatever `iff2`
            # held from before, so a maskable interrupt taken during an NMI handler
            # comes back with the wrong enable -- and it is `RETN`, two files away,
            # that shows the damage.
            (
                "nmi-does-not-save-iff1",
                "        self.iff2 = self.iff1;\n        self.iff1 = false;",
                "        self.iff1 = false;",
                "KILL",
            ),
            # `irq` cleared before the mask test rather than after. The line is
            # level-sensitive: the device is still holding it, so a request refused
            # while `iff1` is clear must stay pending. Clearing it early drops the
            # interrupt entirely, and the sound stops one note in.
            (
                "a-refused-request-is-dropped",
                "        if !self.irq || !self.iff1 || self.ei != 0 {\n            return None;\n        }\n        self.irq = false;",
                "        self.irq = false;\n        if !self.iff1 || self.ei != 0 {\n            return None;\n        }",
                "KILL",
            ),
            # Mode 2 not masking the vector's low bit: an odd bus value reads the high
            # half of one vector and the low half of the next, giving a plausible
            # address in the middle of nothing.
            (
                "mode-two-does-not-mask-the-vector",
                "                let addr = u16::from(self.i) << 8 | u16::from(vector & 0xFE);",
                "                let addr = u16::from(self.i) << 8 | u16::from(vector);",
                "KILL",
            ),
            # `leave_halt` rewinding PC. The tempting wrong fix, and the module's doc
            # comment says why: `HALT` does not advance PC in the first place, so
            # adjusting sends the `RETI` back into the `HALT` and the machine freezes
            # the first time an interrupt arrives in an idle loop -- which is where a
            # sound CPU spends most of its time.
            (
                "leaving-halt-rewinds-the-pc",
                "    fn leave_halt(&mut self) {\n        self.halted = false;\n    }",
                "    fn leave_halt(&mut self) {\n        self.halted = false;\n        self.pc = self.pc.wrapping_sub(1);\n    }",
                "KILL",
            ),
            # The priority reversed. Both are serviced either way, so this is not a
            # lost interrupt -- it is the *order*, which matters because the maskable
            # one is level-sensitive and stays pending while the NMI is edge-triggered
            # and does not.
            (
                "the-maskable-interrupt-wins-against-an-nmi",
                "        if let Some(t) = self.ack_nmi(bus) {\n            return t;\n        }\n        self.ack_irq(bus).unwrap_or(0)",
                "        if let Some(t) = self.ack_irq(bus) {\n            return t;\n        }\n        self.ack_nmi(bus).unwrap_or(0)",
                "KILL",
            ),
            # `step` clearing `ei` *after* dispatch rather than before. One line moved,
            # and the arming delay is off by one instruction in the other direction:
            # `EI` itself would clear the mark it just set, so the delay never applies
            # at all. Measured in `cpu.rs`'s own comment: treating a set `ei` as "now
            # enable" disagrees with hardware on 569,245 of 759,299 cases.
            (
                "ei-cleared-after-dispatch",
                "        self.ei = 0;\n        self.p = 0;\n        let op = self.fetch(bus);\n        crate::decode::execute(self, bus, op)",
                "        self.p = 0;\n        let op = self.fetch(bus);\n        let t = crate::decode::execute(self, bus, op);\n        self.ei = 0;\n        t",
                "KILL",
                "crates/z80/src/cpu.rs",
            ),
            # `RETN` not restoring `iff1`. Measured: of `ed_45`'s 1,000 cases, 498
            # have the two flip-flops disagreeing on entry. Returning from an NMI with
            # interrupts still disabled means every maskable interrupt after the first
            # NMI is lost -- silence, with nothing to point at.
            (
                "retn-does-not-restore-iff1",
                "            cpu.iff1 = cpu.iff2;\n            flow::ret(cpu, bus);",
                "            flow::ret(cpu, bus);",
                "KILL",
                "crates/z80/src/decode.rs",
            ),
            # CONTROL: a discarding borrow of `bus` in `service`. Behaviourally empty
            # -- `&bus` is taken and dropped -- and it compiles, which is the part
            # worth checking: `service` takes `bus: &mut B`, so a control that
            # borrowed it *mutably* alongside the calls below would not build, and a
            # NO-BUILD is not a SURVIVE.
            (
                "CONTROL-a-discarded-borrow-in-service",
                "    pub fn service<B: Bus>(&mut self, bus: &mut B) -> u32 {",
                "    pub fn service<B: Bus>(&mut self, bus: &mut B) -> u32 {\n        let _ = &bus;",
                "SURVIVE",
            ),
        ],
    ),
    # D2 Task 13, first of two. The chip itself, across five files -- `tables.rs`,
    # `operator.rs`, `channel.rs`, `noise.rs`, `timer.rs`, `chip.rs` -- for the reason
    # `z80flags` spans five: the subject is the *chip's arithmetic*, and one set per
    # file would score each against the same suite anyway while hiding what is worth
    # seeing, which is that a table's own closed-form test is the only killer for a
    # number 1,000 audio cases cannot see.
    #
    # Scored against `ym2151` **and** `testrunner` (see `CRATES`): several mutants
    # here are audible only in the 1,000-case suite, and the set's last entry is the
    # claim that one specific mutant is audible *only* there and one *only* in a unit
    # test. Neither crate alone can state that.
    "ymsound": (
        "crates/ym2151/src/operator.rs",
        [
            # The two-bit slot swap. Every algorithm's carrier positions depend on it,
            # so the identity makes algorithm 4's carriers the wrong operators.
            (
                "slot-of-is-the-identity",
                "    ((op_index & 1) << 1) | ((op_index >> 1) & 1)",
                "    op_index & 3",
                "KILL",
                "crates/ym2151/src/regs.rs",
            ),
            # The POW closed form. `- 1024` is what makes the table hold raw mantissas
            # rather than pre-scaled ones; without it every entry is 1,024 too high and
            # `attenuation_to_volume` returns silence-adjacent nonsense.
            (
                "pow-closed-form-loses-its-offset",
                "            let want = (2f64.powf(-(f64::from(i) + 1.0) / 256.0) * 2048.0).round() - 1024.0;",
                "            let want = (2f64.powf(-(f64::from(i) + 1.0) / 256.0) * 2048.0).round();",
                "KILL",
                "crates/ym2151/src/tables.rs",
            ),
            # One entry of the 768-value phase table, off by one. This is the mutant the
            # checksum exists for: the audio difference is a fraction of a cent on one
            # note, which no listener and no coverage premise would notice.
            (
                "phase-step-zero-off-by-one",
                "pub static PHASE_STEP: [u32; 768] = [\n    41568,",
                "pub static PHASE_STEP: [u32; 768] = [\n    41569,",
                "KILL",
                "crates/ym2151/src/tables.rs",
            ),
            # The attack-to-decay transition. `<= 0` on a `u16` is `== 0` in value but
            # not in *reachability*: it is `attenuation == 0` written so that a signed
            # port would transition early. Kept as the plan wrote it because the point
            # is the boundary, and clippy's `-D warnings` would reject `<= 0` on an
            # unsigned -- so the comparison is made against a widened i32 instead.
            (
                "attack-transition-on-le-zero",
                "        if self.env_state == EnvState::Attack && self.env_attenuation == 0 {",
                "        if self.env_state == EnvState::Attack && i32::from(self.env_attenuation) <= 1 {",
                "KILL",
            ),
            # The documented rate-62/63 glitch, off by one: rate 62 would increment.
            (
                "attack-glitch-threshold-is-sixty-three",
                "            if rate < 62 {",
                "            if rate < 63 {",
                "KILL",
            ),
            # Release wraps instead of clamping, so a fully decayed note comes back at
            # full volume -- audible, and only if something drives release to the top.
            (
                "release-wraps-instead-of-clamping",
                "            self.env_attenuation += increment as u16;\n            if self.env_attenuation >= 0x400 {\n                self.env_attenuation = 0x3FF;\n            }",
                "            self.env_attenuation = (self.env_attenuation + increment as u16) & 0x3FF;",
                "KILL",
            ),
            # MUL = 0 means a half, not zero. Read literally it silences every operator
            # with the most common multiple setting there is.
            (
                "multiple-zero-is-zero",
                "        let multiple = match regs.op_multiple(op) {\n            0 => 1,\n            mul => mul * 2,\n        };",
                "        let multiple = regs.op_multiple(op) * 2;",
                "KILL",
            ),
            # DT1's sign bit folded into the magnitude index, so setting 4 -- documented
            # as a second no-op -- becomes a real detune and 6 detunes the wrong way.
            (
                "dt1-sign-bit-folded-into-the-magnitude",
                "    let magnitude = i32::from(DETUNE[(key_code & 0x1F) as usize][(detune & 3) as usize]);\n    if detune & 4 != 0 {\n        -magnitude\n    } else {\n        magnitude\n    }",
                "    i32::from(DETUNE[(key_code & 0x1F) as usize][(detune & 3) as usize])",
                "KILL",
                "crates/ym2151/src/tables.rs",
            ),
            # The envelope clock divider. 2 instead of 3 makes every envelope run at a
            # different rate: no unit test names the divider, so this is the suite's.
            (
                "envelope-divider-is-two",
                "pub const EG_CLOCK_DIVIDER: u32 = 3;",
                "pub const EG_CLOCK_DIVIDER: u32 = 2;",
                "KILL",
                "crates/ym2151/src/chip.rs",
            ),
            # The YM3012 roundtrip removed. Below 513 it is an identity, so this is
            # inaudible on a quiet case and wrong on every loud one -- the suite's.
            (
                "dac-roundtrip-removed",
                "        (roundtrip_fp(left), roundtrip_fp(right))",
                "        (left.clamp(-32768, 32767) as i16, right.clamp(-32768, 32767) as i16)",
                "KILL",
                "crates/ym2151/src/chip.rs",
            ),
            # The pan bits swapped: a channel panned hard left comes out hard right.
            (
                "pan-bits-swapped",
                "        (\n            if left { result } else { 0 },\n            if right { result } else { 0 },\n        )",
                "        (\n            if right { result } else { 0 },\n            if left { result } else { 0 },\n        )",
                "KILL",
                "crates/ym2151/src/channel.rs",
            ),
            # The divider applied to the phase instead of the envelope: every operator's
            # pitch is then wrong by a third, and no unit test clocks a whole chip.
            (
                "divider-applied-to-the-phase",
                "            self.channels[ch as usize].clock(&self.regs, ch, self.env_counter, lfo_pm);",
                "            self.channels[ch as usize].clock(&self.regs, ch, self.env_counter, lfo_pm / 3);",
                "KILL",
                "crates/ym2151/src/chip.rs",
            ),
            # The LFSR's tap moved from 14 to 13. The period test is what pins it: a
            # moved tap still makes noise, and noise that sounds like noise is the
            # hardest wrong thing in this chip to hear.
            (
                "noise-lfsr-tap-moved",
                "            self.lfsr |= ((self.lfsr >> 17) & 1) ^ ((self.lfsr >> 14) & 1) ^ 1;",
                "            self.lfsr |= ((self.lfsr >> 17) & 1) ^ ((self.lfsr >> 13) & 1) ^ 1;",
                "KILL",
                "crates/ym2151/src/noise.rs",
            ),
            # Timer A's period off by one, which is the difference between `1024 - v`
            # and `1023 - v` and shows up as a status bit one sample early.
            (
                "timer-a-period-off-by-one",
                "            1024 - regs.timer_a_value()",
                "            1023 - regs.timer_a_value()",
                "KILL",
                "crates/ym2151/src/timer.rs",
            ),
            # The IRQ line drops when *either* status bit clears rather than both, so a
            # driver clearing timer A loses an interrupt timer B is still asserting.
            (
                "irq-drops-on-either-status-bit",
                "        self.irq = self.status & IRQ_MASK != 0;",
                "        self.irq = self.status & IRQ_MASK == IRQ_MASK;",
                "KILL",
                "crates/ym2151/src/timer.rs",
            ),
            # The prepare() gate forced eager, scored against the **suite alone**. This
            # is Definition of Done item 5 stated as a measurement: with the CSM cases
            # present, the 1,000-case suite by itself sees the gate.
            (
                "prepare-gate-forced-eager-vs-the-suite",
                "        let eager = self.force_eager_prepare();",
                "        let eager = true;",
                "KILL",
                "crates/ym2151/src/chip.rs",
                [],
                "testrunner",
            ),
            # The same edit against the **unit tests alone**, which must also see it.
            # Two rows rather than one because a single run against both crates cannot
            # distinguish "the suite caught it" from "a unit test did".
            (
                "prepare-gate-forced-eager-vs-the-unit-tests",
                "        let eager = self.force_eager_prepare();",
                "        let eager = true;",
                "KILL",
                "crates/ym2151/src/chip.rs",
                [],
                "ym2151",
            ),
            # CONTROL, and the load-bearing one for Definition of Done item 5: the same
            # eager gate, with the suite's CSM cases skipped, scored against the suite
            # alone. It must **SURVIVE** -- which is the whole claim, stated as a
            # discrepancy against the row two above rather than as an argument about
            # which killer names appear where. A suite with no CSM case cannot see the
            # gate at all, so an eager port would pass at 1,000/1,000 while being wrong,
            # and `with_csm_on_eager_and_lazy_diverge` is the only thing standing there.
            #
            # Expressed as two simultaneous edits because either alone says nothing: the
            # gate edit alone dies to the suite (the row above), and the CSM skip alone
            # changes no result.
            (
                "CONTROL-eager-gate-is-invisible-to-a-suite-without-csm",
                "        let eager = self.force_eager_prepare();",
                "        let eager = true;",
                "SURVIVE",
                "crates/ym2151/src/chip.rs",
                [
                    # In `the_suite_passes`, not in `run_case`: skipping the cases
                    # inside the runner would also change what
                    # `the_runner_reports_a_deliberately_corrupted_sample` measures, and
                    # a control killed by the runner's own test would be a control
                    # killed for a reason that has nothing to do with CSM coverage.
                    (
                        "crates/testrunner/tests/ymsuite.rs",
                        "    for (i, case) in v.cases.iter().enumerate() {\n        let r = ymrunner::run_case(case);",
                        "    for (i, case) in v.cases.iter().enumerate() {\n        if case.writes.iter().any(|w| w.reg == 0x14 && w.val & 0x80 != 0) {\n            continue;\n        }\n        let r = ymrunner::run_case(case);",
                    )
                ],
                "testrunner",
            ),
        ],
    ),
    # D2 Task 13, second of two: the machine's side -- the Z80's rational clock, the
    # sample accumulator, the save state's sound fields, and the sound board's map.
    "ymsched": (
        "crates/machine/src/cps1.rs",
        [
            # The rational accumulator replaced by the truncated integer it exists to
            # avoid. 229 per line is 284 T short over a 3,125-line period.
            (
                "rational-clock-truncated-to-229",
                "pub const Z80_T_NUM: u32 = 715_909;",
                "pub const Z80_T_NUM: u32 = 715_625;",
                "KILL",
                "crates/machine/src/timing.rs",
            ),
            # The remainder discarded each line, which is the same truncation reached a
            # different way: every line grants 229 and the fraction never accumulates.
            (
                "accumulator-remainder-reset-each-line",
                "        let total = self.num + self.rem;\n        self.rem = total % self.den;\n        total / self.den",
                "        let total = self.num + self.rem;\n        self.rem = 0;\n        total / self.den",
                "KILL",
                "crates/machine/src/timing.rs",
            ),
            # The remainder dropped from the save state. Invisible for exactly one line
            # after a load, then permanent divergence.
            (
                "remainder-dropped-from-the-save-state",
                "        self.z80_carry = s.z80_carry;",
                "",
                "KILL",
            ),
            # The interleave order swapped: the Z80's line runs before the 68000's.
            (
                "z80-stepped-before-the-68000",
                "            self.z80_debt += i64::from(self.z80_carry.advance());\n            while self.z80_debt > 0 {\n                self.step_sound();\n            }\n            // One sample per scanline",
                "            // One sample per scanline",
                "KILL",
            ),
            # The sample accumulator driven by lines rather than T-states, so the sample
            # rate is locked to the beam and drifts against the chip.
            (
                "samples-accrued-per-line-not-per-t-state",
                "        self.sample_acc += t;",
                "        self.sample_acc += 1;",
                "KILL",
            ),
            # The banked window widened to six banks. SF2's `audiocpu` is 0x18000, so
            # banks 2-5 read past the region and answer 0xFF -- `RST 38h` again.
            (
                "banked-window-is-six-banks",
                "const BANKS: u8 = 2;",
                "const BANKS: u8 = 6;",
                "KILL",
                "crates/machine/src/sound.rs",
            ),
            # The bank register taking the whole byte rather than bit 0.
            (
                "bank-register-uses-the-whole-byte",
                "            0xF004 => self.bank = val & (BANKS - 1),",
                "            0xF004 => self.bank = val,",
                "KILL",
                "crates/machine/src/sound.rs",
            ),
            # The two command latches aliased, so every command byte is also a fade
            # value and the driver's timer byte is whatever the last command was.
            (
                "the-two-latches-aliased",
                "                if addr == 0xF008 {\n                    self.latches[0]\n                } else {\n                    self.latches[1]\n                }",
                "                self.latches[0]",
                "KILL",
                "crates/machine/src/sound.rs",
            ),
            # The chip dropped from the save state: a load restores the driver's idea
            # of the chip and not the chip, so every envelope resumes mid-air.
            (
                "ym2151-dropped-from-the-save-state",
                "        self.sound\n            .restore(&s.sound_ram, s.sound_bank, s.oki_pin7, &s.ym, s.ym_addr);",
                "        self.sound.restore(\n            &s.sound_ram,\n            s.sound_bank,\n            s.oki_pin7,\n            &ym2151::Ym2151::new(),\n            s.ym_addr,\n        );",
                "KILL",
            ),
            # Sound RAM sized 4 KB instead of 2. The map's window is 0xD000-0xD7FF, so
            # the extra 2 KB is unreachable through the bus and only the save state's
            # array length changes -- which is what makes this worth a mutant.
            (
                "sound-ram-is-four-kilobytes",
                "pub const RAM_BYTES: usize = 0x800;",
                "pub const RAM_BYTES: usize = 0x1000;",
                "KILL",
                "crates/machine/src/sound.rs",
            ),
            # CONTROL: `Debug` dropped from `SoundTrace`. Nothing formats it in a
            # passing run -- the assertion messages that would are on paths only a
            # failure takes -- so this compiles and changes no behaviour.
            #
            # Checked against every other mutant in both sets for the overlap the plan
            # warns about: nothing else here touches `SoundTrace`'s derives or the
            # suite's case selection, and the `ymsound` control's second edit is in
            # `testrunner`, which this set does not score against.
            (
                "CONTROL-sound-trace-loses-its-copy-derive",
                "#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]\npub struct SoundTrace {",
                "#[derive(Debug, Clone, PartialEq, Eq, Default)]\npub struct SoundTrace {",
                "SURVIVE",
                "crates/machine/src/sound.rs",
            ),
        ],
    ),
}


def run_rows(name: str) -> list[tuple[str, str, str]]:
    """Applies every mutant of one set, returning (name, expectation, result)."""
    default_src, mutants = SETS[name]

    def args_for(crates) -> list[str]:
        """`-p a -p b`, so one run scores a subject spanning crates."""
        if isinstance(crates, str):
            crates = [crates]
        return [a for c in crates for a in ("-p", c)]

    default_args = args_for(CRATES.get(name, "frontend"))
    # One backup per file the set touches, taken before the first mutant is applied
    # and restored in `finally` whatever happens. Keyed by path so a set spanning
    # five files cannot restore one of them from another's copy -- and `shutil.copy`
    # throughout, never `git checkout`, because a checkout would take uncommitted
    # work in those files with it.
    #
    # The sixth element's files count too. Missing them would leave a control's
    # second edit applied after the mutant that made it, which is the one state this
    # harness exists to never produce.
    files = {m[4] if len(m) > 4 else default_src for m in mutants}
    files |= {f for m in mutants if len(m) > 5 for f, _, _ in m[5]}
    backups = {}
    for i, src in enumerate(sorted(files)):
        backups[src] = f"/tmp/mutate-{name}-{i}.orig"
        shutil.copy(src, backups[src])
    rows = []
    try:
        for mutant in mutants:
            mname, old, new, expect = mutant[:4]
            src = mutant[4] if len(mutant) > 4 else default_src
            # Every edit this mutant makes, the main one first. The main one is
            # spelled as a triple here so the loops below have one shape; a mutant
            # with no sixth element produces exactly the single edit it always did.
            edits = [(src, old, new)] + list(mutant[5] if len(mutant) > 5 else [])
            crate_args = args_for(mutant[6]) if len(mutant) > 6 else default_args
            # Count first, write second. All-or-nothing: a mutant whose second edit
            # no longer matches must report NO-OP rather than apply its first half
            # and be scored, because half a mutant is a different mutant.
            counts = [(f, o, n2, open(backups[f]).read().count(o)) for f, o, n2 in edits]
            missed = [(f, c) for f, _, _, c in counts if c != 1]
            if missed:
                where = ", ".join(f"{f.rsplit('/', 1)[-1]}:{c}" for f, c in missed)
                rows.append((mname, expect, f"NO-OP ({where} matches)"))
                continue
            for f, o, n2, _ in counts:
                text = open(f).read()
                open(f, "w").write(text.replace(o, n2, 1))
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
                    ["cargo", "test", *crate_args, "--quiet"],
                    capture_output=True,
                    text=True,
                    timeout=MUTANT_TIMEOUT_S,
                )
                if r.returncode == 0:
                    got = "SURVIVE"
                else:
                    # `--quiet` still prints `test <name> ... FAILED` for a
                    # failure, so no extra run is needed to learn who killed it.
                    who = _FAILED.findall(r.stdout)
                    named = ", ".join(n.rsplit("::", 1)[-1] for n in who[:NAMED_KILLERS])
                    more = f" +{len(who) - NAMED_KILLERS}" if len(who) > NAMED_KILLERS else ""
                    # No name at all means the crate failed to *build* under the
                    # mutation. That is not a kill: nothing was measured, and
                    # counting it as one is how a mutant that does not compile
                    # inflates the score.
                    got = f"KILL ({named}{more})" if who else "NO-BUILD"
            except subprocess.TimeoutExpired:
                got = "KILL"
                mname = f"{mname} (hung)"
            finally:
                # Restore *inside* the loop, not only at the end. A single-file set
                # got away without this because every iteration rewrote the whole
                # file from the pristine backup; across files it would leave mutant
                # N applied while mutant N+1 ran, and the two together are a third
                # mutant whose result belongs to neither. Every file this mutant
                # wrote, not only `src`, for the same reason one file over.
                for f, _, _ in edits:
                    shutil.copy(backups[f], f)
            rows.append((mname, expect, got))
    finally:
        for path, backup in backups.items():
            shutil.copy(backup, path)
    return rows


def die_cleanly(signum, _frame):
    """Turns a kill signal into an exception, so `finally` gets to run.

    Measured, not hypothetical: a `--all` run was SIGTERM'd at a ten-minute
    wall-clock cap and left `f1-only-ever-turns-the-overlay-on` applied in
    `crates/frontend/src/debug.rs`. Nothing announced it -- the tree simply had a
    live mutant in tracked source, which is the one state this harness exists to
    never produce. `KeyboardInterrupt` unwinds through every `finally` and
    restores every backup; the default SIGTERM handler does not unwind at all.

    Ctrl-C already worked for the same reason. This makes SIGTERM and SIGHUP
    behave like it.
    """
    raise KeyboardInterrupt(f"signal {signum}")


for _sig in (signal.SIGTERM, signal.SIGHUP):
    signal.signal(_sig, die_cleanly)


def verdict(got: str) -> str:
    """The bare outcome, with the detail in parentheses stripped off.

    `KILL (a_test_name)` and `NO-OP (2 matches)` are both a verdict plus evidence.
    The comparison against the expectation must see only the verdict, or every
    mutant that names its killer would read as unexpected.
    """
    return got.split(" ", 1)[0]


def run(name: str) -> int:
    rows = run_rows(name)
    bad = 0
    for mname, expect, got in rows:
        ok = verdict(got) == expect
        bad += not ok
        print(f"{'ok  ' if ok else 'BAD '} {mname:46} expect {expect:8} got {got}")
    print(f"\n{len(rows) - bad}/{len(rows)} as expected")
    return 1 if bad else 0


def run_all() -> int:
    """Every set, one after another, with a roll-up.

    The point of running them together rather than one at a time: a set that has
    started reporting NO-OP because the code it mutates was reworded is invisible
    when you only run the set you are working on.
    """
    # NO-OP and NO-BUILD are counted apart. They are both "nothing was measured",
    # but they say different things about where the fault is: NO-OP means the
    # pattern no longer matches the source, NO-BUILD means it matched and the
    # result does not compile. Rolling them into one bucket made the roll-up
    # report `no-op 1` for a mutant whose pattern was fine, which sent the first
    # reading of it to the wrong file.
    total = killed = survived = noop = nobuild = bad = 0
    for name in SETS:
        print(f"=== {name} ===")
        rows = run_rows(name)
        for mname, expect, got in rows:
            ok = verdict(got) == expect
            bad += not ok
            total += 1
            if verdict(got) == "NO-OP":
                noop += 1
            elif verdict(got) == "NO-BUILD":
                nobuild += 1
            elif verdict(got) == "KILL":
                killed += 1
            else:
                survived += 1
            print(f"{'ok  ' if ok else 'BAD '} {mname:46} expect {expect:8} got {got}")
        print()
    print(
        f"total {total}  killed {killed}  survived {survived}  "
        f"no-op {noop}  no-build {nobuild}"
    )
    print(f"{total - bad}/{total} as expected")
    return 1 if bad else 0


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "--all":
        sys.exit(run_all())
    if len(sys.argv) != 2 or sys.argv[1] not in SETS:
        print(f"usage: {sys.argv[0]} {{{','.join(SETS)},--all}}", file=sys.stderr)
        sys.exit(2)
    sys.exit(run(sys.argv[1]))
