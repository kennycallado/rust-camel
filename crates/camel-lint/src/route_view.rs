//! Span-carrying route view for lint analysis.
//!
//! [`LintRoute`] is the semantic model lint rules operate on. It is built by
//! walking the noyalib CST (see [`crate::document`]) and captures, with
//! byte-exact spans, every location a rule reports on: the route-level
//! `from`, each step `to`/`uri`, nested child steps inside containers
//! (`choice`/`multicast`/`scatter_gather`), and each URI option key/value.
//!
//! Only leaves a rule reports on carry a [`Spanned`] wrapper; structural
//! nodes use [`LintNode::Branch`] to preserve nesting for `endpoints()`
//! flattening without per-token annotation noise.

use crate::diagnostic::Span;
use camel_api::component_metadata::{
    ComponentMetadata, ComponentMetadataCatalog, UriOption, UriOptionMatch,
};

// ---------------------------------------------------------------------------
// Spanned
// ---------------------------------------------------------------------------

/// A value annotated with its byte-exact span into the source text.
#[derive(Clone, Debug)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// LintOption, Endpoint, LintNode
// ---------------------------------------------------------------------------

/// The source origin of a [`LintOption`], mirroring the DSL lowering's
/// vocabulary so rules can distinguish where an option key was declared.
///
/// - [`OptionOrigin::Query`] — parsed out of the raw URI query string
///   (`?k=v`).
/// - [`OptionOrigin::StepParameters`] — an entry of a `parameters:` map
///   sibling of a URI-bearing key (`to`/`from`/`uri`), including the
///   route-level `from`.
/// - [`OptionOrigin::ConfigParameters`] — an entry of the `parameters:` map
///   inside an object-form URI key (cf. `combine_params(config, step)` in
///   `camel-dsl/src/yaml.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OptionOrigin {
    Query,
    StepParameters,
    ConfigParameters,
}

/// An option key-value pair extracted from a URI query string (`?k=v`) or
/// from an endpoint's sibling `parameters:` map entry.
#[derive(Clone, Debug)]
pub struct LintOption {
    pub key: Spanned<String>,
    pub value: Option<Spanned<String>>,
    pub origin: OptionOrigin,
}

impl LintOption {
    /// Split the `?key=value&key2=value2` query portion of `uri_value` into
    /// [`LintOption`]s.
    ///
    /// Spans are byte offsets into the ORIGINAL source: each query token's
    /// offset relative to `uri_value`'s start is added to `uri_span.start`.
    /// A value-less flag (`?flag`) yields `value: None`. URI option tokens
    /// are ASCII (`?`, `&`, `=` are single-byte), so byte offsets are used
    /// throughout.
    pub fn parse_from_query(uri_value: &str, uri_span: Span) -> Vec<LintOption> {
        // The query begins after the first `?`. URIs scheme/path come first;
        // the `?` delimiter is unambiguous in a Camel endpoint URI.
        let Some(qpos) = uri_value.find('?') else {
            return Vec::new();
        };
        let query = &uri_value[qpos + 1..];
        // Absolute byte offset in the source of the first query character.
        let base = uri_span.start + qpos + 1;

        let mut options = Vec::new();
        let mut rel = 0usize; // byte offset within `query`
        while rel < query.len() {
            // Find the end of this `&`-delimited pair.
            let pair_end = query[rel..].find('&').map_or(query.len(), |p| rel + p);
            let pair = &query[rel..pair_end];
            if !pair.is_empty() {
                let key_start = base + rel;
                match pair.find('=') {
                    Some(eq_off) => {
                        let key_end = base + rel + eq_off;
                        let val_start = base + rel + eq_off + 1;
                        let val_end = base + pair_end;
                        let key = Spanned {
                            value: pair[..eq_off].to_string(),
                            span: Span::new(key_start, key_end),
                        };
                        let value = if val_start <= val_end {
                            Some(Spanned {
                                value: pair[eq_off + 1..].to_string(),
                                span: Span::new(val_start, val_end),
                            })
                        } else {
                            None
                        };
                        options.push(LintOption {
                            key,
                            value,
                            origin: OptionOrigin::Query,
                        });
                    }
                    None => {
                        // Value-less flag: `?flag`.
                        let key = Spanned {
                            value: pair.to_string(),
                            span: Span::new(key_start, base + pair_end),
                        };
                        options.push(LintOption {
                            key,
                            value: None,
                            origin: OptionOrigin::Query,
                        });
                    }
                }
            }
            // Advance past the `&`.
            rel = pair_end + 1;
        }
        options
    }
}

/// An endpoint: a URI leaf (`to`/`from`/`uri`) with its options.
///
/// `key` names the origin field that produced this endpoint (e.g. `to`,
/// `from`, `uri`, `wire_tap`, `enrich`, `poll_enrich`, `endpoints`,
/// `dead_letter_channel`). It lets rules distinguish send positions from
/// non-send origins without re-deriving the source path. Its span mirrors the
/// endpoint URI's span.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub key: Spanned<String>,
    pub uri: Spanned<String>,
    pub options: Vec<LintOption>,
}

/// A route step node — either a leaf endpoint or a container branch.
#[derive(Clone, Debug)]
pub enum LintNode {
    Endpoint(Endpoint),
    Branch {
        kind: Spanned<String>,
        children: Vec<Spanned<LintNode>>,
    },
}

// ---------------------------------------------------------------------------
// LintRoute
// ---------------------------------------------------------------------------

/// The span-carrying route view extracted from a parsed document.
#[derive(Clone, Debug, Default)]
pub struct LintRoute {
    pub from: Option<Spanned<String>>,
    /// Route-level `parameters:` entries (spanned options) attached to `from`.
    pub from_parameters: Vec<LintOption>,
    pub nodes: Vec<Spanned<LintNode>>,
}

impl LintRoute {
    /// Flattened endpoint list covering the route-level `from` (with its
    /// query-string options plus the route-level `parameters:` entries)
    /// FOLLOWED BY every [`LintNode::Endpoint`] at any depth — including
    /// those nested inside [`LintNode::Branch`] containers — in source order.
    ///
    /// Rules iterate this flat list to cover `from` + all `to`/`uri` at any
    /// nesting depth without walking the tree themselves.
    pub fn endpoints(&self) -> Vec<Endpoint> {
        let mut out = Vec::new();
        if let Some(f) = &self.from {
            let mut options = LintOption::parse_from_query(&f.value, f.span.clone());
            options.extend(self.from_parameters.iter().cloned());
            out.push(Endpoint {
                key: Spanned {
                    value: "from".to_string(),
                    span: f.span.clone(),
                },
                uri: Spanned {
                    value: f.value.clone(),
                    span: f.span.clone(),
                },
                options,
            });
        }
        for node in &self.nodes {
            Self::collect_endpoints(node, &mut out);
        }
        out
    }

    fn collect_endpoints(node: &Spanned<LintNode>, out: &mut Vec<Endpoint>) {
        match &node.value {
            LintNode::Endpoint(e) => out.push(e.clone()),
            LintNode::Branch { children, .. } => {
                for child in children {
                    Self::collect_endpoints(child, out);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// known_endpoints — shared iterator for rules
// ---------------------------------------------------------------------------

/// Iterate endpoints whose scheme is registered in `catalog` with non-empty
/// `uri_options`.
///
/// Yields `(endpoint, metadata)` for each endpoint whose URI contains a colon,
/// whose scheme is found in `catalog`, and whose metadata has a non-empty
/// `uri_options` list. Endpoints with colonless URIs, unregistered schemes, or
/// minimal (option-less) schemes are skipped.
pub(crate) fn known_endpoints<'a>(
    endpoints: &'a [Endpoint],
    catalog: &'a dyn ComponentMetadataCatalog,
) -> impl Iterator<Item = (&'a Endpoint, ComponentMetadata)> {
    endpoints.iter().filter_map(|ep| {
        if !ep.uri.value.contains(':') {
            return None;
        }
        let scheme = ep.uri.value.split(':').next()?;
        let meta = catalog.get_metadata(scheme)?;
        if meta.uri_options.is_empty() {
            return None;
        }
        Some((ep, meta))
    })
}

// ---------------------------------------------------------------------------
// Shared option-resolution helpers (used by multiple rules)
// ---------------------------------------------------------------------------

/// Resolve a provided [`LintOption`] to its canonical catalog [`UriOption`] by
/// name, alias, or prefix pattern. Returns `None` when the key matches nothing.
///
/// Resolution order (two-phase):
/// 1. Exact name or alias match on options with `pattern: None`.
/// 2. Longest-prefix match on `Prefix`-patterned options, requiring a non-empty
///    suffix.
pub(crate) fn resolve_option<'a>(
    opt: &LintOption,
    uri_options: &'a [UriOption],
) -> Option<&'a UriOption> {
    let key = opt.key.value.as_str();
    // Phase 1: exact-name and alias, only on options whose pattern is None.
    if let Some(hit) = uri_options
        .iter()
        .find(|uo| uo.pattern.is_none() && (uo.name == key || uo.aliases.contains(&opt.key.value)))
    {
        return Some(hit);
    }
    // Phase 2: pattern match, longest separator first, non-empty suffix required.
    let mut pattern_hits: Vec<&UriOption> = uri_options
        .iter()
        .filter(|uo| match &uo.pattern {
            Some(UriOptionMatch::Prefix { separator }) => {
                !separator.is_empty()
                    && key.len() > separator.len()
                    && key.starts_with(separator.as_str())
            }
            _ => false,
        })
        .collect();
    let sep_len = |uo: &UriOption| match &uo.pattern {
        Some(UriOptionMatch::Prefix { separator }) => separator.len(),
        _ => 0,
    };
    pattern_hits.sort_by_key(|b| std::cmp::Reverse(sep_len(b)));
    pattern_hits.first().copied()
}

/// Whether a catalog option (by `name` or any `alias`) is present among the
/// endpoint's provided options.
pub(crate) fn option_present(name: &str, aliases: &[String], options: &[LintOption]) -> bool {
    options
        .iter()
        .any(|o| o.key.value == name || aliases.contains(&o.key.value))
}
