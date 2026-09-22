//! [`Document`] — a parsed route source with a span-carrying route view.
//!
//! Parsing ALWAYS yields a [`Document`] (never `Err`): a syntax error is
//! data surfaced through [`ParseFailure`] for the R-SYN rule, not an engine
//! error. On success the noyalib CST is walked to build [`LintRoute`],
//! capturing every URI-bearing location with byte-exact spans. Mapping keys
//! are interpreted against the schema candidates resolved by
//! [`crate::schema_context`].

use noyalib::Value;
use noyalib::cst;

use crate::diagnostic::{Fix, Span};
use crate::error::LintError;
use crate::route_view::{Endpoint, LintNode, LintOption, LintRoute, OptionOrigin, Spanned};
use crate::schema_context::{
    CONTAINER_KEYS, SCHEMA, container_child_ctx, is_container, is_free_form, items_ctx,
    key_permitted, lookup_property, mapping_candidates, root_context, typed_ap_schemas,
};

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
                let ctx = root_context(&root);
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
                    &ctx,
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
    /// Substitutes `fix.replacement` into `fix.span` via [`apply_edit`](Document::apply_edit), then
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
// URI keys (allowlist)
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

// ---------------------------------------------------------------------------
// CST walker
// ---------------------------------------------------------------------------

/// Build the spanned node list for `value`. Each mapping key is interpreted
/// against `ctx` — the candidate subschemas describing the current node,
/// resolved from the embedded [`SCHEMA`]:
///
/// - a DECLARED key dispatches on its own subschema: URI-bearing leaves
///   reach endpoint emission, structured containers recurse as Branches,
///   and FREE-FORM maps ([`is_free_form`] — REST `response.headers`,
///   `security_policy.config`) are OPAQUE leaves, never interpreted;
/// - an UNDECLARED key the active candidates PERMIT (`additionalProperties`
///   absent/`true`/typed) is legitimate user data — dispatched on the typed
///   schema when present, opaque otherwise;
/// - an UNDECLARED key every candidate REJECTS falls back to the legacy
///   global-name interpretation ([`URI_KEYS`]/[`CONTAINER_KEYS`]) for
///   schema-invalid-but-tolerated shapes, with the empty ctx propagating
///   that fallback to children.
///
/// `from` captures the route-level `from` URI (first occurrence wins) so it
/// is not also emitted as a step node; `from_parameters` captures the
/// sibling `parameters:` entries attached to that `from`. `inherited`
/// carries any step-level `parameters:` entries into an object-form URI key
/// (e.g. `enrich: { uri: ... }`) and is CONCATENATED with the inner
/// config's own `parameters:` map, so entries from both reach the nested
/// `uri` endpoint (the DSL lowerer merges disjoint keys, so dropping either
/// side would miss rules and could false-flag `MissingRequiredOption`).
#[allow(clippy::too_many_arguments)] // recursive walker: one slot per threaded input
fn walk(
    value: &Value,
    path: &str,
    doc: &cst::Document,
    from_slot: &mut Option<Spanned<String>>,
    from_parameters: &mut Vec<LintOption>,
    inherited: &[LintOption],
    local_origin: OptionOrigin,
    ctx: &[&serde_json::Value],
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
                // The mcp block authors no endpoint URIs (rc-6pikg): its
                // consumer from-URIs are fabricated by the lowering at parse
                // time, and `resources[].uri` is an MCP resource URI —
                // operator config with an arbitrary scheme (`crm://...`) —
                // not an endpoint. Skip the subtree so R-URI-known cannot
                // false-positive on it (same shape as the `parameters`
                // skip). Revisit if the mcp block ever authors a real
                // endpoint URI.
                if k == "mcp" {
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
                        span: unquoted_span(doc, start, end),
                    });
                    *from_parameters = effective.clone();
                    continue;
                }
                // Schema-context dispatch: a key declared by the current
                // node's candidates dispatches on its own subschema; an
                // undeclared-but-permitted key is user data; only a key
                // every candidate rejects keeps the legacy global-name
                // fallback (schema-invalid shape tolerance).
                match lookup_property(ctx, k) {
                    Some(ps) if URI_KEYS.contains(&k) => {
                        if matches!(child, Value::Mapping(_)) {
                            // Object form (e.g. `enrich: { uri: ... }`, or
                            // the tolerated object-form `from`): recurse to
                            // find the nested URI, carrying the step-level
                            // parameters alongside the inner config's own
                            // `parameters:` map (ConfigParameters origin).
                            // The child ctx is the property's mapping-capable
                            // branches — EMPTY for scalar-declared keys
                            // (object-form `from`), so the nested walk runs
                            // in legacy global-name mode and today's capture
                            // is preserved.
                            let child_ctx = mapping_candidates(std::slice::from_ref(&ps));
                            nodes.extend(walk(
                                child,
                                &cpath,
                                doc,
                                from_slot,
                                from_parameters,
                                &effective,
                                OptionOrigin::ConfigParameters,
                                &child_ctx,
                            ));
                        } else {
                            emit_endpoints(child, &cpath, doc, &mut nodes, &effective);
                        }
                    }
                    Some(ps) if is_container(ps, &SCHEMA) => {
                        // Structured container: Branch recursion with the
                        // declared shape as child ctx.
                        nodes.push(branch_node(
                            key,
                            child,
                            &cpath,
                            doc,
                            from_slot,
                            from_parameters,
                            &container_child_ctx(ps),
                        ));
                    }
                    Some(ps) if is_free_form(ps) => {
                        // FREE-FORM map (e.g. `response.headers`,
                        // `security_policy.config`): an OPAQUE leaf — user
                        // data, never interpreted, never walked (the
                        // bd rc-ni8qu false positives lived here).
                    }
                    // Declared scalar/other leaf: not an endpoint author.
                    Some(_) => {}
                    None => {
                        if key_permitted(ctx) {
                            // UNDECLARED but PERMITTED: legitimate user
                            // data. A typed `additionalProperties` schema
                            // dispatches the value on THAT schema (only
                            // structured containers recurse; a scalar-typed
                            // AP such as `config`'s `{type: string}` stays
                            // opaque — a string entry named `to` is data,
                            // not an endpoint). AP absent or `true`: opaque
                            // — no global interpretation. Exactly ONE
                            // dispatch: the FIRST container AP wins, so two
                            // mapping-capable candidates carrying typed APs
                            // can never emit duplicate endpoints.
                            if let Some(ap) = typed_ap_schemas(ctx)
                                .into_iter()
                                .find(|ap| is_container(ap, &SCHEMA))
                            {
                                nodes.push(branch_node(
                                    key,
                                    child,
                                    &cpath,
                                    doc,
                                    from_slot,
                                    from_parameters,
                                    &container_child_ctx(ap),
                                ));
                            }
                        } else if URI_KEYS.contains(&k) {
                            // LEGACY fallback (schema-invalid shape
                            // tolerance): exactly today's behavior, with the
                            // empty ctx so children also resolve via
                            // fallback.
                            if matches!(child, Value::Mapping(_)) {
                                nodes.extend(walk(
                                    child,
                                    &cpath,
                                    doc,
                                    from_slot,
                                    from_parameters,
                                    &effective,
                                    OptionOrigin::ConfigParameters,
                                    &[],
                                ));
                            } else {
                                emit_endpoints(child, &cpath, doc, &mut nodes, &effective);
                            }
                        } else if CONTAINER_KEYS.contains(k) {
                            nodes.push(branch_node(
                                key,
                                child,
                                &cpath,
                                doc,
                                from_slot,
                                from_parameters,
                                &[],
                            ));
                        }
                    }
                }
            }
        }
        Value::Sequence(seq) => {
            // Items get the ctx's `items` subschemas when declared (e.g.
            // `steps:` → RouteDslStep per item); otherwise the same ctx —
            // shape tolerance for direct-sequence container forms.
            let item_ctx = items_ctx(ctx);
            let eff_ctx: &[&serde_json::Value] = if item_ctx.is_empty() { ctx } else { &item_ctx };
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
                    eff_ctx,
                ));
            }
        }
        _ => {}
    }
    nodes
}

/// Recurse into a container child and wrap the result in a [`LintNode::Branch`]
/// node keyed by the mapping key. `child_ctx` is the schema context for the
/// nested walk (empty = legacy global-name mode).
fn branch_node(
    key: &str,
    child: &Value,
    cpath: &str,
    doc: &cst::Document,
    from_slot: &mut Option<Spanned<String>>,
    from_parameters: &mut Vec<LintOption>,
    child_ctx: &[&serde_json::Value],
) -> Spanned<LintNode> {
    let children = walk(
        child,
        cpath,
        doc,
        from_slot,
        from_parameters,
        &[],
        OptionOrigin::StepParameters,
        child_ctx,
    );
    Spanned {
        value: LintNode::Branch {
            kind: Spanned {
                value: key.to_string(),
                span: key_span_for(doc, cpath),
            },
            children,
        },
        span: value_span_for(doc, cpath),
    }
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
    let span = unquoted_span(doc, start, end);
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

/// Trim a matching quote pair from a scalar value span. `span_at` returns the
/// raw YAML scalar token — quotes included for quoted scalars — but endpoint
/// URI spans must index the UNQUOTED value: downstream consumers slice
/// `uri.value` offsets against `uri.span.start` (R-URI-known's scheme span,
/// `parse_from_query`'s option spans, engine completions), so an untrimmed
/// span lands one byte early on every quoted URI. Boundary-trim only — inner
/// escapes never touch the first/last byte of the quoted content.
fn unquoted_span(doc: &cst::Document, start: usize, end: usize) -> Span {
    if end >= start + 2 {
        let raw = doc.source().as_bytes();
        let (first, last) = (raw[start], raw[end - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return Span::new(start + 1, end - 1);
        }
    }
    Span::new(start, end)
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
    fn quoted_uri_span_excludes_quote_pair() {
        // Quoted YAML scalars (double and single): the URI span must index
        // the unquoted content so scheme/option spans derived from
        // `uri.span.start` slice byte-exact tokens.
        let source = "from: \"direct:start\"\nsteps:\n  - to: 'log:out'\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let from = doc.route_view.from.as_ref().expect("from must be captured");
        assert_eq!(slice_at(&doc.raw, &from.span), "direct:start");
        assert_eq!(from.span.start, 7, "content begins after the opening quote");
        let to_ep = doc
            .route_view
            .endpoints()
            .into_iter()
            .find(|e| e.uri.value == "log:out")
            .expect("to endpoint must be captured");
        assert_eq!(slice_at(&doc.raw, &to_ep.uri.span), "log:out");
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

    // ---- namebloat Task 1.2: walk-level opacity + capture regressions ----

    #[test]
    fn rest_response_headers_named_uri_to_endpoints_are_opaque() {
        // `response.headers` is a free-form map (arbitrary header names →
        // values). Header entries named `uri`, `to`, and `endpoints` are
        // user DATA, not DSL keys: none may reach endpoint emission.
        let source = "rest:\n  - operations:\n      - method: GET\n        to: direct:ok\n        response:\n          headers:\n            uri: timer:foo?frequency=1s\n            to: log:out\n            endpoints:\n              - direct:a\n              - direct:b\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let uris: Vec<_> = doc
            .route_view
            .endpoints()
            .into_iter()
            .map(|e| e.uri.value)
            .collect();
        assert_eq!(
            uris,
            vec!["direct:ok"],
            "only the operation-level `to` is an endpoint"
        );
    }

    #[test]
    fn security_policy_config_map_is_opaque() {
        // Retention pin: `security_policy.config` is a free-form map, so a
        // `to:` entry inside it is user data. Passes before AND after the
        // Task 1.1 rewrite — the pin guards the rewrite from newly walking it.
        let source = "from: direct:start\nsecurity_policy:\n  config:\n    to: log:leak\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let uris: Vec<_> = doc
            .route_view
            .endpoints()
            .into_iter()
            .map(|e| e.uri.value)
            .collect();
        assert_eq!(uris, vec!["direct:start"], "only `from` is an endpoint");
    }

    #[test]
    fn permissive_root_stray_keys_are_opaque() {
        // The envelope root permits undeclared keys; a stray root-level
        // `response:` block is user data, not DSL to interpret.
        let source = "routes:\n  - from: direct:start\nresponse:\n  to: log:stray\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let uris: Vec<_> = doc
            .route_view
            .endpoints()
            .into_iter()
            .map(|e| e.uri.value)
            .collect();
        assert_eq!(
            uris,
            vec!["direct:start"],
            "only the route `from` is an endpoint; the stray root `to` must not leak"
        );
    }

    #[test]
    fn rest_operation_to_and_steps_still_captured() {
        let source = "rest:\n  - operations:\n      - method: GET\n        to: timer:op\n        steps:\n          - to: timer:nested\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let endpoints = doc.route_view.endpoints();
        let op = endpoints
            .iter()
            .find(|e| e.uri.value == "timer:op")
            .expect("operation-level `to` must be captured");
        let nested = endpoints
            .iter()
            .find(|e| e.uri.value == "timer:nested")
            .expect("nested steps `to` must be captured");
        assert_eq!(slice_at(&doc.raw, &op.uri.span), "timer:op");
        assert_eq!(slice_at(&doc.raw, &nested.uri.span), "timer:nested");
        assert_ne!(op.uri.span, nested.uri.span, "spans must be distinct");
    }

    #[test]
    fn nested_steps_to_captured_across_root_forms() {
        // Envelope, bare-route, and legacy array roots must all capture a
        // nested `steps[].to` with its own byte-exact span.
        let sources = [
            "routes:\n  - steps:\n      - to: log:nested\n",
            "steps:\n  - to: log:nested\n",
            "- steps:\n    - to: log:nested\n",
        ];
        for source in sources {
            let doc = Document::parse(source);
            assert!(
                doc.parse_failure.is_none(),
                "expected clean parse: {source:?}"
            );
            let nested = doc
                .route_view
                .endpoints()
                .into_iter()
                .find(|e| e.uri.value == "log:nested")
                .unwrap_or_else(|| panic!("`log:nested` must be captured in: {source:?}"));
            assert_eq!(slice_at(&doc.raw, &nested.uri.span), "log:nested");
        }
    }

    #[test]
    fn recursive_dotry_nested_to_captured() {
        // `do_try` inside `do_try` revisits RouteDslStep through
        // DoTryData.steps; the innermost `to` must still be captured.
        let source = "steps:\n  - do_try:\n      steps:\n        - do_try:\n            steps:\n              - to: log:deep\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let deep = doc
            .route_view
            .endpoints()
            .into_iter()
            .find(|e| e.uri.value == "log:deep")
            .expect("innermost `to: log:deep` must be captured");
        assert_eq!(slice_at(&doc.raw, &deep.uri.span), "log:deep");
    }

    #[test]
    fn multicast_direct_sequence_explicit() {
        // Explicit sibling of `nested_child_step_uri_captured`: the
        // multicast direct-sequence form is schema-invalid but tolerated
        // through the rejected-key legacy fallback, so its child endpoint
        // stays captured.
        let source = "from: direct:start\nsteps:\n  - multicast:\n      - to: log:nested\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none());
        let nested = doc
            .route_view
            .endpoints()
            .into_iter()
            .find(|e| e.uri.value == "log:nested")
            .expect("tolerated multicast child `to` must be captured");
        assert_eq!(slice_at(&doc.raw, &nested.uri.span), "log:nested");
    }

    #[test]
    fn object_form_from_uri_still_captured() {
        // Object-form `from` has no declared structure in the schema; the
        // legacy fallback must keep capturing both its nested `uri` and a
        // sibling step `to`.
        let source = "from:\n    uri: direct:start\nsteps:\n  - to: log:out\n";
        let doc = Document::parse(source);
        assert!(doc.parse_failure.is_none(), "expected clean parse");
        let uris: Vec<_> = doc
            .route_view
            .endpoints()
            .into_iter()
            .map(|e| e.uri.value)
            .collect();
        assert!(
            uris.iter().any(|u| u == "direct:start"),
            "object-form `from.uri` must be captured"
        );
        assert!(
            uris.iter().any(|u| u == "log:out"),
            "sibling step `to` must be captured"
        );
    }
}
