//! Outbound HTTP bridge end-to-end (ADR-0069 sections 4, 5, 7).
//!
//! Real boot, real partner, real wire: the test boots the full
//! composition root through [`boot_scenario`] (sealed config load,
//! `camel_bundles::boot`, layered-env route interpolation, context
//! start), stimulates the booted route at `direct:start` through the
//! context's producer path, and validates the request that reaches
//! the harness-owned partner on the wire — the normative proof.
//!
//! The method-field tests drive the partner's client role instead:
//! the scenario `send` performs a real HTTP request to the partner's
//! bound address, and `receive` consumes the parked response — the
//! scripted matcher is the oracle for the wire method.
#![cfg(feature = "http")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{CamelError, Value};
use camel_bundles::BootHandle;
use camel_core::CamelContext;

use camel_integration_test::adapters::DirectStimulus;
use camel_integration_test::env_layers::ambient_std;
use camel_integration_test::{
    DocumentOutcome, EndpointRef, Expectation, HttpPartner, LayeredEnv, PartnerAdapter,
    PartnerRouter, Provisioning, RouteSource, ScenarioAction, ScenarioDocument, ScenarioFailure,
    ScenarioTarget, ScenarioVars, ScenarioVerdict, ScriptedResponse, boot_scenario,
    parse_scenario_document, run_scenario_document,
};

/// The doc endpoint URI the fixture declares for the partner. The `:0`
/// port is the router key (dispatch by endpoint equality); the arrival
/// lane keys by request path, so the `:0` form addresses the partner's
/// listener — only the client-role send needs the bound address.
const PARTNER_ENDPOINT: &str = "http://127.0.0.1:0/orders";

/// The fixture root: Camel.toml, routes/, and the scenario document.
fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/outbound")
}

/// A scripted partner response matching one request method on the
/// bridge path. The matcher (method + path) is the oracle: a request
/// that does not match is served the unmatched-500 with an empty body,
/// so the parked-response body validation below is the proof.
fn scripted_response(method: &str, body: &[u8]) -> ScriptedResponse {
    ScriptedResponse {
        method: Some(method.to_string()),
        path: Some("/orders".to_string()),
        status: 200,
        headers: BTreeMap::new(),
        body: body.to_vec(),
    }
}

/// The layered environment for one document: document `env` first,
/// the harness-provisioned bindings (the partner's bound address)
/// winning over everything, passthrough keys reading the ambient
/// process environment.
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

/// A test-only context `Lifecycle` whose `stop()` always fails: a
/// failing teardown dependency for the shutdown-fault injection.
struct FailingTeardown;

#[async_trait]
impl camel_api::lifecycle::Lifecycle for FailingTeardown {
    fn name(&self) -> &str {
        "test-failing-teardown"
    }

    async fn start(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Err(CamelError::Config(
            "test-only failing teardown dependency".to_string(),
        ))
    }
}

/// Everything one booted outbound scenario needs: the parsed document,
/// the router (the partner for the receive endpoint, a
/// [`DirectStimulus`] for the route stimulus), the booted context
/// behind a shared lock, and the teardown handle.
struct BootedFixture {
    doc: ScenarioDocument,
    router: PartnerRouter,
    ctx: Arc<tokio::sync::Mutex<CamelContext>>,
    boot: BootHandle,
}

/// Boots the given scenario document with the given partner registered
/// under `partner_key`: inject `PARTNER=http://127.0.0.1:<bound>` into
/// the harness-provisioned tier, boot through [`boot_scenario`], and
/// wire the router (the `direct:start` stimulus plus the partner).
async fn boot_with(
    doc: ScenarioDocument,
    partner: HttpPartner,
    partner_key: String,
) -> BootedFixture {
    let harness_provisioned = BTreeMap::from([(
        "PARTNER".to_string(),
        format!("http://{}", partner.bound_addr()),
    )]);
    let env = layered_env(&doc, harness_provisioned);
    let run = boot_scenario(&doc, &fixture_root(), &env)
        .await
        .expect("the full boot must succeed");
    let ctx = Arc::new(tokio::sync::Mutex::new(run.ctx));

    let mut adapters: BTreeMap<String, Box<dyn PartnerAdapter>> = BTreeMap::new();
    adapters.insert(
        "direct:start".to_string(),
        Box::new(DirectStimulus::new(Arc::clone(&ctx))),
    );
    adapters.insert(partner_key, Box::new(partner));
    BootedFixture {
        doc,
        router: PartnerRouter::new(adapters),
        ctx,
        boot: run.boot,
    }
}

/// Boots a method-under-test scenario: bind the partner with the
/// scripted response for `method`, build the document against the
/// partner's bound address (the client-role send connects to it), and
/// boot.
async fn boot_method_fixture(method: &str, expected_body: &str) -> BootedFixture {
    let partner = HttpPartner::start(vec![scripted_response(method, expected_body.as_bytes())])
        .await
        .expect("partner must bind 127.0.0.1:0");
    let bound_endpoint = format!("http://{}/orders", partner.bound_addr());
    let doc = method_scenario_document(method, expected_body, &bound_endpoint);
    boot_with(doc, partner, bound_endpoint).await
}

/// Boots the fixture scenario with a freshly bound partner: parse the
/// document, bind the partner on `127.0.0.1:0`, inject
/// `PARTNER=http://127.0.0.1:<bound>` into the harness-provisioned
/// tier, boot through [`boot_scenario`], and wire the router.
async fn boot_fixture() -> BootedFixture {
    let doc = parse_scenario_document(&fixture_root().join("bridge.test.yaml"))
        .expect("fixture document must parse");
    let partner = HttpPartner::start(vec![scripted_response("POST", b"accepted")])
        .await
        .expect("partner must bind 127.0.0.1:0");
    boot_with(doc, partner, PARTNER_ENDPOINT.to_string()).await
}

/// A scenario document over the bridge fixture: send to the partner
/// endpoint with an explicit `method` and no body, receive the parked
/// response, and validate the scripted payload the partner served for
/// that method. The explicit method is the field under test; the
/// parked-response body is the oracle. `partner_endpoint` is the
/// partner's bound address — the client-role send connects to it.
fn method_scenario_document(
    method: &str,
    expected_body: &str,
    partner_endpoint: &str,
) -> ScenarioDocument {
    let partner = EndpointRef {
        endpoint: partner_endpoint.to_string(),
        provisioning: Some(Provisioning::Harness),
        bind_var: Some("PARTNER".to_string()),
    };
    let scenario = vec![
        ScenarioAction::Send {
            to: partner.clone(),
            body: None,
            headers: None,
            method: method.to_string(),
        },
        ScenarioAction::Receive {
            from: partner.clone(),
            deadline: Duration::from_secs(2),
            extract: None,
        },
        ScenarioAction::Validate {
            target: ScenarioTarget::LastReceived(partner),
            expectation: Expectation::Equals(Value::String(expected_body.to_string())),
        },
    ];
    ScenarioDocument {
        route_source: RouteSource::RouteFiles(vec![PathBuf::from("routes/bridge.yaml")]),
        scenario,
        partners: None,
        env: None,
        env_passthrough: None,
        profile: Some("default".to_string()),
    }
}

/// The positive path: the booted route bridges the stimulus to the
/// partner, and the wire arrival validates — method, path, headers,
/// body.
#[tokio::test]
async fn outbound_bridge_validates_wire() {
    let fixture = boot_fixture().await;
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&fixture.doc, &fixture.router, &mut vars).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "every action must pass: {outcome:?}"
    );

    // Teardown: the normal variant asserts clean completion.
    let mut ctx = fixture.ctx.lock().await;
    fixture
        .boot
        .shutdown(&mut ctx)
        .await
        .expect("clean shutdown must complete");
}

/// The explicit `method: PUT` field reaches the partner end to end:
/// the partner's scripted matcher demands PUT, so the parked response
/// body `put-ok` is served only when the wire request really was PUT.
/// Under the legacy `body?POST:GET` rule the send would be GET, the
/// matcher would miss, and the partner would serve the unmatched-500
/// with an empty body — the body validation would fail.
#[tokio::test]
async fn explicit_put_reaches_partner() {
    let fixture = boot_method_fixture("PUT", "put-ok").await;
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&fixture.doc, &fixture.router, &mut vars).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the PUT send must reach the partner and validate: {outcome:?}"
    );

    let mut ctx = fixture.ctx.lock().await;
    fixture
        .boot
        .shutdown(&mut ctx)
        .await
        .expect("clean shutdown must complete");
}

/// The explicit `method: POST` field reaches the partner end to end:
/// the partner's scripted matcher demands POST, so the parked response
/// body `post-ok` is served only when the wire request really was POST.
/// Under the legacy `body?POST:GET` rule a bodyless send would be GET,
/// the matcher would miss, and the partner would serve the unmatched-500
/// with an empty body — the body validation would fail.
#[tokio::test]
async fn bodyless_post_reaches_partner() {
    let fixture = boot_method_fixture("POST", "post-ok").await;
    let mut vars = ScenarioVars::new();
    let outcome = run_scenario_document(&fixture.doc, &fixture.router, &mut vars).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the bodyless POST send must reach the partner and validate: {outcome:?}"
    );

    let mut ctx = fixture.ctx.lock().await;
    fixture
        .boot
        .shutdown(&mut ctx)
        .await
        .expect("clean shutdown must complete");
}

/// The regression shape of rc-eoft: one corrupted header value fails
/// the verdict with a `ValidationMismatch` naming the header.
#[tokio::test]
async fn outbound_bridge_header_corruption_fails() {
    let fixture = boot_fixture().await;
    // Corrupt one header expectation: the route stamps `priority`, the
    // scenario demands `express`.
    let corrupted = ScenarioDocument {
        route_source: fixture.doc.route_source,
        scenario: fixture
            .doc
            .scenario
            .iter()
            .map(|action| {
                if let ScenarioAction::Validate { target, .. } = action
                    && matches!(target, ScenarioTarget::Variable(name) if name == "orderType")
                {
                    ScenarioAction::Validate {
                        target: target.clone(),
                        expectation: Expectation::Equals(Value::String("express".to_string())),
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
    assert_eq!(outcome.verdict, None, "the corrupted header must fail");
    let mismatch = outcome
        .per_action
        .last()
        .and_then(|result| result.as_ref().err())
        .expect("the failing action must carry a failure");
    assert!(
        matches!(mismatch, ScenarioFailure::ValidationMismatch { .. }),
        "expected ValidationMismatch, got {mismatch:?}"
    );
    assert!(
        mismatch.to_string().contains("orderType"),
        "the mismatch must name the header's variable: {mismatch}"
    );

    let mut ctx = fixture.ctx.lock().await;
    fixture
        .boot
        .shutdown(&mut ctx)
        .await
        .expect("shutdown after a verdict failure must still complete");
}

/// Shutdown fault injection, deterministic: a test-only context
/// `Lifecycle` whose `stop()` fails. A passing verdict stays recorded
/// while the shutdown failure reports in the post-verdict slot — exit
/// path 2 at the CLI mapping, never a masked verdict.
#[tokio::test]
async fn shutdown_failure_does_not_mask_verdict() {
    let fixture = boot_fixture().await;
    // Fault injection AFTER the boot, BEFORE the run: the failing
    // teardown dependency sits in the context's lifecycle drain.
    fixture.ctx.lock().await.add_lifecycle(FailingTeardown);

    let mut vars = ScenarioVars::new();
    let mut outcome: DocumentOutcome =
        run_scenario_document(&fixture.doc, &fixture.router, &mut vars).await;
    assert_eq!(outcome.verdict, Some(ScenarioVerdict::Pass));

    // The shutdown-failure slot is the boot-owning caller's to fill
    // (the CLI after `handle.shutdown`, Task 3.5). This test fills it
    // exactly as that mapping will.
    let mut ctx = fixture.ctx.lock().await;
    let shutdown = fixture.boot.shutdown(&mut ctx).await;
    outcome.final_failure = shutdown.err().map(|e| ScenarioFailure::ShutdownFailure {
        message: e.to_string(),
    });

    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "the shutdown failure must not mask the recorded verdict"
    );
    let final_failure = outcome
        .final_failure
        .as_ref()
        .expect("the shutdown failure must be reported deterministically");
    assert!(
        matches!(final_failure, ScenarioFailure::ShutdownFailure { .. }),
        "expected ShutdownFailure, got {final_failure:?}"
    );
    assert!(
        final_failure
            .to_string()
            .contains("test-only failing teardown"),
        "the failure must name the teardown dependency: {final_failure}"
    );
}

/// A receive deadline is honored end-to-end: without the route
/// stimulus (the scenario never sends), the receive reports a
/// verdict-class timeout bounded by the declared deadline.
#[tokio::test]
async fn outbound_receive_deadline_is_real() {
    let mut fixture = boot_fixture().await;
    // Drop the stimulus: only the receive and validate actions run.
    fixture.doc.scenario.retain(|action| {
        matches!(
            action,
            ScenarioAction::Receive { .. } | ScenarioAction::Validate { .. }
        )
    });
    let mut vars = ScenarioVars::new();
    let started = std::time::Instant::now();
    let outcome = run_scenario_document(&fixture.doc, &fixture.router, &mut vars).await;
    assert_eq!(outcome.verdict, None);
    let failure = outcome
        .per_action
        .first()
        .and_then(|result| result.as_ref().err())
        .expect("the receive must fail");
    assert!(
        matches!(failure, ScenarioFailure::ReceiveTimeout { .. }),
        "expected ReceiveTimeout, got {failure:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the 2s deadline must bound the wait, not hang"
    );

    let mut ctx = fixture.ctx.lock().await;
    fixture
        .boot
        .shutdown(&mut ctx)
        .await
        .expect("shutdown must complete after a timeout");
}
