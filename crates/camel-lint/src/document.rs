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
use crate::route_view::{Endpoint, LintNode, LintOption, LintRoute, Spanned};

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
                let nodes = walk(&root, "", &noya_doc, &mut from);
                Document {
                    raw,
                    route_view: LintRoute { from, nodes },
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

    /// Apply a suggested [`Fix`] to this document.
    ///
    /// Substitutes `fix.replacement` into `fix.span` via the noyalib CST
    /// [`replace_span`](cst::Document::replace_span), then re-parses the result
    /// and refreshes [`raw`](Document::raw) / [`route_view`](Document::route_view)
    /// / [`parse_failure`](Document::parse_failure).
    ///
    /// On an edit that breaks syntax — or an out-of-bounds /
    /// non-character-boundary span — returns [`LintError::Internal`] and leaves
    /// the document **unchanged** (no field is mutated before the edit is
    /// fully validated, so there is nothing to roll back).
    ///
    /// This is a document-level operation: the engine is stateless and never
    /// retains a `Document`. A caller applies a fix with `doc.apply_fix(&fix)`
    /// and then re-runs `engine.lint(&doc.raw)` to obtain refreshed
    /// diagnostics.
    pub fn apply_fix(&mut self, fix: &Fix) -> Result<(), LintError> {
        // Re-parse the current source into a CST document to drive
        // `replace_span`. The stored source is normally clean (rules never
        // emit fixes for a document that failed to parse), but the guard is
        // cheap and keeps `apply_fix` total over its inputs.
        let mut cst_doc = match cst::parse_document(&self.raw) {
            Ok(d) => d,
            Err(e) => {
                return Err(LintError::Internal(format!(
                    "apply_fix source un-parseable: {e}"
                )));
            }
        };

        // `replace_span` rejects out-of-bounds / non-character-boundary ranges
        // and re-parses internally. A syntactically broken replacement may
        // still slip through noyalib's lenient recovery, so the authoritative
        // syntax gate is the `Document::parse` re-parse below.
        if let Err(e) = cst_doc.replace_span(fix.span.start, fix.span.end, &fix.replacement) {
            return Err(LintError::Internal(format!("apply_fix edit rejected: {e}")));
        }

        let new_raw = cst_doc.source().to_string();
        let reparsed = Document::parse(&new_raw);
        if reparsed.parse_failure.is_some() {
            // Edit broke syntax. Nothing has been mutated yet, so the document
            // stays byte-identical to its pre-edit state.
            return Err(LintError::Internal(
                "apply_fix produced invalid syntax".into(),
            ));
        }

        // Commit the refreshed view (only reached on a clean re-parse).
        self.raw = reparsed.raw;
        self.route_view = reparsed.route_view;
        self.parse_failure = reparsed.parse_failure;
        Ok(())
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
/// also emitted as a step node.
fn walk(
    value: &Value,
    path: &str,
    doc: &cst::Document,
    from_slot: &mut Option<Spanned<String>>,
) -> Vec<Spanned<LintNode>> {
    let mut nodes = Vec::new();
    match value {
        Value::Mapping(m) => {
            for (key, child) in m.iter() {
                let cpath = child_path(path, key);
                let k = key.as_str();
                if k == "from" {
                    if from_slot.is_none()
                        && let Some(s) = child.as_str()
                        && let Some((start, end)) = doc.span_at(&cpath)
                    {
                        *from_slot = Some(Spanned {
                            value: s.to_string(),
                            span: Span::new(start, end),
                        });
                    }
                    continue;
                }
                if URI_KEYS.contains(&k) {
                    if matches!(child, Value::Mapping(_)) {
                        // Object form (e.g. enrich: { uri: ... }): recurse to
                        // find the nested `uri`, which is itself a URI key.
                        nodes.extend(walk(child, &cpath, doc, from_slot));
                    } else {
                        emit_endpoints(child, &cpath, doc, &mut nodes);
                    }
                } else if CONTAINER_KEYS.contains(k) {
                    let children = walk(child, &cpath, doc, from_slot);
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
                nodes.extend(walk(item, &ipath, doc, from_slot));
            }
        }
        _ => {}
    }
    nodes
}

/// Emit one [`Endpoint`] per string value: a single string (SCALAR-URI) yields
/// one endpoint; a sequence of strings (URI-ARRAY) yields one per item.
fn emit_endpoints(
    value: &Value,
    path: &str,
    doc: &cst::Document,
    nodes: &mut Vec<Spanned<LintNode>>,
) {
    match value {
        Value::String(s) => {
            if let Some(ep) = endpoint_for(s, path, doc) {
                nodes.push(ep);
            }
        }
        Value::Sequence(seq) => {
            for (i, item) in seq.iter().enumerate() {
                if let Value::String(s) = item {
                    let ipath = format!("{path}[{i}]");
                    if let Some(ep) = endpoint_for(s, &ipath, doc) {
                        nodes.push(ep);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Build an [`Endpoint`] node for a single URI string at `path`, parsing its
/// query-string options against the original source.
fn endpoint_for(uri: &str, path: &str, doc: &cst::Document) -> Option<Spanned<LintNode>> {
    let (start, end) = doc.span_at(path)?;
    let span = Span::new(start, end);
    let options = LintOption::parse_from_query(uri, span.clone());
    Some(Spanned {
        value: LintNode::Endpoint(Endpoint {
            uri: Spanned {
                value: uri.to_string(),
                span: span.clone(),
            },
            options,
        }),
        span,
    })
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
}
