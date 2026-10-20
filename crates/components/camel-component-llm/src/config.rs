use std::collections::HashMap;
use std::fmt;

use camel_component_api::CamelError;
use camel_component_api::NetworkRetryPolicy;

use crate::cost::PricingTable;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LlmOperation {
    /// Chat completion (streaming or non-streaming).
    Chat,
    /// Text embedding generation.
    Embed,
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
    ///
    /// # Errors
    /// Returns [`CamelError::InvalidUri`] if the operation is not recognized.
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let (operation_str, query) = match uri.split_once('?') {
            Some((path, q)) => (path, q),
            None => (uri, ""),
        };

        let operation = match operation_str.trim_start_matches("llm:") {
            "chat" => LlmOperation::Chat,
            "embed" => LlmOperation::Embed,
            other => {
                return Err(CamelError::InvalidUri(format!(
                    "unknown llm operation: '{other}' (expected 'chat' or 'embed')"
                )));
            }
        };

        let params: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();

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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_global_timeout() {
        let cfg = LlmGlobalConfig {
            default_provider: None,
            timeout_secs: Some(0), // Some(0) is invalid; None is valid
            max_prompt_bytes: 32768,
            max_header_json_bytes: 65_536,
            providers: HashMap::new(),
            allow_internal: false,
        };
        assert!(
            cfg.validate().is_err(),
            "Some(0) global timeout_secs must be rejected"
        );
    }

    #[test]
    fn accepts_none_global_timeout() {
        let cfg = LlmGlobalConfig {
            default_provider: None,
            timeout_secs: None, // None = no timeout, valid
            max_prompt_bytes: 32768,
            max_header_json_bytes: 65_536,
            providers: HashMap::new(),
            allow_internal: false,
        };
        assert!(
            cfg.validate().is_ok(),
            "None global timeout_secs must be valid"
        );
    }

    #[test]
    fn rejects_zero_provider_timeout() {
        let mut providers = HashMap::new();
        providers.insert(
            "bad".into(),
            LlmProviderConfig::Openai(OpenaiProviderConfig {
                api_key: "sk-test".into(),
                base_url: None,
                default_model: "gpt-4o".into(),
                timeout_secs: Some(0), // invalid
                max_concurrency: None,
                network_retry: None,
                pricing: None,
                cache_ttl_secs: None,
                cache_max_entries: None,
            }),
        );
        let cfg = LlmGlobalConfig {
            default_provider: None,
            timeout_secs: None,
            max_prompt_bytes: 32768,
            max_header_json_bytes: 65_536,
            providers,
            allow_internal: false,
        };
        assert!(
            cfg.validate().is_err(),
            "Some(0) provider timeout_secs must be rejected"
        );
    }

    #[test]
    fn rejects_zero_max_concurrency() {
        let mut providers = HashMap::new();
        providers.insert(
            "bad".into(),
            LlmProviderConfig::Openai(OpenaiProviderConfig {
                api_key: "sk-test".into(),
                base_url: None,
                default_model: "gpt-4o".into(),
                timeout_secs: None,
                max_concurrency: Some(0), // invalid
                network_retry: None,
                pricing: None,
                cache_ttl_secs: None,
                cache_max_entries: None,
            }),
        );
        let cfg = LlmGlobalConfig {
            default_provider: None,
            timeout_secs: None,
            max_prompt_bytes: 32768,
            max_header_json_bytes: 65_536,
            providers,
            allow_internal: false,
        };
        assert!(
            cfg.validate().is_err(),
            "Some(0) max_concurrency must be rejected"
        );
    }

    #[test]
    fn rejects_zero_ollama_timeout() {
        let mut providers = HashMap::new();
        providers.insert(
            "bad".into(),
            LlmProviderConfig::Ollama(OllamaProviderConfig {
                base_url: "http://localhost:11434".into(),
                default_model: "llama3".into(),
                timeout_secs: Some(0), // invalid
                max_concurrency: None,
                network_retry: None,
                pricing: None,
                cache_ttl_secs: None,
                cache_max_entries: None,
            }),
        );
        let cfg = LlmGlobalConfig {
            default_provider: None,
            timeout_secs: None,
            max_prompt_bytes: 32768,
            max_header_json_bytes: 65_536,
            providers,
            allow_internal: false,
        };
        assert!(
            cfg.validate().is_err(),
            "Some(0) Ollama timeout_secs must be rejected"
        );
    }

    #[test]
    fn rejects_negative_pricing() {
        let mut providers = HashMap::new();
        providers.insert(
            "bad".into(),
            LlmProviderConfig::Openai(OpenaiProviderConfig {
                api_key: "sk-test".into(),
                base_url: None,
                default_model: "gpt-4o".into(),
                timeout_secs: None,
                max_concurrency: None,
                network_retry: None,
                pricing: Some(PricingTable {
                    input_per_1k_tokens: -0.01,
                    output_per_1k_tokens: 0.03,
                }),
                cache_ttl_secs: None,
                cache_max_entries: None,
            }),
        );
        let cfg = LlmGlobalConfig {
            default_provider: None,
            timeout_secs: None,
            max_prompt_bytes: 32768,
            max_header_json_bytes: 65_536,
            providers,
            allow_internal: false,
        };
        assert!(
            cfg.validate().is_err(),
            "Negative input_per_1k_tokens must be rejected"
        );
    }

    #[test]
    fn rejects_zero_cache_ttl() {
        let mut providers = HashMap::new();
        providers.insert(
            "bad-cache".into(),
            LlmProviderConfig::Openai(OpenaiProviderConfig {
                api_key: "sk-test".into(),
                base_url: None,
                default_model: "gpt-4o".into(),
                timeout_secs: None,
                max_concurrency: None,
                network_retry: None,
                pricing: None,
                cache_ttl_secs: Some(0), // invalid
                cache_max_entries: None,
            }),
        );
        let cfg = LlmGlobalConfig {
            default_provider: None,
            timeout_secs: None,
            max_prompt_bytes: 32768,
            max_header_json_bytes: 65_536,
            providers,
            allow_internal: false,
        };
        assert!(
            cfg.validate().is_err(),
            "Some(0) cache_ttl_secs must be rejected"
        );
    }

    #[test]
    fn rejects_zero_cache_max_entries() {
        let mut providers = HashMap::new();
        providers.insert(
            "bad-entries".into(),
            LlmProviderConfig::Openai(OpenaiProviderConfig {
                api_key: "sk-test".into(),
                base_url: None,
                default_model: "gpt-4o".into(),
                timeout_secs: None,
                max_concurrency: None,
                network_retry: None,
                pricing: None,
                cache_ttl_secs: None,
                cache_max_entries: Some(0), // invalid
            }),
        );
        let cfg = LlmGlobalConfig {
            default_provider: None,
            timeout_secs: None,
            max_prompt_bytes: 32768,
            max_header_json_bytes: 65_536,
            providers,
            allow_internal: false,
        };
        assert!(
            cfg.validate().is_err(),
            "Some(0) cache_max_entries must be rejected"
        );
    }

    #[test]
    fn accepts_valid_provider_config() {
        let mut providers = HashMap::new();
        providers.insert(
            "valid".into(),
            LlmProviderConfig::Openai(OpenaiProviderConfig {
                api_key: "sk-test".into(),
                base_url: None,
                default_model: "gpt-4o".into(),
                timeout_secs: Some(30),
                max_concurrency: Some(5),
                network_retry: None,
                pricing: None,
                cache_ttl_secs: None,
                cache_max_entries: None,
            }),
        );
        let cfg = LlmGlobalConfig {
            default_provider: None,
            timeout_secs: Some(60),
            max_prompt_bytes: 32768,
            max_header_json_bytes: 65_536,
            providers,
            allow_internal: false,
        };
        assert!(
            cfg.validate().is_ok(),
            "Valid config with non-zero timeouts and concurrency must pass"
        );
    }

    #[test]
    fn validate_error_contains_provider_name() {
        let mut providers = HashMap::new();
        providers.insert(
            "my-openai".into(),
            LlmProviderConfig::Openai(OpenaiProviderConfig {
                api_key: "sk-test".into(),
                base_url: None,
                default_model: "gpt-4o".into(),
                timeout_secs: Some(0),
                max_concurrency: None,
                network_retry: None,
                pricing: None,
                cache_ttl_secs: None,
                cache_max_entries: None,
            }),
        );
        let cfg = LlmGlobalConfig {
            default_provider: None,
            timeout_secs: None,
            max_prompt_bytes: 32768,
            max_header_json_bytes: 65_536,
            providers,
            allow_internal: false,
        };
        let err = cfg.validate().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("my-openai"),
            "validation error should include provider name: {msg}"
        );
    }

    #[test]
    fn deserialize_global_config_with_providers() {
        let toml_str = r#"
default_provider = "my-openai"

[providers.my-openai]
type = "openai"
api_key = "secret-key"
default_model = "gpt-4o"

[providers.local]
type = "ollama"
base_url = "http://localhost:11434"
default_model = "llama3"

[providers.test]
type = "mock"
response = "echo"
default_model = "mock-model"
"#;
        let cfg: LlmGlobalConfig = toml::from_str(toml_str).expect("parse");
        assert_eq!(cfg.default_provider.as_deref(), Some("my-openai"));
        assert_eq!(cfg.providers.len(), 3);
        assert!(matches!(
            cfg.providers["my-openai"],
            LlmProviderConfig::Openai(_)
        ));
        assert!(matches!(
            cfg.providers["local"],
            LlmProviderConfig::Ollama(_)
        ));
        assert!(matches!(cfg.providers["test"], LlmProviderConfig::Mock(_)));
    }

    #[test]
    fn default_config_has_no_providers() {
        let cfg = LlmGlobalConfig::default();
        assert!(cfg.providers.is_empty());
        assert_eq!(cfg.timeout_secs, None);
        assert_eq!(cfg.max_prompt_bytes, 32768);
        assert_eq!(cfg.max_header_json_bytes, 65_536);
    }

    #[test]
    fn debug_redacts_api_key() {
        let cfg = LlmProviderConfig::Openai(OpenaiProviderConfig {
            api_key: "sk-secret123".into(),
            base_url: None,
            default_model: "gpt-4o".into(),
            timeout_secs: None,
            max_concurrency: None,
            network_retry: None,
            pricing: None,
            cache_ttl_secs: None,
            cache_max_entries: None,
        });
        let debug_str = format!("{:?}", cfg);
        assert!(debug_str.contains("[REDACTED]"));
        assert!(!debug_str.contains("sk-secret123"));
    }

    #[test]
    fn endpoint_config_from_uri_chat() {
        let ec = LlmEndpointConfig::from_uri(
            "llm:chat?provider=my-openai&model=gpt-4o&temperature=0.7&stream=false",
        );
        let ec = ec.expect("parse");
        assert_eq!(ec.operation, LlmOperation::Chat);
        assert_eq!(ec.provider.as_deref(), Some("my-openai"));
        assert_eq!(ec.model.as_deref(), Some("gpt-4o"));
        assert!(!ec.stream);
    }

    #[test]
    fn endpoint_config_from_uri_embed() {
        let ec = LlmEndpointConfig::from_uri("llm:embed?provider=local");
        let ec = ec.expect("parse");
        assert_eq!(ec.operation, LlmOperation::Embed);
        assert!(ec.stream); // default
    }

    #[test]
    fn from_uri_unknown_operation_returns_invalid_uri() {
        let result = LlmEndpointConfig::from_uri("llm:summarize?provider=x");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CamelError::InvalidUri(_)));
        assert!(err.to_string().contains("summarize"));
    }

    #[test]
    fn mock_config_with_error_message_deserializes() {
        let toml_str = r#"
[providers.err]
type = "mock"
error_message = "boom"
"#;
        let cfg: LlmGlobalConfig = toml::from_str(toml_str).expect("parse");
        let mock_cfg = match &cfg.providers["err"] {
            LlmProviderConfig::Mock(c) => c,
            _ => panic!("expected Mock"),
        };
        assert_eq!(mock_cfg.error_message.as_deref(), Some("boom"));
    }

    #[test]
    fn from_uri_stream_parsing() {
        // Explicit false
        let ec = LlmEndpointConfig::from_uri("llm:chat?stream=false").unwrap();
        assert!(!ec.stream);

        // Explicit true
        let ec = LlmEndpointConfig::from_uri("llm:chat?stream=true").unwrap();
        assert!(ec.stream);

        // Numeric true
        let ec = LlmEndpointConfig::from_uri("llm:chat?stream=1").unwrap();
        assert!(ec.stream);

        // Default (no param)
        let ec = LlmEndpointConfig::from_uri("llm:chat").unwrap();
        assert!(ec.stream);
    }

    // -----------------------------------------------------------------------
    // Generic provider configuration (llm-provider-extensibility 1.1)
    // -----------------------------------------------------------------------

    fn generic_config(service: LlmServiceKind) -> GenericProviderConfig {
        GenericProviderConfig {
            service,
            api_key: "test-key".into(),
            base_url: None,
            default_model: "explicit-model".into(),
            timeout_secs: None,
            max_concurrency: None,
            network_retry: None,
            pricing: None,
            cache_ttl_secs: None,
            cache_max_entries: None,
        }
    }

    fn generic_global_config(generic: GenericProviderConfig) -> LlmGlobalConfig {
        let mut providers = HashMap::new();
        providers.insert("generic-test".into(), LlmProviderConfig::Generic(generic));
        LlmGlobalConfig {
            providers,
            ..Default::default()
        }
    }

    #[test]
    fn generic_services_parse_and_preserve_values() {
        let toml_str = r#"
[providers.anthropic-generic]
type = "generic"
service = "anthropic"
api_key = " test-key "
default_model = "explicit-model"

[providers.gemini-generic]
type = "generic"
service = "gemini"
api_key = " test-key "
default_model = "explicit-model"
"#;
        let cfg: LlmGlobalConfig = toml::from_str(toml_str).expect("parse");
        cfg.validate().expect("validate");

        assert_eq!(
            cfg.providers.len(),
            2,
            "both generic entries must be present"
        );

        for (name, expected) in [
            ("anthropic-generic", LlmServiceKind::Anthropic),
            ("gemini-generic", LlmServiceKind::Gemini),
        ] {
            let generic = match &cfg.providers[name] {
                LlmProviderConfig::Generic(c) => c,
                other => panic!("expected Generic for {name}, got {other:?}"),
            };
            assert_eq!(generic.service, expected);
            assert_eq!(generic.api_key, " test-key ", "key must stay untrimmed");
            assert_eq!(generic.default_model, "explicit-model");
            assert_eq!(generic.base_url, None);
            assert_eq!(generic.timeout_secs, None);
            assert_eq!(generic.max_concurrency, None);
            assert_eq!(generic.network_retry, None);
            assert!(generic.pricing.is_none());
            assert_eq!(generic.cache_ttl_secs, None);
            assert_eq!(generic.cache_max_entries, None);
        }
    }

    #[test]
    fn generic_unknown_service_is_actionable() {
        let toml_str = r#"
[providers.typo]
type = "generic"
service = "anthropci"
api_key = "test-key"
default_model = "explicit-model"
"#;
        let err =
            toml::from_str::<LlmGlobalConfig>(toml_str).expect_err("unknown service must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("anthropci"),
            "must name the rejected service: {msg}"
        );
        assert!(
            msg.contains("anthropic"),
            "must list supported anthropic: {msg}"
        );
        assert!(msg.contains("gemini"), "must list supported gemini: {msg}");
    }

    #[test]
    fn generic_required_fields_are_enforced() {
        let missing_service = r#"
[providers.missing-service]
type = "generic"
api_key = "test-key"
default_model = "explicit-model"
"#;
        let err = toml::from_str::<LlmGlobalConfig>(missing_service)
            .expect_err("missing service must fail");
        assert!(err.to_string().contains("service"), "error: {err}");

        let missing_key = r#"
[providers.missing-key]
type = "generic"
service = "anthropic"
default_model = "explicit-model"
"#;
        let err =
            toml::from_str::<LlmGlobalConfig>(missing_key).expect_err("missing api_key must fail");
        assert!(err.to_string().contains("api_key"), "error: {err}");

        let missing_model = r#"
[providers.missing-model]
type = "generic"
service = "gemini"
api_key = "test-key"
"#;
        let err = toml::from_str::<LlmGlobalConfig>(missing_model)
            .expect_err("missing default_model must fail");
        assert!(err.to_string().contains("default_model"), "error: {err}");

        for (api_key, default_model, expected_field, forbidden) in [
            ("", "explicit-model", "api_key", "explicit-model"),
            ("  ", "explicit-model", "api_key", "explicit-model"),
            ("super-secret-key", "", "default_model", "super-secret-key"),
            (
                "super-secret-key",
                "  ",
                "default_model",
                "super-secret-key",
            ),
        ] {
            let cfg = generic_global_config(GenericProviderConfig {
                api_key: api_key.into(),
                default_model: default_model.into(),
                ..generic_config(LlmServiceKind::Anthropic)
            });
            let err = cfg.validate().expect_err("blank content must be rejected");
            assert!(
                matches!(&err, CamelError::Config(_)),
                "expected Config error, got: {err}"
            );
            let msg = err.to_string();
            assert!(
                msg.contains(expected_field),
                "must name {expected_field}: {msg}"
            );
            assert!(
                !msg.contains(forbidden),
                "must not expose {forbidden}: {msg}"
            );
        }
    }

    #[test]
    fn generic_debug_redacts_credentials_and_url() {
        let generic = GenericProviderConfig {
            service: LlmServiceKind::Gemini,
            api_key: "generic-secret-marker".into(),
            base_url: Some(
                "https://user:password-marker@example.com/v1?api_key=query-secret-marker".into(),
            ),
            ..generic_config(LlmServiceKind::Gemini)
        };
        let nested_debug = format!("{generic:?}");
        let enum_debug = format!("{:?}", LlmProviderConfig::Generic(generic.clone()));
        let global_debug = format!("{:?}", generic_global_config(generic));

        for rendered in [&nested_debug, &enum_debug, &global_debug] {
            for secret in [
                "generic-secret-marker",
                "password-marker",
                "query-secret-marker",
                "user:password-marker",
            ] {
                assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
            }
            assert!(
                rendered.contains("[REDACTED]"),
                "key redaction missing: {rendered}"
            );
            assert!(
                rendered.contains("***@") || rendered.contains("[redacted]"),
                "URL redaction missing: {rendered}"
            );
        }
    }

    #[test]
    fn generic_policy_fields_round_trip_and_validate() {
        let toml_str = r#"
[providers.generic-anthropic]
type = "generic"
service = "anthropic"
api_key = "test-key"
default_model = "explicit-model"
timeout_secs = 7
max_concurrency = 2
cache_ttl_secs = 9
cache_max_entries = 4
network_retry = { enabled = true, max_attempts = 3, initial_delay_ms = 1, max_delay_ms = 5, multiplier = 2.0, jitter_factor = 0.0 }
pricing = { input_per_1k_tokens = 0.01, output_per_1k_tokens = 0.02 }

[providers.generic-gemini]
type = "generic"
service = "gemini"
api_key = "test-key"
default_model = "explicit-model"
timeout_secs = 7
max_concurrency = 2
cache_ttl_secs = 9
cache_max_entries = 4
network_retry = { enabled = true, max_attempts = 3, initial_delay_ms = 1, max_delay_ms = 5, multiplier = 2.0, jitter_factor = 0.0 }
pricing = { input_per_1k_tokens = 0.01, output_per_1k_tokens = 0.02 }
"#;
        let cfg: LlmGlobalConfig = toml::from_str(toml_str).expect("parse");
        cfg.validate().expect("valid policy fields");

        for name in ["generic-anthropic", "generic-gemini"] {
            let provider = &cfg.providers[name];
            assert_eq!(provider.timeout_secs(), Some(7));
            assert_eq!(provider.max_concurrency(), Some(2));
            let retry = provider.network_retry().expect("retry policy");
            assert!(retry.enabled);
            assert_eq!(retry.max_attempts, 3);
            assert_eq!(retry.initial_delay, std::time::Duration::from_millis(1));
            assert_eq!(retry.max_delay, std::time::Duration::from_millis(5));
            assert_eq!(retry.multiplier, 2.0);
            assert_eq!(retry.jitter_factor, 0.0);
            assert_eq!(retry.max_attempts_absolute, None);
            let pricing = provider.pricing().expect("pricing");
            assert_eq!(pricing.input_per_1k_tokens, 0.01);
            assert_eq!(pricing.output_per_1k_tokens, 0.02);
            assert_eq!(provider.cache_ttl_secs(), Some(9));
            assert_eq!(provider.cache_max_entries(), Some(4));
        }

        for service in [LlmServiceKind::Anthropic, LlmServiceKind::Gemini] {
            let assert_rejected = |generic: GenericProviderConfig, field: &str| {
                let err = generic_global_config(generic)
                    .validate()
                    .expect_err("invalid generic policy must be rejected");
                assert!(
                    matches!(&err, CamelError::Config(_)),
                    "expected Config error, got: {err}"
                );
                assert!(err.to_string().contains(field), "must name {field}: {err}");
            };

            let mut config = generic_config(service);
            config.timeout_secs = Some(0);
            assert_rejected(config, "timeout_secs");

            let mut config = generic_config(service);
            config.max_concurrency = Some(0);
            assert_rejected(config, "max_concurrency");

            let mut config = generic_config(service);
            config.cache_ttl_secs = Some(0);
            assert_rejected(config, "cache_ttl_secs");

            let mut config = generic_config(service);
            config.cache_max_entries = Some(0);
            assert_rejected(config, "cache_max_entries");

            let mut config = generic_config(service);
            config.pricing = Some(PricingTable {
                input_per_1k_tokens: -1.0,
                output_per_1k_tokens: 0.02,
            });
            assert_rejected(config, "pricing");

            let mut config = generic_config(service);
            config.pricing = Some(PricingTable {
                input_per_1k_tokens: 0.01,
                output_per_1k_tokens: -1.0,
            });
            assert_rejected(config, "pricing");
        }
    }
}
