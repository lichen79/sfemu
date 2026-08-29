# Putting a playable video in the README

The README shows `docs/sfemu-demo.gif`, which plays inline on GitHub but has no
sound. GitHub *will* render a real video player with audio controls in a README —
but only for one URL form, and that form cannot be produced from a script. This is
the manual procedure, and it takes about two minutes.

## Why it has to be manual

Measured on 2026-08-29 against `lichen79/sfemu`, by committing each form to a
throwaway branch and fetching the result back:

| Form | `/markdown` API | `contents` API, HTML media type | live repo page |
| --- | --- | --- | --- |
| `<video src="…mp4" controls>` | empty `<p>` | stripped | zero `<video>` elements |
| `<video><source src="…mp4"></video>` | stripped | stripped | zero `<video>` elements |
| `![alt](…mp4)` | `<img src="…mp4">` | `<img src="…mp4">` | `<img src="…mp4">` |
| bare release-asset URL | `<a>` | `<a>` | `<a>` |

The `<img>` case is worth naming precisely, because it looks like it might work:
the sanitizer keeps the tag, so you get an `<img>` element whose source is an
H.264 file. Browsers render that as a broken image, not a player.

GitHub's inline player is injected client-side, and it keys on the
`github.com/user-attachments/assets/<uuid>` host. Those URLs are minted by the
upload flow behind the web UI's comment box. `POST /upload/policies/assets`
answers a personal access token with an error page rather than a policy — it wants
a browser session — and there is no REST endpoint for it, so neither `gh` nor a
script in this repository can create one.

Hence: a human with a browser, once, by hand.

## The procedure

1. Get the clip. It is a release asset rather than a tracked file — `/docs/*.mp4`
   is gitignored — so download it:

   ```bash
   gh release download v0.1.0 -p sfemu-github.mp4 -D /tmp
   ```

2. Open any comment box on the repository — a new issue at
   `https://github.com/lichen79/sfemu/issues/new` is the usual choice. **Do not
   submit it.** The upload happens on drop, before the comment exists.

3. Drag `/tmp/sfemu-github.mp4` into the box. GitHub uploads it and replaces the
   drop with a bare URL of the form:

   ```
   https://github.com/user-attachments/assets/xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
   ```

4. Copy that URL. Close the issue form without submitting — the asset survives the
   discarded draft, which is what makes this work at all.

5. In `README.md`, put the URL on a line of its own in the `## It runs` section,
   above the GIF. Bare, with no markdown syntax around it: link syntax and image
   syntax both defeat the player.

   ```markdown
   ## It runs

   https://github.com/user-attachments/assets/xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
   ```

6. Commit and push, then load the repository page and confirm a player with a
   volume control appears. If you see a plain link instead, the URL is not on its
   own line or it is wrapped in markdown.

Keep the GIF underneath it either way. The player does not render in every
context that shows a README — package registries, mirrors, and offline clones all
fall back — and the GIF does.

## What this file guards

`the_readme_video_procedure_matches_the_readme` in `crates/sfemu/src/main.rs`
asserts the README links here and that this file still names the `user-attachments`
host. If GitHub ever ships an API for minting these URLs, or starts allowing
`<video>` through the sanitizer, this procedure becomes obsolete — re-run the table
above before trusting it.
