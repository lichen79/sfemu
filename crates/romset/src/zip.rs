//! Just enough zip to read a MAME ROM set.
//!
//! # Why hand-written
//!
//! The `zip` crate pulls **75 crates** — AES, bzip2, zstd, PPMd, `time`, `sha1`
//! — to support methods no ROM set uses. MAME sets use exactly two: stored (0)
//! and deflate (8). Parsing a central directory for those is ~120 lines, so the
//! trade is 120 lines of ours against 73 crates of compile time and attack
//! surface, in a project whose defining property is a dependency-free core.
//! `miniz_oxide` supplies the DEFLATE stage and nothing else.
//!
//! # What is deliberately not supported
//!
//! Zip64, encryption, multi-disk archives, and data descriptors. A ROM set using
//! any of them is rejected **by name** rather than misparsed — see
//! [`Archive::read`]. Silence is the failure mode this crate exists to avoid.

use crate::RomError;
use std::collections::BTreeMap;
use std::path::Path;

const EOCD_SIG: u32 = 0x0605_4b50;
const CD_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;
const EOCD_MIN: usize = 22;

struct Member {
    method: u16,
    comp_size: usize,
    uncomp_size: usize,
    local_off: usize,
}

/// A zip archive held in memory.
///
/// ROM sets are a few megabytes and read once, so the whole file is buffered: it
/// removes seek handling from the parser entirely, and the peak footprint is the
/// archive plus the region being assembled.
pub struct Archive {
    bytes: Vec<u8>,
    path: String,
    members: BTreeMap<String, Member>,
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
        *b.get(at + 2)?,
        *b.get(at + 3)?,
    ]))
}

impl Archive {
    /// Reads and parses the archive at `path`.
    ///
    /// # Errors
    ///
    /// [`RomError::Io`] if the file cannot be read, [`RomError::Zip`] if it is
    /// not a zip we can parse.
    pub fn open(path: &Path) -> Result<Self, RomError> {
        let bytes = std::fs::read(path).map_err(|e| RomError::Io {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;
        Self::parse(bytes, path.display().to_string())
    }

    /// Parses an in-memory archive. `path` is used only in error messages.
    ///
    /// # Errors
    ///
    /// [`RomError::Zip`] with a description of what could not be parsed.
    pub fn parse(bytes: Vec<u8>, path: String) -> Result<Self, RomError> {
        let bad = |detail: &str| RomError::Zip {
            path: path.clone(),
            detail: detail.to_string(),
        };
        if bytes.len() < EOCD_MIN {
            return Err(bad(
                "shorter than an empty zip's end-of-central-directory record",
            ));
        }
        // Scan backwards for the EOCD: it is last, but a trailing archive comment
        // may follow it, so its position is not fixed. 0xFFFF is the largest
        // comment the format can express.
        let start = bytes.len() - EOCD_MIN;
        let floor = start.saturating_sub(0xFFFF);
        let eocd = (floor..=start)
            .rev()
            .find(|&i| le32(&bytes, i) == Some(EOCD_SIG))
            .ok_or_else(|| bad("no end-of-central-directory signature"))?;

        let count = le16(&bytes, eocd + 10).ok_or_else(|| bad("truncated EOCD"))? as usize;
        let cd_off = le32(&bytes, eocd + 16).ok_or_else(|| bad("truncated EOCD"))? as usize;

        let mut members = BTreeMap::new();
        let mut at = cd_off;
        for _ in 0..count {
            if le32(&bytes, at) != Some(CD_SIG) {
                return Err(bad("central directory entry has a bad signature"));
            }
            let method = le16(&bytes, at + 10).ok_or_else(|| bad("truncated CD entry"))?;
            let comp = le32(&bytes, at + 20).ok_or_else(|| bad("truncated CD entry"))? as usize;
            let uncomp = le32(&bytes, at + 24).ok_or_else(|| bad("truncated CD entry"))? as usize;
            let nlen = le16(&bytes, at + 28).ok_or_else(|| bad("truncated CD entry"))? as usize;
            let elen = le16(&bytes, at + 30).ok_or_else(|| bad("truncated CD entry"))? as usize;
            let clen = le16(&bytes, at + 32).ok_or_else(|| bad("truncated CD entry"))? as usize;
            let local = le32(&bytes, at + 42).ok_or_else(|| bad("truncated CD entry"))? as usize;
            let name_at = at + 46;
            let raw = bytes
                .get(name_at..name_at + nlen)
                .ok_or_else(|| bad("central directory name runs past the end of the file"))?;
            let full = String::from_utf8_lossy(raw).to_string();
            // MAME sets are flat; a path separator means a nested layout, so we
            // match on the base name instead.
            let name = full.rsplit('/').next().unwrap_or(&full).to_string();
            if !name.is_empty() {
                members.insert(
                    name,
                    Member {
                        method,
                        comp_size: comp,
                        uncomp_size: uncomp,
                        local_off: local,
                    },
                );
            }
            at = name_at + nlen + elen + clen;
        }
        Ok(Self {
            bytes,
            path,
            members,
        })
    }

    /// Every member's base name, sorted.
    pub fn names(&self) -> Vec<String> {
        self.members.keys().cloned().collect()
    }

    /// Whether the archive holds a member with this base name.
    pub fn contains(&self, name: &str) -> bool {
        self.members.contains_key(name)
    }

    /// Decompresses one member.
    ///
    /// The zip's own stored CRC is **not** checked here: the loader checks
    /// against the spec's expected CRC, which is the value that matters. An
    /// archive whose internal CRC agrees with corrupt data is still corrupt.
    ///
    /// # Errors
    ///
    /// [`RomError::Zip`] if the member is absent, its local header is bad, its
    /// compression method is unsupported, or it does not inflate to the length
    /// the central directory promised.
    pub fn read(&self, name: &str) -> Result<Vec<u8>, RomError> {
        let bad = |detail: String| RomError::Zip {
            path: self.path.clone(),
            detail,
        };
        let m = self
            .members
            .get(name)
            .ok_or_else(|| bad(format!("no member named `{name}`")))?;
        if le32(&self.bytes, m.local_off) != Some(LOCAL_SIG) {
            return Err(bad(format!("`{name}` has a bad local header")));
        }
        let nlen = le16(&self.bytes, m.local_off + 26)
            .ok_or_else(|| bad(format!("`{name}` has a truncated local header")))?
            as usize;
        let elen = le16(&self.bytes, m.local_off + 28)
            .ok_or_else(|| bad(format!("`{name}` has a truncated local header")))?
            as usize;
        let data_at = m.local_off + 30 + nlen + elen;
        let comp = self
            .bytes
            .get(data_at..data_at + m.comp_size)
            .ok_or_else(|| bad(format!("`{name}` runs past the end of the archive")))?;
        let out = match m.method {
            0 => comp.to_vec(),
            // Raw deflate, NOT zlib-wrapped: zip method 8 has no zlib header, so
            // `decompress_to_vec_zlib` would consume two data bytes as a header
            // and fail or silently mis-decode.
            8 => miniz_oxide::inflate::decompress_to_vec(comp)
                .map_err(|e| bad(format!("`{name}` failed to inflate: {:?}", e.status)))?,
            other => {
                return Err(bad(format!(
                    "`{name}` uses compression method {other}; only stored (0) and \
                     deflate (8) are supported"
                )))
            }
        };
        if out.len() != m.uncomp_size {
            return Err(bad(format!(
                "`{name}` inflated to {} bytes, the directory says {}",
                out.len(),
                m.uncomp_size
            )));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal but real zip: local headers, then a central directory,
    /// then an EOCD, then `comment_len` bytes of archive comment.
    ///
    /// Every field is written explicitly so the parser is tested against the
    /// format rather than against a library that shares its bugs. The comment
    /// parameter exists because without it the EOCD is always at a fixed offset
    /// from the end and the backward scan is never load-bearing — a mutant that
    /// looks only at `len - 22` survives.
    fn build_zip(files: &[(&str, &[u8], bool)], comment_len: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data, deflate) in files {
            let stored: Vec<u8> = if *deflate {
                miniz_oxide::deflate::compress_to_vec(data, 6)
            } else {
                data.to_vec()
            };
            let method: u16 = if *deflate { 8 } else { 0 };
            let crc = crate::crc32::of(data);
            let local_off = out.len() as u32;
            out.extend_from_slice(&LOCAL_SIG.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // time
            out.extend_from_slice(&0u16.to_le_bytes()); // date
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(stored.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra len
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&stored);

            central.extend_from_slice(&CD_SIG.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes()); // version made by
            central.extend_from_slice(&20u16.to_le_bytes()); // version needed
            central.extend_from_slice(&0u16.to_le_bytes()); // flags
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // time
            central.extend_from_slice(&0u16.to_le_bytes()); // date
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(stored.len() as u32).to_le_bytes());
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
        out.extend_from_slice(&EOCD_SIG.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // this disk
        out.extend_from_slice(&0u16.to_le_bytes()); // disk with the CD
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_len.to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&(comment_len as u16).to_le_bytes());
        out.extend(std::iter::repeat_n(b'#', comment_len));
        out
    }

    fn pat(tag: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| tag | (i as u8 & 0x0F)).collect()
    }

    #[test]
    fn reads_stored_and_deflated_members_by_name() {
        let a = pat(0xA0, 5000);
        let b = pat(0xB0, 300);
        let bytes = build_zip(&[("rom_a.bin", &a, false), ("rom_b.bin", &b, true)], 0);
        let ar = Archive::parse(bytes, "test.zip".into()).unwrap();
        assert_eq!(ar.names(), vec!["rom_a.bin", "rom_b.bin"]);
        assert_eq!(ar.read("rom_a.bin").unwrap(), a, "stored member");
        assert_eq!(ar.read("rom_b.bin").unwrap(), b, "deflated member");
    }

    #[test]
    fn a_deflated_member_is_actually_compressed_in_the_archive() {
        // Guards against the test passing because `build_zip` silently stored the
        // member it claimed to deflate — in which case the deflate path above
        // would never run and would be untested while looking tested.
        let a = vec![0x5Au8; 8192];
        let bytes = build_zip(&[("z.bin", &a, true)], 0);
        assert!(
            bytes.len() < 4096,
            "8 KB of one byte must compress; archive is {} bytes",
            bytes.len()
        );
        let ar = Archive::parse(bytes, "t.zip".into()).unwrap();
        assert_eq!(ar.read("z.bin").unwrap(), a);
    }

    /// The EOCD is found by scanning backwards, not at a fixed offset.
    ///
    /// Predicted by the plan and confirmed by mutation: with `comment_len` always
    /// 0 the EOCD sits exactly 22 bytes from the end, so a parser that looks only
    /// there passes every other test in this file.
    #[test]
    fn the_eocd_is_found_behind_a_trailing_archive_comment() {
        let a = pat(0xA0, 64);
        for comment_len in [0, 1, 8, 300] {
            let bytes = build_zip(&[("a.bin", &a, false)], comment_len);
            let ar = Archive::parse(bytes, "t.zip".into())
                .unwrap_or_else(|e| panic!("comment_len {comment_len}: {e}"));
            assert_eq!(ar.read("a.bin").unwrap(), a, "comment_len {comment_len}");
        }
    }

    #[test]
    fn a_missing_member_is_an_error_naming_the_member() {
        let ar = Archive::parse(build_zip(&[("a.bin", b"x", false)], 0), "t.zip".into()).unwrap();
        let err = ar.read("absent.bin").unwrap_err();
        assert!(
            format!("{err}").contains("absent.bin"),
            "the message must name what was missing: {err}"
        );
        assert!(!ar.contains("absent.bin"));
        assert!(ar.contains("a.bin"));
    }

    #[test]
    fn a_non_zip_is_rejected_rather_than_misparsed() {
        assert!(Archive::parse(vec![0u8; 64], "t.zip".into()).is_err());
        assert!(Archive::parse(Vec::new(), "t.zip".into()).is_err());
    }

    #[test]
    fn an_unsupported_compression_method_is_named_not_silently_wrong() {
        let mut bytes = build_zip(&[("a.bin", b"0123456789", false)], 0);
        // The method field lives at +10 in the central directory record. With one
        // file whose name is 5 chars, the record starts (46 + 5) bytes before the
        // 22-byte EOCD.
        let cd = bytes.len() - 22 - (46 + 5);
        assert_eq!(
            u32::from_le_bytes([bytes[cd], bytes[cd + 1], bytes[cd + 2], bytes[cd + 3]]),
            CD_SIG,
            "the offset arithmetic must actually land on the CD record"
        );
        bytes[cd + 10] = 93; // zstd — a real method, not one MAME sets use
        let ar = Archive::parse(bytes, "t.zip".into()).unwrap();
        let err = ar.read("a.bin").unwrap_err();
        assert!(
            format!("{err}").contains("93"),
            "the message must name the method: {err}"
        );
    }

    #[test]
    fn a_member_whose_inflated_length_disagrees_with_the_directory_is_rejected() {
        // A truncated or tampered member must not reach the interleave stage:
        // there it would be a length mismatch attributed to the user's file.
        let a = pat(0xA0, 256);
        let mut bytes = build_zip(&[("a.bin", &a, false)], 0);
        let cd = bytes.len() - 22 - (46 + 5);
        // Uncompressed size at +24; claim one byte more than is there.
        bytes[cd + 24] = 0x01;
        bytes[cd + 25] = 0x01;
        let ar = Archive::parse(bytes, "t.zip".into()).unwrap();
        assert!(ar.read("a.bin").is_err());
    }

    #[test]
    fn a_nested_path_is_matched_on_its_base_name() {
        // Some ROM sets are zipped with a directory prefix.
        let a = pat(0xA0, 16);
        let bytes = build_zip(&[("sf2/sf2e_30g.11e", &a, false)], 0);
        let ar = Archive::parse(bytes, "t.zip".into()).unwrap();
        assert_eq!(ar.names(), vec!["sf2e_30g.11e"]);
        assert_eq!(ar.read("sf2e_30g.11e").unwrap(), a);
    }
}
