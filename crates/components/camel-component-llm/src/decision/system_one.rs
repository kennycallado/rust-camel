//! System One Decisions provider and codec (feature `decisions`). Serves
//! both the TypeSafe hosted API and a local Ollama server.

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
use crate::decision::config::OllamaDecisionConfig;
use crate::decision::config::TypesafeDecisionConfig;
use crate::decision::dup_map::DupCheckedMap;
use crate::decision::dup_map::DupEntry;
use crate::decision::http::BaseUrlSource;
use crate::decision::http::DecisionHttp;
use crate::decision::validate;
use crate::decision::validate::RawAnswer;
use crate::decision::validate::RawNamedAnswer;
use crate::error::LlmError;
use crate::provider::http_errors;

const TYPESAFE_BASE_URL: &str = "https://api.typesafe.ai";
const SYSTEM_ONE_PATH: &str = "/v1/systemone";
const TYPESAFE_MAX_QUESTIONS: usize = 400;
const OLLAMA_MAX_QUESTIONS: usize = 64;

/// System One Decisions provider. Its `Debug` output redacts the API key.
#[derive(Debug)]
pub(crate) struct SystemOneDecisionProvider {
    name: String,
    default_model: String,
    http: DecisionHttp,
    max_questions: usize,
}

impl SystemOneDecisionProvider {
    /// Builds a TypeSafe-hosted provider. No network I/O happens here.
    pub(crate) fn new_typesafe(
        name: &str,
        cfg: &TypesafeDecisionConfig,
        policy: SsrfPolicy,
        global_timeout: Option<u64>,
    ) -> Self {
        let base_url = cfg
            .base_url
            .clone()
            .unwrap_or_else(|| TYPESAFE_BASE_URL.to_string());
        let base_url_source = if cfg.base_url.is_some() {
            BaseUrlSource::Configured
        } else {
            BaseUrlSource::Default
        };
        let http = DecisionHttp::new(
            base_url,
            base_url_source,
            SYSTEM_ONE_PATH,
            Some(cfg.api_key.clone()),
            policy,
            request_timeout(cfg.timeout_secs, global_timeout),
        );
        Self {
            name: name.to_string(),
            default_model: cfg.default_model.clone(),
            http,
            max_questions: TYPESAFE_MAX_QUESTIONS,
        }
    }

    /// Builds a local Ollama provider. No network I/O happens here.
    pub(crate) fn new_ollama(
        name: &str,
        cfg: &OllamaDecisionConfig,
        policy: SsrfPolicy,
        global_timeout: Option<u64>,
    ) -> Self {
        let http = DecisionHttp::new(
            cfg.base_url.clone(),
            BaseUrlSource::Configured,
            SYSTEM_ONE_PATH,
            None,
            policy,
            request_timeout(cfg.timeout_secs, global_timeout),
        );
        Self {
            name: name.to_string(),
            default_model: cfg.default_model.clone(),
            http,
            max_questions: OLLAMA_MAX_QUESTIONS,
        }
    }
}

/// Per-attempt timeout: the provider value, else the global one, else the default.
fn request_timeout(per_provider: Option<u64>, global: Option<u64>) -> Duration {
    Duration::from_secs(
        config::effective_timeout_secs(per_provider, global)
            .unwrap_or(config::DEFAULT_ATTEMPT_TIMEOUT_SECS),
    )
}

#[async_trait]
impl DecisionProvider for SystemOneDecisionProvider {
    fn id(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn capabilities(&self) -> DecisionCapabilities {
        DecisionCapabilities {
            max_questions: self.max_questions,
        }
    }

    async fn decide(&self, req: DecisionRequest) -> Result<DecisionResponse, LlmError> {
        let body = build_request(&req)?;
        let bytes = self.http.post_json(body, &req.model).await?;
        parse_response(&req.questions, &bytes)
    }
}

/// Encodes a decision request as a System One request body.
/// Builds the System One request body.
///
/// `questions` and choice `criteria` are JSON objects. Their key order depends
/// on whether `serde_json/preserve_order` is enabled in the build, and it is not
/// significant: answers are correlated by name, never by position.
pub(crate) fn build_request(req: &DecisionRequest) -> Result<Vec<u8>, LlmError> {
    let state = match &req.state {
        DecisionState::Text(text) => Value::String(text.clone()),
        DecisionState::Json(value) => value.clone(),
    };
    let mut questions = Map::new();
    for question in req.questions.iter() {
        questions.insert(question.name.clone(), question_json(question));
    }
    let body = json!({
        "model": req.model,
        "state": state,
        "questions": Value::Object(questions),
    });
    serde_json::to_vec(&body).map_err(|e| {
        LlmError::InvalidRequest(format!("system one decision request encoding failed: {e}"))
    })
}

/// Encodes one question as its System One entry, keyed by name by the caller.
fn question_json(question: &DecisionQuestion) -> Value {
    match &question.kind {
        QuestionKind::Predicate => json!({
            "type": "noul",
            "instructions": question.instructions,
        }),
        QuestionKind::Choice { choices } => {
            let mut criteria = Map::new();
            for choice in choices {
                let description = match &choice.description {
                    Some(text) => Value::String(text.clone()),
                    None => Value::Null,
                };
                criteria.insert(choice.value.clone(), description);
            }
            json!({
                "type": "choice",
                "instructions": question.instructions,
                "criteria": Value::Object(criteria),
            })
        }
        QuestionKind::Score { levels } => {
            let criteria: Vec<Value> = levels
                .iter()
                .map(|level| {
                    let text = match &level.description {
                        Some(description) => format!("{}: {description}", level.label),
                        None => level.label.clone(),
                    };
                    Value::String(text)
                })
                .collect();
            json!({
                "type": "score",
                "instructions": question.instructions,
                "criteria": criteria,
            })
        }
    }
}

/// Decodes a System One response body and correlates its answers with `questions`.
pub(crate) fn parse_response(
    questions: &[DecisionQuestion],
    bytes: &[u8],
) -> Result<DecisionResponse, LlmError> {
    let wire: WireResponse = serde_json::from_slice(bytes).map_err(|e| {
        let detail = http_errors::truncate_body(&e.to_string());
        LlmError::protocol(format!("malformed system one decision response: {detail}"))
    })?;
    let raw = wire
        .answers
        .0
        .into_iter()
        .map(|entry| raw_answer(entry.key, entry.value))
        .collect::<Result<Vec<RawNamedAnswer>, LlmError>>()?;
    let answers = validate::correlate(questions, raw)?;
    Ok(DecisionResponse {
        model: wire.model,
        answers,
        usage: wire.usage.map(usage_from_wire).unwrap_or_default(),
    })
}

/// Maps one wire answer, keyed by its question name, to a named raw answer.
fn raw_answer(name: String, answer: WireAnswer) -> Result<RawNamedAnswer, LlmError> {
    let answer = match answer {
        WireAnswer::Noul { noul } => RawAnswer::Predicate { probability: noul },
        WireAnswer::Choice {
            choice,
            confidence,
            probabilities,
        } => RawAnswer::Choice {
            choice,
            confidence,
            probabilities: probabilities
                .0
                .into_iter()
                .map(|entry| ChoiceProbability {
                    value: entry.key,
                    probability: entry.value,
                })
                .collect(),
        },
        WireAnswer::Score {
            score,
            confidence,
            probabilities,
        } => RawAnswer::Score {
            score,
            confidence,
            probabilities: probabilities
                .0
                .into_iter()
                .map(score_probability)
                .collect::<Result<Vec<ScoreProbability>, LlmError>>()?,
        },
    };
    Ok(RawNamedAnswer { name, answer })
}

/// Maps a score key such as `"0"` to its level index. Other keys are rejected.
fn score_probability(entry: DupEntry<f64>) -> Result<ScoreProbability, LlmError> {
    let level = canonical_index(&entry.key).ok_or_else(|| {
        LlmError::protocol(format!(
            "system one score level '{}' is not an index",
            entry.key
        ))
    })?;
    Ok(ScoreProbability {
        level,
        probability: entry.value,
    })
}

/// Parses a canonical decimal index: non-empty ASCII digits with no leading
/// zero unless the key is exactly `"0"`. Signs, whitespace and padding are
/// rejected, unlike `str::parse`.
fn canonical_index(key: &str) -> Option<usize> {
    let all_digits = !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit());
    let leading_zero = key.len() > 1 && key.starts_with('0');
    if !all_digits || leading_zero {
        return None;
    }
    key.parse::<usize>().ok()
}

fn usage_from_wire(usage: WireUsage) -> DecisionUsage {
    DecisionUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_input_tokens: None,
        cache_write_input_tokens: None,
    }
}

/// Top-level System One decision response. Unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct WireResponse {
    model: String,
    answers: DupCheckedMap<WireAnswer>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

/// One wire answer, tagged by `type`. Its name is the key it sits under.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum WireAnswer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        confidence: Option<f64>,
        probabilities: DupCheckedMap<f64>,
    },
    Score {
        score: f64,
        confidence: Option<f64>,
        probabilities: DupCheckedMap<f64>,
    },
}

#[derive(Debug, Deserialize)]
struct WireUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use serde_json::json;

    use super::{build_request, parse_response};
    use crate::decision::test_support::{SYSTEM_ONE_TRIAGE_RESPONSE, triage_questions};
    use crate::decision::{DecisionAnswer, DecisionRequest, DecisionState, DecisionUsage};
    use crate::error::LlmError;

    /// The `dept` entry exactly as it appears in `SYSTEM_ONE_TRIAGE_RESPONSE`.
    const DEPT_ENTRY: &str = r#""dept":{"type":"choice","choice":"billing","confidence":0.99,"probabilities":{"billing":0.99,"tech":0.01}}"#;

    fn triage_value() -> Value {
        serde_json::from_str(SYSTEM_ONE_TRIAGE_RESPONSE).expect("SYSTEM_ONE_TRIAGE_RESPONSE parses")
    }

    fn wire(value: &Value) -> Vec<u8> {
        serde_json::to_vec(value).expect("fixture serializes")
    }

    fn replace_once(source: &str, from: &str, to: &str) -> Vec<u8> {
        assert!(
            source.contains(from),
            "fixture does not contain the text to replace: {from}"
        );
        source.replacen(from, to, 1).into_bytes()
    }

    fn encoded(req: &DecisionRequest) -> Value {
        let bytes = build_request(req).expect("request builds");
        serde_json::from_slice(&bytes).expect("request body is JSON")
    }

    #[test]
    fn request_shapes() {
        let req = DecisionRequest {
            model: "tev1:0.8b".to_string(),
            state: DecisionState::Text("hi".to_string()),
            questions: triage_questions(),
        };
        let body = encoded(&req);
        assert_eq!(body["state"], json!("hi"));
        assert_eq!(
            body["questions"]["is_billing"],
            json!({ "type": "noul", "instructions": "Is this a billing issue?" })
        );
        assert_eq!(
            body["questions"]["dept"]["criteria"],
            json!({ "billing": "payments", "tech": null })
        );
        assert_eq!(
            body["questions"]["urgency"]["criteria"],
            json!(["low", "medium", "high: outage"])
        );
    }

    #[test]
    fn request_json_state() {
        let req = DecisionRequest {
            model: "tev1:0.8b".to_string(),
            state: DecisionState::Json(json!({ "a": 1 })),
            questions: triage_questions(),
        };
        let body = encoded(&req);
        assert_eq!(body["state"], json!({ "a": 1 }));
    }

    #[test]
    fn parse_design_example() {
        let resp = parse_response(&triage_questions(), SYSTEM_ONE_TRIAGE_RESPONSE.as_bytes())
            .expect("design example parses");

        assert_eq!(resp.model, "tev1:0.8b");
        assert_eq!(resp.answers.len(), 3);
        match &resp.answers[0].answer {
            DecisionAnswer::Predicate { probability } => assert_eq!(*probability, 0.97),
            other => panic!("is_billing should be Predicate, got {other:?}"),
        }
        match &resp.answers[1].answer {
            DecisionAnswer::Choice {
                choice, confidence, ..
            } => {
                assert_eq!(choice, "billing");
                assert_eq!(*confidence, 0.99);
            }
            other => panic!("dept should be Choice, got {other:?}"),
        }
        assert_eq!(
            resp.usage,
            DecisionUsage {
                input_tokens: Some(375),
                output_tokens: Some(0),
                cached_input_tokens: None,
                cache_write_input_tokens: None,
            }
        );
    }

    #[test]
    fn parse_live_probe_fixture() {
        let mut value = triage_value();
        value["answers"]["urgency"] = json!({
            "type": "score",
            "score": 0.9640629691834803,
            "confidence": 0.17,
            "legend": { "0": "low", "1": "high" },
            "probabilities": {
                "0": 0.2933117287232892,
                "1": 0.44931357336994154,
                "2": 0.25737469790676937
            }
        });
        let resp = parse_response(&triage_questions(), &wire(&value)).expect("probe parses");

        match &resp.answers[2].answer {
            DecisionAnswer::Score {
                score,
                probabilities,
                ..
            } => {
                assert_eq!(*score, 0.9640629691834803);
                let levels: Vec<usize> = probabilities.iter().map(|p| p.level).collect();
                assert_eq!(levels, [0, 1, 2]);
                let values: Vec<f64> = probabilities.iter().map(|p| p.probability).collect();
                assert_eq!(
                    values,
                    [0.2933117287232892, 0.44931357336994154, 0.25737469790676937]
                );
            }
            other => panic!("urgency should be Score, got {other:?}"),
        }
    }

    #[test]
    fn parse_duplicate_answer_key_is_protocol() {
        let body = replace_once(
            SYSTEM_ONE_TRIAGE_RESPONSE,
            DEPT_ENTRY,
            &format!("{DEPT_ENTRY},{DEPT_ENTRY}"),
        );
        let result = parse_response(&triage_questions(), &body);
        assert!(matches!(result, Err(LlmError::Protocol(_))));
    }

    #[test]
    fn parse_duplicate_probability_key_is_protocol() {
        let body = replace_once(
            SYSTEM_ONE_TRIAGE_RESPONSE,
            r#""probabilities":{"billing":0.99,"tech":0.01}"#,
            r#""probabilities":{"billing":0.5,"billing":0.5,"tech":0.0}"#,
        );
        let result = parse_response(&triage_questions(), &body);
        assert!(matches!(result, Err(LlmError::Protocol(_))));
    }

    #[test]
    fn parse_missing_answer_is_protocol() {
        let mut value = triage_value();
        value["answers"]
            .as_object_mut()
            .expect("answers is an object")
            .remove("urgency");
        match parse_response(&triage_questions(), &wire(&value)) {
            Err(LlmError::Protocol(m)) => assert!(m.contains("urgency"), "message was {m}"),
            other => panic!("expected Protocol, got {other:?}"),
        }
    }

    #[test]
    fn parse_non_numeric_score_key_is_protocol() {
        let mut value = triage_value();
        value["answers"]["urgency"] = json!({
            "type": "score",
            "score": 0.5,
            "confidence": 0.5,
            "probabilities": { "x": 0.5, "1": 0.25, "2": 0.25 }
        });
        let result = parse_response(&triage_questions(), &wire(&value));
        assert!(matches!(result, Err(LlmError::Protocol(_))));
    }

    #[test]
    fn score_key_with_plus_is_protocol() {
        let body = replace_once(
            SYSTEM_ONE_TRIAGE_RESPONSE,
            r#""probabilities":{"0":0.29,"1":0.45,"2":0.26}"#,
            r#""probabilities":{"+0":0.29,"1":0.45,"2":0.26}"#,
        );
        let result = parse_response(&triage_questions(), &body);
        assert!(matches!(result, Err(LlmError::Protocol(_))));
    }

    #[test]
    fn score_key_with_leading_zero_is_protocol() {
        let body = replace_once(
            SYSTEM_ONE_TRIAGE_RESPONSE,
            r#""probabilities":{"0":0.29,"1":0.45,"2":0.26}"#,
            r#""probabilities":{"00":0.29,"1":0.45,"2":0.26}"#,
        );
        let result = parse_response(&triage_questions(), &body);
        assert!(matches!(result, Err(LlmError::Protocol(_))));
    }

    #[test]
    fn never_produces_refusal() {
        let mut value = triage_value();
        value["answers"]["is_billing"] = json!({ "type": "refusal" });
        let result = parse_response(&triage_questions(), &wire(&value));
        assert!(matches!(result, Err(LlmError::Protocol(_))));
    }
}
