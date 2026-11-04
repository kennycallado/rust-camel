use std::collections::HashMap;
use std::fmt;

use camel_component_api::CamelError;
use camel_component_api::NetworkRetryPolicy;

use crate::cost::PricingTable;
use crate::decision::config::DecisionProviderConfig;
use crate::decision::config::QuestionSetConfig;

fn default_max_prompt_bytes() -> usize {
    32768
}

pub(crate) fn default_max_header_json_bytes() -> usize {
    65_536
}

fn default_mock_response() -> String {
    "echo".into()
}

fn default_mock_model() -> String {
    "mock-model".into()
}

/// Global LLM configuration, typically deserialized from TOML.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct LlmGlobalConfig {
    /// Default provider name to use when none is specified.
    #[serde(default)]
    pub default_provider: Option<String>,

    /// Default timeout in seconds for LLM operations. `None` means no timeout.
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Maximum prompt size in bytes before truncation or rejection.
    #[serde(default = "default_max_prompt_bytes")]
    pub max_prompt_bytes: usize,

    /// Maximum serialized size (in bytes) of any JSON-bearing exchange
    /// header (`CamelLlmMessages`, `CamelLlmTools`, `CamelLlmToolChoice`).
    /// The producer rejects headers above this threshold before
    /// deserializing, guarding against DoS via oversized payloads.
    #[serde(default = "default_max_header_json_bytes")]
    pub max_header_json_bytes: usize,

    /// Map of provider name to provider configuration.
    #[serde(default)]
    pub providers: HashMap<String, LlmProviderConfig>,

    /// Allow internal/private/loopback addresses and HTTP for local development.
    #[serde(default)]
    pub allow_internal: bool,

    /// Default decision provider name to use when none is specified.
    #[serde(default)]
    pub default_decision_provider: Option<String>,

    /// Map of decision provider name to decision provider configuration.
    #[serde(default)]
    pub decision_providers: HashMap<String, DecisionProviderConfig>,

    /// Map of question set name to decision question set configuration.
    #[serde(default)]
    pub decision_question_sets: HashMap<String, QuestionSetConfig>,
}

impl Default for LlmGlobalConfig {
    fn default() -> Self {
        Self {
            default_provider: None,
            timeout_secs: None,
            max_prompt_bytes: default_max_prompt_bytes(),
            max_header_json_bytes: default_max_header_json_bytes(),
            providers: HashMap::new(),
            allow_internal: false,
            default_decision_provider: None,
            decision_providers: HashMap::new(),
            decision_question_sets: HashMap::new(),
        }
    }
}

/// Native LLM service targeted by a generic provider entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmServiceKind {
    /// Anthropic Messages API.
    Anthropic,
    /// Gemini generative language API.
    Gemini,
}

/// Configuration for a generic provider selected by service.
///
/// Credentials are explicit: deserialization and the adapter never discover
/// them from the environment.
#[derive(Clone, serde::Deserialize)]
pub struct GenericProviderConfig {
    /// Native service protocol to target.
    pub service: LlmServiceKind,

    /// API key for the selected service.
    pub api_key: String,

    /// Default model to use (e.g., "claude-sonnet-4-0").
    pub default_model: String,

    /// Base URL override (defaults to the service's standard endpoint).
    #[serde(default)]
    pub base_url: Option<String>,

    /// Optional per-provider timeout override (seconds).
    /// Overrides global `timeout_secs` when set.
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Optional max concurrency for this provider.
    #[serde(default)]
    pub max_concurrency: Option<usize>,

    /// Optional network retry policy for transient failures.
    #[serde(default)]
    pub network_retry: Option<NetworkRetryPolicy>,

    /// Optional pricing table for cost estimation.
    #[serde(default)]
    pub pricing: Option<PricingTable>,

    /// Optional response cache TTL in seconds (materialized-only).
    /// Absent = cache disabled for this provider.
    #[serde(default)]
    pub cache_ttl_secs: Option<u64>,

    /// Optional maximum cache entries (LRU eviction boundary).
    /// When set, oldest entries are evicted when the cache exceeds this
    /// size. When absent, the cache is unbounded.
    #[serde(default)]
    pub cache_max_entries: Option<usize>,
}

impl GenericProviderConfig {
    /// Reject credentials or default models that contain only whitespace.
    ///
    /// Nonblank values are stored and returned verbatim; they are never
    /// trimmed.
    pub fn validate_required_fields(&self) -> Result<(), &'static str> {
        if self.api_key.trim().is_empty() {
            return Err("api_key must not be blank");
        }
        if self.default_model.trim().is_empty() {
            return Err("default_model must not be blank");
        }
        Ok(())
    }
}

fn redacted_optional_url(url: &Option<String>) -> Option<String> {
    url.as_deref().map(camel_api::redact::redact_url)
}

impl fmt::Debug for GenericProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GenericProviderConfig")
            .field("service", &self.service)
            .field("api_key", &"[REDACTED]")
            .field("default_model", &self.default_model)
            .field("base_url", &redacted_optional_url(&self.base_url))
            .field("timeout_secs", &self.timeout_secs)
            .field("max_concurrency", &self.max_concurrency)
            .field("network_retry", &self.network_retry)
            .field("pricing", &self.pricing)
            .field("cache_ttl_secs", &self.cache_ttl_secs)
            .field("cache_max_entries", &self.cache_max_entries)
            .finish()
    }
}

/// Configuration for a single LLM provider, discriminated by `type` field.
///
/// `#[non_exhaustive]`: new provider variants are additive, so downstream
/// matches must carry a wildcard arm.
#[derive(Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
#[non_exhaustive]
pub enum LlmProviderConfig {
    /// OpenAI-compatible provider (also Azure OpenAI).
    Openai(OpenaiProviderConfig),
    /// Ollama (local) provider.
    Ollama(OllamaProviderConfig),
    /// Generic provider selected by native service.
    Generic(GenericProviderConfig),
    /// Mock provider for testing.
    Mock(MockProviderConfig),
}

impl LlmProviderConfig {
    /// Extract the `max_concurrency` from whichever provider variant,
    /// returning `None` if the variant doesn't support it (e.g. Mock).
    pub fn max_concurrency(&self) -> Option<usize> {
        match self {
            LlmProviderConfig::Openai(c) => c.max_concurrency,
            LlmProviderConfig::Ollama(c) => c.max_concurrency,
            LlmProviderConfig::Generic(c) => c.max_concurrency,
            LlmProviderConfig::Mock(_) => None,
        }
    }

    /// Extract the `timeout_secs` from whichever provider variant,
    /// returning `None` if the variant doesn't support it (e.g. Mock).
    pub fn timeout_secs(&self) -> Option<u64> {
        match self {
            LlmProviderConfig::Openai(c) => c.timeout_secs,
            LlmProviderConfig::Ollama(c) => c.timeout_secs,
            LlmProviderConfig::Generic(c) => c.timeout_secs,
            LlmProviderConfig::Mock(_) => None,
        }
    }

    /// Extract the `network_retry` from whichever provider variant,
    /// returning `None` if the variant doesn't support it (e.g. Mock).
    pub fn network_retry(&self) -> Option<NetworkRetryPolicy> {
        match self {
            LlmProviderConfig::Openai(c) => c.network_retry.clone(),
            LlmProviderConfig::Ollama(c) => c.network_retry.clone(),
            LlmProviderConfig::Generic(c) => c.network_retry.clone(),
            LlmProviderConfig::Mock(_) => None,
        }
    }

    /// Extract the `pricing` from whichever provider variant,
    /// returning `None` if the variant doesn't support it (e.g. Mock).
    pub fn pricing(&self) -> Option<PricingTable> {
        match self {
            LlmProviderConfig::Openai(c) => c.pricing.clone(),
            LlmProviderConfig::Ollama(c) => c.pricing.clone(),
            LlmProviderConfig::Generic(c) => c.pricing.clone(),
            LlmProviderConfig::Mock(_) => None,
        }
    }

    /// Extract the `cache_ttl_secs` from whichever provider variant,
    /// returning `None` if the variant doesn't support it (e.g. Mock).
    pub fn cache_ttl_secs(&self) -> Option<u64> {
        match self {
            LlmProviderConfig::Openai(c) => c.cache_ttl_secs,
            LlmProviderConfig::Ollama(c) => c.cache_ttl_secs,
            LlmProviderConfig::Generic(c) => c.cache_ttl_secs,
            LlmProviderConfig::Mock(_) => None,
        }
    }

    /// Extract the `cache_max_entries` from whichever provider variant,
    /// returning `None` if the variant doesn't support it (e.g. Mock).
    pub fn cache_max_entries(&self) -> Option<usize> {
        match self {
            LlmProviderConfig::Openai(c) => c.cache_max_entries,
            LlmProviderConfig::Ollama(c) => c.cache_max_entries,
            LlmProviderConfig::Generic(c) => c.cache_max_entries,
            LlmProviderConfig::Mock(_) => None,
        }
    }
}

impl fmt::Debug for LlmProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmProviderConfig::Openai(c) => f
                .debug_struct("Openai")
                .field("api_key", &"[REDACTED]")
                .field("base_url", &c.base_url)
                .field("default_model", &c.default_model)
                .field("timeout_secs", &c.timeout_secs)
                .field("max_concurrency", &c.max_concurrency)
                .field("network_retry", &c.network_retry)
                .field("pricing", &c.pricing)
                .field("cache_ttl_secs", &c.cache_ttl_secs)
                .field("cache_max_entries", &c.cache_max_entries)
                .finish(),
            LlmProviderConfig::Ollama(c) => f
                .debug_struct("Ollama")
                .field("base_url", &c.base_url)
                .field("default_model", &c.default_model)
                .field("timeout_secs", &c.timeout_secs)
                .field("max_concurrency", &c.max_concurrency)
                .field("network_retry", &c.network_retry)
                .field("pricing", &c.pricing)
                .field("cache_ttl_secs", &c.cache_ttl_secs)
                .field("cache_max_entries", &c.cache_max_entries)
                .finish(),
            LlmProviderConfig::Generic(c) => f
                .debug_struct("Generic")
                .field("service", &c.service)
                .field("api_key", &"[REDACTED]")
                .field("default_model", &c.default_model)
                .field("base_url", &redacted_optional_url(&c.base_url))
                .field("timeout_secs", &c.timeout_secs)
                .field("max_concurrency", &c.max_concurrency)
                .field("network_retry", &c.network_retry)
                .field("pricing", &c.pricing)
                .field("cache_ttl_secs", &c.cache_ttl_secs)
                .field("cache_max_entries", &c.cache_max_entries)
                .finish(),
            LlmProviderConfig::Mock(c) => f
                .debug_struct("Mock")
                .field("response", &c.response)
                .field("default_model", &c.default_model)
                .field("error_message", &c.error_message)
                .finish(),
        }
    }
}

/// Configuration for an OpenAI-compatible provider.
#[derive(Clone, serde::Deserialize)]
pub struct OpenaiProviderConfig {
    /// API key for authentication.
    pub api_key: String,

    /// Base URL override (defaults to provider's standard endpoint).
    #[serde(default)]
    pub base_url: Option<String>,

    /// Default model to use (e.g., "gpt-4o").
    pub default_model: String,

    /// Optional per-provider timeout override (seconds).
    /// Overrides global `timeout_secs` when set.
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Optional max concurrency for this provider.
    #[serde(default)]
    pub max_concurrency: Option<usize>,

    /// Optional network retry policy for transient failures.
    #[serde(default)]
    pub network_retry: Option<NetworkRetryPolicy>,

    /// Optional pricing table for cost estimation.
    #[serde(default)]
    pub pricing: Option<PricingTable>,

    /// Optional response cache TTL in seconds (materialized-only).
    /// Absent = cache disabled for this provider.
    #[serde(default)]
    pub cache_ttl_secs: Option<u64>,

    /// Optional maximum cache entries (LRU eviction boundary).
    /// When set, oldest entries are evicted when the cache exceeds this
    /// size. When absent, the cache is unbounded.
    #[serde(default)]
    pub cache_max_entries: Option<usize>,
}

/// Configuration for an Ollama (local) provider.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct OllamaProviderConfig {
    /// Base URL for the Ollama server (e.g., "http://localhost:11434").
    pub base_url: String,

    /// Default model to use (e.g., "llama3").
    pub default_model: String,

    /// Optional per-provider timeout override (seconds).
    /// Overrides global `timeout_secs` when set.
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Optional max concurrency for this provider.
    #[serde(default)]
    pub max_concurrency: Option<usize>,

    /// Optional network retry policy for transient failures.
    #[serde(default)]
    pub network_retry: Option<NetworkRetryPolicy>,

    /// Optional pricing table for cost estimation.
    #[serde(default)]
    pub pricing: Option<PricingTable>,

    /// Optional response cache TTL in seconds (materialized-only).
    /// Absent = cache disabled for this provider.
    #[serde(default)]
    pub cache_ttl_secs: Option<u64>,

    /// Optional maximum cache entries (LRU eviction boundary).
    /// When set, oldest entries are evicted when the cache exceeds this
    /// size. When absent, the cache is unbounded.
    #[serde(default)]
    pub cache_max_entries: Option<usize>,
}

/// Configuration for the mock testing provider.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct MockProviderConfig {
    /// Response mode ("echo" or custom text).
    #[serde(default = "default_mock_response")]
    pub response: String,

    /// Default model identifier.
    #[serde(default = "default_mock_model")]
    pub default_model: String,

    /// Optional error message to simulate provider failure.
    #[serde(default)]
    pub error_message: Option<String>,
}

/// The type of LLM operation to perform.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LlmOperation {
    /// Chat completion (streaming or non-streaming).
    Chat,
    /// Text embedding generation.
    Embed,
    /// Structured decision over a named question set (`llm:decide?questions=...`).
    /// Handled by `DecisionProducer`, not by `LlmProducer`.
    Decide,
}

impl LlmGlobalConfig {
    /// Validate the configuration, rejecting `Some(0)` for `timeout_secs`,
    /// `Some(0)` for `max_concurrency`, `Some(0)` for `cache_ttl_secs`, and
    /// `Some(0)` for `cache_max_entries` on both global and per-provider
    /// levels.
    pub fn validate(&self) -> Result<(), CamelError> {
        // Global timeout_secs: Some(0) is a misconfiguration
        if self.timeout_secs == Some(0) {
            return Err(CamelError::Config(
                "global timeout_secs must be > 0 when set (got 0)".into(),
            ));
        }

        for (name, provider) in &self.providers {
            // Generic entries require nonblank credentials and default models;
            // reject them before any limit/pricing checks so the diagnostic
            // names the missing content rather than an unrelated field.
            if let LlmProviderConfig::Generic(c) = provider {
                c.validate_required_fields()
                    .map_err(|detail| CamelError::Config(format!("provider '{name}' {detail}")))?;
            }

            // Provider-level timeout_secs: Some(0) is invalid
            if provider.timeout_secs() == Some(0) {
                return Err(CamelError::Config(format!(
                    "provider '{name}' timeout_secs must be > 0 when set (got 0)"
                )));
            }

            // Provider-level max_concurrency: Some(0) is invalid
            if provider.max_concurrency() == Some(0) {
                return Err(CamelError::Config(format!(
                    "provider '{name}' max_concurrency must be > 0 when set (got 0)"
                )));
            }

            // Provider-level pricing: negative values are invalid
            if let Some(p) = provider.pricing()
                && (p.input_per_1k_tokens < 0.0 || p.output_per_1k_tokens < 0.0)
            {
                return Err(CamelError::Config(format!(
                    "provider '{name}' pricing has negative values: input={}, output={}",
                    p.input_per_1k_tokens, p.output_per_1k_tokens
                )));
            }

            // Provider-level cache_ttl_secs: Some(0) is invalid
            if provider.cache_ttl_secs() == Some(0) {
                return Err(CamelError::Config(format!(
                    "provider '{name}' cache_ttl_secs must be > 0 when set (got 0)"
                )));
            }

            // Provider-level cache_max_entries: Some(0) is invalid
            if provider.cache_max_entries() == Some(0) {
                return Err(CamelError::Config(format!(
                    "provider '{name}' cache_max_entries must be > 0 when set (got 0)"
                )));
            }
        }

        crate::decision::config::validate_decision_config(self)?;

        Ok(())
    }
}

/// Parsed endpoint configuration derived from a URI like `llm:chat?provider=...`.
#[derive(Clone, Debug)]
pub struct LlmEndpointConfig {
    /// The operation type (chat or embed).
    pub operation: LlmOperation,

    /// Provider name override.
    pub provider: Option<String>,

    /// Model name override.
    pub model: Option<String>,

    /// Sampling temperature.
    pub temperature: Option<f64>,

    /// Maximum tokens to generate.
    pub max_tokens: Option<u32>,

    /// Whether to stream the response (default: true).
    pub stream: bool,

    /// System prompt override.
    pub system_prompt: Option<String>,

    /// Name of the decision question set (`llm:decide` only).
    pub questions: Option<String>,
}

impl Default for LlmEndpointConfig {
    fn default() -> Self {
        Self {
            operation: LlmOperation::Chat,
            provider: None,
            model: None,
            temperature: None,
            max_tokens: None,
            stream: true,
            system_prompt: None,
            questions: None,
        }
    }
}

impl LlmEndpointConfig {
    /// Parse an endpoint configuration from a URI string.
    ///
    /// # Format
    /// `llm:{operation}?provider={name}&model={model}&temperature={n}&max_tokens={n}&stream={bool}&system_prompt={text}`
    ///
    /// # Parameters
    /// - `provider` — Provider name override.
    /// - `model` — Model name override.
    /// - `temperature` — Sampling temperature (parseable float).
    /// - `max_tokens` — Maximum tokens to generate (parseable integer).
    /// - `stream` — Whether to stream the response (default: `true`, also accepts `1`/`0`).
    /// - `system_prompt` — System prompt override.
    /// - `questions` — Question set name. Required by `llm:decide`; rejected by
    ///   `chat` and `embed`.
    ///
    /// `llm:decide` rejects `stream`, `temperature`, `max_tokens`, and
    /// `system_prompt`.
    ///
    /// # Errors
    /// Returns [`CamelError::InvalidUri`] if the operation is not recognized, if
    /// `llm:decide` lacks `questions` or carries a chat-only parameter, or if
    /// `chat`/`embed` carries `questions`.
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let (operation_str, query) = match uri.split_once('?') {
            Some((path, q)) => (path, q),
            None => (uri, ""),
        };

        let operation = match operation_str.trim_start_matches("llm:") {
            "chat" => LlmOperation::Chat,
            "embed" => LlmOperation::Embed,
            "decide" => LlmOperation::Decide,
            other => {
                return Err(CamelError::InvalidUri(format!(
                    "unknown llm operation: '{other}' (expected 'chat', 'embed' or 'decide')"
                )));
            }
        };

        let params: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();

        // Presence is checked with `contains_key`: the parsed `stream` defaults to true,
        // so its value cannot distinguish "absent" from "explicitly true".
        match operation {
            LlmOperation::Decide => {
                for name in ["stream", "temperature", "max_tokens", "system_prompt"] {
                    if params.contains_key(name) {
                        return Err(CamelError::InvalidUri(format!(
                            "parameter '{name}' is not supported by llm:decide"
                        )));
                    }
                }
                if !params.contains_key("questions") {
                    return Err(CamelError::InvalidUri(
                        "llm:decide requires 'questions'".into(),
                    ));
                }
            }
            LlmOperation::Chat | LlmOperation::Embed => {
                if params.contains_key("questions") {
                    return Err(CamelError::InvalidUri(
                        "parameter 'questions' is only supported by llm:decide".into(),
                    ));
                }
            }
        }

        let stream = params
            .get("stream")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true);

        Ok(Self {
            operation,
            provider: params.get("provider").cloned(),
            model: params.get("model").cloned(),
            temperature: params.get("temperature").and_then(|v| v.parse().ok()),
            max_tokens: params.get("max_tokens").and_then(|v| v.parse().ok()),
            stream,
            system_prompt: params.get("system_prompt").cloned(),
            questions: params.get("questions").cloned(),
        })
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
