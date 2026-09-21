//! Partner-validate tests (bd rc-uoxk, moved from `runner_test.rs`
//! wholesale): recorded-request count semantics — filter matching,
//! poll-until-deadline, overshoot, mismatch detail rendering and its
//! ADR-0051 redaction (ADR-0069 §5).
//!
//! Compiles only with the `http` feature: every partner validate
//! dials an [`HttpPartner`](crate::adapters::http::HttpPartner) bound
//! at `:0`.
#![cfg(feature = "http")]

use std::collections::BTreeMap;
use std::time::Duration;

use crate::adapters::PartnerRouter;
use crate::adapters::http::{HttpPartner, HttpWireRequest};
use crate::document::{
    CountBound, EndpointRef, PartnerExpectation, PathFilter, RouteSource, ScenarioAction,
    ScenarioDocument, ScenarioTarget, ValidateExpectation,
};
use crate::runner::{
    DocumentOutcome, ScenarioFailure, ScenarioVars, ScenarioVerdict, matching_requests,
    partner_mismatch_detail, render_bound, render_filters, run_scenario_document,
};
use crate::test_util::router_for;

/// A bare endpoint reference with no provisioning and no bind variable
/// (the `runner_test` fixture, repeated here so the module stays
/// self-contained).
fn endpoint(uri: &str) -> EndpointRef {
    EndpointRef {
        endpoint: uri.to_string(),
        provisioning: None,
        bind_var: None,
    }
}

/// A minimal document with the given actions and file-based routes
/// (the `runner_test` fixture, repeated here so the module stays
/// self-contained).
fn doc_with(actions: Vec<ScenarioAction>) -> ScenarioDocument {
    ScenarioDocument {
        source_path: std::path::PathBuf::new(),
        route_source: RouteSource::RouteFiles(vec!["routes.yaml".into()]),
        scenario: actions,
        partners: None,
        env: None,
        env_passthrough: None,
        profile: None,
        send_deadline: None,
        inbound: None,
        logs: None,
    }
}

/// The declared harness endpoint every partner validate here reads.
const ORDERS: &str = "http://127.0.0.1:0/orders";

/// A single-entry router with `partner` registered under the declared
/// `:0` orders endpoint.
fn orders_router(partner: HttpPartner) -> PartnerRouter {
    router_for(ORDERS, partner)
}

/// A POST send to the declared `:0` orders endpoint.
fn orders_send() -> ScenarioAction {
    ScenarioAction::Send {
        to: endpoint(ORDERS),
        body: None,
        headers: None,
        method: "POST".to_string(),
        expect_reply: None,
    }
}

/// A partner validate on the declared orders endpoint: the count
/// expectation with optional method/path filters and an optional poll
/// deadline.
fn partner_validate(
    count: u64,
    method: Option<&str>,
    path: Option<&str>,
    deadline: Option<Duration>,
) -> ScenarioAction {
    ScenarioAction::Validate {
        target: ScenarioTarget::Partner(endpoint(ORDERS)),
        expectation: ValidateExpectation::Partner(PartnerExpectation {
            bound: CountBound::Exact(count),
            method: method.map(str::to_string),
            path: path.map(|path| PathFilter::Exact(path.to_string())),
            query: None,
        }),
        deadline,
        elapsed_at_least: None,
    }
}

/// One raw HTTP/1.1 exchange straight to the partner's bound address —
/// the foreign-client arrival path no router lane owns.
/// `connection: close` makes it one write and one drained read, and
/// the partner records the request before it answers, so a completed
/// call means a recorded arrival.
async fn raw_request(authority: &str, method: &str, path: &str) {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    let mut stream = tokio::net::TcpStream::connect(authority)
        .await
        .expect("the partner's bound address must accept");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nhost: {authority}\r\nconnection: close\r\ncontent-length: 0\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("the raw request must leave");
    let mut sink = Vec::new();
    stream
        .read_to_end(&mut sink)
        .await
        .expect("the partner must close after its response");
}

/// The first failure of an outcome that must have failed.
fn first_failure(outcome: &DocumentOutcome) -> &ScenarioFailure {
    outcome
        .per_action
        .iter()
        .find_map(|result| result.as_ref().err())
        .expect("the document must have failed")
}

/// A wire HTTP request with no headers and no body.
fn wire(method: &str, path: &str) -> HttpWireRequest {
    HttpWireRequest {
        method: method.to_string(),
        path: path.to_string(),
        headers: BTreeMap::new(),
        body: Vec::new(),
    }
}

/// A GET wire request with no headers and no body.
fn wire_get(path: &str) -> HttpWireRequest {
    wire("GET", path)
}

/// Filter semantics of `matching_requests`: the method filter folds
/// ASCII case, the path filter is the exact path-and-query, and `None`
/// filters pass everything.
#[test]
fn matching_requests_filters_method_case_insensitive_and_exact_path() {
    let requests = vec![
        wire("POST", "/orders"),
        wire("GET", "/orders"),
        wire("GET", "/orders?page=2"),
        wire("GET", "/health"),
        wire("delete", "/orders"),
    ];
    // `None` filters pass every request.
    assert_eq!(matching_requests(&requests, None, None, None), 5);
    // The method filter folds ASCII case in both directions.
    assert_eq!(matching_requests(&requests, Some("get"), None, None), 3);
    assert_eq!(matching_requests(&requests, Some("DELETE"), None, None), 1);
    // The Exact path filter is the exact path-and-query: no prefix and
    // no query-blind matching.
    assert_eq!(
        matching_requests(
            &requests,
            None,
            Some(&PathFilter::Exact("/orders".to_string())),
            None
        ),
        3
    );
    assert_eq!(
        matching_requests(
            &requests,
            None,
            Some(&PathFilter::Exact("/orders?page=2".to_string())),
            None
        ),
        1
    );
    // All filters combine conjunctively.
    assert_eq!(
        matching_requests(
            &requests,
            Some("get"),
            Some(&PathFilter::Exact("/orders".to_string())),
            None
        ),
        1
    );
}

/// The Exact path filter is byte-strict on the path-and-query: the
/// percent-encoded comma never equals the decoded comma, so only the
/// request carrying the identical bytes counts. Encoding leniency
/// belongs to Contains/Matches and the decoded query subset, never to
/// the Exact comparison.
#[test]
fn matching_exact_path_is_byte_strict() {
    let requests = vec![wire_get("/q?bbox=1.5%2C2.5"), wire_get("/q?bbox=1.5,2.5")];
    assert_eq!(
        matching_requests(
            &requests,
            None,
            Some(&PathFilter::Exact("/q?bbox=1.5%2C2.5".to_string())),
            None
        ),
        1
    );
}

/// The Contains path filter tolerates encoding differences: the
/// substring `bbox=` appears in both the percent-encoded and the raw
/// comma form of the request path.
#[test]
fn matching_contains_tolerates_encoding() {
    let requests = vec![wire_get("/q?bbox=1.5%2C2.5"), wire_get("/q?bbox=1.5,2.5")];
    assert_eq!(
        matching_requests(
            &requests,
            None,
            Some(&PathFilter::Contains("bbox=".to_string())),
            None
        ),
        2
    );
}

/// The Matches path filter narrows by regex over the recorded
/// path-and-query: only the request the pattern accepts counts.
#[test]
fn matching_regex_narrows() {
    let requests = vec![wire_get("/orders/42"), wire_get("/health")];
    assert_eq!(
        matching_requests(
            &requests,
            None,
            Some(&PathFilter::Matches("^/orders/\\d+$".to_string())),
            None
        ),
        1
    );
}

/// The query subset filter decodes the request's query (percent and
/// `+` forms) and compares pair-wise: every declared pair must appear
/// among the decoded pairs, in any position order.
#[test]
fn matching_query_subset_decodes_and_ignores_order() {
    let requests = vec![wire_get("/q?b=2&a=1%2B1")];
    let query = BTreeMap::from([
        ("a".to_string(), "1+1".to_string()),
        ("b".to_string(), "2".to_string()),
    ]);
    assert_eq!(matching_requests(&requests, None, None, Some(&query)), 1);
}

/// The query subset filter is a subset, not an equality: a declared
/// pair absent from the request's query excludes the request.
#[test]
fn matching_query_subset_absent_pair_excludes() {
    let requests = vec![wire_get("/q?a=1")];
    let query = BTreeMap::from([
        ("a".to_string(), "1".to_string()),
        ("c".to_string(), "3".to_string()),
    ]);
    assert_eq!(matching_requests(&requests, None, None, Some(&query)), 0);
}

/// The method and query subset filters combine conjunctively: the
/// declared lowercase method folds ASCII case onto the uppercased
/// wire records, and only the one request that passes both counts.
#[test]
fn matching_method_composes_with_query() {
    let requests = vec![wire("POST", "/q?a=1"), wire("GET", "/q?a=1")];
    let query = BTreeMap::from([("a".to_string(), "1".to_string())]);
    assert_eq!(
        matching_requests(&requests, Some("post"), None, Some(&query)),
        1
    );
}

/// The immediate snapshot: exact equality passes without a deadline,
/// and a mismatch names the partner URI and both counts. The receive
/// synchronizes the send's spawned client-lane exchange, so the
/// validates read a settled recorder.
#[tokio::test]
async fn immediate_count_passes_and_mismatch_names_counts() {
    let partner = HttpPartner::start_permissive(200)
        .await
        .expect("partner must bind 127.0.0.1:0");
    let router = orders_router(partner);
    let doc = doc_with(vec![
        orders_send(),
        ScenarioAction::Receive {
            from: endpoint(ORDERS),
            deadline: Duration::from_secs(5),
            extract: None,
        },
        partner_validate(1, None, None, None),
        partner_validate(2, None, None, None),
    ]);
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;

    assert!(
        matches!(outcome.per_action[2], Ok(ScenarioVerdict::Pass)),
        "the exact immediate count must pass: {outcome:?}"
    );
    assert_eq!(outcome.verdict, None, "the count: 2 validate must fail");
    let ScenarioFailure::ValidationMismatch { detail, .. } = first_failure(&outcome) else {
        panic!(
            "expected ValidationMismatch, got {:?}",
            first_failure(&outcome)
        );
    };
    assert!(
        detail.contains("partner http://127.0.0.1:0/orders"),
        "the mismatch must name the partner URI: {detail}"
    );
    assert!(
        detail.contains("expected 2, actual 1"),
        "the mismatch must name both counts: {detail}"
    );
}

/// A filtered count mismatch names the applied filter clauses: the one
/// recorded POST to `/orders` matches both filters, so an expectation
/// of 2 fails with `method post, path /orders` spelled out in the
/// detail.
#[tokio::test]
async fn filtered_mismatch_names_method_and_path_clauses() {
    let partner = HttpPartner::start_permissive(200)
        .await
        .expect("partner must bind 127.0.0.1:0");
    let router = orders_router(partner);
    let doc = doc_with(vec![
        orders_send(),
        ScenarioAction::Receive {
            from: endpoint(ORDERS),
            deadline: Duration::from_secs(5),
            extract: None,
        },
        partner_validate(2, Some("post"), Some("/orders"), None),
    ]);
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;

    assert_eq!(outcome.verdict, None, "the filtered count must fail");
    let ScenarioFailure::ValidationMismatch { detail, .. } = first_failure(&outcome) else {
        panic!(
            "expected ValidationMismatch, got {:?}",
            first_failure(&outcome)
        );
    };
    assert!(
        detail.contains("method post"),
        "the mismatch must name the method filter: {detail}"
    );
    assert!(
        detail.contains("path /orders"),
        "the mismatch must name the path filter: {detail}"
    );
    assert!(
        detail.contains("expected 2, actual 1"),
        "the mismatch must name both counts: {detail}"
    );
}

/// The polled snapshot settles: one arrival lands before the run, two
/// more at 300 ms while the validate polls, and the count reaches its
/// expectation long before the 5 s deadline — the pass comes from a
/// poll seeing the settle, not from waiting the deadline out.
#[tokio::test]
async fn poll_passes_once_count_settles() {
    let partner = HttpPartner::start_permissive(200)
        .await
        .expect("partner must bind 127.0.0.1:0");
    let authority = partner.bound_addr().to_string();
    // One arrival before the run: every early snapshot reads 1, below
    // the expectation, so the validate must keep polling.
    raw_request(&authority, "POST", "/orders").await;
    let router = orders_router(partner);
    let settling = tokio::spawn({
        let authority = authority.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            raw_request(&authority, "POST", "/orders").await;
            raw_request(&authority, "POST", "/orders").await;
        }
    });
    let doc = doc_with(vec![partner_validate(
        3,
        None,
        None,
        Some(Duration::from_secs(5)),
    )]);
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;
    settling.await.expect("the settling task must finish");

    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the polled count must settle to 3: {outcome:?}"
    );
}

/// A count above the expectation never passes: arrivals only add, so
/// every polled snapshot and the final one read 4 against an
/// expectation of 3.
#[tokio::test]
async fn overshoot_never_passes() {
    let partner = HttpPartner::start_permissive(200)
        .await
        .expect("partner must bind 127.0.0.1:0");
    let authority = partner.bound_addr().to_string();
    for _ in 0..4 {
        raw_request(&authority, "POST", "/orders").await;
    }
    let router = orders_router(partner);
    let doc = doc_with(vec![partner_validate(
        3,
        None,
        None,
        Some(Duration::from_secs(1)),
    )]);
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;

    assert_eq!(
        outcome.verdict, None,
        "a count above the expectation must never pass: {outcome:?}"
    );
    let ScenarioFailure::ValidationMismatch { detail, .. } = first_failure(&outcome) else {
        panic!(
            "expected ValidationMismatch, got {:?}",
            first_failure(&outcome)
        );
    };
    assert!(
        detail.contains("expected 3, actual 4"),
        "the mismatch must name the final counts: {detail}"
    );
}

/// Deadline expiry reports the final snapshot's count as the actual:
/// one arrival, an expectation of 3, and after the 1 s deadline the
/// failure names actual 1.
#[tokio::test]
async fn deadline_expiry_reports_final_actual() {
    let partner = HttpPartner::start_permissive(200)
        .await
        .expect("partner must bind 127.0.0.1:0");
    let authority = partner.bound_addr().to_string();
    raw_request(&authority, "POST", "/orders").await;
    let router = orders_router(partner);
    let doc = doc_with(vec![partner_validate(
        3,
        None,
        None,
        Some(Duration::from_secs(1)),
    )]);
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&doc, &router, &mut vars, None).await;

    assert_eq!(outcome.verdict, None, "the count must never reach 3");
    let ScenarioFailure::ValidationMismatch { detail, .. } = first_failure(&outcome) else {
        panic!(
            "expected ValidationMismatch, got {:?}",
            first_failure(&outcome)
        );
    };
    assert!(
        detail.contains("actual 1"),
        "the mismatch must report the final snapshot's count: {detail}"
    );
}

/// The partner count mismatch detail lists the recorded request
/// paths, not only the counts, so a failed assertion is diagnosable
/// from the failure text alone (spec: integration-tier, count
/// mismatch lists recorded paths).
#[test]
fn partner_mismatch_detail_lists_recorded_paths() {
    let expected = PartnerExpectation {
        bound: CountBound::Exact(2),
        method: None,
        path: None,
        query: None,
    };
    let detail = partner_mismatch_detail(
        "http://127.0.0.1:0/a",
        &expected,
        1,
        &["/a?b=1".to_string(), "/c".to_string()],
        &[],
    );
    assert!(
        detail.contains("expected 2, actual 1"),
        "the mismatch must name both counts: {detail}"
    );
    assert!(
        detail.contains("/a?b=1"),
        "must list the first path: {detail}"
    );
    assert!(detail.contains("/c"), "must list the second path: {detail}");
}

/// Secret-marked query keys redact in the partner count mismatch
/// detail (ADR-0051 positive secret rule): the partner URI header,
/// the `path` filter echo, and every recorded path mask the secret
/// value, a non-secret pair stays visible, and the secret value never
/// prints.
#[test]
fn count_mismatch_redacts_secrets() {
    let expected = PartnerExpectation {
        bound: CountBound::Exact(2),
        method: None,
        path: Some(PathFilter::Exact(
            "/login?authPassword=hunter2&x=1".to_string(),
        )),
        query: None,
    };
    let detail = partner_mismatch_detail(
        "http://127.0.0.1:0/login?authPassword=hunter2&x=1",
        &expected,
        1,
        &["/login?authPassword=hunter2&x=1".to_string()],
        &["authPassword".to_string()],
    );
    assert!(
        detail.contains("authPassword=***"),
        "the secret value must be masked: {detail}"
    );
    assert!(
        !detail.contains("hunter2"),
        "the secret must never print: {detail}"
    );
    assert!(
        detail.contains("x=1"),
        "non-secret pairs must stay visible: {detail}"
    );
    assert!(
        detail.contains("partner http://127.0.0.1:0/login?authPassword=***&x=1"),
        "the partner URI header must mask the secret too: {detail}"
    );
    assert!(
        detail.contains("path /login?authPassword=***"),
        "the path filter echo must mask the secret too: {detail}"
    );
}

/// The bound grammar of the mismatch detail: each bound kind renders
/// in its own words, and `Exact` keeps the historical `expected N`
/// phrasing the exact-count mismatch tests pin byte-for-byte.
#[test]
fn render_bound_grammar() {
    assert_eq!(render_bound(&CountBound::Exact(3)), "expected 3");
    assert_eq!(render_bound(&CountBound::AtLeast(3)), "expected at least 3");
    assert_eq!(render_bound(&CountBound::AtMost(2)), "expected at most 2");
    assert_eq!(
        render_bound(&CountBound::Range(2, 4)),
        "expected between 2 and 4"
    );
}

/// Filter rendering redacts secret query pairs and elides pattern
/// payloads (ADR-0051 extended to filter payloads): the declared
/// secret pair masks its value, the non-secret pair stays visible,
/// and a `pathContains` pattern renders by kind only — neither the
/// secret value nor the pattern bytes print.
#[test]
fn render_filters_redacts_secret_query_and_elides_patterns() {
    let expected = PartnerExpectation {
        bound: CountBound::AtLeast(1),
        method: Some("GET".to_string()),
        path: Some(PathFilter::Contains("secret".to_string())),
        query: Some(BTreeMap::from([
            ("bbox".to_string(), "1,2".to_string()),
            ("token".to_string(), "abc".to_string()),
        ])),
    };
    let rendered = render_filters(&expected, &["token".to_string()]);
    assert!(
        rendered.contains("token=<redacted>"),
        "the secret pair must mask its value: {rendered}"
    );
    assert!(
        rendered.contains("bbox=1,2"),
        "the non-secret pair must stay visible: {rendered}"
    );
    assert!(
        rendered.contains("method GET"),
        "the method clause must render: {rendered}"
    );
    assert!(
        rendered.contains("pathContains <pattern elided>"),
        "the pattern must render by kind only: {rendered}"
    );
    assert!(
        !rendered.contains("abc"),
        "the secret value must never print: {rendered}"
    );
    assert!(
        !rendered.contains("secret"),
        "the pattern payload must never print: {rendered}"
    );
}
