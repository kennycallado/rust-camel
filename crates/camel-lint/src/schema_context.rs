//! Schema context for the route walk: the embedded [`ROUTE_SCHEMA`] parsed
//! once, the derived container-name fallback, and the helpers that resolve
//! the candidate subschemas describing a node and interpret mapping keys
//! against them.
//!
//! These helpers depend only on the schema, `serde_json`, and the parsed
//! `noyalib::Value` root — not on the CST walker in [`crate::document`],
//! which calls them per mapping key.

use std::collections::HashSet;
use std::sync::LazyLock;

use noyalib::Value;

use crate::ROUTE_SCHEMA;

// ---------------------------------------------------------------------------
// Embedded schema + container keys (schema-derived fallback)
// ---------------------------------------------------------------------------

/// The embedded route schema, parsed once. Every schema-context helper
/// borrows from this static — the walk holds no per-document mutable
/// global state, so linting one document cannot influence another.
pub(crate) static SCHEMA: LazyLock<serde_json::Value> = LazyLock::new(|| {
    serde_json::from_str(ROUTE_SCHEMA).expect("embedded route schema is valid JSON") // allow-unwrap
});

/// Container property names derived from [`ROUTE_SCHEMA`], EXCLUDING
/// free-form maps ([`is_free_form`]). This is the LEGACY fallback set for
/// undeclared keys that every active schema candidate rejects
/// (`additionalProperties: false`) — schema-invalid-but-tolerated shapes —
/// no longer the primary dispatch. Re-deriving from the embedded schema
/// means a new container is picked up by re-syncing the schema copy.
pub(crate) static CONTAINER_KEYS: LazyLock<HashSet<String>> =
    LazyLock::new(|| container_keys(&SCHEMA));

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
/// FREE-FORM maps ([`is_free_form`]) are never containers: their entries
/// are user data, so they drop out of [`CONTAINER_KEYS`] derivation and out
/// of every structured-recursion decision.
pub(crate) fn is_container(node: &serde_json::Value, root: &serde_json::Value) -> bool {
    if is_free_form(node) {
        return false;
    }
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
// Schema context (namebloat): the walk carries the candidate subschemas
// describing the CURRENT node and interprets mapping keys against them, not
// against name sets derived globally from the whole schema.
// ---------------------------------------------------------------------------

/// Resolve a node's local `$ref` against the embedded [`SCHEMA`] root.
fn resolve_static(node: &serde_json::Value) -> &serde_json::Value {
    resolve_ref(node, &SCHEMA)
}

/// Schema context candidates for a document ROOT, mirroring R-SCHEMA's
/// envelope detection: a root mapping carrying `routes`/`rest`/`mcp` gets
/// the envelope schema itself; any other mapping is a bare route and gets
/// the `RouteDslRoute` def. A sequence root (legacy array form) also gets
/// the bare-route candidates — the walk's sequence arm reuses them for
/// every item.
pub(crate) fn root_context(root: &Value) -> Vec<&'static serde_json::Value> {
    if let Value::Mapping(m) = root
        && (m.contains_key("routes") || m.contains_key("rest") || m.contains_key("mcp"))
    {
        return vec![&SCHEMA];
    }
    let route_def = SCHEMA
        .pointer("/$defs/RouteDslRoute")
        .expect("embedded schema defines RouteDslRoute"); // allow-unwrap
    vec![route_def]
}

/// Search the context candidates for a declaration of `key` — through
/// `properties` after expanding `$ref` and `anyOf`/`oneOf`/`allOf`. The
/// first candidate that declares `key` wins.
///
/// Cycle guard: an ACTIVE-CHAIN of `$ref` strings, pushed on entry and
/// popped on exit, created FRESH per top-level call. A definition reachable
/// through two branches (or twice in one document path, e.g.
/// `RouteDslStep → DoTryData.steps → RouteDslStep`) expands BOTH times;
/// only a ref already on the current chain (a true cycle) is cut.
///
/// Deliberate relaxation vs the mapping-candidates filter: this search runs
/// on RAW ctx. Sound because property declarations can only live on
/// mapping-capable branches — no null/scalar-only branch in this schema
/// declares `properties` — so filtering first would not change the result.
pub(crate) fn lookup_property<'a>(
    ctx: &[&'a serde_json::Value],
    key: &str,
) -> Option<&'a serde_json::Value> {
    let mut chain: Vec<&'a str> = Vec::new();
    ctx.iter()
        .find_map(|c| lookup_property_in(c, key, &mut chain))
}

fn lookup_property_in<'a>(
    node: &'a serde_json::Value,
    key: &str,
    chain: &mut Vec<&'a str>,
) -> Option<&'a serde_json::Value> {
    if let Some(rf) = node.get("$ref").and_then(|v| v.as_str()) {
        if chain.contains(&rf) {
            return None; // true cycle: this ref is already on the chain
        }
        chain.push(rf);
        let resolved = lookup_property_in(resolve_static(node), key, chain);
        chain.pop();
        return resolved;
    }
    if let Some(ps) = node.get("properties").and_then(|p| p.get(key)) {
        return Some(ps);
    }
    for kw in ["allOf", "anyOf", "oneOf"] {
        if let Some(subs) = node.get(kw).and_then(|v| v.as_array()) {
            for sub in subs {
                if let Some(found) = lookup_property_in(sub, key, chain) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// Every leaf subschema reachable from `node` through `$ref` and
/// `anyOf`/`oneOf`/`allOf` expansion. The active-`$ref`-chain guard is
/// push/pop per branch (shared across one top-level expansion): a def
/// reachable through two sibling branches expands both times; only a ref
/// already on the current chain (a true cycle) is cut.
///
/// A node carrying its OWN constraints (`properties` or
/// `additionalProperties`) alongside composition keywords is itself the
/// authoritative leaf — expanding only the branches would silently drop
/// the node's constraints (e.g. `RouteDslPermissionValueSource`'s
/// `additionalProperties: false` beside its `oneOf`). The branches are NOT
/// also expanded: consumers resolve permissiveness with `.any()`, so a
/// strict parent leaf beside permissive branch leaves would still read as
/// permissive. Pure composition wrappers (no properties/AP of their own)
/// keep transparent branch expansion. For `allOf` this is a ceiling: a
/// parent-constrained `allOf` is treated as authoritative without merging
/// the branches' constraints — no such node exists in `ROUTE_SCHEMA` today,
/// so revisit this if one is added.
fn leaves<'a>(
    node: &'a serde_json::Value,
    chain: &mut Vec<&'a str>,
    out: &mut Vec<&'a serde_json::Value>,
) {
    if let Some(rf) = node.get("$ref").and_then(|v| v.as_str()) {
        if chain.contains(&rf) {
            return;
        }
        chain.push(rf);
        leaves(resolve_static(node), chain, out);
        chain.pop();
        return;
    }
    if node.get("properties").is_some() || node.get("additionalProperties").is_some() {
        out.push(node);
        return;
    }
    let mut expanded = false;
    for kw in ["anyOf", "oneOf", "allOf"] {
        if let Some(subs) = node.get(kw).and_then(|v| v.as_array()) {
            for sub in subs {
                leaves(sub, chain, out);
            }
            expanded = true;
        }
    }
    if !expanded {
        out.push(node);
    }
}

fn ctx_leaves<'a>(ctx: &[&'a serde_json::Value]) -> Vec<&'a serde_json::Value> {
    let mut out = Vec::new();
    let mut chain = Vec::new();
    for cand in ctx {
        leaves(cand, &mut chain, &mut out);
    }
    out
}

/// Expand context candidates through `$ref` and composition, dropping every
/// branch that cannot validate a MAPPING instance (`type: "null"`,
/// scalar-only, and array-only branches).
///
/// Nullable-object properties arrive as `anyOf: [$ref, {type: "null"}]`
/// (`response`, `error_handler`, `security_policy`): the null branch must
/// never be consulted for permissiveness or context, or it leaks
/// "permitted" through its absent `additionalProperties`. EVERY consumer
/// that interprets mapping keys runs on this expansion, not on raw ctx.
pub(crate) fn mapping_candidates<'a>(ctx: &[&'a serde_json::Value]) -> Vec<&'a serde_json::Value> {
    ctx_leaves(ctx)
        .into_iter()
        .filter(|l| is_mapping_capable(l))
        .collect()
}

/// True when the node can validate a mapping instance: not declared
/// scalar-only and not declared array-only (untyped schemas can).
fn is_mapping_capable(node: &serde_json::Value) -> bool {
    !is_scalar_only(node) && !is_array_typed(node)
}

/// True when any mapping-capable context candidate PERMITS undeclared keys:
/// `additionalProperties` absent (the JSON-Schema permissive default),
/// `true`, or a typed schema object. `additionalProperties: false` does not
/// permit. Permissiveness in this schema is key-independent.
pub(crate) fn key_permitted(ctx: &[&serde_json::Value]) -> bool {
    mapping_candidates(ctx)
        .iter()
        .any(|c| match c.get("additionalProperties") {
            None => true,
            Some(ap) => *ap == serde_json::Value::Bool(true) || ap.is_object(),
        })
}

/// The typed `additionalProperties` schemas among mapping-capable context
/// candidates — permitted user keys DISPATCH on these when present.
pub(crate) fn typed_ap_schemas<'a>(ctx: &[&'a serde_json::Value]) -> Vec<&'a serde_json::Value> {
    mapping_candidates(ctx)
        .into_iter()
        .filter_map(|c| c.get("additionalProperties"))
        .filter(|ap| ap.is_object())
        .collect()
}

/// True when `node` is a FREE-FORM user map: not array-typed, not
/// scalar-only, and every mapping-capable branch WITHOUT a `properties` key
/// of its own — i.e. an `additionalProperties`-keyed map (`response.headers`,
/// `security_policy.config`, the `parameters` maps). A `"type":
/// ["object", "null"]` LIST counts as object-typed (string equality on
/// `type` would miss `security_policy.config`). Array subschemas
/// (`RouteDslRoute.steps`) and compositions whose branches declare
/// properties are NOT free-form.
pub(crate) fn is_free_form(node: &serde_json::Value) -> bool {
    let r = resolve_static(node);
    if is_array_typed(r) || is_scalar_only(r) {
        return false;
    }
    let branches = mapping_candidates(std::slice::from_ref(&r));
    !branches.is_empty() && branches.iter().all(|b| b.get("properties").is_none())
}

/// `type` names `want` (string or list form).
fn type_includes(node: &serde_json::Value, want: &str) -> bool {
    match node.get("type") {
        Some(serde_json::Value::String(s)) => s == want,
        Some(serde_json::Value::Array(types)) => types.iter().any(|t| t.as_str() == Some(want)),
        _ => false,
    }
}

fn is_array_typed(node: &serde_json::Value) -> bool {
    type_includes(node, "array")
}

/// `type` is present and ONLY names scalar kinds — including the nullable
/// scalar lists (`["string", "null"]`) that URI-bearing properties carry.
fn is_scalar_only(node: &serde_json::Value) -> bool {
    const SCALARS: &[&str] = &["string", "integer", "number", "boolean", "null"];
    match node.get("type") {
        Some(serde_json::Value::String(s)) => SCALARS.contains(&s.as_str()),
        Some(serde_json::Value::Array(types)) => {
            !types.is_empty()
                && types
                    .iter()
                    .all(|t| t.as_str().is_some_and(|s| SCALARS.contains(&s)))
        }
        _ => false,
    }
}

/// Child context for a DECLARED structured container property:
///
/// - the mapping-capable branches when the declared shape is (or includes)
///   an object — when the child is a Sequence, the walk's sequence arm
///   reuses this ctx per item, so `multicast:`'s tolerated direct-sequence
///   form walks its step items against MulticastData itself (their `to` is
///   undeclared there and resolves through the legacy fallback);
/// - otherwise the array-shaped branches, whose `items` the sequence arm
///   turns into per-item context (`steps:` arrays).
pub(crate) fn container_child_ctx(ps: &serde_json::Value) -> Vec<&serde_json::Value> {
    let mapping = mapping_candidates(std::slice::from_ref(&ps));
    if !mapping.is_empty() {
        return mapping;
    }
    array_shaped(std::slice::from_ref(&ps))
}

/// The array-shaped leaves: declared `type: array` or carrying `items`.
fn array_shaped<'a>(ctx: &[&'a serde_json::Value]) -> Vec<&'a serde_json::Value> {
    ctx_leaves(ctx)
        .into_iter()
        .filter(|l| is_array_typed(l) || l.get("items").is_some())
        .collect()
}

/// Per-item context for a sequence: the `items` subschemas of the current
/// context candidates, or empty when no candidate declares `items` (the
/// caller then reuses the ctx — shape tolerance for direct-sequence
/// container forms like `multicast:`).
pub(crate) fn items_ctx<'a>(ctx: &[&'a serde_json::Value]) -> Vec<&'a serde_json::Value> {
    ctx_leaves(ctx)
        .into_iter()
        .filter_map(|l| l.get("items"))
        .map(resolve_static)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use noyalib::cst;

    // ---- namebloat Task 1.1: schema-context machinery ----

    #[test]
    fn root_context_envelope_vs_bare() {
        let envelope = cst::parse_document("routes:\n  - from: direct:a\n").expect("clean parse");
        let envelope_root: Value = (*envelope.as_value()).clone();
        let envelope_ctx = root_context(&envelope_root);
        assert_eq!(envelope_ctx.len(), 1);
        assert!(
            std::ptr::eq(envelope_ctx[0], &*SCHEMA),
            "envelope root resolves to the schema envelope node itself"
        );

        let bare = cst::parse_document("from: direct:a\n").expect("clean parse");
        let bare_root: Value = (*bare.as_value()).clone();
        let bare_ctx = root_context(&bare_root);
        assert_eq!(bare_ctx.len(), 1);
        let route_def = SCHEMA
            .pointer("/$defs/RouteDslRoute")
            .expect("RouteDslRoute def exists");
        assert!(
            std::ptr::eq(bare_ctx[0], route_def),
            "bare-route root resolves to the RouteDslRoute def"
        );
    }

    #[test]
    fn lookup_property_finds_declared_to_through_step_anyof() {
        let step = SCHEMA
            .pointer("/$defs/RouteDslStep")
            .expect("RouteDslStep def exists");
        let ctx = [step];
        let to =
            lookup_property(&ctx, "to").expect("`to` is declared through RouteDslStep's anyOf");
        let expected = SCHEMA
            .pointer("/$defs/ToStep/properties/to")
            .expect("ToStep.to exists");
        assert!(std::ptr::eq(to, expected));

        let multicast =
            lookup_property(&ctx, "multicast").expect("`multicast` is declared on MulticastStep");
        let expected_mc = SCHEMA
            .pointer("/$defs/MulticastStep/properties/multicast")
            .expect("MulticastStep.multicast exists");
        assert!(std::ptr::eq(multicast, expected_mc));
    }

    #[test]
    fn lookup_property_active_chain_allows_repeated_refs() {
        let do_try_data = SCHEMA
            .pointer("/$defs/DoTryData")
            .expect("DoTryData def exists");
        let step = SCHEMA
            .pointer("/$defs/RouteDslStep")
            .expect("RouteDslStep def exists");
        let steps_schema = SCHEMA
            .pointer("/$defs/DoTryData/properties/steps")
            .expect("DoTryData.steps exists");

        let first = lookup_property(&[do_try_data], "steps").expect("DoTryData declares steps");
        assert!(std::ptr::eq(first, steps_schema));

        let do_try =
            lookup_property(&[step], "do_try").expect("RouteDslStep declares do_try via DoTryStep");
        let expected_do_try = SCHEMA
            .pointer("/$defs/DoTryStep/properties/do_try")
            .expect("DoTryStep.do_try exists");
        assert!(std::ptr::eq(do_try, expected_do_try));

        // The repeated `#/$defs/RouteDslStep` / `#/$defs/DoTryData` refs at
        // different chain depths do not cut expansion: the active chain is
        // fresh per top-level call and popped per branch.
        let again =
            lookup_property(&[do_try_data], "steps").expect("repeated refs must not be cut");
        assert!(std::ptr::eq(again, steps_schema));

        // No RouteDslStep branch declares `steps` directly.
        assert!(
            lookup_property(&[step], "steps").is_none(),
            "step wrappers declare their own key, not `steps`"
        );
    }

    #[test]
    fn key_permitted_at_permissive_root_key_permitted_at_strict_step() {
        let root = &*SCHEMA; // envelope: no `additionalProperties` → permissive
        assert!(key_permitted(&[root]));

        let to_step = SCHEMA.pointer("/$defs/ToStep").expect("ToStep def exists");
        assert!(!key_permitted(&[to_step]));

        let error_handler = SCHEMA
            .pointer("/$defs/RouteDslRoute/properties/error_handler")
            .expect("error_handler subschema exists");
        assert!(
            !key_permitted(&[error_handler]),
            "the null branch is dropped and RouteDslErrorHandler rejects undeclared keys"
        );
    }

    #[test]
    fn key_permitted_rejects_on_parent_constrained_composition() {
        // RouteDslPermissionValueSource carries its OWN constraints
        // (`properties` + `additionalProperties: false`) alongside a
        // `oneOf` whose branches restate neither: the node itself is the
        // authoritative leaf. Regression: branch-only expansion made every
        // undeclared key there (e.g. a tolerated nested `to: log:x`) read
        // as permitted — opaque — instead of reaching the legacy fallback.
        let pvs = SCHEMA
            .pointer("/$defs/RouteDslPermissionValueSource")
            .expect("RouteDslPermissionValueSource def exists");
        // Container-named keys are rejected there too, so they still reach
        // the legacy CONTAINER_KEYS fallback (`steps` is a container name).
        assert!(
            !key_permitted(&[pvs]),
            "a parent-constrained composition must reject undeclared keys"
        );
    }

    #[test]
    fn is_free_form_type_list_and_typed_ap() {
        let headers = SCHEMA
            .pointer("/$defs/RouteDslRestResponse/properties/headers")
            .expect("headers subschema exists");
        assert!(
            is_free_form(headers),
            "`type: object` + AP true, no properties"
        );

        let config = SCHEMA
            .pointer("/$defs/RouteDslSecurityPolicy/properties/config")
            .expect("config subschema exists");
        assert!(
            is_free_form(config),
            "a `type: [object, null]` list counts as object-typed"
        );

        let steps = SCHEMA
            .pointer("/$defs/RouteDslRoute/properties/steps")
            .expect("steps subschema exists");
        assert!(!is_free_form(steps), "array subschemas are not free-form");

        let step = SCHEMA
            .pointer("/$defs/RouteDslStep")
            .expect("RouteDslStep def exists");
        assert!(
            !is_free_form(step),
            "composition with declared branches is not free-form"
        );
    }

    #[test]
    fn is_container_rejects_free_form_maps() {
        let headers = SCHEMA
            .pointer("/$defs/RouteDslRestResponse/properties/headers")
            .expect("headers subschema exists");
        assert!(
            !is_container(headers, &SCHEMA),
            "free-form maps are not containers"
        );

        let steps = SCHEMA
            .pointer("/$defs/RouteDslRoute/properties/steps")
            .expect("steps subschema exists");
        assert!(
            is_container(steps, &SCHEMA),
            "step arrays remain containers"
        );
    }
}
