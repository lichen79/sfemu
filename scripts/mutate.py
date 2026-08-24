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
    "layout": "sfemu",
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
    # Five crates, and the breadth is the set's subject rather than laziness. The
    # chip's rules are killed by unit tests in `oki`, the clamp counter and the mix
    # by `machine`, the vector format and its runner by `testrunner`, the queueing
    # by `sfemu`, and the sound panel's row count by `frontend`. Scored against any
    # one of them most of the table would report SURVIVE for never having been
    # compiled, which is the failure mode `ymsound`'s comment above describes.
    "oki": ["oki", "machine", "testrunner", "sfemu", "frontend"],
}

# How long one mutant's test run may take before it is declared killed.
#
# ⚠️ Raised from 120. The premise behind that number was "the whole workspace suite is
# under 3 s, so this is two orders of magnitude of headroom" -- which measured the wrong
# thing: what a mutant costs is the *rebuild*, and `cargo test --workspace` on this tree
# now takes over 70 s to compile before a test runs. `CONTROL-the-title-is-not-highlighted`
# reported `KILL (hung)` at 120 s and then survived when run by hand, which is the failure
# mode this constant can produce: a timeout scores as a kill, so an over-tight budget
# manufactures kills for mutants nothing detected. 600 is generous against the build and
# still an order of magnitude over any real hang, which is an infinite loop in a frame.
MUTANT_TIMEOUT_S = 600

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
            # CONTROL: a pixel added to `~`, the one printable character no panel in
            # this repository ever draws. `every_glyph_is_distinct` still passes --
            # verified: the mutated bitmap collides with none of the other 94 -- and
            # `~` is not one of the sixteen hex digits the literals pin, which is the
            # boundary the module docs draw deliberately: distinctness for all 95,
            # exactness only for the characters the panels are made of. So this
            # survives, and it should.
            #
            # It replaces `CONTROL-line-height-unused-until-task-4`, which was honest
            # when written and stopped being so: `LINE` is now laid out by
            # `overlay.rs` and `gfxpanels.rs`, and D2 Task 12's sound panel made
            # `all_five_panels_can_be_shown_at_once_without_overlapping` kill it. A
            # control that a later task turns into a real mutant has to be replaced,
            # not re-argued -- see the run's own BAD row for it.
            (
                "CONTROL-a-pixel-added-to-a-character-no-panel-draws",
                'g!("....", "....", ".#.#", "#.#.", "....", "...."), // \'~\'',
                'g!("....", "....", ".#.#", "#.#.", "..#.", "...."), // \'~\'',
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
            # ⚠️ The five control mutants below were written against `edge(Key::X)` and
            # reported NO-OP after the key menu landed: every control now goes through
            # `ctl`, which is `!open && edge(k)`. Re-anchored on `ctl`, which is also the
            # sharper edit now -- `now.contains` drops both the edge *and* the menu
            # capture, so a killer must exist for each independently.
            (
                "pause-is-level-triggered",
                "pause_toggled: ctl(Key::F11),",
                "pause_toggled: now.contains(Key::F11),",
                "KILL",
            ),
            (
                "step-is-level-triggered",
                "step: ctl(Key::Period),",
                "step: now.contains(Key::Period),",
                "KILL",
            ),
            (
                "save-is-level-triggered",
                "save: ctl(Key::F5),",
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
                "breakpoint_toggled: ctl(Key::F7),",
                "breakpoint_toggled: now.contains(Key::F7),",
                "KILL",
            ),
            (
                "instruction-step-is-level-triggered",
                "step_instruction: ctl(Key::F4),",
                "step_instruction: now.contains(Key::F4),",
                "KILL",
            ),
            (
                "test-switch-is-edge-triggered",
                "inputs.test = now.contains(Key::F2);",
                "inputs.test = edge(Key::F2);",
                "KILL",
            ),
            # The map itself. ⚠️ These four moved out of `Controls::update` and into
            # `Preset` when the key menu landed: the six buttons are now read as
            # `p.p1_punch().map(...)`, so a mutant anchored on
            # `inputs.p1.kick = [now.contains(Key::I), ...]` reports NO-OP. Each one is
            # re-anchored on the *default* preset's row, which is the arrangement the old
            # pattern named -- and each is now a stronger claim, because the same edit is
            # invisible to any test that only ever exercises one preset.
            (
                "kick-reads-a-punch-key",
                "            Preset::AzertyPunchLow => [Key::I, Key::O, Key::P],\n            Preset::AzertyCabinet => [Key::K, Key::L, Key::M],",
                "            Preset::AzertyPunchLow => [Key::K, Key::L, Key::M],\n            Preset::AzertyCabinet => [Key::K, Key::L, Key::M],",
                "KILL",
            ),
            (
                "punch-key-order-swapped",
                "            Preset::AzertyPunchLow => [Key::K, Key::L, Key::M],\n            Preset::AzertyCabinet => [Key::I, Key::O, Key::P],\n            Preset::QwertyPunchLow => [Key::J, Key::K, Key::L],",
                "            Preset::AzertyPunchLow => [Key::L, Key::K, Key::M],\n            Preset::AzertyCabinet => [Key::I, Key::O, Key::P],\n            Preset::QwertyPunchLow => [Key::J, Key::K, Key::L],",
                "KILL",
            ),
            # The two button rows traded wholesale -- the arrangement this project had
            # until the remap, and the one a reader who knows a real cabinet would
            # "restore". Every key still reaches a real button, all six ports stay
            # distinct, and no key is duplicated or dropped: the only thing wrong is
            # which row is which. `GAME_KEY_PORTS` says per key, and `main.rs`'s
            # usage-text test presses both ends of both rows for exactly this.
            # ⚠️ Re-anchored with the rest of the map: the rows are the preset's now, and
            # the swap is a swap of the two `match` bodies. Which makes it the mutant this
            # set most needs, because a preset table read by a loop over `Preset::ALL`
            # agrees with itself no matter which row is which.
            (
                "the-button-rows-are-swapped",
                "    pub const fn p1_punch(self) -> [Key; 3] {\n        match self {\n            Preset::AzertyPunchLow => [Key::K, Key::L, Key::M],\n            Preset::AzertyCabinet => [Key::I, Key::O, Key::P],\n            Preset::QwertyPunchLow => [Key::J, Key::K, Key::L],\n            Preset::QwertyCabinet => [Key::I, Key::O, Key::P],",
                "    pub const fn p1_punch(self) -> [Key; 3] {\n        match self {\n            Preset::AzertyPunchLow => [Key::I, Key::O, Key::P],\n            Preset::AzertyCabinet => [Key::K, Key::L, Key::M],\n            Preset::QwertyPunchLow => [Key::I, Key::O, Key::P],\n            Preset::QwertyCabinet => [Key::J, Key::K, Key::L],",
                "KILL",
            ),
            # And the same trade on the keypad, which is the half a partial revert leaves
            # behind: P1 fixed, P2 not. The two players would then disagree about which
            # row punches, and every per-key port assertion for P1 still passes.
            (
                "the-keypad-rows-are-swapped",
                "    pub const fn p2_punch(self) -> [Key; 3] {\n        match self {\n            Preset::AzertyPunchLow | Preset::QwertyPunchLow => {\n                [Key::NumPad4, Key::NumPad5, Key::NumPad6]\n            }\n            Preset::AzertyCabinet | Preset::QwertyCabinet => {\n                [Key::NumPad7, Key::NumPad8, Key::NumPad9]\n            }",
                "    pub const fn p2_punch(self) -> [Key; 3] {\n        match self {\n            Preset::AzertyPunchLow | Preset::QwertyPunchLow => {\n                [Key::NumPad7, Key::NumPad8, Key::NumPad9]\n            }\n            Preset::AzertyCabinet | Preset::QwertyCabinet => {\n                [Key::NumPad4, Key::NumPad5, Key::NumPad6]\n            }",
                "KILL",
            ),
            ("coin-is-a-start-key", "inputs.coin1 = now.contains(Key::Num5);", "inputs.coin1 = now.contains(Key::Num1);", "KILL"),
            ("stick-up-is-down", "inputs.p1.up = now.contains(Key::Z);", "inputs.p1.up = now.contains(Key::S);", "KILL"),
            # A P1 key writing a P2 field. This was the mutant the old blanket
            # "no key reaches P2" test caught; with both players mapped, what catches it
            # is `each_game_key_clears_its_own_port_bit` asserting all three ports per
            # key -- `p2.right` moves IN1's *high* byte, and D's row says 0xFFFE.
            (
                "a-p1-key-reaches-player-two",
                "inputs.p1.right = now.contains(Key::D);",
                "inputs.p2.right = now.contains(Key::D);",
                "KILL",
            ),
            # And the reverse, which only exists now that P2 is mapped: the two
            # clusters' stick assignments are eight identical-looking lines, and a
            # copy-paste that left `p1` on a P2 row gives one player two up keys and the
            # other none.
            (
                "a-p2-key-reaches-player-one",
                "inputs.p2.up = now.contains(Key::Up);",
                "inputs.p1.up = now.contains(Key::Up);",
                "KILL",
            ),
            # The number row and the keypad confused for each other: P2's forward kick
            # reading the coin key. Plausible because `Num5` and `NumPad5` differ by
            # three characters, and it makes inserting a coin throw a kick.
            # ⚠️ Re-anchored on the preset's punch row for the same reason as the rest.
            # `Num5` and not `Num9`: there is no `Key::Num9` — only the four number-row
            # keys the coins and starts need exist — and a mutant that does not compile
            # scores as a KILL nothing was measured for.
            (
                "the-keypad-is-the-number-row",
                "            Preset::AzertyPunchLow | Preset::QwertyPunchLow => {\n                [Key::NumPad4, Key::NumPad5, Key::NumPad6]\n            }",
                "            Preset::AzertyPunchLow | Preset::QwertyPunchLow => {\n                [Key::NumPad4, Key::Num5, Key::NumPad6]\n            }",
                "KILL",
            ),
            # A game key losing its port row -- the bug a hand probe found in the test
            # that claims to prevent it. `the_port_bit_table_covers_every_game_key`
            # derived its list from `Key::ALL` and asserted the *count* was 25, never
            # reading the table, so deleting `NumPad6`'s row and editing the length
            # annotation to 24 left P2's roundhouse kick with no port assertion at all
            # and every `keys` test green.
            #
            # This is that bug as one replacement. `F3` is a control: it presses nothing,
            # so all three ports read idle and its row's assertions pass, and the triple
            # is unique so the pairwise check passes too. The *only* thing wrong is that
            # `NumPad6` has no row and a control has one -- which is exactly and solely
            # what the rewritten test compares. Expect one killer name, not two.
            #
            # The literals track the row: `NumPad6` is P2's *fierce punch* since the
            # keypad's rows were reversed, so it clears IN1's bit 14 and leaves IN2 idle.
            # This pattern reported NO-OP on the first pass after that remap -- which is
            # the harness working: a mutant anchored on a port value cannot survive its
            # own subject being renumbered, and NO-OP says so instead of scoring a KILL
            # nothing was measured for.
            (
                "a-game-key-loses-its-port-row",
                '(Key::NumPad6, 0xFF, 0xBFFF, 0xFF, "P2 fierce"),',
                '(Key::F3, 0xFF, 0xFFFF, 0xFF, "P2 fierce"),',
                "KILL",
            ),
            # Two keys sharing a bit.
            ("two-keys-share-a-bit", "Key::D => 3,", "Key::D => 1,", "KILL"),
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
            # The denominator, written down this time: 44 keys hold bits 0-43, and
            # `KeySet` is a `u64`, so everything from 44 up is free. 62 leaves room
            # above and below. This control will die again if a key is ever given bit
            # 62, and that death is the signal it exists for, not a mutant to
            # re-expect.
            #
            # It survived the P1/P2 remap without moving, which is the first time it has:
            # player 2's ten keys took bits 34-43 rather than being interleaved with the
            # existing ones, precisely so no bit that already had a key changed.
            ("CONTROL-escape-moves-to-another-free-bit", "Key::Escape => 21,", "Key::Escape => 62,", "SURVIVE"),
            # --- The key menu's capture. ---
            #
            # The gate deleted: the board reads the keyboard while the menu is up, so your
            # fighter walks as you scroll the rows. Nothing in the menu's own tests can see
            # this -- they assert on rows and selections -- and nothing in the port table
            # can either, since the ports are still correct for the keys held.
            ("the-menu-does-not-capture-the-board", "        if !self.menu_open {", "        if true {", "KILL"),
            # The subtler half of the same bug: the buttons are gated but the stick is not.
            # A partial capture, which is what a hand-written `if` around only the block
            # that was easy to indent looks like.
            (
                "the-menu-captures-the-buttons-but-not-the-stick",
                "        if !self.menu_open {\n            let p = self.preset;",
                "        inputs.p1.up = now.contains(Key::Z);\n        if !self.menu_open {\n            let p = self.preset;",
                "KILL",
            ),
            # "Ignored" instead of *idle*: the board keeps whatever it had. This one needs
            # `Inputs` to be understood as level-triggered to see at all -- a stick held
            # when the menu opened stays held for as long as the menu is up -- and it is
            # why the loop tests assert `board.inputs` rather than an `Actions` field.
            #
            # Expressed as the gate reading the *previous* frame's keys, which is the same
            # symptom with a real cause: a capture written as "keep the last live value".
            (
                "an-open-menu-keeps-the-last-held-keys",
                "        if !self.menu_open {\n            let p = self.preset;",
                "        let now = if self.menu_open { self.was } else { now };\n        if !self.menu_open || true {\n            let p = self.preset;",
                "KILL",
            ),
            # The control gate dropped, so `Escape` quits from inside the menu and `Enter`
            # both applies a preset and acts on the graphics view. `ctl` becoming `edge` is
            # the single edit that does it, and it is exactly what "the menu only needs to
            # gate the board" would produce.
            ("the-menu-does-not-capture-the-controls", "let ctl = |k: Key| !open && edge(k);", "let ctl = |k: Key| edge(k);", "KILL"),
            # `Tab` gated like everything else, which is the one control that must not be:
            # the menu would open and never close, and the only way out would be the window
            # button. Note the mutant is *more* consistent-looking than the real code.
            ("tab-is-gated-like-the-other-controls", "menu_toggled: edge(Key::Tab),", "menu_toggled: ctl(Key::Tab),", "KILL"),
            # The navigation ungated, so the arrows both move the cursor and move P2's
            # stick, and `Enter` applies a preset while the menu is shut. Survivable-looking
            # because with the menu closed there is no cursor to move -- until you notice
            # `menu_apply` firing on every `Enter` press in the graphics viewer.
            ("the-menu-navigates-while-it-is-shut", "menu_apply: open && edge(Key::Enter),", "menu_apply: edge(Key::Enter),", "KILL"),
            # A preset applied but never read: `set_preset` stores it and `update` keeps
            # using the default. The menu still highlights the new row, and the `.keys` file
            # still records it, so everything a test that only reads the menu's own state
            # can see is right -- and no button has moved.
            ("the-preset-is-stored-but-never-read", "            let p = self.preset;", "            let p = Preset::default();", "KILL"),
            # A tag that round-trips through itself. Two presets sharing a tag means the
            # file written for one loads as the other, and `from_tag`'s `find` returns the
            # first match -- so `qwerty-cabinet` silently becomes `qwerty-punch-low`.
            (
                "two-presets-share-a-tag",
                'Preset::QwertyCabinet => "qwerty-cabinet",',
                'Preset::QwertyCabinet => "qwerty-punch-low",',
                "KILL",
            ),
            # `from_tag` stops trimming, so the newline `write_keys` appends makes every
            # saved file unreadable and every session opens on the default. The menu works
            # perfectly within a session, which is what makes it a bug report of the form
            # "it forgets".
            ("from-tag-stops-trimming", "p.tag() == s.trim()", "p.tag() == s", "KILL"),
            # CONTROL: the presets' *order* in `ALL` is the order the menu lists them, and
            # nothing else depends on it -- `from_tag` searches, and no test asserts a row
            # index against a literal. Swapping the two QWERTY rows changes which line of
            # the menu they occupy and nothing about what any key does.
            #
            # It is the right control for this set because it looks like it must fail: it
            # edits the same table three real mutants above edit. It fails only if a test
            # has started asserting a row number instead of a row's content.
            (
                "CONTROL-the-two-qwerty-rows-trade-places",
                "        Preset::QwertyPunchLow,\n        Preset::QwertyCabinet,\n    ];",
                "        Preset::QwertyCabinet,\n        Preset::QwertyPunchLow,\n    ];",
                "SURVIVE",
            ),
        ],
    ),
    # The key menu's state machine. Its characteristic bug is not a wrong pixel and not a
    # wrong port -- it is a cursor that lands on the wrong row, or a frame on which two
    # meanings of one key both fire. Five rows and five actions is a small machine, and
    # every mutant below is an edit that leaves it looking entirely reasonable.
    "menu": (
        "crates/frontend/src/menu.rs",
        [
            # CONTROL, and it was written expecting KILL. The claim was that matching a row
            # by `preset()` rather than by `Use(current)` would open the cursor on
            # `restore defaults` whenever the default is in force, since `RestoreDefaults`
            # also *has* a preset. It survives, and the reason is worth the comment: two
            # rows do match, but `position` returns the *first* and `RestoreDefaults` is
            # last in `ALL`. The loose form is correct today by the order of that array
            # alone.
            #
            # Kept as a control rather than deleted, because that is the honest reading:
            # the edit is safe, and it becomes unsafe the day a row is inserted before the
            # presets. It dies then, which is the signal it exists for.
            (
                "CONTROL-opening-matches-a-row-by-its-preset",
                ".position(|r| *r == MenuRow::Use(current))",
                ".position(|r| r.preset() == current)",
                "SURVIVE",
            ),
            # Opening does not move the cursor at all, so it stays wherever it was left.
            # Harmless on the first open (`sel` starts at 0, which is the default's row) and
            # wrong on every one after, which is why the test opens it twice.
            (
                "opening-does-not-move-the-cursor",
                "            if self.open {\n                // Opening puts the cursor on the row that is in force.",
                "            if false {\n                // Opening puts the cursor on the row that is in force.",
                "KILL",
            ),
            # The toggle stops returning early, so the frame that opens the menu also reads
            # that frame's navigation -- and `Tab`-with-`Enter` on one frame would open and
            # immediately apply. It cannot happen from a keyboard often, and that is the
            # point: a rule that holds only because a human is slow is not a rule.
            (
                "opening-also-reads-this-frames-navigation",
                "            }\n            return None;\n        }\n        if !self.open {",
                "            }\n        }\n        if !self.open {",
                "KILL",
            ),
            # Close and apply on the same frame, resolved the other way: apply wins, so
            # cancelling a row you did not want applies it. Both orders are one line apart
            # and only one is safe.
            (
                "apply-beats-close-on-a-shared-frame",
                "        if a.menu_close {\n            self.open = false;\n            return None;\n        }\n        if a.menu_up {",
                "        if a.menu_apply {\n            return Some(self.selected().preset());\n        }\n        if a.menu_close {\n            self.open = false;\n            return None;\n        }\n        if a.menu_up {",
                "KILL",
            ),
            # The second gate deleted. It is the one that looks redundant -- `Controls`
            # already withholds every one of these actions while the menu is shut -- and
            # deleting it makes this module's own tests rest on a guarantee made in another
            # file, which is precisely what its comment says it exists to prevent.
            (
                "the-shut-menu-reads-actions-anyway",
                "        if !self.open {\n            // Every action below is already gated",
                "        if false {\n            // Every action below is already gated",
                "KILL",
            ),
            # The cursor wraps instead of saturating: `Up` on the top row jumps to
            # `restore defaults`. Five rows, so this is one keypress from "restore" every
            # time a player overshoots upward.
            ("the-cursor-wraps-at-the-top", "self.sel = self.sel.saturating_sub(1);", "self.sel = (self.sel + MenuRow::ALL.len() - 1) % MenuRow::ALL.len();", "KILL"),
            # Off by one at the bottom, in the direction that is not a panic: the last row
            # is unreachable. `restore defaults` would be visible and unselectable, and
            # nothing crashes -- which is why the fencepost is written `.min(len - 1)` and
            # asserted by walking the whole list.
            ("the-last-row-is-unreachable", "self.sel = (self.sel + 1).min(MenuRow::ALL.len() - 1);", "self.sel = (self.sel + 1).min(MenuRow::ALL.len() - 2);", "KILL"),
            # Apply closes the menu, which is the mockup's own reading of "Enter apply" and
            # is still wrong: the confirmation a player needs is `(current)` moving to the
            # row they chose, and a menu that vanishes shows them nothing.
            (
                "apply-closes-the-menu",
                "        if a.menu_apply {\n            return Some(self.selected().preset());",
                "        if a.menu_apply {\n            self.open = false;\n            return Some(self.selected().preset());",
                "KILL",
            ),
            # The restore row applies the row above it instead of the default -- the shape a
            # `RestoreDefaults => self.selected()` recursion or an off-by-one row lookup
            # would take. Killed only because the restore row's preset is asserted as a
            # literal rather than as "whatever the last row says".
            (
                "the-restore-row-applies-the-last-preset",
                "            MenuRow::RestoreDefaults => Preset::default(),",
                "            MenuRow::RestoreDefaults => Preset::QwertyCabinet,",
                "KILL",
            ),
            # A summary row that previews the *active* preset rather than the highlighted
            # one, so the keys shown never change as you scroll. This is the whole reason
            # `draw` takes both `menu` and `current`, and the mutant makes one of them
            # unused in the place it matters.
            (
                "the-summary-previews-the-active-row",
                "    let p = menu.selected().preset();",
                "    let p = current;",
                "KILL",
            ),
            # A label that names the wrong keys: the QWERTY punch row shown as AZERTY's.
            # `K L M` and `J K L` are both plausible home-row runs, and a player reading the
            # menu has no way to tell which is true except by pressing.
            (
                "a-label-names-the-other-layouts-keys",
                '        Preset::QwertyPunchLow => "J K L",',
                '        Preset::QwertyPunchLow => "K L M",',
                "KILL",
            ),
            # The stick label following which row punches rather than which board the keys
            # are named for, which reads correctly for two of the four presets.
            #
            # ⚠️ Written first as a swap of the *left* halves of the two arms and reported
            # NO-BUILD: that leaves `AzertyCabinet` and `QwertyPunchLow` unmatched, and a
            # mutant that does not compile scores as a KILL nothing was measured for. Both
            # arms are replaced together here, so the match stays exhaustive and the only
            # thing wrong is which two presets say `Z S Q D`.
            (
                "the-stick-label-follows-the-wrong-preset",
                '        Preset::AzertyPunchLow | Preset::AzertyCabinet => "Z S Q D",\n        Preset::QwertyPunchLow | Preset::QwertyCabinet => "W A S D",',
                '        Preset::AzertyPunchLow | Preset::QwertyPunchLow => "Z S Q D",\n        Preset::AzertyCabinet | Preset::QwertyCabinet => "W A S D",',
                "KILL",
            ),
            # A shut menu draws anyway, so the box sits over the game permanently. Obvious
            # at a window and invisible to every test that only drives the state machine,
            # which is why `a_shut_menu_draws_nothing` reads the buffer.
            ("a-shut-menu-draws-anyway", "    if !menu.open {\n        return;\n    }", "    if false {\n        return;\n    }", "KILL"),
            # The box one column narrower than its widest row. `draw_text` clips rather than
            # panicking, so the symptom is a silently truncated row -- `(current)` losing its
            # closing bracket -- and nothing fails unless a test measures the rows.
            ("the-box-is-a-column-too-narrow", "pub const COLS: usize = 34;", "pub const COLS: usize = 32;", "KILL"),
            # The box off centre by two lines. ⚠️ Written as a control -- "the exact position
            # is arbitrary, nothing overlaps it" -- and it died, correctly. Centring is a
            # stated property of this panel, not a coincidence of two numbers: it is the one
            # modal panel, and `the_box_is_centred_and_on_screen` asserts
            # `HEIGHT - (MENU_Y + h) <= MENU_Y` rather than only the literal. So the edit
            # violates an asserted design property and KILL is the honest expectation.
            #
            # Re-expected rather than deleted, because the death proves that centring
            # assertion is not vacuous.
            ("the-box-sits-two-lines-off-centre", "pub const MENU_Y: usize = (HEIGHT - (ROWS * LINE + 2 * PAD)) / 2;", "pub const MENU_Y: usize = (HEIGHT - (ROWS * LINE + 2 * PAD)) / 2 - 2 * LINE;", "KILL"),
            # CONTROL: the title's colour. `HI` marks the row the cursor is on, and the title
            # is drawn in it for emphasis only -- nothing distinguishes a row from a heading
            # by colour, since the cursor's `"> "` does that. Dropping it to `FG` is a
            # cosmetic difference no headless test can see: `an_open_menu_draws_inside_its_box_only`
            # counts changed pixels against the box's whole area, so an opaque box of either
            # colour passes, and the summary-band comparison never reaches the title row.
            #
            # The right control for this set: a *visible* edit -- it looks like it must fail
            # -- that nothing can assert without a person at a window. It fails only if a
            # test starts reading a specific pixel's colour, which would be the assertion to
            # question rather than this edit.
            ('CONTROL-the-title-is-not-highlighted', 'line(buf, "KEYS", HI);', 'line(buf, "KEYS", FG);', "SURVIVE"),
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
    # The keyboard-layout boundary. `minifb::Key` names a hardware *position* after the
    # letter US QWERTY prints on it -- the active layout is never consulted -- so P1's
    # stick is mapped `M::W`/`M::A` to land on the keys an AZERTY keyboard labels Z and Q.
    #
    # This set exists because the correct code looks like a typo and the wrong code looks
    # correct. Every mutant here is an edit a reader would make in good faith while
    # "tidying", and each one silently moves P1's stick off the diamond on the keyboard
    # this is actually played on. Nothing else in the project can see it: `frontend`'s 17
    # `keys` tests never touch a keyboard, and `display`'s reachability test only asks
    # that *some* key produce each variant.
    "layout": (
        "crates/sfemu/src/display.rs",
        [
            # The tidy-up: put P1's up key back on the US-QWERTY-named position. Moves it
            # from the key labelled Z to the one labelled W.
            (
                "p1-up-follows-the-us-letter",
                "        M::W => Key::Z,",
                "        M::Z => Key::Z,",
                "KILL",
            ),
            # The same for left: from the key labelled Q to the one labelled A.
            (
                "p1-left-follows-the-us-letter",
                "        M::A => Key::Q,",
                "        M::Q => Key::Q,",
                "KILL",
            ),
            # "Support both layouts" -- map the QWERTY position *as well*. This is the
            # subtlest of the three, because on an AZERTY keyboard it still works: the
            # diamond is intact and W merely also presses up. What it destroys is the
            # one-key-one-input property, and it is the edit most likely to be proposed,
            # which is why the test asserts `M::Z => None` rather than only `M::W => Some`.
            (
                "both-layouts-press-p1-up",
                "        M::W => Key::Z,",
                "        M::W | M::Z => Key::Z,",
                "KILL",
            ),
            # The third trap, and the one with the worst symptom. AZERTY puts `M` on the
            # home row at position 0x29, which `minifb` names `Semicolon` after the US
            # letter printed there; `M::M` is 0x2e, the key labelled `,` here. So this
            # mutant -- which is simply "the obvious spelling" -- moves P1's fierce punch
            # off the home row onto the comma key. Nothing in `frontend` can see it, and
            # `every_frontend_key_can_be_produced_by_a_keypress` still finds exactly one
            # producer for `Key::M`; only the position assertion catches it.
            (
                "p1-fierce-follows-the-us-letter",
                "        M::Semicolon => Key::M,",
                "        M::M => Key::M,",
                "KILL",
            ),
            # And the both-layouts variant of it, for the same reason as `M::W` above: on
            # this keyboard the punch still works, and a second key merely also throws it.
            (
                "both-layouts-press-p1-fierce",
                "        M::Semicolon => Key::M,",
                "        M::Semicolon | M::M => Key::M,",
                "KILL",
            ),
            # CONTROL: five of P1's six buttons sit in the same position on AZERTY and
            # QWERTY -- `M` is the exception, which is what the two mutants above are
            # about -- so reordering the layout-stable arms changes nothing. The right
            # control for this set: it is the edit in this exact neighbourhood that *is*
            # safe, and it fails only if a test has started asserting match-arm order
            # instead of behaviour.
            (
                "CONTROL-punch-arms-reordered",
                "        M::I => Key::I,\n        M::O => Key::O,\n        M::P => Key::P,",
                "        M::O => Key::O,\n        M::I => Key::I,\n        M::P => Key::P,",
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
                # Rewritten in D3: `SoundBoard::restore` gained a sixth argument (the
                # OKI's voices), so the call is no longer the one-line form this pattern
                # was written against and the mutant reported NO-OP. Caught by running
                # `--all` rather than the set under work, which is what `--all` is for.
                "ym2151-dropped-from-the-save-state",
                "            &s.ym,\n            s.ym_addr,",
                "            &ym2151::Ym2151::new(),\n            s.ym_addr,",
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
            # CONTROL: `SoundBoard`'s hand-written `Debug` reports a constant bank
            # instead of the real one. Nothing in this repository formats a
            # `SoundBoard` -- checked: the only `{:?}` uses in `machine` and `frontend`
            # are on `gfx` state and on assertion messages for other types -- so the
            # wrong number is written into a string no passing or failing test reads.
            #
            # It replaces `CONTROL-sound-trace-loses-its-copy-derive`, which the run
            # scored NO-BUILD: dropping `Copy` from `SoundTrace` breaks
            # `pub const fn trace(&self) -> SoundTrace`, so nothing was measured, and a
            # mutant that does not compile is worthless however it scores. Dropping
            # `Debug` instead would not compile either -- the impl below formats
            # `trace` -- which is why the target moved to the formatter itself.
            #
            # This is a genuine gap and named as one: if D3's overlay ever prints a
            # sound board, this control becomes a real mutant and must be replaced
            # rather than argued with -- which is exactly what happened to `font`'s.
            #
            # Checked against every other mutant in both sets for the overlap the plan
            # warns about: nothing else here touches this impl (the bank mutants edit
            # the 0xF004 write arm and the `BANKS` constant), and the `ymsound`
            # control's second edit is in `testrunner`, which this set does not score.
            (
                "CONTROL-the-debug-impl-reports-a-constant-bank",
                '            .field("bank", &self.bank)',
                '            .field("bank", &0u8)',
                "SURVIVE",
                "crates/machine/src/sound.rs",
            ),
        ],
    ),
    # D3 Task 14: the OKI MSM6295 and everything the samples pass through on the way
    # to a speaker -- the decoder, the command protocol, the vector format and its
    # runner, the mix, the resampler, the host ring, the frontend's sound panel, and
    # the save state's version. One set across five crates, for `ymsound`'s reason
    # stated in `CRATES` above.
    "oki": (
        "crates/oki/src/chip.rs",
        [
            # --- The ADPCM decoder (`crates/oki/src/adpcm.rs`) ---
            # One entry of the step table off by one. The table is a literal because
            # Rust has no `const fn` float `pow`, so the only thing standing between a
            # typo and 49 wrong step values is the test that rederives it in f64.
            (
                "steptable",
                "88, 97,",
                "87, 97,",
                "KILL",
                "crates/oki/src/adpcm.rs",
            ),
            # The largest step-index adjustment reduced from 8 to 7. A lone `F` from
            # reset lands on step 8, so exactly one nibble sees this.
            (
                "stepshift",
                "const INDEX_SHIFT: [i8; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];",
                "const INDEX_SHIFT: [i8; 8] = [-1, -1, -1, -1, 2, 4, 6, 7];",
                "KILL",
                "crates/oki/src/adpcm.rs",
            ),
            # The unconditional 1/8 term dropped, which is what makes nibble 0 not a
            # no-op. Every even step value then produces 0, 4, 8 rather than 2, 6, 10.
            (
                "diffeighth",
                "    let mut d = sv / 8;",
                "    let mut d = 0;",
                "KILL",
                "crates/oki/src/adpcm.rs",
            ),
            # Bit 1 weighted at 1 rather than 1/2. Coarser than `diffeighth` and it
            # tests the same claim from the other end: the weights are 1, 1/2, 1/4.
            (
                "diffsumfirst",
                "        d += sv / 2;",
                "        d += sv;",
                "KILL",
                "crates/oki/src/adpcm.rs",
            ),
            # The lower clamp one short of the two's-complement floor. Asymmetric by
            # one, which is inaudible and wrong in 1,000 vector cases.
            (
                "signalclamp",
                "pub const SIGNAL_MIN: i16 = -2048;",
                "pub const SIGNAL_MIN: i16 = -2047;",
                "KILL",
                "crates/oki/src/adpcm.rs",
            ),
            # The step index floored at 1 instead of 0, so a decoder fed silence never
            # returns to its smallest step. Killed by the reset test and by the suite.
            (
                "stepclamp",
                "self.step = step.clamp(0, (STEPS - 1) as i32) as u8;",
                "self.step = step.clamp(1, (STEPS - 1) as i32) as u8;",
                "KILL",
                "crates/oki/src/adpcm.rs",
            ),
            # --- The chip: nibble order, tracing, and the clamp report ---
            # The `^ 4` dropped, so the *low* nibble is decoded first. Every sample
            # pair is then swapped -- audible as a rasp, and exact in the suite.
            (
                "nibbleorder",
                "let shift = ((voice.sample & 1) << 2) ^ 4;",
                "let shift = (voice.sample & 1) << 2;",
                "KILL",
            ),
            # The traced nibbles packed voice 3 first. The sample is unaffected, so
            # only the trace's own test and the runner's nibble check can see it.
            (
                "nibblepack",
                "nibbles |= u16::from(nibble) << (4 * i);",
                "nibbles |= u16::from(nibble) << (4 * (3 - i));",
                "KILL",
            ),
            # A non-playing voice contributing a nibble. `an_idle_chip_traces_no_nibbles`
            # is the whole guard: the sample is still 0, so nothing else notices.
            (
                "nibbleidle",
                "let mut nibbles = 0u16;",
                "let mut nibbles = 0x000Fu16;",
                "KILL",
            ),
            # The clamp report hardwired false, so the board's clip counter never moves.
            (
                "clampreport",
                "(clamped, nibbles, clamped != sum)",
                "(clamped, nibbles, false)",
                "KILL",
            ),
            # The report re-derived from the output instead of from the comparison, which
            # is the bug the three-value form exists to prevent: a sum landing exactly on
            # the bound was not clipped, and the output alone cannot say so.
            (
                "clamponbound",
                "clamped != sum",
                "clamped.abs() == CLAMP_2X",
                "KILL",
            ),
            # The counted form no longer returning the plain form's sample. The vector
            # suite drives `step_2x_traced`, so only `the_three_step_forms_agree` compares
            # the counted projection against the checked one.
            (
                "tracedrift",
                "        let (sample, _, clamped) = self.step_all(rom);",
                "        let (sample, _, clamped) = (0, 0, self.step_all(rom).2);",
                "KILL",
            ),
            # --- The command protocol ---
            # MAME's "fixes Got-cha and Steel Force" skip removed, so a start command
            # restarts a voice that is already playing.
            (
                "voiceskip",
                "                if voice.playing {\n                    continue;\n                }",
                "                if false {\n                    continue;\n                }",
                "KILL",
            ),
            # The stop mask shifted by four rather than three, so a stop hits the wrong
            # voices -- or none.
            (
                "stopshift",
                "let mask = command >> 3;",
                "let mask = command >> 4;",
                "KILL",
            ),
            # A degenerate phrase (start == stop) accepted rather than refused, which
            # MAME logs and skips.
            (
                "startstop",
                "if start < stop {",
                "if start <= stop {",
                "KILL",
            ),
            # The phrase one sample-pair short, so every voice stops two nibbles early.
            (
                "phrasecount",
                "count: 2 * (stop - start + 1),",
                "count: 2 * (stop - start),",
                "KILL",
            ),
            # The phrase table's stride halved: every phrase reads another phrase's
            # pointers. Nothing hand-written pins the stride; this is the suite's.
            (
                "phrasestride",
                "let base = u32::from(phrase) * 8;",
                "let base = u32::from(phrase) * 4;",
                "KILL",
            ),
            # The chip's own clamp removed. Four loud voices then exceed +-1.0, which
            # the real chip cannot do.
            (
                "chipclamp",
                "pub const CLAMP_2X: i32 = 65_536;",
                "pub const CLAMP_2X: i32 = i32::MAX;",
                "KILL",
            ),
            # Volume index 9 no longer silent. The smallest audible wrong volume there
            # is, and the only thing that sees it is the test that sums the energy.
            (
                "volindex9",
                "0x02, 0, 0,",
                "0x02, 1, 0,",
                "KILL",
            ),
            # Every voice at half its volume: the mix is quiet rather than wrong-shaped,
            # so this is the suite's exact-value check.
            (
                "volhalf",
                "sum += i32::from(signal) * i32::from(voice.volume);",
                "sum += i32::from(signal) * i32::from(voice.volume) / 2;",
                "KILL",
            ),
            # The idle status byte's high nibble wrong by a bit. The Z80 polls this to
            # find a free voice, so a wrong high nibble is a driver that never starts one.
            (
                "statusidle",
                "pub const STATUS_IDLE: u8 = 0xF0;",
                "pub const STATUS_IDLE: u8 = 0xE0;",
                "KILL",
            ),
            # --- The board: pin 7, the rate, the mix, the hold, the clip counter ---
            # The board constructed at the wrong divisor. Pin 7 starts *high* on CPS-1,
            # and starting it low is a 25% pitch error in every sample the game plays.
            (
                "pin7default",
                "            oki_pin7: true,",
                "            oki_pin7: false,",
                "KILL",
                "crates/machine/src/sound.rs",
            ),
            # The pin read from bit 1 rather than bit 0 of the 0xF006 write.
            (
                "pin7bit",
                "0xF006 => self.oki_pin7 = val & 0x01 != 0,",
                "0xF006 => self.oki_pin7 = val & 0x02 != 0,",
                "KILL",
                "crates/machine/src/sound.rs",
            ),
            # The fast ratio's numerator off by one. Inaudible per sample and a
            # divergence that accumulates over a session, which is why the constant is
            # derived from the two clocks rather than measured.
            (
                "okiratio",
                "pub const OKI_PER_YM_NUM_PIN7_HIGH: u32 = 3_200_000;",
                "pub const OKI_PER_YM_NUM_PIN7_HIGH: u32 = 3_200_001;",
                "KILL",
                "crates/machine/src/timing.rs",
            ),
            # The OKI's clocks per scanline off by one, so it no longer divides exactly.
            (
                "okiclocks",
                "pub const OKI_CLOCKS_PER_LINE: u32 = 64;",
                "pub const OKI_CLOCKS_PER_LINE: u32 = 65;",
                "KILL",
                "crates/machine/src/timing.rs",
            ),
            # MAME's YM weight reduced from 7 to 6: the FM is quiet against the samples.
            (
                "mixym",
                "let numerator = 7 * (i32::from(ym_l) + i32::from(ym_r)) + 3 * oki_2x;",
                "let numerator = 6 * (i32::from(ym_l) + i32::from(ym_r)) + 3 * oki_2x;",
                "KILL",
                "crates/machine/src/cps1.rs",
            ),
            # The OKI weight doubled, the same error the other way round.
            (
                "mixoki",
                "let numerator = 7 * (i32::from(ym_l) + i32::from(ym_r)) + 3 * oki_2x;",
                "let numerator = 7 * (i32::from(ym_l) + i32::from(ym_r)) + 6 * oki_2x;",
                "KILL",
                "crates/machine/src/cps1.rs",
            ),
            # The divisor halved, which doubles the mix and takes it past `i16` -- the
            # saturation test is exactly the claim that the weights sum to the divisor.
            (
                "mixdiv",
                "(numerator / 20) as i16",
                "(numerator / 10) as i16",
                "KILL",
                "crates/machine/src/cps1.rs",
            ),
            # The rate accumulator rebuilt from scratch on every YM tick rather than
            # carrying its remainder. This one **survived** when it was first written,
            # because every other test leaves pin 7 at its power-up value and never
            # writes 0xF006 mid-run -- see `a_pin_seven_write_does_not_move_the_okis_phase`,
            # which exists because of it.
            (
                "okiaccswap",
                "self.oki_acc = RationalAccumulator::with_remainder(num, den, self.oki_acc.remainder());",
                "self.oki_acc = RationalAccumulator::new(num, den);",
                "KILL",
                "crates/machine/src/cps1.rs",
            ),
            # The sample-and-hold removed: the chip's level is discarded, so the mix
            # hears silence between the chip's own steps. The OKI runs at one step per
            # ~7 YM ticks, so this is 6 zeros in every 7 samples.
            (
                "okihold",
                "                self.oki_last = self.sound.oki_step_2x();",
                "                let _ = self.sound.oki_step_2x();",
                "KILL",
                "crates/machine/src/cps1.rs",
            ),
            # The clip counter incremented by zero. Killed only by a test that asserts
            # the *count*, not `> 0`.
            (
                "okiclampcount",
                "self.trace.oki_clamps = self.trace.oki_clamps.saturating_add(1);",
                "self.trace.oki_clamps = self.trace.oki_clamps.saturating_add(0);",
                "KILL",
                "crates/machine/src/sound.rs",
            ),
            # Every sample counted as a clip, which makes the panel's CLP row a sample
            # counter and useless as a "why does it sound wrong" signal.
            (
                "okiclampalways",
                "        if clamped {",
                "        if true {",
                "KILL",
                "crates/machine/src/sound.rs",
            ),
            # --- The vector format and its runner (`testrunner`) ---
            # The recorded nibbles parsed as zero while still consuming their bytes, so
            # the format stays self-consistent and every case's nibble check compares
            # against 0.
            (
                "fmtnibbles",
                "            let nibbles = r.u16()?;",
                "            let nibbles = 0;\n            let _ = r.u16()?;",
                "KILL",
                "crates/testrunner/src/okifmt.rs",
            ),
            # The per-sample record two bytes short in the bounds check, so a truncated
            # file is read past its end rather than reported.
            (
                "fmtcasesize",
                "let need = writes_len * 3 + rom_len + samples_len * 8;",
                "let need = writes_len * 3 + rom_len + samples_len * 6;",
                "KILL",
                "crates/testrunner/src/okifmt.rs",
            ),
            # The runner's nibble check deleted. The suite still passes 1,000/1,000 on a
            # correct core, which is precisely why the corruption test has a Nibbles arm.
            (
                "runnernibbles",
                """        if got_nibbles != want.nibbles {
            return fail(
                Field::Nibbles,
                i64::from(want.nibbles),
                i64::from(got_nibbles),
            );
        }
        if got_mono != want.mono_2x {
            return fail(Field::Mono, i64::from(want.mono_2x), i64::from(got_mono));
        }
""",
                """        if got_mono != want.mono_2x {
            return fail(Field::Mono, i64::from(want.mono_2x), i64::from(got_mono));
        }
""",
                "KILL",
                "crates/testrunner/src/okirunner.rs",
            ),
            # The two checks swapped, so a divergence in both fields is reported as Mono
            # rather than as Nibbles. **Declared SURVIVE and it KILLed**, and the verdict
            # is the finding: the plan's reasoning was that nothing behavioural changes,
            # because no case fails that did not fail before. That reasoning is wrong
            # here. A wrong nibble decodes to a wrong sample, so in practice *every*
            # nibble divergence is also a mono divergence, and the order is what decides
            # which of the two a report names -- an address-walk bug or a decoder bug,
            # which are different files. `the_nibbles_are_compared_before_the_sample`
            # asserts that priority deliberately and says why, so it is not a test
            # over-specifying the report; it is the report's contract. Reclassified to
            # KILL rather than the test being loosened.
            (
                "runnerorder",
                """        if got_nibbles != want.nibbles {
            return fail(
                Field::Nibbles,
                i64::from(want.nibbles),
                i64::from(got_nibbles),
            );
        }
        if got_mono != want.mono_2x {
            return fail(Field::Mono, i64::from(want.mono_2x), i64::from(got_mono));
        }
""",
                """        if got_mono != want.mono_2x {
            return fail(Field::Mono, i64::from(want.mono_2x), i64::from(got_mono));
        }
        if got_nibbles != want.nibbles {
            return fail(
                Field::Nibbles,
                i64::from(want.nibbles),
                i64::from(got_nibbles),
            );
        }
""",
                "KILL",
                "crates/testrunner/src/okirunner.rs",
            ),
            # --- The host side: the loop, the resampler, the ring (`sfemu`, `machine`) ---
            # Every frame's samples queued twice, which is a doubled rate and a ring that
            # overflows on every tick. A test asserting only "something was queued"
            # cannot see it.
            (
                "queueonce",
                """            if let Err(e) = audio.queue(&samples) {
                note(&mut summary, format!("audio: {e}"));
            }
""",
                """            if let Err(e) = audio.queue(&samples) {
                note(&mut summary, format!("audio: {e}"));
            }
            let _ = audio.queue(&samples);
""",
                "KILL",
                "crates/sfemu/src/loop_.rs",
            ),
            # The YM's own divider dropped from the ratio, so the resampler thinks the
            # board runs at 8 MHz and resamples by 167:1.
            (
                "resratio",
                "(SOUND_XTAL, self.host_rate * YM_SAMPLE_CLOCKS)",
                "(SOUND_XTAL, self.host_rate)",
                "KILL",
                "crates/machine/src/resample.rs",
            ),
            # The fractional phase reset per input sample rather than carried, so every
            # frame boundary is a seam.
            (
                "resphase",
                "self.pos -= den;",
                "self.pos = 0;",
                "KILL",
                "crates/machine/src/resample.rs",
            ),
            # Sample-and-hold instead of linear interpolation. **Read this verdict
            # individually**: nearest-below is still monotonic on a ramp and still
            # constant on a constant, so a SURVIVE here means the resampler's
            # interpolation is untested and the killer is missing -- not that the mutant
            # is equivalent.
            (
                "resinterp",
                "out.push(((a * (d - t) + b * t) / d) as i16);",
                "out.push(a as i16);",
                "KILL",
                "crates/machine/src/resample.rs",
            ),
            # The ring halved, below the measured depth swing, so the recorded cadences
            # start dropping.
            (
                "ringcap",
                "pub const RING_MS: u32 = 100;",
                "pub const RING_MS: u32 = 50;",
                "KILL",
                "crates/machine/src/resample.rs",
            ),
            # The prefill removed, so the device's first callback runs on a nearly-empty
            # ring and underruns immediately.
            (
                "ringprefill",
                "pub const PREFILL_MS: u32 = 50;",
                "pub const PREFILL_MS: u32 = 0;",
                "KILL",
                "crates/machine/src/resample.rs",
            ),
            # The armed flag never set, so the ring prefills forever and outputs silence
            # for the whole session.
            (
                "ringarm",
                "            self.armed = true;",
                "            self.armed = false;",
                "KILL",
                "crates/machine/src/resample.rs",
            ),
            # The flag made un-sticky: the ring re-prefills whenever it empties, which is
            # 50 ms of silence after every stutter rather than one held sample. This is
            # the failure mode the sticky flag exists for, so it needs its own mutant
            # rather than only the initialiser's.
            (
                "ringrearm",
                """                self.stats.underruns = self.stats.underruns.saturating_add(1);
            }
        }
    }
""",
                """                self.stats.underruns = self.stats.underruns.saturating_add(1);
            }
        }
        if self.buf.is_empty() {
            self.armed = false;
        }
    }
""",
                "KILL",
                "crates/machine/src/resample.rs",
            ),
            # Overflow dropping the newest sample instead of the oldest: the audio the
            # player is about to hear is thrown away in favour of audio they should
            # already have heard.
            (
                "ringdropnewest",
                "                self.buf.pop_front();",
                "                self.buf.pop_back();",
                "KILL",
                "crates/machine/src/resample.rs",
            ),
            # The bound removed entirely, so the ring grows and latency rises for as long
            # as the emulator runs ahead. No click, and unbounded delay.
            (
                "ringdropgrow",
                """            if self.buf.len() >= self.capacity {
                self.buf.pop_front();
                self.stats.drops = self.stats.drops.saturating_add(1);
            }
""",
                "",
                "KILL",
                "crates/machine/src/resample.rs",
            ),
            # An underrun zeroed rather than held, which is a click where the policy says
            # a DC excursion the device filters out.
            (
                "ringholdzero",
                "*slot = self.last;",
                "*slot = 0;",
                "KILL",
                "crates/machine/src/resample.rs",
            ),
            # The paused arm deleted, so a paused emulator accrues an underrun per sample
            # and the counter stops meaning "your machine cannot keep up".
            (
                "ringpaused",
                """            } else if paused {
                *slot = 0;
            } else {
""",
                "            } else {\n",
                "KILL",
                "crates/machine/src/resample.rs",
            ),
            # --- The panel and the save state (`frontend`) ---
            # The sound panel's header one row short of what it draws.
            (
                "sndheadrows",
                "const SND_HEAD_ROWS: usize = 11;",
                "const SND_HEAD_ROWS: usize = 10;",
                "KILL",
                "crates/frontend/src/overlay.rs",
            ),
            # The CLP/DRP/UND row deleted, so the panel cannot answer "why does it sound
            # wrong" and 11 rows no longer describes what is drawn.
            (
                "overlaystats",
                """    line(
        buf,
        &mut row,
        &format!(
            "CLP {:06} DRP {:06} UND {:06}",
            m.sound_trace().oki_clamps,
            m.sound_trace().audio_drops,
            m.sound_trace().audio_underruns
        ),
        FG,
    );
""",
                "",
                "KILL",
                "crates/frontend/src/overlay.rs",
            ),
            # The save-state version left behind after the payload grew, so an old state
            # loads into a struct that no longer matches it.
            (
                "stateversion",
                "pub const VERSION: u8 = 3;",
                "pub const VERSION: u8 = 2;",
                "KILL",
                "crates/frontend/src/state.rs",
            ),
            # CONTROL, and the load-bearing one: a doc comment on a private constant,
            # wrong about the address bus width. Nothing compiles differently and no test
            # reads the prose, so it must SURVIVE -- a pass where everything dies is more
            # likely a broken harness than a thorough suite.
            #
            # Checked for overlap with every other mutant in this set: no other row edits
            # this line, and `ADDRESS_MASK` itself is untouched, so a KILL here would mean
            # the harness is scoring something other than what it applied.
            (
                "CONTROL-okidoc",
                "/// The sample ROM address bus is 18 bits",
                "/// The sample ROM address bus is 19 bits",
                "SURVIVE",
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
