//! Loading MAME-format ROM sets supplied by the user at runtime.
//!
//! # This crate never obtains a ROM
//!
//! It reads a path handed to it. It contains no URL, no download, no embedded
//! ROM data, and no test fixture holding any. Its tests build synthetic archives
//! from patterns they generate. See [`spec`] for why the static tables are
//! metadata rather than data.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod assemble;
pub mod crc32;
pub mod games;
pub mod load;
pub mod spec;
pub mod zip;

pub use games::SF2;
pub use load::{identify, load, RomSet};
pub use spec::{GameSpec, LoadKind, RegionSpec, RomEntry};

/// A host fault: our setup is wrong, not the guest's.
///
/// Every variant names the file, because the whole value of checking is that the
/// message says which of eight interleaved files is bad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RomError {
    /// The archive or directory itself could not be read.
    Io {
        /// The path we tried.
        path: String,
        /// The OS's explanation.
        detail: String,
    },
    /// A file the spec requires is not in the set.
    Missing {
        /// The region that wanted it.
        region: &'static str,
        /// The file name the spec asked for.
        name: &'static str,
    },
    /// The file is present but the wrong length.
    WrongLength {
        /// The file.
        name: &'static str,
        /// The length the spec expects.
        want: usize,
        /// The length found.
        got: usize,
    },
    /// The file is present and the right length but the wrong content.
    Crc {
        /// The file.
        name: &'static str,
        /// The CRC-32 the spec expects.
        want: u32,
        /// The CRC-32 computed.
        got: u32,
    },
    /// The spec places an entry past the end of its region — our bug, not the
    /// user's, so it says so.
    SpecOverflow {
        /// The region.
        region: &'static str,
        /// The entry that does not fit.
        name: &'static str,
        /// One past the last byte the entry would write.
        end: usize,
        /// The region's size.
        size: usize,
    },
    /// The archive is not a zip, or uses a compression method MAME sets do not.
    Zip {
        /// The archive path.
        path: String,
        /// What is wrong with it.
        detail: String,
    },
    /// [`load::identify`] found no supported game at the path.
    Unknown {
        /// Why the last spec tried did not load.
        ///
        /// Boxed because `RomError` would otherwise contain itself. Carried rather
        /// than discarded because a user whose set is *nearly* one of ours is far
        /// better served by the specific miss — a named missing file, a CRC that
        /// says "wrong revision or a bad dump" — than by a bare "unrecognised",
        /// which reads as a bug in this program rather than a fact about the files.
        why: Box<RomError>,
    },
}

impl core::fmt::Display for RomError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io { path, detail } => write!(f, "cannot read {path}: {detail}"),
            Self::Missing { region, name } => {
                write!(
                    f,
                    "region `{region}` is missing `{name}` — is this the right ROM set?"
                )
            }
            Self::WrongLength { name, want, got } => {
                write!(f, "`{name}` is {got} bytes, expected {want}")
            }
            Self::Crc { name, want, got } => write!(
                f,
                "`{name}` has CRC32 {got:08x}, expected {want:08x} \
                 — wrong revision or a bad dump"
            ),
            Self::SpecOverflow {
                region,
                name,
                end,
                size,
            } => write!(
                f,
                "internal: `{name}` ends at {end:#x} but region `{region}` is only \
                 {size:#x} — the spec table is wrong"
            ),
            Self::Zip { path, detail } => write!(f, "{path} is not a usable zip: {detail}"),
            Self::Unknown { why } => {
                let names: Vec<&str> = games::ALL.iter().map(|g| g.name).collect();
                write!(
                    f,
                    "not a supported ROM set — tried {}. The closest miss: {why}",
                    names.join(", ")
                )
            }
        }
    }
}

impl std::error::Error for RomError {}
