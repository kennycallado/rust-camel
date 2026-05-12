use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use camel_ai::{ChatMessage, ChatModel, ChatRequest, ChatRole, set_ai_headers};
use camel_component_api::{
    Body, BoxProcessor, CamelError, Consumer, Endpoint, Exchange, ProducerContext,
};
use tower::Service;

pub struct LlmEndpoint {
    pub uri: String,
    pub model: Arc<dyn ChatModel>,
    pub provider_name: String,
    pub model_name: String,
    pub temperature: Option<f32>,
    pub system_prompt: Option<String>,
    pub think: Option<bool>,
}

impl Endpoint for LlmEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::RouteError(
            "llm: component is producer-only".into(),
        ))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(LlmProducer {
            model: Arc::clone(&self.model),
            provider_name: self.provider_name.clone(),
            model_name: self.model_name.clone(),
            temperature: self.temperature,
            system_prompt: self.system_prompt.clone(),
            think: self.think,
        }))
    }
}

#[derive(Clone)]
struct LlmProducer {
    model: Arc<dyn ChatModel>,
    provider_name: String,
    model_name: String,
    temperature: Option<f32>,
    system_prompt: Option<String>,
    think: Option<bool>,
}

impl Service<Exchange> for LlmProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let model = Arc::clone(&self.model);
        let provider_name = self.provider_name.clone();
        let model_name = self.model_name.clone();
        let temperature = self.temperature;
        let system_prompt = self.system_prompt.clone();
        let think = self.think;

        Box::pin(async move {
            let body_str = exchange
                .input
                .body
                .as_text()
                .ok_or_else(|| CamelError::TypeConversionFailed("body is not text".into()))?
                .to_string();

            let mut messages = Vec::new();
            if let Some(sp) = system_prompt {
                messages.push(ChatMessage {
                    role: ChatRole::System,
                    content: sp,
                });
            }
            messages.push(ChatMessage {
                role: ChatRole::User,
                content: body_str,
            });

            let start = Instant::now();
            let req = ChatRequest {
                messages,
                temperature,
                max_tokens: None,
                think,
            };
            let resp = model.complete(req).await?;
            let latency_ms = start.elapsed().as_millis() as u64;

            set_ai_headers(
                &mut exchange.input.headers,
                &provider_name,
                &model_name,
                "llm",
                latency_ms,
                resp.usage.as_ref(),
            );

            exchange.input.body = Body::Text(resp.content);
            Ok(exchange)
        })
    }
}
