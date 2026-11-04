//! Camel-owned decision contract (ADR-0020 addendum). No genai imports.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::LlmError;

pub mod config;
pub mod endpoint;
pub mod factory;
pub mod mock;
pub mod producer;

#[cfg(any(feature = "decisions", test))]
pub(crate) mod dup_map;

#[cfg(any(feature = "decisions", test))]
pub(crate) mod validate;

#[cfg(feature = "decisions")]
pub(crate) mod http;

#[cfg(feature = "decisions")]
pub(crate) mod openai;

#[cfg(feature = "decisions")]
pub(crate) mod system_one;

/// Provider-agnostic decision seam. Implementations answer a batch of
/// named questions about one piece of input in a single call.
#[async_trait]
pub trait DecisionProvider: Send + Sync {
    /// Stable identifier of this provider instance, used in logs and config.
    fn id(&self) -> &str;

    /// Model used when a request does not name one.
    fn default_model(&self) -> &str;

    /// Static limits advertised by this provider.
    fn capabilities(&self) -> DecisionCapabilities;

    /// Answers every question in `req` and returns one answer per question.
    async fn decide(&self, req: DecisionRequest) -> Result<DecisionResponse, LlmError>;
}

/// Limits advertised by a [`DecisionProvider`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecisionCapabilities {
    /// Maximum number of questions a single `decide` call may carry.
    pub max_questions: usize,
}

impl DecisionCapabilities {
    /// Creates capabilities advertising `max_questions` questions per call.
    pub fn new(max_questions: usize) -> Self {
        Self { max_questions }
    }
}

/// Input to a [`DecisionProvider::decide`] call.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionRequest {
    /// Model identifier to answer with.
    pub model: String,
    /// The input the questions are asked about.
    pub state: DecisionState,
    /// Questions to answer, shared cheaply across retries.
    pub questions: Arc<[DecisionQuestion]>,
}

/// Input payload a decision is made over.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum DecisionState {
    /// Plain text input.
    Text(String),
    /// Structured JSON input.
    Json(serde_json::Value),
}

/// A single named question in a decision question set.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionQuestion {
    /// Name used to correlate the question with its answer.
    pub name: String,
    /// Natural-language instructions shown to the model.
    pub instructions: String,
    /// The answer shape the question expects.
    pub kind: QuestionKind,
}

/// The answer shape a [`DecisionQuestion`] expects.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum QuestionKind {
    /// Yes/no question answered with a probability.
    Predicate,
    /// Pick one of the declared choices.
    Choice {
        /// Options the model may pick from, in declared order.
        choices: Vec<ChoiceOption>,
    },
    /// Pick one of the ordered score levels.
    Score {
        /// Ordered levels, lowest first.
        levels: Vec<ScoreLevel>,
    },
}

/// One option of a choice question.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceOption {
    /// Value returned when this option is picked.
    pub value: String,
    /// Optional human-readable hint for the model.
    #[serde(default)]
    pub description: Option<String>,
}

/// One level of a score question.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoreLevel {
    /// Label of the level, shown to the model.
    pub label: String,
    /// Optional human-readable hint for the model.
    #[serde(default)]
    pub description: Option<String>,
}

/// Result of a [`DecisionProvider::decide`] call.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionResponse {
    /// Model that produced the answers.
    pub model: String,
    /// One answer per question, keyed by question name.
    pub answers: Vec<NamedAnswer>,
    /// Token usage reported for the call.
    pub usage: DecisionUsage,
}

/// An answer paired with the name of the question it answers.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedAnswer {
    /// Name of the question this answer belongs to.
    pub name: String,
    /// The answer itself.
    pub answer: DecisionAnswer,
}

/// A decision answer, shaped by the question kind it answers.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum DecisionAnswer {
    /// Probability that the predicate holds.
    Predicate {
        /// Probability in `[0, 1]`.
        probability: f64,
    },
    /// Chosen option with its confidence and per-option probabilities.
    Choice {
        /// Value of the chosen option.
        choice: String,
        /// Confidence in the chosen option.
        confidence: f64,
        /// Probabilities for each option, in declared order.
        probabilities: Vec<ChoiceProbability>,
    },
    /// Chosen score with its confidence and per-level probabilities.
    Score {
        /// Score value selected by the model.
        score: f64,
        /// Confidence in the selected score.
        confidence: f64,
        /// Probabilities for each level, in level order.
        probabilities: Vec<ScoreProbability>,
    },
    /// The model declined to answer.
    Refusal,
}

/// Probability assigned to one choice option.
#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceProbability {
    /// Value of the option.
    pub value: String,
    /// Probability in `[0, 1]`.
    pub probability: f64,
}

/// Probability assigned to one score level.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreProbability {
    /// Index of the level, in level order.
    pub level: usize,
    /// Probability in `[0, 1]`.
    pub probability: f64,
}

/// Token usage reported by a decision provider. Every field is optional
/// because providers do not all report every counter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecisionUsage {
    /// Input tokens consumed.
    pub input_tokens: Option<u64>,
    /// Output tokens produced.
    pub output_tokens: Option<u64>,
    /// Input tokens served from cache.
    pub cached_input_tokens: Option<u64>,
    /// Input tokens written to cache.
    pub cache_write_input_tokens: Option<u64>,
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod test_support {
    use std::sync::Arc;

    use super::{ChoiceOption, DecisionQuestion, QuestionKind, ScoreLevel};
    use crate::config::LlmGlobalConfig;
    use crate::decision::config::QuestionSetConfig;

    /// Parses [`TRIAGE_TOML`] and returns the `triage` question set.
    pub(crate) fn triage_set() -> QuestionSetConfig {
        let mut cfg: LlmGlobalConfig = toml::from_str(TRIAGE_TOML).expect("TRIAGE_TOML parses");
        cfg.decision_question_sets
            .remove("triage")
            .expect("TRIAGE_TOML defines the triage set")
    }

    /// The canonical 3-question triage set, built in code.
    pub(crate) fn triage_questions() -> Arc<[DecisionQuestion]> {
        Arc::from(vec![
            DecisionQuestion {
                name: "is_billing".to_string(),
                instructions: "Is this a billing issue?".to_string(),
                kind: QuestionKind::Predicate,
            },
            DecisionQuestion {
                name: "dept".to_string(),
                instructions: "Which department?".to_string(),
                kind: QuestionKind::Choice {
                    choices: vec![
                        ChoiceOption {
                            value: "billing".to_string(),
                            description: Some("payments".to_string()),
                        },
                        ChoiceOption {
                            value: "tech".to_string(),
                            description: None,
                        },
                    ],
                },
            },
            DecisionQuestion {
                name: "urgency".to_string(),
                instructions: "How urgent?".to_string(),
                kind: QuestionKind::Score {
                    levels: vec![
                        ScoreLevel {
                            label: "low".to_string(),
                            description: None,
                        },
                        ScoreLevel {
                            label: "medium".to_string(),
                            description: None,
                        },
                        ScoreLevel {
                            label: "high".to_string(),
                            description: Some("outage".to_string()),
                        },
                    ],
                },
            },
        ])
    }

    /// TOML form of the triage question set. Provider tables are prepended by tests.
    pub(crate) const TRIAGE_TOML: &str = r#"
[[decision_question_sets.triage.questions]]
name = "is_billing"
type = "predicate"
instructions = "Is this a billing issue?"
threshold = 0.8

[[decision_question_sets.triage.questions]]
name = "dept"
type = "choice"
instructions = "Which department?"
choices = [{ value = "billing", description = "payments" }, { value = "tech" }]

[[decision_question_sets.triage.questions]]
name = "urgency"
type = "score"
instructions = "How urgent?"
levels = [{ label = "low" }, { label = "medium" }, { label = "high", description = "outage" }]
"#;

    /// Canned OpenAI-style response for the triage set.
    pub(crate) const OPENAI_TRIAGE_RESPONSE: &str = r#"{"model":"gpt-6-luna","answers":[
 {"type":"predicate","name":"is_billing","probability":0.97},
 {"type":"choice","name":"dept","choice":"billing","confidence":0.99,
  "probabilities":[{"value":"billing","probability":0.99},{"value":"tech","probability":0.01}]},
 {"type":"score","name":"urgency","score":0.96,"confidence":0.17,
  "probabilities":[{"label":"low","value":0,"probability":0.29},{"label":"medium","value":1,"probability":0.45},{"label":"high","value":2,"probability":0.26}]}],
 "usage":{"input_tokens":2000,"input_tokens_details":{"cached_tokens":1500,"cache_write_tokens":0},"output_tokens":0,"total_tokens":2000}}"#;

    /// Canned System One-style response for the triage set.
    pub(crate) const SYSTEM_ONE_TRIAGE_RESPONSE: &str = r#"{"model":"tev1:0.8b","answers":{
 "is_billing":{"type":"noul","noul":0.97},
 "dept":{"type":"choice","choice":"billing","confidence":0.99,"probabilities":{"billing":0.99,"tech":0.01}},
 "urgency":{"type":"score","score":0.96,"confidence":0.17,"legend":{"0":"low","1":"medium","2":"high"},
            "probabilities":{"0":0.29,"1":0.45,"2":0.26}}},
 "usage":{"input_tokens":375,"output_tokens":0}}"#;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_usage_default_is_all_none() {
        let usage = DecisionUsage::default();
        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.output_tokens, None);
        assert_eq!(usage.cached_input_tokens, None);
        assert_eq!(usage.cache_write_input_tokens, None);
    }

    #[test]
    fn capabilities_new_sets_max_questions() {
        assert_eq!(
            DecisionCapabilities::new(12),
            DecisionCapabilities { max_questions: 12 }
        );
    }

    #[test]
    fn triage_fixture_shape() {
        let questions = test_support::triage_questions();
        assert_eq!(questions.len(), 3);
        let names: Vec<&str> = questions.iter().map(|q| q.name.as_str()).collect();
        assert_eq!(names, ["is_billing", "dept", "urgency"]);
        match &questions[2].kind {
            QuestionKind::Score { levels } => assert_eq!(levels.len(), 3),
            other => panic!("urgency should be Score, got {other:?}"),
        }
    }

    #[test]
    fn choice_option_rejects_unknown_field() {
        assert!(toml::from_str::<ChoiceOption>("value = \"a\"\nweight = 1").is_err());
    }
}
