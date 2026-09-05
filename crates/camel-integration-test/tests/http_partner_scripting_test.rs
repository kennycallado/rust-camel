//! Partner scripting end to end (ADR-0069 sections 5, 8, 9).
//!
//! The first file-run proof of the shipped address/bindVar stack: a
//! YAML string loads through the crate's document path, partners bind
//! (scripted where the document declares a `partners:` entry,
//! permissive otherwise), `fill_bind_vars` writes the bound authority
//! into the scenario variables, and the whole document runs through
//! [`run_scenario_document`]. The assertions inspect the partner's
//! recorder — a literal `:0` dial cannot produce a recorded arrival,
//! so a recording is the proof the send reached the bound address.
//!
//! The helper mirrors the CLI driver's partner mapping (status
//! default 200, headers default empty, body JSON-serialized) at the
//! library level. The scenarios exercise the client-role wire path
//! only — except the two-layer test, which boots one route the same
//! way the CLI driver does, to prove the env-tier binding form.

#![cfg(feature = "http")]

use std::collections::BTreeMap;
use std::sync::Arc;

use camel_integration_test::runner::fill_bind_vars;
use camel_integration_test::{
    DirectStimulus, DocumentOutcome, EndpointRef, HttpPartner, HttpRecorder, LayeredEnv,
    PartnerAdapter, PartnerRouter, Provisioning, ScenarioAction, ScenarioDocument, ScenarioFailure,
    ScenarioTarget, ScenarioVars, ScenarioVerdict, ambient_std, boot_scenario,
    parse_scenario_document, partner_scripts_for, run_scenario_document,
};

/// The endpoint URI every document here declares for its partner. The
/// `:0` port is the router key; the wire target is the bound address.
const ORDERS: &str = "http://127.0.0.1:0/orders";

/// Partner-direct document: a PUT send to the harness-declared `:0`
/// endpoint, a receive extracting the response status, and two
/// validations — the extracted status is 200 and the last received
/// body is the scripted `put-ok`. The `partners:` entry is keyed by
/// the declared URI; its JSON body needs the JSON content type to
/// round-trip as the `put-ok` string.
const PUT_PUT_OK_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
      bindVar: PARTNER
    method: PUT
- receive:
    from:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
      bindVar: PARTNER
    deadline: 5s
    extract:
      status: status
- validate:
    target: {variable: status}
    expectation: 200
- validate:
    target:
      lastReceived: http://127.0.0.1:0/orders
    expectation: put-ok
partners:
  http://127.0.0.1:0/orders:
  - method: PUT
    path: /orders
    response:
      status: 200
      headers:
        content-type: application/json
      body: put-ok
"#;

/// Escape document: a POST whose only body leaf is the escaped
/// `$${not_a_var}`; the partner is permissive (no `partners:` entry),
/// so the recorded wire body is the whole proof.
const ESCAPE_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
      bindVar: PARTNER
    method: POST
    body: '$${not_a_var}'
- receive:
    from:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
      bindVar: PARTNER
    deadline: 5s
"#;

/// Unmatched-script document: the partner scripts only POST /orders,
/// the scenario sends DELETE. The receive extracts the served status,
/// a validation proves it is exactly the unmatched 500, and the final
/// body validation mismatches on the empty body — the failure under
/// assertion.
const DELETE_UNMATCHED_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
      bindVar: PARTNER
    method: DELETE
- receive:
    from:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
      bindVar: PARTNER
    deadline: 5s
    extract:
      status: status
- validate:
    target: {variable: status}
    expectation: 500
- validate:
    target:
      lastReceived: http://127.0.0.1:0/orders
    expectation: 200
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: nope
"#;

/// Declared-but-empty-partners document: the harness reference is
/// declared but its `partners:` entry is an empty sequence, so the
/// partner binds non-permissively (scripted with nothing) and every
/// request is unmatched. The receive extracts the served status, the
/// status validation proves it is exactly the unmatched 500, and the
/// final body validation mismatches on the empty body — the failure
/// under assertion.
const EMPTY_PARTNERS_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
      bindVar: PARTNER
    method: GET
- receive:
    from:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
      bindVar: PARTNER
    deadline: 5s
    extract:
      status: status
- validate:
    target: {variable: status}
    expectation: 500
- validate:
    target:
      lastReceived: http://127.0.0.1:0/orders
    expectation: 200
partners:
  http://127.0.0.1:0/orders: []
"#;

/// Unset-variable document: a send to a dynamic URI whose `missing`
/// variable nothing ever sets. No partner binds (the reference is a
/// plain string), and the send must fail at interpolation, before any
/// dial.
const UNSET_VAR_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: http://${missing}/orders
    method: POST
"#;

/// CRUD-chain document: a POST whose scripted 201 body carries the
/// extracted `orderId`, then a GET whose URI interpolates both
/// `${PARTNER}` and `${orderId}` in string form. The GET script
/// matches the exact path `/orders/ord-7`, so a broken interpolation
/// (the literal `${orderId}` on the wire) misses the script and the
/// final body validation fails.
const CRUD_CHAIN_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
      bindVar: PARTNER
    method: POST
    body:
      sku: abc
- receive:
    from:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
      bindVar: PARTNER
    deadline: 5s
    extract:
      orderId: body.id
- send:
    to: 'http://${PARTNER}/orders/${orderId}'
    method: GET
- receive:
    from: 'http://${PARTNER}/orders/${orderId}'
    deadline: 5s
- validate:
    target:
      lastReceived: 'http://${PARTNER}/orders/${orderId}'
    expectation:
      contains: ord-7
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 201
      headers:
        content-type: application/json
      body:
        id: ord-7
  - method: GET
    path: /orders/ord-7
    response:
      status: 200
      headers:
        content-type: application/json
      body:
        id: ord-7
"#;

/// Roundtrip-by-interpolated-receive document: the send goes through
/// the map-form reference, but the `receive` is declared as the plain
/// string `http://${PARTNER}/orders`. The pass proves the receive
/// found the parked roundtrip: `lane_key_for` resolves the
/// interpolated authority to the registered map-ref key, and a miss
/// would be an unbound endpoint or a receive timeout instead.
const RECEIVE_INTERPOLATED_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
      bindVar: PARTNER
    method: POST
- receive:
    from: 'http://${PARTNER}/orders'
    deadline: 5s
partners:
  http://127.0.0.1:0/orders:
  - method: POST
    path: /orders
    response:
      status: 200
      body: parked-ok
"#;

/// Two-layer document: the same variable name `PARTNER` must be
/// visible on both tiers of one run. The scenario tier carries the
/// bare authority (`host:port`, no scheme — proven by the
/// `http://${PARTNER}/orders` send dialing), and the route env tier
/// carries the `http://host:port` form — the booted route's producer
/// target is exactly `${env:PARTNER}`, so only the full form dials.
/// The direct send is the route stimulus; the third wire arrival, on
/// path `/`, is the producer's env-tier dial.
const TWO_LAYER_DOC: &str = r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
      bindVar: PARTNER
    method: POST
- send:
    to: 'http://${PARTNER}/orders'
    method: GET
- send:
    to: direct:start
    body: env-tier-probe
"#;

/// The endpoint references a document wires, in declaration order:
/// send targets, receive sources, and `lastReceived` validate keys.
/// Shared by the helper and the boot test — `fill_bind_vars` walks
/// exactly this list.
fn wired_refs(doc: &ScenarioDocument) -> Vec<EndpointRef> {
    doc.scenario
        .iter()
        .flat_map(|action| match action {
            ScenarioAction::Send { to, .. } => vec![to.clone()],
            ScenarioAction::Receive { from, .. } => vec![from.clone()],
            ScenarioAction::Validate {
                target: ScenarioTarget::LastReceived(endpoint),
                ..
            } => vec![endpoint.clone()],
            _ => Vec::new(),
        })
        .collect()
}

/// Loads `yaml` through the crate's document path, binds one partner
/// per harness `http` reference (scripted where the document declares
/// a matching `partners:` entry, permissive 200 otherwise), fills the
/// bind variables, runs the whole document, and returns the outcome
/// with the per-endpoint recorders. The script-to-wire mapping is the
/// crate's canonical [`partner_scripts_for`], the same one the CLI
/// driver binds through.
async fn run_doc(yaml: &str) -> (DocumentOutcome, BTreeMap<String, HttpRecorder>) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("case.test.yaml");
    std::fs::write(&path, yaml).expect("write case file");
    let doc = parse_scenario_document(&path).expect("document must load");
    let wired = wired_refs(&doc);

    let mut adapters: BTreeMap<String, Box<dyn PartnerAdapter>> = BTreeMap::new();
    let mut recorders: BTreeMap<String, HttpRecorder> = BTreeMap::new();
    for reference in &wired {
        if reference.provisioning != Some(Provisioning::Harness)
            || !reference.endpoint.starts_with("http://")
        {
            continue;
        }
        let scripts = partner_scripts_for(&doc, &reference.endpoint);
        let partner = match scripts {
            Some(scripts) => HttpPartner::start(scripts).await,
            None => HttpPartner::start_permissive(200).await,
        }
        .expect("partner must bind 127.0.0.1:0");
        recorders.insert(reference.endpoint.clone(), partner.recorder());
        adapters.insert(reference.endpoint.clone(), Box::new(partner));
    }
    let router = PartnerRouter::new(adapters);

    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired, &router, &mut vars);
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    (outcome, recorders)
}

/// The partner-direct path end to end: the send addresses the
/// harness-declared `:0` URI, `fill_bind_vars` supplies the bound
/// authority, and the scripted PUT is served. The recorder saw
/// exactly one arrival on `/orders` as PUT — only the bound address
/// can produce a recording.
#[tokio::test]
async fn partner_direct_send_reaches_bound_address() {
    let (outcome, recorders) = run_doc(PUT_PUT_OK_DOC).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "every action must pass: {outcome:?}"
    );

    let recorded = recorders[ORDERS].recorded_requests();
    assert_eq!(recorded.len(), 1, "exactly one request must reach the wire");
    assert_eq!(recorded[0].method, "PUT");
    assert_eq!(recorded[0].path, "/orders");
}

/// The `$${` escape survives to the wire: the recorded request body
/// is the literal `${not_a_var}`, with no variable lookup attempted.
#[tokio::test]
async fn escape_reaches_wire() {
    let (outcome, recorders) = run_doc(ESCAPE_DOC).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "every action must pass: {outcome:?}"
    );

    let recorded = recorders[ORDERS].recorded_requests();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        String::from_utf8_lossy(&recorded[0].body),
        "${not_a_var}",
        "the escaped leaf must reach the wire as the literal: {:?}",
        recorded[0].body
    );
}

/// A request no script matches gets the unmatched 500 with an empty
/// body: the status validation passes (the received status is exactly
/// 500), and the final body validation is the asserted mismatch whose
/// detail shows the empty body.
#[tokio::test]
async fn unmatched_script_serves_500_empty() {
    let (outcome, _recorders) = run_doc(DELETE_UNMATCHED_DOC).await;
    assert_eq!(outcome.verdict, None, "the last validate must fail");

    assert!(
        matches!(&outcome.per_action[2], Ok(ScenarioVerdict::Pass)),
        "the status validate must pass, proving the received status is 500: {outcome:?}"
    );
    let failure = outcome
        .per_action
        .last()
        .and_then(|result| result.as_ref().err())
        .expect("the last action must have failed");
    let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
        panic!("expected ValidationMismatch, got {failure:?}");
    };
    assert!(
        detail.contains("got \"\""),
        "the mismatch detail must show the empty body: {detail}"
    );
}

/// A harness reference whose `partners:` entry is an explicitly empty
/// sequence is non-permissive: the partner binds scripted-with-nothing,
/// so every request is unmatched and gets the 500 with an empty body.
/// The status validation passes (received status is exactly 500), and
/// the final body validation is the asserted mismatch whose detail
/// shows the empty body.
#[tokio::test]
async fn declared_empty_partners_serves_unmatched_500() {
    let (outcome, _recorders) = run_doc(EMPTY_PARTNERS_DOC).await;
    assert_eq!(outcome.verdict, None, "the last validate must fail");

    assert!(
        matches!(&outcome.per_action[2], Ok(ScenarioVerdict::Pass)),
        "the status validate must pass, proving the received status is 500: {outcome:?}"
    );
    let failure = outcome
        .per_action
        .last()
        .and_then(|result| result.as_ref().err())
        .expect("the last action must have failed");
    let ScenarioFailure::ValidationMismatch { detail, .. } = failure else {
        panic!("expected ValidationMismatch, got {failure:?}");
    };
    assert!(
        detail.contains("got \"\""),
        "the mismatch detail must show the empty body: {detail}"
    );
}

/// A send referencing a variable nothing sets fails the action with
/// the verdict class `VarUnresolved`, before any dial.
#[tokio::test]
async fn unset_variable_fails_verdict() {
    let (outcome, _recorders) = run_doc(UNSET_VAR_DOC).await;
    assert_eq!(outcome.verdict, None);

    let failure = outcome
        .per_action
        .last()
        .and_then(|result| result.as_ref().err())
        .expect("the send must fail");
    assert!(
        matches!(&failure, ScenarioFailure::VarUnresolved { name } if name == "missing"),
        "expected VarUnresolved {{ name: \"missing\" }}, got {failure:?}"
    );
}

/// The CRUD chain end to end: the extracted `orderId` interpolates
/// into the GET's string-form URI, which reaches the scripted
/// exact-path matcher and round-trips through the string-form
/// receive. A non-interpolated `${orderId}` path would miss the
/// `/orders/ord-7` script, serve the unmatched 500 with an empty
/// body, and fail the final `contains` validation.
#[tokio::test]
async fn crud_chain_interpolates_extracted_id() {
    let (outcome, recorders) = run_doc(CRUD_CHAIN_DOC).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the whole chain must pass: {outcome:?}"
    );

    let recorded = recorders[ORDERS].recorded_requests();
    assert_eq!(recorded.len(), 2, "POST then GET on the same partner");
    assert_eq!(recorded[0].method, "POST");
    assert_eq!(recorded[1].method, "GET");
    assert_eq!(
        recorded[1].path, "/orders/ord-7",
        "the extracted id must be on the wire path"
    );
}

/// The `receive` declared as the interpolated string
/// `http://${PARTNER}/orders` finds the roundtrip the map-form send
/// parked: `lane_key_for` resolves the interpolated authority to the
/// registered map-ref key. A miss here is an unbound-endpoint
/// transport failure or a receive timeout, never a pass.
#[tokio::test]
async fn receive_endpoint_interpolates() {
    let (outcome, _recorders) = run_doc(RECEIVE_INTERPOLATED_DOC).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the interpolated receive must find the parked roundtrip: {outcome:?}"
    );
}

/// One run, one partner, one variable name on two tiers: the scenario
/// variable `PARTNER` carries the bare `host:port` authority — proven
/// by the `http://${PARTNER}/orders` send dialing successfully (a
/// scheme-bearing value would double the scheme and fail the dial) —
/// while the route env tier carries `http://host:port` — proven by the
/// booted route whose producer target is exactly `${env:PARTNER}`
/// arriving on the partner recorder at path `/`, the only wire arrival
/// no scenario send can produce.
#[tokio::test]
async fn two_layer_bindvar_both_visible() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();
    // The http producer's SSRF guard rejects loopback targets unless
    // the project allows them — the same opt-in the outbound fixture
    // declares.
    std::fs::write(
        root.join("Camel.toml"),
        "log_level = \"info\"\n\n[components.http]\nallow_internal = true\n",
    )
    .expect("write Camel.toml");
    std::fs::write(
        root.join("routes.yaml"),
        "routes:\n  - id: env-tier-probe\n    from: direct:start\n    steps:\n      - to: ${env:PARTNER}\n",
    )
    .expect("write routes.yaml");
    let path = root.join("case.test.yaml");
    std::fs::write(&path, TWO_LAYER_DOC).expect("write case file");
    let doc = parse_scenario_document(&path).expect("document must load");

    let partner = HttpPartner::start_permissive(200)
        .await
        .expect("partner must bind 127.0.0.1:0");
    let recorder = partner.recorder();

    // The CLI driver's env-tier wiring: the harness tier keeps the
    // `http://host:port` form route files interpolate, while
    // `fill_bind_vars` below keeps the scenario tier at bare
    // `host:port`. Same name, two layers, two forms.
    let harness_provisioned = BTreeMap::from([(
        "PARTNER".to_string(),
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
    adapters.insert(
        "direct:start".to_string(),
        Box::new(DirectStimulus::new(Arc::clone(&ctx))),
    );
    adapters.insert(ORDERS.to_string(), Box::new(partner));
    let router = PartnerRouter::new(adapters);

    let mut vars = ScenarioVars::new();
    fill_bind_vars(&wired_refs(&doc), &router, &mut vars);
    let outcome = run_scenario_document(&doc, &router, &mut vars).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "both layers must be visible under one name: {outcome:?}"
    );

    let recorded = recorder.recorded_requests();
    assert_eq!(recorded.len(), 3, "two scenario sends plus the route dial");
    assert_eq!(recorded[0].method, "POST");
    assert_eq!(recorded[0].path, "/orders");
    assert_eq!(recorded[1].method, "GET");
    assert_eq!(recorded[1].path, "/orders");
    assert_eq!(
        recorded[2].path, "/",
        "only the env-tier producer dial arrives with no path: {:?}",
        recorded[2]
    );

    let mut guard = ctx.lock().await;
    run.boot
        .shutdown(&mut guard)
        .await
        .expect("clean shutdown must complete");
}
