use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use camel_ai::{ChatMessage, ChatModel, ChatRequest, ChatRole};
use camel_component_api::{
    Body, BoxProcessor, CamelError, Consumer, Endpoint, Exchange, ProducerContext,
};
use tower::Service;

pub struct LlmEndpoint {
    pub uri: String,
    pub model: Arc<dyn ChatModel>,
    pub temperature: Option<f32>,
    pub system_prompt: Option<String>,
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
            temperature: self.temperature,
            system_prompt: self.system_prompt.clone(),
        }))
    }
}

#[derive(Clone)]
struct LlmProducer {
    model: Arc<dyn ChatModel>,
    temperature: Option<f32>,
    system_prompt: Option<String>,
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
        let temperature = self.temperature;
        let system_prompt = self.system_prompt.clone();

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

            let req = ChatRequest {
                messages,
                temperature,
                max_tokens: None,
            };
            let resp = model.complete(req).await?;

            exchange.input.body = Body::Text(resp.content);
            Ok(exchange)
        })
    }
}
