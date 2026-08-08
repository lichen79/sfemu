//! The Z80 vector suite's file inventory.
//!
//! Derived from the naming rules rather than typed out, and then checked against
//! the counts measured upstream on 2026-08-08. Both halves matter: the rules keep
//! it to 40 lines, and the counts catch a rule that is plausible and wrong.

/// The number of vector files the suite has.
pub const EXPECTED: usize = 1604;

/// The bytes that open a page instead of being an instruction.
const PREFIXES: [u8; 4] = [0xCB, 0xDD, 0xED, 0xFD];

/// Which opcode page a file belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Base,
    Cb,
    Ed,
    Dd,
    Fd,
    DdCb,
    FdCb,
}

impl Page {
    /// Every page, in the order their discriminants are used as indices.
    pub const ALL: [Page; 7] = [
        Page::Base,
        Page::Cb,
        Page::Ed,
        Page::Dd,
        Page::Fd,
        Page::DdCb,
        Page::FdCb,
    ];

    /// Whether this page claims `name`.
    ///
    /// Order matters: `dd cb __ 06` starts with `dd `, so the double-prefix pages
    /// must be tested before the single ones. That precedence is why this is one
    /// function rather than seven independent predicates.
    ///
    /// [`Page::Base`] checks for two hex digits rather than merely for the absence
    /// of a space. The looser form would claim any unrecognised stem — a stray file
    /// in `testdata/z80`, or a name from a future suite revision — and a coverage
    /// test would then count it as a base-page file it had never run.
    #[must_use]
    pub fn claims(self, name: &str) -> bool {
        match self {
            Page::DdCb => name.starts_with("dd cb "),
            Page::FdCb => name.starts_with("fd cb "),
            Page::Cb => name.starts_with("cb "),
            Page::Ed => name.starts_with("ed "),
            Page::Dd => name.starts_with("dd ") && !name.starts_with("dd cb "),
            Page::Fd => name.starts_with("fd ") && !name.starts_with("fd cb "),
            Page::Base => {
                name.len() == 2
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            }
        }
    }
}

/// The page a name belongs to, or `None` if no page claims it.
#[must_use]
pub fn page_of(name: &str) -> Option<Page> {
    Page::ALL.into_iter().find(|p| p.claims(name))
}

/// How many names fall on each page, indexed by `Page`'s discriminant.
#[must_use]
pub fn page_counts(names: &[String]) -> [usize; 7] {
    let mut c = [0; 7];
    for n in names {
        if let Some(p) = page_of(n) {
            c[p as usize] += 1;
        }
    }
    c
}

/// The on-disk stem for a vector name: spaces become underscores.
///
/// `dd cb __ 06` becomes `dd_cb____06` — four underscores between `cb` and `06`,
/// because the placeholder's own two are kept and the two spaces around it are
/// converted as well. It looks odd and it round-trips, which is what matters.
#[must_use]
pub fn stem(name: &str) -> String {
    name.replace(' ', "_")
}

/// Every vector file's upstream name, in a stable order.
#[must_use]
pub fn all_names() -> Vec<String> {
    let mut out = Vec::with_capacity(EXPECTED);
    let plain = |b: u8| !PREFIXES.contains(&b);
    for op in 0u8..=0xFF {
        if plain(op) {
            out.push(format!("{op:02x}"));
        }
    }
    for op in 0u8..=0xFF {
        out.push(format!("cb {op:02x}"));
    }
    // The `ed` page is sparse: 40-7f entire, plus the four block-instruction
    // quartets. Everything else on the page is undefined and unshipped.
    for op in 0x40u8..=0x7F {
        out.push(format!("ed {op:02x}"));
    }
    for base in [0xA0u8, 0xA8, 0xB0, 0xB8] {
        for op in base..base + 4 {
            out.push(format!("ed {op:02x}"));
        }
    }
    for p in ["dd", "fd"] {
        for op in 0u8..=0xFF {
            if plain(op) {
                out.push(format!("{p} {op:02x}"));
            }
        }
    }
    for p in ["dd", "fd"] {
        for op in 0u8..=0xFF {
            out.push(format!("{p} cb __ {op:02x}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The derived list matches the inventory measured upstream on 2026-08-08.
    ///
    /// The totals are literals. The generation rules could each be wrong in a way
    /// that still produces a plausible list, and the only thing that catches that
    /// is the count the suite actually has.
    #[test]
    fn all_names_matches_the_upstream_inventory() {
        let n = all_names();
        assert_eq!(n.len(), 1604, "the suite has 1,604 files");
        assert_eq!(n.len(), EXPECTED, "and EXPECTED must say so too");
        let c = page_counts(&n);
        assert_eq!(c[Page::Base as usize], 252, "00-ff less the four prefixes");
        assert_eq!(c[Page::Cb as usize], 256);
        assert_eq!(c[Page::Ed as usize], 80, "only 40-7f and the block forms");
        assert_eq!(c[Page::Dd as usize], 252);
        assert_eq!(c[Page::Fd as usize], 252);
        assert_eq!(c[Page::DdCb as usize], 256);
        assert_eq!(c[Page::FdCb as usize], 256);
        // And the per-page counts account for every name: a page predicate that
        // claimed nothing would leave the sum short while each literal above still
        // matched its own (zero) expectation only if that literal were zero too.
        assert_eq!(c.iter().sum::<usize>(), EXPECTED, "no name is unclaimed");
    }

    /// The four prefix bytes have no file of their own on the pages that omit them.
    #[test]
    fn a_prefix_byte_is_not_an_instruction_file() {
        let n = all_names();
        for absent in ["cb", "dd", "ed", "fd"] {
            assert!(!n.contains(&absent.to_string()), "{absent} is a prefix");
            for p in ["dd", "fd"] {
                let s = format!("{p} {absent}");
                assert!(!n.contains(&s), "{s} is a prefix or its own page");
            }
        }
        // But the pages they open are all there.
        assert!(n.contains(&"cb 00".to_string()));
        assert!(n.contains(&"ed 40".to_string()));
        assert!(n.contains(&"dd cb __ 06".to_string()));
        assert!(n.contains(&"fd cb __ ff".to_string()));
    }

    /// The `ed` page holds exactly the defined opcodes and no others.
    ///
    /// Spot-checked at the boundaries, because an off-by-one in a range is what
    /// this rule is most likely to get wrong and the count alone would not catch
    /// a range shifted by one in both directions.
    #[test]
    fn the_ed_page_is_the_defined_opcodes_only() {
        let n = all_names();
        let has = |s: &str| n.contains(&s.to_string());
        assert!(has("ed 40") && has("ed 7f"), "the 40-7f block is whole");
        assert!(!has("ed 3f") && !has("ed 80"), "and it stops at both ends");
        assert!(has("ed a0") && has("ed a3"), "LDI CPI INI OUTI");
        assert!(!has("ed a4"), "and nothing after them");
        assert!(has("ed b8") && has("ed bb"), "LDDR CPDR INDR OTDR");
        assert!(!has("ed bc") && !has("ed 9f"), "nor around them");
    }

    /// A name maps to a filesystem stem with no spaces.
    ///
    /// Spaces in paths work but invite quoting bugs in every shell command that
    /// touches `testdata/`, and the `__` displacement placeholder must survive
    /// unmangled so a stem still reads as its opcode.
    #[test]
    fn a_stem_replaces_spaces_and_keeps_the_displacement_marker() {
        assert_eq!(stem("00"), "00");
        assert_eq!(stem("cb 06"), "cb_06");
        assert_eq!(stem("dd cb __ 06"), "dd_cb____06");
        // Distinct names must give distinct stems, or two files collide silently.
        let n = all_names();
        let mut stems: Vec<String> = n.iter().map(|s| stem(s)).collect();
        stems.sort();
        let before = stems.len();
        stems.dedup();
        assert_eq!(stems.len(), before, "stems must be unique");
    }

    /// Every name is claimed by exactly one page.
    #[test]
    fn page_of_partitions_the_list() {
        for name in all_names() {
            let p = page_of(&name).unwrap_or_else(|| panic!("{name} has no page"));
            // And the page's own prefix test agrees.
            assert!(p.claims(&name), "{name} vs {p:?}");
            for other in Page::ALL {
                if other != p {
                    assert!(
                        !other.claims(&name),
                        "{name} claimed by {p:?} and {other:?}"
                    );
                }
            }
        }
    }

    /// A name from outside the suite belongs to no page.
    ///
    /// This is what lets a later coverage test notice a stray file rather than
    /// silently counting it against the base page's total.
    #[test]
    fn an_unrecognised_name_belongs_to_no_page() {
        for junk in ["", "zz", "0", "000", "gg", "0G", "ff ", " ff", "xy 00"] {
            assert_eq!(page_of(junk), None, "{junk:?} must not be claimed");
        }
    }
}
