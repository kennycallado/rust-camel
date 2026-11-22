//! Loader-layer typed probe over interpolation provenance
//! (env-int-placeholder-typing; widened from `routes:`-only to the whole
//! route document and to JSON by envtyped-routes).
//!
//! After a FAILED typed parse of an interpolated document, candidate leaves
//! (whole-scalar substituted placeholders outside the stay-out subtrees whose
//! value is a clean integer or boolean) are coerced to typed scalars in
//! cloned trees; candidate index subsets are tried smallest-first in document
//! order (cap 8), with the caller's QUIET parser as the oracle. First parse
//! success wins; otherwise the original first-pass error stands. Every
//! document that parses today is unaffected — probing runs only after
//! failure.
//!
//! Correctness (minimum-cardinality argument): the search returns the FIRST
//! minimum-cardinality parsing subset in document order. The oracle spans
//! deserialization AND compilation, so S_min = the set of candidate positions
//! whose coercion is REQUIRED for full oracle success. When the repair set is
//! unique (the typed-failure case), every parsing subset must contain S_min,
//! so the only parsing subset of size |S_min| is S_min itself — the search
//! returns exactly those positions, never a polymorphic superset (e.g. a
//! `set_header` value that would also accept the number). When multiple
//! independent sufficient subsets exist (polymorphic positions whose string
//! AND coerced forms both compile), the document-order-first minimum wins
//! deterministically: coercion never extends beyond a minimum-cardinality
//! parsing subset and never touches non-candidates. Unsigned bounds,
//! narrowing, and negatives are enforced by the real parse, not by
//! heuristics.
//!
//! Stay-out subtrees (paths whose FIRST segment is `templates` or
//! `templated_routes`): `templated_routes.parameters` is
//! `BTreeMap<String, String>` by contract, so a coerced number there can only
//! fail a subset — such candidates would be dead weight against the shared
//! cap; template bodies (`templates[].routes[]`) are untyped at document
//! parse, and their typing happens at MATERIALIZATION, which the
//! document-parse probe never observes. Every other subtree (`routes:`,
//! `rest:`, `mcp:`) joins the candidate set and shares ONE cap.
//!
//! Compatibility classes: (a) `routes:`-only repairs are preserved — their
//! candidate set and document order are identical under the widened filter —
//! EXCEPT when widened candidates from other subtrees push the count over the
//! cap, in which case the first-pass error stands (the accepted boundary: the
//! cap already produces that outcome today for over-cap documents); (b)
//! collision-guarded documents are excluded from the promise — the
//! interpolation walker's insert-time collision guard drops a substituted
//! path overwritten by a later mapping key, so a document that probe-loaded
//! only through such a stale path keeps the first-pass error (a surviving
//! literal must never coerce on a token's provenance); (c) widened-scope
//! repairs (`rest:`/`mcp:` candidates) turn prior hard failures into loads.
//! JSON documents repair through the same probe over a `serde_json` tree;
//! JSON duplicate-key documents never repair (their gate lives in the JSON
//! seam, `json.rs`).

use camel_api::CamelError;
use camel_core::route::RouteDefinition;

use crate::env_interpolation::{ProvenancePath, ProvenanceSeg};
use noyalib::compat::serde_yaml as serde_yml;

/// Maximum number of candidate leaves the subset search will enumerate
/// (2^8 - 1 = 255 in-memory parses worst case). Documents with more
/// candidates keep the first-pass error.
const MAX_PROBE_CANDIDATES: usize = 8;

/// Tree-neutral typed scalar a candidate leaf is coerced to. The document
/// trees differ (YAML shim vs `serde_json`), so the clean-lexical result is
/// format-neutral and each [`ProbeDoc`] impl maps it onto its own tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeScalar {
    /// Signed integer (i64 range).
    Int(i64),
    /// Unsigned integer beyond the i64 range (u64 range).
    UInt(u64),
    /// Boolean.
    Bool(bool),
}

/// Clean integer: the lexical form `-?(0|[1-9][0-9]*)` (no trim, no leading
/// zeros — YAML 1.1 octal ambiguity; floats, bools, and `1e3` are not
/// clean), then an exact i64 or u64 parse (overflow past u64 is not clean).
/// Returns the typed scalar the candidate is coerced to.
// SYNC: this rule is mirrored by camel-config's `clean_i64`
// (crates/camel-config/src/env_int_probe.rs) and camel-lint's
// typing-mirror carve-out (rschema.rs); crate purity forbids the
// dependency. Update all three together. The boolean mirrors are
// camel-config's `clean_bool` (same file) and [`clean_boolean`] below.
pub(crate) fn clean_integer(s: &str) -> Option<ProbeScalar> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    let lexically_clean = match digits.as_bytes() {
        // `0` alone — no leading zeros allowed.
        [b'0'] => true,
        // `[1-9]` followed by ASCII digits only.
        [first, rest @ ..] if first.is_ascii_digit() && *first != b'0' => {
            rest.iter().all(|b| b.is_ascii_digit())
        }
        _ => false,
    };
    if !lexically_clean {
        return None;
    }
    if let Ok(n) = s.parse::<i64>() {
        return Some(ProbeScalar::Int(n));
    }
    if let Ok(n) = s.parse::<u64>() {
        return Some(ProbeScalar::UInt(n));
    }
    None
}

/// Clean boolean: exactly the lowercase literals `true` and `false`
/// (byte-exact match — no trim, no case folding, no `yes`/`no`, no `1`/`0`).
/// Returns the typed scalar the candidate is coerced to.
// SYNC: this rule is mirrored by camel-config's `clean_bool`
// (crates/camel-config/src/env_int_probe.rs) and camel-lint's rschema
// `clean_boolean` (crates/camel-lint/src/rules/rschema.rs); crate purity
// forbids the dependencies. Update all three together.
fn clean_boolean(s: &str) -> Option<ProbeScalar> {
    match s {
        "true" => Some(ProbeScalar::Bool(true)),
        "false" => Some(ProbeScalar::Bool(false)),
        _ => None,
    }
}

/// Tree operations the subset search needs — one impl per document format
/// (YAML shim, `serde_json`). Keeping [`run_probe`] generic over this trait is
/// what lets a coerced JSON document stay JSON: ONE subset loop, two tree
/// glues.
pub(crate) trait ProbeDoc: Sized {
    /// Parse document text into a navigable tree; `None` when the text no
    /// longer parses (nothing to navigate).
    fn from_text(text: &str) -> Option<Self>;

    /// Serialize the tree back to document text for the oracle; `None` when
    /// serialization fails (that subset is simply not a parsing subset).
    fn to_text(&self) -> Option<String>;

    /// Read the leaf at `path` as a string; `None` when a segment does not
    /// navigate or the leaf is not a string.
    fn string_leaf_at(&self, path: &[ProvenanceSeg]) -> Option<&str>;

    /// Replace the string leaf at `path` with `value`; `false` when a segment
    /// does not navigate or the leaf is not a string.
    fn coerce_leaf_at(&mut self, path: &[ProvenanceSeg], value: &ProbeScalar) -> bool;
}

/// YAML document tree (crate shim) for the probe: coercion and serialization
/// are byte-for-byte the pre-generalization behavior.
pub(crate) struct YamlDoc(serde_yml::Value);

impl ProbeDoc for YamlDoc {
    fn from_text(text: &str) -> Option<Self> {
        serde_yml::from_str(text).ok().map(Self)
    }

    fn to_text(&self) -> Option<String> {
        serde_yml::to_string(&self.0).ok()
    }

    fn string_leaf_at(&self, path: &[ProvenanceSeg]) -> Option<&str> {
        match resolve_path(&self.0, path)? {
            serde_yml::Value::String(s) => Some(s),
            _ => None,
        }
    }

    fn coerce_leaf_at(&mut self, path: &[ProvenanceSeg], value: &ProbeScalar) -> bool {
        let Some(leaf) = resolve_path_mut(&mut self.0, path) else {
            return false;
        };
        if !matches!(leaf, serde_yml::Value::String(_)) {
            return false;
        }
        *leaf = match value {
            ProbeScalar::Int(n) => serde_yml::Value::Number(serde_yml::Number::from(*n)),
            ProbeScalar::UInt(n) => serde_yml::Value::Number(serde_yml::Number::from(*n)),
            ProbeScalar::Bool(b) => serde_yml::Value::Bool(*b),
        };
        true
    }
}

/// JSON document tree for the probe: coerced copies serialize with
/// `serde_json`, so the oracle keeps reading JSON (the YAML-shim tree would
/// re-serialize to YAML — the blocker this seam removes).
pub(crate) struct JsonDoc(serde_json::Value);

impl ProbeDoc for JsonDoc {
    fn from_text(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok().map(Self)
    }

    fn to_text(&self) -> Option<String> {
        serde_json::to_string(&self.0).ok()
    }

    fn string_leaf_at(&self, path: &[ProvenanceSeg]) -> Option<&str> {
        let mut node = &self.0;
        for seg in path {
            node = match (node, seg) {
                (serde_json::Value::Object(map), ProvenanceSeg::Key(key)) => map.get(key)?,
                (serde_json::Value::Array(seq), ProvenanceSeg::Index(index)) => seq.get(*index)?,
                _ => return None,
            };
        }
        match node {
            serde_json::Value::String(s) => Some(s),
            _ => None,
        }
    }

    fn coerce_leaf_at(&mut self, path: &[ProvenanceSeg], value: &ProbeScalar) -> bool {
        let mut node = &mut self.0;
        for seg in path {
            node = match (node, seg) {
                (serde_json::Value::Object(map), ProvenanceSeg::Key(key)) => {
                    match map.get_mut(key) {
                        Some(next) => next,
                        None => return false,
                    }
                }
                (serde_json::Value::Array(seq), ProvenanceSeg::Index(index)) => {
                    match seq.get_mut(*index) {
                        Some(next) => next,
                        None => return false,
                    }
                }
                _ => return false,
            };
        }
        if !matches!(node, serde_json::Value::String(_)) {
            return false;
        }
        *node = match value {
            ProbeScalar::Int(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
            ProbeScalar::UInt(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
            ProbeScalar::Bool(b) => serde_json::Value::Bool(*b),
        };
        true
    }
}

/// Resolve a structural provenance path against a YAML tree; `None` when a
/// segment does not navigate (wrong node kind or missing key/index).
fn resolve_path<'a>(
    root: &'a serde_yml::Value,
    path: &[ProvenanceSeg],
) -> Option<&'a serde_yml::Value> {
    let mut node = root;
    for seg in path {
        node = match (node, seg) {
            (serde_yml::Value::Mapping(map), ProvenanceSeg::Key(key)) => map.get(key.as_str())?,
            (serde_yml::Value::Sequence(seq), ProvenanceSeg::Index(index)) => seq.get(*index)?,
            _ => return None,
        };
    }
    Some(node)
}

/// Mutable twin of [`resolve_path`].
fn resolve_path_mut<'a>(
    root: &'a mut serde_yml::Value,
    path: &[ProvenanceSeg],
) -> Option<&'a mut serde_yml::Value> {
    let mut node = root;
    for seg in path {
        node = match (node, seg) {
            (serde_yml::Value::Mapping(map), ProvenanceSeg::Key(key)) => {
                map.get_mut(key.as_str())?
            }
            (serde_yml::Value::Sequence(seq), ProvenanceSeg::Index(index)) => {
                seq.get_mut(*index)?
            }
            _ => return None,
        };
    }
    Some(node)
}

/// Candidate leaves for the probe: provenance paths outside the stay-out
/// subtrees (see [`is_stay_out_subtree`]) that resolve to a `&str`
/// [`clean_integer`] or [`clean_boolean`] accepts. Document order preserved
/// (provenance order).
fn candidates<T: ProbeDoc>(tree: &T, provenance: &[ProvenancePath]) -> Vec<ProvenancePath> {
    provenance
        .iter()
        .filter(|path| !is_stay_out_subtree(path))
        .filter(|path| match tree.string_leaf_at(path) {
            Some(s) => clean_integer(s).is_some() || clean_boolean(s).is_some(),
            None => false,
        })
        .cloned()
        .collect()
}

/// Stay-out subtrees: `templates` bodies are untyped at document parse (their
/// typing happens at materialization, which this probe never observes) and
/// `templated_routes.parameters` is `BTreeMap<String, String>` by contract. A
/// coerced number can never repair either position, so these numeric-looking
/// leaves must not consume the shared cap.
fn is_stay_out_subtree(path: &[ProvenanceSeg]) -> bool {
    matches!(
        path.first(),
        Some(ProvenanceSeg::Key(key)) if key == "templates" || key == "templated_routes"
    )
}

/// Advance `indices` to the next combination of its size over `0..universe`
/// in lexicographic order; `false` when the enumeration is exhausted.
fn next_combination(indices: &mut [usize], universe: usize) -> bool {
    let size = indices.len();
    let mut i = size;
    loop {
        if i == 0 {
            return false;
        }
        i -= 1;
        if indices[i] < universe - (size - i) {
            indices[i] += 1;
            for j in i + 1..size {
                indices[j] = indices[j - 1] + 1;
            }
            return true;
        }
    }
}

/// Generic typed probe parse: run `parse(doc_text)`; on failure, coerce
/// candidate leaves (from `provenance`, see [`candidates`]) to their typed
/// scalars in fresh copies of the document tree `T` and retry
/// smallest-first. The oracle return type is generic so the same bounded
/// search serves route parses ([`parse_with_probe`]), JSON route parses
/// ([`parse_json_with_probe`]) and the openapi REST extract.
///
/// `parse` MUST NOT emit `error!`-level logging for this seam: speculative
/// attempts are expected failures and the CALLER replays the final failure
/// exactly once, after the probe returns it. `warn!` suppression during
/// speculative attempts is the JSON quiet twin's contract
/// (`QuietCompileGuard`); YAML oracles keep their current warning behavior.
/// No environment reads; no `unwrap`.
pub(crate) fn run_probe<T: ProbeDoc, R>(
    doc_text: &str,
    provenance: &[ProvenancePath],
    parse: impl Fn(&str) -> Result<R, CamelError>,
) -> Result<R, CamelError> {
    run_probe_with_text::<T, R>(doc_text, provenance, parse).map(|(result, _text)| result)
}

/// [`run_probe`] twin that also returns the WINNING subset's serialization
/// — the document text that the oracle accepted (the input text when it
/// parses as-is). Discovery's JSON arm feeds the repaired text to the
/// template extractors, which deserialize the whole `RouteDslRoutes` and
/// would reject a still-string typed leaf; the string seam discards it.
///
/// Same single subset-enumeration loop as [`run_probe`] (one probe
/// execution, no second search); the winning text is the serialized tree
/// [`try_coerced_subset`] already builds for the oracle call.
pub(crate) fn run_probe_with_text<T: ProbeDoc, R>(
    doc_text: &str,
    provenance: &[ProvenancePath],
    parse: impl Fn(&str) -> Result<R, CamelError>,
) -> Result<(R, String), CamelError> {
    let first_error = match parse(doc_text) {
        Ok(result) => return Ok((result, doc_text.to_string())),
        Err(e) => e,
    };

    let Some(base_tree) = T::from_text(doc_text) else {
        // The interpolated text no longer parses to a plain tree — nothing
        // to navigate; the first-pass error stands.
        return Err(first_error);
    };
    // Candidate paths paired with their typed scalars. `candidates` accepted
    // these exact paths against the same text moments ago, so the anomaly
    // branches are unreachable; skipping (never unwrapping) keeps the loop
    // total regardless.
    let mut candidate_paths: Vec<ProvenancePath> = Vec::new();
    let mut coerced_values: Vec<ProbeScalar> = Vec::new();
    for path in candidates(&base_tree, provenance) {
        let Some(leaf) = base_tree.string_leaf_at(&path) else {
            continue;
        };
        if let Some(value) = clean_integer(leaf).or_else(|| clean_boolean(leaf)) {
            candidate_paths.push(path);
            coerced_values.push(value);
        }
    }
    if candidate_paths.is_empty() || candidate_paths.len() > MAX_PROBE_CANDIDATES {
        return Err(first_error);
    }

    let universe = candidate_paths.len();
    for size in 1..=universe {
        let mut indices: Vec<usize> = (0..size).collect();
        loop {
            if let Ok((result, text)) = try_coerced_subset::<T, R>(
                doc_text,
                &candidate_paths,
                &coerced_values,
                &indices,
                &parse,
            ) {
                return Ok((result, text));
            }
            if !next_combination(&mut indices, universe) {
                break;
            }
        }
    }
    Err(first_error)
}

/// Typed probe parse for YAML documents: the historic signature and behavior
/// of the loader seam; `None` provenance means no tree walk ran, so only the
/// first pass is possible.
pub(crate) fn parse_with_probe<P>(
    doc_text: &str,
    provenance: Option<&[ProvenancePath]>,
    parse: P,
) -> Result<Vec<RouteDefinition>, CamelError>
where
    P: Fn(&str) -> Result<Vec<RouteDefinition>, CamelError>,
{
    match provenance {
        Some(provenance) => run_probe::<YamlDoc, _>(doc_text, provenance, parse),
        None => parse(doc_text),
    }
}

/// Typed probe parse for JSON documents: same bounded search over a
/// `serde_json` tree, so a coerced copy re-serializes as JSON. Consumed by the
/// JSON route-document seam (`json.rs`) and by the oracle-generic repair
/// helper it exposes.
pub(crate) fn parse_json_with_probe<R>(
    doc_text: &str,
    provenance: Option<&[ProvenancePath]>,
    parse: impl Fn(&str) -> Result<R, CamelError>,
) -> Result<R, CamelError> {
    match provenance {
        Some(provenance) => run_probe::<JsonDoc, _>(doc_text, provenance, parse),
        None => parse(doc_text),
    }
}

/// [`parse_json_with_probe`] twin for discovery's JSON arm that ALSO returns
/// the winning repaired document text. The winning text feeds every
/// downstream consumer of that document (the JSON template extractors
/// deserialize the WHOLE `RouteDslRoutes` and would reject a still-string
/// typed leaf, so they must read the repaired document — not the original
/// splice). ONE probe execution: this wrapper runs the shared bounded search
/// with the QUIET route oracle built from the caller's threshold / security /
/// script context; no second search exists.
///
/// Opaque to `Option` provenance on purpose: the caller reaches this only
/// after the repair gates produced a provenance set (a JSON tree walk that
/// ran), so the search always starts from a known provenance.
pub(crate) fn parse_json_with_probe_with_text(
    json: &str,
    provenance: &[ProvenancePath],
    script_ctx: &crate::yaml::ScriptLoadContext,
    threshold: usize,
    security_ctx: crate::model::SecurityCompileContext,
) -> Result<(Vec<RouteDefinition>, String), CamelError> {
    run_probe_with_text::<JsonDoc, _>(json, provenance, |text| {
        crate::json::parse_json_with_threshold_and_security_quiet(
            text,
            threshold,
            security_ctx.clone(),
            script_ctx,
        )
    })
}

/// Whether `doc_text` carries at least one coercion candidate at
/// `provenance` (same candidate rules as [`candidates`]: stay-out subtrees
/// excluded, clean integer/boolean values only). The JSON repair seam's
/// gate (iii): a repair pass with zero candidates re-parses nothing.
pub(crate) fn json_has_probe_candidates(doc_text: &str, provenance: &[ProvenancePath]) -> bool {
    match JsonDoc::from_text(doc_text) {
        Some(tree) => !candidates(&tree, provenance).is_empty(),
        None => false,
    }
}

/// Scoped "no logging" guard for speculative probe attempts.
///
/// The compile stage is not logging-free: step compilers legitimately emit
/// `warn!` events (e.g. the at-most-once `idempotent_consumer` contract).
/// Probe attempts re-run that compile once per candidate subset, so those
/// events would be repeated per attempt. The guard installs a THREAD-LOCAL
/// [`tracing::Dispatch`] for its lifetime: every event raised on this thread
/// reaches only the quiet subscriber, which counts and drops `WARN`/`ERROR`
/// events and forwards nothing (a speculative attempt is silent by
/// contract). The ambient dispatcher (an application subscriber or a test
/// capture layer) resumes on drop — the guard is scoped, never global.
///
/// Every callsite still reports enabled, so callsite interest is never
/// poisoned to `never`: an event's level decides suppression at event time,
/// not at registration time.
pub(crate) struct QuietCompileGuard {
    _guard: tracing::subscriber::DefaultGuard,
}

impl QuietCompileGuard {
    pub(crate) fn new() -> Self {
        Self {
            _guard: tracing::subscriber::set_default(QuietSubscriber::new()),
        }
    }
}

thread_local! {
    /// WARN/ERROR events dropped by [`QuietCompileGuard`] on THIS thread
    /// (monotonic). Thread-local so parallel tests cannot pollute one
    /// another's count.
    static SUPPRESSED_EVENTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// WARN/ERROR events dropped by [`QuietCompileGuard`] on this thread since
/// the process started. Used by the quietness pins to prove the repair pass
/// actually compiled (a fixture that never retried would make the count
/// assertions vacuous).
#[cfg(test)]
pub(crate) fn suppressed_event_count() -> usize {
    SUPPRESSED_EVENTS.with(std::cell::Cell::get)
}

/// Minimal subscriber behind [`QuietCompileGuard`]: accepts every callsite
/// (interest stays enabled) and drops every event, counting WARN/ERROR.
struct QuietSubscriber {
    next_span: std::sync::atomic::AtomicU64,
}

impl QuietSubscriber {
    fn new() -> Self {
        Self {
            // Span ids must be non-zero unique; the value itself carries no
            // meaning for a subscriber that records nothing.
            next_span: std::sync::atomic::AtomicU64::new(1),
        }
    }
}

impl tracing::Subscriber for QuietSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(
            self.next_span
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        )
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        if matches!(
            *event.metadata().level(),
            tracing::Level::WARN | tracing::Level::ERROR
        ) {
            SUPPRESSED_EVENTS.with(|count| count.set(count.get() + 1));
        }
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// Parse `doc_text` into a fresh `T`, coerce the candidate leaves named by
/// `indices` to their typed scalars, serialize with the format's own writer,
/// and run `parse`. Returns the oracle result plus the serialized text it
/// accepted (the winning document). Skips (returns `Err`) on any navigation
/// anomaly — a subset that cannot be applied is simply not a parsing subset.
fn try_coerced_subset<T: ProbeDoc, R>(
    doc_text: &str,
    candidates: &[ProvenancePath],
    coerced: &[ProbeScalar],
    indices: &[usize],
    parse: &impl Fn(&str) -> Result<R, CamelError>,
) -> Result<(R, String), CamelError> {
    // Fresh tree per attempt — attempts never observe each other's edits.
    let mut tree = T::from_text(doc_text).ok_or_else(|| {
        CamelError::RouteError("probe: interpolated document lost tree shape".into())
    })?;
    for &i in indices {
        if !tree.coerce_leaf_at(&candidates[i], &coerced[i]) {
            return Err(CamelError::RouteError(
                "probe: candidate path no longer resolves to a string leaf".into(),
            ));
        }
    }
    let text = tree.to_text().ok_or_else(|| {
        CamelError::RouteError("probe: coerced document failed to serialize".into())
    })?;
    let result = parse(&text)?;
    Ok((result, text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env_interpolation::interpolate_yaml_source_with_provenance;
    use crate::model::ValueSourceDef;
    use crate::test_support::EventCounts;

    /// Interpolate `raw` through `lookup` and run the probe with the real
    /// quiet parser.
    fn probe_with_lookup(
        raw: &str,
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Vec<RouteDefinition>, CamelError> {
        let (text, provenance) =
            interpolate_yaml_source_with_provenance(raw, lookup).expect("interpolation");
        let provenance = provenance.expect("tree-walk path carries provenance");
        parse_with_probe(&text, Some(&provenance), |probe_text| {
            crate::yaml::parse_yaml_for_probe(
                probe_text,
                &crate::yaml::ScriptLoadContext::new(None),
            )
        })
    }

    /// Interpolate `raw` with a `None` lookup (defaults resolve) and run the
    /// probe with the real quiet parser.
    fn probe_defaults(raw: &str) -> Result<Vec<RouteDefinition>, CamelError> {
        probe_with_lookup(raw, &|_| None)
    }

    fn assert_throttle_max_requests(routes: &[RouteDefinition], expected: usize) {
        match &routes[0].steps()[0] {
            camel_core::route::BuilderStep::Throttle { config, .. } => {
                assert_eq!(config.max_requests, expected);
            }
            other => panic!("expected throttle step, got: {other:?}"),
        }
    }

    fn assert_set_header_string(routes: &[RouteDefinition], expected: &str) {
        match &routes[0].steps()[0] {
            camel_core::route::BuilderStep::DeclarativeSetHeader { key, value } => {
                assert_eq!(key, "k");
                assert_eq!(
                    value,
                    &ValueSourceDef::Literal(serde_json::Value::String(expected.into()))
                );
            }
            other => panic!("expected set_header step, got: {other:?}"),
        }
    }

    #[test]
    fn probe_fixes_single_int_position() {
        let raw = "routes:\n  - id: probe-one\n    from: \"direct:start\"\n    steps:\n      - throttle:\n          max_requests: ${env:RC_PROBE_ONE:-2}\n          period_secs: 1\n";
        let routes = probe_defaults(raw).expect("probe must fix the single int position");
        assert_throttle_max_requests(&routes, 2);
    }

    #[test]
    fn probe_minimal_subset_skips_polymorphic() {
        // Header-first order: the size-1 {header} subset fails typed parse
        // (throttle stays string), the size-1 {throttle} subset succeeds —
        // the search never reaches the size-2 polymorphic superset.
        let header_first = "routes:\n  - id: mixed-hf\n    from: \"direct:start\"\n    steps:\n      - set_header:\n          key: k\n          value: ${env:RC_H:-123}\n      - throttle:\n          max_requests: ${env:RC_L:-2}\n          period_secs: 1\n";
        let routes = probe_defaults(header_first).expect("mixed doc must load");
        assert_set_header_string(&routes, "123");
        match &routes[0].steps()[1] {
            camel_core::route::BuilderStep::Throttle { config, .. } => {
                assert_eq!(config.max_requests, 2);
            }
            other => panic!("expected throttle step, got: {other:?}"),
        }

        // Throttle-first order: the size-1 {throttle} subset is tried first
        // and wins — same result.
        let throttle_first = "routes:\n  - id: mixed-tf\n    from: \"direct:start\"\n    steps:\n      - throttle:\n          max_requests: ${env:RC_L:-2}\n          period_secs: 1\n      - set_header:\n          key: k\n          value: ${env:RC_H:-123}\n";
        let routes = probe_defaults(throttle_first).expect("mixed doc must load");
        assert_throttle_max_requests(&routes, 2);
        match &routes[0].steps()[1] {
            camel_core::route::BuilderStep::DeclarativeSetHeader { key, value } => {
                assert_eq!(key, "k");
                assert_eq!(
                    value,
                    &ValueSourceDef::Literal(serde_json::Value::String("123".into()))
                );
            }
            other => panic!("expected set_header step, got: {other:?}"),
        }
    }

    #[test]
    fn probe_rejects_negative_at_unsigned() {
        // `-2` is a clean i64 candidate, but `usize` deserialization rejects
        // the coerced copy — all subsets fail, pass-1 error stands.
        let raw = "routes:\n  - id: neg\n    from: \"direct:start\"\n    steps:\n      - throttle:\n          max_requests: ${env:RC_NEG:--2}\n          period_secs: 1\n";
        let err = match probe_defaults(raw) {
            Ok(_) => panic!("negative at an unsigned position must fail"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("did not match any variant"),
            "expected the pass-1 untagged-variant error, got: {err}"
        );
    }

    #[test]
    fn probe_rejects_leading_zero_and_overflow() {
        for default in ["007", "99999999999999999999999"] {
            let raw = format!(
                "routes:\n  - id: lz\n    from: \"direct:start\"\n    steps:\n      - throttle:\n          max_requests: ${{env:RC_LZ:-{default}}}\n          period_secs: 1\n"
            );
            let err = match probe_defaults(&raw) {
                Ok(_) => panic!("non-clean default `{default}` must not become a candidate"),
                Err(e) => e.to_string(),
            };
            assert!(
                err.contains("did not match any variant"),
                "expected the pass-1 error for `{default}`, got: {err}"
            );
        }
    }

    /// Build a one-route document with `count` throttle steps, each carrying
    /// its own int placeholder (default = step index + 1).
    fn multi_candidate_doc(count: usize, id: &str) -> String {
        let mut doc = format!("routes:\n  - id: {id}\n    from: \"direct:start\"\n    steps:\n");
        for i in 0..count {
            doc.push_str(&format!(
                "      - throttle:\n          max_requests: ${{env:RC_CAP_{i}:-{}}}\n          period_secs: 1\n",
                i + 1
            ));
        }
        doc
    }

    #[test]
    fn probe_cap_nine_candidates_fails() {
        let err = match probe_defaults(&multi_candidate_doc(9, "cap9")) {
            Ok(_) => panic!("nine candidates exceed the cap; pass-1 error must stand"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("did not match any variant"),
            "expected the pass-1 error, got: {err}"
        );
    }

    #[test]
    fn probe_cap_eight_candidates_succeeds() {
        let routes =
            probe_defaults(&multi_candidate_doc(8, "cap8")).expect("eight candidates load");
        let steps = routes[0].steps();
        assert_eq!(steps.len(), 8);
        for (i, step) in steps.iter().enumerate() {
            match step {
                camel_core::route::BuilderStep::Throttle { config, .. } => {
                    assert_eq!(config.max_requests, i + 1, "step {i}");
                }
                other => panic!("expected throttle step {i}, got: {other:?}"),
            }
        }
    }

    // ── Boolean coercion (task 2.1) ──────────────────────────────────────

    const BOOL_ROUTE_DOC: &str = "routes:\n  - id: bool-one\n    from: \"direct:start\"\n    auto_startup: ${env:RC_AUTO:-false}\n    steps:\n      - throttle:\n          max_requests: 1\n          period_secs: 1\n";

    #[test]
    fn probe_fixes_bool_route_field_with_default() {
        let routes = probe_defaults(BOOL_ROUTE_DOC).expect("probe must fix the bool route field");
        assert!(!routes[0].auto_startup());
    }

    #[test]
    fn probe_bool_route_field_honors_explicit_lookup() {
        let (text, provenance) = interpolate_yaml_source_with_provenance(BOOL_ROUTE_DOC, &|name| {
            (name == "RC_AUTO").then(|| "true".to_string())
        })
        .expect("interpolation");
        let provenance = provenance.expect("tree-walk path carries provenance");
        let routes = parse_with_probe(&text, Some(&provenance), |probe_text| {
            crate::yaml::parse_yaml_for_probe(
                probe_text,
                &crate::yaml::ScriptLoadContext::new(None),
            )
        })
        .expect("probe must fix the explicit bool");
        assert!(routes[0].auto_startup());
    }

    #[test]
    fn literal_quoted_bool_at_bool_position_stays_rejected() {
        let raw = "routes:\n  - id: quoted-bool\n    from: \"direct:start\"\n    auto_startup: \"true\"\n    steps:\n      - throttle:\n          max_requests: 1\n          period_secs: 1\n";
        let err = match probe_defaults(raw) {
            Ok(_) => panic!("a quoted literal must not be coerced"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("type mismatch: expected bool, found string"),
            "expected the pass-1 bool/string error, got: {err}"
        );
    }

    #[test]
    fn literal_quoted_int_at_int_position_stays_rejected() {
        let raw = "routes:\n  - id: quoted-int\n    from: \"direct:start\"\n    steps:\n      - throttle:\n          max_requests: \"2\"\n          period_secs: 1\n";
        let err = match probe_defaults(raw) {
            Ok(_) => panic!("a quoted literal must not be coerced"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("did not match any variant"),
            "expected the pass-1 untagged-variant error, got: {err}"
        );
    }

    #[test]
    fn probe_string_position_bool_default_stays_string() {
        let raw = "routes:\n  - id: str-bool\n    from: \"direct:start\"\n    steps:\n      - set_header:\n          key: k\n          value: ${env:RC_STR:-true}\n";
        let routes = probe_defaults(raw).expect("string position must load");
        assert_set_header_string(&routes, "true");
    }

    #[test]
    fn probe_bool_out_of_set_keeps_pass1_error() {
        for default in ["yes", "TRUE", "1", "true "] {
            let raw = format!(
                "routes:\n  - id: bad-bool\n    from: \"direct:start\"\n    auto_startup: ${{env:RC_B:-{default}}}\n    steps:\n      - throttle:\n          max_requests: 1\n          period_secs: 1\n"
            );
            let err = match probe_defaults(&raw) {
                Ok(_) => panic!("non-clean bool default `{default}` must not fix the doc"),
                Err(e) => e.to_string(),
            };
            assert!(
                err.contains("type mismatch: expected bool, found string"),
                "expected the pass-1 bool/string error for `{default}`, got: {err}"
            );
        }
    }

    /// Nine-route document: routes `1..=bool_routes` carry an
    /// `auto_startup` bool placeholder (default `false`); the remaining
    /// routes have no `auto_startup`; the last route carries one int
    /// throttle placeholder.
    fn mixed_cap_doc(bool_routes: usize) -> String {
        let mut doc = String::from("routes:\n");
        for i in 1..=9 {
            doc.push_str(&format!("  - id: capb-{i}\n    from: \"direct:start\"\n"));
            if i <= bool_routes {
                doc.push_str(&format!("    auto_startup: ${{env:RC_CAPB_{i}:-false}}\n"));
            }
            if i == 9 {
                doc.push_str(
                    "    steps:\n      - throttle:\n          max_requests: ${env:RC_CAPB_INT:-2}\n          period_secs: 1\n",
                );
            } else {
                doc.push_str(
                    "    steps:\n      - set_header:\n          key: k\n          value: x\n",
                );
            }
        }
        doc
    }

    #[test]
    fn probe_cap_eight_bools_one_int_nine_candidates_fails() {
        let err = match probe_defaults(&mixed_cap_doc(8)) {
            Ok(_) => panic!("nine combined candidates exceed the cap; pass-1 error must stand"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("type mismatch: expected bool, found string"),
            "expected the pass-1 bool/string error, got: {err}"
        );

        let routes =
            probe_defaults(&mixed_cap_doc(7)).expect("seven bools plus one int are within the cap");
        for (i, route) in routes.iter().take(7).enumerate() {
            assert!(
                !route.auto_startup(),
                "route {i} must honor the false default"
            );
        }
        for (i, route) in routes.iter().enumerate().skip(7) {
            assert!(route.auto_startup(), "route {i} must keep the default true");
        }
        match &routes[8].steps()[0] {
            camel_core::route::BuilderStep::Throttle { config, .. } => {
                assert_eq!(config.max_requests, 2);
            }
            other => panic!("expected throttle step, got: {other:?}"),
        }
    }

    // ── Logging contract at the real seam (task 1.2 acceptance) ──────────

    fn error_counting_subscriber() -> (EventCounts, tracing::subscriber::DefaultGuard) {
        crate::test_support::event_counting_subscriber()
    }

    #[test]
    fn probe_attempts_log_nothing() {
        // Probe-fixable doc through the REAL seam: pass-1 fails quietly, the
        // probe succeeds, no final-failure replay runs — zero ERROR events.
        let raw = "routes:\n  - id: probe-log\n    from: \"direct:start\"\n    steps:\n      - throttle:\n          max_requests: ${env:RC_PROBE_LOG:-2}\n          period_secs: 1\n";
        let (counts, _guard) = error_counting_subscriber();
        let routes = crate::yaml::parse_routes_with_env(raw, &|_| None).unwrap();
        assert_throttle_max_requests(&routes, 2);
        assert_eq!(
            counts.error(),
            0,
            "successful probe load must log zero error-level events"
        );
    }

    #[test]
    fn probe_final_failure_logs_exactly_one_error() {
        // All-attempts-fail doc (negative default at an unsigned position):
        // every quiet attempt fails, then the seam replays the text through
        // the logging parser exactly once — EXACTLY ONE error event.
        let raw = "routes:\n  - id: probe-fail\n    from: \"direct:start\"\n    steps:\n      - throttle:\n          max_requests: ${env:RC_PROBE_FAIL:--2}\n          period_secs: 1\n";
        let (counts, _guard) = error_counting_subscriber();
        assert!(crate::yaml::parse_routes_with_env(raw, &|_| None).is_err());
        assert_eq!(
            counts.error(),
            1,
            "final-failure replay must log exactly one error event"
        );
    }

    #[test]
    fn probe_later_conversion_failure_logs_nothing_extra() {
        // Doc that survives from_str but fails a LATER conversion stage
        // (unsupported throttle strategy), with a clean-int candidate so
        // the probe actually runs and every coerced subset still fails:
        // the seam must produce exactly the error events the direct
        // logging parse produces on the same doc (none) — no
        // probe-attempt amplification.
        let raw = "routes:\n  - id: later-stage\n    from: \"direct:start\"\n    steps:\n      - set_header:\n          key: k\n          value: ${env:RC_LATER:-123}\n      - throttle:\n          max_requests: 3\n          strategy: bogus\n";

        let (baseline_counts, _baseline_guard) = error_counting_subscriber();
        assert!(crate::yaml::parse_yaml(
            "routes:\n  - id: later-stage\n    from: \"direct:start\"\n    steps:\n      - set_header:\n          key: k\n          value: \"123\"\n      - throttle:\n          max_requests: 3\n          strategy: bogus\n"
        )
        .is_err());
        let baseline = baseline_counts.error();

        let (counts, _guard) = error_counting_subscriber();
        assert!(crate::yaml::parse_routes_with_env(raw, &|_| None).is_err());
        assert_eq!(
            counts.error(),
            baseline,
            "seam must not amplify logging for later-stage failures"
        );
    }

    // ── Widened scope: typed positions outside `routes:` ────────────────

    /// One REST block with a GET operation; `port_placeholder` is the raw
    /// YAML text under test at the typed `port` position.
    fn rest_doc(port_placeholder: &str) -> String {
        format!(
            "rest:\n  - host: 127.0.0.1\n    port: {port_placeholder}\n    path: /api\n    operations:\n      - method: GET\n        to: direct:list\n"
        )
    }

    fn assert_rest_from_uri(routes: &[RouteDefinition], expected: &str) {
        assert_eq!(routes.len(), 1, "one REST block lowers to one route");
        let uri = routes[0].from_uri();
        assert!(
            uri.contains(expected),
            "expected the lowered REST URI to contain {expected:?}, got: {uri}"
        );
    }

    #[test]
    fn probe_fixes_rest_port_default() {
        let routes = probe_defaults(&rest_doc("${env:RC_REST_PORT:-8080}"))
            .expect("the widened probe must fix the rest port");
        assert_rest_from_uri(&routes, "http://127.0.0.1:8080/api");
    }

    #[test]
    fn probe_fixes_rest_port_from_env_lookup() {
        let lookup = |name: &str| (name == "RC_REST_PORT").then(|| "9090".to_string());
        let routes = probe_with_lookup(&rest_doc("${env:RC_REST_PORT:-8080}"), &lookup)
            .expect("the widened probe must fix the looked-up rest port");
        assert_rest_from_uri(&routes, ":9090/api");
    }

    fn assert_first_pass_unsigned_type_mismatch(err: CamelError, context: &str) {
        let msg = err.to_string();
        assert!(
            msg.contains("type mismatch")
                && msg.contains("expected unsigned integer")
                && msg.contains("found string"),
            "{context}: expected the first-pass type-mismatch wording, got: {msg}"
        );
    }

    #[test]
    fn probe_rest_port_notanumber_and_leading_zero_and_overflow_fail() {
        for default in ["notanumber", "007", "99999999999999999999999"] {
            let raw = rest_doc(&format!("${{env:RC_REST_PORT:-{default}}}"));
            let err = match probe_defaults(&raw) {
                Ok(_) => panic!("non-clean port default `{default}` must keep the pass-1 error"),
                Err(e) => e,
            };
            assert_first_pass_unsigned_type_mismatch(err, default);
        }
    }

    #[test]
    fn probe_rest_port_embedded_token_fails() {
        let err = match probe_defaults(&rest_doc("p-${env:RC_EMB:-8080}")) {
            Ok(_) => panic!("an embedded token is not a whole-scalar candidate"),
            Err(e) => e,
        };
        assert_first_pass_unsigned_type_mismatch(err, "embedded token");
    }

    #[test]
    fn probe_rest_port_unresolved_names_variable() {
        match crate::yaml::parse_routes_with_env(&rest_doc("${env:RC_MISSING}"), &|_| None) {
            Err(crate::yaml::RoutesEnvError::Unresolved(var)) => assert_eq!(var, "RC_MISSING"),
            Err(other) => panic!("expected the named-variable error, got: {other}"),
            Ok(_) => panic!("an unresolved placeholder must not load"),
        }
    }

    #[test]
    fn probe_rest_negative_port_fails() {
        let err = match probe_defaults(&rest_doc("${env:RC_REST_NEG:--2}")) {
            Ok(_) => panic!("a negative default can never satisfy u16"),
            Err(e) => e,
        };
        assert_first_pass_unsigned_type_mismatch(err, "negative default");
    }

    /// `route_candidates` routes with one throttle candidate each, plus
    /// `rest_blocks` REST blocks with one port candidate each.
    fn routes_plus_rest_doc(route_candidates: usize, rest_blocks: usize) -> String {
        let mut doc = String::from("routes:\n");
        for i in 0..route_candidates {
            doc.push_str(&format!(
                "  - id: share-{i}\n    from: \"direct:start\"\n    steps:\n      - throttle:\n          max_requests: ${{env:RC_SHARE_R{i}:-{}}}\n          period_secs: 1\n",
                i + 1
            ));
        }
        doc.push_str("rest:\n");
        for i in 0..rest_blocks {
            doc.push_str(&format!(
                "  - host: 127.0.0.1\n    port: ${{env:RC_SHARE_P{i}:-{}}}\n    path: /share{i}\n    operations:\n      - method: GET\n        to: direct:share{i}\n",
                8100 + i
            ));
        }
        doc
    }

    #[test]
    fn probe_cap_shared_across_routes_and_rest() {
        assert!(
            probe_defaults(&routes_plus_rest_doc(4, 5)).is_err(),
            "nine combined candidates from probed subtrees exceed the shared cap"
        );

        let routes = probe_defaults(&routes_plus_rest_doc(4, 4))
            .expect("eight combined candidates stay within the shared cap");
        assert_eq!(routes.len(), 8, "four plain routes plus four REST routes");
        for (i, route) in routes.iter().take(4).enumerate() {
            match &route.steps()[0] {
                camel_core::route::BuilderStep::Throttle { config, .. } => {
                    assert_eq!(config.max_requests, i + 1, "route {i}");
                }
                other => panic!("expected throttle step {i}, got: {other:?}"),
            }
        }
        for (i, route) in routes.iter().enumerate().skip(4) {
            let block = i - 4;
            let port = 8100 + block;
            let uri = route.from_uri();
            assert!(
                uri.contains(&format!(":{port}/share{block}")),
                "REST route {i} must carry the coerced port, got: {uri}"
            );
        }
    }

    #[test]
    fn probe_cap_boundary_schema_token_fails() {
        let raw = format!(
            "$schema: ${{env:RC_SCHEMA:-1}}\n{}",
            multi_candidate_doc(8, "capb")
        );
        assert!(
            probe_defaults(&raw).is_err(),
            "the numeric-looking $schema token pushes the candidacy to nine"
        );
    }

    #[test]
    fn probe_request_schema_compile_required_coerces() {
        let raw = "rest:\n  - host: 127.0.0.1\n    port: ${env:RC_RS_PORT:-8080}\n    path: /api\n    operations:\n      - method: POST\n        to: direct:create\n        request_schema: ${env:RC_RS_SCHEMA:-true}\n";
        let (text, provenance) =
            interpolate_yaml_source_with_provenance(raw, &|_| None).expect("interpolation");
        let provenance = provenance.expect("tree-walk path carries provenance");
        let winning_text = std::cell::RefCell::new(None);
        let routes = parse_with_probe(&text, Some(&provenance), |probe_text| {
            let result = crate::yaml::parse_yaml_for_probe(
                probe_text,
                &crate::yaml::ScriptLoadContext::new(None),
            );
            if result.is_ok() {
                *winning_text.borrow_mut() = Some(probe_text.to_string());
            }
            result
        })
        .expect("the compile-required schema position must coerce alongside the port");
        assert_rest_from_uri(&routes, "http://127.0.0.1:8080/api");

        let winning_text = winning_text.borrow();
        let winning = winning_text
            .as_ref()
            .expect("a parsing subset produced the winning text");
        let dsl: crate::route_ast::RouteDslRoutes =
            serde_yml::from_str(winning).expect("the winning text deserializes");
        assert_eq!(
            dsl.rest[0].operations[0].request_schema,
            Some(serde_json::json!(true)),
            "the schema leaf must carry the boolean, not the string"
        );
    }

    #[test]
    fn probe_templated_routes_parameter_stays_string() {
        let raw = format!(
            "{}templated_routes:\n  - route_template_ref: tpl\n    parameters:\n      delay_ms: ${{env:RC_TR_P:-500}}\n",
            multi_candidate_doc(8, "cap-tr")
        );
        let (text, provenance) =
            interpolate_yaml_source_with_provenance(&raw, &|_| None).expect("interpolation");
        let provenance = provenance.expect("tree-walk path carries provenance");
        let (routes, winning_text) = run_probe_with_text::<YamlDoc, Vec<RouteDefinition>>(
            &text,
            &provenance,
            |probe_text| {
                crate::yaml::parse_yaml_for_probe(
                    probe_text,
                    &crate::yaml::ScriptLoadContext::new(None),
                )
            },
        )
        .expect("eight routes candidates must still load — the parameter is excluded");
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].steps().len(),
            8,
            "every routes: throttle candidate was coerced"
        );

        // The seam above proves the document LOADS; template parameters are
        // not carried by `Vec<RouteDefinition>`, so assert the typed form
        // through the document AST: `parameters` is a `BTreeMap<String,
        // String>` by contract and the coerced number must never appear.
        let parsed: crate::route_ast::RouteDslRoutes =
            serde_yml::from_str(&winning_text).expect("the winning text deserializes");
        assert_eq!(
            parsed.templated_routes[0].parameters["delay_ms"], "500",
            "templated_routes parameters must stay the STRING \"500\""
        );
        assert_eq!(parsed.routes[0].steps.len(), 8);

        let doc = YamlDoc::from_text(&text).expect("interpolated text parses");
        let parameter_path = vec![
            ProvenanceSeg::Key("templated_routes".into()),
            ProvenanceSeg::Index(0),
            ProvenanceSeg::Key("parameters".into()),
            ProvenanceSeg::Key("delay_ms".into()),
        ];
        assert_eq!(
            doc.string_leaf_at(&parameter_path),
            Some("500"),
            "templated_routes parameters keep the STRING value"
        );
    }

    #[test]
    fn probe_below_cap_stale_provenance_yaml_only_fails() {
        // The reproduced compat-boundary document (design §3): the
        // substituted-key entry records the `max_requests` path, then a
        // LITERAL `max_requests` entry overwrites it with the SAME value.
        // The insert-time collision guard drops the stale path, so no
        // candidate survives below the cap and the first-pass error stands.
        let raw = "routes:\n  - id: stale-prov\n    from: \"direct:start\"\n    steps:\n      - throttle:\n          '${env:KEY:-max_requests}': '${env:VALUE:-404}'\n          max_requests: '404'\n          period_secs: 1\n";
        let err = match crate::yaml::parse_routes_with_env(raw, &|_| None) {
            Ok(_) => panic!("a stale substituted path must not probe-load the literal leaf"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("did not match any variant"),
            "expected the pass-1 throttle error, got: {err}"
        );
    }

    // ── Winning-text handoff (task 1.11 / bd rc-3c67o.2.4) ──────────────

    #[test]
    fn run_probe_with_text_returns_the_winning_serialization() {
        // The handoff contract: ONE bounded search, and the text it returns
        // is EXACTLY the serialization the oracle accepted — downstream
        // consumers (the JSON template extractors) must never see a document
        // reconstructed by a second parse or a post-success re-serialization.
        let raw = r#"{"rest":[{"port":"${env:RC_PROBE_TEXT:-8085}"}]}"#;
        let (spliced, provenance) =
            crate::env_interpolation::interpolate_json_source_with_provenance(raw, &|_| None)
                .expect("interpolation");
        let provenance = provenance.expect("tree-walk path carries provenance");
        let accepted: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);

        let (value, winning) =
            run_probe_with_text::<JsonDoc, serde_json::Value>(&spliced, &provenance, |candidate| {
                let tree: serde_json::Value = serde_json::from_str(candidate)
                    .map_err(|error| CamelError::RouteError(error.to_string()))?;
                if !tree["rest"][0]["port"].is_number() {
                    return Err(CamelError::RouteError("port is still a string".to_string()));
                }
                *accepted.borrow_mut() = Some(candidate.to_string());
                Ok(tree)
            })
            .expect("the clean default coerces to a number");

        assert_eq!(value["rest"][0]["port"], 8085);
        assert_eq!(
            accepted.borrow().as_deref(),
            Some(winning.as_str()),
            "the returned text must be the serialization the oracle accepted"
        );
        assert_ne!(
            winning, spliced,
            "a repaired document differs from the interpolated splice"
        );
    }
}
