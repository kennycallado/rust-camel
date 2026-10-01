//! Partner verification for the `validate` action's `partner`
//! target (ADR-0069 §5): the filtered recorded-request count, the
//! per-request shape judgments over the filtered sequence, the
//! deadline poll with its early-settle and ceiling rules, and the
//! mismatch-detail renderers. Split out of the runner core so the
//! message-grammar validation and the partner count assertion stay
//! separately navigable; the runner dispatches the `partner` target
//! here.

#[cfg(feature = "http")]
use std::collections::BTreeMap;
use std::time::Duration;

#[cfg(feature = "http")]
use camel_api::Value;
// The pure predicates live in camel-matchers; this layer only
// sequences recorded-request snapshots against them.
#[cfg(feature = "http")]
pub(crate) use camel_matchers::render_bound;
#[cfg(feature = "http")]
use camel_matchers::{
    ShapeAspect, above_ceiling, bound_holds, request_shape_mismatch, settles_early,
};

use crate::adapters::PartnerRouter;
#[cfg(feature = "http")]
use crate::adapters::http::HttpWireRequest;
#[cfg(feature = "http")]
use crate::adapters::redact_wire_path;
use crate::document::PartnerExpectation;
#[cfg(feature = "http")]
use crate::document::PathFilter;

use super::ScenarioFailure;

/// The poll interval of a partner validate with a deadline
/// (feature `http`): a fresh recorded-request snapshot every 100 ms
/// until the filtered count settles or the deadline passes. The sleep
/// between snapshots means the poll never busy-waits.
#[cfg(feature = "http")]
const PARTNER_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Counts the recorded requests that pass all filters (feature
/// `http`): the `method`, `path`, and `query` subset semantics per
/// [`camel_matchers::matching_count`], each request projected as its
/// `(method, path_and_query)` pair of raw wire bytes.
///
/// This layer is the only place wire leniency lives: arrival-lane
/// keys stay strict raw wire bytes, never canonicalized.
#[cfg(feature = "http")]
pub(crate) fn matching_requests(
    requests: &[HttpWireRequest],
    method: Option<&str>,
    path_filter: Option<&PathFilter>,
    query: Option<&BTreeMap<String, String>>,
) -> usize {
    camel_matchers::matching_count(
        requests
            .iter()
            .map(|request| (request.method.as_str(), request.path.as_str())),
        method,
        path_filter,
        query,
    )
}

/// The filtered recorded sequence, projected for shape judging
/// (feature `http`): every request passing the expectation's
/// `method`, `path`, and `query` filters, as its `(method,
/// path_and_query, projected-body)` triple. The filter is the count
/// logic itself — [`camel_matchers::matching_count`] over a
/// single-element iterator — so the sequence's membership is
/// byte-identical with the filtered count, with zero predicate
/// duplication: this layer only sequences. The body projects through
/// [`super::reply_bytes_value`] (JSON when the bytes parse, lossy
/// text otherwise), the value the `body` aspects judge.
#[cfg(feature = "http")]
pub(crate) fn filtered_projections<'a>(
    requests: &'a [HttpWireRequest],
    method: Option<&str>,
    path_filter: Option<&PathFilter>,
    query: Option<&BTreeMap<String, String>>,
) -> Vec<(&'a str, &'a str, Value)> {
    requests
        .iter()
        .filter(|request| {
            camel_matchers::matching_count(
                std::iter::once((request.method.as_str(), request.path.as_str())),
                method,
                path_filter,
                query,
            ) == 1
        })
        .map(|request| {
            (
                request.method.as_str(),
                request.path.as_str(),
                super::reply_bytes_value(&request.body),
            )
        })
        .collect()
}

/// Asserts the partner expectation against the router's
/// recorded-request snapshots for the declared endpoint key
/// (ADR-0069 §5: what crossed the wire is the normative proof).
///
/// Every snapshot filters by the expectation's `method`, `path`, and
/// `query` subset ([`matching_requests`]) and decides the filtered
/// count per [`CountBound`] (arrivals only add, so the count is
/// monotone non-decreasing). Without a deadline one immediate
/// snapshot decides for every bound. With a deadline the poll runs at
/// [`PARTNER_POLL_INTERVAL`]: `Exact` settles at equality and
/// `AtLeast` at its floor, while `AtMost` and a `Range` are absence
/// claims over the window — they wait the full deadline, fail
/// immediately on any snapshot above the ceiling, and decide on the
/// final snapshot. On expiry one final snapshot decides with its own
/// count the reported actual.
///
/// When the expectation carries `requests` shapes, every snapshot
/// additionally judges the filtered sequence positionally
/// ([`filtered_projections`],
/// [`camel_matchers::request_shape_mismatch`]): a filtered count
/// above the shapes' length, or a present shape mismatch, fails
/// immediately — the recorder is append-only, so a present mismatch
/// never heals — and the poll settles only at equality with every
/// present shape matching. A shape failure renders the mismatched
/// request's aspect detail ([`shape_mismatch_detail`]) alongside the
/// counts evidence.
///
/// Every snapshot clones out of the recorder's lock before any await,
/// so no lock spans an await point and the poll sleeps between
/// snapshots. Each iteration snapshots the recorder once and derives
/// the filtered count and the shape evidence from that single
/// snapshot, so the evidence can never list an arrival the count
/// missed.
#[cfg(feature = "http")]
pub(super) async fn partner_validate_action(
    index: usize,
    uri: &str,
    expected: &PartnerExpectation,
    deadline: Option<Duration>,
    router: &PartnerRouter,
) -> Result<(), ScenarioFailure> {
    // One recorder read per iteration: the filtered count and the
    // evidence paths both derive from the same snapshot, so a request
    // arriving between two reads can never make the evidence list
    // inconsistent with the reported actual.
    let snapshot = || {
        let requests = router.recorded_requests(uri);
        let actual = matching_requests(
            &requests,
            expected.method.as_deref(),
            expected.path.as_ref(),
            expected.query.as_ref(),
        );
        (requests, actual)
    };
    let mismatch = |actual: usize, requests: &[HttpWireRequest]| {
        // The recorded paths of the same snapshot, in arrival order:
        // the wire evidence the mismatch detail lists (redacted).
        let recorded: Vec<String> = requests
            .iter()
            .map(|request| request.path.clone())
            .collect();
        ScenarioFailure::ValidationMismatch {
            action: index,
            detail: partner_mismatch_detail(
                uri,
                expected,
                actual,
                &recorded,
                &router.secret_query_keys(),
            ),
        }
    };
    let shape_failure =
        |requests: &[HttpWireRequest], actual: usize, shape: (usize, ShapeAspect)| {
            let recorded: Vec<String> = requests
                .iter()
                .map(|request| request.path.clone())
                .collect();
            let secrets = router.secret_query_keys();
            ScenarioFailure::ValidationMismatch {
                action: index,
                detail: format!(
                    "{}; {}",
                    shape_mismatch_detail(uri, shape, expected, requests, &secrets),
                    partner_mismatch_detail(uri, expected, actual, &recorded, &secrets),
                ),
            }
        };
    // The per-snapshot failure judgment: `Some` when this snapshot
    // already fails the expectation. With `requests` shapes the
    // judgment is position-aware: a filtered count above the shapes'
    // length, or a present shape mismatch, fails immediately — the
    // recorder is append-only, so neither ever heals.
    let judged_failure = |requests: &[HttpWireRequest], actual: usize| -> Option<ScenarioFailure> {
        match expected.requests.as_deref() {
            Some(shapes) => {
                if actual > shapes.len() {
                    return Some(mismatch(actual, requests));
                }
                let projections = filtered_projections(
                    requests,
                    expected.method.as_deref(),
                    expected.path.as_ref(),
                    expected.query.as_ref(),
                );
                request_shape_mismatch(
                    shapes,
                    projections
                        .iter()
                        .map(|(method, path, body)| (*method, *path, body)),
                )
                .map(|shape| shape_failure(requests, actual, shape))
            }
            // A ceiling broken beyond recovery fails on the first
            // observation instead of waiting the window out.
            None => above_ceiling(&expected.bound, actual).then(|| mismatch(actual, requests)),
        }
    };
    // Whether a snapshot settles the expectation: the shapes path at
    // plain equality (every present shape already matched, or
    // `judged_failure` failed first), the count-only path with the
    // early-settle rules mid-window and the plain bound decision for
    // the no-deadline read and the final expiry snapshot.
    let settles = |actual: usize, final_read: bool| -> bool {
        match expected.requests.as_deref() {
            Some(shapes) => actual == shapes.len(),
            None if final_read => bound_holds(&expected.bound, actual),
            None => settles_early(&expected.bound, actual),
        }
    };
    match deadline {
        // No deadline: one immediate snapshot decides for every
        // expectation.
        None => {
            let (requests, actual) = snapshot();
            if let Some(failure) = judged_failure(&requests, actual) {
                return Err(failure);
            }
            if settles(actual, true) {
                Ok(())
            } else {
                Err(mismatch(actual, &requests))
            }
        }
        // Poll: early success for `Exact`/`AtLeast`, immediate
        // failure above an `AtMost`/`Range` ceiling or on any shape
        // mismatch or over-length count, and the final snapshot
        // decides at expiry.
        Some(deadline) => {
            let until = tokio::time::Instant::now() + deadline;
            loop {
                let (requests, actual) = snapshot();
                if let Some(failure) = judged_failure(&requests, actual) {
                    return Err(failure);
                }
                if settles(actual, false) {
                    return Ok(());
                }
                let now = tokio::time::Instant::now();
                if now >= until {
                    return if settles(actual, true) {
                        Ok(())
                    } else {
                        Err(mismatch(actual, &requests))
                    };
                }
                tokio::time::sleep((until - now).min(PARTNER_POLL_INTERVAL)).await;
            }
        }
    }
}

/// Partner verification needs the http adapter's recording (feature
/// `http`); without the feature the arm fails with the verdict-class
/// mismatch instead of passing silently.
#[cfg(not(feature = "http"))]
pub(super) async fn partner_validate_action(
    index: usize,
    uri: &str,
    expected: &PartnerExpectation,
    deadline: Option<Duration>,
    router: &PartnerRouter,
) -> Result<(), ScenarioFailure> {
    let _ = (uri, expected, deadline, router);
    Err(ScenarioFailure::ValidationMismatch {
        action: index,
        detail: "partner validation requires the `http` feature".to_string(),
    })
}

/// The mismatch detail of a failed partner assertion: the partner
/// URI, the applied filters rendered by kind, the bound in its own
/// grammar with the expected-versus-actual counts, and the recorded
/// request paths — `recorded` carries the RAW wire paths; every
/// secret-marked query value is masked through the shared redactor
/// before it reaches the detail (ADR-0051).
#[cfg(feature = "http")]
pub(crate) fn partner_mismatch_detail(
    uri: &str,
    expected: &PartnerExpectation,
    actual: usize,
    recorded: &[String],
    secret_keys: &[String],
) -> String {
    // The partner URI header may itself carry query bytes (a
    // query-bearing declaration): render it redacted like every
    // recorded path below (ADR-0051).
    let mut detail = format!("partner {}", redact_wire_path(uri, secret_keys));
    let filters = render_filters(expected, secret_keys);
    if !filters.is_empty() {
        detail.push_str(&format!(" ({filters})"));
    }
    detail.push_str(&format!(
        ", {}, actual {actual}",
        render_bound(&expected.bound)
    ));
    // The recorded request paths, deduplicated in arrival order: what
    // actually crossed the wire, every secret-marked query value
    // masked before it reaches the detail (ADR-0051).
    let mut unique: Vec<&str> = Vec::new();
    for path in recorded {
        if !unique.contains(&path.as_str()) {
            unique.push(path);
        }
    }
    if !unique.is_empty() {
        let redacted: Vec<String> = unique
            .iter()
            .map(|path| redact_wire_path(path, secret_keys))
            .collect();
        detail.push_str(&format!(", recorded: [{}]", redacted.join(", ")));
    }
    detail
}

/// Renders the applied filters of a partner expectation for mismatch
/// details (the ADR-0051 redaction law, extended to filter payloads):
/// the method renders `method GET`; an `Exact` path filter renders
/// via [`redact_wire_path`] (a declared filter may carry query bytes,
/// and redaction is idempotent) while `Contains`/`Matches` render by
/// KIND only — the pattern payload never prints; each declared
/// `query` pair renders `k=v` except keys in `secret_keys`, which
/// render `k=<redacted>`. An expectation with no filters renders the
/// empty string.
#[cfg(feature = "http")]
pub(crate) fn render_filters(expected: &PartnerExpectation, secret_keys: &[String]) -> String {
    let mut clauses: Vec<String> = Vec::new();
    if let Some(method) = expected.method.as_deref() {
        clauses.push(format!("method {method}"));
    }
    if let Some(filter) = expected.path.as_ref() {
        clauses.push(render_path_filter(filter, secret_keys));
    }
    if let Some(query) = expected.query.as_ref() {
        clauses.push(render_query_subset(query, secret_keys));
    }
    clauses.join(", ")
}

/// Renders one path filter the way [`render_filters`] renders the
/// outer `path` clause: `Exact` through [`redact_wire_path`],
/// `Contains`/`Matches` by KIND only with the pattern payload elided.
/// Shared with the shape detail's `path` aspect, so a shape's
/// expected side renders byte-identically with the outer filter
/// clause.
#[cfg(feature = "http")]
fn render_path_filter(filter: &PathFilter, secret_keys: &[String]) -> String {
    match filter {
        PathFilter::Exact(path) => format!("path {}", redact_wire_path(path, secret_keys)),
        PathFilter::Contains(_) => "pathContains <pattern elided>".to_string(),
        PathFilter::Matches(_) => "pathMatches <pattern elided>".to_string(),
        // Foreign `#[non_exhaustive]` variants (none today): render by
        // kind with the payload elided, like Contains/Matches above.
        _ => "path <filter elided>".to_string(),
    }
}

/// Renders a declared `query` subset the way [`render_filters`]
/// renders it: each pair `k=v`, except keys in `secret_keys`, which
/// render `k=<redacted>`. Shared with the shape detail's `query`
/// aspect, so a shape's expected side renders byte-identically with
/// the outer filter clause.
#[cfg(feature = "http")]
fn render_query_subset(query: &BTreeMap<String, String>, secret_keys: &[String]) -> String {
    query
        .iter()
        .map(|(key, value)| {
            if secret_keys.iter().any(|secret| secret == key) {
                format!("{key}=<redacted>")
            } else {
                format!("{key}={value}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The grammar name of a failed shape aspect (the diagnostic
/// vocabulary the spec pins: `method`, `path`, `query`, `body`).
#[cfg(feature = "http")]
fn aspect_name(aspect: &ShapeAspect) -> &'static str {
    match aspect {
        ShapeAspect::Method => "method",
        ShapeAspect::Path => "path",
        ShapeAspect::Query => "query",
        ShapeAspect::Body => "body",
        // Foreign `#[non_exhaustive]` aspects (none today).
        _ => "request",
    }
}

/// The mismatch detail of a failed per-request shape assert: the
/// partner URI, the one-based request within the FILTERED sequence,
/// the failed aspect, and the expected-versus-observed rendering, per
/// aspect under the ADR-0051 redaction law — `method` renders plain
/// method texts; `path` renders the shape's path filter exactly like
/// [`render_filters`] and the recorded path through [`redact_wire_path`];
/// `query` renders the declared pairs with secret keys masked and the
/// observed redacted wire path; `body` renders through the shared
/// message-validate renderer. Headers never render: only `method`,
/// `path`, and `body` are read. The mismatch's index selects from the
/// filtered sequence re-derived from `observed` through the
/// expectation's outer filters — the same filtering the action's
/// snapshot used, so both name the same request.
#[cfg(feature = "http")]
pub(crate) fn shape_mismatch_detail(
    partner: &str,
    mismatch: (usize, ShapeAspect),
    expected: &PartnerExpectation,
    observed: &[HttpWireRequest],
    secret_keys: &[String],
) -> String {
    let (index, aspect) = mismatch;
    let shapes = expected.requests.as_deref().unwrap_or(&[]);
    let filtered = filtered_projections(
        observed,
        expected.method.as_deref(),
        expected.path.as_ref(),
        expected.query.as_ref(),
    );
    let projection = filtered.get(index);
    let observed_method = projection.map(|(method, _, _)| *method);
    let observed_path = projection.map(|(_, path, _)| *path);
    let observed_body = projection.map(|(_, _, body)| body);
    let shape = shapes.get(index);
    let rendered = match &aspect {
        ShapeAspect::Method => format!(
            "expected {}, got {}",
            shape
                .and_then(|shape| shape.method.as_deref())
                .unwrap_or("any"),
            observed_method.unwrap_or(""),
        ),
        ShapeAspect::Path => format!(
            "expected {}, got {}",
            shape
                .and_then(|shape| shape.path.as_ref())
                .map(|filter| render_path_filter(filter, secret_keys))
                .unwrap_or_else(|| "any".to_string()),
            observed_path
                .map(|path| redact_wire_path(path, secret_keys))
                .unwrap_or_default(),
        ),
        ShapeAspect::Query => format!(
            "expected {}, got {}",
            shape
                .and_then(|shape| shape.query.as_ref())
                .map(|query| render_query_subset(query, secret_keys))
                .unwrap_or_default(),
            observed_path
                .map(|path| redact_wire_path(path, secret_keys))
                .unwrap_or_default(),
        ),
        ShapeAspect::Body => {
            match shape.and_then(|shape| shape.body.as_ref()) {
                // The only reachable pairing: the `body` aspect fails
                // only when a declared expectation rejected the
                // projected value.
                Some(expectation) => match observed_body {
                    Some(body) => super::render_expectation_mismatch(expectation, body),
                    None => "expected a body, got none".to_string(),
                },
                None => "expected a body, got none".to_string(),
            }
        }
        // Foreign `#[non_exhaustive]` aspects (none today): render by
        // kind without values, like the elided filter payloads.
        _ => "expected the declared shape, got the recorded request".to_string(),
    };
    format!(
        "partner {}: request {} (of the filtered sequence) {}: {rendered}",
        redact_wire_path(partner, secret_keys),
        index + 1,
        aspect_name(&aspect),
    )
}
