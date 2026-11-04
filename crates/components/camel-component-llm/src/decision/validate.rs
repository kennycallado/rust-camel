//! Shared answer correlation and validation for decision codecs.
//!
//! Codecs decode wire answers into [`RawNamedAnswer`] values; [`correlate`]
//! matches them to the declared questions by name and checks every value.

use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::Hash;

use crate::decision::ChoiceOption;
use crate::decision::ChoiceProbability;
use crate::decision::DecisionAnswer;
use crate::decision::DecisionQuestion;
use crate::decision::NamedAnswer;
use crate::decision::QuestionKind;
use crate::decision::ScoreProbability;
use crate::error::LlmError;

/// Tolerance applied to the `[0, 1]` bounds and the score range.
const BOUND_TOLERANCE: f64 = 1e-9;

/// Maximum distance a distribution sum may have from 1.
const SUM_TOLERANCE: f64 = 0.01;

/// An answer as decoded from the wire, before validation against a question.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RawAnswer {
    /// Predicate probability as received.
    Predicate {
        /// Probability that the predicate holds.
        probability: f64,
    },
    /// Choice answer as received.
    Choice {
        /// Value the model picked.
        choice: String,
        /// Confidence in the pick, if the wire carried one.
        confidence: Option<f64>,
        /// Per-option probabilities, in wire order.
        probabilities: Vec<ChoiceProbability>,
    },
    /// Score answer as received.
    Score {
        /// Selected score value.
        score: f64,
        /// Confidence in the score, if the wire carried one.
        confidence: Option<f64>,
        /// Per-level probabilities, in wire order.
        probabilities: Vec<ScoreProbability>,
    },
    /// The model declined to answer.
    Refusal,
}

/// A decoded answer paired with the name it was sent under.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RawNamedAnswer {
    /// Question name the wire answer carries.
    pub(crate) name: String,
    /// The decoded answer body.
    pub(crate) answer: RawAnswer,
}

/// Matches `raw` answers to `questions` by name and validates each one.
///
/// Returns one [`NamedAnswer`] per question, in question order.
pub(crate) fn correlate(
    questions: &[DecisionQuestion],
    raw: Vec<RawNamedAnswer>,
) -> Result<Vec<NamedAnswer>, LlmError> {
    let mut by_name = index_by_name(questions, raw)?;
    questions
        .iter()
        .map(|question| {
            let answer = by_name.remove(question.name.as_str()).ok_or_else(|| {
                LlmError::protocol(format!("missing answer for '{}'", question.name))
            })?;
            Ok(NamedAnswer {
                name: question.name.clone(),
                answer: validate_answer(question, answer)?,
            })
        })
        .collect()
}

/// Indexes raw answers by name, rejecting undeclared and repeated names.
fn index_by_name(
    questions: &[DecisionQuestion],
    raw: Vec<RawNamedAnswer>,
) -> Result<HashMap<String, RawAnswer>, LlmError> {
    let declared: HashSet<&str> = questions.iter().map(|q| q.name.as_str()).collect();
    let mut by_name = HashMap::with_capacity(raw.len());
    for item in raw {
        if !declared.contains(item.name.as_str()) {
            return Err(LlmError::protocol(format!(
                "unknown answer '{}'",
                item.name
            )));
        }
        if by_name.contains_key(&item.name) {
            return Err(LlmError::protocol(format!(
                "duplicate answer for '{}'",
                item.name
            )));
        }
        by_name.insert(item.name, item.answer);
    }
    Ok(by_name)
}

/// Checks one answer against the kind and bounds of its question.
fn validate_answer(
    question: &DecisionQuestion,
    answer: RawAnswer,
) -> Result<DecisionAnswer, LlmError> {
    let name = question.name.as_str();
    match (&question.kind, answer) {
        (_, RawAnswer::Refusal) => Ok(DecisionAnswer::Refusal),
        (QuestionKind::Predicate, RawAnswer::Predicate { probability }) => {
            check_unit(name, "probability", probability)?;
            Ok(DecisionAnswer::Predicate { probability })
        }
        (
            QuestionKind::Choice { choices },
            RawAnswer::Choice {
                choice,
                confidence,
                probabilities,
            },
        ) => validate_choice(name, choices, choice, confidence, probabilities),
        (
            QuestionKind::Score { levels },
            RawAnswer::Score {
                score,
                confidence,
                probabilities,
            },
        ) => validate_score(name, levels.len(), score, confidence, probabilities),
        _ => Err(LlmError::protocol(format!(
            "answer kind mismatch for '{name}'"
        ))),
    }
}

/// Validates a choice answer and returns its distribution in declared order.
fn validate_choice(
    name: &str,
    choices: &[ChoiceOption],
    choice: String,
    confidence: Option<f64>,
    probabilities: Vec<ChoiceProbability>,
) -> Result<DecisionAnswer, LlmError> {
    let confidence = require_confidence(name, confidence)?;
    check_unit(name, "confidence", confidence)?;
    for entry in &probabilities {
        check_unit(name, "probability", entry.probability)?;
    }
    if !choices.iter().any(|option| option.value == choice) {
        return Err(LlmError::protocol(format!(
            "undeclared choice '{choice}' for '{name}'"
        )));
    }
    let ordered = order_choice_distribution(name, choices, probabilities)?;
    check_sum(name, ordered.iter().map(|p| p.probability).sum::<f64>())?;
    Ok(DecisionAnswer::Choice {
        choice,
        confidence,
        probabilities: ordered,
    })
}

/// Validates a score answer and returns its distribution sorted by level.
fn validate_score(
    name: &str,
    level_count: usize,
    score: f64,
    confidence: Option<f64>,
    probabilities: Vec<ScoreProbability>,
) -> Result<DecisionAnswer, LlmError> {
    let confidence = require_confidence(name, confidence)?;
    check_unit(name, "confidence", confidence)?;
    check_score_range(name, score, level_count)?;
    for entry in &probabilities {
        check_unit(name, "probability", entry.probability)?;
    }
    let ordered = order_score_distribution(name, level_count, probabilities)?;
    check_sum(name, ordered.iter().map(|p| p.probability).sum::<f64>())?;
    Ok(DecisionAnswer::Score {
        score,
        confidence,
        probabilities: ordered,
    })
}

/// Reorders a choice distribution into declared order. Its values must be
/// unique and equal the declared set.
fn order_choice_distribution(
    name: &str,
    choices: &[ChoiceOption],
    probabilities: Vec<ChoiceProbability>,
) -> Result<Vec<ChoiceProbability>, LlmError> {
    let mismatch = || {
        LlmError::protocol(format!(
            "choice distribution for '{name}' does not match the declared set"
        ))
    };
    let mut by_value = index_distribution(
        name,
        probabilities.into_iter().map(|p| (p.value, p.probability)),
    )?;
    let mut ordered = Vec::with_capacity(choices.len());
    for option in choices {
        let probability = by_value
            .remove(option.value.as_str())
            .ok_or_else(mismatch)?;
        ordered.push(ChoiceProbability {
            value: option.value.clone(),
            probability,
        });
    }
    if !by_value.is_empty() {
        return Err(mismatch());
    }
    Ok(ordered)
}

/// Orders a score distribution by level. Its levels must be unique and equal
/// `0..level_count`.
fn order_score_distribution(
    name: &str,
    level_count: usize,
    probabilities: Vec<ScoreProbability>,
) -> Result<Vec<ScoreProbability>, LlmError> {
    let mismatch = || {
        LlmError::protocol(format!(
            "score distribution for '{name}' must cover levels 0..{level_count}"
        ))
    };
    let mut by_level = index_distribution(
        name,
        probabilities.into_iter().map(|p| (p.level, p.probability)),
    )?;
    let mut ordered = Vec::with_capacity(level_count);
    for level in 0..level_count {
        let probability = by_level.remove(&level).ok_or_else(mismatch)?;
        ordered.push(ScoreProbability { level, probability });
    }
    if !by_level.is_empty() {
        return Err(mismatch());
    }
    Ok(ordered)
}

/// Collects distribution entries by key, rejecting a repeated key.
fn index_distribution<K: Eq + Hash>(
    name: &str,
    entries: impl IntoIterator<Item = (K, f64)>,
) -> Result<HashMap<K, f64>, LlmError> {
    let mut by_key = HashMap::new();
    for (key, probability) in entries {
        if by_key.insert(key, probability).is_some() {
            return Err(LlmError::protocol(format!(
                "duplicate distribution entry for '{name}'"
            )));
        }
    }
    Ok(by_key)
}

/// Returns the confidence, failing when the wire omitted it.
fn require_confidence(name: &str, confidence: Option<f64>) -> Result<f64, LlmError> {
    confidence.ok_or_else(|| LlmError::protocol(format!("missing confidence for '{name}'")))
}

/// Checks that `score` is finite and within `[0, level_count - 1]`.
fn check_score_range(name: &str, score: f64, level_count: usize) -> Result<(), LlmError> {
    let max = level_count as f64 - 1.0 + BOUND_TOLERANCE;
    if score.is_finite() && (-BOUND_TOLERANCE..=max).contains(&score) {
        Ok(())
    } else {
        Err(LlmError::protocol(format!(
            "score out of range for '{name}'"
        )))
    }
}

/// Checks that `v` is finite and within `[0, 1]`. The value is never clamped.
fn check_unit(name: &str, field: &str, v: f64) -> Result<(), LlmError> {
    if v.is_finite() && (-BOUND_TOLERANCE..=1.0 + BOUND_TOLERANCE).contains(&v) {
        Ok(())
    } else {
        Err(LlmError::protocol(format!(
            "{field} out of range for '{name}'"
        )))
    }
}

/// Checks that a distribution sums to 1 within [`SUM_TOLERANCE`].
fn check_sum(name: &str, total: f64) -> Result<(), LlmError> {
    if (total - 1.0).abs() <= SUM_TOLERANCE {
        Ok(())
    } else {
        Err(LlmError::protocol(format!(
            "probabilities for '{name}' must sum to 1"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{RawAnswer, RawNamedAnswer, correlate};
    use crate::decision::test_support::triage_questions;
    use crate::decision::{ChoiceProbability, DecisionAnswer, NamedAnswer, ScoreProbability};
    use crate::error::LlmError;

    fn named(name: &str, answer: RawAnswer) -> RawNamedAnswer {
        RawNamedAnswer {
            name: name.to_string(),
            answer,
        }
    }

    fn cp(value: &str, probability: f64) -> ChoiceProbability {
        ChoiceProbability {
            value: value.to_string(),
            probability,
        }
    }

    fn sp(level: usize, probability: f64) -> ScoreProbability {
        ScoreProbability { level, probability }
    }

    fn dept(
        choice: &str,
        confidence: Option<f64>,
        probabilities: Vec<ChoiceProbability>,
    ) -> RawAnswer {
        RawAnswer::Choice {
            choice: choice.to_string(),
            confidence,
            probabilities,
        }
    }

    fn urgency(score: f64, probabilities: Vec<ScoreProbability>) -> RawAnswer {
        RawAnswer::Score {
            score,
            confidence: Some(0.17),
            probabilities,
        }
    }

    /// Valid answers for the triage set, in wire order.
    fn valid_raw() -> Vec<RawNamedAnswer> {
        vec![
            named("is_billing", RawAnswer::Predicate { probability: 0.97 }),
            named(
                "dept",
                dept(
                    "billing",
                    Some(0.99),
                    vec![cp("billing", 0.99), cp("tech", 0.01)],
                ),
            ),
            named(
                "urgency",
                urgency(0.96, vec![sp(0, 0.29), sp(1, 0.45), sp(2, 0.26)]),
            ),
        ]
    }

    /// Replaces the answer under `name` (or adds it when absent).
    fn set(mut raw: Vec<RawNamedAnswer>, name: &str, answer: RawAnswer) -> Vec<RawNamedAnswer> {
        raw.retain(|a| a.name != name);
        raw.push(named(name, answer));
        raw
    }

    /// Asserts a `Protocol` error and returns its message.
    fn assert_protocol(result: Result<Vec<NamedAnswer>, LlmError>) -> String {
        match result {
            Err(LlmError::Protocol(msg)) => msg,
            other => panic!("expected Protocol error, got {other:?}"),
        }
    }

    #[test]
    fn correlates_out_of_order_answers() {
        let questions = triage_questions();
        let mut raw = valid_raw();
        raw.reverse();
        let answers = correlate(&questions, raw).expect("reversed answers still correlate");
        let names: Vec<&str> = answers.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["is_billing", "dept", "urgency"]);
    }

    #[test]
    fn missing_answer_is_protocol() {
        let questions = triage_questions();
        let mut raw = valid_raw();
        raw.retain(|a| a.name != "urgency");
        let msg = assert_protocol(correlate(&questions, raw));
        assert!(
            msg.contains("urgency"),
            "message should name 'urgency': {msg}"
        );
    }

    #[test]
    fn duplicate_answer_is_protocol() {
        let questions = triage_questions();
        let mut raw = valid_raw();
        raw.push(named(
            "dept",
            dept("tech", Some(0.5), vec![cp("billing", 0.5), cp("tech", 0.5)]),
        ));
        assert_protocol(correlate(&questions, raw));
    }

    #[test]
    fn unknown_answer_is_protocol() {
        let questions = triage_questions();
        let raw = set(valid_raw(), "foo", RawAnswer::Refusal);
        let msg = assert_protocol(correlate(&questions, raw));
        assert!(msg.contains("foo"), "message should name 'foo': {msg}");
    }

    #[test]
    fn kind_mismatch_is_protocol() {
        let questions = triage_questions();
        let raw = set(
            valid_raw(),
            "dept",
            RawAnswer::Predicate { probability: 0.5 },
        );
        assert_protocol(correlate(&questions, raw));
    }

    #[test]
    fn refusal_accepted_for_any_kind() {
        let questions = triage_questions();
        let raw = set(valid_raw(), "dept", RawAnswer::Refusal);
        let answers = correlate(&questions, raw).expect("refusal is valid for a choice question");
        assert_eq!(answers[1].answer, DecisionAnswer::Refusal);
    }

    #[test]
    fn probability_out_of_range_is_protocol() {
        let questions = triage_questions();
        let raw = set(
            valid_raw(),
            "is_billing",
            RawAnswer::Predicate { probability: 1.5 },
        );
        assert_protocol(correlate(&questions, raw));
    }

    #[test]
    fn nan_probability_is_protocol() {
        let questions = triage_questions();
        let raw = set(
            valid_raw(),
            "is_billing",
            RawAnswer::Predicate {
                probability: f64::NAN,
            },
        );
        assert_protocol(correlate(&questions, raw));
    }

    #[test]
    fn undeclared_choice_is_protocol() {
        let questions = triage_questions();
        let raw = set(
            valid_raw(),
            "dept",
            dept(
                "sales",
                Some(0.99),
                vec![cp("billing", 0.99), cp("tech", 0.01)],
            ),
        );
        assert_protocol(correlate(&questions, raw));
    }

    #[test]
    fn choice_distribution_must_match_declared_set() {
        let questions = triage_questions();
        let raw = set(
            valid_raw(),
            "dept",
            dept("billing", Some(0.99), vec![cp("billing", 1.0)]),
        );
        assert_protocol(correlate(&questions, raw));
    }

    #[test]
    fn choice_distribution_duplicate_value_is_protocol() {
        let questions = triage_questions();
        let raw = set(
            valid_raw(),
            "dept",
            dept(
                "billing",
                Some(0.99),
                vec![cp("billing", 0.5), cp("billing", 0.5)],
            ),
        );
        assert_protocol(correlate(&questions, raw));
    }

    #[test]
    fn distribution_sum_off_is_protocol() {
        let questions = triage_questions();
        let raw = set(
            valid_raw(),
            "dept",
            dept(
                "billing",
                Some(0.99),
                vec![cp("billing", 0.5), cp("tech", 0.3)],
            ),
        );
        assert_protocol(correlate(&questions, raw));
    }

    #[test]
    fn missing_confidence_is_protocol() {
        let questions = triage_questions();
        let raw = set(
            valid_raw(),
            "dept",
            dept("billing", None, vec![cp("billing", 0.99), cp("tech", 0.01)]),
        );
        let msg = assert_protocol(correlate(&questions, raw));
        assert!(
            msg.contains("confidence"),
            "message should name 'confidence': {msg}"
        );
    }

    #[test]
    fn score_out_of_range_is_protocol() {
        let questions = triage_questions();
        let raw = set(
            valid_raw(),
            "urgency",
            urgency(2.5, vec![sp(0, 0.29), sp(1, 0.45), sp(2, 0.26)]),
        );
        assert_protocol(correlate(&questions, raw));
    }

    #[test]
    fn fractional_score_preserved() {
        let questions = triage_questions();
        let raw = set(
            valid_raw(),
            "urgency",
            urgency(0.964, vec![sp(2, 0.26), sp(0, 0.29), sp(1, 0.45)]),
        );
        let answers = correlate(&questions, raw).expect("fractional score is in range");
        match &answers[2].answer {
            DecisionAnswer::Score {
                score,
                probabilities,
                ..
            } => {
                assert_eq!(*score, 0.964);
                assert_eq!(*probabilities, vec![sp(0, 0.29), sp(1, 0.45), sp(2, 0.26)]);
            }
            other => panic!("urgency should be Score, got {other:?}"),
        }
    }

    #[test]
    fn choice_probabilities_in_declared_order() {
        let questions = triage_questions();
        let raw = set(
            valid_raw(),
            "dept",
            dept(
                "billing",
                Some(0.99),
                vec![cp("tech", 0.01), cp("billing", 0.99)],
            ),
        );
        let answers = correlate(&questions, raw).expect("reordered distribution is valid");
        match &answers[1].answer {
            DecisionAnswer::Choice { probabilities, .. } => {
                let values: Vec<&str> = probabilities.iter().map(|p| p.value.as_str()).collect();
                assert_eq!(values, ["billing", "tech"]);
            }
            other => panic!("dept should be Choice, got {other:?}"),
        }
    }

    #[test]
    fn boundary_values_not_clamped() {
        let questions = triage_questions();
        let raw = set(
            valid_raw(),
            "is_billing",
            RawAnswer::Predicate {
                probability: 1.0 + 5e-10,
            },
        );
        let answers = correlate(&questions, raw).expect("value within tolerance is accepted");
        assert_eq!(
            answers[0].answer,
            DecisionAnswer::Predicate {
                probability: 1.0 + 5e-10
            },
            "tolerated value must be stored unchanged"
        );
    }

    #[test]
    fn score_missing_confidence_is_protocol() {
        let answer = RawAnswer::Score {
            score: 0.96,
            confidence: None,
            probabilities: vec![sp(0, 0.29), sp(1, 0.45), sp(2, 0.26)],
        };
        let msg = assert_protocol(correlate(
            &triage_questions(),
            set(valid_raw(), "urgency", answer),
        ));
        assert!(msg.contains("confidence"), "{msg}");
    }

    #[test]
    fn score_incomplete_levels_is_protocol() {
        let raw = set(
            valid_raw(),
            "urgency",
            urgency(0.5, vec![sp(0, 0.5), sp(1, 0.5)]),
        );
        assert_protocol(correlate(&triage_questions(), raw));
    }

    #[test]
    fn score_duplicate_level_is_protocol() {
        let raw = set(
            valid_raw(),
            "urgency",
            urgency(
                0.5,
                vec![sp(0, 0.25), sp(0, 0.25), sp(1, 0.25), sp(2, 0.25)],
            ),
        );
        assert_protocol(correlate(&triage_questions(), raw));
    }

    #[test]
    fn score_unknown_level_is_protocol() {
        let raw = set(
            valid_raw(),
            "urgency",
            urgency(0.5, vec![sp(0, 0.3), sp(1, 0.3), sp(7, 0.4)]),
        );
        assert_protocol(correlate(&triage_questions(), raw));
    }

    #[test]
    fn score_sum_off_is_protocol() {
        let raw = set(
            valid_raw(),
            "urgency",
            urgency(0.5, vec![sp(0, 0.2), sp(1, 0.2), sp(2, 0.2)]),
        );
        assert_protocol(correlate(&triage_questions(), raw));
    }

    #[test]
    fn non_finite_distribution_entry_is_protocol() {
        let raw = set(
            valid_raw(),
            "dept",
            dept(
                "billing",
                Some(0.99),
                vec![cp("billing", f64::INFINITY), cp("tech", 0.01)],
            ),
        );
        assert_protocol(correlate(&triage_questions(), raw));
    }

    #[test]
    fn nan_confidence_is_protocol() {
        let raw = set(
            valid_raw(),
            "dept",
            dept(
                "billing",
                Some(f64::NAN),
                vec![cp("billing", 0.99), cp("tech", 0.01)],
            ),
        );
        assert_protocol(correlate(&triage_questions(), raw));
    }
}
