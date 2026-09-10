//! `${env:}` interpolation for route sources (rc-ayke).
//!
//! Two paths share one string scanner (the `interpolate_string` core behind
//! [`interpolate_env_with`] and `interpolate_env_tree`):
//!
//! - **Parse-tree walk** (`interpolate_env_tree`): the canonical YAML
//!   path. Interpolation runs on the scalars of the parsed tree, so YAML
//!   comments are never interpolated — a placeholder inside a comment
//!   (e.g. a commented-out line referencing a removed var) cannot fail
//!   resolution.
//! - **Legacy whole-text splice** ([`interpolate_env_with`]): a raw pass
//!   over the unparsed text, kept as the fallback for documents the YAML
//!   shim cannot parse.
//!
//! # Typing semantics (design decision, camel-config precedent)
//!
//! An interpolated leaf that resolves to numeric- or boolean-looking text
//! KEEPS STRING typing at the interpolation seam: the YAML `Value` tree
//! cannot preserve plain-scalar style through a round-trip, so the leaf
//! stays a string here. Integer positions are coerced later by the
//! loader-layer typed probe (env_int_probe), which this module feeds via
//! provenance (`interpolate_env_tree_with_provenance`). Consumers that
//! need numbers inside URI strings still compose them there.
//!
//! Interpolated mapping keys that collide after interpolation collapse
//! last-wins, matching the loader's own duplicate-key behavior (parity
//! with the raw-splice outcome class).

//! SYNC: the whole-text splice arm ([`interpolate_env_with`]) is mirrored by
//! camel-lint::env_interpolation (rc-93wct); crate purity forbids the dependency.
//! Update both together.
use noyalib::compat::serde_yaml as serde_yml;
use regex::Regex;
use std::env;
use std::sync::OnceLock;

static ENV_RE: OnceLock<Regex> = OnceLock::new();

fn env_regex() -> &'static Regex {
    // Escape alternatives exist so `$${env:X}` never falls through to plain
    // resolution; the full escape form is listed before bare `$$` so it is
    // consumed atomically.
    ENV_RE.get_or_init(|| {
        Regex::new(r"(\$\$\{env:[^}]*\})|(\$\$)|(\$\{env:([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\})")
            .unwrap() // allow-unwrap
    })
}

/// Replace line/paragraph-break chars with SPACE to prevent YAML
/// structural injection when values are spliced into raw YAML before parse.
///
/// Covers: `\n`, `\r`, `\0`, `\u{2028}`, `\u{2029}`.
///
/// Does NOT strip flow indicators (`[ ] { } ,`), plain-scalar colon
/// (`: `), comment truncation (` #`), or tag/anchor chars (`! & * %`).
/// These can alter YAML within a single line but cannot inject new
/// keys/structure (that requires newlines). Values are inserted into
/// already-trusted template positions, not arbitrary escaping.
fn sanitize_env_value(val: &str) -> String {
    val.chars()
        .map(|c| {
            if matches!(c, '\n' | '\r' | '\0' | '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// Interpolates `${env:VAR_NAME}` placeholders in the source string.
///
/// `$${env:VAR_NAME}` yields the literal text `${env:VAR_NAME}` (escape),
/// and a standalone `$$` yields a single `$`.
///
/// Returns `Err(var_name)` if any referenced variable is not set.
pub fn interpolate_env(src: &str) -> Result<String, String> {
    interpolate_env_with(src, &|name| env::var(name).ok())
}

/// Lookup-injectable variant of [`interpolate_env`]: `${env:VAR_NAME}`
/// placeholders resolve through `lookup` instead of the process
/// environment. Same escape forms, sanitization, and error shape.
///
/// Returns `Err(var_name)` if any referenced variable is not resolved.
pub fn interpolate_env_with(
    src: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    interpolate_string(src, lookup)
}

/// Canonical interpolation strategy for a YAML route source (rc-93wct):
/// parse-tree interpolation first (`interpolate_env_tree` — comments are
/// never interpolated and a substituted leaf keeps STRING typing at the
/// interpolation seam; integer positions are coerced later by the
/// loader-layer typed probe, env_int_probe, which this module feeds via
/// provenance), falling back to the legacy whole-text splice
/// ([`interpolate_env_with`]) when the document does not survive the YAML
/// round-trip. An unresolved variable from either path surfaces as
/// `Err(var_name)`.
///
/// Both the discovery YAML arm and `load_from_file_with_env` route through
/// this seam so the loader cannot drift from discovery semantics.
pub fn interpolate_yaml_source(
    raw: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    interpolate_yaml_source_with_provenance(raw, lookup).map(|(content, _)| content)
}

/// [`interpolate_yaml_source`] paired with provenance: on the tree-walk
/// path it additionally returns the structural paths (see
/// [`ProvenancePath`]) of every leaf whose authored scalar was exactly one
/// whole-scalar `${env:...}` token (see [`is_whole_scalar_env_token`]).
/// When the legacy whole-text splice runs (the document did not survive
/// the YAML round-trip) the provenance is `None` — the fallback carries no
/// provenance and never feeds the typed probe. The `Err(var_name)` surface
/// and the fallback trigger are identical to [`interpolate_yaml_source`].
pub(crate) fn interpolate_yaml_source_with_provenance(
    raw: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(String, Option<Vec<ProvenancePath>>), String> {
    match interpolate_env_tree_with_provenance(raw, lookup) {
        Ok((content, paths)) => Ok((content, Some(paths))),
        Err(TreeInterpolateError::Unresolved(var_name)) => Err(var_name),
        Err(TreeInterpolateError::Fallback) => {
            interpolate_env_with(raw, lookup).map(|content| (content, None))
        }
    }
}

/// Shared string scanner for both interpolation paths (legacy whole-text
/// and parse-tree walk). Grammar: `${env:X}`, `${env:X:-default}`,
/// `$${env:X}` and `$$` escapes; `Err(var_name)` on an unresolved var.
fn interpolate_string(s: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let re = env_regex();
    let mut error: Option<String> = None;

    let result = re.replace_all(s, |caps: &regex::Captures| {
        if error.is_some() {
            return String::new();
        }
        // `$${env:...}` escape: emit the literal placeholder text (strip one `$`).
        if let Some(escaped) = caps.get(1) {
            return escaped.as_str()[1..].to_string();
        }
        // Standalone `$$` escape: emit a single `$`.
        if caps.get(2).is_some() {
            return "$".to_string();
        }
        let var_name = &caps[4];
        let default_value = caps.get(5).map(|m| m.as_str());
        match lookup(var_name) {
            Some(val) => sanitize_env_value(&val),
            None => {
                if let Some(default) = default_value {
                    sanitize_env_value(default)
                } else {
                    error = Some(var_name.to_string());
                    String::new()
                }
            }
        }
    });

    if let Some(missing) = error {
        return Err(missing);
    }

    Ok(result.into_owned())
}

/// Error surface of the parse-tree interpolation walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TreeInterpolateError {
    /// A `${env:NAME}` placeholder did not resolve (no value, no default).
    Unresolved(String),
    /// The document did not survive the YAML parse/serialize round-trip;
    /// callers fall back to legacy whole-text interpolation.
    Fallback,
}

/// One structural segment of a substituted leaf's provenance path: a
/// mapping key or a sequence index. Structural segments (not a joined
/// string) stay safe for keys containing `.` or `[i]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProvenanceSeg {
    /// Mapping key leading to the walked node (the interpolated key text).
    Key(String),
    /// Sequence index leading to the walked node.
    Index(usize),
}

/// Structural path from the document root to a substituted leaf, in
/// document order. Collected by `interpolate_env_tree_with_provenance`
/// and consumed by the loader-layer typed probe (env_int_probe).
pub(crate) type ProvenancePath = Vec<ProvenanceSeg>;

/// Parse-tree `${env:}` interpolation for YAML documents.
///
/// Parses `raw` with the crate's canonical YAML shim (`noyalib::compat::
/// serde_yaml`, the same alias `parse_yaml` uses), applies the shared
/// scanner to scalars only (string mapping keys included), and
/// re-serializes with the same shim. Comments are not part of the tree,
/// so a placeholder inside a comment never fails resolution.
///
/// Scalars whose text contains no `${` or `$$` token pass through
/// untouched, minimizing round-trip drift. An unresolved var propagates
/// as [`TreeInterpolateError::Unresolved`]. Numeric/boolean-looking
/// results keep STRING typing at the interpolation seam; integer
/// positions are coerced later by the loader-layer typed probe
/// (env_int_probe), which this module feeds via provenance (see the
/// module docs).
#[cfg_attr(not(test), allow(dead_code))] // delegate kept for pins; the probe seam (Phase 1) consumes the provenance variant
pub(crate) fn interpolate_env_tree(
    raw: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, TreeInterpolateError> {
    interpolate_env_tree_with_provenance(raw, lookup).map(|(doc, _)| doc)
}

/// [`interpolate_env_tree`] paired with provenance: same parse/walk/
/// serialize, but it additionally returns the structural paths of every
/// leaf whose authored scalar was exactly one whole-scalar `${env:...}`
/// token (see [`is_whole_scalar_env_token`]) at the moment it substituted,
/// in document order. Mapping keys and leaves with embedded tokens are
/// never recorded; escaped forms (`$${env:...}`, `$$`) are literal text,
/// not substitutions, and are never recorded. The substituted leaf itself
/// keeps STRING typing at this seam — provenance feeds the loader-layer
/// typed probe (env_int_probe), which does any coercion.
pub(crate) fn interpolate_env_tree_with_provenance(
    raw: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(String, Vec<ProvenancePath>), TreeInterpolateError> {
    let mut root: serde_yml::Value =
        serde_yml::from_str(raw).map_err(|_| TreeInterpolateError::Fallback)?;
    let mut paths = Vec::new();
    interpolate_value(&mut root, lookup, &mut Vec::new(), &mut paths)?;
    let doc = serde_yml::to_string(&root).map_err(|_| TreeInterpolateError::Fallback)?;
    Ok((doc, paths))
}

/// Whether a scalar's text carries any placeholder or escape token.
fn has_env_token(s: &str) -> bool {
    s.contains("${") || s.contains("$$")
}

/// Whether the ENTIRE scalar is exactly one unescaped `${env:NAME}` or
/// `${env:NAME:-default}` token — the plain token form of `env_regex()`
/// spanning the whole string. Escaped forms (`$${env:...}`, `$$`) are
/// literal text, not substitutions, and never match; a token embedded in
/// larger text (`x${env:A}`, `${env:A}y`) does not either.
pub(crate) fn is_whole_scalar_env_token(s: &str) -> bool {
    env_regex()
        .captures(s)
        .and_then(|caps| caps.get(3))
        .is_some_and(|m| m.start() == 0 && m.end() == s.len())
}

/// Recursive tree walk applying `interpolate_string` to scalars that carry
/// a placeholder or escape token; all other nodes pass through untouched.
/// `path` is the structural path of the current node; every whole-scalar
/// token leaf substituted along the way is appended to `paths` (in
/// document order) at the moment it substitutes. Mapping keys are
/// interpolated but never recorded.
fn interpolate_value(
    value: &mut serde_yml::Value,
    lookup: &dyn Fn(&str) -> Option<String>,
    path: &mut Vec<ProvenanceSeg>,
    paths: &mut Vec<ProvenancePath>,
) -> Result<(), TreeInterpolateError> {
    match value {
        serde_yml::Value::String(s) => {
            if has_env_token(s) {
                let whole_token = is_whole_scalar_env_token(s);
                *s = interpolate_string(s, lookup).map_err(TreeInterpolateError::Unresolved)?;
                if whole_token {
                    paths.push(path.clone());
                }
            }
            Ok(())
        }
        serde_yml::Value::Sequence(seq) => {
            for (index, item) in seq.iter_mut().enumerate() {
                path.push(ProvenanceSeg::Index(index));
                interpolate_value(item, lookup, path, paths)?;
                path.pop();
            }
            Ok(())
        }
        serde_yml::Value::Mapping(map) => {
            // The shim's Mapping keys are strings — interpolate them too.
            // Keys are not mutable in place (`iter_mut` yields `&String`),
            // so rebuilt entries replace the originals in order. Keys are
            // never provenance: only whole-scalar token leaves feed the
            // typed probe.
            let mut rebuilt = serde_yml::Mapping::new();
            for (key, val) in map.iter() {
                let mut key = key.clone();
                if has_env_token(&key) {
                    key = interpolate_string(&key, lookup)
                        .map_err(TreeInterpolateError::Unresolved)?;
                }
                path.push(ProvenanceSeg::Key(key.clone()));
                let mut val = val.clone();
                interpolate_value(&mut val, lookup, path, paths)?;
                path.pop();
                rebuilt.insert(key, val);
            }
            *map = rebuilt;
            Ok(())
        }
        // Tagged nodes are opaque through the shim (the inner value is not
        // reachable) — a document carrying one falls back to the legacy
        // whole-text splice so placeholders inside tagged nodes keep
        // interpolating exactly as before the tree walk existed.
        serde_yml::Value::Tagged(_) => Err(TreeInterpolateError::Fallback),
        // Null/Bool/Number carry no token-bearing text.
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noyalib::compat::serde_yaml as serde_yml;

    #[test]
    fn passthrough_no_placeholders() {
        let result = interpolate_env("hello: world").unwrap();
        assert_eq!(result, "hello: world");
    }

    #[test]
    fn single_var_substitution() {
        unsafe { env::set_var("TEST_DSL_HOST", "localhost") };
        let result = interpolate_env("uri: ${env:TEST_DSL_HOST}/path").unwrap();
        assert_eq!(result, "uri: localhost/path");
        unsafe { env::remove_var("TEST_DSL_HOST") };
    }

    #[test]
    fn multiple_vars_all_set() {
        unsafe { env::set_var("TEST_DSL_USER", "admin") };
        unsafe { env::set_var("TEST_DSL_PASS", "secret") };
        let result = interpolate_env("${env:TEST_DSL_USER}:${env:TEST_DSL_PASS}").unwrap();
        assert_eq!(result, "admin:secret");
        unsafe { env::remove_var("TEST_DSL_USER") };
        unsafe { env::remove_var("TEST_DSL_PASS") };
    }

    #[test]
    fn missing_var_returns_err_with_name() {
        unsafe { env::remove_var("TEST_DSL_MISSING") };
        let err = interpolate_env("uri: ${env:TEST_DSL_MISSING}").unwrap_err();
        assert_eq!(err, "TEST_DSL_MISSING");
    }

    #[test]
    fn multiple_vars_first_missing_fails_fast() {
        unsafe { env::remove_var("TEST_DSL_FIRST") };
        unsafe { env::set_var("TEST_DSL_SECOND", "ok") };
        let err = interpolate_env("${env:TEST_DSL_FIRST} ${env:TEST_DSL_SECOND}").unwrap_err();
        assert_eq!(err, "TEST_DSL_FIRST");
        unsafe { env::remove_var("TEST_DSL_SECOND") };
    }

    #[test]
    fn default_value_used_when_var_missing() {
        unsafe { env::remove_var("TEST_DSL_DEFAULT_VAR") };
        let result = interpolate_env("uri: ${env:TEST_DSL_DEFAULT_VAR:-localhost}/path").unwrap();
        assert_eq!(result, "uri: localhost/path");
    }

    #[test]
    fn default_value_not_used_when_var_set() {
        unsafe { env::set_var("TEST_DSL_OVERRIDE", "production") };
        let result = interpolate_env("uri: ${env:TEST_DSL_OVERRIDE:-localhost}/path").unwrap();
        assert_eq!(result, "uri: production/path");
        unsafe { env::remove_var("TEST_DSL_OVERRIDE") };
    }

    #[test]
    fn empty_default_is_valid() {
        unsafe { env::remove_var("TEST_DSL_EMPTY_DEFAULT") };
        let result = interpolate_env("uri: ${env:TEST_DSL_EMPTY_DEFAULT:-}/path").unwrap();
        assert_eq!(result, "uri: /path");
    }

    #[test]
    fn no_default_still_errors() {
        unsafe { env::remove_var("TEST_DSL_NO_DEFAULT") };
        let err = interpolate_env("uri: ${env:TEST_DSL_NO_DEFAULT}/path").unwrap_err();
        assert_eq!(err, "TEST_DSL_NO_DEFAULT");
    }

    #[test]
    fn default_with_special_chars() {
        unsafe { env::remove_var("TEST_DSL_SPECIAL") };
        let result =
            interpolate_env("uri: ${env:TEST_DSL_SPECIAL:-host.example.com:9092}/path").unwrap();
        assert_eq!(result, "uri: host.example.com:9092/path");
    }

    #[test]
    fn interpolates_value_with_newline_replaced() {
        unsafe { env::set_var("TEST_DSL_INJECTION", "safe_value\ninjected_key: malicious") };
        let result = interpolate_env("uri: ${env:TEST_DSL_INJECTION}/path").unwrap();
        unsafe { env::remove_var("TEST_DSL_INJECTION") };
        assert!(
            !result.contains('\n'),
            "interpolated value must not contain newlines"
        );
        // Newline replaced with SPACE (not deleted)
        assert!(
            result.contains("safe_value injected_key"),
            "newline must be replaced with space, not removed"
        );
    }

    #[test]
    fn interpolates_value_replaces_all_control_chars() {
        // \0 cannot be set via env::set_var (C string terminator).
        // Test sanitize_env_value directly for all 5 chars including \0.
        let input = "a\rb\nb\0c\u{2028}d\u{2029}e";
        let sanitized = sanitize_env_value(input);
        assert!(!sanitized.contains('\r'), "CR must be replaced");
        assert!(!sanitized.contains('\n'), "LF must be replaced");
        assert!(!sanitized.contains('\0'), "NUL must be replaced");
        assert!(!sanitized.contains('\u{2028}'), "LS must be replaced");
        assert!(!sanitized.contains('\u{2029}'), "PS must be replaced");
        assert!(
            sanitized.contains("a b b c d e"),
            "all control chars must be replaced with space"
        );
    }

    #[test]
    fn mixed_defaults_and_non_defaults() {
        unsafe { env::remove_var("TEST_DSL_MIX_A") };
        unsafe { env::set_var("TEST_DSL_MIX_B", "actual") };
        let result =
            interpolate_env("${env:TEST_DSL_MIX_A:-fallback}:${env:TEST_DSL_MIX_B}").unwrap();
        assert_eq!(result, "fallback:actual");
        unsafe { env::remove_var("TEST_DSL_MIX_B") };
    }

    #[test]
    fn escape_full_form_yields_literal() {
        unsafe { env::set_var("RUST_CAMEL_TEST_ESC_A", "real-val") };
        let result = interpolate_env("$${env:RUST_CAMEL_TEST_ESC_A}").unwrap();
        assert_eq!(result, "${env:RUST_CAMEL_TEST_ESC_A}");
        unsafe { env::remove_var("RUST_CAMEL_TEST_ESC_A") };
    }

    #[test]
    fn escape_standalone_dollar_yields_single() {
        let result = interpolate_env("a$$b").unwrap();
        assert_eq!(result, "a$b");
        // Critical end-of-string case: standalone `$$` at end of string.
        let result = interpolate_env("ab$$").unwrap();
        assert_eq!(result, "ab$");
    }

    #[test]
    fn escape_then_placeholder_both_resolve() {
        unsafe { env::set_var("RUST_CAMEL_TEST_ESC_B", "val-b") };
        let result = interpolate_env("$${env:LIT} and ${env:RUST_CAMEL_TEST_ESC_B}").unwrap();
        assert_eq!(result, "${env:LIT} and val-b");
        unsafe { env::remove_var("RUST_CAMEL_TEST_ESC_B") };
    }

    #[test]
    fn comment_placeholder_does_not_fail() {
        let input =
            "# TODO re-enable ${env:MISSING}\nroutes:\n  - id: r1\n    from: direct:start\n";
        let out = interpolate_env_tree(input, &|_| None)
            .expect("placeholder inside a comment must not fail resolution");
        assert!(!out.contains("TODO"), "comment must be dropped, got: {out}");
        assert!(
            !out.contains("${env:MISSING}"),
            "comment placeholder must not leak, got: {out}"
        );
        assert!(out.contains("r1"), "body must be kept, got: {out}");
        assert!(
            out.contains("direct:start"),
            "body must be kept, got: {out}"
        );
    }

    #[test]
    fn quoted_hash_survives_interpolation() {
        let input = "text: \"a # b ${env:X}\"\n";
        let out = interpolate_env_tree(input, &|name| (name == "X").then(|| "ok".to_string()))
            .expect("resolvable placeholder must interpolate");
        let parsed: serde_yml::Value = serde_yml::from_str(&out).expect("output must re-parse");
        assert_eq!(
            parsed.get("text").and_then(serde_yml::Value::as_str),
            Some("a # b ok"),
            "hash must survive and X interpolate, got: {out}"
        );
    }

    #[test]
    fn block_scalar_interpolates_as_value() {
        let input = "text: |\n  hello ${env:X}\n  second line\n";
        let out = interpolate_env_tree(input, &|name| (name == "X").then(|| "ok".to_string()))
            .expect("resolvable placeholder must interpolate");
        let parsed: serde_yml::Value = serde_yml::from_str(&out).expect("output must re-parse");
        assert_eq!(
            parsed.get("text").and_then(serde_yml::Value::as_str),
            Some("hello ok\nsecond line\n"),
            "block scalar content must interpolate as a value, got: {out}"
        );
    }

    #[test]
    fn numeric_leaf_stays_string_after_interpolation() {
        let input = "port: ${env:PORT}\n";
        let out = interpolate_env_tree(input, &|name| (name == "PORT").then(|| "8080".to_string()))
            .expect("resolvable placeholder must interpolate");
        let parsed: serde_yml::Value = serde_yml::from_str(&out).expect("output must re-parse");
        let leaf = parsed.get("port").expect("port leaf must exist");
        assert!(
            leaf.is_string(),
            "numeric-looking result must stay a string (documented typing semantics), got: {out}"
        );
        assert_eq!(leaf.as_str(), Some("8080"));
    }

    #[test]
    fn tagged_node_falls_back_to_legacy() {
        let lookup = |name: &str| (name == "X").then(|| "ok".to_string());
        // Custom tag: core-schema tags (!!str etc.) resolve to plain values,
        // only custom tags reach Value::Tagged in the shim.
        let input = "value: !mytag ${env:X}\n";
        // Fallback contract: the tree walk refuses tagged documents...
        let err = interpolate_env_tree(input, &lookup)
            .expect_err("tagged node must fall back, not pass through");
        assert_eq!(err, TreeInterpolateError::Fallback);
        // ...and discovery's legacy splice interpolates them exactly as
        // before the tree walk existed.
        let legacy = interpolate_env_with(input, &lookup)
            .expect("legacy splice must resolve the placeholder inside the tagged node");
        assert_eq!(legacy, "value: !mytag ok\n");
    }

    // ---- provenance tracking (env-int-placeholder-typing) ----

    #[test]
    fn provenance_records_whole_scalar_leaf() {
        let input = "routes:\n- id: r\n  steps:\n  - throttle:\n      max_requests: ${env:A:-2}\n      period_ms: 7\n";
        let (doc, provenance) = interpolate_yaml_source_with_provenance(input, &|name| {
            (name == "A").then(|| "2".to_string())
        })
        .expect("tree walk must interpolate the whole-scalar token");
        let paths = provenance.expect("tree-walk path must carry provenance");
        assert_eq!(
            paths,
            vec![vec![
                ProvenanceSeg::Key("routes".to_string()),
                ProvenanceSeg::Index(0),
                ProvenanceSeg::Key("steps".to_string()),
                ProvenanceSeg::Index(0),
                ProvenanceSeg::Key("throttle".to_string()),
                ProvenanceSeg::Key("max_requests".to_string()),
            ]],
            "exactly the substituted leaf, in document order (period_ms and \
             key positions absent), got: {doc}"
        );
    }

    #[test]
    fn provenance_excludes_embedded_and_keys() {
        let input = "k${env:A:-1}: v-${env:B:-2}\n";
        let (doc, provenance) =
            interpolate_yaml_source_with_provenance(input, &|_| Some("x".to_string()))
                .expect("embedded tokens must interpolate");
        assert!(
            provenance
                .expect("tree-walk path must carry provenance")
                .is_empty(),
            "embedded-token leaves and mapping keys are never provenance, got: {doc}"
        );
        assert!(
            doc.contains("kx: v-x"),
            "both tokens must substitute, got: {doc}"
        );
    }

    #[test]
    fn provenance_excludes_escapes() {
        let input = "a: $${env:A:-2}\nb: ${env:B:-3}\n";
        let (doc, provenance) = interpolate_yaml_source_with_provenance(input, &|_| None)
            .expect("defaults must resolve both leaves");
        let paths = provenance.expect("tree-walk path must carry provenance");
        assert_eq!(
            paths,
            vec![vec![ProvenanceSeg::Key("b".to_string())]],
            "only the unescaped whole-scalar token is provenance, got: {doc}"
        );
        let parsed: serde_yml::Value = serde_yml::from_str(&doc).expect("output must re-parse");
        assert_eq!(
            parsed.get("a").and_then(serde_yml::Value::as_str),
            Some("${env:A:-2}"),
            "escaped leaf is literal text, got: {doc}"
        );
    }

    #[test]
    fn fallback_yields_none_provenance() {
        let lookup = |name: &str| (name == "X").then(|| "ok".to_string());
        let input = "value: !mytag ${env:X}\n";
        let (doc, provenance) = interpolate_yaml_source_with_provenance(input, &lookup)
            .expect("legacy splice must resolve the tagged document");
        assert_eq!(doc, "value: !mytag ok\n");
        assert!(
            provenance.is_none(),
            "the legacy fallback carries no provenance"
        );
    }

    #[test]
    fn is_whole_scalar_env_token_edges() {
        assert!(is_whole_scalar_env_token("${env:A}"));
        assert!(is_whole_scalar_env_token("${env:A:-2}"));
        assert!(
            !is_whole_scalar_env_token("$${env:A:-2}"),
            "escaped form is literal text, not a substitution"
        );
        assert!(
            !is_whole_scalar_env_token("$$"),
            "bare $$ escape is literal"
        );
        assert!(!is_whole_scalar_env_token("x${env:A}"), "embedded prefix");
        assert!(!is_whole_scalar_env_token("${env:A}y"), "embedded suffix");
        assert!(
            !is_whole_scalar_env_token("${env:A} ${env:B}"),
            "two tokens"
        );
        assert!(!is_whole_scalar_env_token("plain"), "no token at all");
    }
}
