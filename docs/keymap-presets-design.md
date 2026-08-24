# The key menu: presets, not rebinding

A short design note, written before the code, for the one decision in this feature
that is not obvious and the three that are traps.

## What was asked for

A menu the player can open to "reset keys", plus an architecture document and a user
guide. Offered three scopes — presets, rebind-within-the-known-44, and full
physical-position rebinding — the answer was **presets and restore-defaults**.

That is the right call, and not only on cost. Per-key rebinding on this program would
have to solve a problem it does not have: `frontend::Key` is layout-blind by
construction, so "press the key you want" cannot name a key the map does not already
reach. Presets sidestep that entirely by shipping *complete, verified* maps rather than
letting the player assemble one that may not be reachable.

## The one real discovery: the stick needs no preset

The obvious four-way matrix — {AZERTY, QWERTY} × {punches low, punches high} — collapses
to two. **AZERTY's `Z S Q D` and QWERTY's `W A S D` are the same four physical keys.**
`minifb` names hardware positions, so the diamond is one map that reads correctly on
both layouts; only the *printed letters* differ. A "QWERTY stick" preset would be a
preset that changes nothing.

So a preset varies exactly two things:

- **which row punches** — the requested punches-low, or a cabinet's punches-high
- **which three letters the punch/kick rows use** — `K L M` (AZERTY's home-row run) or
  `J K L` (QWERTY's home-row run)

Two axes, four presets, and every one of them a map that has been asserted against the
board's ports.

## The cost this exposes: `Key::J` has to come back

The K L M remap deleted `Key::J`. A QWERTY preset needs it, because QWERTY's home-row
run of three is `J K L` — AZERTY's is `K L M`, since AZERTY moves `M` onto the home row
and pushes `;` off it. So:

- `Key` grows a 45th variant, `J`, taking the next free bit (44)
- `GAME_KEY_PORTS` grows to 26 rows
- `display::translate` maps `M::J` again, and `M::Semicolon` stays mapped

`KeySet` is a `u64` and 45 keys fit with 19 bits spare, so no widening. The existing
bits do not move — `Key::M` keeps 7 — which is what keeps
`mutate.py`'s `CONTROL-escape-moves-to-another-free-bit` parked on 62 and valid.

**A consequence worth writing down:** the presets no longer differ only in behaviour,
they differ in *which keys are live*. Under an AZERTY preset, `J` presses nothing; under
a QWERTY preset, `M` (i.e. `minifb`'s `Semicolon`) presses nothing. That is correct — one
physical key, one board input — but it means "key X does nothing" is now
preset-dependent, and a test has to pin it per preset rather than globally.

## Three traps, all of them collisions

The mockup that was approved binds `Enter` to apply and `F1` to close. Both are taken,
and the third collision is worse than either:

| key | already does | if the menu took it |
|---|---|---|
| `Enter` | graphics viewer: act on the view | menu apply would also cycle a tile layout |
| `F1` | debugger overlay on/off | closing the menu would open the debugger |
| `Escape` | **quits the emulator** | the instinctive "back out" ends the session |

`F1`–`F12` are *all* mapped — twelve of twelve, verified by scanning
`display::translate`. There is no free function key.

**The resolution: the menu captures the keyboard while it is open.** This is the design
decision the feature turns on, and it fixes all three collisions at once:

- **`Tab` opens and closes it.** Position 0x30, unmapped today, and identical on AZERTY
  and QWERTY — so the one key that reaches this menu is not itself a layout question.
- While the menu is up, arrows move the selection, `Enter` applies, `Escape` closes
  **without quitting**, and every other key does nothing.
- **Game inputs read idle while it is up.** Navigating a menu must not throw punches,
  and a stick held when the menu opened must not stay held in the board's eyes.

That last point is the one with a real failure mode: `Inputs` is level-triggered, so
without an explicit idle the board would keep seeing whatever was held at the moment the
menu opened, for as long as the menu stayed open.

## Where the state lives, and where it must not

The map is chosen in `frontend`, which has no filesystem, no clock and no window. So:

- **`frontend::keys`** owns the preset table and which preset is active. It is still
  layout-blind: a preset is a set of `Key` variants, and no code here knows what letter
  any of them prints.
- **`frontend::menu`** (new) owns the menu's state machine — open/closed, which row is
  selected, what the panel says. Testable with no window, like every other panel.
- **`sfemu`** owns persistence, because it is the only crate allowed to touch a disk,
  and `display.rs` stays the only place that names `minifb`.

Persistence goes beside the save state, on the same rule already in force: `sf2.zip` →
`sf2.keys`. A missing or unreadable file is the default preset and not an error, exactly
as a missing save state is.

## What is deliberately not in this

- **No per-key rebinding.** Chosen scope. If it is wanted later, the honest version
  requires `Key` to become a physical-position type — see the rejected option above.
- **No mouse.** There is no pointer anywhere in this program.
- **No preset for the controls** (F-keys, coins, starts). The presets cover the twelve
  player buttons and nothing else; a preset that moved `Escape` could strand the player
  in a window they cannot close.
