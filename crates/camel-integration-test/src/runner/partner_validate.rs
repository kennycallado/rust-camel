//! Partner verification for the `validate` action's `partner`
//! target (ADR-0069 §5): the filtered recorded-request count, the
//! deadline poll with its early-settle and ceiling rules, and the
//! mismatch-detail renderers. Split out of the runner core so the
//! message-grammar validation and the partner count assertion stay
//! separately navigable; the runner dispatches the `partner` target
//! here.

#[cfg(feature = "http")]
use std::collections::BTreeMap;
use std::time::Duration;

use crate::adapters::PartnerRouter;
#[cfg(feature = "http")]
use crate::adapters::http::HttpWireRequest;
#[cfg(feature = "http")]
use crate::adapters::redact_wire_path;
use crate::document::PartnerExpectation;
#[cfg(feature = "http")]
use crate::document::PathFilter;
// The pure predicates live in camel-matchers; this layer only
// sequences recorded-request snapshots against them.
#[cfg(feature = "http")]
pub(crate) use camel_matchers::render_bound;
#[cfg(feature = "http")]
use camel_matchers::{above_ceiling, bound_holds, settles_early};

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
/// count the reported actual. Every snapshot clones out of the
/// recorder's lock before any await, so no lock spans an await point
/// and the poll sleeps between snapshots. Each iteration snapshots
/// the recorder once and derives both the filtered count and a
/// mismatch's evidence paths from that single snapshot, so the
/// evidence can never list an arrival the count missed.
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
    match deadline {
        // No deadline: one immediate snapshot decides for every
        // bound.
        None => {
            let (requests, actual) = snapshot();
            if bound_holds(&expected.bound, actual) {
                Ok(())
            } else {
                Err(mismatch(actual, &requests))
            }
        }
        // Poll: early success for `Exact`/`AtLeast`, immediate
        // failure above an `AtMost`/`Range` ceiling, and the final
        // snapshot decides at expiry.
        Some(deadline) => {
            let until = tokio::time::Instant::now() + deadline;
            loop {
                let (requests, actual) = snapshot();
                if above_ceiling(&expected.bound, actual) {
                    return Err(mismatch(actual, &requests));
                }
                if settles_early(&expected.bound, actual) {
                    return Ok(());
                }
                let now = tokio::time::Instant::now();
                if now >= until {
                    return if bound_holds(&expected.bound, actual) {
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
    match expected.path.as_ref() {
        Some(PathFilter::Exact(path)) => {
            clauses.push(format!("path {}", redact_wire_path(path, secret_keys)));
        }
        Some(PathFilter::Contains(_)) => {
            clauses.push("pathContains <pattern elided>".to_string());
        }
        Some(PathFilter::Matches(_)) => {
            clauses.push("pathMatches <pattern elided>".to_string());
        }
        // Foreign `#[non_exhaustive]` variants (none today): render by
        // kind with the payload elided, like Contains/Matches above.
        Some(_) => {
            clauses.push("path <filter elided>".to_string());
        }
        None => {}
    }
    if let Some(query) = expected.query.as_ref() {
        for (key, value) in query {
            if secret_keys.iter().any(|secret| secret == key) {
                clauses.push(format!("{key}=<redacted>"));
            } else {
                clauses.push(format!("{key}={value}"));
            }
        }
    }
    clauses.join(", ")
}
