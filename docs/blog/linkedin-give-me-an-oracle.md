# Give me an oracle and I'll converge

*A LinkedIn post. The claims in it are the repository's, and every number is one I
measured — sources noted in square brackets, to be dropped before posting.*

---

> **"Give me an oracle and I'll converge. Where no oracle exists, I'll produce
> something fluent, self-consistent, fully tested — and wrong. Only contact with
> reality will tell you which."**
> — Claude Code, describing itself

I vibe-coded a Street Fighter II emulator to find out if that's true.

It is. And the hard part wasn't the 68000.

**Where there was an oracle, it converged.** The 68000 core is validated against a
public vector suite — 317,500 cases, and per case the final registers, both stack
pointers, the status register, both prefetch queue words, every touched RAM byte, the
cycle count, and the bus access sequence *in order* all have to match. 317,500 of
317,500 green. The Z80: 1,604,000 of 1,604,000, down to the two undocumented flag bits.
No public vector suite exists for the FM chip, so it generated 1,000 cases from the
BSD-3 implementation MAME itself uses and required every stereo sample to match
exactly — no tolerance, not a correlation. Same for the ADPCM chip. [README.md]

That is thousands of pages of hardware behaviour, converged on by grinding a diff
against a ground truth. It took days. It was never in doubt.

**Where there was no oracle, it was fluent and wrong.** Every single time.

It wrote in the architecture doc that a memory-safety attribute was in every crate.
Measured: 9 of 11. Then fixing *that* revealed the correction was also wrong — the
attribute is per *crate root*, and a workspace has more roots than directories. The
real figure was 19 of 36. [docs/history.md]

It documented the frame drop rate as "2–3%". My own session reported 17.4%.

It stated the CPU core "contains no third-party code." True — of that one core. And it
was the wrong scope to publish: several other files in the tree are transcribed from
MAME, each saying so in its own header. Nothing is vendored, so a file-level scan finds
nothing; you have to grep the code's *own prose* for "transcribed from".

It mapped the controls by letter. `minifb` key names are physical *positions*, so on my
AZERTY keyboard player one's stick was on the wrong keys. No layout-blind test could
see it.

**Then the sharpest one.** It built an instrument for the frame drops: a histogram of
frames owed per host tick, so one long stall could be told from many small ones. Tested.
Mutation-tested. Reasoned about in three documents.

It printed the histogram only when frames had actually dropped.

Last week a session finally printed one, and the finding wasn't in the drop rate at all:
2,031 host ticks served 5,999 frames in 100.6 seconds. **A mean tick of 49.5 ms against
a 16.768 ms frame — the loop was running at 20 Hz.** Only 1.7% dropped, because the
pacer's catch-up absorbs up to four frames and most ticks never crossed the threshold
that loses one. Every earlier figure — 2–3%, 17.4%, 4.7% — was the tail of a
distribution nobody had looked at.

A host equally slow but *steady* would have dropped nothing, and the report would have
said `dropped 0` and stopped. **That report would have certified the bug.** The gate was
the symptom the instrument existed to look past. It opened by luck: 69 of 2,031 ticks
happened to run long.

Fluent. Self-consistent. Fully tested. Wrong.

---

**What I'd tell anyone doing this seriously:**

**Find the oracle first.** Not the test suite — the oracle. A vector suite, a reference
implementation, a hardware trace, a spec with numbers in it. Where one exists, an agent
will close the gap with a patience no human sustains. Where one doesn't, tests are
*self-portraits*: they encode the same assumption as the code, and pass together.

**Distrust the instrument's silence.** "The diagnostic reported nothing" is not "there
is nothing to report." A trigger deserves the same scepticism as an output.

**Grep the prose, not just the code.** The claims that hurt are the ones stated
confidently in a doc, where nothing checks them. That's where a licence overclaim and
two wrong counts were hiding.

**Be the contact with reality.** The AZERTY keys, the silent audio, the real drop rate,
the 20 Hz loop — every one was found by a human running the program. Not one could have
been found any other way. The README carries an explicit list of eight things only a
person at a window can check, because a repository tested without a display can't check
them.

One more, found while writing this post: the caption burned into the demo video said
"1,880 tests" when the suite reported 1,882. A published claim about the repository that
nothing in the repository checked. There's a test for it now — and adding that test took
the count to 1,884, so the first literal I wrote was already stale by one.

The emulator runs. 1,884 tests, no unsafe code, 299 mutants each with a declared verdict.
Both sound chips verified sample-for-sample. MIT, and it ships no ROMs — you supply a set
you own.

The 68000 was the easy part. The hard part was every place I had to be the oracle myself.

🔗 github.com/lichen79/sfemu

#VibeCoding #Claude #ClaudeCode #Rust #Emulation #SoftwareEngineering #AI
