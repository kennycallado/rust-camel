//! Partner verification end to end (ADR-0069 sections 5, 8, 9).
//!
//! Whole-document proofs of the `validate` `partner` grammar: exact
//! recorded-request counts, `method`/`path` filters, immediate and
//! polled (deadline) reads, bound-aware windows (`atLeast` settles
//! early, `atMost` and a range are absence claims over the window),
//! and the mismatch failure naming the partner, the expectation, and
//! the actual. The flagship boots a
//! REAL retrying route over the shipped two-layer bindVar stack — the
//! route's producer faults against the scripted partner once, the
//! route-level `error_handler.retry` redials, and the recorded count
//! proves both attempts crossed the wire.

#![cfg(feature = "http")]

use std::collections::BTreeMap;
use std::sync::Arc;

use camel_integration_test::runner::fill_bind_vars;
use camel_integration_test::{
    DocumentOutcome, EndpointRef, HttpPartner, HttpRecorder, LayeredEnv, PartnerAdapter,
    PartnerRouter, Provisioning, ScenarioAction, ScenarioDocument, ScenarioFailure, ScenarioVars,
    ScenarioVerdict, TransportError, ambient_std, boot_scenario, parse_scenario_document,
    partner_scripts_for, run_scenario_document,
};

/// The endpoint URI the partner-only documents declare for their
/// partner. The `:0` port is the router key; the wire target is the
/// bound address.
const ORDERS: &str = "http://127.0.0.1:0/orders";

/// The endpoint URI the flagship document declares for its partner.
const PARTNER: &str = "http://127.0.0.1:0/order";

/// Three sends, a settle sleep (the send returns at connect; the
/// request writes land asynchronously), then an immediate count
/// validation with a method filter: three wire arrivals, all POST,
/// pass the exact count.
const IMMEDIATE_COUNT_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- sleep:
    duration: 100ms
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {count: 3, method: POST}
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: ok
"#;

/// One send, a settle sleep, then an immediate count validation
/// expecting three: the validation-mismatch failure names the partner
/// URI, the expected count, and the actual count of one.
const COUNT_MISMATCH_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- sleep:
    duration: 100ms
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {count: 3}
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: ok
"#;

/// Two GET and two POST sends, a settle sleep, then an immediate
/// count validation filtered to GET: the filter narrows the count to
/// exactly the two GET arrivals while the recorder holds four.
const FILTERS_NARROW_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: GET
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: GET
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- sleep:
    duration: 100ms
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {count: 2, method: GET}
partners:
  http://127.0.0.1:0/orders:
  - method: GET
    path: /orders
    response:
      status: 200
      body: get-ok
  - method: POST
    path: /orders
    response:
      status: 201
      body: post-ok
"#;

/// Two scenario sends plus one foreign arrival landed by a background
/// task at +300 ms, validated with a poll deadline: the count only
/// settles after the third arrival, so the pass proves the poll.
const DEADLINE_SETTLE_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {count: 3, method: POST}
    deadline: 5s
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: ok
"#;

/// One send, then a count validation expecting three under a one
/// second deadline: nothing else ever arrives, the poll expires, and
/// the final snapshot decides with actual one.
const NEVER_SETTLES_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {count: 3}
    deadline: 1s
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: ok
"#;

/// Two sends then a validation of `atLeast: 3` under a five second
/// deadline, while a burst of two more arrivals lands near-
/// simultaneously at +300 ms: the polled snapshots jump 2 → 4
/// without ever observing 3, and the floor bound settles early on
/// `4 >= 3` instead of waiting the window out.
const AT_LEAST_SETTLES_EARLY_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {atLeast: 3, method: POST}
    deadline: 5s
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: ok
"#;

/// One send, then a validation of `atLeast: 3` under a one second
/// deadline: nothing else ever arrives, the poll expires, and the
/// failure names the floor in its own grammar (`at least 3`) with the
/// final snapshot's actual count.
const AT_LEAST_FAILS_AT_DEADLINE_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- sleep:
    duration: 100ms
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {atLeast: 3}
    deadline: 1s
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: ok
"#;

/// One send, a settle sleep, then a deadline-less validation of
/// `atMost: 2`: one immediate snapshot decides, and one arrival
/// within a ceiling of two passes.
const AT_MOST_IMMEDIATE_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- sleep:
    duration: 100ms
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {atMost: 2}
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: ok
"#;

/// One matching POST held, then an absence claim of `atMost: 1`
/// filtered to POST under a two second deadline, while a nonmatching
/// GET lands mid-window: the claim must NOT pass on the early
/// snapshot — it waits the full window (the method filter keeps the
/// GET out) and decides on the final snapshot.
const AT_MOST_FULL_WINDOW_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- sleep:
    duration: 100ms
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {atMost: 1, method: POST}
    deadline: 2s
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: ok
"#;

/// Three quick matching sends against an absence claim of `atMost: 2`
/// under a five second deadline: the first snapshot above the ceiling
/// fails immediately — the window never burns.
const AT_MOST_FAILS_FAST_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- sleep:
    duration: 100ms
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {atMost: 2}
    deadline: 5s
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: ok
"#;

/// Two matching POSTs held, then an absence claim of `atMost: 2,
/// method: POST` under a five second deadline while a third matching
/// POST lands mid-window: the ceiling crossing happens AFTER the
/// validate's first snapshot, so only the poll's in-window ceiling
/// re-check can fail fast — a ceiling check hoisted out of the poll
/// loop would miss the crossing and burn the whole window.
const AT_MOST_MID_WINDOW_BREACH_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- sleep:
    duration: 100ms
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {atMost: 2, method: POST}
    deadline: 5s
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: ok
"#;

/// One send to `/orders`, then an absence claim of `atMost: 0` over
/// the path `/never` (zero matching arrivals held) under a one second
/// deadline: the claim waits the full window and passes on the final
/// snapshot — an early pass could not prove the absence holds.
const AT_MOST_ZERO_ABSENCE_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {atMost: 0, path: /never}
    deadline: 1s
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: ok
"#;

/// Five quick sends against a range of `atLeast: 2, atMost: 4` under
/// a five second deadline: the count of five sits above the maximum,
/// the first snapshot fails immediately, and the mismatch names the
/// range in its own grammar.
const RANGE_FAILS_FAST_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- sleep:
    duration: 100ms
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {atLeast: 2, atMost: 4}
    deadline: 5s
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: ok
"#;

/// Three sends settled against a range of `atLeast: 2, atMost: 4`
/// under a one second deadline: within bounds, so the claim never
/// settles early — it waits the window and passes on the final
/// snapshot.
const RANGE_FINAL_SNAPSHOT_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- sleep:
    duration: 100ms
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {atLeast: 2, atMost: 4}
    deadline: 1s
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: ok
"#;

/// One POST bind-send to `/orders` (which the `pathContains: bbox=`
/// filter excludes), then a validation of `atLeast: 1` while two
/// foreign GETs land with drifted bbox spellings (percent-encoded and
/// raw): the substring filter counts both drifted arrivals, so two
/// matching requests satisfy the floor.
const PATH_CONTAINS_DRIFT_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- sleep:
    duration: 300ms
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {atLeast: 1, pathContains: "bbox="}
    deadline: 5s
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: ok
"#;

/// One POST bind-send to `/orders`, then a validation of `atLeast: 1`
/// with a `pathMatches` regex and a `query` subset while three foreign
/// GETs land (`/q?bbox=1.5%2C2.5`, `/q?x=1&bbox=1.5%2C2.5`,
/// `/health`): the regex keeps `/health` out, the subset matches the
/// percent-decoded `bbox` pair in both query orders, so two matching
/// requests satisfy the floor.
const PATH_MATCHES_QUERY_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- sleep:
    duration: 300ms
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {atLeast: 1, pathMatches: '^/q\?', query: {bbox: "1.5,2.5"}}
    deadline: 5s
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: ok
"#;

/// The flagship: the scenario dials the pinned route listener as a
/// plain string, the route's producer faults against the scripted
/// partner (first entry `fault: close`), the route-level
/// `error_handler.retry` redials and the second entry serves the
/// healthy body, the client-role receive validates the healthy
/// roundtrip, the server-role receive consumes the faulted arrival,
/// and the partner count validation proves both attempts on the wire.
/// The partner is declared ONLY through the action endpoint ref — the
/// harness folds `bindVar: PARTNER_URL` into the layered env.
const FLAGSHIP_DOC: &str = r#"
routeFiles: [retry-route.yaml]
scenario:
- send:
    to: http://127.0.0.1:18221/order
- receive:
    from: http://127.0.0.1:18221/order
    deadline: 5s
    extract:
      reply: body
- validate:
    target: {variable: reply}
    expectation: ok
- receive:
    from:
      endpoint: http://127.0.0.1:0/order
      provisioning: harness
      bindVar: PARTNER_URL
    deadline: 5s
- validate:
    target: {partner: http://127.0.0.1:0/order}
    expectation: {count: 2, path: /order}
    deadline: 5s
partners:
  http://127.0.0.1:0/order:
  - fault: close
    path: /order
  - path: /order
    response:
      status: 200
      headers:
        content-type: application/json
      body: ok
"#;

/// The endpoint references a document wires, in declaration order:
/// send targets and receive sources. Partner validate targets need no
/// binding of their own — the cross-check requires the URI to be
/// declared by a send/receive, which is where the partner binds.
fn wired_refs(doc: &ScenarioDocument) -> Vec<EndpointRef> {
    doc.scenario
        .iter()
        .flat_map(|action| match action {
            ScenarioAction::Send { to, .. } => vec![to.clone()],
            ScenarioAction::Receive { from, .. } => vec![from.clone()],
            _ => Vec::new(),
        })
        .collect()
}

/// Binds one partner per harness `http` reference (scripted where the
/// document declares a matching `partners:` entry, permissive 200
/// otherwise) and returns the router with the per-endpoint recorders
/// and bound authorities (`host:port` — the raw-dial path a foreign
/// client takes).
async fn bind_doc_partners(
    doc: &ScenarioDocument,
) -> (
    PartnerRouter,
    BTreeMap<String, HttpRecorder>,
    BTreeMap<String, String>,
) {
    let mut adapters: BTreeMap<String, Box<dyn PartnerAdapter>> = BTreeMap::new();
    let mut recorders: BTreeMap<String, HttpRecorder> = BTreeMap::new();
    let mut authorities: BTreeMap<String, String> = BTreeMap::new();
    for reference in wired_refs(doc) {
        if reference.provisioning != Some(Provisioning::Harness)
            || !reference.endpoint.starts_with("http://")
            || adapters.contains_key(&reference.endpoint)
        {
            continue;
        }
        let partner = match partner_scripts_for(doc, &reference.endpoint) {
            Some(scripts) => HttpPartner::start(scripts).await,
            None => HttpPartner::start_permissive(200).await,
        }
        .expect("partner must bind 127.0.0.1:0");
        authorities.insert(reference.endpoint.clone(), partner.bound_addr().to_string());
        recorders.insert(reference.endpoint.clone(), partner.recorder());
        adapters.insert(reference.endpoint.clone(), Box::new(partner));
    }
    (PartnerRouter::new(adapters), recorders, authorities)
}

/// Loads `yaml` through the crate's document path, binds the
/// declared partners, fills the bind variables, runs the whole
/// document, and returns the outcome with the recorders and bound
/// authorities.
async fn run_doc(
    yaml: &str,
) -> (
    DocumentOutcome,
    BTreeMap<String, HttpRecorder>,
    BTreeMap<String, String>,
) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("case.test.yaml");
    std::fs::write(&path, yaml).expect("write case file");
    let doc = parse_scenario_document(&path).expect("document must load");
    let (router, recorders, authorities) = bind_doc_partners(&doc).await;
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired_refs(&doc), &router, &mut vars);
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    (outcome, recorders, authorities)
}

/// One raw HTTP/1.1 POST straight to the partner's bound address —
/// the foreign-client arrival path no router lane owns.
/// `connection: close` makes it one write and one drained read, and
/// the partner records the request before it answers, so a completed
/// call means a recorded arrival.
async fn raw_post(authority: &str, path: &str) {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    let mut stream = tokio::net::TcpStream::connect(authority)
        .await
        .expect("the partner's bound address must accept");
    let request = format!(
        "POST {path} HTTP/1.1\r\nhost: {authority}\r\nconnection: close\r\ncontent-length: 0\r\n\r\n"
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

/// One raw HTTP/1.1 GET straight to the partner's bound address —
/// the same foreign-client arrival path [`raw_post`] takes, with a
/// GET request line so method filters can discriminate it.
async fn raw_get(authority: &str, path: &str) {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    let mut stream = tokio::net::TcpStream::connect(authority)
        .await
        .expect("the partner's bound address must accept");
    let request = format!("GET {path} HTTP/1.1\r\nhost: {authority}\r\nconnection: close\r\n\r\n");
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

/// Loads `yaml` through the crate's document path and binds the
/// declared partners WITHOUT running: the staging point for tests
/// that race background arrivals against the run or measure the
/// run's elapsed time.
async fn stage_doc(
    yaml: &str,
) -> (
    ScenarioDocument,
    PartnerRouter,
    BTreeMap<String, HttpRecorder>,
    BTreeMap<String, String>,
) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("case.test.yaml");
    std::fs::write(&path, yaml).expect("write case file");
    let doc = parse_scenario_document(&path).expect("document must load");
    let (router, recorders, authorities) = bind_doc_partners(&doc).await;
    (doc, router, recorders, authorities)
}

/// Three sends then the immediate count validation: the scenario
/// completes with every action passing, and the recorder holds
/// exactly the three POST arrivals the count read.
#[tokio::test]
async fn immediate_count_assert_e2e() {
    let (outcome, recorders, _authorities) = run_doc(IMMEDIATE_COUNT_DOC).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the count of three POSTs must pass: {outcome:?}"
    );
    let recorded = recorders[ORDERS].recorded_requests();
    assert_eq!(recorded.len(), 3, "three sends must reach the wire");
    for request in &recorded {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/orders");
    }
}

/// One send against an expectation of three: the validation-mismatch
/// failure names the partner URI, the expected count, and the actual
/// count of one.
#[tokio::test]
async fn count_mismatch_fails_e2e() {
    let (outcome, _recorders, _authorities) = run_doc(COUNT_MISMATCH_DOC).await;
    assert_eq!(outcome.verdict, None, "the count mismatch must fail");

    let failure = outcome
        .per_action
        .last()
        .and_then(|result| result.as_ref().err())
        .expect("the validate must fail");
    let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
        panic!("expected ValidationMismatch, got {failure:?}");
    };
    assert!(
        detail.contains(ORDERS) && detail.contains("expected 3, actual 1"),
        "the mismatch must name the partner, expected 3, actual 1: {detail}"
    );
}

/// Two GET and two POST sends against a GET-filtered count of two:
/// the filter narrows the recorded four down to exactly the two GET
/// arrivals, and the scenario passes.
#[tokio::test]
async fn filters_narrow_count_e2e() {
    let (outcome, recorders, _authorities) = run_doc(FILTERS_NARROW_DOC).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the GET-filtered count must pass: {outcome:?}"
    );
    let recorded = recorders[ORDERS].recorded_requests();
    assert_eq!(
        recorded.len(),
        4,
        "all four sends reached the wire; the filter did the narrowing"
    );
}

/// The third arrival lands from a foreign background client at
/// +300 ms, after the poll began: the deadline validation polls until
/// the count settles at three and the scenario passes.
#[tokio::test]
async fn deadline_polls_until_settle_e2e() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("case.test.yaml");
    std::fs::write(&path, DEADLINE_SETTLE_DOC).expect("write case file");
    let doc = parse_scenario_document(&path).expect("document must load");
    let (router, _recorders, authorities) = bind_doc_partners(&doc).await;
    let authority = authorities
        .get(ORDERS)
        .expect("the orders partner must be bound")
        .clone();

    // The foreign arrival: landed after the validate's poll began,
    // far inside its 5 s deadline.
    let dial = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        raw_post(&authority, "/orders").await;
    });

    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired_refs(&doc), &router, &mut vars);
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    dial.await.expect("the background dial task must join");
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the poll must ride out the late third arrival: {outcome:?}"
    );
}

/// Nothing else ever arrives: the poll runs to its one second
/// deadline, the final snapshot decides with actual one, and the
/// mismatch names the expected three and the actual one.
#[tokio::test]
async fn never_settles_fails_at_deadline_e2e() {
    let (outcome, _recorders, _authorities) = run_doc(NEVER_SETTLES_DOC).await;
    assert_eq!(outcome.verdict, None, "the expired poll must fail");

    let failure = outcome
        .per_action
        .last()
        .and_then(|result| result.as_ref().err())
        .expect("the validate must fail");
    let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
        panic!("expected ValidationMismatch, got {failure:?}");
    };
    assert!(
        detail.contains("expected 3, actual 1"),
        "the expired poll must report the final snapshot: {detail}"
    );
}

/// The flagship: a REAL retrying route over the shipped two-layer
/// bindVar stack. The send dials the pinned route listener; the
/// route's producer faults on the first partner dial (`fault: close`),
/// the route-level `error_handler.retry` redials, and the second
/// attempt serves the healthy body the client-role receive validates.
/// The server-role receive consumes the faulted arrival, and the
/// partner count validation proves both attempts crossed the wire —
/// the fault was recorded, the retry happened, and the route settled.
#[tokio::test]
async fn route_retries_faulted_partner_then_count_e2e() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();
    // The route's http producer dials loopback — the same opt-in the
    // shipped two-layer and fixture projects declare.
    std::fs::write(
        root.join("Camel.toml"),
        "log_level = \"info\"\n\n[components.http]\nallow_internal = true\n",
    )
    .expect("write Camel.toml");
    // The committed flagship route file, staged into the temp project
    // root the document's `routeFiles` resolves against.
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/retry-route.yaml"),
        root.join("retry-route.yaml"),
    )
    .expect("stage the committed route fixture");
    let path = root.join("case.test.yaml");
    std::fs::write(&path, FLAGSHIP_DOC).expect("write case file");
    let doc = parse_scenario_document(&path).expect("document must load");

    let scripts = partner_scripts_for(&doc, PARTNER)
        .expect("the flagship declares its partner through the receive ref");
    let partner = HttpPartner::start(scripts)
        .await
        .expect("partner must bind 127.0.0.1:0");
    let recorder = partner.recorder();

    // The CLI driver's env-tier wiring: the harness tier keeps the
    // `http://host:port` form the route file interpolates, while
    // `fill_bind_vars` below keeps the scenario tier at bare
    // `host:port`. `PARTNER_URL` is reserved — it never appears in
    // the document's own `env`.
    let harness_provisioned = BTreeMap::from([(
        "PARTNER_URL".to_string(),
        format!("http://{}", partner.bound_addr()),
    )]);
    let env = LayeredEnv::new(
        doc.env.clone().unwrap_or_default(),
        harness_provisioned,
        doc.env_passthrough.clone().unwrap_or_default(),
        ambient_std(),
    );
    let run = boot_scenario(&doc, root, &env)
        .await
        .expect("the full boot must succeed");
    let ctx = Arc::new(tokio::sync::Mutex::new(run.ctx));

    let mut adapters: BTreeMap<String, Box<dyn PartnerAdapter>> = BTreeMap::new();
    adapters.insert(PARTNER.to_string(), Box::new(partner));
    let router = PartnerRouter::new(adapters);

    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired_refs(&doc), &router, &mut vars);
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "fault, retry, and healthy body must all settle: {outcome:?}"
    );

    let recorded = recorder.recorded_requests();
    assert_eq!(
        recorded.len(),
        2,
        "the faulted attempt and the retry both reach the wire: {recorded:?}"
    );
    for request in &recorded {
        assert_eq!(request.path, "/order");
    }

    let mut guard = ctx.lock().await;
    run.boot
        .shutdown(&mut guard)
        .await
        .expect("clean shutdown must complete");
}

/// Two held sends and a near-simultaneous burst of two more at
/// +300 ms: the snapshots jump 2 → 4 without ever observing 3, and
/// the `atLeast: 3` floor settles early on the burst — the run
/// finishes well inside its 5 s deadline.
#[tokio::test]
async fn at_least_settles_early() {
    let (doc, router, recorders, authorities) = stage_doc(AT_LEAST_SETTLES_EARLY_DOC).await;
    let authority = authorities
        .get(ORDERS)
        .expect("the orders partner must be bound")
        .clone();
    // The burst: two foreign arrivals fired concurrently, so both
    // land inside one 100 ms poll window and no snapshot reads 3.
    let burst = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        tokio::join!(
            raw_post(&authority, "/orders"),
            raw_post(&authority, "/orders")
        );
    });
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired_refs(&doc), &router, &mut vars);
    let started = std::time::Instant::now();
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    let elapsed = started.elapsed();
    burst.await.expect("the burst task must join");

    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the floor must settle on the burst (4 >= 3): {outcome:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "the settle must come early, not at the 5 s deadline: {elapsed:?}"
    );
    assert_eq!(
        recorders[ORDERS].recorded_requests().len(),
        4,
        "both held sends and both burst arrivals must be on the wire"
    );
}

/// One held arrival against `atLeast: 3`: the poll expires, the final
/// snapshot decides, and the mismatch names the floor in its own
/// grammar (`at least 3`) with the actual count of one.
#[tokio::test]
async fn at_least_fails_at_deadline_naming_actual() {
    let (outcome, _recorders, _authorities) = run_doc(AT_LEAST_FAILS_AT_DEADLINE_DOC).await;
    assert_eq!(outcome.verdict, None, "the expired floor must fail");

    let failure = outcome
        .per_action
        .last()
        .and_then(|result| result.as_ref().err())
        .expect("the validate must fail");
    let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
        panic!("expected ValidationMismatch, got {failure:?}");
    };
    assert!(
        detail.contains("expected at least 3, actual 1"),
        "the mismatch must name the floor and the final actual: {detail}"
    );
}

/// One held arrival against `atMost: 2` without a deadline: one
/// immediate snapshot decides, and one arrival inside a ceiling of
/// two passes.
#[tokio::test]
async fn at_most_decides_immediately_without_deadline() {
    let (outcome, _recorders, _authorities) = run_doc(AT_MOST_IMMEDIATE_DOC).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "one arrival must sit inside the ceiling of two: {outcome:?}"
    );
}

/// One matching POST and a nonmatching GET mid-window against
/// `atMost: 1, method: POST` with a 2 s deadline: the absence claim
/// does NOT pass on the early snapshot — it waits the full window
/// (the filter keeps the GET out) and passes on the final snapshot.
#[tokio::test]
async fn at_most_waits_full_window() {
    let (doc, router, _recorders, authorities) = stage_doc(AT_MOST_FULL_WINDOW_DOC).await;
    let authority = authorities
        .get(ORDERS)
        .expect("the orders partner must be bound")
        .clone();
    // The nonmatching straggler: landed mid-window, filtered out by
    // the method clause.
    let straggler = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        raw_get(&authority, "/orders").await;
    });
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired_refs(&doc), &router, &mut vars);
    let started = std::time::Instant::now();
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    let elapsed = started.elapsed();
    straggler.await.expect("the straggler task must join");

    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the final snapshot must hold one POST, inside the ceiling: {outcome:?}"
    );
    assert!(
        elapsed >= std::time::Duration::from_millis(1900),
        "an early pass cannot prove the absence; the claim must wait the full window: {elapsed:?}"
    );
}

/// Three matching arrivals against `atMost: 2` with a 5 s deadline:
/// the first snapshot above the ceiling fails immediately — the
/// mismatch names the ceiling and the actual, and the window never
/// burns.
#[tokio::test]
async fn at_most_fails_fast_above_bound() {
    let (doc, router, _recorders, _authorities) = stage_doc(AT_MOST_FAILS_FAST_DOC).await;
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired_refs(&doc), &router, &mut vars);
    let started = std::time::Instant::now();
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    let elapsed = started.elapsed();

    assert_eq!(
        outcome.verdict, None,
        "three arrivals must break a ceiling of two"
    );
    let failure = outcome
        .per_action
        .last()
        .and_then(|result| result.as_ref().err())
        .expect("the validate must fail");
    let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
        panic!("expected ValidationMismatch, got {failure:?}");
    };
    assert!(
        detail.contains("expected at most 2, actual 3"),
        "the mismatch must name the ceiling and the observed count: {detail}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "the ceiling breach must fail fast, not burn the 5 s window: {elapsed:?}"
    );
}

/// Two matching POSTs held inside a ceiling of two, then a third
/// matching POST lands from a foreign background client at +300 ms —
/// after the validate's poll began: the in-window ceiling re-check
/// fails fast on the mid-window snapshot, and the mismatch names the
/// ceiling and the observed count without burning the 5 s deadline.
#[tokio::test]
async fn at_most_fails_fast_when_ceiling_crossed_mid_window() {
    let (doc, router, _recorders, authorities) = stage_doc(AT_MOST_MID_WINDOW_BREACH_DOC).await;
    let authority = authorities
        .get(ORDERS)
        .expect("the orders partner must be bound")
        .clone();
    // The breaching arrival: lands after the validate's poll began,
    // far inside its 5 s deadline — the crossing must be caught by a
    // mid-window ceiling re-check, not by the deadline's final
    // snapshot.
    let breach = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        raw_post(&authority, "/orders").await;
    });
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired_refs(&doc), &router, &mut vars);
    let started = std::time::Instant::now();
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    let elapsed = started.elapsed();
    breach.await.expect("the breaching dial task must join");

    assert_eq!(
        outcome.verdict, None,
        "the mid-window crossing must break a ceiling of two"
    );
    let failure = outcome
        .per_action
        .last()
        .and_then(|result| result.as_ref().err())
        .expect("the validate must fail");
    let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
        panic!("expected ValidationMismatch, got {failure:?}");
    };
    assert!(
        detail.contains("expected at most 2, actual 3"),
        "the mismatch must name the ceiling and the observed count: {detail}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "the mid-window crossing must fail fast on the poll, not burn the 5 s window: {elapsed:?}"
    );
}

/// Zero matching arrivals against `atMost: 0` over a 1 s window: the
/// absence claim waits the full window and passes on the final
/// snapshot — an instant pass could not prove the absence holds.
#[tokio::test]
async fn at_most_zero_proves_absence_over_window() {
    let (doc, router, _recorders, _authorities) = stage_doc(AT_MOST_ZERO_ABSENCE_DOC).await;
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired_refs(&doc), &router, &mut vars);
    let started = std::time::Instant::now();
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    let elapsed = started.elapsed();

    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "no matching arrival must prove the absence: {outcome:?}"
    );
    assert!(
        elapsed >= std::time::Duration::from_millis(900),
        "an early pass cannot prove the absence; the claim must wait the window: {elapsed:?}"
    );
}

/// Five settled arrivals against the range `atLeast: 2, atMost: 4`
/// with a 5 s deadline: the count sits above the maximum, the first
/// snapshot fails immediately, and the mismatch names the range.
#[tokio::test]
async fn range_fails_fast_above_max() {
    let (doc, router, _recorders, _authorities) = stage_doc(RANGE_FAILS_FAST_DOC).await;
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired_refs(&doc), &router, &mut vars);
    let started = std::time::Instant::now();
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    let elapsed = started.elapsed();

    assert_eq!(
        outcome.verdict, None,
        "five arrivals must break the max of four"
    );
    let failure = outcome
        .per_action
        .last()
        .and_then(|result| result.as_ref().err())
        .expect("the validate must fail");
    let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
        panic!("expected ValidationMismatch, got {failure:?}");
    };
    assert!(
        detail.contains("expected between 2 and 4, actual 5"),
        "the mismatch must name the range and the observed count: {detail}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "the ceiling breach must fail fast, not burn the 5 s window: {elapsed:?}"
    );
}

/// Three settled arrivals against the range `atLeast: 2, atMost: 4`
/// with a 1 s deadline: inside bounds, so the claim never settles
/// early — it waits the window and passes on the final snapshot.
#[tokio::test]
async fn range_passes_on_final_snapshot() {
    let (doc, router, _recorders, _authorities) = stage_doc(RANGE_FINAL_SNAPSHOT_DOC).await;
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired_refs(&doc), &router, &mut vars);
    let started = std::time::Instant::now();
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    let elapsed = started.elapsed();

    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "three arrivals must sit inside the range: {outcome:?}"
    );
    assert!(
        elapsed >= std::time::Duration::from_millis(900),
        "an in-bounds range must wait the window and decide on the final snapshot: {elapsed:?}"
    );
}

/// Two foreign GETs land with drifted bbox spellings (percent-encoded
/// comma and bare value) while the scenario holds one excluded POST:
/// `pathContains: bbox=` counts both drifted arrivals, so two
/// matching requests satisfy `atLeast: 1` end to end.
#[tokio::test]
async fn path_contains_tolerates_encoding_drift_end_to_end() {
    let (doc, router, recorders, authorities) = stage_doc(PATH_CONTAINS_DRIFT_DOC).await;
    let authority = authorities
        .get(ORDERS)
        .expect("the orders partner must be bound")
        .clone();
    // The drifted spellings: both must land before the validate's
    // first snapshot (the scenario sleeps 300 ms first).
    let drift = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        raw_get(&authority, "/q?bbox=1.5%2C2.5").await;
        raw_get(&authority, "/q?bbox=3.0").await;
    });
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired_refs(&doc), &router, &mut vars);
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    drift.await.expect("the drift task must join");

    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "both drifted spellings must satisfy the floor: {outcome:?}"
    );
    assert_eq!(
        recorders[ORDERS].recorded_requests().len(),
        3,
        "the bind-send and both drifted GETs must be on the wire"
    );
}

/// One send to a non-routable address (RFC 5737 TEST-NET-1): the
/// connect phase hangs, so the document-level `sendDeadline: 500ms`
/// fires — the action fails with the deadline transport error long
/// before the thirty-second default. A CI network that routes
/// TEST-NET away (fast refuse) fails the deadline-error assertion:
/// the built-in tripwire.
const HUNG_SEND_DOC: &str = r#"
sendDeadline: 500ms
routeFiles: [routes.yaml]
scenario:
- send:
    to: http://192.0.2.1:9/hook
"#;

/// Three foreign GETs land (`/q?bbox=1.5%2C2.5`, `/q?x=1&bbox=...`,
/// `/health`) while the scenario holds one excluded POST: the
/// `pathMatches` regex keeps `/health` out, the `query` subset
/// matches the percent-decoded `bbox` pair in both query orders, so
/// two matching requests satisfy `atLeast: 1` end to end.
#[tokio::test]
async fn path_matches_and_query_subset_end_to_end() {
    let (doc, router, recorders, authorities) = stage_doc(PATH_MATCHES_QUERY_DOC).await;
    let authority = authorities
        .get(ORDERS)
        .expect("the orders partner must be bound")
        .clone();
    // The three arrivals: all must land before the validate's first
    // snapshot (the scenario sleeps 300 ms first).
    let arrivals = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        raw_get(&authority, "/q?bbox=1.5%2C2.5").await;
        raw_get(&authority, "/q?x=1&bbox=1.5%2C2.5").await;
        raw_get(&authority, "/health").await;
    });
    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired_refs(&doc), &router, &mut vars);
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    arrivals.await.expect("the arrivals task must join");

    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the regex and the query subset must count two matches: {outcome:?}"
    );
    assert_eq!(
        recorders[ORDERS].recorded_requests().len(),
        4,
        "the bind-send and all three foreign GETs must be on the wire"
    );
}

/// A hung send (connect to the non-routable RFC 5737 address never
/// completes) under `sendDeadline: 500ms`: the send action fails with
/// the deadline transport error carrying the document bound, and the
/// whole run stays far under the thirty-second default — the
/// fail-fast proof (rc-tr4w).
#[tokio::test]
async fn send_deadline_bounds_hung_send() {
    let started = std::time::Instant::now();
    let (outcome, _recorders, _authorities) = run_doc(HUNG_SEND_DOC).await;
    let elapsed = started.elapsed();

    assert_eq!(outcome.verdict, None, "the hung send must fail");
    let failure = outcome
        .per_action
        .last()
        .and_then(|result| result.as_ref().err())
        .expect("the send must fail");
    let ScenarioFailure::ActionTransport { source, .. } = failure else {
        panic!("expected ActionTransport, got {failure:?}");
    };
    let TransportError::Deadline { after } = &source else {
        panic!("expected the Deadline transport error, got {source}");
    };
    assert_eq!(
        *after,
        std::time::Duration::from_millis(500),
        "the deadline must be the document's 500ms bound"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the document deadline must fail fast, not burn the 30 s default: {elapsed:?}"
    );
}
