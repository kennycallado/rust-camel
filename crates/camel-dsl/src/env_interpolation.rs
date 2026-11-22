//! `${env:}` interpolation for route sources (rc-ayke), generalized to
//! also resolve `${arg:NAME}` job-argument tokens (jobargs Task 2.1):
//! both namespaces share one scanner and tree-walk stage, with namespace
//! dispatch before lookup so `arg:` never falls through to ambient env;
//! the arg grammar is exactly `${arg:NAME}` (no `:-fallback`).
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
//! with the raw-splice outcome class). The provenance of the OVERWRITTEN
//! entry — and of every descendant path beneath it — is dropped at
//! insert time (see `interpolate_value`): value equality is irrelevant, so
//! a substituted leaf overwritten by an equal-valued literal is never a
//! typed-probe candidate. A substituted entry that overwrites an earlier
//! literal keeps its provenance, because the surviving leaf at that path is
//! the substituted value.
//!
//! The JSON walk arm records provenance in SORTED path order (see
//! `interpolate_json_source_with_provenance`): `serde_json`'s
//! `preserve_order` feature is enabled elsewhere in the workspace
//! (`camel-language-rhai`), so cargo feature unification can flip map
//! iteration order between key-sorted and authored; the explicit sort keeps
//! the typed probe's tie-break independent of that.
//!
//! # Route-surface `${}` gate (rc-yhvag, rc-veg9m)
//!
//! Route-source substitution handles `${env:NAME}` ONLY: the parse-tree walk
//! passes an EMPTY argument lookup, so an unescaped `${arg:NAME}` fails to load
//! (job arguments are substituted only on the `camel-cli` job path,
//! `crates/camel-cli/src/commands/job/document.rs`). Interpolation runs BEFORE
//! route parsing and is trusted (ADR-0032), so a substituted `${...}` token
//! becomes a Simple template slot in a bare top-level string value. The
//! `$${env:X}` and `$${arg:X}` escapes preserve `${env:X}` and `${arg:X}`,
//! respectively. The value gate treats each as a `lang:` slot: the route
//! compiles, but the step fails at runtime with `Language 'env' not found`
//! or `Language 'arg' not found`, respectively (preserving a
//! substituted literal `${...}` is deferred to rc-luew6). See the escape tests
//! `escape_full_form_yields_literal` and `escape_arg_form_yields_literal`.

//! SYNC: the whole-text splice arm ([`interpolate_env_with`]) is mirrored by
//! camel-lint::env_interpolation (rc-93wct); crate purity forbids the dependency.
//! Update both together.
use noyalib::compat::serde_yaml as serde_yml;
use regex::Regex;
use std::env;
use std::sync::OnceLock;

static ENV_RE: OnceLock<Regex> = OnceLock::new();

fn env_regex() -> &'static Regex {
    // Escape alternatives exist so `$${env:X}` / `$${arg:X}` never fall
    // through to plain resolution; the full escape form is listed before
    // bare `$$` so it is consumed atomically. `arg:` shares the env token
    // grammar at the same stage, but only the identifier-name form — its
    // `:-fallback` suffix is rejected at dispatch (jobargs Task 2.1).
    ENV_RE.get_or_init(|| {
        Regex::new(r"(\$\$\{(?:env|arg):[^}]*\})|(\$\$)|(\$\{(env|arg):([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\})")
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
    interpolate_string(src, lookup, &|_| None)
}

/// The shared interpolation seam for job and embedded-runtime callers
/// (jobargs Task 2.1): `${env:NAME}` resolves through `env_lookup` and
/// `${arg:NAME}` through `arg_lookup` in one scanner pass at the same
/// stage. Same escape forms (`$${env:X}`, `$${arg:X}`, `$$`),
/// sanitization, and `Err(var_name)` error shape as
/// [`interpolate_env_with`]. Namespace dispatch happens BEFORE lookup:
/// `arg:` consults `arg_lookup` only and never falls through to the
/// environment; its grammar is exactly `${arg:NAME}` (identifier names,
/// no `:-fallback` — that form returns `Err(name)`).
pub fn interpolate_with_args(
    src: &str,
    env_lookup: &dyn Fn(&str) -> Option<String>,
    arg_lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    interpolate_string(src, env_lookup, arg_lookup)
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

/// JSON twin of [`interpolate_yaml_source_with_provenance`] (envtyped-routes
/// task 1.3): parse `raw` as a `serde_json::Value`, walk the same scalar
/// tree through the shared scanner helpers (`interpolate_string`,
/// `has_env_token`, `is_whole_scalar_env_token`), and re-serialize with
/// `serde_json::to_string`. Whole-scalar `${env:...}` substitutions record
/// their structural path; mapping keys are interpolated but never recorded.
/// An unresolved variable surfaces as `Err(var_name)`. A document that does
/// not survive the JSON round-trip falls back to the legacy whole-text
/// splice with `None` provenance (fallback semantics mirrored from the YAML
/// seam; the caller's splice is the first pass and this function is only
/// invoked in repair paths).
///
/// # Provenance order (sorted — a guarantee of THIS walker)
///
/// Authored source order is NOT recoverable and is not this seam's contract.
/// The walk appends paths in `serde_json::Map` iteration order, and that
/// order depends on the map backing: `camel-language-rhai` enables
/// `serde_json`'s `preserve_order` feature, so cargo feature unification
/// hands this crate an authored-order `IndexMap` whenever that crate is in
/// the build graph, and the default key-sorted map otherwise. Recording raw
/// iteration order would therefore make the typed probe's equal-size
/// tie-break depend on unrelated crates in the graph.
///
/// The walker therefore SORTS the collected paths lexicographically before
/// returning — segment-wise: a `Key` compares by key text, an `Index`
/// numerically, and at one position a `Key` precedes an `Index` (derived
/// enum order). The probe's minimum-cardinality search walks candidates in
/// this order, so its tie-break is deterministic in EVERY feature
/// configuration (envtyped-routes task 1.10 pins this).
///
/// # Post-substitution key collisions
///
/// Mapping keys are rebuilt into a fresh `serde_json::Map`. `insert`
/// silently overwrites, so a `contains_key` lookup precedes every insert
/// (order-independent): when the key being inserted already exists in the
/// rebuilt map, the overwritten entry's provenance path and every
/// descendant path are dropped before the covering entry is walked.
pub(crate) fn interpolate_json_source_with_provenance(
    raw: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(String, Option<Vec<ProvenancePath>>), String> {
    let mut root: serde_json::Value = match serde_json::from_str(raw) {
        Ok(root) => root,
        Err(_) => return interpolate_env_with(raw, lookup).map(|text| (text, None)),
    };
    let mut paths = Vec::new();
    // The tree walk is the route-source path: no argument context exists
    // here, so `${arg:...}` tokens fail closed as unresolved.
    match interpolate_json_value(&mut root, lookup, &|_| None, &mut Vec::new(), &mut paths) {
        Ok(()) => {}
        Err(TreeInterpolateError::Unresolved(var_name)) => return Err(var_name),
        Err(TreeInterpolateError::Fallback) => {
            return interpolate_env_with(raw, lookup).map(|text| (text, None));
        }
    }
    // Explicit sort: the walk's insertion order follows the map backing,
    // which feature unification can flip to authored order (`preserve_order`
    // via camel-language-rhai). See the fn doc — sorted order is a property
    // of this walker, not of `serde_json`.
    paths.sort();
    match serde_json::to_string(&root) {
        Ok(text) => Ok((text, Some(paths))),
        Err(_) => interpolate_env_with(raw, lookup).map(|text| (text, None)),
    }
}

/// Shared string scanner for both interpolation paths (legacy whole-text
/// and parse-tree walk). Grammar: `${env:X}`, `${env:X:-default}`,
/// `${arg:X}` (identifier names only — the `:-fallback` form is
/// rejected), `$${env:X}` / `$${arg:X}` and `$$` escapes;
/// `Err(var_name)` on an unresolved var or an `arg:` fallback. Namespace
/// dispatch happens BEFORE lookup: `env:` consults `env_lookup` only,
/// `arg:` consults `arg_lookup` only — an argument name never falls
/// through to the environment.
fn interpolate_string(
    s: &str,
    env_lookup: &dyn Fn(&str) -> Option<String>,
    arg_lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let re = env_regex();
    let mut error: Option<String> = None;

    let result = re.replace_all(s, |caps: &regex::Captures| {
        if error.is_some() {
            return String::new();
        }
        // `$${env:...}` / `$${arg:...}` escape: emit the literal
        // placeholder text (strip one `$`).
        if let Some(escaped) = caps.get(1) {
            return escaped.as_str()[1..].to_string();
        }
        // Standalone `$$` escape: emit a single `$`.
        if caps.get(2).is_some() {
            return "$".to_string();
        }
        let namespace = &caps[4];
        let var_name = &caps[5];
        let default_value = caps.get(6).map(|m| m.as_str());
        // Namespace dispatch BEFORE lookup (jobargs Task 2.1): `arg:`
        // consults the argument lookup only — never the environment —
        // and its grammar is exactly `${arg:NAME}`; the `:-fallback`
        // form is rejected rather than applied.
        if namespace == "arg" {
            if default_value.is_some() {
                error = Some(var_name.to_string());
                return String::new();
            }
            return match arg_lookup(var_name) {
                Some(val) => sanitize_env_value(&val),
                None => {
                    error = Some(var_name.to_string());
                    String::new()
                }
            };
        }
        match env_lookup(var_name) {
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
/// string) stay safe for keys containing `.` or `[i]`. The derived `Ord`
/// defines the probe's tie-break order: `Key` compares by key text, `Index`
/// numerically, and a `Key` precedes an `Index` at the same position.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ProvenanceSeg {
    /// Mapping key leading to the walked node (the interpolated key text).
    Key(String),
    /// Sequence index leading to the walked node.
    Index(usize),
}

/// Structural path from the document root to a substituted leaf. Collected
/// by `interpolate_env_tree_with_provenance` (YAML: document order) and by
/// `interpolate_json_source_with_provenance` (JSON: explicitly sorted
/// lexicographically, see that fn) and consumed by the loader-layer typed
/// probe (env_int_probe).
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
/// not substitutions, and are never recorded. A substituted path that a
/// later mapping entry overwrites (insert-time collision) is dropped with
/// all its descendants — a collided path never names a surviving leaf. The
/// substituted leaf itself keeps STRING typing at this seam — provenance
/// feeds the loader-layer typed probe (env_int_probe), which does any
/// coercion.
pub(crate) fn interpolate_env_tree_with_provenance(
    raw: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(String, Vec<ProvenancePath>), TreeInterpolateError> {
    let mut root: serde_yml::Value =
        serde_yml::from_str(raw).map_err(|_| TreeInterpolateError::Fallback)?;
    let mut paths = Vec::new();
    // The tree walk is the route-source path: no argument context exists
    // here, so `${arg:...}` tokens fail closed as unresolved (never an
    // ambient-environment fall-through).
    interpolate_value(&mut root, lookup, &|_| None, &mut Vec::new(), &mut paths)?;
    let doc = serde_yml::to_string(&root).map_err(|_| TreeInterpolateError::Fallback)?;
    Ok((doc, paths))
}

/// Whether a scalar's text carries any placeholder or escape token.
fn has_env_token(s: &str) -> bool {
    s.contains("${") || s.contains("$$")
}

/// Whether the ENTIRE scalar is exactly one unescaped `${env:NAME}` or
/// `${env:NAME:-default}` token — the plain token form of `env_regex()`
/// spanning the whole string, `env:` namespace only (`arg:` whole-scalars
/// are not env provenance and never feed the typed probe). Escaped forms
/// (`$${env:...}`, `$$`) are literal text, not substitutions, and never
/// match; a token embedded in larger text (`x${env:A}`, `${env:A}y`) does
/// not either.
pub(crate) fn is_whole_scalar_env_token(s: &str) -> bool {
    env_regex()
        .captures(s)
        .filter(|caps| caps.get(4).is_some_and(|ns| ns.as_str() == "env"))
        .and_then(|caps| caps.get(3))
        .is_some_and(|m| m.start() == 0 && m.end() == s.len())
}

/// Recursive tree walk applying `interpolate_string` to scalars that carry
/// a placeholder or escape token; all other nodes pass through untouched.
/// `path` is the structural path of the current node; every whole-scalar
/// token leaf substituted along the way is appended to `paths` (in
/// document order) at the moment it substitutes. Mapping keys are
/// interpolated but never recorded.
///
/// Mapping keys are rebuilt via insert into a fresh order-preserving
/// `serde_yml::Mapping`. When an inserted entry lands on a key that already
/// exists there, the OVERWRITTEN entry's provenance path and every
/// descendant path are dropped from `paths` before the covering entry is
/// walked (and re-records its own paths). Collision is structural, so a
/// same-value overwrite drops the stale path too; a substituted entry that
/// covers an earlier literal keeps its provenance because the surviving
/// leaf at that path is the substituted value.
fn interpolate_value(
    value: &mut serde_yml::Value,
    env_lookup: &dyn Fn(&str) -> Option<String>,
    arg_lookup: &dyn Fn(&str) -> Option<String>,
    path: &mut Vec<ProvenanceSeg>,
    paths: &mut Vec<ProvenancePath>,
) -> Result<(), TreeInterpolateError> {
    match value {
        serde_yml::Value::String(s) => {
            if has_env_token(s) {
                let whole_token = is_whole_scalar_env_token(s);
                *s = interpolate_string(s, env_lookup, arg_lookup)
                    .map_err(TreeInterpolateError::Unresolved)?;
                if whole_token {
                    paths.push(path.clone());
                }
            }
            Ok(())
        }
        serde_yml::Value::Sequence(seq) => {
            for (index, item) in seq.iter_mut().enumerate() {
                path.push(ProvenanceSeg::Index(index));
                interpolate_value(item, env_lookup, arg_lookup, path, paths)?;
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
                    key = interpolate_string(&key, env_lookup, arg_lookup)
                        .map_err(TreeInterpolateError::Unresolved)?;
                }
                path.push(ProvenanceSeg::Key(key.clone()));
                // Insert-time collision guard (order-preserving shim API:
                // `contains_key`/`insert`). `rebuilt` can only hold entries
                // already walked from THIS mapping, so every path under the
                // prefix belongs to the entry the insert overwrites; the
                // covering entry has not walked yet and records its own
                // paths below.
                if rebuilt.contains_key(key.as_str()) {
                    paths.retain(|recorded| !recorded.starts_with(&path[..]));
                }
                let mut val = val.clone();
                interpolate_value(&mut val, env_lookup, arg_lookup, path, paths)?;
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

/// JSON twin of [`interpolate_value`] (envtyped-routes task 1.3): the two
/// `Value` enums have no common tree type, so the recursion arms are
/// mirrored rather than shared; the scalar helpers are the same. `path` is
/// the structural path of the current node; every whole-scalar token leaf
/// substituted along the way is appended to `paths` at the moment it
/// substitutes. Mapping keys are interpolated but never recorded. The
/// recursion appends in WALK order (map-iteration order, hence
/// backing-dependent); [`interpolate_json_source_with_provenance`] sorts the
/// collected set afterwards, and no caller may assume walk order.
///
/// Keys are rebuilt into a fresh `serde_json::Map`. `insert` silently
/// overwrites, so a `contains_key` lookup precedes it: an inserted key that
/// already exists drops the OVERWRITTEN entry's provenance path and every
/// descendant path before the covering entry walks (and records its own
/// paths). The lookup makes the guard independent of map iteration order.
fn interpolate_json_value(
    value: &mut serde_json::Value,
    env_lookup: &dyn Fn(&str) -> Option<String>,
    arg_lookup: &dyn Fn(&str) -> Option<String>,
    path: &mut Vec<ProvenanceSeg>,
    paths: &mut Vec<ProvenancePath>,
) -> Result<(), TreeInterpolateError> {
    match value {
        serde_json::Value::String(s) => {
            if has_env_token(s) {
                let whole_token = is_whole_scalar_env_token(s);
                *s = interpolate_string(s, env_lookup, arg_lookup)
                    .map_err(TreeInterpolateError::Unresolved)?;
                if whole_token {
                    paths.push(path.clone());
                }
            }
            Ok(())
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                path.push(ProvenanceSeg::Index(index));
                interpolate_json_value(item, env_lookup, arg_lookup, path, paths)?;
                path.pop();
            }
            Ok(())
        }
        serde_json::Value::Object(map) => {
            let mut rebuilt = serde_json::Map::new();
            for (key, val) in map.iter() {
                let mut key = key.clone();
                if has_env_token(&key) {
                    key = interpolate_string(&key, env_lookup, arg_lookup)
                        .map_err(TreeInterpolateError::Unresolved)?;
                }
                path.push(ProvenanceSeg::Key(key.clone()));
                // Insert-time collision guard (lookup-based: `rebuilt` only
                // holds entries walked from THIS mapping, so any recorded
                // path under this prefix belongs to the entry being
                // overwritten; the covering entry records its own below).
                if rebuilt.contains_key(&key) {
                    paths.retain(|recorded| !recorded.starts_with(&path[..]));
                }
                let mut val = val.clone();
                interpolate_json_value(&mut val, env_lookup, arg_lookup, path, paths)?;
                path.pop();
                rebuilt.insert(key, val);
            }
            *map = rebuilt;
            Ok(())
        }
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
        let result = interpolate_env_with("uri: ${env:TEST_DSL_HOST}/path", &|name| {
            (name == "TEST_DSL_HOST").then(|| "localhost".to_string())
        })
        .unwrap();
        assert_eq!(result, "uri: localhost/path");
    }

    #[test]
    fn multiple_vars_all_set() {
        let result =
            interpolate_env_with(
                "${env:TEST_DSL_USER}:${env:TEST_DSL_PASS}",
                &|name| match name {
                    "TEST_DSL_USER" => Some("admin".to_string()),
                    "TEST_DSL_PASS" => Some("secret".to_string()),
                    _ => None,
                },
            )
            .unwrap();
        assert_eq!(result, "admin:secret");
    }

    #[test]
    fn missing_var_returns_err_with_name() {
        let err = interpolate_env_with("uri: ${env:TEST_DSL_MISSING}", &|_| None).unwrap_err();
        assert_eq!(err, "TEST_DSL_MISSING");
    }

    #[test]
    fn multiple_vars_first_missing_fails_fast() {
        let err = interpolate_env_with("${env:TEST_DSL_FIRST} ${env:TEST_DSL_SECOND}", &|name| {
            (name == "TEST_DSL_SECOND").then(|| "ok".to_string())
        })
        .unwrap_err();
        assert_eq!(err, "TEST_DSL_FIRST");
    }

    #[test]
    fn default_value_used_when_var_missing() {
        let result =
            interpolate_env_with("uri: ${env:TEST_DSL_DEFAULT_VAR:-localhost}/path", &|_| {
                None
            })
            .unwrap();
        assert_eq!(result, "uri: localhost/path");
    }

    #[test]
    fn default_value_not_used_when_var_set() {
        let result =
            interpolate_env_with("uri: ${env:TEST_DSL_OVERRIDE:-localhost}/path", &|name| {
                (name == "TEST_DSL_OVERRIDE").then(|| "production".to_string())
            })
            .unwrap();
        assert_eq!(result, "uri: production/path");
    }

    #[test]
    fn empty_default_is_valid() {
        let result =
            interpolate_env_with("uri: ${env:TEST_DSL_EMPTY_DEFAULT:-}/path", &|_| None).unwrap();
        assert_eq!(result, "uri: /path");
    }

    #[test]
    fn no_default_still_errors() {
        let err =
            interpolate_env_with("uri: ${env:TEST_DSL_NO_DEFAULT}/path", &|_| None).unwrap_err();
        assert_eq!(err, "TEST_DSL_NO_DEFAULT");
    }

    #[test]
    fn default_with_special_chars() {
        let result = interpolate_env_with(
            "uri: ${env:TEST_DSL_SPECIAL:-host.example.com:9092}/path",
            &|_| None,
        )
        .unwrap();
        assert_eq!(result, "uri: host.example.com:9092/path");
    }

    #[test]
    fn interpolates_value_with_newline_replaced() {
        let result = interpolate_env_with("uri: ${env:TEST_DSL_INJECTION}/path", &|name| {
            (name == "TEST_DSL_INJECTION")
                .then(|| "safe_value\ninjected_key: malicious".to_string())
        })
        .unwrap();
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
        // \0 cannot be placed in the process environment (C string terminator).
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
        let result = interpolate_env_with(
            "${env:TEST_DSL_MIX_A:-fallback}:${env:TEST_DSL_MIX_B}",
            &|name| (name == "TEST_DSL_MIX_B").then(|| "actual".to_string()),
        )
        .unwrap();
        assert_eq!(result, "fallback:actual");
    }

    #[test]
    fn escape_full_form_yields_literal() {
        let result = interpolate_env_with("$${env:RUST_CAMEL_TEST_ESC_A}", &|_| {
            Some("real-val".to_string())
        })
        .unwrap();
        assert_eq!(result, "${env:RUST_CAMEL_TEST_ESC_A}");
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
        let result =
            interpolate_env_with("$${env:LIT} and ${env:RUST_CAMEL_TEST_ESC_B}", &|name| {
                (name == "RUST_CAMEL_TEST_ESC_B").then(|| "val-b".to_string())
            })
            .unwrap();
        assert_eq!(result, "${env:LIT} and val-b");
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

    // ---- insert-time provenance collision guard (envtyped-routes 1.2) ----

    #[test]
    fn yaml_same_value_collision_drops_stale_provenance() {
        // The substituted entry is overwritten by a LITERAL entry with the
        // same surviving value: the substituted path no longer describes
        // any leaf, so it must leave the provenance set. Value equality is
        // irrelevant — the collision is structural.
        let input = "'${env:K:-a}': '${env:V:-404}'\na: '404'\n";
        let (doc, provenance) = interpolate_yaml_source_with_provenance(input, &|_| None)
            .expect("defaults must resolve both substitutions");
        let paths = provenance.expect("tree-walk path must carry provenance");
        assert!(
            paths.is_empty(),
            "the overwritten substituted path must be dropped, got: {paths:?} for {doc}"
        );
        let parsed: serde_yml::Value = serde_yml::from_str(&doc).expect("output must re-parse");
        assert_eq!(
            parsed.get("a").and_then(serde_yml::Value::as_str),
            Some("404"),
            "the literal entry survives, got: {doc}"
        );
    }

    #[test]
    fn yaml_substituted_key_over_literal_keeps_provenance() {
        // The substituted entry lands SECOND and overwrites an earlier
        // literal: the final leaf at that path IS the substituted value, so
        // its provenance is kept.
        let input = "a: '1'\n'${env:K:-a}': '${env:V:-2}'\n";
        let (doc, provenance) = interpolate_yaml_source_with_provenance(input, &|_| None)
            .expect("defaults must resolve both substitutions");
        let paths = provenance.expect("tree-walk path must carry provenance");
        assert_eq!(
            paths,
            vec![vec![ProvenanceSeg::Key("a".to_string())]],
            "the substituted entry keeps its provenance, got: {doc}"
        );
        let parsed: serde_yml::Value = serde_yml::from_str(&doc).expect("output must re-parse");
        assert_eq!(
            parsed.get("a").and_then(serde_yml::Value::as_str),
            Some("2"),
            "the substituted value survives, got: {doc}"
        );
    }

    #[test]
    fn yaml_descendants_of_collided_path_dropped() {
        // The overwritten entry carries a nested mapping and sequence with
        // their own whole-scalar tokens: every descendant path must leave
        // the provenance set with it.
        let input = "'${env:K:-a}':\n  nested: '${env:V:-1}'\n  seq:\n    - '${env:W:-2}'\na:\n  other: '3'\n";
        let (doc, provenance) = interpolate_yaml_source_with_provenance(input, &|_| None)
            .expect("defaults must resolve the nested substitutions");
        let paths = provenance.expect("tree-walk path must carry provenance");
        assert!(
            paths.is_empty(),
            "every descendant of the overwritten path must be dropped, got: {paths:?} for {doc}"
        );
        let parsed: serde_yml::Value = serde_yml::from_str(&doc).expect("output must re-parse");
        assert_eq!(
            parsed
                .get("a")
                .and_then(|a| a.get("other"))
                .and_then(serde_yml::Value::as_str),
            Some("3"),
            "the covering entry survives, got: {doc}"
        );
    }

    // ---- JSON provenance walk (envtyped-routes 1.3) ----

    #[test]
    fn json_provenance_records_whole_scalar_paths() {
        let input = r#"{"routes":[{"id":"r","steps":[{"throttle":{"max_requests":"${env:A:-2}","period_ms":7}}]}]}"#;
        let (doc, provenance) = interpolate_json_source_with_provenance(input, &|name| {
            (name == "A").then(|| "2".to_string())
        })
        .expect("json tree walk must interpolate the whole-scalar token");
        let paths = provenance.expect("json tree-walk path must carry provenance");
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
            "exactly the substituted leaf (Key/Index segments), got: {doc}"
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&doc).expect("output must re-parse as JSON");
        assert_eq!(
            parsed["routes"][0]["steps"][0]["throttle"]["max_requests"],
            serde_json::Value::String("2".to_string()),
            "the substituted leaf survives as a JSON string, got: {doc}"
        );
    }

    #[test]
    fn json_post_substitution_key_collision_drops_provenance() {
        // The substituted key resolves to a sibling literal key that the
        // walk visits LATER (default serde_json maps sort `$` before `s`):
        // the literal overwrites the substituted entry, so the collided
        // path names no surviving leaf and must leave the provenance set.
        let input = r#"{"${env:K:-shared}":"${env:V:-1}","shared":"literal"}"#;
        let (doc, provenance) = interpolate_json_source_with_provenance(input, &|_| None)
            .expect("defaults must resolve both substitutions");
        let paths = provenance.expect("json tree-walk path must carry provenance");
        let collided = vec![ProvenanceSeg::Key("shared".to_string())];
        assert!(
            !paths.contains(&collided),
            "the collided path must be absent from provenance, got: {paths:?} for {doc}"
        );
        assert!(
            paths.is_empty(),
            "no other path survives either, got: {paths:?} for {doc}"
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&doc).expect("output must re-parse as JSON");
        assert_eq!(
            parsed["shared"],
            serde_json::Value::String("literal".to_string()),
            "the literal sibling survives the overwrite, got: {doc}"
        );
    }

    #[test]
    fn json_provenance_order_is_sorted_not_authored() {
        // `b` is authored first but sorts after `a`: the walker sorts the
        // collected paths explicitly, so the order in the returned provenance
        // is a guarantee of this walk and does not depend on the map backing
        // (default key-sorted maps vs the authored-order `IndexMap` that
        // `preserve_order` produces under feature unification).
        let input = r#"{"b":"${env:B:-2}","a":"${env:A:-1}"}"#;
        let (doc, provenance) = interpolate_json_source_with_provenance(input, &|_| None)
            .expect("defaults must resolve both leaves");
        let paths = provenance.expect("json tree-walk path must carry provenance");
        assert_eq!(
            paths,
            vec![
                vec![ProvenanceSeg::Key("a".to_string())],
                vec![ProvenanceSeg::Key("b".to_string())],
            ],
            "json provenance is explicitly sorted, got: {doc}"
        );
    }

    /// Assert the collision outcome for one authored order: the overwritten
    /// entry's provenance and every descendant path are dropped, the covering
    /// entry's substituted value survives, and its provenance is the collapsed
    /// path alone.
    fn assert_json_collision_case(input: &str, covering_key: &str, descendant_key: &str) {
        let (doc, provenance) = interpolate_json_source_with_provenance(input, &|_| None)
            .expect("defaults must resolve every substitution");
        let paths = provenance.expect("json tree-walk path must carry provenance");
        let covering_path = vec![ProvenanceSeg::Key(covering_key.to_string())];
        let descendant_path = vec![
            ProvenanceSeg::Key(covering_key.to_string()),
            ProvenanceSeg::Key(descendant_key.to_string()),
        ];
        assert!(
            !paths.contains(&descendant_path),
            "every descendant of the overwritten entry must be dropped, got: {paths:?} for {doc}"
        );
        assert_eq!(
            paths,
            vec![covering_path],
            "only the covering entry's substituted leaf keeps provenance, got: {paths:?} for {doc}"
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&doc).expect("output must re-parse as JSON");
        assert_eq!(
            parsed[covering_key],
            serde_json::Value::String("8".to_string()),
            "the covering entry's substituted value survives, got: {doc}"
        );
    }

    #[test]
    fn json_post_substitution_collision_drops_descendants_in_both_orders() {
        // Same-value collision authored in BOTH orders. `$` (0x24) sorts
        // before `s` (0x73) but after `!` (0x21), so the substituted entry is
        // the EARLIER walk target in the first fixture and the LATER one in the
        // second; the insert-time `contains_key` guard must drop the
        // overwritten entry and its descendants either way (under
        // `preserve_order` the authored order is what the walk sees, so both
        // directions are covered in every feature configuration).
        //
        // Order 1 — substituted entry walked first (authored and key-sorted
        // first); the trailing literal covers it.
        assert_json_collision_case(
            r#"{"${env:RC_COLL_K:-shared}":{"n":"${env:RC_COLL_D:-7}"},"shared":"${env:RC_COLL_L:-8}"}"#,
            "shared",
            "n",
        );
        // Order 2 — literal entry walked first (sorts and is authored first);
        // the trailing substituted entry covers it.
        assert_json_collision_case(
            r#"{"!shared":{"n":"${env:RC_COLL_D:-7}"},"${env:RC_COLL_K:-!shared}":"${env:RC_COLL_L:-8}"}"#,
            "!shared",
            "n",
        );
    }

    #[test]
    fn json_unresolved_var_errors_with_name() {
        let input = r#"{"leaf":"${env:RC_J_MISSING}"}"#;
        let err = interpolate_json_source_with_provenance(input, &|_| None)
            .expect_err("an unresolved variable must fail the walk");
        assert_eq!(err, "RC_J_MISSING");
    }

    #[test]
    fn json_interpolate_fallback_returns_splice_none() {
        // A document that does not parse as JSON takes the legacy whole-text
        // splice with `None` provenance: the repair seam's gate (ii) must see
        // "no tree-walk baseline" rather than an empty provenance set.
        let input = r#"{ not valid json ${env:X} }"#;
        let (text, provenance) = interpolate_json_source_with_provenance(input, &|name| {
            (name == "X").then(|| "ok".to_string())
        })
        .expect("the fallback arm splices and succeeds");
        assert_eq!(text, "{ not valid json ok }");
        assert!(provenance.is_none(), "fallback carries no provenance");
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

    // ---- `arg:` namespace through the shared scanner (jobargs Task 2.1) ----

    #[test]
    fn interpolate_arg_tokens_at_shared_stage() {
        // One call path resolves BOTH namespaces: `env:` through the env
        // lookup, `arg:` through the argument lookup — same scanner, same
        // stage, same escape and sanitization behavior.
        let env = |name: &str| (name == "HOST").then(|| "broker.local".to_string());
        let arg = |name: &str| (name == "NAME").then(|| "gold".to_string());
        // URI-like (mixed namespaces in one string).
        assert_eq!(
            interpolate_with_args("kafka://${env:HOST}:9092/${arg:NAME}", &env, &arg).unwrap(),
            "kafka://broker.local:9092/gold"
        );
        // Body-like.
        assert_eq!(
            interpolate_with_args("Hello, ${arg:NAME} at ${env:HOST}", &env, &arg).unwrap(),
            "Hello, gold at broker.local"
        );
        // Header-like.
        assert_eq!(
            interpolate_with_args("X-Tier: ${arg:NAME}", &env, &arg).unwrap(),
            "X-Tier: gold"
        );
    }

    #[test]
    fn reject_arg_fallback_syntax() {
        // The arg grammar is exactly `${arg:NAME}`: the `:-fallback` form
        // must never apply — not even when NAME resolves.
        let err = interpolate_with_args("${arg:NAME:-fallback}", &|_| None, &|name| {
            (name == "NAME").then(|| "resolved".to_string())
        })
        .unwrap_err();
        assert_eq!(err, "NAME");
        // Same rejection when the argument is unresolved.
        let err = interpolate_with_args("${arg:NAME:-fallback}", &|_| None, &|_| None).unwrap_err();
        assert_eq!(err, "NAME");
    }

    #[test]
    fn arg_namespace_does_not_fall_through() {
        // Ambient HOME exists, but the argument lookup has no HOME: the
        // `arg:` namespace must dispatch BEFORE any environment lookup.
        let home = env::var("HOME").expect("ambient HOME must be set for this test");
        let env_lookup = |name: &str| env::var(name).ok();
        // Control: the env namespace resolves the same name.
        assert_eq!(
            interpolate_with_args("${env:HOME}", &env_lookup, &|_| None).unwrap(),
            home
        );
        // The arg namespace must NOT fall through to the environment.
        let err = interpolate_with_args("${arg:HOME}", &env_lookup, &|_| None).unwrap_err();
        assert_eq!(err, "HOME");
    }

    #[test]
    fn escape_arg_form_yields_literal() {
        let arg = |name: &str| (name == "NAME").then(|| "gold".to_string());
        assert_eq!(
            interpolate_with_args("$${arg:NAME} and $$", &|_| None, &arg).unwrap(),
            "${arg:NAME} and $"
        );
    }
}
