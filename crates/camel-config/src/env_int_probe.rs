//! Config-loader typed probe over interpolation provenance
//! (env-int-placeholder-typing), TOML arm.
//!
//! After a FAILED strict deserialize (`toml::Value::try_into::<CamelConfig>`)
//! of a resolved tree, candidate leaves (whole-scalar substituted
//! placeholders whose value is a clean i64 or a clean boolean) are coerced
//! to typed values in cloned trees; candidate index subsets are tried
//! ascending size (lexicographic), with the strict parse as the oracle. First parse
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
/// deserialize boundaries.
///
/// Paths are structural — internal segments, never navigated by callers.
/// `ConfigSeg` stays crate-private, so the type is opaque outside
/// `camel-config`; callers hand a set back to the probe, they never inspect
/// it.
///
/// Stale-provenance rule: a set describes values the loader resolved. A
/// caller that edits [`crate::ComponentsConfig::raw`] after load must rebuild
/// the config (default-construct `ComponentsConfig`, then assign the edited
/// map) so no stale path coerces a token-free value.
#[derive(Debug, Clone, Default)]
pub struct ProvenanceSet(Vec<Vec<ConfigSeg>>);

impl ProvenanceSet {
    /// Recorded paths, in document order.
    pub(crate) fn paths(&self) -> &[Vec<ConfigSeg>] {
        &self.0
    }

    /// Build a set from the walk's recorded paths (document order).
    pub(crate) fn from_paths(paths: Vec<Vec<ConfigSeg>>) -> Self {
        Self(paths)
    }

    /// Split the whole-tree set into per-component-block sets. Only paths
    /// under `components.<block>` participate; their remaining segments are
    /// stored relative to the block, in traversal order. Paths that resolve
    /// to the block node itself carry no leaf and are skipped.
    pub(crate) fn split_by_component(&self) -> std::collections::HashMap<String, ProvenanceSet> {
        let mut split: std::collections::HashMap<String, ProvenanceSet> =
            std::collections::HashMap::new();
        for segs in self.paths() {
            let [ConfigSeg::Key(root), ConfigSeg::Key(block), rest @ ..] = segs.as_slice() else {
                continue;
            };
            if root != "components" || rest.is_empty() {
                continue;
            }
            split
                .entry(block.clone())
                .or_default()
                .0
                .push(rest.to_vec());
        }
        split
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

/// Clean boolean: exactly the lowercase literals `true` and `false`
/// (byte-exact match — no trim, no case folding, no `yes`/`no`, no `1`/`0`).
/// Returns the boolean the candidate coerces to.
// SYNC: this rule is mirrored by camel-dsl's `clean_boolean`
// (crates/camel-dsl/src/env_int_probe.rs) and camel-lint's rschema
// `clean_boolean` (crates/camel-lint/src/rules/rschema.rs); crate purity
// forbids the dependencies. Update all three together.
fn clean_bool(s: &str) -> Option<bool> {
    match s {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
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
/// (env-int-placeholder-typing), generic over the target type and its parse
/// step.
///
/// The first pass (`parse(block.clone())`) is the caller's strict oracle:
/// its success is returned immediately and no probing runs. Only after a
/// failure are provenance leaves whose (post-resolution) string value is a
/// clean i64 or a clean boolean coerced to `toml::Value::Integer` /
/// `toml::Value::Boolean` in cloned trees, candidate index subsets tried
/// ascending size (lexicographic), each through `parse`. The first parse
/// success wins; otherwise the first-pass error is returned verbatim.
///
/// At most `MAX_PROBE_CANDIDATES` (8) candidates are enumerated, so the
/// worst case is 255 subset retries plus the first pass.
pub fn deserialize_block_with_probe<T, E>(
    block: &toml::Value,
    provenance: &ProvenanceSet,
    parse: impl Fn(toml::Value) -> Result<T, E>,
) -> Result<T, E> {
    let first_err = match parse(block.clone()) {
        Ok(value) => return Ok(value),
        Err(e) => e,
    };

    // Candidate leaves, document order: provenance paths that resolve to a
    // String whose text `clean_i64` or `clean_bool` accepts. One clone for
    // the whole scan, not one per path.
    let mut probe_tree = block.clone();
    let mut candidates: Vec<(Vec<ConfigSeg>, toml::Value)> = Vec::new();
    for segs in provenance.paths() {
        let coerced = match leaf_mut(&mut probe_tree, segs) {
            Some(toml::Value::String(s)) => clean_i64(s)
                .map(toml::Value::Integer)
                .or_else(|| clean_bool(s).map(toml::Value::Boolean)),
            _ => None,
        };
        if let Some(value) = coerced {
            candidates.push((segs.clone(), value));
        }
    }
    if candidates.is_empty() || candidates.len() > MAX_PROBE_CANDIDATES {
        return Err(first_err);
    }

    let universe = candidates.len();
    for size in 1..=universe {
        let mut indices: Vec<usize> = (0..size).collect();
        loop {
            let mut tree = block.clone();
            for &idx in &indices {
                let (segs, value) = &candidates[idx];
                if let Some(leaf) = leaf_mut(&mut tree, segs) {
                    *leaf = value.clone();
                }
            }
            if let Ok(value) = parse(tree) {
                return Ok(value);
            }
            if !next_combination(&mut indices, universe) {
                break;
            }
        }
    }
    Err(first_err)
}

/// Whole-tree wrapper over [`deserialize_block_with_probe`]: the root-field
/// TOML arm of (env-int-placeholder-typing). Strict `try_into` is the
/// oracle and keeps its original error text.
pub(crate) fn deserialize_with_probe(
    merged_tree: &toml::Value,
    provenance: &ProvenanceSet,
) -> Result<CamelConfig, ConfigError> {
    deserialize_block_with_probe(merged_tree, provenance, |tree| {
        tree.try_into::<CamelConfig>()
            .map_err(|e| ConfigError::Message(format!("Failed to deserialize merged config: {e}")))
    })
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

    /// `split_by_component` partitions the whole-tree set into per-block
    /// sets, keeping block-relative paths in traversal order and dropping
    /// paths that are not under `components.<block>`.
    #[test]
    fn components_provenance_split_by_block() {
        let key = |name: &str| ConfigSeg::Key(name.to_string());
        let prov = ProvenanceSet::from_paths(vec![
            vec![key("components"), key("redis"), key("port")],
            vec![
                key("components"),
                key("redis"),
                key("sentinel"),
                key("port"),
            ],
            vec![key("components"), key("ws"), key("port")],
            vec![key("timeout_ms")],
        ]);

        let split = prov.split_by_component();
        assert_eq!(
            split.get("redis").expect("redis block").paths(),
            vec![vec![key("port")], vec![key("sentinel"), key("port")]],
            "redis paths stay block-relative and in traversal order"
        );
        assert_eq!(
            split.get("ws").expect("ws block").paths(),
            vec![vec![key("port")]]
        );
        assert_eq!(split.len(), 2, "non-component paths are dropped");
    }
}

/// Unit tests for the generic block-level probe helper
/// (`deserialize_block_with_probe`) introduced by `envtyped-components`.
#[cfg(test)]
mod probe_helper_tests {
    use super::*;
    use serde::Deserialize;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    fn block(raw: &str) -> toml::Value {
        toml::from_str(raw).expect("test fixture must be valid TOML")
    }

    fn path(key: &str) -> Vec<ConfigSeg> {
        vec![ConfigSeg::Key(key.to_string())]
    }

    /// Counting parse oracle: `Err("first")` on the first call, `Err("retry")`
    /// afterwards. The shared counter proves how many parses ran.
    fn counting_parse() -> (
        impl Fn(toml::Value) -> Result<(), &'static str>,
        Arc<AtomicUsize>,
    ) {
        let counter = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&counter);
        let parse = move |_tree: toml::Value| -> Result<(), &'static str> {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Err("first")
            } else {
                Err("retry")
            }
        };
        (parse, counter)
    }

    fn nine_candidate_block() -> toml::Value {
        block(
            r#"f0 = "1"
f1 = "2"
f2 = "3"
f3 = "4"
f4 = "5"
f5 = "6"
f6 = "7"
f7 = "8"
f8 = "9"
"#,
        )
    }

    fn eight_candidate_block() -> toml::Value {
        block(
            r#"f0 = "1"
f1 = "2"
f2 = "3"
f3 = "4"
f4 = "5"
f5 = "6"
f6 = "7"
f7 = "8"
"#,
        )
    }

    fn candidate_paths(count: usize) -> ProvenanceSet {
        ProvenanceSet::from_paths((0..count).map(|i| path(&format!("f{i}"))).collect())
    }

    #[derive(Deserialize)]
    struct SName {
        name: String,
    }

    /// A numeric-looking STRING leaf whose target field is `String` parses on
    /// the first pass; no probe runs and the text stays a string.
    #[test]
    fn helper_first_pass_success_keeps_numeric_string() {
        let block = block(r#"name = "1234""#);
        let prov = ProvenanceSet::from_paths(vec![path("name")]);
        let parsed: SName =
            deserialize_block_with_probe(&block, &prov, |tree| tree.try_into::<SName>())
                .expect("first pass must succeed");
        assert_eq!(parsed.name, "1234");
    }

    #[derive(Deserialize)]
    struct SFault {
        fault_status: Option<u16>,
    }

    /// The spec's Option-typed component scenario: a resolved placeholder
    /// string coerces to the integer the target accepts.
    #[test]
    fn helper_coerces_option_u16_leaf() {
        let block = block(r#"fault_status = "503""#);
        let prov = ProvenanceSet::from_paths(vec![path("fault_status")]);
        let parsed: SFault =
            deserialize_block_with_probe(&block, &prov, |tree| tree.try_into::<SFault>())
                .expect("probe must coerce the integer leaf");
        assert_eq!(parsed.fault_status, Some(503));
    }

    #[derive(Deserialize)]
    struct SPort {
        port: u16,
    }

    /// A plain (non-Option) integer field coerces too.
    #[test]
    fn helper_integer_leaf_coerces() {
        let block = block(r#"port = "7100""#);
        let prov = ProvenanceSet::from_paths(vec![path("port")]);
        let parsed: SPort =
            deserialize_block_with_probe(&block, &prov, |tree| tree.try_into::<SPort>())
                .expect("probe must coerce the integer leaf");
        assert_eq!(parsed.port, 7100u16);
    }

    #[derive(Deserialize)]
    struct SFlag {
        flag: Option<bool>,
    }

    /// A clean boolean placeholder value coerces to a boolean.
    #[test]
    fn helper_boolean_leaf_coerces() {
        let block = block(r#"flag = "true""#);
        let prov = ProvenanceSet::from_paths(vec![path("flag")]);
        let parsed: SFlag =
            deserialize_block_with_probe(&block, &prov, |tree| tree.try_into::<SFlag>())
                .expect("probe must coerce the boolean leaf");
        assert_eq!(parsed.flag, Some(true));
    }

    /// A literal quoted numeric carries no token, so it has no provenance
    /// path and is never a candidate: the parse error stands.
    #[test]
    fn helper_literal_quoted_numeric_never_coerces() {
        let block = block(r#"port = "8080""#);
        let prov = ProvenanceSet::default();
        let result = deserialize_block_with_probe(&block, &prov, |tree| {
            tree.try_into::<std::collections::HashMap<String, u16>>()
        });
        let err = result.expect_err("no candidate without provenance");
        assert!(
            err.to_string().contains("8080"),
            "the parse error must stand, got: {err}"
        );
    }

    /// Nine candidates exceed the cap: no subset search runs and the first
    /// pass error is kept verbatim.
    #[test]
    fn helper_cap_nine_candidates_skips_search() {
        let block = nine_candidate_block();
        let prov = candidate_paths(9);
        let (parse, counter) = counting_parse();
        let result = deserialize_block_with_probe(&block, &prov, parse);
        assert_eq!(result, Err("first"), "the first-pass error stands");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "cap exceeded before any subset retry"
        );
    }

    /// Eight candidates are enumerated exhaustively: the first pass plus all
    /// `2^8 - 1 = 255` subsets.
    #[test]
    fn helper_cap_eight_candidates_searches_all_subsets() {
        let block = eight_candidate_block();
        let prov = candidate_paths(8);
        let (parse, counter) = counting_parse();
        let result = deserialize_block_with_probe(&block, &prov, parse);
        assert_eq!(result, Err("first"), "the first-pass error stands");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            256,
            "first pass plus all 255 subsets"
        );
    }

    /// On no success the ORIGINAL error is returned, never the retry error:
    /// one candidate means exactly one subset retry after the first pass.
    #[test]
    fn helper_first_error_returned_verbatim() {
        let block = block(r#"port = "1""#);
        let prov = ProvenanceSet::from_paths(vec![path("port")]);
        let (parse, counter) = counting_parse();
        let result = deserialize_block_with_probe(&block, &prov, parse);
        assert_eq!(result, Err("first"), "the first-pass error stands");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "first pass plus the single-subset retry"
        );
    }
}
