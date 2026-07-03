//! Siumai adapter — bridges the siumai SDK to our `LlmProvider` trait.
//!
//! This is the ONLY file (besides provider_factory.rs) that may import siumai types.
//! All siumai types are translated to our Camel-shaped types at this boundary.
//!
//! ## Lazy construction
//!
//! The siumai client is built lazily on the first async call (`chat_stream` or `embed`)
//! rather than synchronously via `block_on` at `build_openai`/`build_ollama` time.
//! This avoids panics when the builder is called from within a tokio runtime.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

// ============================================================================
// Test-only overrides — inject stub builders to avoid network I/O
// ============================================================================

/// Test-only builder: a closure that produces a fully-initialized client pair
/// without network access.
#[cfg(test)]
type BuildOverride = std::sync::Arc<
    dyn Fn() -> BoxFuture<
            'static,
            Result<(Arc<dyn ChatCapability>, Arc<dyn EmbeddingCapability>), LlmError>,
        > + Send
        + Sync,
>;
use futures::stream::{BoxStream, StreamExt};

#[cfg(test)]
use futures::future::BoxFuture;

use tokio::sync::OnceCell;

#[cfg(any(feature = "ollama", feature = "all-providers"))]
use crate::config::OllamaProviderConfig;
#[cfg(any(feature = "openai", feature = "all-providers"))]
use crate::config::OpenaiProviderConfig;
use crate::error::LlmError;
#[cfg(test)]
use crate::provider::EmittedToolCall;
use crate::provider::{
    ChatEvent, ChatMessage, ChatRequest, ChatRole, EmbedRequest, EmbedResponse, FinishReason,
    LlmProvider, LlmUsage, ToolChoice, ToolDefinition,
};

use siumai_core::builder::BuilderBase;
use siumai_core::error::LlmError as SiumaiLlmError;
#[cfg(test)]
use siumai_core::traits::EmbeddingExtensions;
use siumai_core::traits::{ChatCapability, EmbeddingCapability};
use siumai_core::types::{
    ChatMessage as SiumaiChatMessage, ChatRequest as SiumaiChatRequest, CommonParams, ContentPart,
    EmbeddingRequest as SiumaiEmbeddingRequest, Tool as SiumaiTool, ToolChoice as SiumaiToolChoice,
};

// ============================================================================
// Config enum — stores the provider configuration for lazy client construction
// ============================================================================

/// Provider configuration stored for lazy client construction.
///
/// The siumai client is built on first use inside `chat_stream()` or `embed()`
/// to avoid calling `block_on` from a synchronous context.
#[derive(Clone)]
enum SiumaiConfig {
    /// OpenAI-compatible provider configuration.
    #[cfg(any(feature = "openai", feature = "all-providers"))]
    OpenAi {
        api_key: String,
        base_url: Option<String>,
        model: String,
    },
    /// Ollama (local) provider configuration.
    #[cfg(any(feature = "ollama", feature = "all-providers"))]
    Ollama { base_url: String, model: String },
}

// ============================================================================
// Provider struct
// ============================================================================

/// Cached siumai trait-object pair.
///
/// `build_client_from_config` returns a tuple `(Arc<dyn ChatCapability>, Arc<dyn EmbeddingCapability>)`.
/// `OnceCell` requires a single type, so we wrap them in a struct.
struct CachedClient {
    chat: Arc<dyn ChatCapability>,
    embed: Arc<dyn EmbeddingCapability>,
}

/// A provider backed by a siumai client, constructed lazily and cached.
///
/// The siumai client is built exactly once via `tokio::sync::OnceCell`.
/// Concurrent calls share the same cached client.
struct SiumaiProvider {
    id: String,
    default_model: String,
    config: SiumaiConfig,
    client: Arc<OnceCell<CachedClient>>,
    configured_timeout: Duration,
    #[cfg(test)]
    client_builds: Arc<AtomicUsize>,
    #[cfg(test)]
    build_override: Option<BuildOverride>,
}

impl SiumaiProvider {
    fn new(
        id: impl Into<String>,
        default_model: impl Into<String>,
        config: SiumaiConfig,
        configured_timeout: Duration,
    ) -> Self {
        Self {
            id: id.into(),
            default_model: default_model.into(),
            config,
            client: Arc::new(OnceCell::new()),
            configured_timeout,
            #[cfg(test)]
            client_builds: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            build_override: None,
        }
    }

    /// Test-only constructor that injects a build override to avoid network I/O.
    #[cfg(test)]
    fn new_with_build_override(
        id: impl Into<String>,
        default_model: impl Into<String>,
        config: SiumaiConfig,
        configured_timeout: Duration,
        override_: BuildOverride,
    ) -> Self {
        Self {
            id: id.into(),
            default_model: default_model.into(),
            config,
            client: Arc::new(OnceCell::new()),
            configured_timeout,
            client_builds: Arc::new(AtomicUsize::new(0)),
            build_override: Some(override_),
        }
    }
}

#[async_trait]
impl LlmProvider for SiumaiProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn chat_stream(&self, req: ChatRequest) -> BoxStream<'static, Result<ChatEvent, LlmError>> {
        let config = self.config.clone();
        let default_model = self.default_model.clone();
        let client_cell = Arc::clone(&self.client);
        let configured_timeout = self.configured_timeout;
        #[cfg(test)]
        let builds = Arc::clone(&self.client_builds);
        #[cfg(test)]
        let override_ = self.build_override.clone();

        let stream = async_stream::try_stream! {
            // Build/retrieve cached client
            let cached = client_cell.get_or_try_init(|| async {
                build_cached_client(
                    &config,
                    configured_timeout,
                    #[cfg(test)] override_.as_ref(),
                    #[cfg(test)] &builds,
                ).await
            }).await?;
            let chat = Arc::clone(&cached.chat);

            let siumai_req = convert_chat_request(req, &default_model);

            let siumai_stream = chat
                .chat_stream_request(siumai_req)
                .await
                .map_err(|e| map_siumai_error(e, configured_timeout))?;

            let mut stream = siumai_stream;
            // Tool-call accumulator: tool_call_id -> (tool_name, accumulated_args)
            let mut tool_call_buffers: HashMap<String, (String, String)> = HashMap::new();
            while let Some(result) = stream.next().await {
                match result {
                    Ok(event) => {
                        // Check for tool-call streaming deltas first (stateful).
                        if let Some(tc_event) = extract_tool_call_event(&event, &mut tool_call_buffers) {
                            yield tc_event;
                            continue;
                        }
                        match convert_stream_event(event) {
                            Ok(Some(our_event)) => yield our_event,
                            Ok(None) => {} // skip metadata-only events
                            Err(e) => Err(e)?,
                        }
                    }
                    Err(e) => {
                        Err(map_siumai_error(e, configured_timeout))?;
                    }
                }
            }
        };

        stream.boxed()
    }

    async fn embed(&self, req: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        let config = self.config.clone();
        let configured_timeout = self.configured_timeout;
        #[cfg(test)]
        let builds = Arc::clone(&self.client_builds);
        #[cfg(test)]
        let override_ = self.build_override.clone();

        let cached = self
            .client
            .get_or_try_init(|| async {
                build_cached_client(
                    &config,
                    configured_timeout,
                    #[cfg(test)]
                    override_.as_ref(),
                    #[cfg(test)]
                    &builds,
                )
                .await
            })
            .await?;
        let embed = Arc::clone(&cached.embed);

        // Thread the per-request model into the siumai request. Calling
        // `embed(Vec<String>)` would use the client's default model, which may
        // be a chat model that does not support embeddings (e.g. Ollama returns
        // 501 "This server does not support embeddings" when a non-embedding
        // model is used without the `--embeddings` flag).
        let model = if req.model.is_empty() {
            self.default_model.clone()
        } else {
            req.model.clone()
        };

        let siumai_req = SiumaiEmbeddingRequest::new(req.inputs.clone()).with_model(model);

        // `embed_with_config` honors the request's `model` field, falling back to
        // the client default only when the field is absent.
        let extensions = embed.as_embedding_extensions().ok_or_else(|| {
            LlmError::Provider(format!(
                "provider {} does not support model-scoped embeddings (embed_with_config)",
                self.id
            ))
        })?;

        let response = extensions
            .embed_with_config(siumai_req)
            .await
            .map_err(|e| map_siumai_error(e, configured_timeout))?;

        Ok(convert_embed_response(response))
    }
}

// ============================================================================
// Helper: build cached client (shared by chat_stream and embed)
// ============================================================================

/// Build a `CachedClient`, respecting the test-only override when present.
///
/// This is the single point of client construction used by both `chat_stream`
/// and `embed`. Keeping it as a free function (not a method) allows it to be
/// called inside `async_stream::try_stream!` without borrowing `self`.
async fn build_cached_client(
    config: &SiumaiConfig,
    configured_timeout: Duration,
    #[cfg(test)] override_: Option<&BuildOverride>,
    #[cfg(test)] builds: &AtomicUsize,
) -> Result<CachedClient, LlmError> {
    #[cfg(test)]
    {
        builds.fetch_add(1, Ordering::SeqCst);
        if let Some(builder) = override_ {
            let (chat, embed) = builder().await?;
            return Ok(CachedClient { chat, embed });
        }
    }
    let (chat, embed) = build_client_from_config(config, configured_timeout).await?;
    Ok(CachedClient { chat, embed })
}

// ============================================================================
// Helper: build client from config (used in chat_stream try_stream! macro)
// ============================================================================

/// Build a fresh chat + embedding client from config.
///
/// Exists as a free function so it can be called inside `async_stream::try_stream!`
/// without borrowing `self` across a yield point.
async fn build_client_from_config(
    config: &SiumaiConfig,
    configured_timeout: Duration,
) -> Result<(Arc<dyn ChatCapability>, Arc<dyn EmbeddingCapability>), LlmError> {
    // H15: inject our hardened reqwest::Client (no redirects, 10s connect,
    // 30s request). Siumai honors `BuilderBase::http_client` over its own
    // client construction (see `siumai_core::builder::BuilderBase::http_client`).
    let hardened = crate::hardened_http_client()
        .map_err(|e| LlmError::InvalidRequest(format!("llm hardened client: {e}")))?;
    let base = BuilderBase {
        http_client: Some(hardened),
        ..BuilderBase::default()
    };
    match config {
        #[cfg(any(feature = "openai", feature = "all-providers"))]
        SiumaiConfig::OpenAi {
            api_key,
            base_url,
            model,
        } => {
            let mut builder = siumai_provider_openai::providers::openai::OpenAiBuilder::new(base);
            builder = builder.api_key(api_key).model(model);
            if let Some(url) = base_url {
                builder = builder.base_url(url);
            }
            let client = builder
                .build()
                .await
                .map_err(|e| map_siumai_error(e, configured_timeout))?;
            let client = Arc::new(client);
            let chat: Arc<dyn ChatCapability> = client.clone();
            let embed: Arc<dyn EmbeddingCapability> = client;
            Ok((chat, embed))
        }
        #[cfg(any(feature = "ollama", feature = "all-providers"))]
        SiumaiConfig::Ollama { base_url, model } => {
            let mut builder = siumai_provider_ollama::providers::ollama::OllamaBuilder::new(base);
            builder = builder.base_url(base_url).model(model);
            let client = builder
                .build()
                .await
                .map_err(|e| map_siumai_error(e, configured_timeout))?;
            let client = Arc::new(client);
            let chat: Arc<dyn ChatCapability> = client.clone();
            let embed: Arc<dyn EmbeddingCapability> = client;
            Ok((chat, embed))
        }
    }
}

// ============================================================================
// Builder functions — synchronous, no block_on
// ============================================================================

/// Build an OpenAI-backed `LlmProvider`.
///
/// The siumai client is constructed lazily on first use — no `block_on` here.
#[cfg(any(feature = "openai", feature = "all-providers"))]
pub fn build_openai(
    name: &str,
    config: &OpenaiProviderConfig,
    configured_timeout: Duration,
) -> Result<Arc<dyn LlmProvider>, LlmError> {
    let cfg = SiumaiConfig::OpenAi {
        api_key: config.api_key.clone(),
        base_url: config.base_url.clone(),
        model: config.default_model.clone(),
    };
    Ok(Arc::new(SiumaiProvider::new(
        name,
        &config.default_model,
        cfg,
        configured_timeout,
    )))
}

/// Build an Ollama-backed `LlmProvider`.
///
/// The siumai client is constructed lazily on first use — no `block_on` here.
#[cfg(any(feature = "ollama", feature = "all-providers"))]
pub fn build_ollama(
    name: &str,
    config: &OllamaProviderConfig,
    configured_timeout: Duration,
) -> Result<Arc<dyn LlmProvider>, LlmError> {
    let cfg = SiumaiConfig::Ollama {
        base_url: config.base_url.clone(),
        model: config.default_model.clone(),
    };
    Ok(Arc::new(SiumaiProvider::new(
        name,
        &config.default_model,
        cfg,
        configured_timeout,
    )))
}

// ============================================================================
// Type conversions
// ============================================================================

/// Convert a Camel `ToolDefinition` to a siumai `Tool`.
fn map_tool_definition(def: ToolDefinition) -> SiumaiTool {
    SiumaiTool::function(
        def.name,
        def.description,
        serde_json::Value::Object(def.parameters),
    )
}

/// Convert a Camel `ToolChoice` to a siumai `ToolChoice`.
fn map_tool_choice(choice: ToolChoice) -> SiumaiToolChoice {
    match choice {
        ToolChoice::Auto => SiumaiToolChoice::Auto,
        ToolChoice::None => SiumaiToolChoice::None,
        ToolChoice::Specific(name) => SiumaiToolChoice::tool(name),
    }
}

/// Extract a completed `ChatEvent::ToolCall` from siumai tool-input stream deltas.
///
/// Maintains an accumulator buffer keyed by tool-call id. Returns `Some(ChatEvent::ToolCall)`
/// only when a `ToolInputEnd` is seen. `ToolInputStart` initialises the buffer and returns
/// `None` (no emission until the call is complete). `ToolInputDelta` appends to the buffer
/// and returns `None`.
fn extract_tool_call_event(
    event: &siumai_core::streaming::ChatStreamEvent,
    buffers: &mut HashMap<String, (String, String)>,
) -> Option<ChatEvent> {
    let part = match event {
        siumai_core::streaming::ChatStreamEvent::Part { part } => part,
        siumai_core::streaming::ChatStreamEvent::PartWithReplay { part, .. } => part,
        _ => return None,
    };

    match part {
        siumai_core::types::ChatStreamPart::ToolInputStart { id, tool_name, .. } => {
            buffers.insert(id.clone(), (tool_name.clone(), String::new()));
            // Buffer initialised — no event emitted. Wait for ToolInputEnd.
            None
        }
        siumai_core::types::ChatStreamPart::ToolInputDelta { id, delta, .. } => {
            if let Some((_, args)) = buffers.get_mut(id) {
                args.push_str(delta);
            } else {
                tracing::warn!("tool call delta without prior start: id={}", id);
            }
            None
        }
        siumai_core::types::ChatStreamPart::ToolInputEnd { id, .. } => {
            if let Some((name, args)) = buffers.remove(id) {
                Some(ChatEvent::ToolCall {
                    id: id.clone(),
                    name,
                    arguments: args,
                })
            } else {
                tracing::warn!("tool call end without prior start: id={}", id);
                None
            }
        }
        _ => None,
    }
}

/// Convert our `ChatRequest` to a siumai `ChatRequest`.
fn convert_chat_request(req: ChatRequest, default_model: &str) -> SiumaiChatRequest {
    // Move model instead of cloning — it's only used once.
    let model = if req.model.is_empty() {
        default_model.to_string()
    } else {
        req.model
    };

    let mut messages: Vec<SiumaiChatMessage> =
        req.messages.into_iter().map(convert_chat_message).collect();

    // Prepend system prompt as a system message if provided.
    if let Some(system_prompt) = req.system_prompt {
        messages.insert(0, SiumaiChatMessage::system(system_prompt).build());
    }

    let common_params = CommonParams {
        model, // moved, no clone
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        stop_sequences: req.stop,
        ..Default::default()
    };

    SiumaiChatRequest {
        messages,
        common_params,
        stream: true,
        tools: if req.tools.is_empty() {
            None
        } else {
            Some(req.tools.into_iter().map(map_tool_definition).collect())
        },
        tool_choice: req.tool_choice.map(map_tool_choice),
        ..Default::default()
    }
}

/// Convert our `ChatMessage` to a siumai `ChatMessage`.
///
/// Single match — no redundant double-match.
fn convert_chat_message(msg: ChatMessage) -> SiumaiChatMessage {
    match msg.role {
        ChatRole::System => SiumaiChatMessage::system(msg.content).build(),
        ChatRole::User => SiumaiChatMessage::user(msg.content).build(),
        ChatRole::Assistant => {
            if let Some(tool_calls) = msg.tool_calls
                && !tool_calls.is_empty()
            {
                let mut parts = Vec::with_capacity(tool_calls.len() + 1);
                if !msg.content.is_empty() {
                    parts.push(ContentPart::text(msg.content));
                }
                for tc in tool_calls {
                    let args =
                        serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);
                    parts.push(ContentPart::tool_call(tc.id, tc.name, args, None));
                }
                return SiumaiChatMessage::assistant_with_content(parts).build();
            }
            SiumaiChatMessage::assistant(msg.content).build()
        }
        ChatRole::Tool { tool_call_id } => {
            // Tool result messages: content is the tool output.
            // Tool name is not available at this level, pass empty string.
            SiumaiChatMessage::tool_result_text(tool_call_id, "", msg.content).build()
        }
    }
}

/// Convert a siumai `ChatStreamEvent` to our `ChatEvent`.
///
/// Returns `Ok(None)` for events that should be skipped (e.g., metadata-only events).
/// Returns `Err(LlmError)` for error events so the stream consumer sees a failure.
fn convert_stream_event(
    event: siumai_core::streaming::ChatStreamEvent,
) -> Result<Option<ChatEvent>, LlmError> {
    match event {
        siumai_core::streaming::ChatStreamEvent::Part { part } => Ok(convert_stream_part(part)),
        siumai_core::streaming::ChatStreamEvent::PartWithReplay { part, .. } => {
            Ok(convert_stream_part(part))
        }
        siumai_core::streaming::ChatStreamEvent::StreamEnd { response } => {
            let usage = response.usage.as_ref().and_then(convert_usage);
            let finish_reason = response.finish_reason.as_ref().map(convert_finish_reason);
            Ok(Some(ChatEvent::Finished {
                usage,
                model: response.model,
                finish_reason,
                metadata: serde_json::Map::new(),
            }))
        }
        // Mid-stream error → emit as Err so the stream consumer sees a failure,
        // NOT a Finished event.
        siumai_core::streaming::ChatStreamEvent::Error { error } => Err(LlmError::provider(error)),
        // Skip metadata-only events
        siumai_core::streaming::ChatStreamEvent::StreamStart { .. }
        | siumai_core::streaming::ChatStreamEvent::Custom { .. } => Ok(None),
    }
}

/// Convert a `ChatStreamPart` to our `ChatEvent`, or `None` if skippable.
fn convert_stream_part(part: siumai_core::types::ChatStreamPart) -> Option<ChatEvent> {
    match part {
        siumai_core::types::ChatStreamPart::TextDelta { delta, .. } => {
            Some(ChatEvent::Delta { text: delta })
        }
        siumai_core::types::ChatStreamPart::Finish {
            usage,
            finish_reason,
            ..
        } => {
            let usage_opt = convert_usage(&usage);
            Some(ChatEvent::Finished {
                usage: usage_opt,
                model: None,
                finish_reason: Some(convert_finish_reason(&finish_reason.unified)),
                metadata: serde_json::Map::new(),
            })
        }
        siumai_core::types::ChatStreamPart::ToolCall(call) => Some(ChatEvent::ToolCall {
            id: call.tool_call_id.clone(),
            name: call.tool_name.clone(),
            arguments: call.input.clone(),
        }),
        _ => None,
    }
}

/// Convert siumai `Usage` to our `LlmUsage`.
fn convert_usage(usage: &siumai_core::types::Usage) -> Option<LlmUsage> {
    let prompt = usage.prompt_tokens()?;
    let completion = usage.completion_tokens()?;
    let total = usage.total_tokens()?;
    Some(LlmUsage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: total,
    })
}

/// Convert siumai `FinishReason` to our `FinishReason`.
fn convert_finish_reason(reason: &siumai_core::types::FinishReason) -> FinishReason {
    match reason {
        siumai_core::types::FinishReason::Stop => FinishReason::Stop,
        siumai_core::types::FinishReason::Length => FinishReason::Length,
        siumai_core::types::FinishReason::ToolCalls => FinishReason::ToolCall,
        siumai_core::types::FinishReason::ContentFilter => FinishReason::ContentFilter,
        siumai_core::types::FinishReason::StopSequence => {
            FinishReason::Other("stop_sequence".into())
        }
        siumai_core::types::FinishReason::Error => FinishReason::Other("error".into()),
        siumai_core::types::FinishReason::Unknown => FinishReason::Other("unknown".into()),
        siumai_core::types::FinishReason::Other(s) => FinishReason::Other(s.clone()),
    }
}

/// Convert siumai `EmbeddingResponse` to our `EmbedResponse`.
fn convert_embed_response(response: siumai_core::types::EmbeddingResponse) -> EmbedResponse {
    let usage = response.usage.map(|u| LlmUsage {
        prompt_tokens: u.prompt_tokens,
        completion_tokens: 0,
        total_tokens: u.total_tokens,
    });

    let model = response.model;
    let embeddings = response.embeddings;
    let metadata: serde_json::Map<String, serde_json::Value> =
        response.metadata.into_iter().collect();

    EmbedResponse {
        embeddings,
        usage,
        model,
        metadata,
    }
}

// ============================================================================
// Error mapping — proper taxonomy instead of catch-all
// ============================================================================

/// Map a siumai error to our error taxonomy.
///
/// Maps common error cases to specific `LlmError` variants:
/// - `ApiError { code: 401|403 }` / `AuthenticationError` / `MissingApiKey` → `AuthFailed`
/// - `ApiError { code: 429 }` / `RateLimitError` → `RateLimit`
/// - `QuotaExceededError` → `QuotaExceeded`
/// - `ConnectionError` / `HttpError` → `Network`
/// - `TimeoutError` → `Timeout(configured_timeout)`
/// - `ModelNotSupported` / `NotFound` (model-like) → `ModelNotFound`
/// - `StreamError` → `StreamInterrupted`
/// - `InvalidInput` / `InvalidParameter` → `InvalidRequest`
/// - `JsonError` / `ParseError` → `Protocol`
/// - Everything else → `Provider(String)` (catch-all)
fn map_siumai_error(e: SiumaiLlmError, configured_timeout: Duration) -> LlmError {
    match &e {
        // Auth failures
        SiumaiLlmError::ApiError {
            code: 401 | 403, ..
        }
        | SiumaiLlmError::AuthenticationError(_)
        | SiumaiLlmError::MissingApiKey(_) => LlmError::AuthFailed {
            detail: e.to_string(),
        },

        // Rate limiting
        SiumaiLlmError::ApiError { code: 429, .. } | SiumaiLlmError::RateLimitError(_) => {
            LlmError::RateLimit { retry_after: None }
        }

        // Quota
        SiumaiLlmError::QuotaExceededError(_) => LlmError::QuotaExceeded {
            detail: e.to_string(),
        },

        // Network / connection
        SiumaiLlmError::ConnectionError(_) | SiumaiLlmError::HttpError(_) => {
            LlmError::Network(e.to_string())
        }

        // Timeout — use the configured timeout from provider config
        SiumaiLlmError::TimeoutError(_) => LlmError::Timeout(configured_timeout),

        // Model not found or not supported
        SiumaiLlmError::ModelNotSupported(_) | SiumaiLlmError::NotFound(_) => {
            LlmError::ModelNotFound(e.to_string())
        }

        // Stream errors
        SiumaiLlmError::StreamError(_) => LlmError::StreamInterrupted(e.to_string()),

        // Input validation
        SiumaiLlmError::InvalidInput(_) | SiumaiLlmError::InvalidParameter(_) => {
            LlmError::InvalidRequest(e.to_string())
        }

        // Parse / decode errors
        SiumaiLlmError::JsonError(_) | SiumaiLlmError::ParseError(_) => {
            LlmError::Protocol(e.to_string())
        }

        // Server errors (5xx) — transient, retryable
        SiumaiLlmError::ApiError {
            code: 500..=599, ..
        } => LlmError::ProviderUnavailable(e.to_string()),

        // Provider-specific error
        SiumaiLlmError::ProviderError { message, .. } => LlmError::Provider(message.clone()),

        // Catch-all for remaining variants
        _ => LlmError::provider(e.to_string()),
    }
}

// ============================================================================
// Tests — extracted to separate file to keep main file under 1000 lines
// ============================================================================

#[cfg(all(test, feature = "openai"))]
#[path = "siumai_adapter_tests.rs"]
mod tests;
