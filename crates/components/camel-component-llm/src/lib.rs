//! camel-component-llm — LLM integration component for rust-camel.
//!
//! Provides chat (streaming + materialized), embeddings, tool calling, and
//! multi-turn conversations through multiple LLM providers (OpenAI, Ollama,
//! Mock). Features include response caching with single-flight, cost
//! observability (config-driven pricing tables), retry with per-attempt delay
//! override (ADR-0021), and concurrency control via producer semaphore.
//!
//! Uses a project-owned `LlmProvider` trait with strict adapter boundary
//! isolating the siumai SDK to exactly two production files (ADR-0020).

pub mod bundle;
pub mod config;
pub mod cost;
pub mod endpoint;
pub mod error;
pub mod headers;
pub mod producer;
pub mod producer_cache;
pub mod provider;
pub mod provider_factory;

pub use bundle::LlmBundle;
pub use config::{
    LlmEndpointConfig, LlmGlobalConfig, LlmOperation, LlmProviderConfig, MockProviderConfig,
    OllamaProviderConfig, OpenaiProviderConfig,
};
pub use cost::PricingTable;
pub use error::LlmError;
pub use provider::{
    ChatEvent, ChatMessage, ChatRequest, ChatRole, EmbedRequest, EmbedResponse, EmittedToolCall,
    FinishReason, LlmProvider, LlmUsage, ToolChoice, ToolDefinition,
};

use std::sync::Arc;
use std::time::Duration;

use camel_component_api::{CamelError, Component, ComponentContext, Endpoint};
use provider_factory::ProviderMap;

/// Build a hardened `reqwest::Client` for outbound LLM HTTP traffic.
///
/// Hardening (H15):
/// - **No redirects** — a 302/303 to an attacker-controlled host would
///   bypass scheme/IP validation. Both OpenAI and Ollama APIs are
///   non-redirecting; a redirect response is a signal of misconfiguration
///   or attack.
/// - **Connect timeout 10s** — bound TCP handshake.
/// - **Request timeout 30s** — bound total request lifetime. (Per-request
///   deadlines set via siumai's `timeout` builder take precedence for
///   streaming activity; this is the hard ceiling.)
/// - **Optional DNS pinning** — when `pinned` is `Some((host, addrs))`,
///   applies `resolve_to_addrs` to close the TOCTOU window between SSRF
///   validation and the first outbound request (D-M10).
pub fn hardened_http_client_with_pinning(
    pinned: Option<(&str, &[std::net::SocketAddr])>,
) -> Result<reqwest::Client, CamelError> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy() // CRITICAL: env proxies bypass resolve_to_addrs
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30));
    if let Some((host, addrs)) = pinned {
        builder = builder.resolve_to_addrs(host, addrs);
    }
    builder.build().map_err(|e| {
        CamelError::EndpointCreationFailed(format!("llm HTTP client build failed: {e}"))
    })
}

/// Build a hardened `reqwest::Client` for outbound LLM HTTP traffic, without
/// DNS pinning.
///
/// Convenience wrapper around [`hardened_http_client_with_pinning`].
pub fn hardened_http_client() -> Result<reqwest::Client, CamelError> {
    hardened_http_client_with_pinning(None)
}

pub struct LlmComponent {
    providers: Arc<ProviderMap>,
    config: Arc<LlmGlobalConfig>,
}

impl LlmComponent {
    pub fn new(config: LlmGlobalConfig) -> Result<Self, CamelError> {
        let providers = provider_factory::build_provider_map(&config).map_err(CamelError::from)?;
        Ok(Self {
            providers: Arc::new(providers),
            config: Arc::new(config),
        })
    }

    pub fn from_parts(config: LlmGlobalConfig, providers: ProviderMap) -> Self {
        Self {
            providers: Arc::new(providers),
            config: Arc::new(config),
        }
    }

    pub fn providers(&self) -> &ProviderMap {
        &self.providers
    }

    pub fn config(&self) -> &LlmGlobalConfig {
        &self.config
    }
}

impl Component for LlmComponent {
    fn scheme(&self) -> &str {
        "llm"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let endpoint_config = LlmEndpointConfig::from_uri(uri)?;

        // Fail-fast: validate provider exists and supports the operation
        let provider_name = endpoint::resolve_provider_name(&endpoint_config, &self.config)?;

        let provider = self.providers.get(provider_name).ok_or_else(|| {
            CamelError::InvalidUri(format!("provider '{}' not found in config", provider_name))
        })?;

        if endpoint_config.operation == LlmOperation::Embed && !provider.supports_embed() {
            return Err(CamelError::InvalidUri(format!(
                "provider '{}' does not support embed",
                provider_name
            )));
        }

        Ok(Box::new(endpoint::LlmEndpoint::new(
            uri.to_string(),
            endpoint_config,
            Arc::clone(&self.providers),
            Arc::clone(&self.config),
        )))
    }
}
