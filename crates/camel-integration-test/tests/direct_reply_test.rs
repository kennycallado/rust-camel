//! Direct reply assertion (rc-qvz6, ADR-0072 consume-only rule): a
//! request/reply route whose only observable is the synchronous
//! `direct:` reply is asserted through `expectReply` on the send,
//! consuming the same matcher verbs `validate` parses. These tests
//! run WITHOUT the `http` feature — no partner listener is involved;
//! the reply rides the booted context's own producer path
//! (`DirectStimulus`), the same mechanism the CLI driver registers.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use camel_component_api::test_support::acquire_deadline;
use camel_integration_test::DocumentOutcome;
use camel_integration_test::{
    DirectStimulus, LayeredEnv, ScenarioDocument, ScenarioFailure, ScenarioVerdict, ambient_std,
    boot_scenario, parse_scenario_document, run_scenario_document,
};
use tokio::sync::Mutex;

mod common;

use common::router_for;

/// Writes a temporary project — a minimal `Camel.toml`, the echo
/// route file, and the scenario document — and parses the document.
fn project(route: &str, doc: &str) -> (tempfile::TempDir, ScenarioDocument) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("Camel.toml"), "# minimal\n").expect("write Camel.toml");
    std::fs::write(dir.path().join("routes.yaml"), route).expect("write route file");
    let doc_path = dir.path().join("case.test.yaml");
    std::fs::write(&doc_path, doc).expect("write document");
    let document = parse_scenario_document(&doc_path).expect("document parses");
    (dir, document)
}

/// Boots the document, registers the context-stimulus adapter for
/// `direct:echo`, runs the whole document, then tears the boot down.
/// A shutdown failure fills the post-verdict slot and never masks the
/// recorded verdict; the tests below assert on that recorded state.
async fn run_direct(doc: &ScenarioDocument, root: &Path) -> DocumentOutcome {
    let env = LayeredEnv::new(BTreeMap::new(), BTreeMap::new(), Vec::new(), ambient_std());
    let run = boot_scenario(doc, root, &env)
        .await
        .expect("scenario boots");
    let ctx = Arc::new(Mutex::new(run.ctx));
    let router = router_for("direct:echo", DirectStimulus::new(Arc::clone(&ctx)));
    let mut vars = camel_integration_test::ScenarioVars::new();
    let mut outcome = run_scenario_document(doc, &router, &mut vars, None).await;
    // The boot result carries the inbound listener's bound address
    // (rc-5yon); the boot-owning run flow forwards it to the outcome
    // slot, like the shutdown slot below.
    outcome.inbound_bound = run.inbound_bound;
    let mut ctx =
        acquire_deadline(&ctx, "scenario ctx (run_direct)", Duration::from_secs(10)).await;
    if let Err(e) = run.boot.shutdown(&mut ctx).await {
        outcome.final_failure = Some(ScenarioFailure::ShutdownFailure {
            message: e.to_string(),
        });
    }
    outcome
}

/// A send whose `expectReply` matches the route's synchronous reply
/// body passes: the direct producer's reply exchange is observable
/// through the shared matcher verbs (`contains` here).
#[tokio::test]
async fn expect_reply_matches_direct_body() {
    let (dir, doc) = project(
        r#"
routes:
  - id: echo-route
    from: direct:echo
    steps:
      - set_body: "ack-7f3a"
"#,
        r#"
routeFiles: [routes.yaml]
scenario:
  - send:
      to: direct:echo
      body: ping
      expectReply:
        contains: ack
"#,
    );
    let outcome = run_direct(&doc, dir.path()).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "matching expectReply must pass, got {outcome:?}"
    );
}

/// A send whose `expectReply` does not match the reply body fails
/// verdict-class (`ValidationMismatch`) with a Display that names the
/// rendered expectation and the actual body.
#[tokio::test]
async fn expect_reply_mismatch_is_verdict_failure() {
    let (dir, doc) = project(
        r#"
routes:
  - id: echo-route
    from: direct:echo
    steps:
      - set_body: "ack-7f3a"
"#,
        r#"
routeFiles: [routes.yaml]
scenario:
  - send:
      to: direct:echo
      body: ping
      expectReply:
        equals:
          wrong: true
"#,
    );
    let outcome = run_direct(&doc, dir.path()).await;
    assert_eq!(outcome.verdict, None, "mismatched expectReply must fail");
    assert_eq!(outcome.per_action.len(), 1, "the send is the only action");
    let failure = outcome
        .per_action
        .first()
        .and_then(|result| result.as_ref().err().cloned())
        .expect("the send action must carry the failure");
    assert!(
        matches!(failure, ScenarioFailure::ValidationMismatch { .. }),
        "mismatch must be verdict-class ValidationMismatch, got {failure}"
    );
    let rendered = failure.to_string();
    assert!(
        rendered.contains("validation-mismatch"),
        "failure must name the verdict class: {rendered}"
    );
    assert!(
        rendered.contains("{\"wrong\":true}"),
        "failure must name the rendered expectation: {rendered}"
    );
    assert!(
        rendered.contains("ack-7f3a"),
        "failure must name the actual reply body: {rendered}"
    );
}

/// A structured reply body meets the `jsonSubset` verb — the verb set
/// is shared with `validate`, not a parallel grammar.
#[tokio::test]
async fn expect_reply_json_subset_on_direct_body() {
    let (dir, doc) = project(
        r#"
routes:
  - id: echo-json-route
    from: direct:echo
    steps:
      - set_body:
          status: ok
          seq: 7
"#,
        r#"
routeFiles: [routes.yaml]
scenario:
  - send:
      to: direct:echo
      body: ping
      expectReply:
        jsonSubset:
          status: ok
"#,
    );
    let outcome = run_direct(&doc, dir.path()).await;
    assert_eq!(
        outcome.verdict,
        Some(ScenarioVerdict::Pass),
        "jsonSubset must match the JSON reply body, got {outcome:?}"
    );
}
