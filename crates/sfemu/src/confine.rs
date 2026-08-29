//! Where a host library is allowed to be named, as a source-tree scan.
//!
//! Test-only support, shared by [`crate::display`]'s `minifb` check and
//! [`crate::audio`]'s `cpal` check. It exists because those two tests were the same
//! sixty lines twice: two walks of `crates/`, two definitions of "prose", two ways to
//! spell the same assertion. A second copy of a rule drifts, and the drift is invisible
//! precisely here — a confinement test that stopped walking the tree would keep passing
//! for as long as nobody violated the boundary it no longer checks.
//!
//! # Asserting an absence
//!
//! A test that reads the source tree is unusual and justified for one reason: there is
//! no other way to assert a `use` is *not* somewhere. What the type system can express
//! is already expressed — `frontend` does not depend on `minifb` or `cpal`, so a `use`
//! there would not compile — but `sfemu` depends on both, and nothing stops a later
//! `use cpal::Stream` in `loop_.rs` from quietly moving a decision behind a boundary
//! that cannot be tested.
//!
//! # Comments are allowed to name the library
//!
//! `frontend::keys` and `frontend::pixels` both explain themselves by reference to
//! `minifb`, and the plan for the audio work does the same with `cpal`. "A
//! `minifb::Key` here would make this module part of the display boundary" is the
//! clearest statement of the rule in the project, and a check that forbade it would
//! delete the documentation to protect the constraint. So the scan looks at *code*
//! lines: a line whose first non-space characters are `//` is prose.
//!
//! The heuristic's limit, stated rather than hidden: a `/* */` block naming the library
//! is reported as code. That is a false positive — it fails loudly and is fixed by
//! rewording, and the failure these tests exist to catch cannot hide behind it.

use std::path::{Path, PathBuf};

/// Every place a library is named, split by whether a manifest or code names it.
#[derive(Debug, Default)]
pub struct Mentions {
    /// `path:line: text` for each code line naming the library, relative to `crates/`.
    pub code: Vec<String>,
    /// The manifests that name it, relative to `crates/`. A dependency edge, not a
    /// violation — which manifests are allowed is the caller's rule to state.
    pub manifests: Vec<PathBuf>,
    /// How many files were read. The tests assert this is large, because a walk that
    /// silently found nothing would report no offenders and pass.
    pub checked: usize,
}

/// Scan `crates/` for code mentions of `library`.
///
/// `exempt` names files whose every mention is allowed — the one file that owns the
/// boundary, plus anything like a lockfile that names every dependency by nature.
/// Matched on the file name alone, since these are unique within the tree.
///
/// This file is always exempt, and unconditionally: its own tests name `minifb` and a
/// deliberately absent library **as data**, on code lines, which every caller's scan
/// would otherwise report as a violation. That is the one file where a mention cannot be
/// a boundary breach, because there is nothing here to breach — the module holds no
/// device handle and does not depend on either library. Exempting it in the scan rather
/// than in each caller keeps the callers from having to know it exists.
///
/// # Panics
///
/// Panics if `crates/` or any file under it cannot be read: a scan that skipped an
/// unreadable file would be a confinement check with a hole in it.
pub fn mentions(library: &str, exempt: &[&str]) -> Mentions {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/sfemu has two ancestors")
        .join("crates");
    assert!(root.is_dir(), "the crates directory must exist: {root:?}");

    let mut found = Mentions::default();
    walk(&root, &mut |path| {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_manifest = name == "Cargo.toml";
        if !name.ends_with(".rs") && !is_manifest {
            return;
        }
        if exempt.contains(&name) || name == "confine.rs" {
            return;
        }
        found.checked += 1;
        let text = std::fs::read_to_string(path).expect("a source file this crate can read");
        let rel = path.strip_prefix(&root).unwrap_or(path).to_path_buf();
        for (n, line) in text.lines().enumerate() {
            if !line.contains(library) {
                continue;
            }
            if is_manifest {
                found.manifests.push(rel.clone());
                continue;
            }
            // Prose may name it; code may not.
            if line.trim_start().starts_with("//") {
                continue;
            }
            found
                .code
                .push(format!("{}:{}: {}", rel.display(), n + 1, line.trim()));
        }
    });
    found
}

/// Every crate root in the workspace, relative to `crates/`, sorted.
///
/// A *crate root* is a file the compiler starts a crate at: `src/lib.rs`, `src/main.rs`,
/// and — the part that is easy to forget — every file under `src/bin/`, `tests/`,
/// `benches/` and `examples/`. Each is its own crate, so an inner-attribute rule stated
/// in `lib.rs` does not reach any of them.
///
/// This exists for one caller — `every_crate_root_in_the_workspace_forbids_unsafe_code`
/// in `main.rs` — and lives here because the walk is the same walk: `crates/`, minus
/// `target`.
///
/// # Panics
///
/// Panics if `crates/` cannot be read, for the reason [`mentions`] does.
pub fn crate_roots() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/sfemu has two ancestors")
        .join("crates");
    assert!(root.is_dir(), "the crates directory must exist: {root:?}");

    let mut roots = Vec::new();
    walk(&root, &mut |path| {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.ends_with(".rs") {
            return;
        }
        let rel = path.strip_prefix(&root).unwrap_or(path).to_path_buf();
        // `<crate>/src/lib.rs` and `<crate>/src/main.rs` are roots; every other file
        // under `src/` is a module of one, reached by `mod`.
        let parent = rel
            .parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str());
        let is_root = match parent {
            Some("src") => name == "lib.rs" || name == "main.rs",
            Some("bin" | "tests" | "benches" | "examples") => true,
            _ => false,
        };
        if is_root {
            roots.push(rel);
        }
    });
    roots.sort();
    roots
}

/// Every crate manifest in the workspace, relative to `crates/`, sorted.
///
/// One caller — `every_crate_inherits_the_workspace_license` in `main.rs` — and it
/// lives here for the same reason [`crate_roots`] does: it is the same walk of
/// `crates/`, minus `target`.
///
/// A manifest is `<crate>/Cargo.toml` and nothing else. The depth check matters: a
/// `Cargo.toml` two levels down would belong to something that is not a workspace
/// member, and counting it would make the caller's floor pass on files no member
/// owns.
///
/// # Panics
///
/// Panics if `crates/` cannot be read, for the reason [`mentions`] does.
pub fn crate_manifests() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/sfemu has two ancestors")
        .join("crates");
    assert!(root.is_dir(), "the crates directory must exist: {root:?}");

    let mut found = Vec::new();
    walk(&root, &mut |path| {
        if path.file_name().and_then(|n| n.to_str()) != Some("Cargo.toml") {
            return;
        }
        let rel = path.strip_prefix(&root).unwrap_or(path).to_path_buf();
        // `<crate>/Cargo.toml` is two components; anything deeper is not a member's.
        if rel.components().count() == 2 {
            found.push(rel);
        }
    });
    found.sort();
    found
}

/// Calls `f` for every file under `dir`, recursively.
///
/// Skips `target` — a build directory holds vendored sources that would make these
/// checks report someone else's `use minifb` as ours. It is git-ignored and not normally
/// under `crates/`, but a stray one would turn a real check into a permanent failure.
fn walk(dir: &Path, f: &mut impl FnMut(&Path)) {
    let entries = std::fs::read_dir(dir).expect("a directory this crate can read");
    for e in entries {
        let path = e.expect("a readable directory entry").path();
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some("target") {
                continue;
            }
            walk(&path, f);
        } else {
            f(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scan finds a library that *is* there, and finds nothing for one that is not.
    ///
    /// Both halves matter. Without the first, a scan whose walk was broken — wrong root,
    /// an early `return`, a filter that excluded every file — would report an empty
    /// offender list, and every confinement test built on it would pass while checking
    /// nothing. Without the second, a scan that reported every line as a mention would
    /// also "work" until someone read the failure output.
    #[test]
    fn the_scan_finds_a_real_mention_and_not_an_absent_one() {
        // `machine` is a path dependency named in several manifests and used in code
        // across the tree, so it is a mention the scan must find.
        let real = mentions("machine", &[]);
        assert!(
            real.checked > 20,
            "the walk must have found the tree: {} files",
            real.checked
        );
        assert!(
            !real.code.is_empty(),
            "`machine` is used in code all over this tree"
        );
        assert!(
            !real.manifests.is_empty(),
            "and depended on in several manifests"
        );

        let absent = mentions("a_library_this_project_has_never_heard_of", &[]);
        assert_eq!(absent.checked, real.checked, "the same files were read");
        assert!(absent.code.is_empty(), "{:?}", absent.code);
        assert!(absent.manifests.is_empty(), "{:?}", absent.manifests);
    }

    /// An exempt file is skipped entirely, and skipping it is what makes the count drop.
    ///
    /// A confinement test names the one file allowed to hold the boundary. If `exempt`
    /// were ignored, that file's own mentions would be reported and the test would have
    /// to filter them out itself — which is what the two hand-written copies did, and
    /// the filtering is where they disagreed.
    #[test]
    fn an_exempt_file_is_not_read_at_all() {
        let all = mentions("machine", &[]);
        let without = mentions("machine", &["display.rs"]);
        assert_eq!(
            without.checked + 1,
            all.checked,
            "exactly one file was skipped"
        );
        assert!(
            all.code.iter().any(|m| m.contains("display.rs")),
            "the premise: display.rs names `machine` in code, so exempting it changes \
             the result"
        );
        assert!(
            !without.code.iter().any(|m| m.contains("display.rs")),
            "an exempt file's mentions must not be reported"
        );
    }

    /// Prose naming the library is allowed; code is not.
    #[test]
    fn a_comment_is_not_a_mention() {
        // `frontend/src/keys.rs` explains itself by naming `minifb` in prose, and
        // `frontend` does not depend on it — so every mention there is a comment.
        let m = mentions("minifb", &["display.rs", "Cargo.toml"]);
        assert!(
            !m.code.iter().any(|line| line.contains("frontend/src/keys")),
            "a comment was reported as code: {:?}",
            m.code
        );
        // And the file really does name it, or the above proves nothing.
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(Path::parent)
                .expect("two ancestors")
                .join("crates/frontend/src/keys.rs"),
        )
        .expect("keys.rs is readable");
        assert!(
            text.contains("minifb"),
            "the premise: frontend/src/keys.rs names `minifb` in prose"
        );
    }
}
