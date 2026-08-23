//! Reading a ROM set from a zip or a directory.

use crate::assemble::place;
use crate::spec::GameSpec;
use crate::zip::Archive;
use crate::RomError;
use std::collections::BTreeMap;
use std::path::Path;

/// A loaded ROM set: one assembled byte vector per region.
///
/// `Debug` prints the region map, which for a real set is megabytes — it exists
/// so test assertions can report a mismatch, not for printing at runtime.
#[derive(Debug)]
pub struct RomSet {
    /// Region tag to assembled bytes. Space no entry populates is zero.
    pub regions: BTreeMap<String, Vec<u8>>,
}

impl RomSet {
    /// The assembled bytes of one region.
    pub fn region(&self, name: &str) -> Option<&[u8]> {
        self.regions.get(name).map(Vec::as_slice)
    }
}

enum Source {
    Zip(Archive),
    Dir(std::path::PathBuf),
}

impl Source {
    fn get(&self, name: &str) -> Option<Vec<u8>> {
        match self {
            Self::Zip(a) => a.read(name).ok(),
            Self::Dir(d) => std::fs::read(d.join(name)).ok(),
        }
    }

    /// Every file this source offers, by name.
    fn names(&self) -> Vec<String> {
        match self {
            Self::Zip(a) => a.names(),
            Self::Dir(d) => std::fs::read_dir(d)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
        }
    }

    /// The one file of length `len` whose CRC-32 is `crc32`, if exactly one is.
    ///
    /// The fallback for a set whose files are named by a different convention than
    /// MAME's — WinKawaks-era sets call SF2's graphics `sf2_05.bin` where MAME
    /// calls the same bytes `sf2-1m.3a`. Renaming by hand is the alternative, and
    /// it is worse: a user who mis-renames two files gets a booting machine that
    /// draws garbage.
    ///
    /// This is **not** a relaxation of verification. A name is a label anyone can
    /// change; a CRC-32 is a statement about the bytes. Matching on the checksum
    /// the spec already demands is strictly stronger than matching on the name and
    /// then checking the checksum — the accepted file is the same file either way.
    ///
    /// `len` is checked first so a 512 KB file is never even hashed against a
    /// 128 KB entry, and **ambiguity is refused**: if two files in one set have
    /// the same length and CRC, this answers `None` rather than picking one, so
    /// the caller reports the file as missing rather than silently choosing.
    fn find_by_crc(&self, len: usize, crc32: u32) -> Option<Vec<u8>> {
        let mut hit = None;
        for n in self.names() {
            let Some(data) = self.get(&n) else { continue };
            if data.len() != len || crate::crc32::of(&data) != crc32 {
                continue;
            }
            if hit.is_some() {
                return None;
            }
            hit = Some(data);
        }
        hit
    }
}

/// Loads `spec` from `path`, which may be a zip archive or a directory of loose
/// files.
///
/// A directory is accepted because a user who owns the board and dumped it
/// themselves has loose files; requiring them to re-zip would serve nothing.
///
/// # Verification
///
/// Every entry's length **and** CRC-32 is checked, and a mismatch is an error
/// rather than a warning. A wrong or bad-dump ROM produces a 68000 executing
/// garbage, and the symptom surfaces thousands of instructions later as an
/// unexplained address error in code that looks like ours. Checking 32 bits here
/// converts a week of debugging into one line of output. There is deliberately
/// **no** "unknown CRC" exemption: an exemption is how verification stops
/// verifying.
///
/// # Names are a hint; the CRC is the identity
///
/// An entry not present under its MAME name is looked up by length and CRC-32
/// instead ([`Source::find_by_crc`]), because ROM sets in the wild are named by
/// several conventions for identical bytes. Every accepted file still satisfies
/// the spec's length and checksum exactly — the fallback changes which file is
/// *offered* to the check, never whether the check runs. A set with no file of
/// the right bytes is still [`RomError::Missing`].
///
/// # Errors
///
/// [`RomError::Missing`], [`RomError::WrongLength`], or [`RomError::Crc`] naming
/// the offending file; [`RomError::Io`] or [`RomError::Zip`] if the set itself
/// cannot be read; [`RomError::SpecOverflow`] if our own table is wrong.
pub fn load(spec: &GameSpec, path: &Path) -> Result<RomSet, RomError> {
    let src = if path.is_dir() {
        Source::Dir(path.to_path_buf())
    } else {
        Source::Zip(Archive::open(path)?)
    };

    let mut regions = BTreeMap::new();
    for region in spec.regions {
        let mut buf = vec![0u8; region.size];
        for entry in region.entries {
            // By name first, so a correctly named set never pays for a scan of the
            // archive; by CRC only when the name is absent.
            let data = src
                .get(entry.name)
                .or_else(|| src.find_by_crc(entry.len, entry.crc32))
                .ok_or(RomError::Missing {
                    region: region.name,
                    name: entry.name,
                })?;
            if data.len() != entry.len {
                return Err(RomError::WrongLength {
                    name: entry.name,
                    want: entry.len,
                    got: data.len(),
                });
            }
            let got = crate::crc32::of(&data);
            if got != entry.crc32 {
                return Err(RomError::Crc {
                    name: entry.name,
                    want: entry.crc32,
                    got,
                });
            }
            place(&mut buf, &data, entry, region.name)?;
        }
        regions.insert(region.name.to_string(), buf);
    }
    Ok(RomSet { regions })
}

/// Which supported game a set at `path` is, and the set loaded.
///
/// [`load`] answers "does this path hold *that* game"; this answers "which game
/// does this path hold". A caller who has a set and does not know its MAME name —
/// a user running the emulator, a gated test that must work for whoever supplies
/// the ROMs — needs the second question, and guessing from the file name is not
/// an answer: `sf2.zip` in the wild routinely holds a different revision.
///
/// # Why this is trustworthy, and why it could not have been before
///
/// Every spec is tried in [`games::ALL`](crate::games::ALL) order and the first
/// that loads wins. That is only sound because [`load`] verifies the CRC-32 of
/// every entry: a spec that loads has been shown to match the bytes, so "the
/// first that loads" cannot be a near-miss. Ordering therefore matters solely for
/// speed, not for correctness — two specs cannot both load the same set unless
/// they are byte-identical, in which case either answer is the same machine.
///
/// The revision distinction is exactly what this exists to get right. `sf2` and
/// `sf2eb` share their graphics, audio and sample regions and differ only in the
/// eight program ROMs, so a set is told apart from its near neighbour by 32 bits
/// per program ROM and nothing else.
///
/// # Errors
///
/// [`RomError::Unknown`] if no spec loads. The error carries the last spec's
/// failure, because a user whose set is *almost* one of these is far better served
/// by "sf1's `sf-15.bin` is missing" than by "unknown set": the second sends them
/// looking for a bug in this program.
pub fn identify(path: &Path) -> Result<(&'static GameSpec, RomSet), RomError> {
    let mut last = None;
    for spec in crate::games::ALL {
        match load(spec, path) {
            Ok(set) => return Ok((spec, set)),
            Err(e) => last = Some(e),
        }
    }
    Err(RomError::Unknown {
        // `expect`: `games::ALL` is a non-empty static, so the loop ran at least
        // once and `last` is `Some`. A `games::ALL` emptied by an edit would be a
        // workspace with no supported games at all, which every other test fails.
        why: Box::new(last.expect("games::ALL is not empty")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{LoadKind, RegionSpec, RomEntry};

    fn pat(tag: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| tag | (i as u8 & 0x0F)).collect()
    }

    /// A two-file byte-interleaved region, the shape SF2's `maincpu` has, with
    /// the CRCs the synthetic files actually have.
    ///
    /// The CRCs are computed rather than written as literals because they are
    /// checksums of a pattern this test defines — the alternative is not a
    /// stronger test but a `crc32::of` call spelled as a magic number. What must
    /// not happen, and does not, is `load` gaining an exemption for a zero CRC:
    /// an exemption for "not known yet" is how verification stops verifying.
    ///
    /// `region_size` lets one caller widen the region past what the entries
    /// populate, so the zero-fill of unpopulated space is observable.
    fn spec_with_real_crcs(region_size: usize) -> (GameSpec, Vec<u8>, Vec<u8>) {
        let even = pat(0xA0, 8);
        let odd = pat(0xB0, 8);
        let entries: &'static [RomEntry] = Box::leak(Box::new([
            RomEntry {
                name: "even.bin",
                offset: 0,
                len: 8,
                crc32: crate::crc32::of(&pat(0xA0, 8)),
                load: LoadKind::Word16Byte,
            },
            RomEntry {
                name: "odd.bin",
                offset: 1,
                len: 8,
                crc32: crate::crc32::of(&pat(0xB0, 8)),
                load: LoadKind::Word16Byte,
            },
        ]));
        let regions: &'static [RegionSpec] = Box::leak(Box::new([RegionSpec {
            name: "maincpu",
            size: region_size,
            entries,
        }]));
        (
            GameSpec {
                name: "synthetic",
                regions,
            },
            even,
            odd,
        )
    }

    /// A directory that is not any supported set is refused, and the message names
    /// what was tried and the closest miss.
    ///
    /// The message is asserted, not just the variant. "Unrecognised" alone sends a
    /// user looking for a bug in this program; the specific miss — a named file
    /// from a named region — sends them to look at their files, which is where the
    /// problem is. So the wording is the behaviour here.
    #[test]
    fn a_directory_that_is_no_supported_game_is_refused_with_the_closest_miss() {
        let dir = write_dir("identify-unknown", &[("nothing.bin", &[0u8; 16])]);
        let e = identify(&dir).expect_err("16 zero bytes is not Street Fighter");
        let msg = e.to_string();
        assert!(matches!(e, RomError::Unknown { .. }), "{msg}");
        for name in crate::games::ALL.iter().map(|g| g.name) {
            assert!(msg.contains(name), "the message must name {name}: {msg}");
        }
        assert!(
            msg.contains("is missing"),
            "and carry the underlying miss: {msg}"
        );
    }

    /// No two supported specs describe the same bytes, so "the first that loads"
    /// is an identification and not a coin flip.
    ///
    /// This is the assumption [`identify`] rests on, and it is checked against the
    /// spec tables rather than assumed: every pair of specs must differ in some
    /// entry's CRC-32. Two specs that agreed on all of them would make `identify`
    /// order-dependent — and the pair at risk is precisely `sf2` and `sf2eb`, which
    /// share three of their four regions by *reference*.
    #[test]
    fn every_supported_spec_is_distinguishable_from_every_other() {
        let crcs = |g: &crate::GameSpec| -> Vec<u32> {
            let mut v: Vec<u32> = g
                .regions
                .iter()
                .flat_map(|r| r.entries.iter().map(|e| e.crc32))
                .collect();
            v.sort_unstable();
            v
        };
        let all = crate::games::ALL;
        assert!(all.len() >= 2, "there is something to distinguish");
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(
                    crcs(a),
                    crcs(b),
                    "`{}` and `{}` describe the same bytes",
                    a.name,
                    b.name
                );
            }
        }
    }

    /// Each caller passes a distinct `tag` so no two tests share a directory.
    fn write_dir(tag: &str, files: &[(&str, &[u8])]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("romset-test-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (n, d) in files {
            std::fs::write(dir.join(n), d).unwrap();
        }
        dir
    }

    #[test]
    fn loads_a_directory_and_interleaves_correctly() {
        let (spec, even, odd) = spec_with_real_crcs(16);
        let dir = write_dir("dir-ok", &[("even.bin", &even), ("odd.bin", &odd)]);
        let set = load(&spec, &dir).unwrap();
        let r = &set.regions["maincpu"];
        assert_eq!(r.len(), 16);
        assert_eq!(
            &r[..4],
            &[0xA0, 0xB0, 0xA1, 0xB1],
            "even file is the high byte"
        );
    }

    #[test]
    fn loads_a_zip_to_the_same_bytes_as_the_directory() {
        // The two sources must be interchangeable, and comparing them to each
        // other would be self-consistent — so both are compared to the same
        // hand-written literal.
        let (spec, even, odd) = spec_with_real_crcs(16);
        let dir = write_dir("dir-vs-zip", &[("even.bin", &even), ("odd.bin", &odd)]);
        let from_dir = load(&spec, &dir).unwrap();

        let zip_path = std::env::temp_dir().join("romset-test-dir-vs-zip.zip");
        std::fs::write(
            &zip_path,
            stored_zip(&[("even.bin", &even), ("odd.bin", &odd)]),
        )
        .unwrap();
        let from_zip = load(&spec, &zip_path).unwrap();

        assert_eq!(&from_zip.regions["maincpu"][..4], &[0xA0, 0xB0, 0xA1, 0xB1]);
        assert_eq!(from_zip.regions, from_dir.regions);
    }

    #[test]
    fn a_flipped_bit_fails_with_the_file_name_and_both_crcs() {
        let (spec, even, odd) = spec_with_real_crcs(16);
        let mut bad = odd.clone();
        bad[3] ^= 0x01;
        let dir = write_dir("crc-bad", &[("even.bin", &even), ("odd.bin", &bad)]);
        match load(&spec, &dir) {
            Err(RomError::Crc { name, want, got }) => {
                assert_eq!(name, "odd.bin");
                assert_eq!(want, crate::crc32::of(&odd));
                assert_ne!(got, want);
            }
            other => panic!("a one-bit change must be a Crc error, got {other:?}"),
        }
    }

    /// A spec CRC of zero is checked like any other value.
    ///
    /// Found by mutation: adding `entry.crc32 != 0 &&` to the check survived the
    /// whole suite, because no test entry had a zero expected CRC. That mutant is
    /// not hypothetical — "skip the check when we don't know the value yet" is the
    /// single most natural change anyone will propose to this function, and the
    /// doc comment above already declares it forbidden. A declaration no test
    /// enforces is the defect this project keeps producing, so here is the test.
    ///
    /// Zero is a legitimate CRC-32 (of empty input), which is why the exemption
    /// would be wrong even as a convention rather than merely lax.
    #[test]
    fn a_spec_crc_of_zero_is_still_checked() {
        let entries: &'static [RomEntry] = Box::leak(Box::new([RomEntry {
            name: "even.bin",
            offset: 0,
            len: 8,
            crc32: 0,
            load: LoadKind::Byte,
        }]));
        let regions: &'static [RegionSpec] = Box::leak(Box::new([RegionSpec {
            name: "maincpu",
            size: 8,
            entries,
        }]));
        let spec = GameSpec {
            name: "zero-crc",
            regions,
        };
        let data = pat(0xA0, 8);
        assert_ne!(
            crate::crc32::of(&data),
            0,
            "the fixture must disagree with 0"
        );
        let dir = write_dir("zero-crc", &[("even.bin", &data)]);
        match load(&spec, &dir) {
            Err(RomError::Crc { name, want, .. }) => {
                assert_eq!(name, "even.bin");
                assert_eq!(want, 0, "the spec's zero is what was compared against");
            }
            other => panic!("a zero spec CRC must be enforced, not skipped: {other:?}"),
        }
    }

    /// A file under the wrong name is found by its CRC-32.
    ///
    /// The WinKawaks case: identical bytes, a different naming convention. The
    /// assertion is on the assembled region and not merely on "it loaded", because
    /// a fallback that found the *other* file of the same length would also load.
    #[test]
    fn a_file_under_a_foreign_name_is_found_by_its_crc() {
        let (spec, even, odd) = spec_with_real_crcs(16);
        let dir = write_dir("crc-rename", &[("sf2_05.bin", &even), ("sf2_06.bin", &odd)]);
        let set = load(&spec, &dir).expect("the bytes are right, only the names differ");
        assert_eq!(
            &set.regions["maincpu"][..4],
            &[0xA0, 0xB0, 0xA1, 0xB1],
            "and each landed on its own byte lane, not merely somewhere"
        );
    }

    /// A renamed file whose bytes are wrong is still missing.
    ///
    /// The property that keeps the fallback from being a relaxation: it searches
    /// for the CRC the spec demands, so a corrupt file cannot be admitted by any
    /// name. Without this the fallback would read as "accept whatever is there".
    #[test]
    fn a_renamed_file_with_the_wrong_bytes_is_not_accepted() {
        let (spec, even, odd) = spec_with_real_crcs(16);
        let mut bad = odd.clone();
        bad[3] ^= 0x01;
        let dir = write_dir("crc-rename-bad", &[("even.bin", &even), ("zzz.bin", &bad)]);
        assert_eq!(
            load(&spec, &dir).unwrap_err(),
            RomError::Missing {
                region: "maincpu",
                name: "odd.bin"
            },
            "no file has odd.bin's checksum, so the set is short one ROM"
        );
    }

    /// Two candidate files with the same bytes are refused, not guessed between.
    ///
    /// A merged set can hold the same ROM twice. Picking either would be correct
    /// *here* — they are byte-identical — but "pick the first match" is a rule that
    /// silently resolves genuine ambiguity too, and a loader that guesses is the
    /// thing this crate's CRC checking exists to prevent. `None` sends the user a
    /// missing-file error naming the ROM, which is actionable.
    #[test]
    fn two_files_with_the_same_crc_are_ambiguous_rather_than_guessed() {
        let (spec, even, odd) = spec_with_real_crcs(16);
        let dir = write_dir(
            "crc-ambiguous",
            &[("even.bin", &even), ("a.bin", &odd), ("b.bin", &odd)],
        );
        assert_eq!(
            load(&spec, &dir).unwrap_err(),
            RomError::Missing {
                region: "maincpu",
                name: "odd.bin"
            }
        );
    }

    /// The correct name wins over a CRC scan.
    ///
    /// Ordering matters for more than speed: with a file present under its proper
    /// name, that file is the one loaded, so a set containing both a properly named
    /// ROM and a stray duplicate is not thrown into the ambiguity case above.
    #[test]
    fn a_correctly_named_file_is_used_even_when_a_duplicate_exists() {
        let (spec, even, odd) = spec_with_real_crcs(16);
        let dir = write_dir(
            "crc-name-first",
            &[
                ("even.bin", &even),
                ("odd.bin", &odd),
                ("odd-copy.bin", &odd),
            ],
        );
        let set = load(&spec, &dir).expect("odd.bin is present under its own name");
        assert_eq!(&set.regions["maincpu"][..4], &[0xA0, 0xB0, 0xA1, 0xB1]);
    }

    #[test]
    fn a_missing_file_names_the_region_and_the_file() {
        let (spec, even, _) = spec_with_real_crcs(16);
        let dir = write_dir("missing", &[("even.bin", &even)]);
        assert_eq!(
            load(&spec, &dir).unwrap_err(),
            RomError::Missing {
                region: "maincpu",
                name: "odd.bin"
            }
        );
    }

    #[test]
    fn a_short_file_is_a_length_error_not_a_crc_error() {
        // Distinct diagnoses: a truncated file is a different user problem from a
        // wrong revision, and collapsing both into "CRC mismatch" sends the user
        // looking for the wrong thing.
        let (spec, even, odd) = spec_with_real_crcs(16);
        let dir = write_dir("short", &[("even.bin", &even), ("odd.bin", &odd[..4])]);
        assert_eq!(
            load(&spec, &dir).unwrap_err(),
            RomError::WrongLength {
                name: "odd.bin",
                want: 8,
                got: 4
            }
        );
    }

    #[test]
    fn unpopulated_space_in_a_region_is_zero() {
        // SF2 populates 0x000000-0x0FFFFF of a 0x400000 region; the rest must read
        // as zero rather than as uninitialised memory, because that is what an
        // unpopulated socket returns and what the 68000 will fetch if it ever
        // jumps there.
        let (spec, even, odd) = spec_with_real_crcs(32);
        let dir = write_dir("zero-tail", &[("even.bin", &even), ("odd.bin", &odd)]);
        let set = load(&spec, &dir).unwrap();
        let r = &set.regions["maincpu"];
        assert_eq!(r.len(), 32);
        assert_eq!(
            &r[..4],
            &[0xA0, 0xB0, 0xA1, 0xB1],
            "the populated part is unchanged"
        );
        assert_eq!(&r[16..], &[0u8; 16], "the unpopulated tail is zero");
    }

    #[test]
    fn a_path_that_is_neither_a_zip_nor_a_directory_is_an_error() {
        let (spec, _, _) = spec_with_real_crcs(16);
        let nope = std::env::temp_dir().join("romset-test-does-not-exist.zip");
        let _ = std::fs::remove_file(&nope);
        match load(&spec, &nope) {
            Err(RomError::Io { path, .. }) => assert!(path.contains("does-not-exist")),
            other => panic!("expected an Io error naming the path, got {other:?}"),
        }
    }

    /// A stored-only zip, so this module needs no deflate machinery of its own.
    fn stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let crc = crate::crc32::of(data);
            let local_off = out.len() as u32;
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            for _ in 0..2 {
                out.extend_from_slice(&20u16.to_le_bytes());
            }
            out.extend_from_slice(&0u16.to_le_bytes()); // method 0 (stored)
            out.extend_from_slice(&0u32.to_le_bytes()); // time + date
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);

            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // flags
            central.extend_from_slice(&0u16.to_le_bytes()); // method
            central.extend_from_slice(&0u32.to_le_bytes()); // time + date
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // extra
            central.extend_from_slice(&0u16.to_le_bytes()); // comment
            central.extend_from_slice(&0u16.to_le_bytes()); // disk
            central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            central.extend_from_slice(&local_off.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        let cd_len = central.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // disks
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_len.to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }
}
