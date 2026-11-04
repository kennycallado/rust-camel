//! Configuration types for decision providers and their question sets.
//!
//! These types are deserialized from the `[decision_providers.*]` and
//! `[decision_question_sets.*]` tables of [`crate::config::LlmGlobalConfig`].
//! Question sets are converted to runtime [`DecisionQuestion`]s with
//! [`QuestionSetConfig::to_questions`], which callers use only after
//! validation.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use camel_api::redact;
use camel_component_api::CamelError;
use camel_component_api::NetworkRetryPolicy;

use crate::config::LlmGlobalConfig;
use crate::decision::ChoiceOption;
use crate::decision::DecisionQuestion;
use crate::decision::QuestionKind;
use crate::decision::ScoreLevel;

fn default_mock_decision_model() -> String {
    "mock-decision".into()
}

fn default_predicate_probability() -> f64 {
    1.0
}

/// Per-attempt HTTP timeout, in seconds, used by the adapters when neither the
/// provider nor the global configuration sets one.
#[cfg(feature = "decisions")]
pub(crate) const DEFAULT_ATTEMPT_TIMEOUT_SECS: u64 = 30;

/// Resolves the configured timeout in seconds. The provider value wins over the
/// global one; `None` means neither is set.
///
/// Adapters use it per attempt, as
/// `effective_timeout_secs(..).unwrap_or(DEFAULT_ATTEMPT_TIMEOUT_SECS)`. The
/// endpoint uses it as the optional total deadline of a call, where `None`
/// means no deadline.
pub(crate) fn effective_timeout_secs(provider: Option<u64>, global: Option<u64>) -> Option<u64> {
    provider.or(global)
}

/// Configuration for an OpenAI decision provider.
///
/// `Debug` redacts `api_key` and any credentials embedded in `base_url`.
#[derive(Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenaiDecisionConfig {
    /// API key for authentication.
    pub api_key: String,

    /// Model used when a decision request does not name one.
    pub default_model: String,

    /// Base URL override (defaults to the provider's standard endpoint).
    #[serde(default)]
    pub base_url: Option<String>,

    /// Optional per-provider timeout override (seconds).
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Optional max concurrency for this provider.
    #[serde(default)]
    pub max_concurrency: Option<usize>,

    /// Optional network retry policy for transient failures.
    #[serde(default)]
    pub network_retry: Option<NetworkRetryPolicy>,
}

impl fmt::Debug for OpenaiDecisionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenaiDecisionConfig")
            .field("api_key", &"[REDACTED]")
            .field("default_model", &self.default_model)
            .field(
                "base_url",
                &self.base_url.as_deref().map(redact::redact_url),
            )
            .field("timeout_secs", &self.timeout_secs)
            .field("max_concurrency", &self.max_concurrency)
            .field("network_retry", &self.network_retry)
            .finish()
    }
}

/// Configuration for a Typesafe decision provider.
///
/// Same fields as [`OpenaiDecisionConfig`]. `Debug` redacts `api_key` and
/// any credentials embedded in `base_url`.
#[derive(Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypesafeDecisionConfig {
    /// API key for authentication.
    pub api_key: String,

    /// Model used when a decision request does not name one.
    pub default_model: String,

    /// Base URL override (defaults to the provider's standard endpoint).
    #[serde(default)]
    pub base_url: Option<String>,

    /// Optional per-provider timeout override (seconds).
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Optional max concurrency for this provider.
    #[serde(default)]
    pub max_concurrency: Option<usize>,

    /// Optional network retry policy for transient failures.
    #[serde(default)]
    pub network_retry: Option<NetworkRetryPolicy>,
}

impl fmt::Debug for TypesafeDecisionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypesafeDecisionConfig")
            .field("api_key", &"[REDACTED]")
            .field("default_model", &self.default_model)
            .field(
                "base_url",
                &self.base_url.as_deref().map(redact::redact_url),
            )
            .field("timeout_secs", &self.timeout_secs)
            .field("max_concurrency", &self.max_concurrency)
            .field("network_retry", &self.network_retry)
            .finish()
    }
}

/// Configuration for an Ollama (local) decision provider. Has no `api_key`
/// field; supplying one is a parse error. `Debug` redacts any credentials
/// embedded in `base_url`.
#[derive(Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OllamaDecisionConfig {
    /// Base URL for the Ollama server (e.g., "http://localhost:11434").
    pub base_url: String,

    /// Model used when a decision request does not name one.
    pub default_model: String,

    /// Optional per-provider timeout override (seconds).
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Optional max concurrency for this provider.
    #[serde(default)]
    pub max_concurrency: Option<usize>,

    /// Optional network retry policy for transient failures.
    #[serde(default)]
    pub network_retry: Option<NetworkRetryPolicy>,
}

impl fmt::Debug for OllamaDecisionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OllamaDecisionConfig")
            .field("base_url", &redact::redact_url(&self.base_url))
            .field("default_model", &self.default_model)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_concurrency", &self.max_concurrency)
            .field("network_retry", &self.network_retry)
            .finish()
    }
}

/// Configuration for the mock decision provider used in tests.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MockDecisionConfig {
    /// Model identifier reported by the mock.
    #[serde(default = "default_mock_decision_model")]
    pub default_model: String,

    /// Probability returned for every predicate question.
    #[serde(default = "default_predicate_probability")]
    pub predicate_probability: f64,

    /// Question names for which the mock answers with a refusal.
    #[serde(default)]
    pub refuse: Vec<String>,

    /// Optional error message to simulate provider failure.
    #[serde(default)]
    pub error_message: Option<String>,
}

/// Configuration for a decision provider, discriminated by the `type` field.
///
/// `#[non_exhaustive]`: new provider variants are additive, so downstream
/// matches must carry a wildcard arm.
#[derive(Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
#[non_exhaustive]
pub enum DecisionProviderConfig {
    /// OpenAI-compatible decision provider.
    Openai(OpenaiDecisionConfig),
    /// Typesafe decision provider.
    Typesafe(TypesafeDecisionConfig),
    /// Ollama (local) decision provider.
    Ollama(OllamaDecisionConfig),
    /// Mock decision provider for testing.
    Mock(MockDecisionConfig),
}

impl fmt::Debug for DecisionProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecisionProviderConfig::Openai(c) => f.debug_tuple("Openai").field(c).finish(),
            DecisionProviderConfig::Typesafe(c) => f.debug_tuple("Typesafe").field(c).finish(),
            DecisionProviderConfig::Ollama(c) => f.debug_tuple("Ollama").field(c).finish(),
            DecisionProviderConfig::Mock(c) => f.debug_tuple("Mock").field(c).finish(),
        }
    }
}

impl DecisionProviderConfig {
    /// Per-provider timeout in seconds, or `None` when unset or unsupported (Mock).
    pub fn timeout_secs(&self) -> Option<u64> {
        match self {
            DecisionProviderConfig::Openai(c) => c.timeout_secs,
            DecisionProviderConfig::Typesafe(c) => c.timeout_secs,
            DecisionProviderConfig::Ollama(c) => c.timeout_secs,
            DecisionProviderConfig::Mock(_) => None,
        }
    }

    /// Per-provider max concurrency, or `None` when unset or unsupported (Mock).
    pub fn max_concurrency(&self) -> Option<usize> {
        match self {
            DecisionProviderConfig::Openai(c) => c.max_concurrency,
            DecisionProviderConfig::Typesafe(c) => c.max_concurrency,
            DecisionProviderConfig::Ollama(c) => c.max_concurrency,
            DecisionProviderConfig::Mock(_) => None,
        }
    }

    /// Network retry policy, or `None` when unset or unsupported (Mock).
    pub fn network_retry(&self) -> Option<NetworkRetryPolicy> {
        match self {
            DecisionProviderConfig::Openai(c) => c.network_retry.clone(),
            DecisionProviderConfig::Typesafe(c) => c.network_retry.clone(),
            DecisionProviderConfig::Ollama(c) => c.network_retry.clone(),
            DecisionProviderConfig::Mock(_) => None,
        }
    }

    /// Stable name of the provider type: `"openai"`, `"typesafe"`,
    /// `"ollama"`, or `"mock"`.
    pub fn type_name(&self) -> &'static str {
        match self {
            DecisionProviderConfig::Openai(_) => "openai",
            DecisionProviderConfig::Typesafe(_) => "typesafe",
            DecisionProviderConfig::Ollama(_) => "ollama",
            DecisionProviderConfig::Mock(_) => "mock",
        }
    }
}

/// The answer shape a configured question expects, as written in TOML.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum QuestionType {
    /// Yes/no question answered with a probability.
    Predicate,
    /// Pick one of the declared choices.
    Choice,
    /// Pick one of the ordered score levels.
    Score,
}

/// One question in a decision question set, as written in TOML.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionConfig {
    /// Name used to correlate the question with its answer.
    pub name: String,

    /// The answer shape the question expects.
    #[serde(rename = "type")]
    pub kind: QuestionType,

    /// Natural-language instructions shown to the model.
    pub instructions: String,

    /// Options for a `choice` question.
    #[serde(default)]
    pub choices: Option<Vec<ChoiceOption>>,

    /// Ordered levels for a `score` question.
    #[serde(default)]
    pub levels: Option<Vec<ScoreLevel>>,

    /// Probability threshold at which a predicate counts as true.
    #[serde(default)]
    pub threshold: Option<f64>,

    /// Minimum confidence required before an answer is accepted.
    #[serde(default)]
    pub min_confidence: Option<f64>,
}

/// A named decision question set, as written in TOML.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionSetConfig {
    /// Questions in the order they are asked and answered.
    pub questions: Vec<QuestionConfig>,
}

impl QuestionSetConfig {
    /// Converts the configured questions into runtime questions, preserving
    /// declared order. Missing `choices` or `levels` become empty lists.
    ///
    /// Call only after configuration validation.
    pub fn to_questions(&self) -> Arc<[DecisionQuestion]> {
        self.questions
            .iter()
            .map(|q| DecisionQuestion {
                name: q.name.clone(),
                instructions: q.instructions.clone(),
                kind: match q.kind {
                    QuestionType::Predicate => QuestionKind::Predicate,
                    QuestionType::Choice => QuestionKind::Choice {
                        choices: q.choices.clone().unwrap_or_default(),
                    },
                    QuestionType::Score => QuestionKind::Score {
                        levels: q.levels.clone().unwrap_or_default(),
                    },
                },
            })
            .collect()
    }
}

/// Returns true when `s` is a valid question name: 1 to 64 characters, each an
/// ASCII alphanumeric, `_`, or `-`.
pub(crate) fn is_valid_question_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Validates every question set, decision provider, and the default decision
/// provider reference in `global`.
///
/// Sets and providers are visited in name order so the reported error is
/// stable across runs.
pub(crate) fn validate_decision_config(global: &LlmGlobalConfig) -> Result<(), CamelError> {
    let mut sets: Vec<(&String, &QuestionSetConfig)> =
        global.decision_question_sets.iter().collect();
    sets.sort_by(|a, b| a.0.cmp(b.0));
    for (set, cfg) in sets {
        validate_question_set(set, cfg)?;
    }

    let mut providers: Vec<(&String, &DecisionProviderConfig)> =
        global.decision_providers.iter().collect();
    providers.sort_by(|a, b| a.0.cmp(b.0));
    for (name, provider) in providers {
        validate_provider(name, provider)?;
    }

    validate_default_provider(global)
}

/// Validates one question set: its size, then each question in declared order.
fn validate_question_set(set: &str, cfg: &QuestionSetConfig) -> Result<(), CamelError> {
    let count = cfg.questions.len();
    if !(1..=400).contains(&count) {
        return Err(CamelError::Config(format!(
            "question set '{set}' must have 1..=400 questions (got {count})"
        )));
    }

    let mut seen: HashSet<&str> = HashSet::new();
    for q in &cfg.questions {
        if !is_valid_question_name(&q.name) {
            return Err(CamelError::Config(format!(
                "question set '{set}': invalid question name '{}'",
                q.name
            )));
        }
        if !seen.insert(q.name.as_str()) {
            return Err(CamelError::Config(format!(
                "question set '{set}': duplicate question name '{}'",
                q.name
            )));
        }
        validate_question(set, q)?;
    }
    Ok(())
}

/// Validates one question: instructions, field applicability, kind-specific
/// options, and the unit-interval fields.
fn validate_question(set: &str, q: &QuestionConfig) -> Result<(), CamelError> {
    if q.instructions.trim().is_empty() {
        return Err(CamelError::Config(format!(
            "question set '{set}': question '{}' has blank instructions",
            q.name
        )));
    }

    validate_field_applicability(set, q)?;

    match q.kind {
        QuestionType::Choice => validate_choices(set, q)?,
        QuestionType::Score => validate_levels(set, q)?,
        QuestionType::Predicate => {}
    }

    validate_unit_field(set, q, "threshold", q.threshold)?;
    validate_unit_field(set, q, "min_confidence", q.min_confidence)
}

/// Rejects a field that the question's kind does not accept.
fn validate_field_applicability(set: &str, q: &QuestionConfig) -> Result<(), CamelError> {
    let kind = q.kind;
    let problem = if q.choices.is_some() && kind != QuestionType::Choice {
        Some("choices are only valid on choice questions")
    } else if q.levels.is_some() && kind != QuestionType::Score {
        Some("levels are only valid on score questions")
    } else if q.threshold.is_some() && kind != QuestionType::Predicate {
        Some("threshold is only valid on predicate questions")
    } else if q.min_confidence.is_some() && kind == QuestionType::Predicate {
        Some("min_confidence is only valid on choice or score questions")
    } else {
        None
    };

    if let Some(detail) = problem {
        return Err(CamelError::Config(format!(
            "question set '{set}': question '{}': {detail}",
            q.name
        )));
    }
    Ok(())
}

/// Validates the options of a choice question: 2..=255 entries, each with a
/// non-blank and unique value.
fn validate_choices(set: &str, q: &QuestionConfig) -> Result<(), CamelError> {
    let choices: &[ChoiceOption] = q.choices.as_deref().unwrap_or(&[]);
    if !(2..=255).contains(&choices.len()) {
        return Err(CamelError::Config(format!(
            "question set '{set}': choice '{}' needs 2..=255 choices",
            q.name
        )));
    }

    let mut seen: HashSet<&str> = HashSet::new();
    for choice in choices {
        if choice.value.trim().is_empty() {
            return Err(CamelError::Config(format!(
                "question set '{set}': choice '{}' has blank choice value",
                q.name
            )));
        }
        if !seen.insert(choice.value.as_str()) {
            return Err(CamelError::Config(format!(
                "question set '{set}': choice '{}' has duplicate choice value '{}'",
                q.name, choice.value
            )));
        }
    }
    Ok(())
}

/// Validates the levels of a score question: 2..=10 entries, each with a
/// non-blank label.
fn validate_levels(set: &str, q: &QuestionConfig) -> Result<(), CamelError> {
    let levels: &[ScoreLevel] = q.levels.as_deref().unwrap_or(&[]);
    if !(2..=10).contains(&levels.len()) {
        return Err(CamelError::Config(format!(
            "question set '{set}': score '{}' needs 2..=10 levels",
            q.name
        )));
    }

    for level in levels {
        if level.label.trim().is_empty() {
            return Err(CamelError::Config(format!(
                "question set '{set}': score '{}' has blank level label",
                q.name
            )));
        }
    }
    Ok(())
}

/// Rejects a `threshold` or `min_confidence` value outside `[0, 1]`. The
/// range test also rejects NaN and infinities.
fn validate_unit_field(
    set: &str,
    q: &QuestionConfig,
    field: &str,
    value: Option<f64>,
) -> Result<(), CamelError> {
    match value {
        Some(v) if !(0.0..=1.0).contains(&v) => Err(CamelError::Config(format!(
            "question set '{set}': question '{}': {field} must be within [0, 1]",
            q.name
        ))),
        _ => Ok(()),
    }
}

/// Validates one decision provider entry, dispatching on its variant.
fn validate_provider(name: &str, provider: &DecisionProviderConfig) -> Result<(), CamelError> {
    match provider {
        DecisionProviderConfig::Openai(c) => validate_hosted(
            name,
            &c.api_key,
            c.base_url.as_deref(),
            &c.default_model,
            c.timeout_secs,
            c.max_concurrency,
        ),
        DecisionProviderConfig::Typesafe(c) => validate_hosted(
            name,
            &c.api_key,
            c.base_url.as_deref(),
            &c.default_model,
            c.timeout_secs,
            c.max_concurrency,
        ),
        DecisionProviderConfig::Ollama(c) => {
            require_non_blank(name, "base_url", &c.base_url)?;
            require_non_blank(name, "default_model", &c.default_model)?;
            validate_limits(name, c.timeout_secs, c.max_concurrency)
        }
        DecisionProviderConfig::Mock(c) => validate_mock(name, c),
    }
}

/// Validates an OpenAI or Typesafe entry: credentials, optional base URL,
/// default model, and policy limits.
fn validate_hosted(
    provider: &str,
    api_key: &str,
    base_url: Option<&str>,
    default_model: &str,
    timeout_secs: Option<u64>,
    max_concurrency: Option<usize>,
) -> Result<(), CamelError> {
    require_non_blank(provider, "api_key", api_key)?;
    if let Some(url) = base_url {
        require_non_blank(provider, "base_url", url)?;
    }
    require_non_blank(provider, "default_model", default_model)?;
    validate_limits(provider, timeout_secs, max_concurrency)
}

/// Validates a mock entry: the predicate probability and the refuse names.
fn validate_mock(provider: &str, c: &MockDecisionConfig) -> Result<(), CamelError> {
    if !(0.0..=1.0).contains(&c.predicate_probability) {
        return Err(CamelError::Config(format!(
            "decision provider '{provider}' predicate_probability must be within [0, 1]"
        )));
    }
    for name in &c.refuse {
        if !is_valid_question_name(name) {
            return Err(CamelError::Config(format!(
                "decision provider '{provider}': invalid refuse entry '{name}'"
            )));
        }
    }
    Ok(())
}

/// Rejects a required string field that is empty or whitespace only.
fn require_non_blank(provider: &str, field: &str, value: &str) -> Result<(), CamelError> {
    if value.trim().is_empty() {
        return Err(CamelError::Config(format!(
            "decision provider '{provider}' {field} must not be blank"
        )));
    }
    Ok(())
}

/// Rejects a `timeout_secs` or `max_concurrency` set to zero.
fn validate_limits(
    provider: &str,
    timeout_secs: Option<u64>,
    max_concurrency: Option<usize>,
) -> Result<(), CamelError> {
    if timeout_secs == Some(0) {
        return Err(CamelError::Config(format!(
            "decision provider '{provider}' timeout_secs must be > 0 when set (got 0)"
        )));
    }
    if max_concurrency == Some(0) {
        return Err(CamelError::Config(format!(
            "decision provider '{provider}' max_concurrency must be > 0 when set (got 0)"
        )));
    }
    Ok(())
}

/// Rejects a `default_decision_provider` that is not a configured provider.
fn validate_default_provider(global: &LlmGlobalConfig) -> Result<(), CamelError> {
    match &global.default_decision_provider {
        Some(p) if !global.decision_providers.contains_key(p) => Err(CamelError::Config(format!(
            "default_decision_provider '{p}' is not configured"
        ))),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;

#[cfg(test)]
mod effective_timeout_tests {
    use super::effective_timeout_secs;

    #[test]
    fn effective_timeout_prefers_provider() {
        assert_eq!(effective_timeout_secs(Some(5), Some(60)), Some(5));
        assert_eq!(effective_timeout_secs(None, Some(60)), Some(60));
        assert_eq!(effective_timeout_secs(None, None), None);
    }
}
