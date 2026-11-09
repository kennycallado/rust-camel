//! camel-component-llm — LLM integration component for rust-camel.
//!
//! Provides chat (streaming + materialized), embeddings, tool calling, and
//! multi-turn conversations through multiple LLM providers (OpenAI, Ollama,
//! Mock). Features include response caching with single-flight, cost
//! observability (config-driven pricing tables), retry with per-attempt delay
//! override (ADR-0021), and concurrency control via producer semaphore.
//!
//! Uses a project-owned `LlmProvider` trait with strict adapter boundary
//! isolating the genai SDK behind `provider/genai_adapter.rs` (ADR-0020).

pub mod bundle;
pub mod config;
pub mod cost;
pub mod decision;
pub mod endpoint;
pub mod error;
pub mod headers;
pub(crate) mod metadata;
pub mod producer;
pub mod producer_cache;
pub mod provider;
pub mod provider_factory;

pub use bundle::LlmBundle;
pub use config::{
    GenericProviderConfig, LlmEndpointConfig, LlmGlobalConfig, LlmOperation, LlmProviderConfig,
    LlmServiceKind, MockProviderConfig, OllamaProviderConfig, OpenaiProviderConfig,
};
pub use cost::PricingTable;
pub use decision::{
    ChoiceOption, ChoiceProbability, DecisionAnswer, DecisionCapabilities, DecisionProvider,
    DecisionQuestion, DecisionRequest, DecisionResponse, DecisionState, DecisionUsage, NamedAnswer,
    QuestionKind, ScoreLevel, ScoreProbability,
};
pub use error::{LlmError, TransportTimeoutPhase};
pub use provider::{
    ChatEvent, ChatMessage, ChatRequest, ChatRole, EmbedRequest, EmbedResponse, EmittedToolCall,
    FinishReason, LlmProvider, LlmUsage, ToolChoice, ToolDefinition,
};

use std::sync::Arc;
use std::time::Duration;

use camel_component_api::{CamelError, Component, ComponentContext, ComponentMetadata, Endpoint};
use decision::endpoint::DecisionEndpoint;
use decision::factory;
use decision::factory::DecisionProviderMap;
use metadata::LlmMetadataDescriptor;
use provider_factory::ProviderMap;

/// Per-caller transport policy for hardened LLM HTTP clients.
///
/// No total-timeout field exists by design: the configured deadline
/// (`timeout_secs`) owns the total; see ADR-0021 and the change
/// `llm-timeout-deadline`.
#[derive(Clone, Copy)]
pub(crate) struct LlmHttpPolicy {
    /// TCP connect bound (both policies: 10s).
    pub(crate) connect: Duration,
    /// Transport read-idle guard: `Some` on the chat/embed client
    /// (600s), `None` on the decision client (its per-request total
    /// must not be shadowed; reqwest 0.13.5 `read_timeout` also caps
    /// time-to-headers and has no per-request override).
    pub(crate) read_idle: Option<Duration>,
}
// Why Copy: the policy is passed BY VALUE into map_genai_error call sites
// from `&self`; the adapter copies it into a local BEFORE building the
// 'static stream closure (the stream outlives the borrow).
impl LlmHttpPolicy {
    /// Chat/embed: connect 10s + 600s transport read-idle guard.
    /// 600s matches the OpenAI/Anthropic SDK per-read defaults and sits
    /// above Ollama's 300s model-load stall limit. MUST NOT be derived
    /// from `timeout_secs` (the timers would race and flip the reported
    /// layer).
    pub(crate) fn chat() -> Self {
        Self {
            connect: Duration::from_secs(10),
            read_idle: Some(Duration::from_secs(600)),
        }
    }

    /// Decisions: connect 10s, NO read-idle guard.
    // Only the `decisions` feature wires this constructor; unit tests cover it
    // in every configuration, so the test build keeps it available.
    #[cfg(any(feature = "decisions", test))]
    pub(crate) fn decision() -> Self {
        Self {
            connect: Duration::from_secs(10),
            read_idle: None,
        }
    }
}

/// Build a hardened `reqwest::Client` for outbound LLM HTTP traffic.
///
/// Hardening (H15):
/// - **No redirects** — a 302/303 to an attacker-controlled host would
///   bypass scheme/IP validation. Both OpenAI and Ollama APIs are
///   non-redirecting; a redirect response is a signal of misconfiguration
///   or attack.
/// - **`no_proxy`** — required so environment proxies cannot bypass
///   `resolve_to_addrs` DNS pinning.
/// - **Connect timeout 10s** — bound TCP handshake.
/// - **No client-wide total timeout** — the configured deadline
///   (`timeout_secs`) owns the total in materialized mode and the
///   per-chunk activity timeout in streaming mode. The chat/embed client
///   additionally carries a 600s transport read-idle guard; the decision
///   client carries none.
/// - **Optional DNS pinning** — when `pinned` is `Some((host, addrs))`,
///   applies `resolve_to_addrs` to close the TOCTOU window between SSRF
///   validation and the first outbound request (D-M10).
pub fn hardened_http_client_with_pinning(
    pinned: Option<(&str, &[std::net::SocketAddr])>,
) -> Result<reqwest::Client, CamelError> {
    hardened_http_client_with_policy(LlmHttpPolicy::chat(), pinned)
}

/// Policy-parameterized hardened client builder; see [`hardened_http_client_with_pinning`].
pub(crate) fn hardened_http_client_with_policy(
    policy: LlmHttpPolicy,
    pinned: Option<(&str, &[std::net::SocketAddr])>,
) -> Result<reqwest::Client, CamelError> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy() // CRITICAL: env proxies bypass resolve_to_addrs
        .connect_timeout(policy.connect);
    if let Some(d) = policy.read_idle {
        builder = builder.read_timeout(d);
    }
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
    decision_providers: Arc<DecisionProviderMap>,
    config: Arc<LlmGlobalConfig>,
}

impl LlmComponent {
    pub fn new(config: LlmGlobalConfig) -> Result<Self, CamelError> {
        config.validate()?;
        let providers = provider_factory::build_provider_map(&config).map_err(CamelError::from)?;
        let decision_providers =
            factory::build_decision_provider_map(&config).map_err(CamelError::from)?;
        Ok(Self {
            providers: Arc::new(providers),
            decision_providers: Arc::new(decision_providers),
            config: Arc::new(config),
        })
    }

    /// Decision providers start empty; chain `with_decision_providers`.
    pub fn from_parts(config: LlmGlobalConfig, providers: ProviderMap) -> Self {
        Self {
            providers: Arc::new(providers),
            decision_providers: Arc::new(DecisionProviderMap::new()),
            config: Arc::new(config),
        }
    }

    /// Replaces the decision provider map used by `llm:decide` endpoints.
    pub fn with_decision_providers(mut self, map: DecisionProviderMap) -> Self {
        self.decision_providers = Arc::new(map);
        self
    }

    pub fn providers(&self) -> &ProviderMap {
        &self.providers
    }

    /// Decision providers keyed by their configured name.
    pub fn decision_providers(&self) -> &DecisionProviderMap {
        self.decision_providers.as_ref()
    }

    pub fn config(&self) -> &LlmGlobalConfig {
        &self.config
    }

    fn create_decide_endpoint(
        &self,
        uri: &str,
        endpoint_config: LlmEndpointConfig,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        // `from_parts` skips `validate()`; producers rely on validated sets.
        crate::decision::config::validate_decision_config(&self.config)?;
        let provider_name = endpoint_config
            .provider
            .as_deref()
            .or(self.config.default_decision_provider.as_deref())
            .ok_or_else(|| {
                CamelError::InvalidUri(
                    "no decision provider specified and no default_decision_provider configured"
                        .into(),
                )
            })?
            .to_string();

        let provider = self.decision_providers.get(&provider_name).ok_or_else(|| {
            if self.config.decision_providers.contains_key(&provider_name) {
                CamelError::InvalidUri(format!(
                    "decision provider '{provider_name}' is configured but not registered \
                     (build the component with LlmComponent::new or call with_decision_providers)"
                ))
            } else {
                CamelError::InvalidUri(format!(
                    "decision provider '{provider_name}' not found in config"
                ))
            }
        })?;

        let set_name = endpoint_config
            .questions
            .as_deref()
            .ok_or_else(|| CamelError::InvalidUri("llm:decide requires 'questions'".into()))?;
        let set = self
            .config
            .decision_question_sets
            .get(set_name)
            .ok_or_else(|| {
                CamelError::InvalidUri(format!("question set '{set_name}' not found in config"))
            })?;

        let max_questions = provider.capabilities().max_questions;
        if set.questions.len() > max_questions {
            return Err(CamelError::InvalidUri(format!(
                "question set '{set_name}' has {} questions; decision provider '{provider_name}' accepts at most {max_questions}",
                set.questions.len()
            )));
        }

        Ok(Box::new(DecisionEndpoint::new(
            uri.to_string(),
            endpoint_config,
            provider_name,
            Arc::clone(provider),
            Arc::new(set.clone()),
            Arc::clone(&self.config),
        )))
    }
}

impl Component for LlmComponent {
    fn scheme(&self) -> &str {
        "llm"
    }

    fn metadata(&self) -> ComponentMetadata {
        LlmMetadataDescriptor::metadata()
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let endpoint_config = LlmEndpointConfig::from_uri(uri)?;

        if endpoint_config.operation == LlmOperation::Decide {
            return self.create_decide_endpoint(uri, endpoint_config);
        }

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

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
