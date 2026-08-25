# How sfemu was built

A record of the process that produced this repository: what was asked for, what was
built in what order, which methods worked, and — at more length than is comfortable —
which of my own claims turned out to be false and how each one was caught.

This is a **process** document. For what the program *is*, read
[architecture.md](architecture.md); for how to run it, [user-guide.md](user-guide.md).
Where this page and either of those disagree, they are right and this page is stale.

Every figure below was measured from the repository on 2026-08-24, not recalled. The
commands are in [Checking this page](#checking-this-page).

⚠️ **No ROM is in this repository and none was ever fetched by me.** That constraint
shaped the project and has its own section: [The ROM question](#the-rom-question).

---

## What it is, in one paragraph

An arcade hardware emulator in Rust: a cycle-counted M68000, a Z80, CPS-1 graphics, a
YM2151 FM synthesizer, an OKI MSM6295 ADPCM chip, and a second, entirely different
1987 board for Street Fighter 1. It runs Street Fighter II and Champion Edition from
ROM sets the user supplies, in a window, with sound, save states, a debugger and four
graphics viewers. Eleven crates, 91,742 lines of source, 1,751 unit tests.

## The shape of the work

| | |
|---|---|
| First commit | 2026-08-05 |
| Last commit | 2026-08-24 |
| Calendar span | 20 days, 15 of them with commits |
| Commits | 330 |
| Lines added / removed | 172,979 / 6,189 |
| Source lines (`crates/*/src`) | 91,742 |
| Unit tests | 1,751, plus 7 ROM-gated integration tests |
| Specs / plans written first | 10 / 10, totalling 60,603 lines and 121 numbered tasks |
| Hardware notes | 3,274 lines in `docs/hardware/` |
| Mutants | 299 in 21 sets |

Commit subjects by type: 131 `feat`, 95 `docs`, 47 `test`, 26 `fix`, 8 `refactor`, 19
numbered finding-fixes (`F2`, `F7`, `F11`…), 2 merges, and a handful of early commits
written before the convention settled.

**Documentation is 40% of the commits.** That is not diligence for its own sake. The
specs and plans came *before* the code, so most `docs` commits are design work, and a
second large group are corrections to documents that had gone stale — which is a
recurring theme below.

---

## The order things were built, and why that order

The project was decomposed into sub-projects up front, each with its own spec, plan,
and review gate. The sequence, with the commit index each finished at:

| | Sub-project | Finished | Commit # | The reason it sat here |
|---|---|---|---|---|
| **A** | Workspace and M68000 core | 08-07 | 103 | Nothing else can be tested until a CPU executes. Validated against 317,500 vector cases before anything else existed. |
| **B** | Bus, timing, MAME ROM-set loader | 08-07 | ~120 | First execution of real board code — the first moment the project was more than a CPU simulator. |
| **C** | CPS-1 video | 08-08 | ~140 | The largest single piece, and where SF2 becomes *visible*. |
| **E1** | Window, frame clock, keyboard, save states | 08-08 | 147 | Came before sound because it is what makes the thing a program you can use. |
| **E2** | Debugger | 08-08 | 160 | +13 commits on E1: the panels are cheap once there is a window. |
| **E3** | Graphics viewers | 08-08 | 172 | +12 more. Deliberately last of E — a tile browser's value is mostly in *stopping* the machine at the frame you care about, which is E2's stepping. So E3 waited for E2, then took it. |
| **D1** | Z80 audio CPU | 08-09 | 196 | Silent by design: a CPU with no chip attached. 1,604,000 vector cases. |
| **D2** | YM2151 FM + sound-board wiring | 08-11 | 218 | Still silent: samples that reach no speaker. |
| **D3** | OKI ADPCM, mixing, host audio | 08-16 | 248 | The one that ended "there is no sound." |
| **F** | Street Fighter 1 driver | 08-20 | 298 | Last, because it is the test of whether the abstractions held. |
| — | `--demo`, sf2eb, CE, the key menu, these docs | 08-24 | 330 | Work driven by actually running it. |

**Two decompositions are worth defending, because both were arguments about review
attention rather than about code.**

*E was split three ways* because only E1 changes what the project **is**; E2 and E3
change what can be *inspected* about it. Different risk, different review.

*D was split three ways and the number that settled it is 1,604* — the Z80 vector
suite's file count, against the 68000's 127. A Z80 core, an FM synthesizer and a host
audio path are three unrelated subsystems, and asking one review pass to gate all
three is exactly what a decomposition is for.

That split paid off in a measurable way, and it is the best evidence on this page that
the method did real work. D2's vector suite includes at least 100 cases that enable
CSM with timer A running. That requirement exists because the YM2151's `prepare()`
gate is **invisible without them**: eager and lazy preparation agree bit-for-bit over
40,000 samples with CSM off. A suite lacking those cases passes at 1,000/1,000 on a
chip that is wrong. The mutation harness measures precisely that — forcing the gate
eager dies to the suite *and* to the unit tests, but with the CSM cases skipped it
**survives the suite entirely**. A review pass covering the Z80, the FM chip and the
audio path at once would not have had the attention to spend on one gate in one of
them.

**F was the abstraction's exam, and it passed.** SF1 is a different board: three CPUs,
ROM-resident tile maps, two MSM5205s in stereo, a second sound Z80. Adding it changed
`m68k`, `z80` and `ym2151` **not at all**, and `video` and `machine` each gained an
`sf1` module *beside* the CPS-1 one rather than growing a board flag through existing
code. That is the outcome the earlier sub-projects' constraints were chosen to buy.

**Where the effort actually went**, by commits touching each crate: `m68k` 75,
`testrunner` 51, `machine` 49, `sfemu` 44, `frontend` 38, `video` 26, `z80` 12,
`romset` 9, `ym2151` 7, `testrom` 6, `oki` 4.

Two of those numbers say something. **`testrunner` at 51 is the second-highest** — the
vector harness took nearly as much work as the machine it validates, which is what
"validated against 1.9 million cases" actually costs. And **`z80` at 12 against
`m68k`'s 75** is the compounding: by the time the Z80 was written, the bus trait, the
test harness, the prefetch discipline and the tracing convention all already existed.

---

## The method

Each sub-project ran spec → plan → task-by-task implementation → review, with a fresh
agent per task and a review gate between tasks. The parts that earned their cost:

**Specs and plans before code, with exact values in the plan.** 121 numbered tasks
across 10 plans. A task whose plan text contains the literal expected values is a task
whose implementer cannot quietly invent a different answer — and, more importantly, a
task whose *review* has something to check against.

**A fresh agent per task, with a written brief instead of inherited context.** Context
that carries forward carries mistakes forward with it. The failure this avoids is
specific: an implementer who has read the last five tasks' summaries tends to reproduce
their assumptions rather than check them.

**Review as a gate, not a formality.** 19 commits are numbered finding-fixes, which
means 19 defects that review caught and the implementer had not. Two merges exist
specifically to carry review residuals: `Merge Task-10 residuals: six unactioned
review findings, measured`.

**Vector suites as ground truth.** 317,500 M68000 cases over 127 groups; 1,604,000 Z80
cases over 1,604 files; 1,000 YM2151 cases against ymfm; 1,000 ADPCM cases against
MAME's decoder. Per case, every register, every bus transaction and the cycle count are
compared. **Debugging is done from suite diffs, not from standalone analysis** — a
harness written to study a bug tends to encode the misunderstanding that caused it.

**Mutation testing as the check on the tests.** 299 mutants in 21 sets. This is the
single highest-value practice in the project and it deserves its own section.

**Documenting hardware separately from code.** 3,274 lines in `docs/hardware/`, each
claim cited to the MAME source line or the datasheet page it came from, because "the
chip does X" with no citation is indistinguishable from "I assumed X".

---

## Mutation testing, and what it kept finding

A green suite proves the tests ran. It does not prove they can fail. So each set of
mutants is a set of deliberate, single-string breakages of the source, each with a
**declared** expected verdict — `KILL` or `SURVIVE` — and a kill records *which test*
noticed.

Current state: **299 mutants in 21 sets, 273 killed, 26 declared survivors** (23
controls plus 3 proven equivalents).

The rules, each of which exists because it was violated once:

- **Every set carries a control that must survive.** Otherwise a harness that reports
  success without running anything is indistinguishable from a clean pass.
- **`NO-OP` is a distinct verdict** from `KILL`/`SURVIVE`/`NO-BUILD`. A pattern that no
  longer matches the source scores `NO-OP` — silently scoring it a kill would let the
  whole set rot invisibly.
- **A non-compiling mutant scores as a kill**, which is a trap: it means the test
  written for that exact case may never have been exercised. This happened.
- **A timeout scores as a kill**, so the timeout must exceed the rebuild time or a slow
  build fakes a clean sweep.
- **The pass owns the tree.** Commit before mutating. A `cargo` run alongside a
  mutation pass fakes kills, and a killed pass strands live mutated code in tracked
  source — `SIGTERM` skips every `finally`.
- **Probe the mutant before writing the killer.** Two killer tests written from
  plausible guesses both *passed under their mutants*.
- **Write the killer before the mutant** where you can: choosing mutants is how you
  find the assertions that cannot fail.

What it actually caught, as classes of defect that a green suite had hidden:

- **Tests that assert the flag the code sets** rather than the artifact the user gets —
  these pass a half-done fix.
- **A zero observable cannot kill a zero mutant.** One killer asserted the honest
  answer for an unrendered frame, which is also what the mutant returns.
- **A substring assertion passes on the wrong name.** `contains("sf2")` is satisfied by
  `"sf2eb"`, so both tests covering that gap passed.
- **Controls that had rotted into real mutants** when a later task claimed the bit they
  moved to — which is why every set must be run, not one.
- **A hidden layer is invisible to every counter.** The demo drew one grid instead of
  two and every headless number matched a correct run.

---

## Every false claim I made, and how it was caught

This is the most useful section on the page, and the reason it exists is that in
several of these cases the claim was *plausible*, *confidently written*, and would
have been believed.

**"`#![forbid(unsafe_code)]` is in every crate."** Written into
`docs/architecture.md`. Measured: **9 of 11**. `sfemu` and `testrunner` do not carry
it — and `sfemu` is precisely where the two FFI-shaped dependencies (minifb, cpal)
live, so it is the crate where the gap matters most. Caught by counting before
publishing.

Fixed 2026-08-25 — and fixing it found the claim was wronger than the correction.
"9 of 11" counts *directories*, but the attribute is per **crate root**, and a workspace
has more of those than it has crates: each of the 8 files in `testrunner/src/bin/`, the
15 integration tests under `tests/`, the bench and the example is its own crate root that
`lib.rs`'s attribute does not reach. The real figure was **19 of 36**. All 36 carry it
now, and `every_crate_root_in_the_workspace_forbids_unsafe_code` enumerates the roots and
reads the attribute out of each, so the count cannot drift again — which is the part that
was missing the first time: the original evidence for the claim was that no crate
contained the word `unsafe`, a measurement rather than a rule.

**"Two runs of the 2–3% dropped-frame figure."** Both new docs recorded 2–3% dropped
frames as the known observation. The user's own session on 2026-08-24 reported
**18,656 frames, 3,246 dropped — 17.4%**, an order of magnitude worse. Caught by the
user simply running the program. The emulator is not the cause: 1,200 frames headless
took 1.08 s, i.e. **0.90 ms/frame, an 18.6× real-time margin**, so the drops are
host-side stalls longer than the pacer's 67 ms catch-up window, not slow emulation.
The figure is corrected and the cause is still unknown.

**"The README's mutant totals."** The README said 262 mutants in 19 sets, 240 killed.
Actual: **299 in 21, 273 killed**. Caught by executing `scripts/mutate.py` and counting
rather than reading the prose. The drift was the key menu — `keys` went 21→31 and
`menu` was new at 16.

**"299 mutants, 273 killed" — the count was right and the claim was still false.**
Fixed 2026-08-25. Counting the rows in `SETS` is not the same as checking they still
match the source, and **43 of the 299 (14%) matched zero or two times**, which the
harness scores `NO-OP`: nothing applied, nothing measured, and a row in the table that
reads exactly like a kill. Two causes. A rename — `draw`'s `m: &Machine` became
`v: &CpuView`, `SND_HEAD_ROWS` moved to `sndpanel::CPS1_HEAD_ROWS`, `*slot = x` became
`slot.fill(x)` when the audio ring went stereo. And **a second board**: SF1 gave
`pixels.rs`, `state.rs`, `gfx.rs` and `loop_.rs` a parallel path with the same field
names, so patterns that had been unique started matching *twice* — the `NO-OP` nobody
looks for, because the code the pattern names is still sitting right there. Every one
is repaired, and re-running the six affected sets found two more that the audit could
not see: mutants whose `old` matched but whose *replacement* still said `m.`, so they
had been reporting `NO-BUILD` — the verdict that exists to catch exactly that, and
which nobody had read either. The check is one command, `--all`, and its own docstring
says why it must be that: "a set that has started reporting NO-OP because the code it
mutates was reworded is invisible when you only run the set you are working on."

**"Six tests are the documented exception."** Actual: **seven**. `boot.rs` had gained
CE's attract-mode test after that sentence was written, so the prose describing the
one-variable ROM-gating rule failed to name a test the rule covered.

**Two false claims about what a wrong CPS-B row does** (`009bced`, its own fix commit).
I had asserted a wrong row produces a specific visible failure. Measured: `sf2eb`
under sf2's row boots to an **idle loop with no unmapped access at all**, and `sf2ce`
under the wrong row **draws** — 184 distinct pens against 123 at 1,100 frames. Both
docs now state the measured behaviour, which is far less convenient than what I first
wrote.

**"`cpsb_value` of -1 means there is no ID register."** It means **`0xFFFF`**
(`49abe82`). A sentinel read as a semantic absence.

**A silence I explained with a story.** Sound was missing in the window and I had a
plausible account involving attract mode. The real cause was a **DIP switch default**
(`a43de80`) — and the story I'd told pointed at a workaround that would have *weakened*
the real assertion.

**A key map written by letter.** `minifb::Key` names a **hardware position**, not a
letter. P1's stick, mapped by letter, put it on the wrong physical keys for the user's
AZERTY board (`5cd38c6`). No layout-blind test could see this; it took the user
pressing the keys. The fix is verified against Carbon's `UCKeyTranslate`, and the
finding generalized: `M::W => Key::Z`, `M::A => Key::Q`, and the trap that
`M::Semicolon` is the physical `M` on a French board.

**A test whose expectation was computed** (`0xFFFE ^ 0x0010`) rather than written as a
literal, so it could agree with a wrong implementation. Now a project-wide rule: every
expected value in a test is a literal.

**Two over-general figures in my own drafts**, tightened before publishing. "14.2%
slow" is specific to a **48 kHz** device (48,000 / 55,930.390625 = 0.858), not a
general property; and the clock-drift claim of +6.3 ppm was **below the measurement
method's own resolution** (± 59.6 ppm), so the operative bound is the ~60 ppm jitter,
not the drift.

**An invented example.** The user guide's headless report block was illustrative. I
replaced it with the actual output of `cargo run -p sfemu --release -- --demo 600`,
which is reproducible with no ROM set — and which required explaining why `acks 599`
sits against `vblanks 600` (the last frame's interrupt is still pending, not missed).

**A tidy property that was false.** I asserted two ROM sets have disjoint file names.
They do not. The weaker true claim was sufficient for the test's purpose.

**`bits.rs`'s comment claiming `Plan::writes` was dead code** (`F10`) — false, and the
kind of comment that gets believed and then acted on.

### What the pattern is

Read as a group, these are not random errors. Almost all are the same failure:
**stating a property I had reasoned to, in the register of a property I had measured.**
The wrong CPS-B row, the `forbid(unsafe_code)` count, the drop rate, the mutant totals,
the disjoint file names, the DIP-switch story — in each case a plausible mechanism
substituted for a measurement, and the prose gave no hint of the difference.

Three defenses worked, and they are the transferable part of this project:

1. **Measure before publishing, including your own drafts.** The `forbid` overclaim,
   the mutant totals and the stale ignore count were all caught this way — in the same
   session that wrote them.
2. **Assert the artifact, not the internal flag; the literal, not the computation.** A
   test that reads the same value the code writes cannot fail.
3. **Run the program.** The AZERTY map, the missing sound and the 17.4% drop rate were
   all found by a human at a window, and none of them could have been found any other
   way. This is why the README carries a list of things **only a human can check** —
   currently eight items.

---

## The ROM question

Across the project the user asked four times for ROM sets. I declined three times: to
search for one, to download one, and to unpack an archive from a third-party site. Age
does not release copyright — SF2 is 1991 and Capcom still sells both games.

On the fourth, the user stated they were entitled to a file **already on their own
disk** and asked me to use it. I proceeded, having said plainly that the site it came
from is not Capcom's so I could not verify the claim. Whether the user is entitled to a
file on their own machine is the user's call, and running a local file is a different
act from fetching one.

What that left as permanent constraints, all of them still enforced:

- **No ROM or test data is ever committed or bundled.** Extraction goes outside the
  repo; `testdata/` is gitignored (it holds 616 MB of fetched vectors).
- **No URL to any ROM appears anywhere in the repo** — enforced by a test that asserts
  the usage text contains no `http`.
- `romset` tables hold **file names, offsets, lengths and CRC-32s only**.
- The 7 real-ROM tests are each `#[ignore]`d and gated on **one** variable,
  `SFEMU_ROMS`. A second variable is **forbidden by name** in two source comments, so
  the rule cannot be quietly widened.
- The only network fetches I made were **MAME's BSD-3 driver sources**, for citations.

`--demo` exists because of this: a CPS-1 image the program generates itself —
scrolling tilemaps, a sprite on a path, a frame counter and FM music, built from
nothing by `testrom`. It is how the graphics and sound paths can be demonstrated,
and these documents' examples reproduced, with no ROM set at all.

**SF1 was dropped as a target** late on. Its driver, the three gated tests and the
board support all remain in the tree, complete and unexercised against real ROMs; the
third game is Champion Edition instead. The docs say so explicitly, because a code
path that has never run against real data and is not marked as such is worse than one
that does not exist.

---

## What this process did not achieve

- **Two open questions are recorded and uninvestigated**: the dropped-frame cause and
  drift in `mutate.py`'s older patterns. They are written down in
  [architecture.md](architecture.md#open-questions) because an unrecorded open question
  becomes a wrong assumption. A third — CE's clock — was on that list until 2026-08-25,
  when it turned out to be a real bug rather than a question: MAME's `cps1_12MHz`
  overrides the 68000's clock and nothing else, so CE had been running at 83.3% speed.
  Recording it is what made it fixable later; the honest note is that it sat there for
  two days while nobody could see it, because a game running 17% slow looks like a game.
- **A green suite is not a tested codebase.** All 1,604,000 Z80 cases would pass on a
  core with no interrupt support at all — the vectors do not cover it. That sentence is
  in the README and it is the honest limit of the validation.
- **Eight things only a human can check** remain unchecked by me, by construction.
- **The `#![forbid(unsafe_code)]` gap was named for a day before it was fixed.** It is
  closed now, across all 36 crate roots and with a test behind it, but the version of
  this page published on 2026-08-24 said "named, not fixed" — which is the right thing to
  write down and still a gap that shipped.

---

## Checking this page

Every figure above is reproducible. Run these from the repository root.

```sh
# Commits, span, and subject types
git rev-list --count HEAD
git log --format='%ad' --date=short | sort | uniq -c
git log --format='%s' | sed 's/[(:].*//' | sort | uniq -c | sort -rn

# Lines added and removed over the whole history
git log --numstat --format='' | awk '{a+=$1;d+=$2} END {print a" added, "d" removed"}'

# Source lines and unit tests
find crates/*/src -name '*.rs' -exec cat {} + | wc -l
grep -rh '#\[test\]' crates --include='*.rs' | wc -l

# Commits touching each crate
for c in m68k z80 video machine ym2151 oki romset frontend testrom testrunner sfemu; do
  printf '%-11s %s\n' "$c" "$(git log --oneline -- crates/$c | wc -l)"
done

# Specs, plans, and numbered tasks
ls docs/superpowers/specs/*.md docs/superpowers/plans/*.md | wc -l
grep -h '^#\+ Task' docs/superpowers/plans/*.md | wc -l
```

The mutant totals come from the harness itself rather than from any prose:

```sh
python3 - <<'EOF'
import importlib.util
s = importlib.util.spec_from_file_location('m', 'scripts/mutate.py')
m = importlib.util.module_from_spec(s)
try: s.loader.exec_module(m)
except SystemExit: pass
print(len(m.SETS), 'sets', sum(len(v[1]) for v in m.SETS.values()), 'mutants')
EOF
```

And the speed claim, which needs no ROM set to be *checked* but does need one to be
reproduced exactly:

```sh
cargo build -p sfemu --release
/usr/bin/time -p ./target/release/sfemu "$SFEMU_ROMS" 1200 --game sf2eb
# 2026-08-24, this machine: real 1.08 s for 1200 frames = 0.90 ms/frame, 18.6x margin
```
