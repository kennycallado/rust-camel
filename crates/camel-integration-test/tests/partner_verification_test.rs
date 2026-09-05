//! Partner verification end to end (ADR-0069 sections 5, 8, 9).
//!
//! Whole-document proofs of the `validate` `partner` grammar: exact
//! recorded-request counts, `method`/`path` filters, immediate and
//! polled (deadline) reads, and the mismatch failure naming the
//! partner, the expectation, and the actual. The flagship boots a
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
    ScenarioVerdict, ambient_std, boot_scenario, parse_scenario_document, partner_scripts_for,
    run_scenario_document,
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
