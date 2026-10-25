//! Loader-layer typed probe over interpolation provenance
//! (env-int-placeholder-typing).
//!
//! After a FAILED typed parse of an interpolated document, candidate leaves
//! (whole-scalar substituted placeholders under the top-level `routes:` key
//! whose value is a clean integer or boolean) are coerced to typed scalars
//! in cloned trees;
//! candidate index subsets are tried smallest-first in document order
//! (cap 8), with the caller's QUIET parser as the oracle. First parse success
//! wins; otherwise the original first-pass error stands. Every document that
//! parses today is unaffected — probing runs only after failure.
//!
//! Correctness (minimal-subset argument): a coerced copy parses only if
//! every integer-typed position carrying a candidate is coerced (a strict
//! typed field rejects the string otherwise), so every parsing subset
//! contains S_min = the set of integer-position candidates; the only parsing
//! subset of size |S_min| is S_min itself, so the smallest-first search
//! returns exactly the integer positions — never a polymorphic superset
//! (e.g. a `set_header` value that would also accept the number). Unsigned
//! bounds, narrowing, and negatives are enforced by the real parse, not by
//! heuristics.
//!
//! Scope: only provenance paths under `routes:` — REST blocks, route
//! templates, and every other subtree keep today's semantics and are never
//! probed.

use camel_api::CamelError;
use camel_core::route::RouteDefinition;

use crate::env_interpolation::{ProvenancePath, ProvenanceSeg};
use noyalib::compat::serde_yaml as serde_yml;

/// Maximum number of candidate leaves the subset search will enumerate
/// (2^8 - 1 = 255 in-memory parses worst case). Documents with more
/// candidates keep the first-pass error.
const MAX_PROBE_CANDIDATES: usize = 8;

/// Clean integer: the lexical form `-?(0|[1-9][0-9]*)` (no trim, no leading
/// zeros — YAML 1.1 octal ambiguity; floats, bools, and `1e3` are not
/// clean), then an exact i64 or u64 parse (overflow past u64 is not clean).
/// Returns the number leaf the candidate is coerced to.
// SYNC: this rule is mirrored by camel-config's `clean_i64`
// (crates/camel-config/src/env_int_probe.rs) and camel-lint's
// typing-mirror carve-out (rschema.rs); crate purity forbids the
// dependency. Update all three together. The boolean mirrors are
// camel-config's `clean_bool` (same file) and [`clean_boolean`] below.
pub(crate) fn clean_integer(s: &str) -> Option<serde_yml::Value> {
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
        return Some(serde_yml::Value::Number(serde_yml::Number::from(n)));
    }
    if let Ok(n) = s.parse::<u64>() {
        return Some(serde_yml::Value::Number(serde_yml::Number::from(n)));
    }
    None
}

/// Clean boolean: exactly the lowercase literals `true` and `false`
/// (byte-exact match — no trim, no case folding, no `yes`/`no`, no `1`/`0`).
/// Returns the boolean leaf the candidate is coerced to.
// SYNC: this rule is mirrored by camel-config's `clean_bool`
// (crates/camel-config/src/env_int_probe.rs) and camel-lint's rschema
// `clean_boolean` (crates/camel-lint/src/rules/rschema.rs); crate purity
// forbids the dependencies. Update all three together.
fn clean_boolean(s: &str) -> Option<serde_yml::Value> {
    match s {
        "true" => Some(serde_yml::Value::Bool(true)),
        "false" => Some(serde_yml::Value::Bool(false)),
        _ => None,
    }
}

/// Resolve a structural provenance path against a tree; `None` when a
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

/// Candidate leaves for the probe: provenance paths that (a) start with the
/// top-level `routes` key and (b) resolve to a `Value::String` whose text
/// [`clean_integer`] or [`clean_boolean`] accepts. Document order preserved
/// (provenance order).
fn candidates(tree: &serde_yml::Value, provenance: &[ProvenancePath]) -> Vec<ProvenancePath> {
    provenance
        .iter()
        .filter(|path| {
            matches!(
                path.first(),
                Some(ProvenanceSeg::Key(key)) if key == "routes"
            )
        })
        .filter(|path| match resolve_path(tree, path) {
            Some(serde_yml::Value::String(s)) => {
                clean_integer(s).is_some() || clean_boolean(s).is_some()
            }
            _ => false,
        })
        .cloned()
        .collect()
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

/// Typed probe parse: run `parse(doc_text)`; on failure, coerce candidate
/// leaves (from `provenance`, see [`candidates`]) to their clean-integer
/// numbers or clean booleans in fresh copies of the document and retry
/// smallest-first.
///
/// `parse` MUST be a QUIET parser for this seam: speculative attempts are
/// expected failures and MUST NOT emit `error!`/`warn!`. The CALLER emits
/// the existing single `error!` exactly once, only after the probe returns
/// the final failure (logging replay). No environment reads; no `unwrap`.
pub(crate) fn parse_with_probe<P>(
    doc_text: &str,
    provenance: Option<&[ProvenancePath]>,
    parse: P,
) -> Result<Vec<RouteDefinition>, CamelError>
where
    P: Fn(&str) -> Result<Vec<RouteDefinition>, CamelError>,
{
    let first_error = match parse(doc_text) {
        Ok(routes) => return Ok(routes),
        Err(e) => e,
    };

    let Some(provenance) = provenance else {
        return Err(first_error);
    };
    let base_tree: serde_yml::Value = match serde_yml::from_str(doc_text) {
        Ok(tree) => tree,
        // The interpolated text no longer parses to a plain tree — nothing
        // to navigate; the first-pass error stands.
        Err(_) => return Err(first_error),
    };
    // Candidate paths paired with their coerced scalars. `candidates`
    // accepted these exact paths against the same text moments ago, so the
    // anomaly branches are unreachable; skipping (never unwrapping) keeps
    // the loop total regardless.
    let mut candidate_paths: Vec<ProvenancePath> = Vec::new();
    let mut coerced_values: Vec<serde_yml::Value> = Vec::new();
    for path in candidates(&base_tree, provenance) {
        let coerced = resolve_path(&base_tree, &path).and_then(|leaf| match leaf {
            serde_yml::Value::String(s) => clean_integer(s).or_else(|| clean_boolean(s)),
            _ => None,
        });
        if let Some(value) = coerced {
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
            if let Ok(routes) = try_coerced_subset(
                doc_text,
                &candidate_paths,
                &coerced_values,
                &indices,
                &parse,
            ) {
                return Ok(routes);
            }
            if !next_combination(&mut indices, universe) {
                break;
            }
        }
    }
    Err(first_error)
}

/// Parse `doc_text` fresh, coerce the candidate leaves named by `indices`
/// to their typed scalars, serialize with the crate YAML shim, and run `parse`.
/// Skips (returns `Err`) on any navigation anomaly — a subset that cannot
/// be applied is simply not a parsing subset.
fn try_coerced_subset<P>(
    doc_text: &str,
    candidates: &[ProvenancePath],
    coerced: &[serde_yml::Value],
    indices: &[usize],
    parse: &P,
) -> Result<Vec<RouteDefinition>, CamelError>
where
    P: Fn(&str) -> Result<Vec<RouteDefinition>, CamelError>,
{
    // Fresh tree per attempt — attempts never observe each other's edits.
    let mut tree: serde_yml::Value = serde_yml::from_str(doc_text).map_err(|_| {
        CamelError::RouteError("probe: interpolated document lost tree shape".into())
    })?;
    for &i in indices {
        let leaf = resolve_path_mut(&mut tree, &candidates[i]).ok_or_else(|| {
            CamelError::RouteError("probe: candidate path no longer resolves".into())
        })?;
        match leaf {
            serde_yml::Value::String(_) => *leaf = coerced[i].clone(),
            _ => {
                return Err(CamelError::RouteError(
                    "probe: candidate leaf is no longer a string".into(),
                ));
            }
        }
    }
    let text = serde_yml::to_string(&tree).map_err(|_| {
        CamelError::RouteError("probe: coerced document failed to serialize".into())
    })?;
    parse(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env_interpolation::interpolate_yaml_source_with_provenance;
    use crate::model::ValueSourceDef;
    use std::sync::Arc;

    /// Interpolate `raw` with a `None` lookup (defaults resolve) and run the
    /// probe with the real quiet parser.
    fn probe_defaults(raw: &str) -> Result<Vec<RouteDefinition>, CamelError> {
        let (text, provenance) =
            interpolate_yaml_source_with_provenance(raw, &|_| None).expect("interpolation");
        let provenance = provenance.expect("tree-walk path carries provenance");
        parse_with_probe(&text, Some(&provenance), crate::yaml::parse_yaml_for_probe)
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
        let routes = parse_with_probe(&text, Some(&provenance), crate::yaml::parse_yaml_for_probe)
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

    /// ERROR-level counting layer over a thread-local default subscriber —
    /// same mechanism as camel-core's `capture_warns` precedent.
    struct ErrorCountLayer {
        count: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl<C> tracing_subscriber::Layer<C> for ErrorCountLayer
    where
        C: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, C>,
        ) {
            if *event.metadata().level() == tracing::Level::ERROR {
                self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }

    fn error_counting_subscriber() -> (
        Arc<std::sync::atomic::AtomicUsize>,
        tracing::subscriber::DefaultGuard,
    ) {
        use tracing_subscriber::prelude::*;
        crate::test_support::ensure_global_tracing_default();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let layer = ErrorCountLayer {
            count: Arc::clone(&count),
        };
        let guard = tracing_subscriber::registry().with(layer).set_default();
        (count, guard)
    }

    fn error_count(count: &Arc<std::sync::atomic::AtomicUsize>) -> usize {
        count.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[test]
    fn probe_attempts_log_nothing() {
        // Probe-fixable doc through the REAL seam: pass-1 fails quietly, the
        // probe succeeds, no final-failure replay runs — zero ERROR events.
        let raw = "routes:\n  - id: probe-log\n    from: \"direct:start\"\n    steps:\n      - throttle:\n          max_requests: ${env:RC_PROBE_LOG:-2}\n          period_secs: 1\n";
        let (count, _guard) = error_counting_subscriber();
        let routes = crate::yaml::parse_routes_with_env(raw, &|_| None).unwrap();
        assert_throttle_max_requests(&routes, 2);
        assert_eq!(
            error_count(&count),
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
        let (count, _guard) = error_counting_subscriber();
        assert!(crate::yaml::parse_routes_with_env(raw, &|_| None).is_err());
        assert_eq!(
            error_count(&count),
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

        let (baseline_count, _baseline_guard) = error_counting_subscriber();
        assert!(crate::yaml::parse_yaml(
            "routes:\n  - id: later-stage\n    from: \"direct:start\"\n    steps:\n      - set_header:\n          key: k\n          value: \"123\"\n      - throttle:\n          max_requests: 3\n          strategy: bogus\n"
        )
        .is_err());
        let baseline = error_count(&baseline_count);

        let (count, _guard) = error_counting_subscriber();
        assert!(crate::yaml::parse_routes_with_env(raw, &|_| None).is_err());
        assert_eq!(
            error_count(&count),
            baseline,
            "seam must not amplify logging for later-stage failures"
        );
    }
}
