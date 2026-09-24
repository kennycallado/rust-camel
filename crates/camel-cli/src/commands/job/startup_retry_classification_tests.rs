//! Characterization tests for the `camel job` send-phase retry
//! classification (`is_retryable_startup_failure`, rc-fr20u).
//!
//! Pins the retry/no-retry outcome for every error kind reachable as
//! the inner pipeline `CamelError` of `attempt_send`, so the
//! structural replacement of the `"not registered"` text sniff cannot
//! regress any reachable outcome.
//!
//! Classification is by TYPED PROVENANCE (rc-3px7o): only a GENUINE SEDA
//! gate rejection — an `EndpointCreationFailedWithSource` whose source
//! chain carries the seda crate's private marker — is NON-retryable
//! (rc-ucemm: the rejection fires pre-enqueue but inside the caller's
//! pipeline, so a retry re-executes already-run route steps and
//! duplicates their side effects). Display text never classifies: a
//! plain-literal `EndpointCreationFailed` that byte-matches a canonical
//! gate wording is a FOREIGN imitation and stays retryable, as does any
//! typed `EndpointCreationFailedWithSource` lacking the marker. Every
//! non-gate `EndpointCreationFailed` stays retryable — the direct
//! registration race and the documented queue-full residual.
//!
//! The genuine-gate rows are BEHAVIORAL: they boot a real
//! [`camel_component_seda::SedaComponent`] endpoint with no consumer and
//! capture the producer's rejection, so non-retryability is pinned
//! against an error that actually carries the marker. The former
//! plain-literal `gate_single()`/`gate_fanout()` helpers that fabricated
//! gates from the wording were removed: those literals now describe
//! retryable foreign imitations (rc-3px7o).
//!
//! Reachability (production sources of a space-separated
//! "not registered" wording in a pipeline error at send time):
//! - `camel-direct`'s startup race — and it arrives AS
//!   `EndpointCreationFailed` (camel-direct owns the wording).
//! - The function runtime's not-registered failure renders as
//!   `function:not_registered:` (underscore, function_step's
//!   `map_invocation_error`) — never matched by the old sniff.
//! - Route-compile "not registered" errors (`ComponentNotFound` from
//!   step compilers) fire at startup, before the job send loop.
//!
//! This file additionally characterizes the OUTER transport arm of the
//! send loop (bd rc-zovuy): the [`super::TransportFailure`] classifier
//! `is_deterministic_transport_failure`, the byte-identical historical
//! `Display` of every transport class, and the behavioral fail-fast /
//! keep-retrying split of `send_with_startup_retry`'s transport arm.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use camel_api::{BoxProcessor, BoxProcessorExt, CamelError};
use camel_component_api::{
    Component, ComponentContext, Consumer, ConsumerContext, Endpoint, ExchangeEnvelope,
    NoOpComponentContext, NoopRuntimeObservability, ProducerContext, RuntimeObservability,
};
use camel_component_seda::{
    SedaComponent, is_no_active_consumers_gate, is_seda_terminal_config_error,
};
use camel_core::CamelContext;
use tokio_util::sync::CancellationToken;

use super::document::{JobBody, JobSendAction};
use super::{
    SendError, TransportFailure, is_deterministic_transport_failure, is_retryable_startup_failure,
    send_with_startup_retry,
};

fn direct_startup_race() -> CamelError {
    CamelError::EndpointCreationFailed("direct endpoint 'out' not registered".into())
}

// ---- retryable: foreign imitations of the gate wording (rc-3px7o) --------

#[test]
fn foreign_byte_exact_single_imitation_stays_retryable() {
    // A plain-literal variant carrying the byte-exact single-mode gate
    // wording is a FOREIGN imitation: classification is typed, so it is
    // not a gate and stays retryable.
    let e = CamelError::EndpointCreationFailed("SEDA endpoint 'q' has no active consumers".into());
    assert!(
        is_retryable_startup_failure(&e),
        "a plain-literal byte-exact single-gate imitation must stay retryable"
    );
}

#[test]
fn foreign_byte_exact_fanout_imitation_stays_retryable() {
    let e =
        CamelError::EndpointCreationFailed("SEDA endpoint 'q' has no active subscribers".into());
    assert!(
        is_retryable_startup_failure(&e),
        "a plain-literal byte-exact fanout-gate imitation must stay retryable"
    );
}

#[test]
fn foreign_wording_collision_stays_retryable() {
    // A foreign component's own "has no active consumers" wording must
    // never be mistaken for the SEDA gate.
    let e = CamelError::EndpointCreationFailed(
        "kafka topic 'orders' has no active consumers (broker=1)".into(),
    );
    assert!(
        is_retryable_startup_failure(&e),
        "a foreign wording collision must stay retryable"
    );
}

/// Minimal foreign error used as the source of a typed
/// `EndpointCreationFailedWithSource` that carries no gate marker.
#[derive(Debug)]
struct ForeignSource;

impl std::fmt::Display for ForeignSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("foreign source")
    }
}

impl std::error::Error for ForeignSource {}

#[test]
fn typed_non_gate_source_stays_retryable() {
    // A typed source-carrying failure WITHOUT the crate-private gate
    // marker is a plain creation race, not a gate: retryable.
    let e = CamelError::EndpointCreationFailedWithSource(
        "foreign".into(),
        camel_api::OpaqueErrorSource::new(Arc::new(ForeignSource)),
    );
    assert!(
        is_retryable_startup_failure(&e),
        "a typed source without the gate marker must stay retryable"
    );
}

// ---- non-retryable: genuine SEDA gates, captured behaviorally ------------

/// The job send action targeting a `seda:` endpoint with no consumer.
fn tick_send(to: &str) -> JobSendAction {
    JobSendAction {
        to: to.to_string(),
        body: Some(JobBody::Text("tick".to_string())),
        headers: None,
    }
}

/// Boot a started context with a real [`SedaComponent`]. The caller then
/// sends to a `seda:` endpoint with NO consumer, so the producer fires
/// the genuine marker-carrying gate rejection under test.
async fn booted_seda_context() -> CamelContext {
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context build");
    ctx.register_component(SedaComponent::new());
    ctx.start().await.expect("context start");
    ctx
}

#[tokio::test]
async fn behavioral_single_gate_fails_fast_classification() {
    let ctx = booted_seda_context().await;
    let send = tick_send("seda:worker");
    let e = match send_with_startup_retry(&ctx, &send, "seda:worker", &[]).await {
        Err(SendError::Pipeline(e)) => e,
        Err(SendError::Transport(detail)) => {
            panic!("expected the SEDA gate as a pipeline failure, got transport: {detail}")
        }
        Ok(_) => panic!("expected the SEDA gate as a pipeline failure, got Ok"),
    };
    assert!(
        is_no_active_consumers_gate(&e),
        "the captured error must be the genuine SEDA single-mode gate: {e}"
    );
    assert!(
        !is_retryable_startup_failure(&e),
        "a genuine SEDA single-mode gate must fail fast (non-retryable): {e}"
    );
}

#[tokio::test]
async fn behavioral_fanout_gate_fails_fast_classification() {
    let ctx = booted_seda_context().await;
    let send = tick_send("seda:worker?multipleConsumers=true");
    let e = match send_with_startup_retry(&ctx, &send, "seda:worker?multipleConsumers=true", &[])
        .await
    {
        Err(SendError::Pipeline(e)) => e,
        Err(SendError::Transport(detail)) => {
            panic!("expected the SEDA gate as a pipeline failure, got transport: {detail}")
        }
        Ok(_) => panic!("expected the SEDA gate as a pipeline failure, got Ok"),
    };
    assert!(
        is_no_active_consumers_gate(&e),
        "the captured error must be the genuine SEDA fanout gate: {e}"
    );
    assert!(
        !is_retryable_startup_failure(&e),
        "a genuine SEDA fanout gate must fail fast (non-retryable): {e}"
    );
}

// ---- non-retryable: genuine SEDA terminal-config conflict (behavioral) ---

#[tokio::test]
async fn behavioral_multiple_consumers_wait_config_fails_fast() {
    let ctx = booted_seda_context().await;

    // Start ONE consumer on the fanout endpoint: with an active
    // subscriber the pre-enqueue gate passes and the producer reaches
    // the deterministic multipleConsumers+wait configuration conflict.
    let component = ctx.registry().get("seda").expect("seda component");
    let endpoint = component
        .create_endpoint("seda:q?multipleConsumers=true", &ctx)
        .expect("fanout endpoint creation");
    let mut consumer = endpoint
        .create_consumer(Arc::new(NoopRuntimeObservability))
        .expect("consumer creation");
    let (tx, _rx) = tokio::sync::mpsc::channel::<ExchangeEnvelope>(16);
    let consumer_ctx = ConsumerContext::new(tx, CancellationToken::new(), "q".to_string());
    consumer.start(consumer_ctx).await.expect("consumer start");

    // The send URI mirrors the job loop's forced Always (the
    // document::seda_send_uri output shape).
    let send = tick_send("seda:q?multipleConsumers=true");
    let started = Instant::now();
    let e = match send_with_startup_retry(
        &ctx,
        &send,
        "seda:q?multipleConsumers=true&waitForTaskToComplete=Always",
        &[],
    )
    .await
    {
        Err(SendError::Pipeline(e)) => e,
        Err(SendError::Transport(detail)) => {
            panic!(
                "expected the terminal-config rejection as a pipeline failure, got transport: {detail}"
            )
        }
        Ok(_) => panic!("expected the terminal-config rejection as a pipeline failure, got Ok"),
    };
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the terminal-config rejection must return on the first attempt without \
         burning the 3 s retry window, took {:?}",
        started.elapsed()
    );
    assert!(
        is_seda_terminal_config_error(&e),
        "the captured error must carry the seda terminal-config marker: {e}"
    );
    assert!(
        !is_retryable_startup_failure(&e),
        "a deterministic configuration conflict must fail fast (non-retryable): {e}"
    );

    consumer.stop().await.expect("consumer stop");
}

// ---- retryable: the consumer-startup race family -------------------------

#[test]
fn seda_queue_full_is_retryable() {
    // Documented residual (bd rc-ucemm scope): queue-full shares the
    // EndpointCreationFailed variant and is NOT one of the excluded gate
    // wordings, so it stays retryable.
    let e = CamelError::EndpointCreationFailed("SEDA queue 'jobs' is full (size=10)".into());
    assert!(
        is_retryable_startup_failure(&e),
        "SEDA queue-full is not a gate wording and must stay retryable"
    );
}

#[test]
fn direct_not_registered_race_is_retryable() {
    // The bd rc-fr20u case: direct's "not registered" wording arrives
    // as EndpointCreationFailed, so the typed match IS the structural
    // classification of this race.
    assert!(
        is_retryable_startup_failure(&direct_startup_race()),
        "direct endpoint-not-registered startup race must stay retryable"
    );
}

#[test]
fn generic_endpoint_creation_failure_is_retryable() {
    // Pre-existing breadth of the typed check: ANY endpoint-creation
    // failure is retryable, not just gate/race wordings.
    let e = CamelError::EndpointCreationFailed("unsupported option `nope`".into());
    assert!(is_retryable_startup_failure(&e));
}

// ---- non-retryable: every other reachable pipeline failure ---------------

#[test]
fn function_not_registered_stays_non_retryable() {
    // function_step's map_invocation_error renders NotRegistered as
    // "function:not_registered: {id}" (underscore) — the old text sniff
    // never matched it, and the structural classification keeps it
    // non-retryable: a missing function is configuration drift, not a
    // consumer-startup race.
    let e = CamelError::ProcessorError("function:not_registered: 9f2c1a".into());
    assert!(!is_retryable_startup_failure(&e));
}

#[test]
fn generic_pipeline_failure_stays_non_retryable() {
    let e = CamelError::ProcessorError("step 2 transform failed".into());
    assert!(!is_retryable_startup_failure(&e));
}

#[test]
fn io_failure_stays_non_retryable() {
    let e = CamelError::Io("connection reset by peer".into());
    assert!(!is_retryable_startup_failure(&e));
}

#[test]
fn component_not_found_stays_non_retryable() {
    // Display is "Component not found: kafka" — the old sniff never
    // matched this text either. Registry misses in attempt_send take
    // the transport class, never the pipeline-error classification
    // under test. In the transport arm the miss is itself a
    // deterministic failure (bd rc-zovuy): it returns on the FIRST
    // attempt instead of spinning the window, since the registry is
    // frozen after boot and a retry can never register the scheme.
    let e = CamelError::ComponentNotFound("kafka".into());
    assert!(!is_retryable_startup_failure(&e));
}

#[test]
fn wrapped_not_registered_text_is_not_sniffed() {
    // POST-fix contract (rc-fr20u): no CamelError variant is classified
    // by its Display text. A ProcessorError merely CARRYING the wording
    // is not retryable — no production producer emits one (direct uses
    // EndpointCreationFailed; the function runtime uses the underscore
    // form), so narrowing the unreachable input changes no reachable
    // outcome while killing the wording-coupling for good.
    let e = CamelError::ProcessorError("endpoint 'x' not registered".into());
    assert!(
        !is_retryable_startup_failure(&e),
        "classification must be structural (variant), not text-sniffed"
    );
}

// ---- transport classifier: is_deterministic_transport_failure (rc-zovuy) ----

#[test]
fn transport_component_not_registered_is_deterministic() {
    // The registry is frozen after boot, so a scheme miss can never
    // heal: the constructed variant must classify deterministic.
    let f = TransportFailure::ComponentNotRegistered {
        uri: "kafka:orders".to_string(),
        scheme: "kafka".to_string(),
    };
    assert!(
        is_deterministic_transport_failure(&f),
        "a registry miss must fail fast on the first attempt"
    );
}

#[test]
fn transport_invalid_uri_is_deterministic() {
    // A malformed URI is deterministic: no retry ever re-parses it
    // into a valid one.
    let f = TransportFailure::EndpointCreation {
        uri: "seda:q?size=abc".to_string(),
        cause: CamelError::InvalidUri("invalid size: invalid digit found in string".into()),
    };
    assert!(
        is_deterministic_transport_failure(&f),
        "an InvalidUri endpoint-creation cause must fail fast"
    );
}

#[test]
fn transport_seda_config_conflict_is_deterministic() {
    // The marker-carrying rejection, built the genuine way: two
    // conflicting `create_endpoint` calls on a SedaComponent. The
    // conflict fires at the component's state lookup — consumers are
    // irrelevant.
    let component = SedaComponent::new();
    let _existing = component
        .create_endpoint("seda:conflict?size=10", &NoOpComponentContext)
        .expect("first endpoint creation");
    let cause = match component.create_endpoint("seda:conflict?size=5", &NoOpComponentContext) {
        Err(e) => e,
        Ok(_) => panic!("incompatible same-name config must be rejected"),
    };
    assert!(
        is_seda_terminal_config_error(&cause),
        "the fixture must carry the seda terminal-config marker: {cause}"
    );
    let f = TransportFailure::EndpointCreation {
        uri: "seda:conflict?size=5".to_string(),
        cause,
    };
    assert!(
        is_deterministic_transport_failure(&f),
        "the seda endpoint config conflict must fail fast in the transport arm"
    );
}

#[test]
fn transport_plain_creation_failure_stays_retryable() {
    // A plain apparatus failure (the registration-race family) keeps
    // the bounded sleep-and-retry window in the transport arm.
    let f = TransportFailure::EndpointCreation {
        uri: "kafka:orders".to_string(),
        cause: CamelError::EndpointCreationFailed("broker unreachable".into()),
    };
    assert!(
        !is_deterministic_transport_failure(&f),
        "a plain creation failure must stay retryable in the transport arm"
    );
}

// ---- transport Display: byte-identical historical strings ----------------

#[test]
fn transport_display_component_not_registered_byte_identical() {
    let f = TransportFailure::ComponentNotRegistered {
        uri: "kafka:orders".to_string(),
        scheme: "kafka".to_string(),
    };
    assert_eq!(
        f.to_string(),
        "failed to send to kafka:orders: `kafka:` component not registered"
    );
}

#[test]
fn transport_display_endpoint_creation_byte_identical() {
    let f = TransportFailure::EndpointCreation {
        uri: "seda:q?size=abc".to_string(),
        cause: CamelError::InvalidUri("invalid size: invalid digit found in string".into()),
    };
    assert_eq!(
        f.to_string(),
        "failed to create endpoint seda:q?size=abc: \
         Invalid URI: invalid size: invalid digit found in string"
    );
}

#[test]
fn transport_display_producer_creation_byte_identical() {
    let f = TransportFailure::ProducerCreation {
        uri: "seda:worker".to_string(),
        cause: CamelError::EndpointCreationFailed("boom".into()),
    };
    assert_eq!(
        f.to_string(),
        "failed to create producer for seda:worker: Endpoint creation failed: boom"
    );
}

// ---- behavioral: the transport arm's fail-fast / keep-retrying split ------

/// Test-only component under an UNUSED scheme whose FIRST
/// `create_endpoint` call fails with a plain (retryable)
/// `EndpointCreationFailed` and every later call succeeds. The attempt
/// counter is shared with the test through a cloned handle.
#[derive(Debug, Clone)]
struct FlakyOnceComponent {
    attempts: Arc<AtomicUsize>,
}

impl FlakyOnceComponent {
    fn new() -> Self {
        Self {
            attempts: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// The trivial endpoint the flaky component hands out after its first
/// (failing) creation attempt: the producer echoes the exchange back,
/// so the retried send completes with a reply.
struct FlakyEndpoint {
    uri: String,
}

impl Endpoint for FlakyEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "consumers are unused by the transport retry fixture".into(),
        ))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::from_fn(
            |exchange| async move { Ok(exchange) },
        ))
    }
}

#[async_trait]
impl Component for FlakyOnceComponent {
    fn scheme(&self) -> &str {
        "flaky-once"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            return Err(CamelError::EndpointCreationFailed("flaky once".into()));
        }
        Ok(Box::new(FlakyEndpoint {
            uri: uri.to_string(),
        }))
    }
}

#[tokio::test]
async fn seda_config_conflict_transport_fails_fast() {
    let ctx = booted_seda_context().await;

    // Lazily pre-create the conflicting endpoint state on the REAL
    // registered component: the conflict fires at the component's state
    // lookup inside `create_endpoint`, before any producer or gate —
    // consumers are irrelevant.
    let component = ctx.registry().get("seda").expect("seda component");
    let _existing = component
        .create_endpoint("seda:q?size=10", &ctx)
        .expect("conflicting endpoint pre-creation");

    // The send URI mirrors the job loop's forced Always (the
    // document::seda_send_uri output shape) on the same queue name.
    let send = tick_send("seda:q");
    let started = Instant::now();
    let detail = match send_with_startup_retry(
        &ctx,
        &send,
        "seda:q?size=5&waitForTaskToComplete=Always",
        &[],
    )
    .await
    {
        Err(SendError::Transport(detail)) => detail,
        Err(SendError::Pipeline(e)) => {
            panic!("expected the config conflict as a transport failure, got pipeline: {e}")
        }
        Ok(_) => panic!("expected the config conflict as a transport failure, got Ok"),
    };
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the endpoint config conflict must return on the first attempt without \
         burning the 3 s retry window, took {:?}",
        started.elapsed()
    );
    let rendered = detail.to_string();
    assert!(
        rendered.contains("failed to create endpoint seda:q?size=5&waitForTaskToComplete=Always"),
        "the conflict must render through the endpoint-creation stage prefix: {rendered}"
    );
    assert!(
        rendered.contains("already exists with different config: size: 10 vs 5"),
        "the conflict detail must survive into the transport rendering: {rendered}"
    );
}

#[tokio::test]
async fn invalid_uri_transport_fails_fast() {
    let ctx = booted_seda_context().await;
    let send = tick_send("seda:q");

    let started = Instant::now();
    let detail = match send_with_startup_retry(&ctx, &send, "seda:q?size=abc", &[]).await {
        Err(SendError::Transport(detail)) => detail,
        Err(SendError::Pipeline(e)) => {
            panic!("expected the InvalidUri rejection as a transport failure, got pipeline: {e}")
        }
        Ok(_) => panic!("expected the InvalidUri rejection as a transport failure, got Ok"),
    };
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "an InvalidUri transport failure must return on the first attempt without \
         burning the 3 s retry window, took {:?}",
        started.elapsed()
    );
    let rendered = detail.to_string();
    assert!(
        rendered.contains("failed to create endpoint"),
        "the InvalidUri cause must render through the endpoint-creation stage \
         prefix, NOT an `Endpoint creation failed:` prefix: {rendered}"
    );
    assert!(
        rendered.contains("Invalid URI: invalid size"),
        "the InvalidUri cause detail must survive into the transport rendering: {rendered}"
    );
    assert!(
        !rendered.contains("Endpoint creation failed:"),
        "an InvalidUri cause must not wear the EndpointCreationFailed wording: {rendered}"
    );
}

#[tokio::test]
async fn transient_transport_failure_keeps_retry_loop() {
    // RETRY-POSITIVE complement of the transport arm: a plain
    // `EndpointCreationFailed` cause is NOT deterministic, so the loop
    // must sleep (`SEND_RETRY_SLEEP` = 20 ms) and retry instead of
    // failing fast. The second attempt succeeds well inside the 3 s
    // window, so the send completes.
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context build");
    let flaky = FlakyOnceComponent::new();
    ctx.register_component(flaky.clone());
    ctx.start().await.expect("context start");

    let send = tick_send("flaky-once:thing");
    match send_with_startup_retry(&ctx, &send, "flaky-once:thing", &[]).await {
        Ok(_) => {}
        Err(SendError::Pipeline(e)) => {
            panic!(
                "the plain transport failure must be retried to success, got pipeline error: {e}"
            )
        }
        Err(SendError::Transport(detail)) => {
            panic!(
                "the plain transport failure must be retried to success, got transport: {detail}"
            )
        }
    }
    assert_eq!(
        flaky.attempts.load(Ordering::SeqCst),
        2,
        "exactly one retry must follow the flaky first attempt"
    );
}
