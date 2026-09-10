//! Circuit-breaker route-level fallback end to end (bd rc-q16d, the
//! EFFIS anchor): a `circuit_breaker` whose `fallback` pipeline
//! serves a stale cache entry (`cache_peek_stale`) once the wrapped
//! upstream has failed the breaker open. Drives the YAML route-level
//! surface — no programmatic `.fallback()` — through the scenario
//! tier's own boot, with a harness partner scripted `fault: close` as
//! the only upstream.
//!
//! Two documents run against ONE boot (the library tier's privilege;
//! the CLI boots per document): the opener document's send seeds the
//! cache entry (`on_miss`) and its dial failure both opens the
//! circuit and surfaces — the shipped gate counts the failure, and
//! the tripping call itself always observes the error, never the
//! fallback. The consumer document then proves the two fallback
//! contracts:
//! - happy: after a sleep crossing the entry's TTL, the send
//!   short-circuits into the fallback and the served body is the
//!   post-expiry entry — the stale value, never an error. If the
//!   repository dropped expired entries, the peek would MISS and the
//!   send would observe the Stopped shape instead.
//! - negative: peeking a key nothing ever wrote MISSes; the `Stop`
//!   miss policy Stops the branch — a clean outcome per the Stopped
//!   contract, never an `Err` propagation.

#![cfg(feature = "http")]

use std::collections::BTreeMap;
use std::sync::Arc;

use camel_integration_test::{
    DirectStimulus, DocumentOutcome, HttpPartner, LayeredEnv, PartnerAdapter, PartnerRouter,
    ScenarioDocument, ScenarioFailure, ScenarioVerdict, ambient_std, boot_scenario,
    parse_scenario_document, partner_scripts_for, run_scenario_document,
};
use tokio::sync::Mutex;

/// The router key the harness partner binds under; the wire target is
/// the bound address the env-tier `UPSTREAM` variable carries.
const UPSTREAM: &str = "http://127.0.0.1:0/upstream";

/// The seeded entry's TTL: short enough that the consumer document's
/// `sleep` crosses it with a wide margin, long enough that the
/// seeding exchange itself cannot outlive it.
const TTL: &str = "100ms";
/// The consumer document's sleep before its fetch send: comfortably
/// past the TTL, so the peeked entry is provably post-expiry.
const SLEEP: &str = "400ms";

/// The routes under test. `cb-stale` wraps a cache-seed step plus the
/// upstream dial: a closed-circuit run seeds `tile-xyz` through
/// `on_miss`, then fails the dial; once open, the fallback peeks the
/// (aged) entry instead. `cb-miss` is the same breaker shape with no
/// seeding step — its peeked key is never written, so the fallback
/// MISSes.
fn routes_yaml() -> String {
    r#"
routes:
  - id: cb-stale
    from: direct:fetch
    circuit_breaker:
      failure_threshold: 1
      open_duration_ms: 60000
      fallback:
        - cache_peek_stale:
            repository: memory
            key: tile-xyz
    steps:
      - cache:
          repository: memory
          key: tile-xyz
          ttl: "TTL"
          on_miss:
            - set_body: "tile-stale-7f"
      - to: ${env:UPSTREAM}
  - id: cb-miss
    from: direct:miss
    circuit_breaker:
      failure_threshold: 1
      open_duration_ms: 60000
      fallback:
        - cache_peek_stale:
            repository: memory
            key: never-seeded
    steps:
      - to: ${env:UPSTREAM}
"#
    .replace("TTL", TTL)
}

/// Boots the project, binds the faulting partner under
/// [`UPSTREAM`], wires the env-tier `UPSTREAM` to the bound address,
/// runs the opener and consumer documents against the ONE boot, and
/// tears down. The partner script (`fault: close`) applies to every
/// arrival: the upstream dial can never succeed. Circuit-breaker
/// state and the cache repository live in the boot, so both persist
/// across the two documents.
async fn run_two_docs(
    opener_yaml: &str,
    consumer_yaml: &str,
) -> (DocumentOutcome, DocumentOutcome) {
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
    std::fs::write(root.join("routes.yaml"), routes_yaml()).expect("write routes.yaml");
    let docs: Vec<ScenarioDocument> = [opener_yaml, consumer_yaml]
        .iter()
        .enumerate()
        .map(|(n, yaml)| {
            let path = root.join(format!("case-{n}.test.yaml"));
            std::fs::write(&path, yaml).expect("write case file");
            parse_scenario_document(&path).expect("document must load")
        })
        .collect();

    let scripts = partner_scripts_for(&docs[0], UPSTREAM);
    let partner = match scripts {
        Some(scripts) => HttpPartner::start(scripts).await,
        None => HttpPartner::start_permissive(200).await,
    }
    .expect("partner must bind 127.0.0.1:0");
    let bound = partner.bound_addr().to_string();
    let harness_provisioned =
        BTreeMap::from([("UPSTREAM".to_string(), format!("http://{bound}/upstream"))]);
    let env = LayeredEnv::new(
        docs[0].env.clone().unwrap_or_default(),
        harness_provisioned,
        docs[0].env_passthrough.clone().unwrap_or_default(),
        ambient_std(),
    );
    let run = boot_scenario(&docs[0], root, &env)
        .await
        .expect("the full boot must succeed");
    let ctx = Arc::new(Mutex::new(run.ctx));

    // The scenario wires no harness references (its sends are
    // `direct:` plain strings); the partner's bound address rides the
    // env tier through `UPSTREAM`, so the bind-var walk is empty.
    let mut adapters: BTreeMap<String, Box<dyn PartnerAdapter>> = BTreeMap::new();
    adapters.insert(
        "direct:fetch".to_string(),
        Box::new(DirectStimulus::new(Arc::clone(&ctx))),
    );
    adapters.insert(
        "direct:miss".to_string(),
        Box::new(DirectStimulus::new(Arc::clone(&ctx))),
    );
    adapters.insert(UPSTREAM.to_string(), Box::new(partner));
    let router = PartnerRouter::new(adapters);

    let mut outcomes = Vec::with_capacity(docs.len());
    for doc in &docs {
        let mut vars = camel_integration_test::ScenarioVars::new();
        let outcome = run_scenario_document(doc, &router, &mut vars, None).await;
        outcomes.push(outcome);
    }
    let [first, second, ..] = &outcomes[..] else {
        panic!("exactly two documents must run");
    };
    let (first, second) = (first.clone(), second.clone());
    if let Err(e) = run.boot.shutdown(&mut *ctx.lock().await).await {
        tracing::error!(error = %e, "shutdown after circuit docs failed");
    }
    (first, second)
}

/// The happy path (EFFIS anchor): the opener's single send seeds
/// `tile-xyz` (`on_miss`) and its dial failure opens the circuit —
/// the tripping send itself observes the transport failure, as the
/// shipped gate promises. The consumer's sleep crosses the TTL; its
/// send short-circuits into the fallback, and the served body is the
/// past-expiry entry — the stale value, never an error. The partner
/// count pins that only the opener's dial ever reached the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn circuit_fallback_serves_stale_on_upstream_failure() {
    let (opener, consumer) = run_two_docs(
        // Opener: one send on `cb-stale` — seeds, dials, fails, opens.
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:fetch
    body: open
- validate:
    target:
      partner:
        endpoint: http://127.0.0.1:0/upstream
        provisioning: harness
    expectation:
      count: 1
partners:
  http://127.0.0.1:0/upstream:
  - fault: close
"#,
        // Consumer: age past the TTL, then take the fallback.
        r#"
routeFiles: [routes.yaml]
scenario:
- sleep:
    duration: SLEEP
- send:
    to: direct:fetch
    body: fetch
    expectReply:
      contains: tile-stale-7f
- validate:
    target:
      partner:
        endpoint: http://127.0.0.1:0/upstream
        provisioning: harness
    expectation:
      count: 1
partners:
  http://127.0.0.1:0/upstream:
  - fault: close
"#
        .replace("SLEEP", SLEEP)
        .as_str(),
    )
    .await;

    // The opener: the tripping send fails transport-class — the
    // breaker counts it and the call itself observes the failure.
    assert_eq!(
        opener.verdict, None,
        "the opening dial must fail its send: {opener:?}"
    );
    assert!(
        matches!(
            opener.per_action.first(),
            Some(Err(ScenarioFailure::ActionTransport { .. }))
        ),
        "the opening failure must be ActionTransport: {:?}",
        opener.per_action
    );

    // The consumer: the open circuit serves the stale entry.
    assert_eq!(
        consumer.verdict,
        Some(ScenarioVerdict::Pass),
        "the stale fallback must serve the aged value: {consumer:?}"
    );
    assert!(
        consumer.per_action.iter().all(|result| result.is_ok()),
        "no consumer action may fail: {:?}",
        consumer.per_action
    );
}

/// The negative path: `cb-miss` never seeds its peeked key, so after
/// the breaker opens, the fallback's `cache_peek_stale` MISSes. The
/// `Stop` miss policy (the default) Stops the branch — a CLEAN
/// outcome per the Stopped contract: the consumer's send completes
/// without a transport failure, no `Err` propagates, and the consumer
/// document passes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn circuit_fallback_miss_stops_cleanly() {
    let (opener, consumer) = run_two_docs(
        // Opener: one send on `cb-miss` — dials, fails, opens.
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:miss
    body: open
- validate:
    target:
      partner:
        endpoint: http://127.0.0.1:0/upstream
        provisioning: harness
    expectation:
      count: 1
partners:
  http://127.0.0.1:0/upstream:
  - fault: close
"#,
        // Consumer: the open breaker short-circuits into the
        // fallback, which peeks a never-written key.
        r#"
routeFiles: [routes.yaml]
scenario:
- send:
    to: direct:miss
    body: fallback
- validate:
    target:
      partner:
        endpoint: http://127.0.0.1:0/upstream
        provisioning: harness
    expectation:
      count: 1
partners:
  http://127.0.0.1:0/upstream:
  - fault: close
"#,
    )
    .await;

    // The opener: transport-class failure, circuit now open.
    assert_eq!(
        opener.verdict, None,
        "the opening dial must fail its send: {opener:?}"
    );
    assert!(
        matches!(
            opener.per_action.first(),
            Some(Err(ScenarioFailure::ActionTransport { .. }))
        ),
        "the opening failure must be ActionTransport: {:?}",
        opener.per_action
    );

    // The consumer: the MISS Stops the fallback branch cleanly.
    assert_eq!(
        consumer.verdict,
        Some(ScenarioVerdict::Pass),
        "a fallback MISS is a clean Stopped outcome, not a failure: {consumer:?}"
    );
    assert!(
        consumer.per_action.iter().all(|result| result.is_ok()),
        "the MISS must never surface as an action Err: {:?}",
        consumer.per_action
    );
}
