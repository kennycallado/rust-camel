//! Inbound HTTP consumer end-to-end (ADR-0069 sections 4, 5, 7).
//!
//! Real boot, real partner, real wire — inbound: the booted system
//! under test CONSUMES http (the consumer route binds the staged
//! port-0 listener at `ctx.start()`, rc-5yon), the harness-owned
//! partner plays CLIENT and sends a request INTO the system under
//! test, and the scenario validates the response the system under
//! test serves — status, headers, body — at the wire. This is where
//! status validation lands: responses, not requests, carry status.
//!
//! The consumer's address is never pinned: the document declares
//! `inbound: {bindVar: INBOUND}`, the boot provisions a port-0
//! listener and resolves `${env:INBOUND}` in the route file to the
//! bound URL, and the tests read the same address from the run
//! outcome's `inbound_bound` to target the client send. One
//! fixed-port smoke variant (`fixed_port_backcompat`) proves the
//! pre-bindVar document shape — a literal port in the route URI —
//! still boots and serves.
//!
//! Readiness is honest (rc-w1u9): the http consumer declares Explicit
//! startup mode and calls `mark_ready` only after the listener bound,
//! so [`boot_scenario`] returns with the port already accepting — the
//! tests connect immediately, with no waiting of any kind.
#![cfg(feature = "http")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use camel_api::Value;
use camel_bundles::BootHandle;
use camel_core::CamelContext;

use camel_integration_test::env_layers::ambient_std;
use camel_integration_test::{
    Expectation, HttpPartner, LayeredEnv, PartnerAdapter, PartnerRouter, ScenarioAction,
    ScenarioDocument, ScenarioFailure, ScenarioTarget, ScenarioVars, ScenarioVerdict,
    ValidateExpectation, boot_scenario, parse_scenario_document, run_scenario_document,
};

/// The consumer endpoint in the document grammar: the `${INBOUND}`
/// variable resolves at run time from the boot outcome's
/// `inbound_bound` (rc-5yon) — the full URL `http://127.0.0.1:<port>`
/// the staged listener bound. This exact declared string is the
/// router key for the partner's client role (dispatch by declared
/// endpoint equality) and the `lastReceived` recall key.
const CONSUMER_ENDPOINT: &str = "${INBOUND}/in";

/// The pinned port of the back-compat fixture: the only literal-port
/// consumer left in the suite, and its sole user — no shared static
/// port, no guard needed.
const FIXED_CONSUMER_PORT: u16 = 28180;

/// The fixture root: Camel.toml, routes/, and the scenario document.
fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/inbound")
}

/// The back-compat fixture root: the same project shape, pinned to a
/// literal port — the document shape that predates `inbound:`.
fn fixed_fixture_root() -> PathBuf {
    fixture_root().join("fixed")
}

/// The layered environment for one document: document `env` first,
/// the harness-provisioned bindings winning over everything, passthrough
/// keys reading the ambient process environment. The harness tier
/// passed here stays empty: the inbound fixture's bindVar is filled
/// inside the boot itself (`LayeredEnv::with_harness_var`, rc-5yon),
/// after the staged listener has bound.
fn layered_env(
    doc: &ScenarioDocument,
    harness_provisioned: BTreeMap<String, String>,
) -> LayeredEnv {
    LayeredEnv::new(
        doc.env.clone().unwrap_or_default(),
        harness_provisioned,
        doc.env_passthrough.clone().unwrap_or_default(),
        ambient_std(),
    )
}

/// Everything one booted inbound scenario needs: the parsed document,
/// the router (the partner in its client role for the consumer
/// endpoint), the booted context behind a shared lock, and the
/// teardown handle.
struct BootedFixture {
    doc: ScenarioDocument,
    router: PartnerRouter,
    ctx: Arc<tokio::sync::Mutex<CamelContext>>,
    boot: BootHandle,
    /// The inbound listener's bound address as the boot provisioned it
    /// (rc-5yon); `None` for the fixed-port fixture, which declares no
    /// `inbound:` section.
    inbound_bound: Option<std::net::SocketAddr>,
}

/// Boots one scenario document from `root`: parse, start the partner
/// (no scripted responses — the system under test serves; the partner
/// constructor always binds its own loopback listener, which the
/// client role leaves unused), boot through [`boot_scenario`], and
/// wire the router under the declared endpoint key. Returning means
/// the consumer's listener bound: binding waits at `ctx.start()`
/// through the operator readiness signal.
async fn boot_document(doc_path: &Path, root: &Path) -> BootedFixture {
    let doc = parse_scenario_document(doc_path).expect("fixture document must parse");
    let partner = HttpPartner::start(Vec::new())
        .await
        .expect("partner constructor must bind its loopback listener");
    let env = layered_env(&doc, BTreeMap::new());
    let run = boot_scenario(&doc, root, &env)
        .await
        .expect("the full boot must succeed");
    let ctx = Arc::new(tokio::sync::Mutex::new(run.ctx));

    let mut adapters: BTreeMap<String, Box<dyn PartnerAdapter>> = BTreeMap::new();
    adapters.insert(CONSUMER_ENDPOINT.to_string(), Box::new(partner));
    BootedFixture {
        doc,
        router: PartnerRouter::new(adapters),
        ctx,
        boot: run.boot,
        inbound_bound: run.inbound_bound,
    }
}

/// Boots the staged-listener fixture (the main inbound e2e).
async fn boot_fixture() -> BootedFixture {
    boot_document(&fixture_root().join("consumer.test.yaml"), &fixture_root()).await
}

/// Boots the fixed-port back-compat fixture. The partner registered
/// under `${INBOUND}/in` is the client-role vehicle for the staged
/// fixture; this fixed fixture dials its literal URI through the
/// http-scheme fallback path instead.
async fn boot_fixed_fixture() -> BootedFixture {
    boot_document(
        &fixed_fixture_root().join("consumer.test.yaml"),
        &fixed_fixture_root(),
    )
    .await
}

/// The run vars a staged-listener document needs: `INBOUND` carries
/// the full URL read from the boot outcome's `inbound_bound` (rc-5yon)
/// — the same address discovery resolved into the route, never a
/// re-derived port.
fn inbound_vars(bound: std::net::SocketAddr) -> ScenarioVars {
    let mut vars = ScenarioVars::new();
    vars.set("INBOUND", Value::String(format!("http://{bound}")));
    vars
}

/// rc-w1u9 at the wire: [`boot_scenario`] returns only after the
/// Explicit-mode consumer called `mark_ready` behind a bound listener,
/// so a client connects on the very next line — one attempt, refused
/// nowhere. The full document then runs as the wire-level proof: the
/// partner's request crosses in, the response crosses back, and every
/// validation passes.
#[tokio::test]
async fn inbound_consumer_honest_readiness() {
    let fixture = boot_fixture().await;
    let bound = fixture
        .inbound_bound
        .expect("the staged inbound listener must have bound");

    // The immediate connect: no retry, no polling, no waiting of any
    // kind. A dishonest boot (returning before the bind) fails here
    // with connection refused.
    let connected = tokio::net::TcpStream::connect(bound)
        .await
        .expect("connect must succeed immediately after boot_scenario returns");
    drop(connected);

    let mut vars = inbound_vars(bound);
    let mut outcome = run_scenario_document(&fixture.doc, &fixture.router, &mut vars, None).await;
    // The boot-owning flow forwards the provisioned inbound address to
    // the outcome slot (rc-5yon); `None` for the fixed-port fixture.
    outcome.inbound_bound = fixture.inbound_bound;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "every action must pass: {outcome:?}"
    );

    // Teardown: the boot drains cleanly.
    let mut ctx = fixture.ctx.lock().await;
    fixture
        .boot
        .shutdown(&mut ctx)
        .await
        .expect("clean shutdown must complete");
}

/// The response the system under test serves is validated on the wire:
/// the pass variant proves status, header, and body readbacks (the
/// `status` selector head carries the response code — requests carry
/// no status), and the mismatch variant corrupts the body expectation
/// and proves the failure is a `ValidationMismatch` naming its
/// subject.
#[tokio::test]
async fn inbound_response_validated_on_wire() {
    let fixture = boot_fixture().await;
    let bound = fixture
        .inbound_bound
        .expect("the staged inbound listener must have bound");

    // Pass variant: the pristine document — status 201, the stamped
    // reply header, the reply body — validates end to end.
    let mut vars = inbound_vars(bound);
    let outcome = run_scenario_document(&fixture.doc, &fixture.router, &mut vars, None).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "status, header, and body validations must pass: {outcome:?}"
    );
    assert_eq!(
        vars.get("status"),
        Some(&Value::Number(201.into())),
        "the status selector must have read the wire response code"
    );

    // Mismatch variant: demand a body the system under test never
    // serves. The document's route source moves into the corrupted
    // copy (it is neither Debug-printable nor Clone).
    let corrupted = ScenarioDocument {
        source_path: fixture.doc.source_path.clone(),
        route_source: fixture.doc.route_source,
        scenario: fixture
            .doc
            .scenario
            .iter()
            .map(|action| {
                if let ScenarioAction::Validate { target, .. } = action
                    && matches!(target, ScenarioTarget::LastReceived(endpoint) if endpoint.endpoint == CONSUMER_ENDPOINT)
                {
                    ScenarioAction::Validate {
                        target: target.clone(),
                        expectation: ValidateExpectation::Message(Expectation::Equals(
                            Value::String("never-the-served-body".to_string()),
                        )),
                        deadline: None,
                        elapsed_at_least: None,
                    }
                } else {
                    action.clone()
                }
            })
            .collect(),
        env: fixture.doc.env.clone(),
        env_passthrough: fixture.doc.env_passthrough.clone(),
        profile: fixture.doc.profile.clone(),
        partners: None,
        send_deadline: fixture.doc.send_deadline,
        inbound: fixture.doc.inbound,
        logs: fixture.doc.logs,
    };
    let mut vars = inbound_vars(bound);
    let outcome = run_scenario_document(&corrupted, &fixture.router, &mut vars, None).await;
    assert_eq!(outcome.verdict, None, "the corrupted body must fail");
    let mismatch = outcome
        .per_action
        .last()
        .and_then(|result| result.as_ref().err())
        .expect("the failing action must carry a failure");
    assert!(
        matches!(mismatch, ScenarioFailure::ValidationMismatch { .. }),
        "expected ValidationMismatch, got {mismatch:?}"
    );
    let rendered = mismatch.to_string();
    assert!(
        rendered.contains(CONSUMER_ENDPOINT),
        "the mismatch must name the receiving endpoint: {rendered}"
    );
    assert!(
        rendered.contains("never-the-served-body"),
        "the mismatch must state the demanded body: {rendered}"
    );

    // Teardown: the boot drains cleanly after both variants.
    let mut ctx = fixture.ctx.lock().await;
    fixture
        .boot
        .shutdown(&mut ctx)
        .await
        .expect("clean shutdown must complete");
}

/// The back-compat smoke (rc-5yon): a document with no `inbound:`
/// section, its route pinning a literal loopback port, boots and
/// serves exactly as the pre-bindVar documents did — honest
/// readiness, then the full wire round trip.
#[tokio::test]
async fn fixed_port_backcompat() {
    let fixture = boot_fixed_fixture().await;
    assert!(
        fixture.inbound_bound.is_none(),
        "the fixed-port fixture declares no inbound: section"
    );

    // Honest readiness at the pinned port: the connect succeeds on
    // the first attempt after the boot returns.
    let connected = tokio::net::TcpStream::connect(("127.0.0.1", FIXED_CONSUMER_PORT))
        .await
        .expect("the pinned consumer port must accept immediately after boot");
    drop(connected);

    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&fixture.doc, &fixture.router, &mut vars, None).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the literal-port document must serve end to end: {outcome:?}"
    );

    // Teardown: the boot drains cleanly.
    let mut ctx = fixture.ctx.lock().await;
    fixture
        .boot
        .shutdown(&mut ctx)
        .await
        .expect("clean shutdown must complete");
}
