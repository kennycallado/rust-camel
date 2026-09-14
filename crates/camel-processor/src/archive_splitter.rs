//! Shared archive-entry validation and policy helpers for archive splitters.
//!
//! The ZIP and TAR stream splitters share entry-path validation, the default
//! path-length cap, and the duplicate-name policy so security behavior cannot
//! drift between the two formats. Error text stays format-specific: callers
//! pass their format name (`kind`) and it is used verbatim in messages.

use std::path::Path;

use camel_api::CamelError;

use serde::Deserialize;

/// Shared default cap on validated entry path length.
pub(crate) const DEFAULT_MAX_PATH_LENGTH: usize = 4096;

/// Policy applied when an archive contains duplicate entry names.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DuplicatePolicy {
    /// Emit every duplicate; later entries get a deterministic index suffix.
    #[default]
    AllowWithIndex,
    /// Fail the split on the first duplicate name.
    Reject,
}

/// Validate an archive entry path before any use.
///
/// `kind` is the archive format name used verbatim in error messages (for
/// example `"ZIP"` or `"TAR"`), so each splitter keeps its own error text
/// while sharing the validation logic. The validator enforces, in order:
/// maximum length, NUL bytes, absolute paths, `..` traversal components,
/// backslashes, and Windows drive prefixes.
pub(crate) fn validate_entry_path(
    path: &str,
    max_length: usize,
    kind: &str,
) -> Result<String, CamelError> {
    if path.len() > max_length {
        return Err(CamelError::TypeConversionFailed(format!(
            "{kind} entry path exceeds max length: {} > {}",
            path.len(),
            max_length
        )));
    }

    if path.contains('\0') {
        return Err(CamelError::TypeConversionFailed(format!(
            "{kind} entry path contains NUL byte"
        )));
    }

    if Path::new(path).is_absolute() {
        return Err(CamelError::TypeConversionFailed(format!(
            "{kind} entry path is absolute: {path}"
        )));
    }

    for component in Path::new(path).components() {
        if let std::path::Component::ParentDir = component {
            return Err(CamelError::TypeConversionFailed(format!(
                "{kind} entry path contains '..' traversal: {path}"
            )));
        }
    }

    if path.contains('\\') {
        return Err(CamelError::TypeConversionFailed(format!(
            "{kind} entry path contains backslash: {path}"
        )));
    }

    if let Some(c) = path.chars().next()
        && c.is_ascii_alphabetic()
        && path.chars().nth(1) == Some(':')
    {
        return Err(CamelError::TypeConversionFailed(format!(
            "{kind} entry path contains Windows drive prefix: {path}"
        )));
    }

    Ok(path.to_string())
}

/// Deterministic [`DuplicatePolicy::AllowWithIndex`] name for the k-th
/// (k >= 1) occurrence of a duplicate entry name: the occurrence index is
/// inserted before the last extension (`a.tar` -> `a.1.tar`); names without a
/// stem or extension get a plain numeric suffix (`README` -> `README.1`).
///
/// Shared by the ZIP and TAR splitters so the index-mangling semantics cannot
/// drift between the formats. Callers must re-run [`validate_entry_path`] on
/// the result: the suffix grows the name, so the path-length cap stays
/// authoritative for indexed names too.
pub(crate) fn indexed_duplicate_name(name: &str, occurrence: usize) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
            format!("{stem}.{occurrence}.{ext}")
        }
        _ => format!("{name}.{occurrence}"),
    }
}

/// First collision-free indexed name for `base`, starting at `start` and
/// bumping the index until the candidate is absent from `emitted`.
///
/// A literal archive entry can occupy an indexed name first (`a.txt`,
/// `a.1.txt`, `a.txt` would otherwise derive `a.1.txt` twice), and two
/// different base names can converge on the same indexed name, so the
/// occurrence counter alone cannot guarantee uniqueness — only the set of
/// names already emitted can. Returns the chosen name and the occurrence
/// index it consumed, so callers can advance their per-base counter past
/// any skipped indexes.
///
/// Shared by the ZIP and TAR splitters so the collision semantics cannot
/// drift between the formats. Callers must re-run [`validate_entry_path`]
/// on the result: the suffix grows the name, so the path-length cap stays
/// authoritative for indexed names too.
pub(crate) fn next_free_indexed_name(
    base: &str,
    start: usize,
    emitted: &std::collections::HashSet<String>,
) -> (String, usize) {
    let mut occurrence = start.max(1);
    loop {
        let candidate = indexed_duplicate_name(base, occurrence);
        if !emitted.contains(&candidate) {
            return (candidate, occurrence);
        }
        occurrence += 1;
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    //! Test-only archive builders shared by the ZIP and TAR splitter tests.

    /// CRC-32 (IEEE) checksum, bit-wise so no lookup table is needed.
    pub(crate) fn crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// Build a minimal stored-method (uncompressed) ZIP with arbitrary entry
    /// names, including duplicates the `zip` writer refuses to produce.
    pub(crate) fn make_zip_raw(entries: &[(&str, &[u8])]) -> Vec<u8> {
        struct Central {
            name: String,
            crc: u32,
            size: u32,
            offset: u32,
        }

        let mut out = Vec::new();
        let mut centrals = Vec::with_capacity(entries.len());
        for (name, data) in entries {
            let offset = out.len() as u32;
            let crc = crc32(data);
            out.extend_from_slice(&0x0403_4b50_u32.to_le_bytes()); // local header
            out.extend_from_slice(&20u16.to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
            out.extend_from_slice(&0u16.to_le_bytes()); // mod time (1980-01-01)
            out.extend_from_slice(&0x21u16.to_le_bytes()); // mod date
            out.extend_from_slice(&crc.to_le_bytes());
            let size = data.len() as u32;
            out.extend_from_slice(&size.to_le_bytes()); // compressed
            out.extend_from_slice(&size.to_le_bytes()); // uncompressed
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra len
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            centrals.push(Central {
                name: (*name).to_string(),
                crc,
                size,
                offset,
            });
        }

        let cd_start = out.len() as u32;
        for central in &centrals {
            out.extend_from_slice(&0x0201_4b50_u32.to_le_bytes()); // central dir
            out.extend_from_slice(&20u16.to_le_bytes()); // version made by
            out.extend_from_slice(&20u16.to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&0u16.to_le_bytes()); // method
            out.extend_from_slice(&0u16.to_le_bytes()); // mod time
            out.extend_from_slice(&0x21u16.to_le_bytes()); // mod date
            out.extend_from_slice(&central.crc.to_le_bytes());
            out.extend_from_slice(&central.size.to_le_bytes());
            out.extend_from_slice(&central.size.to_le_bytes());
            out.extend_from_slice(&(central.name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra len
            out.extend_from_slice(&0u16.to_le_bytes()); // comment len
            out.extend_from_slice(&0u16.to_le_bytes()); // disk start
            out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            out.extend_from_slice(&central.offset.to_le_bytes());
            out.extend_from_slice(central.name.as_bytes());
        }
        let cd_size = out.len() as u32 - cd_start;

        out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes()); // EOCD
        out.extend_from_slice(&0u16.to_le_bytes()); // disk number
        out.extend_from_slice(&0u16.to_le_bytes()); // central-dir disk
        out.extend_from_slice(&(centrals.len() as u16).to_le_bytes());
        out.extend_from_slice(&(centrals.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_start.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        out
    }

    /// The index-mangling contract shared verbatim by the ZIP and TAR
    /// splitters: the occurrence index is inserted before the last
    /// extension, and names without a stem or extension get a plain numeric
    /// suffix. TAR exercises this end-to-end; ZIP shares the helper so the
    /// semantics cannot drift.
    #[test]
    fn indexed_duplicate_name_is_deterministic_across_name_shapes() {
        assert_eq!(super::indexed_duplicate_name("a.tar", 1), "a.1.tar");
        assert_eq!(super::indexed_duplicate_name("a.tar", 3), "a.3.tar");
        assert_eq!(super::indexed_duplicate_name("README", 1), "README.1");
        assert_eq!(
            super::indexed_duplicate_name("dir/file.bin", 2),
            "dir/file.2.bin"
        );
        // Dotfiles have an empty stem: plain numeric suffix, no reorder.
        assert_eq!(super::indexed_duplicate_name(".hidden", 1), ".hidden.1");
        // Trailing-dot names have an empty extension: plain numeric suffix.
        assert_eq!(super::indexed_duplicate_name("name.", 1), "name..1");
    }
}
