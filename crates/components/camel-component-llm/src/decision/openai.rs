//! OpenAI Decisions provider and codec (feature `decisions`).

use std::time::Duration;

use async_trait::async_trait;
use camel_api::SsrfPolicy;
use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;

use crate::decision::ChoiceProbability;
use crate::decision::DecisionCapabilities;
use crate::decision::DecisionProvider;
use crate::decision::DecisionQuestion;
use crate::decision::DecisionRequest;
use crate::decision::DecisionResponse;
use crate::decision::DecisionState;
use crate::decision::DecisionUsage;
use crate::decision::QuestionKind;
use crate::decision::ScoreProbability;
use crate::decision::config;
use crate::decision::config::OpenaiDecisionConfig;
use crate::decision::http::BaseUrlSource;
use crate::decision::http::DecisionHttp;
use crate::decision::validate;
use crate::decision::validate::RawAnswer;
use crate::decision::validate::RawNamedAnswer;
use crate::error::LlmError;
use crate::provider::http_errors;

const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const DECISIONS_PATH: &str = "/decisions";
const MAX_QUESTIONS: usize = 400;

/// OpenAI Decisions provider. Its `Debug` output redacts the API key.
#[derive(Debug)]
pub(crate) struct OpenaiDecisionProvider {
    name: String,
    default_model: String,
    http: DecisionHttp,
}

impl OpenaiDecisionProvider {
    /// Builds a provider from the OpenAI config. No network I/O happens here.
    pub(crate) fn new(
        name: &str,
        cfg: &OpenaiDecisionConfig,
        policy: SsrfPolicy,
        global_timeout: Option<u64>,
    ) -> Self {
        let base_url = cfg
            .base_url
            .clone()
            .unwrap_or_else(|| OPENAI_BASE_URL.to_string());
        let timeout = Duration::from_secs(
            config::effective_timeout_secs(cfg.timeout_secs, global_timeout)
                .unwrap_or(config::DEFAULT_ATTEMPT_TIMEOUT_SECS),
        );
        let base_url_source = if cfg.base_url.is_some() {
            BaseUrlSource::Configured
        } else {
            BaseUrlSource::Default
        };
        let http = DecisionHttp::new(
            base_url,
            base_url_source,
            DECISIONS_PATH,
            Some(cfg.api_key.clone()),
            policy,
            timeout,
        );
        Self {
            name: name.to_string(),
            default_model: cfg.default_model.clone(),
            http,
        }
    }
}

#[async_trait]
impl DecisionProvider for OpenaiDecisionProvider {
    fn id(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn capabilities(&self) -> DecisionCapabilities {
        DecisionCapabilities {
            max_questions: MAX_QUESTIONS,
        }
    }

    async fn decide(&self, req: DecisionRequest) -> Result<DecisionResponse, LlmError> {
        let body = build_request(&req)?;
        let bytes = self.http.post_json(body, &req.model).await?;
        parse_response(&req.questions, &bytes)
    }
}

/// Encodes a decision request as an OpenAI request body.
pub(crate) fn build_request(req: &DecisionRequest) -> Result<Vec<u8>, LlmError> {
    let input = match &req.state {
        DecisionState::Text(text) => Value::String(text.clone()),
        DecisionState::Json(value) => json_input(value)?,
    };
    let questions: Vec<Value> = req.questions.iter().map(question_json).collect();
    let body = json!({
        "model": req.model,
        "input": input,
        "questions": questions,
    });
    serde_json::to_vec(&body).map_err(|e| {
        LlmError::InvalidRequest(format!("openai decision request encoding failed: {e}"))
    })
}

/// Wraps a JSON object, or an array of objects, as one `input_json` part.
fn json_input(value: &Value) -> Result<Value, LlmError> {
    let accepted = match value {
        Value::Object(_) => true,
        Value::Array(items) => items.iter().all(Value::is_object),
        _ => false,
    };
    if !accepted {
        return Err(LlmError::InvalidRequest(
            "openai decisions require a JSON object or an array of objects".to_string(),
        ));
    }
    Ok(json!([{
        "role": "user",
        "content": [{ "type": "input_json", "data": value }],
    }]))
}

/// Encodes one question as its OpenAI request entry.
fn question_json(question: &DecisionQuestion) -> Value {
    let mut entry = json!({
        "name": question.name,
        "instructions": question.instructions,
    });
    match &question.kind {
        QuestionKind::Predicate => entry["type"] = json!("predicate"),
        QuestionKind::Choice { choices } => {
            entry["type"] = json!("choice");
            entry["choices"] = Value::Array(
                choices
                    .iter()
                    .map(|c| described("value", &c.value, c.description.as_deref()))
                    .collect(),
            );
        }
        QuestionKind::Score { levels } => {
            entry["type"] = json!("score");
            entry["levels"] = Value::Array(
                levels
                    .iter()
                    .map(|l| described("label", &l.label, l.description.as_deref()))
                    .collect(),
            );
        }
    }
    entry
}

/// `{key: value, description}`, with `description` omitted when absent.
fn described(key: &str, value: &str, description: Option<&str>) -> Value {
    let mut entry = Map::new();
    entry.insert(key.to_string(), Value::String(value.to_string()));
    if let Some(text) = description {
        entry.insert("description".to_string(), Value::String(text.to_string()));
    }
    Value::Object(entry)
}

/// Decodes an OpenAI response body and correlates its answers with `questions`.
pub(crate) fn parse_response(
    questions: &[DecisionQuestion],
    bytes: &[u8],
) -> Result<DecisionResponse, LlmError> {
    let wire: WireResponse = serde_json::from_slice(bytes).map_err(|e| {
        let detail = http_errors::truncate_body(&e.to_string());
        LlmError::protocol(format!("malformed openai decision response: {detail}"))
    })?;
    let raw = wire
        .answers
        .into_iter()
        .map(raw_answer)
        .collect::<Result<Vec<RawNamedAnswer>, LlmError>>()?;
    let answers = validate::correlate(questions, raw)?;
    Ok(DecisionResponse {
        model: wire.model,
        answers,
        usage: wire.usage.map(usage_from_wire).unwrap_or_default(),
    })
}

/// Maps one wire answer to a named raw answer. Fails when the name is absent.
fn raw_answer(answer: WireAnswer) -> Result<RawNamedAnswer, LlmError> {
    let (name, answer) = match answer {
        WireAnswer::Predicate { name, probability } => (name, RawAnswer::Predicate { probability }),
        WireAnswer::Choice {
            name,
            choice,
            confidence,
            probabilities,
        } => (
            name,
            RawAnswer::Choice {
                choice,
                confidence,
                probabilities: probabilities
                    .into_iter()
                    .map(|p| ChoiceProbability {
                        value: p.value,
                        probability: p.probability,
                    })
                    .collect(),
            },
        ),
        WireAnswer::Score {
            name,
            score,
            confidence,
            probabilities,
        } => {
            let probabilities = probabilities
                .into_iter()
                .map(score_probability)
                .collect::<Result<Vec<ScoreProbability>, LlmError>>()?;
            (
                name,
                RawAnswer::Score {
                    score,
                    confidence,
                    probabilities,
                },
            )
        }
        WireAnswer::Refusal { name } => (name, RawAnswer::Refusal),
    };
    let name = name.ok_or_else(|| LlmError::Protocol("openai answer without name".to_string()))?;
    Ok(RawNamedAnswer { name, answer })
}

/// Maps a wire score level to its index. The wire `label` is ignored.
fn score_probability(p: WireScoreProbability) -> Result<ScoreProbability, LlmError> {
    let level = usize::try_from(p.value)
        .map_err(|_| LlmError::protocol(format!("openai score level {} is negative", p.value)))?;
    Ok(ScoreProbability {
        level,
        probability: p.probability,
    })
}

fn usage_from_wire(usage: WireUsage) -> DecisionUsage {
    let details = usage.input_tokens_details;
    DecisionUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_input_tokens: details.as_ref().and_then(|d| d.cached_tokens),
        cache_write_input_tokens: details.as_ref().and_then(|d| d.cache_write_tokens),
    }
}

/// Top-level OpenAI decision response. Unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct WireResponse {
    model: String,
    answers: Vec<WireAnswer>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

/// One wire answer, tagged by `type`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum WireAnswer {
    Predicate {
        name: Option<String>,
        probability: f64,
    },
    Choice {
        name: Option<String>,
        choice: String,
        confidence: Option<f64>,
        probabilities: Vec<WireChoiceProbability>,
    },
    Score {
        name: Option<String>,
        score: f64,
        confidence: Option<f64>,
        probabilities: Vec<WireScoreProbability>,
    },
    Refusal {
        name: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
struct WireChoiceProbability {
    value: String,
    probability: f64,
}

/// The wire `label` is not decoded: the declared label is used instead.
#[derive(Debug, Deserialize)]
struct WireScoreProbability {
    value: i64,
    probability: f64,
}

#[derive(Debug, Deserialize)]
struct WireUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    input_tokens_details: Option<WireInputTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct WireInputTokensDetails {
    cached_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
    use camel_api::SsrfPolicy;
    use serde_json::Value;
    use serde_json::json;

    use super::{OpenaiDecisionProvider, build_request, parse_response};
    use crate::decision::config::OpenaiDecisionConfig;
    use crate::decision::test_support::{OPENAI_TRIAGE_RESPONSE, triage_questions};
    use crate::decision::{
        DecisionAnswer, DecisionQuestion, DecisionRequest, DecisionState, DecisionUsage,
        QuestionKind,
    };
    use crate::error::LlmError;

    fn triage_value() -> Value {
        serde_json::from_str(OPENAI_TRIAGE_RESPONSE).expect("OPENAI_TRIAGE_RESPONSE parses")
    }

    fn wire(value: &Value) -> Vec<u8> {
        serde_json::to_vec(value).expect("fixture serializes")
    }

    fn text_request(text: &str) -> DecisionRequest {
        DecisionRequest {
            model: "gpt-6-luna".to_string(),
            state: DecisionState::Text(text.to_string()),
            questions: triage_questions(),
        }
    }

    fn encoded(req: &DecisionRequest) -> Value {
        let bytes = build_request(req).expect("request builds");
        serde_json::from_slice(&bytes).expect("request body is JSON")
    }

    #[test]
    fn request_text_state_and_all_kinds() {
        let body = encoded(&text_request("hi"));
        assert_eq!(body["input"], json!("hi"));
        assert_eq!(
            body["questions"][1],
            json!({
                "type": "choice",
                "name": "dept",
                "instructions": "Which department?",
                "choices": [
                    { "value": "billing", "description": "payments" },
                    { "value": "tech" }
                ]
            })
        );
        assert_eq!(body["questions"][2]["levels"][0], json!({ "label": "low" }));
    }

    #[test]
    fn request_json_object_state() {
        let req = DecisionRequest {
            model: "gpt-6-luna".to_string(),
            state: DecisionState::Json(json!({ "a": 1 })),
            questions: triage_questions(),
        };
        let body = encoded(&req);
        assert_eq!(
            body["input"][0]["content"][0],
            json!({ "type": "input_json", "data": { "a": 1 } })
        );
    }

    #[test]
    fn request_json_array_of_scalars_rejected() {
        let req = DecisionRequest {
            model: "gpt-6-luna".to_string(),
            state: DecisionState::Json(json!([1, 2])),
            questions: triage_questions(),
        };
        assert!(matches!(
            build_request(&req),
            Err(LlmError::InvalidRequest(_))
        ));
    }

    #[test]
    fn parse_design_example() {
        let mut value = triage_value();
        value["answers"]
            .as_array_mut()
            .expect("answers is an array")
            .push(json!({ "type": "refusal", "name": "other" }));
        let mut questions = triage_questions().to_vec();
        questions.push(DecisionQuestion {
            name: "other".to_string(),
            instructions: "Other?".to_string(),
            kind: QuestionKind::Predicate,
        });

        let resp = parse_response(&questions, &wire(&value)).expect("design example parses");

        assert_eq!(resp.answers.len(), 4);
        assert!(matches!(
            resp.answers[0].answer,
            DecisionAnswer::Predicate { .. }
        ));
        match &resp.answers[1].answer {
            DecisionAnswer::Choice { choice, .. } => assert_eq!(choice, "billing"),
            other => panic!("dept should be Choice, got {other:?}"),
        }
        match &resp.answers[2].answer {
            DecisionAnswer::Score { score, .. } => assert_eq!(*score, 0.96),
            other => panic!("urgency should be Score, got {other:?}"),
        }
        assert_eq!(resp.answers[3].answer, DecisionAnswer::Refusal);
        assert_eq!(
            resp.usage,
            DecisionUsage {
                input_tokens: Some(2000),
                output_tokens: Some(0),
                cached_input_tokens: Some(1500),
                cache_write_input_tokens: Some(0),
            }
        );
    }

    #[test]
    fn parse_reverse_order() {
        let mut value = triage_value();
        value["answers"]
            .as_array_mut()
            .expect("answers is an array")
            .reverse();
        let resp = parse_response(&triage_questions(), &wire(&value)).expect("reversed parses");
        let names: Vec<&str> = resp.answers.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["is_billing", "dept", "urgency"]);
    }

    #[test]
    fn parse_duplicate_name_is_protocol() {
        let mut value = triage_value();
        value["answers"]
            .as_array_mut()
            .expect("answers is an array")
            .push(json!({ "type": "predicate", "name": "is_billing", "probability": 0.5 }));
        let result = parse_response(&triage_questions(), &wire(&value));
        assert!(matches!(result, Err(LlmError::Protocol(_))));
    }

    #[test]
    fn parse_duplicate_probability_value_is_protocol() {
        let mut value = triage_value();
        value["answers"][1]["probabilities"]
            .as_array_mut()
            .expect("probabilities is an array")
            .push(json!({ "value": "billing", "probability": 0.0 }));
        let result = parse_response(&triage_questions(), &wire(&value));
        assert!(matches!(result, Err(LlmError::Protocol(_))));
    }

    #[test]
    fn parse_missing_confidence_is_protocol() {
        let mut value = triage_value();
        value["answers"][1]
            .as_object_mut()
            .expect("dept answer is an object")
            .remove("confidence");
        match parse_response(&triage_questions(), &wire(&value)) {
            Err(LlmError::Protocol(m)) => assert!(m.contains("confidence"), "message was {m}"),
            other => panic!("expected Protocol, got {other:?}"),
        }
    }

    #[test]
    fn provider_debug_redacts_key() {
        let cfg = OpenaiDecisionConfig {
            api_key: "sk-secret-123".to_string(),
            default_model: "gpt-6-luna".to_string(),
            base_url: None,
            timeout_secs: None,
            max_concurrency: None,
            network_retry: None,
        };
        let provider = OpenaiDecisionProvider::new("oai", &cfg, SsrfPolicy::AllowInternal, None);
        assert!(
            !format!("{provider:?}").contains("sk-secret-123"), // allow-secret: asserts Debug redaction
            "Debug output must not leak the api key"
        );
    }

    #[test]
    fn parse_ignores_wire_label() {
        let mut value = triage_value();
        value["answers"][2]["probabilities"][0]["label"] = json!("LOW");
        parse_response(&triage_questions(), &wire(&value)).expect("wire label is ignored");
    }

    #[test]
    fn parse_malformed_json_is_protocol() {
        let result = parse_response(&triage_questions(), b"not json");
        assert!(matches!(result, Err(LlmError::Protocol(_))));
    }
}
