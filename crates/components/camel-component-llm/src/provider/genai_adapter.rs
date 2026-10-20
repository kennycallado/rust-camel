//! GenAI adapter — bridges the genai SDK to our `LlmProvider` trait.
//!
//! This is the ONLY file (besides its tests) that may import `genai` types
//! (ADR-0020 boundary). The `LlmProvider` trait and all Camel-shaped
//! request/response types stay SDK-agnostic; everything that knows about
//! `genai` lives here and is translated at this boundary.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use camel_api::SsrfPolicy;
#[cfg(test)]
use futures::future::BoxFuture;
use futures::stream::{BoxStream, StreamExt};

use crate::error::LlmError;
use crate::provider::{
    ChatEvent, ChatMessage, ChatRequest, ChatRole, EmbedRequest, EmbedResponse, FinishReason,
    LlmProvider, LlmUsage, ToolChoice, ToolDefinition,
};
use genai::adapter::AdapterKind;
use genai::chat::{
    ChatRequest as GenaiChatRequest, ChatRole as GenaiChatRole, ChatStreamEvent, ContentPart,
    MessageContent, StopReason, Tool as GenaiTool, ToolCall as GenaiToolCall,
    ToolChoice as GenaiToolChoice, ToolName, ToolResponse as GenaiToolResponse, Usage,
};
use genai::resolver::{AuthData, Endpoint};
use genai::{ModelIden, ModelSpec, ServiceTarget};

// ============================================================================
// Message and tool conversions
// ============================================================================

/// Translate a Camel-shaped message into a genai message.
///
/// Assistant tool calls become genai `ToolCall` content parts and tool-role
/// messages become genai `ToolResponse` parts, preserving the call id.
pub(super) fn map_chat_message(m: &ChatMessage) -> genai::chat::ChatMessage {
    match &m.role {
        ChatRole::System => genai::chat::ChatMessage::system(m.content.clone()),
        ChatRole::User => genai::chat::ChatMessage::user(m.content.clone()),
        ChatRole::Assistant => {
            let mut parts: Vec<ContentPart> = Vec::new();
            if !m.content.is_empty() {
                parts.push(ContentPart::Text(m.content.clone()));
            }
            if let Some(tool_calls) = &m.tool_calls {
                parts.extend(tool_calls.iter().map(|call| {
                    ContentPart::ToolCall(GenaiToolCall {
                        call_id: call.id.clone(),
                        fn_name: call.name.clone(),
                        fn_arguments: serde_json::from_str(&call.arguments)
                            .unwrap_or_else(|_| serde_json::Value::String(call.arguments.clone())),
                        thought_signatures: None,
                    })
                }));
            }
            genai::chat::ChatMessage {
                role: GenaiChatRole::Assistant,
                content: MessageContent::from_parts(parts),
                options: None,
            }
        }
        ChatRole::Tool { tool_call_id } => genai::chat::ChatMessage {
            role: GenaiChatRole::Tool,
            content: MessageContent::from_tool_responses(vec![GenaiToolResponse::new(
                tool_call_id.clone(),
                m.content.clone(),
            )]),
            options: None,
        },
    }
}

/// Translate a Camel-shaped tool definition into a genai tool.
pub(super) fn map_tool_definition(d: ToolDefinition) -> GenaiTool {
    GenaiTool::new(ToolName::from(d.name.as_str()))
        .with_description(d.description)
        .with_schema(serde_json::Value::Object(d.parameters))
}

/// Translate a Camel-shaped tool choice into a genai tool choice.
pub(super) fn map_tool_choice(c: &ToolChoice) -> GenaiToolChoice {
    match c {
        ToolChoice::Auto => GenaiToolChoice::Auto,
        ToolChoice::None => GenaiToolChoice::None,
        ToolChoice::Specific(name) => GenaiToolChoice::Tool { name: name.clone() },
    }
}

/// Build the genai chat request payload (messages, system prompt, tools).
///
/// Sampling knobs (`temperature`, `max_tokens`) and `tool_choice` live on
/// genai's separate `ChatOptions`; see [`build_chat_options`].
pub(super) fn build_chat_request(req: &ChatRequest) -> GenaiChatRequest {
    let messages = req.messages.iter().map(map_chat_message).collect();
    let mut out = GenaiChatRequest::new(messages);

    if let Some(system) = &req.system_prompt {
        out = out.with_system(system.clone());
    }
    if !req.tools.is_empty() {
        out = out.with_tools(req.tools.iter().cloned().map(map_tool_definition));
    }

    out
}

/// Build the genai chat options that accompany [`build_chat_request`].
pub(super) fn build_chat_options(req: &ChatRequest) -> genai::chat::ChatOptions {
    let mut options = genai::chat::ChatOptions {
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        tool_choice: req.tool_choice.as_ref().map(map_tool_choice),
        ..genai::chat::ChatOptions::default()
    };
    if let Some(stop) = &req.stop {
        options.stop_sequences = stop.clone();
    }
    options
}

// ============================================================================
// Usage and finish-reason conversions
// ============================================================================

fn non_negative(value: Option<i32>) -> Option<u32> {
    value.and_then(|n| u32::try_from(n).ok())
}

/// Convert genai chat usage into Camel usage.
///
/// Both prompt and completion counts must be present (and non-negative);
/// `total_tokens` defaults to their sum when the provider omits it.
pub(super) fn convert_usage(u: &Usage) -> Option<LlmUsage> {
    let prompt_tokens = non_negative(u.prompt_tokens)?;
    let completion_tokens = non_negative(u.completion_tokens)?;
    let total_tokens = non_negative(u.total_tokens).unwrap_or(prompt_tokens + completion_tokens);

    Some(LlmUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens,
    })
}

/// Convert embed usage into Camel usage (completion is always zero).
pub(super) fn convert_embed_usage(prompt: Option<i32>, total: Option<i32>) -> Option<LlmUsage> {
    let prompt_tokens = non_negative(prompt)?;
    let total_tokens = non_negative(total).unwrap_or(prompt_tokens);

    Some(LlmUsage {
        prompt_tokens,
        completion_tokens: 0,
        total_tokens,
    })
}

/// Map a genai stop reason to a Camel finish reason.
///
/// `StopSequence` folds into `Stop`: a caller-supplied stop sequence is a
/// natural end of generation from the framework's perspective.
pub(super) fn map_finish_reason(r: &StopReason) -> FinishReason {
    match r {
        StopReason::Completed(_) | StopReason::StopSequence(_) => FinishReason::Stop,
        StopReason::MaxTokens(_) => FinishReason::Length,
        StopReason::ToolCall(_) => FinishReason::ToolCall,
        StopReason::ContentFilter(_) => FinishReason::ContentFilter,
        StopReason::Other(reason) => FinishReason::Other(reason.clone()),
    }
}

// ============================================================================
// Target and endpoint resolution
// ============================================================================

fn ensure_trailing_slash(endpoint: &str) -> String {
    if endpoint.ends_with('/') {
        endpoint.to_string()
    } else {
        format!("{endpoint}/")
    }
}

/// Resolve a complete genai target without name-based adapter inference.
///
/// The endpoint always gains a trailing slash so genai's `base_url + path`
/// joins produce `.../api/chat` rather than `...11434api/chat`.
pub(super) fn resolve_target(
    kind: AdapterKind,
    endpoint: &str,
    api_key: Option<&str>,
    model: &str,
) -> ModelSpec {
    let auth = match api_key {
        Some(key) if !key.is_empty() => AuthData::from_single(key),
        _ => AuthData::None,
    };

    ModelSpec::Target(ServiceTarget {
        endpoint: Endpoint::from_owned(ensure_trailing_slash(endpoint)),
        auth,
        model: ModelIden::new(kind, model),
    })
}

/// OpenAI base URL, defaulting to the public API when no override is given.
#[cfg(any(feature = "openai", feature = "all-providers"))]
pub(super) fn openai_endpoint_for(base_url: Option<&str>) -> String {
    match base_url {
        Some(url) => ensure_trailing_slash(url),
        None => "https://api.openai.com/v1/".to_string(),
    }
}

/// Per-request embedding model, falling back to the provider default.
pub(super) fn embed_model_for(req: &EmbedRequest, default: &str) -> String {
    if req.model.is_empty() {
        default.to_string()
    } else {
        req.model.clone()
    }
}

// ============================================================================
// Error mapping
// ============================================================================

/// Maximum length for body-derived error details.
const MAX_BODY_ERROR_BYTES: usize = 200;

/// Truncate a response body for display, keeping it within
/// `MAX_BODY_ERROR_BYTES` at a UTF-8 boundary.
fn truncate_body(body: &str) -> String {
    if body.len() <= MAX_BODY_ERROR_BYTES {
        body.to_string()
    } else {
        let cut = body.floor_char_boundary(MAX_BODY_ERROR_BYTES);
        format!("{}...[truncated]", &body[..cut])
    }
}

fn map_reqwest_error(err: &reqwest::Error, configured_timeout: Duration) -> LlmError {
    if err.is_timeout() {
        LlmError::Timeout(configured_timeout)
    } else {
        LlmError::Network(err.to_string())
    }
}

fn map_status(
    status: u16,
    body: &str,
    model: Option<&str>,
    retry_after: Option<Duration>,
) -> LlmError {
    match status {
        401 | 403 => LlmError::AuthFailed {
            detail: truncate_body(body),
        },
        404 => LlmError::ModelNotFound(
            model
                .map(str::to_string)
                .unwrap_or_else(|| truncate_body(body)),
        ),
        400 | 422 => LlmError::InvalidRequest(truncate_body(body)),
        429 if is_insufficient_quota(status, body) => LlmError::QuotaExceeded {
            detail: truncate_body(body),
        },
        429 => LlmError::RateLimit { retry_after },
        s if (500..=599).contains(&s) => {
            LlmError::ProviderUnavailable(truncate_body(&format!("HTTP {s}: {body}")))
        }
        _ => LlmError::provider(format!("HTTP {status}: {body}")),
    }
}

/// Detect an OpenAI-style billing quota exhaustion response.
pub(super) fn is_insufficient_quota(status: u16, body: &str) -> bool {
    if status != 429 {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    let Some(error) = value.get("error") else {
        return false;
    };

    error.get("code").and_then(|v| v.as_str()) == Some("insufficient_quota")
        || error.get("type").and_then(|v| v.as_str()) == Some("insufficient_quota")
}

/// Parse a delta-seconds `Retry-After` header; HTTP-date forms are rejected.
pub(super) fn parse_retry_after(value: Option<&str>) -> Option<Duration> {
    let raw = value?.trim();
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    raw.parse::<u64>().ok().map(Duration::from_secs)
}

/// Decompose a `WebStream` error into its boxed transport cause.
fn split_web_stream_error(err: &genai::Error, configured_timeout: Duration) -> Option<LlmError> {
    let genai::Error::WebStream { error, .. } = err else {
        return None;
    };

    if let Some(inner) = error.as_ref().downcast_ref::<genai::Error>() {
        return Some(map_genai_error(inner, configured_timeout));
    }
    if let Some(reqwest_error) = error.as_ref().downcast_ref::<reqwest::Error>() {
        return Some(map_reqwest_error(reqwest_error, configured_timeout));
    }

    Some(LlmError::Network(error.to_string()))
}

/// Decompose `WebModelCall` / `WebAdapterCall` into their webc cause.
fn split_webc_error(err: &genai::Error, configured_timeout: Duration) -> Option<LlmError> {
    let (model, webc_error) = match err {
        genai::Error::WebModelCall {
            model_iden,
            webc_error,
        } => (Some(model_iden.model_name.to_string()), webc_error),
        genai::Error::WebAdapterCall { webc_error, .. } => (None, webc_error),
        _ => return None,
    };

    match webc_error {
        genai::webc::Error::ResponseFailedStatus {
            status,
            body,
            headers,
        } => {
            let retry_after = headers
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| parse_retry_after(Some(value)));
            Some(map_status(
                status.as_u16(),
                body,
                model.as_deref(),
                retry_after,
            ))
        }
        genai::webc::Error::Reqwest(reqwest_error) => {
            Some(map_reqwest_error(reqwest_error, configured_timeout))
        }
        other => Some(LlmError::Network(other.to_string())),
    }
}

/// Map any genai error into the Camel error taxonomy.
pub(super) fn map_genai_error(err: &genai::Error, configured_timeout: Duration) -> LlmError {
    if let Some(mapped) = split_web_stream_error(err, configured_timeout) {
        return mapped;
    }
    if let Some(mapped) = split_webc_error(err, configured_timeout) {
        return mapped;
    }

    match err {
        genai::Error::HttpError { status, body, .. } => {
            map_status(status.as_u16(), body, None, None)
        }
        genai::Error::StreamParse { .. } => LlmError::Protocol(err.to_string()),
        genai::Error::AdapterNotSupported { feature, .. } => {
            LlmError::UnsupportedCapability(feature.clone())
        }
        genai::Error::ChatResponse { body, .. } => {
            LlmError::Protocol(format!("chat response error: {body}"))
        }
        _ => LlmError::provider(err.to_string()),
    }
}

// ============================================================================
// Provider struct and LlmProvider implementation
// ============================================================================

/// Test-only builder: a closure that produces a fully-initialized genai client
/// without network access.
#[cfg(test)]
type BuildOverride =
    Arc<dyn Fn() -> BoxFuture<'static, Result<genai::Client, LlmError>> + Send + Sync>;

/// Provider configuration stored for lazy client construction.
///
/// The genai client is built on first use inside `chat_stream()` or `embed()`
/// so construction stays out of the synchronous builder path.
#[derive(Clone)]
enum GenaiConfig {
    /// OpenAI-compatible provider configuration.
    #[cfg(any(feature = "openai", feature = "all-providers"))]
    OpenAi {
        api_key: String,
        base_url: Option<String>,
    },
    /// Ollama (local) provider configuration.
    #[cfg(any(feature = "ollama", feature = "all-providers"))]
    Ollama { base_url: String },
}

impl GenaiConfig {
    fn adapter_kind(&self) -> AdapterKind {
        match self {
            #[cfg(any(feature = "openai", feature = "all-providers"))]
            GenaiConfig::OpenAi { .. } => AdapterKind::OpenAI,
            #[cfg(any(feature = "ollama", feature = "all-providers"))]
            GenaiConfig::Ollama { .. } => AdapterKind::Ollama,
        }
    }

    /// Endpoint passed to `resolve_target`; `resolve_target` normalises the
    /// trailing slash, so this only selects the public default for OpenAI.
    fn endpoint(&self) -> String {
        match self {
            #[cfg(any(feature = "openai", feature = "all-providers"))]
            GenaiConfig::OpenAi { base_url, .. } => openai_endpoint_for(base_url.as_deref()),
            #[cfg(any(feature = "ollama", feature = "all-providers"))]
            GenaiConfig::Ollama { base_url, .. } => base_url.clone(),
        }
    }

    fn api_key(&self) -> Option<&str> {
        match self {
            #[cfg(any(feature = "openai", feature = "all-providers"))]
            GenaiConfig::OpenAi { api_key, .. } => Some(api_key),
            #[cfg(any(feature = "ollama", feature = "all-providers"))]
            GenaiConfig::Ollama { .. } => None,
        }
    }
}

/// A provider backed by a genai client, constructed lazily and cached.
///
/// The genai client is built exactly once via `tokio::sync::OnceCell`.
/// Concurrent calls share the same cached client.
struct GenaiProvider {
    id: String,
    default_model: String,
    config: GenaiConfig,
    policy: SsrfPolicy,
    configured_timeout: Duration,
    client: Arc<tokio::sync::OnceCell<genai::Client>>,
    #[cfg(test)]
    client_builds: Arc<AtomicUsize>,
    #[cfg(test)]
    build_override: Option<BuildOverride>,
}

impl GenaiProvider {
    fn new(
        id: impl Into<String>,
        default_model: impl Into<String>,
        config: GenaiConfig,
        configured_timeout: Duration,
        policy: SsrfPolicy,
    ) -> Self {
        Self {
            id: id.into(),
            default_model: default_model.into(),
            config,
            policy,
            configured_timeout,
            client: Arc::new(tokio::sync::OnceCell::new()),
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
        config: GenaiConfig,
        configured_timeout: Duration,
        override_: BuildOverride,
        policy: SsrfPolicy,
    ) -> Self {
        Self {
            id: id.into(),
            default_model: default_model.into(),
            config,
            policy,
            configured_timeout,
            client: Arc::new(tokio::sync::OnceCell::new()),
            client_builds: Arc::new(AtomicUsize::new(0)),
            build_override: Some(override_),
        }
    }
}

#[async_trait]
impl LlmProvider for GenaiProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn supports_embed(&self) -> bool {
        true
    }

    fn chat_stream(&self, req: ChatRequest) -> BoxStream<'static, Result<ChatEvent, LlmError>> {
        let config = self.config.clone();
        let default_model = self.default_model.clone();
        let client_cell = Arc::clone(&self.client);
        let configured_timeout = self.configured_timeout;
        let policy = self.policy;
        #[cfg(test)]
        let builds = Arc::clone(&self.client_builds);
        #[cfg(test)]
        let override_ = self.build_override.clone();

        let stream = async_stream::try_stream! {
            let client = client_cell
                .get_or_try_init(|| async {
                    build_cached_client(
                        &config,
                        policy,
                        #[cfg(test)]
                        override_.as_ref(),
                        #[cfg(test)]
                        &builds,
                    )
                    .await
                })
                .await?;

            let model = if req.model.is_empty() {
                default_model.clone()
            } else {
                req.model.clone()
            };

            let target = resolve_target(
                config.adapter_kind(),
                &config.endpoint(),
                config.api_key(),
                &model,
            );

            let mut options = build_chat_options(&req).with_capture_usage(true);
            if !req.tools.is_empty() {
                options = options.with_capture_tool_calls(true);
            }

            let response = client
                .exec_chat_stream(target, build_chat_request(&req), Some(&options))
                .await
                .map_err(|e| map_genai_error(&e, configured_timeout))?;

            let mut stream = response.stream;
            // Per-id buffering: emit at most one ToolCall event per id, in the
            // order the complete calls appear in the captured stream end.
            let mut emitted_ids: HashSet<String> = HashSet::new();
            while let Some(item) = stream.next().await {
                match item {
                    Ok(ChatStreamEvent::Chunk(chunk)) => {
                        yield ChatEvent::Delta { text: chunk.content };
                    }
                    Ok(ChatStreamEvent::End(end)) => {
                        let genai::chat::StreamEnd {
                            captured_usage,
                            captured_stop_reason,
                            captured_content,
                            ..
                        } = end;

                        for call in captured_content
                            .map(MessageContent::into_tool_calls)
                            .unwrap_or_default()
                        {
                            if !emitted_ids.insert(call.call_id.clone()) {
                                continue;
                            }
                            yield ChatEvent::ToolCall {
                                id: call.call_id,
                                name: call.fn_name,
                                arguments: tool_arguments_to_string(&call.fn_arguments),
                            };
                        }

                        yield ChatEvent::Finished {
                            usage: captured_usage.as_ref().and_then(convert_usage),
                            model: Some(model.clone()),
                            finish_reason: captured_stop_reason.as_ref().map(map_finish_reason),
                            metadata: serde_json::Map::new(),
                        };
                    }
                    // Reasoning-only, thought-signature and raw tool-call
                    // chunks are not part of the Camel event surface.
                    Ok(_) => {}
                    // Mid-stream error: emit a mapped Err and terminate without
                    // a trailing Finished event.
                    Err(e) => {
                        Err(map_genai_error(&e, configured_timeout))?;
                    }
                }
            }
        };

        stream.boxed()
    }

    async fn embed(&self, req: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        let config = self.config.clone();
        let configured_timeout = self.configured_timeout;
        let policy = self.policy;
        #[cfg(test)]
        let builds = Arc::clone(&self.client_builds);
        #[cfg(test)]
        let override_ = self.build_override.clone();

        let client = self
            .client
            .get_or_try_init(|| async {
                build_cached_client(
                    &config,
                    policy,
                    #[cfg(test)]
                    override_.as_ref(),
                    #[cfg(test)]
                    &builds,
                )
                .await
            })
            .await?;

        let model = embed_model_for(&req, &self.default_model);
        let target = resolve_target(
            config.adapter_kind(),
            &config.endpoint(),
            config.api_key(),
            &model,
        );

        let response = client
            .embed_batch(target, req.inputs, None)
            .await
            .map_err(|e| map_genai_error(&e, configured_timeout))?;

        let usage = convert_embed_usage(response.usage.prompt_tokens, response.usage.total_tokens);
        // genai maps response embeddings 1:1 and in request order; preserve it.
        let embeddings = response
            .embeddings
            .into_iter()
            .map(|embedding| embedding.vector)
            .collect();

        Ok(EmbedResponse {
            embeddings,
            usage,
            model,
            metadata: serde_json::Map::new(),
        })
    }
}

/// Convert a genai tool-call argument value into the Camel string form.
///
/// A JSON string is passed through verbatim; every other value is serialised.
fn tool_arguments_to_string(arguments: &serde_json::Value) -> String {
    match arguments {
        serde_json::Value::String(raw) => raw.clone(),
        other => other.to_string(),
    }
}

// ============================================================================
// Helper: client construction
// ============================================================================

/// Build a genai client, respecting the test-only override when present.
///
/// This is the single point of client construction used by both `chat_stream`
/// and `embed`. Keeping it as a free function (not a method) allows it to be
/// called inside `async_stream::try_stream!` without borrowing `self`.
async fn build_cached_client(
    config: &GenaiConfig,
    policy: SsrfPolicy,
    #[cfg(test)] override_: Option<&BuildOverride>,
    #[cfg(test)] builds: &AtomicUsize,
) -> Result<genai::Client, LlmError> {
    #[cfg(test)]
    {
        builds.fetch_add(1, Ordering::SeqCst);
        if let Some(builder) = override_ {
            return builder().await;
        }
    }
    build_client_from_config(config, policy).await
}

/// Validate an LLM `base_url` for SSRF and return resolved addresses for
/// DNS-rebinding pinning (D-M10).
///
/// DNS resolution is fail-closed and delegated to
/// `validate_llm_url_pinned` in `provider_factory.rs`.
async fn validate_base_url_ssrf(
    base_url: &str,
    policy: SsrfPolicy,
) -> Result<(String, Vec<std::net::SocketAddr>), LlmError> {
    let url = base_url.to_string();
    tokio::task::spawn_blocking(move || {
        crate::provider_factory::validate_llm_url_pinned(&url, policy)
    })
    .await
    .map_err(|e| {
        LlmError::InvalidRequest(format!("llm base_url SSRF validation task failed: {e}"))
    })?
}

/// Returns `true` if `url_str` parses as an HTTP(S) URL with a domain hostname
/// (as opposed to an IP literal). Only domain hostnames need DNS pinning.
fn is_hostname_domain(url_str: &str) -> bool {
    if let Ok(parsed) = url::Url::parse(url_str) {
        matches!(parsed.host(), Some(url::Host::Domain(_)))
    } else {
        false
    }
}

/// Collect DNS-pinning data from a `GenaiConfig` by validating the base URL
/// and resolving its hostname. Returns `None` when the config has no base_url
/// or when the host is an IP literal (no TOCTOU window).
async fn collect_pinning_from_config(
    config: &GenaiConfig,
    policy: SsrfPolicy,
) -> Result<Option<(String, Vec<std::net::SocketAddr>)>, LlmError> {
    match config {
        #[cfg(any(feature = "openai", feature = "all-providers"))]
        GenaiConfig::OpenAi {
            base_url: Some(url),
            ..
        } => {
            let (host, addrs) = validate_base_url_ssrf(url, policy).await?;
            if is_hostname_domain(url) {
                Ok(Some((host, addrs)))
            } else {
                Ok(None)
            }
        }
        #[cfg(any(feature = "openai", feature = "all-providers"))]
        GenaiConfig::OpenAi { base_url: None, .. } => Ok(None),
        #[cfg(any(feature = "ollama", feature = "all-providers"))]
        GenaiConfig::Ollama { base_url, .. } => {
            let (host, addrs) = validate_base_url_ssrf(base_url, policy).await?;
            if is_hostname_domain(base_url) {
                Ok(Some((host, addrs)))
            } else {
                Ok(None)
            }
        }
    }
}

/// Build a fresh genai client from config.
///
/// 1. **Validate & resolve** the base URL for SSRF, collecting pinned
///    addresses for DNS-rebinding protection (D-M10).
/// 2. **Build the hardened reqwest::Client** with `resolve_to_addrs` to pin
///    the validated IPs.
/// 3. **Construct the genai client** with the pinned reqwest client.
async fn build_client_from_config(
    config: &GenaiConfig,
    policy: SsrfPolicy,
) -> Result<genai::Client, LlmError> {
    let pinning = collect_pinning_from_config(config, policy).await?;

    let hardened = match &pinning {
        Some((host, addrs)) => {
            crate::hardened_http_client_with_pinning(Some((host.as_str(), addrs.as_slice())))
        }
        None => crate::hardened_http_client(),
    }
    .map_err(|e| LlmError::InvalidRequest(format!("llm hardened client: {e}")))?;

    Ok(genai::Client::builder().with_reqwest(hardened).build())
}

// ============================================================================
// Builder functions — synchronous, no block_on
// ============================================================================

/// Build an OpenAI-backed `LlmProvider`.
///
/// The genai client is constructed lazily on first use — no `block_on` here.
#[cfg(any(feature = "openai", feature = "all-providers"))]
pub fn build_openai(
    name: &str,
    config: &crate::config::OpenaiProviderConfig,
    configured_timeout: Duration,
    policy: SsrfPolicy,
) -> Result<Arc<dyn LlmProvider>, LlmError> {
    let cfg = GenaiConfig::OpenAi {
        api_key: config.api_key.clone(),
        base_url: config.base_url.clone(),
    };
    Ok(Arc::new(GenaiProvider::new(
        name,
        &config.default_model,
        cfg,
        configured_timeout,
        policy,
    )))
}

/// Build an Ollama-backed `LlmProvider`.
///
/// The genai client is constructed lazily on first use — no `block_on` here.
#[cfg(any(feature = "ollama", feature = "all-providers"))]
pub fn build_ollama(
    name: &str,
    config: &crate::config::OllamaProviderConfig,
    configured_timeout: Duration,
    policy: SsrfPolicy,
) -> Result<Arc<dyn LlmProvider>, LlmError> {
    let cfg = GenaiConfig::Ollama {
        base_url: config.base_url.clone(),
    };
    Ok(Arc::new(GenaiProvider::new(
        name,
        &config.default_model,
        cfg,
        configured_timeout,
        policy,
    )))
}

#[cfg(all(test, any(feature = "openai", feature = "ollama")))]
#[path = "genai_adapter_tests.rs"]
mod tests;
