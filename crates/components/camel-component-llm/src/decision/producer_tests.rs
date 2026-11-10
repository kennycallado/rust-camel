//! Tests for the decision producer. Each test drives the producer through
//! `tower::Service::call`, so errors surface as `CamelError`.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use camel_api::{Body, CamelError, Exchange, Message};
use camel_component_api::NetworkRetryPolicy;
use serde_json::{Value, json};
use tower::Service;

use super::DecisionProducer;
use crate::decision::config::MockDecisionConfig;
use crate::decision::config::QuestionSetConfig;
use crate::decision::mock::MockDecisionProvider;
use crate::decision::test_support::triage_set;
use crate::decision::{
    ChoiceProbability, DecisionAnswer, DecisionCapabilities, DecisionProvider, DecisionRequest,
    DecisionResponse, DecisionState, DecisionUsage, NamedAnswer, ScoreProbability,
};
use crate::error::LlmError;
use crate::headers::{
    CAMEL_LLM_CACHE_WRITE_TOKENS_IN, CAMEL_LLM_CACHED_TOKENS_IN, CAMEL_LLM_ESTIMATED_COST_USD,
    CAMEL_LLM_MODEL, CAMEL_LLM_PROVIDER, CAMEL_LLM_TOKENS_IN, CAMEL_LLM_TOKENS_OUT,
    CAMEL_LLM_USAGE_AVAILABLE,
};

/// Provider that records the last request and returns a fixed response.
struct RecordingProvider {
    usage: DecisionUsage,
    last: Mutex<Option<DecisionRequest>>,
}

impl RecordingProvider {
    fn new(usage: DecisionUsage) -> Arc<Self> {
        Arc::new(Self {
            usage,
            last: Mutex::new(None),
        })
    }

    fn recorded_state(&self) -> DecisionState {
        self.last
            .lock()
            .expect("recording lock poisoned")
            .as_ref()
            .expect("decide was never called")
            .state
            .clone()
    }
}

#[async_trait]
impl DecisionProvider for RecordingProvider {
    fn id(&self) -> &str {
        "rec"
    }

    fn default_model(&self) -> &str {
        "rec-model"
    }

    fn capabilities(&self) -> DecisionCapabilities {
        DecisionCapabilities::new(400)
    }

    async fn decide(&self, req: DecisionRequest) -> Result<DecisionResponse, LlmError> {
        *self.last.lock().expect("recording lock poisoned") = Some(req);
        Ok(DecisionResponse {
            model: "rec-model".into(),
            answers: triage_answers(),
            usage: self.usage,
        })
    }
}

/// Provider that returns a fixed, caller-crafted answer list, used to feed
/// malformed responses to the producer's shape check.
struct CraftedProvider {
    answers: Vec<NamedAnswer>,
}

#[async_trait]
impl DecisionProvider for CraftedProvider {
    fn id(&self) -> &str {
        "crafted"
    }

    fn default_model(&self) -> &str {
        "crafted-model"
    }

    fn capabilities(&self) -> DecisionCapabilities {
        DecisionCapabilities::new(400)
    }

    async fn decide(&self, _req: DecisionRequest) -> Result<DecisionResponse, LlmError> {
        Ok(DecisionResponse {
            model: "crafted-model".into(),
            answers: self.answers.clone(),
            usage: DecisionUsage::default(),
        })
    }
}

/// A well-formed answer list for the triage set, in question order.
fn triage_answers() -> Vec<NamedAnswer> {
    vec![
        NamedAnswer {
            name: "is_billing".into(),
            answer: DecisionAnswer::Predicate { probability: 0.9 },
        },
        NamedAnswer {
            name: "dept".into(),
            answer: DecisionAnswer::Choice {
                choice: "billing".into(),
                confidence: 0.9,
                probabilities: vec![
                    ChoiceProbability {
                        value: "billing".into(),
                        probability: 0.9,
                    },
                    ChoiceProbability {
                        value: "tech".into(),
                        probability: 0.1,
                    },
                ],
            },
        },
        NamedAnswer {
            name: "urgency".into(),
            answer: DecisionAnswer::Score {
                score: 1.0,
                confidence: 0.8,
                probabilities: vec![
                    ScoreProbability {
                        level: 0,
                        probability: 0.1,
                    },
                    ScoreProbability {
                        level: 1,
                        probability: 0.8,
                    },
                    ScoreProbability {
                        level: 2,
                        probability: 0.1,
                    },
                ],
            },
        },
    ]
}

/// Runs the producer against a provider that returns `answers`.
async fn crafted_call(answers: Vec<NamedAnswer>) -> Result<Exchange, CamelError> {
    let provider: Arc<dyn DecisionProvider> = Arc::new(CraftedProvider { answers });
    let mut producer = DecisionProducer::new(
        provider,
        None,
        Arc::new(triage_set()),
        32768,
        "test-route".into(),
    );
    call(&mut producer, Body::Text("refund please".into())).await
}

/// Runs the producer against `answers` and returns the error text.
async fn crafted_error(answers: Vec<NamedAnswer>) -> String {
    crafted_call(answers)
        .await
        .expect_err("a malformed provider response must be rejected")
        .to_string()
}

fn mock_config() -> MockDecisionConfig {
    MockDecisionConfig {
        default_model: "mock-decision".into(),
        predicate_probability: 1.0,
        refuse: vec![],
        error_message: None,
    }
}

fn mock_provider(config: MockDecisionConfig) -> Arc<dyn DecisionProvider> {
    Arc::new(MockDecisionProvider::new("fake", config))
}

async fn call(producer: &mut DecisionProducer, body: Body) -> Result<Exchange, CamelError> {
    producer.call(Exchange::new(Message::new(body))).await
}

fn json_body(exchange: &Exchange) -> &Value {
    match &exchange.input.body {
        Body::Json(v) => v,
        other => panic!("expected JSON body, got {other:?}"),
    }
}

#[tokio::test]
async fn json_number_body_rejected() {
    let mut producer = DecisionProducer::new(
        mock_provider(mock_config()),
        None,
        Arc::new(triage_set()),
        32768,
        "test-route".into(),
    );
    let err = call(&mut producer, Body::Json(json!(42)))
        .await
        .expect_err("a JSON number is not a valid decision state");
    assert!(
        err.to_string().contains("invalid request"),
        "error was {err}"
    );
}

#[tokio::test]
async fn empty_body_rejected() {
    let mut producer = DecisionProducer::new(
        mock_provider(mock_config()),
        None,
        Arc::new(triage_set()),
        32768,
        "test-route".into(),
    );
    let err = call(&mut producer, Body::Empty)
        .await
        .expect_err("an empty body is not a valid decision state");
    assert!(
        err.to_string().contains("invalid request"),
        "error was {err}"
    );
}

#[tokio::test]
async fn json_string_body_is_text_state() {
    let provider = RecordingProvider::new(DecisionUsage::default());
    let mut producer = DecisionProducer::new(
        provider.clone(),
        None,
        Arc::new(triage_set()),
        32768,
        "test-route".into(),
    );
    call(&mut producer, Body::Json(json!("hello")))
        .await
        .expect("a JSON string is valid text state");
    assert_eq!(
        provider.recorded_state(),
        DecisionState::Text("hello".into())
    );
}

#[tokio::test]
async fn json_object_body_is_json_state() {
    let provider = RecordingProvider::new(DecisionUsage::default());
    let mut producer = DecisionProducer::new(
        provider.clone(),
        None,
        Arc::new(triage_set()),
        32768,
        "test-route".into(),
    );
    call(&mut producer, Body::Json(json!({"subject": "refund"})))
        .await
        .expect("a JSON object is valid state");
    assert!(
        matches!(provider.recorded_state(), DecisionState::Json(_)),
        "a JSON object body must map to JSON state"
    );
}

#[tokio::test]
async fn invalid_utf8_bytes_rejected() {
    let mut producer = DecisionProducer::new(
        mock_provider(mock_config()),
        None,
        Arc::new(triage_set()),
        32768,
        "test-route".into(),
    );
    let result = call(&mut producer, Body::Bytes(Bytes::from(vec![0xff, 0xfe]))).await;
    assert!(result.is_err(), "invalid UTF-8 bytes must be rejected");
}

#[tokio::test]
async fn oversize_state_rejected() {
    let mut producer = DecisionProducer::new(
        mock_provider(mock_config()),
        None,
        Arc::new(triage_set()),
        4,
        "test-route".into(),
    );
    let err = call(&mut producer, Body::Text("hello".into()))
        .await
        .expect_err("a 5-byte state exceeds the 4-byte limit");
    assert!(err.to_string().contains("exceeds"), "error was {err}");
}

#[tokio::test]
async fn output_body_shape() {
    let mut producer = DecisionProducer::new(
        mock_provider(mock_config()),
        None,
        Arc::new(triage_set()),
        32768,
        "test-route".into(),
    );
    let exchange = call(&mut producer, Body::Text("refund please".into()))
        .await
        .expect("mock decision should succeed");
    let body = json_body(&exchange);
    let answers = &body["answers"];

    assert_eq!(
        answers["is_billing"],
        json!({"type": "predicate", "probability": 1.0, "meets_threshold": true})
    );
    assert_eq!(
        answers["dept"]["probabilities"],
        json!({"billing": 1.0, "tech": 0.0})
    );
    assert!(
        answers["dept"].get("meets_min_confidence").is_none(),
        "meets_min_confidence must be absent without min_confidence"
    );
    assert_eq!(
        answers["urgency"]["probabilities"][0],
        json!({"level": 0, "label": "low", "probability": 1.0})
    );
}

#[tokio::test]
async fn no_threshold_no_meets_key() {
    let mut set: QuestionSetConfig = triage_set();
    set.questions[0].threshold = None;
    let mut producer = DecisionProducer::new(
        mock_provider(mock_config()),
        None,
        Arc::new(set),
        32768,
        "test-route".into(),
    );
    let exchange = call(&mut producer, Body::Text("refund please".into()))
        .await
        .expect("mock decision should succeed");
    let predicate = &json_body(&exchange)["answers"]["is_billing"];
    assert!(
        predicate.get("meets_threshold").is_none(),
        "meets_threshold must be absent without a configured threshold, got {predicate}"
    );
}

#[tokio::test]
async fn refusal_has_no_meets_keys() {
    let mut config = mock_config();
    config.refuse = vec!["is_billing".into()];
    let mut producer = DecisionProducer::new(
        mock_provider(config),
        None,
        Arc::new(triage_set()),
        32768,
        "test-route".into(),
    );
    let exchange = call(&mut producer, Body::Text("refund please".into()))
        .await
        .expect("mock decision should succeed");
    assert_eq!(
        json_body(&exchange)["answers"]["is_billing"],
        json!({"type": "refusal"})
    );
}

#[tokio::test]
async fn usage_headers_only_when_reported() {
    let usage = DecisionUsage {
        input_tokens: Some(375),
        output_tokens: Some(0),
        cached_input_tokens: None,
        cache_write_input_tokens: None,
    };
    let mut producer = DecisionProducer::new(
        RecordingProvider::new(usage),
        None,
        Arc::new(triage_set()),
        32768,
        "test-route".into(),
    );
    let exchange = call(&mut producer, Body::Text("refund please".into()))
        .await
        .expect("recording decision should succeed");
    let headers = &exchange.input.headers;
    assert_eq!(headers.get(CAMEL_LLM_TOKENS_IN), Some(&json!(375)));
    assert_eq!(headers.get(CAMEL_LLM_TOKENS_OUT), Some(&json!(0)));
    assert!(
        !headers.contains_key(CAMEL_LLM_CACHED_TOKENS_IN),
        "cached tokens must be absent when the provider does not report them"
    );
}

#[tokio::test]
async fn openai_style_cached_usage_headers() {
    let usage = DecisionUsage {
        input_tokens: Some(2000),
        output_tokens: Some(0),
        cached_input_tokens: Some(1500),
        cache_write_input_tokens: Some(0),
    };
    let mut producer = DecisionProducer::new(
        RecordingProvider::new(usage),
        None,
        Arc::new(triage_set()),
        32768,
        "test-route".into(),
    );
    let exchange = call(&mut producer, Body::Text("refund please".into()))
        .await
        .expect("recording decision should succeed");
    let headers = &exchange.input.headers;
    assert_eq!(headers.get(CAMEL_LLM_TOKENS_IN), Some(&json!(2000)));
    assert_eq!(headers.get(CAMEL_LLM_TOKENS_OUT), Some(&json!(0)));
    assert_eq!(headers.get(CAMEL_LLM_CACHED_TOKENS_IN), Some(&json!(1500)));
    assert_eq!(
        headers.get(CAMEL_LLM_CACHE_WRITE_TOKENS_IN),
        Some(&json!(0))
    );
}

#[tokio::test]
async fn no_cost_header() {
    let mut producer = DecisionProducer::new(
        mock_provider(mock_config()),
        None,
        Arc::new(triage_set()),
        32768,
        "test-route".into(),
    );
    let exchange = call(&mut producer, Body::Text("refund please".into()))
        .await
        .expect("mock decision should succeed");
    assert!(
        !exchange
            .input
            .headers
            .contains_key(CAMEL_LLM_ESTIMATED_COST_USD),
        "decide must not set a cost header"
    );
}

#[tokio::test]
async fn provider_and_model_headers() {
    let mut producer = DecisionProducer::new(
        mock_provider(mock_config()),
        None,
        Arc::new(triage_set()),
        32768,
        "test-route".into(),
    );
    let exchange = call(&mut producer, Body::Text("refund please".into()))
        .await
        .expect("mock decision should succeed");
    let headers = &exchange.input.headers;
    assert_eq!(headers.get(CAMEL_LLM_PROVIDER), Some(&json!("fake")));
    assert_eq!(headers.get(CAMEL_LLM_MODEL), Some(&json!("mock-decision")));
    assert_eq!(headers.get(CAMEL_LLM_USAGE_AVAILABLE), Some(&json!(true)));
}

#[tokio::test]
async fn invalid_body_never_reaches_provider() {
    let provider = RecordingProvider::new(DecisionUsage::default());
    let mut producer = DecisionProducer::new(
        provider.clone(),
        None,
        Arc::new(triage_set()),
        4,
        "test-route".into(),
    );
    for body in [
        Body::Bytes(Bytes::from(vec![0xff, 0xfe])),
        Body::Text("hello".into()),
        Body::Json(json!(42)),
        Body::Empty,
    ] {
        call(&mut producer, body)
            .await
            .expect_err("invalid or oversize state must be rejected");
    }
    assert!(
        provider
            .last
            .lock()
            .expect("recording lock poisoned")
            .is_none(),
        "a rejected state must not reach the provider"
    );
}

#[tokio::test]
async fn model_override_is_sent_to_provider() {
    let provider = RecordingProvider::new(DecisionUsage::default());
    let mut producer = DecisionProducer::new(
        provider.clone(),
        Some("custom-model".into()),
        Arc::new(triage_set()),
        32768,
        "test-route".into(),
    );
    call(&mut producer, Body::Text("hi".into()))
        .await
        .expect("decide succeeds");
    let model = provider
        .last
        .lock()
        .expect("recording lock poisoned")
        .as_ref()
        .map(|req| req.model.clone());
    assert_eq!(model.as_deref(), Some("custom-model"));
}

#[tokio::test]
async fn misnamed_answer_is_protocol() {
    let mut answers = triage_answers();
    answers[1].name = "other".into();
    let err = crafted_error(answers).await;
    assert!(
        err.contains("malformed provider response"),
        "error was {err}"
    );
    assert!(err.contains("dept"), "error must name the question: {err}");
}

#[tokio::test]
async fn reordered_answers_are_protocol() {
    let mut answers = triage_answers();
    answers.swap(0, 1);
    let err = crafted_error(answers).await;
    assert!(
        err.contains("malformed provider response"),
        "error was {err}"
    );
    assert!(
        err.contains("is_billing"),
        "error must name the first misplaced question: {err}"
    );
}

#[tokio::test]
async fn missing_answer_is_protocol_at_producer() {
    let mut answers = triage_answers();
    answers.retain(|a| a.name != "urgency");
    let err = crafted_error(answers).await;
    assert!(
        err.contains("malformed provider response"),
        "error was {err}"
    );
    assert!(
        err.contains("urgency"),
        "error must name the missing question: {err}"
    );
}

#[tokio::test]
async fn kind_mismatch_is_protocol_at_producer() {
    let mut answers = triage_answers();
    answers[0].answer = DecisionAnswer::Choice {
        choice: "billing".into(),
        confidence: 0.9,
        probabilities: vec![],
    };
    let err = crafted_error(answers).await;
    assert!(
        err.contains("malformed provider response"),
        "error was {err}"
    );
    assert!(
        err.contains("is_billing"),
        "error must name the mismatched question: {err}"
    );
}

#[tokio::test]
async fn refusal_accepted_at_producer() {
    let mut answers = triage_answers();
    answers[0].answer = DecisionAnswer::Refusal;
    let exchange = crafted_call(answers)
        .await
        .expect("a refusal is valid for any question kind");
    assert_eq!(
        json_body(&exchange)["answers"]["is_billing"],
        json!({"type": "refusal"})
    );
}

#[tokio::test]
async fn undeclared_score_level_is_protocol() {
    let mut answers = triage_answers();
    let DecisionAnswer::Score { probabilities, .. } = &mut answers[2].answer else {
        panic!("urgency fixture should be a score answer");
    };
    probabilities[0].level = 7;
    let err = crafted_error(answers).await;
    assert!(
        err.contains("malformed provider response"),
        "error was {err}"
    );
    assert!(
        err.contains("urgency"),
        "error must name the question with the undeclared level: {err}"
    );
}

/// Provider whose `decide` future never resolves, so only the producer's total
/// deadline can end the call.
struct PendingDecisionProvider;

#[async_trait]
impl DecisionProvider for PendingDecisionProvider {
    fn id(&self) -> &str {
        "pending"
    }

    fn default_model(&self) -> &str {
        "pending-model"
    }

    fn capabilities(&self) -> DecisionCapabilities {
        DecisionCapabilities::new(400)
    }

    async fn decide(&self, _req: DecisionRequest) -> Result<DecisionResponse, LlmError> {
        std::future::pending::<Result<DecisionResponse, LlmError>>().await
    }
}

/// Provider that always rate limits, counting attempts.
struct RateLimitingDecisionProvider {
    retry_after: Option<Duration>,
    calls: AtomicUsize,
}

impl RateLimitingDecisionProvider {
    fn new(retry_after: Option<Duration>) -> Arc<Self> {
        Arc::new(Self {
            retry_after,
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DecisionProvider for RateLimitingDecisionProvider {
    fn id(&self) -> &str {
        "rate-limited"
    }

    fn default_model(&self) -> &str {
        "rate-limited-model"
    }

    fn capabilities(&self) -> DecisionCapabilities {
        DecisionCapabilities::new(400)
    }

    async fn decide(&self, _req: DecisionRequest) -> Result<DecisionResponse, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(LlmError::RateLimit {
            retry_after: self.retry_after,
        })
    }
}

/// Jitter-free, fixed 1s backoff policy used by the deadline-bounded test.
fn fixed_second_policy(max_attempts: u32) -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        enabled: true,
        max_attempts,
        initial_delay: Duration::from_secs(1),
        multiplier: 1.0,
        max_delay: Duration::from_secs(1),
        jitter_factor: 0.0,
        max_attempts_absolute: None,
    }
}

#[tokio::test(start_paused = true)]
async fn decide_default_total_deadline_fires() {
    let mut producer = DecisionProducer::new(
        Arc::new(PendingDecisionProvider),
        None,
        Arc::new(triage_set()),
        1024,
        "test-route".into(),
    )
    .with_timeout(Some(Duration::from_secs(120)));
    let err = call(&mut producer, Body::Text("refund please".into()))
        .await
        .expect_err("a pending provider must hit the total deadline");
    let msg = err.to_string();
    assert!(msg.contains("timeout"), "error was {msg}");
    assert!(msg.contains("120s"), "error was {msg}");
}

#[tokio::test(start_paused = true)]
async fn decide_configured_timeout_wins_over_default() {
    let mut producer = DecisionProducer::new(
        Arc::new(PendingDecisionProvider),
        None,
        Arc::new(triage_set()),
        1024,
        "test-route".into(),
    )
    .with_timeout(Some(Duration::from_secs(5)));
    let err = call(&mut producer, Body::Text("refund please".into()))
        .await
        .expect_err("a pending provider must hit the configured deadline");
    let msg = err.to_string();
    assert!(msg.contains("timeout"), "error was {msg}");
    assert!(msg.contains("5s"), "error was {msg}");
}

#[tokio::test(start_paused = true)]
async fn decide_unlimited_retries_bounded_by_deadline() {
    let provider = RateLimitingDecisionProvider::new(None);
    let mut producer = DecisionProducer::new(
        provider.clone(),
        None,
        Arc::new(triage_set()),
        1024,
        "test-route".into(),
    )
    .with_timeout(Some(Duration::from_secs(120)))
    .with_retry(Some(fixed_second_policy(0)));
    let err = call(&mut producer, Body::Text("refund please".into()))
        .await
        .expect_err("unlimited retries must be bounded by the total deadline");
    let msg = err.to_string();
    assert!(msg.contains("timeout"), "error was {msg}");
    assert!(msg.contains("120s"), "error was {msg}");
    let calls = provider.calls();
    assert!(calls > 1, "retries must happen, got {calls} calls");
    assert!(calls < 200, "deadline must stop retries, got {calls} calls");
}

#[tokio::test(start_paused = true)]
async fn decide_retry_after_cap_applies() {
    let provider = RateLimitingDecisionProvider::new(Some(Duration::from_secs(86_400)));
    let policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 3,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::from_secs(60),
        jitter_factor: 0.0,
        max_attempts_absolute: None,
    };
    let mut producer = DecisionProducer::new(
        provider.clone(),
        None,
        Arc::new(triage_set()),
        1024,
        "test-route".into(),
    )
    .with_timeout(Some(Duration::from_secs(300)))
    .with_retry(Some(policy));
    let err = call(&mut producer, Body::Text("refund please".into()))
        .await
        .expect_err("exhausted retries must error");
    let msg = err.to_string();
    assert!(msg.contains("86400s"), "missing requested value: {msg}");
    assert!(msg.contains("60s"), "missing cap value: {msg}");
    assert_eq!(provider.calls(), 3);
}

/// Pins the emergent composition of the decide default budget and the
/// Retry-After cap: with a 120 s total and `max_delay = 60 s`, the second
/// capped sleep ends exactly at the deadline, so the budget rule outranks
/// the honest error and the surfaced error is `Timeout(120s)` after 2
/// attempts — never `RetryAfterCapped`, never a third attempt.
#[tokio::test(start_paused = true)]
async fn decide_default_budget_outranks_cap_at_deadline_edge() {
    let provider = RateLimitingDecisionProvider::new(Some(Duration::from_secs(86_400)));
    let policy = NetworkRetryPolicy {
        enabled: true,
        max_attempts: 3,
        initial_delay: Duration::from_millis(1),
        multiplier: 1.0,
        max_delay: Duration::from_secs(60),
        jitter_factor: 0.0,
        max_attempts_absolute: None,
    };
    let mut producer = DecisionProducer::new(
        provider.clone(),
        None,
        Arc::new(triage_set()),
        1024,
        "test-route".into(),
    )
    .with_timeout(Some(Duration::from_secs(120)))
    .with_retry(Some(policy));
    let err = call(&mut producer, Body::Text("refund please".into()))
        .await
        .expect_err("spent budget must error");
    let msg = err.to_string();
    assert!(
        msg.contains("timeout") && msg.contains("120s"),
        "budget spent must surface Timeout(120s), got: {msg}"
    );
    assert!(
        !msg.contains("exceeds the"),
        "capped error must not outrank the spent budget: {msg}"
    );
    assert_eq!(
        provider.calls(),
        2,
        "no attempt may launch after a sleep that ends at the deadline"
    );
}
