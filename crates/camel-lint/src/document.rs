//! [`Document`] — a parsed route source with a span-carrying route view.
//!
//! Parsing ALWAYS yields a [`Document`] (never `Err`): a syntax error is
//! data surfaced through [`ParseFailure`] for the R-SYN rule, not an engine
//! error. On success the noyalib CST is walked to build [`LintRoute`],
//! capturing every URI-bearing location with byte-exact spans.

use std::collections::HashSet;
use std::sync::LazyLock;

use noyalib::Value;
use noyalib::cst;

use crate::ROUTE_SCHEMA;
use crate::diagnostic::{Fix, Span};
use crate::error::LintError;
use crate::route_view::{Endpoint, LintNode, LintOption, LintRoute, OptionOrigin, Spanned};

// ---------------------------------------------------------------------------
// ParseFailure + Document
// ---------------------------------------------------------------------------

/// A syntax error encountered during parsing, carrying a byte-exact span and
/// the parser's message.
#[derive(Clone, Debug)]
pub struct ParseFailure {
    pub span: Span,
    pub message: String,
}

/// The parsed route document: raw source text, the span-carrying route view,
/// and an optional parse failure.
#[derive(Clone, Debug)]
pub struct Document {
    pub raw: String,
    pub route_view: LintRoute,
    pub parse_failure: Option<ParseFailure>,
}

impl Document {
    /// Parse `source` into a [`Document`].
    ///
    /// On a syntax error the result carries `parse_failure = Some(_)` (with a
    /// byte-exact span and the parser's message) and an empty route view.
    /// This function never returns `Err`.
    pub fn parse(source: &str) -> Document {
        let raw = source.to_string();
        match cst::parse_document(source) {
            Ok(noya_doc) => {
                // Clone the typed value out of the CST cache Ref so span
                // queries (which may repopulate the cache on the fallback
                // path) never conflict with a held borrow.
                let root: Value = (*noya_doc.as_value()).clone();
                let mut from = None;
                let mut from_parameters = Vec::new();
                let nodes = walk(
                    &root,
                    "",
                    &noya_doc,
                    &mut from,
                    &mut from_parameters,
                    &[],
                    OptionOrigin::StepParameters,
                );
                Document {
                    raw,
                    route_view: LintRoute {
                        from,
                        from_parameters,
                        nodes,
                    },
                    parse_failure: None,
                }
            }
            Err(err) => {
                let (span, message) = failure_span(&err, source);
                Document {
                    raw,
                    route_view: LintRoute::default(),
                    parse_failure: Some(ParseFailure { span, message }),
                }
            }
        }
    }

    /// Apply a raw byte-range edit to this document.
    ///
    /// Replaces `[start, end)` in the source with `replacement`, re-parses the
    /// result, and **always commits** the new state — including when the
    /// re-parse produces a [`ParseFailure`]. This mirrors an editor's live
    /// state: intermediate edits routinely produce invalid syntax, and the
    /// document must reflect the editor's actual text so R-SYN can report the
    /// syntax error.
    ///
    /// Returns `Err` ONLY for structural problems that prevent applying the
    /// edit at all: an out-of-bounds range, a non-character-boundary offset,
    /// or (when the CST path is used) a `replace_span` rejection. On `Err`
    /// the document is left unchanged.
    ///
    /// This is the low-level edit primitive. [`apply_fix`](Document::apply_fix)
    /// delegates to it for the byte replacement but adds a transactional
    /// rollback when the result has a `parse_failure`.
    pub fn apply_edit(
        &mut self,
        start: usize,
        end: usize,
        replacement: &str,
    ) -> Result<(), LintError> {
        // Try the CST path first: it preserves span fidelity. When the
        // current source cannot be parsed (e.g. during an in-progress editor
        // edit), fall back to raw string manipulation.
        let new_raw = match cst::parse_document(&self.raw) {
            Ok(mut cst_doc) => {
                cst_doc
                    .replace_span(start, end, replacement)
                    .map_err(|e| LintError::Internal(format!("apply_edit edit rejected: {e}")))?;
                cst_doc.source().to_string()
            }
            Err(_) => {
                // CST parse failed (source currently has parse_failure). The
                // always-commits contract requires applying edits even to broken
                // documents (spec scenario "apply_edit recovers invalid→valid"),
                // so fall back to raw byte-splicing instead of returning Err.
                if start > self.raw.len() || end > self.raw.len() || start > end {
                    return Err(LintError::Internal(format!(
                        "apply_edit edit rejected: range ({start}, {end}) out of bounds for source length {}",
                        self.raw.len()
                    )));
                }
                if !self.raw.is_char_boundary(start) || !self.raw.is_char_boundary(end) {
                    return Err(LintError::Internal(format!(
                        "apply_edit edit rejected: range ({start}, {end}) not on character boundary"
                    )));
                }
                let mut s =
                    String::with_capacity(self.raw.len() - (end - start) + replacement.len());
                s.push_str(&self.raw[..start]);
                s.push_str(replacement);
                s.push_str(&self.raw[end..]);
                s
            }
        };

        let reparsed = Document::parse(&new_raw);
        self.raw = reparsed.raw;
        self.route_view = reparsed.route_view;
        self.parse_failure = reparsed.parse_failure;
        Ok(())
    }

    /// Apply a suggested [`Fix`] to this document.
    ///
    /// Substitutes `fix.replacement` into `fix.span` via [`apply_edit`], then
    /// checks the result: if the re-parse produces a [`ParseFailure`], the edit
    /// is **rolled back** (the document is restored to its pre-edit state) and
    /// an `Err` is returned. Automated fixes must never break syntax.
    ///
    /// On an out-of-bounds or non-character-boundary span — or any other error
    /// from `apply_edit` — returns [`LintError::Internal`] and leaves the
    /// document unchanged.
    ///
    /// This is a document-level operation: the engine is stateless and never
    /// retains a `Document`. A caller applies a fix with `doc.apply_fix(&fix)`
    /// and then re-runs `engine.lint(&doc.raw)` to obtain refreshed
    /// diagnostics.
    pub fn apply_fix(&mut self, fix: &Fix) -> Result<(), LintError> {
        let pre_edit = self.clone();
        match self.apply_edit(fix.span.start, fix.span.end, &fix.replacement) {
            Ok(()) => {
                if self.parse_failure.is_some() {
                    // Roll back: the fix broke syntax.
                    *self = pre_edit;
                    return Err(LintError::Internal(
                        "apply_fix produced invalid syntax".into(),
                    ));
                }
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
}

/// Map a parser [`noyalib::Error`] to a byte-exact span and its message.
///
/// The span points at the parser-reported byte index (single-byte width, so
/// downstream renderers underline exactly one character). When the error has
/// no location the span collapses to the document start.
fn failure_span(err: &noyalib::Error, source: &str) -> (Span, String) {
    let message = err.to_string();
    let start = err.location().map_or(0, |loc| loc.index());
    let end = source.len().min(start + 1);
    (Span::new(start, end), message)
}

// ---------------------------------------------------------------------------
// URI keys (allowlist) + container keys (schema-derived)
// ---------------------------------------------------------------------------

/// The closed set of field names whose string value is an endpoint URI
/// (`scheme:target`), not an id, ref, expression, or class name.
///
/// A `type: string` subschema cannot distinguish a URI from an id/ref/
/// expression, so URI leaves are an explicit allowlist. Containers (fields
/// holding nested steps) are still discovered from the schema, where the
/// type/shape classification is sound.
const URI_KEYS: &[&str] = &[
    "from",                // route source endpoint (captured into `from`)
    "to",                  // ToStep endpoint URI
    "uri",                 // EnrichConfig.uri (enrich/poll_enrich full form)
    "wire_tap",            // WireTapStep endpoint URI
    "enrich",              // EnrichStep — shorthand string is an endpoint URI
    "poll_enrich",         // PollEnrichStep — shorthand string is an endpoint URI
    "endpoints",           // ScatterGatherData — array of endpoint URIs
    "dead_letter_channel", // RouteDslErrorHandler — dead-letter endpoint URI
];

/// Container property names derived from [`ROUTE_SCHEMA`]: object or
/// array-of-object fields that hold nested steps or sub-config. Re-deriving
/// from the embedded schema means a new container is picked up by re-syncing
/// the schema copy — no code change needed.
static CONTAINER_KEYS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    let schema: serde_json::Value =
        serde_json::from_str(ROUTE_SCHEMA).expect("embedded route schema is valid JSON"); // allow-unwrap
    container_keys(&schema)
});

/// Collect every container property name from the schema. Resolves local
/// `$ref` pointers into `$defs`.
fn container_keys(schema: &serde_json::Value) -> HashSet<String> {
    let mut container = HashSet::new();
    let mut visited_refs = HashSet::new();
    descend_schema(schema, schema, &mut container, &mut visited_refs);
    container
}

/// Resolve a local `$ref` (e.g. `#/$defs/step`) against the schema root.
/// Returns the original node when there is no `$ref`. Chained refs resolve
/// recursively.
fn resolve_ref<'a>(
    node: &'a serde_json::Value,
    root: &'a serde_json::Value,
) -> &'a serde_json::Value {
    if let Some(rf) = node.get("$ref").and_then(|v| v.as_str())
        && let Some(frag) = rf.strip_prefix("#/")
    {
        let mut cur = root;
        for part in frag.split('/') {
            cur = cur.get(part).unwrap_or(&serde_json::Value::Null);
        }
        return resolve_ref(cur, root);
    }
    node
}

/// Recursively descend through `properties`, `items`, `additionalProperties`,
/// and the composition keywords (`anyOf`/`oneOf`/`allOf`), classifying each
/// named property that is a container. `visited_refs` guards against
/// recursive `$ref` cycles.
fn descend_schema(
    node: &serde_json::Value,
    root: &serde_json::Value,
    container: &mut HashSet<String>,
    visited_refs: &mut HashSet<String>,
) {
    // Cycle guard on raw $ref nodes.
    if let Some(rf) = node.get("$ref").and_then(|v| v.as_str())
        && !visited_refs.insert(rf.to_string())
    {
        return;
    }

    let r = resolve_ref(node, root);

    // Composition keywords: descend each subschema (raw, so $ref tracking
    // fires) but do NOT classify the composition node itself.
    for kw in ["allOf", "anyOf", "oneOf"] {
        if let Some(arr) = r.get(kw).and_then(|v| v.as_array()) {
            for sub in arr {
                descend_schema(sub, root, container, visited_refs);
            }
        }
    }

    if let Some(props) = r.get("properties").and_then(|v| v.as_object()) {
        for (name, sub) in props {
            let resolved = resolve_ref(sub, root);
            if is_container(resolved, root) {
                container.insert(name.clone());
            }
            // Recurse into the property's subschema (raw) to find nested keys.
            descend_schema(sub, root, container, visited_refs);
        }
    }

    if let Some(items) = r.get("items") {
        descend_schema(items, root, container, visited_refs);
    }

    if let Some(ap) = r.get("additionalProperties")
        && ap.is_object()
    {
        descend_schema(ap, root, container, visited_refs);
    }
}

/// True when the subschema is an object, or an array whose items are objects,
/// or a composition leading to an object — i.e. it holds further steps.
fn is_container(node: &serde_json::Value, root: &serde_json::Value) -> bool {
    let r = resolve_ref(node, root);
    if r.get("type") == Some(&serde_json::Value::String("object".into()))
        || r.get("properties").is_some()
    {
        return true;
    }
    if r.get("type") == Some(&serde_json::Value::String("array".into()))
        && let Some(items) = r.get("items")
    {
        let ri = resolve_ref(items, root);
        if ri.get("type") == Some(&serde_json::Value::String("object".into()))
            || ri.get("properties").is_some()
            || ri.get("$ref").is_some()
            || ["anyOf", "oneOf", "allOf"]
                .iter()
                .any(|kw| ri.get(kw).is_some())
        {
            return true;
        }
    }
    for kw in ["anyOf", "oneOf", "allOf"] {
        if let Some(subs) = r.get(kw).and_then(|v| v.as_array())
            && subs.iter().any(|s| is_container(s, root))
        {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// CST walker
// ---------------------------------------------------------------------------

/// Build the spanned node list for `value`, emitting endpoints for [`URI_KEYS`]
/// and recursing through container keys discovered from the schema. `from`
/// captures the route-level `from` URI (first occurrence wins) so it is not
/// also emitted as a step node; `from_parameters` captures the sibling
/// `parameters:` entries attached to that `from`. `inherited` carries any
/// step-level `parameters:` entries into an object-form URI key (e.g.
/// `enrich: { uri: ... }`) and is CONCATENATED with the inner config's own
/// `parameters:` map, so entries from both reach the nested `uri` endpoint
/// (the DSL lowerer merges disjoint keys, so dropping either side would miss
/// rules and could false-flag `MissingRequiredOption`).
fn walk(
    value: &Value,
    path: &str,
    doc: &cst::Document,
    from_slot: &mut Option<Spanned<String>>,
    from_parameters: &mut Vec<LintOption>,
    inherited: &[LintOption],
    local_origin: OptionOrigin,
) -> Vec<Spanned<LintNode>> {
    let mut nodes = Vec::new();
    match value {
        Value::Mapping(m) => {
            // Collect the sibling `parameters:` map (if any) into spanned
            // options. They attach to every endpoint URI key emitted from this
            // mapping (or, for the route-level `from`, into `from_parameters`).
            // Their origin is the caller's `local_origin`: StepParameters for
            // a mapping that is a step (or the document root), ConfigParameters
            // for the mapping inside an object-form URI key.
            let local: Vec<LintOption> = m
                .get("parameters")
                .map(|pv| {
                    collect_parameters(pv, &child_path(path, "parameters"), doc, local_origin)
                })
                .unwrap_or_default();
            // Concatenate, never replace: step-level `parameters:` (inherited)
            // and the inner config map (`local`) both reach the endpoint.
            // Containers/sequences reset `inherited` to `[]`, so the concat is
            // safe at every call site; the root call passes `&[]`.
            let effective: Vec<LintOption> = inherited.iter().cloned().chain(local).collect();

            for (key, child) in m.iter() {
                let cpath = child_path(path, key);
                let k = key.as_str();
                // The parameters map is consumed above; never treat it as a
                // container (which would emit a spurious empty Branch) or a
                // URI key.
                if k == "parameters" {
                    continue;
                }
                // Route 1's scalar `from` is captured in the dedicated slot.
                // Routes 2..N (from_slot already set) and the object form
                // (child.as_str() returns None) fall through to the URI_KEYS
                // handler below so the URI is still emitted as an endpoint
                // node and validated by rules.
                if k == "from"
                    && from_slot.is_none()
                    && let Some(s) = child.as_str()
                    && let Some((start, end)) = doc.span_at(&cpath)
                {
                    *from_slot = Some(Spanned {
                        value: s.to_string(),
                        span: Span::new(start, end),
                    });
                    *from_parameters = effective.clone();
                    continue;
                }
                if URI_KEYS.contains(&k) {
                    if matches!(child, Value::Mapping(_)) {
                        // Object form (e.g. enrich: { uri: ... }): recurse to
                        // find the nested `uri`, which is itself a URI key,
                        // carrying the step-level parameters alongside the
                        // inner config's own `parameters:` map. The inner
                        // map's entries are ConfigParameters.
                        nodes.extend(walk(
                            child,
                            &cpath,
                            doc,
                            from_slot,
                            from_parameters,
                            &effective,
                            OptionOrigin::ConfigParameters,
                        ));
                    } else {
                        emit_endpoints(child, &cpath, doc, &mut nodes, &effective);
                    }
                } else if CONTAINER_KEYS.contains(k) {
                    let children = walk(
                        child,
                        &cpath,
                        doc,
                        from_slot,
                        from_parameters,
                        &[],
                        OptionOrigin::StepParameters,
                    );
                    let kind = Spanned {
                        value: key.clone(),
                        span: key_span_for(doc, &cpath),
                    };
                    let span = value_span_for(doc, &cpath);
                    nodes.push(Spanned {
                        value: LintNode::Branch { kind, children },
                        span,
                    });
                }
                // Non-URI, non-container keys are ignored.
            }
        }
        Value::Sequence(seq) => {
            for (i, item) in seq.iter().enumerate() {
                let ipath = format!("{path}[{i}]");
                nodes.extend(walk(
                    item,
                    &ipath,
                    doc,
                    from_slot,
                    from_parameters,
                    &[],
                    OptionOrigin::StepParameters,
                ));
            }
        }
        _ => {}
    }
    nodes
}

/// Collect the entries of a `parameters:` mapping into [`LintOption`]s, each
/// key and value carrying a byte-exact span into the original source.
///
/// A non-string value (rejected by the schema as `additionalProperties: string`)
/// yields an option with `value: None`; the key is still captured so R-URI-known
/// can resolve it.
fn collect_parameters(
    value: &Value,
    path: &str,
    doc: &cst::Document,
    origin: OptionOrigin,
) -> Vec<LintOption> {
    let Value::Mapping(m) = value else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, val) in m.iter() {
        let entry_path = child_path(path, key.as_str());
        let key_span = doc
            .key_span(&entry_path)
            .map(|(s, e)| Span::new(s, e))
            .unwrap_or_else(|| value_span_for(doc, &entry_path));
        let value_span = value_span_for(doc, &entry_path);
        let value = val.as_str().map(|s| Spanned {
            value: s.to_string(),
            span: value_span,
        });
        out.push(LintOption {
            key: Spanned {
                value: key.clone(),
                span: key_span,
            },
            value,
            origin,
        });
    }
    out
}

/// Emit one [`Endpoint`] per string value: a single string (SCALAR-URI) yields
/// one endpoint; a sequence of strings (URI-ARRAY) yields one per item. `params`
/// are the sibling `parameters:` entries appended to each endpoint's options.
fn emit_endpoints(
    value: &Value,
    path: &str,
    doc: &cst::Document,
    nodes: &mut Vec<Spanned<LintNode>>,
    params: &[LintOption],
) {
    match value {
        Value::String(s) => {
            if let Some(ep) = endpoint_for(s, path, doc, params) {
                nodes.push(ep);
            }
        }
        Value::Sequence(seq) => {
            for (i, item) in seq.iter().enumerate() {
                if let Value::String(s) = item {
                    let ipath = format!("{path}[{i}]");
                    if let Some(ep) = endpoint_for(s, &ipath, doc, params) {
                        nodes.push(ep);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Build an [`Endpoint`] node for a single URI string at `path`, parsing its
/// query-string options against the original source and appending `params`
/// (the sibling `parameters:` entries) after them.
fn endpoint_for(
    uri: &str,
    path: &str,
    doc: &cst::Document,
    params: &[LintOption],
) -> Option<Spanned<LintNode>> {
    let (start, end) = doc.span_at(path)?;
    let span = Span::new(start, end);
    let mut options = LintOption::parse_from_query(uri, span.clone());
    options.extend(params.iter().cloned());
    Some(Spanned {
        value: LintNode::Endpoint(Endpoint {
            key: Spanned {
                value: endpoint_key(path),
                span: span.clone(),
            },
            uri: Spanned {
                value: uri.to_string(),
                span: span.clone(),
            },
            options,
        }),
        span,
    })
}

/// Derive the endpoint's origin key from its noyalib query `path`.
///
/// Takes the FINAL dot-delimited segment of the path, then strips a terminal
/// `[i]` array index if present — so `routes[0]...steps.to` → `to`,
/// `...endpoints[1]` → `endpoints`, and object-form `enrich.uri` → `uri`.
fn endpoint_key(path: &str) -> String {
    let last = path.rsplit('.').next().unwrap_or(path);
    let key = match last.rfind('[') {
        Some(idx) if last.ends_with(']') => &last[..idx],
        _ => last,
    };
    key.to_string()
}

/// Join a parent path and a mapping key into a noyalib query path.
fn child_path(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        key.to_string()
    } else {
        format!("{parent}.{key}")
    }
}

/// Byte span of a container key token, falling back to its value span.
pub(crate) fn key_span_for(doc: &cst::Document, path: &str) -> Span {
    match doc.key_span(path) {
        Some((s, e)) => Span::new(s, e),
        None => value_span_for(doc, path),
    }
}

/// Byte span of the value at `path`, falling back to a zero span at origin.
pub(crate) fn value_span_for(doc: &cst::Document, path: &str) -> Span {
    match doc.span_at(path) {
        Some((s, e)) => Span::new(s, e),
        None => Span::new(0, 0),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn slice_at<'a>(raw: &'a str, span: &Span) -> &'a str {
        &raw[span.start..span.end]
    }

    #[test]
    fn from_uri_span_is_byte_exact() {
        // The probe confirmed `direct:start` starts at byte offset 6 here
        // (the task brief's "12" is a miscount). The assertion is written
        // against the source slice, so it is byte-exact regardless of the
        // hand-counted number.
        let source = "from: direct:start\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let from = doc.route_view.from.as_ref().expect("from must be captured");
        assert_eq!(&source[from.span.start..from.span.end], "direct:start");
        assert_eq!(from.span.start, 6, "`direct:start` begins at byte 6");
        assert_eq!(from.value, "direct:start");
    }

    #[test]
    fn nested_child_step_uri_captured() {
        // Uses multicast (not pipeline — pipeline does not exist in the schema).
        let source = "from: direct:start\nsteps:\n  - multicast:\n      - to: log:nested\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none());
        let endpoints = doc.route_view.endpoints();
        // [0] = from (direct:start), [1] = nested child (log:nested).
        assert_eq!(endpoints.len(), 2, "expected from + one nested endpoint");
        let child = &endpoints[1];
        assert_eq!(child.uri.value, "log:nested");
        // The child's span is byte-exact and distinct from the parent from.
        assert_eq!(slice_at(&doc.raw, &child.uri.span), "log:nested");
        assert_ne!(child.uri.span, endpoints[0].uri.span);
    }

    #[test]
    fn scatter_gather_endpoints_captured() {
        let source = "from: direct:start\nsteps:\n  - scatter_gather:\n      endpoints:\n        - direct:a\n        - direct:b\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none());
        let endpoints = doc.route_view.endpoints();
        // from + direct:a + direct:b.
        assert_eq!(endpoints.len(), 3, "expected from + two scatter endpoints");
        assert_eq!(endpoints[1].uri.value, "direct:a");
        assert_eq!(endpoints[2].uri.value, "direct:b");
        // Each item carries its own distinct span.
        assert_eq!(slice_at(&doc.raw, &endpoints[1].uri.span), "direct:a");
        assert_eq!(slice_at(&doc.raw, &endpoints[2].uri.span), "direct:b");
        assert_ne!(endpoints[1].uri.span, endpoints[2].uri.span);
    }

    #[test]
    fn option_key_value_spans_byte_exact() {
        // The probe confirmed `period` at byte 25 and `1s` at byte 32 in this
        // source (the task brief's "30/37" are miscounts). Assertions slice
        // the source so they are byte-exact independent of the literal.
        let source = "steps:\n  - to: timer:foo?period=1s\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none());
        let endpoints = doc.route_view.endpoints();
        assert_eq!(endpoints.len(), 1);
        let opts = &endpoints[0].options;
        assert_eq!(opts.len(), 1, "one query option expected");
        let opt = &opts[0];
        assert_eq!(opt.key.value, "period");
        assert_eq!(slice_at(&doc.raw, &opt.key.span), "period");
        let val = opt.value.as_ref().expect("option must have a value span");
        assert_eq!(val.value, "1s");
        assert_eq!(slice_at(&doc.raw, &val.span), "1s");
    }

    #[test]
    fn partial_input_records_failure_span() {
        let source = "steps:\n  - to: timer:foo\n  bad: [";
        let doc = Document::parse(source);
        let failure = doc
            .parse_failure
            .as_ref()
            .expect("malformed input must set parse_failure");
        assert!(
            !failure.message.is_empty(),
            "parser message must be carried"
        );
        // Span is a valid range within the source (point span allowed).
        assert!(failure.span.end <= source.len());
        assert!(failure.span.start <= failure.span.end);
        // Route view is empty.
        assert!(doc.route_view.from.is_none());
        assert!(doc.route_view.nodes.is_empty());
    }

    // ---- engine behavior tests (moved here from Task 1.2 per the spec) ----

    #[test]
    fn engine_with_no_rules_returns_empty() {
        use crate::engine::LintEngine;
        use crate::test_support::StubCatalog;
        use std::sync::Arc;

        let engine = LintEngine::new(Arc::new(StubCatalog::empty()));
        let diags = engine.lint("from: direct:start\nsteps:\n  - to: log:out\n");
        assert!(diags.is_empty(), "no rules => no diagnostics");
    }

    #[test]
    fn engine_tolerates_partial_input() {
        use crate::engine::LintEngine;
        use crate::test_support::StubCatalog;
        use std::sync::Arc;

        let engine = LintEngine::new(Arc::new(StubCatalog::empty()));
        // Malformed YAML: must not panic, returns empty (R-SYN reports it
        // only when a rule is registered).
        let diags = engine.lint("from: direct:start\n  unclosed: [");
        assert!(diags.is_empty());
    }

    // ---- apply_edit / apply_fix refactor (Task 1.1) ----

    #[test]
    fn apply_edit_replaces_range() {
        // Byte layout: from: =0-5, direct:=6-12, start=13-17, \n=18
        let mut doc = Document::parse("from: direct:start\n");
        assert!(doc.parse_failure.is_none(), "fixture must parse cleanly");
        doc.apply_edit(13, 18, "end")
            .expect("edit within valid bounds must succeed");
        assert_eq!(doc.raw, "from: direct:end\n");
        assert!(doc.parse_failure.is_none(), "re-parsed doc must be valid");
    }

    #[test]
    fn apply_edit_commits_syntax_breaking_edit() {
        use crate::diagnostic::DiagnosticCode;

        let engine = timer_log_engine();
        let source = "from: direct:start\nsteps:\n  - to: log:out\n";
        let mut doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "fixture must parse cleanly");

        // Replace the `to` endpoint value with `[` — yields an unclosed
        // flow sequence (`to: [`) which breaks YAML syntax.
        let endpoints = doc.route_view.endpoints();
        let to_ep = endpoints
            .iter()
            .find(|e| e.uri.value.starts_with("log:"))
            .expect("log endpoint must be captured");
        doc.apply_edit(to_ep.uri.span.start, to_ep.uri.span.end, "[")
            .expect("syntax-breaking edit must commit (not reject)");

        assert!(
            doc.parse_failure.is_some(),
            "parse_failure must be set after a syntax-breaking edit"
        );
        assert!(doc.raw.contains('['), "raw must reflect the edited text");
        let diags = engine.lint(&doc.raw);
        let syn_count = diags
            .iter()
            .filter(|d| d.code == DiagnosticCode::RSyn)
            .count();
        assert!(
            syn_count >= 1,
            "expected at least one R-SYN diagnostic; got: {diags:?}"
        );
    }

    #[test]
    fn apply_edit_recovers_invalid_to_valid() {
        let source = "steps:\n  - to: timer:foo\n  bad: [";
        let mut doc = Document::parse(source);
        assert!(doc.parse_failure.is_some(), "fixture must be broken");

        // Replace entire content with a valid, minimal route.
        let valid = "from: direct:ok\n";
        doc.apply_edit(0, source.len(), valid)
            .expect("replacing broken content with valid must succeed");

        assert!(
            doc.parse_failure.is_none(),
            "re-parsed doc must be valid after fixing"
        );
        assert_eq!(doc.raw, valid);
        assert!(
            doc.route_view.from.is_some(),
            "route_view must reflect the now-valid structure"
        );
    }

    #[test]
    fn apply_edit_rejects_out_of_bounds() {
        // 20-byte valid source: "from: direct:abcdef\n" = 20 bytes
        let source = "from: direct:abcdef\n";
        assert_eq!(source.len(), 20, "pre-condition: 20-byte source");
        let mut doc = Document::parse(source);
        let original_raw = doc.raw.clone();

        let err = doc
            .apply_edit(0, 25, "x")
            .expect_err("out-of-bounds edit must be rejected");
        assert!(
            matches!(err, crate::error::LintError::Internal(_)),
            "expected LintError::Internal, got: {err:?}"
        );
        assert_eq!(
            doc.raw, original_raw,
            "document must be byte-identical to pre-edit state"
        );
    }

    #[test]
    fn apply_fix_rolls_back_on_parse_failure() {
        use crate::diagnostic::Fix;

        let source = "from: direct:start\nsteps:\n  - to: log:out\n";
        let mut doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "fixture must parse cleanly");
        let original_raw = doc.raw.clone();

        let endpoints = doc.route_view.endpoints();
        let to_ep = endpoints
            .iter()
            .find(|e| e.uri.value.starts_with("log:"))
            .expect("log endpoint must be captured");
        let fix = Fix {
            span: Span::new(to_ep.uri.span.start, to_ep.uri.span.end),
            replacement: "[".to_string(),
        };
        let err = doc
            .apply_fix(&fix)
            .expect_err("syntax-breaking fix must be rejected");
        assert!(
            matches!(err, crate::error::LintError::Internal(_)),
            "expected LintError::Internal, got: {err:?}"
        );
        assert_eq!(
            doc.raw, original_raw,
            "document must be byte-identical after rollback"
        );
    }

    // ---- URI allowlist regression (Task 1.3 fix) ----

    #[test]
    fn non_uri_string_fields_emit_no_endpoints() {
        // type:string fields (id, bean.method/name, catch.exception array,
        // when, kind) must NOT be emitted as endpoints. Only the genuine URIs
        // (route source + the `to` step) count. `catch.exception` is the key
        // array-of-non-uri-strings case.
        let source = "\
from: direct:start
id: r1
steps:
  - bean:
      method: myMethod
      name: myBean
  - do_try:
      steps:
        - to: log:info
      catch:
        - exception:
            - MyException
          when: isError
          kind: someKind
";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let endpoints = doc.route_view.endpoints();
        let uris: Vec<&str> = endpoints.iter().map(|e| e.uri.value.as_str()).collect();
        // Genuine URIs are present.
        assert!(
            uris.contains(&"direct:start"),
            "route source must be captured"
        );
        assert!(
            uris.contains(&"log:info"),
            "the `to` endpoint must be captured"
        );
        // Non-URI string fields must not leak as endpoints.
        for forbidden in [
            "r1",
            "myMethod",
            "myBean",
            "MyException",
            "isError",
            "someKind",
        ] {
            assert!(
                !uris.contains(&forbidden),
                "`{forbidden}` must not be emitted as an endpoint URI"
            );
        }
    }

    // ---- apply_fix (Task 3.3) ----

    /// Build an engine whose catalog knows `direct` (minimal) and `timer`
    /// (with a non-required `period` option), so a `to: timer:foo?bogus=1`
    /// step yields exactly one `UnknownOption` on `bogus`.
    fn timer_log_engine() -> crate::engine::LintEngine {
        use crate::engine::LintEngine;
        use crate::test_support::StubCatalog;
        use camel_api::component_metadata::{ComponentMetadata, OptionKind, UriOption};
        use std::sync::Arc;

        let catalog = StubCatalog::empty()
            .with("direct", ComponentMetadata::minimal("direct"))
            .with(
                "timer",
                ComponentMetadata::minimal("timer").with_uri_options(vec![UriOption::new(
                    "period",
                    "period",
                    OptionKind::Duration,
                )]),
            );
        let catalog: Arc<dyn camel_api::component_metadata::ComponentMetadataCatalog> =
            Arc::new(catalog);
        LintEngine::new(catalog).with_default_rules()
    }

    #[test]
    fn apply_fix_reparses_and_refreshes() {
        use crate::diagnostic::{DiagnosticCode, Fix, UriKnownSubCode};

        let engine = timer_log_engine();
        // The unknown option lives on a `to:` step — `endpoints()` exposes the
        // `from` URI with empty options, so R-URI-known only sees query
        // options on `to`/`uri`/etc. endpoints.
        let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?bogus=1\n";

        // Pre-condition: the catalog reports `bogus` as an unknown option.
        let before = engine.lint(source);
        let unknown_before = before
            .iter()
            .filter(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::UnknownOption))
            .count();
        assert_eq!(
            unknown_before, 1,
            "expected one UnknownOption on `bogus` before the fix; got: {before:?}"
        );

        // Compute the FULL query-segment span `?bogus=1` from the timer
        // endpoint's URI. Replacing only the key `bogus` would leave `?=1`
        // and the diagnostic would persist; spanning the whole segment removes
        // the option.
        let mut doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "fixture must parse cleanly");
        let endpoints = doc.route_view.endpoints();
        let timer_ep = endpoints
            .iter()
            .find(|e| e.uri.value.starts_with("timer:"))
            .expect("timer endpoint must be captured");
        let uri_slice = &doc.raw[timer_ep.uri.span.start..timer_ep.uri.span.end];
        let q_idx = uri_slice
            .find('?')
            .expect("fixture URI has a query segment");
        let seg_span = Span::new(timer_ep.uri.span.start + q_idx, timer_ep.uri.span.end);
        assert_eq!(&doc.raw[seg_span.start..seg_span.end], "?bogus=1");

        let fix = Fix {
            span: seg_span,
            replacement: String::new(),
        };
        doc.apply_fix(&fix)
            .expect("removing the query segment must re-parse cleanly");

        // The fix refreshed the route view: re-lint the new raw text and
        // confirm the `UnknownOption` is gone.
        assert_eq!(
            doc.raw, "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo\n",
            "raw must reflect the applied fix"
        );
        let after = engine.lint(&doc.raw);
        let unknown_after = after
            .iter()
            .filter(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::UnknownOption))
            .count();
        assert_eq!(
            unknown_after, 0,
            "UnknownOption must no longer fire after removing the query; got: {after:?}"
        );
    }

    #[test]
    fn apply_fix_rejects_syntax_breaking_edit() {
        use crate::diagnostic::Fix;

        let engine = timer_log_engine();
        let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo?bogus=1\n";

        let mut doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "fixture must parse cleanly");
        let original_raw = doc.raw.clone();
        let original_diags = engine.lint(&doc.raw);

        // Span the timer endpoint URI value and replace it with an unclosed
        // flow sequence start: splicing yields `to: [\n` whose flow context
        // never closes — a syntax error noyalib reports via `parse_failure`.
        let endpoints = doc.route_view.endpoints();
        let timer_ep = endpoints
            .iter()
            .find(|e| e.uri.value.starts_with("timer:"))
            .expect("timer endpoint must be captured");
        let break_fix = Fix {
            span: Span::new(timer_ep.uri.span.start, timer_ep.uri.span.end),
            replacement: "[".to_string(),
        };

        let err = doc
            .apply_fix(&break_fix)
            .expect_err("a syntax-breaking edit must be rejected");
        assert!(
            matches!(err, crate::error::LintError::Internal(_)),
            "expected LintError::Internal, got: {err:?}"
        );

        // The document is byte-identical to its pre-edit state.
        assert_eq!(
            doc.raw, original_raw,
            "document raw must be unchanged after a rejected edit"
        );
        assert!(
            doc.parse_failure.is_none(),
            "parse_failure must not be set after a rejected edit"
        );
        // Re-linting yields the same number of diagnostics as before.
        let after_diags = engine.lint(&doc.raw);
        assert_eq!(
            after_diags.len(),
            original_diags.len(),
            "diagnostic set must be unchanged after a rejected edit"
        );
    }

    #[test]
    fn multi_route_envelope_captures_all_from_uris() {
        // Regression for rc-m1mx: routes 2..N's `from` was silently dropped
        // by the first-wins guard on from_slot. After the fix, route 1's
        // from is captured in the `from` field, and routes 2..N's from
        // values appear as endpoint nodes.
        let source = "routes:\n  - id: a\n    from: timer:one\n  - id: b\n    from: timer:two\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");

        // Route 1's from is in the dedicated `from` slot.
        let from = doc
            .route_view
            .from
            .as_ref()
            .expect("route 1 from must be captured in from slot");
        assert_eq!(from.value, "timer:one");

        // Both from URIs appear in the flattened endpoint list.
        let endpoints = doc.route_view.endpoints();
        let uris: Vec<&str> = endpoints.iter().map(|e| e.uri.value.as_str()).collect();
        assert!(
            uris.contains(&"timer:one"),
            "route 1 from must appear in endpoints: {uris:?}"
        );
        assert!(
            uris.contains(&"timer:two"),
            "route 2 from must appear in endpoints: {uris:?}"
        );

        // No span appears twice (no duplication).
        let mut spans: Vec<_> = endpoints.iter().map(|e| e.uri.span.clone()).collect();
        spans.sort_by_key(|s| (s.start, s.end));
        let dupes = spans.windows(2).filter(|w| w[0] == w[1]).count();
        assert_eq!(dupes, 0, "endpoint spans must not duplicate");
    }

    // ---- Task 3.1: `parameters:` map entries become spanned options ----

    /// Stub catalog where `direct` is minimal (silent) and `scheme` carries the
    /// given `uri_options`.
    fn catalog_with(
        scheme: &str,
        opts: Vec<camel_api::component_metadata::UriOption>,
    ) -> crate::test_support::StubCatalog {
        use crate::test_support::StubCatalog;
        use camel_api::component_metadata::ComponentMetadata;

        StubCatalog::empty()
            .with("direct", ComponentMetadata::minimal("direct"))
            .with(
                scheme,
                ComponentMetadata::minimal(scheme).with_uri_options(opts),
            )
    }

    #[test]
    fn parameters_entries_become_options_with_spans() {
        let source = "from: direct:start\nsteps:\n  - to: kafka:orders\n    parameters:\n      brokers: my-host:9092\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let endpoints = doc.route_view.endpoints();
        assert_eq!(endpoints.len(), 2, "expected from + one to endpoint");
        let to_ep = &endpoints[1];
        assert_eq!(to_ep.uri.value, "kafka:orders");

        let brokers = to_ep
            .options
            .iter()
            .find(|o| o.key.value == "brokers")
            .expect("parameters entry `brokers` must appear as an option");
        let key_start = source.find("brokers:").expect("brokers key present");
        assert_eq!(brokers.key.span.start, key_start);
        assert_eq!(slice_at(&doc.raw, &brokers.key.span), "brokers");

        let val = brokers.value.as_ref().expect("brokers has a value span");
        let val_start = source.find("my-host:9092").expect("value present");
        assert_eq!(val.span.start, val_start);
        assert_eq!(val.value, "my-host:9092");
        assert_eq!(slice_at(&doc.raw, &val.span), "my-host:9092");
    }

    #[test]
    fn both_maps_enrich_merge_step_and_inner_parameters() {
        // Full-form enrich with BOTH a step-level `parameters:` map and an
        // inner config `parameters:` map: the endpoint must carry entries from
        // both (regression: the inner map used to shadow the step-level one).
        let source = "from: direct:start\nsteps:\n  - enrich:\n      uri: db:query\n      parameters:\n        dataSource: customers\n    parameters:\n      timeoutS: \"5000\"\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let endpoints = doc.route_view.endpoints();
        assert_eq!(endpoints.len(), 2, "expected from + one enrich endpoint");
        let enrich_ep = &endpoints[1];
        assert_eq!(enrich_ep.uri.value, "db:query");

        let opt = |key: &str| enrich_ep.options.iter().find(|o| o.key.value == key);
        let data_source =
            opt("dataSource").expect("inner config parameter `dataSource` must reach the endpoint");
        assert_eq!(
            data_source.value.as_ref().expect("value span").value,
            "customers"
        );
        let timeout =
            opt("timeoutS").expect("step-level parameter `timeoutS` must reach the endpoint");
        assert_eq!(timeout.value.as_ref().expect("value span").value, "5000");
    }

    #[test]
    fn both_maps_poll_enrich_merge_step_and_inner_parameters() {
        // Same regression for poll_enrich, whose full form reuses EnrichConfig.
        let source = "from: direct:start\nsteps:\n  - poll_enrich:\n      uri: db:query\n      parameters:\n        dataSource: customers\n    parameters:\n      timeoutS: \"5000\"\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let endpoints = doc.route_view.endpoints();
        assert_eq!(
            endpoints.len(),
            2,
            "expected from + one poll_enrich endpoint"
        );
        let enrich_ep = &endpoints[1];
        assert_eq!(enrich_ep.uri.value, "db:query");

        let opt = |key: &str| enrich_ep.options.iter().find(|o| o.key.value == key);
        let data_source =
            opt("dataSource").expect("inner config parameter `dataSource` must reach the endpoint");
        assert_eq!(
            data_source.value.as_ref().expect("value span").value,
            "customers"
        );
        let timeout =
            opt("timeoutS").expect("step-level parameter `timeoutS` must reach the endpoint");
        assert_eq!(timeout.value.as_ref().expect("value span").value, "5000");
    }

    #[test]
    fn from_parameters_entries_become_options() {
        let source = "from: timer:tick\nparameters:\n  period: 1s\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let endpoints = doc.route_view.endpoints();
        assert_eq!(endpoints.len(), 1, "expected exactly the from endpoint");
        let from_ep = &endpoints[0];
        assert_eq!(from_ep.uri.value, "timer:tick");

        let period = from_ep
            .options
            .iter()
            .find(|o| o.key.value == "period")
            .expect("route-level parameters entry `period` must appear as an option");
        assert_eq!(slice_at(&doc.raw, &period.key.span), "period");
        let val = period.value.as_ref().expect("period has a value span");
        assert_eq!(val.value, "1s");
        assert_eq!(slice_at(&doc.raw, &val.span), "1s");
    }

    // ---- Task 1.1: origin-tagged options ----

    #[test]
    fn query_options_carry_query_origin() {
        let source = "steps:\n  - to: timer:foo?period=1s\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let endpoints = doc.route_view.endpoints();
        assert_eq!(endpoints.len(), 1);
        let period = endpoints[0]
            .options
            .iter()
            .find(|o| o.key.value == "period")
            .expect("query option `period` must be captured");
        assert_eq!(period.origin, OptionOrigin::Query);
    }

    #[test]
    fn step_parameters_carry_step_origin() {
        let source = "steps:\n  - to: kafka:orders\n    parameters:\n      brokers: my-host:9092\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let endpoints = doc.route_view.endpoints();
        assert_eq!(endpoints.len(), 1);
        let brokers = endpoints[0]
            .options
            .iter()
            .find(|o| o.key.value == "brokers")
            .expect("parameters entry `brokers` must be captured");
        assert_eq!(brokers.origin, OptionOrigin::StepParameters);
    }

    #[test]
    fn nested_object_form_distinguishes_origins() {
        let source = "steps:\n  - enrich:\n      uri: db:query\n      parameters:\n        dataSource: customers\n    parameters:\n      timeoutS: \"5000\"\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let endpoints = doc.route_view.endpoints();
        assert_eq!(endpoints.len(), 1);
        let ep = &endpoints[0];
        assert_eq!(ep.uri.value, "db:query");
        let opt = |key: &str| ep.options.iter().find(|o| o.key.value == key);
        let data_source =
            opt("dataSource").expect("inner config parameter `dataSource` must be captured");
        assert_eq!(data_source.origin, OptionOrigin::ConfigParameters);
        let timeout = opt("timeoutS").expect("step-level parameter `timeoutS` must be captured");
        assert_eq!(timeout.origin, OptionOrigin::StepParameters);
    }

    #[test]
    fn from_parameters_carry_step_origin() {
        let source = "from: timer:tick\nparameters:\n  period: \"2500\"\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let endpoints = doc.route_view.endpoints();
        assert_eq!(endpoints.len(), 1, "expected exactly the from endpoint");
        let from_ep = &endpoints[0];
        assert_eq!(from_ep.uri.value, "timer:tick");
        let period = from_ep
            .options
            .iter()
            .find(|o| o.key.value == "period")
            .expect("route-level parameters entry `period` must be captured");
        assert_eq!(period.origin, OptionOrigin::StepParameters);
    }

    #[test]
    fn unknown_param_in_parameters_flagged() {
        use crate::diagnostic::{DiagnosticCode, UriKnownSubCode};
        use crate::engine::LintEngine;
        use camel_api::component_metadata::{OptionKind, UriOption};
        use std::sync::Arc;

        let catalog = catalog_with(
            "timer",
            vec![UriOption::new("period", "period", OptionKind::Duration)],
        );
        let engine = LintEngine::new(Arc::new(catalog)).with_default_rules();
        let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo\n    parameters:\n      perod: \"1\"\n";
        let diags = engine.lint(source);
        let unknown: Vec<_> = diags
            .iter()
            .filter(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::UnknownOption))
            .collect();
        assert_eq!(
            unknown.len(),
            1,
            "expected one UnknownOption; got: {diags:?}"
        );
        assert_eq!(slice_at(source, &unknown[0].span), "perod");
    }

    #[test]
    fn missing_required_in_parameters_flagged() {
        use crate::diagnostic::{DiagnosticCode, UriKnownSubCode};
        use crate::engine::LintEngine;
        use camel_api::component_metadata::{OptionKind, UriOption};
        use std::sync::Arc;

        let catalog = catalog_with(
            "timer",
            vec![
                UriOption::new("period", "period", OptionKind::Duration).required(),
                UriOption::new("delay", "delay", OptionKind::Duration),
            ],
        );
        let engine = LintEngine::new(Arc::new(catalog)).with_default_rules();
        // `period` is required and omitted; `delay` (non-required) is provided
        // via `parameters:` — so the map must resolve `delay` (not unknown) and
        // must NOT suppress the missing-required error for `period`.
        let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo\n    parameters:\n      delay: 500ms\n";
        let diags = engine.lint(source);
        let missing: Vec<_> = diags
            .iter()
            .filter(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::MissingRequiredOption))
            .collect();
        assert_eq!(
            missing.len(),
            1,
            "expected one MissingRequiredOption; got: {diags:?}"
        );
        assert_eq!(slice_at(source, &missing[0].span), "timer:foo");
        let unknown = diags
            .iter()
            .filter(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::UnknownOption))
            .count();
        assert_eq!(unknown, 0, "delay must resolve, not be unknown: {diags:?}");

        // Complementary case: when the required option IS provided via
        // `parameters:`, `option_present` must see it and the missing-required
        // error must NOT fire.
        let source2 = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo\n    parameters:\n      period: 1s\n";
        let diags2 = engine.lint(source2);
        let missing2 = diags2
            .iter()
            .filter(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::MissingRequiredOption))
            .count();
        assert_eq!(
            missing2, 0,
            "period provided via parameters must satisfy the requirement: {diags2:?}"
        );
    }

    #[test]
    fn deprecated_in_parameters_flagged() {
        use crate::diagnostic::DiagnosticCode;
        use crate::engine::LintEngine;
        use camel_api::component_metadata::{OptionKind, UriOption};
        use std::sync::Arc;

        let catalog = catalog_with(
            "timer",
            vec![
                UriOption::new("oldFreq", "old frequency", OptionKind::Duration)
                    .deprecated("use `period` instead"),
            ],
        );
        let engine = LintEngine::new(Arc::new(catalog)).with_default_rules();
        let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:foo\n    parameters:\n      oldFreq: 1s\n";
        let diags = engine.lint(source);
        let dep: Vec<_> = diags
            .iter()
            .filter(|d| d.code == DiagnosticCode::RDeprecated)
            .collect();
        assert_eq!(dep.len(), 1, "expected one RDeprecated; got: {diags:?}");
        assert_eq!(slice_at(source, &dep[0].span), "oldFreq");
    }

    #[test]
    fn secret_in_parameters_flagged() {
        use crate::diagnostic::DiagnosticCode;
        use crate::engine::LintEngine;
        use camel_api::component_metadata::{OptionKind, UriOption};
        use std::sync::Arc;

        let catalog = catalog_with(
            "http",
            vec![UriOption::new("password", "password", OptionKind::String).secret()],
        );
        let engine = LintEngine::new(Arc::new(catalog)).with_default_rules();
        let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: http:srv\n    parameters:\n      password: hunter2\n";
        let diags = engine.lint(source);
        let secret: Vec<_> = diags
            .iter()
            .filter(|d| d.code == DiagnosticCode::RSecret)
            .collect();
        assert_eq!(secret.len(), 1, "expected one RSecret; got: {diags:?}");
        assert_eq!(slice_at(source, &secret[0].span), "hunter2");
    }
}
