use std::collections::HashMap;
use std::fmt;

use camel_component_api::CamelError;

fn default_timeout() -> u64 {
    30
}

fn default_max_prompt_bytes() -> usize {
    32768
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

    /// Default timeout in seconds for LLM operations.
    ///
    /// **Note:** Parsed and stored but not yet enforced. Deferred to post-MVP.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    /// Maximum prompt size in bytes before truncation or rejection.
    #[serde(default = "default_max_prompt_bytes")]
    pub max_prompt_bytes: usize,

    /// Map of provider name to provider configuration.
    #[serde(default)]
    pub providers: HashMap<String, LlmProviderConfig>,
}

impl Default for LlmGlobalConfig {
    fn default() -> Self {
        Self {
            default_provider: None,
            timeout_secs: default_timeout(),
            max_prompt_bytes: default_max_prompt_bytes(),
            providers: HashMap::new(),
        }
    }
}

/// Configuration for a single LLM provider, discriminated by `type` field.
#[derive(Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LlmProviderConfig {
    /// OpenAI-compatible provider (also Azure OpenAI).
    Openai(OpenaiProviderConfig),
    /// Ollama (local) provider.
    Ollama(OllamaProviderConfig),
    /// Mock provider for testing.
    Mock(MockProviderConfig),
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
                .finish(),
            LlmProviderConfig::Ollama(c) => f
                .debug_struct("Ollama")
                .field("base_url", &c.base_url)
                .field("default_model", &c.default_model)
                .field("timeout_secs", &c.timeout_secs)
                .field("max_concurrency", &c.max_concurrency)
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
    ///
    /// **Note:** Parsed and stored but not yet enforced. Deferred to post-MVP.
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Optional max concurrency for this provider.
    ///
    /// **Note:** Parsed and stored but not yet enforced. Deferred to post-MVP.
    #[serde(default)]
    pub max_concurrency: Option<usize>,
}

/// Configuration for an Ollama (local) provider.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct OllamaProviderConfig {
    /// Base URL for the Ollama server (e.g., "http://localhost:11434").
    pub base_url: String,

    /// Default model to use (e.g., "llama3").
    pub default_model: String,

    /// Optional per-provider timeout override (seconds).
    ///
    /// **Note:** Parsed and stored but not yet enforced. Deferred to post-MVP.
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Optional max concurrency for this provider.
    ///
    /// **Note:** Parsed and stored but not yet enforced. Deferred to post-MVP.
    #[serde(default)]
    pub max_concurrency: Option<usize>,
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
        assert_eq!(cfg.timeout_secs, 30);
        assert_eq!(cfg.max_prompt_bytes, 32768);
    }

    #[test]
    fn debug_redacts_api_key() {
        let cfg = LlmProviderConfig::Openai(OpenaiProviderConfig {
            api_key: "sk-secret123".into(),
            base_url: None,
            default_model: "gpt-4o".into(),
            timeout_secs: None,
            max_concurrency: None,
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
}
