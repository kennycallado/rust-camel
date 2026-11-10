//! Decision producer: maps an Exchange body to decision state, asks a
//! [`DecisionProvider`] the configured questions, and writes the answers back
//! to the Exchange as a stable JSON body.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::task::{Context, Poll};
use std::time::Duration;

use camel_api::{Body, CamelError, Exchange};
use camel_component_api::NetworkRetryPolicy;
use serde_json::{Map, Value, json};
use tokio::sync::Semaphore;
use tower::Service;

use crate::decision::config::{QuestionConfig, QuestionSetConfig};
use crate::decision::{
    DecisionAnswer, DecisionProvider, DecisionQuestion, DecisionRequest, DecisionResponse,
    DecisionState, QuestionKind, ScoreLevel,
};
use crate::error::LlmError;
use crate::headers::{
    CAMEL_LLM_CACHE_WRITE_TOKENS_IN, CAMEL_LLM_CACHED_TOKENS_IN, CAMEL_LLM_MODEL,
    CAMEL_LLM_PROVIDER, CAMEL_LLM_TOKENS_IN, CAMEL_LLM_TOKENS_OUT, CAMEL_LLM_USAGE_AVAILABLE,
};

/// Producer for `llm:decide`. Answers every question of one question set about
/// the Exchange body in a single provider call.
#[derive(Clone)]
pub struct DecisionProducer {
    provider: Arc<dyn DecisionProvider>,
    model: String,
    set: Arc<QuestionSetConfig>,
    questions: Arc<[DecisionQuestion]>,
    max_state_bytes: usize,
    semaphore: Option<Arc<Semaphore>>,
    timeout: Option<Duration>,
    retry: Option<NetworkRetryPolicy>,
    route_id: String,
}

impl DecisionProducer {
    /// Creates a producer for `set`. `model_override` wins over the provider's
    /// default model. `max_state_bytes` caps the decision state size.
    pub fn new(
        provider: Arc<dyn DecisionProvider>,
        model_override: Option<String>,
        set: Arc<QuestionSetConfig>,
        max_state_bytes: usize,
        route_id: String,
    ) -> Self {
        let model = model_override.unwrap_or_else(|| provider.default_model().to_string());
        let questions = set.to_questions();
        Self {
            provider,
            model,
            set,
            questions,
            max_state_bytes,
            semaphore: None,
            timeout: None,
            retry: None,
            route_id,
        }
    }

    /// Sets the concurrency limit shared by this producer's calls. `None` is unbounded.
    pub fn with_semaphore(mut self, semaphore: Option<Arc<Semaphore>>) -> Self {
        self.semaphore = semaphore;
        self
    }

    /// Sets the total deadline for one decide call. `None` disables it.
    pub fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the retry policy for transient provider failures. `None` disables retries.
    pub fn with_retry(mut self, retry: Option<NetworkRetryPolicy>) -> Self {
        self.retry = retry;
        self
    }

    /// Maps the body to decision state. Runs before any provider I/O. The size
    /// is checked on borrowed data first, so an oversize body is never copied
    /// or converted.
    fn extract_state(&self, body: &Body) -> Result<DecisionState, LlmError> {
        match body {
            Body::Text(s) | Body::Xml(s) => {
                self.check_state_len(s.len())?;
                Ok(DecisionState::Text(s.clone()))
            }
            Body::Bytes(b) => {
                self.check_state_len(b.len())?;
                let text = std::str::from_utf8(b).map_err(|_| {
                    LlmError::InvalidRequest("decision state is not valid UTF-8".into())
                })?;
                Ok(DecisionState::Text(text.to_owned()))
            }
            Body::Json(Value::String(s)) => {
                self.check_state_len(s.len())?;
                Ok(DecisionState::Text(s.clone()))
            }
            Body::Json(v @ (Value::Object(_) | Value::Array(_))) => {
                let size = serde_json::to_vec(v)
                    .map_err(|e| {
                        LlmError::InvalidRequest(format!("decision state is not serializable: {e}"))
                    })?
                    .len();
                self.check_state_len(size)?;
                Ok(DecisionState::Json(v.clone()))
            }
            _ => Err(LlmError::InvalidRequest(
                "unsupported body type for llm:decide".into(),
            )),
        }
    }

    /// Rejects a state whose size in bytes is larger than `max_state_bytes`.
    fn check_state_len(&self, len: usize) -> Result<(), LlmError> {
        if len > self.max_state_bytes {
            return Err(LlmError::InvalidRequest(format!(
                "decision state exceeds {} bytes",
                self.max_state_bytes
            )));
        }
        Ok(())
    }

    /// Verifies a provider response against the producer's questions before
    /// any of it reaches the Exchange. Answers must match the questions by name
    /// and position, answer kinds must match question kinds (refusals are
    /// accepted for any kind), and choice and score values must be declared.
    fn check_response_shape(&self, resp: &DecisionResponse) -> Result<(), LlmError> {
        if resp.answers.len() != self.questions.len() {
            let missing = self
                .questions
                .iter()
                .find(|q| !resp.answers.iter().any(|a| a.name == q.name));
            return Err(LlmError::protocol(match missing {
                Some(q) => format!("no answer for question '{}'", q.name),
                None => format!(
                    "{} answers returned for {} questions",
                    resp.answers.len(),
                    self.questions.len()
                ),
            }));
        }
        for (question, named) in self.questions.iter().zip(&resp.answers) {
            if named.name != question.name {
                return Err(LlmError::protocol(format!(
                    "answer '{}' is out of place for question '{}'",
                    named.name, question.name
                )));
            }
            match (&question.kind, &named.answer) {
                (_, DecisionAnswer::Refusal) => {}
                (QuestionKind::Predicate, DecisionAnswer::Predicate { .. }) => {}
                (QuestionKind::Choice { choices }, DecisionAnswer::Choice { choice, .. }) => {
                    if !choices.iter().any(|c| &c.value == choice) {
                        return Err(LlmError::protocol(format!(
                            "choice '{}' is not declared for question '{}'",
                            choice, question.name
                        )));
                    }
                }
                (QuestionKind::Score { levels }, DecisionAnswer::Score { probabilities, .. }) => {
                    if let Some(p) = probabilities.iter().find(|p| p.level >= levels.len()) {
                        return Err(LlmError::protocol(format!(
                            "score level {} is not declared for question '{}' ({} levels)",
                            p.level,
                            question.name,
                            levels.len()
                        )));
                    }
                }
                _ => {
                    return Err(LlmError::protocol(format!(
                        "answer kind does not match question '{}'",
                        question.name
                    )));
                }
            }
        }
        Ok(())
    }

    /// Calls the provider under the producer's timeout and retry policy.
    async fn call_provider(&self, request: DecisionRequest) -> Result<DecisionResponse, LlmError> {
        let provider = Arc::clone(&self.provider);
        let op = move |_content_started: Arc<AtomicBool>| {
            let req = request.clone();
            let p = Arc::clone(&provider);
            async move { p.decide(req).await }
        };
        let fut = crate::producer::run_with_retry(
            self.semaphore.clone(),
            self.retry.clone(),
            self.timeout,
            Arc::new(AtomicBool::new(false)),
            &self.route_id,
            op,
        );
        crate::producer::apply_timeout(self.timeout, fut).await
    }

    /// Extracts the state, calls the provider, and writes the response.
    async fn handle(&self, exchange: &mut Exchange) -> Result<(), LlmError> {
        let state = self.extract_state(&exchange.input.body)?;
        let request = DecisionRequest {
            model: self.model.clone(),
            state,
            questions: Arc::clone(&self.questions),
        };
        let response = self.call_provider(request).await?;
        self.check_response_shape(&response)?;
        self.write_response(exchange, &response);
        Ok(())
    }

    /// Replaces the body with the answers and sets the provider, model, and usage headers.
    fn write_response(&self, exchange: &mut Exchange, resp: &DecisionResponse) {
        let answers = self.answers_json(resp);
        let headers = &mut exchange.input.headers;
        headers.insert(
            CAMEL_LLM_PROVIDER.to_string(),
            Value::String(self.provider.id().to_string()),
        );
        headers.insert(
            CAMEL_LLM_MODEL.to_string(),
            Value::String(resp.model.clone()),
        );
        headers.insert(CAMEL_LLM_USAGE_AVAILABLE.to_string(), Value::Bool(true));
        set_count_header(headers, CAMEL_LLM_TOKENS_IN, resp.usage.input_tokens);
        set_count_header(headers, CAMEL_LLM_TOKENS_OUT, resp.usage.output_tokens);
        set_count_header(
            headers,
            CAMEL_LLM_CACHED_TOKENS_IN,
            resp.usage.cached_input_tokens,
        );
        set_count_header(
            headers,
            CAMEL_LLM_CACHE_WRITE_TOKENS_IN,
            resp.usage.cache_write_input_tokens,
        );
        exchange.input.body = Body::Json(json!({"model": resp.model, "answers": answers}));
    }

    /// Builds the `answers` object, keyed by question name.
    fn answers_json(&self, resp: &DecisionResponse) -> Value {
        let answers: Map<String, Value> = resp
            .answers
            .iter()
            .map(|named| {
                let config = self.set.questions.iter().find(|q| q.name == named.name);
                let question = self.questions.iter().find(|q| q.name == named.name);
                (
                    named.name.clone(),
                    answer_json(&named.answer, config, question),
                )
            })
            .collect();
        Value::Object(answers)
    }
}

/// Sets a token-count header only when the provider reported the value.
fn set_count_header(headers: &mut HashMap<String, Value>, name: &str, count: Option<u64>) {
    if let Some(n) = count {
        headers.insert(name.to_string(), Value::from(n));
    }
}

/// Builds one answer object. `config` supplies the optional threshold and minimum
/// confidence, and `question` supplies the score level labels.
fn answer_json(
    answer: &DecisionAnswer,
    config: Option<&QuestionConfig>,
    question: Option<&DecisionQuestion>,
) -> Value {
    match answer {
        DecisionAnswer::Predicate { probability } => {
            predicate_json(*probability, config.and_then(|c| c.threshold))
        }
        DecisionAnswer::Choice {
            choice,
            confidence,
            probabilities,
        } => {
            let probs: Map<String, Value> = probabilities
                .iter()
                .map(|p| (p.value.clone(), json!(p.probability)))
                .collect();
            let mut obj = json!({
                "type": "choice",
                "choice": choice,
                "confidence": confidence,
                "probabilities": probs,
            });
            put_min_confidence(&mut obj, *confidence, config.and_then(|c| c.min_confidence));
            obj
        }
        DecisionAnswer::Score {
            score,
            confidence,
            probabilities,
        } => {
            let levels = score_levels(question);
            let probs: Vec<Value> = probabilities
                .iter()
                .map(|p| {
                    json!({
                        "level": p.level,
                        "label": level_label(levels, p.level),
                        "probability": p.probability,
                    })
                })
                .collect();
            let mut obj = json!({
                "type": "score",
                "score": score,
                "confidence": confidence,
                "probabilities": probs,
            });
            put_min_confidence(&mut obj, *confidence, config.and_then(|c| c.min_confidence));
            obj
        }
        DecisionAnswer::Refusal => json!({"type": "refusal"}),
    }
}

/// Builds a predicate answer. `meets_threshold` appears only when a threshold is set.
fn predicate_json(probability: f64, threshold: Option<f64>) -> Value {
    let mut obj = json!({"type": "predicate", "probability": probability});
    if let Some(t) = threshold {
        put(&mut obj, "meets_threshold", json!(probability >= t));
    }
    obj
}

/// Adds `meets_min_confidence` only when a minimum confidence is configured.
fn put_min_confidence(obj: &mut Value, confidence: f64, min: Option<f64>) {
    if let Some(m) = min {
        put(obj, "meets_min_confidence", json!(confidence >= m));
    }
}

/// Inserts `key` into a JSON object. Does nothing when `obj` is not an object.
fn put(obj: &mut Value, key: &str, value: Value) {
    if let Value::Object(map) = obj {
        map.insert(key.to_string(), value);
    }
}

/// Returns the ordered levels of a score question, or an empty slice otherwise.
fn score_levels(question: Option<&DecisionQuestion>) -> &[ScoreLevel] {
    match question.map(|q| &q.kind) {
        Some(QuestionKind::Score { levels }) => levels.as_slice(),
        _ => &[],
    }
}

/// Label of a 0-based score level. `check_response_shape` runs before this and
/// rejects undeclared levels, so the empty fallback is unreachable from
/// `handle`. It is kept as a defensive default for direct callers.
fn level_label(levels: &[ScoreLevel], level: usize) -> String {
    levels
        .get(level)
        .map(|l| l.label.clone())
        .unwrap_or_default()
}

impl Service<Exchange> for DecisionProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let producer = self.clone();

        Box::pin(async move {
            let result = producer.handle(&mut exchange).await;
            match result {
                Ok(()) => Ok(exchange),
                Err(e) => {
                    tracing::warn!(route_id = %producer.route_id, error = %e, "llm decide producer error");
                    Err(CamelError::from(e))
                }
            }
        })
    }
}

#[cfg(test)]
#[path = "producer_tests.rs"]
mod tests;
