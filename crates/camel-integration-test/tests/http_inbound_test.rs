//! Inbound HTTP consumer end-to-end (ADR-0069 sections 4, 5, 7).
//!
//! Real boot, real partner, real wire — inbound: the booted system
//! under test CONSUMES http (the consumer route binds the pinned
//! loopback port at `ctx.start()`, rc-w1u9), the harness-owned
//! partner plays CLIENT and sends a request INTO the system under
//! test, and the scenario validates the response the system under
//! test serves — status, headers, body — at the wire. This is where
//! status validation lands: responses, not requests, carry status.
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
    boot_scenario, parse_scenario_document, run_scenario_document,
};

/// The pinned consumer port: the fixture route's `PORT` placeholder
/// default. v1 has no bound-address API, so the route URI pins the
/// port and the document endpoints address it verbatim.
const CONSUMER_PORT: u16 = 18180;

/// The consumer endpoint URI the fixture route serves and the scenario
/// document addresses. This exact string is the router key for the
/// partner's client role (dispatch by endpoint equality).
const CONSUMER_ENDPOINT: &str = "http://127.0.0.1:18180/in";

/// The fixture root: Camel.toml, routes/, and the scenario document.
fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/inbound")
}

/// The layered environment for one document: document `env` first, the
/// harness-provisioned bindings winning over everything, passthrough
/// keys reading the ambient process environment. The inbound fixture
/// provisions no partner (the system under test owns the bind), so the
/// harness tier is empty and the route's `PORT` placeholder resolves
/// to its declared default — ambient variables never reach it.
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

/// Both tests of this suite boot the same pinned consumer port, and
/// the harness runs test binaries concurrently: the guard makes each
/// boot exclusive within this binary. It is a lock, never a delay —
/// readiness itself is asserted per boot, not waited for.
static PORT_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Everything one booted inbound scenario needs: the parsed document,
/// the router (the partner in its client role for the consumer
/// endpoint), the booted context behind a shared lock, and the
/// teardown handle.
struct BootedFixture {
    doc: ScenarioDocument,
    router: PartnerRouter,
    ctx: Arc<tokio::sync::Mutex<CamelContext>>,
    boot: BootHandle,
}

/// Boots the fixture scenario: parse the document, start the partner
/// (no scripted responses — the system under test serves; the partner
/// constructor always binds its own loopback listener, which the
/// client role leaves unused), boot through [`boot_scenario`], and
/// wire the router. Returning means the consumer's listener bound:
/// binding waits at `ctx.start()` through the operator readiness
/// signal.
async fn boot_fixture() -> BootedFixture {
    let doc = parse_scenario_document(&fixture_root().join("consumer.test.yaml"))
        .expect("fixture document must parse");
    let partner = HttpPartner::start(Vec::new())
        .await
        .expect("partner constructor must bind its loopback listener");
    let env = layered_env(&doc, BTreeMap::new());
    let run = boot_scenario(&doc, &fixture_root(), &env)
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
    }
}

/// rc-w1u9 at the wire: [`boot_scenario`] returns only after the
/// Explicit-mode consumer called `mark_ready` behind a bound listener,
/// so a client connects on the very next line — one attempt, refused
/// nowhere. The full document then runs as the wire-level proof: the
/// partner's request crosses in, the response crosses back, and every
/// validation passes.
#[tokio::test]
async fn inbound_consumer_honest_readiness() {
    let _guard = PORT_GUARD.lock().await;
    let fixture = boot_fixture().await;

    // The immediate connect: no retry, no polling, no waiting of any
    // kind. A dishonest boot (returning before the bind) fails here
    // with connection refused.
    let connected = tokio::net::TcpStream::connect(("127.0.0.1", CONSUMER_PORT))
        .await
        .expect("connect must succeed immediately after boot_scenario returns");
    drop(connected);

    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&fixture.doc, &fixture.router, &mut vars).await;
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
    let _guard = PORT_GUARD.lock().await;
    let fixture = boot_fixture().await;

    // Pass variant: the pristine document — status 201, the stamped
    // reply header, the reply body — validates end to end.
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&fixture.doc, &fixture.router, &mut vars).await;
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
                        expectation: Expectation::Equals(Value::String(
                            "never-the-served-body".to_string(),
                        )),
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
    };
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&corrupted, &fixture.router, &mut vars).await;
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
