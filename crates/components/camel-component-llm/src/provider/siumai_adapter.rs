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

use std::sync::Arc;

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
use crate::provider::{
    ChatEvent, ChatMessage, ChatRequest, ChatRole, EmbedRequest, EmbedResponse, FinishReason,
    LlmProvider, LlmUsage,
};

use siumai_core::builder::BuilderBase;
use siumai_core::error::LlmError as SiumaiLlmError;
#[cfg(test)]
use siumai_core::traits::EmbeddingExtensions;
use siumai_core::traits::{ChatCapability, EmbeddingCapability};
use siumai_core::types::{
    ChatMessage as SiumaiChatMessage, ChatRequest as SiumaiChatRequest, CommonParams,
    EmbeddingRequest as SiumaiEmbeddingRequest,
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
    #[cfg(test)]
    client_builds: Arc<AtomicUsize>,
    #[cfg(test)]
    build_override: Option<BuildOverride>,
}

impl SiumaiProvider {
    fn new(id: impl Into<String>, default_model: impl Into<String>, config: SiumaiConfig) -> Self {
        Self {
            id: id.into(),
            default_model: default_model.into(),
            config,
            client: Arc::new(OnceCell::new()),
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
        override_: BuildOverride,
    ) -> Self {
        Self {
            id: id.into(),
            default_model: default_model.into(),
            config,
            client: Arc::new(OnceCell::new()),
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
        #[cfg(test)]
        let builds = Arc::clone(&self.client_builds);
        #[cfg(test)]
        let override_ = self.build_override.clone();

        let stream = async_stream::try_stream! {
            // Build/retrieve cached client
            let cached = client_cell.get_or_try_init(|| async {
                build_cached_client(
                    &config,
                    #[cfg(test)] override_.as_ref(),
                    #[cfg(test)] &builds,
                ).await
            }).await?;
            let chat = Arc::clone(&cached.chat);

            let siumai_req = convert_chat_request(req, &default_model);

            let siumai_stream = chat
                .chat_stream_request(siumai_req)
                .await
                .map_err(map_siumai_error)?;

            let mut stream = siumai_stream;
            while let Some(result) = stream.next().await {
                match result {
                    Ok(event) => match convert_stream_event(event) {
                        Ok(Some(our_event)) => yield our_event,
                        Ok(None) => {} // skip metadata-only events
                        Err(e) => Err(e)?,
                    },
                    Err(e) => {
                        Err(map_siumai_error(e))?;
                    }
                }
            }
        };

        stream.boxed()
    }

    async fn embed(&self, req: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        let config = self.config.clone();
        #[cfg(test)]
        let builds = Arc::clone(&self.client_builds);
        #[cfg(test)]
        let override_ = self.build_override.clone();

        let cached = self
            .client
            .get_or_try_init(|| async {
                build_cached_client(
                    &config,
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
            .map_err(map_siumai_error)?;

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
    let (chat, embed) = build_client_from_config(config).await?;
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
) -> Result<(Arc<dyn ChatCapability>, Arc<dyn EmbeddingCapability>), LlmError> {
    match config {
        #[cfg(any(feature = "openai", feature = "all-providers"))]
        SiumaiConfig::OpenAi {
            api_key,
            base_url,
            model,
        } => {
            let mut builder = siumai_provider_openai::providers::openai::OpenAiBuilder::new(
                BuilderBase::default(),
            );
            builder = builder.api_key(api_key).model(model);
            if let Some(url) = base_url {
                builder = builder.base_url(url);
            }
            let client = builder.build().await.map_err(map_siumai_error)?;
            let client = Arc::new(client);
            let chat: Arc<dyn ChatCapability> = client.clone();
            let embed: Arc<dyn EmbeddingCapability> = client;
            Ok((chat, embed))
        }
        #[cfg(any(feature = "ollama", feature = "all-providers"))]
        SiumaiConfig::Ollama { base_url, model } => {
            let mut builder = siumai_provider_ollama::providers::ollama::OllamaBuilder::new(
                BuilderBase::default(),
            );
            builder = builder.base_url(base_url).model(model);
            let client = builder.build().await.map_err(map_siumai_error)?;
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
    )))
}

/// Build an Ollama-backed `LlmProvider`.
///
/// The siumai client is constructed lazily on first use — no `block_on` here.
#[cfg(any(feature = "ollama", feature = "all-providers"))]
pub fn build_ollama(
    name: &str,
    config: &OllamaProviderConfig,
) -> Result<Arc<dyn LlmProvider>, LlmError> {
    let cfg = SiumaiConfig::Ollama {
        base_url: config.base_url.clone(),
        model: config.default_model.clone(),
    };
    Ok(Arc::new(SiumaiProvider::new(
        name,
        &config.default_model,
        cfg,
    )))
}

// ============================================================================
// Type conversions
// ============================================================================

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
        ChatRole::Assistant => SiumaiChatMessage::assistant(msg.content).build(),
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
/// - `TimeoutError` → `Timeout`
/// - `ModelNotSupported` / `NotFound` (model-like) → `ModelNotFound`
/// - `StreamError` → `StreamInterrupted`
/// - `InvalidInput` / `InvalidParameter` → `InvalidRequest`
/// - `JsonError` / `ParseError` → `Protocol`
/// - Everything else → `Provider(String)` (catch-all)
fn map_siumai_error(e: SiumaiLlmError) -> LlmError {
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

        // Timeout — siumai doesn't expose the elapsed duration, so we cannot
        // populate LlmError::Timeout(duration) honestly. Map to Network
        // instead: both are retryable, and "network error" avoids the
        // misleading "timeout after 0s" Display.
        SiumaiLlmError::TimeoutError(_) => LlmError::Network("provider request timed out".into()),

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
// Tests
// ============================================================================

#[cfg(all(test, feature = "openai"))]
mod tests {
    use super::*;
    use futures::future::join_all;
    use siumai_core::error::LlmError as SiumaiLlmError;
    use siumai_core::streaming::ChatStream;
    use siumai_core::types::{
        ChatMessage as SiumaiChatMessage, ChatResponse, EmbeddingResponse, Tool,
    };

    /// Stub chat capability — never called by `embed()` but must be constructable
    /// so it can be placed in `CachedClient` inside the build override.
    struct StubChat;

    #[async_trait::async_trait]
    impl ChatCapability for StubChat {
        async fn chat_with_tools(
            &self,
            _messages: Vec<SiumaiChatMessage>,
            _tools: Option<Vec<Tool>>,
        ) -> Result<ChatResponse, SiumaiLlmError> {
            unreachable!("StubChat should not be called in OnceCell dedup test")
        }

        async fn chat_stream(
            &self,
            _messages: Vec<SiumaiChatMessage>,
            _tools: Option<Vec<Tool>>,
        ) -> Result<ChatStream, SiumaiLlmError> {
            unreachable!("StubChat should not be called in OnceCell dedup test")
        }
    }

    /// Stub embedding capability — returns a valid embedding response so the
    /// production `embed()` method can complete successfully.
    struct StubEmbed;

    #[async_trait::async_trait]
    impl EmbeddingCapability for StubEmbed {
        async fn embed(&self, _input: Vec<String>) -> Result<EmbeddingResponse, SiumaiLlmError> {
            Ok(EmbeddingResponse::new(
                vec![vec![1.0, 2.0, 3.0]],
                "stub-model".into(),
            ))
        }

        fn as_embedding_extensions(&self) -> Option<&dyn EmbeddingExtensions> {
            Some(self)
        }

        fn embedding_dimension(&self) -> usize {
            3
        }
    }

    #[async_trait::async_trait]
    impl EmbeddingExtensions for StubEmbed {}

    /// Verifies that the production `embed()` method constructs the client
    /// exactly once under 8-way concurrency.
    ///
    /// Uses a build override to avoid network I/O while exercising the real
    /// `build_cached_client` → `OnceCell::get_or_try_init` code path.
    #[tokio::test]
    async fn client_built_once_under_concurrency() {
        let stub_builder: BuildOverride = Arc::new(|| {
            Box::pin(async {
                Ok::<_, LlmError>((
                    Arc::new(StubChat) as Arc<dyn ChatCapability>,
                    Arc::new(StubEmbed) as Arc<dyn EmbeddingCapability>,
                ))
            })
        });

        let provider = Arc::new(SiumaiProvider::new_with_build_override(
            "test",
            "gpt-4",
            SiumaiConfig::OpenAi {
                api_key: "sk-test".into(),
                base_url: Some("http://127.0.0.1:1".into()),
                model: "gpt-4".into(),
            },
            stub_builder,
        ));
        let builds = Arc::clone(&provider.client_builds);

        // 8 concurrent calls to the PRODUCTION `embed()` method — exercises
        // the real `OnceCell::get_or_try_init` path inside `build_cached_client`.
        let mut futs = vec![];
        for _ in 0..8 {
            let p = Arc::clone(&provider);
            futs.push(async move {
                let _ = p
                    .embed(EmbedRequest::new("gpt-4", vec!["hello".into()]))
                    .await;
            });
        }
        join_all(futs).await;
        assert_eq!(
            builds.load(Ordering::SeqCst),
            1,
            "OnceCell must construct the client exactly once across concurrent embed() calls"
        );
    }
}
