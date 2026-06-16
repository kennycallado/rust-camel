//! camel-component-llm — LLM integration component for rust-camel.
//!
//! Provides chat and embedding operations through multiple LLM providers
//! (OpenAI, Ollama, Mock). Uses a provider trait abstraction with strict
//! adapter boundary isolating the siumai SDK.

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

use camel_component_api::{CamelError, Component, ComponentContext, Endpoint};
use provider_factory::ProviderMap;

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
