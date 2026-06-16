use async_trait::async_trait;
use futures::stream::BoxStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crate::error::LlmError;
use crate::provider::{
    ChatEvent, ChatRequest, ChatRole, EmbedRequest, EmbedResponse, FinishReason, LlmProvider,
    LlmUsage,
};

/// Mock provider mode.
#[derive(Debug, Clone)]
pub enum MockMode {
    /// Echo user messages back verbatim.
    Echo,
    /// Return a fixed response.
    Fixed(String),
    /// Stream a partial delta then error — for mid-stream failure testing.
    Error(LlmError),
}

/// A mock LLM provider for testing.
///
/// # Modes
/// - `Echo`: streams back the concatenation of all user messages.
/// - `Fixed(String)`: streams back the given canned text.
/// - `Error(LlmError)`: sends one Delta event, then the error.
///
/// # Deterministic test controls
/// All optional, all default-off. Chain builder methods to configure:
/// - `with_delay`: sleep before emitting any events.
/// - `with_fail_after`: fail on the Nth call.
/// - `with_rate_limit`: always emit a RateLimit error.
/// - `call_count`: query how many times `chat_stream` has been called (always tracked).
/// - `with_concurrent_tracker`: track max in-flight `chat_stream`.
/// - `with_cancellation_tracking`: record whether a stream was dropped early.
pub struct MockProvider {
    id: String,
    mode: MockMode,
    default_model: String,
    /// Delay before emitting events.
    delay: Option<Duration>,
    /// Fail on the Nth invocation (1-based) with the given error.
    fail_after: Option<(usize, LlmError)>,
    /// Always emit a RateLimit error (after any delay), carrying retry_after.
    rate_limit: Option<Option<Duration>>,
    /// Call counter.
    calls: AtomicUsize,
    /// In-flight concurrent counter and peak.
    concurrent: Arc<AtomicUsize>,
    max_concurrent: Arc<AtomicUsize>,
    track_concurrent: bool,
    /// Whether any stream was cancelled (dropped before completion).
    cancelled: Arc<AtomicBool>,
    track_cancel: bool,
}

// ---------------------------------------------------------------------------
// Drop guard that records cancellation when a stream is dropped mid-flight
// ---------------------------------------------------------------------------

struct CancelGuard {
    cancelled: Option<Arc<AtomicBool>>,
    finished: Arc<AtomicBool>,
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        let Some(b) = &self.cancelled else { return };
        if !self.finished.load(Ordering::SeqCst) {
            b.store(true, Ordering::SeqCst);
        }
    }
}

// ---------------------------------------------------------------------------
// RAII guard that increments concurrent on construction and decrements on Drop
// ---------------------------------------------------------------------------

struct ConcurrentGuard {
    concurrent: Arc<AtomicUsize>,
    track: bool,
}

impl ConcurrentGuard {
    fn new(concurrent: Arc<AtomicUsize>, max_concurrent: Arc<AtomicUsize>, track: bool) -> Self {
        if track {
            let cur = concurrent.fetch_add(1, Ordering::SeqCst) + 1;
            max_concurrent.fetch_max(cur, Ordering::SeqCst);
        }
        Self { concurrent, track }
    }
}

impl Drop for ConcurrentGuard {
    fn drop(&mut self) {
        if self.track {
            self.concurrent.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl MockProvider {
    /// Create a mock provider with the given id and mode.
    ///
    /// The default model is `"mock-model"`.
    pub fn new(id: impl Into<String>, mode: MockMode) -> Self {
        Self {
            id: id.into(),
            mode,
            default_model: "mock-model".to_string(),
            delay: None,
            fail_after: None,
            rate_limit: None,
            calls: AtomicUsize::new(0),
            concurrent: Arc::new(AtomicUsize::new(0)),
            max_concurrent: Arc::new(AtomicUsize::new(0)),
            track_concurrent: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            track_cancel: false,
        }
    }

    /// Set the default model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = model.into();
        self
    }

    /// Shortcut for `MockProvider::new("mock", MockMode::Echo)`.
    pub fn echo() -> Self {
        Self::new("mock", MockMode::Echo)
    }

    // -----------------------------------------------------------------------
    // Deterministic test controls
    // -----------------------------------------------------------------------

    /// Add a delay before emitting any events.
    pub fn with_delay(mut self, d: Duration) -> Self {
        self.delay = Some(d);
        self
    }

    /// Fail on the Nth invocation (1-based) with the given error.
    pub fn with_fail_after(mut self, n: usize, e: LlmError) -> Self {
        self.fail_after = Some((n, e));
        self
    }

    /// Always emit a `RateLimit` error.  `retry_after` is the duration the
    /// caller should wait before retrying (`None` means the provider did not
    /// specify a wait).
    pub fn with_rate_limit(mut self, retry_after: Option<Duration>) -> Self {
        self.rate_limit = Some(retry_after);
        self
    }

    /// Track the number of in-flight `chat_stream` invocations and the
    /// peak concurrent count.
    pub fn with_concurrent_tracker(mut self) -> Self {
        self.track_concurrent = true;
        self
    }

    /// Track whether any stream produced by this provider was dropped
    /// (cancelled) before reaching a terminal event.
    pub fn with_cancellation_tracking(mut self) -> Self {
        self.track_cancel = true;
        self
    }

    /// Number of times `chat_stream` has been called.
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Peak number of concurrent `chat_stream` invocations.
    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent.load(Ordering::SeqCst)
    }

    /// Whether any stream was cancelled (dropped before completion).
    pub fn was_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn chat_stream(&self, req: ChatRequest) -> BoxStream<'static, Result<ChatEvent, LlmError>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let delay = self.delay;
        let fail = self
            .fail_after
            .as_ref()
            .and_then(|(at, e)| if n == *at { Some(e.clone()) } else { None });
        let rl = self
            .rate_limit
            .map(|retry_after| LlmError::RateLimit { retry_after });

        let concurrent = Arc::clone(&self.concurrent);
        let max_concurrent = Arc::clone(&self.max_concurrent);
        let cancelled = Arc::clone(&self.cancelled);
        let track_concurrent = self.track_concurrent;
        let track_cancel = self.track_cancel;
        let mode = self.mode.clone();
        let model = req.model.clone();

        let s = async_stream::stream! {
            // Guards at the TOP so they cover delay and every exit path.
            let _conc_guard = ConcurrentGuard::new(
                Arc::clone(&concurrent),
                Arc::clone(&max_concurrent),
                track_concurrent,
            );
            let finished = Arc::new(AtomicBool::new(false));
            let _cancel_guard = CancelGuard {
                cancelled: track_cancel.then(|| Arc::clone(&cancelled)),
                finished: Arc::clone(&finished),
            };

            if let Some(d) = delay {
                tokio::time::sleep(d).await;
            }

            // Rate-limit simulation (always fires if configured)
            if let Some(e) = rl {
                finished.store(true, Ordering::SeqCst);
                yield Err(e);
                return;
            }

            // Fail-after simulation
            if let Some(e) = fail {
                finished.store(true, Ordering::SeqCst);
                yield Err(e);
                return;
            }

            let text = match &mode {
                MockMode::Echo => req
                    .messages
                    .iter()
                    .filter(|m| matches!(m.role, ChatRole::User))
                    .map(|m| m.content.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
                MockMode::Fixed(t) => t.clone(),
                MockMode::Error(e) => {
                    yield Ok(ChatEvent::Delta { text: "partial".into() });
                    finished.store(true, Ordering::SeqCst);
                    yield Err(e.clone());
                    return;
                }
            };

            let pt = text.len() as u32 / 4;
            yield Ok(ChatEvent::Delta { text });
            finished.store(true, Ordering::SeqCst);
            yield Ok(ChatEvent::Finished {
                usage: Some(LlmUsage {
                    prompt_tokens: pt,
                    completion_tokens: pt,
                    total_tokens: pt * 2,
                }),
                model: Some(model),
                finish_reason: Some(FinishReason::Stop),
                metadata: serde_json::Map::new(),
            });
        };
        Box::pin(s)
    }

    async fn embed(&self, req: EmbedRequest) -> Result<EmbedResponse, LlmError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;

        // Delay before error checks (same ordering as chat_stream —
        // simulates network latency before the response is determined).
        if let Some(d) = self.delay {
            tokio::time::sleep(d).await;
        }

        // Rate-limit simulation (always fires if configured).
        if let Some(retry_after) = self.rate_limit {
            return Err(LlmError::RateLimit { retry_after });
        }

        // Check fail_after (same semantics as chat_stream).
        if let Some((at, ref e)) = self.fail_after
            && n == at
        {
            return Err(e.clone());
        }

        let embeddings: Vec<Vec<f32>> = req
            .inputs
            .iter()
            .map(|input| {
                let hash = input
                    .bytes()
                    .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
                (0..16).map(|i| ((hash >> i) & 1) as f32).collect()
            })
            .collect();

        let prompt_tokens: u32 = req.inputs.iter().map(|i| i.len() as u32 / 4).sum();
        Ok(EmbedResponse {
            embeddings,
            usage: Some(LlmUsage {
                prompt_tokens,
                completion_tokens: 0,
                total_tokens: prompt_tokens,
            }),
            model: req.model,
            metadata: serde_json::Map::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ChatMessage;
    use futures::StreamExt;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn echo_provider_streams_prompt_back() {
        let provider = MockProvider::new("test", MockMode::Echo);
        let req = ChatRequest::new("mock-model", vec![ChatMessage::user("hello world")]);
        let mut stream = provider.chat_stream(req);

        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event.expect("stream ok") {
                ChatEvent::Delta { text: t } => text.push_str(&t),
                ChatEvent::Finished { .. } => {}
            }
        }
        assert_eq!(text, "hello world");
    }

    #[tokio::test]
    async fn fixed_provider_returns_fixed_text() {
        let provider = MockProvider::new("test", MockMode::Fixed("canned response".into()));
        let req = ChatRequest::new("mock-model", vec![ChatMessage::user("anything")]);
        let mut stream = provider.chat_stream(req);

        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event.expect("stream ok") {
                ChatEvent::Delta { text: t } => text.push_str(&t),
                ChatEvent::Finished { .. } => {}
            }
        }
        assert_eq!(text, "canned response");
    }

    #[tokio::test]
    async fn mock_provider_embeds() {
        let provider = MockProvider::new("test", MockMode::Fixed("irrelevant".into()));
        let req = EmbedRequest::new("mock-model", vec!["hello".into()]);
        let resp = provider.embed(req).await.expect("embed ok");
        assert_eq!(resp.embeddings.len(), 1);
        assert_eq!(
            resp.embeddings[0].len(),
            16,
            "embedding must be 16-dimensional"
        );
        // Verify binary range (all values are 0.0 or 1.0)
        for &val in &resp.embeddings[0] {
            assert!(
                val == 0.0 || val == 1.0,
                "embedding value must be binary, got {val}"
            );
        }
        // Verify determinism: same input → same output
        let req2 = EmbedRequest::new("mock-model", vec!["hello".into()]);
        let resp2 = provider.embed(req2).await.expect("embed ok 2");
        assert_eq!(
            resp.embeddings[0], resp2.embeddings[0],
            "embeddings must be deterministic"
        );
    }

    #[tokio::test]
    async fn error_mode_emits_partial_then_error() {
        let provider = MockProvider::new(
            "test",
            MockMode::Error(LlmError::ProviderUnavailable("service down".into())),
        );
        let req = ChatRequest::new("mock-model", vec![ChatMessage::user("hello")]);
        let mut stream = provider.chat_stream(req);

        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }

        assert_eq!(events.len(), 2, "should emit exactly 2 events");
        assert!(matches!(events[0], Ok(ChatEvent::Delta { ref text }) if text == "partial"));
        assert!(matches!(events[1], Err(LlmError::ProviderUnavailable(_))));
    }

    #[tokio::test]
    async fn finished_event_has_usage() {
        let provider = MockProvider::new("test", MockMode::Fixed("hi".into()));
        let req = ChatRequest::new("mock-model", vec![ChatMessage::user("hi")]);
        let mut stream = provider.chat_stream(req);

        let mut found_usage = false;
        while let Some(event) = stream.next().await {
            if let ChatEvent::Finished { usage, .. } = event.expect("ok") {
                assert!(usage.is_some());
                found_usage = true;
            }
        }
        assert!(found_usage);
    }

    #[tokio::test]
    async fn delay_mode_emits_events_after_delay() {
        let provider = MockProvider::new("t", MockMode::Fixed("hi".into()))
            .with_delay(Duration::from_millis(50));
        let start = std::time::Instant::now();
        let mut s = provider.chat_stream(ChatRequest::new("m", vec![ChatMessage::user("x")]));
        while s.next().await.is_some() {}
        assert!(start.elapsed() >= Duration::from_millis(45));
    }

    #[tokio::test]
    async fn fail_after_succeeds_then_fails_at_n() {
        let provider = MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_fail_after(2, LlmError::Network("boom".into()));
        let r1 = collect_chat(&provider).await;
        let r2 = collect_chat(&provider).await;
        assert!(r1.is_ok());
        assert!(matches!(r2.unwrap_err(), LlmError::Network(_)));
    }

    #[tokio::test]
    async fn rate_limit_mode_carries_retry_after() {
        let provider = MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_rate_limit(Some(Duration::from_millis(10)));
        let err = collect_chat(&provider).await.unwrap_err();
        assert!(matches!(
            err,
            LlmError::RateLimit {
                retry_after: Some(_)
            }
        ));
    }

    #[tokio::test]
    async fn call_count_tracks_invocations() {
        let provider = MockProvider::echo();
        let _ = collect_chat(&provider).await;
        let _ = collect_chat(&provider).await;
        assert_eq!(provider.call_count(), 2);
    }

    #[tokio::test]
    async fn concurrent_tracker_peaks_at_inflight() {
        let provider = MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_delay(Duration::from_millis(50))
            .with_concurrent_tracker();
        let p = Arc::new(provider);
        let mut handles = vec![];
        for _ in 0..3 {
            let p = Arc::clone(&p);
            handles.push(tokio::spawn(async move { collect_chat(&p).await }));
        }
        for h in handles {
            let _ = h.await;
        }
        assert!(p.max_concurrent() >= 2);
    }

    #[tokio::test]
    async fn cancellation_tracker_records_drop() {
        let provider = MockProvider::new("t", MockMode::Fixed("ok".into()))
            .with_delay(Duration::from_millis(100))
            .with_cancellation_tracking();
        let mut s = provider.chat_stream(ChatRequest::new("m", vec![ChatMessage::user("x")]));
        let _ = s.next().await;
        drop(s);
        assert!(provider.was_cancelled());
    }

    #[tokio::test]
    async fn completed_stream_not_marked_cancelled() {
        let provider =
            MockProvider::new("t", MockMode::Fixed("ok".into())).with_cancellation_tracking();
        let mut s = provider.chat_stream(ChatRequest::new("m", vec![ChatMessage::user("x")]));
        while let Some(_) = s.next().await {} // drain to completion
        assert!(
            !provider.was_cancelled(),
            "a fully-consumed stream must NOT report cancellation"
        );
    }

    async fn collect_chat(p: &MockProvider) -> Result<(), LlmError> {
        let mut s = p.chat_stream(ChatRequest::new("m", vec![ChatMessage::user("x")]));
        while let Some(ev) = s.next().await {
            ev?;
        }
        Ok(())
    }
}
