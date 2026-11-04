//! Deterministic mock decision provider.

use async_trait::async_trait;

use crate::decision::ChoiceProbability;
use crate::decision::DecisionAnswer;
use crate::decision::DecisionCapabilities;
use crate::decision::DecisionProvider;
use crate::decision::DecisionQuestion;
use crate::decision::DecisionRequest;
use crate::decision::DecisionResponse;
use crate::decision::DecisionUsage;
use crate::decision::NamedAnswer;
use crate::decision::QuestionKind;
use crate::decision::ScoreProbability;
use crate::decision::config::MockDecisionConfig;
use crate::error::LlmError;

/// Maximum question count advertised by the mock provider.
const MOCK_MAX_QUESTIONS: usize = 400;

/// Mock decision provider that answers every question deterministically.
pub struct MockDecisionProvider {
    name: String,
    config: MockDecisionConfig,
}

impl MockDecisionProvider {
    /// Creates a mock provider registered under `name`.
    pub fn new(name: impl Into<String>, config: MockDecisionConfig) -> Self {
        Self {
            name: name.into(),
            config,
        }
    }

    fn answer_for(&self, question: &DecisionQuestion) -> DecisionAnswer {
        if self.config.refuse.contains(&question.name) {
            return DecisionAnswer::Refusal;
        }
        match &question.kind {
            QuestionKind::Predicate => DecisionAnswer::Predicate {
                probability: self.config.predicate_probability,
            },
            QuestionKind::Choice { choices } => DecisionAnswer::Choice {
                choice: choices.first().map(|c| c.value.clone()).unwrap_or_default(),
                confidence: 1.0,
                probabilities: choices
                    .iter()
                    .enumerate()
                    .map(|(i, c)| ChoiceProbability {
                        value: c.value.clone(),
                        probability: if i == 0 { 1.0 } else { 0.0 },
                    })
                    .collect(),
            },
            QuestionKind::Score { levels } => DecisionAnswer::Score {
                score: 0.0,
                confidence: 1.0,
                probabilities: (0..levels.len())
                    .map(|level| ScoreProbability {
                        level,
                        probability: if level == 0 { 1.0 } else { 0.0 },
                    })
                    .collect(),
            },
        }
    }
}

#[async_trait]
impl DecisionProvider for MockDecisionProvider {
    fn id(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> &str {
        &self.config.default_model
    }

    fn capabilities(&self) -> DecisionCapabilities {
        DecisionCapabilities::new(MOCK_MAX_QUESTIONS)
    }

    async fn decide(&self, req: DecisionRequest) -> Result<DecisionResponse, LlmError> {
        if let Some(msg) = &self.config.error_message {
            return Err(LlmError::provider(msg.clone()));
        }
        let answers = req
            .questions
            .iter()
            .map(|q| NamedAnswer {
                name: q.name.clone(),
                answer: self.answer_for(q),
            })
            .collect();
        Ok(DecisionResponse {
            model: req.model,
            answers,
            usage: DecisionUsage::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::ChoiceProbability;
    use crate::decision::DecisionAnswer;
    use crate::decision::DecisionState;
    use crate::decision::DecisionUsage;
    use crate::decision::NamedAnswer;
    use crate::decision::ScoreProbability;
    use crate::decision::test_support::triage_questions;

    fn config(
        predicate_probability: f64,
        refuse: Vec<String>,
        error_message: Option<String>,
    ) -> MockDecisionConfig {
        MockDecisionConfig {
            default_model: "mock-decision".to_string(),
            predicate_probability,
            refuse,
            error_message,
        }
    }

    fn request() -> DecisionRequest {
        DecisionRequest {
            model: "mock-decision".to_string(),
            state: DecisionState::Text("refund please".to_string()),
            questions: triage_questions(),
        }
    }

    #[tokio::test]
    async fn mock_answers_mixed_set() {
        let provider =
            MockDecisionProvider::new("mock", config(0.25, vec!["urgency".to_string()], None));
        let response = provider
            .decide(request())
            .await
            .expect("mock should answer");
        assert_eq!(
            response.answers,
            vec![
                NamedAnswer {
                    name: "is_billing".to_string(),
                    answer: DecisionAnswer::Predicate { probability: 0.25 },
                },
                NamedAnswer {
                    name: "dept".to_string(),
                    answer: DecisionAnswer::Choice {
                        choice: "billing".to_string(),
                        confidence: 1.0,
                        probabilities: vec![
                            ChoiceProbability {
                                value: "billing".to_string(),
                                probability: 1.0,
                            },
                            ChoiceProbability {
                                value: "tech".to_string(),
                                probability: 0.0,
                            },
                        ],
                    },
                },
                NamedAnswer {
                    name: "urgency".to_string(),
                    answer: DecisionAnswer::Refusal,
                },
            ]
        );
    }

    #[tokio::test]
    async fn mock_score_defaults_to_level_zero() {
        let provider = MockDecisionProvider::new("mock", config(1.0, vec![], None));
        let response = provider
            .decide(request())
            .await
            .expect("mock should answer");
        assert_eq!(
            response.answers[2].answer,
            DecisionAnswer::Score {
                score: 0.0,
                confidence: 1.0,
                probabilities: vec![
                    ScoreProbability {
                        level: 0,
                        probability: 1.0,
                    },
                    ScoreProbability {
                        level: 1,
                        probability: 0.0,
                    },
                    ScoreProbability {
                        level: 2,
                        probability: 0.0,
                    },
                ],
            }
        );
        assert_eq!(response.usage, DecisionUsage::default());
    }

    #[tokio::test]
    async fn mock_error_message_fails() {
        let provider =
            MockDecisionProvider::new("mock", config(1.0, vec![], Some("boom".to_string())));
        match provider.decide(request()).await {
            Err(LlmError::Provider(m)) => assert!(m.contains("boom"), "message was {m}"),
            other => panic!("expected Provider error, got {other:?}"),
        }
    }

    #[test]
    fn mock_reports_400_max_questions() {
        let provider = MockDecisionProvider::new("mock", config(1.0, vec![], None));
        assert_eq!(provider.capabilities(), DecisionCapabilities::new(400));
    }

    #[tokio::test]
    async fn mock_output_satisfies_correlate() {
        use crate::decision::validate::{RawAnswer, RawNamedAnswer, correlate};

        let provider = MockDecisionProvider::new("fake", config(0.25, vec![], None));
        let response = provider.decide(request()).await.expect("mock decides");
        let raw: Vec<RawNamedAnswer> = response
            .answers
            .into_iter()
            .map(|named| RawNamedAnswer {
                name: named.name,
                answer: match named.answer {
                    DecisionAnswer::Predicate { probability } => {
                        RawAnswer::Predicate { probability }
                    }
                    DecisionAnswer::Choice {
                        choice,
                        confidence,
                        probabilities,
                    } => RawAnswer::Choice {
                        choice,
                        confidence: Some(confidence),
                        probabilities,
                    },
                    DecisionAnswer::Score {
                        score,
                        confidence,
                        probabilities,
                    } => RawAnswer::Score {
                        score,
                        confidence: Some(confidence),
                        probabilities,
                    },
                    _ => RawAnswer::Refusal,
                },
            })
            .collect();
        correlate(&triage_questions(), raw).expect("mock output must pass correlate");
    }
}
