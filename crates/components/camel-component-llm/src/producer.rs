use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use camel_api::{Body, CamelError, Exchange, StreamBody, StreamMetadata};
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::Mutex;
use tower::Service;

use crate::config::{LlmEndpointConfig, LlmOperation};
use crate::error::LlmError;
use crate::headers::*;
use crate::provider::{ChatEvent, ChatMessage, ChatRequest, ChatRole, EmbedRequest, LlmProvider};

#[derive(Clone)]
pub struct LlmProducer {
    config: LlmEndpointConfig,
    provider: Arc<dyn LlmProvider>,
    max_prompt_bytes: usize,
    route_id: String,
}

impl LlmProducer {
    pub fn new(
        config: LlmEndpointConfig,
        provider: Arc<dyn LlmProvider>,
        max_prompt_bytes: usize,
        route_id: String,
    ) -> Self {
        Self {
            config,
            provider,
            max_prompt_bytes,
            route_id,
        }
    }

    fn build_chat_request(&self, prompt: &str, exchange: &Exchange) -> ChatRequest {
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

        let messages = vec![ChatMessage {
            role: ChatRole::User,
            content: prompt.to_string(),
        }];

        ChatRequest {
            model,
            messages,
            temperature,
            max_tokens,
            stop: None,
            system_prompt,
            extra: serde_json::Map::new(),
        }
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

    async fn handle_chat(&self, exchange: &mut Exchange) -> Result<(), LlmError> {
        let prompt = self
            .extract_prompt(exchange)
            .map_err(|e| LlmError::InvalidRequest(e.to_string()))?;
        let request = self.build_chat_request(&prompt, exchange);
        self.set_start_headers(exchange);

        if self.config.stream {
            let route_id = self.route_id.clone();
            let stream = self.provider.chat_stream(request);

            let byte_stream = stream.map(move |event| match event {
                Ok(ChatEvent::Delta { text }) => Ok(Bytes::from(text)),
                Ok(ChatEvent::Finished { usage, .. }) => {
                    if let Some(u) = usage {
                        tracing::info!(
                            route_id = %route_id,
                            prompt_tokens = u.prompt_tokens,
                            completion_tokens = u.completion_tokens,
                            "llm stream finished"
                        );
                    }
                    Ok(Bytes::new())
                }
                Err(e) => {
                    // log-policy: handler-owned
                    tracing::warn!(route_id = %route_id, error = %e, "llm stream error");
                    Err(CamelError::from(e))
                }
            });

            exchange.input.body = Body::Stream(StreamBody {
                stream: Arc::new(Mutex::new(Some(Box::pin(byte_stream)))),
                metadata: StreamMetadata::default(),
            });
        } else {
            let mut full_text = String::new();
            let mut final_usage = None;
            let mut finish_reason = None;
            let mut final_model = None;

            let mut stream = self.provider.chat_stream(request);
            while let Some(event) = stream.next().await {
                match event? {
                    ChatEvent::Delta { text } => full_text.push_str(&text),
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

            exchange.input.body = Body::Text(full_text);
            let headers = &mut exchange.input.headers;
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

        let response = self.provider.embed(request).await?;

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
mod tests {
    use super::*;
    use crate::provider::mock::{MockMode, MockProvider};
    use camel_api::Message;

    fn make_producer(stream: bool, operation: LlmOperation) -> LlmProducer {
        let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hello".into())));
        let config = LlmEndpointConfig {
            operation,
            stream,
            ..Default::default()
        };
        LlmProducer::new(config, provider, 32768, "test-route".into())
    }

    fn make_exchange(body: Body) -> Exchange {
        Exchange::new(Message::new(body))
    }

    // ---- handle_chat (materialized) ----

    #[tokio::test]
    async fn chat_materialized_returns_text() {
        let producer = make_producer(false, LlmOperation::Chat);
        let mut exchange = make_exchange(Body::Text("hello".into()));
        producer.handle_chat(&mut exchange).await.expect("chat ok");
        match &exchange.input.body {
            Body::Text(s) => assert_eq!(s, "hello"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn chat_materialized_sets_usage_available_true() {
        let producer = make_producer(false, LlmOperation::Chat);
        let mut exchange = make_exchange(Body::Text("prompt".into()));
        producer.handle_chat(&mut exchange).await.expect("chat ok");
        assert_eq!(
            exchange.input.headers.get(CAMEL_LLM_USAGE_AVAILABLE),
            Some(&Value::Bool(true))
        );
    }

    #[tokio::test]
    async fn chat_materialized_sets_token_headers() {
        let producer = make_producer(false, LlmOperation::Chat);
        let mut exchange = make_exchange(Body::Text("hello".into()));
        producer.handle_chat(&mut exchange).await.expect("chat ok");
        assert!(exchange.input.headers.contains_key(CAMEL_LLM_TOKENS_IN));
        assert!(exchange.input.headers.contains_key(CAMEL_LLM_TOKENS_OUT));
    }

    #[tokio::test]
    async fn chat_materialized_sets_finish_reason() {
        let producer = make_producer(false, LlmOperation::Chat);
        let mut exchange = make_exchange(Body::Text("hello".into()));
        producer.handle_chat(&mut exchange).await.expect("chat ok");
        assert!(exchange.input.headers.contains_key(CAMEL_LLM_FINISH_REASON));
    }

    // ---- handle_chat (streaming) ----

    #[tokio::test]
    async fn chat_streaming_sets_stream_body() {
        let producer = make_producer(true, LlmOperation::Chat);
        let mut exchange = make_exchange(Body::Text("hello".into()));
        producer.handle_chat(&mut exchange).await.expect("chat ok");
        assert!(matches!(exchange.input.body, Body::Stream(_)));
    }

    #[tokio::test]
    async fn chat_streaming_sets_stream_header() {
        let producer = make_producer(true, LlmOperation::Chat);
        let mut exchange = make_exchange(Body::Text("hello".into()));
        producer.handle_chat(&mut exchange).await.expect("chat ok");
        assert_eq!(
            exchange.input.headers.get(CAMEL_LLM_STREAM),
            Some(&Value::Bool(true))
        );
    }

    #[tokio::test]
    async fn chat_streaming_sets_usage_available_false() {
        let producer = make_producer(true, LlmOperation::Chat);
        let mut exchange = make_exchange(Body::Text("hello".into()));
        producer.handle_chat(&mut exchange).await.expect("chat ok");
        assert_eq!(
            exchange.input.headers.get(CAMEL_LLM_USAGE_AVAILABLE),
            Some(&Value::Bool(false))
        );
    }

    #[tokio::test]
    async fn chat_streaming_sets_provider_header() {
        let producer = make_producer(true, LlmOperation::Chat);
        let mut exchange = make_exchange(Body::Text("hello".into()));
        producer.handle_chat(&mut exchange).await.expect("chat ok");
        assert_eq!(
            exchange.input.headers.get(CAMEL_LLM_PROVIDER),
            Some(&Value::String("test".into()))
        );
    }

    // ---- handle_embed ----

    #[tokio::test]
    async fn embed_returns_json() {
        let producer = make_producer(false, LlmOperation::Embed);
        let mut exchange = make_exchange(Body::Text("hello".into()));
        producer
            .handle_embed(&mut exchange)
            .await
            .expect("embed ok");
        assert!(matches!(exchange.input.body, Body::Json(_)));
    }

    #[tokio::test]
    async fn embed_sets_model_header() {
        let producer = make_producer(false, LlmOperation::Embed);
        let mut exchange = make_exchange(Body::Text("hello".into()));
        producer
            .handle_embed(&mut exchange)
            .await
            .expect("embed ok");
        assert!(exchange.input.headers.contains_key(CAMEL_LLM_MODEL));
    }

    #[tokio::test]
    async fn embed_sets_usage_available() {
        let producer = make_producer(false, LlmOperation::Embed);
        let mut exchange = make_exchange(Body::Text("hello".into()));
        producer
            .handle_embed(&mut exchange)
            .await
            .expect("embed ok");
        assert_eq!(
            exchange.input.headers.get(CAMEL_LLM_USAGE_AVAILABLE),
            Some(&Value::Bool(true))
        );
    }

    #[tokio::test]
    async fn embed_sets_tokens_in_header() {
        let producer = make_producer(false, LlmOperation::Embed);
        let mut exchange = make_exchange(Body::Text("hello".into()));
        producer
            .handle_embed(&mut exchange)
            .await
            .expect("embed ok");
        assert!(exchange.input.headers.contains_key(CAMEL_LLM_TOKENS_IN));
    }

    // ---- extract_prompt ----

    #[test]
    fn extract_prompt_from_text() {
        let producer = make_producer(false, LlmOperation::Chat);
        let exchange = make_exchange(Body::Text("hello".into()));
        let prompt = producer.extract_prompt(&exchange).expect("extract ok");
        assert_eq!(prompt, "hello");
    }

    #[test]
    fn extract_prompt_from_bytes() {
        let producer = make_producer(false, LlmOperation::Chat);
        let exchange = make_exchange(Body::Bytes(Bytes::from("hello")));
        let prompt = producer.extract_prompt(&exchange).expect("extract ok");
        assert_eq!(prompt, "hello");
    }

    #[test]
    fn extract_prompt_from_json() {
        let producer = make_producer(false, LlmOperation::Chat);
        let exchange = make_exchange(Body::Json(serde_json::json!("hello")));
        let prompt = producer.extract_prompt(&exchange).expect("extract ok");
        assert_eq!(prompt, "\"hello\"");
    }

    #[test]
    fn extract_prompt_from_empty_errors() {
        let producer = make_producer(false, LlmOperation::Chat);
        let exchange = make_exchange(Body::Empty);
        assert!(producer.extract_prompt(&exchange).is_err());
    }

    #[test]
    fn extract_prompt_from_stream_errors() {
        let producer = make_producer(false, LlmOperation::Chat);
        let stream_body = StreamBody {
            stream: Arc::new(Mutex::new(None)),
            metadata: StreamMetadata::default(),
        };
        let exchange = make_exchange(Body::Stream(stream_body));
        assert!(producer.extract_prompt(&exchange).is_err());
    }

    #[test]
    fn extract_prompt_enforces_max_bytes() {
        let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
        let config = LlmEndpointConfig::default();
        let producer = LlmProducer::new(config, provider, 3, "route".into());
        let exchange = make_exchange(Body::Text("hello world".into()));
        assert!(producer.extract_prompt(&exchange).is_err());
    }

    #[test]
    fn extract_prompt_allows_at_max_bytes() {
        let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
        let config = LlmEndpointConfig::default();
        let producer = LlmProducer::new(config, provider, 5, "route".into());
        let exchange = make_exchange(Body::Text("hello".into()));
        assert!(producer.extract_prompt(&exchange).is_ok());
    }

    // ---- build_chat_request ----

    #[test]
    fn build_chat_request_falls_back_to_provider_model() {
        let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
        let config = LlmEndpointConfig::default();
        let producer = LlmProducer::new(config, provider, 32768, "route".into());
        let exchange = make_exchange(Body::Text("hello".into()));
        let req = producer.build_chat_request("hello", &exchange);
        assert_eq!(req.model, "mock-model");
    }

    #[test]
    fn build_chat_request_uses_config_model() {
        let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
        let config = LlmEndpointConfig {
            model: Some("gpt-4o".into()),
            ..Default::default()
        };
        let producer = LlmProducer::new(config, provider, 32768, "route".into());
        let exchange = make_exchange(Body::Text("hello".into()));
        let req = producer.build_chat_request("hello", &exchange);
        assert_eq!(req.model, "gpt-4o");
    }

    #[test]
    fn build_chat_request_uses_header_model() {
        let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
        let config = LlmEndpointConfig::default();
        let producer = LlmProducer::new(config, provider, 32768, "route".into());
        let mut exchange = make_exchange(Body::Text("hello".into()));
        exchange
            .input
            .headers
            .insert(CAMEL_LLM_MODEL.into(), Value::String("header-model".into()));
        let req = producer.build_chat_request("hello", &exchange);
        assert_eq!(req.model, "header-model");
    }

    #[test]
    fn build_chat_request_uses_temperature_from_config() {
        let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
        let config = LlmEndpointConfig {
            temperature: Some(0.5),
            ..Default::default()
        };
        let producer = LlmProducer::new(config, provider, 32768, "route".into());
        let exchange = make_exchange(Body::Text("hello".into()));
        let req = producer.build_chat_request("hello", &exchange);
        assert_eq!(req.temperature, Some(0.5));
    }

    #[test]
    fn build_chat_request_uses_max_tokens_from_config() {
        let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
        let config = LlmEndpointConfig {
            max_tokens: Some(100),
            ..Default::default()
        };
        let producer = LlmProducer::new(config, provider, 32768, "route".into());
        let exchange = make_exchange(Body::Text("hello".into()));
        let req = producer.build_chat_request("hello", &exchange);
        assert_eq!(req.max_tokens, Some(100));
    }

    #[test]
    fn build_chat_request_respects_system_prompt_from_config() {
        let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
        let config = LlmEndpointConfig {
            system_prompt: Some("be helpful".into()),
            ..Default::default()
        };
        let producer = LlmProducer::new(config, provider, 32768, "route".into());
        let exchange = make_exchange(Body::Text("hello".into()));
        let req = producer.build_chat_request("hello", &exchange);
        assert_eq!(req.system_prompt.as_deref(), Some("be helpful"));
    }

    #[test]
    fn build_chat_request_includes_user_message() {
        let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
        let config = LlmEndpointConfig::default();
        let producer = LlmProducer::new(config, provider, 32768, "route".into());
        let exchange = make_exchange(Body::Text("hello".into()));
        let req = producer.build_chat_request("hello", &exchange);
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].content, "hello");
        assert_eq!(req.messages[0].role, ChatRole::User);
    }

    // ---- set_start_headers ----

    #[test]
    fn set_start_headers_sets_provider() {
        let producer = make_producer(false, LlmOperation::Chat);
        let mut exchange = make_exchange(Body::Empty);
        producer.set_start_headers(&mut exchange);
        assert_eq!(
            exchange.input.headers.get(CAMEL_LLM_PROVIDER),
            Some(&Value::String("test".into()))
        );
    }

    #[test]
    fn set_start_headers_sets_stream_false() {
        let producer = make_producer(false, LlmOperation::Chat);
        let mut exchange = make_exchange(Body::Empty);
        producer.set_start_headers(&mut exchange);
        assert_eq!(
            exchange.input.headers.get(CAMEL_LLM_STREAM),
            Some(&Value::Bool(false))
        );
    }

    #[test]
    fn set_start_headers_sets_stream_true() {
        let producer = make_producer(true, LlmOperation::Chat);
        let mut exchange = make_exchange(Body::Empty);
        producer.set_start_headers(&mut exchange);
        assert_eq!(
            exchange.input.headers.get(CAMEL_LLM_STREAM),
            Some(&Value::Bool(true))
        );
    }

    #[test]
    fn set_start_headers_usage_available_starts_false() {
        let producer = make_producer(true, LlmOperation::Chat);
        let mut exchange = make_exchange(Body::Empty);
        producer.set_start_headers(&mut exchange);
        assert_eq!(
            exchange.input.headers.get(CAMEL_LLM_USAGE_AVAILABLE),
            Some(&Value::Bool(false))
        );
    }

    #[test]
    fn set_start_headers_skips_model_when_not_configured() {
        let producer = make_producer(false, LlmOperation::Chat);
        let mut exchange = make_exchange(Body::Empty);
        producer.set_start_headers(&mut exchange);
        assert!(!exchange.input.headers.contains_key(CAMEL_LLM_MODEL));
    }

    #[test]
    fn set_start_headers_sets_model_when_configured() {
        let provider = Arc::new(MockProvider::new("test", MockMode::Fixed("hi".into())));
        let config = LlmEndpointConfig {
            model: Some("gpt-4o".into()),
            ..Default::default()
        };
        let producer = LlmProducer::new(config, provider, 32768, "route".into());
        let mut exchange = make_exchange(Body::Empty);
        producer.set_start_headers(&mut exchange);
        assert_eq!(
            exchange.input.headers.get(CAMEL_LLM_MODEL),
            Some(&Value::String("gpt-4o".into()))
        );
    }
}
