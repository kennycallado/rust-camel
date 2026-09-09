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
//! KEEPS string typing after the tree walk. The legacy raw-splice used to
//! re-parse such text as a number; the YAML `Value` tree cannot preserve
//! plain-scalar style through a round-trip, so the leaf stays a string.
//! Consumers that need numbers compose them inside URI strings instead.
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
/// never interpolated and a substituted leaf keeps STRING typing), falling
/// back to the legacy whole-text splice ([`interpolate_env_with`]) when the
/// document does not survive the YAML round-trip. An unresolved variable
/// from either path surfaces as `Err(var_name)`.
///
/// Both the discovery YAML arm and `load_from_file_with_env` route through
/// this seam so the loader cannot drift from discovery semantics.
pub fn interpolate_yaml_source(
    raw: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    match interpolate_env_tree(raw, lookup) {
        Ok(content) => Ok(content),
        Err(TreeInterpolateError::Unresolved(var_name)) => Err(var_name),
        Err(TreeInterpolateError::Fallback) => interpolate_env_with(raw, lookup),
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
/// results keep string typing (see the module docs).
pub(crate) fn interpolate_env_tree(
    raw: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, TreeInterpolateError> {
    let mut root: serde_yml::Value =
        serde_yml::from_str(raw).map_err(|_| TreeInterpolateError::Fallback)?;
    interpolate_value(&mut root, lookup)?;
    serde_yml::to_string(&root).map_err(|_| TreeInterpolateError::Fallback)
}

/// Whether a scalar's text carries any placeholder or escape token.
fn has_env_token(s: &str) -> bool {
    s.contains("${") || s.contains("$$")
}

/// Recursive tree walk applying `interpolate_string` to scalars that carry
/// a placeholder or escape token; all other nodes pass through untouched.
fn interpolate_value(
    value: &mut serde_yml::Value,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(), TreeInterpolateError> {
    match value {
        serde_yml::Value::String(s) => {
            if has_env_token(s) {
                *s = interpolate_string(s, lookup).map_err(TreeInterpolateError::Unresolved)?;
            }
            Ok(())
        }
        serde_yml::Value::Sequence(seq) => {
            for item in seq.iter_mut() {
                interpolate_value(item, lookup)?;
            }
            Ok(())
        }
        serde_yml::Value::Mapping(map) => {
            // The shim's Mapping keys are strings — interpolate them too.
            // Keys are not mutable in place (`iter_mut` yields `&String`),
            // so rebuilt entries replace the originals in order.
            let mut rebuilt = serde_yml::Mapping::new();
            for (key, val) in map.iter() {
                let mut key = key.clone();
                if has_env_token(&key) {
                    key = interpolate_string(&key, lookup)
                        .map_err(TreeInterpolateError::Unresolved)?;
                }
                let mut val = val.clone();
                interpolate_value(&mut val, lookup)?;
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
}
