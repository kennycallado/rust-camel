//! Config-loader typed probe over interpolation provenance
//! (env-int-placeholder-typing), TOML arm.
//!
//! After a FAILED strict deserialize (`toml::Value::try_into::<CamelConfig>`)
//! of a resolved tree, candidate leaves (whole-scalar substituted
//! placeholders whose value is a clean i64) are coerced to integers in
//! cloned trees; candidate index subsets are tried ascending size
//! (lexicographic), with the strict parse as the oracle. First parse
//! success wins; otherwise the original first-pass error stands. Every
//! document that deserializes today is unaffected — probing runs only
//! after failure.
//!
//! Mirrors `crates/camel-dsl/src/env_int_probe.rs` (YAML arm); the
//! provenance-recording resolver lives in
//! [`crate::config::resolve_tree_with_provenance`].

use crate::config::CamelConfig;
use config::ConfigError;

/// One STRUCTURAL path segment into the TOML tree. Keys are stored verbatim:
/// a key that literally contains `.` or an index-like suffix navigates
/// unambiguously (the dotted/indexed rendering is display-only — see
/// [`segs_display`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigSeg {
    Key(String),
    Index(usize),
}

/// Structural paths of the token-carrying leaves recorded while resolving,
/// in document order (env-int-placeholder-typing). Feeds the typed probe at
/// the deserialize boundary.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProvenanceSet(Vec<Vec<ConfigSeg>>);

impl ProvenanceSet {
    /// Recorded paths, in document order.
    pub(crate) fn paths(&self) -> &[Vec<ConfigSeg>] {
        &self.0
    }

    /// Build a set from the walk's recorded paths (document order).
    pub(crate) fn from_paths(paths: Vec<Vec<ConfigSeg>>) -> Self {
        Self(paths)
    }
}

/// Navigate `segs` from `root` through tables and arrays; `None` when a
/// segment does not apply (wrong node kind, missing key or out-of-range
/// index).
fn leaf_mut<'a>(root: &'a mut toml::Value, segs: &[ConfigSeg]) -> Option<&'a mut toml::Value> {
    let mut node = root;
    for seg in segs {
        node = match (node, seg) {
            (toml::Value::Table(table), ConfigSeg::Key(key)) => table.get_mut(key)?,
            (toml::Value::Array(arr), ConfigSeg::Index(index)) => arr.get_mut(*index)?,
            _ => return None,
        };
    }
    Some(node)
}

/// Render segments in the walk's display format: keys join with `.`, array
/// indices render as `[i]` (e.g. `security.native.credentials[1].secret`).
/// NEVER use this for navigation — a key containing `.` is a single segment.
#[cfg(test)]
fn segs_display(segs: &[ConfigSeg]) -> String {
    let mut out = String::new();
    for seg in segs {
        match seg {
            ConfigSeg::Key(key) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(key);
            }
            ConfigSeg::Index(index) => {
                out.push_str(&format!("[{index}]"));
            }
        }
    }
    out
}

/// Maximum number of candidate leaves the subset search will enumerate
/// (2^8 - 1 = 255 in-memory parses worst case). Documents with more
/// candidates keep the first-pass error.
const MAX_PROBE_CANDIDATES: usize = 8;

/// Clean integer: the lexical form `-?(0|[1-9][0-9]*)` (no trim, no leading
/// zeros — floats, bools, and `1e3` are not clean), then an exact i64 parse
/// (overflow is not clean). Returns the integer the candidate coerces to.
// SYNC: this rule is mirrored by camel-dsl's `clean_integer`
// (crates/camel-dsl/src/env_int_probe.rs); crate purity forbids the
// dependency. The DSL arm also accepts u64 magnitude; this mirror is
// deliberately i64-only — u64-magnitude tokens stay on today's rejection
// path. Update the pair together.
fn clean_i64(s: &str) -> Option<i64> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    let lexically_clean = match digits.as_bytes() {
        // `0` alone — no leading zeros allowed.
        [b'0'] => true,
        // `[1-9]` followed by ASCII digits only.
        [first, rest @ ..] if (b'1'..=b'9').contains(first) => {
            rest.iter().all(|b| b.is_ascii_digit())
        }
        _ => false,
    };
    if !lexically_clean {
        return None;
    }
    s.parse::<i64>().ok()
}

/// Advance `indices` to the next combination of its size over `0..universe`
/// in lexicographic order; `false` when the enumeration is exhausted.
fn next_combination(indices: &mut [usize], universe: usize) -> bool {
    let size = indices.len();
    if size == 0 {
        return false;
    }
    for i in (0..size).rev() {
        let max = universe - (size - i);
        if indices[i] < max {
            indices[i] += 1;
            for j in i + 1..size {
                indices[j] = indices[j - 1] + 1;
            }
            return true;
        }
    }
    false
}

/// Strict deserialization with a typed probe fallback
/// (env-int-placeholder-typing). First pass is today's strict `try_into`;
/// on failure, provenance leaves whose (post-resolution) string value is a
/// clean i64 are coerced to `toml::Value::Integer` in cloned trees, candidate
/// index subsets tried ascending size (lexicographic). First parse success
/// wins; otherwise the original first-pass error stands. Documents that
/// deserialize today are unaffected — probing runs only after failure.
pub(crate) fn deserialize_with_probe(
    merged_tree: &toml::Value,
    provenance: &ProvenanceSet,
) -> Result<CamelConfig, ConfigError> {
    let first_attempt: Result<CamelConfig, _> = merged_tree.clone().try_into();
    let first_err = match first_attempt {
        Ok(config) => return Ok(config),
        Err(e) => ConfigError::Message(format!("Failed to deserialize merged config: {e}")),
    };

    // Candidate leaves, document order: provenance paths that resolve to a
    // String whose text `clean_i64` accepts.
    let mut probe_tree = merged_tree.clone();
    let mut candidates: Vec<(Vec<ConfigSeg>, i64)> = Vec::new();
    for segs in provenance.paths() {
        let coerced = match leaf_mut(&mut probe_tree, segs) {
            Some(toml::Value::String(s)) => clean_i64(s),
            _ => None,
        };
        if let Some(number) = coerced {
            candidates.push((segs.clone(), number));
        }
    }
    if candidates.is_empty() || candidates.len() > MAX_PROBE_CANDIDATES {
        return Err(first_err);
    }

    let universe = candidates.len();
    for size in 1..=universe {
        let mut indices: Vec<usize> = (0..size).collect();
        loop {
            let mut tree = merged_tree.clone();
            for &idx in &indices {
                let (segs, number) = &candidates[idx];
                if let Some(leaf) = leaf_mut(&mut tree, segs) {
                    *leaf = toml::Value::Integer(*number);
                }
            }
            let attempt: Result<CamelConfig, _> = tree.try_into();
            if let Ok(config) = attempt {
                return Ok(config);
            }
            if !next_combination(&mut indices, universe) {
                break;
            }
        }
    }
    Err(first_err)
}

/// Unit tests for the provenance-recording resolver variant
/// (env-int-placeholder-typing). Behavioral parity of the walk itself is
/// pinned by `tests/placeholder_walk.rs` and `tests/placeholder_e2e.rs`.
#[cfg(test)]
mod provenance_tests {
    use super::*;
    use crate::config::resolve_tree_with_provenance;

    fn tree(raw: &str) -> toml::Value {
        toml::from_str(raw).expect("test fixture must be valid TOML")
    }

    fn static_lookup(value: &str) -> impl Fn(&str) -> Option<String> + '_ {
        move |_| Some(value.to_string())
    }

    /// Only leaves carrying token text (`${env:` or `$$`) are recorded;
    /// plain string leaves never enter the set.
    #[test]
    fn provenance_set_records_token_leaves() {
        let lookup = static_lookup("1");
        let mut root = tree(
            r#"timeout_ms = "${env:T:-1}"
log_level = "debug"
"#,
        );
        let prov = resolve_tree_with_provenance(&mut root, &lookup).expect("resolve must succeed");
        assert_eq!(prov.paths().len(), 1, "exactly the token leaf is recorded");
        assert_eq!(
            prov.paths()[0],
            vec![ConfigSeg::Key("timeout_ms".to_string())]
        );
    }

    /// Structural segments keep keys that literally contain a dot
    /// unambiguous: navigation reaches the leaf, and the dotted form is
    /// display-only.
    #[test]
    fn provenance_segments_navigate_keys_with_dots() {
        let lookup = static_lookup("1");
        let mut root = tree(
            r#"[a]
"b.c" = "${env:X:-1}"
"#,
        );
        let prov = resolve_tree_with_provenance(&mut root, &lookup).expect("resolve must succeed");
        assert_eq!(prov.paths().len(), 1);
        assert_eq!(
            prov.paths()[0],
            vec![
                ConfigSeg::Key("a".to_string()),
                ConfigSeg::Key("b.c".to_string()),
            ]
        );
        let leaf = leaf_mut(&mut root, &prov.paths()[0]).expect("segments must navigate");
        assert_eq!(leaf.as_str(), Some("1"));
        assert_eq!(segs_display(&prov.paths()[0]), "a.b.c");
    }
}
