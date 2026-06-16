use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use async_stream::stream;
use bytes::Bytes;
use camel_api::{Body, CamelError, Exchange, StreamBody, StreamMetadata};
use camel_component_api::NetworkRetryPolicy;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::sync::Semaphore;
use tower::Service;

use crate::config::{LlmEndpointConfig, LlmOperation};
use crate::cost::PricingTable;
use crate::error::LlmError;
use crate::error::is_retryable;
use crate::headers::*;
use crate::producer_cache::{self as cache_mod, ProducerCache, Slot, canonical_key};
use crate::provider::{
    ChatEvent, ChatMessage, ChatRequest, ChatRole, EmbedRequest, EmittedToolCall, LlmProvider,
    ToolChoice, ToolDefinition,
};

#[derive(Clone)]
pub struct LlmProducer {
    config: LlmEndpointConfig,
    provider: Arc<dyn LlmProvider>,
    max_prompt_bytes: usize,
    route_id: String,
    /// Semaphore to bound concurrency to the provider. If `None`, unbounded.
    /// For streaming, the permit lives inside the returned `Body::Stream` so
    /// that max_concurrency is real for streaming — not vacuous (see ADR-0021).
    semaphore: Option<Arc<Semaphore>>,
    /// Total deadline for the entire operation (materialized) or per-`next()`
    /// activity timeout (streaming). `None` means no timeout enforcement.
    timeout: Option<Duration>,
    /// Optional network retry policy for transient failures.
    /// Only applies to materialized mode (chat + embed).
    retry: Option<NetworkRetryPolicy>,
    /// Optional pricing table for cost estimation.
    /// When present and usage is available, computes estimated cost.
    pricing: Option<Arc<PricingTable>>,
    /// Optional producer-level response cache (materialized-only).
    /// Lookup happens BEFORE semaphore/retry/timeout.
    /// Single-flight waiters hold zero permits.
    cache: Option<Arc<ProducerCache>>,
}

impl LlmProducer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: LlmEndpointConfig,
        provider: Arc<dyn LlmProvider>,
        max_prompt_bytes: usize,
        route_id: String,
        semaphore: Option<Arc<Semaphore>>,
        timeout: Option<Duration>,
        retry: Option<NetworkRetryPolicy>,
        pricing: Option<Arc<PricingTable>>,
        cache: Option<Arc<ProducerCache>>,
    ) -> Self {
        Self {
            config,
            provider,
            max_prompt_bytes,
            route_id,
            semaphore,
            timeout,
            retry,
            pricing,
            cache,
        }
    }

    fn build_chat_request(
        &self,
        prompt: &str,
        exchange: &Exchange,
    ) -> Result<ChatRequest, LlmError> {
        let headers = &exchange.input.headers;

        let model = headers
            .get(CAMEL_LLM_MODEL)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| self.config.model.clone())
            .unwrap_or_else(|| self.provider.default_model().to_string());

        let temperature = headers
            .get(CAMEL_LLM_TEMPERATURE)
            .and_then(|v| v.as_f64())
            .or(self.config.temperature);

        let max_tokens = headers
            .get(CAMEL_LLM_MAX_TOKENS)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .or(self.config.max_tokens);

        let system_prompt = headers
            .get(CAMEL_LLM_SYSTEM_PROMPT)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| self.config.system_prompt.clone());

        let messages = if let Some(msgs_val) = headers.get(CAMEL_LLM_MESSAGES) {
            serde_json::from_value(msgs_val.clone()).map_err(|e| {
                LlmError::InvalidRequest(format!("CamelLlmMessages header is malformed: {e}"))
            })?
        } else {
            vec![ChatMessage {
                role: ChatRole::User,
                content: prompt.to_string(),
                tool_calls: None,
            }]
        };

        let tools: Vec<ToolDefinition> = headers
            .get(CAMEL_LLM_TOOLS)
            .map(|v| {
                serde_json::from_value(v.clone()).map_err(|e| {
                    LlmError::InvalidRequest(format!("CamelLlmTools header is malformed: {e}"))
                })
            })
            .transpose()?
            .unwrap_or_default();

        for (i, tool) in tools.iter().enumerate() {
            if tool.name.is_empty() {
                return Err(LlmError::InvalidRequest(format!(
                    "tool at index {i} has empty name"
                )));
            }
        }

        let tool_choice: Option<ToolChoice> = headers
            .get(CAMEL_LLM_TOOL_CHOICE)
            .map(|v| {
                serde_json::from_value(v.clone()).map_err(|e| {
                    LlmError::InvalidRequest(format!("CamelLlmToolChoice header is malformed: {e}"))
                })
            })
            .transpose()?;

        Ok(ChatRequest {
            model,
            messages,
            temperature,
            max_tokens,
            stop: None,
            system_prompt,
            tools,
            tool_choice,
            extra: serde_json::Map::new(),
        })
    }

    fn extract_prompt(&self, exchange: &Exchange) -> Result<String, CamelError> {
        let prompt = match &exchange.input.body {
            Body::Text(s) => s.clone(),
            Body::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            Body::Json(v) => v.to_string(),
            Body::Empty => {
                return Err(CamelError::ProcessorError(
                    "exchange body is empty — cannot use as LLM prompt".into(),
                ));
            }
            Body::Xml(s) => s.clone(),
            Body::Stream(_) => {
                return Err(CamelError::ProcessorError(
                    "stream body must be materialized before sending to LLM producer".into(),
                ));
            }
        };

        if prompt.len() > self.max_prompt_bytes {
            return Err(CamelError::ProcessorError(format!(
                "prompt exceeds max_prompt_bytes ({} > {})",
                prompt.len(),
                self.max_prompt_bytes
            )));
        }
        Ok(prompt)
    }

    fn set_start_headers(&self, exchange: &mut Exchange) {
        let headers = &mut exchange.input.headers;
        headers.insert(
            CAMEL_LLM_PROVIDER.to_string(),
            Value::String(self.provider.id().to_string()),
        );
        headers.insert(
            CAMEL_LLM_STREAM.to_string(),
            Value::Bool(self.config.stream),
        );
        headers.insert(CAMEL_LLM_USAGE_AVAILABLE.to_string(), Value::Bool(false));
        if let Some(model) = &self.config.model {
            headers.insert(CAMEL_LLM_MODEL.to_string(), Value::String(model.clone()));
        }
    }

    /// Wrap a future with the configured total deadline timeout.
    /// When `self.timeout` is `None`, runs without wrapping.
    ///
    /// ## Composition: single total deadline (no nested timeouts)
    /// This helper wraps ONCE. The retry loop is wrapped inside
    /// `with_timeout`, so do NOT nest timeouts (see ADR-0021).
    async fn with_timeout<F, T>(&self, fut: F) -> Result<T, LlmError>
    where
        F: std::future::Future<Output = Result<T, LlmError>>,
    {
        match self.timeout {
            Some(d) => match tokio::time::timeout(d, fut).await {
                Ok(inner) => inner,
                Err(_) => Err(LlmError::Timeout(d)),
            },
            None => fut.await,
        }
    }

    /// Manual retry loop that honors `RateLimit.retry_after` over exponential
    /// backoff. Only runs when `retry` is `Some`. Does NOT retry after
    /// content-start (first non-empty Delta).
    ///
    /// ## Composition
    /// - NO inner timeout — the total deadline wraps `run_with_retry`
    ///   from outside, so one deadline covers all attempts + backoff.
    /// - Semaphore permit is acquired PER ATTEMPT and released during backoff
    ///   sleep (see ADR-0021).
    ///
    /// This is an associated function (not `&self`) so the returned future
    /// does not borrow `self`, allowing it to be passed to `with_timeout`.
    async fn run_with_retry<F, Fut, T>(
        semaphore: Option<Arc<Semaphore>>,
        retry: Option<NetworkRetryPolicy>,
        content_started: Arc<AtomicBool>,
        route_id: &str,
        mut op: F,
    ) -> Result<T, LlmError>
    where
        F: FnMut(Arc<AtomicBool>) -> Fut,
        Fut: std::future::Future<Output = Result<T, LlmError>>,
    {
        let policy = retry;
        let mut attempt: u32 = 0;
        loop {
            // Acquire permit per attempt — released during backoff (dropped
            // before sleep). Using .ok() is intentional (lint-unwrap gate).
            // Permit is acquired even when retry is disabled (semaphore lives
            // outside the retry decision).
            let _permit = match &semaphore {
                Some(sem) => sem.clone().acquire_owned().await.ok(),
                None => None,
            };
            let result = op(Arc::clone(&content_started)).await;
            match result {
                Ok(v) => return Ok(v),
                Err(e) => {
                    // No retry policy configured — surface error immediately.
                    let Some(ref policy) = policy else {
                        return Err(e);
                    };
                    // Do not retry after content-started (first non-empty Delta).
                    if content_started.load(Ordering::SeqCst) {
                        return Err(e);
                    }
                    // 0-indexed: attempt 0 = first try. should_retry(attempt+1) per ADR-0013.
                    if !is_retryable(&e) || !policy.should_retry(attempt + 1) {
                        return Err(e);
                    }
                    // Compute delay: retry_after wins over exponential backoff.
                    let delay = match &e {
                        LlmError::RateLimit {
                            retry_after: Some(ra),
                        } => *ra,
                        _ => policy.delay_for(attempt),
                    };
                    tracing::warn!(
                        route_id = %route_id,
                        attempt,
                        delay_ms = delay.as_millis() as u64,
                        error = %e,
                        "llm: transient error — retrying"
                    );
                    // Release permit during backoff (dropped here; re-acquired next loop).
                    drop(_permit);
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
            }
        }
    }

    async fn handle_chat(&self, exchange: &mut Exchange) -> Result<(), LlmError> {
        let prompt = self
            .extract_prompt(exchange)
            .map_err(|e| LlmError::InvalidRequest(e.to_string()))?;
        let request = self.build_chat_request(&prompt, exchange)?;
        self.set_start_headers(exchange);

        // Semaphore permit:
        //   - Streaming: acquired BEFORE with_timeout (by design — streaming
        //     uses per-next() activity timeout, not total deadline). The
        //     permit is moved into PermitStream so it lives until the byte
        //     stream is fully consumed or dropped (see ADR-0021).
        //   - Materialized: acquired INSIDE with_timeout so the permit wait
        //     is covered by the total deadline timeout (review finding).
        // The semaphore is never explicitly closed in this crate; .ok() is
        // defensive only — a closed semaphore would mean the producer is
        // shutting down, so allowing the call through (fail-open) is
        // acceptable.
        if self.config.stream {
            // Acquire semaphore permit before building the stream. Wrapped in
            // the configured timeout so a saturated semaphore cannot block
            // indefinitely — the streaming activity timeout (per-next()) only
            // starts AFTER the stream exists, so the permit wait needs its own
            // bound.
            let permit = match &self.semaphore {
                Some(sem) => {
                    let acquired = sem.clone().acquire_owned();
                    match self.timeout {
                        Some(d) => match tokio::time::timeout(d, acquired).await {
                            Ok(Ok(p)) => Some(p),
                            Ok(Err(_)) => None, // semaphore closed — fail-open
                            Err(_) => return Err(LlmError::Timeout(d)),
                        },
                        None => acquired.await.ok(),
                    }
                }
                None => None,
            };

            let route_id = self.route_id.clone();
            let stream = self.provider.chat_stream(request);
            let timeout = self.timeout;
            let pricing = self.pricing.clone();

            let byte_stream = stream! {
                let mut s = stream;
                loop {
                    let next = match timeout {
                        Some(d) => match tokio::time::timeout(d, s.next()).await {
                            Ok(Some(ev)) => Some(ev),
                            Ok(None) => None,
                            Err(_) => {
                                yield Err(CamelError::from(LlmError::Timeout(d)));
                                return;
                            }
                        },
                        None => s.next().await,
                    };
                    let mapped = match next {
                        Some(Ok(ChatEvent::Delta { text })) => Ok(Bytes::from(text)),
                        Some(Ok(ChatEvent::ToolCall {
                            id,
                            name,
                            arguments,
                        })) => {
                            tracing::info!(
                                route_id = %route_id,
                                tool_name = %name,
                                "llm tool call emitted"
                            );
                            let obj = serde_json::json!({
                                "type": "tool_call",
                                "id": id,
                                "name": name,
                                "arguments": arguments,
                            });
                            let json = serde_json::to_string(&obj).unwrap(); // allow-unwrap: json!() Value is always serializable
                            Ok(Bytes::from(json))
                        }
                        Some(Ok(ChatEvent::Finished { usage, .. })) => {
                            if let Some(u) = usage {
                                tracing::info!(
                                    route_id = %route_id,
                                    prompt_tokens = u.prompt_tokens,
                                    completion_tokens = u.completion_tokens,
                                    "llm stream finished"
                                );
                                if let Some(cost) = pricing
                                    .as_ref()
                                    .and_then(|p| p.cost_usd(u.prompt_tokens, u.completion_tokens))
                                {
                                    tracing::info!(
                                        route_id = %route_id,
                                        cost_usd = cost,
                                        "llm stream cost estimated"
                                    );
                                }
                            }
                            Ok(Bytes::new())
                        }
                        Some(Err(e)) => {
                            // log-policy: handler-owned
                            tracing::warn!(route_id = %route_id, error = %e, "llm stream error");
                            Err(CamelError::from(e))
                        }
                        None => return,
                    };
                    yield mapped;
                }
            };

            // Wrap the byte stream in PermitStream so the semaphore permit
            // outlives `call()` — it is released when the Body::Stream is
            // consumed / dropped.
            let with_permit = PermitStream {
                inner: byte_stream.boxed(),
                _permit: permit,
            };

            exchange.input.body = Body::Stream(StreamBody {
                stream: Arc::new(Mutex::new(Some(Box::pin(with_permit)))),
                metadata: StreamMetadata::default(),
            });
        } else {
            // Materialized mode: compose retry loop inside total deadline.
            // The retry loop acquires its own permit per attempt (released
            // during backoff) and is wrapped by with_timeout so one total
            // deadline covers all attempts + backoff.
            //
            // Cache integration: lookup BEFORE semaphore/retry/timeout.
            // Single-flight waiters hold ZERO permits.
            // -----------------------------------------------------------------
            // Cache key computation (needed before cache check and before
            // the provider-work closure that captures `request`).
            // Skip cache entirely when the request has tools — tool-call
            // responses are actionable intents, not idempotent text, and
            // must NOT be stored or replayed from cache.
            let cache = if request.tools.is_empty() {
                self.cache.as_ref().map(|c| {
                    let key = canonical_key(self.provider.id(), &request);
                    let cache_ref = Arc::clone(c);
                    (cache_ref, key)
                })
            } else {
                tracing::debug!(
                    route_id = %self.route_id,
                    "llm: skipping cache because request has tools"
                );
                None
            };

            let mut leader_handle: Option<crate::producer_cache::LeaderHandle> = None;

            if let Some((ref cache, ref key)) = cache {
                // Cache hit — set body/headers from cached entry, return early.
                if let Some((text, usage)) = cache.get(key) {
                    exchange.input.body = Body::Text(text);
                    let headers = &mut exchange.input.headers;
                    headers.insert(CAMEL_LLM_USAGE_AVAILABLE.to_string(), Value::Bool(true));
                    if let Some(u) = usage {
                        headers.insert(
                            CAMEL_LLM_TOKENS_IN.to_string(),
                            Value::from(u.prompt_tokens),
                        );
                        headers.insert(
                            CAMEL_LLM_TOKENS_OUT.to_string(),
                            Value::from(u.completion_tokens),
                        );
                        if let Some(ref pricing) = self.pricing
                            && let Some(cost) =
                                pricing.cost_usd(u.prompt_tokens, u.completion_tokens)
                        {
                            headers.insert(
                                CAMEL_LLM_ESTIMATED_COST_USD.to_string(),
                                Value::from(cost),
                            );
                            tracing::info!(
                                route_id = %self.route_id,
                                prompt_tokens = u.prompt_tokens,
                                completion_tokens = u.completion_tokens,
                                cost_usd = cost,
                                "llm cache hit cost estimated"
                            );
                        }
                    }
                    tracing::debug!(route_id = %self.route_id, "llm cache hit");
                    return Ok(());
                }

                // Cache miss: acquire single-flight slot.
                match Arc::clone(cache).acquire(key) {
                    Slot::Waiter { mut rx } => {
                        tracing::debug!(route_id = %self.route_id, "llm cache waiter");
                        let entry = match self.timeout {
                            Some(d) => tokio::time::timeout(d, cache_mod::wait(&mut rx))
                                .await
                                .map_err(|_| LlmError::Timeout(d))?,
                            None => cache_mod::wait(&mut rx).await,
                        }?;
                        // Apply the leader's cached result.
                        exchange.input.body = Body::Text(entry.text.clone());
                        let headers = &mut exchange.input.headers;
                        headers.insert(CAMEL_LLM_USAGE_AVAILABLE.to_string(), Value::Bool(true));
                        if let Some(u) = entry.usage {
                            headers.insert(
                                CAMEL_LLM_TOKENS_IN.to_string(),
                                Value::from(u.prompt_tokens),
                            );
                            headers.insert(
                                CAMEL_LLM_TOKENS_OUT.to_string(),
                                Value::from(u.completion_tokens),
                            );
                            if let Some(ref pricing) = self.pricing
                                && let Some(cost) =
                                    pricing.cost_usd(u.prompt_tokens, u.completion_tokens)
                            {
                                headers.insert(
                                    CAMEL_LLM_ESTIMATED_COST_USD.to_string(),
                                    Value::from(cost),
                                );
                                tracing::info!(
                                    route_id = %self.route_id,
                                    prompt_tokens = u.prompt_tokens,
                                    completion_tokens = u.completion_tokens,
                                    cost_usd = cost,
                                    "llm cache waiter cost estimated"
                                );
                            }
                        }
                        return Ok(());
                    }
                    Slot::Leader(handle) => {
                        tracing::debug!(route_id = %self.route_id, "llm cache leader");
                        leader_handle = Some(handle);
                    }
                }
            }

            // Provider work (leader path, or no-cache path when cache is None).
            let provider = Arc::clone(&self.provider);
            let content_started = Arc::new(AtomicBool::new(false));
            let route_id = self.route_id.clone();

            let op = move |cs: Arc<AtomicBool>| {
                let req = request.clone();
                let p = Arc::clone(&provider);
                let rid = route_id.clone();
                async move {
                    let mut stream = p.chat_stream(req);
                    let mut full_text = String::new();
                    let mut final_usage = None;
                    let mut finish_reason = None;
                    let mut final_model = None;
                    let mut tool_calls: Vec<EmittedToolCall> = Vec::new();

                    while let Some(event) = stream.next().await {
                        match event? {
                            ChatEvent::Delta { text } => {
                                if !text.is_empty() {
                                    cs.store(true, Ordering::SeqCst);
                                }
                                full_text.push_str(&text);
                            }
                            ChatEvent::ToolCall {
                                id,
                                name,
                                arguments,
                            } => {
                                cs.store(true, Ordering::SeqCst);
                                tool_calls.push(EmittedToolCall {
                                    id,
                                    name,
                                    arguments,
                                });
                            }
                            ChatEvent::Finished {
                                usage,
                                finish_reason: fr,
                                model,
                                ..
                            } => {
                                final_usage = usage;
                                finish_reason = fr;
                                final_model = model;
                            }
                        }
                    }

                    if !tool_calls.is_empty() {
                        tracing::info!(
                            route_id = rid.as_str(),
                            tool_count = tool_calls.len(),
                            "llm tool calls collected"
                        );
                    }

                    Ok::<_, LlmError>((
                        full_text,
                        final_usage,
                        finish_reason,
                        final_model,
                        tool_calls,
                    ))
                }
            };

            let semaphore = self.semaphore.clone();
            let retry = self.retry.clone();
            let fut = Self::run_with_retry(semaphore, retry, content_started, &self.route_id, op);
            let result = self.with_timeout(fut).await;

            // Leader handle: complete BEFORE propagating to caller.
            // Do NOT use `?` before this — the Drop guard would send a
            // spurious cancellation error instead of the real result.
            if let Some(handle) = leader_handle {
                match &result {
                    Ok((text, usage, _, _, _)) => {
                        handle.complete(Ok((text.clone(), *usage)));
                    }
                    Err(e) => {
                        handle.complete(Err(e.clone()));
                    }
                }
            }

            // Now propagate the result to the caller.
            let (full_text, final_usage, finish_reason, final_model, tool_calls) = result?;

            let headers = &mut exchange.input.headers;
            if tool_calls.is_empty() {
                exchange.input.body = Body::Text(full_text);
            } else {
                if !full_text.is_empty() {
                    headers.insert(CAMEL_LLM_TEXT.to_string(), Value::String(full_text));
                }
                exchange.input.body = Body::Empty;
                headers.insert(
                    CAMEL_LLM_TOOL_CALLS.to_string(),
                    serde_json::to_value(&tool_calls).map_err(|e| {
                        LlmError::Protocol(format!("failed to serialize tool calls: {e}"))
                    })?,
                );
            }
            headers.insert(CAMEL_LLM_USAGE_AVAILABLE.to_string(), Value::Bool(true));
            if let Some(u) = final_usage {
                headers.insert(
                    CAMEL_LLM_TOKENS_IN.to_string(),
                    Value::from(u.prompt_tokens),
                );
                headers.insert(
                    CAMEL_LLM_TOKENS_OUT.to_string(),
                    Value::from(u.completion_tokens),
                );
                // Cost observability: compute estimated cost from pricing
                if let Some(ref pricing) = self.pricing
                    && let Some(cost) = pricing.cost_usd(u.prompt_tokens, u.completion_tokens)
                {
                    headers.insert(CAMEL_LLM_ESTIMATED_COST_USD.to_string(), Value::from(cost));
                    tracing::info!(
                        route_id = %self.route_id,
                        prompt_tokens = u.prompt_tokens,
                        completion_tokens = u.completion_tokens,
                        cost_usd = cost,
                        "llm request cost estimated"
                    );
                }
            }
            if let Some(fr) = finish_reason {
                headers.insert(
                    CAMEL_LLM_FINISH_REASON.to_string(),
                    Value::String(format!("{:?}", fr)),
                );
            }
            if let Some(m) = final_model {
                headers.insert(CAMEL_LLM_MODEL.to_string(), Value::from(m));
            }
        }
        Ok(())
    }

    async fn handle_embed(&self, exchange: &mut Exchange) -> Result<(), LlmError> {
        let prompt = self
            .extract_prompt(exchange)
            .map_err(|e| LlmError::InvalidRequest(e.to_string()))?;
        let model = self
            .config
            .model
            .clone()
            .unwrap_or_else(|| self.provider.default_model().to_string());
        let request = EmbedRequest::new(model, vec![prompt]);

        // Embed uses the retry loop (materialized). content_started is
        // intentionally never set: embed is a single-shot idempotent operation
        // with no streaming content to corrupt, so retry on transient failure
        // is always safe (no "partial output" to protect).
        let provider = Arc::clone(&self.provider);
        let op = move |_content_started: Arc<AtomicBool>| {
            // _content_started ignored: embed has no Delta-based content
            // tracking — every failed embed is a clean retry.
            let req = request.clone();
            let p = Arc::clone(&provider);
            async move { p.embed(req).await }
        };

        let semaphore = self.semaphore.clone();
        let retry = self.retry.clone();
        let content_started = Arc::new(AtomicBool::new(false));
        let fut = Self::run_with_retry(semaphore, retry, content_started, &self.route_id, op);
        let response = self.with_timeout(fut).await?;

        exchange.input.body = Body::Json(
            serde_json::to_value(&response.embeddings)
                .map_err(|e| LlmError::Protocol(format!("failed to serialize embeddings: {e}")))?,
        );
        let headers = &mut exchange.input.headers;
        headers.insert(
            CAMEL_LLM_PROVIDER.to_string(),
            Value::String(self.provider.id().to_string()),
        );
        headers.insert(CAMEL_LLM_USAGE_AVAILABLE.to_string(), Value::Bool(true));
        if let Some(u) = response.usage {
            headers.insert(
                CAMEL_LLM_TOKENS_IN.to_string(),
                Value::from(u.prompt_tokens),
            );
        }
        headers.insert(CAMEL_LLM_MODEL.to_string(), Value::String(response.model));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PermitStream — holds the semaphore permit for streaming responses
// ---------------------------------------------------------------------------

/// A Stream wrapper that holds an optional semaphore permit until the
/// stream is consumed or dropped. Concrete (not generic) — uses `BoxStream`
/// for the inner field so that it remains `Unpin` for use inside the
/// `async_stream::stream!` macro without a `S: Unpin` bound.
///
/// Dropping PermitStream drops the inner provider stream, cancelling
/// upstream work via standard Rust future-drop semantics — no explicit
/// CancellationToken needed (drop IS cancellation).
struct PermitStream {
    inner: futures::stream::BoxStream<'static, Result<Bytes, CamelError>>,
    _permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl futures::Stream for PermitStream {
    type Item = Result<Bytes, CamelError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Safe: PermitStream is Unpin (all fields are Unpin).
        let this = self.get_mut();
        this.inner.as_mut().poll_next(cx)
    }
}

impl Service<Exchange> for LlmProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let producer = self.clone();

        Box::pin(async move {
            let result = match producer.config.operation {
                LlmOperation::Chat => producer.handle_chat(&mut exchange).await,
                LlmOperation::Embed => producer.handle_embed(&mut exchange).await,
            };

            match result {
                Ok(()) => Ok(exchange),
                Err(e) => {
                    tracing::warn!(route_id = %producer.route_id, error = %e, "llm producer error");
                    Err(CamelError::from(e))
                }
            }
        })
    }
}

#[cfg(test)]
#[path = "producer_tests.rs"]
mod tests;
